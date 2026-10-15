//! Port of kit/src/scanner.zig.
//!
//! A cursor over a parse's input whose position is a byte offset, so every
//! span it hands out is already the table's. Nothing here advances on a
//! miss. The scanner every kit has — the JavaScript kit's `Scanner`, Rust's
//! `fig::kit::scan::Scanner` — and the one `grammar.rs` runs over.

use super::span::Span;

#[derive(Debug, Clone, Copy)]
pub struct Scanner<'a> {
    pub src: &'a [u8],
    /// The cursor: a 0-based byte offset into `src`.
    pub pos: usize,
}

impl<'a> Scanner<'a> {
    pub fn init(src: &'a [u8]) -> Scanner<'a> {
        Scanner { src, pos: 0 }
    }

    pub fn eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// The byte `k` past the cursor (`0` is the one at it), or `None` past
    /// either end; `k` may be negative.
    pub fn byte(&self, k: isize) -> Option<u8> {
        let i = self.pos as isize + k;
        if i < 0 || i as usize >= self.src.len() {
            return None;
        }
        Some(self.src[i as usize])
    }

    /// Whether the input at the cursor begins with `s`.
    pub fn starts(&self, s: &[u8]) -> bool {
        self.pos <= self.src.len() && self.src[self.pos..].starts_with(s)
    }

    /// The literal `s` at the cursor: advances past it and returns its span.
    pub fn lit(&mut self, s: &[u8]) -> Option<Span> {
        if !self.starts(s) {
            return None;
        }
        let start = self.pos;
        self.pos += s.len();
        Some(Span::new(start, self.pos))
    }

    /// Bytes while `f` holds of them, possibly none: advances past them and
    /// returns their span.
    pub fn take_while(&mut self, f: impl Fn(u8) -> bool) -> Span {
        let start = self.pos;
        while self.pos < self.src.len() && f(self.src[self.pos]) {
            self.pos += 1;
        }
        Span::new(start, self.pos)
    }

    /// `take_while`, at least one byte.
    pub fn take_while1(&mut self, f: impl Fn(u8) -> bool) -> Option<Span> {
        let s = self.take_while(f);
        if s.is_empty() { None } else { Some(s) }
    }

    /// The offset of the next `s` at or after the cursor.
    pub fn find(&self, s: &[u8]) -> Option<usize> {
        if self.pos > self.src.len() {
            return None;
        }
        let i = index_of(&self.src[self.pos..], s)?;
        Some(self.pos + i)
    }

    /// Move the cursor `k` bytes forward.
    pub fn advance(&mut self, k: usize) {
        self.pos += k;
    }

    pub fn is_hs(c: u8) -> bool {
        c == b' ' || c == b'\t'
    }

    /// Skip spaces and tabs; the cursor after them.
    pub fn hs(&mut self) -> usize {
        self.take_while(Self::is_hs);
        self.pos
    }

    /// Whether no newline lies in `[a, b)`: what "on the same line" means to
    /// a trailing comment.
    pub fn same_line(&self, a: usize, b: usize) -> bool {
        let end = b.min(self.src.len());
        if a >= end {
            return true;
        }
        !self.src[a..end].contains(&b'\n')
    }
}

/// `std.mem.indexOf`: the first offset of `needle` in `haystack` (0 for an
/// empty needle).
fn index_of(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scanner_moves_only_on_a_match() {
        let mut sc = Scanner::init(b"key = v # c\nx");
        assert_eq!(None, sc.byte(-1));
        assert!(sc.take_while1(|c| c.is_ascii_alphabetic()).is_some());
        assert_eq!(4, sc.hs());
        assert_eq!(None, sc.lit(b":"));
        assert!(sc.lit(b"=").is_some());
        assert_eq!(Some(8), sc.find(b"#"));
        assert!(sc.same_line(0, 11));
        assert!(!sc.same_line(0, 12));
    }
}
