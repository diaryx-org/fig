//! The inputs every build helper in `build/` shares, bundled so each `add`
//! entry point takes one `Context` instead of re-threading the same five
//! parameters. Built once in `build.zig` after the target/optimize/strip
//! triple is resolved and the single `build_options` module is created, then
//! handed to `artifacts`, `tools`, `checks` and `tests` in turn.
//!
//! This is a "struct file": the file's top-level *is* the struct, so callers
//! write `const Context = @import("Context.zig");` and take a `ctx: Context`.

const std = @import("std");
const Options = @import("Options.zig");
const list = @import("../languages/list.zig");

/// The build graph everything hangs off of.
b: *std.Build,
/// The target selected from `-Dtarget` (or the host default). `.result` gives
/// the resolved `std.Target` for `.cpu.arch`/`.abi` queries.
target: std.Build.ResolvedTarget,
/// The optimize mode from `-Doptimize` (or the default).
optimize: std.lang.Optimize,
/// Whether to strip debug info (`-Dstrip`, defaulting to on for ReleaseSmall).
strip: bool,
/// The resolved `-D` configuration behind `options_mod`. The module is what
/// artifacts IMPORT; this is the same values readable by the build script
/// itself, for decisions the build graph has to make rather than the compiled
/// code — chiefly whether to build an artifact at all.
cfg: Options.BuildOptions,
/// The one `build_options` module instance, shared by every artifact. Built
/// once because `addOptions` per-module would generate a fresh module from the
/// same generated file, which Zig rejects ("file belongs to two modules").
options_mod: *std.Build.Module,
/// The package-identity numbers (the version, the ABI version, the epoch), sourced from
/// `build.zig`. Carried here because more than one stage needs them: `tests`
/// rebuilds a second `build_options` for the all-on run, and `checks`
/// embeds the version into the abi/semver tool args.
ver: Options.Versions,
/// The helper wire's JSON-RPC envelope, or null when the build leaves the
/// wire out (`-Dhelper-wire=false`) or the lazy dependency is still being
/// fetched. Every module that can reach `src/languages/wire.zig` imports it
/// through `importWire`.
zigjr: ?*std.Build.Module,
/// The kit (`kit/`), a package of its own: what core and every language
/// import as `kit`. One module, so core and a bundled language agree on
/// every type in it.
kit: *std.Build.Module,
/// The kit package itself, for its tests.
kit_dep: *std.Build.Dependency,
/// Each language fig ships, a package of its own (`languages/<name>/`), in
/// `list.rows` order, built for this build's target.
languages: [list.rows.len]*std.Build.Dependency,

/// Give `m` the `kit` import.
pub fn importKit(self: @This(), m: *std.Build.Module) void {
    m.addImport("kit", self.kit);
}

/// Give `m` every language's module, `fig_<name>`. Unreferenced, an import
/// costs nothing: the bundle reaches a language only when it bundles it,
/// and core's tests reach the ones they read.
pub fn importLanguages(self: @This(), m: *std.Build.Module) void {
    for (list.rows, self.languages) |row, dep| {
        const name = self.b.fmt("fig_{s}", .{row.name});
        m.addImport(name, dep.module(name));
    }
}

/// Give `m` the `zigjr` import, when the build carries the wire.
pub fn importWire(self: @This(), m: *std.Build.Module) void {
    if (self.zigjr) |z| m.addImport("zigjr", z);
}
