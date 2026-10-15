//! Port of languages/toml/src/tokenizer.zig.
//!
//! TOML tokenizer. Turns a TOML `&[u8]` into a list of Tokens.
//!
//! TOML is line-oriented and context-sensitive: the same bytes mean different
//! things before vs after `=` (a bare key `1979-05-27` vs a date value), so the
//! tokenizer tracks a per-line key/value position. Value scalars are emitted as
//! single tokens (`String` / `Number` / `Datetime` / `Boolean`) spanning
//! their whole source; the parser decodes them, mirroring the JSON tokenizer.
//!
//! Table headers (`[...]`/`[[...]]`), arrays, and inline tables emit their
//! bracket/brace tokens and are assembled by the parser.
//!
//! Errors (`TokenizeError`): `UnexpectedCarriageReturn`, `UnclosedString`,
//! `BadKey`, `BadValue`, `BadControlChar`.

use super::Type;
use crate::kit::span::Span;
use crate::kit::token;
use crate::{err, Result};

pub type Token = token::Token<Kind>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    // Structural
    /// =
    Equals,
    /// .
    Dot,
    /// ,
    Comma,
    /// [
    OpenBracket,
    /// ]
    CloseBracket,
    /// [[
    DoubleOpenBracket,
    /// ]]
    DoubleCloseBracket,
    /// {
    OpenBrace,
    /// }
    CloseBrace,
    Newline,
    EndOfFile,

    // variable-length
    /// bare or quoted key
    Key,
    /// string scalar (basic/literal, single or multi-line)
    String,
    /// integer/float scalar (raw, parser validates)
    Number,
    /// true/false
    Boolean,
    /// RFC 3339-derived date/time scalar
    Datetime,
    Comment,
    Whitespace,
}

impl token::TokenKind for Kind {
    /// Length of fixed-width token kinds; `None` for variable-length.
    fn len(self) -> Option<usize> {
        match self {
            Kind::EndOfFile => Some(0),
            Kind::Equals | Kind::Dot | Kind::Comma | Kind::OpenBracket | Kind::CloseBracket | Kind::OpenBrace | Kind::CloseBrace => Some(1),
            Kind::DoubleOpenBracket | Kind::DoubleCloseBracket => Some(2),
            _ => None,
        }
    }
}

/// The names `tokenize` may fail with.
pub const TOKENIZE_ERRORS: &[&str] = &["UnexpectedCarriageReturn", "UnclosedString", "BadKey", "BadValue", "BadControlChar", "OutOfMemory"];

/// Control characters forbidden in comments and single-line strings: U+0000–
/// U+0008, U+000A–U+001F, U+007F. Tab (U+0009) is allowed.
fn forbidden_inline(c: u8) -> bool {
    (c < 0x20 && c != b'\t') || c == 0x7f
}

/// Same, but for multi-line strings, which additionally permit the newline
/// bytes (U+000A, U+000D).
fn forbidden_multiline(c: u8) -> bool {
    (c < 0x20 && c != b'\t' && c != b'\n' && c != b'\r') || c == 0x7f
}

#[derive(Debug, Clone, Copy)]
struct Flow {
    /// `{` inline table (vs `[` array).
    table: bool,
    /// (tables only) currently reading a key, not a value.
    expect_key: bool,
}

#[derive(Debug)]
pub struct Tokenizer<'a> {
    pub tokens: Vec<Token>,
    pub str: &'a [u8],
    pub version: Type,
    pub i: usize,
    /// True between `=` and end-of-line at the top level: value position. Reset at
    /// every newline (only when not inside a flow collection).
    in_value: bool,
    /// Stack of open `[`/`{` collections. Inside a `{ }` inline table we must know
    /// whether we're at a key or a value position so a `.` is read as a key
    /// separator vs a decimal point — the line-level `in_value` can't track that.
    flow: Vec<Flow>,
}

impl<'a> Tokenizer<'a> {
    pub fn new(str: &'a [u8], version: Type) -> Tokenizer<'a> {
        Tokenizer { tokens: Vec::new(), str, version, i: 0, in_value: false, flow: Vec::new() }
    }

    fn in_flow(&self) -> bool {
        !self.flow.is_empty()
    }

    fn flow_top(&mut self) -> &mut Flow {
        self.flow.last_mut().expect("in flow")
    }

    /// True when the next token is an inline-table key (bare key / quoted key / dot
    /// / `=`), rather than a value.
    fn at_inline_key(&self) -> bool {
        matches!(self.flow.last(), Some(f) if f.table && f.expect_key)
    }

    /// The tokens of `str`. On failure `i` rests on (or just after) the
    /// offending byte.
    pub fn tokenize(&mut self) -> Result<Vec<Token>> {
        // A leading UTF-8 BOM is permitted and ignored.
        if self.str.starts_with(b"\xEF\xBB\xBF") {
            self.i = 3;
        }

        while self.i < self.str.len() {
            let c = self.str[self.i];
            match c {
                b'\n' => {
                    self.emit(Kind::Newline, self.i, self.i + 1);
                    self.i += 1;
                    if !self.in_flow() {
                        self.in_value = false;
                    }
                }
                b'\r' => {
                    // CR is only valid as part of CRLF.
                    if self.i + 1 < self.str.len() && self.str[self.i + 1] == b'\n' {
                        self.emit(Kind::Newline, self.i, self.i + 2);
                        self.i += 2;
                        if !self.in_flow() {
                            self.in_value = false;
                        }
                    } else {
                        return Err(err!(UnexpectedCarriageReturn));
                    }
                }
                b' ' | b'\t' => self.lex_whitespace(),
                b'#' => self.lex_comment()?,
                _ => {
                    if self.in_value || self.in_flow() {
                        self.lex_value()?;
                    } else {
                        self.lex_key_context()?;
                    }
                }
            }
        }

        self.emit(Kind::EndOfFile, self.str.len(), self.str.len());
        self.flow.clear();
        Ok(std::mem::take(&mut self.tokens))
    }

    fn emit(&mut self, kind: Kind, start: usize, end: usize) {
        self.tokens.push(Token::new(kind, Span::new(start, end)));
    }

    fn lex_whitespace(&mut self) {
        let start = self.i;
        while self.i < self.str.len() && (self.str[self.i] == b' ' || self.str[self.i] == b'\t') {
            self.i += 1;
        }
        self.emit(Kind::Whitespace, start, self.i);
    }

    fn lex_comment(&mut self) -> Result<()> {
        let start = self.i;
        while self.i < self.str.len() && self.str[self.i] != b'\n' && self.str[self.i] != b'\r' {
            if forbidden_inline(self.str[self.i]) {
                return Err(err!(BadControlChar));
            }
            self.i += 1;
        }
        self.emit(Kind::Comment, start, self.i);
        Ok(())
    }

    // ── Key context ─────────────────────────────────────────────────────────

    fn lex_key_context(&mut self) -> Result<()> {
        let c = self.str[self.i];
        match c {
            b'=' => {
                self.emit(Kind::Equals, self.i, self.i + 1);
                self.i += 1;
                self.in_value = true;
            }
            b'.' => {
                self.emit(Kind::Dot, self.i, self.i + 1);
                self.i += 1;
            }
            b'[' => {
                if self.i + 1 < self.str.len() && self.str[self.i + 1] == b'[' {
                    self.emit(Kind::DoubleOpenBracket, self.i, self.i + 2);
                    self.i += 2;
                } else {
                    self.emit(Kind::OpenBracket, self.i, self.i + 1);
                    self.i += 1;
                }
            }
            b']' => {
                if self.i + 1 < self.str.len() && self.str[self.i + 1] == b']' {
                    self.emit(Kind::DoubleCloseBracket, self.i, self.i + 2);
                    self.i += 2;
                } else {
                    self.emit(Kind::CloseBracket, self.i, self.i + 1);
                    self.i += 1;
                }
            }
            b'"' | b'\'' => self.lex_quoted_key()?,
            _ => self.lex_bare_key()?,
        }
        Ok(())
    }

    fn lex_bare_key(&mut self) -> Result<()> {
        let start = self.i;
        while self.i < self.str.len() && is_bare_key_char(self.str[self.i]) {
            self.i += 1;
        }
        if self.i == start {
            return Err(err!(BadKey)); // nothing consumed → stray char
        }
        self.emit(Kind::Key, start, self.i);
        Ok(())
    }

    /// A quoted key is a single-line basic or literal string used as a key.
    fn lex_quoted_key(&mut self) -> Result<()> {
        let start = self.i;
        self.scan_single_line_string(self.str[self.i])?;
        self.emit(Kind::Key, start, self.i);
        Ok(())
    }

    // ── Value context ───────────────────────────────────────────────────────

    fn lex_value(&mut self) -> Result<()> {
        let c = self.str[self.i];
        match c {
            b'[' => {
                self.emit(Kind::OpenBracket, self.i, self.i + 1);
                self.i += 1;
                self.flow.push(Flow { table: false, expect_key: false });
            }
            b']' => {
                if self.in_flow() {
                    self.flow.pop();
                }
                self.emit(Kind::CloseBracket, self.i, self.i + 1);
                self.i += 1;
            }
            b'{' => {
                self.emit(Kind::OpenBrace, self.i, self.i + 1);
                self.i += 1;
                self.flow.push(Flow { table: true, expect_key: true });
            }
            b'}' => {
                if self.in_flow() {
                    self.flow.pop();
                }
                self.emit(Kind::CloseBrace, self.i, self.i + 1);
                self.i += 1;
            }
            b',' => {
                self.emit(Kind::Comma, self.i, self.i + 1);
                self.i += 1;
                if self.in_flow() && self.flow_top().table {
                    self.flow_top().expect_key = true;
                }
            }
            b'=' => {
                self.emit(Kind::Equals, self.i, self.i + 1);
                self.i += 1;
                if self.in_flow() && self.flow_top().table {
                    self.flow_top().expect_key = false;
                }
            }
            b'.' => {
                self.emit(Kind::Dot, self.i, self.i + 1);
                self.i += 1;
            }
            b'"' | b'\'' => {
                // A quoted key in inline-table key position, else a string value.
                // Both lex identically; the parser interprets by position.
                self.lex_string_value()?;
            }
            _ => {
                if self.at_inline_key() {
                    return self.lex_bare_key();
                }
                if let Some(end) = self.match_datetime(self.i) {
                    self.emit(Kind::Datetime, self.i, end);
                    self.i = end;
                    return Ok(());
                }
                self.lex_bareword()?;
            }
        }
        Ok(())
    }

    fn lex_string_value(&mut self) -> Result<()> {
        let start = self.i;
        let q = self.str[self.i];
        if self.i + 2 < self.str.len() && self.str[self.i + 1] == q && self.str[self.i + 2] == q {
            self.scan_multi_line_string(q)?;
        } else {
            self.scan_single_line_string(q)?;
        }
        self.emit(Kind::String, start, self.i);
        Ok(())
    }

    /// Scan a single-line basic (`"`) or literal (`'`) string, leaving `i`
    /// just past the closing quote. A newline before the close is an error.
    fn scan_single_line_string(&mut self, q: u8) -> Result<()> {
        debug_assert!(self.str[self.i] == q);
        self.i += 1;
        let basic = q == b'"';
        while self.i < self.str.len() {
            let c = self.str[self.i];
            if c == b'\n' || c == b'\r' {
                return Err(err!(UnclosedString));
            }
            if forbidden_inline(c) {
                return Err(err!(BadControlChar));
            }
            if basic && c == b'\\' {
                // Skip the escaped byte (validated by the parser's decoder).
                self.i += 2;
                continue;
            }
            if c == q {
                self.i += 1;
                return Ok(());
            }
            self.i += 1;
        }
        Err(err!(UnclosedString))
    }

    /// Scan a multi-line basic (`"""`) or literal (`'''`) string. The opening and
    /// closing delimiters are three `q`s; basic strings honor `\` escapes (so an
    /// escaped quote doesn't close the string).
    fn scan_multi_line_string(&mut self, q: u8) -> Result<()> {
        self.i += 3; // opening delimiter
        let basic = q == b'"';
        while self.i < self.str.len() {
            let c = self.str[self.i];
            if forbidden_multiline(c) {
                return Err(err!(BadControlChar));
            }
            // A carriage return is only allowed as part of CRLF, even in a
            // multi-line string (a bare CR is a forbidden control char).
            if c == b'\r' && !(self.i + 1 < self.str.len() && self.str[self.i + 1] == b'\n') {
                return Err(err!(BadControlChar));
            }
            if basic && c == b'\\' {
                self.i += 2;
                continue;
            }
            if c == q && self.i + 2 <= self.str.len() && self.str.get(self.i + 1) == Some(&q) && self.str.get(self.i + 2) == Some(&q) {
                self.i += 3;
                // TOML allows up to two extra quotes hugging the close (e.g. `""""""`
                // → a string of `""`). Consume up to two trailing quotes.
                let mut extra = 0;
                while extra < 2 && self.i < self.str.len() && self.str[self.i] == q {
                    self.i += 1;
                    extra += 1;
                }
                return Ok(());
            }
            self.i += 1;
        }
        Err(err!(UnclosedString))
    }

    /// Scan a non-string scalar bareword (boolean or number) and classify it.
    fn lex_bareword(&mut self) -> Result<()> {
        let start = self.i;
        while self.i < self.str.len() && !is_value_terminator(self.str[self.i]) {
            self.i += 1;
        }
        let word = &self.str[start..self.i];
        if word.is_empty() {
            return Err(err!(BadValue));
        }
        if word == b"true" || word == b"false" {
            self.emit(Kind::Boolean, start, self.i);
        } else {
            self.emit(Kind::Number, start, self.i);
        }
        Ok(())
    }

    // ── Datetime matching ───────────────────────────────────────────────────
    // Returns the end index of an RFC-3339-derived date/time starting at `at`, or
    // `None` if the bytes there are not a datetime (then the bareword/number lexer
    // runs). Only structural shape is checked; range validity (month ≤ 12, etc.) is
    // the parser's job.

    fn digits_at(&self, at: usize, n: usize) -> bool {
        if at + n > self.str.len() {
            return false;
        }
        self.str[at..at + n].iter().all(|c| c.is_ascii_digit())
    }

    fn char_at(&self, at: usize, c: u8) -> bool {
        at < self.str.len() && self.str[at] == c
    }

    fn match_datetime(&self, at: usize) -> Option<usize> {
        // Full date: 4DIGIT - 2DIGIT - 2DIGIT
        if self.digits_at(at, 4) && self.char_at(at + 4, b'-') && self.digits_at(at + 5, 2) && self.char_at(at + 7, b'-') && self.digits_at(at + 8, 2) {
            let date_end = at + 10;
            // Time separator: `T`/`t`, or a space *only if* a time follows.
            let mut sep: Option<usize> = None;
            if self.char_at(date_end, b'T') || self.char_at(date_end, b't') {
                sep = Some(date_end + 1);
            } else if self.char_at(date_end, b' ') && self.match_time(date_end + 1).is_some() {
                sep = Some(date_end + 1);
            }
            if let Some(time_start) = sep {
                let Some(time_end) = self.match_time(time_start) else { return Some(date_end) }; // date-only fallback
                return Some(self.match_offset(time_end).unwrap_or(time_end));
            }
            return Some(date_end); // local date
        }
        // Local time: 2DIGIT : 2DIGIT : 2DIGIT [.frac]
        self.match_time(at)
    }

    /// Match HH:MM[:SS[.fraction]]. Seconds are required in TOML 1.0 but optional
    /// in 1.1. Returns the end index.
    fn match_time(&self, at: usize) -> Option<usize> {
        if !(self.digits_at(at, 2) && self.char_at(at + 2, b':') && self.digits_at(at + 3, 2)) {
            return None; // HH:MM
        }
        let mut end = at + 5;
        if self.char_at(end, b':') && self.digits_at(end + 1, 2) {
            end += 3; // :SS
            if self.char_at(end, b'.') {
                let mut f = end + 1;
                if !self.digits_at(f, 1) {
                    return None; // a dot needs ≥1 fractional digit
                }
                while self.digits_at(f, 1) {
                    f += 1;
                }
                end = f;
            }
        } else if self.version == Type::Toml1_0 {
            return None; // 1.0 requires seconds
        }
        Some(end)
    }

    /// Match a time offset (`Z`/`z` or ±HH:MM) at `at`, or `None` if none.
    fn match_offset(&self, at: usize) -> Option<usize> {
        if self.char_at(at, b'Z') || self.char_at(at, b'z') {
            return Some(at + 1);
        }
        if (self.char_at(at, b'+') || self.char_at(at, b'-')) && self.digits_at(at + 1, 2) && self.char_at(at + 3, b':') && self.digits_at(at + 4, 2) {
            return Some(at + 6);
        }
        None
    }
}

fn is_value_terminator(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' ||
        c == b'#' || c == b',' || c == b']' || c == b'}' ||
        // `=` ends an inline-table bare key (`{a=1}`); a scalar never contains
        // one, so this is safe in value position too.
        c == b'='
}

pub fn is_bare_key_char(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-'
}
