const std = @import("std");
const Context = @import("src/build/Context.zig");
const Options = @import("src/build/Options.zig");
const artifacts = @import("src/build/artifacts.zig");
const tools = @import("src/build/tools.zig");
const tests = @import("src/build/tests.zig");
const checks = @import("src/build/checks.zig");

// The build graph lives in src/build/, split by role and wired together at the
// bottom of `build`:
//   * src/build/Context.zig   — the inputs every stage shares (target, optimize, …)
//   * src/build/Options.zig   — the `-D` knobs baked into `build_options`
//   * src/build/artifacts.zig — the fig lib, CLI, LSP, C ABI (static+shared), wasm/wasi
//   * src/build/tools.zig     — vendor-rust, gen-*-conformance, sync/check-figl, version-sync
//   * src/build/tests.zig     — test, conformance, fuzz, install-tests
//   * src/build/checks.zig    — abi/semver guards, rust/ts suites, the `check` gate
//
// The package-identity constants below stay HERE, not in Options.zig, and are
// handed to the rest of the graph via `Options.Versions`.

/// The one version every artifact ships under — the core, the CLI, the Rust
/// crates and both npm packages (see docs/VERSIONING.md) — parsed once from
/// `build.zig.zon`'s `.version`, so the C ABI's `fig_version*` accessors, `fig
/// version`, and the version-drift check all read from a single source.
const version = std.SemanticVersion.parse(@import("build.zig.zon").version) catch
    @compileError("invalid `.version` in build.zig.zon");

/// The binary C ABI contract version (see `FIG_ABI_VERSION` in bindings/c/include/fig.h).
/// Canonical source of truth — surfaced to the C ABI as `fig_abi_version()` and
/// pinned to the header macro by `zig build abi-check`. A monotonic counter,
/// bumped ONLY on a breaking ABI change (which fig's forward-compat policy makes
/// rare); decoupled from the marketing `.version` above so a feature release does
/// not move it. `zig build semver-check` requires it to increment whenever the C
/// ABI diff against the last release tag is breaking.
const abi_version: u8 = 3;

/// The current "epoch" — a marketing name that changes far less often than
/// `version`'s major (see docs/VERSIONING.md: "major releases are not
/// sacred," so this exists precisely to give users a stable, human-facing
/// handle across a run of otherwise-eager SemVer bumps). Purely cosmetic —
/// no compatibility contract, so it lives here as a bare constant (like
/// `abi_version`) rather than in build.zig.zon (a
/// toolchain-parsed package manifest with its own schema) or the C ABI (which
/// only ever exposes things a consumer might actually branch on). Surfaced
/// only by the CLI's `fig version`.
const epoch = "Texas Everbearing";

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});
    const strip = b.option(bool, "strip", "Strip debug information") orelse (optimize == .small);

    const ver: Options.Versions = .{
        .version = version,
        .abi = abi_version,
        .epoch = epoch,
    };

    // The user-facing `-D` configuration, baked into the one shared
    // `build_options` module every artifact imports.
    const cfg = Options.resolve(b);
    const fig_options = Options.addFigOptions(b, cfg, ver);
    const options_mod = fig_options.createModule();

    // The same module written out as a source file, for building the CLI
    // without a build system: a Zig compiled to wasm32-wasi has `build`
    // compiled out, and WASI preview 1 cannot spawn a build runner anyway, so
    // `build_options` has to arrive as a plain .zig file the `build-exe`
    // command line can name. Emitting it from `fig_options` rather than
    // hand-maintaining a copy is what keeps it from drifting. Attached to each
    // `v*` release by .github/workflows/release-binaries.yml.
    const wasi_options_step = b.step("wasi-options", "Write build_options.zig for a build-system-free build");
    wasi_options_step.dependOn(&b.addInstallFileWithDir(fig_options.getOutput(), .prefix, "build_options.zig").step);

    const ctx: Context = .{
        .b = b,
        .target = target,
        .optimize = optimize,
        .strip = strip,
        .cfg = cfg,
        .options_mod = options_mod,
        .ver = ver,
    };

    // Order matters only for the data handed downstream, not the build graph:
    // artifacts hands its lib/exe to tools & tests, and its static C ABI lib to
    // checks; tools & tests hand their gate steps to checks.
    const arts = artifacts.add(ctx);
    const tools_result = tools.add(ctx, arts);
    const tests_result = tests.add(ctx, arts);
    checks.add(ctx, arts, .{
        .check_figl_step = tools_result.check_figl_step,
        .validate_check_step = tools_result.validate_check_step,
        .vendor_check_step = tools_result.vendor_check_step,
        .version_check_step = tools_result.version_check_step,
        .test_step = tests_result.test_step,
        .conformance_step = tests_result.conformance_step,
    });
}
