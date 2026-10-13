//! Serialization: the `serialize*` entry points, which print a tree as
//! core's canonical form or through a registered language's printer. The
//! options and errors a printer takes are the kit's (`kit/ast/options.zig`).

const std = @import("std");
const Writer = std.Io.Writer;
const build_options = @import("build_options");

const AST = @import("kit/ast/ast.zig");
pub const SerializeOptions = AST.SerializeOptions;
pub const SerializeError = AST.SerializeError;
const Node = AST.Node;
const Runtime = @import("languages/runtime.zig");

// The canonical form is the AST's own 1:1 oracle encoding. It is not exposed
// through the C ABI or any binding, so it is opt-in (`-Dcanonical=true`) —
// but ALWAYS compiled for a test build (`is_test`), since the suite leans on
// it as a comparison oracle. When gated out, `CanonicalPrinter` is `void`
// and the guarded arms below are never analyzed.
const canonical_enabled = build_options.lang_canonical or @import("builtin").is_test;
const CanonicalPrinter = if (canonical_enabled) @import("canonical/printer.zig") else void;

/// What a tree is printed as: core's canonical form, or a registered
/// language, whose printer is reached through its vtable.
pub const Target = union(enum) {
    canonical,
    language: *const Runtime.Entry,

    /// The registered dialect named `name`, or null.
    pub fn named(dialect: []const u8) ?Target {
        if (std.mem.eql(u8, dialect, "canonical")) return .canonical;
        return .{ .language = Runtime.entryByName(dialect) orelse return null };
    }

    /// What the target is called in a message.
    pub fn name(self: Target) []const u8 {
        return switch (self) {
            .canonical => "canonical",
            .language => |e| e.name,
        };
    }
};

/// `self`, or a comment-stripped *view* of it when `options.strip_comments` is
/// set. The view is a shallow struct copy that shares all row/string storage and
/// only blanks the comment lists, so stripping costs no allocation. `buf` provides
/// the view's stack storage; the returned pointer is valid for `buf`'s lifetime.
fn commentView(self: *const AST, options: SerializeOptions, buf: *AST) *const AST {
    if (!options.strip_comments) return self;
    buf.* = self.*;
    buf.comment_runs = &.{};
    buf.comment_list = &.{};
    return buf;
}

/// Render the whole AST to `writer` as `target`, using default options.
pub fn serialize(self: *const AST, writer: *Writer, target: Target) SerializeError!void {
    return serializeWith(self, writer, target, .{});
}

/// Render the whole AST to `writer` as `target`, controlling output style via
/// `options`.
pub fn serializeWith(self: *const AST, writer: *Writer, target: Target, options: SerializeOptions) SerializeError!void {
    if (target == .canonical) {
        var buf: AST = undefined;
        return if (comptime canonical_enabled) CanonicalPrinter.print(writer, commentView(self, options, &buf)) else error.FormatDisabled;
    }
    return serializeNodeWith(self, writer, target, self.root, options);
}

/// Render the whole AST as a value *fragment*. A language's printer is told
/// a fragment by the options it reads (`splice`, `flow`) and by its root, so
/// this is `serializeWith`; the name says what the caller holds.
pub fn serializeFragmentWith(self: *const AST, writer: *Writer, target: Target, options: SerializeOptions) SerializeError!void {
    return serializeWith(self, writer, target, options);
}

/// Render the subtree rooted at `id`, using default options.
pub fn serializeNode(self: *const AST, writer: *Writer, target: Target, id: Node.Id) SerializeError!void {
    return serializeNodeWith(self, writer, target, id, .{});
}

/// Render the subtree rooted at `id`, controlling output style via `options`.
pub fn serializeNodeWith(self: *const AST, writer: *Writer, target: Target, id: Node.Id, options: SerializeOptions) SerializeError!void {
    var buf: AST = undefined;
    const ast = commentView(self, options, &buf);
    return switch (target) {
        .canonical => if (comptime canonical_enabled) CanonicalPrinter.printNode(writer, ast, id, 0) else error.FormatDisabled,
        .language => |e| Runtime.printNodeWith(e, writer, ast, id, options) catch |err| switch (err) {
            // A writer reports its own allocation failure as `WriteFailed`.
            error.OutOfMemory, error.WriteFailed => error.WriteFailed,
            error.NullUnsupported, error.NonStringKey, error.UnrepresentableRoot, error.UnsupportedValue => |x| x,
            error.InvalidKey, error.NonFiniteNumber, error.UnresolvedAlias, error.NestingTooDeep, error.UnexpectedNodeKind => |x| x,
            error.FormatDisabled => error.FormatDisabled,
            else => error.PrintRefused,
        },
    };
}

/// The registered dialect `name`, for a test.
fn testTarget(name: []const u8) Target {
    return Target.named(name).?;
}

// ── trees built by hand, printed through the registry ──────────────────────

test "Builder constructs an AST that serializes" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    const testing = std.testing;
    var b = AST.Builder.init(testing.allocator);
    defer b.deinit();

    // Build { "name": "fig", "nums": [1, 2] } bottom-up.
    const v_name = try b.addString("fig");
    const n1 = try b.addInt(1);
    const n2 = try b.addInt(2);
    const v_nums = try b.addSequence(&.{ n1, n2 });
    const k_name = try b.addString("name");
    const k_nums = try b.addString("nums");
    const root = try b.addMapping(&.{
        .{ .key = k_name, .value = v_name },
        .{ .key = k_nums, .value = v_nums },
    });

    var ast = try b.finish(root);
    defer ast.deinit();

    var buf: [256]u8 = undefined;
    var w = std.Io.Writer.fixed(&buf);
    try serialize(&ast, &w, testTarget("json"));
    try testing.expectEqualStrings(
        \\{
        \\  "name": "fig",
        \\  "nums": [
        \\    1,
        \\    2
        \\  ]
        \\}
        \\
    , w.buffered());

    // Same AST, YAML — exercises the empty side-table (anchor/tag) guard path.
    if (comptime build_options.lang_yaml) {
        var ybuf: [256]u8 = undefined;
        var yw = std.Io.Writer.fixed(&ybuf);
        try serialize(&ast, &yw, testTarget("yaml"));
        try testing.expectEqualStrings(
            \\name: fig
            \\nums: [1, 2]
            \\
        , yw.buffered());
    }
}

test "Builder.setTag surfaces a normalized kind tag as a YAML core tag" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const testing = std.testing;
    var b = AST.Builder.init(testing.allocator);
    defer b.deinit();
    const v = try b.addNumberRaw("09", false); // integer lexeme kept verbatim
    try b.setTag(v, .{ .kind = .integer });
    const k = try b.addString("id");
    const root = try b.addMapping(&.{.{ .key = k, .value = v }});
    var ast = try b.finish(root);
    defer ast.deinit();
    try testing.expect(ast.tagOf(v).?.kind == .integer);

    var buf: [64]u8 = undefined;
    var w = std.Io.Writer.fixed(&buf);
    try serialize(&ast, &w, testTarget("yaml"));
    // A `.kind` tag renders as its YAML core-schema shorthand; the lexeme is kept.
    try testing.expect(std.mem.indexOf(u8, w.buffered(), "!!int") != null);
    try testing.expect(std.mem.indexOf(u8, w.buffered(), "09") != null);
}

test "strip_comments drops carried comments across formats" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    var b = AST.Builder.init(std.testing.allocator);
    defer b.deinit();
    const v = try b.addString("fig");
    const k = try b.addString("name");
    try b.setComments(k, .{ .leading = &.{.{ .text = "hi", .style = .line }} });
    const root = try b.addMapping(&.{.{ .key = k, .value = v }});
    var ast = try b.finish(root);
    defer ast.deinit();

    // Default: comment preserved (JSON5).
    var kept: Writer.Allocating = .init(std.testing.allocator);
    defer kept.deinit();
    try serializeWith(&ast, &kept.writer, testTarget("json5"), .{});
    try std.testing.expect(std.mem.indexOf(u8, kept.written(), "// hi") != null);

    // Stripped: same AST, no comment, in JSON5 and (an options-less printer) YAML.
    var stripped: Writer.Allocating = .init(std.testing.allocator);
    defer stripped.deinit();
    try serializeWith(&ast, &stripped.writer, testTarget("json5"), .{ .strip_comments = true });
    try std.testing.expect(std.mem.indexOf(u8, stripped.written(), "hi") == null);

    if (comptime build_options.lang_yaml) {
        var y: Writer.Allocating = .init(std.testing.allocator);
        defer y.deinit();
        try serializeWith(&ast, &y.writer, testTarget("yaml"), .{ .strip_comments = true });
        try std.testing.expectEqualStrings("name: fig\n", y.written());
    }
}

test "Builder incremental comment helpers append and serialize" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    var b = AST.Builder.init(std.testing.allocator);
    defer b.deinit();

    const v = try b.addNumberRaw("1", false);
    const k = try b.addString("a");
    // Two leading comments accumulate in call order; a trailing rides the value.
    try b.addLeadingComment(k, .{ .text = "first" });
    try b.addLeadingComment(k, .{ .text = "second" });
    try b.setTrailingComment(v, .{ .text = "tail" });
    const root = try b.addMapping(&.{.{ .key = k, .value = v }});
    // A dangling comment sits at the end of the mapping body.
    try b.addDanglingComment(root, .{ .text = "end" });

    var ast = try b.finish(root);
    defer ast.deinit();

    // The leading run kept insertion order and the trailing/dangling bound too.
    const kc = ast.comments(k);
    try std.testing.expectEqual(@as(usize, 2), kc.leading.len);
    try std.testing.expectEqualStrings("first", kc.leading[0].text);
    try std.testing.expectEqualStrings("second", kc.leading[1].text);
    try std.testing.expectEqualStrings("tail", ast.comments(v).trailing.?.text);
    try std.testing.expectEqualStrings("end", ast.comments(root).dangling[0].text);

    // A comment-bearing format renders them; this also proves the runs are valid.
    var out: Writer.Allocating = .init(std.testing.allocator);
    defer out.deinit();
    try serializeWith(&ast, &out.writer, testTarget("json5"), .{});
    const text = out.written();
    try std.testing.expect(std.mem.indexOf(u8, text, "// first") != null);
    try std.testing.expect(std.mem.indexOf(u8, text, "// second") != null);
    try std.testing.expect(std.mem.indexOf(u8, text, "// tail") != null);
    try std.testing.expect(std.mem.indexOf(u8, text, "// end") != null);
}
