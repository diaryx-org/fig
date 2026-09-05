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
    /// Parse straight to a core AST with no `Document` around it — what
    /// `deserialize.zig` maps onto a Zig type. Optional `Language` decl,
    /// required exactly of a language with a `deserializable` dialect row.
    pub const parseAbstract = toml.Parser.parseAbstract;

    pub const name = "toml";
    pub const extensions: []const []const u8 = &.{"toml"};
    pub const caps: lang.Caps = .{
        .read = true,
        .edit = true,
        .serialize = true,
        // The four datetimes and `inf`/`nan` floats are native; `null` is
        // not — TOML is the one typed format without one, which makes a
        // `null` the one value the lossy path drops outright rather than
        // degrades. Enum/char literals and plist's date/data are enveloped.
        .lossless = .{
            .offset_datetime = true,
            .local_datetime = true,
            .local_date = true,
            .local_time = true,
            .number_special = true,
        },
    };

    pub const dialects: []const lang.Dialect(@This()) = &.{.{
        .name = "toml",
        .abi_value = 4,
        // First of the `key = value` grammars. fig, INI and dotenv all accept
        // plain TOML-shaped content, so TOML gets first claim on it and they
        // win only what TOML rejects.
        .sniff_rank = 5,
        .deserializable = true,
        .splice = .literal,
        .empty_doc_seed = "",
        .specs = &.{
            .{ .name = "1.0", .dialect = .TOML_1_0 },
            .{ .name = "1.0.0", .dialect = .TOML_1_0 },
            .{ .name = "1.1", .dialect = .TOML_1_1 },
            .{ .name = "1.1.0", .dialect = .TOML_1_1 },
        },
        .embed = .{
            .fence_tag = "toml",
            .frontmatter = "---toml",
            .script_mime = "application/toml",
            .code_class = "language-toml",
        },
    }};

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
            // A section format: a `[table]`, `[[array]]` or dotted table is
            // assembled from scattered lines, which `parser.zig` records in
            // `Document.node_regions`. Its refusals say "table".
            .section_noun = .table,
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

    // No delete/replace/move/reorder guards: the engine's section rule covers
    // them. A block table is a section node (`Document.node_regions`), and a
    // section node cannot be line-spliced — `deleteKey`, `replaceValAtPath`,
    // `moveKey` and `reorderKeys` refuse it in this format's vocabulary
    // (`CannotDeleteTable`, …) and point at the whole-container ops.

    /// A table's name is written once per `[header]`/dotted line that mentions
    /// it, but only the first has a key node — so the generic one-span splice
    /// renames that mention and leaves the rest behind, splitting the table.
    /// Routes a block table to `renameContainer`'s multi-mention rewrite.
    pub const replaceKeyAtPath = edit.tomlReplaceKey;

    // ── Whole-container ops ──────────────────────────────────────────────────
    //
    // `deleteContainer`, `moveContainer` and `reorderContainers` are GENERIC:
    // `editor.zig` derives a table's scattered regions from `Document.
    // node_regions` and needs nothing from here. The three below are the ops
    // that have to SPELL something TOML — a `[header]` line, or every mention
    // of a table's name — and stay hooks; `Editor` dispatches on `@hasDecl`
    // for them exactly as for the hooks above.

    /// A new `[path]` table, spliced past the parent's whole subtree so no
    /// existing key is reparented.
    pub const insertContainer = edit.insertTable;

    /// TOML alone needs a rename op: the renamed segment appears in every
    /// descendant header (`[a.b]`, `[a.b.c]`, `[[a.b]]`) and every dotted line
    /// that spells it, not just its own key node.
    pub const renameContainer = edit.renameTable;

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
