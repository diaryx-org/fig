//! A format as a description: combinators over a [`Scanner`] that write the
//! node table as they match, so the spans, the comment binding and the
//! duplicate policy follow from the grammar.
//!
//! The JavaScript kit's `@diaryx/fig/kit/grammar`, in Rust, rule for rule
//! and message for message. A [`Rule`] runs over a [`Ctx`] — the scanner,
//! the [`RowWriter`] the table is written to, and the comments waiting to
//! be bound — and answers `Ok(Some(_))` when it matches, `Ok(None)` when it
//! does not (the cursor where it was, so a [`choice`] tries the next), or
//! `Err` to refuse the input with a message at an offset. The primitives
//! answer spans; the rules that make nodes ([`scalar`], [`key`], [`bare`],
//! [`quoted`], [`entry`], [`map`], [`sequence`]) answer the row ids they
//! wrote; [`document`] turns a root rule into a [`Grammar`], whose
//! [`parse`](Grammar::parse) is a language's.
//!
//! Comments are bound as trivia is skipped: a comment on the same line as
//! the last node completed is that node's trailing comment; any other
//! waits, and becomes the leading comment of the next entry (on its key, by
//! default) or, when a container closes with it still waiting, a dangling
//! comment of that container. [`Binding`] takes the refinements some
//! formats add.
//!
//! Where the JavaScript grammar matches a regular expression, this one
//! takes a byte predicate ([`bytes_while`], [`ident`]).
//!
//! ```
//! use fig::kit::grammar::{self as g, DocOpts, EntryOpts, MapOpts, TriviaOpts};
//!
//! let entry = g::entry(
//!     EntryOpts::new(g::key(g::ident(|b| b.is_ascii_alphabetic(), |b| b.is_ascii_alphanumeric())))
//!         .sep(g::lit("="))
//!         .value(g::bare(Default::default())),
//! );
//! let trivia = g::trivia(TriviaOpts::default().comment(g::comment_line("#")));
//! let kv = g::document(DocOpts::new(g::map(MapOpts::new(entry).whole(true).trivia(trivia))));
//! let t = kv.parse(b"# greeting\na = hello\n").unwrap();
//! assert_eq!(t.rows.len(), 4);
//! assert_eq!(t.rows[3].text.as_deref(), Some("hello"));
//! assert_eq!(t.comments[0].text, "greeting");
//! ```

use std::sync::Arc;

use crate::language::{CommentForm, CommentSlot, LanguageError, MentionKind, NodeKind, NodeRow, NodeTable};
use crate::{ExtKind, Span};

use super::rows::{DuplicatePolicy, RowWriter};
use super::scan::Scanner;

// ── the context ─────────────────────────────────────────────────────────────

/// How comments bind beyond the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Binding {
    /// A comment trails only when it is the last thing on its line, so a
    /// block comment with more after it on the line waits instead.
    pub ends_line: bool,
    /// A node takes one trailing comment; a second on the same line waits
    /// like any other.
    pub one: bool,
    /// A container that spans lines takes the comment on its closing line
    /// as dangling rather than trailing.
    pub closing_line: bool,
}

/// A comment a [`comment`] rule matched: its span, text and form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentMatch {
    pub span: Span,
    pub text: String,
    pub style: CommentForm,
}

/// What every rule runs over: the scanner, the rows written so far, and
/// the comments waiting to be bound.
pub struct Ctx<'a> {
    pub sc: Scanner<'a>,
    pub rows: RowWriter,
    pending: Vec<CommentMatch>,
    /// The node a comment on its line trails.
    pub last: Option<u32>,
    binding: Binding,
}

impl<'a> Ctx<'a> {
    /// A context at the start of `input`.
    pub fn new(input: &'a [u8], binding: Binding) -> Self {
        Ctx {
            sc: Scanner::new(input),
            rows: RowWriter::new(),
            pending: Vec::new(),
            last: None,
            binding,
        }
    }

    /// Row `id`'s span.
    pub fn span(&self, id: u32) -> Span {
        self.rows.row(id).span.unwrap_or(Span::from(0..0))
    }

    /// Bind comment `c`, met in trivia: trailing on the last node when it
    /// is on that node's line, else waiting.
    pub fn comment(&mut self, c: CommentMatch) {
        if let Some(last) = self.last {
            let s = self.span(last);
            if self.sc.same_line(s.end, c.span.start) && (!self.binding.ends_line || self.rest_is_blank(c.span.end)) {
                let kind = self.rows.row(last).kind;
                let container = matches!(kind, NodeKind::Mapping | NodeKind::Sequence);
                let slot = if self.binding.closing_line && container && !self.sc.same_line(s.start, s.end) {
                    CommentSlot::Dangling
                } else {
                    CommentSlot::Trailing
                };
                self.rows.comment(last, slot, c.style, c.text);
                if self.binding.one {
                    self.last = None;
                }
                return;
            }
        }
        self.pending.push(c);
        self.last = None;
    }

    /// Whether only spaces and tabs lie between `at` and the line's end.
    fn rest_is_blank(&self, at: usize) -> bool {
        let b = self.sc.bytes();
        let mut i = at;
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        i >= b.len() || b[i] == b'\n' || b[i] == b'\r'
    }

    /// Give every waiting comment to row `node`, in `slot`.
    pub fn flush(&mut self, node: u32, slot: CommentSlot) {
        for c in std::mem::take(&mut self.pending) {
            self.rows.comment(node, slot, c.style, c.text);
        }
    }

    /// A refusal at the cursor.
    pub fn fail(&self, message: impl Into<String>) -> LanguageError {
        self.sc.fail(message)
    }
}

// ── rules ───────────────────────────────────────────────────────────────────

type RuleFn<T> = dyn Fn(&mut Ctx<'_>) -> Result<Option<T>, LanguageError> + Send + Sync;

/// A rule: matches at the cursor and answers a `T`, does not match, or
/// refuses the input. Cheap to clone.
pub struct Rule<T>(Arc<RuleFn<T>>);

impl<T> Clone for Rule<T> {
    fn clone(&self) -> Self {
        Rule(self.0.clone())
    }
}

impl<T: 'static> Rule<T> {
    /// A rule from a function.
    pub fn new(f: impl Fn(&mut Ctx<'_>) -> Result<Option<T>, LanguageError> + Send + Sync + 'static) -> Self {
        Rule(Arc::new(f))
    }

    /// Run the rule at the cursor.
    pub fn run(&self, cx: &mut Ctx<'_>) -> Result<Option<T>, LanguageError> {
        (self.0)(cx)
    }

    /// The rule, its answer passed through `f`.
    pub fn map<U: 'static>(self, f: impl Fn(T, &mut Ctx<'_>) -> Result<U, LanguageError> + Send + Sync + 'static) -> Rule<U> {
        apply(self, f)
    }
}

/// A hook a container runs: `otherwise` where an entry was expected and
/// none matched, `after` after each entry.
pub type Hook = Arc<dyn Fn(&mut Ctx<'_>) -> Result<(), LanguageError> + Send + Sync>;

/// A function from a scalar's matched text to the text its row holds.
pub type TextFn = Arc<dyn Fn(&str, Span, &Scanner<'_>) -> Result<String, LanguageError> + Send + Sync>;

/// A function from a scalar's text to its row's kind.
pub type KindFn = Arc<dyn Fn(&str) -> NodeKind + Send + Sync>;

/// Chainable setters for an options struct: `opt` fields take the value
/// and set `Some`, `val` fields take it as it is.
macro_rules! setters {
    ($t:ty { $(opt $o:ident : $ot:ty,)* $(val $v:ident : $vt:ty,)* }) => {
        impl $t {
            $(
                #[doc = concat!("Set `", stringify!($o), "`.")]
                pub fn $o(mut self, v: $ot) -> Self {
                    self.$o = Some(v);
                    self
                }
            )*
            $(
                #[doc = concat!("Set `", stringify!($v), "`.")]
                pub fn $v(mut self, v: $vt) -> Self {
                    self.$v = v;
                    self
                }
            )*
        }
    };
}

// ── primitives ──────────────────────────────────────────────────────────────

/// The literal `s`.
pub fn lit(s: impl Into<String>) -> Rule<Span> {
    let s = s.into();
    Rule::new(move |cx| Ok(cx.sc.lit(&s)))
}

/// Bytes while `f` holds of them, possibly none.
pub fn bytes_while(f: impl Fn(u8) -> bool + Send + Sync + 'static) -> Rule<Span> {
    Rule::new(move |cx| Ok(Some(cx.sc.take_while(&f))))
}

/// Bytes while `f` holds of them, at least one.
pub fn bytes_while1(f: impl Fn(u8) -> bool + Send + Sync + 'static) -> Rule<Span> {
    Rule::new(move |cx| Ok(cx.sc.take_while1(&f)))
}

/// One byte of which `first` holds, then bytes while `rest` does: an
/// identifier.
pub fn ident(first: impl Fn(u8) -> bool + Send + Sync + 'static, rest: impl Fn(u8) -> bool + Send + Sync + 'static) -> Rule<Span> {
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        match cx.sc.byte(0) {
            Some(b) if first(b) => {
                cx.sc.advance(1);
                cx.sc.take_while(&rest);
                Ok(Some(Span::from(s..cx.sc.pos)))
            }
            _ => Ok(None),
        }
    })
}

/// Matches, empty, where `f` holds of the scanner.
pub fn when(f: impl Fn(&Scanner<'_>) -> bool + Send + Sync + 'static) -> Rule<Span> {
    Rule::new(move |cx| Ok(f(&cx.sc).then(|| Span::from(cx.sc.pos..cx.sc.pos))))
}

/// The end of the input.
pub fn eof() -> Rule<Span> {
    when(|sc| sc.eof())
}

/// A line ending: `\n` or `\r\n`.
pub fn eol() -> Rule<Span> {
    Rule::new(|cx| Ok(cx.sc.lit("\n").or_else(|| cx.sc.lit("\r\n"))))
}

fn is_hs(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

/// Spaces and tabs, possibly none.
pub fn hs() -> Rule<Span> {
    bytes_while(is_hs)
}

/// Spaces and tabs, at least one.
pub fn hs1() -> Rule<Span> {
    bytes_while1(is_hs)
}

/// Whitespace, possibly none.
pub fn ws() -> Rule<Span> {
    bytes_while(is_ws)
}

/// Whitespace, at least one.
pub fn ws1() -> Rule<Span> {
    bytes_while1(is_ws)
}

// ── combinators ─────────────────────────────────────────────────────────────

/// Each rule in turn; the whole span.
pub fn seq(rules: Vec<Rule<Span>>) -> Rule<Span> {
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        for r in &rules {
            if r.run(cx)?.is_none() {
                cx.sc.pos = s;
                return Ok(None);
            }
        }
        Ok(Some(Span::from(s..cx.sc.pos)))
    })
}

/// The first rule that matches.
pub fn choice<T: 'static>(rules: Vec<Rule<T>>) -> Rule<T> {
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        for r in &rules {
            if let Some(v) = r.run(cx)? {
                return Ok(Some(v));
            }
            cx.sc.pos = s;
        }
        Ok(None)
    })
}

/// `r`, or `None` where it does not match; always matches.
pub fn opt<T: 'static>(r: Rule<T>) -> Rule<Option<T>> {
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        match r.run(cx)? {
            Some(v) => Ok(Some(Some(v))),
            None => {
                cx.sc.pos = s;
                Ok(Some(None))
            }
        }
    })
}

/// `r` as many times as it matches and moves.
pub fn many<T: 'static>(r: Rule<T>) -> Rule<Vec<T>> {
    Rule::new(move |cx| {
        let mut out = Vec::new();
        loop {
            let p = cx.sc.pos;
            match r.run(cx)? {
                Some(v) if cx.sc.pos != p => out.push(v),
                _ => {
                    cx.sc.pos = p;
                    break;
                }
            }
        }
        Ok(Some(out))
    })
}

/// `r` at least once.
pub fn many1<T: 'static>(r: Rule<T>) -> Rule<Vec<T>> {
    let m = many(r);
    Rule::new(move |cx| Ok(m.run(cx)?.filter(|v| !v.is_empty())))
}

/// Whether `r` matches here, without moving.
pub fn ahead<T: 'static>(r: Rule<T>) -> Rule<T> {
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        let v = r.run(cx)?;
        cx.sc.pos = s;
        Ok(v)
    })
}

/// Matches, empty, only where `r` does not.
pub fn not<T: 'static>(r: Rule<T>) -> Rule<Span> {
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        let v = r.run(cx)?;
        cx.sc.pos = s;
        Ok(v.is_none().then(|| Span::from(s..s)))
    })
}

/// `r`, or refuse the input with `message` at the cursor.
pub fn expect<T: 'static>(r: Rule<T>, message: impl Into<String>) -> Rule<T> {
    let message = message.into();
    Rule::new(move |cx| match r.run(cx)? {
        Some(v) => Ok(Some(v)),
        None => Err(cx.fail(message.clone())),
    })
}

/// Refuse the input with `message` at the cursor.
pub fn fail<T: 'static>(message: impl Into<String>) -> Rule<T> {
    let message = message.into();
    Rule::new(move |cx| Err(cx.fail(message.clone())))
}

/// Refuse the input with `message` where `r` matches; otherwise empty.
pub fn fail_if<T: 'static>(r: Rule<T>, message: impl Into<String>) -> Rule<Span> {
    let message = message.into();
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        let v = r.run(cx)?;
        cx.sc.pos = s;
        if v.is_some() {
            return Err(cx.fail(message.clone()));
        }
        Ok(Some(Span::from(s..s)))
    })
}

/// `r`, its answer passed through `f`.
pub fn apply<T: 'static, U: 'static>(r: Rule<T>, f: impl Fn(T, &mut Ctx<'_>) -> Result<U, LanguageError> + Send + Sync + 'static) -> Rule<U> {
    Rule::new(move |cx| match r.run(cx)? {
        Some(v) => Ok(Some(f(v, cx)?)),
        None => Ok(None),
    })
}

// ── nodes ───────────────────────────────────────────────────────────────────

/// How a scalar rule makes its row.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct ScalarOpts {
    /// The row's text from what was matched; the matched text itself
    /// without it.
    pub text: Option<TextFn>,
    /// The row's kind from its text, in place of the rule's own.
    pub kind_of: Option<KindFn>,
    /// An extended scalar's kind.
    pub ext_kind: Option<ExtKind>,
}

setters!(ScalarOpts { opt text: TextFn, opt kind_of: KindFn, opt ext_kind: ExtKind, });

fn make_scalar(cx: &mut Ctx<'_>, kind: NodeKind, opts: &ScalarOpts, raw: &str, span: Span) -> Result<u32, LanguageError> {
    let text = match &opts.text {
        Some(f) => f(raw, span, &cx.sc)?,
        None => raw.to_owned(),
    };
    let kind = opts.kind_of.as_ref().map_or(kind, |f| f(&text));
    let mut row = NodeRow::new(kind, None, span);
    if kind != NodeKind::Null {
        row.text = Some(text);
    }
    row.ext_kind = opts.ext_kind;
    Ok(cx.rows.add(row))
}

/// A scalar of `kind` over what `r` matched.
pub fn scalar(kind: NodeKind, r: Rule<Span>, opts: ScalarOpts) -> Rule<u32> {
    Rule::new(move |cx| {
        let Some(span) = r.run(cx)? else { return Ok(None) };
        let raw = cx.sc.slice(span.start, span.end).into_owned();
        make_scalar(cx, kind, &opts, &raw, span).map(Some)
    })
}

/// A key: a string scalar over what `r` matched.
pub fn key(r: Rule<Span>) -> Rule<u32> {
    scalar(NodeKind::String, r, ScalarOpts::default())
}

/// A bare value: to the end of the line, or to where `stop` matches,
/// trimmed of spaces and tabs unless `trim` is off. May be empty unless
/// `empty` is off.
#[derive(Clone)]
#[non_exhaustive]
pub struct BareOpts {
    pub stop: Option<Rule<Span>>,
    pub trim: bool,
    pub empty: bool,
    pub kind: NodeKind,
    pub scalar: ScalarOpts,
}

impl Default for BareOpts {
    fn default() -> Self {
        BareOpts {
            stop: None,
            trim: true,
            empty: true,
            kind: NodeKind::String,
            scalar: ScalarOpts::default(),
        }
    }
}

setters!(BareOpts { opt stop: Rule<Span>, val trim: bool, val empty: bool, val kind: NodeKind, val scalar: ScalarOpts, });

/// A bare value, as [`BareOpts`] says.
pub fn bare(opts: BareOpts) -> Rule<u32> {
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        while let Some(c) = cx.sc.byte(0) {
            if c == b'\n' || c == b'\r' {
                break;
            }
            if let Some(stop) = &opts.stop {
                let p = cx.sc.pos;
                let hit = stop.run(cx)?.is_some();
                cx.sc.pos = p;
                if hit {
                    break;
                }
            }
            cx.sc.advance(1);
        }
        let (mut ts, mut te) = (s, cx.sc.pos);
        if opts.trim {
            let b = cx.sc.bytes();
            while ts < te && is_hs(b[ts]) {
                ts += 1;
            }
            while te > ts && is_hs(b[te - 1]) {
                te -= 1;
            }
        }
        if !opts.empty && ts == te {
            cx.sc.pos = s;
            return Ok(None);
        }
        let raw = cx.sc.slice(ts, te).into_owned();
        make_scalar(cx, opts.kind, &opts.scalar, &raw, Span::from(ts..te)).map(Some)
    })
}

/// What [`quoted`] runs once past the closing quote: it may refuse what
/// follows on the line, and may answer the offset a bad escape is
/// reported at.
pub type AfterFn = Arc<dyn Fn(&mut Ctx<'_>, Span) -> Result<Option<usize>, LanguageError> + Send + Sync>;

/// A quoted value.
#[derive(Clone)]
#[non_exhaustive]
pub struct QuotedOpts {
    pub open: String,
    /// The closing quote; `open` when `None`.
    pub close: Option<String>,
    /// The byte after `escape` and what it means; nothing escapes when
    /// empty.
    pub escapes: Vec<(u8, String)>,
    /// The escape byte (`\`).
    pub escape: u8,
    /// `\r\n` inside reads as `\n`.
    pub crlf: bool,
    /// A bare `\r` inside refuses the input with this message.
    pub bare_cr: Option<String>,
    pub unclosed: Option<String>,
    pub bad_escape: Option<String>,
    pub after: Option<AfterFn>,
    pub kind: NodeKind,
    pub scalar: ScalarOpts,
}

impl QuotedOpts {
    /// A value between two `open`s, with nothing escaped.
    pub fn new(open: impl Into<String>) -> Self {
        QuotedOpts {
            open: open.into(),
            close: None,
            escapes: Vec::new(),
            escape: b'\\',
            crlf: true,
            bare_cr: None,
            unclosed: None,
            bad_escape: None,
            after: None,
            kind: NodeKind::String,
            scalar: ScalarOpts::default(),
        }
    }

    /// Set what each byte after the escape byte means.
    pub fn escapes(mut self, escapes: &[(u8, &str)]) -> Self {
        self.escapes = escapes.iter().map(|(b, s)| (*b, (*s).to_owned())).collect();
        self
    }
}

setters!(QuotedOpts {
    opt close: String,
    opt bare_cr: String,
    opt unclosed: String,
    opt bad_escape: String,
    opt after: AfterFn,
    val escape: u8,
    val crlf: bool,
    val kind: NodeKind,
    val scalar: ScalarOpts,
});

/// A quoted value, as [`QuotedOpts`] says; its span includes the quotes.
pub fn quoted(opts: QuotedOpts) -> Rule<u32> {
    Rule::new(move |cx| {
        let close = opts.close.clone().unwrap_or_else(|| opts.open.clone());
        let unclosed = opts
            .unclosed
            .clone()
            .unwrap_or_else(|| format!("unclosed quoted value; expected a matching `{close}`"));
        let s = cx.sc.pos;
        if cx.sc.lit(&opts.open).is_none() {
            return Ok(None);
        }
        let bytes = cx.sc.bytes();
        let n = bytes.len();
        let escaping = !opts.escapes.is_empty();
        let mut i = cx.sc.pos;
        loop {
            if i >= n {
                return Err(LanguageError::at(unclosed, i));
            }
            let c = bytes[i];
            if c == b'\r' {
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 2;
                } else if let Some(m) = &opts.bare_cr {
                    return Err(LanguageError::at(m.clone(), i));
                } else {
                    i += 1;
                }
            } else if escaping && c == opts.escape {
                if i + 1 >= n {
                    return Err(LanguageError::at(unclosed, i));
                }
                i += 2;
            } else if bytes[i..].starts_with(close.as_bytes()) {
                i += close.len();
                break;
            } else {
                i += 1;
            }
        }
        cx.sc.pos = i;
        let span = Span::from(s..i);
        let err_at = match &opts.after {
            Some(f) => f(cx, span)?.unwrap_or(i),
            None => i,
        };
        let inner = &bytes[s + opts.open.len()..i - close.len()];
        let mut out: Vec<u8> = Vec::with_capacity(inner.len());
        let mut j = 0;
        while j < inner.len() {
            let ch = inner[j];
            if opts.crlf && ch == b'\r' && inner.get(j + 1) == Some(&b'\n') {
                out.push(b'\n');
                j += 2;
            } else if escaping && ch == opts.escape {
                let next = inner[j + 1];
                match opts.escapes.iter().find(|(b, _)| *b == next) {
                    Some((_, rep)) => out.extend_from_slice(rep.as_bytes()),
                    None => {
                        let message = opts.bad_escape.clone().unwrap_or_else(|| {
                            format!("invalid escape `{}{}` in a quoted value", opts.escape as char, next as char)
                        });
                        return Err(LanguageError::at(message, err_at));
                    }
                }
                j += 2;
            } else {
                out.push(ch);
                j += 1;
            }
        }
        let text = String::from_utf8_lossy(&out).into_owned();
        make_scalar(cx, opts.kind, &opts.scalar, &text, span).map(Some)
    })
}

/// A comment: `open` to the end of the line, or to `close`.
#[derive(Clone)]
#[non_exhaustive]
pub struct CommentOpts {
    pub open: String,
    pub close: Option<String>,
    /// Trim the text: spaces, tabs and `\r` at either end of a line
    /// comment, all whitespace at either end of a block one.
    pub trim: bool,
    /// Its form; block when there is a `close`, else line.
    pub style: Option<CommentForm>,
    pub unclosed: Option<String>,
}

impl CommentOpts {
    pub fn new(open: impl Into<String>) -> Self {
        CommentOpts {
            open: open.into(),
            close: None,
            trim: true,
            style: None,
            unclosed: None,
        }
    }
}

setters!(CommentOpts { opt close: String, opt style: CommentForm, opt unclosed: String, val trim: bool, });

/// A comment rule, as [`CommentOpts`] says.
pub fn comment(opts: CommentOpts) -> Rule<CommentMatch> {
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        if cx.sc.lit(&opts.open).is_none() {
            return Ok(None);
        }
        let body = cx.sc.pos;
        let end = match &opts.close {
            Some(close) => {
                let Some(at) = cx.sc.find(close) else {
                    let m = opts.unclosed.clone().unwrap_or_else(|| format!("unclosed comment; expected `{close}`"));
                    return Err(LanguageError::at(m, s));
                };
                cx.sc.pos = at + close.len();
                at
            }
            None => {
                let b = cx.sc.bytes();
                let at = b[body..].iter().position(|&c| c == b'\n' || c == b'\r').map_or(b.len(), |i| body + i);
                cx.sc.pos = at;
                at
            }
        };
        let mut text = cx.sc.slice(body, end).into_owned();
        if opts.trim {
            text = if opts.close.is_some() {
                text.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')).to_owned()
            } else {
                text.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\r')).to_owned()
            };
        }
        let style = opts
            .style
            .unwrap_or(if opts.close.is_some() { CommentForm::Block } else { CommentForm::Line });
        Ok(Some(CommentMatch {
            span: Span::from(s..cx.sc.pos),
            text,
            style,
        }))
    })
}

/// A line comment opened by `open`.
pub fn comment_line(open: impl Into<String>) -> Rule<CommentMatch> {
    comment(CommentOpts::new(open))
}

/// What lies between the things that matter.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct TriviaOpts {
    /// Spaces, tabs and line endings by default.
    pub space: Option<Rule<Span>>,
    pub comment: Option<Rule<CommentMatch>>,
}

setters!(TriviaOpts { opt space: Rule<Span>, opt comment: Rule<CommentMatch>, });

/// `space` and `comment`, in any order, comments bound as they pass.
/// Always matches.
pub fn trivia(opts: TriviaOpts) -> Rule<Span> {
    let space = opts.space.unwrap_or_else(|| choice(vec![hs1(), eol()]));
    let comment = opts.comment;
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        while !cx.sc.eof() {
            let p = cx.sc.pos;
            let moved = space.run(cx)?.is_some() && cx.sc.pos != p;
            if moved {
                continue;
            }
            cx.sc.pos = p;
            let Some(c) = &comment else { break };
            match c.run(cx)? {
                Some(m) => cx.comment(m),
                None => break,
            }
        }
        Ok(Some(Span::from(s..cx.sc.pos)))
    })
}

/// Which of an entry's nodes a comment is bound to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    #[default]
    Key,
    Value,
    Entry,
}

/// A keyvalue.
#[derive(Clone)]
#[non_exhaustive]
pub struct EntryOpts {
    /// Matched, and skipped when it does not match.
    pub prefix: Option<Rule<Span>>,
    pub key: Rule<u32>,
    /// Around the separator: spaces and tabs by default.
    pub between: Option<Rule<Span>>,
    pub sep: Option<Rule<Span>>,
    pub value: Option<Rule<u32>>,
    pub missing_sep: Option<String>,
    pub missing_value: Option<String>,
    /// Which node the waiting comments lead.
    pub leading: Target,
}

impl EntryOpts {
    pub fn new(key: Rule<u32>) -> Self {
        EntryOpts {
            prefix: None,
            key,
            between: None,
            sep: None,
            value: None,
            missing_sep: None,
            missing_value: None,
            leading: Target::Key,
        }
    }
}

setters!(EntryOpts {
    opt prefix: Rule<Span>,
    opt between: Rule<Span>,
    opt sep: Rule<Span>,
    opt value: Rule<u32>,
    opt missing_sep: String,
    opt missing_value: String,
    val leading: Target,
});

/// A keyvalue: the prefix, the key, the separator and the value. Once the
/// key has matched the rest is expected. Answers the keyvalue's row.
pub fn entry(opts: EntryOpts) -> Rule<u32> {
    let between = opts.between.clone().unwrap_or_else(hs);
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        if let Some(p) = &opts.prefix
            && p.run(cx)?.is_none()
        {
            cx.sc.pos = s;
        }
        let Some(k) = opts.key.run(cx)? else {
            cx.sc.pos = s;
            return Ok(None);
        };
        between.run(cx)?;
        if let Some(sep) = &opts.sep {
            if sep.run(cx)?.is_none() {
                let m = opts.missing_sep.clone().unwrap_or_else(|| "expected a separator after this key".into());
                return Err(cx.fail(m));
            }
            between.run(cx)?;
        }
        let v = match &opts.value {
            Some(value) => match value.run(cx)? {
                Some(v) => v,
                None => {
                    let m = opts.missing_value.clone().unwrap_or_else(|| "expected a value here".into());
                    return Err(cx.fail(m));
                }
            },
            // An entry with no value holds a null at its end.
            None => cx.rows.add(NodeRow::new(NodeKind::Null, None, Span::from(cx.sc.pos..cx.sc.pos))),
        };
        let ks = cx.span(k);
        let end = if opts.value.is_some() { cx.span(v).end } else { cx.sc.pos };
        let kv = cx.rows.add(NodeRow::new(NodeKind::KeyValue, None, Span::from(ks.start..end)));
        cx.rows.append(kv, k);
        cx.rows.append(kv, v);
        let target = match opts.leading {
            Target::Key => k,
            Target::Value => v,
            Target::Entry => kv,
        };
        cx.flush(target, CommentSlot::Leading);
        Ok(Some(kv))
    })
}

/// A container: entries or items between `open` and `close`.
#[derive(Clone)]
#[non_exhaustive]
pub struct MapOpts {
    pub open: Option<Rule<Span>>,
    /// The end of the input when `None`.
    pub close: Option<Rule<Span>>,
    pub entry: Rule<u32>,
    /// Between entries: whitespace by default.
    pub trivia: Option<Rule<Span>>,
    /// Runs where an entry is expected and none matches, to refuse the
    /// input with a message of its own.
    pub otherwise: Option<Hook>,
    pub expected: Option<String>,
    /// Runs after every entry.
    pub after: Option<Hook>,
    pub duplicates: DuplicatePolicy,
    /// The span is the whole input, for a root.
    pub whole: bool,
    pub unclosed: Option<String>,
    /// Which of an entry's nodes a comment on its line trails.
    pub trailing: Target,
}

impl MapOpts {
    pub fn new(entry: Rule<u32>) -> Self {
        MapOpts {
            open: None,
            close: None,
            entry,
            trivia: None,
            otherwise: None,
            expected: None,
            after: None,
            duplicates: DuplicatePolicy::LastValue,
            whole: false,
            unclosed: None,
            trailing: Target::Value,
        }
    }
}

setters!(MapOpts {
    opt open: Rule<Span>,
    opt close: Rule<Span>,
    opt trivia: Rule<Span>,
    opt otherwise: Hook,
    opt expected: String,
    opt after: Hook,
    opt unclosed: String,
    val duplicates: DuplicatePolicy,
    val whole: bool,
    val trailing: Target,
});

/// The container loop [`map`] and [`sequence`] share: `each` is run for
/// every member, after the trivia, until the close.
#[allow(clippy::too_many_arguments)]
fn members(
    cx: &mut Ctx<'_>,
    between: &Rule<Span>,
    close: &Option<Rule<Span>>,
    unclosed: &str,
    member: &Rule<u32>,
    otherwise: &Option<Hook>,
    expected: &str,
    after: &Option<Hook>,
    mut each: impl FnMut(&mut Ctx<'_>, u32) -> Result<(), LanguageError>,
) -> Result<(), LanguageError> {
    loop {
        between.run(cx)?;
        if let Some(close) = close {
            let p = cx.sc.pos;
            if close.run(cx)?.is_some() {
                break;
            }
            cx.sc.pos = p;
            if cx.sc.eof() {
                return Err(cx.fail(unclosed));
            }
        } else if cx.sc.eof() {
            break;
        }
        let Some(m) = member.run(cx)? else {
            if let Some(f) = otherwise {
                f(cx)?;
            }
            return Err(cx.fail(expected));
        };
        each(cx, m)?;
        if let Some(f) = after {
            f(cx)?;
        }
    }
    Ok(())
}

/// A mapping, as [`MapOpts`] says. Comments still waiting at the close are
/// its dangling comments.
pub fn map(opts: MapOpts) -> Rule<u32> {
    let between = opts.trivia.clone().unwrap_or_else(|| trivia(TriviaOpts::default()));
    let unclosed = opts
        .unclosed
        .clone()
        .unwrap_or_else(|| "unclosed mapping; expected its close before the end of the input".into());
    let expected = opts.expected.clone().unwrap_or_else(|| "expected an entry here".into());
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        if let Some(open) = &opts.open
            && open.run(cx)?.is_none()
        {
            return Ok(None);
        }
        let m = cx.rows.add(NodeRow::new(NodeKind::Mapping, None, Span::from(s..s)));
        let (policy, trailing) = (opts.duplicates, opts.trailing);
        members(cx, &between, &opts.close, &unclosed, &opts.entry, &opts.otherwise, &expected, &opts.after, |cx, kv| {
            cx.rows.put_entry(m, kv, policy)?;
            // The entry as it was read: a repeat's value, which may now be
            // the first entry's, takes the comment on its line.
            cx.last = Some(trailing_target(cx, kv, trailing));
            Ok(())
        })?;
        let span = if opts.whole { Span::from(0..cx.sc.len()) } else { Span::from(s..cx.sc.pos) };
        cx.rows.row_mut(m).span = Some(span);
        cx.flush(m, CommentSlot::Dangling);
        cx.last = Some(m);
        Ok(Some(m))
    })
}

/// Keyvalue `kv`'s key, value or itself.
fn trailing_target(cx: &Ctx<'_>, kv: u32, t: Target) -> u32 {
    match t {
        Target::Entry => kv,
        Target::Key => cx.rows.first_child(kv).unwrap_or(kv),
        Target::Value => cx
            .rows
            .first_child(kv)
            .and_then(|k| cx.rows.next_sibling(k))
            .unwrap_or(kv),
    }
}

/// A sequence: as [`MapOpts`], an item for an entry.
#[derive(Clone)]
#[non_exhaustive]
pub struct SequenceOpts {
    pub open: Option<Rule<Span>>,
    pub close: Option<Rule<Span>>,
    pub item: Rule<u32>,
    pub trivia: Option<Rule<Span>>,
    pub otherwise: Option<Hook>,
    pub expected: Option<String>,
    pub after: Option<Hook>,
    pub whole: bool,
    pub unclosed: Option<String>,
}

impl SequenceOpts {
    pub fn new(item: Rule<u32>) -> Self {
        SequenceOpts {
            open: None,
            close: None,
            item,
            trivia: None,
            otherwise: None,
            expected: None,
            after: None,
            whole: false,
            unclosed: None,
        }
    }
}

setters!(SequenceOpts {
    opt open: Rule<Span>,
    opt close: Rule<Span>,
    opt trivia: Rule<Span>,
    opt otherwise: Hook,
    opt expected: String,
    opt after: Hook,
    opt unclosed: String,
    val whole: bool,
});

/// A sequence, as [`SequenceOpts`] says. Each item takes the comments
/// waiting before it as leading.
pub fn sequence(opts: SequenceOpts) -> Rule<u32> {
    let between = opts.trivia.clone().unwrap_or_else(|| trivia(TriviaOpts::default()));
    let unclosed = opts
        .unclosed
        .clone()
        .unwrap_or_else(|| "unclosed sequence; expected its close before the end of the input".into());
    let expected = opts.expected.clone().unwrap_or_else(|| "expected an item here".into());
    Rule::new(move |cx| {
        let s = cx.sc.pos;
        if let Some(open) = &opts.open
            && open.run(cx)?.is_none()
        {
            return Ok(None);
        }
        let q = cx.rows.add(NodeRow::new(NodeKind::Sequence, None, Span::from(s..s)));
        members(cx, &between, &opts.close, &unclosed, &opts.item, &opts.otherwise, &expected, &opts.after, |cx, v| {
            cx.rows.append(q, v);
            cx.flush(v, CommentSlot::Leading);
            cx.last = Some(v);
            Ok(())
        })?;
        let span = if opts.whole { Span::from(0..cx.sc.len()) } else { Span::from(s..cx.sc.pos) };
        cx.rows.row_mut(q).span = Some(span);
        cx.flush(q, CommentSlot::Dangling);
        cx.last = Some(q);
        Ok(Some(q))
    })
}

/// A whole document.
#[derive(Clone)]
#[non_exhaustive]
pub struct DocOpts {
    pub root: Rule<u32>,
    /// Before and after the root.
    pub trivia: Option<Rule<Span>>,
    /// Skip a byte-order mark.
    pub bom: bool,
    /// The refusal when the root does not match.
    pub missing: Option<String>,
    /// The refusal when something follows the root.
    pub trailing: Option<String>,
    pub comments: Binding,
}

impl DocOpts {
    pub fn new(root: Rule<u32>) -> Self {
        DocOpts {
            root,
            trivia: None,
            bom: false,
            missing: None,
            trailing: None,
            comments: Binding::default(),
        }
    }
}

setters!(DocOpts { opt trivia: Rule<Span>, opt missing: String, opt trailing: String, val bom: bool, val comments: Binding, });

/// A grammar: a root rule that reads a whole document into a node table.
#[derive(Clone)]
pub struct Grammar(DocOpts);

/// A [`Grammar`] over `opts.root`. Comments left waiting at the end are the
/// root's dangling comments.
pub fn document(opts: DocOpts) -> Grammar {
    Grammar(opts)
}

impl Grammar {
    /// `input`'s node table, or why it is refused: a language's `parse`.
    pub fn parse(&self, input: &[u8]) -> Result<NodeTable, LanguageError> {
        let o = &self.0;
        let mut cx = Ctx::new(input, o.comments);
        if o.bom && input.starts_with(b"\xef\xbb\xbf") {
            cx.sc.pos = 3;
        }
        if let Some(t) = &o.trivia {
            t.run(&mut cx)?;
        }
        let Some(r) = o.root.run(&mut cx)? else {
            return Err(cx.fail(o.missing.clone().unwrap_or_else(|| "no document here".into())));
        };
        if let Some(t) = &o.trivia {
            t.run(&mut cx)?;
        }
        if !cx.sc.eof() {
            return Err(cx.fail(o.trailing.clone().unwrap_or_else(|| "unexpected content after the document".into())));
        }
        cx.flush(r, CommentSlot::Dangling);
        Ok(cx.rows.finish(r))
    }
}

// ── sections ────────────────────────────────────────────────────────────────
// A section format — INI, TOML, fig — opens a container with a header line
// and may re-enter it with another, and the table asks the same of each: a
// region for every line that opened or re-entered the container, the whole
// physical line, newline included; a mention for every place its name is
// written; and comments placed by where they fell. What a header means
// stays in the parser.

/// The whole physical line holding `at`, newline included.
pub fn line_region(bytes: &[u8], at: usize) -> Span {
    let mut s = at.min(bytes.len());
    while s > 0 && bytes[s - 1] != b'\n' {
        s -= 1;
    }
    let mut e = at.min(bytes.len());
    while e < bytes.len() && bytes[e] != b'\n' {
        e += 1;
    }
    if e < bytes.len() {
        e += 1;
    }
    Span::from(s..e)
}

/// The comments a hand-written section parser holds until a key claims
/// them, by the depth of the line each was met on.
#[derive(Clone, Debug, Default)]
pub struct Sections {
    pending: Vec<(String, usize, CommentForm)>,
}

impl Sections {
    pub fn new() -> Self {
        Self::default()
    }

    /// The line holding `at` in `bytes`, as a region of `node`.
    pub fn region(&self, w: &mut RowWriter, bytes: &[u8], node: u32, at: usize) {
        w.region(node, line_region(bytes, at));
    }

    /// Open a section: keyvalue `kv` over a container goes into mapping
    /// `parent`; the line holding its key is the container's first region,
    /// and the key its first mention, of `kind`. Answers the container.
    pub fn open(&self, w: &mut RowWriter, bytes: &[u8], parent: u32, kv: u32, kind: MentionKind) -> Result<u32, LanguageError> {
        let put = w.put_entry(parent, kv, DuplicatePolicy::Keep)?;
        let key = w.first_child(put.entry).expect("an entry has its key");
        let value = w.next_sibling(key).expect("an entry has its value");
        let ks = w.row(key).span.unwrap_or(Span::from(0..0));
        w.region(value, line_region(bytes, ks.start));
        w.mention(value, ks, kind);
        Ok(value)
    }

    /// Re-enter `node` by a header that names it at `span`.
    pub fn reopen(&self, w: &mut RowWriter, bytes: &[u8], node: u32, span: Span, kind: MentionKind) {
        w.region(node, line_region(bytes, span.start));
        w.mention(node, span, kind);
    }

    /// A comment waiting for its key, met on a line at `depth`.
    pub fn comment(&mut self, text: impl Into<String>, depth: usize, style: CommentForm) {
        self.pending.push((text.into(), depth, style));
    }

    /// Give the waiting comments from lines at `depth` or deeper to `node`,
    /// in `slot`; the shallower ones keep waiting.
    pub fn claim(&mut self, w: &mut RowWriter, node: u32, slot: CommentSlot, depth: usize) {
        let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending).into_iter().partition(|c| c.1 >= depth);
        self.pending = kept;
        for (text, _, style) in taken {
            w.comment(node, slot, style, text);
        }
    }
}
