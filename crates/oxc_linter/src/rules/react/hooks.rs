use oxc_macros::declare_oxc_lint;
use oxc_react_compiler::ErrorCategory;

use crate::{
    context::{ContextHost, LintContext},
    rule::Rule,
    utils::{run_react_compiler_rule, should_run_react_compiler},
};

#[derive(Debug, Default, Clone)]
pub struct Hooks;

declare_oxc_lint!(
    /// ### What it does
    ///
    /// Runs the React Compiler's Rules of Hooks validation: hooks must be
    /// called unconditionally, in a consistent order, at the top level of a
    /// component or hook, and not be used as first-class values.
    ///
    /// Powered by the React Compiler, which runs once per file and is shared
    /// with the other React Compiler rules. Port of
    /// [`react-hooks/hooks`](https://react.dev/reference/eslint-plugin-react-hooks/lints/hooks).
    ///
    /// This rule overlaps with `react/rules-of-hooks`; upstream ships it
    /// disabled for that reason.
    ///
    /// ### Why is this bad?
    ///
    /// React tracks hook state by call order. A hook that is called
    /// conditionally or in a different order between renders breaks the
    /// association between each hook call and its state, corrupting
    /// component state.
    ///
    /// ### Examples
    ///
    /// Examples of **incorrect** code for this rule:
    /// ```jsx
    /// function Component(props) {
    ///   if (props.cond) {
    ///     useState(0); // hooks may not be called conditionally
    ///   }
    ///   return <div>{props.text}</div>;
    /// }
    /// ```
    ///
    /// Examples of **correct** code for this rule:
    /// ```jsx
    /// function Component(props) {
    ///   const [state, setState] = useState(0);
    ///   return <div onClick={() => setState(state + 1)}>{props.text}</div>;
    /// }
    /// ```
    Hooks,
    react,
    nursery,
    version = "next",
    short_description = "Validates the Rules of Hooks with the React Compiler's analysis.",
);

impl Rule for Hooks {
    fn run_once(&self, ctx: &LintContext) {
        run_react_compiler_rule(ctx, ErrorCategory::Hooks);
    }

    fn should_run(&self, ctx: &ContextHost) -> bool {
        should_run_react_compiler(ctx)
    }
}

#[test]
fn test() {
    use crate::tester::Tester;

    let pass = vec![
        // ---- InvalidHooksRule-test.ts ----
        // Basic example
        "
function Component() {
  useHook();
  return <div>Hello world</div>;
}
",
        // Violation with Flow suppression
        "
      // Valid since error already suppressed with flow.
      function useHook() {
        if (cond) {
          // $FlowFixMe[react-rule-hook]
          useConditionalHook();
        }
      }
    ",
        // ---- RustBackend-test.ts ----
        // Component with hooks compiles without errors
        "
import {useState} from 'react';
function Component(props) {
  const [state, setState] = useState(0);
  return <div onClick={() => setState(state + 1)}>{state}</div>;
}
",
        // ---- oxlint-specific ----
        // A bail-out (local named `fbt`) is a Todo diagnostic, not a Hooks
        // violation.
        "function Component() {
                const fbt = 'span';
                return <fbt desc='label'>Hello</fbt>;
            }",
    ];

    let fail = vec![
        // ---- PluginTest-test.ts ----
        // Multiple diagnostic kinds from the same function are surfaced
        // Also produces a CapitalizedCalls finding, reported only by react/capitalized-calls.
        "
import Child from './Child';
function Component() {
  const result = cond ?? useConditionalHook();
  return <>
    {Child(result)}
  </>;
}
",
        // Multiple diagnostics within the same file are surfaced
        "
function useConditional1() {
  'use memo';
  return cond ?? useConditionalHook();
}
function useConditional2(props) {
  'use memo';
  return props.cond && useConditionalHook();
}",
        // 'use no forget' does not disable eslint rule
        "
let count = 0;
function Component() {
  'use no forget';
  return cond ?? useConditionalHook();

}
",
        // ---- InvalidHooksRule-test.ts ----
        // Simple violation
        "
function useConditional() {
  if (cond) {
    useConditionalHook();
  }
}
",
        // Multiple diagnostics within the same function are surfaced
        "
function useConditional() {
  cond ?? useConditionalHook();
  props.cond && useConditionalHook();
  return <div>Hello world</div>;
}",
    ];

    Tester::new(Hooks::NAME, Hooks::PLUGIN, pass, fail).test_and_snapshot();
}
