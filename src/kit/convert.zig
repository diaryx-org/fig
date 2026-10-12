//! The node table and `Document`, each made from the other: what a
//! language's parse fills becomes a `Document` (`tableToDocument`), and a
//! `Document` becomes the table a print reads (`documentToTable`) or a parse
//! would have filled (`fullTable`). The kit's, so that a language built apart
//! from core can turn its own tree into the table and the table it is handed
//! back into a tree.

const std = @import("std");
const Allocator = std.mem.Allocator;
const AST = @import("ast/ast.zig");
const Document = @import("document.zig");
const Span = @import("span.zig");
const table = @import("table.zig");
const Table = table.Table;
const Node = AST.Node;
const ExtKind = Node.Kind.Extended.ExtKind;
const no_node = table.none;

pub const TableError = error{
    /// A row's `kind` or `ext_kind` is not a value core knows.
    UnknownKind,
    /// A parent index past the table, a child before its parent, a keyvalue
    /// without exactly a key and a value, a mapping child that is not a
    /// keyvalue, a scalar with children, a bool whose text is not
    /// `true`/`false`, a row with no span, a second root.
    MalformedTable,
    OutOfMemory,
};

/// A `Document` over `source` from a parse's table, which is checked as it
/// is read: row 0 is the root and the only row without a parent, a parent
/// precedes its children, a mapping's children are keyvalues, a keyvalue
/// has exactly a key and a value, a scalar has no children, every span and
/// text lies in range, and every side fact names a row. Every string the AST
/// keeps is copied into it; the table is not retained.
pub fn tableToDocument(allocator: Allocator, source: []const u8, t: *const Table) TableError!Document {
    const rows = t.rows.items;
    var ast: AST = .{ .allocator = allocator, .root = 0, .nodes = &.{} };
    var owned: std.ArrayList([]const u8) = .empty;
    errdefer {
        for (owned.items) |x| allocator.free(x);
        owned.deinit(allocator);
    }
    // The empty document is spelled as one null row by the format; a table
    // with no rows at all is refused as the ambiguity it is.
    if (rows.len == 0) return error.MalformedTable;

    const nodes = try allocator.alloc(Node, rows.len);
    errdefer allocator.free(nodes);
    const spans = try allocator.alloc(Span, rows.len);
    errdefer allocator.free(spans);
    // The AST's side tables are assigned as they are built; on any later
    // failure each is freed here (an unassigned one is the empty slice,
    // which frees as nothing).
    errdefer {
        allocator.free(ast.node_anchors);
        allocator.free(ast.anchors);
        allocator.free(ast.node_tags);
        allocator.free(ast.tag_directives);
        for (ast.node_comments) |x| {
            allocator.free(x.leading);
            allocator.free(x.dangling);
        }
        allocator.free(ast.node_comments);
    }

    // A required text, and an optional one: absent is allowed, but text
    // that names bytes outside the source or the buffer is not.
    const textOf = struct {
        fn f(tt: *const Table, x: table.Text) TableError![]const u8 {
            return tt.textOf(x) orelse error.MalformedTable;
        }
    }.f;
    const optText = struct {
        fn f(tt: *const Table, x: table.Text) TableError![]const u8 {
            if (x.isAbsent()) return "";
            return tt.textOf(x) orelse error.MalformedTable;
        }
    }.f;

    // Pass 1: kinds, text, spans, and the parent check — a parent precedes
    // its child (pre-order) and only row 0 has none.
    const first_child = try allocator.alloc(?u32, rows.len);
    defer allocator.free(first_child);
    const last_child = try allocator.alloc(?u32, rows.len);
    defer allocator.free(last_child);
    @memset(first_child, null);
    @memset(last_child, null);
    for (rows, 0..) |r, i| {
        if (i == 0) {
            if (r.parent != no_node) return error.MalformedTable;
        } else {
            if (r.parent == no_node or r.parent >= i) return error.MalformedTable;
        }
        spans[i] = try spanIn(source, r.start, r.end);
        if (r.flags & ~table.row_open != 0) return error.MalformedTable;
        if (r.flags & table.row_open != 0 and r.kind != table.Kind.sequence) return error.MalformedTable;
        const K = table.Kind;
        const kind: Node.Kind = switch (r.kind) {
            K.null_ => .null_,
            K.false_ => .{ .boolean = false },
            K.true_ => .{ .boolean = true },
            K.int => .{ .number = .{ .raw = try own(allocator, &owned, try textOf(t, r.text)), .kind = .integer } },
            K.float => .{ .number = .{ .raw = try own(allocator, &owned, try textOf(t, r.text)), .kind = .float } },
            K.string => .{ .string = try own(allocator, &owned, try optText(t, r.text)) },
            K.sequence => .{ .sequence = null },
            K.mapping => .{ .mapping = null },
            K.keyvalue => .{ .keyvalue = .{ .key = 0, .value = 0 } },
            K.alias => .{ .alias = try own(allocator, &owned, try textOf(t, r.text)) },
            else => blk: {
                if (r.kind < K.ext or r.kind - K.ext >= @typeInfo(ExtKind).@"enum".field_names.len) return error.UnknownKind;
                const ek: ExtKind = @fromBackingInt(@intCast(r.kind - K.ext));
                break :blk .{ .extended = .{ .kind = ek, .text = try own(allocator, &owned, try optText(t, r.text)) } };
            },
        };
        nodes[i] = .{ .id = @intCast(i), .kind = kind, .next_sibling = null };
    }

    // Pass 2: children. A container's children are linked as siblings in
    // row order; a keyvalue takes exactly two, its key then its value.
    for (rows, 0..) |r, i| {
        if (i == 0) continue;
        const p: usize = r.parent;
        switch (nodes[p].kind) {
            .sequence => {
                if (nodes[i].kind == .keyvalue) return error.MalformedTable;
            },
            .mapping => {
                if (nodes[i].kind != .keyvalue) return error.MalformedTable;
            },
            .keyvalue => {
                if (nodes[i].kind == .keyvalue) return error.MalformedTable;
            },
            else => return error.MalformedTable,
        }
        if (last_child[p]) |lc| {
            nodes[lc].next_sibling = @intCast(i);
        } else {
            first_child[p] = @intCast(i);
        }
        last_child[p] = @intCast(i);
    }
    for (nodes, 0..) |*n, i| {
        switch (n.kind) {
            .sequence => n.kind = .{ .sequence = first_child[i] },
            .mapping => n.kind = .{ .mapping = first_child[i] },
            .keyvalue => {
                const k = first_child[i] orelse return error.MalformedTable;
                const v = nodes[k].next_sibling orelse return error.MalformedTable;
                if (nodes[v].next_sibling != null) return error.MalformedTable;
                // A keyvalue's key and value are not siblings in the AST:
                // the entry names them and the sibling chain is the
                // mapping's, over its keyvalues.
                nodes[k].next_sibling = null;
                n.kind = .{ .keyvalue = .{ .key = k, .value = v } };
            },
            else => {
                if (first_child[i] != null) return error.MalformedTable;
            },
        }
    }

    // Side facts, each naming a row.
    var anchor_spans: []const ?Span = &.{};
    errdefer allocator.free(anchor_spans);
    var tag_spans: []const ?Span = &.{};
    errdefer allocator.free(tag_spans);
    if (t.anchors.items.len > 0) {
        const names = try allocator.alloc(?[]const u8, rows.len);
        ast.node_anchors = names;
        @memset(names, null);
        const aspans = try allocator.alloc(?Span, rows.len);
        anchor_spans = aspans;
        @memset(aspans, null);
        var anchors: std.ArrayList(AST.Anchor) = .empty;
        errdefer anchors.deinit(allocator);
        for (t.anchors.items) |a| {
            if (a.node >= rows.len) return error.MalformedTable;
            const name = try own(allocator, &owned, try textOf(t, a.name));
            names[a.node] = name;
            aspans[a.node] = try optSpanIn(source, a.start, a.end);
            try anchors.append(allocator, .{ .name = name, .node = a.node });
        }
        // Definition order is the AST's: by node, which is source order.
        std.mem.sort(AST.Anchor, anchors.items, {}, struct {
            fn lt(_: void, x: AST.Anchor, y: AST.Anchor) bool {
                return x.node < y.node;
            }
        }.lt);
        ast.anchors = try anchors.toOwnedSlice(allocator);
    }
    if (t.tags.items.len > 0) {
        const tags = try allocator.alloc(?AST.Tag, rows.len);
        ast.node_tags = tags;
        @memset(tags, null);
        const tspans = try allocator.alloc(?Span, rows.len);
        tag_spans = tspans;
        @memset(tspans, null);
        for (t.tags.items) |tg| {
            if (tg.node >= rows.len) return error.MalformedTable;
            tags[tg.node] = try tagOf(allocator, &owned, t, tg);
            tspans[tg.node] = try optSpanIn(source, tg.start, tg.end);
        }
    }
    var markers: std.ArrayList(Document.SpanEntry) = .empty;
    defer markers.deinit(allocator);
    for (t.markers.items) |m| {
        if (m.node >= rows.len) return error.MalformedTable;
        try markers.append(allocator, .{ .node_id = m.node, .span = try spanIn(source, m.start, m.end) });
    }
    var seps: std.ArrayList(Document.SpanEntry) = .empty;
    defer seps.deinit(allocator);
    for (t.seps.items) |m| {
        if (m.node >= rows.len) return error.MalformedTable;
        try seps.append(allocator, .{ .node_id = m.node, .span = try spanIn(source, m.start, m.end) });
    }
    const marker_spans = try Document.buildSpanTable(allocator, rows.len, markers.items);
    errdefer allocator.free(marker_spans);
    const sep_spans = try Document.buildSpanTable(allocator, rows.len, seps.items);
    errdefer allocator.free(sep_spans);

    // A separated sequence: an item separator of its own, or the open flag,
    // or both. Only a sequence has one; an empty separator is none.
    var item_seps: []const ?Document.ItemSep = &.{};
    errdefer allocator.free(item_seps);
    var any_open = false;
    for (rows) |r| {
        if (r.flags & table.row_open != 0) any_open = true;
    }
    if (t.item_seps.items.len > 0 or any_open) {
        const out = try allocator.alloc(?Document.ItemSep, rows.len);
        item_seps = out;
        for (rows, out) |r, *o| o.* = if (r.flags & table.row_open != 0) .{ .text = null, .open = true } else null;
        for (t.item_seps.items) |is| {
            if (is.node >= rows.len or rows[is.node].kind != table.Kind.sequence) return error.MalformedTable;
            const text = try optText(t, is.text);
            const sep = if (text.len == 0) null else try own(allocator, &owned, text);
            const open = rows[is.node].flags & table.row_open != 0;
            out[is.node] = if (sep != null or open) .{ .text = sep, .open = open } else null;
        }
    }

    const regions = try allocator.alloc(Document.NodeRegion, t.regions.items.len);
    errdefer allocator.free(regions);
    for (t.regions.items, regions) |r, *o| {
        if (r.node >= rows.len) return error.MalformedTable;
        const sp = try spanIn(source, r.start, r.end);
        o.* = .{ .node_id = r.node, .start = sp.start, .end = sp.end };
    }
    Document.sortRegions(regions);
    const mentions = try allocator.alloc(Document.NodeMention, t.mentions.items.len);
    errdefer allocator.free(mentions);
    for (t.mentions.items, mentions) |m, *o| {
        if (m.node >= rows.len) return error.MalformedTable;
        o.* = .{
            .node_id = m.node,
            .span = try spanIn(source, m.start, m.end),
            .kind = switch (m.kind) {
                table.mention_header => .header,
                table.mention_entry => .entry,
                else => return error.MalformedTable,
            },
        };
    }
    Document.sortMentions(mentions);

    // Comments: a per-node table only when any exist, grown from the flat
    // list. The runs are built in list order, which is the source order the
    // AST asks for.
    if (t.comments.items.len > 0) {
        const nc = try allocator.alloc(AST.NodeComments, rows.len);
        @memset(nc, .{});
        ast.node_comments = nc;
        const leading = try allocator.alloc(std.ArrayList(AST.Comment), rows.len);
        defer allocator.free(leading);
        const dangling = try allocator.alloc(std.ArrayList(AST.Comment), rows.len);
        defer allocator.free(dangling);
        @memset(leading, .empty);
        @memset(dangling, .empty);
        defer for (leading) |*l| l.deinit(allocator);
        defer for (dangling) |*d| d.deinit(allocator);
        for (t.comments.items) |c| {
            if (c.node >= rows.len) return error.MalformedTable;
            const comment: AST.Comment = .{
                .text = try own(allocator, &owned, try optText(t, c.text)),
                .style = switch (c.style) {
                    table.comment_line => .line,
                    table.comment_block => .block,
                    else => return error.MalformedTable,
                },
            };
            switch (c.slot) {
                table.comment_leading => try leading[c.node].append(allocator, comment),
                table.comment_trailing => {
                    if (nc[c.node].trailing != null) return error.MalformedTable;
                    nc[c.node].trailing = comment;
                },
                table.comment_dangling => try dangling[c.node].append(allocator, comment),
                else => return error.MalformedTable,
            }
        }
        for (nc, 0..) |*x, i| {
            x.leading = try leading[i].toOwnedSlice(allocator);
            x.dangling = try dangling[i].toOwnedSlice(allocator);
        }
    }

    if (t.directives.items.len > 0) {
        const directives = try allocator.alloc(AST.TagDirective, t.directives.items.len);
        errdefer allocator.free(directives);
        for (t.directives.items, directives) |r, *d| {
            d.* = .{
                .handle = try own(allocator, &owned, try textOf(t, r.handle)),
                .prefix = try own(allocator, &owned, try textOf(t, r.prefix)),
            };
        }
        ast.tag_directives = directives;
    }

    ast.nodes = nodes;
    ast.owned_strings = try owned.toOwnedSlice(allocator);
    return .{
        .source = source,
        .ast = ast,
        .node_spans = spans,
        .node_anchor_spans = anchor_spans,
        .node_tag_spans = tag_spans,
        .node_marker_spans = marker_spans,
        .node_sep_spans = sep_spans,
        .node_item_seps = item_seps,
        .node_regions = regions,
        .node_mentions = mentions,
    };
}

/// `[start, end)` within `source`, required.
fn spanIn(source: []const u8, start: u32, end: u32) TableError!Span {
    if (start == table.none or end == table.none or start > end or end > source.len) return error.MalformedTable;
    return Span.init(start, end);
}

/// `[start, end)` within `source`, or null when both are `none`.
fn optSpanIn(source: []const u8, start: u32, end: u32) TableError!?Span {
    if (start == table.none and end == table.none) return null;
    return try spanIn(source, start, end);
}

/// A table tag as the AST holds one: a kind tag where the language resolved
/// it to a core type, else the text as written. A tag the language left
/// unstated is read by its text, as before the resolution was the
/// language's: the seven core-schema spellings (`!!int`, `!!str`, …) are
/// kind tags, and anything else a text tag.
fn tagOf(allocator: Allocator, owned: *std.ArrayList([]const u8), t: *const Table, tg: table.Tag) TableError!AST.Tag {
    const kinds = @typeInfo(AST.Tag.KindTag).@"enum".field_names.len;
    if (tg.resolved >= table.tag_kind) {
        if (tg.resolved - table.tag_kind >= kinds) return error.MalformedTable;
        return .{ .kind = @fromBackingInt(@intCast(tg.resolved - table.tag_kind)) };
    }
    const text = t.textOf(tg.text) orelse return error.MalformedTable;
    if (tg.resolved == table.tag_unstated) return tagOfText(allocator, owned, text);
    return .{ .text = try own(allocator, owned, text) };
}

/// A tag as the wire spells it, decoded to what `tableFromDocument` encoded:
/// the seven core-schema spellings (`!!int`, `!!str`, …) are *kind* tags —
/// a runtime language's `: int =` is the same type assertion a compiled
/// fig's is, and the printers honour only that form — and anything else is
/// a verbatim text tag, kept for the format that can spell it.
fn tagOfText(allocator: Allocator, owned: *std.ArrayList([]const u8), t: []const u8) Allocator.Error!AST.Tag {
    const kinds = [_]struct { text: []const u8, kind: AST.Tag.KindTag }{
        .{ .text = "!!null", .kind = .null_ },
        .{ .text = "!!bool", .kind = .boolean },
        .{ .text = "!!str", .kind = .string },
        .{ .text = "!!int", .kind = .integer },
        .{ .text = "!!float", .kind = .float },
        .{ .text = "!!seq", .kind = .sequence },
        .{ .text = "!!map", .kind = .mapping },
    };
    for (kinds) |k| if (std.mem.eql(u8, t, k.text)) return .{ .kind = k.kind };
    return .{ .text = try own(allocator, owned, t) };
}

fn own(allocator: Allocator, owned: *std.ArrayList([]const u8), s: []const u8) Allocator.Error![]const u8 {
    const copy = try allocator.dupe(u8, s);
    errdefer allocator.free(copy);
    try owned.append(allocator, copy);
    return copy;
}

/// The table a `print` receives: `ast` from `root` in pre-order, every text
/// in the table's buffer, spans none. The document's tag directives ride
/// along only when `root` is the document's own root: a fragment printed
/// for a splice has no directives prefix, as a compiled printer's
/// `printNode` writes none. Allocated in `arena`; nothing to free but the
/// arena.
pub fn documentToTable(arena: Allocator, ast: *const AST, root: Node.Id) Allocator.Error!Table {
    var t = Table.init(arena, "");
    try appendRows(&t, ast, root, no_node);
    if (root == ast.root) {
        for (ast.tag_directives) |d| try t.directives.append(arena, .{ .handle = try t.addString(d.handle), .prefix = try t.addString(d.prefix) });
    }
    return t;
}

/// The table a `parse` of `doc`'s source would have to fill: rows as
/// `documentToTable` builds them, plus every source-coupled fact the
/// document records — node spans, item markers, entry separators, item
/// separators, anchor and tag spans, header regions and name mentions. What
/// a twin of a compiled format is held to, row for row.
pub fn fullTable(arena: Allocator, doc: *const Document) Allocator.Error!Table {
    const ast = &doc.ast;
    var t = try documentToTable(arena, ast, ast.root);
    // Pre-order row i is node `ids[i]`; the walk is the one `appendRows`
    // makes.
    var ids: std.ArrayList(Node.Id) = .empty;
    try preorder(arena, ast, ast.root, &ids);
    const row_of = try arena.alloc(u32, ast.nodes.len);
    for (ids.items, 0..) |id, i| row_of[id] = @intCast(i);
    for (ids.items, t.rows.items, 0..) |id, *row, i| {
        const node = ast.nodes[id];
        const sp = doc.span(node);
        row.start = @intCast(sp.start);
        row.end = @intCast(sp.end);
        const n: u32 = @intCast(i);
        if (doc.markerSpan(node)) |m| try t.markers.append(arena, .{ .node = n, .start = @intCast(m.start), .end = @intCast(m.end) });
        if (doc.sepSpan(node)) |m| try t.seps.append(arena, .{ .node = n, .start = @intCast(m.start), .end = @intCast(m.end) });
        if (doc.itemSep(node)) |is| {
            if (is.open) row.flags |= table.row_open;
            if (is.text) |x| try t.item_seps.append(arena, .{ .node = n, .text = try t.addString(x) });
        }
        for (doc.regionsOf(id)) |r| try t.regions.append(arena, .{ .node = n, .start = @intCast(r.start), .end = @intCast(r.end) });
        for (doc.mentionsOf(id)) |m| try t.mentions.append(arena, .{
            .node = n,
            .start = @intCast(m.span.start),
            .end = @intCast(m.span.end),
            .kind = if (m.kind == .header) table.mention_header else table.mention_entry,
        });
    }
    for (t.anchors.items) |*a| {
        if (doc.anchorSpan(ast.nodes[ids.items[a.node]])) |m| {
            a.start = @intCast(m.start);
            a.end = @intCast(m.end);
        }
    }
    for (t.tags.items) |*tg| {
        if (doc.tagSpan(ast.nodes[ids.items[tg.node]])) |m| {
            tg.start = @intCast(m.start);
            tg.end = @intCast(m.end);
        }
    }
    return t;
}

fn preorder(arena: Allocator, ast: *const AST, id: Node.Id, out: *std.ArrayList(Node.Id)) Allocator.Error!void {
    try out.append(arena, id);
    switch (ast.nodes[id].kind) {
        .sequence, .mapping => |first| {
            var next = first;
            while (next) |child| {
                try preorder(arena, ast, child, out);
                next = ast.nodes[child].next_sibling;
            }
        },
        .keyvalue => |kv| {
            try preorder(arena, ast, kv.key, out);
            try preorder(arena, ast, kv.value, out);
        },
        else => {},
    }
}

fn appendRows(t: *Table, ast: *const AST, id: Node.Id, parent: u32) Allocator.Error!void {
    const node = ast.nodes[id];
    const K = table.Kind;
    var row: table.Row = .{ .parent = parent };
    switch (node.kind) {
        .null_ => row.kind = K.null_,
        .boolean => |b| row.kind = if (b) K.true_ else K.false_,
        .string => |x| {
            row.kind = K.string;
            row.text = try t.addString(x);
        },
        .number => |n| {
            row.kind = if (n.kind == .float) K.float else K.int;
            row.text = try t.addString(n.raw);
        },
        .extended => |e| {
            row.kind = K.ext + @as(u8, @intCast(@backingInt(e.kind)));
            row.text = try t.addString(e.text);
        },
        .sequence => row.kind = K.sequence,
        .mapping => row.kind = K.mapping,
        .keyvalue => row.kind = K.keyvalue,
        .alias => |a| {
            row.kind = K.alias;
            row.text = try t.addString(a);
        },
    }
    const index = try t.addRow(row);
    const a = t.allocator;
    if (id < ast.node_anchors.len) {
        if (ast.node_anchors[id]) |name| try t.anchors.append(a, .{ .node = index, .name = try t.addString(name) });
    }
    if (ast.tagOf(id)) |tag| try t.tags.append(a, switch (tag) {
        .text => |x| .{ .node = index, .text = try t.addString(x) },
        // A kind tag is resolved, and spelled as YAML's core schema spells
        // it for a language that writes tags as text.
        .kind => |k| .{ .node = index, .text = try t.addString(tag.spelling()), .resolved = table.tag_kind + @as(u8, @intCast(@backingInt(k))) },
    });
    const nc = ast.comments(id);
    for (nc.leading) |c| try t.comments.append(a, .{ .node = index, .slot = table.comment_leading, .style = @intCast(@backingInt(c.style)), .text = try t.addString(c.text) });
    if (nc.trailing) |c| try t.comments.append(a, .{ .node = index, .slot = table.comment_trailing, .style = @intCast(@backingInt(c.style)), .text = try t.addString(c.text) });
    for (nc.dangling) |c| try t.comments.append(a, .{ .node = index, .slot = table.comment_dangling, .style = @intCast(@backingInt(c.style)), .text = try t.addString(c.text) });

    switch (node.kind) {
        .sequence, .mapping => |first| {
            var next = first;
            while (next) |child| {
                try appendRows(t, ast, child, index);
                next = ast.nodes[child].next_sibling;
            }
        },
        .keyvalue => |kv| {
            try appendRows(t, ast, kv.key, index);
            try appendRows(t, ast, kv.value, index);
        },
        else => {},
    }
}
