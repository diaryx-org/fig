const ini = @This();
const Document = @import("../../document.zig");
const lang = @import("../manifest.zig");

pub const Parser = @import("parser.zig");
pub const Tokenizer = @import("tokenizer.zig");
pub const Printer = @import("printer.zig");

pub const Type = enum {
    /// The one dialect this parser accepts: `=`-separated `key = value`
    /// lines, `[section]` headers, `;`/`#` full-line comments. See
    /// `tokenizer.zig`'s module doc for exactly what's deliberately excluded
    /// from the many incompatible things "INI" means in the wild.
    INI,
};

pub const Language = struct {
    pub const Type = ini.Type;
    pub const Parser = ini.Parser;
    pub const default_type: ini.Type = .INI;
    pub fn parse(parser: *ini.Parser, input: []const u8, format: ini.Type) !Document {
        return ini.Parser.parse(parser.allocator, input, format);
    }
    pub const print = Printer.print;
    pub const printNode = Printer.printNode;

    pub const name = "ini";
    pub const extensions: []const []const u8 = &.{"ini"};
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    pub fn syntax(t: ini.Type) lang.Syntax {
        _ = t;
        return .{
            // The printer accepts a leading `#` on read but always WRITES
            // `;`, so `;` is what the editor's inserts and scans use.
            .comment_style = .semicolon,
            .line_comment = ";",
            // No same-line trailing comment syntax at all: a `;`/`#` after a
            // value on the SAME line is literal value text (see `parser.zig`,
            // "a value runs to end of line"). Splicing one in would corrupt
            // the value on reread, so trailing ops are refused.
            .trailing_comment = null,
            .kv_sep = " = ",
            // No literal spelling for an empty nested mapping — `{}` in INI
            // is the two-character STRING `{}`, not a container — so `set`
            // cannot auto-vivify a missing ancestor here at all.
            .empty_map_literal = null,
        };
    }
};

// Test discovery: importing `ini.zig` (from root.zig) pulls in every INI
// submodule's tests, so the module owns its own test surface. `editor_helper.zig`
// holds the INI editor (section-nesting) tests, mirroring TOML's split.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_helper.zig");
}
