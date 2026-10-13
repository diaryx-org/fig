//! What a parser that keeps adding entries to its mappings needs to find,
//! in constant time: a mapping's last child, to link the next after, and its
//! entry under a key, to tell a repeat. Without it both are a walk of the
//! sibling chain, and a table of n keys costs n² — TOML's, INI's, dotenv's.
//!
//! The index holds key texts, not copies: a caller passes a slice that
//! outlives the parse (the source, or a decoded string it keeps), never one
//! `RowWriter.node` handed back, which points into a buffer that moves.

const ChildIndex = @This();
const std = @import("std");
const Allocator = std.mem.Allocator;
const RowWriter = @import("row_writer.zig");
const Node = @import("ast/ast.zig").Node;

/// A container's last child.
last: std.AutoHashMapUnmanaged(Node.Id, Node.Id) = .empty,
/// A mapping's entry under a key.
keyed: std.HashMapUnmanaged(Key, Node.Id, KeyContext, std.hash_map.default_max_load_percentage) = .empty,

const Key = struct { parent: Node.Id, text: []const u8 };

const KeyContext = struct {
    pub fn hash(_: KeyContext, k: Key) u64 {
        var h = std.hash.Wyhash.init(k.parent);
        h.update(k.text);
        return h.final();
    }
    pub fn eql(_: KeyContext, a: Key, b: Key) bool {
        return a.parent == b.parent and std.mem.eql(u8, a.text, b.text);
    }
};

pub fn deinit(self: *ChildIndex, allocator: Allocator) void {
    self.last.deinit(allocator);
    self.keyed.deinit(allocator);
}

/// Link `child` as `parent`'s last child: after the one before it, or as
/// its first. `parent` is a mapping or a sequence whose children are all
/// linked through here.
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
    slot.value_ptr.* = child;
}

/// Record `entry` as mapping `parent`'s entry under `key`, replacing any.
pub fn put(self: *ChildIndex, allocator: Allocator, parent: Node.Id, key: []const u8, entry: Node.Id) Allocator.Error!void {
    try self.keyed.put(allocator, .{ .parent = parent, .text = key }, entry);
}

/// Mapping `parent`'s entry under `key`, or null.
pub fn get(self: *const ChildIndex, parent: Node.Id, key: []const u8) ?Node.Id {
    return self.keyed.get(.{ .parent = parent, .text = key });
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
        try ix.put(std.testing.allocator, map, k, e.*);
    }
    try std.testing.expectEqual(entries[0], w.kindOf(map).mapping.?);
    try std.testing.expectEqual(entries[1], w.node(entries[0]).next_sibling.?);
    try std.testing.expectEqual(entries[2], w.node(entries[1]).next_sibling.?);
    try std.testing.expectEqual(@as(?Node.Id, entries[1]), ix.get(map, "b"));
    try std.testing.expectEqual(@as(?Node.Id, null), ix.get(map, "d"));
}
