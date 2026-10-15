//! Port of languages/toml/src/parser.zig.
//!
//! The parser turns a TOML-formatted `&[u8]` into the node table fig
//! hands it, a row per node written through a `RowWriter`. A table is
//! reopened by a later header or dotted line, so rows are written in the
//! order the statements meet them, not in pre-order; `RowWriter::finish`
//! puts them in pre-order once the document is done.
//!
//! Strings and datetimes keep their source text where they can; a decoded
//! string (one with escapes) or a canonical number is the parser's own
//! (`Text::Own`), copied into the table's buffer when its row is written.
//!
//! Errors (`ParserError`): the tokenizer's (`tokenizer::TOKENIZE_ERRORS`)
//! and `NotImplemented`, `UnexpectedToken`, `UnclosedString`, `BadEscape`,
//! `InvalidUnicode`, `InvalidNumber`, `UnquotedString`, `InvalidDatetime`,
//! `InvalidKey`, `DuplicateKey`, `TrailingContent`, `InvalidUtf8`.

use std::collections::HashMap;
use std::sync::Arc;

use super::tokenizer::{self, Kind, Token, Tokenizer};
use super::Type;
use crate::kit::ascii;
use crate::kit::ast::{Comment, ExtKind, Extended, KeyValue, NodeId, NodeKind, Number, NumberKind, AST};
use crate::kit::child_index::ChildIndex;
use crate::kit::datetime;
use crate::kit::document::{self, Document, MentionKind};
use crate::kit::parse_diagnostic;
use crate::kit::row_writer::RowWriter;
use crate::kit::span::Span;
use crate::kit::table::{self, Table};
use crate::kit::unicode;
use crate::{err, Error, Result};

/// Every error this parser can produce — both the tokenizer's (which runs to
/// completion before the statement loop starts, so its failures need
/// describing too) and the parser proper.
pub const PARSER_ERRORS: &[&str] = &[
    "NotImplemented",
    "UnexpectedToken",
    "UnclosedString",
    "BadEscape",
    "InvalidUnicode",
    "InvalidNumber",
    "UnquotedString",
    "InvalidDatetime",
    "InvalidKey",
    "DuplicateKey",
    "TrailingContent",
    "InvalidUtf8",
    "UnexpectedCarriageReturn",
    "BadKey",
    "BadValue",
    "BadControlChar",
    "OutOfMemory",
];

/// The teaching message for `code` — one sentence naming the fix, same
/// contract as fig's `describe` (DESIGN.md "every diagnostic names the fix").
pub fn describe(code: Error) -> &'static str {
    match code.name() {
        "NotImplemented" => "this TOML construct is not implemented by this parser",
        "UnexpectedToken" => "unexpected token here; check for a missing `=`, `.`, `,`, or closing `]`/`}`",
        "UnclosedString" => "unclosed string; a single-line string cannot contain a literal newline — close the quote, or use a triple-quoted string (`\"\"\"`/`'''`) for multi-line text",
        "BadEscape" => "invalid escape; basic strings support \\b \\t \\n \\f \\r \\\" \\\\ \\uXXXX \\UXXXXXXXX — use a literal string ('...') for raw text with backslashes",
        "InvalidUnicode" => "invalid unicode escape; the hex digits do not form a valid Unicode codepoint",
        "InvalidNumber" => "not a valid TOML number; if this is text (a version, an id), quote it — TOML has no bare strings. Otherwise check the radix prefix, digit grouping (a single `_` between digits, none leading/trailing), and that there is no leading zero",
        "UnquotedString" => "TOML has no bare strings: a value that is not a number, boolean, date, array, or inline table must be quoted (`\"...\"`, or `'...'` for raw text)",
        "InvalidDatetime" => "not a valid RFC 3339 date/time",
        "InvalidKey" => "invalid key; a bare key allows only letters, digits, `-`, and `_` — quote it for anything else",
        "DuplicateKey" => "this key or table conflicts with one already defined; a TOML key or table may be defined only once",
        "TrailingContent" => "unexpected content after this line's value; each TOML statement must end its line (a `#` comment needs whitespace before it)",
        "InvalidUtf8" => "this file is not valid UTF-8; TOML documents must be UTF-8 encoded",
        "UnexpectedCarriageReturn" => "a bare `\\r` must be followed by `\\n`; TOML line endings are `\\n` or `\\r\\n`",
        "BadKey" => "invalid key; expected a bare key, a quoted key, `.`, or `=` here",
        "BadValue" => "not a recognized value; TOML values are strings, numbers, booleans, datetimes, arrays, or inline tables",
        "BadControlChar" => "control characters are not allowed here (only tab is permitted outside a multi-line string)",
        "OutOfMemory" => "out of memory",
        _ => "",
    }
}

/// A short (few-word) noun phrase for `code` — for a caret annotation
/// (`^ duplicate key/table`), same purpose as fig's `shortLabel`.
pub fn short_label(code: Error) -> &'static str {
    match code.name() {
        "NotImplemented" => "not implemented",
        "UnexpectedToken" => "unexpected token",
        "UnclosedString" => "unclosed string",
        "BadEscape" => "invalid escape",
        "InvalidUnicode" => "invalid unicode escape",
        "InvalidNumber" => "invalid number",
        "UnquotedString" => "needs quotes",
        "InvalidDatetime" => "invalid datetime",
        "InvalidKey" => "invalid key",
        "DuplicateKey" => "duplicate key/table",
        "TrailingContent" => "trailing content",
        "InvalidUtf8" => "invalid UTF-8",
        "UnexpectedCarriageReturn" => "bare CR",
        "BadKey" => "invalid key",
        "BadValue" => "invalid value",
        "BadControlChar" => "control character",
        "OutOfMemory" => "out of memory",
        _ => "",
    }
}

/// A parse failure plus the byte span where it fired. Most sites need no
/// special handling: the statement loop (`parse_once`) anchors on whatever
/// token `pos` rests on when the error propagates back up to it, which
/// is correct for a plain token mismatch (the cursor hasn't advanced past the
/// bad token yet). Sites where the cursor HAS already scanned past the real
/// offender (a duplicate key/table noticed only after the rest of the line
/// parsed) call `fail_at`/`fail_span` to pin the precise span instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Error,
    pub offset: usize,
    pub end: Option<usize>,
}

impl Diagnostic {
    /// 1-based line/column of `offset`, plus the full offending line.
    pub fn locate<'s>(&self, source: &'s [u8]) -> parse_diagnostic::Location<'s> {
        parse_diagnostic::locate_offset(source, self.offset)
    }

    /// Render `file:line:col: error: <message>` + source line + caret.
    pub fn render_alloc(&self, source: &[u8], file: &[u8]) -> Vec<u8> {
        parse_diagnostic::render_report_alloc(source, self.offset, file, b"error", describe(self.code).as_bytes())
    }
}

/// An authoring-time lint: the document is valid TOML, but the shape is a
/// likely mistake. No lint has been identified yet; `WarningCode` is a real
/// (empty) type rather than omitted, so `Report`'s shape matches every other
/// language's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warning {
    pub code: WarningCode,
    pub offset: usize,
    pub end: Option<usize>,
}

/// Uninhabited: nothing ever constructs one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {}

impl Warning {
    pub fn describe_warning(code: WarningCode) -> &'static str {
        match code {}
    }

    pub fn short_label(code: WarningCode) -> &'static str {
        match code {}
    }

    pub fn locate<'s>(&self, source: &'s [u8]) -> parse_diagnostic::Location<'s> {
        parse_diagnostic::locate_offset(source, self.offset)
    }

    pub fn render_alloc(&self, source: &[u8], file: &[u8]) -> Vec<u8> {
        parse_diagnostic::render_report_alloc(source, self.offset, file, b"warning", Self::describe_warning(self.code).as_bytes())
    }
}

/// Everything a parse reports besides the tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub diag: Option<Diagnostic>,
    /// Every parse error, in source order — populated ONLY by the recovering
    /// entry point (`parse_collecting`), which resyncs past each failed
    /// statement and keeps going. The single-shot `parse`/`parse_with_report`
    /// stop at the first error and leave this empty, setting `diag` alone.
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Warning>,
}

/// `input` as a `Document`: its table written and adopted.
pub fn parse(input: &[u8], format: Type) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_into(&mut t, input, format)?;
    RowWriter::adopt(source, &mut t)
}

/// `parse`, reporting as `parse_with_report_into` does.
pub fn parse_with_report(input: &[u8], format: Type, out: &mut Report) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_with_report_into(&mut t, input, format, out)?;
    RowWriter::adopt(source, &mut t)
}

/// `parse`, collecting as `parse_collecting_into` does.
pub fn parse_collecting(input: &[u8], format: Type, out: &mut Report) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_collecting_into(&mut t, input, format, out)?;
    RowWriter::adopt(source, &mut t)
}

/// Write `input`'s rows into `t`, the table fig hands a parse.
pub fn parse_into(t: &mut Table, input: &[u8], format: Type) -> Result<()> {
    parse_impl(t, input, format, None, false)
}

/// `parse_into`, but also fills `out`: `diag` on failure (error code + byte
/// span, for `file:line:col` teaching messages), `warnings` always
/// (currently always empty — see `Warning`).
pub fn parse_with_report_into(t: &mut Table, input: &[u8], format: Type, out: &mut Report) -> Result<()> {
    parse_impl(t, input, format, Some(out), false)
}

/// `parse_with_report_into`, but recovers past each failed top-level statement
/// (resyncing to the next statement boundary — see `resync`) to collect the
/// WHOLE file's diagnostics in one pass (`out.errors`, source order) rather
/// than stopping at the first. On any error the return value is still the
/// first error code and the table is not finished; a clean parse writes it
/// exactly as `parse_with_report_into` would. `out.diag` mirrors `errors[0]`.
pub fn parse_collecting_into(t: &mut Table, input: &[u8], format: Type, out: &mut Report) -> Result<()> {
    parse_impl(t, input, format, Some(out), true)
}

fn parse_impl(t: &mut Table, input: &[u8], format: Type, out: Option<&mut Report>, recover: bool) -> Result<()> {
    // Slices of the table's own source stay in it (`Table::text`); a
    // different input is copied as it is written.
    let source: Arc<[u8]> = if &t.source[..] == input { t.source_arc() } else { Arc::from(input) };
    let mut parser = Parser {
        version: format,
        source,
        tokens: Vec::new(),
        pos: 0,
        w: RowWriter::init(t),
        pending_leading: Vec::new(),
        pending_trailing: Vec::new(),
        last_value_id: None,
        current_table: 0,
        table_meta: HashMap::new(),
        children: ChildIndex::new(),
        recover,
        diagnostics: Vec::new(),
        warnings: Vec::new(),
        fail_offset: None,
        fail_end: None,
    };
    let result = parser.parse_once();
    if let Some(o) = out {
        // Warnings go out on every exit path (valid alongside a failure too).
        o.warnings = parser.warnings.clone();
        if result.is_err() && !parser.diagnostics.is_empty() {
            o.diag = Some(parser.diagnostics[0]);
            if recover {
                o.errors = parser.diagnostics.clone();
            }
        }
    }
    result
}

/// `input` as an `AST`, alone.
pub fn parse_abstract(input: &[u8], format: Type) -> Result<AST> {
    Ok(parse(input, format)?.ast)
}

/// How a table (mapping) node came to exist — determines whether a later header
/// or dotted key may target/extend it.
#[derive(Debug, Clone, Copy, Default)]
struct TableMeta {
    /// Defined by its own `[header]` (or `[[array]]` element). Cannot be
    /// redefined by a header, nor extended by dotted keys from another line.
    explicit: bool,
    /// Created as the value of a dotted-key segment (`a.b = 1` makes `a`).
    dotted: bool,
    /// Created only as an intermediate on a header path (`[a.b.c]` makes
    /// `a`, `a.b`). May still be promoted to `explicit` by its own header.
    #[allow(dead_code)]
    implicit: bool,
    /// A sequence node created by `[[array.of.tables]]`. Only such sequences
    /// may be navigated into (last element) or extended; a static `= [...]`
    /// array has no meta and is therefore closed.
    aot: bool,
    /// An array of tables' last element, so that appending one and descending
    /// into it do not walk the elements before it.
    last: Option<NodeId>,
    /// A mapping from an inline `{ ... }` table — fully defined and closed; no
    /// header or dotted key may extend it.
    inline_table: bool,
}

/// Bytes the parser holds: a range of the source, or its own (a decoded
/// string, a canonical number).
#[derive(Debug, Clone)]
enum Text {
    Src(usize, usize),
    Own(Vec<u8>),
}

impl Text {
    fn bytes<'s>(&'s self, src: &'s [u8]) -> &'s [u8] {
        match self {
            Text::Src(a, b) => &src[*a..*b],
            Text::Own(v) => v,
        }
    }
}

/// A key segment: its text and its token's span.
#[derive(Debug, Clone)]
struct KeySeg {
    str: Text,
    span: Span,
}

/// A same-line comment (its text's range in the source) and the value it
/// trails.
#[derive(Debug, Clone, Copy)]
struct Trailing {
    id: NodeId,
    c: Span,
}

struct Parser<'t> {
    version: Type,
    source: Arc<[u8]>,
    tokens: Vec<Token>,
    pos: usize,
    /// The table being written.
    w: RowWriter<'t>,
    // Comment layer. Comments are captured in `skip_inline`/`skip_blank` (the
    // trivia skippers): a comment on the same line as a just-parsed value
    // (`last_value_id` set) trails it; one on its own line buffers as
    // `pending_leading`, claimed onto the next entry's key in
    // `append_key_value`. A newline resets the trailing window. Text borrows
    // `source` (a span of it); trailing comments reach the table when the
    // document is done.
    pending_leading: Vec<Span>,
    pending_trailing: Vec<Trailing>,
    last_value_id: Option<NodeId>,
    /// The mapping that bare/dotted `key = value` lines attach to. Starts at root,
    /// repointed by each `[table]` header.
    current_table: NodeId,
    /// Per-mapping provenance, used to enforce TOML's table/key conflict rules.
    /// Absent ⇒ a value node or the root.
    table_meta: HashMap<NodeId, TableMeta>,
    /// Each mapping's last entry and its entries by key, so appending to a
    /// table and asking whether it has a key are constant time.
    children: ChildIndex,
    /// Error-recovery mode: when set, a failed top-level statement is recorded
    /// into `diagnostics` and the parser resyncs to the next statement boundary
    /// (see `resync`) instead of aborting.
    recover: bool,
    /// Every parse error hit so far, in source order.
    diagnostics: Vec<Diagnostic>,
    /// Authoring-time lints (none identified yet).
    warnings: Vec<Warning>,
    /// Overrides the next diagnostic's offset when the cursor has already scanned
    /// past the token that actually offends (set via `fail_at`/`fail_span`,
    /// cleared after each statement).
    fail_offset: Option<usize>,
    /// The offending span's end, paired with `fail_offset`.
    fail_end: Option<usize>,
}

impl<'t> Parser<'t> {
    /// One top-level statement (`key = value` / `[table]` / `[[array.of.tables]]`)
    /// plus the line-end check that must follow it.
    fn dispatch_statement(&mut self) -> Result<()> {
        match self.peek().kind {
            Kind::Key => self.parse_key_value()?,
            Kind::OpenBracket => self.parse_table_header()?,
            Kind::DoubleOpenBracket => self.parse_array_table()?,
            _ => return Err(err!(UnexpectedToken)),
        }
        self.require_line_end()
    }

    /// After a parse error while parsing one top-level statement (only reachable
    /// in `recover` mode), scan forward from `start` for the next `Newline`/
    /// `EndOfFile` OUTSIDE any bracket nesting — tracking `[`/`[[`/`{` opens
    /// and `]`/`]]`/`}` closes so a broken multi-line array/inline table's
    /// interior newlines don't count as a safe statement boundary (a stray
    /// unmatched closer just clamps depth at 0). Always finds one
    /// (`EndOfFile` is always the tokenizer's last token). The returned token
    /// is not consumed here.
    fn resync(&self, start: usize) -> usize {
        let mut depth: isize = 0;
        let mut j = start;
        while j < self.tokens.len() {
            match self.tokens[j].kind {
                Kind::OpenBracket | Kind::OpenBrace | Kind::DoubleOpenBracket => depth += 1,
                Kind::CloseBracket | Kind::CloseBrace | Kind::DoubleCloseBracket => {
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                Kind::Newline => {
                    if depth == 0 {
                        return j;
                    }
                }
                Kind::EndOfFile => return j,
                _ => {}
            }
            j += 1;
        }
        self.tokens.len() - 1
    }

    /// `resync`, then re-tokenizes everything from the landing point on and swaps
    /// it in for the tokens, resetting `pos` to the start of the fresh segment:
    /// the tokenizer is context-sensitive, and an unclosed `[`/`{` leaves its
    /// context stuck "inside a collection" for the rest of the file, which a
    /// fresh tokenizer at a real statement boundary does not inherit.
    ///
    /// Returns `false` when the remainder can't even be lexed — recorded as one
    /// final diagnostic, then the caller stops recovering.
    fn resync_and_retokenize(&mut self) -> Result<bool> {
        let sync = self.resync(self.pos);
        let restart_byte = self.tokens[sync].span.start;
        let src = self.source.clone();
        let mut tokenizer = Tokenizer::new(&src[restart_byte..], self.version);
        let mut fresh = match tokenizer.tokenize() {
            Ok(t) => t,
            Err(e) => {
                self.diagnostics.push(Diagnostic { code: e, offset: restart_byte + tokenizer.i, end: None });
                return Ok(false);
            }
        };
        // Spans from `tokenizer` are relative to the subslice — shift them back
        // to absolute.
        for t in fresh.iter_mut() {
            t.span.start += restart_byte;
            t.span.end += restart_byte;
        }
        self.tokens = fresh;
        self.pos = 0;
        Ok(true)
    }

    /// Return `e` with the diagnostic caret pinned to `offset`.
    #[allow(dead_code)]
    fn fail_at(&mut self, offset: usize, e: Error) -> Error {
        self.fail_offset = Some(offset);
        e
    }

    /// Like `fail_at`, but also pins the offending token's end.
    fn fail_span(&mut self, start: usize, end: usize, e: Error) -> Error {
        self.fail_offset = Some(start);
        self.fail_end = Some(end);
        e
    }

    fn parse_once(&mut self) -> Result<()> {
        let input = self.source.clone();

        // A TOML file must be valid UTF-8 (catches all bad-utf8 fixtures at once).
        if std::str::from_utf8(&input).is_err() {
            return Err(err!(InvalidUtf8));
        }

        let mut tokenizer = Tokenizer::new(&input, self.version);
        self.tokens = match tokenizer.tokenize() {
            Ok(t) => t,
            Err(e) => {
                // The tokenizer's cursor sits on (or just after) the offending
                // byte. Never recoverable: a lexical failure happens before the
                // statement loop even starts.
                self.diagnostics.push(Diagnostic { code: e, offset: tokenizer.i, end: None });
                return Err(e);
            }
        };

        // The root mapping's row is written first.
        let root_id = self.add_node(NodeKind::Mapping(None), Span::new(0, input.len()))?;
        self.current_table = root_id;

        self.skip_blank();
        while !self.at_end() {
            if let Err(e) = self.dispatch_statement() {
                if e.is("OutOfMemory") {
                    return Err(e); // never a document defect
                }
                // The cursor rests on the offending token by default;
                // `fail_at`/`fail_span` sites override this, then it's
                // cleared so it can't leak into a LATER error.
                let offset = self.fail_offset.take().unwrap_or(self.peek().span.start);
                let end = self.fail_end.take();
                self.diagnostics.push(Diagnostic { code: e, offset, end });
                if !self.recover {
                    return Err(e);
                }
                if !self.resync_and_retokenize()? {
                    break;
                }
            }
            self.skip_blank();
        }
        // End-of-file orphan comments dangle off the table they sit in (the last one
        // `current_table` points at). Mid-file orphans are instead claimed as the
        // next key's / header's leading.
        self.claim_dangling(self.current_table)?;

        if !self.diagnostics.is_empty() {
            // Recover mode collected 1+ errors: the tree may be malformed, so
            // don't finish it — return the first failure's code.
            return Err(self.diagnostics[0].code);
        }

        // A value's same-line comment, in source order: a later one on the
        // same value replaces an earlier, as `RowWriter::comment` says.
        let trailing = std::mem::take(&mut self.pending_trailing);
        for tc in trailing {
            self.w.comment(tc.id, table::COMMENT_TRAILING, Comment::line(&input[tc.c.start..tc.c.end]))?;
        }
        self.w.finish(root_id)
    }

    fn add_node(&mut self, kind: NodeKind<'_>, span: Span) -> Result<NodeId> {
        self.w.add(kind, span)
    }

    // ── Token cursor ────────────────────────────────────────────────────────

    fn peek(&self) -> Token {
        self.tokens[self.pos]
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens[self.pos];
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn at_end(&self) -> bool {
        self.peek().kind == Kind::EndOfFile
    }

    /// Skip whitespace and comments (but not newlines). A comment here is on the
    /// current line, so it can trail a just-parsed value.
    fn skip_inline(&mut self) {
        loop {
            match self.peek().kind {
                Kind::Whitespace => self.pos += 1,
                Kind::Comment => {
                    self.capture_comment(self.peek());
                    self.pos += 1;
                }
                _ => return,
            }
        }
    }

    /// Skip whitespace, comments, and blank lines (newlines). A newline closes the
    /// trailing-comment window, so comments past it lead the next entry.
    fn skip_blank(&mut self) {
        loop {
            match self.peek().kind {
                Kind::Whitespace => self.pos += 1,
                Kind::Comment => {
                    self.capture_comment(self.peek());
                    self.pos += 1;
                }
                Kind::Newline => {
                    self.last_value_id = None;
                    self.pos += 1;
                }
                _ => return,
            }
        }
    }

    // ── Comments ─────────────────────────────────────────────────────────────

    /// Classify a comment token: trailing the most recent value when its window is
    /// open (`last_value_id` set, no newline since), else buffered as leading.
    fn capture_comment(&mut self, tok: Token) {
        let c = comment_text(&self.source, tok.span);
        if let Some(id) = self.last_value_id {
            self.pending_trailing.push(Trailing { id, c });
            self.last_value_id = None; // one trailing per value
        } else {
            self.pending_leading.push(c);
        }
    }

    /// Hand the buffered leading comments to node `id`, then clear the buffer.
    fn claim_leading(&mut self, id: NodeId) -> Result<()> {
        let src = self.source.clone();
        for c in std::mem::take(&mut self.pending_leading) {
            self.w.comment(id, table::COMMENT_LEADING, Comment::line(&src[c.start..c.end]))?;
        }
        Ok(())
    }

    /// Hand buffered orphan comments (no key/header followed them — at end of file)
    /// to table `id` as its `dangling` run.
    fn claim_dangling(&mut self, id: NodeId) -> Result<()> {
        let src = self.source.clone();
        for c in std::mem::take(&mut self.pending_leading) {
            self.w.comment(id, table::COMMENT_DANGLING, Comment::line(&src[c.start..c.end]))?;
        }
        Ok(())
    }

    /// After a statement, only trivia then a newline or EOF may follow.
    fn require_line_end(&mut self) -> Result<()> {
        self.skip_inline();
        match self.peek().kind {
            Kind::Newline | Kind::EndOfFile => Ok(()),
            _ => Err(err!(TrailingContent)),
        }
    }

    // ── Statements ──────────────────────────────────────────────────────────

    /// Parse `[table.path]` and repoint `current_table`. The path is resolved from
    /// the root; intermediates may pass through any existing table, the final must
    /// be new (→ explicit) or an implicit path-table (→ promoted to explicit).
    fn parse_table_header(&mut self) -> Result<()> {
        self.advance(); // '['
        self.skip_inline();

        let mut segs: Vec<KeySeg> = Vec::new();
        self.parse_key_path(&mut segs)?;
        self.skip_inline();
        if self.peek().kind != Kind::CloseBracket {
            return Err(err!(UnexpectedToken));
        }
        self.advance();
        if segs.is_empty() {
            return Err(err!(InvalidKey));
        }

        let n = segs.len();
        let cur = self.navigate_header_path(0, &segs[..n - 1])?;
        let fin = &segs[n - 1];
        if let Some(child) = self.lookup_child(cur, &fin.str) {
            // The cursor has already consumed the whole `[header]` line by this
            // point — pin the caret to the conflicting final path segment.
            if !matches!(self.w.kind_of(child), NodeKind::Mapping(_)) {
                return Err(self.fail_span(fin.span.start, fin.span.end, err!(DuplicateKey)));
            }
            let m = self.table_meta.get(&child).copied().unwrap_or_default();
            if m.explicit || m.dotted || m.inline_table {
                return Err(self.fail_span(fin.span.start, fin.span.end, err!(DuplicateKey)));
            }
            self.table_meta.insert(child, TableMeta { explicit: true, ..TableMeta::default() });
            self.record_header(child, fin.span.start)?;
            self.record_mention(child, fin.span, MentionKind::Header)?;
            self.current_table = child;
        } else {
            self.current_table = self.create_table(cur, fin, TableMeta { explicit: true, ..TableMeta::default() })?;
            self.record_mention(self.current_table, fin.span, MentionKind::Header)?;
        }
        Ok(())
    }

    /// Parse `[[array.of.tables]]`: navigate the path from root (intermediates like
    /// a header), then create-or-extend the final array-of-tables, appending a fresh
    /// element table that becomes `current_table`.
    fn parse_array_table(&mut self) -> Result<()> {
        self.advance(); // '[['
        self.skip_inline();
        let mut segs: Vec<KeySeg> = Vec::new();
        self.parse_key_path(&mut segs)?;
        self.skip_inline();
        if self.peek().kind != Kind::DoubleCloseBracket {
            return Err(err!(UnexpectedToken));
        }
        self.advance();
        if segs.is_empty() {
            return Err(err!(InvalidKey));
        }

        let n = segs.len();
        let cur = self.navigate_header_path(0, &segs[..n - 1])?;
        let fin = &segs[n - 1];
        if let Some(child) = self.lookup_child(cur, &fin.str) {
            let m = self.table_meta.get(&child).copied().unwrap_or_default();
            // See `parse_table_header`: pin the caret to the conflicting final segment.
            if !matches!(self.w.kind_of(child), NodeKind::Sequence(_)) || !m.aot {
                return Err(self.fail_span(fin.span.start, fin.span.end, err!(DuplicateKey)));
            }
            self.record_header(child, fin.span.start)?;
            self.record_mention(child, fin.span, MentionKind::Header)?;
            self.current_table = self.append_array_element(child)?;
        } else {
            let seq_id = self.add_node(NodeKind::Sequence(None), fin.span)?;
            self.append_key_value(cur, fin, seq_id)?;
            self.table_meta.insert(seq_id, TableMeta { aot: true, ..TableMeta::default() });
            self.record_header(seq_id, fin.span.start)?;
            self.record_mention(seq_id, fin.span, MentionKind::Header)?;
            self.current_table = self.append_array_element(seq_id)?;
        }
        // The new element shares the array's node span, so its `[[…]]` line is
        // recorded on the element too — the only way its header is findable.
        self.record_header(self.current_table, fin.span.start)
    }

    /// Walk a header/array-of-tables path of intermediate segments from `start`,
    /// creating missing tables (implicit) and descending into existing ones. An
    /// array-of-tables intermediate descends into its last element.
    fn navigate_header_path(&mut self, start: NodeId, intermediates: &[KeySeg]) -> Result<NodeId> {
        let mut cur = start;
        for seg in intermediates {
            if let Some(child) = self.lookup_child(cur, &seg.str) {
                // The segment names `child` (an array of tables included — the
                // walk continues into its last element, but the NAME is the
                // array's). A conflict pins the caret to `seg`.
                self.record_mention(child, seg.span, MentionKind::Header)?;
                cur = match self.descend(child) {
                    Ok(c) => c,
                    Err(e) => return Err(self.fail_span(seg.span.start, seg.span.end, e)),
                };
            } else {
                cur = self.create_table(cur, seg, TableMeta { implicit: true, ..TableMeta::default() })?;
                self.record_mention(cur, seg.span, MentionKind::Header)?;
            }
        }
        Ok(cur)
    }

    /// Resolve an existing path node to the mapping to continue from: a plain table
    /// directly, an array-of-tables to its last element. A non-table value, a
    /// static array, or a closed inline table is an error.
    fn descend(&self, child: NodeId) -> Result<NodeId> {
        match self.w.kind_of(child) {
            NodeKind::Mapping(_) => {
                let m = self.table_meta.get(&child).copied().unwrap_or_default();
                if m.inline_table {
                    return Err(err!(DuplicateKey));
                }
                Ok(child)
            }
            NodeKind::Sequence(_) => {
                let m = self.table_meta.get(&child).copied().unwrap_or_default();
                if !m.aot {
                    return Err(err!(DuplicateKey));
                }
                self.last_element(child)
            }
            _ => Err(err!(DuplicateKey)),
        }
    }

    fn last_element(&self, seq_id: NodeId) -> Result<NodeId> {
        let m = self.table_meta.get(&seq_id).copied().unwrap_or_default();
        m.last.ok_or(err!(DuplicateKey))
    }

    fn append_array_element(&mut self, seq_id: NodeId) -> Result<NodeId> {
        let span = self.w.span_of(seq_id);
        let elem = self.add_node(NodeKind::Mapping(None), span)?;
        let last = self.table_meta.get(&seq_id).expect("an array of tables has meta").last;
        match last {
            Some(last) => self.w.set_next(last, Some(elem)),
            None => self.w.set(seq_id, NodeKind::Sequence(Some(elem)))?,
        }
        self.table_meta.get_mut(&seq_id).expect("an array of tables has meta").last = Some(elem);
        Ok(elem)
    }

    /// Parse a `key = value` line (the key possibly dotted), attaching to
    /// `current_table`. Dotted intermediates create/extend dotted tables but may
    /// not descend into an explicitly-defined table (TOML forbids using dotted keys
    /// to append to a `[table]`) nor through a non-table value.
    fn parse_key_value(&mut self) -> Result<()> {
        let mut segs: Vec<KeySeg> = Vec::new();
        self.parse_key_path(&mut segs)?;
        self.skip_inline();
        if self.peek().kind != Kind::Equals {
            return Err(err!(UnexpectedToken));
        }
        self.advance(); // '='
        self.skip_inline();

        let n = segs.len();
        let cur = self.navigate_dotted_path(self.current_table, &segs[..n - 1])?;
        let fin = &segs[n - 1];
        // The cursor already sits on the VALUE token here (past `=`) — pin the
        // caret to the conflicting key segment instead of the value.
        if self.lookup_child(cur, &fin.str).is_some() {
            return Err(self.fail_span(fin.span.start, fin.span.end, err!(DuplicateKey)));
        }
        let value_id = self.parse_value()?;
        // The value is now the trailing-comment candidate for the rest of this line.
        self.last_value_id = Some(value_id);
        self.append_key_value(cur, fin, value_id)
    }

    /// Walk dotted-key intermediates from `start`, creating missing tables (dotted)
    /// and descending into dotted/implicit ones. Descending into an explicitly
    /// defined table, a closed inline table, or a non-table value is an error —
    /// dotted keys may not append to a `[table]`.
    fn navigate_dotted_path(&mut self, start: NodeId, intermediates: &[KeySeg]) -> Result<NodeId> {
        let mut cur = start;
        for seg in intermediates {
            if let Some(child) = self.lookup_child(cur, &seg.str) {
                // The cursor is already past this whole dotted key (and its `=`)
                // — pin the caret to the conflicting intermediate segment itself.
                if !matches!(self.w.kind_of(child), NodeKind::Mapping(_)) {
                    return Err(self.fail_span(seg.span.start, seg.span.end, err!(DuplicateKey)));
                }
                let m = self.table_meta.get(&child).copied().unwrap_or_default();
                if m.explicit || m.inline_table {
                    return Err(self.fail_span(seg.span.start, seg.span.end, err!(DuplicateKey)));
                }
                // A dotted line extending an existing dotted/implicit table is a
                // re-entry of it — this line is in no node's span of that table.
                self.record_header(child, seg.span.start)?;
                self.record_mention(child, seg.span, MentionKind::Entry)?;
                cur = child;
            } else {
                cur = self.create_table(cur, seg, TableMeta { dotted: true, ..TableMeta::default() })?;
                self.record_mention(cur, seg.span, MentionKind::Entry)?;
            }
        }
        Ok(cur)
    }

    /// Parse a dotted key path (`a.b.c`); cursor must be at the first `Key`. Stops
    /// when the next significant token is not a dot.
    fn parse_key_path(&mut self, segs: &mut Vec<KeySeg>) -> Result<()> {
        loop {
            let tok = self.peek();
            if tok.kind != Kind::Key {
                return Err(err!(UnexpectedToken));
            }
            self.advance();
            let str = self.decode_key(tok)?;
            segs.push(KeySeg { str, span: tok.span });
            self.skip_inline();
            if self.peek().kind != Kind::Dot {
                return Ok(());
            }
            self.advance();
            self.skip_inline();
        }
    }

    /// Value node id of a child of `map_id` keyed `key`, or `None`. A key's
    /// text is read from its row as it is compared, never kept.
    fn lookup_child(&self, map_id: NodeId, key: &Text) -> Option<NodeId> {
        let entry = self.children.get(&self.w, map_id, key.bytes(&self.source))?;
        match self.w.kind_of(entry) {
            NodeKind::KeyValue(kv) => Some(kv.value),
            _ => unreachable!("an indexed entry is a keyvalue"),
        }
    }

    /// Append a `key = value` entry to mapping `map_id`.
    fn append_key_value(&mut self, map_id: NodeId, key: &KeySeg, value_id: NodeId) -> Result<()> {
        let src = self.source.clone();
        let key_id = self.add_node(NodeKind::String(key.str.bytes(&src)), key.span)?;
        // A leading comment block above this line (or above a `[header]`, since
        // headers also route through here) binds to the key node.
        self.claim_leading(key_id)?;
        let value_end = self.w.span_of(value_id).end;
        let kv_id = self.add_node(NodeKind::KeyValue(KeyValue { key: key_id, value: value_id }), Span::new(key.span.start, value_end))?;
        self.children.append(&mut self.w, map_id, kv_id)?;
        self.children.put(&self.w, kv_id)
    }

    /// Create an empty child table under `parent` keyed `key`, record its origin,
    /// and return the new mapping node id.
    fn create_table(&mut self, parent: NodeId, key: &KeySeg, meta: TableMeta) -> Result<NodeId> {
        let map_id = self.add_node(NodeKind::Mapping(None), key.span)?;
        self.append_key_value(parent, key, map_id)?;
        self.table_meta.insert(map_id, meta);
        // Explicit, implicit or dotted, the table is a section node: record the
        // line that created it. (Inline tables never come through here.)
        self.record_header(map_id, key.span.start)?;
        Ok(map_id)
    }

    /// Record the header line holding `at` as one of block table `id`'s regions
    /// (`Document::regions_of`): the `[a]`/`[[a]]` line that created or re-opened
    /// it, or — for a dotted table — each `a.b = 1` line that created or
    /// extended it. A block table's node span is only its key segment, so these
    /// lines are what let the editor's region gather find the table's physical
    /// extent.
    fn record_header(&mut self, id: NodeId, at: usize) -> Result<()> {
        let line = document::line_region_at(&self.source, at);
        self.w.region(id, line.start, line.end)
    }

    /// Record that the key token `span` names table `id` (`Document::mentions_of`).
    fn record_mention(&mut self, id: NodeId, span: Span, kind: MentionKind) -> Result<()> {
        self.w.mention(id, span, kind)
    }

    fn parse_value(&mut self) -> Result<NodeId> {
        let tok = self.peek();
        let src = self.source.clone();
        match tok.kind {
            Kind::String => {
                self.advance();
                let decoded = self.decode_string(tok.span.start, tok.span.end)?;
                self.add_node(NodeKind::String(decoded.bytes(&src)), tok.span)
            }
            Kind::Number => {
                self.advance();
                let raw = &src[tok.span.start..tok.span.end];
                let kind = match classify_number(raw) {
                    Ok(k) => k,
                    Err(e) => {
                        // The cursor already moved past the offending token, so pin
                        // its span explicitly. Every bareword that isn't
                        // `true`/`false` tokenizes as a number, so re-label the
                        // ones that never started out number-shaped (they need
                        // quotes, not number grammar).
                        let code = if e.is("InvalidNumber") && !looks_numeric(raw) { err!(UnquotedString) } else { e };
                        return Err(self.fail_span(tok.span.start, tok.span.end, code));
                    }
                };
                // Store the value in canonical, format-independent form (decimal,
                // no underscores) so TOML→JSON/YAML conversion is direct.
                let canon = match kind {
                    NumberKind::Integer => canonical_int(raw, tok.span.start)?,
                    NumberKind::Float => canonical_float(raw, tok.span.start)?,
                };
                self.add_node(NodeKind::Number(Number { raw: canon.bytes(&src), kind }), tok.span)
            }
            Kind::Datetime => {
                self.advance();
                let raw = &src[tok.span.start..tok.span.end];
                let shape = self.classify_datetime(raw)?;
                self.add_node(NodeKind::Extended(Extended { text: raw, kind: shape }), tok.span)
            }
            Kind::Boolean => {
                self.advance();
                self.add_node(NodeKind::Boolean(src[tok.span.start] == b't'), tok.span)
            }
            Kind::OpenBracket => self.parse_array(),
            Kind::OpenBrace => self.parse_inline_table(),
            _ => Err(err!(UnexpectedToken)),
        }
    }

    /// Parse a `[ value, value, ... ]` array. Arrays may span lines and carry a
    /// trailing comma; elements are heterogeneous.
    fn parse_array(&mut self) -> Result<NodeId> {
        let start = self.peek().span.start;
        self.advance(); // '['
        let seq_id = self.add_node(NodeKind::Sequence(None), Span::new(start, start + 1))?;
        let mut last: Option<NodeId> = None;

        loop {
            self.skip_blank_flow();
            if self.peek().kind == Kind::CloseBracket {
                break;
            }
            let elem = self.parse_value()?;
            match last {
                Some(l) => self.w.set_next(l, Some(elem)),
                None => self.w.set(seq_id, NodeKind::Sequence(Some(elem)))?,
            }
            last = Some(elem);
            self.skip_blank_flow();
            match self.peek().kind {
                Kind::Comma => {
                    self.advance();
                }
                Kind::CloseBracket => break,
                _ => return Err(err!(UnexpectedToken)),
            }
        }
        let end = self.peek().span.end;
        self.advance(); // ']'
        self.w.set_span(seq_id, Span::new(start, end));
        Ok(seq_id)
    }

    /// Parse a `{ key = value, ... }` inline table. TOML 1.0 inline tables are
    /// single-line (no newlines inside) and have no trailing comma; 1.1
    /// permits both.
    fn parse_inline_table(&mut self) -> Result<NodeId> {
        let allow_nl = self.version == Type::Toml1_1;
        let start = self.peek().span.start;
        self.advance(); // '{'
        let map_id = self.add_node(NodeKind::Mapping(None), Span::new(start, start + 1))?;
        self.table_meta.insert(map_id, TableMeta { inline_table: true, ..TableMeta::default() });

        self.skip_flow_ws(allow_nl);
        if self.peek().kind != Kind::CloseBrace {
            loop {
                self.parse_inline_entry(map_id)?;
                self.skip_flow_ws(allow_nl);
                match self.peek().kind {
                    Kind::Comma => {
                        self.advance();
                        self.skip_flow_ws(allow_nl);
                        if self.peek().kind == Kind::CloseBrace {
                            if !allow_nl {
                                return Err(err!(UnexpectedToken)); // trailing comma (1.0)
                            }
                            break;
                        }
                    }
                    Kind::CloseBrace => break,
                    _ => return Err(err!(UnexpectedToken)),
                }
            }
        }
        let end = self.peek().span.end;
        if self.peek().kind != Kind::CloseBrace {
            return Err(err!(UnexpectedToken));
        }
        self.advance(); // '}'
        self.w.set_span(map_id, Span::new(start, end));
        Ok(map_id)
    }

    /// One `key = value` (key possibly dotted) inside an inline table.
    fn parse_inline_entry(&mut self, table_id: NodeId) -> Result<()> {
        let allow_nl = self.version == Type::Toml1_1;
        let mut segs: Vec<KeySeg> = Vec::new();
        // Inline-table keys are lexed in value mode, so a bare key may arrive as a
        // Number/Boolean/Datetime token; reinterpret it by position.
        loop {
            let tok = self.peek();
            let key = self.decode_inline_key(tok)?;
            self.advance();
            segs.push(KeySeg { str: key, span: tok.span });
            self.skip_flow_ws(allow_nl);
            if self.peek().kind != Kind::Dot {
                break;
            }
            self.advance();
            self.skip_flow_ws(allow_nl);
        }
        if self.peek().kind != Kind::Equals {
            return Err(err!(UnexpectedToken));
        }
        self.advance();
        self.skip_flow_ws(allow_nl);

        let n = segs.len();
        let cur = self.navigate_dotted_path(table_id, &segs[..n - 1])?;
        let fin = &segs[n - 1];
        // See `parse_key_value`: the cursor is past `=` here — pin the caret to the
        // conflicting key segment instead of the value.
        if self.lookup_child(cur, &fin.str).is_some() {
            return Err(self.fail_span(fin.span.start, fin.span.end, err!(DuplicateKey)));
        }
        let value_id = self.parse_value()?;
        self.append_key_value(cur, fin, value_id)
    }

    fn decode_inline_key(&mut self, tok: Token) -> Result<Text> {
        match tok.kind {
            Kind::String => self.decode_string(tok.span.start, tok.span.end), // quoted key
            Kind::Key => Ok(Text::Src(tok.span.start, tok.span.end)),
            Kind::Number | Kind::Boolean | Kind::Datetime => {
                let text = &self.source[tok.span.start..tok.span.end];
                if text.is_empty() {
                    return Err(err!(InvalidKey));
                }
                if !text.iter().all(|&c| tokenizer::is_bare_key_char(c)) {
                    return Err(err!(InvalidKey));
                }
                Ok(Text::Src(tok.span.start, tok.span.end))
            }
            _ => Err(err!(UnexpectedToken)),
        }
    }

    /// Skip whitespace, comments, and newlines — for inside a multi-line array.
    fn skip_blank_flow(&mut self) {
        self.skip_blank();
    }

    /// Skip inline-table separators: whitespace + comments, plus newlines when the
    /// version permits them inside inline tables (1.1).
    fn skip_flow_ws(&mut self, allow_nl: bool) {
        if allow_nl {
            self.skip_blank();
        } else {
            self.skip_inline();
        }
    }

    // ── Key / string decoding ───────────────────────────────────────────────

    fn decode_key(&mut self, tok: Token) -> Result<Text> {
        let raw = &self.source[tok.span.start..tok.span.end];
        if raw.is_empty() {
            return Err(err!(InvalidKey));
        }
        match raw[0] {
            b'"' | b'\'' => self.decode_string(tok.span.start, tok.span.end),
            _ => Ok(Text::Src(tok.span.start, tok.span.end)), // bare key
        }
    }

    /// Decode any of the four TOML string forms (the source's `start..end`) to
    /// its byte value: a range of the source when no transformation is needed,
    /// else the parser's own bytes.
    fn decode_string(&mut self, start: usize, end: usize) -> Result<Text> {
        let src = self.source.clone();
        let raw = &src[start..end];
        if raw.len() < 2 {
            return Err(err!(UnclosedString));
        }
        let q = raw[0];
        let triple = raw.len() >= 6 && raw[1] == q && raw[2] == q;
        if q == b'\'' {
            // Literal: no escapes.
            if triple {
                let (a, b) = trim_leading_newline(raw, 3, raw.len() - 3);
                return Ok(Text::Src(start + a, start + b));
            }
            return Ok(Text::Src(start + 1, end - 1));
        }
        // Basic (q == '"').
        if triple {
            let (a, b) = trim_leading_newline(raw, 3, raw.len() - 3);
            return self.decode_basic(&raw[a..b], true);
        }
        let inner = &raw[1..raw.len() - 1];
        if !inner.contains(&b'\\') {
            return Ok(Text::Src(start + 1, end - 1));
        }
        self.decode_basic(inner, false)
    }

    fn decode_basic(&self, inner: &[u8], multiline: bool) -> Result<Text> {
        let mut out: Vec<u8> = Vec::new();

        let mut i = 0usize;
        while i < inner.len() {
            let c = inner[i];
            if c != b'\\' {
                out.push(c);
                i += 1;
                continue;
            }
            if i + 1 >= inner.len() {
                return Err(err!(BadEscape));
            }
            let n = inner[i + 1];
            match n {
                b'b' => out.push(0x08),
                b't' => out.push(b'\t'),
                b'n' => out.push(b'\n'),
                b'f' => out.push(0x0c),
                b'r' => out.push(b'\r'),
                b'"' => out.push(b'"'),
                b'\\' => out.push(b'\\'),
                b'u' => i = append_unicode(&mut out, inner, i + 2, 4)? - 2,
                b'U' => i = append_unicode(&mut out, inner, i + 2, 8)? - 2,
                // TOML 1.1: \e is ESC (U+001B); \xHH is shorthand for \u00HH.
                b'e' => {
                    if self.version == Type::Toml1_0 {
                        return Err(err!(BadEscape));
                    }
                    out.push(0x1b);
                }
                b'x' => {
                    if self.version == Type::Toml1_0 {
                        return Err(err!(BadEscape));
                    }
                    i = append_unicode(&mut out, inner, i + 2, 2)? - 2;
                }
                b' ' | b'\t' | b'\n' | b'\r' => {
                    if !multiline {
                        return Err(err!(BadEscape));
                    }
                    // Line-ending backslash: `\` + optional whitespace + newline
                    // trims all following whitespace up to the next content.
                    let mut j = i + 1;
                    while j < inner.len() && (inner[j] == b' ' || inner[j] == b'\t') {
                        j += 1;
                    }
                    if j >= inner.len() || (inner[j] != b'\n' && inner[j] != b'\r') {
                        return Err(err!(BadEscape));
                    }
                    while j < inner.len() && (inner[j] == b' ' || inner[j] == b'\t' || inner[j] == b'\n' || inner[j] == b'\r') {
                        j += 1;
                    }
                    i = j;
                    continue;
                }
                _ => return Err(err!(BadEscape)),
            }
            i += 2;
        }
        Ok(Text::Own(out))
    }

    // ── Datetime validation / classification ────────────────────────────────

    /// Validate + classify a datetime token via the shared `datetime` helper.
    /// TOML accepts a bare time and (in 1.1) minute precision; seconds are
    /// mandatory in 1.0.
    fn classify_datetime(&self, raw: &[u8]) -> Result<ExtKind> {
        let kind = datetime::classify(
            raw,
            datetime::Options { allow_minute_precision: self.version != Type::Toml1_0, ..datetime::Options::default() },
        )
        .map_err(|_| err!(InvalidDatetime))?;
        Ok(match kind {
            datetime::Kind::OffsetDatetime => ExtKind::OffsetDatetime,
            datetime::Kind::LocalDatetime => ExtKind::LocalDatetime,
            datetime::Kind::LocalDate => ExtKind::LocalDate,
            datetime::Kind::LocalTime => ExtKind::LocalTime,
        })
    }
}

/// Strip the leading `#` and surrounding spaces from a comment token's bytes:
/// the range of the text left.
fn comment_text(src: &[u8], tok: Span) -> Span {
    let raw = &src[tok.start..tok.end];
    let mut a = 0usize;
    if !raw.is_empty() && raw[0] == b'#' {
        a = 1;
    }
    let mut b = raw.len();
    let trim = |c: u8| c == b' ' || c == b'\t' || c == b'\r';
    while a < b && trim(raw[a]) {
        a += 1;
    }
    while b > a && trim(raw[b - 1]) {
        b -= 1;
    }
    Span::new(tok.start + a, tok.start + b)
}

/// A multi-line string with a newline immediately after the opening delimiter
/// drops that first newline. `raw[a..b]` is the inner text; the range left.
fn trim_leading_newline(raw: &[u8], a: usize, b: usize) -> (usize, usize) {
    let inner = &raw[a..b];
    if !inner.is_empty() && inner[0] == b'\n' {
        return (a + 1, b);
    }
    if inner.len() >= 2 && inner[0] == b'\r' && inner[1] == b'\n' {
        return (a + 2, b);
    }
    (a, b)
}

/// Decode `n` hex digits at `inner[at..]` into a UTF-8 codepoint appended to
/// `out`; returns the index just past the digits.
fn append_unicode(out: &mut Vec<u8>, inner: &[u8], at: usize, n: usize) -> Result<usize> {
    if at + n > inner.len() {
        return Err(err!(BadEscape));
    }
    // Zig's `parseInt(u21, …, 16)`.
    let cp = zig_parse_int(&inner[at..at + n], 16, 0, (1 << 21) - 1).ok_or(err!(InvalidUnicode))?;
    unicode::encode_append(out, cp as u32).map_err(|_| err!(InvalidUnicode))?;
    Ok(at + n)
}

/// Zig's `std.fmt.parseInt` into an integer type whose range is
/// `min..=max`: an optional sign, a radix prefix when `base` is 0, `_`
/// between digits; `None` for a bad character or an overflow.
fn zig_parse_int(buf: &[u8], base: u8, min: i128, max: i128) -> Option<i128> {
    if buf.is_empty() {
        return None;
    }
    let (neg, buf) = match buf[0] {
        b'+' => (false, &buf[1..]),
        b'-' => (true, &buf[1..]),
        _ => (false, buf),
    };
    if buf.is_empty() {
        return None;
    }
    let mut b = base;
    let mut s = buf;
    if base == 0 {
        b = 10;
        if s.len() > 2 && s[0] == b'0' {
            match s[1].to_ascii_lowercase() {
                b'b' => {
                    b = 2;
                    s = &s[2..];
                }
                b'o' => {
                    b = 8;
                    s = &s[2..];
                }
                b'x' => {
                    b = 16;
                    s = &s[2..];
                }
                _ => {}
            }
        }
    }
    if s[0] == b'_' || s[s.len() - 1] == b'_' {
        return None;
    }
    let in_range = |v: i128| if v < min || v > max { None } else { Some(v) };
    let mut acc: i128 = 0;
    for &c in s {
        if c == b'_' {
            continue;
        }
        let digit = match c {
            b'0'..=b'9' => c - b'0',
            b'A'..=b'Z' => c - b'A' + 10,
            b'a'..=b'z' => c - b'a' + 10,
            _ => return None,
        };
        if digit >= b {
            return None;
        }
        if acc != 0 {
            acc = in_range(acc * b as i128)?;
        } else if neg {
            // The first digit of a negative number.
            acc = in_range(-(digit as i128))?;
            continue;
        }
        acc = in_range(if neg { acc - digit as i128 } else { acc + digit as i128 })?;
    }
    Some(acc)
}

// ── Number validation / classification ──────────────────────────────────────

fn eq_any(s: &[u8], list: &[&[u8]]) -> bool {
    list.iter().any(|x| *x == s)
}

/// Whether `raw` was plausibly *meant* as a number: it starts with a digit
/// (after an optional sign), or is one of TOML's spelled-out floats. Used only
/// to pick which diagnostic a failed `classify_number` deserves — `1.2.3` and
/// `0x1g` are botched numbers, `cc5e7e51` and `hello` are unquoted strings.
fn looks_numeric(raw: &[u8]) -> bool {
    let mut body = raw;
    if !body.is_empty() && (body[0] == b'+' || body[0] == b'-') {
        body = &body[1..];
    }
    if body.is_empty() {
        return false;
    }
    if eq_any(body, &[b"inf", b"nan"]) {
        return true;
    }
    ascii::is_digit(body[0])
}

fn classify_number(raw: &[u8]) -> Result<NumberKind> {
    if raw.is_empty() {
        return Err(err!(InvalidNumber));
    }

    // Special floats.
    if eq_any(raw, &[b"inf", b"+inf", b"-inf", b"nan", b"+nan", b"-nan"]) {
        return Ok(NumberKind::Float);
    }

    // Radix-prefixed integers (no sign permitted).
    if raw.len() >= 2 && raw[0] == b'0' {
        let pred: Option<fn(u8) -> bool> = match raw[1] {
            b'x' => Some(ascii::is_hex),
            b'o' => Some(ascii::is_octal),
            b'b' => Some(ascii::is_binary),
            _ => None,
        };
        if let Some(p) = pred {
            return if valid_underscored(&raw[2..], p) { Ok(NumberKind::Integer) } else { Err(err!(InvalidNumber)) };
        }
    }

    let mut body = raw;
    if body[0] == b'+' || body[0] == b'-' {
        body = &body[1..];
    }
    if body.is_empty() {
        return Err(err!(InvalidNumber));
    }

    // Split mantissa / exponent.
    let mut mantissa = body;
    let mut exponent: Option<&[u8]> = None;
    if let Some(e) = body.iter().position(|&c| c == b'e' || c == b'E') {
        mantissa = &body[..e];
        exponent = Some(&body[e + 1..]);
    }

    // Mantissa: int part, optional `.fraction`.
    let mut int_part = mantissa;
    let mut frac_part: Option<&[u8]> = None;
    if let Some(d) = mantissa.iter().position(|&c| c == b'.') {
        int_part = &mantissa[..d];
        frac_part = Some(&mantissa[d + 1..]);
    }

    if !valid_decimal_int(int_part) {
        return Err(err!(InvalidNumber));
    }
    let mut is_float = false;
    if let Some(f) = frac_part {
        if !valid_underscored(f, ascii::is_digit) {
            return Err(err!(InvalidNumber));
        }
        is_float = true;
    }
    if let Some(e) = exponent {
        let mut exp = e;
        if !exp.is_empty() && (exp[0] == b'+' || exp[0] == b'-') {
            exp = &exp[1..];
        }
        if !valid_underscored(exp, ascii::is_digit) {
            return Err(err!(InvalidNumber));
        }
        is_float = true;
    }
    Ok(if is_float { NumberKind::Float } else { NumberKind::Integer })
}

/// A decimal integer literal: `0`, or a non-zero-leading run of digits. Used for
/// the standalone integer and for a float's integer part (leading zeros banned
/// in both: `01` and `03.14` are invalid).
fn valid_decimal_int(s: &[u8]) -> bool {
    if !valid_underscored(s, ascii::is_digit) {
        return false;
    }
    if s.len() > 1 && s[0] == b'0' {
        return false; // no leading zeros
    }
    true
}

/// Non-empty, all chars satisfy `pred` or are `_`, and every `_` sits between
/// two `pred` digits (no leading/trailing/doubled underscore).
fn valid_underscored(s: &[u8], pred: fn(u8) -> bool) -> bool {
    if s.is_empty() {
        return false;
    }
    if s[0] == b'_' || s[s.len() - 1] == b'_' {
        return false;
    }
    let mut prev_us = false;
    for &c in s {
        if c == b'_' {
            if prev_us {
                return false;
            }
            prev_us = true;
        } else if pred(c) {
            prev_us = false;
        } else {
            return false;
        }
    }
    true
}

/// Canonicalize an integer literal (any radix, underscores, sign) to a decimal
/// string. Returns `raw` (at `at` in the source) unchanged when already
/// canonical, else the parser's own copy.
fn canonical_int(raw: &[u8], at: usize) -> Result<Text> {
    let mut buf: Vec<u8> = Vec::with_capacity(80);
    for &c in raw {
        if c == b'_' {
            continue;
        }
        if buf.len() >= 80 {
            return Err(err!(InvalidNumber));
        }
        buf.push(c);
    }
    let v = zig_parse_int(&buf, 0, i64::MIN as i128, i64::MAX as i128).ok_or(err!(InvalidNumber))?;
    let s = v.to_string().into_bytes();
    if s == raw {
        return Ok(Text::Src(at, at + raw.len()));
    }
    Ok(Text::Own(s))
}

/// Canonicalize a float literal: special values to `inf`/`-inf`/`nan`, and strip
/// digit-group underscores (the remaining decimal/exponent form is valid JSON).
fn canonical_float(raw: &[u8], at: usize) -> Result<Text> {
    if eq_any(raw, &[b"inf", b"+inf"]) {
        return Ok(Text::Own(b"inf".to_vec()));
    }
    if raw == b"-inf" {
        return Ok(Text::Own(b"-inf".to_vec()));
    }
    if eq_any(raw, &[b"nan", b"+nan", b"-nan"]) {
        return Ok(Text::Own(b"nan".to_vec()));
    }
    if !raw.contains(&b'_') {
        return Ok(Text::Src(at, at + raw.len()));
    }
    let mut buf: Vec<u8> = Vec::with_capacity(80);
    for &c in raw {
        if c == b'_' {
            continue;
        }
        if buf.len() >= 80 {
            return Err(err!(InvalidNumber));
        }
        buf.push(c);
    }
    Ok(Text::Own(buf))
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::PathSegment;

    fn key(k: &str) -> PathSegment<'_> {
        PathSegment::Key(k.as_bytes())
    }

    #[test]
    fn parses_empty_comment_only_documents() {
        for input in ["", "# c\n", "\n\n  \n"] {
            let doc = parse(input.as_bytes(), Type::Toml1_0).unwrap();
            assert!(matches!(doc.ast.node(doc.ast.root).kind, NodeKind::Mapping(_)));
        }
    }

    #[test]
    fn parses_scalar_key_value_pairs() {
        let doc = parse(
            b"name = \"Tom\"\ncount = 42\npi = 3.14\nhex = 0xDEAD_beef\nflag = true\nwhen = 1979-05-27T07:32:00Z\n",
            Type::Toml1_0,
        )
        .unwrap();
        let ast = &doc.ast;
        assert_eq!(b"Tom", ast.get_val_by_path(&[key("name")]).unwrap().kind.as_string().unwrap());
        assert_eq!(NumberKind::Integer, ast.get_val_by_path(&[key("count")]).unwrap().kind.as_number().unwrap().kind);
        assert_eq!(NumberKind::Float, ast.get_val_by_path(&[key("pi")]).unwrap().kind.as_number().unwrap().kind);
        let NodeKind::Extended(e) = ast.get_val_by_path(&[key("when")]).unwrap().kind else { panic!() };
        assert_eq!(ExtKind::OffsetDatetime, e.kind);
    }

    #[test]
    fn datetime_shapes() {
        let cases: [(&str, ExtKind); 6] = [
            ("1979-05-27T07:32:00Z", ExtKind::OffsetDatetime),
            ("1979-05-27T07:32:00", ExtKind::LocalDatetime),
            ("1979-05-27 07:32:00", ExtKind::LocalDatetime),
            ("1979-05-27", ExtKind::LocalDate),
            ("07:32:00", ExtKind::LocalTime),
            ("00:32:00.999999", ExtKind::LocalTime),
        ];
        for (src, shape) in cases {
            let src = format!("d = {src}\n");
            let doc = parse(src.as_bytes(), Type::Toml1_0).unwrap();
            let NodeKind::Extended(e) = doc.ast.get_val_by_path(&[key("d")]).unwrap().kind else { panic!() };
            assert_eq!(shape, e.kind);
        }
    }

    #[test]
    fn rejects_bad_scalars() {
        let bad = [
            "x = 01\n",         // leading zero
            "x = 1__2\n",       // doubled underscore
            "x = 0x\n",         // empty hex
            "x = 1979-13-01\n", // month over
            "x = 2021-02-29\n", // not a leap year
            "x = 00:00:61\n",   // second over
            "x = \"a\nb\"\n",   // newline in single-line string
            "x = \"\\q\"\n",    // bad escape
            "a = 1 b = 2\n",    // trailing content
        ];
        for input in bad {
            assert!(parse(input.as_bytes(), Type::Toml1_0).is_err(), "expected rejection: {input}");
        }
    }

    #[test]
    fn rejects_duplicate_root_keys() {
        assert_eq!(Some(err!(DuplicateKey)), parse(b"a = 1\na = 2\n", Type::Toml1_0).err());
    }

    #[test]
    fn tables_and_dotted_keys_build_nested_mappings() {
        let doc = parse(b"[server.tcp]\nport = 80\nopts.timeout = 30\n\n[server]\nname = \"main\"\n", Type::Toml1_0).unwrap();
        let ast = &doc.ast;
        let port = ast.get_val_by_path(&[key("server"), key("tcp"), key("port")]).unwrap();
        assert_eq!(NumberKind::Integer, port.kind.as_number().unwrap().kind);
        let timeout = ast.get_val_by_path(&[key("server"), key("tcp"), key("opts"), key("timeout")]).unwrap();
        assert_eq!(b"30", timeout.kind.as_number().unwrap().raw);
        let name = ast.get_val_by_path(&[key("server"), key("name")]).unwrap();
        assert_eq!(b"main", name.kind.as_string().unwrap());
    }

    #[test]
    fn implicit_table_promoted_to_explicit_is_allowed() {
        parse(b"[a.b.c]\nx = 1\n\n[a]\ny = 2\n", Type::Toml1_0).unwrap();
    }

    #[test]
    fn arrays_inline_tables_and_arrays_of_tables() {
        let doc = parse(
            b"nums = [1, 2, 3]\nnested = [[1, 2], [\"a\", \"b\"]]\npoint = { x = 1, y = 2 }\n\n[[fruit]]\nname = \"apple\"\n\n[[fruit]]\nname = \"pear\"\n",
            Type::Toml1_0,
        )
        .unwrap();
        let ast = &doc.ast;
        assert_eq!(b"2", ast.get_val_by_path(&[key("nums"), PathSegment::Index(1)]).unwrap().kind.as_number().unwrap().raw);
        let inner = ast.get_val_by_path(&[key("nested"), PathSegment::Index(1), PathSegment::Index(0)]).unwrap();
        assert_eq!(b"a", inner.kind.as_string().unwrap());
        assert_eq!(b"2", ast.get_val_by_path(&[key("point"), key("y")]).unwrap().kind.as_number().unwrap().raw);
        let pear = ast.get_val_by_path(&[key("fruit"), PathSegment::Index(1), key("name")]).unwrap();
        assert_eq!(b"pear", pear.kind.as_string().unwrap());
    }

    #[test]
    fn inline_table_dotted_keys() {
        let doc = parse(b"a = { b.c = 1, b.d = 2 }\n", Type::Toml1_0).unwrap();
        let c = doc.ast.get_val_by_path(&[key("a"), key("b"), key("c")]).unwrap();
        assert_eq!(b"1", c.kind.as_number().unwrap().raw);
    }

    #[test]
    fn rejects_inline_table_and_array_errors() {
        let bad = [
            "a = { b = 1, }\n",         // trailing comma (1.0)
            "a = { b = 1\n c = 2 }\n",  // newline inside inline table (1.0)
            "a = {b=1}\n[a.c]\nx=1\n",  // extend a closed inline table
            "a = [1, 2\n",              // unclosed array
            "a = { b = 1, b = 2 }\n",   // duplicate inline key
        ];
        for input in bad {
            assert!(parse(input.as_bytes(), Type::Toml1_0).is_err(), "expected rejection: {input}");
        }
    }

    #[test]
    fn toml_1_1_features_optional_seconds_e_x_escapes_inline_table_newlines() {
        let doc = parse(b"t = 13:37\nesc = \"\\e\\x41\"\ntbl = {\n  a = 1,\n  b = 2,\n}\n", Type::Toml1_1).unwrap();
        let ast = &doc.ast;
        let NodeKind::Extended(t) = ast.get_val_by_path(&[key("t")]).unwrap().kind else { panic!() };
        assert_eq!(ExtKind::LocalTime, t.kind);
        assert_eq!(b"\x1bA", ast.get_val_by_path(&[key("esc")]).unwrap().kind.as_string().unwrap());
        assert_eq!(b"2", ast.get_val_by_path(&[key("tbl"), key("b")]).unwrap().kind.as_number().unwrap().raw);
    }

    #[test]
    fn only_1_1_constructs_are_rejected_under_1_0() {
        let only_1_1 = [
            "t = 13:37\n",              // optional seconds
            "s = \"\\e\"\n",            // \e escape
            "s = \"\\x41\"\n",          // \x escape
            "t = { a = 1,\n b = 2 }\n", // newline in inline table
            "t = { a = 1, }\n",         // trailing comma
        ];
        for input in only_1_1 {
            parse(input.as_bytes(), Type::Toml1_1).unwrap();
            assert!(parse(input.as_bytes(), Type::Toml1_0).is_err(), "expected 1.0 rejection: {input}");
        }
    }

    #[test]
    fn rejects_table_key_conflicts() {
        let bad = [
            "[a]\nb = 1\n\n[a.b]\nc = 2\n",          // value used as table
            "[a]\n\n[a]\n",                          // duplicate table
            "a.b = 1\na.b.c = 2\n",                  // dotted key through a value
            "[a.b.c]\nz = 9\n\n[a]\nb.c.t = 1\n",    // dotted-append to explicit table
        ];
        for input in bad {
            assert!(parse(input.as_bytes(), Type::Toml1_0).is_err(), "expected rejection: {input}");
        }
    }

    // ── diagnostics ──────────────────────────────────────────────────────────

    #[test]
    fn parse_with_report_anchors_the_diagnostic_on_the_offending_token() {
        let mut report = Report::default();
        // `a = ` — nothing follows the `=`; the newline is the unexpected token.
        let src = b"a = \nb = 1\n";
        assert_eq!(Some(err!(UnexpectedToken)), parse_with_report(src, Type::Toml1_0, &mut report).err());
        let d = report.diag.unwrap();
        assert_eq!(err!(UnexpectedToken), d.code);
        assert_eq!(4, d.offset); // the newline right after `a = `
        assert_eq!(0, report.errors.len()); // single-shot: no recovery
    }

    #[test]
    fn parse_with_report_pins_a_duplicate_key_diagnostic_on_the_key_not_the_value() {
        let mut report = Report::default();
        let src = b"a = 1\na = 2\n";
        assert_eq!(Some(err!(DuplicateKey)), parse_with_report(src, Type::Toml1_0, &mut report).err());
        let d = report.diag.unwrap();
        assert_eq!(err!(DuplicateKey), d.code);
        assert_eq!(6, d.offset); // the second `a`, not `2`
    }

    #[test]
    fn parse_with_report_reports_the_tokenizers_own_failures_with_a_location() {
        let mut report = Report::default();
        let src = b"a = \"unterminated\nb = 1\n";
        assert_eq!(Some(err!(UnclosedString)), parse_with_report(src, Type::Toml1_0, &mut report).err());
        assert_eq!(err!(UnclosedString), report.diag.unwrap().code);
    }

    #[test]
    fn parse_collecting_reports_every_broken_statement_in_one_pass() {
        let mut report = Report::default();
        // Three independent broken lines interleaved with good ones.
        let src = b"a = \nb = 2\nc = \nd = 4\ne = \nf = 6\n";
        assert_eq!(Some(err!(UnexpectedToken)), parse_collecting(src, Type::Toml1_0, &mut report).err());
        assert_eq!(3, report.errors.len());
        let mut last_offset = 0;
        for d in &report.errors {
            assert_eq!(err!(UnexpectedToken), d.code);
            assert!(d.offset >= last_offset);
            last_offset = d.offset;
        }
    }

    #[test]
    fn parse_collecting_resyncs_past_a_broken_multi_line_array_without_losing_later_statements() {
        let mut report = Report::default();
        let src = b"bad = [1, 2\ngood = 42\n";
        assert_eq!(Some(err!(UnexpectedToken)), parse_collecting(src, Type::Toml1_0, &mut report).err());
        assert!(!report.errors.is_empty());
    }

    #[test]
    fn an_unclosed_array_doesnt_cascade_into_false_errors_on_every_later_well_formed_statement() {
        let mut report = Report::default();
        let src = b"bad = [1, 2\nc = 3\nd = 4\ne = 5\n";
        assert_eq!(Some(err!(UnexpectedToken)), parse_collecting(src, Type::Toml1_0, &mut report).err());
        assert_eq!(1, report.errors.len());
    }

    #[test]
    fn parse_collecting_still_finds_a_second_genuinely_separate_error_after_an_unclosed_array() {
        let mut report = Report::default();
        let src = b"bad = [1, 2\ngood = 42\nbad2 = \nalsogood = 99\n";
        assert_eq!(Some(err!(UnexpectedToken)), parse_collecting(src, Type::Toml1_0, &mut report).err());
        assert_eq!(2, report.errors.len());
    }

    #[test]
    fn an_unquoted_text_value_is_reported_as_needing_quotes_not_as_a_bad_number() {
        for src in [
            "rev = cc5e7e518c1d95e5c1288418f072bd7e351ef62d\n",
            "name = hello\n",
            "branch = -main\n",
            "arr = [ ok ]\n",
            "t = { k = word }\n",
        ] {
            assert_eq!(Some(err!(UnquotedString)), parse(src.as_bytes(), Type::Toml1_0).err(), "{src}");
        }
        for src in ["v = 1.2.3\n", "h = 0x1g\n", "z = 01\n"] {
            assert_eq!(Some(err!(InvalidNumber)), parse(src.as_bytes(), Type::Toml1_0).err(), "{src}");
        }
    }

    #[test]
    fn describe_and_short_label_cover_every_error_variant() {
        for name in PARSER_ERRORS {
            let e = Error(name);
            assert!(!describe(e).is_empty(), "{name}");
            assert!(!short_label(e).is_empty(), "{name}");
        }
    }

    #[test]
    fn an_array_of_tables_appends_after_its_last_element_and_a_header_descends_into_it() {
        let doc = parse(b"[[a]]\nn = 1\n[[a]]\nn = 2\n[a.sub]\nx = 1\n[[a.list]]\ny = 1\n[[a]]\nn = 3\n", Type::Toml1_0).unwrap();
        let ast = &doc.ast;
        for (i, want) in [b"1", b"2", b"3"].iter().enumerate() {
            let n = ast.get_val_by_path(&[key("a"), PathSegment::Index(i), key("n")]).unwrap();
            assert_eq!(&want[..], n.kind.as_number().unwrap().raw);
        }
        ast.get_val_by_path(&[key("a"), PathSegment::Index(1), key("sub"), key("x")]).unwrap();
        ast.get_val_by_path(&[key("a"), PathSegment::Index(1), key("list"), PathSegment::Index(0), key("y")]).unwrap();
        assert_eq!(Some(err!(NotFound)), ast.get_val_by_path(&[key("a"), PathSegment::Index(3)]).err());
    }
}
