const json = @This();
const std = @import("std");
const AST = @import("../../kit/ast/ast.zig");
const Document = @import("../../kit/document.zig");
const lang = @import("../../kit/manifest.zig");
const json_string = @import("../../kit/json_string.zig");

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
    pub fn parse(parser: *json.Parser, input: []const u8, format: json.Type) !Document {
        return json.Parser.parse(parser.allocator, input, format);
    }
    pub const print = json.Printer.print;
    pub const printNode = json.Printer.printNode;
    /// Parse straight to a core AST with no `Document` around it — what
    /// `deserialize.zig` maps onto a Zig type. Optional `Language` decl,
    /// required exactly of a language with a `deserializable` dialect row.
    pub const parseAbstract = json.Parser.parseAbstract;

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

    /// What `languages/harness.zig` round-trips and edits: a mapping, a
    /// sequence, and the scalar kinds the grammar has.
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
            .abi_value = 1,
            // Strictest grammar of all, so it goes first: nothing JSON accepts is
            // ambiguous with a looser format's reading of it.
            .sniff_rank = 0,
            .deserializable = true,
            .splice = .escaped,
            .empty_doc_seed = "{}\n",
            .embed = .{
                .fence_tag = "json",
                .frontmatter = "---json",
                .script_mime = "application/json",
                .script_mime_aliases = &.{"application/ld+json"},
                .code_class = "language-json",
            },
        },
        .{
            .name = "jsonc",
            .prints = .{ .holds = .{ .null = true }, .char_as = "number", .number_special_as = "quoted string", .comments = .pretty },
            .dialect = .JSONC,
            .abi_value = 2,
            // The one dialect `detect` never sniffs: plain JSON and JSON5
            // already claim everything JSONC accepts that they can parse, so
            // sniffing it would only ever mis-attribute a comment-free
            // document.
            .sniff_rank = null,
            .deserializable = true,
            .splice = .escaped,
            .empty_doc_seed = "{}\n",
            .print_name = "printc",
            .print_node_name = "printNodec",
        },
        .{
            .name = "json5",
            .prints = .{ .holds = .{ .null = true, .number_special = true }, .char_as = "number", .comments = .pretty },
            .dialect = .JSON5,
            // 7, not 3: JSON5 was added to the C ABI after XML (6), and a
            // released value is appended rather than inserted — the reason
            // the C enum's numbering is not its order.
            .abi_value = 7,
            // Right after plain JSON: a superset of it, and still stricter than
            // everything below.
            .sniff_rank = 1,
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

// Test discovery: importing `json.zig` (from root.zig) pulls in every JSON
// submodule's tests, so the module owns its own test surface. `editor_test.zig`
// holds the JSON/JSON5 editor tests; conformance suites are build-option-gated
// and stay in root.zig.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_test.zig");
}
