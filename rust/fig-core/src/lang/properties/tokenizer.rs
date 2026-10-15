//! Port of languages/properties/src/tokenizer.zig.
//!
//! Java `.properties` tokenizer. Turns a `.properties` `&[u8]` into a list of
//! Tokens.
//!
//! The richest — and most permissive — of fig's three flat key/value
//! formats, matching `java.util.Properties.load`'s actual documented
//! behavior rather than re-skinning INI or dotenv:
//!   * the key/value separator is the FIRST unescaped `=`, `:`, or plain
//!     whitespace — `a=b`, `a:b`, and `a b` are all the same statement. A key
//!     with no separator at all (`flag` alone on a line) is legal: value `""`.
//!   * both key AND value recognize backslash escapes: `\t \n \r \f \\`, a
//!     `\uXXXX` unicode escape, `\` followed by anything else yields that
//!     character literally (`\:`/`\=`/`\#`/`\ ` — the standard way to embed a
//!     separator-looking or leading-whitespace-looking byte), and — the
//!     hallmark feature neither INI nor dotenv has — a `\` immediately before
//!     a line ending is a LINE CONTINUATION: the backslash, the newline, and
//!     the next line's leading whitespace all vanish, joining it onto the
//!     current logical line. Unlike Java's stricter unrecognized-escape
//!     handling in some other contexts, an unrecognized `\c` here is never an
//!     error (matching `Properties.load`) — only a malformed `\uXXXX` is.
//!   * comments start with `#` OR `!` (both accepted, matching real
//!     `.properties` tooling) as the first non-whitespace byte of a physical
//!     line — never continued, unlike a key/value line.
//!   * source is required to be valid UTF-8 (fig's usual policy), with
//!     `\uXXXX` still honored for interop with the classic ISO-8859-1-era
//!     convention — NOT the legacy raw-Latin-1-bytes reading some very old
//!     tooling still defaults to.
//!
//! Escape/continuation scanning (`scan_escaped`) is shared between key and
//! value scanning — the only difference is which bytes terminate a KEY scan
//! (`=`/`:`/whitespace); a value scan never terminates early, running to the
//! true (non-continued) end of the logical line.

use super::Type;
use crate::err;
use crate::kit::span::Span;
use crate::kit::token::{self, TokenKind};
use crate::Result;

pub type Token = token::Token<Kind>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Newline,
    EndOfFile,
    /// escape-aware raw key span (decoded by the parser)
    Key,
    /// escape-aware raw value span (decoded by the parser); zero-width,
    /// where a value would begin, when the line had no separator or nothing
    /// follows one
    Value,
    /// `#`/`!` line comment; span covers the content only (leader excluded)
    Comment,
}

impl TokenKind for Kind {
    fn len(self) -> Option<usize> {
        match self {
            Kind::EndOfFile => Some(0),
            _ => None,
        }
    }
}

/// `TokenizeError`: the names `tokenize` returns. `UnclosedEscape` is a `\`
/// as the very last byte of the file (not part of a `\<newline>`
/// continuation) — Java's own reader is more lenient here; fig treats it as
/// a hard error rather than silently guessing what was meant.
pub const TOKENIZE_ERRORS: &[&str] = &["UnexpectedCarriageReturn", "UnclosedEscape", "OutOfMemory"];

#[derive(Debug)]
pub struct Tokenizer<'a> {
    pub tokens: Vec<Token>,
    pub str: &'a [u8],
    pub version: Type,
    pub i: usize,
}

fn is_inline_ws(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == 0x0c // space, tab, form feed
}

fn is_separator(c: u8) -> bool {
    c == b'=' || c == b':' || is_inline_ws(c)
}

impl<'a> Tokenizer<'a> {
    pub fn new(str: &'a [u8], version: Type) -> Tokenizer<'a> {
        Tokenizer { tokens: Vec::new(), str, version, i: 0 }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>> {
        if self.str.starts_with(b"\xEF\xBB\xBF") {
            self.i = 3; // BOM
        }

        while self.i < self.str.len() {
            // Leading whitespace on a fresh logical line is insignificant
            // (and a whitespace-only line collapses to the `\n` case below —
            // a "blank line" needs no separate detection).
            while self.i < self.str.len() && is_inline_ws(self.str[self.i]) {
                self.i += 1;
            }
            if self.i >= self.str.len() {
                break;
            }
            match self.str[self.i] {
                b'\n' => {
                    self.emit(Kind::Newline, self.i, self.i + 1);
                    self.i += 1;
                }
                b'\r' => {
                    if self.i + 1 < self.str.len() && self.str[self.i + 1] == b'\n' {
                        self.emit(Kind::Newline, self.i, self.i + 2);
                        self.i += 2;
                    } else {
                        return Err(err!(UnexpectedCarriageReturn));
                    }
                }
                b'#' | b'!' => self.lex_comment(),
                _ => self.lex_key_value_line()?,
            }
        }

        self.emit(Kind::EndOfFile, self.str.len(), self.str.len());
        Ok(std::mem::take(&mut self.tokens))
    }

    fn emit(&mut self, kind: Kind, start: usize, end: usize) {
        self.tokens.push(Token::new(kind, Span::new(start, end)));
    }

    fn lex_comment(&mut self) {
        self.i += 1; // leader
        let start = self.i;
        while self.i < self.str.len() && self.str[self.i] != b'\n' && self.str[self.i] != b'\r' {
            self.i += 1;
        }
        self.emit(Kind::Comment, start, self.i);
    }

    /// `key<sep>value`. `<sep>`: whitespace*, then optionally ONE `=`/`:`,
    /// then whitespace* — any of `a=b`, `a:b`, `a b`, `a = b` are equivalent.
    /// No separator at all (scan ran straight to end-of-line) leaves the
    /// whole line as the key, with the value absent (empty).
    fn lex_key_value_line(&mut self) -> Result<()> {
        let key_start = self.i;
        self.scan_escaped(true)?;
        self.emit(Kind::Key, key_start, self.i);

        while self.i < self.str.len() && is_inline_ws(self.str[self.i]) {
            self.i += 1;
        }
        if self.i < self.str.len() && (self.str[self.i] == b'=' || self.str[self.i] == b':') {
            self.i += 1;
            while self.i < self.str.len() && is_inline_ws(self.str[self.i]) {
                self.i += 1;
            }
        }
        if self.i >= self.str.len() || self.str[self.i] == b'\n' || self.str[self.i] == b'\r' {
            // Nothing follows: an empty value, spanning nothing where a value
            // would begin — after the separator and its whitespace, not at
            // the key's end — so that an editor writing a value into that
            // slot keeps the separator the line already has. A bare key with
            // no separator at all puts it at the key's end, and the editor
            // writes the separator with the value there.
            self.emit(Kind::Value, self.i, self.i);
            return Ok(());
        }

        let value_start = self.i;
        self.scan_escaped(false)?;
        self.emit(Kind::Value, value_start, self.i);
        Ok(())
    }

    /// Advance `self.i` across an escape-aware run: a backslash always
    /// protects the byte after it (never checked against `key_mode`'s
    /// terminator set, and never itself a stopping point), EXCEPT a backslash
    /// immediately before a line ending, which is a continuation — consumed
    /// together with the next line's leading whitespace, transparently
    /// extending the scan onto it. Stops (without consuming) at the first
    /// unescaped byte `is_separator` accepts when `key_mode`, or at the true
    /// end of the logical line/file otherwise.
    fn scan_escaped(&mut self, key_mode: bool) -> Result<()> {
        while self.i < self.str.len() {
            let c = self.str[self.i];
            if c == b'\n' || c == b'\r' {
                return Ok(());
            }
            if c == b'\\' {
                if self.i + 1 < self.str.len() && self.str[self.i + 1] == b'\n' {
                    self.i += 2;
                    self.skip_continuation_ws();
                    continue;
                }
                if self.i + 2 < self.str.len() && self.str[self.i + 1] == b'\r' && self.str[self.i + 2] == b'\n' {
                    self.i += 3;
                    self.skip_continuation_ws();
                    continue;
                }
                if self.i + 1 >= self.str.len() {
                    return Err(err!(UnclosedEscape));
                }
                self.i += 2; // backslash + escaped byte, protected as one unit
                continue;
            }
            if key_mode && is_separator(c) {
                return Ok(());
            }
            self.i += 1;
        }
        Ok(())
    }

    fn skip_continuation_ws(&mut self) {
        while self.i < self.str.len() && (self.str[self.i] == b' ' || self.str[self.i] == b'\t') {
            self.i += 1;
        }
    }
}
