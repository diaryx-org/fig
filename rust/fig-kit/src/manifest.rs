//! Port of kit/src/manifest.zig.
//!
//! The declared half of the `Language` interface — what a format supplies
//! about its own syntax, rather than what the editor infers by testing which
//! language it was instantiated with. A format declares `syntax`, `caps`,
//! `name` and `extensions`, and the generic engine reads them instead of
//! asking what it is. See `docs/proposals/language-interface.md`.
//!
//! This module is the PARAMETER half of that interface — the answers a
//! format can give as a value. The other half is how a format SPELLS a
//! fragment the engine cannot: the renderers (`Renderer`, `RenderRequest`),
//! methods of `language::Language`.
//!
//! It also holds the shape of a format-registry entry, `DialectInfo` (Zig's
//! `Dialect(L)`), since each language declares its own dialects
//! (`Language::dialects`).
//!
//! **Strings (Rust).** Every string here is `&'static`: a compiled language
//! declares them as constants, and a runtime language's live as long as its
//! registration, which in Zig is the process (`Box::leak` them). Text a
//! format splices into a document (markers, separators, literals, samples)
//! is bytes; names a consumer prints (dialect names, `char_as`) are `str`.
//! Zig's comptime-gating helpers `DialectOf`/`defaultDialect` have no
//! counterpart: a dialect is a `Dialect` index, whatever the build.

use super::ast::ExtKind;

/// A dialect of a language: the ordinal of the language's own dialect enum
/// (Zig's per-language `Type`: JSON's `JSON`/`JSONC`/`JSON5` are 0/1/2,
/// YAML's `v1_2_2`/`v1_1` are 0/1). A language maps it internally.
pub type Dialect = u8;

/// How one comment is delimited: the token that opens it and, for a paired
/// syntax, the one that closes it. `#`, `//` and `;` are an `open` alone;
/// plist's `<!-- … -->` is a pair. The engine writes a comment as `open`, a
/// space, the text, and — when `close` is non-empty — a space and `close`;
/// it strips the same shape when reading one back, and it finds a trailing
/// comment by searching for `open`. A pair used to be inexpressible here,
/// which is why plist hooked all six leading/trailing comment ops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommentDelimiter {
    pub open: &'static [u8],
    /// Empty for an `open` alone.
    pub close: &'static [u8],
    /// Text a comment body may not contain — `--` inside an XML comment —
    /// refused as `InvalidComment` before anything is spliced.
    pub forbidden: Option<&'static [u8]>,
}

impl CommentDelimiter {
    /// A delimiter that is an `open` alone.
    pub const fn open(open: &'static [u8]) -> CommentDelimiter {
        CommentDelimiter { open, close: b"", forbidden: None }
    }

    /// A paired delimiter.
    pub const fn pair(open: &'static [u8], close: &'static [u8]) -> CommentDelimiter {
        CommentDelimiter { open, close, forbidden: None }
    }
}

/// A format's whole comment surface: which scanner walks an owned comment
/// block, and the markers the editor writes and strips.
///
/// One field on `Syntax` rather than three, because the three answers
/// coincide for most formats. The two presets below are exactly the "all
/// three agree" case; a format whose answers diverge — INI, NestedText,
/// plist — writes the literal out, and so does JSON, whose marker varies by
/// dialect while its scanner does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comments {
    /// The own-line (leading) comment delimiter, or `None` when the dialect
    /// has none to write — strict JSON, where the comment ops return
    /// `CommentsUnsupported`. A paired delimiter (plist) serves the leading
    /// and trailing ops; the dangling and comment-out ops need a bare prefix
    /// and refuse a pair.
    pub line: Option<CommentDelimiter>,

    /// The marker for a same-line TRAILING comment specifically, or `None`
    /// when the format has no such syntax.
    ///
    /// Distinct from `line` because INI and NestedText have real, safe
    /// leading comments but no trailing ones: a `;`/`#` after a value on the
    /// SAME line is literal value text, not a comment. Splicing one in anyway
    /// would silently corrupt the value on reread, so trailing ops are
    /// refused there.
    pub trailing: Option<CommentDelimiter>,

    /// A paired comment that may run over several lines, which the
    /// owned-comment scan walks as a unit (`/* ... */` in JSONC and JSON5),
    /// or `None`. A paired `line` delimiter (plist's `<!-- -->`) is
    /// recognized on one line only; this is the one that is walked across
    /// lines.
    pub block: Option<CommentDelimiter>,
}

impl Comments {
    /// `#` throughout — YAML, TOML, fig, dotenv, `.properties`.
    pub const HASH: Comments = Comments {
        line: Some(CommentDelimiter::open(b"#")),
        trailing: Some(CommentDelimiter::open(b"#")),
        block: None,
    };

    /// `//` throughout — ZON, which follows Zig.
    pub const SLASHES: Comments = Comments {
        line: Some(CommentDelimiter::open(b"//")),
        trailing: Some(CommentDelimiter::open(b"//")),
        block: None,
    };

    /// No comment syntax at all.
    pub const NONE: Comments = Comments { line: None, trailing: None, block: None };
}

/// How a section format spells a header line that opens a container of its
/// own: `[` + path + `]` for a TOML table, `[[` + path + `]]` for an element
/// of an array of tables, an INI `[section]`. See `Syntax::section_header`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionHeader {
    pub open: &'static [u8],
    pub close: &'static [u8],
    /// The element-of-a-sequence form, or `None` for a format without one.
    pub seq_open: Option<&'static [u8]>,
    pub seq_close: Option<&'static [u8]>,
    /// Joins the path's key segments, each spelled by the language's
    /// `render_key_name`. Default `.`.
    pub sep: &'static [u8],
    /// Whether index segments are left out of the path — `[[a.b]]` always
    /// names `a`'s last element, so the index is implied. Default true.
    pub skip_index: bool,
}

impl SectionHeader {
    /// `open` + path + `close`, with the defaults for the rest.
    pub const fn new(open: &'static [u8], close: &'static [u8]) -> SectionHeader {
        SectionHeader { open, close, seq_open: None, seq_close: None, sep: b".", skip_index: true }
    }
}

/// What `fig` can do with a format, as declared by the format itself.
///
/// The single source: the C ABI's `fig_format_capabilities` reads these bits
/// rather than restating them, so it cannot disagree with the format about
/// what the format can do.
///
/// Per-language, where the format registry is per-dialect (json/jsonc/json5
/// are three registry rows over one `Language`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// `parse` accepts this format. True for every language in tree.
    pub read: bool,
    /// The editor may edit this format. True for every language in tree; an
    /// out-of-tree `Language` may still declare false, and the CLI and C ABI
    /// refuse to edit it.
    pub edit: bool,
    /// `print` can write this format.
    pub serialize: bool,
    /// This format has a reference layer: its `parse` may return anchors,
    /// aliases, `<<` merges and tags, and its `print` spells them. A document
    /// carrying one is written out intact when the target declares this too,
    /// and collapsed by `Materialize` first when it does not — which is the
    /// whole of how the CLI and the C ABI decide to run that pass, so a
    /// language that produces alias rows without declaring this hands another
    /// format's printer an alias it cannot spell. YAML alone in tree.
    pub references: bool,
    /// What the lossless `$fig` envelope pass (`lossless.rs`) may assume
    /// about this format's value model on OUTPUT, or `None` when the format
    /// takes no envelope at all.
    ///
    /// `Some` says: when `--lossless` targets this format, wrap every scalar
    /// kind NOT marked native in `NativeKinds` in a `$fig` envelope so a
    /// later run can rebuild it, and leave the marked kinds bare. Only the
    /// four typed formats with a real value model and a mapping to carry the
    /// envelope in — JSON, YAML, TOML, ZON — declare one.
    ///
    /// `None` says: never encode an envelope into this format (envelopes in
    /// its INPUT are still decoded) — because fig and canonical spell every
    /// kind directly, or because INI, dotenv, `.properties`, plist and
    /// NestedText have no typed scalar envelope of their own. It sits on the
    /// LANGUAGE (json/jsonc/json5 share it), which is why JSON5's native
    /// `Infinity`/`NaN` are still enveloped.
    pub lossless: Option<NativeKinds>,
    /// How many levels of mapping nesting this format can represent, or
    /// `None` for a format with no depth limit at all (every typed format).
    /// INI holds a root mapping plus one level of `[section]`s (1); dotenv
    /// and `.properties` are flat — the root mapping itself, nothing nested
    /// under it (0).
    ///
    /// A `Some` also says the format holds no sequence anywhere and no
    /// `null`: the three flat formats share that shape, and `flat_strip.rs`
    /// — the lossy pass that drops what such a format cannot hold before
    /// printing — and `diagnostics.rs`'s matching warning read this one field
    /// for the depth and take the rest as given.
    pub max_mapping_depth: Option<u8>,
}

impl Caps {
    /// Zig's defaults: `read` alone.
    pub const DEFAULT: Caps =
        Caps { read: true, edit: false, serialize: false, references: false, lossless: None, max_mapping_depth: None };
}

impl Default for Caps {
    fn default() -> Self {
        Caps::DEFAULT
    }
}

/// The scalar kinds a format spells natively, beyond the core four every
/// serialize format has (boolean, string, number, and the two containers).
/// Read by `lossless.rs`, whose `needsEnvelope` is exactly "the kind is one
/// of these and the format did not mark it".
///
/// One field per kind the envelope can carry: `null`, plus one per `ExtKind`
/// member, named identically (`ext` reads one by `ExtKind`). Every field
/// defaults to false — a format declares what it holds, and an omission is
/// "envelope it", which is always lossless if sometimes unidiomatic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NativeKinds {
    /// A bare `null`. Every typed format but TOML has one; TOML's absence is
    /// the one kind the lossy path DROPS rather than degrades, since there is
    /// no string to collapse it to.
    pub null: bool,
    /// The four RFC-3339-derived TOML datetimes.
    pub offset_datetime: bool,
    pub local_datetime: bool,
    pub local_date: bool,
    pub local_time: bool,
    /// ZON's `.name` and `'c'` literals.
    pub enum_literal: bool,
    pub char_literal: bool,
    /// A non-finite float (`inf`/`nan`, JSON5's `Infinity`/`NaN`).
    pub number_special: bool,
    /// plist's `<date>`.
    pub plist_date: bool,
    /// A byte string (`ExtKind::Data`): plist's `<data>`, YAML's `!!binary`,
    /// a binary format's bytes.
    pub data: bool,
}

impl NativeKinds {
    /// Nothing held.
    pub const NONE: NativeKinds = NativeKinds {
        null: false,
        offset_datetime: false,
        local_datetime: false,
        local_date: false,
        local_time: false,
        enum_literal: false,
        char_literal: false,
        number_special: false,
        plist_date: false,
        data: false,
    };

    /// The field for `k` (Zig reads it as `@field(kinds, @tagName(k))`).
    pub fn ext(&self, k: ExtKind) -> bool {
        match k {
            ExtKind::OffsetDatetime => self.offset_datetime,
            ExtKind::LocalDatetime => self.local_datetime,
            ExtKind::LocalDate => self.local_date,
            ExtKind::LocalTime => self.local_time,
            ExtKind::EnumLiteral => self.enum_literal,
            ExtKind::CharLiteral => self.char_literal,
            ExtKind::NumberSpecial => self.number_special,
            ExtKind::PlistDate => self.plist_date,
            ExtKind::Data => self.data,
        }
    }

    /// Set the field for `k`.
    pub fn set_ext(&mut self, k: ExtKind, v: bool) {
        match k {
            ExtKind::OffsetDatetime => self.offset_datetime = v,
            ExtKind::LocalDatetime => self.local_datetime = v,
            ExtKind::LocalDate => self.local_date = v,
            ExtKind::LocalTime => self.local_time = v,
            ExtKind::EnumLiteral => self.enum_literal = v,
            ExtKind::CharLiteral => self.char_literal = v,
            ExtKind::NumberSpecial => self.number_special = v,
            ExtKind::PlistDate => self.plist_date = v,
            ExtKind::Data => self.data = v,
        }
    }
}

/// Every kind held: a format whose printer writes every value as itself.
pub const HOLDS_EVERYTHING: NativeKinds = NativeKinds {
    null: true,
    offset_datetime: true,
    local_datetime: true,
    local_date: true,
    local_time: true,
    enum_literal: true,
    char_literal: true,
    number_special: true,
    plist_date: true,
    data: true,
};

/// How a format takes the caller's edit text, which decides what the fix is
/// when the text turns out not to fit. Declared once per dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpliceStyle {
    /// Spliced in verbatim as source, so a string value needs its own quotes
    /// — YAML, TOML, ZON, fig.
    Literal,
    /// Wrapped as a quoted, escaped string first (`edit_ops.jsonifyEdit`):
    /// in `"` quotes, with `"` and `\` escaped as `\"` and `\\`, the C0
    /// controls with a short form as `\b \f \n \r \t` and the rest as
    /// `\u00XX`, and every other byte as itself — the JSON family.
    Escaped,
    /// Written as raw characters, so only the format's own separators can
    /// break it — INI, dotenv, `.properties`, plist, NestedText. (plist and
    /// NestedText *render* the text rather than splicing it.)
    Raw,
}

impl SpliceStyle {
    /// The enum ordinal (the wire's `splice` number: 0, 1, 2).
    pub fn ordinal(self) -> u8 {
        self as u8
    }
}

/// One `--spec <version>` string and the dialect it selects. The element
/// type of `DialectInfo::specs`. A language's registration lists each as a
/// dialect row of its own, named `<dialect>-<version>` (`yaml-1.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecName {
    /// The accepted `--spec` text, matched exactly. Several map to one
    /// dialect (`1.0` and `1.0.0` both select TOML 1.0).
    pub name: &'static str,
    pub dialect: Dialect,
}

/// One user-facing dialect of a language (Zig's `manifest.Dialect(L)`):
/// everything about it that is not the language itself. A language declares
/// its own as `Language::dialects` — one entry for most, three for JSON
/// (json/jsonc/json5 share one `Language`) — and its registration lists each
/// as a dialect row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialectInfo {
    /// The dialect's registered name. The entry that selects the language's
    /// `default_dialect` must be named `Language::name` (`validate` checks);
    /// the others are the language's business (JSON's `jsonc`/`json5`).
    pub name: &'static str,
    /// The `Dialect` this row selects. `new` sets 0, the language's default
    /// in every language in tree; only the JSON trio overrides it.
    pub dialect: Dialect,
    /// How this dialect takes spliced edit text. See `SpliceStyle`.
    pub splice: SpliceStyle,
    /// The document `set` seeds when the target file does not exist yet, or
    /// `None` for a format that refuses to be created from scratch.
    ///
    /// An empty string is NOT the same statement as `None`: it means an
    /// empty file already parses as an empty root mapping, so the first key
    /// can just be inserted into it.
    pub empty_doc_seed: Option<&'static [u8]>,
    /// The Zig printer declarations that write a whole document in this
    /// dialect, and a single node: informational here, since a Rust
    /// language's `print` dispatches on the dialect itself.
    pub print_name: &'static str,
    pub print_node_name: &'static str,
    /// The printer entry for a value FRAGMENT where it differs from the
    /// document's, or `None`. figl's document printer refuses a scalar root,
    /// which a whole figl document cannot be, but a value a caller built is
    /// not a document. (Informational, as `print_name`.)
    pub print_fragment_name: Option<&'static str>,
    /// The `--spec` strings this dialect accepts and what each selects, each
    /// registered as a dialect row `<name>-<version>`. Empty for the dialects
    /// with a single grammar.
    pub specs: &'static [SpecName],
    /// What this dialect's printer makes of a tree from another format, for
    /// the conversion warnings (`diagnostics.rs`). See `Prints`.
    pub prints: Option<Prints>,
}

impl DialectInfo {
    /// A dialect row with Zig's defaults for everything but the three fields
    /// it requires.
    pub const fn new(name: &'static str, splice: SpliceStyle, empty_doc_seed: Option<&'static [u8]>) -> DialectInfo {
        DialectInfo {
            name,
            dialect: 0,
            splice,
            empty_doc_seed,
            print_name: "print",
            print_node_name: "printNode",
            print_fragment_name: None,
            specs: &[],
            prints: None,
        }
    }
}

/// One document of a multi-document stream, as a language's `split_stream`
/// locates it: its byte range in the stream's source, `[start, end)`, which
/// parses as a document of its own, and whether it opened with a
/// document-start marker (YAML's `---`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamDocument {
    pub start: usize,
    pub end: usize,
    pub explicit: bool,
}

/// When a printer writes comments (`Prints::comments`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrintsComments {
    /// Always.
    #[default]
    Always,
    /// Only when pretty-printing (on one line it has nowhere to put them).
    Pretty,
    /// Never.
    Never,
}

impl PrintsComments {
    /// The enum ordinal (0, 1, 2).
    pub fn ordinal(self) -> u8 {
        self as u8
    }
}

/// What a dialect's printer writes of a tree read from another format: the
/// declaration the conversion warnings are computed from, so that no part
/// of core needs to know which format is the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prints {
    /// The kinds written as themselves. A `null` not held is dropped; an
    /// extended scalar not held degrades, to what `char_as` and
    /// `number_special_as` say for those two and to a string for the rest.
    pub holds: NativeKinds,
    /// Booleans and numbers written as themselves; false where every scalar
    /// is text (INI, NestedText), which a typed one degrades to. Default
    /// true.
    pub typed_scalars: bool,
    /// A `null` at the root is an empty document rather than a loss
    /// (NestedText), though a nested one is.
    pub null_at_root: bool,
    /// The root must be a mapping or a sequence (figl); any other root is
    /// dropped.
    pub root_container: bool,
    /// What a char literal not held is written as: its codepoint as a
    /// `"number"` or an `"integer"`, or a `"string"` (the default).
    pub char_as: &'static str,
    /// What a non-finite float not held is written as: a `"string"` (the
    /// default), or a `"quoted string"` where the bare one would not read
    /// back as text.
    pub number_special_as: &'static str,
    /// When the printer writes comments.
    pub comments: PrintsComments,
}

impl Prints {
    /// Zig's defaults around `holds`.
    pub const fn new(holds: NativeKinds) -> Prints {
        Prints {
            holds,
            typed_scalars: true,
            null_at_root: false,
            root_container: false,
            char_as: "string",
            number_special_as: "string",
            comments: PrintsComments::Always,
        }
    }
}

/// Everything the generic splice engine needs to know about a format's
/// surface syntax, indexed by dialect.
///
/// Obtained as `Language::syntax(dialect)` rather than as a constant because
/// `comments.line` genuinely varies by dialect: strict JSON has no comment
/// syntax while JSONC and JSON5 do, and the splice is reparsed under
/// whichever dialect the editor is holding.
///
/// `Syntax::new` takes the three fields Zig requires and gives the rest
/// Zig's defaults; spell a format's as
/// `Syntax { section_noun: Some(b"section"), ..Syntax::new(..) }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Syntax {
    // ── COMMENTS ──
    /// This format's comment scanner and markers. See `Comments`.
    pub comments: Comments,

    // ── ENTRIES ──
    /// The mapping key/value separator spliced by the generic flow-entry
    /// insert helpers (`insertFlowMapEntry`/`insertFlowEntry`) and by
    /// `writeMapValue`'s block-insert path — or `None` for a format that
    /// owns every one of those paths itself and so has no answer to give.
    ///
    /// This is the separator the GENERIC engine writes, which need not be the
    /// separator the format's printer writes: ZON's struct-field syntax is
    /// ` = `, dotenv/`.properties` print a bare `=` with no surrounding
    /// spaces, INI always pads it.
    ///
    /// The `None` is not "no separator" — it is "not the generic engine's
    /// question". plist's entries are a pair of sibling ELEMENTS with no
    /// separator to write, and its `render_entry` spells the whole entry;
    /// `validate` requires a `None` here to come with one, and the editor's
    /// `kvSep` is where that is cashed in. A format with a flow form whose
    /// separator varies by object (fig) declares its default here and
    /// `flow_kv_sep_from_siblings` beside it.
    pub kv_sep: Option<&'static [u8]>,

    /// Whether the flow-entry insert copies the separator the container's
    /// FIRST entry uses — the bytes between that entry's key and value —
    /// rather than writing `kv_sep`. True for fig, whose flow objects are
    /// either `=`-mode or `:`-mode (JSON-embedded) and may not mix the two,
    /// so the right separator is whichever the object already uses. An empty
    /// flow mapping has no first entry and takes `flow_map_kv_sep`, or else
    /// `kv_sep`. With it, an insert into a flow object whose first two
    /// members share a line also copies the bytes between them.
    pub flow_kv_sep_from_siblings: bool,

    /// Bytes written inside the braces around a freshly created single
    /// member of an EMPTY flow mapping: `" "` for TOML's `{ x = 1 }`, none
    /// for JSON's and YAML's tight `{x: 1}` or fig's `{x=1}`.
    pub flow_map_pad: &'static [u8],

    /// A sigil each key carries in the source that its AST key span EXCLUDES
    /// — ZON's leading `.`, whose span starts at the bare identifier.
    ///
    /// A flow-mapping entry delete backs up over it so the splice carries
    /// `.name` as a unit rather than stranding a bare `.` next to a survivor.
    pub key_sigil: Option<u8>,

    // ── SHAPES ──
    /// The empty-mapping seed `set` splices to auto-vivify a missing
    /// ancestor, or `None` for a format that cannot vivify at all.
    ///
    ///   * Most formats use the flow `{}` literal, which each accepts as an
    ///     empty mapping value (JSON object, TOML inline table, fig flow
    ///     map). ZON spells it `.{}`.
    ///   * YAML seeds with NOTHING (`""`) — a bare `key:`, i.e. a null value,
    ///     which `insertKey` promotes to a real block mapping
    ///     (`promoteNullToMapping`), taking block and inline values alike.
    ///   * INI, plist and NestedText declare `None`: they have no literal
    ///     spelling for "an empty nested mapping" that the generic seed could
    ///     use (`{}` in INI is a two-character STRING value).
    pub empty_map_literal: Option<&'static [u8]>,

    /// Whether a BLOCK (non-flow) sequence can be edited in place.
    ///
    /// False for TOML alone: a non-flow TOML sequence is an array-of-tables,
    /// which `appendContainerToSeq` handles instead, and TOML has no block
    /// scalar array. Append/prepend/remove/reorder all refuse with
    /// `NotAnInlineArray` when this is false. Default true.
    pub block_seq_editable: bool,

    /// Whether this format has FLOW container syntax at all — a `{…}` or
    /// `[…]` collection spelled inline, which the engine edits by
    /// comma-aware splice rather than by line.
    ///
    /// The engine tells flow from block by sniffing a container's first byte
    /// (`splice.isFlow`), and the sniff is right for every format that has
    /// both shapes. It is wrong for a format that has neither: INI's root
    /// span is the whole file, so a file opening with `[section]` reads as a
    /// bracket-delimited flow root, and NestedText's `- item` lines are not a
    /// flow sequence however a value happens to begin. Declaring false here
    /// answers "block" before the sniff runs. INI, NestedText, plist, dotenv
    /// and `.properties` declare false. Default true.
    pub flow_containers: bool,

    /// The bytes one level of block nesting adds to a line's prefix: two
    /// spaces for YAML (the default), four for NestedText, one `> ` marker
    /// cell for fig. Read by the editor's `writeMapValue` and
    /// `promoteNullToMapping`.
    pub indent_unit: &'static [u8],

    /// What introduces a block-sequence item, separator included: `- ` (the
    /// default) for YAML and NestedText, `* ` for fig, `""` for plist, whose
    /// item is a bare element. The engine writes a new item as the first
    /// item's line prefix (the bytes before its marker — see
    /// `Document::marker_span`) followed by this.
    pub seq_item_marker: &'static [u8],

    /// The tokens that open and close a block container, for a format whose
    /// block containers close themselves — plist's `<dict>`/`</dict>` and
    /// `<array>`/`</array>` — or `None` for every line-structured format,
    /// where a block collection has no closing token at all.
    ///
    /// The engine expands an EMPTY container of such a format (`<dict/>`)
    /// into its multi-line form around the first entry or item it inserts; a
    /// format declaring `None` refuses that insert with
    /// `EmptyInlineContainer`. And a same-line trailing comment on a
    /// container value follows the close here.
    pub closed_containers: Option<ClosedContainers>,

    /// Whether a single line of the form `k: v` is a block MAPPING entry
    /// rather than scalar text — the one value shape that cannot be told
    /// apart by sniffing, so it is settled by the language's own parser. True
    /// for YAML alone.
    pub single_line_block_mapping: bool,

    /// Whether a bare `key: value` document form exists — a keyless
    /// top-level mapping, as in YAML and JSON5. False for ZON alone, where a
    /// null value promotes in place to a flow `.{ key = value }` container
    /// built from `flow_map_open`/`flow_map_close`. Default true.
    pub bare_document_mapping: bool,

    /// The flow-mapping delimiters, used when promoting a null in a format
    /// with no `bare_document_mapping`. ZON's opener is `.{`. Default `{`
    /// and `}`.
    pub flow_map_open: &'static [u8],
    pub flow_map_close: &'static [u8],

    /// Whether a line's prefix is STRUCTURAL rather than whitespace. True for
    /// fig alone: its `#`-only comment lines need the same `>` marker-run
    /// prefix as the line they anchor above, so the "indent" a new comment
    /// copies is the raw byte range from the line start to the node's span.
    pub structural_indent: bool,

    /// Declares this a SECTION format — one whose logical containers are
    /// assembled from lines scattered through the source (a TOML `[table]`,
    /// an INI `[section]`, a fig block container) — and names what the
    /// format calls such a container ("table", "section", "container"), for
    /// a consumer to put in a message. `None` for every format whose
    /// containers are contiguous.
    ///
    /// With it, the parser is expected to fill `Document::regions_of` with
    /// the header lines of every such container, and the generic
    /// `deleteContainer`/`moveContainer`/`reorderContainers` are live.
    pub section_noun: Option<&'static [u8]>,

    /// How a header line that opens a container of its own is spelled, for a
    /// section format that has one — TOML's `[a.b]` and `[[a.b]]`. With it
    /// the engine's `insertContainer` and `appendContainerToSeq` are live.
    /// `None` for a section format whose containers are not opened by a
    /// header line the engine could write on its own.
    pub section_header: Option<SectionHeader>,

    /// The key that MERGES another mapping's entries into this one — YAML's
    /// `<<` — or `None` for a format with no such key. With it declared, a
    /// key the document resolves through a merge but never spells out is
    /// INHERITED: `replaceValAtPath` shadows it with a local entry and
    /// `deleteKey` refuses it with `MergeOnlyKey`. The merge's resolution is
    /// core (`AST::merged_child`).
    pub merge_key: Option<&'static [u8]>,

    // ── SEPARATED ENTRIES ──
    /// The bytes between two members of a flow container: `,` (the default)
    /// for every format with commas, `;` for a CSS declaration block. See
    /// `docs/proposals/separated-entries.md`.
    pub flow_entry_sep: &'static [u8],

    /// Whether the document ROOT is a flow mapping spelled with no
    /// delimiters — members joined by `flow_entry_sep` from the source's
    /// first byte to its last, as a CSS `style` attribute is.
    pub flow_root: bool,

    /// Whether only a mapping opener (`{`, or ZON's `.{`) opens a flow
    /// container — a `[` not opening a flow sequence.
    pub flow_maps_only: bool,

    /// Bytes other than whitespace that may stand before a node at the start
    /// of its line, beside the non-space bytes of `seq_item_marker`, which
    /// always may: figl's `>` depth markers and `+` lines. A line whose
    /// prefix holds anything else holds another node too, so the node does
    /// not own the line, and commenting it out is refused.
    pub line_prefix_marks: &'static [u8],

    // ── COMPACT OBJECTS ──
    /// The key/value separator of a flow object whose entries are spelled
    /// apart from block entries: fig's `=` in `{a=1}`, against ` = ` in
    /// `a = 1`. Empty for a format whose flow and block entries are spelled
    /// alike.
    pub flow_map_kv_sep: &'static [u8],

    /// The bytes between two members, on one line, of a flow object whose
    /// first entry separates its key and value with `flow_map_kv_sep`: fig's
    /// ` ` in `{a=1 b=2}`. Empty for none.
    pub flow_map_entry_sep: &'static [u8],

    /// The byte that, as the last on a line and not itself escaped, carries
    /// the line on to the next physical one — `.properties`' `\` — or `None`
    /// for a format whose lines never continue.
    pub line_continuation: Option<u8>,

    // ── EMPTY SEQUENCES ──
    /// The empty-sequence seed `setAppend` and `setSequence` splice to start
    /// a list that is not there yet, or `None` for a format with no spelling
    /// for an empty one — `empty_map_literal`'s twin. `[]` for JSON, TOML,
    /// YAML and fig; `.{}` for ZON; `<array/>` for plist.
    pub empty_seq_literal: Option<&'static [u8]>,

    // ── MULTI-LINE VALUES ──
    /// Whether a scalar written over several lines takes its same-line
    /// trailing comment on the line it OPENS on — figl's `x = ''' # note` —
    /// rather than after its close. So does a flow list or object written
    /// over several lines whose opener line holds no element. True for figl
    /// alone.
    pub opener_line_comment: bool,
}

impl Syntax {
    /// The three fields Zig requires, with Zig's defaults for the rest.
    pub const fn new(comments: Comments, kv_sep: Option<&'static [u8]>, empty_map_literal: Option<&'static [u8]>) -> Syntax {
        Syntax {
            comments,
            kv_sep,
            flow_kv_sep_from_siblings: false,
            flow_map_pad: b"",
            key_sigil: None,
            empty_map_literal,
            block_seq_editable: true,
            flow_containers: true,
            indent_unit: b"  ",
            seq_item_marker: b"- ",
            closed_containers: None,
            single_line_block_mapping: false,
            bare_document_mapping: true,
            flow_map_open: b"{",
            flow_map_close: b"}",
            structural_indent: false,
            section_noun: None,
            section_header: None,
            merge_key: None,
            flow_entry_sep: b",",
            flow_root: false,
            flow_maps_only: false,
            line_prefix_marks: b"",
            flow_map_kv_sep: b"",
            flow_map_entry_sep: b"",
            line_continuation: None,
            empty_seq_literal: None,
            opener_line_comment: false,
        }
    }
}

/// The fragment renderers a format may declare — `Decls.renderers` in
/// `language.zig`, as an enum the engine can ask about
/// (`Language::has_renderer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Renderer {
    Value,
    Entry,
    Item,
    Tail,
    Key,
    KeyName,
    String,
}

impl Renderer {
    pub const ALL: [Renderer; 7] =
        [Renderer::Value, Renderer::Entry, Renderer::Item, Renderer::Tail, Renderer::Key, Renderer::KeyName, Renderer::String];

    /// The Zig declaration name: `renderValue` for `Value`.
    pub fn decl_name(self) -> &'static str {
        match self {
            Renderer::Value => "renderValue",
            Renderer::Entry => "renderEntry",
            Renderer::Item => "renderItem",
            Renderer::Tail => "renderTail",
            Renderer::Key => "renderKey",
            Renderer::KeyName => "renderKeyName",
            Renderer::String => "renderString",
        }
    }

    /// The enum tag name as the wire spells it: `key_name` for `KeyName`.
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

/// Everything a renderer is told, as one record: every renderer takes it,
/// and reads the fields its fragment needs. `indent` is the line prefix the
/// fragment's first line sits after, which a continuation line copies; `key`
/// the key as written (entry, tail, key); `value` the value text, already
/// through `render_value` for every renderer but the value's own; `literal`
/// what fig's bare-literal rules make of it (value); `old_key` the key being
/// renamed, as the source spells it (key). For `String`, `value` is the new
/// string itself, decoded, and `old_value` the string it replaces as the
/// source spells it, quotes and all.
///
/// `parent_key` and `parent_tag` say where the fragment goes: the CONTAINER
/// it is written into — the mapping an entry joins, the sequence an item
/// joins, the container holding a value replaced or a key renamed — by the
/// NAME of the key it stands under (decoded, not spelled; empty at the root
/// and for a container that is itself a sequence item) and by its tag as the
/// node table spells one (`!dependency`, `!!map`; empty for none). Empty
/// where the engine has no container in hand; nothing is filled for one
/// renderer and not another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderRequest<'a> {
    pub indent: &'a [u8],
    pub key: &'a [u8],
    pub value: &'a [u8],
    pub literal: Literal,
    pub old_key: &'a [u8],
    pub parent_key: &'a [u8],
    pub parent_tag: &'a [u8],
    pub old_value: &'a [u8],
}

/// What fig's bare-literal rules make of the text a value renderer is
/// handed: `null`, `true`/`false`, a number (integer or float), a datetime
/// shape, or a string — the classification the `.fig` dialect gives a bare
/// token (`languages/figl/tokenizer.zig`'s `sniffBare`: `Yes` and `007` stay
/// strings), computed once by the engine over the text trimmed of whitespace
/// and passed to `render_value`, so that no format restates the rule and
/// every format's `set` means the same thing by `42`. A datetime is a string
/// in the node table; here it is its own answer, since a renderer spells it
/// differently (plist's `<date>`). The names (`name`) are the spelling on
/// the runtime vtable and the helper wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
    pub const ALL: [Literal; 6] = [Literal::Null, Literal::Bool, Literal::Int, Literal::Float, Literal::Datetime, Literal::String];

    /// The tag name: `null`, `bool`, `int`, `float`, `datetime`, `string`.
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
    pub fn parse(name: &[u8]) -> Option<Literal> {
        Self::ALL.into_iter().find(|l| l.name().as_bytes() == name)
    }
}

/// An opening and closing token pair. See `Syntax::closed_containers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delimiters {
    pub open: &'static [u8],
    pub close: &'static [u8],
}

/// The self-closing block container spellings of a format whose containers
/// have them. See `Syntax::closed_containers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClosedContainers {
    pub map: Delimiters,
    pub seq: Delimiters,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_names_round_trip() {
        for l in Literal::ALL {
            assert_eq!(Some(l), Literal::parse(l.name().as_bytes()));
        }
        assert_eq!(None, Literal::parse(b"nope"));
    }

    #[test]
    fn native_kinds_has_a_field_per_ext_kind() {
        let mut k = NativeKinds::NONE;
        for e in ExtKind::ALL {
            assert!(!k.ext(e));
            k.set_ext(e, true);
            assert!(HOLDS_EVERYTHING.ext(e));
        }
        k.null = true;
        assert_eq!(HOLDS_EVERYTHING, k);
    }
}
