//! A cursor over a parse's input whose position is a byte offset, so every
//! span it hands out is already the table's. Nothing here advances on a
//! miss. The scanner every kit has — the JavaScript kit's `Scanner`, Rust's
//! `fig::kit::scan::Scanner` — and the one `grammar.zig` runs over.

const Scanner = @This();
const std = @import("std");
const Span = @import("span.zig");

src: []const u8,
/// The cursor: a 0-based byte offset into `src`.
pos: usize = 0,

pub fn init(src: []const u8) Scanner {
    return .{ .src = src };
}

pub fn eof(self: *const Scanner) bool {
    return self.pos >= self.src.len;
}

/// The byte `k` past the cursor (`0` is the one at it), or null past
/// either end; `k` may be negative.
pub fn byte(self: *const Scanner, k: isize) ?u8 {
    const i = @as(isize, @intCast(self.pos)) + k;
    if (i < 0 or i >= self.src.len) return null;
    return self.src[@intCast(i)];
}

/// Whether the input at the cursor begins with `s`.
pub fn starts(self: *const Scanner, s: []const u8) bool {
    return self.pos <= self.src.len and std.mem.startsWith(u8, self.src[self.pos..], s);
}

/// The literal `s` at the cursor: advances past it and returns its span.
pub fn lit(self: *Scanner, s: []const u8) ?Span {
    if (!self.starts(s)) return null;
    const start = self.pos;
    self.pos += s.len;
    return Span.init(start, self.pos);
}

/// Bytes while `f` holds of them, possibly none: advances past them and
/// returns their span.
pub fn takeWhile(self: *Scanner, comptime f: fn (u8) bool) Span {
    const start = self.pos;
    while (self.pos < self.src.len and f(self.src[self.pos])) self.pos += 1;
    return Span.init(start, self.pos);
}

/// `takeWhile`, at least one byte.
pub fn takeWhile1(self: *Scanner, comptime f: fn (u8) bool) ?Span {
    const s = self.takeWhile(f);
    return if (s.len() == 0) null else s;
}

/// The offset of the next `s` at or after the cursor.
pub fn find(self: *const Scanner, s: []const u8) ?usize {
    if (self.pos > self.src.len) return null;
    const i = std.mem.indexOf(u8, self.src[self.pos..], s) orelse return null;
    return self.pos + i;
}

/// Move the cursor `k` bytes forward.
pub fn advance(self: *Scanner, k: usize) void {
    self.pos += k;
}

pub fn isHs(c: u8) bool {
    return c == ' ' or c == '\t';
}

/// Skip spaces and tabs; the cursor after them.
pub fn hs(self: *Scanner) usize {
    _ = self.takeWhile(isHs);
    return self.pos;
}

/// Whether no newline lies in `[a, b)`: what "on the same line" means to a
/// trailing comment.
pub fn sameLine(self: *const Scanner, a: usize, b: usize) bool {
    const end = @min(b, self.src.len);
    if (a >= end) return true;
    return std.mem.indexOfScalar(u8, self.src[a..end], '\n') == null;
}

test "the scanner moves only on a match" {
    const t = std.testing;
    var sc = init("key = v # c\nx");
    try t.expectEqual(@as(?u8, null), sc.byte(-1));
    try t.expect(sc.takeWhile1(std.ascii.isAlphabetic) != null);
    try t.expectEqual(@as(usize, 4), sc.hs());
    try t.expectEqual(@as(?Span, null), sc.lit(":"));
    try t.expect(sc.lit("=") != null);
    try t.expectEqual(@as(?usize, 8), sc.find("#"));
    try t.expect(sc.sameLine(0, 11));
    try t.expect(!sc.sameLine(0, 12));
}
