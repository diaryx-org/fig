//! The node table and `Document`, each made from the other: the table a
//! language's parse fills becomes a `Document` (`tableToDocument`, which
//! adopts it), a print's table is read in place (`viewToDocument`), and a
//! `Document` becomes the table a print reads (`documentToTable`) or a parse
//! would have filled (`fullTable`). The kit's, so that a language built apart
//! from core can read the table it is handed back.

const std = @import("std");
const Allocator = std.mem.Allocator;
const AST = @import("ast/ast.zig");
const Document = @import("document.zig");
const table = @import("table.zig");
const Table = table.Table;
const Node = AST.Node;
const no_node = table.none;

pub const TableError = AST.TableError;
pub const Spans = AST.Spans;

/// A `Document` over `source` from a parse's table, which is checked and
/// adopted as the tree's storage (`AST.adopt`): nothing is copied, and `t`
/// is left empty.
pub fn tableToDocument(allocator: Allocator, source: []const u8, t: *Table) TableError!Document {
    const ast = try AST.adopt(allocator, source, t, .required);
    return .{ .source = source, .ast = ast };
}

/// A `Document` over the table a print is handed, read in place
/// (`AST.borrow`): the view must outlive it. Its rows carry no spans.
pub fn viewToDocument(allocator: Allocator, source: []const u8, v: *const table.View) TableError!Document {
    const ast = try AST.borrow(allocator, source, v);
    return .{ .source = source, .ast = ast };
}

/// The table a `print` receives: `ast` from `root` in pre-order, every text
/// in the table's buffer, spans none, the child links filled. The document's
/// tag directives ride along only when `root` is the document's own root: a
/// fragment printed for a splice has no directives prefix, as a compiled
/// printer's `printNode` writes none. Allocated in `arena`; nothing to free
/// but the arena.
pub fn documentToTable(arena: Allocator, ast: *const AST, root: Node.Id) Allocator.Error!Table {
    var t = Table.init(arena, "");
    try write(&t, ast, null, root);
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
    var t = Table.init(arena, "");
    try write(&t, &doc.ast, doc, doc.ast.root);
    for (doc.ast.tag_directives) |d| try t.directives.append(arena, .{ .handle = try t.text(d.handle), .prefix = try t.text(d.prefix) });
    return t;
}

/// `ast` from `root`, in pre-order, into the empty table `t`, with parents
/// and child links; with `doc`, its spans and source-coupled facts too.
fn write(t: *Table, ast: *const AST, doc: ?*const Document, root: Node.Id) Allocator.Error!void {
    std.debug.assert(t.rows.items.len == 0);
    const a = t.allocator;
    // Pre-order: each node's row, and its parent's.
    const Item = struct { id: Node.Id, parent: u32 };
    var stack: std.ArrayList(Item) = .empty;
    defer stack.deinit(a);
    var ids: std.ArrayList(Node.Id) = .empty;
    defer ids.deinit(a);
    var kids: std.ArrayList(Node.Id) = .empty;
    defer kids.deinit(a);
    try stack.append(a, .{ .id = root, .parent = no_node });
    // The last child row of each row, while its subtree is being written.
    var last: std.ArrayList(u32) = .empty;
    defer last.deinit(a);
    while (stack.pop()) |item| {
        const index: u32 = @intCast(t.rows.items.len);
        const n = ast.node(item.id);
        try t.rows.append(a, try rowOf(t, ast, n, item.parent));
        try ids.append(a, item.id);
        try last.append(a, no_node);
        if (item.parent != no_node) {
            const p = item.parent;
            if (last.items[p] != no_node) t.rows.items[last.items[p]].next_sibling = index else t.rows.items[p].first_child = index;
            last.items[p] = index;
        }
        // Children, pushed last-first so they pop in order.
        kids.clearRetainingCapacity();
        switch (n.kind) {
            .sequence, .mapping => |first| {
                var next = first;
                while (next) |c| : (next = ast.node(c).next_sibling) try kids.append(a, c);
            },
            .keyvalue => |kv| try kids.appendSlice(a, &.{ kv.key, kv.value }),
            else => {},
        }
        var k = kids.items.len;
        while (k > 0) {
            k -= 1;
            try stack.append(a, .{ .id = kids.items[k], .parent = index });
        }
    }

    for (ids.items, 0..) |id, i| {
        const n: u32 = @intCast(i);
        if (ast.anchorOf(id)) |name| {
            var f: table.Anchor = .{ .node = n, .name = try t.text(name) };
            if (doc) |d| if (d.anchorSpan(ast.node(id))) |s| {
                f.start = @intCast(s.start);
                f.end = @intCast(s.end);
            };
            try t.anchors.append(a, f);
        }
        if (ast.tagFact(id)) |src| {
            // The tag as the tree states it: its text as written — a kind
            // tag with none spelled as YAML's core schema spells it, for a
            // language that writes tags as text — and its resolution.
            // An unstated core-schema spelling is read as the kind it names,
            // and goes out resolved.
            var f: table.Tag = .{ .node = n, .text = undefined, .resolved = src.resolved, .agrees = src.agrees };
            const tag = ast.tagOfFact(src);
            if (f.resolved == table.tag_unstated) if (tag == .kind) {
                f.resolved = table.tag_kind + @as(u8, @intCast(@backingInt(tag.kind)));
            };
            f.text = if (ast.textOf(src.text)) |x| try t.text(x) else try t.text(tag.spelling());
            if (doc) |d| if (d.tagSpan(ast.node(id))) |s| {
                f.start = @intCast(s.start);
                f.end = @intCast(s.end);
            };
            try t.tags.append(a, f);
        }
        const nc = ast.comments(id);
        for (nc.leading) |c| try t.comments.append(a, .{ .node = n, .slot = table.comment_leading, .style = @intCast(@backingInt(c.style)), .text = try t.text(c.text) });
        if (nc.trailing) |c| try t.comments.append(a, .{ .node = n, .slot = table.comment_trailing, .style = @intCast(@backingInt(c.style)), .text = try t.text(c.text) });
        for (nc.dangling) |c| try t.comments.append(a, .{ .node = n, .slot = table.comment_dangling, .style = @intCast(@backingInt(c.style)), .text = try t.text(c.text) });

        const d = doc orelse continue;
        const node = ast.node(id);
        const row = &t.rows.items[i];
        const sp = d.span(node);
        row.start = @intCast(sp.start);
        row.end = @intCast(sp.end);
        if (d.markerSpan(node)) |m| try t.markers.append(a, .{ .node = n, .start = @intCast(m.start), .end = @intCast(m.end) });
        if (d.sepSpan(node)) |m| try t.seps.append(a, .{ .node = n, .start = @intCast(m.start), .end = @intCast(m.end) });
        if (d.itemSep(node)) |is| {
            if (is.open) row.flags |= table.row_open;
            if (is.text) |x| try t.item_seps.append(a, .{ .node = n, .text = try t.text(x) });
        }
        for (d.regionsOf(id)) |r| try t.regions.append(a, .{ .node = n, .start = r.start, .end = r.end });
        var mentions = d.mentionsOf(id);
        while (mentions.next()) |m| try t.mentions.append(a, .{
            .node = n,
            .start = @intCast(m.span.start),
            .end = @intCast(m.span.end),
            .kind = if (m.kind == .header) table.mention_header else table.mention_entry,
        });
    }
}

fn rowOf(t: *Table, ast: *const AST, n: Node, parent: u32) Allocator.Error!table.Row {
    _ = ast;
    const K = table.Kind;
    var row: table.Row = .{ .parent = parent };
    switch (n.kind) {
        .null_ => row.kind = K.null_,
        .boolean => |b| row.kind = if (b) K.true_ else K.false_,
        .string => |x| {
            row.kind = K.string;
            row.text = try t.text(x);
        },
        .number => |num| {
            row.kind = if (num.kind == .float) K.float else K.int;
            row.text = try t.text(num.raw);
        },
        .extended => |e| {
            row.kind = K.ext + @as(u8, @intCast(@backingInt(e.kind)));
            row.text = try t.text(e.text);
        },
        .sequence => row.kind = K.sequence,
        .mapping => row.kind = K.mapping,
        .keyvalue => row.kind = K.keyvalue,
        .alias => |x| {
            row.kind = K.alias;
            row.text = try t.text(x);
        },
    }
    return row;
}
