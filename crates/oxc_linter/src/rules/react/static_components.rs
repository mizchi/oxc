use oxc_macros::declare_oxc_lint;
use oxc_react_compiler::ErrorCategory;

use crate::{
    context::{ContextHost, LintContext},
    rule::Rule,
    utils::{run_react_compiler_rule, should_run_react_compiler},
};

#[derive(Debug, Default, Clone)]
pub struct StaticComponents;

declare_oxc_lint!(
    /// ### What it does
    ///
    /// Validates that components are static — defined at module scope rather
    /// than recreated on every render — because dynamically recreated
    /// components reset state and cause excessive re-rendering.
    ///
    /// Powered by the React Compiler, which runs once per file and is shared
    /// with the other React Compiler rules. Port of
    /// [`react-hooks/static-components`](https://react.dev/reference/eslint-plugin-react-hooks/lints/static-components).
    ///
    /// ### Why is this bad?
    ///
    /// A component created during render gets a new identity on every render,
    /// so React unmounts and remounts it each time — resetting all of its
    /// state and re-rendering its entire subtree.
    ///
    /// ### Examples
    ///
    /// Examples of **incorrect** code for this rule:
    /// ```jsx
    /// function Example(props) {
    ///   const Component = createComponent();
    ///   return <Component />;
    /// }
    /// ```
    ///
    /// Examples of **correct** code for this rule:
    /// ```jsx
    /// function Inner(props) {
    ///   return <div>{props.text}</div>;
    /// }
    /// function Outer() {
    ///   return <Inner text='hello' />;
    /// }
    /// ```
    StaticComponents,
    react,
    nursery,
    version = "next",
);

impl Rule for StaticComponents {
    fn run_once(&self, ctx: &LintContext) {
        run_react_compiler_rule(ctx, ErrorCategory::StaticComponents);
    }

    fn should_run(&self, ctx: &ContextHost) -> bool {
        should_run_react_compiler(ctx)
    }
}

#[test]
fn test() {
    use crate::tester::Tester;

    let pass = vec![
        // Components defined at module scope are static.
        "
function Inner(props) {
  return <div>{props.text}</div>;
}
function Outer() {
  return <Inner text='hello' />;
}
",
    ];

    let fail = vec![
        // Derived from crates/oxc_react_compiler/fixtures cases.
        "
function Example(props) {
  const Component = createComponent();
  return <Component />;
}
",
    ];

    Tester::new(StaticComponents::NAME, StaticComponents::PLUGIN, pass, fail).test_and_snapshot();
}
