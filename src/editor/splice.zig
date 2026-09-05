//! The API a format's editing hooks are written against.
//!
//! A hook — a `Language` decl named for the `Editor` method it overrides (see
//! `Decls.hooks` in `languages/language.zig`) — is handed the editor and
//! does its own splicing. The surface it may reach is small and is stated
//! here, as a file boundary rather than a new abstraction: the free
//! functions below, plus the handful of `Editor` members named in the next
//! paragraph. A change to anything in `editor.zig` outside that set cannot
//! break a format, and a format author can read this one file and know what
//! they may call. See `docs/proposals/pluggable-formats.md` §5.4.
//!
//! **The `Editor` members a hook may use.** These are methods on the generic
//! `Editor(Language)` and so cannot live in this file; they are the contract
//! all the same:
//!
//!   * `allocator`, `source` — the editor's allocator and its source buffer
//!     (`source.items` is the current text).
//!   * `replaceAtSpan(span, text)` — the one primitive every edit reduces to.
//!   * `getParsed()` — the current parse, reparsed on demand after a splice.
//!   * `sectionExtentEnd(parsed, node)` and `gatherRegions(parsed, node,
//!     merge_touching)` — a section node's line regions, derived from
//!     `Document.node_regions` (see `editor/regions.zig`).
//!   * `writeMapValue(out, col, text)` — the engine's own `key: value`
//!     writer, for a hook that only redirects where it lands.
//!   * `insertBlockKey(parsed, node, key_text, value_text)` — the engine's
//!     block-mapping insert, for a hook that only bypasses the flow sniff.
//!
//! Everything else `pub` on `Editor` is the public editing API (`set`,
//! `deleteKey`, …) that a hook is *implementing*, not calling.
//!
//! **Source-coordinate utilities.** Editing reframes splice text against the
//! raw source, because indentation, trailing newlines, and comments live
//! *outside* any AST node span (node spans are tight: they exclude leading
//! indent and, except for block scalars, the trailing newline; comments are
//! not represented in the AST at all). The functions below are that
//! arithmetic, shared by the engine, `editor/regions.zig` and every hook.

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

/// Whether the container at `span` is written in flow style (`{...}`/`[...]`).
/// The AST records no flow/block flag, so we sniff the first content byte. ZON
/// has no other container shape — every struct/array literal opens `.{` (its
/// node span starts at the `.`, not the brace) — so it is unconditionally flow.
pub fn isFlow(source: []const u8, span: Span) bool {
    const i = firstNonSpace(source, span.start);
    if (i >= source.len) return false;
    if (source[i] == '{' or source[i] == '[') return true;
    return source[i] == '.' and i + 1 < source.len and source[i + 1] == '{';
}

/// Comment syntax for the owned-comment scan: `#` line comments (YAML/TOML) vs
/// `//` line comments and `/* */` blocks (JSON5/JSONC). Re-exported from the
/// language manifest, where it lives so a `<lang>/<lang>.zig` can name it in
/// its own `syntax` without importing the editor.
pub const CommentStyle = lang.CommentStyle;

/// Grow `line_start` upward to absorb an entry's owned comment block: the
/// contiguous run of comment lines immediately above, with no intervening blank
/// line (trivia policy "comment-above-belongs-to-key"). A blank line or any
/// non-comment content stops the scan. With `.slashes`, multi-line `/* ... */`
/// blocks are walked as a unit so a delete/move carries the whole block, not
/// just its closing line.
pub fn commentBlockStart(source: []const u8, line_start: usize, style: CommentStyle) usize {
    var ls = line_start;
    // `.slashes` only: set while scanning upward through the interior of a
    // `/* ... */` block whose opener `/*` has not been reached yet.
    var in_block = false;
    while (ls > 0) {
        const prev_start = lineStartBefore(source, ls - 1);
        const line = source[prev_start..ls];
        const trimmed = std.mem.trimStart(u8, std.mem.trimEnd(u8, line, "\r\n"), " \t");
        const is_comment = switch (style) {
            .hash => trimmed.len > 0 and trimmed[0] == '#',
            .semicolon => trimmed.len > 0 and trimmed[0] == ';',
            // plist/XML `<!-- ... -->`. Only the common own-line, single-line
            // comment is recognized for the owned-block scan (a multi-line
            // `<!--\n...\n-->` block is not walked as a unit — a rare shape a
            // hand-editor is unlikely to place directly above a key). A line
            // that merely opens a block (`<!--` with no closing `-->`) is not
            // treated as an owned comment, so a delete never half-swallows one.
            .xml_comment => std.mem.startsWith(u8, trimmed, "<!--") and std.mem.endsWith(u8, trimmed, "-->"),
            .slashes => blk: {
                if (in_block) {
                    // Inside a block comment, moving up: every line belongs to it
                    // until we reach the line bearing the `/*` opener.
                    if (std.mem.indexOf(u8, trimmed, "/*") != null) in_block = false;
                    break :blk true;
                }
                if (std.mem.startsWith(u8, trimmed, "//")) break :blk true;
                // A line ending a `/* */` block: enter block-scan mode unless it is
                // a self-contained single-line `/* ... */`.
                if (std.mem.endsWith(u8, trimmed, "*/")) {
                    if (!std.mem.startsWith(u8, trimmed, "/*")) in_block = true;
                    break :blk true;
                }
                break :blk false;
            },
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
