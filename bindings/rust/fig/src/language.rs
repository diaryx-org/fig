//! Languages resolved at runtime: the [`Language`] trait, the node table its
//! `parse` returns, and [`register`], which carries an implementation to the
//! core as a vtable and hands back a [`Format`] that every entry point of
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
//! integer is per process; persist the name and resolve it with
//! [`Format::by_name`].

use std::ffi::{CString, c_void};
use std::os::raw::{c_char, c_int};
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::ffi;
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
    pub plist_data: bool,
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
            NodeKind::Null => ffi::FIG_ROW_NULL,
            NodeKind::Bool => ffi::FIG_ROW_FALSE,
            NodeKind::Int => ffi::FIG_ROW_INT,
            NodeKind::Float => ffi::FIG_ROW_FLOAT,
            NodeKind::String => ffi::FIG_ROW_STRING,
            NodeKind::Sequence => ffi::FIG_ROW_SEQUENCE,
            NodeKind::Mapping => ffi::FIG_ROW_MAPPING,
            NodeKind::KeyValue => ffi::FIG_ROW_KEYVALUE,
            NodeKind::Alias => ffi::FIG_ROW_ALIAS,
        }
    }

    pub(crate) fn from_row(v: u8) -> Option<Self> {
        Some(match v {
            ffi::FIG_ROW_NULL => NodeKind::Null,
            ffi::FIG_ROW_FALSE | ffi::FIG_ROW_TRUE => NodeKind::Bool,
            ffi::FIG_ROW_INT => NodeKind::Int,
            ffi::FIG_ROW_FLOAT => NodeKind::Float,
            ffi::FIG_ROW_STRING => NodeKind::String,
            ffi::FIG_ROW_SEQUENCE => NodeKind::Sequence,
            ffi::FIG_ROW_MAPPING => NodeKind::Mapping,
            ffi::FIG_ROW_KEYVALUE => NodeKind::KeyValue,
            ffi::FIG_ROW_ALIAS => NodeKind::Alias,
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

/// One place a section node's name is written.
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
/// `old_key` for `Key`.
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
/// own. A dialect named after a format compiled out of the linked library
/// stands in for it: it comes back as that format's own variant
/// ([`Format::Yaml`] for a dialect named `yaml`), and every API taking that
/// variant reaches it. Refused, with the reason in [`Error::Language`], when the
/// description breaks a rule the core holds its own formats to, when a
/// sample fails to parse, print, reparse to the same tree or take a no-op
/// edit, or when the name is taken.
pub fn register(lang: impl Language) -> Result<Vec<Format>, Error> {
    let desc = lang.describe();
    let dialect_names: Vec<String> = desc.dialects.iter().map(|d| d.name.clone()).collect();
    let reg = Registration::new(Box::new(lang), desc)?;
    // The registration lives for the process: the core keeps `ctx` and the
    // function pointers, and the strings the vtable points to are copied
    // at registration but `ctx` is read on every call.
    let reg: &'static Registration = Box::leak(Box::new(reg));
    let vt = reg.vtable();
    let mut format: c_int = -1;
    let mut err = ffi::FigError::new();
    let status = unsafe { ffi::fig_language_register(&vt, &mut format, &mut err) };
    if status != ffi::FigStatus::OK {
        let len = err.message_len.min(err.message.len());
        let message = String::from_utf8_lossy(&err.message[..len]).into_owned();
        if message.is_empty() {
            Error::from_status(status)?;
            return Err(Error::Internal);
        }
        return Err(Error::Language(LanguageFailure {
            message,
            byte_offset: (err.byte_offset != 0).then_some(err.byte_offset),
        }));
    }
    // By name, not by counting up from `format`: a dialect named after a
    // format compiled out of the linked library stands in for it and takes
    // that format's integer, so the rows' integers need not be consecutive.
    dialect_names
        .iter()
        .map(|name| Format::by_name(name).ok_or(Error::Internal))
        .collect()
}

/// Everything the vtable points at, owned for the life of the process, plus
/// the language itself.
struct Registration {
    lang: Box<dyn Language>,
    name: CString,
    caps: u32,
    max_mapping_depth: c_int,
    lossless: u32,
    syntax: Option<Box<CSyntax>>,
    dialects: Vec<ffi::FigDialectDesc>,
    /// What each dialect's pointers reach.
    _dialect_owned: Vec<CDialect>,
    samples: Vec<Vec<u8>>,
    sample_strs: Vec<ffi::FigStr>,
    renderers: Renderers,
    streams: bool,
}

/// A `FigSyntax` and the strings it points into.
struct CSyntax {
    c: ffi::FigSyntax,
    _strings: Vec<CString>,
}

struct CDialect {
    _name: CString,
    _extensions: Vec<CString>,
    _extension_ptrs: Vec<*const c_char>,
    _seed: Option<CString>,
    _syntax: Option<Box<CSyntax>>,
    _char_as: Option<CString>,
    _number_special_as: Option<CString>,
}

/// `kinds` as `FigLanguageVTable.lossless`'s bits: the envelope bit, then
/// one bit per kind held natively.
fn native_bits(l: &NativeKinds) -> u32 {
    let ext = [
        (l.offset_datetime, ExtKind::OffsetDateTime),
        (l.local_datetime, ExtKind::LocalDateTime),
        (l.local_date, ExtKind::LocalDate),
        (l.local_time, ExtKind::LocalTime),
        (l.enum_literal, ExtKind::EnumLiteral),
        (l.char_literal, ExtKind::CharLiteral),
        (l.number_special, ExtKind::NumberSpecial),
        (l.plist_date, ExtKind::PlistDate),
        (l.plist_data, ExtKind::PlistData),
    ];
    let mut bits = ffi::FIG_LOSSLESS_ENVELOPE;
    if l.null {
        bits |= ffi::FIG_NATIVE_NULL;
    }
    for (held, kind) in ext {
        if held {
            bits |= ffi::fig_native_ext(kind.to_c() as u32);
        }
    }
    bits
}

unsafe impl Send for Registration {}
unsafe impl Sync for Registration {}

fn cstr(s: &str) -> Result<CString, Error> {
    CString::new(s).map_err(|e| {
        Error::Language(LanguageFailure {
            message: format!("a declared string contains a NUL byte: {s:?}"),
            byte_offset: Some(e.nul_position()),
        })
    })
}

fn opt_ptr(strings: &mut Vec<CString>, s: Option<&str>) -> Result<*const c_char, Error> {
    match s {
        Some(s) => {
            let c = cstr(s)?;
            let p = c.as_ptr();
            strings.push(c);
            Ok(p)
        }
        None => Ok(std::ptr::null()),
    }
}

impl CSyntax {
    fn new(s: &Syntax) -> Result<Box<Self>, Error> {
        let mut strings = Vec::new();
        let delim = |strings: &mut Vec<CString>,
                     d: Option<&CommentDelimiter>|
         -> Result<ffi::FigCommentDelimiter, Error> {
            Ok(match d {
                Some(d) => ffi::FigCommentDelimiter {
                    open: opt_ptr(strings, Some(&d.open))?,
                    close: opt_ptr(strings, Some(&d.close))?,
                    forbidden: opt_ptr(strings, d.forbidden.as_deref())?,
                },
                None => ffi::FigCommentDelimiter {
                    open: std::ptr::null(),
                    close: std::ptr::null(),
                    forbidden: std::ptr::null(),
                },
            })
        };
        let c = ffi::FigSyntax {
            size: std::mem::size_of::<ffi::FigSyntax>() as u32,
            comments: ffi::FigComments {
                line: delim(&mut strings, s.comments.line.as_ref())?,
                trailing: delim(&mut strings, s.comments.trailing.as_ref())?,
                block: delim(&mut strings, s.comments.block.as_ref())?,
            },
            kv_sep: opt_ptr(&mut strings, s.kv_sep.as_deref())?,
            flow_kv_sep_from_siblings: s.flow_kv_sep_from_siblings,
            flow_map_pad: opt_ptr(&mut strings, Some(&s.flow_map_pad))?,
            key_sigil: s.key_sigil.unwrap_or(0),
            empty_map_literal: opt_ptr(&mut strings, s.empty_map_literal.as_deref())?,
            block_seq_editable: s.block_seq_editable,
            flow_containers: s.flow_containers,
            indent_unit: opt_ptr(&mut strings, Some(&s.indent_unit))?,
            seq_item_marker: opt_ptr(&mut strings, Some(&s.seq_item_marker))?,
            closed_containers: match &s.closed_containers {
                Some(c) => ffi::FigClosedContainers {
                    map_open: opt_ptr(&mut strings, Some(&c.map_open))?,
                    map_close: opt_ptr(&mut strings, Some(&c.map_close))?,
                    seq_open: opt_ptr(&mut strings, Some(&c.seq_open))?,
                    seq_close: opt_ptr(&mut strings, Some(&c.seq_close))?,
                },
                None => ffi::FigClosedContainers {
                    map_open: std::ptr::null(),
                    map_close: std::ptr::null(),
                    seq_open: std::ptr::null(),
                    seq_close: std::ptr::null(),
                },
            },
            single_line_block_mapping: s.single_line_block_mapping,
            bare_document_mapping: s.bare_document_mapping,
            flow_map_open: opt_ptr(&mut strings, Some(&s.flow_map_open))?,
            flow_map_close: opt_ptr(&mut strings, Some(&s.flow_map_close))?,
            structural_indent: s.structural_indent,
            section_noun: opt_ptr(&mut strings, s.section_noun.as_deref())?,
            section_header: match &s.section_header {
                Some(h) => ffi::FigSectionHeader {
                    open: opt_ptr(&mut strings, Some(&h.open))?,
                    close: opt_ptr(&mut strings, Some(&h.close))?,
                    seq_open: opt_ptr(&mut strings, h.seq_open.as_deref())?,
                    seq_close: opt_ptr(&mut strings, h.seq_close.as_deref())?,
                    sep: opt_ptr(&mut strings, Some(&h.sep))?,
                    skip_index: h.skip_index,
                },
                None => ffi::FigSectionHeader {
                    open: std::ptr::null(),
                    close: std::ptr::null(),
                    seq_open: std::ptr::null(),
                    seq_close: std::ptr::null(),
                    sep: std::ptr::null(),
                    skip_index: true,
                },
            },
            merge_key: opt_ptr(&mut strings, s.merge_key.as_deref())?,
            flow_entry_sep: opt_ptr(&mut strings, Some(&s.flow_entry_sep))?,
            flow_root: s.flow_root,
            flow_maps_only: s.flow_maps_only,
            line_prefix_marks: opt_ptr(&mut strings, Some(&s.line_prefix_marks))?,
        };
        Ok(Box::new(CSyntax {
            c,
            _strings: strings,
        }))
    }
}

impl Registration {
    fn new(lang: Box<dyn Language>, desc: Description) -> Result<Self, Error> {
        let name = cstr(&desc.name)?;
        let mut caps = 0u32;
        if desc.caps.read {
            caps |= 1 << 0;
        }
        if desc.caps.edit {
            caps |= 1 << 1;
        }
        if desc.caps.serialize {
            caps |= 1 << 2;
        }
        if desc.caps.references {
            caps |= 1 << 3;
        }
        // The envelope bit, then one bit per kind held natively.
        let lossless = desc.lossless.map_or(0, |l| native_bits(&l));
        let syntax = match &desc.syntax {
            Some(s) => Some(CSyntax::new(s)?),
            None => None,
        };
        let mut dialects = Vec::with_capacity(desc.dialects.len());
        let mut owned = Vec::with_capacity(desc.dialects.len());
        for d in &desc.dialects {
            let dname = cstr(&d.name)?;
            let extensions: Vec<CString> = d
                .extensions
                .iter()
                .map(|e| cstr(e))
                .collect::<Result<_, _>>()?;
            let mut extension_ptrs: Vec<*const c_char> =
                extensions.iter().map(|e| e.as_ptr()).collect();
            extension_ptrs.push(std::ptr::null());
            let seed = match &d.empty_doc_seed {
                Some(s) => Some(cstr(s)?),
                None => None,
            };
            let dsyntax = match &d.syntax {
                Some(s) => Some(CSyntax::new(s)?),
                None => None,
            };
            let char_as = match &d.prints {
                Some(p) => Some(cstr(&p.char_as)?),
                None => None,
            };
            let number_special_as = match &d.prints {
                Some(p) => Some(cstr(&p.number_special_as)?),
                None => None,
            };
            let mut prints_flags = 0u32;
            if let Some(p) = &d.prints {
                if !p.typed_scalars {
                    prints_flags |= ffi::FIG_PRINTS_TEXT_SCALARS;
                }
                if p.null_at_root {
                    prints_flags |= ffi::FIG_PRINTS_NULL_AT_ROOT;
                }
                if p.root_container {
                    prints_flags |= ffi::FIG_PRINTS_ROOT_CONTAINER;
                }
            }
            dialects.push(ffi::FigDialectDesc {
                name: dname.as_ptr(),
                extensions: extension_ptrs.as_ptr(),
                splice: match d.splice {
                    Splice::Literal => 0,
                    Splice::Escaped => 1,
                    Splice::Raw => 2,
                },
                empty_doc_seed: seed.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                syntax: dsyntax
                    .as_ref()
                    .map_or(std::ptr::null(), |s| &s.c as *const _),
                prints_holds: d.prints.as_ref().map_or(0, |p| native_bits(&p.holds)),
                prints_flags,
                prints_char_as: char_as.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                prints_number_special_as: number_special_as
                    .as_ref()
                    .map_or(std::ptr::null(), |s| s.as_ptr()),
                prints_comments: d.prints.as_ref().map_or(0, |p| match p.comments {
                    CommentsPrinted::Always => 0,
                    CommentsPrinted::Pretty => 1,
                    CommentsPrinted::Never => 2,
                }),
            });
            owned.push(CDialect {
                _name: dname,
                _extensions: extensions,
                _extension_ptrs: extension_ptrs,
                _seed: seed,
                _syntax: dsyntax,
                _char_as: char_as,
                _number_special_as: number_special_as,
            });
        }
        let samples: Vec<Vec<u8>> = desc.samples.iter().map(|s| s.as_bytes().to_vec()).collect();
        let sample_strs = samples
            .iter()
            .map(|s| ffi::FigStr {
                ptr: s.as_ptr(),
                len: s.len(),
            })
            .collect();
        Ok(Registration {
            lang,
            name,
            caps,
            max_mapping_depth: desc
                .max_mapping_depth
                .map_or(ffi::FIG_DEPTH_NONE, c_int::from),
            lossless,
            syntax,
            dialects,
            _dialect_owned: owned,
            samples,
            sample_strs,
            renderers: desc.renderers,
            streams: desc.streams,
        })
    }

    fn vtable(&'static self) -> ffi::FigLanguageVTable {
        ffi::FigLanguageVTable {
            version: ffi::FIG_LANGUAGE_VTABLE_VERSION,
            ctx: self as *const Registration as *mut c_void,
            name: self.name.as_ptr(),
            caps: self.caps,
            max_mapping_depth: self.max_mapping_depth,
            size: std::mem::size_of::<ffi::FigLanguageVTable>() as u32,
            lossless: self.lossless,
            syntax: self
                .syntax
                .as_ref()
                .map_or(std::ptr::null(), |s| &s.c as *const _),
            dialects: self.dialects.as_ptr(),
            dialect_count: self.dialects.len(),
            dialect_size: std::mem::size_of::<ffi::FigDialectDesc>(),
            samples: self.sample_strs.as_ptr(),
            sample_count: self.samples.len(),
            parse: parse_thunk,
            print: if self.caps & (1 << 2) != 0 {
                Some(print_thunk)
            } else {
                None
            },
            free_bytes: free_bytes_thunk,
            render_value: if self.renderers.value {
                Some(render_value_thunk)
            } else {
                None
            },
            render_entry: if self.renderers.entry {
                Some(render_entry_thunk)
            } else {
                None
            },
            render_item: if self.renderers.item {
                Some(render_item_thunk)
            } else {
                None
            },
            render_tail: if self.renderers.tail {
                Some(render_tail_thunk)
            } else {
                None
            },
            render_key: if self.renderers.key {
                Some(render_key_thunk)
            } else {
                None
            },
            render_key_name: if self.renderers.key_name {
                Some(render_key_name_thunk)
            } else {
                None
            },
            split_stream: if self.streams {
                Some(split_stream_thunk)
            } else {
                None
            },
        }
    }
}

// ── the thunks ─────────────────────────────────────────────────────────────
//
// Each is the C signature over one `Language` method. A panic in the
// language is caught here — unwinding across the C boundary is undefined —
// and reported as a failure with the panic's message.

fn fill_err(err: *mut ffi::FigError, e: &LanguageError) {
    if err.is_null() {
        return;
    }
    let err = unsafe { &mut *err };
    let bytes = e.message.as_bytes();
    let n = bytes.len().min(err.message.len() - 1);
    err.message[..n].copy_from_slice(&bytes[..n]);
    err.message[n] = 0;
    err.message_len = n;
    err.byte_offset = e.byte_offset.unwrap_or(0);
}

fn panic_message(p: Box<dyn std::any::Any + Send>) -> LanguageError {
    let msg = p
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_owned());
    LanguageError::new(format!("the language panicked: {msg}"))
}

/// The `T` fig wrote at `p`, which leads with its `size` as a `u32`: the
/// prefix this crate's `T` shares with fig's, and `fill` past it. A fig
/// built against another header than this crate's writes a shorter or a
/// longer record, and neither is read past what both know.
unsafe fn read_gated<T: Copy>(p: *const T, fill: T) -> T {
    let size = unsafe { (p as *const u32).read_unaligned() } as usize;
    unsafe { read_prefix(p, size, fill) }
}

/// The first `size` bytes at `p` over `fill`, as far as a `T` reaches.
unsafe fn read_prefix<T: Copy>(p: *const T, size: usize, fill: T) -> T {
    let mut out = fill;
    let n = size.min(std::mem::size_of::<T>());
    unsafe { std::ptr::copy_nonoverlapping(p as *const u8, &mut out as *mut T as *mut u8, n) };
    out
}

const DEFAULT_PRINT_OPTIONS: ffi::FigPrintOptions = ffi::FigPrintOptions {
    size: 0,
    pretty: true,
    strip_comments: false,
    indent: 2,
    width: 80,
    splice: false,
    flow: false,
};

const EMPTY_STR: ffi::FigStr = ffi::FigStr {
    ptr: std::ptr::null(),
    len: 0,
};

const EMPTY_REQUEST: ffi::FigRenderRequest = ffi::FigRenderRequest {
    size: 0,
    dialect: c"".as_ptr(),
    indent: EMPTY_STR,
    key: EMPTY_STR,
    value: EMPTY_STR,
    literal: c"string".as_ptr(),
    old_key: EMPTY_STR,
    parent_key: EMPTY_STR,
    parent_tag: EMPTY_STR,
};

unsafe fn reg_of<'a>(ctx: *mut c_void) -> &'a Registration {
    unsafe { &*(ctx as *const Registration) }
}

unsafe fn dialect_of<'a>(dialect: *const c_char) -> &'a str {
    unsafe { std::ffi::CStr::from_ptr(dialect) }
        .to_str()
        .unwrap_or("")
}

fn bytes_of<'a>(s: ffi::FigStr) -> &'a [u8] {
    if s.len == ffi::FIG_LEN_NONE || s.len == 0 || s.ptr.is_null() {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(s.ptr, s.len) }
}

/// `table` written into fig's table `t` through the `fig_table_*` calls. Every
/// text is copied into the table's buffer.
fn fill_table(t: *mut ffi::FigTable, table: &NodeTable) -> Result<(), LanguageError> {
    let oom = || LanguageError::new("out of memory filling the node table");
    let check = |status: ffi::FigStatus| -> Result<(), LanguageError> {
        if status == ffi::FigStatus::OK {
            Ok(())
        } else {
            Err(LanguageError::new("fig refused a node table fact"))
        }
    };
    let text = |s: &str| -> Result<ffi::FigText, LanguageError> {
        let mut out = ffi::FigText::NONE;
        let status = unsafe { ffi::fig_table_string(t, s.as_ptr(), s.len(), &mut out) };
        if status == ffi::FigStatus::OK {
            Ok(out)
        } else {
            Err(oom())
        }
    };
    let opt_text = |s: &Option<String>| -> Result<ffi::FigText, LanguageError> {
        match s {
            Some(s) => text(s),
            None => Ok(ffi::FigText::NONE),
        }
    };
    let span = |s: Option<Span>| -> (u32, u32) {
        match s {
            Some(s) => (s.start as u32, s.end as u32),
            None => (ffi::FIG_ROW_NONE, ffi::FIG_ROW_NONE),
        }
    };
    // Every row's text first, then the rows in one call: a row pointer fig
    // hands back is valid only until the next call on the table.
    let mut rows = Vec::with_capacity(table.rows.len());
    for r in &table.rows {
        let (start, end) = span(r.span);
        let (kind, row_text) = match (r.ext_kind, r.kind) {
            (Some(k), _) => (ffi::FIG_ROW_EXT + k.to_c() as u8, opt_text(&r.text)?),
            (None, NodeKind::Bool) => match r.text.as_deref() {
                Some("true") => (ffi::FIG_ROW_TRUE, ffi::FigText::NONE),
                Some("false") => (ffi::FIG_ROW_FALSE, ffi::FigText::NONE),
                _ => return Err(LanguageError::new("a bool row's text is not true or false")),
            },
            (None, k) => (k.to_row(), opt_text(&r.text)?),
        };
        rows.push(ffi::FigRow {
            kind,
            flags: if r.open { ffi::FIG_ROW_OPEN } else { 0 },
            reserved: 0,
            parent: r.parent.unwrap_or(ffi::FIG_ROW_NONE),
            start,
            end,
            text: row_text,
            first_child: ffi::FIG_ROW_NONE,
            next_sibling: ffi::FIG_ROW_NONE,
        });
    }
    if !rows.is_empty() {
        let dest = unsafe { ffi::fig_table_rows(t, rows.len()) };
        if dest.is_null() {
            return Err(oom());
        }
        unsafe { std::ptr::copy_nonoverlapping(rows.as_ptr(), dest, rows.len()) };
    }
    for (i, r) in table.rows.iter().enumerate() {
        let node = i as u32;
        if let Some(name) = &r.anchor {
            let (s, e) = span(r.anchor_span);
            let name = text(name)?;
            check(unsafe { ffi::fig_table_anchor(t, node, s, e, name) })?;
        }
        if let Some(tag) = &r.tag {
            let (s, e) = span(r.tag_span);
            let tag = text(tag)?;
            check(unsafe { ffi::fig_table_tag(t, node, s, e, tag, ffi::FIG_TAG_UNSTATED, true) })?;
        }
        if let Some(m) = r.marker {
            check(unsafe { ffi::fig_table_marker(t, node, m.start as u32, m.end as u32) })?;
        }
        if let Some(m) = r.sep {
            check(unsafe { ffi::fig_table_sep(t, node, m.start as u32, m.end as u32) })?;
        }
        if let Some(is) = &r.item_sep {
            let is = text(is)?;
            check(unsafe { ffi::fig_table_item_sep(t, node, is) })?;
        }
    }
    for r in &table.regions {
        check(unsafe { ffi::fig_table_region(t, r.node, r.span.start as u32, r.span.end as u32) })?;
    }
    for m in &table.mentions {
        let kind = match m.kind {
            MentionKind::Header => ffi::FIG_MENTION_HEADER,
            MentionKind::Entry => ffi::FIG_MENTION_ENTRY,
        };
        check(unsafe {
            ffi::fig_table_mention(t, m.node, m.span.start as u32, m.span.end as u32, kind)
        })?;
    }
    for c in &table.comments {
        let slot = match c.slot {
            CommentSlot::Leading => ffi::FIG_COMMENT_LEADING,
            CommentSlot::Trailing => ffi::FIG_COMMENT_TRAILING,
            CommentSlot::Dangling => ffi::FIG_COMMENT_DANGLING,
        };
        let style = match c.style {
            CommentForm::Line => ffi::FIG_COMMENT_LINE,
            CommentForm::Block => ffi::FIG_COMMENT_BLOCK,
        };
        let body = text(&c.text)?;
        check(unsafe { ffi::fig_table_comment(t, c.node, slot, style, body) })?;
    }
    for d in &table.directives {
        let handle = text(&d.handle)?;
        let prefix = text(&d.prefix)?;
        check(unsafe { ffi::fig_table_directive(t, handle, prefix) })?;
    }
    Ok(())
}

/// A slice of a view's list.
fn view_slice<'a, T>(ptr: *const T, count: usize) -> &'a [T] {
    if ptr.is_null() || count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(ptr, count) }
    }
}

/// A Rust table from the view fig hands `print`.
pub(crate) fn table_from_c(v: &ffi::FigTableView) -> Result<NodeTable, LanguageError> {
    let source = view_slice(v.source, v.source_len);
    let strings = view_slice(v.strings, v.strings_len);
    let text_of = |t: ffi::FigText| -> Result<Option<String>, LanguageError> {
        if t.len == ffi::FIG_ROW_NONE {
            return Ok(None);
        }
        let (base, len) = if t.len & ffi::FIG_TEXT_BUFFER != 0 {
            (strings, (t.len & !ffi::FIG_TEXT_BUFFER) as usize)
        } else {
            (source, t.len as usize)
        };
        let bytes = base
            .get(t.off as usize..t.off as usize + len)
            .ok_or_else(|| LanguageError::new("a table text is out of range"))?;
        String::from_utf8(bytes.to_vec())
            .map(Some)
            .map_err(|_| LanguageError::new("a table string is not UTF-8"))
    };
    let span_of = |start: u32, end: u32| -> Option<Span> {
        if start == ffi::FIG_ROW_NONE {
            None
        } else {
            Some(Span {
                start: start as usize,
                end: end as usize,
            })
        }
    };
    let mut out = NodeTable::new();
    for r in view_slice(v.rows, v.row_count) {
        let (kind, ext_kind, text) = if r.kind >= ffi::FIG_ROW_EXT {
            let k = ExtKind::from_c((r.kind - ffi::FIG_ROW_EXT) as c_int)
                .ok_or_else(|| LanguageError::new("unknown extended kind"))?;
            let kind = if k == ExtKind::CharLiteral {
                NodeKind::Int
            } else {
                NodeKind::String
            };
            (kind, Some(k), text_of(r.text)?)
        } else {
            match r.kind {
                ffi::FIG_ROW_FALSE => (NodeKind::Bool, None, Some("false".to_owned())),
                ffi::FIG_ROW_TRUE => (NodeKind::Bool, None, Some("true".to_owned())),
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
            parent: if r.parent == ffi::FIG_ROW_NONE {
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
            open: r.flags & ffi::FIG_ROW_OPEN != 0,
        });
    }
    let row = |out: &mut NodeTable, node: u32| -> Result<usize, LanguageError> {
        let i = node as usize;
        if i < out.rows.len() {
            Ok(i)
        } else {
            Err(LanguageError::new("a side fact names no row"))
        }
    };
    for a in view_slice(v.anchors, v.anchor_count) {
        let i = row(&mut out, a.node)?;
        out.rows[i].anchor = text_of(a.name)?;
        out.rows[i].anchor_span = span_of(a.start, a.end);
    }
    for t in view_slice(v.tags, v.tag_count) {
        let i = row(&mut out, t.node)?;
        out.rows[i].tag = text_of(t.text)?;
        out.rows[i].tag_span = span_of(t.start, t.end);
    }
    for m in view_slice(v.markers, v.marker_count) {
        let i = row(&mut out, m.node)?;
        out.rows[i].marker = span_of(m.start, m.end);
    }
    for m in view_slice(v.seps, v.sep_count) {
        let i = row(&mut out, m.node)?;
        out.rows[i].sep = span_of(m.start, m.end);
    }
    for m in view_slice(v.item_seps, v.item_sep_count) {
        let i = row(&mut out, m.node)?;
        out.rows[i].item_sep = text_of(m.text)?;
    }
    for r in view_slice(v.regions, v.region_count) {
        out.regions.push(RegionRow {
            node: r.node,
            span: Span {
                start: r.start as usize,
                end: r.end as usize,
            },
        });
    }
    for m in view_slice(v.mentions, v.mention_count) {
        out.mentions.push(MentionRow {
            node: m.node,
            span: span_of(m.start, m.end)
                .ok_or_else(|| LanguageError::new("a mention has no span"))?,
            kind: if m.kind == ffi::FIG_MENTION_ENTRY {
                MentionKind::Entry
            } else {
                MentionKind::Header
            },
        });
    }
    for c in view_slice(v.comments, v.comment_count) {
        out.comments.push(CommentRow {
            node: c.node,
            slot: match c.slot {
                ffi::FIG_COMMENT_TRAILING => CommentSlot::Trailing,
                ffi::FIG_COMMENT_DANGLING => CommentSlot::Dangling,
                _ => CommentSlot::Leading,
            },
            style: if c.style == ffi::FIG_COMMENT_BLOCK {
                CommentForm::Block
            } else {
                CommentForm::Line
            },
            text: text_of(c.text)?.unwrap_or_default(),
        });
    }
    for d in view_slice(v.directives, v.directive_count) {
        out.directives.push(DirectiveRow {
            handle: text_of(d.handle)?
                .ok_or_else(|| LanguageError::new("a directive has no handle"))?,
            prefix: text_of(d.prefix)?
                .ok_or_else(|| LanguageError::new("a directive has no prefix"))?,
        });
    }
    Ok(out)
}

unsafe extern "C" fn parse_thunk(
    ctx: *mut c_void,
    dialect: *const c_char,
    input: ffi::FigStr,
    out: *mut ffi::FigTable,
    err: *mut ffi::FigError,
) -> c_int {
    let reg = unsafe { reg_of(ctx) };
    let dialect = unsafe { dialect_of(dialect) };
    let input = bytes_of(input);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let table = reg.lang.parse(dialect, input)?;
        fill_table(out, &table)
    }));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            fill_err(err, &e);
            ffi::FigStatus::PARSE_ERROR.0
        }
        Err(p) => {
            fill_err(err, &panic_message(p));
            ffi::FigStatus::INTERNAL_ERROR.0
        }
    }
}

/// Hand `bytes` to the core as a `FigStr` it will return through
/// `free_bytes`: a boxed slice, whose pointer and length are enough to
/// rebuild it.
fn give_bytes(bytes: Vec<u8>, out: *mut ffi::FigStr) {
    let boxed = bytes.into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed) as *const u8;
    unsafe { *out = ffi::FigStr { ptr, len } };
}

unsafe extern "C" fn free_bytes_thunk(_ctx: *mut c_void, bytes: ffi::FigStr) {
    if bytes.ptr.is_null() || bytes.len == ffi::FIG_LEN_NONE {
        return;
    }
    let slice = std::ptr::slice_from_raw_parts_mut(bytes.ptr as *mut u8, bytes.len);
    drop(unsafe { Box::from_raw(slice) });
}

unsafe extern "C" fn print_thunk(
    ctx: *mut c_void,
    dialect: *const c_char,
    table: *const ffi::FigTableView,
    options: *const ffi::FigPrintOptions,
    out: *mut ffi::FigStr,
    err: *mut ffi::FigError,
) -> c_int {
    let reg = unsafe { reg_of(ctx) };
    let dialect = unsafe { dialect_of(dialect) };
    let opts = unsafe { read_gated(options, DEFAULT_PRINT_OPTIONS) };
    let options = PrintOptions {
        pretty: opts.pretty,
        strip_comments: opts.strip_comments,
        indent: opts.indent,
        width: opts.width,
        splice: opts.splice,
        flow: opts.flow,
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let table = table_from_c(unsafe { &*table })?;
        reg.lang.print(dialect, &table, &options)
    }));
    match result {
        Ok(Ok(bytes)) => {
            give_bytes(bytes, out);
            0
        }
        Ok(Err(e)) => {
            fill_err(err, &e);
            ffi::FigStatus::UNSUPPORTED_FORMAT.0
        }
        Err(p) => {
            fill_err(err, &panic_message(p));
            ffi::FigStatus::INTERNAL_ERROR.0
        }
    }
}

fn render_thunk_body(
    ctx: *mut c_void,
    which: Renderer,
    request: *const ffi::FigRenderRequest,
    out: *mut ffi::FigStr,
    err: *mut ffi::FigError,
) -> c_int {
    let reg = unsafe { reg_of(ctx) };
    let r = unsafe { read_gated(request, EMPTY_REQUEST) };
    let args = RenderArgs {
        dialect: unsafe { dialect_of(r.dialect) },
        indent: bytes_of(r.indent),
        key: bytes_of(r.key),
        value: bytes_of(r.value),
        // A name this crate does not know is a core newer than it; the
        // fallback is what a renderer does with any text it cannot type.
        literal: Literal::from_name(unsafe { dialect_of(r.literal) }).unwrap_or_default(),
        old_key: bytes_of(r.old_key),
        parent_key: bytes_of(r.parent_key),
        parent_tag: bytes_of(r.parent_tag),
    };
    match catch_unwind(AssertUnwindSafe(|| reg.lang.render(which, args))) {
        Ok(Ok(bytes)) => {
            give_bytes(bytes, out);
            0
        }
        Ok(Err(e)) => {
            fill_err(err, &e);
            ffi::FigStatus::UNSUPPORTED_OPERATION.0
        }
        Err(p) => {
            fill_err(err, &panic_message(p));
            ffi::FigStatus::INTERNAL_ERROR.0
        }
    }
}

unsafe extern "C" fn split_stream_thunk(
    ctx: *mut c_void,
    dialect: *const c_char,
    input: ffi::FigStr,
    out: *mut ffi::FigStr,
    err: *mut ffi::FigError,
) -> c_int {
    let reg = unsafe { reg_of(ctx) };
    let dialect = unsafe { dialect_of(dialect) };
    let input = bytes_of(input);
    match catch_unwind(AssertUnwindSafe(|| reg.lang.split_stream(dialect, input))) {
        Ok(Ok(docs)) => {
            // The rows as bytes, which `free_bytes` frees as it frees any.
            let mut bytes =
                Vec::with_capacity(docs.len() * std::mem::size_of::<ffi::FigStreamDoc>());
            for d in docs {
                let row = ffi::FigStreamDoc {
                    start: d.start,
                    end: d.end,
                    marked: d.explicit,
                };
                bytes.extend_from_slice(unsafe {
                    std::slice::from_raw_parts(
                        (&row as *const ffi::FigStreamDoc).cast::<u8>(),
                        std::mem::size_of::<ffi::FigStreamDoc>(),
                    )
                });
            }
            give_bytes(bytes, out);
            0
        }
        Ok(Err(e)) => {
            fill_err(err, &e);
            ffi::FigStatus::PARSE_ERROR.0
        }
        Err(p) => {
            fill_err(err, &panic_message(p));
            ffi::FigStatus::INTERNAL_ERROR.0
        }
    }
}

macro_rules! render_thunk {
    ($name:ident, $which:expr) => {
        unsafe extern "C" fn $name(
            ctx: *mut c_void,
            request: *const ffi::FigRenderRequest,
            out: *mut ffi::FigStr,
            err: *mut ffi::FigError,
        ) -> c_int {
            render_thunk_body(ctx, $which, request, out, err)
        }
    };
}

render_thunk!(render_value_thunk, Renderer::Value);
render_thunk!(render_entry_thunk, Renderer::Entry);
render_thunk!(render_item_thunk, Renderer::Item);
render_thunk!(render_tail_thunk, Renderer::Tail);
render_thunk!(render_key_thunk, Renderer::Key);
render_thunk!(render_key_name_thunk, Renderer::KeyName);
