//! Positions in source text by line: where a line starts and ends, its
//! first non-blank byte, a byte's column, and whether a container opens in
//! flow. Shared by the editor's splices and the languages' renderers.

const std = @import("std");
const Span = @import("span.zig");

/// Byte index of the start of the line containing `at` (just past the previous
/// '\n', or 0).
pub fn lineStartBefore(source: []const u8, at: usize) usize {
    var i = at;
    while (i > 0) : (i -= 1) {
        if (source[i - 1] == '\n') return i;
    }
    return 0;
}

/// Byte index just past the next '\n' at or after `at`, or `source.len`.
pub fn lineEndAfter(source: []const u8, at: usize) usize {
    if (std.mem.indexOfScalarPos(u8, source, at, '\n')) |nl| return nl + 1;
    return source.len;
}

/// Index of the first non-space/non-tab byte at or after `from`.
pub fn firstNonSpace(source: []const u8, from: usize) usize {
    var i = from;
    while (i < source.len and (source[i] == ' ' or source[i] == '\t')) i += 1;
    return i;
}

/// Column (0-based) of the byte at `at` within its line.
pub fn columnOf(source: []const u8, at: usize) usize {
    return at - lineStartBefore(source, at);
}

/// Whether the container at `span` is written in flow style: it opens with
/// `{`, `[`, or the language's own `flow_map_open` (ZON's `.{`, which opens
/// its arrays too). The AST records no flow/block flag, so we sniff the
/// first content bytes.
pub fn isFlow(source: []const u8, span: Span, flow_map_open: []const u8) bool {
    const i = firstNonSpace(source, span.start);
    if (i >= source.len) return false;
    if (source[i] == '{' or source[i] == '[') return true;
    return std.mem.startsWith(u8, source[i..], flow_map_open);
}

/// Byte index just past the flow container's opener at `span`: the
/// language's `flow_map_open` where it opens with that, else the one-byte
/// `{` or `[`.
pub fn flowOpenEnd(source: []const u8, span: Span, flow_map_open: []const u8) usize {
    const i = firstNonSpace(source, span.start);
    if (std.mem.startsWith(u8, source[i..], flow_map_open)) return i + flow_map_open.len;
    return i + 1;
}
