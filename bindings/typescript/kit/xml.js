// `import X from "@diaryx/fig/kit/xml"`: the XML shape, for an XML-shaped
// format.
//
// What fig's shared XML tokenizer is to its compiled `plist` and `xml`
// formats, this is to a language: purely lexical — tags, attributes,
// character data, CDATA; comments, processing instructions and a DOCTYPE
// skipped; entity references decoded — and knowing nothing about which
// elements mean what. Every function speaks the grammar module's rule
// shape, `(sc, ctx) => result | null`, so a tag is a rule and an element
// body is a `G.map` or `G.sequence` closed by `X.close(name)`.
//
// `X.with({ ... })` is the same kit with some of its messages replaced, for
// a format that words a refusal its own way.
import * as fig from "@diaryx/fig/kit";

const DEFAULTS = {
  unclosedComment: "unclosed comment; expected `-->`",
  unclosedCdata: "unclosed CDATA section; expected `]]>`",
  doctypeSubset: "a DOCTYPE with an internal subset is not supported",
  unterminatedDoctype: "unterminated DOCTYPE",
  unclosedPi: "unclosed processing instruction; expected `?>`",
  malformed: "markup is not well-formed here",
  unclosedTag: "unclosed tag; expected `>`",
  unclosedEmptyTag: "unclosed tag; expected `/>`",
  ltInAttribute: "`<` is not allowed in an attribute value",
  unclosedAttribute: "unclosed attribute value",
  badTagChar: "unexpected character inside a tag",
  textNotAllowed: "text is not allowed here; only markup",
  unexpected: "unexpected content here",
  noAttributes: (name) => "an attribute is not allowed on `<" + name + ">`",
  wrongClose: (name) => "expected `</" + name + ">` here",
  wrongName: (name) => "expected `<" + name + ">` here",
  noElement: (name) => "`<" + name + ">` holds text only; an element is not allowed inside it",
  unknownElement: (name) => "`<" + name + ">` is not an element this reader knows",
  entityNoSemicolon: "an entity reference here has no `;`",
  badCharRef: (ent) => "`&" + ent + ";` is not a character reference",
  unknownEntity: (ent) => "`&" + ent + ";` is not an entity this reader knows",
};

const ENTITIES = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" };

/** Whether byte `c` may start an XML name. */
export function isNameStart(c) {
  return c !== undefined && ((c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c === 95 || c === 58 || c >= 128);
}

/** Whether byte `c` may continue an XML name. */
export function isNameChar(c) {
  return isNameStart(c) || (c >= 48 && c <= 57) || c === 45 || c === 46;
}

function isSpace(c) {
  return c === 32 || c === 9 || c === 13 || c === 10;
}

function build(M) {
  const X = { messages: M, isNameStart, isNameChar };

  const msg = (key, arg) => {
    const m = M[key];
    return typeof m === "function" ? m(arg) : m;
  };

  /** The kit with some messages replaced. */
  X.with = (overrides) => build({ ...M, ...overrides });

  /** Decode entity and character references in `raw`, the text of bytes
   *  that began at offset `at`. */
  X.decodeEntities = (raw, at) => {
    if (!raw.includes("&")) return raw;
    // The offset of index `i` in `raw`: `at` plus the bytes before it.
    const offset = (i) => at + fig.byteLength(raw.slice(0, i));
    let out = "";
    let i = 0;
    while (i < raw.length) {
      const c = raw[i];
      if (c !== "&") {
        out += c;
        i += 1;
        continue;
      }
      const semi = raw.indexOf(";", i + 1);
      if (semi < 0) fig.fail(msg("entityNoSemicolon"), offset(i));
      const ent = raw.slice(i + 1, semi);
      if (ent[0] === "#") {
        const hex = ent[1] === "x" || ent[1] === "X";
        const digits = hex ? ent.slice(2) : ent.slice(1);
        const cp = digits === "" || !(hex ? /^[0-9a-fA-F]+$/ : /^[0-9]+$/).test(digits) ? NaN : parseInt(digits, hex ? 16 : 10);
        if (Number.isNaN(cp) || cp > 0x10ffff) fig.fail(msg("badCharRef", ent), offset(i));
        out += String.fromCodePoint(cp);
      } else {
        const r = Object.hasOwn(ENTITIES, ent) ? ENTITIES[ent] : undefined;
        if (r === undefined) fig.fail(msg("unknownEntity", ent), offset(i));
        out += r;
      }
      i = semi + 1;
    }
    return out;
  };

  /** At a `<`: skip a comment, a processing instruction or a DOCTYPE and
   *  answer true; answer false where a tag, a close tag or a CDATA section
   *  begins; refuse other `<!` markup. */
  X.skipMarkup = (sc) => {
    const s = sc.pos;
    if (sc.starts("<!--")) {
      sc.advance(4);
      const at = sc.find("-->");
      if (at < 0) fig.fail(msg("unclosedComment"), s);
      sc.pos = at + 3;
      return true;
    }
    if (sc.starts("<![CDATA[")) return false;
    if (sc.starts("<!DOCTYPE")) {
      sc.advance(9);
      while (!sc.eof()) {
        const c = sc.byte();
        if (c === 91) sc.fail(msg("doctypeSubset"));
        sc.advance();
        if (c === 62) return true;
      }
      sc.fail(msg("unterminatedDoctype"));
    }
    if (sc.starts("<!")) sc.fail(msg("malformed"));
    if (sc.starts("<?")) {
      sc.advance(2);
      const at = sc.find("?>");
      if (at < 0) fig.fail(msg("unclosedPi"), s);
      sc.pos = at + 2;
      return true;
    }
    return false;
  };

  /** Skip whitespace and ignorable markup until a tag begins or the input
   *  ends; text where markup is expected refuses the input with `message`
   *  at the start of the run it is in. */
  X.skip = (sc, message) => {
    for (;;) {
      const runStart = sc.pos;
      while (!sc.eof() && isSpace(sc.byte())) sc.advance();
      if (sc.eof()) return;
      if (sc.byte() !== 60) fig.fail(message, runStart);
      if (sc.starts("<![CDATA[")) fig.fail(message, sc.pos + 9);
      if (!X.skipMarkup(sc)) return;
    }
  };

  /** The trivia rule between elements. */
  X.trivia = (sc) => {
    const s = sc.pos;
    X.skip(sc, msg("textNotAllowed"));
    return [s, sc.pos];
  };

  /** A tag rule: `<name>`, `<name/>`, with attributes as `opts.attributes`
   *  allows — false (the default: one refuses the input with
   *  `noAttributes`), true, or a function of the element name. The result
   *  is `[s, e]` with `name`, `nameStart`, `empty` and `attrs` (each
   *  `{name, value, s}`); null where no tag begins (a close tag, or not
   *  markup). */
  X.tag = (opts) => {
    opts ??= {};
    const allowed = opts.attributes;
    return (sc) => {
      const s = sc.pos;
      if (sc.byte() !== 60 || !isNameStart(sc.byte(1))) {
        if (sc.byte() === 60 && !sc.starts("</") && !sc.starts("<!") && !sc.starts("<?")) {
          sc.fail(msg("malformed"));
        }
        return null;
      }
      sc.advance();
      const nameStart = sc.pos;
      sc.advance();
      while (!sc.eof() && isNameChar(sc.byte())) sc.advance();
      const name = sc.slice(nameStart, sc.pos);
      const may = typeof allowed === "function" ? allowed(name) : allowed;
      const tag = [s, s];
      tag.name = name;
      tag.nameStart = nameStart;
      tag.empty = false;
      tag.attrs = [];
      for (;;) {
        while (!sc.eof() && isSpace(sc.byte())) sc.advance();
        if (sc.eof()) sc.fail(msg("unclosedTag"));
        const c = sc.byte();
        if (c === 62) {
          sc.advance();
          break;
        } else if (c === 47) {
          if (!sc.starts("/>")) sc.fail(msg("unclosedEmptyTag"));
          sc.advance(2);
          tag.empty = true;
          break;
        } else if (isNameStart(c)) {
          if (!may) sc.fail(msg("noAttributes", name));
          const aStart = sc.pos;
          sc.advance();
          while (!sc.eof() && isNameChar(sc.byte())) sc.advance();
          const aName = sc.slice(aStart, sc.pos);
          while (!sc.eof() && isSpace(sc.byte())) sc.advance();
          if (sc.byte() !== 61) sc.fail(msg("unexpected"));
          sc.advance();
          while (!sc.eof() && isSpace(sc.byte())) sc.advance();
          const q = sc.byte();
          if (q !== 34 && q !== 39) sc.fail(msg("unexpected"));
          sc.advance();
          const vStart = sc.pos;
          while (!sc.eof() && sc.byte() !== q) {
            if (sc.byte() === 60) sc.fail(msg("ltInAttribute"));
            sc.advance();
          }
          if (sc.eof()) sc.fail(msg("unclosedAttribute"));
          tag.attrs.push({ name: aName, value: sc.slice(vStart, sc.pos), s: aStart });
          sc.advance();
        } else if (c === 61 || c === 34 || c === 39) {
          sc.fail(msg("unexpected"));
        } else {
          sc.fail(msg("badTagChar"));
        }
      }
      tag[1] = sc.pos;
      return tag;
    };
  };

  /** A close-tag rule for `name`: null where no close tag begins; a close
   *  tag of another name refuses the input. */
  X.close = (name) => (sc) => {
    const s = sc.pos;
    if (!sc.starts("</")) return null;
    sc.advance(2);
    while (!sc.eof() && isSpace(sc.byte())) sc.advance();
    const nameStart = sc.pos;
    if (!isNameStart(sc.byte())) sc.fail(msg("wrongClose", name));
    sc.advance();
    while (!sc.eof() && isNameChar(sc.byte())) sc.advance();
    if (sc.slice(nameStart, sc.pos) !== name) fig.fail(msg("wrongClose", name), nameStart);
    while (!sc.eof() && isSpace(sc.byte())) sc.advance();
    if (sc.byte() !== 62) sc.fail(msg("unexpected"));
    sc.advance();
    return [s, sc.pos];
  };

  /** The text content of an opened `tag`, entity references decoded and
   *  CDATA taken raw, through its close tag. An element inside refuses the
   *  input. */
  X.content = (sc, tag) => {
    if (tag.empty) return "";
    let out = "";
    for (;;) {
      if (sc.eof()) sc.fail(msg("unexpected"));
      if (sc.byte() === 60) {
        if (sc.starts("</")) break;
        if (sc.starts("<![CDATA[")) {
          sc.advance(9);
          const at = sc.find("]]>");
          if (at < 0) fig.fail(msg("unclosedCdata"), sc.pos - 9);
          out += sc.slice(sc.pos, at);
          sc.pos = at + 3;
        } else if (!X.skipMarkup(sc)) {
          sc.fail(msg("noElement", tag.name));
        }
      } else {
        const s = sc.pos;
        let at = sc.find("<");
        if (at < 0) at = sc.n;
        sc.pos = at;
        out += X.decodeEntities(sc.slice(s, at), s);
      }
    }
    X.close(tag.name)(sc);
    return out;
  };

  /** A rule for `<name>text</name>`: a string scalar over the whole
   *  element, its text decoded. null where no tag begins; another element's
   *  tag refuses the input with `opts.wrongName`. */
  X.textElement = (name, opts) => {
    opts ??= {};
    const tagRule = X.tag({ attributes: opts.attributes });
    return (sc) => {
      const tag = tagRule(sc);
      if (tag == null) return null;
      if (tag.name !== name) fig.fail(opts.wrongName ?? msg("wrongName", name), tag.nameStart);
      const text = X.content(sc, tag);
      return fig.scalar("string", [tag[0], sc.pos], text);
    };
  };

  /** A rule that reads a tag and hands it to `handlers[name](sc, ctx,
   *  tag)`, or to `opts.otherwise(sc, ctx, tag)`; with neither, the input
   *  is refused with `unknownElement` at the tag. */
  X.element = (handlers, opts) => {
    opts ??= {};
    const tagRule = X.tag({ attributes: opts.attributes });
    return (sc, ctx) => {
      const tag = tagRule(sc);
      if (tag == null) return null;
      const h = (Object.hasOwn(handlers, tag.name) ? handlers[tag.name] : undefined) ?? opts.otherwise;
      if (!h) fig.fail(msg("unknownElement", tag.name), tag[0]);
      return h(sc, ctx, tag);
    };
  };

  /** Escape `&`, `<` and `>` for text content. */
  X.escape = (s) => s.replace(/[&<>]/g, (ch) => (ch === "&" ? "&amp;" : ch === "<" ? "&lt;" : "&gt;"));

  return X;
}

const X = build(DEFAULTS);
export default X;
export const { with: withMessages, decodeEntities, skipMarkup, skip, trivia, tag, close, content, textElement, element, escape, messages } = X;
