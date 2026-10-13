const properties = @This();
const Document = @import("kit").Document;
const Table = @import("kit").table.Table;
const lang = @import("kit").manifest;

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
    /// Write `input`'s rows into `out`, the table fig hands a parse.
    pub fn parseTable(parser: *properties.Parser, input: []const u8, format: properties.Type, out: *Table) !void {
        _ = parser;
        return properties.Parser.parseInto(out, input, format);
    }
    /// The document fig would adopt from a parse.
    pub fn parse(parser: *properties.Parser, input: []const u8, format: properties.Type) !Document {
        return properties.Parser.parse(parser.allocator, input, format);
    }
    pub const print = properties.Printer.print;
    pub const printNode = properties.Printer.printNode;

    pub const name = "properties";
    pub const extensions: []const []const u8 = &.{"properties"};
    /// Flat, like dotenv: as much a `Hashtable<String, String>` as dotenv is
    /// a flat environment map.
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true, .max_mapping_depth = 0 };

    /// What fig's `src/contract/harness.zig` round-trips and edits: the three
    /// separators the grammar accepts.
    pub const samples: []const []const u8 = &.{
        "a=1\nb: two\nc three\n",
    };

    /// Flat and untyped, the same representational limits as dotenv.
    pub const dialects: []const lang.Dialect(@This()) = &.{.{
        .name = "properties",
        .prints = .{ .holds = .{}, .typed_scalars = false, .char_as = "number" },
        .splice = .raw,
        .empty_doc_seed = "",
    }};

    pub fn syntax(t: properties.Type) lang.Syntax {
        _ = t;
        return .{
            // The grammar accepts `#` and `!` as comment leaders; the printer
            // writes `#`, so that is what the editor scans for and inserts.
            // As in INI, there is no trailing comment: a `#` after a value is
            // value text (`parser.zig`), so a spliced one would change the
            // value on reread, and trailing ops are refused.
            .comments = .{ .line = .{ .open = "#" }, .trailing = null },
            // Three separators are legal on read (`=`, `:`, space); the
            // printer always writes a bare `=`.
            .kv_sep = "=",
            // Flat, same reasoning as dotenv.
            .empty_map_literal = "{}",
            .flow_containers = false,
            // A `\` before a line ending carries the line on (`tokenizer.zig`),
            // so a line written after `k=v\` would be read as the rest of
            // `k`'s value.
            .line_continuation = '\\',
        };
    }
};

// The package's own tests (`zig build test`): every file's.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
}
