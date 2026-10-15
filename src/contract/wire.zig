//! The helper wire as a vtable over any transport.
//!
//! The out-of-process carrier of the runtime-language contract is JSON-RPC
//! 2.0, one message per line — `describe` once, then `parse`, `print`,
//! `render` and `split_stream` per call, one request to one response —
//! specified in `docs/helper-protocol.md`. This module is fig's half of it:
//! a `Runtime.VTable` whose functions write a request, hand it to a
//! `Transport`, and read the response back into the C shapes the registry
//! takes. The envelope is zigjr's (a fork kept for fig and twig); the
//! methods and their payloads are fig's. What carries the line is the
//! transport's business, and there are two: the CLI's helper runner
//! (`src/cli/languages.zig`), where the line crosses a child process's
//! stdin and stdout, and the wasm module's host call (`src/wasm_host.zig`),
//! where it crosses into JavaScript and back. Both register through the
//! same `Runtime.register` a host's own vtable does; there is no second way
//! in.
//!
//! `tableToJson` and `tableFromJson` are the node table on the wire, and
//! are also what `fig lang table` prints and `fig lang check` holds a twin
//! to — the shape a helper author reads is the shape this module writes.

const std = @import("std");
const Io = std.Io;
const Allocator = std.mem.Allocator;
const zigjr = @import("zigjr");

const Runtime = @import("runtime.zig");
const manifest = @import("kit").manifest;
const Language = @import("language.zig");
const AST = @import("kit").AST;
const Span = @import("kit").Span;
const Document = @import("kit").Document;

/// Where a request line goes and a response line comes from. Embedded in
/// whatever owns the connection — the CLI's `Helper`, the wasm host's
/// language record — which recovers itself with `@fieldParentPtr` in
/// `exchangeFn`. The vtable's `ctx` is a pointer to this.
pub const Transport = struct {
    /// Every allocation the vtable makes per call — request buffers, the
    /// arena a table is decoded into, the bytes a print returns — comes
    /// from here, and `free_table` / `free_bytes` return it here.
    allocator: Allocator,
    /// Send one request line and take one response line, allocated in
    /// `out_arena`, which the caller owns. Neither line carries its
    /// newline. An error is reported by name to whoever asked, so the set
    /// is the transport's own; `error.HelperExited` is the one every
    /// transport can raise.
    exchangeFn: *const fn (self: *Transport, out_arena: Allocator, request: []const u8) anyerror![]const u8,
    /// The id the last request carried; each request takes the next, and
    /// its response must carry the same.
    last_id: i64 = 0,
};

/// What a helper answered a request with.
pub const Answer = union(enum) {
    /// The method's result, as a tree in the caller's arena. A number that
    /// is not an integer is its text (`.number_string`): the wire carries no
    /// floats, and reading one would link the float parser into every host.
    result: std.json.Value,
    /// The helper's error object.
    refused: Refused,
};

/// A JSON-RPC error: `code` as `docs/helper-protocol.md` assigns them,
/// `message` for a person, and `byte_offset` from `data` where the helper
/// gave one.
pub const Refused = struct {
    code: i64,
    message: []const u8,
    byte_offset: usize = 0,

    /// The message to report: the helper's own, or, for the codes JSON-RPC
    /// reserves for a request the helper could not read, that said first.
    pub fn text(self: Refused, arena: Allocator) []const u8 {
        if (self.code >= -32768 and self.code <= -32100) {
            return std.fmt.allocPrint(arena, "the helper could not handle fig's request (JSON-RPC error {d}): {s}", .{ self.code, self.message }) catch self.message;
        }
        return self.message;
    }
};

/// The ways an answer can fail to be one, beyond the transport's own.
pub const AnswerError = error{
    /// The line is not a JSON-RPC response object.
    HelperSpokeNoJson,
    /// The line is a JSON object without `"jsonrpc":"2.0"`: a helper built
    /// against fig 5, whose wire was `{"op":…}` / `{"ok":…}`.
    HelperSpeaksOldWire,
    /// The response's id is not the request's.
    HelperOutOfStep,
    OutOfMemory,
};

/// Send `method` with `params` (a JSON object's text, or null for none)
/// and read the answer, every string of it in `out_arena`.
pub fn call(t: *Transport, out_arena: Allocator, method: []const u8, params: ?[]const u8) anyerror!Answer {
    t.last_id += 1;
    const id = t.last_id;
    var req: Io.Writer.Allocating = .init(out_arena);
    try zigjr.composer.writeRequestJson(method, params, zigjr.RpcId.of(id), &req.writer);
    const line = try t.exchangeFn(t, out_arena, req.written());
    return readAnswer(out_arena, line, id);
}

/// One response line, checked against the request's `id`.
pub fn readAnswer(arena: Allocator, line: []const u8, id: i64) AnswerError!Answer {
    var scanner = std.json.Scanner.initCompleteInput(arena, line);
    const first = scanner.peekNextTokenType() catch return error.HelperSpokeNoJson;
    if (first != .object_begin) return error.HelperSpokeNoJson;
    const resp = zigjr.RpcResponse.jsonParse(arena, &scanner, .{ .ignore_unknown_fields = true, .allocate = .alloc_always }) catch |err| return switch (err) {
        error.OutOfMemory => error.OutOfMemory,
        else => error.HelperSpokeNoJson,
    };
    const end = scanner.next() catch return error.HelperSpokeNoJson;
    if (end != .end_of_document) return error.HelperSpokeNoJson;
    if (!std.mem.eql(u8, &resp.jsonrpc, "2.0")) return error.HelperSpeaksOldWire;
    if (resp.id != .num or resp.id.num != id) return error.HelperOutOfStep;
    const e = resp.@"error";
    if (e.code != 0 or e.message.len != 0) {
        var offset: usize = 0;
        if (e.data) |d| if (d == .object) if (d.object.get("byte_offset")) |o| {
            if (o == .integer and o.integer >= 0) offset = @intCast(o.integer);
        };
        return .{ .refused = .{ .code = e.code, .message = e.message, .byte_offset = offset } };
    }
    return .{ .result = resp.result };
}

/// A sentence for a failed call, where `@errorName` would be a Zig name.
pub fn failureText(err: anyerror) []const u8 {
    return switch (err) {
        error.HelperSpeaksOldWire => "the helper speaks fig 5's wire, not JSON-RPC 2.0; rebuild it against fig 6",
        error.HelperOutOfStep => "the helper answered a different request than the one fig sent",
        error.HelperSpokeNoJson => "the helper's answer is not a JSON-RPC response",
        else => @errorName(err),
    };
}

pub const DescribeError = error{
    /// The transport failed, or the helper answered `describe` with a
    /// refusal; `Runtime.lastRefusal` has the reason.
    HelperRefused,
    /// The description is not the shape the wire documents.
    HelperSpokeNoJson,
    OutOfMemory,
};

/// Ask the transport to `describe` itself and build the vtable that
/// description declares, its strings and arrays in `arena` — which must
/// live as long as the registration does, since `Runtime.register` copies
/// the record but the thunks read `ctx` for the life of the process. The
/// caller registers the result; the CLI checks the name against its
/// configuration first.
pub fn describe(t: *Transport, arena: Allocator) DescribeError!Runtime.VTable {
    const answer = call(t, arena, "describe", null) catch |err| {
        Runtime.refuse("the helper did not describe itself: {s}", .{failureText(err)});
        return error.HelperRefused;
    };
    const desc = switch (answer) {
        .result => |r| r,
        .refused => |r| {
            Runtime.refuse("the helper refused to describe itself: {s}", .{r.text(arena)});
            return error.HelperRefused;
        },
    };
    return vtableFromDescription(arena, t, desc) catch |err| switch (err) {
        error.OutOfMemory => error.OutOfMemory,
        else => error.HelperSpokeNoJson,
    };
}

// ── description → vtable ───────────────────────────────────────────────────

fn zstr(a: Allocator, s: []const u8) ![*:0]const u8 {
    return (try a.dupeSentinel(u8, s, 0)).ptr;
}

fn optString(a: Allocator, obj: std.json.ObjectMap, key: []const u8) !?[*:0]const u8 {
    const v = obj.get(key) orelse return null;
    return switch (v) {
        .string => |s| try zstr(a, s),
        else => null,
    };
}

fn boolOr(obj: std.json.ObjectMap, key: []const u8, default: bool) bool {
    const v = obj.get(key) orelse return default;
    return if (v == .bool) v.bool else default;
}

fn enumOrdinal(comptime E: type, obj: std.json.ObjectMap, key: []const u8, default: c_int) c_int {
    const v = obj.get(key) orelse return default;
    if (v != .string) return default;
    inline for (@typeInfo(E).@"enum".field_names, @typeInfo(E).@"enum".field_values) |f_name, f_value| {
        if (std.mem.eql(u8, f_name, v.string)) return @intCast(f_value);
    }
    return default;
}

fn delimiterOf(a: Allocator, v: ?std.json.Value) !Runtime.CommentDelimiterDesc {
    const d = v orelse return .{};
    if (d != .object) return .{};
    return .{
        .open = try optString(a, d.object, "open"),
        .close = try optString(a, d.object, "close"),
        .forbidden = try optString(a, d.object, "forbidden"),
    };
}

/// A dialect's `prints` object as `DialectDesc` fields, or null for none:
/// `holds` an object of the kinds held (as `lossless` is), `typed_scalars`,
/// `null_at_root` and `root_container` booleans, `char_as` and
/// `number_special_as` strings, and `comments` one of "always", "pretty" or
/// "never".
fn printsOfWire(a: Allocator, v: ?std.json.Value) !?struct { holds: u32, flags: u32, char_as: ?[*:0]const u8, number_special_as: ?[*:0]const u8, comments: c_int } {
    const pv = v orelse return null;
    if (pv != .object) return null;
    const o = pv.object;
    var kinds: manifest.NativeKinds = .{};
    if (o.get("holds")) |h| if (h == .object) {
        inline for (@typeInfo(manifest.NativeKinds).@"struct".field_names) |f_name| {
            @field(kinds, f_name) = boolOr(h.object, f_name, false);
        }
    };
    var flags: u32 = 0;
    if (!boolOr(o, "typed_scalars", true)) flags |= Runtime.prints_text_scalars;
    if (boolOr(o, "null_at_root", false)) flags |= Runtime.prints_null_at_root;
    if (boolOr(o, "root_container", false)) flags |= Runtime.prints_root_container;
    const comments: c_int = if (o.get("comments")) |c| (if (c == .string) (if (std.mem.eql(u8, c.string, "pretty")) 1 else if (std.mem.eql(u8, c.string, "never")) 2 else 0) else 0) else 0;
    return .{
        .holds = Runtime.losslessBits(kinds),
        .flags = flags,
        .char_as = try optString(a, o, "char_as"),
        .number_special_as = try optString(a, o, "number_special_as"),
        .comments = comments,
    };
}

fn syntaxOf(a: Allocator, v: std.json.Value) !*const Runtime.SyntaxDesc {
    if (v != .object) return error.HelperSpokeNoJson;
    const o = v.object;
    const s = try a.create(Runtime.SyntaxDesc);
    var comments: Runtime.CommentsDesc = .{ .line = .{}, .trailing = .{} };
    if (o.get("comments")) |c| if (c == .object) {
        comments.line = try delimiterOf(a, c.object.get("line"));
        comments.trailing = try delimiterOf(a, c.object.get("trailing"));
        comments.block = try delimiterOf(a, c.object.get("block"));
    };
    var closed: Runtime.ClosedContainersDesc = .{};
    if (o.get("closed_containers")) |c| if (c == .object) {
        closed = .{
            .map_open = try optString(a, c.object, "map_open"),
            .map_close = try optString(a, c.object, "map_close"),
            .seq_open = try optString(a, c.object, "seq_open"),
            .seq_close = try optString(a, c.object, "seq_close"),
        };
    };
    var header: Runtime.SectionHeaderDesc = .{};
    if (o.get("section_header")) |h| if (h == .object) {
        header = .{
            .open = try optString(a, h.object, "open"),
            .close = try optString(a, h.object, "close"),
            .seq_open = try optString(a, h.object, "seq_open"),
            .seq_close = try optString(a, h.object, "seq_close"),
            .sep = try optString(a, h.object, "sep"),
            .skip_index = boolOr(h.object, "skip_index", true),
        };
    };
    const sigil = byteOr0(o, "key_sigil");
    s.* = .{
        .comments = comments,
        .kv_sep = try optString(a, o, "kv_sep"),
        .flow_kv_sep_from_siblings = boolOr(o, "flow_kv_sep_from_siblings", false),
        .flow_map_pad = try optString(a, o, "flow_map_pad"),
        .key_sigil = sigil,
        .empty_map_literal = try optString(a, o, "empty_map_literal"),
        .block_seq_editable = boolOr(o, "block_seq_editable", true),
        .flow_containers = boolOr(o, "flow_containers", true),
        .indent_unit = try optString(a, o, "indent_unit"),
        .seq_item_marker = try optString(a, o, "seq_item_marker"),
        .closed_containers = closed,
        .single_line_block_mapping = boolOr(o, "single_line_block_mapping", false),
        .bare_document_mapping = boolOr(o, "bare_document_mapping", true),
        .flow_map_open = try optString(a, o, "flow_map_open"),
        .flow_map_close = try optString(a, o, "flow_map_close"),
        .structural_indent = boolOr(o, "structural_indent", false),
        .section_noun = try optString(a, o, "section_noun"),
        .section_header = header,
        .merge_key = try optString(a, o, "merge_key"),
        .flow_entry_sep = try optString(a, o, "flow_entry_sep"),
        .flow_root = boolOr(o, "flow_root", false),
        .flow_maps_only = boolOr(o, "flow_maps_only", false),
        .line_prefix_marks = try optString(a, o, "line_prefix_marks"),
        .flow_map_kv_sep = try optString(a, o, "flow_map_kv_sep"),
        .flow_map_entry_sep = try optString(a, o, "flow_map_entry_sep"),
        .line_continuation = byteOr0(o, "line_continuation"),
        .empty_seq_literal = try optString(a, o, "empty_seq_literal"),
        .opener_line_comment = boolOr(o, "opener_line_comment", false),
    };
    return s;
}

/// The byte `o[name]` states as an integer, or 0 — the C record's "none" —
/// when it is absent or not a byte.
fn byteOr0(o: std.json.ObjectMap, name: []const u8) u8 {
    const v = o.get(name) orelse return 0;
    return if (v == .integer and v.integer > 0 and v.integer < 256) @intCast(v.integer) else 0;
}

fn vtableFromDescription(a: Allocator, t: *Transport, desc: std.json.Value) !Runtime.VTable {
    if (desc != .object) return error.HelperSpokeNoJson;
    const o = desc.object;
    const name = o.get("name") orelse return error.HelperSpokeNoJson;
    if (name != .string) return error.HelperSpokeNoJson;

    var caps: u32 = 0;
    if (o.get("caps")) |c| if (c == .object) {
        if (boolOr(c.object, "read", false)) caps |= Runtime.cap_read;
        if (boolOr(c.object, "edit", false)) caps |= Runtime.cap_edit;
        if (boolOr(c.object, "serialize", false)) caps |= Runtime.cap_serialize;
        if (boolOr(c.object, "references", false)) caps |= Runtime.cap_references;
    };
    // `null` (or absent) is unbounded; 0 is a flat format.
    const depth: c_int = if (o.get("max_mapping_depth")) |d| (if (d == .integer and d.integer >= 0 and d.integer < 256) @intCast(d.integer) else Runtime.no_depth_limit) else Runtime.no_depth_limit;

    // An object is the envelope, with the kinds it names held natively.
    var lossless: u32 = 0;
    if (o.get("lossless")) |l| if (l == .object) {
        var kinds: manifest.NativeKinds = .{};
        inline for (@typeInfo(manifest.NativeKinds).@"struct".field_names) |f_name| {
            @field(kinds, f_name) = boolOr(l.object, f_name, false);
        }
        lossless = Runtime.losslessBits(kinds);
    };

    var syntax: ?*const Runtime.SyntaxDesc = null;
    if (o.get("syntax")) |sv| if (sv == .object) {
        syntax = try syntaxOf(a, sv);
    };

    var dialects: std.ArrayList(Runtime.DialectDesc) = .empty;
    if (o.get("dialects")) |dv| if (dv == .array) {
        for (dv.array.items) |d| {
            if (d != .object) continue;
            const dn = d.object.get("name") orelse continue;
            if (dn != .string) continue;
            var exts: std.ArrayList(?[*:0]const u8) = .empty;
            if (d.object.get("extensions")) |ev| if (ev == .array) {
                for (ev.array.items) |x| if (x == .string) try exts.append(a, try zstr(a, x.string));
            };
            try exts.append(a, null);
            const splice: c_int = if (d.object.get("splice")) |sp| (if (sp == .string) (if (std.mem.eql(u8, sp.string, "escaped")) 1 else if (std.mem.eql(u8, sp.string, "raw")) 2 else 0) else 0) else 0;
            const pv = try printsOfWire(a, d.object.get("prints"));
            try dialects.append(a, .{
                .name = try zstr(a, dn.string),
                .extensions = (try exts.toOwnedSlice(a)).ptr,
                .splice = splice,
                .empty_doc_seed = try optString(a, d.object, "empty_doc_seed"),
                .syntax = if (d.object.get("syntax")) |sv| (if (sv == .object) try syntaxOf(a, sv) else null) else null,
                .prints_holds = if (pv) |p| p.holds else 0,
                .prints_flags = if (pv) |p| p.flags else 0,
                .prints_char_as = if (pv) |p| p.char_as else null,
                .prints_number_special_as = if (pv) |p| p.number_special_as else null,
                .prints_comments = if (pv) |p| p.comments else 0,
            });
        }
    };

    var samples: std.ArrayList(Runtime.Str) = .empty;
    if (o.get("samples")) |sv| if (sv == .array) {
        for (sv.array.items) |x| if (x == .string) try samples.append(a, Runtime.Str.of(try a.dupe(u8, x.string)));
    };

    var renderers: [7]bool = .{ false, false, false, false, false, false, false };
    if (o.get("renderers")) |rv| if (rv == .array) {
        for (rv.array.items) |x| if (x == .string) {
            inline for ([_][]const u8{ "value", "entry", "item", "tail", "key", "key_name", "string" }, 0..) |n, i| {
                if (std.mem.eql(u8, x.string, n)) renderers[i] = true;
            }
        };
    };

    const dialect_slice = try dialects.toOwnedSlice(a);
    const sample_slice = try samples.toOwnedSlice(a);
    return .{
        .version = Runtime.vtable_version,
        .ctx = t,
        .name = try zstr(a, name.string),
        .caps = caps,
        .max_mapping_depth = depth,
        .lossless = lossless,
        .syntax = syntax,
        .dialects = dialect_slice.ptr,
        .dialect_count = dialect_slice.len,
        .samples = sample_slice.ptr,
        .sample_count = sample_slice.len,
        .parse = parseThunk,
        .print = if (caps & Runtime.cap_serialize != 0) printThunk else null,
        .free_bytes = freeBytesThunk,
        .render_value = if (renderers[0]) renderValueThunk else null,
        .render_entry = if (renderers[1]) renderEntryThunk else null,
        .render_item = if (renderers[2]) renderItemThunk else null,
        .render_tail = if (renderers[3]) renderTailThunk else null,
        .render_key = if (renderers[4]) renderKeyThunk else null,
        .render_key_name = if (renderers[5]) renderKeyNameThunk else null,
        .split_stream = if (boolOr(o, "split_stream", false)) splitStreamThunk else null,
        .render_string = if (renderers[6]) renderStringThunk else null,
    };
}

// ── the wire, per call ─────────────────────────────────────────────────────

fn transportOf(ctx: ?*anyopaque) *Transport {
    return @ptrCast(@alignCast(ctx.?));
}

fn setErr(err: *Runtime.ErrorInfo, message: []const u8, offset: usize) void {
    err.set(message);
    err.byte_offset = offset;
}

/// Write a JSON string literal.
pub fn jsonString(w: *Io.Writer, s: []const u8) !void {
    try std.json.Stringify.encodeJsonString(s, .{}, w);
}

fn parseThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, input: Runtime.Str, out: *Runtime.Table, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    const t = transportOf(ctx);
    const src = input.slice() orelse "";
    if (!std.unicode.utf8ValidateSlice(src)) {
        setErr(err, "the input is not UTF-8, which the helper wire carries as text", 0);
        return 2;
    }
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    var req: Io.Writer.Allocating = .init(arena.allocator());
    const w = &req.writer;
    w.writeAll("{\"dialect\":") catch return 3;
    jsonString(w, std.mem.span(dialect)) catch return 3;
    w.writeAll(",\"input\":") catch return 3;
    jsonString(w, src) catch return 3;
    w.writeAll("}") catch return 3;

    const table_v = switch (call(t, arena.allocator(), "parse", req.written()) catch |e| {
        setErr(err, failureText(e), 0);
        return 2;
    }) {
        .result => |r| r,
        .refused => |r| {
            setErr(err, r.text(arena.allocator()), r.byte_offset);
            return 2;
        },
    };
    tableFromJson(table_v, out) catch |e| {
        setErr(err, switch (e) {
            error.MalformedTable => "the helper returned a malformed table",
            else => @errorName(e),
        }, 0);
        return 2;
    };
    return 0;
}

fn freeBytesThunk(ctx: ?*anyopaque, bytes: Runtime.Str) callconv(.c) void {
    const t = transportOf(ctx);
    if (bytes.slice()) |s| if (s.len > 0) t.allocator.free(s);
}

fn printThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, view: *const Runtime.table.View, options: *const Runtime.PrintOptions, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    const t = transportOf(ctx);
    var req: Io.Writer.Allocating = .init(t.allocator);
    defer req.deinit();
    const w = &req.writer;
    w.writeAll("{\"dialect\":") catch return 3;
    jsonString(w, std.mem.span(dialect)) catch return 3;
    w.writeAll(",\"table\":") catch return 3;
    tableToJson(w, view) catch return 3;
    w.print(",\"options\":{{\"pretty\":{},\"strip_comments\":{},\"indent\":{d},\"width\":{d},\"splice\":{},\"flow\":{}}}}}", .{ options.pretty, options.strip_comments, options.indent, options.width, options.splice, options.flow }) catch return 3;
    return outputCall(t, "print", req.written(), out, err);
}

/// Call `method` with `params` and hand back the string it answers. A
/// refusal's code is `err.code` when it is one of `Runtime.Refusal`'s, so a
/// printer's reason reaches the caller as the failure it names.
fn outputCall(t: *Transport, method: []const u8, params: []const u8, out: *Runtime.Str, err: *Runtime.ErrorInfo) c_int {
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const output = switch (call(t, arena.allocator(), method, params) catch |e| {
        setErr(err, failureText(e), 0);
        return 4;
    }) {
        .result => |r| r,
        .refused => |r| {
            setErr(err, r.text(arena.allocator()), 0);
            if (r.code > 0 and r.code <= std.math.maxInt(c_int)) err.code = @intCast(r.code);
            return 4;
        },
    };
    if (output != .string) {
        setErr(err, "the helper's output is not a string", 0);
        return 4;
    }
    const copy = t.allocator.dupe(u8, output.string) catch return 3;
    out.* = Runtime.Str.of(copy);
    return 0;
}

fn renderCall(ctx: ?*anyopaque, which: []const u8, r: *const Runtime.RenderRequest, out: *Runtime.Str, err: *Runtime.ErrorInfo) c_int {
    const t = transportOf(ctx);
    var req: Io.Writer.Allocating = .init(t.allocator);
    defer req.deinit();
    const w = &req.writer;
    w.print("{{\"which\":\"{s}\",\"dialect\":", .{which}) catch return 3;
    jsonString(w, std.mem.span(r.dialect)) catch return 3;
    inline for (.{ "indent", "key", "value" }) |name| {
        w.print(",\"{s}\":", .{name}) catch return 3;
        jsonString(w, @field(r, name).slice() orelse "") catch return 3;
    }
    // What fig made of the value, for the value renderer alone.
    if (std.mem.eql(u8, which, "value")) {
        w.writeAll(",\"literal\":") catch return 3;
        jsonString(w, std.mem.span(r.literal)) catch return 3;
    }
    inline for (.{ "old_key", "parent_key", "parent_tag" }) |name| {
        w.print(",\"{s}\":", .{name}) catch return 3;
        jsonString(w, @field(r, name).slice() orelse "") catch return 3;
    }
    // The string replaced, for the string renderer alone.
    if (std.mem.eql(u8, which, "string")) {
        w.writeAll(",\"old_value\":") catch return 3;
        jsonString(w, r.old_value.slice() orelse "") catch return 3;
    }
    w.writeAll("}") catch return 3;
    return outputCall(t, "render", req.written(), out, err);
}

fn renderValueThunk(ctx: ?*anyopaque, r: *const Runtime.RenderRequest, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "value", r, out, err);
}
fn renderEntryThunk(ctx: ?*anyopaque, r: *const Runtime.RenderRequest, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "entry", r, out, err);
}
fn renderItemThunk(ctx: ?*anyopaque, r: *const Runtime.RenderRequest, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "item", r, out, err);
}
fn renderTailThunk(ctx: ?*anyopaque, r: *const Runtime.RenderRequest, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "tail", r, out, err);
}
fn renderKeyThunk(ctx: ?*anyopaque, r: *const Runtime.RenderRequest, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "key", r, out, err);
}
fn renderKeyNameThunk(ctx: ?*anyopaque, r: *const Runtime.RenderRequest, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "key_name", r, out, err);
}
fn renderStringThunk(ctx: ?*anyopaque, r: *const Runtime.RenderRequest, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "string", r, out, err);
}

/// `split_stream` with `{"dialect","input"}`, answered with
/// `[{"start","end","explicit"}]` in byte offsets.
fn splitStreamThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, input: Runtime.Str, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    const t = transportOf(ctx);
    const src = input.slice() orelse "";
    if (!std.unicode.utf8ValidateSlice(src)) {
        setErr(err, "the input is not UTF-8, which the helper wire carries as text", 0);
        return 2;
    }
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    var req: Io.Writer.Allocating = .init(arena.allocator());
    const w = &req.writer;
    w.writeAll("{\"dialect\":") catch return 3;
    jsonString(w, std.mem.span(dialect)) catch return 3;
    w.writeAll(",\"input\":") catch return 3;
    jsonString(w, src) catch return 3;
    w.writeAll("}") catch return 3;

    const docs = switch (call(t, arena.allocator(), "split_stream", req.written()) catch |e| {
        setErr(err, failureText(e), 0);
        return 2;
    }) {
        .result => |r| r,
        .refused => |r| {
            setErr(err, r.text(arena.allocator()), r.byte_offset);
            return 2;
        },
    };
    if (docs != .array) {
        setErr(err, "the helper's documents are not an array", 0);
        return 2;
    }
    // Bytes, not `StreamDoc`s: `free_bytes` frees what it is handed as bytes.
    const size = @sizeOf(Runtime.StreamDoc);
    const bytes = t.allocator.alloc(u8, docs.array.items.len * size) catch return 3;
    for (docs.array.items, 0..) |d, i| {
        const start = if (d == .object) d.object.get("start") else null;
        const end = if (d == .object) d.object.get("end") else null;
        if (start == null or end == null or start.? != .integer or end.? != .integer or start.?.integer < 0 or end.?.integer < start.?.integer) {
            t.allocator.free(bytes);
            setErr(err, "a stream document is not {\"start\", \"end\"} byte offsets", 0);
            return 2;
        }
        const r: Runtime.StreamDoc = .{ .start = @intCast(start.?.integer), .end = @intCast(end.?.integer), .marked = boolOr(d.object, "explicit", false) };
        @memcpy(bytes[i * size ..][0..size], std.mem.asBytes(&r));
    }
    out.* = Runtime.Str.of(bytes);
    return 0;
}

// ── the table on the wire ──────────────────────────────────────────────────

const kind_names = [_][]const u8{ "null", "bool", "int", "float", "string", "sequence", "mapping", "keyvalue", "alias" };

fn kindOf(name: []const u8) ?c_int {
    for (kind_names, 0..) |k, i| if (std.mem.eql(u8, k, name)) return @intCast(i);
    return null;
}

const ExtKind = AST.Node.Kind.Extended.ExtKind;

fn extKindOf(name: []const u8) ?c_int {
    inline for (@typeInfo(ExtKind).@"enum".field_names, @typeInfo(ExtKind).@"enum".field_values) |f_name, f_value| {
        if (std.mem.eql(u8, f_name, name)) return @intCast(f_value);
    }
    return null;
}

const T = Runtime.table;

/// A `[start, end]` span, or `none` for an absent one.
fn spanOf(v: ?std.json.Value) !struct { start: u32, end: u32 } {
    const s = v orelse return .{ .start = T.none, .end = T.none };
    if (s == .null) return .{ .start = T.none, .end = T.none };
    if (s != .array or s.array.items.len != 2) return error.MalformedTable;
    const a = s.array.items[0];
    const b = s.array.items[1];
    if (a != .integer or b != .integer or a.integer < 0 or b.integer < 0 or a.integer >= T.none or b.integer >= T.none) return error.MalformedTable;
    return .{ .start = @intCast(a.integer), .end = @intCast(b.integer) };
}

/// A string as the table's text: in the source where it lies there, else
/// copied into the buffer.
fn textOf(t: *Runtime.Table, v: ?std.json.Value) !T.Text {
    const s = v orelse return .absent;
    return switch (s) {
        .null => .absent,
        .string => |x| try t.addString(x),
        else => error.MalformedTable,
    };
}

fn u32Of(v: ?std.json.Value) !u32 {
    const s = v orelse return error.MalformedTable;
    if (s != .integer or s.integer < 0 or s.integer >= T.none) return error.MalformedTable;
    return @intCast(s.integer);
}

/// A mention's `kind` as the table's byte: `header`, `entry`, `index` or
/// `append`, and `header` for anything else, as the wire has always read
/// a kind it does not name.
fn mentionKind(v: std.json.Value) u8 {
    const kind: Document.MentionKind = if (v == .string) std.meta.stringToEnum(Document.MentionKind, v.string) orelse .header else .header;
    return kind.byte();
}

/// The table a helper answers `parse` with, decoded into `out` through
/// fig's own calls. `error.MalformedTable` for a shape the wire does not
/// document. The wire spells a row as it always has — a bool's text is
/// `true`/`false`, an extended scalar's kind is `ext_kind` beside a
/// `string` (or `int`) row, and each side fact rides on its row — so a
/// helper written before the ABI 4 table needs no change.
pub fn tableFromJson(v: std.json.Value, out: *Runtime.Table) !void {
    if (v != .object) return error.MalformedTable;
    const o = v.object;
    const a = out.allocator;
    if (o.get("rows")) |rv| {
        if (rv != .array) return error.MalformedTable;
        for (rv.array.items, 0..) |r, i| {
            if (r != .object) return error.MalformedTable;
            const ro = r.object;
            const node: u32 = @intCast(i);
            const kind_v = ro.get("kind") orelse return error.MalformedTable;
            if (kind_v != .string) return error.MalformedTable;
            const wire_kind = kindOf(kind_v.string) orelse return error.MalformedTable;
            const parent: u32 = if (ro.get("parent")) |p| (if (p == .null) Runtime.no_node else try u32Of(p)) else Runtime.no_node;
            const span = try spanOf(ro.get("span"));
            var row: T.Row = .{ .parent = parent, .start = span.start, .end = span.end };
            if (ro.get("ext_kind")) |e| {
                if (e != .string) return error.MalformedTable;
                row.kind = T.Kind.ext + @as(u8, @intCast(extKindOf(e.string) orelse return error.MalformedTable));
                row.text = try textOf(out, ro.get("text"));
            } else switch (wire_kind) {
                1 => { // bool
                    const b = ro.get("text") orelse return error.MalformedTable;
                    if (b != .string) return error.MalformedTable;
                    if (std.mem.eql(u8, b.string, "true")) row.kind = T.Kind.true_ else if (std.mem.eql(u8, b.string, "false")) row.kind = T.Kind.false_ else return error.MalformedTable;
                },
                else => {
                    row.kind = wire_row_kinds[@intCast(wire_kind)];
                    row.text = try textOf(out, ro.get("text"));
                },
            }
            if (boolOr(ro, "open", false)) row.flags |= T.row_open;
            _ = try out.addRow(row);
            if (ro.get("anchor")) |an| if (an != .null) {
                const sp = try spanOf(ro.get("anchor_span"));
                try out.anchors.append(a, .{ .node = node, .start = sp.start, .end = sp.end, .name = try textOf(out, an) });
            };
            if (ro.get("tag")) |tg| if (tg != .null) {
                const sp = try spanOf(ro.get("tag_span"));
                try out.tags.append(a, .{ .node = node, .start = sp.start, .end = sp.end, .text = try textOf(out, tg) });
            };
            if (ro.get("marker")) |m| if (m != .null) {
                const sp = try spanOf(m);
                try out.markers.append(a, .{ .node = node, .start = sp.start, .end = sp.end });
            };
            if (ro.get("sep")) |m| if (m != .null) {
                const sp = try spanOf(m);
                try out.seps.append(a, .{ .node = node, .start = sp.start, .end = sp.end });
            };
            if (ro.get("item_sep")) |is| if (is != .null) {
                try out.item_seps.append(a, .{ .node = node, .text = try textOf(out, is) });
            };
        }
    }
    if (o.get("regions")) |rv| if (rv == .array) {
        for (rv.array.items) |r| {
            if (r != .object) return error.MalformedTable;
            try out.regions.append(a, .{ .node = try u32Of(r.object.get("node")), .start = try u32Of(r.object.get("start")), .end = try u32Of(r.object.get("end")) });
        }
    };
    if (o.get("mentions")) |mv| if (mv == .array) {
        for (mv.array.items) |m| {
            if (m != .object) return error.MalformedTable;
            const k = m.object.get("kind") orelse return error.MalformedTable;
            const sp = try spanOf(m.object.get("span"));
            try out.mentions.append(a, .{
                .node = try u32Of(m.object.get("node")),
                .start = sp.start,
                .end = sp.end,
                .kind = mentionKind(k),
            });
        }
    };
    if (o.get("comments")) |cv| if (cv == .array) {
        for (cv.array.items) |c| {
            if (c != .object) return error.MalformedTable;
            const slot_v = c.object.get("slot") orelse return error.MalformedTable;
            const style_v = c.object.get("style") orelse return error.MalformedTable;
            if (slot_v != .string or style_v != .string) return error.MalformedTable;
            const slot: u8 = if (std.mem.eql(u8, slot_v.string, "trailing")) T.comment_trailing else if (std.mem.eql(u8, slot_v.string, "dangling")) T.comment_dangling else T.comment_leading;
            const style: u8 = if (std.mem.eql(u8, style_v.string, "block")) T.comment_block else T.comment_line;
            const text = try textOf(out, c.object.get("text"));
            try out.comments.append(a, .{ .node = try u32Of(c.object.get("node")), .slot = slot, .style = style, .text = if (text.isAbsent()) try out.addString("") else text });
        }
    };
    if (o.get("diagnostics")) |dv| if (dv == .array) {
        for (dv.array.items) |d| {
            if (d != .object) return error.MalformedTable;
            const sp = try spanOf(d.object.get("span"));
            const sev = d.object.get("severity");
            const warning = sev != null and sev.? == .string and std.mem.eql(u8, sev.?.string, "warning");
            try out.diagnostics.append(a, .{
                .severity = if (warning) T.diagnostic_warning else T.diagnostic_error,
                .start = if (sp.start == T.none) 0 else sp.start,
                .end = sp.end,
                .message = try textOf(out, d.object.get("message")),
                .label = try textOf(out, d.object.get("label")),
            });
        }
    };
    if (o.get("directives")) |dv| if (dv == .array) {
        for (dv.array.items) |d| {
            if (d != .object) return error.MalformedTable;
            const handle = try textOf(out, d.object.get("handle"));
            const prefix = try textOf(out, d.object.get("prefix"));
            if (handle.isAbsent() or prefix.isAbsent()) return error.MalformedTable;
            try out.directives.append(a, .{ .handle = handle, .prefix = prefix });
        }
    };
}

/// The table kind of each wire kind but `bool`, which is two.
const wire_row_kinds = [_]u8{ T.Kind.null_, 0, T.Kind.int, T.Kind.float, T.Kind.string, T.Kind.sequence, T.Kind.mapping, T.Kind.keyvalue, T.Kind.alias };

fn writeSpan(w: *Io.Writer, key: []const u8, start: u32, end: u32) !void {
    if (start != T.none) try w.print(",\"{s}\":[{d},{d}]", .{ key, start, end });
}

fn writeText(w: *Io.Writer, v: *const T.View, key: []const u8, t: T.Text) !void {
    if (v.textOf(t)) |x| {
        try w.print(",\"{s}\":", .{key});
        try jsonString(w, x);
    }
}

/// A table as the JSON a helper answers `parse` with and is handed for
/// `print`: an absent optional is omitted, not `null`. The rows' side facts
/// ride on their rows, as the wire has always spelled them.
pub fn tableToJson(w: *Io.Writer, v: *const T.View) !void {
    try w.writeAll("{\"rows\":[");
    for (v.rowSlice(), 0..) |r, i| {
        if (i > 0) try w.writeByte(',');
        const node: u32 = @intCast(i);
        const K = T.Kind;
        if (r.kind >= K.ext) {
            const ek: ExtKind = @fromBackingInt(@intCast(r.kind - K.ext));
            try w.print("{{\"kind\":\"{s}\",\"ext_kind\":\"{s}\"", .{ if (ek == .char_literal) "int" else "string", @tagName(ek) });
        } else {
            const name: []const u8 = switch (r.kind) {
                K.null_ => "null",
                K.false_, K.true_ => "bool",
                K.string => "string",
                K.int => "int",
                K.float => "float",
                K.sequence => "sequence",
                K.mapping => "mapping",
                K.keyvalue => "keyvalue",
                K.alias => "alias",
                else => "null",
            };
            try w.print("{{\"kind\":\"{s}\"", .{name});
        }
        if (r.parent == Runtime.no_node) try w.writeAll(",\"parent\":null") else try w.print(",\"parent\":{d}", .{r.parent});
        try writeSpan(w, "span", r.start, r.end);
        if (r.kind == K.false_) try w.writeAll(",\"text\":\"false\"") else if (r.kind == K.true_) try w.writeAll(",\"text\":\"true\"") else try writeText(w, v, "text", r.text);
        const f = v.facts(node);
        if (f.anchor) |a| {
            try writeText(w, v, "anchor", a.name);
            try writeSpan(w, "anchor_span", a.start, a.end);
        }
        if (f.tag) |t| {
            try writeText(w, v, "tag", t.text);
            try writeSpan(w, "tag_span", t.start, t.end);
        }
        if (f.marker) |m| try writeSpan(w, "marker", m.start, m.end);
        if (f.sep) |m| try writeSpan(w, "sep", m.start, m.end);
        if (f.item_sep) |is| try writeText(w, v, "item_sep", is.text);
        if (r.flags & T.row_open != 0) try w.writeAll(",\"open\":true");
        try w.writeByte('}');
    }
    try w.writeAll("],\"regions\":[");
    for (T.slice(T.SpanFact, v.regions, v.region_count), 0..) |r, i| {
        if (i > 0) try w.writeByte(',');
        try w.print("{{\"node\":{d},\"start\":{d},\"end\":{d}}}", .{ r.node, r.start, r.end });
    }
    try w.writeAll("],\"mentions\":[");
    for (T.slice(T.Mention, v.mentions, v.mention_count), 0..) |m, i| {
        if (i > 0) try w.writeByte(',');
        try w.print("{{\"node\":{d},\"span\":[{d},{d}],\"kind\":\"{s}\"}}", .{ m.node, m.start, m.end, @tagName(Document.MentionKind.of(m.kind)) });
    }
    try w.writeAll("],\"comments\":[");
    for (T.slice(T.Comment, v.comments, v.comment_count), 0..) |c, i| {
        if (i > 0) try w.writeByte(',');
        const slot: []const u8 = switch (c.slot) {
            T.comment_trailing => "trailing",
            T.comment_dangling => "dangling",
            else => "leading",
        };
        try w.print("{{\"node\":{d},\"slot\":\"{s}\",\"style\":\"{s}\",\"text\":", .{ c.node, slot, if (c.style == T.comment_block) "block" else "line" });
        try jsonString(w, v.textOf(c.text) orelse "");
        try w.writeByte('}');
    }
    try w.writeAll("]");
    // Absent, not empty, for the formats that have none — which is every
    // helper written before the column existed.
    const directives = T.slice(T.Directive, v.directives, v.directive_count);
    if (directives.len > 0) {
        try w.writeAll(",\"directives\":[");
        for (directives, 0..) |d, i| {
            if (i > 0) try w.writeByte(',');
            try w.writeAll("{\"handle\":");
            try jsonString(w, v.textOf(d.handle) orelse "");
            try w.writeAll(",\"prefix\":");
            try jsonString(w, v.textOf(d.prefix) orelse "");
            try w.writeByte('}');
        }
        try w.writeByte(']');
    }
    try w.writeAll("}");
}

// ── tests ──────────────────────────────────────────────────────────────────

/// A transport that answers from a fixed script — what a helper would say
/// — so the vtable's half of the wire is tested with no process behind it.
/// Each script is the member a response carries after its id: a
/// `"result":…` or an `"error":…`, which `exchange` wraps in the envelope
/// with the request's own id.
const Scripted = struct {
    transport: Transport,
    describe_response: []const u8,
    parse_response: []const u8,
    print_response: []const u8,
    last_request: std.ArrayList(u8) = .empty,

    fn exchange(t: *Transport, out_arena: Allocator, request: []const u8) anyerror![]const u8 {
        const self: *Scripted = @fieldParentPtr("transport", t);
        self.last_request.clearRetainingCapacity();
        try self.last_request.appendSlice(t.allocator, request);
        const member = if (std.mem.indexOf(u8, request, "\"method\": \"describe\"") != null)
            self.describe_response
        else if (std.mem.indexOf(u8, request, "\"method\": \"parse\"") != null)
            self.parse_response
        else
            self.print_response;
        return std.fmt.allocPrint(out_arena, "{{\"jsonrpc\":\"2.0\",\"id\":{d},{s}}}", .{ t.last_id, member });
    }

    /// The request's `params`, as the request spells them.
    fn params(self: *const Scripted) []const u8 {
        const req = self.last_request.items;
        const at = std.mem.indexOf(u8, req, "\"params\": ") orelse return "";
        const id_at = std.mem.lastIndexOf(u8, req, ", \"id\": ") orelse return "";
        return req[at + "\"params\": ".len .. id_at];
    }
};

test "readAnswer: a result, a refusal with its offset, and every way a line is not an answer" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    const ok = try readAnswer(a, "{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"n\":-3,\"f\":1.5,\"s\":\"a\\nb\"}}", 3);
    try t.expectEqual(@as(i64, -3), ok.result.object.get("n").?.integer);
    try t.expectEqualStrings("1.5", ok.result.object.get("f").?.number_string);
    try t.expectEqualStrings("a\nb", ok.result.object.get("s").?.string);

    const no = try readAnswer(a, "{\"jsonrpc\":\"2.0\",\"id\":4,\"error\":{\"code\":-32000,\"message\":\"bad\",\"data\":{\"byte_offset\":7}}}", 4);
    try t.expectEqual(@as(i64, -32000), no.refused.code);
    try t.expectEqualStrings("bad", no.refused.text(a));
    try t.expectEqual(@as(usize, 7), no.refused.byte_offset);

    const unknown = try readAnswer(a, "{\"jsonrpc\":\"2.0\",\"id\":5,\"error\":{\"code\":-32601,\"message\":\"no such method\"}}", 5);
    try t.expect(std.mem.indexOf(u8, unknown.refused.text(a), "could not handle") != null);

    try t.expectError(error.HelperSpeaksOldWire, readAnswer(a, "{\"ok\":true,\"output\":\"x\"}", 1));
    try t.expectError(error.HelperOutOfStep, readAnswer(a, "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":1}", 1));
    try t.expectError(error.HelperSpokeNoJson, readAnswer(a, "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":1} trailing", 1));
    try t.expectError(error.HelperSpokeNoJson, readAnswer(a, "not json", 1));
    try t.expectError(error.HelperSpokeNoJson, readAnswer(a, "[]", 1));
}

test "describe builds a vtable the registry would accept" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    var s: Scripted = .{
        .transport = .{ .allocator = t.allocator, .exchangeFn = Scripted.exchange },
        .describe_response =
        \\"result":{"name":"wire-kv","caps":{"read":true,"edit":true,"serialize":true},
        \\"max_mapping_depth":0,"syntax":{"comments":{"line":{"open":"#"},"trailing":{"open":"#"}},
        \\"kv_sep":"=","empty_map_literal":"{}","flow_containers":false},
        \\"dialects":[{"name":"wire-kv","extensions":["wkv"],"splice":"raw","empty_doc_seed":""}],
        \\"samples":["a=1\n"],"renderers":["value"]}
        ,
        .parse_response = "",
        .print_response = "",
    };
    defer s.last_request.deinit(t.allocator);
    const vt = try describe(&s.transport, arena.allocator());
    try t.expectEqualStrings("wire-kv", std.mem.span(vt.name));
    try t.expectEqual(Runtime.cap_read | Runtime.cap_edit | Runtime.cap_serialize, vt.caps);
    try t.expectEqual(@as(c_int, 0), vt.max_mapping_depth);
    try t.expectEqual(@as(usize, 1), vt.dialect_count);
    try t.expectEqualStrings("wkv", std.mem.span(vt.dialects[0].extensions.?[0].?));
    try t.expectEqual(@as(c_int, 2), vt.dialects[0].splice);
    try t.expectEqualStrings("=", std.mem.span(vt.syntax.?.kv_sep.?));
    try t.expect(!vt.syntax.?.flow_containers);
    try t.expect(vt.render_value != null);
    try t.expect(vt.render_entry == null);
    try t.expect(vt.print != null);
    try t.expectEqual(@as(*anyopaque, @ptrCast(&s.transport)), vt.ctx.?);
}

test "a render request carries the container the fragment goes into" {
    const t = std.testing;
    var s: Scripted = .{
        .transport = .{ .allocator = t.allocator, .exchangeFn = Scripted.exchange },
        .describe_response = "",
        .parse_response = "",
        .print_response = "\"result\":\"<dependency>x</dependency>\"",
    };
    defer s.last_request.deinit(t.allocator);
    const request: Runtime.RenderRequest = .{
        .dialect = "wire-kv",
        .indent = Runtime.Str.of("  "),
        .value = Runtime.Str.of("x"),
        .literal = "string",
        .parent_key = Runtime.Str.of("dependencies"),
        .parent_tag = Runtime.Str.of("!dependency"),
    };
    var out: Runtime.Str = .{};
    var err: Runtime.ErrorInfo = .empty;
    try t.expectEqual(@as(c_int, 0), renderItemThunk(@ptrCast(&s.transport), &request, &out, &err));
    defer freeBytesThunk(@ptrCast(&s.transport), out);
    try t.expectEqualStrings("<dependency>x</dependency>", out.slice().?);
    try t.expect(std.mem.indexOf(u8, s.last_request.items, "\"method\": \"render\"") != null);
    try t.expectEqualStrings(
        \\{"which":"item","dialect":"wire-kv","indent":"  ","key":"","value":"x","old_key":"","parent_key":"dependencies","parent_tag":"!dependency"}
    , s.params());
}

test "a refusal to describe is reported through Runtime.refuse" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    var s: Scripted = .{
        .transport = .{ .allocator = t.allocator, .exchangeFn = Scripted.exchange },
        .describe_response = "\"error\":{\"code\":-32000,\"message\":\"no script given\"}",
        .parse_response = "",
        .print_response = "",
    };
    defer s.last_request.deinit(t.allocator);
    try t.expectError(error.HelperRefused, describe(&s.transport, arena.allocator()));
    try t.expect(std.mem.indexOf(u8, Runtime.lastRefusal(), "no script given") != null);
}

test "parse crosses as a request line and comes back as a table; print gets the table back" {
    const t = std.testing;
    var s: Scripted = .{
        .transport = .{ .allocator = t.allocator, .exchangeFn = Scripted.exchange },
        .describe_response = "",
        .parse_response =
        \\"result":{"rows":[{"kind":"mapping","parent":null,"span":[0,4]},
        \\{"kind":"keyvalue","parent":0,"span":[0,3],"sep":[1,2]},{"kind":"string","parent":1,"span":[0,1],"text":"a"},
        \\{"kind":"string","parent":1,"span":[2,3],"text":"1"}],"regions":[],"mentions":[],
        \\"comments":[{"node":2,"slot":"leading","style":"line","text":"hi"}],
        \\"directives":[{"handle":"!e!","prefix":"tag:x/"}]}
        ,
        .print_response = "\"result\":\"a=1\\n\"",
    };
    defer s.last_request.deinit(t.allocator);
    var table = Runtime.Table.init(t.allocator, "a=1\n");
    defer table.deinit();
    var err: Runtime.ErrorInfo = .empty;
    try t.expectEqual(@as(c_int, 0), parseThunk(&s.transport, "wire-kv", Runtime.Str.of("a=1\n"), &table, &err));
    try t.expectEqualStrings("{\"jsonrpc\": \"2.0\", \"method\": \"parse\", \"params\": {\"dialect\":\"wire-kv\",\"input\":\"a=1\\n\"}, \"id\": 1}", s.last_request.items);
    try t.expectEqual(@as(usize, 4), table.rows.items.len);
    try t.expectEqual(@as(usize, 1), table.comments.items.len);
    try t.expectEqual(@as(usize, 1), table.directives.items.len);
    try t.expectEqualStrings("tag:x/", table.textOf(table.directives.items[0].prefix).?);
    try t.expectEqualStrings("a", table.textOf(table.rows.items[2].text).?);
    try t.expectEqual(@as(usize, 1), table.seps.items.len);
    try t.expectEqual(@as(u32, 1), table.seps.items[0].node);
    try t.expectEqual(@as(u32, 1), table.seps.items[0].start);

    var out: Runtime.Str = .none;
    const options: Runtime.PrintOptions = .{ .pretty = true, .strip_comments = false, .indent = 2, .width = 80 };
    const view = table.view();
    try t.expectEqual(@as(c_int, 0), printThunk(&s.transport, "wire-kv", &view, &options, &out, &err));
    try t.expectEqualStrings("a=1\n", out.slice().?);
    // The print request carries the table as `parse` answered it.
    try t.expect(std.mem.indexOf(u8, s.last_request.items, "\"sep\":[1,2]") != null);
    try t.expect(std.mem.indexOf(u8, s.last_request.items, "\"slot\":\"leading\"") != null);
    try t.expect(std.mem.indexOf(u8, s.last_request.items, "\"directives\":[{\"handle\":\"!e!\",\"prefix\":\"tag:x/\"}]") != null);
    try t.expect(std.mem.indexOf(u8, s.last_request.items, "\"options\":{\"pretty\":true,\"strip_comments\":false,\"indent\":2,\"width\":80,\"splice\":false,\"flow\":false}") != null);
    freeBytesThunk(&s.transport, out);
}

test "a parse refusal carries the helper's message and offset" {
    const t = std.testing;
    var s: Scripted = .{
        .transport = .{ .allocator = t.allocator, .exchangeFn = Scripted.exchange },
        .describe_response = "",
        .parse_response = "\"error\":{\"code\":-32000,\"message\":\"expected `=` here\",\"data\":{\"byte_offset\":3}}",
        .print_response = "",
    };
    defer s.last_request.deinit(t.allocator);
    var table = Runtime.Table.init(t.allocator, "abc");
    defer table.deinit();
    var err: Runtime.ErrorInfo = .empty;
    try t.expectEqual(@as(c_int, 2), parseThunk(&s.transport, "wire-kv", Runtime.Str.of("abc"), &table, &err));
    try t.expectEqualStrings("expected `=` here", err.text());
    try t.expectEqual(@as(usize, 3), err.byte_offset);
}
