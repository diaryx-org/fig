// TOML, in JavaScript: a twin of fig's compiled `toml` format — TOML 1.1,
// the default dialect.
//
// What is held to the compiled format is the *table*, not the parser's
// shape. `cargo test --test twins toml` parses every fixture under
// `tests/fixtures/toml/` through this module and compares the node table
// it builds — every row, kind, `ext_kind`, text, span, comment, region and
// mention — against the `*.table.json` beside it, which is what `fig lang
// table -i toml` printed; it then prints those tables back and requires
// this printer's bytes to be the compiled printer's, and edits both the
// same way. The grammar module's rules would not do: TOML is a section
// format, and which line may open or extend which table is stated here by
// hand. What every section format shares — the region a header line is,
// the mention a name is, the comments waiting for a key — is the grammar
// module's `sections`, and comes from there.
//
// The shape of the table:
//
//   * the root mapping spans the whole input; a `key = value` line is a
//     keyvalue from the key's start to the value's end; a string's span
//     includes its quotes, a container's its brackets;
//   * a `[a.b]` header, a `[[a]]` header and a dotted `a.b = 1` line each
//     make a *section* table per segment they create: a keyvalue and a
//     mapping whose span is just that segment's key text. An array of
//     tables is a sequence spanning its first header's key text, and so is
//     every element mapping in it. A header names the table again rather
//     than creating it when it exists;
//   * a table's *regions* are the header lines that created or reopened it
//     — `[a]`, `[[a]]`, and a dotted line for every table it passes
//     through — each the whole physical line; an element of an array of
//     tables carries its own `[[a]]` line, and the array every one;
//   * a table's *mentions* are every place its name is written: each
//     segment of a header (`header`) or of a dotted key (`entry`), on the
//     node that segment names — an array of tables, not its element;
//   * an integer's text is its decimal value (`0xff` → `255`, `1_000` →
//     `1000`, `+5` → `5`); a float keeps its lexeme minus underscores,
//     `+inf` is `inf`, every nan is `nan`; a datetime is a string with an
//     `ext_kind` and its lexeme as text;
//   * a comment on the line of a top-level `key = value` trails the value;
//     any other waits and leads the next key or header's key, whatever it
//     was inside — an array, a `[header]` line — and at the end of the
//     file dangles on the table the last header opened.
//
// The refusals are the format's, worded here: every document the compiled
// format refuses this module refuses too, with a message of its own and an
// offset that points at the problem rather than at the compiled parser's
// caret.
//
// Every offset is a byte offset: the tokenizer walks the scanner's
// one-char-per-byte shadow of the input (`sc.bin`) and decodes text from
// the bytes a token covers. The same object `@diaryx/fig`'s
// `registerLanguage` takes, so it serves the browser and Node unchanged.
import * as fig from "@diaryx/fig/kit";
import * as DT from "@diaryx/fig/kit/datetime";
import * as G from "@diaryx/fig/kit/grammar";
import * as N from "@diaryx/fig/kit/number";

// ── refusals ──────────────────────────────────────────────────────────────
// The wordings more than one rule reaches for; the rest are written where
// they are raised.

const UNEXPECTED = "unexpected token here; check for a missing `=`, `.`, `,`, or a closing `]`/`}`";
const DUPLICATE = "this key or table conflicts with one already defined; a TOML key or table may be defined only once";
const UNCLOSED = "unclosed string; close the quote, or use a triple-quoted string (`\"\"\"`/`'''`) for text over several lines";
const BAD_ESCAPE = 'invalid escape; a basic string takes \\b \\t \\n \\f \\r \\e \\" \\\\ \\xXX \\uXXXX \\UXXXXXXXX — use a literal string (\'...\') for raw text';
const BAD_UNICODE = "invalid unicode escape; those hex digits are not a Unicode codepoint";
const BAD_KEY = "invalid key; a bare key allows only letters, digits, `-` and `_` — quote it for anything else";
const CONTROL = "a control character is not allowed here; only tab is, outside a multi-line string";

// ── the tokenizer ─────────────────────────────────────────────────────────
// Line-oriented and context-sensitive: the same bytes are a bare key
// before `=` and a date after it, so a per-line key/value position is
// tracked, and a stack of open `[`/`{` says whether an inline table is at
// a key or a value.

const BARE_KEY = /^[A-Za-z0-9_-]+$/;
const isBareKeyChar = (c) => (c >= 65 && c <= 90) || (c >= 97 && c <= 122) || (c >= 48 && c <= 57) || c === 95 || c === 45;
const isValueTerminator = (c) => c === 32 || c === 9 || c === 10 || c === 13 || c === 35 || c === 44 || c === 93 || c === 125 || c === 61;
const CONTROL_CHAR = /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/;

class Tokenizer {
  constructor(bin) {
    this.bin = bin;
    this.n = bin.length;
    this.i = 0;
    this.tokens = [];
    this.inValue = false;
    this.flow = [];
  }

  byte(k) {
    const i = this.i + (k ?? 0);
    return i < this.n ? this.bin.charCodeAt(i) : undefined;
  }

  emit(kind, s, e) {
    this.tokens.push({ kind, s, e });
  }

  /** Emit a one-byte token and step past it. */
  one(kind) {
    this.emit(kind, this.i, this.i + 1);
    this.i += 1;
  }

  /** How far `re` reaches from `at`, or null. */
  matchAt(re, at) {
    const sticky = fig.stickyOf(re);
    sticky.lastIndex = at;
    return sticky.test(this.bin) ? sticky.lastIndex : null;
  }

  run() {
    if (this.bin.startsWith("\xef\xbb\xbf")) this.i = 3;
    while (this.i < this.n) {
      const c = this.byte();
      if (c === 10 || (c === 13 && this.byte(1) === 10)) {
        const e = this.i + (c === 10 ? 1 : 2);
        this.emit("newline", this.i, e);
        this.i = e;
        if (this.flow.length === 0) this.inValue = false;
      } else if (c === 13) {
        fig.fail("a bare `\\r` must be followed by `\\n`; TOML line endings are `\\n` or `\\r\\n`", this.i);
      } else if (c === 32 || c === 9) {
        const s = this.i;
        this.i = this.matchAt(/[ \t]+/, s);
        this.emit("whitespace", s, this.i);
      } else if (c === 35) {
        const s = this.i;
        this.i = this.matchAt(/[^\n\r]*/, s);
        const bad = this.bin.slice(s, this.i).search(CONTROL_CHAR);
        if (bad >= 0) fig.fail(CONTROL, s + bad);
        this.emit("comment", s, this.i);
      } else if (this.inValue || this.flow.length > 0) {
        this.lexValue();
      } else {
        this.lexKey();
      }
    }
    this.emit("end_of_file", this.n, this.n);
    return this.tokens;
  }

  lexKey() {
    const c = this.byte();
    if (c === 91 || c === 93) {
      // `[[`/`]]` are an array of tables' header, `[`/`]` a table's.
      const kind = c === 91 ? "open_bracket" : "close_bracket";
      const width = this.byte(1) === c ? 2 : 1;
      this.emit(width === 2 ? "double_" + kind : kind, this.i, this.i + width);
      this.i += width;
    } else if (c === 61) {
      this.one("equals");
      this.inValue = true;
    } else if (c === 46) {
      this.one("dot");
    } else if (c === 34 || c === 39) {
      const s = this.i;
      this.scanString(c, false);
      this.emit("key", s, this.i);
    } else {
      this.lexBareKey();
    }
  }

  lexBareKey() {
    const s = this.i;
    while (this.i < this.n && isBareKeyChar(this.byte())) this.i += 1;
    if (this.i === s) fig.fail(BAD_KEY, this.i);
    this.emit("key", s, this.i);
  }

  lexValue() {
    const c = this.byte();
    const top = this.flow[this.flow.length - 1];
    if (c === 91) {
      this.one("open_bracket");
      this.flow.push({ table: false });
    } else if (c === 123) {
      this.one("open_brace");
      this.flow.push({ table: true, expectKey: true });
    } else if (c === 93 || c === 125) {
      this.flow.pop();
      this.one(c === 93 ? "close_bracket" : "close_brace");
    } else if (c === 44) {
      this.one("comma");
      if (top && top.table) top.expectKey = true;
    } else if (c === 61) {
      this.one("equals");
      if (top && top.table) top.expectKey = false;
    } else if (c === 46) {
      this.one("dot");
    } else if (c === 34 || c === 39) {
      // A quoted key in an inline table's key position, else a string
      // value; both lex the same and the parser reads them by position.
      const s = this.i;
      this.scanString(c, this.byte(1) === c && this.byte(2) === c);
      this.emit("string", s, this.i);
    } else if (top && top.table && top.expectKey) {
      this.lexBareKey();
    } else {
      const e = this.matchDatetime(this.i);
      if (e !== null) {
        this.emit("datetime", this.i, e);
        this.i = e;
        return;
      }
      const s = this.i;
      while (this.i < this.n && !isValueTerminator(this.byte())) this.i += 1;
      if (this.i === s) fig.fail("not a recognized value; a TOML value is a string, number, boolean, datetime, array or inline table", this.i);
      const word = this.bin.slice(s, this.i);
      this.emit(word === "true" || word === "false" ? "boolean" : "number", s, this.i);
    }
  }

  // The four string forms are one scan: `multiline` says whether the
  // delimiter is three quotes, which lets newlines in and lets up to two
  // more quotes hug the close.
  scanString(q, multiline) {
    this.i += multiline ? 3 : 1;
    const basic = q === 34;
    while (this.i < this.n) {
      const c = this.byte();
      if (c === 10 || c === 13) {
        if (!multiline) fig.fail(UNCLOSED, this.i);
        if (c === 13 && this.byte(1) !== 10) fig.fail(CONTROL, this.i);
      } else if ((c < 32 && c !== 9) || c === 127) {
        fig.fail(CONTROL, this.i);
      }
      if (basic && c === 92) {
        this.i += 2;
      } else if (c === q && (!multiline || (this.byte(1) === q && this.byte(2) === q))) {
        this.i += multiline ? 3 : 1;
        for (let extra = 0; multiline && extra < 2 && this.byte() === q; extra++) this.i += 1;
        return;
      } else {
        this.i += 1;
      }
    }
    fig.fail(UNCLOSED, this.i);
  }

  // A datetime's outline, as far as it reaches from `at`, or null; whether
  // it spells a real instant is `DT.classify`'s to say.
  matchTime(at) {
    const hm = this.matchAt(/\d\d:\d\d/, at);
    if (hm === null) return null;
    const sec = this.matchAt(/:\d\d/, hm);
    if (sec === null) return hm;
    return this.matchAt(/\.\d+/, sec) ?? sec;
  }

  matchDatetime(at) {
    const date = this.matchAt(/\d{4}-\d\d-\d\d/, at);
    if (date === null) return this.matchTime(at);
    const sep = this.matchAt(/[Tt ]/, date);
    if (sep === null) return date;
    const time = this.matchTime(sep);
    if (time === null) return date;
    return this.matchAt(/[Zz]/, time) ?? this.matchAt(/[+-]\d\d:\d\d/, time) ?? time;
  }
}

// ── scalars ───────────────────────────────────────────────────────────────

const SPECIALS = new Set(["inf", "+inf", "-inf", "nan", "+nan", "-nan"]);
const DIGIT = /[0-9]/;

// The digits of each radix prefix: one of them, and a whole run.
const RADIX = {
  "0x": [/[0-9a-fA-F]/, /^[0-9a-fA-F]+$/],
  "0o": [/[0-7]/, /^[0-7]+$/],
  "0b": [/[01]/, /^[01]+$/],
};

// `s` with its underscores removed, or null unless every one sits between
// two characters of `cls` — TOML's digit grouping.
function ungroup(s, cls) {
  for (let i = 0; i < s.length; i++) {
    if (s[i] === "_" && !(cls.test(s[i - 1] ?? "") && cls.test(s[i + 1] ?? ""))) return null;
  }
  return s.replace(/_/g, "");
}

// "int", "float", or null for not a number at all.
function classifyNumber(raw) {
  if (SPECIALS.has(raw)) return "float";
  const radix = RADIX[raw.slice(0, 2)];
  if (radix) {
    const digits = ungroup(raw.slice(2), radix[0]);
    return digits !== null && radix[1].test(digits) ? "int" : null;
  }
  const body = ungroup(raw.replace(/^[+-]/, ""), DIGIT);
  if (body === null || /^0[0-9]/.test(body)) return null;
  if (/^[0-9]+$/.test(body)) return "int";
  return /^[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?$/.test(body) ? "float" : null;
}

const I64_MAX = 9223372036854775807n;
const I64_MIN = -9223372036854775808n;

// An integer's text is its value in decimal: radix converted, underscores
// and a leading `+` gone, `-0` just `0`. Beyond i64 there is no value, and
// the document is refused.
function canonicalInt(raw) {
  const v = BigInt(N.canonical(raw));
  return v > I64_MAX || v < I64_MIN ? null : v.toString();
}

// A float keeps its lexeme; only the grouping and the spelling of the
// specials are normalized.
function canonicalFloat(raw) {
  if (raw === "inf" || raw === "+inf") return "inf";
  if (raw === "-inf") return "-inf";
  if (raw === "nan" || raw === "+nan" || raw === "-nan") return "nan";
  return raw.replace(/_/g, "");
}

// ── string decoding ───────────────────────────────────────────────────────

function trimLeadingNewline(inner) {
  if (inner.startsWith("\n")) return inner.slice(1);
  if (inner.startsWith("\r\n")) return inner.slice(2);
  return inner;
}

const SIMPLE = { b: "\b", t: "\t", n: "\n", f: "\f", r: "\r", e: "\x1b", '"': '"', "\\": "\\" };

// ── the parser ────────────────────────────────────────────────────────────
// A token cursor — `peek`, `advance`, `at`, `take`, `text` and `fail` come
// from `fig.cursor` — with TOML's table rules written over it.

class Parser extends fig.Cursor {
  constructor(tokens, sc) {
    super(tokens, sc);
    this.sections = G.sections(sc.bin); // regions, mentions, comments waiting for a key
    this.lastValue = null; // the node a same-line comment trails
    this.meta = new Map(); // per table: explicit / dotted / implicit / aot / inlineTable
  }

  // ── strings ──

  decodeBasic(inner, multiline, t) {
    let out = "";
    let i = 0;
    const n = inner.length;
    while (i < n) {
      const c = inner[i];
      if (c !== "\\") {
        out += c;
        i += 1;
        continue;
      }
      const e = inner[i + 1];
      if (Object.hasOwn(SIMPLE, e)) {
        out += SIMPLE[e];
        i += 2;
      } else if (e === "u" || e === "U" || e === "x") {
        const width = e === "u" ? 4 : e === "U" ? 8 : 2;
        const hex = inner.slice(i + 2, i + 2 + width);
        if (hex.length < width || !/^[0-9a-fA-F]+$/.test(hex)) this.fail(BAD_UNICODE, t);
        const cp = parseInt(hex, 16);
        if (cp > 0x10ffff || fig.isSurrogate(cp)) this.fail(BAD_UNICODE, t);
        out += String.fromCodePoint(cp);
        i += 2 + width;
      } else if (e === " " || e === "\t" || e === "\n" || e === "\r") {
        // A line-ending backslash: whitespace, then a newline, then every
        // blank up to the next content, all trimmed.
        if (!multiline) this.fail(BAD_ESCAPE, t);
        let j = i + 1;
        while (j < n && (inner[j] === " " || inner[j] === "\t")) j += 1;
        if (j >= n || (inner[j] !== "\n" && inner[j] !== "\r")) this.fail(BAD_ESCAPE, t);
        while (j < n && " \t\r\n".includes(inner[j])) j += 1;
        i = j;
      } else {
        this.fail(BAD_ESCAPE, t);
      }
    }
    return out;
  }

  // Any of the four string forms to its value.
  decodeString(t) {
    const raw = this.text(t);
    if (raw.length < 2) this.fail(UNCLOSED, t);
    const q = raw[0];
    const triple = raw.length >= 6 && raw[1] === q && raw[2] === q;
    const inner = triple ? trimLeadingNewline(raw.slice(3, -3)) : raw.slice(1, -1);
    if (q === "'" || !inner.includes("\\")) return inner;
    return this.decodeBasic(inner, triple, t);
  }

  // ── comments and trivia ──

  captureComment(t) {
    const raw = this.text(t);
    const text = fig.trimComment(raw.startsWith("#") ? raw.slice(1) : raw);
    if (this.lastValue !== null) {
      this.lastValue.comment("trailing", text);
      this.lastValue = null;
    } else {
      this.sections.comment(text);
    }
  }

  // Whitespace and comments, and blank lines too when `newlines`. A
  // comment before a newline is on the line just parsed, so it can trail
  // that line's value; past one it waits and leads the next key.
  skip(newlines) {
    for (;;) {
      const k = this.peek().kind;
      if (k === "comment") this.captureComment(this.peek());
      else if (k === "newline" && newlines) this.lastValue = null;
      else if (k !== "whitespace") return;
      this.pos += 1;
    }
  }

  skipInline() {
    this.skip(false);
  }

  skipBlank() {
    this.skip(true);
  }

  requireLineEnd() {
    this.skipInline();
    if (!this.at("newline") && !this.at("end_of_file")) {
      this.fail("unexpected content after this line's value; a TOML statement ends its line, and a `#` comment wants whitespace before it");
    }
  }

  // ── keys ──

  decodeKey(t) {
    const raw = this.text(t);
    if (raw === "") this.fail(BAD_KEY, t);
    return raw[0] === '"' || raw[0] === "'" ? this.decodeString(t) : raw;
  }

  // `a.b.c`: the cursor at the first key.
  parseKeyPath() {
    const segs = [];
    for (;;) {
      const t = this.peek();
      if (t.kind !== "key") this.fail(UNEXPECTED);
      this.advance();
      segs.push({ str: this.decodeKey(t), span: [t.s, t.e] });
      this.skipInline();
      if (!this.take("dot")) return segs;
      this.skipInline();
    }
  }

  lookupChild(map, key) {
    const e = map.byKey.get(key);
    return e ? e.value : null;
  }

  keyOf(seg) {
    const key = fig.scalar("string", seg.span, seg.str);
    this.sections.claim(key, "leading");
    return key;
  }

  appendKeyValue(map, seg, value) {
    return map.put(fig.entry(this.keyOf(seg), value, [seg.span[0], value.span[1]]));
  }

  // A table `seg` names, opened under `parent`: its header line the first
  // region, its name the first mention, of `kind`.
  createTable(parent, seg, meta, kind) {
    const m = fig.mapping(seg.span, { duplicates: "keep" });
    this.sections.open(parent, fig.entry(this.keyOf(seg), m), kind);
    this.meta.set(m, meta);
    return m;
  }

  appendArrayElement(seq) {
    return seq.add(fig.mapping(seq.span, { duplicates: "keep" }));
  }

  // An existing path node to continue from: a table itself, an array of
  // tables its last element; anything else is a conflict.
  descend(child, seg) {
    const meta = this.meta.get(child) ?? {};
    if (child.kind === "mapping" && !meta.inlineTable) return child;
    if (child.kind === "sequence" && meta.aot && child.items.length > 0) return child.items[child.items.length - 1];
    fig.fail(DUPLICATE, seg.span[0]);
  }

  navigateHeaderPath(start, segs, count) {
    let cur = start;
    for (let j = 0; j < count; j++) {
      const seg = segs[j];
      const child = this.lookupChild(cur, seg.str);
      if (child === null) {
        cur = this.createTable(cur, seg, { implicit: true }, "header");
      } else {
        // Passed through, not reopened: a mention and no region.
        this.sections.mention(child, seg.span, "header");
        cur = this.descend(child, seg);
      }
    }
    return cur;
  }

  navigateDottedPath(start, segs, count) {
    let cur = start;
    for (let j = 0; j < count; j++) {
      const seg = segs[j];
      const child = this.lookupChild(cur, seg.str);
      if (child === null) {
        cur = this.createTable(cur, seg, { dotted: true }, "entry");
      } else {
        const meta = this.meta.get(child) ?? {};
        if (child.kind !== "mapping" || meta.explicit || meta.inlineTable) fig.fail(DUPLICATE, seg.span[0]);
        this.sections.reopen(child, seg.span, "entry");
        cur = child;
      }
    }
    return cur;
  }

  // ── statements ──

  // `[a.b]`, and `[[a.b]]` when `array`: every segment but the last is
  // navigated from the root, and the last is the table the line opens or
  // reopens and the one the lines below it fill.
  parseHeader(array) {
    this.advance();
    this.skipInline();
    const segs = this.parseKeyPath();
    this.skipInline();
    if (!this.take(array ? "double_close_bracket" : "close_bracket")) this.fail(UNEXPECTED);
    const cur = this.navigateHeaderPath(this.root, segs, segs.length - 1);
    const final = segs[segs.length - 1];
    const child = this.lookupChild(cur, final.str);
    const meta = child === null ? {} : (this.meta.get(child) ?? {});
    if (!array) {
      if (child === null) {
        this.current = this.createTable(cur, final, { explicit: true }, "header");
        return;
      }
      if (child.kind !== "mapping" || meta.explicit || meta.dotted || meta.inlineTable) fig.fail(DUPLICATE, final.span[0]);
      this.meta.set(child, { explicit: true });
      this.sections.reopen(child, final.span, "header");
      this.current = child;
      return;
    }
    let seq = child;
    if (seq === null) {
      seq = fig.sequence(final.span);
      this.sections.open(cur, fig.entry(this.keyOf(final), seq), "header");
      this.meta.set(seq, { aot: true });
    } else {
      if (seq.kind !== "sequence" || !meta.aot) fig.fail(DUPLICATE, final.span[0]);
      this.sections.reopen(seq, final.span, "header");
    }
    this.current = this.appendArrayElement(seq);
    // The element shares the array's span, so its `[[…]]` line is recorded
    // on the element too: the only way its header is findable.
    this.sections.region(this.current, final.span[0]);
  }

  parseKeyValue() {
    const segs = this.parseKeyPath();
    this.skipInline();
    if (!this.take("equals")) this.fail(UNEXPECTED);
    this.skipInline();
    const cur = this.navigateDottedPath(this.current, segs, segs.length - 1);
    const final = segs[segs.length - 1];
    if (this.lookupChild(cur, final.str) !== null) fig.fail(DUPLICATE, final.span[0]);
    const value = this.parseValue();
    this.lastValue = value;
    this.appendKeyValue(cur, final, value);
  }

  // ── values ──

  parseValue() {
    const t = this.peek();
    const k = t.kind;
    if (k === "open_bracket") return this.parseArray();
    if (k === "open_brace") return this.parseInlineTable();
    this.advance();
    const span = [t.s, t.e];
    if (k === "string") return fig.scalar("string", span, this.decodeString(t));
    if (k === "boolean") return fig.scalar("bool", span, this.text(t));
    const raw = this.text(t);
    if (k === "datetime") {
      const shape = DT.classify(raw, { minutePrecision: true });
      if (shape === null) this.fail("not a valid RFC 3339 date or time", t);
      return fig.scalar("string", span, raw, { ext_kind: shape });
    }
    if (k === "number") {
      const kind = classifyNumber(raw);
      if (kind === null) {
        this.fail("not a TOML value; TOML has no bare strings, so quote text — and a number takes one `_` between digits, no leading zero, and `0x`/`0o`/`0b` for another radix", t);
      }
      const text = kind === "int" ? canonicalInt(raw) : canonicalFloat(raw);
      if (text === null) this.fail("this integer is too large for the 64 bits TOML gives it", t);
      return fig.scalar(kind, span, text);
    }
    this.fail(UNEXPECTED, t);
  }

  parseArray() {
    const start = this.peek().s;
    this.advance();
    const seq = fig.sequence([start, start + 1]);
    for (;;) {
      this.skipBlank();
      if (this.at("close_bracket")) break;
      seq.add(this.parseValue());
      this.skipBlank();
      if (this.at("close_bracket")) break;
      if (!this.take("comma")) this.fail(UNEXPECTED);
    }
    seq.span = [start, this.peek().e];
    this.advance();
    return seq;
  }

  decodeInlineKey(t) {
    const k = t.kind;
    if (k === "string") return this.decodeString(t);
    if (k === "key") return this.text(t);
    // A number, a boolean or a date in key position is a bare key when
    // its bytes spell one: `1 = true`, `2024 = "y"`.
    if (k === "number" || k === "boolean" || k === "datetime") {
      const text = this.text(t);
      if (!BARE_KEY.test(text)) this.fail(BAD_KEY, t);
      return text;
    }
    this.fail(UNEXPECTED, t);
  }

  parseInlineEntry(map) {
    const segs = [];
    for (;;) {
      const t = this.peek();
      segs.push({ str: this.decodeInlineKey(t), span: [t.s, t.e] });
      this.advance();
      this.skipBlank();
      if (!this.take("dot")) break;
      this.skipBlank();
    }
    if (!this.take("equals")) this.fail(UNEXPECTED);
    this.skipBlank();
    const cur = this.navigateDottedPath(map, segs, segs.length - 1);
    const final = segs[segs.length - 1];
    if (this.lookupChild(cur, final.str) !== null) fig.fail(DUPLICATE, final.span[0]);
    this.appendKeyValue(cur, final, this.parseValue());
  }

  parseInlineTable() {
    const start = this.peek().s;
    this.advance();
    const map = fig.mapping([start, start + 1], { duplicates: "keep" });
    this.meta.set(map, { inlineTable: true });
    this.skipBlank();
    while (!this.at("close_brace")) {
      this.parseInlineEntry(map);
      this.skipBlank();
      if (this.at("close_brace")) break;
      // TOML 1.1 lets a newline and a trailing comma into an inline table.
      if (!this.take("comma")) this.fail(UNEXPECTED);
      this.skipBlank();
    }
    if (!this.at("close_brace")) this.fail(UNEXPECTED);
    map.span = [start, this.peek().e];
    this.advance();
    return map;
  }
}

// ── the document ──

function parse(_dialect, input) {
  const sc = fig.scanner(input);
  const p = new Parser(new Tokenizer(sc.bin).run(), sc);
  const root = fig.mapping([0, sc.n], { duplicates: "keep" });
  p.root = root;
  p.current = root;
  p.skipBlank();
  while (!p.at("end_of_file")) {
    const k = p.peek().kind;
    if (k === "key") p.parseKeyValue();
    else if (k === "open_bracket") p.parseHeader(false);
    else if (k === "double_open_bracket") p.parseHeader(true);
    else p.fail(UNEXPECTED);
    p.requireLineEnd();
    p.skipBlank();
  }
  // Comments left at the end of the file dangle off the table the last
  // header opened.
  p.sections.claim(p.current, "dangling");
  return fig.rows(root);
}

// ── the printer ───────────────────────────────────────────────────────────
// The root's scalar and array entries as `key = value` lines; a mapping
// inline when the whole line fits the width and nothing in it carries a
// comment, else a `[section]`; a non-empty sequence of mappings always
// `[[array.of.tables]]`; an array wider than the budget wrapped one element
// per line. TOML puts a table's sections after its lines, so a table child
// that sits before a later inline sibling is *demoted* to dotted keys (an
// inline array, for an array of tables) to keep its place — unless it
// carries comments, which outrank order and keep the section form. Widths
// are byte widths.

const ESCAPES = { '"': '\\"', "\\": "\\\\", "\b": "\\b", "\t": "\\t", "\n": "\\n", "\f": "\\f", "\r": "\\r" };

function basicString(s) {
  return (
    '"' +
    s.replace(/[\x00-\x1f"\\\x7f]/g, (ch) => ESCAPES[ch] ?? "\\u" + ch.charCodeAt(0).toString(16).toUpperCase().padStart(4, "0")) +
    '"'
  );
}

function keyText(row) {
  if (row.kind !== "string") throw new Error("a TOML key must be a string");
  return row.text ?? "";
}

const spellKey = (name) => (BARE_KEY.test(name) ? name : basicString(name));
const spellPath = (path) => path.map(spellKey).join(".");

function tomlSpecial(text) {
  if (text.endsWith("NaN")) return text[0] === "-" ? "-nan" : text[0] === "+" ? "+nan" : "nan";
  return text[0] === "-" ? "-inf" : text[0] === "+" ? "+inf" : "inf";
}

function inlineValue(row) {
  const k = row.kind;
  if (k === "null") throw new Error("TOML has no null; a null value cannot be written");
  if (k === "bool" || k === "int" || k === "float") return row.text;
  if (k === "string") {
    const ext = row.ext_kind;
    if (ext == null) return basicString(row.text ?? "");
    if (DT.KINDS.has(ext) || ext === "char_literal") return row.text;
    if (ext === "number_special") return tomlSpecial(row.text);
    return basicString(row.text ?? "");
  }
  if (k === "sequence") return row.items.length === 0 ? "[]" : "[" + row.items.map(inlineValue).join(", ") + "]";
  if (k === "mapping") {
    if (row.items.length === 0) return "{}";
    return "{ " + row.items.map((e) => spellKey(keyText(e.key)) + " = " + inlineValue(e.value)).join(", ") + " }";
  }
  if (k === "alias") throw new Error("an alias must be resolved before it is written as TOML");
  throw new Error("a " + k + " is not a value");
}

const hasComments = (row) => row.leading.length > 0 || row.trailing.length > 0 || row.dangling.length > 0;

function subtreeHasComments(row) {
  if (hasComments(row)) return true;
  if (row.kind === "sequence") return row.items.some(subtreeHasComments);
  if (row.kind === "mapping") return row.items.some((e) => hasComments(e) || subtreeHasComments(e.key) || subtreeHasComments(e.value));
  return false;
}

// Whether anything about an entry is commented: what keeps a table in its
// section form rather than letting it demote or go inline.
const commented = (e) => hasComments(e) || subtreeHasComments(e.key) || subtreeHasComments(e.value);

const allMappings = (row) => row.items.length > 0 && row.items.every((item) => item.kind === "mapping");

function fitsInline(ctx, e) {
  if (e.value.kind === "mapping" && e.value.items.length === 0) return false;
  if (commented(e)) return false;
  return fig.byteLength(spellKey(keyText(e.key))) + 3 + fig.byteLength(inlineValue(e.value)) <= ctx.width;
}

function classify(ctx, e) {
  const v = e.value;
  if (v.kind === "mapping") return fitsInline(ctx, e) ? "inline" : "section";
  if (v.kind === "sequence") return allMappings(v) ? "aot" : "inline";
  return "inline";
}

function writeValue(ctx, w, value, col) {
  if (value.kind === "sequence" && ctx.pretty && value.items.length > 0) {
    const text = inlineValue(value);
    if (col + fig.byteLength(text) > ctx.width) {
      w.put("[\n");
      for (const item of value.items) w.put(ctx.unit, inlineValue(item), ",\n");
      w.put("]");
      return;
    }
    w.put(text);
    return;
  }
  w.put(inlineValue(value));
}

function kvLine(ctx, w, e) {
  w.comments(e.key.leading, "#");
  const key = spellKey(keyText(e.key));
  w.put(key, " = ");
  writeValue(ctx, w, e.value, fig.byteLength(key) + 3);
  const t = e.value.trailing[0];
  if (t) {
    w.put(" #");
    if (t.text !== "") w.put(" ", t.text.replace(/\n/g, " "));
  }
  w.put("\n");
  ctx.wrote = true;
}

function dottedBody(ctx, w, prefix, map) {
  for (const e of map.items) {
    const path = [...prefix, keyText(e.key)];
    if (e.value.kind === "mapping" && e.value.items.length > 0) {
      dottedBody(ctx, w, path, e.value);
      continue;
    }
    const spelled = spellPath(path);
    w.put(spelled, " = ");
    writeValue(ctx, w, e.value, fig.byteLength(spelled) + 3);
    w.put("\n");
    ctx.wrote = true;
  }
}

function needsHeader(ctx, map) {
  if (map.items.length === 0) return true;
  return map.items.some((e) => classify(ctx, e) === "inline");
}

function section(ctx, w, e, parent) {
  const path = [...parent, keyText(e.key)];
  if (needsHeader(ctx, e.value)) {
    if (ctx.wrote) w.put("\n");
    w.comments(e.key.leading, "#");
    w.put("[", spellPath(path), "]\n");
    ctx.wrote = true;
  }
  body(ctx, w, e.value, path);
}

function aot(ctx, w, e, parent) {
  const path = [...parent, keyText(e.key)];
  for (const elem of e.value.items) {
    if (ctx.wrote) w.put("\n");
    w.put("[[", spellPath(path), "]]\n");
    ctx.wrote = true;
    body(ctx, w, elem, path);
  }
}

function body(ctx, w, map, path) {
  let lastInline = -1;
  map.items.forEach((e, i) => {
    if (classify(ctx, e) === "inline") lastInline = i;
  });
  // Pass 1: the lines, in document order, up to the last inline child.
  for (let i = 0; i <= lastInline; i++) {
    const e = map.items[i];
    const cls = classify(ctx, e);
    if (cls === "inline") kvLine(ctx, w, e);
    else if (cls === "section") {
      if (!commented(e)) dottedBody(ctx, w, [keyText(e.key)], e.value);
    } else if (!commented(e)) kvLine(ctx, w, e);
  }
  if (w.comments(map.dangling, "#")) ctx.wrote = true;
  // Pass 2: the sections, after the lines; a commented one that could not
  // demote comes here from the middle.
  map.items.forEach((e, i) => {
    const demoted = i <= lastInline && !commented(e);
    const cls = classify(ctx, e);
    if (cls === "section") {
      if (!demoted) section(ctx, w, e, path);
    } else if (cls === "aot") {
      if (!demoted) aot(ctx, w, e, path);
    }
  });
}

function print(_dialect, t, options) {
  options ??= {};
  fig.index(t);
  const w = fig.writer(options);
  const ctx = { width: options.width ?? 80, pretty: options.pretty !== false, unit: " ".repeat(options.indent ?? 2), wrote: false };
  const root = t.byid(0);
  // Splice text is what follows `key = `, which holds only an inline value:
  // a table is an inline table there, never the `[header]` sections a
  // document gives it — the compiled `printSplice`.
  if (options.splice === true) return inlineValue(root);
  if (root.kind === "mapping") body(ctx, w, root, []);
  // A non-table root has no TOML document form; the inline value is a
  // best-effort fragment.
  else w.put(inlineValue(root), "\n");
  return w.string();
}

// ── the language ──────────────────────────────────────────────────────────

/** @type {import("@diaryx/fig/helper").Language} */
const language = {
  name: "toml",
  caps: { read: true, edit: true, serialize: true },
  // TOML holds its four datetimes and inf/nan natively; nothing else is
  // native, so a null or an enum literal rides in a `$fig` envelope.
  lossless: { offset_datetime: true, local_datetime: true, local_date: true, local_time: true, number_special: true },
  syntax: {
    comments: { style: "hash", line: { open: "#" }, trailing: { open: "#" } },
    kv_sep: " = ",
    flow_map_pad: " ",
    key_style: "bare_or_quoted",
    empty_map_literal: "{}",
    block_seq_editable: false,
    section_noun: "table",
    section_header: { open: "[", close: "]", seq_open: "[[", seq_close: "]]" },
  },
  // The compiled format's extension: where the format is compiled out
  // and this stands in for it, the extension reaches it.
  dialects: [{ name: "toml", extensions: ["toml"], splice: "literal", empty_doc_seed: "" }],
  samples: ['a = 1\nb = [1, 2]\n\n[s]\nk = "v"\nt = { x = 1 }\n'],
  renderers: [],
  parse,
  print,
};

export default language;
