//! The languages fig ships, as a consumer registers them: each Zig language
//! the build includes (`-D<name>`), reached through its contract vtable
//! (`kit/contract.zig`) and registered under its own name. Not core — core
//! names no language — but what the CLI, the LSP and the C library bundle.
//!
//! A language registered under the name of a format compiled out of core's
//! registry stands in at that format's number, so a caller holding the
//! number reaches it the same way.

const std = @import("std");
const build_options = @import("build_options");
const list = @import("list.zig");
const Runtime = @import("runtime.zig");
const contract = @import("../kit/contract.zig");

const modules = .{
    .json = @import("json/json.zig"),
    .yaml = @import("yaml/yaml.zig"),
    .toml = @import("toml/toml.zig"),
    .zon = @import("zon/zon.zig"),
    .fig = @import("fig/fig.zig"),
    .ini = @import("ini/ini.zig"),
    .dotenv = @import("dotenv/dotenv.zig"),
    .properties = @import("properties/properties.zig"),
    .plist = @import("plist/plist.zig"),
    .nestedtext = @import("nestedtext/nestedtext.zig"),
};

comptime {
    for (list.rows) |row| {
        if (!@hasField(@TypeOf(modules), row.name)) @compileError("the bundle has no module for '" ++ row.name ++ "'");
    }
}

/// Whether the build bundles the language named `name`.
pub fn bundles(comptime name: []const u8) bool {
    return @field(build_options, "lang_" ++ name);
}

/// The `Language` of the bundled language `name`.
pub fn Language(comptime name: []const u8) type {
    return @field(modules, name).Language;
}

/// The dialect record named `name` (`"json5"`, `"yaml-1.1"`), from
/// whichever bundled language declares it.
pub fn dialect(comptime name: []const u8) DialectOf(name) {
    inline for (@typeInfo(@TypeOf(modules)).@"struct".field_names) |f| {
        inline for (@field(modules, f).Language.dialects) |d| {
            if (comptime std.mem.eql(u8, d.name, name)) return d;
        }
    }
    unreachable;
}

fn DialectOf(comptime name: []const u8) type {
    for (@typeInfo(@TypeOf(modules)).@"struct".field_names) |f| {
        for (@field(modules, f).Language.dialects) |d| {
            if (std.mem.eql(u8, d.name, name)) return @TypeOf(d);
        }
    }
    @compileError("no bundled dialect '" ++ name ++ "'");
}

/// Register the bundled language `name`, unless it is already, and return
/// the format number of its first dialect.
pub fn register(allocator: std.mem.Allocator, comptime name: []const u8) Runtime.RegisterError!c_int {
    if (comptime !bundles(name)) @compileError("'" ++ name ++ "' is not bundled in this build");
    const L = Language(name);
    if (Runtime.entryByName(L.dialects[0].name)) |e| return e.abi;
    const V = contract.VTable(L, "");
    return Runtime.register(allocator, &V.vtable);
}

/// Register every language the build bundles.
pub fn registerAll(allocator: std.mem.Allocator) Runtime.RegisterError!void {
    inline for (list.rows) |row| {
        if (comptime bundles(row.name)) _ = try register(allocator, row.name);
    }
}
