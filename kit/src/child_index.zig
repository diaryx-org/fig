//! What a parser that keeps adding entries to its mappings needs to find,
//! in constant time: a mapping's last child, to link the next after, and its
//! entry under a key, to tell a repeat. Without it both are a walk of the
//! sibling chain, and a table of n keys costs n² — TOML's, INI's, dotenv's.
//!
//! The index holds row ids, not key texts: an entry's parent and key are
//! read from the rows each time it is hashed or compared.

const ChildIndex = @This();
const std = @import("std");
const Allocator = std.mem.Allocator;
const RowWriter = @import("row_writer.zig");
const Node = @import("ast/ast.zig").Node;

/// A container's last child.
last: std.AutoHashMapUnmanaged(Node.Id, Node.Id) = .empty,
/// A mapping's entries, by the entry's row alone: its parent and key are
/// read from the rows when it is hashed or compared, so an entry costs the
/// index four bytes and no copy of its key.
keyed: std.HashMapUnmanaged(Node.Id, void, EntryContext, std.hash_map.default_max_load_percentage) = .empty,

/// Hashes and compares entries by the rows `w` has written.
const EntryContext = struct {
    w: RowWriter,

    /// Entry `id`'s parent and key text, as the rows hold them.
    fn keyOf(self: EntryContext, id: Node.Id) Key {
        const rows = self.w.t.rows.items;
        const key = self.w.kindOf(id).keyvalue.key;
        return .{ .parent = rows[id].parent, .text = self.w.kindOf(key).string };
    }

    pub fn hash(self: EntryContext, id: Node.Id) u64 {
        return hashKey(self.keyOf(id));
    }

    pub fn eql(self: EntryContext, a: Node.Id, b: Node.Id) bool {
        return eqlKey(self.keyOf(a), self.keyOf(b));
    }
};

/// Looks an entry up by a parent and a key text.
const KeyAdapter = struct {
    ctx: EntryContext,

    pub fn hash(_: KeyAdapter, k: Key) u64 {
        return hashKey(k);
    }

    pub fn eql(self: KeyAdapter, k: Key, id: Node.Id) bool {
        return eqlKey(k, self.ctx.keyOf(id));
    }
};

const Key = struct { parent: Node.Id, text: []const u8 };

fn hashKey(k: Key) u64 {
    var h = std.hash.Wyhash.init(k.parent);
    h.update(k.text);
    return h.final();
}

fn eqlKey(a: Key, b: Key) bool {
    return a.parent == b.parent and std.mem.eql(u8, a.text, b.text);
}

pub fn deinit(self: *ChildIndex, allocator: Allocator) void {
    self.last.deinit(allocator);
    self.keyed.deinit(allocator);
}

/// Link `child` as `parent`'s last child: after the one before it, or as
/// its first, naming `parent` as its parent. `parent` is a mapping or a
/// sequence whose children are all linked through here.
pub fn append(self: *ChildIndex, allocator: Allocator, w: RowWriter, parent: Node.Id, child: Node.Id) Allocator.Error!void {
    const slot = try self.last.getOrPut(allocator, parent);
    if (slot.found_existing) {
        w.setNext(slot.value_ptr.*, child);
    } else {
        try w.set(parent, switch (w.kindOf(parent)) {
            .sequence => .{ .sequence = child },
            else => .{ .mapping = child },
        });
    }
    w.setNext(child, null);
    w.t.rows.items[child].parent = parent;
    slot.value_ptr.* = child;
}

/// Record keyvalue `entry`, already appended to its mapping, under its key,
/// replacing an entry with the same key.
pub fn put(self: *ChildIndex, allocator: Allocator, w: RowWriter, entry: Node.Id) Allocator.Error!void {
    const ctx: EntryContext = .{ .w = w };
    const slot = try self.keyed.getOrPutContextAdapted(allocator, ctx.keyOf(entry), KeyAdapter{ .ctx = ctx }, ctx);
    slot.key_ptr.* = entry;
}

/// Mapping `parent`'s entry under `key`, or null.
pub fn get(self: *const ChildIndex, w: RowWriter, parent: Node.Id, key: []const u8) ?Node.Id {
    const ctx: EntryContext = .{ .w = w };
    return self.keyed.getKeyAdapted(Key{ .parent = parent, .text = key }, KeyAdapter{ .ctx = ctx });
}

test "append links in order, and get finds an entry by its key" {
    const table = @import("table.zig");
    const Span = @import("span.zig");
    var t = table.Table.init(std.testing.allocator, "");
    defer t.deinit();
    const w = RowWriter.init(&t);
    var ix: ChildIndex = .{};
    defer ix.deinit(std.testing.allocator);
    const map = try w.add(.{ .mapping = null }, Span.init(0, 0));
    var entries: [3]Node.Id = undefined;
    for (&entries, [_][]const u8{ "a", "b", "c" }) |*e, k| {
        const key = try w.add(.{ .string = k }, Span.init(0, 0));
        const val = try w.add(.null_, Span.init(0, 0));
        e.* = try w.add(.{ .keyvalue = .{ .key = key, .value = val } }, Span.init(0, 0));
        try ix.append(std.testing.allocator, w, map, e.*);
        try ix.put(std.testing.allocator, w, e.*);
    }
    try std.testing.expectEqual(entries[0], w.kindOf(map).mapping.?);
    try std.testing.expectEqual(entries[1], w.node(entries[0]).next_sibling.?);
    try std.testing.expectEqual(entries[2], w.node(entries[1]).next_sibling.?);
    try std.testing.expectEqual(@as(?Node.Id, entries[1]), ix.get(w, map, "b"));
    try std.testing.expectEqual(@as(?Node.Id, null), ix.get(w, map, "d"));
}
