use crate::{
    minify,
    oracle::{Comparison, Oracle},
    unresolved_names,
};

/// Remove lines from `source` for as long as the result stays "interesting".
///
/// This is delta debugging in its subset-removal form: try dropping one half,
/// then one quarter, and so on, keeping any removal that preserves the property
/// under test. `select` is given every candidate for the current granularity at
/// once and returns the index of the first interesting one, so an implementation
/// backed by a subprocess can test a whole round in one call.
///
/// Reduction is purely textual. A removal that unbalances braces produces a
/// candidate that no longer parses, which `select` simply rejects.
///
/// Lines `protect` accepts are never removed. That exists for lines whose
/// removal changes the meaning of everything around them rather than removing
/// something: dropping `"use strict"` turns every undeclared name from a
/// `ReferenceError` into an implicit global, and the reduction then
/// demonstrates a different program than the one that failed.
pub fn reduce(
    source: &str,
    protect: impl Fn(&str) -> bool,
    mut select: impl FnMut(&[String]) -> Option<usize>,
) -> String {
    let mut lines: Vec<&str> = source.lines().collect();
    let mut granularity = 2;

    while lines.len() >= 2 {
        let chunk = lines.len().div_ceil(granularity);
        // A chunk of nothing but protected lines would be removed to no effect,
        // leaving the reducer in exactly the state it started the round in.
        let ranges: Vec<(usize, usize)> = (0..lines.len())
            .step_by(chunk)
            .map(|start| (start, (start + chunk).min(lines.len())))
            .filter(|(start, end)| lines[*start..*end].iter().any(|line| !protect(line)))
            .collect();
        let candidates: Vec<String> = ranges
            .iter()
            .map(|(start, end)| {
                let kept: Vec<&str> = lines
                    .iter()
                    .enumerate()
                    .filter(|(index, line)| index < start || index >= end || protect(line))
                    .map(|(_, line)| *line)
                    .collect();
                kept.join("\n")
            })
            .collect();

        let mut removed = false;
        if let Some(index) = select(&candidates) {
            let (start, end) = ranges[index];
            let before = lines.len();
            let mut position = 0;
            lines.retain(|line| {
                let keep = position < start || position >= end || protect(line);
                position += 1;
                keep
            });
            removed = lines.len() < before;
            // Removing a chunk shrinks the input, so the same granularity now
            // means larger chunks. Step back one to retry the coarse cut.
            granularity = granularity.saturating_sub(1).max(2);
        }
        if !removed {
            if granularity >= lines.len() {
                break;
            }
            granularity = (granularity * 2).min(lines.len());
        }
    }

    lines.join("\n")
}

/// A mismatching program reduced to a smaller one that still mismatches.
#[derive(Debug, Clone)]
pub struct Reduction {
    pub source: String,
    pub minified: String,
    pub original_lines: usize,
    pub reduced_lines: usize,
    /// Whether reduction ran out of candidates rather than out of budget. A
    /// reduction that stopped early is still a valid reproduction, just not a
    /// minimal one, and saying so beats presenting it as minimal.
    pub converged: bool,
}

/// Reduce a program that the minifier compiles wrongly.
///
/// Returns `None` when `source` does not actually mismatch, which is the case
/// for every input that only differs by throwing or timing out — those prove
/// nothing and must not be reported as reproductions.
pub fn shrink(source: &str, mangle: bool, oracle: Oracle) -> Option<Reduction> {
    // Each round spawns a Node process per candidate batch, so an unbounded
    // reduction of a large program can run for many minutes. The budget keeps
    // that predictable; `converged` records whether it was hit.
    const MAX_ROUNDS: usize = 120;

    if !is_mismatch(source, mangle, oracle) {
        return None;
    }
    // Removing a declaration but not its uses turns them into references to
    // something that is not there. The reduced program still misbehaves, but
    // for a reason the original never had, which sends the reader after the
    // wrong bug. Reject any candidate that refers to more names than the
    // program it came from.
    let allowed = unresolved_names(source);
    let mut rounds = 0;

    let reduced = reduce(
        source,
        |line| line.trim() == "\"use strict\";",
        |candidates| {
            if rounds >= MAX_ROUNDS {
                return None;
            }
            rounds += 1;
            // Candidates that no longer parse or that the minifier rejects cannot
            // be compared, so they are dropped before the oracle runs.
            let compilable: Vec<(usize, String)> = candidates
                .iter()
                .enumerate()
                .filter(|(_, candidate)| {
                    unresolved_names(candidate).iter().all(|name| allowed.contains(name))
                })
                .filter_map(|(index, candidate)| {
                    minify(candidate, mangle).ok().map(|minified| (index, minified.code))
                })
                .collect();
            let cases: Vec<(&str, &str)> = compilable
                .iter()
                .map(|(index, minified)| (candidates[*index].as_str(), minified.as_str()))
                .collect();
            oracle
                .compare_many(&cases)
                .iter()
                .position(|comparison| matches!(comparison, Comparison::Mismatch { .. }))
                .map(|position| compilable[position].0)
        },
    );

    let minified = minify(&reduced, mangle).ok()?;
    Some(Reduction {
        source: reduced.clone(),
        minified: minified.code,
        original_lines: source.lines().count(),
        reduced_lines: reduced.lines().count(),
        converged: rounds < MAX_ROUNDS,
    })
}

fn is_mismatch(source: &str, mangle: bool, oracle: Oracle) -> bool {
    let Ok(minified) = minify(source, mangle) else { return false };
    matches!(oracle.compare(source, &minified.code), Comparison::Mismatch { .. })
}

#[cfg(test)]
mod tests {
    use super::reduce;

    #[test]
    fn removes_everything_but_what_the_property_depends_on() {
        let source = (0..40)
            .map(|index| if index == 17 { "MARKER".to_owned() } else { format!("noise{index}") })
            .collect::<Vec<_>>()
            .join("\n");

        let reduced = reduce(
            &source,
            |_| false,
            |candidates| candidates.iter().position(|candidate| candidate.contains("MARKER")),
        );

        assert_eq!(reduced, "MARKER");
    }

    #[test]
    fn keeps_every_line_the_property_needs() {
        let source = "a\nKEEP1\nb\nc\nKEEP2\nd";
        let reduced = reduce(
            source,
            |line| line.trim() == "\"use strict\";",
            |candidates| {
                candidates.iter().position(|candidate| {
                    candidate.contains("KEEP1") && candidate.contains("KEEP2")
                })
            },
        );
        assert_eq!(reduced, "KEEP1\nKEEP2");
    }

    /// Without this, a reduction of a strict-mode program can drop the
    /// directive, and every name whose declaration was also dropped silently
    /// becomes an implicit global instead of a `ReferenceError`. The reduction
    /// then still "fails", but for a reason the original program never had.
    #[test]
    fn a_protected_line_survives_every_removal() {
        let source = "\"use strict\";\na\nb\nc\nd\ne\nf\ng";
        let reduced = reduce(
            source,
            |line| line.trim() == "\"use strict\";",
            // Accept the first removal offered every time, which without
            // protection would strip the file down to nothing.
            |candidates| (!candidates.is_empty()).then_some(0),
        );
        assert_eq!(reduced, "\"use strict\";");
    }

    #[test]
    fn returns_the_input_when_nothing_can_be_removed() {
        let source = "only";
        assert_eq!(reduce(source, |_| false, |_| None), "only");
    }

    #[test]
    fn terminates_when_no_removal_is_ever_interesting() {
        let source = (0..64).map(|index| index.to_string()).collect::<Vec<_>>().join("\n");
        assert_eq!(reduce(&source, |_| false, |_| None), source);
    }
}
