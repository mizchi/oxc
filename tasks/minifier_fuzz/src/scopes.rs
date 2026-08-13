//! Name-resolution fuzzing, aimed at the mangler.
//!
//! The program generator in [`crate::generator`] gives every binding a unique
//! name, which is the easiest possible input for a mangler: nothing shadows
//! anything, so no reference can resolve to the wrong binding no matter how the
//! renaming goes. Running it with `--mangle` therefore says very little.
//!
//! This generator does the opposite. It draws from a pool of three names and
//! reuses them down a deep chain of nested scopes, so most names are shadowed
//! several times over. Every binding gets a distinct value and every read is
//! recorded together with the name it was written as, so the log says which
//! binding each read resolved to. Renaming must not change that log.
//!
//! It also covers the places where a name is *not* an ordinary reference and so
//! must not be renamed along with one: shorthand properties, property keys,
//! labels, private class fields, and the scope a direct `eval` can see.

use std::fmt::Write as _;

use rand::{RngExt, SeedableRng, rngs::StdRng};

/// Short names collide with what a mangler wants to allocate, and a small pool
/// guarantees shadowing rather than leaving it to chance.
const POOL: [&str; 3] = ["a", "b", "c"];

/// Nesting budget. Deep enough for long shadowing chains, shallow enough that
/// the program stays readable once reduced.
const MAX_DEPTH: usize = 5;

/// A read is logged as the name it was written as, paired with the identity of
/// what it resolved to.
///
/// Functions and classes are logged by an identity number assigned in the order
/// they are first seen, never by anything derived from the source. Stringifying
/// one would call `Function.prototype.toString`, which returns source text that
/// minification is supposed to change, and every seed would then "fail" for a
/// reason that says nothing about name resolution.
const PREAMBLE: &str = "\
(function () {\n\
\"use strict\";\n\
var log = [], identities = new WeakMap(), nextIdentity = 0;\n\
function identify(value) {\n\
  if (!identities.has(value)) { identities.set(value, ++nextIdentity); }\n\
  return \"#\" + identities.get(value);\n\
}\n\
function mark(name, value) {\n\
  var held = value !== null && (typeof value === \"object\" || typeof value === \"function\")\n\
    ? identify(value)\n\
    : String(value);\n\
  log.push(name + \"=\" + held);\n\
}\n";

/// Build a program that reads a heavily shadowed set of names and reports which
/// binding each read resolved to.
#[must_use]
pub fn generate(seed: u64) -> String {
    let mut generator = Generator {
        rng: StdRng::seed_from_u64(seed),
        next_value: 0,
        scopes: Vec::new(),
        labels: Vec::new(),
        shadowed_labels: Vec::new(),
    };
    let mut source = String::from(PREAMBLE);
    generator.push_scope(true);
    source.push_str(&generator.body(MAX_DEPTH));
    generator.pop_scope();
    source.push_str("console.log(log);\n})();\n");
    source
}

/// How a name was introduced. The two kinds conflict by different rules, and
/// getting that wrong produces a program the parser rejects rather than one
/// that tests anything.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `let`, `const`, `class`, a block-level function, a parameter. Conflicts
    /// only with another declaration in the same scope.
    Lexical,
    /// `var`. Hoists to the enclosing function scope, and conflicts with a
    /// lexical declaration anywhere between here and there.
    Var,
}

struct Binding {
    name: String,
    kind: Kind,
}

struct Scope {
    /// Whether `var` stops here on its way out.
    function_boundary: bool,
    bindings: Vec<Binding>,
}

struct Generator {
    rng: StdRng,
    next_value: usize,
    scopes: Vec<Scope>,
    labels: Vec<String>,
    shadowed_labels: Vec<Vec<String>>,
}

impl Generator {
    fn body(&mut self, depth: usize) -> String {
        let mut body = String::new();
        // Lexical declarations come first, because `let`, `const` and `class`
        // shadow their name across the *whole* scope, not from the declaration
        // onwards. A read placed above one throws on the temporal dead zone
        // instead of resolving to the outer binding, and a program that throws
        // proves nothing.
        for _ in 0..self.range(0..=2) {
            body.push_str(&self.lexical_declaration());
        }
        // Function and class declarations are lexical too, and `var` in a
        // nested block hoists *past* them, so one generated later would
        // retroactively invalidate a `var` already emitted below it.
        if depth > 0 && self.chance(0.3) {
            body.push_str(&self.class_body(depth - 1));
        }
        if depth > 0 && self.chance(0.4) {
            body.push_str(&self.function_declaration(depth - 1));
        }
        for _ in 0..self.range(2..=3) {
            body.push_str(&self.statement(depth));
        }
        // Always read on the way out, so every scope contributes to the log
        // even when its statements happened to be all declarations.
        body.push_str(&self.reads());
        body
    }

    fn statement(&mut self, depth: usize) -> String {
        if depth == 0 {
            return if self.chance(0.5) { self.var_declaration() } else { self.reads() };
        }
        match self.range(0..14) {
            0 | 1 => self.var_declaration(),
            2 => self.reads(),
            3 | 4 => self.block(depth - 1),
            5 | 6 => self.immediately_invoked(depth - 1),
            7 => self.arrow(depth - 1),
            8 => self.for_let(depth - 1),
            9 => self.catch_clause(depth - 1),
            10 => self.labelled(depth - 1),
            11 => self.shorthand_and_keys(),
            12 => self.wide_scope(),
            _ => self.direct_eval(),
        }
    }

    // ----------------------------------------------------------- declarations

    /// Declare one of the pool names in the current scope, shadowing whatever
    /// the name meant outside.
    fn lexical_declaration(&mut self) -> String {
        let Some(name) = self.declarable_lexical() else { return String::new() };
        let value = self.fresh_value();
        let keyword = self.pick(&["let", "const"]);
        self.declare_lexical(&name);
        format!("{keyword} {name} = {value};\n")
    }

    /// `var` has no dead zone — it is initialised to `undefined` when the scope
    /// is entered — so it is safe to place anywhere, including after a read of
    /// the name it goes on to shadow.
    fn var_declaration(&mut self) -> String {
        let Some(name) = self.declarable_var() else { return self.reads() };
        let value = self.fresh_value();
        self.declare_var(&name);
        format!("var {name} = {value};\n")
    }

    /// Read every visible name, recording the name as written and the value it
    /// resolved to. A renaming that redirects a reference changes the value.
    fn reads(&self) -> String {
        let visible = self.visible();
        if visible.is_empty() {
            return String::new();
        }
        visible.iter().fold(String::new(), |mut body, name| {
            let _ = writeln!(body, "mark(\"{name}\", {name});");
            body
        })
    }

    // -------------------------------------------------------------- structure

    fn block(&mut self, depth: usize) -> String {
        self.push_scope(false);
        let body = self.body(depth);
        self.pop_scope();
        format!("{{\n{body}}}\n")
    }

    fn function_declaration(&mut self, depth: usize) -> String {
        let Some(name) = self.declarable_lexical() else { return self.reads() };
        self.declare_lexical(&name);
        // A parameter shares the function's scope, so a `let` of the same name
        // in the body is a syntax error while a `var` of it is not.
        let parameters = self.parameters();
        self.push_scope(true);
        for parameter in &parameters {
            self.declare_lexical(&parameter.clone());
        }
        let body = self.body(depth);
        self.pop_scope();
        let arguments = self.arguments(parameters.len());
        format!("function {name}({}) {{\n{body}}}\n{name}({arguments});\n", parameters.join(", "))
    }

    /// A named function expression binds its own name inside itself only, which
    /// is a scope the mangler has to invent rather than one the source has.
    fn immediately_invoked(&mut self, depth: usize) -> String {
        let name = self.pick(&POOL).to_owned();
        let parameters = self.parameters();
        self.push_scope(true);
        self.declare_lexical(&name);
        for parameter in &parameters {
            self.declare_lexical(&parameter.clone());
        }
        let body = self.body(depth);
        self.pop_scope();
        let arguments = self.arguments(parameters.len());
        format!("(function {name}({}) {{\n{body}}})({arguments});\n", parameters.join(", "))
    }

    fn arrow(&mut self, depth: usize) -> String {
        let parameters = self.parameters();
        self.push_scope(true);
        for parameter in &parameters {
            self.declare_lexical(&parameter.clone());
        }
        let body = self.body(depth);
        self.pop_scope();
        let arguments = self.arguments(parameters.len());
        format!("(({}) => {{\n{body}}})({arguments});\n", parameters.join(", "))
    }

    /// A `let` loop head gets a fresh binding per iteration, which the mangler
    /// has to keep separate from the body's own bindings.
    fn for_let(&mut self, depth: usize) -> String {
        let Some(name) = self.declarable_lexical() else { return self.reads() };
        let limit = self.fresh_value();
        self.push_scope(false);
        self.declare_lexical(&name);
        let body = self.body(depth);
        self.pop_scope();
        format!("for (let {name} = {limit}; {name} < {}; {name}++) {{\n{body}}}\n", limit + 2)
    }

    /// A `catch` parameter lives in its own scope between the enclosing one and
    /// the block, and shadows a binding of the same name outside.
    fn catch_clause(&mut self, depth: usize) -> String {
        let name = self.pick(&POOL).to_owned();
        let thrown = self.fresh_value();
        self.push_scope(false);
        self.declare_lexical(&name);
        let body = self.body(depth);
        self.pop_scope();
        format!("try {{ throw {thrown}; }} catch ({name}) {{\n{body}}}\n")
    }

    /// Labels are a separate namespace, so a label may share a name with a
    /// binding in scope without either shadowing the other.
    fn labelled(&mut self, depth: usize) -> String {
        // A label nested inside another of the same name is a syntax error,
        // even though the names are otherwise unconstrained.
        let free: Vec<&str> = POOL
            .iter()
            .copied()
            .filter(|name| !self.labels.iter().any(|held| held == name))
            .collect();
        if free.is_empty() {
            return self.reads();
        }
        let name = free[self.range(0..free.len())].to_owned();
        self.labels.push(name.clone());
        self.push_scope(false);
        let body = self.body(depth);
        self.pop_scope();
        self.labels.pop();
        format!("{name}: {{\n{body}if (log.length < 0) break {name};\n}}\n")
    }

    /// A private field name is its own namespace too: `#a` and `a` are
    /// unrelated, and neither renaming may follow the other.
    fn class_body(&mut self, depth: usize) -> String {
        let Some(name) = self.declarable_lexical() else { return self.reads() };
        let field = self.fresh_value();
        self.declare_lexical(&name);
        let private = self.pick(&POOL).to_owned();
        self.push_scope(true);
        let body = self.body(depth);
        self.pop_scope();
        format!(
            "class {name} {{ #{private} = {field}; read() {{\n{body}return this.#{private}; }} }}\n\
             mark(\"#{private}\", new {name}().read());\n"
        )
    }

    /// Shorthand properties and property keys look like references but are not
    /// one, or are one only on the value side.
    fn shorthand_and_keys(&mut self) -> String {
        let visible = self.visible();
        let Some(name) = self.pick_visible(&visible) else { return String::new() };
        format!(
            "mark(\"shorthand\", ({{ {name} }}).{name});\n\
             mark(\"key\", ({{ {name}: \"literal\" }}).{name});\n\
             mark(\"computed\", ({{ [\"{name}\"]: \"computed\" }})[\"{name}\"]);\n"
        )
    }

    /// Declare more bindings than there are single-character identifiers, so
    /// the mangler has to move on to two-character names.
    ///
    /// That is where its name allocator can go wrong: a generated name may be a
    /// reserved word (`do`, `if`, `in`), may be one of the names a binding is
    /// not allowed to have in strict mode (`eval`, `arguments`), or may collide
    /// with a global the program still refers to. Each of those shows up either
    /// as output that fails to parse or as a program that throws.
    fn wide_scope(&mut self) -> String {
        // 26 lowercase + 26 uppercase + `$` + `_` is 54 single-character
        // identifiers, so this forces a good number of two-character ones.
        const WIDTH: usize = 80;
        let base = self.fresh_value();
        let mut body = String::from("(function () {\n");
        for index in 0..WIDTH {
            let _ = writeln!(body, "let q{index} = {};", base + index);
        }
        // Read them all, so none can be dropped as unused before renaming.
        for index in 0..WIDTH {
            let _ = writeln!(body, "mark(\"q{index}\", q{index});");
        }
        body.push_str("})();\n");
        body
    }

    /// A direct `eval` can read the enclosing scope by source name, so nothing
    /// visible from it may be renamed.
    fn direct_eval(&mut self) -> String {
        let visible = self.visible();
        let Some(name) = self.pick_visible(&visible) else { return String::new() };
        format!("mark(\"eval\", eval(\"{name}\"));\n")
    }

    // --------------------------------------------------------------- bindings

    fn parameters(&mut self) -> Vec<String> {
        let mut parameters: Vec<String> = Vec::new();
        for _ in 0..self.range(0..=2) {
            let name = self.pick(&POOL).to_owned();
            // Duplicate parameter names are a syntax error in strict mode.
            if !parameters.contains(&name) {
                parameters.push(name);
            }
        }
        parameters
    }

    fn arguments(&mut self, count: usize) -> String {
        let mut arguments = Vec::with_capacity(count);
        for _ in 0..count {
            arguments.push(self.fresh_value().to_string());
        }
        arguments.join(", ")
    }

    fn push_scope(&mut self, function_boundary: bool) {
        // A label is not visible inside a nested function, so the names are
        // free to be reused there.
        if function_boundary {
            self.shadowed_labels.push(std::mem::take(&mut self.labels));
        }
        self.scopes.push(Scope { function_boundary, bindings: Vec::new() });
    }

    fn pop_scope(&mut self) {
        if self.scopes.pop().is_some_and(|scope| scope.function_boundary)
            && let Some(labels) = self.shadowed_labels.pop()
        {
            self.labels = labels;
        }
    }

    fn declare_lexical(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.bindings.push(Binding { name: name.to_owned(), kind: Kind::Lexical });
        }
    }

    fn declare_var(&mut self, name: &str) {
        let index = self.scopes.iter().rposition(|scope| scope.function_boundary);
        if let Some(scope) = index.and_then(|index| self.scopes.get_mut(index)) {
            scope.bindings.push(Binding { name: name.to_owned(), kind: Kind::Var });
        }
    }

    /// A pool name free for a lexical declaration here: nothing else in this
    /// scope may already carry it, whatever kind it is.
    fn declarable_lexical(&mut self) -> Option<String> {
        let taken: Vec<String> = self
            .scopes
            .last()
            .map(|scope| scope.bindings.iter().map(|binding| binding.name.clone()).collect())
            .unwrap_or_default();
        self.free_name(&taken)
    }

    /// A pool name free for a `var`. It hoists to the enclosing function scope,
    /// so it collides with any lexical declaration on the way there — `{ let a;
    /// { var a; } }` is a syntax error even though the two look unrelated.
    fn declarable_var(&mut self) -> Option<String> {
        let mut taken = Vec::new();
        for scope in self.scopes.iter().rev() {
            for binding in &scope.bindings {
                if binding.kind == Kind::Lexical {
                    taken.push(binding.name.clone());
                }
            }
            if scope.function_boundary {
                break;
            }
        }
        self.free_name(&taken)
    }

    fn free_name(&mut self, taken: &[String]) -> Option<String> {
        let free: Vec<&str> =
            POOL.iter().copied().filter(|name| !taken.iter().any(|held| held == name)).collect();
        if free.is_empty() {
            return None;
        }
        Some(free[self.range(0..free.len())].to_owned())
    }

    fn visible(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for scope in &self.scopes {
            for binding in &scope.bindings {
                if !names.contains(&binding.name) {
                    names.push(binding.name.clone());
                }
            }
        }
        names.sort();
        names
    }

    fn pick_visible(&mut self, visible: &[String]) -> Option<String> {
        if visible.is_empty() {
            return None;
        }
        Some(visible[self.range(0..visible.len())].clone())
    }

    fn fresh_value(&mut self) -> usize {
        self.next_value += 1;
        self.next_value
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

    fn pick<'a>(&mut self, choices: &'a [&'a str]) -> &'a str {
        choices[self.range(0..choices.len())]
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
        assert_eq!(generate(3), generate(3));
        assert_ne!(generate(3), generate(4));
    }

    #[test]
    fn generated_programs_parse_and_have_valid_semantics() {
        for seed in 0..500 {
            let source = generate(seed);
            assert!(check_syntax_and_semantics(&source).is_ok(), "seed {seed}\n{source}");
        }
    }

    /// The whole point is shadowing, so a generator that stopped producing it
    /// would keep passing while testing nothing.
    #[test]
    fn programs_actually_shadow_names() {
        let shadowing = (0..100u64)
            .filter(|seed| {
                let source = generate(*seed);
                // More declarations than pool names means at least one name is
                // declared twice, which under the same-scope rule means one
                // shadows the other.
                source.matches(" a = ").count()
                    + source.matches(" b = ").count()
                    + source.matches(" c = ").count()
                    > 3
            })
            .count();
        assert!(shadowing > 90, "only {shadowing} of 100 seeds shadow a name");
    }

    #[test]
    fn resolution_survives_mangling() {
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
