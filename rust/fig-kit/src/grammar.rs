//! Port of kit/src/grammar.zig.
//!
//! A format as a description: combinators over a `Scanner` that write the
//! node table as they match, so the spans, the comment binding and the
//! duplicate policy follow from the grammar.
//!
//! The JavaScript kit's `@diaryx/fig/kit/grammar` and the Zig kit's
//! `grammar.zig`, in Rust: the same rules, the same messages, the same
//! comment binding. A rule is a value implementing `Rule`:
//!
//! ```text
//! type Out = T;
//! fn match_(&self, cx: &mut Ctx) -> Result<Option<T>>
//! ```
//!
//! answering `Some(T)` when it matches, `None` when it does not (the cursor
//! where it was, so a `choice` tries the next), or the error `Refused` with
//! the message and offset in `cx.failure`. The primitives answer spans; the
//! rules that make nodes (`scalar`, `key`, `bare`, `quoted`, `entry`, `map`,
//! `sequence`) write rows through a `RowWriter` and answer their ids;
//! `Document` reads a whole input into a table, and `Parser` makes it a
//! language's parser. Zig builds each rule as a type at compile time; here a
//! rule is a struct, and an option that names a rule holds it boxed
//! (`RuleExt::boxed`, `RuleExt::any`).
//!
//! Comments are bound as trivia is skipped: a comment on the same line as
//! the last node completed is that node's trailing comment; any other waits,
//! and becomes the leading comment of the next entry (on its key, by
//! default) or, when a container closes with it still waiting, a dangling
//! comment of that container. `Binding` takes the refinements some formats
//! add.
//!
//! Where the JavaScript grammar matches a regular expression, this one takes
//! a byte predicate (`bytes_while`, `ident`).
//!
//! Errors: `Refused` (with `Ctx::failure`), and `OutOfMemory` from the
//! table.

use std::borrow::Cow;
use std::sync::Arc;

use super::ast::{self, ExtKind, KeyValue, NodeId, NodeKind, Number, NumberKind, Style};
use super::child_index::ChildIndex;
use super::document::Document as Document_;
use super::flat_map::DuplicatePolicy;
use super::language::add_diagnostic;
use super::row_writer::RowWriter;
use super::scanner::Scanner;
use super::span::Span;
use super::table::{self, Table};
use crate::err;
use crate::{Error, Result};

/// How long a refusal's message may be: Zig formats it into a 256-byte
/// buffer, and keeps that many bytes of it past the parse.
const MESSAGE_MAX: usize = 256;

/// Why an input was refused, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub message: Vec<u8>,
    pub at: usize,
}

/// How comments bind beyond the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Binding {
    /// A comment trails only when it is the last thing on its line.
    pub ends_line: bool,
    /// A node takes one trailing comment; a second waits like any other.
    pub one: bool,
    /// A container that spans lines takes the comment on its closing line as
    /// dangling rather than trailing.
    pub closing_line: bool,
}

/// A comment a `comment` rule matched. Zig's `text` is the slice itself;
/// here it is the slice's span in the input (`Ctx::text`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comment {
    pub span: Span,
    pub text: Span,
    pub style: Style,
}

/// Which of an entry's nodes a comment is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Key,
    Value,
    Entry,
}

/// What every rule runs over: the scanner, the rows written so far and an
/// index of each container's last child and each mapping's entries, the
/// comments waiting to be bound, and the refusal once there is one.
#[derive(Debug)]
pub struct Ctx<'s, 't> {
    pub sc: Scanner<'s>,
    pub w: RowWriter<'t>,
    pub index: ChildIndex,
    pub pending: Vec<Comment>,
    /// The node a comment on its line trails.
    pub last: Option<NodeId>,
    pub binding: Binding,
    pub failure: Option<Failure>,
}

impl<'s, 't> Ctx<'s, 't> {
    /// A context over `input`, writing to `t`. For in-source texts, `input`
    /// is `t`'s own source (`Table::source_arc`).
    pub fn init(t: &'t mut Table, input: &'s [u8], binding: Binding) -> Ctx<'s, 't> {
        Ctx {
            sc: Scanner::init(input),
            w: RowWriter::init(t),
            index: ChildIndex::new(),
            pending: Vec::new(),
            last: None,
            binding,
            failure: None,
        }
    }

    /// Refuse the input with `message` at `at`: the error to return.
    pub fn fail(&mut self, message: &[u8], at: usize) -> Error {
        self.failure = Some(Failure { message: message.to_vec(), at });
        err!(Refused)
    }

    /// Refuse the input with `message` at the cursor.
    pub fn fail_here(&mut self, message: &[u8]) -> Error {
        let at = self.sc.pos;
        self.fail(message, at)
    }

    /// Refuse the input with a message formatted by the caller, at `at`,
    /// cut to the 256 bytes Zig's buffer holds.
    pub fn fail_fmt(&mut self, at: usize, mut message: Vec<u8>) -> Error {
        message.truncate(MESSAGE_MAX);
        self.failure = Some(Failure { message, at });
        err!(Refused)
    }

    /// Row `id`'s span.
    pub fn span(&self, id: NodeId) -> Span {
        self.w.span_of(id)
    }

    /// The input bytes `s` names.
    pub fn text(&self, s: Span) -> &'s [u8] {
        &self.sc.src[s.start..s.end]
    }

    /// Bind comment `c`, met in trivia: trailing on the last node when it is
    /// on that node's line, else waiting.
    pub fn comment(&mut self, c: Comment) -> Result<()> {
        if let Some(last) = self.last {
            let s = self.span(last);
            if self.sc.same_line(s.end, c.span.start) && (!self.binding.ends_line || self.rest_is_blank(c.span.end)) {
                let k = self.w.t.rows[last as usize].kind;
                let container = k == table::Kind::MAPPING || k == table::Kind::SEQUENCE;
                let slot = if self.binding.closing_line && container && !self.sc.same_line(s.start, s.end) {
                    table::COMMENT_DANGLING
                } else {
                    table::COMMENT_TRAILING
                };
                let text = self.text(c.text);
                self.w.comment(last, slot, ast::Comment { text, style: c.style })?;
                if self.binding.one {
                    self.last = None;
                }
                return Ok(());
            }
        }
        self.pending.push(c);
        self.last = None;
        Ok(())
    }

    fn rest_is_blank(&self, at: usize) -> bool {
        let b = self.sc.src;
        let mut i = at;
        while i < b.len() && Scanner::is_hs(b[i]) {
            i += 1;
        }
        i >= b.len() || b[i] == b'\n' || b[i] == b'\r'
    }

    /// Give every waiting comment to row `node`, in `slot`.
    pub fn flush(&mut self, node: NodeId, slot: u8) -> Result<()> {
        let src = self.sc.src;
        for c in std::mem::take(&mut self.pending) {
            self.w.comment(node, slot, ast::Comment { text: &src[c.text.start..c.text.end], style: c.style })?;
        }
        Ok(())
    }

    /// Put keyvalue `kv` in mapping `into` by `policy`, as `flat_map` does
    /// for a keyvalue already written: on a repeat that the policy does not
    /// keep, `kv` and its key are left unlinked for `finish` to drop.
    pub fn put_entry(&mut self, into: NodeId, kv: NodeId, policy: DuplicatePolicy) -> Result<()> {
        let e = self.w.kind_of(kv).as_keyvalue().expect("put_entry: not a keyvalue");
        let name: Option<Vec<u8>> = self.w.kind_of(e.key).as_string().map(|s| s.to_vec());
        let repeat = match &name {
            Some(n) => self.index.get(&self.w, into, n),
            None => None,
        };
        if let Some(existing) = repeat {
            match policy {
                DuplicatePolicy::Overwrite => {
                    let old_key = self.w.kind_of(existing).as_keyvalue().expect("an entry").key;
                    self.w.set(existing, NodeKind::KeyValue(KeyValue { key: old_key, value: e.value }))?;
                    return Ok(());
                }
                DuplicatePolicy::FirstValue => return Ok(()),
                DuplicatePolicy::Err => {
                    let at = self.span(e.key).start;
                    let mut m = b"duplicate key `".to_vec();
                    m.extend_from_slice(name.as_deref().unwrap_or(b""));
                    m.push(b'`');
                    return Err(self.fail_fmt(at, m));
                }
                DuplicatePolicy::Keep => {}
            }
        }
        self.index.append(&mut self.w, into, kv)?;
        if repeat.is_none() && name.is_some() {
            self.index.put(&self.w, kv)?;
        }
        Ok(())
    }
}

// ── rules ───────────────────────────────────────────────────────────────────

/// A grammar rule: `Some` when it matches, `None` (cursor unmoved) when it
/// does not, `Err(Refused)` to refuse the input.
pub trait Rule {
    type Out;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Self::Out>>;
}

impl<R: Rule + ?Sized> Rule for Box<R> {
    type Out = R::Out;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<R::Out>> {
        (**self).match_(cx)
    }
}

impl<R: Rule + ?Sized> Rule for &R {
    type Out = R::Out;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<R::Out>> {
        (**self).match_(cx)
    }
}

/// A rule whose answer is only whether it matched: what an option naming a
/// rule of any `Out` holds.
pub trait Matches {
    fn matches(&self, cx: &mut Ctx<'_, '_>) -> Result<bool>;
}

impl<R: Rule + ?Sized> Matches for R {
    fn matches(&self, cx: &mut Ctx<'_, '_>) -> Result<bool> {
        Ok(self.match_(cx)?.is_some())
    }
}

/// A boxed rule answering `O`.
pub type BoxRule<O> = Box<dyn Rule<Out = O>>;
/// A boxed rule of any answer.
pub type AnyRule = Box<dyn Matches>;

/// Boxing, for the options that hold a rule.
pub trait RuleExt: Rule + Sized + 'static {
    fn boxed(self) -> BoxRule<Self::Out> {
        Box::new(self)
    }
    fn any(self) -> AnyRule {
        Box::new(self)
    }
}

impl<R: Rule + Sized + 'static> RuleExt for R {}

// ── primitives ──────────────────────────────────────────────────────────────

/// The literal `s`.
#[derive(Debug, Clone, Copy)]
pub struct Lit(pub &'static [u8]);

pub fn lit(s: &'static [u8]) -> Lit {
    Lit(s)
}

impl Rule for Lit {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        Ok(cx.sc.lit(self.0))
    }
}

/// Bytes while `f` holds of them, possibly none.
#[derive(Debug, Clone, Copy)]
pub struct BytesWhile(pub fn(u8) -> bool);

pub fn bytes_while(f: fn(u8) -> bool) -> BytesWhile {
    BytesWhile(f)
}

impl Rule for BytesWhile {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        Ok(Some(cx.sc.take_while(self.0)))
    }
}

/// Bytes while `f` holds of them, at least one.
#[derive(Debug, Clone, Copy)]
pub struct BytesWhile1(pub fn(u8) -> bool);

pub fn bytes_while1(f: fn(u8) -> bool) -> BytesWhile1 {
    BytesWhile1(f)
}

impl Rule for BytesWhile1 {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        Ok(cx.sc.take_while1(self.0))
    }
}

/// One byte of which `first` holds, then bytes while `rest` does.
#[derive(Debug, Clone, Copy)]
pub struct Ident {
    pub first: fn(u8) -> bool,
    pub rest: fn(u8) -> bool,
}

pub fn ident(first: fn(u8) -> bool, rest: fn(u8) -> bool) -> Ident {
    Ident { first, rest }
}

impl Rule for Ident {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        let s = cx.sc.pos;
        let Some(b) = cx.sc.byte(0) else { return Ok(None) };
        if !(self.first)(b) {
            return Ok(None);
        }
        cx.sc.advance(1);
        cx.sc.take_while(self.rest);
        Ok(Some(Span::new(s, cx.sc.pos)))
    }
}

/// Matches, empty, where `f` holds of the scanner.
#[derive(Debug, Clone, Copy)]
pub struct When(pub fn(&Scanner<'_>) -> bool);

pub fn when(f: fn(&Scanner<'_>) -> bool) -> When {
    When(f)
}

impl Rule for When {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        Ok(if (self.0)(&cx.sc) { Some(Span::new(cx.sc.pos, cx.sc.pos)) } else { None })
    }
}

fn at_eof(sc: &Scanner<'_>) -> bool {
    sc.eof()
}

fn is_ws(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\r' || c == b'\n'
}

/// The end of the input.
pub fn eof() -> When {
    when(at_eof)
}

/// A line ending: `\n` or `\r\n`.
pub fn eol() -> Choice<(Lit, Lit)> {
    choice((lit(b"\n"), lit(b"\r\n")))
}

/// Spaces and tabs, possibly none.
pub fn hs() -> BytesWhile {
    bytes_while(Scanner::is_hs)
}

/// Spaces and tabs, at least one.
pub fn hs1() -> BytesWhile1 {
    bytes_while1(Scanner::is_hs)
}

/// All whitespace, possibly none.
pub fn ws() -> BytesWhile {
    bytes_while(is_ws)
}

/// At least one whitespace byte.
pub fn ws1() -> BytesWhile1 {
    bytes_while1(is_ws)
}

// ── combinators ─────────────────────────────────────────────────────────────

/// The rules of a `seq`: a tuple of rules of any answers.
pub trait SeqList {
    fn match_all(&self, cx: &mut Ctx<'_, '_>) -> Result<bool>;
}

/// The rules of a `choice`: a tuple of rules with one answer.
pub trait ChoiceList {
    type Out;
    fn first(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Self::Out>>;
}

macro_rules! tuple_lists {
    ($($T:ident $i:tt),+) => {
        impl<$($T: Rule),+> SeqList for ($($T,)+) {
            fn match_all(&self, cx: &mut Ctx<'_, '_>) -> Result<bool> {
                $( if self.$i.match_(cx)?.is_none() { return Ok(false); } )+
                Ok(true)
            }
        }
        impl<O, $($T: Rule<Out = O>),+> ChoiceList for ($($T,)+) {
            type Out = O;
            fn first(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<O>> {
                let s = cx.sc.pos;
                $(
                    if let Some(v) = self.$i.match_(cx)? { return Ok(Some(v)); }
                    cx.sc.pos = s;
                )+
                Ok(None)
            }
        }
    };
}

tuple_lists!(A 0);
tuple_lists!(A 0, B 1);
tuple_lists!(A 0, B 1, C 2);
tuple_lists!(A 0, B 1, C 2, D 3);
tuple_lists!(A 0, B 1, C 2, D 3, E 4);
tuple_lists!(A 0, B 1, C 2, D 3, E 4, F 5);
tuple_lists!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
tuple_lists!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);
tuple_lists!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8);
tuple_lists!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9);
tuple_lists!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10);
tuple_lists!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11);

impl SeqList for Vec<AnyRule> {
    fn match_all(&self, cx: &mut Ctx<'_, '_>) -> Result<bool> {
        for r in self {
            if !r.matches(cx)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl<O> ChoiceList for Vec<BoxRule<O>> {
    type Out = O;
    fn first(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<O>> {
        let s = cx.sc.pos;
        for r in self {
            if let Some(v) = r.match_(cx)? {
                return Ok(Some(v));
            }
            cx.sc.pos = s;
        }
        Ok(None)
    }
}

/// Each rule in turn; the whole span.
#[derive(Debug, Clone, Copy)]
pub struct Seq<T>(pub T);

pub fn seq<T: SeqList>(rules: T) -> Seq<T> {
    Seq(rules)
}

impl<T: SeqList> Rule for Seq<T> {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        let s = cx.sc.pos;
        if !self.0.match_all(cx)? {
            cx.sc.pos = s;
            return Ok(None);
        }
        Ok(Some(Span::new(s, cx.sc.pos)))
    }
}

/// The first rule that matches. Every rule answers the same `Out`.
#[derive(Debug, Clone, Copy)]
pub struct Choice<T>(pub T);

pub fn choice<T: ChoiceList>(rules: T) -> Choice<T> {
    Choice(rules)
}

impl<T: ChoiceList> Rule for Choice<T> {
    type Out = T::Out;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<T::Out>> {
        self.0.first(cx)
    }
}

/// `R`, or `None` where it does not match; always matches.
#[derive(Debug, Clone, Copy)]
pub struct Opt<R>(pub R);

pub fn opt<R: Rule>(r: R) -> Opt<R> {
    Opt(r)
}

impl<R: Rule> Rule for Opt<R> {
    type Out = Option<R::Out>;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Option<R::Out>>> {
        let s = cx.sc.pos;
        if let Some(v) = self.0.match_(cx)? {
            return Ok(Some(Some(v)));
        }
        cx.sc.pos = s;
        Ok(Some(None))
    }
}

fn many_match<R: Rule + ?Sized>(r: &R, cx: &mut Ctx<'_, '_>) -> Result<Span> {
    let s = cx.sc.pos;
    loop {
        let p = cx.sc.pos;
        if r.match_(cx)?.is_none() || cx.sc.pos == p {
            cx.sc.pos = p;
            break;
        }
    }
    Ok(Span::new(s, cx.sc.pos))
}

/// `R` as many times as it matches and moves; the span of them all.
#[derive(Debug, Clone, Copy)]
pub struct Many<R>(pub R);

pub fn many<R: Rule>(r: R) -> Many<R> {
    Many(r)
}

impl<R: Rule> Rule for Many<R> {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        Ok(Some(many_match(&self.0, cx)?))
    }
}

/// `R` at least once.
#[derive(Debug, Clone, Copy)]
pub struct Many1<R>(pub R);

pub fn many1<R: Rule>(r: R) -> Many1<R> {
    Many1(r)
}

impl<R: Rule> Rule for Many1<R> {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        let v = many_match(&self.0, cx)?;
        Ok(if v.is_empty() { None } else { Some(v) })
    }
}

/// Whether `R` matches here, without moving.
#[derive(Debug, Clone, Copy)]
pub struct Ahead<R>(pub R);

pub fn ahead<R: Rule>(r: R) -> Ahead<R> {
    Ahead(r)
}

impl<R: Rule> Rule for Ahead<R> {
    type Out = R::Out;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<R::Out>> {
        let s = cx.sc.pos;
        let v = self.0.match_(cx)?;
        cx.sc.pos = s;
        Ok(v)
    }
}

/// Matches, empty, only where `R` does not.
#[derive(Debug, Clone, Copy)]
pub struct Not<R>(pub R);

pub fn not<R: Rule>(r: R) -> Not<R> {
    Not(r)
}

impl<R: Rule> Rule for Not<R> {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        let s = cx.sc.pos;
        let v = self.0.match_(cx)?;
        cx.sc.pos = s;
        Ok(if v.is_none() { Some(Span::new(s, s)) } else { None })
    }
}

/// `R`, or refuse the input with `message` at the cursor.
#[derive(Debug, Clone, Copy)]
pub struct Expect<R> {
    pub r: R,
    pub message: &'static str,
}

pub fn expect<R: Rule>(r: R, message: &'static str) -> Expect<R> {
    Expect { r, message }
}

impl<R: Rule> Rule for Expect<R> {
    type Out = R::Out;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<R::Out>> {
        match self.r.match_(cx)? {
            Some(v) => Ok(Some(v)),
            None => Err(cx.fail_here(self.message.as_bytes())),
        }
    }
}

/// Refuse the input with `message` at the cursor.
#[derive(Debug, Clone, Copy)]
pub struct Fail(pub &'static str);

pub fn fail(message: &'static str) -> Fail {
    Fail(message)
}

impl Rule for Fail {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        Err(cx.fail_here(self.0.as_bytes()))
    }
}

/// Refuse the input with `message` where `R` matches; otherwise empty.
#[derive(Debug, Clone, Copy)]
pub struct FailIf<R> {
    pub r: R,
    pub message: &'static str,
}

pub fn fail_if<R: Rule>(r: R, message: &'static str) -> FailIf<R> {
    FailIf { r, message }
}

impl<R: Rule> Rule for FailIf<R> {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        let s = cx.sc.pos;
        let v = self.r.match_(cx)?;
        cx.sc.pos = s;
        if v.is_some() {
            return Err(cx.fail_here(self.message.as_bytes()));
        }
        Ok(Some(Span::new(s, s)))
    }
}

/// `R`, its answer passed through `f`.
pub struct Apply<R: Rule, U> {
    pub r: R,
    pub f: fn(&mut Ctx<'_, '_>, R::Out) -> Result<U>,
}

pub fn apply<R: Rule, U>(r: R, f: fn(&mut Ctx<'_, '_>, R::Out) -> Result<U>) -> Apply<R, U> {
    Apply { r, f }
}

impl<R: Rule, U> Rule for Apply<R, U> {
    type Out = U;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<U>> {
        let Some(v) = self.r.match_(cx)? else { return Ok(None) };
        Ok(Some((self.f)(cx, v)?))
    }
}

// ── nodes ───────────────────────────────────────────────────────────────────

/// A scalar row's kind, before its text gives it one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScalarKind {
    Null,
    Boolean,
    Int,
    Float,
    #[default]
    String,
}

/// The row's text from what was matched and where. What it answers is
/// borrowed from the input or owned; the row copies an owned one.
pub type TextFn = for<'c, 's, 't> fn(&'c mut Ctx<'s, 't>, &'s [u8], Span) -> Result<Cow<'s, [u8]>>;

/// How a scalar rule makes its row.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScalarOpts {
    /// The row's text from what was matched; the matched text itself
    /// without it.
    pub text: Option<TextFn>,
    /// The row's kind from its text, in place of the rule's own.
    pub kind_of: Option<fn(&[u8]) -> ScalarKind>,
    /// An extended scalar's kind.
    pub ext_kind: Option<ExtKind>,
}

fn make_scalar<'s>(cx: &mut Ctx<'s, '_>, kind: ScalarKind, opts: ScalarOpts, raw: &'s [u8], span: Span) -> Result<NodeId> {
    let text: Cow<'s, [u8]> = match opts.text {
        Some(f) => f(cx, raw, span)?,
        None => Cow::Borrowed(raw),
    };
    write_scalar(cx, kind, opts.kind_of, opts.ext_kind, &text, span)
}

fn write_scalar(
    cx: &mut Ctx<'_, '_>,
    kind: ScalarKind,
    kind_of: Option<fn(&[u8]) -> ScalarKind>,
    ext_kind: Option<ExtKind>,
    text: &[u8],
    span: Span,
) -> Result<NodeId> {
    let k = match kind_of {
        Some(f) => f(text),
        None => kind,
    };
    let node = match ext_kind {
        Some(e) => NodeKind::Extended(ast::Extended { kind: e, text }),
        None => match k {
            ScalarKind::Null => NodeKind::Null,
            ScalarKind::Boolean => NodeKind::Boolean(text == b"true"),
            ScalarKind::Int => NodeKind::Number(Number { raw: text, kind: NumberKind::Integer }),
            ScalarKind::Float => NodeKind::Number(Number { raw: text, kind: NumberKind::Float }),
            ScalarKind::String => NodeKind::String(text),
        },
    };
    cx.w.add(node, span)
}

/// A scalar of `kind` over what `R` matched.
#[derive(Debug, Clone, Copy)]
pub struct Scalar<R> {
    pub kind: ScalarKind,
    pub r: R,
    pub opts: ScalarOpts,
}

pub fn scalar<R: Rule<Out = Span>>(kind: ScalarKind, r: R, opts: ScalarOpts) -> Scalar<R> {
    Scalar { kind, r, opts }
}

impl<R: Rule<Out = Span>> Rule for Scalar<R> {
    type Out = NodeId;
    fn match_<'s>(&self, cx: &mut Ctx<'s, '_>) -> Result<Option<NodeId>> {
        let Some(span) = self.r.match_(cx)? else { return Ok(None) };
        let src: &'s [u8] = cx.sc.src;
        Ok(Some(make_scalar(cx, self.kind, self.opts, &src[span.start..span.end], span)?))
    }
}

/// A key: a string scalar over what `R` matched.
pub fn key<R: Rule<Out = Span>>(r: R) -> Scalar<R> {
    scalar(ScalarKind::String, r, ScalarOpts::default())
}

pub struct BareOpts {
    /// Where the value stops, besides the line's end.
    pub stop: Option<AnyRule>,
    /// Default true.
    pub trim: bool,
    /// Default true.
    pub empty: bool,
    pub kind: ScalarKind,
    pub scalar: ScalarOpts,
}

impl Default for BareOpts {
    fn default() -> Self {
        BareOpts { stop: None, trim: true, empty: true, kind: ScalarKind::String, scalar: ScalarOpts::default() }
    }
}

/// A bare value: to the end of the line, or to where `stop` matches, trimmed
/// of spaces and tabs unless `trim` is off. May be empty unless `empty` is
/// off.
pub struct Bare(pub BareOpts);

pub fn bare(opts: BareOpts) -> Bare {
    Bare(opts)
}

impl Rule for Bare {
    type Out = NodeId;
    fn match_<'s>(&self, cx: &mut Ctx<'s, '_>) -> Result<Option<NodeId>> {
        let opts = &self.0;
        let s = cx.sc.pos;
        while let Some(c) = cx.sc.byte(0) {
            if c == b'\n' || c == b'\r' {
                break;
            }
            if let Some(stop) = &opts.stop {
                let p = cx.sc.pos;
                let hit = stop.matches(cx)?;
                cx.sc.pos = p;
                if hit {
                    break;
                }
            }
            cx.sc.advance(1);
        }
        let mut ts = s;
        let mut te = cx.sc.pos;
        let b: &'s [u8] = cx.sc.src;
        if opts.trim {
            while ts < te && Scanner::is_hs(b[ts]) {
                ts += 1;
            }
            while te > ts && Scanner::is_hs(b[te - 1]) {
                te -= 1;
            }
        }
        if !opts.empty && ts == te {
            cx.sc.pos = s;
            return Ok(None);
        }
        Ok(Some(make_scalar(cx, opts.kind, opts.scalar, &b[ts..te], Span::new(ts, te))?))
    }
}

/// One escape: the byte after the escape byte, and what it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Escape {
    pub byte: u8,
    pub means: &'static [u8],
}

/// Runs once past a quoted value's closing quote: may refuse what follows on
/// the line, and may answer the offset a bad escape is reported at.
pub type AfterFn = fn(&mut Ctx<'_, '_>, Span) -> Result<Option<usize>>;

#[derive(Debug, Clone, Copy)]
pub struct QuotedOpts {
    pub open: &'static [u8],
    /// `open` when `None`.
    pub close: Option<&'static [u8]>,
    /// Nothing escapes when empty.
    pub escapes: &'static [Escape],
    /// Default `\`.
    pub escape: u8,
    /// `\r\n` inside reads as `\n`. Default true.
    pub crlf: bool,
    /// A bare `\r` inside refuses the input with this message.
    pub bare_cr: Option<&'static str>,
    pub unclosed: Option<&'static str>,
    pub bad_escape: Option<&'static str>,
    pub after: Option<AfterFn>,
    pub kind: ScalarKind,
}

impl QuotedOpts {
    /// Quoted by `open`, with Zig's defaults for the rest.
    pub const fn new(open: &'static [u8]) -> QuotedOpts {
        QuotedOpts {
            open,
            close: None,
            escapes: &[],
            escape: b'\\',
            crlf: true,
            bare_cr: None,
            unclosed: None,
            bad_escape: None,
            after: None,
            kind: ScalarKind::String,
        }
    }
}

/// A quoted value; its span includes the quotes.
#[derive(Debug, Clone, Copy)]
pub struct Quoted(pub QuotedOpts);

pub fn quoted(opts: QuotedOpts) -> Quoted {
    Quoted(opts)
}

impl Rule for Quoted {
    type Out = NodeId;
    fn match_<'s>(&self, cx: &mut Ctx<'s, '_>) -> Result<Option<NodeId>> {
        let opts = &self.0;
        let close = opts.close.unwrap_or(opts.open);
        let unclosed: Vec<u8> = match opts.unclosed {
            Some(m) => m.as_bytes().to_vec(),
            None => {
                let mut m = b"unclosed quoted value; expected a matching `".to_vec();
                m.extend_from_slice(close);
                m.push(b'`');
                m
            }
        };
        let escaping = !opts.escapes.is_empty();
        let s = cx.sc.pos;
        if cx.sc.lit(opts.open).is_none() {
            return Ok(None);
        }
        let bytes: &'s [u8] = cx.sc.src;
        let mut i = cx.sc.pos;
        loop {
            if i >= bytes.len() {
                return Err(cx.fail(&unclosed, i));
            }
            let c = bytes[i];
            if c == b'\r' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 2;
                } else if let Some(m) = opts.bare_cr {
                    return Err(cx.fail(m.as_bytes(), i));
                } else {
                    i += 1;
                }
            } else if escaping && c == opts.escape {
                if i + 1 >= bytes.len() {
                    return Err(cx.fail(&unclosed, i));
                }
                i += 2;
            } else if bytes[i..].starts_with(close) {
                i += close.len();
                break;
            } else {
                i += 1;
            }
        }
        cx.sc.pos = i;
        let span = Span::new(s, i);
        let err_at = match opts.after {
            Some(f) => f(cx, span)?.unwrap_or(i),
            None => i,
        };
        let inner: &'s [u8] = &bytes[s + opts.open.len()..i - close.len()];
        // The text is the inner bytes themselves unless something in them
        // reads otherwise.
        let plain = !inner.contains(&b'\r') && (!escaping || !inner.contains(&opts.escape));
        if plain {
            return Ok(Some(make_scalar(cx, opts.kind, ScalarOpts::default(), inner, span)?));
        }
        let mut out: Vec<u8> = Vec::new();
        let mut j = 0usize;
        while j < inner.len() {
            let ch = inner[j];
            if opts.crlf && ch == b'\r' && j + 1 < inner.len() && inner[j + 1] == b'\n' {
                out.push(b'\n');
                j += 2;
            } else if escaping && ch == opts.escape {
                let next = inner[j + 1];
                let Some(e) = opts.escapes.iter().find(|e| e.byte == next) else {
                    if let Some(m) = opts.bad_escape {
                        return Err(cx.fail(m.as_bytes(), err_at));
                    }
                    let mut m = b"invalid escape `".to_vec();
                    m.push(opts.escape);
                    m.push(next);
                    m.extend_from_slice(b"` in a quoted value");
                    return Err(cx.fail_fmt(err_at, m));
                };
                out.extend_from_slice(e.means);
                j += 2;
            } else {
                out.push(ch);
                j += 1;
            }
        }
        Ok(Some(write_scalar(cx, opts.kind, None, None, &out, span)?))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CommentOpts {
    pub open: &'static [u8],
    pub close: Option<&'static [u8]>,
    /// Trim spaces, tabs and `\r` at either end of a line comment, all
    /// whitespace at either end of a block one. Default true.
    pub trim: bool,
    /// Block when there is a `close`, else line.
    pub style: Option<Style>,
    pub unclosed: Option<&'static str>,
}

impl CommentOpts {
    /// Opened by `open`, with Zig's defaults for the rest.
    pub const fn new(open: &'static [u8]) -> CommentOpts {
        CommentOpts { open, close: None, trim: true, style: None, unclosed: None }
    }
}

/// A comment: `open` to the end of the line, or to `close`.
#[derive(Debug, Clone, Copy)]
pub struct CommentRule(pub CommentOpts);

pub fn comment(opts: CommentOpts) -> CommentRule {
    CommentRule(opts)
}

impl Rule for CommentRule {
    type Out = Comment;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Comment>> {
        let opts = &self.0;
        let style = opts.style.unwrap_or(if opts.close.is_some() { Style::Block } else { Style::Line });
        let s = cx.sc.pos;
        if cx.sc.lit(opts.open).is_none() {
            return Ok(None);
        }
        let body = cx.sc.pos;
        let end;
        if let Some(c) = opts.close {
            let Some(e) = cx.sc.find(c) else {
                let m: Vec<u8> = match opts.unclosed {
                    Some(m) => m.as_bytes().to_vec(),
                    None => {
                        let mut m = b"unclosed comment; expected `".to_vec();
                        m.extend_from_slice(c);
                        m.push(b'`');
                        m
                    }
                };
                return Err(cx.fail(&m, s));
            };
            end = e;
            cx.sc.pos = end + c.len();
        } else {
            let rest = &cx.sc.src[body..];
            end = body + rest.iter().position(|&b| b == b'\r' || b == b'\n').unwrap_or(rest.len());
            cx.sc.pos = end;
        }
        let mut ts = body;
        let mut te = end;
        if opts.trim {
            let strip: &[u8] = if opts.close.is_some() { b" \t\n\x0b\x0c\r" } else { b" \t\r" };
            let src = cx.sc.src;
            while ts < te && strip.contains(&src[ts]) {
                ts += 1;
            }
            while te > ts && strip.contains(&src[te - 1]) {
                te -= 1;
            }
        }
        Ok(Some(Comment { span: Span::new(s, cx.sc.pos), text: Span::new(ts, te), style }))
    }
}

pub struct TriviaOpts {
    /// Spaces, tabs and line endings by default (`choice((hs1(), eol()))`).
    pub space: AnyRule,
    pub comment: Option<BoxRule<Comment>>,
}

impl Default for TriviaOpts {
    fn default() -> Self {
        TriviaOpts { space: choice((hs1(), eol())).any(), comment: None }
    }
}

/// What lies between the things that matter: `space` and `comment`, in any
/// order, comments bound as they pass. Always matches.
pub struct Trivia(pub TriviaOpts);

pub fn trivia(opts: TriviaOpts) -> Trivia {
    Trivia(opts)
}

impl Rule for Trivia {
    type Out = Span;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<Span>> {
        let opts = &self.0;
        let s = cx.sc.pos;
        while !cx.sc.eof() {
            let p = cx.sc.pos;
            if opts.space.matches(cx)? && cx.sc.pos != p {
                continue;
            }
            cx.sc.pos = p;
            let Some(c_rule) = &opts.comment else { break };
            let Some(c) = c_rule.match_(cx)? else { break };
            cx.comment(c)?;
        }
        Ok(Some(Span::new(s, cx.sc.pos)))
    }
}

pub struct EntryOpts {
    /// Matched, and skipped when it does not match.
    pub prefix: Option<AnyRule>,
    pub key: BoxRule<NodeId>,
    /// Around the separator. Default `hs()`.
    pub between: AnyRule,
    pub sep: Option<AnyRule>,
    pub value: Option<BoxRule<NodeId>>,
    pub missing_sep: &'static str,
    pub missing_value: &'static str,
    /// Which node the waiting comments lead. Default `Target::Key`.
    pub leading: Target,
}

impl EntryOpts {
    /// An entry whose key is `key`, with Zig's defaults for the rest.
    pub fn new(key: BoxRule<NodeId>) -> EntryOpts {
        EntryOpts {
            prefix: None,
            key,
            between: hs().any(),
            sep: None,
            value: None,
            missing_sep: "expected a separator after this key",
            missing_value: "expected a value here",
            leading: Target::Key,
        }
    }
}

/// A keyvalue: the prefix, the key, the separator and the value. Once the
/// key has matched the rest is expected. Answers the keyvalue's row.
pub struct Entry(pub EntryOpts);

pub fn entry(opts: EntryOpts) -> Entry {
    Entry(opts)
}

impl Rule for Entry {
    type Out = NodeId;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<NodeId>> {
        let opts = &self.0;
        let s = cx.sc.pos;
        if let Some(p) = &opts.prefix {
            if !p.matches(cx)? {
                cx.sc.pos = s;
            }
        }
        let Some(k) = opts.key.match_(cx)? else {
            cx.sc.pos = s;
            return Ok(None);
        };
        opts.between.matches(cx)?;
        if let Some(sep) = &opts.sep {
            if !sep.matches(cx)? {
                return Err(cx.fail_here(opts.missing_sep.as_bytes()));
            }
            opts.between.matches(cx)?;
        }
        let v = match &opts.value {
            Some(vr) => match vr.match_(cx)? {
                Some(v) => v,
                None => return Err(cx.fail_here(opts.missing_value.as_bytes())),
            },
            None => {
                let at = cx.sc.pos;
                cx.w.add(NodeKind::Null, Span::new(at, at))?
            }
        };
        let end = cx.span(v).end;
        let kv = cx.w.add(NodeKind::KeyValue(KeyValue { key: k, value: v }), Span::new(cx.span(k).start, end))?;
        let target = match opts.leading {
            Target::Key => k,
            Target::Value => v,
            Target::Entry => kv,
        };
        cx.flush(target, table::COMMENT_LEADING)?;
        Ok(Some(kv))
    }
}

/// A hook a container rule runs: where an entry is expected and none
/// matches (`otherwise`), or after every entry (`after`).
pub type CtxFn = fn(&mut Ctx<'_, '_>) -> Result<()>;

pub struct MapOpts {
    pub open: Option<AnyRule>,
    /// The end of the input when `None`.
    pub close: Option<AnyRule>,
    pub entry: BoxRule<NodeId>,
    /// Between entries. Default `trivia(TriviaOpts::default())`.
    pub trivia: AnyRule,
    /// Runs where an entry is expected and none matches, to refuse the input
    /// with a message of its own.
    pub otherwise: Option<CtxFn>,
    pub expected: &'static str,
    /// Runs after every entry.
    pub after: Option<CtxFn>,
    pub duplicates: DuplicatePolicy,
    /// The span is the whole input, for a root.
    pub whole: bool,
    pub unclosed: &'static str,
    /// Which of an entry's nodes a comment on its line trails. Default
    /// `Target::Value`.
    pub trailing: Target,
}

impl MapOpts {
    /// A mapping of `entry`, with Zig's defaults for the rest.
    pub fn new(entry: BoxRule<NodeId>) -> MapOpts {
        MapOpts {
            open: None,
            close: None,
            entry,
            trivia: trivia(TriviaOpts::default()).any(),
            otherwise: None,
            expected: "expected an entry here",
            after: None,
            duplicates: DuplicatePolicy::Overwrite,
            whole: false,
            unclosed: "unclosed mapping; expected its close before the end of the input",
            trailing: Target::Value,
        }
    }
}

/// A mapping. Comments still waiting at the close are its dangling ones.
pub struct Map(pub MapOpts);

pub fn map(opts: MapOpts) -> Map {
    Map(opts)
}

impl Rule for Map {
    type Out = NodeId;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<NodeId>> {
        let opts = &self.0;
        let s = cx.sc.pos;
        if let Some(o) = &opts.open {
            if !o.matches(cx)? {
                return Ok(None);
            }
        }
        let m = cx.w.add(NodeKind::Mapping(None), Span::new(s, s))?;
        loop {
            opts.trivia.matches(cx)?;
            if let Some(c) = &opts.close {
                let p = cx.sc.pos;
                if c.matches(cx)? {
                    break;
                }
                cx.sc.pos = p;
                if cx.sc.eof() {
                    return Err(cx.fail_here(opts.unclosed.as_bytes()));
                }
            } else if cx.sc.eof() {
                break;
            }
            let Some(kv) = opts.entry.match_(cx)? else {
                if let Some(f) = opts.otherwise {
                    f(cx)?;
                }
                return Err(cx.fail_here(opts.expected.as_bytes()));
            };
            cx.put_entry(m, kv, opts.duplicates)?;
            let e = cx.w.kind_of(kv).as_keyvalue().expect("an entry");
            cx.last = Some(match opts.trailing {
                Target::Key => e.key,
                Target::Value => e.value,
                Target::Entry => kv,
            });
            if let Some(f) = opts.after {
                f(cx)?;
            }
        }
        let span = if opts.whole { Span::new(0, cx.sc.src.len()) } else { Span::new(s, cx.sc.pos) };
        cx.w.set_span(m, span);
        cx.flush(m, table::COMMENT_DANGLING)?;
        cx.last = Some(m);
        Ok(Some(m))
    }
}

pub struct SequenceOpts {
    pub open: Option<AnyRule>,
    pub close: Option<AnyRule>,
    pub item: BoxRule<NodeId>,
    pub trivia: AnyRule,
    pub otherwise: Option<CtxFn>,
    pub expected: &'static str,
    pub after: Option<CtxFn>,
    pub whole: bool,
    pub unclosed: &'static str,
}

impl SequenceOpts {
    /// A sequence of `item`, with Zig's defaults for the rest.
    pub fn new(item: BoxRule<NodeId>) -> SequenceOpts {
        SequenceOpts {
            open: None,
            close: None,
            item,
            trivia: trivia(TriviaOpts::default()).any(),
            otherwise: None,
            expected: "expected an item here",
            after: None,
            whole: false,
            unclosed: "unclosed sequence; expected its close before the end of the input",
        }
    }
}

/// A sequence. Each item takes the comments waiting before it as leading.
pub struct Sequence(pub SequenceOpts);

pub fn sequence(opts: SequenceOpts) -> Sequence {
    Sequence(opts)
}

impl Rule for Sequence {
    type Out = NodeId;
    fn match_(&self, cx: &mut Ctx<'_, '_>) -> Result<Option<NodeId>> {
        let opts = &self.0;
        let s = cx.sc.pos;
        if let Some(o) = &opts.open {
            if !o.matches(cx)? {
                return Ok(None);
            }
        }
        let q = cx.w.add(NodeKind::Sequence(None), Span::new(s, s))?;
        loop {
            opts.trivia.matches(cx)?;
            if let Some(c) = &opts.close {
                let p = cx.sc.pos;
                if c.matches(cx)? {
                    break;
                }
                cx.sc.pos = p;
                if cx.sc.eof() {
                    return Err(cx.fail_here(opts.unclosed.as_bytes()));
                }
            } else if cx.sc.eof() {
                break;
            }
            let Some(v) = opts.item.match_(cx)? else {
                if let Some(f) = opts.otherwise {
                    f(cx)?;
                }
                return Err(cx.fail_here(opts.expected.as_bytes()));
            };
            cx.index.append(&mut cx.w, q, v)?;
            cx.flush(v, table::COMMENT_LEADING)?;
            cx.last = Some(v);
            if let Some(f) = opts.after {
                f(cx)?;
            }
        }
        let span = if opts.whole { Span::new(0, cx.sc.src.len()) } else { Span::new(s, cx.sc.pos) };
        cx.w.set_span(q, span);
        cx.flush(q, table::COMMENT_DANGLING)?;
        cx.last = Some(q);
        Ok(Some(q))
    }
}

pub struct DocOpts {
    pub root: BoxRule<NodeId>,
    /// Before and after the root.
    pub trivia: Option<AnyRule>,
    /// Skip a byte-order mark.
    pub bom: bool,
    pub missing: &'static str,
    pub trailing: &'static str,
    pub comments: Binding,
}

impl DocOpts {
    /// A document whose root is `root`, with Zig's defaults for the rest.
    pub fn new(root: BoxRule<NodeId>) -> DocOpts {
        DocOpts {
            root,
            trivia: None,
            bom: false,
            missing: "no document here",
            trailing: "unexpected content after the document",
            comments: Binding::default(),
        }
    }
}

/// A whole document: `root` between `trivia`, then the end of the input.
/// Comments left waiting at the end are the root's dangling comments.
pub struct Document(pub DocOpts);

impl Document {
    pub fn new(opts: DocOpts) -> Document {
        Document(opts)
    }

    /// Write `input`'s rows into `t`, finished. On a refusal, `Refused` with
    /// why in `*failure` (its message at most 256 bytes, as Zig keeps it).
    /// For in-source texts, `input` is `t`'s own source.
    pub fn parse_into(&self, t: &mut Table, input: &[u8], failure: &mut Option<Failure>) -> Result<()> {
        let opts = &self.0;
        let mut cx = Ctx::init(t, input, opts.comments);
        let r = Self::run(opts, &mut cx);
        if r.is_err() {
            if let Some(f) = cx.failure.take() {
                *failure = Some(keep(f));
            }
        }
        r
    }

    fn run(opts: &DocOpts, cx: &mut Ctx<'_, '_>) -> Result<()> {
        if opts.bom && cx.sc.src.starts_with(b"\xef\xbb\xbf") {
            cx.sc.pos = 3;
        }
        if let Some(t) = &opts.trivia {
            t.matches(cx)?;
        }
        let Some(r) = opts.root.match_(cx)? else { return Err(cx.fail_here(opts.missing.as_bytes())) };
        if let Some(t) = &opts.trivia {
            t.matches(cx)?;
        }
        if !cx.sc.eof() {
            return Err(cx.fail_here(opts.trailing.as_bytes()));
        }
        cx.flush(r, table::COMMENT_DANGLING)?;
        cx.w.finish(r)
    }
}

/// A failure whose message outlives the context that wrote it: at most 256
/// bytes of it.
fn keep(mut f: Failure) -> Failure {
    f.message.truncate(MESSAGE_MAX);
    f
}

/// What a grammar parser reports of a refusal: its message (Zig's `code`,
/// which `describe` reads as itself) and offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Vec<u8>,
    pub offset: usize,
    pub end: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub diag: Option<Diagnostic>,
}

/// The parser of a language whose grammar is `doc` (Zig's
/// `Parser(Doc, T)`; the dialect, which a grammar does not read, is not
/// taken): what a `Language` implementation calls, with a refusal's message
/// and offset in its report.
pub struct Parser {
    pub doc: Document,
}

impl Parser {
    pub fn new(doc: Document) -> Parser {
        Parser { doc }
    }

    pub fn describe(code: &[u8]) -> &[u8] {
        code
    }

    pub fn short_label(_: &[u8]) -> &'static [u8] {
        b"here"
    }

    /// Write `input`'s rows into `out`, with a refusal in `report`.
    pub fn parse_with_report_into(&self, out: &mut Table, input: &[u8], report: &mut Report) -> Result<()> {
        let mut failure: Option<Failure> = None;
        let r = self.doc.parse_into(out, input, &mut failure);
        if r.is_err() {
            if let Some(f) = failure {
                report.diag = Some(Diagnostic { code: f.message, offset: f.at, end: None });
            }
        }
        r
    }

    /// `Language::parse_table` for this grammar, as `kit/contract.zig`'s
    /// `parse` thunk runs a parser with a report: the rows into `out` (a
    /// table over `input`; its source is what is scanned), and a refusal's
    /// diagnostic into `out.diagnostics` before the error is returned.
    pub fn parse_table(&self, out: &mut Table, report: &mut Report) -> Result<()> {
        let src = out.source_arc();
        let r = self.parse_with_report_into(out, &src, report);
        if r.is_err() {
            if let Some(d) = &report.diag {
                add_diagnostic(out, table::DIAGNOSTIC_ERROR, d.offset, d.end, Self::describe(&d.code), Self::short_label(&d.code))?;
            }
        }
        r
    }

    /// `input` as the document fig would adopt.
    pub fn parse(&self, input: &[u8]) -> Result<Document_> {
        let src: Arc<[u8]> = Arc::from(input);
        let mut tbl = Table::init(src.clone());
        let mut report = Report::default();
        self.parse_with_report_into(&mut tbl, &src, &mut report)?;
        RowWriter::adopt(src, &mut tbl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::PathSegment;

    fn grammar() -> Document {
        let kv = entry(EntryOpts {
            sep: Some(lit(b"=").any()),
            value: Some(bare(BareOpts { stop: Some(lit(b" #").any()), ..BareOpts::default() }).boxed()),
            ..EntryOpts::new(key(ident(|c| c.is_ascii_alphabetic(), |c| c.is_ascii_alphanumeric())).boxed())
        });
        Document::new(DocOpts::new(
            map(MapOpts {
                whole: true,
                trivia: trivia(TriviaOpts { comment: Some(comment(CommentOpts::new(b"#")).boxed()), ..TriviaOpts::default() })
                    .any(),
                ..MapOpts::new(kv.boxed())
            })
            .boxed(),
        ))
    }

    #[test]
    fn a_grammar_reads_entries_binds_comments_and_keeps_a_repeated_keys_first_place() {
        let doc_rule = grammar();
        let src: Arc<[u8]> = Arc::from(&b"# lead\na=1 # one\nb=2\na=3\n# end\n"[..]);
        let mut t = Table::init(src.clone());
        let mut failure = None;
        doc_rule.parse_into(&mut t, &src, &mut failure).unwrap();
        let doc = RowWriter::adopt(src, &mut t).unwrap();
        let ast = &doc.ast;
        assert_eq!(b"3", ast.get_val_by_path(&[PathSegment::Key(b"a")]).unwrap().kind.as_string().unwrap());
        assert_eq!(7, ast.node_count());
        // The lead comment is the first key's; `# one` trails its value;
        // `# end` dangles on the root.
        let a = ast.get_key_by_path(&[PathSegment::Key(b"a")]).unwrap();
        assert_eq!(b"lead", ast.comments(a.id).leading[0].text);
        assert_eq!(b"end", ast.comments(ast.root).dangling[0].text);

        let bad: Arc<[u8]> = Arc::from(&b"a=1\n=2\n"[..]);
        let mut t2 = Table::init(bad.clone());
        assert_eq!(err!(Refused), doc_rule.parse_into(&mut t2, &bad, &mut failure).unwrap_err());
        let f = failure.unwrap();
        assert_eq!(4, f.at);
        assert_eq!(b"expected an entry here", &f.message[..]);
    }

    #[test]
    fn a_grammar_parser_reports_its_refusal_in_the_table() {
        let p = Parser::new(grammar());
        assert!(p.parse(b"a=1\n").is_ok());
        let mut t = Table::init(&b"a=1\n=2\n"[..]);
        let mut report = Report::default();
        assert!(p.parse_table(&mut t, &mut report).is_err());
        assert_eq!(1, t.diagnostics.len());
        assert_eq!(Some(&b"expected an entry here"[..]), t.text_of(t.diagnostics[0].message));
        assert_eq!(4, report.diag.unwrap().offset);
    }

    #[test]
    fn quoted_values_decode_escapes_and_combinators_backtrack() {
        const ESC: &[Escape] = &[Escape { byte: b'n', means: b"\n" }, Escape { byte: b'"', means: b"\"" }];
        let src: Arc<[u8]> = Arc::from(&b"[\"a\\nb\", \"c\"]"[..]);
        let mut t = Table::init(src.clone());
        let item = quoted(QuotedOpts { escapes: ESC, ..QuotedOpts::new(b"\"") });
        let rule = sequence(SequenceOpts {
            open: Some(lit(b"[").any()),
            close: Some(lit(b"]").any()),
            trivia: many(choice((hs1(), lit(b",")))).any(),
            ..SequenceOpts::new(item.boxed())
        });
        let d = Document::new(DocOpts::new(rule.boxed()));
        let mut failure = None;
        d.parse_into(&mut t, &src, &mut failure).unwrap();
        let doc = RowWriter::adopt(src, &mut t).unwrap();
        let first = doc.ast.node(doc.ast.root).kind.as_sequence().unwrap().unwrap();
        assert_eq!(b"a\nb", doc.ast.node(first).kind.as_string().unwrap());
        let second = doc.ast.node(first).next_sibling.unwrap();
        assert_eq!(b"c", doc.ast.node(second).kind.as_string().unwrap());
        assert_eq!(Span::new(1, 7), doc.ast.span_of(first));
    }
}
