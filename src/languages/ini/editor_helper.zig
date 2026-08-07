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
const lineEndAfter = editor.lineEndAfter;
const firstNonSpace = editor.firstNonSpace;

/// The multi-region machinery INI shares with TOML and fig — everything
/// downstream of the gather below. See `../shared/sections.zig`.
const sections = @import("../shared/sections.zig");
const Region = sections.Region;

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

// ============================================================================
// WHOLE-SECTION STRUCTURAL EDITING (multi-region)
// ============================================================================
//
// What `sectionDeleteGuard` above refuses, these do properly. A `[section]` may
// be REOPENED (`[a]` … `[b]` … `[a]`), which the parser merges into one mapping
// whose span anchors only the FIRST header — so a section's bytes are scattered
// exactly the way a TOML table's or a fig container's are, and the same answer
// applies: gather the disjoint line-regions, rebuild the source once. The
// reopened headers come from `Document.reentry_headers`, which `parser.zig`
// records at its merge branch for this.
//
// INI's gather is the simplest of the three: one level of nesting, no arrays,
// no dotted keys, no flow syntax — a section is its header lines plus its
// entries' lines, with no recursion. Everything after that is shared
// (`../shared/sections.zig`).
//
// There is no `insertContainer`/`renameContainer` twin: a new `[section]` is
// `set`'s business (INI cannot auto-vivify — see the module doc), and a rename
// is one tight span the generic `replaceKeyAtPath` already rewrites, since an
// INI header has no dotted descendants to follow.

/// The physical line of a `[section]` header — the owned comment block above it
/// through the header line's own newline. `content_start` is any position on
/// that line at or after its indent: a section mapping's own span (anchored at
/// the name token inside the brackets) or a recorded re-entry's `content_start`.
fn headerLineRegion(source: []const u8, content_start: usize) Region {
    const ls = lineStartBefore(source, content_start);
    return .{ .start = editor.commentBlockStart(source, ls, .semicolon), .end = lineEndAfter(source, ls) };
}

/// Every region belonging to the section at `path`: its header line, every
/// reopened header line, and each of its entries' own lines.
///
/// `error.NotAContainer` when `path` doesn't name a `[section]` — a root-level
/// scalar key (use `deleteKey`), or a path that resolves to a value rather than
/// a mapping. INI has one level of nesting, so a section is always at the root.
fn gatherSection(parsed: Document, source: []const u8, allocator: std.mem.Allocator, path: []const AST.PathSegment) !struct { node: AST.Node, regions: std.ArrayList(Region) } {
    if (path.len != 1) return error.NotAContainer;
    const node = try parsed.ast.getValByPath(path);
    if (node.kind != .mapping) return error.NotAContainer;

    var regions: std.ArrayList(Region) = .empty;
    errdefer regions.deinit(allocator);
    try regions.append(allocator, headerLineRegion(source, parsed.span(node).start));
    for (parsed.reentry_headers) |rh| {
        if (rh.node_id == node.id) try regions.append(allocator, headerLineRegion(source, rh.content_start));
    }
    var cur = node.kind.mapping;
    while (cur) |id| : (cur = parsed.ast.nodes[id].next_sibling) {
        try regions.append(allocator, sections.entryLineRegion(source, parsed.span(parsed.ast.nodes[id]), .semicolon));
    }
    return .{ .node = node, .regions = regions };
}

/// Delete the whole `[section]` named by `path` — every occurrence of its
/// header plus all of its entries — leaving interleaved foreign sections in
/// place. The op `sectionDeleteGuard` points a `deleteKey` caller at.
pub fn deleteContainer(self: *IniEditor, path: []const AST.PathSegment) !void {
    const parsed = try self.getParsed();
    const source = self.source.items;
    var g = try gatherSection(parsed, source, self.allocator, path);
    defer g.regions.deinit(self.allocator);
    const n = sections.normalize(g.regions.items, true);
    try sections.spliceOut(self, g.regions.items[0..n]);
}

/// Move the whole `[section]` at `src_path` so it begins immediately before the
/// section at `dest_path`, or at end-of-file when `dest_path` is null. A
/// reopened section's fragments are collapsed together at the destination;
/// foreign sections stay put.
pub fn moveContainer(self: *IniEditor, src_path: []const AST.PathSegment, dest_path: ?[]const AST.PathSegment) !void {
    const parsed = try self.getParsed();
    const source = self.source.items;
    var g = try gatherSection(parsed, source, self.allocator, src_path);
    defer g.regions.deinit(self.allocator);
    const n = sections.normalize(g.regions.items, true);

    const dest_at = blk: {
        if (dest_path) |dp| {
            if (dp.len != 1) return error.NotAContainer;
            const dn = try parsed.ast.getValByPath(dp);
            if (dn.kind != .mapping) return error.NotAContainer;
            break :blk headerLineRegion(source, parsed.span(dn).start).start;
        }
        break :blk source.len;
    };
    try sections.relocate(self, g.regions.items[0..n], dest_at);
}

/// Reorder the `[section]`s named by `order` among themselves, each re-emitted
/// contiguously at the position the earliest currently occupies. Sections not
/// named — and any root-level keys above the first section — are untouched.
pub fn reorderContainers(self: *IniEditor, order: []const []const u8) !void {
    if (order.len == 0) return;
    const parsed = try self.getParsed();
    const source = self.source.items;

    var all: std.ArrayList(Region) = .empty;
    defer all.deinit(self.allocator);
    var bundles: std.ArrayList([]u8) = .empty;
    defer {
        for (bundles.items) |b| self.allocator.free(b);
        bundles.deinit(self.allocator);
    }

    for (order) |name| {
        const path: [1]AST.PathSegment = .{.{ .key = name }};
        var g = try gatherSection(parsed, source, self.allocator, &path);
        defer g.regions.deinit(self.allocator);
        const n = sections.normalize(g.regions.items, true);
        const owned = try sections.captureBundle(self.allocator, source, g.regions.items[0..n], &all);
        errdefer self.allocator.free(owned);
        try bundles.append(self.allocator, owned);
    }
    const total = sections.normalize(all.items, true);
    try sections.reorderBundles(self, all.items[0..total], bundles.items);
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

test "ini deleteContainer removes a whole section" {
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[a]\nx = 1\n[b]\ny = 2\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try testing.expectEqualStrings("[b]\ny = 2\n", ed.source.items);
}

test "ini deleteContainer removes EVERY occurrence of a reopened section" {
    // The case `sectionDeleteGuard` refuses a line-delete for: `[a]` is
    // scattered, and its second header is in no node's span. Without
    // `Document.reentry_headers` the trailing `[a]` would survive and adopt
    // whatever followed it.
    var ed: IniEditor = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("[a]\nx = 1\n[b]\ny = 2\n[a]\nz = 3\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try testing.expectEqualStrings("[b]\ny = 2\n", ed.source.items);
}

test "ini deleteContainer removes an EMPTY reopened header too" {
    // A reopen with no entries under it has nothing to find it by except the
    // recorded re-entry — a gather that scanned upward from each child would
    // leave this one behind.
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
