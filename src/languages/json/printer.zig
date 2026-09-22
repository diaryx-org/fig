const Printer = @This();
const std = @import("std");
const AST = @import("../../ast/ast.zig");
const json_string = @import("../../util/json_string.zig");
const num = @import("../../util/number.zig");
const Writer = std.Io.Writer;

/// `UnresolvedAlias`: JSON cannot represent a YAML alias. A materialized AST
/// contains none (aliases are expanded to copied subtrees by
/// `yaml.materialize`), so reaching one here means an unmaterialized YAML AST
/// was handed to the JSON printer.
///
/// `NonStringKey`: a sequence or mapping was used as an object key. See `key`
/// for what the printer does with the other key kinds and why.
pub const Error = Writer.Error || error{ UnresolvedAlias, NonStringKey };

writer: *Writer,
ast: *const AST,
options: AST.SerializeOptions,
/// Which dialect to emit. JSON5 differs from JSON in exactly two places:
/// object keys print unquoted when they are bare identifiers, and the
/// non-finite `number_special` scalars (`Infinity`/`NaN`) print verbatim
/// rather than degrading to quoted strings. JSONC is plain JSON syntax (quoted
/// keys, normalized numbers) but, like JSON5, carries `//` and `/* */` comments.
dialect: Dialect = .json,

pub const Dialect = enum { json, jsonc, json5 };

/// Prints a given document in JSON format.
pub fn print(writer: *Writer, ast: *const AST, options: AST.SerializeOptions) Error!void {
    var p: Printer = .{ .writer = writer, .ast = ast, .options = options };
    try p.node(ast.root, 0);
    try writer.writeByte('\n');
    try writer.flush();
}

/// Prints the subtree rooted at `id`. Used for partial renders; unlike `print`
/// it adds no trailing newline and does not flush.
pub fn printNode(writer: *Writer, ast: *const AST, id: AST.Node.Id, depth: usize, options: AST.SerializeOptions) Error!void {
    var p: Printer = .{ .writer = writer, .ast = ast, .options = options };
    try p.node(id, depth);
}

/// `print`, emitting JSON5: unquoted bare-identifier keys and verbatim
/// `Infinity`/`NaN`.
pub fn print5(writer: *Writer, ast: *const AST, options: AST.SerializeOptions) Error!void {
    var p: Printer = .{ .writer = writer, .ast = ast, .options = options, .dialect = .json5 };
    try p.leadingComments(ast.leadingCommentAnchor(ast.root), 0);
    try p.node(ast.root, 0);
    try p.rootTrailing();
    try writer.writeByte('\n');
    try writer.flush();
}

/// `printNode`, emitting JSON5.
pub fn printNode5(writer: *Writer, ast: *const AST, id: AST.Node.Id, depth: usize, options: AST.SerializeOptions) Error!void {
    var p: Printer = .{ .writer = writer, .ast = ast, .options = options, .dialect = .json5 };
    try p.node(id, depth);
}

/// `print`, emitting JSONC: plain-JSON syntax with `//`/`/* */` comments.
pub fn printc(writer: *Writer, ast: *const AST, options: AST.SerializeOptions) Error!void {
    var p: Printer = .{ .writer = writer, .ast = ast, .options = options, .dialect = .jsonc };
    try p.leadingComments(ast.leadingCommentAnchor(ast.root), 0);
    try p.node(ast.root, 0);
    try p.rootTrailing();
    try writer.writeByte('\n');
    try writer.flush();
}

/// Emit the root's trailing comment — but only when the root is a scalar. A
/// container root already emitted its own trailing beside its opening delimiter.
fn rootTrailing(self: *Printer) Error!void {
    const anchor = self.ast.trailingCommentAnchor(self.ast.root);
    if (!self.isContainer(anchor)) try self.trailingComment(anchor);
}

/// `printNode`, emitting JSONC.
pub fn printNodec(writer: *Writer, ast: *const AST, id: AST.Node.Id, depth: usize, options: AST.SerializeOptions) Error!void {
    var p: Printer = .{ .writer = writer, .ast = ast, .options = options, .dialect = .jsonc };
    try p.node(id, depth);
}

fn node(self: *Printer, id: AST.Node.Id, depth: usize) Error!void {
    const n = self.ast.nodes[id];
    switch (n.kind) {
        .null_ => try self.writer.writeAll("null"),
        .boolean => |value| try self.writer.writeAll(if (value) "true" else "false"),
        .number => |value| try self.number(value.raw),
        // JSON has none of these types. Datetimes and enum literals render as
        // strings (the timestamp / the bare name); a char literal renders as its
        // codepoint number.
        .extended => |value| switch (value.kind) {
            .char_literal => try self.writer.writeAll(value.text),
            // JSON5 has native non-finite floats; JSON must degrade to a string.
            .number_special => if (self.dialect == .json5)
                try self.writer.writeAll(value.text)
            else
                try json_string.writeQuoted(self.writer, value.text),
            else => try json_string.writeQuoted(self.writer, value.text),
        },
        .string => |value| try json_string.writeQuoted(self.writer, value),
        .sequence => |first_child| try self.sequence(id, first_child, depth),
        .mapping => |first_child| try self.mapping(id, first_child, depth),
        .keyvalue => |kv| {
            try self.key(kv.key);
            // Compact output omits the space after the colon.
            try self.writer.writeAll(if (self.options.pretty) ": " else ":");
            try self.node(kv.value, depth);
        },
        .alias => return error.UnresolvedAlias,
    }
}

/// Render an object key.
///
/// A JSON object key is a string and nothing else, but the AST's key node can
/// hold any kind: YAML writes `null: a`, `23: x`, `true: x`, and even
/// `? [a, b] : c`. **The decision, for all three dialects: a non-string SCALAR
/// key is spelled as the JSON string of its source text — `"null"`, `"23"`,
/// `"true"` — and a key with no such spelling (a sequence or a mapping) is
/// refused with `NonStringKey`.** The alternative — refuse every non-string
/// key, as the fig/TOML/ZON/XML printers do — was weighed and rejected, because
/// `-o json` is a *conversion*: a scalar key has one obvious faithful reading,
/// and losing a whole document over `23:` would help nobody, while a collection
/// key has no reading at all. Until this existed the key node was
/// printed as a value, so those documents came out as bytes no JSON parser
/// accepts (docs/tasks/closed/json-printer-emits-non-string-keys.md).
///
/// Spelling can collide — a YAML document with both `null:` and `"null":`, or
/// with two null keys (2JQS's `: a` / `: b`), emits the same name twice. That is
/// still JSON: RFC 8259 permits a repeated name and every parser reads it, where
/// the bare `null:` this replaced was not parseable at all.
///
/// An `alias` key stays `UnresolvedAlias`, not `NonStringKey`: like an alias
/// *value* it means an unmaterialized YAML AST reached a non-YAML printer (the
/// CLI materializes first), not a key JSON has no room for.
///
/// Dialect: in JSON5 a *source* string key that is a bare ECMAScript identifier
/// prints unquoted (`foo: 1`); every spelled scalar key is quoted in every
/// dialect, so the spelling of a converted key does not depend on which JSON
/// came out.
fn key(self: *Printer, id: AST.Node.Id) Error!void {
    switch (self.ast.nodes[id].kind) {
        .string => |s| {
            if (self.dialect == .json5 and isBareIdentifier(s)) {
                try self.writer.writeAll(s);
                return;
            }
            try json_string.writeQuoted(self.writer, s);
        },
        .null_ => try json_string.writeQuoted(self.writer, "null"),
        .boolean => |value| try json_string.writeQuoted(self.writer, if (value) "true" else "false"),
        // The lexeme verbatim rather than the normalized number: the string is
        // the key's source text, not a number being re-spelled, so a YAML `23`
        // is `"23"` and a `0x1F` key stays `"0x1F"`.
        .number => |value| try json_string.writeQuoted(self.writer, value.raw),
        // Every extended scalar's `text` IS its value (a TOML timestamp, an enum
        // literal's name, a char literal's codepoint, an `Infinity` lexeme) — the
        // same bytes the value path prints, quoted because this is a key.
        .extended => |value| try json_string.writeQuoted(self.writer, value.text),
        .alias => return error.UnresolvedAlias,
        .sequence, .mapping, .keyvalue => return error.NonStringKey,
    }
}

/// The ASCII subset of an ECMAScript IdentifierName, matching what the JSON5
/// tokenizer accepts unquoted. Reserved words are intentionally allowed (JSON5
/// permits `while: 1`); only the lexical shape matters.
fn isBareIdentifier(name: []const u8) bool {
    if (name.len == 0) return false;
    for (name, 0..) |c, i| {
        const start_ok = (c >= 'a' and c <= 'z') or (c >= 'A' and c <= 'Z') or c == '_' or c == '$';
        const part_ok = start_ok or (c >= '0' and c <= '9');
        if (!(if (i == 0) start_ok else part_ok)) return false;
    }
    return true;
}

/// Render a number. JSON5 keeps the source lexeme where it can read it back —
/// hex, leading/trailing `.`, and leading `+` are all valid JSON5 — but NOT
/// `0o`/`0b`, `_`, or a leading zero, which its own tokenizer rejects; those
/// canonicalize, as does every non-decimal lexeme under plain JSON.
fn number(self: *Printer, raw: []const u8) Error!void {
    const spelling = if (self.dialect == .json5) num.json5 else num.json;
    try num.write(self.writer, raw, spelling);
}

fn sequence(self: *Printer, node_id: AST.Node.Id, first_child: ?AST.Node.Id, depth: usize) Error!void {
    try self.container(node_id, '[', ']', first_child, depth);
}

fn mapping(self: *Printer, node_id: AST.Node.Id, first_child: ?AST.Node.Id, depth: usize) Error!void {
    try self.container(node_id, '{', '}', first_child, depth);
}

/// Sequences and mappings differ only in their delimiters and in how each child
/// renders (a bare node vs. a `key: value`), the latter dispatched by `node`.
fn container(self: *Printer, node_id: AST.Node.Id, open: u8, close: u8, first_child: ?AST.Node.Id, depth: usize) Error!void {
    // Dangling comments (orphans at the end of the body) force the block form so
    // they have somewhere to print; only an empty-and-comment-free container is
    // inline. Comments only print in pretty JSON5/JSONC, so an empty container
    // with dangling comments in a comment-less mode still prints inline.
    const dangling = if (self.commentsOn()) self.ast.comments(node_id).dangling else &.{};
    if (first_child == null and dangling.len == 0) {
        try self.writer.writeByte(open);
        try self.writer.writeByte(close);
        try self.trailingComment(node_id); // empty inline container: `[] // c`
        return;
    }

    const pretty = self.options.pretty;
    try self.writer.writeByte(open);
    // A container's own trailing comment rides the line it opened on, so it sits
    // beside the `[`/`{` (next to its key), not after the distant close.
    try self.trailingComment(node_id);
    if (pretty) try self.writer.writeByte('\n');

    var current_id = first_child;
    while (current_id) |id| {
        try self.leadingComments(self.ast.leadingCommentAnchor(id), depth + 1);
        if (pretty) try self.writeIndent(depth + 1);
        try self.node(id, depth + 1);
        current_id = self.ast.nodes[id].next_sibling;
        if (current_id != null) try self.writer.writeByte(',');
        // A scalar child's trailing prints here; a container child emits its own
        // (beside its opener, above), so skip it to avoid a double / misplacement.
        const anchor = self.ast.trailingCommentAnchor(id);
        if (!self.isContainer(anchor)) try self.trailingComment(anchor);
        if (pretty) try self.writer.writeByte('\n');
    }
    try self.danglingComments(dangling, depth + 1);

    if (pretty) try self.writeIndent(depth);
    try self.writer.writeByte(close);
}

/// Whether `id` is a container node (whose own trailing comment is emitted beside
/// its opening delimiter, not by its parent).
fn isContainer(self: *const Printer, id: AST.Node.Id) bool {
    return switch (self.ast.nodes[id].kind) {
        .sequence, .mapping => true,
        else => false,
    };
}

// ── comments (JSON5 only) ───────────────────────────────────────────────────
// Plain JSON has no comment syntax, so comments are emitted only in the JSON5
// dialect and only when pretty-printing (a `//` line comment can't survive on a
// minified single line). Both predicates are checked in the helpers, so callers
// can invoke them unconditionally.

/// True when comments may be emitted: a comment-bearing dialect (JSON5 or JSONC)
/// and multi-line output (a `//` can't survive on a minified single line).
fn commentsOn(self: *const Printer) bool {
    return (self.dialect == .json5 or self.dialect == .jsonc) and self.options.pretty;
}

/// Emit a node's leading comments, one per line at `depth`.
fn leadingComments(self: *Printer, id: AST.Node.Id, depth: usize) Error!void {
    if (!self.commentsOn()) return;
    for (self.ast.comments(id).leading) |c| {
        try self.writeIndent(depth);
        try self.writeComment(c);
        try self.writer.writeByte('\n');
    }
}

/// Emit a node's trailing comment (if any) after a leading space, no newline.
fn trailingComment(self: *Printer, id: AST.Node.Id) Error!void {
    if (!self.commentsOn()) return;
    if (self.ast.comments(id).trailing) |c| {
        try self.writer.writeByte(' ');
        try self.writeComment(c);
    }
}

/// Emit a container's dangling comments (end of body), one per line at `depth`.
/// The caller has already gated on `commentsOn` via the passed slice.
fn danglingComments(self: *Printer, dangling: []const AST.Comment, depth: usize) Error!void {
    for (dangling) |c| {
        try self.writeIndent(depth);
        try self.writeComment(c);
        try self.writer.writeByte('\n');
    }
}

/// Render one comment in JSON5 syntax. JSON5 has both forms, so the stored
/// `style` is honored directly with no degradation.
fn writeComment(self: *Printer, c: AST.Comment) Error!void {
    switch (c.style) {
        .line => {
            try self.writer.writeAll("//");
            if (c.text.len != 0) {
                try self.writer.writeByte(' ');
                try self.writer.writeAll(c.text);
            }
        },
        .block => {
            try self.writer.writeAll("/*");
            if (c.text.len != 0) {
                try self.writer.writeByte(' ');
                try self.writer.writeAll(c.text);
                try self.writer.writeByte(' ');
            }
            try self.writer.writeAll("*/");
        },
    }
}

fn writeIndent(self: *Printer, depth: usize) Writer.Error!void {
    for (0..depth * self.options.indent) |_| try self.writer.writeByte(' ');
}

test "prints JSON document" {
    const Parser = @import("parser.zig");
    const input = "{\"name\":\"Ada\",\"tags\":[\"zig\",true,null]}";
    var doc = try Parser.parseAbstract(std.testing.allocator, input, .JSON);
    defer doc.deinit();

    var output: Writer.Allocating = .init(std.testing.allocator);
    defer output.deinit();

    try print(&output.writer, &doc, .{});
    try std.testing.expectEqualSlices(u8,
        \\{
        \\  "name": "Ada",
        \\  "tags": [
        \\    "zig",
        \\    true,
        \\    null
        \\  ]
        \\}
        \\
    , output.written());
}

test "prints compact JSON document" {
    const Parser = @import("parser.zig");
    const input = "{\"name\":\"Ada\",\"tags\":[\"zig\",true,null],\"empty\":{}}";
    var doc = try Parser.parseAbstract(std.testing.allocator, input, .JSON);
    defer doc.deinit();

    var output: Writer.Allocating = .init(std.testing.allocator);
    defer output.deinit();

    try print(&output.writer, &doc, .{ .pretty = false });
    try std.testing.expectEqualSlices(u8,
        \\{"name":"Ada","tags":["zig",true,null],"empty":{}}
        \\
    , output.written());
}

test "json5: unquoted keys, Infinity/NaN, pretty" {
    const Parser = @import("parser.zig");
    const input = "{ a: 1, 'b c': 2, while: true, n: NaN, inf: -Infinity }";
    var doc = try Parser.parseAbstract(std.testing.allocator, input, .JSON5);
    defer doc.deinit();

    var output: Writer.Allocating = .init(std.testing.allocator);
    defer output.deinit();

    try print5(&output.writer, &doc, .{});
    try std.testing.expectEqualSlices(u8,
        \\{
        \\  a: 1,
        \\  "b c": 2,
        \\  while: true,
        \\  n: NaN,
        \\  inf: -Infinity
        \\}
        \\
    , output.written());
}

test "json5: compact output" {
    const Parser = @import("parser.zig");
    const input = "{a:1,b:[2,Infinity,'x'],$_:3}";
    var doc = try Parser.parseAbstract(std.testing.allocator, input, .JSON5);
    defer doc.deinit();

    var output: Writer.Allocating = .init(std.testing.allocator);
    defer output.deinit();

    try print5(&output.writer, &doc, .{ .pretty = false });
    try std.testing.expectEqualSlices(u8,
        \\{a:1,b:[2,Infinity,"x"],$_:3}
        \\
    , output.written());
}

test "json5: round-trips through serialize and reparse" {
    const Parser = @import("parser.zig");
    const input = "{ a: .5, b: 0xC8, c: [+1, -Infinity, NaN], 'has space': null, while: 'kw' }";
    var doc = try Parser.parseAbstract(std.testing.allocator, input, .JSON5);
    defer doc.deinit();

    var output: Writer.Allocating = .init(std.testing.allocator);
    defer output.deinit();
    try print5(&output.writer, &doc, .{ .pretty = false });

    var reparsed = try Parser.parseAbstract(std.testing.allocator, output.written(), .JSON5);
    defer reparsed.deinit();
    try std.testing.expect(doc.eql(reparsed));
}

test "json dialect normalizes JSON5 number lexemes to valid JSON" {
    const Parser = @import("parser.zig");
    var doc = try Parser.parseAbstract(std.testing.allocator, "[0xFF, -0xa, .5, 5., +15, 1.5e3, -.25]", .JSON5);
    defer doc.deinit();

    var output: Writer.Allocating = .init(std.testing.allocator);
    defer output.deinit();
    try print(&output.writer, &doc, .{ .pretty = false });
    try std.testing.expectEqualSlices(u8,
        \\[255,-10,0.5,5.0,15,1.5e3,-0.25]
        \\
    , output.written());
}

test "json dialect degrades Infinity to a quoted string" {
    const Parser = @import("parser.zig");
    var doc = try Parser.parseAbstract(std.testing.allocator, "Infinity", .JSON5);
    defer doc.deinit();

    var output: Writer.Allocating = .init(std.testing.allocator);
    defer output.deinit();
    // The same extended node, rendered by the plain-JSON dialect.
    try print(&output.writer, &doc, .{ .pretty = false });
    try std.testing.expectEqualSlices(u8, "\"Infinity\"\n", output.written());
}

test "json5 emits leading and trailing comments; plain json drops them" {
    const a = std.testing.allocator;
    var b = AST.Builder.init(a);
    defer b.deinit();

    // { name: "fig" } with a leading line comment on the entry, a trailing line
    // comment on the value, and a leading block comment on the document.
    const v = try b.addString("fig");
    try b.setComments(v, .{ .trailing = .{ .text = "inline", .style = .line } });
    const k = try b.addString("name");
    try b.setComments(k, .{ .leading = &.{.{ .text = "greeting", .style = .line }} });
    const root = try b.addMapping(&.{.{ .key = k, .value = v }});
    try b.setComments(root, .{ .leading = &.{.{ .text = "doc", .style = .block }} });

    var ast = try b.finish(root);
    defer ast.deinit();

    var j5: Writer.Allocating = .init(a);
    defer j5.deinit();
    try print5(&j5.writer, &ast, .{});
    try std.testing.expectEqualStrings(
        \\/* doc */
        \\{
        \\  // greeting
        \\  name: "fig" // inline
        \\}
        \\
    , j5.written());

    // Plain JSON has no comment syntax: same AST emits clean JSON.
    var j: Writer.Allocating = .init(a);
    defer j.deinit();
    try print(&j.writer, &ast, .{});
    try std.testing.expectEqualStrings(
        \\{
        \\  "name": "fig"
        \\}
        \\
    , j.written());

    // Compact JSON5 also drops comments (a `//` can't survive one line).
    var c: Writer.Allocating = .init(a);
    defer c.deinit();
    try print5(&c.writer, &ast, .{ .pretty = false });
    try std.testing.expectEqualStrings("{name:\"fig\"}\n", c.written());
}

test "json5: a container value's trailing comment rides its opening bracket" {
    const Parser = @import("parser.zig");
    // `key: [ // note` round-trips: the comment stays beside the `[` (next to its
    // key), not after the distant `]`.
    const input =
        \\{
        \\  contents: [ // the note
        \\    "a",
        \\    "b"
        \\  ]
        \\}
    ;
    var doc = try Parser.parseAbstract(std.testing.allocator, input, .JSON5);
    defer doc.deinit();
    var out: Writer.Allocating = .init(std.testing.allocator);
    defer out.deinit();
    try print5(&out.writer, &doc, .{});
    try std.testing.expectEqualStrings(input ++ "\n", out.written());
}

test "json5: opening-line comment stays at top, closing-line at bottom" {
    const Parser = @import("parser.zig");
    // A comment on the `]` line is a bottom comment: it normalizes to the last
    // line of the body (before the close), distinct from the opening-line one.
    var doc = try Parser.parseAbstract(std.testing.allocator,
        \\{
        \\  a: [ // top
        \\    1
        \\  ],
        \\  b: [
        \\    2
        \\  ] // bottom
        \\}
    , .JSON5);
    defer doc.deinit();
    var out: Writer.Allocating = .init(std.testing.allocator);
    defer out.deinit();
    try print5(&out.writer, &doc, .{});
    try std.testing.expectEqualStrings(
        \\{
        \\  a: [ // top
        \\    1
        \\  ],
        \\  b: [
        \\    2
        \\    // bottom
        \\  ]
        \\}
        \\
    , out.written());
}

test "jsonc emits comments with quoted keys (unlike json5)" {
    const a = std.testing.allocator;
    var b = AST.Builder.init(a);
    defer b.deinit();

    const v = try b.addString("fig");
    try b.setComments(v, .{ .trailing = .{ .text = "inline", .style = .line } });
    const k = try b.addString("name");
    try b.setComments(k, .{ .leading = &.{.{ .text = "greeting", .style = .block }} });
    const root = try b.addMapping(&.{.{ .key = k, .value = v }});

    var ast = try b.finish(root);
    defer ast.deinit();

    var out: Writer.Allocating = .init(a);
    defer out.deinit();
    try printc(&out.writer, &ast, .{});
    // JSON syntax (quoted key) + JSON5-style comments.
    try std.testing.expectEqualStrings(
        \\{
        \\  /* greeting */
        \\  "name": "fig" // inline
        \\}
        \\
    , out.written());
}

test "honors custom indent width" {
    const Parser = @import("parser.zig");
    const input = "{\"a\":[1]}";
    var doc = try Parser.parseAbstract(std.testing.allocator, input, .JSON);
    defer doc.deinit();

    var output: Writer.Allocating = .init(std.testing.allocator);
    defer output.deinit();

    try print(&output.writer, &doc, .{ .indent = 4 });
    try std.testing.expectEqualSlices(u8,
        \\{
        \\    "a": [
        \\        1
        \\    ]
        \\}
        \\
    , output.written());
}

// ── Mapping keys (see `key`) ────────────────────────────────────────────────
// A JSON object key is a string, but the AST's key node can be any kind. These
// build the AST literally (as the XML/plist printer tests do) rather than
// parsing, because no JSON dialect can *write* the keys under test — they come
// from YAML, whose `null: a`, `23: x` and `? [a, b] : c` are all legal.

/// `{ <key node> : "v" }`, given the key node's kind. Node 0 is the mapping,
/// 1 the entry, 2 the key, 3 the value.
fn keyKindAst(kind: AST.Node.Kind, nodes: *[4]AST.Node) AST {
    nodes.* = .{
        .{ .id = 0, .kind = .{ .mapping = 1 } },
        .{ .id = 1, .kind = .{ .keyvalue = .{ .key = 2, .value = 3 } } },
        .{ .id = 2, .kind = kind },
        .{ .id = 3, .kind = .{ .string = "v" } },
    };
    return .{ .allocator = std.testing.allocator, .root = 0, .nodes = nodes };
}

/// Print `{ <key> : "v" }` compactly in `dialect` and return the bytes (owned by
/// `out`), or the printer's error.
fn printKeyKind(kind: AST.Node.Kind, dialect: Dialect, out: *Writer.Allocating) Error![]const u8 {
    var nodes: [4]AST.Node = undefined;
    const ast = keyKindAst(kind, &nodes);
    switch (dialect) {
        .json => try print(&out.writer, &ast, .{ .pretty = false }),
        .jsonc => try printc(&out.writer, &ast, .{ .pretty = false }),
        .json5 => try print5(&out.writer, &ast, .{ .pretty = false }),
    }
    return out.written();
}

fn expectKeyKind(kind: AST.Node.Kind, dialect: Dialect, expected: []const u8) !void {
    var out: Writer.Allocating = .init(std.testing.allocator);
    defer out.deinit();
    try std.testing.expectEqualStrings(expected, try printKeyKind(kind, dialect, &out));
}

fn expectKeyKindError(kind: AST.Node.Kind, dialect: Dialect, expected: anyerror) !void {
    var out: Writer.Allocating = .init(std.testing.allocator);
    defer out.deinit();
    try std.testing.expectError(expected, printKeyKind(kind, dialect, &out));
}

test "key kind: a string key quotes, except a JSON5 bare identifier" {
    try expectKeyKind(.{ .string = "k" }, .json, "{\"k\":\"v\"}\n");
    try expectKeyKind(.{ .string = "k" }, .jsonc, "{\"k\":\"v\"}\n");
    try expectKeyKind(.{ .string = "k" }, .json5, "{k:\"v\"}\n");
    // Not an identifier: quoted even in JSON5.
    try expectKeyKind(.{ .string = "a b" }, .json5, "{\"a b\":\"v\"}\n");
}

test "key kind: a null key spells as the string \"null\"" {
    try expectKeyKind(.null_, .json, "{\"null\":\"v\"}\n");
    try expectKeyKind(.null_, .jsonc, "{\"null\":\"v\"}\n");
    // Quoted in JSON5 too: a spelled key reads the same in every dialect.
    try expectKeyKind(.null_, .json5, "{\"null\":\"v\"}\n");
}

test "key kind: a boolean key spells as \"true\"/\"false\"" {
    try expectKeyKind(.{ .boolean = true }, .json, "{\"true\":\"v\"}\n");
    try expectKeyKind(.{ .boolean = false }, .json, "{\"false\":\"v\"}\n");
    try expectKeyKind(.{ .boolean = true }, .json5, "{\"true\":\"v\"}\n");
}

test "key kind: a number key spells as its source lexeme, quoted" {
    try expectKeyKind(.{ .number = .{ .raw = "23", .kind = .integer } }, .json, "{\"23\":\"v\"}\n");
    try expectKeyKind(.{ .number = .{ .raw = "-1.5e3", .kind = .float } }, .json, "{\"-1.5e3\":\"v\"}\n");
    // The lexeme verbatim, NOT the JSON-normalized number a value would get.
    try expectKeyKind(.{ .number = .{ .raw = "0x1F", .kind = .integer } }, .json, "{\"0x1F\":\"v\"}\n");
}

test "key kind: an extended scalar key spells as its text" {
    try expectKeyKind(.{ .extended = .{ .kind = .local_date, .text = "1979-05-27" } }, .json, "{\"1979-05-27\":\"v\"}\n");
    // Even in JSON5, where the same scalar as a VALUE prints bare.
    try expectKeyKind(.{ .extended = .{ .kind = .number_special, .text = "-Infinity" } }, .json5, "{\"-Infinity\":\"v\"}\n");
}

test "key kind: an alias key is UnresolvedAlias, like an alias value" {
    // Not `NonStringKey`: it means an unmaterialized YAML AST reached a
    // non-YAML printer, not a key JSON has no room for.
    try expectKeyKindError(.{ .alias = "a" }, .json, error.UnresolvedAlias);
    try expectKeyKindError(.{ .alias = "a" }, .json5, error.UnresolvedAlias);
}

test "key kind: a sequence key is NonStringKey" {
    try expectKeyKindError(.{ .sequence = null }, .json, error.NonStringKey);
    try expectKeyKindError(.{ .sequence = null }, .jsonc, error.NonStringKey);
    try expectKeyKindError(.{ .sequence = null }, .json5, error.NonStringKey);
}

test "key kind: a mapping key is NonStringKey" {
    try expectKeyKindError(.{ .mapping = null }, .json, error.NonStringKey);
    try expectKeyKindError(.{ .mapping = null }, .jsonc, error.NonStringKey);
    try expectKeyKindError(.{ .mapping = null }, .json5, error.NonStringKey);
}
