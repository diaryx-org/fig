//! Asserts that `language.validate` REJECTS a malformed `Language`.
//!
//! Zig has no built-in way to assert that a `@compileError` fires: a test that
//! triggers one does not fail, it fails to build, and takes the suite with it.
//! So the checks in `validate` — the closed declaration set and the coherence
//! rules, which are the whole of the proposal's §4 — cannot be covered by
//! `zig build test` at all. This is the harness the proposal's §6 called for
//! instead: write a deliberately-broken `Language` to a temp directory, run
//! `zig build-obj` over it, and assert the compile fails with the message it
//! should. Run by `zig build validate-check`, and by `zig build check`.
//!
//! Why it earns its keep: `validate`'s value is entirely in what it refuses,
//! and nothing else in the tree ever exercises a refusal. Every language in
//! `src/languages/` is well-formed by construction, so a `validate` that had
//! quietly stopped rejecting anything — an allowlist accidentally opened up, a
//! coherence rule short-circuiting — would look exactly like a healthy one. The
//! silent-fallback hazard this all exists to close (see the proposal's §10.5)
//! would be back, undetected.
//!
//! THE POSITIVE CONTROL IS LOAD-BEARING. A compile-failure suite has one
//! characteristic way of going useless: if the probe stops building for a
//! reason that has nothing to do with the defect under test — a moved path, a
//! renamed field in `manifest.Syntax`, a std API change — then every negative
//! case "passes" while testing nothing. Case 0 is a WELL-FORMED fixture that
//! must COMPILE CLEANLY. If it fails, the harness reports its own breakage
//! rather than a wall of green.
//!
//! That is not hypothetical: it fired twice while this file was being written,
//! both times on the module wiring below, and both times it was the only thing
//! standing between "9/9 ok" and nine cases proving nothing.
//!
//! Adding a rule to `validate` means adding a case here. The two are a pair.

const std = @import("std");
const Dir = std.Io.Dir;

/// One fixture: a `Language` with a single deliberate defect, and the fragment
/// of `validate`'s complaint that proves the right rule caught it.
///
/// `expect` is matched as a SUBSTRING of the compiler's stderr, and is chosen
/// to pin the rule rather than the phrasing — enough of the message to be
/// unambiguous, not so much that rewording the error breaks the harness.
const Case = struct {
    name: []const u8,
    /// Declarations spliced into the fixture's `Language` struct, after the
    /// well-formed base. A case adds a bad decl, or overrides a good one.
    decls: []const u8 = "",
    /// Replaces the base `syntax` body when non-empty.
    syntax_body: []const u8 = "",
    /// Replaces the base `caps` when non-empty.
    caps: []const u8 = "",
    /// Text `validate`'s error must contain. Empty means "must compile" — the
    /// positive control.
    expect: []const u8,
    /// When set, this decl is removed from the base rather than added.
    omit: []const u8 = "",
};

const cases = [_]Case{
    // ---- the positive control; see the module doc ----
    .{
        .name = "well-formed fixture compiles",
        .expect = "",
    },

    // ---- the closed declaration set (§4 job 2) ----
    .{
        .name = "unknown decl, capitalization slip",
        .decls = "pub const insertkey = {};",
        .expect = "declares unknown 'insertkey' — did you mean 'insertKey'?",
    },
    .{
        .name = "unknown decl, no near match",
        .decls = "pub const frobnicate = {};",
        .expect = "declares unknown 'frobnicate'",
    },
    .{
        .name = "missing required decl",
        .omit = "name",
        .expect = "must define name",
    },
    .{
        .name = "missing syntax on an editable format",
        .omit = "syntax",
        .expect = "has caps.edit and must define syntax",
    },

    // ---- coherence rules (§4 job 3) ----
    .{
        .name = "caps.edit = false with an editing hook",
        .caps = ".{ .read = true, .edit = false, .serialize = true }",
        .decls = "pub const insertKey = {};",
        .expect = "declares caps.edit = false but supplies the editing hook 'insertKey'",
    },
    .{
        .name = "trailing comment marker with no line comment marker",
        .syntax_body =
        \\.comment_style = .hash, .line_comment = null,
        \\.trailing_comment = "#", .kv_sep = ": ", .empty_map_literal = "{}",
        ,
        .expect = "trailing comment marker but no line comment marker",
    },
    .{
        .name = "sequence hook under block_seq_editable = false",
        .syntax_body =
        \\.comment_style = .hash, .line_comment = "#",
        \\.trailing_comment = "#", .kv_sep = ": ", .empty_map_literal = "{}",
        \\.block_seq_editable = false,
        ,
        .decls = "pub const appendToSeq = {};",
        .expect = "supplies 'appendToSeq', which the engine refuses before reaching",
    },
    .{
        .name = "partial comment hooks with no marker in any dialect",
        .syntax_body =
        \\.comment_style = .hash, .line_comment = null,
        \\.trailing_comment = null, .kv_sep = ": ", .empty_map_literal = null,
        ,
        .decls = "pub const addLeadingComment = {};",
        .expect = "can then only ever return CommentsUnsupported",
    },
};

/// A `Language` with nothing wrong with it. Each case perturbs exactly one
/// thing, so any failure is attributable to that one thing.
///
/// Deliberately minimal: `parse`/`print`/`Parser` are never CALLED here (no
/// `Editor` is instantiated, and `validate` checks only that the declarations
/// exist), so they are stubs. That is the point — the fixture exercises the
/// contract, not an implementation of it.
fn buildProbe(allocator: std.mem.Allocator, case: Case) ![]u8 {
    const default_syntax =
        \\.comment_style = .hash, .line_comment = "#",
        \\.trailing_comment = "#", .kv_sep = ": ", .empty_map_literal = "{}",
    ;
    const default_caps = ".{ .read = true, .edit = true, .serialize = true }";

    var buf: std.ArrayList(u8) = .empty;
    errdefer buf.deinit(allocator);

    // `fig` is passed as a MODULE, not imported by path. Two reasons, both
    // learned the hard way: a root module rooted in the work directory cannot
    // `@import` a file outside its own module path, and the module has to be
    // rooted at `src/root.zig` rather than at `languages/language.zig` because
    // the language modules reach up to `src/document.zig` and `src/editor.zig`
    // — a module rooted any deeper puts those outside its path. `language.zig`
    // re-exports `Caps`/`Syntax`, so one dependency covers the whole fixture.
    try buf.appendSlice(allocator, "const language = @import(\"fig\").Language;\n\n");
    try buf.appendSlice(allocator, "pub const Language = struct {\n");
    try buf.appendSlice(allocator, "    pub const Type = enum { Only };\n");
    try buf.appendSlice(allocator, "    pub const Parser = struct {};\n");
    try buf.appendSlice(allocator, "    pub const default_type: Type = .Only;\n");
    try buf.appendSlice(allocator, "    pub fn parse() void {}\n");
    try buf.appendSlice(allocator, "    pub fn print() void {}\n");
    if (!std.mem.eql(u8, case.omit, "name"))
        try buf.appendSlice(allocator, "    pub const name = \"fixture\";\n");
    try buf.appendSlice(allocator, "    pub const extensions: []const []const u8 = &.{\"fx\"};\n");
    try buf.print(allocator, "    pub const caps: language.Caps = {s};\n", .{
        if (case.caps.len != 0) case.caps else default_caps,
    });
    if (!std.mem.eql(u8, case.omit, "syntax")) {
        try buf.appendSlice(allocator, "    pub fn syntax(t: Type) language.Syntax {\n        _ = t;\n        return .{\n");
        try buf.print(allocator, "            {s}\n", .{
            if (case.syntax_body.len != 0) case.syntax_body else default_syntax,
        });
        try buf.appendSlice(allocator, "        };\n    }\n");
    }
    if (case.decls.len != 0) try buf.print(allocator, "    {s}\n", .{case.decls});
    try buf.appendSlice(allocator, "};\n\n");
    try buf.appendSlice(allocator, "comptime {\n    language.validate(Language);\n}\n");

    return buf.toOwnedSlice(allocator);
}

/// A `build_options` with every format compiled out.
///
/// Not a shortcut — a requirement. `language.zig` ends in a comptime block that
/// validates every compiled-in language, so importing it to reach `validate`
/// would otherwise drag all eleven real formats into each of these compiles.
/// With the gates off they resolve to `void` and the registry loop skips them,
/// leaving the fixture as the only thing under test (and each probe fast).
const options_src =
    \\pub const lang_json: bool = false;
    \\pub const lang_yaml: bool = false;
    \\pub const lang_toml: bool = false;
    \\pub const lang_zon: bool = false;
    \\pub const lang_xml: bool = false;
    \\pub const lang_fig: bool = false;
    \\pub const lang_ini: bool = false;
    \\pub const lang_dotenv: bool = false;
    \\pub const lang_properties: bool = false;
    \\pub const lang_plist: bool = false;
    \\pub const lang_canonical: bool = false;
    \\pub const lang_nestedtext: bool = false;
    \\
;

pub fn main(init: std.process.Init) !void {
    const io = init.io;
    const gpa = init.gpa;
    const arena = init.arena.allocator();

    var args = try init.minimal.args.iterateAllocator(gpa);
    defer args.deinit();
    _ = args.next(); // argv0
    const zig_exe = args.next() orelse return error.MissingArgument;
    const root = args.next() orelse return error.MissingArgument;
    const work = args.next() orelse return error.MissingArgument;

    const cwd = Dir.cwd();
    try cwd.createDirPath(io, work);

    const probe_path = try std.fs.path.join(arena, &.{ work, "probe.zig" });
    const opts_path = try std.fs.path.join(arena, &.{ work, "build_options.zig" });
    {
        const f = try cwd.createFile(io, opts_path, .{ .read = true });
        defer f.close(io);
        try f.writePositionalAll(io, options_src, 0);
        try f.setLength(io, options_src.len);
    }
    const root_arg = try std.fmt.allocPrint(arena, "-Mroot={s}", .{probe_path});
    const lang_arg = try std.fmt.allocPrint(arena, "-Mfig={s}/src/root.zig", .{root});
    const opts_arg = try std.fmt.allocPrint(arena, "-Mbuild_options={s}", .{opts_path});
    const emit_arg = try std.fmt.allocPrint(arena, "-femit-bin={s}", .{
        try std.fs.path.join(arena, &.{ work, "probe.o" }),
    });

    var failures: usize = 0;
    for (cases) |case| {
        const src = try buildProbe(gpa, case);
        defer gpa.free(src);
        {
            const f = try cwd.createFile(io, probe_path, .{ .read = true });
            defer f.close(io);
            try f.writePositionalAll(io, src, 0);
            try f.setLength(io, src.len);
        }

        const res = std.process.run(gpa, io, .{ .argv = &.{
            zig_exe,   "build-obj",
            // root: depends on both; language: depends on build_options.
            "--dep",   "build_options",
            "--dep",   "fig",
            root_arg,  "--dep",
            "build_options", lang_arg,
            opts_arg,  emit_arg,
        } }) catch |err| {
            std.debug.print("validate-check: could not run `{s} build-obj`: {s}\n", .{ zig_exe, @errorName(err) });
            return error.CompilerUnavailable;
        };
        defer gpa.free(res.stdout);
        defer gpa.free(res.stderr);

        const compiled = switch (res.term) {
            .exited => |c| c == 0,
            else => false,
        };

        if (case.expect.len == 0) {
            // Positive control: this one must build.
            if (compiled) {
                std.debug.print("  ok    {s}\n", .{case.name});
            } else {
                failures += 1;
                std.debug.print(
                    \\  FAIL  {s}
                    \\        the well-formed fixture did not compile, so every case below
                    \\        this one proves nothing. Fix the harness, not `validate`.
                    \\{s}
                    \\
                , .{ case.name, res.stderr });
            }
            continue;
        }

        if (!compiled and std.mem.indexOf(u8, res.stderr, case.expect) != null) {
            std.debug.print("  ok    {s}\n", .{case.name});
        } else if (compiled) {
            failures += 1;
            std.debug.print(
                "  FAIL  {s}\n        compiled cleanly; expected `validate` to reject it with:\n        {s}\n",
                .{ case.name, case.expect },
            );
        } else {
            failures += 1;
            std.debug.print(
                \\  FAIL  {s}
                \\        rejected, but not for the expected reason. Wanted:
                \\        {s}
                \\        got:
                \\{s}
                \\
            , .{ case.name, case.expect, res.stderr });
        }
    }

    if (failures != 0) {
        std.debug.print("validate-check: {d}/{d} cases failed\n", .{ failures, cases.len });
        return error.ValidateCheckFailed;
    }
    std.debug.print("validate-check: {d}/{d} cases ok\n", .{ cases.len, cases.len });
}
