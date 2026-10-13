const zon = @This();
const std = @import("std");
const Document = @import("kit").Document;
const Table = @import("kit").table.Table;
const AST = @import("kit").AST;
const lang = @import("kit").manifest;

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
    pub const Printer = zon.Printer;
    pub const default_type: zon.Type = .ZON;
    /// Write `input`'s rows into `out`, the table fig hands a parse.
    pub fn parseTable(parser: *zon.Parser, input: []const u8, format: zon.Type, out: *Table) !void {
        _ = parser;
        return zon.Parser.parseInto(out, input, format);
    }
    /// The document fig would adopt from a parse.
    pub fn parse(parser: *zon.Parser, input: []const u8, format: zon.Type) !Document {
        return zon.Parser.parse(parser.allocator, input, format);
    }
    pub const print = zon.Printer.print;
    pub const printNode = zon.Printer.printNode;

    pub const name = "zon";
    pub const extensions: []const []const u8 = &.{"zon"};
    pub const caps: lang.Caps = .{
        .read = true,
        .edit = true,
        .serialize = true,
        // `null`, `.enum_literal` and `'c'` are native Zig syntax; datetimes,
        // non-finite floats and plist's date/data are enveloped.
        .lossless = .{ .null = true, .enum_literal = true, .char_literal = true },
    };

    /// What fig's `src/contract/harness.zig` round-trips and edits: a struct
    /// literal with a nested struct and a tuple.
    pub const samples: []const []const u8 = &.{
        ".{ .a = 1, .b = .{ .c = \"d\" }, .e = .{ 1, 2 } }\n",
    };

    pub const dialects: []const lang.Dialect(@This()) = &.{.{
        .name = "zon",
        .prints = .{ .holds = Language.caps.lossless.?, .char_as = "number", .comments = .pretty },
        .splice = .literal,
        .empty_doc_seed = ".{}\n",
    }};

    /// A key's name as a struct field: `.name`, or `.@"has space"` when
    /// it is not a bare identifier.
    pub fn renderKeyName(_: zon.Type, allocator: std.mem.Allocator, out: *std.ArrayList(u8), r: lang.RenderRequest) !void {
        var w = std.Io.Writer.Allocating.init(allocator);
        defer w.deinit();
        try zon.Printer.writeDotName(&w.writer, r.key);
        try out.appendSlice(allocator, w.written());
    }

    /// The format that most exercises the manifest's "push traits down into
    /// parameters" rule: three of these fields exist because ZON's surface
    /// syntax differs from the block-mapping formats, not because ZON needs
    /// bespoke logic. Any future format whose keys carry a sigil, or which
    /// has no keyless top-level mapping, reuses them unchanged.
    pub fn syntax(t: zon.Type) lang.Syntax {
        _ = t;
        return .{
            // ZON follows Zig: `//` lines, and no block comment.
            .comments = .slashes,
            // Struct-field syntax.
            .kv_sep = " = ",
            // A struct field's key SPAN starts at the bare identifier, so the
            // leading `.` has to be absorbed explicitly on a flow-entry
            // delete or the splice strands it beside a survivor.
            .key_sigil = '.',
            .empty_map_literal = ".{}",
            // The same `.{}`: ZON's empty tuple is its empty struct.
            .empty_seq_literal = ".{}",
            // No bare `key: value` document form (unlike YAML/JSON5, whose
            // root can be a keyless top-level mapping), so a null promotes in
            // place to `.{ key = value }` — root or nested, no distinction.
            .bare_document_mapping = false,
            .flow_map_open = ".{",
            .flow_map_close = "}",
        };
    }
};

// The package's own tests (`zig build test`): every file's. Its editor
// tests need fig itself, which a language cannot depend on, so they are
// `tests/editor.zig`, which fig's build compiles.
test {
    _ = @import("parser.zig");
    _ = @import("printer.zig");
}
