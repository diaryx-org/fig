//! The XML property list for fig, as a package of its own, which depends on the
//! kit and on nothing else of fig's. `zig build test` runs its tests, `zig
//! build c` builds its C library and `zig build wasm` its WebAssembly module:
//! see `language` in the kit's `build.zig`.

const std = @import("std");

pub fn build(b: *std.Build) void {
    // Its tests print what they read as JSON.
    const json = b.dependency("fig_json", .{}).module("fig_json");
    _ = @import("fig_kit").language(b, .{
        .name = "plist",
        .test_imports = &.{.{ .name = "fig_json", .module = json }},
    });
}
