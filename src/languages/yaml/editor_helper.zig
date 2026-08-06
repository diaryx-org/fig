//! YAML-specific editing logic and tests for `Editor(Yaml)`.
//!
//! Mirrors `toml/editor_helper.zig`: the format-specific arm of the generic
//! engine lives here, next to the tests that exercise it, so `editor.zig` stays
//! format-agnostic. The logic below is the YAML reference layer (merge-key
//! detection, anchored-value spans) plus block-mapping value reframing, reached
//! as HOOKS: `yaml.zig` declares each on its `Language` and the generic engine
//! dispatches on `@hasDecl`, naming no format. See that file's "Editing hooks"
//! block for which operations YAML takes over and why.
//!
//! The tests cover the public `Editor(Yaml)` surface end-to-end — alias
//! copy-on-write / opt-in follow / merge materialization, inline<->block value
//! reframing, comment-aware delete/move, and block/flow sequence ops.

const std = @import("std");

const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const Span = @import("../../util/span.zig");
const editor = @import("../../editor.zig");
const Yaml = @import("yaml.zig").Language;
const log = std.log.scoped(.editor);

/// The concrete editor these reference-layer ops drive. The public methods on
/// `editor.Editor(Yaml)` stay in `editor.zig` (they are the shared, generic
/// entry points); these are the YAML-only pieces they hand off to. `columnOf` is
/// a shared source-coordinate utility defined in `editor.zig`.
const YamlEditor = editor.Editor(Yaml);
const columnOf = editor.columnOf;

// --- reference layer + block-mapping value framing (the YAML arm of the engine) ---

/// Replace a mapping key's value, re-emitting `: value` through `writeMapValue`
/// so the new value's framing (inline scalar vs block collection on following
/// lines) is always valid regardless of the old value's shape.
///
/// The `replaceValAtPath` hook (see `editor.Editor.replaceValAtPath`), so it
/// owns every target — but only a MAPPING VALUE can change between inline and
/// block framing. A sequence item or the document root has no `key:` to
/// re-emit, so those take the generic direct splice, here rather than in the
/// engine. `node` is unused: the decision is `path`'s to make.
pub fn reframeMappingValue(self: *YamlEditor, parsed: Document, path: []const AST.PathSegment, node: AST.Node, val_span: Span, replacement: []const u8) !void {
    _ = node;
    if (path.len == 0 or std.meta.activeTag(path[path.len - 1]) != .key)
        return self.replaceAtSpan(val_span, replacement);
    const source = self.source.items;
    const key_node = try parsed.ast.getKeyByPath(path);
    const key_span = parsed.span(key_node);
    const col = columnOf(source, key_span.start);
    // The `:` indicator sits just past the key (a plain key cannot contain `:`,
    // and a quoted key's `:` is inside `key_span`).
    const colon = std.mem.indexOfScalarPos(u8, source, key_span.end, ':') orelse
        return error.InvalidDocument;

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(self.allocator);
    // writeMapValue emits the `:` itself, so replace from the existing colon
    // through the old value's end (a null value is a zero-width span at the
    // colon, hence the `@max`).
    try self.writeMapValue(&out, col, replacement);
    const end = @max(val_span.end, colon + 1);
    try self.replaceAtSpan(Span.init(colon, end), out.items);
}

/// True when `path`'s final `.key` segment is not a physical entry of its parent
/// mapping but is supplied by a `<<` merge.
pub fn mergeSuppliesKey(parsed: Document, path: []const AST.PathSegment) !bool {
    if (path.len == 0 or std.meta.activeTag(path[path.len - 1]) != .key) return false;
    const parent = parsed.ast.getValByPath(path[0 .. path.len - 1]) catch return false;
    if (parent.kind != .mapping) return false;
    return (parsed.ast.mergedChild(parent, path[path.len - 1].key) catch return false) != null;
}

/// Follow-mode replace: when the target is an alias (`b: *x`), edit the
/// ANCHORED value instead, so every alias to that anchor reflects the change.
///
/// The `replaceValAtPathFollowing` hook (see
/// `editor.Editor.replaceValAtPathFollowing`). A non-alias target is handed
/// straight back to the plain `replaceValAtPath`, which is that op's documented
/// contract — so the whole of "following" is this one branch, and the reference
/// layer stays out of the generic engine. `span` is unused: the alias arm
/// splices the anchored node's span, not the alias's.
pub fn replaceAliasTarget(self: *YamlEditor, parsed: Document, path: []const AST.PathSegment, node: AST.Node, span: Span, replacement: []const u8) !void {
    _ = span;
    if (node.kind == .alias) {
        const target = parsed.ast.nodes[try parsed.ast.resolveAlias(node)];
        return self.replaceAtSpan(valueSpanWithoutProps(self, parsed, target), replacement);
    }
    return self.replaceValAtPath(path, replacement);
}

/// The span of `node`'s value bytes, excluding any leading `&anchor`/`!tag`
/// property prefix (the node's stored span starts at the property). Used by
/// follow-mode so editing the anchored value keeps the anchor intact.
pub fn valueSpanWithoutProps(self: *YamlEditor, parsed: Document, node: AST.Node) Span {
    const source = self.source.items;
    const full = parsed.span(node);
    var start = full.start;
    if (parsed.anchorSpan(node)) |a| start = @max(start, a.end);
    if (parsed.tagSpan(node)) |t| start = @max(start, t.end);
    while (start < full.end and (source[start] == ' ' or source[start] == '\t')) start += 1;
    return Span.init(start, full.end);
}

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
