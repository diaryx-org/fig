//! The wasm module's half of the runtime-language carrier: a language
//! whose `parse`, `print` and renderers are JavaScript, registered from the
//! TypeScript binding. `docs/proposals/runtime-languages.md` §7.3.
//!
//! The binding cannot hand the module a C function pointer, so the vtable
//! is `fig.Wire`'s — the helper wire, newline-delimited JSON — over a
//! transport whose one call is a wasm import: `fig_host.call` takes a
//! request line and answers a response line, and the JavaScript side of it
//! is the same `handle` a helper process runs over stdin and stdout. A
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
const Wire = @import("languages/wire.zig");

/// One request line in, one response line out, answered by the host.
/// `lang` is the binding's own id for the language, handed back to it so
/// it finds the object; the response is bytes the host allocated with
/// `fig_alloc`, written to `*out_ptr`/`*out_len`, which this side reads
/// and frees. A nonzero return is a host that could not answer at all —
/// a refusal is an ordinary `{"ok":false,…}` response.
extern "fig_host" fn call(lang: u32, request: [*]const u8, request_len: usize, out_ptr: *?[*]u8, out_len: *usize) callconv(.c) c_int;

/// The transport for one registered language. Lives for the process, as
/// every vtable's `ctx` must.
const HostLanguage = struct {
    transport: Wire.Transport,
    lang: u32,
    /// What the vtable's description pointers reach.
    arena: std.heap.ArenaAllocator,

    fn callHost(t: *Wire.Transport, out_arena: std.mem.Allocator, request: []const u8) anyerror!std.json.Value {
        const self: *HostLanguage = @fieldParentPtr("transport", t);
        var ptr: ?[*]u8 = null;
        var len: usize = 0;
        if (call(self.lang, request.ptr, request.len, &ptr, &len) != 0) return error.HelperExited;
        const line = (ptr orelse return error.HelperExited)[0..len];
        defer t.allocator.free(line);
        return Wire.parseLine(out_arena, line);
    }
};

/// Register the language the host knows as `lang`: ask it to `describe`
/// itself through `fig_host.call`, build the vtable, and register that
/// through the same `Runtime.register` a C host's vtable goes through —
/// which validates the description and runs the harness over its samples,
/// calling back into the host for each parse. On `.ok`, `*out_format` is
/// the format integer of the first dialect row, as `fig_language_register`
/// would return it; a refusal is `invalid_argument` with the reason in
/// `out_err`.
pub export fn fig_host_language_register(lang: u32, out_format: ?*c_int, out_err: ?*c_api.FigError) c_api.FigStatus {
    const out = out_format orelse return c_api.fillError(out_err, .invalid_argument, "out_format is null");
    out.* = -1;
    const allocator = std.heap.wasm_allocator;
    const host = allocator.create(HostLanguage) catch return c_api.fillError(out_err, .out_of_memory, "out of memory");
    host.* = .{
        .transport = .{ .allocator = allocator, .callFn = HostLanguage.callHost },
        .lang = lang,
        .arena = std.heap.ArenaAllocator.init(allocator),
    };
    const vt = Wire.describe(&host.transport, host.arena.allocator()) catch |err| {
        host.arena.deinit();
        allocator.destroy(host);
        return switch (err) {
            error.OutOfMemory => c_api.fillError(out_err, .out_of_memory, "out of memory"),
            error.HelperRefused => c_api.fillError(out_err, .invalid_argument, Runtime.lastRefusal()),
            error.HelperSpokeNoJson => c_api.fillError(out_err, .invalid_argument, "the language's description is not the shape the wire documents"),
        };
    };
    const abi = Runtime.register(allocator, &vt) catch |err| {
        host.arena.deinit();
        allocator.destroy(host);
        return switch (err) {
            error.OutOfMemory => c_api.fillError(out_err, .out_of_memory, "out of memory"),
            error.InvalidLanguage, error.HarnessFailed, error.NameTaken => c_api.fillError(out_err, .invalid_argument, Runtime.lastRefusal()),
            error.AllocatorMismatch, error.RegistryFull => c_api.fillError(out_err, .unsupported_operation, "the language registry cannot take another registration"),
        };
    };
    out.* = abi;
    return .ok;
}
