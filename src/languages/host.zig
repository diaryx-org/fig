//! A runtime language whose functions live in the wasm module's host: the
//! vtable is `fig.Wire`'s — the helper wire, newline-delimited JSON — over a
//! transport whose one call is a wasm import, `fig_host.call`, answered by
//! the same `handle` a helper process runs over stdin and stdout.
//!
//! Two modules register this way. The reactor the TypeScript binding loads
//! registers a language when the binding asks (`wasm_host.zig`'s
//! `fig_host_language_register`); the CLI built for WASI registers one when
//! it meets a format it did not compile in and its host has a language by
//! that name (`cli/host_languages.zig`). Each declares the import itself —
//! an `extern` is a symbol of the module that references it — and hands it
//! here.

const std = @import("std");
const Runtime = @import("runtime.zig");
const Wire = @import("wire.zig");

/// The shape of `fig_host.call`: one request line in, one response line
/// out. `lang` is the host's own id for the language; the response is bytes
/// the host allocated through the module's `fig_alloc`, written to
/// `*out_ptr`/`*out_len`, which this side reads and frees with
/// `std.heap.wasm_allocator`. A nonzero return is a host that could not
/// answer at all — a refusal is an ordinary `{"ok":false,…}` response.
pub const Call = *const fn (lang: u32, request: [*]const u8, request_len: usize, out_ptr: *?[*]u8, out_len: *usize) callconv(.c) c_int;

/// What `fig_alloc` allocates with, and a response is freed with.
pub const allocator = std.heap.wasm_allocator;

/// The transport for one registered language. Lives for the process, as
/// every vtable's `ctx` must.
const HostLanguage = struct {
    transport: Wire.Transport,
    call: Call,
    lang: u32,
    /// What the vtable's description pointers reach.
    arena: std.heap.ArenaAllocator,

    fn callHost(t: *Wire.Transport, out_arena: std.mem.Allocator, request: []const u8) anyerror!std.json.Value {
        const self: *HostLanguage = @fieldParentPtr("transport", t);
        var ptr: ?[*]u8 = null;
        var len: usize = 0;
        if (self.call(self.lang, request.ptr, request.len, &ptr, &len) != 0) return error.HelperExited;
        const line = (ptr orelse return error.HelperExited)[0..len];
        defer t.allocator.free(line);
        return Wire.parseLine(out_arena, line);
    }
};

pub const RegisterError = error{
    OutOfMemory,
    /// The host's description was refused, or the harness failed over its
    /// samples, or its name is taken: `Runtime.lastRefusal()` says which.
    Refused,
    /// The description is not the shape the wire documents.
    Malformed,
    RegistryFull,
};

/// Register the language the host knows as `lang`: ask it to `describe`
/// itself through `call`, build the vtable, and register that through the
/// same `Runtime.register` a C host's vtable goes through — which validates
/// the description and runs the harness over its samples, calling back into
/// the host for each parse. Returns the format integer of the first
/// dialect row.
pub fn register(call: Call, lang: u32) RegisterError!c_int {
    const host = try allocator.create(HostLanguage);
    host.* = .{
        .transport = .{ .allocator = allocator, .callFn = HostLanguage.callHost },
        .call = call,
        .lang = lang,
        .arena = std.heap.ArenaAllocator.init(allocator),
    };
    errdefer {
        host.arena.deinit();
        allocator.destroy(host);
    }
    const vt = Wire.describe(&host.transport, host.arena.allocator()) catch |err| return switch (err) {
        error.OutOfMemory => error.OutOfMemory,
        error.HelperRefused => error.Refused,
        error.HelperSpokeNoJson => error.Malformed,
    };
    return Runtime.register(allocator, &vt) catch |err| switch (err) {
        error.OutOfMemory => error.OutOfMemory,
        error.InvalidLanguage, error.HarnessFailed, error.NameTaken => error.Refused,
        error.AllocatorMismatch, error.RegistryFull => error.RegistryFull,
    };
}
