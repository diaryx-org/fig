//! A cursor over a parse's input whose position is a byte offset, so every
//! span it hands out is already the table's; and a cursor over a token list,
//! for a parser whose tokenizer runs first. Nothing here advances on a miss.
//!
//! The JavaScript kit's `Scanner` and `Cursor`, in Rust. Where the
//! JavaScript scanner matches a regular expression, this one takes a byte
//! predicate: [`Scanner::take_while`], [`Scanner::take_while1`].
//!
//! ```
//! use fig::Span;
//! use fig::kit::scan::Scanner;
//!
//! let mut sc = Scanner::new(b"key = value\n");
//! let key = sc.take_while1(|b| b.is_ascii_alphanumeric()).unwrap();
//! sc.hs();
//! assert!(sc.lit("=").is_some());
//! sc.hs();
//! assert_eq!(sc.slice(key.start, key.end), "key");
//! assert_eq!(sc.pos, 6);
//! ```

use std::borrow::Cow;

use crate::Span;
use crate::language::LanguageError;

/// A byte cursor over `bytes`. See the [module](self).
#[derive(Clone, Debug)]
pub struct Scanner<'a> {
    bytes: &'a [u8],
    /// The cursor: a 0-based byte offset into the input.
    pub pos: usize,
}

impl<'a> Scanner<'a> {
    /// A scanner at the start of `bytes`.
    pub fn new(bytes: &'a [u8]) -> Self {
        Scanner { bytes, pos: 0 }
    }

    /// The whole input.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The input's length in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    /// The byte `k` past the cursor (`0` is the one at it), or `None` past
    /// either end; `k` may be negative.
    pub fn byte(&self, k: isize) -> Option<u8> {
        let i = self.pos.checked_add_signed(k)?;
        self.bytes.get(i).copied()
    }

    /// Whether the input at the cursor begins with `s`.
    pub fn starts(&self, s: &str) -> bool {
        self.bytes[self.pos.min(self.bytes.len())..].starts_with(s.as_bytes())
    }

    /// The text of the bytes `[s, e)`; a byte sequence that is not UTF-8 is
    /// replaced.
    pub fn slice(&self, s: usize, e: usize) -> Cow<'a, str> {
        String::from_utf8_lossy(&self.bytes[s..e])
    }

    /// The literal `s` at the cursor: advances past it and returns its
    /// span, or `None`.
    pub fn lit(&mut self, s: &str) -> Option<Span> {
        if !self.starts(s) {
            return None;
        }
        let start = self.pos;
        self.pos += s.len();
        Some(Span::from(start..self.pos))
    }

    /// Bytes while `f` holds of them, possibly none: advances past them and
    /// returns their span.
    pub fn take_while(&mut self, f: impl Fn(u8) -> bool) -> Span {
        let start = self.pos;
        while self.pos < self.bytes.len() && f(self.bytes[self.pos]) {
            self.pos += 1;
        }
        Span::from(start..self.pos)
    }

    /// [`take_while`](Self::take_while), at least one byte, or `None`.
    pub fn take_while1(&mut self, f: impl Fn(u8) -> bool) -> Option<Span> {
        let s = self.take_while(f);
        if s.start == s.end { None } else { Some(s) }
    }

    /// The offset of the next `s` at or after the cursor, or `None`.
    pub fn find(&self, s: &str) -> Option<usize> {
        let needle = s.as_bytes();
        if needle.is_empty() {
            return Some(self.pos);
        }
        let hay = self.bytes.get(self.pos..)?;
        hay.windows(needle.len()).position(|w| w == needle).map(|i| self.pos + i)
    }

    /// Move the cursor `k` bytes forward.
    pub fn advance(&mut self, k: usize) -> usize {
        self.pos += k;
        self.pos
    }

    /// Skip spaces and tabs; the cursor after them.
    pub fn hs(&mut self) -> usize {
        self.take_while(|b| b == b' ' || b == b'\t');
        self.pos
    }

    /// Whether no newline lies in `[a, b)`: what "on the same line" means to
    /// a trailing comment.
    pub fn same_line(&self, a: usize, b: usize) -> bool {
        let b = b.min(self.bytes.len());
        a >= b || !self.bytes[a..b].contains(&b'\n')
    }

    /// The refusal `message`, at the cursor: `return Err(sc.fail(…))`.
    pub fn fail(&self, message: impl Into<String>) -> LanguageError {
        LanguageError::at(message, self.pos)
    }

    /// The refusal `message`, at `at`.
    pub fn fail_at(&self, message: impl Into<String>, at: usize) -> LanguageError {
        LanguageError::at(message, at)
    }
}

/// One token: its kind and its byte span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token<K> {
    pub kind: K,
    pub span: Span,
}

/// A cursor over a token list, the last an end-of-input token it never
/// moves past.
#[derive(Clone, Debug)]
pub struct Cursor<'t, K> {
    tokens: &'t [Token<K>],
    pub pos: usize,
}

impl<'t, K: Copy + PartialEq> Cursor<'t, K> {
    /// A cursor at the first of `tokens`, which must not be empty.
    pub fn new(tokens: &'t [Token<K>]) -> Self {
        assert!(!tokens.is_empty(), "a cursor needs an end-of-input token");
        Cursor { tokens, pos: 0 }
    }

    /// The token in hand.
    pub fn peek(&self) -> Token<K> {
        self.tokens[self.pos]
    }

    /// The token `k` past the one in hand, or the last.
    pub fn ahead(&self, k: usize) -> Token<K> {
        self.tokens[(self.pos + k).min(self.tokens.len() - 1)]
    }

    /// The token in hand, moving to the next unless it is the last.
    pub fn advance(&mut self) -> Token<K> {
        let t = self.tokens[self.pos];
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    /// Whether the token in hand is of `kind`.
    pub fn at(&self, kind: K) -> bool {
        self.tokens[self.pos].kind == kind
    }

    /// The token in hand when it is of `kind`, moving past it.
    pub fn take(&mut self, kind: K) -> Option<Token<K>> {
        if self.at(kind) { Some(self.advance()) } else { None }
    }

    /// The refusal `message`, at the start of the token in hand.
    pub fn fail(&self, message: impl Into<String>) -> LanguageError {
        LanguageError::at(message, self.tokens[self.pos].span.start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner() {
        let mut sc = Scanner::new("é # c\nx".as_bytes());
        assert_eq!(sc.byte(-1), None);
        assert_eq!(sc.byte(0), Some(0xc3));
        assert_eq!(sc.find("#"), Some(3));
        assert!(sc.same_line(0, 6));
        assert!(!sc.same_line(0, 7));
        assert_eq!(sc.take_while1(|b| b >= 0x80), Some(Span::from(0..2)));
        assert_eq!(sc.lit("#"), None);
        assert_eq!(sc.fail("x").byte_offset, Some(2));
    }

    #[test]
    fn cursor() {
        let toks = [
            Token { kind: 'a', span: Span::from(0..1) },
            Token { kind: '$', span: Span::from(1..1) },
        ];
        let mut c = Cursor::new(&toks);
        assert!(c.take('b').is_none());
        assert_eq!(c.take('a').unwrap().span, Span::from(0..1));
        c.advance();
        assert!(c.at('$'));
        assert_eq!(c.ahead(3).kind, '$');
    }
}
