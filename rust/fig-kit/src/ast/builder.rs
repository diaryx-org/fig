//! Port of kit/src/ast/builder.zig.
//!
//! Builder — programmatic construction of an AST, the write-path mirror of
//! the read-path navigation helpers in `reader.rs`.
//!
//! It writes rows: id is row index, a container's children are linked
//! through `first_child` and `next_sibling`, and every string is copied into
//! the table's buffer, since a built tree has no source to borrow from.
//!
//! Construction is bottom-up: add the children, then add the container from
//! their ids. `finish` freezes the result into an owned `AST`.
//!
//! Each node id must be placed in exactly one container. A row carries a
//! single parent and `next_sibling`, so reusing an id in two containers
//! corrupts the tree.

use super::{Comment, ExtKind, KindTag, NodeComments, NodeId, Style, Tag, AST};
use crate::span::Span;
use crate::table::{self, Row, Table, NONE};
use crate::Result;

/// One mapping entry for `add_mapping`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub key: NodeId,
    pub value: NodeId,
}

/// A comment whose text is in the build's buffer.
#[derive(Debug, Clone, Copy)]
struct PendingComment {
    text: table::Text,
    style: Style,
}

/// A node's comments, as growable runs.
#[derive(Debug, Clone, Default)]
struct PendingComments {
    leading: Vec<PendingComment>,
    trailing: Option<PendingComment>,
    dangling: Vec<PendingComment>,
}

#[derive(Debug, Clone, Copy)]
enum PendingTag {
    Kind(KindTag),
    Text(table::Text),
}

#[derive(Debug, Default)]
pub struct Builder {
    t: Table,
    /// Indexed by node id, grown per node: the comments set on it, kept apart
    /// until `finish` so that `set_comments` can replace them.
    comments: Vec<PendingComments>,
    any_comments: bool,
    /// Type tags by node id, grown on demand: tags are rare.
    node_tags: Vec<Option<PendingTag>>,
    any_tags: bool,
    /// What `view` last made, replaced by the next `view`.
    snapshot: Option<AST>,
}

impl Builder {
    pub fn init() -> Builder {
        Builder::default()
    }

    /// How many nodes have been added.
    pub fn node_count(&self) -> usize {
        self.t.rows.len()
    }

    /// Set `id`'s source span (a byte range into the input `id` was read
    /// from).
    pub fn set_span(&mut self, id: NodeId, span: Span) {
        let r = &mut self.t.rows[id as usize];
        r.start = span.start as u32;
        r.end = span.end as u32;
    }

    /// Attach comments to an already-added node, REPLACING any previously
    /// set on `id` (pass `NodeComments::default()` to clear). Every comment's
    /// text is copied. For attaching comments one at a time, prefer
    /// `add_leading_comment`/`add_dangling_comment`/`set_trailing_comment`.
    pub fn set_comments(&mut self, id: NodeId, node_comments: &NodeComments<'_>) -> Result<()> {
        let mut leading = std::mem::take(&mut self.comments[id as usize].leading);
        let mut dangling = std::mem::take(&mut self.comments[id as usize].dangling);
        leading.clear();
        dangling.clear();
        for c in node_comments.leading.iter() {
            leading.push(self.own_comment(c)?);
        }
        for c in node_comments.dangling.iter() {
            dangling.push(self.own_comment(c)?);
        }
        let trailing = match &node_comments.trailing {
            Some(c) => Some(self.own_comment(c)?),
            None => None,
        };
        let slot = &mut self.comments[id as usize];
        slot.leading = leading;
        slot.dangling = dangling;
        slot.trailing = trailing;
        self.any_comments = true;
        Ok(())
    }

    /// Append one comment to the run rendered ABOVE `id` (its `leading` run),
    /// in call order. The text is copied.
    ///
    /// Comments are format-agnostic: a target format without comment syntax
    /// here (plain JSON, or compact JSON5/JSONC/ZON) drops them on serialize.
    /// That is a reported loss, not an error — see `diagnostics` — so this
    /// never rejects based on a format.
    pub fn add_leading_comment(&mut self, id: NodeId, comment: Comment<'_>) -> Result<()> {
        let c = self.own_comment(&comment)?;
        self.comments[id as usize].leading.push(c);
        self.any_comments = true;
        Ok(())
    }

    /// Append one comment to `id`'s `dangling` run — comments at the END of a
    /// container's body, after its last child. Only meaningful on a container
    /// node; see `NodeComments`.
    pub fn add_dangling_comment(&mut self, id: NodeId, comment: Comment<'_>) -> Result<()> {
        let c = self.own_comment(&comment)?;
        self.comments[id as usize].dangling.push(c);
        self.any_comments = true;
        Ok(())
    }

    /// Set `id`'s single same-line `trailing` comment, replacing any previous
    /// one. The text is copied.
    pub fn set_trailing_comment(&mut self, id: NodeId, comment: Comment<'_>) -> Result<()> {
        let c = self.own_comment(&comment)?;
        self.comments[id as usize].trailing = Some(c);
        self.any_comments = true;
        Ok(())
    }

    /// Attach a cross-format type tag to an already-added node, replacing any
    /// tag previously set on `id`.
    pub fn set_tag(&mut self, id: NodeId, tag: Tag<'_>) -> Result<()> {
        while self.node_tags.len() <= id as usize {
            self.node_tags.push(None);
        }
        self.node_tags[id as usize] = Some(match tag {
            Tag::Kind(k) => PendingTag::Kind(k),
            Tag::Text(x) => PendingTag::Text(self.t.add_string(x)?),
        });
        self.any_tags = true;
        Ok(())
    }

    pub fn add_null(&mut self) -> Result<NodeId> {
        self.append(Row { kind: table::Kind::NULL, ..Row::default() })
    }

    pub fn add_bool(&mut self, value: bool) -> Result<NodeId> {
        self.append(Row { kind: if value { table::Kind::TRUE } else { table::Kind::FALSE }, ..Row::default() })
    }

    /// Add a signed integer scalar (rendered as decimal text).
    pub fn add_int(&mut self, value: i64) -> Result<NodeId> {
        self.add_number_raw(value.to_string().as_bytes(), false)
    }

    /// Add an unsigned integer scalar (decimal text). Separate from `add_int`
    /// so values above `i64::MAX` round-trip without overflow.
    pub fn add_uint(&mut self, value: u64) -> Result<NodeId> {
        self.add_number_raw(value.to_string().as_bytes(), false)
    }

    /// Add a numeric scalar from already-formatted text (copied). `is_float`
    /// records which number kind it is. This is the escape hatch for floats
    /// and for integers outside the i64/u64 range.
    pub fn add_number_raw(&mut self, raw: &[u8], is_float: bool) -> Result<NodeId> {
        let text = self.t.add_string(raw)?;
        self.append(Row { kind: if is_float { table::Kind::FLOAT } else { table::Kind::INT }, text, ..Row::default() })
    }

    /// Add a string scalar (copied).
    pub fn add_string(&mut self, value: &[u8]) -> Result<NodeId> {
        let text = self.t.add_string(value)?;
        self.append(Row { kind: table::Kind::STRING, text, ..Row::default() })
    }

    /// Add a format-specific scalar (TOML datetime, ZON enum/char literal).
    /// See `Extended`.
    pub fn add_extended(&mut self, kind: ExtKind, text: &[u8]) -> Result<NodeId> {
        let text = self.t.add_string(text)?;
        self.append(Row { kind: table::Kind::EXT + kind.ordinal(), text, ..Row::default() })
    }

    /// Add an alias to the anchor `name` (copied).
    pub fn add_alias(&mut self, name: &[u8]) -> Result<NodeId> {
        let text = self.t.add_string(name)?;
        self.append(Row { kind: table::Kind::ALIAS, text, ..Row::default() })
    }

    /// Add a sequence whose elements are the given node ids, in order.
    /// Empty is allowed (`&[]`), yielding a node with no children.
    pub fn add_sequence(&mut self, items: &[NodeId]) -> Result<NodeId> {
        let id = self.append(Row { kind: table::Kind::SEQUENCE, ..Row::default() })?;
        self.adopt(id, items);
        Ok(id)
    }

    /// Add a mapping from the given entries, in order. A `keyvalue` node is
    /// minted for each entry.
    pub fn add_mapping(&mut self, entries: &[Entry]) -> Result<NodeId> {
        let mut kvs = Vec::with_capacity(entries.len());
        for e in entries {
            kvs.push(self.add_key_value(e.key, e.value)?);
        }
        self.add_mapping_from_entries(&kvs)
    }

    /// Add one `keyvalue` node wrapping `key`/`value`, returning its own id —
    /// the lower-level twin of `add_mapping` for a caller that needs the
    /// entry's id before assembling the mapping (e.g. to `set_span` it). Pair
    /// with `add_mapping_from_entries`.
    pub fn add_key_value(&mut self, key: NodeId, value: NodeId) -> Result<NodeId> {
        let id = self.append(Row { kind: table::Kind::KEYVALUE, ..Row::default() })?;
        self.adopt(id, &[key, value]);
        Ok(id)
    }

    /// Add a mapping from already-built `keyvalue` node ids (as returned by
    /// `add_key_value`), in order.
    pub fn add_mapping_from_entries(&mut self, kv_ids: &[NodeId]) -> Result<NodeId> {
        let id = self.append(Row { kind: table::Kind::MAPPING, ..Row::default() })?;
        self.adopt(id, kv_ids);
        Ok(id)
    }

    /// Freeze the builder into an owned `AST` rooted at `root`. The builder
    /// is reset to empty. The AST carries the comments and type tags set; the
    /// anchor/alias layer stays empty (no builder API defines an anchor).
    pub fn finish(&mut self, root: NodeId) -> Result<AST> {
        let mut t = std::mem::take(&mut self.t);
        self.flush(&mut t)?;
        let ast = AST::own(&mut t, root, None)?;
        self.t = t;
        self.reset();
        Ok(ast)
    }

    /// An `AST` of the builder's current nodes, rooted at `root`, without
    /// consuming the build: a copy the builder owns, valid until the next
    /// `view`.
    pub fn view(&mut self, root: NodeId) -> Result<&AST> {
        self.snapshot = None;
        let mut copy = Table::default();
        copy.rows.extend_from_slice(&self.t.rows);
        copy.strings.extend_from_slice(&self.t.strings);
        self.flush(&mut copy)?;
        self.snapshot = Some(AST::own(&mut copy, root, None)?);
        Ok(self.snapshot.as_ref().expect("just set"))
    }

    // ── internals ───────────────────────────────────────────────────────────

    fn append(&mut self, row: Row) -> Result<NodeId> {
        let id = self.t.add_row(row)?;
        self.comments.push(PendingComments::default());
        Ok(id)
    }

    /// Make `ids` the children of `parent`, in order.
    fn adopt(&mut self, parent: NodeId, ids: &[NodeId]) {
        let rows = &mut self.t.rows;
        rows[parent as usize].first_child = if ids.is_empty() { NONE } else { ids[0] };
        for (i, &c) in ids.iter().enumerate() {
            rows[c as usize].parent = parent;
            rows[c as usize].next_sibling = if i + 1 < ids.len() { ids[i + 1] } else { NONE };
        }
    }

    /// Write the pending comments and tags into `t`'s fact lists. The texts
    /// are offsets into the build's buffer, which `t` has (it is the build's
    /// table, or a copy of its rows and buffer).
    fn flush(&self, t: &mut Table) -> Result<()> {
        if self.any_tags {
            for (i, tag) in self.node_tags.iter().enumerate() {
                let Some(x) = tag else { continue };
                let f = match *x {
                    PendingTag::Text(s) => table::Tag { node: i as u32, text: s, ..Default::default() },
                    PendingTag::Kind(k) => table::Tag {
                        node: i as u32,
                        text: t.add_string(Tag::Kind(k).spelling())?,
                        resolved: table::TAG_KIND + k.ordinal(),
                        ..Default::default()
                    },
                };
                t.tags.push(f);
            }
        }
        if self.any_comments {
            for (i, nc) in self.comments.iter().enumerate() {
                let n = i as u32;
                let rec = |slot: u8, c: &PendingComment| table::Comment { node: n, slot, style: c.style.ordinal(), text: c.text };
                for c in &nc.leading {
                    t.comments.push(rec(table::COMMENT_LEADING, c));
                }
                if let Some(c) = &nc.trailing {
                    t.comments.push(rec(table::COMMENT_TRAILING, c));
                }
                for c in &nc.dangling {
                    t.comments.push(rec(table::COMMENT_DANGLING, c));
                }
            }
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.comments.clear();
        self.node_tags.clear();
        self.any_comments = false;
        self.any_tags = false;
    }

    /// A comment whose text is copied into the build's buffer.
    fn own_comment(&mut self, c: &Comment<'_>) -> Result<PendingComment> {
        Ok(PendingComment { text: self.t.add_string(c.text)?, style: c.style })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::NodeKind;

    #[test]
    fn builder_builds_a_tree_with_comments_and_tags() {
        let mut b = Builder::init();
        let v = b.add_number_raw(b"09", false).unwrap();
        b.set_tag(v, Tag::Kind(KindTag::Integer)).unwrap();
        let k = b.add_string(b"id").unwrap();
        b.add_leading_comment(k, Comment::line(b"hi")).unwrap();
        let root = b.add_mapping(&[Entry { key: k, value: v }]).unwrap();
        {
            let view = b.view(root).unwrap();
            assert_eq!(b"hi", view.comments(k).leading[0].text);
        }
        let ast = b.finish(root).unwrap();
        assert_eq!(Some(Tag::Kind(KindTag::Integer)), ast.tag_of(v));
        assert_eq!(b"hi", ast.comments(k).leading[0].text);
        let kv = ast.kind_of(root).as_mapping().unwrap().unwrap();
        assert!(matches!(ast.kind_of(kv), NodeKind::KeyValue(_)));
        assert_eq!(0, b.node_count());
    }
}
