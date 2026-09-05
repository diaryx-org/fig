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
//! standing between an all-green scoreboard and every case proving nothing.
//!
//! Adding a rule to `validate` means adding a case here. The two are a pair.

const std = @import("std");
const Dir = std.Io.Dir;
const list = @import("languages");

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
    /// Replaces the base `dialects` rows (the contents of the `&.{ … }`)
    /// when non-empty.
    dialects: []const u8 = "",
    /// Text `validate`'s error must contain. Empty means "must compile" — the
    /// positive control.
    expect: []const u8,
    /// When set, this decl is removed from the base rather than added.
    omit: []const u8 = "",
    /// When set, the probe is a complete out-of-tree `Language` with a
    /// working parser and printer (see `buildDrivenProbe`), compiled with
    /// `zig test` and RUN: it must build and its test must pass. The
    /// positive control for the engine rather than for `validate` alone.
    run: bool = false,
};

const cases = [_]Case{
    // ---- the positive control; see the module doc ----
    .{
        .name = "well-formed fixture compiles",
        .expect = "",
    },
    .{
        // The claim `docs/zig.md` makes for an out-of-tree format: a type that
        // passes `validate` can be given to `Editor`, its hooks may use
        // `editor/splice.zig`, and it appears in no registry-derived enum.
        // Proved by running one: a `Language` declared here, outside
        // `src/languages/`, drives `Editor.set` end to end.
        .name = "out-of-tree Language drives Editor",
        .expect = "",
        .run = true,
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
        // The same rule as above, on the decl that joined `Decls.required`
        // when the serializer's dispatch became registry-derived: `Printer` is
        // the printer MODULE (`@field(Lang.Printer, entry.print_name)`), which
        // every format must expose even when — like plist and xml — its
        // `Language` declares no `printNode` of its own.
        .name = "missing required decl (Printer)",
        .omit = "Printer",
        .expect = "must define Printer",
    },
    .{
        .name = "missing syntax on an editable format",
        .omit = "syntax",
        .expect = "has caps.edit and must define syntax",
    },
    .{
        // The format's own registry rows joined `Decls.required` when the
        // registry became assembled from the languages rather than written
        // in `language.zig`.
        .name = "missing required decl (dialects)",
        .omit = "dialects",
        .expect = "must define dialects",
    },

    // ---- the dialect table's own coherence ----
    .{
        // The row selecting `default_type` is the one every consumer reaches
        // the language by, so it carries the language's name.
        .name = "default dialect row not named after the language",
        .dialects = ".{ .name = \"other\", .abi_value = 99, .splice = .raw, .empty_doc_seed = \"\" }",
        .expect = "which must be named after the language",
    },
    .{
        .name = "two dialect rows with one name",
        .dialects = ".{ .name = \"fixture\", .abi_value = 99, .splice = .raw, .empty_doc_seed = \"\" }," ++
            " .{ .name = \"fixture\", .abi_value = 98, .splice = .raw, .empty_doc_seed = \"\" }",
        .expect = "declares two dialects named 'fixture'",
    },

    .{
        // `deserialize.zig` parses a deserializable dialect through the
        // language's `parseAbstract`, so the row and the decl are a pair.
        .name = "deserializable dialect without parseAbstract",
        .dialects = ".{ .name = \"fixture\", .abi_value = 99, .deserializable = true, .splice = .raw, .empty_doc_seed = \"\" }",
        .expect = "marks dialect 'fixture' deserializable but declares no parseAbstract",
    },

    // ---- coherence rules (§4 job 3) ----
    .{
        .name = "caps.edit = false with an editing hook",
        .caps = ".{ .read = true, .edit = false, .serialize = true }",
        .decls = "pub const insertKey = {};",
        .expect = "declares caps.edit = false but supplies the editing hook 'insertKey'",
    },
    .{
        // `caps.lossless` describes what the envelope pass may write INTO the
        // format, so it contradicts `serialize = false` the way a hook
        // contradicts `edit = false`.
        .name = "caps.lossless on a format that cannot serialize",
        .caps = ".{ .read = true, .edit = true, .serialize = false, .lossless = .{ .null = true } }",
        .expect = "declares caps.lossless (an envelope target for serialized output) but caps.serialize = false",
    },
    .{
        .name = "trailing comment marker with no line comment marker",
        .syntax_body =
        \\.comments = .{ .style = .hash, .line = null, .trailing = "#" },
        \\.kv_sep = ": ", .empty_map_literal = "{}",
        ,
        .expect = "trailing comment marker but no line comment marker",
    },
    .{
        // The other direction of the same field: a null says "my own
        // `insertKey` writes every entry", so it is only true alongside the
        // hook. Without the rule the mismatch surfaces as a runtime
        // `UnsupportedShape` from `Editor.kvSep` on an ordinary `set`.
        .name = "kv_sep = null with no insertKey hook",
        .syntax_body =
        \\.comments = .hash, .kv_sep = null, .empty_map_literal = "{}",
        ,
        .expect = "declares kv_sep = null but does not hook insertKey",
    },
    .{
        // The closed set covers whole-container ops too (`Decls.exclusive`), so
        // a misspelled one is caught the same way a misspelled hook is rather
        // than silently never dispatching.
        .name = "unknown decl, whole-container op typo",
        .decls = "pub const insertcontainer = {};",
        .expect = "declares unknown 'insertcontainer' — did you mean 'insertContainer'?",
    },
    .{
        .name = "caps.edit = false with a whole-container op",
        .caps = ".{ .read = true, .edit = false, .serialize = true }",
        .decls = "pub const renameContainer = {};",
        .expect = "declares caps.edit = false but supplies the editing hook 'renameContainer'",
    },
    .{
        // The three remaining whole-container hooks address section nodes,
        // which only a section format (`section_noun` non-null) has. The
        // base fixture declares none, so the hook contradicts the manifest.
        .name = "whole-container op on a non-section format",
        .decls = "pub const insertContainer = {};",
        .expect = "is not a section format (section_noun is null in every dialect) but supplies the whole-container op 'insertContainer'",
    },
    .{
        // The generic ops are not declarations any more: a format that names
        // one is naming a method the engine already has.
        .name = "unknown decl, generic whole-container op",
        .decls = "pub const deleteContainer = {};",
        .expect = "declares unknown 'deleteContainer'",
    },
    .{
        .name = "sequence hook under block_seq_editable = false",
        .syntax_body =
        \\.comments = .hash, .kv_sep = ": ", .empty_map_literal = "{}",
        \\.block_seq_editable = false,
        ,
        .decls = "pub const appendToSeq = {};",
        .expect = "supplies 'appendToSeq', which the engine refuses before reaching",
    },
    .{
        .name = "partial comment hooks with no marker in any dialect",
        .syntax_body =
        \\.comments = .{ .style = .hash, .line = null, .trailing = null },
        \\.kv_sep = ": ", .empty_map_literal = null,
        ,
        .decls = "pub const addLeadingComment = {};",
        .expect = "can then only ever return CommentsUnsupported",
    },
};

/// A `Language` with nothing wrong with it. Each case perturbs exactly one
/// thing, so any failure is attributable to that one thing.
///
/// Deliberately minimal: `parse`/`print`/`Parser`/`Printer` are never CALLED
/// here (no `Editor` is instantiated, and `validate` checks only that the
/// declarations exist), so they are stubs. That is the point — the fixture
/// exercises the contract, not an implementation of it.
fn buildProbe(allocator: std.mem.Allocator, case: Case) ![]u8 {
    const default_syntax =
        \\.comments = .hash, .kv_sep = ": ", .empty_map_literal = "{}",
    ;
    const default_caps = ".{ .read = true, .edit = true, .serialize = true }";
    // One row, named after the language, selecting its (only) `Type`. The
    // ABI value is arbitrary: the fixture is never in the real registry, so
    // nothing cross-checks it.
    const default_dialects = ".{ .name = \"fixture\", .abi_value = 99, .splice = .raw, .empty_doc_seed = \"\" }";

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
    if (!std.mem.eql(u8, case.omit, "Printer"))
        try buf.appendSlice(allocator, "    pub const Printer = struct {};\n");
    try buf.appendSlice(allocator, "    pub const default_type: Type = .Only;\n");
    try buf.appendSlice(allocator, "    pub fn parse() void {}\n");
    try buf.appendSlice(allocator, "    pub fn print() void {}\n");
    if (!std.mem.eql(u8, case.omit, "name"))
        try buf.appendSlice(allocator, "    pub const name = \"fixture\";\n");
    try buf.appendSlice(allocator, "    pub const extensions: []const []const u8 = &.{\"fx\"};\n");
    try buf.print(allocator, "    pub const caps: language.Caps = {s};\n", .{
        if (case.caps.len != 0) case.caps else default_caps,
    });
    if (!std.mem.eql(u8, case.omit, "dialects")) {
        try buf.print(allocator, "    pub const dialects: []const language.Dialect(@This()) = &.{{ {s} }};\n", .{
            if (case.dialects.len != 0) case.dialects else default_dialects,
        });
    }
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

/// A complete out-of-tree `Language`, plus the test that drives it.
///
/// The declarations are the fixture's own — its name, extension, caps,
/// dialect row and syntax are nothing the tree has — while the parser and
/// printer are borrowed from dotenv through `Language.moduleFor`, the
/// ungated route to a language's module, so the probe has a real grammar to
/// edit without shipping one. dotenv declares no hooks, so every operation
/// runs the generic engine, which is exactly the surface an out-of-tree
/// format without hooks would be relying on. `build_options` still gates
/// every real format off (see `options_src`), so the fixture is the only
/// `Language` in the compile and cannot be reached through any registry
/// enum — which the test also checks.
fn buildDrivenProbe(allocator: std.mem.Allocator) ![]u8 {
    var buf: std.ArrayList(u8) = .empty;
    errdefer buf.deinit(allocator);
    try buf.appendSlice(allocator,
        \\const std = @import("std");
        \\const fig = @import("fig");
        \\const language = fig.Language;
        \\const borrowed = language.moduleFor("dotenv");
        \\
        \\pub const Language = struct {
        \\    pub const Type = borrowed.Type;
        \\    pub const Parser = borrowed.Parser;
        \\    pub const Printer = borrowed.Printer;
        \\    pub const default_type: Type = borrowed.Language.default_type;
        \\    pub fn parse(parser: *Parser, input: []const u8, format: Type) !fig.Document {
        \\        return Parser.parse(parser.allocator, input, format);
        \\    }
        \\    pub const print = Printer.print;
        \\    pub const printNode = Printer.printNode;
        \\
        \\    pub const name = "fixture";
        \\    pub const extensions: []const []const u8 = &.{"fx"};
        \\    pub const caps: language.Caps = .{ .read = true, .edit = true, .serialize = true, .max_mapping_depth = 0 };
        \\    pub const dialects: []const language.Dialect(@This()) = &.{
        \\        .{ .name = "fixture", .abi_value = 99, .sniff_rank = 200, .splice = .raw, .empty_doc_seed = "" },
        \\    };
        \\    pub fn syntax(t: Type) language.Syntax {
        \\        return borrowed.Language.syntax(t);
        \\    }
        \\};
        \\
        \\comptime {
        \\    language.validate(Language);
        \\    // Out of tree means out of the registry: no derived enum names it.
        \\    if (@hasField(fig.AST.SerializeFormat, "fixture")) @compileError("the fixture leaked into the registry");
        \\}
        \\
        \\test "an out-of-tree Language drives Editor end to end" {
        \\    const Ed = fig.Editor(Language);
        \\    var ed: Ed = .{ .allocator = std.testing.allocator };
        \\    defer ed.deinit();
        \\    try ed.init("A=1\n");
        \\    try ed.set(&.{.{ .key = "B" }}, "2");
        \\    try std.testing.expectEqualStrings("A=1\nB=2\n", ed.source.items);
        \\    try ed.deleteKey(&.{.{ .key = "A" }});
        \\    try std.testing.expectEqualStrings("B=2\n", ed.source.items);
        \\}
        \\
    );
    return buf.toOwnedSlice(allocator);
}

/// A `build_options` with every format compiled out.
///
/// Not a shortcut — a requirement. `language.zig` ends in a comptime block that
/// validates every compiled-in language, so importing it to reach `validate`
/// would otherwise drag every real format into each of these compiles. With
/// the gates off they resolve to `void` and the registry loop skips them,
/// leaving the fixture as the only thing under test (and each probe fast).
///
/// Written from `src/languages/list.zig` (the `languages` module) rather than
/// by hand, so a format added to the list is gated off here without an edit.
/// `canonical` is not a format and is named by hand, as it is in
/// `src/build/Options.zig`.
const options_src = blk: {
    var src: []const u8 = "";
    for (list.rows) |row| src = src ++ "pub const lang_" ++ row.name ++ ": bool = false;\n";
    src = src ++ "pub const lang_canonical: bool = false;\n";
    break :blk src;
};

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
        const src = if (case.run) try buildDrivenProbe(gpa) else try buildProbe(gpa, case);
        defer gpa.free(src);
        {
            const f = try cwd.createFile(io, probe_path, .{ .read = true });
            defer f.close(io);
            try f.writePositionalAll(io, src, 0);
            try f.setLength(io, src.len);
        }

        // `build-obj` for a compile-or-refuse case; `test` — which also runs
        // the binary — for a case that must execute.
        const res = std.process.run(gpa, io, .{
            .argv = if (case.run) &.{
                zig_exe,         "test",
                "--dep",         "build_options",
                "--dep",         "fig",
                root_arg,        "--dep",
                "build_options", lang_arg,
                opts_arg,
            } else &.{
                zig_exe,         "build-obj",
                // root: depends on both; language: depends on build_options.
                "--dep",         "build_options",
                "--dep",         "fig",
                root_arg,        "--dep",
                "build_options", lang_arg,
                opts_arg,        emit_arg,
            },
        }) catch |err| {
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
            // Positive control: this one must build (and, for a run case, pass).
            if (compiled) {
                std.debug.print("  ok    {s}\n", .{case.name});
            } else {
                failures += 1;
                std.debug.print(
                    \\  FAIL  {s}
                    \\        the well-formed fixture did not {s}, so every case below
                    \\        this one proves nothing. Fix the harness, not `validate`.
                    \\{s}
                    \\
                , .{ case.name, if (case.run) "build and pass" else "compile", res.stderr });
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
