use oxc_macros::declare_oxc_lint;
use oxc_react_compiler::ErrorCategory;

use crate::{
    context::{ContextHost, LintContext},
    rule::Rule,
    utils::{run_react_compiler_rule, should_run_react_compiler},
};

#[derive(Debug, Default, Clone)]
pub struct Invariant;

declare_oxc_lint!(
    /// ### What it does
    ///
    /// Reports internal React Compiler invariant violations. These indicate a
    /// bug in the compiler itself, not in your code — consider reporting them
    /// to the oxc or React teams.
    ///
    /// Powered by the React Compiler, which runs once per file and is shared
    /// with the other React Compiler rules. Port of
    /// [`react-hooks/invariant`](https://react.dev/reference/eslint-plugin-react-hooks/lints/invariant).
    ///
    /// ### Why is this bad?
    ///
    /// An invariant violation means the compiler's internal state is
    /// inconsistent; the affected function is skipped rather than optimized.
    Invariant,
    react,
    nursery,
    version = "next",
);

impl Rule for Invariant {
    fn run_once(&self, ctx: &LintContext) {
        run_react_compiler_rule(ctx, ErrorCategory::Invariant);
    }

    fn should_run(&self, ctx: &ContextHost) -> bool {
        should_run_react_compiler(ctx)
    }
}

#[test]
fn test() {
    use crate::tester::Tester;

    let pass = vec![
        // ---- PluginTest-test.ts ----
        // [Invariant] Defined after use
        // (OK because invariants are only meant for the compiler team's consumption)
        "
function Component(props) {
  let y = function () {
    m(x);
  };

  let x = { a };
  m(x);
  return y;
}
",
    ];

    let fail = vec![];

    Tester::new(Invariant::NAME, Invariant::PLUGIN, pass, fail).test_and_snapshot();
}
