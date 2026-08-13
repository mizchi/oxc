# Minifier fuzzer

Four ways of asking whether the minifier preserves meaning. All of them are
manual tools — none is wired into CI.

```sh
just fuzz-minifier --seed 0 --iterations 10000    # generated programs
just fuzz-minifier --contexts --iterations 10000  # one pattern, every context
just fuzz-minifier --scopes --mangle              # name resolution
just fuzz-minifier --corpus                       # Terser's compress suite
just fuzz-minifier --invariants --iterations 200000
```

`--mangle` adds name mangling to any of the first four, and `--invariants`
accepts `--contexts` and `--scopes` too. Without it, a mismatch
is attributable to compression or code generation rather than to name
allocation.

## Generated programs

Follows the central idea of Terser's
[`ufuzz`](https://github.com/terser/terser/blob/v5.50.0/test/ufuzz.js): generate
deterministic, self-contained programs with bounded loops and calls, execute the
original and compressed forms, and compare their observable behavior.

Programs share a function-call budget and give every loop its own brake, so they
always terminate. Values are compared as tagged JSON, so `undefined`, `NaN`,
`-0`, `Infinity` and array holes stay distinguishable, and an execution `trace`
records ordering, catching a pass that computes the right answer by running the
wrong things. Inputs that throw or time out before minification are skipped:
nothing can be concluded from those.

The generator tracks scopes, so generated code reads the bindings it declares
and a name declared further in shadows the outer one. It emits `let`/`const`,
destructuring, labels, `try`/`finally`, `for-in`/`for-of`, generators, classes
with a private field and an accessor pair, nested and named function
expressions, arrows, optional chaining, `**`, template literals, and closures
captured in one place and called in another.

Two probes make an optimisation observable rather than merely value-preserving:
an accessor pair that appends to `trace`, and coercion hooks for `valueOf` and
`toString`. The coercion hooks are deliberately silent — oxc documents that it
treats `ToPrimitive` invoked by `==` and the relational operators as
side-effect free, so a loud hook would report that accepted trade-off as a
mismatch on nearly every seed.

On a mismatch the source, compressed source and structured outcomes are written
under `target/minifier-fuzz/` under the failing seed's name, along with a
delta-debugged reduction (`--no-shrink` turns that off). Re-run one seed with
`--seed <N> --iterations 1`.

## Context invariance

Modelled on esbuild's
[`destructuring-fuzzer.js`](https://github.com/evanw/esbuild/blob/main/scripts/destructuring-fuzzer.js).
A binding pattern means the same thing wherever it appears, so one generated
pattern is bound in all thirteen contexts that accept it — `var`/`let`/`const`,
assignment, function, arrow, method and class-method parameters, a default
initialiser, `catch`, and both `for-of` heads — and each context reports what it
bound. Compression sees a different AST in each, so a pass that gets one wrong
shows up as that context disagreeing with the other twelve.

## Name resolution

Aimed at the mangler. The program generator above gives every binding a unique
name, which is the easiest possible input for one: nothing shadows anything, so
no reference can resolve to the wrong binding however the renaming goes. Running
it with `--mangle` therefore says very little.

`--scopes` does the opposite. It draws from a pool of three names and reuses
them down a deep chain of nested scopes, so most names are shadowed several
times over. Every binding gets a distinct value and every read is logged with
the name it was written as, so the log says which binding each read resolved to.

It also covers the places where a name is not an ordinary reference, and so must
not be renamed along with one: shorthand properties, property keys, labels,
private class fields, `catch` parameters, named function expressions, and the
scope a direct `eval` can see. A scope of 80 bindings appears too, which pushes
the name allocator past the 54 single-character identifiers into two-character
ones — where a generated name can turn out to be a reserved word.

Lexical declarations are emitted at the top of each scope on purpose. `let`,
`const`, `class` and block-level functions bind their name across the *whole*
scope rather than from the declaration onwards, so a read placed above one
throws on the temporal dead zone instead of resolving outward, and a `var` in a
nested block collides with one introduced later in an enclosing block.

## Terser's compress suite

Modelled on esbuild's
[`terser-tests.js`](https://github.com/evanw/esbuild/blob/main/scripts/terser-tests.js).
The generator explores a space we chose; `test/compress` is a space minifier
authors chose, accumulated over a decade of UglifyJS and Terser bug reports.
Every case carrying `expect_stdout` is run through oxc and compared with its
own input. The per-case `options` block is ignored: the corpus supplies inputs,
the comparison is ours.

The suite is provisioned on demand from a pinned tag into `terser/`
(gitignored), the way `oxc_formatter_tests` provisions Prettier. The first run
needs the network; later ones do not.

## Invariants

Checks that need no Node.js, so they sweep seed ranges two orders of magnitude
larger: the compressed program parses and binds, minifying it again never
produces more code, and the output is a fixed point. Output that fails to parse
is invisible to the runtime comparison — it is recorded there as "both threw"
and skipped — so this is the only mode that can see it.

Violations are collected rather than stopping the sweep, since they are rare
and independent.
