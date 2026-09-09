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

    /// What `languages/harness.zig` round-trips and edits: a root key, a
    /// `[table]` (the section shape the parser records regions for), an
    /// array, and an inline table.
    pub const samples: []const []const u8 = &.{
        "a = 1\nb = [1, 2]\n\n[s]\nk = \"v\"\nt = { x = 1 }\n",
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
            // `key = value`, in a block table and an inline one alike; an
            // empty inline table takes its first entry padded, `{ k = v }`.
            .kv_sep = " = ",
            .flow_map_pad = " ",
            // A key is bare when it can be, else basic-quoted — for an
            // inserted key, a header path's segments and a renamed table's
            // leaf alike.
            .key_style = .bare_or_quoted,
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
            // `[a.b]` opens a table, `[[a.b]]` an element of an array of
            // tables; an index segment is implied and left out.
            .section_header = .{ .open = "[", .close = "]", .seq_open = "[[", .seq_close = "]]" },
        };
    }

    // ── Editing ──────────────────────────────────────────────────────────────
    //
    // No hooks. Every edit is the generic engine's, driven by `syntax` above
    // and by what `parser.zig` records: a table's header lines
    // (`Document.node_regions`, which the whole-container delete, move and
    // reorder gather) and every place a table's name is spelled
    // (`Document.node_mentions`). The engine keeps an inserted entry inside
    // the intended table by skipping children that sit under a header of
    // their own, spells a new `[a.b]` or `[[a.b]]` from `section_header`,
    // and renames a table by rewriting each mention — its own header,
    // every descendant header sharing the prefix, every dotted line — which
    // is what `toml/editor_helper.zig` used to do by scanning the source.
    // That file now holds this format's editor tests.
    //
    // The line-splice ops refuse a section node in this format's vocabulary
    // (`CannotDeleteTable`, …) and point at the whole-container ops.
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
