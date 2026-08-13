// Shared machinery for the conformance tests.
//
// Each test takes one fixture's source text and prints it three times:
//
// 1. In Rust, with `oxc_parser` + `oxc_codegen`, via the `oxc_codegen_conformance` Node addon.
// 2. In JS without source maps, by parsing with `oxc-parser` and printing with this package.
// 3. In JS with source maps, through the separately compiled maps-enabled build.
//
// The two must agree byte for byte. Both sides are given the same source text and the same `lang`
// and `sourceType`, and the addon derives its `SourceType` with `oxc_napi::get_source_type` -
// the same function `oxc-parser` uses. So the two printers are handed the same AST and the only
// things under test are printing and decoded source map positions and names.
//
// Fixtures which do not parse cleanly have no AST to print, so the addon returns `null` for them
// and the test is reported as skipped rather than passing quietly.

import { join as pathJoin } from "node:path";
import { codegen as rustPrint } from "oxc-codegen-conformance";
import { parseSync } from "oxc-parser";
import { expect } from "vitest";

import { printSync } from "../../dist/index.js";

import type { Mapping, Position, SourceMapGenerator } from "../../dist/index.js";
import type { Program } from "oxc-parser";

export const ROOT_DIR_PATH = pathJoin(import.meta.dirname, "../../../..");

// The fixture suites, all of them git submodules of this repo. `just submodules` clones them.
export const TEST262_DIR_PATH = pathJoin(ROOT_DIR_PATH, "tasks/coverage/test262/test");
export const TS_CASES_DIR_PATH = pathJoin(ROOT_DIR_PATH, "tasks/coverage/typescript/tests/cases");
export const JSX_DIR_PATH = pathJoin(
  ROOT_DIR_PATH,
  "tasks/coverage/estree-conformance/tests/acorn-jsx/pass",
);

// How the source text is to be parsed. The same values go to both printers.
export type Lang = "js" | "jsx" | "ts" | "tsx";
export type SourceTypeOption = "script" | "module" | "unambiguous";

// --- The check --------------------------------------------------------------------------------

// Both parse modes are checked for every fixture.
//
// `true` is `oxc-parser`'s default, and puts `ParenthesizedExpression` / `TSParenthesizedType`
// wrapper nodes in the AST. `false` drops them, leaving the printer to re-derive every
// parenthesis from precedence. The two are different code paths through the printer,
// so both are worth checking.
const PRESERVE_PARENS_MODES = [false, true];

/** One decoded source map token, using the zero-based positions `oxc_sourcemap` exposes. */
interface SourceMapToken {
  generatedLine: number;
  generatedColumn: number;
  originalLine: number;
  originalColumn: number;
  name?: string;
}

/** Collect source mappings in the same decoded form returned by the Rust reference addon. */
class SourceMapCollector implements SourceMapGenerator {
  file: string;
  mappings: SourceMapToken[] = [];

  constructor(file: string) {
    this.file = file;
  }

  addMapping(mapping: Mapping): void {
    const token: SourceMapToken = {
      generatedLine: mapping.generated.line - 1,
      generatedColumn: mapping.generated.column,
      originalLine: mapping.original.line - 1,
      originalColumn: mapping.original.column,
    };
    if (mapping.name !== undefined) token.name = mapping.name;
    this.mappings.push(token);
  }
}

/**
 * Add ESTree `loc` data to every AST node from its UTF-16 `start` / `end` offsets.
 *
 * `oxc-parser` deliberately does not materialize `loc`; the JavaScript code generator accepts ASTs
 * from parsers which do. Source maps therefore need the same positions synthesized for conformance.
 */
export function addSourceLocations(program: Program, sourceText: string): void {
  const lineStarts = [0];
  for (let index = 0; index < sourceText.length; index++) {
    const char = sourceText.charCodeAt(index);
    if (char === 13 && sourceText.charCodeAt(index + 1) === 10) {
      lineStarts.push(++index + 1);
    } else if (char === 10 || char === 13 || char === 0x2028 || char === 0x2029) {
      lineStarts.push(index + 1);
    }
  }

  const positionAt = (offset: number): Position => {
    let low = 0;
    let high = lineStarts.length - 1;
    while (low < high) {
      const middle = (low + high + 1) >> 1;
      if (lineStarts[middle] <= offset) low = middle;
      else high = middle - 1;
    }
    return { line: low + 1, column: offset - lineStarts[low] };
  };

  const seen = new Set<object>();
  const stack: unknown[] = [program];
  while (stack.length > 0) {
    const value = stack.pop();
    if (value === null || typeof value !== "object" || seen.has(value)) continue;
    seen.add(value);

    if (Array.isArray(value)) {
      for (const child of value) stack.push(child);
      continue;
    }

    const node = value as Record<string, unknown>;
    if (typeof node.type === "string" && typeof node.start === "number") {
      node.loc = { start: positionAt(node.start), end: positionAt(node.end as number) };
    }
    for (const key in node) {
      if (key === "loc" || key === "parent") continue;
      const child = node[key];
      if (child !== null && typeof child === "object") stack.push(child);
    }
  }
}

/**
 * Check this package prints a fixture exactly as Rust `oxc_codegen` does.
 *
 * Checked in both parse modes - see `PRESERVE_PARENS_MODES`.
 *
 * Throws if the two disagree.
 *
 * Returns `false` without checking anything if the fixture does not parse cleanly,
 * so the caller can report the test as skipped.
 *
 * @param filename - Fixture filename, which is what decides the language when `lang` is absent
 * @param sourceText - Fixture source text
 * @param lang - Language to parse as
 * @param sourceType - Script/module/unambiguous
 * @param astType - Which properties the AST carries, defaulting to whatever `lang` implies.
 *   A JS-shaped AST leaves out the TypeScript-only properties - `optional`, `accessibility`,
 *   `override`, `declare` and the rest - which Oxc's Rust AST holds for a `.js` file all the same,
 *   and prints. Fixtures which can contain that syntax must ask for a TS-shaped AST, or the JS
 *   side is being asked to reproduce something it was never told.
 * @returns `true` if the fixture was checked, `false` if it does not parse
 */
export function checkFixture(
  filename: string,
  sourceText: string,
  lang: Lang,
  sourceType: SourceTypeOption,
  astType?: "js" | "ts",
): boolean {
  const ts = astType === undefined ? lang[0] === "t" : astType === "ts";
  const jsx = lang.endsWith("x");

  let checked = false;
  for (const preserveParens of PRESERVE_PARENS_MODES) {
    // Rust first.
    // `null` means the fixture does not parse cleanly, so nothing to compare.
    const expected = rustPrint(filename, sourceText, { lang, sourceType, preserveParens });
    if (expected === null) continue;

    const { program, errors } = parseSync(filename, sourceText, {
      preserveParens,
      lang,
      sourceType,
      astType,
      // Conformance tests require Node.js 22+ for raw transfer. This is a test-harness
      // requirement, not `oxc-codegen`'s runtime engine floor.
      // @ts-expect-error `experimentalRawTransfer` is experimental, so is not in `ParserOptions`
      experimentalRawTransfer: true,
    });

    // Rust parsed it cleanly, so this should not happen - if it does, it is a parser difference
    // rather than a printer one, and saying so is more use than a diff of printed output.
    expect(errors, "Rust parsed this fixture cleanly but `oxc-parser` did not").toEqual([]);

    const actual = printSync(program, { ts, jsx });
    expect(actual, `preserveParens: ${preserveParens}`).toBe(expected.code);

    // Source maps use the maps-enabled build, which is compiled separately from the normal
    // printer. Compare both its code and every decoded mapping against Rust.
    addSourceLocations(program, sourceText);
    const sourceMap = new SourceMapCollector(filename);
    const actualWithSourceMap = printSync(program, { ts, jsx, sourceMap, sourceText });
    expect(actualWithSourceMap, `preserveParens: ${preserveParens}, sourceMap: code`).toBe(
      expected.code,
    );
    expect(sourceMap.mappings, `preserveParens: ${preserveParens}, sourceMap: mappings`).toEqual(
      expected.mappings,
    );
    checked = true;
  }

  return checked;
}

/**
 * The `lang` to parse a unit as, from what it is.
 *
 * @param ts - `true` if the unit is TypeScript
 * @param jsx - `true` if JSX syntax is enabled
 * @returns The `lang` to give both printers
 */
export function langOf(ts: boolean, jsx: boolean): Lang {
  if (ts) return jsx ? "tsx" : "ts";
  return jsx ? "jsx" : "js";
}

// The `flags` value in a Test262 frontmatter block, as either a flow sequence (`[a, b]`)
// or the indented lines of a block sequence.
const FLAGS_REGEX = /^flags:[ \t]*(\[[^\]]*\]|(?:\r?\n[ \t]+-[^\r\n]*)+)/m;

/**
 * Read the `sourceType` a Test262 fixture asks for.
 *
 * Test262 files carry a YAML frontmatter block, in which a `module` flag marks the ones
 * which must be parsed as modules. Everything else is a script.
 *
 * The flags are written either as a YAML flow sequence, `flags: [module]`,
 * or as a block sequence on the lines below `flags:`. Both forms are in use.
 *
 * The `_FIXTURE.js` files are the exception. They are module dependencies of other tests
 * rather than tests themselves, so they carry no frontmatter at all,
 * and plenty of them use `import` or top-level `await`.
 *
 * The comparison stays valid whatever this returns, since both printers are given the same value -
 * getting it right only decides how many fixtures parse, not whether the check is sound.
 *
 * @param sourceText - Fixture source text
 * @returns The source type to parse it as
 */
export function test262SourceType(sourceText: string): SourceTypeOption {
  const frontmatterEnd = sourceText.indexOf("---*/");
  if (frontmatterEnd === -1) return "unambiguous";

  const flags = FLAGS_REGEX.exec(sourceText.slice(0, frontmatterEnd));
  if (flags === null) return "script";

  return /\bmodule\b/.test(flags[1]) ? "module" : "script";
}
