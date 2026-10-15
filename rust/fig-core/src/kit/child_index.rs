//! Port of kit/src/child_index.zig.
//!
//! What a parser that keeps adding entries to its mappings needs to find,
//! in constant time: a mapping's last child, to link the next after, and its
//! entry under a key, to tell a repeat. Without it both are a walk of the
//! sibling chain, and a table of n keys costs n² — TOML's, INI's, dotenv's.
//!
//! The index holds row ids, not key texts: an entry's parent and key are
//! read from the rows each time it is hashed or compared. (Zig's hash map
//! takes an adapted context for that; here the keyed index is buckets of
//! row ids by the key's hash, compared through the rows.)

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use super::ast::{NodeId, NodeKind};
use super::row_writer::RowWriter;
use crate::Result;

#[derive(Debug, Default, Clone)]
pub struct ChildIndex {
    /// A container's last child.
    last: HashMap<NodeId, NodeId>,
    /// A mapping's entries, by the entry's row alone: its parent and key are
    /// read from the rows when it is hashed or compared, so an entry costs
    /// the index four bytes and no copy of its key.
    keyed: HashMap<u64, Vec<NodeId>>,
}

/// Entry `id`'s parent and key text, as the rows hold them.
fn key_of<'a>(w: &'a RowWriter<'_>, id: NodeId) -> (NodeId, &'a [u8]) {
    let parent = w.t.rows[id as usize].parent;
    let NodeKind::KeyValue(kv) = w.kind_of(id) else { panic!("ChildIndex: entry {id} is not a keyvalue") };
    let NodeKind::String(text) = w.kind_of(kv.key) else { panic!("ChildIndex: entry {id}'s key is not a string") };
    (parent, text)
}

fn hash_key(parent: NodeId, text: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    parent.hash(&mut h);
    text.hash(&mut h);
    h.finish()
}

impl ChildIndex {
    pub fn new() -> ChildIndex {
        ChildIndex::default()
    }

    /// Link `child` as `parent`'s last child: after the one before it, or as
    /// its first, naming `parent` as its parent. `parent` is a mapping or a
    /// sequence whose children are all linked through here.
    pub fn append(&mut self, w: &mut RowWriter<'_>, parent: NodeId, child: NodeId) -> Result<()> {
        match self.last.get(&parent) {
            Some(&prev) => w.set_next(prev, Some(child)),
            None => {
                let kind = match w.kind_of(parent) {
                    NodeKind::Sequence(_) => NodeKind::Sequence(Some(child)),
                    _ => NodeKind::Mapping(Some(child)),
                };
                w.set(parent, kind)?;
            }
        }
        w.set_next(child, None);
        w.t.rows[child as usize].parent = parent;
        self.last.insert(parent, child);
        Ok(())
    }

    /// Record keyvalue `entry`, already appended to its mapping, under its
    /// key, replacing an entry with the same key.
    pub fn put(&mut self, w: &RowWriter<'_>, entry: NodeId) -> Result<()> {
        let (parent, text) = key_of(w, entry);
        let bucket = self.keyed.entry(hash_key(parent, text)).or_default();
        for slot in bucket.iter_mut() {
            if key_of(w, *slot) == (parent, text) {
                *slot = entry;
                return Ok(());
            }
        }
        bucket.push(entry);
        Ok(())
    }

    /// Mapping `parent`'s entry under `key`, or `None`.
    pub fn get(&self, w: &RowWriter<'_>, parent: NodeId, key: &[u8]) -> Option<NodeId> {
        let bucket = self.keyed.get(&hash_key(parent, key))?;
        bucket.iter().copied().find(|&id| key_of(w, id) == (parent, key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::KeyValue;
    use crate::kit::span::Span;
    use crate::kit::table::Table;

    #[test]
    fn append_links_in_order_and_get_finds_an_entry_by_its_key() {
        let mut t = Table::default();
        let mut w = RowWriter::init(&mut t);
        let mut ix = ChildIndex::new();
        let map = w.add(NodeKind::Mapping(None), Span::new(0, 0)).unwrap();
        let mut entries = [0 as NodeId; 3];
        for (e, k) in entries.iter_mut().zip([b"a", b"b", b"c"]) {
            let key = w.add(NodeKind::String(k), Span::new(0, 0)).unwrap();
            let val = w.add(NodeKind::Null, Span::new(0, 0)).unwrap();
            *e = w.add(NodeKind::KeyValue(KeyValue { key, value: val }), Span::new(0, 0)).unwrap();
            ix.append(&mut w, map, *e).unwrap();
            ix.put(&w, *e).unwrap();
        }
        assert_eq!(Some(Some(entries[0])), w.kind_of(map).as_mapping());
        assert_eq!(Some(entries[1]), w.node(entries[0]).next_sibling);
        assert_eq!(Some(entries[2]), w.node(entries[1]).next_sibling);
        assert_eq!(Some(entries[1]), ix.get(&w, map, b"b"));
        assert_eq!(None, ix.get(&w, map, b"d"));
    }
}
