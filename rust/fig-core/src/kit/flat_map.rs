//! Port of kit/src/flat_map.zig.
//!
//! Shared "flat mapping" builder for line-oriented `key = value` formats —
//! INI, dotenv, `.properties`. Each of those parsers needs exactly the glue
//! TOML's parser hand-rolls for itself (`addNode`/`appendKeyValue`/
//! `lookupChild`), minus everything TOML needs that they don't: dotted keys,
//! arrays, inline tables, typed scalars. This module factors out the part
//! that's actually identical — node allocation and "does this mapping
//! already have a child keyed X" — so the parsers (and INI's one-level
//! section nesting) share it.
//!
//! What's deliberately NOT here: comment capture (each parser's leading/
//! trailing rules differ enough to stay bespoke) and value decoding (escape
//! sets conflict across the three formats). Only the tree-shape plumbing is
//! shared.

use super::ast::{KeyValue, NodeId, NodeKind};
use super::child_index::ChildIndex;
use super::row_writer::RowWriter;
use super::span::Span;
use super::table::Table;
use crate::err;
use crate::Result;

/// The rows of the table a parse is writing (`RowWriter`), the one
/// operation that adds a node, and an index of each mapping's entries by key
/// and its last entry, so that adding a key is constant time rather than a
/// walk of the mapping. A node's spans and side facts (comments,
/// NestedText's item markers and entry separators) go through `w`.
#[derive(Debug)]
pub struct NodeArena<'t> {
    pub w: RowWriter<'t>,
    pub index: ChildIndex,
}

impl<'t> NodeArena<'t> {
    pub fn init(t: &'t mut Table) -> NodeArena<'t> {
        NodeArena { w: RowWriter::init(t), index: ChildIndex::new() }
    }

    pub fn add_node(&mut self, kind: NodeKind<'_>, span: Span) -> Result<NodeId> {
        self.w.add(kind, span)
    }
}

/// The keyvalue child of `map_id` keyed `key`, or `None`.
pub fn lookup_child(arena: &NodeArena<'_>, map_id: NodeId, key: &[u8]) -> Option<NodeId> {
    arena.index.get(&arena.w, map_id, key)
}

/// How a repeated key is handled. Every INI/dotenv/`.properties`
/// implementation in the wild silently lets the later assignment win —
/// unlike TOML, which hard-errors — so `Overwrite` is what all three callers
/// actually want; `Err` is NestedText's, which refuses a repeat.
/// `FirstValue` drops the later entry, and `Keep` adds it beside the first.
/// They are the duplicate policies of the other kits' row writers, under
/// these names there: `last_value` (`Overwrite`), `first_value`, `keep` and
/// `error` (`Err`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DuplicatePolicy {
    #[default]
    Overwrite,
    Err,
    FirstValue,
    Keep,
}

/// `DuplicateError`: the name `put_entry` refuses a repeat with under `Err`.
pub const DUPLICATE_ERRORS: &[&str] = &["DuplicateKey"];

/// What `put_entry` did with the entry: added it fresh, or met a repeat and
/// overwrote the first entry's value (`Overwrite`), ignored the later entry
/// (`FirstValue`), or kept it beside the first (`Keep`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutResult {
    Added,
    Overwrote,
    Ignored,
    Kept,
}

/// What `put_entry` did, and the keyvalue node it added or overwrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Put {
    pub result: PutResult,
    pub kv: NodeId,
}

/// Add (or, on a repeat with `Overwrite`, update) a `key = value` entry in
/// mapping `map_id`. `key_id` must already be a string node (each caller
/// adds it via its own `arena.add_node` so it can attach leading comments
/// first, same as TOML's `appendKeyValue`).
///
/// `Overwrite` keeps the FIRST-SEEN position but the LAST-ASSIGNED value —
/// repointing the existing keyvalue node's `value` in place. The overwritten
/// value's row, and the repeat's key row, are left unlinked:
/// `RowWriter::finish` drops them. The result says whether this was a fresh
/// entry or a repeat, so a caller can turn a repeat into a `DuplicateKey`
/// warning (real content, likely a mistake) without it being a parse
/// *error*.
pub fn put_entry(
    arena: &mut NodeArena<'_>,
    map_id: NodeId,
    key_id: NodeId,
    value_id: NodeId,
    policy: DuplicatePolicy,
) -> Result<Put> {
    let key = arena.w.kind_of(key_id).as_string().expect("put_entry: the key is not a string");
    let repeat = arena.index.get(&arena.w, map_id, key);
    if let Some(existing_kv) = repeat {
        match policy {
            DuplicatePolicy::Err => return Err(err!(DuplicateKey)),
            DuplicatePolicy::Overwrite => {
                let old_key = arena.w.kind_of(existing_kv).as_keyvalue().expect("an entry").key;
                arena.w.set(existing_kv, NodeKind::KeyValue(KeyValue { key: old_key, value: value_id }))?;
                return Ok(Put { result: PutResult::Overwrote, kv: existing_kv });
            }
            DuplicatePolicy::FirstValue => return Ok(Put { result: PutResult::Ignored, kv: existing_kv }),
            DuplicatePolicy::Keep => {}
        }
    }
    let span = Span::new(arena.w.span_of(key_id).start, arena.w.span_of(value_id).end);
    let kv_id = arena.w.add(NodeKind::KeyValue(KeyValue { key: key_id, value: value_id }), span)?;
    arena.index.append(&mut arena.w, map_id, kv_id)?;
    // A kept repeat leaves the key finding the first entry.
    if repeat.is_some() {
        return Ok(Put { result: PutResult::Kept, kv: kv_id });
    }
    arena.index.put(&arena.w, kv_id)?;
    Ok(Put { result: PutResult::Added, kv: kv_id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::{self, PathSegment, AST};
    use std::sync::Arc;

    #[test]
    fn put_entry_appends_in_order_and_reports_fresh_vs_repeat() {
        let src: Arc<[u8]> = Arc::from(&b"a=1\nb=2\na=3\n"[..]);
        let mut t = Table::init(src.clone());
        {
            let mut arena = NodeArena::init(&mut t);
            let root = arena.add_node(NodeKind::Mapping(None), Span::new(0, 0)).unwrap();
            let k1 = arena.add_node(NodeKind::String(b"a"), Span::new(0, 1)).unwrap();
            let v1 = arena.add_node(NodeKind::String(b"1"), Span::new(2, 3)).unwrap();
            assert_eq!(PutResult::Added, put_entry(&mut arena, root, k1, v1, DuplicatePolicy::Overwrite).unwrap().result);

            let k2 = arena.add_node(NodeKind::String(b"b"), Span::new(4, 5)).unwrap();
            let v2 = arena.add_node(NodeKind::String(b"2"), Span::new(6, 7)).unwrap();
            assert_eq!(PutResult::Added, put_entry(&mut arena, root, k2, v2, DuplicatePolicy::Overwrite).unwrap().result);

            // Repeat of "a": overwrites in place, first-seen position preserved.
            let k3 = arena.add_node(NodeKind::String(b"a"), Span::new(8, 9)).unwrap();
            let v3 = arena.add_node(NodeKind::String(b"3"), Span::new(10, 11)).unwrap();
            let put = put_entry(&mut arena, root, k3, v3, DuplicatePolicy::Overwrite).unwrap();
            assert_eq!(PutResult::Overwrote, put.result);

            let w = &arena.w;
            let first = w.kind_of(root).as_mapping().unwrap().unwrap();
            assert_eq!(first, put.kv);
            assert_eq!(k1, w.kind_of(first).as_keyvalue().unwrap().key);
            assert_eq!(v3, w.kind_of(first).as_keyvalue().unwrap().value); // last value wins
            let second = w.node(first).next_sibling.unwrap();
            assert_eq!(k2, w.kind_of(second).as_keyvalue().unwrap().key);
            assert_eq!(None, w.node(second).next_sibling); // no 3rd sibling appended
            assert_eq!(Some(second), lookup_child(&arena, root, b"b"));
            assert_eq!(None, lookup_child(&arena, root, b"c"));

            // The overwritten value and the repeat's key are unreachable:
            // finished, the table holds the root, two entries and their keys
            // and values.
            arena.w.finish(root).unwrap();
        }
        let ast = AST::adopt(src, &mut t, ast::Spans::Required).unwrap();
        assert_eq!(7, ast.node_count());
        assert_eq!(b"3", ast.get_val_by_path(&[PathSegment::Key(b"a")]).unwrap().kind.as_string().unwrap());
    }

    #[test]
    fn put_entry_err_rejects_a_repeat() {
        let mut t = Table::init(&b"a=1\nb=2\na=3\n"[..]);
        let mut arena = NodeArena::init(&mut t);
        let root = arena.add_node(NodeKind::Mapping(None), Span::new(0, 0)).unwrap();
        let k1 = arena.add_node(NodeKind::String(b"a"), Span::new(0, 1)).unwrap();
        let v1 = arena.add_node(NodeKind::String(b"1"), Span::new(2, 3)).unwrap();
        put_entry(&mut arena, root, k1, v1, DuplicatePolicy::Err).unwrap();
        let k2 = arena.add_node(NodeKind::String(b"a"), Span::new(4, 5)).unwrap();
        let v2 = arena.add_node(NodeKind::String(b"2"), Span::new(6, 7)).unwrap();
        assert_eq!(err!(DuplicateKey), put_entry(&mut arena, root, k2, v2, DuplicatePolicy::Err).unwrap_err());
    }
}
