//! The kit's rules through their C functions — `fig_kit_*`, which C, Rust
//! and (as `fig-kit.wasm`) JavaScript call — held to the fixtures every
//! binding's kit is held to (`kit/testdata/rules/`): number spelling,
//! datetime classification, the bare literal and the JSON string.

const std = @import("std");
const testing = std.testing;
const c = @import("../c_api.zig");
const kit = @import("kit");
const rows = @import("kit_rows_test.zig");

const Value = std.json.Value;

fn fixture(a: std.mem.Allocator, name: []const u8) !std.json.ObjectMap {
    const io = testing.io;
    const path = try std.fmt.allocPrint(a, "kit/testdata/rules/{s}", .{name});
    const bytes = try std.Io.Dir.cwd().readFileAlloc(io, path, a, .limited(1 << 20));
    return (try std.json.parseFromSliceLeaky(Value, a, bytes, .{})).object;
}

fn cases(fx: std.json.ObjectMap, name: []const u8) []Value {
    return fx.get(name).?.array.items;
}

/// A spelling's names as `FIG_SPELL_*` bits.
fn spellingBits(names: []Value) u32 {
    const order = [_][]const u8{ "hex", "octal", "binary", "underscores", "leading_zero", "bare_dot", "plus" };
    var bits: u32 = 0;
    for (names) |n| for (order, 0..) |o, i| {
        if (std.mem.eql(u8, n.string, o)) bits |= @as(u32, 1) << @intCast(i);
    };
    return bits;
}

/// Datetime options as `FIG_DATETIME_*` bits.
fn optionBits(o: Value) u32 {
    return (if (o.object.get("minute_precision").?.bool) @as(u32, 1) else 0) |
        (if (o.object.get("time_only").?.bool) @as(u32, 2) else 0);
}

fn expectName(want: Value, got: ?[]const u8, input: []const u8) !void {
    const ok = if (want == .null) got == null else got != null and std.mem.eql(u8, want.string, got.?);
    if (!ok) {
        std.debug.print("{s}: want {f}, got {?s}\n", .{ input, std.json.fmt(want, .{}), got });
        return error.TestExpectedEqual;
    }
}

const ext_names = [_][]const u8{ "offset_datetime", "local_datetime", "local_date", "local_time" };

test "the kit's number rules give every number fixture's answer" {
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const fx = try fixture(a, "number.json");
    for (cases(fx, "spellable")) |cv| {
        const x = cv.array.items;
        const raw = x[0].string;
        testing.expectEqual(x[2].bool, c.fig_kit_number_spellable(raw.ptr, raw.len, spellingBits(x[1].array.items))) catch |e| {
            std.debug.print("spellable {s}\n", .{raw});
            return e;
        };
    }
    for (cases(fx, "canonical")) |cv| {
        const x = cv.array.items;
        const raw = x[0].string;
        const out = try a.alloc(u8, raw.len + kit.rules.answer_room);
        const n = c.fig_kit_number_canonical(raw.ptr, raw.len, out.ptr, out.len);
        try testing.expectEqualStrings(x[1].string, out[0..n]);
    }
    const non_finite = [_][]const u8{ "inf", "-inf", "nan" };
    for (cases(fx, "non_finite")) |cv| {
        const x = cv.array.items;
        const raw = x[0].string;
        const got = c.fig_kit_number_non_finite(raw.ptr, raw.len);
        try expectName(x[1], if (got < 0) null else non_finite[@intCast(got)], raw);
    }
}

test "the kit's datetime rules give every datetime fixture's answer" {
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const fx = try fixture(a, "datetime.json");
    for (cases(fx, "classify")) |cv| {
        const x = cv.array.items;
        const raw = x[0].string;
        const got = c.fig_kit_datetime_classify(raw.ptr, raw.len, optionBits(x[1]));
        try expectName(x[2], if (got < 0) null else ext_names[@intCast(got)], raw);
    }
    for (cases(fx, "valid_date")) |cv| {
        const x = cv.array.items;
        try testing.expectEqual(x[1].bool, c.fig_kit_datetime_valid_date(x[0].string.ptr, x[0].string.len));
    }
    for (cases(fx, "valid_time")) |cv| {
        const x = cv.array.items;
        try testing.expectEqual(x[2].bool, c.fig_kit_datetime_valid_time(x[0].string.ptr, x[0].string.len, optionBits(x[1])));
    }
    for (cases(fx, "valid_offset")) |cv| {
        const x = cv.array.items;
        try testing.expectEqual(x[1].bool, c.fig_kit_datetime_valid_offset(x[0].string.ptr, x[0].string.len));
    }
}

/// A row kind as the literal fixture names it.
fn kindName(k: u8) []const u8 {
    const K = kit.table.Kind;
    return switch (k) {
        K.null_ => "null",
        K.false_ => "false",
        K.true_ => "true",
        K.int => "int",
        K.float => "float",
        K.string => "string",
        else => ext_names[k - K.ext],
    };
}

test "the kit's literal rule gives every literal fixture's answer" {
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const fx = try fixture(arena.allocator(), "literal.json");
    for (cases(fx, "kind")) |cv| {
        const x = cv.array.items;
        try expectName(x[1], kindName(c.fig_kit_literal_kind(x[0].string.ptr, x[0].string.len)), x[0].string);
    }
    for (cases(fx, "number")) |cv| {
        const x = cv.array.items;
        const got = c.fig_kit_literal_number(x[0].string.ptr, x[0].string.len);
        try expectName(x[1], if (got < 0) null else kindName(@intCast(got)), x[0].string);
    }
}

test "the kit's JSON string quoting gives every json-string fixture's answer, and says when there is no room" {
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const fx = try fixture(a, "json-string.json");
    for (cases(fx, "quote")) |cv| {
        const x = cv.array.items;
        const raw = x[0].string;
        const out = try a.alloc(u8, kit.rules.jsonStringRoom(raw.len));
        const n = c.fig_kit_json_string_quote(raw.ptr, raw.len, x[1].string[0], out.ptr, out.len);
        try testing.expectEqualStrings(x[2].string, out[0..n]);
    }
    var small: [3]u8 = undefined;
    try testing.expectEqual(std.math.maxInt(usize), c.fig_kit_json_string_quote("abcd", 4, '"', &small, small.len));
    try testing.expectEqual(std.math.maxInt(usize), c.fig_kit_json_string_quote("a", 1, '`', &small, small.len));
    // Every byte a control byte: the room's worst case, met exactly.
    const worst: [8]u8 = @splat(1);
    var room: [kit.rules.jsonStringRoom(worst.len)]u8 = undefined;
    try testing.expectEqual(room.len, c.fig_kit_json_string_quote(&worst, worst.len, '"', &room, room.len));
}

comptime {
    _ = rows;
}
