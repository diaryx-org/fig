// `import * as G from "@diaryx/fig/kit/grammar"`: a format as a description.
//
// A *rule* is a function `(sc, ctx) => result | null` over a `Scanner` and
// a binding context. `null` (or undefined) means "not here" — the cursor is
// where it was, and a `choice` tries the next alternative; a *hard* failure
// is `fig.fail`, thrown through `expect`, `fail` or a scanner, and refuses
// the input with a message at an offset. The primitives (`lit`, `pat`,
// `eol`) return spans; the fig-aware rules (`key`, `scalar`, `quoted`,
// `bare`, `entry`, `map`, `sequence`) return tree nodes, with the spans,
// the comment binding and the duplicate policy taken care of; `document`
// turns a root rule into a `parse` function. `sections`, at the end, is
// not a rule: it is what a format of header lines — INI, TOML, fig —
// shares when its parser is written by hand.
//
// Comments are bound by the context as trivia is skipped: a comment on the
// same line as the last node completed is that node's trailing comment;
// any other waits, and becomes the leading comment of the next entry (on
// its key, by default) or, when a container closes with it still waiting,
// a dangling comment of that container. That is the policy every format
// fig compiles in shares; `context(sc, opts)` — or `document({ comments
// })` — takes the three refinements some of them add.
//
// Every offset is a byte offset: a rule reads the scanner's bytes or its
// one-char-per-byte `bin`, never the input string. A regular expression
// given to `pat` runs over `bin`, so its classes are byte classes — `\d`,
// `[A-Za-z_]`, `[^\x00-\x7f]` for "any byte of a multibyte character".
import * as fig from "@diaryx/fig/kit";

// ── the context ───────────────────────────────────────────────────────────

const isContainer = (n) => n.kind === "mapping" || n.kind === "sequence";

export class Context {
  /** A binding context over `sc`. `opts` refines what trails:
   *  `endsLine` (false) — a comment trails only when it is the last thing
   *  on its line, so a block comment with more after it on the line waits
   *  instead; `one` (false) — a node takes one trailing comment, a second
   *  on the same line waiting like any other; `closingLine` (false) — a
   *  container that spans lines takes the comment on its closing line as
   *  dangling rather than trailing. */
  constructor(sc, opts) {
    this.sc = sc;
    this.pending = [];
    this.last = null;
    this.endsLine = opts?.endsLine === true;
    this.one = opts?.one === true;
    this.closingLine = opts?.closingLine === true;
  }

  /** A comment `[s, e]` with `text` and `style`, met in trivia. */
  comment(c) {
    const last = this.last;
    if (last && this.sc.sameLine(last.span[1], c[0]) && (!this.endsLine || this.restIsBlank(c[1]))) {
      if (this.closingLine && isContainer(last) && !this.sc.sameLine(last.span[0], last.span[1])) {
        last.comment("dangling", c.text, c.style);
      } else {
        last.comment("trailing", c.text, c.style);
      }
      if (this.one) this.last = null;
    } else {
      this.pending.push(c);
      this.last = null;
    }
  }

  /** Whether only spaces and tabs lie between `at` and the line's end. */
  restIsBlank(at) {
    const { bin } = this.sc;
    let i = at;
    while (i < bin.length && (bin[i] === " " || bin[i] === "\t")) i += 1;
    return i >= bin.length || bin[i] === "\n" || bin[i] === "\r";
  }

  /** Give every waiting comment to `node`, in `slot`. */
  flush(node, slot) {
    for (const c of this.pending) node.comment(slot, c.text, c.style);
    this.pending = [];
  }
}

/** A binding context over `sc`, with `opts` as `Context` takes them. */
export function context(sc, opts) {
  return new Context(sc, opts);
}

// ── primitives ────────────────────────────────────────────────────────────

/** The span a result covers. */
export function spanOf(r) {
  return r.span ?? r;
}

/** The literal `s`. */
export function lit(s) {
  return (sc) => sc.lit(s);
}

/** A regular expression, anchored at the cursor: a RegExp or its source.
 *  The result is the span; captures are at `[2]` onward. */
export function pat(re) {
  const sticky = fig.stickyOf(re);
  return (sc) => sc.match(sticky);
}

/** The end of the input. */
export function eof(sc) {
  return sc.eof() ? [sc.pos, sc.pos] : null;
}

/** A line ending: `\n` or `\r\n`. */
export function eol(sc) {
  return sc.lit("\n") ?? sc.lit("\r\n");
}

/** Spaces and tabs, possibly none; at least one; all whitespace; at least
 *  one whitespace. */
export const hs = pat(/[ \t]*/);
export const hs1 = pat(/[ \t]+/);
export const ws = pat(/[ \t\r\n]*/);
export const ws1 = pat(/[ \t\r\n]+/);

// ── combinators ───────────────────────────────────────────────────────────

/** Each rule in turn; the results as a list with the whole `span`. */
export function seq(rules) {
  return (sc, ctx) => {
    const s = sc.pos;
    const out = [];
    for (const r of rules) {
      const v = r(sc, ctx);
      if (v == null) {
        sc.pos = s;
        return null;
      }
      out.push(v);
    }
    out.span = [s, sc.pos];
    return out;
  };
}

/** The first rule that matches. */
export function choice(rules) {
  return (sc, ctx) => {
    const s = sc.pos;
    for (const r of rules) {
      const v = r(sc, ctx);
      if (v != null) return v;
      sc.pos = s;
    }
    return null;
  };
}

/** `r`, or an empty result marked `absent`. */
export function opt(r) {
  return (sc, ctx) => {
    const s = sc.pos;
    const v = r(sc, ctx);
    if (v != null) return v;
    sc.pos = s;
    const empty = [s, s];
    empty.absent = true;
    return empty;
  };
}

/** `r` as many times as it matches and moves; the results as a list. */
export function many(r) {
  return (sc, ctx) => {
    const s = sc.pos;
    const out = [];
    for (;;) {
      const p = sc.pos;
      const v = r(sc, ctx);
      if (v == null || sc.pos === p) {
        sc.pos = p;
        break;
      }
      out.push(v);
    }
    out.span = [s, sc.pos];
    return out;
  };
}

/** `r` at least once. */
export function many1(r) {
  const rule = many(r);
  return (sc, ctx) => {
    const v = rule(sc, ctx);
    return v.length === 0 ? null : v;
  };
}

/** Whether `r` matches here, without moving. */
export function ahead(r) {
  return (sc, ctx) => {
    const s = sc.pos;
    const v = r(sc, ctx);
    sc.pos = s;
    return v;
  };
}

/** Matches, empty, only where `r` does not. */
export function none(r) {
  return (sc, ctx) => {
    const s = sc.pos;
    const v = r(sc, ctx);
    sc.pos = s;
    return v == null ? [s, s] : null;
  };
}

/** `r`, or refuse the input with `message` at the cursor. */
export function expect(r, message) {
  return (sc, ctx) => {
    const v = r(sc, ctx);
    if (v == null) sc.fail(message);
    return v;
  };
}

/** Refuse the input with `message` at the cursor. `message` may be a
 *  function of the scanner. */
export function fail(message) {
  return (sc) => {
    sc.fail(typeof message === "function" ? message(sc) : message);
  };
}

/** Refuse the input with `message` where `r` matches; otherwise empty. */
export function failIf(r, message) {
  return (sc, ctx) => {
    const s = sc.pos;
    const v = r(sc, ctx);
    sc.pos = s;
    if (v != null) sc.fail(message);
    return [s, s];
  };
}

/** `r`, with its result passed through `fn(result, sc, ctx)`. */
export function apply(r, fn) {
  return (sc, ctx) => {
    const v = r(sc, ctx);
    return v == null ? null : fn(v, sc, ctx);
  };
}

// ── nodes ─────────────────────────────────────────────────────────────────

function textOf(opts, raw, span, sc) {
  return opts?.text ? opts.text(raw, span, sc) : raw;
}

function kindOf(kind, raw) {
  return typeof kind === "function" ? kind(raw) : kind;
}

function extra(opts) {
  return opts?.extKind ? { ext_kind: opts.extKind } : undefined;
}

/** A scalar of `kind` over what `r` matched: its span, and its text — the
 *  bytes matched, or `opts.text(raw, span, sc)` of them. `kind` may be a
 *  function of the raw text. `opts.extKind` marks an extended scalar. */
export function scalar(kind, r, opts) {
  return (sc, ctx) => {
    const v = r(sc, ctx);
    if (v == null) return null;
    const [s, e] = spanOf(v);
    const span = [s, e];
    const raw = sc.slice(s, e);
    return fig.scalar(kindOf(kind, raw), span, textOf(opts, raw, span, sc), extra(opts));
  };
}

/** A key: a string scalar over what `r` matched. */
export function key(r, opts) {
  return scalar("string", r, opts);
}

/** A bare value: to the end of the line, or to where `opts.stop` matches
 *  (a rule, or a function of the scanner that answers true), trimmed of
 *  spaces and tabs unless `opts.trim` is false. May be empty unless
 *  `opts.empty` is false. `opts.kind` is the scalar kind (a string or a
 *  function of the text, "string"). */
export function bare(opts) {
  opts ??= {};
  const stop = opts.stop;
  const trim = opts.trim !== false;
  const kind = opts.kind ?? "string";
  return (sc, ctx) => {
    const s = sc.pos;
    while (!sc.eof()) {
      const c = sc.byte();
      if (c === 10 || c === 13) break;
      if (stop) {
        const p = sc.pos;
        const hit = stop(sc, ctx);
        sc.pos = p;
        if (hit) break;
      }
      sc.advance();
    }
    let ts = s;
    let te = sc.pos;
    if (trim) {
      const b = sc.bytes;
      while (ts < te && (b[ts] === 32 || b[ts] === 9)) ts++;
      while (te > ts && (b[te - 1] === 32 || b[te - 1] === 9)) te--;
    }
    if (opts.empty === false && ts === te) {
      sc.pos = s;
      return null;
    }
    const raw = sc.slice(ts, te);
    const span = [ts, te];
    return fig.scalar(kindOf(kind, raw), span, textOf(opts, raw, span, sc), extra(opts));
  };
}

/** A quoted value: `opts.open` through `opts.close` (the same), the span
 *  including both. `opts.escapes` is an object from the character after
 *  `opts.escape` (`\`) to what it means; without it, nothing escapes.
 *  `\r\n` inside reads as `\n` unless `opts.crlf` is false; a bare `\r`
 *  refuses the input with `opts.bareCr` when that is given. Messages:
 *  `unclosed`, `badEscape`. `opts.after(sc, span)` runs once the closing
 *  quote is passed — to check what follows on the line — and may return
 *  the offset a bad escape is reported at. */
export function quoted(opts) {
  const open = opts.open;
  const close = opts.close ?? open;
  const escapes = opts.escapes;
  const escape = opts.escape ?? (escapes ? "\\" : undefined);
  const escByte = escape !== undefined ? escape.charCodeAt(0) : undefined;
  const closeBin = fig.binOf(close);
  const closeByte = closeBin.charCodeAt(0);
  const crlf = opts.crlf !== false;
  const kind = opts.kind ?? "string";
  const unclosed = opts.unclosed ?? "unclosed quoted value; expected a matching `" + close + "`";
  return (sc) => {
    const s = sc.pos;
    if (sc.lit(open) == null) return null;
    const { bytes, n, bin } = sc;
    let i = sc.pos;
    for (;;) {
      if (i >= n) fig.fail(unclosed, i);
      const c = bytes[i];
      if (c === 13) {
        if (bytes[i + 1] === 10) i += 2;
        else if (opts.bareCr) fig.fail(opts.bareCr, i);
        else i += 1;
      } else if (escByte !== undefined && c === escByte) {
        if (i + 1 >= n) fig.fail(unclosed, i);
        i += 2;
      } else if (c === closeByte && bin.startsWith(closeBin, i)) {
        i += closeBin.length;
        break;
      } else {
        i += 1;
      }
    }
    sc.pos = i;
    const span = [s, i];
    const errAt = (opts.after && opts.after(sc, span)) ?? i;
    const innerStart = s + fig.binOf(open).length;
    const innerEnd = i - closeBin.length;
    let text;
    if (!escapes && !crlf) {
      text = sc.slice(innerStart, innerEnd);
    } else {
      // Over the bytes, so an escape is found by its byte and never inside
      // a multibyte character; the pieces are one-char-per-byte too.
      const inner = sc.binSlice(innerStart, innerEnd);
      let out = "";
      let j = 0;
      while (j < inner.length) {
        const ch = inner[j];
        if (crlf && ch === "\r" && inner[j + 1] === "\n") {
          out += "\n";
          j += 2;
        } else if (escapes && ch === escape) {
          const next = inner[j + 1];
          const rep = Object.hasOwn(escapes, next) ? escapes[next] : undefined;
          if (rep === undefined) {
            fig.fail(opts.badEscape ?? "invalid escape `" + escape + next + "` in a quoted value", errAt);
          }
          out += fig.binOf(rep);
          j += 2;
        } else {
          out += ch;
          j += 1;
        }
      }
      text = fig.fromBin(out);
    }
    return fig.scalar(kindOf(kind, text), span, textOf(opts, text, span, sc), extra(opts));
  };
}

const TRIM_BLOCK = /^[ \t\n\v\f\r]+|[ \t\n\v\f\r]+$/g;
const TRIM_LINE = /^[ \t\r]+|[ \t\r]+$/g;

/** A comment: `opts.open` to the end of the line, or to `opts.close`. The
 *  text is trimmed unless `opts.trim` is false; `opts.style` is "line" or
 *  "block" (by whether there is a `close`). A string is the opener alone.
 *  The result is `[s, e]` with `text` and `style`, which trivia binds. */
export function comment(opts) {
  if (typeof opts === "string") opts = { open: opts };
  const { open, close } = opts;
  const trim = opts.trim !== false;
  const style = opts.style ?? (close ? "block" : "line");
  return (sc) => {
    const s = sc.pos;
    if (sc.lit(open) == null) return null;
    const bodyStart = sc.pos;
    let bodyEnd;
    if (close) {
      const at = sc.find(close);
      if (at < 0) fig.fail(opts.unclosed ?? "unclosed comment; expected `" + close + "`", s);
      bodyEnd = at;
      sc.pos = at + fig.binOf(close).length;
    } else {
      const nl = sc.bin.indexOf("\n", sc.pos);
      const cr = sc.bin.indexOf("\r", sc.pos);
      let at = nl < 0 ? cr : cr < 0 ? nl : Math.min(nl, cr);
      if (at < 0) at = sc.n;
      bodyEnd = at;
      sc.pos = at;
    }
    let text = sc.slice(bodyStart, bodyEnd);
    if (trim) text = text.replace(close ? TRIM_BLOCK : TRIM_LINE, "");
    const result = [s, sc.pos];
    result.text = text;
    result.style = style;
    return result;
  };
}

/** What lies between the things that matter: `opts.space` (spaces, tabs
 *  and line endings) and `opts.comment` (a `comment` rule, or a `choice`
 *  of them), in any order, comments bound to the context as they pass.
 *  Always matches. */
export function trivia(opts) {
  opts ??= {};
  const space = opts.space ?? choice([hs1, eol]);
  const comment = opts.comment;
  return (sc, ctx) => {
    const s = sc.pos;
    while (!sc.eof()) {
      const p = sc.pos;
      const v = space(sc, ctx);
      if (v == null || sc.pos === p) {
        sc.pos = p;
        if (!comment) break;
        const c = comment(sc, ctx);
        if (c == null) break;
        if (ctx) ctx.comment(c);
      }
    }
    return [s, sc.pos];
  };
}

/** A keyvalue: `opts.prefix` (optional by nature), `opts.key`,
 *  `opts.between` (`hs`), `opts.sep`, `between` again, `opts.value`. Once
 *  the key has matched the rest is expected: a missing separator refuses
 *  the input with `opts.missingSep`, a missing value with
 *  `opts.missingValue`. Waiting comments become leading comments of the
 *  key (`opts.comments.leading`: "key", "value" or "entry"). */
export function entry(opts) {
  const { prefix, key, sep, value } = opts;
  const between = opts.between ?? hs;
  const leading = opts.comments?.leading ?? "key";
  return (sc, ctx) => {
    const s = sc.pos;
    if (prefix && prefix(sc, ctx) == null) sc.pos = s;
    const k = key(sc, ctx);
    if (k == null) {
      sc.pos = s;
      return null;
    }
    between(sc, ctx);
    if (sep) {
      if (sep(sc, ctx) == null) sc.fail(opts.missingSep ?? "expected a separator after this key");
      between(sc, ctx);
    }
    let v = null;
    if (value) {
      v = value(sc, ctx);
      if (v == null) sc.fail(opts.missingValue ?? "expected a value here");
    }
    const e = fig.entry(k, v, v == null ? [k.span[0], sc.pos] : undefined);
    if (ctx) ctx.flush(leading === "key" ? k : leading === "value" ? v : e, "leading");
    return e;
  };
}

function trailingTarget(opts, e) {
  const on = opts.comments?.trailing ?? "value";
  return on === "value" ? e.value : on === "key" ? e.key : e;
}

/** A mapping: `opts.open` (none), then entries by `opts.entry` separated
 *  by `opts.trivia` (whitespace), until `opts.close` (none: the end of the
 *  input). Where an entry is expected and none matches,
 *  `opts.otherwise(sc, ctx)` may refuse the input with its own message;
 *  otherwise `opts.expected` is the message. `opts.after` runs after every
 *  entry (an entry terminator). `opts.duplicates` is the policy
 *  (`fig.mapping`); `opts.whole` makes the span the whole input, for a
 *  root. Comments still waiting at the close are the mapping's dangling
 *  comments. */
export function map(opts) {
  const entry = opts.entry;
  const between = opts.trivia ?? trivia();
  const { open, close } = opts;
  return (sc, ctx) => {
    const s = sc.pos;
    if (open && open(sc, ctx) == null) return null;
    const m = fig.mapping(undefined, { duplicates: opts.duplicates });
    for (;;) {
      between(sc, ctx);
      if (close) {
        const p = sc.pos;
        if (close(sc, ctx) != null) break;
        sc.pos = p;
        if (sc.eof()) sc.fail(opts.unclosed ?? "unclosed mapping; expected its close before the end of the input");
      } else if (sc.eof()) {
        break;
      }
      const e = entry(sc, ctx);
      if (e == null) {
        if (opts.otherwise) opts.otherwise(sc, ctx);
        sc.fail(opts.expected ?? "expected an entry here");
      }
      m.put(e);
      if (ctx) ctx.last = trailingTarget(opts, e);
      if (opts.after) opts.after(sc, ctx);
    }
    m.span = opts.whole ? [0, sc.n] : [s, sc.pos];
    if (ctx) {
      ctx.flush(m, "dangling");
      ctx.last = m;
    }
    return m;
  };
}

/** A sequence: as `map`, with `opts.item` for `entry`. */
export function sequence(opts) {
  const item = opts.item;
  const between = opts.trivia ?? trivia();
  const { open, close } = opts;
  return (sc, ctx) => {
    const s = sc.pos;
    if (open && open(sc, ctx) == null) return null;
    const q = fig.sequence(undefined);
    for (;;) {
      between(sc, ctx);
      if (close) {
        const p = sc.pos;
        if (close(sc, ctx) != null) break;
        sc.pos = p;
        if (sc.eof()) sc.fail(opts.unclosed ?? "unclosed sequence; expected its close before the end of the input");
      } else if (sc.eof()) {
        break;
      }
      const v = item(sc, ctx);
      if (v == null) {
        if (opts.otherwise) opts.otherwise(sc, ctx);
        sc.fail(opts.expected ?? "expected an item here");
      }
      q.add(v);
      if (ctx) {
        ctx.flush(v, "leading");
        ctx.last = v;
      }
      if (opts.after) opts.after(sc, ctx);
    }
    q.span = opts.whole ? [0, sc.n] : [s, sc.pos];
    if (ctx) {
      ctx.flush(q, "dangling");
      ctx.last = q;
    }
    return q;
  };
}

/** A `parse` function over `opts.root`: `opts.trivia` before and after it,
 *  then the end of the input, or `opts.trailing` (a message, or a function
 *  of the scanner giving one). `opts.bom` skips a byte-order mark;
 *  `opts.missing` is the message when the root does not match. Comments
 *  left waiting are the root's dangling comments; `opts.comments` refines
 *  how they bind, as `context` takes it. The function answers the wire's
 *  node table.
 *
 *  There is no `utf8` option: the input reaches a language as a string
 *  the host already decoded, so what is not UTF-8 never arrives. */
export function document(opts) {
  const { root, trivia: between } = opts;
  return (_dialect, input) => {
    const sc = fig.scanner(input);
    if (opts.bom && sc.bytes[0] === 0xef && sc.bytes[1] === 0xbb && sc.bytes[2] === 0xbf) sc.pos = 3;
    const ctx = context(sc, opts.comments);
    if (between) between(sc, ctx);
    const r = root(sc, ctx);
    if (r == null) sc.fail(opts.missing ?? "no document here");
    if (between) between(sc, ctx);
    if (!sc.eof()) {
      const message = opts.trailing ?? "unexpected content after the document";
      sc.fail(typeof message === "function" ? message(sc) : message);
    }
    ctx.flush(r, "dangling");
    return fig.rows(r);
  };
}

// ── sections ──────────────────────────────────────────────────────────────
// A section format — INI, TOML, fig — opens a container with a header
// line and may re-enter it with another, and the wire asks the same of
// each: a *region* for every line that opened or re-entered the
// container, the whole physical line, newline included; a *mention* for
// every place its name is written; and comments placed by where they
// fell — waiting until the next key claims them as leading, or dangling
// on the container that was open when its run ended. What a header
// *means* — which containers it may re-enter, what its dots and brackets
// say, where the cursor goes — differs in each, and stays in the parser.

/** The whole physical line holding `at`, newline included: what a header
 *  line contributes to its container's regions. `bin` is the input one
 *  char per byte, as a scanner's `bin`. */
export function lineRegion(bin, at) {
  let s = at;
  while (s > 0 && bin.charCodeAt(s - 1) !== 10) s -= 1;
  let e = at;
  while (e < bin.length && bin.charCodeAt(e) !== 10) e += 1;
  if (e < bin.length) e += 1;
  return [s, e];
}

export class Sections {
  /** A section context over `bin`. */
  constructor(bin) {
    this.bin = bin;
    this.pending = [];
  }

  lineRegion(at) {
    return lineRegion(this.bin, at);
  }

  /** The line holding `at`, as a region of `node`. */
  region(node, at) {
    (node.regions ??= []).push(this.lineRegion(at));
  }

  /** `span`, where the name of `node` is written, as a mention of `kind`
   *  ("header" or "entry"). */
  mention(node, span, kind) {
    (node.mentions ??= []).push({ span, kind });
  }

  /** Open a section: `entry`, a keyvalue over the container, goes into
   *  `parent`; the line holding its key is the container's first region,
   *  and the key its first mention, of `kind`. Returns the container. */
  open(parent, entry, kind) {
    parent.put(entry);
    this.region(entry.value, entry.key.span[0]);
    this.mention(entry.value, entry.key.span, kind);
    return entry.value;
  }

  /** Re-enter `node` by a header that names it at `span`: another region,
   *  another mention, of `kind`. */
  reopen(node, span, kind) {
    this.region(node, span[0]);
    this.mention(node, span, kind);
  }

  /** A comment waiting for its key, met on a line at `depth` (0) — a
   *  format whose lines have no depth leaves it — of `style` ("line"). */
  comment(text, depth = 0, style = "line") {
    this.pending.push({ text, depth, style });
  }

  /** Take the waiting comments from lines at `depth` (0: every one) or
   *  deeper, in order; the shallower ones keep waiting. */
  take(depth = 0) {
    const taken = [];
    const kept = [];
    for (const c of this.pending) (c.depth >= depth ? taken : kept).push(c);
    this.pending = kept;
    return taken;
  }

  /** Give the waiting comments at `depth` or deeper to `node`, in `slot`
   *  ("leading" for a key, "dangling" for a container). */
  claim(node, slot, depth = 0) {
    for (const c of this.take(depth)) node.comment(slot, c.text, c.style);
  }
}

/** A section context over `bin`. */
export function sections(bin) {
  return new Sections(bin);
}
