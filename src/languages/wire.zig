//! The helper wire as a vtable over any transport.
//!
//! The out-of-process carrier of the runtime-language contract is
//! newline-delimited JSON — `describe` once, then `parse`, `print` and
//! `render` per call, one request line to one response line — documented
//! once, on `bindings/rust/fig/src/helper.rs`. This module is fig's half of
//! it: a `Runtime.VTable` whose functions write a request, hand it to a
//! `Transport`, and read the response back into the C shapes the registry
//! takes. What carries the line is the transport's business, and there are
//! two: the CLI's helper runner (`src/cli/languages.zig`), where the line
//! crosses a child process's stdin and stdout, and the wasm module's host
//! call (`src/wasm_host.zig`), where it crosses into JavaScript and back.
//! Both register through the same `Runtime.register` a host's own vtable
//! does; there is no second way in.
//!
//! `tableToJson` and `tableFromJson` are the node table on the wire, and
//! are also what `fig lang table` prints and `fig lang check` holds a twin
//! to — the shape a helper author reads is the shape this module writes.

const std = @import("std");
const Io = std.Io;
const Allocator = std.mem.Allocator;

const Runtime = @import("runtime.zig");
const Language = @import("language.zig");
const AST = @import("../ast/ast.zig");
const Span = @import("../util/span.zig");

/// Where a request line goes and a response line comes from. Embedded in
/// whatever owns the connection — the CLI's `Helper`, the wasm host's
/// language record — which recovers itself with `@fieldParentPtr` in
/// `callFn`. The vtable's `ctx` is a pointer to this.
pub const Transport = struct {
    /// Every allocation the vtable makes per call — request buffers, the
    /// arena a table is decoded into, the bytes a print returns — comes
    /// from here, and `free_table` / `free_bytes` return it here.
    allocator: Allocator,
    /// Send one request line and take one response line, parsed into a
    /// `std.json.Value` tree in `out_arena`, which the caller owns. An
    /// error is reported by name to whoever asked, so the set is the
    /// transport's own; `error.HelperExited` and `error.HelperSpokeNoJson`
    /// are the two every transport can raise.
    callFn: *const fn (self: *Transport, out_arena: Allocator, request: []const u8) anyerror!std.json.Value,

    pub fn call(self: *Transport, out_arena: Allocator, request: []const u8) anyerror!std.json.Value {
        return self.callFn(self, out_arena, request);
    }
};

/// One response line as a `std.json.Value` tree in `arena`. The wire
/// carries no floats — offsets, ids and depths are integers, everything
/// else is a string — so this walks `std.json.Scanner` itself rather than
/// going through `std.json.parseFromSlice`, whose number path links the
/// float parser into every host; a non-integer number arrives as
/// `.number_string` and is refused wherever an integer is read.
/// `error.HelperSpokeNoJson` for a line that is not one JSON value.
pub fn parseLine(arena: Allocator, line: []const u8) error{ HelperSpokeNoJson, OutOfMemory }!std.json.Value {
    var scanner = std.json.Scanner.initCompleteInput(arena, line);
    defer scanner.deinit();
    const value = parseValue(arena, &scanner, 0) catch |err| return switch (err) {
        error.OutOfMemory => error.OutOfMemory,
        else => error.HelperSpokeNoJson,
    };
    const end = scanner.next() catch return error.HelperSpokeNoJson;
    if (end != .end_of_document) return error.HelperSpokeNoJson;
    return value;
}

/// The nesting a description or a table needs; a line deeper than this is
/// not the wire's.
const max_json_depth = 32;

/// Every string and number is copied into `arena` (`.alloc_always`): the
/// line itself is the transport's, and is gone once `call` returns.
fn parseValue(arena: Allocator, scanner: *std.json.Scanner, depth: usize) anyerror!std.json.Value {
    if (depth > max_json_depth) return error.TooDeep;
    return switch (try scanner.nextAlloc(arena, .alloc_always)) {
        .null => .null,
        .true => .{ .bool = true },
        .false => .{ .bool = false },
        .number, .allocated_number => |n| if (std.fmt.parseInt(i64, n, 10)) |i| .{ .integer = i } else |_| .{ .number_string = n },
        .string, .allocated_string => |str| .{ .string = str },
        .array_begin => blk: {
            var items: std.json.Array = .init(arena);
            while (true) {
                if (try scanner.peekNextTokenType() == .array_end) {
                    _ = try scanner.next();
                    break;
                }
                try items.append(try parseValue(arena, scanner, depth + 1));
            }
            break :blk .{ .array = items };
        },
        .object_begin => blk: {
            var map: std.json.ObjectMap = .empty;
            while (true) {
                const key = switch (try scanner.nextAlloc(arena, .alloc_always)) {
                    .object_end => break,
                    .string, .allocated_string => |k| k,
                    else => return error.UnexpectedToken,
                };
                try map.put(arena, key, try parseValue(arena, scanner, depth + 1));
            }
            break :blk .{ .object = map };
        },
        else => error.UnexpectedToken,
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
    const resp = t.call(arena, "{\"op\":\"describe\"}") catch |err| {
        Runtime.refuse("the helper did not describe itself: {s}", .{@errorName(err)});
        return error.HelperRefused;
    };
    const desc = okField(resp, "description") orelse {
        Runtime.refuse("the helper refused to describe itself: {s}", .{failureMessage(resp)});
        return error.HelperRefused;
    };
    return vtableFromDescription(arena, t, desc) catch |err| switch (err) {
        error.OutOfMemory => error.OutOfMemory,
        else => error.HelperSpokeNoJson,
    };
}

pub fn okField(resp: std.json.Value, name: []const u8) ?std.json.Value {
    if (resp != .object) return null;
    const ok = resp.object.get("ok") orelse return null;
    if (ok != .bool or !ok.bool) return null;
    return resp.object.get(name);
}

pub fn failureMessage(resp: std.json.Value) []const u8 {
    if (resp != .object) return "malformed response";
    const m = resp.object.get("message") orelse return "no reason given";
    return if (m == .string) m.string else "no reason given";
}

fn failureOffset(resp: std.json.Value) usize {
    if (resp != .object) return 0;
    const m = resp.object.get("byte_offset") orelse return 0;
    return if (m == .integer and m.integer >= 0) @intCast(m.integer) else 0;
}

// ── description → vtable ───────────────────────────────────────────────────

fn zstr(a: Allocator, s: []const u8) ![*:0]const u8 {
    return (try a.dupeZ(u8, s)).ptr;
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
    inline for (@typeInfo(E).@"enum".fields) |f| {
        if (std.mem.eql(u8, f.name, v.string)) return @intCast(f.value);
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

fn syntaxOf(a: Allocator, v: std.json.Value) !*const Runtime.SyntaxDesc {
    if (v != .object) return error.HelperSpokeNoJson;
    const o = v.object;
    const s = try a.create(Runtime.SyntaxDesc);
    var comments: Runtime.CommentsDesc = .{ .style = 0, .line = .{}, .trailing = .{} };
    if (o.get("comments")) |c| if (c == .object) {
        comments.style = enumOrdinal(Language.CommentStyle, c.object, "style", 0);
        comments.line = try delimiterOf(a, c.object.get("line"));
        comments.trailing = try delimiterOf(a, c.object.get("trailing"));
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
    const sigil: u8 = if (o.get("key_sigil")) |k| (if (k == .integer and k.integer > 0 and k.integer < 256) @intCast(k.integer) else 0) else 0;
    s.* = .{
        .comments = comments,
        .kv_sep = try optString(a, o, "kv_sep"),
        .flow_kv_sep_from_siblings = boolOr(o, "flow_kv_sep_from_siblings", false),
        .flow_map_pad = try optString(a, o, "flow_map_pad"),
        .key_style = enumOrdinal(Language.KeyStyle, o, "key_style", 0),
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
        .section_noun = enumOrdinal(Language.SectionNoun, o, "section_noun", -1),
        .section_header = header,
        .merge_key = try optString(a, o, "merge_key"),
    };
    return s;
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

    var lossless: ?*const Runtime.NativeKindsDesc = null;
    if (o.get("lossless")) |l| if (l == .object) {
        const nk = try a.create(Runtime.NativeKindsDesc);
        nk.* = .{};
        inline for (@typeInfo(Runtime.NativeKindsDesc).@"struct".fields) |f| {
            @field(nk, f.name) = boolOr(l.object, f.name, false);
        }
        lossless = nk;
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
            const splice: c_int = if (d.object.get("splice")) |sp| (if (sp == .string) (if (std.mem.eql(u8, sp.string, "json_string")) 1 else if (std.mem.eql(u8, sp.string, "raw")) 2 else 0) else 0) else 0;
            try dialects.append(a, .{
                .name = try zstr(a, dn.string),
                .extensions = (try exts.toOwnedSlice(a)).ptr,
                .splice = splice,
                .empty_doc_seed = try optString(a, d.object, "empty_doc_seed"),
                .syntax = if (d.object.get("syntax")) |sv| (if (sv == .object) try syntaxOf(a, sv) else null) else null,
            });
        }
    };

    var samples: std.ArrayList(Runtime.Str) = .empty;
    if (o.get("samples")) |sv| if (sv == .array) {
        for (sv.array.items) |x| if (x == .string) try samples.append(a, Runtime.Str.of(try a.dupe(u8, x.string)));
    };

    var renderers: [5]bool = .{ false, false, false, false, false };
    if (o.get("renderers")) |rv| if (rv == .array) {
        for (rv.array.items) |x| if (x == .string) {
            inline for ([_][]const u8{ "value", "entry", "item", "tail", "key" }, 0..) |n, i| {
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
        .free_table = freeTableThunk,
        .free_bytes = freeBytesThunk,
        .render_value = if (renderers[0]) renderValueThunk else null,
        .render_entry = if (renderers[1]) renderEntryThunk else null,
        .render_item = if (renderers[2]) renderItemThunk else null,
        .render_tail = if (renderers[3]) renderTailThunk else null,
        .render_key = if (renderers[4]) renderKeyThunk else null,
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

/// What a parse's memory is: the arena its rows and strings were decoded
/// into. `NodeTable.owner`.
const TableOwner = struct { arena: std.heap.ArenaAllocator };

fn parseThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, input: Runtime.Str, out: *Runtime.NodeTable, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    const t = transportOf(ctx);
    const src = input.slice() orelse "";
    if (!std.unicode.utf8ValidateSlice(src)) {
        setErr(err, "the input is not UTF-8, which the helper wire carries as text", 0);
        return 2;
    }
    var req: Io.Writer.Allocating = .init(t.allocator);
    defer req.deinit();
    const w = &req.writer;
    w.writeAll("{\"op\":\"parse\",\"dialect\":") catch return 3;
    jsonString(w, std.mem.span(dialect)) catch return 3;
    w.writeAll(",\"input\":") catch return 3;
    jsonString(w, src) catch return 3;
    w.writeAll("}") catch return 3;

    const owner = t.allocator.create(TableOwner) catch return 3;
    owner.* = .{ .arena = std.heap.ArenaAllocator.init(t.allocator) };
    const a = owner.arena.allocator();
    const resp = t.call(a, req.written()) catch |e| {
        owner.arena.deinit();
        t.allocator.destroy(owner);
        setErr(err, @errorName(e), 0);
        return 2;
    };
    const table_v = okField(resp, "table") orelse {
        setErr(err, failureMessage(resp), failureOffset(resp));
        owner.arena.deinit();
        t.allocator.destroy(owner);
        return 2;
    };
    const table = tableFromJson(a, table_v) catch |e| {
        setErr(err, switch (e) {
            error.MalformedTable => "the helper returned a malformed table",
            else => @errorName(e),
        }, 0);
        owner.arena.deinit();
        t.allocator.destroy(owner);
        return 2;
    };
    out.* = table;
    out.owner = owner;
    return 0;
}

fn freeTableThunk(ctx: ?*anyopaque, table: *Runtime.NodeTable) callconv(.c) void {
    const t = transportOf(ctx);
    const owner: *TableOwner = @ptrCast(@alignCast(table.owner orelse return));
    owner.arena.deinit();
    t.allocator.destroy(owner);
}

fn freeBytesThunk(ctx: ?*anyopaque, bytes: Runtime.Str) callconv(.c) void {
    const t = transportOf(ctx);
    if (bytes.slice()) |s| if (s.len > 0) t.allocator.free(s);
}

fn printThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, table: *const Runtime.NodeTable, options: *const Runtime.PrintOptions, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    const t = transportOf(ctx);
    var req: Io.Writer.Allocating = .init(t.allocator);
    defer req.deinit();
    const w = &req.writer;
    w.writeAll("{\"op\":\"print\",\"dialect\":") catch return 3;
    jsonString(w, std.mem.span(dialect)) catch return 3;
    w.writeAll(",\"table\":") catch return 3;
    tableToJson(w, table) catch return 3;
    w.print(",\"options\":{{\"pretty\":{},\"strip_comments\":{},\"indent\":{d},\"width\":{d},\"splice\":{}}}}}", .{ options.pretty, options.strip_comments, options.indent, options.width, options.splice }) catch return 3;
    return outputCall(t, req.written(), out, err);
}

/// Send `request` and hand back its `output` string.
fn outputCall(t: *Transport, request: []const u8, out: *Runtime.Str, err: *Runtime.ErrorInfo) c_int {
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const resp = t.call(arena.allocator(), request) catch |e| {
        setErr(err, @errorName(e), 0);
        return 4;
    };
    const output = okField(resp, "output") orelse {
        setErr(err, failureMessage(resp), 0);
        return 4;
    };
    if (output != .string) {
        setErr(err, "the helper's output is not a string", 0);
        return 4;
    }
    const copy = t.allocator.dupe(u8, output.string) catch return 3;
    out.* = Runtime.Str.of(copy);
    return 0;
}

fn renderCall(ctx: ?*anyopaque, which: []const u8, dialect: [*:0]const u8, indent: Runtime.Str, key: Runtime.Str, value: Runtime.Str, literal: ?[*:0]const u8, old_key: Runtime.Str, out: *Runtime.Str, err: *Runtime.ErrorInfo) c_int {
    const t = transportOf(ctx);
    var req: Io.Writer.Allocating = .init(t.allocator);
    defer req.deinit();
    const w = &req.writer;
    w.print("{{\"op\":\"render\",\"which\":\"{s}\",\"dialect\":", .{which}) catch return 3;
    jsonString(w, std.mem.span(dialect)) catch return 3;
    inline for (.{ .{ "indent", indent }, .{ "key", key }, .{ "value", value } }) |pair| {
        w.print(",\"{s}\":", .{pair[0]}) catch return 3;
        jsonString(w, pair[1].slice() orelse "") catch return 3;
    }
    // What fig made of the value, for the value renderer alone.
    if (literal) |l| {
        w.writeAll(",\"literal\":") catch return 3;
        jsonString(w, std.mem.span(l)) catch return 3;
    }
    w.writeAll(",\"old_key\":") catch return 3;
    jsonString(w, old_key.slice() orelse "") catch return 3;
    w.writeAll("}") catch return 3;
    return outputCall(t, req.written(), out, err);
}

fn renderValueThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, value: Runtime.Str, literal: [*:0]const u8, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "value", dialect, .{}, .{}, value, literal, .{}, out, err);
}
fn renderEntryThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Runtime.Str, key: Runtime.Str, value: Runtime.Str, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "entry", dialect, indent, key, value, null, .{}, out, err);
}
fn renderItemThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Runtime.Str, value: Runtime.Str, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "item", dialect, indent, .{}, value, null, .{}, out, err);
}
fn renderTailThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Runtime.Str, key: Runtime.Str, value: Runtime.Str, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "tail", dialect, indent, key, value, null, .{}, out, err);
}
fn renderKeyThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Runtime.Str, key: Runtime.Str, old_key: Runtime.Str, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "key", dialect, indent, key, .{}, null, old_key, out, err);
}

// ── the table on the wire ──────────────────────────────────────────────────

const kind_names = [_][]const u8{ "null", "bool", "int", "float", "string", "sequence", "mapping", "keyvalue", "alias" };

fn kindOf(name: []const u8) ?c_int {
    for (kind_names, 0..) |k, i| if (std.mem.eql(u8, k, name)) return @intCast(i);
    return null;
}

const ExtKind = AST.Node.Kind.Extended.ExtKind;

fn extKindOf(name: []const u8) ?c_int {
    inline for (@typeInfo(ExtKind).@"enum".fields) |f| {
        if (std.mem.eql(u8, f.name, name)) return @intCast(f.value);
    }
    return null;
}

fn spanOf(v: ?std.json.Value) !Runtime.CSpan {
    const s = v orelse return .none;
    if (s == .null) return .none;
    if (s != .array or s.array.items.len != 2) return error.MalformedTable;
    const a = s.array.items[0];
    const b = s.array.items[1];
    if (a != .integer or b != .integer or a.integer < 0 or b.integer < 0) return error.MalformedTable;
    return .{ .start = @intCast(a.integer), .end = @intCast(b.integer) };
}

fn strOf(a: Allocator, v: ?std.json.Value) !Runtime.Str {
    const s = v orelse return .none;
    return switch (s) {
        .null => .none,
        .string => |x| Runtime.Str.of(try a.dupe(u8, x)),
        else => error.MalformedTable,
    };
}

fn u32Of(v: ?std.json.Value) !u32 {
    const s = v orelse return error.MalformedTable;
    if (s != .integer or s.integer < 0 or s.integer > std.math.maxInt(u32)) return error.MalformedTable;
    return @intCast(s.integer);
}

fn usizeOf(v: ?std.json.Value) !usize {
    const s = v orelse return error.MalformedTable;
    if (s != .integer or s.integer < 0) return error.MalformedTable;
    return @intCast(s.integer);
}

/// A table from the JSON a helper answers `parse` with, its rows and
/// strings in `a`. `error.MalformedTable` for a shape the wire does not
/// document.
pub fn tableFromJson(a: Allocator, v: std.json.Value) !Runtime.NodeTable {
    if (v != .object) return error.MalformedTable;
    const o = v.object;
    var rows: std.ArrayList(Runtime.NodeRow) = .empty;
    if (o.get("rows")) |rv| {
        if (rv != .array) return error.MalformedTable;
        for (rv.array.items) |r| {
            if (r != .object) return error.MalformedTable;
            const ro = r.object;
            const kind_v = ro.get("kind") orelse return error.MalformedTable;
            if (kind_v != .string) return error.MalformedTable;
            const kind = kindOf(kind_v.string) orelse return error.MalformedTable;
            const ext: c_int = if (ro.get("ext_kind")) |e| (if (e == .string) (extKindOf(e.string) orelse return error.MalformedTable) else Runtime.no_ext_kind) else Runtime.no_ext_kind;
            const parent: u32 = if (ro.get("parent")) |p| (if (p == .null) Runtime.no_node else try u32Of(p)) else Runtime.no_node;
            try rows.append(a, .{
                .kind = kind,
                .ext_kind = ext,
                .parent = parent,
                .span = try spanOf(ro.get("span")),
                .text = try strOf(a, ro.get("text")),
                .anchor = try strOf(a, ro.get("anchor")),
                .anchor_span = try spanOf(ro.get("anchor_span")),
                .tag = try strOf(a, ro.get("tag")),
                .tag_span = try spanOf(ro.get("tag_span")),
                .marker = try spanOf(ro.get("marker")),
                .sep = try spanOf(ro.get("sep")),
            });
        }
    }
    var regions: std.ArrayList(Runtime.RegionRow) = .empty;
    if (o.get("regions")) |rv| if (rv == .array) {
        for (rv.array.items) |r| {
            if (r != .object) return error.MalformedTable;
            try regions.append(a, .{ .node = try u32Of(r.object.get("node")), .start = try usizeOf(r.object.get("start")), .end = try usizeOf(r.object.get("end")) });
        }
    };
    var mentions: std.ArrayList(Runtime.MentionRow) = .empty;
    if (o.get("mentions")) |mv| if (mv == .array) {
        for (mv.array.items) |m| {
            if (m != .object) return error.MalformedTable;
            const k = m.object.get("kind") orelse return error.MalformedTable;
            try mentions.append(a, .{
                .node = try u32Of(m.object.get("node")),
                .span = try spanOf(m.object.get("span")),
                .kind = if (k == .string and std.mem.eql(u8, k.string, "entry")) Runtime.mention_entry else Runtime.mention_header,
            });
        }
    };
    var comments: std.ArrayList(Runtime.CommentRow) = .empty;
    if (o.get("comments")) |cv| if (cv == .array) {
        for (cv.array.items) |c| {
            if (c != .object) return error.MalformedTable;
            const slot_v = c.object.get("slot") orelse return error.MalformedTable;
            const style_v = c.object.get("style") orelse return error.MalformedTable;
            if (slot_v != .string or style_v != .string) return error.MalformedTable;
            const slot: c_int = if (std.mem.eql(u8, slot_v.string, "trailing")) Runtime.comment_trailing else if (std.mem.eql(u8, slot_v.string, "dangling")) Runtime.comment_dangling else Runtime.comment_leading;
            const style: c_int = if (std.mem.eql(u8, style_v.string, "block")) 1 else 0;
            const text = try strOf(a, c.object.get("text"));
            try comments.append(a, .{ .node = try u32Of(c.object.get("node")), .slot = slot, .style = style, .text = if (text.slice() == null) Runtime.Str.of("") else text });
        }
    };
    var directives: std.ArrayList(Runtime.DirectiveRow) = .empty;
    if (o.get("directives")) |dv| if (dv == .array) {
        for (dv.array.items) |d| {
            if (d != .object) return error.MalformedTable;
            const handle = try strOf(a, d.object.get("handle"));
            const prefix = try strOf(a, d.object.get("prefix"));
            if (handle.slice() == null or prefix.slice() == null) return error.MalformedTable;
            try directives.append(a, .{ .handle = handle, .prefix = prefix });
        }
    };
    const row_slice = try rows.toOwnedSlice(a);
    const region_slice = try regions.toOwnedSlice(a);
    const mention_slice = try mentions.toOwnedSlice(a);
    const comment_slice = try comments.toOwnedSlice(a);
    const directive_slice = try directives.toOwnedSlice(a);
    return .{
        .rows = row_slice.ptr,
        .row_count = row_slice.len,
        .regions = region_slice.ptr,
        .region_count = region_slice.len,
        .mentions = mention_slice.ptr,
        .mention_count = mention_slice.len,
        .comments = comment_slice.ptr,
        .comment_count = comment_slice.len,
        .directives = directive_slice.ptr,
        .directive_count = directive_slice.len,
    };
}

fn writeSpan(w: *Io.Writer, key: []const u8, s: Runtime.CSpan) !void {
    if (s.span()) |sp| try w.print(",\"{s}\":[{d},{d}]", .{ key, sp.start, sp.end });
}

fn writeStr(w: *Io.Writer, key: []const u8, s: Runtime.Str) !void {
    if (s.slice()) |x| {
        try w.print(",\"{s}\":", .{key});
        try jsonString(w, x);
    }
}

/// A table as the JSON a helper answers `parse` with and is handed for
/// `print`: an absent optional is omitted, not `null`.
pub fn tableToJson(w: *Io.Writer, t: *const Runtime.NodeTable) !void {
    try w.writeAll("{\"rows\":[");
    for (t.rowSlice(), 0..) |r, i| {
        if (i > 0) try w.writeByte(',');
        const kind: usize = @intCast(r.kind);
        try w.print("{{\"kind\":\"{s}\"", .{if (kind < kind_names.len) kind_names[kind] else "null"});
        if (r.ext_kind != Runtime.no_ext_kind) {
            const ek: ExtKind = @enumFromInt(r.ext_kind);
            try w.print(",\"ext_kind\":\"{s}\"", .{@tagName(ek)});
        }
        if (r.parent == Runtime.no_node) try w.writeAll(",\"parent\":null") else try w.print(",\"parent\":{d}", .{r.parent});
        try writeSpan(w, "span", r.span);
        try writeStr(w, "text", r.text);
        try writeStr(w, "anchor", r.anchor);
        try writeSpan(w, "anchor_span", r.anchor_span);
        try writeStr(w, "tag", r.tag);
        try writeSpan(w, "tag_span", r.tag_span);
        try writeSpan(w, "marker", r.marker);
        try writeSpan(w, "sep", r.sep);
        try w.writeByte('}');
    }
    try w.writeAll("],\"regions\":[");
    for (t.regionSlice(), 0..) |r, i| {
        if (i > 0) try w.writeByte(',');
        try w.print("{{\"node\":{d},\"start\":{d},\"end\":{d}}}", .{ r.node, r.start, r.end });
    }
    try w.writeAll("],\"mentions\":[");
    for (t.mentionSlice(), 0..) |m, i| {
        if (i > 0) try w.writeByte(',');
        const sp = m.span.span() orelse Span.init(0, 0);
        try w.print("{{\"node\":{d},\"span\":[{d},{d}],\"kind\":\"{s}\"}}", .{ m.node, sp.start, sp.end, if (m.kind == Runtime.mention_entry) "entry" else "header" });
    }
    try w.writeAll("],\"comments\":[");
    for (t.commentSlice(), 0..) |c, i| {
        if (i > 0) try w.writeByte(',');
        const slot: []const u8 = switch (c.slot) {
            Runtime.comment_trailing => "trailing",
            Runtime.comment_dangling => "dangling",
            else => "leading",
        };
        try w.print("{{\"node\":{d},\"slot\":\"{s}\",\"style\":\"{s}\",\"text\":", .{ c.node, slot, if (c.style == 1) "block" else "line" });
        try jsonString(w, c.text.slice() orelse "");
        try w.writeByte('}');
    }
    try w.writeAll("]");
    // Absent, not empty, for the formats that have none — which is every
    // helper written before the column existed.
    if (t.directive_count > 0) {
        try w.writeAll(",\"directives\":[");
        for (t.directiveSlice(), 0..) |d, i| {
            if (i > 0) try w.writeByte(',');
            try w.writeAll("{\"handle\":");
            try jsonString(w, d.handle.slice() orelse "");
            try w.writeAll(",\"prefix\":");
            try jsonString(w, d.prefix.slice() orelse "");
            try w.writeByte('}');
        }
        try w.writeByte(']');
    }
    try w.writeAll("}");
}

// ── tests ──────────────────────────────────────────────────────────────────

/// A transport that answers from a fixed script — what a helper would say
/// — so the vtable's half of the wire is tested with no process behind it.
const Scripted = struct {
    transport: Transport,
    describe_response: []const u8,
    parse_response: []const u8,
    print_response: []const u8,
    last_request: std.ArrayList(u8) = .empty,

    fn call(t: *Transport, out_arena: Allocator, request: []const u8) anyerror!std.json.Value {
        const self: *Scripted = @fieldParentPtr("transport", t);
        self.last_request.clearRetainingCapacity();
        try self.last_request.appendSlice(t.allocator, request);
        const line = if (std.mem.indexOf(u8, request, "\"op\":\"describe\"") != null)
            self.describe_response
        else if (std.mem.indexOf(u8, request, "\"op\":\"parse\"") != null)
            self.parse_response
        else
            self.print_response;
        return parseLine(out_arena, line);
    }
};

test "parseLine: the wire's JSON, integers as integers, no floats needed, nothing borrowed from the line" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    // The line is a transport's buffer, freed once the call returns: the
    // tree must own every string it holds.
    const line = try t.allocator.dupe(u8, "{\"ok\":true,\"n\":-3,\"f\":1.5,\"s\":\"a\\nb\",\"a\":[1,{}],\"z\":null}");
    const v = try parseLine(arena.allocator(), line);
    @memset(line, 'x');
    t.allocator.free(line);
    try t.expect(v.object.get("ok").?.bool);
    try t.expectEqual(@as(i64, -3), v.object.get("n").?.integer);
    try t.expectEqualStrings("1.5", v.object.get("f").?.number_string);
    try t.expectEqualStrings("a\nb", v.object.get("s").?.string);
    try t.expectEqual(@as(usize, 2), v.object.get("a").?.array.items.len);
    try t.expectEqual(@as(usize, 0), v.object.get("a").?.array.items[1].object.count());
    try t.expect(v.object.get("z").? == .null);
    try t.expectError(error.HelperSpokeNoJson, parseLine(arena.allocator(), "{\"ok\":true} trailing"));
    try t.expectError(error.HelperSpokeNoJson, parseLine(arena.allocator(), "not json"));
    try t.expectError(error.HelperSpokeNoJson, parseLine(arena.allocator(), "{\"a\":[1,2"));
}

test "describe builds a vtable the registry would accept" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    var s: Scripted = .{
        .transport = .{ .allocator = t.allocator, .callFn = Scripted.call },
        .describe_response =
        \\{"ok":true,"description":{"name":"wire-kv","caps":{"read":true,"edit":true,"serialize":true},
        \\"max_mapping_depth":0,"syntax":{"comments":{"style":"hash","line":{"open":"#"},"trailing":{"open":"#"}},
        \\"kv_sep":"=","empty_map_literal":"{}","flow_containers":false},
        \\"dialects":[{"name":"wire-kv","extensions":["wkv"],"splice":"raw","empty_doc_seed":""}],
        \\"samples":["a=1\n"],"renderers":["value"]}}
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

test "a refusal to describe is reported through Runtime.refuse" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    var s: Scripted = .{
        .transport = .{ .allocator = t.allocator, .callFn = Scripted.call },
        .describe_response = "{\"ok\":false,\"message\":\"no script given\"}",
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
        .transport = .{ .allocator = t.allocator, .callFn = Scripted.call },
        .describe_response = "",
        .parse_response =
        \\{"ok":true,"table":{"rows":[{"kind":"mapping","parent":null,"span":[0,4]},
        \\{"kind":"keyvalue","parent":0,"span":[0,3],"sep":[1,2]},{"kind":"string","parent":1,"span":[0,1],"text":"a"},
        \\{"kind":"string","parent":1,"span":[2,3],"text":"1"}],"regions":[],"mentions":[],
        \\"comments":[{"node":2,"slot":"leading","style":"line","text":"hi"}],
        \\"directives":[{"handle":"!e!","prefix":"tag:x/"}]}}
        ,
        .print_response = "{\"ok\":true,\"output\":\"a=1\\n\"}",
    };
    defer s.last_request.deinit(t.allocator);
    var table: Runtime.NodeTable = undefined;
    var err: Runtime.ErrorInfo = .empty;
    try t.expectEqual(@as(c_int, 0), parseThunk(&s.transport, "wire-kv", Runtime.Str.of("a=1\n"), &table, &err));
    try t.expectEqualStrings("{\"op\":\"parse\",\"dialect\":\"wire-kv\",\"input\":\"a=1\\n\"}", s.last_request.items);
    try t.expectEqual(@as(usize, 4), table.row_count);
    try t.expectEqual(@as(usize, 1), table.comment_count);
    try t.expectEqual(@as(usize, 1), table.directive_count);
    try t.expectEqualStrings("tag:x/", table.directiveSlice()[0].prefix.slice().?);
    try t.expectEqualStrings("a", table.rowSlice()[2].text.slice().?);
    try t.expectEqual(@as(usize, 1), table.rowSlice()[1].sep.span().?.start);
    try t.expect(table.rowSlice()[0].sep.span() == null);

    var out: Runtime.Str = .none;
    const options: Runtime.PrintOptions = .{ .pretty = true, .strip_comments = false, .indent = 2, .width = 80 };
    try t.expectEqual(@as(c_int, 0), printThunk(&s.transport, "wire-kv", &table, &options, &out, &err));
    try t.expectEqualStrings("a=1\n", out.slice().?);
    // The print request carries the table as `parse` answered it.
    try t.expect(std.mem.indexOf(u8, s.last_request.items, "\"sep\":[1,2]") != null);
    try t.expect(std.mem.indexOf(u8, s.last_request.items, "\"slot\":\"leading\"") != null);
    try t.expect(std.mem.indexOf(u8, s.last_request.items, "\"directives\":[{\"handle\":\"!e!\",\"prefix\":\"tag:x/\"}]") != null);
    try t.expect(std.mem.indexOf(u8, s.last_request.items, "\"options\":{\"pretty\":true,\"strip_comments\":false,\"indent\":2,\"width\":80,\"splice\":false}") != null);
    freeBytesThunk(&s.transport, out);
    freeTableThunk(&s.transport, &table);
}

test "a parse refusal carries the helper's message and offset" {
    const t = std.testing;
    var s: Scripted = .{
        .transport = .{ .allocator = t.allocator, .callFn = Scripted.call },
        .describe_response = "",
        .parse_response = "{\"ok\":false,\"message\":\"expected `=` here\",\"byte_offset\":3}",
        .print_response = "",
    };
    defer s.last_request.deinit(t.allocator);
    var table: Runtime.NodeTable = undefined;
    var err: Runtime.ErrorInfo = .empty;
    try t.expectEqual(@as(c_int, 2), parseThunk(&s.transport, "wire-kv", Runtime.Str.of("abc"), &table, &err));
    try t.expectEqualStrings("expected `=` here", err.text());
    try t.expectEqual(@as(usize, 3), err.byte_offset);
}
