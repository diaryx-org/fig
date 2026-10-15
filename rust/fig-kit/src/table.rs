//! Port of kit/src/table.zig.
//!
//! The node table: what a language's `parse` writes and its `print` reads,
//! as ABI 4 spells it (docs/proposals/core-only.md §4.9).
//!
//! A language's parse fills a `Table` through its calls — `add_rows` hands
//! back rows to write in place, `add_string` copies decoded text into the
//! table's buffer, and each side fact is appended to a list of its own.
//! `print` reads one fig filled, through a `View`.
//!
//! A language writes a row's `parent`; fig derives the child links
//! (`first_child`, `next_sibling`) while it checks the table. Every side
//! fact is sparse: a document with no anchors, tags or comments has no row
//! in those lists. Every offset is a `u32`, so a source is at most 4 GiB.
//!
//! **Ownership (Rust).** Zig's table borrows its source. Here the table
//! owns it, shared, as an `Arc<[u8]>`: the `AST` adopted from the table and
//! the `Document` over it hold the same allocation, so either can be stored
//! and passed around without a lifetime. `Table::text` tells an in-source
//! slice from any other by address, as Zig's does, so a parser must slice
//! the table's own source (`Table::source_arc`, cloned before it borrows
//! the table mutably) for its texts to stay in the source rather than be
//! copied into the buffer.
//!
//! The rows and side facts are `#[repr(C)]`, laid out as `fig.h`'s
//! `FigRow`, `FigText` and `FigTable*` records (the C `reserved` bytes are
//! padding here), so the C ABI (`fig-capi`) hands a language a row or a
//! list of a `Table` to write in place, as Zig's `extern struct`s are. A
//! `View` is a struct of slices borrowing a `Table`; its C form, raw
//! pointers and `size`, is `fig-capi`'s.

use std::sync::Arc;

use crate::err;
use crate::Result;

/// No node, no offset: an absent parent, span or text.
pub const NONE: u32 = u32::MAX;

/// A row's text: `len` bytes at `off` in the source, or — `len`'s high bit
/// set — in the table's string buffer. `len == NONE` is no text at all,
/// which is not the empty string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Text {
    pub off: u32,
    pub len: u32,
}

impl Default for Text {
    fn default() -> Self {
        Text::ABSENT
    }
}

impl Text {
    pub const ABSENT: Text = Text { off: 0, len: NONE };
    pub const BUFFER_BIT: u32 = 1 << 31;

    /// `source[off .. off + len]`.
    pub fn in_source(off: usize, len: usize) -> Text {
        Text { off: off as u32, len: len as u32 }
    }

    pub fn is_absent(self) -> bool {
        self.len == NONE
    }
}

/// `Row.kind`: a scalar kind, a container kind, or `EXT + ExtKind`.
pub struct Kind;

impl Kind {
    pub const NULL: u8 = 0;
    pub const FALSE: u8 = 1;
    pub const TRUE: u8 = 2;
    pub const STRING: u8 = 3;
    pub const INT: u8 = 4;
    pub const FLOAT: u8 = 5;
    pub const SEQUENCE: u8 = 6;
    pub const MAPPING: u8 = 7;
    pub const KEYVALUE: u8 = 8;
    pub const ALIAS: u8 = 9;
    /// An extended scalar: `EXT` plus the `ExtKind` ordinal.
    pub const EXT: u8 = 16;

    /// Whether a row of this kind carries text: a string's decoded bytes,
    /// a number's lexeme, an alias's anchor name, an extended scalar's
    /// payload.
    pub fn has_text(k: u8) -> bool {
        k == Self::STRING || k == Self::INT || k == Self::FLOAT || k == Self::ALIAS || k >= Self::EXT
    }

    pub fn is_container(k: u8) -> bool {
        k == Self::SEQUENCE || k == Self::MAPPING || k == Self::KEYVALUE
    }
}

/// `Row.flags`: a sequence with no delimiters, whose items run from its
/// span's start to its end.
pub const ROW_OPEN: u8 = 1 << 0;

/// One node. Row index is node id; rows are in pre-order, so a parent
/// precedes its children, and a keyvalue's two children are its key and
/// then its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Row {
    pub kind: u8,
    pub flags: u8,
    /// The parent's row, or `NONE` for the root, which is row 0.
    pub parent: u32,
    /// The node's span in the source; `NONE` on the print side, where there
    /// is no source.
    pub start: u32,
    pub end: u32,
    pub text: Text,
    /// Filled by fig while it checks the table; a language leaves them.
    pub first_child: u32,
    pub next_sibling: u32,
}

impl Default for Row {
    fn default() -> Self {
        Row {
            kind: Kind::NULL,
            flags: 0,
            parent: NONE,
            start: NONE,
            end: NONE,
            text: Text::ABSENT,
            first_child: NONE,
            next_sibling: NONE,
        }
    }
}

/// A side fact that names a row: what `fact_of` searches by.
pub trait Fact: Copy {
    fn node(&self) -> u32;
    fn set_node(&mut self, node: u32);
}

macro_rules! fact {
    ($t:ty) => {
        impl Fact for $t {
            fn node(&self) -> u32 {
                self.node
            }
            fn set_node(&mut self, node: u32) {
                self.node = node;
            }
        }
    };
}

/// An anchor defined on `node`, and where its `&name` is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Anchor {
    pub node: u32,
    pub start: u32,
    pub end: u32,
    pub name: Text,
}

impl Default for Anchor {
    fn default() -> Self {
        Anchor { node: 0, start: NONE, end: NONE, name: Text::ABSENT }
    }
}
fact!(Anchor);

/// `Tag.resolved`: what the language made of a tag.
pub const TAG_UNSTATED: u8 = 0;
/// A tag with no core meaning (`!foo`), kept for a language that spells it.
pub const TAG_CUSTOM: u8 = 1;
/// The non-specific tag (YAML's `!`), which asserts nothing; and a tag the
/// language has already read into the node's own kind (YAML's `!!binary`,
/// on the byte string it made the node), which asks nothing more of it.
pub const TAG_NON_SPECIFIC: u8 = 2;
/// A core type: `TAG_KIND + KindTag ordinal` (null, boolean, string,
/// integer, float, sequence, mapping).
pub const TAG_KIND: u8 = 3;

/// A tag on `node`: its text as written, for a language that re-emits it,
/// what the language resolved it to (`TAG_*`), and whether the node's text
/// agrees with that resolution (`!!int` on `1 - 3` does not). Where the
/// `!tag` is written, or `NONE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Tag {
    pub node: u32,
    pub start: u32,
    pub end: u32,
    pub text: Text,
    pub resolved: u8,
    pub agrees: u8,
}

impl Default for Tag {
    fn default() -> Self {
        Tag { node: 0, start: NONE, end: NONE, text: Text::ABSENT, resolved: TAG_UNSTATED, agrees: 1 }
    }
}
fact!(Tag);

/// A fact with a span: a block-sequence item's marker, a keyvalue's
/// separator, a section's header line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct SpanFact {
    pub node: u32,
    pub start: u32,
    pub end: u32,
}
fact!(SpanFact);

/// The bytes a separated sequence writes between two items on one line,
/// or absent for the dialect's `flow_entry_sep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct ItemSep {
    pub node: u32,
    pub text: Text,
}
fact!(ItemSep);

pub const COMMENT_LEADING: u8 = 0;
pub const COMMENT_TRAILING: u8 = 1;
pub const COMMENT_DANGLING: u8 = 2;
pub const COMMENT_LINE: u8 = 0;
pub const COMMENT_BLOCK: u8 = 1;

/// A comment bound to `node`, in its `slot`, in source order within it; at
/// most one trailing per node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct Comment {
    pub node: u32,
    pub slot: u8,
    pub style: u8,
    pub text: Text,
}
fact!(Comment);

pub const MENTION_HEADER: u8 = 0;
pub const MENTION_ENTRY: u8 = 1;
pub const MENTION_INDEX: u8 = 2;
pub const MENTION_APPEND: u8 = 3;

/// A place a section node's name is written: a header line of its own, or
/// its parent's entry line (a dotted key). Or a place a sequence item's
/// path is written: its index (`MENTION_INDEX`), or the header that
/// appended it (`MENTION_APPEND`). See `document::MentionKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct Mention {
    pub node: u32,
    pub start: u32,
    pub end: u32,
    pub kind: u8,
}
fact!(Mention);

/// A tag-handle declaration of the document (YAML's `%TAG`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct Directive {
    pub handle: Text,
    pub prefix: Text,
}

pub const DIAGNOSTIC_ERROR: u8 = 0;
pub const DIAGNOSTIC_WARNING: u8 = 1;

/// What a parse found worth telling: an error (a parse that fails may still
/// leave the ones it found) or a warning (a lint on input that parses), its
/// span — `end` may be `NONE` for a point — a sentence that says what is
/// wrong and how to fix it, and a few words for the caret's label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Diagnostic {
    pub severity: u8,
    pub start: u32,
    pub end: u32,
    pub message: Text,
    pub label: Text,
}

impl Default for Diagnostic {
    fn default() -> Self {
        Diagnostic { severity: DIAGNOSTIC_ERROR, start: 0, end: NONE, message: Text::ABSENT, label: Text::ABSENT }
    }
}

/// What a `print` is handed, and what any reader of a table reads: every
/// list as a slice borrowing the table. (Zig's is pointers and counts, for
/// the C ABI.)
#[derive(Debug, Clone, Copy, Default)]
pub struct View<'a> {
    pub source: &'a [u8],
    pub strings: &'a [u8],
    pub rows: &'a [Row],
    pub anchors: &'a [Anchor],
    pub tags: &'a [Tag],
    pub markers: &'a [SpanFact],
    pub seps: &'a [SpanFact],
    pub item_seps: &'a [ItemSep],
    pub comments: &'a [Comment],
    pub regions: &'a [SpanFact],
    pub mentions: &'a [Mention],
    pub directives: &'a [Directive],
    pub diagnostics: &'a [Diagnostic],
}

impl<'a> View<'a> {
    pub fn row_slice(&self) -> &'a [Row] {
        self.rows
    }

    /// The bytes `t` names, or `None` when it is absent or out of range.
    pub fn text_of(&self, t: Text) -> Option<&'a [u8]> {
        view_text(self.source, self.strings, t)
    }

    /// Row `node`'s side facts, each found by binary search: every list is
    /// in node order, as every writer in fig appends it.
    pub fn facts(&self, node: u32) -> Facts {
        Facts {
            anchor: fact_of(self.anchors, node),
            tag: fact_of(self.tags, node),
            marker: fact_of(self.markers, node),
            sep: fact_of(self.seps, node),
            item_sep: fact_of(self.item_seps, node),
        }
    }
}

/// A row's side facts, gathered (`View::facts`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    pub anchor: Option<Anchor>,
    pub tag: Option<Tag>,
    pub marker: Option<SpanFact>,
    pub sep: Option<SpanFact>,
    pub item_sep: Option<ItemSep>,
}

/// The first fact of `items` naming `node`, by binary search over a list in
/// node order.
pub fn fact_of<F: Fact>(items: &[F], node: u32) -> Option<F> {
    let mut lo = 0usize;
    let mut hi = items.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if items[mid].node() < node {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo < items.len() && items[lo].node() == node { Some(items[lo]) } else { None }
}

/// The table itself, fig's. A language writes it through the calls below
/// (`fig_table_*` in C) and reads it through `view`.
#[derive(Debug, Clone)]
pub struct Table {
    /// What row spans and source texts index; empty on the print side.
    pub source: Arc<[u8]>,
    pub rows: Vec<Row>,
    pub strings: Vec<u8>,
    pub anchors: Vec<Anchor>,
    pub tags: Vec<Tag>,
    pub markers: Vec<SpanFact>,
    pub seps: Vec<SpanFact>,
    pub item_seps: Vec<ItemSep>,
    pub comments: Vec<Comment>,
    pub regions: Vec<SpanFact>,
    pub mentions: Vec<Mention>,
    pub directives: Vec<Directive>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Default for Table {
    fn default() -> Self {
        Table::init(Arc::<[u8]>::from(&[][..]))
    }
}

impl Table {
    /// An empty table over `source` (a `&[u8]` is copied; an `Arc` or a
    /// `Vec` is taken).
    pub fn init(source: impl Into<Arc<[u8]>>) -> Table {
        Table {
            source: source.into(),
            rows: Vec::new(),
            strings: Vec::new(),
            anchors: Vec::new(),
            tags: Vec::new(),
            markers: Vec::new(),
            seps: Vec::new(),
            item_seps: Vec::new(),
            comments: Vec::new(),
            regions: Vec::new(),
            mentions: Vec::new(),
            directives: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    /// The source, shared: what a parser scans, so that the slices it hands
    /// `text` are recognised as in-source.
    pub fn source_arc(&self) -> Arc<[u8]> {
        self.source.clone()
    }

    /// `n` new rows, each a null with no parent, span or text, to be filled
    /// in place. Valid until the next `add_rows`.
    pub fn add_rows(&mut self, n: usize) -> Result<&mut [Row]> {
        if self.rows.len() + n >= NONE as usize {
            return Err(err!(OutOfMemory));
        }
        let from = self.rows.len();
        self.rows.resize(from + n, Row::default());
        Ok(&mut self.rows[from..])
    }

    /// One new row: `add_rows(1)[0]`, returning its index.
    pub fn add_row(&mut self, row: Row) -> Result<u32> {
        let i = self.rows.len() as u32;
        self.add_rows(1)?[0] = row;
        Ok(i)
    }

    /// `bytes` copied into the string buffer.
    pub fn add_string(&mut self, bytes: &[u8]) -> Result<Text> {
        if bytes.len() >= Text::BUFFER_BIT as usize || self.strings.len() + bytes.len() >= Text::BUFFER_BIT as usize {
            return Err(err!(OutOfMemory));
        }
        let off = self.strings.len() as u32;
        self.strings.extend_from_slice(bytes);
        Ok(Text { off, len: bytes.len() as u32 | Text::BUFFER_BIT })
    }

    /// `bytes` as text: in the source, when `bytes` lies within it (by
    /// address), else copied into the buffer.
    pub fn text(&mut self, bytes: &[u8]) -> Result<Text> {
        if !self.source.is_empty() && !bytes.is_empty() {
            let base = self.source.as_ptr() as usize;
            let at = bytes.as_ptr() as usize;
            if at >= base && at + bytes.len() <= base + self.source.len() {
                return Ok(Text::in_source(at - base, bytes.len()));
            }
        }
        self.add_string(bytes)
    }

    /// The bytes `t` names, or `None` when it is absent or out of range.
    pub fn text_of(&self, t: Text) -> Option<&[u8]> {
        view_text(&self.source, &self.strings, t)
    }

    pub fn view(&self) -> View<'_> {
        View {
            source: &self.source,
            strings: &self.strings,
            rows: &self.rows,
            anchors: &self.anchors,
            tags: &self.tags,
            markers: &self.markers,
            seps: &self.seps,
            item_seps: &self.item_seps,
            comments: &self.comments,
            regions: &self.regions,
            mentions: &self.mentions,
            directives: &self.directives,
            diagnostics: &self.diagnostics,
        }
    }
}

/// The bytes `t` names in `source` or `strings`, or `None` when it is absent
/// or out of range.
pub fn view_text<'a>(source: &'a [u8], strings: &'a [u8], t: Text) -> Option<&'a [u8]> {
    if t.is_absent() {
        return None;
    }
    if t.len & Text::BUFFER_BIT != 0 {
        let len = (t.len & !Text::BUFFER_BIT) as usize;
        if t.off as u64 + len as u64 > strings.len() as u64 {
            return None;
        }
        return Some(&strings[t.off as usize..t.off as usize + len]);
    }
    if t.off as u64 + t.len as u64 > source.len() as u64 {
        return None;
    }
    Some(&source[t.off as usize..t.off as usize + t.len as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_names_the_source_or_the_buffer_and_an_absent_one_is_neither() {
        let mut t = Table::init(&b"hello world"[..]);
        let src = t.source_arc();
        let s = t.text(&src[6..11]).unwrap();
        assert_eq!(6, s.off);
        assert_eq!(b"world", t.text_of(s).unwrap());
        let buf = t.text(b"decoded").unwrap();
        assert!(buf.len & Text::BUFFER_BIT != 0);
        assert_eq!(b"decoded", t.text_of(buf).unwrap());
        assert!(t.text_of(Text::ABSENT).is_none());
        let e = t.add_string(b"").unwrap();
        assert_eq!(b"", t.text_of(e).unwrap());
    }
}
