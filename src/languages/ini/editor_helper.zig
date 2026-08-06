//! INI-specific editing helpers for `Editor(Ini)`.
//!
//! The generic span-splice engine lives in `../../editor.zig`; this module holds
//! the INI-only logic it delegates to, mirroring TOML/fig's own
//! `editor_helper.zig` split (structural per-language decisions live here;
//! `editor.zig` stays a one-line dispatch to them). INI is nearly flat like
//! dotenv/.properties — one level of `[section]` nesting, no arrays/inline
//! tables/dotted keys — so it needs far less than TOML: no multi-region
//! gather, because a reopened/scattered section already threads correctly
//! through the generic `lastChild`-anchored block insert (parsing always
//! appends a reopened section's new entries to the tail of its child list, in
//! file order — see `parser.zig`'s `parseSectionHeader` merge branch). What
//! IS needed:
//!
//!   - `iniInsertKey`: INI has no flow syntax at all, so this skips the
//!     generic `isFlow` sniff outright rather than risk a false positive — a
//!     file opening directly with `[section]` would otherwise make `isFlow`
//!     see the `[` and misdetect the root as a bracket-delimited flow
//!     container (the same hazard TOML's tables have, which is why TOML
//!     declares an `insertKey` hook of its own too).
//!   - `sectionDeleteGuard` (over `isSectionHeaderLine`): the `deleteKey`
//!     guard that refuses to line-delete a `[section]` entry — its span is
//!     anchored at the FIRST occurrence's header only (see this module's
//!     sibling `parser.zig`), so a reopened section's later entries would be
//!     orphaned into misparsed content if the "table" were deleted this way.
//!     TOML's `CannotDeleteTable` twin (`CannotDeleteSection` here).
//!
//! Unlike TOML/fig, INI does NOT get its own `set` auto-vivify path — it has
//! no literal spelling for "an empty nested mapping" (`{}` is just a
//! two-character STRING value in INI, not a container), so there is nothing
//! for a seed to splice and `set` refuses rather than write a nonsense
//! `section = {}` root key. That is an ABSENCE of syntax, not logic to
//! delegate, so it is declared as `syntax().empty_map_literal = null` in
//! `ini.zig` — see `manifest.Syntax.empty_map_literal`.

const std = @import("std");
const testing = std.testing;

const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const Span = @import("../../util/span.zig");
const editor = @import("../../editor.zig");
const Ini = @import("ini.zig").Language;

/// The concrete editor these ops drive — the INI arm of the generic engine.
const IniEditor = editor.Editor(Ini);

const lineStartBefore = editor.lineStartBefore;
const firstNonSpace = editor.firstNonSpace;

/// Insert `key_text = value_text` into the mapping at `node` (root or a
/// section) — the same block-mapping primitive JSON/YAML/dotenv/.properties
/// use (`Editor.insertBlockKey`), just reached without the generic `isFlow`
/// check INI doesn't need (see the module doc). `node.kind` must already be
/// `.mapping`; anything else is a real type error, not a container to insert
/// into (e.g. a path landing on a plain scalar key).
///
/// Takes the full `insertKey` hook signature (see `editor.Editor.insertKey`);
/// `path` and `span` are the generic engine's, unused here.
pub fn iniInsertKey(self: *IniEditor, parsed: Document, path: []const AST.PathSegment, node: AST.Node, span: Span, key_text: []const u8, value_text: []const u8) !void {
    _ = path;
    _ = span;
    return switch (node.kind) {
        .mapping => self.insertBlockKey(parsed, node, key_text, value_text),
        else => error.NotAMapping,
    };
}

/// Refuse a line-based delete of a `[section]` entry — INI's twin of TOML's
/// `CannotDeleteTable`.
///
/// The `deleteKeyGuard` hook (see `editor.Editor.deleteKey`). A section's span
/// is anchored at its FIRST occurrence's header only, so deleting that line
/// would orphan a reopened section's later entries into misparsed content.
pub fn sectionDeleteGuard(self: *IniEditor, parsed: Document, node: AST.Node, span: Span) !void {
    _ = parsed;
    _ = node;
    if (isSectionHeaderLine(self.source.items, span)) return error.CannotDeleteSection;
}

/// Whether the entry at `span` is a `[section]` header line — i.e. whether
/// deleting it via the generic line-based `deleteKey` would only remove that
/// one header line and orphan a reopened section's later entries elsewhere
/// in the file. `span.start` may land anywhere on the header line (an INI
/// section-mapping's span is anchored at just its name token, not the
/// header's own extent — see `parser.zig`'s `parseSectionHeader`), so this
/// scans back to the line start first rather than checking `span.start`
/// itself.
pub fn isSectionHeaderLine(source: []const u8, span: Span) bool {
    const fns = firstNonSpace(source, lineStartBefore(source, span.start));
    return fns < source.len and source[fns] == '[';
}

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
