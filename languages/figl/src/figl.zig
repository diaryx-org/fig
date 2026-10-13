//! The fig authoring dialect: a memorable, typeable, whitespace-insensitive
//! surface over the AST (`kit/src/ast/ast.zig`), parsed by `fig fmt` into the same
//! tree the lossless `canonical` form encodes. It is NOT the oracle — it is
//! allowed to be lossy at the edges (the canonical form and `$fig-envelope`
//! are the faithful fallback). See `DESIGN.md` (this directory) for the full
//! spec.
//!
//! Single grammar (no versions to select), so `Type` has one member, mirroring
//! ZON's `Type = enum { ZON }` pattern.

const fig = @This();
const Document = @import("kit").Document;
const Table = @import("kit").table.Table;
const lang = @import("kit").manifest;

pub const Parser = @import("parser.zig");
pub const Printer = @import("printer.zig");
/// The renderers: how this language spells what the editor splices.
pub const edit = @import("edit.zig");

pub const Type = enum {
    Fig,
};

pub const Language = struct {
    pub const Type = fig.Type;
    pub const Parser = fig.Parser;
    pub const Printer = fig.Printer;
    pub const default_type: fig.Type = .Fig;
    /// Write `input`'s rows into `out`, the table fig hands a parse.
    pub fn parseTable(parser: *fig.Parser, input: []const u8, format: fig.Type, out: *Table) !void {
        _ = parser;
        return fig.Parser.parseInto(out, input, format);
    }
    /// The document fig would adopt from a parse.
    pub fn parse(parser: *fig.Parser, input: []const u8, format: fig.Type) !Document {
        return fig.Parser.parse(parser.allocator, input, format);
    }
    pub const print = fig.Printer.print;
    pub const printNode = fig.Printer.printNode;

    pub const name = "figl";
    /// `.figl` is the authoring dialect's extension. (The canonical form
    /// deliberately owns no extension — select it with `--input canonical`.)
    pub const extensions: []const []const u8 = &.{"figl"};
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
        .name = "figl",
        .prints = .{ .holds = lang.holds_everything, .root_container = true },
        // A whole figl document must be a container; a value need not be.
        .print_fragment_name = "printFragment",
        .splice = .literal,
        .empty_doc_seed = "",
    }};

    pub fn syntax(t: fig.Type) lang.Syntax {
        _ = t;
        return .{
            .comments = .hash,
            // `key = value`. A flow object is `=`-mode or `:`-mode
            // (JSON-embedded) and may not mix, so an insert into a non-empty
            // one copies the separator its first entry uses; an empty one
            // takes this, padded fig-inline style: `{ x = 1 }`.
            .kv_sep = " = ",
            .flow_kv_sep_from_siblings = true,
            .flow_map_pad = " ",
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
            // A line may open with its `>` depth run and a `+` line's mark
            // before the node it holds.
            .line_prefix_marks = ">+",
            // A section format: a block container may be re-entered and
            // scattered, and `parser.zig` records every block container's
            // header lines in `Document.regionsOf`. Its refusals say
            // "container".
            .section_noun = "container",
        };
    }

    // ── Renderers ────────────────────────────────────────────────────────────
    //
    // No hooks. Every edit is the generic engine's, driven by `syntax` above
    // and by one string renderer: how fig spells what follows a key. The
    // `>` marker prefix a block insert copies is `structural_indent`; where
    // an insert lands (after the last child's full extent, correct for a
    // re-entered container) and how a flow object's separator is chosen
    // (`flow_kv_sep_from_siblings`) are engine rules. The logic lives in
    // `edit.zig` (and this format's editor tests are in `tests/editor.zig`).

    /// ` = value` for an inline value; for a block map or sequence value,
    /// which has no inline `key = <block>` spelling — section headers, `> `
    /// and `* ` lines only parse standalone — a newline and the value
    /// re-printed as a nested section one marker level below the key. The
    /// document root takes the value as written.
    pub const renderTail = edit.renderTail;

    // No delete guard: the engine's section rule covers it. Every block
    // container is a section node (`Document.regionsOf`), and a section
    // node cannot be line-spliced — `deleteKey`, `moveKey` and `reorderKeys`
    // refuse it (`CannotDeleteSection`, …) and point at the
    // whole-container ops. A block container's VALUE is still replaceable:
    // its entry records its `=`, so the engine reframes it through
    // `renderTail` rather than splicing the section's span.

    // ── Whole-container ops ──────────────────────────────────────────────────
    //
    // All generic (see `editor.Editor`'s block of the same name): a fig block
    // container may be re-entered and scattered, and `deleteContainer`/
    // `moveContainer`/`reorderContainers` derive its disjoint regions from
    // `Document.regionsOf` — re-entered header lines included — with no
    // fig code at all. No `insertContainer`/`appendContainerToSeq` — `set`
    // already vivifies a path — and no `renameContainer`, since a fig header
    // carries its key in one tight span the generic `replaceKeyAtPath`
    // rewrites in place.
};

// The package's own tests (`zig build test`): every file's. Its editor
// tests need fig itself, which a language cannot depend on, so they are
// `tests/editor.zig`, which fig's build compiles.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
}
