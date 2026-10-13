//! The kit, as a package of its own: what a language written in Zig builds
//! with. A language depends on this package and on nothing else of fig's,
//! which is what keeps it a language and not a part of fig.
//!
//! `zig build test` runs the kit's own tests.

const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    // No target of its own: the kit compiles for whatever imports it, a
    // native library, a WebAssembly module or fig itself.
    _ = b.addModule("kit", .{ .root_source_file = b.path("src/kit.zig") });

    const tests = b.addTest(.{ .root_module = b.createModule(.{
        .root_source_file = b.path("src/kit.zig"),
        .target = target,
        .optimize = optimize,
    }) });
    b.step("test", "Run the kit's tests").dependOn(&b.addRunArtifact(tests).step);
}
