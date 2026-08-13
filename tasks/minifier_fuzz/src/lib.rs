pub mod campaign;
pub mod context;
pub mod corpus;
pub mod generator;
pub mod invariants;
pub mod oracle;
pub mod scopes;
pub mod shrink;

use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_minifier::{CompressOptions, MangleOptions, Minifier, MinifierOptions};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

#[derive(Debug, Clone)]
pub struct Minified {
    pub code: String,
    pub iterations: u8,
}

/// Compress `source` with `CompressOptions::smallest()` and print it back out.
///
/// With `mangle`, bindings are renamed as well, which also exercises the
/// mangler's scope analysis. Generated programs only observe behavior through
/// their final `console.log`, so renaming must not change the comparison.
///
/// # Errors
///
/// Returns a message when the generated source fails to parse or fails semantic analysis.
pub fn minify(source: &str, mangle: bool) -> Result<Minified, String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::script()).parse();
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        return Err(format!("parser rejected generated input: {:?}", parsed.diagnostics));
    }

    let mut program = parsed.program;
    let result = Minifier::new(MinifierOptions {
        mangle: mangle.then(|| MangleOptions { top_level: Some(true), ..MangleOptions::default() }),
        compress: Some(CompressOptions::smallest()),
    })
    .minify(&allocator, &mut program);
    let code = Codegen::new()
        .with_options(CodegenOptions::minify())
        .with_scoping(result.scoping)
        .build(&program)
        .code;
    Ok(Minified { code, iterations: result.iterations })
}

/// Names `source` refers to without declaring them.
///
/// Used to keep a reduction faithful: dropping a declaration while keeping its
/// uses turns them into references to something that is not there, and the
/// reduced program then fails for a reason the original never had.
#[must_use]
pub fn unresolved_names(source: &str) -> Vec<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::script()).parse();
    if parsed.panicked {
        return Vec::new();
    }
    let semantic = SemanticBuilder::new().build(&parsed.program);
    let mut names: Vec<String> = semantic
        .semantic
        .scoping()
        .root_unresolved_references()
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Parse `source` and run semantic analysis over it, reporting either failure.
///
/// Used on the minifier's *output*: code generation that emits something the
/// parser rejects, or that binds names inconsistently, is a defect no runtime
/// comparison would ever reach.
///
/// # Errors
///
/// Returns a message describing the parse or semantic diagnostics.
pub fn check_syntax_and_semantics(source: &str) -> Result<(), String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::script()).parse();
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        return Err(format!("does not parse: {:?}", parsed.diagnostics));
    }
    let semantic = SemanticBuilder::new().build(&parsed.program);
    if semantic.diagnostics.is_empty() {
        Ok(())
    } else {
        Err(format!("fails semantic analysis: {:?}", semantic.diagnostics))
    }
}

#[cfg(test)]
mod tests {
    use oxc_allocator::Allocator;
    use oxc_parser::Parser;
    use oxc_semantic::SemanticBuilder;
    use oxc_span::SourceType;

    use crate::{
        generator::generate,
        minify,
        oracle::{Comparison, Oracle},
    };

    #[test]
    fn same_seed_generates_same_program() {
        assert_eq!(generate(42), generate(42));
        assert_ne!(generate(42), generate(43));
    }

    #[test]
    fn generated_programs_parse_and_have_valid_semantics() {
        for seed in 0..500 {
            let source = generate(seed);
            let allocator = Allocator::default();
            let parsed = Parser::new(&allocator, &source, SourceType::script()).parse();
            assert!(!parsed.panicked, "seed {seed}\n{source}");
            assert!(parsed.diagnostics.is_empty(), "seed {seed}\n{source}");

            let semantic = SemanticBuilder::new().build(&parsed.program);
            assert!(semantic.diagnostics.is_empty(), "seed {seed}\n{source}");
        }
    }

    #[test]
    fn oracle_accepts_equivalent_programs_and_rejects_changed_output() {
        let oracle = Oracle::new(100);
        assert!(matches!(
            oracle.compare("console.log(1)", "console.log(1)"),
            Comparison::Equivalent { .. }
        ));
        assert!(matches!(
            oracle.compare("console.log(1)", "console.log(2)"),
            Comparison::Mismatch { .. }
        ));
    }

    #[test]
    fn oracle_skips_inputs_that_do_not_complete() {
        let oracle = Oracle::new(25);
        assert!(matches!(
            oracle.compare("for (;;) {}", "console.log(1)"),
            Comparison::Skipped { .. }
        ));
    }

    /// `s += s` in a loop doubles a string, so a program can produce hundreds
    /// of megabytes of output in a few dozen cheap iterations. Encoding that
    /// verbatim overflows the maximum string length once a batch is
    /// serialised, which takes down the whole campaign rather than one seed.
    #[test]
    fn oracle_handles_programs_with_enormous_output() {
        let huge = "var s = 'x'; for (var i = 0; i < 24; i++) s += s; console.log(s, [s, s]);";
        let different = "var s = 'y'; for (var i = 0; i < 24; i++) s += s; console.log(s, [s, s]);";
        let oracle = Oracle::new(5_000);
        assert!(matches!(oracle.compare(huge, huge), Comparison::Equivalent { .. }));
        // Summarising must not make two different large values look equal.
        assert!(matches!(oracle.compare(huge, different), Comparison::Mismatch { .. }));
    }

    #[test]
    fn generated_programs_keep_their_observable_behavior_after_minification() {
        check_generated_programs(false);
    }

    #[test]
    fn generated_programs_keep_their_observable_behavior_after_mangling() {
        check_generated_programs(true);
    }

    fn check_generated_programs(mangle: bool) {
        const SEEDS: u64 = 50;
        let programs: Vec<_> = (0..SEEDS)
            .map(|seed| {
                let original = generate(seed);
                let minified = minify(&original, mangle)
                    .unwrap_or_else(|error| panic!("seed {seed}: {error}"));
                (seed, original, minified.code)
            })
            .collect();
        let cases: Vec<_> = programs
            .iter()
            .map(|(_, original, minified)| (original.as_str(), minified.as_str()))
            .collect();

        let mut skipped = 0;
        for ((seed, original, minified), comparison) in
            programs.iter().zip(Oracle::new(200).compare_many(&cases))
        {
            if matches!(comparison, Comparison::Skipped { .. }) {
                skipped += 1;
                continue;
            }
            assert!(
                matches!(comparison, Comparison::Equivalent { .. }),
                "seed {seed} (mangle={mangle}): {comparison:#?}\noriginal:\n{original}\nminified:\n{minified}"
            );
        }
        // A program whose original throws or times out proves nothing, so a
        // generator that mostly emits those is silently testing nothing. This
        // is a guard on generator quality, not on the minifier.
        assert!(
            skipped * 5 <= SEEDS,
            "{skipped} of {SEEDS} seeds were skipped (mangle={mangle}); \
             the generator is producing programs that do not complete"
        );
    }
}
