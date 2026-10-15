// `import * as L from "@diaryx/fig/kit/literal"`: what a bare token means
// when nothing else says — the Zig kit's `literal.zig`, answering through
// its WebAssembly module (`zig.js`), so a language written here reads a
// bare value exactly as fig's own languages and its editor do.
//
// The rule is literal-else-string: `null`, `true` and `false` (lowercase
// only), a number, a datetime, and otherwise the text as a string. A
// number is decimal with an optional sign, fraction and exponent, or a
// `0x`/`0o`/`0b` integer, with `_` between digits; a multi-digit integer
// part led by a zero (`007`), `inf` and `nan` stay strings, as `Yes` and
// `TRUE` do.

import * as Z from "./zig.js";

/** The row kind bare `raw` is written as: "null", "true", "false", "int",
 *  "float", "string", or a datetime's `ext_kind` ("offset_datetime",
 *  "local_datetime", "local_date", "local_time"). "true" and "false" are a
 *  "bool" row whose text is that word. */
export function kind(raw) {
  return Z.literalKind(raw);
}

/** Whether `raw` is a number by the same rule: "int", "float", or null. */
export function number(raw) {
  return Z.literalNumber(raw);
}

export default { kind, number };
