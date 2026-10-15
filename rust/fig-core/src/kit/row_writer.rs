//! Port of kit/src/row_writer.zig.
//!
//! A parser's way of writing the node table fig hands it
//! (docs/proposals/core-only.md §4.9): a node is a row, added in whatever
//! order the parser meets it, and linked to its siblings and children
//! through the row's own `first_child` and `next_sibling` — the links a
//! parser building a tree of `Node`s kept in the nodes. `finish` then puts
//! the rows in the pre-order a parse's table is read in, each naming its
//! parent, and moves every side fact to its node's new row; fig derives the
//! links again when it adopts the table, so a parser's are scratch.
//!
//! A text is written as `Table::text` writes it: a slice of the table's
//! source stays there (so slice `Table::source_arc`, not a copy), any other
//! is copied into the table's buffer. A slice `node` hands back may point
//! into that buffer: read it, don't keep it (the borrow checker holds you
//! to that here).

use std::sync::Arc;

use super::ast::{self, Comment, KindTag, Node, NodeComments, NodeId, NodeKind, NumberKind, Tag, AST};
use super::document::{Document, MentionKind};
use super::span::Span;
use super::table::{self, Table, NONE};
use crate::Result;

/// The rows of a table being written. Zig's is a pointer to the table,
/// passed by value; here it borrows the table mutably, and the functions
/// that took one by value take `&mut RowWriter`.
#[derive(Debug)]
pub struct RowWriter<'t> {
    pub t: &'t mut Table,
}

impl<'t> RowWriter<'t> {
    pub fn init(t: &'t mut Table) -> RowWriter<'t> {
        RowWriter { t }
    }

    /// How many rows have been written.
    pub fn count(&self) -> usize {
        self.t.rows.len()
    }

    /// A new node of `kind` over `span`; its id is its row. A container's
    /// first child and a keyvalue's key and value are linked as `kind` says.
    pub fn add(&mut self, kind: NodeKind<'_>, span: Span) -> Result<NodeId> {
        let id = self.t.add_row(table::Row { start: span.start as u32, end: span.end as u32, ..table::Row::default() })?;
        self.set(id, kind)?;
        Ok(id)
    }

    /// Make node `id` one of `kind`, replacing what it was: a scalar's text
    /// is written again, a container's first child and a keyvalue's key and
    /// value relinked. A sibling link is kept.
    pub fn set(&mut self, id: NodeId, kind: NodeKind<'_>) -> Result<()> {
        use table::Kind as K;
        let i = id as usize;
        {
            let r = &mut self.t.rows[i];
            r.first_child = NONE;
            r.text = table::Text::ABSENT;
        }
        match kind {
            NodeKind::Null => self.t.rows[i].kind = K::NULL,
            NodeKind::Boolean(b) => self.t.rows[i].kind = if b { K::TRUE } else { K::FALSE },
            NodeKind::String(x) => {
                let text = self.t.text(x)?;
                let r = &mut self.t.rows[i];
                r.kind = K::STRING;
                r.text = text;
            }
            NodeKind::Number(n) => {
                let text = self.t.text(n.raw)?;
                let r = &mut self.t.rows[i];
                r.kind = if n.kind == NumberKind::Float { K::FLOAT } else { K::INT };
                r.text = text;
            }
            NodeKind::Extended(e) => {
                let text = self.t.text(e.text)?;
                let r = &mut self.t.rows[i];
                r.kind = K::EXT + e.kind.ordinal();
                r.text = text;
            }
            NodeKind::Alias(x) => {
                let text = self.t.text(x)?;
                let r = &mut self.t.rows[i];
                r.kind = K::ALIAS;
                r.text = text;
            }
            NodeKind::Sequence(first) => {
                let r = &mut self.t.rows[i];
                r.kind = K::SEQUENCE;
                r.first_child = first.unwrap_or(NONE);
            }
            NodeKind::Mapping(first) => {
                let r = &mut self.t.rows[i];
                r.kind = K::MAPPING;
                r.first_child = first.unwrap_or(NONE);
            }
            NodeKind::KeyValue(kv) => {
                let rows = &mut self.t.rows;
                rows[i].kind = K::KEYVALUE;
                rows[i].first_child = kv.key;
                rows[kv.key as usize].next_sibling = kv.value;
                rows[kv.key as usize].parent = id;
                rows[kv.value as usize].next_sibling = NONE;
                rows[kv.value as usize].parent = id;
            }
        }
        Ok(())
    }

    /// Node `id` as it stands: its kind, and its next sibling — none for a
    /// keyvalue's key or value.
    pub fn node(&self, id: NodeId) -> Node<'_> {
        let rows = &self.t.rows;
        let r = rows[id as usize];
        let in_entry = r.parent != NONE && rows[r.parent as usize].kind == table::Kind::KEYVALUE;
        Node {
            id,
            kind: ast::row_kind(&self.t.source, &self.t.strings, rows, r),
            next_sibling: if in_entry || r.next_sibling == NONE { None } else { Some(r.next_sibling) },
        }
    }

    /// Node `id`'s kind: `node(id).kind`.
    pub fn kind_of(&self, id: NodeId) -> NodeKind<'_> {
        let rows = &self.t.rows;
        ast::row_kind(&self.t.source, &self.t.strings, rows, rows[id as usize])
    }

    /// Link `next` after `id` among its siblings, or end the chain at `id`.
    pub fn set_next(&mut self, id: NodeId, next: Option<NodeId>) {
        self.t.rows[id as usize].next_sibling = next.unwrap_or(NONE);
    }

    /// Name `parent` as node `id`'s parent — which only `finish_in_order`
    /// reads; `finish` names every parent itself.
    pub fn set_parent(&mut self, id: NodeId, parent: NodeId) {
        self.t.rows[id as usize].parent = parent;
    }

    /// Node `id`'s span.
    pub fn span_of(&self, id: NodeId) -> Span {
        let r = self.t.rows[id as usize];
        Span::new(r.start as usize, r.end as usize)
    }

    pub fn set_span(&mut self, id: NodeId, span: Span) {
        let r = &mut self.t.rows[id as usize];
        r.start = span.start as u32;
        r.end = span.end as u32;
    }

    /// Mark sequence `id` as having no delimiters (`document::ItemSep::open`).
    pub fn set_open(&mut self, id: NodeId) {
        self.t.rows[id as usize].flags |= table::ROW_OPEN;
    }

    // ── Side facts ──────────────────────────────────────────────────────────
    // Appended as the parser meets them; a later fact of the same kind on the
    // same node replaces an earlier when fig adopts the table.

    pub fn anchor(&mut self, id: NodeId, name: &[u8], span: Option<Span>) -> Result<()> {
        let name = self.t.text(name)?;
        self.t.anchors.push(table::Anchor {
            node: id,
            name,
            start: span.map_or(NONE, |s| s.start as u32),
            end: span.map_or(NONE, |s| s.end as u32),
        });
        Ok(())
    }

    /// A tag on `id`: a kind tag resolved and spelled as YAML's core schema
    /// spells it; a text tag as written, resolved as `resolved` says
    /// (`table::TAG_*`), with whether the node agrees.
    pub fn tag(&mut self, id: NodeId, t: Tag<'_>, resolved: u8, agrees: bool, span: Option<Span>) -> Result<()> {
        let mut f = match t {
            Tag::Text(x) => table::Tag {
                node: id,
                text: self.t.text(x)?,
                resolved,
                agrees: agrees as u8,
                ..Default::default()
            },
            Tag::Kind(k) => table::Tag {
                node: id,
                text: self.t.text(t.spelling())?,
                resolved: table::TAG_KIND + KindTag::ordinal(k),
                ..Default::default()
            },
        };
        if let Some(s) = span {
            f.start = s.start as u32;
            f.end = s.end as u32;
        }
        self.t.tags.push(f);
        Ok(())
    }

    /// The `-`/`*` marker that introduces item `id` (`Document::marker_span`).
    pub fn marker(&mut self, id: NodeId, span: Span) -> Result<()> {
        self.t.markers.push(table::SpanFact { node: id, start: span.start as u32, end: span.end as u32 });
        Ok(())
    }

    /// The separator between entry `id`'s key and value (`Document::sep_span`).
    pub fn sep(&mut self, id: NodeId, span: Span) -> Result<()> {
        self.t.seps.push(table::SpanFact { node: id, start: span.start as u32, end: span.end as u32 });
        Ok(())
    }

    /// The separator a separated sequence writes between its items.
    pub fn item_sep(&mut self, id: NodeId, text: &[u8]) -> Result<()> {
        let text = self.t.text(text)?;
        self.t.item_seps.push(table::ItemSep { node: id, text });
        Ok(())
    }

    /// A header line of section `id` (`Document::regions_of`), whole.
    pub fn region(&mut self, id: NodeId, start: usize, end: usize) -> Result<()> {
        self.t.regions.push(table::SpanFact { node: id, start: start as u32, end: end as u32 });
        Ok(())
    }

    /// A place section `id`'s name, or item `id`'s path, is written
    /// (`Document::mentions_of`).
    pub fn mention(&mut self, id: NodeId, span: Span, kind: MentionKind) -> Result<()> {
        self.t.mentions.push(table::Mention { node: id, start: span.start as u32, end: span.end as u32, kind: kind.byte() });
        Ok(())
    }

    /// `nc` as comments on `id`.
    pub fn comments(&mut self, id: NodeId, nc: &NodeComments<'_>) -> Result<()> {
        for c in nc.leading.iter() {
            self.comment(id, table::COMMENT_LEADING, *c)?;
        }
        if let Some(c) = nc.trailing {
            self.comment(id, table::COMMENT_TRAILING, c)?;
        }
        for c in nc.dangling.iter() {
            self.comment(id, table::COMMENT_DANGLING, *c)?;
        }
        Ok(())
    }

    /// One comment on `id`, in `slot` (`table::COMMENT_*`), after any already
    /// in that slot.
    pub fn comment(&mut self, id: NodeId, slot: u8, c: Comment<'_>) -> Result<()> {
        let text = self.t.text(c.text)?;
        self.t.comments.push(table::Comment { node: id, slot, style: c.style.ordinal(), text });
        Ok(())
    }

    /// A `%TAG` declaration.
    pub fn directive(&mut self, handle: &[u8], prefix: &[u8]) -> Result<()> {
        let handle = self.t.text(handle)?;
        let prefix = self.t.text(prefix)?;
        self.t.directives.push(table::Directive { handle, prefix });
        Ok(())
    }

    // ── Finishing ───────────────────────────────────────────────────────────

    /// Put the rows in pre-order from `root`, as fig reads a parse's table:
    /// row 0 the root, each row after its parent and naming it, a container's
    /// children in their sibling order, a keyvalue's key before its value.
    /// Every side fact moves with its node, and each list is left in node
    /// order; a row the tree does not reach, and its facts, are dropped. The
    /// rows' links are written again to name the new rows. Rows already in
    /// pre-order stay where they are.
    pub fn finish(&mut self, root: NodeId) -> Result<()> {
        self.last_trailing();
        let rows = &mut self.t.rows;
        // The pre-order walk, and each row's place in it.
        let mut order = vec![0u32; rows.len()];
        let mut place = vec![NONE; rows.len()];
        struct Item {
            id: u32,
            parent: u32,
        }
        let mut stack: Vec<Item> = Vec::new();
        let mut n = 0usize;
        let mut in_place = true;
        stack.push(Item { id: root, parent: NONE });
        // A child's next sibling goes on the stack before its own first
        // child, so the stack pops a subtree whole before the sibling after
        // it.
        while let Some(item) = stack.pop() {
            let id = item.id as usize;
            // A node linked twice is written once, and its siblings were met
            // the first time, so a cycle in the links ends here too.
            if place[id] != NONE {
                continue;
            }
            if item.parent != NONE {
                let sibling = rows[id].next_sibling;
                if sibling != NONE {
                    stack.push(Item { id: sibling, parent: item.parent });
                }
            }
            place[id] = n as u32;
            order[n] = item.id;
            if id != n {
                in_place = false;
            }
            rows[id].parent = item.parent;
            n += 1;
            let first = rows[id].first_child;
            if table::Kind::is_container(rows[id].kind) && first != NONE {
                stack.push(Item { id: first, parent: item.id });
            }
        }
        if in_place && n == rows.len() {
            // Parents name rows that did not move.
            return Ok(());
        }
        let mut out: Vec<table::Row> = Vec::with_capacity(n);
        for &old in &order[..n] {
            let mut r = rows[old as usize];
            r.parent = if r.parent == NONE { NONE } else { place[r.parent as usize] };
            r.first_child = NONE;
            r.next_sibling = NONE;
            out.push(r);
        }
        // The links again, naming the new rows: each row's parent's children
        // are the rows after it that name it, in order.
        let mut last = vec![NONE; n];
        for i in 0..n {
            let p = out[i].parent;
            if p != NONE {
                let p = p as usize;
                if last[p] == NONE {
                    out[p].first_child = i as u32;
                } else {
                    out[last[p] as usize].next_sibling = i as u32;
                }
                last[p] = i as u32;
            }
        }
        self.t.rows = out;
        remap(&mut self.t.anchors, &place);
        remap(&mut self.t.tags, &place);
        remap(&mut self.t.markers, &place);
        remap(&mut self.t.seps, &place);
        remap(&mut self.t.item_seps, &place);
        remap(&mut self.t.comments, &place);
        remap(&mut self.t.regions, &place);
        remap(&mut self.t.mentions, &place);
        Ok(())
    }

    /// Whether the rows are already in the pre-order `finish` from row 0
    /// would leave them in, each naming its parent as it stands: what
    /// `finish_in_order` takes on trust. A link out of range is not.
    pub fn is_in_order(&self) -> Result<bool> {
        let rows = &self.t.rows;
        if rows.is_empty() {
            return Ok(false);
        }
        let mut stack: Vec<(u32, u32)> = Vec::new();
        let mut n = 0usize;
        stack.push((0, NONE));
        while let Some((id, parent)) = stack.pop() {
            if id as usize >= rows.len() || id as usize != n || rows[id as usize].parent != parent {
                return Ok(false);
            }
            let r = rows[id as usize];
            if parent != NONE && r.next_sibling != NONE {
                stack.push((r.next_sibling, parent));
            }
            n += 1;
            if table::Kind::is_container(r.kind) && r.first_child != NONE {
                stack.push((r.first_child, id));
            }
        }
        Ok(n == rows.len())
    }

    /// Whether every link in the rows names a row or none, so that `finish`
    /// can walk them.
    pub fn links_in_range(&self) -> bool {
        let len = self.t.rows.len() as u32;
        self.t.rows.iter().all(|r| {
            !(r.first_child != NONE && r.first_child >= len) && !(r.next_sibling != NONE && r.next_sibling >= len)
        })
    }

    /// `finish` for a parser that wrote its rows in pre-order from the root
    /// at row 0, each naming its parent (`set_parent`): the rows are left as
    /// they are, and only the comments put in order. A debug build walks the
    /// tree as `finish` does and checks the claim.
    pub fn finish_in_order(&mut self) -> Result<()> {
        debug_assert!(self.is_in_order()?);
        self.last_trailing();
        Ok(())
    }

    /// Sort the comments by node and slot, keeping their order within a
    /// slot, and keep only the last trailing comment a node was given: a
    /// parser that sets one twice means the second.
    fn last_trailing(&mut self) {
        let cs = &mut self.t.comments;
        cs.sort_by_key(|c| (c.node, c.slot));
        let mut w = 0usize;
        for i in 0..cs.len() {
            let c = cs[i];
            if c.slot == table::COMMENT_TRAILING {
                if let Some(nx) = cs.get(i + 1) {
                    if nx.node == c.node && nx.slot == c.slot {
                        continue;
                    }
                }
            }
            cs[w] = c;
            w += 1;
        }
        cs.truncate(w);
    }

    /// The `Document` a parser's finished table is, adopted. A parser writes
    /// a well-formed tree, so a refusal is a bug in it (a panic, as in Zig).
    pub fn adopt(source: Arc<[u8]>, t: &mut Table) -> Result<Document> {
        let ast = match AST::adopt(source.clone(), t, ast::Spans::Required) {
            Ok(a) => a,
            Err(e) if e.is("OutOfMemory") => return Err(e),
            Err(e) => panic!("a parser wrote a malformed table: {e:?}"),
        };
        Ok(Document { source, ast })
    }
}

/// Move each fact in `list` to its node's new row, dropping a fact whose
/// node the tree does not reach, and leave the list in node order, a node's
/// facts in the order they were written.
fn remap<F: table::Fact>(list: &mut Vec<F>, place: &[u32]) {
    let mut w = 0usize;
    for i in 0..list.len() {
        let f = list[i];
        let node = f.node() as usize;
        if node >= place.len() || place[node] == NONE {
            continue;
        }
        let mut g = f;
        g.set_node(place[node]);
        list[w] = g;
        w += 1;
    }
    list.truncate(w);
    list.sort_by_key(|f| f.node());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::KeyValue;

    #[test]
    fn finish_puts_rows_written_out_of_order_into_pre_order_facts_and_all() {
        let src: Arc<[u8]> = Arc::from(&b"{a: [x]}"[..]);
        let mut t = Table::init(src.clone());
        {
            let mut w = RowWriter::init(&mut t);
            // Leaves first, as a bottom-up parser writes them.
            let x = w.add(NodeKind::String(&src[5..6]), Span::new(5, 6)).unwrap();
            let seq = w.add(NodeKind::Sequence(Some(x)), Span::new(4, 7)).unwrap();
            let key = w.add(NodeKind::String(&src[1..2]), Span::new(1, 2)).unwrap();
            let kv = w.add(NodeKind::KeyValue(KeyValue { key, value: seq }), Span::new(1, 7)).unwrap();
            let map = w.add(NodeKind::Mapping(Some(kv)), Span::new(0, 8)).unwrap();
            let orphan = w.add(NodeKind::Null, Span::new(0, 0)).unwrap();
            w.marker(x, Span::new(4, 5)).unwrap();
            w.comment(orphan, table::COMMENT_LEADING, Comment::line(b"gone")).unwrap();
            w.finish(map).unwrap();
        }
        let ast = AST::adopt(src, &mut t, ast::Spans::Required).unwrap();
        assert_eq!(5, ast.node_count());
        let entry = ast.node(ast.node(0).kind.as_mapping().unwrap().unwrap()).kind.as_keyvalue().unwrap();
        assert_eq!(b"a", ast.node(entry.key).kind.as_string().unwrap());
        let item = ast.node(entry.value).kind.as_sequence().unwrap().unwrap();
        assert_eq!(b"x", ast.node(item).kind.as_string().unwrap());
        assert_eq!(1, ast.marker_facts().len());
        assert_eq!(item, ast.marker_facts()[0].node);
        assert!(!ast.has_comments());
    }

    #[test]
    fn finish_writes_a_node_linked_twice_once_and_ends_a_cycle_in_the_links() {
        let mut t = Table::default();
        let mut w = RowWriter::init(&mut t);
        let a = w.add(NodeKind::Null, Span::new(0, 0)).unwrap();
        let b = w.add(NodeKind::Null, Span::new(0, 0)).unwrap();
        let seq = w.add(NodeKind::Sequence(Some(a)), Span::new(0, 0)).unwrap();
        w.set_next(a, Some(b));
        w.set_next(b, Some(a));
        assert!(w.links_in_range());
        w.finish(seq).unwrap();
        assert_eq!(3, w.t.rows.len());
        assert!(w.is_in_order().unwrap());
    }
}
