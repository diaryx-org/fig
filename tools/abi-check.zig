//! Dev tool: C ABI symbol diff. Cross-checks that every `export fn fig_*` in
//! src/c_api.zig has a matching prototype in bindings/c/include/fig.h, and vice versa —
//! catching a symbol that is exported but undocumented (a caller cannot find it)
//! or declared but unimplemented (a dangling prototype). This is the check that
//! catches drift like `fig_alloc`/`fig_free` being exported without a header
//! declaration.
//!
//! Run via `zig build abi-check`, which also compiles the abi_probe.{c,cpp} TUs
//! against fig.h as C and C++ to prove the header parses and links in both. What
//! is NOT checked here: signatures (C has no name mangling, so a param-type or
//! arity change links fine) — that drift would need parsing and comparing both
//! sides' parameter lists.
//!
//! It also verifies that the header's FIG_VERSION_MAJOR/MINOR/PATCH macros match
//! the canonical version (parsed from build.zig.zon and passed in by build.zig),
//! so the C header cannot silently drift from the package version, and that the
//! header's FIG_ABI_VERSION macro matches the canonical ABI version compiled into
//! `fig_abi_version()` (likewise passed in by build.zig).
//!
//! Finally it diffs the header's `FIG_FORMAT_*` enumerators against the format
//! registry (`src/languages/language.zig`'s `dialects`) in both directions —
//! name AND integer value. This is the half of the format ABI that the symbol
//! diff above cannot see: `FigFormat` is one symbol-free `typedef enum`, so a
//! renumbered or missing format is invisible to a prototype comparison but
//! catastrophic to a compiled caller, which holds the integer. The registry is
//! the third party here (the Zig enum is pinned to it by a comptime assert in
//! src/c_api.zig), so the header, the implementation and the table can no
//! longer drift pairwise.
//!
//! The same diff runs over the three hand-written mirrors of that enum in the
//! bindings: `fig-sys`'s `FigFormat` (name and value), the TypeScript
//! `Format` (name and value), and the Rust wrapper's `Format` (name only — its
//! values are the `From` impl's business). Each used to drift on its own; the
//! Rust wrapper went five formats behind before this check existed.
//!
//! `FigExtKind` is held the same way: fig.h's `FIG_EXT_*` (name and value),
//! the TypeScript `ExtKind` (name and value) and the Rust `ExtKind` (name
//! only) are each diffed against the core's `AST.Node.Kind.Extended.ExtKind`,
//! whose ordinal is the ABI value. It was not, and fig.h and both bindings
//! stopped at `number_special` for a release while `fig_node_extended` was
//! already returning the two plist kinds after it.
//!
//! Usage (driven by build.zig):
//!   abi-check <header.h> <impl.zig> <major.minor.patch> <abi-version>
//!             <fig-sys/lib.rs> <typescript/types.ts> <fig/lib.rs> <fig/value.rs>

const std = @import("std");
/// For `Language.dialects` — the format registry, which owns the canonical
/// `FIG_FORMAT_*` values. Wired in by src/build/checks.zig.
const fig = @import("fig");

const max_file = 4 * 1024 * 1024;

pub fn main(init: std.process.Init) !void {
    const io = init.io;
    var arena_state = std.heap.ArenaAllocator.init(init.gpa);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    var args = try init.minimal.args.iterateAllocator(init.gpa);
    defer args.deinit();
    _ = args.next(); // argv0
    const header_path = args.next() orelse return error.MissingArgument;
    const impl_path = args.next() orelse return error.MissingArgument;
    const want_version = args.next() orelse return error.MissingArgument;
    const want_abi = args.next() orelse return error.MissingArgument;
    const sys_path = args.next() orelse return error.MissingArgument;
    const ts_path = args.next() orelse return error.MissingArgument;
    const rust_path = args.next() orelse return error.MissingArgument;
    const rust_value_path = args.next() orelse return error.MissingArgument;

    const cwd = std.Io.Dir.cwd();
    const header = try cwd.readFileAlloc(io, header_path, arena, .limited(max_file));
    const impl = try cwd.readFileAlloc(io, impl_path, arena, .limited(max_file));
    const sys_src = try cwd.readFileAlloc(io, sys_path, arena, .limited(max_file));
    const ts_src = try cwd.readFileAlloc(io, ts_path, arena, .limited(max_file));
    const rust_src = try cwd.readFileAlloc(io, rust_path, arena, .limited(max_file));
    const rust_value_src = try cwd.readFileAlloc(io, rust_value_path, arena, .limited(max_file));

    // Declared: `fig_x(` tokens on non-comment header lines (so prose mentions
    // like "release with fig_free" don't count as declarations).
    const declared = try collectDeclared(arena, header);
    // Exported: every `pub export fn fig_*` in the implementation.
    const exported = try collectExported(arena, impl);

    var fail = false;
    for (exported) |name| {
        if (!contains(declared, name)) {
            if (!fail) std.debug.print("abi-check: FAIL\n", .{});
            std.debug.print("  exported by c_api.zig but NOT declared in fig.h: {s}\n", .{name});
            fail = true;
        }
    }
    for (declared) |name| {
        if (!contains(exported, name)) {
            if (!fail) std.debug.print("abi-check: FAIL\n", .{});
            std.debug.print("  declared in fig.h but NOT exported by c_api.zig: {s}\n", .{name});
            fail = true;
        }
    }
    // Version drift: fig.h's macros must match the canonical build.zig.zon version.
    const header_version = try headerVersion(arena, header);
    if (!std.mem.eql(u8, header_version, want_version)) {
        if (!fail) std.debug.print("abi-check: FAIL\n", .{});
        std.debug.print(
            "  version drift: fig.h is {s} but build.zig.zon is {s} (update the FIG_VERSION_* macros)\n",
            .{ header_version, want_version },
        );
        fail = true;
    }

    // ABI-version drift: fig.h's FIG_ABI_VERSION must match the value compiled
    // into `fig_abi_version()` (build.zig's `abi_version`), so the macro a caller
    // compiles against and the integer the library reports cannot disagree.
    const header_abi = macroInt(header, "FIG_ABI_VERSION") orelse return error.MissingAbiMacro;
    const want_abi_int = std.fmt.parseInt(u32, want_abi, 10) catch return error.BadAbiArg;
    if (header_abi != want_abi_int) {
        if (!fail) std.debug.print("abi-check: FAIL\n", .{});
        std.debug.print(
            "  ABI-version drift: fig.h FIG_ABI_VERSION is {d} but build.zig is {d} (update the macro or the abi_version constant)\n",
            .{ header_abi, want_abi_int },
        );
        fail = true;
    }

    // Runtime-range drift: fig.h's FIG_FORMAT_RUNTIME_BASE and fig-sys's mirror
    // of it must both state the registry's `runtime_abi_base`. The registry
    // refuses a compiled-in value at or above it at comptime; this is the check
    // that the number a caller compiles against is the number that rule uses.
    const want_base: i64 = fig.Language.runtime_abi_base;
    const header_base = macroInt(header, "FIG_FORMAT_RUNTIME_BASE") orelse return error.MissingRuntimeBaseMacro;
    if (@as(i64, header_base) != want_base) {
        if (!fail) std.debug.print("abi-check: FAIL\n", .{});
        std.debug.print(
            "  runtime-range drift: fig.h FIG_FORMAT_RUNTIME_BASE is {d} but the format registry's runtime_abi_base is {d}\n",
            .{ header_base, want_base },
        );
        fail = true;
    }
    const sys_base = rustConstInt(sys_src, "FIG_FORMAT_RUNTIME_BASE") orelse return error.MissingRuntimeBaseConst;
    if (sys_base != want_base) {
        if (!fail) std.debug.print("abi-check: FAIL\n", .{});
        std.debug.print(
            "  runtime-range drift: fig-sys FIG_FORMAT_RUNTIME_BASE is {d} but the format registry's runtime_abi_base is {d}\n",
            .{ sys_base, want_base },
        );
        fail = true;
    }

    // Vtable-version drift: fig.h's FIG_LANGUAGE_VTABLE_VERSION is what a host
    // writes into a FigLanguageVTable, and the registry refuses any other
    // value, so the macro and `runtime.vtable_version` must agree.
    const header_vt = macroInt(header, "FIG_LANGUAGE_VTABLE_VERSION") orelse return error.MissingVTableVersionMacro;
    if (header_vt != fig.Runtime.vtable_version) {
        if (!fail) std.debug.print("abi-check: FAIL\n", .{});
        std.debug.print(
            "  vtable-version drift: fig.h FIG_LANGUAGE_VTABLE_VERSION is {d} but runtime.vtable_version is {d}\n",
            .{ header_vt, fig.Runtime.vtable_version },
        );
        fail = true;
    }

    // Format-enum drift: fig.h's FIG_FORMAT_* enumerators must match the format
    // registry name-for-name and value-for-value — and so must each binding's
    // mirror of them.
    const formats = try parseEnumerators(arena, header, "typedef enum FigFormat", .values_required);
    try checkFormats(arena, "fig.h", formats, .c_macro, &fail);
    const sys_formats = try parseEnumerators(arena, sys_src, "pub enum FigFormat", .values_required);
    try checkFormats(arena, "fig-sys FigFormat", sys_formats, .pascal_valued, &fail);
    const ts_formats = try parseEnumerators(arena, ts_src, "export enum Format", .values_required);
    try checkFormats(arena, "TypeScript Format", ts_formats, .pascal_valued, &fail);
    const rust_formats = try parseEnumerators(arena, rust_src, "pub enum Format", .names_only);
    try checkFormats(arena, "Rust Format", rust_formats, .pascal_named, &fail);

    // Extended-scalar-kind drift: fig.h's FIG_EXT_* enumerators and the two
    // binding mirrors must match the core's `ExtKind` name-for-name, and
    // value-for-value where the surface states one.
    const ext_kinds = try parseEnumerators(arena, header, "typedef enum FigExtKind", .values_required);
    try checkExtKinds(arena, "fig.h", ext_kinds, .c_macro, &fail);
    const ts_ext_kinds = try parseEnumerators(arena, ts_src, "export enum ExtKind", .values_required);
    try checkExtKinds(arena, "TypeScript ExtKind", ts_ext_kinds, .pascal_valued, &fail);
    const rust_ext_kinds = try parseEnumerators(arena, rust_value_src, "pub enum ExtKind", .names_only);
    try checkExtKinds(arena, "Rust ExtKind", rust_ext_kinds, .pascal_named, &fail);

    if (fail) std.process.exit(1);
    std.debug.print("abi-check: symbol diff OK ({d} symbols), version {s}, ABI v{d}\n", .{ exported.len, want_version, want_abi_int });
    std.debug.print("abi-check: FIG_FORMAT_* enumerators OK ({d} formats) — fig.h, fig-sys, the TypeScript and Rust `Format` enums all match the format registry\n", .{formats.len});
    std.debug.print("abi-check: FIG_EXT_* enumerators OK ({d} kinds) — fig.h, the TypeScript and Rust `ExtKind` enums all match the core's ExtKind\n", .{ext_kinds.len});
}

/// One enumerator of a format enum: its name as spelled in that surface, and
/// its value where the surface states one.
const FormatEnumerator = struct { name: []const u8, value: ?i64 };

/// How a surface spells a registry entry, and whether its value is checked.
const NameStyle = enum {
    /// `FIG_FORMAT_<UPPER>` with a value — fig.h.
    c_macro,
    /// `<Pascal>` with a value — `fig-sys`'s `FigFormat`, the TypeScript `Format`.
    pascal_valued,
    /// `<Pascal>` and nothing else — the Rust wrapper's `Format`, whose ABI
    /// value lives in its `From` impl rather than on the variant.
    pascal_named,
};

/// Compare one surface's enumerators against the registry in both directions,
/// setting `fail` (and printing a line naming the surface, the enumerator and
/// both values) for each disagreement.
fn checkFormats(arena: std.mem.Allocator, surface: []const u8, formats: []const FormatEnumerator, style: NameStyle, fail: *bool) !void {
    // Registry -> surface: every format must be declared, with its exact value
    // where the surface carries one.
    inline for (fig.Language.dialects) |d| {
        const want_name = try enumeratorName(arena, d.name, style);
        if (findFormat(formats, want_name)) |e| {
            if (style != .pascal_named) {
                if (e.value) |got| {
                    if (got != @as(i64, d.abi_value)) {
                        if (!fail.*) std.debug.print("abi-check: FAIL\n", .{});
                        std.debug.print(
                            "  {s}: format value drift: {s} = {d} but the format registry gives '{s}' the ABI value {d}\n",
                            .{ surface, want_name, got, d.name, d.abi_value },
                        );
                        fail.* = true;
                    }
                } else {
                    if (!fail.*) std.debug.print("abi-check: FAIL\n", .{});
                    std.debug.print("  {s}: {s} states no value, but the format registry gives '{s}' the ABI value {d}\n", .{ surface, want_name, d.name, d.abi_value });
                    fail.* = true;
                }
            }
        } else {
            if (!fail.*) std.debug.print("abi-check: FAIL\n", .{});
            std.debug.print(
                "  {s}: missing enumerator: the format registry has '{s}' (ABI value {d}) but {s} is not declared\n",
                .{ surface, d.name, d.abi_value, want_name },
            );
            fail.* = true;
        }
    }
    // Surface -> registry: an enumerator no format claims is a value a caller
    // can pass that nothing implements.
    for (formats) |e| {
        var found = false;
        inline for (fig.Language.dialects) |d| {
            const want_name = try enumeratorName(arena, d.name, style);
            if (std.mem.eql(u8, want_name, e.name)) found = true;
        }
        if (!found) {
            if (!fail.*) std.debug.print("abi-check: FAIL\n", .{});
            std.debug.print(
                "  {s}: unknown enumerator: {s} matches no format-registry entry\n",
                .{ surface, e.name },
            );
            fail.* = true;
        }
    }
}

/// The core's extended-scalar kinds: the ABI value of each is its ordinal,
/// which is what `c_api.zig`'s `FigExtKind` pins and `fig_node_extended`
/// returns.
const ExtKind = fig.AST.Node.Kind.Extended.ExtKind;

/// `checkFormats` for `FigExtKind`: every core `ExtKind` member must be
/// declared on the surface, with its ordinal as the value where the surface
/// carries one, and the surface may declare nothing the core does not have.
fn checkExtKinds(arena: std.mem.Allocator, surface: []const u8, kinds: []const FormatEnumerator, style: NameStyle, fail: *bool) !void {
    inline for (@typeInfo(ExtKind).@"enum".fields) |f| {
        const want_name = try extKindName(arena, f.name, style);
        if (findFormat(kinds, want_name)) |e| {
            if (style != .pascal_named) {
                if (e.value) |got| {
                    if (got != @as(i64, f.value)) {
                        if (!fail.*) std.debug.print("abi-check: FAIL\n", .{});
                        std.debug.print(
                            "  {s}: ext-kind value drift: {s} = {d} but the core's ExtKind gives '{s}' the value {d}\n",
                            .{ surface, want_name, got, f.name, f.value },
                        );
                        fail.* = true;
                    }
                } else {
                    if (!fail.*) std.debug.print("abi-check: FAIL\n", .{});
                    std.debug.print("  {s}: {s} states no value, but the core's ExtKind gives '{s}' the value {d}\n", .{ surface, want_name, f.name, f.value });
                    fail.* = true;
                }
            }
        } else {
            if (!fail.*) std.debug.print("abi-check: FAIL\n", .{});
            std.debug.print(
                "  {s}: missing enumerator: the core's ExtKind has '{s}' (value {d}) but {s} is not declared\n",
                .{ surface, f.name, f.value, want_name },
            );
            fail.* = true;
        }
    }
    for (kinds) |e| {
        var found = false;
        inline for (@typeInfo(ExtKind).@"enum".fields) |f| {
            const want_name = try extKindName(arena, f.name, style);
            if (std.mem.eql(u8, want_name, e.name)) found = true;
        }
        if (!found) {
            if (!fail.*) std.debug.print("abi-check: FAIL\n", .{});
            std.debug.print("  {s}: unknown enumerator: {s} matches no core ExtKind member\n", .{ surface, e.name });
            fail.* = true;
        }
    }
}

/// The spelling of a core `ExtKind` member on a given surface:
/// `FIG_EXT_OFFSET_DATETIME` in C; `OffsetDateTime` in Rust and TypeScript,
/// where the snake_case member name is upper-camel-cased at each underscore
/// — except `datetime`, which both bindings spell `DateTime`.
fn extKindName(arena: std.mem.Allocator, name: []const u8, style: NameStyle) ![]const u8 {
    switch (style) {
        .c_macro => {
            const out = try arena.alloc(u8, "FIG_EXT_".len + name.len);
            @memcpy(out[0.."FIG_EXT_".len], "FIG_EXT_");
            _ = std.ascii.upperString(out["FIG_EXT_".len..], name);
            return out;
        },
        .pascal_valued, .pascal_named => {
            var out: std.ArrayList(u8) = .empty;
            var up = true;
            var i: usize = 0;
            while (i < name.len) : (i += 1) {
                const c = name[i];
                if (c == '_') {
                    up = true;
                    continue;
                }
                if (std.mem.startsWith(u8, name[i..], "datetime")) {
                    try out.appendSlice(arena, "DateTime");
                    i += "datetime".len - 1;
                    up = false;
                    continue;
                }
                try out.append(arena, if (up) std.ascii.toUpper(c) else c);
                up = false;
            }
            return out.items;
        },
    }
}

/// The spelling of a registry entry name on a given surface — the one place
/// each naming convention is written down. `FIG_FORMAT_JSON5` in C;
/// `Json5` (first letter up, the rest as the registry has it) in Rust and
/// TypeScript.
fn enumeratorName(arena: std.mem.Allocator, name: []const u8, style: NameStyle) ![]const u8 {
    switch (style) {
        .c_macro => {
            const out = try arena.alloc(u8, "FIG_FORMAT_".len + name.len);
            @memcpy(out[0.."FIG_FORMAT_".len], "FIG_FORMAT_");
            _ = std.ascii.upperString(out["FIG_FORMAT_".len..], name);
            return out;
        },
        .pascal_valued, .pascal_named => {
            const out = try arena.dupe(u8, name);
            if (out.len > 0) out[0] = std.ascii.toUpper(out[0]);
            return out;
        },
    }
}

fn findFormat(formats: []const FormatEnumerator, name: []const u8) ?FormatEnumerator {
    for (formats) |e| if (std.mem.eql(u8, e.name, name)) return e;
    return null;
}

const ValueRule = enum { values_required, names_only };

/// The enumerators of the enum whose declaration starts with `decl` in `text`,
/// in declaration order — `Name = value,` or, under `.names_only`, bare
/// `Name,`.
///
/// Adapted from `parseEnumerators` in tools/semver-check.zig, minus the parts
/// this doesn't need: a valued surface gives every member an explicit decimal
/// value (that is the ABI contract), so there is no implicit-value running
/// counter and no expression evaluation — a member without a literal integer
/// is itself an error worth reporting rather than something to infer.
fn parseEnumerators(arena: std.mem.Allocator, text: []const u8, decl: []const u8, rule: ValueRule) ![]const FormatEnumerator {
    const at = std.mem.indexOf(u8, text, decl) orelse return error.MissingFormatEnum;
    const open = std.mem.indexOfScalarPos(u8, text, at, '{') orelse return error.MissingFormatEnum;

    // Comments are removed on the way to the closing brace, not after it is
    // found: most members carry a prose comment, and prose contains commas and
    // braces (`{@link Format.Dotenv}` in the TypeScript doc comments), so
    // splitting on `,` or stopping at `}` before removing them would cut a
    // sentence in half or end the enum early. Rust attributes on a variant
    // (`#[doc(hidden)]`, …) are not members and are dropped with the comments.
    const code = try enumBody(arena, text[open + 1 ..]);

    var list: std.ArrayList(FormatEnumerator) = .empty;
    var it = std.mem.splitScalar(u8, code, ',');
    while (it.next()) |raw| {
        const item = std.mem.trim(u8, raw, " \t");
        if (item.len == 0) continue;
        if (std.mem.indexOfScalar(u8, item, '=')) |eq| {
            const name = std.mem.trim(u8, item[0..eq], " \t");
            const value = std.fmt.parseInt(i64, std.mem.trim(u8, item[eq + 1 ..], " \t"), 10) catch
                return error.MalformedFormatEnum;
            try list.append(arena, .{ .name = name, .value = value });
        } else {
            if (rule == .values_required) return error.MalformedFormatEnum;
            try list.append(arena, .{ .name = item, .value = null });
        }
    }
    return list.items;
}

/// The code of an enum body — `text` starts just after its `{` — up to its
/// closing `}`, with `//` line comments, `/* … */` block comments and `#[…]`
/// attribute lines removed. The brace is looked for only outside a comment.
fn enumBody(arena: std.mem.Allocator, text: []const u8) ![]const u8 {
    var out: std.ArrayList(u8) = .empty;
    var i: usize = 0;
    var at_line_start = true;
    while (i < text.len) {
        if (std.mem.startsWith(u8, text[i..], "//")) {
            i = std.mem.indexOfScalarPos(u8, text, i, '\n') orelse text.len;
            continue;
        }
        if (std.mem.startsWith(u8, text[i..], "/*")) {
            const end = std.mem.indexOfPos(u8, text, i + 2, "*/") orelse return error.MalformedFormatEnum;
            i = end + 2;
            continue;
        }
        if (at_line_start and std.mem.startsWith(u8, std.mem.trimStart(u8, text[i..], " \t"), "#[")) {
            i = std.mem.indexOfScalarPos(u8, text, i, '\n') orelse text.len;
            continue;
        }
        const c = text[i];
        if (c == '}') return out.items;
        at_line_start = c == '\n';
        if (c != '\n' and c != '\r') try out.append(arena, c);
        i += 1;
    }
    return error.MissingFormatEnum;
}

/// The `major.minor.patch` spelled by the header's `#define FIG_VERSION_*` lines.
fn headerVersion(arena: std.mem.Allocator, header: []const u8) ![]const u8 {
    const major = macroInt(header, "FIG_VERSION_MAJOR") orelse return error.MissingVersionMacro;
    const minor = macroInt(header, "FIG_VERSION_MINOR") orelse return error.MissingVersionMacro;
    const patch = macroInt(header, "FIG_VERSION_PATCH") orelse return error.MissingVersionMacro;
    return std.fmt.allocPrint(arena, "{d}.{d}.{d}", .{ major, minor, patch });
}

/// The integer defined by `#define <name> <int>` in `text`, or null if absent.
fn macroInt(text: []const u8, name: []const u8) ?u32 {
    var lines = std.mem.splitScalar(u8, text, '\n');
    while (lines.next()) |line| {
        const trimmed = std.mem.trimStart(u8, line, " \t");
        if (!std.mem.startsWith(u8, trimmed, "#define")) continue;
        var it = std.mem.tokenizeAny(u8, trimmed, " \t");
        _ = it.next(); // #define
        const macro = it.next() orelse continue;
        if (!std.mem.eql(u8, macro, name)) continue;
        const value = it.next() orelse return null;
        return std.fmt.parseInt(u32, value, 10) catch null;
    }
    return null;
}

/// The value of a Rust `pub const <name>: <type> = <int>;` item, or null if
/// no line declares one.
fn rustConstInt(text: []const u8, name: []const u8) ?i64 {
    var lines = std.mem.splitScalar(u8, text, '\n');
    while (lines.next()) |line| {
        const trimmed = std.mem.trimStart(u8, line, " \t");
        if (!std.mem.startsWith(u8, trimmed, "pub const ")) continue;
        const rest = trimmed["pub const ".len..];
        if (!std.mem.startsWith(u8, rest, name)) continue;
        if (rest.len == name.len or rest[name.len] != ':') continue;
        const eq = std.mem.indexOfScalar(u8, rest, '=') orelse continue;
        const semi = std.mem.indexOfScalar(u8, rest, ';') orelse continue;
        if (semi <= eq) continue;
        const value = std.mem.trim(u8, rest[eq + 1 .. semi], " \t");
        return std.fmt.parseInt(i64, value, 10) catch null;
    }
    return null;
}

fn isNameChar(c: u8) bool {
    return (c >= 'a' and c <= 'z') or (c >= '0' and c <= '9') or c == '_';
}

/// Header prototypes: a `fig_<name>(` token (the `(` distinguishes a declaration
/// or call from a bare prose mention) on a line that is not a `//` comment.
fn collectDeclared(arena: std.mem.Allocator, text: []const u8) ![]const []const u8 {
    var list: std.ArrayList([]const u8) = .empty;
    var lines = std.mem.splitScalar(u8, text, '\n');
    while (lines.next()) |line| {
        const trimmed = std.mem.trimStart(u8, line, " \t");
        if (std.mem.startsWith(u8, trimmed, "//")) continue;
        var i: usize = 0;
        while (std.mem.indexOfPos(u8, line, i, "fig_")) |pos| {
            // Skip a `fig_` that is the tail of a longer identifier.
            if (pos > 0 and isNameChar(line[pos - 1])) {
                i = pos + 4;
                continue;
            }
            var end = pos + 4;
            while (end < line.len and isNameChar(line[end])) end += 1;
            if (end < line.len and line[end] == '(') {
                try list.append(arena, line[pos..end]);
            }
            i = end;
        }
    }
    return sortDedup(arena, &list);
}

/// Implementation exports: the name following each `export fn ` marker, kept when
/// it starts with `fig_`.
fn collectExported(arena: std.mem.Allocator, text: []const u8) ![]const []const u8 {
    var list: std.ArrayList([]const u8) = .empty;
    const marker = "export fn ";
    var i: usize = 0;
    while (std.mem.indexOfPos(u8, text, i, marker)) |pos| {
        const start = pos + marker.len;
        var end = start;
        while (end < text.len and isNameChar(text[end])) end += 1;
        const name = text[start..end];
        if (std.mem.startsWith(u8, name, "fig_")) try list.append(arena, name);
        i = end;
    }
    return sortDedup(arena, &list);
}

fn lessThanStr(_: void, a: []const u8, b: []const u8) bool {
    return std.mem.lessThan(u8, a, b);
}

fn sortDedup(arena: std.mem.Allocator, list: *std.ArrayList([]const u8)) ![]const []const u8 {
    std.mem.sort([]const u8, list.items, {}, lessThanStr);
    var out: std.ArrayList([]const u8) = .empty;
    for (list.items, 0..) |name, idx| {
        if (idx > 0 and std.mem.eql(u8, name, list.items[idx - 1])) continue;
        try out.append(arena, name);
    }
    return out.items;
}

fn contains(haystack: []const []const u8, needle: []const u8) bool {
    for (haystack) |s| if (std.mem.eql(u8, s, needle)) return true;
    return false;
}
