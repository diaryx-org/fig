//! Nodes that hold their children and their comments, for a parser that
//! builds a document as owned values — a recursive descent whose every
//! function returns the subtree it read — and lays it out as rows at the
//! end with [`rows`].
//!
//! The JavaScript kit's tree (`mapping`, `sequence`, `entry`, `scalar`,
//! `rows`), in Rust: a mapping puts its entries by a
//! [`DuplicatePolicy`], and every comment is on the node it belongs to.
//!
//! ```
//! use fig::Span;
//! use fig::kit::rows::DuplicatePolicy;
//! use fig::kit::tree::{self, Node};
//! use fig::language::{CommentSlot, NodeKind};
//!
//! let mut m = Node::mapping(Span::from(0..8), DuplicatePolicy::LastValue);
//! let key = Node::scalar(NodeKind::String, Span::from(0..1), "a");
//! let mut value = Node::scalar(NodeKind::Int, Span::from(2..3), "1");
//! value.comment(CommentSlot::Trailing, "one");
//! m.put(Node::entry(key, value, None)).unwrap();
//! let t = tree::rows(&m);
//! assert_eq!(t.rows.len(), 4);
//! assert_eq!(t.comments[0].node, 3);
//! ```

use std::collections::HashMap;

use crate::language::{CommentForm, CommentSlot, LanguageError, MentionKind, NodeKind, NodeRow, NodeTable};
use crate::{ExtKind, Span};

use super::rows::{DuplicatePolicy, RowWriter};

/// A comment's text, without its delimiters, and its form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub text: String,
    pub style: CommentForm,
}

/// What a node holds besides its own facts.
#[derive(Clone, Debug)]
pub enum Body {
    /// A scalar, an alias, or a null.
    Leaf,
    /// A mapping's entries, each a [`Body::Entry`] node, in order.
    Mapping {
        entries: Vec<Node>,
        duplicates: DuplicatePolicy,
        by_key: HashMap<String, usize>,
    },
    /// A sequence's items, in order.
    Sequence { items: Vec<Node> },
    /// A keyvalue's key and its value.
    Entry { key: Box<Node>, value: Box<Node> },
}

/// One node: a row's facts, its comments, and its children.
#[derive(Clone, Debug)]
pub struct Node {
    /// The row a node is a [`NodeRow`] with no parent: its kind, span,
    /// text and side facts.
    pub row: NodeRow,
    pub leading: Vec<Comment>,
    pub trailing: Vec<Comment>,
    pub dangling: Vec<Comment>,
    /// A section's header lines.
    pub regions: Vec<Span>,
    /// Where a section's name, or an item's path, is written.
    pub mentions: Vec<(Span, MentionKind)>,
    pub body: Body,
}

impl Node {
    /// A node over `row`, with nothing in it.
    pub fn new(row: NodeRow, body: Body) -> Self {
        Node {
            row,
            leading: Vec::new(),
            trailing: Vec::new(),
            dangling: Vec::new(),
            regions: Vec::new(),
            mentions: Vec::new(),
            body,
        }
    }

    /// A scalar of `kind` (null, bool, int, float, string, alias) whose
    /// text is `text`: a string's decoded value, a number's lexeme, `true`
    /// or `false`, an alias's anchor name.
    pub fn scalar(kind: NodeKind, span: Span, text: impl Into<String>) -> Self {
        let mut row = NodeRow::new(kind, None, span);
        if kind != NodeKind::Null {
            row.text = Some(text.into());
        }
        Node::new(row, Body::Leaf)
    }

    /// An extended scalar: a string (or, for a char literal, an int) row of
    /// `ext_kind`.
    pub fn extended(ext_kind: ExtKind, kind: NodeKind, span: Span, text: impl Into<String>) -> Self {
        let mut n = Node::scalar(kind, span, text);
        n.row.ext_kind = Some(ext_kind);
        n
    }

    /// A null.
    pub fn null(span: Span) -> Self {
        Node::scalar(NodeKind::Null, span, "")
    }

    /// An empty mapping that puts its entries by `duplicates`.
    pub fn mapping(span: Span, duplicates: DuplicatePolicy) -> Self {
        Node::new(
            NodeRow::new(NodeKind::Mapping, None, span),
            Body::Mapping {
                entries: Vec::new(),
                duplicates,
                by_key: HashMap::new(),
            },
        )
    }

    /// An empty sequence.
    pub fn sequence(span: Span) -> Self {
        Node::new(NodeRow::new(NodeKind::Sequence, None, span), Body::Sequence { items: Vec::new() })
    }

    /// A keyvalue over `key` and `value`, spanning from the key's start to
    /// the value's end unless `span` says otherwise.
    pub fn entry(key: Node, value: Node, span: Option<Span>) -> Self {
        let span = span.unwrap_or_else(|| Span::from(start(&key)..end(&value)));
        Node::new(
            NodeRow::new(NodeKind::KeyValue, None, span),
            Body::Entry {
                key: Box::new(key),
                value: Box::new(value),
            },
        )
    }

    /// The node's span.
    pub fn span(&self) -> Span {
        self.row.span.unwrap_or(Span::from(0..0))
    }

    /// Bind a comment to the node, after any already in `slot`.
    pub fn comment(&mut self, slot: CommentSlot, text: impl Into<String>) -> &mut Self {
        self.comment_as(slot, text, CommentForm::Line)
    }

    /// [`comment`](Self::comment) in a form of its own.
    pub fn comment_as(&mut self, slot: CommentSlot, text: impl Into<String>, style: CommentForm) -> &mut Self {
        let c = Comment {
            text: text.into(),
            style,
        };
        match slot {
            CommentSlot::Leading => self.leading.push(c),
            CommentSlot::Trailing => self.trailing.push(c),
            CommentSlot::Dangling => self.dangling.push(c),
        }
        self
    }

    /// A keyvalue's key, or `None`.
    pub fn key(&self) -> Option<&Node> {
        match &self.body {
            Body::Entry { key, .. } => Some(key),
            _ => None,
        }
    }

    /// A keyvalue's value, or `None`.
    pub fn value(&self) -> Option<&Node> {
        match &self.body {
            Body::Entry { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Put `entry` in this mapping by its duplicate policy, and return the
    /// index of the entry that holds the value afterwards. Under
    /// [`DuplicatePolicy::LastValue`] a repeated key's first entry keeps its
    /// place, its key and its span and takes the later value; the later
    /// key, and its comments, are dropped. A key with no text repeats
    /// nothing. Refused at the later key under [`DuplicatePolicy::Error`].
    ///
    /// # Panics
    /// When the node is not a mapping or `entry` not a keyvalue.
    pub fn put(&mut self, entry: Node) -> Result<usize, LanguageError> {
        let Body::Mapping {
            entries,
            duplicates,
            by_key,
        } = &mut self.body
        else {
            panic!("put into a node that is not a mapping");
        };
        let name = entry.key().expect("put a node that is not an entry").row.text.clone();
        let existing = name.as_ref().and_then(|n| by_key.get(n).copied());
        if let Some(i) = existing {
            match duplicates {
                DuplicatePolicy::LastValue => {
                    let Body::Entry { value, .. } = entry.body else { unreachable!() };
                    if let Body::Entry { value: old, .. } = &mut entries[i].body {
                        *old = value;
                    }
                    return Ok(i);
                }
                DuplicatePolicy::FirstValue => return Ok(i),
                DuplicatePolicy::Error => {
                    let message = format!("duplicate key `{}`", name.unwrap_or_default());
                    return Err(LanguageError::at(message, start(entry.key().unwrap())));
                }
                _ => {}
            }
        }
        entries.push(entry);
        let i = entries.len() - 1;
        if let (Some(n), None) = (name, existing) {
            by_key.insert(n, i);
        }
        Ok(i)
    }

    /// Append `item` to this sequence.
    ///
    /// # Panics
    /// When the node is not a sequence.
    pub fn add(&mut self, item: Node) -> &mut Node {
        let Body::Sequence { items } = &mut self.body else {
            panic!("add to a node that is not a sequence");
        };
        items.push(item);
        items.last_mut().unwrap()
    }
}

fn start(n: &Node) -> usize {
    n.row.span.map_or(0, |s| s.start)
}

fn end(n: &Node) -> usize {
    n.row.span.map_or(0, |s| s.end)
}

/// `root` as a node table: its rows in pre-order, each comment, region and
/// mention on its node's row. What a `parse` returns for the tree it built.
pub fn rows(root: &Node) -> NodeTable {
    let mut w = RowWriter::new();
    let id = emit(&mut w, root, None);
    w.finish(id)
}

fn emit(w: &mut RowWriter, n: &Node, parent: Option<u32>) -> u32 {
    let mut row = n.row.clone();
    row.parent = parent;
    let id = w.add(row);
    for (slot, list) in [
        (CommentSlot::Leading, &n.leading),
        (CommentSlot::Trailing, &n.trailing),
        (CommentSlot::Dangling, &n.dangling),
    ] {
        for c in list {
            w.comment(id, slot, c.style, c.text.clone());
        }
    }
    for r in &n.regions {
        w.region(id, *r);
    }
    for (s, k) in &n.mentions {
        w.mention(id, *s, *k);
    }
    match &n.body {
        Body::Leaf => {}
        Body::Mapping { entries, .. } => {
            for e in entries {
                emit(w, e, Some(id));
            }
        }
        Body::Sequence { items } => {
            for i in items {
                emit(w, i, Some(id));
            }
        }
        Body::Entry { key, value } => {
            emit(w, key, Some(id));
            emit(w, value, Some(id));
        }
    }
    id
}
