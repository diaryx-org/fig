const Document = @This();
const std = @import("std");

const AST = @import("ast/ast.zig");
const Span = @import("util/span.zig");

/// A parsed document plus source-location metadata.
///
/// `source` is borrowed. `document.nodes` and `node_spans` are owned by this
/// value and freed by `deinit`.
source: []const u8,
ast: AST,
/// Indexed by node id: `node_spans[node.id]` is that node's source span.
node_spans: []const Span,
/// Indexed by node id: source span of the `&name` token attached to this node,
/// or null. Source-coupled, so it lives here (the editor uses it to splice);
/// the decoded name lives on `ast.node_anchors`. Empty when no anchors.
node_anchor_spans: []const ?Span = &.{},
/// Indexed by node id: source span of the `!tag` token attached to this node,
/// or null. Decoded tag text lives on `ast.node_tags`. Empty when no tags.
node_tag_spans: []const ?Span = &.{},
/// Indexed by node id: source span of the token that INTRODUCES this node as
/// a block-sequence item — YAML's and NestedText's `-`, fig's `*` — or null
/// for every node that is not one, and for an item whose span already begins
/// at its introducing token (plist's `<string>` element). Empty when the
/// document has no such items, which is every document of a format with no
/// block-sequence marker.
///
/// This is the one item fact `node_spans` cannot carry: an item's span starts
/// at its VALUE, which for a nested or empty value (`-\n  k: v`, a bare `-`)
/// begins on a later line than the marker, so nothing in the span says where
/// the item's own line is. The editor reads it for every block-sequence op
/// — where an item's line starts, where a leading comment goes, what prefix
/// a new sibling copies. NestedText used to hook five operations to
/// recompute it by scanning; see `docs/proposals/runtime-languages.md` §4.1.
node_marker_spans: []const ?Span = &.{},
/// Indexed by node id: for a `keyvalue` node, the source span of the token
/// that SEPARATES its key from its value — YAML's `:`, fig's `=`, a
/// NestedText plain key's `:` — or null. Empty when the document records
/// none.
///
/// A recorded separator is the parser's statement that the entry's value is
/// REFRAMED rather than spliced in place: the editor rewrites everything
/// after the key, from the separator through the old value's end, so a
/// value can change shape between inline (`key: v`) and block (`key:` over
/// indented lines) — the reframe fig, YAML and NestedText each used to hook.
/// A format whose values never change shape (JSON, TOML) records none and
/// gets the direct span splice. The separator also fixes where the value's
/// text begins: past it, then past any anchor or tag. See
/// `editor.Editor.replaceValAtPath`.
node_sep_spans: []const ?Span = &.{},
/// The physical HEADER LINES of every container whose span does not describe
/// its extent — a SECTION node: a TOML `[table]`/`[[array]]`/dotted table, an
/// INI `[section]`, a fig block container. One entry per header line that
/// created or re-opened the node, in source order, each a whole line (from
/// its first byte through its newline). Empty for every other node, and for
/// every document of a format with no sections (`Syntax.section_noun == null`).
///
/// This is the one fact the editor cannot derive from `node_spans`: a section
/// node's span anchors only the key segment of the line that CREATED it (or,
/// for fig, its opening line widened to the subtree's end), so a later header
/// that re-enters the same container — TOML's `[a]` … `[b]` … `[a.c]`, INI's
/// reopened `[a]`, fig's re-entered `database` — sits in no node's span at
/// all. Everything else about a container's physical extent is DERIVED from
/// this plus the spans: `editor/regions.zig`'s `gather` walks the subtree,
/// takes each header line here, and takes each contiguous entry's own line
/// from its span. Recording the entries too would be a second copy of
/// `node_spans`, and which comment lines ride with a line is the editor's
/// policy (`commentBlockStart`), not the parser's.
///
/// Sorted by `(node_id, start)`, so `regionsOf` is a binary search; parsers
/// hand `finishRegions` an unsorted list. Presence in this table is what
/// makes a node a section: `isSection` is the predicate behind the engine's
/// one line-splice rule (see `editor.zig`, "Whole-container structural
/// editing") and behind the recursion in `gather`.
node_regions: []const NodeRegion = &.{},
/// Every place a SECTION node's NAME is written, as a span the editor can
/// splice a new name over: a TOML table's own `[a]`, the `a` in `[a.b]`
/// and `[[a.c]]`, the `a` in a dotted `a.b = 1`; an INI `[section]` and each
/// reopening; a fig block container's header and each re-entry. Sorted by
/// `(node_id, span.start)` like `node_regions`; empty for a document of a
/// format with no sections.
///
/// `kind` says how the mention sits relative to the node's PARENT: a
/// `.header` opens a region of the node's own, outside the parent's lines
/// (`[a.b]` is not inside `[a]`'s text; a reopened INI `[a]` is a new
/// region), while an `.entry` mention is on one of the parent's own entry
/// lines (a dotted key, a fig header at the parent's depth). The editor's
/// block insert reads it to keep a new entry inside the intended table —
/// after the last child written on the parent's own lines, never after a
/// sub-table header — and `renameContainer` and a section's
/// `replaceKeyAtPath` rewrite every mention at once, which is the whole of
/// what TOML's rename hook did by scanning.
node_mentions: []const NodeMention = &.{},

/// One physical header line belonging to node `node_id`: `[start, end)` is the
/// whole line, `end` just past its newline (or `source.len` on an unterminated
/// final line). See `node_regions`.
pub const NodeRegion = struct { node_id: AST.Node.Id, start: usize, end: usize };

/// One place section node `node_id`'s name is written. See `node_mentions`.
pub const NodeMention = struct { node_id: AST.Node.Id, span: Span, kind: MentionKind };

/// How a mention sits relative to the node's parent. See `node_mentions`.
pub const MentionKind = enum {
    /// A header line of the node's own, outside its parent's lines.
    header,
    /// On one of the parent's own entry lines.
    entry,
};

pub fn deinit(self: Document, allocator: std.mem.Allocator) void {
    var ast = self.ast;
    ast.deinit();
    allocator.free(self.node_spans);
    allocator.free(self.node_anchor_spans);
    allocator.free(self.node_tag_spans);
    allocator.free(self.node_marker_spans);
    allocator.free(self.node_sep_spans);
    allocator.free(self.node_regions);
    allocator.free(self.node_mentions);
}

/// The name mentions recorded for node `id`, in source order — empty for a
/// node that is not a section, or whose format records none. O(log n) over
/// `node_mentions`.
pub fn mentionsOf(self: Document, id: AST.Node.Id) []const NodeMention {
    const all = self.node_mentions;
    var lo: usize = 0;
    var hi: usize = all.len;
    while (lo < hi) {
        const mid = lo + (hi - lo) / 2;
        if (all[mid].node_id < id) lo = mid + 1 else hi = mid;
    }
    const first = lo;
    hi = all.len;
    while (lo < hi) {
        const mid = lo + (hi - lo) / 2;
        if (all[mid].node_id <= id) lo = mid + 1 else hi = mid;
    }
    return all[first..lo];
}

/// Sort a parser's accumulated mentions into `node_mentions` order.
pub fn sortMentions(mentions: []NodeMention) void {
    std.mem.sort(NodeMention, mentions, {}, struct {
        fn lt(_: void, a: NodeMention, b: NodeMention) bool {
            if (a.node_id != b.node_id) return a.node_id < b.node_id;
            return a.span.start < b.span.start;
        }
    }.lt);
}

/// Source span of the key/value separator on the `keyvalue` node `node`, or
/// null. Null when the document records no separators (the table is empty),
/// and for a node that is not an entry — see `node_sep_spans`.
pub fn sepSpan(self: Document, node: AST.Node) ?Span {
    if (node.id >= self.node_sep_spans.len) return null;
    return self.node_sep_spans[node.id];
}

/// Source span of the `-`/`*` token introducing `node` as a block-sequence
/// item, or null. Null when the document records no markers (the table is
/// empty), and for a node that is not an item — see `node_marker_spans`.
pub fn markerSpan(self: Document, node: AST.Node) ?Span {
    if (node.id >= self.node_marker_spans.len) return null;
    return self.node_marker_spans[node.id];
}

/// One recorded per-node span, as a parser accumulates them: `span` is the
/// token (an item's `-`/`*` marker, an entry's separator) belonging to node
/// `node_id`. `buildSpanTable` turns the accumulated list into a per-node
/// table.
pub const SpanEntry = struct { node_id: AST.Node.Id, span: Span };

/// A sparse per-node span table (`node_marker_spans`, `node_sep_spans`) for
/// `node_count` nodes from a parser's accumulated `entries`: empty (`&.{}`,
/// nothing allocated) when there are none, else one slot per node with each
/// entry's span in its node's slot. The caller owns the result; `deinit`
/// frees it.
pub fn buildSpanTable(allocator: std.mem.Allocator, node_count: usize, entries: []const SpanEntry) std.mem.Allocator.Error![]const ?Span {
    if (entries.len == 0) return &.{};
    const table = try allocator.alloc(?Span, node_count);
    @memset(table, null);
    for (entries) |e| table[e.node_id] = e.span;
    return table;
}

pub fn span(self: Document, node: AST.Node) Span {
    return self.node_spans[node.id];
}

/// Source span of the `&name` anchor token on `node`, or null. Returns null
/// when the document declares no anchors (the table is empty).
pub fn anchorSpan(self: Document, node: AST.Node) ?Span {
    if (node.id >= self.node_anchor_spans.len) return null;
    return self.node_anchor_spans[node.id];
}

/// Source span of the `!tag` token on `node`, or null. Returns null when the
/// document declares no tags (the table is empty).
pub fn tagSpan(self: Document, node: AST.Node) ?Span {
    if (node.id >= self.node_tag_spans.len) return null;
    return self.node_tag_spans[node.id];
}

/// The header lines recorded for node `id`, in source order — empty for a
/// node that is not a section. O(log n) over `node_regions`.
pub fn regionsOf(self: Document, id: AST.Node.Id) []const NodeRegion {
    const all = self.node_regions;
    // Lower bound: first entry with node_id >= id.
    var lo: usize = 0;
    var hi: usize = all.len;
    while (lo < hi) {
        const mid = lo + (hi - lo) / 2;
        if (all[mid].node_id < id) lo = mid + 1 else hi = mid;
    }
    var end = lo;
    while (end < all.len and all[end].node_id == id) end += 1;
    return all[lo..end];
}

/// Whether `node` is a SECTION node — a container the parser recorded header
/// lines for, because its span does not describe its physical extent. The
/// root is never one.
pub fn isSection(self: Document, node: AST.Node) bool {
    return self.regionsOf(node.id).len > 0;
}

/// The whole line containing `at`: from the byte after the previous newline
/// (or 0) through the next newline inclusive (or `source.len`). What a parser
/// records for a header line, given any byte offset on it — the header's key
/// token, its line start, whichever the parser has to hand.
pub fn lineRegionAt(source: []const u8, at: usize) struct { start: usize, end: usize } {
    var start = at;
    while (start > 0 and source[start - 1] != '\n') start -= 1;
    const end = if (std.mem.indexOfScalarPos(u8, source, at, '\n')) |nl| nl + 1 else source.len;
    return .{ .start = start, .end = end };
}

/// Sort a parser's accumulated header lines into `node_regions` order —
/// `(node_id, start)` ascending — in place. Every parser that fills the table
/// calls this once, at document assembly; nothing else may append afterwards.
pub fn sortRegions(regions: []NodeRegion) void {
    std.mem.sort(NodeRegion, regions, {}, struct {
        fn lt(_: void, a: NodeRegion, b: NodeRegion) bool {
            if (a.node_id != b.node_id) return a.node_id < b.node_id;
            return a.start < b.start;
        }
    }.lt);
}

test "regionsOf finds a node's header lines by binary search" {
    var regs = [_]NodeRegion{
        .{ .node_id = 7, .start = 20, .end = 30 },
        .{ .node_id = 3, .start = 0, .end = 10 },
        .{ .node_id = 7, .start = 10, .end = 20 },
    };
    sortRegions(&regs);
    const doc: Document = .{ .source = "", .ast = undefined, .node_spans = &.{}, .node_regions = &regs };
    try std.testing.expectEqual(@as(usize, 0), doc.regionsOf(0).len);
    try std.testing.expectEqual(@as(usize, 1), doc.regionsOf(3).len);
    const seven = doc.regionsOf(7);
    try std.testing.expectEqual(@as(usize, 2), seven.len);
    try std.testing.expectEqual(@as(usize, 10), seven[0].start);
    try std.testing.expectEqual(@as(usize, 20), seven[1].start);
    try std.testing.expectEqual(@as(usize, 0), doc.regionsOf(9).len);
}

test "lineRegionAt covers the whole line, newline included" {
    const src = "ab\ncd\nef";
    try std.testing.expectEqual(@as(usize, 0), lineRegionAt(src, 1).start);
    try std.testing.expectEqual(@as(usize, 3), lineRegionAt(src, 1).end);
    try std.testing.expectEqual(@as(usize, 3), lineRegionAt(src, 3).start);
    try std.testing.expectEqual(@as(usize, 6), lineRegionAt(src, 4).end);
    try std.testing.expectEqual(@as(usize, 6), lineRegionAt(src, 7).start);
    try std.testing.expectEqual(@as(usize, 8), lineRegionAt(src, 7).end);
}
