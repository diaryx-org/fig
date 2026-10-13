//! The wasm module's half of the runtime-language carrier: a language
//! whose `parse`, `print` and renderers are JavaScript, registered from the
//! TypeScript binding. `docs/proposals/runtime-languages.md` §7.3.
//!
//! The binding cannot hand the module a C function pointer, so the vtable
//! is `fig.Wire`'s over a wasm import, `fig_host.call` — the transport
//! `languages/host.zig` builds, which the CLI's WASI build shares. A
//! language written in JavaScript is therefore a helper in every respect
//! but the pipe, and `fig-lua`'s scripts, the CLI's helpers and the
//! binding's objects all speak one wire.
//!
//! Only the wasm module exports what is here. It is not part of the C ABI
//! `fig.h` declares — `c_api.zig` pulls this file in under a wasm-only
//! `comptime` guard, and `abi-check` reads `c_api.zig` alone — because a
//! C host registers through `fig_language_register` with a vtable of its
//! own, and the import below has no meaning outside a wasm instance.

const std = @import("std");
const c_api = @import("c_api.zig");
const Runtime = @import("languages/runtime.zig");
const Host = @import("languages/host.zig");

/// One request line in, one response line out, answered by the host — the
/// shape `Host.Call` documents.
extern "fig_host" fn call(lang: u32, request: [*]const u8, request_len: usize, out_ptr: *?[*]u8, out_len: *usize) callconv(.c) c_int;

/// Register the language the host knows as `lang` (`Host.register`). On
/// `.ok`, `*out_format` is the format integer of the first dialect row, as
/// `fig_language_register` would return it; a refusal is
/// `invalid_argument` with the reason in `out_err`.
pub export fn fig_host_language_register(lang: u32, out_format: ?*c_int, out_err: ?*c_api.FigError) c_api.FigStatus {
    const out = out_format orelse return c_api.fillError(out_err, .invalid_argument, "out_format is null");
    out.* = -1;
    out.* = Host.register(&call, lang) catch |err| return switch (err) {
        error.OutOfMemory => c_api.fillError(out_err, .out_of_memory, "out of memory"),
        error.Refused => c_api.fillError(out_err, .invalid_argument, Runtime.lastRefusal()),
        error.Malformed => c_api.fillError(out_err, .invalid_argument, "the language's description is not the shape the wire documents"),
        error.RegistryFull => c_api.fillError(out_err, .unsupported_operation, "the language registry cannot take another registration"),
    };
    return .ok;
}

/// The host's half of a language in a WebAssembly module of its own
/// (`kit/wasm_language.zig`): parse there and write the table into `out`,
/// print there what `view` shows. `Host.Parse`, `Host.Print`.
extern "fig_host" fn parse(lang: u32, dialect: [*]const u8, dialect_len: usize, input: ?[*]const u8, input_len: usize, out: *Runtime.Table, err: *Runtime.ErrorInfo) callconv(.c) c_int;
extern "fig_host" fn print(lang: u32, dialect: [*]const u8, dialect_len: usize, view: *const Runtime.table.View, options: *const Runtime.PrintOptions, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int;

/// Register the language the host knows as `lang`, a Zig language in a
/// WebAssembly module of its own: described and rendered over `call`, as
/// any host language is, and parsed and printed through `parse` and
/// `print`. Otherwise as `fig_host_language_register`.
pub export fn fig_host_native_register(lang: u32, out_format: ?*c_int, out_err: ?*c_api.FigError) c_api.FigStatus {
    const out = out_format orelse return c_api.fillError(out_err, .invalid_argument, "out_format is null");
    out.* = -1;
    out.* = Host.registerNative(&call, &parse, &print, lang) catch |err| return switch (err) {
        error.OutOfMemory => c_api.fillError(out_err, .out_of_memory, "out of memory"),
        error.Refused => c_api.fillError(out_err, .invalid_argument, Runtime.lastRefusal()),
        error.Malformed => c_api.fillError(out_err, .invalid_argument, "the language's description is not the shape the wire documents"),
        error.RegistryFull => c_api.fillError(out_err, .unsupported_operation, "the language registry cannot take another registration"),
    };
    return .ok;
}

/// Append `n` zeroed elements to list `which` of `t` and return them, for
/// the host to copy a language module's table into: 0 rows, 1 the string
/// buffer (bytes), 2 anchors, 3 tags, 4 markers, 5 separators, 6 item
/// separators, 7 comments, 8 regions, 9 mentions, 10 directives, 11
/// diagnostics. Null when `which` is not a list or there is no memory.
pub export fn fig_host_table_extend(t: ?*Runtime.Table, which: u32, n: usize) ?*anyopaque {
    const tbl = t orelse return null;
    const a = tbl.allocator;
    return switch (which) {
        0 => extend(Runtime.table.Row, &tbl.rows, a, n),
        1 => extend(u8, &tbl.strings, a, n),
        2 => extend(Runtime.table.Anchor, &tbl.anchors, a, n),
        3 => extend(Runtime.table.Tag, &tbl.tags, a, n),
        4 => extend(Runtime.table.SpanFact, &tbl.markers, a, n),
        5 => extend(Runtime.table.SpanFact, &tbl.seps, a, n),
        6 => extend(Runtime.table.ItemSep, &tbl.item_seps, a, n),
        7 => extend(Runtime.table.Comment, &tbl.comments, a, n),
        8 => extend(Runtime.table.SpanFact, &tbl.regions, a, n),
        9 => extend(Runtime.table.Mention, &tbl.mentions, a, n),
        10 => extend(Runtime.table.Directive, &tbl.directives, a, n),
        11 => extend(Runtime.table.Diagnostic, &tbl.diagnostics, a, n),
        else => null,
    };
}

fn extend(comptime T: type, list: *std.ArrayList(T), a: std.mem.Allocator, n: usize) ?*anyopaque {
    const out = list.addManyAsSlice(a, n) catch return null;
    @memset(std.mem.sliceAsBytes(out), 0);
    return if (n == 0) @ptrCast(list.items.ptr) else @ptrCast(out.ptr);
}
