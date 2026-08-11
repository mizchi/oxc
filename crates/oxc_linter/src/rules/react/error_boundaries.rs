use oxc_macros::declare_oxc_lint;
use oxc_react_compiler::ErrorCategory;

use crate::{
    context::{ContextHost, LintContext},
    rule::Rule,
    utils::{run_react_compiler_rule, should_run_react_compiler},
};

#[derive(Debug, Default, Clone)]
pub struct ErrorBoundaries;

declare_oxc_lint!(
    /// ### What it does
    ///
    /// Validates using error boundaries instead of `try`/`catch` around JSX
    /// for errors in child components.
    ///
    /// Powered by the React Compiler, which runs once per file and is shared
    /// with the other React Compiler rules. Port of
    /// [`react-hooks/error-boundaries`](https://react.dev/reference/eslint-plugin-react-hooks/lints/error-boundaries).
    ///
    /// ### Why is this bad?
    ///
    /// React renders components lazily — the child has not rendered yet
    /// inside the `try` block, so the `catch` never sees its errors; only an
    /// error boundary can catch them.
    ///
    /// ### Examples
    ///
    /// Examples of **incorrect** code for this rule:
    /// ```jsx
    /// function Component(props) {
    ///   let el;
    ///   try {
    ///     el = <Child />;
    ///   } catch {
    ///     return null;
    ///   }
    ///   return el;
    /// }
    /// ```
    ///
    /// Examples of **correct** code for this rule:
    /// ```jsx
    /// function Component(props) {
    ///   return (
    ///     <ErrorBoundary fallback={null}>
    ///       <Child />
    ///     </ErrorBoundary>
    ///   );
    /// }
    /// ```
    ErrorBoundaries,
    react,
    nursery,
    version = "next",
);

impl Rule for ErrorBoundaries {
    fn run_once(&self, ctx: &LintContext) {
        run_react_compiler_rule(ctx, ErrorCategory::ErrorBoundaries);
    }

    fn should_run(&self, ctx: &ContextHost) -> bool {
        should_run_react_compiler(ctx)
    }
}

#[test]
fn test() {
    use crate::tester::Tester;

    let pass = vec![
        "
function Component(props) {
  return <div>{props.text}</div>;
}
",
    ];

    let fail = vec![
        // ---- NoAmbiguousJsxRule-test.ts ----
        // JSX in try blocks are warned against
        "
function Component(props) {
  let el;
  try {
    el = <Child />;
  } catch {
    return null;
  }
  return el;
}
",
    ];

    Tester::new(ErrorBoundaries::NAME, ErrorBoundaries::PLUGIN, pass, fail).test_and_snapshot();
}
