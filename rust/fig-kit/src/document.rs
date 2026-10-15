//! Port of kit/src/document.zig.
//!
//! A parsed document: its source, and the tree read from it (`ast`), whose
//! rows carry every node's span and whose sparse lists carry the other
//! source positions the editor splices at — anchor and tag tokens, item
//! markers, entry separators, a section's header lines and the places its
//! name is written. The accessors here read those; the tree's own are on
//! `AST`.
//!
//! **Ownership (Rust).** Zig's `Document` borrows its source. Here `source`
//! is the same shared `Arc<[u8]>` the AST's table holds, so a `Document`
//! owns everything it reads and can be stored and moved freely.

use std::sync::Arc;

use super::ast::{Node, NodeId, AST};
use super::span::Span;
use super::table;

#[derive(Debug, Clone)]
pub struct Document {
    /// Every in-source text and span of `ast` indexes it (the same
    /// allocation as `ast.storage.source`).
    pub source: Arc<[u8]>,
    pub ast: AST,
}

/// How a separated sequence's items are joined: the separator the language
/// names for this one container, and whether the container has delimiters.
///
/// The dialect's `Syntax.flow_entry_sep` and `flow_root` say how a format's
/// flow containers are spelled, once for the whole format. A format whose
/// separator differs by container cannot say it there: a `.tune` voice
/// joins its bars with `|` and a bar its tokens with spaces, and neither
/// has brackets. Only the parser knows which container it read, so the fact
/// is a column of the parse, as the item markers and entry separators are.
/// A sequence with one is edited by the flow arm with this separator,
/// whatever its first byte. See `docs/proposals/runtime-languages.md` §4.1,
/// the `item_sep` and `open` columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemSep<'a> {
    /// The bytes written between two items that share a line — `" | "`,
    /// `" "`, or `"\n\n"` for items a blank line apart. What a delete
    /// consumes is this with its whitespace trimmed (`|`), or whitespace
    /// alone where nothing is left. `None`: the dialect's `flow_entry_sep`.
    pub text: Option<&'a [u8]>,
    /// The container has no delimiters: its items run from its span's start
    /// to its end, a trailing separator included where the format allows
    /// one. `flow_root` is this for a root mapping.
    pub open: bool,
}

/// One physical HEADER LINE of a SECTION node — a container whose span does
/// not describe its extent: a TOML `[table]`/`[[array]]`/dotted table, an
/// INI `[section]`, a fig block container. One per header line that created
/// or re-opened the node, in source order, each a whole line (from its first
/// byte through its newline).
///
/// This is the one fact the editor cannot derive from the spans: a section
/// node's span anchors only the key segment of the line that CREATED it (or,
/// for fig, its opening line widened to the subtree's end), so a later
/// header that re-enters the same container — TOML's `[a]` … `[b]` …
/// `[a.c]`, INI's reopened `[a]`, fig's re-entered `database` — sits in no
/// node's span at all. Everything else about a container's physical extent
/// is DERIVED from this plus the spans: `editor/regions.rs`'s `gather` walks
/// the subtree, takes each header line here, and takes each contiguous
/// entry's own line from its span. Presence of one is what makes a node a
/// section.
pub type NodeRegion = table::SpanFact;

/// One place a SECTION node's NAME is written, as a span the editor can
/// splice a new name over: a TOML table's own `[a]`, the `a` in `[a.b]` and
/// `[[a.c]]`, the `a` in a dotted `a.b = 1`; an INI `[section]` and each
/// reopening; a fig block container's header and each re-entry.
///
/// `kind` says how the mention sits relative to the node's PARENT
/// (`MentionKind`): a header opens a region of the node's own, outside the
/// parent's lines (`[a.b]` is not inside `[a]`'s text; a reopened INI `[a]`
/// is a new region; figl's `p` in `c[].p[]`, after `c[]`'s own lines), while
/// an entry mention is on one of the parent's own entry lines (a dotted key,
/// a fig header at the parent's depth). The editor's block insert reads it
/// to keep a new entry inside the intended table, and `renameContainer` and
/// a section's `replaceKeyAtPath` rewrite every mention at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeMention {
    pub node_id: NodeId,
    pub span: Span,
    pub kind: MentionKind,
}

impl NodeMention {
    fn of(m: table::Mention) -> NodeMention {
        NodeMention { node_id: m.node, span: Span::new(m.start as usize, m.end as usize), kind: MentionKind::of(m.kind) }
    }
}

/// How a mention sits relative to the node's parent. See `NodeMention`.
///
/// The first two are a section's NAME. The other two are where a SEQUENCE
/// ITEM's place in its path is written, which no span carries either: an
/// item has no key, and the lines that address it may be anywhere. A delete
/// renumbers an `Index` mention and re-spells an `Append` one so that the
/// items after the deleted one stay the items they were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MentionKind {
    /// A header line of the node's own, outside its parent's lines.
    Header,
    /// On one of the parent's own entry lines.
    Entry,
    /// The item's index, where a path addresses it by position: figl's `1`
    /// in `xs[1] = b`, `xs[1]` and `xs[1].k = v`. Deleting an earlier item
    /// writes the new one over it.
    Index,
    /// The header that appended the item — figl's `a.b[]`, the whole path —
    /// or the line that repeats the header before it, figl's `+`. When the
    /// item before goes, taking its header, the next item's `Append` mention
    /// is written as that header, in case it repeated it.
    Append,
}

impl MentionKind {
    /// Whether this mention is a section's name, which a rename rewrites.
    pub fn is_name(self) -> bool {
        self == MentionKind::Header || self == MentionKind::Entry
    }

    /// The node table's byte for this kind (`table::MENTION_*`).
    pub fn byte(self) -> u8 {
        match self {
            MentionKind::Header => table::MENTION_HEADER,
            MentionKind::Entry => table::MENTION_ENTRY,
            MentionKind::Index => table::MENTION_INDEX,
            MentionKind::Append => table::MENTION_APPEND,
        }
    }

    /// The kind the node table's byte `b` stands for; a table is checked for
    /// an unknown one when it is read.
    pub fn of(b: u8) -> MentionKind {
        match b {
            table::MENTION_ENTRY => MentionKind::Entry,
            table::MENTION_INDEX => MentionKind::Index,
            table::MENTION_APPEND => MentionKind::Append,
            _ => MentionKind::Header,
        }
    }
}

/// A section's mentions, as `NodeMention`s.
#[derive(Debug, Clone)]
pub struct MentionIterator<'a> {
    items: &'a [table::Mention],
    i: usize,
}

impl MentionIterator<'_> {
    pub fn count(&self) -> usize {
        self.items.len()
    }

    pub fn at(&self, i: usize) -> NodeMention {
        NodeMention::of(self.items[i])
    }
}

impl Iterator for MentionIterator<'_> {
    type Item = NodeMention;

    fn next(&mut self) -> Option<NodeMention> {
        if self.i >= self.items.len() {
            return None;
        }
        let m = NodeMention::of(self.items[self.i]);
        self.i += 1;
        Some(m)
    }
}

impl Document {
    /// A document over `ast`, whose table's source is the document's.
    pub fn new(ast: AST) -> Document {
        Document { source: ast.storage.source.clone(), ast }
    }

    /// The tree, for a caller that wants values, not source positions.
    pub fn into_ast(self) -> AST {
        self.ast
    }

    pub fn span(&self, node: &Node<'_>) -> Span {
        self.ast.span_of(node.id)
    }

    /// Node `id`'s span.
    pub fn span_of(&self, id: NodeId) -> Span {
        self.ast.span_of(id)
    }

    /// Source span of the `&name` anchor token on `node`, or `None`.
    pub fn anchor_span(&self, node: &Node<'_>) -> Option<Span> {
        let f = table::fact_of(self.ast.anchor_facts(), node.id)?;
        opt_span(f.start, f.end)
    }

    /// Source span of the `!tag` token on `node`, or `None`.
    pub fn tag_span(&self, node: &Node<'_>) -> Option<Span> {
        let f = table::fact_of(self.ast.tag_facts(), node.id)?;
        opt_span(f.start, f.end)
    }

    /// Source span of the token that INTRODUCES `node` as a block-sequence
    /// item — YAML's and NestedText's `-`, fig's `*` — or `None` for every
    /// node that is not one, and for an item whose span already begins at
    /// its introducing token (plist's `<string>` element).
    ///
    /// This is the one item fact a span cannot carry: an item's span starts
    /// at its VALUE, which for a nested or empty value (`-\n  k: v`, a bare
    /// `-`) begins on a later line than the marker, so nothing in the span
    /// says where the item's own line is. The editor reads it for every
    /// block-sequence op — where an item's line starts, where a leading
    /// comment goes, what prefix a new sibling copies.
    pub fn marker_span(&self, node: &Node<'_>) -> Option<Span> {
        let f = table::fact_of(self.ast.marker_facts(), node.id)?;
        Some(Span::new(f.start as usize, f.end as usize))
    }

    /// For a `keyvalue` node, the source span of the token that SEPARATES
    /// its key from its value — YAML's `:`, fig's `=`, a NestedText plain
    /// key's `:` — or `None`.
    ///
    /// A recorded separator is the parser's statement that the entry's value
    /// is REFRAMED rather than spliced in place: the editor rewrites
    /// everything after the key, from the separator through the old value's
    /// end, so a value can change shape between inline (`key: v`) and block
    /// (`key:` over indented lines). A format whose values never change shape
    /// (JSON, TOML) records none and gets the direct span splice. The
    /// separator also fixes where the value's text begins: past it, then past
    /// any anchor or tag. See the editor's `replaceValAtPath`.
    pub fn sep_span(&self, node: &Node<'_>) -> Option<Span> {
        let f = table::fact_of(self.ast.sep_facts(), node.id)?;
        Some(Span::new(f.start as usize, f.end as usize))
    }

    /// How the items of sequence `node` are joined, when the parser said so
    /// for this container, or `None` — see `ItemSep`.
    pub fn item_sep(&self, node: &Node<'_>) -> Option<ItemSep<'_>> {
        let open = self.ast.rows()[node.id as usize].flags & table::ROW_OPEN != 0;
        let text = match table::fact_of(self.ast.item_sep_facts(), node.id) {
            Some(f) => match self.ast.text_of(f.text) {
                Some(x) if !x.is_empty() => Some(x),
                _ => None,
            },
            None => None,
        };
        if text.is_none() && !open {
            return None;
        }
        Some(ItemSep { text, open })
    }

    /// The header lines recorded for node `id`, in source order — empty for
    /// a node that is not a section. O(log n).
    pub fn regions_of(&self, id: NodeId) -> &[NodeRegion] {
        run(self.ast.region_facts(), id)
    }

    /// The name mentions recorded for node `id`, in source order — empty for
    /// a node that is not a section, or whose format records none. O(log n).
    pub fn mentions_of(&self, id: NodeId) -> MentionIterator<'_> {
        MentionIterator { items: run(self.ast.mention_facts(), id), i: 0 }
    }

    /// Whether `node` is a SECTION node — a container the parser recorded
    /// header lines for, because its span does not describe its physical
    /// extent. The root is never one.
    pub fn is_section(&self, node: &Node<'_>) -> bool {
        !self.regions_of(node.id).is_empty()
    }
}

/// The whole line containing `at`: from the byte after the previous newline
/// (or 0) through the next newline inclusive (or `source.len()`). What a
/// parser records for a header line, given any byte offset on it — the
/// header's key token, its line start, whichever the parser has to hand.
pub fn line_region_at(source: &[u8], at: usize) -> Span {
    let mut start = at;
    while start > 0 && source[start - 1] != b'\n' {
        start -= 1;
    }
    let end = match source.get(at..).and_then(|rest| rest.iter().position(|&c| c == b'\n')) {
        Some(nl) => at + nl + 1,
        None => source.len(),
    };
    Span::new(start, end)
}

fn opt_span(start: u32, end: u32) -> Option<Span> {
    if start == table::NONE {
        return None;
    }
    Some(Span::new(start as usize, end as usize))
}

/// The run of `items` (sorted by node) naming `id`.
fn run<F: table::Fact>(items: &[F], id: u32) -> &[F] {
    let mut lo = 0usize;
    let mut hi = items.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if items[mid].node() < id {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let mut end = lo;
    while end < items.len() && items[end].node() == id {
        end += 1;
    }
    &items[lo..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_region_at_covers_the_whole_line_newline_included() {
        let src = b"ab\ncd\nef";
        assert_eq!(0, line_region_at(src, 1).start);
        assert_eq!(3, line_region_at(src, 1).end);
        assert_eq!(3, line_region_at(src, 3).start);
        assert_eq!(6, line_region_at(src, 4).end);
        assert_eq!(6, line_region_at(src, 7).start);
        assert_eq!(8, line_region_at(src, 7).end);
    }
}
