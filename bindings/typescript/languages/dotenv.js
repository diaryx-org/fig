// dotenv, in JavaScript: the twin of fig's compiled `dotenv` format, row
// for row.
//
// The format:
//
//   * a key is a bash identifier, `[A-Za-z_][A-Za-z0-9_]*`; an `export `
//     before one is recognized and discarded;
//   * a value is unquoted (trimmed, to end of line, or to a `#` that at
//     least one space or tab precedes), double-quoted (`\n \t \r \\ \"`
//     decoded, may span lines) or single-quoted (raw, may span lines); a
//     `\r\n` inside a quoted value reads as `\n`;
//   * a full-line `#` is a comment, leading for the next key, or dangling
//     on the root when no key follows; a `#` after a value on its line is
//     the value's trailing comment; a `#` right after a closing quote needs
//     no space before it;
//   * a repeated key keeps the FIRST entry's place and takes the LAST
//     value; the later key's own leading comments go with it.
//
// Every value is a string: dotenv has no typed scalars and does no `$VAR`
// interpolation. The node table is held row for row against the compiled
// format — `fig lang table -i dotenv` prints that table, and `fig lang
// check js-dotenv --against dotenv <files…>` compares them file by file.
// Every document the compiled format refuses is refused here, in this
// module's own words and at its own offsets: the contract is the format,
// not the parser. Spans are byte offsets, 0-based, `[start, end)`.
import * as fig from "@diaryx/fig/kit";
import * as G from "@diaryx/fig/kit/grammar";

// ── the grammar ───────────────────────────────────────────────────────────

const BARE_CR = "a bare `\\r` must be followed by `\\n`; line endings must be `\\n` or `\\r\\n`";
const UNCLOSED = 'unclosed quoted value; expected a matching `"`/`\'` before the end of the file';

// Only spaces, a `#` comment or the line's end may follow a closing quote.
function afterQuoted(sc) {
  const save = sc.pos;
  sc.hs();
  if (!(sc.eof() || sc.starts("#") || sc.starts("\n") || sc.starts("\r"))) {
    sc.fail("unexpected content after this quoted value; only a `#` comment may follow it on the same line");
  }
  sc.pos = save;
}

const ESCAPES = { n: "\n", t: "\t", r: "\r", "\\": "\\", '"': '"' };

const value = G.choice([
  G.quoted({
    open: '"',
    escapes: ESCAPES,
    unclosed: UNCLOSED,
    badEscape:
      'invalid escape in a double-quoted value; supported: \\n \\t \\r \\\\ \\" — use a single-quoted value for raw text with backslashes',
    bareCr: BARE_CR,
    after: afterQuoted,
  }),
  G.quoted({ open: "'", unclosed: UNCLOSED, bareCr: BARE_CR, after: afterQuoted }),
  // Unquoted: to the line's end, or to a `#` that a space or tab precedes —
  // the one right after `=` counts, since `A= #c` is empty.
  G.bare({
    stop: (sc) => {
      const before = sc.byte(-1);
      return sc.starts("#") && (before === 32 || before === 9);
    },
  }),
]);

const entry = G.entry({
  // `export` is a prefix only when a key follows it on the line; alone
  // before `=` it is the key.
  prefix: G.seq([G.lit("export"), G.hs1, G.ahead(G.pat(/[A-Za-z_]/))]),
  key: G.key(G.pat(/[A-Za-z_][A-Za-z0-9_]*/)),
  sep: G.lit("="),
  value,
  missingSep: "expected `=` after this key; every dotenv line is `KEY=value`",
});

const parse = G.document({
  bom: true,
  root: G.map({
    whole: true,
    entry,
    trivia: G.trivia({
      space: G.choice([G.hs1, G.eol, G.failIf(G.lit("\r"), BARE_CR)]),
      comment: G.comment("#"),
    }),
    // Where a key should be.
    otherwise: (sc) => {
      if (sc.starts("=")) sc.fail("unexpected content here; expected `KEY=value` (optionally `export KEY=value`)");
      sc.fail("not a valid key here; a dotenv key is a bash identifier (`[A-Za-z_][A-Za-z0-9_]*`)");
    },
  }),
});

// ── the printer ───────────────────────────────────────────────────────────
// Canonical `.env`, as the compiled printer writes it: one `KEY=value` line
// per entry, a value bare when that is unambiguous and double-quoted with
// escapes otherwise (never a literal embedded newline), leading comments as
// `# …` lines above the key, a trailing comment inline after the value,
// dangling comments at the end.

const isIdentifier = (name) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(name);

function needsQuoting(v) {
  if (v === "") return false;
  const first = v[0];
  const last = v[v.length - 1];
  if (first === " " || first === "\t" || last === " " || last === "\t") return true;
  return /[\n\r"\\#]/.test(v);
}

const QUOTE = { "\n": "\\n", "\r": "\\r", "\t": "\\t", '"': '\\"', "\\": "\\\\" };

function writeValue(w, row) {
  const k = row.kind;
  if (k === "string") {
    const v = row.text ?? "";
    if (needsQuoting(v)) w.put('"', v.replace(/[\n\r\t"\\]/g, (ch) => QUOTE[ch]), '"');
    else w.put(v);
  } else if (k === "int" || k === "float" || k === "bool") {
    w.put(row.text ?? "");
  } else if (k === "null") {
    throw new Error("dotenv has no null; a null value cannot be written");
  } else if (k === "sequence" || k === "mapping") {
    throw new Error("dotenv holds a flat map of strings; a nested value cannot be written");
  } else if (k === "alias") {
    throw new Error("an alias must be resolved before it is written as dotenv");
  } else {
    throw new Error("a " + k + " is not a value");
  }
}

function print(_dialect, t, _options) {
  fig.index(t);
  const w = fig.writer();
  const root = t.byid(0);
  if (root.kind !== "mapping") {
    // A fragment: the scalar as it stands alone.
    writeValue(w, root);
    return w.string();
  }
  for (const kv of root.items) {
    const { key, value } = kv;
    if (key.kind !== "string") throw new Error("a dotenv key must be a string");
    if (!isIdentifier(key.text ?? "")) {
      throw new Error("`" + (key.text ?? "") + "` is not a dotenv key; a key is a bash identifier");
    }
    w.comments(key.leading, "#");
    w.put(key.text, "=");
    writeValue(w, value);
    const trailing = value.trailing[0];
    if (trailing) {
      w.put(" #");
      if (trailing.text !== "") w.put(" ", trailing.text.replace(/\n/g, " "));
    }
    w.put("\n");
  }
  w.comments(root.dangling, "#");
  return w.string();
}

// ── the language ──────────────────────────────────────────────────────────
// The declarations are the wire's `description`, field for field — the
// same object `@diaryx/fig`'s `registerLanguage` takes.

/** @type {import("@diaryx/fig/helper").Language} */
const language = {
  name: "dotenv",
  caps: { read: true, edit: true, serialize: true },
  // Flat: no mapping inside the root.
  max_mapping_depth: 0,
  syntax: {
    comments: { style: "hash", line: { open: "#" }, trailing: { open: "#" } },
    kv_sep: "=",
    empty_map_literal: "{}",
    flow_containers: false,
  },
  dialects: [
    // `env`, the extension: the compiled format owns `.env` too, and a
    // compiled format's extension wins, so this is reached by `--lang`.
    { name: "dotenv", extensions: ["env"], splice: "raw", empty_doc_seed: "" },
  ],
  samples: ["A=1\nB=\"two words\"\n", "# top\nexport C='raw \\n'\nD=x # trailing\n"],
  renderers: [],
  parse,
  print,
};

export default language;
