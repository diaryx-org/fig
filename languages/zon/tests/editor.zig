//! ZON editor tests for `Editor(Zon)`.
//!
//! ZON's editing needs are small parameter swaps on the generic engine, not the
//! structural logic TOML (multi-region tables)/YAML (reference layer)/Fig
//! (marker-prefix copying) delegate out to their own helper modules:
//!   * every ZON struct/array literal opens `.{`, its declared
//!     `flow_map_open`, which `editor/splice.zig`'s `isFlow` treats as flow,
//!     so ZON routes through the same flow-entry splice engine JSON does
//!     (see `editor.zig`'s `kv_sep`/`flowOpenEnd`);
//!   * struct fields separate key and value with ` = `, not `: ` (`kv_sep`);
//!   * a struct-field key needs a leading `.` (`.name`), quoted as
//!     `.@"..."` when it isn't a bare identifier: ZON's `renderKeyName`,
//!     which writes it through the printer's `writeDotName`.

const std = @import("std");

const AST = @import("kit").AST;
const editor = @import("fig").editor;
const Zon = @import("fig_zon").Language;
const log = std.log.scoped(.editor);

fn newZonEditor(input: []const u8) !editor.Editor(Zon) {
    var ed: editor.Editor(Zon) = .{ .allocator = std.testing.allocator };
    try ed.init(input);
    return ed;
}

fn expectZonSource(ed: *const editor.Editor(Zon), expected: []const u8) !void {
    errdefer log.err("actual:   \"{s}\"", .{ed.source.items});
    errdefer log.err("expected: \"{s}\"", .{expected});
    try std.testing.expectEqualStrings(expected, ed.source.items);
}

// Regression: a compact single-line struct packs multiple fields onto one
// physical line, so `deleteKey`'s generic line-based delete used to remove
// the whole line. ZON compounds this: a field's key span starts at the bare
// identifier, not its leading `.` (its declared `key_sigil`; see
// `insertFlowMapEntry`), so the comma-aware flow-splice fallback must also
// back up over that `.` itself, or it strands a bare `.` next to the
// surviving sibling instead of removing the field cleanly.
test "zon delete a key from a packed single-line struct (regression)" {
    var ed = try newZonEditor(".{ .a = 1, .b = 2, .c = 3 }");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectZonSource(&ed, ".{ .a = 1, .c = 3 }");
}

test "zon delete first key of a packed single-line struct" {
    var ed = try newZonEditor(".{ .a = 1, .b = 2 }");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "a" }});
    try expectZonSource(&ed, ".{ .b = 2 }");
}

// Regression: deleting the *only* field of a single-field struct must leave an
// empty struct `.{}`, not delete the braces with the line. The `.`-backup of the
// flow splice must still fire so no bare `.` is stranded.
test "zon delete only key of a single-field struct (regression)" {
    var ed = try newZonEditor(".{ .a = 1 }");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "a" }});
    try expectZonSource(&ed, ".{ }");
}

// Regression: deleting the *last* field of a one-field-per-line struct used to
// strand the predecessor's separator comma before the closing brace.
test "zon delete last key of a multi-line struct (regression)" {
    var ed = try newZonEditor(".{\n    .a = 1,\n    .b = 2\n}");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectZonSource(&ed, ".{\n    .a = 1\n}");
}

test "zon array append/prepend/remove" {
    var ed = try newZonEditor(".{ 1, 2 }");
    defer ed.deinit();
    try ed.appendToSeq(&.{}, "3");
    try expectZonSource(&ed, ".{ 1, 2, 3 }");
    try ed.prependToSeq(&.{}, "0");
    try expectZonSource(&ed, ".{ 0, 1, 2, 3 }");
    try ed.removeSeqItem(&.{}, 2);
    try expectZonSource(&ed, ".{ 0, 1, 3 }");
}

test "zon appendToSeq on an empty .{} is NotASequence (ZON has no way to tell an empty struct from an empty array)" {
    // Unlike JSON's `[]`/`{}` or YAML's `[]`/`{}`, ZON's struct and array
    // literals share one empty spelling (`.{}`); fig's parser resolves that
    // ambiguity by always treating an empty container as a mapping (see
    // `zon/parser.zig`'s "empty .{} is an empty mapping" test). So there is no
    // ZON source an empty *array* append could target — the caller must seed a
    // non-empty array (or a struct) explicitly.
    var ed = try newZonEditor(".{}");
    defer ed.deinit();
    try std.testing.expectError(error.NotASequence, ed.appendToSeq(&.{}, "1"));
}

test "zon setAppend starts an absent list as .{}, which its first item makes a tuple" {
    // The seed is the same `.{}` the test above cannot append to; what
    // `setAppend` wrote it as is a list, and the item goes in as one.
    var ed = try newZonEditor(".{ .a = 1 }");
    defer ed.deinit();
    try ed.setAppend(&.{.{ .key = "tags" }}, "\"x\"");
    try expectZonSource(&ed, ".{ .a = 1, .tags = .{\"x\"} }");
    try ed.setAppend(&.{.{ .key = "tags" }}, "\"y\"");
    try expectZonSource(&ed, ".{ .a = 1, .tags = .{\"x\", \"y\"} }");
    try ed.setSequence(&.{.{ .key = "l" }}, &.{ "1", "2" });
    try expectZonSource(&ed, ".{ .a = 1, .tags = .{\"x\", \"y\"}, .l = .{1, 2} }");
    // An empty struct that was there is real data, and is not taken for one.
    var empty = try newZonEditor(".{ .m = .{} }");
    defer empty.deinit();
    try std.testing.expectError(error.NotASequence, empty.setAppend(&.{.{ .key = "m" }}, "1"));
}

test "zon append onto a multi-line one-item-per-line array" {
    var ed = try newZonEditor(
        \\.{
        \\    1,
        \\    2,
        \\}
    );
    defer ed.deinit();
    try ed.appendToSeq(&.{}, "3");
    try expectZonSource(&ed,
        \\.{
        \\    1,
        \\    2,
        \\    3,
        \\}
    );
}

test "zon set replaces an existing key" {
    var ed = try newZonEditor(".{ .a = 1 }");
    defer ed.deinit();
    try ed.set(&.{.{ .key = "a" }}, "2");
    try expectZonSource(&ed, ".{ .a = 2 }");
}

test "zon set creates a missing bare-identifier key with a leading dot" {
    var ed = try newZonEditor(".{ .a = 1 }");
    defer ed.deinit();
    try ed.set(&.{.{ .key = "b" }}, "2");
    try expectZonSource(&ed, ".{ .a = 1, .b = 2 }");
}

test "zon set creates a missing key needing @\"...\" quoting" {
    var ed = try newZonEditor(".{ .a = 1 }");
    defer ed.deinit();
    try ed.set(&.{.{ .key = "has space" }}, "2");
    try expectZonSource(&ed, ".{ .a = 1, .@\"has space\" = 2 }");
}

test "zon set auto-vivifies a missing parent as an empty struct" {
    var ed = try newZonEditor(".{}");
    defer ed.deinit();
    try ed.set(&.{ .{ .key = "server" }, .{ .key = "port" } }, "8080");
    try expectZonSource(&ed, ".{.server = .{.port = 8080}}");
}

test "zon insertKey promotes a null value to a struct" {
    var ed = try newZonEditor(".{ .a = null }");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "a" }}, ".b", "1");
    try expectZonSource(&ed, ".{ .a = .{ .b = 1 } }");
}

test "zon failed edit rolls back and keeps editor usable" {
    var ed = try newZonEditor(".{ .a = 1 }");
    defer ed.deinit();
    if (ed.replaceValAtPath(&.{.{ .key = "a" }}, "[oops")) |_| {
        return error.TestExpectedFailedEdit;
    } else |_| {}
    try expectZonSource(&ed, ".{ .a = 1 }");
    try ed.replaceValAtPath(&.{.{ .key = "a" }}, "9");
    try expectZonSource(&ed, ".{ .a = 9 }");
}
