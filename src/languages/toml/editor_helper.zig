//! TOML editor tests for `Editor(Toml)`.
//!
//! No TOML-specific editing logic is left. The generic span-splice engine in
//! `../../editor.zig` does every edit, driven by `toml.zig`'s `syntax` and by
//! what `parser.zig` records: a table's header lines in
//! `Document.node_regions` — a TOML table is assembled from scattered
//! `[header]`, `[[array]]` and dotted lines, and `../../editor/regions.zig`
//! derives its whole region set from those — and every place a table's name
//! is written in `Document.node_mentions`. This module used to hold the ops
//! that had to SPELL something TOML: a `[header]`/`[[header]]` line, which
//! `Syntax.section_header` now declares; every mention of a table's name,
//! which the parser now records and the engine's `renameContainer` rewrites
//! at once; and an `insertKey` that kept a new entry inside the intended
//! table's own region, which the engine does by skipping children under a
//! header of their own.
//!
//! The tests below pin every one of those behaviours in TOML's own shapes;
//! the logic they exercise lives in `editor.zig`.

const std = @import("std");

const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const Span = @import("../../util/span.zig");
const editor = @import("../../editor.zig");
const splice = @import("../../editor/splice.zig");
const Toml = @import("toml.zig").Language;
const log = std.log.scoped(.editor);

/// The concrete editor these structural ops drive. All functions below take a
/// `*TomlEditor`: they are the TOML arm of the generic engine, factored out so
/// `editor.zig` stays format-agnostic and every TOML edit lives in one file. The
/// public methods on `editor.Editor(Toml)` are thin wrappers that call these.
const TomlEditor = editor.Editor(Toml);

// Shared source-coordinate / rendering utilities (defined in editor.zig).
const lineStartBefore = splice.lineStartBefore;
const lineEndAfter = splice.lineEndAfter;
const firstNonSpace = splice.firstNonSpace;
const isFlow = splice.isFlow;

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
