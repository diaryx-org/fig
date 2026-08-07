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

    /// A block container may be re-entered and scattered, so a line delete
    /// risks swallowing an interleaved foreign sibling.
    pub const deleteKeyGuard = edit.containerDeleteGuard;

    /// A block map or sequence value has no inline `key = <block>` spelling —
    /// section headers, `> ` and `* ` lines only parse standalone — so it is
    /// re-framed onto the following lines as a nested section instead.
    pub const replaceValAtPath = edit.reframeMappingValue;

    /// A block sequence item is a `* ` line carrying the same `>` marker-run
    /// prefix as its siblings, which the generic `dashColumn`/`insertSeqLine`
    /// pair — sized for a plain-whitespace indent — would drop.
    pub const appendToSeq = edit.figAppendSeqLine;
    pub const prependToSeq = edit.figPrependSeqLine;

    // ── Whole-container ops ──────────────────────────────────────────────────
    //
    // The EXCLUSIVE operations, shared in shape with TOML and INI (see
    // `editor.Editor`'s block of the same name): a fig block container may be
    // re-entered and scattered, so these gather its disjoint regions rather
    // than splicing one range. No `insertContainer`/`appendContainerToSeq` —
    // `set` already vivifies a path — and no `renameContainer`, since a fig
    // header carries its key in one tight span the generic `replaceKeyAtPath`
    // rewrites in place.

    /// Every region of the container's subtree, re-entered header lines
    /// included (`Document.reentry_headers`).
    pub const deleteContainer = edit.deleteContainer;

    /// The container's fragments, re-emitted contiguously at the destination.
    pub const moveContainer = edit.moveContainer;

    /// Top-level containers reordered among themselves.
    pub const reorderContainers = edit.reorderContainers;
};

// Test discovery: importing `fig.zig` (from root.zig) pulls in every fig
// submodule's tests, so the module owns its own test surface.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_helper.zig");
}
