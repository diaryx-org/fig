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
//! Usage (driven by build.zig): abi-check <header.h> <impl.zig> <major.minor.patch> <abi-version>

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

    const cwd = std.Io.Dir.cwd();
    const header = try cwd.readFileAlloc(io, header_path, arena, .limited(max_file));
    const impl = try cwd.readFileAlloc(io, impl_path, arena, .limited(max_file));

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

    // Format-enum drift: fig.h's FIG_FORMAT_* enumerators must match the format
    // registry name-for-name and value-for-value.
    const formats = try parseFormatEnumerators(arena, header);
    try checkFormats(arena, formats, &fail);

    if (fail) std.process.exit(1);
    std.debug.print("abi-check: symbol diff OK ({d} symbols), version {s}, ABI v{d}\n", .{ exported.len, want_version, want_abi_int });
    std.debug.print("abi-check: FIG_FORMAT_* enumerators OK ({d} formats) — fig.h matches the format registry\n", .{formats.len});
}

/// One `FIG_FORMAT_<NAME> = <value>` line from the header.
const FormatEnumerator = struct { name: []const u8, value: i64 };

/// Compare the header's enumerators against the registry in both directions,
/// setting `fail` (and printing a line naming the enumerator and both values)
/// for each disagreement.
fn checkFormats(arena: std.mem.Allocator, formats: []const FormatEnumerator, fail: *bool) !void {
    // Registry -> header: every format must be declared, with its exact value.
    inline for (fig.Language.dialects) |d| {
        const want_name = try enumeratorName(arena, d.name);
        if (findFormat(formats, want_name)) |e| {
            if (e.value != @as(i64, d.abi_value)) {
                if (!fail.*) std.debug.print("abi-check: FAIL\n", .{});
                std.debug.print(
                    "  format value drift: fig.h has {s} = {d} but the format registry gives '{s}' the ABI value {d}\n",
                    .{ want_name, e.value, d.name, d.abi_value },
                );
                fail.* = true;
            }
        } else {
            if (!fail.*) std.debug.print("abi-check: FAIL\n", .{});
            std.debug.print(
                "  missing enumerator: the format registry has '{s}' (ABI value {d}) but fig.h declares no {s}\n",
                .{ d.name, d.abi_value, want_name },
            );
            fail.* = true;
        }
    }
    // Header -> registry: an enumerator no format claims is a value a caller can
    // pass that nothing implements.
    for (formats) |e| {
        var found = false;
        inline for (fig.Language.dialects) |d| {
            const want_name = try enumeratorName(arena, d.name);
            if (std.mem.eql(u8, want_name, e.name)) found = true;
        }
        if (!found) {
            if (!fail.*) std.debug.print("abi-check: FAIL\n", .{});
            std.debug.print(
                "  unknown enumerator: fig.h declares {s} = {d}, which matches no format-registry entry\n",
                .{ e.name, e.value },
            );
            fail.* = true;
        }
    }
}

/// The `FIG_FORMAT_<NAME>` spelling of a registry entry name — the one place
/// the naming convention between the two surfaces is written down.
fn enumeratorName(arena: std.mem.Allocator, name: []const u8) ![]const u8 {
    const out = try arena.alloc(u8, "FIG_FORMAT_".len + name.len);
    @memcpy(out[0.."FIG_FORMAT_".len], "FIG_FORMAT_");
    _ = std.ascii.upperString(out["FIG_FORMAT_".len..], name);
    return out;
}

fn findFormat(formats: []const FormatEnumerator, name: []const u8) ?FormatEnumerator {
    for (formats) |e| if (std.mem.eql(u8, e.name, name)) return e;
    return null;
}

/// The enumerators of the header's `FigFormat` enum, in declaration order.
///
/// Adapted from `parseEnumerators` in tools/semver-check.zig, minus the parts
/// this doesn't need: `FigFormat` gives every member an explicit decimal value
/// (that is the ABI contract), so there is no implicit-value running counter
/// and no expression evaluation — a member without a literal integer is itself
/// an error worth reporting rather than something to infer.
fn parseFormatEnumerators(arena: std.mem.Allocator, header: []const u8) ![]const FormatEnumerator {
    const decl = "typedef enum FigFormat";
    const at = std.mem.indexOf(u8, header, decl) orelse return error.MissingFigFormatEnum;
    const open = std.mem.indexOfScalarPos(u8, header, at, '{') orelse return error.MissingFigFormatEnum;
    const close = std.mem.indexOfScalarPos(u8, header, open, '}') orelse return error.MissingFigFormatEnum;

    // Strip `//` comments FIRST, whole-body: most members carry a prose comment,
    // and prose contains commas — splitting on `,` before removing it would cut
    // a sentence in half and hand the tail to the next enumerator.
    var code: std.ArrayList(u8) = .empty;
    var lines = std.mem.splitScalar(u8, header[open + 1 .. close], '\n');
    while (lines.next()) |line| {
        const bare = if (std.mem.indexOf(u8, line, "//")) |c| line[0..c] else line;
        try code.appendSlice(arena, std.mem.trim(u8, bare, " \t\r"));
    }

    var list: std.ArrayList(FormatEnumerator) = .empty;
    var it = std.mem.splitScalar(u8, code.items, ',');
    while (it.next()) |raw| {
        const item = std.mem.trim(u8, raw, " \t");
        if (item.len == 0) continue;
        const eq = std.mem.indexOfScalar(u8, item, '=') orelse return error.MalformedFigFormatEnum;
        const name = std.mem.trim(u8, item[0..eq], " \t");
        const value = std.fmt.parseInt(i64, std.mem.trim(u8, item[eq + 1 ..], " \t"), 10) catch
            return error.MalformedFigFormatEnum;
        try list.append(arena, .{ .name = name, .value = value });
    }
    return list.items;
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
