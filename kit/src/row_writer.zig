//! A Zig parser's way of writing the node table fig hands it
//! (docs/proposals/core-only.md §4.9): a node is a row, added in whatever
//! order the parser meets it, and linked to its siblings and children
//! through the row's own `first_child` and `next_sibling` — the links a
//! parser building a tree of `Node`s kept in the nodes. `finish` then puts
//! the rows in the pre-order a parse's table is read in, each naming its
//! parent, and moves every side fact to its node's new row; fig derives the
//! links again when it adopts the table, so a parser's are scratch.
//!
//! A text is written as `Table.text` writes it: a slice of the source stays
//! there, any other is copied into the table's buffer. A slice `node` hands
//! back may point into that buffer, which moves as it grows: read it, don't
//! keep it.

const RowWriter = @This();
const std = @import("std");
const Allocator = std.mem.Allocator;
const table = @import("table.zig");
const AST = @import("ast/ast.zig");
const Document = @import("document.zig");
const Span = @import("span.zig");
const Node = AST.Node;
const none = table.none;

t: *table.Table,

pub fn init(t: *table.Table) RowWriter {
    return .{ .t = t };
}

pub fn allocator(self: RowWriter) Allocator {
    return self.t.allocator;
}

/// How many rows have been written.
pub fn count(self: RowWriter) usize {
    return self.t.rows.items.len;
}

/// A new node of `kind` over `span`; its id is its row. A container's first
/// child and a keyvalue's key and value are linked as `kind` says.
pub fn add(self: RowWriter, kind: Node.Kind, span: Span) Allocator.Error!Node.Id {
    const id = try self.t.addRow(.{ .start = @intCast(span.start), .end = @intCast(span.end) });
    try self.set(id, kind);
    return id;
}

/// Make node `id` one of `kind`, replacing what it was: a scalar's text is
/// written again, a container's first child and a keyvalue's key and value
/// relinked. A sibling link is kept.
pub fn set(self: RowWriter, id: Node.Id, kind: Node.Kind) Allocator.Error!void {
    const K = table.Kind;
    var r = &self.t.rows.items[id];
    r.first_child = none;
    r.text = .absent;
    switch (kind) {
        .null_ => r.kind = K.null_,
        .boolean => |b| r.kind = if (b) K.true_ else K.false_,
        .string => |x| {
            const text = try self.t.text(x);
            r = &self.t.rows.items[id];
            r.kind = K.string;
            r.text = text;
        },
        .number => |n| {
            const text = try self.t.text(n.raw);
            r = &self.t.rows.items[id];
            r.kind = if (n.kind == .float) K.float else K.int;
            r.text = text;
        },
        .extended => |e| {
            const text = try self.t.text(e.text);
            r = &self.t.rows.items[id];
            r.kind = K.ext + @as(u8, @intCast(@backingInt(e.kind)));
            r.text = text;
        },
        .alias => |x| {
            const text = try self.t.text(x);
            r = &self.t.rows.items[id];
            r.kind = K.alias;
            r.text = text;
        },
        .sequence => |first| {
            r.kind = K.sequence;
            r.first_child = first orelse none;
        },
        .mapping => |first| {
            r.kind = K.mapping;
            r.first_child = first orelse none;
        },
        .keyvalue => |kv| {
            r.kind = K.keyvalue;
            r.first_child = kv.key;
            const rows = self.t.rows.items;
            rows[kv.key].next_sibling = kv.value;
            rows[kv.key].parent = id;
            rows[kv.value].next_sibling = none;
            rows[kv.value].parent = id;
        },
    }
}

/// Node `id` as it stands: its kind, and its next sibling — none for a
/// keyvalue's key or value.
pub fn node(self: RowWriter, id: Node.Id) Node {
    const rows = self.t.rows.items;
    const r = rows[id];
    const in_entry = r.parent != none and rows[r.parent].kind == table.Kind.keyvalue;
    return .{
        .id = id,
        .kind = AST.rowKind(self.t.source, self.t.strings.items, rows, r),
        .next_sibling = if (in_entry or r.next_sibling == none) null else r.next_sibling,
    };
}

/// Node `id`'s kind: `node(id).kind`.
pub fn kindOf(self: RowWriter, id: Node.Id) Node.Kind {
    const rows = self.t.rows.items;
    return AST.rowKind(self.t.source, self.t.strings.items, rows, rows[id]);
}

/// Link `next` after `id` among its siblings, or end the chain at `id`.
pub fn setNext(self: RowWriter, id: Node.Id, next: ?Node.Id) void {
    self.t.rows.items[id].next_sibling = next orelse none;
}

/// Node `id`'s span.
pub fn spanOf(self: RowWriter, id: Node.Id) Span {
    const r = self.t.rows.items[id];
    return Span.init(r.start, r.end);
}

pub fn setSpan(self: RowWriter, id: Node.Id, span: Span) void {
    const r = &self.t.rows.items[id];
    r.start = @intCast(span.start);
    r.end = @intCast(span.end);
}

/// Mark sequence `id` as having no delimiters (`Document.ItemSep.open`).
pub fn setOpen(self: RowWriter, id: Node.Id) void {
    self.t.rows.items[id].flags |= table.row_open;
}

// ── Side facts ──────────────────────────────────────────────────────────────
// Appended as the parser meets them; a later fact of the same kind on the
// same node replaces an earlier when fig adopts the table.

pub fn anchor(self: RowWriter, id: Node.Id, name: []const u8, span: ?Span) Allocator.Error!void {
    try self.t.anchors.append(self.t.allocator, .{
        .node = id,
        .name = try self.t.text(name),
        .start = if (span) |s| @intCast(s.start) else none,
        .end = if (span) |s| @intCast(s.end) else none,
    });
}

/// A tag on `id`: a kind tag resolved and spelled as YAML's core schema
/// spells it; a text tag as written, resolved as `resolved` says
/// (`table.tag_*`), with whether the node agrees.
pub fn tag(self: RowWriter, id: Node.Id, t: AST.Tag, resolved: u8, agrees: bool, span: ?Span) Allocator.Error!void {
    var f: table.Tag = switch (t) {
        .text => |x| .{ .node = id, .text = try self.t.text(x), .resolved = resolved, .agrees = @intFromBool(agrees) },
        .kind => |k| .{ .node = id, .text = try self.t.text(t.spelling()), .resolved = table.tag_kind + @as(u8, @intCast(@backingInt(k))) },
    };
    if (span) |s| {
        f.start = @intCast(s.start);
        f.end = @intCast(s.end);
    }
    try self.t.tags.append(self.t.allocator, f);
}

/// The `-`/`*` marker that introduces item `id` (`Document.markerSpan`).
pub fn marker(self: RowWriter, id: Node.Id, span: Span) Allocator.Error!void {
    try self.t.markers.append(self.t.allocator, .{ .node = id, .start = @intCast(span.start), .end = @intCast(span.end) });
}

/// The separator between entry `id`'s key and value (`Document.sepSpan`).
pub fn sep(self: RowWriter, id: Node.Id, span: Span) Allocator.Error!void {
    try self.t.seps.append(self.t.allocator, .{ .node = id, .start = @intCast(span.start), .end = @intCast(span.end) });
}

/// The separator a separated sequence writes between its items.
pub fn itemSep(self: RowWriter, id: Node.Id, text: []const u8) Allocator.Error!void {
    try self.t.item_seps.append(self.t.allocator, .{ .node = id, .text = try self.t.text(text) });
}

/// A header line of section `id` (`Document.regionsOf`), whole.
pub fn region(self: RowWriter, id: Node.Id, start: usize, end: usize) Allocator.Error!void {
    try self.t.regions.append(self.t.allocator, .{ .node = id, .start = @intCast(start), .end = @intCast(end) });
}

/// A place section `id`'s name, or item `id`'s path, is written
/// (`Document.mentionsOf`).
pub fn mention(self: RowWriter, id: Node.Id, span: Span, kind: Document.MentionKind) Allocator.Error!void {
    try self.t.mentions.append(self.t.allocator, .{
        .node = id,
        .start = @intCast(span.start),
        .end = @intCast(span.end),
        .kind = kind.byte(),
    });
}

/// `nc` as comments on `id`.
pub fn comments(self: RowWriter, id: Node.Id, nc: AST.NodeComments) Allocator.Error!void {
    for (nc.leading) |c| try self.comment(id, table.comment_leading, c);
    if (nc.trailing) |c| try self.comment(id, table.comment_trailing, c);
    for (nc.dangling) |c| try self.comment(id, table.comment_dangling, c);
}

/// One comment on `id`, in `slot` (`table.comment_*`), after any already
/// in that slot.
pub fn comment(self: RowWriter, id: Node.Id, slot: u8, c: AST.Comment) Allocator.Error!void {
    try self.t.comments.append(self.t.allocator, .{ .node = id, .slot = slot, .style = @intCast(@backingInt(c.style)), .text = try self.t.text(c.text) });
}

/// A `%TAG` declaration.
pub fn directive(self: RowWriter, handle: []const u8, prefix: []const u8) Allocator.Error!void {
    try self.t.directives.append(self.t.allocator, .{ .handle = try self.t.text(handle), .prefix = try self.t.text(prefix) });
}

// ── Finishing ───────────────────────────────────────────────────────────────

/// Put the rows in pre-order from `root`, as fig reads a parse's table:
/// row 0 the root, each row after its parent and naming it, a container's
/// children in their sibling order, a keyvalue's key before its value.
/// Every side fact moves with its node; a row the tree does not reach, and
/// its facts, are dropped. Rows already in pre-order stay where they are.
pub fn finish(self: RowWriter, root: Node.Id) Allocator.Error!void {
    const a = self.t.allocator;
    self.lastTrailing();
    const rows = self.t.rows.items;
    // The pre-order walk, and each row's place in it.
    const order = try a.alloc(u32, rows.len);
    defer a.free(order);
    const place = try a.alloc(u32, rows.len);
    defer a.free(place);
    @memset(place, none);
    var stack: std.ArrayList(struct { id: u32, parent: u32 }) = .empty;
    defer stack.deinit(a);
    var kids: std.ArrayList(u32) = .empty;
    defer kids.deinit(a);
    var n: usize = 0;
    var in_place = true;
    try stack.append(a, .{ .id = root, .parent = none });
    while (stack.pop()) |item| {
        if (place[item.id] != none) continue; // a node linked twice is written once
        place[item.id] = @intCast(n);
        order[n] = item.id;
        if (item.id != n) in_place = false;
        rows[item.id].parent = item.parent;
        n += 1;
        kids.clearRetainingCapacity();
        var c = rows[item.id].first_child;
        if (table.Kind.isContainer(rows[item.id].kind)) {
            while (c != none) : (c = rows[c].next_sibling) try kids.append(a, c);
        }
        var k = kids.items.len;
        while (k > 0) {
            k -= 1;
            try stack.append(a, .{ .id = kids.items[k], .parent = item.id });
        }
    }
    if (in_place and n == rows.len) {
        // Parents name rows that did not move.
        return;
    }
    const out = try a.alloc(table.Row, n);
    for (order[0..n], out) |old, *r| {
        r.* = rows[old];
        r.parent = if (r.parent == none) none else place[r.parent];
    }
    self.t.rows.deinit(a);
    self.t.rows = .fromOwnedSlice(out);
    remap(table.Anchor, &self.t.anchors, place);
    remap(table.Tag, &self.t.tags, place);
    remap(table.SpanFact, &self.t.markers, place);
    remap(table.SpanFact, &self.t.seps, place);
    remap(table.ItemSep, &self.t.item_seps, place);
    remap(table.Comment, &self.t.comments, place);
    remap(table.SpanFact, &self.t.regions, place);
    remap(table.Mention, &self.t.mentions, place);
}

/// Sort the comments by node and slot, keeping their order within a slot,
/// and keep only the last trailing comment a node was given: a parser that
/// sets one twice means the second.
fn lastTrailing(self: RowWriter) void {
    const cs = &self.t.comments;
    std.mem.sort(table.Comment, cs.items, {}, struct {
        fn lt(_: void, x: table.Comment, y: table.Comment) bool {
            return x.node < y.node or (x.node == y.node and x.slot < y.slot);
        }
    }.lt);
    var w: usize = 0;
    for (cs.items, 0..) |c, i| {
        const next = if (i + 1 < cs.items.len) cs.items[i + 1] else null;
        if (c.slot == table.comment_trailing) if (next) |nx| {
            if (nx.node == c.node and nx.slot == c.slot) continue;
        };
        cs.items[w] = c;
        w += 1;
    }
    cs.shrinkRetainingCapacity(w);
}

/// Move each fact in `list` to its node's new row, dropping a fact whose
/// node the tree does not reach.
fn remap(comptime F: type, list: *std.ArrayList(F), place: []const u32) void {
    var w: usize = 0;
    for (list.items) |f| {
        if (f.node >= place.len or place[f.node] == none) continue;
        var g = f;
        g.node = place[f.node];
        list.items[w] = g;
        w += 1;
    }
    list.shrinkRetainingCapacity(w);
}

/// The `Document` a parser's finished table is, adopted. A parser writes a
/// well-formed tree, so a refusal is a bug in it.
pub fn adopt(allocator_: Allocator, source: []const u8, t: *table.Table) Allocator.Error!Document {
    const ast = AST.adopt(allocator_, source, t, .required) catch |e| switch (e) {
        error.OutOfMemory => return error.OutOfMemory,
        else => @panic("a parser wrote a malformed table"),
    };
    return .{ .source = source, .ast = ast };
}

test "finish puts rows written out of order into pre-order, facts and all" {
    const src = "{a: [x]}";
    var t = table.Table.init(std.testing.allocator, src);
    defer t.deinit();
    const w = init(&t);
    // Leaves first, as a bottom-up parser writes them.
    const x = try w.add(.{ .string = src[5..6] }, Span.init(5, 6));
    const seq = try w.add(.{ .sequence = x }, Span.init(4, 7));
    const key = try w.add(.{ .string = src[1..2] }, Span.init(1, 2));
    const kv = try w.add(.{ .keyvalue = .{ .key = key, .value = seq } }, Span.init(1, 7));
    const map = try w.add(.{ .mapping = kv }, Span.init(0, 8));
    const orphan = try w.add(.null_, Span.init(0, 0));
    try w.marker(x, Span.init(4, 5));
    try w.comment(orphan, table.comment_leading, .{ .text = "gone" });
    try w.finish(map);

    var ast = try AST.adopt(std.testing.allocator, src, &t, .required);
    defer ast.deinit();
    try std.testing.expectEqual(@as(usize, 5), ast.nodeCount());
    const entry = ast.node(ast.node(0).kind.mapping.?).kind.keyvalue;
    try std.testing.expectEqualStrings("a", ast.node(entry.key).kind.string);
    const item = ast.node(entry.value).kind.sequence.?;
    try std.testing.expectEqualStrings("x", ast.node(item).kind.string);
    try std.testing.expectEqual(@as(usize, 1), ast.marker_facts.len);
    try std.testing.expectEqual(item, ast.marker_facts[0].node);
    try std.testing.expect(!ast.hasComments());
}
