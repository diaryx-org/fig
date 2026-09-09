//! fig-specific editing helpers for `Editor(Fig)`.
//!
//! The generic span-splice engine lives in `../editor.zig`; this module holds
//! the fig-only logic it delegates to. Unlike TOML (whose `[header]` syntax
//! forces a multi-region gather for almost every structural op) or YAML
//! (indentation-column rendering), fig's block layer is line-oriented and
//! self-describing — every line's `>` count states its own depth, so a new
//! sibling line can be spliced in *anywhere* after an existing child's full
//! text and still parse correctly. That collapses most of what would
//! otherwise be format-specific plumbing into one trick, used throughout this
//! file: **copy an existing sibling's marker-prefix text verbatim** (the run
//! of `>` markers + separator space immediately before its key/value) rather
//! than recomputing depth/indentation from scratch. See DESIGN.md's "Depth is
//! a correctness risk" and "prefix-count depth" for why this is safe.
//!
//! ## Whole-container structural ops (`deleteContainer`/`moveContainer`/
//! `reorderContainers`)
//!
//! Nothing of these lives here any more: they are the generic ops in
//! `../../editor.zig`, over the derived region set `../../editor/regions.zig`
//! builds from `Document.node_regions`. What fig contributes is the parser's
//! half — `fig/parser.zig` records every BLOCK (non-flow) container's header
//! lines: the line that created it, plus every LATER header that re-entered
//! it verbatim (`database` written a second time, `> pool` reopened later in
//! the same parent's body, a dotted path whose final segment re-selects an
//! existing container, `xs[i]` re-opening an element — DESIGN.md
//! "Re-entering a path to add new keys is fine", a shape `fig fmt`'s own
//! grouped hoisting EMITS). A container's node span anchors only the line
//! that CREATED it, so those later lines are in no child's span and are
//! exactly what the table exists to carry. From that, the engine's gather
//! recurses into every block-container child (each is a section node) and
//! takes every other child's own line, which handles fig's TOML-equivalent
//! scattering (`a`/`other`/`a.b`) and verbatim re-entry alike. The tests below
//! pin every one of those behaviours for fig; the logic is the engine's.
//!
//! The same table is why `deleteKey`, `moveKey` and `reorderKeys` refuse a
//! block-container-valued entry (`CannotDeleteContainer`, `CannotMoveContainer`,
//! `CannotReorderContainers` — the engine's one section rule, spelled per
//! `Syntax.section_noun`): such a container may be scattered, and a line
//! splice would move or remove only the fragment it can see.
//!
//! ## Scope (documented, not silent)
//!
//! `replaceValAtPath` overwriting a re-entered/scattered container's entire
//! value in ONE splice still carries a narrow gap (it replaces the node's
//! widened span, which is not region-aware); the reparse-rollback net keeps
//! it safe. And deleting a container whose removal leaves an ANCESTOR header
//! childless (e.g. `a.b` when `b` was `a`'s only child and `a` was written as
//! a header) still rolls back via `FigEmptyContainer` — the cascade
//! ("also delete the now-empty ancestor header") is deliberately not implied
//! by a delete of the child path.

const std = @import("std");

const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const Span = @import("../../util/span.zig");
const editor = @import("../../editor.zig");
const splice = @import("../../editor/splice.zig");
const Fig = @import("fig.zig").Language;
const Printer = @import("printer.zig");
const Writer = std.Io.Writer;
const log = std.log.scoped(.editor);

const FigEditor = editor.Editor(Fig);

const lineStartBefore = splice.lineStartBefore;
const lineEndAfter = splice.lineEndAfter;
const firstNonSpace = splice.firstNonSpace;
const isFlow = splice.isFlow;

// ============================================================================
// renderTail — what follows a key (the fig `renderTail` renderer)
// ============================================================================

/// If `value_text` is a fig BLOCK-container fragment — a section body (`a = 1`
/// lines) or `*`-element list that has no inline `key = <value>` spelling —
/// return it re-printed as a block body at marker depth `depth` (caller frees).
/// Inline values return null: flow containers (`{ … }` / `[ … ]`), single-line
/// scalars, and multi-line `'''`/`"""` block strings all splice directly after
/// `key = `. A fragment that fails to parse, or parses to a non-container,
/// likewise returns null so the caller keeps the plain inline splice and lets
/// the reparse-rollback net (`replaceAtSpan`) report any real error.
///
/// Fig markers are an absolute depth ruler measured from column 0 (`> ` per
/// level), so re-printing the fragment's root at `depth` — one level below the
/// key it hangs under — yields body lines that carry their own full marker run
/// and need no further indentation. This is what lets a caller splice a block
/// map/sequence into a document (e.g. a fenced embed) instead of freezing every
/// short map inline as flow.
fn blockBody(allocator: std.mem.Allocator, depth: usize, value_text: []const u8) ?[]u8 {
    const t = std.mem.trim(u8, value_text, " \t\r\n");
    // Inline forms keep the direct splice: flow braces/brackets, quoted or
    // block-string scalars, and any single-line value.
    if (t.len == 0 or t[0] == '{' or t[0] == '[' or t[0] == '\'' or t[0] == '"') return null;
    if (std.mem.indexOfScalar(u8, t, '\n') == null) return null;

    var parser: Fig.Parser = .{ .allocator = allocator };
    var frag = Fig.parse(&parser, value_text, Fig.default_type) catch return null;
    defer frag.deinit(allocator);
    switch (frag.ast.nodes[frag.ast.root].kind) {
        .mapping, .sequence => {},
        else => return null, // a multi-line scalar is still an inline value
    }

    var w: Writer.Allocating = .init(allocator);
    defer w.deinit();
    Printer.printNode(&w.writer, &frag.ast, frag.ast.root, depth, .{}) catch return null;
    return allocator.dupe(u8, w.written()) catch return null;
}

/// What follows a key on its entry line, after `<indent><key>`: ` = <value>`
/// for an inline value, or a newline plus the value re-framed as a block
/// section (see `blockBody`) one level below the key's marker depth — which
/// is the count of `>` cells in `indent`, the structural prefix the engine
/// copied from the key's line. An empty `key_text` is the document root,
/// whose value is a whole document already and is taken as written. No
/// trailing newline. See `editor.Editor.writeTail`.
pub fn renderTail(allocator: std.mem.Allocator, out: *std.ArrayList(u8), indent: []const u8, key_text: []const u8, value_text: []const u8) !void {
    if (key_text.len == 0) return out.appendSlice(allocator, value_text);
    const depth = std.mem.count(u8, indent, ">");
    if (blockBody(allocator, depth + 1, value_text)) |body| {
        defer allocator.free(body);
        try out.append(allocator, '\n');
        // printNode ends every line (the last included) with '\n'; the caller
        // supplies the entry's own line break, so drop the printed trailing one.
        try out.appendSlice(allocator, std.mem.trimEnd(u8, body, "\n"));
    } else {
        try out.appendSlice(allocator, " = ");
        try out.appendSlice(allocator, value_text);
    }
}

// =======
// TESTS
// =======
//
// fig editor tests live here (rather than in editor.zig) so each language's
// editing tests sit next to that language's helpers, mirroring
// `toml/editor_helper.zig`.

fn newFigEditor(input: []const u8) !editor.Editor(Fig) {
    var ed: editor.Editor(Fig) = .{ .allocator = std.testing.allocator };
    try ed.init(input);
    return ed;
}

fn expectFigSource(ed: *const editor.Editor(Fig), expected: []const u8) !void {
    errdefer log.err("actual:   \"{s}\"", .{ed.source.items});
    errdefer log.err("expected: \"{s}\"", .{expected});
    try std.testing.expectEqualStrings(expected, ed.source.items);
}

// --- point edits (value/key replace — generic engine, spans only) ---

test "fig replace root scalar value" {
    var ed = try newFigEditor("title = old\nport = 8080\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "port" }}, "9090");
    try expectFigSource(&ed, "title = old\nport = 9090\n");
}

test "fig replace value nested under marker depth" {
    var ed = try newFigEditor("database\n> host = localhost\n> pool\n> > size = 10\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{ .{ .key = "database" }, .{ .key = "pool" }, .{ .key = "size" } }, "20");
    try expectFigSource(&ed, "database\n> host = localhost\n> pool\n> > size = 20\n");
}

test "fig set: a block-map value re-frames as a nested section (not an inline splice)" {
    // The prov case: splice a whole block map as a new key's value. A block
    // container has no valid inline `key = <block>` spelling, so it descends
    // under a `registry` header with its entries one marker level deeper.
    var ed = try newFigEditor("title = hi\n");
    defer ed.deinit();
    try ed.set(&.{.{ .key = "registry" }}, "a = 1\nb = 2\n");
    try expectFigSource(&ed, "title = hi\nregistry\n> a = 1\n> b = 2\n");
}

test "fig replace: an inline scalar value re-frames into a block map" {
    var ed = try newFigEditor("registry = old\ntitle = hi\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "registry" }}, "a = 1\nb = 2\n");
    try expectFigSource(&ed, "registry\n> a = 1\n> b = 2\ntitle = hi\n");
}

test "fig set: a block value under an existing depth-1 key nests one level deeper" {
    var ed = try newFigEditor("server\n> port = 8080\n");
    defer ed.deinit();
    try ed.set(&.{ .{ .key = "server" }, .{ .key = "opts" } }, "x = 1\ny = 2\n");
    try expectFigSource(&ed, "server\n> port = 8080\n> opts\n> > x = 1\n> > y = 2\n");
}

test "fig set: a block-sequence value re-frames as `> *` element lines" {
    var ed = try newFigEditor("title = hi\n");
    defer ed.deinit();
    try ed.set(&.{.{ .key = "items" }}, "* a\n* b\n");
    try expectFigSource(&ed, "title = hi\nitems\n> * a\n> * b\n");
}

test "fig set: a flow-map value still splices inline (unchanged)" {
    // A `{ … }` fragment has a valid inline spelling, so it is NOT re-framed.
    var ed = try newFigEditor("title = hi\n");
    defer ed.deinit();
    try ed.set(&.{.{ .key = "m" }}, "{ a = 1, b = 2 }");
    try expectFigSource(&ed, "title = hi\nm = { a = 1, b = 2 }\n");
}

test "fig rename a leaf key" {
    var ed = try newFigEditor("server\n> port = 8080\n");
    defer ed.deinit();
    try ed.replaceKeyAtPath(&.{ .{ .key = "server" }, .{ .key = "port" } }, "listen_port");
    try expectFigSource(&ed, "server\n> listen_port = 8080\n");
}

test "fig failed edit rolls back and keeps editor usable" {
    var ed = try newFigEditor("a = 1\nb = 2\n");
    defer ed.deinit();
    if (ed.replaceValAtPath(&.{.{ .key = "a" }}, "[oops")) |_| {
        return error.TestExpectedFailedEdit;
    } else |_| {}
    try expectFigSource(&ed, "a = 1\nb = 2\n");
    try ed.replaceValAtPath(&.{.{ .key = "a" }}, "9");
    try expectFigSource(&ed, "a = 9\nb = 2\n");
}

// --- comments (generic engine, once spans + fig's `#` marker are right) ---

test "fig add leading comment matches marker depth" {
    var ed = try newFigEditor("database\n> host = localhost\n> pool\n> > size = 10\n");
    defer ed.deinit();
    try ed.addLeadingComment(&.{ .{ .key = "database" }, .{ .key = "pool" }, .{ .key = "size" } }, "note");
    try expectFigSource(&ed, "database\n> host = localhost\n> pool\n> > # note\n> > size = 10\n");
}

test "fig set trailing comment on a nested header line" {
    var ed = try newFigEditor("database\n> pool\n> > size = 10\n");
    defer ed.deinit();
    try ed.setTrailingComment(&.{ .{ .key = "database" }, .{ .key = "pool" } }, "nested container");
    try expectFigSource(&ed, "database\n> pool # nested container\n> > size = 10\n");
}

// --- insertKey (block) ---

test "fig insert key into root" {
    var ed = try newFigEditor("a = 1\nb = 2\n");
    defer ed.deinit();
    try ed.insertKey(&.{}, "c", "3");
    try expectFigSource(&ed, "a = 1\nb = 2\nc = 3\n");
}

test "fig insert key into a nested marker-block mapping" {
    var ed = try newFigEditor("database\n> host = localhost\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "database" }}, "port", "5432");
    try expectFigSource(&ed, "database\n> host = localhost\n> port = 5432\n");
}

test "fig insert key preserves spaced marker style at depth 2" {
    var ed = try newFigEditor("database\n> pool\n> > size = 10\n");
    defer ed.deinit();
    try ed.insertKey(&.{ .{ .key = "database" }, .{ .key = "pool" } }, "timeout", "30");
    try expectFigSource(&ed, "database\n> pool\n> > size = 10\n> > timeout = 30\n");
}

test "fig insert key after a container whose own line ends without a value" {
    // The new key must land after `pool`'s WHOLE nested body, not right after
    // the `pool` header line itself.
    var ed = try newFigEditor("database\n> pool\n> > size = 10\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "database" }}, "name", "primary");
    try expectFigSource(&ed, "database\n> pool\n> > size = 10\n> name = primary\n");
}

test "fig insert key into a fig-inline flow mapping" {
    var ed = try newFigEditor("p = { x = 1 }\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "p" }}, "y", "2");
    try expectFigSource(&ed, "p = { x = 1, y = 2 }\n");
}

test "fig insert key into an empty flow mapping defaults to fig-inline" {
    var ed = try newFigEditor("p = {}\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "p" }}, "x", "1");
    try expectFigSource(&ed, "p = { x = 1 }\n");
}

test "fig insert key into a JSON-mode flow mapping matches its colon separator" {
    var ed = try newFigEditor("p = { \"x\": 1 }\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "p" }}, "\"y\"", "2");
    try expectFigSource(&ed, "p = { \"x\": 1, \"y\": 2 }\n");
}

test "fig insert duplicate key rolls back" {
    var ed = try newFigEditor("a = 1\n");
    defer ed.deinit();
    try std.testing.expectError(error.FigDuplicateKey, ed.insertKey(&.{}, "a", "2"));
    try expectFigSource(&ed, "a = 1\n");
}

// --- deleteKey ---

test "fig delete scalar key" {
    var ed = try newFigEditor("a = 1\nb = 2\nc = 3\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectFigSource(&ed, "a = 1\nc = 3\n");
}

test "fig delete key with owned comment" {
    var ed = try newFigEditor("a = 1\n# note\nb = 2\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectFigSource(&ed, "a = 1\n");
}

test "fig delete a nested scalar key" {
    var ed = try newFigEditor("database\n> host = localhost\n> port = 5432\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "database" }, .{ .key = "port" } });
    try expectFigSource(&ed, "database\n> host = localhost\n");
}

test "fig delete a flow-container-valued key" {
    var ed = try newFigEditor("a = 1\np = { x = 1 }\nb = 2\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "p" }});
    try expectFigSource(&ed, "a = 1\nb = 2\n");
}

// Regression: deleting a key *inside* a flow mapping — the packed case swallowed
// a sibling, the single-entry case wiped the whole `{ … }` line. The flow-aware
// splice removes only the targeted entry and its adjoining comma, leaving the
// braces (and any survivor) intact.
test "fig delete key inside a packed flow mapping (regression)" {
    var ed = try newFigEditor("p = { x = 1, y = 2 }\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "p" }, .{ .key = "y" } });
    try expectFigSource(&ed, "p = { x = 1 }\n");
}

test "fig delete only key of a single-entry flow mapping (regression)" {
    var ed = try newFigEditor("p = { x = 1 }\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "p" }, .{ .key = "x" } });
    try expectFigSource(&ed, "p = { }\n");
}

test "fig deleting a block-container-valued key is refused" {
    var ed = try newFigEditor("database\n> host = localhost\n> pool\n> > size = 10\n");
    defer ed.deinit();
    try std.testing.expectError(error.CannotDeleteContainer, ed.deleteKey(&.{ .{ .key = "database" }, .{ .key = "pool" } }));
    try expectFigSource(&ed, "database\n> host = localhost\n> pool\n> > size = 10\n");
}

// --- block sequence append/prepend/remove ---

test "fig append/prepend/remove a scalar sequence" {
    var ed = try newFigEditor("ports\n> * 1\n> * 2\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "ports" }}, "3");
    try expectFigSource(&ed, "ports\n> * 1\n> * 2\n> * 3\n");
    try ed.prependToSeq(&.{.{ .key = "ports" }}, "0");
    try expectFigSource(&ed, "ports\n> * 0\n> * 1\n> * 2\n> * 3\n");
    try ed.removeSeqItem(&.{.{ .key = "ports" }}, 2);
    try expectFigSource(&ed, "ports\n> * 0\n> * 1\n> * 3\n");
}

test "fig remove a map-shaped sequence element carries its whole body" {
    var ed = try newFigEditor("servers\n> *\n>> host = a.com\n> *\n>> host = b.com\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "servers" }}, 0);
    try expectFigSource(&ed, "servers\n> *\n>> host = b.com\n");
}

test "fig inline array append/prepend/remove (flow)" {
    var ed = try newFigEditor("ports = [1, 2]\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "ports" }}, "3");
    try expectFigSource(&ed, "ports = [1, 2, 3]\n");
    try ed.prependToSeq(&.{.{ .key = "ports" }}, "0");
    try expectFigSource(&ed, "ports = [0, 1, 2, 3]\n");
    try ed.removeSeqItem(&.{.{ .key = "ports" }}, 2);
    try expectFigSource(&ed, "ports = [0, 1, 3]\n");
}

test "fig inline array append with pre-existing trailing comma (single line)" {
    // A trailing comma before ']' is legal fig flow-array syntax; appending
    // must not double it into an empty element that fails to reparse.
    var ed = try newFigEditor("ports = [1, 2,]\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "ports" }}, "3");
    try expectFigSource(&ed, "ports = [1, 2, 3,]\n");
}

test "fig inline array append onto a multi-line one-item-per-line array" {
    // Regression: appending used to splice right before the closing ']',
    // which — combined with the pre-existing trailing comma after the last
    // item — produced a doubled comma that failed to reparse. The fix
    // splices after the last item and keeps the one-per-line style.
    var ed = try newFigEditor("contents = [\n  a,\n  b,\n]\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "contents" }}, "c");
    try expectFigSource(&ed, "contents = [\n  a,\n  b,\n  c,\n]\n");
}

test "fig remove last flow item via the [-] end sentinel" {
    // `removeSeqItem` treats `std.math.maxInt(usize)` — the same sentinel
    // `parsePath` produces for `contents[-]`/`contents[$]` — as "the last
    // item", so delete can address the end symmetrically with append.
    var ed = try newFigEditor("ports = [1, 2, 3]\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "ports" }}, std.math.maxInt(usize));
    try expectFigSource(&ed, "ports = [1, 2]\n");
}

test "fig remove last item of a multi-line trailing-comma array (regression)" {
    // Regression: appending to this shape used to leave the array such that
    // removing the new last item (found via a preceding-comma backward scan
    // that only skipped spaces/tabs, not newlines) left the item's own
    // trailing comma dangling with nothing before it — an empty element that
    // failed to reparse. The scan must cross the newline to find the real
    // separator comma.
    var ed = try newFigEditor("contents = [\n  a,\n  b,\n]\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "contents" }}, "c");
    try expectFigSource(&ed, "contents = [\n  a,\n  b,\n  c,\n]\n");
    try ed.removeSeqItem(&.{.{ .key = "contents" }}, std.math.maxInt(usize));
    try expectFigSource(&ed, "contents = [\n  a,\n  b,\n]\n");
}

test "fig remove middle item of a multi-line one-item-per-line array" {
    var ed = try newFigEditor("contents = [\n  a,\n  b,\n  c,\n]\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "contents" }}, 1);
    try expectFigSource(&ed, "contents = [\n  a,\n  c,\n]\n");
}

test "fig remove first item of a multi-line one-item-per-line array" {
    var ed = try newFigEditor("contents = [\n  a,\n  b,\n  c,\n]\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "contents" }}, 0);
    try expectFigSource(&ed, "contents = [\n  b,\n  c,\n]\n");
}

// --- renameContainer: no dedicated op — replaceKeyAtPath already does it ---

test "fig rename a container's key via the generic replaceKeyAtPath" {
    var ed = try newFigEditor("database\n> host = localhost\n> pool\n> > size = 10\n");
    defer ed.deinit();
    try ed.replaceKeyAtPath(&.{ .{ .key = "database" }, .{ .key = "pool" } }, "settings");
    try expectFigSource(&ed, "database\n> host = localhost\n> settings\n> > size = 10\n");
}

// --- deleteContainer ---

test "fig delete a nested block container" {
    var ed = try newFigEditor("database\n> host = localhost\n> pool\n> > size = 10\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{ .{ .key = "database" }, .{ .key = "pool" } });
    try expectFigSource(&ed, "database\n> host = localhost\n");
}

test "fig delete a whole top-level container with all descendants" {
    var ed = try newFigEditor("database\n> host = localhost\n> pool\n> > size = 10\nother = 1\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "database" }});
    try expectFigSource(&ed, "other = 1\n");
}

test "fig delete carries an owned leading comment" {
    var ed = try newFigEditor("# about database\ndatabase\n> host = localhost\nother = 1\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "database" }});
    try expectFigSource(&ed, "other = 1\n");
}

test "fig delete a container split across dotted re-entry, foreign sibling intact" {
    // `a`/`other`/`a.b` — the fig equivalent of TOML's `[a]`/`[other]`/`[a.b]`
    // interleaving: `a.b` is a SEPARATE dotted path (not the identical header
    // `a` written twice), so gather finds it via recursion into `a`'s own
    // child "b", with no separate header-occurrence tracking needed.
    var ed = try newFigEditor("a\n> x = 1\nother = 1\na.b\n> z = 3\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "a" }});
    try expectFigSource(&ed, "other = 1\n");
}

test "fig delete a block sequence" {
    var ed = try newFigEditor("servers\n> *\n>> host = a.com\n> *\n>> host = b.com\nother = 1\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "servers" }});
    try expectFigSource(&ed, "other = 1\n");
}

test "fig delete one index-addressed block-mapped sequence element" {
    var ed = try newFigEditor("servers\n> *\n>> host = a.com\n> *\n>> host = b.com\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{ .{ .key = "servers" }, .{ .index = 0 } });
    try expectFigSource(&ed, "servers\n> *\n>> host = b.com\n");
}

test "fig deleteContainer on a scalar is refused" {
    var ed = try newFigEditor("x = 1\n");
    defer ed.deinit();
    try std.testing.expectError(error.NotAContainer, ed.deleteContainer(&.{.{ .key = "x" }}));
    try expectFigSource(&ed, "x = 1\n");
}

test "fig deleteContainer on a flow-valued key is refused" {
    var ed = try newFigEditor("p = { x = 1 }\n");
    defer ed.deinit();
    try std.testing.expectError(error.NotAContainer, ed.deleteContainer(&.{.{ .key = "p" }}));
    try expectFigSource(&ed, "p = { x = 1 }\n");
}

test "fig delete a verbatim re-entered header removes every occurrence" {
    // The exact same header (`database`, not a deeper dotted path) written
    // twice — the shape spans alone can't discover; found via the parser's
    // `Document.node_regions` record (see the module doc comment). Both
    // header lines and every child go; the foreign sibling stays put.
    var ed = try newFigEditor("database\n> x = 1\nother = 1\ndatabase\n> y = 2\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "database" }});
    try expectFigSource(&ed, "other = 1\n");
}

test "fig delete a re-entered NESTED header removes the reopened line too" {
    // `> pool` reopened later inside the same parent's body — the nested twin
    // of the verbatim root re-entry (same `resolveHeaderFinal` record).
    var ed = try newFigEditor("database\n> pool\n>> a = 1\n> pool\n>> b = 2\n> keep = 1\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{ .{ .key = "database" }, .{ .key = "pool" } });
    try expectFigSource(&ed, "database\n> keep = 1\n");
}

test "fig delete a container re-opened by a dotted header's final segment" {
    // `b` is CREATED by the dotted assignment `> b.x = 1` (so its span
    // anchors that line), then RE-OPENED by the `a.b` section header — a
    // deeper-dotted-path line that is in no child's span. The re-entry record
    // is what removes it.
    var ed = try newFigEditor("a\n> keep = 1\n> b.x = 1\nother = 1\na.b\n> y = 2\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{ .{ .key = "a" }, .{ .key = "b" } });
    try expectFigSource(&ed, "a\n> keep = 1\nother = 1\n");
}

test "fig delete a sequence whose element header is re-opened by index" {
    // `xs[0]` written twice: the first creates element 0, the second re-opens
    // it (`resolveHeaderFinal`'s index twin of the key re-open).
    var ed = try newFigEditor("xs[0]\n> a = 1\nxs[0]\n> b = 2\nother = 1\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "xs" }});
    try expectFigSource(&ed, "other = 1\n");
}

test "fig delete carries a re-entered header's own leading comment" {
    var ed = try newFigEditor("database\n> x = 1\nother = 1\n# more database\ndatabase\n> y = 2\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{.{ .key = "database" }});
    try expectFigSource(&ed, "other = 1\n");
}

test "fig delete leaving an ANCESTOR header childless fails safely" {
    // The documented residual edge (module doc "Scope"): removing `a.b`
    // leaves the `a` header with nothing under it. The cascade delete is
    // deliberately not implied; `FigEmptyContainer` on the reparse rolls the
    // edit back instead of leaving a bare childless container behind.
    var ed = try newFigEditor("a\n> b\n>> x = 1\nother = 1\n");
    defer ed.deinit();
    try std.testing.expectError(error.FigEmptyContainer, ed.deleteContainer(&.{ .{ .key = "a" }, .{ .key = "b" } }));
    try expectFigSource(&ed, "a\n> b\n>> x = 1\nother = 1\n");
}

// --- moveContainer ---

test "fig move a container to end of file" {
    var ed = try newFigEditor("a\n> x = 1\nb\n> y = 2\n");
    defer ed.deinit();
    try ed.moveContainer(&.{.{ .key = "a" }}, null);
    try expectFigSource(&ed, "b\n> y = 2\n\na\n> x = 1\n");
}

test "fig move a container before another" {
    var ed = try newFigEditor("a\n> x = 1\nb\n> y = 2\nc\n> w = 3\n");
    defer ed.deinit();
    try ed.moveContainer(&.{.{ .key = "c" }}, &.{.{ .key = "b" }});
    try expectFigSource(&ed, "a\n> x = 1\n\nc\n> w = 3\nb\n> y = 2\n");
}

test "fig move a dotted-re-entry-scattered container collapses fragments contiguously" {
    var ed = try newFigEditor("a\n> x = 1\nb\n> y = 2\na.c\n> z = 3\n");
    defer ed.deinit();
    try ed.moveContainer(&.{.{ .key = "a" }}, null);
    try expectFigSource(&ed, "b\n> y = 2\n\na\n> x = 1\na.c\n> z = 3\n");
}

test "fig move a verbatim re-entered container relocates both occurrences" {
    // Both physical `database` blocks (the creating header and the verbatim
    // re-entry, found via `Document.node_regions`) move contiguously; the
    // re-entered spelling itself is preserved — still-valid fig that parses
    // to the same merged mapping.
    var ed = try newFigEditor("database\n> x = 1\nother = 1\ndatabase\n> y = 2\n");
    defer ed.deinit();
    try ed.moveContainer(&.{.{ .key = "database" }}, null);
    try expectFigSource(&ed, "other = 1\n\ndatabase\n> x = 1\ndatabase\n> y = 2\n");
}

test "fig move destination inside the source is a no-op" {
    var ed = try newFigEditor("a\n> x = 1\n> pool\n> > size = 10\n");
    defer ed.deinit();
    try ed.moveContainer(&.{.{ .key = "a" }}, &.{ .{ .key = "a" }, .{ .key = "pool" } });
    try expectFigSource(&ed, "a\n> x = 1\n> pool\n> > size = 10\n");
}

test "fig moveContainer on a scalar is refused" {
    var ed = try newFigEditor("x = 1\na\n> y = 2\n");
    defer ed.deinit();
    try std.testing.expectError(error.NotAContainer, ed.moveContainer(&.{.{ .key = "x" }}, null));
    try expectFigSource(&ed, "x = 1\na\n> y = 2\n");
}

// --- realistic input lifted from con.fig's kitchen sink (`fig fmt` house
// style: spaced markers, trailing comments on nearly every line, a `#`
// section-header comment above the next top-level entry) ---

test "fig delete a nested container carries its own trailing comments, leaves the sibling section comment alone" {
    var ed = try newFigEditor(
        \\database # container header (bare word, no `=`)
        \\> host = localhost # database.host
        \\> port = 5432 # database.port  (bare number)
        \\> pool # nested container header
        \\> > size = 10 # database.pool.size
        \\> > timeout = 30 # database.pool.timeout
        \\
        \\# === Dotted-key flattener (flatten within one line) ===
        \\cache
        \\> redis
        \\> > host = 127.0.0.1 # cache.redis.host  (IP -> string, 3 dots)
        \\
    );
    defer ed.deinit();
    try ed.deleteContainer(&.{ .{ .key = "database" }, .{ .key = "pool" } });
    try expectFigSource(
        &ed,
        \\database # container header (bare word, no `=`)
        \\> host = localhost # database.host
        \\> port = 5432 # database.port  (bare number)
        \\
        \\# === Dotted-key flattener (flatten within one line) ===
        \\cache
        \\> redis
        \\> > host = 127.0.0.1 # cache.redis.host  (IP -> string, 3 dots)
        \\
        ,
    );
}

test "fig move a container up front of a differently-commented sibling" {
    var ed = try newFigEditor(
        \\database # container header (bare word, no `=`)
        \\> host = localhost # database.host
        \\
        \\# === Dotted-key flattener (flatten within one line) ===
        \\cache
        \\> redis
        \\> > host = 127.0.0.1 # cache.redis.host
        \\
    );
    defer ed.deinit();
    try ed.moveContainer(&.{.{ .key = "cache" }}, &.{.{ .key = "database" }});
    // No blank line goes IN FRONT of `cache` (it lands at the absolute start
    // of the file — `appendWithBlankBefore` only separates from PRECEDING
    // output, and there is none here); the original blank line that used to
    // separate `database` from `cache`'s leading comment rides along after
    // `database` instead (part of `database`'s own "kept" tail).
    try expectFigSource(
        &ed,
        \\# === Dotted-key flattener (flatten within one line) ===
        \\cache
        \\> redis
        \\> > host = 127.0.0.1 # cache.redis.host
        \\database # container header (bare word, no `=`)
        \\> host = localhost # database.host
        \\
        \\
        ,
    );
}

// --- reorderContainers ---

test "fig reorder top-level containers" {
    var ed = try newFigEditor("a\n> x = 1\nb\n> y = 2\nc\n> w = 3\n");
    defer ed.deinit();
    try ed.reorderContainers(&.{ "c", "a", "b" });
    try expectFigSource(&ed, "c\n> w = 3\na\n> x = 1\nb\n> y = 2\n");
}

test "fig reorder leaves an unnamed container untouched, in its original relative position" {
    var ed = try newFigEditor("a\n> x = 1\nb\n> y = 2\nc\n> w = 3\n");
    defer ed.deinit();
    // Only `b`/`a` are named (swapped); `c` isn't mentioned, so it stays put.
    try ed.reorderContainers(&.{ "b", "a" });
    try expectFigSource(&ed, "b\n> y = 2\na\n> x = 1\nc\n> w = 3\n");
}

test "fig reorderContainers on a scalar is refused" {
    var ed = try newFigEditor("x = 1\na\n> y = 2\n");
    defer ed.deinit();
    try std.testing.expectError(error.NotAContainer, ed.reorderContainers(&.{ "x", "a" }));
    try expectFigSource(&ed, "x = 1\na\n> y = 2\n");
}

// --- the engine's section rule on move/reorder ---
//
// fig only ever guarded `deleteKey`; `moveKey` and `reorderKeys` relocated a
// block container's widened span, which is the whole container when it is
// contiguous and only its FIRST fragment when it has been re-entered — the
// re-entered fragment stayed behind, still parsing, so the edit reported
// success with half the container moved. A block container is a section node
// now, and the one engine rule refuses all three line ops for it.

test "fig moveKey refuses a block container at either end" {
    var ed = try newFigEditor("a\n> x = 1\nother = 1\na\n> y = 2\nb\n> z = 3\n");
    defer ed.deinit();
    // Moving `a` by its span would carry only the first `a` block.
    try std.testing.expectError(error.CannotMoveContainer, ed.moveKey(&.{.{ .key = "a" }}, &.{.{ .key = "b" }}));
    try std.testing.expectError(error.CannotMoveContainer, ed.moveKey(&.{.{ .key = "other" }}, &.{.{ .key = "b" }}));
    try expectFigSource(&ed, "a\n> x = 1\nother = 1\na\n> y = 2\nb\n> z = 3\n");
    // `moveContainer` carries both fragments.
    try ed.moveContainer(&.{.{ .key = "a" }}, null);
    try expectFigSource(&ed, "other = 1\nb\n> z = 3\n\na\n> x = 1\na\n> y = 2\n");
}

test "fig moveKey still moves scalar entries, inside and around a container" {
    var ed = try newFigEditor("x = 1\ny = 2\na\n> p = 1\n> q = 2\n");
    defer ed.deinit();
    try ed.moveKey(&.{.{ .key = "y" }}, &.{.{ .key = "x" }});
    try ed.moveKey(&.{ .{ .key = "a" }, .{ .key = "q" } }, &.{ .{ .key = "a" }, .{ .key = "p" } });
    try expectFigSource(&ed, "y = 2\nx = 1\na\n> q = 2\n> p = 1\n");
}

test "fig reorderKeys refuses a reorder that shifts a block container" {
    var ed = try newFigEditor("x = 1\na\n> p = 1\nb\n> q = 2\n");
    defer ed.deinit();
    try std.testing.expectError(error.CannotReorderContainers, ed.reorderKeys(&.{}, &.{ "b", "a" }));
    try expectFigSource(&ed, "x = 1\na\n> p = 1\nb\n> q = 2\n");
    // Scalars reordered around a container that keeps its index are fine.
    try ed.reorderKeys(&.{.{ .key = "a" }}, &.{"p"});
    try expectFigSource(&ed, "x = 1\na\n> p = 1\nb\n> q = 2\n");
}

test "fig replaceValAtPath still re-frames a contiguous block container from its recorded separator" {
    // The section rule guards only the ENGINE's splice; a fig header records
    // a zero-width separator, so the reframe runs first and owns the
    // target and re-frames the value in place.
    var ed = try newFigEditor("a\n> p = 1\nz = 0\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "a" }}, "q = 2\nr = 3\n");
    try expectFigSource(&ed, "a\n> q = 2\n> r = 3\nz = 0\n");
}
