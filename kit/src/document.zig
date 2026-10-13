//! A parsed document: its source, and the tree read from it (`ast`), whose
//! rows carry every node's span and whose sparse lists carry the other
//! source positions the editor splices at — anchor and tag tokens, item
//! markers, entry separators, a section's header lines and the places its
//! name is written. The accessors here read those; the tree's own are on
//! `AST`.

const Document = @This();
const std = @import("std");

const AST = @import("ast/ast.zig");
const Span = @import("span.zig");
const table = @import("table.zig");

/// Borrowed: every in-source text and span of `ast` indexes it.
source: []const u8,
ast: AST,

/// How a separated sequence's items are joined: the separator the language
/// names for this one container, and whether the container has delimiters.
///
/// The dialect's `Syntax.flow_entry_sep` and `flow_root` say how a format's
/// flow containers are spelled, once for the whole format. A format whose
/// separator differs by container cannot say it there: a `.tune` voice
/// joins its bars with `|` and a bar its tokens with spaces, and neither
/// has brackets. Only the parser knows which container it read, so the
/// fact is a column of the parse, as the item markers and entry separators
/// are. A sequence with one is edited by the flow arm with this separator,
/// whatever its first byte. See `docs/proposals/runtime-languages.md` §4.1,
/// the `item_sep` and `open` columns.
pub const ItemSep = struct {
    /// The bytes written between two items that share a line — `" | "`,
    /// `" "`, or `"\n\n"` for items a blank line apart. What a delete
    /// consumes is this with its whitespace trimmed (`|`), or whitespace
    /// alone where nothing is left. Null: the dialect's `flow_entry_sep`.
    text: ?[]const u8,
    /// The container has no delimiters: its items run from its span's
    /// start to its end, a trailing separator included where the format
    /// allows one. `flow_root` is this for a root mapping.
    open: bool,
};

/// One physical HEADER LINE of a SECTION node — a container whose span does
/// not describe its extent: a TOML `[table]`/`[[array]]`/dotted table, an
/// INI `[section]`, a fig block container. One per header line that created
/// or re-opened the node, in source order, each a whole line (from its
/// first byte through its newline).
///
/// This is the one fact the editor cannot derive from the spans: a section
/// node's span anchors only the key segment of the line that CREATED it (or,
/// for fig, its opening line widened to the subtree's end), so a later header
/// that re-enters the same container — TOML's `[a]` … `[b]` … `[a.c]`, INI's
/// reopened `[a]`, fig's re-entered `database` — sits in no node's span at
/// all. Everything else about a container's physical extent is DERIVED from
/// this plus the spans: `editor/regions.zig`'s `gather` walks the subtree,
/// takes each header line here, and takes each contiguous entry's own line
/// from its span. Presence of one is what makes a node a section.
pub const NodeRegion = table.SpanFact;

/// One place a SECTION node's NAME is written, as a span the editor can
/// splice a new name over: a TOML table's own `[a]`, the `a` in `[a.b]`
/// and `[[a.c]]`, the `a` in a dotted `a.b = 1`; an INI `[section]` and each
/// reopening; a fig block container's header and each re-entry.
///
/// `kind` says how the mention sits relative to the node's PARENT
/// (`MentionKind`): a header opens a region of the node's own, outside the
/// parent's lines (`[a.b]` is not inside `[a]`'s text; a reopened INI `[a]`
/// is a new region), while an entry mention is on one of the parent's own
/// entry lines (a dotted key, a fig header at the parent's depth). The
/// editor's block insert reads it to keep a new entry inside the intended
/// table, and `renameContainer` and a section's `replaceKeyAtPath` rewrite
/// every mention at once.
pub const NodeMention = struct {
    node_id: AST.Node.Id,
    span: Span,
    kind: MentionKind,

    fn of(m: table.Mention) NodeMention {
        return .{ .node_id = m.node, .span = Span.init(m.start, m.end), .kind = .of(m.kind) };
    }
};

/// How a mention sits relative to the node's parent. See `NodeMention`.
///
/// The first two are a section's NAME. The other two are where a SEQUENCE
/// ITEM's place in its path is written, which no span carries either: an
/// item has no key, and the lines that address it may be anywhere. A
/// delete renumbers an `index` mention and re-spells an `append` one so
/// that the items after the deleted one stay the items they were.
pub const MentionKind = enum {
    /// A header line of the node's own, outside its parent's lines.
    header,
    /// On one of the parent's own entry lines.
    entry,
    /// The item's index, where a path addresses it by position: figl's `1`
    /// in `xs[1] = b`, `xs[1]` and `xs[1].k = v`. Deleting an earlier item
    /// writes the new one over it.
    index,
    /// The header that appended the item — figl's `a.b[]`, the whole path —
    /// or the line that repeats the header before it, figl's `+`. When the
    /// item before goes, taking its header, the next item's `append`
    /// mention is written as that header, in case it repeated it.
    append,

    /// Whether this mention is a section's name, which a rename rewrites.
    pub fn isName(k: MentionKind) bool {
        return k == .header or k == .entry;
    }

    /// The node table's byte for this kind (`table.mention_*`).
    pub fn byte(k: MentionKind) u8 {
        return switch (k) {
            .header => table.mention_header,
            .entry => table.mention_entry,
            .index => table.mention_index,
            .append => table.mention_append,
        };
    }

    /// The kind the node table's byte `b` stands for; a table is checked
    /// for an unknown one when it is read.
    pub fn of(b: u8) MentionKind {
        return switch (b) {
            table.mention_entry => .entry,
            table.mention_index => .index,
            table.mention_append => .append,
            else => .header,
        };
    }
};

/// Free everything but the tree, and return the tree: for a caller that
/// wants values, not source positions.
pub fn intoAst(self: Document, allocator: std.mem.Allocator) AST {
    _ = allocator;
    return self.ast;
}

pub fn deinit(self: Document, allocator: std.mem.Allocator) void {
    _ = allocator;
    var ast = self.ast;
    ast.deinit();
}

pub fn span(self: *const Document, node: AST.Node) Span {
    return self.ast.spanOf(node.id);
}

/// Node `id`'s span.
pub fn spanOf(self: *const Document, id: AST.Node.Id) Span {
    return self.ast.spanOf(id);
}

/// Source span of the `&name` anchor token on `node`, or null.
pub fn anchorSpan(self: *const Document, node: AST.Node) ?Span {
    const f = table.factOf(table.Anchor, self.ast.anchor_facts, node.id) orelse return null;
    return optSpan(f.start, f.end);
}

/// Source span of the `!tag` token on `node`, or null.
pub fn tagSpan(self: *const Document, node: AST.Node) ?Span {
    const f = table.factOf(table.Tag, self.ast.tag_facts, node.id) orelse return null;
    return optSpan(f.start, f.end);
}

/// Source span of the token that INTRODUCES `node` as a block-sequence item
/// — YAML's and NestedText's `-`, fig's `*` — or null for every node that is
/// not one, and for an item whose span already begins at its introducing
/// token (plist's `<string>` element).
///
/// This is the one item fact a span cannot carry: an item's span starts at
/// its VALUE, which for a nested or empty value (`-\n  k: v`, a bare `-`)
/// begins on a later line than the marker, so nothing in the span says where
/// the item's own line is. The editor reads it for every block-sequence op
/// — where an item's line starts, where a leading comment goes, what prefix
/// a new sibling copies.
pub fn markerSpan(self: *const Document, node: AST.Node) ?Span {
    const f = table.factOf(table.SpanFact, self.ast.marker_facts, node.id) orelse return null;
    return Span.init(f.start, f.end);
}

/// For a `keyvalue` node, the source span of the token that SEPARATES its
/// key from its value — YAML's `:`, fig's `=`, a NestedText plain key's `:`
/// — or null.
///
/// A recorded separator is the parser's statement that the entry's value is
/// REFRAMED rather than spliced in place: the editor rewrites everything
/// after the key, from the separator through the old value's end, so a
/// value can change shape between inline (`key: v`) and block (`key:` over
/// indented lines). A format whose values never change shape (JSON, TOML)
/// records none and gets the direct span splice. The separator also fixes
/// where the value's text begins: past it, then past any anchor or tag. See
/// `editor.Editor.replaceValAtPath`.
pub fn sepSpan(self: *const Document, node: AST.Node) ?Span {
    const f = table.factOf(table.SpanFact, self.ast.sep_facts, node.id) orelse return null;
    return Span.init(f.start, f.end);
}

/// How the items of sequence `node` are joined, when the parser said so for
/// this container, or null — see `ItemSep`.
pub fn itemSep(self: *const Document, node: AST.Node) ?ItemSep {
    const open = self.ast.rows[node.id].flags & table.row_open != 0;
    const text: ?[]const u8 = if (table.factOf(table.ItemSep, self.ast.item_sep_facts, node.id)) |f| blk: {
        const x = self.ast.textOf(f.text) orelse break :blk null;
        break :blk if (x.len == 0) null else x;
    } else null;
    if (text == null and !open) return null;
    return .{ .text = text, .open = open };
}

/// The header lines recorded for node `id`, in source order — empty for a
/// node that is not a section. O(log n).
pub fn regionsOf(self: *const Document, id: AST.Node.Id) []const NodeRegion {
    return run(table.SpanFact, self.ast.region_facts, id);
}

/// The name mentions recorded for node `id`, in source order — empty for a
/// node that is not a section, or whose format records none. O(log n).
pub fn mentionsOf(self: *const Document, id: AST.Node.Id) MentionIterator {
    return .{ .items = run(table.Mention, self.ast.mention_facts, id) };
}

/// A section's mentions, as `NodeMention`s.
pub const MentionIterator = struct {
    items: []const table.Mention,
    i: usize = 0,

    pub fn next(it: *MentionIterator) ?NodeMention {
        if (it.i >= it.items.len) return null;
        defer it.i += 1;
        return NodeMention.of(it.items[it.i]);
    }

    pub fn count(it: MentionIterator) usize {
        return it.items.len;
    }

    pub fn at(it: MentionIterator, i: usize) NodeMention {
        return NodeMention.of(it.items[i]);
    }
};

/// Whether `node` is a SECTION node — a container the parser recorded header
/// lines for, because its span does not describe its physical extent. The
/// root is never one.
pub fn isSection(self: *const Document, node: AST.Node) bool {
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

fn optSpan(start: u32, end: u32) ?Span {
    if (start == table.none) return null;
    return Span.init(start, end);
}

/// The run of `items` (sorted by node) naming `id`.
fn run(comptime T: type, items: []const T, id: u32) []const T {
    var lo: usize = 0;
    var hi: usize = items.len;
    while (lo < hi) {
        const mid = lo + (hi - lo) / 2;
        if (items[mid].node < id) lo = mid + 1 else hi = mid;
    }
    var end = lo;
    while (end < items.len and items[end].node == id) end += 1;
    return items[lo..end];
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
