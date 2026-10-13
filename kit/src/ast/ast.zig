//! AST = Abstract Syntax Tree
//!
//! A document's values, as the node table holds them: one 32-byte row per
//! node (`../table.zig`), a scalar's text in the source or in the table's
//! string buffer, and every rarer fact — anchors, tags, markers, separators,
//! regions, mentions, comments, directives — in a sparse list sorted by node.
//! The table a language's parse fills IS this storage: fig checks it and
//! adopts it (`adopt`), with no copy (docs/proposals/core-only.md §4.9).
//!
//! A reader asks for a node by id and gets a `Node` — a view, with its kind
//! as a tagged union — so a `switch` over a node's kind stays exhaustive.
//! Two documents of different formats can have the same AST.
//!
//! This file owns the struct itself — its storage, the node model, the
//! comment layer, and equality. The rest of the surface is split across
//! sibling files and re-exported below as `AST` methods (see "Sub-modules").

const AST = @This();
const std = @import("std");
const activeTag = std.meta.activeTag;
const Allocator = std.mem.Allocator;
const util = @import("../kit.zig");
const table = @import("../table.zig");
const Span = @import("../span.zig");
const Row = table.Row;
const none = table.none;

// ── Sub-modules ─────────────────────────────────────────────────────────────
// The AST's behavior is split across sibling files, each operating on a
// `*const AST`/`*AST`. Every method defined there is surfaced here as a decl
// alias: `ast.foo()` resolves `foo` on this type, and the aliased function
// receives the AST as its first parameter.
const reader = @import("reader.zig");
const options = @import("options.zig");

// Builder — programmatic, bottom-up construction (the write path). Its file is
// itself the `Builder` struct, so the import IS the type.
pub const Builder = @import("builder.zig");

// Navigation + opt-in reference-layer resolution (the read path).
pub const PathSegment = reader.PathSegment;
pub const ResolveError = reader.ResolveError;
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

// What a printer takes and how it fails. Rendering through the format
// registry is core's (`serialize.zig`), so it is not a method here.
pub const SerializeOptions = options.SerializeOptions;
pub const SerializeError = options.SerializeError;

allocator: Allocator,
/// What a row's in-source text indexes. Empty for a tree built with no
/// source, whose every text is in `strings`.
source: []const u8 = "",
/// The node every other is under. Row 0 for a parse's table; a tree a
/// caller built may root anywhere.
root: Node.Id = 0,
/// The key whose value is merged into its mapping (YAML's `<<`), from the
/// language's `Syntax.merge_key`, or null when the language has no merge.
/// Not owned: it lives as long as the language's registration.
merge_key: ?[]const u8 = null,

// ── The table ───────────────────────────────────────────────────────────────
// Row `i` is node `i`. A row's `first_child` and `next_sibling` link the
// tree: a container's children are a sibling chain, and a keyvalue's two
// children are its key and then its value. Every sparse list is sorted by
// node, so a node's fact is a binary search; a list is empty when the
// document has no such fact.

rows: []const Row = &.{},
/// The decoded text rows name with `Text`'s high bit set.
strings: []const u8 = "",
anchor_facts: []const table.Anchor = &.{},
tag_facts: []const table.Tag = &.{},
marker_facts: []const table.SpanFact = &.{},
sep_facts: []const table.SpanFact = &.{},
item_sep_facts: []const table.ItemSep = &.{},
/// Sorted by `(node, start)`.
region_facts: []const table.SpanFact = &.{},
/// Sorted by `(node, start)`.
mention_facts: []const table.Mention = &.{},

// ── Decoded from the table ──────────────────────────────────────────────────
// The comment and directive lists, decoded once into the shapes a printer
// reads (`comments(id)`, `tag_directives`). Both are sparse and small.

/// `comment_list` grouped by node: `comment_runs[i]` is node `node`'s
/// comments in `slot`, `comment_list[start..][0..len]`. Sorted by
/// `(node, slot)`.
comment_runs: []const CommentRun = &.{},
comment_list: []const Comment = &.{},
/// Tag-handle definitions in source order — a YAML `%TAG` directive's handle
/// and the prefix it expands to. Empty when the document declares none.
tag_directives: []const TagDirective = &.{},

/// The table the rows and lists live in, when this AST owns it; null for an
/// AST that borrows another's (`borrow`).
storage: ?*table.Table = null,
/// Whether `comment_runs`, `comment_list` and `tag_directives` are this
/// AST's to free.
owns_decoded: bool = true,

pub const CommentRun = struct { node: Node.Id, slot: u8, start: u32, len: u32 };

pub fn deinit(self: *AST) void {
    if (self.owns_decoded) {
        self.allocator.free(self.comment_runs);
        self.allocator.free(self.comment_list);
        self.allocator.free(self.tag_directives);
    }
    if (self.storage) |t| {
        t.deinit();
        self.allocator.destroy(t);
    }
    self.* = undefined;
}

// ── Reading a node ──────────────────────────────────────────────────────────

/// How many nodes the tree has. Ids run from 0 to this, exclusive.
pub fn nodeCount(self: *const AST) usize {
    return self.rows.len;
}

/// Node `id`, as a view: its kind with its payload, and its next sibling in
/// its container. A keyvalue's key and value have none; the entry names
/// them.
pub fn node(self: *const AST, id: Node.Id) Node {
    const r = self.rows[id];
    return .{ .id = id, .kind = self.kindOfRow(r), .next_sibling = self.siblingOfRow(r) };
}

/// Node `id`'s kind: `node(id).kind`.
pub fn kindOf(self: *const AST, id: Node.Id) Node.Kind {
    return self.kindOfRow(self.rows[id]);
}

fn siblingOfRow(self: *const AST, r: Row) ?Node.Id {
    if (r.next_sibling == none) return null;
    if (r.parent != none and self.rows[r.parent].kind == table.Kind.keyvalue) return null;
    return r.next_sibling;
}

fn kindOfRow(self: *const AST, r: Row) Node.Kind {
    return rowKind(self.source, self.strings, self.rows, r);
}

/// Row `r`'s kind, its texts in `source` or `strings` and its children in
/// `rows`: what `node(id).kind` reads, for a table still being written.
pub fn rowKind(source: []const u8, strings: []const u8, rows: []const Row, r: Row) Node.Kind {
    const K = table.Kind;
    const text = table.viewText(source, strings, r.text) orelse "";
    return switch (r.kind) {
        K.null_ => .null_,
        K.false_ => .{ .boolean = false },
        K.true_ => .{ .boolean = true },
        K.string => .{ .string = text },
        K.int => .{ .number = .{ .raw = text, .kind = .integer } },
        K.float => .{ .number = .{ .raw = text, .kind = .float } },
        K.sequence => .{ .sequence = opt(r.first_child) },
        K.mapping => .{ .mapping = opt(r.first_child) },
        K.keyvalue => .{ .keyvalue = .{ .key = r.first_child, .value = rows[r.first_child].next_sibling } },
        K.alias => .{ .alias = text },
        else => .{ .extended = .{ .kind = @fromBackingInt(@intCast(r.kind - K.ext)), .text = text } },
    };
}

fn opt(x: u32) ?u32 {
    return if (x == none) null else x;
}

/// The bytes `t` names, or null when it is absent.
pub fn textOf(self: *const AST, t: table.Text) ?[]const u8 {
    return table.viewText(self.source, self.strings, t);
}

fn textOr(self: *const AST, t: table.Text, default: []const u8) []const u8 {
    return self.textOf(t) orelse default;
}

/// Node `id`'s parent, or null for a root.
pub fn parentOf(self: *const AST, id: Node.Id) ?Node.Id {
    return opt(self.rows[id].parent);
}

/// Node `id`'s span in the source. A node with none — a tree built with no
/// source — is the empty span at 0.
pub fn spanOf(self: *const AST, id: Node.Id) Span {
    const r = self.rows[id];
    if (r.start == none) return Span.init(0, 0);
    return Span.init(r.start, r.end);
}

// ── Side facts ──────────────────────────────────────────────────────────────

/// The type tag on node `id`, or null: a kind tag where the tag names a
/// core type in the spelling every format reads (`!!int`, or no text at
/// all), and the text as written otherwise. A tag the language resolved to
/// a core type but spelled otherwise (`!<tag:yaml.org,2002:int>`) is its
/// text, which a printer re-emits; `tagFact` has the resolution. A tag the
/// language left unstated is read by its text, the seven core-schema
/// spellings as the kinds they name.
pub fn tagOf(self: *const AST, id: Node.Id) ?Tag {
    const f = self.tagFact(id) orelse return null;
    return self.tagOfFact(f);
}

/// Node `id`'s tag as the table states it: its text, its resolution, and
/// whether the node agrees with the resolution.
pub fn tagFact(self: *const AST, id: Node.Id) ?table.Tag {
    return table.factOf(table.Tag, self.tag_facts, id);
}

/// A tag fact as a `Tag`; see `tagOf`.
pub fn tagOfFact(self: *const AST, f: table.Tag) Tag {
    const text = self.textOf(f.text);
    if (f.resolved >= table.tag_kind) {
        const k: Tag.KindTag = @fromBackingInt(@intCast(f.resolved - table.tag_kind));
        const x = text orelse return .{ .kind = k };
        if (std.mem.eql(u8, x, (Tag{ .kind = k }).spelling())) return .{ .kind = k };
        return .{ .text = x };
    }
    if (f.resolved == table.tag_unstated) {
        if (Tag.kindOfSpelling(text orelse "")) |k| return .{ .kind = k };
    }
    return .{ .text = text orelse "" };
}

/// The anchor name defined on node `id` (no leading `&`), or null.
pub fn anchorOf(self: *const AST, id: Node.Id) ?[]const u8 {
    const f = table.factOf(table.Anchor, self.anchor_facts, id) orelse return null;
    return self.textOf(f.name);
}

/// The comments bound to `id`; empty for a node with none.
pub fn comments(self: *const AST, id: Node.Id) NodeComments {
    const runs = self.comment_runs;
    var lo: usize = 0;
    var hi: usize = runs.len;
    while (lo < hi) {
        const mid = lo + (hi - lo) / 2;
        if (runs[mid].node < id) lo = mid + 1 else hi = mid;
    }
    var out: NodeComments = .{};
    while (lo < runs.len and runs[lo].node == id) : (lo += 1) {
        const r = runs[lo];
        const items = self.comment_list[r.start..][0..r.len];
        switch (r.slot) {
            table.comment_leading => out.leading = items,
            table.comment_trailing => out.trailing = items[0],
            else => out.dangling = items,
        }
    }
    return out;
}

/// Whether the tree carries any comment.
pub fn hasComments(self: *const AST) bool {
    return self.comment_list.len > 0;
}

/// Whether the tree carries any type tag.
pub fn hasTags(self: *const AST) bool {
    return self.tag_facts.len > 0;
}

/// Whether the tree defines any anchor.
pub fn hasAnchors(self: *const AST) bool {
    return self.anchor_facts.len > 0;
}

/// The anchors defined, in definition order — by node, which is source
/// order.
pub fn anchorIterator(self: *const AST) AnchorIterator {
    return .{ .ast = self };
}

pub const AnchorIterator = struct {
    ast: *const AST,
    i: usize = 0,

    pub fn next(it: *AnchorIterator) ?Anchor {
        if (it.i >= it.ast.anchor_facts.len) return null;
        const f = it.ast.anchor_facts[it.i];
        it.i += 1;
        return .{ .name = it.ast.textOr(f.name, ""), .node = f.node };
    }
};

/// The node a container child's *leading* comment binds to: the key of a
/// `keyvalue` entry (so the comment renders above the key), else the child node
/// itself (a sequence element). Shared by every printer that emits comments.
pub fn leadingCommentAnchor(self: *const AST, id: Node.Id) Node.Id {
    return switch (self.kindOf(id)) {
        .keyvalue => |kv| kv.key,
        else => id,
    };
}

/// The node a container child's *trailing* comment binds to: the value of a
/// `keyvalue` entry (so the comment renders after the value), else the child
/// node itself.
pub fn trailingCommentAnchor(self: *const AST, id: Node.Id) Node.Id {
    return switch (self.kindOf(id)) {
        .keyvalue => |kv| kv.value,
        else => id,
    };
}

/// An AST of `nodes`, given as `Node` values (`nodes[i].id == i`), rooted
/// at `root`: for a test or a caller that spells a small tree out by hand.
/// The nodes' strings are copied.
pub fn fromNodes(allocator: Allocator, root: Node.Id, nodes: []const Node) Allocator.Error!AST {
    var t = table.Table.init(allocator, "");
    errdefer t.deinit();
    const K = table.Kind;
    const rows = try t.addRows(nodes.len);
    for (nodes, 0..) |n, i| {
        var r: Row = .{};
        switch (n.kind) {
            .null_ => r.kind = K.null_,
            .boolean => |b| r.kind = if (b) K.true_ else K.false_,
            .string => |x| {
                r.kind = K.string;
                r.text = try t.addString(x);
            },
            .number => |num| {
                r.kind = if (num.kind == .float) K.float else K.int;
                r.text = try t.addString(num.raw);
            },
            .extended => |e| {
                r.kind = K.ext + @as(u8, @intCast(@backingInt(e.kind)));
                r.text = try t.addString(e.text);
            },
            .alias => |x| {
                r.kind = K.alias;
                r.text = try t.addString(x);
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
            },
        }
        rows[i] = r;
    }
    for (nodes, 0..) |n, i| switch (n.kind) {
        .sequence, .mapping => |first| {
            var c = first;
            while (c) |id| : (c = nodes[id].next_sibling) {
                rows[id].parent = @intCast(i);
                rows[id].next_sibling = nodes[id].next_sibling orelse none;
            }
        },
        .keyvalue => |kv| {
            rows[kv.key].parent = @intCast(i);
            rows[kv.key].next_sibling = kv.value;
            rows[kv.value].parent = @intCast(i);
            rows[kv.value].next_sibling = none;
        },
        else => {},
    };
    return own(allocator, &t, root, null);
}

/// A tree spelled out as `Node` values, as a test writes the one it
/// expects: `nodes[i].id == i`, rooted at `root`.
pub const Literal = struct {
    allocator: Allocator,
    root: Node.Id,
    nodes: []const Node,

    /// Whether `ast` is this tree, node for node from the root, whatever
    /// ids either gives its nodes.
    pub fn matches(self: Literal, ast: AST) bool {
        var want = fromNodes(self.allocator, self.root, self.nodes) catch return false;
        defer want.deinit();
        return want.sameTree(want.root, &ast, ast.root);
    }
};

/// Whether the subtree at `id` here and the one at `other_id` in `other`
/// hold the same values in the same shape — kinds, scalar payloads,
/// children in order — whatever their ids. Tags, anchors and comments are
/// not compared.
pub fn sameTree(self: *const AST, id: Node.Id, other: *const AST, other_id: Node.Id) bool {
    const a = self.kindOf(id);
    const b = other.kindOf(other_id);
    if (activeTag(a) != activeTag(b)) return false;
    switch (a) {
        .sequence, .mapping => |first| {
            var x = first;
            var y = switch (b) {
                .sequence, .mapping => |f| f,
                else => unreachable,
            };
            while (x != null and y != null) : ({
                x = self.node(x.?).next_sibling;
                y = other.node(y.?).next_sibling;
            }) {
                if (!self.sameTree(x.?, other, y.?)) return false;
            }
            return x == null and y == null;
        },
        .keyvalue => |kv| return self.sameTree(kv.key, other, b.keyvalue.key) and self.sameTree(kv.value, other, b.keyvalue.value),
        else => return a.eql(b),
    }
}

/// Set node `id`'s type tag, replacing any it had. Only for an AST that
/// owns its table; a kind tag is spelled as YAML's core schema spells it.
pub fn setTag(self: *AST, id: Node.Id, tag: Tag) Allocator.Error!void {
    const t = self.storage.?;
    const f: table.Tag = switch (tag) {
        .text => |x| .{ .node = id, .text = try t.addString(x) },
        .kind => |k| .{ .node = id, .text = try t.addString(tag.spelling()), .resolved = table.tag_kind + @as(u8, @intCast(@backingInt(k))) },
    };
    try t.tags.append(t.allocator, f);
    sortFacts(t);
    self.bind(t);
}

// ── Making one ──────────────────────────────────────────────────────────────

pub const TableError = error{
    /// A row's `kind` is not a value core knows.
    UnknownKind,
    /// A parent index past the table, a child before its parent, a keyvalue
    /// without exactly a key and a value, a mapping child that is not a
    /// keyvalue, a scalar with children, a row with no span, a second root,
    /// a side fact naming no row, a text outside the source and the buffer.
    MalformedTable,
    OutOfMemory,
};

/// Whether a table's rows must carry spans: a parse's do, and a print's,
/// built from a tree with no source, do not.
pub const Spans = enum { required, absent };

/// The AST a parse's table is, over `source`: the table is checked — row 0
/// is the root and the only row without a parent, a parent precedes its
/// children, a mapping's children are keyvalues, a keyvalue has exactly a
/// key and a value, a scalar has no children, every span and text lies in
/// range, every side fact names a row — its child links are derived, and it
/// becomes the AST's storage. `t` is emptied, whatever the outcome; nothing
/// it held is copied.
pub fn adopt(allocator: Allocator, source: []const u8, t: *table.Table, spans_rule: Spans) TableError!AST {
    const storage = try allocator.create(table.Table);
    storage.* = t.*;
    storage.source = source;
    t.* = table.Table.init(t.allocator, t.source);
    errdefer {
        storage.deinit();
        allocator.destroy(storage);
    }
    try check(storage, spans_rule);
    var ast: AST = .{ .allocator = allocator, .source = source, .root = 0, .storage = storage };
    try ast.decode(storage);
    ast.bind(storage);
    return ast;
}

/// The AST a caller built into `t`, which becomes its storage: rows whose
/// links the caller set, in any order, rooted at `root`. The side lists are
/// sorted by node; nothing else is checked. `t` is emptied.
pub fn own(allocator: Allocator, t: *table.Table, root: Node.Id, merge_key: ?[]const u8) Allocator.Error!AST {
    const storage = try allocator.create(table.Table);
    storage.* = t.*;
    t.* = table.Table.init(t.allocator, t.source);
    errdefer {
        storage.deinit();
        allocator.destroy(storage);
    }
    sortFacts(storage);
    var ast: AST = .{ .allocator = allocator, .source = storage.source, .root = root, .merge_key = merge_key, .storage = storage };
    ast.decode(storage) catch |e| switch (e) {
        error.OutOfMemory => return error.OutOfMemory,
        else => unreachable, // a caller's comments name its own rows
    };
    ast.bind(storage);
    return ast;
}

/// An AST over a table fig filled and linked — what a `print` is handed —
/// without taking it: the rows and lists are borrowed and must outlive the
/// AST. `deinit` frees only what the AST decoded.
pub fn borrow(allocator: Allocator, source: []const u8, v: *const table.View) TableError!AST {
    const rows = v.rowSlice();
    if (rows.len == 0) return error.MalformedTable;
    for (rows) |r| {
        if (r.first_child != none and r.first_child >= rows.len) return error.MalformedTable;
        if (r.next_sibling != none and r.next_sibling >= rows.len) return error.MalformedTable;
        if (r.parent != none and r.parent >= rows.len) return error.MalformedTable;
        if (r.kind == table.Kind.keyvalue and (r.first_child == none or rows[r.first_child].next_sibling == none)) return error.MalformedTable;
        if (r.kind > table.Kind.alias and r.kind < table.Kind.ext) return error.UnknownKind;
        if (r.kind >= table.Kind.ext and r.kind - table.Kind.ext >= ext_kinds) return error.UnknownKind;
    }
    var ast: AST = .{
        .allocator = allocator,
        .source = source,
        .root = 0,
        .rows = rows,
        .strings = if (v.strings) |p| p[0..v.strings_len] else "",
        .anchor_facts = table.slice(table.Anchor, v.anchors, v.anchor_count),
        .tag_facts = table.slice(table.Tag, v.tags, v.tag_count),
        .marker_facts = table.slice(table.SpanFact, v.markers, v.marker_count),
        .sep_facts = table.slice(table.SpanFact, v.seps, v.sep_count),
        .item_sep_facts = table.slice(table.ItemSep, v.item_seps, v.item_sep_count),
        .region_facts = table.slice(table.SpanFact, v.regions, v.region_count),
        .mention_facts = table.slice(table.Mention, v.mentions, v.mention_count),
    };
    try ast.decodeLists(table.slice(table.Comment, v.comments, v.comment_count), table.slice(table.Directive, v.directives, v.directive_count));
    return ast;
}

const ext_kinds = @typeInfo(Node.Kind.Extended.ExtKind).@"enum".field_names.len;

/// Point the AST's slices at `t`'s lists.
fn bind(self: *AST, t: *const table.Table) void {
    self.rows = t.rows.items;
    self.strings = t.strings.items;
    self.anchor_facts = t.anchors.items;
    self.tag_facts = t.tags.items;
    self.marker_facts = t.markers.items;
    self.sep_facts = t.seps.items;
    self.item_sep_facts = t.item_seps.items;
    self.region_facts = t.regions.items;
    self.mention_facts = t.mentions.items;
}

fn decode(self: *AST, t: *const table.Table) TableError!void {
    // The decoded lists read texts through the AST, which reads the table.
    self.bind(t);
    try self.decodeLists(t.comments.items, t.directives.items);
}

/// Decode the comment and directive lists. Comments must be sorted by
/// node, and within a node by slot, keeping source order within a slot.
fn decodeLists(self: *AST, cs: []const table.Comment, ds: []const table.Directive) TableError!void {
    const a = self.allocator;
    if (cs.len > 0) {
        const list = try a.alloc(Comment, cs.len);
        errdefer a.free(list);
        var runs: std.ArrayList(CommentRun) = .empty;
        errdefer runs.deinit(a);
        for (cs, list, 0..) |c, *o, i| {
            if (c.node >= self.rows.len) return error.MalformedTable;
            o.* = .{
                .text = if (c.text.isAbsent()) "" else self.textOf(c.text) orelse return error.MalformedTable,
                .style = switch (c.style) {
                    table.comment_line => .line,
                    table.comment_block => .block,
                    else => return error.MalformedTable,
                },
            };
            if (c.slot > table.comment_dangling) return error.MalformedTable;
            const last = if (runs.items.len > 0) &runs.items[runs.items.len - 1] else null;
            if (last != null and last.?.node == c.node and last.?.slot == c.slot) {
                // At most one trailing comment per node.
                if (c.slot == table.comment_trailing) return error.MalformedTable;
                last.?.len += 1;
            } else {
                if (last) |l| if (l.node > c.node or (l.node == c.node and l.slot > c.slot)) return error.MalformedTable;
                try runs.append(a, .{ .node = c.node, .slot = c.slot, .start = @intCast(i), .len = 1 });
            }
        }
        self.comment_runs = try runs.toOwnedSlice(a);
        self.comment_list = list;
    }
    if (ds.len > 0) {
        const out = try a.alloc(TagDirective, ds.len);
        errdefer a.free(out);
        for (ds, out) |d, *o| o.* = .{
            .handle = self.textOf(d.handle) orelse return error.MalformedTable,
            .prefix = self.textOf(d.prefix) orelse return error.MalformedTable,
        };
        self.tag_directives = out;
    }
}

/// Sort each side list by node, as the AST reads them. Single-valued facts
/// keep the last stated for a node, as a later statement replaces an
/// earlier; comments keep their order within a node and slot.
fn sortFacts(t: *table.Table) void {
    dedupeLast(table.Anchor, &t.anchors);
    dedupeLast(table.Tag, &t.tags);
    dedupeLast(table.SpanFact, &t.markers);
    dedupeLast(table.SpanFact, &t.seps);
    dedupeLast(table.ItemSep, &t.item_seps);
    sortBy(table.SpanFact, t.regions.items, struct {
        fn lt(_: void, x: table.SpanFact, y: table.SpanFact) bool {
            return x.node < y.node or (x.node == y.node and x.start < y.start);
        }
    }.lt);
    sortBy(table.Mention, t.mentions.items, struct {
        fn lt(_: void, x: table.Mention, y: table.Mention) bool {
            return x.node < y.node or (x.node == y.node and x.start < y.start);
        }
    }.lt);
    sortBy(table.Comment, t.comments.items, struct {
        fn lt(_: void, x: table.Comment, y: table.Comment) bool {
            return x.node < y.node or (x.node == y.node and x.slot < y.slot);
        }
    }.lt);
}

/// Stable-sort `items` by `lt`, unless it is sorted already.
fn sortBy(comptime T: type, items: []T, comptime lt: fn (void, T, T) bool) void {
    var sorted = true;
    for (items[0..items.len -| 1], items[@min(1, items.len)..]) |x, y| {
        if (lt({}, y, x)) {
            sorted = false;
            break;
        }
    }
    if (!sorted) std.mem.sort(T, items, {}, lt);
}

/// Sort a single-valued fact list by node, keeping the last fact stated
/// for each node.
fn dedupeLast(comptime T: type, list: *std.ArrayList(T)) void {
    const lt = struct {
        fn f(_: void, x: T, y: T) bool {
            return x.node < y.node;
        }
    }.f;
    sortBy(T, list.items, lt);
    var w: usize = 0;
    for (list.items, 0..) |f, i| {
        if (i + 1 < list.items.len and list.items[i + 1].node == f.node) continue;
        list.items[w] = f;
        w += 1;
    }
    list.shrinkRetainingCapacity(w);
}

/// Check a parse's table and derive its child links, in place.
fn check(t: *table.Table, spans_rule: Spans) TableError!void {
    const rows = t.rows.items;
    // The empty document is spelled as one null row by the format; a table
    // with no rows at all is refused as the ambiguity it is.
    if (rows.len == 0) return error.MalformedTable;
    const source = t.source;
    const strings = t.strings.items;
    const K = table.Kind;

    // Rows: kinds, texts, spans, and the parent order — a parent precedes
    // its child (pre-order) and only row 0 has none. Each child is linked
    // after its parent's last, which `last` holds while the parent's
    // subtree is being read.
    const last = try t.allocator.alloc(u32, rows.len);
    defer t.allocator.free(last);
    @memset(last, none);
    for (rows, 0..) |*r, i| {
        if (i == 0) {
            if (r.parent != none) return error.MalformedTable;
        } else {
            if (r.parent == none or r.parent >= i) return error.MalformedTable;
        }
        if (!(spans_rule == .absent and r.start == none)) try checkSpan(source, r.start, r.end);
        if (r.flags & ~table.row_open != 0) return error.MalformedTable;
        if (r.flags & table.row_open != 0 and r.kind != K.sequence) return error.MalformedTable;
        switch (r.kind) {
            K.null_, K.false_, K.true_, K.sequence, K.mapping, K.keyvalue => {},
            K.int, K.float, K.alias => if (table.viewText(source, strings, r.text) == null) return error.MalformedTable,
            K.string => try optText(source, strings, r.text),
            else => {
                if (r.kind < K.ext or r.kind - K.ext >= ext_kinds) return error.UnknownKind;
                try optText(source, strings, r.text);
            },
        }
        r.first_child = none;
        r.next_sibling = none;
        if (i == 0) continue;
        const p = r.parent;
        const pk = rows[p].kind;
        switch (pk) {
            K.sequence => if (r.kind == K.keyvalue) return error.MalformedTable,
            K.mapping => if (r.kind != K.keyvalue) return error.MalformedTable,
            K.keyvalue => if (r.kind == K.keyvalue) return error.MalformedTable,
            else => return error.MalformedTable,
        }
        if (last[p] != none) rows[last[p]].next_sibling = @intCast(i) else rows[p].first_child = @intCast(i);
        last[p] = @intCast(i);
    }
    // A keyvalue has exactly two children: its key, then its value.
    for (rows) |r| {
        if (r.kind != K.keyvalue) continue;
        if (r.first_child == none) return error.MalformedTable;
        const v = rows[r.first_child].next_sibling;
        if (v == none or rows[v].next_sibling != none) return error.MalformedTable;
    }

    // Side facts, each naming a row; spans in the source.
    const n = rows.len;
    for (t.anchors.items) |f| {
        if (f.node >= n or table.viewText(source, strings, f.name) == null) return error.MalformedTable;
        try optSpan(source, f.start, f.end);
    }
    for (t.tags.items) |f| {
        if (f.node >= n) return error.MalformedTable;
        if (f.resolved >= table.tag_kind) {
            if (f.resolved - table.tag_kind >= @typeInfo(Tag.KindTag).@"enum".field_names.len) return error.MalformedTable;
        } else if (table.viewText(source, strings, f.text) == null) return error.MalformedTable;
        try optSpan(source, f.start, f.end);
    }
    for (t.markers.items) |f| {
        if (f.node >= n) return error.MalformedTable;
        try checkSpan(source, f.start, f.end);
    }
    for (t.seps.items) |f| {
        if (f.node >= n) return error.MalformedTable;
        try checkSpan(source, f.start, f.end);
    }
    for (t.item_seps.items) |f| {
        if (f.node >= n or rows[f.node].kind != K.sequence) return error.MalformedTable;
        try optText(source, strings, f.text);
    }
    for (t.regions.items) |f| {
        if (f.node >= n) return error.MalformedTable;
        try checkSpan(source, f.start, f.end);
    }
    for (t.mentions.items) |f| {
        if (f.node >= n or f.kind > table.mention_append) return error.MalformedTable;
        try checkSpan(source, f.start, f.end);
    }
    sortFacts(t);
}

/// `[start, end)` within `source`, required.
fn checkSpan(source: []const u8, start: u32, end: u32) TableError!void {
    if (start == none or end == none or start > end or end > source.len) return error.MalformedTable;
}

/// `[start, end)` within `source`, or both `none`.
fn optSpan(source: []const u8, start: u32, end: u32) TableError!void {
    if (start == none and end == none) return;
    return checkSpan(source, start, end);
}

/// A text that may be absent, but that names bytes in range if present.
fn optText(source: []const u8, strings: []const u8, t: table.Text) TableError!void {
    if (t.isAbsent()) return;
    if (table.viewText(source, strings, t) == null) return error.MalformedTable;
}

// ── Types ───────────────────────────────────────────────────────────────────

pub const Anchor = struct { name: []const u8, node: Node.Id };

/// One `%TAG` directive: the handle it declares, including both `!`s (`!e!`, or
/// `!`/`!!` when a document redefines a default handle), and the prefix that
/// handle expands to. Both are stored verbatim, so re-emitting the directive is
/// a copy rather than a re-encoding.
pub const TagDirective = struct { handle: []const u8, prefix: []const u8 };

/// A cross-format type tag attached to a node. It unifies fig's `: type =`
/// annotation, YAML's `!!str`/`!foo`, and canonical's `!`-sigil into one
/// concept: a type assertion carried alongside the value.
pub const Tag = union(enum) {
    /// A normalized, cross-format type identity — the companion to `Node.Kind`
    /// (its tags, with `number` split into `integer`/`float`, the one
    /// distinction a bare kind tag can't otherwise carry). A fig `: int =`, a
    /// YAML `!!int`, and a canonical `!!int` all decode to `.{ .kind = .integer }`,
    /// so this arm is what crosses formats. `extended` kinds (enum/datetime) are
    /// NOT here — they are distinct node kinds that self-annotate, never a tag.
    kind: KindTag,
    /// A format tag with no core type meaning (a YAML custom `!foo`/`!<uri>`),
    /// stored verbatim (leading `!` included) so YAML re-emits it byte-for-byte.
    /// A format that can't represent it drops it (fig tier-3).
    text: []const u8,

    pub const KindTag = enum { null_, boolean, string, integer, float, sequence, mapping };

    /// The tag as a node table spells it: a text tag verbatim, a kind tag
    /// as YAML's core schema spells it (`!!int`, `!!str`) — the one
    /// spelling every format that has tags at all can read.
    pub fn spelling(self: Tag) []const u8 {
        return switch (self) {
            .text => |t| t,
            .kind => |k| switch (k) {
                .null_ => "!!null",
                .boolean => "!!bool",
                .string => "!!str",
                .integer => "!!int",
                .float => "!!float",
                .sequence => "!!seq",
                .mapping => "!!map",
            },
        };
    }

    /// The kind a core-schema type name (`int`, `str`, `seq`) names, or
    /// null.
    pub fn ofCoreName(name: []const u8) ?KindTag {
        const names = [_][]const u8{ "null", "bool", "str", "int", "float", "seq", "map" };
        for (names, 0..) |n, i| if (std.mem.eql(u8, n, name)) return @fromBackingInt(@intCast(i));
        return null;
    }

    /// The kind a core-schema spelling (`!!int`) names, or null.
    pub fn kindOfSpelling(text: []const u8) ?KindTag {
        inline for (@typeInfo(KindTag).@"enum".field_names) |name| {
            const k: KindTag = @field(KindTag, name);
            if (std.mem.eql(u8, text, (Tag{ .kind = k }).spelling())) return k;
        }
        return null;
    }

    pub fn eql(self: Tag, other: Tag) bool {
        if (activeTag(self) != activeTag(other)) return false;
        return switch (self) {
            .kind => |k| k == other.kind,
            .text => |t| util.eql(u8, t, other.text),
        };
    }
};

pub const Comment = struct {
    /// Marker-stripped content. A `block` comment may contain newlines; a `line`
    /// comment never does.
    text: []const u8,
    /// The author's original form — a hint. A printer whose target format lacks
    /// the requested form downgrades (a `block` rendered to YAML/TOML becomes a
    /// run of `#` line comments).
    style: Style = .line,

    pub const Style = enum { line, block };

    pub fn eql(self: Comment, other: Comment) bool {
        return self.style == other.style and util.eql(u8, self.text, other.text);
    }
};

/// Comments bound to one node: a run of own-line comments above it (in source
/// order), at most one same-line trailing comment, and — on a container node,
/// or the document root of any kind — a run of `dangling` comments that sit at
/// the END of its body with no child after them (an orphan in an empty `[]`,
/// comments before the closing brace, or EOF orphans after a non-container
/// root, which has no closing brace to precede). `dangling` is empty for
/// every other non-container node.
pub const NodeComments = struct {
    leading: []const Comment = &.{},
    trailing: ?Comment = null,
    dangling: []const Comment = &.{},

    pub fn isEmpty(self: NodeComments) bool {
        return self.leading.len == 0 and self.trailing == null and self.dangling.len == 0;
    }

    pub fn eql(self: NodeComments, other: NodeComments) bool {
        if (self.leading.len != other.leading.len) return false;
        for (self.leading, other.leading) |a, b| if (!a.eql(b)) return false;
        if (self.dangling.len != other.dangling.len) return false;
        for (self.dangling, other.dangling) |a, b| if (!a.eql(b)) return false;
        if ((self.trailing == null) != (other.trailing == null)) return false;
        if (self.trailing) |t| if (!t.eql(other.trailing.?)) return false;
        return true;
    }
};

/// Compare two ASTs' comment layers. Separate from `eql` (which ignores
/// comments) so round-trip tests can assert comments survived.
pub fn commentsEql(self: AST, b: AST) bool {
    const n = @max(self.rows.len, b.rows.len);
    var i: Node.Id = 0;
    while (i < n) : (i += 1) {
        const x: NodeComments = if (i < self.rows.len) self.comments(i) else .{};
        const y: NodeComments = if (i < b.rows.len) b.comments(i) else .{};
        if (!x.eql(y)) return false;
    }
    return true;
}

/// Compare two ASTs' type-tag layers. Separate from `eql` (which ignores tags,
/// like comments) so round-trip tests can assert an explicit annotation survived.
pub fn tagsEql(self: AST, b: AST) bool {
    const n = @max(self.rows.len, b.rows.len);
    var i: Node.Id = 0;
    while (i < n) : (i += 1) {
        const x: ?Tag = if (i < self.rows.len) self.tagOf(i) else null;
        const y: ?Tag = if (i < b.rows.len) b.tagOf(i) else null;
        if ((x == null) != (y == null)) return false;
        if (x) |xt| if (!xt.eql(y.?)) return false;
    }
    return true;
}

/// Represents a unit of data in an AST.
pub const Node = struct {
    /// Unique number identifier for this node.
    id: Id,
    kind: Kind,
    /// Indicates "next" value when inside a sequence/mapping.
    next_sibling: ?Id = null,

    pub const Id = u32;
    pub const Kind = union(enum) {
        null_,
        boolean: bool,
        string: []const u8,
        number: Number,
        /// A leaf scalar a source format gives special lexical meaning that the
        /// abstract model has no first-class variant for: TOML datetimes, ZON
        /// enum/char literals, etc. The `kind` tag records which, and `text` is
        /// the value's intrinsic bytes (kept verbatim, like `number.raw` — the
        /// payload IS the value, so it lives inline, not in a side-table). Adding
        /// a new such scalar is a new `ExtKind`, not a new union arm: the outer
        /// switches stay closed; only the printers (where cross-format rendering
        /// is inherently type-specific) gain an `ExtKind` case.
        extended: Extended,
        sequence: ?Id,
        mapping: ?Id,
        keyvalue: struct { key: Id, value: Id },
        /// A `*name` alias: a reference to an anchored node. The payload is the
        /// alias name (no leading `*`); the target is resolved on demand via
        /// `resolveAlias` (kept out of the node so equal documents compare equal
        /// regardless of whether resolution has run). A non-YAML printer never
        /// sees this — `materialize` expands aliases to copied subtrees first.
        alias: []const u8,
        pub const Number = struct {
            raw: []const u8,
            kind: enum { integer, float },
            pub fn eql(self: Number, other: Number) bool {
                return self.kind == other.kind and util.eql(u8, self.raw, other.raw);
            }
        };
        pub const Extended = struct {
            kind: ExtKind,
            text: []const u8,
            /// Which special scalar this is. The first four are RFC-3339-derived
            /// TOML datetimes (`text` is the raw timestamp). `enum_literal`'s
            /// `text` is the decoded name without the leading `.`; `char_literal`'s
            /// `text` is the decimal Unicode codepoint (so non-ZON printers can
            /// treat it as a number, while the ZON printer re-encodes `'A'`).
            pub const ExtKind = enum {
                offset_datetime,
                local_datetime,
                local_date,
                local_time,
                enum_literal,
                char_literal,
                /// A non-finite JSON5 number (`Infinity`, `-Infinity`, `NaN`).
                /// `text` is the source lexeme verbatim, sign included. No JSON
                /// number can hold these, so they ride in an extended scalar.
                number_special,
                /// A plist `<date>` — `text` is the raw ISO-8601-ish timestamp
                /// (plist's DTD-documented subset), kept verbatim like the TOML
                /// datetimes above.
                plist_date,
                /// A plist `<data>` — `text` is the base64 payload with all
                /// whitespace (plists commonly wrap it across lines) stripped,
                /// so the bytes are the intrinsic value, same convention as
                /// every other extended scalar.
                plist_data,
            };
            pub fn eql(self: Extended, other: Extended) bool {
                return self.kind == other.kind and util.eql(u8, self.text, other.text);
            }
        };
        pub fn eql(self: Kind, other: Kind) bool {
            if (activeTag(self) != activeTag(other)) return false;
            return switch (self) {
                .null_ => true,
                .boolean => |value| value == other.boolean,
                .string => |value| util.eql(u8, value, other.string),
                .number => |value| value.eql(other.number),
                .extended => |value| value.eql(other.extended),
                .sequence => |value| value == other.sequence,
                .mapping => |value| value == other.mapping,
                .keyvalue => |value| value.key == other.keyvalue.key and value.value == other.keyvalue.value,
                .alias => |value| util.eql(u8, value, other.alias),
            };
        }
    };

    pub fn eql(self: Node, other: Node) bool {
        if (self.id != other.id) return false;
        if (self.next_sibling != other.next_sibling) return false;
        if (!self.kind.eql(other.kind)) return false;
        return true;
    }
};

/// Function to tell if two documents are equal abstractly (does not compare
/// source text): the same root, and node for node the same kind, payload
/// and sibling.
pub fn eql(self: AST, b: AST) bool {
    if (self.root != b.root or self.rows.len != b.rows.len) return false;
    for (0..self.rows.len) |i| {
        if (!self.node(@intCast(i)).eql(b.node(@intCast(i)))) return false;
    }
    return true;
}

test {
    // Pull the split-out surface into the test binary so each sibling's `test {}`
    // blocks run.
    _ = Builder;
    _ = reader;
    _ = options;
}

test "adopt: a parse's table becomes the tree, linked, with no copy" {
    const src = "a: [1, x]";
    var t = table.Table.init(std.testing.allocator, src);
    const K = table.Kind;
    _ = try t.addRow(.{ .kind = K.mapping, .start = 0, .end = 9 });
    _ = try t.addRow(.{ .kind = K.keyvalue, .parent = 0, .start = 0, .end = 9 });
    _ = try t.addRow(.{ .kind = K.string, .parent = 1, .start = 0, .end = 1, .text = table.Text.inSource(0, 1) });
    _ = try t.addRow(.{ .kind = K.sequence, .parent = 1, .start = 3, .end = 9 });
    _ = try t.addRow(.{ .kind = K.int, .parent = 3, .start = 4, .end = 5, .text = table.Text.inSource(4, 1) });
    _ = try t.addRow(.{ .kind = K.string, .parent = 3, .start = 7, .end = 8, .text = try t.addString("x") });
    try t.comments.append(t.allocator, .{ .node = 4, .slot = table.comment_trailing, .style = table.comment_line, .text = try t.addString("one") });
    const rows_ptr = t.rows.items.ptr;
    var ast = try adopt(std.testing.allocator, src, &t, .required);
    defer ast.deinit();
    t.deinit();
    try std.testing.expectEqual(rows_ptr, ast.rows.ptr);
    const kv = ast.node(ast.node(0).kind.mapping.?).kind.keyvalue;
    try std.testing.expectEqualStrings("a", ast.node(kv.key).kind.string);
    try std.testing.expectEqual(@as(?Node.Id, null), ast.node(kv.key).next_sibling);
    const first = ast.node(ast.node(kv.value).kind.sequence.?);
    try std.testing.expectEqualStrings("1", first.kind.number.raw);
    try std.testing.expectEqualStrings("x", ast.node(first.next_sibling.?).kind.string);
    try std.testing.expectEqualStrings("one", ast.comments(4).trailing.?.text);
    try std.testing.expectEqual(Span.init(4, 5), ast.spanOf(4));
}

test "adopt: a malformed table is refused, and nothing leaks" {
    var t = table.Table.init(std.testing.allocator, "x");
    _ = try t.addRow(.{ .kind = table.Kind.mapping, .start = 0, .end = 1 });
    _ = try t.addRow(.{ .kind = table.Kind.string, .parent = 0, .start = 0, .end = 1, .text = table.Text.inSource(0, 1) });
    try std.testing.expectError(error.MalformedTable, adopt(std.testing.allocator, "x", &t, .required));
    t.deinit();
}
