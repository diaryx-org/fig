//! Builder — programmatic construction of an AST, the write-path mirror of the
//! read-path navigation helpers in `reader.zig`. This file IS the `Builder`
//! type (re-exported as `AST.Builder` from `ast.zig`).
//!
//! It writes rows: id is row index, a container's children are linked
//! through `first_child` and `next_sibling`, and every string is copied into
//! the table's buffer, since a built tree has no source to borrow from.
//!
//! Construction is bottom-up: add the children, then add the container from
//! their ids. `finish` freezes the result into an owned `AST`.
//!
//! Each node id must be placed in exactly one container. A row carries a
//! single parent and `next_sibling`, so reusing an id in two containers
//! corrupts the tree.

const Builder = @This();
const std = @import("std");
const Allocator = std.mem.Allocator;

const AST = @import("ast.zig");
const table = @import("../table.zig");
const Node = AST.Node;
const Comment = AST.Comment;
const NodeComments = AST.NodeComments;
const Span = @import("../span.zig");
const none = table.none;

allocator: Allocator,
t: table.Table,
/// Indexed by node id, grown per node: the comments set on it, kept apart
/// until `finish` so that `setComments` can replace them.
comments: std.ArrayList(PendingComments) = .empty,
any_comments: bool = false,
/// Type tags by node id, grown on demand: tags are rare.
node_tags: std.ArrayList(?PendingTag) = .empty,
any_tags: bool = false,
/// What `view` last made, freed by the next `view` or `deinit`.
snapshot: ?AST = null,

pub const Entry = struct { key: Node.Id, value: Node.Id };

/// A node's comments, as growable runs, their texts in the build's buffer.
const PendingComments = struct {
    leading: std.ArrayList(PendingComment) = .empty,
    trailing: ?PendingComment = null,
    dangling: std.ArrayList(PendingComment) = .empty,
};

const PendingComment = struct { text: table.Text, style: Comment.Style };

const PendingTag = union(enum) { kind: AST.Tag.KindTag, text: table.Text };

pub fn init(allocator: Allocator) Builder {
    return .{ .allocator = allocator, .t = table.Table.init(allocator, "") };
}

pub fn deinit(self: *Builder) void {
    self.t.deinit();
    for (self.comments.items) |*nc| {
        nc.leading.deinit(self.allocator);
        nc.dangling.deinit(self.allocator);
    }
    self.comments.deinit(self.allocator);
    self.node_tags.deinit(self.allocator);
    if (self.snapshot) |*s| s.deinit();
}

/// How many nodes have been added.
pub fn nodeCount(self: *const Builder) usize {
    return self.t.rows.items.len;
}

/// Set `id`'s source span (a byte range into the input `id` was read from).
pub fn setSpan(self: *Builder, id: Node.Id, span: Span) void {
    self.t.rows.items[id].start = @intCast(span.start);
    self.t.rows.items[id].end = @intCast(span.end);
}

/// Attach comments to an already-added node, REPLACING any previously set on
/// `id` (pass `.{}` to clear). Every comment's text is copied. For attaching
/// comments one at a time, prefer `addLeadingComment`/`addDanglingComment`/
/// `setTrailingComment`.
pub fn setComments(self: *Builder, id: Node.Id, node_comments: NodeComments) Allocator.Error!void {
    const slot = &self.comments.items[id];
    slot.leading.clearRetainingCapacity();
    slot.dangling.clearRetainingCapacity();
    for (node_comments.leading) |c| try slot.leading.append(self.allocator, try self.ownComment(c));
    for (node_comments.dangling) |c| try slot.dangling.append(self.allocator, try self.ownComment(c));
    slot.trailing = if (node_comments.trailing) |c| try self.ownComment(c) else null;
    self.any_comments = true;
}

/// Append one comment to the run rendered ABOVE `id` (its `leading` run), in
/// call order. The text is copied.
///
/// Comments are format-agnostic: a target format without comment syntax here
/// (plain JSON, or compact JSON5/JSONC/ZON) drops them on serialize. That is a
/// reported loss, not an error — see `fig.Diagnostics.analyze` — so this never
/// rejects based on a format.
pub fn addLeadingComment(self: *Builder, id: Node.Id, comment: Comment) Allocator.Error!void {
    try self.comments.items[id].leading.append(self.allocator, try self.ownComment(comment));
    self.any_comments = true;
}

/// Append one comment to `id`'s `dangling` run — comments at the END of a
/// container's body, after its last child. Only meaningful on a container
/// node; see `NodeComments`.
pub fn addDanglingComment(self: *Builder, id: Node.Id, comment: Comment) Allocator.Error!void {
    try self.comments.items[id].dangling.append(self.allocator, try self.ownComment(comment));
    self.any_comments = true;
}

/// Set `id`'s single same-line `trailing` comment, replacing any previous
/// one. The text is copied.
pub fn setTrailingComment(self: *Builder, id: Node.Id, comment: Comment) Allocator.Error!void {
    self.comments.items[id].trailing = try self.ownComment(comment);
    self.any_comments = true;
}

/// Attach a cross-format type tag to an already-added node, replacing any
/// tag previously set on `id`.
pub fn setTag(self: *Builder, id: Node.Id, tag: AST.Tag) Allocator.Error!void {
    while (self.node_tags.items.len <= id) try self.node_tags.append(self.allocator, null);
    self.node_tags.items[id] = switch (tag) {
        .kind => |k| .{ .kind = k },
        .text => |x| .{ .text = try self.t.addString(x) },
    };
    self.any_tags = true;
}

pub fn addNull(self: *Builder) Allocator.Error!Node.Id {
    return self.append(.{ .kind = table.Kind.null_ });
}

pub fn addBool(self: *Builder, value: bool) Allocator.Error!Node.Id {
    return self.append(.{ .kind = if (value) table.Kind.true_ else table.Kind.false_ });
}

/// Add a signed integer scalar (rendered as decimal text).
pub fn addInt(self: *Builder, value: i64) Allocator.Error!Node.Id {
    var buf: [24]u8 = undefined;
    const raw = std.fmt.bufPrint(&buf, "{d}", .{value}) catch unreachable;
    return self.addNumberRaw(raw, false);
}

/// Add an unsigned integer scalar (decimal text). Separate from `addInt` so
/// values above `maxInt(i64)` round-trip without overflow.
pub fn addUint(self: *Builder, value: u64) Allocator.Error!Node.Id {
    var buf: [24]u8 = undefined;
    const raw = std.fmt.bufPrint(&buf, "{d}", .{value}) catch unreachable;
    return self.addNumberRaw(raw, false);
}

/// Add a numeric scalar from already-formatted text (copied). `is_float`
/// records which number kind it is. This is the escape hatch for floats and
/// for integers outside the i64/u64 range.
pub fn addNumberRaw(self: *Builder, raw: []const u8, is_float: bool) Allocator.Error!Node.Id {
    return self.append(.{ .kind = if (is_float) table.Kind.float else table.Kind.int, .text = try self.t.addString(raw) });
}

/// Add a string scalar (copied).
pub fn addString(self: *Builder, value: []const u8) Allocator.Error!Node.Id {
    return self.append(.{ .kind = table.Kind.string, .text = try self.t.addString(value) });
}

/// Add a format-specific scalar (TOML datetime, ZON enum/char literal).
/// See `Node.Kind.Extended`.
pub fn addExtended(self: *Builder, kind: Node.Kind.Extended.ExtKind, text: []const u8) Allocator.Error!Node.Id {
    return self.append(.{ .kind = table.Kind.ext + @as(u8, @intCast(@backingInt(kind))), .text = try self.t.addString(text) });
}

/// Add an alias to the anchor `name` (copied).
pub fn addAlias(self: *Builder, name: []const u8) Allocator.Error!Node.Id {
    return self.append(.{ .kind = table.Kind.alias, .text = try self.t.addString(name) });
}

/// Add a sequence whose elements are the given node ids, in order.
/// Empty is allowed (`&.{}`), yielding a node with no children.
pub fn addSequence(self: *Builder, items: []const Node.Id) Allocator.Error!Node.Id {
    const id = try self.append(.{ .kind = table.Kind.sequence });
    self.adopt(id, items);
    return id;
}

/// Add a mapping from the given entries, in order. A `keyvalue` node is
/// minted for each entry.
pub fn addMapping(self: *Builder, entries: []const Entry) Allocator.Error!Node.Id {
    const kvs = try self.allocator.alloc(Node.Id, entries.len);
    defer self.allocator.free(kvs);
    for (entries, kvs) |e, *kv| kv.* = try self.addKeyValue(e.key, e.value);
    return self.addMappingFromEntries(kvs);
}

/// Add one `keyvalue` node wrapping `key`/`value`, returning its own id —
/// the lower-level twin of `addMapping` for a caller that needs the entry's
/// id before assembling the mapping (e.g. to `setSpan` it). Pair with
/// `addMappingFromEntries`.
pub fn addKeyValue(self: *Builder, key: Node.Id, value: Node.Id) Allocator.Error!Node.Id {
    const id = try self.append(.{ .kind = table.Kind.keyvalue });
    self.adopt(id, &.{ key, value });
    return id;
}

/// Add a mapping from already-built `keyvalue` node ids (as returned by
/// `addKeyValue`), in order.
pub fn addMappingFromEntries(self: *Builder, kv_ids: []const Node.Id) Allocator.Error!Node.Id {
    const id = try self.append(.{ .kind = table.Kind.mapping });
    self.adopt(id, kv_ids);
    return id;
}

/// Freeze the builder into an owned `AST` rooted at `root`. The builder is
/// reset to empty, so a subsequent `deinit` is harmless. The AST carries the
/// comments and type tags set; the anchor/alias layer stays empty (no
/// builder API defines an anchor).
pub fn finish(self: *Builder, root: Node.Id) Allocator.Error!AST {
    try self.flush(&self.t);
    const ast = try AST.own(self.allocator, &self.t, root, null);
    self.reset();
    return ast;
}

/// An `AST` of the builder's current nodes, rooted at `root`, without
/// consuming the build: a copy the builder owns, valid until the next
/// `view` or the builder's `deinit`, and never to be `deinit`ed by the
/// caller.
pub fn view(self: *Builder, root: Node.Id) Allocator.Error!AST {
    if (self.snapshot) |*s| s.deinit();
    self.snapshot = null;
    var copy = table.Table.init(self.allocator, "");
    errdefer copy.deinit();
    try copy.rows.appendSlice(self.allocator, self.t.rows.items);
    try copy.strings.appendSlice(self.allocator, self.t.strings.items);
    try self.flush(&copy);
    self.snapshot = try AST.own(self.allocator, &copy, root, null);
    return self.snapshot.?;
}

// ── internals ───────────────────────────────────────────────────────────────

fn append(self: *Builder, row: table.Row) Allocator.Error!Node.Id {
    const id = try self.t.addRow(row);
    errdefer self.t.rows.items.len -= 1;
    try self.comments.append(self.allocator, .{});
    return id;
}

/// Make `ids` the children of `parent`, in order.
fn adopt(self: *Builder, parent: Node.Id, ids: []const Node.Id) void {
    const rows = self.t.rows.items;
    rows[parent].first_child = if (ids.len == 0) none else ids[0];
    for (ids, 0..) |c, i| {
        rows[c].parent = parent;
        rows[c].next_sibling = if (i + 1 < ids.len) ids[i + 1] else none;
    }
}

/// Write the pending comments and tags into `t`'s fact lists.
fn flush(self: *Builder, t: *table.Table) Allocator.Error!void {
    const a = self.allocator;
    // The texts are offsets into the build's buffer, which `t` has (it is
    // the build's table, or a copy of its rows and buffer).
    if (self.any_tags) for (self.node_tags.items, 0..) |tag, i| if (tag) |x| {
        try t.tags.append(a, switch (x) {
            .text => |s| .{ .node = @intCast(i), .text = s },
            .kind => |k| .{ .node = @intCast(i), .text = try t.addString((AST.Tag{ .kind = k }).spelling()), .resolved = table.tag_kind + @as(u8, @intCast(@backingInt(k))) },
        });
    };
    if (self.any_comments) for (self.comments.items, 0..) |nc, i| {
        const n: u32 = @intCast(i);
        for (nc.leading.items) |c| try t.comments.append(a, .{ .node = n, .slot = table.comment_leading, .style = @intCast(@backingInt(c.style)), .text = c.text });
        if (nc.trailing) |c| try t.comments.append(a, .{ .node = n, .slot = table.comment_trailing, .style = @intCast(@backingInt(c.style)), .text = c.text });
        for (nc.dangling.items) |c| try t.comments.append(a, .{ .node = n, .slot = table.comment_dangling, .style = @intCast(@backingInt(c.style)), .text = c.text });
    };
}

fn reset(self: *Builder) void {
    for (self.comments.items) |*nc| {
        nc.leading.deinit(self.allocator);
        nc.dangling.deinit(self.allocator);
    }
    self.comments.clearRetainingCapacity();
    self.node_tags.clearRetainingCapacity();
    self.any_comments = false;
    self.any_tags = false;
}

/// A comment whose text is copied into the build's buffer.
fn ownComment(self: *Builder, c: Comment) Allocator.Error!PendingComment {
    return .{ .text = try self.t.addString(c.text), .style = c.style };
}
