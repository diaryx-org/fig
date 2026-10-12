//! Source-coordinate utilities shared by the editor engine, the regions
//! module and the formats' editor tests.
//!
//! This file used to be the API a format's editing hooks were written
//! against, stated as a file boundary: what a hook could reach was the free
//! functions here and a handful of `Editor` members. There are no hooks any
//! more — a format states facts (`syntax`, what its parser records on
//! `Document`) and spells fragments (the renderers, which are pure string
//! functions) and never receives the editor — so the boundary is simply the
//! engine's own. See `docs/proposals/runtime-languages.md` §4.4.
//!
//! **Source-coordinate utilities.** Editing reframes splice text against the
//! raw source, because indentation, trailing newlines, and comments live
//! *outside* any AST node span (node spans are tight: they exclude leading
//! indent and, except for block scalars, the trailing newline; comments are
//! not represented in the AST at all). The functions below are that
//! arithmetic, shared by the engine and `editor/regions.zig`.

const std = @import("std");
const Span = @import("../util/span.zig");
const lang = @import("../languages/manifest.zig");

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

/// The comment syntax the owned-comment scan reads: the language's declared
/// delimiters. Re-exported from the language manifest, where it lives so a
/// `<lang>/<lang>.zig` can name it in its own `syntax` without importing the
/// editor.
pub const Comments = lang.Comments;

/// Grow `line_start` upward to absorb an entry's owned comment block: the
/// contiguous run of comment lines immediately above, with no intervening blank
/// line (trivia policy "comment-above-belongs-to-key"). A blank line or any
/// non-comment content stops the scan. A comment line is one that opens with
/// the declared `line` delimiter, or holds the whole of it when it is a pair
/// (`<!-- ... -->`, recognized on one line only, so a delete never
/// half-swallows a block); a declared `block` pair (`/* ... */`) is walked as
/// a unit across lines, so a delete or move carries the whole block, not just
/// its closing line.
pub fn commentBlockStart(source: []const u8, line_start: usize, comments: Comments) usize {
    var ls = line_start;
    // Set while scanning upward through the interior of a `block` comment
    // whose opener has not been reached yet.
    var in_block = false;
    while (ls > 0) {
        const prev_start = lineStartBefore(source, ls - 1);
        const line = source[prev_start..ls];
        const trimmed = std.mem.trimStart(u8, std.mem.trimEnd(u8, line, "\r\n"), " \t");
        const is_comment = blk: {
            if (comments.block) |b| {
                if (in_block) {
                    // Inside a block comment, moving up: every line belongs to
                    // it until we reach the line bearing the opener.
                    if (std.mem.indexOf(u8, trimmed, b.open) != null) in_block = false;
                    break :blk true;
                }
                // A line ending a block: enter block-scan mode unless it is a
                // self-contained single-line one.
                if (b.close.len > 0 and std.mem.endsWith(u8, trimmed, b.close)) {
                    if (!std.mem.startsWith(u8, trimmed, b.open)) in_block = true;
                    break :blk true;
                }
            }
            const l = comments.line orelse break :blk false;
            if (l.open.len == 0 or !std.mem.startsWith(u8, trimmed, l.open)) break :blk false;
            break :blk l.close.len == 0 or std.mem.endsWith(u8, trimmed, l.close);
        };
        if (is_comment) {
            ls = prev_start;
        } else break;
    }
    return ls;
}

/// Append a relocated entry `block` to `out`, guaranteeing a single '\n'
/// separator from whatever precedes it. The block's own bytes are appended
/// verbatim (its trailing newline, if any, is preserved), so concatenating
/// blocks in a new order never welds two entries onto one line.
pub fn appendBlockSep(out: *std.ArrayList(u8), allocator: std.mem.Allocator, block: []const u8) !void {
    if (out.items.len > 0 and out.items[out.items.len - 1] != '\n') {
        try out.append(allocator, '\n');
    }
    try out.appendSlice(allocator, block);
}

/// A relocatable entry block: a byte range `[start, end)` covering one mapping
/// entry or sequence item (its owned comment block through its last line).
pub const Block = struct { start: usize, end: usize };

/// Fill each block's `end` from the next block's `start` so the blocks tile a
/// contiguous region; the final block runs to `last_end`. Trailing trivia (a
/// blank line, an orphan comment) thus rides with the preceding entry.
pub fn tileBlocks(blocks: []Block, last_end: usize) void {
    for (blocks, 0..) |*b, i| {
        b.end = if (i + 1 < blocks.len) blocks[i + 1].start else last_end;
    }
}

/// Build a full permutation of `0..n`: the valid, de-duplicated indices in
/// `order` first (in the given order), then every remaining index in ascending
/// (original) order. Caller owns the returned slice. An empty `order` yields
/// the identity, so a reorder with nothing to bring forward is a no-op.
pub fn fullOrder(allocator: std.mem.Allocator, order: []const usize, n: usize) ![]usize {
    const result = try allocator.alloc(usize, n);
    errdefer allocator.free(result);
    const used = try allocator.alloc(bool, n);
    defer allocator.free(used);
    @memset(used, false);
    var k: usize = 0;
    for (order) |idx| {
        if (idx < n and !used[idx]) {
            result[k] = idx;
            used[idx] = true;
            k += 1;
        }
    }
    for (0..n) |i| {
        if (!used[i]) {
            result[k] = i;
            k += 1;
        }
    }
    return result;
}
