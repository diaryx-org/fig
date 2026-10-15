//! Languages resolved at runtime: the [`Language`] trait, the node table its
//! `parse` returns, and [`register`], which carries an implementation to the
//! core — as the core's own `Language`, with the semantics the C ABI's
//! vtable gave it — and hands back a [`Format`] that every entry point of
//! this crate then accepts.
//!
//! This is the in-process carrier of the core's runtime-language contract
//! (`docs/proposals/runtime-languages.md`): the same declarations a compiled
//! format makes, as a [`Description`]; the same parse result, as a
//! [`NodeTable`]; the same fragment renderers, as [`Language::render`]. The
//! core validates the description by the rules it holds its own formats to,
//! runs its harness over the `samples` the description declares — each is
//! parsed, printed, reparsed and edited — and only then registers the
//! language. A description that fails either is refused with the reason in
//! [`Error::Language`] and registers nothing.
//!
//! The out-of-process carrier — the same calls as JSON over a child
//! process's stdio — is [`crate::helper`], which serves any [`Language`] to
//! a `fig` binary and is what a helper written against this crate speaks.
//!
//! A registered language lives for the rest of the process: there is no
//! unregistration, and the [`Format`] it returns is valid until exit. Its
//! handle is per process; persist [`Format::name`] and resolve it with
//! [`Format::by_name`].

use std::os::raw::c_int;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use fig_core::kit::ast::{AST, NodeId, SerializeOptions as CoreSerializeOptions};
use fig_core::kit::convert;
use fig_core::kit::language::{self as core_language, Language as CoreLanguage};
use fig_core::kit::manifest;
use fig_core::kit::table::{self, Kind as RowKind, Table};
use fig_core::registry::{self, RegisterOptions};

use crate::{Capabilities, Error, ExtKind, Format, LanguageFailure, Span};

// ── the description ────────────────────────────────────────────────────────

/// What a language declares: the `Language` declarations of a compiled
/// format, as data. See the core's `manifest.zig` for what each field means
/// to the editor; the doc on each here says what it is, not why.
///
/// This and the structs it holds are `#[non_exhaustive]`, since the
/// contract gains fields as the core's does: a field appended here is a
/// minor release. Their fields stay public, so build one with its `new`
/// constructor (or `Default`) and assign the fields you need —
/// `let mut d = Description::new("hcl"); d.samples = vec![…];` — rather than
/// with a struct literal, which only this crate can write.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Description {
    /// The language's name; also its first dialect's.
    pub name: String,
    /// What it can do. `read` is required.
    pub caps: Capabilities,
    /// How deep a mapping may nest, or `None` for unbounded. `Some(0)` is a
    /// flat format (dotenv), `Some(1)` a root mapping and one level of
    /// sections (INI).
    pub max_mapping_depth: Option<u8>,
    /// Which kinds the format holds natively — what the `$fig` lossless
    /// envelope need not wrap — or `None` for no envelope at all. Requires
    /// `caps.serialize`.
    pub lossless: Option<NativeKinds>,
    /// What the splice engine needs to write this format. Required iff
    /// `caps.edit`.
    pub syntax: Option<Syntax>,
    /// At least one; the first is named after the language.
    pub dialects: Vec<Dialect>,
    /// Small documents in the format's own grammar. Required, and at least
    /// one: registration checks the language against them.
    pub samples: Vec<String>,
    /// Which fragment renderers [`Language::render`] answers. A renderer
    /// declared here and not answered is a failed edit; one answered and not
    /// declared is never called.
    pub renderers: Renderers,
    /// The language splits multi-document streams, through
    /// [`Language::split_stream`].
    pub streams: bool,
}

/// One document of a multi-document stream: its byte range `[start, end)`
/// of the stream's input, which parses as a document of its own, and
/// whether it opened with a document-start marker (YAML's `---`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamDocument {
    pub start: usize,
    pub end: usize,
    pub explicit: bool,
}

impl Description {
    /// A description with `name`, one dialect of the same name, and the
    /// default (read-only) capabilities; assign the rest of the fields.
    pub fn new(name: &str) -> Self {
        Description {
            name: name.to_owned(),
            caps: Capabilities::default(),
            dialects: vec![Dialect::new(name)],
            ..Default::default()
        }
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities::new(true, false, false)
    }
}

/// One dialect of a language. Built with [`Dialect::new`], since a dialect
/// without a name is not one; the rest of the fields are assigned.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Dialect {
    /// The name every entry point resolves it by.
    pub name: String,
    /// File extensions, without the dot.
    pub extensions: Vec<String>,
    /// How edit text is taken.
    pub splice: Splice,
    /// What `set` writes to a file that does not exist yet; `None` refuses
    /// creation. The empty string means an empty file already parses.
    pub empty_doc_seed: Option<String>,
    /// This dialect's own syntax where it differs from the language's.
    pub syntax: Option<Syntax>,
    /// What the dialect's printer makes of a tree from another format, for
    /// the conversion warnings; `None` leaves them to `caps` and `lossless`.
    pub prints: Option<Prints>,
}

/// What a dialect's printer writes of a tree read from another format: the
/// declaration fig's conversion warnings are computed from.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Prints {
    /// The kinds written as themselves. A `null` not held is dropped; an
    /// extended scalar not held degrades.
    pub holds: NativeKinds,
    /// Booleans and numbers written as themselves; false where every scalar
    /// is text.
    pub typed_scalars: bool,
    /// A `null` at the root is an empty document, not a loss.
    pub null_at_root: bool,
    /// The root must be a mapping or a sequence.
    pub root_container: bool,
    /// What a char literal not held is written as (`"number"`, `"string"`).
    pub char_as: String,
    /// What a non-finite float not held is written as.
    pub number_special_as: String,
    /// When the printer writes comments.
    pub comments: CommentsPrinted,
}

impl Prints {
    /// A printer that holds `holds`, types its scalars, and writes every
    /// comment; an unheld char or non-finite float becomes a string.
    pub fn new(holds: NativeKinds) -> Self {
        Prints {
            holds,
            typed_scalars: true,
            null_at_root: false,
            root_container: false,
            char_as: "string".to_owned(),
            number_special_as: "string".to_owned(),
            comments: CommentsPrinted::Always,
        }
    }
}

/// When a printer writes comments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum CommentsPrinted {
    #[default]
    Always,
    /// Only when pretty-printing.
    Pretty,
    Never,
}

impl Dialect {
    /// A dialect named `name`, with no extensions, literal splicing, no
    /// empty-document seed and the language's own syntax.
    pub fn new(name: &str) -> Self {
        Dialect {
            name: name.to_owned(),
            extensions: Vec::new(),
            splice: Splice::Literal,
            empty_doc_seed: None,
            syntax: None,
            prints: None,
        }
    }
}

/// How a dialect takes spliced edit text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Splice {
    /// As written.
    #[default]
    Literal,
    /// Quoted, with `"`, `\` and control characters escaped.
    Escaped,
    /// Raw bytes, no quoting.
    Raw,
}

/// Which kinds a format holds natively. One field per [`ExtKind`], plus null.
/// Start from `Default` (nothing native) and set the kinds the format holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NativeKinds {
    pub null: bool,
    pub offset_datetime: bool,
    pub local_datetime: bool,
    pub local_date: bool,
    pub local_time: bool,
    pub enum_literal: bool,
    pub char_literal: bool,
    pub number_special: bool,
    pub plist_date: bool,
    pub data: bool,
}

/// Which renderers a language answers. Start from `Default` (none) and set
/// the ones the language answers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Renderers {
    pub value: bool,
    pub entry: bool,
    pub item: bool,
    pub tail: bool,
    pub key: bool,
    pub key_name: bool,
    pub string: bool,
}

/// The surface syntax the splice engine writes a format with. Every field
/// is the core's `manifest.Syntax` field of the same name. Start from
/// `Default` and assign what differs.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Syntax {
    pub comments: Comments,
    /// The key/value separator the engine writes, or `None` when it never
    /// writes `key<sep>value` for this format — which requires an `entry`
    /// renderer.
    pub kv_sep: Option<String>,
    pub flow_kv_sep_from_siblings: bool,
    pub flow_map_pad: String,
    /// A byte every key starts with.
    pub key_sigil: Option<u8>,
    pub empty_map_literal: Option<String>,
    pub block_seq_editable: bool,
    pub flow_containers: bool,
    pub indent_unit: String,
    pub seq_item_marker: String,
    pub closed_containers: Option<ClosedContainers>,
    pub single_line_block_mapping: bool,
    pub bare_document_mapping: bool,
    pub flow_map_open: String,
    pub flow_map_close: String,
    pub structural_indent: bool,
    /// What the format calls a container scattered through the source
    /// ("table", "section"); `None` for a format with none.
    pub section_noun: Option<String>,
    pub section_header: Option<SectionHeader>,
    pub merge_key: Option<String>,
    /// The bytes between two members of a flow container: `,` by default,
    /// `;` for a CSS declaration block. Never empty.
    pub flow_entry_sep: String,
    /// The document root is a flow mapping spelled with no delimiters —
    /// members joined by `flow_entry_sep` from the first byte to the last,
    /// as a CSS `style` attribute is. Requires `flow_containers`.
    pub flow_root: bool,
    /// Only a mapping opener opens a flow container; a `[` does not open a
    /// flow sequence (a CSS attribute selector begins with one).
    pub flow_maps_only: bool,
    /// Bytes besides whitespace and `seq_item_marker`'s that may stand
    /// before a node at the start of its line (figl's `>` and `+`).
    pub line_prefix_marks: String,
    /// The key/value separator of a flow object's entries, where it differs
    /// from a block entry's (figl's `=` in `{a=1}`): the first entry of an
    /// empty flow object is written with it. Empty for none.
    pub flow_map_kv_sep: String,
    /// The bytes between two members of a flow object whose entries are
    /// written with `flow_map_kv_sep` (figl's ` ` in `{a=1 b=2}`). Empty
    /// for none: `flow_entry_sep` and a space.
    pub flow_map_entry_sep: String,
    /// The byte that, last on a line, carries it on to the next
    /// (`.properties`' `\`). The engine ends a continued line with a
    /// blank one before writing a line after it.
    pub line_continuation: Option<u8>,
    /// The empty list `set` seeds to start one that is absent (`[]`), or
    /// `None` for a format with no spelling for one.
    pub empty_seq_literal: Option<String>,
    /// Whether a scalar written over several lines takes its trailing
    /// comment on the line it opens on (figl's `x = ''' # note`) rather
    /// than after its close.
    pub opener_line_comment: bool,
}

impl Default for Syntax {
    /// The core's defaults: `#` comments, `: ` separator is NOT assumed (set
    /// `kv_sep`), two-space indent, `- ` items, `{`/`}` flow maps.
    fn default() -> Self {
        Syntax {
            comments: Comments::hash(),
            kv_sep: None,
            flow_kv_sep_from_siblings: false,
            flow_map_pad: String::new(),
            key_sigil: None,
            empty_map_literal: None,
            block_seq_editable: true,
            flow_containers: true,
            indent_unit: "  ".to_owned(),
            seq_item_marker: "- ".to_owned(),
            closed_containers: None,
            single_line_block_mapping: false,
            bare_document_mapping: true,
            flow_map_open: "{".to_owned(),
            flow_map_close: "}".to_owned(),
            structural_indent: false,
            section_noun: None,
            section_header: None,
            merge_key: None,
            flow_entry_sep: ",".to_owned(),
            flow_root: false,
            flow_maps_only: false,
            line_prefix_marks: String::new(),
            flow_map_kv_sep: String::new(),
            flow_map_entry_sep: String::new(),
            line_continuation: None,
            empty_seq_literal: None,
            opener_line_comment: false,
        }
    }
}

/// A format's comment surface: [`Comments::new`], or one of the shorthands
/// [`hash`](Comments::hash), [`slashes`](Comments::slashes) and
/// [`none`](Comments::none).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Comments {
    /// The own-line delimiter, or `None` for a format with no comments.
    pub line: Option<CommentDelimiter>,
    /// The same-line trailing delimiter, or `None` where a marker after a
    /// value is value text.
    pub trailing: Option<CommentDelimiter>,
    /// A paired comment that may run over several lines (`/*`, `*/`), which
    /// the owned-comment scan walks as a unit.
    pub block: Option<CommentDelimiter>,
}

impl Comments {
    /// A comment surface with `line` as its own-line delimiter and
    /// `trailing` as its same-line one, and no block comment.
    pub fn new(line: Option<CommentDelimiter>, trailing: Option<CommentDelimiter>) -> Self {
        Comments {
            line,
            trailing,
            block: None,
        }
    }

    /// `#` throughout.
    pub fn hash() -> Self {
        Comments::new(
            Some(CommentDelimiter::open("#")),
            Some(CommentDelimiter::open("#")),
        )
    }
    /// `//` throughout.
    pub fn slashes() -> Self {
        Comments::new(
            Some(CommentDelimiter::open("//")),
            Some(CommentDelimiter::open("//")),
        )
    }
    /// No comment syntax at all.
    pub fn none() -> Self {
        Comments::new(None, None)
    }
}

/// How one comment is delimited: [`CommentDelimiter::open`] for an
/// unpaired marker, [`CommentDelimiter::pair`] for a paired one, and
/// `forbidden` assigned where the body has text it may not contain.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommentDelimiter {
    pub open: String,
    /// Empty for an unpaired delimiter.
    pub close: String,
    /// Text a comment body may not contain.
    pub forbidden: Option<String>,
}

impl CommentDelimiter {
    /// An unpaired delimiter: `open` to the end of the line.
    pub fn open(open: &str) -> Self {
        CommentDelimiter {
            open: open.to_owned(),
            close: String::new(),
            forbidden: None,
        }
    }
    /// A paired delimiter, `open` … `close`.
    pub fn pair(open: &str, close: &str) -> Self {
        CommentDelimiter {
            open: open.to_owned(),
            close: close.to_owned(),
            forbidden: None,
        }
    }
}

/// How a section format spells a header line.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SectionHeader {
    pub open: String,
    pub close: String,
    /// The array-of-sections spelling (TOML's `[[`/`]]`), or `None` where
    /// the format has none.
    pub seq_open: Option<String>,
    pub seq_close: Option<String>,
    pub sep: String,
    pub skip_index: bool,
}

impl SectionHeader {
    /// A header spelled `open` name `close`, its path segments joined by
    /// `sep` — `SectionHeader::new("[", "]", ".")` — with no
    /// array-of-sections spelling and `skip_index` set, which is also what
    /// the wire takes an absent `skip_index` to mean.
    pub fn new(open: &str, close: &str, sep: &str) -> Self {
        SectionHeader {
            open: open.to_owned(),
            close: close.to_owned(),
            seq_open: None,
            seq_close: None,
            sep: sep.to_owned(),
            skip_index: true,
        }
    }
}

/// The self-closing spellings of an empty block container.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClosedContainers {
    pub map_open: String,
    pub map_close: String,
    pub seq_open: String,
    pub seq_close: String,
}

impl ClosedContainers {
    /// An empty mapping spelled `map_open` `map_close`, an empty sequence
    /// `seq_open` `seq_close`.
    pub fn new(map_open: &str, map_close: &str, seq_open: &str, seq_close: &str) -> Self {
        ClosedContainers {
            map_open: map_open.to_owned(),
            map_close: map_close.to_owned(),
            seq_open: seq_open.to_owned(),
            seq_close: seq_close.to_owned(),
        }
    }
}

// ── the node table ─────────────────────────────────────────────────────────

/// What a parse returns and a print receives: one [`NodeRow`] per node in
/// pre-order — row index is node id, a parent precedes its children, a
/// keyvalue is followed by its key row and then its value row — plus four
/// side tables. It is the shape the core's `Document` holds, as values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NodeTable {
    pub rows: Vec<NodeRow>,
    pub regions: Vec<RegionRow>,
    pub mentions: Vec<MentionRow>,
    pub comments: Vec<CommentRow>,
    /// The document's tag-handle declarations, in source order. A parse
    /// returns those it read; a print of a whole document — never of a
    /// fragment — receives them back, to re-emit above any tag that uses
    /// one. Empty for a format without directives.
    pub directives: Vec<DirectiveRow>,
    /// What the parse found worth telling: the errors a failing parse
    /// found — the first is the one a caller sees — and the warnings of one
    /// that succeeds. A print is handed none.
    pub diagnostics: Vec<DiagnosticRow>,
}

impl NodeTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a row and return its index — the id a child names as
    /// `parent`.
    pub fn push(&mut self, row: NodeRow) -> u32 {
        self.rows.push(row);
        (self.rows.len() - 1) as u32
    }
}

/// One node.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct NodeRow {
    pub kind: NodeKind,
    /// A format-specific scalar's kind; the row's `kind` is then the scalar
    /// it is written as (`String`, or `Int` for a char literal), where
    /// `fig_node_kind` reports the node as `FIG_NODE_EXTENDED`.
    pub ext_kind: Option<ExtKind>,
    /// `None` on the root only.
    pub parent: Option<u32>,
    /// Required of every row a parse returns; `None` in a table built for
    /// print, where there is no source.
    pub span: Option<Span>,
    /// A scalar's decoded text; an int or float's lexeme; `true`/`false`
    /// for a bool; an alias's target anchor name; an extended kind's
    /// payload. `None` for a container or a null.
    pub text: Option<String>,
    /// The anchor this row defines, and where the `&name` token is.
    pub anchor: Option<String>,
    pub anchor_span: Option<Span>,
    /// The tag on this row, verbatim, and where it is written.
    pub tag: Option<String>,
    pub tag_span: Option<Span>,
    /// For a block-sequence item: the span of the `-`/`*` introducing it.
    pub marker: Option<Span>,
    /// For a keyvalue: the span of the token separating key from value. A
    /// zero-width span marks a value hanging under a bare key.
    pub sep: Option<Span>,
    /// For a sequence whose items are joined by a separator of its own, not
    /// the dialect's `flow_entry_sep` nor one per line: the bytes written
    /// between two items on one line — `" | "`, `" "`, or `"\n\n"` for
    /// items a blank line apart. The sequence is then edited item by item
    /// with it: a delete takes one item and the separator beside it (this
    /// with its whitespace trimmed, or only whitespace where nothing is
    /// left), and an append splices after the last item, so a trailing
    /// separator stays trailing. `None` on every other row (an empty one is
    /// `None`); on a row that is not a sequence it makes the table
    /// malformed.
    pub item_sep: Option<String>,
    /// For a sequence: it has no delimiters, so its items run from its
    /// span's start to its end — a voice's bars after `Name:`. Makes the
    /// sequence a separated one, with the dialect's separator when
    /// `item_sep` is `None`.
    pub open: bool,
}

impl NodeRow {
    /// A row of `kind` under `parent` covering `span`, with nothing else.
    pub fn new(kind: NodeKind, parent: Option<u32>, span: Span) -> Self {
        NodeRow {
            kind,
            ext_kind: None,
            parent,
            span: Some(span),
            text: None,
            anchor: None,
            anchor_span: None,
            tag: None,
            tag_span: None,
            marker: None,
            sep: None,
            item_sep: None,
            open: false,
        }
    }

    pub fn with_text(mut self, text: &str) -> Self {
        self.text = Some(text.to_owned());
        self
    }

    pub fn with_sep(mut self, sep: Span) -> Self {
        self.sep = Some(sep);
        self
    }

    pub fn with_marker(mut self, marker: Span) -> Self {
        self.marker = Some(marker);
        self
    }

    /// A sequence joined by `sep` (see [`item_sep`](Self::item_sep)), with
    /// delimiters or, when `open`, none.
    pub fn with_item_sep(mut self, sep: &str, open: bool) -> Self {
        self.item_sep = Some(sep.to_owned());
        self.open = open;
        self
    }

    pub fn with_anchor(mut self, name: &str, span: Span) -> Self {
        self.anchor = Some(name.to_owned());
        self.anchor_span = Some(span);
        self
    }

    pub fn with_tag(mut self, tag: &str, span: Span) -> Self {
        self.tag = Some(tag.to_owned());
        self.tag_span = Some(span);
        self
    }

    pub fn with_ext_kind(mut self, kind: ExtKind) -> Self {
        self.ext_kind = Some(kind);
        self
    }
}

/// A row's kind: what `fig_node_kind` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NodeKind {
    Null,
    Bool,
    Int,
    Float,
    String,
    Sequence,
    Mapping,
    KeyValue,
    Alias,
}

impl NodeKind {
    /// The `FIG_ROW_*` kind of a row of this kind; a bool's is decided by
    /// its text.
    pub(crate) fn to_row(self) -> u8 {
        match self {
            NodeKind::Null => RowKind::NULL,
            NodeKind::Bool => RowKind::FALSE,
            NodeKind::Int => RowKind::INT,
            NodeKind::Float => RowKind::FLOAT,
            NodeKind::String => RowKind::STRING,
            NodeKind::Sequence => RowKind::SEQUENCE,
            NodeKind::Mapping => RowKind::MAPPING,
            NodeKind::KeyValue => RowKind::KEYVALUE,
            NodeKind::Alias => RowKind::ALIAS,
        }
    }

    pub(crate) fn from_row(v: u8) -> Option<Self> {
        Some(match v {
            RowKind::NULL => NodeKind::Null,
            RowKind::FALSE | RowKind::TRUE => NodeKind::Bool,
            RowKind::INT => NodeKind::Int,
            RowKind::FLOAT => NodeKind::Float,
            RowKind::STRING => NodeKind::String,
            RowKind::SEQUENCE => NodeKind::Sequence,
            RowKind::MAPPING => NodeKind::Mapping,
            RowKind::KEYVALUE => NodeKind::KeyValue,
            RowKind::ALIAS => NodeKind::Alias,
            _ => return None,
        })
    }
}

/// One whole header line of a section node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegionRow {
    pub node: u32,
    pub span: Span,
}

impl RegionRow {
    /// The header line of row `node`, at `span`.
    pub fn new(node: u32, span: Span) -> Self {
        RegionRow { node, span }
    }
}

/// One place a section node's name, or a sequence item's path, is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MentionRow {
    pub node: u32,
    pub span: Span,
    pub kind: MentionKind,
}

impl MentionRow {
    /// Row `node`'s name, written at `span` and sitting as `kind` says.
    pub fn new(node: u32, span: Span, kind: MentionKind) -> Self {
        MentionRow { node, span, kind }
    }
}

/// How a mention sits relative to the node's parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MentionKind {
    /// A header line of the node's own.
    Header,
    /// On one of the parent's own entry lines.
    Entry,
    /// A sequence item's index, where a path addresses it by position
    /// (figl's `1` in `xs[1]`). Deleting an earlier item renumbers it.
    Index,
    /// The header that appended a sequence item (figl's `a.b[]`), or the
    /// line repeating the header before it (figl's `+`). Deleting the item
    /// before writes its header here.
    Append,
}

/// One comment bound to a row.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommentRow {
    pub node: u32,
    pub slot: CommentSlot,
    pub style: CommentForm,
    pub text: String,
}

impl CommentRow {
    /// A `style` comment reading `text`, in `slot` on row `node`.
    pub fn new(node: u32, slot: CommentSlot, style: CommentForm, text: impl Into<String>) -> Self {
        CommentRow {
            node,
            slot,
            style,
            text: text.into(),
        }
    }
}

/// One tag-handle declaration — a YAML `%TAG` directive's handle (`!e!`,
/// or a redefined `!`/`!!`) and the prefix it expands to. A tag spelled
/// with a named handle is legal only in a document that declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DirectiveRow {
    pub handle: String,
    pub prefix: String,
}

impl DirectiveRow {
    /// `handle` declared as expanding to `prefix`.
    pub fn new(handle: impl Into<String>, prefix: impl Into<String>) -> Self {
        DirectiveRow {
            handle: handle.into(),
            prefix: prefix.into(),
        }
    }
}

/// What a parse found: an error or a warning at `span` (`end` equal to
/// `start` for a point), a sentence saying what is wrong and how to fix it,
/// and a few words for a caret's label.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiagnosticRow {
    pub severity: Severity,
    pub span: Span,
    pub message: String,
    pub label: String,
}

impl DiagnosticRow {
    pub fn new(
        severity: Severity,
        span: Span,
        message: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        DiagnosticRow {
            severity,
            span,
            message: message.into(),
            label: label.into(),
        }
    }
}

/// Whether a diagnostic stops the parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// Where a comment sits on its node: the core's three comment anchors.
///
/// Deliberately exhaustive, unlike the enums around it. A printer has to
/// put every comment somewhere, and one that met a slot it had never heard
/// of through a `_` arm could only drop it; a fourth anchor is a major
/// release, and a compile error in each printer is the right way to hear
/// about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentSlot {
    Leading,
    Trailing,
    Dangling,
}

/// A comment's written form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommentForm {
    Line,
    Block,
}

/// What a printer outside the core is told of the serialize options. A test
/// of a [`Language`] that calls its `print` directly starts from `Default`
/// and assigns what differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PrintOptions {
    pub pretty: bool,
    pub strip_comments: bool,
    pub indent: u8,
    pub width: u16,
    /// Print the value as the editor takes it spliced into a document, not
    /// as a document of its own: a root the document wraps (plist's
    /// `<plist>`) or a scalar root spelled differently from a scalar in
    /// place (NestedText's `>` block) is written bare. The bindings' editor
    /// text and `fig patch` set it; every other print leaves it `false`.
    pub splice: bool,
    /// Print a container root inline — fig's `{a=1}` / `[a, b]` — as a
    /// value spliced after `key = ` must be, since a block spelling re-reads
    /// there as a string. The bindings' editor text sets it for fig alone,
    /// so a language standing in for fig answers it; any other ignores it.
    pub flow: bool,
}

impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions {
            pretty: true,
            strip_comments: false,
            indent: 2,
            width: 80,
            splice: false,
            flow: false,
        }
    }
}

// ── the trait ──────────────────────────────────────────────────────────────

/// A failure a language reports: a message, and where in the input for a
/// parse. Built with [`LanguageError::new`] or [`LanguageError::at`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LanguageError {
    pub message: String,
    pub byte_offset: Option<usize>,
}

impl LanguageError {
    /// A failure with no location.
    pub fn new(message: impl Into<String>) -> Self {
        LanguageError {
            message: message.into(),
            byte_offset: None,
        }
    }
    /// A failure at `byte_offset` in the input.
    pub fn at(message: impl Into<String>, byte_offset: usize) -> Self {
        LanguageError {
            message: message.into(),
            byte_offset: Some(byte_offset),
        }
    }
}

impl std::fmt::Display for LanguageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.byte_offset {
            Some(off) => write!(f, "{} (byte offset {off})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for LanguageError {}

/// The fragment renderers. Non-exhaustive: a renderer the core gains is
/// only ever called on a language whose description declares it, so the
/// `_` arm [`Language::render`] needs is never reached by one it does not
/// know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Renderer {
    /// Spell a value in place (plist's typed element).
    Value,
    /// Spell a block-mapping entry past its line's indent.
    Entry,
    /// Spell a block-sequence item past its line's indent.
    Item,
    /// Spell what follows a key: the separator and the value, inline or
    /// re-framed as a block. An empty `key` is the document root.
    Tail,
    /// Spell a renamed key in the form the old one allows.
    Key,
    /// Spell a key's name as the format writes a key (JSON's `"b"`, ZON's
    /// `.b`). Without it a name is written as it stands.
    KeyName,
    /// Spell a string that replaces a string in the replaced one's quote
    /// style (`old_value`), or nothing where that style is the printer's
    /// own or cannot hold it, and the editor writes fig's spelling.
    String,
}

impl Renderer {
    pub fn name(self) -> &'static str {
        match self {
            Renderer::Value => "value",
            Renderer::Entry => "entry",
            Renderer::Item => "item",
            Renderer::Tail => "tail",
            Renderer::Key => "key",
            Renderer::KeyName => "key_name",
            Renderer::String => "string",
        }
    }
}

/// What fig's bare-literal rules make of the text a value renderer is
/// handed: `null`, `true`/`false`, a number, a datetime shape, or a
/// string. The core classifies the text once, trimmed of whitespace, by
/// the `.fig` dialect's own rules (`Yes`, `007` and `TRUE` stay strings),
/// and every format's `set` means the same thing by `42`; a renderer
/// spells the kind it is told. A datetime is a string in the node table
/// and its own answer here, since a renderer spells it differently.
///
/// Non-exhaustive: a kind fig's literal rules learn later is a minor
/// release, and a renderer's `_` arm spells it as the string every
/// untyped text falls back to anyway.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Literal {
    Null,
    Bool,
    Int,
    Float,
    Datetime,
    #[default]
    String,
}

impl Literal {
    /// The name on the vtable and the wire: `null`, `bool`, `int`,
    /// `float`, `datetime`, `string`.
    pub fn name(self) -> &'static str {
        match self {
            Literal::Null => "null",
            Literal::Bool => "bool",
            Literal::Int => "int",
            Literal::Float => "float",
            Literal::Datetime => "datetime",
            Literal::String => "string",
        }
    }

    /// The literal named `name`, or `None`.
    pub fn from_name(name: &str) -> Option<Literal> {
        Some(match name {
            "null" => Literal::Null,
            "bool" => Literal::Bool,
            "int" => Literal::Int,
            "float" => Literal::Float,
            "datetime" => Literal::Datetime,
            "string" => Literal::String,
            _ => return None,
        })
    }
}

/// What a renderer is handed. Which fields are set depends on the
/// [`Renderer`]: `value` for all but `Key`, and with it, for `Value`
/// only, `literal`, what fig's bare-literal rules make of that value;
/// `indent` for all but `Value`; `key` for `Entry`, `Tail` and `Key`;
/// `old_key` for `Key`. For `String`, `value` is the new string, decoded,
/// and `old_value` the string it replaces as the source spells it, quotes
/// and all.
///
/// `parent_key` and `parent_tag`, for every renderer, are the container
/// the fragment is written into — the mapping an entry joins, the
/// sequence an item joins, the container holding a value replaced or a
/// key renamed: the name of the key it stands under (decoded; empty at
/// the root and for a container that is itself a sequence item) and its
/// tag as a node row spells one (`!dependency`, `!!map`; empty for
/// none). An XML list spells an item by its item element's name, which
/// is exactly those two.
///
/// Non-exhaustive, since the request grows — a field appended to it is
/// what fig tells a renderer next, and not a major release — so a test of
/// a [`Language`] that calls its `render` directly builds one from
/// `Default` (every field empty, `literal` a string) with one setter per
/// field: `RenderArgs::default().value(b"42").literal(Literal::Int)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RenderArgs<'a> {
    pub dialect: &'a str,
    pub indent: &'a [u8],
    pub key: &'a [u8],
    pub value: &'a [u8],
    pub literal: Literal,
    pub old_key: &'a [u8],
    pub parent_key: &'a [u8],
    pub parent_tag: &'a [u8],
    pub old_value: &'a [u8],
}

impl<'a> RenderArgs<'a> {
    /// Set `dialect`.
    pub fn dialect(mut self, dialect: &'a str) -> Self {
        self.dialect = dialect;
        self
    }

    /// Set `indent`.
    pub fn indent(mut self, indent: &'a [u8]) -> Self {
        self.indent = indent;
        self
    }

    /// Set `key`.
    pub fn key(mut self, key: &'a [u8]) -> Self {
        self.key = key;
        self
    }

    /// Set `value`.
    pub fn value(mut self, value: &'a [u8]) -> Self {
        self.value = value;
        self
    }

    /// Set `literal`.
    pub fn literal(mut self, literal: Literal) -> Self {
        self.literal = literal;
        self
    }

    /// Set `old_key`.
    pub fn old_key(mut self, old_key: &'a [u8]) -> Self {
        self.old_key = old_key;
        self
    }

    /// Set `parent_key`.
    pub fn parent_key(mut self, parent_key: &'a [u8]) -> Self {
        self.parent_key = parent_key;
        self
    }

    /// Set `parent_tag`.
    pub fn parent_tag(mut self, parent_tag: &'a [u8]) -> Self {
        self.parent_tag = parent_tag;
        self
    }

    /// Set `old_value`.
    pub fn old_value(mut self, old_value: &'a [u8]) -> Self {
        self.old_value = old_value;
        self
    }
}

/// A format implemented in Rust, or in anything Rust can call.
///
/// `Send + Sync + 'static` because the core calls it from whichever thread
/// parses, for the life of the process.
pub trait Language: Send + Sync + 'static {
    /// The declarations. Called once, at [`register`].
    fn describe(&self) -> Description;

    /// `input` as the dialect named `dialect`. Every row needs a `span`.
    fn parse(&self, dialect: &str, input: &[u8]) -> Result<NodeTable, LanguageError>;

    /// `table` as bytes in the dialect. Called only when the description
    /// declares `caps.serialize`.
    fn print(
        &self,
        dialect: &str,
        table: &NodeTable,
        options: &PrintOptions,
    ) -> Result<Vec<u8>, LanguageError> {
        let _ = (dialect, table, options);
        Err(LanguageError::new("this language does not print"))
    }

    /// Spell a fragment for the editor. Called only for a renderer the
    /// description declares.
    fn render(&self, which: Renderer, args: RenderArgs<'_>) -> Result<Vec<u8>, LanguageError> {
        let _ = args;
        Err(LanguageError::new(format!(
            "this language does not render {}",
            which.name()
        )))
    }

    /// Locate the documents of a multi-document stream, each one's byte
    /// range to parse on its own. Called only when the description declares
    /// `streams`.
    fn split_stream(
        &self,
        dialect: &str,
        input: &[u8],
    ) -> Result<Vec<StreamDocument>, LanguageError> {
        let _ = (dialect, input);
        Err(LanguageError::new(
            "this language has no multi-document streams",
        ))
    }
}

// ── registration ───────────────────────────────────────────────────────────

/// Register `lang` with the core. Returns one [`Format`] per dialect the
/// description declares, in declaration order — the first is the language's
/// own. A runtime language is a peer of the bundled ones: a dialect named
/// after a language no crate has registered (`yaml` in a program without
/// `fig-yaml`) takes that name, and [`Format::by_name`] resolves it.
/// Refused, with the reason in [`Error::Language`], when the description
/// breaks a rule the core holds its own formats to, when a sample fails to
/// parse, print, reparse to the same tree or take a no-op edit, or when a
/// dialect's name is already registered — a registered language's names
/// included.
pub fn register(lang: impl Language) -> Result<Vec<Format>, Error> {
    let desc = lang.describe();
    let dialect_names: Vec<String> = desc.dialects.iter().map(|d| d.name.clone()).collect();
    let adapter = Registration::new(Box::new(lang), desc)?;
    registration(adapter.register())?;
    // By name, not by counting up from the first format: the core promises
    // each row a handle of its own, not that the handles are consecutive.
    dialect_names
        .iter()
        .map(|name| Format::by_name(name).ok_or(Error::Internal))
        .collect()
}

/// A registration's answer as a `Result`, as `fig_language_register`
/// reports a refusal: the reason the core gave, when it gave one, as
/// [`Error::Language`].
fn registration(r: fig_core::Result<i32>) -> Result<Format, Error> {
    let err = match r {
        Ok(abi) => return Ok(Format(abi)),
        Err(err) => err,
    };
    let (status, message) = match err.name() {
        "OutOfMemory" => (crate::capi::Status::OutOfMemory, b"out of memory".to_vec()),
        "InvalidLanguage" | "HarnessFailed" | "NameTaken" => (
            crate::capi::Status::InvalidArgument,
            registry::last_refusal(),
        ),
        _ => (
            crate::capi::Status::UnsupportedOperation,
            b"the language registry cannot take another registration".to_vec(),
        ),
    };
    // What the C `FigError`'s message buffer holds.
    let message = String::from_utf8_lossy(&message[..message.len().min(255)]).into_owned();
    if message.is_empty() {
        Error::from_status(status)?;
        return Err(Error::Internal);
    }
    Err(Error::Language(LanguageFailure {
        message,
        byte_offset: None,
    }))
}

/// Register one of fig's own languages, as `fig-core` implements it, and
/// return the format of its first dialect. What each language crate's
/// `register` is (`fig_yaml::register()`): the crate hands its language
/// here. Registering one again returns the format it already has.
pub fn register_core(lang: &'static dyn CoreLanguage) -> Result<Format, Error> {
    // Two threads registering at once: the second waits, then finds the
    // first's (the lock each language library's `fig_<name>_register` took).
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    registration(registry::register_bundled(registry::global(), lang))
}

/// A [`Language`] as the core's `Language`, read off its [`Description`]
/// once: what `fig_language_register` built from the vtable this crate
/// handed it, and the calls the vtable's thunks made. Lives for the
/// process, as a registration does.
struct Registration {
    lang: Box<dyn Language>,
    name: &'static str,
    caps: manifest::Caps,
    syntax: Option<manifest::Syntax>,
    dialects: &'static [manifest::DialectInfo],
    /// Each dialect row's own syntax, and its extensions.
    dialect_syntax: Vec<Option<manifest::Syntax>>,
    dialect_extensions: Vec<Vec<&'static str>>,
    /// Every row's extensions, in order.
    extensions: &'static [&'static str],
    samples: &'static [&'static [u8]],
    renderers: Renderers,
    streams: bool,
}

fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn leak_bytes(s: &str) -> &'static [u8] {
    leak_str(s).as_bytes()
}

/// A declared string, which the core reads as C reads one: a NUL in it is
/// refused, as it was when it crossed the ABI as a C string.
fn checked(s: &str) -> Result<&str, Error> {
    match s.find('\0') {
        None => Ok(s),
        Some(at) => Err(Error::Language(LanguageFailure {
            message: format!("a declared string contains a NUL byte: {s:?}"),
            byte_offset: Some(at),
        })),
    }
}

fn str_of(s: &str) -> Result<&'static str, Error> {
    Ok(leak_str(checked(s)?))
}

fn bytes_of(s: &str) -> Result<&'static [u8], Error> {
    Ok(leak_bytes(checked(s)?))
}

fn opt_bytes(s: Option<&str>) -> Result<Option<&'static [u8]>, Error> {
    s.map(bytes_of).transpose()
}

/// `kinds` as the core holds them.
fn native_kinds(l: &NativeKinds) -> manifest::NativeKinds {
    manifest::NativeKinds {
        null: l.null,
        offset_datetime: l.offset_datetime,
        local_datetime: l.local_datetime,
        local_date: l.local_date,
        local_time: l.local_time,
        enum_literal: l.enum_literal,
        char_literal: l.char_literal,
        number_special: l.number_special,
        plist_date: l.plist_date,
        data: l.data,
    }
}

fn delimiter(d: Option<&CommentDelimiter>) -> Result<Option<manifest::CommentDelimiter>, Error> {
    let Some(d) = d else { return Ok(None) };
    Ok(Some(manifest::CommentDelimiter {
        open: bytes_of(&d.open)?,
        close: bytes_of(&d.close)?,
        forbidden: opt_bytes(d.forbidden.as_deref())?,
    }))
}

/// `s` as the core's syntax: the runtime registry's `syntaxOf`, every
/// field as stated, a 0 byte as none.
fn syntax_of(s: &Syntax) -> Result<manifest::Syntax, Error> {
    Ok(manifest::Syntax {
        comments: manifest::Comments {
            line: delimiter(s.comments.line.as_ref())?,
            trailing: delimiter(s.comments.trailing.as_ref())?,
            block: delimiter(s.comments.block.as_ref())?,
        },
        kv_sep: opt_bytes(s.kv_sep.as_deref())?,
        flow_kv_sep_from_siblings: s.flow_kv_sep_from_siblings,
        flow_map_pad: bytes_of(&s.flow_map_pad)?,
        key_sigil: s.key_sigil.filter(|&b| b != 0),
        empty_map_literal: opt_bytes(s.empty_map_literal.as_deref())?,
        block_seq_editable: s.block_seq_editable,
        flow_containers: s.flow_containers,
        indent_unit: bytes_of(&s.indent_unit)?,
        seq_item_marker: bytes_of(&s.seq_item_marker)?,
        closed_containers: match &s.closed_containers {
            Some(c) => Some(manifest::ClosedContainers {
                map: manifest::Delimiters {
                    open: bytes_of(&c.map_open)?,
                    close: bytes_of(&c.map_close)?,
                },
                seq: manifest::Delimiters {
                    open: bytes_of(&c.seq_open)?,
                    close: bytes_of(&c.seq_close)?,
                },
            }),
            None => None,
        },
        single_line_block_mapping: s.single_line_block_mapping,
        bare_document_mapping: s.bare_document_mapping,
        flow_map_open: bytes_of(&s.flow_map_open)?,
        flow_map_close: bytes_of(&s.flow_map_close)?,
        structural_indent: s.structural_indent,
        section_noun: opt_bytes(s.section_noun.as_deref())?,
        section_header: match &s.section_header {
            Some(h) => Some(manifest::SectionHeader {
                open: bytes_of(&h.open)?,
                close: bytes_of(&h.close)?,
                seq_open: opt_bytes(h.seq_open.as_deref())?,
                seq_close: opt_bytes(h.seq_close.as_deref())?,
                sep: bytes_of(&h.sep)?,
                skip_index: h.skip_index,
            }),
            None => None,
        },
        merge_key: opt_bytes(s.merge_key.as_deref())?,
        flow_entry_sep: bytes_of(&s.flow_entry_sep)?,
        flow_root: s.flow_root,
        flow_maps_only: s.flow_maps_only,
        line_prefix_marks: bytes_of(&s.line_prefix_marks)?,
        flow_map_kv_sep: bytes_of(&s.flow_map_kv_sep)?,
        flow_map_entry_sep: bytes_of(&s.flow_map_entry_sep)?,
        line_continuation: s.line_continuation.filter(|&b| b != 0),
        empty_seq_literal: opt_bytes(s.empty_seq_literal.as_deref())?,
        opener_line_comment: s.opener_line_comment,
    })
}

fn prints_of(p: &Prints) -> Result<manifest::Prints, Error> {
    Ok(manifest::Prints {
        holds: native_kinds(&p.holds),
        typed_scalars: p.typed_scalars,
        null_at_root: p.null_at_root,
        root_container: p.root_container,
        char_as: str_of(&p.char_as)?,
        number_special_as: str_of(&p.number_special_as)?,
        comments: match p.comments {
            CommentsPrinted::Always => manifest::PrintsComments::Always,
            CommentsPrinted::Pretty => manifest::PrintsComments::Pretty,
            CommentsPrinted::Never => manifest::PrintsComments::Never,
        },
    })
}

impl Registration {
    fn new(lang: Box<dyn Language>, desc: Description) -> Result<Self, Error> {
        let name = str_of(&desc.name)?;
        let caps = manifest::Caps {
            read: desc.caps.read,
            edit: desc.caps.edit,
            serialize: desc.caps.serialize,
            references: desc.caps.references,
            lossless: desc.lossless.as_ref().map(native_kinds),
            max_mapping_depth: desc.max_mapping_depth,
        };
        let syntax = desc.syntax.as_ref().map(syntax_of).transpose()?;
        let mut dialects = Vec::with_capacity(desc.dialects.len());
        let mut dialect_syntax = Vec::with_capacity(desc.dialects.len());
        let mut dialect_extensions = Vec::with_capacity(desc.dialects.len());
        let mut extensions = Vec::new();
        for (index, d) in desc.dialects.iter().enumerate() {
            let exts: Vec<&'static str> = d
                .extensions
                .iter()
                .map(|e| str_of(e))
                .collect::<Result<_, _>>()?;
            let splice = match d.splice {
                Splice::Literal => manifest::SpliceStyle::Literal,
                Splice::Escaped => manifest::SpliceStyle::Escaped,
                Splice::Raw => manifest::SpliceStyle::Raw,
            };
            dialects.push(manifest::DialectInfo {
                dialect: index as manifest::Dialect,
                prints: d.prints.as_ref().map(prints_of).transpose()?,
                ..manifest::DialectInfo::new(
                    str_of(&d.name)?,
                    splice,
                    opt_bytes(d.empty_doc_seed.as_deref())?,
                )
            });
            dialect_syntax.push(d.syntax.as_ref().map(syntax_of).transpose()?);
            extensions.extend(exts.iter().copied());
            dialect_extensions.push(exts);
        }
        let samples: Vec<&'static [u8]> = desc.samples.iter().map(|s| leak_bytes(s)).collect();
        Ok(Registration {
            lang,
            name,
            caps,
            syntax,
            dialects: Box::leak(dialects.into_boxed_slice()),
            dialect_syntax,
            dialect_extensions,
            extensions: Box::leak(extensions.into_boxed_slice()),
            samples: Box::leak(samples.into_boxed_slice()),
            renderers: desc.renderers,
            streams: desc.streams,
        })
    }

    /// `fig_language_register`: the record's rules a core `Syntax` cannot
    /// state (an editable language states a syntax), then the registry's
    /// own validation and the harness over the samples.
    fn register(self) -> fig_core::Result<i32> {
        registry::clear_refusal();
        let name = self.name;
        if !name.is_empty()
            && self.caps.read
            && !self.dialects.is_empty()
            && self.caps.edit
            && self.syntax.is_none()
        {
            registry::refuse(&format!(
                "'{name}' declares the edit capability and must supply syntax"
            ));
            return Err(fig_core::err!(InvalidLanguage));
        }
        registry::global().register_arc(Arc::new(self), RegisterOptions::default())
    }

    fn dialect_name(&self, d: manifest::Dialect) -> &'static str {
        self.dialects.get(d as usize).map_or(self.name, |r| r.name)
    }

    fn has(&self, which: manifest::Renderer) -> bool {
        let r = &self.renderers;
        match which {
            manifest::Renderer::Value => r.value,
            manifest::Renderer::Entry => r.entry,
            manifest::Renderer::Item => r.item,
            manifest::Renderer::Tail => r.tail,
            manifest::Renderer::Key => r.key,
            manifest::Renderer::KeyName => r.key_name,
            manifest::Renderer::String => r.string,
        }
    }

    /// The vtable's `print` thunk over `ast` from `root`: the subtree's
    /// table, stripped of comments when asked, as this crate's
    /// [`NodeTable`], handed to the language with the C `PrintOptions`.
    fn print_from(
        &self,
        dialect: manifest::Dialect,
        ast: &AST,
        root: NodeId,
        options: &CoreSerializeOptions,
        out: &mut Vec<u8>,
    ) -> fig_core::Result<()> {
        if !self.caps.serialize {
            return Err(fig_core::err!(FormatDisabled));
        }
        let mut built = convert::document_to_table(ast, root)?;
        if options.strip_comments {
            built.comments.clear();
        }
        let print_options = PrintOptions {
            pretty: options.pretty,
            strip_comments: options.strip_comments,
            indent: options.indent,
            width: options.width,
            splice: options.splice,
            flow: options.flow,
        };
        let dialect = self.dialect_name(dialect);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let table = table_from_view(&built.view())?;
            self.lang.print(dialect, &table, &print_options)
        }));
        match result {
            Ok(Ok(bytes)) => {
                out.extend_from_slice(&bytes);
                Ok(())
            }
            // A refusal names no `Refusal` code: the core reads it as a
            // printer's failure.
            Ok(Err(_)) | Err(_) => Err(fig_core::err!(RuntimePrintFailed)),
        }
    }
}

// ── the language, as the core calls it ─────────────────────────────────────
//
// Each method is one of the vtable's thunks over the [`Language`] and the
// runtime registry's reading of its answer. A panic in the language is
// caught here and reported as a failure with the panic's message, as the
// thunks did at the C boundary.

fn panic_message(p: Box<dyn std::any::Any + Send>) -> LanguageError {
    let msg = p
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_owned());
    LanguageError::new(format!("the language panicked: {msg}"))
}

impl CoreLanguage for Registration {
    fn name(&self) -> &str {
        self.name
    }

    fn extensions(&self) -> &[&'static str] {
        self.extensions
    }

    fn dialect_extensions(&self, row: usize) -> Option<Vec<&'static str>> {
        Some(
            self.dialect_extensions
                .get(row)
                .cloned()
                .unwrap_or_default(),
        )
    }

    fn caps(&self) -> manifest::Caps {
        self.caps
    }

    fn dialects(&self) -> &[manifest::DialectInfo] {
        self.dialects
    }

    fn samples(&self) -> &[&'static [u8]] {
        self.samples
    }

    /// The dialect's syntax, else the language's (the runtime registry's
    /// `Language.syntax`), else none at all for a read-only format.
    fn syntax(&self, dialect: manifest::Dialect) -> manifest::Syntax {
        if let Some(Some(s)) = self.dialect_syntax.get(dialect as usize) {
            return *s;
        }
        self.syntax
            .unwrap_or(manifest::Syntax::new(manifest::Comments::NONE, None, None))
    }

    fn is_runtime(&self) -> bool {
        true
    }

    /// The language's parse, its table copied into the core's. A refusal is
    /// `RuntimeParseFailed`, its message and offset stated with
    /// `set_parse_error` (the vtable's `ErrorInfo`).
    fn parse_table(
        &self,
        input: &[u8],
        dialect: manifest::Dialect,
        out: &mut Table,
    ) -> fig_core::Result<()> {
        let dialect = self.dialect_name(dialect);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let table = self.lang.parse(dialect, input)?;
            fill_table(out, &table)
        }));
        let e = match result {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(e)) => e,
            Err(p) => panic_message(p),
        };
        core_language::set_parse_error(e.message.as_bytes(), e.byte_offset.unwrap_or(0));
        Err(fig_core::err!(RuntimeParseFailed))
    }

    fn print(
        &self,
        dialect: manifest::Dialect,
        ast: &AST,
        options: &CoreSerializeOptions,
        out: &mut Vec<u8>,
    ) -> fig_core::Result<()> {
        self.print_from(dialect, ast, ast.root, options, out)
    }

    fn print_node(
        &self,
        dialect: manifest::Dialect,
        ast: &AST,
        node: NodeId,
        options: &CoreSerializeOptions,
        out: &mut Vec<u8>,
    ) -> fig_core::Result<()> {
        self.print_from(dialect, ast, node, options, out)
    }

    /// One renderer call, when the description lists it. A refusal is the
    /// language's words in `registry::last_refusal` and `RendererRefused`
    /// (the runtime registry's `rendererError`).
    fn render(
        &self,
        which: manifest::Renderer,
        dialect: manifest::Dialect,
        out: &mut Vec<u8>,
        r: &manifest::RenderRequest<'_>,
    ) -> Option<fig_core::Result<()>> {
        if !self.has(which) {
            return None;
        }
        let renderer = match which {
            manifest::Renderer::Value => Renderer::Value,
            manifest::Renderer::Entry => Renderer::Entry,
            manifest::Renderer::Item => Renderer::Item,
            manifest::Renderer::Tail => Renderer::Tail,
            manifest::Renderer::Key => Renderer::Key,
            manifest::Renderer::KeyName => Renderer::KeyName,
            manifest::Renderer::String => Renderer::String,
        };
        let args = RenderArgs {
            dialect: self.dialect_name(dialect),
            indent: r.indent,
            key: r.key,
            value: r.value,
            // A name this crate does not know is a core newer than it; the
            // fallback is what a renderer does with any text it cannot type.
            literal: Literal::from_name(r.literal.name()).unwrap_or_default(),
            old_key: r.old_key,
            parent_key: r.parent_key,
            parent_tag: r.parent_tag,
            old_value: r.old_value,
        };
        let result = match catch_unwind(AssertUnwindSafe(|| self.lang.render(renderer, args))) {
            Ok(result) => result,
            Err(p) => Err(panic_message(p)),
        };
        Some(match result {
            Ok(bytes) => {
                out.extend_from_slice(&bytes);
                Ok(())
            }
            Err(e) => {
                // What the C `ErrorInfo`'s message buffer held.
                let bytes = &e.message.as_bytes()[..e.message.len().min(255)];
                let text = String::from_utf8_lossy(bytes);
                registry::refuse(if text.is_empty() {
                    "the renderer declined"
                } else {
                    &text
                });
                Err(fig_core::err!(RendererRefused))
            }
        })
    }

    fn render_value(
        &self,
        d: manifest::Dialect,
        out: &mut Vec<u8>,
        r: &manifest::RenderRequest<'_>,
    ) -> Option<fig_core::Result<()>> {
        self.render(manifest::Renderer::Value, d, out, r)
    }
    fn render_entry(
        &self,
        d: manifest::Dialect,
        out: &mut Vec<u8>,
        r: &manifest::RenderRequest<'_>,
    ) -> Option<fig_core::Result<()>> {
        self.render(manifest::Renderer::Entry, d, out, r)
    }
    fn render_item(
        &self,
        d: manifest::Dialect,
        out: &mut Vec<u8>,
        r: &manifest::RenderRequest<'_>,
    ) -> Option<fig_core::Result<()>> {
        self.render(manifest::Renderer::Item, d, out, r)
    }
    fn render_tail(
        &self,
        d: manifest::Dialect,
        out: &mut Vec<u8>,
        r: &manifest::RenderRequest<'_>,
    ) -> Option<fig_core::Result<()>> {
        self.render(manifest::Renderer::Tail, d, out, r)
    }
    fn render_key(
        &self,
        d: manifest::Dialect,
        out: &mut Vec<u8>,
        r: &manifest::RenderRequest<'_>,
    ) -> Option<fig_core::Result<()>> {
        self.render(manifest::Renderer::Key, d, out, r)
    }
    fn render_key_name(
        &self,
        d: manifest::Dialect,
        out: &mut Vec<u8>,
        r: &manifest::RenderRequest<'_>,
    ) -> Option<fig_core::Result<()>> {
        self.render(manifest::Renderer::KeyName, d, out, r)
    }
    fn render_string(
        &self,
        d: manifest::Dialect,
        out: &mut Vec<u8>,
        r: &manifest::RenderRequest<'_>,
    ) -> Option<fig_core::Result<()>> {
        self.render(manifest::Renderer::String, d, out, r)
    }

    /// Whether the description lists the renderer (the vtable slot is
    /// non-null), without asking the language.
    fn has_renderer(&self, _dialect: manifest::Dialect, which: manifest::Renderer) -> bool {
        self.has(which)
    }

    /// `split_stream`, when the description declares `streams`. A refusal
    /// is `RuntimeParseFailed`.
    fn split_stream(
        &self,
        input: &[u8],
        dialect: manifest::Dialect,
    ) -> Option<fig_core::Result<Vec<manifest::StreamDocument>>> {
        if !self.streams {
            return None;
        }
        let dialect = self.dialect_name(dialect);
        let result = catch_unwind(AssertUnwindSafe(|| self.lang.split_stream(dialect, input)));
        Some(match result {
            Ok(Ok(docs)) => Ok(docs
                .into_iter()
                .map(|d| manifest::StreamDocument {
                    start: d.start,
                    end: d.end,
                    explicit: d.explicit,
                })
                .collect()),
            Ok(Err(_)) | Err(_) => Err(fig_core::err!(RuntimeParseFailed)),
        })
    }

    fn has_split_stream(&self) -> bool {
        self.streams
    }
}

// ── the node table, across ─────────────────────────────────────────────────

/// `table` written into the core's table `t`, as the `fig_table_*` calls
/// wrote it: every text copied into the table's buffer, and a side fact on
/// a row that does not exist refused.
fn fill_table(t: &mut Table, table: &NodeTable) -> Result<(), LanguageError> {
    let oom = || LanguageError::new("out of memory filling the node table");
    let refused = || LanguageError::new("fig refused a node table fact");
    let span = |s: Option<Span>| -> (u32, u32) {
        match s {
            Some(s) => (s.start as u32, s.end as u32),
            None => (table::NONE, table::NONE),
        }
    };
    fn text(t: &mut Table, s: &str) -> Result<table::Text, LanguageError> {
        t.add_string(s.as_bytes())
            .map_err(|_| LanguageError::new("out of memory filling the node table"))
    }
    fn opt_text(t: &mut Table, s: &Option<String>) -> Result<table::Text, LanguageError> {
        match s {
            Some(s) => text(t, s),
            None => Ok(table::Text::ABSENT),
        }
    }
    // Every row's text first, then the rows in one call.
    let mut rows = Vec::with_capacity(table.rows.len());
    for r in &table.rows {
        let (start, end) = span(r.span);
        let (kind, row_text) = match (r.ext_kind, r.kind) {
            (Some(k), _) => (RowKind::EXT + k.to_c() as u8, opt_text(t, &r.text)?),
            (None, NodeKind::Bool) => match r.text.as_deref() {
                Some("true") => (RowKind::TRUE, table::Text::ABSENT),
                Some("false") => (RowKind::FALSE, table::Text::ABSENT),
                _ => return Err(LanguageError::new("a bool row's text is not true or false")),
            },
            (None, k) => (k.to_row(), opt_text(t, &r.text)?),
        };
        rows.push(table::Row {
            kind,
            flags: if r.open { table::ROW_OPEN } else { 0 },
            parent: r.parent.unwrap_or(table::NONE),
            start,
            end,
            text: row_text,
            first_child: table::NONE,
            next_sibling: table::NONE,
        });
    }
    if !rows.is_empty() {
        let dest = t.add_rows(rows.len()).map_err(|_| oom())?;
        dest.copy_from_slice(&rows);
    }
    let row_count = t.rows.len() as u32;
    let on_row = |node: u32| {
        if node < row_count {
            Ok(())
        } else {
            Err(refused())
        }
    };
    for (i, r) in table.rows.iter().enumerate() {
        let node = i as u32;
        if let Some(name) = &r.anchor {
            let (start, end) = span(r.anchor_span);
            let name = text(t, name)?;
            on_row(node)?;
            t.anchors.push(table::Anchor {
                node,
                start,
                end,
                name,
            });
        }
        if let Some(tag) = &r.tag {
            let (start, end) = span(r.tag_span);
            let tag = text(t, tag)?;
            on_row(node)?;
            t.tags.push(table::Tag {
                node,
                start,
                end,
                text: tag,
                resolved: table::TAG_UNSTATED,
                agrees: 1,
            });
        }
        if let Some(m) = r.marker {
            on_row(node)?;
            t.markers.push(table::SpanFact {
                node,
                start: m.start as u32,
                end: m.end as u32,
            });
        }
        if let Some(m) = r.sep {
            on_row(node)?;
            t.seps.push(table::SpanFact {
                node,
                start: m.start as u32,
                end: m.end as u32,
            });
        }
        if let Some(is) = &r.item_sep {
            let is = text(t, is)?;
            on_row(node)?;
            t.item_seps.push(table::ItemSep { node, text: is });
        }
    }
    for r in &table.regions {
        on_row(r.node)?;
        t.regions.push(table::SpanFact {
            node: r.node,
            start: r.span.start as u32,
            end: r.span.end as u32,
        });
    }
    for m in &table.mentions {
        let kind = match m.kind {
            MentionKind::Header => table::MENTION_HEADER,
            MentionKind::Entry => table::MENTION_ENTRY,
            MentionKind::Index => table::MENTION_INDEX,
            MentionKind::Append => table::MENTION_APPEND,
        };
        on_row(m.node)?;
        t.mentions.push(table::Mention {
            node: m.node,
            start: m.span.start as u32,
            end: m.span.end as u32,
            kind,
        });
    }
    for c in &table.comments {
        let slot = match c.slot {
            CommentSlot::Leading => table::COMMENT_LEADING,
            CommentSlot::Trailing => table::COMMENT_TRAILING,
            CommentSlot::Dangling => table::COMMENT_DANGLING,
        };
        let style = match c.style {
            CommentForm::Line => table::COMMENT_LINE,
            CommentForm::Block => table::COMMENT_BLOCK,
        };
        let body = text(t, &c.text)?;
        on_row(c.node)?;
        t.comments.push(table::Comment {
            node: c.node,
            slot,
            style,
            text: body,
        });
    }
    for d in &table.directives {
        let handle = text(t, &d.handle)?;
        let prefix = text(t, &d.prefix)?;
        t.directives.push(table::Directive { handle, prefix });
    }
    for d in &table.diagnostics {
        let severity = match d.severity {
            Severity::Error => table::DIAGNOSTIC_ERROR,
            Severity::Warning => table::DIAGNOSTIC_WARNING,
        };
        let message = text(t, &d.message)?;
        let label = text(t, &d.label)?;
        t.diagnostics.push(table::Diagnostic {
            severity,
            start: d.span.start as u32,
            end: d.span.end as u32,
            message,
            label,
        });
    }
    Ok(())
}

/// A Rust table from the view the core hands `print`.
pub(crate) fn table_from_view(v: &table::View<'_>) -> Result<NodeTable, LanguageError> {
    let text_of = |t: table::Text| -> Result<Option<String>, LanguageError> {
        if t.is_absent() {
            return Ok(None);
        }
        let bytes = v
            .text_of(t)
            .ok_or_else(|| LanguageError::new("a table text is out of range"))?;
        String::from_utf8(bytes.to_vec())
            .map(Some)
            .map_err(|_| LanguageError::new("a table string is not UTF-8"))
    };
    let span_of = |start: u32, end: u32| -> Option<Span> {
        if start == table::NONE {
            None
        } else {
            Some(Span {
                start: start as usize,
                end: end as usize,
            })
        }
    };
    let mut out = NodeTable::new();
    for r in v.rows {
        let (kind, ext_kind, text) = if r.kind >= RowKind::EXT {
            let k = ExtKind::from_c((r.kind - RowKind::EXT) as c_int)
                .ok_or_else(|| LanguageError::new("unknown extended kind"))?;
            let kind = if k == ExtKind::CharLiteral {
                NodeKind::Int
            } else {
                NodeKind::String
            };
            (kind, Some(k), text_of(r.text)?)
        } else {
            match r.kind {
                RowKind::FALSE => (NodeKind::Bool, None, Some("false".to_owned())),
                RowKind::TRUE => (NodeKind::Bool, None, Some("true".to_owned())),
                k => (
                    NodeKind::from_row(k).ok_or_else(|| LanguageError::new("unknown node kind"))?,
                    None,
                    text_of(r.text)?,
                ),
            }
        };
        out.rows.push(NodeRow {
            kind,
            ext_kind,
            parent: if r.parent == table::NONE {
                None
            } else {
                Some(r.parent)
            },
            span: span_of(r.start, r.end),
            text,
            anchor: None,
            anchor_span: None,
            tag: None,
            tag_span: None,
            marker: None,
            sep: None,
            item_sep: None,
            open: r.flags & table::ROW_OPEN != 0,
        });
    }
    let row = |out: &NodeTable, node: u32| -> Result<usize, LanguageError> {
        let i = node as usize;
        if i < out.rows.len() {
            Ok(i)
        } else {
            Err(LanguageError::new("a side fact names no row"))
        }
    };
    for a in v.anchors {
        let i = row(&out, a.node)?;
        out.rows[i].anchor = text_of(a.name)?;
        out.rows[i].anchor_span = span_of(a.start, a.end);
    }
    for t in v.tags {
        let i = row(&out, t.node)?;
        out.rows[i].tag = text_of(t.text)?;
        out.rows[i].tag_span = span_of(t.start, t.end);
    }
    for m in v.markers {
        let i = row(&out, m.node)?;
        out.rows[i].marker = span_of(m.start, m.end);
    }
    for m in v.seps {
        let i = row(&out, m.node)?;
        out.rows[i].sep = span_of(m.start, m.end);
    }
    for m in v.item_seps {
        let i = row(&out, m.node)?;
        out.rows[i].item_sep = text_of(m.text)?;
    }
    for r in v.regions {
        out.regions.push(RegionRow {
            node: r.node,
            span: Span {
                start: r.start as usize,
                end: r.end as usize,
            },
        });
    }
    for m in v.mentions {
        out.mentions.push(MentionRow {
            node: m.node,
            span: span_of(m.start, m.end)
                .ok_or_else(|| LanguageError::new("a mention has no span"))?,
            kind: match m.kind {
                table::MENTION_ENTRY => MentionKind::Entry,
                table::MENTION_INDEX => MentionKind::Index,
                table::MENTION_APPEND => MentionKind::Append,
                _ => MentionKind::Header,
            },
        });
    }
    for c in v.comments {
        out.comments.push(CommentRow {
            node: c.node,
            slot: match c.slot {
                table::COMMENT_TRAILING => CommentSlot::Trailing,
                table::COMMENT_DANGLING => CommentSlot::Dangling,
                _ => CommentSlot::Leading,
            },
            style: if c.style == table::COMMENT_BLOCK {
                CommentForm::Block
            } else {
                CommentForm::Line
            },
            text: text_of(c.text)?.unwrap_or_default(),
        });
    }
    for d in v.directives {
        out.directives.push(DirectiveRow {
            handle: text_of(d.handle)?
                .ok_or_else(|| LanguageError::new("a directive has no handle"))?,
            prefix: text_of(d.prefix)?
                .ok_or_else(|| LanguageError::new("a directive has no prefix"))?,
        });
    }
    Ok(out)
}
