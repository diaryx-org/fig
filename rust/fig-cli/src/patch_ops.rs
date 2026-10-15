//! Port of src/cli/patch_ops.zig.
//!
//! The CLI's half of `fig patch`: the per-format dispatch that turns a target
//! `Format` into "which language, which dialect, and which target do patch
//! subtrees render as", and the slice-in/slice-out wrapper the `patch` action
//! drives.
//!
//! The merge itself is `fig_core::patch` — format-agnostic, and part of the
//! library rather than the binary, because it is the editor-level primitive a
//! binding would want too. What lives here is only the routing, mirroring
//! `edit_ops::route`: the same `caps.edit` gate, the same canonical/gron
//! refusals, the same registry-derived dialect.
//!
//! Slice in, slice out (never a file handle) so the one function serves all
//! three of the action's modes — write in place, `--dry-run` and `--diff`.

use fig_core::editor::Editor;
use fig_core::kit::ast::{NodeId, PathSegment, AST};
use fig_core::patch::{self, Stats};
use fig_core::serialize::Target;
use fig_core::{err, Result};

use crate::types::{self, Format};

/// One patch, resolved: what to merge, where it lands, and what to remove
/// afterwards. `patch` is borrowed and never edited — the merge only reads it.
pub struct Request<'a> {
    /// Where in the TARGET the patch lands. Empty is the document root.
    pub at: &'a [PathSegment<'a>],
    /// The patch document. Must carry no unresolved YAML aliases; the caller
    /// materializes (see `fig_core::patch::apply_to`).
    pub patch: &'a AST,
    /// Which subtree of `patch` to merge — its root unless `--from` narrowed it.
    pub from: NodeId,
    /// Paths to remove from the target after the merge (`--delete`).
    pub deletes: &'a [&'a [PathSegment<'a>]],
    pub options: patch::Options,
}

pub struct PatchResult {
    /// The patched document.
    pub content: Vec<u8>,
    pub stats: Stats,
}

/// Patch `content` — a whole document in `format` — and return the new bytes.
///
/// The per-format facts are the registry's, exactly as in `edit_ops::route`:
/// whether the format is editable at all (`caps.edit`), which dialect to
/// reparse each splice under, and — the one this dispatch adds — which
/// target a patch subtree renders as on the way in.
///
/// Unlike `edit_ops`, there is no requoting step: a patch splices a
/// *rendered* subtree, which the target's own printer already spelled
/// correctly.
pub fn apply_to_slice(content: &[u8], format: Format, req: &Request<'_>) -> Result<PatchResult> {
    match format {
        // Same two refusals `edit_ops::route` makes, for the same reasons: the
        // canonical form is a parse/print pair with no span-splicing editor,
        // and gron is a CLI-only projection.
        Format::CANONICAL => Err(err!(UnsupportedCanonicalEdit)),
        Format::GRON => Err(err!(UnsupportedGronEdit)),
        // A registered language: the one `Editor`, as in `edit_ops::route`,
        // with patch subtrees rendered by the entry's own printer.
        _ => {
            let e = types::runtime_entry(format).ok_or(err!(FormatDisabled))?;
            if !e.language.caps.edit {
                return Err(err!(FormatNotEditable));
            }
            let mut editor = Editor::new(e.lang(), e.dialect);
            editor.init(content)?;
            let target = Target::Language(e.clone());
            let stats = match patch::apply_to(&mut editor, &target, req.at, req.patch, req.from, req.deletes, req.options) {
                Ok(s) => s,
                Err(error) => {
                    // `init` parsed `content`, so a splice the editor rejected
                    // from here on is about text this CLI RENDERED, not about
                    // either file the user named — a value with no spelling the
                    // target's reader accepts back. Relabel it; `main` reports
                    // it against the patch.
                    if editor.splice_rejected {
                        return Err(err!(PatchRenderRejected));
                    }
                    // A value the target has no spelling for at all, or not
                    // where it lands: the same refusal `set` makes, under the
                    // name `main` reports.
                    return Err(match error.name() {
                        "NullUnsupported" => err!(UnwritableNull),
                        "UnsupportedValue" => err!(UnwritableNested),
                        "NonFiniteNumber" => err!(UnwritableNonFinite),
                        _ => error,
                    });
                }
            };
            Ok(PatchResult { content: editor.source().to_vec(), stats })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fig_core::kit::ast::Builder;

    #[test]
    fn apply_to_slice_merges_one_yaml_document_into_another() {
        let Some(yaml) = crate::args::parse_format_name("yaml") else { return };
        let patch = types::runtime_entry(yaml).unwrap().parse(b"replicas: 5\n").unwrap();
        let req = Request { at: &[], patch: &patch.ast, from: patch.ast.root, deletes: &[], options: patch::Options::default() };
        let result = apply_to_slice(b"name: api # keep me\nreplicas: 2\n", yaml, &req).unwrap();
        assert_eq!(result.content, b"name: api # keep me\nreplicas: 5\n");
        assert!(result.stats.changed());
    }

    #[test]
    fn apply_to_slice_refuses_the_two_formats_with_no_in_place_editor() {
        let mut b = Builder::init();
        let n = b.add_null().unwrap();
        let ast = b.finish(n).unwrap();
        let req = Request { at: &[], patch: &ast, from: 0, deletes: &[], options: patch::Options::default() };
        assert_eq!(apply_to_slice(b"", Format::CANONICAL, &req).err(), Some(err!(UnsupportedCanonicalEdit)));
        assert_eq!(apply_to_slice(b"", Format::GRON, &req).err(), Some(err!(UnsupportedGronEdit)));
    }
}
