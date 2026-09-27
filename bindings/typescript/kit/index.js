// `import * as fig from "@diaryx/fig/kit"`: the kit a format written in
// JavaScript is built from, with `@diaryx/fig/kit/grammar`, `/xml`,
// `/number` and `/datetime` — the modules built beside it. The formats in
// `@diaryx/fig/languages/*` are written with it, and fig-quickjs serves
// the same files to a language it runs.
//
// Three layers, each written in terms of the one below:
//
//   * the **node table** — `table()`, `t.row(...)`, the wire's shape, which
//     a `parse` returns and a `print` receives, and `index(t)` makes
//     navigable;
//   * the **tree** — `mapping`, `sequence`, `entry`, `scalar`, nodes that
//     hold their children and their comments as fields, which `rows` lays
//     out in pre-order; and `scanner`, a byte cursor whose positions are
//     already the wire's;
//   * the **grammar** — `import * as G from "@diaryx/fig/kit/grammar"`, combinators
//     whose primitives are these nodes, so a format is a description and
//     the tree, the spans, the comment binding and the duplicate policy
//     fall out of it.
//
// Offsets are byte offsets into the input, 0-based, `[start, end)` — the
// wire's, and what `fig lang table` prints. JavaScript's strings are
// UTF-16, so the scanner does not index the string it was given: it
// encodes the input once and works over the bytes, with a one-char-per-byte
// shadow of them for regular expressions, and only decodes what it slices.
// A language that stays above the scanner never meets a UTF-16 index. Row
// ids are 0-based too: the root is row 0.
//
// This module runs unchanged under Node and in the browser — it imports
// only `@diaryx/fig/helper`, so a language written against it is the same
// object `registerLanguage` takes there.
import { LanguageError } from "@diaryx/fig/helper";

const encoder = new TextEncoder();
// `ignoreBOM`: a slice is text of the input, and a byte-order mark at its
// start is text too — a WHATWG decoder would otherwise drop it, and the
// slice would no longer be the bytes its span says.
const decoder = new TextDecoder("utf-8", { ignoreBOM: true });

// ── failure ───────────────────────────────────────────────────────────────

/** Refuse the input: throws, from anywhere inside a `parse`, and fig
 *  reports `message` at byte offset `at` (undefined for "no position").
 *  Any other error a `parse` throws is reported with its message and no
 *  offset. */
export function fail(message, at) {
  throw new LanguageError(message, at);
}

/** Whether `e`, a caught error, is what `fail` throws. */
export function isFailure(e) {
  return e instanceof LanguageError;
}

// ── the node table ────────────────────────────────────────────────────────

export class Table {
  constructor() {
    this.rows = [];
    this.comments = [];
    this.regions = [];
    this.mentions = [];
    this.directives = [];
  }

  /** Add one row and return its id. `kind` is one of "null", "bool",
   *  "int", "float", "string", "sequence", "mapping", "keyvalue", "alias";
   *  `parent` is a row id or null for the root; `span` is `[start, end]`.
   *  `extra` may carry `text`, `sep`, `marker`, `anchor`, `anchor_span`,
   *  `tag`, `tag_span` and `ext_kind`; an undefined one is left out. */
  row(kind, parent, span, extra) {
    const id = this.rows.length;
    const row = { kind, parent: parent ?? null, span };
    if (span === undefined) delete row.span;
    if (extra) {
      for (const k of Object.keys(extra)) {
        if (extra[k] !== undefined && extra[k] !== null) row[k] = extra[k];
      }
    }
    this.rows.push(row);
    return id;
  }

  /** Bind a comment to row `node`. `slot` is "leading", "trailing" or
   *  "dangling"; `style` is "line" or "block"; `text` is the comment's
   *  text with its delimiters removed. */
  comment(node, slot, style, text) {
    this.comments.push({ node, slot, style, text });
  }

  /** Record one header line `[start, end)` belonging to section row `node`. */
  region(node, start, end) {
    this.regions.push({ node, start, end });
  }

  /** Record one place section row `node`'s name is written; `kind` is
   *  "header" or "entry". */
  mention(node, span, kind) {
    this.mentions.push({ node, span, kind });
  }

  /** Record one tag-handle declaration of the document (a YAML `%TAG`):
   *  `handle` is `!e!`, or a redefined `!`/`!!`, and `prefix` what it
   *  expands to. A print of a whole document receives them back, in
   *  this order, to re-emit above any tag that uses one. */
  directive(handle, prefix) {
    this.directives.push({ handle, prefix });
  }
}

/** A new, empty node table. Rows are added in pre-order: a container before
 *  its children, a keyvalue before its key and then its value. */
export function table() {
  return new Table();
}

/** Whether `kind` names a scalar row. */
export function isScalar(kind) {
  return kind !== "sequence" && kind !== "mapping" && kind !== "keyvalue" && kind !== "alias";
}

/** Index a table a `print` receives — the first thing a `print` does:
 *  every row gains `id`, `children` (its child ids, in order), `items`
 *  (the same children as rows), and `leading`, `trailing`, `dangling` (its
 *  comments, each `{style, text}`); a keyvalue row gains `key` and
 *  `value`, its two children as rows; the table gains `byid(id)`. Returns
 *  the table. */
export function index(t) {
  t.rows ??= [];
  t.comments ??= [];
  t.regions ??= [];
  t.mentions ??= [];
  t.directives ??= [];
  t.rows.forEach((row, i) => {
    row.id = i;
    row.children = [];
    row.items = [];
    row.leading = [];
    row.trailing = [];
    row.dangling = [];
  });
  for (const row of t.rows) {
    if (row.parent !== null && row.parent !== undefined) {
      const p = t.rows[row.parent];
      p.children.push(row.id);
      p.items.push(row);
    }
  }
  for (const row of t.rows) {
    if (row.kind === "keyvalue") {
      row.key = row.items[0];
      row.value = row.items[1];
    }
  }
  for (const c of t.comments) {
    t.rows[c.node][c.slot].push({ style: c.style, text: c.text });
  }
  t.byid = (id) => t.rows[id];
  return t;
}

/** The child ids of row `id` in an indexed table. */
export function children(t, id) {
  return t.rows[id].children;
}

// ── the tree ──────────────────────────────────────────────────────────────
// A node has `kind` and `span`, and for a scalar `text`; a mapping holds
// `entries` (keyvalue nodes), a sequence `items`, a keyvalue its `key` and
// `value`. Comments are `leading`, `trailing` and `dangling` lists of
// `{style, text}`. `rows` walks one into a node table.

export class Node {
  constructor(kind, span, extra) {
    this.kind = kind;
    this.span = span;
    if (extra) Object.assign(this, extra);
  }

  /** Bind a comment to this node: `slot` is "leading", "trailing" or
   *  "dangling", `style` "line" (the default) or "block". */
  comment(slot, text, style) {
    (this[slot] ??= []).push({ style: style ?? "line", text });
    return this;
  }

  /** Put an entry in a mapping by its duplicate policy; returns the entry
   *  that holds the value afterwards. A key with no text — a null, a
   *  container, in a format whose keys are nodes — repeats nothing. */
  put(entry) {
    const name = entry.key.text;
    const existing = name !== undefined && name !== null ? this.byKey.get(name) : undefined;
    const policy = this.duplicates;
    if (existing && policy !== "keep") {
      if (policy === "last_value") {
        // The first entry keeps its place, its key and its span; the later
        // key, and the comments on it, are unreachable.
        existing.value = entry.value;
        return existing;
      }
      if (policy === "first_value") return existing;
      if (policy === "error") fail("duplicate key `" + name + "`", entry.key.span[0]);
      throw new Error("unknown duplicate policy `" + String(policy) + "`");
    }
    this.entries.push(entry);
    if (name !== undefined && name !== null && !existing) this.byKey.set(name, entry);
    return entry;
  }

  /** Append an item to a sequence. */
  add(item) {
    this.items.push(item);
    return item;
  }
}

/** Any node; the constructors below are the usual spellings of it. */
export function node(kind, span, extra) {
  return new Node(kind, span, extra);
}

/** A scalar: `kind` is "null", "bool", "int", "float" or "string"; `extra`
 *  may carry `ext_kind`, `tag`, `tag_span`, `anchor`, `anchor_span`. */
export function scalar(kind, span, text, extra) {
  const n = new Node(kind, span, extra);
  n.text = text;
  return n;
}

/** A mapping. `opts.duplicates` says what `map.put` does with a repeated
 *  key: "last_value" (the default — the first entry keeps its place and
 *  span and takes the last value, as `plutil` and every flat format here
 *  do), "first_value" (a later entry is ignored), "keep" (both stay), or
 *  "error" (refused, at the later key). The span may be filled in later. */
export function mapping(span, opts) {
  const n = new Node("mapping", span);
  n.entries = [];
  n.byKey = new Map();
  n.duplicates = opts?.duplicates ?? "last_value";
  return n;
}

/** A sequence; `seq.add(item)` appends. */
export function sequence(span) {
  const n = new Node("sequence", span);
  n.items = [];
  return n;
}

/** A keyvalue over two nodes. The span defaults to the key's start through
 *  the value's end. */
export function entry(key, value, span) {
  const n = new Node("keyvalue", span ?? [key.span[0], value.span[1]]);
  n.key = key;
  n.value = value;
  return n;
}

/** A tree as a node table: rows in pre-order, comments bound by row — what
 *  a `parse` returns for the tree it built. */
export function rows(root) {
  const t = table();
  const emit = (n, parent) => {
    const id = t.row(n.kind, parent, n.span, {
      text: n.text,
      ext_kind: n.ext_kind,
      sep: n.sep,
      marker: n.marker,
      anchor: n.anchor,
      anchor_span: n.anchor_span,
      tag: n.tag,
      tag_span: n.tag_span,
    });
    for (const slot of ["leading", "trailing", "dangling"]) {
      for (const c of n[slot] ?? []) t.comment(id, slot, c.style ?? "line", c.text);
    }
    for (const r of n.regions ?? []) t.region(id, r[0], r[1]);
    for (const m of n.mentions ?? []) t.mention(id, m.span, m.kind);
    if (n.kind === "mapping") {
      for (const e of n.entries) emit(e, id);
    } else if (n.kind === "sequence") {
      for (const item of n.items) emit(item, id);
    } else if (n.kind === "keyvalue") {
      emit(n.key, id);
      if (n.value) emit(n.value, id);
    }
    return id;
  };
  emit(root, null);
  return t;
}

// ── bytes and text ────────────────────────────────────────────────────────

/** The UTF-8 bytes of `s`. */
export function bytesOf(s) {
  return encoder.encode(s);
}

/** How many bytes `s` takes in UTF-8. */
export function byteLength(s) {
  return encoder.encode(s).length;
}

/** The text of UTF-8 `bytes`. */
export function textOf(bytes) {
  return decoder.decode(bytes);
}

/** A string with one char per byte — the scanner's shadow of its input,
 *  which regular expressions run over so that every index is a byte
 *  offset. */
export function toBin(bytes) {
  let out = "";
  for (let i = 0; i < bytes.length; i += 8192) {
    out += String.fromCharCode.apply(null, bytes.subarray(i, i + 8192));
  }
  return out;
}

/** The text a one-char-per-byte string spells: the inverse of `toBin`. */
export function fromBin(bin) {
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return decoder.decode(bytes);
}

/** `toBin` of the UTF-8 encoding of `s`; `s` itself when it is ASCII. */
export function binOf(s) {
  for (let i = 0; i < s.length; i++) {
    if (s.charCodeAt(i) > 0x7f) return toBin(encoder.encode(s));
  }
  return s;
}

/** The UTF-8 bytes of the codepoint `cp`, as an array of byte values —
 *  for a string decoder that builds its text byte by byte. */
export function utf8Bytes(cp) {
  if (cp < 0x80) return [cp];
  if (cp < 0x800) return [0xc0 | (cp >> 6), 0x80 | (cp & 0x3f)];
  if (cp < 0x10000) return [0xe0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f)];
  return [0xf0 | (cp >> 18), 0x80 | ((cp >> 12) & 0x3f), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f)];
}

/** Whether `cp` is a UTF-16 surrogate, high or low: a codepoint no text
 *  can hold on its own. */
export function isSurrogate(cp) {
  return cp >= 0xd800 && cp <= 0xdfff;
}

/** The codepoint a `\uXXXX\uXXXX` surrogate pair spells, or null when
 *  `hi` and `lo` are not a high surrogate followed by a low one. */
export function surrogatePair(hi, lo) {
  if (hi >= 0xd800 && hi <= 0xdbff && lo >= 0xdc00 && lo <= 0xdfff) {
    return 0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00);
  }
  return null;
}

/** A comment's text as the wire carries it: `raw` with the spaces, tabs
 *  and carriage returns at either end removed. */
export function trimComment(raw) {
  return raw.replace(/^[ \t\r]+|[ \t\r]+$/g, "");
}

// ── the token cursor ──────────────────────────────────────────────────────
// A parser written over a token list rather than a grammar — one whose
// tokenizer runs first — walks the list with this: `peek`, `advance`, the
// text a token covers, and a failure at the token in hand.

export class Cursor {
  /** A cursor over `tokens` — each `{ kind, s, e }` over byte offsets,
   *  the last an end-of-input token the cursor never moves past — reading
   *  text through scanner `sc`. */
  constructor(tokens, sc) {
    this.tokens = tokens;
    this.sc = sc;
    this.pos = 0;
  }

  /** The token in hand. */
  peek() {
    return this.tokens[this.pos];
  }

  /** The token `k` (1) past the one in hand, or the last. */
  ahead(k) {
    return this.tokens[Math.min(this.pos + (k ?? 1), this.tokens.length - 1)];
  }

  /** The token in hand, moving to the next unless it is the last. */
  advance() {
    const t = this.tokens[this.pos];
    if (this.pos < this.tokens.length - 1) this.pos += 1;
    return t;
  }

  /** Whether the token in hand is of `kind`. */
  at(kind) {
    return this.tokens[this.pos].kind === kind;
  }

  /** The token in hand when it is of `kind`, moving past it; else null. */
  take(kind) {
    return this.at(kind) ? this.advance() : null;
  }

  /** The text `t` (the token in hand) covers. */
  text(t) {
    t ??= this.tokens[this.pos];
    return this.sc.slice(t.s, t.e);
  }

  /** `fail` at the start of `t` (the token in hand). */
  fail(message, t) {
    fail(message, (t ?? this.tokens[this.pos]).s);
  }
}

/** A cursor over `tokens`, reading through `sc`. */
export function cursor(tokens, sc) {
  return new Cursor(tokens, sc);
}

// ── the scanner ───────────────────────────────────────────────────────────
// A cursor over the input whose `pos` is a 0-based byte offset, so every
// span it hands out is the wire's. Nothing here advances on a miss.

const SPACES = /[ \t]*/y;

export class Scanner {
  /** A scanner over `src` — a string, or the bytes of one — at `pos` (0). */
  constructor(src, pos) {
    this.bytes = typeof src === "string" ? encoder.encode(src) : src;
    /** One char per byte, for regular expressions. */
    this.bin = toBin(this.bytes);
    this.pos = pos ?? 0;
    this.n = this.bytes.length;
  }

  eof() {
    return this.pos >= this.n;
  }

  /** The byte `k` (0) past the cursor, or undefined past either end; `k`
   *  may be negative. */
  byte(k) {
    const i = this.pos + (k ?? 0);
    return i >= 0 && i < this.n ? this.bytes[i] : undefined;
  }

  /** Whether the input at the cursor begins with `s`. */
  starts(s) {
    return this.bin.startsWith(binOf(s), this.pos);
  }

  /** The text of the bytes `[s, e)`. */
  slice(s, e) {
    return decoder.decode(this.bytes.subarray(s, e));
  }

  /** The bytes `[s, e)` as a one-char-per-byte string. */
  binSlice(s, e) {
    return this.bin.slice(s, e);
  }

  /** Match a regular expression anchored at the cursor: `re` is a RegExp
   *  (its `y` flag is added) or its source. Advances past the match and
   *  returns `[s, e, ...captures]`, or null. */
  match(re) {
    const sticky = stickyOf(re);
    sticky.lastIndex = this.pos;
    const m = sticky.exec(this.bin);
    if (m === null) return null;
    const s = this.pos;
    this.pos = s + m[0].length;
    const out = [s, this.pos];
    for (let i = 1; i < m.length; i++) out.push(m[i]);
    return out;
  }

  /** Match the literal `s` at the cursor: advances and returns the span,
   *  or null. */
  lit(s) {
    const b = binOf(s);
    if (!this.bin.startsWith(b, this.pos)) return null;
    const start = this.pos;
    this.pos = start + b.length;
    return [start, this.pos];
  }

  /** The offset of the next `s` at or after the cursor, or -1. */
  find(s) {
    return this.bin.indexOf(binOf(s), this.pos);
  }

  /** Move the cursor `k` bytes (1) forward. */
  advance(k) {
    this.pos += k ?? 1;
    return this.pos;
  }

  /** Skip spaces and tabs. */
  hs() {
    SPACES.lastIndex = this.pos;
    this.pos += SPACES.exec(this.bin)[0].length;
    return this.pos;
  }

  /** Whether no newline lies in `[a, b)`: what "on the same line" means to
   *  a trailing comment. */
  sameLine(a, b) {
    const nl = this.bin.indexOf("\n", a);
    return nl < 0 || nl >= b;
  }

  /** `fail` at the cursor, or at `at`. */
  fail(message, at) {
    fail(message, at ?? this.pos);
  }
}

/** A scanner over `src`, at `pos` (0). */
export function scanner(src, pos) {
  return new Scanner(src, pos);
}

const stickyCache = new Map();

/** `re` with its `y` flag, compiled once per source and flag set. */
export function stickyOf(re) {
  const source = typeof re === "string" ? re : re.source;
  const flags = typeof re === "string" ? "" : re.flags.replace("y", "").replace("g", "");
  const key = flags + "/" + source;
  let sticky = stickyCache.get(key);
  if (!sticky) {
    sticky = new RegExp(source, flags + "y");
    stickyCache.set(key, sticky);
  }
  return sticky;
}

// ── the writer ────────────────────────────────────────────────────────────

export class Writer {
  /** A buffer for a `print`, that knows the `options` it was given:
   *  `w.nl()` and `w.indent(depth)` write nothing when `pretty` is off,
   *  and the unit of indentation is `options.indent` spaces (2). */
  constructor(options) {
    options ??= {};
    this.parts = [];
    this.pretty = options.pretty !== false;
    this.unit = " ".repeat(options.indent ?? 2);
  }

  /** Append each argument. */
  put(...parts) {
    for (const p of parts) this.parts.push(p);
    return this;
  }

  nl() {
    if (this.pretty) this.parts.push("\n");
    return this;
  }

  indent(depth) {
    if (this.pretty && depth > 0) this.parts.push(this.unit.repeat(depth));
    return this;
  }

  /** Indent, write the arguments, end the line. */
  line(depth, ...parts) {
    return this.indent(depth).put(...parts).nl();
  }

  /** Write `comments` (a row's `leading`, say) as full lines, each line
   *  of each comment's text on its own, trimmed, after `leader` and a
   *  space — `# text`, or the bare leader for an empty line — indented by
   *  `depth` (0). Answers whether anything was written. */
  comments(comments, leader, depth) {
    let wrote = false;
    for (const c of comments) {
      for (const line of c.text.split("\n")) {
        const t = line.replace(/^[ \t]+|[ \t]+$/g, "");
        this.indent(depth ?? 0).put(t === "" ? leader : leader + " " + t, "\n");
        wrote = true;
      }
    }
    return wrote;
  }

  string() {
    return this.parts.join("");
  }
}

/** A writer over `options`. */
export function writer(options) {
  return new Writer(options);
}

export default {
  fail,
  isFailure,
  Table,
  table,
  isScalar,
  index,
  children,
  Node,
  node,
  scalar,
  mapping,
  sequence,
  entry,
  rows,
  bytesOf,
  byteLength,
  textOf,
  toBin,
  fromBin,
  binOf,
  utf8Bytes,
  isSurrogate,
  surrogatePair,
  trimComment,
  Cursor,
  cursor,
  Scanner,
  scanner,
  stickyOf,
  Writer,
  writer,
};
