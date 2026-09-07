//! Build-time configuration: the `-D` feature knobs that get baked into the
//! `build_options` module every artifact imports.
//!
//! The package-identity constants (`version`, `abi_version`, `cli_version`,
//! `epoch`) deliberately live in `build.zig`, NOT here — external tooling
//! treats `build.zig` as their canonical home (e.g. `tools/version-floor.zig`
//! parses `cli_version` out of it), so they are passed in via `Versions` rather
//! than owned here.
//!
//! The per-format knobs are not written here either. `src/languages/list.zig`
//! is the one list of formats and conformance suites, and this file declares
//! one `-D<name>` option and one `build_options.lang_<name>` decl per row of
//! it (and one `-D<name>-conformance` / `<name>_conformance` pair per suite).
//! The only knob spelled by hand below is `canonical`, which is not a format:
//! it gates the AST's own oracle encoding.

const std = @import("std");
const list = @import("../languages/list.zig");

/// The package-identity numbers `addFigOptions` bakes into `build_options`.
/// Passed in from `build.zig` (their canonical home) so this module owns only
/// the knob machinery, not the identity.
pub const Versions = struct {
    /// The canonical package version (`build.zig.zon`'s `.version`, parsed in
    /// build.zig where that import path is shallow).
    core: std.SemanticVersion,
    /// The binary C ABI contract version.
    abi: u8,
    /// The `fig` CLI binary's own SemVer track.
    cli: std.SemanticVersion,
    /// The current marketing epoch.
    epoch: []const u8,
};

/// Every build-time knob baked into the `build_options` module.
///
/// This exists as a struct + `addFigOptions` rather than an inline block because
/// the option set is now constructed TWICE: once from the user's `-D` flags —
/// shared by the library, CLI, wasm and `zig build test` — and once with
/// everything forced on for `zig build conformance` (see that step below).
/// Options are baked into a module at configure time, so a step that needs
/// different values has no choice but to build its own `addOptions` instance;
/// funnelling both through one function is what keeps the two from drifting
/// apart as knobs get added.
pub const BuildOptions = struct {
    /// One per `list.rows`, in that order: whether the format is compiled in.
    langs: [list.rows.len]bool,
    /// One per `list.suites`, in that order: whether the suite runs.
    suites: [list.suites.len]bool,
    /// The canonical form is the AST's own 1:1 oracle encoding — invaluable
    /// in tests but not exposed through the C ABI or any binding, so shipping
    /// it in the default library/CLI/wasm is dead weight for everyone but the
    /// test suite. Opt-in like plist (`-Dcanonical=true`); the code still
    /// compiles for ANY test build regardless, gated as
    /// `lang_canonical or @import("builtin").is_test`.
    lang_canonical: bool,

    /// Whether the format named `name` (a `list.Row.name`) is compiled in.
    pub fn lang(self: BuildOptions, comptime name: []const u8) bool {
        return self.langs[comptime list.indexOf(name)];
    }

    /// The configuration `zig build conformance` builds: every suite and every
    /// language on, independent of whatever `-D` flags the caller passed, so the
    /// gate means the same thing on every machine.
    ///
    /// Forcing the suites on is the point of the step. Forcing every language
    /// on is a deliberate second win: plist and canonical are both off by
    /// default, so nothing else in CI ever compiles them together — this is
    /// the only build that proves the everything-on configuration still
    /// builds at all. It builds both the library's test root and the CLI's
    /// (tests.zig): the CLI is what instantiates the generic engines for
    /// every language, so the library alone is not the whole proof.
    pub const all_on: BuildOptions = .{
        .langs = @splat(true),
        .suites = @splat(true),
        .lang_canonical = true,
    };
};

/// Read every `-D` flag into a `BuildOptions`. Called once from `build.zig` for
/// the user-facing configuration; `zig build conformance` uses
/// `BuildOptions.all_on` instead of calling this.
pub fn resolve(b: *std.Build) BuildOptions {
    var cfg: BuildOptions = undefined;

    inline for (list.suites, 0..) |suite, i| {
        cfg.suites[i] = b.option(bool, suite.name ++ "-conformance", suite.help) orelse false;
    }

    // Per-language feature gates. Any format can be compiled out to shrink the
    // binary and drop its parser/printer — including JSON, now that the native
    // `.fig` format exists and `Language.detect()` sniffs every compiled-in
    // language rather than assuming a JSON base. A build with no language at all
    // is rejected at the call sites that need one (e.g. the C ABI editor union).
    // Which formats default off, and why, is said on each row of the list.
    inline for (list.rows, 0..) |row, i| {
        cfg.langs[i] = b.option(bool, row.name, row.help) orelse row.default_on;
    }

    cfg.lang_canonical = b.option(bool, "canonical", "Include the canonical oracle format (opt-in; default off — used mainly by the test suite)") orelse false;
    return cfg;
}

/// Build one `build_options` instance from `cfg` and `ver`. The version/ABI
/// values come in via `ver` rather than being owned here because they are
/// canonical facts about the package (their home is `build.zig`) rather than
/// knobs — every build gets the same ones.
pub fn addFigOptions(b: *std.Build, cfg: BuildOptions, ver: Versions) *std.Build.Step.Options {
    const options = b.addOptions();
    inline for (list.suites, 0..) |suite, i| {
        options.addOption(bool, suite.name ++ "_conformance", cfg.suites[i]);
    }
    // Language gates, consumed across the codebase as `build_options.lang_*`.
    inline for (list.rows, 0..) |row, i| {
        options.addOption(bool, "lang_" ++ row.name, cfg.langs[i]);
    }
    options.addOption(bool, "lang_canonical", cfg.lang_canonical);
    // Library version surfaced through the C ABI (`fig_version` /
    // `fig_version_string`). Parsed from `.version` in `build.zig.zon` — the one
    // canonical package version — and split into the components the ABI's
    // packed-integer/string accessors need. `zig build abi-check` separately
    // asserts that bindings/c/include/fig.h's FIG_VERSION_* macros match this same source.
    options.addOption(u8, "version_major", @intCast(ver.core.major));
    options.addOption(u8, "version_minor", @intCast(ver.core.minor));
    options.addOption(u8, "version_patch", @intCast(ver.core.patch));
    // The binary C ABI contract version, surfaced through `fig_abi_version()`.
    // `zig build abi-check` asserts bindings/c/include/fig.h's FIG_ABI_VERSION matches this.
    options.addOption(u8, "abi_version", ver.abi);
    // The CLI's own version (see `cli_version`'s doc comment in build.zig),
    // surfaced by `fig version` alongside the embedded core version.
    options.addOption(u8, "cli_version_major", @intCast(ver.cli.major));
    options.addOption(u8, "cli_version_minor", @intCast(ver.cli.minor));
    options.addOption(u8, "cli_version_patch", @intCast(ver.cli.patch));
    // The current marketing epoch (see `epoch`'s doc comment in build.zig),
    // surfaced only by the CLI's `fig version` — no ABI/library counterpart.
    options.addOption([]const u8, "epoch", ver.epoch);
    return options;
}
