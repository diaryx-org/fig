//! Port of languages/ini/src/parser.zig.
//!
//! The parser turns an INI-formatted `&[u8]` into the node table fig hands
//! it, a row per node (`RowWriter`, through `kit/flat_map.rs`).
//!
//! Shape: the root mapping holds any keys that appear before the first
//! `[section]`, plus one child mapping per section (keyed by section name).
//! INI has no dotted keys, arrays, or inline tables, and — unlike TOML —
//! no typed scalars: every value is a string node (INI's real-world
//! semantics never coerce a value's type at the format layer; that's a
//! consumer concern). A value that is wholly wrapped in matching `"`/`'`
//! quotes has them stripped verbatim (no escape decoding — see the
//! tokenizer's module doc for why).
//!
//! A repeated `[section]` header reopens (merges into) the existing section
//! rather than erroring — the common lenient behavior (git-config, many INI
//! libraries). A repeated key anywhere silently keeps the LAST value at the
//! FIRST-seen position (see `kit/flat_map.rs`'s `put_entry`) and raises a
//! `duplicate_key`/`duplicate_section` warning — real, parseable content, but
//! likely an authoring mistake, so it's a lint rather than a parse error.

use std::sync::Arc;

use super::tokenizer::{Kind, Token, Tokenizer};
use super::Type;
use crate::err;
use crate::kit::ast::{Comment, NodeId, NodeKind, Style, AST};
use crate::kit::document::{self, Document, MentionKind};
use crate::kit::flat_map::{self, DuplicatePolicy, NodeArena, PutResult};
use crate::kit::parse_diagnostic;
use crate::kit::row_writer::RowWriter;
use crate::kit::span::Span;
use crate::kit::table::{self, Table};
use crate::{Error, Result};

/// `ParseError`: the names the parser itself raises.
pub const PARSE_ERRORS: &[&str] = &["UnexpectedToken", "InvalidKey", "DuplicateKey", "InvalidUtf8"];

/// A long, teaching message for `code`.
pub fn describe(code: Error) -> &'static str {
    match code.name() {
        "UnexpectedToken" => "unexpected content here; expected a `[section]` header or a `key = value` line",
        "InvalidKey" => "a key/section name cannot be empty",
        "DuplicateKey" => "this section conflicts with a key of the same name already defined at this level",
        "InvalidUtf8" => "this file is not valid UTF-8; INI documents must be UTF-8 encoded",
        "UnexpectedCarriageReturn" => "a bare `\\r` must be followed by `\\n`; line endings must be `\\n` or `\\r\\n`",
        "UnclosedSection" => "unclosed `[section]` header; expected a `]` before the end of the line",
        "MissingEquals" => "expected `=` after this key; every INI line is `key = value`",
        "TrailingContent" => "unexpected content after `]`; a section header must be alone on its line",
        "OutOfMemory" => "out of memory",
        other => unreachable!("not an INI parse error: {other}"),
    }
}

/// A few words for the caret's label.
pub fn short_label(code: Error) -> &'static str {
    match code.name() {
        "UnexpectedToken" => "unexpected content",
        "InvalidKey" => "empty key",
        "DuplicateKey" => "section/key conflict",
        "InvalidUtf8" => "invalid UTF-8",
        "UnexpectedCarriageReturn" => "bare CR",
        "UnclosedSection" => "unclosed section",
        "MissingEquals" => "missing `=`",
        "TrailingContent" => "trailing content",
        "OutOfMemory" => "out of memory",
        other => unreachable!("not an INI parse error: {other}"),
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

/// An authoring-time lint: valid, parseable INI, but a shape a reader
/// wouldn't expect — currently only "this key/section was defined more than
/// once, and the later one silently won."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warning {
    pub code: WarningCode,
    pub offset: usize,
    pub end: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    DuplicateKey,
    DuplicateSection,
}

impl Warning {
    pub fn describe_warning(code: WarningCode) -> &'static str {
        match code {
            WarningCode::DuplicateKey => {
                "this key is defined more than once; the last value wins and earlier ones are silently discarded"
            }
            WarningCode::DuplicateSection => {
                "this section is defined more than once; entries merge into the first occurrence"
            }
        }
    }
    pub fn short_label(code: WarningCode) -> &'static str {
        match code {
            WarningCode::DuplicateKey => "duplicate key",
            WarningCode::DuplicateSection => "duplicate section",
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
    /// The mapping new keys attach to: the root until the first `[section]`,
    /// then whichever section is currently open.
    current_table: NodeId,
    // Comment layer: INI comments are full-line only (see the tokenizer), so
    // there is no trailing-comment window to track like TOML's
    // `last_value_id` — every captured comment is simply buffered as
    // `pending_leading` until the next key/section-header claims it (or it
    // dangles at EOF).
    pending_leading: Vec<Comment<'s>>,
    // Every `[section]` header line is written as a region of the section's
    // node (`Document::regions_of`) — the one that created it and every
    // REOPENING `[a]` in `[a]…[b]…[a]`. A section's node span anchors only
    // its name token in the first header, so this is what lets the editor's
    // region gather find every physical occurrence rather than just the
    // first, the same fact fig's and TOML's parsers write for the same
    // reason. Each header's name token is a mention of the section
    // (`Document::mentions_of`).
    recover: bool,
    diagnostics: Vec<Diagnostic>,
    warnings: Vec<Warning>,
    /// Overrides the next diagnostic's offset/end when the cursor has already
    /// scanned past the offending span — mirrors TOML's `Parser.failAt`/`failSpan`.
    fail_offset: Option<usize>,
    fail_end: Option<usize>,
}

impl<'s, 't> Parser<'s, 't> {
    fn new(source: &'s [u8], arena: NodeArena<'t>, recover: bool) -> Self {
        Parser {
            version: Type::Ini,
            source,
            tokens: Vec::new(),
            pos: 0,
            arena,
            root_id: 0,
            current_table: 0,
            pending_leading: Vec::new(),
            recover,
            diagnostics: Vec::new(),
            warnings: Vec::new(),
            fail_offset: None,
            fail_end: None,
        }
    }

    fn dispatch_statement(&mut self) -> Result<()> {
        match self.peek().kind {
            Kind::OpenBracket => self.parse_section_header(),
            Kind::Text => self.parse_key_value(),
            _ => Err(err!(UnexpectedToken)),
        }
    }

    /// From `start`, the next `Newline`/`EndOfFile` token — always safe for
    /// INI (unlike TOML, no lexical state persists across a newline, so the
    /// pre-computed token stream never needs re-tokenizing after a
    /// parser-level error; see the tokenizer's module doc).
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
    fn fail_at(&mut self, offset: usize, e: Error) -> Error {
        self.fail_offset = Some(offset);
        e
    }

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
        self.current_table = self.root_id;

        self.skip_blank();
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
            self.skip_blank();
        }
        self.claim_dangling(self.current_table)?;

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

    fn skip_blank(&mut self) {
        loop {
            match self.peek().kind {
                Kind::Comment => {
                    self.capture_comment(self.peek());
                    self.pos += 1;
                }
                Kind::Newline => self.pos += 1,
                _ => return,
            }
        }
    }

    // ── Comments ────────────────────────────────────────────────────────────

    fn capture_comment(&mut self, tok: Token) {
        let text = trim(self.token_text(tok), b" \t\r");
        self.pending_leading.push(Comment { text, style: Style::Line });
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

    /// `[name]` — reopen the existing section mapping if `name` was seen
    /// before (merge), else create a fresh child mapping under root.
    fn parse_section_header(&mut self) -> Result<()> {
        self.advance(); // '['
        let name_tok = self.peek();
        if name_tok.kind != Kind::Text {
            return Err(err!(UnexpectedToken));
        }
        self.advance();
        let name = self.token_text(name_tok);
        if self.peek().kind != Kind::CloseBracket {
            return Err(err!(UnexpectedToken));
        }
        self.advance();
        if name.is_empty() {
            return Err(self.fail_span(name_tok.span.start, name_tok.span.end, err!(InvalidKey)));
        }

        if let Some(existing_kv) = flat_map::lookup_child(&self.arena, self.root_id, name) {
            let kv = self.arena.w.kind_of(existing_kv).as_keyvalue().expect("an entry");
            if !matches!(self.arena.w.kind_of(kv.value), NodeKind::Mapping(_)) {
                return Err(self.fail_span(name_tok.span.start, name_tok.span.end, err!(DuplicateKey)));
            }
            self.current_table = kv.value;
            // The header line that REOPENED this section. Its position is in
            // no node's span (the section's own anchors the first `[a]`), so
            // record it for the editor's region gather.
            self.record_header(kv.value, name_tok)?;
            self.add_warning(WarningCode::DuplicateSection, name_tok.span);
        } else {
            let key_id = self.arena.add_node(NodeKind::String(name), name_tok.span)?;
            self.claim_leading(key_id)?;
            let map_id = self.arena.add_node(NodeKind::Mapping(None), name_tok.span)?;
            flat_map::put_entry(&mut self.arena, self.root_id, key_id, map_id, DuplicatePolicy::Overwrite)?;
            self.record_header(map_id, name_tok)?;
            self.current_table = map_id;
        }
        Ok(())
    }

    /// Record the header line naming section `id` at `name_tok` as one of
    /// its regions, and the name as one of its mentions.
    fn record_header(&mut self, id: NodeId, name_tok: Token) -> Result<()> {
        let line = document::line_region_at(self.source, name_tok.span.start);
        self.arena.w.region(id, line.start, line.end)?;
        self.arena.w.mention(id, name_tok.span, MentionKind::Header)
    }

    /// `key = value`, attaching to `current_table`.
    fn parse_key_value(&mut self) -> Result<()> {
        let key_tok = self.peek();
        self.advance();
        let key_str = self.token_text(key_tok);
        if key_str.is_empty() {
            return Err(self.fail_span(key_tok.span.start, key_tok.span.end, err!(InvalidKey)));
        }
        if self.peek().kind != Kind::Equals {
            return Err(err!(UnexpectedToken));
        }
        self.advance();

        // A value token is absent for `key=` at end of line (empty value).
        let mut value_text: &'s [u8] = b"";
        let mut value_span = Span::new(self.peek().span.start, self.peek().span.start);
        if self.peek().kind == Kind::Text {
            let value_tok = self.peek();
            self.advance();
            value_text = self.token_text(value_tok);
            value_span = value_tok.span;
        }

        let key_id = self.arena.add_node(NodeKind::String(key_str), key_tok.span)?;
        self.claim_leading(key_id)?;
        let value_id = self.arena.add_node(NodeKind::String(decode_value(value_text)), value_span)?;
        let put = flat_map::put_entry(&mut self.arena, self.current_table, key_id, value_id, DuplicatePolicy::Overwrite)?;
        if put.result == PutResult::Overwrote {
            self.add_warning(WarningCode::DuplicateKey, key_tok.span);
        }
        Ok(())
    }
}

/// Strip one layer of matching `"..."`/`'...'` quotes spanning the whole
/// (already-trimmed) value. No escape decoding — see the tokenizer's doc.
fn decode_value(raw: &[u8]) -> &[u8] {
    if raw.len() >= 2 {
        let q = raw[0];
        if (q == b'"' || q == b'\'') && raw[raw.len() - 1] == q {
            return &raw[1..raw.len() - 1];
        }
    }
    raw
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
        let ast = parse_abstract(input, Type::Ini).unwrap();
        let v = ast.get_val_by_path(&[PathSegment::Key(key)]).unwrap();
        assert_eq!(value, v.kind.as_string().unwrap());
    }

    #[test]
    fn root_level_key_before_any_section() {
        expect_root(b"name = fig\n", b"name", b"fig");
    }

    #[test]
    fn value_runs_to_end_of_line_untouched_whitespace_trimmed() {
        expect_root(b"path = C:\\Users\\bob;keep\n", b"path", b"C:\\Users\\bob;keep");
    }

    #[test]
    fn quoted_value_has_its_outer_quotes_stripped() {
        expect_root(b"name = \"fig lang\"\n", b"name", b"fig lang");
        expect_root(b"name = 'fig lang'\n", b"name", b"fig lang");
    }

    #[test]
    fn section_nesting() {
        let ast = parse_abstract(b"[server]\nhost = localhost\nport = 80\n", Type::Ini).unwrap();
        let v = ast.get_val_by_path(&[PathSegment::Key(b"server"), PathSegment::Key(b"host")]).unwrap();
        assert_eq!(b"localhost", v.kind.as_string().unwrap());
    }

    #[test]
    fn a_section_name_of_only_whitespace_is_an_empty_name_refused() {
        // `[ ]` used to tokenize to an inverted span (trimmed from both ends
        // independently), which read as garbage and crashed the printer.
        for src in [&b"[ ]\n"[..], b"[\t]\n", b"a = 1\n[  ]\nb = 2\n"] {
            let mut report = Report::default();
            assert_eq!(err!(InvalidKey), parse_with_report(src, Type::Ini, &mut report).unwrap_err());
        }
        let mut report = Report::default();
        assert_eq!(err!(InvalidKey), parse_with_report(b"x = 1\n[ ]\n", Type::Ini, &mut report).unwrap_err());
        // Pinned to the empty name: the `]`, where the trimmed name ends.
        assert_eq!(8, report.diag.unwrap().offset);
    }

    #[test]
    fn repeated_section_merges_entries_and_warns() {
        let mut report = Report::default();
        let doc = parse_with_report(b"[a]\nx = 1\n[a]\ny = 2\n", Type::Ini, &mut report).unwrap();
        let x = doc.ast.get_val_by_path(&[PathSegment::Key(b"a"), PathSegment::Key(b"x")]).unwrap();
        let y = doc.ast.get_val_by_path(&[PathSegment::Key(b"a"), PathSegment::Key(b"y")]).unwrap();
        assert_eq!(b"1", x.kind.as_string().unwrap());
        assert_eq!(b"2", y.kind.as_string().unwrap());
        assert_eq!(1, report.warnings.len());
        assert_eq!(WarningCode::DuplicateSection, report.warnings[0].code);
    }

    #[test]
    fn repeated_key_keeps_first_position_last_value_and_warns() {
        let mut report = Report::default();
        let doc = parse_with_report(b"a = 1\nb = 2\na = 3\n", Type::Ini, &mut report).unwrap();
        let a = doc.ast.get_val_by_path(&[PathSegment::Key(b"a")]).unwrap();
        assert_eq!(b"3", a.kind.as_string().unwrap());
        assert_eq!(1, report.warnings.len());
        assert_eq!(WarningCode::DuplicateKey, report.warnings[0].code);
    }

    #[test]
    fn comments_full_line_only_leading_run_attaches_to_the_next_key() {
        let ast = parse_abstract(b"; a header comment\n# another style\nname = fig\n", Type::Ini).unwrap();
        let key_node = ast.first_child_key(&ast.node(ast.root)).unwrap().unwrap();
        let cs = ast.comments(key_node.id);
        assert_eq!(2, cs.leading.len());
        assert_eq!(b"a header comment", cs.leading[0].text);
        assert_eq!(b"another style", cs.leading[1].text);
    }

    #[test]
    fn a_semicolon_or_hash_inside_a_value_is_literal_not_a_comment() {
        expect_root(b"greeting = hi ; not a comment #still not\n", b"greeting", b"hi ; not a comment #still not");
    }

    #[test]
    fn empty_value() {
        expect_root(b"flag = \n", b"flag", b"");
    }

    #[test]
    fn missing_equals_is_an_error() {
        assert_eq!(err!(MissingEquals), parse_abstract(b"not-a-kv-line\n", Type::Ini).unwrap_err());
    }

    #[test]
    fn unclosed_section_is_an_error() {
        assert_eq!(err!(UnclosedSection), parse_abstract(b"[oops\n", Type::Ini).unwrap_err());
    }

    #[test]
    fn trailing_content_after_a_section_header_is_an_error() {
        assert_eq!(err!(TrailingContent), parse_abstract(b"[a] junk\n", Type::Ini).unwrap_err());
    }

    #[test]
    fn a_section_colliding_with_an_existing_plain_key_is_a_duplicate_key_error() {
        assert_eq!(err!(DuplicateKey), parse_abstract(b"a = 1\n[a]\nx = 2\n", Type::Ini).unwrap_err());
    }

    #[test]
    fn parse_collecting_recovers_across_multiple_parser_level_errors() {
        // `MissingEquals`/`UnclosedSection` are TOKENIZER errors — like TOML,
        // those abort the whole lexical pass before the parser's statement
        // loop (and its recovery) ever starts, so they can't be collected
        // one-per-line. This exercises recovery over genuine PARSER-level
        // errors instead: two section/key name collisions in one file.
        let mut report = Report::default();
        let result = parse_collecting(b"a = 1\n[a]\nx = 1\nb = 2\n[b]\ny = 2\n", Type::Ini, &mut report);
        assert_eq!(err!(DuplicateKey), result.unwrap_err());
        assert_eq!(2, report.errors.len());
    }

    #[test]
    fn regions_record_every_section_header_line_reopenings_included_keyed_by_node_id() {
        let src: &[u8] = b"[a]\nx = 1\n[b]\ny = 2\n[a]\nz = 3\n";
        let parsed = parse(src, Type::Ini).unwrap();
        let a = parsed.ast.get_val_by_path(&[PathSegment::Key(b"a")]).unwrap();
        let regs = parsed.regions_of(a.id);
        assert_eq!(2, regs.len());
        assert_eq!(0, regs[0].start);
        assert_eq!(b"[a]\n".len() as u32, regs[0].end);
        // The SECOND `[a]` line — the occurrence the section's own span
        // (pinned to the first header's name token) cannot carry. The
        // editor's region gather is the consumer; see `editor/regions.rs`.
        let last = src.windows(3).rposition(|w| w == b"[a]").unwrap();
        assert_eq!(last as u32, regs[1].start);
        assert_eq!((src.len() - b"z = 3\n".len()) as u32, regs[1].end);
        let b = parsed.ast.get_val_by_path(&[PathSegment::Key(b"b")]).unwrap();
        assert_eq!(1, parsed.regions_of(b.id).len());
        // Neither the root nor an entry is a section.
        assert!(!parsed.is_section(&parsed.ast.node(parsed.ast.root)));
        assert!(!parsed.is_section(&parsed.ast.get_val_by_path(&[PathSegment::Key(b"a"), PathSegment::Key(b"x")]).unwrap()));
    }

    #[test]
    fn regions_are_empty_for_a_document_with_no_section() {
        let parsed = parse(b"x = 1\ny = 2\n", Type::Ini).unwrap();
        assert_eq!(0, parsed.ast.region_facts().len());
    }
}
