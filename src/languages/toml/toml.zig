const toml = @This();
const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const lang = @import("../manifest.zig");

pub const Parser = @import("parser.zig");
pub const Tokenizer = @import("tokenizer.zig");
pub const Printer = @import("printer.zig");

pub const Type = enum {
    /// TOML 1.0.0 (stable, finalized).
    TOML_1_0,
    /// TOML 1.1.0 (draft): newlines + trailing commas in inline tables,
    /// seconds-optional times, `\e` and `\xHH` string escapes.
    TOML_1_1,
};

pub const Language = struct {
    pub const Type = toml.Type;
    pub const Parser = toml.Parser;
    pub const default_type: toml.Type = .TOML_1_1;
    pub fn parse(parser: *toml.Parser, input: []const u8, format: toml.Type) !Document {
        return toml.Parser.parse(parser.allocator, input, format);
    }
    pub const print = Printer.print;
    pub const printNode = Printer.printNode;

    pub const name = "toml";
    pub const extensions: []const []const u8 = &.{"toml"};
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    /// 1.0 and 1.1 differ in what the PARSER accepts (newlines and trailing
    /// commas in inline tables, seconds-optional times, `\e`/`\xHH` escapes),
    /// not in what the editor writes, so both dialects answer identically.
    pub fn syntax(t: toml.Type) lang.Syntax {
        _ = t;
        return .{
            .comment_style = .hash,
            .line_comment = "#",
            .trailing_comment = "#",
            // TOML spells an entry `key = value`, but every path where that
            // matters is delegated to `toml/editor_helper.zig`; `": "` is
            // what the shared flow/block helpers were already using here.
            .kv_sep = ": ",
            // The dotted-key formats keep the flow `{}` chain as the
            // idiomatic intermediate form — `fig fmt` canonicalizes
            // `a = { b = { c = v }}` back to `a.b.c = v`.
            .empty_map_literal = "{}",
            // A non-flow TOML sequence is an array-of-tables (use
            // `appendTableToArray`), and TOML has no block scalar array — so
            // the generic block-sequence edits refuse with `NotAnInlineArray`.
            .block_seq_editable = false,
        };
    }
};

// Test discovery for the TOML module: importing `toml.zig` (from root.zig) pulls
// in every TOML submodule's tests, so the module owns its own test surface rather
// than root.zig enumerating each file. `editor_helper.zig` holds the TOML editor
// tests; conformance is gated by a build option and stays in root.zig.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_helper.zig");
}
