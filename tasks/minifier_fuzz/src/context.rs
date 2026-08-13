//! Context-invariance fuzzing for binding patterns.
//!
//! Modelled on esbuild's `scripts/destructuring-fuzzer.js`. The idea there is
//! that a binding pattern means the same thing wherever it appears: `const
//! [a = 1] = v` must bind `a` exactly as `function f([a = 1]) {}; f(v)` does,
//! as `try { throw v } catch ([a = 1]) {}` does, and so on. Generating one
//! pattern and evaluating it in every context that accepts it turns that into a
//! property that needs no reference implementation.
//!
//! Compression sees a different AST in each context — a declaration, a
//! parameter list, a `catch` clause — but they share the binding-pattern code
//! paths. A pass that gets one context wrong shows up as that context's
//! bindings disagreeing with the rest, which is visible in the output even
//! before the minified program is compared with the original.
//!
//! <https://github.com/evanw/esbuild/blob/main/scripts/destructuring-fuzzer.js>

use std::fmt::Write as _;

use rand::{RngExt, SeedableRng, rngs::StdRng};

/// Nesting budget for a generated pattern.
const MAX_PATTERN_DEPTH: usize = 3;

/// Build a program that binds one generated pattern in every context that
/// accepts it and reports what each context bound.
#[must_use]
pub fn generate(seed: u64) -> String {
    // Reporting every context rather than just "they agree" keeps a failure
    // legible: the log shows which context disagreed and how.
    format!("{}console.log(results);\n", bindings(seed))
}

/// The bindings alone, leaving `results` for the caller to report on.
#[must_use]
pub fn bindings(seed: u64) -> String {
    let mut generator = Generator { rng: StdRng::seed_from_u64(seed), next_name: 0 };
    let fragment = generator.fragment(MAX_PATTERN_DEPTH);
    let captures = fragment.names.iter().fold(String::new(), |mut body, name| {
        let _ = write!(body, "c[\"{name}\"] = {name};");
        body
    });

    let mut source = String::from("var results = [];\n");
    for body in contexts(&fragment, &captures) {
        // Each context runs in its own function so the bindings it introduces
        // cannot leak into the next one.
        let _ = writeln!(
            source,
            "results.push((function () {{ var c = {{}};\n{body}\nreturn c; }})());"
        );
    }
    source
}

/// A pattern, a value that fits it, and the names it binds.
struct Fragment {
    pattern: String,
    value: String,
    names: Vec<String>,
    /// Whether the pattern is a plain binding rather than a nested pattern.
    /// A default for a nested pattern has to be shape-compatible with it —
    /// `[[a] = 0]` destructures `0`, which throws — so the two cases cannot
    /// draw their fallback from the same place.
    is_leaf: bool,
}

/// Every context that accepts a destructuring pattern as a binding target.
fn contexts(fragment: &Fragment, captures: &str) -> Vec<String> {
    let Fragment { pattern, value, names, .. } = fragment;
    let declared = names.join(", ");
    vec![
        format!("var {pattern} = {value};\n{captures}"),
        format!("let {pattern} = {value};\n{captures}"),
        format!("const {pattern} = {value};\n{captures}"),
        // Assignment rather than declaration: the pattern is parsed as an
        // expression first and reinterpreted, a separate code path.
        format!("var {declared};\n({pattern} = {value});\n{captures}"),
        format!("function f({pattern}) {{ {captures} }}\nf({value});"),
        format!("(function ({pattern}) {{ {captures} }})({value});"),
        format!("(({pattern}) => {{ {captures} }})({value});"),
        // A default initialiser, so the pattern is applied to `value` without
        // an argument ever being passed.
        format!("(function ({pattern} = {value}) {{ {captures} }})();"),
        format!("({{ m({pattern}) {{ {captures} }} }}).m({value});"),
        format!("new (class {{ m({pattern}) {{ {captures} }} }})().m({value});"),
        format!("try {{ throw {value}; }} catch ({pattern}) {{ {captures} }}"),
        format!("for (const {pattern} of [{value}]) {{ {captures} }}"),
        format!("for (var {pattern} of [{value}]) {{ {captures} }}"),
    ]
}

struct Generator {
    rng: StdRng,
    next_name: usize,
}

impl Generator {
    fn fragment(&mut self, depth: usize) -> Fragment {
        if depth == 0 || self.chance(0.25) {
            let name = self.fresh_name();
            let value = self.literal().to_owned();
            return Fragment { pattern: name.clone(), value, names: vec![name], is_leaf: true };
        }
        if self.chance(0.5) { self.array_fragment(depth) } else { self.object_fragment(depth) }
    }

    fn array_fragment(&mut self, depth: usize) -> Fragment {
        let mut patterns = Vec::new();
        let mut values = Vec::new();
        let mut names = Vec::new();

        // A hole skips an element without binding anything.
        if self.chance(0.25) {
            patterns.push(String::new());
            values.push(self.literal().to_owned());
        }

        for _ in 0..self.range(1..=2) {
            let element = self.fragment(depth - 1);
            names.extend(element.names.iter().cloned());
            if self.chance(0.3) {
                let (fallback, value) = self.default_for(&element);
                patterns.push(format!("{} = {fallback}", element.pattern));
                values.push(value);
            } else {
                patterns.push(element.pattern.clone());
                values.push(element.value.clone());
            }
        }

        if self.chance(0.3) {
            let name = self.fresh_name();
            patterns.push(format!("...{name}"));
            names.push(name);
            values.push(self.literal().to_owned());
            values.push(self.literal().to_owned());
        }

        Fragment {
            pattern: format!("[{}]", patterns.join(", ")),
            value: format!("[{}]", values.join(", ")),
            names,
            is_leaf: false,
        }
    }

    fn object_fragment(&mut self, depth: usize) -> Fragment {
        let mut patterns = Vec::new();
        let mut values = Vec::new();
        let mut names = Vec::new();

        for index in 0..self.range(1..=2) {
            let key = format!("k{index}");
            let property = self.fragment(depth - 1);
            names.extend(property.names.iter().cloned());

            // A computed key is a different code path from a literal one, and
            // its expression has to stay where it is.
            let written_key = if self.chance(0.25) { format!("[\"{key}\"]") } else { key.clone() };

            if self.chance(0.3) {
                let (fallback, value) = self.default_for(&property);
                patterns.push(format!("{written_key}: {} = {fallback}", property.pattern));
                // A missing property is what makes the default fire, so the
                // "not fired" case has to supply the property.
                if value != "undefined" {
                    values.push(format!("{key}: {value}"));
                }
            } else {
                patterns.push(format!("{written_key}: {}", property.pattern));
                values.push(format!("{key}: {}", property.value));
            }
        }

        if self.chance(0.3) {
            let name = self.fresh_name();
            patterns.push(format!("...{name}"));
            names.push(name);
            values.push(format!("extra: {}", self.literal()));
        }

        Fragment {
            pattern: format!("{{ {} }}", patterns.join(", ")),
            value: format!("{{ {} }}", values.join(", ")),
            names,
            is_leaf: false,
        }
    }

    /// Pick a default for `element` and the value it is applied to.
    ///
    /// A nested pattern only accepts a fallback shaped like itself, so it
    /// reuses its own value and the default always fires. A plain binding takes
    /// any literal, so both the fired and the not-fired path are reachable.
    fn default_for(&mut self, element: &Fragment) -> (String, String) {
        if element.is_leaf {
            let fallback = self.literal().to_owned();
            let value =
                if self.chance(0.5) { "undefined".to_owned() } else { element.value.clone() };
            (fallback, value)
        } else {
            (element.value.clone(), "undefined".to_owned())
        }
    }

    fn fresh_name(&mut self) -> String {
        let value = self.next_name;
        self.next_name += 1;
        format!("v{value}")
    }

    fn literal(&mut self) -> &'static str {
        let choices =
            ["0", "1", "-1", "2", "'x'", "''", "true", "false", "null", "NaN", "[]", "{}"];
        choices[self.range(0..choices.len())]
    }

    fn chance(&mut self, probability: f64) -> bool {
        self.rng.random_bool(probability)
    }

    fn range<T, R>(&mut self, range: R) -> T
    where
        T: rand::distr::uniform::SampleUniform,
        R: rand::distr::uniform::SampleRange<T>,
    {
        self.rng.random_range(range)
    }
}

#[cfg(test)]
mod tests {
    use super::generate;
    use crate::{
        check_syntax_and_semantics, minify,
        oracle::{Comparison, Oracle},
    };

    #[test]
    fn same_seed_generates_same_program() {
        assert_eq!(generate(7), generate(7));
        assert_ne!(generate(7), generate(8));
    }

    #[test]
    fn generated_programs_parse_and_have_valid_semantics() {
        for seed in 0..300 {
            let source = generate(seed);
            assert!(check_syntax_and_semantics(&source).is_ok(), "seed {seed}\n{source}");
        }
    }

    /// The premise of the whole mode: the contexts must already agree before
    /// the minifier is involved. If they do not, a reported mismatch would be
    /// the generator's fault and would say nothing about the minifier.
    ///
    /// Checked by having the program answer "did every context agree?" and
    /// comparing that against a program that just says yes.
    #[test]
    fn every_context_binds_the_same_thing_before_minification() {
        const DEEP_EQUAL: &str = r"
function eq(a, b) {
  if (a === b) return true;
  if (a !== a && b !== b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || !a || !b) return false;
  var ka = Object.keys(a).sort(), kb = Object.keys(b).sort();
  if (ka.length !== kb.length) return false;
  for (var i = 0; i < ka.length; i++) {
    if (ka[i] !== kb[i] || !eq(a[ka[i]], b[kb[i]])) return false;
  }
  return true;
}
console.log(results.length > 1 && results.every(function (r) { return eq(r, results[0]); }));
";
        // Ask the yes/no question instead of reporting each context, so the
        // answer can be compared against a constant.
        let programs: Vec<_> =
            (0..200).map(|seed| (seed, format!("{}{DEEP_EQUAL}", super::bindings(seed)))).collect();
        let cases: Vec<_> =
            programs.iter().map(|(_, source)| (source.as_str(), "console.log(true);")).collect();

        for ((seed, source), comparison) in
            programs.iter().zip(Oracle::new(2_000).compare_many(&cases))
        {
            assert!(
                matches!(comparison, Comparison::Equivalent { .. }),
                "seed {seed}: the contexts do not already agree: {comparison:#?}\n{source}"
            );
        }
    }

    #[test]
    fn contexts_keep_agreeing_after_minification() {
        for mangle in [false, true] {
            let programs: Vec<_> = (0..100)
                .map(|seed| {
                    let source = generate(seed);
                    let minified = minify(&source, mangle)
                        .unwrap_or_else(|error| panic!("seed {seed}: {error}"));
                    (seed, source, minified.code)
                })
                .collect();
            let cases: Vec<_> = programs
                .iter()
                .map(|(_, source, minified)| (source.as_str(), minified.as_str()))
                .collect();
            for ((seed, source, minified), comparison) in
                programs.iter().zip(Oracle::new(2_000).compare_many(&cases))
            {
                assert!(
                    matches!(comparison, Comparison::Equivalent { .. }),
                    "seed {seed} (mangle={mangle}): {comparison:#?}\n{source}\n{minified}"
                );
            }
        }
    }
}
