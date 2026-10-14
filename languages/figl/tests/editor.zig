//! The fig editor tests: the generic engine (`fig.editor`) driven
//! over this language, with its renderers (`edit.zig`).

const std = @import("std");
const AST = @import("kit").AST;
const Document = @import("kit").Document;
const Span = @import("kit").Span;
const editor = @import("fig").editor;
const lang = @import("kit").manifest;
const splice = @import("fig").editor.splice;
const Fig = @import("fig_figl").Language;
const Printer = @import("fig_figl").Printer;
const Writer = std.Io.Writer;
const log = std.log.scoped(.editor);
const FigEditor = editor.Editor(Fig);
const lineStartBefore = splice.lineStartBefore;
const lineEndAfter = splice.lineEndAfter;
const firstNonSpace = splice.firstNonSpace;
const isFlow = splice.isFlow;
const edit = @import("fig_figl").edit;
const blockBody = edit.blockBody;
const renderTail = edit.renderTail;

// =======
// TESTS
// =======
//
// fig editor tests live here (rather than in editor.zig) so each language's
// editing tests sit next to that language's helpers, mirroring
// `toml/tests/editor.zig`.

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

/// Set the trailing comment at `path` in `input` and expect `expected`; then
/// read it back as `text`, delete it, and expect `input` again.
fn expectTrailingRoundTrip(input: []const u8, path: []const AST.PathSegment, text: []const u8, expected: []const u8) !void {
    var ed = try newFigEditor(input);
    defer ed.deinit();
    try ed.setTrailingComment(path, text);
    try expectFigSource(&ed, expected);
    const got = (try ed.getTrailingComment(path)).?;
    defer std.testing.allocator.free(got);
    try std.testing.expectEqualStrings(text, got);
    try ed.deleteTrailingComment(path);
    try expectFigSource(&ed, input);
    try std.testing.expectEqual(null, try ed.getTrailingComment(path));
}

// A multi-line string's comment lives on its opener line (spec § 4.6). The
// window began after the closing quotes, so get missed `x = ''' # op` and set
// wrote a second comment after the close.
test "fig a multi-line string's trailing comment lives on its opener line" {
    const x: []const AST.PathSegment = &.{.{ .key = "x" }};
    const el: []const AST.PathSegment = &.{ .{ .key = "xs" }, .{ .index = 0 } };
    for ([_][]const u8{ "'''", "\"\"\"" }) |q| {
        var in_buf: [128]u8 = undefined;
        var out_buf: [128]u8 = undefined;
        // A map entry's value, LF and CRLF, and a sequence element.
        try expectTrailingRoundTrip(
            try std.fmt.bufPrint(&in_buf, "x = {s}\ntext\n{s}\ny = 1\n", .{ q, q }),
            x,
            "note",
            try std.fmt.bufPrint(&out_buf, "x = {s} # note\ntext\n{s}\ny = 1\n", .{ q, q }),
        );
        try expectTrailingRoundTrip(
            try std.fmt.bufPrint(&in_buf, "x = {s}\r\ntext\r\n{s}\r\ny = 1\r\n", .{ q, q }),
            x,
            "note",
            try std.fmt.bufPrint(&out_buf, "x = {s} # note\r\ntext\r\n{s}\r\ny = 1\r\n", .{ q, q }),
        );
        try expectTrailingRoundTrip(
            try std.fmt.bufPrint(&in_buf, "xs\n> * {s}\ntext\n{s}\n> * z\n", .{ q, q }),
            el,
            "note",
            try std.fmt.bufPrint(&out_buf, "xs\n> * {s} # note\ntext\n{s}\n> * z\n", .{ q, q }),
        );

        // An opener comment is read, replaced in place, and deleted there.
        {
            var ed = try newFigEditor(try std.fmt.bufPrint(&in_buf, "x = {s}  # op\r\ntext\r\n{s}\r\n", .{ q, q }));
            defer ed.deinit();
            const got = (try ed.getTrailingComment(x)).?;
            defer std.testing.allocator.free(got);
            try std.testing.expectEqualStrings("op", got);
            try ed.setTrailingComment(x, "new");
            try expectFigSource(&ed, try std.fmt.bufPrint(&out_buf, "x = {s} # new\r\ntext\r\n{s}\r\n", .{ q, q }));
            try ed.deleteTrailingComment(x);
            try expectFigSource(&ed, try std.fmt.bufPrint(&out_buf, "x = {s}\r\ntext\r\n{s}\r\n", .{ q, q }));
        }

        // A comment after the closing quotes is not the string's: get does
        // not report it, delete leaves it, and set writes beside the opener.
        {
            const input = try std.fmt.bufPrint(&in_buf, "x = {s}\ntext\n{s} # close\n", .{ q, q });
            var ed = try newFigEditor(input);
            defer ed.deinit();
            try std.testing.expectEqual(null, try ed.getTrailingComment(x));
            try ed.deleteTrailingComment(x);
            try expectFigSource(&ed, input);
            try ed.setTrailingComment(x, "op");
            try expectFigSource(&ed, try std.fmt.bufPrint(&out_buf, "x = {s} # op\ntext\n{s} # close\n", .{ q, q }));
        }
    }
    // An empty multi-line string still opens a line of its own; a quoted
    // string on one line keeps its comment after the close.
    try expectTrailingRoundTrip("x = '''\n'''\n", x, "note", "x = ''' # note\n'''\n");
    try expectTrailingRoundTrip("x = 'a'\n", x, "note", "x = 'a' # note\n");
}

// A flow list or object over several lines takes its comment on its opener
// line, as the parser reads it (`xs = [ # op`). The window began after the
// close, so get missed `# op` and set wrote a second comment after the `]`.
test "fig a multi-line flow list's or object's trailing comment lives on its opener line" {
    const xs: []const AST.PathSegment = &.{.{ .key = "xs" }};
    const el: []const AST.PathSegment = &.{ .{ .key = "ys" }, .{ .index = 0 } };
    const Shape = struct { open: []const u8, lf: []const u8, crlf: []const u8, close: []const u8 };
    for ([_]Shape{
        .{ .open = "[", .lf = "  1,\n  2,\n", .crlf = "  1,\r\n  2,\r\n", .close = "]" },
        .{ .open = "{", .lf = "  a = 1,\n  b = 2,\n", .crlf = "  a = 1,\r\n  b = 2,\r\n", .close = "}" },
    }) |sh| {
        var in_buf: [256]u8 = undefined;
        var out_buf: [256]u8 = undefined;
        // A map entry's value, LF and CRLF, and a sequence element.
        try expectTrailingRoundTrip(
            try std.fmt.bufPrint(&in_buf, "xs = {s}\n{s}{s}\ny = 1\n", .{ sh.open, sh.lf, sh.close }),
            xs,
            "note",
            try std.fmt.bufPrint(&out_buf, "xs = {s} # note\n{s}{s}\ny = 1\n", .{ sh.open, sh.lf, sh.close }),
        );
        try expectTrailingRoundTrip(
            try std.fmt.bufPrint(&in_buf, "xs = {s}\r\n{s}{s}\r\ny = 1\r\n", .{ sh.open, sh.crlf, sh.close }),
            xs,
            "note",
            try std.fmt.bufPrint(&out_buf, "xs = {s} # note\r\n{s}{s}\r\ny = 1\r\n", .{ sh.open, sh.crlf, sh.close }),
        );
        try expectTrailingRoundTrip(
            try std.fmt.bufPrint(&in_buf, "ys\n> * {s}\n{s}{s}\n> * z\n", .{ sh.open, sh.lf, sh.close }),
            el,
            "note",
            try std.fmt.bufPrint(&out_buf, "ys\n> * {s} # note\n{s}{s}\n> * z\n", .{ sh.open, sh.lf, sh.close }),
        );

        // An opener comment is read, replaced in place, and deleted there.
        {
            var ed = try newFigEditor(try std.fmt.bufPrint(&in_buf, "xs = {s}  # op\r\n{s}{s}\r\n", .{ sh.open, sh.crlf, sh.close }));
            defer ed.deinit();
            const got = (try ed.getTrailingComment(xs)).?;
            defer std.testing.allocator.free(got);
            try std.testing.expectEqualStrings("op", got);
            try ed.setTrailingComment(xs, "new");
            try expectFigSource(&ed, try std.fmt.bufPrint(&out_buf, "xs = {s} # new\r\n{s}{s}\r\n", .{ sh.open, sh.crlf, sh.close }));
            try ed.deleteTrailingComment(xs);
            try expectFigSource(&ed, try std.fmt.bufPrint(&out_buf, "xs = {s}\r\n{s}{s}\r\n", .{ sh.open, sh.crlf, sh.close }));
        }

        // A comment after the close is not the container's when its opener
        // line is the comment's: get does not report it, delete leaves it,
        // and set writes beside the opener.
        {
            const input = try std.fmt.bufPrint(&in_buf, "xs = {s}\n{s}{s} # close\n", .{ sh.open, sh.lf, sh.close });
            var ed = try newFigEditor(input);
            defer ed.deinit();
            try std.testing.expectEqual(null, try ed.getTrailingComment(xs));
            try ed.deleteTrailingComment(xs);
            try expectFigSource(&ed, input);
            try ed.setTrailingComment(xs, "op");
            try expectFigSource(&ed, try std.fmt.bufPrint(&out_buf, "xs = {s} # op\n{s}{s} # close\n", .{ sh.open, sh.lf, sh.close }));
        }
    }
    // A flow container on one line keeps its comment after the close, and
    // so does one whose opener line holds an element: a `#` there is inside
    // the flow, which the parser skips.
    try expectTrailingRoundTrip("xs = [1, 2]\n", xs, "note", "xs = [1, 2] # note\n");
    try expectTrailingRoundTrip("xs = {a=1}\n", xs, "note", "xs = {a=1} # note\n");
    try expectTrailingRoundTrip("xs = [1,\n  2,\n]\n", xs, "note", "xs = [1,\n  2,\n] # note\n");
    {
        var ed = try newFigEditor("xs = [1, # inner\n  2,\n]\n");
        defer ed.deinit();
        try std.testing.expectEqual(null, try ed.getTrailingComment(xs));
    }
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
    try expectFigSource(&ed, "p = { x = 1 y = 2 }\n");
}

// A compact object (`{a=1 b=2}`) stays in the form `fig fmt` writes: pairs
// parted by a space, no spaces around `=`, no padding inside the braces.
test "fig insert key into a compact flow mapping writes no comma" {
    var ed = try newFigEditor("p = {a=1}\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "p" }}, "b", "2");
    try expectFigSource(&ed, "p = {a=1 b=2}\n");
    try ed.insertKey(&.{.{ .key = "p" }}, "c", "3");
    try expectFigSource(&ed, "p = {a=1 b=2 c=3}\n");
}

test "fig insert key into an empty flow mapping writes it compact" {
    var ed = try newFigEditor("p = {}\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "p" }}, "x", "1");
    try expectFigSource(&ed, "p = {x=1}\n");
    try ed.insertKey(&.{.{ .key = "p" }}, "y", "2");
    try expectFigSource(&ed, "p = {x=1 y=2}\n");
}

test "fig insert key into a comma-joined fig-inline flow mapping keeps its commas" {
    var ed = try newFigEditor("p = {a=1, b=2}\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "p" }}, "c", "3");
    try expectFigSource(&ed, "p = {a=1, b=2, c=3}\n");
}

test "fig insert key into a multi-line compact flow mapping writes no comma" {
    var ed = try newFigEditor("p = {\n  a=1\n}\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "p" }}, "b", "2");
    try expectFigSource(&ed, "p = {\n  a=1\n  b=2\n}\n");
}

test "fig insert key into a JSON-mode flow mapping matches its colon separator" {
    var ed = try newFigEditor("p = { \"x\": 1 }\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "p" }}, "\"y\"", "2");
    try expectFigSource(&ed, "p = { \"x\": 1, \"y\": 2 }\n");
}

test "fig insert key into a two-member JSON-mode flow mapping keeps its commas" {
    var ed = try newFigEditor("p = {\"x\": 1, \"y\": 2}\n");
    defer ed.deinit();
    try ed.insertKey(&.{.{ .key = "p" }}, "\"z\"", "3");
    try expectFigSource(&ed, "p = {\"x\": 1, \"y\": 2, \"z\": 3}\n");
}

test "fig insert key into a dotted object in a flow mapping writes its key path inside the braces" {
    var ed = try newFigEditor("p = {a.b=1 c=2 a.d=3}\n");
    defer ed.deinit();
    try ed.set(&.{ .{ .key = "p" }, .{ .key = "a" }, .{ .key = "e" } }, "5");
    try expectFigSource(&ed, "p = {a.b=1 c=2 a.d=3 a.e=5}\n");
    try ed.set(&.{ .{ .key = "p" }, .{ .key = "f" } }, "6");
    try expectFigSource(&ed, "p = {a.b=1 c=2 a.d=3 a.e=5 f=6}\n");
    // A dotted first entry: the separators are its leaf's.
    var comma = try newFigEditor("p = {a.b=1, c=2}\n");
    defer comma.deinit();
    try comma.set(&.{ .{ .key = "p" }, .{ .key = "e" } }, "5");
    try expectFigSource(&comma, "p = {a.b=1, c=2, e=5}\n");
    var deep = try newFigEditor("p = {c=1 a.\"x y\".c=1}\n");
    defer deep.deinit();
    try deep.set(&.{ .{ .key = "p" }, .{ .key = "a" }, .{ .key = "x y" }, .{ .key = "e" } }, "5");
    try expectFigSource(&deep, "p = {c=1 a.\"x y\".c=1 a.\"x y\".e=5}\n");
}

test "fig insert key into a dotted object among block lines writes its key path" {
    var ed = try newFigEditor("s\n> a.b = 1\n> z = 2\n");
    defer ed.deinit();
    try ed.set(&.{ .{ .key = "s" }, .{ .key = "a" }, .{ .key = "y" } }, "1");
    try expectFigSource(&ed, "s\n> a.b = 1\n> a.y = 1\n> z = 2\n");
}

test "fig insert into a flow array still writes commas" {
    var ed = try newFigEditor("p = {a=[1]}\n");
    defer ed.deinit();
    try ed.appendToSeq(&.{ .{ .key = "p" }, .{ .key = "a" } }, "2");
    try expectFigSource(&ed, "p = {a=[1, 2]}\n");
}

test "fig insert duplicate key rolls back" {
    var ed = try newFigEditor("a = 1\n");
    defer ed.deinit();
    try std.testing.expectError(error.DuplicateKey, ed.insertKey(&.{}, "a", "2"));
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

// A dotted line is all there is of the maps it names: deleting its key takes
// them too, as the delete guard allows (`impliedOnlyBy` in editor.zig).
test "fig delete a dotted key takes the maps only it made" {
    var ed = try newFigEditor("a.b.c = 1\ndeep.nested.value = 42 # c\n'q k'.sub = 1\nv = 2\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "q k" }, .{ .key = "sub" } });
    try expectFigSource(&ed, "a.b.c = 1\ndeep.nested.value = 42 # c\nv = 2\n");
    try ed.deleteKey(&.{ .{ .key = "deep" }, .{ .key = "nested" }, .{ .key = "value" } });
    try expectFigSource(&ed, "a.b.c = 1\nv = 2\n");
}

// `ys[0]` is written in two places, with `ys[1]` between them: the delete
// takes both, and not the lines between, and `ys[1]` is renumbered, an
// index being a position (spec § 7.4).
test "fig delete an element written in two places takes both, and renumbers the next" {
    const source = "ys[0]\n> k = 1\nys[1]  # one\n> k = 2\nys[0]\n> j = 3\n";
    var ed = try newFigEditor(source);
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "ys" }}, 0);
    try expectFigSource(&ed, "ys[0]  # one\n> k = 2\n");

    var whole = try newFigEditor(source);
    defer whole.deinit();
    try whole.deleteContainer(&.{ .{ .key = "ys" }, .{ .index = 0 } });
    try expectFigSource(&whole, "ys[0]  # one\n> k = 2\n");
}

test "fig delete an index-addressed element renumbers every line after it" {
    var ed = try newFigEditor("xs[0] = a\nxs[1] = b  # b\nxs[2] = c\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "xs" }}, 0);
    try expectFigSource(&ed, "xs[0] = b  # b\nxs[1] = c\n");
    try ed.removeSeqItem(&.{.{ .key = "xs" }}, 0);
    try expectFigSource(&ed, "xs[0] = c\n");

    // Dotted lines through the index, and an index deeper in the path,
    // which is another sequence's and stays.
    var dotted = try newFigEditor("c[0].name = x\nc[1].name = y\nc[0].size = 1\nc[1].p[0] = 2\nc[2].name = z\n");
    defer dotted.deinit();
    try dotted.deleteContainer(&.{ .{ .key = "c" }, .{ .index = 0 } });
    try expectFigSource(&dotted, "c[0].name = y\nc[0].p[0] = 2\nc[1].name = z\n");
}

test "fig delete the middle index-addressed element leaves the first alone" {
    var ed = try newFigEditor("ys[0]\n> k = 1\nys[1]\n> k = 2\nys[2]\n> k = 3\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "ys" }}, 1);
    try expectFigSource(&ed, "ys[0]\n> k = 1\nys[1]\n> k = 3\n");
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

test "fig delete key inside a compact flow mapping leaves it compact" {
    var ed = try newFigEditor("p = {a=1 b=2 c=3}\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "p" }, .{ .key = "b" } });
    try expectFigSource(&ed, "p = {a=1 c=3}\n");
    try ed.deleteKey(&.{ .{ .key = "p" }, .{ .key = "a" } });
    try expectFigSource(&ed, "p = {c=3}\n");
    try ed.deleteKey(&.{ .{ .key = "p" }, .{ .key = "c" } });
    try expectFigSource(&ed, "p = {}\n");
}

test "fig delete key inside a JSON-mode flow mapping keeps its commas" {
    var ed = try newFigEditor("p = {\"x\": 1, \"y\": 2, \"z\": 3}\n");
    defer ed.deinit();
    try ed.deleteKey(&.{ .{ .key = "p" }, .{ .key = "y" } });
    try expectFigSource(&ed, "p = {\"x\": 1, \"z\": 3}\n");
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
    try std.testing.expectError(error.CannotDeleteSection, ed.deleteKey(&.{ .{ .key = "database" }, .{ .key = "pool" } }));
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

// Deleting a list's only item leaves `key = []`, as a flow list and a YAML
// block list are left, keeping the key's comments; the key used to go with
// the item's lines.

test "fig remove the only `*` item leaves the key and an empty list" {
    var ed = try newFigEditor("a = 1\n# lead\nports  # tail\n> * 25565\nb = 2\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "ports" }}, 0);
    try expectFigSource(&ed, "a = 1\n# lead\nports = []  # tail\nb = 2\n");
    try ed.appendToSeq(&.{.{ .key = "ports" }}, "80");
    try expectFigSource(&ed, "a = 1\n# lead\nports = [80]  # tail\nb = 2\n");
}

test "fig remove the only map element under a header leaves an empty list" {
    var ed = try newFigEditor("servers\n> *  # first\n> > host = a\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "servers" }}, 0);
    try expectFigSource(&ed, "servers = []\n");
}

test "fig remove the only append-header element keeps its key's line and comments" {
    var ed = try newFigEditor("a = 1\n# lead\nreplacements[]  # tail\n> file = README.md\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "replacements" }}, 0);
    try expectFigSource(&ed, "a = 1\n# lead\nreplacements = []  # tail\n");
}

test "fig delete the only append-header element as a container leaves an empty list" {
    // The CLI's `delete xs[0]` takes a block element whole, through
    // `deleteContainer`.
    var ed = try newFigEditor("a = 1\nreplacements[]\n> file = README.md\n> exactly = 0\n");
    defer ed.deinit();
    try ed.deleteContainer(&.{ .{ .key = "replacements" }, .{ .index = 0 } });
    try expectFigSource(&ed, "a = 1\nreplacements = []\n");
}

test "fig remove the only element of a nested append header keeps the path to it" {
    var ed = try newFigEditor("a[]\n> x = 1\na[].b[]\n> y = 2\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{ .{ .key = "a" }, .{ .index = 0 }, .{ .key = "b" } }, 0);
    try expectFigSource(&ed, "a[]\n> x = 1\na[].b = []\n");

    var nested = try newFigEditor("a[]\n> x = 1\n> b[]\n> > y = 2\n");
    defer nested.deinit();
    try nested.deleteContainer(&.{ .{ .key = "a" }, .{ .index = 0 }, .{ .key = "b" }, .{ .index = 0 } });
    try expectFigSource(&nested, "a[]\n> x = 1\n> b = []\n");
}

test "fig emptying a `+` chain one element at a time ends at an empty list" {
    var ed = try newFigEditor("r[]\n> f = 1\n+\n> f = 2\nb = 2\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "r" }}, 1);
    try expectFigSource(&ed, "r[]\n> f = 1\nb = 2\n");
    try ed.removeSeqItem(&.{.{ .key = "r" }}, 0);
    try expectFigSource(&ed, "r = []\nb = 2\n");
}

// Deleting the first element of a `+` chain takes its `r[]` line, and the
// `+` after it, which re-ran that header, takes its text (spec § 8).
// Each element keeps its own comments.
test "fig delete the first element of a two-element `+` chain moves the header onto the next" {
    const source = "# r\nr[]  # first\n> file = a\n# b's\n+  # second\n> file = b\nafter = 1\n";
    var ed = try newFigEditor(source);
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "r" }}, 0);
    try expectFigSource(&ed, "# b's\nr[]  # second\n> file = b\nafter = 1\n");

    var whole = try newFigEditor(source);
    defer whole.deinit();
    try whole.deleteContainer(&.{ .{ .key = "r" }, .{ .index = 0 } });
    try expectFigSource(&whole, "# b's\nr[]  # second\n> file = b\nafter = 1\n");

    // A comment set in a column stays in it.
    var aligned = try newFigEditor("rows[]    # a\n> f = 1\n+         # b\n> f = 2\n");
    defer aligned.deinit();
    try aligned.removeSeqItem(&.{.{ .key = "rows" }}, 0);
    try expectFigSource(&aligned, "rows[]    # b\n> f = 2\n");
}

test "fig delete from a three-element `+` chain" {
    const source = "r[]\n> f = 1\n+\n> f = 2\n+\n> f = 3\n";
    var first = try newFigEditor(source);
    defer first.deinit();
    try first.removeSeqItem(&.{.{ .key = "r" }}, 0);
    try expectFigSource(&first, "r[]\n> f = 2\n+\n> f = 3\n");
    try first.removeSeqItem(&.{.{ .key = "r" }}, 0);
    try expectFigSource(&first, "r[]\n> f = 3\n");

    var middle = try newFigEditor(source);
    defer middle.deinit();
    try middle.removeSeqItem(&.{.{ .key = "r" }}, 1);
    try expectFigSource(&middle, "r[]\n> f = 1\n+\n> f = 3\n");
}

// A `+` under a nested header re-runs the whole path; with the header gone
// it would re-run `spec.containers[]` and append a container.
test "fig delete the first element of a nested `+` chain keeps the next in its container" {
    const source = "spec.containers[]\n> name = app\nspec.containers[].ports[]  # web\n> port = 80\n+  # tls\n> port = 443\nspec.containers[]\n> name = sidecar\n";
    var ed = try newFigEditor(source);
    defer ed.deinit();
    try ed.removeSeqItem(&.{ .{ .key = "spec" }, .{ .key = "containers" }, .{ .index = 0 }, .{ .key = "ports" } }, 0);
    try expectFigSource(&ed, "spec.containers[]\n> name = app\nspec.containers[].ports[]  # tls\n> port = 443\nspec.containers[]\n> name = sidecar\n");

    var whole = try newFigEditor(source);
    defer whole.deinit();
    try whole.deleteContainer(&.{ .{ .key = "spec" }, .{ .key = "containers" }, .{ .index = 0 }, .{ .key = "ports" }, .{ .index = 0 } });
    try expectFigSource(&whole, "spec.containers[]\n> name = app\nspec.containers[].ports[]  # tls\n> port = 443\nspec.containers[]\n> name = sidecar\n");
}

// The move took `c[].p[]`'s lines past the `+` that re-runs it, and the `+`
// would read as a second `c` element: the move guard (`readsBack`) refuses
// (docs/tasks/figl-move-chains-and-indices.md).
test "fig moving the first element of a nested `+` chain is refused, not a new container" {
    const source = "c[]\n> n = a\nc[].p[]\n> x = 1\n+\n> x = 2\n";
    var ed = try newFigEditor(source);
    defer ed.deinit();
    try std.testing.expectError(error.EditReadsBackWrong, ed.moveItem(&.{ .{ .key = "c" }, .{ .index = 0 }, .{ .key = "p" } }, 0, 1));
    try expectFigSource(&ed, source);
}

test "fig an element's index and header are not names a rename rewrites" {
    var ed = try newFigEditor("ys[0]\n> k = 1\n");
    defer ed.deinit();
    try std.testing.expectError(error.NotASection, ed.renameContainer(&.{ .{ .key = "ys" }, .{ .index = 0 } }, "z"));
    try expectFigSource(&ed, "ys[0]\n> k = 1\n");
}

test "fig remove the only index-addressed element takes every line of it" {
    // The comment ends an assignment to the element, so it goes with it.
    var ed = try newFigEditor("c[0].name = x  # the name\nother = 1\nc[0].size = 3\n");
    defer ed.deinit();
    try ed.removeSeqItem(&.{.{ .key = "c" }}, 0);
    try expectFigSource(&ed, "c = []\nother = 1\n");

    var scattered = try newFigEditor("a[]\n> x = 1\nb = 2\na[0].y = 3\n");
    defer scattered.deinit();
    try scattered.deleteContainer(&.{ .{ .key = "a" }, .{ .index = 0 } });
    try expectFigSource(&scattered, "a = []\nb = 2\n");
}

test "fig removing the only item of a root list is refused" {
    // figl reads an empty document as an empty mapping, and `[]` at a
    // line's start is TOML's header, not a list.
    var ed = try newFigEditor("# c\n* a\n");
    defer ed.deinit();
    try std.testing.expectError(error.NoEmptyRootList, ed.removeSeqItem(&.{}, 0));
    try expectFigSource(&ed, "# c\n* a\n");
    try std.testing.expect(!ed.splice_rejected);

    var map = try newFigEditor("*\n> a = 1\n");
    defer map.deinit();
    try std.testing.expectError(error.NoEmptyRootList, map.deleteContainer(&.{.{ .index = 0 }}));
    try expectFigSource(&map, "*\n> a = 1\n");
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
    try std.testing.expectError(error.NotASection, ed.deleteContainer(&.{.{ .key = "x" }}));
    try expectFigSource(&ed, "x = 1\n");
}

test "fig deleteContainer on a flow-valued key is refused" {
    var ed = try newFigEditor("p = { x = 1 }\n");
    defer ed.deinit();
    try std.testing.expectError(error.NotASection, ed.deleteContainer(&.{.{ .key = "p" }}));
    try expectFigSource(&ed, "p = { x = 1 }\n");
}

test "fig delete a verbatim re-entered header removes every occurrence" {
    // The exact same header (`database`, not a deeper dotted path) written
    // twice — the shape spans alone can't discover; found via the parser's
    // `Document.regionsOf` record (see the module doc comment). Both
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
    // re-entry, found via `Document.regionsOf`) move contiguously; the
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
    try std.testing.expectError(error.NotASection, ed.moveContainer(&.{.{ .key = "x" }}, null));
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
    try std.testing.expectError(error.NotASection, ed.reorderContainers(&.{ "x", "a" }));
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
    try std.testing.expectError(error.CannotMoveSection, ed.moveKey(&.{.{ .key = "a" }}, &.{.{ .key = "b" }}));
    try std.testing.expectError(error.CannotMoveSection, ed.moveKey(&.{.{ .key = "other" }}, &.{.{ .key = "b" }}));
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
    try std.testing.expectError(error.CannotReorderSections, ed.reorderKeys(&.{}, &.{ "b", "a" }));
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
