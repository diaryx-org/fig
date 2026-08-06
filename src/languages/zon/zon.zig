const zon = @This();
const Document = @import("../../document.zig");
const AST = @import("../../ast/ast.zig");
const lang = @import("../manifest.zig");

pub const Parser = @import("parser.zig");
pub const Printer = @import("printer.zig");

pub const Type = enum {
    /// ZON as accepted by the Zig 0.16 `std.zig` parser. There is no versioned
    /// ZON spec; the grammar tracks whatever the pinned compiler accepts.
    ZON,
};

pub const Language = struct {
    pub const Type = zon.Type;
    pub const Parser = zon.Parser;
    pub const default_type: zon.Type = .ZON;
    pub fn parse(parser: *zon.Parser, input: []const u8, format: zon.Type) !Document {
        return zon.Parser.parse(parser.allocator, input, format);
    }
    pub const print = Printer.print;
    pub const printNode = Printer.printNode;

    pub const name = "zon";
    pub const extensions: []const []const u8 = &.{"zon"};
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    /// The format that most exercises the manifest's "push traits down into
    /// parameters" rule: three of these fields exist because ZON's surface
    /// syntax differs from the block-mapping formats, not because ZON needs
    /// bespoke logic. Any future format whose keys carry a sigil, or which
    /// has no keyless top-level mapping, reuses them unchanged.
    pub fn syntax(t: zon.Type) lang.Syntax {
        _ = t;
        return .{
            // ZON follows Zig: `//`, and the owned-block scanner that also
            // walks `/* ... */` as a unit.
            .comment_style = .slashes,
            .line_comment = "//",
            .trailing_comment = "//",
            // Struct-field syntax.
            .kv_sep = " = ",
            .key_style = .zon_field,
            // A struct field's key SPAN starts at the bare identifier, so the
            // leading `.` has to be absorbed explicitly on a flow-entry
            // delete or the splice strands it beside a survivor.
            .key_sigil = '.',
            .empty_map_literal = ".{}",
            // No bare `key: value` document form (unlike YAML/JSON5, whose
            // root can be a keyless top-level mapping), so a null promotes in
            // place to `.{ key = value }` — root or nested, no distinction.
            .bare_document_mapping = false,
            .flow_map_open = ".{",
            .flow_map_close = "}",
        };
    }
};

// Test discovery: importing `zon.zig` (from root.zig) pulls in every ZON
// submodule's tests, so the module owns its own test surface.
test {
    _ = @import("parser.zig");
    _ = @import("printer.zig");
}
