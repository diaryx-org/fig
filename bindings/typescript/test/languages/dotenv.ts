// dotenv, in JavaScript: the twin of fig's compiled `dotenv` format, row for
// row. The test suite holds it to the tables the compiled parser gives
// (`fig lang table -i dotenv <file>`, kept in ../fixtures/dotenv) and, once
// registered, to the compiled format's reads, prints and edits. It is also
// what a new language is written against: a complete parser and printer,
// following src/languages/dotenv/ in the core line for line —
//
//   * a key is a bash identifier, `[A-Za-z_][A-Za-z0-9_]*`; an `export `
//     before one is recognized and discarded;
//   * a value is unquoted (trimmed, to end of line, or to a `#` that at
//     least one space or tab precedes), double-quoted (`\n \t \r \\ \"`
//     decoded, may span lines) or single-quoted (raw, may span lines);
//     a `\r\n` inside a quoted value reads as `\n`;
//   * a full-line `#` is a comment, leading for the next key, or dangling
//     on the root when no key follows; a `#` after a value on its line is
//     the value's trailing comment; a `#` right after a closing quote
//     needs no space before it;
//   * a repeated key keeps the FIRST entry's place and takes the LAST
//     value; the later key's own leading comments go with it.
//
// Every value is a string: dotenv has no typed scalars and does no `$VAR`
// interpolation. Spans are BYTE offsets, 0-based, `[start, end)` — the wire
// is UTF-8, JavaScript strings are not — so the parser runs over the input's
// bytes and decodes each scalar's text.
import { LanguageError, type CommentRow, type Language, type NodeRow, type NodeTable, type PrintOptions } from "../../src/index.ts";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

// The compiled parser's own words, so a refusal reads the same.
const MESSAGES = {
  UnexpectedToken: "unexpected content here; expected `KEY=value` (optionally `export KEY=value`)",
  MissingEquals: "expected `=` after this key; every dotenv line is `KEY=value`",
  BadEscape: 'invalid escape in a double-quoted value; supported: \\n \\t \\r \\\\ \\" — use a single-quoted value for raw text with backslashes',
  UnexpectedCarriageReturn: "a bare `\\r` must be followed by `\\n`; line endings must be `\\n` or `\\r\\n`",
  UnclosedString: "unclosed quoted value; expected a matching `\"`/`'` before the end of the file",
  UnexpectedChar: "not a valid key here; a dotenv key is a bash identifier (`[A-Za-z_][A-Za-z0-9_]*`)",
  TrailingContent: "unexpected content after this quoted value; only a `#` comment may follow it on the same line",
};

const SP = 0x20, TAB = 0x09, NL = 0x0a, CR = 0x0d, HASH = 0x23, EQ = 0x3d, DQ = 0x22, SQ = 0x27, BS = 0x5c;

function isIdentStart(c: number): boolean {
  return c === 0x5f || (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a);
}
function isIdentChar(c: number): boolean {
  return isIdentStart(c) || (c >= 0x30 && c <= 0x39);
}

// ── the parser ────────────────────────────────────────────────────────────

function parse(_dialect: string, input: string): NodeTable {
  const src = encoder.encode(input);
  const text = (s: number, e: number) => decoder.decode(src.subarray(s, e));
  const rows: NodeRow[] = [{ kind: "mapping", parent: null, span: [0, src.length] }];
  const comments: CommentRow[] = [];
  // Entries by key: the first entry's rows stay; a repeat replaces the value.
  const entries = new Map<string, { kv: number; value: number }>();
  let pendingLeading: string[] = [];
  let lastValue: number | null = null;

  let i = src.byteLength >= 3 && src[0] === 0xef && src[1] === 0xbb && src[2] === 0xbf ? 3 : 0;
  const atLineEnd = (at: number) => at >= src.length || src[at] === NL || src[at] === CR;
  const fail = (message: string, at: number): never => {
    throw new LanguageError(message, at);
  };
  const skipHs = () => {
    while (i < src.length && (src[i] === SP || src[i] === TAB)) i++;
  };
  // A comment's content, leader excluded, trimmed as the compiled parser
  // trims it; binds trailing to the value just parsed on this line, else
  // waits for the next key.
  const comment = () => {
    i++; // '#'
    const start = i;
    while (!atLineEnd(i)) i++;
    const body = text(start, i).replace(/^[ \t\r]+|[ \t\r]+$/g, "");
    if (lastValue !== null) {
      comments.push({ node: lastValue, slot: "trailing", style: "line", text: body });
      lastValue = null;
    } else {
      pendingLeading.push(body);
    }
  };
  const newline = () => {
    if (src[i] === CR) {
      if (src[i + 1] !== NL) fail(MESSAGES.UnexpectedCarriageReturn, i);
      i += 2;
    } else i++;
    lastValue = null;
  };
  const quoted = (q: number): [number, number, string] => {
    const start = i;
    i++;
    const parts: number[] = [];
    while (i < src.length) {
      const c = src[i]!;
      if (c === CR) {
        if (src[i + 1] !== NL) fail(MESSAGES.UnexpectedCarriageReturn, i);
        parts.push(NL);
        i += 2;
        continue;
      }
      if (q === DQ && c === BS) {
        const e = src[i + 1];
        if (e === undefined) fail(MESSAGES.UnclosedString, start);
        const decoded = e === 0x6e ? NL : e === 0x74 ? TAB : e === 0x72 ? CR : e === BS ? BS : e === DQ ? DQ : null;
        if (decoded === null) fail(MESSAGES.BadEscape, i);
        parts.push(decoded!);
        i += 2;
        continue;
      }
      if (c === q) {
        i++;
        return [start, i, decoder.decode(Uint8Array.from(parts))];
      }
      parts.push(c);
      i++;
    }
    return fail(MESSAGES.UnclosedString, start);
  };

  while (i < src.length) {
    const c = src[i]!;
    if (c === NL || c === CR) {
      newline();
      continue;
    }
    if (c === SP || c === TAB) {
      i++;
      continue;
    }
    if (c === HASH) {
      comment();
      continue;
    }
    if (c === EQ) fail(MESSAGES.UnexpectedToken, i);
    if (!isIdentStart(c)) fail(MESSAGES.UnexpectedChar, i);

    // key, with an `export ` prefix discarded when a key follows it
    let keyStart = i;
    while (i < src.length && isIdentChar(src[i]!)) i++;
    let keyEnd = i;
    if (text(keyStart, keyEnd) === "export") {
      const save = i;
      skipHs();
      if (i > save && i < src.length && isIdentStart(src[i]!)) {
        keyStart = i;
        while (i < src.length && isIdentChar(src[i]!)) i++;
        keyEnd = i;
      } else i = save;
    }
    skipHs();
    if (src[i] !== EQ) fail(MESSAGES.MissingEquals, keyStart);
    i++;
    skipHs();

    // value: quoted, or unquoted to the line's end or a `#` that whitespace
    // precedes (the one right after `=` counts, so `A= #c` is empty)
    let vStart: number, vEnd: number, value: string;
    if (src[i] === DQ || src[i] === SQ) {
      [vStart, vEnd, value] = quoted(src[i]!);
      skipHs();
      if (src[i] === HASH) {
        // bound below, once the value row exists
      } else if (!atLineEnd(i)) fail(MESSAGES.TrailingContent, i);
    } else {
      vStart = i;
      let end = i;
      let sawSpace = i > 0 && (src[i - 1] === SP || src[i - 1] === TAB);
      while (i < src.length) {
        const b = src[i]!;
        if (b === NL || b === CR) break;
        if (b === HASH && sawSpace) break;
        sawSpace = b === SP || b === TAB;
        i++;
        end = i;
      }
      while (end > vStart && (src[end - 1] === SP || src[end - 1] === TAB)) end--;
      vEnd = end;
      value = text(vStart, vEnd);
    }

    const key = text(keyStart, keyEnd);
    const existing = entries.get(key);
    if (existing) {
      // The first entry keeps its place and its key; the value is the last
      // one written, and the replaced value's trailing comment goes with
      // the node it was bound to. Comments waiting for this key go with it:
      // nowhere.
      pendingLeading = [];
      rows[existing.value] = { kind: "string", parent: existing.kv, span: [vStart, vEnd], text: value };
      for (let k = comments.length - 1; k >= 0; k--) {
        if (comments[k]!.node === existing.value && comments[k]!.slot === "trailing") comments.splice(k, 1);
      }
      lastValue = existing.value;
    } else {
      const kv = rows.length;
      rows.push({ kind: "keyvalue", parent: 0, span: [keyStart, vEnd] });
      const keyRow = rows.length;
      rows.push({ kind: "string", parent: kv, span: [keyStart, keyEnd], text: key });
      for (const c of pendingLeading) comments.push({ node: keyRow, slot: "leading", style: "line", text: c });
      pendingLeading = [];
      const valueRow = rows.length;
      rows.push({ kind: "string", parent: kv, span: [vStart, vEnd], text: value });
      entries.set(key, { kv, value: valueRow });
      lastValue = valueRow;
    }
    if (src[i] === HASH) comment();
  }
  for (const c of pendingLeading) comments.push({ node: 0, slot: "dangling", style: "line", text: c });

  // As `fig lang table` orders them: by node, then leading, trailing,
  // dangling, in source order within a slot.
  const SLOT = { leading: 0, trailing: 1, dangling: 2 };
  comments.sort((a, b) => a.node - b.node || SLOT[a.slot] - SLOT[b.slot]);
  return { rows, comments };
}

// ── the printer ───────────────────────────────────────────────────────────
// Canonical `.env`, as the compiled printer writes it: one `KEY=value` line
// per entry, a value bare when that is unambiguous and double-quoted with
// escapes otherwise, leading comments as `# …` lines above the key, a
// trailing comment inline after the value, dangling comments at the end.

function needsQuoting(v: string): boolean {
  if (v === "") return false;
  if (/^[ \t]|[ \t]$/.test(v)) return true;
  return /[\n\r"\\#]/.test(v);
}

function writeText(v: string): string {
  if (!needsQuoting(v)) return v;
  return '"' + v.replace(/[\n\r\t"\\]/g, (c) => ({ "\n": "\\n", "\r": "\\r", "\t": "\\t", '"': '\\"', "\\": "\\\\" })[c]!) + '"';
}

function writeValue(row: NodeRow): string {
  switch (row.kind) {
    case "string":
      return writeText(row.text ?? "");
    case "int":
    case "float":
    case "bool":
      return row.text ?? "";
    case "null":
      throw new LanguageError("dotenv has no null; a null value cannot be written");
    case "sequence":
    case "mapping":
      throw new LanguageError("dotenv holds a flat map of strings; a nested value cannot be written");
    case "alias":
      throw new LanguageError("an alias must be resolved before it is written as dotenv");
    default:
      throw new LanguageError(`a ${row.kind} is not a value`);
  }
}

function commentLines(text: string): string {
  return text
    .split("\n")
    .map((line) => {
      const t = line.replace(/^[ \t]+|[ \t]+$/g, "");
      return t === "" ? "#\n" : `# ${t}\n`;
    })
    .join("");
}

function print(_dialect: string, t: NodeTable, _options: PrintOptions): string {
  const rows = t.rows;
  const root = rows[0]!;
  if (root.kind !== "mapping") return writeValue(root);
  const by = (node: number, slot: CommentRow["slot"]) => (t.comments ?? []).filter((c) => c.node === node && c.slot === slot);
  let out = "";
  for (let kv = 0; kv < rows.length; kv++) {
    if (rows[kv]!.parent !== 0 || rows[kv]!.kind !== "keyvalue") continue;
    const children: number[] = [];
    for (let j = kv + 1; j < rows.length && children.length < 2; j++) if (rows[j]!.parent === kv) children.push(j);
    const [keyRow, valueRow] = children as [number, number];
    const key = rows[keyRow]!;
    const value = rows[valueRow]!;
    if (key.kind !== "string") throw new LanguageError("a dotenv key must be a string");
    const name = key.text ?? "";
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) throw new LanguageError(`\`${name}\` is not a dotenv key; a key is a bash identifier`);
    for (const c of by(keyRow, "leading")) out += commentLines(c.text);
    out += `${name}=${writeValue(value)}`;
    const trailing = by(valueRow, "trailing")[0];
    if (trailing) out += " #" + (trailing.text === "" ? "" : " " + trailing.text.replace(/\n/g, " "));
    out += "\n";
  }
  for (const c of by(0, "dangling")) out += commentLines(c.text);
  return out;
}

/** The language: `js-dotenv`, so as not to collide with the compiled
 *  `dotenv`, whose `.env` extension also wins. */
export const dotenv: Language = {
  name: "js-dotenv",
  caps: { read: true, edit: true, serialize: true },
  // Flat: no mapping inside the root.
  max_mapping_depth: 0,
  syntax: {
    comments: { style: "hash", line: { open: "#" }, trailing: { open: "#" } },
    kv_sep: "=",
    empty_map_literal: "{}",
    flow_containers: false,
  },
  dialects: [{ name: "js-dotenv", extensions: ["env"], splice: "raw", empty_doc_seed: "" }],
  samples: ['A=1\nB="two words"\n', "# top\nexport C='raw \\n'\nD=x # trailing\n"],
  parse,
  print,
};
