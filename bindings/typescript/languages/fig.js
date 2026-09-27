// fig, in JavaScript: the twin of fig's own authoring dialect — the
// compiled `fig` format, `.figl` — row for row, region for region, mention
// for mention, marker for marker.
//
// The contract is the format and the tree it makes: for every document the
// compiled `fig` reads, this module builds the same node table and prints
// the same bytes. `fig lang table -i fig` prints that table, and `fig lang
// check js-fig --against fig <files…>` holds this module to it on any
// file; where the format is written down is fig's `src/languages/fig/`
// (`DESIGN.md`, `tokenizer.zig`, `parser.zig`, `printer.zig`).
//
// The shape, as the compiled parser builds it:
//
//   * a line's depth is its count of leading `>` markers, relative to the
//     header that last re-anchored the baseline; a bare word opens a
//     container (a *section*), `key = value` assigns, `> *` is an element,
//     `a.b[]` appends an element and `+` appends another;
//   * the root is a mapping (or a sequence of `*` elements) spanning from 0
//     to the end of its last child; a block container spans from its
//     creating key (an element from the byte after its `*` prefix, an
//     appended element from its header line's start) to the end of its
//     last child; a flow `[…]`/`{…}` spans its brackets; a keyvalue runs
//     from its key to its value's end and records its `=` as `sep` — a
//     header's or dotted segment's is zero-width at the key's end; an
//     element records its `*` as `marker`;
//   * a block container's *regions* are the line that created it and every
//     later header line that re-opened it by its final segment; its
//     *mentions* are its key and each such re-opening's key, all of kind
//     `entry` — a fig header sits on its parent's own lines;
//   * a bare value is sniffed literal-else-string: `null`, `true`, `false`,
//     a number with its lexeme kept verbatim, a datetime; a `: type =`
//     annotation is checked and kept as a `tag` (`!!int`, `!!float`,
//     `!!bool`, `!!str`), and `enum`/`char`/`float = inf` give extended
//     kinds; a char literal's text is its decimal codepoint;
//   * a comment line waits with its own depth: when a container closes, a
//     comment at or below its child depth dangles on it, a shallower one
//     leads the next key or element; a same-line `#` after a value trails
//     the value; what waits at the end dangles on the root; comments
//     inside a flow value are discarded.
//
// This module refuses every document the compiled format refuses, in words
// and at offsets of its own — the message is written for whoever is holding
// the file, not copied. What is not carried: the authoring-time warnings (a
// leading zero kept as text, an indent that disagrees with its markers),
// which have no row.
//
// What fig shares with the other section formats — the region a header
// line is, the mention a name is, the comments waiting for a key, by
// depth — is the grammar module's `sections`; the frames, the markers and
// what a header may re-enter are fig's own.
//
// Every offset is a byte offset: the parser walks the scanner's
// one-char-per-byte shadow of the input (`sc.bin`) and decodes text from
// the bytes a token covers. The same object `@diaryx/fig`'s
// `registerLanguage` takes, so it serves the browser and Node unchanged.
import * as fig from "@diaryx/fig/kit";
import * as G from "@diaryx/fig/kit/grammar";
import * as DT from "@diaryx/fig/kit/datetime";

// ── refusals ──────────────────────────────────────────────────────────────

const E = {
  FigForeignSyntaxColon: "`:` introduces a type, not a value; write `key = value`, or `key: type = value`",
  FigFlowBareKeyColon: 'a bare key cannot take a `:` pair; write `key = 1` (fig) or `"key": 1` (JSON)',
  FigForeignSyntaxDash: "`-` is YAML's element marker; fig elements are `*` — write `> *` then `> > host = a.com` (a scalar element is `* value`)",
  FigForeignSyntaxBracket: "`[section]` / `[[x]]` is TOML; fig section headers are bare dotted paths — write `section`, or `x[]` to append an element",
  FigElementInlineField: "an element's fields go on following lines; write `> *` then `> > host = a.com`, not `* host = a.com`",
  FigRootMarker: "root keys carry zero markers; remove the `>` (a marker line needs a parent header above it)",
  FigSkippedLevel: "this line skips a nesting level; depth may only grow one `>` at a time — add the missing parent line, or drop the extra `>`",
  FigBadMarkerSeparator: "put a space between the marker run and what follows: `> key`, not `>key`",
  FigBadKey: 'empty or malformed key; a bare key cannot contain `.` `:` `=` `[` or whitespace, or begin with `>`/`-` — quote it: `"my.key" = x`',
  FigDuplicateKey: "duplicate key: this key already has a value here; remove one of the definitions (re-enter a header only to add NEW keys)",
  FigMixedContainerChildren: "a container holds either `key = value` entries or `*` elements, never both",
  FigMixedSequenceAddressing: "one sequence cannot mix `[]`/`[i]` addressing with `*` element lines; pick one spelling",
  FigKeyNotContainer: "this path steps into an existing non-container value; remove the extra path segment, or restructure the earlier value",
  FigIndexSkipped: "sequence indices cannot skip ahead; write the earlier element first (even `[n] = null`)",
  FigIndexAlreadySet: "this index already has a value; address a new index, or use a header-final `[]` to append",
  FigEmptyAppendTarget: 'a non-final `[]` means "the last element", but this sequence is empty; append one first with a header-final `[]`',
  FigAppendAssignment: "a final `[]` appends a container via a header, never a `= value`; write `key[]` on its own line with `> field = value` lines below, or spell the next index out: `key[N] = value`",
  FigEmptyContainer: "this container has no children; write an inline empty value instead: `key = {}` (map) or `key = []` (sequence)",
  FigInvalidValue: "missing value; write one after `=`, or `{}`/`[]` for an empty container",
  FigTypeMismatch: "the value does not satisfy its `: type` annotation; fix the value, or drop/correct the annotation",
  FigUnknownType: "unknown type name; the built-in types are int, float, bool, string, enum, char, datetime, date, time",
  FigTrailingContent: "unexpected content after this line's value or header; quote the whole value if it is one string (a `#` comment needs a space before it)",
  FigQuotedTrailingContent:
    'this string ends at its matching quote, and the rest of the line is stray content; fig bare strings need no outer quotes — write `key = She said, "Hey there!"`, or escape the inner quotes: `"She said, \\"Hey there!\\""`',
  FigMultilineOpenerContent:
    "a multiline string's content begins on the line AFTER the opening `'''`/`\"\"\"`; move this text down a line (only a `# comment` may share the opener line)",
  FigDanglingContinuation: "`+` has no `[]` append header to re-run; move it directly after its `a.b[]` group, or repeat the header",
  FigClosedFlowValue: "a value written inline as `[…]`/`{…}` is closed and cannot be extended later; write the block or header form if it needs to grow",
  FigUnclosedFlow: "this `[`/`{` value never finds its matching close; close it, or quote the whole value to make it a string",
  FigMixedFlowSeparators: "a flow object is fig (`=` pairs) or JSON (`:` pairs), never both in one object",
  FigUnclosedString: "unclosed string; add the closing quote (a single-line quote cannot span lines — use `'''` for multi-line)",
  FigBadEscape: "invalid escape; double quotes support \\n \\t \\r \\\\ \\\" \\uXXXX — use single quotes ('…') for raw text with literal backslashes",
};

// ── lexical helpers, as `tokenizer.zig` ───────────────────────────────────
// `s` is a bin string (one char per byte) throughout this section.

const isDigit = (c) => c >= 48 && c <= 57;
const isHex = (c) => isDigit(c) || (c >= 65 && c <= 70) || (c >= 97 && c <= 102);
const isOctal = (c) => c >= 48 && c <= 55;
const isBinary = (c) => c === 48 || c === 49;
const isAlnum = (c) => isDigit(c) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122);
const isBareKeyChar = (c) => isAlnum(c) || c === 95 || c === 45;

// A digit run at `start` with `_` separators anchored between digits; the
// index just past it, or null.
function scanUnderscored(s, start, pred) {
  let j = start;
  let prevUs = true;
  while (j < s.length) {
    const c = s.charCodeAt(j);
    if (c === 95) {
      if (prevUs) return null;
      prevUs = true;
    } else if (pred(c)) prevUs = false;
    else break;
    j += 1;
  }
  if (prevUs) return null;
  return j;
}

// TOML-style bare number: "int"/"float", or null.
function sniffNumber(token) {
  if (token === "") return null;
  let i = 0;
  const f = token.charCodeAt(0);
  if (f === 43 || f === 45) i = 1;
  if (i >= token.length) return null;
  const body = token.slice(i);
  if (body.length >= 2 && body.charCodeAt(0) === 48) {
    const r = body[1];
    const pred = r === "x" ? isHex : r === "o" ? isOctal : r === "b" ? isBinary : null;
    if (pred) {
      const e = scanUnderscored(body, 2, pred);
      if (e === null || e !== body.length) return null;
      return "int";
    }
  }
  let j = scanUnderscored(token, i, isDigit);
  if (j === null) return null;
  if (j - i > 1 && token.charCodeAt(i) === 48) return null;
  let isFloat = false;
  if (j < token.length && token.charCodeAt(j) === 46) {
    isFloat = true;
    j = scanUnderscored(token, j + 1, isDigit);
    if (j === null) return null;
  }
  if (j < token.length && (token.charCodeAt(j) === 101 || token.charCodeAt(j) === 69)) {
    isFloat = true;
    j += 1;
    if (j < token.length && (token.charCodeAt(j) === 43 || token.charCodeAt(j) === 45)) j += 1;
    j = scanUnderscored(token, j, isDigit);
    if (j === null) return null;
  }
  if (j !== token.length) return null;
  return isFloat ? "float" : "int";
}

// RFC 3339, through the shared `fig/datetime`: fig takes a bare time with
// no date, and a time to the minute with no seconds, so both options are
// on.
const DATETIME = { minutePrecision: true, timeOnly: true };

// Literal-else-string over a trimmed bare token (as text).
function sniffBare(token) {
  if (token === "") return { kind: "string", text: token };
  if (token === "null") return { kind: "null" };
  if (token === "true" || token === "false") return { kind: "bool", text: token };
  const n = sniffNumber(token);
  if (n) return { kind: n, text: token };
  const d = DT.classify(token, DATETIME);
  if (d) return { kind: "string", text: token, extKind: d };
  return { kind: "string", text: token };
}

const quoteOpensSpan = (prev) => prev === 0 || prev === 91 || prev === 123 || prev === 44 || prev === 58 || prev === 61;

function skipQuotedSpan(bin, start) {
  const q = bin.charCodeAt(start);
  let i = start + 1;
  while (i < bin.length && bin.charCodeAt(i) !== 10) {
    const c = bin.charCodeAt(i);
    if (q === 34 && c === 92) i += 2;
    else {
      if (c === q) return i + 1;
      i += 1;
    }
  }
  return null;
}

function bracketCloseIndex(bin, start) {
  let depth = 0;
  let i = start;
  let prev = 0;
  while (i < bin.length && bin.charCodeAt(i) !== 10) {
    const c = bin.charCodeAt(i);
    if (c === 32 || c === 9 || c === 13) i += 1;
    else if (c === 39 || c === 34) {
      const opens = quoteOpensSpan(prev);
      prev = c;
      if (opens) {
        i = skipQuotedSpan(bin, i);
        if (i === null) return null;
      } else i += 1;
    } else if (c === 91 || c === 123) {
      depth += 1;
      prev = c;
      i += 1;
    } else if (c === 93 || c === 125) {
      depth -= 1;
      prev = c;
      i += 1;
      if (depth === 0) return i - 1;
    } else {
      prev = c;
      i += 1;
    }
  }
  return null;
}

function restIsCommentOnly(bin, from) {
  let i = from;
  let sawSpace = false;
  while (i < bin.length) {
    const c = bin.charCodeAt(i);
    if (c === 32 || c === 9 || c === 13) {
      sawSpace = true;
      i += 1;
    } else break;
  }
  if (i >= bin.length || bin.charCodeAt(i) === 10) return true;
  return sawSpace && bin.charCodeAt(i) === 35;
}

function classifyBracketCommit(bin, start) {
  const close = bracketCloseIndex(bin, start);
  if (close === null) return "unclosed";
  return restIsCommentOnly(bin, close + 1) ? "flow" : "bare_trailing";
}

function flowRestIsTerminator(bin, from) {
  let i = from;
  while (i < bin.length && (bin.charCodeAt(i) === 32 || bin.charCodeAt(i) === 9 || bin.charCodeAt(i) === 13)) i += 1;
  if (i >= bin.length) return true;
  const c = bin.charCodeAt(i);
  return c === 44 || c === 93 || c === 125 || c === 10 || c === 35;
}

function classifyFlowBracket(bin, start) {
  let depth = 0;
  let i = start;
  let prev = 0;
  while (i < bin.length) {
    const c = bin.charCodeAt(i);
    if (c === 32 || c === 9 || c === 13 || c === 10) i += 1;
    else if (c === 39 || c === 34) {
      const opens = quoteOpensSpan(prev);
      prev = c;
      if (opens) {
        i = skipQuotedSpan(bin, i);
        if (i === null) return "flow";
      } else i += 1;
    } else if (c === 91 || c === 123) {
      depth += 1;
      prev = c;
      i += 1;
    } else if (c === 93 || c === 125) {
      depth -= 1;
      prev = c;
      i += 1;
      if (depth === 0) return flowRestIsTerminator(bin, i) ? "flow" : "bare_trailing";
    } else {
      prev = c;
      i += 1;
    }
  }
  return "flow";
}

// ── the parser ────────────────────────────────────────────────────────────
// A line loop over an intermediate tree of containers, entries and nodes,
// converted to the wire's tree at the end, as the compiled parser does;
// `pos` is a byte offset and is where a refusal points unless a site pins
// another.

const fail = (message, at) => fig.fail(message, at);

const newContainer = () => ({ kind: "undecided", closed: false, bornOfAppend: false, entries: [], index: new Map(), elements: [], style: "undecided", reentries: [] });
const newNode = (value, span) => ({ value, span: span ?? [0, 0], leading: [], dangling: [], trailing: null, marker: null });

const trimSpan = (bin, s, e) => {
  while (s < e) {
    const c = bin.charCodeAt(s);
    if (c !== 32 && c !== 9 && c !== 13) break;
    s += 1;
  }
  while (e > s) {
    const c = bin.charCodeAt(e - 1);
    if (c !== 32 && c !== 9 && c !== 13) break;
    e -= 1;
  }
  return [s, e];
};

// One `\…` escape at `i` in bin string `s`; the decoded text and the index
// past it. `at` is where a bad one is reported.
function decodeEscape(s, i, at) {
  if (i + 1 >= s.length) fail(E.FigBadEscape, at);
  const e = s[i + 1];
  if (e === "n") return ["\n", i + 2];
  if (e === "t") return ["\t", i + 2];
  if (e === "r") return ["\r", i + 2];
  if (e === "\\") return ["\\", i + 2];
  if (e === '"') return ['"', i + 2];
  if (e === "'") return ["'", i + 2];
  if (e === "u") {
    if (i + 6 > s.length) fail(E.FigBadEscape, at);
    const hex = s.slice(i + 2, i + 6);
    if (!/^[0-9a-fA-F]{4}$/.test(hex)) fail(E.FigBadEscape, at);
    const cp = parseInt(hex, 16);
    if (fig.isSurrogate(cp)) fail(E.FigBadEscape, at);
    return [String.fromCodePoint(cp), i + 6];
  }
  fail(E.FigBadEscape, at);
}

const normalizeDecimal = (text) => {
  let body = text;
  if (body[0] === "+" || body[0] === "-") body = body.slice(1);
  return /^[0-9]+$/.test(body) ? body : null;
};

// A ZON-style char literal `'A'`, `'\n'`, `'\u{1F600}'`: its codepoint, or
// null.
function parseCharLiteral(text) {
  if (text.length < 3 || text[0] !== "'" || text[text.length - 1] !== "'") return null;
  const inner = text.slice(1, -1);
  if (inner[0] !== "\\") {
    const cps = Array.from(inner);
    if (cps.length !== 1) return null;
    return cps[0].codePointAt(0);
  }
  const e = inner[1];
  if (inner.length === 2) {
    const simple = { n: 10, r: 13, t: 9, "\\": 92, "'": 39, '"': 34 };
    return simple[e] ?? null;
  }
  if (e === "x" && inner.length === 4 && /^[0-9a-fA-F]{2}$/.test(inner.slice(2))) return parseInt(inner.slice(2), 16);
  if (e === "u") {
    const m = inner.match(/^\\u\{([0-9a-fA-F]+)\}$/);
    if (!m || m[1].length > 6) return null;
    const cp = parseInt(m[1], 16);
    if (cp > 0x10ffff || fig.isSurrogate(cp)) return null;
    return cp;
  }
  return null;
}

class Parser {
  constructor(sc) {
    this.sc = sc;
    this.bin = sc.bin;
    this.n = sc.n;
    this.pos = 0;
    this.root = newContainer();
    this.root.isRoot = true;
    this.rootDangling = [];
    this.sections = G.sections(sc.bin);
    this.stack = [];
    this.lastAppendSteps = null;
    this.curLineStart = 0;
  }

  at(i) {
    return i < this.n ? this.bin.charCodeAt(i) : undefined;
  }
  peek() {
    return this.at(this.pos);
  }
  advance() {
    if (this.pos < this.n) this.pos += 1;
  }
  fail(name) {
    fail(E[name], this.pos);
  }
  failAt(at, name) {
    fail(E[name], at);
  }
  text(s, e) {
    return this.sc.slice(s, e);
  }
  skipSpacesTabs() {
    while (this.pos < this.n) {
      const c = this.at(this.pos);
      if (c !== 32 && c !== 9) break;
      this.pos += 1;
    }
  }
  isCrlfAt(pos) {
    return pos < this.n && this.at(pos) === 13 && (pos + 1 >= this.n || this.at(pos + 1) === 10);
  }
  atCrlf() {
    return this.isCrlfAt(this.pos);
  }
  atEndOfContent() {
    const p = this.peek();
    return p === undefined || p === 10 || p === 35 || this.atCrlf();
  }
  atTrueLineEnd() {
    const p = this.peek();
    return p === undefined || p === 10 || this.atCrlf();
  }
  skipToNextLine() {
    if (this.atCrlf()) this.pos += 1;
    if (this.pos < this.n && this.at(this.pos) === 10) this.pos += 1;
  }
  consumeLineEnd() {
    while (this.pos < this.n && this.at(this.pos) !== 10) this.pos += 1;
    this.skipToNextLine();
  }
  isTripleAt(pos, q) {
    return pos + 3 <= this.n && this.at(pos) === q && this.at(pos + 1) === q && this.at(pos + 2) === q;
  }

  // ── containers ──

  open(c) {
    if (c.closed) this.fail("FigClosedFlowValue");
    return c;
  }
  asMapping(c) {
    if (c.kind === "undecided") c.kind = "mapping";
    if (c.kind === "sequence") this.fail("FigMixedContainerChildren");
    return c;
  }
  asSequence(c) {
    if (c.kind === "undecided") c.kind = "sequence";
    if (c.kind === "mapping") this.fail("FigMixedContainerChildren");
    return c;
  }
  markElement(c) {
    if (c.style === "undecided") c.style = "element";
    else if (c.style === "addressed") this.fail("FigMixedSequenceAddressing");
  }
  markAddressed(c) {
    if (c.style === "undecided") c.style = "addressed";
    else if (c.style === "element") this.fail("FigMixedSequenceAddressing");
  }

  // ── quoted strings ──

  scanSingleQuoted(start) {
    const body = start + 1;
    let i = body;
    while (i < this.n) {
      const c = this.at(i);
      if (c === 39) return [this.text(body, i), i + 1];
      if (c === 10) fail(E.FigUnclosedString, this.pos);
      i += 1;
    }
    fail(E.FigUnclosedString, this.pos);
  }

  scanDoubleQuoted(start) {
    const parts = [];
    let i = start + 1;
    let from = i;
    while (i < this.n) {
      const c = this.at(i);
      if (c === 10) fail(E.FigUnclosedString, this.pos);
      if (c === 34) {
        parts.push(this.text(from, i));
        return [parts.join(""), i + 1];
      }
      if (c === 92) {
        parts.push(this.text(from, i));
        const [t, nxt] = decodeEscape(this.bin, i, this.pos);
        parts.push(t);
        i = nxt;
        from = i;
      } else i += 1;
    }
    fail(E.FigUnclosedString, this.pos);
  }

  // `'''`/`"""` at `start`: the text, the index past the close, and the
  // opener line's comment if any.
  scanTriple(start, q, dedentAndEscape) {
    const bin = this.bin;
    let i = start + 3;
    let openerComment = null;
    while (i < this.n && this.at(i) !== 10) {
      if (this.at(i) === 35) {
        const cstart = i + 1;
        let j = cstart;
        while (j < this.n && this.at(j) !== 10) j += 1;
        openerComment = fig.trimComment(this.text(cstart, j));
        i = j;
        break;
      }
      i += 1;
    }
    if (i < this.n && this.at(i) === 10) i += 1;
    const bodyStart = i;
    let lineStart = bodyStart;
    let closePos = null;
    while (lineStart <= this.n) {
      const nl = bin.indexOf("\n", lineStart);
      const lineEnd = nl < 0 ? this.n : nl;
      let j = lineStart;
      while (j < lineEnd && (this.at(j) === 32 || this.at(j) === 9)) j += 1;
      if (j + 3 <= this.n && this.at(j) === q && this.at(j + 1) === q && this.at(j + 2) === q) {
        closePos = j;
        break;
      }
      if (lineEnd >= this.n) fail(E.FigUnclosedString, this.pos);
      lineStart = lineEnd + 1;
    }
    if (closePos === null) fail(E.FigUnclosedString, this.pos);
    const dedent = closePos - lineStart;
    let bodyEnd = lineStart;
    if (bodyEnd > bodyStart && this.at(bodyEnd - 1) === 10) bodyEnd -= 1;
    if (bodyEnd > bodyStart && this.at(bodyEnd - 1) === 13) bodyEnd -= 1;
    const body = bodyEnd >= bodyStart ? bin.slice(bodyStart, bodyEnd) : "";
    const out = [];
    let first = true;
    for (let rawLine of body.split("\n")) {
      if (!first) out.push("\n");
      first = false;
      let line = rawLine;
      if (line.endsWith("\r")) line = line.slice(0, -1);
      if (dedentAndEscape) {
        let stripped = 0;
        while (stripped < dedent && line.length > 0 && (line[0] === " " || line[0] === "\t")) {
          line = line.slice(1);
          stripped += 1;
        }
        let k = 0;
        let from = 0;
        while (k < line.length) {
          if (line[k] === "\\") {
            out.push(fig.fromBin(line.slice(from, k)));
            const [t, nxt] = decodeEscape(line, k, this.pos);
            out.push(t);
            k = nxt;
            from = k;
          } else k += 1;
        }
        out.push(fig.fromBin(line.slice(from)));
      } else {
        out.push(fig.fromBin(line));
      }
    }
    return [out.join(""), closePos + 3, openerComment];
  }

  // ── comments and frames ──

  // The waiting comments from lines at `depth` (0: all of them) or deeper,
  // as text: what the intermediate tree keeps of a comment.
  drainPending(depth = 0) {
    return this.sections.take(depth).map((c) => c.text);
  }

  closeFramesAbove(depth) {
    while (this.stack.length > 0 && this.stack[this.stack.length - 1].childDepth > depth) {
      const frame = this.stack.pop();
      if (frame.container.kind === "undecided") this.fail("FigEmptyContainer");
      if (frame.container.bornOfAppend && frame.container.entries.length === 0) this.fail("FigEmptyContainer");
      frame.owner.dangling.push(...this.drainPending(frame.childDepth));
    }
  }

  // ── markers ──

  scanMarkers() {
    let count = 0;
    let broke = false;
    while (this.peek() === 62) {
      count += 1;
      this.advance();
      while (this.peek() === 32 || this.peek() === 9) {
        const save = this.pos;
        this.skipSpacesTabs();
        if (this.peek() === 42) {
          broke = true;
          break;
        }
        if (this.peek() !== 62) {
          this.pos = save;
          break;
        }
      }
      if (broke) break;
    }
    if (count > 0 && this.peek() === 45) this.fail("FigForeignSyntaxDash");
    if (this.peek() === 42) {
      const starSpan = [this.pos, this.pos + 1];
      this.advance();
      if (this.peek() === 32 || this.peek() === 9) this.skipSpacesTabs();
      else if (this.peek() === 58) {
        // `*: type = v`: the `:` binds to the `*`, no separator needed.
      } else if (!this.atEndOfContent()) this.fail("FigBadMarkerSeparator");
      return [count, true, starSpan];
    }
    if (count > 0) {
      if (this.peek() === 32 || this.peek() === 9) this.skipSpacesTabs();
      else if (!this.atEndOfContent()) this.fail("FigBadMarkerSeparator");
    }
    return [count, false, null];
  }

  // ── key paths ──

  scanKeySeg() {
    const c = this.peek();
    if (c === undefined) this.fail("FigBadKey");
    const start = this.pos;
    if (c === 34) {
      const [t, e] = this.scanDoubleQuoted(this.pos);
      this.pos = e;
      return { name: t, span: [start, e] };
    }
    if (c === 39) {
      const [t, e] = this.scanSingleQuoted(this.pos);
      this.pos = e;
      return { name: t, span: [start, e] };
    }
    while (this.pos < this.n && isBareKeyChar(this.at(this.pos))) this.pos += 1;
    if (this.pos === start) this.fail("FigBadKey");
    const text = this.bin.slice(start, this.pos);
    const f = text.charCodeAt(0);
    if (f === 45 || f === 62) this.fail("FigBadKey");
    return { name: text, span: [start, this.pos] };
  }

  scanIndexSeg() {
    this.advance(); // [
    if (this.peek() === 93) {
      this.advance();
      return { index: "append" };
    }
    const start = this.pos;
    while (this.pos < this.n && isDigit(this.at(this.pos))) this.pos += 1;
    if (this.pos === start || this.peek() !== 93) this.fail("FigBadKey");
    const n = parseInt(this.bin.slice(start, this.pos), 10);
    this.advance();
    return { index: n };
  }

  scanKeyPath() {
    const steps = [];
    for (;;) {
      steps.push({ key: this.scanKeySeg() });
      while (this.peek() === 91) steps.push(this.scanIndexSeg());
      if (this.peek() === 46) this.advance();
      else break;
    }
    return steps;
  }

  // ── path navigation ──

  getOrCreateMapContainer(m, seg) {
    const e = m.index.get(seg.name);
    if (e) {
      if (e.value.value.kind !== "container") this.fail("FigKeyNotContainer");
      return this.open(e.value.value.container);
    }
    const child = newContainer();
    const entry = { key: seg.name, keySpan: seg.span, sepSpan: [seg.span[1], seg.span[1]], keyLeading: [], value: newNode({ kind: "container", container: child }, [seg.span[0], seg.span[1]]) };
    m.entries.push(entry);
    m.index.set(entry.key, entry);
    return child;
  }

  getOrCreateSeqContainer(s, idx) {
    if (idx === "append") {
      if (s.elements.length === 0) this.fail("FigEmptyAppendTarget");
      const last = s.elements[s.elements.length - 1];
      if (last.value.kind !== "container") this.fail("FigKeyNotContainer");
      return this.open(last.value.container);
    }
    if (idx < s.elements.length) {
      const el = s.elements[idx];
      if (el.value.kind !== "container") this.fail("FigKeyNotContainer");
      return this.open(el.value.container);
    } else if (idx === s.elements.length) {
      const child = newContainer();
      s.elements.push(newNode({ kind: "container", container: child }, [this.curLineStart, this.curLineStart]));
      return child;
    }
    this.fail("FigIndexSkipped");
  }

  navigateIntermediate(start, steps) {
    let cur = start;
    for (let i = 0; i < steps.length - 1; i++) {
      const step = steps[i];
      if (step.key) cur = this.getOrCreateMapContainer(this.asMapping(cur), step.key);
      else {
        const s = this.asSequence(cur);
        this.markAddressed(s);
        cur = this.getOrCreateSeqContainer(s, step.index);
      }
    }
    return cur;
  }

  // A header line's final segment: the container to push, its owner node,
  // and its entry when reached through a key.
  resolveHeaderFinal(parent, last) {
    if (last.key) {
      const k = last.key;
      const m = this.asMapping(parent);
      const e = m.index.get(k.name);
      if (e) {
        if (e.value.value.kind !== "container") this.fail("FigDuplicateKey");
        const c = this.open(e.value.value.container);
        c.reentries.push({ at: k.span[0], name: k.span });
        return [c, e.value, e];
      }
      const child = newContainer();
      const entry = { key: k.name, keySpan: k.span, sepSpan: [k.span[1], k.span[1]], keyLeading: [], value: newNode({ kind: "container", container: child }, [k.span[0], k.span[1]]) };
      m.entries.push(entry);
      m.index.set(entry.key, entry);
      return [child, entry.value, entry];
    }
    const s = this.asSequence(parent);
    this.markAddressed(s);
    const idx = last.index;
    if (idx === "append") {
      const child = newContainer();
      child.bornOfAppend = true;
      child.kind = "mapping";
      const el = newNode({ kind: "container", container: child }, [this.curLineStart, this.curLineStart]);
      s.elements.push(el);
      return [child, el, null];
    }
    if (idx < s.elements.length) {
      const el = s.elements[idx];
      if (el.value.kind !== "container") this.fail("FigKeyNotContainer");
      const c = this.open(el.value.container);
      c.reentries.push({ at: this.curLineStart });
      return [c, el, null];
    } else if (idx === s.elements.length) {
      const child = newContainer();
      const el = newNode({ kind: "container", container: child }, [this.curLineStart, this.curLineStart]);
      s.elements.push(el);
      return [child, el, null];
    }
    this.fail("FigIndexSkipped");
  }

  // ── values ──

  // A bare RHS to the end of the line, the `#`-after-whitespace rule
  // honoured; `pos` is left at the newline. Returns [text, span, comment].
  scanBareRestOfLine() {
    const start = this.pos;
    let i = start;
    let prevSpace = true;
    let commentStart = null;
    while (i < this.n && this.at(i) !== 10) {
      const ch = this.at(i);
      if (ch === 35 && prevSpace) {
        commentStart = i;
        break;
      }
      prevSpace = ch === 32 || ch === 9;
      i += 1;
    }
    const valueEnd = commentStart ?? i;
    const [ts, te] = trimSpan(this.bin, start, valueEnd);
    let comment = null;
    let lineEnd = valueEnd;
    if (commentStart !== null) {
      let j = commentStart + 1;
      while (j < this.n && this.at(j) !== 10) j += 1;
      comment = fig.trimComment(this.text(commentStart + 1, j));
      lineEnd = j;
    }
    this.pos = lineEnd;
    return [this.text(ts, te), [ts, te], comment];
  }

  scanTrailingCommentOnly() {
    this.skipSpacesTabs();
    if (this.pos >= this.n || this.at(this.pos) === 10 || this.atCrlf()) return null;
    if (this.at(this.pos) === 35) {
      const cs = this.pos + 1;
      let j = cs;
      while (j < this.n && this.at(j) !== 10) j += 1;
      const c = fig.trimComment(this.text(cs, j));
      this.pos = j;
      return c;
    }
    this.fail("FigTrailingContent");
  }

  scanFlowOpenerComment() {
    let i = this.pos + 1;
    while (i < this.n && (this.at(i) === 32 || this.at(i) === 9)) i += 1;
    if (i >= this.n || this.at(i) !== 35) return null;
    let j = i + 1;
    while (j < this.n && this.at(j) !== 10) j += 1;
    return fig.trimComment(this.text(i + 1, j));
  }

  applyKnownType(typeName, text, span) {
    switch (typeName) {
      case "int": {
        const n = sniffNumber(text);
        if (n) return n === "int" ? newNode({ kind: "int", text, tag: "!!int" }, span) : null;
        return normalizeDecimal(text) ? newNode({ kind: "int", text, tag: "!!int" }, span) : null;
      }
      case "float": {
        if (text === "inf" || text === "-inf" || text === "nan") return newNode({ kind: "string", text, extKind: "number_special" }, span);
        if (sniffNumber(text)) return newNode({ kind: "float", text, tag: "!!float" }, span);
        if (text.endsWith(".") && text.length >= 2) return normalizeDecimal(text.slice(0, -1)) ? newNode({ kind: "float", text, tag: "!!float" }, span) : null;
        return normalizeDecimal(text) ? newNode({ kind: "float", text, tag: "!!float" }, span) : null;
      }
      case "bool":
        return text === "true" || text === "false" ? newNode({ kind: "bool", text, tag: "!!bool" }, span) : null;
      case "enum":
        return text === "" ? null : newNode({ kind: "string", text, extKind: "enum_literal" }, span);
      case "char": {
        const cp = parseCharLiteral(text);
        return cp === null ? null : newNode({ kind: "int", text: String(cp), extKind: "char_literal" }, span);
      }
      case "datetime":
      case "date":
      case "time": {
        const k = DT.classify(text, DATETIME);
        if (!k) return null;
        if (typeName === "date" && k !== "local_date") return null;
        if (typeName === "time" && k !== "local_time") return null;
        return newNode({ kind: "string", text, extKind: k }, span);
      }
      default:
        return "unknown";
    }
  }

  parseAssignedValue(typeName) {
    this.skipSpacesTabs();
    if (typeName !== null) {
      if (typeName === "string") {
        const node = this.parseUntypedValue(true);
        node.value.tag = "!!str";
        return node;
      }
      const [text, span, comment] = this.scanBareRestOfLine();
      if (text === "") this.fail("FigInvalidValue");
      const node = this.applyKnownType(typeName, text, span);
      if (node === "unknown") fail(E.FigUnknownType, span[0]);
      if (node === null) fail(E.FigTypeMismatch, span[0]);
      node.trailing = comment;
      return node;
    }
    return this.parseUntypedValue(false);
  }

  sniffToNode(text, span) {
    const s = sniffBare(text);
    if (s.kind === "null") return newNode({ kind: "null" }, span);
    if (s.extKind) return newNode({ kind: "string", text: s.text, extKind: s.extKind }, span);
    return newNode({ kind: s.kind, text: s.text }, span);
  }

  parseUntypedValue(forceStringBare) {
    if (forceStringBare) {
      const [text, span, comment] = this.scanBareRestOfLine();
      if (text === "") this.fail("FigInvalidValue");
      const node = newNode({ kind: "string", text }, span);
      node.trailing = comment;
      return node;
    }
    const c = this.peek();
    if (c === undefined) this.fail("FigInvalidValue");
    if (c === 39 || c === 34) return this.parseQuotedOrTriple(c);
    if (c === 91 || c === 123) {
      if (classifyBracketCommit(this.bin, this.pos) === "bare_trailing") {
        const [text, span, comment] = this.scanBareRestOfLine();
        if (text === "") this.fail("FigInvalidValue");
        const node = newNode({ kind: "string", text }, span);
        node.trailing = comment;
        return node;
      }
      const openerComment = this.scanFlowOpenerComment();
      const node = this.parseFlowValue();
      const trailing = this.scanTrailingCommentOnly();
      node.trailing = openerComment ?? trailing;
      return node;
    }
    const [text, span, comment] = this.scanBareRestOfLine();
    if (text === "") this.fail("FigInvalidValue");
    const node = this.sniffToNode(text, span);
    node.trailing = comment;
    return node;
  }

  parseQuotedOrTriple(q) {
    const start = this.pos;
    if (this.isTripleAt(this.pos, q)) {
      let j = this.pos + 3;
      while (j < this.n && (this.at(j) === 32 || this.at(j) === 9)) j += 1;
      if (j < this.n && this.at(j) !== 10 && this.at(j) !== 35 && !this.isCrlfAt(j)) this.failAt(j, "FigMultilineOpenerContent");
      const [text, e, openerComment] = this.scanTriple(this.pos, q, q === 34);
      this.pos = e;
      const node = newNode({ kind: "string", text }, [start, e]);
      const trailing = this.scanTrailingCommentOnly();
      node.trailing = openerComment ?? trailing;
      return node;
    }
    const [text, e] = q === 39 ? this.scanSingleQuoted(this.pos) : this.scanDoubleQuoted(this.pos);
    this.pos = e;
    const node = newNode({ kind: "string", text }, [start, e]);
    // Stray content after the closing quote is the wrapped-bare-string
    // slip, with its own message.
    this.skipSpacesTabs();
    if (!(this.pos >= this.n || this.at(this.pos) === 10 || this.atCrlf()) && this.at(this.pos) !== 35) this.fail("FigQuotedTrailingContent");
    node.trailing = this.scanTrailingCommentOnly();
    return node;
  }

  // ── flow ──

  skipFlowWs() {
    while (this.pos < this.n) {
      const c = this.at(this.pos);
      if (c === 32 || c === 9 || c === 13 || c === 10) this.pos += 1;
      else if (c === 35) {
        while (this.pos < this.n && this.at(this.pos) !== 10) this.pos += 1;
      } else return;
    }
  }

  parseFlowValue() {
    return this.peek() === 91 ? this.parseFlowArray() : this.parseFlowObject();
  }

  parseFlowArray() {
    const open = this.pos;
    this.advance();
    const seq = newContainer();
    seq.closed = true;
    seq.kind = "sequence";
    this.skipFlowWs();
    if (this.peek() === 93) {
      this.advance();
      return newNode({ kind: "container", container: seq }, [open, this.pos]);
    }
    for (;;) {
      seq.elements.push(this.parseFlowScalarOrNested());
      this.skipFlowWs();
      const p = this.peek();
      if (p === undefined) this.fail("FigUnclosedFlow");
      if (p === 44) {
        this.advance();
        this.skipFlowWs();
        if (this.peek() === 93) {
          this.advance();
          break;
        }
      } else if (p === 93) {
        this.advance();
        break;
      } else this.fail("FigUnclosedFlow");
    }
    return newNode({ kind: "container", container: seq }, [open, this.pos]);
  }

  parseFlowKey() {
    const c = this.peek();
    if (c === undefined) this.fail("FigBadKey");
    const start = this.pos;
    if (c === 34) {
      const [t, e] = this.scanDoubleQuoted(this.pos);
      this.pos = e;
      return { text: t, quoted: true, span: [start, e] };
    }
    if (c === 39) {
      const [t, e] = this.scanSingleQuoted(this.pos);
      this.pos = e;
      return { text: t, quoted: true, span: [start, e] };
    }
    const stop = (ch) => ch === 44 || ch === 93 || ch === 125 || ch === 58 || ch === 61 || ch === 32 || ch === 9 || ch === 10 || ch === 13;
    while (this.pos < this.n && !stop(this.at(this.pos))) this.pos += 1;
    if (this.pos === start) this.fail("FigBadKey");
    return { text: this.text(start, this.pos), quoted: false, span: [start, this.pos] };
  }

  parseFlowObject() {
    const open = this.pos;
    this.advance();
    const map = newContainer();
    map.closed = true;
    map.kind = "mapping";
    this.skipFlowWs();
    if (this.peek() === 125) {
      this.advance();
      return newNode({ kind: "container", container: map }, [open, this.pos]);
    }
    let mode = null;
    for (;;) {
      const key = this.parseFlowKey();
      this.skipFlowWs();
      const sep = this.peek();
      if (sep === undefined) this.fail("FigUnclosedFlow");
      let thisMode;
      if (sep === 61) thisMode = "fig";
      else if (sep === 58) {
        if (!key.quoted) this.fail("FigFlowBareKeyColon");
        thisMode = "json";
      } else this.fail("FigUnclosedFlow");
      if (mode !== null && mode !== thisMode) this.fail("FigMixedFlowSeparators");
      mode = thisMode;
      this.advance();
      this.skipFlowWs();
      const v = this.parseFlowScalarOrNested();
      if (map.index.has(key.text)) this.fail("FigDuplicateKey");
      const entry = { key: key.text, keySpan: key.span, sepSpan: null, keyLeading: [], value: v };
      map.entries.push(entry);
      map.index.set(key.text, entry);
      this.skipFlowWs();
      const p = this.peek();
      if (p === undefined) this.fail("FigUnclosedFlow");
      if (p === 44) {
        this.advance();
        this.skipFlowWs();
        if (this.peek() === 125) {
          this.advance();
          break;
        }
      } else if (p === 125) {
        this.advance();
        break;
      } else this.fail("FigUnclosedFlow");
    }
    return newNode({ kind: "container", container: map }, [open, this.pos]);
  }

  scanFlowBareValue() {
    const start = this.pos;
    let prevSpace = false;
    while (this.pos < this.n) {
      const ch = this.at(this.pos);
      if (ch === 44 || ch === 93 || ch === 125 || ch === 10) break;
      if (ch === 35 && prevSpace) break;
      prevSpace = ch === 32 || ch === 9;
      this.pos += 1;
    }
    const [ts, te] = trimSpan(this.bin, start, this.pos);
    return [this.text(ts, te), [ts, te]];
  }

  scanFlowBareBracket() {
    const start = this.pos;
    let depth = 0;
    let prevSpace = false;
    while (this.pos < this.n) {
      const ch = this.at(this.pos);
      if (ch === 91 || ch === 123) depth += 1;
      else if (ch === 93 || ch === 125) {
        if (depth === 0) break;
        depth -= 1;
      } else if (ch === 44 || ch === 10) {
        if (depth === 0) break;
      } else if (ch === 35) {
        if (prevSpace && depth === 0) break;
      }
      prevSpace = ch === 32 || ch === 9;
      this.pos += 1;
    }
    const [ts, te] = trimSpan(this.bin, start, this.pos);
    return [this.text(ts, te), [ts, te]];
  }

  parseFlowScalarOrNested() {
    this.skipFlowWs();
    const c = this.peek();
    if (c === undefined) this.fail("FigUnclosedFlow");
    if (c === 91 || c === 123) {
      if (classifyFlowBracket(this.bin, this.pos) === "bare_trailing") {
        const [text, span] = this.scanFlowBareBracket();
        if (text === "") this.fail("FigInvalidValue");
        return newNode({ kind: "string", text }, span);
      }
      return this.parseFlowValue();
    }
    if (c === 34 || c === 39) {
      const start = this.pos;
      const [text, e] = c === 34 ? this.scanDoubleQuoted(this.pos) : this.scanSingleQuoted(this.pos);
      this.pos = e;
      return newNode({ kind: "string", text }, [start, e]);
    }
    const [text, span] = this.scanFlowBareValue();
    if (text === "") this.fail("FigInvalidValue");
    return this.sniffToNode(text, span);
  }

  // ── lines ──

  finishHeader(target, steps, depth) {
    if (!this.atEndOfContent()) this.fail("FigTrailingContent");
    const leading = this.drainPending();
    const parent = this.navigateIntermediate(target, steps);
    const last = steps[steps.length - 1];
    const [container, owner, entry] = this.resolveHeaderFinal(parent, last);
    if (entry) entry.keyLeading.push(...leading);
    else owner.leading.push(...leading);
    const cm = this.scanTrailingCommentOnly();
    if (cm !== null) owner.trailing = cm;
    this.consumeLineEnd();
    this.stack.push({ container, childDepth: depth + 1, owner });
    if (depth === 0 && last.index === "append") this.lastAppendSteps = steps;
  }

  finishAssignment(target, steps, typeName, sep) {
    this.skipSpacesTabs();
    const leading = this.drainPending();
    const parent = this.navigateIntermediate(target, steps);
    const last = steps[steps.length - 1];
    const valueNode = this.parseAssignedValue(typeName);
    this.consumeLineEnd();
    if (last.key) {
      const k = last.key;
      const m = this.asMapping(parent);
      if (m.index.has(k.name)) fail(E.FigDuplicateKey, k.span[0]);
      const entry = { key: k.name, keySpan: k.span, sepSpan: sep, keyLeading: [...leading], value: valueNode };
      m.entries.push(entry);
      m.index.set(k.name, entry);
    } else {
      const s = this.asSequence(parent);
      this.markAddressed(s);
      valueNode.leading.push(...leading);
      const idx = last.index;
      if (idx === "append") this.fail("FigAppendAssignment");
      if (idx < s.elements.length) this.fail("FigIndexAlreadySet");
      if (idx > s.elements.length) this.fail("FigIndexSkipped");
      s.elements.push(valueNode);
    }
  }

  scanTypeName() {
    const colonPos = this.pos;
    this.advance();
    this.skipSpacesTabs();
    const typeStart = this.pos;
    while (this.pos < this.n && isBareKeyChar(this.at(this.pos))) this.pos += 1;
    const typeName = this.bin.slice(typeStart, this.pos);
    if (typeName === "") this.fail("FigBadKey");
    this.skipSpacesTabs();
    if (this.peek() !== 61) this.failAt(colonPos, "FigForeignSyntaxColon");
    return typeName;
  }

  parseKeyLine(target, depth) {
    const steps = this.scanKeyPath();
    this.skipSpacesTabs();
    if (this.peek() === 58) {
      const typeName = this.scanTypeName();
      const eq = [this.pos, this.pos + 1];
      this.advance();
      this.finishAssignment(target, steps, typeName, eq);
    } else if (this.peek() === 61) {
      const eq = [this.pos, this.pos + 1];
      this.advance();
      this.finishAssignment(target, steps, null, eq);
    } else this.finishHeader(target, steps, depth);
  }

  parseElementLine(target, depth, star) {
    const bodyStart = this.pos;
    this.skipSpacesTabs();
    const leading = this.drainPending();
    const seq = this.asSequence(target);
    this.markElement(seq);
    if (this.atEndOfContent()) {
      const child = newContainer();
      const el = newNode({ kind: "container", container: child }, [bodyStart, bodyStart]);
      el.marker = star;
      el.leading.push(...leading);
      seq.elements.push(el);
      const cm = this.scanTrailingCommentOnly();
      if (cm !== null) el.trailing = cm;
      this.consumeLineEnd();
      this.stack.push({ container: child, childDepth: depth + 1, owner: el });
      return;
    }
    if (this.peek() === 58) {
      const typeName = this.scanTypeName();
      this.advance();
      const node = this.parseAssignedValue(typeName);
      node.marker = star;
      this.consumeLineEnd();
      node.leading.push(...leading);
      seq.elements.push(node);
      return;
    }
    const c = this.peek();
    if (c === 39 || c === 34 || c === 91 || c === 123) {
      const node = this.parseUntypedValue(false);
      node.marker = star;
      this.consumeLineEnd();
      node.leading.push(...leading);
      seq.elements.push(node);
      return;
    }
    // A bare element: the no-inline-field guardrail on the raw rest of the
    // line, before the sniff.
    let k = this.pos;
    while (k < this.n && this.at(k) !== 10) k += 1;
    if (this.bin.slice(this.pos, k).includes(" = ")) this.fail("FigElementInlineField");
    const node = this.parseUntypedValue(false);
    node.marker = star;
    this.consumeLineEnd();
    node.leading.push(...leading);
    seq.elements.push(node);
  }

  isPlusLine() {
    const j = this.pos + 1;
    if (j >= this.n) return true;
    const c = this.at(j);
    return c === 32 || c === 9 || c === 13 || c === 10 || c === 35;
  }

  parseContinuationLine(depth) {
    this.advance(); // +
    if (depth !== 0) this.fail("FigDanglingContinuation");
    const steps = this.lastAppendSteps;
    if (steps === null) this.fail("FigDanglingContinuation");
    this.skipSpacesTabs();
    if (!this.atEndOfContent()) this.fail("FigTrailingContent");
    const leading = this.drainPending();
    const parent = this.navigateIntermediate(this.root, steps);
    const [container, owner] = this.resolveHeaderFinal(parent, steps[steps.length - 1]);
    owner.leading.push(...leading);
    const cm = this.scanTrailingCommentOnly();
    if (cm !== null) owner.trailing = cm;
    this.consumeLineEnd();
    this.stack.push({ container, childDepth: 1, owner });
  }

  processContentLine(depth, starSpan) {
    const star = starSpan !== null;
    this.closeFramesAbove(depth);
    const required = this.stack.length === 0 ? 0 : this.stack[this.stack.length - 1].childDepth;
    if (depth !== required) this.fail(required === 0 ? "FigRootMarker" : "FigSkippedLevel");
    const target = this.stack.length === 0 ? this.root : this.stack[this.stack.length - 1].container;
    const isPlus = !star && this.peek() === 43 && this.isPlusLine();
    if (depth === 0 && !isPlus) this.lastAppendSteps = null;
    if (isPlus) return this.parseContinuationLine(depth);
    if (star) return this.parseElementLine(target, depth, starSpan);
    const c = this.peek();
    if (c === 45) this.fail("FigForeignSyntaxDash");
    if (c === 91) this.fail("FigForeignSyntaxBracket");
    this.parseKeyLine(target, depth);
  }

  processLine() {
    this.skipSpacesTabs();
    const [depth, star, starSpan] = this.scanMarkers();
    if (!star && this.atTrueLineEnd()) {
      this.skipToNextLine();
      return;
    }
    if (!star && this.peek() === 35) {
      this.advance();
      let j = this.pos;
      while (j < this.n && this.at(j) !== 10) j += 1;
      this.sections.comment(fig.trimComment(this.text(this.pos, j)), depth);
      this.pos = j;
      this.skipToNextLine();
      return;
    }
    this.processContentLine(depth, starSpan);
  }
}

// ── the tree, as the wire sees it ──
// Containers widen to their last child's end here, so every span below is
// final before the one above it is read.

// A block container is a section: the line that created it is its first
// region, and every header that re-entered it by its final segment adds a
// region and, when the segment is a key, a mention.
function isSection(node) {
  const v = node.value;
  return v.kind === "container" && !v.container.closed && !v.container.isRoot;
}

function reenter(S, built, c) {
  for (const r of c.reentries) {
    if (r.name) S.reopen(built, r.name, "entry");
    else S.region(built, r.at);
  }
}

function buildNode(S, node) {
  const v = node.value;
  let built;
  if (v.kind === "container") {
    const c = v.container;
    let end = 0;
    if (c.kind === "undecided") fail(E.FigEmptyContainer, 0);
    if (c.kind === "mapping") {
      built = fig.mapping(undefined, { duplicates: "keep" });
      for (const e of c.entries) {
        const key = fig.scalar("string", e.keySpan, e.key);
        for (const t of e.keyLeading) key.comment("leading", t);
        const value = buildNode(S, e.value);
        const kv = fig.entry(key, value, [e.keySpan[0], e.value.span[1]]);
        if (e.sepSpan) kv.sep = e.sepSpan;
        if (e.value.span[1] > end) end = e.value.span[1];
        if (isSection(e.value)) {
          S.open(built, kv, "entry");
          reenter(S, value, e.value.value.container);
        } else {
          built.put(kv);
        }
      }
    } else {
      built = fig.sequence(undefined);
      for (const el of c.elements) {
        const item = buildNode(S, el);
        if (el.span[1] > end) end = el.span[1];
        if (el.marker) item.marker = el.marker;
        for (const t of el.leading) item.comment("leading", t);
        built.add(item);
        if (isSection(el)) {
          S.region(item, el.span[0]);
          reenter(S, item, el.value.container);
        }
      }
    }
    if (end > node.span[1]) node.span[1] = end;
    built.span = [node.span[0], node.span[1]];
  } else {
    const extra = {};
    if (v.extKind) extra.ext_kind = v.extKind;
    if (v.tag) extra.tag = v.tag;
    built = fig.scalar(v.kind, [node.span[0], node.span[1]], v.text, extra);
  }
  if (node.trailing !== null && node.trailing !== undefined) built.comment("trailing", node.trailing);
  for (const t of node.dangling) built.comment("dangling", t);
  return built;
}

function parseDocument(input) {
  const sc = fig.scanner(input);
  const p = new Parser(sc);
  if (sc.bin.startsWith("\xef\xbb\xbf")) p.pos = 3;
  while (p.pos < p.n) {
    p.curLineStart = p.pos;
    p.processLine();
  }
  p.closeFramesAbove(0);
  p.rootDangling.push(...p.drainPending());
  // An empty document is an empty mapping.
  if (p.root.kind === "undecided") p.root.kind = "mapping";
  const rootWrap = newNode({ kind: "container", container: p.root }, [0, 0]);
  rootWrap.dangling = p.rootDangling;
  return buildNode(p.sections, rootWrap);
}

function parse(_dialect, input) {
  return fig.rows(parseDocument(input));
}

// ── the printer ───────────────────────────────────────────────────────────
// The compiled printer's house style, rule for rule: spaced marker runs,
// fits-or-breaks inline flow, dotted-key collapse, sections at the root
// with hoisting past depth 2, `path[]`/`+` append groups for lists of
// maps, multi-line flow for long scalar lists, comments as conservative
// blockers. Widths are byte widths, as the compiled printer counts them.

const MAX_BODY_DEPTH = 2;
const MAX_INLINE_SEQ_ITEMS = 6;
const ASSIGN_OP_WIDTH = 3;
const MARKER_CELL = 2;

const blen = (s) => fig.byteLength(s);

function isTotalSinkSafe(s) {
  if (s === "") return false;
  if (s[0] === " " || s[s.length - 1] === " ") return false;
  if (/[\n\r\t]/.test(s)) return false;
  let prevWs = true;
  for (const ch of s) {
    if (ch === "#" && prevWs) return false;
    prevWs = ch === " ";
  }
  return true;
}

function flowLeadingBracketBare(s) {
  if (classifyFlowBracket(fig.binOf(s), 0) !== "bare_trailing") return false;
  let depth = 0;
  let prevSpace = false;
  for (const ch of s) {
    if (ch === "[" || ch === "{") depth += 1;
    else if (ch === "]" || ch === "}") {
      if (depth === 0) return false;
      depth -= 1;
    } else if (ch === ",") {
      if (depth === 0) return false;
    } else if (ch === "#") {
      if (prevSpace && depth === 0) return false;
    }
    prevSpace = ch === " " || ch === "\t";
  }
  return depth === 0;
}

function isBareSafe(s, isKey, inFlow) {
  if (s === "") return false;
  if (s[0] === " " || s[s.length - 1] === " ") return false;
  for (const ch of s) {
    if (ch === "\n" || ch === "\r" || ch === "\t") return false;
    if (isKey && !(ch.length === 1 && ch.charCodeAt(0) < 128 && isBareKeyChar(ch.charCodeAt(0)))) return false;
  }
  if (isKey && (s[0] === "-" || s[0] === ">")) return false;
  if (!isKey) {
    const f = s[0];
    if (f === "'" || f === '"') return false;
    if (f === "[" || f === "{") {
      if (inFlow) return flowLeadingBracketBare(s);
      if (classifyBracketCommit(fig.binOf(s), 0) !== "bare_trailing") return false;
    }
    if (inFlow && /[,\]}]/.test(s)) return false;
    let prevWs = true;
    for (const ch of s) {
      if (ch === "#" && prevWs) return false;
      prevWs = ch === " " || ch === "\t";
    }
    const sn = sniffBare(s);
    if (sn.kind !== "string" || sn.extKind) return false;
  }
  return true;
}

const canRawQuote = (s) => !/['\n]/.test(s);

function quotedWidth(s) {
  if (canRawQuote(s)) return blen(s) + 2;
  let w = 2;
  for (const ch of s) w += ch === '"' || ch === "\\" || ch === "\n" || ch === "\r" || ch === "\t" ? 2 : blen(ch);
  return w;
}

const scalarStringWidth = (s, isKey, inFlow) => (isBareSafe(s, isKey, inFlow) ? blen(s) : quotedWidth(s));

const ESCAPES = { '"': '\\"', "\\": "\\\\", "\n": "\\n", "\r": "\\r", "\t": "\\t" };

function bareOrQuoted(s, isKey, inFlow) {
  if (isBareSafe(s, isKey, inFlow)) return s;
  if (canRawQuote(s)) return "'" + s + "'";
  return '"' + s.replace(/["\\\n\r\t]/g, (ch) => ESCAPES[ch]) + '"';
}

function multilineString(s) {
  if (!s.includes("'''") && !s.includes("\r")) return "'''\n" + s + "\n'''";
  return '"""\n' + s.replace(/[\\"\r]/g, (ch) => (ch === "\\" ? "\\\\" : ch === '"' ? '\\"' : "\\r")) + '\n"""';
}

function charLiteral(text) {
  if (!/^[0-9]+$/.test(text)) return text;
  const cp = parseInt(text, 10);
  if (cp === 39) return "'\\''";
  if (cp === 92) return "'\\\\'";
  if (cp === 10) return "'\\n'";
  if (cp === 13) return "'\\r'";
  if (cp === 9) return "'\\t'";
  if ((cp >= 0x20 && cp <= 0x26) || (cp >= 0x28 && cp <= 0x5b) || (cp >= 0x5d && cp <= 0x7e)) return "'" + String.fromCharCode(cp) + "'";
  return "'\\u{" + cp.toString(16) + "}'";
}

const isMultiline = (row) => row.kind === "string" && typeof row.text === "string" && row.text.includes("\n");

class Printer {
  constructor(options) {
    options ??= {};
    this.w = fig.writer(options);
    this.width = options.width ?? 80;
    this.strip = options.strip_comments === true;
    this.memo = new Map();
    this.path = [];
    this.started = false;
    this.prevMultiline = false;
  }

  commentsOn() {
    return !this.strip;
  }
  put(...parts) {
    this.w.put(...parts);
  }
  keyText(row) {
    if (row.kind !== "string") throw new Error("a fig key must be a string");
    return row.text ?? "";
  }
  keyWidth(row) {
    return scalarStringWidth(this.keyText(row), true, false);
  }
  leadingOf(row) {
    return this.commentsOn() ? row.leading : [];
  }
  trailingOf(row) {
    return this.commentsOn() ? (row.trailing[0] ?? null) : null;
  }
  danglingOf(row) {
    return this.commentsOn() ? row.dangling : [];
  }

  // The kind tag the annotation re-emits, or null: a `!!str` tag on a value
  // the total-sink form cannot carry is dropped.
  kindTag(row) {
    const k = row.tag;
    if (!k) return null;
    if (k === "!!str") return row.kind === "string" && !isTotalSinkSafe(row.text ?? "") ? null : "string";
    if (k === "!!int") return "integer";
    if (k === "!!float") return "float";
    if (k === "!!bool") return "boolean";
    return null;
  }

  inlineWidth(row, policy) {
    if (this.commentsOn()) {
      if (row.dangling.length > 0) return null;
      if (policy === "strict" && (row.leading.length > 0 || row.trailing.length > 0)) return null;
      if (policy === "allow_trailing" && row.leading.length > 0) return null;
    }
    return this.structuralWidth(row);
  }

  structuralWidth(row) {
    if (this.memo.has(row)) return this.memo.get(row);
    const w = this.computeInlineWidth(row);
    this.memo.set(row, w);
    return w;
  }

  computeInlineWidth(row) {
    if (this.kindTag(row) !== null) return null;
    const k = row.kind;
    if (k === "null") return 4;
    if (k === "bool") return row.text === "true" ? 4 : 5;
    if (k === "int" || k === "float") return row.ext_kind ? null : blen(row.text);
    if (k === "string") {
      const e = row.ext_kind;
      if (e) return DT.KINDS.has(e) ? blen(row.text) : null;
      if (typeof row.text === "string" && row.text.includes("\n")) return null;
      return scalarStringWidth(row.text ?? "", false, true);
    }
    if (k === "sequence") {
      let w = 2;
      for (let i = 0; i < row.items.length; i++) {
        const ew = this.inlineWidth(row.items[i], "strict");
        if (ew === null) return null;
        w += ew + (i > 0 ? 2 : 0);
      }
      return w;
    }
    if (k === "mapping") {
      if (row.items.length === 0) return 2;
      let w = 4;
      for (let i = 0; i < row.items.length; i++) {
        const e = row.items[i];
        if (this.commentsOn() && e.key.leading.length > 0) return null;
        if (e.value.kind === "mapping") return null;
        const vw = this.inlineWidth(e.value, "strict");
        if (vw === null) return null;
        w += this.keyWidth(e.key) + 3 + vw + (i > 0 ? 2 : 0);
      }
      return w;
    }
    return null;
  }

  fitsInline(row, prefixWidth, policy) {
    const w = this.inlineWidth(row, policy);
    return w !== null && prefixWidth + w <= this.width;
  }
  seqFitsInline(row, prefixWidth, policy) {
    return row.items.length <= MAX_INLINE_SEQ_ITEMS && this.fitsInline(row, prefixWidth, policy);
  }

  multilineFlowEligible(row, policy) {
    if (row.items.length === 0) return false;
    if (row.items.some((el) => el.kind === "mapping")) return false;
    if (flowQuotesWhatBlockLeavesBare(row)) return false;
    return this.inlineWidth(row, policy) !== null;
  }

  collapseChild(e) {
    const v = e.value;
    if (v.kind !== "mapping" || v.items.length !== 1) return null;
    if (this.commentsOn()) {
      if (e.key.leading.length > 0) return null;
      if (v.leading.length > 0 || v.trailing.length > 0 || v.dangling.length > 0) return null;
    }
    return v.items[0];
  }

  resolveChain(e) {
    let cur = e;
    let w = this.keyWidth(cur.key);
    for (;;) {
      const child = this.collapseChild(cur);
      if (!child) break;
      cur = child;
      w += 1 + this.keyWidth(cur.key);
    }
    return [cur, w];
  }

  chainKeys(e, endE) {
    const parts = [bareOrQuoted(this.keyText(e.key), true, false)];
    let cur = e;
    while (cur !== endE) {
      cur = this.collapseChild(cur);
      parts.push(bareOrQuoted(this.keyText(cur.key), true, false));
    }
    return parts.join(".");
  }

  blockDepthMap(row, depth) {
    let maxd = depth;
    for (const e of row.items) {
      const [endE, kw] = this.resolveChain(e);
      const v = endE.value;
      if (v.kind === "mapping") {
        if (!this.fitsInline(v, mapEntryPrefixWidth(depth, kw), "allow_trailing")) maxd = Math.max(maxd, this.blockDepthMap(v, depth + 1));
      } else if (v.kind === "sequence") {
        if (!this.fitsInline(v, mapEntryPrefixWidth(depth, kw), "allow_trailing") && !this.multilineFlowEligible(v, "allow_trailing"))
          maxd = Math.max(maxd, this.blockDepthSeq(v, depth + 1));
      }
    }
    return maxd;
  }

  blockDepthSeq(row, depth) {
    let maxd = depth;
    for (const el of row.items) {
      if (el.kind === "mapping") {
        if (!this.fitsInline(el, elementPrefixWidth(depth), "allow_leading_trailing")) maxd = Math.max(maxd, this.blockDepthMap(el, depth + 1));
      } else if (el.kind === "sequence") {
        if (!this.fitsInline(el, elementPrefixWidth(depth), "allow_leading_trailing") && !this.multilineFlowEligible(el, "allow_leading_trailing"))
          maxd = Math.max(maxd, this.blockDepthSeq(el, depth + 1));
      }
    }
    return maxd;
  }

  // ── writing ──

  markers(depth) {
    for (let i = 0; i < depth; i++) this.put("> ");
  }

  commentLines(c, depth) {
    for (const line of c.text.split("\n")) {
      this.markers(depth);
      this.put("#");
      const t = line.replace(/^[ \t]+|[ \t]+$/g, "");
      if (t !== "") this.put(" ", t);
      this.put("\n");
    }
  }

  leadingComments(row, depth) {
    for (const c of this.leadingOf(row)) this.commentLines(c, depth);
  }

  trailingComment(row) {
    const c = this.trailingOf(row);
    if (!c) return;
    this.put(" #");
    if (c.text !== "") this.put(" ", c.text.replace(/\n/g, " "));
  }

  typeAnnotation(row) {
    const k = this.kindTag(row);
    if (k === "integer") {
      this.put(": int");
      return true;
    }
    if (k === "float") {
      this.put(": float");
      return true;
    }
    if (k === "string") {
      this.put(": string");
      return true;
    }
    if (k === "boolean") {
      this.put(": bool");
      return true;
    }
    const e = row.ext_kind;
    if (e === "enum_literal") {
      this.put(": enum");
      return true;
    }
    if (e === "char_literal") {
      this.put(": char");
      return true;
    }
    if (e === "number_special") {
      this.put(": float");
      return true;
    }
    return false;
  }

  value(row, inFlow) {
    const k = row.kind;
    if (k === "null") this.put("null");
    else if (k === "bool") this.put(row.text);
    else if (k === "int" || k === "float") this.put(row.ext_kind === "char_literal" ? charLiteral(row.text) : row.text);
    else if (k === "string") {
      if (row.ext_kind) this.put(row.text);
      else if (!inFlow && isMultiline(row)) this.put(multilineString(row.text));
      else if (!inFlow && this.kindTag(row) === "string") this.put(row.text);
      else this.put(bareOrQuoted(row.text ?? "", false, inFlow));
    } else if (k === "mapping" || k === "sequence") this.flowValue(row);
    else if (k === "alias") throw new Error("an alias must be resolved before it is written as fig");
    else throw new Error("a " + k + " is not a value");
  }

  flowValue(row) {
    if (row.kind === "sequence") {
      this.put("[");
      row.items.forEach((el, i) => {
        if (i > 0) this.put(", ");
        this.value(el, true);
      });
      this.put("]");
    } else if (row.kind === "mapping") {
      if (row.items.length === 0) return this.put("{}");
      this.put("{ ");
      row.items.forEach((e, i) => {
        if (i > 0) this.put(", ");
        this.put(bareOrQuoted(this.keyText(e.key), true, false), " = ");
        this.value(e.value, true);
      });
      this.put(" }");
    } else throw new Error("a " + row.kind + " has no flow spelling");
  }

  multilineFlowSeq(row, indent, commentAnchor) {
    this.put("[");
    if (commentAnchor) this.trailingComment(commentAnchor);
    this.put("\n");
    const inner = indent + 2;
    for (const el of row.items) {
      this.put(" ".repeat(inner));
      if (el.kind === "sequence" && el.items.length > 0 && !this.seqFitsInline(el, inner + 1, "strict")) this.multilineFlowSeq(el, inner, null);
      else this.value(el, true);
      this.put(",\n");
    }
    this.put(" ".repeat(indent), "]");
  }

  body(row, depth) {
    if (row.kind === "mapping") return this.mapBody(row, depth);
    if (row.kind === "sequence") return this.seqBody(row, depth);
    throw new Error("a " + row.kind + " has no block body");
  }

  mapBody(row, depth) {
    for (const e of row.items) this.mapEntryLine(e, depth);
    for (const c of this.danglingOf(row)) this.commentLines(c, depth);
  }

  mapEntryLine(e, depth) {
    const [endE, kw] = this.resolveChain(e);
    this.leadingComments(endE.key, depth);
    this.markers(depth);
    this.put(this.chainKeys(e, endE));
    const v = endE.value;
    const prefixWidth = mapEntryPrefixWidth(depth, kw);
    if (v.kind === "mapping") {
      if (this.fitsInline(v, prefixWidth, "allow_trailing")) {
        this.put(" = ");
        this.flowValue(v);
        this.trailingComment(v);
        this.put("\n");
      } else {
        this.trailingComment(v);
        this.put("\n");
        this.body(v, depth + 1);
      }
    } else if (v.kind === "sequence") {
      if (this.seqFitsInline(v, prefixWidth, "allow_trailing")) {
        this.put(" = ");
        this.flowValue(v);
        this.trailingComment(v);
        this.put("\n");
      } else if (this.multilineFlowEligible(v, "allow_trailing")) {
        this.put(" = ");
        this.multilineFlowSeq(v, 2 * depth, v);
        this.put("\n");
      } else {
        this.trailingComment(v);
        this.put("\n");
        this.body(v, depth + 1);
      }
    } else {
      this.typeAnnotation(v);
      this.put(" = ");
      this.value(v, false);
      this.trailingComment(v);
      this.put("\n");
    }
  }

  seqBody(row, depth) {
    for (const el of row.items) {
      this.leadingComments(el, depth);
      this.markers(depth);
      this.put("*");
      if (el.kind === "mapping") {
        if (this.fitsInline(el, elementPrefixWidth(depth), "allow_leading_trailing")) {
          this.put(" ");
          this.flowValue(el);
          this.trailingComment(el);
          this.put("\n");
        } else {
          this.trailingComment(el);
          this.put("\n");
          this.body(el, depth + 1);
        }
      } else if (el.kind === "sequence") {
        if (this.seqFitsInline(el, elementPrefixWidth(depth), "allow_leading_trailing")) {
          this.put(" ");
          this.flowValue(el);
          this.trailingComment(el);
          this.put("\n");
        } else if (this.multilineFlowEligible(el, "allow_leading_trailing")) {
          this.put(" ");
          this.multilineFlowSeq(el, elementPrefixWidth(depth), el);
          this.put("\n");
        } else {
          this.trailingComment(el);
          this.put("\n");
          this.body(el, depth + 1);
        }
      } else {
        const annotated = this.typeAnnotation(el);
        this.put(annotated ? " = " : " ");
        this.value(el, false);
        this.trailingComment(el);
        this.put("\n");
      }
    }
    for (const c of this.danglingOf(row)) this.commentLines(c, depth);
  }

  // ── sections ──

  pathText() {
    return this.path.map((key) => bareOrQuoted(this.keyText(key), true, false)).join(".");
  }
  pathWidth() {
    let w = this.path.length > 0 ? this.path.length - 1 : 0;
    for (const key of this.path) w += this.keyWidth(key);
    return w;
  }

  beginSection(multiline, anchor) {
    const hasLead = anchor !== null && this.commentsOn() && anchor.leading.length > 0;
    const ml = multiline || hasLead;
    if (this.started && (ml || this.prevMultiline)) this.put("\n");
    this.started = true;
    this.prevMultiline = ml;
    if (anchor !== null) this.leadingComments(anchor, 0);
  }

  isFlatChild(e) {
    const [endE, kw] = this.resolveChain(e);
    const v = endE.value;
    if (v.kind === "mapping") return this.fitsInline(v, mapEntryPrefixWidth(1, kw), "allow_trailing");
    if (v.kind === "sequence") return this.seqFitsInline(v, mapEntryPrefixWidth(1, kw), "allow_trailing");
    return true;
  }

  sectionAssign(anchorKey, v) {
    this.beginSection(false, anchorKey);
    this.put(this.pathText());
    if (v.kind === "mapping" || v.kind === "sequence") {
      this.put(" = ");
      this.flowValue(v);
    } else {
      this.typeAnnotation(v);
      this.put(" = ");
      this.value(v, false);
    }
    this.trailingComment(v);
    this.put("\n");
  }

  emitAppendGroup(seq) {
    seq.items.forEach((el, i) => {
      if (i === 0) this.beginSection(true, null);
      this.leadingComments(el, 0);
      if (i === 0) this.put(this.pathText(), "[]");
      else this.put("+");
      this.trailingComment(el);
      this.put("\n");
      this.mapBody(el, 1);
    });
  }

  emitSection(e) {
    this.path.push(e.key);
    try {
      const child = this.collapseChild(e);
      if (child) return this.emitSection(child);
      const v = e.value;
      if (v.kind === "mapping") {
        if (this.fitsInline(v, this.pathWidth() + ASSIGN_OP_WIDTH, "allow_trailing")) return this.sectionAssign(e.key, v);
        const keyHasLeading = this.commentsOn() && e.key.leading.length > 0;
        const vHasTail = this.commentsOn() && (v.trailing.length > 0 || v.dangling.length > 0);
        if (v.items.length > 0 && this.blockDepthMap(v, 1) > MAX_BODY_DEPTH && !keyHasLeading && !vHasTail) {
          let i = 0;
          while (i < v.items.length) {
            let runLen = 0;
            let j = i;
            while (j < v.items.length && this.isFlatChild(v.items[j])) {
              runLen += 1;
              j += 1;
            }
            if (runLen === 1) {
              this.emitSection(v.items[i]);
              i += 1;
            } else if (runLen >= 2) {
              this.beginSection(true, null);
              this.put(this.pathText(), "\n");
              for (let r = 0; r < runLen; r++) {
                this.mapEntryLine(v.items[i], 1);
                i += 1;
              }
            }
            if (i < v.items.length) {
              this.emitSection(v.items[i]);
              i += 1;
            }
          }
          return;
        }
        this.beginSection(true, e.key);
        this.put(this.pathText());
        this.trailingComment(v);
        this.put("\n");
        this.mapBody(v, 1);
      } else if (v.kind === "sequence") {
        if (this.seqFitsInline(v, this.pathWidth() + ASSIGN_OP_WIDTH, "allow_trailing")) return this.sectionAssign(e.key, v);
        const keyHasLeading = this.commentsOn() && e.key.leading.length > 0;
        const vHasTail = this.commentsOn() && (v.trailing.length > 0 || v.dangling.length > 0);
        if (v.items.length > 0 && v.items.every((el) => el.kind === "mapping") && !keyHasLeading && !vHasTail) return this.emitAppendGroup(v);
        if (this.multilineFlowEligible(v, "allow_trailing")) {
          this.beginSection(false, e.key);
          this.put(this.pathText(), " = ");
          this.multilineFlowSeq(v, 0, v);
          this.put("\n");
          return;
        }
        this.beginSection(true, e.key);
        this.put(this.pathText());
        this.trailingComment(v);
        this.put("\n");
        this.seqBody(v, 1);
      } else this.sectionAssign(e.key, v);
    } finally {
      this.path.pop();
    }
  }

  // A mapping at depth 0 is a document of sections; deeper, a block body.
  printNode(row, depth) {
    if (row.kind === "mapping") {
      if (depth === 0) {
        for (const e of row.items) this.emitSection(e);
        for (const c of this.danglingOf(row)) this.commentLines(c, 0);
      } else this.mapBody(row, depth);
    } else if (row.kind === "sequence") this.seqBody(row, depth);
    else {
      if (depth === 0) throw new Error("a scalar has no fig document spelling; use the canonical form");
      this.value(row, false);
      this.put("\n");
    }
  }
}

const mapEntryPrefixWidth = (depth, keyWidth) => MARKER_CELL * depth + keyWidth + ASSIGN_OP_WIDTH;
const elementPrefixWidth = (depth) => MARKER_CELL * (depth + 1);

function flowQuotesWhatBlockLeavesBare(row) {
  if (row.kind === "string" && !row.ext_kind) {
    const s = row.text ?? "";
    return !isBareSafe(s, false, true) && isBareSafe(s, false, false);
  }
  if (row.kind === "sequence") return row.items.some(flowQuotesWhatBlockLeavesBare);
  return false;
}

function print(_dialect, t, options) {
  fig.index(t);
  const root = t.byid(0);
  const p = new Printer(options);
  p.leadingComments(root, 0);
  if (root.kind === "mapping") {
    for (const e of root.items) p.emitSection(e);
    for (const c of p.danglingOf(root)) p.commentLines(c, 0);
  } else if (root.kind === "sequence") p.seqBody(root, 0);
  else {
    // A scalar root is a fragment — a value the editor will splice after
    // `key = ` — and takes its ordinary spelling; a whole document is never
    // a scalar.
    p.value(root, false);
    p.put("\n");
  }
  return p.w.string();
}

// ── the renderer ──────────────────────────────────────────────────────────
// What follows a key on its entry line: ` = value` for an inline value, or
// a newline and a block map or sequence re-printed as a nested section one
// marker level below the key's — the count of `>` in `indent`. The root
// (an empty key) takes the value as written.

function blockBody(depth, valueText) {
  const t = valueText.replace(/^[ \t\r\n]+|[ \t\r\n]+$/g, "");
  if (t === "" || "{['\"".includes(t[0])) return null;
  if (!t.includes("\n")) return null;
  let root;
  try {
    root = parseDocument(valueText);
  } catch (e) {
    return null;
  }
  if (root.kind !== "mapping" && root.kind !== "sequence") return null;
  const t2 = fig.rows(root);
  fig.index(t2);
  const p = new Printer({});
  p.printNode(t2.byid(0), depth);
  return p.w.string();
}

function render(which, args) {
  if (which !== "tail") throw new Error("no renderer `" + which + "`");
  if (args.key === "") return args.value;
  const depth = (args.indent.match(/>/g) ?? []).length;
  const body = blockBody(depth + 1, args.value);
  if (body !== null) return "\n" + body.replace(/\n+$/, "");
  return " = " + args.value;
}

// ── the language ──────────────────────────────────────────────────────────

/** @type {import("@diaryx/fig/helper").Language} */
const language = {
  name: "fig",
  caps: { read: true, edit: true, serialize: true },
  syntax: {
    comments: { style: "hash", line: { open: "#" }, trailing: { open: "#" } },
    kv_sep: " = ",
    flow_kv_sep_from_siblings: true,
    flow_map_pad: " ",
    empty_map_literal: "{}",
    // The `>` run that opens a line is depth, not whitespace: a line the
    // engine writes repeats it; one level is one `> ` cell, an element `* `.
    structural_indent: true,
    indent_unit: "> ",
    seq_item_marker: "* ",
    section_noun: "container",
  },
  // The compiled format's extension: where the format is compiled out
  // and this stands in for it, the extension reaches it.
  dialects: [{ name: "fig", extensions: ["figl"], splice: "literal", empty_doc_seed: "" }],
  samples: ["a = 1\ndatabase\n> host = localhost\n> port = 5432\n"],
  // What follows a key: ` = value`, or a block value re-printed as a nested
  // section one marker level below the key.
  renderers: ["tail"],
  parse,
  print,
  render,
};

export default language;
