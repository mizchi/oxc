use oxc_macros::declare_oxc_lint;
use oxc_react_compiler::ErrorCategory;

use crate::{
    context::{ContextHost, LintContext},
    rule::Rule,
    utils::{run_react_compiler_rule, should_run_react_compiler},
};

#[derive(Debug, Default, Clone)]
pub struct SetStateInRender;

declare_oxc_lint!(
    /// ### What it does
    ///
    /// Disallows unconditionally setting state during render (including
    /// inside `useMemo` callbacks), which triggers additional renders and can
    /// cause infinite render loops.
    ///
    /// Powered by the React Compiler, which runs once per file and is shared
    /// with the other React Compiler rules. Port of
    /// [`react-hooks/set-state-in-render`](https://react.dev/reference/eslint-plugin-react-hooks/lints/set-state-in-render).
    ///
    /// ### Why is this bad?
    ///
    /// Each render-time `setState` schedules another render; unconditional
    /// ones loop forever, conditional ones still double-render.
    ///
    /// ### Examples
    ///
    /// Examples of **incorrect** code for this rule:
    /// ```jsx
    /// import { useState } from 'react';
    /// function Component() {
    ///   const [state, setState] = useState(0);
    ///   setState(state + 1); // schedules another render on every render
    ///   return <div>{state}</div>;
    /// }
    /// ```
    ///
    /// Examples of **correct** code for this rule:
    /// ```jsx
    /// import { useState } from 'react';
    /// function Component() {
    ///   const [state, setState] = useState(0);
    ///   return <button onClick={() => setState(state + 1)}>{state}</button>;
    /// }
    /// ```
    SetStateInRender,
    react,
    nursery,
    version = "next",
);

impl Rule for SetStateInRender {
    fn run_once(&self, ctx: &LintContext) {
        run_react_compiler_rule(ctx, ErrorCategory::RenderSetState);
    }

    fn should_run(&self, ctx: &ContextHost) -> bool {
        should_run_react_compiler(ctx)
    }
}

#[test]
fn test() {
    use crate::tester::Tester;

    let pass = vec![
        // ---- RustBackend-test.ts ----
        // Component with hooks compiles without errors
        // (setState inside an onClick handler is fine)
        "
import {useState} from 'react';
function Component(props) {
  const [state, setState] = useState(0);
  return <div onClick={() => setState(state + 1)}>{state}</div>;
}
",
    ];

    let fail = vec![
        // ---- PluginTest-test.ts ----
        // Multiple non-fatal useMemo diagnostics are surfaced
        // Also produces a VoidUseMemo finding, reported only by react/void-use-memo.
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

    Tester::new(SetStateInRender::NAME, SetStateInRender::PLUGIN, pass, fail).test_and_snapshot();
}
