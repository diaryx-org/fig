// ZON, in JavaScript: the twin of fig's compiled `zon` format, row for row.
//
// ZON is a data format, and this module reads it as one. The compiled
// format hands its source to `std.zig.Ast.parse(.zon)` and walks the tree
// Zig's own parser built, but what the two sides are held to is not that
// parser: it is the node table — every fixture in `tests/fixtures/zon`,
// row for row, comments and spans included — and the printer's bytes with
// it. So what is written below is ZON's own grammar, small, rather than as
// much of Zig as decides what ZON is.
//
//   value     ::= '.{' container '}' | scalar | '(' value ')'
//   container ::= [ field (',' field)* [','] ] | [ value (',' value)* [','] ]
//   field     ::= '.' name '=' value
//   name      ::= ident | '@"' … '"'
//   scalar    ::= number | '"' … '"' | '\\' line… | "'" char "'" | '.' name
//               | 'true' | 'false' | 'null' | 'inf' | 'nan'
//
// `.{ .a = 1 }` is a mapping, `.{ 1, 2 }` a sequence, `.{}` an empty
// mapping; a container spans `.{` through `}`, an entry its field name
// (without the dot) through its value. A number keeps its lexeme verbatim
// — `0xFF`, `1_000`, `- 1` with the space, since `-` is a prefix operator
// — and is a float when it has a `.` or its radix's exponent; `inf`,
// `-inf` and `nan` are floats. A string decodes Zig's escapes, and a `\\`
// multiline string joins its lines with `\n`, spanning the first `\\` to
// the last line's end. `.name` is an `enum_literal` string, `'c'` a
// `char_literal` int whose text is the decimal codepoint, and a `.@"…"`
// name decodes as a string. Parentheses group and leave no trace: the node
// is the one inside them, and a comment in there is dropped, as the
// compiled walk drops it.
//
// Comments are `//` to the end of the line, bound by the grammar module's
// policy with `closingLine`: one on the line a value ends is that value's
// trailing comment — unless the value is a container spanning lines, which
// takes it as its last dangling one; one on the line a `.{` opens is that
// container's trailing comment; anything else waits for the next field
// name or element and leads it, or dangles on the container that closes
// over it. Two places hold nothing for a comment to belong to, and drop
// it: between a field name and its value, and below the document.
//
// Everything else is a refusal, and the refusals are the format's, worded
// here: an operator, a bare identifier, a `@builtin`, a `[N]T{…}`, a
// keyword-led expression, a `///` doc comment, a `/* … */`, a second
// document. The compiled format says `InvalidZon` or `UnsupportedZon` with
// no offset at all; this says what was wrong and where, which is each
// side's own business.
//
// Every offset is a byte offset.
import * as fig from "@diaryx/fig/kit";
import * as G from "@diaryx/fig/kit/grammar";

// ── bytes ─────────────────────────────────────────────────────────────────
// The lexical rules read bytes by offset rather than moving the scanner, so
// that a literal is measured before the cursor commits to it.

const IDENT = /[A-Za-z_][A-Za-z0-9_]*/;

const byteAt = (sc, i) => (i >= 0 && i < sc.n ? sc.bytes[i] : -1);
const chr = (b) => String.fromCharCode(b);
const isDigit = (b) => b >= 48 && b <= 57;
const isHex = (b) => isDigit(b) || (b >= 65 && b <= 70) || (b >= 97 && b <= 102);
const isIdentStart = (b) => b === 95 || (b >= 65 && b <= 90) || (b >= 97 && b <= 122);
// What a string or a character literal may not hold: a newline is its own
// refusal, and every other control byte has an escape instead.
const isControl = (b) => b < 32 || b === 127;
// The bytes of a codepoint, and what a decoder's buffer spells:
// JavaScript's strings are UTF-16, so a buffer built byte by byte is text
// only once it is decoded.
const utf8 = (cp) => String.fromCharCode(...fig.utf8Bytes(cp));
const fromBytes = fig.fromBin;

// ── comments ──────────────────────────────────────────────────────────────

const slashes = G.comment("//");

// `//` to the end of the line. `///` and `//!` are Zig's doc comments, and
// a ZON document is data: there is nothing in it for them to document.
function lineComment(sc) {
  const s = sc.pos;
  if (byteAt(sc, s) !== 47 || byteAt(sc, s + 1) !== 47) return null;
  const c = byteAt(sc, s + 2);
  if (c === 33 || (c === 47 && byteAt(sc, s + 3) !== 47)) {
    sc.fail("a `" + sc.slice(s, s + 3) + "` doc comment is not ZON; a ZON comment is `//`", s);
  }
  return slashes(sc);
}

const trivia = G.trivia({ space: G.ws1, comment: lineComment });

// ── escapes ───────────────────────────────────────────────────────────────

const SIMPLE = { 110: 10, 114: 13, 116: 9, 92: 92, 39: 39, 34: 34 };
const BRACES = "`\\u` takes a codepoint in braces, as `\\u{1F600}`";

// The escape at `i` (a `\`): the bytes it stands for, the codepoint it
// spells, and where it ends. Zig's set is `\n \r \t \\ \' \"`, `\xNN` for a
// raw byte, and `\u{…}` for a codepoint — which a string may not spell as a
// surrogate, no text holding one, and a character literal may.
function escapeAt(sc, i, inChar) {
  const c = byteAt(sc, i + 1);
  const simple = SIMPLE[c];
  if (simple !== undefined) return [chr(simple), simple, i + 2];
  if (c === 120) {
    const hex = sc.slice(i + 2, i + 4);
    if (!/^[0-9a-fA-F]{2}$/.test(hex)) sc.fail("`\\x` takes two hex digits, as `\\x41`", i);
    const b = parseInt(hex, 16);
    return [chr(b), b, i + 4];
  }
  if (c === 117) {
    if (byteAt(sc, i + 2) !== 123) sc.fail(BRACES, i);
    let j = i + 3;
    while (isHex(byteAt(sc, j))) j += 1;
    if (j === i + 3 || byteAt(sc, j) !== 125) sc.fail(BRACES, i);
    const cp = parseInt(sc.slice(i + 3, j), 16);
    if (cp > 0x10ffff) sc.fail("`\\u{…}` is past the last codepoint, `10FFFF`", i);
    if (!inChar && fig.isSurrogate(cp)) sc.fail("`\\u{…}` is a surrogate, which no text holds", i);
    return [utf8(cp), cp, j + 1];
  }
  sc.fail("`\\" + (c >= 32 && c < 127 ? chr(c) : "") + "` is not a ZON escape", i);
}

// ── scalars ───────────────────────────────────────────────────────────────

// `"…"`: Zig's escapes, and one line. A tab, or any other control byte,
// has an escape and is not itself a string character.
function stringLiteral(sc) {
  if (byteAt(sc, sc.pos) !== 34) return null;
  const s = sc.pos;
  let i = s + 1;
  let out = "";
  for (;;) {
    const c = byteAt(sc, i);
    if (c === 34) {
      i += 1;
      break;
    }
    if (c < 0 || c === 10) sc.fail('unclosed string; expected a closing `"` on this line', s);
    if (isControl(c)) sc.fail("a control byte is not a ZON string character; write it as an escape", i);
    if (c === 92) {
      const [bytes, , next] = escapeAt(sc, i, false);
      out += bytes;
      i = next;
    } else {
      out += chr(c);
      i += 1;
    }
  }
  sc.pos = i;
  return fig.scalar("string", [s, i], fromBytes(out));
}

// `'c'`: exactly one character, escaped or not, as the decimal codepoint it
// spells — a `char_literal` int, the kind the wire keeps it in.
function charLiteral(sc) {
  if (byteAt(sc, sc.pos) !== 39) return null;
  const s = sc.pos;
  let cp;
  let i;
  if (byteAt(sc, s + 1) === 92) {
    [, cp, i] = escapeAt(sc, s + 1, true);
  } else {
    i = s + 1;
    while (byteAt(sc, i) >= 0 && byteAt(sc, i) !== 39 && byteAt(sc, i) !== 10) i += 1;
    const inner = sc.slice(s + 1, i);
    if ([...inner].length !== 1) sc.fail("a `'…'` character literal holds one character", s);
    cp = inner.codePointAt(0);
  }
  if (byteAt(sc, i) !== 39) sc.fail("a `'…'` character literal holds one character", s);
  sc.pos = i + 1;
  return fig.scalar("int", [s, sc.pos], String(cp), { ext_kind: "char_literal" });
}

// `\\line`, one or more: each line's text after its `\\`, joined with a
// newline. The span runs from the first `\\` to the end of the last line,
// the line ending itself left out.
function multiline(sc) {
  if (byteAt(sc, sc.pos) !== 92 || byteAt(sc, sc.pos + 1) !== 92) return null;
  const s = sc.pos;
  const lines = [];
  let end = s;
  for (;;) {
    const from = sc.pos + 2;
    let i = from;
    while (byteAt(sc, i) >= 0 && byteAt(sc, i) !== 10) i += 1;
    end = byteAt(sc, i - 1) === 13 ? i - 1 : i;
    lines.push(sc.slice(from, end));
    sc.pos = i;
    const after = sc.pos;
    G.ws(sc);
    if (byteAt(sc, sc.pos) === 92 && byteAt(sc, sc.pos + 1) === 92) continue;
    sc.pos = after;
    return fig.scalar("string", [s, end], lines.join("\n"));
  }
}

// A Zig number lexeme: a digit, then digits, letters and `_`, with a `.`
// only before another of those and a sign only after an exponent letter.
// The text is the lexeme as written, the sign and its space included.
function numberValue(sc, s) {
  sc.advance();
  for (;;) {
    const c = byteAt(sc, sc.pos);
    if (isDigit(c) || isIdentStart(c)) {
      sc.advance();
      const sign = byteAt(sc, sc.pos);
      if ((c === 101 || c === 69 || c === 112 || c === 80) && (sign === 43 || sign === 45)) sc.advance();
    } else if (c === 46 && (isDigit(byteAt(sc, sc.pos + 1)) || isIdentStart(byteAt(sc, sc.pos + 1)))) {
      sc.advance();
    } else {
      break;
    }
  }
  const raw = sc.slice(s, sc.pos);
  const body = raw.replace(/^-\s*/, "");
  const exponent = /^0[xX]/.test(body) ? /[pP]/ : /[eE]/;
  const float = raw.includes(".") || exponent.test(body);
  return fig.scalar(float ? "float" : "int", [s, sc.pos], raw);
}

// `-` negates a literal: a number, or `inf`. ZON has no other prefix, and
// no operator at all.
function negated(sc) {
  const s = sc.pos;
  sc.advance();
  G.ws(sc);
  if (isDigit(byteAt(sc, sc.pos))) return numberValue(sc, s);
  const m = sc.match(IDENT);
  // A negated `inf` is spelled `-inf` whatever the space between them: it
  // is a value with one name, not a lexeme.
  if (m !== null && sc.slice(m[0], m[1]) === "inf") return fig.scalar("float", [s, sc.pos], "-inf");
  sc.fail("`-` negates a number in ZON; ZON holds values, not expressions", s);
}

// `true`, `false`, `null`, `inf` and `nan` are ZON's words. A bare
// identifier — `foo`, `undefined`, `if` — is Zig, and Zig is not ZON.
function word(sc) {
  const [s, e] = sc.match(IDENT);
  const w = sc.slice(s, e);
  if (w === "true" || w === "false") return fig.scalar("bool", [s, e], w);
  if (w === "null") return fig.scalar("null", [s, e]);
  if (w === "inf" || w === "nan") return fig.scalar("float", [s, e], w);
  sc.fail("`" + w + "` is not a ZON value; ZON holds numbers, strings, `.name`, `true`, `false` and `null`", s);
}

// A name, the span the wire reports for a field or an enum literal — the
// `.` before it is not in the span: `foo`, or `@"any text"`.
function name(sc) {
  const s = sc.pos;
  if (byteAt(sc, s) === 64) {
    sc.advance();
    const q = stringLiteral(sc);
    if (q == null) sc.fail('expected a `"…"` name after `@`', s);
    return fig.scalar("string", [s, q.span[1]], q.text);
  }
  const m = sc.match(IDENT);
  return m === null ? null : fig.scalar("string", [m[0], m[1]], sc.slice(m[0], m[1]));
}

// ── containers ────────────────────────────────────────────────────────────

// A container's opener, `.{`, and a `//` comment on its line, which the
// container takes as its trailing comment once it exists — held on a stack
// until then, because containers nest. The context's `last` is cleared so
// that nothing outside catches a comment written inside.
function opener(sc, ctx) {
  const s = sc.pos;
  if (byteAt(sc, s) !== 46 || byteAt(sc, s + 1) !== 123) return null;
  sc.advance(2);
  const p = sc.pos;
  G.hs(sc);
  const c = lineComment(sc);
  if (c == null) sc.pos = p;
  ctx.heads.push(c ?? false);
  ctx.last = null;
  return [s, sc.pos];
}

// After a value: a comma, or the `}` that closes the container — a trailing
// comma is optional. What stands there instead says what the input meant.
const OPERATORS = "+-*/%<>!&|^~?:";

function separator(sc, ctx) {
  trivia(sc, ctx);
  if (sc.lit(",") !== null) return;
  const c = byteAt(sc, sc.pos);
  if (c === 125) return;
  if (c < 0) sc.fail("unclosed `.{`; expected `}` before the end of the input");
  if (c === 61) sc.fail("a `.name = value` field cannot follow a value; a ZON container holds fields or values, not both");
  if (OPERATORS.includes(chr(c))) sc.fail("ZON holds values, not expressions; `" + chr(c) + "` is not ZON");
  sc.fail("expected `,` or `}` after this value");
}

let value;

const mappingRule = G.map({
  open: opener,
  close: G.lit("}"),
  trivia,
  entry: G.entry({
    prefix: G.lit("."),
    // The comments waiting are the field name's, taken before the value is
    // read: a container value would otherwise give them to its first
    // member, which is what a comment written inside one means.
    key: (sc, ctx) => {
      const k = name(sc);
      if (k != null) ctx.flush(k, "leading");
      return k;
    },
    // A comment between a field name and its value belongs to neither and
    // is dropped, as the compiled walk drops it: the walk never looks at
    // the gap the `=` sits in.
    between: (sc) => trivia(sc, null),
    sep: G.lit("="),
    value: (sc, ctx) => value(sc, ctx),
    missingSep: "expected `=` after this field name",
    missingValue: "expected a value after `=`",
  }),
  after: separator,
  duplicates: "keep",
  expected: "expected a `.name = value` field or `}` here",
  unclosed: "unclosed `.{`; expected `}` before the end of the input",
});

const sequenceRule = G.sequence({
  open: opener,
  close: G.lit("}"),
  trivia,
  item: (sc, ctx) => value(sc, ctx),
  after: separator,
  expected: "expected a value or `}` here",
  unclosed: "unclosed `.{`; expected `}` before the end of the input",
});

// Whether the `.{` here holds fields rather than positional values: a
// `.name =` after it, past whatever trivia. An empty `.{}` is a mapping too
// — which is why `.{}` is the empty document's seed.
function holdsFields(sc) {
  const p = sc.pos;
  sc.advance(2);
  trivia(sc, null);
  let fields = byteAt(sc, sc.pos) === 125;
  if (!fields && byteAt(sc, sc.pos) === 46 && byteAt(sc, sc.pos + 1) !== 123) {
    sc.advance();
    if (name(sc) != null) {
      G.ws(sc);
      fields = byteAt(sc, sc.pos) === 61 && byteAt(sc, sc.pos + 1) !== 61;
    }
  }
  sc.pos = p;
  return fields;
}

function container(sc, ctx) {
  const built = (holdsFields(sc) ? mappingRule : sequenceRule)(sc, ctx);
  const head = ctx.heads.pop();
  if (head) built.comment("trailing", head.text, head.style);
  return built;
}

// ── values ────────────────────────────────────────────────────────────────

// What a byte that opens no value means. `null` where a closer stands, so
// that the container's own rule reports the shape it wanted.
function notAValue(sc, c) {
  if (c < 0 || c === 125 || c === 41 || c === 44) return null;
  if (c === 64) sc.fail("a `@builtin` is not ZON; ZON holds values, not code");
  if (c === 91 || c === 123) sc.fail("ZON writes every container as `.{ … }`; `" + chr(c) + "` is not ZON");
  if (c === 47 && byteAt(sc, sc.pos + 1) === 42) {
    sc.fail("`/* … */` is not a ZON comment; a ZON comment is `//` to the end of the line");
  }
  sc.fail("ZON holds values, not expressions; `" + chr(c) + "` is not ZON");
}

value = (sc, ctx) => {
  const c = byteAt(sc, sc.pos);
  if (c === 40) {
    // Parentheses group and are not in the tree: the node is the one
    // inside them, and a comment in there is dropped.
    sc.advance();
    trivia(sc, null);
    const inner = value(sc, ctx);
    if (inner == null) sc.fail("expected a value after `(`");
    trivia(sc, null);
    if (sc.lit(")") === null) sc.fail("expected `)` after this value");
    return inner;
  }
  if (c === 46) {
    if (byteAt(sc, sc.pos + 1) === 123) return container(sc, ctx);
    const s = sc.pos;
    sc.advance();
    const n = name(sc);
    if (n == null) sc.fail("expected a name after `.`", s);
    return fig.scalar("string", n.span, n.text, { ext_kind: "enum_literal" });
  }
  if (c === 45) return negated(sc);
  if (c === 34) return stringLiteral(sc);
  if (c === 39) return charLiteral(sc);
  if (c === 92) return multiline(sc);
  if (isDigit(c)) return numberValue(sc, sc.pos);
  if (isIdentStart(c)) return word(sc);
  return notAValue(sc, c);
};

// The document: one value, and nothing after it. Comments before a
// container root go to its first field name or element — the container's
// own rule flushes them — and a scalar root takes them as leading.
function parse(_dialect, input) {
  const sc = fig.scanner(input);
  const ctx = G.context(sc, { closingLine: true });
  ctx.heads = [];
  trivia(sc, ctx);
  const root = value(sc, ctx);
  if (root == null) sc.fail(sc.eof() ? "no document here; expected a value" : "expected a value here");
  ctx.flush(root, "leading");
  ctx.last = root;
  trivia(sc, ctx);
  if (!sc.eof()) sc.fail("unexpected content after the document; a ZON file holds one value");
  // What is left waiting is a comment written below the document, on a
  // line of its own, with nothing after it to lead: it is dropped, as the
  // compiled walk drops it.
  return fig.rows(root);
}

// ── the printer ───────────────────────────────────────────────────────────
// The compiled printer's layout: every container broken one member per line
// at four spaces a level with a trailing comma, `.{}` when empty; comments
// where they were bound — a container's own trailing comment after its
// `.{`, a member's leading above it, its trailing after the comma, its
// dangling before the `}`. Without `pretty`, everything inline and no
// comments at all.

// Zig's keywords, which a field name may not be spelled bare — `.@"error"`
// rather than `.error`. `true`, `false` and `null` are not among them.
const KEYWORDS = new Set(
  `addrspace align allowzero and anyframe anytype asm break callconv catch comptime const continue
  defer else enum errdefer error export extern fn for if inline noalias noinline nosuspend opaque or orelse
  packed pub resume return linksection struct suspend switch test threadlocal try union unreachable var
  volatile while`.split(/\s+/),
);
const BARE = /^[A-Za-z_][A-Za-z0-9_]*$/;
const ESCAPES = { '"': '\\"', "\\": "\\\\", "\n": "\\n", "\r": "\\r", "\t": "\\t" };
const CHAR_ESCAPES = { 39: "\\'", 92: "\\\\", 10: "\\n", 13: "\\r", 9: "\\t" };

const isContainer = (row) => row.kind === "mapping" || row.kind === "sequence";

// A string's characters as the compiled printer escapes them: the five it
// spells, any other control character as `\xNN`, everything else itself.
const escaped = (s) =>
  s.replace(/["\\\n\r\t\x00-\x1f\x7f]/g, (c) => ESCAPES[c] ?? "\\x" + c.charCodeAt(0).toString(16).padStart(2, "0"));

function writeName(w, name) {
  w.put(".");
  if (BARE.test(name) && !KEYWORDS.has(name)) w.put(name);
  else w.put('@"', escaped(name), '"');
}

// A character literal from the codepoint its text spells: the escapes it
// has, a printable ASCII character as itself, anything else as `\u{…}`.
function writeChar(w, text) {
  if (!/^\d+$/.test(text)) return w.put(text);
  const cp = Number(text);
  const printable = cp >= 0x20 && cp <= 0x7e ? chr(cp) : "\\u{" + cp.toString(16) + "}";
  w.put("'", CHAR_ESCAPES[cp] ?? printable, "'");
}

function writeTrailing(w, row) {
  const c = w.pretty ? row.trailing[0] : undefined;
  if (!c) return;
  w.put(" //");
  if (c.text !== "") w.put(" ", c.text.replace(/\n/g, " "));
}

function writeNode(w, row, depth) {
  const k = row.kind;
  if (k === "null") w.put("null");
  else if (k === "bool") w.put(row.text);
  else if (k === "int" && row.ext_kind === "char_literal") writeChar(w, row.text);
  else if (k === "int" || k === "float") w.put(row.text);
  else if (k === "string" && row.ext_kind === "enum_literal") writeName(w, row.text ?? "");
  else if (k === "string") w.put('"', escaped(row.text ?? ""), '"');
  else if (isContainer(row)) writeContainer(w, row, depth);
  else if (k === "keyvalue") {
    if (row.key.kind !== "string") throw new Error("a ZON field name must be a string");
    writeName(w, row.key.text ?? "");
    w.put(" = ");
    writeNode(w, row.value, depth);
  } else if (k === "alias") {
    throw new Error("an alias must be resolved before it is written as ZON");
  } else {
    throw new Error("a " + k + " is not a value");
  }
}

function writeContainer(w, row, depth) {
  const dangling = w.pretty ? row.dangling : [];
  if (row.items.length === 0 && dangling.length === 0) {
    w.put(".{}");
    writeTrailing(w, row);
    return;
  }
  if (!w.pretty) {
    w.put(".{ ");
    row.items.forEach((item, i) => {
      if (i > 0) w.put(", ");
      writeNode(w, item, depth + 1);
    });
    w.put(" }");
    return;
  }
  w.put(".{");
  writeTrailing(w, row);
  w.put("\n");
  for (const item of row.items) {
    const lead = item.kind === "keyvalue" ? item.key : item;
    const trail = item.kind === "keyvalue" ? item.value : item;
    w.comments(lead.leading, "//", depth + 1);
    w.indent(depth + 1);
    writeNode(w, item, depth + 1);
    w.put(",");
    // A container writes its own trailing comment, beside its `.{` or
    // after the `.{}` it is — which is before the comma, not after it.
    if (!isContainer(trail)) writeTrailing(w, trail);
    w.put("\n");
  }
  w.comments(dangling, "//", depth + 1);
  w.indent(depth);
  w.put("}");
}

function print(_dialect, t, options) {
  const w = fig.writer({ pretty: options?.pretty, indent: 4 });
  const root = fig.index(t).byid(0);
  if (w.pretty) w.comments(root.leading, "//", 0);
  writeNode(w, root, 0);
  if (!isContainer(root)) writeTrailing(w, root);
  w.put("\n");
  return w.string();
}

/** @type {import("@diaryx/fig/helper").Language} */
const language = {
  name: "zon",
  caps: { read: true, edit: true, serialize: true },
  lossless: { null: true, enum_literal: true, char_literal: true },
  syntax: {
    comments: { style: "slashes", line: { open: "//" }, trailing: { open: "//" } },
    kv_sep: " = ",
    key_style: "zon_field",
    // The byte before every key, which the editor removes with the key:
    // `.` (the wire carries a sigil as its byte).
    key_sigil: 46,
    empty_map_literal: ".{}",
    bare_document_mapping: false,
    flow_map_open: ".{",
    flow_map_close: "}",
  },
  // The compiled format's extension: where the format is compiled out
  // and this stands in for it, the extension reaches it.
  dialects: [{ name: "zon", extensions: ["zon"], splice: "literal", empty_doc_seed: ".{}\n" }],
  samples: ['.{ .a = 1, .b = .{ .c = "d" }, .e = .{ 1, 2 } }\n'],
  renderers: [],
  parse,
  print,
};

export default language;
