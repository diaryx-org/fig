//! Serialization — the canonical output formats and the `serialize*` entry
//! points that dispatch to each format's printer. Core's, since the dispatch
//! reads the format registry; the options and errors a printer takes are the
//! kit's (`kit/ast/options.zig`).

const std = @import("std");
const Writer = std.Io.Writer;
const build_options = @import("build_options");

const AST = @import("kit/ast/ast.zig");
pub const SerializeOptions = AST.SerializeOptions;
pub const SerializeError = AST.SerializeError;
const Node = AST.Node;
/// The format registry. `SerializeFormat`'s members come from it, and so does
/// every dispatch arm below: a registry entry names the language whose
/// `Printer` module prints the dialect (`entry.Lang.Printer`) and the two
/// declarations in it that do the printing (`entry.print_name`,
/// `entry.print_node_name`). No printer is imported by path here any more —
/// `canonical`'s is the one exception, since canonical is not a `Language`.
const Language = @import("languages/language.zig");
const Runtime = @import("languages/runtime.zig");

// Printers reach this file only through the registry, and only for the formats
// compiled into this build: a gated-out language collapses its entry's `Lang`
// to `void`, the void guard that opens every arm below returns
// `error.FormatDisabled` before anything reads `Lang.Printer`, and the printer
// never compiles.
//
// The canonical form is the AST's own 1:1 oracle encoding. It is not exposed
// through the C ABI or any binding, so it is opt-in (`-Dcanonical=true`) like
// plist — but ALWAYS compiled for a test build (`is_test`), since the suite leans
// on it as a comparison oracle. When gated out, `CanonicalPrinter` is `void`
// and the guarded arms below are never analyzed, so its code never compiles in.
const canonical_enabled = build_options.lang_canonical or @import("builtin").is_test;
const CanonicalPrinter = if (canonical_enabled) @import("canonical/printer.zig") else void;

/// The canonical output format families. `canonical` (formerly `native`) is the
/// AST's own 1:1 oracle encoding; `fig` is the human-facing authoring dialect
/// (lossy at the edges — see src/languages/fig/DESIGN.md).
pub const SerializeFormat = @Enum(
    Language.EnumTag(serialize_format_names),
    .exhaustive,
    serialize_format_names,
    &Language.enumValues(serialize_format_names),
);

/// Every format registry entry (`languages/language.zig`'s `dialects`), in
/// registry order, plus `canonical` directly after `zon` — where the
/// hand-written enum this replaces put it, once generic `xml` (which used to
/// sit between them) was removed in core 3.0. `canonical` is not a `Language` at
/// all (no parser module, no dialect, an options-less printer), so it stays an
/// explicit named arm in every switch below and is spliced in by hand here.
///
/// Unlike `cli.Format` there is no `gron`: gron is a CLI-only projection with
/// no printer on this side, and its callers intercept it before the serializer
/// dispatch (see `cli/types.zig`'s `toSerializeFormat`).
const serialize_format_names = blk: {
    @setEvalBranchQuota(20_000);
    break :blk Language.namesWith(.all, &.{.{ .after = "zon", .name = "canonical" }});
};

// Membership and registry order are true by construction (`namesWith` fails
// the build if `canonical` names an entry that does not exist to follow); what
// is left to state is that `canonical` belongs beside `zon` specifically, not
// merely somewhere in the list.
comptime {
    if (@backingInt(SerializeFormat.canonical) != @backingInt(SerializeFormat.zon) + 1)
        @compileError("AST.SerializeFormat's `canonical` no longer sits directly after `zon`");
}

/// `self`, or a comment-stripped *view* of it when `options.strip_comments` is
/// set. The view is a shallow struct copy that shares all node/string storage and
/// only blanks `node_comments`, so stripping costs no allocation. `buf` provides
/// the view's stack storage; the returned pointer is valid for `buf`'s lifetime.
fn commentView(self: *const AST, options: SerializeOptions, buf: *AST) *const AST {
    if (!options.strip_comments) return self;
    buf.* = self.*;
    buf.node_comments = &.{};
    return buf;
}

/// Render the whole AST to `writer` in the given format, using default options.
/// Does not handle aliases, tags, or lossless `$fig` envelopes.
pub fn serialize(self: *const AST, writer: *Writer, format: SerializeFormat) SerializeError!void {
    return serializeWith(self, writer, format, .{});
}

/// Render the whole AST to `writer`, controlling output style via `options`.
pub fn serializeWith(self: *const AST, writer: *Writer, format: SerializeFormat, options: SerializeOptions) SerializeError!void {
    @setEvalBranchQuota(30_000);
    var buf: AST = undefined;
    const ast = commentView(self, options, &buf);
    return switch (format) {
        // The AST's own 1:1 oracle encoding — not a `Language`, so not a
        // registry entry, so its own arm. Its printer also takes no options
        // (the canonical form has exactly one spelling).
        .canonical => if (comptime canonical_enabled) CanonicalPrinter.print(writer, ast) else error.FormatDisabled,
        inline else => |f| {
            const d = comptime Language.entryFor(@tagName(f));
            if (comptime d.Lang == void) return registered(@tagName(f), writer, ast, ast.root, options);
            // `print_name` is what separates the three JSON dialects, which
            // share one printer module and differ only by entry point
            // (`print`/`printc`/`print5`), and what routes YAML to its
            // options-taking `printWith`.
            return @field(d.Lang.Printer, d.print_name)(writer, ast, options);
        },
    };
}

/// Render the whole AST to `writer` as a value *fragment*, controlling output
/// style via `options`. Identical to `serializeWith` except for a dialect
/// that declares `print_fragment_name`: one whose document printer refuses
/// a root a value may have (a scalar, where a whole document must be a
/// container), which a value fragment built by the caller
/// (`fig_value_serialize_opts`, backing the editors' `replace`/`set` and the
/// bindings' `serialize`) is not bound by.
///
/// With `options.splice` the fragment is the editor's splice text, and a
/// printer that declares `printSplice` renders it — see that option.
pub fn serializeFragmentWith(self: *const AST, writer: *Writer, format: SerializeFormat, options: SerializeOptions) SerializeError!void {
    @setEvalBranchQuota(30_000);
    var buf: AST = undefined;
    const ast = commentView(self, options, &buf);
    return switch (format) {
        .canonical => if (comptime canonical_enabled) CanonicalPrinter.print(writer, ast) else error.FormatDisabled,
        inline else => |f| {
            const d = comptime Language.entryFor(@tagName(f));
            if (comptime d.Lang == void) return registered(@tagName(f), writer, ast, ast.root, options);
            if (comptime @hasDecl(d.Lang.Printer, "printSplice")) {
                if (options.splice) return d.Lang.Printer.printSplice(writer, ast, options);
            }
            return @field(d.Lang.Printer, d.print_fragment_name orelse d.print_name)(writer, ast, options);
        },
    };
}

/// Render the subtree rooted at `id` to `writer`, using default options.
pub fn serializeNode(self: *const AST, writer: *Writer, format: SerializeFormat, id: Node.Id) SerializeError!void {
    return serializeNodeWith(self, writer, format, id, .{});
}

/// Render the subtree rooted at `id`, controlling output style via `options`.
pub fn serializeNodeWith(self: *const AST, writer: *Writer, format: SerializeFormat, id: Node.Id, options: SerializeOptions) SerializeError!void {
    @setEvalBranchQuota(30_000);
    var buf: AST = undefined;
    const ast = commentView(self, options, &buf);
    return switch (format) {
        .canonical => if (comptime canonical_enabled) CanonicalPrinter.printNode(writer, ast, id, 0) else error.FormatDisabled,
        inline else => |f| {
            const d = comptime Language.entryFor(@tagName(f));
            if (comptime d.Lang == void) return registered(@tagName(f), writer, ast, id, options);
            // `printNode` on the printer MODULE, which every format has —
            // including plist, whose `Language` declares no `printNode`
            // of its own (its `print` is written inline; see `Decls.optional`
            // in languages/language.zig).
            return @field(d.Lang.Printer, d.print_node_name)(writer, ast, id, 0, options);
        },
    };
}

/// Print through the language registered under `name`, for a format this
/// build does not compile in.
fn registered(name: []const u8, writer: *Writer, ast: *const AST, root: Node.Id, options: SerializeOptions) SerializeError!void {
    const e = Runtime.entryByName(name) orelse return error.FormatDisabled;
    Runtime.printNodeWith(e, writer, ast, root, options) catch |err| return switch (err) {
        // A writer reports its own allocation failure as `WriteFailed`.
        error.OutOfMemory, error.WriteFailed => error.WriteFailed,
        error.NullUnsupported, error.NonStringKey, error.UnrepresentableRoot, error.UnsupportedValue => |x| x,
        error.InvalidKey, error.NonFiniteNumber, error.UnresolvedAlias, error.NestingTooDeep, error.UnexpectedNodeKind => |x| x,
        error.FormatDisabled => error.FormatDisabled,
        else => error.PrintRefused,
    };
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
    try serialize(&ast, &w, .json);
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
        try serialize(&ast, &yw, .yaml);
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
    try testing.expect(ast.node_tags[v].?.kind == .integer);

    var buf: [64]u8 = undefined;
    var w = std.Io.Writer.fixed(&buf);
    try serialize(&ast, &w, .yaml);
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
    try serializeWith(&ast, &kept.writer, .json5, .{});
    try std.testing.expect(std.mem.indexOf(u8, kept.written(), "// hi") != null);

    // Stripped: same AST, no comment, in JSON5 and (an options-less printer) YAML.
    var stripped: Writer.Allocating = .init(std.testing.allocator);
    defer stripped.deinit();
    try serializeWith(&ast, &stripped.writer, .json5, .{ .strip_comments = true });
    try std.testing.expect(std.mem.indexOf(u8, stripped.written(), "hi") == null);

    if (comptime build_options.lang_yaml) {
        var y: Writer.Allocating = .init(std.testing.allocator);
        defer y.deinit();
        try serializeWith(&ast, &y.writer, .yaml, .{ .strip_comments = true });
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
    try serializeWith(&ast, &out.writer, .json5, .{});
    const text = out.written();
    try std.testing.expect(std.mem.indexOf(u8, text, "// first") != null);
    try std.testing.expect(std.mem.indexOf(u8, text, "// second") != null);
    try std.testing.expect(std.mem.indexOf(u8, text, "// tail") != null);
    try std.testing.expect(std.mem.indexOf(u8, text, "// end") != null);
}
