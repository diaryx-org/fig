//! Dev tool: copy fig's one version out of `build.zig.zon` into every other
//! file that carries it, or (`--check`) fail if any of them disagrees.
//!
//! fig ships every artifact — the core, the CLI, the Rust crates, `@diaryx/fig`
//! and `@diaryx/fig-wasi` — under one version and one `v*` tag (see
//! docs/VERSIONING.md). The version is decided in `build.zig.zon`'s `.version`;
//! `dx release` moves it there and in `bindings/rust/Cargo.toml` (its
//! `mirrors`), then runs `zig build version-sync` as its `post_bump` for the
//! files no Cargo-manifest rewrite reaches. `zig build version-check` is the
//! read-only half, and `zig build check` runs it.
//!
//! The files, and the fields in each:
//!   * `figl/build.zig.figl`          `version = X.Y.Z` — the source build.zig.zon
//!                                     is generated from, so `check-figl` agrees
//!   * `bindings/c/include/fig.h`     `FIG_VERSION_MAJOR` / `_MINOR` / `_PATCH`
//!   * `README.md`                    the frontmatter's `version`
//!   * `bindings/rust/Cargo.toml`     `[workspace.package] version` and every
//!                                     internal `{ path, version }` pin
//!   * `bindings/typescript/package.json` and `bindings/wasi/package.json`
//!   * both `package-lock.json` files — the root `version` and `packages[""]`
//!
//! `FIG_ABI_VERSION` / `abi_version` is deliberately not here: it is the C ABI's
//! own contract integer, bumped only on a breaking ABI change, and it has no tag.
//!
//! Scanners rather than parsers, for the reason every build tool here gives:
//! fig cannot bootstrap-parse its own build manifests, and each field is one
//! line a `std.mem` search finds exactly. A value is spliced in place, so every
//! byte around it survives.
//!
//! Usage (driven by build.zig): version-sync <repo-root> [--check]

const std = @import("std");
const Dir = std.Io.Dir;

const max_file = 4 * 1024 * 1024;

/// Where the version is decided.
const zon_rel = "build.zig.zon";

/// Every file the version is copied into, in the order they are reported.
const targets = [_][]const u8{
    "figl/build.zig.figl",
    "bindings/c/include/fig.h",
    "README.md",
    "bindings/rust/Cargo.toml",
    "bindings/typescript/package.json",
    "bindings/typescript/package-lock.json",
    "bindings/wasi/package.json",
    "bindings/wasi/package-lock.json",
};

/// A half-open `[start, end)` byte range into the text a locator was given.
pub const Range = struct { start: usize, end: usize };

/// One field: where its value sits, and what it should read.
pub const Edit = struct { range: Range, value: []const u8, what: []const u8 };

pub fn main(init: std.process.Init) !void {
    const io = init.io;
    const arena = init.arena.allocator();

    var args = try init.minimal.args.iterateAllocator(init.gpa);
    defer args.deinit();
    _ = args.next(); // argv0
    const repo_root = args.next() orelse fail("usage: version-sync <repo-root> [--check]", .{});
    var check_only = false;
    while (args.next()) |arg| {
        if (std.mem.eql(u8, arg, "--check")) {
            check_only = true;
        } else fail("unknown argument `{s}` (expected --check)", .{arg});
    }

    const cwd = Dir.cwd();
    const zon = try readRel(io, arena, cwd, repo_root, zon_rel);
    const zon_range = zonVersionRange(zon) orelse fail("no `.version` in {s}", .{zon_rel});
    const version_str = zon[zon_range.start..zon_range.end];
    const version = std.SemanticVersion.parse(version_str) catch
        fail("`.version` in {s} is `{s}`, which is not a SemVer version", .{ zon_rel, version_str });

    var failed = false;
    var cargo_moved = false;
    for (targets) |rel| {
        const text = try readRel(io, arena, cwd, repo_root, rel);
        const edits = plan(arena, rel, text, version, version_str) catch |err| switch (err) {
            error.FieldNotFound => {
                std.debug.print("version-sync: FAIL    {s}: a version field is missing — the scanner could not find it\n", .{rel});
                failed = true;
                continue;
            },
            else => return err,
        };

        var stale: usize = 0;
        for (edits) |e| {
            const have = text[e.range.start..e.range.end];
            if (!std.mem.eql(u8, have, e.value)) {
                stale += 1;
                if (check_only)
                    std.debug.print("version-sync: STALE   {s}: {s} is {s}, want {s}\n", .{ rel, e.what, have, e.value });
            }
        }

        if (stale == 0) {
            std.debug.print("version-sync: ok      {s}\n", .{rel});
            continue;
        }
        if (check_only) {
            failed = true;
            continue;
        }
        try writeRel(io, arena, cwd, repo_root, rel, try splice(arena, text, edits));
        std.debug.print("version-sync: updated {s} ({d} field{s} -> {s})\n", .{ rel, stale, if (stale == 1) "" else "s", version_str });
        if (std.mem.eql(u8, rel, "bindings/rust/Cargo.toml")) cargo_moved = true;
    }

    if (failed) {
        std.debug.print("version-sync: build.zig.zon says {s}; run `zig build version-sync` to copy it everywhere\n", .{version_str});
        std.process.exit(1);
    }
    if (cargo_moved)
        std.debug.print("version-sync: now refresh the lockfile: (cd bindings/rust && cargo update --workspace)\n", .{});
}

/// Every version field in the file at `rel`, with the value it should hold.
pub fn plan(
    arena: std.mem.Allocator,
    rel: []const u8,
    text: []const u8,
    version: std.SemanticVersion,
    version_str: []const u8,
) ![]const Edit {
    var out: std.ArrayList(Edit) = .empty;
    if (std.mem.endsWith(u8, rel, ".figl")) {
        try out.append(arena, .{ .range = figlVersionRange(text, 0, text.len) orelse return error.FieldNotFound, .value = version_str, .what = "version" });
    } else if (std.mem.endsWith(u8, rel, "fig.h")) {
        const parts = [_]struct { []const u8, usize }{
            .{ "FIG_VERSION_MAJOR", version.major },
            .{ "FIG_VERSION_MINOR", version.minor },
            .{ "FIG_VERSION_PATCH", version.patch },
        };
        for (parts) |p| {
            try out.append(arena, .{
                .range = cMacroIntRange(text, p[0]) orelse return error.FieldNotFound,
                .value = try std.fmt.allocPrint(arena, "{d}", .{p[1]}),
                .what = p[0],
            });
        }
    } else if (std.mem.endsWith(u8, rel, "README.md")) {
        try out.append(arena, .{ .range = frontmatterVersionRange(text) orelse return error.FieldNotFound, .value = version_str, .what = "frontmatter version" });
    } else if (std.mem.endsWith(u8, rel, "Cargo.toml")) {
        try out.append(arena, .{ .range = cargoWorkspaceVersionRange(text) orelse return error.FieldNotFound, .value = version_str, .what = "[workspace.package] version" });
        var pins: std.ArrayList(Range) = .empty;
        if (try cargoInternalPinRanges(text, arena, &pins) == 0) return error.FieldNotFound;
        for (pins.items) |r| try out.append(arena, .{ .range = r, .value = version_str, .what = "an internal dependency pin" });
    } else if (std.mem.endsWith(u8, rel, "package-lock.json")) {
        const lock = packageLockVersionRanges(text) orelse return error.FieldNotFound;
        try out.append(arena, .{ .range = lock[0], .value = version_str, .what = "version" });
        try out.append(arena, .{ .range = lock[1], .value = version_str, .what = "packages[\"\"].version" });
    } else if (std.mem.endsWith(u8, rel, "package.json")) {
        try out.append(arena, .{ .range = jsonVersionRange(text, 0) orelse return error.FieldNotFound, .value = version_str, .what = "version" });
    } else unreachable;
    return out.items;
}

/// `text` with every edit's range replaced by its value. The ranges come from
/// distinct fields and never overlap; they are applied in file order.
pub fn splice(arena: std.mem.Allocator, text: []const u8, edits: []const Edit) ![]u8 {
    const sorted = try arena.dupe(Edit, edits);
    std.mem.sort(Edit, sorted, {}, struct {
        fn lt(_: void, a: Edit, b: Edit) bool {
            return a.range.start < b.range.start;
        }
    }.lt);
    var out: std.ArrayList(u8) = .empty;
    var pos: usize = 0;
    for (sorted) |e| {
        try out.appendSlice(arena, text[pos..e.range.start]);
        try out.appendSlice(arena, e.value);
        pos = e.range.end;
    }
    try out.appendSlice(arena, text[pos..]);
    return out.items;
}

// --- Locators. Each returns the byte range of a field's VALUE (between the
// quotes, where it has them), so a splice leaves every byte around it alone.

/// The contents of the next double-quoted string at or after `idx`.
fn quotedRangeAfter(text: []const u8, idx: usize) ?Range {
    const open = std.mem.indexOfScalarPos(u8, text, idx, '"') orelse return null;
    const close = std.mem.indexOfScalarPos(u8, text, open + 1, '"') orelse return null;
    return .{ .start = open + 1, .end = close };
}

/// `.version = "…"` in a build.zig.zon. `.minimum_zig_version` carries `_version`,
/// never `.version`, so the first match is the package's own.
pub fn zonVersionRange(text: []const u8) ?Range {
    const at = std.mem.indexOf(u8, text, ".version") orelse return null;
    return quotedRangeAfter(text, at + ".version".len);
}

/// The bare value of the first `version = X.Y.Z` line in `text[from..to]` — a
/// `.figl` source, or the `fig` frontmatter block of a markdown file. The key
/// has to be exactly `version`, so `minimum_zig_version = …` is never taken for
/// it; the value runs to the first space, `#` comment, or end of line.
pub fn figlVersionRange(text: []const u8, from: usize, to: usize) ?Range {
    var i = from;
    while (i < to) {
        const nl = std.mem.indexOfScalarPos(u8, text[0..to], i, '\n') orelse to;
        const line = text[i..nl];
        const lead = line.len - std.mem.trimStart(u8, line, " \t").len;
        const rest = line[lead..];
        if (std.mem.startsWith(u8, rest, "version")) {
            const after = std.mem.trimStart(u8, rest["version".len..], " \t");
            if (after.len > 0 and after[0] == '=') {
                var s = nl - after.len + 1;
                while (s < nl and (text[s] == ' ' or text[s] == '\t')) s += 1;
                var e = s;
                while (e < nl and text[e] != ' ' and text[e] != '\t' and text[e] != '#' and text[e] != '\r') e += 1;
                if (e > s) return .{ .start = s, .end = e };
            }
        }
        i = nl + 1;
    }
    return null;
}

/// The `version` inside a markdown file's leading ```` ```fig ```` frontmatter
/// block — and only there, so a `version = …` in a code sample further down the
/// README is never mistaken for it.
pub fn frontmatterVersionRange(text: []const u8) ?Range {
    const fence = "```fig\n";
    if (!std.mem.startsWith(u8, text, fence)) return null;
    const close = std.mem.indexOfPos(u8, text, fence.len, "\n```") orelse return null;
    return figlVersionRange(text, fence.len, close + 1);
}

/// The integer of a `#define <macro> <int>` line. Anchors on the first
/// occurrence of `macro` followed by digits — the `#define` precedes every use
/// in `FIG_VERSION_NUM`, and those uses have no digits after the name.
pub fn cMacroIntRange(text: []const u8, macro: []const u8) ?Range {
    var from: usize = 0;
    while (std.mem.indexOfPos(u8, text, from, macro)) |at| {
        var s = at + macro.len;
        while (s < text.len and (text[s] == ' ' or text[s] == '\t')) s += 1;
        var e = s;
        while (e < text.len and text[e] >= '0' and text[e] <= '9') e += 1;
        if (e > s) return .{ .start = s, .end = e };
        from = at + macro.len;
    }
    return null;
}

/// The first `version = "…"` under `[workspace.package]`, before the next
/// section header.
pub fn cargoWorkspaceVersionRange(text: []const u8) ?Range {
    const sec = std.mem.indexOf(u8, text, "[workspace.package]") orelse return null;
    var i = (std.mem.indexOfScalarPos(u8, text, sec, '\n') orelse return null) + 1;
    while (i < text.len) {
        const nl = std.mem.indexOfScalarPos(u8, text, i, '\n') orelse text.len;
        const trimmed = std.mem.trimStart(u8, text[i..nl], " \t");
        if (std.mem.startsWith(u8, trimmed, "[")) return null;
        if (std.mem.startsWith(u8, trimmed, "version")) {
            const eq = std.mem.indexOfScalarPos(u8, text[0..nl], i, '=') orelse return null;
            return quotedRangeAfter(text[0..nl], eq + 1);
        }
        i = nl + 1;
    }
    return null;
}

/// The `version = "…"` of every `name = { path = "…", version = "…" }` line
/// under `[workspace.dependencies]` — the crates of this workspace, which all
/// share the one version. External dependencies carry no `path` and are skipped.
pub fn cargoInternalPinRanges(text: []const u8, arena: std.mem.Allocator, out: *std.ArrayList(Range)) !usize {
    const sec = std.mem.indexOf(u8, text, "[workspace.dependencies]") orelse return 0;
    var i = (std.mem.indexOfScalarPos(u8, text, sec, '\n') orelse return 0) + 1;
    var count: usize = 0;
    while (i < text.len) {
        const nl = std.mem.indexOfScalarPos(u8, text, i, '\n') orelse text.len;
        const line = text[i..nl];
        if (std.mem.startsWith(u8, std.mem.trimStart(u8, line, " \t"), "[")) break;
        if (std.mem.indexOf(u8, line, "path = \"") != null) {
            if (std.mem.indexOfPos(u8, text[0..nl], i, "version")) |ver| {
                if (std.mem.indexOfScalarPos(u8, text[0..nl], ver, '=')) |veq| {
                    if (quotedRangeAfter(text[0..nl], veq + 1)) |r| {
                        try out.append(arena, r);
                        count += 1;
                    }
                }
            }
        }
        i = nl + 1;
    }
    return count;
}

/// The string after the first `"version"` key at or after `from` in a JSON file.
pub fn jsonVersionRange(text: []const u8, from: usize) ?Range {
    const at = std.mem.indexOfPos(u8, text, from, "\"version\"") orelse return null;
    const colon = std.mem.indexOfScalarPos(u8, text, at + "\"version\"".len, ':') orelse return null;
    return quotedRangeAfter(text, colon + 1);
}

/// A package-lock.json's two copies of the package's own version: the root
/// `version`, and the one under `packages[""]` — the first `"version"` after
/// the `""` key opens.
pub fn packageLockVersionRanges(text: []const u8) ?[2]Range {
    const root = jsonVersionRange(text, 0) orelse return null;
    const packages = std.mem.indexOf(u8, text, "\"packages\"") orelse return null;
    const self = std.mem.indexOfPos(u8, text, packages, "\"\":") orelse return null;
    const inner = jsonVersionRange(text, self) orelse return null;
    return .{ root, inner };
}

fn readRel(io: std.Io, arena: std.mem.Allocator, cwd: Dir, root: []const u8, rel: []const u8) ![]u8 {
    const path = try std.fs.path.join(arena, &.{ root, rel });
    return cwd.readFileAlloc(io, path, arena, .limited(max_file));
}

fn writeRel(io: std.Io, arena: std.mem.Allocator, cwd: Dir, root: []const u8, rel: []const u8, contents: []const u8) !void {
    const path = try std.fs.path.join(arena, &.{ root, rel });
    const file = try cwd.createFile(io, path, .{ .read = true });
    defer file.close(io);
    try file.writePositionalAll(io, contents, 0);
    try file.setLength(io, contents.len);
}

fn fail(comptime fmt: []const u8, args: anytype) noreturn {
    std.debug.print("version-sync: " ++ fmt ++ "\n", args);
    std.process.exit(2);
}

// --- Tests. `zig build test` runs these; the tool's `main` is not run.

fn applied(rel: []const u8, text: []const u8, v: []const u8) ![]u8 {
    var state = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer state.deinit();
    const a = state.allocator();
    const edits = try plan(a, rel, text, try std.SemanticVersion.parse(v), v);
    return std.testing.allocator.dupe(u8, try splice(a, text, edits));
}

test "figl: the package version, not minimum_zig_version" {
    const got = try applied("figl/build.zig.figl", "name: enum = fig\nversion = 3.1.0\nminimum_zig_version = 0.16.0\n", "5.0.0");
    defer std.testing.allocator.free(got);
    try std.testing.expectEqualStrings("name: enum = fig\nversion = 5.0.0\nminimum_zig_version = 0.16.0\n", got);
}

test "fig.h: the three macros, and not their uses" {
    const src =
        \\#define FIG_VERSION_MAJOR 3
        \\#define FIG_VERSION_MINOR 1
        \\#define FIG_VERSION_PATCH 0
        \\#define FIG_VERSION_NUM (((uint32_t)FIG_VERSION_MAJOR << 16) | FIG_VERSION_PATCH)
        \\#define FIG_ABI_VERSION 2
        \\
    ;
    const want =
        \\#define FIG_VERSION_MAJOR 10
        \\#define FIG_VERSION_MINOR 2
        \\#define FIG_VERSION_PATCH 3
        \\#define FIG_VERSION_NUM (((uint32_t)FIG_VERSION_MAJOR << 16) | FIG_VERSION_PATCH)
        \\#define FIG_ABI_VERSION 2
        \\
    ;
    const got = try applied("bindings/c/include/fig.h", src, "10.2.3");
    defer std.testing.allocator.free(got);
    try std.testing.expectEqualStrings(want, got);
}

test "README: the frontmatter only" {
    const src = "```fig\ntitle = fig\nversion = 3.1.0\n```\n\n```fig\nversion = 1.0.0\n```\n";
    const got = try applied("README.md", src, "5.0.0");
    defer std.testing.allocator.free(got);
    try std.testing.expectEqualStrings("```fig\ntitle = fig\nversion = 5.0.0\n```\n\n```fig\nversion = 1.0.0\n```\n", got);
}

test "Cargo.toml: the workspace version and internal pins, not external deps" {
    const src =
        \\[workspace]
        \\resolver = "3"
        \\
        \\[workspace.package]
        \\version = "4.1.0"
        \\
        \\[workspace.dependencies]
        \\fig-sys = { path = "fig-sys", version = "4.1.0", default-features = false }
        \\syn = { version = "2", features = ["full"] }
        \\
    ;
    const want =
        \\[workspace]
        \\resolver = "3"
        \\
        \\[workspace.package]
        \\version = "5.0.0"
        \\
        \\[workspace.dependencies]
        \\fig-sys = { path = "fig-sys", version = "5.0.0", default-features = false }
        \\syn = { version = "2", features = ["full"] }
        \\
    ;
    const got = try applied("bindings/rust/Cargo.toml", src, "5.0.0");
    defer std.testing.allocator.free(got);
    try std.testing.expectEqualStrings(want, got);
}

test "package-lock.json: both copies of the package's own version" {
    const src =
        \\{
        \\  "name": "@diaryx/fig",
        \\  "version": "3.1.0",
        \\  "packages": {
        \\    "": {
        \\      "name": "@diaryx/fig",
        \\      "version": "3.1.0",
        \\      "devDependencies": { "typescript": "^5" }
        \\    },
        \\    "node_modules/typescript": { "version": "5.9.0" }
        \\  }
        \\}
    ;
    const got = try applied("bindings/typescript/package-lock.json", src, "5.0.0");
    defer std.testing.allocator.free(got);
    try std.testing.expect(std.mem.count(u8, got, "\"version\": \"5.0.0\"") == 2);
    try std.testing.expect(std.mem.indexOf(u8, got, "\"version\": \"5.9.0\"") != null);
}
