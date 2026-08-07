//! Shared multi-region machinery for SECTION formats — the ones whose logical
//! containers are not contiguous in the source.
//!
//! TOML assembles a table from scattered headers (`[a]` x=1 … `[other]` y=2 …
//! `[a.b]` z=3); fig does the same with `>` marker runs and re-entered headers;
//! INI does it with a reopened `[section]`. In all three the AST has one node
//! per logical container while its bytes are spread across the file, so a
//! whole-container op cannot splice a single `[min,max)` range — it must gather
//! the disjoint line-regions that belong to the container and rebuild the source
//! once.
//!
//! What is shared is everything downstream of that gather: coalescing regions,
//! splicing them out, relocating them contiguously, and reordering bundles of
//! them. What is NOT shared is the gather itself — "which lines belong to this
//! container" is the one genuinely per-format question (TOML classifies by a
//! leading `[`, fig by its value's kind plus the parser's re-entry table, INI by
//! section membership), so each `<lang>/editor_helper.zig` keeps its own and
//! hands the result here.
//!
//! Before this module, TOML and fig each carried a private copy of all of it:
//! `spliceOutRegions` and `appendWithBlankBefore` were byte-identical, and
//! `normalizeRegions` differed by a single character (see `normalize`'s
//! `merge_touching`). The move and reorder algorithms had drifted — fig's was a
//! single pass where TOML's built an intermediate buffer, and fig's had picked
//! up OOM guards TOML's was missing. This module is fig's version of each, which
//! is why TOML's `reorderTables` no longer leaks its capture buffer when an
//! allocation fails mid-bundle.
//!
//! The editor-taking functions take `self: anytype` because `Editor(Toml)`,
//! `Editor(Fig)` and `Editor(Ini)` are three distinct types with no common
//! supertype in Zig; each is used only for `.allocator`, `.source` and
//! `replaceAtSpan`, which all three have by construction.

const std = @import("std");

const Span = @import("../../util/span.zig");
const editor = @import("../../editor.zig");

const lineStartBefore = editor.lineStartBefore;
const lineEndAfter = editor.lineEndAfter;
const commentBlockStart = editor.commentBlockStart;
const CommentStyle = editor.CommentStyle;

/// A line-aligned source range `[start, end)` belonging to one logical
/// container's subtree.
pub const Region = struct { start: usize, end: usize };

/// Full line-region of an in-container entry (`key = value`, possibly spanning
/// several lines): its owned comment block through the newline ending its last
/// line.
pub fn entryLineRegion(source: []const u8, span: Span, style: CommentStyle) Region {
    return .{
        .start = commentBlockStart(source, lineStartBefore(source, span.start), style),
        .end = lineEndAfter(source, span.end -| 1),
    };
}

/// Sort `regions` by start and coalesce them into a disjoint, ascending set in
/// place; returns the coalesced count (the caller uses `regions[0..n]`).
///
/// `merge_touching` decides whether two regions that meet exactly — one's `end`
/// is the next's `start` — become one. Only TOML's rename needs false: it
/// addresses each header region's own start to rewrite the segment inside it,
/// which a merged region would hide. Every other caller is insensitive to the
/// choice, because a zero-width gap between touching regions copies nothing.
pub fn normalize(regions: []Region, merge_touching: bool) usize {
    std.mem.sort(Region, regions, {}, struct {
        fn lt(_: void, a: Region, b: Region) bool {
            return a.start < b.start;
        }
    }.lt);
    if (regions.len == 0) return 0;
    var w: usize = 0;
    for (regions[1..]) |r| {
        const overlaps = if (merge_touching) r.start <= regions[w].end else r.start < regions[w].end;
        if (overlaps) {
            regions[w].end = @max(regions[w].end, r.end);
        } else {
            w += 1;
            regions[w] = r;
        }
    }
    return w + 1;
}

/// Rebuild the source with `regions` (disjoint, ascending) removed, in one
/// `replaceAtSpan` so the reparse/rollback runs once. The whole of a
/// delete-container op once the gather has run.
pub fn spliceOut(self: anytype, regions: []const Region) !void {
    const source = self.source.items;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(self.allocator);
    var pos: usize = 0;
    for (regions) |r| {
        try out.appendSlice(self.allocator, source[pos..r.start]);
        pos = r.end;
    }
    try out.appendSlice(self.allocator, source[pos..]);
    try self.replaceAtSpan(Span.init(0, source.len), out.items);
}

/// Append `block` to `out` separated from any preceding content by exactly one
/// blank line (two newlines). `block` is appended verbatim (it already ends in
/// a newline). Used when relocating a container so it reads as its own section
/// at the destination.
pub fn appendWithBlankBefore(out: *std.ArrayList(u8), allocator: std.mem.Allocator, block: []const u8) !void {
    if (block.len == 0) return;
    const n = out.items.len;
    if (n > 0) {
        if (n >= 2 and out.items[n - 1] == '\n' and out.items[n - 2] == '\n') {
            // already a blank line
        } else if (out.items[n - 1] == '\n') {
            try out.append(allocator, '\n');
        } else {
            try out.appendSlice(allocator, "\n\n");
        }
    }
    try out.appendSlice(allocator, block);
}

/// Remove `used` (a container's gathered, normalized regions) from their
/// current positions and re-emit those bytes CONTIGUOUSLY at `dest_at`,
/// separated from surrounding content by one blank line. Interleaved foreign
/// content stays put. One splice.
///
/// A no-op when `used` is empty, or when `dest_at` falls strictly inside one of
/// the moved regions — the container would be moving into itself. A boundary
/// landing exactly at a fragment's edge (EOF coinciding with the last fragment,
/// say) is still a real relocation, since it collapses the fragments together.
pub fn relocate(self: anytype, used: []const Region, dest_at: usize) !void {
    if (used.len == 0) return;
    for (used) |r| if (dest_at > r.start and dest_at < r.end) return;

    const source = self.source.items;
    var moved: std.ArrayList(u8) = .empty;
    defer moved.deinit(self.allocator);
    for (used) |r| try moved.appendSlice(self.allocator, source[r.start..r.end]);

    // Emit the kept source with `used` removed and `moved` spliced in at
    // `dest_at`, in a single pass. The result is at most the source plus the
    // separator `appendWithBlankBefore` may add (the moved bytes are cut from
    // the source, then re-added), so one precise reservation avoids reallocs.
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(self.allocator);
    try out.ensureTotalCapacity(self.allocator, source.len + 2);
    var inserted = false;
    var pos: usize = 0;
    for (used) |r| {
        if (!inserted and dest_at >= pos and dest_at <= r.start) {
            try out.appendSlice(self.allocator, source[pos..dest_at]);
            try appendWithBlankBefore(&out, self.allocator, moved.items);
            try out.appendSlice(self.allocator, source[dest_at..r.start]);
            inserted = true;
        } else {
            try out.appendSlice(self.allocator, source[pos..r.start]);
        }
        pos = r.end;
    }
    if (!inserted) {
        try out.appendSlice(self.allocator, source[pos..dest_at]);
        try appendWithBlankBefore(&out, self.allocator, moved.items);
        try out.appendSlice(self.allocator, source[dest_at..]);
    } else {
        try out.appendSlice(self.allocator, source[pos..]);
    }
    try self.replaceAtSpan(Span.init(0, source.len), out.items);
}

/// Remove `used` — the union of every reordered container's regions, already
/// normalized — and re-emit `bundles` (each container's captured bytes, in the
/// caller's desired final order) at the position the earliest of them currently
/// occupies. Containers not named are untouched.
///
/// Separation is `appendBlockSep`'s (tight) rather than `relocate`'s blank
/// line: these were already siblings in one region of the file, so reordering
/// them should not introduce spacing that was not there.
pub fn reorderBundles(self: anytype, used: []const Region, bundles: []const []const u8) !void {
    if (used.len == 0) return;
    const source = self.source.items;
    const anchor = used[0].start;

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(self.allocator);
    var pos: usize = 0;
    for (used) |r| {
        if (anchor >= pos and anchor <= r.start) {
            try out.appendSlice(self.allocator, source[pos..anchor]);
            for (bundles) |b| {
                try editor.appendBlockSep(&out, self.allocator, b);
                if (b.len > 0 and b[b.len - 1] != '\n') try out.append(self.allocator, '\n');
            }
            try out.appendSlice(self.allocator, source[anchor..r.start]);
        } else {
            try out.appendSlice(self.allocator, source[pos..r.start]);
        }
        pos = r.end;
    }
    try out.appendSlice(self.allocator, source[pos..]);
    try self.replaceAtSpan(Span.init(0, source.len), out.items);
}

/// Capture one container's bytes from its gathered regions, appending those
/// regions to `all` (the caller's global removal set) as it goes. The per-name
/// body of a reorder: the caller gathers, this captures, `reorderBundles`
/// rewrites.
///
/// The returned slice is owned by `allocator`. The errdefer pair is load-
/// bearing on the OOM path: `bytes` owns the buffer while it is growing, and
/// nothing owns it between `toOwnedSlice` and the caller taking it.
pub fn captureBundle(allocator: std.mem.Allocator, source: []const u8, regions: []const Region, all: *std.ArrayList(Region)) ![]u8 {
    var bytes: std.ArrayList(u8) = .empty;
    errdefer bytes.deinit(allocator);
    for (regions) |r| {
        try bytes.appendSlice(allocator, source[r.start..r.end]);
        try all.append(allocator, r);
    }
    return bytes.toOwnedSlice(allocator);
}
