//! Port of src/cli/args.zig, on clap.
//!
//! The command line, declared with clap's derive API — `Cli`, an `Action`
//! per verb, a struct of flags and positionals each — and `parse_config`,
//! which turns what clap read into the `CliConfig` the actions take: paths
//! parsed, each file's format resolved from `--input`, `--lang` or its
//! extension. Plus the small format/path resolvers parsing and the action
//! handlers share.
//!
//! The rules clap keeps for fig (they are `help::GENERAL`'s): a word that
//! begins with `-` and is none of the action's flags is a usage error, never
//! a file name, and so is one positional more than the action takes; `--`
//! ends the flags; `-` alone is stdin; a negative number is a value wherever
//! it stands (`allow_negative_numbers`); a flag given twice is not an error,
//! the last one counts (`args_override_self`); and an option's value is the
//! next argument, whatever it looks like (`--gron-sep --`), as every option
//! here sets `allow_hyphen_values`. Every usage error exits 2.

use std::ffi::OsString;

use clap::error::ErrorKind;
use clap::{ArgAction, Args, CommandFactory, Parser, Subcommand};
use fig_core::kit::ast::SerializeOptions;
use fig_core::patch::{self, CommentStrategy, SeqStrategy};

use crate::gron;
use crate::help;
use crate::languages;
use crate::types::{self, *};

/// Map a `--input`/`-i` format name to a `Format`. The registered dialect
/// names cover every accepted token (including `canonical` and `figl`)
/// directly, plus the one alias below. Returns `None` for an unknown name so
/// callers can emit a tailored error.
pub fn parse_format_name(name: &str) -> Option<Format> {
    // `yml` is an accepted SPELLING of yaml, not a format of its own, and
    // collapses here, so downstream code only ever sees `yaml`.
    if name == "yml" {
        return parse_format_name("yaml");
    }
    if let Some(f) = types::named(name) {
        return Some(f);
    }
    // A language configured in `languages.figl`, spawned and registered on
    // this first ask.
    languages::resolve_name(name)
}

/// JSON: the format a command falls back to where none is named and none
/// is detected, as `convert`'s output.
fn json_format() -> Format {
    parse_format_name("json").unwrap_or(Format::CANONICAL)
}

/// A path that does not parse (`parse_path`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPath;

/// Parse a command-line path — `a.b[2].c` — into segments. A `.` separates
/// keys and a `[n]` is an index; `[-]`/`[$]` is the end sentinel.
///
/// A key that holds a `.` or a `[` is written quoted or escaped:
///
///   * `a."b.c"` — double quotes, with JSON's escapes inside (`\"`, `\\`,
///     `\n`, `\u00e9`, …);
///   * `a.'b.c'` — single quotes, verbatim, as a TOML literal key;
///   * `a["b.c"]` / `a['b.c']` — the same quoting in brackets, which is how
///     `-o gron` prints such a key, so a gron line reads back;
///   * `a.b\.c` — a backslash in a bare key takes the next byte as it is.
pub fn parse_path(path: &[u8]) -> Result<Vec<PathSeg>, InvalidPath> {
    let mut path_in_progress: Vec<PathSeg> = Vec::new();
    let mut i = 0usize;
    while i < path.len() {
        match path[i] {
            b'.' => {
                // Dot is a separator. Else branch parses the key.
                i += 1;
            }
            b'[' => {
                // A quoted key in brackets — gron's spelling of a key that
                // is not a bare identifier.
                if i + 1 < path.len() && (path[i + 1] == b'"' || path[i + 1] == b'\'') {
                    i += 1;
                    let key = quoted_key(path, &mut i)?;
                    if i >= path.len() || path[i] != b']' {
                        return Err(InvalidPath);
                    }
                    i += 1;
                    path_in_progress.push(PathSeg::Key(key));
                    continue;
                }
                // Skip open bracket
                let start = i + 1;
                i = start;
                // Loop until end or close bracket
                while i < path.len() && path[i] != b']' {
                    i += 1;
                }
                if i >= path.len() || i == start {
                    return Err(InvalidPath);
                }

                // `[-]` and `[$]` are the "end" tokens: `insert` reads the
                // sentinel as "append", and `delete` reads it as "the last
                // item". Any other caller that walks the path literally (e.g.
                // `get`) just sees an out-of-range index and surfaces NotFound.
                let inner = &path[start..i];
                let seg = if inner == b"-" || inner == b"$" {
                    PathSeg::Index(APPEND_INDEX)
                } else {
                    PathSeg::Index(gron::parse_usize(inner).ok_or(InvalidPath)?)
                };
                path_in_progress.push(seg);
                // Skip close bracket
                i += 1;
            }
            b'"' | b'\'' => {
                let key = quoted_key(path, &mut i)?;
                // A quoted key ends its segment.
                if i < path.len() && path[i] != b'.' && path[i] != b'[' {
                    return Err(InvalidPath);
                }
                path_in_progress.push(PathSeg::Key(key));
            }
            _ => {
                let start = i;
                let mut escaped = false;
                // Loop until a dot or open bracket; a backslash carries the
                // byte after it past both.
                while i < path.len() && path[i] != b'.' && path[i] != b'[' {
                    if path[i] == b'\\' {
                        if i + 1 >= path.len() {
                            return Err(InvalidPath);
                        }
                        escaped = true;
                        i += 1;
                    }
                    i += 1;
                }
                if i == start {
                    return Err(InvalidPath);
                }
                let key = if !escaped {
                    path[start..i].to_vec()
                } else {
                    let mut out = Vec::new();
                    let mut j = start;
                    while j < i {
                        if path[j] == b'\\' {
                            j += 1;
                        }
                        out.push(path[j]);
                        j += 1;
                    }
                    out
                };
                path_in_progress.push(PathSeg::Key(key));
            }
        }
    }
    Ok(path_in_progress)
}

/// The key quoted at `path[*i]` — `'…'` verbatim, `"…"` with JSON's
/// escapes decoded — leaving `*i` just past the closing quote.
fn quoted_key(path: &[u8], i: &mut usize) -> Result<Vec<u8>, InvalidPath> {
    let quote = path[*i];
    let start = *i + 1;
    let mut j = start;
    if quote == b'\'' {
        while j < path.len() && path[j] != b'\'' {
            j += 1;
        }
        if j >= path.len() {
            return Err(InvalidPath);
        }
        *i = j + 1;
        return Ok(path[start..j].to_vec());
    }
    let mut out: Vec<u8> = Vec::new();
    loop {
        if j >= path.len() {
            return Err(InvalidPath);
        }
        let c = path[j];
        if c == b'"' {
            break;
        }
        if c != b'\\' {
            out.push(c);
            j += 1;
            continue;
        }
        j += 1;
        if j >= path.len() {
            return Err(InvalidPath);
        }
        match path[j] {
            e @ (b'"' | b'\\' | b'/') => out.push(e),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'u' => {
                let mut cp: u32 = hex4(path, j + 1)?;
                j += 4;
                // A high surrogate takes the `\uXXXX` low half after it.
                if (0xD800..=0xDBFF).contains(&cp) {
                    if j + 2 >= path.len() || path[j + 1] != b'\\' || path[j + 2] != b'u' {
                        return Err(InvalidPath);
                    }
                    let lo = hex4(path, j + 3)?;
                    if !(0xDC00..=0xDFFF).contains(&lo) {
                        return Err(InvalidPath);
                    }
                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                    j += 6;
                } else if (0xDC00..=0xDFFF).contains(&cp) {
                    return Err(InvalidPath);
                }
                let ch = char::from_u32(cp).ok_or(InvalidPath)?;
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            _ => return Err(InvalidPath),
        }
        j += 1;
    }
    *i = j + 1;
    Ok(out)
}

fn hex4(path: &[u8], at: usize) -> Result<u32, InvalidPath> {
    if at + 4 > path.len() {
        return Err(InvalidPath);
    }
    let s = std::str::from_utf8(&path[at..at + 4]).map_err(|_| InvalidPath)?;
    if s.starts_with('+') || s.starts_with('-') {
        return Err(InvalidPath);
    }
    u32::from_str_radix(s, 16).map_err(|_| InvalidPath)
}

/// Infer the parse strategy from a file's extension, or `None` when the extension
/// is missing/unrecognized — the caller then falls back to content sniffing
/// (`Language.detect`) rather than failing outright.
pub fn detect_language_from_file_ending(file_path: &str) -> Option<Detected> {
    // `--lang <name>` names the language outright, whatever the extension
    // says — it is how a file a compiled format owns by extension is read
    // through a runtime language instead (`secrets.env --lang lua-dotenv`).
    if let Some(f) = languages::lang_override() {
        return Some(Detected { format: f });
    }

    let ext = match file_path.rfind('.') {
        Some(d) => &file_path[d + 1..],
        None => &file_path[1.min(file_path.len())..],
    };

    // An extension that names a format IS that format. Tried first so every
    // name keeps its exact meaning — notably the dialects (`jsonc`, `json5`)
    // that share a language with the default spelling, and the CLI's own
    // `gron`. `yml` is not a name: YAML declares it as an extension, below.
    // A configured language of that name answers to it too.
    if let Some(format) = parse_format_name(ext) {
        return Some(Detected { format });
    }

    // Otherwise ask the languages: each declares the extensions it owns —
    // `.figl`, `.env` (dotenv files are conventionally named exactly `.env`,
    // so the last-dot split gives a literal `env`), `.nt`, `.yml`. Not
    // recognized here: multi-suffix variants like `.env.production`, whose
    // last-dot extension is `production`; pass `--input dotenv` for those.
    if let Some(format) = languages::registered_extension(ext) {
        return Some(Detected { format });
    }

    // Last, the languages `languages.figl` configures: a registered
    // language's extension always wins, so a configured language is only
    // reached by an extension nothing registered owns (proposal §7.1).
    if let Some(format) = languages::resolve_extension(ext) {
        return Some(Detected { format });
    }

    None
}

/// A `<PATH>` argument, parsed (`parse_path`). A type of its own so clap
/// takes it as one value, not as a list of segments.
#[derive(Debug, Clone)]
pub struct DocPath(Vec<PathSeg>);

/// clap's reading of a `<PATH>`: one that does not parse is a usage error
/// that says how a path is written.
fn path_arg(path: &str) -> Result<DocPath, String> {
    parse_path(path.as_bytes()).map(DocPath).map_err(|_| {
        "not a path: keys are separated by `.`, an index is `[n]` (or `[-]` for the end), and a key holding `.` or `[` is quoted: a.\"b.c\"".to_string()
    })
}

/// clap's reading of a `--input`/`--output` format name (`parse_format_name`).
fn format_arg(name: &str) -> Result<Format, String> {
    parse_format_name(name).ok_or_else(|| {
        // A configured language that was refused has said why already,
        // through `languages::ensure`.
        if languages::is_configured(name) {
            format!("the language `{name}` was refused (`fig lang check {name}` says why)")
        } else {
            let known = help::format_list("", &[]).replace(",\n", ", ");
            format!("not a format: one of {}, or a language configured in languages.figl (`fig lang list`)", known.trim_end())
        }
    })
}

/// clap's reading of `--lang <NAME>`.
fn lang_arg(name: &str) -> Result<Format, String> {
    parse_format_name(name).ok_or_else(|| {
        if languages::is_configured(name) {
            format!("the language `{name}` was refused (`fig lang check {name}` says why)")
        } else {
            format!("No language named `{name}`: not a compiled format, and no languages.figl configures it (see `fig lang --help`)")
        }
    })
}

/// `std.fmt.parseInt(u8/u16, n, 10)`.
fn parse_int<T: TryFrom<usize>>(n: &str) -> Option<T> {
    let v = gron::parse_usize(n.as_bytes())?;
    T::try_from(v).ok()
}

fn indent_arg(n: &str) -> Result<u8, String> {
    parse_int(n).ok_or_else(|| "not a number of spaces (0 to 255)".to_string())
}

fn width_arg(n: &str) -> Result<u16, String> {
    parse_int(n).ok_or_else(|| "not a number of columns (0 to 65535)".to_string())
}

/// `--seq`'s strategy. `union` is the word a user would reach for; the Zig
/// enum member is `unite`, which it also takes.
fn seq_arg(name: &str) -> Result<SeqStrategy, String> {
    if name == "union" {
        return Ok(SeqStrategy::Unite);
    }
    SeqStrategy::from_name(name).ok_or_else(|| "not a --seq strategy (replace, append, union)".to_string())
}

fn comments_arg(name: &str) -> Result<CommentStrategy, String> {
    CommentStrategy::from_name(name).ok_or_else(|| "not a --comments strategy (ours, theirs, none)".to_string())
}

/// fig's command line, as clap reads it. `parse_config` turns it into the
/// `CliConfig` the actions take.
#[derive(Parser, Debug)]
#[command(
    name = "fig",
    about = help::ABOUT,
    after_help = help::GENERAL,
    // `help`, `-h` and `--help` are one action, as are `version`, `-v` and
    // `--version` (see `Action`), and as strict as every other: `fig --help
    // --bogus` is a usage error, not a silent 0.
    disable_help_flag = true,
    disable_help_subcommand = true,
    disable_version_flag = true,
    // Git's fallback: a word fig has no action for goes to `fig-<word>`.
    allow_external_subcommands = true,
    // A flag given twice is not an error: the last one counts.
    args_override_self = true,
    subcommand_value_name = "ACTION",
    subcommand_help_heading = "Actions",
)]
pub struct Cli {
    /// Read and write files through this language, whatever their extension
    ///
    /// A language configured in languages.figl, or a built-in format:
    /// `fig get secrets.env --lang lua-dotenv`. Every action takes it
    /// (`fig lang --help`).
    #[arg(long, global = true, value_name = "NAME", value_parser = lang_arg, allow_hyphen_values = true)]
    lang: Option<Format>,

    /// With none, fig prints its help.
    #[command(subcommand)]
    action: Option<Action>,
}

/// fig's actions, each with its short spellings.
#[derive(Subcommand, Debug)]
enum Action {
    /// Print this text (the default action); `help --oneline` prints what fig is for in one line
    #[command(long_flag = "help", short_flag = 'h')]
    Help(HelpArgs),

    /// Print fig's version
    ///
    /// Prints fig's version and its epoch, a name with no compatibility
    /// meaning: `fig 5.0.0 "Texas Everbearing"`. Every artifact — the CLI,
    /// the core, the Rust crates, the npm packages — carries the one
    /// version. `--version` and `-v` are the same action. It takes no
    /// arguments.
    #[command(long_flag = "version", short_flag = 'v')]
    Version(VersionArgs),

    /// Replace the value at a path
    ///
    /// Replaces the value at <PATH>, which must already exist (`set`
    /// creates it when absent). To rename the key there, use `fig rename`.
    #[command(allow_negative_numbers = true)]
    Replace(ReplaceArgs),

    /// Rename the key at a path
    ///
    /// Renames the key at <PATH> to <NAME>, keeping its value and comments.
    /// <NAME> is the new key's name, which fig spells as the file's format
    /// spells a key, quoting it where it must. A <PATH> that ends in an
    /// index has no key, and a <NAME> the mapping already holds would be a
    /// second key of that name: both are refused, exit 1. To replace the
    /// value there, use `fig replace`.
    #[command(allow_negative_numbers = true)]
    Rename(RenameArgs),

    /// Upsert a value (create the key, or file, if absent)
    ///
    /// Upsert: replace the value at <PATH>, or create it when absent — one
    /// verb for `replace`+`insert`. Missing parent maps along <PATH> are
    /// auto-created (`mkdir -p`); a segment that is an existing non-map
    /// scalar is a type error and left untouched.
    ///
    /// A path ending in an index sets a sequence item: list[2] replaces the
    /// item that is there, and list[-] (or list[$]) appends one, starting
    /// the list when it is absent or null. An index past the end is
    /// refused. INI, dotenv and .properties have no list to start.
    ///
    /// When <FILE> itself does not exist, it is CREATED and seeded with
    /// <PATH>: <VALUE>; `fig get <FILE>` then prints what was written. The
    /// format comes from the extension, so a new file needs a known one —
    /// .figl/.json/.jsonc/.yaml/.yml/.toml. .zon/.json5 have no
    /// from-scratch seed and must already exist.
    ///
    /// A created key is written in the file's syntax too, so new keys work
    /// for strict JSON.
    #[command(visible_alias = "s", allow_negative_numbers = true)]
    Set(SetArgs),

    /// Add a new key or list item to a file
    ///
    /// Adds a new entry. The last path segment names the slot to create:
    ///   a.b.newkey   -> insert key `newkey` into the mapping at a.b
    ///   a.list[0]    -> prepend <VALUE> as the first item of a.list
    ///   a.list[2]    -> insert <VALUE> before the item now at [2]
    ///   a.list[-]    -> append <VALUE> as the last item ([$] also works)
    /// An empty parent targets the root container, so the document's own
    /// root (mapping vs list) decides which form applies — not the format.
    #[command(visible_alias = "i", allow_negative_numbers = true, verbatim_doc_comment)]
    Insert(InsertArgs),

    /// Remove a key or list item from a file
    ///
    /// Removes the entry the path points at. The last path segment decides:
    ///   a.b.key      -> delete that mapping entry (with its own comments)
    ///   a.list[2]    -> remove item 2 from the sequence a.list
    ///   a.list[-]    -> remove the last item of the sequence a.list
    /// A path naming a whole [table]/[section]/block container — or one
    /// [[array-of-tables]] element — removes all of it, header, entries
    /// and every place it is reopened further down the file.
    #[command(visible_alias = "d", allow_negative_numbers = true, verbatim_doc_comment)]
    Delete(DeleteArgs),

    /// Print a file or a specific part of a file to stdout
    #[command(visible_alias = "g", allow_negative_numbers = true, after_long_help = help::get_formats())]
    Get(GetArgs),

    /// Add or edit a comment on part of a file
    ///
    /// By default, adds an own-line comment ABOVE the node at <PATH>. The
    /// comment marker is added for you: # for YAML/TOML, // for
    /// JSONC/JSON5/ZON. Strict JSON has no comments (rejected).
    #[command(visible_alias = "c", allow_negative_numbers = true)]
    Comment(CommentArgs),

    /// Validate that one or more files parse cleanly
    ///
    /// Validate that each file parses cleanly as its format. Prints an `ok`
    /// line per file and exits 0 when all parse; prints an error line to
    /// stderr for each failing file and exits 1 if any fail.
    #[command(visible_alias = "ck", allow_negative_numbers = true, after_long_help = help::formats(&[]))]
    Check(CheckArgs),

    /// Reformat a file in place (house style; gofmt-style)
    ///
    /// Reformat a file in place: parse then re-emit in the format's house
    /// style — `.figl`'s printer applies the style DESIGN.md describes
    /// (spaced marker runs, `[]`/`+` list sigils, ...); other formats get
    /// their own printer's canonical layout. Unlike `get`, the output
    /// format always matches the input — reformatting never converts. A
    /// file already in house style is left byte-identical (no-op write).
    #[command(visible_alias = "f", allow_negative_numbers = true, after_long_help = help::formats(&["gron"]))]
    Fmt(FmtArgs),

    /// Convert a file from one format to another; prints to stdout, or writes the file in place with --write
    ///
    /// Convert a file — `fmt`'s twin for when the target format differs
    /// from the source: parse the whole file as --input (else the
    /// extension, else sniffed from its contents) and re-emit it as
    /// --output, in the target format's house style. Like `get`, it prints
    /// the converted result to stdout by default; pass --write to write it
    /// back to <FILE> in place instead.
    #[command(visible_alias = "cv", allow_negative_numbers = true, after_long_help = help::formats(&["gron"]))]
    Convert(ConvertArgs),

    /// Merge one document into another, in place and losslessly
    ///
    /// Merge <PATCH_FILE> into <FILE>, in place. Every byte of <FILE>
    /// outside the paths the patch actually names is left untouched —
    /// comments, key order, quoting style and all. The two files need not
    /// share a format: a TOML patch merges into a YAML target, rendered in
    /// the target's syntax on the way in.
    ///
    /// <FILE> must already exist — `patch` merges into a document, it does
    /// not create one; use `set` for that.
    #[command(visible_alias = "p", allow_negative_numbers = true, after_long_help = help::PATCH_RULES)]
    Patch(PatchArgs),

    /// List the languages fig knows, or check a configured one
    #[command(long_about = help::LANG, after_long_help = help::LANG_AFTER)]
    Lang(LangArgs),

    /// Any other word: handed to a `fig-<word>` program on PATH, with every
    /// argument after it untouched (`external.rs`).
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

/// `-h` (a summary) and `--help` (the whole page), on every action. clap's
/// own are off (`disable_help_flag`, which reaches every action) so that
/// `fig -h` and `fig --help` can be the `help` action; each action takes
/// this one in their place.
#[derive(Args, Debug)]
struct HelpFlag {
    /// Print help (a summary with -h)
    #[arg(short = 'h', long = "help", action = ArgAction::Help, global = true)]
    #[allow(dead_code)]
    help: Option<bool>,
}

#[derive(Args, Debug)]
struct VersionArgs {
    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Args, Debug)]
struct HelpArgs {
    /// Print what fig is for, in one line, on stdout
    #[arg(long)]
    oneline: bool,

    /// `fig help set` reads as asking for `set`'s help: refused, saying
    /// where it is.
    #[arg(hide = true)]
    topic: Option<String>,

    #[command(flatten)]
    help: HelpFlag,
}

/// How `replace`, `set` and `insert` read their value (`value_arg.rs`):
/// as a figl value, unless one of these says otherwise.
#[derive(Args, Debug)]
struct ValueReading {
    /// Take <VALUE> as a string, whatever it looks like (`set f.json version --string 1.10`)
    #[arg(long, conflicts_with = "raw")]
    string: bool,

    /// Splice <VALUE> verbatim as source text in the file's format, for
    /// what only it can spell (a YAML anchor, a TOML local datetime)
    #[arg(long)]
    raw: bool,
}

impl ValueReading {
    fn mode(&self) -> ValueMode {
        if self.raw {
            ValueMode::Raw
        } else if self.string {
            ValueMode::String
        } else {
            ValueMode::Fig
        }
    }
}

/// The output style `get`, `fmt`, `convert` and `patch` share.
#[derive(Args, Debug)]
struct Layout {
    /// Single-line output with minimal whitespace (JSON, JSON5, ZON)
    #[arg(long, overrides_with = "pretty")]
    compact: bool,

    /// Multi-line, indented output (the default)
    #[arg(long, overrides_with = "compact")]
    pretty: bool,

    /// Spaces per indent level for pretty JSON, and for TOML's wrapped arrays (default 2)
    #[arg(long, value_name = "N", value_parser = indent_arg, allow_hyphen_values = true)]
    indent: Option<u8>,

    /// TOML column budget (default 80); a mapping/array that fits stays
    /// inline, a wider one expands to a [section] / wrapped array
    #[arg(long, value_name = "N", value_parser = width_arg, allow_hyphen_values = true)]
    width: Option<u16>,
}

impl Layout {
    fn apply(&self, serialize: &mut SerializeOptions) {
        if self.compact {
            serialize.pretty = false;
        } else if self.pretty {
            serialize.pretty = true;
        }
        if let Some(n) = self.indent {
            serialize.indent = n;
            // fig has no `pretty` gate of its own to read `indent`'s value
            // against (see `SerializeOptions.indent_requested`'s doc comment),
            // so an explicit `--indent` is fig's own on/off signal,
            // independent of the numeric width other formats use it for.
            serialize.indent_requested = true;
        }
        if let Some(w) = self.width {
            serialize.width = w;
        }
    }
}

/// `--lossless`/`--lossy`, the last one given winning.
#[derive(Args, Debug)]
struct Fidelity {
    /// Preserve values the target can't represent natively (e.g. a null in
    /// TOML, a TOML datetime in JSON) via a $fig envelope, and reconstruct
    /// any such envelope in the input
    #[arg(long, overrides_with = "lossy")]
    lossless: bool,

    /// Emit clean, idiomatic output instead (the default)
    #[arg(long, overrides_with = "lossless")]
    lossy: bool,
}

impl Fidelity {
    /// Whether the last of the two given was `--lossless`.
    fn lossless(&self) -> bool {
        self.lossless && !self.lossy
    }
}

#[derive(Args, Debug)]
struct ReplaceArgs {
    #[command(flatten)]
    reading: ValueReading,

    /// The file to edit
    file: String,

    /// Where the value to replace is
    #[arg(value_parser = path_arg, long_help = help::PATH)]
    path: DocPath,

    /// The new value
    #[arg(long_help = help::VALUE)]
    value: String,

    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Args, Debug)]
struct RenameArgs {
    /// The file to edit
    file: String,

    /// Where the key to rename is
    #[arg(value_parser = path_arg, long_help = help::PATH)]
    path: DocPath,

    /// The key's new name
    name: String,

    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Args, Debug)]
struct SetArgs {
    /// Reconcile the sequence at <PATH> to exactly the <VALUE>s
    ///
    /// Keeps the comments on items that survive (only new items are
    /// inserted, only dropped ones removed; the result's order matches the
    /// arguments). An absent or null <PATH> is started as a list of the
    /// <VALUE>s, each read as a value is.
    #[arg(long)]
    seq: bool,

    #[command(flatten)]
    reading: ValueReading,

    /// The file to edit, or to create
    file: String,

    /// Where to set the value
    #[arg(value_parser = path_arg, long_help = help::SET_PATH)]
    path: DocPath,

    /// The value; with --seq, the sequence's items
    #[arg(required = true, long_help = help::VALUE)]
    value: Vec<String>,

    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Args, Debug)]
struct InsertArgs {
    #[command(flatten)]
    reading: ValueReading,

    /// The file to edit
    file: String,

    /// The slot to create
    #[arg(value_parser = path_arg, long_help = help::INSERT_PATH)]
    path: DocPath,

    /// The value to insert
    #[arg(long_help = help::VALUE)]
    value: String,

    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Args, Debug)]
struct DeleteArgs {
    /// The file to edit
    file: String,

    /// What to remove
    #[arg(value_parser = path_arg, long_help = help::PATH)]
    path: DocPath,

    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Args, Debug)]
struct CommentArgs {
    /// Target the same-line trailing comment on the value at <PATH>
    /// instead (adding one replaces any existing one on that line)
    #[arg(long)]
    inline: bool,

    /// Remove the targeted comment instead of adding it; <TEXT> is then
    /// omitted (a no-op when there is no such comment)
    #[arg(long)]
    delete: bool,

    /// Print the targeted comment to stdout (markers stripped) and make no
    /// change; <TEXT> is then omitted (exits 1, printing nothing, when
    /// there is no such comment)
    #[arg(long)]
    get: bool,

    /// The file to edit
    file: String,

    /// The node the comment is on
    #[arg(value_parser = path_arg, long_help = help::PATH)]
    path: DocPath,

    /// The comment's text; it may span several lines, one comment line each
    #[arg(required_unless_present_any = ["delete", "get"], conflicts_with_all = ["delete", "get"])]
    text: Option<String>,

    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Args, Debug)]
struct GetArgs {
    /// Input format of <FILE> (defaults to the file extension, then to
    /// sniffing the file's contents if the extension is unknown)
    #[arg(short, long, value_name = "FORMAT", value_parser = format_arg, allow_hyphen_values = true)]
    input: Option<Format>,

    /// Output format (defaults to the input format)
    #[arg(short, long, value_name = "FORMAT", value_parser = format_arg, allow_hyphen_values = true)]
    output: Option<Format>,

    /// The file to read; `-` reads stdin
    file: String,

    /// The part of the file to print (the whole file without one)
    ///
    /// A scalar there prints as its text and a newline — a string
    /// unquoted, anything else as fig spells it (42, true, null) — in
    /// every format, so $(fig get f name) is the value. A container prints
    /// as a document. With -o, the fragment prints in that format's own
    /// spelling (-o json: "hi").
    ///
    /// Dot syntax for keys, bracket syntax for indices; a key holding a .
    /// or [ is quoted or escaped: a."b.c", a.'b.c', a["b.c"] (as `-o gron`
    /// prints it), or a.b\.c. Example: school.class[0].student[3]. [-] or
    /// [$] in place of an index means "the last item".
    #[arg(value_parser = path_arg)]
    path: Option<DocPath>,

    /// Root identifier for `-o gron` (default "json")
    #[arg(long, value_name = "NAME", allow_hyphen_values = true)]
    gron_root: Option<String>,

    /// Key/value separator for `-o gron` (default " = ")
    ///
    /// Print-only: ungron always splits on " = ", so a custom separator is
    /// one-way unless it matches the default.
    #[arg(long, value_name = "STR", allow_hyphen_values = true)]
    gron_sep: Option<String>,

    /// Per-line terminator for `-o gron` (default ";"); pass "" to drop it
    ///
    /// ungron strips an optional ";" regardless.
    #[arg(long, value_name = "STR", allow_hyphen_values = true)]
    gron_term: Option<String>,

    #[command(flatten)]
    layout: Layout,

    /// Drop comments instead of carrying them across formats
    #[arg(long)]
    strip_comments: bool,

    #[command(flatten)]
    fidelity: Fidelity,

    /// Drop unknown/custom YAML tags instead of erroring, when converting away from YAML
    #[arg(long)]
    lax_tags: bool,

    /// Suppress warnings on stderr — lossy conversions, and figl authoring
    /// lints (`Yes`-style strings, a likely missing comma in a flow value,
    /// indent/marker-count disagreement, ...)
    #[arg(short, long, visible_alias = "no-warnings")]
    quiet: bool,

    /// Treat any warning as an error (exit 1)
    #[arg(long)]
    strict: bool,

    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Args, Debug)]
struct CheckArgs {
    /// Parse every file as this format (default: infer from each file's
    /// extension, then by sniffing its contents)
    #[arg(short, long, value_name = "FORMAT", value_parser = format_arg, allow_hyphen_values = true)]
    input: Option<Format>,

    /// Validate against a specific language version, where one is
    /// selectable: TOML `1.0`/`1.1` (default 1.1), YAML `1.2.2`/`1.1`
    /// (default 1.2.2)
    ///
    /// JSON strictness is the format itself (json vs jsonc vs json5).
    #[arg(short, long, value_name = "VERSION", allow_hyphen_values = true)]
    spec: Option<String>,

    /// Suppress the per-file `ok` lines and figl authoring warnings; errors still print
    #[arg(short, long, visible_alias = "no-warnings")]
    quiet: bool,

    /// A file that parses with warnings fails too (exit 1)
    #[arg(long)]
    strict: bool,

    /// The files to check; `-` reads stdin
    #[arg(required = true)]
    files: Vec<String>,

    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Args, Debug)]
struct FmtArgs {
    /// Input format (defaults to the file extension, then to sniffing the
    /// file's contents if the extension is unknown)
    #[arg(short, long, value_name = "FORMAT", value_parser = format_arg, allow_hyphen_values = true)]
    input: Option<Format>,

    /// Print the reformatted result to stdout instead of writing it back
    ///
    /// Exits 1 if reformatting would change the file, 0 if it's already
    /// clean — a check gate for pre-commit/CI.
    #[arg(long, conflicts_with = "diff")]
    dry_run: bool,

    /// Like --dry-run, but print a unified diff of the change to stdout
    /// instead of the whole reformatted file
    ///
    /// Nothing is printed (and exit is 0) when the file is already clean.
    #[arg(long)]
    diff: bool,

    #[command(flatten)]
    layout: Layout,

    /// Drop comments instead of re-emitting them
    #[arg(long)]
    strip_comments: bool,

    /// Suppress warnings on stderr
    #[arg(short, long, visible_alias = "no-warnings")]
    quiet: bool,

    /// Treat any warning as an error (exit 1, no write)
    #[arg(long)]
    strict: bool,

    /// The file to reformat; `-` reads stdin, but only with --dry-run or
    /// --diff: there is nowhere to write an in-place result back to
    file: String,

    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Args, Debug)]
struct ConvertArgs {
    /// The format to convert to
    #[arg(short, long, value_name = "FORMAT", value_parser = format_arg, allow_hyphen_values = true)]
    output: Format,

    /// The format of <FILE> (else its extension, else sniffed from its contents)
    #[arg(short, long, value_name = "FORMAT", value_parser = format_arg, allow_hyphen_values = true)]
    input: Option<Format>,

    /// Write the converted result back to <FILE> in place (skipped if it's
    /// already byte-identical) instead of printing it
    #[arg(short, long)]
    write: bool,

    /// Print a unified diff of the change instead of the whole converted
    /// file; combine with --write to write AND see what changed
    #[arg(long)]
    diff: bool,

    /// Not `convert`'s: it prints by default, and --write writes.
    #[arg(long, hide = true)]
    dry_run: bool,

    #[command(flatten)]
    layout: Layout,

    /// Drop comments instead of carrying them across formats
    #[arg(long)]
    strip_comments: bool,

    #[command(flatten)]
    fidelity: Fidelity,

    /// Drop unknown/custom YAML tags instead of erroring, when converting away from YAML
    #[arg(long)]
    lax_tags: bool,

    /// Suppress warnings on stderr
    #[arg(short, long, visible_alias = "no-warnings")]
    quiet: bool,

    /// Treat any warning as an error (exit 1, no write)
    #[arg(long)]
    strict: bool,

    /// The file to convert; `-` reads stdin, but only without --write
    file: String,

    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Args, Debug)]
struct PatchArgs {
    /// The document to change, in place; `-` reads stdin, which needs
    /// --dry-run or --diff
    file: String,

    /// The document supplying the change; `-` reads stdin (only one of the
    /// two files can be)
    patch_file: String,

    /// Merge into that path in <FILE> instead of its root
    #[arg(long, value_name = "PATH", value_parser = path_arg, allow_hyphen_values = true)]
    at: Option<DocPath>,

    /// Take only that subtree of <PATCH_FILE>
    ///
    /// Together, --at and --from move one part of one file into another
    /// part of another: `fig patch app.yaml defaults.toml --from db --at
    /// service.db`.
    #[arg(long, value_name = "PATH", value_parser = path_arg, allow_hyphen_values = true)]
    from: Option<DocPath>,

    /// Remove a path from <FILE> after the merge; repeatable
    ///
    /// A path that isn't there is not an error.
    #[arg(long, value_name = "PATH", value_parser = path_arg, allow_hyphen_values = true)]
    delete: Vec<DocPath>,

    /// What to do when both files hold a sequence at the same path
    ///
    /// `replace` (default) takes the patch's; `append` adds every item;
    /// `union` adds only the items <FILE> does not already hold, compared
    /// by value rather than by source text.
    #[arg(long, value_name = "replace|append|union", value_parser = seq_arg, allow_hyphen_values = true)]
    seq: Option<SeqStrategy>,

    /// Whose comment wins where both files carry one in the same position
    ///
    /// `ours` (default) keeps <FILE>'s and contributes the patch's only
    /// where <FILE> has none; `theirs` lets the patch overwrite; `none`
    /// carries no comment from the patch at all. Comments nested inside a
    /// subtree the patch contributes whole always ride along with it unless
    /// `none`.
    #[arg(long, value_name = "ours|theirs|none", value_parser = comments_arg, allow_hyphen_values = true)]
    comments: Option<CommentStrategy>,

    /// The same as `--comments none`, in `get`'s word
    ///
    /// No comment from the patch reaches <FILE>; <FILE>'s own are never
    /// stripped — they are what patch keeps.
    #[arg(long)]
    strip_comments: bool,

    /// The format of <FILE> (else its extension, else sniffed)
    #[arg(short, long, value_name = "FORMAT", value_parser = format_arg, allow_hyphen_values = true)]
    input: Option<Format>,

    /// The format of <PATCH_FILE> (else its extension, else sniffed)
    #[arg(long, value_name = "FORMAT", value_parser = format_arg, allow_hyphen_values = true)]
    patch_input: Option<Format>,

    #[command(flatten)]
    fidelity: Fidelity,

    #[command(flatten)]
    layout: Layout,

    /// Print the patched document to stdout; write nothing
    #[arg(long)]
    dry_run: bool,

    /// Print a unified diff of the change; write nothing
    ///
    /// Neither preview mode sets a non-zero exit status — a patch is
    /// expected to change the file.
    #[arg(long)]
    diff: bool,

    /// Suppress the patch file's warnings and the dropped-comment note
    #[arg(short, long, visible_alias = "no-warnings")]
    quiet: bool,

    /// Treat any warning as an error (exit 1, no write)
    #[arg(long)]
    strict: bool,

    #[command(flatten)]
    help: HelpFlag,
}

/// `lang`'s flags are taken before or after its verb, by any verb, as they
/// always were; each says which verb reads it.
#[derive(Args, Debug)]
struct LangArgs {
    /// `check`: also hold the language to this format
    #[arg(long, global = true, value_name = "FORMAT", allow_hyphen_values = true)]
    against: Option<String>,

    /// `table`: read <FILE> as this format (else by its extension, else by its contents)
    #[arg(short, long, global = true, value_name = "FORMAT", value_parser = format_arg, allow_hyphen_values = true)]
    input: Option<Format>,

    /// `table`: the version of the format to read <FILE> as, where one is
    /// selectable, as `check`'s --spec
    #[arg(short, long, global = true, value_name = "VERSION", allow_hyphen_values = true)]
    spec: Option<String>,

    /// With none, `lang` lists.
    #[command(subcommand)]
    verb: Option<LangVerbArgs>,

    #[command(flatten)]
    help: HelpFlag,
}

#[derive(Subcommand, Debug)]
enum LangVerbArgs {
    /// Every built-in language, and every configured one with what it can
    /// do (read, edit, serialize), its extensions and the file that
    /// configured it — or why it was refused
    List,

    /// Load a configured language and report what the harness found
    ///
    /// --against <FORMAT> also holds it to another format: parse each of
    /// the language's samples and each file given with both, and compare
    /// the tables row for row, the way a reimplementation of a built-in
    /// language is proven (`fig lang check lua-dotenv --against dotenv
    /// secrets.env`). Exits 1 on the first difference.
    Check {
        /// The language's name
        name: String,
        /// Files to parse with both, with --against
        files: Vec<String>,
    },

    /// Print a file's node table, as the JSON a helper answers `parse` with
    ///
    /// Parse the file (as -i <FORMAT>, else by its extension, else by its
    /// contents) and print its node table — what a twin has to produce,
    /// row for row, to pass `check --against`. --spec selects a version of
    /// the format where one is selectable, as `check`'s does.
    Table {
        /// The file to read
        file: String,
    },
}

/// Parse the command line into the `CliConfig` the actions take. A command
/// line clap refuses, or one it reads that `into_options` then refuses, is a
/// `clap::Error` (`exit()` prints it and exits 2; `--help` is one too, which
/// prints the help and exits 0).
pub fn parse_config<I: IntoIterator<Item = String>>(argv: I) -> Result<CliConfig, clap::Error> {
    let argv: Vec<String> = argv.into_iter().collect();
    let binary_name = argv.first().cloned().unwrap_or_else(|| "fig".to_string());
    let cli = Cli::try_parse_from(&argv)?;
    // `--lang` decides every file's language, whatever its extension, so it
    // is recorded before any file's is resolved.
    if cli.lang.is_some() {
        languages::set_lang_override(cli.lang);
    }
    let options = match cli.action {
        None => CliActionOptions::Help(HelpOptions::default()),
        Some(action) => action.into_options()?,
    };
    Ok(CliConfig { options, binary_name })
}

/// A usage error clap cannot see for itself, reported the way clap reports
/// its own — with `action`'s usage, exit 2.
fn usage_error(action: &str, kind: ErrorKind, message: impl std::fmt::Display) -> clap::Error {
    let mut cmd = Cli::command();
    cmd.build();
    match cmd.find_subcommand_mut(action) {
        Some(sub) => sub.error(kind, message),
        None => cmd.error(kind, message),
    }
}

/// The format a file's extension names (or `--lang`), and whether there was
/// none — in which case the handler sniffs the file's contents. `canonical`
/// stands in until it does.
fn by_extension(file: &str) -> (Format, bool) {
    match detect_language_from_file_ending(file) {
        Some(d) => (d.format, false),
        None => (Format::CANONICAL, true),
    }
}

impl Action {
    fn into_options(self) -> Result<CliActionOptions, clap::Error> {
        Ok(match self {
            Action::Help(a) => {
                if let Some(topic) = a.topic {
                    // `fig help set` reads as asking for `set`'s help: say where it is.
                    let message = if Cli::command().find_subcommand(&topic).is_some() {
                        format!("help takes no arguments; for {topic}'s help, run `fig {topic} --help`")
                    } else {
                        format!("help takes no arguments; unexpected argument: {topic}")
                    };
                    return Err(usage_error("help", ErrorKind::UnknownArgument, message));
                }
                CliActionOptions::Help(HelpOptions { oneline: a.oneline })
            }
            Action::Version(_) => CliActionOptions::Version,
            // `replace` swaps the value at a path; `rename` renames the key
            // there. One operation each — no flag turns one into the other
            // (cli-5 §7).
            Action::Replace(a) => {
                let (format, detect) = by_extension(&a.file);
                CliActionOptions::Replace(EditOptions {
                    file: a.file,
                    path: a.path.0,
                    replacement: a.value,
                    key: false,
                    value_mode: a.reading.mode(),
                    format,
                    detect,
                })
            }
            Action::Rename(a) => {
                let (format, detect) = by_extension(&a.file);
                CliActionOptions::Rename(EditOptions {
                    file: a.file,
                    path: a.path.0,
                    replacement: a.name,
                    key: true,
                    value_mode: ValueMode::Fig,
                    format,
                    detect,
                })
            }
            Action::Set(a) => {
                if !a.seq && a.value.len() > 1 {
                    return Err(usage_error(
                        "set",
                        ErrorKind::TooManyValues,
                        format!("set takes one value (--seq takes several); unexpected argument: {}", a.value[1]),
                    ));
                }
                let (format, detect) = by_extension(&a.file);
                CliActionOptions::Set(SetOptions {
                    file: a.file,
                    path: a.path.0,
                    value: if a.seq { String::new() } else { a.value[0].clone() },
                    seq: a.seq,
                    values: if a.seq { a.value } else { Vec::new() },
                    value_mode: a.reading.mode(),
                    format,
                    detect,
                })
            }
            Action::Insert(a) => {
                let (format, detect) = by_extension(&a.file);
                CliActionOptions::Insert(InsertOptions {
                    file: a.file,
                    path: a.path.0,
                    value: a.value,
                    value_mode: a.reading.mode(),
                    format,
                    detect,
                })
            }
            Action::Delete(a) => {
                let (format, detect) = by_extension(&a.file);
                CliActionOptions::Delete(DeleteOptions { file: a.file, path: a.path.0, format, detect })
            }
            Action::Comment(a) => {
                let (format, detect) = by_extension(&a.file);
                CliActionOptions::Comment(CommentOptions {
                    file: a.file,
                    path: a.path.0,
                    text: a.text.unwrap_or_default(),
                    inline_comment: a.inline,
                    delete: a.delete,
                    get: a.get,
                    format,
                    detect,
                })
            }
            Action::Get(a) => {
                let detected = if a.input.is_none() { detect_language_from_file_ending(&a.file) } else { None };
                // No `--input` and an unrecognized extension ⇒ sniff the
                // contents in the handler. `json` here is a placeholder
                // `from`/`to`, overwritten once the real format is known.
                let detect = a.input.is_none() && detected.is_none();
                let from = a.input.or(detected.map(|d| d.format)).unwrap_or_else(json_format);
                let mut serialize = SerializeOptions::default();
                a.layout.apply(&mut serialize);
                if a.strip_comments {
                    serialize.strip_comments = true;
                }
                let mut gron_projection = gron::Projection::gron();
                if let Some(r) = a.gron_root {
                    gron_projection.root_name = r.into_bytes();
                }
                if let Some(s) = a.gron_sep {
                    gron_projection.assign = s.into_bytes();
                }
                if let Some(t) = a.gron_term {
                    // Empty is allowed: it drops the terminator entirely.
                    gron_projection.terminator = t.into_bytes();
                }
                CliActionOptions::Get(GetOptions {
                    file: a.file,
                    path: a.path.map(|p| p.0),
                    from,
                    to: a.output.unwrap_or(from),
                    detect,
                    output_explicit: a.output.is_some(),
                    lax_tags: a.lax_tags,
                    lossless: a.fidelity.lossless(),
                    serialize,
                    quiet: a.quiet,
                    strict: a.strict,
                    gron_projection,
                })
            }
            Action::Check(a) => CliActionOptions::Check(CheckOptions {
                files: a.files,
                format: a.input,
                spec: a.spec,
                quiet: a.quiet,
                strict: a.strict,
            }),
            Action::Fmt(a) => {
                let detected = if a.input.is_none() { detect_language_from_file_ending(&a.file) } else { None };
                let detect = a.input.is_none() && detected.is_none();
                let from = a.input.or(detected.map(|d| d.format)).unwrap_or_else(json_format);
                let mut serialize = SerializeOptions::default();
                a.layout.apply(&mut serialize);
                if a.strip_comments {
                    serialize.strip_comments = true;
                }
                CliActionOptions::Fmt(FmtOptions {
                    file: a.file,
                    from,
                    detect,
                    serialize,
                    quiet: a.quiet,
                    strict: a.strict,
                    dry_run: a.dry_run,
                    diff: a.diff,
                })
            }
            Action::Convert(a) => {
                if a.dry_run {
                    return Err(usage_error(
                        "convert",
                        ErrorKind::UnknownArgument,
                        "convert prints to stdout by default; pass --write/-w to write in place instead.",
                    ));
                }
                let detected = detect_language_from_file_ending(&a.file);
                let detect = a.input.is_none() && detected.is_none();
                let from = a.input.or(detected.map(|d| d.format)).unwrap_or_else(json_format);
                let mut serialize = SerializeOptions::default();
                a.layout.apply(&mut serialize);
                if a.strip_comments {
                    serialize.strip_comments = true;
                }
                CliActionOptions::Convert(ConvertOptions {
                    file: a.file,
                    from,
                    to: a.output,
                    detect,
                    lax_tags: a.lax_tags,
                    lossless: a.fidelity.lossless(),
                    serialize,
                    quiet: a.quiet,
                    strict: a.strict,
                    write: a.write,
                    diff: a.diff,
                })
            }
            Action::Patch(a) => {
                let mut patch_options = patch::Options::default();
                a.layout.apply(&mut patch_options.serialize);
                if let Some(seq) = a.seq {
                    patch_options.seq = seq;
                }
                if let Some(comments) = a.comments {
                    patch_options.comments = comments;
                }
                if a.delete.iter().any(|d| d.0.is_empty()) {
                    return Err(usage_error(
                        "patch",
                        ErrorKind::InvalidValue,
                        "--delete needs a path within the document; the root cannot be deleted.",
                    ));
                }
                // `--strip-comments` is `get`/`convert`'s word for what
                // `patch` calls `--comments none`: no comment from the patch
                // reaches the target. (The target's own are never stripped —
                // they are what patch keeps.)
                if a.strip_comments {
                    if let Some(c) = a.comments.filter(|c| *c != CommentStrategy::None) {
                        return Err(usage_error(
                            "patch",
                            ErrorKind::ArgumentConflict,
                            format!(
                                "--strip-comments carries no comment from the patch; --comments {} asks for some. Pass one.",
                                c.name()
                            ),
                        ));
                    }
                    patch_options.comments = CommentStrategy::None;
                }
                // Both from a pipe would mean reading one stream twice and
                // getting half of each; the second read comes back empty and
                // the failure is an empty patch, which is silent. Refuse it up
                // front.
                if a.file == "-" && a.patch_file == "-" {
                    return Err(usage_error("patch", ErrorKind::ArgumentConflict, "only one of the two files can be stdin."));
                }
                let target_ext = detect_language_from_file_ending(&a.file);
                let source_ext = detect_language_from_file_ending(&a.patch_file);
                CliActionOptions::Patch(PatchOptions {
                    format: a.input.or(target_ext.map(|d| d.format)).unwrap_or_else(json_format),
                    detect: a.input.is_none() && target_ext.is_none(),
                    patch_format: a.patch_input.or(source_ext.map(|d| d.format)).unwrap_or_else(json_format),
                    detect_patch: a.patch_input.is_none() && source_ext.is_none(),
                    file: a.file,
                    patch_file: a.patch_file,
                    at: a.at.map(|p| p.0).unwrap_or_default(),
                    from: a.from.map(|p| p.0).unwrap_or_default(),
                    deletes: a.delete.into_iter().map(|p| p.0).collect(),
                    patch_options,
                    lossless: a.fidelity.lossless(),
                    dry_run: a.dry_run,
                    diff: a.diff,
                    quiet: a.quiet,
                    strict: a.strict,
                })
            }
            Action::Lang(a) => {
                let (verb, name, files) = match a.verb.unwrap_or(LangVerbArgs::List) {
                    LangVerbArgs::List => (LangVerb::List, String::new(), Vec::new()),
                    LangVerbArgs::Check { name, files } => (LangVerb::Check, name, files),
                    LangVerbArgs::Table { file } => (LangVerb::Table, file, Vec::new()),
                };
                CliActionOptions::Lang(LangOptions { verb, name, input: a.input, spec: a.spec, against: a.against, files })
            }
            Action::External(words) => {
                // Git's fallback, and the reason fig-schema can grow a CLI
                // without fig growing a `schema` action: a word fig has no
                // verb for is handed to a `fig-<word>` executable, with
                // everything after the word passed through untouched — see
                // `ExternalOptions.argv` — so none of it is parsed here.
                let mut words = words.into_iter();
                let name = words.next().map(|w| w.to_string_lossy().into_owned()).unwrap_or_default();
                match external_command_name(&name) {
                    Some(word) => {
                        let program = format!("fig-{word}");
                        let mut argv: Vec<OsString> = vec![program.clone().into()];
                        argv.extend(words);
                        CliActionOptions::External(ExternalOptions { name, program: Some(program), argv })
                    }
                    // Not one of fig's verbs, and not a word that could name
                    // a program either (`external_command_name` says which
                    // shapes those are). Still an `External`: a `None`
                    // `program` means the handoff was never on the table, so
                    // one place gets to say "no such action" for both halves
                    // of the unrecognized case.
                    None => CliActionOptions::External(ExternalOptions { name, program: None, argv: Vec::new() }),
                }
            }
        })
    }
}

/// The `<name>` for a `fig-<name>` handoff, or `None` if this word can't be one
/// — in which case it stays an ordinary unrecognized action.
///
/// The rule is deliberately narrower than "whatever `execvp` would take".
/// Restricting it to ASCII letters, digits, `-` and `_` keeps two shapes out:
/// a leading `-`, so a mistyped flag can never become a program name; and
/// anything containing a path separator or a `.`, so a forgotten verb
/// (`fig config.toml`, `fig ../x`) can't reach the filesystem as a program
/// name.
fn external_command_name(word: &str) -> Option<&str> {
    if word.is_empty() || word.starts_with('-') {
        return None;
    }
    if !word.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
        return None;
    }
    Some(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(items: &[&str]) -> Result<CliConfig, clap::Error> {
        parse_config(items.iter().map(|s| s.to_string()))
    }

    #[test]
    fn the_command_line_declaration_is_consistent() {
        crate::languages::init();
        Cli::command().debug_assert();
    }

    #[test]
    fn parse_path_reads_the_append_sentinel_and_literal_indices() {
        crate::languages::init();
        let dash = parse_path(b"list[-]").unwrap();
        assert_eq!(dash, [PathSeg::Key(b"list".to_vec()), PathSeg::Index(APPEND_INDEX)]);
        assert_eq!(parse_path(b"list[$]").unwrap()[1], PathSeg::Index(APPEND_INDEX));
        assert_eq!(parse_path(b"a.b[2]").unwrap()[2], PathSeg::Index(2));
    }

    #[test]
    fn parse_path_takes_a_key_with_a_dot_or_bracket_quoted_bracketed_or_escaped() {
        crate::languages::init();
        let key = |p: &[u8]| parse_path(p).unwrap();
        assert_eq!(key(b"a.\"b.c\""), [PathSeg::Key(b"a".to_vec()), PathSeg::Key(b"b.c".to_vec())]);
        assert_eq!(key(b"a.'b.c'"), [PathSeg::Key(b"a".to_vec()), PathSeg::Key(b"b.c".to_vec())]);
        assert_eq!(key(b"a[\"b.c\"]"), [PathSeg::Key(b"a".to_vec()), PathSeg::Key(b"b.c".to_vec())]);
        assert_eq!(key(b"a.b\\.c"), [PathSeg::Key(b"a".to_vec()), PathSeg::Key(b"b.c".to_vec())]);
        assert_eq!(key(b"\"\\u00e9\""), [PathSeg::Key("é".as_bytes().to_vec())]);
        assert!(parse_path(b"a.\"b").is_err());
        assert!(parse_path(b"a[").is_err());
    }

    #[test]
    fn parse_config_routes_insert_delete_to_the_right_action() {
        crate::languages::init();
        let c = parse(&["fig", "insert", "f.yaml", "a.b", "1"]).unwrap();
        assert!(matches!(c.options, CliActionOptions::Insert(_)));
        let c = parse(&["fig", "d", "f.yaml", "a[-]"]).unwrap();
        let CliActionOptions::Delete(d) = c.options else { panic!() };
        assert_eq!(d.path.last(), Some(&PathSeg::Index(APPEND_INDEX)));
    }

    #[test]
    fn parse_config_hands_an_unknown_action_to_fig_action() {
        crate::languages::init();
        let c = parse(&["fig", "schema", "lint", "--input", "x"]).unwrap();
        let CliActionOptions::External(e) = c.options else { panic!() };
        assert_eq!(e.program.as_deref(), Some("fig-schema"));
        assert_eq!(e.argv.len(), 4);
        let c = parse(&["fig", "config.toml"]).unwrap();
        let CliActionOptions::External(e) = c.options else { panic!() };
        assert!(e.program.is_none());
    }

    #[test]
    fn values_that_look_like_flags_and_numbers() {
        crate::languages::init();
        // A negative number is a value wherever it stands; after `--`,
        // anything is.
        let c = parse(&["fig", "set", "f.yaml", "a", "-5"]).unwrap();
        let CliActionOptions::Set(s) = c.options else { panic!() };
        assert_eq!(s.value, "-5");
        let c = parse(&["fig", "set", "f.yaml", "a", "--", "--dash"]).unwrap();
        let CliActionOptions::Set(s) = c.options else { panic!() };
        assert_eq!(s.value, "--dash");
        // An option's value is the next argument, whatever it looks like.
        let c = parse(&["fig", "get", "f.json", "--gron-sep", "--", "-o", "gron"]).unwrap();
        let CliActionOptions::Get(g) = c.options else { panic!() };
        assert_eq!(g.gron_projection.assign, b"--");
        // -inf is a flag, as fig has no bare -inf.
        assert!(parse(&["fig", "set", "f.yaml", "a", "-inf"]).is_err());
        // The last of a pair of opposites wins.
        let c = parse(&["fig", "get", "f.json", "--compact", "--pretty", "--lossless", "--lossy"]).unwrap();
        let CliActionOptions::Get(g) = c.options else { panic!() };
        assert!(g.serialize.pretty && !g.lossless);
    }

    #[test]
    fn external_command_name_accepts_only_words_that_could_name_a_program() {
        crate::languages::init();
        assert_eq!(external_command_name("schema"), Some("schema"));
        assert_eq!(external_command_name("a_b-1"), Some("a_b-1"));
        assert_eq!(external_command_name("--colour"), None);
        assert_eq!(external_command_name("x.toml"), None);
        assert_eq!(external_command_name("../x"), None);
        assert_eq!(external_command_name(""), None);
    }

    #[test]
    fn md_names_no_language() {
        crate::languages::init();
        assert!(detect_language_from_file_ending("x.md").is_none());
        assert!(detect_language_from_file_ending("x.markdown").is_none());
    }

    #[test]
    fn yml_is_an_accepted_spelling_of_yaml() {
        crate::languages::init();
        assert_eq!(parse_format_name("yml"), parse_format_name("yaml"));
        assert_eq!(detect_language_from_file_ending("a.yml").map(|d| d.format), parse_format_name("yaml"));
    }
}
