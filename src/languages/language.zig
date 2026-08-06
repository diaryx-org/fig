const std = @import("std");
const Allocator = std.mem.Allocator;
const build_options = @import("build_options");
const manifest = @import("manifest.zig");

pub const Language = @This();

// The declared half of the interface, re-exported so a caller needs only this
// file. The definitions live in `manifest.zig` because it is a leaf — every
// `<lang>/<lang>.zig` imports it to spell its own `syntax`, and this file
// imports each of them in turn, so the types cannot live here without the
// manifest depending on the languages that declare it.
pub const CommentStyle = manifest.CommentStyle;
pub const KeyStyle = manifest.KeyStyle;
pub const Caps = manifest.Caps;
pub const Syntax = manifest.Syntax;

// Per-language gates: a compiled-out format resolves to `void`, so its module is
// never referenced and never built. Every call site that touches a gated
// `Language.*` must guard the access behind the same `build_options.lang_*`
// flag (a `comptime` check), or it will fail to compile against `void`. JSON is
// gateable like the rest now that `detect` no longer assumes it as a base.
pub const JSON = if (build_options.lang_json) @import("json/json.zig").Language else void;
pub const YAML = if (build_options.lang_yaml) @import("yaml/yaml.zig").Language else void;
pub const TOML = if (build_options.lang_toml) @import("toml/toml.zig").Language else void;
pub const ZON = if (build_options.lang_zon) @import("zon/zon.zig").Language else void;
pub const XML = if (build_options.lang_xml) @import("xml/xml.zig").Language else void;
pub const FIG = if (build_options.lang_fig) @import("fig/fig.zig").Language else void;
pub const INI = if (build_options.lang_ini) @import("ini/ini.zig").Language else void;
pub const DOTENV = if (build_options.lang_dotenv) @import("dotenv/dotenv.zig").Language else void;
pub const PROPERTIES = if (build_options.lang_properties) @import("properties/properties.zig").Language else void;
pub const PLIST = if (build_options.lang_plist) @import("plist/plist.zig").Language else void;
pub const NESTEDTEXT = if (build_options.lang_nestedtext) @import("nestedtext/nestedtext.zig").Language else void;

// ============================================================================
// THE FORMAT REGISTRY
// ============================================================================
//
// `compiled` (below) is the per-LANGUAGE list. This is the per-DIALECT one: the
// table the five hand-written parallel format enumerations — `Detected` here,
// `cli.Format`, `AST.SerializeFormat`, `c_api.FigFormat`, `deserialize.Format`,
// `Embed.InnerFormat` — are all restatements of, plus the per-dialect facts
// (ABI value, splice style, empty-document seed, `--spec` strings, embedded
// spellings) that today live scattered across six files as switches nothing
// cross-checks.
//
// As of this stage NOTHING CONSUMES IT except `cli/args.zig`'s extension table.
// What it does instead is ASSERT: the `comptime` block after `dialects` and its
// siblings in `cli/types.zig`, `c_api.zig`, `ast/serialize_options.zig`,
// `deserialize.zig` and `embed.zig` fail the build the moment the registry and
// the hand-written enum they pin disagree. The derivations that make those
// enums *reifications* of this table arrive in Stages 3-7.

/// `Lang.Type` when `Lang` is compiled in, `void` when it is gated out.
///
/// A `-D<lang>=false` build resolves that language to `void` above, and `void`
/// has no `.Type` — so a field naming one directly fails to compile in exactly
/// the builds the flag exists to produce. Routing the type through here keeps
/// every dependent shape (a registry `Entry`, the CLI's `Spec`) identical in
/// every build: the gated-out field becomes a zero-bit `void` that nothing
/// reads, because every consumer already sits behind the same `build_options`
/// test. Moved here from `cli/parse_dispatch.zig`, which now re-exports it —
/// the registry needs it one layer below the CLI.
pub fn DialectOf(comptime Lang: type) type {
    return if (Lang == void) void else Lang.Type;
}

/// `Lang.default_type`, or the `void` value when `Lang` is gated out.
pub fn defaultDialect(comptime Lang: type) DialectOf(Lang) {
    return if (Lang == void) {} else Lang.default_type;
}

/// A named dialect of `Lang` spelled by its member NAME rather than by a
/// literal, so a registry entry can name one in a build where `Lang` is `void`
/// (there is no enum to write `.JSONC` against). Collapses to the `void` value
/// exactly when the language is gated out.
pub fn dial(comptime Lang: type, comptime tag: []const u8) DialectOf(Lang) {
    return if (Lang == void) {} else @field(Lang.Type, tag);
}

/// How a format takes the caller's edit text, which decides what the fix is
/// when the text turns out not to fit. The semantic `cli/diag_report.zig`'s
/// `spliceStyle` states today (and which `cli/edit_ops.zig` acts on), lifted
/// here so it is declared once per dialect beside everything else about it.
pub const SpliceStyle = enum {
    /// Spliced in verbatim as source, so a string value needs its own quotes —
    /// YAML, TOML, ZON, fig.
    literal,
    /// Wrapped as a JSON string first (`edit_ops.jsonifyEdit`), so `"`/`\` in
    /// the text are escaped rather than taken as syntax — the JSON family.
    json_string,
    /// Written as raw characters, so only the format's own separators can
    /// break it — INI, dotenv, `.properties`, XML, plist, NestedText. (plist
    /// and NestedText *render* the text rather than splicing it; XML has no
    /// in-place editor at all, so no edit text ever reaches it.)
    raw,
};

/// One `--spec <version>` string and the dialect it selects. The element type
/// of `Entry.specs`, generic over the language so a gated-out one collapses to
/// a `void` dialect and the table still compiles (and still lists the version
/// STRINGS, which are build-invariant — `resolveSpec` rejects them for a
/// gated-out language rather than not knowing them).
pub fn SpecName(comptime Lang: type) type {
    return struct {
        /// The accepted `--spec` text, matched exactly. Several map to one
        /// dialect (`1.0` and `1.0.0` both select TOML 1.0).
        name: []const u8,
        dialect: DialectOf(Lang),
    };
}

/// One user-facing dialect: everything about it that is not the language
/// module itself. Generic over the language so the `void` protocol survives —
/// see `DialectOf`.
fn Entry(comptime L: type) type {
    return struct {
        /// The member name this dialect has in every derived enum, and —
        /// upper-cased — the `FIG_FORMAT_<NAME>` suffix in fig.h. Sentinel-
        /// terminated because a reified enum's field names must be.
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
        /// never change or be reused, so new entries append (which is why
        /// these run 1,2,7 down the JSON family — JSON5 arrived after XML).
        /// `zig build abi-check` compares these against fig.h's
        /// `FIG_FORMAT_*` enumerators in both directions.
        abi_value: c_int,

        /// Whether `detect` can sniff this dialect, i.e. whether it is a
        /// member of `Detected`. False for `jsonc` alone, which overlaps
        /// json/json5 on almost all input.
        detectable: bool = true,

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

        /// The `Printer` declaration that writes a whole document in this
        /// dialect, and the one that writes a single node. Two names rather
        /// than one because the JSON family shares a printer and separates its
        /// dialects by entry point (`print`/`printc`/`print5`), and YAML's
        /// document printer is `printWith`. Consumed in Stage 5, when
        /// `Printer` joins the `Language` interface.
        print_name: [:0]const u8 = "print",
        print_node_name: [:0]const u8 = "printNode",

        /// The `--spec` strings this dialect accepts and what each selects.
        /// Empty for the eleven dialects with a single grammar. See
        /// `cli/parse_dispatch.zig`'s `resolveSpec`, whose behaviour a
        /// comptime assert beside it pins against this table.
        specs: []const SpecName(L) = &.{},

        /// How this format spells itself inside a host document, or null when
        /// it has no embedded form (`Embed.InnerFormat` is exactly the four
        /// entries where this is non-null). Consumed in Stage 6.
        embed: ?manifest.EmbedSpellings = null,
    };
}

/// EVERY user-facing dialect, as a heterogeneous comptime tuple — thirteen
/// entries over eleven languages (the JSON module supplies three).
///
/// Two properties of this table are frozen, and both are load-bearing:
///
///   * ORDER. It is the member order of every enum derived from it in Stages
///     3-7 (`Detected`, `cli.Format`, `SerializeFormat`, …), and reproduces
///     today's `cli.Format` order minus its three non-registry members (`yml`,
///     an alias removed in Stage 3; `canonical`, which is not a `Language` at
///     all; `gron`, a CLI-only projection). Reordering it would silently
///     renumber `@intFromEnum` for every one of those enums. Append.
///
///   * `abi_value`. It is the C ABI, and a released value is permanent.
///
/// Entries are ALWAYS present — a gated-out language collapses its entry's
/// `Lang` to `void` rather than dropping the row — so every derived enum is
/// build-invariant and only the *behaviour* behind a member is gated.
///
/// `canonical` and `gron` are deliberately absent: canonical is the AST's own
/// oracle grammar (no `Language`, no dialect, an options-less printer) and
/// gron is a CLI-only projection of JSON. Both stay explicit named arms at
/// every switch, which is also what keeps an exhaustive switch honest — a new
/// member has to be either a registry entry or one of those two.
pub const dialects = .{
    Entry(JSON){
        .name = "json",
        .dialect = dial(JSON, "JSON"),
        .abi_value = 1,
        .deserializable = true,
        .splice = .json_string,
        .empty_doc_seed = "{}\n",
        .embed = .{
            .fence_tag = "json",
            .frontmatter = "---json",
            .script_mime = "application/json",
            .script_mime_aliases = &.{"application/ld+json"},
            .code_class = "language-json",
        },
    },
    Entry(JSON){
        .name = "jsonc",
        .dialect = dial(JSON, "JSONC"),
        .abi_value = 2,
        // The one non-detectable dialect: plain JSON and JSON5 already claim
        // everything JSONC accepts that they can parse, so sniffing it would
        // only ever mis-attribute a comment-free document.
        .detectable = false,
        .deserializable = true,
        .splice = .json_string,
        .empty_doc_seed = "{}\n",
        .print_name = "printc",
        .print_node_name = "printNodec",
    },
    Entry(JSON){
        .name = "json5",
        .dialect = dial(JSON, "JSON5"),
        .abi_value = 7,
        .splice = .json_string,
        .empty_doc_seed = "{}\n",
        .print_name = "print5",
        .print_node_name = "printNode5",
    },
    Entry(YAML){
        .name = "yaml",
        .abi_value = 3,
        .deserializable = true,
        .splice = .literal,
        // A bare `key:` seed, not `{}`: see `Syntax.empty_map_literal`'s note
        // on why an empty YAML document is the empty string.
        .empty_doc_seed = "",
        .print_name = "printWith",
        .specs = &.{
            .{ .name = "1.2", .dialect = dial(YAML, "v1_2_2") },
            .{ .name = "1.2.2", .dialect = dial(YAML, "v1_2_2") },
            .{ .name = "1.1", .dialect = dial(YAML, "v1_1") },
            .{ .name = "1.1.0", .dialect = dial(YAML, "v1_1") },
        },
        .embed = .{
            .fence_tag = "yaml",
            .fence_aliases = &.{"yml"},
            // Bare, not `---yaml`: an untagged frontmatter block is YAML.
            .frontmatter = "---",
            .script_mime = "application/yaml",
            .script_mime_aliases = &.{ "application/x-yaml", "text/yaml" },
            .code_class = "language-yaml",
        },
    },
    Entry(TOML){
        .name = "toml",
        .abi_value = 4,
        .deserializable = true,
        .splice = .literal,
        .empty_doc_seed = "",
        .specs = &.{
            .{ .name = "1.0", .dialect = dial(TOML, "TOML_1_0") },
            .{ .name = "1.0.0", .dialect = dial(TOML, "TOML_1_0") },
            .{ .name = "1.1", .dialect = dial(TOML, "TOML_1_1") },
            .{ .name = "1.1.0", .dialect = dial(TOML, "TOML_1_1") },
        },
        .embed = .{
            .fence_tag = "toml",
            .frontmatter = "---toml",
            .script_mime = "application/toml",
            .code_class = "language-toml",
        },
    },
    Entry(ZON){
        .name = "zon",
        .abi_value = 5,
        .deserializable = true,
        .splice = .literal,
        .empty_doc_seed = ".{}\n",
    },
    Entry(XML){
        .name = "xml",
        .abi_value = 6,
        // XML has a reader and a writer but no in-place editor, so no edit
        // text ever reaches a splice; `.raw` is what `spliceStyle` says today.
        .splice = .raw,
        // No from-scratch creation: a bare XML document needs a root element
        // this layer cannot name.
        .empty_doc_seed = null,
    },
    Entry(FIG){
        .name = "fig",
        .abi_value = 8,
        .splice = .literal,
        .empty_doc_seed = "",
        .embed = .{
            .fence_tag = "fig",
            .fence_aliases = &.{"figl"},
            .frontmatter = "---fig",
            .script_mime = "application/figl",
            .script_mime_aliases = &.{"application/fig"},
            // `language-figl`, not `language-fig`: the class token and the
            // fence tag genuinely differ in `embed.zig` today.
            .code_class = "language-figl",
        },
    },
    Entry(INI){
        .name = "ini",
        .abi_value = 9,
        .splice = .raw,
        .empty_doc_seed = "",
    },
    Entry(DOTENV){
        .name = "dotenv",
        .abi_value = 10,
        .splice = .raw,
        .empty_doc_seed = "",
    },
    Entry(PROPERTIES){
        .name = "properties",
        .abi_value = 11,
        .splice = .raw,
        .empty_doc_seed = "",
    },
    Entry(PLIST){
        .name = "plist",
        .abi_value = 12,
        .splice = .raw,
        // DELIBERATE DEVIATION from `cli/edit_ops.zig`'s `emptyDocSeed`, which
        // returns null for plist today — so `fig set` on a nonexistent
        // `.plist` refuses instead of creating one. A bare `<dict>` IS a
        // document `Language.PLIST` parses (see its `detect` probe), so the
        // registry declares the seed the fix needs. NOTHING READS IT YET: the
        // switch is converted in Stage 4, which is where the behaviour change
        // and its CLI test land. The assert beside `emptyDocSeed` exempts this
        // one row for exactly that reason.
        .empty_doc_seed = "<dict>\n</dict>\n",
    },
    Entry(NESTEDTEXT){
        .name = "nestedtext",
        .abi_value = 13,
        .splice = .raw,
        .empty_doc_seed = "",
    },
};

/// The registry entry named `name`, or a compile error naming the format that
/// has none. The lookup every derived dispatch arm opens with.
pub fn entryFor(comptime name: []const u8) EntryOf(name) {
    inline for (dialects) |d| {
        if (comptime std.mem.eql(u8, d.name, name)) return d;
    }
    unreachable; // `EntryOf` already failed the build for an unknown name
}

/// The `Entry(L)` instantiation `entryFor(name)` returns — its own function
/// because each entry is a DIFFERENT type (they are generic over the language),
/// so the return type has to be computed from the name.
fn EntryOf(comptime name: []const u8) type {
    inline for (dialects) |d| {
        if (std.mem.eql(u8, d.name, name)) return @TypeOf(d);
    }
    @compileError("no registry entry for format '" ++ name ++ "'");
}

/// Which registry entries a derived enum is built from.
pub const Selector = enum {
    /// All thirteen.
    all,
    /// `.detectable` — `Language.Detected`.
    detectable,
    /// `.deserializable` — `deserialize.Format`.
    deserializable,
    /// `.embed != null` — `Embed.InnerFormat`.
    embeddable,
};

/// The names of the entries `sel` selects, in registry order. The expected
/// member list of the enum each selector names.
pub fn namesOf(comptime sel: Selector) []const [:0]const u8 {
    comptime {
        var out: []const [:0]const u8 = &.{};
        for (dialects) |d| {
            const take = switch (sel) {
                .all => true,
                .detectable => d.detectable,
                .deserializable => d.deserializable,
                .embeddable => d.embed != null,
            };
            if (take) out = out ++ [_][:0]const u8{d.name};
        }
        return out;
    }
}

/// Fail the build unless `E`'s members are exactly `want` (in `want`'s order)
/// plus `extra` (which may sit anywhere, and must all be present). `what`
/// names the enum in the message.
///
/// The shape every "this enum is a restatement of the registry" assert needs:
/// order matters for the members that come FROM the registry, because that
/// order becomes theirs when the enum is reified, while the deliberate
/// non-registry members (`canonical`, `gron`, `yml`) are positioned by hand and
/// only have to still exist.
pub fn assertDerivedEnum(
    comptime E: type,
    comptime want: []const [:0]const u8,
    comptime extra: []const []const u8,
    comptime what: []const u8,
) void {
    comptime {
        @setEvalBranchQuota(20_000);
        var seen_extra = [_]bool{false} ** extra.len;
        var i: usize = 0;
        for (@typeInfo(E).@"enum".fields) |f| {
            var is_extra = false;
            for (extra, 0..) |x, xi| {
                if (std.mem.eql(u8, x, f.name)) {
                    seen_extra[xi] = true;
                    is_extra = true;
                }
            }
            if (is_extra) continue;
            if (i == want.len)
                @compileError(what ++ " has the member '" ++ f.name ++ "' after the last" ++
                    " registry entry — add it to `language.zig`'s `dialects`, or declare it" ++
                    " a deliberate non-registry member at this assert");
            if (!std.mem.eql(u8, want[i], f.name))
                @compileError(what ++ " member '" ++ f.name ++ "' sits where registry entry '" ++
                    want[i] ++ "' does — the registry's ORDER is the member order every" ++
                    " derived enum inherits, so the two cannot diverge");
            i += 1;
        }
        if (i != want.len)
            @compileError(what ++ " has no member for the registry entry '" ++ want[i] ++ "'");
        for (extra, seen_extra) |x, s| {
            if (!s) @compileError(what ++ " no longer has the non-registry member '" ++ x ++
                "' this assert exempts — drop it from the exemption list");
        }
    }
}

/// `assertDerivedEnum` without the ordering claim: the member SET must match,
/// the order is the enum's own business. For the two enums whose order is
/// deliberately not the registry's (`c_api.FigFormat`, ordered by ABI history;
/// `Embed.InnerFormat`, internal-only and reordered in Stage 6).
pub fn assertEnumMembers(
    comptime E: type,
    comptime want: []const [:0]const u8,
    comptime what: []const u8,
) void {
    comptime {
        @setEvalBranchQuota(20_000);
        for (@typeInfo(E).@"enum".fields) |f| {
            var found = false;
            for (want) |w| {
                if (std.mem.eql(u8, w, f.name)) found = true;
            }
            if (!found)
                @compileError(what ++ " has the member '" ++ f.name ++
                    "' with no format-registry entry of that name");
        }
        for (want) |w| {
            if (!@hasField(E, w))
                @compileError(what ++ " has no member for the registry entry '" ++ w ++ "'");
        }
    }
}

// The registry's self-consistency, plus the one derived enum that lives in this
// file. The cross-module pins — `cli.Format`, `c_api.FigFormat`,
// `AST.SerializeFormat`, `deserialize.Format`, `Embed.InnerFormat` — cannot be
// written here (this file sits BELOW all five, and reaching up would invert the
// dependency), so each lives beside the enum it pins and calls the helpers
// above.
comptime {
    @setEvalBranchQuota(50_000);

    // Names and ABI values are both identities: a duplicate of either would
    // make a derived enum ill-formed (two members of one name) or the C ABI
    // ambiguous (two formats answering to one integer).
    for (namesOf(.all), 0..) |a, ai| {
        for (namesOf(.all)[ai + 1 ..]) |b| {
            if (std.mem.eql(u8, a, b))
                @compileError("two format-registry entries are both named '" ++ a ++ "'");
        }
    }
    for (dialects, 0..) |a, ai| {
        for (dialects, 0..) |b, bi| {
            if (bi > ai and a.abi_value == b.abi_value)
                @compileError("format-registry entries '" ++ a.name ++ "' and '" ++ b.name ++
                    "' share the ABI value " ++ std.fmt.comptimePrint("{d}", .{a.abi_value}) ++
                    " — released ABI values are permanent and unique");
        }
    }

    // Language ↔ registry bijection, both directions. A compiled-in language
    // with no entry would be a format the derived enums cannot name; an entry
    // whose (non-gated) language is not compiled in would be a member nothing
    // can serve.
    for (compiled) |Lang| {
        var found = false;
        for (dialects) |d| {
            if (d.Lang == Lang) found = true;
        }
        if (!found)
            @compileError("the compiled-in language '" ++ Lang.name ++
                "' has no entry in `dialects`, so no derived format enum can name it");
    }
    for (dialects) |d| {
        if (d.Lang == void) continue;
        var found = false;
        for (compiled) |Lang| {
            if (d.Lang == Lang) found = true;
        }
        if (!found)
            @compileError("format-registry entry '" ++ d.name ++
                "' names a language missing from `compiled`");
    }

    // Each entry's dialect. The JSON trio is the whole reason `dialect` is a
    // field rather than always `default_type`; everything else must BE the
    // language's default, which is what every current call site passes.
    for (dialects) |d| {
        if (d.Lang == void) continue;
        const expected = if (std.mem.eql(u8, d.name, "jsonc"))
            dial(d.Lang, "JSONC")
        else if (std.mem.eql(u8, d.name, "json5"))
            dial(d.Lang, "JSON5")
        else
            defaultDialect(d.Lang);
        if (d.dialect != expected)
            @compileError("format-registry entry '" ++ d.name ++
                "' selects a dialect other than the one its call sites pass today");
    }

    // `entryFor` itself, which nothing else calls until Stage 3 — an unused
    // comptime function is an unanalyzed one, and a lookup that does not
    // compile is not a lookup the next stage can build a dispatch idiom on.
    if (entryFor("json").abi_value != 1 or entryFor("nestedtext").abi_value != 13)
        @compileError("`entryFor` does not return the entry it was asked for");

    // LAST, deliberately: `Detected` is the one derived enum living in this
    // file, and a registry that is internally inconsistent (a missing entry, a
    // duplicated ABI value) would fail this assert too — with a message about
    // `Detected` rather than about the registry. Checking the table's own
    // coherence first means the error names the actual mistake.
    assertDerivedEnum(Detected, namesOf(.detectable), &.{}, "Language.Detected");
}

/// A format `detect` can recognize. The `jsonc` dialect and `canonical` are
/// deliberately excluded: jsonc overlaps json/json5 on most input, and
/// canonical is an explicit selection rather than something to sniff. `fig`
/// IS included, but slotted just ahead of YAML (see the ordering note on
/// `detect`) since its grammar overlaps TOML/YAML on plain `key = value`
/// content — it only wins detection on input that is either invalid for
/// every stricter format, or uses fig-only structural syntax (`>` section
/// depth, `*` elements, `+` continuations, `[]` group headers).
pub const Detected = enum { json, json5, yaml, toml, zon, xml, fig, ini, dotenv, properties, plist, nestedtext };

/// Best-effort content sniffing: try each COMPILED-IN parser and return the
/// first that accepts `input`, or null if none do (also what an
/// all-languages-disabled build returns). Order matters because the grammars
/// overlap — from most to least strict: JSON/JSON5, ZON, XML, TOML, then fig,
/// then INI, then YAML. fig sits just before INI/YAML, not after: YAML is so
/// permissive (a bare line is a valid plain scalar) that almost anything falls
/// through to it, which would starve fig (and INI) of a turn if it went last.
/// fig itself overlaps TOML heavily (both accept plain `key = value`), so it is
/// tried only after TOML has had first claim — a plain TOML-shaped document
/// still resolves to `.toml`, and fig only wins on content TOML can't parse
/// (its `>`/`*`/`+`/`[]` structural markers) or that is otherwise TOML-invalid.
/// INI overlaps TOML/fig too (same `[section]`/`key = value` shape) but accepts
/// strictly more — any raw, unquoted value text — so it sits right after fig
/// and wins only what both of those reject. dotenv sits last of the four
/// key/value-shaped formats since INI's grammar shadows almost all of it too
/// (see the `dotenv` branch below for the one thing that doesn't). This is a
/// heuristic, not a proof: input valid as more than one format resolves to
/// the earliest candidate in this order.
pub fn detect(allocator: Allocator, input: []const u8) ?Detected {
    if (comptime build_options.lang_json) {
        if (tryParse(JSON, allocator, input, .JSON)) return .json;
        if (tryParse(JSON, allocator, input, .JSON5)) return .json5;
    }
    if (comptime build_options.lang_zon) {
        if (tryParse(ZON, allocator, input, ZON.default_type)) return .zon;
    }
    if (comptime build_options.lang_plist) {
        // plist's DTD vocabulary (`<dict>`/`<array>`/`<key>`/...) is a STRICT
        // SUBSET of well-formed XML: the generic XML reader below would also
        // happily accept any real plist document, just folding it into a
        // differently-shaped AST (attribute/`#text` folding, no typed
        // scalars). So plist must get first claim, or a compiled-in XML
        // reader would starve it completely — the reverse isn't a problem:
        // plist's own grammar rejects anything outside its fixed element
        // vocabulary (`error.UnknownElement`), so ordinary XML falls through
        // to the `.xml` branch below untouched.
        if (tryParse(PLIST, allocator, input, PLIST.default_type)) return .plist;
    }
    if (comptime build_options.lang_xml) {
        if (tryParse(XML, allocator, input, XML.default_type)) return .xml;
    }
    if (comptime build_options.lang_toml) {
        if (tryParse(TOML, allocator, input, TOML.default_type)) return .toml;
    }
    if (comptime build_options.lang_fig) {
        if (tryParse(FIG, allocator, input, FIG.default_type)) return .fig;
    }
    if (comptime build_options.lang_ini) {
        // INI's grammar is also permissive (a bare `key = value` line, or an
        // empty file, both parse), so it's tried only after everything
        // stricter above has had first claim — it wins only on content those
        // reject, e.g. a `[section]` header or an unquoted value with
        // characters no TOML/fig scalar allows (`path = C:\a\b`).
        if (tryParse(INI, allocator, input, INI.default_type)) return .ini;
    }
    if (comptime build_options.lang_dotenv) {
        // dotenv is almost entirely shadowed by INI above: INI's key scanner
        // accepts any non-`=`/newline run (so even `export FOO=bar` parses as
        // one weird INI key) and its value decoding is quote-agnostic, so
        // nearly anything dotenv accepts, INI already claimed first. The one
        // thing only dotenv parses — a `"`/`'`-quoted value spanning a literal
        // embedded newline (INI's value never crosses a physical line) — is
        // this branch's actual reason to exist; `.env`'s real path to
        // selection is its extension (`detectLanguageFromFileEnding`
        // special-cases the `env` extension), not this content sniff.
        if (tryParse(DOTENV, allocator, input, DOTENV.default_type)) return .dotenv;
    }
    if (comptime build_options.lang_yaml) {
        if (tryParse(YAML, allocator, input, YAML.default_type)) return .yaml;
    }
    if (comptime build_options.lang_properties) {
        // `.properties` is even more permissive than YAML: a line with no
        // separator at all is still legal (a bare key, empty value — see
        // `properties/tokenizer.zig`), so nearly any UTF-8 text parses. It
        // therefore sits LAST, after YAML — the one thing this format
        // accepts that YAML rejects outright is a malformed-YAML shape
        // (see the test below); `.properties`'s real path to selection is
        // its extension, same as `.env`.
        if (tryParse(PROPERTIES, allocator, input, PROPERTIES.default_type)) return .properties;
    }
    if (comptime build_options.lang_nestedtext) {
        // NestedText goes LAST, after even `.properties` — not because its
        // own grammar is unusually permissive (it isn't: keys/values have
        // real restrictions, unlike `.properties`'s "nearly any text"), but
        // because a huge, ordinary swath of it — plain `key: value` lines
        // and `- item` lists — is ALSO valid YAML, and parses to a MEANINGFULLY
        // DIFFERENT tree there (YAML types `port: 80` as an integer;
        // NestedText's `port` is the untyped string `"80"`). Trying this
        // before YAML would silently change what today's `detect()` returns
        // for ordinary plain-YAML content already relied upon elsewhere in
        // this codebase — a real regression, not just an academic ambiguity
        // — so NestedText only gets a turn once every stricter-or-equally-
        // plausible format (including YAML) has already rejected the input.
        // Its real path to selection is the `.nt` extension (see
        // `cli/args.zig`), exactly like dotenv/`.properties` above.
        if (tryParse(NESTEDTEXT, allocator, input, NESTEDTEXT.default_type)) return .nestedtext;
    }
    return null;
}

/// Parse with `Lang` and report only whether it succeeded, releasing the document
/// either way. The detection probe — content is parsed, never retained.
fn tryParse(comptime Lang: type, allocator: Allocator, input: []const u8, t: Lang.Type) bool {
    const doc = Lang.Parser.parse(allocator, input, t) catch return false;
    doc.deinit(allocator);
    return true;
}

/// Every declaration a `Language` may carry. `validate` rejects anything not
/// named here, which is the whole point of the list: `@hasDecl` dispatch is
/// silent about names it does not recognize, so without a closed set an author
/// who writes `insertkey` gets a format that COMPILES, quietly runs the generic
/// implementation it meant to override, and corrupts a file on the first edit.
/// That was reproduced on the tree, not imagined — see the proposal's §10.5.
///
/// Adding a hook to `editor.zig` means adding its name here too. That is the
/// deliberate cost of the check, and the compiler charges it immediately: a
/// hook the list does not know is a hook no format can declare.
const Decls = struct {
    /// Required of every format, editable or not.
    const required = [_][]const u8{
        "Type",         "Parser", "default_type", "parse", "print",
        "name", "extensions",     "caps",
    };

    /// Required of an editable format only. `syntax` describes how the generic
    /// splice engine writes this format; asking a read-only format for one is
    /// asking it to describe an editing surface it does not have.
    const required_edit = [_][]const u8{"syntax"};

    /// Permitted, not required.
    ///
    ///   * `printNode` — every format but plist and xml, whose `print` is
    ///     written inline.
    ///   * `materialize`/`TagMode` — YAML only: collapsing the reference layer
    ///     before a non-YAML printer sees the tree. Callers already gate on
    ///     `@hasDecl(Lang, "materialize")`.
    const optional = [_][]const u8{ "printNode", "materialize", "TagMode" };

    /// Editing hooks. Declaring one takes over `editor.Editor`'s method of the
    /// same name — except `keyIsInherited` (a predicate the engine queries) and
    /// `seqItemLineStart` (a sub-computation), which are named for what they
    /// answer rather than for a method. Signatures are documented on the
    /// `Editor` method each overrides; see `editor.zig`.
    const hooks = [_][]const u8{
        "insertKey",         "deleteKeyGuard",
        "replaceValAtPath",  "replaceValAtPathFollowing",
        "replaceKeyAtPath",  "keyIsInherited",
        "seqItemLineStart",  "appendToSeq",
        "prependToSeq",      "removeSeqItem",
        "reorderSeqItems",   "addLeadingComment",
        "deleteLeadingComments", "getLeadingComment",
        "setTrailingComment", "deleteTrailingComment",
        "getTrailingComment",
    };

    fn has(comptime set: []const []const u8, comptime name: []const u8) bool {
        for (set) |k| if (std.mem.eql(u8, k, name)) return true;
        return false;
    }

    fn known(comptime name: []const u8) bool {
        return has(&required, name) or has(&required_edit, name) or
            has(&optional, name) or has(&hooks, name);
    }

    /// The known name `name` differs from only by letter case, or null.
    ///
    /// Not a general edit distance — deliberately. Every name above is
    /// camelCase, so the typo that actually costs something is a capitalization
    /// slip (`insertkey`, `appendtoseq`), and that is the one this catches. A
    /// wilder misspelling still fails; it just fails without a suggestion.
    fn nearest(comptime name: []const u8) ?[]const u8 {
        for ([_][]const []const u8{ &required, &required_edit, &optional, &hooks }) |set| {
            for (set) |k| if (std.ascii.eqlIgnoreCase(k, name)) return k;
        }
        return null;
    }
};

/// The enforcement point for the `Language` contract: every declaration a
/// format must supply, the closed set it may supply, plus the coherence rules
/// between them.
///
/// `Editor()` calls this for the format it is generic over, but that is not
/// enough on its own — a read-only format has no editor, so `validate(XML)`
/// would never be instantiated and XML's manifest would go unchecked. The
/// `comptime` block below this function closes that gap by running `validate`
/// over every compiled-in language, editable or not.
pub fn validate(comptime Lang: type) void {
    comptime {
        // Every check here is a linear scan over a name list, and the closed-set
        // check runs one such scan PER declaration — so the work is roughly
        // `decls × known-names` per format, times eleven formats from the
        // registry loop below. That clears the default 1000-branch budget
        // comfortably; the quota is per-evaluation, not a leak.
        @setEvalBranchQuota(20_000);

        // The original four, plus `Parser` — which `tryParse` and `Editor`
        // have both required in practice for as long as they have existed,
        // and which this now states.
        // `@typeName` rather than `Lang.name` here and in `required_edit`:
        // `name` is itself one of the declarations being checked, so it cannot
        // be relied on to identify the format that is missing it.
        for (Decls.required) |name| {
            if (!@hasDecl(Lang, name))
                @compileError(@typeName(Lang) ++ " must define " ++ name);
        }
        if (@TypeOf(Lang.caps) != Caps)
            @compileError("Language.caps must be a language.Caps");

        // `syntax` describes how the generic splice engine writes this
        // format, so it is required exactly when there is an editor to read
        // it. Requiring it unconditionally would be asking a read-only
        // format to describe an editing surface it does not have.
        if (Lang.caps.edit) {
            for (Decls.required_edit) |name| {
                if (!@hasDecl(Lang, name))
                    @compileError(@typeName(Lang) ++ " has caps.edit and must define " ++ name);
            }

            // Coherence: a format cannot have a same-line trailing comment
            // marker without having a comment syntax at all. Checked over
            // every dialect, since `syntax` is indexed by one.
            for (std.meta.tags(Lang.Type)) |t| {
                const s: Syntax = Lang.syntax(t);
                if (s.trailing_comment != null and s.line_comment == null)
                    @compileError("Language declares a trailing comment marker but no line comment marker");
            }
        }

        // The closed set. `@typeInfo(...).decls` lists only PUBLIC
        // declarations, so a format's private helpers — the
        // `const edit = @import("editor_helper.zig")` each hooks block opens
        // with — are invisible here and need no exemption.
        for (@typeInfo(Lang).@"struct".decls) |d| {
            if (Decls.known(d.name)) continue;
            @compileError("Language '" ++ Lang.name ++ "' declares unknown '" ++ d.name ++ "'" ++
                if (Decls.nearest(d.name)) |near|
                    " — did you mean '" ++ near ++ "'?"
                else
                    ". Editing hooks must be named for the `editor.Editor` method they" ++
                        " override, and added to `Decls.hooks` in language.zig.");
        }

        // Coherence: a format that says it cannot be edited must not declare
        // editing behaviour. Without this, `caps.edit = false` and a live hook
        // can disagree indefinitely — nothing else reads both.
        if (!Lang.caps.edit) {
            for (Decls.hooks) |name| {
                if (@hasDecl(Lang, name))
                    @compileError("Language '" ++ Lang.name ++ "' declares caps.edit = false" ++
                        " but supplies the editing hook '" ++ name ++ "'");
            }
            return;
        }

        // The remaining rules are about hooks being REACHABLE. Both follow from
        // where `editor.zig` dispatches, so both are dead-code checks rather
        // than taste: a hook the engine can never call is a silent no-op, and
        // silent is the failure mode this whole section exists to remove.

        // A block-sequence hook sits below `editor.zig`'s
        // `block_seq_editable` refusal, so a format that declares no editable
        // block sequences in any dialect can never reach one.
        var any_block_seq = false;
        var any_line_comment = false;
        for (std.meta.tags(Lang.Type)) |t| {
            const s: Syntax = Lang.syntax(t);
            if (s.block_seq_editable) any_block_seq = true;
            if (s.line_comment != null) any_line_comment = true;
        }
        if (!any_block_seq) {
            for ([_][]const u8{ "appendToSeq", "prependToSeq", "removeSeqItem", "reorderSeqItems" }) |name| {
                if (@hasDecl(Lang, name))
                    @compileError("Language '" ++ Lang.name ++ "' declares block_seq_editable = false" ++
                        " but supplies '" ++ name ++ "', which the engine refuses before reaching");
            }
        }

        // Comment hooks, the other direction. With no line-comment marker in
        // ANY dialect, every comment op is either hooked or permanently
        // `CommentsUnsupported` — so hooking SOME is almost certainly a
        // dropped delegation rather than a decision. plist is the case this
        // guards: `<!-- ... -->` is a delimiter pair with no leader, so it
        // declares null and hooks all six deliberately (see `plist.zig`), and
        // this makes losing one a compile error instead of a runtime refusal.
        //
        // Note this is the OPPOSITE of the rule the proposal's §4 proposed —
        // "trailing_comment == null alongside a declared setTrailingComment is
        // a contradiction". plist is exactly that pair, and is correct. A hook
        // does not read the marker, so a null marker beside a hook is not a
        // contradiction; it is the hook making the marker irrelevant.
        if (!any_line_comment) {
            const comment_hooks = [_][]const u8{
                "addLeadingComment",  "deleteLeadingComments", "getLeadingComment",
                "setTrailingComment", "deleteTrailingComment", "getTrailingComment",
            };
            var declared = 0;
            for (comment_hooks) |name| {
                if (@hasDecl(Lang, name)) declared += 1;
            }
            if (declared != 0 and declared != comment_hooks.len) {
                for (comment_hooks) |name| {
                    if (!@hasDecl(Lang, name))
                        @compileError("Language '" ++ Lang.name ++ "' has no line-comment marker" ++
                            " in any dialect and hooks some comment ops but not '" ++ name ++
                            "', which can then only ever return CommentsUnsupported");
                }
            }
        }
    }
}

/// Every compiled-in language, as a comptime list to iterate.
///
/// The set of formats written down ONCE, so anything that has to do something
/// per-format — `validate` below, the CLI's extension table — cannot fall out
/// of step with the set that actually exists. A gated-out format is ABSENT
/// here rather than present as `void`, so a consumer needs no gate of its own.
///
/// This is the "comptime registry with something to iterate" the proposal's §7
/// names as what the manifest unlocks. It does not by itself retire the five
/// parallel format enumerations — those are per-DIALECT and this is
/// per-LANGUAGE — but a consumer that is genuinely per-language now has one
/// list to walk instead of eleven `build_options` tests to repeat.
pub const compiled: []const type = blk: {
    var list: []const type = &.{};
    if (build_options.lang_json) list = list ++ [_]type{JSON};
    if (build_options.lang_yaml) list = list ++ [_]type{YAML};
    if (build_options.lang_toml) list = list ++ [_]type{TOML};
    if (build_options.lang_zon) list = list ++ [_]type{ZON};
    if (build_options.lang_xml) list = list ++ [_]type{XML};
    if (build_options.lang_fig) list = list ++ [_]type{FIG};
    if (build_options.lang_ini) list = list ++ [_]type{INI};
    if (build_options.lang_dotenv) list = list ++ [_]type{DOTENV};
    if (build_options.lang_properties) list = list ++ [_]type{PROPERTIES};
    if (build_options.lang_plist) list = list ++ [_]type{PLIST};
    if (build_options.lang_nestedtext) list = list ++ [_]type{NESTEDTEXT};
    break :blk list;
};

// Validate every compiled-in language, including the read-only ones that no
// `Editor()` instantiation would otherwise reach. Runs whenever this file is
// analyzed, which is whenever anything touches a format at all.
comptime {
    for (compiled) |Lang| validate(Lang);
}

test "detect identifies each compiled-in format by content" {
    const a = std.testing.allocator;
    if (comptime build_options.lang_json) {
        try std.testing.expectEqual(Detected.json, detect(a, "{\"x\":1}").?);
    }
    if (comptime build_options.lang_zon) {
        try std.testing.expectEqual(Detected.zon, detect(a, ".{ .x = 1 }").?);
    }
    if (comptime build_options.lang_plist) {
        try std.testing.expectEqual(Detected.plist, detect(a, "<dict><key>a</key><string>b</string></dict>").?);
    }
    if (comptime build_options.lang_xml) {
        try std.testing.expectEqual(Detected.xml, detect(a, "<r/>").?);
    }
    if (comptime build_options.lang_toml) {
        try std.testing.expectEqual(Detected.toml, detect(a, "x = 1\n").?);
    }
    if (comptime build_options.lang_fig) {
        // A bare container header line (no `=`, no `:`, no brackets) followed
        // by a `>`-depth child isn't valid JSON/ZON/XML/TOML, so this resolves
        // to fig even though it's tried before YAML.
        try std.testing.expectEqual(Detected.fig, detect(a, "database\n> host = localhost\n").?);
    }
    if (comptime build_options.lang_ini) {
        // A `;`-led comment line is invalid JSON/ZON/XML/TOML (TOML has no `;`
        // comment leader — its bare-key scanner rejects `;` outright) and not
        // fig syntax either, so this resolves to INI even though it's tried
        // right before YAML.
        try std.testing.expectEqual(Detected.ini, detect(a, "; header\nname = fig\n").?);
    }
    if (comptime build_options.lang_dotenv) {
        // A double-quoted value spanning a literal embedded newline is the one
        // shape only dotenv parses: INI's value never crosses a physical line
        // (it hits the line's `\n` first), so `[a]` on its own next line is a
        // bad INI statement — this falls all the way through INI to dotenv.
        try std.testing.expectEqual(Detected.dotenv, detect(a, "A=\"line1\nline2\"\n").?);
    }
    if (comptime build_options.lang_yaml) {
        // A plain mapping that is not valid JSON/TOML/fig/INI/etc. falls
        // through to YAML, the most permissive grammar and therefore tried
        // second-to-last.
        try std.testing.expectEqual(Detected.yaml, detect(a, "key: value\n").?);
    }
    if (comptime build_options.lang_properties) {
        // Malformed YAML (a scalar followed by unexpectedly-indented content)
        // still parses as `.properties`: worst case, each line is just a bare
        // key with an empty value (see `properties/tokenizer.zig`) — the most
        // permissive grammar of all, so it's tried dead last.
        try std.testing.expectEqual(Detected.properties, detect(a, "a: 1\n b: 2\n").?);
    }
}

test "detect: plain `key = value` prefers TOML over fig despite fig accepting it too" {
    const a = std.testing.allocator;
    if (comptime !build_options.lang_toml or !build_options.lang_fig) return error.SkipZigTest;
    // fig's root-level dotted assignment accepts the exact same shape TOML
    // does; TOML is tried first, so it wins the tie.
    try std.testing.expectEqual(Detected.toml, detect(a, "x = 1\n").?);
}

test "detect: a plist document prefers plist over generic xml despite xml accepting it too" {
    const a = std.testing.allocator;
    if (comptime !build_options.lang_plist or !build_options.lang_xml) return error.SkipZigTest;
    // Any well-formed plist is also well-formed generic XML; plist is tried
    // first, so it wins. Ordinary XML that isn't plist-shaped still falls
    // through to `.xml`.
    try std.testing.expectEqual(Detected.plist, detect(a, "<dict><key>a</key><string>b</string></dict>").?);
    try std.testing.expectEqual(Detected.xml, detect(a, "<r/>").?);
}
