//! Differential testing against Terser's own compress test suite.
//!
//! The generator in [`crate::generator`] explores a space we chose. Terser's
//! `test/compress` is a space *minifier authors* chose: a few thousand programs
//! written specifically to break a compressor, accumulated over a decade of
//! UglifyJS and Terser bug reports. Running them through oxc costs nothing to
//! author and covers shapes a random generator reaches rarely or never.
//!
//! This mirrors esbuild's `scripts/terser-tests.js`, which does the same thing
//! for esbuild. Like esbuild, the per-test `options = { ... }` block is ignored:
//! the corpus supplies inputs, and the comparison is our own — run the input,
//! run the compressed input, require the same observable behavior.
//!
//! <https://github.com/evanw/esbuild/blob/main/scripts/terser-tests.js>

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use oxc_allocator::Allocator;
use oxc_ast::ast::Statement;
use oxc_parser::Parser;
use oxc_span::SourceType;

use crate::{
    minify,
    oracle::{Comparison, Oracle},
};

/// Pinned so a corpus change is a deliberate, reviewable step rather than
/// something that happens on its own the next time the tests run.
const TERSER_VERSION: &str = "v5.50.0";

/// Root of the extracted suite (gitignored), holding `test/compress` and a
/// `.version` stamp written after a successful extraction.
#[must_use]
pub fn terser_suite_root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/terser"))
}

/// Ensure the suite matches [`TERSER_VERSION`] and return its root.
///
/// Convergent: a matching `.version` stamp returns without touching the
/// network, anything else wipes and re-extracts. The stamp is written last, so
/// a half-extracted tree is always redone.
///
/// # Errors
///
/// Any download or extraction failure, as a display string.
pub fn ensure_terser_suite() -> Result<&'static Path, String> {
    static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    RESULT.get_or_init(provision).clone()?;
    Ok(terser_suite_root())
}

fn provision() -> Result<(), String> {
    let root = terser_suite_root();
    let stamp = root.join(".version");
    fs::create_dir_all(root).map_err(|error| format!("create {}: {error}", root.display()))?;

    // Advisory lock so parallel test binaries do not extract over each other.
    let lock_path = root.join(".lock");
    let lock = File::create(&lock_path)
        .map_err(|error| format!("create {}: {error}", lock_path.display()))?;
    lock.lock().map_err(|error| format!("lock {}: {error}", lock_path.display()))?;

    if fs::read_to_string(&stamp).is_ok_and(|stamped| stamped.trim() == TERSER_VERSION) {
        return Ok(());
    }

    let compress = root.join("test").join("compress");
    let _ = fs::remove_dir_all(&compress);
    let _ = fs::remove_file(&stamp);

    let tarball = std::env::temp_dir().join(format!("oxc-terser-{TERSER_VERSION}.tar.gz"));
    let url =
        format!("https://codeload.github.com/terser/terser/tar.gz/refs/tags/{TERSER_VERSION}");
    run("curl", &["-fsSL", "-o", &tarball.to_string_lossy(), &url], root)?;
    // `v5.50.0` tags extract into `terser-5.50.0/`.
    let prefix = format!("terser-{}", TERSER_VERSION.trim_start_matches('v'));
    run(
        "tar",
        &[
            "-xzf",
            &tarball.to_string_lossy(),
            "--strip-components=1",
            &format!("{prefix}/test/compress"),
        ],
        root,
    )?;
    let _ = fs::remove_file(&tarball);

    fs::write(&stamp, TERSER_VERSION)
        .map_err(|error| format!("write {}: {error}", stamp.display()))?;
    Ok(())
}

fn run(program: &str, args: &[&str], cwd: &Path) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("spawn {program}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// One executable case lifted out of a Terser test file.
#[derive(Debug, Clone)]
pub struct CorpusTest {
    pub file: String,
    pub name: String,
    pub source: String,
}

/// Extract the runnable cases from one Terser test file.
///
/// The file is not JavaScript that anyone runs — it is a DSL that happens to
/// parse as JavaScript, which is why parsing it with a real parser is the
/// simplest way to read it. Each case is a labelled block:
///
/// ```text
/// dead_code_2: {
///     options = { dead_code: true }
///     input: { ...the program... }
///     expect: { ...what Terser produces... }
///     expect_stdout: true
/// }
/// ```
///
/// Only cases carrying `expect_stdout` are returned. The rest compare Terser's
/// output text against an expectation and are not written to be executed: they
/// call undeclared functions, so running one would only ever throw.
#[must_use]
pub fn parse_tests(file: &str, source: &str) -> Vec<CorpusTest> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::script()).parse();
    if parsed.panicked {
        return Vec::new();
    }

    let mut tests = Vec::new();
    for statement in &parsed.program.body {
        let Statement::LabeledStatement(test) = statement else { continue };
        let Statement::BlockStatement(body) = &test.body else { continue };

        let mut input = None;
        let mut runnable = false;
        for entry in &body.body {
            let Statement::LabeledStatement(entry) = entry else { continue };
            match entry.label.name.as_str() {
                "input" => {
                    if let Statement::BlockStatement(block) = &entry.body {
                        // Strip the braces the DSL wraps the program in.
                        let start = block.span.start as usize + 1;
                        let end = block.span.end as usize - 1;
                        input = source.get(start..end).map(str::to_owned);
                    }
                }
                "expect_stdout" => runnable = true,
                _ => {}
            }
        }

        if let Some(source) = input.filter(|_| runnable) {
            tests.push(CorpusTest {
                file: file.to_owned(),
                name: test.label.name.to_string(),
                source,
            });
        }
    }
    tests
}

/// Read every `test/compress/*.js` under `root` and return the runnable cases.
///
/// # Errors
///
/// Any failure to read the suite directory.
pub fn collect_tests(root: &Path) -> Result<Vec<CorpusTest>, String> {
    let directory = root.join("test").join("compress");
    let read_err = |error| format!("read {}: {error}", directory.display());
    let mut paths: Vec<PathBuf> = fs::read_dir(&directory)
        .map_err(read_err)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(read_err))
        .collect::<Result<_, _>>()?;
    // Directory order is not stable across machines, and a corpus run should
    // report the same thing everywhere.
    paths.sort();

    let mut tests = Vec::new();
    for path in paths {
        if path.extension().is_none_or(|extension| extension != "js") {
            continue;
        }
        let Ok(source) = fs::read_to_string(&path) else { continue };
        let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        tests.extend(parse_tests(&file, &source));
    }
    Ok(tests)
}

#[derive(Debug, Clone)]
pub struct CorpusFailure {
    pub file: String,
    pub name: String,
    pub source: String,
    pub minified: String,
    pub comparison: Comparison,
}

#[derive(Debug, Default)]
pub struct CorpusSummary {
    pub checked: usize,
    /// Cases whose original does not complete under our sandbox — they need
    /// `require`, or they throw by design. Nothing can be concluded from those.
    pub skipped: usize,
    /// Cases oxc's parser rejects. Terser's suite includes syntax oxc does not
    /// accept in a script, so this is expected to be non-zero; it is reported
    /// rather than folded into `skipped` because a rise means a parser gap.
    pub unparsed: usize,
    pub mismatches: Vec<CorpusFailure>,
}

/// Compress every runnable case and compare its behavior with the original.
#[must_use]
pub fn check(
    tests: &[CorpusTest],
    mangle: bool,
    timeout_ms: u64,
    batch_size: usize,
) -> CorpusSummary {
    let oracle = Oracle::new(timeout_ms);
    let mut summary = CorpusSummary::default();

    for batch in tests.chunks(batch_size.max(1)) {
        let mut pairs = Vec::with_capacity(batch.len());
        for test in batch {
            match minify(&test.source, mangle) {
                Ok(minified) => pairs.push((test, minified.code)),
                Err(_) => summary.unparsed += 1,
            }
        }

        let cases: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(test, minified)| (test.source.as_str(), minified.as_str()))
            .collect();
        for ((test, minified), comparison) in pairs.iter().zip(oracle.compare_many(&cases)) {
            match comparison {
                Comparison::Equivalent { .. } => summary.checked += 1,
                Comparison::Skipped { .. } | Comparison::HarnessError { .. } => {
                    summary.skipped += 1;
                }
                Comparison::Mismatch { .. } => summary.mismatches.push(CorpusFailure {
                    file: test.file.clone(),
                    name: test.name.clone(),
                    source: test.source.clone(),
                    minified: minified.clone(),
                    comparison,
                }),
            }
        }
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::parse_tests;

    const SAMPLE: &str = r"
dead_code_1: {
    options = {
        dead_code: true,
    }
    input: {
        function f() { return 1; }
        console.log(f());
    }
    expect: {
        function f() { return 1; }
        console.log(f());
    }
    expect_stdout: true
}

not_runnable: {
    options = {
        dead_code: true,
    }
    input: {
        a();
    }
    expect: {
        a();
    }
}
";

    #[test]
    fn lifts_the_input_out_of_a_runnable_case() {
        let tests = parse_tests("sample.js", SAMPLE);
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].name, "dead_code_1");
        assert_eq!(tests[0].file, "sample.js");
        assert!(tests[0].source.contains("function f() { return 1; }"));
        assert!(tests[0].source.contains("console.log(f())"));
        // The braces the DSL wraps the program in must not survive, or the
        // program would be one block rather than a sequence of statements.
        assert!(!tests[0].source.trim().starts_with('{'));
    }

    #[test]
    fn ignores_cases_that_were_never_meant_to_run() {
        let tests = parse_tests("sample.js", SAMPLE);
        assert!(tests.iter().all(|test| test.name != "not_runnable"));
    }

    /// Provisioning needs the network, so this does not run by default. The
    /// comparison itself is the binary's `--corpus` mode.
    #[test]
    #[ignore = "downloads Terser's test suite"]
    fn the_suite_provisions_and_yields_runnable_cases() {
        let root = super::ensure_terser_suite().expect("provision Terser's suite");
        let tests = super::collect_tests(root).expect("read Terser's suite");
        assert!(tests.len() > 1_000, "only {} runnable cases", tests.len());
    }

    #[test]
    fn a_file_that_is_not_the_dsl_yields_nothing_rather_than_garbage() {
        assert!(parse_tests("x.js", "console.log(1);").is_empty());
        assert!(parse_tests("x.js", "this is not javascript {{{").is_empty());
    }
}
