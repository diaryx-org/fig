const json = @This();
const std = @import("std");
const AST = @import("kit").AST;
const Document = @import("kit").Document;
const Table = @import("kit").table.Table;
const lang = @import("kit").manifest;
const json_string = @import("kit").json_string;
const abi = @import("kit").abi;
const contract = @import("kit").contract;

pub const Parser = @import("parser.zig");
pub const Tokenizer = @import("tokenizer.zig");
pub const Printer = @import("printer.zig");
pub const Type = enum {
    JSON,
    JSONC,
    JSON5,
};

pub const Language = struct {
    pub const Type = json.Type;
    pub const Parser = json.Parser;
    pub const Printer = json.Printer;
    pub const default_type: json.Type = .JSON;
    /// Write `input`'s rows into `out`, the table fig hands a parse.
    pub fn parseTable(parser: *json.Parser, input: []const u8, format: json.Type, out: *Table) !void {
        _ = parser;
        return json.Parser.parseInto(out, input, format);
    }
    /// The document fig would adopt from a parse.
    pub fn parse(parser: *json.Parser, input: []const u8, format: json.Type) !Document {
        return json.Parser.parse(parser.allocator, input, format);
    }
    pub const print = json.Printer.print;
    pub const printNode = json.Printer.printNode;

    pub const name = "json";
    pub const extensions: []const []const u8 = &.{ "json", "jsonc", "json5" };
    pub const caps: lang.Caps = .{
        .read = true,
        .edit = true,
        .serialize = true,
        // Beyond the core kinds, JSON holds a `null` and nothing else: every
        // extended scalar rides in a `$fig` envelope. Declared for the
        // strict dialect and shared by JSONC/JSON5 — so JSON5's native
        // `Infinity`/`NaN` are enveloped too, which is conservative but still
        // lossless (see `Caps.lossless`).
        .lossless = .{ .null = true },
    };

    /// What fig's `src/contract/harness.zig` round-trips and edits: a mapping,
    /// a sequence, and the scalar kinds the grammar has.
    pub const samples: []const []const u8 = &.{
        "{\"a\": 1, \"b\": [true, null, \"s\"], \"c\": {\"d\": 2.5}}\n",
        "[]\n",
    };

    /// The three user-facing dialects this one module serves. The only
    /// language with more than one: strictness is the format NAME here
    /// (json/jsonc/json5 select a `Type` each), where TOML and YAML select
    /// theirs with `--spec`. Registry order within the language is this order.
    pub const dialects: []const lang.Dialect(@This()) = &.{
        .{
            .name = "json",
            .prints = .{ .holds = .{ .null = true }, .char_as = "number", .number_special_as = "quoted string", .comments = .never },
            .dialect = .JSON,
            .splice = .escaped,
            .empty_doc_seed = "{}\n",
        },
        .{
            .name = "jsonc",
            .prints = .{ .holds = .{ .null = true }, .char_as = "number", .number_special_as = "quoted string", .comments = .pretty },
            .dialect = .JSONC,
            .splice = .escaped,
            .empty_doc_seed = "{}\n",
            .print_name = "printc",
            .print_node_name = "printNodec",
        },
        .{
            .name = "json5",
            .prints = .{ .holds = .{ .null = true, .number_special = true }, .char_as = "number", .comments = .pretty },
            .dialect = .JSON5,
            .splice = .escaped,
            .empty_doc_seed = "{}\n",
            .print_name = "print5",
            .print_node_name = "printNode5",
        },
    };

    /// A key's name as JSON writes it: quoted and escaped (`b` -> `"b"`),
    /// which strict JSON requires and JSONC and JSON5 take.
    pub fn renderKeyName(_: json.Type, allocator: std.mem.Allocator, out: *std.ArrayList(u8), r: lang.RenderRequest) !void {
        var w = std.Io.Writer.Allocating.init(allocator);
        defer w.deinit();
        try json_string.writeQuoted(&w.writer, r.key);
        try out.appendSlice(allocator, w.written());
    }

    /// The one manifest in tree whose answer genuinely varies by dialect, and
    /// therefore the reason `syntax` is a function of `Type` at all: strict
    /// JSON has no comment syntax, JSONC and JSON5 have `//` lines and
    /// `/* */` blocks. The editor's splices are reparsed under the dialect it
    /// is holding, so writing `//` into a document being read as strict JSON
    /// would produce source that no longer parses — the comment ops return
    /// `CommentsUnsupported` there instead.
    pub fn syntax(t: json.Type) lang.Syntax {
        const marker: ?lang.CommentDelimiter = if (t == .JSON) null else .{ .open = "//" };
        const block: ?lang.CommentDelimiter = if (t == .JSON) null else .{ .open = "/*", .close = "*/" };
        return .{
            .comments = .{ .line = marker, .trailing = marker, .block = block },
            .kv_sep = ": ",
            .empty_map_literal = "{}",
        };
    }
};

// The package's own tests (`zig build test`): every file's. Its editor
// tests need fig itself, which a language cannot depend on, so they are
// `tests/editor.zig`, which fig's build compiles.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
}

// What the TypeScript binding does with a language module's parse: copies
// its table's lists into the table fig handed it (`kit/wasm_language.zig`).
test "a module's table, copied byte for byte into a fresh one, is adopted" {
    const V = contract.VTable(json.Language, "");
    const src = "{\"a\": 1, \"b\": [true, null, \"s\"]}";
    var a = Table.init(std.testing.allocator, src);
    defer a.deinit();
    var err: abi.ErrorInfo = .empty;
    try std.testing.expectEqual(@as(c_int, 0), V.vtable.parse(null, "json", abi.Str.of(src), &a, &err));
    var b = Table.init(std.testing.allocator, src);
    defer b.deinit();
    try b.rows.appendSlice(std.testing.allocator, a.rows.items);
    try b.strings.appendSlice(std.testing.allocator, a.strings.items);
    try b.seps.appendSlice(std.testing.allocator, a.seps.items);
    try b.comments.appendSlice(std.testing.allocator, a.comments.items);
    var ast = try AST.adopt(std.testing.allocator, src, &b, .required);
    ast.deinit();
}
