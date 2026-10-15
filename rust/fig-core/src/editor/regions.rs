//! Port of src/editor/regions.zig.
//!
//! Derived regions: the physical extent of a SECTION node, and the
//! multi-region machinery the whole-container ops run on it.
//!
//! A section format's logical container is not contiguous in the source.
//! TOML assembles a table from scattered headers (`[a]` x=1 … `[other]` y=2 …
//! `[a.b]` z=3); fig does the same with `>` marker runs and re-entered
//! headers; INI with a reopened `[section]`. In all three the AST has one node
//! per logical container while its bytes are spread across the file, so a
//! whole-container op cannot splice a single `[min,max)` range — it must
//! gather the disjoint line-regions that belong to the container and rebuild
//! the source once.
//!
//! `gather` below is that step made generic: the parser records each section
//! node's HEADER LINES in `Document::regions_of` (the one fact spans cannot
//! carry — see that field), and everything else about the extent falls out of
//! the tree. A child that is itself a section recurses; any other child is one
//! contiguous entry whose line comes from its span. No format is named, and no
//! source is sniffed.
//!
//! Downstream of the gather: coalescing regions, splicing them out,
//! relocating them contiguously and reordering bundles of them.
//! `docs/proposals/derived-regions.md` has the catalogue of what each format's
//! ops needed and what became generic.
//!
//! The editor-taking functions take the `Editor` itself; each uses only its
//! `source` and `replace_at_span`.

use super::splice::{self, comment_block_start, line_end_after, line_start_before, Comments};
use super::Editor;
use crate::kit::ast::{Node, NodeKind};
use crate::kit::document::{Document, NodeRegion};
use crate::kit::manifest::Syntax;
use crate::kit::span::Span;
use crate::Result;

/// A line-aligned source range `[start, end)` belonging to one logical
/// container's subtree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: usize,
    pub end: usize,
}

/// Full line-region of an in-container entry (`key = value`, possibly spanning
/// several lines): its owned comment block through the newline ending its last
/// line.
pub fn entry_line_region(source: &[u8], span: Span, comments: Comments) -> Region {
    Region {
        start: comment_block_start(source, line_start_before(source, span.start), comments),
        end: line_end_after(source, span.end.saturating_sub(1)),
    }
}

/// The physical line of one recorded header: its owned comment block through
/// the header line's own newline. `comments` drives the comment scan, as it
/// does for `entry_line_region`.
pub fn header_line_region(source: &[u8], header: NodeRegion, comments: Comments) -> Region {
    Region { start: comment_block_start(source, header.start as usize, comments), end: header.end as usize }
}

/// Append every line-region belonging to the subtree of the section node
/// `node`: its own recorded header lines (`Document::regions_of` — the
/// creating line and every re-entry), then each child's contribution. A child
/// whose value is itself a section recurses; any other child (a scalar, a flow
/// container, a multi-line string) is one contiguous entry, taken as its own
/// line-region from its span. The result is unsorted and may overlap (a
/// header line that also holds a dotted entry appears twice); callers run
/// `normalize` before using it.
pub fn gather(parsed: &Document, source: &[u8], node: Node<'_>, comments: Comments, out: &mut Vec<Region>) {
    for h in parsed.regions_of(node.id) {
        out.push(header_line_region(source, *h, comments));
    }
    match node.kind {
        NodeKind::Mapping(first) => {
            let mut cur = first;
            while let Some(id) = cur {
                let kv = parsed.ast.node(id);
                let val = parsed.ast.node(kv.kind.as_keyvalue().expect("a mapping child that is not a keyvalue").value);
                if parsed.is_section(&val) {
                    gather(parsed, source, val, comments, out);
                } else {
                    out.push(entry_line_region(source, parsed.span(&kv), comments));
                }
                cur = kv.next_sibling;
            }
        }
        NodeKind::Sequence(first) => {
            let mut cur = first;
            while let Some(id) = cur {
                let el = parsed.ast.node(id);
                if parsed.is_section(&el) {
                    gather(parsed, source, el, comments, out);
                } else {
                    out.push(entry_line_region(source, parsed.span(&el), comments));
                }
                cur = el.next_sibling;
            }
        }
        _ => {}
    }
}

/// `gather` followed by `normalize`: the coalesced, ascending region set of
/// the section node `node`, as a fresh list. `merge_touching` is
/// `normalize`'s.
pub fn gather_normalized(parsed: &Document, source: &[u8], node: Node<'_>, comments: Comments, merge_touching: bool) -> Vec<Region> {
    let mut regions = Vec::new();
    gather(parsed, source, node, comments, &mut regions);
    let n = normalize(&mut regions, merge_touching);
    regions.truncate(n);
    regions
}

/// Byte offset just past the last line of the section node `node`'s subtree
/// — where a new sibling section can be spliced without splitting it. The
/// end of its last derived region, or of its own line when it has none.
pub fn extent_end(parsed: &Document, source: &[u8], node: Node<'_>, comments: Comments) -> usize {
    let regions = gather_normalized(parsed, source, node, comments, true);
    match regions.last() {
        None => line_end_after(source, parsed.span(&node).end.saturating_sub(1)),
        Some(r) => r.end,
    }
}

/// Sort `regions` by start and coalesce them into a disjoint, ascending set in
/// place; returns the coalesced count (the caller uses `regions[..n]`).
///
/// `merge_touching` decides whether two regions that meet exactly — one's `end`
/// is the next's `start` — become one. Only TOML's rename needs false: it
/// addresses each header region's own start to rewrite the segment inside it,
/// which a merged region would hide. Every other caller is insensitive to the
/// choice, because a zero-width gap between touching regions copies nothing.
pub fn normalize(regions: &mut [Region], merge_touching: bool) -> usize {
    // Zig's `std.mem.sort` is a stable sort, as is this.
    regions.sort_by_key(|r| r.start);
    if regions.is_empty() {
        return 0;
    }
    let mut w = 0usize;
    for i in 1..regions.len() {
        let r = regions[i];
        let overlaps = if merge_touching { r.start <= regions[w].end } else { r.start < regions[w].end };
        if overlaps {
            regions[w].end = regions[w].end.max(r.end);
        } else {
            w += 1;
            regions[w] = r;
        }
    }
    w + 1
}

/// Rebuild the source with `regions` (disjoint, ascending) removed, in one
/// `replace_at_span` so the reparse/rollback runs once. The whole of a
/// delete-container op once the gather has run.
pub fn splice_out(ed: &mut Editor<'_>, regions: &[Region]) -> Result<()> {
    let source = &ed.source;
    let mut out = Vec::with_capacity(source.len());
    let mut pos = 0usize;
    for r in regions {
        out.extend_from_slice(&source[pos..r.start]);
        pos = r.end;
    }
    out.extend_from_slice(&source[pos..]);
    let len = source.len();
    ed.replace_at_span(Span::new(0, len), &out)
}

/// Append `block` to `out` separated from any preceding content by exactly one
/// blank line (two newlines). `block` is appended verbatim (it already ends in
/// a newline). Used when relocating a container so it reads as its own section
/// at the destination.
pub fn append_with_blank_before(out: &mut Vec<u8>, block: &[u8]) {
    if block.is_empty() {
        return;
    }
    let n = out.len();
    if n > 0 {
        if n >= 2 && out[n - 1] == b'\n' && out[n - 2] == b'\n' {
            // already a blank line
        } else if out[n - 1] == b'\n' {
            out.push(b'\n');
        } else {
            out.extend_from_slice(b"\n\n");
        }
    }
    out.extend_from_slice(block);
}

/// Remove `used` (a container's gathered, normalized regions) from their
/// current positions and re-emit those bytes CONTIGUOUSLY at `dest_at`,
/// separated from surrounding content by one blank line. Interleaved foreign
/// content stays put. One splice.
///
/// A no-op when `used` is empty, or when `dest_at` falls strictly inside one of
/// the moved regions — the container would be moving into itself. A boundary
/// landing exactly at a fragment's edge (EOF coinciding with the last fragment,
/// say) is still a real relocation, since it collapses the fragments together.
pub fn relocate(ed: &mut Editor<'_>, used: &[Region], dest_at: usize) -> Result<()> {
    if used.is_empty() {
        return Ok(());
    }
    for r in used {
        if dest_at > r.start && dest_at < r.end {
            return Ok(());
        }
    }
    let source = &ed.source;
    let mut moved = Vec::new();
    for r in used {
        moved.extend_from_slice(&source[r.start..r.end]);
    }

    // Emit the kept source with `used` removed and `moved` spliced in at
    // `dest_at`, in a single pass.
    let mut out = Vec::with_capacity(source.len() + 2);
    let mut inserted = false;
    let mut pos = 0usize;
    for r in used {
        if !inserted && dest_at >= pos && dest_at <= r.start {
            out.extend_from_slice(&source[pos..dest_at]);
            append_with_blank_before(&mut out, &moved);
            out.extend_from_slice(&source[dest_at..r.start]);
            inserted = true;
        } else {
            out.extend_from_slice(&source[pos..r.start]);
        }
        pos = r.end;
    }
    if !inserted {
        out.extend_from_slice(&source[pos..dest_at]);
        append_with_blank_before(&mut out, &moved);
        out.extend_from_slice(&source[dest_at..]);
    } else {
        out.extend_from_slice(&source[pos..]);
    }
    let len = source.len();
    ed.replace_at_span(Span::new(0, len), &out)
}

/// Remove `used` — the union of every reordered container's regions, already
/// normalized — and re-emit `bundles` (each container's captured bytes, in the
/// caller's desired final order) at the position the earliest of them currently
/// occupies. Containers not named are untouched.
///
/// Separation is `append_block_sep`'s (tight) rather than `relocate`'s blank
/// line: these were already siblings in one region of the file, so reordering
/// them should not introduce spacing that was not there — except the blank
/// line that ends a continued line before a bundle follows it (`syn`).
pub fn reorder_bundles(ed: &mut Editor<'_>, used: &[Region], bundles: &[Vec<u8>], syn: &Syntax) -> Result<()> {
    if used.is_empty() {
        return Ok(());
    }
    let source = &ed.source;
    let anchor = used[0].start;

    let mut out = Vec::with_capacity(source.len());
    let mut pos = 0usize;
    for r in used {
        if anchor >= pos && anchor <= r.start {
            out.extend_from_slice(&source[pos..anchor]);
            for b in bundles {
                splice::append_block_sep(&mut out, b, syn);
                if !b.is_empty() && b[b.len() - 1] != b'\n' {
                    out.push(b'\n');
                }
            }
            out.extend_from_slice(&source[anchor..r.start]);
        } else {
            out.extend_from_slice(&source[pos..r.start]);
        }
        pos = r.end;
    }
    out.extend_from_slice(&source[pos..]);
    let len = source.len();
    ed.replace_at_span(Span::new(0, len), &out)
}

/// Capture one container's bytes from its gathered regions, appending those
/// regions to `all` (the caller's global removal set) as it goes. The per-name
/// body of a reorder: the caller gathers, this captures, `reorder_bundles`
/// rewrites.
pub fn capture_bundle(source: &[u8], regions: &[Region], all: &mut Vec<Region>) -> Vec<u8> {
    let mut bytes = Vec::new();
    for r in regions {
        bytes.extend_from_slice(&source[r.start..r.end]);
        all.push(*r);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_sorts_and_coalesces() {
        let mut r = vec![
            Region { start: 10, end: 20 },
            Region { start: 0, end: 5 },
            Region { start: 5, end: 8 },
            Region { start: 15, end: 25 },
        ];
        let n = normalize(&mut r, true);
        assert_eq!(&[Region { start: 0, end: 8 }, Region { start: 10, end: 25 }], &r[..n]);
        let mut r = vec![Region { start: 5, end: 8 }, Region { start: 0, end: 5 }];
        assert_eq!(2, normalize(&mut r, false));
        assert_eq!(0, normalize(&mut [], true));
    }

    #[test]
    fn append_with_blank_before_leaves_exactly_one_blank_line() {
        let mut out = b"a\n".to_vec();
        append_with_blank_before(&mut out, b"b\n");
        assert_eq!(b"a\n\nb\n", &out[..]);
        let mut out = b"a".to_vec();
        append_with_blank_before(&mut out, b"b\n");
        assert_eq!(b"a\n\nb\n", &out[..]);
        let mut out = b"a\n\n".to_vec();
        append_with_blank_before(&mut out, b"b\n");
        assert_eq!(b"a\n\nb\n", &out[..]);
    }

    #[test]
    fn capture_bundle_records_its_regions() {
        let mut all = Vec::new();
        let b = capture_bundle(b"0123456789", &[Region { start: 1, end: 3 }, Region { start: 5, end: 6 }], &mut all);
        assert_eq!(b"125", &b[..]);
        assert_eq!(2, all.len());
    }
}
