//! Port of languages/figl/src/edit.zig.
//!
//! fig-specific editing helpers for the editor over figl.
//!
//! The generic span-splice engine lives in the editor; this module holds the
//! fig-only logic it delegates to. Unlike TOML (whose `[header]` syntax
//! forces a multi-region gather for almost every structural op) or YAML
//! (indentation-column rendering), fig's block layer is line-oriented and
//! self-describing — every line's `>` count states its own depth, so a new
//! sibling line can be spliced in *anywhere* after an existing child's full
//! text and still parse correctly. That collapses most of what would
//! otherwise be format-specific plumbing into one trick: **copy an existing
//! sibling's marker-prefix text verbatim** (the run of `>` markers +
//! separator space immediately before its key/value) rather than
//! recomputing depth/indentation from scratch. See DESIGN.md's "Depth is a
//! correctness risk" and "prefix-count depth" for why this is safe.
//!
//! ## Whole-container structural ops (`deleteContainer`/`moveContainer`/
//! `reorderContainers`)
//!
//! Nothing of these lives here: they are the generic editor ops, over the
//! derived region set built from `Document::regions_of`. What fig contributes
//! is the parser's half — `parser.rs` records every BLOCK (non-flow)
//! container's header lines: the line that created it, plus every LATER
//! header that re-entered it verbatim. A container's node span anchors only
//! the line that CREATED it, so those later lines are in no child's span and
//! are exactly what the table exists to carry.
//!
//! The same table is why `deleteKey`, `moveKey` and `reorderKeys` refuse a
//! block-container-valued entry (`CannotDeleteSection`, `CannotMoveSection`,
//! `CannotReorderSections` — the engine's one section rule, spelled per
//! `Syntax::section_noun`): such a container may be scattered, and a line
//! splice would move or remove only the fragment it can see.
//!
//! ## Scope (documented, not silent)
//!
//! `replaceValAtPath` overwriting a re-entered/scattered container's entire
//! value in ONE splice still carries a narrow gap (it replaces the node's
//! widened span, which is not region-aware); the reparse-rollback net keeps
//! it safe. And deleting a container whose removal leaves an ANCESTOR header
//! childless still rolls back via `FigEmptyContainer` — the cascade is
//! deliberately not implied by a delete of the child path.

use super::parser;
use super::printer;
use super::tokenizer as tok;
use super::Type;
use crate::kit::ast::{NodeKind, SerializeOptions};
use crate::kit::manifest::RenderRequest;
use crate::Result;

// ============================================================================
// render_tail — what follows a key (the fig `renderTail` renderer)
// ============================================================================

/// If `value_text` is a fig BLOCK-container fragment — a section body (`a = 1`
/// lines) or `*`-element list that has no inline `key = <value>` spelling —
/// return it re-printed as a block body at marker depth `depth`. Inline values
/// return `None`: flow containers (`{ … }` / `[ … ]`), single-line scalars,
/// and multi-line `'''`/`"""` block strings all splice directly after
/// `key = `. A fragment that fails to parse, or parses to a non-container,
/// likewise returns `None` so the caller keeps the plain inline splice and
/// lets the reparse-rollback net report any real error.
///
/// Fig markers are an absolute depth ruler measured from column 0 (`> ` per
/// level), so re-printing the fragment's root at `depth` — one level below the
/// key it hangs under — yields body lines that carry their own full marker run
/// and need no further indentation.
fn block_body(depth: usize, value_text: &[u8]) -> Option<Vec<u8>> {
    let t = tok::trim(value_text, b" \t\r\n");
    // Inline forms keep the direct splice: flow braces/brackets, quoted or
    // block-string scalars, and any single-line value.
    if t.is_empty() || t[0] == b'{' || t[0] == b'[' || t[0] == b'\'' || t[0] == b'"' {
        return None;
    }
    if !t.contains(&b'\n') {
        return None;
    }

    let frag = parser::parse(value_text, Type::Fig).ok()?;
    match frag.ast.kind_of(frag.ast.root) {
        NodeKind::Mapping(_) | NodeKind::Sequence(_) => {}
        _ => return None, // a multi-line scalar is still an inline value
    }

    let mut w = Vec::new();
    printer::print_node(&mut w, &frag.ast, frag.ast.root, depth, SerializeOptions::default()).ok()?;
    Some(w)
}

/// What follows a key on its entry line, after `<indent><key>`: ` = <value>`
/// for an inline value, or a newline plus the value re-framed as a block
/// section (see `block_body`) one level below the key's marker depth — which
/// is the count of `>` cells in `indent`, the structural prefix the engine
/// copied from the key's line. An empty `key` is the document root, whose
/// value is a whole document already and is taken as written. No trailing
/// newline.
pub fn render_tail(_: Type, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()> {
    let indent = r.indent;
    let key_text = r.key;
    let value_text = r.value;
    if key_text.is_empty() {
        out.extend_from_slice(value_text);
        return Ok(());
    }
    let depth = indent.iter().filter(|&&c| c == b'>').count();
    if let Some(body) = block_body(depth + 1, value_text) {
        out.push(b'\n');
        // print_node ends every line (the last included) with '\n'; the caller
        // supplies the entry's own line break, so drop the printed trailing
        // one.
        let mut end = body.len();
        while end > 0 && body[end - 1] == b'\n' {
            end -= 1;
        }
        out.extend_from_slice(&body[..end]);
    } else {
        out.extend_from_slice(b" = ");
        out.extend_from_slice(value_text);
    }
    Ok(())
}
