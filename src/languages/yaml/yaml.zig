const yaml = @This();
const Document = @import("../../document.zig");
const AST = @import("../../ast/ast.zig");
const lang = @import("../manifest.zig");

pub const Parser = @import("parser.zig");
pub const Tokenizer = @import("tokenizer.zig");
pub const Printer = @import("printer.zig");
pub const Materialize = @import("materialize.zig");
pub const Type = enum {
    v1_2_2,
    /// YAML 1.1 (2005). Differs from 1.2 almost entirely in *scalar type
    /// resolution* (the tag repository at yaml.org/type): `yes/no/on/off/y/n`
    /// booleans, leading-zero octal (`0777`) + binary (`0b…`) + sexagesimal
    /// (`190:20:30`) ints, `_` digit separators, mandatory-sign float exponents,
    /// and `!!timestamp` auto-resolution. Structure/syntax is unchanged.
    /// Resolution differences are pinned by the spec fixtures in
    /// `testdata/yaml-1.1/` (see `conformance_1_1.zig`) and implemented by
    /// `scalarKind1_1` in `parser.zig`. Structure/syntax is shared with 1.2.
    v1_1,
};

pub const Language = struct {
    pub const Type = yaml.Type;
    pub const Parser = yaml.Parser;
    pub const default_type: yaml.Type = .v1_2_2;
    pub fn parse(parser: *yaml.Parser, input: []const u8, format: yaml.Type) !Document {
        return yaml.Parser.parse(parser.allocator, input, format);
    }
    pub const print = Printer.print;
    pub const printNode = Printer.printNode;
    /// Collapse the reference layer (aliases/merges/tags/anchors) into a core AST
    /// before handing it to a non-YAML printer. Optional Language decl: callers
    /// gate on `@hasDecl(Lang, "materialize")`.
    pub const materialize = Materialize.materialize;
    pub const TagMode = Materialize.TagMode;

    pub const name = "yaml";
    pub const extensions: []const []const u8 = &.{ "yaml", "yml" };
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    /// 1.1 and 1.2.2 differ only in scalar type RESOLUTION, never in the
    /// syntax the editor splices, so both dialects answer identically. This
    /// is where an editing divergence would land if one ever appeared.
    pub fn syntax(t: yaml.Type) lang.Syntax {
        _ = t;
        return .{
            .comment_style = .hash,
            .line_comment = "#",
            .trailing_comment = "#",
            .kv_sep = ": ",
            // The empty seed rather than `{}` — see `Syntax.empty_map_literal`
            // for why the two are not interchangeable here.
            .empty_map_literal = "",
            // The only editable format whose block mapping has a single-line
            // spelling (`k: v`) that reaches the splice paths.
            .single_line_block_mapping = true,
        };
    }
};

// Test discovery: importing `yaml.zig` (from root.zig) pulls in every YAML
// submodule's tests, so the module owns its own test surface. The conformance
// suite is build-option-gated and stays in root.zig.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("materialize.zig");
    _ = @import("editor_helper.zig");
}
