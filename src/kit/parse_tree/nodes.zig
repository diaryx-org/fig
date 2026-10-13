//! The tree half of a `ParseTree`: a parser's nodes as it builds them, one
//! `Node` per id with its kind and next sibling inline, and the type-tag,
//! anchor and comment layers beside them as tables indexed by id.
//!
//! This is the form fig's Zig parsers build. What a parse hands fig is rows
//! (`../table.zig`); `ParseTree.emit` writes these nodes as rows, in
//! pre-order, and `toAst` as rows in id order. `docs/proposals/core-only.md`
//! §4.9 has a parser write the rows itself, after which this goes.
//!
//! `Node`, `Tag`, `Comment` and the rest are the `AST`'s own types, so a
//! parser's nodes and a reader's views are the same values.

const Nodes = @This();
const std = @import("std");
const Allocator = std.mem.Allocator;
const util = @import("../kit.zig");
const AST = @import("../ast/ast.zig");
const table = @import("../table.zig");
const Span = @import("../span.zig");

pub const Node = AST.Node;
pub const Tag = AST.Tag;
pub const Comment = AST.Comment;
pub const NodeComments = AST.NodeComments;
pub const Anchor = AST.Anchor;
pub const TagDirective = AST.TagDirective;
pub const PathSegment = AST.PathSegment;
pub const SerializeOptions = AST.SerializeOptions;
pub const SerializeError = AST.SerializeError;

/// Construction, with a span per node (fig's parser builds through it).
pub const Builder = @import("builder.zig");

// The read path, as on `AST`.
const reader = @import("../ast/reader.zig");
pub const child = reader.child;
pub const next = reader.next;
pub const lastChild = reader.lastChild;
pub const firstChildKey = reader.firstChildKey;
pub const getNodeByPath = reader.getNodeByPath;
pub const getKeyByPath = reader.getKeyByPath;
pub const getValByPath = reader.getValByPath;
pub const resolveAlias = reader.resolveAlias;
pub const resolveDeep = reader.resolveDeep;
pub const mergedChild = reader.mergedChild;

allocator: Allocator,
owned_strings: []const []const u8 = &.{},
root: Node.Id,
merge_key: ?[]const u8 = null,
/// Complete node tree, such that `nodes[node.id] == node`.
nodes: []const Node,
/// Indexed by node id: the node's type tag, or null. Empty when none.
node_tags: []const ?Tag = &.{},
/// Indexed by node id: the anchor name defined on the node, or null. Empty
/// when none.
node_anchors: []const ?[]const u8 = &.{},
/// Anchor definitions in id order.
anchors: []const Anchor = &.{},
/// Indexed by node id, beside `node_tags`: what the language resolved each
/// text tag to, and whether the node agrees with it. Empty when the parser
/// resolves none, and then every text tag is unstated.
node_tag_resolutions: []const TagResolution = &.{},
/// `%TAG` declarations, in source order.
tag_directives: []const TagDirective = &.{},
/// Indexed by node id: the comments bound to the node. Empty when none.
node_comments: []const NodeComments = &.{},

/// What a language made of a text tag: `table.tag_*`, and whether the
/// node's text agrees with the kind it names (`!!int` on `1 - 3` does not).
pub const TagResolution = struct { resolved: u8 = table.tag_unstated, agrees: bool = true };

/// The resolution recorded for node `id`'s tag.
pub fn resolutionOf(self: *const Nodes, id: Node.Id) TagResolution {
    return if (id < self.node_tag_resolutions.len) self.node_tag_resolutions[id] else .{};
}

pub fn deinit(self: *Nodes) void {
    self.allocator.free(self.node_tag_resolutions);
    for (self.owned_strings) |string| self.allocator.free(string);
    self.allocator.free(self.owned_strings);
    self.allocator.free(self.nodes);
    // The side tables' strings alias the source or `owned_strings`.
    self.allocator.free(self.node_tags);
    self.allocator.free(self.node_anchors);
    self.allocator.free(self.anchors);
    self.allocator.free(self.tag_directives);
    for (self.node_comments) |nc| {
        self.allocator.free(nc.leading);
        self.allocator.free(nc.dangling);
    }
    self.allocator.free(self.node_comments);
}

/// How many nodes the tree has.
pub fn nodeCount(self: *const Nodes) usize {
    return self.nodes.len;
}

/// Node `id`.
pub fn node(self: *const Nodes, id: Node.Id) Node {
    return self.nodes[id];
}

/// The anchors defined, in id order.
pub fn anchorIterator(self: *const Nodes) AnchorIterator {
    return .{ .items = self.anchors };
}

pub const AnchorIterator = struct {
    items: []const Anchor,
    i: usize = 0,

    pub fn next(it: *AnchorIterator) ?Anchor {
        if (it.i >= it.items.len) return null;
        defer it.i += 1;
        return it.items[it.i];
    }
};

/// The type tag on node `id`, or null.
pub fn tagOf(self: *const Nodes, id: Node.Id) ?Tag {
    return if (id < self.node_tags.len) self.node_tags[id] else null;
}

/// The comments bound to `id`.
pub fn comments(self: *const Nodes, id: Node.Id) NodeComments {
    return if (id < self.node_comments.len) self.node_comments[id] else .{};
}

/// See `AST.leadingCommentAnchor`.
pub fn leadingCommentAnchor(self: *const Nodes, id: Node.Id) Node.Id {
    return switch (self.nodes[id].kind) {
        .keyvalue => |kv| kv.key,
        else => id,
    };
}

/// See `AST.trailingCommentAnchor`.
pub fn trailingCommentAnchor(self: *const Nodes, id: Node.Id) Node.Id {
    return switch (self.nodes[id].kind) {
        .keyvalue => |kv| kv.value,
        else => id,
    };
}

/// The nodes as an `AST` of their own, row `i` node `i` and every text in
/// the AST's buffer: for a caller that built a tree with no source. The
/// nodes are not consumed.
pub fn toAst(self: *const Nodes, allocator: Allocator) Allocator.Error!AST {
    var t = table.Table.init(allocator, "");
    errdefer t.deinit();
    try writeRows(&t, self, null);
    return AST.own(allocator, &t, self.root, self.merge_key);
}

/// Write every node as a row in id order into the empty table `t`, with
/// its links, and every side fact. `spans`, when given, is each node's.
pub fn writeRows(t: *table.Table, self: *const Nodes, spans: ?[]const Span) Allocator.Error!void {
    std.debug.assert(t.rows.items.len == 0);
    const a = t.allocator;
    const rows = try t.addRows(self.nodes.len);
    const K = table.Kind;
    for (self.nodes, rows, 0..) |n, *row, i| {
        row.* = .{};
        if (spans) |s| if (i < s.len) {
            row.start = @intCast(s[i].start);
            row.end = @intCast(s[i].end);
        };
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
            .sequence => |first| {
                row.kind = K.sequence;
                row.first_child = first orelse table.none;
            },
            .mapping => |first| {
                row.kind = K.mapping;
                row.first_child = first orelse table.none;
            },
            .keyvalue => row.kind = K.keyvalue,
            .alias => |x| {
                row.kind = K.alias;
                row.text = try t.text(x);
            },
        }
    }
    // Links: a container's children are its sibling chain; a keyvalue's
    // are its key, then its value.
    const all = t.rows.items;
    for (self.nodes, 0..) |n, i| {
        switch (n.kind) {
            .sequence, .mapping => |first| {
                var cursor = first;
                while (cursor) |c| : (cursor = self.nodes[c].next_sibling) {
                    all[c].parent = @intCast(i);
                    all[c].next_sibling = self.nodes[c].next_sibling orelse table.none;
                }
            },
            .keyvalue => |kv| {
                all[i].first_child = kv.key;
                all[kv.key].parent = @intCast(i);
                all[kv.key].next_sibling = kv.value;
                all[kv.value].parent = @intCast(i);
                all[kv.value].next_sibling = table.none;
            },
            else => {},
        }
    }
    for (self.node_anchors, 0..) |name, i| if (name) |x| {
        try t.anchors.append(a, .{ .node = @intCast(i), .name = try t.text(x) });
    };
    for (self.node_tags, 0..) |tag, i| if (tag) |x| try t.tags.append(a, try tagRow(t, @intCast(i), x, self.resolutionOf(@intCast(i))));
    for (self.node_comments, 0..) |nc, i| try appendComments(t, @intCast(i), nc);
    for (self.tag_directives) |d| try t.directives.append(a, .{ .handle = try t.text(d.handle), .prefix = try t.text(d.prefix) });
}

/// A tag as a row states it: a kind tag resolved, spelled as YAML's core
/// schema spells it for a language that writes tags as text; a text tag as
/// written, with what the language resolved it to (`res`), or unstated.
pub fn tagRow(t: *table.Table, id: u32, tag: Tag, res: TagResolution) Allocator.Error!table.Tag {
    return switch (tag) {
        .text => |x| .{ .node = id, .text = try t.text(x), .resolved = res.resolved, .agrees = @intFromBool(res.agrees) },
        .kind => |k| .{ .node = id, .text = try t.text(tag.spelling()), .resolved = table.tag_kind + @as(u8, @intCast(@backingInt(k))) },
    };
}

/// `nc` as comment facts on node `id`: leading, trailing, dangling.
pub fn appendComments(t: *table.Table, id: u32, nc: NodeComments) Allocator.Error!void {
    const a = t.allocator;
    for (nc.leading) |c| try t.comments.append(a, .{ .node = id, .slot = table.comment_leading, .style = @intCast(@backingInt(c.style)), .text = try t.text(c.text) });
    if (nc.trailing) |c| try t.comments.append(a, .{ .node = id, .slot = table.comment_trailing, .style = @intCast(@backingInt(c.style)), .text = try t.text(c.text) });
    for (nc.dangling) |c| try t.comments.append(a, .{ .node = id, .slot = table.comment_dangling, .style = @intCast(@backingInt(c.style)), .text = try t.text(c.text) });
}

/// Function to tell if two trees are equal abstractly.
pub fn eql(self: Nodes, b: Nodes) bool {
    return self.root == b.root and util.eql(Node, self.nodes, b.nodes);
}
