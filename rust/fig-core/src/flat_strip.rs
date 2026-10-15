//! Port of src/flat_strip.zig.
//!
//! Lossy-mode stripping for fig's three flat/shallow-only output formats —
//! INI, dotenv, `.properties`: drop whatever their capability model can't
//! hold at all before printing, so the printer never aborts a document
//! partway through. Same warn-then-strip-then-print contract as
//! `lossless.rs`'s `lossy_strip` (the CLI runs `diagnostics::analyze` first to
//! warn, then this, exactly mirroring TOML's existing `null`-stripping path in
//! `cli/actions.zig`'s `runGet`).
//!
//! Kept as its own small pass rather than folded into `lossless::NativeKinds`/
//! `lossy_strip` because the capability rule here is DEPTH-based (a mapping
//! nested past some per-format limit) rather than scalar-kind-based —
//! bolting a depth parameter onto `lossless::is_unrepresentable` for the sake of
//! three formats would complicate it for the other four (json/yaml/toml/zon),
//! which have no nesting-depth limit at all. Reuses `lossless.rs`'s `Rows`
//! — the table a rebuilding pass writes, with its `emit`/`carry`/`link` —
//! rather than a third copy of that plumbing.
//!
//! The depth limit is the format's own declaration — `Caps.max_mapping_depth`
//! — and the caller passes it in (`cli/parse_dispatch.zig`'s `flatStripDepth`
//! reads it off the registry). `diagnostics.rs`'s `value_loss` reads the same
//! field, so the warning pass reports exactly what this one removes (a `null`
//! or a `sequence` at any depth; a `mapping` nested past the limit) without
//! restating which formats are flat.

use crate::kit::ast::{KeyValue, Node, NodeId as Id, NodeKind, AST};
use crate::lossless::{key_path, Rows};
use crate::Result;

/// `depth` is the node's OWN depth (0 = document root), matching
/// `diagnostics.rs`'s `value_loss` exactly.
fn is_unrepresentable(kind: &NodeKind<'_>, depth: usize, max_mapping_depth: usize) -> bool {
    match kind {
        NodeKind::Null | NodeKind::Sequence(_) => true,
        NodeKind::Mapping(_) => depth > max_mapping_depth,
        _ => false,
    }
}

/// The result of a `lossy_strip`: a new AST with every node the format can't
/// represent removed, plus the dot/`[i]` paths of what was dropped (for
/// warnings — though the CLI's `diagnostics::analyze` pass, run first, already
/// covers that). `ast` is `None` only when `root_id` ITSELF is unrepresentable
/// (a bare array/`null` root, or a root mapping if the format somehow limited
/// depth to less than 0 — never happens today, kept for shape parity with
/// `lossless::StripResult`).
#[derive(Debug)]
pub struct StripResult {
    pub ast: Option<AST>,
    pub dropped: Vec<Vec<u8>>,
}

/// Build a fresh AST rooted at the subtree `root_id`, dropping every mapping
/// entry/sequence element a format holding at most `max_depth` levels of
/// mapping nesting (its `Caps.max_mapping_depth`) can't represent at all.
/// Dropped paths are reported relative to `root_id`.
pub fn lossy_strip(ast: &AST, root_id: Id, max_depth: usize) -> Result<StripResult> {
    let mut s = Stripper { src: ast, max_mapping_depth: max_depth, out: Rows::new(), dropped: Vec::new() };
    if is_unrepresentable(&ast.kind_of(root_id), 0, max_depth) {
        s.dropped.push(b"(value)".to_vec());
        return Ok(StripResult { ast: None, dropped: s.dropped });
    }
    let root = s.copy(ast.node(root_id), b"", 0)?;
    let ast = s.out.finish(ast, root)?;
    Ok(StripResult { ast: Some(ast), dropped: s.dropped })
}

struct Stripper<'a> {
    src: &'a AST,
    max_mapping_depth: usize,
    out: Rows,
    dropped: Vec<Vec<u8>>,
}

impl Stripper<'_> {
    /// `node` is never itself unrepresentable here — the caller (`lossy_strip`
    /// for the root, `copy_map` for every child) always checks
    /// `is_unrepresentable` before recursing into a node, so a `Sequence` (or
    /// a `Mapping` past the depth limit) is skipped/dropped at the PARENT
    /// and this is never reached with one.
    fn copy(&mut self, node: Node<'_>, path: &[u8], depth: usize) -> Result<Id> {
        match node.kind {
            NodeKind::Mapping(_) => self.copy_map(node, path, depth),
            NodeKind::Sequence(_) => unreachable!("always unrepresentable — see the doc above"),
            NodeKind::KeyValue(_) => unreachable!("never a value position"),
            _ => {
                let id = self.out.emit(node.kind)?;
                self.out.carry(self.src, node.id, id, true)?;
                Ok(id)
            }
        }
    }

    fn copy_map(&mut self, src_node: Node<'_>, path: &[u8], depth: usize) -> Result<Id> {
        let id = self.out.emit(NodeKind::Mapping(None))?;
        self.out.carry(self.src, src_node.id, id, true)?;
        let mut last: Option<Id> = None;
        let mut c = src_node.kind.as_mapping().flatten();
        while let Some(cid) = c {
            c = self.src.node(cid).next_sibling;
            let kv = self.src.kind_of(cid).as_keyvalue().expect("a mapping child is a keyvalue");
            let child_path = key_path(self.src, path, kv.key);
            if is_unrepresentable(&self.src.kind_of(kv.value), depth + 1, self.max_mapping_depth) {
                self.dropped.push(child_path);
                continue;
            }
            let new_key = self.copy(self.src.node(kv.key), path, depth)?;
            let new_val = self.copy(self.src.node(kv.value), &child_path, depth + 1)?;
            let kvid = self.out.emit(NodeKind::KeyValue(KeyValue { key: new_key, value: new_val }))?;
            self.out.link(id, &mut last, kvid);
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::err;
    use crate::kit::ast::{Builder, Entry, PathSegment as P};

    #[test]
    fn drops_an_array_at_any_depth_keeps_everything_else() {
        let mut b = Builder::init();
        let one = b.add_int(1).unwrap();
        let arr = b.add_sequence(&[one]).unwrap();
        let arr_key = b.add_string(b"list").unwrap();
        let scalar_key = b.add_string(b"name").unwrap();
        let scalar_val = b.add_string(b"fig").unwrap();
        let root = b
            .add_mapping(&[Entry { key: scalar_key, value: scalar_val }, Entry { key: arr_key, value: arr }])
            .unwrap();
        let ast = b.finish(root).unwrap();

        let result = lossy_strip(&ast, ast.root, 0).unwrap();
        assert_eq!(1, result.dropped.len());
        assert_eq!(b"list", &result.dropped[0][..]);
        let stripped = result.ast.unwrap();
        assert_eq!(b"fig", stripped.get_val_by_path(&[P::Key(b"name")]).unwrap().kind.as_string().unwrap());
        assert_eq!(err!(NotFound), stripped.get_val_by_path(&[P::Key(b"list")]).unwrap_err());
    }

    #[test]
    fn ini_keeps_one_level_of_mapping_nesting_drops_the_next() {
        let mut b = Builder::init();
        let inner_key = b.add_string(b"deep").unwrap();
        let inner_val = b.add_string(b"x").unwrap();
        let inner = b.add_mapping(&[Entry { key: inner_key, value: inner_val }]).unwrap();
        let section_key = b.add_string(b"server").unwrap();
        let host_key = b.add_string(b"host").unwrap();
        let host_val = b.add_string(b"localhost").unwrap();
        let nested_key = b.add_string(b"nested").unwrap();
        let section =
            b.add_mapping(&[Entry { key: host_key, value: host_val }, Entry { key: nested_key, value: inner }]).unwrap();
        let root = b.add_mapping(&[Entry { key: section_key, value: section }]).unwrap();
        let ast = b.finish(root).unwrap();

        let result = lossy_strip(&ast, ast.root, 1).unwrap();
        assert_eq!(1, result.dropped.len());
        assert_eq!(b"server.nested", &result.dropped[0][..]);
        let stripped = result.ast.unwrap();
        let host = stripped.get_val_by_path(&[P::Key(b"server"), P::Key(b"host")]).unwrap();
        assert_eq!(b"localhost", host.kind.as_string().unwrap());
        assert_eq!(err!(NotFound), stripped.get_val_by_path(&[P::Key(b"server"), P::Key(b"nested")]).unwrap_err());
    }

    #[test]
    fn a_bare_array_root_has_nothing_to_print() {
        let mut b = Builder::init();
        let one = b.add_int(1).unwrap();
        let root = b.add_sequence(&[one]).unwrap();
        let ast = b.finish(root).unwrap();

        let result = lossy_strip(&ast, ast.root, 0).unwrap();
        assert!(result.ast.is_none());
        assert_eq!(1, result.dropped.len());
    }
}
