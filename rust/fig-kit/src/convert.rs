//! Port of kit/src/convert.zig.
//!
//! The node table and `Document`, each made from the other: the table a
//! language's parse fills becomes a `Document` (`table_to_document`, which
//! adopts it), a print's table is read (`view_to_document`), and a
//! `Document` becomes the table a print reads (`document_to_table`) or a
//! parse would have filled (`full_table`). The kit's, so that a language
//! built apart from core can read the table it is handed back.

use std::sync::Arc;

use super::ast::{Node, NodeId, NodeKind, NumberKind, Tag, AST};
use super::document::Document;
use super::table::{self, Table, NONE as NO_NODE};
use crate::Result;

pub use super::ast::Spans;

/// `TableError`: `UnknownKind`, `MalformedTable`, `OutOfMemory`.
pub use super::ast::TABLE_ERRORS;

/// A `Document` over `source` from a parse's table, which is checked and
/// adopted as the tree's storage (`AST::adopt`): nothing is copied, and `t`
/// is left empty.
pub fn table_to_document(source: Arc<[u8]>, t: &mut Table) -> Result<Document> {
    let ast = AST::adopt(source.clone(), t, Spans::Required)?;
    Ok(Document { source, ast })
}

/// A `Document` over the table a print is handed (`AST::borrow`, which
/// copies it here). Its rows carry no spans.
pub fn view_to_document(source: Arc<[u8]>, v: &table::View<'_>) -> Result<Document> {
    let ast = AST::borrow(source.clone(), v)?;
    Ok(Document { source, ast })
}

/// The table a `print` receives: `ast` from `root` in pre-order, every text
/// in the table's buffer, spans none, the child links filled. The document's
/// tag directives ride along only when `root` is the document's own root: a
/// fragment printed for a splice has no directives prefix, as a compiled
/// printer's `printNode` writes none.
pub fn document_to_table(ast: &AST, root: NodeId) -> Result<Table> {
    let mut t = Table::default();
    write(&mut t, ast, None, root)?;
    if root == ast.root {
        for d in ast.tag_directives() {
            let handle = t.add_string(d.handle)?;
            let prefix = t.add_string(d.prefix)?;
            t.directives.push(table::Directive { handle, prefix });
        }
    }
    Ok(t)
}

/// The table a `parse` of `doc`'s source would have to fill: rows as
/// `document_to_table` builds them, plus every source-coupled fact the
/// document records — node spans, item markers, entry separators, item
/// separators, anchor and tag spans, header regions and name mentions. What
/// a twin of a compiled format is held to, row for row.
pub fn full_table(doc: &Document) -> Result<Table> {
    let mut t = Table::default();
    write(&mut t, &doc.ast, Some(doc), doc.ast.root)?;
    for d in doc.ast.tag_directives() {
        let handle = t.text(d.handle)?;
        let prefix = t.text(d.prefix)?;
        t.directives.push(table::Directive { handle, prefix });
    }
    Ok(t)
}

/// `ast` from `root`, in pre-order, into the empty table `t`, with parents
/// and child links; with `doc`, its spans and source-coupled facts too.
fn write(t: &mut Table, ast: &AST, doc: Option<&Document>, root: NodeId) -> Result<()> {
    debug_assert!(t.rows.is_empty());
    struct Item {
        id: NodeId,
        parent: u32,
    }
    // Pre-order: each node's row, and its parent's.
    let mut stack: Vec<Item> = Vec::new();
    let mut ids: Vec<NodeId> = Vec::new();
    let mut kids: Vec<NodeId> = Vec::new();
    stack.push(Item { id: root, parent: NO_NODE });
    // The last child row of each row, while its subtree is being written.
    let mut last: Vec<u32> = Vec::new();
    // The whole tree is every node, and its texts are at most the source
    // and the strings it decoded: room for all of it up front, rather than a
    // copy each time a list outgrows itself.
    if root == ast.root {
        let n = ast.node_count();
        t.rows.reserve(n);
        ids.reserve(n);
        last.reserve(n);
        t.strings.reserve(ast.source().len() + ast.strings().len());
        // A tree whose rows are already in an order the walk would keep —
        // each parent before its children, which follow one another as their
        // links do — is the table row for row, as a parse's always is.
        if doc.is_none() {
            last.resize(n, NO_NODE);
            if rows_in_order(t, ast, &mut last)? {
                ids.extend(0..n as NodeId);
                stack.pop();
            } else {
                t.rows.clear();
                t.strings.clear();
            }
            last.clear();
        }
    }
    while let Some(item) = stack.pop() {
        let index = t.rows.len() as u32;
        let n = ast.node(item.id);
        let row = row_of(t, &n, item.parent)?;
        t.rows.push(row);
        ids.push(item.id);
        last.push(NO_NODE);
        if item.parent != NO_NODE {
            let p = item.parent as usize;
            if last[p] != NO_NODE {
                t.rows[last[p] as usize].next_sibling = index;
            } else {
                t.rows[p].first_child = index;
            }
            last[p] = index;
        }
        // Children, pushed last-first so they pop in order.
        kids.clear();
        match n.kind {
            NodeKind::Sequence(first) | NodeKind::Mapping(first) => {
                let mut next = first;
                while let Some(c) = next {
                    kids.push(c);
                    next = ast.node(c).next_sibling;
                }
            }
            NodeKind::KeyValue(kv) => kids.extend_from_slice(&[kv.key, kv.value]),
            _ => {}
        }
        for &k in kids.iter().rev() {
            stack.push(Item { id: k, parent: index });
        }
    }

    for (i, &id) in ids.iter().enumerate() {
        let n = i as u32;
        if let Some(name) = ast.anchor_of(id) {
            let mut f = table::Anchor { node: n, name: t.text(name)?, ..Default::default() };
            if let Some(d) = doc {
                if let Some(s) = d.anchor_span(&ast.node(id)) {
                    f.start = s.start as u32;
                    f.end = s.end as u32;
                }
            }
            t.anchors.push(f);
        }
        if let Some(src) = ast.tag_fact(id) {
            // The tag as the tree states it: its text as written — a kind tag
            // with none spelled as YAML's core schema spells it, for a
            // language that writes tags as text — and its resolution. An
            // unstated core-schema spelling is read as the kind it names, and
            // goes out resolved.
            let mut f = table::Tag { node: n, resolved: src.resolved, agrees: src.agrees, ..Default::default() };
            let tag = ast.tag_of_fact(src);
            if f.resolved == table::TAG_UNSTATED {
                if let Tag::Kind(k) = tag {
                    f.resolved = table::TAG_KIND + k.ordinal();
                }
            }
            f.text = match ast.text_of(src.text) {
                Some(x) => t.text(x)?,
                None => t.text(tag.spelling())?,
            };
            if let Some(d) = doc {
                if let Some(s) = d.tag_span(&ast.node(id)) {
                    f.start = s.start as u32;
                    f.end = s.end as u32;
                }
            }
            t.tags.push(f);
        }
        let nc = ast.comments(id);
        for c in nc.leading.iter() {
            let text = t.text(c.text)?;
            t.comments.push(table::Comment { node: n, slot: table::COMMENT_LEADING, style: c.style.ordinal(), text });
        }
        if let Some(c) = nc.trailing {
            let text = t.text(c.text)?;
            t.comments.push(table::Comment { node: n, slot: table::COMMENT_TRAILING, style: c.style.ordinal(), text });
        }
        for c in nc.dangling.iter() {
            let text = t.text(c.text)?;
            t.comments.push(table::Comment { node: n, slot: table::COMMENT_DANGLING, style: c.style.ordinal(), text });
        }

        let Some(d) = doc else { continue };
        let node = ast.node(id);
        let sp = d.span(&node);
        t.rows[i].start = sp.start as u32;
        t.rows[i].end = sp.end as u32;
        if let Some(m) = d.marker_span(&node) {
            t.markers.push(table::SpanFact { node: n, start: m.start as u32, end: m.end as u32 });
        }
        if let Some(m) = d.sep_span(&node) {
            t.seps.push(table::SpanFact { node: n, start: m.start as u32, end: m.end as u32 });
        }
        if let Some(is) = d.item_sep(&node) {
            if is.open {
                t.rows[i].flags |= table::ROW_OPEN;
            }
            if let Some(x) = is.text {
                let text = t.text(x)?;
                t.item_seps.push(table::ItemSep { node: n, text });
            }
        }
        for r in d.regions_of(id) {
            t.regions.push(table::SpanFact { node: n, start: r.start, end: r.end });
        }
        for m in d.mentions_of(id) {
            t.mentions.push(table::Mention { node: n, start: m.span.start as u32, end: m.span.end as u32, kind: m.kind.byte() });
        }
    }
    Ok(())
}

/// `ast`'s rows into the empty table `t` as they stand, when the walk in
/// `write` would write them in that order: row 0 the root, each later row's
/// parent before it, and each the first child of its parent or the next
/// sibling of the one before it. Each row is as `row_of` makes it, its text
/// copied to the buffer. False, with `t` partly written, when they are not.
/// `last` is scratch, a slot per row.
fn rows_in_order(t: &mut Table, ast: &AST, last: &mut [u32]) -> Result<bool> {
    use table::Kind as K;
    let rows = ast.rows();
    if ast.root != 0 || rows.is_empty() || rows[0].parent != NO_NODE {
        return Ok(false);
    }
    last.fill(NO_NODE);
    for (i, r) in rows.iter().enumerate() {
        let index = i as u32;
        let mut row = table::Row { kind: r.kind, parent: r.parent, ..table::Row::default() };
        if i > 0 {
            let p = r.parent;
            if p == NO_NODE || p >= index {
                return Ok(false);
            }
            let p = p as usize;
            let linked = if last[p] != NO_NODE { rows[last[p] as usize].next_sibling } else { rows[p].first_child };
            if linked != index {
                return Ok(false);
            }
            if last[p] != NO_NODE {
                t.rows[last[p] as usize].next_sibling = index;
            } else {
                t.rows[p].first_child = index;
            }
            last[p] = index;
        }
        match r.kind {
            K::NULL | K::FALSE | K::TRUE | K::SEQUENCE | K::MAPPING | K::KEYVALUE => {}
            _ => row.text = t.add_string(ast.text_of(r.text).unwrap_or(b""))?,
        }
        t.rows.push(row);
    }
    // Every child the links name was reached in order: a row with no child
    // names none, and a last child links nowhere further.
    for (i, r) in rows.iter().enumerate() {
        let tail = if last[i] == NO_NODE { r.first_child } else { rows[last[i] as usize].next_sibling };
        if tail != NO_NODE {
            return Ok(false);
        }
    }
    Ok(true)
}

fn row_of(t: &mut Table, n: &Node<'_>, parent: u32) -> Result<table::Row> {
    use table::Kind as K;
    let mut row = table::Row { parent, ..table::Row::default() };
    match n.kind {
        NodeKind::Null => row.kind = K::NULL,
        NodeKind::Boolean(b) => row.kind = if b { K::TRUE } else { K::FALSE },
        NodeKind::String(x) => {
            row.kind = K::STRING;
            row.text = t.text(x)?;
        }
        NodeKind::Number(num) => {
            row.kind = if num.kind == NumberKind::Float { K::FLOAT } else { K::INT };
            row.text = t.text(num.raw)?;
        }
        NodeKind::Extended(e) => {
            row.kind = K::EXT + e.kind.ordinal();
            row.text = t.text(e.text)?;
        }
        NodeKind::Sequence(_) => row.kind = K::SEQUENCE,
        NodeKind::Mapping(_) => row.kind = K::MAPPING,
        NodeKind::KeyValue(_) => row.kind = K::KEYVALUE,
        NodeKind::Alias(x) => {
            row.kind = K::ALIAS;
            row.text = t.text(x)?;
        }
    }
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{self, Builder, Comment, Entry};

    #[test]
    fn document_to_table_round_trips_through_borrow() {
        let mut b = Builder::init();
        let one = b.add_int(1).unwrap();
        let x = b.add_string(b"x").unwrap();
        let seq = b.add_sequence(&[one, x]).unwrap();
        let k = b.add_string(b"a").unwrap();
        b.add_leading_comment(k, Comment::line(b"note")).unwrap();
        let root = b.add_mapping(&[Entry { key: k, value: seq }]).unwrap();
        let ast = b.finish(root).unwrap();
        let t = document_to_table(&ast, ast.root).unwrap();
        assert_eq!(table::NONE, t.rows[0].parent);
        let doc = view_to_document(Arc::from(&b""[..]), &t.view()).unwrap();
        assert!(doc.ast.same_tree(doc.ast.root, &ast, ast.root));
        let key = doc.ast.get_key_by_path(&[ast::PathSegment::Key(b"a")]).unwrap();
        assert_eq!(b"note", doc.ast.comments(key.id).leading[0].text);
    }
}
