//! What a printer is asked for and how it can fail: the options every printer
//! takes and the errors it may return. The kit's, since a printer is a
//! language's; core's serializer dispatch (`serialize.zig`) passes them on.

const Writer = @import("std").Io.Writer;

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
    /// The caller asked for indentation (`fig fmt --indent`) rather than
    /// taking the default, which `indent`'s value alone cannot say: its
    /// default, 2, is also an explicit `--indent 2`. A format with no
    /// `pretty` gate on its indentation reads this instead — figl, whose
    /// canonical output is unindented and which, asked, prefixes each line
    /// with `2 × depth` cosmetic spaces ahead of the `> ` markers that carry
    /// its depth (docs/spec.md §3.3's "clean" convention). Every other format
    /// ignores it.
    indent_requested: bool = false,
    /// Fragment path only: render a container root inline, in *flow*
    /// (`[a, b]` / `{ k = v }`), rather than in a block spelling. The
    /// editors' splice paths set it, because a fragment spliced after
    /// `key = ` lands inline, and a block spelling that only parses as lines
    /// of its own (figl's `* ` items and section headers) re-reads there as
    /// a bare string. `false` (default) keeps a container root in block, as a
    /// caller that treats the output as a document needs. A format with one
    /// spelling for a container ignores it.
    flow: bool = false,
    /// Fragment path only: render the value as the editor takes it spliced
    /// into a document, not as a document of its own. A printer whose
    /// document differs from that declares `printSplice`: plist's drops the
    /// XML declaration, DOCTYPE and `<plist>` wrapper; NestedText's writes a
    /// scalar as its plain text, since the editor's renderers spell the `>`
    /// block where the value lands. Set by the bindings' editor splice
    /// paths and by `fig patch`; `false` (default) for a value serialized to
    /// be written out, which must stay a whole document.
    splice: bool = false,
};

/// The canonical set of ways serialization can fail
pub const SerializeError = Writer.Error || error{
    UnresolvedAlias, // an alias reached a printer with no reference layer (materialize first)
    NullUnsupported, // a `null` reached a format with no null type
    NonStringKey, // a mapping key was not a string, in a format whose keys are
    FormatDisabled, // the target format was compiled out of this build
    NestingTooDeep, // container nesting exceeded the canonical printer's depth guard
    UnexpectedNodeKind, // a node kind reached a printer path that expects a container
    UnrepresentableRoot, // the root is a kind the format has no document spelling for (a scalar, where it must be a container)
    UnsupportedValue, // a sequence or mapping value has no spelling where it sits
    InvalidKey, // a mapping key is not one the format can spell
    NonFiniteNumber, // an infinite or NaN float has no spelling in the format
};
