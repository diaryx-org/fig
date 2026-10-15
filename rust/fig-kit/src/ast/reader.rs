//! Port of kit/src/ast/reader.zig.
//!
//! Navigation helpers + opt-in YAML reference-layer resolution — the
//! read-path surface of `AST` (the mirror of the write path in
//! `builder.rs`). Zig's functions take any tree (`anytype`) with `node(id)`,
//! `root`, `merge_key` and `anchorIterator`; here they are `AST` methods.
//!
//! Errors: `NotAContainer`, `NotAMapping`, `NotASequence`, `NotFound`,
//! `InvalidDocument`; and the resolution errors (`ResolveError`):
//! `UndefinedAlias`, `AliasCycle`, `TooDeep`, `NotAMapping`.

use super::{Node, NodeId, NodeKind, AST};
use crate::err;
use crate::Result;

/// Part of a path in the document structure, as in
/// `&[PathSegment::Index(0), PathSegment::Key(b"hello")]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSegment<'a> {
    Key(&'a [u8]),
    Index(usize),
}

/// `ResolveError`: the names the reference-layer resolution returns.
pub const RESOLVE_ERRORS: &[&str] = &["UndefinedAlias", "AliasCycle", "TooDeep", "NotAMapping"];

impl AST {
    /// Returns a node's child. Returns an error if the given node is not a
    /// container.
    pub fn child(&self, node: &Node<'_>) -> Result<Option<Node<'_>>> {
        let child_id = match node.kind {
            NodeKind::Mapping(first) | NodeKind::Sequence(first) => first,
            _ => return Err(err!(NotAContainer)),
        };
        Ok(child_id.map(|id| self.node(id)))
    }

    /// Iterate on a node to find the next sibling.
    pub fn next(&self, node: &Node<'_>) -> Option<Node<'_>> {
        self.node(node.id).next_sibling.map(|id| self.node(id))
    }

    /// Returns a container's last child, following `next_sibling` to the
    /// end. `None` when the container is empty. Errors if `node` is not a
    /// container.
    pub fn last_child(&self, node: &Node<'_>) -> Result<Option<Node<'_>>> {
        let Some(mut current) = self.child(node)? else { return Ok(None) };
        while let Some(sibling) = self.next(&current) {
            current = sibling;
        }
        Ok(Some(current))
    }

    /// Returns the key node of a mapping's first entry, or `None` when empty.
    /// Used to recover a mapping's indentation column. Errors if `node` is
    /// not a mapping or its first child is not a keyvalue.
    pub fn first_child_key(&self, node: &Node<'_>) -> Result<Option<Node<'_>>> {
        if !matches!(node.kind, NodeKind::Mapping(_)) {
            return Err(err!(NotAMapping));
        }
        let Some(first) = self.child(node)? else { return Ok(None) };
        match first.kind {
            NodeKind::KeyValue(kv) => Ok(Some(self.node(kv.key))),
            _ => Err(err!(InvalidDocument)),
        }
    }

    /// Returns the raw node at `path` without unwrapping keyvalue pairs, so
    /// callers editing structure can see the whole `key: value` span. Compare
    /// `get_key_by_path`/`get_val_by_path`, which unwrap to the key or value
    /// node.
    pub fn get_node_by_path(&self, path: &[PathSegment<'_>]) -> Result<Node<'_>> {
        Ok(self.node(get_id_by_path(self, path)?))
    }

    /// Get a node at a specific path. If a keyvalue is found, get the key.
    pub fn get_key_by_path(&self, path: &[PathSegment<'_>]) -> Result<Node<'_>> {
        let node = self.node(get_id_by_path(self, path)?);
        match node.kind {
            NodeKind::KeyValue(kv) => Ok(self.node(kv.key)),
            _ => Ok(node),
        }
    }

    // ===========================================
    // REFERENCE-LAYER RESOLUTION (read-only, opt-in)
    // ===========================================
    // The default navigation above never follows aliases or merge keys — an
    // `alias` node is an opaque leaf. These helpers resolve the YAML
    // reference layer on demand, for callers (materialize, the editor's
    // "follow" mode) that explicitly want it.

    /// Resolves a `*name` alias node to the id of the anchor it references —
    /// the nearest `&name` defined before the alias (YAML aliases refer
    /// backward). For a non-alias node, returns its own id. The result may
    /// itself be an alias (`&x *y`); a caller wanting a concrete value
    /// follows the chain with a guard.
    pub fn resolve_alias(&self, node: &Node<'_>) -> Result<NodeId> {
        let name = match node.kind {
            NodeKind::Alias(n) => n,
            _ => return Ok(node.id),
        };
        let mut best: Option<NodeId> = None;
        for a in self.anchor_iterator() {
            if a.node >= node.id {
                break; // sorted by id; anchors must precede the alias
            }
            if a.name == name {
                best = Some(a.node);
            }
        }
        best.ok_or(err!(UndefinedAlias))
    }

    /// Fully resolves a node through any chain of aliases to a concrete node
    /// id, guarding against cycles (`&a [*a]`) and runaway depth.
    pub fn resolve_deep(&self, node: &Node<'_>) -> Result<NodeId> {
        let mut current = *node;
        let mut hops = 0usize;
        while matches!(current.kind, NodeKind::Alias(_)) {
            if hops >= 100 {
                return Err(err!(TooDeep));
            }
            hops += 1;
            let target = self.resolve_alias(&current)?;
            if target == current.id {
                return Err(err!(AliasCycle));
            }
            current = self.node(target);
        }
        Ok(current.id)
    }

    /// Looks up `key` in `mapping`, consulting a `<<` merge key when the key
    /// is not a direct child. The merge value is a single alias to a mapping,
    /// an inline mapping, or a sequence of those (earlier entries win); the
    /// host mapping's own keys take precedence over anything merged. Returns
    /// the value node id, or `None` if absent. Read-only and opt-in;
    /// cycle/recursion guarded.
    pub fn merged_child(&self, mapping: &Node<'_>, key: &[u8]) -> Result<Option<NodeId>> {
        let mut visited = [0 as NodeId; 64];
        merged_child_inner(self, mapping, key, &mut visited, 0)
    }

    /// Get a node at a specific path. If a keyvalue is found, get the value.
    pub fn get_val_by_path(&self, path: &[PathSegment<'_>]) -> Result<Node<'_>> {
        let node = self.node(get_id_by_path(self, path)?);
        match node.kind {
            NodeKind::KeyValue(kv) => Ok(self.node(kv.value)),
            _ => Ok(node),
        }
    }
}

fn merged_child_inner(
    ast: &AST,
    mapping: &Node<'_>,
    key: &[u8],
    visited: &mut [NodeId; 64],
    depth: usize,
) -> Result<Option<NodeId>> {
    let NodeKind::Mapping(first) = mapping.kind else { return Err(err!(NotAMapping)) };
    if visited[..depth].contains(&mapping.id) {
        return Err(err!(AliasCycle));
    }
    if depth >= visited.len() {
        return Err(err!(TooDeep));
    }
    visited[depth] = mapping.id;

    // Direct children win; remember a `<<` merge value for the fallback.
    let mut merge_value: Option<NodeId> = None;
    let mut entry = first;
    while let Some(cid) = entry {
        let kv = ast.node(cid).kind.as_keyvalue().expect("a mapping child that is not a keyvalue");
        if let NodeKind::String(k) = ast.node(kv.key).kind {
            if ast.merge_key.is_some_and(|mk| k == mk) {
                merge_value = Some(kv.value);
            } else if k == key {
                return Ok(Some(kv.value));
            }
        }
        entry = ast.node(cid).next_sibling;
    }

    let Some(mv) = merge_value else { return Ok(None) };
    let mvn = ast.node(mv);
    match mvn.kind {
        NodeKind::Sequence(first) => {
            let mut e = first;
            while let Some(eid) = e {
                let target = ast.node(ast.resolve_deep(&ast.node(eid))?);
                if matches!(target.kind, NodeKind::Mapping(_)) {
                    if let Some(found) = merged_child_inner(ast, &target, key, visited, depth + 1)? {
                        return Ok(Some(found));
                    }
                }
                e = ast.node(eid).next_sibling;
            }
        }
        NodeKind::Alias(_) | NodeKind::Mapping(_) => {
            let target = ast.node(ast.resolve_deep(&mvn)?);
            if matches!(target.kind, NodeKind::Mapping(_)) {
                if let Some(found) = merged_child_inner(ast, &target, key, visited, depth + 1)? {
                    return Ok(Some(found));
                }
            }
        }
        _ => {}
    }
    Ok(None)
}

/// Takes a path and runs `get_child_node_id` for each segment.
fn get_id_by_path(ast: &AST, path: &[PathSegment<'_>]) -> Result<NodeId> {
    let mut current_node = ast.root;
    for segment in path {
        current_node = get_child_node_id(ast, current_node, *segment)?;
    }
    Ok(current_node)
}

/// Traverses down the node tree according to `segment`. Can return a
/// keyvalue node that must be deconstructed.
fn get_child_node_id(ast: &AST, parent_id: NodeId, segment: PathSegment<'_>) -> Result<NodeId> {
    let mut current_node = parent_id;
    // A `Key` lookup yields the keyvalue wrapper node; to descend through it
    // we need its value, so unwrap an intermediate keyvalue before applying
    // the next segment. (Sequence elements are stored as bare value nodes,
    // so this only fires after a previous `Key` step.)
    if let NodeKind::KeyValue(kv) = ast.node(current_node).kind {
        current_node = kv.value;
    }
    match segment {
        PathSegment::Key(want) => {
            current_node = match ast.node(current_node).kind {
                NodeKind::Mapping(first_child) => first_child.ok_or(err!(NotFound))?,
                _ => return Err(err!(NotAMapping)),
            };
            // Node is a mapping: loop through its keyvalue children to find
            // the matching key.
            loop {
                // Only string keys are allowed.
                let keyvalue = match ast.node(current_node).kind {
                    NodeKind::KeyValue(kv) => kv,
                    _ => return Err(err!(InvalidDocument)),
                };
                let node_key = match ast.node(keyvalue.key).kind {
                    NodeKind::String(k) => k,
                    _ => return Err(err!(InvalidDocument)),
                };
                if want == node_key {
                    break;
                }
                current_node = ast.node(current_node).next_sibling.ok_or(err!(NotFound))?;
            }
            // The keyvalue wrapper node.
            Ok(current_node)
        }
        PathSegment::Index(index) => {
            current_node = match ast.node(current_node).kind {
                NodeKind::Sequence(first_child) => first_child.ok_or(err!(NotFound))?,
                _ => return Err(err!(NotASequence)),
            };
            for _ in 0..index {
                current_node = ast.node(current_node).next_sibling.ok_or(err!(NotFound))?;
            }
            Ok(current_node)
        }
    }
}
