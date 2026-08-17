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
    pub const Printer = toml.Printer;
    pub const default_type: toml.Type = .TOML_1_1;
    pub fn parse(parser: *toml.Parser, input: []const u8, format: toml.Type) !Document {
        return toml.Parser.parse(parser.allocator, input, format);
    }
    pub const print = toml.Printer.print;
    pub const printNode = toml.Printer.printNode;

    pub const name = "toml";
    pub const extensions: []const []const u8 = &.{"toml"};
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    /// 1.0 and 1.1 differ in what the PARSER accepts (newlines and trailing
    /// commas in inline tables, seconds-optional times, `\e`/`\xHH` escapes),
    /// not in what the editor writes, so both dialects answer identically.
    pub fn syntax(t: toml.Type) lang.Syntax {
        _ = t;
        return .{
            .comments = .hash,
            // TOML spells an entry `key = value`, and every path where that
            // matters is delegated to `toml/editor_helper.zig` — so the
            // generic engine never writes a TOML separator. See
            // `Syntax.kv_sep`.
            .kv_sep = null,
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

    // ── Editing hooks ────────────────────────────────────────────────────────
    //
    // Operations this format takes over from the generic splice engine.
    // `Editor` dispatches on PRESENCE — `@hasDecl(Language, "insertKey")` — so
    // declaring one here is the whole of opting in, and every operation not
    // named below runs the generic implementation. Each signature is fixed by
    // the `editor.Editor` method of the same name; see its doc comment.
    //
    // The logic lives in `editor_helper.zig` (which holds this format's editor
    // tests too), not here: this block is the DECLARATION of which operations
    // are overridden, so a reader can see a format's whole answer in one struct
    // without opening the helper.
    const edit = @import("editor_helper.zig");

    /// TOML splits a logical table across scattered `[header]` and dotted-key
    /// lines, so a new entry has to land at the end of the intended table's own
    /// header region — never after a sub-table header, which would silently
    /// reparent it.
    pub const insertKey = edit.tomlInsertKey;

    /// A `[header]` table has no contiguous line span, so the generic
    /// line-based delete would remove its header and orphan the body.
    pub const deleteKeyGuard = edit.tableDeleteGuard;

    /// A block table's node span is its key segment inside the `[header]` (or
    /// its dotted key), not any value text, so the generic splice would rewrite
    /// the table's NAME instead of its body.
    pub const replaceValGuard = edit.tableReplaceGuard;

    // ── Whole-container ops ──────────────────────────────────────────────────
    //
    // The EXCLUSIVE operations: they override nothing, because the generic
    // engine has no counterpart for editing a container assembled from
    // scattered `[header]` regions. Declaring one is still the whole of opting
    // in — `Editor` dispatches on `@hasDecl` here exactly as it does for the
    // hooks above. TOML declares all six; a format that declares none simply
    // has no whole-container surface.

    /// Every region of the table / array-of-tables / AoT element's subtree.
    pub const deleteContainer = edit.deleteTable;

    /// A new `[path]` table, spliced past the parent's whole subtree so no
    /// existing key is reparented.
    pub const insertContainer = edit.insertTable;

    /// TOML alone needs a rename op: the renamed segment appears in every
    /// descendant header (`[a.b]`, `[a.b.c]`, `[[a.b]]`), not just its own.
    pub const renameContainer = edit.renameTable;

    /// The table's scattered fragments, re-emitted contiguously at the
    /// destination; interleaved foreign tables stay put.
    pub const moveContainer = edit.moveTable;

    /// Top-level tables reordered among themselves, each re-emitted
    /// contiguously at the position the earliest currently occupies.
    pub const reorderContainers = edit.reorderTables;

    /// A new `[[header]]` element on the end of an array-of-tables, past every
    /// line of the current last element's subtree.
    pub const appendContainerToSeq = edit.appendTableToArray;
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
