//! TOML-specific editing helpers for `Editor(Toml)`.
//!
//! The generic span-splice engine lives in `../editor.zig`; this module holds the
//! TOML-only logic it delegates to: the ops that have to SPELL something TOML
//! — a `[header]`/`[[header]]` line (`insertTable`, `appendTableToArray`) or
//! every place a table's name is written (`renameTable`, and the
//! `replaceKeyAtPath` hook that routes a block table to it) — plus the
//! `insertKey` hook that lands a new entry inside the intended table's own
//! header region. See `editor.zig` for the public methods.
//!
//! What is NOT here any more is the multi-region gather. A TOML table is
//! assembled from scattered `[header]`, `[[array]]` and dotted lines, and the
//! parser now records every one of those lines per table in
//! `Document.node_regions`; `../../editor/regions.zig` derives a table's whole
//! region set from that, and `deleteContainer`, `moveContainer` and
//! `reorderContainers` — and the four line-splice refusals
//! (`CannotDeleteTable`, `CannotReplaceTable`, `CannotMoveTable`,
//! `CannotReorderTables`) — are generic in `editor.zig`. The rename below is
//! the one consumer of that region set left here, because it addresses each
//! header region's own start to rewrite the segment inside it.

const std = @import("std");

const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const Span = @import("../../util/span.zig");
const editor = @import("../../editor.zig");
const Toml = @import("toml.zig").Language;
const log = std.log.scoped(.editor);

/// The concrete editor these structural ops drive. All functions below take a
/// `*TomlEditor`: they are the TOML arm of the generic engine, factored out so
/// `editor.zig` stays format-agnostic and every TOML edit lives in one file. The
/// public methods on `editor.Editor(Toml)` are thin wrappers that call these.
const TomlEditor = editor.Editor(Toml);

// Shared source-coordinate / rendering utilities (defined in editor.zig).
const lineStartBefore = editor.lineStartBefore;
const lineEndAfter = editor.lineEndAfter;
const firstNonSpace = editor.firstNonSpace;
const isFlow = editor.isFlow;

/// The engine's derived-region type — `renameTableSegments` reads each header
/// region's start out of the set `Editor.gatherRegions` hands back.
const Region = @import("../../editor/regions.zig").Region;

/// Span of the dotted-key segment at `depth` (0-based) within the header line of
/// `region` (`[a.b.c]` or `[[a.b.c]]`), or null when the region has no header
/// line or fewer than `depth+1` segments. The span covers the raw key token
/// (including any surrounding quotes), so splicing a rendered replacement over it
/// rewrites just that one path segment.
pub fn headerSegmentSpan(source: []const u8, region: Region, depth: usize) ?Span {
    // Locate the `[`-line inside the region (comments may precede it).
    var ls = region.start;
    const line_start = while (ls < region.end) : (ls = lineEndAfter(source, ls)) {
        const fns = firstNonSpace(source, ls);
        if (fns < source.len and source[fns] == '[') break fns;
    } else return null;

    var p = line_start;
    while (p < source.len and source[p] == '[') p += 1; // skip `[` or `[[`
    return keySegmentSpan(source, p, depth, ']');
}

/// Span of the `index`-th (0-based) segment of the dotted key path starting at
/// `p0`, or null when the path has fewer than `index+1` segments. `terminator`
/// ends the path: `]` for a `[a.b.c]` header, `=` for an `a.b.c = v` line. The
/// span covers the raw key token (quotes included), so splicing over it rewrites
/// exactly one path segment and nothing else on the line.
///
/// Shared by the two callers because a TOML key path is spelled the same in both
/// places — bare, `"basic"` or `'literal'` segments joined by `.`, spaces
/// allowed around each. Stops at a newline as well as at `terminator`, so a
/// malformed line can't run the scan into the rest of the file.
fn keySegmentSpan(source: []const u8, p0: usize, index: usize, terminator: u8) ?Span {
    var p = p0;
    var seg: usize = 0;
    while (p < source.len and source[p] != terminator and source[p] != '\n') {
        while (p < source.len and (source[p] == ' ' or source[p] == '\t')) p += 1;
        if (p >= source.len or source[p] == terminator or source[p] == '\n') break;
        const start = p;
        switch (source[p]) {
            '"' => {
                p += 1;
                while (p < source.len and source[p] != '"') : (p += 1) {
                    if (source[p] == '\\') p += 1;
                }
                if (p < source.len) p += 1; // closing quote
            },
            '\'' => {
                p += 1;
                while (p < source.len and source[p] != '\'') p += 1;
                if (p < source.len) p += 1; // closing quote
            },
            else => while (p < source.len and isTomlBareKey(source[p .. p + 1])) : (p += 1) {},
        }
        const end = p;
        if (end == start) break; // no key token here — malformed; don't spin
        if (seg == index) return Span.init(start, end);
        seg += 1;
        while (p < source.len and (source[p] == ' ' or source[p] == '\t')) p += 1;
        if (p < source.len and source[p] == '.') p += 1; // segment separator
    }
    return null;
}

/// The 0-based position of the key token starting at `key_start` within the
/// dotted key path on its own line: 0 for `a` in `a.b = 1`, 1 for that line's
/// `b`. Null when `key_start` names no segment of a `key = value` line — which
/// is how a `[header]` line answers, since its path is scanned by
/// `headerSegmentSpan` instead.
///
/// This is what lets a dotted rename find its segment without tracking parser
/// state: a dotted key path is spelled relative to the enclosing `[header]`, so
/// the index of a given table within it cannot be derived from the AST path
/// alone (`[t]` + `a.b = 1` puts `t.a` at index 0, not 1) — but it is right
/// there in the source.
fn dottedIndexOfKey(source: []const u8, key_start: usize) ?usize {
    const p0 = firstNonSpace(source, lineStartBefore(source, key_start));
    if (p0 >= source.len or source[p0] == '[') return null;
    var i: usize = 0;
    while (keySegmentSpan(source, p0, i, '=')) |seg| : (i += 1) {
        if (seg.start == key_start) return i;
        if (seg.start > key_start) return null;
    }
    return null;
}

/// Render a TOML header path (`a.b.c`) from a PathSegment list into `out`. Index
/// segments are skipped — `[[a.b]]` always targets `a`'s last element, so the
/// index is implied. Each key prints bare when it is all `[A-Za-z0-9_-]`, else as
/// a basic-quoted string.
pub fn appendTomlHeaderPath(out: *std.ArrayList(u8), allocator: std.mem.Allocator, path: []const AST.PathSegment) !void {
    var first = true;
    for (path) |seg| switch (seg) {
        .index => {},
        .key => |k| {
            if (!first) try out.append(allocator, '.');
            first = false;
            if (isTomlBareKey(k)) {
                try out.appendSlice(allocator, k);
            } else {
                try out.append(allocator, '"');
                for (k) |ch| switch (ch) {
                    '"' => try out.appendSlice(allocator, "\\\""),
                    '\\' => try out.appendSlice(allocator, "\\\\"),
                    else => try out.append(allocator, ch),
                };
                try out.append(allocator, '"');
            }
        },
    };
}

pub fn isTomlBareKey(name: []const u8) bool {
    if (name.len == 0) return false;
    for (name) |c| {
        const ok = (c >= 'A' and c <= 'Z') or (c >= 'a' and c <= 'z') or
            (c >= '0' and c <= '9') or c == '_' or c == '-';
        if (!ok) return false;
    }
    return true;
}

// ============================================================================
// STRUCTURAL EDITING — the TOML arm of `editor.Editor`
// ============================================================================
//
// These drive the editor's splice engine (`self.replaceAtSpan`, which reparses
// and rolls back on failure). They are reached two different ways:
//
//   * The SHARED ops — `insertKey`, `replaceKeyAtPath` — are HOOKS: `toml.zig`
//     declares each on its `Language` and the generic engine dispatches on
//     `@hasDecl`, naming no format. See that file's "Editing hooks" block.
//   * The whole-table ops that SPELL a header (`insertTable`,
//     `appendTableToArray`) or rewrite a name (`renameTable`) have no generic
//     counterpart to override, but are declared and dispatched the same way —
//     `toml.zig`'s "Whole-container ops" block maps each to the shared name
//     `editor.zig` exposes it under (`insertTable` → `insertContainer`). The
//     TOML words stay here, where the `[header]` reasoning they describe
//     lives. Delete, move and reorder need no TOML words at all: they are
//     the engine's, over `Document.node_regions`.

/// Rename the key at `path`, routing a BLOCK table to the multi-line rename.
///
/// The `replaceKeyAtPath` hook (see `editor.Editor.replaceKeyAtPath`). The
/// generic op splices over the key node's span, which for a TOML table is the
/// ONE place its name has a node — its first `[header]` or dotted mention. Every
/// other place it is written (`[a.b]` sub-headers, further `[[a]]` elements,
/// sibling dotted lines) would keep the old name, and since what stays behind
/// still parses, the rename SPLIT the table in two and reported success:
/// `[a]`/`[a.b]` renamed to `q` left `[q]` holding `a`'s scalars while `[a.b]`
/// re-created `a` around `b`.
///
/// `renameTableSegments` is that operation done over every mention at once;
/// `replacement` is key syntax in both, so it passes straight through. A scalar,
/// an inline table and an inline array keep the generic splice below: their key
/// is written exactly once, so its span IS the whole rename.
pub fn tomlReplaceKey(self: *TomlEditor, parsed: Document, path: []const AST.PathSegment, replacement: []const u8) !void {
    if (path.len > 0) {
        if (parsed.ast.getValByPath(path)) |node| {
            // A block table of any kind — `[header]`, dotted, `[[array]]` —
            // is a section node; an inline table or array is not.
            if (parsed.isSection(node)) return renameTableSegments(self, path, replacement);
        } else |_| {}
    }
    const key = try parsed.ast.getKeyByPath(path);
    try self.replaceAtSpan(parsed.span(key), replacement);
}

// --- TOML structural inserts ---
//
// TOML splits a logical table across `[header]`…dotted-key…lines, so an insert
// must land where the new entry attaches to the *intended* table. A scalar
// `key = value` is placed at the end of the table's own header region — after
// its last direct (non-`[header]`) entry, before any sub-table header opens —
// never after a sub-table, which would silently reparent it. `key_text`/
// `value_text` are verbatim TOML literals.
//
// Takes the full `insertKey` hook signature (see `editor.Editor.insertKey`).
pub fn tomlInsertKey(self: *TomlEditor, parsed: Document, path: []const AST.PathSegment, node: AST.Node, span: Span, key_text: []const u8, value_text: []const u8) !void {
    // An empty path is the document root — the one table with no `[header]`
    // line to insert after.
    const is_root = path.len == 0;
    if (node.kind != .mapping) return error.NotAMapping;
    const source = self.source.items;
    // Inline table `{ … }`: splice a `key = value` inside the braces. The root
    // is never one — its span is the whole document, so `isFlow` would read the
    // leading `[` of a header-first file (`[package]` in every Cargo.toml) as an
    // opening delimiter and splice the new entry into that header.
    if (!is_root and isFlow(source, span))
        return tomlInsertFlowEntry(self, parsed, node, span, key_text, value_text);

    // Block table: scan its direct children for the in-region ones (those whose
    // line does not start with `[` — i.e. scalars, arrays, inline tables, and
    // dotted sub-tables, all of which live under this table's header). The last
    // such child's line is where the new entry goes; its column sets the indent.
    var last_end: ?usize = null;
    var col: usize = 0;
    var col_set = false;
    var cur = node.kind.mapping;
    while (cur) |id| : (cur = parsed.ast.nodes[id].next_sibling) {
        const kv_span = parsed.span(parsed.ast.nodes[id]);
        const ls = lineStartBefore(source, kv_span.start);
        const fns = firstNonSpace(source, ls);
        if (fns < source.len and source[fns] == '[') continue; // sub-table header: out of region
        last_end = kv_span.end;
        if (!col_set) {
            col = fns - ls;
            col_set = true;
        }
    }

    const insert_at = if (last_end) |e|
        lineEndAfter(source, e -| 1)
    else if (is_root)
        0 // empty document: top of file
    else
        lineEndAfter(source, span.end -| 1); // header-only table: just past its `[header]` line

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(self.allocator);
    if (insert_at > 0 and source[insert_at - 1] != '\n') try out.append(self.allocator, '\n');
    try out.appendNTimes(self.allocator, ' ', col);
    try out.appendSlice(self.allocator, key_text);
    try out.appendSlice(self.allocator, " = ");
    try out.appendSlice(self.allocator, value_text);
    try out.append(self.allocator, '\n');
    try self.replaceAtSpan(Span.init(insert_at, insert_at), out.items);
}

/// Splice `key = value` into an inline table, keeping the conventional
/// `{ a = 1, b = 2 }` spacing: into a non-empty table the entry is inserted
/// right after the last entry's value (`…1` → `…1, key = value`); into an
/// empty `{}` it is padded with surrounding spaces (`{ key = value }`).
pub fn tomlInsertFlowEntry(self: *TomlEditor, parsed: Document, node: AST.Node, span: Span, key_text: []const u8, value_text: []const u8) !void {
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(self.allocator);
    const at = if (node.kind.mapping) |first| blk: {
        var last = first;
        while (parsed.ast.nodes[last].next_sibling) |n| last = n;
        try out.appendSlice(self.allocator, ", ");
        break :blk parsed.span(parsed.ast.nodes[last]).end;
    } else blk: {
        try out.append(self.allocator, ' ');
        break :blk span.start + 1; // just after '{'
    };
    try out.appendSlice(self.allocator, key_text);
    try out.appendSlice(self.allocator, " = ");
    try out.appendSlice(self.allocator, value_text);
    if (node.kind.mapping == null) try out.append(self.allocator, ' ');
    try self.replaceAtSpan(Span.init(at, at), out.items);
}

/// Append a new `[[header]]` element to the array-of-tables at `path`, with
/// `body_text` (verbatim TOML `key = value` lines, possibly empty) as its
/// contents. The element is spliced after the AoT's current last element — past
/// every line of that element's subtree, so a nested sub-table inside it is not
/// split.
pub fn appendTableToArray(self: *TomlEditor, path: []const AST.PathSegment, body_text: []const u8) !void {
    const parsed = try self.getParsed();
    const node = try parsed.ast.getValByPath(path);
    if (node.kind != .sequence) return error.NotAnArrayOfTables;
    var elem = node.kind.sequence orelse return error.NotAnArrayOfTables;
    var last_elem = elem;
    while (true) {
        if (parsed.ast.nodes[elem].kind != .mapping) return error.NotAnArrayOfTables;
        last_elem = elem;
        elem = parsed.ast.nodes[elem].next_sibling orelse break;
    }
    const source = self.source.items;
    // Past the last element's whole derived extent — its `[[…]]` line, its
    // entries and every nested `[a.b]` sub-table, wherever they sit.
    const insert_at = try self.sectionExtentEnd(parsed, parsed.ast.nodes[last_elem]);

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(self.allocator);
    if (insert_at > 0 and source[insert_at - 1] != '\n') try out.append(self.allocator, '\n');
    try out.append(self.allocator, '\n'); // blank line before the new header
    try out.appendSlice(self.allocator, "[[");
    try appendTomlHeaderPath(&out, self.allocator, path);
    try out.appendSlice(self.allocator, "]]\n");
    if (body_text.len > 0) {
        try out.appendSlice(self.allocator, body_text);
        if (body_text[body_text.len - 1] != '\n') try out.append(self.allocator, '\n');
    }
    try self.replaceAtSpan(Span.init(insert_at, insert_at), out.items);
}

/// Create a new `[path]` table (or sub-table) whose body is `body_text`
/// (verbatim TOML `key = value` lines, possibly empty). The header is spliced
/// *after* the parent table's entire subtree — or at end-of-file for a
/// root-level table — so no existing key is reparented. Refuses
/// `error.TableExists` if the table already exists.
pub fn insertTable(self: *TomlEditor, path: []const AST.PathSegment, body_text: []const u8) !void {
    if (path.len == 0) return error.NotATable;
    const parsed = try self.getParsed();
    if (parsed.ast.getValByPath(path)) |_| {
        return error.TableExists;
    } else |_| {}
    const source = self.source.items;

    // Insertion point: just past the parent table's whole derived extent
    // (header, entries and sub-tables wherever they sit), else EOF.
    const insert_at = blk: {
        if (path.len > 1) {
            if (parsed.ast.getValByPath(path[0 .. path.len - 1])) |parent| {
                if (parent.kind == .mapping)
                    break :blk try self.sectionExtentEnd(parsed, parent);
            } else |_| {}
        }
        break :blk source.len;
    };

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(self.allocator);
    if (insert_at > 0 and source[insert_at - 1] != '\n') try out.append(self.allocator, '\n');
    if (insert_at > 0) try out.append(self.allocator, '\n'); // blank line before the header
    try out.appendSlice(self.allocator, "[");
    try appendTomlHeaderPath(&out, self.allocator, path);
    try out.appendSlice(self.allocator, "]\n");
    if (body_text.len > 0) {
        try out.appendSlice(self.allocator, body_text);
        if (body_text[body_text.len - 1] != '\n') try out.append(self.allocator, '\n');
    }
    try self.replaceAtSpan(Span.init(insert_at, insert_at), out.items);
}

/// Rename the leaf key of the table at `path` to `new_leaf` — the
/// `renameContainer` op. `new_leaf` is a LOGICAL key name, rendered into TOML key
/// syntax (quoted if it needs to be) before it is spliced; `renameTableSegments`
/// is the same operation taking pre-rendered syntax.
pub fn renameTable(self: *TomlEditor, path: []const AST.PathSegment, new_leaf: []const u8) !void {
    var rendered: std.ArrayList(u8) = .empty;
    defer rendered.deinit(self.allocator);
    try appendTomlHeaderPath(&rendered, self.allocator, &.{.{ .key = new_leaf }});
    return renameTableSegments(self, path, rendered.items);
}

/// Rewrite every source segment that names the table at `path` to `rendered`
/// (TOML key syntax, spliced verbatim), leaving the rest of each line alone.
///
/// A table's name is written in as many places as TOML has ways to name it, and a
/// rename that misses one does not fail — it SPLITS the table in two, since what
/// is left behind still parses as a table of the old name:
///
///   * its own header and every descendant sub-header that shares the prefix —
///     `[a]`, `[a.b]`, `[[a.c]]` all carry `a` at the same segment index, which
///     is `path`'s own key depth (AoT indices don't appear in headers);
///   * every DOTTED line under it — `a.b = 1`, `a.c = 2` name `a` twice, and only
///     the first has a node whose span points at it. Their index is NOT the path
///     depth: a dotted key is spelled relative to the enclosing `[header]`, so
///     `[t]` + `a.b = 1` puts `t.a` at index 0. It is read off the source per line
///     instead (`dottedIndexOfKey`), walking down from this table through dotted
///     levels only — a header boundary ends the prefix, so `[a]` + `b.c = 1` has
///     no `a` on the entry line and is correctly left alone.
///
/// Format-preserving: only the renamed segments change. A collision with an
/// existing sibling is rejected by the reparse rollback (`error.DuplicateKey`),
/// and a table whose name is nowhere to be found is refused rather than reported
/// as a rename that did nothing.
pub fn renameTableSegments(self: *TomlEditor, path: []const AST.PathSegment, rendered: []const u8) !void {
    if (path.len == 0) return error.NotATable;
    const parsed = try self.getParsed();
    const node = try parsed.ast.getValByPath(path);
    // Only a block table has headers or dotted lines to rewrite; an inline
    // table's key is a plain key (`replaceKeyAtPath` splices it), and so is a
    // scalar's.
    if (!parsed.isSection(node)) return error.NotATable;
    const source = self.source.items;

    // Depth of the renamed segment within each header (count of key segments
    // before the leaf; AoT indices don't appear in headers).
    var depth: usize = 0;
    for (path[0 .. path.len - 1]) |seg| switch (seg) {
        .key => depth += 1,
        .index => {},
    };

    var spans: std.ArrayList(Span) = .empty;
    defer spans.deinit(self.allocator);

    // Header lines: take the engine's derived regions for the subtree and the
    // segment at `depth` from each `[`-line among them (a region may open with
    // an owned comment block, so the scan locates the header line inside it).
    // Coalesced on OVERLAP only: each header region's own start is addressed
    // here, which a region merged with a touching neighbour would hide.
    var regions = try self.gatherRegions(parsed, node, false);
    defer regions.deinit(self.allocator);
    for (regions.items) |r| {
        if (headerSegmentSpan(source, r, depth)) |seg| try spans.append(self.allocator, seg);
    }

    // Dotted lines: only a mapping can have them — an AoT is spelled `[[…]]` and
    // so is named in headers alone.
    if (node.kind == .mapping) try appendDottedNameSpans(parsed, source, self.allocator, node, 0, &spans);

    if (spans.items.len == 0) return error.NotATable;
    std.mem.sort(Span, spans.items, {}, struct {
        fn lessThan(_: void, a: Span, b: Span) bool {
            return a.start < b.start;
        }
    }.lessThan);

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(self.allocator);
    var pos: usize = 0;
    for (spans.items) |seg| {
        if (seg.start < pos) continue; // same segment reached twice; splice once
        try out.appendSlice(self.allocator, source[pos..seg.start]);
        try out.appendSlice(self.allocator, rendered);
        pos = seg.end;
    }
    try out.appendSlice(self.allocator, source[pos..]);
    try self.replaceAtSpan(Span.init(0, source.len), out.items);
}

/// Append the span naming `node` on every dotted line beneath it, descending
/// through dotted levels only. `level` is how many dotted segments separate
/// `node` from the children being walked, so a child's key at dotted index `i`
/// puts `node` at `i - 1 - level` — negative when the child's line does not spell
/// `node` at all (`[a]` + `x = 1`, or a `[header]` child), which is skipped.
///
/// Recursing per dotted LEVEL rather than per line is what covers the sibling
/// case: `a.b.c = 1` / `a.b.d = 2` are two lines both naming `a` and `b`, but only
/// the first line's keys have nodes of their own — the second is reached as a
/// child of `b`.
fn appendDottedNameSpans(
    parsed: Document,
    source: []const u8,
    allocator: std.mem.Allocator,
    node: AST.Node,
    level: usize,
    out: *std.ArrayList(Span),
) std.mem.Allocator.Error!void {
    var cur = node.kind.mapping;
    while (cur) |id| : (cur = parsed.ast.nodes[id].next_sibling) {
        const kv = parsed.ast.nodes[id];
        const key_start = parsed.span(parsed.ast.nodes[kv.kind.keyvalue.key]).start;
        const idx = dottedIndexOfKey(source, key_start) orelse continue; // `[header]` child
        if (idx >= level + 1) {
            if (keySegmentSpan(source, firstNonSpace(source, lineStartBefore(source, key_start)), idx - 1 - level, '=')) |seg|
                try out.append(allocator, seg);
        }
        // A dotted intermediate continues the prefix onto its own children's
        // lines; a flow container or a scalar ends it.
        const val = parsed.ast.nodes[kv.kind.keyvalue.value];
        if (val.kind == .mapping and !isFlow(source, parsed.span(val)))
            try appendDottedNameSpans(parsed, source, allocator, val, level + 1, out);
    }
}

// =======
// TESTS
// =======
//
// TOML editor tests live here (rather than in editor.zig) so each language's
// editing tests sit next to that language's helpers. They exercise the public
// `Editor(Toml)` surface end-to-end: point edits (value/key replacement on the
// contiguous spans every node keeps even in a scattered table), scalar/inline
// insert+delete, the section refusals, and the whole-table structural ops
// (delete/insert/rename/move/reorder) — the generic ones pinned here in TOML's
// own shapes, since this is where the `[header]` cases live.

fn newTomlEditor(input: []const u8) !editor.Editor(Toml) {
    var ed: editor.Editor(Toml) = .{ .allocator = std.testing.allocator };
    try ed.init(input);
    return ed;
}

fn expectTomlSource(ed: *const editor.Editor(Toml), expected: []const u8) !void {
    errdefer log.err("actual:   \"{s}\"", .{ed.source.items});
    errdefer log.err("expected: \"{s}\"", .{expected});
    try std.testing.expectEqualStrings(expected, ed.source.items);
}

test "toml replace root scalar value" {
    var ed = try newTomlEditor("title = \"old\"\nport = 8080\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "port" }}, "9090");
    try expectTomlSource(&ed, "title = \"old\"\nport = 9090\n");
}

test "toml replace string value keeps quoting verbatim" {
    var ed = try newTomlEditor("title = \"old\"\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "title" }}, "\"new title\"");
    try expectTomlSource(&ed, "title = \"new title\"\n");
}

test "toml replace value in a table" {
    var ed = try newTomlEditor("[server]\nhost = \"a\"\nport = 1\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{ .{ .key = "server" }, .{ .key = "port" } }, "2");
    try expectTomlSource(&ed, "[server]\nhost = \"a\"\nport = 2\n");
}

test "toml replace value through scattered table headers" {
    var ed = try newTomlEditor("[a]\nx = 1\n[a.b]\ny = 2\n[a.c]\nz = 3\n");
    defer ed.deinit();
    // The owning table `a` spans the whole file (it nests b and c), but the
    // value node's span is contiguous, so the point edit is exact.
    try ed.replaceValAtPath(&.{ .{ .key = "a" }, .{ .key = "b" }, .{ .key = "y" } }, "99");
    try expectTomlSource(&ed, "[a]\nx = 1\n[a.b]\ny = 99\n[a.c]\nz = 3\n");
}

test "toml replace dotted-key value" {
    var ed = try newTomlEditor("a.b.c = 1\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{ .{ .key = "a" }, .{ .key = "b" }, .{ .key = "c" } }, "2");
    try expectTomlSource(&ed, "a.b.c = 2\n");
}

test "toml replace value with an inline array" {
    var ed = try newTomlEditor("ports = [1, 2]\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "ports" }}, "[3, 4, 5]");
    try expectTomlSource(&ed, "ports = [3, 4, 5]\n");
}

test "toml replace value with an inline table" {
    var ed = try newTomlEditor("t = { a = 1 }\n");
    defer ed.deinit();
    // An inline container's span IS its `{ … }` text, so it splices in place —
    // the shape `tableReplaceGuard` deliberately lets through.
    try ed.replaceValAtPath(&.{.{ .key = "t" }}, "{ z = 2 }");
    try expectTomlSource(&ed, "t = { z = 2 }\n");
}

test "toml replace at the document root rewrites the whole document" {
    var ed = try newTomlEditor("k = 1\n");
    defer ed.deinit();
    // The root's span is the whole file, so replacing it is exact — the empty
    // path is the one container `tableReplaceGuard` exempts.
    try ed.replaceValAtPath(&.{}, "z = 2\n");
    try expectTomlSource(&ed, "z = 2\n");
}

// --- the section refusals on replace (block tables are NOT value slots) ---
//
// A block table's node span is its KEY segment — the `nested` inside
// `[nested]`, the `a` in `a.b = 1` — because that is the only contiguous text a
// scattered table owns. Splicing a replacement there would rewrite the table's
// NAME and, for a string replacement, still reparse: `[nested]` became
// `["REPLACED"]`, silently renaming the section and rehoming its body while
// reporting success. Every such shape now refuses, source untouched.

test "toml replace refuses a [header] table (would rename the header)" {
    var ed = try newTomlEditor("[nested]\nk = \"v\"\n");
    defer ed.deinit();
    try std.testing.expectError(
        error.CannotReplaceTable,
        ed.replaceValAtPath(&.{.{ .key = "nested" }}, "\"REPLACED\""),
    );
    try expectTomlSource(&ed, "[nested]\nk = \"v\"\n");
}

test "toml replace refuses a dotted table" {
    var ed = try newTomlEditor("a.b = 1\n");
    defer ed.deinit();
    // Not a `[header]` line at all — the line starts with the key — so this is
    // the case a line-based sniff would miss and splice into `"x".b = 1`.
    try std.testing.expectError(
        error.CannotReplaceTable,
        ed.replaceValAtPath(&.{.{ .key = "a" }}, "\"REPLACED\""),
    );
    try expectTomlSource(&ed, "a.b = 1\n");
}

test "toml replace refuses an array of tables and its elements" {
    var ed = try newTomlEditor("[[aot]]\nk = 1\n");
    defer ed.deinit();
    // The `[[aot]]` sequence and every element share the header key's span, so
    // both paths are the same hazard.
    try std.testing.expectError(
        error.CannotReplaceTable,
        ed.replaceValAtPath(&.{.{ .key = "aot" }}, "[1, 2]"),
    );
    try std.testing.expectError(
        error.CannotReplaceTable,
        ed.replaceValAtPath(&.{ .{ .key = "aot" }, .{ .index = 0 } }, "{ z = 1 }"),
    );
    try expectTomlSource(&ed, "[[aot]]\nk = 1\n");
}

test "toml set on an existing [header] table refuses without touching the file" {
    var ed = try newTomlEditor("[nested]\nk = \"v\"\n");
    defer ed.deinit();
    // `set` falls back to `insertKey` on ANY replace error, so the guard has to
    // leave the document byte-for-byte intact through that second attempt too
    // (the insert's own reparse would hit TOML's duplicate-key rule).
    try std.testing.expectError(
        error.CannotReplaceTable,
        ed.set(&.{.{ .key = "nested" }}, "\"REPLACED\""),
    );
    try expectTomlSource(&ed, "[nested]\nk = \"v\"\n");
}

test "toml rename a leaf key" {
    var ed = try newTomlEditor("[server]\nport = 8080\n");
    defer ed.deinit();
    try ed.replaceKeyAtPath(&.{ .{ .key = "server" }, .{ .key = "port" } }, "listen_port");
    try expectTomlSource(&ed, "[server]\nlisten_port = 8080\n");
}

test "toml failed edit rolls back and keeps editor usable" {
    var ed = try newTomlEditor("a = 1\nb = 2\n");
    defer ed.deinit();
    // An unterminated array fails to reparse; the source must be restored.
    if (ed.replaceValAtPath(&.{.{ .key = "a" }}, "[oops")) |_| {
        return error.TestExpectedFailedEdit;
    } else |_| {}
    try expectTomlSource(&ed, "a = 1\nb = 2\n");
    try ed.replaceValAtPath(&.{.{ .key = "a" }}, "9");
    try expectTomlSource(&ed, "a = 9\nb = 2\n");
}

// --- TOML structural editing (insert/delete scalar keys, inline arrays, AoT append) ---
//
// Format-preserving via spans; the genuinely scattered cases (whole-table
// delete/move, non-contiguous tables) refuse with a clear error.

test "toml insert key into root" {
    var ed = try newTomlEditor("a = 1\nb = 2\n");
    defer ed.deinit();
    try ed.insertKey(&.{}, "c", "3");
    try expectTomlSource(&ed, "a = 1\nb = 2\nc = 3\n");
}

test "toml insert key into empty document" {
    var ed = try newTomlEditor("");
    defer ed.deinit();
    try ed.insertKey(&.{}, "a", "1");
    try expectTomlSource(&ed, "a = 1\n");
}

test "toml insert root key goes above the first header" {
    // The new root key must land in root's own region — before `[t]` opens —
    // not after the table (which would reparent it into `[t]`).
    var ed = try newTomlEditor("x = 1\n[t]\ny = 2\n");
    defer ed.deinit();
    try ed.insertKey(&.{}, "z", "3");
    try expectTomlSource(&ed, "x = 1\nz = 3\n[t]\ny = 2\n");
}

test "toml insert root key into a document that OPENS with a header" {
    // The root's span is the whole document, so its first byte is the `[` of
    // `[t]` — which the generic `isFlow` sniff read as an inline table's opening
    // delimiter, splicing the new entry into the header itself (`[t, z = 3]`)
    // and failing the reparse with `BadKey`. Every header-first file was
    // affected, which is to say every Cargo.toml.
    var ed = try newTomlEditor("[t]\ny = 2\n");
    defer ed.deinit();
    try ed.insertKey(&.{}, "z", "3");
    try expectTomlSource(&ed, "z = 3\n[t]\ny = 2\n");
}

test "toml insert root key into a document that opens with an array-of-tables" {
    // `[[bin]]` is the same hazard with a doubled delimiter.
    var ed = try newTomlEditor("[[bin]]\nname = \"a\"\n");
    defer ed.deinit();
    try ed.insertKey(&.{}, "z", "3");
    try expectTomlSource(&ed, "z = 3\n[[bin]]\nname = \"a\"\n");
}

test "toml insert key into a table" {
    var ed = try newTomlEditor("[server]\nhost = \"a\"\nport = 1\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "server" }}, "tls", "true");
    try expectTomlSource(&ed, "[server]\nhost = \"a\"\nport = 1\ntls = true\n");
}

test "toml insert into a table that has a sub-table inserts before the sub-header" {
    var ed = try newTomlEditor("[a]\nx = 1\n[a.b]\ny = 2\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "a" }}, "w", "9");
    try expectTomlSource(&ed, "[a]\nx = 1\nw = 9\n[a.b]\ny = 2\n");
}

test "toml insert into a header-only table" {
    var ed = try newTomlEditor("[a]\n[a.b]\ny = 2\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "a" }}, "x", "1");
    try expectTomlSource(&ed, "[a]\nx = 1\n[a.b]\ny = 2\n");
}

test "toml insert preserves the column of existing entries" {
    var ed = try newTomlEditor("[a]\n  x = 1\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "a" }}, "y", "2");
    try expectTomlSource(&ed, "[a]\n  x = 1\n  y = 2\n");
}

test "toml insert into an inline table" {
    var ed = try newTomlEditor("p = { x = 1 }\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "p" }}, "y", "2");
    try expectTomlSource(&ed, "p = { x = 1, y = 2 }\n");
}

test "toml insert into an empty inline table" {
    var ed = try newTomlEditor("p = {}\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "p" }}, "x", "1");
    try expectTomlSource(&ed, "p = { x = 1 }\n");
}

test "toml insert duplicate key rolls back" {
    var ed = try newTomlEditor("a = 1\n");
    defer ed.deinit();
    try std.testing.expectError(error.DuplicateKey, ed.insertKey(&.{}, "a", "2"));
    try expectTomlSource(&ed, "a = 1\n");
}

test "toml delete scalar key" {
    var ed = try newTomlEditor("a = 1\nb = 2\nc = 3\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectTomlSource(&ed, "a = 1\nc = 3\n");
}

test "toml delete key with owned comment" {
    var ed = try newTomlEditor("a = 1\n# note\nb = 2\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectTomlSource(&ed, "a = 1\n");
}

test "toml delete key inside a table" {
    var ed = try newTomlEditor("[t]\nx = 1\ny = 2\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "t" }, .{ .key = "x" } });
    try expectTomlSource(&ed, "[t]\ny = 2\n");
}

test "toml delete an inline-table-valued key" {
    var ed = try newTomlEditor("a = 1\np = { x = 1, y = 2 }\nb = 2\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "p" }});
    try expectTomlSource(&ed, "a = 1\nb = 2\n");
}

// Regression: an inline table's entries are comma-separated on one physical
// line, not one-per-line like a block table, so `deleteKey`'s generic
// line-based delete (built for the block shape) used to delete the whole
// containing line — here, the entire (single-line) document — leaving an
// empty file that TOML's empty-document-is-an-empty-table grammar then
// accepted, silently committing the data loss instead of erroring. Deleting a
// key *inside* a packed inline table must only remove that key.
test "toml delete key inside a packed inline table (regression)" {
    var ed = try newTomlEditor("point = { x = 1, y = 2 }\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "point" }, .{ .key = "y" } });
    try expectTomlSource(&ed, "point = { x = 1 }\n");
}

test "toml delete first key of a packed inline table" {
    var ed = try newTomlEditor("point = { x = 1, y = 2 }\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "point" }, .{ .key = "x" } });
    try expectTomlSource(&ed, "point = { y = 2 }\n");
}

// Regression: deleting the *only* key of a single-entry inline table must leave
// an empty inline table `{}`, not delete the braces with the line. The old
// block-shaped line delete wiped the whole `point = { x = 1 }` line, which
// TOML's empty-document-is-an-empty-table grammar then silently accepted,
// committing the data loss to disk instead of preserving `point = {}`.
test "toml delete only key of a single-entry inline table (regression)" {
    var ed = try newTomlEditor("point = { x = 1 }\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "point" }, .{ .key = "x" } });
    try expectTomlSource(&ed, "point = { }\n");
}

// Regression: deleting the *last* key of a one-entry-per-line inline table used
// to strand the predecessor's separator comma before the closing brace — which
// TOML forbids (no trailing comma in an inline table). The flow-aware splice
// drops the preceding comma instead.
test "toml delete last key of a multi-line inline table (regression)" {
    var ed = try newTomlEditor("point = {\n  x = 1,\n  y = 2\n}\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "point" }, .{ .key = "y" } });
    try expectTomlSource(&ed, "point = {\n  x = 1\n}\n");
}

test "toml delete dotted key removes the line" {
    var ed = try newTomlEditor("a.b.c = 1\na.b.d = 2\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "a" }, .{ .key = "b" }, .{ .key = "c" } });
    try expectTomlSource(&ed, "a.b.d = 2\n");
}

test "toml deleting a header table is refused" {
    var ed = try newTomlEditor("[a]\nx = 1\n[a.b]\ny = 2\n");
    defer ed.deinit();
    try std.testing.expectError(error.CannotDeleteTable, ed.deleteKey(&.{ .{ .key = "a" }, .{ .key = "b" } }));
    try expectTomlSource(&ed, "[a]\nx = 1\n[a.b]\ny = 2\n");
}

test "toml deleting an array-of-tables is refused" {
    var ed = try newTomlEditor("[[fruit]]\nname = \"apple\"\n");
    defer ed.deinit();
    try std.testing.expectError(error.CannotDeleteTable, ed.deleteKey(&.{.{ .key = "fruit" }}));
}

test "toml inline array append/prepend/remove" {
    var ed = try newTomlEditor("ports = [1, 2]\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "ports" }}, "3");
    try expectTomlSource(&ed, "ports = [1, 2, 3]\n");
    try ed.prependToSeq(&.{.{ .key = "ports" }}, "0");
    try expectTomlSource(&ed, "ports = [0, 1, 2, 3]\n");
    try ed.removeSeqItem(&.{.{ .key = "ports" }}, 2);
    try expectTomlSource(&ed, "ports = [0, 1, 3]\n");
}

test "toml inline array append with pre-existing trailing comma" {
    // A trailing comma before ']' is legal TOML inline-array syntax;
    // appending must not double it into an empty element that fails to
    // reparse.
    var ed = try newTomlEditor("ports = [1, 2,]\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "ports" }}, "3");
    try expectTomlSource(&ed, "ports = [1, 2, 3,]\n");
}

test "toml inline array append onto a multi-line one-item-per-line array" {
    var ed = try newTomlEditor("ports = [\n  1,\n  2,\n]\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "ports" }}, "3");
    try expectTomlSource(&ed, "ports = [\n  1,\n  2,\n  3,\n]\n");
}

test "toml remove last item of a multi-line trailing-comma inline array (regression)" {
    // Same class of bug as the append regression above, on the delete side:
    // the backward scan for the preceding comma didn't cross newlines, so
    // removing the last item left its own trailing comma dangling as an
    // empty element that failed to reparse.
    var ed = try newTomlEditor("ports = [\n  1,\n  2,\n]\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "ports" }}, std.math.maxInt(usize));
    try expectTomlSource(&ed, "ports = [\n  1,\n]\n");
}

test "toml inline array ops on array-of-tables are refused" {
    var ed = try newTomlEditor("[[fruit]]\nname = \"apple\"\n");
    defer ed.deinit();
    try std.testing.expectError(error.NotAnInlineArray, ed.appendToSeq(&.{.{ .key = "fruit" }}, "1"));
}

test "toml append array-of-tables element" {
    var ed = try newTomlEditor("[[fruit]]\nname = \"apple\"\n");
    defer ed.deinit();
    try ed.appendContainerToSeq(&.{.{ .key = "fruit" }}, "name = \"pear\"\n");
    try expectTomlSource(&ed, "[[fruit]]\nname = \"apple\"\n\n[[fruit]]\nname = \"pear\"\n");
}

test "toml append AoT element after one with a sub-table" {
    // The new element must splice past the last element's nested sub-table, not
    // into the middle of it.
    var ed = try newTomlEditor("[[fruit]]\nname = \"apple\"\n\n[fruit.variety]\nkind = \"red\"\n");
    defer ed.deinit();
    try ed.appendContainerToSeq(&.{.{ .key = "fruit" }}, "name = \"pear\"\n");
    try expectTomlSource(&ed, "[[fruit]]\nname = \"apple\"\n\n[fruit.variety]\nkind = \"red\"\n\n[[fruit]]\nname = \"pear\"\n");
}

test "toml append empty AoT element" {
    var ed = try newTomlEditor("[[fruit]]\nname = \"apple\"\n");
    defer ed.deinit();
    try ed.appendContainerToSeq(&.{.{ .key = "fruit" }}, "");
    try expectTomlSource(&ed, "[[fruit]]\nname = \"apple\"\n\n[[fruit]]\n");
}

test "toml append AoT with a dotted header path" {
    var ed = try newTomlEditor("[[a.b]]\nx = 1\n");
    defer ed.deinit();
    try ed.appendContainerToSeq(&.{ .{ .key = "a" }, .{ .key = "b" } }, "x = 2\n");
    try expectTomlSource(&ed, "[[a.b]]\nx = 1\n\n[[a.b]]\nx = 2\n");
}

test "toml appendTableToArray on a non-AoT is refused" {
    var ed = try newTomlEditor("nums = [1, 2]\n");
    defer ed.deinit();
    try std.testing.expectError(error.NotAnArrayOfTables, ed.appendContainerToSeq(&.{.{ .key = "nums" }}, "x = 1\n"));
}

// --- deleteTable ---

test "toml delete simple header table" {
    var ed = try newTomlEditor("[a]\nx = 1\n[b]\ny = 2\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try expectTomlSource(&ed, "[b]\ny = 2\n");
}

test "toml delete table leaves interleaved foreign table intact" {
    var ed = try newTomlEditor("[a]\nx = 1\n[other]\ny = 2\n[a.b]\nz = 3\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try expectTomlSource(&ed, "[other]\ny = 2\n");
}

test "toml delete header-only table with sub-tables" {
    var ed = try newTomlEditor("[a]\n[a.b]\ny = 2\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try expectTomlSource(&ed, "");
}

test "toml delete table carries owned comment" {
    var ed = try newTomlEditor("# about a\n[a]\nx = 1\n[b]\ny = 2\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try expectTomlSource(&ed, "[b]\ny = 2\n");
}

test "toml delete table with multi-line array value" {
    var ed = try newTomlEditor("[a]\nl = [\n  1,\n  2,\n]\n[b]\ny = 2\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try expectTomlSource(&ed, "[b]\ny = 2\n");
}

test "toml delete dotted-only table" {
    var ed = try newTomlEditor("a.b = 1\na.c = 2\nz = 9\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try expectTomlSource(&ed, "z = 9\n");
}

test "toml delete whole array-of-tables" {
    var ed = try newTomlEditor("[[f]]\nn = \"a\"\n[[f]]\nn = \"b\"\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "f" }});
    try expectTomlSource(&ed, "");
}

test "toml delete single AoT element" {
    var ed = try newTomlEditor("[[f]]\nn = \"a\"\n[[f]]\nn = \"b\"\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{ .{ .key = "f" }, .{ .index = 0 } });
    try expectTomlSource(&ed, "[[f]]\nn = \"b\"\n");
}

test "toml delete AoT element with nested sub-table" {
    var ed = try newTomlEditor("[[f]]\nn = \"a\"\n[f.sub]\nk = 1\n[[f]]\nn = \"b\"\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{ .{ .key = "f" }, .{ .index = 0 } });
    try expectTomlSource(&ed, "[[f]]\nn = \"b\"\n");
}

test "toml deleteTable on a scalar key is refused" {
    var ed = try newTomlEditor("x = 1\n");
    defer ed.deinit();
    try std.testing.expectError(error.NotATable, ed.deleteContainer(&.{.{ .key = "x" }}));
}

// --- insertTable ---

test "toml insert new table at root end" {
    var ed = try newTomlEditor("a = 1\n[t]\nx = 1\n");
    defer ed.deinit();
    try ed.insertContainer(&.{.{ .key = "s" }}, "p = 1\n");
    try expectTomlSource(&ed, "a = 1\n[t]\nx = 1\n\n[s]\np = 1\n");
}

test "toml insert sub-table after parent subtree" {
    var ed = try newTomlEditor("[a]\nx = 1\n");
    defer ed.deinit();
    try ed.insertContainer(&.{ .{ .key = "a" }, .{ .key = "b" } }, "z = 3\n");
    try expectTomlSource(&ed, "[a]\nx = 1\n\n[a.b]\nz = 3\n");
}

test "toml insert empty table" {
    var ed = try newTomlEditor("a = 1\n");
    defer ed.deinit();
    try ed.insertContainer(&.{.{ .key = "t" }}, "");
    try expectTomlSource(&ed, "a = 1\n\n[t]\n");
}

test "toml insert table with quoted-key segment" {
    var ed = try newTomlEditor("a = 1\n");
    defer ed.deinit();
    try ed.insertContainer(&.{.{ .key = "needs space" }}, "x = 1\n");
    try expectTomlSource(&ed, "a = 1\n\n[\"needs space\"]\nx = 1\n");
}

test "toml insert duplicate table is refused" {
    var ed = try newTomlEditor("[a]\nx = 1\n");
    defer ed.deinit();
    try std.testing.expectError(error.TableExists, ed.insertContainer(&.{.{ .key = "a" }}, "y = 2\n"));
}

// --- renameTable ---

test "toml rename leaf table header" {
    var ed = try newTomlEditor("[server]\nport = 8080\n");
    defer ed.deinit();
    try ed.renameContainer(&.{.{ .key = "server" }}, "http");
    try expectTomlSource(&ed, "[http]\nport = 8080\n");
}

test "toml rename rewrites descendant sub-headers" {
    var ed = try newTomlEditor("[a]\nx = 1\n[a.b]\nz = 3\n[a.b.c]\nw = 4\n");
    defer ed.deinit();
    try ed.renameContainer(&.{.{ .key = "a" }}, "q");
    try expectTomlSource(&ed, "[q]\nx = 1\n[q.b]\nz = 3\n[q.b.c]\nw = 4\n");
}

test "toml rename does not touch a similar-prefix foreign table" {
    var ed = try newTomlEditor("[a]\nx = 1\n[ab]\ny = 2\n");
    defer ed.deinit();
    try ed.renameContainer(&.{.{ .key = "a" }}, "q");
    try expectTomlSource(&ed, "[q]\nx = 1\n[ab]\ny = 2\n");
}

test "toml rename leaf needing quotes" {
    var ed = try newTomlEditor("[a]\nx = 1\n");
    defer ed.deinit();
    try ed.renameContainer(&.{.{ .key = "a" }}, "new key");
    try expectTomlSource(&ed, "[\"new key\"]\nx = 1\n");
}

test "toml rename AoT header" {
    var ed = try newTomlEditor("[[a.b]]\nn = 1\n[[a.b]]\nn = 2\n");
    defer ed.deinit();
    try ed.renameContainer(&.{ .{ .key = "a" }, .{ .key = "b" } }, "c");
    try expectTomlSource(&ed, "[[a.c]]\nn = 1\n[[a.c]]\nn = 2\n");
}

// --- renaming a DOTTED table: every line that spells the prefix ---
//
// A dotted table is named on each of its lines, and only the first of those has
// a key node — so a rename that follows node spans alone renamed nothing at all
// here (the gather finds no `[header]` line to rewrite) and reported success.

test "toml rename a dotted table rewrites every line that names it" {
    var ed = try newTomlEditor("a.b = 1\na.c = 2\n");
    defer ed.deinit();
    try ed.renameContainer(&.{.{ .key = "a" }}, "q");
    try expectTomlSource(&ed, "q.b = 1\nq.c = 2\n");
}

test "toml rename an intermediate dotted segment" {
    var ed = try newTomlEditor("a.b.c = 1\na.b.d = 2\n");
    defer ed.deinit();
    // `a.b.d = 2` has no node of its own for `b` — it is reached as a child of
    // the `b` created by line 1, which is why the walk recurses per dotted
    // LEVEL rather than per node with a span.
    try ed.renameContainer(&.{ .{ .key = "a" }, .{ .key = "b" } }, "q");
    try expectTomlSource(&ed, "a.q.c = 1\na.q.d = 2\n");
}

test "toml rename a dotted table inside a header uses its LINE index" {
    var ed = try newTomlEditor("[t]\na.b = 1\na.c = 2\n");
    defer ed.deinit();
    // `t.a` is at path depth 1 but segment 0 of each line: a dotted key is
    // spelled relative to the enclosing header, so the index comes from the
    // source, not the path.
    try ed.renameContainer(&.{ .{ .key = "t" }, .{ .key = "a" } }, "q");
    try expectTomlSource(&ed, "[t]\nq.b = 1\nq.c = 2\n");
}

test "toml rename a header does NOT touch its children's dotted keys" {
    var ed = try newTomlEditor("[t]\na.b = 1\na.c = 2\n");
    defer ed.deinit();
    // The mirror of the case above: `[t]`'s name appears in the header alone —
    // its children's dotted lines are relative to it and must stay as they are.
    try ed.renameContainer(&.{.{ .key = "t" }}, "q");
    try expectTomlSource(&ed, "[q]\na.b = 1\na.c = 2\n");
}

test "toml rename a table named by BOTH a dotted line and a sub-header" {
    var ed = try newTomlEditor("a.b = 1\n[a.c]\nd = 2\n");
    defer ed.deinit();
    // Renaming `a` has to rewrite both mentions; either one alone split the
    // document into a renamed table plus a re-created `a`.
    try ed.renameContainer(&.{.{ .key = "a" }}, "q");
    try expectTomlSource(&ed, "q.b = 1\n[q.c]\nd = 2\n");
}

test "toml rename a quoted dotted segment replaces the whole token" {
    var ed = try newTomlEditor("[t]\n\"q k\".b = 1\n");
    defer ed.deinit();
    try ed.renameContainer(&.{ .{ .key = "t" }, .{ .key = "q k" } }, "plain");
    try expectTomlSource(&ed, "[t]\nplain.b = 1\n");
}

test "toml rename refuses a target whose name is nowhere to rewrite" {
    var ed = try newTomlEditor("t = { a = 1 }\nk = 1\n");
    defer ed.deinit();
    // An inline table's key is a plain key, not a table name — the whole-table
    // rename has nothing to gather, so it says so instead of reporting a rename
    // that changed nothing. (`replaceKeyAtPath` is what renames these; see
    // below.)
    try std.testing.expectError(error.NotATable, ed.renameContainer(&.{.{ .key = "t" }}, "q"));
    try std.testing.expectError(error.NotATable, ed.renameContainer(&.{.{ .key = "k" }}, "q"));
    try expectTomlSource(&ed, "t = { a = 1 }\nk = 1\n");
}

// --- `replaceKeyAtPath` routes a block table to that same rewrite ---

test "toml replaceKey on a [header] table renames every mention" {
    var ed = try newTomlEditor("[a]\nx = 1\n[a.b]\ny = 2\n");
    defer ed.deinit();
    // Was: `[a]` → `[Q]` with `[a.b]` left behind, which re-created `a` around
    // `b` and split the table — reported as a successful rename.
    try ed.replaceKeyAtPath(&.{.{ .key = "a" }}, "q");
    try expectTomlSource(&ed, "[q]\nx = 1\n[q.b]\ny = 2\n");
}

test "toml replaceKey on an array of tables renames every element header" {
    var ed = try newTomlEditor("[[aot]]\nk = 1\n[[aot]]\nk = 2\n");
    defer ed.deinit();
    try ed.replaceKeyAtPath(&.{.{ .key = "aot" }}, "q");
    try expectTomlSource(&ed, "[[q]]\nk = 1\n[[q]]\nk = 2\n");
}

test "toml replaceKey on a dotted table renames every line" {
    var ed = try newTomlEditor("a.b = 1\na.c = 2\n");
    defer ed.deinit();
    try ed.replaceKeyAtPath(&.{.{ .key = "a" }}, "q");
    try expectTomlSource(&ed, "q.b = 1\nq.c = 2\n");
}

test "toml replaceKey on a scalar or inline container still splices one span" {
    var ed = try newTomlEditor("t = { a = 1 }\nk = 1\nl = [1, 2]\n");
    defer ed.deinit();
    // These keys are written exactly once, so the key span IS the whole rename —
    // the routing above must not reach for the table machinery here. `replaceKey`
    // takes key SYNTAX, which is what a quoted rename spells.
    try ed.replaceKeyAtPath(&.{.{ .key = "t" }}, "tbl");
    try ed.replaceKeyAtPath(&.{.{ .key = "k" }}, "\"a key\"");
    try ed.replaceKeyAtPath(&.{.{ .key = "l" }}, "list");
    try expectTomlSource(&ed, "tbl = { a = 1 }\n\"a key\" = 1\nlist = [1, 2]\n");
}

test "toml replaceKey rolls back a rename that collides with a sibling" {
    var ed = try newTomlEditor("[a]\nx = 1\n[b]\ny = 2\n");
    defer ed.deinit();
    // `[a]` → `[b]` makes two `[b]` tables; the reparse rejects it and the whole
    // multi-line rewrite is undone.
    try std.testing.expectError(error.DuplicateKey, ed.replaceKeyAtPath(&.{.{ .key = "a" }}, "b"));
    try expectTomlSource(&ed, "[a]\nx = 1\n[b]\ny = 2\n");
}

// --- moveTable / reorderTables ---

test "toml move table to end" {
    var ed = try newTomlEditor("[a]\nx = 1\n[b]\ny = 2\n");
    defer ed.deinit();
    try ed.moveContainer(&.{.{ .key = "a" }}, null);
    try expectTomlSource(&ed, "[b]\ny = 2\n\n[a]\nx = 1\n");
}

test "toml move scattered table collapses fragments contiguously" {
    var ed = try newTomlEditor("[a]\nx = 1\n[b]\ny = 2\n[a.c]\nz = 3\n");
    defer ed.deinit();
    try ed.moveContainer(&.{.{ .key = "a" }}, null);
    try expectTomlSource(&ed, "[b]\ny = 2\n\n[a]\nx = 1\n[a.c]\nz = 3\n");
}

test "toml move table before another" {
    var ed = try newTomlEditor("[a]\nx = 1\n[b]\ny = 2\n[c]\nw = 3\n");
    defer ed.deinit();
    try ed.moveContainer(&.{.{ .key = "c" }}, &.{.{ .key = "b" }});
    try expectTomlSource(&ed, "[a]\nx = 1\n\n[c]\nw = 3\n[b]\ny = 2\n");
}

test "toml reorder top-level tables" {
    var ed = try newTomlEditor("[a]\nx = 1\n[b]\ny = 2\n[c]\nw = 3\n");
    defer ed.deinit();
    try ed.reorderContainers(&.{ "c", "a", "b" });
    try expectTomlSource(&ed, "[c]\nw = 3\n[a]\nx = 1\n[b]\ny = 2\n");
}

// --- the move/reorder guards (a table's block is its header LINE) ---
//
// `reorderContainers` and `moveContainer` above are what these two refusals
// point at: the generic key ops relocate an entry's tiled block, which for a
// `[header]` table is the header alone (or the header plus a body that stops
// at the next sibling — with the LAST entry's body left out of the range
// entirely). Both used to report success while rehoming keys.

test "toml moveKey refuses to move a [header] table" {
    var ed = try newTomlEditor("z = 0\n[b]\ny = 2\n[a]\nx = 1\n");
    defer ed.deinit();
    // Used to produce `[b]\nz = 0\ny = 2\n…` — only the header line moved, so
    // the root key `z` landed inside `b`.
    try std.testing.expectError(
        error.CannotMoveTable,
        ed.moveKey(&.{.{ .key = "b" }}, &.{.{ .key = "z" }}),
    );
    try expectTomlSource(&ed, "z = 0\n[b]\ny = 2\n[a]\nx = 1\n");
}

test "toml moveKey refuses to move an entry to before a [header]" {
    var ed = try newTomlEditor("z = 0\n[a]\nx = 1\n[b]\ny = 2\n");
    defer ed.deinit();
    // "Before `[b]`" is the end of `[a]`'s body, so `z` would have become `a.z`
    // — the destination is as much of a hazard as the source.
    try std.testing.expectError(
        error.CannotMoveTable,
        ed.moveKey(&.{.{ .key = "z" }}, &.{.{ .key = "b" }}),
    );
    try expectTomlSource(&ed, "z = 0\n[a]\nx = 1\n[b]\ny = 2\n");
}

test "toml moveKey still moves plain entries inside a table" {
    var ed = try newTomlEditor("[a]\nx = 1\ny = 2\nz = 3\n");
    defer ed.deinit();
    try ed.moveKey(&.{ .{ .key = "a" }, .{ .key = "z" } }, &.{ .{ .key = "a" }, .{ .key = "y" } });
    try expectTomlSource(&ed, "[a]\nx = 1\nz = 3\ny = 2\n");
}

test "toml reorderKeys refuses a reorder that shifts a table" {
    var ed = try newTomlEditor("z = 0\n[b]\ny = 2\n[a]\nx = 1\n");
    defer ed.deinit();
    // Used to produce `z = 0\n[a]\n[b]\ny = 2\nx = 1\n`: `[a]` was the last
    // entry, so its block stopped at its own header line and `x = 1` stayed
    // put — becoming `b.x`, with `[a]` left empty.
    try std.testing.expectError(
        error.CannotReorderTables,
        ed.reorderKeys(&.{}, &.{ "z", "a", "b" }),
    );
    try expectTomlSource(&ed, "z = 0\n[b]\ny = 2\n[a]\nx = 1\n");
}

test "toml reorderKeys still reorders scalars around a sub-table that stays put" {
    var ed = try newTomlEditor("[a]\nx = 1\ny = 2\n[a.b]\nz = 3\n");
    defer ed.deinit();
    // The guard sees only entries whose position CHANGES, and `b` keeps its
    // index here — so this legitimate reorder is untouched.
    try ed.reorderKeys(&.{.{ .key = "a" }}, &.{ "y", "x" });
    try expectTomlSource(&ed, "[a]\ny = 2\nx = 1\n[a.b]\nz = 3\n");
}

// --- dotted tables are section nodes too ---
//
// A dotted table (`a.b = 1`) has no `[` on its line, so the old line-sniffing
// guards let the line ops through — correct for a one-line table, and a silent
// partial edit for one spread over several lines (`a.b = 1` … `a.c = 2`),
// since a line op sees only the line the node's span is on. The parser records
// every line that creates or extends a dotted table (`Document.node_regions`),
// so the engine's rule now refuses the line ops for it and the container ops
// take every line.

test "toml deleteKey refuses a dotted table; deleteContainer takes every line" {
    var ed = try newTomlEditor("a.b = 1\nz = 0\na.c = 2\n");
    defer ed.deinit();
    // Used to delete `a.b = 1` alone, leaving `a.c = 2` to keep `a` alive.
    try std.testing.expectError(error.CannotDeleteTable, ed.deleteKey(&.{.{ .key = "a" }}));
    try expectTomlSource(&ed, "a.b = 1\nz = 0\na.c = 2\n");
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try expectTomlSource(&ed, "z = 0\n");
}

test "toml deleteContainer of a header table takes a multi-line dotted child whole" {
    // `x` is a dotted table under `[a]` spread over two lines; the second line
    // is in no node span of `x`'s keyvalue, and a gather that took the entry's
    // own line would have left `x.z = 2` behind to become a root key.
    var ed = try newTomlEditor("[a]\nx.y = 1\nx.z = 2\n[b]\nw = 3\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try expectTomlSource(&ed, "[b]\nw = 3\n");
}

test "toml moveContainer accepts a dotted table as the destination" {
    var ed = try newTomlEditor("[a]\nx = 1\n[b]\ny = 2\n");
    defer ed.deinit();
    // A dotted-only table has no `[` line, which the old header-line sniff
    // refused as a destination; it is a section node like any other.
    try ed.replaceAtSpan(Span.init(0, 0), "d.k = 0\n");
    try ed.moveContainer(&.{.{ .key = "b" }}, &.{.{ .key = "d" }});
    try expectTomlSource(&ed, "[b]\ny = 2\nd.k = 0\n[a]\nx = 1\n");
}

test "toml reorderKeys still reorders a document of plain root keys" {
    var ed = try newTomlEditor("a = 1\nb = 2\nc = 3\n");
    defer ed.deinit();
    try ed.reorderKeys(&.{}, &.{ "c", "a" });
    try expectTomlSource(&ed, "c = 3\na = 1\nb = 2\n");
}
