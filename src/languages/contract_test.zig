//! Every compiled language, reached through the contract alone: its vtable
//! built by `kit/contract.zig`, registered beside the compiled format (its
//! dialects prefixed `c-`), held by registration to the harness on its own
//! samples, and then read alike — the table its contract parse fills is the
//! table the compiled parse records, row for row.

const std = @import("std");
const testing = std.testing;
const Languages = @import("language.zig");
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
    defer Runtime.deinitAll();
    inline for (list.rows) |row| {
        const L = Languages.of(row.name);
        if (L != void) {
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
