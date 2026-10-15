//! Port of kit/src/token.zig: a token of some kind and its span.

use super::span::Span;

/// What a token's kind must say: its fixed length, or `None` for a kind
/// whose length varies.
pub trait TokenKind: Copy {
    fn len(self) -> Option<usize>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<K> {
    pub kind: K,
    pub span: Span,
}

impl<K: TokenKind> Token<K> {
    pub fn new(kind: K, span: Span) -> Self {
        Token { kind, span }
    }

    pub fn source<'a>(&self, bytes: &'a [u8]) -> &'a [u8] {
        &bytes[self.span.start..self.span.end]
    }

    /// A token of a fixed-length kind starting at `start`.
    pub fn fixed(kind: K, start: usize) -> Self {
        let len = kind.len().expect("Token::fixed on a kind with no fixed length");
        Token { kind, span: Span::new(start, start + len) }
    }
}
