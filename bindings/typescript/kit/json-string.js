// `import * as J from "@diaryx/fig/kit/json-string"`: a string written as a
// JSON string literal — the Zig kit's `json_string.zig`, answering through
// its WebAssembly module (`zig.js`), the escaper fig's JSON and canonical
// printers write with and YAML, TOML and plist borrow.

import * as Z from "./zig.js";

/** `s` between two `quote`s — `"` (the default), or the `'` JSON5 also
 *  takes, which escapes `'` and lets `"` through — escaped as JSON escapes
 *  it: `\\`, the quote, `\b \f \n \r \t`, and `\u00XX` for any other
 *  control character. Every other character, non-ASCII included, is
 *  written as it is. */
export function quote(s, q = '"') {
  return Z.jsonStringQuote(s, q);
}

export default { quote };
