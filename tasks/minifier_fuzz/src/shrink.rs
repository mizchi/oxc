use crate::{
    minify,
    oracle::{Comparison, Oracle},
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
pub fn reduce(source: &str, mut select: impl FnMut(&[String]) -> Option<usize>) -> String {
    let mut lines: Vec<&str> = source.lines().collect();
    let mut granularity = 2;

    while lines.len() >= 2 {
        let chunk = lines.len().div_ceil(granularity);
        let ranges: Vec<(usize, usize)> = (0..lines.len())
            .step_by(chunk)
            .map(|start| (start, (start + chunk).min(lines.len())))
            .collect();
        let candidates: Vec<String> = ranges
            .iter()
            .map(|(start, end)| {
                let kept: Vec<&str> = lines
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| index < start || index >= end)
                    .map(|(_, line)| *line)
                    .collect();
                kept.join("\n")
            })
            .collect();

        if let Some(index) = select(&candidates) {
            let (start, end) = ranges[index];
            lines.drain(start..end);
            // Removing a chunk shrinks the input, so the same granularity now
            // means larger chunks. Step back one to retry the coarse cut.
            granularity = granularity.saturating_sub(1).max(2);
        } else if granularity >= lines.len() {
            break;
        } else {
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
}

/// Reduce a program that the minifier compiles wrongly.
///
/// Returns `None` when `source` does not actually mismatch, which is the case
/// for every input that only differs by throwing or timing out — those prove
/// nothing and must not be reported as reproductions.
pub fn shrink(source: &str, mangle: bool, oracle: Oracle) -> Option<Reduction> {
    if !is_mismatch(source, mangle, oracle) {
        return None;
    }

    let reduced = reduce(source, |candidates| {
        // Candidates that no longer parse or that the minifier rejects cannot
        // be compared, so they are dropped before the oracle runs.
        let compilable: Vec<(usize, String)> = candidates
            .iter()
            .enumerate()
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
    });

    let minified = minify(&reduced, mangle).ok()?;
    Some(Reduction {
        source: reduced.clone(),
        minified: minified.code,
        original_lines: source.lines().count(),
        reduced_lines: reduced.lines().count(),
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

        let reduced = reduce(&source, |candidates| {
            candidates.iter().position(|candidate| candidate.contains("MARKER"))
        });

        assert_eq!(reduced, "MARKER");
    }

    #[test]
    fn keeps_every_line_the_property_needs() {
        let source = "a\nKEEP1\nb\nc\nKEEP2\nd";
        let reduced = reduce(source, |candidates| {
            candidates
                .iter()
                .position(|candidate| candidate.contains("KEEP1") && candidate.contains("KEEP2"))
        });
        assert_eq!(reduced, "KEEP1\nKEEP2");
    }

    #[test]
    fn returns_the_input_when_nothing_can_be_removed() {
        let source = "only";
        assert_eq!(reduce(source, |_| None), "only");
    }

    #[test]
    fn terminates_when_no_removal_is_ever_interesting() {
        let source = (0..64).map(|index| index.to_string()).collect::<Vec<_>>().join("\n");
        assert_eq!(reduce(&source, |_| None), source);
    }
}
