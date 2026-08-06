//! The declared half of the `Language` interface — what a format supplies
//! about its own syntax, rather than what `Editor` infers by testing which
//! language it was instantiated with.
//!
//! `language.zig`'s `validate` states the four-declaration contract (`Type`,
//! `default_type`, `parse`, `print`). That was never the whole of it: the rest
//! lived as `if (Language == Toml)` / `== Plist` / `!= NestedText` branches
//! inside `editor.zig`, discoverable only by reading them. The types here are
//! where that unwritten half becomes writable — a format declares `syntax`,
//! `caps`, `name` and `extensions`, and the generic engine reads them instead
//! of asking what it is. See `docs/proposals/language-interface.md`.
//!
//! This module is the PARAMETER half of that interface — the answers a format
//! can give as a value. The other half is operations it takes over outright,
//! which are declared as hooks in the "Editing hooks" block of each
//! `<lang>/<lang>.zig` and dispatched by `@hasDecl` from `editor.zig`; they
//! need no types here, because a hook's signature is fixed by the `Editor`
//! method it overrides.
//!
//! This module is deliberately a LEAF: it imports nothing, not even `std`.
//! `language.zig` re-exports these types and every `<lang>/<lang>.zig` imports
//! them, so anything pulled in here would be pulled into all eleven language
//! modules — and an import back to `language.zig` (which imports each of them
//! in turn) would make the manifest's own types depend on the languages that
//! declare them.

/// Which leading-comment syntax a language uses, so the owned-comment scan in
/// delete/move (`editor.commentBlockStart`) recognizes the right marker.
///
/// Distinct from `Syntax.line_comment`: this selects the *scanner*, which is
/// per-language and comptime, while the marker is per-dialect and may be null
/// where the scanner still has a sensible answer. Plain JSON has no comments,
/// but `.slashes` is harmless there since no `//` line can exist.
pub const CommentStyle = enum {
    /// YAML, TOML, fig, dotenv, `.properties`, NestedText.
    hash,
    /// JSON/JSONC/JSON5 and ZON (which follows Zig). The only style whose
    /// scanner also walks multi-line `/* ... */` blocks as a unit.
    slashes,
    /// INI. Its printer accepts a leading `#` on read but always WRITES `;`,
    /// so `;` is the marker the editor's own inserts and scans use.
    semicolon,
    /// plist's `<!-- ... -->`.
    xml_comment,
};

/// How a logical mapping key renders as this format's key syntax on the `set`
/// insert path (`editor.formatInsertKey`).
///
/// A logical key is plain text (`b`, `has space`); what reaches the source
/// depends on the format's key grammar, and the splice is reparsed under it.
pub const KeyStyle = enum {
    /// Spliced as-is — YAML, TOML, fig, INI, dotenv, `.properties`,
    /// NestedText, plist. The same thing `insertKey`'s other callers do.
    verbatim,
    /// Quoted and escaped as a JSON string (`b` -> `"b"`). Required by strict
    /// JSON and harmless in JSONC/JSON5.
    json_quoted,
    /// ZON's struct-field syntax, which always carries a leading `.`
    /// (`b` -> `.b`, quoted as `.@"has space"` when not a bare identifier).
    zon_field,
};

/// What `fig` can do with a format, as declared by the format itself.
///
/// Hand-maintained a second time in `c_api.fig_format_capabilities`, which is
/// the drift this is eventually meant to close — see the proposal's §7. Note
/// that `caps` is per-LANGUAGE while the C ABI's `FigFormat` is per-dialect
/// (json/jsonc/json5 all map to this one `Language`), so the two are not the
/// same table and the mapping between them cannot be generated from here.
pub const Caps = struct {
    /// `parse` accepts this format. True for every language in tree.
    read: bool = true,
    /// `Editor(Language)` is instantiated for this format. False for XML,
    /// which has a reader and a writer but no in-place editor yet.
    edit: bool = false,
    /// `print` can write this format.
    serialize: bool = false,
};

/// Everything the generic splice engine needs to know about a format's
/// surface syntax, indexed by dialect.
///
/// Obtained as `Language.syntax(t)` rather than as a constant because
/// `line_comment` genuinely varies by dialect: strict JSON has no comment
/// syntax while JSONC and JSON5 do, and the splice is reparsed under whichever
/// dialect the editor is holding. Making the whole struct a function of `Type`
/// keeps that question in one place instead of scattering per-field `fn(Type)`
/// types across the struct, and is where a TOML 1.0/1.1 or YAML 1.1/1.2.2
/// *editing* divergence would land if one ever appears.
///
/// The cost is a runtime switch where there used to be a comptime constant,
/// and it stops there: every consumer is an `appendSlice` call or an argument
/// to `commentBlockStart`/`entryBlockStart`. Nothing downstream needs a
/// comptime value — no array lengths, no `++`, no switch prongs.
pub const Syntax = struct {
    // ==================
    // COMMENTS
    // ==================

    /// Selects the owned-comment-block scanner. See `CommentStyle`.
    comment_style: CommentStyle,

    /// The own-line (leading) comment marker, or null when the dialect
    /// forbids comments entirely — strict JSON, where the comment ops return
    /// `CommentsUnsupported`.
    line_comment: ?[]const u8,

    /// The marker for a same-line TRAILING comment specifically, or null when
    /// the format has no such syntax.
    ///
    /// Distinct from `line_comment` because INI and NestedText have real,
    /// safe leading comments but no trailing ones: a `;`/`#` after a value on
    /// the SAME line is literal value text, not a comment (see
    /// `ini/parser.zig`'s "a value runs to end of line" and
    /// `nestedtext/parser.zig`'s "rest-of-line values are 100% literal", and
    /// both printers, which render a "trailing" comment as its own line
    /// immediately after the entry). Splicing one in anyway would silently
    /// corrupt the value on reread, so trailing ops are refused there.
    /// Every other language repeats its `line_comment` here.
    trailing_comment: ?[]const u8,

    // ==================
    // ENTRIES
    // ==================

    /// The mapping key/value separator spliced by the generic flow-entry
    /// insert helpers (`insertFlowMapEntry`/`insertFlowEntry`) and by
    /// `writeMapValue`'s block-insert path.
    ///
    /// This is the separator the GENERIC engine writes, which is not always
    /// the separator the format's printer writes: TOML and fig both spell an
    /// entry `key = value`, but every path where that matters is delegated to
    /// their own `editor_helper.zig`, so the value they declare here is the
    /// one the shared helpers were already using. ZON's struct-field syntax
    /// is ` = `; dotenv/`.properties` print a bare `=` with no surrounding
    /// spaces, while INI always pads it. See each `printer.zig`.
    kv_sep: []const u8,

    /// How a logical key renders into this format's key syntax. See `KeyStyle`.
    key_style: KeyStyle = .verbatim,

    /// A sigil each key carries in the source that its AST key span EXCLUDES
    /// — ZON's leading `.`, whose span starts at the bare identifier.
    ///
    /// A flow-mapping entry delete backs up over it so the splice carries
    /// `.name` as a unit rather than stranding a bare `.` next to a survivor.
    key_sigil: ?u8 = null,

    // ==================
    // SHAPES
    // ==================

    /// The empty-mapping seed `set` splices to auto-vivify a missing ancestor,
    /// or null for a format that cannot vivify at all.
    ///
    /// Three distinct answers, and the null is not a degenerate case:
    ///
    ///   * Most formats use the flow `{}` literal, which each accepts as an
    ///     empty mapping value (JSON object, TOML inline table, fig flow map).
    ///     ZON spells it `.{}`. The dotted-key formats (fig/TOML) deliberately
    ///     keep `{}`: there the flow chain is the idiomatic intermediate form,
    ///     and `fig fmt` canonicalizes `a = { b = { c = v }}` to `a.b.c = v`.
    ///
    ///   * YAML seeds with NOTHING (`""`) — a bare `key:`, i.e. a null value.
    ///     Both spellings are valid YAML for "no entries yet" but they are not
    ///     interchangeable as a SEED: a flow `{}` can only ever be extended
    ///     with flow members, so every block-spelled value landing under a
    ///     vivified ancestor had to be refused (`BlockValueIntoFlow`). A null
    ///     value has the opposite property — `insertKey` promotes it to a real
    ///     block mapping (`promoteNullToMapping`), which takes block and inline
    ///     values alike — so `set(a.b.c, 1)` produces the block containers YAML
    ///     is normally written in.
    ///
    ///   * INI, plist and NestedText declare null: they have no literal
    ///     spelling for "an empty nested mapping" that the generic seed could
    ///     use. INI's case is the sharpest — `{}` there is a two-character
    ///     STRING value, not a container, so seeding with it would write a
    ///     nonsense `section = {}` root key. This is an ABSENCE of a syntax,
    ///     which is why it collapses into this field rather than standing as
    ///     a separate "can vivify" flag: `empty_map_literal` has exactly one
    ///     consumer, inside `set`'s vivify branch, so null and "excluded from
    ///     vivify" are the same statement.
    empty_map_literal: ?[]const u8,

    /// Whether a BLOCK (non-flow) sequence can be edited in place.
    ///
    /// False for TOML alone: a non-flow TOML sequence is an array-of-tables,
    /// which `appendTableToArray` handles instead, and TOML has no block
    /// scalar array. Append/prepend/remove/reorder all refuse with
    /// `NotAnInlineArray` when this is false.
    block_seq_editable: bool = true,

    /// Whether a single line of the form `k: v` is a block MAPPING entry
    /// rather than scalar text — the one value shape that cannot be told
    /// apart by sniffing, so it is settled by the language's own parser.
    ///
    /// True for YAML alone: it is the only editable format whose block mapping
    /// has a single-line spelling reaching these splice paths. The flat
    /// formats (dotenv/`.properties`/INI) route through the same code, and
    /// there a `k: v` value is genuinely just scalar text — so they must keep
    /// splicing it inline, and declare false.
    single_line_block_mapping: bool = false,

    /// Whether a bare `key: value` document form exists — a keyless top-level
    /// mapping, as in YAML and JSON5.
    ///
    /// False for ZON alone, which has no such form: a null value, root or
    /// nested, promotes in place to a flow `.{ key = value }` container built
    /// from `flow_map_open`/`flow_map_close`, so `promoteNullToMapping` needs
    /// no root-versus-descend distinction there.
    bare_document_mapping: bool = true,

    /// The flow-mapping delimiters, used when promoting a null in a format
    /// with no `bare_document_mapping`. ZON's opener is `.{`.
    flow_map_open: []const u8 = "{",
    flow_map_close: []const u8 = "}",

    /// Whether a line's prefix is STRUCTURAL rather than whitespace.
    ///
    /// True for fig alone. Its `#`-only comment lines need the same `>`
    /// marker-run prefix as the line they anchor above — comment depth is
    /// load-bearing for attachment (see `fig/DESIGN.md`, "Comments") — so the
    /// "indent" a new comment copies is the raw byte range from the line start
    /// to the node's span, not the leading whitespace. `firstNonSpace` would
    /// stop at the `>` and yield bare whitespace, dropping the markers
    /// entirely. `span.start` already sits just past that prefix for every fig
    /// node (see `TNode.span` in `fig/parser.zig`), so slicing back to the line
    /// start recovers it exactly. Every other language's prefix is pure
    /// whitespace, where `firstNonSpace` and `span.start` agree anyway.
    structural_indent: bool = false,
};
