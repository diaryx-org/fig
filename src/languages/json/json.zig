const json = @This();
const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const lang = @import("../manifest.zig");

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

    pub const name = "json";
    pub const extensions: []const []const u8 = &.{ "json", "jsonc", "json5" };
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    /// The one manifest in tree whose answer genuinely varies by dialect, and
    /// therefore the reason `syntax` is a function of `Type` at all: strict
    /// JSON has no comment syntax, JSONC and JSON5 do. The editor's splices
    /// are reparsed under the dialect it is holding, so writing `//` into a
    /// document being read as strict JSON would produce source that no longer
    /// parses — the comment ops return `CommentsUnsupported` there instead.
    ///
    /// `style` stays `.slashes` for all three: it selects the scanner for
    /// OWNED comment blocks, and in strict JSON no `//` line can exist for
    /// that scanner to find, so the choice is unobservable rather than wrong.
    /// That split — one scanner, a marker that varies — is why this is written
    /// out rather than taking the `Comments.slashes` preset.
    pub fn syntax(t: json.Type) lang.Syntax {
        const marker: ?[]const u8 = if (t == .JSON) null else "//";
        return .{
            .comments = .{ .style = .slashes, .line = marker, .trailing = marker },
            .kv_sep = ": ",
            // Strict-JSON-family keys must be quoted and escaped (`b` -> `"b"`).
            .key_style = .json_quoted,
            .empty_map_literal = "{}",
        };
    }
};

// Test discovery: importing `json.zig` (from root.zig) pulls in every JSON
// submodule's tests, so the module owns its own test surface. `editor_helper.zig`
// holds the JSON/JSON5 editor tests; conformance suites are build-option-gated
// and stay in root.zig.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_helper.zig");
}
