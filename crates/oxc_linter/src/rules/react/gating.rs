use oxc_macros::declare_oxc_lint;
use oxc_react_compiler::ErrorCategory;

use crate::{
    context::{ContextHost, LintContext},
    rule::Rule,
    utils::{run_react_compiler_rule, should_run_react_compiler},
};

#[derive(Debug, Default, Clone)]
pub struct Gating;

declare_oxc_lint!(
    /// ### What it does
    ///
    /// Reports invalid configuration of React Compiler's gating mode.
    ///
    /// Powered by the React Compiler, which runs once per file and is shared
    /// with the other React Compiler rules. Port of
    /// [`react-hooks/gating`](https://react.dev/reference/eslint-plugin-react-hooks/lints/gating).
    ///
    /// ::: warning
    /// This rule is currently inactive: oxlint never configures the React
    /// Compiler's gating mode, so gating errors cannot occur yet. It will
    /// activate once React Compiler options become configurable in oxlint.
    /// :::
    ///
    /// ### Why is this bad?
    ///
    /// A broken gating setup means the compiled and uncompiled variants of a
    /// component can be selected incorrectly at runtime.
    Gating,
    react,
    nursery,
    version = "next",
);

impl Rule for Gating {
    fn run_once(&self, ctx: &LintContext) {
        run_react_compiler_rule(ctx, ErrorCategory::Gating);
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

    let fail = vec![];

    Tester::new(Gating::NAME, Gating::PLUGIN, pass, fail).test_and_snapshot();
}
