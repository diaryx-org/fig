//! CLI-only type definitions shared across the `fig` binary: the `Format`
//! enum, the `CliAction`/`CliActionOptions` action model, the in-place
//! `EditOp` union, and the small helper types (`CliConfig`, `ArgError`,
//! `Detected`, `append_index`) threaded through arg parsing and dispatch.
//! Nothing here touches `Io` or does any real work — see `args.zig` for
//! parsing, `actions.zig` for execution.
const std = @import("std");
const fig = @import("fig");

// gron is a CLI-only format: it lives here in the binary, never in the `fig`
// library, the C ABI, or `Language.detect`. It rides the `get` pipeline by
// deriving straight from the public AST (see `cli/gron.zig`).
const gron = @import("gron.zig");

const L = fig.Language;

// `gron` is a CLI-only output/echo format with no `AST.SerializeFormat`
// counterpart; the `get` handler intercepts it before the serializer dispatch.
// `canonical` (formerly `native`) is the AST's 1:1 oracle encoding, selectable
// only via `--input/--output canonical` — it owns no file extension. `fig` is
// the human-facing authoring dialect: it owns `.figl` (with `.fig` still
// accepted for back-compat), has a reader + `fig fmt`
// printer (see `get`), and `Editor(fig.Language.FIG)` wires `edit`/`set`/
// `insert`/`delete`/`comment` through the same span-splice engine as
// TOML/YAML/ZON (see `fig/editor_helper.zig`, which also carries the
// whole-container structural ops — `deleteContainer`/`moveContainer`/
// `reorderContainers`, the same declared ops TOML and INI supply for their own
// scattered containers; reachable from the C ABI and the Rust/TypeScript
// bindings as `fig_editor_*_container`, but not from this CLI, which has no
// verb for a whole container yet). `gron` is a CLI-only echo format with no
// `AST.SerializeFormat` counterpart.
///
/// NON-EXHAUSTIVE, as of cli 4.1: a value at or above `runtime_base` names a
/// language registered at runtime (`languages.zig`) — `runtime_base + i`
/// for the registry's entry `i` — and every switch over the enum has a `_`
/// arm for it, which reads the entry through `runtimeEntry`. The named
/// members are the compiled formats, and `@tagName` is only ever applied to
/// one of those; `name` is the spelling that covers both.
pub const Format = @Enum(u16, .nonexhaustive, format_names, &format_values);

const format_values = blk: {
    var values: [format_names.len]u16 = undefined;
    for (&values, 0..) |*v, i| v.* = @intCast(i);
    break :blk values;
};

/// The first `Format` value naming a runtime language. The same number as
/// the C ABI's `FIG_FORMAT_RUNTIME_BASE` — so the CLI's value for a runtime
/// entry is its ABI integer — though the two are separate enumerations (a
/// compiled CLI member is its registry position, not its ABI value).
pub const runtime_base: u16 = @intCast(fig.Language.runtime_abi_base);

/// The `Format` naming the registry entry `e`.
pub fn runtimeFormat(e: *const fig.Runtime.Entry) Format {
    return @enumFromInt(runtime_base + e.index);
}

/// The registry entry `f` names, or null for a compiled format.
pub fn runtimeEntry(f: Format) ?*const fig.Runtime.Entry {
    const v = @intFromEnum(f);
    if (v < runtime_base) return null;
    return fig.Runtime.entryAt(v - runtime_base);
}

/// What `f` is called: the member name of a compiled format, the registered
/// name of a runtime one. Use this wherever a format is printed.
pub fn name(f: Format) []const u8 {
    if (runtimeEntry(f)) |e| return e.name;
    return @tagName(f);
}

/// Every format registry entry (`languages/language.zig`'s `dialects`), in
/// registry order, plus the two members no `Language` backs: `canonical` after
/// `zon`, `gron` after `fig`. Those positions are not arbitrary — they are
/// where the hand-written enum this replaces put them (`canonical` followed
/// generic `xml` until core 3.0 removed it), and the member order is what
/// `@intFromEnum` and `main.zig`'s `--help` format list both read.
///
/// `yml` is deliberately NOT here: it used to be a member of its own, an alias
/// of `yaml` that duplicated it in ~ten switches and bought nothing but a
/// second spelling in `@tagName` echoes. It survives where it was actually
/// used — `--input yml` is an accepted spelling (`args.parseFormatName`), and
/// the `.yml` FILE extension resolves through `Language.YAML.extensions` —
/// and both now land on `.yaml` itself.
const format_names = blk: {
    @setEvalBranchQuota(20_000);
    break :blk L.namesWith(.all, &.{
        .{ .after = "zon", .name = "canonical" },
        .{ .after = "fig", .name = "gron" },
    });
};

// `namesWith` places the two non-registry members and would fail the build if
// either named a nonexistent entry to follow, so membership and registry order
// are true by construction. What that does NOT state is the intent — that
// `canonical` belongs beside `zon` and `gron` beside `fig` rather than merely
// somewhere — so that is what is left to check, plus the removal of `yml`.
comptime {
    if (@intFromEnum(Format.canonical) != @intFromEnum(Format.zon) + 1)
        @compileError("cli.Format's `canonical` no longer sits directly after `zon`");
    if (@intFromEnum(Format.gron) != @intFromEnum(Format.fig) + 1)
        @compileError("cli.Format's `gron` no longer sits directly after `fig`");
    if (@hasField(Format, "yml"))
        @compileError("`yml` is an accepted SPELLING of `yaml` (see `args.parseFormatName`)," ++
            " not a format of its own — a member here would resurrect the duplicated switch arms");
}

/// The `AST.SerializeFormat` counterpart of a CLI format, or null for gron (a
/// CLI-only projection with no serializer — the `get`/`fmt`/`convert` handlers
/// intercept it before reaching the serializer dispatch; see each call site's
/// own early return/`orelse unreachable`). gron's `.gron` maps to `.json` in
/// the callers that still need one (the lossless-envelope target switches);
/// every other member is identity.
pub fn toSerializeFormat(f: Format) ?fig.AST.SerializeFormat {
    return switch (f) {
        .json => .json,
        .jsonc => .jsonc,
        .json5 => .json5,
        .yaml => .yaml,
        .toml => .toml,
        .zon => .zon,
        .canonical => .canonical,
        .fig => .fig,
        .ini => .ini,
        .dotenv => .dotenv,
        .properties => .properties,
        .plist => .plist,
        .nestedtext => .nestedtext,
        // gron has no serializer; a runtime format prints through its
        // entry (`fig.Runtime.printWith`), which is not a `SerializeFormat`
        // either. Both are intercepted before the serializer dispatch.
        .gron, _ => null,
    };
}

pub const CliAction = enum {
    help,
    version,
    edit,
    set,
    insert,
    delete,
    get,
    comment,
    check,
    fmt,
    convert,
    patch,
    /// `fig lang list` / `fig lang check`: the languages the CLI did not
    /// compile in. See `LangOptions` and `cli/languages.zig`.
    lang,
    /// Not one of fig's own verbs: a word handed off to a `fig-<word>`
    /// executable on PATH. See `ExternalOptions`.
    external,
};

pub const LangOptions = struct {
    pub const Verb = enum { list, check, table };
    requested_help: bool = false,
    verb: Verb = .list,
    /// `check`'s language name; `table`'s file.
    name: []const u8 = "",
    /// `table -i <format>`: the format to read the file as, else its
    /// extension decides, else its contents.
    input: ?Format = null,
    /// `table --spec <version>`: the version of that format to read it
    /// as, where the format has one (`check`'s `--spec`).
    spec: ?[]const u8 = null,
    /// `check --against <compiled>`: the compiled format to hold it to.
    against: ?[]const u8 = null,
    /// `check`'s files, parsed by both and compared.
    files: []const []const u8 = &.{},
};

pub const HelpOptions = struct {
    requested_help: bool = false,
};

pub const VersionOptions = struct {};

/// How a value argument (`set`/`insert`/`edit`) is read — see `value_arg.zig`.
pub const ValueMode = enum {
    /// As a fig value: `5` a number, `hello` a string, `[1, 2]` a sequence.
    fig,
    /// `--string`: a string, whatever it looks like.
    string,
    /// `--raw`: the text itself, spliced as source in the file's format.
    raw,
};

pub const EditOptions = struct {
    file: []const u8,
    path: []fig.AST.PathSegment,
    replacement: []const u8,
    key: bool = false,
    value_mode: ValueMode = .fig,
    requested_help: bool = false,
    format: Format,
    /// Set when the format could not be inferred from the file extension:
    /// the handler then sniffs the file's contents with `Language.detect`.
    detect: bool = false,
    /// When set, `file` is a host document (e.g. markdown) and edits apply
    /// to the embedded config of this archetype, spliced back in place.
    embed: ?fig.Embed.Type = null,
    /// Set when `embed` couldn't be pinned by the extension (e.g. `.md`
    /// implies SOME embedded region but not which archetype): the handler
    /// sniffs the host content with `Embed.detect` (see `resolveEmbedType`).
    detect_embed: bool = false,
};

pub const SetOptions = struct {
    file: []const u8,
    /// The target. For a scalar upsert the last segment is the key to
    /// replace-or-create; for `--seq` it names the sequence to reconcile.
    path: []fig.AST.PathSegment,
    /// The value to upsert (unused when `seq` is set).
    value: []const u8,
    /// When set, reconcile the sequence at `path` to exactly `values`,
    /// preserving comments on survivors (the `set_sequence` primitive),
    /// instead of upserting a single scalar.
    seq: bool = false,
    values: []const []const u8 = &.{},
    value_mode: ValueMode = .fig,
    requested_help: bool = false,
    format: Format,
    detect: bool = false,
    /// When set, `file` is a host document and the upsert targets the
    /// embedded config of this archetype — creating the block (open-or-init)
    /// when the host has none.
    embed: ?fig.Embed.Type = null,
    /// As in `edit`: set when `embed` needs a runtime content sniff
    /// (`resolveEmbedType`) rather than being pinned by `--embed`.
    detect_embed: bool = false,
};

pub const InsertOptions = struct {
    file: []const u8,
    /// The destination *slot*, not an existing node: the last segment names
    /// what to create. A trailing key (`a.b.newkey`) inserts that key into
    /// the mapping at the parent path; a trailing index (`a.list[0]` /
    /// `a.list[-]`) prepends/appends to the sequence at the parent path. An
    /// empty parent means the root container, so the root's actual kind
    /// (mapping vs sequence) decides which applies — not the file format.
    path: []fig.AST.PathSegment,
    value: []const u8,
    value_mode: ValueMode = .fig,
    requested_help: bool = false,
    format: Format,
    /// Set when the format could not be inferred from the extension; the
    /// handler then sniffs the contents with `Language.detect`.
    detect: bool = false,
    /// As in `edit`: when set, edit the embedded config of this archetype.
    embed: ?fig.Embed.Type = null,
    /// As in `edit`: set when `embed` needs a runtime content sniff.
    detect_embed: bool = false,
};

pub const DeleteOptions = struct {
    file: []const u8,
    /// The node to remove. A trailing key deletes that mapping entry (with
    /// its owned leading comments); a trailing index removes that sequence
    /// item from the parent sequence.
    path: []fig.AST.PathSegment,
    requested_help: bool = false,
    format: Format,
    detect: bool = false,
    embed: ?fig.Embed.Type = null,
    /// As in `edit`: set when `embed` needs a runtime content sniff.
    detect_embed: bool = false,
};

pub const GetOptions = struct {
    file: []const u8,
    path: ?[]fig.AST.PathSegment = null,
    from: Format,
    to: Format,
    requested_help: bool = false,
    /// Set when `from` could not be inferred from the file extension and no
    /// `--input` was given: the handler sniffs the contents with
    /// `Language.detect`. When `to` was also left to default (`output_explicit`
    /// is false), the detected format flows through to the output too.
    detect: bool = false,
    /// Whether `--output`/`-o` was given. When false and `detect` fires, the
    /// detected input format becomes the output format (echo round-trip).
    output_explicit: bool = false,
    /// When converting YAML to another format, drop unknown/custom tags
    /// instead of erroring on them. Has no effect on parsing or YAML→YAML.
    lax_tags: bool = false,
    /// Lossless conversion: preserve values the target format can't represent
    /// natively (a null in TOML, a TOML datetime in JSON, …) through a `$fig`
    /// envelope, and reconstruct any such envelope found in the input. Gates
    /// both the encode (output) and decode (input) passes; default is lossy.
    lossless: bool = false,
    /// When set, the input is extracted from a host document of this
    /// archetype (e.g. YAML frontmatter inside markdown) before parsing.
    embed: ?fig.Embed.Type = null,
    /// As in `edit`: set when `embed` needs a runtime content sniff
    /// (`resolveEmbedType`) rather than being pinned by `--embed`.
    detect_embed: bool = false,
    /// When set, print the host *body* (the prose outside the fences) of the
    /// embed archetype instead of converting its content. Demonstrates the
    /// region's `body` span; ignored when there is no embed.
    body: bool = false,
    /// Output style. `--compact` clears `pretty` for a single-line render;
    /// `--indent N` sets the indent width; `--width N` sets TOML's inline-vs-
    /// expanded column budget. Honored by JSON (pretty + indent), ZON (pretty),
    /// and TOML (pretty gates array wrapping; indent/width drive its layout);
    /// YAML renders with its own fixed layout.
    serialize: fig.AST.SerializeOptions = .{},
    /// Suppress the lossy-conversion warnings normally written to stderr.
    quiet: bool = false,
    /// Treat any lossy conversion as an error: print the warnings, then exit
    /// non-zero without writing output.
    strict: bool = false,
    /// Syntax knobs for `-o gron` (root name, key/value separator, terminator).
    /// Defaults reproduce gron exactly; ignored unless the output is gron.
    gron_projection: gron.Projection = .gron,
};

pub const CommentOptions = struct {
    file: []const u8,
    path: []fig.AST.PathSegment,
    text: []const u8,
    /// When set, target the same-line trailing comment on the value at
    /// `path`; otherwise the own-line comment block above the node.
    inline_comment: bool = false,
    /// When set, delete the targeted comment instead of adding/setting it
    /// (then `text` is unused).
    delete: bool = false,
    /// When set, print the targeted comment to stdout instead of editing it
    /// (then `text` is unused, and the file is opened read-only).
    get: bool = false,
    requested_help: bool = false,
    format: Format,
    /// Set when the format could not be inferred from the file extension:
    /// the handler then sniffs the file's contents with `Language.detect`.
    detect: bool = false,
    /// As in `edit`: when set, `file` is a host document and the comment is
    /// applied to the embedded config of this archetype, spliced back.
    embed: ?fig.Embed.Type = null,
    /// As in `edit`: set when `embed` needs a runtime content sniff.
    detect_embed: bool = false,
};

pub const CheckOptions = struct {
    /// One or more files to validate. `-` reads stdin (single document).
    files: [][]const u8,
    /// Explicit `--input` format applied to every file. When null, each
    /// file's format is resolved from its extension, then by sniffing its
    /// contents — the same precedence `get` uses.
    format: ?Format = null,
    /// `--spec` version string (e.g. "1.0" for TOML). Resolved per file
    /// against the resolved format; null validates against the default
    /// version of each format.
    spec: ?[]const u8 = null,
    /// Suppress the per-file `ok` lines on success; errors still print.
    quiet: bool = false,
    requested_help: bool = false,
};

pub const FmtOptions = struct {
    /// The file to reformat in place. `-` reads stdin — only valid with
    /// `dry_run` (there is nowhere to write an in-place result back to).
    file: []const u8,
    /// The single format `fmt` parses AND re-emits — unlike `get`, there is
    /// no `--output`: reformatting never changes the document's format.
    from: Format,
    requested_help: bool = false,
    /// Set when `from` could not be inferred from the file extension and no
    /// `--input` was given: the handler sniffs the contents with
    /// `Language.detect`.
    detect: bool = false,
    /// Output style — see `get`'s twin field.
    serialize: fig.AST.SerializeOptions = .{},
    /// Suppress the lossy-conversion (e.g. `--strip-comments`) and fig
    /// authoring-lint warnings normally written to stderr.
    quiet: bool = false,
    /// Treat any warning as an error (exit non-zero without writing).
    strict: bool = false,
    /// Print the reformatted result to stdout instead of writing it back,
    /// and exit 1 if reformatting would change the file (0 if already
    /// clean) — the CI-friendly "would this file's formatting change" gate.
    dry_run: bool = false,
    /// Like `dry_run`, but print a unified diff of the change instead of
    /// the whole reformatted file (nothing is written either way).
    diff: bool = false,
    /// When set, `file` is a host document (e.g. markdown) and only its
    /// embedded region is reformatted, spliced back in place.
    embed: ?fig.Embed.Type = null,
    /// As in `get`: set when `embed` needs a runtime content sniff
    /// (`resolveEmbedTypeFromContent`) rather than being pinned by `--embed`.
    detect_embed: bool = false,
};

pub const ConvertOptions = struct {
    /// The file to convert. `-` reads stdin — only valid without `--write`
    /// (there is nowhere to write an in-place result back to).
    file: []const u8,
    requested_help: bool = false,
    /// Whole-file mode (`--output`): parse as `from`, re-emit as `to`.
    /// Mutually exclusive with the embed-archetype mode (`to_embed`) — one
    /// of the two must be set, checked in `parseConfig`.
    from: Format = .json,
    to: Format = .json,
    /// Set when `from` couldn't be pinned by `--input`/the file extension:
    /// the handler sniffs the contents with `Language.detect`, mirroring
    /// `fmt`/`get`.
    detect: bool = false,
    /// Embed-archetype mode (`--to-embed <archetype>`): rehouse a host
    /// document's embedded region from one archetype's fence-and-format
    /// convention to another's (e.g. YAML frontmatter → JSON frontmatter),
    /// splicing the new fences + re-serialized content in place while
    /// leaving the host prose (`Embed.Region.body`) byte-identical. `to`/
    /// `from`/`detect` are unused in this mode; the archetypes fix both
    /// formats.
    to_embed: ?fig.Embed.Type = null,
    /// The source archetype for embed-archetype mode: `--embed`, else —
    /// when `detect_embed` is set — sniffed from the content with
    /// `Embed.detect` (the extension alone, e.g. `.md`, only tells us an
    /// embed is likely present, never which archetype it is).
    embed: ?fig.Embed.Type = null,
    /// Set when `embed` couldn't be pinned by `--embed` and `to_embed` is
    /// set: the handler sniffs the host content with `Embed.detect`.
    detect_embed: bool = false,
    /// As in `get`: drop unknown/custom YAML tags instead of erroring,
    /// when converting away from YAML.
    lax_tags: bool = false,
    /// As in `get`: preserve values the target can't represent natively
    /// through a `$fig` envelope, and decode any such envelope on input.
    lossless: bool = false,
    serialize: fig.AST.SerializeOptions = .{},
    quiet: bool = false,
    strict: bool = false,
    /// Write the converted result back to `file` in place (skipped when the
    /// bytes are already identical). Without this, `convert` never touches
    /// disk — it just prints, like `get`. Combinable with `diff`: writes the
    /// file AND prints the unified diff of what changed.
    write: bool = false,
    /// Print a unified diff of the change instead of the whole converted
    /// file. Independent of `write` — with neither flag, the whole converted
    /// document prints to stdout.
    diff: bool = false,
};

pub const PatchOptions = struct {
    /// The document being patched, and the only one written. `-` reads stdin —
    /// only valid with `--dry-run`/`--diff`, there being nowhere to write an
    /// in-place result back to. Must already exist: `patch` merges into a
    /// document, it does not seed one (that is `set`'s job).
    file: []const u8,
    /// The document supplying the change. `-` reads stdin (so `fig get a.toml
    /// service | fig patch b.yaml -` works); at most one of the two may.
    patch_file: []const u8,
    /// Where in the target the patch lands (`--at`). Empty is the root.
    at: []fig.AST.PathSegment = &.{},
    /// Which subtree of the patch document to take (`--from`). Empty is its
    /// root.
    from: []fig.AST.PathSegment = &.{},
    /// Paths to remove from the target after the merge (`--delete`, repeatable).
    deletes: []const fig.Patch.Deletion = &.{},
    /// How the merge behaves — sequence strategy, comment strategy, and the
    /// serialize knobs patch subtrees are rendered with.
    patch_options: fig.Patch.Options = .{},
    requested_help: bool = false,
    /// The TARGET's format. As everywhere else: `--input`, else the extension,
    /// else (with `detect`) a content sniff.
    format: Format,
    detect: bool = false,
    /// The PATCH document's format (`--patch-input`), resolved the same way
    /// against its own extension — hence its own `detect` twin.
    patch_format: Format,
    detect_patch: bool = false,
    /// When set, the target is a host document and the merge applies to its
    /// embedded config of this archetype, spliced back in place.
    embed: ?fig.Embed.Type = null,
    /// As in `edit`: set when `embed` needs a runtime content sniff.
    detect_embed: bool = false,
    /// The same pair for the PATCH document, so one post's frontmatter can be
    /// merged into another's.
    patch_embed: ?fig.Embed.Type = null,
    detect_patch_embed: bool = false,
    /// Preserve values the target format can't represent natively through a
    /// `$fig` envelope, and decode any envelope the patch document carries.
    /// Same flag, same meaning, as `get`/`convert`.
    lossless: bool = false,
    /// Print the patched document to stdout instead of writing it back.
    dry_run: bool = false,
    /// Print a unified diff of the change instead of writing it back.
    /// Unlike `fmt`'s, neither preview mode sets a non-zero exit status: a
    /// patch is EXPECTED to change the file, so "it changed" is not a failure.
    diff: bool = false,
    /// Suppress the summary line and the dropped-comment warning.
    quiet: bool = false,
};

/// A subcommand fig has no action of its own for, on its way to the
/// git-style `fig <name>` → `fig-<name>` handoff — see `external.zig`, which
/// is the only consumer. `parseConfig` only builds this for a word that could
/// name an executable at all (see `externalCommandName`), so `name` is never
/// a path, a flag, or empty.
pub const ExternalOptions = struct {
    /// The word as the user typed it (`schema` in `fig schema get f.json`).
    /// Kept alongside `program` for the not-found report, which talks about
    /// what was typed rather than what was looked up.
    name: []const u8,
    /// `fig-<name>` — the executable to look for on PATH. Always spelled
    /// `fig-`, never after `binary_name`: a renamed or path-qualified argv[0]
    /// (`./zig-out/bin/fig`) would otherwise ask for a sibling nothing ships.
    ///
    /// Null when `name` is a word that could not name an executable at all
    /// (`fig config.toml`, `fig --colour`), which is the ordinary "no such
    /// action" case wearing this union's clothes: there is nothing to look
    /// up, and `argv` is empty.
    program: ?[]const u8,
    /// The full argv to hand over: `program`, then every argument after the
    /// subcommand word, verbatim and unparsed. fig deliberately does not read
    /// them — the flags after `fig schema` belong to `fig-schema`, including
    /// the ones fig itself would recognize.
    argv: []const []const u8,
};

pub const CliActionOptions = union(CliAction) {
    help: HelpOptions,
    version: VersionOptions,
    edit: EditOptions,
    set: SetOptions,
    insert: InsertOptions,
    delete: DeleteOptions,
    get: GetOptions,
    comment: CommentOptions,
    check: CheckOptions,
    fmt: FmtOptions,
    convert: ConvertOptions,
    patch: PatchOptions,
    lang: LangOptions,
    external: ExternalOptions,
};

/// The in-place editing operation `applyEdit` performs. Generalizes the editor's
/// span-splice surface so `edit` and `comment` share one code path.
pub const EditOp = union(enum) {
    replace_value,
    replace_key,
    add_leading_comment,
    set_trailing_comment,
    delete_leading_comments,
    delete_trailing_comment,
    /// Insert `key: text` into the mapping at `path`. The payload is the new
    /// key's text; the value rides in `applyEdit`'s `text` argument.
    insert_key: []const u8,
    /// Upsert the value at `path`: replace it, or insert the trailing key when
    /// only it is absent. `text` is the value; `path` ends in the key.
    set,
    /// Reconcile the sequence at `path` to exactly `items`, preserving the
    /// comments on items that survive (`text` unused).
    set_sequence: []const []const u8,
    /// Append `text` as a new last item to the sequence at `path`.
    append_seq,
    /// Insert `text` as the new first item of the sequence at `path`.
    prepend_seq,
    /// Delete the mapping entry named by `path` (text unused).
    delete_key,
    /// Remove the item at this index from the sequence at `path` (text unused).
    remove_seq_item: usize,
};

/// Sentinel sequence index meaning "the end" — produced by `parsePath` for the
/// `[-]`/`[$]` append tokens and consumed by the `insert` handler to pick
/// `append_seq` over `prepend_seq`. Out of range for any real index, so it never
/// collides with an addressable item.
pub const append_index = std.math.maxInt(usize);

pub const CliConfig = struct {
    action: CliAction = .help,
    options: CliActionOptions = .{ .help = .{} },
    binary_name: []const u8 = "fig",
    requested_help: bool = false,
};

/// The caller-supplied text an action splices into the document, plus what it
/// takes to report it: which file it was going into, and which format that
/// file is (null under `--detect`, where only the handler resolves it). `text`
/// is null when the action carries several (`set --seq`). Read only by
/// `main`'s `error.InvalidEditText` path — see `diag_report.reportBadEditText`.
pub const SplicedText = struct {
    file: []const u8,
    format: ?Format,
    kind: EditTextKind,
    text: ?[]const u8,
};

/// What a piece of spliced text was meant to be — only ever used to word the
/// report in `diag_report.reportBadEditText` ("the new value" vs "the new key").
pub const EditTextKind = enum {
    /// A value argument, read as a fig value (or with `--string`) and
    /// rendered in the file's own spelling.
    value,
    /// A `--raw` value argument, spliced as the user typed it.
    raw_value,
    key,
    comment,

    pub fn noun(self: EditTextKind) []const u8 {
        return switch (self) {
            .value, .raw_value => "value",
            .key => "key",
            .comment => "comment text",
        };
    }
};

/// The spliced text `config`'s action carries, or null for the actions that
/// splice none (`delete`, and the read-only ones) — those can't produce an
/// `InvalidEditText` in the first place.
pub fn splicedText(config: CliConfig) ?SplicedText {
    return switch (config.options) {
        .edit => |o| .{
            .file = o.file,
            .format = if (o.detect) null else o.format,
            // `--key` makes the argument a replacement KEY, not a value.
            .kind = if (o.key) .key else valueKind(o.value_mode),
            .text = o.replacement,
        },
        .set => |o| .{
            .file = o.file,
            .format = if (o.detect) null else o.format,
            .kind = valueKind(o.value_mode),
            .text = if (o.seq) null else o.value,
        },
        .insert => |o| .{
            .file = o.file,
            .format = if (o.detect) null else o.format,
            .kind = valueKind(o.value_mode),
            .text = o.value,
        },
        .comment => |o| .{
            .file = o.file,
            .format = if (o.detect) null else o.format,
            .kind = .comment,
            .text = o.text,
        },
        else => null,
    };
}

fn valueKind(mode: ValueMode) EditTextKind {
    return if (mode == .raw) .raw_value else .value;
}

/// The single file `config`'s action works on, when it has exactly one — so a
/// failure that escapes the action can at least name it (see
/// `diag_report.reportUnhandled`). Null for `check`, which takes a list and
/// reports per file itself, and for the file-less actions.
pub fn targetFile(config: CliConfig) ?[]const u8 {
    return switch (config.options) {
        .edit => |o| o.file,
        .set => |o| o.file,
        .insert => |o| o.file,
        .delete => |o| o.file,
        .get => |o| o.file,
        .comment => |o| o.file,
        .fmt => |o| o.file,
        .convert => |o| o.file,
        // The TARGET, not the patch document: it is the file being written,
        // so it is the one a failure has to name.
        .patch => |o| o.file,
        // `external` never touches a file itself — whatever it does with its
        // arguments is the other binary's business, and its failures are its
        // own to report.
        .help, .version, .check, .lang, .external => null,
    };
}

/// The single file `config`'s action parses, and the format to parse it as —
/// null where the action resolves that itself (`--detect` sniffing, or an
/// embedded region, whose host format is the extension's to decide). What
/// `main` hands `parse_dispatch.checkOne` to locate a parse failure that
/// escaped the action unreported. Null for stdin, which cannot be read twice,
/// and for everything `targetFile` is null for.
pub const ParseTarget = struct { file: []const u8, format: ?Format };

pub fn parseTarget(config: CliConfig) ?ParseTarget {
    const t: ParseTarget = switch (config.options) {
        inline .edit, .set, .insert, .delete, .comment, .patch => |o| .{
            .file = o.file,
            .format = if (o.detect or o.embed != null or o.detect_embed) null else o.format,
        },
        inline .get, .fmt, .convert => |o| .{
            .file = o.file,
            .format = if (o.detect or o.embed != null or o.detect_embed) null else o.from,
        },
        .help, .version, .check, .lang, .external => return null,
    };
    if (std.mem.eql(u8, t.file, "-")) return null;
    return t;
}

pub const ArgError = error{ UnsupportedFileFormat, MissingEditArgument, MissingSetArgument, MissingInsertArgument, MissingDeleteArgument, MissingGetArgument, MissingCommentArgument, MissingCheckArgument, MissingFmtArgument, MissingConvertArgument, MissingPatchArgument, MissingLangArgument, OutOfMemory, Overflow, InvalidCharacter, InvalidPath };

/// Result of mapping a file extension to a parse strategy. `embed_detect` is
/// set when the file is a host document whose config lives in an embedded
/// region (currently only `.md`/`.markdown`) — but the extension alone can't
/// say which archetype it is (YAML/JSON/fig frontmatter, YAML endmatter all
/// use different fences), so the caller still has to sniff the actual bytes
/// with `Embed.detect` (see `resolveEmbedType`/`resolveEmbedTypeFromContent`)
/// rather than assuming one outright. `format` describes the whole-file parse
/// strategy for the (rarer) case where there turns out to be no embed at all.
pub const Detected = struct {
    format: Format,
    embed_detect: bool = false,
};
