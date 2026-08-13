use std::fmt::Write as _;

use rand::{RngExt, SeedableRng, rngs::StdRng};
use rustc_hash::FxHashSet;

/// Depth budget for a single expression tree.
const MAX_EXPRESSION_DEPTH: usize = 3;

/// Shared call budget. Every generated function body decrements it, so self and
/// mutual recursion always bottoms out.
const CALL_BUDGET: u32 = 30;

/// Iterations a single generated loop may run before its brake fires. Loops
/// nest at most `MAX_STATEMENT_DEPTH` deep, bounding total iterations.
const LOOP_BRAKE: u32 = 4;

/// Statement nesting budget for the top-level body.
const MAX_STATEMENT_DEPTH: usize = 4;

/// Fixed bindings every program starts with.
///
/// `side` is the observable-effect probe: reading or writing it appends to
/// `trace`, which the final `console.log` reports, so a pass that drops,
/// reorders or duplicates the access changes the trace even when the final
/// value does not.
///
/// `prim` and `str` are coercion probes, and deliberately *silent*. Oxc
/// documents that it treats `ToPrimitive` invoked by `==`, `!=` and the
/// relational operators as side-effect free — see the `test_binary_expressions`
/// cases in `crates/oxc_minifier/tests/ecmascript/may_have_side_effects.rs`
/// marked "actually have a side effect, but this treated as side-effect free".
/// Appending to `trace` from `valueOf`/`toString` would report that accepted
/// trade-off as a mismatch on almost every seed. Returning a value without a
/// trace still checks the part that must hold: the *result* of the coercion.
///
/// `Function.prototype.toString` returns source text, which minification is
/// *supposed* to change, so any function value that reaches a string coercion
/// would report a mismatch that says nothing about semantics. Overriding it
/// makes that harmless instead of relying on no such value ever being built.
const PREAMBLE: &str = "\
Function.prototype.toString = function () { return '[function]'; };\n\
var a = 100, b = 10, c = 0, x = 1, y = 2, z = 3;\n\
var obj = { p: 0, q: 1 }, arr = [0, 1, 2], trace = [], thunks = [];\n\
obj.m = function () { return this.p + this.q; };\n\
var side = { get g() { trace.push(-1); return c; }, set s(v) { trace.push(-2); c = v; } };\n\
var prim = { valueOf: function () { return b; } };\n\
var str = { toString: function () { return 's'; } };\n";

/// Generate a deterministic, self-contained JavaScript program for a seed.
///
/// Like Terser's ufuzz generator, programs have bounded loops and a shared call
/// budget. They only observe behavior through the final `console.log` call.
///
/// <https://github.com/terser/terser/blob/v5.50.0/test/ufuzz.js>
pub fn generate(seed: u64) -> String {
    Generator::new(seed).program()
}

/// What a name may be used for. Calling a class without `new` and constructing
/// a plain function both throw, so the use site has to know which it has.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Value,
    Function,
    Generator,
    Class,
}

struct Binding {
    name: String,
    kind: Kind,
    mutable: bool,
    /// Whether generated code may refer to the name. A `catch` parameter is
    /// declared but never read: its value is an engine-produced `Error` whose
    /// message quotes identifiers from the source, which mangling renames.
    readable: bool,
}

struct Scope {
    /// `var` hoists to the nearest function scope; `let` and `const` stay put.
    function_boundary: bool,
    bindings: Vec<Binding>,
}

struct Label {
    name: String,
    /// `continue` only accepts a label attached to an iteration statement.
    loop_label: bool,
}

#[derive(Clone, Copy)]
struct StatementContext {
    in_function: bool,
    loop_depth: usize,
}

struct Generator {
    rng: StdRng,
    next_var: usize,
    next_brake: usize,
    next_effect: usize,
    next_label: usize,
    next_closure: usize,
    scopes: Vec<Scope>,
    labels: Vec<Label>,
    /// `this` is `undefined` inside a strict-mode function called as a plain
    /// function, so `this.p` may only be generated where a receiver exists.
    this_available: bool,
}

impl Generator {
    fn new(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            next_var: 0,
            next_brake: 0,
            next_effect: 0,
            next_label: 0,
            next_closure: 0,
            scopes: Vec::new(),
            labels: Vec::new(),
            this_available: false,
        }
    }

    fn program(mut self) -> String {
        let mut source = String::from("(function () {\n\"use strict\";\n");
        let _ = writeln!(source, "var _calls_ = {CALL_BUDGET};");
        source.push_str(PREAMBLE);

        self.push_scope(true);
        for name in ["a", "b", "c", "x", "y", "z"] {
            self.declare(name, Kind::Value, true);
        }
        for name in ["obj", "arr", "trace", "thunks", "side", "prim", "str"] {
            self.declare(name, Kind::Value, false);
        }

        // Function declarations hoist, so registering them before their bodies
        // exist lets any of them reference any other.
        for index in 0..3 {
            self.declare(&format!("f{index}"), Kind::Function, false);
        }
        self.declare("g0", Kind::Generator, false);
        // A class binding is *not* hoisted, so it is emitted before the
        // top-level statements and must not be reached from a call made
        // earlier. Nothing in the preamble calls anything.
        self.declare("C0", Kind::Class, false);

        // `nullable` feeds optional chaining. Fixing it per seed lets the
        // minifier constant-fold the chain, which is itself worth checking.
        let nullable = self.pick(&["obj", "null", "undefined"]).to_owned();
        let _ = writeln!(source, "var nullable = {nullable};");
        self.declare("nullable", Kind::Value, false);

        source.push_str(&self.class_declaration());
        for index in 0..3 {
            source.push_str(&self.function_declaration(index));
        }
        source.push_str(&self.generator_declaration());

        let context = StatementContext { in_function: false, loop_depth: 0 };
        for _ in 0..self.range(4..=7) {
            source.push_str(&self.statement(MAX_STATEMENT_DEPTH, context));
        }
        // Drain once at the end so closures captured but never called still
        // contribute to the comparison.
        source.push_str(&self.drain_thunks());

        let reported = self.reportable_bindings();
        let _ = writeln!(
            source,
            "console.log(null, a, b, c, x, y, z, _calls_, obj, arr, trace{});",
            reported.iter().fold(String::new(), |mut acc, name| {
                let _ = write!(acc, ", {name}");
                acc
            })
        );
        source.push_str("})();\n");
        source
    }

    // ---------------------------------------------------------------- scopes

    fn push_scope(&mut self, function_boundary: bool) {
        self.scopes.push(Scope { function_boundary, bindings: Vec::new() });
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Declare in the innermost scope, as `let`, `const` and parameters do.
    fn declare(&mut self, name: &str, kind: Kind, mutable: bool) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.bindings.push(Binding { name: name.to_owned(), kind, mutable, readable: true });
        }
    }

    /// Declare a name that shadows whatever is outside but must never be read.
    fn declare_opaque(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.bindings.push(Binding {
                name: name.to_owned(),
                kind: Kind::Value,
                mutable: false,
                readable: false,
            });
        }
    }

    /// Declare in the nearest function scope, as `var` does.
    fn declare_hoisted(&mut self, name: &str, mutable: bool) {
        let index = self.scopes.iter().rposition(|scope| scope.function_boundary);
        if let Some(scope) = index.and_then(|index| self.scopes.get_mut(index)) {
            scope.bindings.push(Binding {
                name: name.to_owned(),
                kind: Kind::Value,
                mutable,
                readable: true,
            });
        }
    }

    /// Names usable at the current point, innermost declaration winning. A name
    /// redeclared further in shadows the outer one, so the outer binding must
    /// not be offered — otherwise generated code would think it is reading `a`
    /// while actually reading the `catch` parameter that shadows it.
    fn names_where(&self, predicate: impl Fn(&Binding) -> bool) -> Vec<String> {
        let mut seen = FxHashSet::default();
        let mut names = Vec::new();
        for binding in self.scopes.iter().rev().flat_map(|scope| scope.bindings.iter().rev()) {
            if seen.insert(binding.name.as_str()) && binding.readable && predicate(binding) {
                names.push(binding.name.clone());
            }
        }
        names
    }

    /// Pick one of `names`, or fall back to a literal when nothing is in scope.
    fn pick_name(&mut self, names: &[String]) -> Option<String> {
        if names.is_empty() {
            return None;
        }
        Some(names[self.range(0..names.len())].clone())
    }

    /// Values the final `console.log` reports, on top of the fixed preamble
    /// bindings. Only the outermost function scope survives to that point.
    fn reportable_bindings(&self) -> Vec<String> {
        self.scopes
            .first()
            .map(|scope| {
                scope
                    .bindings
                    .iter()
                    .filter(|binding| binding.kind == Kind::Value && binding.name.starts_with('v'))
                    .map(|binding| binding.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Run `body` inside a fresh function scope. Labels do not cross a function
    /// boundary, and neither does `this` unless the function is an arrow.
    fn in_function_scope<T>(&mut self, keeps_this: bool, body: impl FnOnce(&mut Self) -> T) -> T {
        let labels = std::mem::take(&mut self.labels);
        let this_available = self.this_available;
        if !keeps_this {
            self.this_available = false;
        }
        self.push_scope(true);
        let result = body(self);
        self.pop_scope();
        self.this_available = this_available;
        self.labels = labels;
        result
    }

    // ----------------------------------------------------------- declarations

    fn function_declaration(&mut self, index: usize) -> String {
        let (parameters, names) = match index {
            0 => ("p0, p1".to_owned(), vec!["p0", "p1"]),
            1 => (format!("p0, p1 = {}", self.literal()), vec!["p0", "p1"]),
            _ => ("...rest".to_owned(), vec!["rest"]),
        };
        let body = self.in_function_scope(false, |generator| {
            for name in names {
                generator.declare(name, Kind::Value, true);
            }
            let mut body = String::from("if (--_calls_ < 0) return 0;\n");
            let context = StatementContext { in_function: true, loop_depth: 0 };
            for _ in 0..generator.range(1..=3) {
                body.push_str(&generator.statement(3, context));
            }
            let _ = writeln!(body, "return {};", generator.expression(MAX_EXPRESSION_DEPTH));
            body
        });
        format!("function f{index}({parameters}) {{\n{body}}}\n")
    }

    fn generator_declaration(&mut self) -> String {
        let body = self.in_function_scope(false, |generator| {
            generator.declare("p0", Kind::Value, true);
            let first = generator.expression(2);
            let second = generator.expression(2);
            format!("if (--_calls_ < 0) return;\nyield {first};\nyield {second};\n")
        });
        format!("function* g0(p0) {{\n{body}}}\n")
    }

    /// A single class, with the members a mangler and a property-order pass
    /// have to get right: a private field, an accessor pair, a static member
    /// and an instance method.
    fn class_declaration(&mut self) -> String {
        let static_field = self.literal();
        let private_field = self.literal();
        let effect = self.fresh_effect();
        // Each member is its own function, so each gets its own scope: `q0`
        // belongs to `m` alone, and `this` is absent from the static method.
        let getter = self.member_body(true, &[], 2);
        let setter = self.member_body(true, &["v"], 1);
        let method = self.member_body(true, &["q0"], 2);
        let static_method = self.member_body(false, &[], 2);
        format!(
            "class C0 {{\n\
             static s = {static_field};\n\
             #secret = {private_field};\n\
             constructor(p) {{ if (--_calls_ < 0) {{ this.p = 0; this.q = 0; return; }} trace.push({effect}); this.p = p; this.q = 1; }}\n\
             get g() {{ if (--_calls_ < 0) return 0; return {getter}; }}\n\
             set g(v) {{ if (--_calls_ < 0) return; this.p = {setter}; }}\n\
             m(q0) {{ if (--_calls_ < 0) return 0; return {method}; }}\n\
             static t() {{ if (--_calls_ < 0) return 0; return {static_method}; }}\n\
             peek() {{ return this.#secret; }}\n\
             }}\n"
        )
    }

    fn member_body(&mut self, has_receiver: bool, parameters: &[&str], depth: usize) -> String {
        self.in_function_scope(false, |generator| {
            generator.this_available = has_receiver;
            for parameter in parameters {
                generator.declare(parameter, Kind::Value, true);
            }
            generator.expression(depth)
        })
    }

    // ------------------------------------------------------------- statements

    fn statement(&mut self, depth: usize, context: StatementContext) -> String {
        if depth == 0 {
            return self.simple_statement();
        }

        match self.range(0..24) {
            0..=3 => self.simple_statement(),
            4 => self.declaration_statement(),
            5 => self.destructuring_statement(),
            6 => {
                let condition = self.expression(2);
                let consequent = self.scoped_statement(depth - 1, context);
                let alternate = self.scoped_statement(depth - 1, context);
                format!("if ({condition}) {{\n{consequent}}} else {{\n{alternate}}}\n")
            }
            7 => self.block(depth - 1, context),
            8 | 9 => self.while_statement(depth - 1, context, None),
            10 => self.do_while_statement(depth - 1, context),
            11 | 12 => self.for_statement(depth - 1, context, None),
            13 => self.for_in_statement(depth - 1, context, None),
            14 => self.for_of_statement(depth - 1, context, None),
            15 => self.switch_statement(depth - 1, context),
            16 | 17 => self.try_statement(depth - 1, context),
            18 => self.labeled_statement(depth - 1, context),
            19 => self.nested_function_declaration(depth - 1),
            20 => self.drain_thunks(),
            21 if context.in_function => {
                let expression = self.expression(2);
                format!("return {expression};\n")
            }
            22 if context.loop_depth > 0 => {
                if self.chance(0.5) {
                    "break;\n".into()
                } else {
                    "continue;\n".into()
                }
            }
            23 if !self.labels.is_empty() => self.labeled_jump(),
            // Reached when a guarded arm above did not apply.
            _ => {
                let expression = self.expression(MAX_EXPRESSION_DEPTH);
                format!("{expression};\n")
            }
        }
    }

    fn simple_statement(&mut self) -> String {
        if self.chance(0.25) {
            self.declaration_statement()
        } else {
            let expression = self.expression(MAX_EXPRESSION_DEPTH);
            format!("{expression};\n")
        }
    }

    /// `var`, `let` and `const` differ in hoisting and in whether the binding
    /// may be reassigned, all of which the minifier reasons about.
    fn declaration_statement(&mut self) -> String {
        let name = self.fresh_var();
        let expression = self.expression(MAX_EXPRESSION_DEPTH);
        match self.range(0..3) {
            0 => {
                self.declare_hoisted(&name, true);
                format!("var {name} = {expression};\n")
            }
            1 => {
                self.declare(&name, Kind::Value, true);
                format!("let {name} = {expression};\n")
            }
            _ => {
                self.declare(&name, Kind::Value, false);
                format!("const {name} = {expression};\n")
            }
        }
    }

    fn destructuring_statement(&mut self) -> String {
        let first = self.fresh_var();
        let second = self.fresh_var();
        let keyword = if self.chance(0.5) { "var" } else { "let" };
        let statement = if self.chance(0.5) {
            let head = self.expression(2);
            let tail = self.expression(2);
            let fallback = self.literal();
            format!("{keyword} [{first} = {fallback}, {second}] = [{head}, {tail}];\n")
        } else {
            let value = self.expression(2);
            let fallback = self.literal();
            format!(
                "{keyword} {{ p: {first} = {fallback}, q: {second} = {fallback} }} = {{ p: {value} }};\n"
            )
        };
        for name in [&first, &second] {
            if keyword == "var" {
                self.declare_hoisted(name, true);
            } else {
                self.declare(name, Kind::Value, true);
            }
        }
        statement
    }

    fn block(&mut self, depth: usize, context: StatementContext) -> String {
        self.push_scope(false);
        let mut block = String::from("{\n");
        for _ in 0..self.range(1..=3) {
            block.push_str(&self.statement(depth, context));
        }
        block.push_str("}\n");
        self.pop_scope();
        block
    }

    /// Generate a statement that the caller wraps in braces. The braces make it
    /// a block at runtime, so the generator has to open a scope too — otherwise
    /// a `let` inside would be offered to code that cannot see it.
    fn scoped_statement(&mut self, depth: usize, context: StatementContext) -> String {
        self.push_scope(false);
        let statement = self.statement(depth, context);
        self.pop_scope();
        statement
    }

    /// A loop body gets its own scope so `let` inside it is per-iteration.
    fn loop_body(&mut self, depth: usize, context: StatementContext) -> String {
        let context = StatementContext { loop_depth: context.loop_depth + 1, ..context };
        self.scoped_statement(depth, context)
    }

    fn while_statement(
        &mut self,
        depth: usize,
        context: StatementContext,
        label: Option<&str>,
    ) -> String {
        let brake = self.fresh_brake();
        let condition = self.expression(2);
        let body = self.loop_body(depth, context);
        let label = label_prefix(label);
        format!(
            "var {brake} = {LOOP_BRAKE};\n{label}while (--{brake} > 0 && ({condition})) {{\n{body}}}\n"
        )
    }

    fn do_while_statement(&mut self, depth: usize, context: StatementContext) -> String {
        let brake = self.fresh_brake();
        let condition = self.expression(2);
        let body = self.loop_body(depth, context);
        format!(
            "var {brake} = {LOOP_BRAKE};\ndo {{\n{body}}} while (--{brake} > 0 && ({condition}));\n"
        )
    }

    fn for_statement(
        &mut self,
        depth: usize,
        context: StatementContext,
        label: Option<&str>,
    ) -> String {
        let brake = self.fresh_brake();
        let condition = self.expression(2);
        // A `let` head gets a fresh binding per iteration; a `var` head does
        // not. Closures captured in the body make the difference observable.
        let keyword = if self.chance(0.5) { "let" } else { "var" };
        self.push_scope(false);
        // Readable but never assignable: the head counter is what bounds the
        // loop, so the body must not be able to reset it.
        self.declare(&brake, Kind::Value, false);
        let body = self.loop_body(depth, context);
        self.pop_scope();
        let label = label_prefix(label);
        format!(
            "{label}for ({keyword} {brake} = {LOOP_BRAKE}; {brake}-- > 0 && ({condition}); ) {{\n{body}}}\n"
        )
    }

    fn for_in_statement(
        &mut self,
        depth: usize,
        context: StatementContext,
        label: Option<&str>,
    ) -> String {
        let brake = self.fresh_brake();
        let key = self.fresh_var();
        let keyword = if self.chance(0.5) { "let" } else { "var" };
        let source = self.pick(&["obj", "arr"]).to_owned();
        self.push_scope(false);
        self.declare(&key, Kind::Value, true);
        let body = self.loop_body(depth, context);
        self.pop_scope();
        let label = label_prefix(label);
        format!(
            "var {brake} = {LOOP_BRAKE};\n{label}for ({keyword} {key} in {source}) {{\nif (--{brake} < 0) break;\n{body}}}\n"
        )
    }

    fn for_of_statement(
        &mut self,
        depth: usize,
        context: StatementContext,
        label: Option<&str>,
    ) -> String {
        let brake = self.fresh_brake();
        let element = self.fresh_var();
        let keyword = if self.chance(0.5) { "let" } else { "var" };
        let source = if self.chance(0.5) {
            let argument = self.expression(1);
            format!("g0({argument})")
        } else {
            "arr".to_owned()
        };
        self.push_scope(false);
        self.declare(&element, Kind::Value, true);
        let body = self.loop_body(depth, context);
        self.pop_scope();
        let label = label_prefix(label);
        format!(
            "var {brake} = {LOOP_BRAKE};\n{label}for ({keyword} {element} of {source}) {{\nif (--{brake} < 0) break;\n{body}}}\n"
        )
    }

    fn switch_statement(&mut self, depth: usize, context: StatementContext) -> String {
        let discriminant = self.expression(2);
        self.push_scope(false);
        let first = self.statement(depth, context);
        let second = self.statement(depth, context);
        let default = self.statement(depth, context);
        self.pop_scope();
        // Falling through half the time keeps the cases from collapsing into
        // independent branches.
        let separator = if self.chance(0.5) { "break;\n" } else { "" };
        format!(
            "switch ({discriminant}) {{\ncase 0:\n{first}{separator}case 1:\n{second}break;\ndefault:\n{default}}}\n"
        )
    }

    fn try_statement(&mut self, depth: usize, context: StatementContext) -> String {
        let condition = self.expression(1);
        let thrown = self.expression(1);
        let try_body = self.scoped_statement(depth, context);
        let effect = self.fresh_effect();

        // `finally` that completes abruptly overrides the outcome of `try`,
        // including a pending exception. That interaction is what makes this
        // shape worth generating.
        let with_catch = self.chance(0.75);
        let with_finally = !with_catch || self.chance(0.5);

        let mut statement = format!("try {{\nif ({condition}) throw {thrown};\n{try_body}}}");
        if with_catch {
            // Shadowing an outer scalar with the catch parameter is a case the
            // mangler has to keep distinct from the outer binding.
            let scalars = self.names_where(|binding| {
                binding.mutable && binding.kind == Kind::Value && binding.name.len() <= 3
            });
            let parameter = if self.chance(0.5) {
                self.pick_name(&scalars).unwrap_or_else(|| "caught".to_owned())
            } else {
                "caught".to_owned()
            };
            self.push_scope(false);
            self.declare_opaque(&parameter);
            let catch_body = self.statement(depth, context);
            self.pop_scope();
            // `typeof` keeps the binding live for the mangler without letting
            // the engine's error message into the comparison.
            let _ = write!(
                statement,
                " catch ({parameter}) {{\ntrace.push({effect});\ntrace.push(typeof {parameter});\n{catch_body}}}"
            );
        }
        if with_finally {
            let finally_body = self.scoped_statement(depth, context);
            let _ = write!(statement, " finally {{\n{finally_body}}}");
        }
        statement.push('\n');

        if with_catch {
            return statement;
        }
        // A `try`/`finally` with no `catch` lets the exception escape, which
        // would abort the program and cost the seed. Catching it outside keeps
        // the interesting part — a `finally` that completes abruptly while an
        // exception is pending — without losing the comparison.
        let outer = self.fresh_effect();
        format!("try {{\n{statement}}} catch (escaped) {{\ntrace.push({outer});\n}}\n")
    }

    fn labeled_statement(&mut self, depth: usize, context: StatementContext) -> String {
        let name = self.fresh_label();
        if self.chance(0.6) {
            self.labels.push(Label { name: name.clone(), loop_label: true });
            let statement = match self.range(0..4) {
                0 => self.while_statement(depth, context, Some(&name)),
                1 => self.for_in_statement(depth, context, Some(&name)),
                2 => self.for_of_statement(depth, context, Some(&name)),
                _ => self.for_statement(depth, context, Some(&name)),
            };
            self.labels.pop();
            statement
        } else {
            self.labels.push(Label { name: name.clone(), loop_label: false });
            let body = self.block(depth, context);
            self.labels.pop();
            format!("{name}: {body}")
        }
    }

    fn labeled_jump(&mut self) -> String {
        let index = self.range(0..self.labels.len());
        let label = &self.labels[index];
        let (name, loop_label) = (label.name.clone(), label.loop_label);
        if loop_label && self.chance(0.5) {
            format!("continue {name};\n")
        } else {
            format!("break {name};\n")
        }
    }

    fn nested_function_declaration(&mut self, depth: usize) -> String {
        let name = self.fresh_closure();
        self.declare(&name, Kind::Function, false);
        let body = self.in_function_scope(false, |generator| {
            generator.declare("p0", Kind::Value, true);
            let mut body = String::from("if (--_calls_ < 0) return 0;\n");
            let context = StatementContext { in_function: true, loop_depth: 0 };
            if depth > 0 {
                body.push_str(&generator.statement(depth - 1, context));
            }
            let _ = writeln!(body, "return {};", generator.expression(2));
            body
        });
        format!("function {name}(p0) {{\n{body}}}\n")
    }

    /// Call every closure captured so far. Closures created in a `let` loop
    /// head see a per-iteration binding; ones created in a `var` loop do not.
    fn drain_thunks(&mut self) -> String {
        let brake = self.fresh_brake();
        format!(
            "var {brake} = 16;\nwhile (thunks.length > 0 && --{brake} > 0) trace.push(thunks.shift()());\n"
        )
    }

    // ------------------------------------------------------------ expressions

    fn expression(&mut self, depth: usize) -> String {
        if depth == 0 {
            return self.primitive();
        }

        match self.range(0..24) {
            0 | 1 => self.primitive(),
            2 | 3 => {
                let left = self.expression(depth - 1);
                let right = self.expression(depth - 1);
                let operator = self.pick(&[
                    "+", "-", "*", "/", "%", "&", "|", "^", "<<", ">>", ">>>", "<", "<=", ">",
                    ">=", "==", "===", "!=", "!==",
                ]);
                format!("({left} {operator} {right})")
            }
            4 => {
                let left = self.expression(depth - 1);
                let right = self.expression(depth - 1);
                let operator = self.pick(&["&&", "||", "??"]);
                format!("({left} {operator} {right})")
            }
            5 => {
                let condition = self.expression(depth - 1);
                let consequent = self.expression(depth - 1);
                let alternate = self.expression(depth - 1);
                format!("({condition} ? {consequent} : {alternate})")
            }
            6 => {
                let argument = self.expression(depth - 1);
                let operator = self.pick(&["+", "-", "!", "~", "void "]);
                format!("({operator} {argument})")
            }
            7 => {
                let effect = self.fresh_effect();
                let value = self.expression(depth - 1);
                format!("(trace.push({effect}), {value})")
            }
            8 => {
                let target = self.mutable_target();
                let value = self.expression(depth - 1);
                let operator = self.pick(&["=", "+=", "-=", "*=", "|=", "^=", "&&=", "||=", "??="]);
                format!("({target} {operator} {value})")
            }
            9 => {
                let target = self.mutable_target();
                let operator = self.pick(&["++", "--"]);
                if self.chance(0.5) {
                    format!("({target}{operator})")
                } else {
                    format!("({operator}{target})")
                }
            }
            10 | 11 => self.call_expression(depth),
            12 => {
                let first = self.expression(depth - 1);
                let second = self.expression(depth - 1);
                let index = self.range(0..3);
                if self.chance(0.5) {
                    format!("([{first}, {second}])[{index}]")
                } else {
                    format!("([{first}, ...arr, {second}]).length")
                }
            }
            13 => {
                let value = self.expression(depth - 1);
                let effect = self.fresh_effect();
                if self.chance(0.5) {
                    format!("({{ p: {value} }}).p")
                } else {
                    format!("({{ get p() {{ trace.push({effect}); return {value}; }} }}).p")
                }
            }
            14 => {
                let left = self.expression(depth - 1);
                let right = self.expression(depth - 1);
                format!("({left}, {right})")
            }
            15 => self.operator_expression(depth),
            16 => {
                let first = self.expression(depth - 1);
                let second = self.expression(depth - 1);
                format!("(`${{{first}}}|${{{second}}}`)")
            }
            17 => {
                let base = self.expression(depth - 1);
                let exponent = self.expression(depth - 1);
                // The base of `**` may not be an unparenthesized unary
                // expression, and primitives such as `-3` are exactly that.
                format!("(({base}) ** ({exponent}))")
            }
            18 => self.optional_chain(depth),
            19 | 20 => self.closure_expression(depth),
            21 => {
                let argument = self.expression(depth - 1);
                format!("([...g0({argument})]).length")
            }
            22 => self.thunk_expression(),
            _ => "(arguments.length)".to_owned(),
        }
    }

    fn call_expression(&mut self, depth: usize) -> String {
        let functions = self.names_where(|binding| binding.kind == Kind::Function);
        if functions.is_empty() || self.chance(0.25) {
            // Class members: construction, an accessor, an instance method, a
            // static member and a private field read.
            let argument = self.expression(depth - 1);
            return match self.range(0..5) {
                0 => format!("(new C0({argument})).g"),
                1 => {
                    let second = self.expression(depth - 1);
                    format!("(new C0({argument})).m({second})")
                }
                2 => format!("(new C0({argument})).peek()"),
                3 => "(C0.t())".to_owned(),
                _ => "(C0.s)".to_owned(),
            };
        }
        let Some(name) = self.pick_name(&functions) else { return self.primitive() };
        let first = self.expression(depth - 1);
        let second = self.expression(depth - 1);
        format!("{name}({first}, {second})")
    }

    fn operator_expression(&mut self, depth: usize) -> String {
        match self.range(0..5) {
            0 => {
                let value = self.expression(depth - 1);
                format!("(typeof {value})")
            }
            // `typeof` is the one way to touch an undeclared name without a
            // `ReferenceError`, so the minifier must not rewrite it into a read.
            1 => "(typeof notDeclared)".to_owned(),
            2 => self.pick(&["(delete obj.q)", "(delete arr[1])"]).to_owned(),
            3 => self.pick(&["('p' in obj)", "('q' in obj)", "(0 in arr)"]).to_owned(),
            _ => self
                .pick(&["(arr instanceof Array)", "(obj instanceof Object)", "(obj instanceof C0)"])
                .to_owned(),
        }
    }

    fn optional_chain(&mut self, depth: usize) -> String {
        match self.range(0..4) {
            0 => "(nullable?.p)".to_owned(),
            1 => "(nullable?.m())".to_owned(),
            2 => {
                let value = self.expression(depth - 1);
                format!("(({value})?.p)")
            }
            _ => {
                let value = self.expression(depth - 1);
                format!("(({value})?.[0])")
            }
        }
    }

    /// Immediately invoked function and arrow expressions. The named form
    /// references itself, which the mangler has to keep resolvable.
    fn closure_expression(&mut self, depth: usize) -> String {
        match self.range(0..3) {
            0 => {
                let body =
                    self.in_function_scope(false, |generator| generator.expression(depth - 1));
                format!("((function () {{ if (--_calls_ < 0) return 0; return {body}; }})())")
            }
            1 => {
                let name = self.fresh_closure();
                let body = self.in_function_scope(false, |generator| {
                    generator.declare(&name, Kind::Function, false);
                    generator.declare("n", Kind::Value, true);
                    generator.expression(depth - 1)
                });
                format!(
                    "((function {name}(n) {{ if (--_calls_ < 0) return 0; return n > 0 ? {name}(n - 1) : {body}; }})(2))"
                )
            }
            _ => {
                // An arrow keeps the enclosing `this`, so `this.p` stays legal.
                let body = self.in_function_scope(true, |generator| {
                    generator.declare("n", Kind::Value, true);
                    generator.expression(depth - 1)
                });
                let argument = self.expression(depth - 1);
                format!("(((n) => {body})({argument}))")
            }
        }
    }

    /// Capture a binding in a closure that is called later, so that a wrong
    /// per-iteration binding shows up in `trace` rather than being invisible.
    fn thunk_expression(&mut self) -> String {
        let readable = self.names_where(|binding| binding.kind == Kind::Value);
        let Some(name) = self.pick_name(&readable) else { return "0".to_owned() };
        format!("(thunks.push(() => {name}), 0)")
    }

    // --------------------------------------------------------------- operands

    fn primitive(&mut self) -> String {
        match self.range(0..12) {
            0..=4 => self.literal().to_owned(),
            5..=7 => {
                let readable = self.names_where(|binding| binding.kind == Kind::Value);
                self.pick_name(&readable).unwrap_or_else(|| "0".to_owned())
            }
            8 | 9 => self
                .pick(&["obj.p", "obj.q", "arr[0]", "arr[1]", "arr[2]", "arr.length"])
                .to_owned(),
            // Reads that are *not* pure: an accessor and two coercion hooks.
            10 => self.pick(&["side.g", "prim", "str"]).to_owned(),
            _ if self.this_available => self.pick(&["this.p", "this.q"]).to_owned(),
            _ => self.literal().to_owned(),
        }
    }

    fn literal(&mut self) -> &'static str {
        self.pick(&[
            "-3",
            "-1",
            "-0",
            "0",
            "0.5",
            "1",
            "2",
            "3",
            "10",
            "42",
            "1e21",
            "0x7fffffff",
            "2147483648",
            "9007199254740993",
            "true",
            "false",
            "null",
            "undefined",
            "NaN",
            "Infinity",
            "''",
            "'0'",
            "' '",
            "'x'",
            "'10'",
            "'1e3'",
        ])
    }

    fn mutable_target(&mut self) -> String {
        let writable = self.names_where(|binding| binding.mutable && binding.kind == Kind::Value);
        match self.range(0..6) {
            0..=3 if !writable.is_empty() => {
                self.pick_name(&writable).unwrap_or_else(|| "obj.p".to_owned())
            }
            4 => self.pick(&["obj.p", "obj.q", "arr[0]", "arr[1]", "side.s"]).to_owned(),
            _ if self.this_available => self.pick(&["this.p", "this.q"]).to_owned(),
            _ => self.pick(&["obj.p", "obj.q", "arr[0]", "arr[1]"]).to_owned(),
        }
    }

    // --------------------------------------------------------------- counters

    fn fresh_var(&mut self) -> String {
        let value = self.next_var;
        self.next_var += 1;
        format!("v{value}")
    }

    fn fresh_brake(&mut self) -> String {
        let value = self.next_brake;
        self.next_brake += 1;
        format!("brake{value}")
    }

    fn fresh_label(&mut self) -> String {
        let value = self.next_label;
        self.next_label += 1;
        format!("label{value}")
    }

    fn fresh_closure(&mut self) -> String {
        let value = self.next_closure;
        self.next_closure += 1;
        format!("h{value}")
    }

    fn fresh_effect(&mut self) -> usize {
        let value = self.next_effect;
        self.next_effect += 1;
        value
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

fn label_prefix(label: Option<&str>) -> String {
    label.map_or_else(String::new, |label| format!("{label}: "))
}
