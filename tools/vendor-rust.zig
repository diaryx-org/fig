//! Dev tool: vendor the repo-root license texts and README into the Rust
//! crates so each published crate carries them.
//!
//! crates.io takes the SPDX `license` field, but a published crate should also
//! carry the license *text*, and the `fig` crate the README crates.io renders
//! on its page. A crate published to crates.io cannot reach outside its own
//! directory, so before packaging they are copied into each crate dir
//! (git-ignored there; each crate's `include` force-adds them to the tarball).
//! fig's Rust is pure Rust (`fig-kit`, `fig-core`, the language crates and the
//! `fig` binding), so no Zig source is vendored any more: the `fig-sys` crate
//! that compiled fig's Zig core from a vendored copy is gone.
//!
//! This is the cross-platform replacement for a shell `cp`: it runs through the
//! same Zig toolchain the release already uses, so it works on Windows too.
//!
//! Run via `zig build vendor-rust`. Driven by build.zig as:
//!   vendor-rust <src-root> [--check]
//!
//! `--check` verifies that every path the vendor step copies from or into —
//! and every path `build.zig.zon`'s `.paths` promises a `zig fetch` consumer —
//! still exists, copying nothing. `zig build check` runs it, because the copy
//! itself only ever runs at publish time: renaming a file out from under this
//! list is invisible until a release workflow dies on it, which is exactly how
//! `fig.md` -> `README.md` reached CI.

const std = @import("std");
const Dir = std.Io.Dir;
const list = @import("languages");

fn languagePaths(comptime names: []const []const u8) [list.rows.len * names.len][]const u8 {
    var out: [list.rows.len * names.len][]const u8 = undefined;
    for (list.rows, 0..) |row, i| {
        for (names, 0..) |name, j| out[i * names.len + j] = "languages/" ++ row.name ++ (if (name.len == 0) "" else "/" ++ name);
    }
    return out;
}

// Dual license, single source of truth at the repo root, copied into every
// published crate's dir: the binding and its derive macros, the kit, the
// core, and each language's crate (languages/<name>/).
const license_files = [_][]const u8{ "LICENSE-MIT", "LICENSE-APACHE" };
const crate_dirs = [_][]const u8{ "bindings/rust/fig", "bindings/rust/fig-macros", "rust/fig-kit", "rust/fig-core" } ++ languagePaths(&.{""});

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

    // Fan the root license files out into each crate dir (paths relative to the
    // repo root we were handed as src_root).
    inline for (crate_dirs) |crate_dir| {
        var dir = try cwd.openDir(io, try std.fs.path.join(arena, &.{ src_root_path, crate_dir }), .{});
        defer dir.close(io);
        inline for (license_files) |name| {
            try src_root.copyFile(name, dir, name, io, .{ .make_path = true });
        }
        // The crate README crates.io renders on the package page. The canonical
        // text is the repo-root `README.md`. Only the top-level `fig` crate gets
        // one from here; each language crate has its own. Git-ignored here;
        // `include`-added.
        if (comptime std.mem.eql(u8, crate_dir, "bindings/rust/fig")) {
            try src_root.copyFile(readme_src, dir, "README.md", io, .{ .make_path = true });
        }
    }

    std.debug.print("vendor-rust: copied {d} licenses x{d} crates + 1 readme\n", .{ license_files.len, crate_dirs.len });
}

/// Verify every path the vendor step reads or writes into, without writing
/// anything: the license texts, the crate README, every crate dir, and —
/// because it is the same
/// class of mistake, a path promised in a list nothing exercises until publish
/// day — every entry in `build.zig.zon`'s `.paths`, which is what a `zig fetch`
/// consumer actually receives.
fn checkSources(io: std.Io, arena: std.mem.Allocator, src_root: Dir) !void {
    var missing: usize = 0;
    inline for (crate_dirs) |name| missing += report(io, src_root, name, "crate dir");
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
    std.debug.print("vendor-rust: OK ({d} crate dirs + {d} licenses + README + {d} .paths entries all present)\n", .{ crate_dirs.len, license_files.len, count });
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
/// rather than a ZON parse for the same reason `tools/version-sync.zig` scans
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
