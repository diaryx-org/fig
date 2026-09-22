//! plist-specific editing helpers for `Editor(Plist)`.
//!
//! The generic span-splice engine lives in `../../editor.zig`; this module holds
//! the plist-only logic it delegates to. plist needs far MORE of its own logic
//! than TOML/YAML/fig/INI, because it is the one editable format that isn't
//! line-oriented `key<sep>value`: it's XML, so a dict entry is a *pair of
//! sibling elements* — `<key>name</key>` then a typed value element
//! (`<string>fig</string>`) — on two separate lines, and a value has no bare
//! literal spelling at all. That breaks every assumption the generic block-map
//! helpers make (`kv_sep`, one-line entries, splice-a-literal-value), so the
//! two string renderers below say how plist spells them, and the generic
//! engine does the rest:
//!
//!   - `renderValue`: the crux. A CLI value string (`fig set app.plist k=42`)
//!     has no plist meaning until it's wrapped in a typed element. We reuse the
//!     `.fig` dialect's own literal-else-string classifier — the engine's
//!     `literalOf`, handed in as `Literal` — to pick
//!     the type — `true`/`false` → `<true/>`/`<false/>`, integer → `<integer>`,
//!     float → `<real>`, any datetime shape → `<date>`, everything else →
//!     `<string>` (XML-escaped). `null` has no plist type (`NullUnsupported`).
//!     A value that already starts with `<` is spliced VERBATIM — the escape
//!     hatch for `<data>` (which can't be sniffed from bare text), an explicit
//!     `<date>`, a nested `<dict>`/`<array>`, or forcing a type (`<string>2.0`).
//!   - `renderEntry`: a dict entry is two lines, `<key>k</key>` then the
//!     value element at the same indent.
//!
//! Replace, insert, append and prepend are the engine's: it renders the value
//! through `renderValue`, writes an entry through `renderEntry` or an item as
//! the bare element (`seq_item_marker = ""`), and expands an empty `<dict/>`
//! or `<array/>` from `Syntax.closed_containers`.
//!
//! Delete-key, remove-seq-item and every comment op are generic too.
//! Once `comments.style` is `.xml_comment` (see `editor.zig`), the generic
//! line-based delete already does the right thing — a plist entry/item
//! occupies whole lines, and the keyvalue's full-extent span (recorded by
//! `parser.zig`) covers both the key and value lines, so
//! `lineStartBefore(span.start)`→`lineEndAfter(span.end)` removes the entry
//! cleanly, owned `<!-- -->` block and all. And `<!-- … -->` is declared as a
//! `CommentDelimiter` pair in `plist.zig`, so the generic leading and
//! trailing comment ops write and strip it themselves. No plist branch
//! needed for any of them.

const std = @import("std");
const testing = std.testing;

const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const Span = @import("../../util/span.zig");
const editor = @import("../../editor.zig");
const splice = @import("../../editor/splice.zig");
const Plist = @import("plist.zig").Language;
// The `.fig` dialect's bare-token classifier — reused so plist value typing
// obeys the exact same literal-else-string rules the fig language documents
// (Norway-safe booleans, leading-zero-stays-string, datetime sniffing). Pure
// functions on caller-owned slices; compiles regardless of `-Dfig`.
const lang = @import("../manifest.zig");

/// The concrete editor these ops drive — the plist arm of the generic engine.
const PlistEditor = editor.Editor(Plist);

// ── value rendering ────────────────────────────────────────────────────────────

/// Render a CLI value string into a plist typed element, appended to `out`.
/// See the module header for the typing rules and the `<`-prefix escape hatch.
pub fn renderValue(_: Plist.Type, allocator: std.mem.Allocator, out: *std.ArrayList(u8), value_text: []const u8, literal: lang.Literal) !void {
    const t = std.mem.trim(u8, value_text, " \t\r\n");
    if (t.len > 0 and t[0] == '<') {
        // Explicit element (or element tree): the caller has spelled the plist
        // syntax themselves — splice it as-is. Reparse validates it; a bogus
        // `<foo` rolls back via `replaceAtSpan`.
        try out.appendSlice(allocator, t);
        return;
    }
    switch (literal) {
        .null => return error.NullUnsupported, // plist has no null primitive
        .bool => try out.appendSlice(allocator, if (std.mem.eql(u8, t, "true")) "<true/>" else "<false/>"),
        .int => try wrapText(allocator, out, "integer", t, false),
        .float => try wrapText(allocator, out, "real", t, false),
        .datetime => try wrapText(allocator, out, "date", t, false),
        .string => try wrapText(allocator, out, "string", t, true),
    }
}

/// `<tag>text</tag>`, XML-escaping `text` when `escape` (PCDATA content, i.e.
/// strings/keys — numbers and dates are already lexically safe).
fn wrapText(allocator: std.mem.Allocator, out: *std.ArrayList(u8), tag: []const u8, text: []const u8, escape: bool) !void {
    try out.append(allocator, '<');
    try out.appendSlice(allocator, tag);
    try out.append(allocator, '>');
    if (escape) try appendEscaped(allocator, out, text) else try out.appendSlice(allocator, text);
    try out.appendSlice(allocator, "</");
    try out.appendSlice(allocator, tag);
    try out.append(allocator, '>');
}

/// Escape `&`/`<`/`>` in PCDATA content (mirrors the printer's `writeEscaped`).
fn appendEscaped(allocator: std.mem.Allocator, out: *std.ArrayList(u8), s: []const u8) !void {
    for (s) |c| switch (c) {
        '&' => try out.appendSlice(allocator, "&amp;"),
        '<' => try out.appendSlice(allocator, "&lt;"),
        '>' => try out.appendSlice(allocator, "&gt;"),
        else => try out.append(allocator, c),
    };
}

// ── entry rendering ────────────────────────────────────────────────────────────

/// `<key>key</key>\n<indent><value…>` — a dict entry's two lines, the key
/// and value at the same indent (matching the printer's layout). The first
/// line's indent is the engine's; `rendered_value` has been through
/// `renderValue`, and a container's further lines (a `<dict>` fragment's
/// entries and close tag) are moved under `indent` as well. See
/// `editor.Editor.writeEntry`.
pub fn renderEntry(_: Plist.Type, allocator: std.mem.Allocator, out: *std.ArrayList(u8), indent: []const u8, key: []const u8, rendered_value: []const u8) !void {
    try out.appendSlice(allocator, "<key>");
    try appendEscaped(allocator, out, key);
    try out.appendSlice(allocator, "</key>\n");
    try out.appendSlice(allocator, indent);
    var lines = std.mem.splitScalar(u8, rendered_value, '\n');
    try out.appendSlice(allocator, lines.first());
    while (lines.next()) |line| {
        try out.append(allocator, '\n');
        if (line.len > 0) try out.appendSlice(allocator, indent);
        try out.appendSlice(allocator, line);
    }
}

/// A renamed key: `<key>new_key</key>`, escaped. A key's span is the whole
/// element, so a bare name spliced over it would drop the tags. See
/// `editor.Editor.replaceKeyAtPath`.
pub fn renderKey(_: Plist.Type, allocator: std.mem.Allocator, out: *std.ArrayList(u8), indent: []const u8, new_key: []const u8, old_key: []const u8) !void {
    _ = indent;
    _ = old_key;
    try out.appendSlice(allocator, "<key>");
    try appendEscaped(allocator, out, new_key);
    try out.appendSlice(allocator, "</key>");
}

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
        try renderValue(.XML, testing.allocator, &out, c.in, editor.literalOf(c.in));
        try testing.expectEqualStrings(c.out, out.items);
    }
}

test "renderValue: null has no plist type" {
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(testing.allocator);
    try testing.expectError(error.NullUnsupported, renderValue(.XML, testing.allocator, &out, "null", .null));
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

test "insertKey refuses a dict that closes on its last entry's line" {
    // Appending after the line would land the entry in the outer dict.
    var ed: PlistEditor = .{ .allocator = testing.allocator, .format = .XML };
    try ed.init("<dict>\n\t<key>o</key><dict><key>a</key><string>x</string></dict>\n</dict>\n");
    defer ed.deinit();
    try testing.expectError(error.ContainerClosesOnItsLine, ed.insertKey(&[_]AST.PathSegment{.{ .key = "o" }}, "b", "y"));
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
