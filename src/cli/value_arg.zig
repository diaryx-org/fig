//! A value argument — `set`'s value and `--seq` items, `insert`'s value,
//! `edit`'s replacement — read once as a fig value and rendered into whatever
//! format it lands in, the way a binding's `Value` is (CLI 5 §1). The format
//! decides the spelling, never the meaning: `fig set f version 5` writes the
//! number 5 into JSON, YAML and TOML alike, and `hello` is a string in all
//! three.
//!
//! Reading, by `Mode`:
//!
//!   * `.fig` (the default) — the argument is what `v = <arg>` makes of it in
//!     fig's own dialect: `5`, `2.5`, `true`, `null` and `2026-09-22` are
//!     typed; `hello world`, `Yes` and `007` are strings (the last two with
//!     the lint the dialect gives them); `[1, 2]` and `{a = 1}` are
//!     structures; `'"5"'` is the string `5`. An empty argument, one with a
//!     line break in it, and one in which a `#` would start a comment
//!     (`see #3`) are strings as written — a command line has no comments.
//!     An argument that is none of these (`[1, 2`) is a usage error.
//!   * `.string` (`--string`) — a string, whatever it looks like.
//!   * `.raw` (`--raw`) — the argument's own text, spliced as source in the
//!     target format and reparsed in place: for what only the target can
//!     spell (a YAML anchor, a TOML local datetime, a ZON enum literal).
//!
//! Rendering (`render`) is `fig patch`'s: the value re-rooted as a fragment
//! with `splice` set, flow for fig, and the printer's newline trimmed — on
//! one line for JSON, and for any format when the site is inside a flow
//! collection (`Layout.flow`).
const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");

const types = @import("types.zig");
const diag_report = @import("diag_report.zig");
const parse_dispatch = @import("parse_dispatch.zig");

const Format = types.Format;
const Io = std.Io;

pub const Mode = types.ValueMode;

/// A value argument as read: its tree, or (under `--raw`) its text.
pub const Value = union(enum) {
    tree: fig.AST,
    raw: []const u8,
};

/// What `v = ` puts in front of the argument to make it a fig document.
const prefix = "v = ";

/// Read `text` under `mode`. A `.fig` argument that is not a value is
/// reported against the argument itself and exits 2 — the command line is
/// wrong, not the file; its lints are printed to `term` and reading goes on.
pub fn read(allocator: std.mem.Allocator, term: *Io.Terminal, text: []const u8, mode: Mode) !Value {
    return switch (mode) {
        .raw => .{ .raw = text },
        .string => .{ .tree = try stringTree(allocator, text) },
        .fig => readFig(allocator, term, text),
    };
}

fn readFig(allocator: std.mem.Allocator, term: *Io.Terminal, text: []const u8) !Value {
    // Without the fig dialect compiled in there is nothing to read a value
    // with; every argument is then a string, which is the one reading that
    // needs no parser.
    if (comptime !build_options.lang_fig) return .{ .tree = try stringTree(allocator, text) };

    if (std.mem.trim(u8, text, " \t").len == 0 or std.mem.indexOfAny(u8, text, "\r\n") != null)
        return .{ .tree = try stringTree(allocator, text) };

    const source = try std.mem.concat(allocator, u8, &.{ prefix, text, "\n" });
    var reports: parse_dispatch.Reports = .{};
    const doc = parse_dispatch.parseSliceAs(.fig, .{}, allocator, source, false, &reports) catch |err| {
        if (std.mem.indexOfScalar(u8, text, '#') != null) return .{ .tree = try stringTree(allocator, text) };
        const d = reports.fig.diag orelse return err;
        const P = fig.Language.entryFor("fig").Lang.Parser;
        try diag_report.printDiag(term, text, "<value>", shift(d.offset, text), if (d.end) |e| shift(e, text) else null, "error", .red, P.describe(d.code), P.shortLabel(d.code));
        try term.writer.print("help: this argument is read as a fig value; pass --string to take it as a string, or --raw to splice it as the file's own syntax\n", .{});
        try term.writer.flush();
        std.process.exit(2);
    };
    // A `#` the dialect took as a comment start was part of the argument.
    if (doc.ast.node_comments.len > 0) return .{ .tree = try stringTree(allocator, text) };

    const W = fig.Language.entryFor("fig").Lang.Parser.Warning;
    for (reports.fig.warnings) |w|
        try diag_report.printDiag(term, text, "<value>", shift(w.offset, text), if (w.end) |e| shift(e, text) else null, "warning", .yellow, W.describeWarning(w.code), W.shortLabel(w.code));
    try term.writer.flush();

    var ast = doc.ast;
    ast.root = (try ast.getValByPath(&.{.{ .key = "v" }})).id;
    return .{ .tree = ast };
}

/// An offset into `v = <text>\n`, as an offset into `text`.
fn shift(offset: usize, text: []const u8) usize {
    return @min(offset -| prefix.len, text.len);
}

fn stringTree(allocator: std.mem.Allocator, text: []const u8) !fig.AST {
    var b = fig.AST.Builder.init(allocator);
    const id = try b.addString(text);
    return b.finish(id);
}

/// How a structure is laid out: the format's own layout, or on one line.
pub const Layout = enum {
    /// A YAML block, a TOML inline table, a ZON literal across lines: what
    /// the format writes for a value of its own. JSON is compact either way,
    /// since a value spliced into an object cannot span lines.
    natural,
    /// On one line, for a site inside a flow collection (`k: {a: 1}` in
    /// YAML), where a block spelling cannot land. YAML's is JSON's, which
    /// every YAML reads as flow.
    flow,
};

/// The splice text for `value` in `format` — what the editor takes for it.
/// A value the format has no spelling for (`null` in TOML, a table in
/// dotenv) is the serializer's error, for the caller to report.
pub fn render(allocator: std.mem.Allocator, value: Value, format: Format, layout: Layout) ![]const u8 {
    const ast = switch (value) {
        .raw => |text| return text,
        .tree => |*t| t,
    };
    var w: std.Io.Writer.Allocating = .init(allocator);
    defer w.deinit();
    const one_line = layout == .flow or switch (format) {
        .json, .jsonc, .json5 => true,
        else => false,
    };
    const options: fig.AST.SerializeOptions = .{
        // What the editor takes, not a document — see `SerializeOptions.splice`.
        .splice = true,
        .pretty = !one_line,
        // fig's block spellings only parse as standalone lines; a value
        // spliced after `key = ` has to be flow.
        .flow = format == .fig,
    };
    if (types.runtimeEntry(format)) |e| {
        try fig.Runtime.printNodeWith(e, &w.writer, ast, ast.root, options);
    } else {
        const target: fig.AST.SerializeFormat = if (layout == .flow and format == .yaml)
            .json
        else
            types.toSerializeFormat(format) orelse return error.UnsupportedValueTarget;
        try ast.serializeFragmentWith(&w.writer, target, options);
    }
    // Every printer ends a document with a newline; a spliced value never
    // carries one (an appended flow item would land on a line of its own).
    return allocator.dupe(u8, std.mem.trimEnd(u8, w.written(), "\n"));
}

test "a fig reading types what fig types and keeps the rest a string" {
    if (comptime !build_options.lang_fig or !build_options.lang_json) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var sink: std.Io.Writer.Discarding = .init(&.{});
    var term: Io.Terminal = .{ .writer = &sink.writer, .mode = .no_color };

    const cases = [_][2][]const u8{
        .{ "5", "5" },
        .{ "-2.5", "-2.5" },
        .{ "true", "true" },
        .{ "null", "null" },
        .{ "hello", "\"hello\"" },
        .{ "hello world", "\"hello world\"" },
        .{ "a: b", "\"a: b\"" },
        .{ "\"5\"", "\"5\"" },
        .{ "'q'", "\"q\"" },
        .{ "see #3", "\"see #3\"" },
        .{ "#3", "\"#3\"" },
        .{ "", "\"\"" },
        .{ "two\nlines", "\"two\\nlines\"" },
        .{ "[1, 2]", "[1,2]" },
        .{ "{a = 1, b = [x]}", "{\"a\":1,\"b\":[\"x\"]}" },
    };
    for (cases) |c|
        try t.expectEqualStrings(c[1], try render(a, try read(a, &term, c[0], .fig), .json, .natural));
}

test "--string and --raw take the argument as it stands" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var sink: std.Io.Writer.Discarding = .init(&.{});
    var term: Io.Terminal = .{ .writer = &sink.writer, .mode = .no_color };

    try t.expectEqualStrings("\"1.10\"", try render(a, try read(a, &term, "1.10", .string), .json, .natural));
    try t.expectEqualStrings("[1,2", try render(a, try read(a, &term, "[1,2", .raw), .json, .natural));
}
