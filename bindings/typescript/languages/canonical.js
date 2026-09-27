// fig's canonical form, in JavaScript: the twin of the compiled `canonical`
// format, row for row.
//
// The canonical form is the tree spelled with nothing added — a total,
// one-spelling encoding of every node kind fig has, the oracle two
// documents are compared through — so this module touches every row the
// wire can carry, which neither `dotenv.mjs` (flat) nor `plist.mjs` (XML)
// does. The form, and the tree it makes — the contract this module is held
// to, fixture by fixture, against the compiled `canonical`:
//
//   node   ::= ('&' name | '!' tag)* value
//   value  ::= 'null' | 'true' | 'false'
//            | '"' … '"'                       JSON escapes, `\uXXXX` included
//            | ['~i' | '~f'] number            the lexeme kept verbatim
//            | '@' extkind '"' … '"'           an extended scalar
//            | '[' node (',' node)* [','] ']'
//            | '{' node ':' node (',' …)* [','] '}'   keys are nodes
//            | '*' name                        an alias
//
// A number's kind is implied by its lexeme (`0x…` is an int; a `.` or an
// exponent makes a float) unless `~i`/`~f` pins it; the prefix is in the
// span and not in the text. Repeated keys all stay. Comments are `//` to
// the end of the line and `/* … */`; where each binds is the grammar
// module's rule with `one` and `closingLine` set, which is to say:
//
//   * a `//` on the line a container opens is that container's trailing
//     comment (a block comment there leads the first child);
//   * a comment after an element, past its comma, is the element's
//     trailing comment — unless the element is a container that spans
//     lines, in which case it is that container's last dangling one;
//   * comments before the root go to its first child (or dangle on an
//     empty root), and a scalar root takes them as leading; a comment on
//     the root's closing line is the root's trailing, and anything after
//     dangles on the root.
//
// The compiled form is a parse/print pair with no editor, so `caps.edit` is
// off and there is no `syntax`. Spans are byte offsets, 0-based, `[start,
// end)`. The compiled build is opt-in (`zig build -Dcanonical=true`), which
// is what `fig lang check js-canonical --against canonical` needs.
import * as fig from "@diaryx/fig/kit";
import * as G from "@diaryx/fig/kit/grammar";

// ── lexical ───────────────────────────────────────────────────────────────

const NAME = /[A-Za-z0-9_\-]+/;
const EXT_KINDS = new Set([
  "offset_datetime",
  "local_datetime",
  "local_date",
  "local_time",
  "enum_literal",
  "char_literal",
  "number_special",
  "plist_date",
  "plist_data",
]);

const lineComment = G.comment("//");
const blockComment = G.comment({ open: "/*", close: "*/", unclosed: "unclosed `/*` comment; expected `*/`" });
const comment = G.choice([lineComment, blockComment]);
// Whitespace is spaces, tabs and line endings of either kind, as the
// compiled parser's `skipWs`; a lone `\r` counts.
const trivia = G.trivia({ space: G.ws1, comment });

const isContainer = (n) => n.kind === "mapping" || n.kind === "sequence";

// The compiled printer's `impliedNumberKind`, so a bare lexeme reparses to
// the same kind it was printed from.
function impliedKind(raw) {
  const body = raw.replace(/^[+-]/, "");
  if (/^0[xX]/.test(body)) return "int";
  if (/[.eE]/.test(raw)) return "float";
  return "int";
}

const looksLikeNumber = (raw) => /^[0-9.]/.test(raw.replace(/^[+-]/, ""));

// ── scalars ───────────────────────────────────────────────────────────────

const SIMPLE_ESCAPES = { '"': '"', "\\": "\\", "/": "/", b: "\b", f: "\f", n: "\n", r: "\r", t: "\t" };

// JSON's escapes, `\uXXXX` as one character. The grammar's `quoted` decodes
// an escape to a fixed string, which `\u` is not, so it only finds the
// closing quote here and the text is decoded whole.
function decode(raw, span) {
  if (!raw.includes("\\")) return raw;
  let out = "";
  let i = 0;
  while (i < raw.length) {
    const c = raw[i];
    if (c !== "\\") {
      out += c;
      i += 1;
      continue;
    }
    const e = raw[i + 1];
    if (e === "u") {
      const hex = raw.slice(i + 2, i + 6);
      if (!/^[0-9a-fA-F]{4}$/.test(hex)) {
        fig.fail("invalid `\\u` escape in a string; expected four hex digits", span[0]);
      }
      const cp = parseInt(hex, 16);
      if (fig.isSurrogate(cp)) {
        fig.fail("invalid `\\u` escape in a string; a surrogate is not a character", span[0]);
      }
      out += String.fromCodePoint(cp);
      i += 6;
    } else if (e !== undefined && Object.hasOwn(SIMPLE_ESCAPES, e)) {
      out += SIMPLE_ESCAPES[e];
      i += 2;
    } else {
      fig.fail("invalid escape `\\" + (e ?? "") + "` in a string", span[0]);
    }
  }
  return out;
}

const stringValue = G.quoted({
  open: '"',
  escape: "\\",
  crlf: false,
  text: decode,
  unclosed: 'unclosed string; expected a closing `"`',
});

// `null`, `true`, `false`: a word of name characters that is one of them.
function bareword(sc) {
  const s = sc.pos;
  const c = sc.byte();
  if (!(c !== undefined && (c === 95 || (c >= 65 && c <= 90) || (c >= 97 && c <= 122)))) return null;
  const [, e] = sc.match(NAME);
  const word = sc.slice(s, e);
  if (word === "null") return fig.scalar("null", [s, e], undefined);
  if (word === "true" || word === "false") return fig.scalar("bool", [s, e], word);
  sc.fail("`" + word + "` is not a value; expected `null`, `true` or `false`", s);
}

// A number: an optional `~i`/`~f`, then the lexeme, drawn from the
// characters the compiled lexer takes and beginning like a number.
function number(sc) {
  const s = sc.pos;
  const c = sc.byte();
  if (!(c !== undefined && ((c >= 48 && c <= 57) || c === 43 || c === 45 || c === 46 || c === 126))) return null;
  let pinned;
  if (c === 126) {
    sc.advance();
    const k = sc.byte();
    if (k === 102) pinned = "float";
    else if (k === 105) pinned = "int";
    else sc.fail("expected `i` or `f` after `~`");
    sc.advance();
  }
  const m = sc.match(/[0-9a-fA-FxXoObB._+\-]+/);
  if (m === null) sc.fail("expected a number here");
  const [rs, re] = m;
  const raw = sc.slice(rs, re);
  if (!looksLikeNumber(raw)) sc.fail("`" + raw + "` is not a number", rs);
  return fig.scalar(pinned ?? impliedKind(raw), [s, re], raw);
}

// `@kind "text"`: a string scalar of an extended kind, spanning both.
function extended(sc) {
  const s = sc.pos;
  if (sc.lit("@") === null) return null;
  const [ks, ke] = sc.match(/[a-z_]*/);
  const kind = sc.slice(ks, ke);
  if (!EXT_KINDS.has(kind)) sc.fail("`@" + kind + "` is not an extended scalar kind", s);
  G.ws(sc);
  const v = stringValue(sc);
  if (v == null) sc.fail('expected a `"` string after `@' + kind + "`");
  return fig.scalar("string", [s, v.span[1]], v.text, { ext_kind: kind });
}

function alias(sc) {
  const s = sc.pos;
  if (sc.lit("*") === null) return null;
  const m = sc.match(NAME);
  if (m === null) sc.fail("expected a name after `*`");
  const [ns, ne] = m;
  return fig.scalar("alias", [s, ne], sc.slice(ns, ne));
}

// ── nodes ─────────────────────────────────────────────────────────────────
// `node` is prefixes then a value; the containers are `G.map` and
// `G.sequence`, with the openers, the separators and the key and value
// rules doing what the compiled binding does that the module's defaults do
// not.

let node;

// A container's opener: the delimiter, and a `//` comment on its line,
// which the container takes as its trailing comment once it exists — held
// on the context until then, a stack because containers nest. The
// context's `last` is cleared so nothing before the opener catches a
// comment inside it.
const opener = (delim) => (sc, ctx) => {
  const s = sc.pos;
  if (sc.lit(delim) === null) return null;
  const p = sc.pos;
  G.hs(sc);
  const c = lineComment(sc);
  if (c == null) sc.pos = p;
  ctx.heads.push(c ?? false);
  ctx.last = null;
  return [s, sc.pos];
};

// After an element: a comma, or the close ahead (comments allowed between,
// bound later by the container's own trivia); anything else is an element
// with no comma before it.
function separator(close) {
  const closeAhead = G.ahead((sc) => {
    trivia(sc, null);
    return close(sc);
  });
  return G.seq([G.hs, G.choice([G.lit(","), closeAhead, G.fail("expected `,` between elements")])]);
}

const container = (rule) => (sc, ctx) => {
  const n = rule(sc, ctx);
  if (n == null) return null;
  const head = ctx.heads.pop();
  if (head) n.comment("trailing", head.text, head.style);
  return n;
};

// A key is any node; the comments waiting when it is done are its own, so
// that those between the `:` and the value are the value's.
function key(sc, ctx) {
  const k = node(sc, ctx);
  if (k != null) ctx.flush(k, "leading");
  return k;
}

function value(sc, ctx) {
  ctx.last = null;
  trivia(sc, ctx);
  const v = node(sc, ctx);
  if (v != null) ctx.flush(v, "leading");
  return v;
}

const mapping = container(
  G.map({
    open: opener("{"),
    close: G.lit("}"),
    trivia,
    entry: G.entry({
      key,
      between: G.ws,
      sep: G.lit(":"),
      value,
      missingSep: "expected `:` after this key",
      missingValue: "expected a value after `:`",
    }),
    after: separator(G.lit("}")),
    duplicates: "keep",
    expected: "expected a `key: value` entry or `}` here",
    unclosed: "unclosed mapping; expected `}` before the end of the input",
  }),
);

const sequence = container(
  G.sequence({
    open: opener("["),
    close: G.lit("]"),
    trivia,
    item: (sc, ctx) => node(sc, ctx),
    after: separator(G.lit("]")),
    expected: "expected a value or `]` here",
    unclosed: "unclosed sequence; expected `]` before the end of the input",
  }),
);

const plainValue = G.choice([mapping, sequence, stringValue, extended, alias, number, bareword]);

// Prefixes, then the value they mark: `&name` at most once, `!tag` at most
// once, in either order, whitespace between. The node's span is the
// value's; the prefixes are fields on it.
node = (sc, ctx) => {
  let anchor;
  let tag;
  for (;;) {
    const c = sc.byte();
    if (c === 38) {
      if (anchor !== undefined) sc.fail("a node takes one anchor");
      sc.advance();
      const m = sc.match(NAME);
      if (m === null) sc.fail("expected a name after `&`");
      anchor = sc.slice(m[0], m[1]);
      G.ws(sc);
    } else if (c === 33) {
      if (tag !== undefined) sc.fail("a node takes one tag");
      const [ts, te] = sc.match(/[^ \t\r\n,:{}[\]"]+/);
      tag = sc.slice(ts, te);
      G.ws(sc);
    } else {
      break;
    }
  }
  const v = plainValue(sc, ctx);
  if (v == null) {
    if (anchor !== undefined || tag !== undefined) sc.fail("expected a value after the prefix");
    return null;
  }
  if (anchor !== undefined) v.anchor = anchor;
  if (tag !== undefined) v.tag = tag;
  return v;
};

// The document, by hand rather than `G.document`: the root's comments bind
// by rules of their own (see the top), and the binding policy is the
// context's, so this is where a module states one.
function parse(_dialect, input) {
  const sc = fig.scanner(input);
  // The binding rule is the context's own, refined twice: `closingLine` —
  // a container that spans lines takes the comment on its closing line as
  // dangling, not trailing; `one` — a node takes one trailing comment, a
  // second on the same line waiting like any other.
  const ctx = G.context(sc, { one: true, closingLine: true });
  ctx.heads = [];
  trivia(sc, ctx);
  const root = node(sc, ctx);
  if (root == null) {
    if (sc.eof()) sc.fail("no document here; expected a value");
    sc.fail("expected a value here");
  }
  // A container root gave what was waiting to its first child already; a
  // scalar root takes it as leading.
  ctx.flush(root, "leading");
  G.hs(sc);
  const c = comment(sc);
  if (c != null) root.comment("trailing", c.text, c.style);
  ctx.last = null;
  trivia(sc, ctx);
  if (!sc.eof()) sc.fail("unexpected content after the document");
  ctx.flush(root, "dangling");
  return fig.rows(root);
}

// ── the printer ───────────────────────────────────────────────────────────
// The compiled printer's layout, which is the only one: two spaces per
// level, one element per line, an empty container inline, a container's
// trailing comment beside its opener, its dangling comments before its
// close. `options` is not consulted — one document has one spelling — so
// `pretty` off changes nothing.

function quote(s) {
  return (
    '"' +
    s.replace(/[\x00-\x1f\x7f"\\]/g, (ch) => {
      switch (ch) {
        case '"':
          return '\\"';
        case "\\":
          return "\\\\";
        case "\b":
          return "\\b";
        case "\f":
          return "\\f";
        case "\n":
          return "\\n";
        case "\r":
          return "\\r";
        case "\t":
          return "\\t";
        default:
          return "\\u" + ch.charCodeAt(0).toString(16).padStart(4, "0");
      }
    }) +
    '"'
  );
}

function writeComment(w, c) {
  if (c.style === "block") w.put("/*", c.text !== "" ? " " + c.text + " " : "", "*/");
  else w.put("//", c.text !== "" ? " " + c.text : "");
}

// A container with no items and no dangling comments prints inline, and its
// trailing comment is written by whoever writes the line.
const isInline = (row) => isContainer(row) && row.items.length === 0 && row.dangling.length === 0;

const indent = (w, depth) => w.put("  ".repeat(depth));

// The rows a child's comments hang on: an entry's leading on its key, its
// trailing after its value.
const anchorsOf = (row) => (row.kind === "keyvalue" ? [row.key, row.value] : [row, row]);

let writeNode;

function writeContainer(w, row, open, close, depth) {
  if (isInline(row)) return w.put(open, close);
  w.put(open);
  if (row.trailing[0]) {
    w.put(" ");
    writeComment(w, row.trailing[0]);
  }
  w.put("\n");
  row.items.forEach((child, i) => {
    const [lead, trail] = anchorsOf(child);
    for (const c of lead.leading) {
      indent(w, depth + 1);
      writeComment(w, c);
      w.put("\n");
    }
    indent(w, depth + 1);
    writeNode(w, child, depth + 1);
    if (i < row.items.length - 1) w.put(",");
    if ((!isContainer(trail) || isInline(trail)) && trail.trailing[0]) {
      w.put(" ");
      writeComment(w, trail.trailing[0]);
    }
    w.put("\n");
  });
  for (const c of row.dangling) {
    indent(w, depth + 1);
    writeComment(w, c);
    w.put("\n");
  }
  indent(w, depth);
  w.put(close);
  return w;
}

writeNode = (w, row, depth) => {
  if (row.anchor !== undefined) w.put("&", row.anchor, " ");
  if (row.tag !== undefined) w.put(row.tag, " ");
  const k = row.kind;
  if (k === "null") {
    w.put("null");
  } else if (k === "bool") {
    w.put(row.text);
  } else if (k === "int" || k === "float") {
    if (impliedKind(row.text) !== k) w.put(k === "float" ? "~f" : "~i");
    w.put(row.text);
  } else if (k === "string") {
    if (row.ext_kind !== undefined) w.put("@", row.ext_kind, " ");
    w.put(quote(row.text ?? ""));
  } else if (k === "alias") {
    w.put("*", row.text);
  } else if (k === "sequence") {
    writeContainer(w, row, "[", "]", depth);
  } else if (k === "mapping") {
    writeContainer(w, row, "{", "}", depth);
  } else if (k === "keyvalue") {
    writeNode(w, row.key, depth);
    w.put(": ");
    // A comment between the `:` and the value is the value's leading
    // comment, and stays between them: a block one on the line, a line one
    // ending it, the value on the next. (The compiled printer writes an
    // entry's leading comments from its key alone and loses these.)
    for (const c of row.value.leading) {
      writeComment(w, c);
      if (c.style === "block") {
        w.put(" ");
      } else {
        w.put("\n");
        indent(w, depth + 1);
      }
    }
    writeNode(w, row.value, depth);
  } else {
    throw new Error("a " + k + " is not a value");
  }
};

function print(_dialect, t, _options) {
  fig.index(t);
  const root = t.byid(0);
  const w = fig.writer();
  for (const c of root.leading) {
    writeComment(w, c);
    w.put("\n");
  }
  writeNode(w, root, 0);
  if ((!isContainer(root) || isInline(root)) && root.trailing[0]) {
    w.put(" ");
    writeComment(w, root.trailing[0]);
  }
  if (!isContainer(root)) {
    for (const c of root.dangling) {
      w.put("\n");
      writeComment(w, c);
    }
  }
  w.put("\n");
  return w.string();
}

// ── the language ──────────────────────────────────────────────────────────

/** @type {import("@diaryx/fig/helper").Language} */
const language = {
  name: "canonical",
  caps: { read: true, serialize: true },
  // The compiled form owns no extension — it is selected by name — and
  // neither does fig's format registry, so this is a new format beside it
  // rather than a stand-in; the extension is for the fixtures beside the
  // tests.
  dialects: [{ name: "canonical", extensions: ["canonical"] }],
  samples: [
    '{\n  "a": [1, ~f2, -0x1F, .5, "s\\n", null, true],\n  &x "k": @local_date "2024-01-01",\n  1: *x,\n  "e": {} // empty\n}\n',
    '// lead\n[ // head\n  !!str "t", // one\n  [\n    "deep"\n  ], // after\n  "last" /* end */\n  // before close\n]\n// eof\n',
    '"a scalar"\n',
  ],
  parse,
  print,
};

export default language;
