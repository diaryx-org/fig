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
/// Distinct from `Comments.line`: this selects the *scanner*, which is
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

/// A format's whole comment surface: which scanner walks an owned comment
/// block, and the markers the editor writes and strips.
///
/// One field on `Syntax` rather than three, because the three answers coincide
/// for most formats and restating the same marker three times reads as
/// redundancy rather than as the three independent questions it is. The two
/// presets below are exactly the "all three agree" case; a format whose
/// answers diverge — INI, NestedText, plist — writes the literal out, and so
/// does JSON, whose marker varies by dialect while its scanner does not.
pub const Comments = struct {
    /// Selects the owned-comment-block scanner. See `CommentStyle`. Never
    /// null, and never redundant with `line`: the scanner is per-language
    /// while a marker is per-dialect, so strict JSON declares `.slashes`
    /// alongside a null marker — unobservable there, since no `//` line can
    /// exist for the scanner to find.
    style: CommentStyle,

    /// The own-line (leading) comment marker, or null when the dialect has
    /// none to write — strict JSON, where the comment ops return
    /// `CommentsUnsupported`, and plist, whose `<!-- ... -->` is a delimiter
    /// pair with no leader (it hooks all six comment ops instead).
    line: ?[]const u8,

    /// The marker for a same-line TRAILING comment specifically, or null when
    /// the format has no such syntax.
    ///
    /// Distinct from `line` because INI and NestedText have real, safe leading
    /// comments but no trailing ones: a `;`/`#` after a value on the SAME line
    /// is literal value text, not a comment (see `ini/parser.zig`'s "a value
    /// runs to end of line" and `nestedtext/parser.zig`'s "rest-of-line values
    /// are 100% literal", and both printers, which render a "trailing" comment
    /// as its own line immediately after the entry). Splicing one in anyway
    /// would silently corrupt the value on reread, so trailing ops are refused
    /// there.
    trailing: ?[]const u8,

    /// `#` throughout — YAML, TOML, fig, dotenv, `.properties`.
    pub const hash: Comments = .{ .style = .hash, .line = "#", .trailing = "#" };

    /// `//` throughout — ZON, which follows Zig.
    pub const slashes: Comments = .{ .style = .slashes, .line = "//", .trailing = "//" };
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
/// The single source: `c_api.fig_format_capabilities` reads these bits rather
/// than restating them, so the C ABI cannot disagree with the format about what
/// the format can do. It used to be hand-maintained in both places, and drifted
/// silently in both directions — see the proposal's §7 and §12.
///
/// The MAPPING between the two is not spelled out anywhere either: `FigFormat`
/// is per-dialect (json/jsonc/json5 are three ABI values over this one
/// `Language`) while `caps` is per-language, and the format registry's `Lang`
/// field is the bridge — `fig_format_capabilities` looks the entry up by member
/// name and reads the bits off the language it names.
pub const Caps = struct {
    /// `parse` accepts this format. True for every language in tree.
    read: bool = true,
    /// `Editor(Language)` is instantiated for this format. False for XML,
    /// which has a reader and a writer but no in-place editor yet.
    edit: bool = false,
    /// `print` can write this format.
    serialize: bool = false,
};

/// How a format spells itself when it is EMBEDDED in a host document — the
/// four openers `embed.zig` writes, and the tags/MIMEs it accepts on read.
///
/// Plain data, and deliberately so: this module is a leaf (see the header),
/// and these are strings a format knows about itself, not behaviour. They sit
/// here rather than on `Language` because only four of the eleven formats have
/// an embedded spelling at all — a `Language` decl would either be optional
/// (and so invisible to `Decls`' closed set) or a lie for the other seven. The
/// registry entry in `language.zig` carries `?EmbedSpellings`, and null is the
/// statement that the format has no embedded form.
///
/// These fields ARE `embed.zig`'s spelling tables: its four literal builders
/// (`fencedLiteral`/`frontmatterLiteral`/`scriptLiteral`/`codeLiteral`) and its
/// two resolvers (`formatFromLangTag`/`formatFromScriptMime`) are one
/// `inline`-over-the-registry each, so a value changed here changes the bytes
/// a host document is written with and the spellings it is read back from.
pub const EmbedSpellings = struct {
    /// The ```` ```<tag> ```` info-string this format writes for a fenced
    /// block, WITHOUT the backticks — `embed.zig`'s `fencedLiteral` is
    /// ```` "```" ++ fence_tag ````. Also the canonical spelling
    /// `formatFromLangTag` resolves.
    fence_tag: []const u8,

    /// Extra `<tag>` spellings `formatFromLangTag` accepts for this format on
    /// READ but never writes — `yml` for YAML, `figl` for fig. Matched
    /// case-insensitively, like the canonical tag.
    fence_aliases: []const []const u8 = &.{},

    /// The WHOLE `---<lang>` frontmatter opener, not just the tag: YAML's is a
    /// bare `---` (the ecosystem default — an untagged frontmatter block IS
    /// YAML), while every other format tags it. The one field here that is a
    /// literal rather than a token, because that asymmetry has no token to
    /// carry it. `frontmatterLiteral` emits it verbatim.
    frontmatter: []const u8,

    /// The `type` attribute an `html_script` block is written with —
    /// `scriptLiteral` is `<script type="` ++ script_mime ++ `">`. Also
    /// `formatFromScriptMime`'s canonical arm.
    script_mime: []const u8,

    /// Extra `type` MIMEs `formatFromScriptMime` accepts on READ but never
    /// writes — `application/x-yaml`/`text/yaml`, `application/ld+json`,
    /// `application/fig`. Matched case-insensitively.
    script_mime_aliases: []const []const u8 = &.{},

    /// The `class` token an `html_code` block is written with —
    /// `codeLiteral` is `<pre><code class="` ++ code_class ++ `">`. Note fig's
    /// is `language-figl` while its fence tag is `fig`: the two spellings
    /// genuinely differ. On READ the class token's `language-`/`lang-`
    /// suffix is resolved by `formatFromLangTag`, so `fence_aliases` covers
    /// reading and this covers writing.
    code_class: []const u8,
};

/// Everything the generic splice engine needs to know about a format's
/// surface syntax, indexed by dialect.
///
/// Obtained as `Language.syntax(t)` rather than as a constant because
/// `comments.line` genuinely varies by dialect: strict JSON has no comment
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

    /// This format's comment scanner and markers. See `Comments`.
    comments: Comments,

    // ==================
    // ENTRIES
    // ==================

    /// The mapping key/value separator spliced by the generic flow-entry
    /// insert helpers (`insertFlowMapEntry`/`insertFlowEntry`) and by
    /// `writeMapValue`'s block-insert path — or null for a format that owns
    /// every one of those paths itself and so has no answer to give.
    ///
    /// This is the separator the GENERIC engine writes, which need not be the
    /// separator the format's printer writes: ZON's struct-field syntax is
    /// ` = `, dotenv/`.properties` print a bare `=` with no surrounding
    /// spaces, INI always pads it. See each `printer.zig`.
    ///
    /// The null is not "no separator" — it is "not the generic engine's
    /// question". fig and TOML spell an entry `key = value` but hook
    /// `insertKey` and decide the separator from the source there (fig's flow
    /// objects are `=`-mode or `:`-mode and may not mix, so there is no one
    /// answer to declare); plist's entries are a pair of sibling ELEMENTS and
    /// NestedText's are `key:` lines its own helper writes. All four used to
    /// declare `": "` — a value no code read, and for fig one its own parser
    /// rejects (`FigFlowBareKeyColon`). `language.validate` requires a null
    /// here to come with an `insertKey` hook, which is what makes every
    /// consumer below unreachable; `editor.Editor.kvSep` is where that is
    /// cashed in.
    kv_sep: ?[]const u8,

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
    /// which `appendContainerToSeq` handles instead, and TOML has no block
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
