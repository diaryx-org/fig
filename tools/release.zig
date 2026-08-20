//! Dev tool: cut a release, as one command.
//!
//! fig releases on a tag: pushing `cli/v*` builds and attaches the binaries and
//! writes the tap, `rust/v*` publishes to crates.io, `npm/v*` publishes the TS
//! package, `core/v*` triggers nothing but anchors the ABI diff. Everything
//! before that push is mechanical and easy to get half-right by hand — four
//! independently versioned artifacts with a floor between them, a changelog
//! whose generated region has to be cut into a released section, one tag per
//! track that actually moved — so it lives here instead of in a checklist:
//!
//!   zig build release -- <artifact> <version|major|minor|patch> [...] [flags]
//!
//!     artifact : core | cli | rust | npm   (fig-wasi rides cli, as ever)
//!     flags    : --push       also push the branch and the tags
//!                --no-verify  skip `zig build check`
//!
//! Several artifacts move in one release by naming several pairs:
//!
//!   zig build release -- core minor cli patch
//!
//! and the release set is read back off the manifests rather than off the
//! arguments, so the artifacts `version-set` raises to satisfy the `>= core`
//! floor are tagged and named in the changelog heading too.
//!
//! The order is: preflight, bump, verify, changelog, commit, tag, stop. The
//! verify step runs AFTER the bump on purpose — `semver-check` and
//! `version-floor` judge the versions in the tree, so running them first would
//! be judging the versions the release is replacing.
//!
//! `release` stops at the tag unless it is given `--push`. That asymmetry is the
//! whole safety model: everything before the push is a local commit that can be
//! thrown away (the tool prints the two-command undo), and the push is the one
//! that puts a version number on crates.io and npm, where it can be yanked but
//! never reused. So the push is asked for explicitly, each time, and a run
//! without it ends by printing the commands it did not run.
//!
//! Preflight refuses a release that is already doomed — dirty tree, wrong
//! branch, behind origin, no git-cliff, a changelog whose `## Unreleased`
//! section isn't where the cut expects it — because a half-applied release is a
//! working tree to untangle by hand, and not doing that is the point. A tag
//! collision is checked a moment later, since the tag names only exist once the
//! bump has run, and still before anything slow or irreversible. After the
//! bump, any failure restores the tree (the preflight guarantees it was clean,
//! so `git checkout -- .` is exact).

const std = @import("std");
const fields = @import("version_fields.zig");
const Dir = std.Io.Dir;

const max_file = 1 * 1024 * 1024;

/// The four independently versioned artifacts, each with its own tag prefix.
/// fig-wasi is deliberately absent: it is pinned to `cli_version` rather than
/// versioned on its own, so it rides the `cli/` tag (see docs/VERSIONING.md).
const Track = enum {
    core,
    cli,
    rust,
    npm,

    /// The tag prefix this track releases under.
    fn prefix(t: Track) []const u8 {
        return switch (t) {
            .core => "core",
            .cli => "cli",
            .rust => "rust",
            .npm => "npm",
        };
    }

    /// What pushing this track's tag sets off, for the closing summary.
    fn triggers(t: Track) []const u8 {
        return switch (t) {
            .core => "nothing (a plain anchor tag: `zig fetch`, and semver-check's ABI baseline)",
            .cli => "release-binaries + homebrew + release-npm-wasi, and the tangled.org mirror's release",
            .rust => "release.yml's crate job — crates.io (fig, fig-macros, fig-sys, the payload crates)",
            .npm => "release-npm.yml — @diaryx/fig",
        };
    }
};

const changelog_rel = "docs/CHANGELOG.md";
const changelog_script_rel = "tools/changelog.sh";

// The generated region inside the `## Unreleased` section. Only these two lines
// locate the cut; the bytes between them are git-cliff's, and the bytes after
// them (a handwritten release intro, when a release wants one) are the author's
// and move into the released section untouched.
const begin_marker = "<!-- git-cliff:begin — generated; edits here are overwritten -->";
const end_marker = "<!-- git-cliff:end -->";
/// What the region says when there is nothing unreleased — the state the cut
/// leaves behind, and `tools/changelog.sh`'s own empty-case text.
const empty_region = "_No commits since the last release tag._";
const unreleased_heading = "## Unreleased";
/// git-cliff's bucket for a commit whose subject it could not parse. Not fatal —
/// the release is still shippable — but it is the one thing in the region that
/// wants a human's eye before it becomes permanent.
const uncategorised_needle = "Uncategorised";

/// Every file a release may move. Named explicitly so the release commit holds
/// the bump and the changelog and nothing else, and so an unexpected edit
/// (something `check` regenerated, say) is caught rather than swept in.
const release_paths = [_][]const u8{
    "build.zig.zon",
    "figl/build.zig.figl",
    "build.zig",
    "bindings/c/include/fig.h",
    "README.md",
    "bindings/rust/Cargo.toml",
    "bindings/rust/Cargo.lock",
    "bindings/typescript/package.json",
    "bindings/typescript/package-lock.json",
    "bindings/wasi/package.json",
    "bindings/wasi/package-lock.json",
    changelog_rel,
};

pub fn main(init: std.process.Init) !void {
    const io = init.io;
    const gpa = init.gpa;
    var arena_state = std.heap.ArenaAllocator.init(gpa);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    var args = try init.minimal.args.iterateAllocator(gpa);
    defer args.deinit();
    _ = args.next(); // argv0
    // Injected by build.zig via addArtifactArg/addArg, not passed through `--`.
    const version_set_binary = args.next() orelse return usage("missing <version-set-binary>");
    const fig_binary = args.next() orelse return usage("missing <fig-binary>");
    const repo_root = args.next() orelse return usage("missing <repo-root>");

    var push = false;
    var verify = true;
    var pairs: std.ArrayList(Pair) = .empty;
    var pending: ?[]const u8 = null;
    while (args.next()) |arg| {
        if (std.mem.eql(u8, arg, "--push")) {
            push = true;
        } else if (std.mem.eql(u8, arg, "--no-verify")) {
            verify = false;
        } else if (std.mem.startsWith(u8, arg, "--")) {
            return usage(std.fmt.allocPrint(arena, "unknown flag `{s}`", .{arg}) catch "unknown flag");
        } else if (pending) |artifact| {
            try pairs.append(arena, .{ .artifact = artifact, .spec = arg });
            pending = null;
        } else {
            pending = arg;
        }
    }
    if (pending) |artifact|
        return usage(std.fmt.allocPrint(arena, "`{s}` has no <version|major|minor|patch> after it", .{artifact}) catch "missing version");
    if (pairs.items.len == 0) return usage("nothing to release — name at least one artifact");
    for (pairs.items) |p| {
        if (std.meta.stringToEnum(Track, p.artifact) == null) {
            if (std.mem.eql(u8, p.artifact, "wasi"))
                fail("fig-wasi is pinned to cli_version; release the CLI instead: `zig build release -- cli {s}`", .{p.spec});
            return usage(std.fmt.allocPrint(arena, "unknown artifact `{s}` (want core|cli|rust|npm)", .{p.artifact}) catch "unknown artifact");
        }
    }

    const cwd = Dir.cwd();
    const sh = Sh{ .gpa = gpa, .io = io, .root = repo_root };

    // ---- preflight: everything that can say no, before anything is written --
    step("preflight");
    try preflight(sh, arena, cwd, repo_root);

    const before = try readVersions(io, arena, cwd, repo_root);

    // ---- bump ---------------------------------------------------------------
    step("bump");
    for (pairs.items) |p| {
        const argv = [_][]const u8{ version_set_binary, fig_binary, repo_root, p.artifact, p.spec };
        if (!try sh.stream(&argv))
            fail("`version-set {s} {s}` failed — the tree is untouched by anything after it", .{ p.artifact, p.spec });
    }

    const after = try readVersions(io, arena, cwd, repo_root);
    var moved: std.ArrayList(Moved) = .empty;
    for (std.enums.values(Track)) |t| {
        const old = before.get(t);
        const new = after.get(t);
        if (!std.mem.eql(u8, old, new)) try moved.append(arena, .{ .track = t, .from = old, .to = new });
    }
    if (moved.items.len == 0) {
        // Nothing was written, so there is nothing to restore.
        fail("no version changed — every artifact is already at the version asked for", .{});
    }
    const heading = try headingFor(arena, moved.items);
    std.debug.print("\nrelease: {s}\n", .{heading});

    // From here on the tree is dirty, so every exit restores it.
    errdefer sh.restore();

    // The tag names only exist once the bump has run, so this is as early as a
    // collision can be caught — still well before `check`, the changelog, and
    // the commit, and while restoring the tree is all it takes to back out.
    try tagsAreFree(sh, arena, repo_root, moved.items);

    // ---- verify -------------------------------------------------------------
    if (verify) {
        step("verify (zig build check)");
        if (!try sh.stream(&.{ "zig", "build", "check" })) {
            sh.restore();
            fail("`zig build check` failed — tree restored, nothing committed", .{});
        }
    } else {
        step("verify — SKIPPED (--no-verify)");
    }

    // ---- changelog ----------------------------------------------------------
    step("changelog");
    const script = try std.fs.path.join(arena, &.{ repo_root, changelog_script_rel });
    if (!try sh.stream(&.{ "sh", script, "--write" })) {
        sh.restore();
        fail("`{s}` failed — tree restored, nothing committed", .{changelog_script_rel});
    }
    const changelog_path = try std.fs.path.join(arena, &.{ repo_root, changelog_rel });
    const changelog = cwd.readFileAlloc(io, changelog_path, arena, .limited(max_file)) catch {
        sh.restore();
        fail("could not re-read {s} — tree restored", .{changelog_rel});
    };
    const cut = cutReleaseFull(arena, changelog, heading) catch |err| {
        sh.restore();
        fail("could not cut {s}: {s} — tree restored", .{ changelog_rel, @errorName(err) });
    };
    try writeFile(io, cwd, changelog_path, cut.text);
    std.debug.print("{s}: `## Unreleased` -> `## {s}`, fresh empty region above it\n", .{ changelog_rel, heading });
    if (std.mem.indexOf(u8, cut.released_body, uncategorised_needle) != null)
        std.debug.print(
            \\
            \\  WARNING: the cut section still has an "Uncategorised — triage before release"
            \\  bucket. Those are commits whose subject git-conventional could not read. They
            \\  are shipped as-is; the region is generated, so fixing them means an amended
            \\  subject, not an edit here. Look before you push.
            \\
        , .{});

    // ---- commit + tag -------------------------------------------------------
    step("commit + tag");
    try commit(sh, arena, cwd, repo_root, heading);
    var tags: std.ArrayList([]const u8) = .empty;
    for (moved.items) |m| {
        const tag = try std.fmt.allocPrint(arena, "{s}/v{s}", .{ m.track.prefix(), m.to });
        if (!try sh.stream(&.{ "git", "-C", repo_root, "tag", "-a", tag, "-m", heading })) {
            // The commit is already made; leave it and say so rather than
            // guessing which half to unwind.
            fail("could not create tag {s} — the release COMMIT is in place; finish or undo by hand", .{tag});
        }
        try tags.append(arena, tag);
    }

    // ---- push, or the commands not run --------------------------------------
    const branch = try sh.capture(arena, &.{ "git", "-C", repo_root, "rev-parse", "--abbrev-ref", "HEAD" });
    if (!push) {
        step("done — nothing has left this machine");
        std.debug.print("To release:\n\n    git push origin {s}\n    git push origin", .{branch});
        for (tags.items) |t| std.debug.print(" {s}", .{t});
        std.debug.print("\n\nWhat each tag sets off:\n", .{});
        for (moved.items) |m|
            std.debug.print("  {s}/v{s}  ->  {s}\n", .{ m.track.prefix(), m.to, m.track.triggers() });
        std.debug.print("\nNone of it can be undone — a published version number is spent even\nafter a yank. To undo locally instead:\n\n    git tag -d", .{});
        for (tags.items) |t| std.debug.print(" {s}", .{t});
        std.debug.print(" && git reset --hard HEAD~1\n\n", .{});
        return;
    }

    step("push");
    if (!try sh.stream(&.{ "git", "-C", repo_root, "push", "origin", branch }))
        fail("could not push {s} — the commit and tags are still local", .{branch});
    for (tags.items) |t| {
        if (!try sh.stream(&.{ "git", "-C", repo_root, "push", "origin", t }))
            fail("could not push {s} — earlier tags may already be pushed", .{t});
    }
    std.debug.print("\nrelease: pushed. The release workflows are running:\n  https://github.com/diaryx-org/fig/actions\n\n", .{});
}

const Pair = struct { artifact: []const u8, spec: []const u8 };
const Moved = struct { track: Track, from: []const u8, to: []const u8 };

/// The four tracked versions, read straight off the manifests (the same
/// locators `version-floor` and `version-set` use), so "what moved" is a fact
/// about the tree rather than a re-derivation of the arguments.
const Versions = struct {
    core: []const u8,
    cli: []const u8,
    rust: []const u8,
    npm: []const u8,

    fn get(v: Versions, t: Track) []const u8 {
        return switch (t) {
            .core => v.core,
            .cli => v.cli,
            .rust => v.rust,
            .npm => v.npm,
        };
    }
};

fn readVersions(io: std.Io, arena: std.mem.Allocator, cwd: Dir, root: []const u8) !Versions {
    const zon = try readRel(io, arena, cwd, root, "build.zig.zon");
    const build_zig = try readRel(io, arena, cwd, root, "build.zig");
    const cargo = try readRel(io, arena, cwd, root, "bindings/rust/Cargo.toml");
    const ts_pkg = try readRel(io, arena, cwd, root, "bindings/typescript/package.json");
    return .{
        .core = fields.zonVersion(zon) orelse fail("no `.version` in build.zig.zon", .{}),
        .cli = fields.buildZigCliVersion(build_zig) orelse fail("no `cli_version` in build.zig", .{}),
        .rust = fields.cargoWorkspaceVersion(cargo) orelse fail("no `[workspace.package] version` in bindings/rust/Cargo.toml", .{}),
        .npm = fields.jsonVersion(ts_pkg) orelse fail("no `\"version\"` in bindings/typescript/package.json", .{}),
    };
}

/// `core 2.7.0 · cli 3.6.0` — the changelog heading and the tag message, naming
/// every artifact that moved, in track order. The separator and shape are
/// docs/CHANGELOG.md's ("One entry per release, not per artifact").
fn headingFor(arena: std.mem.Allocator, moved: []const Moved) ![]const u8 {
    var out: std.ArrayList(u8) = .empty;
    for (moved, 0..) |m, i| {
        if (i > 0) try out.appendSlice(arena, " · ");
        try out.print(arena, "{s} {s}", .{ m.track.prefix(), m.to });
    }
    return out.toOwnedSlice(arena);
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

fn preflight(sh: Sh, arena: std.mem.Allocator, cwd: Dir, root: []const u8) !void {
    // git-cliff writes the region the cut then moves; finding it missing after
    // the bump would mean unwinding a bump for a missing dependency.
    if (!try sh.quiet(&.{ "git-cliff", "--version" }))
        fail("git-cliff is not on PATH — `nix develop` (it is in the dev shell), or `cargo install git-cliff`", .{});

    const dirty = try sh.capture(arena, &.{ "git", "-C", root, "status", "--porcelain" });
    if (dirty.len != 0)
        fail("the working tree is dirty — commit or stash first, so the release commit holds only the bump and the changelog:\n{s}", .{dirty});

    const branch = try sh.capture(arena, &.{ "git", "-C", root, "rev-parse", "--abbrev-ref", "HEAD" });
    if (!std.mem.eql(u8, branch, "main"))
        fail("on branch `{s}`, and fig releases from `main`", .{branch});

    // A release cut on a stale main is a release missing commits. Advisory: a
    // laptop offline enough to fail the fetch can still cut the commit and push
    // later.
    if (try sh.quiet(&.{ "git", "-C", root, "fetch", "--quiet", "origin", "main" })) {
        const behind = try sh.capture(arena, &.{ "git", "-C", root, "rev-list", "--count", "HEAD..origin/main" });
        if (!std.mem.eql(u8, behind, "0"))
            fail("main is {s} commit(s) behind origin/main — pull first", .{behind});
    } else {
        std.debug.print("  WARNING: could not reach origin; releasing against the local main\n", .{});
    }

    // The cut is a text rewrite of one section, so its shape is a precondition,
    // not something to discover with a bumped tree on the floor.
    const path = try std.fs.path.join(arena, &.{ root, changelog_rel });
    const text = try cwd.readFileAlloc(io_of(sh), path, arena, .limited(max_file));
    const parsed = parseUnreleased(text) catch |err| fail("{s}: {s}", .{ changelog_rel, explain(err) });
    if (std.mem.eql(u8, std.mem.trim(u8, parsed.body, " \n"), empty_region))
        fail("{s}'s `## Unreleased` region is empty — there is nothing to release (run `zig build changelog` if commits have landed since the last tag)", .{changelog_rel});

    std.debug.print("  clean tree, on main, up to date with origin, git-cliff present\n", .{});
    std.debug.print("  {s}: `## Unreleased` region found and non-empty\n", .{changelog_rel});
}

fn explain(err: CutError) []const u8 {
    return switch (err) {
        error.NoBeginMarker => "the git-cliff begin marker is missing",
        error.NoEndMarker => "the git-cliff end marker is missing",
        error.DuplicateMarker => "more than one git-cliff marker pair — the cut strips them from released sections, so exactly one pair should exist, in `## Unreleased`",
        error.NotUnreleased => "the generated region is not inside a `## Unreleased` section — a previous release was cut by hand and left the markers behind",
        error.OutOfMemory => "out of memory",
    };
}

// ---------------------------------------------------------------------------
// The changelog cut
// ---------------------------------------------------------------------------

const CutError = error{
    NoBeginMarker,
    NoEndMarker,
    DuplicateMarker,
    NotUnreleased,
    OutOfMemory,
};

/// Where the `## Unreleased` section is, and what is in it.
const Unreleased = struct {
    /// Byte offset of the `## Unreleased` line.
    heading_start: usize,
    /// The generated bytes between the markers (git-cliff's).
    body: []const u8,
    /// Anything written by hand between the end marker and the next `## `
    /// heading — a release intro, which the cut carries down with the section.
    tail: []const u8,
    /// Byte offset of the next `## ` heading (the previous release), or the end
    /// of the file.
    section_end: usize,
};

fn parseUnreleased(text: []const u8) CutError!Unreleased {
    const begin = std.mem.indexOf(u8, text, begin_marker) orelse return error.NoBeginMarker;
    const after_begin = begin + begin_marker.len;
    if (std.mem.indexOf(u8, text[after_begin..], begin_marker) != null) return error.DuplicateMarker;
    const end = std.mem.indexOfPos(u8, text, after_begin, end_marker) orelse return error.NoEndMarker;
    if (std.mem.indexOfPos(u8, text, end + end_marker.len, end_marker) != null) return error.DuplicateMarker;

    // The last `## ` heading above the region. The file's preamble has several
    // (`## One entry per release, not per artifact`, …), so this must be the
    // nearest one, and it must be `## Unreleased` — anything else means a
    // previous release was cut by hand and the markers were left inside it.
    const heading_start = lastHeadingStart(text[0..begin]) orelse return error.NotUnreleased;
    const heading_line = lineAt(text, heading_start);
    if (!std.mem.eql(u8, std.mem.trimEnd(u8, heading_line, " \r"), unreleased_heading)) return error.NotUnreleased;

    const end_of_end_line = lineEnd(text, end + end_marker.len);
    const section_end = nextHeadingStart(text, end_of_end_line) orelse text.len;
    return .{
        .heading_start = heading_start,
        .body = std.mem.trim(u8, text[lineEnd(text, after_begin)..end], " \n\r"),
        .tail = std.mem.trim(u8, text[end_of_end_line..section_end], " \n\r"),
        .section_end = section_end,
    };
}

const Cut = struct {
    text: []const u8,
    /// The bytes that became the released section's body — what the caller
    /// scans for a triage bucket.
    released_body: []const u8,
};

/// Rename `## Unreleased` to `## <heading>`, drop the two marker lines from the
/// section that just became history (so exactly one marker pair is ever in the
/// file, and the next `zig build changelog` cannot rewrite a released section),
/// and open a fresh empty `## Unreleased` above it.
fn cutRelease(arena: std.mem.Allocator, text: []const u8, heading: []const u8) CutError![]const u8 {
    const cut = try cutReleaseFull(arena, text, heading);
    return cut.text;
}

fn cutReleaseFull(arena: std.mem.Allocator, text: []const u8, heading: []const u8) CutError!Cut {
    const u = try parseUnreleased(text);
    var out: std.ArrayList(u8) = .empty;
    try out.appendSlice(arena, text[0..u.heading_start]);
    // The fresh, empty Unreleased section — same shape tools/changelog.sh writes.
    try out.print(arena, "{s}\n\n{s}\n\n{s}\n\n{s}\n\n", .{ unreleased_heading, begin_marker, empty_region, end_marker });
    // The section that just became history, markers stripped.
    try out.print(arena, "## {s}\n\n{s}\n", .{ heading, u.body });
    if (u.tail.len != 0) try out.print(arena, "\n{s}\n", .{u.tail});
    if (u.section_end < text.len) try out.print(arena, "\n{s}", .{text[u.section_end..]});
    return .{ .text = try out.toOwnedSlice(arena), .released_body = u.body };
}

/// Start offset of the last line beginning with `## ` in `text`.
fn lastHeadingStart(text: []const u8) ?usize {
    var i = text.len;
    while (i > 0) {
        const line_start = if (std.mem.lastIndexOfScalar(u8, text[0 .. i - 1], '\n')) |nl| nl + 1 else 0;
        if (std.mem.startsWith(u8, text[line_start..], "## ")) return line_start;
        if (line_start == 0) return null;
        i = line_start;
    }
    return null;
}

/// Start offset of the first line beginning with `## ` at or after `from`.
fn nextHeadingStart(text: []const u8, from: usize) ?usize {
    var i = from;
    while (i < text.len) {
        if (std.mem.startsWith(u8, text[i..], "## ")) return i;
        const nl = std.mem.indexOfScalarPos(u8, text, i, '\n') orelse return null;
        i = nl + 1;
    }
    return null;
}

/// The line containing `at`, without its newline.
fn lineAt(text: []const u8, at: usize) []const u8 {
    const nl = std.mem.indexOfScalarPos(u8, text, at, '\n') orelse text.len;
    return text[at..nl];
}

/// The offset just past the newline ending the line that contains `at`.
fn lineEnd(text: []const u8, at: usize) usize {
    const nl = std.mem.indexOfScalarPos(u8, text, at, '\n') orelse return text.len;
    return nl + 1;
}

// ---------------------------------------------------------------------------
// Commit
// ---------------------------------------------------------------------------

/// Stage exactly the files a release moves, refuse if anything else changed,
/// and commit. `chore: release …` is skipped by `.config/cliff.toml`, so the
/// release commit never appears in the next release's changelog.
fn commit(sh: Sh, arena: std.mem.Allocator, cwd: Dir, root: []const u8, heading: []const u8) !void {
    var argv: std.ArrayList([]const u8) = .empty;
    try argv.appendSlice(arena, &.{ "git", "-C", root, "add", "--" });
    for (release_paths) |rel| {
        const path = try std.fs.path.join(arena, &.{ root, rel });
        // A path that doesn't exist is a repo-layout change, not a release
        // problem; skip it rather than failing the whole cut on `git add`.
        if (cwd.access(io_of(sh), path, .{})) |_| {
            try argv.append(arena, rel);
        } else |_| {}
    }
    if (!try sh.stream(argv.items)) fail("`git add` failed", .{});

    // Anything still unstaged is something this tool did not mean to write —
    // a file `check` regenerated, most likely. Say so instead of committing a
    // release with a stranger in it.
    const leftover = try sh.capture(arena, &.{ "git", "-C", root, "status", "--porcelain", "--untracked-files=no" });
    var it = std.mem.splitScalar(u8, leftover, '\n');
    while (it.next()) |line| {
        if (line.len == 0) continue;
        // Staged-only entries have a space in the second status column.
        if (line.len > 1 and line[1] != ' ')
            fail("unexpected change outside the release files: `{s}` — commit or discard it, then re-run", .{line});
    }

    const message = try std.fmt.allocPrint(arena, "chore: release {s}", .{heading});
    if (!try sh.stream(&.{ "git", "-C", root, "commit", "-m", message })) fail("`git commit` failed", .{});
}

/// Refuse a release whose tags are already taken. Local and, when origin is
/// reachable, remote — a tag that exists on origin is a release that already
/// happened, whatever this checkout knows.
fn tagsAreFree(sh: Sh, arena: std.mem.Allocator, root: []const u8, moved: []const Moved) !void {
    for (moved) |m| {
        const tag = try std.fmt.allocPrint(arena, "{s}/v{s}", .{ m.track.prefix(), m.to });
        const ref = try std.fmt.allocPrint(arena, "refs/tags/{s}", .{tag});
        if (try sh.quiet(&.{ "git", "-C", root, "rev-parse", "-q", "--verify", ref })) {
            sh.restore();
            fail("tag {s} already exists locally — tree restored, nothing committed", .{tag});
        }
        if (try sh.tryCapture(arena, &.{ "git", "-C", root, "ls-remote", "--tags", "origin", ref })) |remote| {
            if (remote.len != 0) {
                sh.restore();
                fail("tag {s} already exists on origin — that release already happened; tree restored", .{tag});
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

/// Child processes, in the two shapes this tool needs: `stream` for the long
/// ones whose output the maintainer should watch (the bump, `check`, git), and
/// `capture`/`quiet` for the short questions asked of git.
const Sh = struct {
    gpa: std.mem.Allocator,
    io: std.Io,
    root: []const u8,

    /// Run with the parent's stdio; `true` if it exited 0.
    fn stream(sh: Sh, argv: []const []const u8) !bool {
        var child = try std.process.spawn(sh.io, .{ .argv = argv, .cwd = .{ .path = sh.root } });
        return ok(try child.wait(sh.io));
    }

    /// Run silently; `true` if it exited 0. For "is this tool here" and for
    /// commands whose failure is not itself news (`git fetch` offline).
    fn quiet(sh: Sh, argv: []const []const u8) !bool {
        const res = std.process.run(sh.gpa, sh.io, .{ .argv = argv, .cwd = .{ .path = sh.root } }) catch return false;
        defer sh.gpa.free(res.stdout);
        defer sh.gpa.free(res.stderr);
        return ok(res.term);
    }

    /// Run and return trimmed stdout, failing the release if the command does.
    fn capture(sh: Sh, arena: std.mem.Allocator, argv: []const []const u8) ![]const u8 {
        const res = std.process.run(sh.gpa, sh.io, .{ .argv = argv, .cwd = .{ .path = sh.root } }) catch
            fail("could not run `{s}`", .{argv[0]});
        defer sh.gpa.free(res.stdout);
        defer sh.gpa.free(res.stderr);
        if (!ok(res.term))
            fail("`{s} …` failed:\n{s}", .{ argv[0], res.stderr });
        return arena.dupe(u8, std.mem.trim(u8, res.stdout, " \t\r\n")) catch fail("out of memory", .{});
    }

    /// Trimmed stdout, or null if the command could not run or failed —
    /// for questions asked of the network, where "no answer" is not "no".
    fn tryCapture(sh: Sh, arena: std.mem.Allocator, argv: []const []const u8) !?[]const u8 {
        const res = std.process.run(sh.gpa, sh.io, .{ .argv = argv, .cwd = .{ .path = sh.root } }) catch return null;
        defer sh.gpa.free(res.stdout);
        defer sh.gpa.free(res.stderr);
        if (!ok(res.term)) return null;
        return try arena.dupe(u8, std.mem.trim(u8, res.stdout, " \t\r\n"));
    }

    /// Put the tree back. Sound because preflight proved it was clean: nothing
    /// but this tool's own writes can be lost.
    fn restore(sh: Sh) void {
        _ = sh.quiet(&.{ "git", "-C", sh.root, "checkout", "--", "." }) catch {};
    }
};

/// A process that finished the way a tool is supposed to.
fn ok(term: std.process.Child.Term) bool {
    return switch (term) {
        .exited => |code| code == 0,
        else => false,
    };
}

fn io_of(sh: Sh) std.Io {
    return sh.io;
}

// ---------------------------------------------------------------------------
// Files, messages
// ---------------------------------------------------------------------------

fn readRel(io: std.Io, arena: std.mem.Allocator, cwd: Dir, root: []const u8, rel: []const u8) ![]u8 {
    const path = try std.fs.path.join(arena, &.{ root, rel });
    return cwd.readFileAlloc(io, path, arena, .limited(max_file));
}

fn writeFile(io: std.Io, cwd: Dir, path: []const u8, text: []const u8) !void {
    const file = try cwd.createFile(io, path, .{ .read = true });
    defer file.close(io);
    try file.writePositionalAll(io, text, 0);
    try file.setLength(io, text.len);
}

fn step(title: []const u8) void {
    std.debug.print("\n━━ {s} ━━\n", .{title});
}

fn usage(why: []const u8) noreturn {
    std.debug.print(
        \\release: {s}
        \\
        \\usage: zig build release -- <artifact> <version|major|minor|patch> [...] [--push] [--no-verify]
        \\  artifact : core | cli | rust | npm   (fig-wasi rides the cli track)
        \\  version  : an explicit SemVer (e.g. 2.7.0) or a bump keyword
        \\
        \\examples:
        \\  zig build release -- rust minor
        \\  zig build release -- core minor cli patch
        \\  zig build release -- cli patch --push
        \\
        \\Preview a bump without releasing: zig build version-set -- <artifact> <spec> --dry-run
        \\
    , .{why});
    std.process.exit(2);
}

fn fail(comptime fmt: []const u8, args: anytype) noreturn {
    std.debug.print("release: FAIL: " ++ fmt ++ "\n", args);
    std.process.exit(1);
}

// ---------------------------------------------------------------------------
// Tests — the cut, which is the one part that rewrites a file rather than
// shelling out, and the one part whose mistakes are only visible after a
// release is already history.
// ---------------------------------------------------------------------------

const testing = std.testing;

/// A changelog in the shape the real one has: a preamble carrying `## ` headings
/// of its own, the marked region inside `## Unreleased`, and a previous release
/// below it.
const sample =
    \\# fig — changelog
    \\
    \\## How the Unreleased section is written
    \\
    \\Prose about the markers.
    \\
    \\## Unreleased
    \\
    \\<!-- git-cliff:begin — generated; edits here are overwritten -->
    \\
    \\### Added
    \\
    \\- **rust** — a thing ([`abc1234`](url))
    \\
    \\<!-- git-cliff:end -->
    \\
    \\## 2.6.0
    \\
    \\### Fixed
    \\
    \\- **toml** — an older thing
    \\
;

test "the cut renames the section, strips its markers, and opens a fresh one" {
    const out = try cutRelease(testing.allocator, sample, "core 2.7.0 · rust 3.3.0");
    defer testing.allocator.free(out);

    // Exactly one marker pair survives, and it is in the new empty section.
    try testing.expectEqual(1, std.mem.count(u8, out, begin_marker));
    try testing.expectEqual(1, std.mem.count(u8, out, end_marker));
    const fresh = std.mem.indexOf(u8, out, "## Unreleased").?;
    const released = std.mem.indexOf(u8, out, "## core 2.7.0 · rust 3.3.0").?;
    try testing.expect(fresh < std.mem.indexOf(u8, out, begin_marker).?);
    try testing.expect(std.mem.indexOf(u8, out, end_marker).? < released);
    try testing.expect(std.mem.indexOf(u8, out, empty_region).? < released);

    // The generated bullets moved down into the released section, and the
    // older release is still below them, untouched.
    const bullet = std.mem.indexOf(u8, out, "- **rust** — a thing").?;
    try testing.expect(released < bullet);
    try testing.expect(bullet < std.mem.indexOf(u8, out, "## 2.6.0").?);
    try testing.expect(std.mem.endsWith(u8, out, "- **toml** — an older thing\n"));

    // The preamble is byte-identical, `## ` headings and all.
    try testing.expect(std.mem.startsWith(u8, out, "# fig — changelog\n\n## How the Unreleased section is written\n\nProse about the markers.\n\n## Unreleased\n"));

    // No blank-line pileup where the markers were.
    try testing.expectEqual(null, std.mem.indexOf(u8, out, "\n\n\n"));
}

test "a handwritten intro below the end marker rides down with the release" {
    const with_intro = try std.mem.replaceOwned(u8, testing.allocator, sample, "<!-- git-cliff:end -->\n", "<!-- git-cliff:end -->\n\nThis release is mostly about spans.\n");
    defer testing.allocator.free(with_intro);
    const out = try cutRelease(testing.allocator, with_intro, "core 2.7.0");
    defer testing.allocator.free(out);

    const released = std.mem.indexOf(u8, out, "## core 2.7.0").?;
    const intro = std.mem.indexOf(u8, out, "This release is mostly about spans.").?;
    try testing.expect(released < intro);
    try testing.expect(intro < std.mem.indexOf(u8, out, "## 2.6.0").?);
}

test "the cut is idempotent in the sense that matters: it can be run again" {
    const once = try cutRelease(testing.allocator, sample, "rust 3.3.0");
    defer testing.allocator.free(once);
    // The fresh section is empty, so a second cut is refused upstream by
    // preflight — but the shape must still parse, or the next release cannot
    // find its own region.
    const parsed = try parseUnreleased(once);
    try testing.expectEqualStrings(empty_region, parsed.body);
}

test "a changelog the cut cannot trust is refused, not guessed at" {
    // Markers left inside a released section (what a hand-cut release leaves).
    const hand_cut = try std.mem.replaceOwned(u8, testing.allocator, sample, "## Unreleased", "## 2.7.0");
    defer testing.allocator.free(hand_cut);
    try testing.expectError(error.NotUnreleased, parseUnreleased(hand_cut));

    // Two pairs: the cut would have to choose, so it doesn't.
    const doubled = try std.mem.concat(testing.allocator, u8, &.{ sample, sample });
    defer testing.allocator.free(doubled);
    try testing.expectError(error.DuplicateMarker, parseUnreleased(doubled));

    try testing.expectError(error.NoBeginMarker, parseUnreleased("# nothing here\n"));
}

test "the heading names every artifact that moved, in track order" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    try testing.expectEqualStrings("rust 3.3.0", try headingFor(arena, &.{
        .{ .track = .rust, .from = "3.2.0", .to = "3.3.0" },
    }));
    try testing.expectEqualStrings("core 2.7.0 · cli 3.6.0 · npm 2.7.0", try headingFor(arena, &.{
        .{ .track = .core, .from = "2.6.0", .to = "2.7.0" },
        .{ .track = .cli, .from = "3.5.4", .to = "3.6.0" },
        .{ .track = .npm, .from = "2.6.0", .to = "2.7.0" },
    }));
}
