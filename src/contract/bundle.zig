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
const contract = @import("kit").contract;

const modules = .{
    .json = @import("fig_json"),
    .yaml = @import("fig_yaml"),
    .toml = @import("fig_toml"),
    .zon = @import("fig_zon"),
    .figl = @import("fig_figl"),
    .ini = @import("fig_ini"),
    .dotenv = @import("fig_dotenv"),
    .properties = @import("fig_properties"),
    .plist = @import("fig_plist"),
    .nestedtext = @import("fig_nestedtext"),
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

/// The module of the language `name`, bundled or not: its `Language`,
/// `Parser`, `Printer` and `Type`.
pub fn module(comptime name: []const u8) type {
    return @field(modules, name);
}

/// The `Language` of the bundled language `name`.
pub fn Language(comptime name: []const u8) type {
    return @field(modules, name).Language;
}

/// The declared dialects of every language fig ships, in list order,
/// whether or not this build bundles each — what a consumer lists as the
/// formats it knows of. `--spec` versions are rows of their own once
/// registered, but are not listed here.
pub const all_dialects: []const []const u8 = blk: {
    var out: []const []const u8 = &.{};
    for (list.rows) |row| {
        for (@field(modules, row.name).Language.dialects) |d| out = out ++ [_][]const u8{d.name};
    }
    break :blk out;
};

/// The language fig ships that owns extension `ext` but this build does not
/// bundle, or null: what a consumer names when such a file meets no
/// language, rather than reading it as something else.
pub fn unbundledOwner(ext: []const u8) ?[]const u8 {
    inline for (list.rows) |row| {
        if (comptime !bundles(row.name)) {
            inline for (@field(modules, row.name).Language.extensions) |x| {
                if (std.mem.eql(u8, x, ext)) return row.name;
            }
        }
    }
    return null;
}

/// `all_dialects`, less the languages this build does not bundle.
pub const dialect_names: []const []const u8 = blk: {
    var out: []const []const u8 = &.{};
    for (list.rows) |row| {
        if (!@field(build_options, "lang_" ++ row.name)) continue;
        for (@field(modules, row.name).Language.dialects) |d| out = out ++ [_][]const u8{d.name};
    }
    break :blk out;
};

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
    if (Runtime.registeredByName(L.dialects[0].name)) |e| return e.abi;
    const V = contract.VTable(L, "");
    // Its build ran the harness over its samples (`contract_test.zig`); a
    // test build runs it again here.
    return Runtime.registerWith(allocator, &V.vtable, .{ .harness = @import("builtin").is_test });
}

/// Register every language the build bundles.
pub fn registerAll(allocator: std.mem.Allocator) Runtime.RegisterError!void {
    inline for (list.rows) |row| {
        if (comptime bundles(row.name)) _ = try register(allocator, row.name);
    }
}

// Test discovery: referencing each language module pulls its own `test {}`
// block — and through it every submodule's tests — into the suite, so
// `root.zig` names no language. Every module, bundled or not: a module's
// tests skip themselves when its language is left out.
test {
    inline for (@typeInfo(@TypeOf(modules)).@"struct".field_names) |f| _ = @field(modules, f);
    inline for (list.rows) |row| {
        if (comptime bundles(row.name)) @import("language.zig").validate(Language(row.name));
    }
}
