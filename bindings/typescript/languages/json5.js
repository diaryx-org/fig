// JSONC and JSON5, in JavaScript: the twins of fig's compiled `jsonc` and
// `json5` dialects — one module, two dialects, as the compiled JSON module
// is one parser with a dialect switch.
//
// The contract is the format and the tree, not the compiled parser's
// shape: this module's node table is held row for row against the compiled
// dialects' on every fixture under `tests/fixtures/jsonc/` and
// `tests/fixtures/json5/` — every row, span, text and comment — and every
// document the compiled dialects refuse this refuses too, in words and at
// offsets of its own. `fig lang table -i jsonc` / `-i json5` prints the
// table a document must give.
//
// The format:
//
//   * JSONC is JSON with `//` and `/* */` comments; JSON5 adds unquoted
//     `identifier` keys (and `true`/`false`/`null` as keys), single-quoted
//     strings, the escapes `\x`, `\v`, `\0`, `\'`, any other `\c` as `c`,
//     a `\` before a line ending as a continuation, trailing commas,
//     `Infinity`/`NaN` (signed too) as `number_special` strings, `0x`
//     integers, `.5`, `5.`, `+1`, and `\v`/`\f` as whitespace;
//   * a container spans its brackets, an entry its key through its value;
//     a number keeps its lexeme and is a float when it has a `.` or an
//     exponent (a `0x` integer never); a duplicate key keeps both entries;
//   * a comment that is the last thing on the line of the value just
//     finished trails it — or dangles on a container whose closing line it
//     is; one on the line a container opens, with nothing else after it,
//     is that container's trailing comment; any other waits and leads the
//     next key, value or container, or dangles on the container that
//     closes with it still waiting; what waits after the document is
//     dropped. A comment's text is trimmed; a block comment's style is
//     `block`.
//
// That is the grammar module's own binding with `endsLine`, `one` and
// `closingLine` set, and two things it cannot express: a container claims
// what waits and what rides its opening line before it exists, which the
// `opener`/`container` pair below does; and "the last thing on its line"
// counts a following `,` or closer as nothing, so `[1 /* one */, 2]`
// trails `1`, which `endsLine` below widens the context's `restIsBlank`
// to say.
//
// Spans are byte offsets, 0-based, `[start, end)`; a container's span
// includes its brackets and a string's its quotes. This is the same object
// `@diaryx/fig`'s `registerLanguage` takes, so it serves the browser and
// Node unchanged.
import * as fig from "@diaryx/fig/kit";
import * as G from "@diaryx/fig/kit/grammar";
import * as N from "@diaryx/fig/kit/number";

// ── errors ────────────────────────────────────────────────────────────────

const MESSAGES = {
  value: "expected a value here: an object, an array, a string, a number, `true`, `false` or `null`",
  entry: "expected a `key: value` entry or `}` here",
  item: "expected a value or `]` here",
  colon: "expected `:` after this key",
  comma: "expected `,` between elements",
  trailingComma: "a trailing comma is not allowed here; JSONC follows strict JSON — drop it, or use a .json5 file",
  unclosedObject: "unclosed object; expected `}` before the end of the input",
  unclosedArray: "unclosed array; expected `]` before the end of the input",
  unclosedString: "unclosed string; add the closing quote, or escape the line ending as `\\n`",
  unclosedComment: "unclosed block comment; add the closing `*/`",
  control: "a raw control character cannot stand inside a string; escape it (`\\n`, `\\t`, `\\u0000`)",
  leadingZero: "a number cannot have a leading zero; write the digits without the padding, or quote it as a string to keep the padding (e.g. a zip code)",
  digits: "expected the digits of a number here",
  hexDigits: "expected hex digits after `0x`",
  exponent: "expected the digits of the exponent here",
  badU: "invalid `\\u` escape; it needs exactly 4 hex digits (e.g. `\\u00e9`)",
  badX: "invalid `\\x` escape; it needs exactly 2 hex digits (e.g. `\\x41`)",
  empty: "no document here; expected a value",
  trailing: "unexpected content after the document; a JSON document is one value",
};

const badEscape = (e) => "invalid escape `\\" + e + "` in a string; JSON reads `\\\" \\\\ \\/ \\b \\f \\n \\r \\t \\uXXXX`";
const notAValue = (w) => "`" + w + "` is not a value; quote it to mean the string";

// ── lexical ───────────────────────────────────────────────────────────────

const IDENT = /[A-Za-z_$][A-Za-z0-9_$]*/;
const HEX2 = /^[0-9a-fA-F]{2}$/;
const HEX4 = /^[0-9a-fA-F]{4}$/;
const SIMPLE_ESCAPES = { '"': 34, "\\": 92, "/": 47, b: 8, f: 12, n: 10, r: 13, t: 9 };

const isDigit = (c) => c !== undefined && c >= 48 && c <= 57;

const comment = G.choice([
  G.comment("//"),
  G.comment({ open: "/*", close: "*/", unclosed: MESSAGES.unclosedComment }),
]);

// Whether nothing but a `,` or a closer stands between `at` and the end of
// the line: what these dialects count as the end of a line, so that a
// comment before a comma still trails what the comma follows.
function endsLine(sc, at) {
  const { bin } = sc;
  let i = at;
  while (i < bin.length && (bin[i] === " " || bin[i] === "\t")) i += 1;
  if (i >= bin.length) return true;
  const c = bin[i];
  return c === "\n" || c === "\r" || c === "," || c === "]" || c === "}";
}

// ── strings ───────────────────────────────────────────────────────────────

// The text of a string's inside, one char per byte. Decoded by hand rather
// than with `G.quoted`, because `\u` is not a fixed replacement: a
// surrogate pair is one character, and a lone surrogate is no character at
// all — the string is then kept as written, every escape in it undecoded.
function decode(sc, inner, json5, at) {
  if (!inner.includes("\\")) return fig.fromBin(inner);
  const fail = (message) => sc.fail(message, at);
  const out = [];
  let i = 0;
  while (i < inner.length) {
    const c = inner.charCodeAt(i);
    if (c !== 92) {
      out.push(c);
      i += 1;
      continue;
    }
    const e = inner[i + 1];
    i += 2;
    if (e !== undefined && Object.hasOwn(SIMPLE_ESCAPES, e)) {
      out.push(SIMPLE_ESCAPES[e]);
    } else if (e === "u") {
      const hex = inner.slice(i, i + 4);
      if (!HEX4.test(hex)) fail(MESSAGES.badU);
      let cp = parseInt(hex, 16);
      i += 4;
      if (fig.isSurrogate(cp)) {
        let pair = null;
        if (inner.slice(i, i + 2) === "\\u") {
          const lo = inner.slice(i + 2, i + 6);
          if (!HEX4.test(lo)) fail(MESSAGES.badU);
          pair = fig.surrogatePair(cp, parseInt(lo, 16));
        }
        if (pair === null) return fig.fromBin(inner);
        cp = pair;
        i += 6;
      }
      out.push(...fig.utf8Bytes(cp));
    } else if (!json5 || e === undefined) {
      fail(badEscape(e ?? ""));
    } else if (e === "x") {
      const hex = inner.slice(i, i + 2);
      if (!HEX2.test(hex)) fail(MESSAGES.badX);
      out.push(...fig.utf8Bytes(parseInt(hex, 16)));
      i += 2;
    } else if (e === "v") {
      out.push(11);
    } else if (e === "0") {
      out.push(0);
    } else if (e === "\n") {
      // A `\` before a line ending is a continuation: neither is in the text.
    } else if (e === "\r") {
      if (inner[i] === "\n") i += 1;
    } else {
      out.push(inner.charCodeAt(i - 1));
    }
  }
  return fig.textOf(new Uint8Array(out));
}

// A string: `"…"`, and in JSON5 `'…'`. A raw control character inside, a
// line ending among them, is a refusal rather than a byte.
function stringRule(json5) {
  return (sc) => {
    const s = sc.pos;
    const q = sc.byte();
    if (!(q === 34 || (json5 && q === 39))) return null;
    const { bin, n } = sc;
    let i = s + 1;
    for (;;) {
      if (i >= n) sc.fail(MESSAGES.unclosedString, s);
      const c = bin.charCodeAt(i);
      if (c === q) {
        i += 1;
        break;
      } else if (c === 92) {
        if (i + 1 >= n) sc.fail(MESSAGES.unclosedString, s);
        i += bin[i + 1] === "\r" && bin[i + 2] === "\n" ? 3 : 2;
      } else if (c < 32) {
        sc.fail(MESSAGES.control, i);
      } else {
        i += 1;
      }
    }
    sc.pos = i;
    return fig.scalar("string", [s, i], decode(sc, sc.binSlice(s + 1, i - 1), json5, s));
  };
}

// ── numbers and words ─────────────────────────────────────────────────────

// The kind a lexeme implies: a float when it has a `.` or an exponent, an
// int otherwise — a `0x` integer never, whatever its digits look like.
function numberKind(raw) {
  if (/^[+-]?0[xX]/.test(raw)) return "int";
  return /[.eE]/.test(raw) ? "float" : "int";
}

// A number, the lexeme kept verbatim as the row's text: JSON's
// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`, and for JSON5 an
// ES5.1 numeric literal — a `+` too, `Infinity` and `NaN` (no number the
// wire can hold, so they ride as `number_special` strings), a `0x`
// integer, and either side of the point left out.
function numberRule(json5) {
  return (sc) => {
    const s = sc.pos;
    let c = sc.byte();
    // `I` and `N` open `Infinity` and `NaN`; any other word the rule meets
    // it hands back, for `word` below to judge.
    if (!(isDigit(c) || c === 45 || (json5 && (c === 43 || c === 46 || c === 73 || c === 78)))) return null;
    if (c === 45 || c === 43) {
      sc.advance();
      c = sc.byte();
    }
    if (json5) {
      if (sc.match(/(?:Infinity|NaN)(?![A-Za-z0-9_$])/) !== null) {
        return fig.scalar("string", [s, sc.pos], sc.slice(s, sc.pos), { ext_kind: "number_special" });
      }
      if (!(isDigit(c) || c === 46)) {
        sc.pos = s;
        return null;
      }
    }
    if (json5 && sc.match(/0[xX]/) !== null) {
      if (sc.match(/[0-9a-fA-F]+/) === null) sc.fail(MESSAGES.hexDigits);
    } else {
      const ds = sc.pos;
      let seen = sc.match(/[0-9]+/) !== null;
      if (seen && sc.bin[ds] === "0" && sc.pos > ds + 1) sc.fail(MESSAGES.leadingZero, ds);
      if (!json5 && !seen) sc.fail(MESSAGES.digits);
      if (sc.byte() === 46) {
        sc.advance();
        const frac = sc.match(/[0-9]+/) !== null;
        if (!json5 && !frac) sc.fail(MESSAGES.digits);
        seen = seen || frac;
      }
      if (!seen) sc.fail(MESSAGES.digits, s);
      const e = sc.byte();
      if (e === 101 || e === 69) {
        sc.advance();
        const sign = sc.byte();
        if (sign === 43 || sign === 45) sc.advance();
        if (sc.match(/[0-9]+/) === null) sc.fail(MESSAGES.exponent);
      }
    }
    const raw = sc.slice(s, sc.pos);
    return fig.scalar(numberKind(raw), [s, sc.pos], raw);
  };
}

// `null`, `true`, `false`, whole words; any other word is a value only
// once it is quoted. (In JSON5 `Infinity` and `NaN` are numbers, and the
// number rule has taken them before this one is tried.)
function word(sc) {
  const m = sc.match(IDENT);
  if (m === null) return null;
  const [s, e] = m;
  const w = sc.slice(s, e);
  if (w === "null") return fig.scalar("null", [s, e], undefined);
  if (w === "true" || w === "false") return fig.scalar("bool", [s, e], w);
  sc.fail(notAValue(w), s);
}

// ── the dialect ───────────────────────────────────────────────────────────
// One set of rules per dialect: the containers are `G.map` and
// `G.sequence`, with the opener, the separator and the key and value rules
// doing what this binding asks that the module's defaults do not.

function build(json5) {
  const stringValue = stringRule(json5);
  const number = numberRule(json5);
  const trivia = G.trivia({ space: G.pat(json5 ? /[ \t\r\n\v\f]+/ : /[ \t\r\n]+/), comment });

  let value;

  // A container's opener: the delimiter, and — when nothing before it can
  // claim it — a comment on its line that ends the line, which the
  // container takes as its trailing comment once it exists. What was
  // waiting when it opened is its leading comments. Both ride a stack,
  // because containers nest, and neither can go through the context: the
  // node does not exist until the container closes.
  const opener = (delim) => (sc, ctx) => {
    const s = sc.pos;
    if (sc.lit(delim) === null) return null;
    const head = { leading: ctx.pending };
    ctx.pending = [];
    const p = sc.pos;
    G.hs(sc);
    const at = sc.pos;
    const c = comment(sc);
    const claimed = ctx.last != null && sc.sameLine(ctx.last.span[1], at);
    if (c != null && !claimed && endsLine(sc, c[1])) head.trailing = c;
    else sc.pos = p;
    ctx.heads.push(head);
    return [s, sc.pos];
  };

  const container = (rule) => (sc, ctx) => {
    const n = rule(sc, ctx);
    if (n == null) return null;
    const head = ctx.heads.pop();
    for (const c of head.leading) n.comment("leading", c.text, c.style);
    if (head.trailing) n.comment("trailing", head.trailing.text, head.trailing.style);
    return n;
  };

  // After an element: a `,`, or the close ahead. JSON5 takes a trailing
  // comma before the close; JSONC refuses one where the next element
  // should be.
  const separator = (close) => (sc, ctx) => {
    trivia(sc, ctx);
    if (sc.lit(",")) {
      if (!json5) {
        const at = sc.pos - 1;
        trivia(sc, ctx);
        if (close(sc)) sc.fail(MESSAGES.trailingComma, at);
      }
      return;
    }
    const p = sc.pos;
    if (close(sc)) {
      sc.pos = p;
      return;
    }
    if (!sc.eof()) sc.fail(MESSAGES.comma);
  };

  // A key claims what is waiting as soon as it is read, so that a comment
  // between the `:` and the value is the value's and not the key's; a
  // value claims what is waiting for the same reason.
  const claims = (rule) => (sc, ctx) => {
    const n = rule(sc, ctx);
    if (n != null) ctx.flush(n, "leading");
    return n;
  };

  // A JSON key is a quoted string; JSON5 takes a bare identifier too, the
  // keywords among them (`{ true: 1, null: 2 }`).
  const key = claims(json5 ? G.choice([stringValue, G.key(G.pat(IDENT))]) : stringValue);

  const mapping = container(
    G.map({
      open: opener("{"),
      close: G.lit("}"),
      trivia,
      entry: G.entry({
        key,
        between: trivia,
        sep: G.lit(":"),
        value: claims((sc, ctx) => value(sc, ctx)),
        missingSep: MESSAGES.colon,
        missingValue: MESSAGES.value,
      }),
      after: separator(G.lit("}")),
      duplicates: "keep",
      expected: MESSAGES.entry,
      unclosed: MESSAGES.unclosedObject,
    }),
  );

  const sequence = container(
    G.sequence({
      open: opener("["),
      close: G.lit("]"),
      trivia,
      item: (sc, ctx) => value(sc, ctx),
      after: separator(G.lit("]")),
      expected: MESSAGES.item,
      unclosed: MESSAGES.unclosedArray,
    }),
  );

  value = G.choice([mapping, sequence, stringValue, number, word]);

  // The document, by hand rather than `G.document`: what is still waiting
  // when it ends is dropped, where `document` would dangle it on the root.
  return (input) => {
    const sc = fig.scanner(input);
    if (sc.bytes[0] === 0xef && sc.bytes[1] === 0xbb && sc.bytes[2] === 0xbf) sc.pos = 3;
    const ctx = G.context(sc, { endsLine: true, one: true, closingLine: true });
    ctx.restIsBlank = (at) => endsLine(sc, at);
    ctx.heads = [];
    trivia(sc, ctx);
    const root = value(sc, ctx);
    if (root == null) sc.fail(sc.eof() ? MESSAGES.empty : MESSAGES.value);
    ctx.flush(root, "leading");
    ctx.last = root;
    trivia(sc, ctx);
    if (!sc.eof()) sc.fail(MESSAGES.trailing);
    return fig.rows(root);
  };
}

const DIALECTS = { json5: build(true), jsonc: build(false) };

function parse(dialect, input) {
  return DIALECTS[dialect](input);
}

// ── the printer ───────────────────────────────────────────────────────────
// The compiled printer's layout, as `json.mjs` has it — one element per
// line at `indent` spaces a level when `pretty`, nothing but the tokens
// otherwise — plus what the dialects add: comments (leading above a
// member, trailing after its comma, a container's own after its opener,
// dangling before its close; `pretty` only), and in JSON5 a bare
// identifier key, `Infinity`/`NaN` as written, and the number spellings
// JSON5 reads (`0xC8`, `.5`, `+1`). A number no JSON can read is written
// as decimal, which `fig/number` is the rule for.

const QUOTE = { '"': '\\"', "\\": "\\\\", "\b": "\\b", "\f": "\\f", "\n": "\\n", "\r": "\\r", "\t": "\\t" };

function quote(s) {
  return '"' + s.replace(/[\x00-\x1f"\\]/g, (ch) => QUOTE[ch] ?? "\\u" + ch.charCodeAt(0).toString(16).padStart(4, "0")) + '"';
}

const isBareIdentifier = (name) => /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name);

const isContainer = (row) => row.kind === "mapping" || row.kind === "sequence";

class Printer {
  constructor(w, json5) {
    this.w = w;
    this.json5 = json5;
    this.spelling = json5 ? N.JSON5 : N.JSON;
  }

  commentsOn() {
    return this.w.pretty;
  }

  commentText(c) {
    if (c.style === "block") return c.text === "" ? "/**/" : "/* " + c.text + " */";
    return c.text === "" ? "//" : "// " + c.text;
  }

  leading(row, depth) {
    if (!this.commentsOn()) return;
    for (const c of row.leading) this.w.indent(depth).put(this.commentText(c), "\n");
  }

  trailing(row) {
    if (!this.commentsOn()) return;
    const c = row.trailing[0];
    if (c) this.w.put(" ", this.commentText(c));
  }

  key(row) {
    const { w } = this;
    const k = row.kind;
    if (k === "string") {
      if (this.json5 && !row.ext_kind && isBareIdentifier(row.text ?? "")) w.put(row.text);
      else w.put(quote(row.text ?? ""));
    } else if (k === "null") {
      w.put(quote("null"));
    } else if (k === "bool" || k === "int" || k === "float") {
      w.put(quote(row.text ?? ""));
    } else if (k === "alias") {
      throw new Error("an alias must be resolved before it is written as JSON");
    } else {
      throw new Error("a JSON key must be a string");
    }
  }

  node(row, depth) {
    const { w } = this;
    const k = row.kind;
    if (k === "null") w.put("null");
    else if (k === "bool") w.put(row.text);
    else if (k === "int" || k === "float") {
      if (row.ext_kind === "char_literal") w.put(row.text);
      else w.put(N.text(row.text ?? "", this.spelling));
    } else if (k === "string") {
      if (row.ext_kind === "number_special" && this.json5) w.put(row.text);
      else w.put(quote(row.text ?? ""));
    } else if (k === "sequence" || k === "mapping") {
      this.container(row, depth);
    } else if (k === "keyvalue") {
      this.key(row.key);
      w.put(w.pretty ? ": " : ":");
      this.node(row.value, depth);
    } else if (k === "alias") {
      throw new Error("an alias must be resolved before it is written as JSON");
    } else {
      throw new Error("a " + k + " is not a value");
    }
  }

  container(row, depth) {
    const { w } = this;
    const [open, close] = row.kind === "mapping" ? ["{", "}"] : ["[", "]"];
    const dangling = this.commentsOn() ? row.dangling : [];
    if (row.items.length === 0 && dangling.length === 0) {
      w.put(open, close);
      this.trailing(row);
      return;
    }
    w.put(open);
    this.trailing(row);
    w.nl();
    row.items.forEach((item, i) => {
      const lead = item.kind === "keyvalue" ? item.key : item;
      const trail = item.kind === "keyvalue" ? item.value : item;
      this.leading(lead, depth + 1);
      w.indent(depth + 1);
      this.node(item, depth + 1);
      if (i < row.items.length - 1) w.put(",");
      if (!isContainer(trail)) this.trailing(trail);
      w.nl();
    });
    for (const c of dangling) w.indent(depth + 1).put(this.commentText(c), "\n");
    w.indent(depth).put(close);
  }
}

function print(dialect, t, options) {
  const w = fig.writer(options);
  const p = new Printer(w, dialect === "json5");
  const root = fig.index(t).byid(0);
  p.leading(root, 0);
  p.node(root, 0);
  if (!isContainer(root)) p.trailing(root);
  w.put("\n");
  return w.string();
}

// ── the language ──────────────────────────────────────────────────────────

/** @type {import("@diaryx/fig/helper").Language} */
const language = {
  name: "json5",
  caps: { read: true, edit: true, serialize: true },
  // Beyond the core kinds, `null` and nothing else: every extended scalar
  // rides in a `$fig` envelope, JSON5's own `Infinity`/`NaN` included.
  lossless: { null: true },
  syntax: {
    comments: { style: "slashes", line: { open: "//" }, trailing: { open: "//" } },
    kv_sep: ": ",
    key_style: "json_quoted",
    empty_map_literal: "{}",
  },
  // The language's own name goes on its first dialect; each takes its
  // compiled dialect's extension, which reaches it where that dialect is
  // compiled out and this stands in for it.
  dialects: [
    { name: "json5", extensions: ["json5"], splice: "json_string", empty_doc_seed: "{}\n" },
    { name: "jsonc", extensions: ["jsonc"], splice: "json_string", empty_doc_seed: "{}\n" },
  ],
  samples: ['{"a": 1, "b": [true, null, "s"], "c": {"d": 2.5}}\n', "// top\n[] // empty\n"],
  renderers: [],
  parse,
  print,
};

export default language;
