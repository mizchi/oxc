// Source map generation.
//
// `write` records the output offset and original position of every mapped node as it goes,
// and this converts those offsets to generated line/column in one pass at the end.

import { debugAssert, typeAssertIs } from "../asserts.ts";

import type { Mapping, MutableMapping, SourceMapGenerator } from "./options.ts";
import type { State } from "../state.ts";

/**
 * Convert the offsets recorded during printing into mappings, and hand them to the generator.
 *
 * Printing records only an output offset per mapped node, which is why this walks the output once
 * at the end counting newlines, rather than tracking a line and column throughout.
 *
 * @param sourceMap - Generator the mappings are added to, which the caller was given in its options
 */
export function emitMappings(state: State, sourceMap: SourceMapGenerator): void {
  debugAssert(
    state.mapOffsets !== null && state.mapPositions !== null && state.mapNames !== null,
    "Source map arrays should exist when a `sourceMap` was given",
  );

  const { output, mapOffsets, mapPositions, mapNames } = state;
  const source = sourceMap.file || sourceMap._file;

  // The `generated` and `mapping` objects are reused across `addMapping` calls to avoid generating ephemeral objects
  const generated = { line: 1, column: 0 };
  const mapping: MutableMapping = { original: null, generated, name: undefined, source };

  let line = 1;
  let lineStart = 0;
  let nextLineStart = findNextLineStart(output, 0);

  const { length } = mapOffsets;
  for (let i = 0; i < length; i++) {
    const offset = mapOffsets[i];
    while (offset >= nextLineStart) {
      line++;
      lineStart = nextLineStart;
      nextLineStart = findNextLineStart(output, lineStart);
    }

    generated.line = line;
    generated.column = offset - lineStart;
    mapping.original = mapPositions[i];
    mapping.name = mapNames[i];

    typeAssertIs<Mapping>(mapping);
    sourceMap.addMapping(mapping);
  }
}

/** Find the UTF-16 offset after the next ECMAScript line terminator. */
function findNextLineStart(output: string, from: number): number {
  // Let the regexp engine scan long generated lines. A JS `charCodeAt` loop is much slower for
  // large literals, while `lastIndex` avoids allocating a substring just to start the search at
  // `from`.
  NEXT_LINE_TERMINATOR_REGEX.lastIndex = from;
  const match = NEXT_LINE_TERMINATOR_REGEX.exec(output);
  return match === null ? Infinity : match.index + match[0].length;
}

// `\r\n` must be one line terminator, rather than two.
const NEXT_LINE_TERMINATOR_REGEX = /\r\n|[\r\n\u2028\u2029]/g;
