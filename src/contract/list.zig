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
//! A leaf, deliberately: both `build.zig` (through `Options.zig`) and the
//! library read it, so it can import nothing and hold nothing but data.

/// One format. `name` is the package (`languages/<name>/`, a dependency of
/// fig's named `fig_<name>`), its module (`fig_<name>`), the `-D<name>`
/// build option and the `build_options.lang_<name>` decl the option becomes.
pub const Row = struct {
    name: [:0]const u8,
    /// The `-D<name>` option's help text.
    help: []const u8,
    /// Whether the format is compiled in when the option is not given.
    default_on: bool,
};

/// The formats, in registry order. The order is load-bearing: it is the
/// member order of every enum `language.zig` derives from the registry, so a
/// new format is appended, never inserted.
pub const rows = [_]Row{
    .{ .name = "json", .help = "Include JSON/JSONC/JSON5 support", .default_on = true },
    .{ .name = "yaml", .help = "Include YAML support", .default_on = true },
    .{ .name = "toml", .help = "Include TOML support", .default_on = true },
    .{ .name = "zon", .help = "Include ZON support", .default_on = true },
    // There is no `xml` row. Generic XML — a best-effort fold of attributes
    // and `#text` into a mapping, with no typed scalars and a single-root-key
    // printer — was a selectable, opt-in format through core 2.x and was
    // removed in core 3.0; its C ABI value, 6, is retired and never reused.
    // What survives is the shared XML *lexing substrate*, `xml/tokenizer.zig`,
    // which typed flavors (plist, and future `.csproj`/manifest readers) sit
    // on top of and which compiles whenever one of them does.
    .{ .name = "figl", .help = "Include the figl authoring dialect support", .default_on = true },
    .{ .name = "ini", .help = "Include INI support", .default_on = true },
    .{ .name = "dotenv", .help = "Include dotenv (.env) support", .default_on = true },
    .{ .name = "properties", .help = "Include Java .properties support", .default_on = true },
    // plist (XML variant only so far): the newest, least battle-tested
    // format, opt-in via `-Dplist=true`. plist is a first-class typed flavor
    // (typed scalars, round-trips, in-place editor) and is the intended
    // long-term home for structured XML config.
    .{ .name = "plist", .help = "Include Apple XML property list support (opt-in; default off)", .default_on = false },
    // NestedText (nestedtext.org): reader + printer + editor, untyped-string
    // scalars like INI, held to the official test suite (in fig-conformance).
    // On by default like TOML/ZON/INI.
    .{ .name = "nestedtext", .help = "Include NestedText support", .default_on = true },
};

/// The position of the row named `name`, or a compile error naming what is
/// not a format.
pub fn indexOf(comptime name: []const u8) usize {
    inline for (rows, 0..) |row, i| {
        if (comptime eql(row.name, name)) return i;
    }
    @compileError("'" ++ name ++ "' is not a row of src/contract/list.zig");
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
    // twice.
    for (rows, 0..) |a, ai| {
        for (rows[ai + 1 ..]) |b| {
            if (eql(a.name, b.name)) @compileError("two rows of src/contract/list.zig are named '" ++ a.name ++ "'");
        }
    }
}
