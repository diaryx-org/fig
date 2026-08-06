const properties = @This();
const Document = @import("../../document.zig");
const lang = @import("../manifest.zig");

pub const Parser = @import("parser.zig");
pub const Tokenizer = @import("tokenizer.zig");
pub const Printer = @import("printer.zig");

pub const Type = enum {
    /// The one dialect this parser accepts — see `tokenizer.zig`'s module doc
    /// for the full grammar (three interchangeable separators, backslash
    /// escapes on both key and value, line continuation, `#`/`!` comments).
    PROPERTIES,
};

pub const Language = struct {
    pub const Type = properties.Type;
    pub const Parser = properties.Parser;
    pub const Printer = properties.Printer;
    pub const default_type: properties.Type = .PROPERTIES;
    pub fn parse(parser: *properties.Parser, input: []const u8, format: properties.Type) !Document {
        return properties.Parser.parse(parser.allocator, input, format);
    }
    pub const print = properties.Printer.print;
    pub const printNode = properties.Printer.printNode;

    pub const name = "properties";
    pub const extensions: []const []const u8 = &.{"properties"};
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    pub fn syntax(t: properties.Type) lang.Syntax {
        _ = t;
        return .{
            // The grammar accepts `#` and `!` as comment leaders; the printer
            // writes `#`, so that is what the editor scans for and inserts.
            .comment_style = .hash,
            .line_comment = "#",
            .trailing_comment = "#",
            // Three separators are legal on read (`=`, `:`, space); the
            // printer always writes a bare `=`.
            .kv_sep = "=",
            // Flat, same reasoning as dotenv.
            .empty_map_literal = "{}",
        };
    }
};

// Test discovery: importing `properties.zig` (from root.zig) pulls in every
// `.properties` submodule's tests, so the module owns its own test surface.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
}
