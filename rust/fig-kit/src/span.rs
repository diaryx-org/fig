//! Port of kit/src/span.zig: a helper for selecting parts of a slice.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    pub const fn len(self) -> usize {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.end == self.start
    }

    /// Take the span out of a slice.
    pub fn of<T>(self, slice: &[T]) -> &[T] {
        &slice[self.start..self.end]
    }
}
