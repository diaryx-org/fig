//! The kit, as a package of its own: what a language written in Zig builds
//! with. A language depends on this package and on nothing else of fig's,
//! which is what keeps it a language and not a part of fig.
//!
//! `zig build test` runs the kit's own tests. A language's own `build.zig`
//! calls `language` below, which gives it its module and its two libraries:
//!
//!     const std = @import("std");
//!
//!     pub fn build(b: *std.Build) void {
//!         _ = @import("fig_kit").language(b, .{ .name = "yaml" });
//!     }

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

    // The kit's rules as a WebAssembly module, `fig-kit.wasm`, which the
    // JavaScript kit answers through (`src/js.zig`).
    const wasm = b.addExecutable(.{
        .name = "fig-kit",
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/js.zig"),
            .target = b.resolveTargetQuery(.{ .cpu_arch = .wasm32, .os_tag = .freestanding }),
            .optimize = .ReleaseSmall,
            .strip = true,
        }),
    });
    wasm.entry = .disabled;
    wasm.rdynamic = true;
    b.step("wasm", "Build the kit's WebAssembly module, for the JavaScript kit").dependOn(&b.addInstallArtifact(wasm, .{}).step);
}

pub const LanguageOptions = struct {
    /// The language's name. Its source is `src/<name>.zig`, its module
    /// `fig_<name>`, its C library `libfig_<name>.a` with the header
    /// `include/fig/<name>.h`, and its WebAssembly module `fig-<name>.wasm`.
    name: []const u8,
    /// What the language's own tests import beyond the kit.
    test_imports: []const std.Build.Module.Import = &.{},
};

/// What `language` builds.
pub const Language = struct {
    /// The language as a Zig module, `fig_<name>`, which fig bundles.
    module: *std.Build.Module,
    /// The language as a C library of its own, `libfig_<name>.a`: the one
    /// export `fig_<name>_register`, from `src/c.zig`.
    c_library: *std.Build.Step.Compile,
    /// The language as a WebAssembly module of its own, `fig-<name>.wasm`,
    /// from `src/wasm.zig`.
    wasm: *std.Build.Step.Compile,
};

/// The build of a language package, called from its `build.zig`. The
/// package depends on this one under the name `fig_kit`. It declares
/// `-Dtarget`, `-Doptimize` and `-Dstrip`, and these steps:
///
///   * `test`: the language's own tests;
///   * `c`: its C library and header, installed to `lib/` and `include/`;
///   * `wasm`: its WebAssembly module, installed to `bin/`.
///
/// The default install step builds both libraries.
pub fn language(b: *std.Build, options: LanguageOptions) Language {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});
    const strip = b.option(bool, "strip", "Strip debug information") orelse (optimize == .small);
    const kit = b.dependency("fig_kit", .{}).module("kit");
    const name = options.name;
    const source = b.path(b.fmt("src/{s}.zig", .{name}));

    const module = b.addModule(b.fmt("fig_{s}", .{name}), .{
        .root_source_file = source,
        .imports = &.{.{ .name = "kit", .module = kit }},
    });

    const test_module = b.createModule(.{
        .root_source_file = source,
        .target = target,
        .optimize = optimize,
        .imports = options.test_imports,
    });
    test_module.addImport("kit", kit);
    const tests = b.addTest(.{ .root_module = test_module });
    b.step("test", b.fmt("Run {s}'s tests", .{name})).dependOn(&b.addRunArtifact(tests).step);

    // The C library links beside fig's `libfig.a`, which resolves its one
    // import, `fig_language_register`.
    const c_library = b.addLibrary(.{
        .linkage = .static,
        .name = b.fmt("fig_{s}", .{name}),
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/c.zig"),
            .target = target,
            .optimize = optimize,
            .strip = strip,
            // Android has no libc Zig can link, and the kit's C exports
            // need none; wasm has no libc either.
            .link_libc = !target.result.cpu.arch.isWasm() and !target.result.abi.isAndroid(),
            // Position-independent, as fig's own C library is: every
            // Android consumer is a shared object, which refuses the
            // local-exec TLS model non-PIC code uses, and Rust links a musl
            // program as a static PIE, which refuses an absolute relocation.
            .pic = if (target.result.cpu.arch.isWasm()) null else true,
            .imports = &.{.{ .name = "kit", .module = kit }},
        }),
    });
    // `installArtifact`, which a package depending on this one finds the
    // library by (`dependency.artifact("fig_<name>")`).
    b.installArtifact(c_library);
    const install_c = b.addInstallArtifact(c_library, .{});
    const install_header = b.addInstallHeaderFile(b.path(b.fmt("include/fig/{s}.h", .{name})), b.fmt("fig/{s}.h", .{name}));
    const c_step = b.step("c", b.fmt("Build {s}'s C library and header", .{name}));
    c_step.dependOn(&install_c.step);
    c_step.dependOn(&install_header.step);
    b.getInstallStep().dependOn(&install_header.step);

    // A freestanding reactor: no `_start`, every `fig_lang_*` export kept.
    const wasm = b.addExecutable(.{
        .name = b.fmt("fig-{s}", .{name}),
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/wasm.zig"),
            .target = b.resolveTargetQuery(.{ .cpu_arch = .wasm32, .os_tag = .freestanding }),
            .optimize = .small,
            .strip = true,
            .imports = &.{.{ .name = "kit", .module = kit }},
        }),
    });
    wasm.entry = .disabled;
    wasm.rdynamic = true;
    const wasm_step = b.step("wasm", b.fmt("Build {s}'s WebAssembly module", .{name}));
    wasm_step.dependOn(&b.addInstallArtifact(wasm, .{}).step);
    b.installArtifact(wasm);

    return .{ .module = module, .c_library = c_library, .wasm = wasm };
}
