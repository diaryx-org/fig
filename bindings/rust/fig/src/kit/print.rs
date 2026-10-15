//! What a `print` reads and writes with: the table it receives, indexed by
//! node — each row's children, a keyvalue's key and value, its comments by
//! slot — and a [`Writer`] that knows the options it was given.
//!
//! The JavaScript kit's `index(t)` and `Writer`, in Rust.
//!
//! ```
//! use fig::Span;
//! use fig::kit::print::{Index, Writer};
//! use fig::language::{CommentSlot, CommentForm, CommentRow, NodeKind, NodeRow, NodeTable, PrintOptions};
//!
//! let mut t = NodeTable::new();
//! let m = t.push(NodeRow::new(NodeKind::Mapping, None, Span::from(0..0)));
//! let kv = t.push(NodeRow::new(NodeKind::KeyValue, Some(m), Span::from(0..0)));
//! t.push(NodeRow::new(NodeKind::String, Some(kv), Span::from(0..0)).with_text("a"));
//! t.push(NodeRow::new(NodeKind::Int, Some(kv), Span::from(0..0)).with_text("1"));
//! t.comments.push(CommentRow::new(2, CommentSlot::Leading, CommentForm::Line, "the a"));
//!
//! let ix = Index::new(&t);
//! let mut w = Writer::new(&PrintOptions::default());
//! for &e in ix.children(ix.root()) {
//!     w.comments(ix.leading(ix.key(e)), "#", 0);
//!     w.put(ix.text(ix.key(e))).put(" = ").put(ix.text(ix.value(e))).nl();
//! }
//! assert_eq!(w.finish(), "# the a\na = 1\n");
//! ```

use crate::language::{CommentForm, CommentSlot, NodeRow, NodeTable, PrintOptions};

/// A comment as a printer reads it: its text and form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Comment<'t> {
    pub text: &'t str,
    pub style: CommentForm,
}

/// A node table indexed by node. Built once, at the top of a `print`.
#[derive(Clone, Debug)]
pub struct Index<'t> {
    table: &'t NodeTable,
    children: Vec<Vec<u32>>,
    comments: Vec<[Vec<Comment<'t>>; 3]>,
}

impl<'t> Index<'t> {
    pub fn new(table: &'t NodeTable) -> Self {
        let n = table.rows.len();
        let mut children = vec![Vec::new(); n];
        for (i, r) in table.rows.iter().enumerate() {
            if let Some(p) = r.parent
                && (p as usize) < n
            {
                children[p as usize].push(i as u32);
            }
        }
        let mut comments: Vec<[Vec<Comment<'t>>; 3]> = (0..n).map(|_| Default::default()).collect();
        for c in &table.comments {
            if let Some(slots) = comments.get_mut(c.node as usize) {
                let slot = match c.slot {
                    CommentSlot::Leading => 0,
                    CommentSlot::Trailing => 1,
                    CommentSlot::Dangling => 2,
                };
                slots[slot].push(Comment {
                    text: &c.text,
                    style: c.style,
                });
            }
        }
        Index {
            table,
            children,
            comments,
        }
    }

    /// The table.
    pub fn table(&self) -> &'t NodeTable {
        self.table
    }

    /// The root's row id: 0.
    pub fn root(&self) -> u32 {
        0
    }

    /// Row `id`.
    pub fn row(&self, id: u32) -> &'t NodeRow {
        &self.table.rows[id as usize]
    }

    /// Row `id`'s text, or `""`.
    pub fn text(&self, id: u32) -> &'t str {
        self.row(id).text.as_deref().unwrap_or("")
    }

    /// Row `id`'s children, in order.
    pub fn children(&self, id: u32) -> &[u32] {
        &self.children[id as usize]
    }

    /// Keyvalue `id`'s key.
    pub fn key(&self, id: u32) -> u32 {
        self.children[id as usize][0]
    }

    /// Keyvalue `id`'s value.
    pub fn value(&self, id: u32) -> u32 {
        self.children[id as usize][1]
    }

    /// Row `id`'s comments in `slot`, in order.
    pub fn comments(&self, id: u32, slot: CommentSlot) -> &[Comment<'t>] {
        let i = match slot {
            CommentSlot::Leading => 0,
            CommentSlot::Trailing => 1,
            CommentSlot::Dangling => 2,
        };
        &self.comments[id as usize][i]
    }

    pub fn leading(&self, id: u32) -> &[Comment<'t>] {
        self.comments(id, CommentSlot::Leading)
    }

    pub fn trailing(&self, id: u32) -> &[Comment<'t>] {
        self.comments(id, CommentSlot::Trailing)
    }

    pub fn dangling(&self, id: u32) -> &[Comment<'t>] {
        self.comments(id, CommentSlot::Dangling)
    }
}

/// A `print`'s output, which knows the options it was given: [`nl`] and
/// [`indent`] write nothing when `pretty` is off, and a level of
/// indentation is `indent` spaces.
///
/// [`nl`]: Writer::nl
/// [`indent`]: Writer::indent
#[derive(Clone, Debug)]
pub struct Writer {
    out: String,
    pretty: bool,
    unit: String,
}

impl Writer {
    pub fn new(options: &PrintOptions) -> Self {
        Writer {
            out: String::new(),
            pretty: options.pretty,
            unit: " ".repeat(options.indent as usize),
        }
    }

    /// Append `s`.
    pub fn put(&mut self, s: &str) -> &mut Self {
        self.out.push_str(s);
        self
    }

    /// End the line, when pretty.
    pub fn nl(&mut self) -> &mut Self {
        if self.pretty {
            self.out.push('\n');
        }
        self
    }

    /// Indent `depth` levels, when pretty.
    pub fn indent(&mut self, depth: usize) -> &mut Self {
        if self.pretty {
            for _ in 0..depth {
                self.out.push_str(&self.unit);
            }
        }
        self
    }

    /// Indent, write `s`, end the line.
    pub fn line(&mut self, depth: usize, s: &str) -> &mut Self {
        self.indent(depth).put(s).nl()
    }

    /// Write `comments` as whole lines, each line of each comment's text on
    /// its own and trimmed, after `leader` and a space (`# text`, or the bare
    /// leader for an empty line), indented `depth` levels. Answers whether
    /// anything was written.
    pub fn comments(&mut self, comments: &[Comment<'_>], leader: &str, depth: usize) -> bool {
        let mut wrote = false;
        for c in comments {
            for line in c.text.split('\n') {
                let t = line.trim_matches(|ch| ch == ' ' || ch == '\t');
                self.indent(depth).put(leader);
                if !t.is_empty() {
                    self.put(" ").put(t);
                }
                self.out.push('\n');
                wrote = true;
            }
        }
        wrote
    }

    /// What has been written so far.
    pub fn as_str(&self) -> &str {
        &self.out
    }

    /// What was written.
    pub fn finish(self) -> String {
        self.out
    }
}
