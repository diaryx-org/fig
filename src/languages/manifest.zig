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
//! can give as a value. The other half is how a format SPELLS a fragment the
//! engine cannot: the renderers, declared in the "Renderers" block of a
//! `<lang>/<lang>.zig` and dispatched by `@hasDecl` from `editor.zig`; they
//! need no types here, because each is a pure function from strings to a
//! string whose signature is fixed by the `Editor` method that calls it
//! (`Decls.renderers` in `language.zig` lists them).
//!
//! It also holds the shape of a format-registry entry, `Dialect`, since each
//! language now declares its own dialects (`Language.dialects`) and
//! `language.zig` only assembles them.
//!
//! This module is deliberately a LEAF: it imports nothing, not even `std`.
//! `language.zig` re-exports these types and every `<lang>/<lang>.zig` imports
//! them, so anything pulled in here would be pulled into all eleven language
//! modules — and an import back to `language.zig` (which imports each of them
//! in turn) would make the manifest's own types depend on the languages that
//! declare them.

/// Which leading-comment syntax a language uses, so the owned-comment scan in
/// delete/move (`editor/splice.zig`'s `commentBlockStart`) recognizes the right marker.
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
/// How one comment is delimited: the token that opens it and, for a paired
/// syntax, the one that closes it. `#`, `//` and `;` are an `open` alone;
/// plist's `<!-- … -->` is a pair. The engine writes a comment as
/// `open`, a space, the text, and — when `close` is non-empty — a space and
/// `close`; it strips the same shape when reading one back, and it finds a
/// trailing comment by searching for `open`. A pair used to be inexpressible
/// here, which is why plist hooked all six leading/trailing comment ops.
pub const CommentDelimiter = struct {
    open: []const u8,
    close: []const u8 = "",
    /// Text a comment body may not contain — `--` inside an XML comment —
    /// refused as `InvalidComment` before anything is spliced.
    forbidden: ?[]const u8 = null,
};

pub const Comments = struct {
    /// Selects the owned-comment-block scanner. See `CommentStyle`. Never
    /// null, and never redundant with `line`: the scanner is per-language
    /// while a marker is per-dialect, so strict JSON declares `.slashes`
    /// alongside a null marker — unobservable there, since no `//` line can
    /// exist for the scanner to find.
    style: CommentStyle,

    /// The own-line (leading) comment delimiter, or null when the dialect has
    /// none to write — strict JSON, where the comment ops return
    /// `CommentsUnsupported`. A paired delimiter (plist) serves the leading
    /// and trailing ops; the dangling and comment-out ops need a bare prefix
    /// and refuse a pair.
    line: ?CommentDelimiter,

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
    trailing: ?CommentDelimiter,

    /// `#` throughout — YAML, TOML, fig, dotenv, `.properties`.
    pub const hash: Comments = .{ .style = .hash, .line = .{ .open = "#" }, .trailing = .{ .open = "#" } };

    /// `//` throughout — ZON, which follows Zig.
    pub const slashes: Comments = .{ .style = .slashes, .line = .{ .open = "//" }, .trailing = .{ .open = "//" } };
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
    /// Bare when every byte is `[A-Za-z0-9_-]`, else a basic-quoted string
    /// with `"` and `\` escaped — TOML's key rule, used for a header path's
    /// segments and a renamed table's leaf as well as an inserted key.
    bare_or_quoted,
};

/// How a section format spells a header line that opens a container of its
/// own: `[` + path + `]` for a TOML table, `[[` + path + `]]` for an element
/// of an array of tables, an INI `[section]`. See `Syntax.section_header`.
pub const SectionHeader = struct {
    open: []const u8,
    close: []const u8,
    /// The element-of-a-sequence form, or null for a format without one.
    seq_open: ?[]const u8 = null,
    seq_close: ?[]const u8 = null,
    /// Joins the path's key segments, each rendered per `key_style`.
    sep: []const u8 = ".",
    /// Whether index segments are left out of the path — `[[a.b]]` always
    /// names `a`'s last element, so the index is implied.
    skip_index: bool = true,
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
    /// `Editor(Language)` is instantiated for this format. True for every
    /// language in tree since generic XML (a reader and a writer with no
    /// in-place editor) was removed in core 3.0; an out-of-tree `Language`
    /// may still declare false, and the CLI and C ABI refuse to edit it.
    edit: bool = false,
    /// `print` can write this format.
    serialize: bool = false,
    /// What the lossless `$fig` envelope pass (`lossless.zig`) may assume
    /// about this format's value model on OUTPUT, or null when the format
    /// takes no envelope at all.
    ///
    /// Non-null says: when `--lossless` targets this format, wrap every
    /// scalar kind NOT marked native in `NativeKinds` in a `$fig` envelope so
    /// a later run can rebuild it, and leave the marked kinds bare. Only the
    /// four typed formats with a real value model and a mapping to carry the
    /// envelope in — JSON, YAML, TOML, ZON — declare one.
    ///
    /// Null says: never encode an envelope into this format (envelopes in
    /// its INPUT are still decoded). Two distinct reasons collapse into the
    /// one answer, deliberately, because the pass has one behaviour for both:
    ///
    ///   * fig and canonical spell every kind directly, so an envelope would
    ///     preserve nothing a plain print does not.
    ///   * INI, dotenv, `.properties`, plist and NestedText have no typed
    ///     scalar envelope of their own — their printers already reduce the
    ///     value to text, so a mapping-shaped envelope would be no more
    ///     recoverable than the degraded scalar it replaced.
    ///
    /// A field on `Caps` rather than its own `Language` declaration because it
    /// IS a capability — "can fig round-trip a value through this format
    /// without loss, and which values need help" — and because the seven
    /// null answers then cost nothing to state: the default is the
    /// conservative one. It sits on the LANGUAGE (json/jsonc/json5 share it),
    /// which is why JSON5's native `Infinity`/`NaN` are still enveloped: the
    /// declaration is per-language and JSON's is the strict dialect's.
    lossless: ?NativeKinds = null,

    /// How many levels of mapping nesting this format can represent, or
    /// null for a format with no depth limit at all (every typed format).
    /// INI holds a root mapping plus one level of `[section]`s (1); dotenv
    /// and `.properties` are flat — the root mapping itself, nothing nested
    /// under it (0).
    ///
    /// A non-null value also says the format holds no sequence anywhere and
    /// no `null`: the three flat formats share that shape, and
    /// `flat_strip.zig` — the lossy pass that drops what such a format
    /// cannot hold before printing — and `diagnostics.zig`'s matching
    /// warning read this one field for the depth and take the rest as
    /// given. A future shallow format with sequences would need a second
    /// field, not a different reading of this one.
    max_mapping_depth: ?u8 = null,
};

/// The scalar kinds a format spells natively, beyond the core four every
/// serialize format has (boolean, string, number, and the two containers).
/// Read by `lossless.zig`, whose `needsEnvelope` is exactly "the kind is one
/// of these and the format did not mark it".
///
/// One field per kind the envelope can carry: `null`, plus one per
/// `AST.Node.Kind.Extended.ExtKind` member, named identically. This module is
/// a leaf and cannot name the AST's enum, so the correspondence is a
/// comptime pin in `lossless.zig` (both directions) rather than a type: a new
/// `ExtKind` fails the build until a field for it exists here, and a field
/// with no `ExtKind` behind it fails the same way. Every field defaults to
/// false — a format declares what it holds, and an omission is "envelope it",
/// which is always lossless if sometimes unidiomatic.
pub const NativeKinds = struct {
    /// A bare `null`. Every typed format but TOML has one; TOML's absence is
    /// the one kind the lossy path (`Lossless.lossyStrip`) DROPS rather than
    /// degrades, since there is no string to collapse it to.
    null: bool = false,
    /// The four RFC-3339-derived TOML datetimes.
    offset_datetime: bool = false,
    local_datetime: bool = false,
    local_date: bool = false,
    local_time: bool = false,
    /// ZON's `.name` and `'c'` literals.
    enum_literal: bool = false,
    char_literal: bool = false,
    /// A non-finite float (`inf`/`nan`, JSON5's `Infinity`/`NaN`).
    number_special: bool = false,
    /// plist's `<date>` and `<data>`.
    plist_date: bool = false,
    plist_data: bool = false,
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

/// `Lang.Type` when `Lang` is a language, `void` when it is the gated-out
/// placeholder.
///
/// A `-D<lang>=false` build resolves that language to `void` in
/// `language.zig`, and `void` has no `.Type` — so a field naming one directly
/// fails to compile in exactly the builds the flag exists to produce. Routing
/// the type through here keeps every dependent shape (a registry `Dialect`,
/// the CLI's `Spec`) identical in every build: the gated-out field becomes a
/// zero-bit `void` that nothing reads, because every consumer already sits
/// behind the same `build_options` test.
pub fn DialectOf(comptime Lang: type) type {
    return if (Lang == void) void else Lang.Type;
}

/// `Lang.default_type`, or the `void` value when `Lang` is gated out.
pub fn defaultDialect(comptime Lang: type) DialectOf(Lang) {
    return if (Lang == void) {} else Lang.default_type;
}

/// How a format takes the caller's edit text, which decides what the fix is
/// when the text turns out not to fit. The semantic `cli/diag_report.zig`'s
/// `spliceStyle` states (and which `cli/edit_ops.zig` acts on), declared once
/// per dialect beside everything else about it.
pub const SpliceStyle = enum {
    /// Spliced in verbatim as source, so a string value needs its own quotes —
    /// YAML, TOML, ZON, fig.
    literal,
    /// Wrapped as a JSON string first (`edit_ops.jsonifyEdit`), so `"`/`\` in
    /// the text are escaped rather than taken as syntax — the JSON family.
    json_string,
    /// Written as raw characters, so only the format's own separators can
    /// break it — INI, dotenv, `.properties`, plist, NestedText. (plist and
    /// NestedText *render* the text rather than splicing it.)
    raw,
};

/// One `--spec <version>` string and the dialect it selects. The element type
/// of `Dialect.specs`, generic over the language so a gated-out one collapses
/// to a `void` dialect and the registry still compiles (and still lists the
/// version STRINGS, which are build-invariant — `resolveSpec` rejects them
/// for a gated-out language rather than not knowing them).
pub fn SpecName(comptime Lang: type) type {
    return struct {
        /// The accepted `--spec` text, matched exactly. Several map to one
        /// dialect (`1.0` and `1.0.0` both select TOML 1.0).
        name: []const u8,
        dialect: DialectOf(Lang),
    };
}

/// One user-facing dialect of a language: everything about it that is not
/// the language module itself. A language declares its own as
/// `Language.dialects: []const Dialect(Language)` — one entry for most, three
/// for JSON (json/jsonc/json5 share one `Language`) — and `language.zig`
/// assembles the format registry from those tables, in language order.
///
/// Generic over the language so the `void` protocol survives (see
/// `DialectOf`): in the registry a gated-out language's entries are
/// `Dialect(void)`, still present with their names, ABI values and spellings,
/// so every enum derived from the registry is build-invariant. A language's
/// own table is always `Dialect(Language)` — it is the registry that lifts
/// the entries to the gated type.
pub fn Dialect(comptime L: type) type {
    return struct {
        /// The member name this dialect has in every derived enum, and —
        /// upper-cased — the `FIG_FORMAT_<NAME>` suffix in fig.h. Sentinel-
        /// terminated because a reified enum's field names must be. The
        /// entry that selects the language's `default_type` must be named
        /// `Language.name` (`validate` checks); the others are the language's
        /// business (JSON's `jsonc`/`json5`).
        name: [:0]const u8,

        /// The language this dialect is a dialect OF; `void` when that
        /// language is gated out of this build. Every consumer must test this
        /// FIRST — it is the gate, and reading any other `Lang`-derived field
        /// past a `void` is a compile error, which is the point.
        Lang: type = L,

        /// The `Lang.Type` value this dialect selects. Defaults to the
        /// language's own default; only the JSON trio overrides it.
        dialect: DialectOf(L) = defaultDialect(L),

        /// The `FigFormat` value in the C ABI. FROZEN: a released value can
        /// never change or be reused, so a new dialect takes the next unused
        /// integer (which is why these run 1,2,7 down the JSON family — JSON5
        /// arrived after generic XML took 6, a value that stays retired now
        /// that the format is gone). `zig build abi-check` compares these against
        /// fig.h's `FIG_FORMAT_*` enumerators in both directions, and
        /// `language.zig` refuses a duplicate.
        abi_value: c_int,

        /// Where this dialect sits in `Language.detect`'s probe order, or
        /// null for a dialect `detect` never sniffs (and which is then no
        /// member of `Detected`) — `jsonc` alone today, since it overlaps
        /// json/json5 on almost all input.
        ///
        /// The order is one argument about grammar overlap — strictest first,
        /// so a permissive grammar cannot claim what a stricter one would
        /// have accepted — and each row carries its own place in it with the
        /// reasoning beside the number. Ranks are unique across the registry
        /// (`language.zig` refuses a duplicate) and need not be contiguous;
        /// `language.zig` pins the resulting sequence in a test, so a new
        /// format choosing a rank cannot reorder the existing ones unnoticed.
        /// Every language must give at least one of its dialects a rank.
        sniff_rank: ?u8 = null,

        /// Whether `deserialize.Format` covers it — the typed
        /// struct-deserialization entry points, which today reach five of the
        /// thirteen dialects.
        deserializable: bool = false,

        /// How this dialect takes spliced edit text. See `SpliceStyle`.
        splice: SpliceStyle,

        /// The document `set` seeds when the target file does not exist yet
        /// (and what `Embed.initRegion` writes into a freshly created region),
        /// or null for a format that refuses to be created from scratch.
        ///
        /// An empty string is NOT the same statement as null: it means an
        /// empty file already parses as an empty root mapping, so the first
        /// key can just be inserted into it.
        empty_doc_seed: ?[]const u8,

        /// The `Lang.Printer` declaration that writes a whole document in this
        /// dialect, and the one that writes a single node. Two names rather
        /// than one because the JSON family shares a printer and separates its
        /// dialects by entry point (`print`/`printc`/`print5`), and YAML's
        /// document printer is `printWith`.
        ///
        /// These ARE the serializer's dispatch: `ast/serialize_options.zig`
        /// calls `@field(d.Lang.Printer, d.print_name)(writer, ast, options)`
        /// (and the `print_node_name` twin) for every entry, so a wrong name
        /// here is a compile error rather than a wrong output. There is no
        /// separate fragment name: `serializeFragmentWith` uses `print_name`
        /// for every dialect but fig, whose `printFragment` takes an explicit
        /// arm there for the reason documented on that function.
        print_name: [:0]const u8 = "print",
        print_node_name: [:0]const u8 = "printNode",

        /// The `--spec` strings this dialect accepts and what each selects.
        /// Empty for the eleven dialects with a single grammar. See
        /// `cli/parse_dispatch.zig`'s `resolveSpec`, whose behaviour a
        /// comptime assert beside it pins against the registry.
        specs: []const SpecName(L) = &.{},

        /// How this format spells itself inside a host document, or null when
        /// it has no embedded form (`Embed.InnerFormat` is REIFIED from
        /// exactly the entries where this is non-null). `embed.zig` builds
        /// every fence, frontmatter marker, `<script type>` and `<code class>`
        /// it writes — and every tag/MIME it accepts on read — out of these
        /// fields, so they are the spelling, not a description of it.
        embed: ?EmbedSpellings = null,
    };
}

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
    /// question". plist's entries are a pair of sibling ELEMENTS with no
    /// separator to write, and its `renderEntry` spells the whole entry;
    /// `language.validate` requires a null here to come with one, which is
    /// what makes every consumer below unreachable, and `editor.Editor.kvSep`
    /// is where that is cashed in. A format with a flow form whose separator
    /// varies by object (fig) declares its default here and
    /// `flow_kv_sep_from_siblings` beside it.
    kv_sep: ?[]const u8,

    /// Whether the flow-entry insert copies the separator the container's
    /// FIRST entry uses — the bytes between that entry's key and value —
    /// rather than writing `kv_sep`. True for fig, whose flow objects are
    /// either `=`-mode or `:`-mode (JSON-embedded) and may not mix the two,
    /// so the right separator is whichever the object already uses. An
    /// empty flow mapping has no first entry and takes `kv_sep`.
    flow_kv_sep_from_siblings: bool = false,

    /// Bytes written inside the braces around a freshly created single
    /// member of an EMPTY flow mapping: `" "` for fig's `{ x = 1 }`, none
    /// for JSON's and YAML's tight `{x: 1}`.
    flow_map_pad: []const u8 = "",

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

    /// Whether this format has FLOW container syntax at all — a `{…}` or
    /// `[…]` collection spelled inline, which the engine edits by comma-aware
    /// splice rather than by line.
    ///
    /// The engine tells flow from block by sniffing a container's first byte
    /// (`splice.isFlow`), and the sniff is right for every format that has
    /// both shapes. It is wrong for a format that has neither: INI's root
    /// span is the whole file, so a file opening with `[section]` reads as a
    /// bracket-delimited flow root, and NestedText's `- item` lines are not
    /// a flow sequence however a value happens to begin. Declaring false
    /// here answers "block" before the sniff runs. Every format with a flow
    /// spelling keeps the default; INI, NestedText, plist, dotenv and
    /// `.properties` declare false. See `editor.Editor.isFlowNode`, which
    /// also settles the one case a sniff cannot — a SECTION format's root,
    /// whose first byte is the first header's `[` — from `section_noun`.
    flow_containers: bool = true,

    /// The bytes one level of block nesting adds to a line's prefix: two
    /// spaces for YAML, four for NestedText, one `> ` marker cell for fig.
    ///
    /// The engine writes a block value that descends under a new entry
    /// (`key:` and then the value's lines) by copying the entry's own line
    /// prefix and appending this. It used to add two spaces to a column
    /// count, which is YAML's answer and nobody else's; a runtime format
    /// states its own. Read by `editor.Editor.writeMapValue` and
    /// `promoteNullToMapping`.
    indent_unit: []const u8 = "  ",

    /// What introduces a block-sequence item, separator included: `- ` for
    /// YAML and NestedText, `* ` for fig, `""` for plist, whose item is a
    /// bare element. The engine writes a new item as the first item's line
    /// prefix (the bytes before its marker — see `Document.node_marker_spans`)
    /// followed by this. Used to be a `"- "` literal in the engine.
    seq_item_marker: []const u8 = "- ",

    /// The tokens that open and close a block container, for a format whose
    /// block containers close themselves — plist's `<dict>`/`</dict>` and
    /// `<array>`/`</array>` — or null for every line-structured format, where
    /// a block collection has no closing token at all.
    ///
    /// Two things read it. The engine expands an EMPTY container of such a
    /// format (`<dict/>`) into its multi-line form around the first entry or
    /// item it inserts, since the childless form has no line to splice
    /// after; a format declaring null refuses that insert with
    /// `EmptyInlineContainer`, having no spelling for it. And a same-line
    /// trailing comment on a container value follows the close here, where
    /// in a line-structured format the value span begins at its first child
    /// on a later line and the comment rides the KEY's line (`contents: #
    /// note`). See `editor.Editor.expandEmptyContainer` and
    /// `trailingCommentWindow`.
    closed_containers: ?ClosedContainers = null,

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

    /// Declares this a SECTION format — one whose logical containers are
    /// assembled from lines scattered through the source (a TOML `[table]`,
    /// an INI `[section]`, a fig block container) — and names what the format
    /// calls such a container. Null for every format whose containers are
    /// contiguous, which is what the default says.
    ///
    /// Three things hang off a non-null value, all in `editor.zig`:
    ///
    ///   * the parser is expected to fill `Document.node_regions` with the
    ///     header lines of every such container (see that field), which is
    ///     what the generic whole-container ops and the line-splice guards
    ///     read — nothing here can check that the parser does, so the two
    ///     are a pair by contract rather than by `validate`;
    ///   * the generic `deleteContainer`/`moveContainer`/`reorderContainers`
    ///     are live, and refuse at comptime for a null;
    ///   * the engine's refusals are spelled in this vocabulary
    ///     (`CannotDeleteTable` for `.table`, `CannotDeleteSection` for
    ///     `.section`, …), so a format's own words survive in its errors.
    ///
    /// A VALUE rather than a hook because the engine's rule is the same for
    /// all three — "a section node cannot be line-spliced; use the container
    /// op" — and only the noun in the error differs.
    section_noun: ?SectionNoun = null,

    /// How a header line that opens a container of its own is spelled, for
    /// a section format that has one — TOML's `[a.b]` and `[[a.b]]`. With
    /// it the engine's `insertContainer` and `appendContainerToSeq` are
    /// live: they render the path through `key_style`, frame it with these
    /// tokens, and splice the line past the parent's whole extent. Null for
    /// a section format whose containers are not opened by a header line
    /// the engine could write on its own (fig's headers are bare dotted
    /// paths that `set` already creates; INI cannot vivify), and those ops
    /// refuse at comptime there as before.
    section_header: ?SectionHeader = null,

    /// The key that MERGES another mapping's entries into this one — YAML's
    /// `<<` — or null for a format with no such key. With it declared, a key
    /// the document resolves through a merge but never spells out is
    /// INHERITED: `replaceValAtPath` shadows it with a local entry
    /// (copy-on-write) and `deleteKey` refuses it with `MergeOnlyKey`, since
    /// there is no syntax to un-inherit one. The merge's resolution is core
    /// (`AST.mergedChild`); this field says whether the format has it.
    merge_key: ?[]const u8 = null,
};

/// An opening and closing token pair. See `Syntax.closed_containers`.
/// The five fragment renderers a format may declare — `Decls.renderers` in
/// `language.zig`, as an enum the engine can ask about. `Editor.hasRenderer`
/// answers for a compiled language from `@hasDecl` and for a runtime one
/// from the language's own `hasRenderer(t, which)`, since a vtable answers
/// with a null pointer rather than an absent declaration.
pub const Renderer = enum {
    value,
    entry,
    item,
    tail,
    key,

    /// The declaration name: `renderValue` for `.value`.
    pub fn declName(comptime self: Renderer) []const u8 {
        return switch (self) {
            .value => "renderValue",
            .entry => "renderEntry",
            .item => "renderItem",
            .tail => "renderTail",
            .key => "renderKey",
        };
    }
};

pub const Delimiters = struct { open: []const u8, close: []const u8 };

/// The self-closing block container spellings of a format whose containers
/// have them. See `Syntax.closed_containers`.
pub const ClosedContainers = struct { map: Delimiters, seq: Delimiters };

/// What a section format calls its scattered container — the one word that
/// differs between the three formats' otherwise identical refusals. See
/// `Syntax.section_noun`.
pub const SectionNoun = enum {
    /// TOML: `NotATable`, `CannotDeleteTable`, `CannotReplaceTable`,
    /// `CannotMoveTable`, `CannotReorderTables`.
    table,
    /// INI: `NotAContainer`, `CannotDeleteSection`, `CannotReplaceSection`,
    /// `CannotMoveSection`, `CannotReorderSections`.
    section,
    /// fig: `NotAContainer`, `CannotDeleteContainer`,
    /// `CannotReplaceContainer`, `CannotMoveContainer`,
    /// `CannotReorderContainers`.
    container,
};
