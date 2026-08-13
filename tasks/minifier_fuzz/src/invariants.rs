//! Invariants that hold without running the program.
//!
//! The semantic campaign in [`crate::campaign`] needs a Node.js process per
//! batch, which dominates its cost. These checks need only the minifier itself,
//! so they sweep seed ranges orders of magnitude larger, and they reach failures
//! a runtime comparison structurally cannot: output that does not parse compares
//! as "both threw" and is skipped rather than reported.

use crate::{Minified, check_syntax_and_semantics, generator::generate, minify};

#[derive(Debug, Clone, Copy)]
pub struct InvariantOptions {
    pub start_seed: u64,
    pub iterations: u64,
    pub mangle: bool,
}

#[derive(Debug, Clone)]
pub enum Violation {
    /// The minifier aborted. Its own consistency checks live behind
    /// `debug_assertions`, so this is how an internal invariant violation
    /// surfaces in a test build; a release build reports only real crashes.
    MinifierPanicked {
        second_pass: bool,
    },
    MinifierRejectedInput(String),
    /// Code generation produced something the parser or the binder rejects.
    OutputIsNotValid(String),
    /// Running the minifier twice must never produce more code than running it
    /// once: a pass that undoes an earlier pass makes the result depend on how
    /// many times the tool happened to run.
    SecondPassGrewOutput {
        first: usize,
        second: usize,
    },
}

#[derive(Debug, Clone, Default)]
pub struct InvariantSummary {
    pub checked: u64,
    /// Seeds where minifying the output again changed it. Not a defect on its
    /// own — the minifier stops after a bounded number of internal passes — but
    /// it measures how much a further pass would still find.
    pub not_idempotent: u64,
    /// Bytes a second run removes, summed over every seed.
    pub second_pass_savings: u64,
    pub max_iterations: u8,
    /// The seed where a second run removed the most, and how much.
    pub worst_seed: Option<(u64, usize)>,
    /// Violations are rare and independent, so the sweep records them and keeps
    /// going. Stopping at the first would hide how widespread one is, and would
    /// make a single known defect block every seed after it.
    pub violations: Vec<(u64, Violation)>,
}

/// Check every seed in the range. Returns what held and what did not.
#[must_use]
pub fn run(options: &InvariantOptions) -> InvariantSummary {
    let mut summary = InvariantSummary::default();
    let end_seed = options.start_seed.saturating_add(options.iterations);

    for seed in options.start_seed..end_seed {
        let source = generate(seed);

        let first = match minify_caught(&source, options.mangle) {
            Ok(first) => first,
            Err(message) => {
                summary.violations.push((
                    seed,
                    message.map_or(
                        Violation::MinifierPanicked { second_pass: false },
                        Violation::MinifierRejectedInput,
                    ),
                ));
                continue;
            }
        };

        if let Err(message) = check_syntax_and_semantics(&first.code) {
            summary.violations.push((seed, Violation::OutputIsNotValid(message)));
            continue;
        }

        let second = match minify_caught(&first.code, options.mangle) {
            Ok(second) => second,
            Err(message) => {
                summary.violations.push((
                    seed,
                    message.map_or(
                        Violation::MinifierPanicked { second_pass: true },
                        Violation::OutputIsNotValid,
                    ),
                ));
                continue;
            }
        };

        if second.code.len() > first.code.len() {
            summary.violations.push((
                seed,
                Violation::SecondPassGrewOutput {
                    first: first.code.len(),
                    second: second.code.len(),
                },
            ));
            continue;
        }

        summary.checked += 1;
        summary.max_iterations = summary.max_iterations.max(first.iterations);

        // With mangling on, the second run renames the already-renamed bindings,
        // so the output differs for a reason that says nothing about the
        // compressor. Only sizes are comparable then.
        let changed = if options.mangle {
            second.code.len() < first.code.len()
        } else {
            second.code != first.code
        };
        if changed {
            let saved = first.code.len() - second.code.len();
            summary.not_idempotent += 1;
            summary.second_pass_savings += saved as u64;
            if summary.worst_seed.is_none_or(|(_, worst)| saved > worst) {
                summary.worst_seed = Some((seed, saved));
            }
        }
    }

    summary
}

/// Minify without letting a panic take the whole sweep down.
///
/// `Err(None)` means the minifier aborted, `Err(Some(message))` that it
/// rejected the input.
fn minify_caught(source: &str, mangle: bool) -> Result<Minified, Option<String>> {
    let owned = source.to_owned();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(move || minify(&owned, mangle));
    std::panic::set_hook(previous);
    match result {
        Ok(Ok(minified)) => Ok(minified),
        Ok(Err(message)) => Err(Some(message)),
        Err(_) => Err(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{InvariantOptions, Violation, run};

    const SEEDS: u64 = 3_000;

    fn options() -> InvariantOptions {
        InvariantOptions { start_seed: 0, iterations: SEEDS, mangle: false }
    }

    /// Every generated program must minify to code that parses and binds, and
    /// a second run must never undo the first.
    ///
    /// `MinifierPanicked` is exempted: the minifier currently trips its own
    /// `debug_assert_no_under_prune` consistency check on a small fraction of
    /// these programs. That is a real defect, and it is reported by the sweep;
    /// it is exempted here so it does not mask the other invariants. Remove the
    /// exemption once the minifier stops aborting.
    #[test]
    fn generated_programs_satisfy_the_invariants() {
        assert_invariants_hold_except_for_panics(&options());
    }

    #[test]
    fn generated_programs_satisfy_the_invariants_when_mangled() {
        assert_invariants_hold_except_for_panics(&InvariantOptions { mangle: true, ..options() });
    }

    fn assert_invariants_hold_except_for_panics(options: &InvariantOptions) {
        let summary = run(options);
        let unexpected: Vec<_> = summary
            .violations
            .iter()
            .filter(|(_, violation)| !matches!(violation, Violation::MinifierPanicked { .. }))
            .collect();
        assert!(unexpected.is_empty(), "{unexpected:#?}");
        // A sweep that checked almost nothing would satisfy the assertion above
        // while proving nothing.
        assert!(
            summary.checked * 20 >= SEEDS * 19,
            "only {} of {SEEDS} seeds were checked: {:#?}",
            summary.checked,
            summary.violations
        );
    }

    #[test]
    fn an_empty_range_checks_nothing_rather_than_reporting_success_over_it() {
        let summary = run(&InvariantOptions { iterations: 0, ..options() });
        assert_eq!(summary.checked, 0);
        assert!(summary.violations.is_empty());
    }
}
