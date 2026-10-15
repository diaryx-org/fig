//! A [`NodeTable`] written in whatever order a parser meets its nodes.
//!
//! A parse returns its rows in pre-order: a container before its children,
//! a keyvalue before its key and then its value, each naming its parent. A
//! parser that reads top-down can write them so with [`NodeTable::push`].
//! One that meets a value before the entry that holds it, or that keeps a
//! repeated key's first place and takes its last value, writes rows here
//! instead: each row as it comes, linked to its parent and siblings, and
//! [`RowWriter::finish`] once at the end, which puts them in pre-order and
//! moves every comment, region and mention to its node's new row.
//!
//! It is the row writer of every kit: the Zig kit's `RowWriter` and
//! `flat_map`, the JavaScript kit's `Table`, C's `fig_table_finish`, all
//! held to the same fixtures (`kit/testdata/rows/`).
//!
//! ```
//! use fig::Span;
//! use fig::kit::rows::{DuplicatePolicy, RowWriter};
//! use fig::language::{NodeKind, NodeRow};
//!
//! // `A=1` then `A=2`: the first entry keeps its place and takes the last value.
//! let mut w = RowWriter::new();
//! let root = w.add(NodeRow::new(NodeKind::Mapping, None, Span::from(0..8)));
//! for (at, v) in [(0, "1"), (4, "2")] {
//!     let key = w.add(NodeRow::new(NodeKind::String, None, Span::from(at..at + 1)).with_text("A"));
//!     let value = w.add(NodeRow::new(NodeKind::String, None, Span::from(at + 2..at + 3)).with_text(v));
//!     w.put(root, key, value, DuplicatePolicy::LastValue).unwrap();
//! }
//! let t = w.finish(root);
//! assert_eq!(t.rows.len(), 4);
//! assert_eq!(t.rows[3].text.as_deref(), Some("2"));
//! ```

use std::collections::HashMap;

use crate::Span;
use crate::language::{
    CommentForm, CommentRow, CommentSlot, DiagnosticRow, DirectiveRow, LanguageError, MentionKind,
    MentionRow, NodeKind, NodeRow, NodeTable, RegionRow,
};

/// What [`RowWriter::put`] does with a key its mapping already holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DuplicatePolicy {
    /// The first entry keeps its place, its key and its span, and takes the
    /// later value: what every flat format (INI, dotenv, `.properties`)
    /// and `plutil` do.
    #[default]
    LastValue,
    /// The later entry is dropped.
    FirstValue,
    /// Both entries stay.
    Keep,
    /// The later entry is refused, at its key.
    Error,
}

/// What [`RowWriter::put`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Put {
    /// The keyvalue row that holds the value afterwards.
    pub entry: u32,
    /// Whether the key was already there.
    pub repeat: bool,
}

/// A node table being written in any order. See the [module](self).
#[derive(Clone, Debug, Default)]
pub struct RowWriter {
    table: NodeTable,
    first: Vec<Option<u32>>,
    next: Vec<Option<u32>>,
    last: Vec<Option<u32>>,
    keyed: HashMap<(u32, String), u32>,
}

impl RowWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `row` and return its id. A row with a `parent` is appended to
    /// that row's children as it is written, so rows written in pre-order
    /// need nothing more; one with none waits to be linked.
    pub fn add(&mut self, row: NodeRow) -> u32 {
        let parent = row.parent;
        let id = self.table.push(row);
        self.first.push(None);
        self.next.push(None);
        self.last.push(None);
        if let Some(p) = parent {
            self.append(p, id);
        }
        id
    }

    /// Row `id` as it stands.
    pub fn row(&self, id: u32) -> &NodeRow {
        &self.table.rows[id as usize]
    }

    /// Row `id`, to change in place.
    pub fn row_mut(&mut self, id: u32) -> &mut NodeRow {
        &mut self.table.rows[id as usize]
    }

    /// How many rows have been written.
    pub fn len(&self) -> usize {
        self.table.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.rows.is_empty()
    }

    /// Link `child` as `parent`'s last child, naming `parent` as its
    /// parent. A keyvalue's first child is its key, its second its value.
    pub fn append(&mut self, parent: u32, child: u32) {
        match self.last[parent as usize] {
            None => self.first[parent as usize] = Some(child),
            Some(prev) => self.next[prev as usize] = Some(child),
        }
        self.next[child as usize] = None;
        self.last[parent as usize] = Some(child);
        self.table.rows[child as usize].parent = Some(parent);
    }

    /// Make `child` container `id`'s first child, or empty it for `None`.
    /// Its later children follow through [`set_next`](Self::set_next).
    pub fn set_first(&mut self, id: u32, child: Option<u32>) {
        self.first[id as usize] = child;
        if child.is_none() {
            self.last[id as usize] = None;
        }
    }

    /// Link `next` after `id` among its siblings, or end the chain at `id`.
    pub fn set_next(&mut self, id: u32, next: Option<u32>) {
        self.next[id as usize] = next;
    }

    /// Put the entry `key`: `value` in mapping `map`, which takes its
    /// entries through `put` alone, by `policy`. A new entry is a new
    /// keyvalue row whose span runs from the key's start to the value's
    /// end. The rows a repeat leaves unlinked are dropped by
    /// [`finish`](Self::finish). Refused under [`DuplicatePolicy::Error`],
    /// at the later key.
    pub fn put(
        &mut self,
        map: u32,
        key: u32,
        value: u32,
        policy: DuplicatePolicy,
    ) -> Result<Put, LanguageError> {
        let name = self.row(key).text.clone();
        let existing = name
            .as_ref()
            .and_then(|n| self.keyed.get(&(map, n.clone())).copied());
        if let Some(kv) = existing {
            match policy {
                DuplicatePolicy::LastValue => {
                    let k = self.first[kv as usize].expect("an entry put here has its key");
                    self.next[k as usize] = Some(value);
                    self.next[value as usize] = None;
                    self.last[kv as usize] = Some(value);
                    self.table.rows[value as usize].parent = Some(kv);
                    return Ok(Put {
                        entry: kv,
                        repeat: true,
                    });
                }
                DuplicatePolicy::FirstValue => {
                    return Ok(Put {
                        entry: kv,
                        repeat: true,
                    });
                }
                DuplicatePolicy::Error => {
                    let message = format!("duplicate key `{}`", name.unwrap_or_default());
                    return Err(match self.row(key).span {
                        Some(s) => LanguageError::at(message, s.start),
                        None => LanguageError::new(message),
                    });
                }
                DuplicatePolicy::Keep => {}
            }
        }
        let span = match (self.row(key).span, self.row(value).span) {
            (Some(k), Some(v)) => Span::from(k.start..v.end),
            _ => Span::from(0..0),
        };
        let mut row = NodeRow::new(NodeKind::KeyValue, Some(map), span);
        if self.row(key).span.is_none() {
            row.span = None;
        }
        let kv = self.add(row);
        self.append(kv, key);
        self.append(kv, value);
        if let (Some(n), None) = (name, existing) {
            self.keyed.insert((map, n), kv);
        }
        Ok(Put {
            entry: kv,
            repeat: existing.is_some(),
        })
    }

    /// Mapping `map`'s entry whose key's text is `key`, found by walking
    /// its links, or `None`.
    pub fn child_by_key(&self, map: u32, key: &str) -> Option<u32> {
        let mut e = self.first[map as usize];
        let mut steps = 0;
        while let Some(id) = e {
            if steps > self.len() {
                break;
            }
            steps += 1;
            if self.row(id).kind == NodeKind::KeyValue
                && let Some(k) = self.first[id as usize]
                && self.row(k).text.as_deref() == Some(key)
            {
                return Some(id);
            }
            e = self.next[id as usize];
        }
        None
    }

    /// A comment on row `node`, after any already in `slot`.
    pub fn comment(
        &mut self,
        node: u32,
        slot: CommentSlot,
        style: CommentForm,
        text: impl Into<String>,
    ) {
        self.table
            .comments
            .push(CommentRow::new(node, slot, style, text));
    }

    /// A header line of section `node`.
    pub fn region(&mut self, node: u32, span: Span) {
        self.table.regions.push(RegionRow::new(node, span));
    }

    /// A place `node`'s name is written.
    pub fn mention(&mut self, node: u32, span: Span, kind: MentionKind) {
        self.table.mentions.push(MentionRow::new(node, span, kind));
    }

    /// A tag-handle declaration of the document.
    pub fn directive(&mut self, row: DirectiveRow) {
        self.table.directives.push(row);
    }

    /// A diagnostic of the parse.
    pub fn diagnostic(&mut self, row: DiagnosticRow) {
        self.table.diagnostics.push(row);
    }

    /// The table: its rows in pre-order from `root` — the root first, each
    /// row after its parent and naming it, a container's children in their
    /// linked order, a keyvalue's key before its value — with every
    /// comment, region and mention on its node's new row, in node order. A
    /// row the links do not reach, and what it carries, is dropped; a row
    /// linked twice is written once. Comments are ordered by node and slot,
    /// keeping their order within a slot, and a node given two trailing
    /// comments keeps the last.
    pub fn finish(self, root: u32) -> NodeTable {
        let RowWriter {
            table, first, next, ..
        } = self;
        let n = table.rows.len();
        let mut place: Vec<Option<u32>> = vec![None; n];
        let mut order: Vec<(u32, Option<u32>)> = Vec::with_capacity(n);
        let mut stack: Vec<(u32, Option<u32>)> = vec![(root, None)];
        // A child's next sibling goes on the stack before its own first
        // child, so a subtree is written whole before the sibling after it.
        while let Some((id, parent)) = stack.pop() {
            if place[id as usize].is_some() {
                continue;
            }
            if parent.is_some()
                && let Some(sib) = next[id as usize]
            {
                stack.push((sib, parent));
            }
            place[id as usize] = Some(order.len() as u32);
            order.push((id, parent));
            let kind = table.rows[id as usize].kind;
            if matches!(
                kind,
                NodeKind::Mapping | NodeKind::Sequence | NodeKind::KeyValue
            ) && let Some(f) = first[id as usize]
            {
                stack.push((f, Some(id)));
            }
        }
        let NodeTable {
            rows,
            regions,
            mentions,
            comments,
            directives,
            diagnostics,
            ..
        } = table;
        let mut old: Vec<Option<NodeRow>> = rows.into_iter().map(Some).collect();
        let mut out = NodeTable::new();
        for (id, parent) in order {
            let mut r = old[id as usize].take().expect("a row is written once");
            r.parent = parent.and_then(|p| place[p as usize]);
            out.rows.push(r);
        }
        let moved = |node: u32| place.get(node as usize).copied().flatten();
        let slot = |s: CommentSlot| match s {
            CommentSlot::Leading => 0,
            CommentSlot::Trailing => 1,
            CommentSlot::Dangling => 2,
        };
        let mut cs: Vec<CommentRow> = comments
            .into_iter()
            .filter_map(|mut c| {
                c.node = moved(c.node)?;
                Some(c)
            })
            .collect();
        // Stable: a slot's comments keep the order they were written in.
        cs.sort_by_key(|c| (c.node, slot(c.slot)));
        let mut kept = Vec::with_capacity(cs.len());
        for (i, c) in cs.iter().enumerate() {
            let replaced = c.slot == CommentSlot::Trailing
                && cs
                    .get(i + 1)
                    .is_some_and(|d| d.node == c.node && d.slot == CommentSlot::Trailing);
            if !replaced {
                kept.push(c.clone());
            }
        }
        out.comments = kept;
        out.regions = regions
            .into_iter()
            .filter_map(|mut r| {
                r.node = moved(r.node)?;
                Some(r)
            })
            .collect();
        out.regions.sort_by_key(|r| r.node);
        out.mentions = mentions
            .into_iter()
            .filter_map(|mut m| {
                m.node = moved(m.node)?;
                Some(m)
            })
            .collect();
        out.mentions.sort_by_key(|m| m.node);
        out.directives = directives;
        out.diagnostics = diagnostics;
        out
    }
}
