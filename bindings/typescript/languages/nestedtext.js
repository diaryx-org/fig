// NestedText, in JavaScript: a twin of fig's compiled `nestedtext` format.
//
// NestedText is an indentation-based format in which every value is a
// string, held literally — no quoting, no escapes, no types. A line is
// `key: value` or `key:` (a dict item), `: text` (one line of a multiline
// key), `- value` (a list item), `> text` (one line of a string block),
// `# comment`, or blank; a value left empty on its line is the block of
// more-indented lines below it, or the empty string where those lines
// would have begun. A line that begins `{` or `[` where a value starts is
// an inline dict or list, read to the end of that line.
//
// The node table this builds is held row for row against the compiled
// format — kinds, spans, `sep`, `marker` and comments alike — by `cargo
// test --test twins nestedtext`, against the `*.table.json` beside every
// fixture in `tests/fixtures/nestedtext/`; `fig lang table -i nestedtext`
// prints that table for any input. The printer's bytes and the four
// editor renderers are held to the compiled format the same way.
//
// The refusals are the format's, worded here: every document the compiled
// format refuses this module refuses too, in its own words and at its own
// offsets. Indentation is plain spaces: a tab or a no-break space where
// indentation is read is refused. There is no UTF-8 refusal — the input
// reaches a module as text the host already decoded.
//
// Every offset is a byte offset: the tokenizer walks the scanner's
// one-char-per-byte shadow of the input (`sc.bin`) and decodes text from
// the bytes a line covers.
import * as fig from "@diaryx/fig/kit";

// ── the tokenizer ─────────────────────────────────────────────────────────
// One record per physical line: `{ kind, indent, lineStart, s, e }` with
// `kind` one of `blank`, `comment`, `dash`, `colon`, `gt`, `other`, `end`;
// `[s, e)` is the content after the tag and its one space, the whole line
// for `other`.

const TAGS = { 45: "dash", 58: "colon", 62: "gt" };

function tokenize(bin) {
  const n = bin.length;
  const at = (k) => (k < n ? bin.charCodeAt(k) : undefined);
  const lines = [];
  let i = bin.startsWith("\xef\xbb\xbf") ? 3 : 0;
  while (i < n) {
    const lineStart = i;
    let j = i;
    while (at(j) === 32) j += 1;
    let e = j;
    while (e < n && at(e) !== 10 && at(e) !== 13) e += 1;
    const line = { kind: "blank", indent: j - lineStart, lineStart, s: e, e };
    if (j < e) {
      const c = at(j);
      if (c === 9 || (c === 0xc2 && at(j + 1) === 0xa0)) {
        fig.fail("indentation must be plain spaces", lineStart);
      }
      const tagged = TAGS[c] !== undefined && (j + 1 === e || at(j + 1) === 32);
      if (c === 35) {
        line.kind = "comment";
        line.s = j + 1;
      } else if (tagged) {
        line.kind = TAGS[c];
        line.s = Math.min(j + 2, e);
      } else {
        line.kind = "other";
        line.s = j;
      }
    }
    lines.push(line);
    i = e + (at(e) === 13 ? 1 : 0);
    if (at(i) === 10) i += 1;
  }
  lines.push({ kind: "end", indent: 0, lineStart: n, s: n, e: n });
  return lines;
}

// ── the parser ────────────────────────────────────────────────────────────

const INLINE_STOP = new Set([44, 123, 125, 91, 93]);

function attach(node, slot, texts) {
  for (const t of texts) node.comment(slot, t);
  return node;
}

class Parser {
  constructor(sc) {
    this.sc = sc;
    this.bin = sc.bin;
    this.n = sc.n;
    this.lines = tokenize(sc.bin);
    this.pos = 0;
    this.pending = [];
    this.ipos = 0;
    this.iend = 0;
  }

  peek() {
    return this.lines[this.pos];
  }
  atEnd() {
    return this.peek().kind === "end";
  }
  text(s, e) {
    return this.sc.slice(s, e);
  }
  failLine(l, message) {
    fig.fail(message, l.lineStart);
  }

  // Comment lines pile up to lead the next item; blank lines are nothing.
  skipBlank() {
    for (;;) {
      const l = this.peek();
      if (l.kind === "comment") this.pending.push(this.text(l.s, l.e));
      else if (l.kind !== "blank") return;
      this.pos += 1;
    }
  }

  takeLeading() {
    const out = this.pending;
    this.pending = [];
    return out;
  }

  // A line beginning `{` or `[` is an inline value, not a dict item.
  bracket(l) {
    const c = this.bin.charCodeAt(l.s);
    return l.e > l.s && (c === 123 || c === 91);
  }

  // `key: value` / `key:`: where the key ends and the value begins, or null.
  splitDictItem(l) {
    for (let i = l.s; i < l.e; i++) {
      if (this.bin.charCodeAt(i) !== 58) continue;
      if (i + 1 === l.e) return [i, l.e];
      if (this.bin.charCodeAt(i + 1) === 32) return [i, i + 2];
    }
    return null;
  }

  isDictItem(l) {
    return !this.bracket(l) && this.splitDictItem(l) !== null;
  }

  // The value that belongs here: the whole document (`parentIndent` null,
  // at indent 0) or the block under an item whose own line left its value
  // empty (more indented than `parentIndent`). Nothing there is
  // `onMissing`: a null document, an empty string, or a refusal.
  parseRegion(parentIndent, onMissing) {
    this.skipBlank();
    if (this.atEnd() || (parentIndent !== null && this.peek().indent <= parentIndent)) {
      if (onMissing === "null") return fig.scalar("null", [this.n, this.n]);
      if (onMissing === "empty") {
        const at = this.atEnd() ? this.n : this.peek().lineStart;
        return fig.scalar("string", [at, at], "");
      }
      this.failLine(this.peek(), "expected a value here");
    }
    const first = this.peek();
    if (parentIndent === null && first.indent !== 0) {
      this.failLine(first, "the document's first line must start in column 1");
    }
    if (first.kind === "other" && this.bracket(first)) {
      const node = this.parseInlineLine(first);
      this.pos += 1;
      this.skipBlank();
      if (!this.atEnd() && (parentIndent === null || this.peek().indent > parentIndent)) {
        this.failLine(this.peek(), "nothing may follow an inline value");
      }
      return node;
    }
    return this.parseBlock(first.indent);
  }

  parseBlock(indent) {
    const first = this.peek();
    if (first.kind === "dash") return this.parseList(indent);
    if (first.kind === "gt") {
      const [text, span] = this.joinLines("gt", indent);
      return fig.scalar("string", span, text);
    }
    if (first.kind === "colon" || this.isDictItem(first)) return this.parseDict(indent);
    this.failLine(first, "this line is not a dictionary item, a list item, a string line, or a comment");
  }

  // The value written after a `- ` or `key: `, or the block below it when
  // the line left it empty.
  itemValue(s, e, indent) {
    if (e !== s) return fig.scalar("string", [s, e], this.text(s, e));
    return this.parseRegion(indent, "empty");
  }

  parseList(indent) {
    const start = this.peek().lineStart;
    const seq = fig.sequence([start, start]);
    for (;;) {
      this.skipBlank();
      if (this.atEnd()) break;
      const l = this.peek();
      if (l.indent < indent) break;
      if (l.indent > indent) this.failLine(l, "this line is indented past the list it is in");
      if (l.kind !== "dash") this.failLine(l, "expected a list item (`- value`) here");
      this.pos += 1;
      const leading = this.takeLeading();
      const value = attach(this.itemValue(l.s, l.e, indent), "leading", leading);
      const dash = l.lineStart + l.indent;
      value.marker = [dash, dash + 1];
      seq.span[1] = seq.add(value).span[1];
    }
    return seq;
  }

  parseDict(indent) {
    const start = this.peek().lineStart;
    const map = fig.mapping([start, start], { duplicates: "error" });
    for (;;) {
      this.skipBlank();
      if (this.atEnd()) break;
      const l = this.peek();
      if (l.indent < indent) break;
      if (l.indent > indent) this.failLine(l, "this line is indented past the dictionary it is in");
      let key, value, sep;
      if (l.kind === "colon") {
        const [text, span] = this.joinLines("colon", indent);
        key = attach(fig.scalar("string", span, text), "leading", this.takeLeading());
        if (this.atEnd() || this.peek().indent <= indent) {
          this.failLine(this.peek(), "a multiline key needs its value on a more-indented line below it");
        }
        value = this.parseRegion(indent, null);
        sep = [span[1], span[1]];
      } else if (l.kind === "other" && this.isDictItem(l)) {
        const [keyEnd, valueStart] = this.splitDictItem(l);
        this.pos += 1;
        // The key is what precedes the `:`, trimmed of trailing spaces,
        // tabs and no-break spaces, byte by byte.
        const keyBin = this.bin.slice(l.s, keyEnd).replace(/[ \t\xc2\xa0]+$/, "");
        const keyEndTrimmed = l.s + keyBin.length;
        key = fig.scalar("string", [l.s, keyEndTrimmed], this.text(l.s, keyEndTrimmed));
        attach(key, "leading", this.takeLeading());
        value = this.itemValue(valueStart, l.e, indent);
        sep = [keyEnd, keyEnd + 1];
      } else {
        this.failLine(l, "expected a dictionary item (`key: value`, or a `: key` line) here");
      }
      const kv = fig.entry(key, value);
      kv.sep = sep;
      map.put(kv);
      map.span[1] = value.span[1];
    }
    return map;
  }

  // The `: ` lines of a multiline key, or the `> ` lines of a string
  // block, at `indent`: joined with `\n`, spanning the first line's start
  // to the last line's content end; blank and comment lines between are
  // skipped.
  joinLines(kind, indent) {
    const parts = [];
    const start = this.peek().lineStart;
    let end = start;
    for (;;) {
      let i = this.pos;
      while (this.lines[i].kind === "blank" || this.lines[i].kind === "comment") i += 1;
      const l = this.lines[i];
      if (l.kind !== kind || l.indent !== indent) break;
      parts.push(this.text(l.s, l.e));
      end = l.e;
      this.pos = i + 1;
    }
    return [parts.join("\n"), [start, end]];
  }

  // ── inline values ───────────────────────────────────────────────────────
  // `{…}` and `[…]` on one line, scanned over absolute offsets between
  // `ipos` and the line's end.

  byte() {
    return this.bin.charCodeAt(this.ipos);
  }

  skipInlineWs() {
    while (this.ipos < this.iend && (this.byte() === 32 || this.byte() === 9)) this.ipos += 1;
  }

  parseInlineLine(l) {
    this.ipos = l.s;
    this.iend = l.e;
    const node = this.parseInlineValue(null);
    this.skipInlineWs();
    if (this.ipos < this.iend) fig.fail("nothing may follow the closing `}` or `]`", this.ipos);
    return node;
  }

  // A value inside an inline container, or the container itself. Text runs
  // to the next `,` or bracket and keeps its inner spaces, losing the ones
  // at its end.
  parseInlineValue(close) {
    this.skipInlineWs();
    if (this.ipos >= this.iend) fig.fail("this inline value was never closed", this.ipos);
    const c = this.byte();
    if (c === 123 || c === 91) return this.parseInlineContainer(c);
    const start = this.ipos;
    while (this.ipos < this.iend && !INLINE_STOP.has(this.byte())) this.ipos += 1;
    if (this.ipos < this.iend && this.byte() !== 44 && this.byte() !== close) {
      fig.fail("expected `,` or a closing `}` or `]` here", this.ipos);
    }
    return fig.scalar("string", [start, this.ipos], this.text(start, this.ipos).replace(/[ \t]+$/, ""));
  }

  // `{key: value, …}` when `open` is `{`, `[value, …]` when it is `[`. An
  // empty one closes at once; a dict refuses a trailing comma, where a
  // list reads one as an empty value.
  parseInlineContainer(open) {
    const dict = open === 123;
    const close = dict ? 125 : 93;
    const start = this.ipos;
    this.ipos += 1;
    const node = dict ? fig.mapping([start, start], { duplicates: "error" }) : fig.sequence([start, start]);
    if (this.ipos < this.iend && this.byte() === close) {
      this.ipos += 1;
    } else {
      for (;;) {
        if (dict) this.parseInlineEntry(node);
        else node.add(this.parseInlineValue(close));
        this.skipInlineWs();
        if (this.ipos >= this.iend) fig.fail("this inline value was never closed", this.ipos);
        const d = this.byte();
        this.ipos += 1;
        if (d === close) break;
        if (d !== 44) fig.fail("expected `,` or a closing `}` or `]` here", this.ipos - 1);
        if (dict) {
          this.skipInlineWs();
          if (this.ipos < this.iend && this.byte() === close) fig.fail("expected a value after `,`", this.ipos);
        }
      }
    }
    node.span[1] = this.ipos;
    return node;
  }

  parseInlineEntry(map) {
    this.skipInlineWs();
    const start = this.ipos;
    while (this.ipos < this.iend && this.byte() !== 58 && !INLINE_STOP.has(this.byte())) this.ipos += 1;
    if (this.ipos >= this.iend) fig.fail("this inline value was never closed", this.ipos);
    if (this.byte() !== 58) fig.fail("expected `:` after this inline dictionary key", this.ipos);
    const key = fig.scalar("string", [start, this.ipos], this.text(start, this.ipos).replace(/[ \t]+$/, ""));
    this.ipos += 1;
    map.put(fig.entry(key, this.parseInlineValue(125)));
  }
}

function parse(_dialect, input) {
  const p = new Parser(fig.scanner(input));
  const root = p.parseRegion(null, "null");
  p.skipBlank();
  if (!p.atEnd()) p.failLine(p.peek(), "nothing may follow the document's value");
  attach(root, "dangling", p.takeLeading());
  return fig.rows(root);
}

// ── the printer ───────────────────────────────────────────────────────────
// Block form throughout, four spaces a level: `key: value` on one line
// when the value is a non-empty single line and a `>` block below the key
// otherwise, `- ` items likewise, a key that would not read back as one
// spelled as `: ` lines with its value always nested; comments as `#`
// lines — leading above the item, a trailing one on its own line after
// it, dangling at a container's end. An empty container has no block
// spelling — `key:` over nothing reads back as the empty string — so it
// is its inline `{}` or `[]` on a line of its own, at the root as well.
// A null root prints nothing; a scalar root is a `>` block.
//
// Splice text (`options.splice`) is what follows `key:` or `-` in place,
// the compiled `printSplice`: a one-line scalar is its plain text; a
// container, or a string with a line break, is a newline and then its
// nested block at depth 0, which the renderers re-indent under the key;
// a null is printed as a document.

const INDENT = "    ";

// Whether `key` has to be written as `: ` lines: one that would read back
// as a tag, a comment, an inline value, or a second key.
function needsMultilineKey(key) {
  const c0 = key[0];
  if (key === "" || "#{[ \t".includes(c0)) return true;
  if ((c0 === "-" || c0 === ":" || c0 === ">") && (key.length === 1 || key[1] === " ")) return true;
  return key.includes("\n") || key.includes(": ");
}

function scalarText(row) {
  const k = row.kind;
  if (k === "string" || k === "int" || k === "float" || k === "bool") return row.text ?? "";
  if (k === "null") throw new Error("NestedText has no null; a null value cannot be written");
  if (k === "alias") throw new Error("an alias must be resolved before it is written as NestedText");
  throw new Error("a " + k + " is not a value");
}

class Printer {
  constructor(w) {
    this.w = w;
  }

  // `tag` lines (`> ` or `: `) for each line of `text`, a bare tag for an
  // empty line.
  taggedLines(tag, depth, text) {
    for (const line of text.split("\n")) {
      this.w.indent(depth).put(line === "" ? tag : tag + " " + line, "\n");
    }
  }

  itemValue(row, depth, forceNested) {
    if (row.kind === "mapping" || row.kind === "sequence") {
      if (!forceNested) this.w.put("\n");
      this.container(row, depth + 1);
      return;
    }
    const text = scalarText(row);
    if (!forceNested && text !== "" && !text.includes("\n")) {
      this.w.put(" ", text, "\n");
      return;
    }
    if (!forceNested) this.w.put("\n");
    this.taggedLines(">", depth + 1, text);
  }

  container(row, depth) {
    const dict = row.kind === "mapping";
    if (row.items.length === 0) {
      this.w.comments(row.dangling, "#", depth);
      this.w.indent(depth).put(dict ? "{}" : "[]", "\n");
      return;
    }
    for (const item of row.items) {
      const value = dict ? item.value : item;
      this.w.comments((dict ? item.key : item).leading, "#", depth);
      let multiline = false;
      if (dict) {
        if (item.key.kind !== "string") throw new Error("a NestedText key must be a string");
        const keyText = item.key.text ?? "";
        multiline = needsMultilineKey(keyText);
        if (multiline) this.taggedLines(":", depth, keyText);
        else this.w.indent(depth).put(keyText, ":");
      } else {
        this.w.indent(depth).put("-");
      }
      this.itemValue(value, depth, multiline);
      this.w.comments(value.trailing, "#", depth);
    }
    this.w.comments(row.dangling, "#", depth);
  }
}

function print(_dialect, t, options) {
  const w = fig.writer({ indent: INDENT.length });
  const p = new Printer(w);
  const root = fig.index(t).byid(0);
  const splice = options?.splice === true;
  if (root.kind === "null") return "";
  if (root.kind === "mapping" || root.kind === "sequence") {
    if (splice) w.put("\n");
    p.container(root, 0);
  } else {
    const text = scalarText(root);
    if (splice && !text.includes("\n")) return text;
    if (splice) w.put("\n");
    p.taggedLines(">", 0, text);
  }
  return w.string();
}

// ── the renderers ─────────────────────────────────────────────────────────
// What the editor splices. `args.indent` is the line's indentation; a
// value that is empty or has a line break goes under its key as a `>`
// block one level in, a single line rides the key's line — unless it is
// a nested block (`nestedBlock`): a line break and then lines that read as
// NestedText on their own, which is how splice text spells a container or
// a multi-line string. Those lines are re-indented under the key as they
// stand.

// Each line of `text` after `tag` — a bare tag for an empty line — joined
// with `sep`.
function tagged(text, tag, sep) {
  return text.split("\n").map((line) => (line === "" ? tag : tag + " " + line)).join(sep);
}

// The nested block `text` spells, when it spells one: text that opens with
// a line break and whose rest, trailing line breaks trimmed, parses as
// NestedText with a value that is not null — returned without the leading
// and trailing breaks. Otherwise null, and `text` is a string: an argument
// means structure only if it starts with a line break AND the rest is
// NestedText.
function nestedBlock(text) {
  if (text.length < 2 || text[0] !== "\n") return null;
  const block = text.slice(1).replace(/\n+$/, "");
  if (block === "") return null;
  let root;
  try {
    root = parse("nestedtext", block).rows[0];
  } catch {
    return null;
  }
  return root === undefined || root.kind === "null" ? null : block;
}

function valueTail(out, childIndent, text, forceNested) {
  const block = nestedBlock(text);
  if (block !== null) {
    for (const line of block.split("\n")) out.push("\n" + (line === "" ? "" : childIndent) + line);
    return;
  }
  if (!forceNested && text !== "" && !text.includes("\n")) out.push(" " + text);
  else out.push("\n" + childIndent + tagged(text, ">", "\n" + childIndent));
}

// Whether the key's source text is already the `: ` spelling.
function isMultilineKeyText(key) {
  const k = key.replace(/^[ \t]+/, "");
  if (k === "" || k[0] !== ":") return false;
  return k.length === 1 || k[1] === " " || k[1] === "\n" || k[1] === "\r";
}

function render(which, args) {
  const out = [];
  const indent = args.indent ?? "";
  const child = indent + INDENT;
  if (which === "entry") {
    const multiline = needsMultilineKey(args.key);
    out.push(multiline ? tagged(args.key, ":", "\n" + indent) : args.key + ":");
    valueTail(out, child, args.value, multiline);
  } else if (which === "item") {
    out.push("-");
    valueTail(out, child, args.value, false);
  } else if (which === "tail") {
    if (args.key === "") {
      // The document root: a nested block as it stands, at column 0, or
      // else a `>` block — a bare scalar line has no grammar at the top.
      out.push(nestedBlock(args.value) ?? tagged(args.value, ">", "\n"));
    } else {
      const multiline = isMultilineKeyText(args.key);
      if (!multiline) out.push(":");
      valueTail(out, child, args.value, multiline);
    }
  } else if (which === "key") {
    const wasMultiline = isMultilineKeyText(args.old_key ?? "");
    if (needsMultilineKey(args.key)) {
      if (!wasMultiline) {
        throw new Error("this key needs the `: ` multiline spelling, which cannot replace a plain key in place");
      }
      out.push(indent + tagged(args.key, ":", "\n" + indent));
    } else {
      out.push(wasMultiline ? args.key + ":" : args.key);
    }
  } else {
    throw new Error("no renderer named " + String(which));
  }
  return out.join("");
}

/** @type {import("@diaryx/fig/helper").Language} */
const language = {
  name: "nestedtext",
  caps: { read: true, edit: true, serialize: true },
  syntax: {
    comments: { style: "hash", line: { open: "#" } },
    kv_sep: ": ",
    indent_unit: "    ",
    seq_item_marker: "- ",
  },
  // The compiled format's extension: where the format is compiled out
  // and this stands in for it, the extension reaches it.
  dialects: [{ name: "nestedtext", extensions: ["nt"], splice: "raw", empty_doc_seed: "" }],
  samples: ["a: 1\nb:\n  - x\n  - y\n"],
  // The compiled editor's four renderers: an entry, an item, a value's
  // tail after its key, and a key over an old one.
  renderers: ["entry", "item", "tail", "key"],
  parse,
  print,
  render,
};

export default language;
