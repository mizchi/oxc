use oxc_macros::declare_oxc_lint;
use oxc_react_compiler::ErrorCategory;

use crate::{
    context::{ContextHost, LintContext},
    rule::Rule,
    utils::{run_react_compiler_rule, should_run_react_compiler},
};

#[derive(Debug, Default, Clone)]
pub struct VoidUseMemo;

declare_oxc_lint!(
    /// ### What it does
    ///
    /// Validates that `useMemo()` callbacks return a value and that the
    /// memoized result is actually used by the component or hook.
    ///
    /// Powered by the React Compiler, which runs once per file and is shared
    /// with the other React Compiler rules. Port of
    /// [`react-hooks/void-use-memo`](https://react.dev/reference/eslint-plugin-react-hooks/lints/void-use-memo).
    ///
    /// ### Why is this bad?
    ///
    /// A `useMemo` callback that returns nothing, or whose result is never
    /// used, is not memoizing anything — it is usually a side effect in
    /// disguise, which belongs in an event handler or effect instead.
    ///
    /// ### Examples
    ///
    /// Examples of **incorrect** code for this rule:
    /// ```jsx
    /// import { useMemo } from 'react';
    /// function Component({ a }) {
    ///   useMemo(() => {
    ///     console.log(a); // returns nothing, result unused
    ///   }, [a]);
    ///   return <div>{a}</div>;
    /// }
    /// ```
    ///
    /// Examples of **correct** code for this rule:
    /// ```jsx
    /// import { useMemo } from 'react';
    /// function Component({ a }) {
    ///   const x = useMemo(() => a + 1, [a]);
    ///   return <div>{x}</div>;
    /// }
    /// ```
    VoidUseMemo,
    react,
    nursery,
    version = "next",
    short_description = "Validates that `useMemo()` callbacks return a value and the result is used.",
);

impl Rule for VoidUseMemo {
    fn run_once(&self, ctx: &LintContext) {
        run_react_compiler_rule(ctx, ErrorCategory::VoidUseMemo);
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
import {useMemo} from 'react';
function Component({a}) {
  const x = useMemo(() => a + 1, [a]);
  return <div>{x}</div>;
}
",
    ];

    let fail = vec![
        // ---- PluginTest-test.ts ----
        // Multiple non-fatal useMemo diagnostics are surfaced
        // Also produces RenderSetState findings, reported only by react/set-state-in-render.
        "
import {useMemo, useState} from 'react';

function Component({item, cond}) {
  const [prevItem, setPrevItem] = useState(item);
  const [state, setState] = useState(0);

  useMemo(() => {
    if (cond) {
      setPrevItem(item);
      setState(0);
    }
  }, [cond, item, init]);

  return <Child x={state} />;
  }",
    ];

    Tester::new(VoidUseMemo::NAME, VoidUseMemo::PLUGIN, pass, fail).test_and_snapshot();
}
