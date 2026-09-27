// INI, in JavaScript: the twin of fig's compiled `ini` format, row for
// row, region for region, mention for mention.
//
// The format:
//
//   * a line is a `[section]` header, a `key = value`, a `;` or `#`
//     comment, or blank; `=` is the only separator; a value is the rest of
//     its line, trimmed, with one layer of matching `"…"`/`'…'` quotes
//     removed and nothing decoded — a `;` after a value is value text, not
//     a comment;
//   * the root is a mapping spanning the whole input; a section is a
//     keyvalue and a mapping, both spanning the name token of the header
//     that created it; a `key = value` is a keyvalue from the key's start
//     to the value's end, its key and value strings spanning their trimmed
//     text (an absent value spans nothing at the line's end);
//   * a repeated key keeps the first entry's place and takes the last
//     value; a repeated `[section]` reopens the section, which then holds
//     every header line as a *region* and every header's name as a
//     *mention*; a section named like a root key is refused;
//   * a comment leads the next key or section, and at the end of the file
//     dangles on the section the last header opened.
//
// One level deep by construction (`max_mapping_depth = 1`): there is no
// spelling for a section inside a section. The node table is held row for
// row against the compiled format — `fig lang table -i ini` prints that
// table, and `fig lang check js-ini --against ini <files…>` compares them
// file by file. Every document the compiled format refuses is refused
// here, in this module's own words and at its own offsets: the contract is
// the format, not the parser. Spans are byte offsets, 0-based,
// `[start, end)`. The same object `@diaryx/fig`'s `registerLanguage`
// takes, so it serves the browser and Node unchanged.
import * as fig from "@diaryx/fig/kit";
import * as G from "@diaryx/fig/kit/grammar";

// ── the grammar ───────────────────────────────────────────────────────────

// Blank lines and whole-line `;`/`#` comments. A comment waits in the
// section context for the key or header it leads; nothing trails a value,
// since a `;` after one is value text.
const trivia = G.trivia({
  space: G.choice([
    G.hs1,
    G.eol,
    G.failIf(G.lit("\r"), "a bare `\\r` must be followed by `\\n`; line endings must be `\\n` or `\\r\\n`"),
  ]),
  comment: G.choice([G.comment(";"), G.comment("#")]),
});

// `[name]`, the name trimmed of spaces and tabs.
const header = G.seq([
  G.lit("["),
  G.expect(G.key(G.bare({ stop: (sc) => sc.starts("]"), empty: false })), "a `[section]` header needs a name"),
  G.expect(G.lit("]"), "unclosed `[section]` header; expected a `]` before the end of the line"),
]);

// One layer of matching quotes around the whole value comes off; nothing
// inside is decoded.
function unquote(raw) {
  const q = raw[0];
  if (raw.length >= 2 && (q === '"' || q === "'") && raw.endsWith(q)) return raw.slice(1, -1);
  return raw;
}

const entry = G.entry({
  key: G.key(G.bare({ stop: (sc) => sc.starts("="), empty: false })),
  sep: G.lit("="),
  value: G.bare({ text: unquote }),
  missingSep: "expected `=` after this key; every INI line is `key = value`",
});

// A header line: the section its name already stands for, re-entered, or a
// fresh one under the root. Nothing but spaces may follow the `]`.
function section(sc, S, root, key) {
  sc.hs();
  if (!(sc.eof() || sc.starts("\n") || sc.starts("\r"))) {
    sc.fail("unexpected content after `]`; a section header must be alone on its line");
  }
  const existing = root.byKey.get(key.text);
  if (existing) {
    if (existing.value.kind !== "mapping") {
      fig.fail("this section conflicts with a key of the same name already defined at this level", key.span[0]);
    }
    S.reopen(existing.value, key.span, "header");
    return existing.value;
  }
  S.claim(key, "leading");
  return S.open(root, fig.entry(key, fig.mapping(key.span)), "header");
}

// Line by line: what a header *means* — which section it re-enters, where
// the keys after it land — is the parser's, as `G.sections` says.
function parse(_dialect, input) {
  const sc = fig.scanner(input);
  if (sc.bin.startsWith("\xef\xbb\xbf")) sc.pos = 3;
  const S = G.sections(sc.bin);
  const ctx = { comment: (c) => S.comment(c.text) };
  const root = fig.mapping([0, sc.n]);
  let current = root;
  for (;;) {
    trivia(sc, ctx);
    if (sc.eof()) break;
    const h = header(sc);
    if (h != null) {
      current = section(sc, S, root, h[1]);
      continue;
    }
    const e = entry(sc);
    if (e == null) sc.fail("unexpected content here; expected a `[section]` header or a `key = value` line");
    S.claim(e.key, "leading");
    current.put(e);
  }
  S.claim(current, "dangling");
  return fig.rows(root);
}

// ── the printer ───────────────────────────────────────────────────────────
// The compiled printer's layout: the root's scalar entries first as
// `key = value` lines, then every mapping-valued entry as a `[section]`
// block, in that order whatever the tree's — INI cannot interleave them.
// A value is bare unless bare would not read back the same (empty, padded,
// or already quoted), then `"…"`. Comments are `;` lines: leading above a
// key, a trailing one on its own line after, dangling at the end of its
// section.

function needsQuoting(v) {
  if (v === "") return true;
  const first = v[0];
  const last = v[v.length - 1];
  if (first === " " || first === "\t" || last === " " || last === "\t") return true;
  return v.length >= 2 && (first === '"' || first === "'") && last === first;
}

function writeText(w, v) {
  if (/[\n\r]/.test(v)) throw new Error("a value with a line break has no INI spelling");
  if (needsQuoting(v)) w.put('"', v, '"');
  else w.put(v);
}

function writeValue(w, row) {
  const k = row.kind;
  if (k === "string") writeText(w, row.text ?? "");
  else if (k === "int" || k === "float" || k === "bool") w.put(row.text);
  else if (k === "null") throw new Error("INI has no null; a null value cannot be written");
  else if (k === "sequence" || k === "mapping") throw new Error("a nested container has no INI spelling");
  else if (k === "alias") throw new Error("an alias must be resolved before it is written as INI");
  else throw new Error("a " + k + " is not a value");
}

function keyText(row) {
  if (row.kind !== "string") throw new Error("an INI key must be a string");
  return row.text ?? "";
}

function print(_dialect, t, options) {
  fig.index(t);
  const w = fig.writer();
  const root = t.byid(0);
  // Splice text is what follows `key = ` on one line, where a mapping has
  // no spelling — a section is a header of its own — so it is refused
  // rather than printed as the document (`c = 1`) that would read back as
  // a string there: the compiled `printSplice`.
  if (options?.splice === true && root.kind === "mapping") throw new Error("a section cannot be written as a value");
  if (root.kind !== "mapping") {
    writeValue(w, root);
    w.put("\n");
    return w.string();
  }
  // Whether anything stands above what comes next: a blank line goes
  // between the root's keys and the first section, and between sections.
  let wrote = false;
  const comments = (list) => {
    wrote = w.comments(list, ";") || wrote;
  };
  const kvLine = (e) => {
    comments(e.key.leading);
    writeText(w, keyText(e.key));
    w.put(" = ");
    writeValue(w, e.value);
    w.put("\n");
    wrote = true;
    comments(e.value.trailing);
  };
  for (const e of root.items) if (e.value.kind !== "mapping") kvLine(e);
  for (const e of root.items) {
    if (e.value.kind !== "mapping") continue;
    comments(e.key.leading);
    if (wrote) w.put("\n");
    w.put("[");
    writeText(w, keyText(e.key));
    w.put("]\n");
    wrote = true;
    for (const inner of e.value.items) {
      if (inner.value.kind === "mapping") throw new Error("a section inside a section has no INI spelling");
      kvLine(inner);
    }
    comments(e.value.dangling);
  }
  comments(root.dangling);
  return w.string();
}

// ── the language ──────────────────────────────────────────────────────────

/** @type {import("@diaryx/fig/helper").Language} */
const language = {
  name: "ini",
  caps: { read: true, edit: true, serialize: true },
  max_mapping_depth: 1,
  syntax: {
    // `#` is read; `;` is what is written, and what the editor writes. No
    // same-line trailing comment: a `;` after a value is value text.
    comments: { style: "semicolon", line: { open: ";" } },
    kv_sep: " = ",
    // `{}` in INI is the two-character string `{}`, so nothing can be
    // auto-vivified; and a `[` opening the file is a header, not a flow
    // container.
    flow_containers: false,
    section_noun: "section",
  },
  // The compiled format's extension: where the format is compiled out
  // and this stands in for it, the extension reaches it.
  dialects: [{ name: "ini", extensions: ["ini"], splice: "raw", empty_doc_seed: "" }],
  samples: ["a = 1\n\n[s]\nk = v\n"],
  renderers: [],
  parse,
  print,
};

export default language;
