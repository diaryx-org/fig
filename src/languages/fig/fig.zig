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
    pub const Printer = fig.Printer;
    pub const default_type: fig.Type = .Fig;
    pub fn parse(parser: *fig.Parser, input: []const u8, format: fig.Type) !Document {
        return fig.Parser.parse(parser.allocator, input, format);
    }
    pub const print = fig.Printer.print;
    pub const printNode = fig.Printer.printNode;

    pub const name = "fig";
    /// `.figl` is the authoring dialect's canonical extension; `.fig` is still
    /// accepted for back-compat. (The canonical form deliberately owns no
    /// extension — select it with `--input canonical`.)
    pub const extensions: []const []const u8 = &.{ "figl", "fig" };
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    /// What `languages/harness.zig` round-trips and edits: a root key, a
    /// block container (the section shape the parser records regions for)
    /// with a `>` child, and a list.
    pub const samples: []const []const u8 = &.{
        "a = 1\ndatabase\n> host = localhost\n> port = 5432\n",
    };

    /// The native authoring dialect (`DESIGN.md`): read, written and edited
    /// by every surface.
    pub const dialects: []const lang.Dialect(@This()) = &.{.{
        .name = "fig",
        // 8: appended to the C ABI after json5 (7).
        .abi_value = 8,
        // Right after TOML and before INI/YAML, not last: fig overlaps TOML
        // heavily (both accept plain `key = value`), so it is tried only after
        // TOML has had first claim — a plain TOML-shaped document still
        // resolves to `toml`, and fig wins on content TOML can't parse (its
        // `>`/`*`/`+`/`[]` structural markers) or that is otherwise
        // TOML-invalid. It cannot go later: YAML is so permissive (a bare line
        // is a valid plain scalar) that almost anything falls through to it,
        // which would starve fig (and INI) of a turn.
        .sniff_rank = 6,
        .splice = .literal,
        .empty_doc_seed = "",
        .embed = .{
            .fence_tag = "fig",
            .fence_aliases = &.{"figl"},
            .frontmatter = "---fig",
            .script_mime = "application/figl",
            .script_mime_aliases = &.{"application/fig"},
            // `language-figl`, not `language-fig`: the class token and the
            // fence tag genuinely differ in `embed.zig`.
            .code_class = "language-figl",
        },
    }};

    pub fn syntax(t: fig.Type) lang.Syntax {
        _ = t;
        return .{
            .comments = .hash,
            // fig spells an entry `key = value`, but `insertKey` is hooked and
            // picks the separator from the object it is inserting into — a
            // flow object is `=`-mode or `:`-mode (JSON-embedded) and may not
            // mix — so there is no one answer for the generic engine to write,
            // and no generic path left that would read it. See `Syntax.kv_sep`.
            .kv_sep = null,
            // A dotted-key format: the flow chain is the idiomatic
            // intermediate form, and `fig fmt` canonicalizes it back.
            .empty_map_literal = "{}",
            // The `>` marker run that opens a line is section depth, not
            // whitespace — a comment inserted above a line must repeat it or
            // it detaches. See `Syntax.structural_indent`.
            .structural_indent = true,
            // One nesting level is one marker cell; an element line is the
            // `>` run of its container plus `* `. Both are copied by the
            // engine's prefix-bytes policy, so a glued `>>*` file stays glued.
            .indent_unit = "> ",
            .seq_item_marker = "* ",
            // A section format: a block container may be re-entered and
            // scattered, and `parser.zig` records every block container's
            // header lines in `Document.node_regions`. Its refusals say
            // "container".
            .section_noun = .container,
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

    /// A block insert copies the anchor line's `>` marker prefix (section depth
    /// is load-bearing, see `Syntax.structural_indent`) and lands after the
    /// last child's full extent, which stays correct for a re-entered or
    /// scattered container.
    pub const insertKey = edit.figInsertKey;

    // No delete guard: the engine's section rule covers it. Every block
    // container is a section node (`Document.node_regions`), and a section
    // node cannot be line-spliced — `deleteKey`, `moveKey` and `reorderKeys`
    // refuse it (`CannotDeleteContainer`, …) and point at the
    // whole-container ops. `replaceValAtPath` is hooked below, and a hook
    // owns its targets: a block container's value is re-framed in place.

    /// A block map or sequence value has no inline `key = <block>` spelling —
    /// section headers, `> ` and `* ` lines only parse standalone — so it is
    /// re-framed onto the following lines as a nested section instead.
    pub const replaceValAtPath = edit.reframeMappingValue;


    // ── Whole-container ops ──────────────────────────────────────────────────
    //
    // All generic (see `editor.Editor`'s block of the same name): a fig block
    // container may be re-entered and scattered, and `deleteContainer`/
    // `moveContainer`/`reorderContainers` derive its disjoint regions from
    // `Document.node_regions` — re-entered header lines included — with no
    // fig code at all. No `insertContainer`/`appendContainerToSeq` — `set`
    // already vivifies a path — and no `renameContainer`, since a fig header
    // carries its key in one tight span the generic `replaceKeyAtPath`
    // rewrites in place.
};

// Test discovery: importing `fig.zig` (from root.zig) pulls in every fig
// submodule's tests, so the module owns its own test surface.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_helper.zig");
}
