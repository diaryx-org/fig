//! Dev tool: vendor fig's Zig source into the Rust crate so the published crate
//! is self-contained.
//!
//! The `fig-sys` crate compiles the fig core from source with `zig build` (see
//! bindings/rust/fig-sys/build.rs) as a fallback. In a checkout that source is
//! found by walking up to the repo root, but a crate published to crates.io
//! cannot reach outside its own directory — so before packaging we copy the
//! minimal source set into bindings/rust/fig-sys/zig, which Cargo.toml's
//! `include` force-adds to the tarball.
//!
//! This is the cross-platform replacement for a shell `cp`: it runs through the
//! same Zig toolchain the crate already requires, so it works on Windows too.
//!
//! Run via `zig build vendor-rust`. Driven by build.zig as:
//!   vendor-rust <src-root> <dest-dir> [--check]
//!
//! `--check` verifies that every path the vendor step copies — and every path
//! `build.zig.zon`'s `.paths` promises a `zig fetch` consumer — still exists,
//! copying nothing. `zig build check` runs it, because the copy itself only
//! ever runs at publish time: renaming a file out from under this list is
//! invisible until a release workflow dies on it, which is exactly how
//! `fig.md` -> `README.md` reached CI.

const std = @import("std");
const Dir = std.Io.Dir;

// The set `zig build install-c-lib` actually reads: the build scripts (build.zig
// plus its src/build/ helper tree, which rides along inside `src`), the whole Zig
// source rooted at src/c_api.zig, and the public header. testdata/, tools/, and
// the other bindings are not needed to build the static library.
const files = [_][]const u8{ "build.zig", "build.zig.zon" };
const trees = [_][]const u8{ "src", "bindings/c/include" };

// Dual license, single source of truth at the repo root. crates.io takes the
// SPDX `license` field, but a published crate should also carry the license
// *text*; copy it into each crate dir (git-ignored, `include`-added for fig,
// default-packaged for fig-macros) so neither has to vendor its own copy.
const license_files = [_][]const u8{ "LICENSE-MIT", "LICENSE-APACHE" };
const crate_dirs = [_][]const u8{ "bindings/rust/fig", "bindings/rust/fig-sys", "bindings/rust/fig-macros" };

/// The repo-root README, which the `fig` crate carries as its own so crates.io
/// has a page to render. Named once here because it is the file this tool is
/// most exposed to: it is the only source path that isn't build input, so a
/// rename of it breaks nothing until publish day.
const readme_src = "README.md";

pub fn main(init: std.process.Init) !void {
    const io = init.io;
    var arena_state = std.heap.ArenaAllocator.init(init.gpa);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    var args = try init.minimal.args.iterateAllocator(init.gpa);
    defer args.deinit();
    _ = args.next(); // argv0
    const src_root_path = args.next() orelse return error.MissingArgument;
    const dest_root_path = args.next() orelse return error.MissingArgument;
    var check_only = false;
    while (args.next()) |arg| {
        if (std.mem.eql(u8, arg, "--check")) {
            check_only = true;
        } else {
            std.debug.print("vendor-rust: unknown argument `{s}` (expected --check)\n", .{arg});
            std.process.exit(2);
        }
    }

    const cwd = Dir.cwd();
    var src_root = try cwd.openDir(io, src_root_path, .{});
    defer src_root.close(io);

    if (check_only) return checkSources(io, arena, src_root);

    // Start from a clean slate so a removed source file doesn't linger in the vendor.
    cwd.deleteTree(io, dest_root_path) catch {};
    try cwd.createDirPath(io, dest_root_path);
    var dest_root = try cwd.openDir(io, dest_root_path, .{});
    defer dest_root.close(io);

    inline for (files) |name| {
        try src_root.copyFile(name, dest_root, name, io, .{ .make_path = true });
    }
    inline for (trees) |name| {
        try copyTree(io, arena, src_root, dest_root, name);
    }

    // Fan the root license files out into each crate dir (paths relative to the
    // repo root we were handed as src_root).
    inline for (crate_dirs) |crate_dir| {
        var dir = try cwd.openDir(io, try std.fs.path.join(arena, &.{ src_root_path, crate_dir }), .{});
        defer dir.close(io);
        inline for (license_files) |name| {
            try src_root.copyFile(name, dir, name, io, .{ .make_path = true });
        }
        // The crate README crates.io renders on the package page. The canonical
        // text is the repo-root `README.md`. Only the top-level `fig` crate gets one —
        // `fig-macros` is an internal helper re-exported through `fig`'s derive
        // feature, so it stays README-less. Git-ignored here; `include`-added.
        if (comptime std.mem.eql(u8, crate_dir, "bindings/rust/fig")) {
            try src_root.copyFile(readme_src, dir, "README.md", io, .{ .make_path = true });
        }
    }

    std.debug.print("vendor-rust: copied {d} files + {d} trees + {d} licenses x{d} crates + 1 readme -> {s}\n", .{ files.len, trees.len, license_files.len, crate_dirs.len, dest_root_path });
}

/// Verify every path the vendor step reads, without writing anything: the build
/// inputs, the license texts, the crate README, and — because it is the same
/// class of mistake, a path promised in a list nothing exercises until publish
/// day — every entry in `build.zig.zon`'s `.paths`, which is what a `zig fetch`
/// consumer actually receives.
fn checkSources(io: std.Io, arena: std.mem.Allocator, src_root: Dir) !void {
    var missing: usize = 0;
    inline for (files) |name| missing += report(io, src_root, name, "build input");
    inline for (trees) |name| missing += report(io, src_root, name, "source tree");
    inline for (license_files) |name| missing += report(io, src_root, name, "license");
    missing += report(io, src_root, readme_src, "crate README");

    const zon = try src_root.readFileAlloc(io, "build.zig.zon", arena, .limited(1024 * 1024));
    var count: usize = 0;
    var it = zonPaths(zon);
    while (it.next()) |path| {
        count += 1;
        missing += report(io, src_root, path, "build.zig.zon .paths");
    }

    if (missing != 0) {
        std.debug.print("vendor-rust: {d} missing path(s) — a publish would fail on the first one\n", .{missing});
        std.process.exit(1);
    }
    std.debug.print("vendor-rust: OK ({d} build inputs + {d} trees + {d} licenses + README + {d} .paths entries all present)\n", .{ files.len, trees.len, license_files.len, count });
}

/// `1` if `name` is absent (having said so), `0` if it is there.
fn report(io: std.Io, src_root: Dir, name: []const u8, what: []const u8) usize {
    src_root.access(io, name, .{}) catch {
        std.debug.print("vendor-rust: MISSING {s}: {s}\n", .{ what, name });
        return 1;
    };
    return 0;
}

/// The quoted strings inside `build.zig.zon`'s `.paths = .{ … }`. A scanner
/// rather than a ZON parse for the same reason `tools/version_fields.zig` scans
/// rather than parses: fig cannot bootstrap-parse its own build manifest.
fn zonPaths(zon: []const u8) PathIter {
    const key = std.mem.indexOf(u8, zon, ".paths");
    const open = if (key) |k| std.mem.indexOfScalarPos(u8, zon, k, '{') else null;
    const close = if (open) |o| std.mem.indexOfScalarPos(u8, zon, o, '}') else null;
    if (open == null or close == null) return .{ .rest = "" };
    return .{ .rest = zon[open.? + 1 .. close.?] };
}

const PathIter = struct {
    rest: []const u8,

    fn next(it: *PathIter) ?[]const u8 {
        const start = std.mem.indexOfScalar(u8, it.rest, '"') orelse return null;
        const end = std.mem.indexOfScalarPos(u8, it.rest, start + 1, '"') orelse return null;
        const out = it.rest[start + 1 .. end];
        it.rest = it.rest[end + 1 ..];
        return out;
    }
};

/// Recursively copy `sub` from `src_root` to the same relative path under `dest_root`.
fn copyTree(io: std.Io, arena: std.mem.Allocator, src_root: Dir, dest_root: Dir, sub: []const u8) !void {
    var dir = try src_root.openDir(io, sub, .{ .iterate = true });
    defer dir.close(io);
    try dest_root.createDirPath(io, sub);

    var walker = try dir.walk(arena);
    defer walker.deinit();
    while (try walker.next(io)) |entry| {
        const dest_path = try std.fs.path.join(arena, &.{ sub, entry.path });
        switch (entry.kind) {
            // copyFile's `make_path` builds parents, so directory entries only
            // matter for preserving (rare) empty directories.
            .directory => try dest_root.createDirPath(io, dest_path),
            .file => try entry.dir.copyFile(entry.basename, dest_root, dest_path, io, .{ .make_path = true }),
            else => {},
        }
    }
}
