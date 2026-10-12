//! The nestedtext editor tests: the generic engine (`../../editor.zig`) driven
//! over this language, with its renderers (`edit.zig`).

const std = @import("std");
const testing = std.testing;
const AST = @import("../../kit/ast/ast.zig");
const Document = @import("../../kit/document.zig");
const Span = @import("../../kit/span.zig");
const editor = @import("../../editor.zig");
const lang = @import("../../kit/manifest.zig");
const splice = @import("../../editor/splice.zig");
const NestedText = @import("nestedtext.zig").Language;
const Parser = @import("parser.zig");
const NtEditor = editor.Editor(NestedText);
const lineStartBefore = splice.lineStartBefore;
const lineEndAfter = splice.lineEndAfter;
const firstNonSpace = splice.firstNonSpace;
const columnOf = splice.columnOf;
const edit = @import("edit.zig");
const appendValueTail = edit.appendValueTail;
const nestedBlock = edit.nestedBlock;
const appendRootBlock = edit.appendRootBlock;
const needsMultilineKey = edit.needsMultilineKey;
const appendMultilineKeyLines = edit.appendMultilineKeyLines;
const isMultilineKeyText = edit.isMultilineKeyText;
const renderEntry = edit.renderEntry;
const renderItem = edit.renderItem;
const renderTail = edit.renderTail;
const renderKey = edit.renderKey;

// ── Tests ────────────────────────────────────────────────────────────────────

fn expectEdit(comptime op: []const u8, src: []const u8, args: anytype, expected: []const u8) !void {
    var ed: NtEditor = .{ .allocator = testing.allocator, .format = .NESTEDTEXT };
    try ed.init(src);
    defer ed.deinit();
    try @call(.auto, @field(NtEditor, op), .{&ed} ++ args);
    errdefer std.log.err("actual: \"{s}\"", .{ed.source.items});
    try testing.expectEqualStrings(expected, ed.source.items);
}

test "insertKey: promotes an empty document to the first root key" {
    try expectEdit("set", "", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "fig" }, "name: fig\n");
}

test "insertKey: appends after the last existing entry, matching indent" {
    try expectEdit("insertKey", "a: 1\n", .{ &[_]AST.PathSegment{}, "b", "2" }, "a: 1\nb: 2\n");
    try expectEdit(
        "insertKey",
        "server:\n    host: localhost\n",
        .{ &[_]AST.PathSegment{.{ .key = "server" }}, "port", "80" },
        "server:\n    host: localhost\n    port: 80\n",
    );
}

test "insertKey: empty/multiline value renders as a nested `>`-block" {
    try expectEdit("insertKey", "a: 1\n", .{ &[_]AST.PathSegment{}, "b", "" }, "a: 1\nb:\n    >\n");
    try expectEdit("insertKey", "a: 1\n", .{ &[_]AST.PathSegment{}, "b", "line1\nline2" }, "a: 1\nb:\n    > line1\n    > line2\n");
}

test "insertKey: a key needing multiline form gets the `: key` spelling" {
    try expectEdit("insertKey", "a: 1\n", .{ &[_]AST.PathSegment{}, "- looks like a list tag", "v" }, "a: 1\n: - looks like a list tag\n    > v\n");
}

test "insertKey: fills a childless inline `{}` through the generic flow insert" {
    try expectEdit("insertKey", "{}", .{ &[_]AST.PathSegment{}, "a", "1" }, "{a: 1}");
}

test "set: same-line scalar replace, autodetecting old shape" {
    try expectEdit("set", "name: fig\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "zig" }, "name: zig\n");
}

test "set: switches a same-line value to a nested `>`-block and back" {
    try expectEdit("set", "name: fig\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "line1\nline2" }, "name:\n    > line1\n    > line2\n");
    try expectEdit("set", "name:\n    > line1\n    > line2\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "fig" }, "name: fig\n");
}

test "set: empty value becomes the nested bare `>` block" {
    try expectEdit("set", "name: fig\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "" }, "name:\n    >\n");
    try expectEdit("set", "name:\n    >\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "fig" }, "name: fig\n");
}

test "set: replaces a whole nested container value with a scalar" {
    try expectEdit(
        "set",
        "server:\n    host: localhost\n    port: 80\n",
        .{ &[_]AST.PathSegment{.{ .key = "server" }}, "disabled" },
        "server: disabled\n",
    );
}

test "set: on a multiline key's value stays nested even for a short value" {
    try expectEdit(
        "set",
        ": key 1\n: spread over 2 lines\n    > value 1\n",
        .{ &[_]AST.PathSegment{.{ .key = "key 1\nspread over 2 lines" }}, "v" },
        ": key 1\n: spread over 2 lines\n    > v\n",
    );
}

test "replaceValAtPath: on a list item, by index" {
    // `set` only ever creates/replaces a MAPPING entry (its path must end in
    // `.key`); an `.index`-ending or path-less (root) target goes through
    // `replaceValAtPath` directly, same as every other language.
    try expectEdit("replaceValAtPath", "- a\n- b\n- c\n", .{ &[_]AST.PathSegment{.{ .index = 1 }}, "z" }, "- a\n- z\n- c\n");
}

test "replaceValAtPath: on a list item whose value is nested/empty" {
    try expectEdit(
        "replaceValAtPath",
        "- a\n-\n    nested: 1\n- c\n",
        .{ &[_]AST.PathSegment{.{ .index = 1 }}, "b" },
        "- a\n- b\n- c\n",
    );
}

test "replaceValAtPath: the whole-document root value" {
    try expectEdit("replaceValAtPath", "> hello\n", .{ &[_]AST.PathSegment{}, "goodbye" }, "> goodbye\n");
    try expectEdit("replaceValAtPath", "", .{ &[_]AST.PathSegment{}, "hi" }, "> hi\n");
}

test "replaceKeyAtPath: plain to plain" {
    try expectEdit("replaceKeyAtPath", "name: fig\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "lang" }, "lang: fig\n");
}

test "replaceKeyAtPath: multiline to plain adds the separator colon" {
    try expectEdit(
        "replaceKeyAtPath",
        ": - looks like a list tag\n    > v\n",
        .{ &[_]AST.PathSegment{.{ .key = "- looks like a list tag" }}, "plain" },
        "plain:\n    > v\n",
    );
}

test "replaceKeyAtPath: plain to multiline is declined when the value is same-line" {
    var ed: NtEditor = .{ .allocator = testing.allocator, .format = .NESTEDTEXT };
    try ed.init("name: fig\n");
    defer ed.deinit();
    try testing.expectError(error.KeyRequiresMultilineForm, ed.replaceKeyAtPath(&.{.{ .key = "name" }}, "- oops"));
}

test "appendToSeq / prependToSeq put a scalar item on its own line" {
    try expectEdit("appendToSeq", "- a\n- b\n", .{ &[_]AST.PathSegment{}, "c" }, "- a\n- b\n- c\n");
    try expectEdit("prependToSeq", "- a\n- b\n", .{ &[_]AST.PathSegment{}, "z" }, "- z\n- a\n- b\n");
}

test "appendToSeq / prependToSeq render an empty/multiline item as a nested `>`-block" {
    try expectEdit("appendToSeq", "- a\n", .{ &[_]AST.PathSegment{}, "" }, "- a\n-\n    >\n");
    try expectEdit("prependToSeq", "- a\n", .{ &[_]AST.PathSegment{}, "l1\nl2" }, "-\n    > l1\n    > l2\n- a\n");
}

test "appendToSeq: after a nested-value first item, indent still matches the sequence" {
    try expectEdit(
        "appendToSeq",
        "-\n    nested: 1\n",
        .{ &[_]AST.PathSegment{}, "b" },
        "-\n    nested: 1\n- b\n",
    );
}

test "removeSeqItem removes a nested/empty-valued item cleanly, leaving siblings intact" {
    try expectEdit(
        "removeSeqItem",
        "- a\n-\n    nested: 1\n- c\n",
        .{ &[_]AST.PathSegment{}, @as(usize, 1) },
        "- a\n- c\n",
    );
    // Removing the first item (nested/empty) is anchored via the sequence's
    // own span, not a (nonexistent) previous sibling.
    try expectEdit(
        "removeSeqItem",
        "-\n    nested: 1\n- b\n",
        .{ &[_]AST.PathSegment{}, @as(usize, 0) },
        "- b\n",
    );
}

test "removeSeqItem carries a leading comment above a nested item" {
    try expectEdit(
        "removeSeqItem",
        "- a\n# note\n-\n    nested: 1\n- c\n",
        .{ &[_]AST.PathSegment{}, @as(usize, 1) },
        "- a\n- c\n",
    );
}

test "moveItem / reorderItems relocate whole (possibly nested) item blocks" {
    try expectEdit(
        "moveItem",
        "- a\n-\n    nested: 1\n- c\n",
        .{ &[_]AST.PathSegment{}, @as(usize, 2), @as(usize, 0) },
        "- c\n- a\n-\n    nested: 1\n",
    );
    try expectEdit(
        "reorderItems",
        "- a\n- b\n- c\n",
        .{ &[_]AST.PathSegment{}, &[_]usize{ 2, 0 } },
        "- c\n- a\n- b\n",
    );
}

test "addLeadingComment / getLeadingComment / deleteLeadingComments on a nested-valued list item" {
    var ed: NtEditor = .{ .allocator = testing.allocator, .format = .NESTEDTEXT };
    try ed.init("- a\n-\n    nested: 1\n- c\n");
    defer ed.deinit();
    const path = &[_]AST.PathSegment{.{ .index = 1 }};
    try ed.addLeadingComment(path, "note");
    try testing.expectEqualStrings("- a\n# note\n-\n    nested: 1\n- c\n", ed.source.items);
    const got = (try ed.getLeadingComment(path)).?;
    defer testing.allocator.free(got);
    try testing.expectEqualStrings("note", got);
    try ed.deleteLeadingComments(path);
    try testing.expectEqualStrings("- a\n-\n    nested: 1\n- c\n", ed.source.items);
}

test "trailing comments are unsupported (no same-line comment spelling)" {
    var ed: NtEditor = .{ .allocator = testing.allocator, .format = .NESTEDTEXT };
    try ed.init("name: fig\n");
    defer ed.deinit();
    const path = &[_]AST.PathSegment{.{ .key = "name" }};
    try testing.expectError(error.CommentsUnsupported, ed.setTrailingComment(path, "note"));
    try testing.expectError(error.CommentsUnsupported, ed.getTrailingComment(path));
    try testing.expectError(error.CommentsUnsupported, ed.deleteTrailingComment(path));
}

test "deleteKey removes a whole nested-value entry (generic engine, no override needed)" {
    try expectEdit(
        "deleteKey",
        "a: 1\nserver:\n    host: localhost\n    port: 80\nb: 2\n",
        .{&[_]AST.PathSegment{.{ .key = "server" }}},
        "a: 1\nb: 2\n",
    );
}

test "a value that opens with a line break and reads as NestedText is nested, not blocked" {
    const P = &[_]AST.PathSegment{.{ .key = "m" }};
    // Splice text for a mapping, a list and an empty dict (`printSplice`).
    try expectEdit("set", "k: v\n", .{ P, "\nx: 1\ny:\n    - a" }, "k: v\nm:\n    x: 1\n    y:\n        - a\n");
    try expectEdit("set", "k: v\n", .{ P, "\n- a\n- b" }, "k: v\nm:\n    - a\n    - b\n");
    try expectEdit("set", "k: v\n", .{ P, "\n{}" }, "k: v\nm:\n    {}\n");
    try expectEdit("set", "k: v\nm: 1\n", .{ P, "\nx: 1" }, "k: v\nm:\n    x: 1\n");
    // A string argument stays a string: no leading break, or a rest that is
    // not NestedText on its own.
    try expectEdit("set", "k: v\n", .{ P, "x: 1\ny: 2" }, "k: v\nm:\n    > x: 1\n    > y: 2\n");
    try expectEdit("set", "k: v\n", .{ P, "\nhello" }, "k: v\nm:\n    >\n    > hello\n");
}
