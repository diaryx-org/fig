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
