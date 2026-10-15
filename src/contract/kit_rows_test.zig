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

// ── C ───────────────────────────────────────────────────────────────────────
// The same fixtures written as a C language writes a table: rows through
// `fig_table_rows`, linked by their `first_child` and `next_sibling`, text
// through `fig_table_string`, facts through `fig_table_*`, a repeat found by
// `fig_table_child_by_key`, and `fig_table_finish` at the end. Appending
// and the duplicate policies are the C author's own few lines, here.

const c = @import("../c_api.zig");
const FigRow = c.FigRow;
const none = table.none;

const C = struct {
    t: *table.Table,
    last: std.AutoHashMapUnmanaged(u32, u32) = .empty,

    fn row(self: *C, id: u32) *FigRow {
        return c.fig_table_row(self.t, id).?;
    }

    fn text(self: *C, s: []const u8) !table.Text {
        var out: table.Text = undefined;
        try testing.expectEqual(c.FigStatus.ok, c.fig_table_string(self.t, s.ptr, s.len, &out));
        return out;
    }

    fn append(self: *C, a: std.mem.Allocator, parent: u32, child: u32) !void {
        if (self.last.get(parent)) |prev| self.row(prev).next_sibling = child else self.row(parent).first_child = child;
        self.row(child).next_sibling = none;
        try self.last.put(a, parent, child);
    }
};

const row_kinds = [_]struct { []const u8, u8 }{
    .{ "null", table.Kind.null_ },     .{ "string", table.Kind.string },
    .{ "int", table.Kind.int },        .{ "float", table.Kind.float },
    .{ "sequence", table.Kind.sequence }, .{ "mapping", table.Kind.mapping },
    .{ "keyvalue", table.Kind.keyvalue }, .{ "alias", table.Kind.alias },
};

/// A fixture's row kind as `FIG_ROW_*`: a bool is true or false by its text.
fn cKind(name: []const u8, text: ?[]const u8) u8 {
    for (row_kinds) |k| if (std.mem.eql(u8, k[0], name)) return k[1];
    return if (std.mem.eql(u8, text.?, "true")) table.Kind.true_ else table.Kind.false_;
}

fn runC(a: std.mem.Allocator, _: []const u8, fx: Value) !void {
    var t = table.Table.init(a, fx.object.get("source").?.string);
    var w: C = .{ .t = &t };
    var puts: std.json.Array = .init(a);
    const eq = std.mem.eql;
    for (fx.object.get("ops").?.array.items) |op_v| {
        const op = op_v.array.items;
        const name = op[0].string;
        if (eq(u8, name, "add")) {
            const extra: ?std.json.ObjectMap = if (op.len > 4) op[4].object else null;
            const span = spanOf(op[3]);
            const id: u32 = @intCast(t.rows.items.len);
            const r = &(c.fig_table_rows(&t, 1) orelse return error.OutOfMemory)[0];
            const text: ?[]const u8 = if (extra) |x| if (x.get("text")) |s| s.string else null else null;
            r.kind = cKind(op[1].string, text);
            if (extra) |x| if (x.get("ext_kind")) |ek| {
                r.kind = table.Kind.ext + @as(u8, @intFromEnum(std.meta.stringToEnum(Node.Kind.Extended.ExtKind, ek.string).?));
            };
            r.start = @intCast(span.start);
            r.end = @intCast(span.end);
            if (text != null and r.kind != table.Kind.true_ and r.kind != table.Kind.false_) {
                const tx = try w.text(text.?);
                w.row(id).text = tx;
            }
            if (extra) |x| {
                if (x.get("marker")) |s| _ = c.fig_table_marker(&t, id, @intCast(spanOf(s).start), @intCast(spanOf(s).end));
                if (x.get("sep")) |s| _ = c.fig_table_sep(&t, id, @intCast(spanOf(s).start), @intCast(spanOf(s).end));
                if (x.get("anchor")) |n| {
                    const s = spanOf(x.get("anchor_span").?);
                    _ = c.fig_table_anchor(&t, id, @intCast(s.start), @intCast(s.end), try w.text(n.string));
                }
                if (x.get("item_sep")) |s| _ = c.fig_table_item_sep(&t, id, try w.text(s.string));
                if (x.get("open")) |o| if (o.bool) {
                    w.row(id).flags |= table.row_open;
                };
            }
            if (idOf(op[2])) |parent| try w.append(a, parent, id);
        } else if (eq(u8, name, "append")) {
            try w.append(a, idOf(op[1]).?, idOf(op[2]).?);
        } else if (eq(u8, name, "first")) {
            w.row(idOf(op[1]).?).first_child = idOf(op[2]) orelse none;
            _ = w.last.remove(idOf(op[1]).?);
        } else if (eq(u8, name, "next")) {
            w.row(idOf(op[1]).?).next_sibling = idOf(op[2]) orelse none;
        } else if (eq(u8, name, "put")) {
            const map = idOf(op[1]).?;
            const key = idOf(op[2]).?;
            const value = idOf(op[3]).?;
            const policy = op[4].string;
            const key_text = t.textOf(w.row(key).text).?;
            const repeat = c.fig_table_child_by_key(&t, map, key_text.ptr, key_text.len);
            if (repeat != none and !eq(u8, policy, "keep")) {
                if (eq(u8, policy, "error")) {
                    try puts.append(.{ .string = "error" });
                    continue;
                }
                if (eq(u8, policy, "last_value")) {
                    w.row(w.row(repeat).first_child).next_sibling = value;
                    w.row(value).next_sibling = none;
                }
                var pair: std.json.Array = .init(a);
                try pair.append(.{ .integer = repeat });
                try pair.append(.{ .bool = true });
                try puts.append(.{ .array = pair });
                continue;
            }
            const kv: u32 = @intCast(t.rows.items.len);
            const r = &(c.fig_table_rows(&t, 1) orelse return error.OutOfMemory)[0];
            r.kind = table.Kind.keyvalue;
            r.start = w.row(key).start;
            r.end = w.row(value).end;
            try w.append(a, map, kv);
            try w.append(a, kv, key);
            try w.append(a, kv, value);
            var pair: std.json.Array = .init(a);
            try pair.append(.{ .integer = kv });
            try pair.append(.{ .bool = repeat != none });
            try puts.append(.{ .array = pair });
        } else if (eq(u8, name, "child_by_key")) {
            const k = op[2].string;
            const got = c.fig_table_child_by_key(&t, idOf(op[1]).?, k.ptr, k.len);
            try testing.expectEqual(idOf(op[3]), if (got == none) null else got);
        } else if (eq(u8, name, "comment")) {
            const style: u8 = if (eq(u8, op[3].string, "block")) table.comment_block else table.comment_line;
            try testing.expectEqual(c.FigStatus.ok, c.fig_table_comment(&t, idOf(op[1]).?, slotOf(op[2].string), style, try w.text(op[4].string)));
        } else if (eq(u8, name, "region")) {
            _ = c.fig_table_region(&t, idOf(op[1]).?, @intCast(op[2].integer), @intCast(op[3].integer));
        } else if (eq(u8, name, "mention")) {
            const s = spanOf(op[2]);
            const kind = std.meta.stringToEnum(kit.Document.MentionKind, op[3].string).?;
            _ = c.fig_table_mention(&t, idOf(op[1]).?, @intCast(s.start), @intCast(s.end), kind.byte());
        } else if (eq(u8, name, "finish")) {
            try testing.expectEqual(c.FigStatus.ok, c.fig_table_finish(&t, idOf(op[1]).?));
            try testing.expectEqual(c.FigStatus.ok, c.fig_table_finish_in_order(&t));
        } else return error.UnknownOp;
    }
    if (fx.object.get("puts")) |want| try testing.expect(jsonEql(want, .{ .array = puts }));
    try expectTable(a, fx.object.get("table").?, &t);
}

test "C's fig_table_finish and fig_table_child_by_key give every rows fixture's table" {
    try eachFixture(runC);
}

test "fig_table_finish refuses a link to no row, and fig_table_finish_in_order rows out of order" {
    var t = table.Table.init(testing.allocator, "");
    defer t.deinit();
    const rows = c.fig_table_rows(&t, 2).?;
    rows[0].kind = table.Kind.sequence;
    rows[0].first_child = 7;
    try testing.expectEqual(c.FigStatus.invalid_argument, c.fig_table_finish(&t, 0));
    try testing.expectEqual(c.FigStatus.invalid_argument, c.fig_table_finish(&t, 2));
    try testing.expectEqual(@as(?*FigRow, null), c.fig_table_row(&t, 2));
    t.rows.items[0].first_child = 1;
    try testing.expectEqual(c.FigStatus.invalid_argument, c.fig_table_finish_in_order(&t));
    try testing.expectEqual(c.FigStatus.ok, c.fig_table_finish(&t, 0));
    try testing.expectEqual(@as(u32, 0), t.rows.items[1].parent);
    try testing.expectEqual(c.FigStatus.ok, c.fig_table_finish_in_order(&t));
}
