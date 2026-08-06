//! Serialization — the canonical output formats, their options/errors, and the
//! `serialize*` entry points that dispatch to each format's printer. The public
//! functions here are re-exported as `AST` methods from `ast.zig`.

const std = @import("std");
const Writer = std.Io.Writer;
const build_options = @import("build_options");

const AST = @import("ast.zig");
const Node = AST.Node;
/// The format registry. `SerializeFormat`'s members come from it, and so does
/// every dispatch arm below: a registry entry names the language whose
/// `Printer` module prints the dialect (`entry.Lang.Printer`) and the two
/// declarations in it that do the printing (`entry.print_name`,
/// `entry.print_node_name`). No printer is imported by path here any more —
/// `canonical`'s is the one exception, since canonical is not a `Language`.
const Language = @import("../languages/language.zig");

// Printers reach this file only through the registry, and only for the formats
// compiled into this build: a gated-out language collapses its entry's `Lang`
// to `void`, the void guard that opens every arm below returns
// `error.FormatDisabled` before anything reads `Lang.Printer`, and the printer
// never compiles.
//
// The canonical form is the AST's own 1:1 oracle encoding. It is not exposed
// through the C ABI or any binding, so it is opt-in (`-Dcanonical=true`) like
// xml — but ALWAYS compiled for a test build (`is_test`), since the suite leans
// on it as a comparison oracle. When gated out, `CanonicalPrinter` is `void`
// and the guarded arms below are never analyzed, so its code never compiles in.
const canonical_enabled = build_options.lang_canonical or @import("builtin").is_test;
const CanonicalPrinter = if (canonical_enabled) @import("../canonical/printer.zig") else void;

/// The canonical output format families. `canonical` (formerly `native`) is the
/// AST's own 1:1 oracle encoding; `fig` is the human-facing authoring dialect
/// (lossy at the edges — see src/languages/fig/DESIGN.md). `xml` requires its
/// AST root to be a one-entry mapping (see `languages/xml/printer.zig`'s
/// header) — anything else is `RootNotSingleElement`, not a silent fallback.
pub const SerializeFormat = @Enum(
    Language.EnumTag(serialize_format_names),
    .exhaustive,
    serialize_format_names,
    &Language.enumValues(serialize_format_names),
);

/// Every format registry entry (`languages/language.zig`'s `dialects`), in
/// registry order, plus `canonical` directly after `xml` — where the
/// hand-written enum this replaces put it. `canonical` is not a `Language` at
/// all (no parser module, no dialect, an options-less printer), so it stays an
/// explicit named arm in every switch below and is spliced in by hand here.
///
/// Unlike `cli.Format` there is no `gron`: gron is a CLI-only projection with
/// no printer on this side, and its callers intercept it before the serializer
/// dispatch (see `cli/types.zig`'s `toSerializeFormat`).
const serialize_format_names = blk: {
    @setEvalBranchQuota(20_000);
    break :blk Language.namesWith(.all, &.{.{ .after = "xml", .name = "canonical" }});
};

// Membership and registry order are true by construction (`namesWith` fails
// the build if `canonical` names an entry that does not exist to follow); what
// is left to state is that `canonical` belongs beside `xml` specifically, not
// merely somewhere in the list.
comptime {
    if (@intFromEnum(SerializeFormat.canonical) != @intFromEnum(SerializeFormat.xml) + 1)
        @compileError("AST.SerializeFormat's `canonical` no longer sits directly after `xml`");
}

/// Knobs controlling how a value is rendered. The defaults reproduce fig's
/// historical output (pretty-printed, two-space indent), so `.{}` is a no-op
/// change for existing callers.
///
/// Honored where each setting is meaningful:
///   * `pretty` — JSON/JSON5 (multi-line vs. minified) and ZON (`zig fmt`
///     multi-line vs. inline `.{ a, b }`). TOML uses it to gate array wrapping
///     (`true`: wrap arrays wider than `width`; `false`: keep every array on one
///     line). YAML ignores it (its compact flow style is not yet emitted).
///   * `indent` — JSON/JSON5 spaces per level, and the per-level indent of TOML's
///     wrapped arrays. ZON keeps its idiomatic four-space block indent; YAML has
///     its own fixed layout.
///   * `width` — TOML, YAML, and fig: the column budget that decides whether a
///     container renders inline (TOML's inline table `k = { ... }` / a one-line
///     array; YAML's flow `k: {a: 1}` / `[a, b]`; fig's flow spellings) or
///     expands (a `[section]`, a wrapped array, block lines). A value that fits
///     within `width` stays inline; one that exceeds it expands. Two things to
///     know for YAML: the budget applies to NESTED containers only — a root
///     mapping/sequence always renders block — and `0` is not "never inline"
///     but the C ABI's "unset" sentinel (see `FigSerializeOptions.width`),
///     which resolves back to 80. Use `1` to force block layout.
///   * `strip_comments` — every format: drop the AST's carried comments instead
///     of emitting them. Honored uniformly (it blanks the comment side-table
///     before printing), so it works even for the formats whose printers take no
///     options.
pub const SerializeOptions = struct {
    /// `true`: multi-line, indented output. `false`: compact single-line output
    /// with no insignificant whitespace.
    pretty: bool = true,
    /// Spaces per indentation level when `pretty` is set (JSON; TOML wrapped
    /// arrays).
    indent: u8 = 2,
    /// Column budget for the inline-vs-expanded layout decision of TOML, YAML,
    /// and fig. Anything that renders within this many columns stays inline;
    /// wider values expand to sections / wrapped arrays / block lines. Ignored by
    /// the other formats. `1` is the way to say "always block"; `0` reads as
    /// "unset" across the C ABI and resolves to 80.
    width: u16 = 80,
    /// `true`: do not emit comments carried on the AST (a clean, comment-free
    /// render). `false` (default): preserve them where the target format allows.
    strip_comments: bool = false,
    /// fig only, opt-in (`fig fmt --indent`): prefix each marker/comment line
    /// with `2 × depth` literal spaces of cosmetic indentation on top of the
    /// spaced `> ` marker runs that alone carry parse depth (docs/spec.md §
    /// 3.3's "clean" convention — indentation that agrees with `2 × depth`
    /// reparses with no `indent_marker_mismatch` warning and doesn't change
    /// the AST). Every other format instead treats `indent` as the width to
    /// use whenever it already indents, gated by `pretty` (JSON/JSON5's
    /// multi-line mode; TOML's wrapped arrays) — fig has no such `pretty` gate
    /// of its own (its zero-indent house style holds regardless of `pretty`),
    /// and its cosmetic indentation is a fixed 2-per-depth overlay rather than
    /// a configurable width, so `indent`'s numeric value can't double as
    /// fig's on/off signal the way it does for those formats (its default,
    /// 2, is not distinguishable from an explicit `--indent 2`). `false`
    /// (default): canonical, zero-indent output — unchanged from before this
    /// field existed.
    fig_indent: bool = false,
    /// fig only, fragment path only: render a container root as inline *flow*
    /// (`[a, b]` / `{ k = v }`) instead of the block spelling. The editors'
    /// splice path sets this: a fragment spliced after `key = ` has NO valid
    /// block spelling in the fig dialect (`* ` element lines and section
    /// headers only parse as standalone lines — a block sequence spliced
    /// inline re-reads as a bare string), so flow is the only spelling that
    /// survives the round-trip. `false` (default): unchanged behavior —
    /// `serializeFragmentWith(.fig)` keeps rendering a mapping root as block
    /// sections (callers do treat that output as a whole document, which flow
    /// would break). Ignored by every other format and by the document path.
    flow: bool = false,
};

/// `self`, or a comment-stripped *view* of it when `options.strip_comments` is
/// set. The view is a shallow struct copy that shares all node/string storage and
/// only blanks `node_comments`, so stripping costs no allocation. `buf` provides
/// the view's stack storage; the returned pointer is valid for `buf`'s lifetime.
fn commentView(self: *const AST, options: SerializeOptions, buf: *AST) *const AST {
    if (!options.strip_comments) return self;
    buf.* = self.*;
    buf.node_comments = &.{};
    return buf;
}

/// The canonical set of ways serialization can fail
pub const SerializeError = Writer.Error || error{
    UnresolvedAlias, // a YAML `*alias` reached a non-YAML printer (materialize first)
    NullUnsupported, // a `null` reached a format with no null type (TOML)
    NonStringKey, // a mapping key was not a string (TOML, ZON, XML)
    FormatDisabled, // the target format was compiled out of this build
    NestingTooDeep, // container nesting exceeded the canonical printer's depth guard
    RootNotSingleElement, // XML: the AST root was not a one-entry mapping
    NestedSequenceUnsupported, // XML: a sequence with no element name to expand under
    InvalidElementName, // XML: a mapping key is not a valid XML `Name`
    NonScalarValue, // XML: an `@`-attribute or `#text` entry held a mapping/sequence
    UnexpectedNodeKind, // fig: a node kind reached a printer path that expects a container
    FigUnrepresentableRoot, // fig: a scalar/null value has no authoring spelling as a document root
    UnsupportedValue, // INI/dotenv: a sequence/mapping value has no spelling there
    InvalidKey, // dotenv: a mapping key is not a valid bash identifier
};

/// Render the whole AST to `writer` in the given format, using default options.
/// Does not handle aliases, tags, or lossless `$fig` envelopes.
pub fn serialize(self: *const AST, writer: *Writer, format: SerializeFormat) SerializeError!void {
    return self.serializeWith(writer, format, .{});
}

/// Render the whole AST to `writer`, controlling output style via `options`.
pub fn serializeWith(self: *const AST, writer: *Writer, format: SerializeFormat, options: SerializeOptions) SerializeError!void {
    @setEvalBranchQuota(30_000);
    var buf: AST = undefined;
    const ast = commentView(self, options, &buf);
    return switch (format) {
        // The AST's own 1:1 oracle encoding — not a `Language`, so not a
        // registry entry, so its own arm. Its printer also takes no options
        // (the canonical form has exactly one spelling).
        .canonical => if (comptime canonical_enabled) CanonicalPrinter.print(writer, ast) else error.FormatDisabled,
        inline else => |f| {
            const d = comptime Language.entryFor(@tagName(f));
            if (comptime d.Lang == void) return error.FormatDisabled;
            // `print_name` is what separates the three JSON dialects, which
            // share one printer module and differ only by entry point
            // (`print`/`printc`/`print5`), and what routes YAML to its
            // options-taking `printWith`.
            return @field(d.Lang.Printer, d.print_name)(writer, ast, options);
        },
    };
}

/// Render the whole AST to `writer` as a value *fragment*, controlling output
/// style via `options`. Identical to `serializeWith` for every format except
/// `fig`: JSON/YAML/ZON/canonical already treat a scalar/null root as a fine
/// value to render (e.g. `9090`), and TOML falls back to an inline fragment —
/// but fig's document printer (`fig/printer.zig`'s `print`, which
/// `serializeWith` reaches) deliberately errors `FigUnrepresentableRoot` on a
/// bare scalar/null root, since a *whole fig document* (`fig fmt`, `fig get`,
/// `fig_document_serialize`) can't be spelled that way. A value fragment built
/// by the caller (`fig_value_serialize_opts`, backing the editors'
/// `replace`/`set`) is never asked to stand alone as a document — it's spliced
/// into existing source — so it uses that module's `printFragment` instead,
/// which allows that root.
pub fn serializeFragmentWith(self: *const AST, writer: *Writer, format: SerializeFormat, options: SerializeOptions) SerializeError!void {
    @setEvalBranchQuota(30_000);
    var buf: AST = undefined;
    const ast = commentView(self, options, &buf);
    return switch (format) {
        .canonical => if (comptime canonical_enabled) CanonicalPrinter.print(writer, ast) else error.FormatDisabled,
        // The one format whose fragment entry point differs from its document
        // one — see this function's doc comment for why. The registry's
        // `print_name` names the DOCUMENT printer, which is the right entry
        // point for every other dialect here (including YAML, whose
        // `printWith` serves both), so fig takes an explicit arm rather than
        // the registry carrying a second name.
        .fig => {
            const d = comptime Language.entryFor("fig");
            if (comptime d.Lang == void) return error.FormatDisabled;
            return d.Lang.Printer.printFragment(writer, ast, options);
        },
        inline else => |f| {
            const d = comptime Language.entryFor(@tagName(f));
            if (comptime d.Lang == void) return error.FormatDisabled;
            return @field(d.Lang.Printer, d.print_name)(writer, ast, options);
        },
    };
}

/// Render the subtree rooted at `id` to `writer`, using default options.
pub fn serializeNode(self: *const AST, writer: *Writer, format: SerializeFormat, id: Node.Id) SerializeError!void {
    return self.serializeNodeWith(writer, format, id, .{});
}

/// Render the subtree rooted at `id`, controlling output style via `options`.
pub fn serializeNodeWith(self: *const AST, writer: *Writer, format: SerializeFormat, id: Node.Id, options: SerializeOptions) SerializeError!void {
    @setEvalBranchQuota(30_000);
    var buf: AST = undefined;
    const ast = commentView(self, options, &buf);
    return switch (format) {
        .canonical => if (comptime canonical_enabled) CanonicalPrinter.printNode(writer, ast, id, 0) else error.FormatDisabled,
        inline else => |f| {
            const d = comptime Language.entryFor(@tagName(f));
            if (comptime d.Lang == void) return error.FormatDisabled;
            // `printNode` on the printer MODULE, which every format has —
            // including plist and xml, whose `Language` declares no `printNode`
            // of its own (their `print` is written inline; see `Decls.optional`
            // in languages/language.zig).
            return @field(d.Lang.Printer, d.print_node_name)(writer, ast, id, 0, options);
        },
    };
}
