use oxc_macros::declare_oxc_lint;
use oxc_react_compiler::ErrorCategory;

use crate::{
    context::{ContextHost, LintContext},
    rule::Rule,
    utils::{run_react_compiler_rule, should_run_react_compiler},
};

#[derive(Debug, Default, Clone)]
pub struct ExhaustiveEffectDependencies;

declare_oxc_lint!(
    /// ### What it does
    ///
    /// Validates that effect dependency arrays are exhaustive and contain no
    /// extraneous values.
    ///
    /// Powered by the React Compiler, which runs once per file and is shared
    /// with the other React Compiler rules. Port of
    /// [`react-hooks/exhaustive-effect-dependencies`](https://react.dev/reference/eslint-plugin-react-hooks/lints/exhaustive-effect-dependencies).
    ///
    /// ::: warning
    /// This rule is currently inactive: the underlying validation
    /// (`validateExhaustiveEffectDependencies`) is disabled in the fixed
    /// options oxlint compiles with, matching the upstream ESLint plugin's
    /// defaults. It will activate once React Compiler options become
    /// configurable in oxlint.
    /// :::
    ///
    /// ### Why is this bad?
    ///
    /// Missing effect dependencies capture stale values from a previous
    /// render; extraneous dependencies re-fire the effect needlessly.
    ExhaustiveEffectDependencies,
    react,
    nursery,
    version = "next",
    short_description = "Validates that effect dependencies are exhaustive, without extraneous values.",
);

impl Rule for ExhaustiveEffectDependencies {
    fn run_once(&self, ctx: &LintContext) {
        run_react_compiler_rule(ctx, ErrorCategory::EffectExhaustiveDependencies);
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

    Tester::new(
        ExhaustiveEffectDependencies::NAME,
        ExhaustiveEffectDependencies::PLUGIN,
        pass,
        fail,
    )
    .test_and_snapshot();
}
