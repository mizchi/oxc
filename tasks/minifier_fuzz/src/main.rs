#![expect(clippy::print_stdout, clippy::print_stderr)]
use std::{error::Error, path::PathBuf};

use oxc_minifier_fuzz::{
    campaign::{CampaignOptions, CampaignResult, Shape, run, save_failure},
    corpus,
    invariants::{self, InvariantOptions},
    oracle::Oracle,
    shrink::shrink,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = pico_args::Arguments::from_env();
    if args.contains(["-h", "--help"]) {
        print_help();
        return Ok(());
    }

    let mangle = args.contains("--mangle");
    let no_shrink = args.contains("--no-shrink");
    let invariants_only = args.contains("--invariants");
    let corpus_only = args.contains("--corpus");
    let shape = if args.contains("--contexts") {
        Shape::Contexts
    } else if args.contains("--scopes") {
        Shape::Scopes
    } else {
        Shape::Program
    };
    let options = CampaignOptions {
        start_seed: args.opt_value_from_str("--seed")?.unwrap_or(0),
        iterations: args.opt_value_from_str("--iterations")?.unwrap_or(1_000),
        timeout_ms: args.opt_value_from_str("--timeout-ms")?.unwrap_or(100),
        batch_size: args.opt_value_from_str("--batch-size")?.unwrap_or(100),
        mangle,
        shape,
    };
    let save_dir: PathBuf = args
        .opt_value_from_os_str("--save-dir", |value| Ok::<_, &'static str>(PathBuf::from(value)))?
        .unwrap_or_else(|| PathBuf::from("target/minifier-fuzz"));
    let remaining = args.finish();
    if !remaining.is_empty() {
        return Err(format!("unexpected arguments: {remaining:?}").into());
    }
    options.validate()?;

    if corpus_only {
        return run_corpus(mangle, options.timeout_ms, options.batch_size);
    }

    if invariants_only {
        return run_invariants(&InvariantOptions {
            start_seed: options.start_seed,
            iterations: options.iterations,
            mangle,
            shape,
        });
    }

    match run(&options) {
        CampaignResult::Completed(summary) => {
            println!(
                "checked {} seeds from {}, skipped {}, max minifier iterations {}",
                summary.checked,
                options.start_seed,
                summary.skipped,
                summary.max_minifier_iterations
            );
            // A campaign that compared nothing must not look like a pass. The
            // option validation above covers the known cause, but the generator
            // could also drift into emitting only programs that throw.
            if options.iterations > 0 && summary.checked == 0 {
                return Err(format!(
                    "no seed was compared: all {} seeds were skipped",
                    summary.skipped
                )
                .into());
            }
            Ok(())
        }
        CampaignResult::Failed { summary, failure } => {
            let mut paths = save_failure(&failure, &save_dir)?;
            eprintln!(
                "semantic mismatch at seed {} after {} checked seeds: {:#?}",
                failure.seed, summary.checked, failure.comparison
            );

            if !no_shrink {
                let oracle = Oracle::new(options.timeout_ms);
                match shrink(&failure.original, mangle, oracle) {
                    Some(reduction) => {
                        eprintln!(
                            "reduced {} lines to {}{}",
                            reduction.original_lines,
                            reduction.reduced_lines,
                            if reduction.converged {
                                ""
                            } else {
                                " (stopped on the round budget, not minimal)"
                            }
                        );
                        paths.extend(save_reduction(&reduction, failure.seed, &save_dir)?);
                    }
                    // The campaign found the mismatch with the same oracle, so
                    // this only happens for a program whose behavior is not
                    // reproducible — worth saying out loud rather than hiding.
                    None => eprintln!("could not reproduce the mismatch while reducing"),
                }
            }

            eprintln!("saved failure artifacts:");
            for path in paths {
                eprintln!("  {}", path.display());
            }
            Err("minifier semantic mismatch".into())
        }
        CampaignResult::HarnessError { seed, message } => {
            Err(format!("oracle failed at seed {seed}: {message}").into())
        }
        CampaignResult::MinifierError { seed, source, message } => {
            Err(format!("minifier rejected generated input at seed {seed}: {message}\n{source}")
                .into())
        }
    }
}

fn run_corpus(mangle: bool, timeout_ms: u64, batch_size: usize) -> Result<(), Box<dyn Error>> {
    let root = corpus::ensure_terser_suite()?;
    let tests = corpus::collect_tests(root)?;
    println!("{} runnable cases from Terser's compress suite", tests.len());

    let summary = corpus::check(&tests, mangle, timeout_ms, batch_size);
    println!(
        "checked {}, skipped {} (do not complete under the sandbox), {} rejected by the parser",
        summary.checked, summary.skipped, summary.unparsed
    );
    if summary.mismatches.is_empty() {
        return Ok(());
    }
    for failure in &summary.mismatches {
        eprintln!("{}: {}", failure.file, failure.name);
        eprintln!("  original: {}", failure.source.trim());
        eprintln!("  minified: {}", failure.minified);
    }
    Err(format!("{} behavior mismatches", summary.mismatches.len()).into())
}

fn run_invariants(options: &InvariantOptions) -> Result<(), Box<dyn Error>> {
    let summary = invariants::run(options);
    println!(
        "checked {} seeds from {}, max minifier iterations {}",
        summary.checked, options.start_seed, summary.max_iterations
    );
    println!(
        "not a fixed point for {} seeds, {} bytes a second pass would still remove{}",
        summary.not_idempotent,
        summary.second_pass_savings,
        summary
            .worst_seed
            .map_or_else(String::new, |(seed, saved)| format!(" (worst: seed {seed}, {saved})"))
    );

    if summary.violations.is_empty() {
        return Ok(());
    }
    for (seed, violation) in &summary.violations {
        eprintln!("seed {seed}: {violation:?}");
    }
    Err(format!("{} invariant violations", summary.violations.len()).into())
}

fn save_reduction(
    reduction: &oxc_minifier_fuzz::shrink::Reduction,
    seed: u64,
    directory: &std::path::Path,
) -> Result<Vec<PathBuf>, std::io::Error> {
    let source_path = directory.join(format!("seed-{seed}.reduced.js"));
    let minified_path = directory.join(format!("seed-{seed}.reduced.min.js"));
    std::fs::write(&source_path, &reduction.source)?;
    std::fs::write(&minified_path, &reduction.minified)?;
    Ok(vec![source_path, minified_path])
}

fn print_help() {
    println!(
        "oxc_minifier_fuzz\n\n\
         Generate deterministic JavaScript programs, compress them with oxc_minifier,\n\
         and compare observable behavior in isolated Node.js VM contexts.\n\n\
         Options:\n\
           --seed <N>          first seed (default: 0)\n\
           --iterations <N>    number of seeds (default: 1000)\n\
           --timeout-ms <N>    VM timeout per program, 1..=4294967295 (default: 100)\n\
           --batch-size <N>    programs per Node.js process, at least 1 (default: 100)\n\
           --mangle            also mangle names (default: compression only)\n\
           --no-shrink         do not reduce a mismatch before saving it\n\
           --contexts          generate one binding pattern per seed and bind it\n\
                               in every context that accepts it\n\
           --scopes            reuse a small pool of names down nested scopes,\n\
                               aimed at the mangler (pair with --mangle)\n\
           --corpus            run Terser's compress test suite through the\n\
                               minifier instead of generated programs\n\
           --invariants        skip Node.js: only check that the output parses,\n\
                               binds, and is a fixed point of the minifier\n\
           --save-dir <PATH>   mismatch artifacts (default: target/minifier-fuzz)\n"
    );
}
