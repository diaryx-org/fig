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
pub const NativeKinds = manifest.NativeKinds;
pub const Syntax = manifest.Syntax;
pub const Literal = manifest.Literal;
pub const SectionNoun = manifest.SectionNoun;

/// The one list of formats — `src/languages/list.zig` — re-exported so a
/// consumer that is genuinely per-format (the build, `validate-check`, the
/// harness) reads the same rows this file pairs with modules.
pub const list = @import("list.zig");

// The language MODULES, imported unconditionally, each named by its row in
// `list.rows`. Every other per-language or per-dialect thing is derived from
// the pairing: the gated aliases just below, `compiled`, and the format
// registry `dialects`, which is assembled from each module's own
// `Language.dialects` table in `list.rows` order (see the registry note on
// why that order is frozen).
//
// This is the ONE line a new format adds to core beside its row in the list.
// It cannot be generated from the row — `@import` takes only a string
// literal — so the comptime block after it checks the two lists agree in
// both directions and in order, and the gate is looked up by name rather
// than paired by hand.
//
// The module is imported whether or not its gate is on. Nothing runtime is
// ever reached through the module itself — only comptime DECLARATIONS
// (`Language.dialects`, `Language.caps`) are read off it, which is how the
// registry keeps a gated-out language's rows, names, ABI values and spellings
// build-invariant, the way its `--spec` strings always were. Zig's analysis is
// lazy, so reading a declaration builds no parser and no printer; everything
// that would goes through the gated alias, which is `void` when the gate is
// off.
const slots = .{
    .{ .name = "json", .mod = @import("json/json.zig") },
    .{ .name = "yaml", .mod = @import("yaml/yaml.zig") },
    .{ .name = "toml", .mod = @import("toml/toml.zig") },
    .{ .name = "zon", .mod = @import("zon/zon.zig") },
    .{ .name = "fig", .mod = @import("fig/fig.zig") },
    .{ .name = "ini", .mod = @import("ini/ini.zig") },
    .{ .name = "dotenv", .mod = @import("dotenv/dotenv.zig") },
    .{ .name = "properties", .mod = @import("properties/properties.zig") },
    .{ .name = "plist", .mod = @import("plist/plist.zig") },
    .{ .name = "nestedtext", .mod = @import("nestedtext/nestedtext.zig") },
};

comptime {
    // `slots` and `list.rows` are the same list twice, in the same order —
    // one holding what only a string literal can spell, the other what the
    // build has to read without importing a language. Either drifting fails
    // here, naming the row or slot that has no partner.
    if (slots.len != list.rows.len)
        @compileError("src/languages/list.zig has " ++ std.fmt.comptimePrint("{d}", .{list.rows.len}) ++
            " rows but `slots` in language.zig has " ++ std.fmt.comptimePrint("{d}", .{slots.len}) ++
            " — a format is one row in the list and one `@import` slot here");
    for (slots, list.rows, 0..) |slot, row, i| {
        if (!std.mem.eql(u8, slot.name, row.name))
            @compileError("slot " ++ std.fmt.comptimePrint("{d}", .{i}) ++ " of language.zig is '" ++
                slot.name ++ "' but row " ++ std.fmt.comptimePrint("{d}", .{i}) ++
                " of src/languages/list.zig is '" ++ row.name ++
                "' — the two lists must agree in order, since the order is every derived enum's");
        if (!std.mem.eql(u8, slot.mod.Language.name, slot.name))
            @compileError("slot '" ++ slot.name ++ "' imports a module whose Language.name is '" ++
                slot.mod.Language.name ++ "'");
    }
}

/// Whether the format named `name` is compiled into this build: the
/// `build_options.lang_<name>` decl the build declares per row of the list.
fn gateOf(comptime name: []const u8) bool {
    return @field(build_options, "lang_" ++ name);
}

/// A slot's `Language` when its gate is on, `void` when it is off.
fn gated(comptime slot: anytype) type {
    return if (gateOf(slot.name)) slot.mod.Language else void;
}

/// The gated `Language` of the format named `name` — `void` when the format
/// is compiled out — looked up by name in the list rather than by position.
/// A name that is not a row is a compile error naming it.
pub fn of(comptime name: []const u8) type {
    return gated(slots[comptime list.indexOf(name)]);
}

// Per-language gates: a compiled-out format resolves to `void`, so nothing that
// would build its parser or printer is ever referenced. Every call site that
// touches a gated `Language.*` must guard the access behind the same
// `build_options.lang_*` flag (a `comptime` check), or it will fail to compile
// against `void`. JSON is gateable like the rest now that `detect` no longer
// assumes it as a base.
//
// Named aliases rather than `of("json")` at every call site because the
// names are how every guide, test and consumer reaches a format; they are
// the one thing here a new format still adds by hand, since Zig cannot
// declare a named constant from a loop. `of` is the same lookup for a caller
// that has the name as a string.
pub const JSON = of("json");
pub const YAML = of("yaml");
pub const TOML = of("toml");
pub const ZON = of("zon");
pub const FIG = of("fig");
pub const INI = of("ini");
pub const DOTENV = of("dotenv");
pub const PROPERTIES = of("properties");
pub const PLIST = of("plist");
pub const NESTEDTEXT = of("nestedtext");

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
// As of this stage ALL SIX of those enums are REIFIED from it rather than
// merely pinned against it — `Detected` below, `cli.Format`,
// `AST.SerializeFormat`, `deserialize.Format`, `Embed.InnerFormat` and
// `c_api.FigFormat` are all `@Enum` over `namesOf(…)` — along with
// `cli/args.zig`'s extension table. The C ABI's enum is the one built over
// `abi_value` rather than over member positions, since its integers are a
// permanent contract; what remains hand-written beside it is a literal pin of
// those integers, plus `zig build abi-check` diffing the same values against
// fig.h. Every per-dialect FACT is now read rather than restated: the CLI's
// dispatch takes the splice style, empty-document seed and `--spec` strings,
// the serializer takes the printer names (`ast/serialize_options.zig`, whose
// three switches are one `inline else` each over
// `@field(d.Lang.Printer, d.print_name)`), and `embed.zig` takes the embedded
// spellings — its four literal builders and two tag/MIME resolvers are one
// `inline` over the registry each. `c_api.zig` reads the rest: `abi_value` for
// the enum, `Lang` for parser/capability/editor dispatch, and `dialect` for the
// three JSON ABI values that share one language.
//
// The rows themselves are no longer written here. Each language declares its
// own — `Language.dialects`, a `[]const manifest.Dialect(Language)` beside its
// `name`/`extensions`/`caps` — and `dialects` below is ASSEMBLED from those
// tables in `list.rows` order, lifting each row to the gated `Dialect(Lang)` so
// the `void` protocol holds exactly as it did when the rows were literal.

/// The first `abi_value` no compiled-in format may take. Every integer at or
/// above it is reserved for a language registered at runtime
/// (`docs/proposals/runtime-languages.md` §4.3), where the integers are
/// assigned per process and never pinned. Reserved in core 3.0 so that a
/// later minor can hand out a runtime integer without asking whether the
/// value might collide with a format compiled in after it. fig.h states the
/// same number as `FIG_FORMAT_RUNTIME_BASE`, `zig build abi-check` holds the
/// two equal, and the comptime block below refuses a registry row that
/// reaches it.
pub const runtime_abi_base: c_int = 4096;

// The `void` protocol and the entry shape, re-exported from the leaf manifest
// (where they moved so a language can name `Dialect(Language)` for its own
// table). `cli/parse_dispatch.zig` re-exports the first two again under the
// same names.
pub const DialectOf = manifest.DialectOf;
pub const defaultDialect = manifest.defaultDialect;
pub const SpliceStyle = manifest.SpliceStyle;
pub const SpecName = manifest.SpecName;
pub const Dialect = manifest.Dialect;

/// The registry's element type for a (possibly gated-out) language: what a
/// language's own `Dialect(Language)` row becomes once lifted into the table.
/// The same struct — `Lang` collapses to `void`, and `dialect`/`specs` with
/// it, when the gate is off.
fn Entry(comptime L: type) type {
    return Dialect(L);
}

/// A language's own row lifted to the registry's gated type: every field
/// copied as declared, except the three the `void` protocol touches — `Lang`
/// becomes the gated alias, and `dialect` and each `specs[i].dialect` collapse
/// to the `void` value with it. Field-by-field over `@typeInfo`, so a field
/// added to `manifest.Dialect` is carried without an edit here; the three
/// exceptions are named, and anything else is copied verbatim.
fn lift(comptime d: anytype, comptime G: type) Entry(G) {
    comptime {
        var out: Entry(G) = undefined;
        for (@typeInfo(Entry(G)).@"struct".fields) |f| {
            if (std.mem.eql(u8, f.name, "Lang")) {
                out.Lang = G;
            } else if (std.mem.eql(u8, f.name, "dialect")) {
                out.dialect = if (G == void) {} else d.dialect;
            } else if (std.mem.eql(u8, f.name, "specs")) {
                var specs: []const SpecName(G) = &.{};
                for (d.specs) |sp| {
                    specs = specs ++ [_]SpecName(G){.{
                        .name = sp.name,
                        .dialect = if (G == void) {} else sp.dialect,
                    }};
                }
                out.specs = specs;
            } else {
                @field(out, f.name) = @field(d, f.name);
            }
        }
        return out;
    }
}

/// The registry's element types, one per row, in registry order — the tuple
/// type `dialects` is built as. Heterogeneous because each language's rows
/// are `Entry(<its gated alias>)`, so the table cannot be a plain slice.
const entry_types: []const type = blk: {
    var ts: []const type = &.{};
    for (slots) |slot| {
        for (slot.mod.Language.dialects) |_| ts = ts ++ [_]type{Entry(gated(slot))};
    }
    break :blk ts;
};

/// EVERY user-facing dialect, as a heterogeneous comptime tuple — thirteen
/// entries over eleven languages (the JSON module supplies three) — assembled
/// from each language's own `Language.dialects` in `list.rows` order.
///
/// Two properties of this table are frozen, and both are load-bearing:
///
///   * ORDER. It IS the member order of every enum derived from it
///     (`Detected`, `cli.Format`, `SerializeFormat`, `deserialize.Format`,
///     `Embed.InnerFormat` and `c_api.FigFormat` — the last of which takes its
///     member order from here but its VALUES from `abi_value`), and
///     reproduces the pre-registry `cli.Format` order minus its two
///     non-registry members (`canonical`, which is not a `Language` at all,
///     and `gron`, a CLI-only projection — both spliced back by hand at their
///     old positions, see `namesWith`) and minus `yml`, an alias of `yaml`
///     retired in Stage 3. Reordering it would silently renumber
///     `@intFromEnum` for every one of those enums. It is the product of
///     two orders — `list.rows` (which `slots` is checked against) and each
///     language's own table. Append: a new language is a new last row of the
///     list and a new last slot, a new dialect of an existing language is its
///     table's last row.
///
///   * `abi_value`. It is the C ABI, and a released value is permanent.
///
/// Entries are ALWAYS present — a gated-out language's rows are read off its
/// module all the same and lifted with `Lang` collapsed to `void` rather than
/// dropped — so every derived enum is build-invariant and only the *behaviour*
/// behind a member is gated.
///
/// `canonical` and `gron` are deliberately absent: canonical is the AST's own
/// oracle grammar (no `Language`, no dialect, an options-less printer) and
/// gron is a CLI-only projection of JSON. Both stay explicit named arms at
/// every switch, which is also what keeps an exhaustive switch honest — a new
/// member has to be either a registry entry or one of those two.
pub const dialects: std.meta.Tuple(entry_types) = blk: {
    @setEvalBranchQuota(20_000);
    var out: std.meta.Tuple(entry_types) = undefined;
    var i: usize = 0;
    for (slots) |slot| {
        for (slot.mod.Language.dialects) |d| {
            out[i] = lift(d, gated(slot));
            i += 1;
        }
    }
    break :blk out;
};

/// The MODULE the registry entry `name` was declared in — the ungated route
/// to that language's comptime declarations in a build that has it compiled
/// out (`Lossless.nativeFor` reads `caps.lossless` this way, so the envelope
/// table stays build-invariant). Only declarations may be read through it;
/// anything that would build the language's code goes through the entry's
/// gated `Lang`.
pub fn moduleFor(comptime name: []const u8) type {
    inline for (slots) |slot| {
        inline for (slot.mod.Language.dialects) |d| {
            if (comptime std.mem.eql(u8, d.name, name)) return slot.mod;
        }
    }
    @compileError("no registry entry for format '" ++ name ++ "'");
}

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
    /// `.sniff_rank != null` — `Language.Detected`.
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
                .detectable => d.sniff_rank != null,
                .deserializable => d.deserializable,
                .embeddable => d.embed != null,
            };
            if (take) out = out ++ [_][:0]const u8{d.name};
        }
        return out;
    }
}

/// A member a derived enum carries that the registry does not back: what it is
/// called, and which registry entry it sits immediately after. `cli.Format`'s
/// `canonical` (after `zon`) and `gron` (after `fig`), and `SerializeFormat`'s
/// `canonical`, are the only three uses — see `dialects`' note on why neither
/// format is an entry.
pub const Extra = struct {
    /// The registry entry name this member follows directly.
    after: []const u8,
    /// The member name. Sentinel-terminated: a reified enum's field
    /// names must be.
    name: [:0]const u8,
};

/// `namesOf(sel)` with each `extras` member spliced in directly after the
/// registry entry it names. The member list of a derived enum that has
/// hand-placed members on top of the registry's; the splice positions are what
/// reproduce the pre-reification member ORDER, so they are as load-bearing as
/// the registry's own order and are checked here rather than trusted.
pub fn namesWith(comptime sel: Selector, comptime extras: []const Extra) []const [:0]const u8 {
    comptime {
        @setEvalBranchQuota(20_000);
        var out: []const [:0]const u8 = &.{};
        var placed = [_]bool{false} ** extras.len;
        for (namesOf(sel)) |n| {
            out = out ++ [_][:0]const u8{n};
            for (extras, 0..) |x, xi| {
                if (!std.mem.eql(u8, x.after, n)) continue;
                if (placed[xi])
                    @compileError("two format-registry entries are named '" ++ x.after ++
                        "', so '" ++ x.name ++ "' has no single position to take");
                placed[xi] = true;
                out = out ++ [_][:0]const u8{x.name};
            }
        }
        for (extras, placed) |x, p| {
            if (!p)
                @compileError("the derived member '" ++ x.name ++ "' is placed after '" ++ x.after ++
                    "', which is not a format-registry entry this enum draws from");
        }
        return out;
    }
}

// Reifying an enum from a name list. This Zig spells type reification as
// granular builtins (`@Enum`/`@Union`) rather than `@Type(.{...})`; the
// in-tree precedent is `c_api.zig`'s `EditorUnion`.
//
// The two halves below are helpers rather than one `MakeEnum(names)` function
// deliberately: a type CREATED inside a generic function is named after that
// function, so every derived format enum would answer to
// `language.MakeEnum(&.{ &.{ ... }[0..(...)], … }[0..14])` in every compile
// error that mentions it. Calling `@Enum` at the declaration site instead
// gives each one its own name (`cli.types.Format`, `serialize_options
// .SerializeFormat`, …) — the error a missing switch arm produces is the whole
// point of these enums, so it is worth two call sites' worth of noise.

/// The tag type a registry-derived enum of `names.len` members gets:
/// `IntFittingRange(0, n - 1)` — exactly what Zig infers for a hand-written
/// `enum { … }` of the same size, so a reified enum is bit-for-bit the one it
/// replaces rather than merely name-compatible.
pub fn EnumTag(comptime member_names: []const [:0]const u8) type {
    if (member_names.len == 0)
        @compileError("a format enum derived from the registry must have at least one member");
    return std.math.IntFittingRange(0, member_names.len - 1);
}

/// The tag values of a registry-derived enum: 0..n-1 in `names` order, so the
/// member order IS the registry order and `@intFromEnum` keeps meaning what it
/// meant before reification. Pass as `&enumValues(names)`.
pub fn enumValues(comptime member_names: []const [:0]const u8) [member_names.len]EnumTag(member_names) {
    comptime {
        var values: [member_names.len]EnumTag(member_names) = undefined;
        for (&values, 0..) |*v, i| v.* = @intCast(i);
        return values;
    }
}

/// Fail the build unless `E`'s members are exactly `want` (in `want`'s order)
/// plus `extra` (which may sit anywhere, and must all be present). `what`
/// names the enum in the message.
///
/// The shape every "this enum is a restatement of the registry" assert needs:
/// order matters for the members that come FROM the registry, because that
/// order becomes theirs when the enum is reified, while the deliberate
/// non-registry members (`canonical`, `gron`) are positioned by hand and only
/// have to still exist.
///
/// Reification has since taken every caller — `cli.Format`,
/// `AST.SerializeFormat`, `deserialize.Format`, `Detected`,
/// `Embed.InnerFormat` and `c_api.FigFormat` are all BUILT from
/// `namesOf`/`namesWith` rather than checked against them, which is the
/// stronger statement. It stays for the next enum that must remain
/// hand-written and still restate the registry in order.
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

// The registry's self-consistency, plus the one derived enum that lives in this
// file. What each derived enum still states for itself — the `--spec` spellings
// beside `resolveSpec`, the ABI integers beside `c_api.FigFormat` — cannot be
// written here (this file sits BELOW all of them, and reaching up would invert
// the dependency), so each lives beside the enum it pins.
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
    // And every compiled-in value sits below the runtime range: an integer at
    // or above `runtime_abi_base` is assigned per process to a language
    // registered at runtime, so a row taking one would be ambiguous with a
    // registration in any process that made one.
    for (dialects) |d| {
        if (d.abi_value >= runtime_abi_base)
            @compileError("format-registry entry '" ++ d.name ++ "' takes the ABI value " ++
                std.fmt.comptimePrint("{d}", .{d.abi_value}) ++ ", but values from " ++
                std.fmt.comptimePrint("{d}", .{runtime_abi_base}) ++
                " up are reserved for languages registered at runtime (FIG_FORMAT_RUNTIME_BASE)");
    }

    // Sniff ranks are identities too: two dialects at one rank would have no
    // defined probe order between them. And every language must be sniffable
    // through at least one of its dialects — a rank left off every row is a
    // format `detect` silently never returns, which is the failure mode a
    // default of null would otherwise invite.
    for (dialects, 0..) |a, ai| {
        for (dialects, 0..) |b, bi| {
            if (bi > ai and a.sniff_rank != null and a.sniff_rank == b.sniff_rank)
                @compileError("format-registry entries '" ++ a.name ++ "' and '" ++ b.name ++
                    "' both declare sniff_rank " ++ std.fmt.comptimePrint("{d}", .{a.sniff_rank.?}) ++
                    " — the probe order is total, so every rank is unique");
        }
    }
    for (slots) |slot| {
        var any = false;
        for (slot.mod.Language.dialects) |d| {
            if (d.sniff_rank != null) any = true;
        }
        if (!any)
            @compileError("no dialect of '" ++ slot.name ++ "' declares a sniff_rank, so `detect`" ++
                " could never return it — give the row named after the language a rank");
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
            @field(d.Lang.Type, "JSONC")
        else if (std.mem.eql(u8, d.name, "json5"))
            @field(d.Lang.Type, "JSON5")
        else
            defaultDialect(d.Lang);
        if (d.dialect != expected)
            @compileError("format-registry entry '" ++ d.name ++
                "' selects a dialect other than the one its call sites pass today");
    }

    // Every row's `Lang` is the gated alias of the language whose table it
    // came from, and every gated-in row still describes a dialect of that
    // language — `lift` is what makes this true, checked here so it stays so.
    for (dialects) |d| {
        if (d.Lang == void) continue;
        var declared = false;
        for (d.Lang.dialects) |own| {
            if (std.mem.eql(u8, own.name, d.name)) declared = true;
        }
        if (!declared)
            @compileError("format-registry entry '" ++ d.name ++ "' is attributed to '" ++
                d.Lang.name ++ "', whose own `dialects` table does not declare it");
    }

    // `entryFor` itself: the lookup every derived dispatch in the CLI, the
    // serializer, `embed.zig` and the C ABI opens with, checked here so a
    // build that touches a format at all also proves the lookup works.
    if (entryFor("json").abi_value != 1 or entryFor("nestedtext").abi_value != 13)
        @compileError("`entryFor` does not return the entry it was asked for");

    // LAST, deliberately: `Detected` is the one derived enum living in this
    // file, so a registry that is internally inconsistent (a missing entry, a
    // duplicated name) would fail HERE too — with a message about `Detected`
    // rather than about the registry. Checking the table's own coherence first
    // means the error names the actual mistake.
    //
    // `Detected` is REIFIED from `.sniff_rank != null`, so "its members are
    // the sniffable entries, in registry order" is true by construction and
    // there is nothing left to compare. What is still a real claim is which
    // entries carry a rank — the membership its doc comment argues for — so
    // that is what this checks: jsonc out, its json/json5 siblings in, and
    // `canonical` (no entry at all) absent.
    if (@hasField(Detected, "jsonc"))
        @compileError("the `jsonc` registry entry declares a sniff_rank, but `detect` deliberately" ++
            " never sniffs it — it overlaps json/json5 on almost all input");
    if (!@hasField(Detected, "json") or !@hasField(Detected, "json5"))
        @compileError("`detect` returns `.json`/`.json5`, so both entries must stay detectable");
    if (@hasField(Detected, "canonical"))
        @compileError("`canonical` is not a registry entry and cannot be sniffed");
}

/// Whether `name` is a compiled-in format's dialect name — what a runtime
/// registration may not reuse (`languages/runtime.zig`).
pub fn isCompiledName(name: []const u8) bool {
    inline for (dialects) |d| {
        if (std.mem.eql(u8, d.name, name)) return true;
    }
    return false;
}

/// A format `detect` can recognize: the registry entries with a
/// `sniff_rank`, in registry order (NOT probe order — see `sniff_order`).
/// The `jsonc` dialect and `canonical` are deliberately excluded: jsonc
/// overlaps json/json5 on most input, and canonical is an explicit selection
/// rather than something to sniff (it is not a registry entry at all).
pub const Detected = @Enum(EnumTag(detected_names), .exhaustive, detected_names, &enumValues(detected_names));

const detected_names = namesOf(.detectable);

/// The probe order: every detectable entry's name, sorted by the
/// `sniff_rank` its own row declares. The order is a single argument about
/// grammar overlap — strictest first — but each step of it is a fact about
/// one format, so each row carries its rank and the reasoning for it, and
/// this is only the sort. A test below pins the result to the sequence the
/// hand-written `detect` used to spell, so a rank cannot move unnoticed.
pub const sniff_order: []const [:0]const u8 = blk: {
    @setEvalBranchQuota(20_000);
    const n = detected_names.len;
    var ranks: [n]u8 = undefined;
    var names: [n][:0]const u8 = undefined;
    var i: usize = 0;
    for (dialects) |d| {
        if (d.sniff_rank) |r| {
            ranks[i] = r;
            names[i] = d.name;
            i += 1;
        }
    }
    // Insertion sort by rank; `n` is thirteen at most.
    var a: usize = 1;
    while (a < n) : (a += 1) {
        var b = a;
        while (b > 0 and ranks[b - 1] > ranks[b]) : (b -= 1) {
            const tr = ranks[b - 1];
            ranks[b - 1] = ranks[b];
            ranks[b] = tr;
            const tn = names[b - 1];
            names[b - 1] = names[b];
            names[b] = tn;
        }
    }
    const out = names;
    break :blk &out;
};

/// Best-effort content sniffing: try each COMPILED-IN parser in `sniff_order`
/// and return the first that accepts `input`, or null if none do (also what
/// an all-languages-disabled build returns). Order matters because the
/// grammars overlap — from most to least strict — and each row's
/// `sniff_rank` says where it sits and why. This is a heuristic, not a
/// proof: input valid as more than one format resolves to the earliest
/// candidate in the order.
pub fn detect(allocator: Allocator, input: []const u8) ?Detected {
    inline for (sniff_order) |name| {
        const d = comptime entryFor(name);
        if (comptime d.Lang != void) {
            if (tryParse(d.Lang, allocator, input, d.dialect)) return @field(Detected, name);
        }
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
/// Adding a renderer to `editor.zig` means adding its name here too. That is
/// the deliberate cost of the check, and the compiler charges it immediately:
/// a renderer the list does not know is one no format can declare.
const Decls = struct {
    /// Required of every format, editable or not.
    ///
    /// `Printer` is the format's printer MODULE, and is distinct from the
    /// optional `printNode` decl below: the module is what the serializer
    /// dispatches through (`ast/serialize_options.zig` reaches
    /// `@field(d.Lang.Printer, d.print_name)` for every registry entry), so
    /// every format must expose one even when — as with plist — its
    /// `Language` wraps only the module's `print`.
    ///
    /// `dialects` is the format's own rows of the format registry — see
    /// `manifest.Dialect` — which `language.zig` assembles rather than writes.
    const required = [_][]const u8{
        "Type",  "Parser", "Printer",    "default_type", "parse",
        "print", "name",   "extensions", "caps",         "dialects",
    };

    /// Required of an editable format only. `syntax` describes how the generic
    /// splice engine writes this format; asking a read-only format for one is
    /// asking it to describe an editing surface it does not have.
    const required_edit = [_][]const u8{"syntax"};

    /// Permitted, not required.
    ///
    ///   * `printNode` — every format but plist, whose `print` is
    ///     written inline.
    ///   * `parseAbstract` — the AST-only parse `deserialize.zig` dispatches
    ///     through. Required of exactly the languages with a `deserializable`
    ///     dialect row (a `validate` rule below), optional for the rest.
    ///   * `samples` — a few small documents in the format's own grammar
    ///     that `languages/harness.zig` parses, prints, reparses and edits
    ///     to check what the engine assumes of every format. Every in-tree
    ///     format declares some; an out-of-tree one may.
    ///   * `hasRenderer` — a language whose renderers are resolved at
    ///     runtime (`languages/runtime.zig`) answers `Editor.hasRenderer`
    ///     itself; a compiled format never declares it, since presence is
    ///     `@hasDecl` there.
    ///   * `runtime` — the same language's marker that its dialect table is
    ///     not a comptime fact: the editor's two gates that read every
    ///     dialect's `syntax` at comptime (may it have sections, may it
    ///     spell a header) answer "ask the entry" instead.
    const optional = [_][]const u8{ "printNode", "parseAbstract", "samples", "hasRenderer", "runtime" };

    /// Editing hooks: none. A hook was a `pub` decl that took over an
    /// `editor.Editor` method wholesale, with the editor in hand. Twenty-five
    /// existed across six formats, and every one ended in a single splice
    /// and was Zig only for want of a fact the parser had dropped (now a
    /// `Document` table — markers, separators, regions, mentions), an engine
    /// constant that was really syntax (now a `Syntax` field), or a string
    /// function (now a renderer below). The set is kept, empty, so that the
    /// closed-set check names a hook a format still spells as unknown
    /// rather than silently never dispatching. See
    /// `docs/proposals/runtime-languages.md` §4.4.
    ///
    /// There are no `*Guard` vetoes either. The four that existed
    /// (`deleteKeyGuard`, `replaceValGuard`, `moveKeyGuard`,
    /// `reorderKeysGuard`) each refused a generic op on a SCATTERED container
    /// (a TOML `[header]` table, an INI `[section]`, a fig block container),
    /// and that is one engine rule over `Document.node_regions` — a section
    /// node cannot be line-spliced — spelled in the format's vocabulary
    /// through `Syntax.section_noun`. See
    /// `docs/proposals/derived-regions.md`.
    const hooks = [_][]const u8{};

    /// Fragment renderers. Each is a pure function from the dialect and
    /// strings to a string the engine splices under the reparse net:
    /// `renderValue(t, allocator, out, value_text, literal)` spells a value
    /// (plist's typed element) given what fig's bare-literal rules make of
    /// the text (`Literal`, classified once by the engine — the one rule
    /// every format's `set` shares), `renderEntry(t, allocator, out, indent, key_text,
    /// value_text)` spells a block-mapping entry past its line's indent
    /// (plist's two-line pair, NestedText's `key:` and `>`-block),
    /// `renderItem(t, allocator, out, indent, value_text)` a block-sequence
    /// item, `renderTail(t, allocator, out, indent, key_text, value_text)`
    /// what follows a key — the separator and the value inline, or the
    /// value re-framed as a block on the following lines — and
    /// `renderKey(t, allocator, out, indent, key_text, old_key)` a renamed
    /// key in the form the old one allows. `t` is the editor's dialect; no
    /// compiled renderer varies by it, and a runtime language's must
    /// (`docs/proposals/runtime-languages.md` §8.4). None receives the
    /// editor or performs a splice; the engine calls one at most once per
    /// edit and splices the result. The engine asks whether a renderer is
    /// present through `Editor.hasRenderer` — `@hasDecl` for a compiled
    /// language; a language may declare `hasRenderer(t, which)` itself and
    /// answer at runtime, which is how a vtable's null slot is an absent
    /// renderer. See `editor.Editor.renderedValue`, `writeEntry`,
    /// `writeItem`, `writeTail` and `replaceKeyAtPath`, and the proposal's
    /// §4.4.
    const renderers = [_][]const u8{ "renderValue", "renderEntry", "renderItem", "renderTail", "renderKey" };

    fn has(comptime set: []const []const u8, comptime name: []const u8) bool {
        for (set) |k| if (std.mem.eql(u8, k, name)) return true;
        return false;
    }

    fn known(comptime name: []const u8) bool {
        return has(&required, name) or has(&required_edit, name) or
            has(&optional, name) or has(&hooks, name) or has(&renderers, name);
    }

    /// The known name `name` differs from only by letter case, or null.
    ///
    /// Not a general edit distance — deliberately. Every name above is
    /// camelCase, so the typo that actually costs something is a capitalization
    /// slip (`insertkey`, `appendtoseq`), and that is the one this catches. A
    /// wilder misspelling still fails; it just fails without a suggestion.
    fn nearest(comptime name: []const u8) ?[]const u8 {
        for ([_][]const []const u8{ &required, &required_edit, &optional, &hooks, &renderers }) |set| {
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
/// enough on its own — a read-only format (generic XML was one, through core
/// 2.x) has no editor, so `validate` of it would never be instantiated and its
/// manifest would go unchecked. The `comptime` block below this function
/// closes that gap by running `validate` over every compiled-in language,
/// editable or not.
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
        // and which this now states — plus `Printer`, which the serializer's
        // registry-derived dispatch requires in exactly the same way.
        // `@typeName` rather than `Lang.name` here and in `required_edit`:
        // `name` is itself one of the declarations being checked, so it cannot
        // be relied on to identify the format that is missing it.
        for (Decls.required) |name| {
            if (!@hasDecl(Lang, name))
                @compileError(@typeName(Lang) ++ " must define " ++ name);
        }
        if (@TypeOf(Lang.caps) != Caps)
            @compileError("Language.caps must be a language.Caps");

        // The format's registry rows. Their type is fixed (the registry lifts
        // exactly `Dialect(Lang)`), there must be at least one (a language
        // with no dialect is a format no derived enum can name), names are
        // unique within the language (the registry checks them across it),
        // and exactly one row selects `default_type` and is named
        // `Lang.name` — that is what ties the language's identity to the
        // member every consumer reaches it by, and what the JSON trio's
        // `jsonc`/`json5` rows are the exception to (they select the other
        // two `Type` members).
        if (@TypeOf(Lang.dialects) != []const Dialect(Lang))
            @compileError("Language '" ++ Lang.name ++ "'.dialects must be a []const language.Dialect(Language)");
        if (Lang.dialects.len == 0)
            @compileError("Language '" ++ Lang.name ++ "' declares no dialects, so no format enum can name it");
        var default_rows = 0;
        for (Lang.dialects, 0..) |d, i| {
            for (Lang.dialects[i + 1 ..]) |other| {
                if (std.mem.eql(u8, d.name, other.name))
                    @compileError("Language '" ++ Lang.name ++ "' declares two dialects named '" ++ d.name ++ "'");
            }
            if (d.Lang != Lang)
                @compileError("Language '" ++ Lang.name ++ "'.dialects row '" ++ d.name ++
                    "' names a different language");
            if (d.dialect == Lang.default_type) {
                default_rows += 1;
                if (!std.mem.eql(u8, d.name, Lang.name))
                    @compileError("Language '" ++ Lang.name ++ "' selects its default_type in the dialect" ++
                        " row named '" ++ d.name ++ "', which must be named after the language");
            }
        }
        if (default_rows != 1)
            @compileError("Language '" ++ Lang.name ++ "' must have exactly one dialect row selecting" ++
                " its default_type (the one named after the language)");

        // Coherence: a `deserializable` row is a promise that `deserialize.zig`
        // can parse the dialect, and it parses through `Lang.parseAbstract`.
        // Without this the row would compile and the dispatch would fail
        // later, in a file that never named the format.
        for (Lang.dialects) |d| {
            if (d.deserializable and !@hasDecl(Lang, "parseAbstract"))
                @compileError("Language '" ++ Lang.name ++ "' marks dialect '" ++ d.name ++
                    "' deserializable but declares no parseAbstract for deserialize.zig to call");
        }

        // Coherence: `caps.lossless` describes what the `$fig` envelope pass
        // may write INTO this format, so it is only meaningful for a format
        // that can be written at all. A read-only format declaring one would
        // be describing output it never produces — the same shape of
        // contradiction as an editing hook under `caps.edit = false`.
        if (Lang.caps.lossless != null and !Lang.caps.serialize)
            @compileError("Language '" ++ Lang.name ++ "' declares caps.lossless (an envelope" ++
                " target for serialized output) but caps.serialize = false, so it never writes" ++
                " the output the envelope would go into");

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
                if (s.comments.trailing != null and s.comments.line == null)
                    @compileError("Language declares a trailing comment marker but no line comment marker");

                // Coherence: `kv_sep = null` says "the generic engine never
                // writes `key<sep>value` for me". Every path that would is
                // under the generic `insertKey`, so the claim holds exactly
                // when that op is hooked or the entry is spelled by a
                // `renderEntry` renderer — and `Editor.kvSep` refuses rather
                // than fabricating a separator if one is ever reached anyway.
                // Without this the null would be a silent `UnsupportedShape`
                // on an ordinary `set` instead of a compile error here.
                if (s.kv_sep == null and !@hasDecl(Lang, "insertKey") and !@hasDecl(Lang, "renderEntry"))
                    @compileError("Language declares kv_sep = null but neither hooks insertKey nor" ++
                        " declares renderEntry, so the generic entry-insert paths have no separator to write");
            }
        }

        // The closed set. `@typeInfo(...).decls` lists only PUBLIC
        // declarations, so a format's private helpers — the
        // `const edit = @import("editor_helper.zig")` a renderers block opens
        // with — are invisible here and need no exemption.
        for (@typeInfo(Lang).@"struct".decls) |d| {
            if (Decls.known(d.name)) continue;
            @compileError("Language '" ++ Lang.name ++ "' declares unknown '" ++ d.name ++ "'" ++
                if (Decls.nearest(d.name)) |near|
                    " — did you mean '" ++ near ++ "'?"
                else
                    ". A fragment renderer must be one of `Decls.renderers` in language.zig;" ++
                        " there are no editing hooks.");
        }

        // Coherence: a format that says it cannot be edited must not declare
        // editing behaviour. Without this, `caps.edit = false` and a live
        // renderer can disagree indefinitely — nothing else reads both.
        if (!Lang.caps.edit) {
            for (Decls.hooks ++ Decls.renderers) |name| {
                if (@hasDecl(Lang, name))
                    @compileError("Language '" ++ Lang.name ++ "' declares caps.edit = false" ++
                        " but supplies the editing renderer '" ++ name ++ "'");
            }
            return;
        }

        // The remaining rules are about hooks being REACHABLE. Both follow from
        // where `editor.zig` dispatches, so both are dead-code checks rather
        // than taste: a hook the engine can never call is a silent no-op, and
        // silent is the failure mode this whole section exists to remove.

        // The block-sequence item renderer sits below `editor.zig`'s
        // `block_seq_editable` refusal, so a format that declares no editable
        // block sequences in any dialect can never reach it.
        //
        // A runtime language has no dialect table to read here; its record
        // is held to the same rule by `runtime.validateVTable`.
        var any_block_seq = @hasDecl(Lang, "runtime");
        for (std.meta.tags(Lang.Type)) |t| {
            const s: Syntax = Lang.syntax(t);
            if (s.block_seq_editable) any_block_seq = true;
        }
        if (!any_block_seq) {
            for ([_][]const u8{"renderItem"}) |name| {
                if (@hasDecl(Lang, name))
                    @compileError("Language '" ++ Lang.name ++ "' declares block_seq_editable = false" ++
                        " but supplies '" ++ name ++ "', which the engine refuses before reaching");
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
    var out: []const type = &.{};
    for (slots) |slot| {
        if (gateOf(slot.name)) out = out ++ [_]type{slot.mod.Language};
    }
    break :blk out;
};

// Validate every compiled-in language, including the read-only ones that no
// `Editor()` instantiation would otherwise reach. Runs whenever this file is
// analyzed, which is whenever anything touches a format at all.
comptime {
    for (compiled) |Lang| validate(Lang);
}

// Test discovery: referencing each language module from a test block pulls
// its own `test {}` block — and through it every submodule's tests — into
// the suite, so `root.zig` names this file and no language.
test {
    inline for (slots) |slot| _ = slot.mod;
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
    if (comptime build_options.lang_toml) {
        try std.testing.expectEqual(Detected.toml, detect(a, "x = 1\n").?);
    }
    if (comptime build_options.lang_fig) {
        // A bare container header line (no `=`, no `:`, no brackets) followed
        // by a `>`-depth child isn't valid JSON/ZON/plist/TOML, so this resolves
        // to fig even though it's tried before YAML.
        try std.testing.expectEqual(Detected.fig, detect(a, "database\n> host = localhost\n").?);
    }
    if (comptime build_options.lang_ini) {
        // A `;`-led comment line is invalid JSON/ZON/plist/TOML (TOML has no `;`
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

test "sniff_order: the ranks the rows declare reproduce the probe order detect has always used" {
    // The whole sequence, written once, so a new format choosing a rank —
    // or an existing row changing its mind — has to change this line too.
    // Build-invariant: gated-out languages keep their rows and their ranks.
    const want = [_][]const u8{
        "json", "json5",  "zon",  "plist",      "toml",       "fig",
        "ini",  "dotenv", "yaml", "properties", "nestedtext",
    };
    try std.testing.expectEqual(want.len, sniff_order.len);
    for (want, sniff_order) |w, got| try std.testing.expectEqualStrings(w, got);
}

test "detect: plain `key = value` prefers TOML over fig despite fig accepting it too" {
    const a = std.testing.allocator;
    if (comptime !build_options.lang_toml or !build_options.lang_fig) return error.SkipZigTest;
    // fig's root-level dotted assignment accepts the exact same shape TOML
    // does; TOML is tried first, so it wins the tie.
    try std.testing.expectEqual(Detected.toml, detect(a, "x = 1\n").?);
}
