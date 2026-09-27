//! Languages the CLI's host serves, in the WASI build: `@diaryx/fig-wasi`'s
//! `bin/fig.mjs` runs the CLI under Node and holds `@diaryx/fig`'s
//! JavaScript languages, and a format this build did not compile in is
//! asked of it before `languages.figl` is — a WASI module cannot spawn the
//! helper a `languages.figl` names.
//!
//! The host answers three imports: `resolve` and `resolve_extension` name
//! the language serving a dialect name or a file extension (0 for none),
//! and `call` carries the wire to it (`fig.Host`). A response is written
//! into this module's memory through the `fig_alloc` export below. A host
//! that does not provide the imports cannot instantiate this module, so
//! only a `-Dwasi-host=true` build asks: `@diaryx/fig-wasi`'s. `zig build
//! wasi` alone is the plain module any WASI runtime runs.

const std = @import("std");
const builtin = @import("builtin");
const fig = @import("fig");
const build_options = @import("build_options");

/// Whether this build asks a host at all: a WASI build made for one
/// (`-Dwasi-host=true`), since no other WASI runtime provides the imports.
pub const enabled = builtin.os.tag == .wasi and build_options.wasi_host;

extern "fig_host" fn call(lang: u32, request: [*]const u8, request_len: usize, out_ptr: *?[*]u8, out_len: *usize) callconv(.c) c_int;
extern "fig_host" fn resolve(name: [*]const u8, name_len: usize) u32;
extern "fig_host" fn resolve_extension(ext: [*]const u8, ext_len: usize) u32;

comptime {
    if (enabled) @export(&figAlloc, .{ .name = "fig_alloc" });
}

/// Where the host writes a response: `len` bytes, freed by `fig.Host`
/// once the response is read.
fn figAlloc(len: usize) callconv(.c) ?[*]u8 {
    if (len == 0) return null;
    const mem = fig.Host.allocator.alloc(u8, len) catch return null;
    return mem.ptr;
}

/// The language the host serves for the dialect `name`, registered on this
/// first ask; null when the host has none, or refused to register it.
pub fn byName(name: []const u8) ?*const fig.Runtime.Entry {
    if (comptime !enabled) return null;
    if (fig.Runtime.entryByName(name)) |e| return e;
    const lang = resolve(name.ptr, name.len);
    if (lang == 0) return null;
    return registered(lang, name);
}

/// The language the host serves for files ending `.ext`, registered on this
/// first ask.
pub fn byExtension(ext: []const u8) ?*const fig.Runtime.Entry {
    if (comptime !enabled) return null;
    const lang = resolve_extension(ext.ptr, ext.len);
    if (lang == 0) return null;
    return registered(lang, null);
}

/// The host languages registered so far, by the host's id: each is
/// registered once, whichever of its names or extensions asked first.
var known: std.AutoHashMapUnmanaged(u32, c_int) = .empty;

/// Register host language `lang` if it is not yet, and hand back the entry
/// for `name`, or for its first dialect when asked by extension.
fn registered(lang: u32, name: ?[]const u8) ?*const fig.Runtime.Entry {
    const abi = known.get(lang) orelse blk: {
        fig.Runtime.clearRefusal();
        const abi = fig.Host.register(&call, lang) catch |err| {
            const why = if (err == error.Refused) fig.Runtime.lastRefusal() else @errorName(err);
            std.log.scoped(.languages).err("the language the host serves for `{s}`: {s}", .{ name orelse "an extension", why });
            return null;
        };
        known.put(fig.Host.allocator, lang, abi) catch {};
        break :blk abi;
    };
    if (name) |n| return fig.Runtime.entryByName(n);
    return fig.Runtime.entryByAbi(abi);
}
