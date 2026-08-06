//! The fig authoring dialect: a memorable, typeable, whitespace-insensitive
//! surface over the AST (`src/ast/ast.zig`), parsed by `fig fmt` into the same
//! tree the lossless `canonical` form encodes. It is NOT the oracle — it is
//! allowed to be lossy at the edges (the canonical form and `$fig-envelope`
//! are the faithful fallback). See `DESIGN.md` (this directory) for the full
//! spec.
//!
//! Single grammar (no versions to select), so `Type` has one member, mirroring
//! ZON's `Type = enum { ZON }` pattern.

const fig = @This();
const Document = @import("../../document.zig");
const lang = @import("../manifest.zig");

pub const Parser = @import("parser.zig");
pub const Printer = @import("printer.zig");

pub const Type = enum {
    Fig,
};

pub const Language = struct {
    pub const Type = fig.Type;
    pub const Parser = fig.Parser;
    pub const default_type: fig.Type = .Fig;
    pub fn parse(parser: *fig.Parser, input: []const u8, format: fig.Type) !Document {
        return fig.Parser.parse(parser.allocator, input, format);
    }
    pub const print = Printer.print;
    pub const printNode = Printer.printNode;

    pub const name = "fig";
    /// `.figl` is the authoring dialect's canonical extension; `.fig` is still
    /// accepted for back-compat. (The canonical form deliberately owns no
    /// extension — select it with `--input canonical`.)
    pub const extensions: []const []const u8 = &.{ "figl", "fig" };
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    pub fn syntax(t: fig.Type) lang.Syntax {
        _ = t;
        return .{
            .comment_style = .hash,
            .line_comment = "#",
            .trailing_comment = "#",
            // fig spells an entry `key = value`, but as with TOML the paths
            // where that matters delegate to `fig/editor_helper.zig`; `": "`
            // is what the shared helpers were already using here.
            .kv_sep = ": ",
            // A dotted-key format: the flow chain is the idiomatic
            // intermediate form, and `fig fmt` canonicalizes it back.
            .empty_map_literal = "{}",
            // The `>` marker run that opens a line is section depth, not
            // whitespace — a comment inserted above a line must repeat it or
            // it detaches. See `Syntax.structural_indent`.
            .structural_indent = true,
        };
    }
};

// Test discovery: importing `fig.zig` (from root.zig) pulls in every fig
// submodule's tests, so the module owns its own test surface.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_helper.zig");
}
