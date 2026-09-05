//! The one list of formats.
//!
//! Every place that has to do something per format reads this file: the build
//! declares one `-D<name>` option and one `build_options.lang_<name>` decl per
//! row (`src/build/Options.zig`), `language.zig` pairs each row with its module
//! and derives the registry from the pairing, and `tools/validate-check.zig`
//! writes its all-formats-off `build_options` stub from it. Adding a format is
//! appending a row here and one `@import` line in `language.zig`'s `slots` —
//! the import has to be spelled by hand because Zig's `@import` takes only a
//! string literal, so it cannot be built from a row — and `language.zig`
//! refuses to build if the two lists disagree in either direction.
//!
//! The conformance suites are listed here too because they are the other
//! per-format build option, though they are per SUITE rather than per format:
//! JSON's corpus is scored by two (`json`, `json5`) and every other format
//! with a corpus by one.
//!
//! A leaf, deliberately: both `build.zig` (through `Options.zig`) and the
//! library read it, so it can import nothing and hold nothing but data.

/// One format. `name` is the directory (`src/languages/<name>/`), the entry
/// file (`<name>.zig` inside it), the `-D<name>` build option and the
/// `build_options.lang_<name>` decl the option becomes.
pub const Row = struct {
    name: [:0]const u8,
    /// The `-D<name>` option's help text.
    help: []const u8,
    /// Whether the format is compiled in when the option is not given.
    default_on: bool,
};

/// One conformance suite: `-D<name>-conformance` / `build_options.<name>_conformance`,
/// scoring the corpus of `format` (a `Row.name`).
pub const Suite = struct {
    name: [:0]const u8,
    format: [:0]const u8,
    help: []const u8,
};

/// The formats, in registry order. The order is load-bearing: it is the
/// member order of every enum `language.zig` derives from the registry, so a
/// new format is appended, never inserted.
pub const rows = [_]Row{
    .{ .name = "json", .help = "Include JSON/JSONC/JSON5 support", .default_on = true },
    .{ .name = "yaml", .help = "Include YAML support", .default_on = true },
    .{ .name = "toml", .help = "Include TOML support", .default_on = true },
    .{ .name = "zon", .help = "Include ZON support", .default_on = true },
    // Opt-in even in a full build. Generic XML is a demoted, best-effort
    // *fold* (attributes/`#text` collapse, no typed scalars, single-root-key
    // output), NOT a first-class config format, and it is slated for removal
    // as a selectable format in a future major (see
    // `docs/BREAKING-CHANGES.md`). What survives that removal is the shared
    // XML *lexing substrate* — `xml/tokenizer.zig` — that typed flavors
    // (plist, and future `.csproj`/manifest readers) sit on top of; that
    // layer is always compiled when any XML-family flavor is, so it does not
    // ride on this gate. The gate controls only the generic reader/printer,
    // which is why non-users shouldn't pay for it by default.
    .{ .name = "xml", .help = "Include XML support (opt-in; default off)", .default_on = false },
    .{ .name = "fig", .help = "Include the fig authoring dialect support", .default_on = true },
    .{ .name = "ini", .help = "Include INI support", .default_on = true },
    .{ .name = "dotenv", .help = "Include dotenv (.env) support", .default_on = true },
    .{ .name = "properties", .help = "Include Java .properties support", .default_on = true },
    // plist (XML variant only so far): the newest, least battle-tested
    // format, opt-in via `-Dplist=true`. Unlike generic xml above, plist is a
    // first-class typed flavor (typed scalars, round-trips, in-place editor)
    // and is the intended long-term home for structured XML config — it is
    // not slated for removal.
    .{ .name = "plist", .help = "Include Apple XML property list support (opt-in; default off)", .default_on = false },
    // NestedText (nestedtext.org): reader + printer + editor, untyped-string
    // scalars like INI. The official test suite (vendored to
    // `testdata/nestedtext/tests.json`) is wired up from the start. On by
    // default like TOML/ZON/INI.
    .{ .name = "nestedtext", .help = "Include NestedText support", .default_on = true },
};

/// The conformance suites, in the order `zig build --help` lists them.
pub const suites = [_]Suite{
    .{ .name = "json", .format = "json", .help = "Run JSON conformance tests" },
    .{ .name = "json5", .format = "json", .help = "Run JSON5 conformance tests" },
    .{ .name = "yaml", .format = "yaml", .help = "Run YAML conformance tests" },
    .{ .name = "toml", .format = "toml", .help = "Run TOML conformance tests" },
    .{ .name = "plist", .format = "plist", .help = "Run plist conformance tests" },
    .{ .name = "nestedtext", .format = "nestedtext", .help = "Run NestedText conformance tests" },
};

/// The position of the row named `name`, or a compile error naming what is
/// not a format.
pub fn indexOf(comptime name: []const u8) usize {
    inline for (rows, 0..) |row, i| {
        if (comptime eql(row.name, name)) return i;
    }
    @compileError("'" ++ name ++ "' is not a row of src/languages/list.zig");
}

/// Whether `name` is a row.
pub fn has(comptime name: []const u8) bool {
    inline for (rows) |row| {
        if (comptime eql(row.name, name)) return true;
    }
    return false;
}

// No `std` here (this file is a leaf; see the module doc), so the one string
// comparison it needs is spelled out.
fn eql(a: []const u8, b: []const u8) bool {
    if (a.len != b.len) return false;
    for (a, b) |x, y| if (x != y) return false;
    return true;
}

comptime {
    // Names are identities: a duplicate row would declare one build option
    // twice, and a suite must score a format that exists.
    for (rows, 0..) |a, ai| {
        for (rows[ai + 1 ..]) |b| {
            if (eql(a.name, b.name)) @compileError("two rows of src/languages/list.zig are named '" ++ a.name ++ "'");
        }
    }
    for (suites, 0..) |a, ai| {
        if (!has(a.format)) @compileError("conformance suite '" ++ a.name ++ "' scores '" ++ a.format ++ "', which is not a format");
        for (suites[ai + 1 ..]) |b| {
            if (eql(a.name, b.name)) @compileError("two conformance suites are named '" ++ a.name ++ "'");
        }
    }
}
