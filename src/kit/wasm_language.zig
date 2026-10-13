//! A Zig language as a WebAssembly module of its own, beside fig's: the
//! exports the TypeScript binding drives to register it into fig's module
//! and to parse and print through it. One module per language, so a caller
//! loads only the languages it imports.
//!
//! The two modules do not share memory. A parse writes its table here
//! (`fig_lang_parse`), and the binding copies the table's lists into the
//! table fig handed its own parse — rows, buffer and side facts as they
//! stand, since every offset in them is into the input or the buffer, which
//! both sides hold. A print is the same in the other direction
//! (`fig_lang_print`). Everything rarer — the description, a renderer, a
//! stream split — goes as the helper wire's JSON-RPC (`fig_lang_call`), the
//! way a helper answers it.

const std = @import("std");
const Allocator = std.mem.Allocator;
const Writer = std.Io.Writer;
const abi = @import("abi.zig");
const table = @import("table.zig");
const contract = @import("contract.zig");
const describe_json = @import("describe_json.zig");

const gpa = std.heap.wasm_allocator;

/// The byte sizes the binding copies each list by (`native.ts`), in the
/// order `fig_host_table_extend` numbers them, and the size of the view it
/// lays out: a change here is a change there.
pub const list_sizes = [_]usize{ 32, 1, 20, 24, 12, 12, 12, 16, 12, 16, 16, 28 };
comptime {
    const types = .{ table.Row, u8, table.Anchor, table.Tag, table.SpanFact, table.SpanFact, table.ItemSep, table.Comment, table.SpanFact, table.Mention, table.Directive, table.Diagnostic };
    for (list_sizes, 0..) |size, i| {
        if (@sizeOf(types[i]) != size) @compileError("a table list's element size changed; change native.ts with it");
    }
    if (@import("builtin").cpu.arch == .wasm32 and @sizeOf(table.View) != 108) @compileError("FigTableView's layout changed; change native.ts with it");
}

/// The exports for the Zig language `L`.
pub fn Exports(comptime L: type) type {
    return struct {
        const V = contract.VTable(L, "");
        const vt = &V.vtable;

        /// The table of the last parse, kept until the next, so the binding
        /// can read it after `fig_lang_parse` returns.
        var last: ?table.Table = null;

        comptime {
            @export(&alloc, .{ .name = "fig_lang_alloc" });
            @export(&free, .{ .name = "fig_lang_free" });
            @export(&parse, .{ .name = "fig_lang_parse" });
            @export(&print, .{ .name = "fig_lang_print" });
            @export(&call, .{ .name = "fig_lang_call" });
            @export(&viewSize, .{ .name = "fig_lang_view_size" });
        }

        /// `n` bytes the binding writes into, freed with `fig_lang_free`.
        fn alloc(n: usize) callconv(.c) ?[*]u8 {
            const s = gpa.alloc(u8, @max(n, 1)) catch return null;
            return s.ptr;
        }

        fn free(p: ?[*]u8, n: usize) callconv(.c) void {
            const ptr = p orelse return;
            gpa.free(ptr[0..@max(n, 1)]);
        }

        /// `sizeof(FigTableView)` here, so the binding lays one out.
        fn viewSize() callconv(.c) usize {
            return @sizeOf(table.View);
        }

        /// Parse `input` in `dialect` into this module's table and describe
        /// it in `out_view`: valid until the next parse. Returns the
        /// vtable's status; `err` is filled on a refusal, and the table's
        /// diagnostics say more.
        fn parse(dialect: [*]const u8, dialect_len: usize, input: ?[*]const u8, input_len: usize, out_view: *table.View, err: *abi.ErrorInfo) callconv(.c) c_int {
            if (last) |*t| t.deinit();
            last = null;
            const src: []const u8 = if (input) |p| p[0..input_len] else "";
            const d = gpa.dupeSentinel(u8, dialect[0..dialect_len], 0) catch return 3;
            defer gpa.free(d);
            last = table.Table.init(gpa, src);
            const rc = vt.parse(null, d.ptr, abi.Str.of(src), &last.?, err);
            out_view.* = last.?.view();
            return rc;
        }

        /// Print the table `view` shows in `dialect`; `out` is bytes the
        /// binding frees with `fig_lang_free`.
        fn print(dialect: [*]const u8, dialect_len: usize, view: *const table.View, options: *const abi.PrintOptions, out: *abi.Str, err: *abi.ErrorInfo) callconv(.c) c_int {
            const p = vt.print orelse {
                err.set("this language does not print");
                return 2;
            };
            const d = gpa.dupeSentinel(u8, dialect[0..dialect_len], 0) catch return 3;
            defer gpa.free(d);
            return p(null, d.ptr, view, options, out, err);
        }

        /// Answer one JSON-RPC request line — `describe`, `render`,
        /// `split_stream` — with one response line in `out`, which the
        /// binding frees with `fig_lang_free`. Nonzero only when there is no
        /// memory for an answer.
        fn call(request: [*]const u8, request_len: usize, out: *abi.Str) callconv(.c) c_int {
            var arena = std.heap.ArenaAllocator.init(gpa);
            defer arena.deinit();
            var w: Writer.Allocating = .init(gpa);
            defer w.deinit();
            answer(arena.allocator(), request[0..request_len], &w.writer) catch return 1;
            out.* = abi.Str.of(w.toOwnedSlice() catch return 1);
            return 0;
        }

        fn answer(a: Allocator, line: []const u8, w: *Writer) !void {
            const req = Request.read(a, line) catch {
                try w.writeAll("{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32700,\"message\":\"the request is not a JSON-RPC request\"}}");
                return;
            };
            try w.print("{{\"jsonrpc\":\"2.0\",\"id\":{d},", .{req.id});
            if (std.mem.eql(u8, req.method, "describe")) {
                try w.writeAll("\"result\":");
                try describe_json.write(w, vt);
                try w.writeByte('}');
                return;
            }
            const dialect = try a.dupeSentinel(u8, req.str("dialect"), 0);
            var out: abi.Str = .{};
            var err: abi.ErrorInfo = .empty;
            const rc: c_int = if (std.mem.eql(u8, req.method, "render")) blk: {
                const which = req.str("which");
                const f: ?abi.RenderFn = inline for (.{ "value", "entry", "item", "tail", "key", "key_name" }) |name| {
                    if (std.mem.eql(u8, which, name)) break @field(vt, "render_" ++ name);
                } else null;
                const render = f orelse return refuse(w, -32601, "this language has no such renderer");
                const literal = try a.dupeSentinel(u8, if (req.has("literal")) req.str("literal") else "string", 0);
                const r: abi.RenderRequest = .{
                    .dialect = dialect.ptr,
                    .indent = abi.Str.of(req.str("indent")),
                    .key = abi.Str.of(req.str("key")),
                    .value = abi.Str.of(req.str("value")),
                    .literal = literal.ptr,
                    .old_key = abi.Str.of(req.str("old_key")),
                    .parent_key = abi.Str.of(req.str("parent_key")),
                    .parent_tag = abi.Str.of(req.str("parent_tag")),
                };
                break :blk render(null, &r, &out, &err);
            } else if (std.mem.eql(u8, req.method, "split_stream")) blk: {
                const split = vt.split_stream orelse return refuse(w, -32601, "this language has no streams");
                break :blk split(null, dialect.ptr, abi.Str.of(req.str("input")), &out, &err);
            } else return refuse(w, -32601, "no such method");
            if (rc != 0) {
                try w.writeAll("\"error\":{\"code\":-32000,\"message\":");
                try std.json.Stringify.encodeJsonString(err.text(), .{}, w);
                try w.print(",\"data\":{{\"byte_offset\":{d}}}}}}}", .{err.byte_offset});
                return;
            }
            const bytes = out.slice() orelse "";
            defer if (bytes.len > 0) vt.free_bytes(null, out);
            try w.writeAll("\"result\":");
            if (std.mem.eql(u8, req.method, "render")) {
                try std.json.Stringify.encodeJsonString(bytes, .{}, w);
            } else {
                const docs = std.mem.bytesAsSlice(abi.StreamDoc, @as([]align(1) const u8, bytes));
                try w.writeByte('[');
                for (docs, 0..) |doc, i| {
                    if (i > 0) try w.writeByte(',');
                    try w.print("{{\"start\":{d},\"end\":{d},\"explicit\":{}}}", .{ doc.start, doc.end, doc.marked });
                }
                try w.writeByte(']');
            }
            try w.writeByte('}');
        }

        fn refuse(w: *Writer, code: i32, message: []const u8) !void {
            try w.print("\"error\":{{\"code\":{d},\"message\":\"{s}\"}}}}", .{ code, message });
        }
    };
}

/// A JSON-RPC request with an integer id and an object of string (or
/// boolean) params — all a language module is asked. Read by a walk of the
/// tokens, so no float parser is linked in.
const Request = struct {
    id: i64,
    method: []const u8,
    names: []const []const u8,
    values: []const []const u8,

    fn str(self: Request, name: []const u8) []const u8 {
        for (self.names, self.values) |n, v| if (std.mem.eql(u8, n, name)) return v;
        return "";
    }

    fn has(self: Request, name: []const u8) bool {
        for (self.names) |n| if (std.mem.eql(u8, n, name)) return true;
        return false;
    }

    fn read(a: Allocator, line: []const u8) !Request {
        var s = std.json.Scanner.initCompleteInput(a, line);
        var out: Request = .{ .id = 0, .method = "", .names = &.{}, .values = &.{} };
        var names: std.ArrayList([]const u8) = .empty;
        var values: std.ArrayList([]const u8) = .empty;
        if (try s.next() != .object_begin) return error.NotARequest;
        while (true) {
            const key = switch (try s.nextAlloc(a, .alloc_always)) {
                .object_end => break,
                .allocated_string => |k| k,
                else => return error.NotARequest,
            };
            if (std.mem.eql(u8, key, "id")) {
                out.id = switch (try s.nextAlloc(a, .alloc_always)) {
                    .allocated_number => |n| try std.fmt.parseInt(i64, n, 10),
                    else => return error.NotARequest,
                };
            } else if (std.mem.eql(u8, key, "method")) {
                out.method = switch (try s.nextAlloc(a, .alloc_always)) {
                    .allocated_string => |m| m,
                    else => return error.NotARequest,
                };
            } else if (std.mem.eql(u8, key, "params")) {
                if (try s.next() != .object_begin) return error.NotARequest;
                while (true) {
                    const name = switch (try s.nextAlloc(a, .alloc_always)) {
                        .object_end => break,
                        .allocated_string => |n| n,
                        else => return error.NotARequest,
                    };
                    switch (try s.peekNextTokenType()) {
                        .string => switch (try s.nextAlloc(a, .alloc_always)) {
                            .allocated_string => |v| {
                                try names.append(a, name);
                                try values.append(a, v);
                            },
                            else => return error.NotARequest,
                        },
                        else => try s.skipValue(),
                    }
                }
            } else try s.skipValue();
        }
        out.names = names.items;
        out.values = values.items;
        return out;
    }
};

test "a module's table, copied byte for byte into a fresh one, is adopted" {
    const AST = @import("ast/ast.zig");
    const L = @import("../languages/json/json.zig").Language;
    const V = contract.VTable(L, "");
    const src = "{\"a\": 1, \"b\": [true, null, \"s\"]}";
    var a = table.Table.init(std.testing.allocator, src);
    defer a.deinit();
    var err: abi.ErrorInfo = .empty;
    try std.testing.expectEqual(@as(c_int, 0), V.vtable.parse(null, "json", abi.Str.of(src), &a, &err));
    var b = table.Table.init(std.testing.allocator, src);
    defer b.deinit();
    try b.rows.appendSlice(std.testing.allocator, a.rows.items);
    try b.strings.appendSlice(std.testing.allocator, a.strings.items);
    try b.seps.appendSlice(std.testing.allocator, a.seps.items);
    try b.comments.appendSlice(std.testing.allocator, a.comments.items);
    var ast = try AST.adopt(std.testing.allocator, src, &b, .required);
    ast.deinit();
}
