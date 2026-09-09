//! INI editor tests for `Editor(Ini)`.
//!
//! No INI-specific editing logic is left: the generic span-splice engine in
//! `../../editor.zig` does every edit, driven by `ini.zig`'s `syntax`. This
//! module used to hold one hook, `iniInsertKey`, whose whole job was to skip
//! the engine's flow sniff on a root whose first byte is a `[section]`'s
//! bracket; `Syntax.flow_containers = false` and the engine's own rule that a
//! section format's root is block (`editor.Editor.isFlowNode`) say the same
//! thing as data. A reopened/scattered section threads correctly through the
//! generic `lastChild`-anchored block insert, because parsing always appends
//! a reopened section's new entries to the tail of its child list, in file
//! order — see `parser.zig`'s `parseSectionHeader` merge branch.
//!
//! Everything about a `[section]` as a WHOLE is the engine's. A section may be
//! REOPENED (`[a]` … `[b]` … `[a]`), which the parser merges into one mapping
//! whose span anchors only the FIRST header's name token — so its bytes are
//! scattered exactly the way a TOML table's or a fig container's are.
//! `parser.zig` records every header line of every section in
//! `Document.node_regions`, and from that the engine derives the section's
//! region set (`editor/regions.zig`): `deleteContainer`, `moveContainer` and
//! `reorderContainers` gather it and rebuild the source once, and the four
//! line-splice ops (`deleteKey`, `replaceValAtPath`, `moveKey`, `reorderKeys`)
//! refuse a section node — `CannotDeleteSection`, `CannotReplaceSection`,
//! `CannotMoveSection`, `CannotReorderSections`, per `Syntax.section_noun` —
//! and point at those ops. The tests below pin every one of those behaviours
//! for INI; the logic they exercise lives in `editor.zig`.
//!
//! There is no `insertContainer`/`renameContainer` twin: a new `[section]` is
//! `set`'s business (INI cannot auto-vivify — it has no literal spelling for
//! "an empty nested mapping", since `{}` is just a two-character STRING value
//! in INI, declared as `syntax().empty_map_literal = null` in `ini.zig`), and
//! a rename is one tight span the generic `replaceKeyAtPath` already rewrites,
//! since an INI header has no dotted descendants to follow.

const std = @import("std");
const testing = std.testing;

const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const Span = @import("../../util/span.zig");
const editor = @import("../../editor.zig");
const Ini = @import("ini.zig").Language;

/// The concrete editor the tests drive — the INI arm of the generic engine.
const IniEditor = editor.Editor(Ini);

// ── Tests ────────────────────────────────────────────────────────────────────
//
// Structural/section-nesting behavior lives here, next to the logic it
// exercises (mirroring TOML/fig's own editor-test placement); the bare
// root-level sanity checks stay in `editor.zig` alongside dotenv/.properties.

test "ini insertKey adds a key into an EXISTING section" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[server]\nhost = localhost\n");
    defer ed.deinit();
    try ed.set(&.{ .{ .key = "server" }, .{ .key = "port" } }, "80");
    try testing.expectEqualStrings("[server]\nhost = localhost\nport = 80\n", ed.source.items);
}

test "ini insertKey adds the first key into an EMPTY existing section" {
    // `[server]\n` with nothing under it yet — an empty section is a
    // childless block mapping, the same shape a from-scratch dotenv/
    // .properties file starts as, but with a narrow (name-token-anchored)
    // span rather than root's whole-file span — exercises the root-vs-
    // section split in `Editor.insertBlockKey`.
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[server]\n");
    defer ed.deinit();
    try ed.set(&.{ .{ .key = "server" }, .{ .key = "host" } }, "localhost");
    try testing.expectEqualStrings("[server]\nhost = localhost\n", ed.source.items);
}

test "ini set does NOT auto-vivify a missing section; surfaces NotFound" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("name = fig\n");
    defer ed.deinit();
    try testing.expectError(error.NotFound, ed.set(&.{ .{ .key = "server" }, .{ .key = "host" } }, "localhost"));
    // Refused cleanly — no stray `server = {}` (or any other) line spliced in.
    try testing.expectEqualStrings("name = fig\n", ed.source.items);
}

test "ini deleteKey refuses to delete a whole [section] header" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[server]\nhost = localhost\n");
    defer ed.deinit();
    try testing.expectError(error.CannotDeleteSection, ed.deleteKey(&.{.{ .key = "server" }}));
    // File is untouched by the refused delete.
    try testing.expectEqualStrings("[server]\nhost = localhost\n", ed.source.items);
    // A key WITHIN the section still deletes normally, leaving the (now
    // empty) section header intact.
    try ed.deleteKey(&.{ .{ .key = "server" }, .{ .key = "host" } });
    try testing.expectEqualStrings("[server]\n", ed.source.items);
}

test "ini replaceValAtPath refuses a whole [section] (would rename the header)" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[server]\nhost = localhost\n");
    defer ed.deinit();
    // The section mapping's span is the NAME token inside the header, so the
    // generic splice used to write `[REPLACED]` and report success — renaming
    // the section while its entries stayed under it.
    try testing.expectError(error.CannotReplaceSection, ed.replaceValAtPath(&.{.{ .key = "server" }}, "REPLACED"));
    try testing.expectEqualStrings("[server]\nhost = localhost\n", ed.source.items);
    // A value WITHIN the section still replaces normally.
    try ed.replaceValAtPath(&.{ .{ .key = "server" }, .{ .key = "host" } }, "example.com");
    try testing.expectEqualStrings("[server]\nhost = example.com\n", ed.source.items);
}

test "ini replaceValAtPath at the root rewrites the whole document" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[server]\nhost = localhost\n");
    defer ed.deinit();
    // The root's span is the whole file — the one container the guard exempts,
    // and the reason it tests the PATH rather than sniffing for a `[` (which the
    // first line here would trip).
    try ed.replaceValAtPath(&.{}, "[db]\nname = fig\n");
    try testing.expectEqualStrings("[db]\nname = fig\n", ed.source.items);
}

test "ini deleteContainer removes a whole section" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[a]\nx = 1\n[b]\ny = 2\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try testing.expectEqualStrings("[b]\ny = 2\n", ed.source.items);
}

test "ini deleteContainer removes EVERY occurrence of a reopened section" {
    // The case the engine's section rule refuses a line-delete for: `[a]` is
    // scattered, and its second header is in no node's span. Without
    // `Document.node_regions` the trailing `[a]` would survive and adopt
    // whatever followed it.
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[a]\nx = 1\n[b]\ny = 2\n[a]\nz = 3\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try testing.expectEqualStrings("[b]\ny = 2\n", ed.source.items);
}

test "ini deleteContainer removes an EMPTY reopened header too" {
    // A reopen with no entries under it has nothing to find it by except the
    // recorded header line — a gather that scanned upward from each child
    // would leave this one behind.
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[a]\nx = 1\n[b]\ny = 2\n[a]\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try testing.expectEqualStrings("[b]\ny = 2\n", ed.source.items);
}

test "ini deleteContainer takes owned comments with the section" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("; about a\n[a]\n; about x\nx = 1\n[b]\ny = 2\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try testing.expectEqualStrings("[b]\ny = 2\n", ed.source.items);
}

test "ini deleteContainer refuses a root-level scalar key" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("name = fig\n[a]\nx = 1\n");
    defer ed.deinit();
    try testing.expectError(error.NotAContainer, ed.deleteContainer(&.{.{ .key = "name" }}));
    try testing.expectEqualStrings("name = fig\n[a]\nx = 1\n", ed.source.items);
}

test "ini moveContainer relocates a section before another, collapsing its fragments" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[a]\nx = 1\n[b]\ny = 2\n[a]\nz = 3\n");
    defer ed.deinit();
    // `a`'s two fragments are removed and re-emitted as one section at `b`.
    // No blank line before it: `b` was already the file's second section, so
    // the relocated block lands at the very start with nothing preceding it to
    // separate from (see `sections.appendWithBlankBefore`).
    try ed.moveContainer(&.{.{ .key = "a" }}, &.{.{ .key = "b" }});
    try testing.expectEqualStrings("[a]\nx = 1\n[a]\nz = 3\n[b]\ny = 2\n", ed.source.items);
}

test "ini moveContainer with a null destination moves to EOF" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[a]\nx = 1\n[b]\ny = 2\n");
    defer ed.deinit();
    try ed.moveContainer(&.{.{ .key = "a" }}, null);
    try testing.expectEqualStrings("[b]\ny = 2\n\n[a]\nx = 1\n", ed.source.items);
}

test "ini reorderContainers reorders named sections, leaving others in place" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[a]\nx = 1\n[b]\ny = 2\n[c]\nz = 3\n");
    defer ed.deinit();
    try ed.reorderContainers(&.{ "c", "a" });
    // `b` is untouched; `c` and `a` swap into the slot `a` held.
    try testing.expectEqualStrings("[c]\nz = 3\n[a]\nx = 1\n[b]\ny = 2\n", ed.source.items);
}

test "ini reopened/scattered section: insertKey appends after the LAST physical entry" {
    // Merged sections thread new entries onto the tail of the (single,
    // logical) child list in file order, so the generic `lastChild`-anchored
    // `insertBlockKey` already lands the new key right after the section's
    // most recent physical occurrence — no multi-region gather needed,
    // unlike TOML's scattered tables.
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[a]\nx = 1\n[b]\nz = 1\n[a]\ny = 2\n");
    defer ed.deinit();
    try ed.set(&.{ .{ .key = "a" }, .{ .key = "w" } }, "3");
    try testing.expectEqualStrings("[a]\nx = 1\n[b]\nz = 1\n[a]\ny = 2\nw = 3\n", ed.source.items);
}

// --- the move/reorder guards (a section's block is its header LINE) ---
//
// The same span fact the delete and replace guards rest on, in the two ops
// that relocate an entry's block. `moveContainer`/`reorderContainers` above are
// what these refusals point at; both generic ops used to report success while
// handing one section's entries to another.

test "ini moveKey refuses to move a [section], or to move an entry before one" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("z = 0\n[a]\nx = 1\n[b]\ny = 2\n");
    defer ed.deinit();
    // Moving the section relocates its header alone, leaving `y = 2` for
    // whichever section ends up above it.
    try testing.expectError(error.CannotMoveSection, ed.moveKey(&.{.{ .key = "b" }}, &.{.{ .key = "z" }}));
    // And "before `[b]`" is the tail of `[a]`'s body, so the root key `z` would
    // have become `a.z`.
    try testing.expectError(error.CannotMoveSection, ed.moveKey(&.{.{ .key = "z" }}, &.{.{ .key = "b" }}));
    try testing.expectEqualStrings("z = 0\n[a]\nx = 1\n[b]\ny = 2\n", ed.source.items);
}

test "ini moveKey still moves plain entries inside a section" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[a]\nx = 1\ny = 2\nz = 3\n");
    defer ed.deinit();
    try ed.moveKey(&.{ .{ .key = "a" }, .{ .key = "z" } }, &.{ .{ .key = "a" }, .{ .key = "y" } });
    try testing.expectEqualStrings("[a]\nx = 1\nz = 3\ny = 2\n", ed.source.items);
}

test "ini reorderKeys refuses a reorder that shifts a section" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("z = 0\n[b]\ny = 2\n[a]\nx = 1\n");
    defer ed.deinit();
    // Used to produce `z = 0\n[a]\n[b]\ny = 2\nx = 1\n` — `[a]` emptied and
    // `x = 1` rehomed into `b`.
    try testing.expectError(error.CannotReorderSections, ed.reorderKeys(&.{}, &.{ "z", "a", "b" }));
    try testing.expectEqualStrings("z = 0\n[b]\ny = 2\n[a]\nx = 1\n", ed.source.items);
}

test "ini reorderKeys still reorders entries within a section" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[a]\nx = 1\ny = 2\nz = 3\n");
    defer ed.deinit();
    try ed.reorderKeys(&.{.{ .key = "a" }}, &.{ "z", "x" });
    try testing.expectEqualStrings("[a]\nz = 3\nx = 1\ny = 2\n", ed.source.items);
}
