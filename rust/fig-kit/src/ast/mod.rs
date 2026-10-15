//! Port of kit/src/ast/ast.zig.
//!
//! AST = Abstract Syntax Tree
//!
//! A document's values, as the node table holds them: one row per node
//! (`../table.rs`), a scalar's text in the source or in the table's string
//! buffer, and every rarer fact — anchors, tags, markers, separators,
//! regions, mentions, comments, directives — in a sparse list sorted by node.
//! The table a language's parse fills IS this storage: fig checks it and
//! adopts it (`adopt`), with no copy (docs/proposals/core-only.md §4.9).
//!
//! A reader asks for a node by id and gets a `Node` — a view, with its kind
//! as an enum — so a `match` over a node's kind stays exhaustive. Two
//! documents of different formats can have the same AST.
//!
//! This file owns the struct itself — its storage, the node model, the
//! comment layer, and equality. Navigation is `reader.rs` (more `impl AST`),
//! construction `builder.rs`, and a printer's options `options.rs`.
//!
//! **Ownership (Rust).** An `AST` always owns its table (`storage`), whose
//! source is an `Arc<[u8]>` shared with the `Document`. Zig's `borrow`,
//! which reads a print's view in place, copies it here. Zig's AST decodes
//! the comment and directive lists into slices once; here the comment runs
//! are decoded once into texts by reference (`Text`), and `comments(id)`
//! hands out `Comment`s borrowing the table.

pub mod builder;
pub mod options;
pub mod reader;

use std::borrow::Cow;
use std::sync::Arc;

use super::span::Span;
use super::table::{self, Row, Table, NONE};
use crate::err;
use crate::Result;

pub use builder::{Builder, Entry};
pub use options::{SerializeOptions, SERIALIZE_ERRORS};
pub use reader::PathSegment;

/// `Node.Id`: unique number identifier for a node, its row.
pub type NodeId = u32;

/// One run of `comment_list`: node `node`'s comments in `slot`,
/// `comment_list[start..][..len]`. Sorted by `(node, slot)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommentRun {
    pub node: NodeId,
    pub slot: u8,
    pub start: u32,
    pub len: u32,
}

/// A decoded comment: its text, by reference into the table, and style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DecodedComment {
    /// Absent reads as the empty text.
    text: table::Text,
    style: Style,
}

#[derive(Debug, Clone)]
pub struct AST {
    /// The node every other is under. Row 0 for a parse's table; a tree a
    /// caller built may root anywhere.
    pub root: NodeId,
    /// The key whose value is merged into its mapping (YAML's `<<`), from the
    /// language's `Syntax.merge_key`, or `None` when the language has no
    /// merge. It lives as long as the language's registration.
    pub merge_key: Option<&'static [u8]>,
    /// The table the rows and lists live in. Row `i` is node `i`. A row's
    /// `first_child` and `next_sibling` link the tree: a container's children
    /// are a sibling chain, and a keyvalue's two children are its key and
    /// then its value. Every sparse list is sorted by node, so a node's fact
    /// is a binary search; a list is empty when the document has no such
    /// fact. Its `source` is what a row's in-source text indexes: empty for a
    /// tree built with no source, whose every text is in `strings`.
    pub storage: Table,
    /// `comment_list` grouped by node.
    comment_runs: Vec<CommentRun>,
    comment_list: Vec<DecodedComment>,
}

/// Row `r`'s kind, its texts in `source` or `strings` and its children in
/// `rows`: what `node(id).kind` reads, for a table still being written.
pub fn row_kind<'a>(source: &'a [u8], strings: &'a [u8], rows: &[Row], r: Row) -> NodeKind<'a> {
    use table::Kind as K;
    let text = table::view_text(source, strings, r.text).unwrap_or(b"");
    match r.kind {
        K::NULL => NodeKind::Null,
        K::FALSE => NodeKind::Boolean(false),
        K::TRUE => NodeKind::Boolean(true),
        K::STRING => NodeKind::String(text),
        K::INT => NodeKind::Number(Number { raw: text, kind: NumberKind::Integer }),
        K::FLOAT => NodeKind::Number(Number { raw: text, kind: NumberKind::Float }),
        K::SEQUENCE => NodeKind::Sequence(opt(r.first_child)),
        K::MAPPING => NodeKind::Mapping(opt(r.first_child)),
        K::KEYVALUE => NodeKind::KeyValue(KeyValue { key: r.first_child, value: rows[r.first_child as usize].next_sibling }),
        K::ALIAS => NodeKind::Alias(text),
        k => NodeKind::Extended(Extended {
            kind: ExtKind::from_ordinal(k.wrapping_sub(K::EXT)).expect("a row of an unknown kind"),
            text,
        }),
    }
}

fn opt(x: u32) -> Option<u32> {
    if x == NONE { None } else { Some(x) }
}

impl AST {
    // ── The table ───────────────────────────────────────────────────────────

    /// What a row's in-source text indexes.
    pub fn source(&self) -> &[u8] {
        &self.storage.source
    }

    pub fn rows(&self) -> &[Row] {
        &self.storage.rows
    }

    /// The decoded text rows name with `Text`'s high bit set.
    pub fn strings(&self) -> &[u8] {
        &self.storage.strings
    }

    pub fn anchor_facts(&self) -> &[table::Anchor] {
        &self.storage.anchors
    }

    pub fn tag_facts(&self) -> &[table::Tag] {
        &self.storage.tags
    }

    pub fn marker_facts(&self) -> &[table::SpanFact] {
        &self.storage.markers
    }

    pub fn sep_facts(&self) -> &[table::SpanFact] {
        &self.storage.seps
    }

    pub fn item_sep_facts(&self) -> &[table::ItemSep] {
        &self.storage.item_seps
    }

    /// Sorted by `(node, start)`.
    pub fn region_facts(&self) -> &[table::SpanFact] {
        &self.storage.regions
    }

    /// Sorted by `(node, start)`.
    pub fn mention_facts(&self) -> &[table::Mention] {
        &self.storage.mentions
    }

    /// The comment runs, grouped by node and slot.
    pub fn comment_runs(&self) -> &[CommentRun] {
        &self.comment_runs
    }

    /// Tag-handle definitions in source order — a YAML `%TAG` directive's
    /// handle and the prefix it expands to. Empty when the document declares
    /// none.
    pub fn tag_directives(&self) -> Vec<TagDirective<'_>> {
        self.storage
            .directives
            .iter()
            .map(|d| TagDirective { handle: self.text_or(d.handle, b""), prefix: self.text_or(d.prefix, b"") })
            .collect()
    }

    /// Drop the comment layer, as a printer asked to `strip_comments` is
    /// handed it (Zig's `commentView` blanks `comment_runs`/`comment_list`).
    pub fn strip_comments(&mut self) {
        self.comment_runs.clear();
        self.comment_list.clear();
    }

    // ── Reading a node ──────────────────────────────────────────────────────

    /// How many nodes the tree has. Ids run from 0 to this, exclusive.
    pub fn node_count(&self) -> usize {
        self.storage.rows.len()
    }

    /// Node `id`, as a view: its kind with its payload, and its next sibling
    /// in its container. A keyvalue's key and value have none; the entry
    /// names them.
    pub fn node(&self, id: NodeId) -> Node<'_> {
        let r = self.storage.rows[id as usize];
        Node { id, kind: self.kind_of_row(r), next_sibling: self.sibling_of_row(r) }
    }

    /// Node `id`'s kind: `node(id).kind`.
    pub fn kind_of(&self, id: NodeId) -> NodeKind<'_> {
        self.kind_of_row(self.storage.rows[id as usize])
    }

    fn sibling_of_row(&self, r: Row) -> Option<NodeId> {
        if r.next_sibling == NONE {
            return None;
        }
        if r.parent != NONE && self.storage.rows[r.parent as usize].kind == table::Kind::KEYVALUE {
            return None;
        }
        Some(r.next_sibling)
    }

    fn kind_of_row(&self, r: Row) -> NodeKind<'_> {
        row_kind(&self.storage.source, &self.storage.strings, &self.storage.rows, r)
    }

    /// Row `r`'s kind, for a table still being written. See `row_kind`.
    pub fn row_kind<'a>(source: &'a [u8], strings: &'a [u8], rows: &[Row], r: Row) -> NodeKind<'a> {
        row_kind(source, strings, rows, r)
    }

    /// The bytes `t` names, or `None` when it is absent.
    pub fn text_of(&self, t: table::Text) -> Option<&[u8]> {
        table::view_text(&self.storage.source, &self.storage.strings, t)
    }

    fn text_or<'a>(&'a self, t: table::Text, default: &'a [u8]) -> &'a [u8] {
        self.text_of(t).unwrap_or(default)
    }

    /// Node `id`'s parent, or `None` for a root.
    pub fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        opt(self.storage.rows[id as usize].parent)
    }

    /// Node `id`'s span in the source. A node with none — a tree built with
    /// no source — is the empty span at 0.
    pub fn span_of(&self, id: NodeId) -> Span {
        let r = self.storage.rows[id as usize];
        if r.start == NONE {
            return Span::new(0, 0);
        }
        Span::new(r.start as usize, r.end as usize)
    }

    // ── Side facts ──────────────────────────────────────────────────────────

    /// The type tag on node `id`, or `None`: a kind tag where the tag names a
    /// core type in the spelling every format reads (`!!int`, or no text at
    /// all), and the text as written otherwise. A tag the language resolved
    /// to a core type but spelled otherwise (`!<tag:yaml.org,2002:int>`) is
    /// its text, which a printer re-emits; `tag_fact` has the resolution. A
    /// tag the language left unstated is read by its text, the seven
    /// core-schema spellings as the kinds they name.
    pub fn tag_of(&self, id: NodeId) -> Option<Tag<'_>> {
        let f = self.tag_fact(id)?;
        Some(self.tag_of_fact(f))
    }

    /// Node `id`'s tag as the table states it: its text, its resolution, and
    /// whether the node agrees with the resolution.
    pub fn tag_fact(&self, id: NodeId) -> Option<table::Tag> {
        table::fact_of(&self.storage.tags, id)
    }

    /// A tag fact as a `Tag`; see `tag_of`.
    pub fn tag_of_fact(&self, f: table::Tag) -> Tag<'_> {
        let text = self.text_of(f.text);
        if f.resolved >= table::TAG_KIND {
            let k = KindTag::from_ordinal(f.resolved - table::TAG_KIND).expect("a tag of an unknown kind");
            let Some(x) = text else { return Tag::Kind(k) };
            if x == Tag::Kind(k).spelling() {
                return Tag::Kind(k);
            }
            return Tag::Text(x);
        }
        if f.resolved == table::TAG_UNSTATED {
            if let Some(k) = Tag::kind_of_spelling(text.unwrap_or(b"")) {
                return Tag::Kind(k);
            }
        }
        Tag::Text(text.unwrap_or(b""))
    }

    /// The anchor name defined on node `id` (no leading `&`), or `None`.
    pub fn anchor_of(&self, id: NodeId) -> Option<&[u8]> {
        let f = table::fact_of(&self.storage.anchors, id)?;
        self.text_of(f.name)
    }

    /// The comments bound to `id`; empty for a node with none.
    pub fn comments(&self, id: NodeId) -> NodeComments<'_> {
        let runs = &self.comment_runs;
        let mut lo = 0usize;
        let mut hi = runs.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if runs[mid].node < id {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut out = NodeComments::default();
        while lo < runs.len() && runs[lo].node == id {
            let r = runs[lo];
            let items: Vec<Comment<'_>> = self.comment_list[r.start as usize..(r.start + r.len) as usize]
                .iter()
                .map(|c| Comment { text: if c.text.is_absent() { b"" } else { self.text_or(c.text, b"") }, style: c.style })
                .collect();
            match r.slot {
                table::COMMENT_LEADING => out.leading = Cow::Owned(items),
                table::COMMENT_TRAILING => out.trailing = Some(items[0]),
                _ => out.dangling = Cow::Owned(items),
            }
            lo += 1;
        }
        out
    }

    /// Whether the tree carries any comment.
    pub fn has_comments(&self) -> bool {
        !self.comment_list.is_empty()
    }

    /// Whether the tree carries any type tag.
    pub fn has_tags(&self) -> bool {
        !self.storage.tags.is_empty()
    }

    /// Whether the tree defines any anchor.
    pub fn has_anchors(&self) -> bool {
        !self.storage.anchors.is_empty()
    }

    /// The anchors defined, in definition order — by node, which is source
    /// order.
    pub fn anchor_iterator(&self) -> AnchorIterator<'_> {
        AnchorIterator { ast: self, i: 0 }
    }

    /// The node a container child's *leading* comment binds to: the key of a
    /// `keyvalue` entry (so the comment renders above the key), else the
    /// child node itself (a sequence element). Shared by every printer that
    /// emits comments.
    pub fn leading_comment_anchor(&self, id: NodeId) -> NodeId {
        match self.kind_of(id) {
            NodeKind::KeyValue(kv) => kv.key,
            _ => id,
        }
    }

    /// The node a container child's *trailing* comment binds to: the value of
    /// a `keyvalue` entry (so the comment renders after the value), else the
    /// child node itself.
    pub fn trailing_comment_anchor(&self, id: NodeId) -> NodeId {
        match self.kind_of(id) {
            NodeKind::KeyValue(kv) => kv.value,
            _ => id,
        }
    }

    /// An AST of `nodes`, given as `Node` values (`nodes[i].id == i`), rooted
    /// at `root`: for a test or a caller that spells a small tree out by hand.
    /// The nodes' strings are copied.
    pub fn from_nodes(root: NodeId, nodes: &[Node<'_>]) -> Result<AST> {
        use table::Kind as K;
        let mut t = Table::default();
        t.add_rows(nodes.len())?;
        for (i, n) in nodes.iter().enumerate() {
            let mut r = Row::default();
            match n.kind {
                NodeKind::Null => r.kind = K::NULL,
                NodeKind::Boolean(b) => r.kind = if b { K::TRUE } else { K::FALSE },
                NodeKind::String(x) => {
                    r.kind = K::STRING;
                    r.text = t.add_string(x)?;
                }
                NodeKind::Number(num) => {
                    r.kind = if num.kind == NumberKind::Float { K::FLOAT } else { K::INT };
                    r.text = t.add_string(num.raw)?;
                }
                NodeKind::Extended(e) => {
                    r.kind = K::EXT + e.kind.ordinal();
                    r.text = t.add_string(e.text)?;
                }
                NodeKind::Alias(x) => {
                    r.kind = K::ALIAS;
                    r.text = t.add_string(x)?;
                }
                NodeKind::Sequence(first) => {
                    r.kind = K::SEQUENCE;
                    r.first_child = first.unwrap_or(NONE);
                }
                NodeKind::Mapping(first) => {
                    r.kind = K::MAPPING;
                    r.first_child = first.unwrap_or(NONE);
                }
                NodeKind::KeyValue(kv) => {
                    r.kind = K::KEYVALUE;
                    r.first_child = kv.key;
                }
            }
            t.rows[i] = r;
        }
        for (i, n) in nodes.iter().enumerate() {
            match n.kind {
                NodeKind::Sequence(first) | NodeKind::Mapping(first) => {
                    let mut c = first;
                    while let Some(id) = c {
                        t.rows[id as usize].parent = i as u32;
                        t.rows[id as usize].next_sibling = nodes[id as usize].next_sibling.unwrap_or(NONE);
                        c = nodes[id as usize].next_sibling;
                    }
                }
                NodeKind::KeyValue(kv) => {
                    t.rows[kv.key as usize].parent = i as u32;
                    t.rows[kv.key as usize].next_sibling = kv.value;
                    t.rows[kv.value as usize].parent = i as u32;
                    t.rows[kv.value as usize].next_sibling = NONE;
                }
                _ => {}
            }
        }
        AST::own(&mut t, root, None)
    }

    /// Whether the subtree at `id` here and the one at `other_id` in `other`
    /// hold the same values in the same shape — kinds, scalar payloads,
    /// children in order — whatever their ids. Tags, anchors and comments are
    /// not compared.
    pub fn same_tree(&self, id: NodeId, other: &AST, other_id: NodeId) -> bool {
        let a = self.kind_of(id);
        let b = other.kind_of(other_id);
        if a.tag() != b.tag() {
            return false;
        }
        match a {
            NodeKind::Sequence(first) | NodeKind::Mapping(first) => {
                let mut x = first;
                let mut y = match b {
                    NodeKind::Sequence(f) | NodeKind::Mapping(f) => f,
                    _ => unreachable!(),
                };
                while let (Some(xi), Some(yi)) = (x, y) {
                    if !self.same_tree(xi, other, yi) {
                        return false;
                    }
                    x = self.node(xi).next_sibling;
                    y = other.node(yi).next_sibling;
                }
                x.is_none() && y.is_none()
            }
            NodeKind::KeyValue(kv) => {
                let NodeKind::KeyValue(okv) = b else { unreachable!() };
                self.same_tree(kv.key, other, okv.key) && self.same_tree(kv.value, other, okv.value)
            }
            _ => a.eql(&b),
        }
    }

    /// Set node `id`'s type tag, replacing any it had. A kind tag is spelled
    /// as YAML's core schema spells it.
    pub fn set_tag(&mut self, id: NodeId, tag: Tag<'_>) -> Result<()> {
        let t = &mut self.storage;
        let f = match tag {
            Tag::Text(x) => table::Tag { node: id, text: t.add_string(x)?, ..Default::default() },
            Tag::Kind(k) => table::Tag {
                node: id,
                text: t.add_string(tag.spelling())?,
                resolved: table::TAG_KIND + k.ordinal(),
                ..Default::default()
            },
        };
        t.tags.push(f);
        sort_facts(t);
        Ok(())
    }

    // ── Making one ──────────────────────────────────────────────────────────

    /// The AST a parse's table is, over `source`: the table is checked — row
    /// 0 is the root and the only row without a parent, a parent precedes its
    /// children, a mapping's children are keyvalues, a keyvalue has exactly a
    /// key and a value, a scalar has no children, every span and text lies in
    /// range, every side fact names a row — its child links are derived, and
    /// it becomes the AST's storage. `t` is emptied (keeping its source),
    /// whatever the outcome; nothing it held is copied.
    ///
    /// Errors (`TableError`): `UnknownKind` — a row's `kind` is not a value
    /// core knows; `MalformedTable` — a parent index past the table, a child
    /// before its parent, a keyvalue without exactly a key and a value, a
    /// mapping child that is not a keyvalue, a scalar with children, a row
    /// with no span, a second root, a side fact naming no row, a text outside
    /// the source and the buffer; `OutOfMemory`.
    pub fn adopt(source: Arc<[u8]>, t: &mut Table, spans_rule: Spans) -> Result<AST> {
        let keep = t.source.clone();
        let mut storage = std::mem::replace(t, Table::init(keep));
        storage.source = source;
        check(&mut storage, spans_rule)?;
        let mut ast = AST { root: 0, merge_key: None, storage, comment_runs: Vec::new(), comment_list: Vec::new() };
        ast.decode_lists()?;
        Ok(ast)
    }

    /// The AST a caller built into `t`, which becomes its storage: rows whose
    /// links the caller set, in any order, rooted at `root`. The side lists
    /// are sorted by node; nothing else is checked. `t` is emptied.
    pub fn own(t: &mut Table, root: NodeId, merge_key: Option<&'static [u8]>) -> Result<AST> {
        let keep = t.source.clone();
        let mut storage = std::mem::replace(t, Table::init(keep));
        sort_facts(&mut storage);
        let mut ast = AST { root, merge_key, storage, comment_runs: Vec::new(), comment_list: Vec::new() };
        match ast.decode_lists() {
            Ok(()) => {}
            Err(e) if e.is("OutOfMemory") => return Err(e),
            // A caller's comments name its own rows.
            Err(e) => panic!("AST::own: {e:?}"),
        }
        Ok(ast)
    }

    /// An AST over a table fig filled and linked — what a `print` is handed —
    /// read as it stands, with no check of the parse's rules. Zig reads the
    /// view in place; this copies its lists.
    pub fn borrow(source: Arc<[u8]>, v: &table::View<'_>) -> Result<AST> {
        let rows = v.row_slice();
        if rows.is_empty() {
            return Err(err!(MalformedTable));
        }
        let n = rows.len() as u32;
        for r in rows {
            if r.first_child != NONE && r.first_child >= n {
                return Err(err!(MalformedTable));
            }
            if r.next_sibling != NONE && r.next_sibling >= n {
                return Err(err!(MalformedTable));
            }
            if r.parent != NONE && r.parent >= n {
                return Err(err!(MalformedTable));
            }
            if r.kind == table::Kind::KEYVALUE
                && (r.first_child == NONE || rows[r.first_child as usize].next_sibling == NONE)
            {
                return Err(err!(MalformedTable));
            }
            if r.kind > table::Kind::ALIAS && r.kind < table::Kind::EXT {
                return Err(err!(UnknownKind));
            }
            if r.kind >= table::Kind::EXT && (r.kind - table::Kind::EXT) as usize >= EXT_KINDS {
                return Err(err!(UnknownKind));
            }
        }
        let storage = Table {
            source,
            rows: rows.to_vec(),
            strings: v.strings.to_vec(),
            anchors: v.anchors.to_vec(),
            tags: v.tags.to_vec(),
            markers: v.markers.to_vec(),
            seps: v.seps.to_vec(),
            item_seps: v.item_seps.to_vec(),
            comments: v.comments.to_vec(),
            regions: v.regions.to_vec(),
            mentions: v.mentions.to_vec(),
            directives: v.directives.to_vec(),
            diagnostics: v.diagnostics.to_vec(),
        };
        let mut ast = AST { root: 0, merge_key: None, storage, comment_runs: Vec::new(), comment_list: Vec::new() };
        ast.decode_lists()?;
        Ok(ast)
    }

    /// Decode the comment and directive lists. Comments must be sorted by
    /// node, and within a node by slot, keeping source order within a slot.
    fn decode_lists(&mut self) -> Result<()> {
        let cs = &self.storage.comments;
        let n = self.storage.rows.len();
        if !cs.is_empty() {
            let mut list: Vec<DecodedComment> = Vec::with_capacity(cs.len());
            let mut runs: Vec<CommentRun> = Vec::new();
            for (i, c) in cs.iter().enumerate() {
                if c.node as usize >= n {
                    return Err(err!(MalformedTable));
                }
                if !c.text.is_absent() && self.text_of(c.text).is_none() {
                    return Err(err!(MalformedTable));
                }
                let style = match c.style {
                    table::COMMENT_LINE => Style::Line,
                    table::COMMENT_BLOCK => Style::Block,
                    _ => return Err(err!(MalformedTable)),
                };
                list.push(DecodedComment { text: c.text, style });
                if c.slot > table::COMMENT_DANGLING {
                    return Err(err!(MalformedTable));
                }
                match runs.last_mut() {
                    Some(last) if last.node == c.node && last.slot == c.slot => {
                        // At most one trailing comment per node.
                        if c.slot == table::COMMENT_TRAILING {
                            return Err(err!(MalformedTable));
                        }
                        last.len += 1;
                    }
                    last => {
                        if let Some(l) = last {
                            if l.node > c.node || (l.node == c.node && l.slot > c.slot) {
                                return Err(err!(MalformedTable));
                            }
                        }
                        runs.push(CommentRun { node: c.node, slot: c.slot, start: i as u32, len: 1 });
                    }
                }
            }
            self.comment_runs = runs;
            self.comment_list = list;
        }
        for d in &self.storage.directives {
            if self.text_of(d.handle).is_none() || self.text_of(d.prefix).is_none() {
                return Err(err!(MalformedTable));
            }
        }
        Ok(())
    }

    /// Compare two ASTs' comment layers. Separate from `eql` (which ignores
    /// comments) so round-trip tests can assert comments survived.
    pub fn comments_eql(&self, b: &AST) -> bool {
        let n = self.node_count().max(b.node_count());
        for i in 0..n as NodeId {
            let x = if (i as usize) < self.node_count() { self.comments(i) } else { NodeComments::default() };
            let y = if (i as usize) < b.node_count() { b.comments(i) } else { NodeComments::default() };
            if !x.eql(&y) {
                return false;
            }
        }
        true
    }

    /// Compare two ASTs' type-tag layers. Separate from `eql` (which ignores
    /// tags, like comments) so round-trip tests can assert an explicit
    /// annotation survived.
    pub fn tags_eql(&self, b: &AST) -> bool {
        let n = self.node_count().max(b.node_count());
        for i in 0..n as NodeId {
            let x = if (i as usize) < self.node_count() { self.tag_of(i) } else { None };
            let y = if (i as usize) < b.node_count() { b.tag_of(i) } else { None };
            if x.is_none() != y.is_none() {
                return false;
            }
            if let (Some(xt), Some(yt)) = (x, y) {
                if !xt.eql(&yt) {
                    return false;
                }
            }
        }
        true
    }

    /// Whether two documents are equal abstractly (does not compare source
    /// text): the same root, and node for node the same kind, payload and
    /// sibling.
    pub fn eql(&self, b: &AST) -> bool {
        if self.root != b.root || self.node_count() != b.node_count() {
            return false;
        }
        (0..self.node_count()).all(|i| self.node(i as NodeId).eql(&b.node(i as NodeId)))
    }
}

/// The number of `ExtKind`s.
const EXT_KINDS: usize = ExtKind::ALL.len();

/// Sort each side list by node, as the AST reads them. Single-valued facts
/// keep the last stated for a node, as a later statement replaces an
/// earlier; comments keep their order within a node and slot.
fn sort_facts(t: &mut Table) {
    dedupe_last(&mut t.anchors);
    dedupe_last(&mut t.tags);
    dedupe_last(&mut t.markers);
    dedupe_last(&mut t.seps);
    dedupe_last(&mut t.item_seps);
    sort_by(&mut t.regions, |x, y| x.node < y.node || (x.node == y.node && x.start < y.start));
    sort_by(&mut t.mentions, |x, y| x.node < y.node || (x.node == y.node && x.start < y.start));
    sort_by(&mut t.comments, |x, y| x.node < y.node || (x.node == y.node && x.slot < y.slot));
}

/// Stable-sort `items` by `lt`, unless it is sorted already.
fn sort_by<T>(items: &mut [T], lt: impl Fn(&T, &T) -> bool) {
    let sorted = items.windows(2).all(|w| !lt(&w[1], &w[0]));
    if !sorted {
        items.sort_by(|a, b| {
            if lt(a, b) {
                std::cmp::Ordering::Less
            } else if lt(b, a) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
    }
}

/// Sort a single-valued fact list by node, keeping the last fact stated
/// for each node.
fn dedupe_last<T: table::Fact>(list: &mut Vec<T>) {
    sort_by(list, |x, y| x.node() < y.node());
    let mut w = 0usize;
    for i in 0..list.len() {
        let f = list[i];
        if i + 1 < list.len() && list[i + 1].node() == f.node() {
            continue;
        }
        list[w] = f;
        w += 1;
    }
    list.truncate(w);
}

/// Check a parse's table and derive its child links, in place.
fn check(t: &mut Table, spans_rule: Spans) -> Result<()> {
    use table::Kind as K;
    // The empty document is spelled as one null row by the format; a table
    // with no rows at all is refused as the ambiguity it is.
    if t.rows.is_empty() {
        return Err(err!(MalformedTable));
    }
    let source: &[u8] = &t.source;
    let strings: &[u8] = &t.strings;
    let rows = &mut t.rows;

    // Rows: kinds, texts, spans, and the parent order — a parent precedes
    // its child (pre-order) and only row 0 has none. Each child is linked
    // after its parent's last, which `last` holds while the parent's
    // subtree is being read.
    let mut last = vec![NONE; rows.len()];
    for i in 0..rows.len() {
        let r = rows[i];
        if i == 0 {
            if r.parent != NONE {
                return Err(err!(MalformedTable));
            }
        } else if r.parent == NONE || r.parent as usize >= i {
            return Err(err!(MalformedTable));
        }
        if !(spans_rule == Spans::Absent && r.start == NONE) {
            check_span(source, r.start, r.end)?;
        }
        if r.flags & !table::ROW_OPEN != 0 {
            return Err(err!(MalformedTable));
        }
        if r.flags & table::ROW_OPEN != 0 && r.kind != K::SEQUENCE {
            return Err(err!(MalformedTable));
        }
        match r.kind {
            K::NULL | K::FALSE | K::TRUE | K::SEQUENCE | K::MAPPING | K::KEYVALUE => {}
            K::INT | K::FLOAT | K::ALIAS => {
                if table::view_text(source, strings, r.text).is_none() {
                    return Err(err!(MalformedTable));
                }
            }
            K::STRING => opt_text(source, strings, r.text)?,
            k => {
                if k < K::EXT || (k - K::EXT) as usize >= EXT_KINDS {
                    return Err(err!(UnknownKind));
                }
                opt_text(source, strings, r.text)?;
            }
        }
        rows[i].first_child = NONE;
        rows[i].next_sibling = NONE;
        if i == 0 {
            continue;
        }
        let p = r.parent as usize;
        let pk = rows[p].kind;
        match pk {
            K::SEQUENCE => {
                if r.kind == K::KEYVALUE {
                    return Err(err!(MalformedTable));
                }
            }
            K::MAPPING => {
                if r.kind != K::KEYVALUE {
                    return Err(err!(MalformedTable));
                }
            }
            K::KEYVALUE => {
                if r.kind == K::KEYVALUE {
                    return Err(err!(MalformedTable));
                }
            }
            _ => return Err(err!(MalformedTable)),
        }
        if last[p] != NONE {
            rows[last[p] as usize].next_sibling = i as u32;
        } else {
            rows[p].first_child = i as u32;
        }
        last[p] = i as u32;
    }
    // A keyvalue has exactly two children: its key, then its value.
    for r in rows.iter() {
        if r.kind != K::KEYVALUE {
            continue;
        }
        if r.first_child == NONE {
            return Err(err!(MalformedTable));
        }
        let v = rows[r.first_child as usize].next_sibling;
        if v == NONE || rows[v as usize].next_sibling != NONE {
            return Err(err!(MalformedTable));
        }
    }

    // Side facts, each naming a row; spans in the source.
    let n = rows.len() as u32;
    let rows: &[Row] = rows;
    for f in &t.anchors {
        if f.node >= n || table::view_text(source, strings, f.name).is_none() {
            return Err(err!(MalformedTable));
        }
        opt_span(source, f.start, f.end)?;
    }
    for f in &t.tags {
        if f.node >= n {
            return Err(err!(MalformedTable));
        }
        if f.resolved >= table::TAG_KIND {
            if (f.resolved - table::TAG_KIND) as usize >= KindTag::ALL.len() {
                return Err(err!(MalformedTable));
            }
        } else if table::view_text(source, strings, f.text).is_none() {
            return Err(err!(MalformedTable));
        }
        opt_span(source, f.start, f.end)?;
    }
    for f in &t.markers {
        if f.node >= n {
            return Err(err!(MalformedTable));
        }
        check_span(source, f.start, f.end)?;
    }
    for f in &t.seps {
        if f.node >= n {
            return Err(err!(MalformedTable));
        }
        check_span(source, f.start, f.end)?;
    }
    for f in &t.item_seps {
        if f.node >= n || rows[f.node as usize].kind != K::SEQUENCE {
            return Err(err!(MalformedTable));
        }
        opt_text(source, strings, f.text)?;
    }
    for f in &t.regions {
        if f.node >= n {
            return Err(err!(MalformedTable));
        }
        check_span(source, f.start, f.end)?;
    }
    for f in &t.mentions {
        if f.node >= n || f.kind > table::MENTION_APPEND {
            return Err(err!(MalformedTable));
        }
        check_span(source, f.start, f.end)?;
    }
    sort_facts(t);
    Ok(())
}

/// `[start, end)` within `source`, required.
fn check_span(source: &[u8], start: u32, end: u32) -> Result<()> {
    if start == NONE || end == NONE || start > end || end as usize > source.len() {
        return Err(err!(MalformedTable));
    }
    Ok(())
}

/// `[start, end)` within `source`, or both `NONE`.
fn opt_span(source: &[u8], start: u32, end: u32) -> Result<()> {
    if start == NONE && end == NONE {
        return Ok(());
    }
    check_span(source, start, end)
}

/// A text that may be absent, but that names bytes in range if present.
fn opt_text(source: &[u8], strings: &[u8], t: table::Text) -> Result<()> {
    if t.is_absent() {
        return Ok(());
    }
    if table::view_text(source, strings, t).is_none() {
        return Err(err!(MalformedTable));
    }
    Ok(())
}

/// The anchors defined, in definition order (`AST::anchor_iterator`).
pub struct AnchorIterator<'a> {
    ast: &'a AST,
    i: usize,
}

impl<'a> Iterator for AnchorIterator<'a> {
    type Item = Anchor<'a>;

    fn next(&mut self) -> Option<Anchor<'a>> {
        let facts = &self.ast.storage.anchors;
        if self.i >= facts.len() {
            return None;
        }
        let f = facts[self.i];
        self.i += 1;
        Some(Anchor { name: self.ast.text_or(f.name, b""), node: f.node })
    }
}

/// `TableError`: the names `adopt` and `borrow` return.
pub const TABLE_ERRORS: &[&str] = &["UnknownKind", "MalformedTable", "OutOfMemory"];

/// Whether a table's rows must carry spans: a parse's do, and a print's,
/// built from a tree with no source, do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spans {
    Required,
    Absent,
}

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor<'a> {
    pub name: &'a [u8],
    pub node: NodeId,
}

/// One `%TAG` directive: the handle it declares, including both `!`s (`!e!`,
/// or `!`/`!!` when a document redefines a default handle), and the prefix
/// that handle expands to. Both are stored verbatim, so re-emitting the
/// directive is a copy rather than a re-encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagDirective<'a> {
    pub handle: &'a [u8],
    pub prefix: &'a [u8],
}

/// A normalized, cross-format type identity — the companion to `NodeKind`
/// (its tags, with `number` split into `Integer`/`Float`, the one
/// distinction a bare kind tag can't otherwise carry). Zig's `Tag.KindTag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KindTag {
    Null,
    Boolean,
    String,
    Integer,
    Float,
    Sequence,
    Mapping,
}

impl KindTag {
    pub const ALL: [KindTag; 7] = [
        KindTag::Null,
        KindTag::Boolean,
        KindTag::String,
        KindTag::Integer,
        KindTag::Float,
        KindTag::Sequence,
        KindTag::Mapping,
    ];

    /// The enum ordinal (Zig's `@backingInt`).
    pub fn ordinal(self) -> u8 {
        self as u8
    }

    pub fn from_ordinal(i: u8) -> Option<KindTag> {
        Self::ALL.get(i as usize).copied()
    }
}

/// A cross-format type tag attached to a node. It unifies fig's `: type =`
/// annotation, YAML's `!!str`/`!foo`, and canonical's `!`-sigil into one
/// concept: a type assertion carried alongside the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag<'a> {
    /// A normalized, cross-format type identity. A fig `: int =`, a YAML
    /// `!!int`, and a canonical `!!int` all decode to `Kind(Integer)`, so
    /// this arm is what crosses formats. `extended` kinds (enum/datetime) are
    /// NOT here — they are distinct node kinds that self-annotate, never a
    /// tag.
    Kind(KindTag),
    /// A format tag with no core type meaning (a YAML custom `!foo`/`!<uri>`),
    /// stored verbatim (leading `!` included) so YAML re-emits it
    /// byte-for-byte. A format that can't represent it drops it (fig tier-3).
    Text(&'a [u8]),
}

impl<'a> Tag<'a> {
    /// The tag as a node table spells it: a text tag verbatim, a kind tag
    /// as YAML's core schema spells it (`!!int`, `!!str`) — the one
    /// spelling every format that has tags at all can read.
    pub fn spelling(&self) -> &'a [u8] {
        match *self {
            Tag::Text(t) => t,
            Tag::Kind(k) => match k {
                KindTag::Null => b"!!null",
                KindTag::Boolean => b"!!bool",
                KindTag::String => b"!!str",
                KindTag::Integer => b"!!int",
                KindTag::Float => b"!!float",
                KindTag::Sequence => b"!!seq",
                KindTag::Mapping => b"!!map",
            },
        }
    }

    /// The kind a core-schema type name (`int`, `str`, `seq`) names, or
    /// `None`.
    pub fn of_core_name(name: &[u8]) -> Option<KindTag> {
        const NAMES: [&[u8]; 7] = [b"null", b"bool", b"str", b"int", b"float", b"seq", b"map"];
        NAMES.iter().position(|n| *n == name).and_then(|i| KindTag::from_ordinal(i as u8))
    }

    /// The kind a core-schema spelling (`!!int`) names, or `None`.
    pub fn kind_of_spelling(text: &[u8]) -> Option<KindTag> {
        KindTag::ALL.into_iter().find(|k| text == Tag::Kind(*k).spelling())
    }

    pub fn eql(&self, other: &Tag<'_>) -> bool {
        match (self, other) {
            (Tag::Kind(a), Tag::Kind(b)) => a == b,
            (Tag::Text(a), Tag::Text(b)) => a == b,
            _ => false,
        }
    }
}

/// A comment's original form — a hint. A printer whose target format lacks
/// the requested form downgrades (a `Block` rendered to YAML/TOML becomes a
/// run of `#` line comments). Zig's `Comment.Style`; the ordinal is the
/// table's `COMMENT_LINE`/`COMMENT_BLOCK`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    #[default]
    Line,
    Block,
}

impl Style {
    /// The table's byte for this style.
    pub fn ordinal(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Comment<'a> {
    /// Marker-stripped content. A `Block` comment may contain newlines; a
    /// `Line` comment never does.
    pub text: &'a [u8],
    /// The author's original form — a hint.
    pub style: Style,
}

impl<'a> Comment<'a> {
    /// A line comment of `text`.
    pub fn line(text: &'a [u8]) -> Comment<'a> {
        Comment { text, style: Style::Line }
    }

    pub fn eql(&self, other: &Comment<'_>) -> bool {
        self.style == other.style && self.text == other.text
    }
}

/// Comments bound to one node: a run of own-line comments above it (in
/// source order), at most one same-line trailing comment, and — on a
/// container node, or the document root of any kind — a run of `dangling`
/// comments that sit at the END of its body with no child after them (an
/// orphan in an empty `[]`, comments before the closing brace, or EOF
/// orphans after a non-container root, which has no closing brace to
/// precede). `dangling` is empty for every other non-container node.
///
/// The runs are `Cow`s: what `AST::comments` hands out is owned, and what a
/// caller spells out for `Builder::set_comments` may borrow.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NodeComments<'a> {
    pub leading: Cow<'a, [Comment<'a>]>,
    pub trailing: Option<Comment<'a>>,
    pub dangling: Cow<'a, [Comment<'a>]>,
}

impl<'a> NodeComments<'a> {
    pub fn is_empty(&self) -> bool {
        self.leading.is_empty() && self.trailing.is_none() && self.dangling.is_empty()
    }

    pub fn eql(&self, other: &NodeComments<'_>) -> bool {
        if self.leading.len() != other.leading.len() {
            return false;
        }
        if !self.leading.iter().zip(other.leading.iter()).all(|(a, b)| a.eql(b)) {
            return false;
        }
        if self.dangling.len() != other.dangling.len() {
            return false;
        }
        if !self.dangling.iter().zip(other.dangling.iter()).all(|(a, b)| a.eql(b)) {
            return false;
        }
        match (&self.trailing, &other.trailing) {
            (None, None) => true,
            (Some(a), Some(b)) => a.eql(b),
            _ => false,
        }
    }
}

/// A unit of data in an AST: a view of row `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node<'a> {
    /// Unique number identifier for this node.
    pub id: NodeId,
    pub kind: NodeKind<'a>,
    /// The "next" value when inside a sequence/mapping.
    pub next_sibling: Option<NodeId>,
}

impl<'a> Node<'a> {
    pub fn eql(&self, other: &Node<'_>) -> bool {
        self.id == other.id && self.next_sibling == other.next_sibling && self.kind.eql(&other.kind)
    }
}

/// A keyvalue's two children (Zig's anonymous `keyvalue` payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyValue {
    pub key: NodeId,
    pub value: NodeId,
}

/// Which number a `Number` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumberKind {
    Integer,
    Float,
}

/// A number, by its lexeme (`Node.Kind.Number`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Number<'a> {
    pub raw: &'a [u8],
    pub kind: NumberKind,
}

impl Number<'_> {
    pub fn eql(&self, other: &Number<'_>) -> bool {
        self.kind == other.kind && self.raw == other.raw
    }
}

/// Which special scalar an `Extended` is. The first four are
/// RFC-3339-derived TOML datetimes (`text` is the raw timestamp).
/// `EnumLiteral`'s `text` is the decoded name without the leading `.`;
/// `CharLiteral`'s `text` is the decimal Unicode codepoint (so non-ZON
/// printers can treat it as a number, while the ZON printer re-encodes `'A'`).
/// The ordinal is what a row's kind adds to `Kind::EXT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtKind {
    OffsetDatetime,
    LocalDatetime,
    LocalDate,
    LocalTime,
    EnumLiteral,
    CharLiteral,
    /// A non-finite JSON5 number (`Infinity`, `-Infinity`, `NaN`). `text` is
    /// the source lexeme verbatim, sign included. No JSON number can hold
    /// these, so they ride in an extended scalar.
    NumberSpecial,
    /// A plist `<date>` — `text` is the raw ISO-8601-ish timestamp (plist's
    /// DTD-documented subset), kept verbatim like the TOML datetimes above.
    PlistDate,
    /// A byte string. `text` is always its base64 (the standard alphabet,
    /// padded, no whitespace), whatever the source spells: a text format
    /// writes the base64 itself (plist's `<data>`, YAML's `!!binary`, wrapped
    /// across lines or not), and a binary format holds the raw bytes, which
    /// its language encodes on parse and decodes on print. One spelling is
    /// what lets a byte string move between them unchanged.
    Data,
}

impl ExtKind {
    pub const ALL: [ExtKind; 9] = [
        ExtKind::OffsetDatetime,
        ExtKind::LocalDatetime,
        ExtKind::LocalDate,
        ExtKind::LocalTime,
        ExtKind::EnumLiteral,
        ExtKind::CharLiteral,
        ExtKind::NumberSpecial,
        ExtKind::PlistDate,
        ExtKind::Data,
    ];

    /// The enum ordinal (Zig's `@backingInt`).
    pub fn ordinal(self) -> u8 {
        self as u8
    }

    pub fn from_ordinal(i: u8) -> Option<ExtKind> {
        Self::ALL.get(i as usize).copied()
    }

    /// The Zig field name (`offset_datetime`), as the wire and
    /// `manifest::NativeKinds` spell it.
    pub fn name(self) -> &'static str {
        match self {
            ExtKind::OffsetDatetime => "offset_datetime",
            ExtKind::LocalDatetime => "local_datetime",
            ExtKind::LocalDate => "local_date",
            ExtKind::LocalTime => "local_time",
            ExtKind::EnumLiteral => "enum_literal",
            ExtKind::CharLiteral => "char_literal",
            ExtKind::NumberSpecial => "number_special",
            ExtKind::PlistDate => "plist_date",
            ExtKind::Data => "data",
        }
    }
}

/// A leaf scalar a source format gives special lexical meaning that the
/// abstract model has no first-class variant for: TOML datetimes, ZON
/// enum/char literals, etc. The `kind` records which, and `text` is the
/// value's intrinsic bytes (kept verbatim, like `Number::raw`). Adding a new
/// such scalar is a new `ExtKind`, not a new `NodeKind` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extended<'a> {
    pub kind: ExtKind,
    pub text: &'a [u8],
}

impl Extended<'_> {
    pub fn eql(&self, other: &Extended<'_>) -> bool {
        self.kind == other.kind && self.text == other.text
    }
}

/// A node's kind with its payload (Zig's `Node.Kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind<'a> {
    Null,
    Boolean(bool),
    String(&'a [u8]),
    Number(Number<'a>),
    /// See `Extended`.
    Extended(Extended<'a>),
    /// The first child, or `None` for an empty sequence.
    Sequence(Option<NodeId>),
    /// The first entry, or `None` for an empty mapping.
    Mapping(Option<NodeId>),
    KeyValue(KeyValue),
    /// A `*name` alias: a reference to an anchored node. The payload is the
    /// alias name (no leading `*`); the target is resolved on demand via
    /// `resolve_alias` (kept out of the node so equal documents compare equal
    /// regardless of whether resolution has run). A non-YAML printer never
    /// sees this — `materialize` expands aliases to copied subtrees first.
    Alias(&'a [u8]),
}

/// A `NodeKind`'s arm alone (Zig's `std.meta.activeTag`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeTag {
    Null,
    Boolean,
    String,
    Number,
    Extended,
    Sequence,
    Mapping,
    KeyValue,
    Alias,
}

impl<'a> NodeKind<'a> {
    /// The arm (Zig's `activeTag`).
    pub fn tag(&self) -> NodeTag {
        match self {
            NodeKind::Null => NodeTag::Null,
            NodeKind::Boolean(_) => NodeTag::Boolean,
            NodeKind::String(_) => NodeTag::String,
            NodeKind::Number(_) => NodeTag::Number,
            NodeKind::Extended(_) => NodeTag::Extended,
            NodeKind::Sequence(_) => NodeTag::Sequence,
            NodeKind::Mapping(_) => NodeTag::Mapping,
            NodeKind::KeyValue(_) => NodeTag::KeyValue,
            NodeKind::Alias(_) => NodeTag::Alias,
        }
    }

    pub fn eql(&self, other: &NodeKind<'_>) -> bool {
        match (self, other) {
            (NodeKind::Null, NodeKind::Null) => true,
            (NodeKind::Boolean(a), NodeKind::Boolean(b)) => a == b,
            (NodeKind::String(a), NodeKind::String(b)) => a == b,
            (NodeKind::Number(a), NodeKind::Number(b)) => a.eql(b),
            (NodeKind::Extended(a), NodeKind::Extended(b)) => a.eql(b),
            (NodeKind::Sequence(a), NodeKind::Sequence(b)) => a == b,
            (NodeKind::Mapping(a), NodeKind::Mapping(b)) => a == b,
            (NodeKind::KeyValue(a), NodeKind::KeyValue(b)) => a == b,
            (NodeKind::Alias(a), NodeKind::Alias(b)) => a == b,
            _ => false,
        }
    }

    /// The string payload (Zig's `kind.string`), or `None` for another arm.
    pub fn as_string(&self) -> Option<&'a [u8]> {
        if let NodeKind::String(s) = *self { Some(s) } else { None }
    }

    /// The number payload, or `None`.
    pub fn as_number(&self) -> Option<Number<'a>> {
        if let NodeKind::Number(n) = *self { Some(n) } else { None }
    }

    /// The keyvalue payload, or `None`.
    pub fn as_keyvalue(&self) -> Option<KeyValue> {
        if let NodeKind::KeyValue(kv) = *self { Some(kv) } else { None }
    }

    /// A mapping's first entry; `None` for an empty mapping or another arm.
    pub fn as_mapping(&self) -> Option<Option<NodeId>> {
        if let NodeKind::Mapping(f) = *self { Some(f) } else { None }
    }

    /// A sequence's first item; `None` for an empty sequence or another arm.
    pub fn as_sequence(&self) -> Option<Option<NodeId>> {
        if let NodeKind::Sequence(f) = *self { Some(f) } else { None }
    }
}

/// A tree spelled out as `Node` values, as a test writes the one it
/// expects: `nodes[i].id == i`, rooted at `root`.
pub struct Literal<'a> {
    pub root: NodeId,
    pub nodes: &'a [Node<'a>],
}

impl Literal<'_> {
    /// Whether `ast` is this tree, node for node from the root, whatever
    /// ids either gives its nodes.
    pub fn matches(&self, ast: &AST) -> bool {
        let Ok(want) = AST::from_nodes(self.root, self.nodes) else { return false };
        want.same_tree(want.root, ast, ast.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopt_a_parses_table_becomes_the_tree_linked_with_no_copy() {
        use table::Kind as K;
        let src: Arc<[u8]> = Arc::from(&b"a: [1, x]"[..]);
        let mut t = Table::init(src.clone());
        t.add_row(Row { kind: K::MAPPING, start: 0, end: 9, ..Row::default() }).unwrap();
        t.add_row(Row { kind: K::KEYVALUE, parent: 0, start: 0, end: 9, ..Row::default() }).unwrap();
        t.add_row(Row { kind: K::STRING, parent: 1, start: 0, end: 1, text: table::Text::in_source(0, 1), ..Row::default() })
            .unwrap();
        t.add_row(Row { kind: K::SEQUENCE, parent: 1, start: 3, end: 9, ..Row::default() }).unwrap();
        t.add_row(Row { kind: K::INT, parent: 3, start: 4, end: 5, text: table::Text::in_source(4, 1), ..Row::default() })
            .unwrap();
        let x = t.add_string(b"x").unwrap();
        t.add_row(Row { kind: K::STRING, parent: 3, start: 7, end: 8, text: x, ..Row::default() }).unwrap();
        let one = t.add_string(b"one").unwrap();
        t.comments.push(table::Comment { node: 4, slot: table::COMMENT_TRAILING, style: table::COMMENT_LINE, text: one });
        let rows_ptr = t.rows.as_ptr();
        let ast = AST::adopt(src, &mut t, Spans::Required).unwrap();
        assert!(t.rows.is_empty());
        assert_eq!(rows_ptr, ast.rows().as_ptr());
        let kv = ast.node(ast.node(0).kind.as_mapping().unwrap().unwrap()).kind.as_keyvalue().unwrap();
        assert_eq!(b"a", ast.node(kv.key).kind.as_string().unwrap());
        assert_eq!(None, ast.node(kv.key).next_sibling);
        let first = ast.node(ast.node(kv.value).kind.as_sequence().unwrap().unwrap());
        assert_eq!(b"1", first.kind.as_number().unwrap().raw);
        assert_eq!(b"x", ast.node(first.next_sibling.unwrap()).kind.as_string().unwrap());
        assert_eq!(b"one", ast.comments(4).trailing.unwrap().text);
        assert_eq!(Span::new(4, 5), ast.span_of(4));
    }

    #[test]
    fn adopt_a_malformed_table_is_refused() {
        let src: Arc<[u8]> = Arc::from(&b"x"[..]);
        let mut t = Table::init(src.clone());
        t.add_row(Row { kind: table::Kind::MAPPING, start: 0, end: 1, ..Row::default() }).unwrap();
        t.add_row(Row {
            kind: table::Kind::STRING,
            parent: 0,
            start: 0,
            end: 1,
            text: table::Text::in_source(0, 1),
            ..Row::default()
        })
        .unwrap();
        let e = AST::adopt(src, &mut t, Spans::Required).unwrap_err();
        assert_eq!(err!(MalformedTable), e);
    }
}
