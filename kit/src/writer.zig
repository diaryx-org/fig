//! A printer's output, which knows the options it was given: `nl` and
//! `indent` write nothing when `pretty` is off, and a level of indentation
//! is `indent` spaces; `comments` writes a node's comments as whole lines
//! after a leader. The writer every kit has — the JavaScript kit's
//! `Writer`, Rust's `fig::kit::print::Writer` — over a `std.Io.Writer`.

const Writer = @This();
const std = @import("std");
const AST = @import("ast/ast.zig");
const Error = std.Io.Writer.Error;

out: *std.Io.Writer,
pretty: bool = true,
/// Spaces per level of indentation.
indent_width: usize = 2,

pub fn init(out: *std.Io.Writer, pretty: bool, indent_width: usize) Writer {
    return .{ .out = out, .pretty = pretty, .indent_width = indent_width };
}

pub fn put(self: Writer, s: []const u8) Error!void {
    try self.out.writeAll(s);
}

/// End the line, when pretty.
pub fn nl(self: Writer) Error!void {
    if (self.pretty) try self.out.writeByte('\n');
}

/// Indent `depth` levels, when pretty.
pub fn indent(self: Writer, depth: usize) Error!void {
    if (self.pretty) try self.out.splatByteAll(' ', depth * self.indent_width);
}

/// Indent, write `s`, end the line.
pub fn line(self: Writer, depth: usize, s: []const u8) Error!void {
    try self.indent(depth);
    try self.put(s);
    try self.nl();
}

/// Write `comments` as whole lines, each line of each comment's text on
/// its own and trimmed, after `leader` and a space (`# text`, or the bare
/// leader for an empty line), indented `depth` levels. Answers whether
/// anything was written.
pub fn comments(self: Writer, cs: []const AST.Comment, leader: []const u8, depth: usize) Error!bool {
    var wrote = false;
    for (cs) |c| {
        var it = std.mem.splitScalar(u8, c.text, '\n');
        while (it.next()) |l| {
            const t = std.mem.trim(u8, l, " \t");
            try self.indent(depth);
            try self.put(leader);
            if (t.len > 0) {
                try self.out.writeByte(' ');
                try self.put(t);
            }
            try self.out.writeByte('\n');
            wrote = true;
        }
    }
    return wrote;
}

test "the writer indents and writes comment lines" {
    var buf: std.Io.Writer.Allocating = .init(std.testing.allocator);
    defer buf.deinit();
    const w = init(&buf.writer, true, 2);
    _ = try w.comments(&.{.{ .text = "two\n lines " }}, "#", 1);
    try w.line(1, "a = 1");
    try std.testing.expectEqualStrings("  # two\n  # lines\n  a = 1\n", buf.written());
    var flat: std.Io.Writer.Allocating = .init(std.testing.allocator);
    defer flat.deinit();
    const f = init(&flat.writer, false, 2);
    try f.line(3, "x");
    try std.testing.expectEqualStrings("x", flat.written());
}
