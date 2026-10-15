//! Port of src/cli/types.zig.
//!
//! CLI-only type definitions shared across the `fig` binary: the `Format`
//! enum, the `CliActionOptions` action model, the in-place
//! `EditOp` union, and the small helper types (`CliConfig`, `Detected`,
//! `APPEND_INDEX`) threaded through arg parsing and dispatch.
//! Nothing here touches I/O or does any real work — see `args.rs` for
//! parsing, `actions.rs` for execution.

use std::sync::Arc;

use fig_core::kit::ast::{PathSegment, SerializeOptions};
use fig_core::patch;
use fig_core::registry::{self, Entry};
use fig_core::serialize::Target;

// gron is a CLI-only format: it lives here in the binary, never in the `fig`
// library, the C ABI, or `Language.detect`. It rides the `get` pipeline by
// deriving straight from the public AST (see `cli/gron.rs`).
use crate::gron;

/// What a CLI command reads or writes: `canonical`, the AST's 1:1 oracle
/// encoding (selectable only by name — it owns no file extension); `gron`, a
/// CLI-only line projection the `get` handler intercepts before any printer;
/// or a registered dialect — `RUNTIME_BASE + i` for the registry's entry `i`
/// (`languages.rs` registers the bundled ones at startup). Every match over
/// it has an arm for the registered dialects, which reads the entry through
/// `runtime_entry`; `name` is the spelling that covers all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Format(pub u16);

impl Format {
    pub const CANONICAL: Format = Format(0);
    pub const GRON: Format = Format(1);
}

/// The first `Format` value naming a registered dialect.
pub const RUNTIME_BASE: u16 = 16;

/// The `Format` naming the registry entry `e`.
pub fn runtime_format(e: &Entry) -> Format {
    Format(RUNTIME_BASE + e.index)
}

/// The registry entry `f` names, or `None` for `canonical` and `gron`.
pub fn runtime_entry(f: Format) -> Option<Arc<Entry>> {
    let v = f.0;
    if v < RUNTIME_BASE {
        return None;
    }
    registry::global().entry_at((v - RUNTIME_BASE) as usize)
}

/// What `f` is called: `canonical`, `gron`, or the registered dialect's
/// name. Use this wherever a format is printed.
pub fn name(f: Format) -> String {
    match f {
        Format::CANONICAL => "canonical".to_string(),
        Format::GRON => "gron".to_string(),
        _ => match runtime_entry(f) {
            Some(e) => e.name.clone(),
            None => "?".to_string(),
        },
    }
}

/// The format called `n` — `canonical`, `gron` or a registered dialect — or
/// `None`.
pub fn named(n: &str) -> Option<Format> {
    if n == "canonical" {
        return Some(Format::CANONICAL);
    }
    if n == "gron" {
        return Some(Format::GRON);
    }
    let e = registry::global().entry_by_name(n)?;
    Some(runtime_format(&e))
}

/// `named(n)`, which the caller knows is registered — a test's format.
#[allow(dead_code)]
pub fn must_named(n: &str) -> Format {
    named(n).unwrap()
}

/// Whether `f` is the dialect called `n`.
pub fn is(f: Format, n: &str) -> bool {
    name(f) == n
}

/// What a CLI format prints as, or `None` for gron (a CLI-only projection
/// with no printer — the `get`/`fmt`/`convert` handlers intercept it first).
pub fn to_target(f: Format) -> Option<Target> {
    match f {
        Format::CANONICAL => Some(Target::Canonical),
        Format::GRON => None,
        _ => Some(Target::Language(runtime_entry(f)?)),
    }
}

/// Part of a command-line path: Zig's `fig.AST.PathSegment`, owning its key
/// (a quoted or escaped key is decoded into bytes of its own).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSeg {
    Key(Vec<u8>),
    Index(usize),
}

/// `path` as the AST's borrowed segments.
pub fn segs(path: &[PathSeg]) -> Vec<PathSegment<'_>> {
    path.iter()
        .map(|s| match s {
            PathSeg::Key(k) => PathSegment::Key(k),
            PathSeg::Index(i) => PathSegment::Index(*i),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LangVerb {
    #[default]
    List,
    Check,
    Table,
}

#[derive(Debug, Clone, Default)]
pub struct LangOptions {
    pub verb: LangVerb,
    /// `check`'s language name; `table`'s file.
    pub name: String,
    /// `table -i <format>`: the format to read the file as, else its
    /// extension decides, else its contents.
    pub input: Option<Format>,
    /// `table --spec <version>`: the version of that format to read it
    /// as, where the format has one (`check`'s `--spec`).
    pub spec: Option<String>,
    /// `check --against <compiled>`: the compiled format to hold it to.
    pub against: Option<String>,
    /// `check`'s files, parsed by both and compared.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct HelpOptions {
    /// `help --oneline`: what fig is for, in one line on stdout.
    pub oneline: bool,
}

/// How a value argument (`set`/`insert`/`replace`) is read — see `value_arg.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ValueMode {
    /// As a figl value: `5` a number, `hello` a string, `[1, 2]` a sequence.
    #[default]
    Fig,
    /// `--string`: a string, whatever it looks like.
    String,
    /// `--raw`: the text itself, spliced as source in the file's format.
    Raw,
}

/// `replace` and `rename`: one splice at a path, of a value or of a key.
#[derive(Debug, Clone)]
pub struct EditOptions {
    pub file: String,
    pub path: Vec<PathSeg>,
    /// The new value (`replace`) or the new key name (`rename`).
    pub replacement: String,
    /// Set for `rename`: `replacement` is a key name, not a value.
    pub key: bool,
    pub value_mode: ValueMode,
    pub format: Format,
    /// Set when the format could not be inferred from the file extension:
    /// the handler then sniffs the file's contents with `Language.detect`.
    pub detect: bool,
}

#[derive(Debug, Clone)]
pub struct SetOptions {
    pub file: String,
    /// The target. For a scalar upsert the last segment is the key to
    /// replace-or-create; for `--seq` it names the sequence to reconcile.
    pub path: Vec<PathSeg>,
    /// The value to upsert (unused when `seq` is set).
    pub value: String,
    /// When set, reconcile the sequence at `path` to exactly `values`,
    /// preserving comments on survivors (the `set_sequence` primitive),
    /// instead of upserting a single scalar.
    pub seq: bool,
    pub values: Vec<String>,
    pub value_mode: ValueMode,
    pub format: Format,
    pub detect: bool,
}

#[derive(Debug, Clone)]
pub struct InsertOptions {
    pub file: String,
    /// The destination *slot*, not an existing node: the last segment names
    /// what to create. A trailing key (`a.b.newkey`) inserts that key into
    /// the mapping at the parent path; a trailing index (`a.list[0]` /
    /// `a.list[-]`) prepends/appends to the sequence at the parent path. An
    /// empty parent means the root container, so the root's actual kind
    /// (mapping vs sequence) decides which applies — not the file format.
    pub path: Vec<PathSeg>,
    pub value: String,
    pub value_mode: ValueMode,
    pub format: Format,
    /// Set when the format could not be inferred from the extension; the
    /// handler then sniffs the contents with `Language.detect`.
    pub detect: bool,
}

#[derive(Debug, Clone)]
pub struct DeleteOptions {
    pub file: String,
    /// The node to remove. A trailing key deletes that mapping entry (with
    /// its owned leading comments); a trailing index removes that sequence
    /// item from the parent sequence.
    pub path: Vec<PathSeg>,
    pub format: Format,
    pub detect: bool,
}

#[derive(Debug, Clone)]
pub struct GetOptions {
    pub file: String,
    pub path: Option<Vec<PathSeg>>,
    pub from: Format,
    pub to: Format,
    /// Set when `from` could not be inferred from the file extension and no
    /// `--input` was given: the handler sniffs the contents with
    /// `Language.detect`. When `to` was also left to default (`output_explicit`
    /// is false), the detected format flows through to the output too.
    pub detect: bool,
    /// Whether `--output`/`-o` was given. When false and `detect` fires, the
    /// detected input format becomes the output format (echo round-trip).
    pub output_explicit: bool,
    /// When converting YAML to another format, drop unknown/custom tags
    /// instead of erroring on them. Has no effect on parsing or YAML→YAML.
    pub lax_tags: bool,
    /// Lossless conversion: preserve values the target format can't represent
    /// natively (a null in TOML, a TOML datetime in JSON, …) through a `$fig`
    /// envelope, and reconstruct any such envelope found in the input. Gates
    /// both the encode (output) and decode (input) passes; default is lossy.
    pub lossless: bool,
    /// Output style. `--compact` clears `pretty` for a single-line render;
    /// `--indent N` sets the indent width; `--width N` sets TOML's inline-vs-
    /// expanded column budget. Honored by JSON (pretty + indent), ZON (pretty),
    /// and TOML (pretty gates array wrapping; indent/width drive its layout);
    /// YAML renders with its own fixed layout.
    pub serialize: SerializeOptions,
    /// Suppress the lossy-conversion warnings normally written to stderr.
    pub quiet: bool,
    /// Treat any lossy conversion as an error: print the warnings, then exit
    /// non-zero without writing output.
    pub strict: bool,
    /// Syntax knobs for `-o gron` (root name, key/value separator, terminator).
    /// Defaults reproduce gron exactly; ignored unless the output is gron.
    pub gron_projection: gron::Projection,
}

#[derive(Debug, Clone)]
pub struct CommentOptions {
    pub file: String,
    pub path: Vec<PathSeg>,
    pub text: String,
    /// When set, target the same-line trailing comment on the value at
    /// `path`; otherwise the own-line comment block above the node.
    pub inline_comment: bool,
    /// When set, delete the targeted comment instead of adding/setting it
    /// (then `text` is unused).
    pub delete: bool,
    /// When set, print the targeted comment to stdout instead of editing it
    /// (then `text` is unused, and the file is opened read-only).
    pub get: bool,
    pub format: Format,
    /// Set when the format could not be inferred from the file extension:
    /// the handler then sniffs the file's contents with `Language.detect`.
    pub detect: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    /// One or more files to validate. `-` reads stdin (single document).
    pub files: Vec<String>,
    /// Explicit `--input` format applied to every file. When `None`, each
    /// file's format is resolved from its extension, then by sniffing its
    /// contents — the same precedence `get` uses.
    pub format: Option<Format>,
    /// `--spec` version string (e.g. "1.0" for TOML). Resolved per file
    /// against the resolved format; `None` validates against the default
    /// version of each format.
    pub spec: Option<String>,
    /// Suppress the per-file `ok` lines on success; errors still print.
    pub quiet: bool,
    /// `--strict`: a file that parses with warnings fails (exit 1), as one
    /// that does not parse does.
    pub strict: bool,
}

#[derive(Debug, Clone)]
pub struct FmtOptions {
    /// The file to reformat in place. `-` reads stdin — only valid with
    /// `dry_run` (there is nowhere to write an in-place result back to).
    pub file: String,
    /// The single format `fmt` parses AND re-emits — unlike `get`, there is
    /// no `--output`: reformatting never changes the document's format.
    pub from: Format,
    /// Set when `from` could not be inferred from the file extension and no
    /// `--input` was given: the handler sniffs the contents with
    /// `Language.detect`.
    pub detect: bool,
    /// Output style — see `get`'s twin field.
    pub serialize: SerializeOptions,
    /// Suppress the lossy-conversion (e.g. `--strip-comments`) and fig
    /// authoring-lint warnings normally written to stderr.
    pub quiet: bool,
    /// Treat any warning as an error (exit non-zero without writing).
    pub strict: bool,
    /// Print the reformatted result to stdout instead of writing it back,
    /// and exit 1 if reformatting would change the file (0 if already
    /// clean) — the CI-friendly "would this file's formatting change" gate.
    pub dry_run: bool,
    /// Like `dry_run`, but print a unified diff of the change instead of
    /// the whole reformatted file (nothing is written either way).
    pub diff: bool,
}

#[derive(Debug, Clone)]
pub struct ConvertOptions {
    /// The file to convert. `-` reads stdin — only valid without `--write`
    /// (there is nowhere to write an in-place result back to).
    pub file: String,
    /// Parse as `from`, re-emit as `to` (`--output`, required).
    pub from: Format,
    pub to: Format,
    /// Set when `from` couldn't be pinned by `--input`/the file extension:
    /// the handler sniffs the contents with `Language.detect`, mirroring
    /// `fmt`/`get`.
    pub detect: bool,
    /// As in `get`: drop unknown/custom YAML tags instead of erroring,
    /// when converting away from YAML.
    pub lax_tags: bool,
    /// As in `get`: preserve values the target can't represent natively
    /// through a `$fig` envelope, and decode any such envelope on input.
    pub lossless: bool,
    pub serialize: SerializeOptions,
    pub quiet: bool,
    pub strict: bool,
    /// Write the converted result back to `file` in place (skipped when the
    /// bytes are already identical). Without this, `convert` never touches
    /// disk — it just prints, like `get`. Combinable with `diff`: writes the
    /// file AND prints the unified diff of what changed.
    pub write: bool,
    /// Print a unified diff of the change instead of the whole converted
    /// file. Independent of `write` — with neither flag, the whole converted
    /// document prints to stdout.
    pub diff: bool,
}

#[derive(Debug, Clone)]
pub struct PatchOptions {
    /// The document being patched, and the only one written. `-` reads stdin —
    /// only valid with `--dry-run`/`--diff`, there being nowhere to write an
    /// in-place result back to. Must already exist: `patch` merges into a
    /// document, it does not seed one (that is `set`'s job).
    pub file: String,
    /// The document supplying the change. `-` reads stdin (so `fig get a.toml
    /// service | fig patch b.yaml -` works); at most one of the two may.
    pub patch_file: String,
    /// Where in the target the patch lands (`--at`). Empty is the root.
    pub at: Vec<PathSeg>,
    /// Which subtree of the patch document to take (`--from`). Empty is its
    /// root.
    pub from: Vec<PathSeg>,
    /// Paths to remove from the target after the merge (`--delete`, repeatable).
    pub deletes: Vec<Vec<PathSeg>>,
    /// How the merge behaves — sequence strategy, comment strategy, and the
    /// serialize knobs patch subtrees are rendered with.
    pub patch_options: patch::Options,
    /// The TARGET's format. As everywhere else: `--input`, else the extension,
    /// else (with `detect`) a content sniff.
    pub format: Format,
    pub detect: bool,
    /// The PATCH document's format (`--patch-input`), resolved the same way
    /// against its own extension — hence its own `detect` twin.
    pub patch_format: Format,
    pub detect_patch: bool,
    /// Preserve values the target format can't represent natively through a
    /// `$fig` envelope, and decode any envelope the patch document carries.
    /// Same flag, same meaning, as `get`/`convert`.
    pub lossless: bool,
    /// Print the patched document to stdout instead of writing it back.
    pub dry_run: bool,
    /// Print a unified diff of the change instead of writing it back.
    /// Unlike `fmt`'s, neither preview mode sets a non-zero exit status: a
    /// patch is EXPECTED to change the file, so "it changed" is not a failure.
    pub diff: bool,
    /// Suppress the patch file's warnings and the dropped-comment warning.
    pub quiet: bool,
    /// `--strict`: any warning — the patch file's own lints, a comment the
    /// target has nowhere to put — exits 1 with nothing written.
    pub strict: bool,
}

/// A subcommand fig has no action of its own for, on its way to the
/// git-style `fig <name>` → `fig-<name>` handoff — see `external.rs`, which
/// is the only consumer. `parse_config` only builds this for a word that could
/// name an executable at all (see `external_command_name`), so `name` is never
/// a path, a flag, or empty.
#[derive(Debug, Clone)]
pub struct ExternalOptions {
    /// The word as the user typed it (`schema` in `fig schema get f.json`).
    /// Kept alongside `program` for the not-found report, which talks about
    /// what was typed rather than what was looked up.
    pub name: String,
    /// `fig-<name>` — the executable to look for on PATH. Always spelled
    /// `fig-`, never after `binary_name`: a renamed or path-qualified argv[0]
    /// (`./zig-out/bin/fig`) would otherwise ask for a sibling nothing ships.
    ///
    /// `None` when `name` is a word that could not name an executable at all
    /// (`fig config.toml`, `fig --colour`), which is the ordinary "no such
    /// action" case wearing this union's clothes: there is nothing to look
    /// up, and `argv` is empty.
    pub program: Option<String>,
    /// The full argv to hand over: `program`, then every argument after the
    /// subcommand word, verbatim and unparsed. fig deliberately does not read
    /// them — the flags after `fig schema` belong to `fig-schema`, including
    /// the ones fig itself would recognize.
    pub argv: Vec<std::ffi::OsString>,
}

#[derive(Debug, Clone)]
pub enum CliActionOptions {
    Help(HelpOptions),
    Version,
    Replace(EditOptions),
    Rename(EditOptions),
    Set(SetOptions),
    Insert(InsertOptions),
    Delete(DeleteOptions),
    Get(GetOptions),
    Comment(CommentOptions),
    Check(CheckOptions),
    Fmt(FmtOptions),
    Convert(ConvertOptions),
    Patch(PatchOptions),
    Lang(LangOptions),
    External(ExternalOptions),
}

/// The in-place editing operation `apply_edit` performs. Generalizes the
/// editor's span-splice surface so `replace`, `rename` and `comment` share one
/// code path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOp {
    ReplaceValue,
    ReplaceKey,
    AddLeadingComment,
    SetTrailingComment,
    DeleteLeadingComments,
    DeleteTrailingComment,
    /// Insert `key: text` into the mapping at `path`. The payload is the new
    /// key's text; the value rides in `apply_edit`'s `text` argument.
    InsertKey(Vec<u8>),
    /// Upsert the value at `path`: replace it, or insert the trailing key when
    /// only it is absent. `text` is the value; `path` ends in the key.
    Set,
    /// Reconcile the sequence at `path` to exactly `items`, preserving the
    /// comments on items that survive (`text` unused).
    SetSequence(Vec<Vec<u8>>),
    /// Append `text` as a new last item to the sequence at `path`.
    AppendSeq,
    /// `AppendSeq`, starting the sequence at `path` first when it is absent
    /// or null: `set`'s `[-]`.
    SetAppend,
    /// Insert `text` as the new first item of the sequence at `path`.
    PrependSeq,
    /// Insert `text` into the sequence at `path` as the item at this index,
    /// before the item there now.
    InsertSeqItem(usize),
    /// Delete the mapping entry named by `path` (text unused).
    DeleteKey,
    /// Remove the item at this index from the sequence at `path` (text unused).
    RemoveSeqItem(usize),
    /// `ReplaceValue` and `Set` for a value fig rendered that is a string:
    /// the payload is the string, and `text` fig's spelling of it. A string
    /// it replaces keeps its quote style (`Editor.replace_string_at_path`).
    /// `render_edit` makes these; nothing asks for one directly.
    ReplaceString(Vec<u8>),
    SetString(Vec<u8>),
}

/// Sentinel sequence index meaning "the end" — produced by `parse_path` for the
/// `[-]`/`[$]` append tokens and consumed by the `insert` handler to pick
/// `AppendSeq` over an insert at an index. Out of range for any real index, so
/// it never collides with an addressable item.
pub const APPEND_INDEX: usize = usize::MAX;

/// What `args::parse_config` makes of the command line: the action, with
/// its options resolved, and the name fig was run as (argv[0]), which the
/// reports quote.
#[derive(Debug, Clone)]
pub struct CliConfig {
    pub options: CliActionOptions,
    pub binary_name: String,
}

/// The caller-supplied text an action splices into the document, plus what it
/// takes to report it: which file it was going into, and which format that
/// file is (`None` under `--detect`, where only the handler resolves it).
/// `text` is `None` when the action carries several (`set --seq`). Read only by
/// `main`'s `InvalidEditText` path — see `diag_report::report_bad_edit_text`.
pub struct SplicedText<'a> {
    pub file: &'a str,
    pub format: Option<Format>,
    pub kind: EditTextKind,
    pub text: Option<&'a str>,
}

/// What a piece of spliced text was meant to be — only ever used to word the
/// report in `diag_report::report_bad_edit_text` ("the new value" vs "the new key").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTextKind {
    /// A value argument, read as a figl value (or with `--string`) and
    /// rendered in the file's own spelling.
    Value,
    /// A `--raw` value argument, spliced as the user typed it.
    RawValue,
    Key,
    Comment,
}

impl EditTextKind {
    pub fn noun(self) -> &'static str {
        match self {
            EditTextKind::Value | EditTextKind::RawValue => "value",
            EditTextKind::Key => "key",
            EditTextKind::Comment => "comment text",
        }
    }
}

/// The spliced text `config`'s action carries, or `None` for the actions that
/// splice none of the caller's (`delete`, `comment --delete`, and the
/// read-only ones). One of those can still return `InvalidEditText`, when
/// what it leaves behind does not parse; `main` reports that against the
/// path instead (`diag_report::report_unparseable_edit`).
pub fn spliced_text(config: &CliConfig) -> Option<SplicedText<'_>> {
    match &config.options {
        CliActionOptions::Replace(o) | CliActionOptions::Rename(o) => Some(SplicedText {
            file: &o.file,
            format: if o.detect { None } else { Some(o.format) },
            // `rename`'s argument is a replacement KEY, not a value.
            kind: if o.key { EditTextKind::Key } else { value_kind(o.value_mode) },
            text: Some(&o.replacement),
        }),
        CliActionOptions::Set(o) => Some(SplicedText {
            file: &o.file,
            format: if o.detect { None } else { Some(o.format) },
            kind: value_kind(o.value_mode),
            text: if o.seq { None } else { Some(&o.value) },
        }),
        CliActionOptions::Insert(o) => Some(SplicedText {
            file: &o.file,
            format: if o.detect { None } else { Some(o.format) },
            kind: value_kind(o.value_mode),
            text: Some(&o.value),
        }),
        CliActionOptions::Comment(o) => {
            if o.delete || o.get {
                None
            } else {
                Some(SplicedText {
                    file: &o.file,
                    format: if o.detect { None } else { Some(o.format) },
                    kind: EditTextKind::Comment,
                    text: Some(&o.text),
                })
            }
        }
        _ => None,
    }
}

/// The key an edit that may create one names — the last segment of
/// `insert`'s or `set`'s path — for the report when the document refuses
/// the key itself (`InvalidEditKey`).
pub fn created_key(config: &CliConfig) -> Option<&[u8]> {
    let path = match &config.options {
        CliActionOptions::Set(o) => &o.path,
        CliActionOptions::Insert(o) => &o.path,
        _ => return None,
    };
    match path.last()? {
        PathSeg::Key(k) => Some(k),
        PathSeg::Index(_) => None,
    }
}

fn value_kind(mode: ValueMode) -> EditTextKind {
    if mode == ValueMode::Raw { EditTextKind::RawValue } else { EditTextKind::Value }
}

/// The single file `config`'s action works on, when it has exactly one — so a
/// failure that escapes the action can at least name it (see
/// `diag_report::report_unhandled`). `None` for `check`, which takes a list and
/// reports per file itself, and for the file-less actions.
pub fn target_file(config: &CliConfig) -> Option<&str> {
    match &config.options {
        CliActionOptions::Replace(o) | CliActionOptions::Rename(o) => Some(&o.file),
        CliActionOptions::Set(o) => Some(&o.file),
        CliActionOptions::Insert(o) => Some(&o.file),
        CliActionOptions::Delete(o) => Some(&o.file),
        CliActionOptions::Get(o) => Some(&o.file),
        CliActionOptions::Comment(o) => Some(&o.file),
        CliActionOptions::Fmt(o) => Some(&o.file),
        CliActionOptions::Convert(o) => Some(&o.file),
        // The TARGET, not the patch document: it is the file being written,
        // so it is the one a failure has to name.
        CliActionOptions::Patch(o) => Some(&o.file),
        // `external` never touches a file itself — whatever it does with its
        // arguments is the other binary's business, and its failures are its
        // own to report.
        CliActionOptions::Help(_)
        | CliActionOptions::Version
        | CliActionOptions::Check(_)
        | CliActionOptions::Lang(_)
        | CliActionOptions::External(_) => None,
    }
}

/// The path `config`'s action edits at, for a report that has no text of
/// the caller's to quote. `None` for the actions that edit nothing.
pub fn edit_path(config: &CliConfig) -> Option<&[PathSeg]> {
    match &config.options {
        CliActionOptions::Replace(o) | CliActionOptions::Rename(o) => Some(&o.path),
        CliActionOptions::Set(o) => Some(&o.path),
        CliActionOptions::Insert(o) => Some(&o.path),
        CliActionOptions::Delete(o) => Some(&o.path),
        CliActionOptions::Comment(o) => Some(&o.path),
        _ => None,
    }
}

/// The single file `config`'s action parses, and the format to parse it as —
/// `None` where the action resolves that itself (`--detect` sniffing). What
/// `main` hands `parse_dispatch::check_one` to locate a parse failure that
/// escaped the action unreported. `None` for stdin, which cannot be read twice,
/// and for everything `target_file` is `None` for.
pub struct ParseTarget<'a> {
    pub file: &'a str,
    pub format: Option<Format>,
}

pub fn parse_target(config: &CliConfig) -> Option<ParseTarget<'_>> {
    fn of(file: &str, detect: bool, format: Format) -> ParseTarget<'_> {
        ParseTarget { file, format: if detect { None } else { Some(format) } }
    }
    let t = match &config.options {
        CliActionOptions::Replace(o) | CliActionOptions::Rename(o) => of(&o.file, o.detect, o.format),
        CliActionOptions::Set(o) => of(&o.file, o.detect, o.format),
        CliActionOptions::Insert(o) => of(&o.file, o.detect, o.format),
        CliActionOptions::Delete(o) => of(&o.file, o.detect, o.format),
        CliActionOptions::Comment(o) => of(&o.file, o.detect, o.format),
        CliActionOptions::Patch(o) => of(&o.file, o.detect, o.format),
        CliActionOptions::Get(o) => of(&o.file, o.detect, o.from),
        CliActionOptions::Fmt(o) => of(&o.file, o.detect, o.from),
        CliActionOptions::Convert(o) => of(&o.file, o.detect, o.from),
        CliActionOptions::Help(_)
        | CliActionOptions::Version
        | CliActionOptions::Check(_)
        | CliActionOptions::Lang(_)
        | CliActionOptions::External(_) => return None,
    };
    if t.file == "-" {
        return None;
    }
    Some(t)
}

/// Result of mapping a file extension to a parse strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    pub format: Format,
}
