const dotenv = @This();
const Document = @import("../../document.zig");
const lang = @import("../manifest.zig");

pub const Parser = @import("parser.zig");
pub const Tokenizer = @import("tokenizer.zig");
pub const Printer = @import("printer.zig");

pub const Type = enum {
    /// The one dialect this parser accepts — see `tokenizer.zig`'s module doc
    /// for exactly what it does and doesn't accept (bash-identifier keys,
    /// optional `export`, real `"`/`'` quoting, no `$VAR` interpolation).
    DOTENV,
};

pub const Language = struct {
    pub const Type = dotenv.Type;
    pub const Parser = dotenv.Parser;
    pub const default_type: dotenv.Type = .DOTENV;
    pub fn parse(parser: *dotenv.Parser, input: []const u8, format: dotenv.Type) !Document {
        return dotenv.Parser.parse(parser.allocator, input, format);
    }
    pub const print = Printer.print;
    pub const printNode = Printer.printNode;

    pub const name = "dotenv";
    /// A dotenv file is conventionally named exactly `.env`, whose last-dot
    /// "extension" is the literal `env`. (`.env.production` is not recognized
    /// by extension — pass `--input dotenv`.)
    pub const extensions: []const []const u8 = &.{"env"};
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    pub fn syntax(t: dotenv.Type) lang.Syntax {
        _ = t;
        return .{
            .comment_style = .hash,
            .line_comment = "#",
            .trailing_comment = "#",
            // A bare `=` with no surrounding spaces — see `printer.zig`.
            .kv_sep = "=",
            // Flat: no nesting to vivify into, but `{}` is still the literal
            // the generic seed would splice, and the format has no nested
            // path for it to seed. Kept as the shared default rather than
            // null, which would change the error a nested `set` reports.
            .empty_map_literal = "{}",
        };
    }
};

// Test discovery: importing `dotenv.zig` (from root.zig) pulls in every
// dotenv submodule's tests, so the module owns its own test surface.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
}
