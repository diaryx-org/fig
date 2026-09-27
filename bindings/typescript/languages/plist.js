// Apple property lists (the XML form), in JavaScript: the twin of fig's
// compiled `plist` format, row for row.
//
// The contract is the format and the tree it makes: for every document the
// compiled `plist` reads, this module builds the same node table and
// prints the same bytes, and it refuses what the compiled format refuses —
// in words and at offsets of its own. `fig lang table -i plist` prints
// that table, and `fig lang check js-plist --against plist <files…>` holds
// this module to it on any file. What the format is:
//
//   * `<plist [version="…"]>OBJECT</plist>`, or a bare OBJECT with no
//     wrapper; an XML declaration, a DOCTYPE without an internal subset,
//     processing instructions and `<!-- -->` comments are skipped;
//   * `dict` is a mapping of `<key>`/value pairs, `array` a sequence;
//     `string`, `integer`, `real`, `true`, `false` are the scalars they
//     name; `date` and `data` are extended scalars (`data` with all
//     whitespace stripped from the base64); entity references and CDATA are
//     decoded in text content; no element takes an attribute;
//   * a repeated `<key>` keeps the first entry's place and takes the last
//     value, as `plutil` does;
//   * a node's span is its whole element, `<` through `>`; an entry's runs
//     from its `<key>` through its value element.
//
// The compiled parser keeps no comments (its editor works on the `<!-- -->`
// pairs in the source), so neither does this one. The compiled format
// declares three fragment renderers — how a value spells as a typed
// element, an entry as `<key>k</key>` over the value, and a renamed key as
// its whole `<key>` element — and they are here too.
// Spans are byte offsets, 0-based, `[start, end)`.
//
// The XML shape — tags, text, CDATA, what is skipped, and how each is
// refused — is `fig/xml`, the twin of fig's shared XML tokenizer; what is
// here is what makes a plist a plist.
import * as fig from "@diaryx/fig/kit";
import * as G from "@diaryx/fig/kit/grammar";
import XML from "@diaryx/fig/kit/xml";

const X = XML.with({
  unexpected: "unexpected content here; expected plist markup",
  noAttributes: "an attribute is not allowed on a plist element",
  unknownElement: (name) => "`<" + name + ">` is not a plist element",
});

const UNEXPECTED = X.messages.unexpected;

// ── text ──────────────────────────────────────────────────────────────────

const trim = (s) => s.replace(/^[ \t\r\n]+|[ \t\r\n]+$/g, "");

const validInteger = (s) => /^[+-]?[0-9]+$/.test(s);
const validReal = (s) => /^[+-]?[0-9]+(\.[0-9]*)?([eE][+-]?[0-9]+)?$/.test(s);
const validBase64 = (s) => /^[A-Za-z0-9+/]*={0,2}$/.test(s);
const validDate = (s) => s !== "" && /^[0-9\-:.TZ+]+$/.test(s);

// ── the grammar ───────────────────────────────────────────────────────────
// Every value is an element, dispatched on its name; a container's body is
// a `G.map` or `G.sequence` closed by its close tag, so the tree, the spans
// and the duplicate policy are the grammar's.

let value;

// A scalar element: its span is the whole element.
const scalar = (kind, sc, tag, text, extra) => fig.scalar(kind, [tag[0], sc.pos], text, extra);

// `<true/>`, `<true></true>`, `<true> </true>`: nothing but whitespace and
// ignorable markup inside.
function noContent(sc, tag) {
  if (tag.empty) return;
  const message = "`<" + tag.name + ">` takes no content";
  X.skip(sc, message);
  if (sc.eof()) sc.fail(UNEXPECTED);
  if (!sc.starts("</")) sc.fail(message);
  X.close(tag.name)(sc);
}

const dictBody = G.map({
  entry: G.entry({
    key: X.textElement("key", { wrongName: "expected `<key>` here; a dict is `<key>`/value pairs" }),
    between: X.trivia,
    value: (sc, ctx) => value(sc, ctx),
    missingValue: "this `<key>` has no value after it",
  }),
  trivia: X.trivia,
  close: X.close("dict"),
  duplicates: "last_value",
  expected: UNEXPECTED,
  unclosed: UNEXPECTED,
});

const arrayBody = G.sequence({
  item: (sc, ctx) => value(sc, ctx),
  trivia: X.trivia,
  close: X.close("array"),
  expected: UNEXPECTED,
  unclosed: UNEXPECTED,
});

const elements = {
  dict(sc, ctx, tag) {
    if (tag.empty) return fig.mapping([tag[0], tag[1]]);
    const m = dictBody(sc, ctx);
    m.span[0] = tag[0];
    return m;
  },
  array(sc, ctx, tag) {
    if (tag.empty) return fig.sequence([tag[0], tag[1]]);
    const q = arrayBody(sc, ctx);
    q.span[0] = tag[0];
    return q;
  },
  string(sc, _ctx, tag) {
    return scalar("string", sc, tag, X.content(sc, tag));
  },
  integer(sc, _ctx, tag) {
    const raw = trim(X.content(sc, tag));
    if (!validInteger(raw)) sc.fail("`" + raw + "` is not an integer", tag[0]);
    return scalar("int", sc, tag, raw);
  },
  real(sc, _ctx, tag) {
    const raw = trim(X.content(sc, tag));
    if (!validReal(raw)) sc.fail("`" + raw + "` is not a real", tag[0]);
    return scalar("float", sc, tag, raw);
  },
  true(sc, _ctx, tag) {
    noContent(sc, tag);
    return scalar("bool", sc, tag, "true");
  },
  false(sc, _ctx, tag) {
    noContent(sc, tag);
    return scalar("bool", sc, tag, "false");
  },
  date(sc, _ctx, tag) {
    const t = trim(X.content(sc, tag));
    if (!validDate(t)) sc.fail("`" + t + "` is not an ISO 8601 date", tag[0]);
    return scalar("string", sc, tag, t, { ext_kind: "plist_date" });
  },
  data(sc, _ctx, tag) {
    const t = X.content(sc, tag).replace(/[ \t\r\n]/g, "");
    if (!validBase64(t)) sc.fail("`<data>` holds something other than base64", tag[0]);
    return scalar("string", sc, tag, t, { ext_kind: "plist_data" });
  },
};

value = X.element(elements);

// The document's object: inside a `<plist>` wrapper, which alone may carry
// an attribute, `version`; or bare.
const rootTag = X.tag({ attributes: (name) => name === "plist" });

function root(sc, ctx) {
  const tag = rootTag(sc);
  if (tag == null) return null;
  if (tag.name !== "plist") {
    const h = Object.hasOwn(elements, tag.name) ? elements[tag.name] : undefined;
    if (!h) sc.fail("`<" + tag.name + ">` is not a plist element", tag[0]);
    return h(sc, ctx, tag);
  }
  for (const a of tag.attrs) {
    if (a.name !== "version") sc.fail("`<plist>` takes only a `version` attribute", a.s);
  }
  if (tag.empty) sc.fail("`<plist/>` holds no object", tag[0]);
  X.trivia(sc);
  if (sc.starts("</")) sc.fail("`<plist>` holds no object");
  const inner = value(sc, ctx);
  if (inner == null) sc.fail(UNEXPECTED);
  X.trivia(sc);
  if (sc.byte() === 60 && !sc.starts("</")) sc.fail("a plist holds one object; a second one starts here");
  if (!sc.starts("</")) sc.fail(UNEXPECTED);
  X.close("plist")(sc);
  return inner;
}

const parse = G.document({
  root,
  trivia: X.trivia,
  missing: "no plist object; expected `<plist>` or a `<dict>`",
  trailing: (sc) => (sc.byte() === 60 ? "a plist holds one object; a second one starts here" : UNEXPECTED),
});

// ── the printer ───────────────────────────────────────────────────────────
// The compiled printer's layout: the XML declaration, the DOCTYPE, a
// `<plist version="1.0">` wrapper, and one element per line indented by
// `options.indent` per depth; `<dict/>` and `<array/>` when empty. Splice
// text (`options.splice`) is the root as its bare element, with no header
// and no wrapper.

const textElement = (tag, text) => "<" + tag + ">" + X.escape(text) + "</" + tag + ">";

const HEADER =
  '<?xml version="1.0" encoding="UTF-8"?>\n' +
  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n' +
  '<plist version="1.0">';

function writeValue(w, row, depth) {
  const k = row.kind;
  if (k === "null") throw new Error("a plist has no null; a null value cannot be written");
  if (k === "alias") throw new Error("an alias must be resolved before it is written as a plist");
  if (k === "bool") {
    w.put(row.text === "true" ? "<true/>" : "<false/>");
  } else if (k === "int") {
    w.put("<integer>", row.text ?? "", "</integer>");
  } else if (k === "float") {
    w.put("<real>", row.text ?? "", "</real>");
  } else if (k === "string") {
    const tag = row.ext_kind === "plist_date" ? "date" : row.ext_kind === "plist_data" ? "data" : "string";
    w.put(textElement(tag, row.text ?? ""));
  } else if (k === "sequence") {
    if (row.items.length === 0) return w.put("<array/>");
    w.put("<array>").nl();
    for (const item of row.items) {
      w.indent(depth);
      writeValue(w, item, depth + 1);
      w.nl();
    }
    w.indent(depth - 1).put("</array>");
  } else if (k === "mapping") {
    if (row.items.length === 0) return w.put("<dict/>");
    w.put("<dict>").nl();
    for (const kv of row.items) {
      if (kv.key.kind !== "string") throw new Error("a plist dict key must be a string");
      w.line(depth, textElement("key", kv.key.text ?? ""));
      w.indent(depth);
      writeValue(w, kv.value, depth + 1);
      w.nl();
    }
    w.indent(depth - 1).put("</dict>");
  } else {
    throw new Error("a " + k + " is not a value");
  }
  return w;
}

function print(_dialect, t, options) {
  fig.index(t);
  const rootRow = t.byid(0);
  const w = fig.writer(options);
  if (options?.splice || fig.isScalar(rootRow.kind)) {
    // Splice text, or a scalar fragment: the bare element with no
    // declaration, DOCTYPE or `<plist>` wrapper, a container's lines at the
    // top level — the compiled `printSplice`. What the editor splices, and
    // what `renderValue` takes as an element already spelled.
    writeValue(w, rootRow, 1);
    return w.string();
  }
  w.put(HEADER).nl();
  writeValue(w, rootRow, 1);
  w.nl().put("</plist>\n");
  return w.string();
}

// ── the renderers ─────────────────────────────────────────────────────────
// A value the CLI hands the editor is bare text, and fig has already said
// what its own literal rules make of it — `args.literal`: `true`/`false`, a
// number (decimal with `_` separators and no leading zero, or `0x`/`0o`/
// `0b`), a datetime shape, or a string — so this only spells the typed
// element for the kind it is told. Text that already starts with `<` is
// spliced as written: how `<data>` or a nested `<dict>` is given, or a type
// forced.

const ELEMENT = { int: "integer", float: "real", datetime: "date" };

function renderValue(valueText, literal) {
  const t = trim(valueText);
  if (t[0] === "<") return t;
  if (literal === "null") throw new Error("a plist has no null; a null value cannot be written");
  if (literal === "bool") return t === "true" ? "<true/>" : "<false/>";
  const tag = ELEMENT[literal];
  if (tag) return "<" + tag + ">" + t + "</" + tag + ">";
  return textElement("string", t);
}

// An entry: `<key>k</key>`, then the value at the same indent. A value that
// spans lines — a `<dict>` or `<array>` spliced as the printer spells it,
// its lines at the top level — has every further non-empty line moved
// under `indent` too, so it lands at the entry's depth.
function renderEntry(indent, key, value) {
  const [first, ...rest] = value.split("\n");
  let out = "<key>" + X.escape(key) + "</key>\n" + indent + first;
  for (const line of rest) out += "\n" + (line === "" ? "" : indent) + line;
  return out;
}

function render(which, args) {
  if (which === "value") return renderValue(args.value, args.literal ?? "string");
  if (which === "entry") return renderEntry(args.indent, args.key, args.value);
  // A renamed key: a key's span is its whole `<key>…</key>` element, so the
  // new name is spelled and escaped rather than spliced bare over the tags.
  if (which === "key") return "<key>" + X.escape(args.key) + "</key>";
  throw new Error("no renderer `" + which + "`");
}

// ── the language ──────────────────────────────────────────────────────────

/** @type {import("@diaryx/fig/helper").Language} */
const language = {
  name: "plist",
  caps: { read: true, edit: true, serialize: true },
  syntax: {
    comments: {
      style: "xml_comment",
      line: { open: "<!--", close: "-->", forbidden: "--" },
      trailing: { open: "<!--", close: "-->", forbidden: "--" },
    },
    // An entry is a pair of sibling elements, not a `key<sep>value` line,
    // and a value is always a typed element: no separator, no bare `{}`.
    flow_containers: false,
    closed_containers: { map_open: "<dict>", map_close: "</dict>", seq_open: "<array>", seq_close: "</array>" },
    seq_item_marker: "",
  },
  dialects: [{ name: "plist", extensions: ["plist"], splice: "raw", empty_doc_seed: "<dict>\n</dict>\n" }],
  samples: [
    '<plist version="1.0"><dict><key>a</key><string>b</string><key>n</key><integer>1</integer><key>l</key><array><true/><false/></array></dict></plist>\n',
  ],
  renderers: ["value", "entry", "key"],
  parse,
  print,
  render,
};

export default language;
