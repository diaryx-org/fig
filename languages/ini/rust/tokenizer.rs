//! Port of languages/ini/src/tokenizer.zig.
//!
//! INI tokenizer. Turns an INI `&[u8]` into a list of Tokens.
//!
//! INI is line-oriented and, like TOML, context-sensitive: the same byte means
//! different things before vs. after `=` on a line (`in_value`), reset at
//! every newline — no flow-nesting stack is needed (INI has no arrays/inline
//! tables to nest into).
//!
//! Deliberately narrow scope vs. the many incompatible things "INI" means in
//! the wild: `=` is the only separator (`.properties`'s `:`/bare-whitespace
//! forms live in that format instead — see `lang/properties/`), `;` and
//! `#` both start comments, and — the one rule that most affects real-world
//! files — **a comment must be the first non-whitespace content on its line**.
//! There is no same-line trailing `# ...` after a value: many real INI files
//! legitimately contain `;`/`#` inside an unquoted value (`path=C:\a;b`), so
//! treating those as unterminated comments would corrupt them. A value is
//! simply "the rest of the line, trimmed."

use super::Type;
use fig_kit::err;
use fig_kit::span::Span;
use fig_kit::token::{self, TokenKind};
use fig_kit::Result;

pub type Token = token::Token<Kind>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// [
    OpenBracket,
    /// ]
    CloseBracket,
    /// =
    Equals,
    Newline,
    EndOfFile,

    // variable-length
    /// A bare run of text: a section name, a key, or (in value position) the
    /// rest of the line. Interpretation is entirely positional — the parser
    /// decides which of the three this is from where it appears.
    Text,
    /// `;`/`#` line comment; span covers the content only (leader excluded).
    Comment,
}

impl TokenKind for Kind {
    fn len(self) -> Option<usize> {
        match self {
            Kind::EndOfFile => Some(0),
            Kind::OpenBracket | Kind::CloseBracket | Kind::Equals => Some(1),
            _ => None,
        }
    }
}

/// `TokenizeError`: the names `tokenize` returns.
pub const TOKENIZE_ERRORS: &[&str] =
    &["UnexpectedCarriageReturn", "UnclosedSection", "MissingEquals", "TrailingContent", "OutOfMemory"];

#[derive(Debug)]
pub struct Tokenizer<'a> {
    pub tokens: Vec<Token>,
    pub str: &'a [u8],
    pub version: Type,
    pub i: usize,
    /// True between `=` and end-of-line: value position. Reset at every newline.
    pub in_value: bool,
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
                        self.lex_value_text();
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

    fn at_line_end(&self, at: usize) -> bool {
        at >= self.str.len() || self.str[at] == b'\n' || self.str[at] == b'\r'
    }

    // ── Key-context line start ──────────────────────────────────────────────

    fn lex_key_context(&mut self) -> Result<()> {
        match self.str[self.i] {
            b';' | b'#' => {
                self.lex_comment();
                Ok(())
            }
            b'[' => self.lex_section_header(),
            _ => self.lex_key(),
        }
    }

    fn lex_comment(&mut self) {
        self.i += 1; // leader
        let start = self.i;
        while !self.at_line_end(self.i) {
            self.i += 1;
        }
        self.emit(Kind::Comment, start, self.i);
    }

    /// `[name]` — name runs to the next `]` (not itself a line boundary);
    /// nothing but whitespace may follow before the newline/EOF.
    fn lex_section_header(&mut self) -> Result<()> {
        self.emit(Kind::OpenBracket, self.i, self.i + 1);
        self.i += 1;
        let start = self.i;
        while self.i < self.str.len() && self.str[self.i] != b']' {
            if self.str[self.i] == b'\n' || self.str[self.i] == b'\r' {
                return Err(err!(UnclosedSection));
            }
            self.i += 1;
        }
        if self.i >= self.str.len() {
            return Err(err!(UnclosedSection));
        }
        self.emit_trimmed(start, self.i);
        self.emit(Kind::CloseBracket, self.i, self.i + 1);
        self.i += 1;
        // Only whitespace may trail a header on its own line.
        while self.i < self.str.len() && (self.str[self.i] == b' ' || self.str[self.i] == b'\t') {
            self.i += 1;
        }
        if !self.at_line_end(self.i) {
            return Err(err!(TrailingContent));
        }
        Ok(())
    }

    /// A key runs to `=` (required) or the line end (an error: `MissingEquals`).
    fn lex_key(&mut self) -> Result<()> {
        let start = self.i;
        while self.i < self.str.len() && self.str[self.i] != b'=' && self.str[self.i] != b'\n' && self.str[self.i] != b'\r' {
            self.i += 1;
        }
        if self.i >= self.str.len() || self.str[self.i] != b'=' {
            return Err(err!(MissingEquals));
        }
        self.emit_trimmed(start, self.i);
        self.emit(Kind::Equals, self.i, self.i + 1);
        self.i += 1;
        self.in_value = true;
        Ok(())
    }

    // ── Value context ───────────────────────────────────────────────────────

    /// Everything to end-of-line is the value, trimmed of surrounding
    /// whitespace. No escaping, no comment recognition — see the module doc.
    fn lex_value_text(&mut self) {
        let start = self.i;
        while !self.at_line_end(self.i) {
            self.i += 1;
        }
        self.emit_trimmed(start, self.i);
    }

    /// A `Text` token over `[start, end)` with the surrounding spaces and tabs
    /// left out. Trimmed from the front first and the back no further than
    /// that, so a run that is ALL whitespace — `[ ]`'s name — is an empty span
    /// at its end rather than an inverted one (start past end), which the
    /// parser read as garbage and the printer once crashed on.
    fn emit_trimmed(&mut self, start: usize, end: usize) {
        let s = trim_start(self.str, start);
        self.emit(Kind::Text, s, trim_end(self.str, s, end));
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
