//! Port of languages/properties/src/parser.zig.
//!
//! The parser turns a `.properties`-formatted `&[u8]` into the node table
//! fig hands it, a row per node (`RowWriter`, through `kit/flat_map.rs`).
//!
//! Shape: a single flat root mapping, `key<sep>value` per entry — like
//! dotenv, no sections/nesting; unlike dotenv, keys are NOT restricted to
//! identifiers (any escaped text up to the separator is legal, including the
//! empty string). Every value is a string node: no typed scalars, matching
//! `java.util.Properties`, which is itself just a `Hashtable<String,String>`.
//!
//! Like INI (and unlike dotenv), there is no trailing-comment syntax: a
//! `#`/`!` only starts a comment as the first byte of its OWN physical line
//! (see `tokenizer.rs`) — one appearing later is just part of the key/value
//! text.
//!
//! A repeated key silently keeps the LAST value at the FIRST-seen position
//! (see `kit/flat_map.rs`'s `put_entry`, `Overwrite`) and raises a
//! `duplicate_key` warning, exactly like INI/dotenv.

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
use crate::kit::unicode;
use crate::{Error, Result};

/// `ParseError`: the names the parser itself raises. `DuplicateKey` is
/// never actually raised — `parse_key_value` always calls
/// `flat_map::put_entry` with `Overwrite`, which never returns it — but Zig's
/// static error set includes it (the same function can be called with `Err`
/// elsewhere).
pub const PARSE_ERRORS: &[&str] = &["InvalidUnicode", "InvalidUtf8", "DuplicateKey"];

/// A long, teaching message for `code`.
pub fn describe(code: Error) -> &'static str {
    match code.name() {
        "InvalidUnicode" => "invalid \\uXXXX escape; expected exactly 4 hex digits forming a valid Unicode codepoint",
        "InvalidUtf8" => "this file is not valid UTF-8; fig requires .properties documents to be UTF-8 encoded",
        "UnexpectedCarriageReturn" => "a bare `\\r` must be followed by `\\n`; line endings must be `\\n` or `\\r\\n`",
        "UnclosedEscape" => "a `\\` at the very end of the file has nothing to escape",
        "OutOfMemory" => "out of memory",
        // `DuplicateKey`: see `PARSE_ERRORS`.
        other => unreachable!("not a .properties parse error: {other}"),
    }
}

/// A few words for the caret's label.
pub fn short_label(code: Error) -> &'static str {
    match code.name() {
        "InvalidUnicode" => "invalid unicode escape",
        "InvalidUtf8" => "invalid UTF-8",
        "UnexpectedCarriageReturn" => "bare CR",
        "UnclosedEscape" => "unclosed escape",
        "OutOfMemory" => "out of memory",
        // `DuplicateKey`: see `PARSE_ERRORS`.
        other => unreachable!("not a .properties parse error: {other}"),
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
    pending_leading: Vec<Comment<'s>>,

    recover: bool,
    diagnostics: Vec<Diagnostic>,
    warnings: Vec<Warning>,
    fail_offset: Option<usize>,
    fail_end: Option<usize>,
}

impl<'s, 't> Parser<'s, 't> {
    fn new(source: &'s [u8], arena: NodeArena<'t>, recover: bool) -> Self {
        Parser {
            version: Type::Properties,
            source,
            tokens: Vec::new(),
            pos: 0,
            arena,
            root_id: 0,
            pending_leading: Vec::new(),
            recover,
            diagnostics: Vec::new(),
            warnings: Vec::new(),
            fail_offset: None,
            fail_end: None,
        }
    }

    fn dispatch_statement(&mut self) -> Result<()> {
        // The tokenizer never emits anything else at a statement boundary (a
        // comment/newline is consumed by `skip_blank` before this runs).
        self.parse_key_value()
    }

    /// The next `Newline`/`EndOfFile` token from `start`. No lexical state
    /// persists across a real newline (a continuation is fully resolved
    /// WITHIN one key/value token by the tokenizer — see its module doc), so,
    /// like INI/dotenv and unlike TOML, the pre-computed token stream never
    /// needs re-tokenizing after a parser-level error.
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
    // Full-line only (see the module doc) — every captured comment simply
    // buffers as `pending_leading` until the next key claims it, or it
    // dangles at EOF, exactly like INI's (simpler) comment model.

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

    fn parse_key_value(&mut self) -> Result<()> {
        let key_tok = self.peek();
        self.advance();
        let key_str = decode_escaped(self.token_text(key_tok))?;

        // The tokenizer always emits a value, zero-width when the line has
        // none — where one would begin, after the separator.
        let value_tok = self.peek();
        self.advance();
        let value_str = decode_escaped(self.token_text(value_tok))?;
        let value_span = value_tok.span;

        let key_id = self.arena.add_node(NodeKind::String(&key_str), key_tok.span)?;
        self.claim_leading(key_id)?;
        let value_id = self.arena.add_node(NodeKind::String(&value_str), value_span)?;
        let put = flat_map::put_entry(&mut self.arena, self.root_id, key_id, value_id, DuplicatePolicy::Overwrite)?;
        if put.result == PutResult::Overwrote {
            self.add_warning(WarningCode::DuplicateKey, key_tok.span);
        }
        Ok(())
    }
}

/// Decode a raw key/value span: `\t \n \r \f \\`, `\uXXXX`, a `\<newline>`
/// continuation (drops the backslash, the newline, AND the resumed line's
/// leading whitespace — see the tokenizer), and — matching real
/// `java.util.Properties.load`, which never rejects an unrecognized escape —
/// `\` followed by anything else decodes to that byte literally.
fn decode_escaped(raw: &[u8]) -> Result<Cow<'_, [u8]>> {
    if !raw.contains(&b'\\') {
        return Ok(Cow::Borrowed(raw)); // nothing to decode; borrow
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let c = raw[i];
        if c != b'\\' {
            out.push(c);
            i += 1;
            continue;
        }
        if i + 1 < raw.len() && raw[i + 1] == b'\n' {
            i += 2;
            while i < raw.len() && (raw[i] == b' ' || raw[i] == b'\t') {
                i += 1;
            }
            continue;
        }
        if i + 2 < raw.len() && raw[i + 1] == b'\r' && raw[i + 2] == b'\n' {
            i += 3;
            while i < raw.len() && (raw[i] == b' ' || raw[i] == b'\t') {
                i += 1;
            }
            continue;
        }
        if i + 1 >= raw.len() {
            return Err(err!(UnclosedEscape)); // tokenizer guarantees this can't happen
        }
        match raw[i + 1] {
            b't' => out.push(b'\t'),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b'f' => out.push(0x0c),
            b'u' => {
                i = append_unicode(&mut out, raw, i + 2)? - 2;
            }
            ch => out.push(ch), // literal passthrough (incl. `\\`)
        }
        i += 2;
    }
    Ok(Cow::Owned(out))
}

/// Decode exactly 4 hex digits at `raw[at..]` into a UTF-8 codepoint
/// appended to `out`; returns the index just past the digits.
fn append_unicode(out: &mut Vec<u8>, raw: &[u8], at: usize) -> Result<usize> {
    if at + 4 > raw.len() {
        return Err(err!(InvalidUnicode));
    }
    let cp = parse_hex_u21(&raw[at..at + 4]).ok_or(err!(InvalidUnicode))?;
    unicode::encode_append(out, cp).map_err(|e| if e.is("InvalidCodepoint") { err!(InvalidUnicode) } else { e })?;
    Ok(at + 4)
}

/// Zig's `std.fmt.parseInt(u21, buf, 16)`: an optional `+`/`-` sign, then
/// hex digits with `_` allowed between them (not first or last). A negative
/// is an overflow unless every digit is zero. `None` for every refusal.
fn parse_hex_u21(buf: &[u8]) -> Option<u32> {
    let (digits, neg) = match buf.first()? {
        b'+' => (&buf[1..], false),
        b'-' => (&buf[1..], true),
        _ => (buf, false),
    };
    if digits.is_empty() || digits[0] == b'_' || digits[digits.len() - 1] == b'_' {
        return None;
    }
    let mut acc: u32 = 0;
    for &c in digits {
        if c == b'_' {
            continue;
        }
        let d = (c as char).to_digit(16)?;
        if neg && d != 0 {
            return None; // Overflow: a u21 holds no negative
        }
        acc = acc.checked_mul(16)?.checked_add(d)?;
        if acc > 0x1F_FFFF {
            return None;
        }
    }
    Some(acc)
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
        let ast = parse_abstract(input, Type::Properties).unwrap();
        let v = ast.get_val_by_path(&[PathSegment::Key(key)]).unwrap();
        assert_eq!(value, v.kind.as_string().unwrap());
    }

    #[test]
    fn equals_separator() {
        expect_root(b"db.host=localhost\n", b"db.host", b"localhost");
    }

    #[test]
    fn colon_separator() {
        expect_root(b"db.host: localhost\n", b"db.host", b"localhost");
    }

    #[test]
    fn whitespace_only_separator() {
        expect_root(b"db.host localhost\n", b"db.host", b"localhost");
    }

    #[test]
    fn surrounding_whitespace_around_the_separator_is_trimmed() {
        expect_root(b"db.host   =   localhost\n", b"db.host", b"localhost");
    }

    #[test]
    fn a_bare_key_with_no_separator_has_an_empty_value() {
        expect_root(b"flag\n", b"flag", b"");
    }

    #[test]
    fn line_continuation_joins_onto_the_next_physical_line_stripping_its_leading_whitespace() {
        expect_root(b"long=part1\\\n   part2\n", b"long", b"part1part2");
    }

    #[test]
    fn escapes_t_n_r_f_backslash() {
        expect_root(b"v=a\\tb\\nc\\\\d\n", b"v", b"a\tb\nc\\d");
    }

    #[test]
    fn unicode_escape() {
        expect_root(b"v=\\u00e9\n", b"v", "\u{e9}".as_bytes()); // é
    }

    #[test]
    fn an_unrecognized_escape_decodes_to_the_literal_escaped_character() {
        expect_root(b"v=\\:\\=\\#\\ x\n", b"v", b":=# x");
    }

    #[test]
    fn escaping_a_separator_character_embeds_it_literally_in_the_key() {
        expect_root(b"a\\:b=1\n", b"a:b", b"1");
    }

    #[test]
    fn comments_hash_and_bang_full_line_only() {
        let ast = parse_abstract(b"# a header\n! also a comment\nname=fig\n", Type::Properties).unwrap();
        let key_node = ast.first_child_key(&ast.node(ast.root)).unwrap().unwrap();
        let cs = ast.comments(key_node.id);
        assert_eq!(2, cs.leading.len());
        assert_eq!(b"a header", cs.leading[0].text);
        assert_eq!(b"also a comment", cs.leading[1].text);
    }

    #[test]
    fn a_hash_in_the_middle_of_a_line_is_literal_not_a_comment() {
        expect_root(b"v=a#b\n", b"v", b"a#b");
    }

    #[test]
    fn empty_key() {
        expect_root(b"=value\n", b"", b"value");
    }

    #[test]
    fn repeated_key_keeps_first_position_last_value_and_warns() {
        let mut report = Report::default();
        let doc = parse_with_report(b"a=1\nb=2\na=3\n", Type::Properties, &mut report).unwrap();
        let a = doc.ast.get_val_by_path(&[PathSegment::Key(b"a")]).unwrap();
        assert_eq!(b"3", a.kind.as_string().unwrap());
        assert_eq!(1, report.warnings.len());
        assert_eq!(WarningCode::DuplicateKey, report.warnings[0].code);
    }

    #[test]
    fn invalid_unicode_escape_is_an_error() {
        assert_eq!(err!(InvalidUnicode), parse_abstract(b"v=\\uZZZZ\n", Type::Properties).unwrap_err());
    }

    #[test]
    fn parse_hex_u21_reads_as_zig_parse_int_does() {
        assert_eq!(Some(0xe9), parse_hex_u21(b"00e9"));
        assert_eq!(Some(0xe9), parse_hex_u21(b"+0e9"));
        assert_eq!(Some(0xe9), parse_hex_u21(b"0_e9"));
        assert_eq!(Some(0), parse_hex_u21(b"-000"));
        assert_eq!(None, parse_hex_u21(b"-001"));
        assert_eq!(None, parse_hex_u21(b"_0e9"));
        assert_eq!(None, parse_hex_u21(b"0e9_"));
        assert_eq!(None, parse_hex_u21(b"+_e9"));
        assert_eq!(None, parse_hex_u21(b"00g9"));
    }
}
