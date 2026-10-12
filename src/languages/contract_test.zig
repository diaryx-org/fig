//! Every bundled language, reached through the contract alone: its vtable
//! built by `kit/contract.zig`, registered (its dialects prefixed `c-`),
//! held by registration to the harness on its own samples, and then read
//! alike — the table its contract parse fills is the table its own parse
//! records, row for row.

const std = @import("std");
const testing = std.testing;
const bundle = @import("bundle.zig");
const list = @import("list.zig");
const Runtime = @import("runtime.zig");
const contract = @import("../kit/contract.zig");
const Wire = @import("wire.zig");

/// The table of `doc` as the wire spells it, for comparing two.
fn tableJson(a: std.mem.Allocator, doc: anytype) ![]const u8 {
    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();
    const t = try Runtime.fullTable(arena.allocator(), doc);
    const v = t.view();
    var w: std.Io.Writer.Allocating = .init(a);
    errdefer w.deinit();
    try Wire.tableToJson(&w.writer, &v);
    return w.toOwnedSlice();
}

test "every compiled language registers through the contract and reads its samples as the compiled format does" {
    Runtime.deinitAll();
    defer Runtime.deinitAll();
    inline for (list.rows) |row| {
        if (comptime bundle.bundles(row.name)) {
            const L = bundle.Language(row.name);
            const V = contract.VTable(L, "c-");
            _ = Runtime.register(testing.allocator, &V.vtable) catch |err| {
                std.debug.print("{s}: {s}: {s}\n", .{ row.name, @errorName(err), Runtime.lastRefusal() });
                return err;
            };
            inline for (L.dialects) |d| {
                const e = Runtime.entryByName("c-" ++ d.name).?;
                if (@hasDecl(L, "samples")) for (L.samples) |sample| {
                    var p: L.Parser = .{ .allocator = testing.allocator };
                    const compiled = L.parse(&p, sample, d.dialect) catch continue;
                    defer compiled.deinit(testing.allocator);
                    const through = try Runtime.Language.Parser.parse(testing.allocator, sample, e.typeOf());
                    defer through.deinit(testing.allocator);
                    const want = try tableJson(testing.allocator, &compiled);
                    defer testing.allocator.free(want);
                    const got = try tableJson(testing.allocator, &through);
                    defer testing.allocator.free(got);
                    testing.expectEqualStrings(want, got) catch |err| {
                        std.debug.print("{s}: sample {s}\n", .{ d.name, sample });
                        return err;
                    };
                };
            }
        }
    }
}

test "a language's diagnostics cross the contract: a refusal's message and offset, and the warnings of a clean parse" {
    if (comptime !bundle.bundles("dotenv")) return error.SkipZigTest;
    const L = bundle.Language("dotenv");
    Runtime.deinitAll();
    defer Runtime.deinitAll();
    const V = contract.VTable(L, "c-");
    _ = try Runtime.register(testing.allocator, &V.vtable);
    const e = Runtime.entryByName("c-dotenv").?;
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var errors: std.ArrayList(@import("../kit/parse_diagnostic.zig").Rendered) = .empty;
    var warnings: @TypeOf(errors) = .empty;

    var p: Runtime.Language.Parser = .{ .allocator = a, .errors = &errors, .warnings = &warnings };
    const doc = try Runtime.Language.parse(&p, "A=1\nA=2\n", e.typeOf());
    _ = doc;
    try testing.expectEqual(@as(usize, 0), errors.items.len);
    try testing.expectEqual(@as(usize, 1), warnings.items.len);
    try testing.expectEqualStrings("duplicate key", warnings.items[0].short_label);
    try testing.expectEqual(@as(usize, 4), warnings.items[0].offset);

    warnings.clearRetainingCapacity();
    try testing.expectError(error.RuntimeParseFailed, Runtime.Language.parse(&p, "A=1\n=x\n", e.typeOf()));
    try testing.expectEqual(@as(usize, 1), errors.items.len);
    try testing.expect(std.mem.indexOf(u8, errors.items[0].message, "KEY=value") != null);
    try testing.expectEqualStrings(errors.items[0].message, p.lastMessage());
    try testing.expectEqual(@as(usize, 4), errors.items[0].offset);
}
