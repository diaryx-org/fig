//! The plist editor tests: the generic engine (`fig.editor`) driven
//! over this language, with its renderers (`edit.zig`).

const std = @import("std");
const testing = std.testing;
const AST = @import("kit").AST;
const Document = @import("kit").Document;
const Span = @import("kit").Span;
const editor = @import("fig").editor;
const splice = @import("fig").editor.splice;
const Plist = @import("fig_plist").Language;
const lang = @import("kit").manifest;
const PlistEditor = editor.Editor(Plist);
const edit = @import("fig_plist").edit;
const renderValue = edit.renderValue;
const wrapText = edit.wrapText;
const appendEscaped = edit.appendEscaped;
const renderEntry = edit.renderEntry;
const renderKey = edit.renderKey;

// ── tests ────────────────────────────────────────────────────────────────────

const wrapper =
    \\<?xml version="1.0" encoding="UTF-8"?>
    \\<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    \\<plist version="1.0">
;

fn expectEdit(comptime op: []const u8, src: []const u8, args: anytype, expected: []const u8) !void {
    var ed: PlistEditor = .{ .allocator = testing.allocator, .format = .XML };
    try ed.init(src);
    defer ed.deinit();
    try @call(.auto, @field(PlistEditor, op), .{&ed} ++ args);
    try testing.expectEqualStrings(expected, ed.source.items);
}

test "renderValue: the engine's literal picks the typed element" {
    const cases = [_]struct { in: []const u8, out: []const u8 }{
        .{ .in = "42", .out = "<integer>42</integer>" },
        .{ .in = "-7", .out = "<integer>-7</integer>" },
        .{ .in = "3.14", .out = "<real>3.14</real>" },
        .{ .in = "true", .out = "<true/>" },
        .{ .in = "false", .out = "<false/>" },
        .{ .in = "hello", .out = "<string>hello</string>" },
        .{ .in = "007", .out = "<string>007</string>" }, // leading zero stays string
        .{ .in = "Yes", .out = "<string>Yes</string>" }, // Norway-safe
        .{ .in = "2026-07-08", .out = "<date>2026-07-08</date>" },
        .{ .in = "a < b & c", .out = "<string>a &lt; b &amp; c</string>" },
        .{ .in = "<data>SGk=</data>", .out = "<data>SGk=</data>" }, // escape hatch
        .{ .in = "<string>2.0</string>", .out = "<string>2.0</string>" }, // force string
    };
    for (cases) |c| {
        var out: std.ArrayList(u8) = .empty;
        defer out.deinit(testing.allocator);
        try renderValue(.XML, testing.allocator, &out, .{ .value = c.in, .literal = editor.literalOf(c.in) });
        try testing.expectEqualStrings(c.out, out.items);
    }
}

test "renderValue: null has no plist type" {
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(testing.allocator);
    try testing.expectError(error.NullUnsupported, renderValue(.XML, testing.allocator, &out, .{ .value = "null", .literal = .null }));
}

test "replaceKeyAtPath renames a key inside its element, escaped" {
    try expectEdit("replaceKeyAtPath", "<dict><key>a</key><string>x</string></dict>", .{ &[_]AST.PathSegment{.{ .key = "a" }}, "b&c" }, "<dict><key>b&amp;c</key><string>x</string></dict>");
}

test "set replaces a value, preserving or changing type by autodetection" {
    // string -> string
    try expectEdit("set", "<dict><key>name</key><string>fig</string></dict>", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "zig" }, "<dict><key>name</key><string>zig</string></dict>");
    // autodetect promotes to integer
    try expectEdit("set", "<dict><key>n</key><integer>1</integer></dict>", .{ &[_]AST.PathSegment{.{ .key = "n" }}, "99" }, "<dict><key>n</key><integer>99</integer></dict>");
    // escape hatch forces string over a numeric-looking value
    try expectEdit("set", "<dict><key>v</key><string>1.0</string></dict>", .{ &[_]AST.PathSegment{.{ .key = "v" }}, "<string>2.0</string>" }, "<dict><key>v</key><string>2.0</string></dict>");
}

test "an empty dict after its key on one line expands under the line's indent, not the dict's column" {
    try expectEdit(
        "insertKey",
        "<dict>\n\t<key>a</key><dict/>\n</dict>\n",
        .{ &[_]AST.PathSegment{.{ .key = "a" }}, "x", "y" },
        "<dict>\n\t<key>a</key><dict>\n\t  <key>x</key>\n\t  <string>y</string>\n\t</dict>\n</dict>\n",
    );
}

test "setAppend starts an absent array as <array/>, expanded around its first item" {
    try expectEdit("setAppend", wrapper ++ "\n<dict>\n  <key>a</key>\n  <integer>1</integer>\n</dict>\n</plist>\n", .{ &[_]AST.PathSegment{.{ .key = "tags" }}, "x" }, wrapper ++
        \\
        \\<dict>
        \\  <key>a</key>
        \\  <integer>1</integer>
        \\  <key>tags</key>
        \\  <array>
        \\    <string>x</string>
        \\  </array>
        \\</dict>
        \\</plist>
        \\
    );
}

test "insertKey refuses a dict that closes on its last entry's line" {
    // Appending after the line would land the entry in the outer dict.
    var ed: PlistEditor = .{ .allocator = testing.allocator, .format = .XML };
    try ed.init("<dict>\n\t<key>o</key><dict><key>a</key><string>x</string></dict>\n</dict>\n");
    defer ed.deinit();
    try testing.expectError(error.ContainerClosesOnItsLine, ed.insertKey(&[_]AST.PathSegment{.{ .key = "o" }}, "b", "y"));
}

test "appendToSeq refuses an array that closes on its last item's line" {
    var ed: PlistEditor = .{ .allocator = testing.allocator, .format = .XML };
    try ed.init("<dict>\n\t<key>l</key><array><string>a</string></array>\n</dict>\n");
    defer ed.deinit();
    try testing.expectError(error.ContainerClosesOnItsLine, ed.appendToSeq(&[_]AST.PathSegment{.{ .key = "l" }}, "b"));
}

test "insertKey appends a two-line entry at the children's indent" {
    try expectEdit(
        "insertKey",
        wrapper ++ "\n<dict>\n  <key>a</key>\n  <integer>1</integer>\n</dict>\n</plist>\n",
        .{ &[_]AST.PathSegment{}, "b", "hello" },
        wrapper ++ "\n<dict>\n  <key>a</key>\n  <integer>1</integer>\n  <key>b</key>\n  <string>hello</string>\n</dict>\n</plist>\n",
    );
}

test "insertKey expands an empty <dict/>" {
    try expectEdit(
        "insertKey",
        wrapper ++ "\n<dict/>\n</plist>\n",
        .{ &[_]AST.PathSegment{}, "a", "1" },
        wrapper ++ "\n<dict>\n  <key>a</key>\n  <integer>1</integer>\n</dict>\n</plist>\n",
    );
}

test "deleteKey (generic path, xml_comment style) removes the whole entry" {
    try expectEdit(
        "deleteKey",
        wrapper ++ "\n<dict>\n  <key>a</key>\n  <integer>1</integer>\n  <key>b</key>\n  <string>x</string>\n</dict>\n</plist>\n",
        .{&[_]AST.PathSegment{.{ .key = "a" }}},
        wrapper ++ "\n<dict>\n  <key>b</key>\n  <string>x</string>\n</dict>\n</plist>\n",
    );
}

test "array append and prepend put an element on its own line (no dash)" {
    try expectEdit(
        "appendToSeq",
        wrapper ++ "\n<array>\n  <string>one</string>\n</array>\n</plist>\n",
        .{ &[_]AST.PathSegment{}, "two" },
        wrapper ++ "\n<array>\n  <string>one</string>\n  <string>two</string>\n</array>\n</plist>\n",
    );
    try expectEdit(
        "prependToSeq",
        wrapper ++ "\n<array>\n  <string>one</string>\n</array>\n</plist>\n",
        .{ &[_]AST.PathSegment{}, "zero" },
        wrapper ++ "\n<array>\n  <string>zero</string>\n  <string>one</string>\n</array>\n</plist>\n",
    );
}

test "leading comment add / get / delete round-trips through <!-- -->" {
    var ed: PlistEditor = .{ .allocator = testing.allocator, .format = .XML };
    try ed.init(wrapper ++ "\n<dict>\n  <key>a</key>\n  <integer>1</integer>\n</dict>\n</plist>\n");
    defer ed.deinit();
    const path = &[_]AST.PathSegment{.{ .key = "a" }};
    try ed.addLeadingComment(path, "the answer");
    try testing.expect(std.mem.indexOf(u8, ed.source.items, "<!-- the answer -->") != null);
    const got = (try ed.getLeadingComment(path)).?;
    defer testing.allocator.free(got);
    try testing.expectEqualStrings("the answer", got);
    try ed.deleteLeadingComments(path);
    try testing.expect(std.mem.indexOf(u8, ed.source.items, "<!--") == null);
}

test "trailing comment set / get on a value element" {
    var ed: PlistEditor = .{ .allocator = testing.allocator, .format = .XML };
    try ed.init(wrapper ++ "\n<dict>\n  <key>a</key>\n  <integer>1</integer>\n</dict>\n</plist>\n");
    defer ed.deinit();
    const path = &[_]AST.PathSegment{.{ .key = "a" }};
    try ed.setTrailingComment(path, "note");
    try testing.expect(std.mem.indexOf(u8, ed.source.items, "<integer>1</integer> <!-- note -->") != null);
    const got = (try ed.getTrailingComment(path)).?;
    defer testing.allocator.free(got);
    try testing.expectEqualStrings("note", got);
}
