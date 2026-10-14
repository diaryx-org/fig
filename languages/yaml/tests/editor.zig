//! YAML editor tests for `Editor(Yaml)`.
//!
//! No YAML-specific editing logic is left. The generic span-splice engine in
//! fig's `editor.zig` does every edit, driven by `yaml.zig`'s `syntax` and by
//! what `parser.zig` records — every entry's `:` in `Document.sepSpan`,
//! every item's `-` in `Document.markerSpan`, the anchor and tag spans — and by
//! the reference layer that is core: the alias node kind, `AST.resolveAlias`
//! and `AST.mergedChild`. This module used to hold three hooks: the value
//! reframe (re-emitting `: value` so a block collection can replace an inline
//! one), which the engine does from the recorded separator; the inherited-key
//! predicate, which the engine asks once `merge_key` is declared; and the
//! follow-mode replace through an alias, which is generic over core kinds.
//!
//! The tests below pin every one of those behaviours in YAML's own shapes;
//! the logic they exercise lives in `editor.zig`.

const std = @import("std");

const AST = @import("kit").AST;
const Document = @import("kit").Document;
const Span = @import("kit").Span;
const editor = @import("fig").editor;
const splice = @import("fig").editor.splice;
const Yaml = @import("fig_yaml").Language;
const log = std.log.scoped(.editor);

/// The concrete editor these reference-layer ops drive. The public methods on
/// `editor.Editor(Yaml)` stay in `editor.zig` (they are the shared, generic
/// entry points); these are the YAML-only pieces they hand off to. `columnOf` is
/// a shared source-coordinate utility defined in `editor.zig`.
const YamlEditor = editor.Editor(Yaml);
const columnOf = splice.columnOf;

// --- reference layer + block-mapping value framing (the YAML arm of the engine) ---

// =======
// TESTS
// =======

fn newYamlEditor(input: []const u8) !editor.Editor(Yaml) {
    var ed: editor.Editor(Yaml) = .{ .allocator = std.testing.allocator };
    try ed.init(input);
    return ed;
}

fn expectSource(ed: *const editor.Editor(Yaml), expected: []const u8) !void {
    errdefer log.err("actual:   \"{s}\"", .{ed.source.items});
    errdefer log.err("expected: \"{s}\"", .{expected});
    try std.testing.expectEqualStrings(expected, ed.source.items);
}

// --- reference layer: COW + opt-in follow ---

test "yaml edit through alias is copy-on-write (severs only that alias)" {
    var ed = try newYamlEditor("a: &x 1\nb: *x\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "b" }}, "5");
    try expectSource(&ed, "a: &x 1\nb: 5\n"); // anchor + value of `a` untouched
}

test "yaml follow-mode edits the anchored value, keeping the anchor" {
    var ed = try newYamlEditor("a: &x 1\nb: *x\n");
    defer ed.deinit();
    try ed.replaceValAtPathFollowing(&.{.{ .key = "b" }}, "5");
    try expectSource(&ed, "a: &x 5\nb: *x\n"); // shared source changed; alias intact
}

test "yaml COW materializes a merge-only key locally" {
    var ed = try newYamlEditor("base: &b\n  x: 1\nd:\n  <<: *b\n  y: 2\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{ .{ .key = "d" }, .{ .key = "x" } }, "5");
    try expectSource(&ed, "base: &b\n  x: 1\nd:\n  <<: *b\n  y: 2\n  x: 5\n");
}

test "yaml deleting a merge-only key is refused" {
    var ed = try newYamlEditor("base: &b\n  x: 1\nd:\n  <<: *b\n  y: 2\n");
    defer ed.deinit();
    try std.testing.expectError(error.MergeOnlyKey, ed.deleteKey(&.{ .{ .key = "d" }, .{ .key = "x" } }));
}

// --- insert key, block ---

test "yaml insert key block" {
    var ed = try newYamlEditor("a: 1\nb: 2\n");
    defer ed.deinit();
    try ed.insertKey(&.{}, "c", "3");
    try expectSource(&ed, "a: 1\nb: 2\nc: 3\n");
}

test "yaml insert key no trailing newline" {
    var ed = try newYamlEditor("a: 1\nb: 2");
    defer ed.deinit();
    try ed.insertKey(&.{}, "c", "3");
    try expectSource(&ed, "a: 1\nb: 2\nc: 3\n");
}

test "yaml insert key nested column inheritance" {
    var ed = try newYamlEditor("root:\n  x: 1\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "root" }}, "y", "2");
    try expectSource(&ed, "root:\n  x: 1\n  y: 2\n");
}

test "yaml insert key multiline block scalar" {
    var ed = try newYamlEditor("a: 1\n");
    defer ed.deinit();
    try ed.insertKey(&.{}, "desc", "|\n  line one\n  line two\n");
    try expectSource(&ed, "a: 1\ndesc: |\n  line one\n  line two\n");
}

// --- insert key, flow / empty ---

test "yaml insert key empty flow map" {
    var ed = try newYamlEditor("env: {}\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "env" }}, "X", "1");
    try expectSource(&ed, "env: {X: 1}\n");
}

test "yaml insert key nonempty flow map" {
    var ed = try newYamlEditor("env: {a: 1}\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "env" }}, "b", "2");
    try expectSource(&ed, "env: {a: 1, b: 2}\n");
}

test "yaml insert key promotes null value" {
    var ed = try newYamlEditor("k:\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "k" }}, "n", "1");
    try expectSource(&ed, "k:\n  n: 1\n");
}

test "yaml insert key with nested mapping value" {
    var ed = try newYamlEditor("a: 1\n");
    defer ed.deinit();
    try ed.insertKey(&.{}, "meta", "x: 1\ny: 2\n");
    try expectSource(&ed, "a: 1\nmeta:\n  x: 1\n  y: 2\n");
}

test "yaml insert key with indentless sequence value" {
    var ed = try newYamlEditor("a: 1\n");
    defer ed.deinit();
    try ed.insertKey(&.{}, "tags", "- x\n- y\n");
    try expectSource(&ed, "a: 1\ntags:\n- x\n- y\n");
}

test "yaml insert key with single-entry mapping value descends like a multi-entry one" {
    // A one-entry mapping renders `k: v` — one line, no dash — so shape alone
    // can't tell it from a scalar. Spliced inline it would read `meta: x: 1`,
    // which is not YAML; it has to descend exactly like the two-entry value in
    // the test above.
    var ed = try newYamlEditor("a: 1\n");
    defer ed.deinit();
    try ed.insertKey(&.{}, "meta", "x: 1\n");
    try expectSource(&ed, "a: 1\nmeta:\n  x: 1\n");
}

test "yaml insert key with single-entry mapping value nests under an existing key" {
    var ed = try newYamlEditor("root:\n  x: 1\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "root" }}, "y", "k: v\n");
    try expectSource(&ed, "root:\n  x: 1\n  y:\n    k: v\n");
}

test "yaml replacing a value with a single-entry mapping reframes it to block" {
    var ed = try newYamlEditor("a: 1\nb: 2\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "a" }}, "k: v\n");
    try expectSource(&ed, "a:\n  k: v\nb: 2\n");
}

test "yaml promoting a null value takes a block value, not just a scalar" {
    // `promoteNullToMapping` writes the new entry through `writeMapValue`, so a
    // block value descends instead of being jammed inline after the `:`.
    var ed = try newYamlEditor("k:\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "k" }}, "n", "x: 1\n");
    try expectSource(&ed, "k:\n  n:\n    x: 1\n");

    var seq = try newYamlEditor("k:\n");
    defer seq.deinit();
    try seq.insertKey(&.{.{ .key = "k" }}, "n", "- q\n");
    try expectSource(&seq, "k:\n  n:\n  - q\n");
}

// --- flow containers refuse block values ---

test "yaml insert into a flow mapping refuses a block value" {
    // `{a: 1, b: - q}` is not the sequence that was asked for — and a lenient
    // reader would take it as the string `"- q"`, losing it silently. Refuse
    // before splicing, naming the reason.
    var ed = try newYamlEditor("env: {a: 1}\n");
    defer ed.deinit();
    try std.testing.expectError(error.BlockValueIntoFlow, ed.insertKey(&.{.{ .key = "env" }}, "b", "- q\n"));
    try expectSource(&ed, "env: {a: 1}\n"); // untouched

    // Same for a block mapping, a block scalar, and an empty flow target.
    try std.testing.expectError(error.BlockValueIntoFlow, ed.insertKey(&.{.{ .key = "env" }}, "b", "k: v\n"));
    try std.testing.expectError(error.BlockValueIntoFlow, ed.insertKey(&.{.{ .key = "env" }}, "b", "|\n  text\n"));
    var empty = try newYamlEditor("env: {}\n");
    defer empty.deinit();
    try std.testing.expectError(error.BlockValueIntoFlow, empty.insertKey(&.{.{ .key = "env" }}, "b", "- q\n"));
    try expectSource(&empty, "env: {}\n");
}

test "yaml appending to a flow sequence refuses a block value" {
    var ed = try newYamlEditor("a: [1]\n");
    defer ed.deinit();
    try std.testing.expectError(error.BlockValueIntoFlow, ed.appendToSeq(&.{.{ .key = "a" }}, "- q\n"));
    try std.testing.expectError(error.BlockValueIntoFlow, ed.prependToSeq(&.{.{ .key = "a" }}, "k: v\n"));
    try expectSource(&ed, "a: [1]\n");
}

test "yaml flow inserts still take scalars and flow containers" {
    // The guard is about BLOCK spelling only: flow values and scalars — including
    // quoted text that merely looks dash- or colon-led — splice as before.
    var ed = try newYamlEditor("env: {a: 1}\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "env" }}, "b", "[x, y]");
    try ed.insertKey(&.{.{ .key = "env" }}, "c", "{k: v}");
    try ed.insertKey(&.{.{ .key = "env" }}, "d", "'- q'");
    try expectSource(&ed, "env: {a: 1, b: [x, y], c: {k: v}, d: '- q'}\n");
}

// --- delete key + trivia ---

test "yaml delete middle key" {
    var ed = try newYamlEditor("a: 1\nb: 2\nc: 3\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectSource(&ed, "a: 1\nc: 3\n");
}

test "yaml delete key with owned comment" {
    var ed = try newYamlEditor("a: 1\n# note\nb: 2\nc: 3\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectSource(&ed, "a: 1\nc: 3\n");
}

test "yaml delete key preserves comment across blank line" {
    var ed = try newYamlEditor("a: 1\n# orphan\n\nb: 2\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectSource(&ed, "a: 1\n# orphan\n\n");
}

test "yaml delete key with multiline comment block" {
    var ed = try newYamlEditor("# l1\n# l2\nb: 2\nc: 3\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectSource(&ed, "c: 3\n");
}

test "yaml delete last key" {
    var ed = try newYamlEditor("a: 1\nb: 2\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectSource(&ed, "a: 1\n");
}

test "yaml delete sole key" {
    var ed = try newYamlEditor("a: 1\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "a" }});
    try expectSource(&ed, "");
}

test "yaml delete key trailing same-line comment" {
    var ed = try newYamlEditor("a: 1\nb: 2 # gone\nc: 3\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "b" }});
    try expectSource(&ed, "a: 1\nc: 3\n");
}

// Regression: a flow mapping's entries are comma-separated on one physical
// line, not one-per-line like a block mapping, so `deleteKey`'s generic
// line-based delete (built for the block shape) used to delete the whole
// containing line — here, the entire (single-line) document — leaving an
// empty file that YAML's empty-document-is-an-empty-mapping grammar then
// accepted, silently committing the data loss instead of erroring. Deleting a
// key *inside* a packed flow mapping must only remove that key.
test "yaml delete key inside a packed flow mapping (regression)" {
    var ed = try newYamlEditor("point: { x: 1, y: 2 }\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "point" }, .{ .key = "y" } });
    try expectSource(&ed, "point: { x: 1 }\n");
}

test "yaml delete first key of a packed flow mapping" {
    var ed = try newYamlEditor("point: { x: 1, y: 2 }\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "point" }, .{ .key = "x" } });
    try expectSource(&ed, "point: { y: 2 }\n");
}

// Regression: deleting the *only* key of a single-entry flow mapping must leave
// an empty flow mapping `{}`, not delete the braces with the line. The old
// block-shaped line delete wiped the whole `point: { x: 1 }` line, and YAML's
// empty-document-is-an-empty-mapping grammar then silently "succeeded",
// committing a data-loss reparse to disk.
test "yaml delete only key of a single-entry flow mapping (regression)" {
    var ed = try newYamlEditor("point: { x: 1 }\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "point" }, .{ .key = "x" } });
    try expectSource(&ed, "point: { }\n");
}

// Regression: deleting the *last* key of a one-entry-per-line flow mapping used
// to strand the predecessor's separator comma before the closing brace; the
// flow-aware splice drops the preceding comma instead.
test "yaml delete last key of a multi-line flow mapping (regression)" {
    var ed = try newYamlEditor("point: {\n  x: 1,\n  y: 2\n}\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "point" }, .{ .key = "y" } });
    try expectSource(&ed, "point: {\n  x: 1\n}\n");
}

test "yaml delete key with block scalar value" {
    var ed = try newYamlEditor("a: |\n  x\n  y\nb: 2\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "a" }});
    try expectSource(&ed, "b: 2\n");
}

// --- sequences ---

test "yaml append block seq" {
    var ed = try newYamlEditor("- a\n- b\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{}, "c");
    try expectSource(&ed, "- a\n- b\n- c\n");
}

test "yaml append indentless seq" {
    var ed = try newYamlEditor("one:\n- 2\n- 3\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "one" }}, "4");
    try expectSource(&ed, "one:\n- 2\n- 3\n- 4\n");
}

test "yaml append indented nested seq" {
    var ed = try newYamlEditor("k:\n  - a\n  - b\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "k" }}, "c");
    try expectSource(&ed, "k:\n  - a\n  - b\n  - c\n");
}

test "yaml prepend block seq" {
    var ed = try newYamlEditor("- a\n- b\n");
    defer ed.deinit();
    try ed.prependToSeq(&.{}, "z");
    try expectSource(&ed, "- z\n- a\n- b\n");
}

test "yaml append flow seq" {
    var ed = try newYamlEditor("t: [a, b]\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "t" }}, "c");
    try expectSource(&ed, "t: [a, b, c]\n");
}

test "yaml append empty flow seq" {
    var ed = try newYamlEditor("t: []\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "t" }}, "a");
    try expectSource(&ed, "t: [a]\n");
}

test "yaml append flow seq with pre-existing trailing comma" {
    // A trailing comma before ']' is legal flow-sequence syntax; appending
    // must not double it into an empty element that fails to reparse.
    var ed = try newYamlEditor("t: [a, b,]\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "t" }}, "c");
    try expectSource(&ed, "t: [a, b, c,]\n");
}

test "yaml append onto a multi-line one-item-per-line flow seq" {
    var ed = try newYamlEditor("t: [\n  a,\n  b,\n]\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{.{ .key = "t" }}, "c");
    try expectSource(&ed, "t: [\n  a,\n  b,\n  c,\n]\n");
}

test "yaml insertItem: a block item at an index, above the item there and its comment" {
    var ed = try newYamlEditor("- a\n# about b\n- b\n");
    defer ed.deinit();
    try ed.insertItem(&.{}, 1, "m");
    try expectSource(&ed, "- a\n- m\n# about b\n- b\n");
    // 0 prepends and the length appends, as their own ops do.
    try ed.insertItem(&.{}, 0, "first");
    try ed.insertItem(&.{}, 4, "last");
    try expectSource(&ed, "- first\n- a\n- m\n# about b\n- b\n- last\n");
    // Past the end is refused, and nothing is written.
    try std.testing.expectError(error.NotFound, ed.insertItem(&.{}, 9, "x"));
    try expectSource(&ed, "- first\n- a\n- m\n# about b\n- b\n- last\n");
}

test "yaml insertItem: a nested block item takes its siblings' indent" {
    var ed = try newYamlEditor("k:\n  - a\n  - b\n");
    defer ed.deinit();
    try ed.insertItem(&.{.{ .key = "k" }}, 1, "m");
    try expectSource(&ed, "k:\n  - a\n  - m\n  - b\n");
}

test "yaml insertItem: a flow item at an index, on one line and one per line" {
    var ed = try newYamlEditor("t: [a, b, c]\n");
    defer ed.deinit();
    try ed.insertItem(&.{.{ .key = "t" }}, 2, "m");
    try expectSource(&ed, "t: [a, b, m, c]\n");
    try ed.insertItem(&.{.{ .key = "t" }}, std.math.maxInt(usize), "z");
    try expectSource(&ed, "t: [a, b, m, c, z]\n");

    var multi = try newYamlEditor("t: [\n  a,\n  b,\n]\n");
    defer multi.deinit();
    try multi.insertItem(&.{.{ .key = "t" }}, 1, "m");
    try expectSource(&multi, "t: [\n  a,\n  m,\n  b,\n]\n");
    // 0 is a prepend, which keeps the dialect's one-line spelling.
    try multi.insertItem(&.{.{ .key = "t" }}, 0, "z");
    try expectSource(&multi, "t: [\n  z, a,\n  m,\n  b,\n]\n");
}

test "yaml remove block seq middle" {
    var ed = try newYamlEditor("- a\n- b\n- c\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{}, 1);
    try expectSource(&ed, "- a\n- c\n");
}

test "yaml remove flow seq middle" {
    var ed = try newYamlEditor("t: [a, b, c]\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "t" }}, 1);
    try expectSource(&ed, "t: [a, c]\n");
}

test "yaml remove flow seq first" {
    var ed = try newYamlEditor("t: [a, b]\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "t" }}, 0);
    try expectSource(&ed, "t: [b]\n");
}

test "yaml remove last flow item via the [-] end sentinel" {
    var ed = try newYamlEditor("t: [a, b, c]\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "t" }}, std.math.maxInt(usize));
    try expectSource(&ed, "t: [a, b]\n");
}

test "yaml remove last item of a multi-line trailing-comma flow seq (regression)" {
    // Regression: the backward scan for the preceding separator comma only
    // skipped spaces/tabs, not newlines, so it never found the previous
    // item's comma across a one-item-per-line layout — leaving the removed
    // item's own trailing comma dangling as an empty element that failed to
    // reparse.
    var ed = try newYamlEditor("t: [\n  a,\n  b,\n]\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "t" }}, std.math.maxInt(usize));
    try expectSource(&ed, "t: [\n  a,\n]\n");
}

test "yaml remove middle item of a multi-line one-item-per-line flow seq" {
    var ed = try newYamlEditor("t: [\n  a,\n  b,\n  c,\n]\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "t" }}, 1);
    try expectSource(&ed, "t: [\n  a,\n  c,\n]\n");
}

// A block sequence is nothing but its items, so the line delete of its last
// one used to leave the key null (`links:`). It leaves `[]`, as a flow list
// does.

test "yaml remove the only item of a block seq leaves an empty flow seq" {
    var ed = try newYamlEditor("links:\n  - a\nother: 1\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "links" }}, 0);
    try expectSource(&ed, "links: []\nother: 1\n");
    const parsed = try ed.getParsed();
    const node = try parsed.ast.getValByPath(&.{.{ .key = "links" }});
    try std.testing.expect(node.kind == .sequence);
}

test "yaml remove the only item of a block seq keeps the key's comments" {
    var ed = try newYamlEditor("# links\nlinks: # keep\n  # the item's\n  - a # its own\nother: 1\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "links" }}, 0);
    try expectSource(&ed, "# links\nlinks: [] # keep\nother: 1\n");
}

test "yaml remove the only item of a nested, indentless or last block seq" {
    var nested = try newYamlEditor("x:\n  links:\n    - a\n  y: 2\n");
    defer nested.deinit();
    try nested.removeSeqItem(&.{ .{ .key = "x" }, .{ .key = "links" } }, 0);
    try expectSource(&nested, "x:\n  links: []\n  y: 2\n");

    var indentless = try newYamlEditor("links:\n- a\nother: 1\n");
    defer indentless.deinit();
    try indentless.removeSeqItem(&.{.{ .key = "links" }}, 0);
    try expectSource(&indentless, "links: []\nother: 1\n");

    var last = try newYamlEditor("title: x\nlinks:\n  - k: 1\n    j: 2");
    defer last.deinit();
    try last.removeSeqItem(&.{.{ .key = "links" }}, std.math.maxInt(usize));
    try expectSource(&last, "title: x\nlinks: []");
}

test "yaml remove the only item of a block seq keeps its anchor and tag" {
    var anchored = try newYamlEditor("links: &x\n  - a\nother: *x\n");
    defer anchored.deinit();
    try anchored.removeSeqItem(&.{.{ .key = "links" }}, 0);
    try expectSource(&anchored, "links: &x []\nother: *x\n");

    var tagged = try newYamlEditor("links: !!seq\n  - a\n");
    defer tagged.deinit();
    try tagged.removeSeqItem(&.{.{ .key = "links" }}, 0);
    try expectSource(&tagged, "links: !!seq []\n");
}

test "yaml remove the only item of a block seq inside a block seq" {
    var own_line = try newYamlEditor("-\n  - a\n- b\n");
    defer own_line.deinit();
    try own_line.removeSeqItem(&.{.{ .index = 0 }}, 0);
    try expectSource(&own_line, "- []\n- b\n");

    var shared = try newYamlEditor("- - a\n- b\n");
    defer shared.deinit();
    try shared.removeSeqItem(&.{.{ .index = 0 }}, 0);
    try expectSource(&shared, "- []\n- b\n");
}

test "yaml remove the first item of a block seq on its parent's marker line keeps the parent" {
    // The line delete took the outer `-` with the item: `[[a, c]]` read
    // back as `[c]`. The next item moves up onto the marker line instead.
    var root = try newYamlEditor("- - a\n  - c\n");
    defer root.deinit();
    try root.removeSeqItem(&.{.{ .index = 0 }}, 0);
    try expectSource(&root, "- - c\n");

    var keyed = try newYamlEditor("k:\n  - - a\n    - c\nj: 1\n");
    defer keyed.deinit();
    try keyed.removeSeqItem(&.{ .{ .key = "k" }, .{ .index = 0 } }, 0);
    try expectSource(&keyed, "k:\n  - - c\nj: 1\n");

    var deep = try newYamlEditor("- - - a # gone\n\n    - c\n- d\n");
    defer deep.deinit();
    try deep.removeSeqItem(&.{ .{ .index = 0 }, .{ .index = 0 } }, 0);
    try expectSource(&deep, "- - - c\n- d\n");

    // A comment line above the next item is the next item's: it stays, and
    // the parent's marker ends its line.
    var commented = try newYamlEditor("- - a # gone\n  # c's\n  - c\n");
    defer commented.deinit();
    try commented.removeSeqItem(&.{.{ .index = 0 }}, 0);
    try expectSource(&commented, "-\n  # c's\n  - c\n");
}

test "yaml delete inside compact items two deep" {
    // These were refused with DeleteChangedOtherValues while the parser
    // misread the shapes (`[[[a, c], d]]` as `[[[a, c]], d]`).
    var first = try newYamlEditor("- - - a\n    - c\n  - d\n");
    defer first.deinit();
    try first.removeSeqItem(&.{ .{ .index = 0 }, .{ .index = 0 } }, 0);
    try expectSource(&first, "- - - c\n  - d\n");

    var after = try newYamlEditor("- - - a\n    - c\n  - d\n");
    defer after.deinit();
    try after.removeSeqItem(&.{.{ .index = 0 }}, 1);
    try expectSource(&after, "- - - a\n    - c\n");

    var only = try newYamlEditor("- - - a\n  - d\n");
    defer only.deinit();
    try only.removeSeqItem(&.{ .{ .index = 0 }, .{ .index = 0 } }, 0);
    try expectSource(&only, "- - []\n  - d\n");

    var key = try newYamlEditor("- - k: v\n    j: w\n  - c\n");
    defer key.deinit();
    try key.deleteKey(&.{ .{ .index = 0 }, .{ .index = 0 }, .{ .key = "k" } });
    try expectSource(&key, "- - j: w\n  - c\n");

    var mapping = try newYamlEditor("- - k: v\n  - c\n");
    defer mapping.deinit();
    try mapping.removeSeqItem(&.{.{ .index = 0 }}, 0);
    try expectSource(&mapping, "- - c\n");
}

test "yaml delete the first key of a mapping on its item's marker line keeps the item" {
    var ed = try newYamlEditor("- k: v\n  j: w\n- x\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .index = 0 }, .{ .key = "k" } });
    try expectSource(&ed, "- j: w\n- x\n");

    var nested = try newYamlEditor("- k:\n    - a\n  j: w\n");
    defer nested.deinit();
    try nested.deleteKey(&.{ .{ .index = 0 }, .{ .key = "k" } });
    try expectSource(&nested, "- j: w\n");

    // The only key: the item stays, emptied to null as `k:` is.
    var only = try newYamlEditor("- k: v\n- x\n");
    defer only.deinit();
    try only.deleteKey(&.{ .{ .index = 0 }, .{ .key = "k" } });
    try expectSource(&only, "-\n- x\n");
}

test "yaml a delete that would change another value is rolled back" {
    // An explicit key's span starts at `k`, past its `?`, so no splice of
    // it reads back as `{j: w}`; the guard refuses whatever it would write.
    const source = "- ? k\n  : v\n  j: w\n";
    var ed = try newYamlEditor(source);
    defer ed.deinit();
    try std.testing.expectError(error.DeleteChangedOtherValues, ed.deleteKey(&.{ .{ .index = 0 }, .{ .key = "k" } }));
    try expectSource(&ed, source);
}

test "yaml a flow mapping delete passes the guard, and the line delete of a flow pair does not" {
    var ed = try newYamlEditor("o: {a: 1, b: NaN}\np: x\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "o" }, .{ .key = "a" } });
    try expectSource(&ed, "o: {b: NaN}\np: x\n");
    try ed.deleteKey(&.{ .{ .key = "o" }, .{ .key = "b" } });
    try expectSource(&ed, "o: {}\np: x\n");

    // A single-pair mapping in a flow sequence has no braces of its own, and
    // the delete would take the whole `n:` line; the guard refuses
    // (docs/tasks/delete-splices-taking-other-values.md).
    const pairs = "n: [a: b, c: d]\nm: 1\n";
    var flow = try newYamlEditor(pairs);
    defer flow.deinit();
    try std.testing.expectError(error.DeleteChangedOtherValues, flow.deleteKey(&.{ .{ .key = "n" }, .{ .index = 0 }, .{ .key = "a" } }));
    try expectSource(&flow, pairs);
}

test "yaml a delete does not take an implicit mapping's parent" {
    // A YAML block mapping is never implicit: the key that holds it stays,
    // null, when its only entry goes.
    var ed = try newYamlEditor("k:\n  a: 1\nj: 2\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "k" }, .{ .key = "a" } });
    try expectSource(&ed, "k:\nj: 2\n");
}

test "yaml remove the only item of a root block seq" {
    var ed = try newYamlEditor("---\n- a\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{}, 0);
    try expectSource(&ed, "---\n[]\n");
}

test "yaml remove the only item of a frontmatter block seq" {
    // What a Markdown host hands the editor: the block between the fences.
    var ed = try newYamlEditor("title: Note\ntags:\n  - draft\nlinks:\n  - \"[a](/a.md)\"\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "links" }}, 0);
    try ed.removeSeqItem(&.{.{ .key = "tags" }}, 0);
    try expectSource(&ed, "title: Note\ntags: []\nlinks: []\n");
}

// --- atomicity ---

test "yaml failed edit rolls back source and keeps editor usable" {
    var ed = try newYamlEditor("a: 1\nb: 2\n");
    defer ed.deinit();

    // Splice an unterminated flow sequence as a's value: the reparse fails
    // (the specific error is the parser's concern; we only require failure).
    if (ed.replaceValAtPath(&.{.{ .key = "a" }}, "[oops")) |_| {
        return error.TestExpectedFailedEdit;
    } else |_| {}
    // Source is byte-identical to before the failed edit...
    try expectSource(&ed, "a: 1\nb: 2\n");
    // ...and the document still matches it, so a later valid edit works.
    try ed.replaceValAtPath(&.{.{ .key = "a" }}, "9");
    try expectSource(&ed, "a: 9\nb: 2\n");
}

// --- value reframing on replace (inline <-> block) ---

test "yaml replace inline empty seq with a block list" {
    var ed = try newYamlEditor("t: []\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "t" }}, "- a\n- b");
    try expectSource(&ed, "t:\n- a\n- b\n");
}

test "yaml replace block list with an inline empty seq" {
    var ed = try newYamlEditor("t:\n- a\n- b\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "t" }}, "[]");
    try expectSource(&ed, "t: []\n");
}

test "yaml replace scalar with a single-item block list" {
    var ed = try newYamlEditor("k: old\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "k" }}, "- a");
    try expectSource(&ed, "k:\n- a\n");
}

test "yaml replace null value with a block list" {
    var ed = try newYamlEditor("k:\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "k" }}, "- a");
    try expectSource(&ed, "k:\n- a\n");
}

test "yaml replace scalar with a nested mapping" {
    var ed = try newYamlEditor("m: x\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "m" }}, "a: 1\nb: 2");
    try expectSource(&ed, "m:\n  a: 1\n  b: 2\n");
}

test "yaml replace scalar with scalar keeps it inline" {
    var ed = try newYamlEditor("title: Hello\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "title" }}, "Hi");
    try expectSource(&ed, "title: Hi\n");
}

// A block value written over an item goes under the item's content column,
// as the printer writes a map in a list. Its second line at column 0 put `y`
// in the mapping around the list, and the replace reported success.
test "yaml replace an item with a block value indents it under the item" {
    const cases = [_]struct { source: []const u8, path: []const AST.PathSegment, value: []const u8, expected: []const u8 }{
        // scalar → map, under a key
        .{ .source = "k:\n  - x\n", .path = &.{ .{ .key = "k" }, .{ .index = 0 } }, .value = "x: 1\ny: two", .expected = "k:\n  - x: 1\n    y: two\n" },
        // scalar → list, at the root
        .{ .source = "- x\n- z\n", .path = &.{.{ .index = 0 }}, .value = "- 1\n- 2", .expected = "- - 1\n  - 2\n- z\n" },
        // map → map
        .{ .source = "k:\n  - a: 1\n    b: 2\n  - z\n", .path = &.{ .{ .key = "k" }, .{ .index = 0 } }, .value = "x: 1\ny: two", .expected = "k:\n  - x: 1\n    y: two\n  - z\n" },
        // list → list, nested in a compact item
        .{ .source = "- - - a\n    - b\n  - c\n", .path = &.{ .{ .index = 0 }, .{ .index = 0 } }, .value = "- 1\n- 2", .expected = "- - - 1\n    - 2\n  - c\n" },
        // a compact item's scalar
        .{ .source = "- - x\n  - z\n", .path = &.{ .{ .index = 0 }, .{ .index = 0 } }, .value = "x: 1\ny: two", .expected = "- - x: 1\n    y: two\n  - z\n" },
        // the item's comments stay
        .{ .source = "# lead\n- x # tail\n- z\n", .path = &.{.{ .index = 0 }}, .value = "x: 1\ny: two", .expected = "# lead\n- x: 1\n  y: two # tail\n- z\n" },
        .{ .source = "- # c\n  a: 1\n  b: 2\n- z\n", .path = &.{.{ .index = 0 }}, .value = "x: 1\ny: two", .expected = "- # c\n  x: 1\n  y: two\n- z\n" },
        // CRLF: the value's own lines are written as the editor writes lines
        .{ .source = "a: 1\r\nb:\r\n  - x\r\n  - y\r\n", .path = &.{ .{ .key = "b" }, .{ .index = 0 } }, .value = "x: 1\ny: two", .expected = "a: 1\r\nb:\r\n  - x: 1\n    y: two\r\n  - y\r\n" },
        // a block scalar's item keeps the line break its span took
        .{ .source = "- >-\n  folded\n- |\n  lit\n", .path = &.{.{ .index = 0 }}, .value = "42", .expected = "- 42\n- |\n  lit\n" },
    };
    for (cases) |c| {
        var ed = try newYamlEditor(c.source);
        defer ed.deinit();
        try ed.replaceValAtPath(c.path, c.value);
        try expectSource(&ed, c.expected);
    }
}

// An explicit key's new value is written from its own `:` line; written
// after the `? k` instead, `? k:` over `  x: 1` was a key that is itself a
// mapping, and the write guard (`readsBack`) refused it.
test "yaml replace an explicit key's value with a block mapping" {
    var ed = try newYamlEditor("? k\n: v\nz: 1\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "k" }}, "x: 1\ny: two");
    try expectSource(&ed, "? k\n:\n  x: 1\n  y: two\nz: 1\n");
}

test "yaml replace preserves a trailing line comment" {
    var ed = try newYamlEditor("title: Hello # note\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "title" }}, "Hi");
    try expectSource(&ed, "title: Hi # note\n");
}

test "yaml reframe a nested mapping value" {
    var ed = try newYamlEditor("root:\n  c: []\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{ .{ .key = "root" }, .{ .key = "c" } }, "- x");
    try expectSource(&ed, "root:\n  c:\n  - x\n");
}

// --- move key ---

test "yaml move key forward (src before dest)" {
    var ed = try newYamlEditor("a: 1\nb: 2\nc: 3\n");
    defer ed.deinit();
    // Move a to before c: a lands between b and c.
    try ed.moveKey(&.{.{ .key = "a" }}, &.{.{ .key = "c" }});
    try expectSource(&ed, "b: 2\na: 1\nc: 3\n");
}

test "yaml move key backward (dest before src)" {
    var ed = try newYamlEditor("a: 1\nb: 2\nc: 3\n");
    defer ed.deinit();
    // Move c to before a.
    try ed.moveKey(&.{.{ .key = "c" }}, &.{.{ .key = "a" }});
    try expectSource(&ed, "c: 3\na: 1\nb: 2\n");
}

test "yaml move key carries owned comment" {
    var ed = try newYamlEditor("a: 1\n# note for c\nc: 3\nb: 2\n");
    defer ed.deinit();
    try ed.moveKey(&.{.{ .key = "c" }}, &.{.{ .key = "a" }});
    try expectSource(&ed, "# note for c\nc: 3\na: 1\nb: 2\n");
}

test "yaml move key carries trailing same-line comment" {
    var ed = try newYamlEditor("a: 1\nb: 2 # keep\nc: 3\n");
    defer ed.deinit();
    try ed.moveKey(&.{.{ .key = "b" }}, &.{.{ .key = "a" }});
    try expectSource(&ed, "b: 2 # keep\na: 1\nc: 3\n");
}

test "yaml move key with block scalar value" {
    var ed = try newYamlEditor("a: |\n  x\n  y\nb: 2\n");
    defer ed.deinit();
    try ed.moveKey(&.{.{ .key = "b" }}, &.{.{ .key = "a" }});
    try expectSource(&ed, "b: 2\na: |\n  x\n  y\n");
}

test "yaml move key to itself is a no-op" {
    var ed = try newYamlEditor("a: 1\nb: 2\n");
    defer ed.deinit();
    try ed.moveKey(&.{.{ .key = "a" }}, &.{.{ .key = "a" }});
    try expectSource(&ed, "a: 1\nb: 2\n");
}

// --- reorder keys ---

test "yaml reorder keys full order" {
    var ed = try newYamlEditor("a: 1\nb: 2\nc: 3\n");
    defer ed.deinit();
    try ed.reorderKeys(&.{}, &.{ "c", "a", "b" });
    try expectSource(&ed, "c: 3\na: 1\nb: 2\n");
}

test "yaml reorder keys of a flow mapping keeps its commas" {
    var ed = try newYamlEditor("m: {a: 1, b: 2, c: 3}\n");
    defer ed.deinit();
    try ed.reorderKeys(&.{.{ .key = "m" }}, &.{ "c", "a" });
    try expectSource(&ed, "m: {c: 3, a: 1, b: 2}\n");
    try ed.moveKey(&.{ .{ .key = "m" }, .{ .key = "b" } }, &.{ .{ .key = "m" }, .{ .key = "c" } });
    try expectSource(&ed, "m: {b: 2, c: 3, a: 1}\n");
}

test "yaml reorder keys partial appends rest in original order" {
    var ed = try newYamlEditor("a: 1\nb: 2\nc: 3\nd: 4\n");
    defer ed.deinit();
    // Only c, a listed; b and d keep their original relative order after.
    try ed.reorderKeys(&.{}, &.{ "c", "a" });
    try expectSource(&ed, "c: 3\na: 1\nb: 2\nd: 4\n");
}

test "yaml reorder keys preserves owned comments" {
    var ed = try newYamlEditor("# about a\na: 1\nb: 2\n# about c\nc: 3\n");
    defer ed.deinit();
    try ed.reorderKeys(&.{}, &.{ "c", "b", "a" });
    try expectSource(&ed, "# about c\nc: 3\nb: 2\n# about a\na: 1\n");
}

test "yaml reorder keys preserves interleaved blank line with preceding entry" {
    var ed = try newYamlEditor("a: 1\n\nb: 2\nc: 3\n");
    defer ed.deinit();
    // The blank line rides with a (its preceding entry).
    try ed.reorderKeys(&.{}, &.{ "c", "a", "b" });
    try expectSource(&ed, "c: 3\na: 1\n\nb: 2\n");
}

test "yaml reorder keys unknown key ignored" {
    var ed = try newYamlEditor("a: 1\nb: 2\n");
    defer ed.deinit();
    try ed.reorderKeys(&.{}, &.{ "z", "b", "a" });
    try expectSource(&ed, "b: 2\na: 1\n");
}

test "yaml reorder keys no-op when order matches" {
    var ed = try newYamlEditor("a: 1\nb: 2\nc: 3\n");
    defer ed.deinit();
    try ed.reorderKeys(&.{}, &.{ "a", "b", "c" });
    try expectSource(&ed, "a: 1\nb: 2\nc: 3\n");
}

test "yaml reorder keys empty list keeps original order" {
    var ed = try newYamlEditor("a: 1\nb: 2\n");
    defer ed.deinit();
    try ed.reorderKeys(&.{}, &.{});
    try expectSource(&ed, "a: 1\nb: 2\n");
}

test "yaml reorder keys nested mapping" {
    var ed = try newYamlEditor("root:\n  x: 1\n  y: 2\n  z: 3\n");
    defer ed.deinit();
    try ed.reorderKeys(&.{.{ .key = "root" }}, &.{ "z", "x" });
    try expectSource(&ed, "root:\n  z: 3\n  x: 1\n  y: 2\n");
}

test "yaml reorder keys with block scalar entry" {
    var ed = try newYamlEditor("a: 1\nbody: |\n  line one\n  line two\nb: 2\n");
    defer ed.deinit();
    try ed.reorderKeys(&.{}, &.{ "body", "b", "a" });
    try expectSource(&ed, "body: |\n  line one\n  line two\nb: 2\na: 1\n");
}

// --- move sequence item ---

test "yaml move item block forward" {
    var ed = try newYamlEditor("- a\n- b\n- c\n");
    defer ed.deinit();
    // Move item 0 (a) to index 2: remove a, reinsert -> b, c, a.
    try ed.moveItem(&.{}, 0, 2);
    try expectSource(&ed, "- b\n- c\n- a\n");
}

test "yaml move item block backward" {
    var ed = try newYamlEditor("- a\n- b\n- c\n");
    defer ed.deinit();
    try ed.moveItem(&.{}, 2, 0);
    try expectSource(&ed, "- c\n- a\n- b\n");
}

test "yaml move item carries owned comment" {
    var ed = try newYamlEditor("- a\n# note for c\n- c\n- b\n");
    defer ed.deinit();
    // Items: a(0), c(1), b(2). Move c to the front.
    try ed.moveItem(&.{}, 1, 0);
    try expectSource(&ed, "# note for c\n- c\n- a\n- b\n");
}

test "yaml move item to itself is a no-op" {
    var ed = try newYamlEditor("- a\n- b\n");
    defer ed.deinit();
    try ed.moveItem(&.{}, 1, 1);
    try expectSource(&ed, "- a\n- b\n");
}

test "yaml move flow item" {
    var ed = try newYamlEditor("t: [a, b, c]\n");
    defer ed.deinit();
    try ed.moveItem(&.{.{ .key = "t" }}, 0, 2);
    try expectSource(&ed, "t: [b, c, a]\n");
}

// --- reorder sequence items ---

test "yaml reorder items block partial" {
    var ed = try newYamlEditor("- a\n- b\n- c\n");
    defer ed.deinit();
    // Bring index 2 then 0 to the front; the rest (b) follows in order.
    try ed.reorderItems(&.{}, &.{ 2, 0 });
    try expectSource(&ed, "- c\n- a\n- b\n");
}

test "yaml reorder items nested under key" {
    var ed = try newYamlEditor("tags:\n- x\n- y\n- z\n");
    defer ed.deinit();
    try ed.reorderItems(&.{.{ .key = "tags" }}, &.{ 2, 1, 0 });
    try expectSource(&ed, "tags:\n- z\n- y\n- x\n");
}

test "yaml reorder items indented nested seq" {
    var ed = try newYamlEditor("k:\n  - a\n  - b\n  - c\n");
    defer ed.deinit();
    try ed.reorderItems(&.{.{ .key = "k" }}, &.{1});
    try expectSource(&ed, "k:\n  - b\n  - a\n  - c\n");
}

test "yaml reorder items preserves owned comment" {
    var ed = try newYamlEditor("- a\n# note for b\n- b\n- c\n");
    defer ed.deinit();
    try ed.reorderItems(&.{}, &.{ 1, 0 });
    try expectSource(&ed, "# note for b\n- b\n- a\n- c\n");
}

test "yaml reorder flow items keeps spaced separators" {
    var ed = try newYamlEditor("t: [a, b, c]\n");
    defer ed.deinit();
    try ed.reorderItems(&.{.{ .key = "t" }}, &.{ 2, 0 });
    try expectSource(&ed, "t: [c, a, b]\n");
}

test "yaml reorder flow items keeps tight separators" {
    var ed = try newYamlEditor("t: [a,b,c]\n");
    defer ed.deinit();
    try ed.reorderItems(&.{.{ .key = "t" }}, &.{ 2, 1, 0 });
    try expectSource(&ed, "t: [c,b,a]\n");
}

test "yaml reorder items out of range index ignored" {
    var ed = try newYamlEditor("- a\n- b\n");
    defer ed.deinit();
    try ed.reorderItems(&.{}, &.{ 9, 1 });
    try expectSource(&ed, "- b\n- a\n");
}

test "yaml reorder items no-op when order matches" {
    var ed = try newYamlEditor("- a\n- b\n- c\n");
    defer ed.deinit();
    try ed.reorderItems(&.{}, &.{ 0, 1, 2 });
    try expectSource(&ed, "- a\n- b\n- c\n");
}

test "yaml reorder items empty list keeps original order" {
    var ed = try newYamlEditor("- a\n- b\n");
    defer ed.deinit();
    try ed.reorderItems(&.{}, &.{});
    try expectSource(&ed, "- a\n- b\n");
}

// --- setSequence: comment-preserving list reconcile ---

test "yaml setSequence keeps surviving items' comments while reordering" {
    var ed = try newYamlEditor("tags:\n- a # first\n- b # second\n- c # third\n");
    defer ed.deinit();
    // Drop b, add d, and reorder to [c, a, d]. a and c keep their comments.
    try ed.setSequence(&.{.{ .key = "tags" }}, &.{ "c", "a", "d" });
    try expectSource(&ed, "tags:\n- c # third\n- a # first\n- d\n");
}

test "yaml setSequence pure reorder preserves every comment" {
    var ed = try newYamlEditor("- a # ca\n- b # cb\n- c # cc\n");
    defer ed.deinit();
    try ed.setSequence(&.{}, &.{ "c", "b", "a" });
    try expectSource(&ed, "- c # cc\n- b # cb\n- a # ca\n");
}

test "yaml setSequence is a byte-identical no-op when unchanged" {
    var ed = try newYamlEditor("- a\n# note\n- b\n");
    defer ed.deinit();
    try ed.setSequence(&.{}, &.{ "a", "b" });
    try expectSource(&ed, "- a\n# note\n- b\n");
}

test "yaml setSequence appends a new item keeping existing comments" {
    var ed = try newYamlEditor("- a # ka\n- b # kb\n");
    defer ed.deinit();
    try ed.setSequence(&.{}, &.{ "a", "b", "c" });
    try expectSource(&ed, "- a # ka\n- b # kb\n- c\n");
}

test "yaml setSequence handles a full replacement (no survivors)" {
    var ed = try newYamlEditor("- a\n- b\n");
    defer ed.deinit();
    try ed.setSequence(&.{}, &.{ "c", "d" });
    try expectSource(&ed, "- c\n- d\n");
}

test "yaml setSequence dedups by occurrence (keeps the right duplicate's comment)" {
    var ed = try newYamlEditor("- a # one\n- a # two\n- b\n");
    defer ed.deinit();
    // Two a's -> one a: the first occurrence survives, the second is dropped.
    try ed.setSequence(&.{}, &.{ "a", "b" });
    try expectSource(&ed, "- a # one\n- b\n");
}

test "yaml setSequence matches by value, not lexical form (int vs string)" {
    var ed = try newYamlEditor("- 1 # int\n- '1' # str\n");
    defer ed.deinit();
    // Swap order; int 1 and string '1' are distinct identities and keep comments.
    try ed.setSequence(&.{}, &.{ "'1'", "1" });
    try expectSource(&ed, "- '1' # str\n- 1 # int\n");
}

test "yaml setSequence declines an empty target" {
    var ed = try newYamlEditor("- a\n- b\n");
    defer ed.deinit();
    try std.testing.expectError(error.UnsupportedShape, ed.setSequence(&.{}, &.{}));
    try expectSource(&ed, "- a\n- b\n");
}

test "yaml setSequence declines a non-scalar current item" {
    var ed = try newYamlEditor("- a\n- k: v\n");
    defer ed.deinit();
    try std.testing.expectError(error.UnsupportedShape, ed.setSequence(&.{}, &.{ "a", "z" }));
    try expectSource(&ed, "- a\n- k: v\n");
}

test "yaml setSequence declines a non-sequence target" {
    var ed = try newYamlEditor("a: 1\n");
    defer ed.deinit();
    try std.testing.expectError(error.NotASequence, ed.setSequence(&.{.{ .key = "a" }}, &.{"x"}));
}

test "yaml setSequence starts a list that is absent or null" {
    var ed = try newYamlEditor("a: 1\n");
    defer ed.deinit();
    try ed.setSequence(&.{.{ .key = "tags" }}, &.{ "x", "y" });
    try expectSource(&ed, "a: 1\ntags: [x, y]\n");
    var null_ed = try newYamlEditor("tags:\n");
    defer null_ed.deinit();
    try null_ed.setSequence(&.{.{ .key = "tags" }}, &.{"x"});
    try expectSource(&null_ed, "tags: [x]\n");
}

// --- setAppend: `set`'s `[-]`, which starts a missing list ---

test "yaml setAppend appends to a list that is there" {
    var ed = try newYamlEditor("tags:\n  - a\n");
    defer ed.deinit();
    try ed.setAppend(&.{.{ .key = "tags" }}, "b");
    try expectSource(&ed, "tags:\n  - a\n  - b\n");
}

test "yaml setAppend starts an absent list, and its missing parents" {
    var ed = try newYamlEditor("a: 1\n");
    defer ed.deinit();
    try ed.setAppend(&.{.{ .key = "tags" }}, "x");
    try ed.setAppend(&.{.{ .key = "tags" }}, "y");
    try expectSource(&ed, "a: 1\ntags: [x, y]\n");
    try ed.setAppend(&.{ .{ .key = "p" }, .{ .key = "q" } }, "z");
    try expectSource(&ed, "a: 1\ntags: [x, y]\np:\n  q: [z]\n");
}

test "yaml setAppend starts a list where a null stands, and in an empty document" {
    var ed = try newYamlEditor("tags:\n");
    defer ed.deinit();
    try ed.setAppend(&.{.{ .key = "tags" }}, "x");
    try expectSource(&ed, "tags: [x]\n");
    var empty = try newYamlEditor("");
    defer empty.deinit();
    try empty.setAppend(&.{.{ .key = "tags" }}, "x");
    try expectSource(&empty, "tags: [x]\n");
}

test "yaml setAppend never clobbers a scalar or a mapping" {
    var ed = try newYamlEditor("a: 1\nm:\n  k: v\n");
    defer ed.deinit();
    try std.testing.expectError(error.NotASequence, ed.setAppend(&.{.{ .key = "a" }}, "x"));
    try std.testing.expectError(error.NotASequence, ed.setAppend(&.{.{ .key = "m" }}, "x"));
    try std.testing.expectError(error.NotAMapping, ed.setAppend(&.{ .{ .key = "a" }, .{ .key = "l" } }, "x"));
    try expectSource(&ed, "a: 1\nm:\n  k: v\n");
}

test "yaml setAppend takes its seed back when the item is refused" {
    var ed = try newYamlEditor("a: 1\n");
    defer ed.deinit();
    // A block mapping cannot go into the flow `[]` the seed is.
    try std.testing.expectError(error.BlockValueIntoFlow, ed.setAppend(&.{.{ .key = "tags" }}, "k: v\nj: w"));
    try expectSource(&ed, "a: 1\n");
}

test "a key inserted into an item's mapping aligns under the item's keys, past each marker" {
    var ed = try newYamlEditor("x:\n  - k: v\n    j: w\n");
    defer ed.deinit();
    try ed.set(&.{ .{ .key = "x" }, .{ .index = 0 }, .{ .key = "n" } }, "1");
    try expectSource(&ed, "x:\n  - k: v\n    j: w\n    n: 1\n");
    var nested = try newYamlEditor("x:\n  - - k: v\n");
    defer nested.deinit();
    try nested.set(&.{ .{ .key = "x" }, .{ .index = 0 }, .{ .index = 0 }, .{ .key = "n" } }, "1");
    try expectSource(&nested, "x:\n  - - k: v\n      n: 1\n");
}

// --- replace: shapes whose value slot is not where the value's text is ---

// A plain scalar ends before the blanks that precede its comment, a tab
// included; with the tab in its span, a replace wrote `42# c`, the comment
// read as more of the value.
test "yaml replace a value with a tab before its comment" {
    var ed = try newYamlEditor("a: 1\t# c\nb: 2\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "a" }}, "42");
    try expectSource(&ed, "a: 42\t# c\nb: 2\n");
}

// An empty tagged value covers its tag, as a written one does (`t: !!str y`
// replaced is `t: 42`); anchored before it, a replace wrote `x !!str`.
test "yaml replace an empty tagged value" {
    var ed = try newYamlEditor("t: !!str\nu: 1\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "t" }}, "x");
    try expectSource(&ed, "t: x\nu: 1\n");
}

test "yaml replace an empty tagged sequence item" {
    var ed = try newYamlEditor("- !!str\n- 1\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .index = 0 }}, "5");
    try expectSource(&ed, "- 5\n- 1\n");
}

// An explicit key's `:` opens a line of its own; the value is rewritten
// from there, and the key keeps its `?` line.
test "yaml replace an explicit key's value" {
    var ed = try newYamlEditor("? k\n: v # c\nz: 1\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{.{ .key = "k" }}, "42");
    try expectSource(&ed, "? k\n: 42 # c\nz: 1\n");
}

test "yaml replace an explicit key's empty value with a block list" {
    var ed = try newYamlEditor("m:\n  ? k\n  :\n  z: 1\n");
    defer ed.deinit();
    try ed.replaceValAtPath(&.{ .{ .key = "m" }, .{ .key = "k" } }, "- 1\n- 2");
    try expectSource(&ed, "m:\n  ? k\n  :\n  - 1\n  - 2\n  z: 1\n");
}
