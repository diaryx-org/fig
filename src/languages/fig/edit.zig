//! fig-specific editing helpers for `Editor(Fig)`.
//!
//! The generic span-splice engine lives in `../editor.zig`; this module holds
//! the fig-only logic it delegates to. Unlike TOML (whose `[header]` syntax
//! forces a multi-region gather for almost every structural op) or YAML
//! (indentation-column rendering), fig's block layer is line-oriented and
//! self-describing — every line's `>` count states its own depth, so a new
//! sibling line can be spliced in *anywhere* after an existing child's full
//! text and still parse correctly. That collapses most of what would
//! otherwise be format-specific plumbing into one trick, used throughout this
//! file: **copy an existing sibling's marker-prefix text verbatim** (the run
//! of `>` markers + separator space immediately before its key/value) rather
//! than recomputing depth/indentation from scratch. See DESIGN.md's "Depth is
//! a correctness risk" and "prefix-count depth" for why this is safe.
//!
//! ## Whole-container structural ops (`deleteContainer`/`moveContainer`/
//! `reorderContainers`)
//!
//! Nothing of these lives here any more: they are the generic ops in
//! `../../editor.zig`, over the derived region set `../../editor/regions.zig`
//! builds from `Document.node_regions`. What fig contributes is the parser's
//! half — `fig/parser.zig` records every BLOCK (non-flow) container's header
//! lines: the line that created it, plus every LATER header that re-entered
//! it verbatim (`database` written a second time, `> pool` reopened later in
//! the same parent's body, a dotted path whose final segment re-selects an
//! existing container, `xs[i]` re-opening an element — DESIGN.md
//! "Re-entering a path to add new keys is fine", a shape `fig fmt`'s own
//! grouped hoisting EMITS). A container's node span anchors only the line
//! that CREATED it, so those later lines are in no child's span and are
//! exactly what the table exists to carry. From that, the engine's gather
//! recurses into every block-container child (each is a section node) and
//! takes every other child's own line, which handles fig's TOML-equivalent
//! scattering (`a`/`other`/`a.b`) and verbatim re-entry alike. The tests below
//! pin every one of those behaviours for fig; the logic is the engine's.
//!
//! The same table is why `deleteKey`, `moveKey` and `reorderKeys` refuse a
//! block-container-valued entry (`CannotDeleteSection`, `CannotMoveSection`,
//! `CannotReorderSections` — the engine's one section rule, spelled per
//! `Syntax.section_noun`): such a container may be scattered, and a line
//! splice would move or remove only the fragment it can see.
//!
//! ## Scope (documented, not silent)
//!
//! `replaceValAtPath` overwriting a re-entered/scattered container's entire
//! value in ONE splice still carries a narrow gap (it replaces the node's
//! widened span, which is not region-aware); the reparse-rollback net keeps
//! it safe. And deleting a container whose removal leaves an ANCESTOR header
//! childless (e.g. `a.b` when `b` was `a`'s only child and `a` was written as
//! a header) still rolls back via `FigEmptyContainer` — the cascade
//! ("also delete the now-empty ancestor header") is deliberately not implied
//! by a delete of the child path.

const std = @import("std");

const AST = @import("../../kit/ast/ast.zig");
const Document = @import("../../kit/document.zig");
const Span = @import("../../kit/span.zig");
const lang = @import("../manifest.zig");
const splice = @import("../../kit/lines.zig");
const Fig = @import("fig.zig").Language;
const Printer = @import("printer.zig");
const Writer = std.Io.Writer;
const log = std.log.scoped(.editor);


const lineStartBefore = splice.lineStartBefore;
const lineEndAfter = splice.lineEndAfter;
const firstNonSpace = splice.firstNonSpace;
const isFlow = splice.isFlow;

// ============================================================================
// renderTail — what follows a key (the fig `renderTail` renderer)
// ============================================================================

/// If `value_text` is a fig BLOCK-container fragment — a section body (`a = 1`
/// lines) or `*`-element list that has no inline `key = <value>` spelling —
/// return it re-printed as a block body at marker depth `depth` (caller frees).
/// Inline values return null: flow containers (`{ … }` / `[ … ]`), single-line
/// scalars, and multi-line `'''`/`"""` block strings all splice directly after
/// `key = `. A fragment that fails to parse, or parses to a non-container,
/// likewise returns null so the caller keeps the plain inline splice and lets
/// the reparse-rollback net (`replaceAtSpan`) report any real error.
///
/// Fig markers are an absolute depth ruler measured from column 0 (`> ` per
/// level), so re-printing the fragment's root at `depth` — one level below the
/// key it hangs under — yields body lines that carry their own full marker run
/// and need no further indentation. This is what lets a caller splice a block
/// map/sequence into a document (e.g. a fenced embed) instead of freezing every
/// short map inline as flow.
fn blockBody(allocator: std.mem.Allocator, depth: usize, value_text: []const u8) ?[]u8 {
    const t = std.mem.trim(u8, value_text, " \t\r\n");
    // Inline forms keep the direct splice: flow braces/brackets, quoted or
    // block-string scalars, and any single-line value.
    if (t.len == 0 or t[0] == '{' or t[0] == '[' or t[0] == '\'' or t[0] == '"') return null;
    if (std.mem.indexOfScalar(u8, t, '\n') == null) return null;

    var parser: Fig.Parser = .{ .allocator = allocator };
    var frag = Fig.parse(&parser, value_text, Fig.default_type) catch return null;
    defer frag.deinit(allocator);
    switch (frag.ast.nodes[frag.ast.root].kind) {
        .mapping, .sequence => {},
        else => return null, // a multi-line scalar is still an inline value
    }

    var w: Writer.Allocating = .init(allocator);
    defer w.deinit();
    Printer.printNode(&w.writer, &frag.ast, frag.ast.root, depth, .{}) catch return null;
    return allocator.dupe(u8, w.written()) catch return null;
}

/// What follows a key on its entry line, after `<indent><key>`: ` = <value>`
/// for an inline value, or a newline plus the value re-framed as a block
/// section (see `blockBody`) one level below the key's marker depth — which
/// is the count of `>` cells in `indent`, the structural prefix the engine
/// copied from the key's line. An empty `key_text` is the document root,
/// whose value is a whole document already and is taken as written. No
/// trailing newline. See `editor.Editor.writeTail`.
pub fn renderTail(_: Fig.Type, allocator: std.mem.Allocator, out: *std.ArrayList(u8), r: lang.RenderRequest) !void {
    const indent = r.indent;
    const key_text = r.key;
    const value_text = r.value;
    if (key_text.len == 0) return out.appendSlice(allocator, value_text);
    const depth = std.mem.count(u8, indent, ">");
    if (blockBody(allocator, depth + 1, value_text)) |body| {
        defer allocator.free(body);
        try out.append(allocator, '\n');
        // printNode ends every line (the last included) with '\n'; the caller
        // supplies the entry's own line break, so drop the printed trailing one.
        try out.appendSlice(allocator, std.mem.trimEnd(u8, body, "\n"));
    } else {
        try out.appendSlice(allocator, " = ");
        try out.appendSlice(allocator, value_text);
    }
}
