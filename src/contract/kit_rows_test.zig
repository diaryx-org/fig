//! The Zig kit's row writer — `RowWriter`, `ChildIndex` and `flat_map` —
//! held to the fixtures every binding's kit is held to
//! (`kit/testdata/rows/`, whose README names the ops): rows written in any
//! order, linked, put by a duplicate policy, and finished into the
//! pre-order table each file gives, compared as the wire spells it.
//!
//! The tests read the fixtures from the repository at run time, from the
//! build root the test step runs in.

const std = @import("std");
const testing = std.testing;
const kit = @import("kit");
const Wire = @import("wire.zig");

const table = kit.table;
const RowWriter = kit.RowWriter;
const flat_map = kit.flat_map;
const Span = kit.Span;
const Node = kit.AST.Node;
const Value = std.json.Value;

pub const rows_dir = "kit/testdata/rows";

/// Every fixture in `rows_dir`, each run by `run`.
pub fn eachFixture(comptime run: fn (std.mem.Allocator, []const u8, Value) anyerror!void) !void {
    const io = testing.io;
    var dir = try std.Io.Dir.cwd().openDir(io, rows_dir, .{ .iterate = true });
    defer dir.close(io);
    var it = dir.iterate();
    var n: usize = 0;
    while (try it.next(io)) |e| {
        if (!std.mem.endsWith(u8, e.name, ".json")) continue;
        var arena = std.heap.ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        const a = arena.allocator();
        const bytes = try dir.readFileAlloc(io, e.name, a, .limited(1 << 20));
        const fx = try std.json.parseFromSliceLeaky(Value, a, bytes, .{});
        run(a, e.name, fx) catch |err| {
            std.debug.print("rows fixture {s}\n", .{e.name});
            return err;
        };
        n += 1;
    }
    try testing.expect(n > 0);
}

/// Whether two JSON values are the same value, objects compared by key.
pub fn jsonEql(x: Value, y: Value) bool {
    switch (x) {
        .object => |o| {
            if (y != .object or y.object.count() != o.count()) return false;
            var it = o.iterator();
            while (it.next()) |kv| {
                const other = y.object.get(kv.key_ptr.*) orelse return false;
                if (!jsonEql(kv.value_ptr.*, other)) return false;
            }
            return true;
        },
        .array => |xs| {
            if (y != .array or y.array.items.len != xs.items.len) return false;
            for (xs.items, y.array.items) |p, q| if (!jsonEql(p, q)) return false;
            return true;
        },
        .string => |s| return y == .string and std.mem.eql(u8, s, y.string),
        .integer => |i| return y == .integer and y.integer == i,
        .bool => |b| return y == .bool and y.bool == b,
        .null => return y == .null,
        else => return false,
    }
}

/// `want`, the fixture's table, against `t` as fig adopts it and the wire
/// spells it: the rows `finish` wrote, and each side fact on its row, in
/// node order — which is what a parse's table means, whatever order its
/// lists were left in.
pub fn expectTable(a: std.mem.Allocator, want: Value, t: *table.Table) !void {
    const doc = RowWriter.adopt(a, t.source, t) catch |e| return e;
    const full = try kit.convert.fullTable(a, &doc);
    const v = full.view();
    var w: std.Io.Writer.Allocating = .init(a);
    try Wire.tableToJson(&w.writer, &v);
    const got = try std.json.parseFromSliceLeaky(Value, a, w.written(), .{});
    if (!jsonEql(want, got)) {
        var ws: std.Io.Writer.Allocating = .init(a);
        try std.json.Stringify.value(want, .{}, &ws.writer);
        std.debug.print("want {s}\ngot  {s}\n", .{ ws.written(), w.written() });
        return error.TestExpectedEqual;
    }
}

pub fn spanOf(v: Value) Span {
    return Span.init(@intCast(v.array.items[0].integer), @intCast(v.array.items[1].integer));
}

pub fn idOf(v: Value) ?u32 {
    return if (v == .null) null else @intCast(v.integer);
}

pub fn slotOf(s: []const u8) u8 {
    if (std.mem.eql(u8, s, "trailing")) return table.comment_trailing;
    if (std.mem.eql(u8, s, "dangling")) return table.comment_dangling;
    return table.comment_leading;
}

/// A row's kind as the fixture names it, with its text.
fn kindOf(kind: []const u8, extra: ?std.json.ObjectMap) Node.Kind {
    const text: []const u8 = if (extra) |x| if (x.get("text")) |t| t.string else "" else "";
    const eq = std.mem.eql;
    if (eq(u8, kind, "null")) return .null_;
    if (eq(u8, kind, "bool")) return .{ .boolean = eq(u8, text, "true") };
    if (eq(u8, kind, "int")) return .{ .number = .{ .raw = text, .kind = .integer } };
    if (eq(u8, kind, "float")) return .{ .number = .{ .raw = text, .kind = .float } };
    if (eq(u8, kind, "alias")) return .{ .alias = text };
    if (eq(u8, kind, "sequence")) return .{ .sequence = null };
    if (eq(u8, kind, "mapping")) return .{ .mapping = null };
    if (extra) |x| if (x.get("ext_kind")) |ek|
        return .{ .extended = .{ .kind = std.meta.stringToEnum(Node.Kind.Extended.ExtKind, ek.string).?, .text = text } };
    return .{ .string = text };
}

fn policyOf(s: []const u8) flat_map.DuplicatePolicy {
    if (std.mem.eql(u8, s, "last_value")) return .overwrite;
    if (std.mem.eql(u8, s, "error")) return .err;
    return std.meta.stringToEnum(flat_map.DuplicatePolicy, s).?;
}

/// The writer under test: the rows of `t`, an index of each container's
/// last child and each mapping's entries, and the parts of keyvalues
/// appended so far — a keyvalue is written whole by `RowWriter`, so one
/// built a child at a time is held here until it has both.
const Zig = struct {
    t: *table.Table,
    arena: flat_map.NodeArena,
    pending_key: std.AutoHashMapUnmanaged(u32, u32) = .empty,

    fn append(self: *Zig, a: std.mem.Allocator, parent: u32, child: u32) !void {
        const w = self.arena.w;
        if (self.t.rows.items[parent].kind == table.Kind.keyvalue) {
            if (self.pending_key.fetchRemove(parent)) |k| {
                try w.set(parent, .{ .keyvalue = .{ .key = k.value, .value = child } });
            } else try self.pending_key.put(a, parent, child);
            return;
        }
        try self.arena.index.append(a, w, parent, child);
    }
};

fn runZig(a: std.mem.Allocator, _: []const u8, fx: Value) !void {
    var t = table.Table.init(a, fx.object.get("source").?.string);
    var z: Zig = .{ .t = &t, .arena = .init(&t) };
    const w = z.arena.w;
    var puts: std.json.Array = .init(a);
    for (fx.object.get("ops").?.array.items) |op_v| {
        const op = op_v.array.items;
        const name = op[0].string;
        const eq = std.mem.eql;
        if (eq(u8, name, "add")) {
            const extra: ?std.json.ObjectMap = if (op.len > 4) op[4].object else null;
            const span = spanOf(op[3]);
            const id = if (eq(u8, op[1].string, "keyvalue")) blk: {
                // A keyvalue with no key and value yet: its row, typed.
                const id = try w.add(.null_, span);
                t.rows.items[id].kind = table.Kind.keyvalue;
                break :blk id;
            } else try w.add(kindOf(op[1].string, extra), span);
            if (extra) |x| {
                if (x.get("marker")) |s| try w.marker(id, spanOf(s));
                if (x.get("sep")) |s| try w.sep(id, spanOf(s));
                if (x.get("anchor")) |n| try w.anchor(id, n.string, if (x.get("anchor_span")) |s| spanOf(s) else null);
                if (x.get("tag")) |n| try w.tag(id, .{ .text = n.string }, table.tag_unstated, true, if (x.get("tag_span")) |s| spanOf(s) else null);
                if (x.get("item_sep")) |s| try w.itemSep(id, s.string);
                if (x.get("open")) |o| if (o.bool) w.setOpen(id);
            }
            if (idOf(op[2])) |parent| try z.append(a, parent, id);
        } else if (eq(u8, name, "append")) {
            try z.append(a, idOf(op[1]).?, idOf(op[2]).?);
        } else if (eq(u8, name, "first")) {
            const id = idOf(op[1]).?;
            try w.set(id, if (t.rows.items[id].kind == table.Kind.sequence) .{ .sequence = idOf(op[2]) } else .{ .mapping = idOf(op[2]) });
        } else if (eq(u8, name, "next")) {
            w.setNext(idOf(op[1]).?, idOf(op[2]));
        } else if (eq(u8, name, "put")) {
            const put = flat_map.putEntry(&z.arena, idOf(op[1]).?, idOf(op[2]).?, idOf(op[3]).?, policyOf(op[4].string)) catch |e| switch (e) {
                error.DuplicateKey => {
                    try puts.append(.{ .string = "error" });
                    continue;
                },
                else => return e,
            };
            var pair: std.json.Array = .init(a);
            try pair.append(.{ .integer = put.kv });
            try pair.append(.{ .bool = put.result != .added });
            try puts.append(.{ .array = pair });
        } else if (eq(u8, name, "child_by_key")) {
            try testing.expectEqual(idOf(op[3]), flat_map.lookupChild(&z.arena, idOf(op[1]).?, op[2].string));
        } else if (eq(u8, name, "comment")) {
            try w.comment(idOf(op[1]).?, slotOf(op[2].string), .{
                .text = op[4].string,
                .style = if (eq(u8, op[3].string, "block")) .block else .line,
            });
        } else if (eq(u8, name, "region")) {
            try w.region(idOf(op[1]).?, @intCast(op[2].integer), @intCast(op[3].integer));
        } else if (eq(u8, name, "mention")) {
            try w.mention(idOf(op[1]).?, spanOf(op[2]), std.meta.stringToEnum(kit.Document.MentionKind, op[3].string).?);
        } else if (eq(u8, name, "finish")) {
            try w.finish(idOf(op[1]).?);
        } else return error.UnknownOp;
    }
    if (fx.object.get("puts")) |want| try testing.expect(jsonEql(want, .{ .array = puts }));
    try expectTable(a, fx.object.get("table").?, &t);
}

test "the Zig kit's row writer gives every rows fixture's table" {
    try eachFixture(runZig);
}
