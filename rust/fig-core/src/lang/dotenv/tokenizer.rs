//! Port of languages/dotenv/src/tokenizer.zig.
//!
//! dotenv (.env) tokenizer. Turns a dotenv `&[u8]` into a list of Tokens.
//!
//! Like INI, line-oriented and context-sensitive (`in_value`, reset at every
//! newline) — but a deliberately DIFFERENT dialect from INI, not a re-skin of
//! it, in every place real `.env` tooling (the JS/Ruby/Python `dotenv`
//! packages, which have mostly converged on one convention) actually differs:
//!   * keys are strict bash identifiers (`[A-Za-z_][A-Za-z0-9_]*`) — a `.env`
//!     is meant to become shell/process environment variables, unlike INI's
//!     free-form keys;
//!   * an optional `export ` prefix (bash-export style) is accepted and
//!     discarded — handled by the PARSER as "the last key-shaped token before
//!     `=` wins, iff the one before it is literally `export`", not here;
//!   * `"..."`/`'...'` quoting is real: double-quoted values decode `\n \t \r
//!     \\ \"` escapes AND may embed a literal newline (span multiple physical
//!     lines); single-quoted values are fully raw (no escapes at all, not even
//!     `\\` — same as a shell) but may ALSO embed a literal newline. Neither
//!     exists in INI.
//!   * comments: a full-line `#` (first non-whitespace on the line) always
//!     starts one, matching INI; but so does a `#` that trails an UNQUOTED
//!     value when at least one whitespace character precedes it (`FOO=bar
//!     #note` → value `bar`), or one that follows a closing quote at all
//!     (whitespace before it is optional there — the quote is already an
//!     unambiguous terminator). A `#` with NO preceding whitespace inside an
//!     unquoted value is literal (`FOO=#bar` → value `#bar`) — this exact
//!     whitespace-gated rule is what real dotenv parsers do, and it's the one
//!     place dotenv recognizes a trailing comment at all where INI never does.
//!
//! `MissingEquals` — unlike INI's tokenizer, where it's tokenizer-level and so
//! not recoverable — is a PARSER error here (see `parser.rs`): key-scanning is
//! just "an identifier run," so the tokenizer never needs to look ahead for
//! `=` itself, which makes a missing `=` recoverable under `parse_collecting`
//! the same way `DuplicateKey` is.

use super::Type;
use crate::err;
use crate::kit::span::Span;
use crate::kit::token::{self, TokenKind};
use crate::Result;

pub type Token = token::Token<Kind>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// =
    Equals,
    Newline,
    EndOfFile,

    // variable-length
    /// `[A-Za-z_][A-Za-z0-9_]*`
    Key,
    /// `"..."`, span includes both quotes
    DoubleQuoted,
    /// `'...'`, span includes both quotes
    SingleQuoted,
    /// raw, trimmed value text with no quoting
    Unquoted,
    /// `#`/trailing comment; span covers the content only (leader excluded)
    Comment,
}

impl TokenKind for Kind {
    fn len(self) -> Option<usize> {
        match self {
            Kind::EndOfFile => Some(0),
            Kind::Equals => Some(1),
            _ => None,
        }
    }
}

/// `TokenizeError`: the names `tokenize` returns.
pub const TOKENIZE_ERRORS: &[&str] =
    &["UnexpectedCarriageReturn", "UnclosedString", "UnexpectedChar", "TrailingContent", "OutOfMemory"];

#[derive(Debug)]
pub struct Tokenizer<'a> {
    pub tokens: Vec<Token>,
    pub str: &'a [u8],
    pub version: Type,
    pub i: usize,
    /// True between `=` and end-of-line: value position. Reset at every newline.
    pub in_value: bool,
}

fn is_ident_start(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphabetic()
}
fn is_ident_char(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}

impl<'a> Tokenizer<'a> {
    pub fn new(str: &'a [u8], version: Type) -> Tokenizer<'a> {
        Tokenizer { tokens: Vec::new(), str, version, i: 0, in_value: false }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>> {
        if self.str.starts_with(b"\xEF\xBB\xBF") {
            self.i = 3; // BOM
        }

        while self.i < self.str.len() {
            let c = self.str[self.i];
            match c {
                b'\n' => {
                    self.emit(Kind::Newline, self.i, self.i + 1);
                    self.i += 1;
                    self.in_value = false;
                }
                b'\r' => {
                    if self.i + 1 < self.str.len() && self.str[self.i + 1] == b'\n' {
                        self.emit(Kind::Newline, self.i, self.i + 2);
                        self.i += 2;
                        self.in_value = false;
                    } else {
                        return Err(err!(UnexpectedCarriageReturn));
                    }
                }
                b' ' | b'\t' => self.i += 1, // insignificant; never tokenized
                _ => {
                    if self.in_value {
                        self.lex_value_context()?;
                    } else {
                        self.lex_key_context()?;
                    }
                }
            }
        }

        self.emit(Kind::EndOfFile, self.str.len(), self.str.len());
        Ok(std::mem::take(&mut self.tokens))
    }

    fn emit(&mut self, kind: Kind, start: usize, end: usize) {
        self.tokens.push(Token::new(kind, Span::new(start, end)));
    }

    // ── Key context ─────────────────────────────────────────────────────────

    fn lex_key_context(&mut self) -> Result<()> {
        match self.str[self.i] {
            b'#' => self.lex_comment(),
            b'=' => {
                self.emit(Kind::Equals, self.i, self.i + 1);
                self.i += 1;
                self.in_value = true;
            }
            c => {
                if is_ident_start(c) {
                    self.lex_key();
                } else {
                    return Err(err!(UnexpectedChar));
                }
            }
        }
        Ok(())
    }

    fn lex_key(&mut self) {
        let start = self.i;
        self.i += 1; // is_ident_start already checked
        while self.i < self.str.len() && is_ident_char(self.str[self.i]) {
            self.i += 1;
        }
        self.emit(Kind::Key, start, self.i);
    }

    /// Full-line comment (only reachable at the start of a logical line, in
    /// key context). Content only — leader excluded — like INI's.
    fn lex_comment(&mut self) {
        self.i += 1; // '#'
        let start = self.i;
        while !self.at_line_end(self.i) {
            self.i += 1;
        }
        self.emit(Kind::Comment, start, self.i);
    }

    fn at_line_end(&self, at: usize) -> bool {
        at >= self.str.len() || self.str[at] == b'\n' || self.str[at] == b'\r'
    }

    // ── Value context ───────────────────────────────────────────────────────

    fn lex_value_context(&mut self) -> Result<()> {
        match self.str[self.i] {
            b'"' => {
                self.lex_quoted(b'"', Kind::DoubleQuoted)?;
                self.after_quoted_value()
            }
            b'\'' => {
                self.lex_quoted(b'\'', Kind::SingleQuoted)?;
                self.after_quoted_value()
            }
            _ => {
                self.lex_unquoted();
                Ok(())
            }
        }
    }

    /// Scan a `"`/`'`-delimited value, which may embed a literal newline (a
    /// bare `\r` is still only valid as part of `\r\n`, checked the same as
    /// the top-level scanner). Double-quoted honors `\`-escapes structurally
    /// (skips the escaped byte so `\"` doesn't close early); single-quoted
    /// has none — a `\` is just a literal byte, matching a shell's
    /// single-quote semantics (so it also can never contain a `'`).
    fn lex_quoted(&mut self, q: u8, kind: Kind) -> Result<()> {
        let start = self.i;
        self.i += 1; // opening quote
        let escapes = q == b'"';
        while self.i < self.str.len() {
            let c = self.str[self.i];
            if c == b'\r' {
                if self.i + 1 < self.str.len() && self.str[self.i + 1] == b'\n' {
                    self.i += 2;
                    continue;
                }
                return Err(err!(UnexpectedCarriageReturn));
            }
            if escapes && c == b'\\' {
                if self.i + 1 >= self.str.len() {
                    return Err(err!(UnclosedString));
                }
                self.i += 2;
                continue;
            }
            if c == q {
                self.i += 1;
                self.emit(kind, start, self.i);
                return Ok(());
            }
            self.i += 1;
        }
        Err(err!(UnclosedString))
    }

    /// After a closing quote: optional whitespace, then a `#` comment or the
    /// line end — anything else is `TrailingContent` (mirrors INI's rule
    /// after a `[section]` header). Unlike an unquoted value's comment (see
    /// the module doc), no preceding whitespace is required here — the quote
    /// itself is already an unambiguous terminator.
    fn after_quoted_value(&mut self) -> Result<()> {
        while self.i < self.str.len() && (self.str[self.i] == b' ' || self.str[self.i] == b'\t') {
            self.i += 1;
        }
        if self.i < self.str.len() && self.str[self.i] == b'#' {
            self.lex_comment();
            return Ok(());
        }
        if !self.at_line_end(self.i) {
            return Err(err!(TrailingContent));
        }
        Ok(())
    }

    /// Raw value text with no quoting: runs to end of line, UNLESS a `#`
    /// preceded by whitespace starts a trailing comment (see the module doc)
    /// — that comment is lexed immediately here so the outer loop doesn't
    /// re-enter value context on it. Trimmed of surrounding whitespace.
    fn lex_unquoted(&mut self) {
        let start = self.i;
        let mut end = start;
        // Whitespace between `=` and the value's first byte was already
        // consumed (silently) by the top-level loop before dispatching here,
        // so a `#` sitting right at `start` needs this to know whitespace
        // preceded it too (`FOO=   #note` is a comment; `FOO=#note` is the
        // literal value `#note`).
        let mut saw_space = self.i > 0 && (self.str[self.i - 1] == b' ' || self.str[self.i - 1] == b'\t');
        while self.i < self.str.len() {
            let c = self.str[self.i];
            if c == b'\n' || c == b'\r' {
                break;
            }
            if c == b'#' && saw_space {
                break;
            }
            saw_space = c == b' ' || c == b'\t';
            self.i += 1;
            end = self.i;
        }
        self.emit(Kind::Unquoted, trim_start(self.str, start), trim_end(self.str, start, end));
        if self.i < self.str.len() && self.str[self.i] == b'#' {
            self.lex_comment();
        }
    }
}

fn trim_start(str: &[u8], start: usize) -> usize {
    let mut s = start;
    while s < str.len() && (str[s] == b' ' || str[s] == b'\t') {
        s += 1;
    }
    s
}

fn trim_end(str: &[u8], start: usize, end: usize) -> usize {
    let mut e = end;
    while e > start && (str[e - 1] == b' ' || str[e - 1] == b'\t') {
        e -= 1;
    }
    e
}
