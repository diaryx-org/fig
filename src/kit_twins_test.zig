//! The kit's dotenv twins that live in-process — Zig's
//! (`kit/examples/dotenv.zig`, a `kit.grammar` description) and C's
//! (`bindings/c/examples/dotenv.c`, against fig.h) — registered as
//! languages, which runs the harness over their samples, and held to fig's
//! compiled dotenv over every dotenv twin fixture: the same table, compared
//! as `fig lang check --against` compares one (`Runtime.fullTable` of each,
//! as the wire spells it); the same printed bytes; and the same refusals at
//! the same offsets. The Rust and JavaScript twins are held there by their
//! own suites and by `tools/cli-lang-check.sh`.
//!
//! A compilation of its own (`src/build/tests.zig`), since the twins are not
//! part of fig: this file is the root of a test that carries core and both.

const std = @import("std");
const testing = std.testing;
const kit = @import("kit");
const Runtime = @import("contract/runtime.zig");
const Wire = @import("contract/wire.zig");
const bundle = @import("contract/bundle.zig");
const c = @import("c_api.zig");
const twin = @import("kit_dotenv");

extern fn kit_dotenv_c_register(name: [*:0]const u8, out_format: *c_int, err: *c.FigError) c.FigStatus;

const fixtures = "bindings/typescript/test/fixtures/dotenv";

/// The table of `doc` as the wire spells it.
fn tableJson(a: std.mem.Allocator, doc: *const kit.Document) ![]const u8 {
    const t = try Runtime.fullTable(a, doc);
    const v = t.view();
    var w: std.Io.Writer.Allocating = .init(a);
    try Wire.tableToJson(&w.writer, &v);
    return w.written();
}

fn compiled(a: std.mem.Allocator, src: []const u8) !kit.Document {
    const L = bundle.Language("dotenv");
    var p: L.Parser = .{ .allocator = a };
    return L.parse(&p, src, .DOTENV);
}

const Formats = struct { zig: c_int, c: c_int };

/// The two twins, registered once for the process.
var registered: ?Formats = null;

fn register() !Formats {
    if (registered) |r| return r;
    // Both through the C ABI, as any host registers a language.
    const V = kit.contract.VTable(twin.Language, "kit-zig-");
    var err: c.FigError = .empty;
    var zig: c_int = 0;
    if (c.fig_language_register(&V.vtable, &zig, &err) != .ok) {
        std.debug.print("kit-zig-dotenv: {s}\n", .{err.text()});
        return error.Refused;
    }
    var cf: c_int = 0;
    if (kit_dotenv_c_register("kit-c-dotenv", &cf, &err) != .ok) {
        std.debug.print("kit-c-dotenv: {s}\n", .{err.text()});
        return error.Refused;
    }
    registered = .{ .zig = zig, .c = cf };
    return registered.?;
}

test "the Zig and C dotenv twins give the compiled dotenv's table and printed bytes for every fixture" {
    const formats = try register();
    const io = testing.io;
    var dir = try std.Io.Dir.cwd().openDir(io, fixtures, .{ .iterate = true });
    defer dir.close(io);
    var it = dir.iterate();
    var n: usize = 0;
    while (try it.next(io)) |e| {
        if (!std.mem.endsWith(u8, e.name, ".env")) continue;
        var arena = std.heap.ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        const a = arena.allocator();
        const src = try dir.readFileAlloc(io, e.name, a, .limited(1 << 20));
        const printed_name = try std.fmt.allocPrint(a, "{s}.printed", .{e.name[0 .. e.name.len - 4]});
        const printed = try dir.readFileAlloc(io, printed_name, a, .limited(1 << 20));
        const want = try tableJson(a, &(try compiled(a, src)));
        for ([_]c_int{ formats.zig, formats.c }, [_][]const u8{ "zig", "c" }) |f, which| {
            const entry = Runtime.entryByAbi(f).?;
            const doc = try Runtime.Language.Parser.parse(a, src, entry.typeOf());
            const got = try tableJson(a, &doc);
            testing.expectEqualStrings(want, got) catch |err| {
                std.debug.print("{s} twin, {s}\n", .{ which, e.name });
                return err;
            };
            // Printed through the C ABI, as any host prints a format.
            var fd: ?*c.FigDocument = null;
            try testing.expectEqual(c.FigStatus.ok, c.fig_parse(src.ptr, src.len, f, &fd));
            defer c.fig_document_destroy(fd);
            var out: [*c]const u8 = undefined;
            var out_len: usize = 0;
            try testing.expectEqual(c.FigStatus.ok, c.fig_document_serialize(fd, f, null, &out, &out_len));
            testing.expectEqualStrings(printed, out[0..out_len]) catch |err| {
                std.debug.print("{s} twin printed, {s}\n", .{ which, e.name });
                return err;
            };
        }
        n += 1;
    }
    try testing.expect(n > 0);
}

test "the Zig and C dotenv twins refuse what the compiled dotenv refuses, at its offsets" {
    const formats = try register();
    const cases = [_][]const u8{ "A=\"x", "A='x\ny", "=x\n", "1A=x\n", "A x\n", "A=\"x\" y\n", "A=\"\\q\"\n", "A=1\rB=2\n" };
    for (cases) |src| {
        var want: c.FigError = .empty;
        var doc: ?*c.FigDocument = null;
        try testing.expect(c.fig_parse_ex(src.ptr, src.len, c.fig_format_by_name("dotenv"), &doc, &want) != .ok);
        for ([_]c_int{ formats.zig, formats.c }) |f| {
            var got: c.FigError = .empty;
            try testing.expect(c.fig_parse_ex(src.ptr, src.len, f, &doc, &got) != .ok);
            testing.expectEqual(want.byte_offset, got.byte_offset) catch |err| {
                std.debug.print("{s}: {s} / {s}\n", .{ src, want.text(), got.text() });
                return err;
            };
            try testing.expectEqualStrings(want.text(), got.text());
        }
    }
}
