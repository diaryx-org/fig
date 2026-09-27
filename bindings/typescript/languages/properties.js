// Java `.properties`, in JavaScript: the twin of fig's compiled
// `properties` format, row for row.
//
// The format — `java.util.Properties.load` as documented, not INI or
// dotenv re-skinned:
//
//   * a logical line is `key`, then the first unescaped `=`, `:` or run of
//     spaces and tabs, then the value to the line's end; `a=b`, `a:b`,
//     `a b` and `a = b` are the same statement; a line with nothing after
//     its separator, or no separator at all, is a key with an empty value
//     spanning nothing where a value would begin — after the separator,
//     or at the key's end;
//   * key and value both decode `\t \n \r \f`, `\uXXXX`, and `\` before any
//     other byte as that byte (`\:`, `\=`, `\#`, `\ `); a `\` before a line
//     ending is a continuation — it, the newline and the next line's
//     leading spaces and tabs vanish, and the token's span runs on;
//   * a full-line `#` or `!` is a comment, leading for the next key, or
//     dangling on the root when no key follows; nothing trails a value;
//   * a repeated key keeps the first entry's place and takes the last
//     value; the later key's own leading comments go with it.
//
// Every value is a string: the format has no typed scalars. The node table
// is held row for row against the compiled format — `fig lang table -i
// properties` prints that table, and `fig lang check js-properties
// --against properties <files…>` compares them file by file. Every
// document the compiled format refuses is refused here, in this module's
// own words and at its own offsets: the contract is the format, not the
// parser. Spans are byte offsets, 0-based, `[start, end)`; a rule reads
// the scanner's bytes, so `\uXXXX` decodes to UTF-8 and no offset is ever
// a UTF-16 index.
import * as fig from "@diaryx/fig/kit";
import * as G from "@diaryx/fig/kit/grammar";

// ── the grammar ───────────────────────────────────────────────────────────
// A form feed counts as inline whitespace, and so as a separator.

const hs = G.pat(/[ \t\f]*/);
const hs1 = G.pat(/[ \t\f]+/);

const isSeparator = (c) => c === 61 || c === 58 || c === 32 || c === 9 || c === 12;

// An escape-aware run: a `\` protects the byte after it, and a `\` before a
// line ending is a continuation — the run goes on into the next line, past
// its leading spaces and tabs. In `keyMode` it stops before the first
// unescaped separator, otherwise at the line's end. Always matches, and
// may be empty: a value is zero-width where one would begin.
function run(keyMode) {
  return (sc) => {
    const { bytes, n } = sc;
    const s = sc.pos;
    let i = s;
    while (i < n) {
      const c = bytes[i];
      if (c === 10 || c === 13) break;
      if (c === 92) {
        if (i + 1 >= n) fig.fail("a `\\` at the very end of the file has nothing to escape", i);
        if (bytes[i + 1] === 10) i += 2;
        else if (bytes[i + 1] === 13 && bytes[i + 2] === 10) i += 3;
        else {
          i += 2;
          continue;
        }
        while (i < n && (bytes[i] === 32 || bytes[i] === 9)) i += 1;
      } else if (keyMode && isSeparator(c)) {
        break;
      } else {
        i += 1;
      }
    }
    sc.pos = i;
    return [s, i];
  };
}

const SIMPLE = { t: "\t", n: "\n", r: "\r", f: "\f" };

// The text a run spells. Over the bytes, one char per byte, so a `\uXXXX`
// becomes the UTF-8 it stands for and a refusal lands on the escape itself.
function decode(sc, [s, e]) {
  const bin = sc.binSlice(s, e);
  let out = "";
  let i = 0;
  while (i < bin.length) {
    const c = bin[i];
    const next = bin[i + 1];
    if (c !== "\\") {
      out += c;
      i += 1;
    } else if (next === "\n" || (next === "\r" && bin[i + 2] === "\n")) {
      i += next === "\n" ? 2 : 3;
      while (bin[i] === " " || bin[i] === "\t") i += 1;
    } else if (next === "u") {
      const hex = bin.slice(i + 2, i + 6);
      const cp = /^[0-9A-Fa-f]{4}$/.test(hex) ? parseInt(hex, 16) : -1;
      if (cp < 0 || fig.isSurrogate(cp)) {
        fig.fail("invalid `\\u` escape; expected four hex digits spelling a character", s + i);
      }
      for (const b of fig.utf8Bytes(cp)) out += String.fromCharCode(b);
      i += 6;
    } else {
      out += SIMPLE[next] ?? next;
      i += 2;
    }
  }
  return fig.fromBin(out);
}

const text = (raw, span, sc) => (raw.includes("\\") ? decode(sc, span) : raw);

const entry = G.entry({
  key: G.key(run(true), { text }),
  between: hs,
  // Any run of inline whitespace separates; one `=` or `:` may join it.
  sep: G.opt(G.pat(/[=:]/)),
  value: G.scalar("string", run(false), { text }),
});

const parse = G.document({
  bom: true,
  root: G.map({
    whole: true,
    entry,
    trivia: G.trivia({
      space: G.choice([
        hs1,
        G.eol,
        G.failIf(G.lit("\r"), "a bare `\\r` must be followed by `\\n`; line endings must be `\\n` or `\\r\\n`"),
      ]),
      comment: G.choice([G.comment("#"), G.comment("!")]),
    }),
  }),
});

// ── the printer ───────────────────────────────────────────────────────────
// Canonical `.properties`, as the compiled printer writes it: one
// `key=value` line per entry, `=` whatever the source's separator was, the
// escapes that keep it reading back the same — control characters, `\`, a
// separator or leading `#`/`!` in a key, leading whitespace in a value —
// and nothing else; leading comments as `# …` lines above the key,
// dangling comments at the end. A comment that trails a value has no
// spelling and is not written.

const ESCAPED = { "\n": "\\n", "\r": "\\r", "\t": "\\t", "\f": "\\f", "\\": "\\\\" };

const escapedChar = (c) => ESCAPED[c] ?? "\\" + c;

function writeKey(w, name) {
  let idx = 0;
  for (const c of name) {
    if (idx === 0 && (c === "#" || c === "!")) w.put(escapedChar(c));
    else if (c in ESCAPED || c === "=" || c === ":" || c === " " || c === "\t" || c === "\f") w.put(escapedChar(c));
    else w.put(c);
    idx += 1;
  }
}

function writeValueText(w, v) {
  let idx = 0;
  for (const c of v) {
    if (idx === 0 && (c === " " || c === "\t" || c === "\f")) w.put(escapedChar(c));
    else if (c in ESCAPED) w.put(ESCAPED[c]);
    else w.put(c);
    idx += 1;
  }
}

function writeValue(w, row) {
  const k = row.kind;
  if (k === "string") writeValueText(w, row.text ?? "");
  else if (k === "int" || k === "float" || k === "bool") w.put(row.text ?? "");
  else if (k === "null") throw new Error("a .properties file has no null; a null value cannot be written");
  else if (k === "sequence" || k === "mapping")
    throw new Error("a .properties file holds a flat map of strings; a nested value cannot be written");
  else if (k === "alias") throw new Error("an alias must be resolved before it is written as .properties");
  else throw new Error("a " + k + " is not a value");
}

function print(_dialect, t, _options) {
  const w = fig.writer();
  const root = fig.index(t).byid(0);
  if (root.kind !== "mapping") {
    // A fragment: the scalar as it stands alone.
    writeValue(w, root);
    w.put("\n");
    return w.string();
  }
  for (const kv of root.items) {
    const { key, value } = kv;
    if (key.kind !== "string") throw new Error("a .properties key must be a string");
    w.comments(key.leading, "#");
    writeKey(w, key.text ?? "");
    w.put("=");
    writeValue(w, value);
    w.put("\n");
  }
  w.comments(root.dangling, "#");
  return w.string();
}

/** @type {import("@diaryx/fig/helper").Language} */
const language = {
  name: "properties",
  caps: { read: true, edit: true, serialize: true },
  // Flat: no mapping inside the root.
  max_mapping_depth: 0,
  syntax: {
    // `#` and `!` are read; `#` is written, and what the editor scans for.
    comments: { style: "hash", line: { open: "#" } },
    // `=`, `:` and a space are read; `=` is written.
    kv_sep: "=",
    empty_map_literal: "{}",
    flow_containers: false,
  },
  // The compiled format's extension: where the format is compiled out
  // and this stands in for it, the extension reaches it.
  dialects: [{ name: "properties", extensions: ["properties"], splice: "raw", empty_doc_seed: "" }],
  samples: ["a=1\nb: two\nc three\n"],
  renderers: [],
  parse,
  print,
};

export default language;
