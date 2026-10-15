//! Port of languages/dotenv/src/parser.zig.
//!
//! The parser turns a dotenv-formatted `&[u8]` into the node table fig hands
//! it, a row per node (`RowWriter`, through `kit/flat_map.rs`).
//!
//! Shape: a single flat root mapping, `KEY = value` per entry — dotenv has no
//! sections (unlike INI) and no dotted/nested keys. Every value is a string
//! node: dotenv has no typed scalars either, and — deliberately — no
//! `$VAR`/`${VAR}` interpolation is performed; a value is stored exactly as
//! written (fig is a reader, not a shell). An optional leading `export `
//! (bash-export style) is recognized and discarded: see `parse_key_value`.
//!
//! A repeated key silently keeps the LAST value at the FIRST-seen position
//! (see `kit/flat_map.rs`'s `put_entry`, `Overwrite`) and raises a
//! `duplicate_key` warning, exactly like INI/`.properties` — real,
//! parseable content, but likely an authoring mistake.

use std::borrow::Cow;
use std::sync::Arc;

use super::tokenizer::{Kind, Token, Tokenizer};
use super::Type;
use crate::err;
use crate::kit::ast::{Comment, NodeId, NodeKind, Style, AST};
use crate::kit::document::Document;
use crate::kit::flat_map::{self, DuplicatePolicy, NodeArena, PutResult};
use crate::kit::parse_diagnostic;
use crate::kit::row_writer::RowWriter;
use crate::kit::span::Span;
use crate::kit::table::{self, Table};
use crate::{Error, Result};

/// `ParseError`: the names the parser itself raises.
pub const PARSE_ERRORS: &[&str] = &["UnexpectedToken", "MissingEquals", "BadEscape", "DuplicateKey", "InvalidUtf8"];

/// A long, teaching message for `code`.
pub fn describe(code: Error) -> &'static str {
    match code.name() {
        "UnexpectedToken" => "unexpected content here; expected `KEY=value` (optionally `export KEY=value`)",
        "MissingEquals" => "expected `=` after this key; every dotenv line is `KEY=value`",
        "BadEscape" => "invalid escape in a double-quoted value; supported: \\n \\t \\r \\\\ \\\" — use a single-quoted value for raw text with backslashes",
        "DuplicateKey" => "this key conflicts with one already defined",
        "InvalidUtf8" => "this file is not valid UTF-8; dotenv documents must be UTF-8 encoded",
        "UnexpectedCarriageReturn" => "a bare `\\r` must be followed by `\\n`; line endings must be `\\n` or `\\r\\n`",
        "UnclosedString" => "unclosed quoted value; expected a matching `\"`/`'` before the end of the file",
        "UnexpectedChar" => "not a valid key here; a dotenv key is a bash identifier (`[A-Za-z_][A-Za-z0-9_]*`)",
        "TrailingContent" => "unexpected content after this quoted value; only a `#` comment may follow it on the same line",
        "OutOfMemory" => "out of memory",
        other => unreachable!("not a dotenv parse error: {other}"),
    }
}

/// A few words for the caret's label.
pub fn short_label(code: Error) -> &'static str {
    match code.name() {
        "UnexpectedToken" => "unexpected content",
        "MissingEquals" => "missing `=`",
        "BadEscape" => "invalid escape",
        "DuplicateKey" => "duplicate key",
        "InvalidUtf8" => "invalid UTF-8",
        "UnexpectedCarriageReturn" => "bare CR",
        "UnclosedString" => "unclosed string",
        "UnexpectedChar" => "invalid key",
        "TrailingContent" => "trailing content",
        "OutOfMemory" => "out of memory",
        other => unreachable!("not a dotenv parse error: {other}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Error,
    pub offset: usize,
    pub end: Option<usize>,
}

impl Diagnostic {
    pub fn locate<'a>(&self, source: &'a [u8]) -> parse_diagnostic::Location<'a> {
        parse_diagnostic::locate_offset(source, self.offset)
    }
    pub fn render_alloc(&self, source: &[u8], file: &[u8]) -> Vec<u8> {
        parse_diagnostic::render_report_alloc(source, self.offset, file, b"error", describe(self.code).as_bytes())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warning {
    pub code: WarningCode,
    pub offset: usize,
    pub end: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    DuplicateKey,
}

impl Warning {
    pub fn describe_warning(code: WarningCode) -> &'static str {
        match code {
            WarningCode::DuplicateKey => {
                "this key is defined more than once; the last value wins and earlier ones are silently discarded"
            }
        }
    }
    pub fn short_label(code: WarningCode) -> &'static str {
        match code {
            WarningCode::DuplicateKey => "duplicate key",
        }
    }
    pub fn locate<'a>(&self, source: &'a [u8]) -> parse_diagnostic::Location<'a> {
        parse_diagnostic::locate_offset(source, self.offset)
    }
    pub fn render_alloc(&self, source: &[u8], file: &[u8]) -> Vec<u8> {
        parse_diagnostic::render_report_alloc(source, self.offset, file, b"warning", Self::describe_warning(self.code).as_bytes())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub diag: Option<Diagnostic>,
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

/// Write `input`'s rows into `t`, the table fig hands a parse (a table over
/// `input`: its texts are sliced from `t`'s own source).
pub fn parse_into(t: &mut Table, input: &[u8], format: Type) -> Result<()> {
    parse_impl(t, input, format, None, false)
}

/// `parse_into`, but also fills `out`: `diag` on failure, `warnings` always.
pub fn parse_with_report_into(t: &mut Table, input: &[u8], format: Type, out: &mut Report) -> Result<()> {
    parse_impl(t, input, format, Some(out), false)
}

/// `parse_with_report_into`, but recovers past each parser-level error to
/// collect every diagnostic (`out.errors`). On an error the table is not
/// finished.
pub fn parse_collecting_into(t: &mut Table, input: &[u8], format: Type, out: &mut Report) -> Result<()> {
    parse_impl(t, input, format, Some(out), true)
}

fn parse_impl(t: &mut Table, input: &[u8], format: Type, out: Option<&mut Report>, recover: bool) -> Result<()> {
    let src = t.source_arc();
    debug_assert_eq!(&src[..], input, "the table must be over the input");
    let mut parser = Parser::new(&src, NodeArena::init(t), recover);
    let result = parser.parse_once(format);
    if let Some(o) = out {
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

struct Parser<'s, 't> {
    version: Type,
    source: &'s [u8],
    tokens: Vec<Token>,
    pos: usize,
    arena: NodeArena<'t>,
    root_id: NodeId,
    // Comment layer. Unlike INI (full-line comments only), dotenv's grammar
    // has a real trailing comment (see the tokenizer): `last_value_id` tracks
    // the most recently parsed value so `capture_comment` can tell "a `#`
    // right after that value, same line" (trailing) from "a `#` on its own
    // line" (leading, buffered until the next key claims it) — mirrors
    // TOML's identical `last_value_id` window exactly.
    pending_leading: Vec<Comment<'s>>,
    last_value_id: Option<NodeId>,

    recover: bool,
    diagnostics: Vec<Diagnostic>,
    warnings: Vec<Warning>,
    fail_offset: Option<usize>,
    fail_end: Option<usize>,
}

impl<'s, 't> Parser<'s, 't> {
    fn new(source: &'s [u8], arena: NodeArena<'t>, recover: bool) -> Self {
        Parser {
            version: Type::Dotenv,
            source,
            tokens: Vec::new(),
            pos: 0,
            arena,
            root_id: 0,
            pending_leading: Vec::new(),
            last_value_id: None,
            recover,
            diagnostics: Vec::new(),
            warnings: Vec::new(),
            fail_offset: None,
            fail_end: None,
        }
    }

    fn dispatch_statement(&mut self) -> Result<()> {
        match self.peek().kind {
            Kind::Key => self.parse_key_value(),
            _ => Err(err!(UnexpectedToken)),
        }
    }

    /// The next `Newline`/`EndOfFile` token from `start` — always safe here:
    /// a multi-line quoted value is one token regardless of the newlines it
    /// embeds (see the tokenizer's module doc), so this can never land
    /// "inside" one, and (like INI, unlike TOML) no lexical state persists
    /// across a real newline, so the pre-computed token stream never needs
    /// re-tokenizing.
    fn resync(&self, start: usize) -> usize {
        let mut j = start;
        while j < self.tokens.len() {
            match self.tokens[j].kind {
                Kind::Newline | Kind::EndOfFile => return j,
                _ => {}
            }
            j += 1;
        }
        self.tokens.len() - 1
    }

    #[allow(dead_code)]
    fn fail_span(&mut self, start: usize, end: usize, e: Error) -> Error {
        self.fail_offset = Some(start);
        self.fail_end = Some(end);
        e
    }

    fn parse_once(&mut self, format: Type) -> Result<()> {
        self.version = format;
        let input = self.source;

        if std::str::from_utf8(input).is_err() {
            return Err(err!(InvalidUtf8));
        }

        let mut tokenizer = Tokenizer::new(input, format);
        self.tokens = match tokenizer.tokenize() {
            Ok(t) => t,
            Err(e) => {
                self.diagnostics.push(Diagnostic { code: e, offset: tokenizer.i, end: None });
                return Err(e);
            }
        };
        self.pending_leading.reserve(self.tokens.len());

        self.root_id = self.arena.add_node(NodeKind::Mapping(None), Span::new(0, input.len()))?;

        self.skip_blank()?;
        while !self.at_end() {
            if let Err(e) = self.dispatch_statement() {
                if e.is("OutOfMemory") {
                    return Err(e);
                }
                let offset = self.fail_offset.unwrap_or(self.peek().span.start);
                let end = self.fail_end;
                self.fail_offset = None;
                self.fail_end = None;
                self.diagnostics.push(Diagnostic { code: e, offset, end });
                if !self.recover {
                    return Err(e);
                }
                self.pos = self.resync(self.pos);
            }
            self.skip_blank()?;
        }
        self.claim_dangling(self.root_id)?;

        if let Some(d) = self.diagnostics.first() {
            return Err(d.code);
        }

        self.arena.w.finish(self.root_id)
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
    fn token_text(&self, tok: Token) -> &'s [u8] {
        &self.source[tok.span.start..tok.span.end]
    }

    fn skip_blank(&mut self) -> Result<()> {
        loop {
            match self.peek().kind {
                Kind::Comment => {
                    self.capture_comment(self.peek())?;
                    self.pos += 1;
                }
                Kind::Newline => {
                    self.last_value_id = None;
                    self.pos += 1;
                }
                _ => return Ok(()),
            }
        }
    }

    // ── Comments ────────────────────────────────────────────────────────────

    /// A comment right after a just-parsed value (`last_value_id` set, no
    /// newline since — the tokenizer only ever emits a comment token adjacent
    /// to a value like this when it scanned a real trailing `#`) binds to it
    /// as trailing; otherwise it's a leading comment buffered for the next
    /// key.
    fn capture_comment(&mut self, tok: Token) -> Result<()> {
        let text = trim(self.token_text(tok), b" \t\r");
        let c = Comment { text, style: Style::Line };
        if let Some(id) = self.last_value_id {
            self.arena.w.comment(id, table::COMMENT_TRAILING, c)?;
            self.last_value_id = None;
        } else {
            self.pending_leading.push(c);
        }
        Ok(())
    }

    fn claim_leading(&mut self, id: NodeId) -> Result<()> {
        for c in self.pending_leading.drain(..) {
            self.arena.w.comment(id, table::COMMENT_LEADING, c)?;
        }
        Ok(())
    }

    fn claim_dangling(&mut self, id: NodeId) -> Result<()> {
        for c in self.pending_leading.drain(..) {
            self.arena.w.comment(id, table::COMMENT_DANGLING, c)?;
        }
        Ok(())
    }

    fn add_warning(&mut self, code: WarningCode, span: Span) {
        self.warnings.push(Warning { code, offset: span.start, end: Some(span.end) });
    }

    // ── Statements ──────────────────────────────────────────────────────────

    /// `[export] KEY = value`. An `export` prefix is recognized only when the
    /// FIRST key-shaped token is literally that word AND a second key-shaped
    /// token follows it (so a variable genuinely named `export` — `export=1`
    /// — is unaffected: there, `=` immediately follows and no second key
    /// token exists to trigger this).
    fn parse_key_value(&mut self) -> Result<()> {
        let mut key_tok = self.peek();
        self.advance();
        if self.token_text(key_tok) == b"export" && self.peek().kind == Kind::Key {
            key_tok = self.peek();
            self.advance();
        }
        if self.peek().kind != Kind::Equals {
            return Err(err!(MissingEquals));
        }
        self.advance();

        let mut value_text: &'s [u8] = b"";
        let mut value_span = Span::new(self.peek().span.start, self.peek().span.start);
        let mut kind: Option<Kind> = None;
        match self.peek().kind {
            Kind::DoubleQuoted | Kind::SingleQuoted | Kind::Unquoted => {
                let tok = self.peek();
                self.advance();
                value_text = self.token_text(tok);
                value_span = tok.span;
                kind = Some(tok.kind);
            }
            _ => {} // `KEY=` at end of line: empty value
        }
        let decoded: Cow<'s, [u8]> = match kind {
            Some(k) => decode_value(value_text, k)?,
            None => Cow::Borrowed(b""),
        };

        let key_id = self.arena.add_node(NodeKind::String(self.token_text(key_tok)), key_tok.span)?;
        self.claim_leading(key_id)?;
        let value_id = self.arena.add_node(NodeKind::String(&decoded), value_span)?;
        // Opens the trailing-comment window for a `#` immediately following
        // on this same line (see `capture_comment`); `skip_blank`'s next real
        // newline closes it.
        self.last_value_id = Some(value_id);
        let put = flat_map::put_entry(&mut self.arena, self.root_id, key_id, value_id, DuplicatePolicy::Overwrite)?;
        if put.result == PutResult::Overwrote {
            self.add_warning(WarningCode::DuplicateKey, key_tok.span);
        }
        Ok(())
    }
}

fn decode_value(raw: &[u8], kind: Kind) -> Result<Cow<'_, [u8]>> {
    match kind {
        Kind::Unquoted => Ok(Cow::Borrowed(raw)), // never contains a literal newline; nothing to decode
        Kind::SingleQuoted => Ok(decode_single_quoted(raw)),
        Kind::DoubleQuoted => decode_double_quoted(raw).map(Cow::Owned),
        _ => unreachable!(),
    }
}

/// Fully raw between the quotes — no escapes — except a `\r\n` embedded in a
/// multi-line value normalizes to `\n` (fig's cross-platform line-ending
/// convention; see the YAML scanner's equivalent rule).
fn decode_single_quoted(raw: &[u8]) -> Cow<'_, [u8]> {
    let inner = &raw[1..raw.len() - 1];
    if !inner.contains(&b'\r') {
        return Cow::Borrowed(inner);
    }
    Cow::Owned(normalize_crlf(inner))
}

/// Decodes `\n \t \r \\ \"` and normalizes an embedded `\r\n` to `\n`,
/// matching a widely-used, minimal, unambiguous dotenv escape set — NOT
/// shell/JSON's fuller sets, and no `$VAR`/`${VAR}` interpolation (see the
/// module doc: fig is a reader, not a shell).
fn decode_double_quoted(raw: &[u8]) -> Result<Vec<u8>> {
    let inner = &raw[1..raw.len() - 1];
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let c = inner[i];
        if c == b'\r' && i + 1 < inner.len() && inner[i + 1] == b'\n' {
            out.push(b'\n');
            i += 2;
            continue;
        }
        if c == b'\\' {
            if i + 1 >= inner.len() {
                return Err(err!(BadEscape)); // tokenizer guarantees this can't happen
            }
            match inner[i + 1] {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'\\' => out.push(b'\\'),
                b'"' => out.push(b'"'),
                _ => return Err(err!(BadEscape)),
            }
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    Ok(out)
}

fn normalize_crlf(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'\r' && i + 1 < s.len() && s[i + 1] == b'\n' {
            out.push(b'\n');
            i += 2;
            continue;
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// Zig's `std.mem.trim`: `s` without any of `set` at either end.
fn trim<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let mut a = 0;
    let mut b = s.len();
    while a < b && set.contains(&s[a]) {
        a += 1;
    }
    while b > a && set.contains(&s[b - 1]) {
        b -= 1;
    }
    &s[a..b]
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::PathSegment;

    fn expect_root(input: &[u8], key: &[u8], value: &[u8]) {
        let ast = parse_abstract(input, Type::Dotenv).unwrap();
        let v = ast.get_val_by_path(&[PathSegment::Key(key)]).unwrap();
        assert_eq!(value, v.kind.as_string().unwrap());
    }

    #[test]
    fn unquoted_value_trimmed() {
        expect_root(b"NAME = fig \n", b"NAME", b"fig");
    }

    #[test]
    fn export_prefix_is_recognized_and_discarded() {
        expect_root(b"export NAME=fig\n", b"NAME", b"fig");
    }

    #[test]
    fn a_variable_actually_named_export_is_unaffected() {
        expect_root(b"export=1\n", b"export", b"1");
    }

    #[test]
    fn single_quoted_value_is_fully_raw() {
        expect_root(b"PATH = 'C:\\Users\\bob'\n", b"PATH", b"C:\\Users\\bob");
    }

    #[test]
    fn double_quoted_value_decodes_escapes() {
        expect_root(b"MSG=\"line1\\nline2\\t!\"\n", b"MSG", b"line1\nline2\t!");
    }

    #[test]
    fn double_quoted_value_may_embed_a_literal_newline() {
        expect_root(b"MSG=\"line1\nline2\"\n", b"MSG", b"line1\nline2");
    }

    #[test]
    fn single_quoted_value_may_also_embed_a_literal_newline() {
        expect_root(b"MSG='line1\nline2'\n", b"MSG", b"line1\nline2");
    }

    #[test]
    fn unquoted_trailing_comment_requires_preceding_whitespace() {
        expect_root(b"A=bar #note\n", b"A", b"bar");
        expect_root(b"A=bar#not-a-comment\n", b"A", b"bar#not-a-comment");
    }

    #[test]
    fn a_comment_may_follow_a_closing_quote_with_no_space() {
        expect_root(b"A=\"bar\"#note\n", b"A", b"bar");
    }

    #[test]
    fn empty_value() {
        expect_root(b"A=\n", b"A", b"");
    }

    #[test]
    fn full_line_comment_leads_the_next_key() {
        let ast = parse_abstract(b"# a header\nNAME=fig\n", Type::Dotenv).unwrap();
        let key_node = ast.first_child_key(&ast.node(ast.root)).unwrap().unwrap();
        let cs = ast.comments(key_node.id);
        assert_eq!(1, cs.leading.len());
        assert_eq!(b"a header", cs.leading[0].text);
    }

    #[test]
    fn repeated_key_keeps_first_position_last_value_and_warns() {
        let mut report = Report::default();
        let doc = parse_with_report(b"A=1\nB=2\nA=3\n", Type::Dotenv, &mut report).unwrap();
        let a = doc.ast.get_val_by_path(&[PathSegment::Key(b"A")]).unwrap();
        assert_eq!(b"3", a.kind.as_string().unwrap());
        assert_eq!(1, report.warnings.len());
        assert_eq!(WarningCode::DuplicateKey, report.warnings[0].code);
    }

    #[test]
    fn an_invalid_key_character_is_a_tokenizer_error() {
        assert_eq!(err!(UnexpectedChar), parse_abstract(b"1BAD=1\n", Type::Dotenv).unwrap_err());
    }

    #[test]
    fn missing_equals_is_a_recoverable_parser_error() {
        let mut report = Report::default();
        let result = parse_collecting(b"good=1\nbadline\nfine=2\n", Type::Dotenv, &mut report);
        assert_eq!(err!(MissingEquals), result.unwrap_err());
        assert_eq!(1, report.errors.len());
    }

    #[test]
    fn unclosed_quoted_value_is_an_error() {
        assert_eq!(err!(UnclosedString), parse_abstract(b"A=\"oops\n", Type::Dotenv).unwrap_err());
    }

    #[test]
    fn trailing_content_after_a_quoted_value_is_an_error() {
        assert_eq!(err!(TrailingContent), parse_abstract(b"A=\"ok\" junk\n", Type::Dotenv).unwrap_err());
    }

    #[test]
    fn invalid_escape_in_a_double_quoted_value_is_an_error() {
        assert_eq!(err!(BadEscape), parse_abstract(b"A=\"\\q\"\n", Type::Dotenv).unwrap_err());
    }
}
