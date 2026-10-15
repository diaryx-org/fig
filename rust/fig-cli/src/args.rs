//! Port of src/cli/args.zig.
//!
//! Command-line parsing: turns the raw arg iterator into a `CliConfig`
//! (`parse_config`), plus the small format/path resolvers that
//! parsing (and later, the action handlers) share.

use fig_core::kit::ast::SerializeOptions;
use fig_core::patch::{self, CommentStrategy, SeqStrategy};

use crate::gron;
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
pub fn parse_path(path: &[u8]) -> Result<Vec<PathSeg>, ArgError> {
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
                        return Err(ArgError::InvalidPath);
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
                    return Err(ArgError::InvalidPath);
                }

                // `[-]` and `[$]` are the "end" tokens: `insert` reads the
                // sentinel as "append", and `delete` reads it as "the last
                // item". Any other caller that walks the path literally (e.g.
                // `get`) just sees an out-of-range index and surfaces NotFound.
                let inner = &path[start..i];
                let seg = if inner == b"-" || inner == b"$" {
                    PathSeg::Index(APPEND_INDEX)
                } else {
                    PathSeg::Index(gron::parse_usize(inner).ok_or(ArgError::InvalidPath)?)
                };
                path_in_progress.push(seg);
                // Skip close bracket
                i += 1;
            }
            b'"' | b'\'' => {
                let key = quoted_key(path, &mut i)?;
                // A quoted key ends its segment.
                if i < path.len() && path[i] != b'.' && path[i] != b'[' {
                    return Err(ArgError::InvalidPath);
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
                            return Err(ArgError::InvalidPath);
                        }
                        escaped = true;
                        i += 1;
                    }
                    i += 1;
                }
                if i == start {
                    return Err(ArgError::InvalidPath);
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

/// `parse_path` for a path on the command line: one that does not parse is a
/// usage error (exit 2) naming it, rather than an error name escaping `main`.
fn path_arg(path: &str) -> Result<Vec<PathSeg>, ArgError> {
    parse_path(path.as_bytes()).map_err(|_| {
        crate::log_err!(
            "`{path}` is not a path: keys are separated by `.`, an index is `[n]` (or `[-]` for the end), and a key holding `.` or `[` is quoted: a.\"b.c\"\n"
        );
        ArgError::InvalidPath
    })
}

/// The key quoted at `path[*i]` — `'…'` verbatim, `"…"` with JSON's
/// escapes decoded — leaving `*i` just past the closing quote.
fn quoted_key(path: &[u8], i: &mut usize) -> Result<Vec<u8>, ArgError> {
    let quote = path[*i];
    let start = *i + 1;
    let mut j = start;
    if quote == b'\'' {
        while j < path.len() && path[j] != b'\'' {
            j += 1;
        }
        if j >= path.len() {
            return Err(ArgError::InvalidPath);
        }
        *i = j + 1;
        return Ok(path[start..j].to_vec());
    }
    let mut out: Vec<u8> = Vec::new();
    loop {
        if j >= path.len() {
            return Err(ArgError::InvalidPath);
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
            return Err(ArgError::InvalidPath);
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
                        return Err(ArgError::InvalidPath);
                    }
                    let lo = hex4(path, j + 3)?;
                    if !(0xDC00..=0xDFFF).contains(&lo) {
                        return Err(ArgError::InvalidPath);
                    }
                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                    j += 6;
                } else if (0xDC00..=0xDFFF).contains(&cp) {
                    return Err(ArgError::InvalidPath);
                }
                let ch = char::from_u32(cp).ok_or(ArgError::InvalidPath)?;
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            _ => return Err(ArgError::InvalidPath),
        }
        j += 1;
    }
    *i = j + 1;
    Ok(out)
}

fn hex4(path: &[u8], at: usize) -> Result<u32, ArgError> {
    if at + 4 > path.len() {
        return Err(ArgError::InvalidPath);
    }
    let s = std::str::from_utf8(&path[at..at + 4]).map_err(|_| ArgError::InvalidPath)?;
    if s.starts_with('+') || s.starts_with('-') {
        return Err(ArgError::InvalidPath);
    }
    u32::from_str_radix(s, 16).map_err(|_| ArgError::InvalidPath)
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

/// The argument list with `--lang <name>` taken out: the one flag every
/// action accepts, read here once rather than in each action's loop, and
/// recorded for `detect_language_from_file_ending` to answer with.
struct Args {
    items: Vec<String>,
    i: usize,
}

impl Args {
    fn next(&mut self) -> Option<String> {
        if self.i >= self.items.len() {
            return None;
        }
        self.i += 1;
        Some(self.items[self.i - 1].clone())
    }
}

/// An action's positional arguments, read strictly. An argument that begins
/// with `-` and is none of the action's flags is a usage error, not a file
/// name; `--` ends the flags, so everything after it is a positional (a file
/// really named `-x` is `-- -x`). `-` alone (stdin) and a negative number
/// (`fig set f.yaml n -5`) are positionals wherever they stand.
///
/// Each action's loop asks `rest` first, so `--` and what follows it never
/// reach the flag comparisons, and hands `add` whatever no flag matched.
#[derive(Default)]
struct Positionals {
    items: Vec<String>,
    /// Set once `--` is seen.
    flags_done: bool,
}

impl Positionals {
    /// True when `arg` is taken here without being read as a flag: `--`
    /// itself, or anything after it.
    fn rest(&mut self, arg: &str) -> bool {
        if self.flags_done {
            self.items.push(arg.to_string());
            return true;
        }
        if arg == "--" {
            self.flags_done = true;
            return true;
        }
        false
    }

    /// Take `arg`, which matched none of `action`'s flags, as a positional.
    /// False, having said so, when it is spelled like a flag.
    fn add(&mut self, action: &str, arg: &str) -> bool {
        if spelled_as_flag(arg) {
            crate::log_err!("{action} has no flag {arg} (a file or value that begins with `-` goes after `--`)\n");
            return false;
        }
        self.items.push(arg.to_string());
        true
    }

    /// False, having said so, when there are more than `max` positionals.
    fn at_most(&self, action: &str, max: usize, what: &str) -> bool {
        if self.items.len() <= max {
            return true;
        }
        crate::log_err!("{action} takes {what}; unexpected argument: {}\n", self.items[max]);
        false
    }
}

/// Whether `flag` takes the next argument as its value in `action` — every
/// flag of every action that does, for `parse_config`'s `--lang` pre-pass,
/// which reads the line before any action does. `--seq` takes a strategy in
/// `patch` and nothing in `set`.
fn takes_value(flag: &str, action: &str) -> bool {
    const VALUED: [&str; 18] = [
        "--lang", "--input", "-i", "--output", "-o", "--indent", "--width", "--gron-root", "--gron-sep", "--gron-term", "--at",
        "--from", "--delete", "--comments", "--patch-input", "--against", "--spec", "-s",
    ];
    for v in VALUED {
        if flag == v {
            return !(flag == "--delete" && !is_patch(action));
        }
    }
    flag == "--seq" && is_patch(action)
}

fn is_patch(action: &str) -> bool {
    action == "patch" || action == "p"
}

/// Whether `arg` reads as a flag: a leading `-` with something after it
/// that does not start a number. `-inf` and `-nan` are flags here: fig has
/// no bare non-finite float (docs/spec.md § 5.3 — bare `-inf` is the string
/// it spells), so taking one as a value would quietly write a string where
/// a float was meant. `-- -inf` passes the string on purpose.
fn spelled_as_flag(arg: &str) -> bool {
    let b = arg.as_bytes();
    if b.len() < 2 || b[0] != b'-' {
        return false;
    }
    !(b[1].is_ascii_digit() || b[1] == b'.')
}

/// Take `arg` as `--string`/`--raw` into `mode`, refusing the pair: true when
/// it was one of them. `None` in `mode` is "not given" (the default reading).
fn value_mode_flag(action: &str, arg: &str, mode: &mut Option<ValueMode>, usage: ArgError) -> Result<bool, ArgError> {
    let this = match arg {
        "--string" => ValueMode::String,
        "--raw" => ValueMode::Raw,
        _ => return Ok(false),
    };
    if let Some(prev) = *mode {
        if prev != this {
            crate::log_err!("{action}: --string and --raw are two readings of the value; pass one.\n");
            return Err(usage);
        }
    }
    *mode = Some(this);
    Ok(true)
}

fn is_help(arg: &str) -> bool {
    arg == "--help" || arg == "-h"
}

/// `std.fmt.parseInt(u8/u16, n, 10)`.
fn parse_int<T: TryFrom<usize>>(n: &str) -> Option<T> {
    let v = gron::parse_usize(n.as_bytes())?;
    T::try_from(v).ok()
}

/// The value after flag `arg`, or a usage error naming it.
fn value_after(args: &mut Args, arg: &str, what: &str, usage: ArgError) -> Result<String, ArgError> {
    match args.next() {
        Some(v) => Ok(v),
        None => {
            crate::log_err!("Missing {what} after {arg}\n");
            Err(usage)
        }
    }
}

/// `--input`/`--output`'s format, or a usage error naming it.
fn format_after(args: &mut Args, arg: &str, usage: ArgError) -> Result<Format, ArgError> {
    let fmt = value_after(args, arg, "format value", usage)?;
    match parse_format_name(&fmt) {
        Some(f) => Ok(f),
        None => {
            crate::log_err!("Unsupported format: {fmt}\n");
            Err(ArgError::UnsupportedFileFormat)
        }
    }
}

/// The shared `--compact`/`--pretty`/`--strip-comments`/`--indent`/`--width`
/// flags of `get`, `fmt` and `convert`: true when `arg` was one.
fn serialize_flag(args: &mut Args, arg: &str, serialize: &mut SerializeOptions, usage: ArgError) -> Result<bool, ArgError> {
    match arg {
        "--compact" => serialize.pretty = false,
        "--pretty" => serialize.pretty = true,
        "--strip-comments" => serialize.strip_comments = true,
        "--indent" => {
            let n = value_after(args, arg, "value", usage)?;
            serialize.indent = match parse_int::<u8>(&n) {
                Some(v) => v,
                None => {
                    crate::log_err!("Invalid --indent value: {n}\n");
                    return Err(usage);
                }
            };
            // fig has no `pretty` gate of its own to read `indent`'s value
            // against (see `SerializeOptions.indent_requested`'s doc comment), so
            // an explicit `--indent` is fig's own on/off signal, independent
            // of the numeric width other formats use it for.
            serialize.indent_requested = true;
        }
        "--width" => {
            let n = value_after(args, arg, "value", usage)?;
            serialize.width = match parse_int::<u16>(&n) {
                Some(v) => v,
                None => {
                    crate::log_err!("Invalid --width value: {n}\n");
                    return Err(usage);
                }
            };
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn is_quiet(arg: &str) -> bool {
    arg == "--quiet" || arg == "-q" || arg == "--no-warnings"
}

pub fn parse_config<I: Iterator<Item = String>>(args_in: I) -> Result<CliConfig, ArgError> {
    let mut config = CliConfig::default();
    let mut args_in = args_in;

    let mut collected: Vec<String> = Vec::new();
    // Past `--` every argument is a positional, `--lang` included — unless
    // that `--` is the value of the flag before it (`--gron-sep --`), which
    // ends nothing.
    let mut flags_done = false;
    let mut prev = String::new();
    while let Some(arg) = args_in.next() {
        let action = if collected.len() > 1 { collected[1].clone() } else { String::new() };
        if arg == "--" && !takes_value(&prev, &action) {
            flags_done = true;
        }
        if !flags_done && arg == "--lang" {
            let Some(name) = args_in.next() else {
                crate::log_err!("Missing language name after --lang\n");
                return Err(ArgError::UnsupportedFileFormat);
            };
            let Some(format) = parse_format_name(&name) else {
                // A refused helper has already said why, through `ensure`.
                if !languages::is_configured(&name) {
                    crate::log_err!(
                        "No language named `{name}`: not a compiled format, and no languages.figl configures it (see `fig lang --help`)\n"
                    );
                }
                return Err(ArgError::UnsupportedFileFormat);
            };
            languages::set_lang_override(Some(format));
            prev = arg;
            continue;
        }
        prev = arg.clone();
        collected.push(arg);
    }
    let mut args = Args { items: collected, i: 0 };

    config.binary_name = args.next().unwrap_or_else(|| "fig".to_string());

    let Some(action_str) = args.next() else {
        config.action = CliAction::Help;
        config.options = CliActionOptions::Help(HelpOptions::default());
        return Ok(config);
    };
    let a = action_str.as_str();

    if a == "help" || a == "--help" || a == "-h" {
        config.action = CliAction::Help;
        let mut opts = HelpOptions { requested_help: true, oneline: false };
        // Nothing follows but a repeated `--help` (`fig help --help`): strict
        // like every other action, so `fig help --bogus` is not a silent 0.
        while let Some(arg) = args.next() {
            if is_help(&arg) {
                continue;
            }
            if arg == "--oneline" {
                opts.oneline = true;
                continue;
            }
            // `fig help set` reads as asking for `set`'s help: say where it is.
            if let Some(named) = CliAction::from_name(&arg) {
                if named != CliAction::External {
                    crate::log_err!("{a} takes no arguments; for {arg}'s help, run `fig {arg} --help`\n");
                    return Err(ArgError::HelpNamesAction);
                }
            }
            crate::log_err!("{a} takes no arguments; unexpected argument: {arg}\n");
            return Err(ArgError::SurplusArgument);
        }
        config.options = CliActionOptions::Help(opts);
    } else if a == "version" || a == "--version" || a == "-v" {
        config.action = CliAction::Version;
        let mut opts = VersionOptions::default();
        // `--help` is the one thing that may follow, as for every action.
        while let Some(arg) = args.next() {
            if is_help(&arg) {
                opts.requested_help = true;
                continue;
            }
            crate::log_err!("{a} takes no arguments; unexpected argument: {arg}\n");
            return Err(ArgError::SurplusArgument);
        }
        config.options = CliActionOptions::Version(opts);
    } else if a == "replace" || a == "rename" {
        // `replace` swaps the value at a path; `rename` renames the key there.
        // One operation each — no flag turns one into the other (cli-5 §7).
        let rename = a == "rename";
        let usage = if rename { ArgError::MissingRenameArgument } else { ArgError::MissingReplaceArgument };
        config.action = if rename { CliAction::Rename } else { CliAction::Replace };

        let mut value_mode: Option<ValueMode> = None;
        let mut requested_help = false;
        let mut positionals = Positionals::default();
        while let Some(arg) = args.next() {
            if positionals.rest(&arg) {
                continue;
            }
            if is_help(&arg) {
                requested_help = true;
            } else if !rename && value_mode_flag(a, &arg, &mut value_mode, usage)? {
                // taken
            } else if !positionals.add(a, &arg) {
                return Err(usage);
            }
        }
        let pos = &positionals.items;

        let mut file_path = String::new();
        let mut path: Vec<PathSeg> = Vec::new();
        let mut replacement = String::new();
        if !requested_help {
            if pos.len() < 3 {
                crate::log_err!(
                    "{}\n",
                    match pos.len() {
                        0 => "No file provided.",
                        1 => "No path provided.",
                        _ =>
                            if rename {
                                "No new key name provided."
                            } else {
                                "No value provided."
                            },
                    }
                );
                return Err(usage);
            }
            if !positionals.at_most(a, 3, if rename { "a file, a path and a new name" } else { "a file, a path and a value" }) {
                return Err(usage);
            }
            file_path = pos[0].clone();
            path = path_arg(&pos[1])?;
            replacement = pos[2].clone();
        }

        // Skip extension detection when the user only asked for help. An
        // unrecognized extension is not an error here: `detect = true`
        // defers to content sniffing in the handler.
        let ext = if requested_help { None } else { detect_language_from_file_ending(&file_path) };
        let opts = EditOptions {
            file: file_path,
            path,
            replacement,
            key: rename,
            value_mode: value_mode.unwrap_or_default(),
            requested_help,
            format: ext.map_or(Format::CANONICAL, |d| d.format),
            detect: !requested_help && ext.is_none(),
        };
        config.options = if rename { CliActionOptions::Rename(opts) } else { CliActionOptions::Replace(opts) };
    } else if a == "set" || a == "s" {
        config.action = CliAction::Set;

        // Leading flags in any order: `--seq`, `--string`/`--raw`, `--help`.
        // Positionals follow: file, path, then the value (or, with `--seq`, the
        // sequence items).
        let mut seq = false;
        let mut value_mode: Option<ValueMode> = None;
        let mut requested_help = false;
        let mut positionals = Positionals::default();
        while let Some(arg) = args.next() {
            if positionals.rest(&arg) {
                continue;
            }
            if is_help(&arg) {
                requested_help = true;
            } else if arg == "--seq" {
                seq = true;
            } else if value_mode_flag("set", &arg, &mut value_mode, ArgError::MissingSetArgument)? {
                // taken
            } else if !positionals.add("set", &arg) {
                return Err(ArgError::MissingSetArgument);
            }
        }
        let pos = &positionals.items;

        if requested_help {
            config.options = CliActionOptions::Set(SetOptions {
                file: String::new(),
                path: Vec::new(),
                value: String::new(),
                seq: false,
                values: Vec::new(),
                value_mode: ValueMode::Fig,
                requested_help: true,
                format: json_format(),
                detect: false,
            });
        } else {
            // Need file, path, and at least one value (a scalar, or one or more
            // sequence items with `--seq`).
            if pos.len() < 3 {
                crate::log_err!("set needs a file, a path, and a value (e.g. `fig set f.yaml a.b 1`).\n");
                return Err(ArgError::MissingSetArgument);
            }
            if !seq && !positionals.at_most("set", 3, "a file, a path and one value (--seq takes several)") {
                return Err(ArgError::MissingSetArgument);
            }
            let file_path = pos[0].clone();
            let path = path_arg(&pos[1])?;
            let ext = detect_language_from_file_ending(&file_path);
            config.options = CliActionOptions::Set(SetOptions {
                file: file_path,
                path,
                value: if seq { String::new() } else { pos[2].clone() },
                seq,
                values: if seq { pos[2..].to_vec() } else { Vec::new() },
                value_mode: value_mode.unwrap_or_default(),
                requested_help: false,
                format: ext.map_or(Format::CANONICAL, |d| d.format),
                // Sniff the content when the extension resolved nothing.
                detect: ext.is_none(),
            });
        }
    } else if a == "insert" || a == "i" {
        config.action = CliAction::Insert;

        let mut value_mode: Option<ValueMode> = None;
        let mut requested_help = false;
        let mut positionals = Positionals::default();
        while let Some(arg) = args.next() {
            if positionals.rest(&arg) {
                continue;
            }
            if is_help(&arg) {
                requested_help = true;
            } else if value_mode_flag("insert", &arg, &mut value_mode, ArgError::MissingInsertArgument)? {
                // taken
            } else if !positionals.add("insert", &arg) {
                return Err(ArgError::MissingInsertArgument);
            }
        }
        let pos = &positionals.items;

        let mut file_path = String::new();
        let mut path: Vec<PathSeg> = Vec::new();
        let mut value = String::new();
        if !requested_help {
            if pos.len() < 3 {
                crate::log_err!(
                    "{}\n",
                    match pos.len() {
                        0 => "No file provided.",
                        1 => "No path provided.",
                        _ => "No value provided.",
                    }
                );
                return Err(ArgError::MissingInsertArgument);
            }
            if !positionals.at_most("insert", 3, "a file, a path and a value") {
                return Err(ArgError::MissingInsertArgument);
            }
            file_path = pos[0].clone();
            path = path_arg(&pos[1])?;
            value = pos[2].clone();
        }

        let ext = if requested_help { None } else { detect_language_from_file_ending(&file_path) };
        config.options = CliActionOptions::Insert(InsertOptions {
            file: file_path,
            path,
            value,
            value_mode: value_mode.unwrap_or_default(),
            requested_help,
            format: ext.map_or(Format::CANONICAL, |d| d.format),
            detect: !requested_help && ext.is_none(),
        });
    } else if a == "delete" || a == "d" {
        config.action = CliAction::Delete;

        let mut requested_help = false;
        let mut positionals = Positionals::default();
        while let Some(arg) = args.next() {
            if positionals.rest(&arg) {
                continue;
            }
            if is_help(&arg) {
                requested_help = true;
            } else if !positionals.add("delete", &arg) {
                return Err(ArgError::MissingDeleteArgument);
            }
        }
        let pos = &positionals.items;

        let mut file_path = String::new();
        let mut path: Vec<PathSeg> = Vec::new();
        if !requested_help {
            if pos.len() < 2 {
                crate::log_err!("{}\n", if pos.is_empty() { "No file provided." } else { "No path provided." });
                return Err(ArgError::MissingDeleteArgument);
            }
            if !positionals.at_most("delete", 2, "a file and a path") {
                return Err(ArgError::MissingDeleteArgument);
            }
            file_path = pos[0].clone();
            path = path_arg(&pos[1])?;
        }

        let ext = if requested_help { None } else { detect_language_from_file_ending(&file_path) };
        config.options = CliActionOptions::Delete(DeleteOptions {
            file: file_path,
            path,
            requested_help,
            format: ext.map_or(Format::CANONICAL, |d| d.format),
            detect: !requested_help && ext.is_none(),
        });
    } else if a == "comment" || a == "c" {
        config.action = CliAction::Comment;

        // Flags, in any order: `--inline`, `--delete`, `--get`.
        let mut inline_comment = false;
        let mut delete = false;
        let mut get = false;
        let mut requested_help = false;
        let mut positionals = Positionals::default();
        while let Some(arg) = args.next() {
            if positionals.rest(&arg) {
                continue;
            }
            if is_help(&arg) {
                requested_help = true;
            } else if arg == "--inline" {
                inline_comment = true;
            } else if arg == "--delete" {
                delete = true;
            } else if arg == "--get" {
                get = true;
            } else if !positionals.add("comment", &arg) {
                return Err(ArgError::MissingCommentArgument);
            }
        }
        let pos = &positionals.items;

        let mut file_path = String::new();
        let mut path: Vec<PathSeg> = Vec::new();
        let mut text = String::new();
        if !requested_help {
            // Delete/get need no text; add/set requires it.
            let wants_text = !delete && !get;
            if pos.len() < 2 || (wants_text && pos.len() < 3) {
                crate::log_err!(
                    "{}\n",
                    match pos.len() {
                        0 => "No file provided.",
                        1 => "No path provided.",
                        _ => "No comment text provided.",
                    }
                );
                return Err(ArgError::MissingCommentArgument);
            }
            if !positionals.at_most(
                "comment",
                if wants_text { 3 } else { 2 },
                if wants_text { "a file, a path and the comment text" } else { "a file and a path with --get/--delete" },
            ) {
                return Err(ArgError::MissingCommentArgument);
            }
            file_path = pos[0].clone();
            path = path_arg(&pos[1])?;
            if wants_text {
                text = pos[2].clone();
            }
        }

        let ext = if requested_help { None } else { detect_language_from_file_ending(&file_path) };
        config.options = CliActionOptions::Comment(CommentOptions {
            file: file_path,
            path,
            text,
            inline_comment,
            delete,
            get,
            requested_help,
            format: ext.map_or(Format::CANONICAL, |d| d.format),
            detect: !requested_help && ext.is_none(),
        });
    } else if a == "get" || a == "g" {
        config.action = CliAction::Get;
        let usage = ArgError::MissingGetArgument;

        let mut input_override: Option<Format> = None;
        let mut output_override: Option<Format> = None;
        let mut lax_tags = false;
        let mut lossless = false;
        let mut quiet = false;
        let mut strict = false;
        let mut serialize = SerializeOptions::default();
        // gron projection overrides; `None` means "keep the gron default".
        let mut gron_root: Option<String> = None;
        let mut gron_sep: Option<String> = None;
        let mut gron_term: Option<String> = None;
        let mut requested_help = false;
        let mut positionals = Positionals::default();

        while let Some(arg) = args.next() {
            if positionals.rest(&arg) {
                continue;
            }
            if is_help(&arg) {
                requested_help = true;
            } else if arg == "--lax-tags" {
                lax_tags = true;
            } else if arg == "--gron-root" {
                gron_root = Some(value_after(&mut args, &arg, "value", usage)?);
            } else if arg == "--gron-sep" {
                gron_sep = Some(value_after(&mut args, &arg, "value", usage)?);
            } else if arg == "--gron-term" {
                // Empty is allowed (drop the terminator entirely).
                gron_term = Some(value_after(&mut args, &arg, "value", usage)?);
            } else if arg == "--lossless" {
                lossless = true;
            } else if arg == "--lossy" {
                lossless = false;
            } else if serialize_flag(&mut args, &arg, &mut serialize, usage)? {
                // taken
            } else if is_quiet(&arg) {
                quiet = true;
            } else if arg == "--strict" {
                strict = true;
            } else if arg == "--input" || arg == "-i" {
                // Delegate to the same registry-driven lookup `check`/`fmt`/
                // `convert` use, so this never drifts out of sync as formats
                // are added.
                input_override = Some(format_after(&mut args, &arg, usage)?);
            } else if arg == "--output" || arg == "-o" {
                output_override = Some(format_after(&mut args, &arg, usage)?);
            } else if !positionals.add("get", &arg) {
                return Err(usage);
            }
        }
        let pos = &positionals.items;

        if !requested_help && pos.is_empty() {
            crate::log_err!("No file provided.\n");
            return Err(usage);
        }
        if !requested_help && !positionals.at_most("get", 2, "a file and at most one path") {
            return Err(usage);
        }
        let file_path = if !pos.is_empty() { pos[0].clone() } else { "-".to_string() };

        let mut path: Option<Vec<PathSeg>> = None;
        if !requested_help && pos.len() > 1 {
            path = Some(path_arg(&pos[1])?);
        }

        let detected_input =
            if !requested_help && input_override.is_none() { detect_language_from_file_ending(&file_path) } else { None };
        // No `--input` and an unrecognized extension ⇒ sniff the contents in the
        // handler. `json` here is a placeholder `from`/`to`, overwritten once the
        // real format is known.
        let needs_detect = !requested_help && input_override.is_none() && detected_input.is_none();
        let input_format = input_override.or(detected_input.map(|d| d.format)).unwrap_or_else(json_format);

        let mut gron_projection = gron::Projection::gron();
        if let Some(r) = gron_root {
            gron_projection.root_name = r.into_bytes();
        }
        if let Some(s) = gron_sep {
            gron_projection.assign = s.into_bytes();
        }
        if let Some(t) = gron_term {
            gron_projection.terminator = t.into_bytes();
        }

        config.options = CliActionOptions::Get(GetOptions {
            file: file_path,
            path,
            from: input_format,
            to: output_override.unwrap_or(input_format),
            requested_help,
            detect: needs_detect,
            output_explicit: output_override.is_some(),
            lax_tags,
            lossless,
            serialize,
            quiet,
            strict,
            gron_projection,
        });
    } else if a == "check" || a == "ck" {
        config.action = CliAction::Check;

        let mut opts = CheckOptions::default();
        let mut files = Positionals::default();

        while let Some(arg) = args.next() {
            if files.rest(&arg) {
                continue;
            }
            if is_help(&arg) {
                opts.requested_help = true;
            } else if is_quiet(&arg) {
                opts.quiet = true;
            } else if arg == "--strict" {
                opts.strict = true;
            } else if arg == "--input" || arg == "-i" {
                opts.format = Some(format_after(&mut args, &arg, ArgError::MissingCheckArgument)?);
            } else if arg == "--spec" || arg == "-s" {
                opts.spec = Some(value_after(&mut args, &arg, "version value", ArgError::MissingCheckArgument)?);
            } else if !files.add("check", &arg) {
                return Err(ArgError::MissingCheckArgument);
            }
        }

        if !opts.requested_help && files.items.is_empty() {
            crate::log_err!("No file provided.\n");
            return Err(ArgError::MissingCheckArgument);
        }
        opts.files = files.items;
        config.options = CliActionOptions::Check(opts);
    } else if a == "fmt" || a == "f" {
        config.action = CliAction::Fmt;
        let usage = ArgError::MissingFmtArgument;

        let mut input_override: Option<Format> = None;
        let mut quiet = false;
        let mut strict = false;
        let mut dry_run = false;
        let mut diff_mode = false;
        let mut requested_help = false;
        let mut serialize = SerializeOptions::default();
        let mut positionals = Positionals::default();

        while let Some(arg) = args.next() {
            if positionals.rest(&arg) {
                continue;
            }
            if is_help(&arg) {
                requested_help = true;
            } else if arg == "--dry-run" {
                dry_run = true;
            } else if arg == "--diff" {
                diff_mode = true;
            } else if serialize_flag(&mut args, &arg, &mut serialize, usage)? {
                // taken
            } else if is_quiet(&arg) {
                quiet = true;
            } else if arg == "--strict" {
                strict = true;
            } else if arg == "--input" || arg == "-i" {
                input_override = Some(format_after(&mut args, &arg, usage)?);
            } else if !positionals.add("fmt", &arg) {
                return Err(usage);
            }
        }

        if !requested_help && positionals.items.is_empty() {
            crate::log_err!("No file provided.\n");
            return Err(usage);
        }
        // `fmt` reformats a whole file — there is no
        // sub-document path argument the way `get`/`replace`/etc. take one.
        if !requested_help && positionals.items.len() > 1 {
            crate::log_err!("fmt takes a single file, not a path within it: {}\n", positionals.items[1]);
            return Err(usage);
        }
        if !requested_help && dry_run && diff_mode {
            crate::log_err!("--dry-run and --diff are mutually exclusive.\n");
            return Err(usage);
        }

        let file_path = positionals.items.first().cloned().unwrap_or_else(|| "-".to_string());

        let detected_input =
            if !requested_help && input_override.is_none() { detect_language_from_file_ending(&file_path) } else { None };
        let needs_detect = !requested_help && input_override.is_none() && detected_input.is_none();
        let from = input_override.or(detected_input.map(|d| d.format)).unwrap_or_else(json_format);

        config.options = CliActionOptions::Fmt(FmtOptions {
            file: file_path,
            from,
            requested_help,
            detect: needs_detect,
            serialize,
            quiet,
            strict,
            dry_run,
            diff: diff_mode,
        });
    } else if a == "convert" || a == "cv" {
        config.action = CliAction::Convert;
        let usage = ArgError::MissingConvertArgument;

        let mut input_override: Option<Format> = None;
        let mut output_override: Option<Format> = None;
        let mut lax_tags = false;
        let mut lossless = false;
        let mut quiet = false;
        let mut strict = false;
        let mut write = false;
        let mut diff_mode = false;
        let mut requested_help = false;
        let mut serialize = SerializeOptions::default();
        let mut positionals = Positionals::default();

        while let Some(arg) = args.next() {
            if positionals.rest(&arg) {
                continue;
            }
            if is_help(&arg) {
                requested_help = true;
            } else if arg == "--write" || arg == "-w" {
                write = true;
            } else if arg == "--dry-run" {
                crate::log_err!("convert prints to stdout by default; pass --write/-w to write in place instead.\n");
                return Err(usage);
            } else if arg == "--diff" {
                diff_mode = true;
            } else if serialize_flag(&mut args, &arg, &mut serialize, usage)? {
                // taken
            } else if arg == "--lax-tags" {
                lax_tags = true;
            } else if arg == "--lossless" {
                lossless = true;
            } else if arg == "--lossy" {
                lossless = false;
            } else if is_quiet(&arg) {
                quiet = true;
            } else if arg == "--strict" {
                strict = true;
            } else if arg == "--input" || arg == "-i" {
                input_override = Some(format_after(&mut args, &arg, usage)?);
            } else if arg == "--output" || arg == "-o" {
                output_override = Some(format_after(&mut args, &arg, usage)?);
            } else if !positionals.add("convert", &arg) {
                return Err(usage);
            }
        }

        if !requested_help && positionals.items.is_empty() {
            crate::log_err!("No file provided.\n");
            return Err(usage);
        }
        if !requested_help && positionals.items.len() > 1 {
            crate::log_err!("convert takes a single file, not a path within it: {}\n", positionals.items[1]);
            return Err(usage);
        }
        if !requested_help && output_override.is_none() {
            crate::log_err!("convert needs a target: pass --output <format>.\n");
            return Err(usage);
        }

        let file_path = positionals.items.first().cloned().unwrap_or_else(|| "-".to_string());

        let detected_input = if !requested_help { detect_language_from_file_ending(&file_path) } else { None };

        let needs_detect = !requested_help && input_override.is_none() && detected_input.is_none();
        let from = input_override.or(detected_input.map(|d| d.format)).unwrap_or_else(json_format);
        config.options = CliActionOptions::Convert(ConvertOptions {
            file: file_path,
            requested_help,
            from,
            to: output_override.unwrap_or_else(json_format),
            detect: needs_detect,
            lax_tags,
            lossless,
            serialize,
            quiet,
            strict,
            write,
            diff: diff_mode,
        });
    } else if a == "patch" || a == "p" {
        config.action = CliAction::Patch;
        let usage = ArgError::MissingPatchArgument;

        let mut input_override: Option<Format> = None;
        let mut patch_input_override: Option<Format> = None;
        let mut at: Vec<PathSeg> = Vec::new();
        let mut from: Vec<PathSeg> = Vec::new();
        let mut deletes: Vec<Vec<PathSeg>> = Vec::new();
        let mut patch_options = patch::Options::default();
        let mut lossless = false;
        let mut dry_run = false;
        let mut diff_mode = false;
        let mut quiet = false;
        let mut strict = false;
        let mut strip_comments = false;
        let mut comments_given = false;
        let mut requested_help = false;
        let mut positionals = Positionals::default();

        while let Some(arg) = args.next() {
            if positionals.rest(&arg) {
                continue;
            }
            if is_help(&arg) {
                requested_help = true;
            } else if arg == "--dry-run" {
                dry_run = true;
            } else if arg == "--diff" {
                diff_mode = true;
            } else if is_quiet(&arg) {
                quiet = true;
            } else if arg == "--strict" {
                strict = true;
            } else if arg == "--strip-comments" {
                strip_comments = true;
            } else if arg == "--lossless" {
                lossless = true;
            } else if arg == "--lossy" {
                lossless = false;
            } else if arg == "--compact" {
                patch_options.serialize.pretty = false;
            } else if arg == "--pretty" {
                patch_options.serialize.pretty = true;
            } else if arg == "--at" {
                let p = value_after(&mut args, &arg, "path", usage)?;
                at = path_arg(&p)?;
            } else if arg == "--from" {
                let p = value_after(&mut args, &arg, "path", usage)?;
                from = path_arg(&p)?;
            } else if arg == "--delete" {
                let p = value_after(&mut args, &arg, "path", usage)?;
                let parsed_path = path_arg(&p)?;
                if parsed_path.is_empty() {
                    crate::log_err!("--delete needs a path within the document; the root cannot be deleted.\n");
                    return Err(usage);
                }
                deletes.push(parsed_path);
            } else if arg == "--seq" {
                let Some(name) = args.next() else {
                    crate::log_err!("Missing strategy after {arg} (replace, append, union)\n");
                    return Err(usage);
                };
                // `union` is the word a user would reach for; the Zig enum
                // member is `unite`, which it also takes.
                patch_options.seq = if name == "union" {
                    SeqStrategy::Unite
                } else {
                    match SeqStrategy::from_name(&name) {
                        Some(s) => s,
                        None => {
                            crate::log_err!("Unknown --seq strategy: {name} (replace, append, union)\n");
                            return Err(usage);
                        }
                    }
                };
            } else if arg == "--comments" {
                let Some(name) = args.next() else {
                    crate::log_err!("Missing strategy after {arg} (ours, theirs, none)\n");
                    return Err(usage);
                };
                patch_options.comments = match CommentStrategy::from_name(&name) {
                    Some(c) => c,
                    None => {
                        crate::log_err!("Unknown --comments strategy: {name} (ours, theirs, none)\n");
                        return Err(usage);
                    }
                };
                comments_given = true;
            } else if arg == "--indent" || arg == "--width" {
                serialize_flag(&mut args, &arg, &mut patch_options.serialize, usage)?;
            } else if arg == "--input" || arg == "-i" {
                input_override = Some(format_after(&mut args, &arg, usage)?);
            } else if arg == "--patch-input" {
                patch_input_override = Some(format_after(&mut args, &arg, usage)?);
            } else if !positionals.add("patch", &arg) {
                return Err(usage);
            }
        }

        // `--strip-comments` is `get`/`convert`'s word for what `patch` calls
        // `--comments none`: no comment from the patch reaches the target.
        // (The target's own are never stripped — they are what patch keeps.)
        if strip_comments {
            if comments_given && patch_options.comments != CommentStrategy::None {
                crate::log_err!(
                    "--strip-comments carries no comment from the patch; --comments {} asks for some. Pass one.\n",
                    patch_options.comments.name()
                );
                return Err(usage);
            }
            patch_options.comments = CommentStrategy::None;
        }
        if !requested_help && positionals.items.len() < 2 {
            crate::log_err!("patch takes two files: the document to change, then the one supplying the change.\n");
            return Err(usage);
        }
        if !requested_help && positionals.items.len() > 2 {
            crate::log_err!("patch takes two files; use --at/--from to name a path within one: {}\n", positionals.items[2]);
            return Err(usage);
        }
        // Both from a pipe would mean reading one stream twice and getting
        // half of each; the second read comes back empty and the failure is
        // an empty patch, which is silent. Refuse it up front.
        let target_path = positionals.items.first().cloned().unwrap_or_else(|| "-".to_string());
        let source_path = positionals.items.get(1).cloned().unwrap_or_else(|| "-".to_string());
        if !requested_help && target_path == "-" && source_path == "-" {
            crate::log_err!("only one of the two files can be stdin.\n");
            return Err(usage);
        }

        let target_ext = if !requested_help { detect_language_from_file_ending(&target_path) } else { None };
        let source_ext = if !requested_help { detect_language_from_file_ending(&source_path) } else { None };

        config.options = CliActionOptions::Patch(PatchOptions {
            file: target_path,
            patch_file: source_path,
            at,
            from,
            deletes,
            patch_options,
            requested_help,
            format: input_override.or(target_ext.map(|d| d.format)).unwrap_or_else(json_format),
            detect: !requested_help && input_override.is_none() && target_ext.is_none(),
            patch_format: patch_input_override.or(source_ext.map(|d| d.format)).unwrap_or_else(json_format),
            detect_patch: !requested_help && patch_input_override.is_none() && source_ext.is_none(),
            lossless,
            dry_run,
            diff: diff_mode,
            quiet,
            strict,
        });
    } else if a == "lang" {
        config.action = CliAction::Lang;
        let mut opts = LangOptions::default();
        let mut positionals = Positionals::default();
        while let Some(arg) = args.next() {
            if positionals.rest(&arg) {
                continue;
            }
            if is_help(&arg) {
                opts.requested_help = true;
            } else if arg == "--against" {
                let Some(v) = args.next() else {
                    crate::log_err!("Missing format after --against\n");
                    return Err(ArgError::UnsupportedFileFormat);
                };
                opts.against = Some(v);
            } else if arg == "--input" || arg == "-i" {
                let Some(name) = args.next() else {
                    crate::log_err!("Missing format after --input\n");
                    return Err(ArgError::UnsupportedFileFormat);
                };
                opts.input = match parse_format_name(&name) {
                    Some(f) => Some(f),
                    None => {
                        crate::log_err!("Unsupported input format: {name}\n");
                        return Err(ArgError::UnsupportedFileFormat);
                    }
                };
            } else if arg == "--spec" || arg == "-s" {
                opts.spec = Some(value_after(&mut args, &arg, "version value", ArgError::MissingLangArgument)?);
            } else if !positionals.add("lang", &arg) {
                return Err(ArgError::MissingLangArgument);
            }
        }
        let pos = &positionals.items;
        if pos.is_empty() || pos[0] == "list" {
            opts.verb = LangVerb::List;
            if !opts.requested_help && !positionals.at_most("lang list", 1, "no arguments") {
                return Err(ArgError::MissingLangArgument);
            }
        } else if pos[0] == "check" {
            opts.verb = LangVerb::Check;
            if pos.len() >= 2 {
                opts.name = pos[1].clone();
                opts.files = pos[2..].to_vec();
            } else if !opts.requested_help {
                crate::log_err!("lang check needs a language name (e.g. `fig lang check lua-dotenv --against dotenv`).\n");
                return Err(ArgError::MissingLangArgument);
            }
        } else if pos[0] == "table" {
            opts.verb = LangVerb::Table;
            if pos.len() >= 2 {
                opts.name = pos[1].clone();
                if !opts.requested_help && !positionals.at_most("lang table", 2, "one file") {
                    return Err(ArgError::MissingLangArgument);
                }
            } else if !opts.requested_help {
                crate::log_err!("lang table needs a file (e.g. `fig lang table secrets.env`).\n");
                return Err(ArgError::MissingLangArgument);
            }
        } else {
            crate::log_err!("Unknown lang verb: {} (list, check, table)\n", pos[0]);
            return Err(ArgError::MissingLangArgument);
        }
        config.options = CliActionOptions::Lang(opts);
    } else if let Some(name) = external_command_name(a) {
        // Git's fallback, and the reason fig-schema can grow a CLI without
        // fig growing a `schema` action: a word fig has no verb for is handed
        // to a `fig-<word>` executable. Everything after the word is passed
        // through untouched — see `ExternalOptions.argv` — so this branch
        // consumes the rest of the iterator and parses none of it.
        config.action = CliAction::External;
        let program = format!("fig-{name}");
        let mut argv: Vec<std::ffi::OsString> = vec![program.clone().into()];
        while let Some(arg) = args.next() {
            argv.push(arg.into());
        }
        config.options = CliActionOptions::External(ExternalOptions { name: name.to_string(), program: Some(program), argv });
    } else {
        // Not one of fig's verbs, and not a word that could name a program
        // either (`external_command_name` says which shapes those are). Still
        // an `External`: a `None` `program` means the handoff was never on the
        // table, so one place gets to say "no such action" for both halves of
        // the unrecognized case rather than two places saying it differently.
        config.action = CliAction::External;
        config.options = CliActionOptions::External(ExternalOptions { name: a.to_string(), program: None, argv: Vec::new() });
    }

    Ok(config)
}

/// The `<name>` for a `fig-<name>` handoff, or `None` if this word can't be one
/// — in which case it stays an ordinary unrecognized action.
///
/// The rule is deliberately narrower than "whatever `execvp` would take".
/// Restricting it to ASCII letters, digits, `-` and `_` keeps two shapes out:
/// a leading `-`, so a mistyped flag (`fig --colour`) reports itself as a flag
/// instead of hunting for `fig---colour`; and anything containing a path
/// separator or a `.`, so a forgotten verb (`fig config.toml`, `fig ../x`)
/// can't reach the filesystem as a program name.
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

    fn parse(items: &[&str]) -> Result<CliConfig, ArgError> {
        parse_config(items.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parse_path_reads_the_append_sentinel_and_literal_indices() {
        let dash = parse_path(b"list[-]").unwrap();
        assert_eq!(dash, [PathSeg::Key(b"list".to_vec()), PathSeg::Index(APPEND_INDEX)]);
        assert_eq!(parse_path(b"list[$]").unwrap()[1], PathSeg::Index(APPEND_INDEX));
        assert_eq!(parse_path(b"a.b[2]").unwrap()[2], PathSeg::Index(2));
    }

    #[test]
    fn parse_path_takes_a_key_with_a_dot_or_bracket_quoted_bracketed_or_escaped() {
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
        let c = parse(&["fig", "insert", "f.yaml", "a.b", "1"]).unwrap();
        assert_eq!(c.action, CliAction::Insert);
        let c = parse(&["fig", "delete", "f.yaml", "a[-]"]).unwrap();
        let CliActionOptions::Delete(d) = c.options else { panic!() };
        assert_eq!(d.path.last(), Some(&PathSeg::Index(APPEND_INDEX)));
    }

    #[test]
    fn parse_config_hands_an_unknown_action_to_fig_action() {
        let c = parse(&["fig", "schema", "lint", "--input", "x"]).unwrap();
        let CliActionOptions::External(e) = c.options else { panic!() };
        assert_eq!(e.program.as_deref(), Some("fig-schema"));
        assert_eq!(e.argv.len(), 4);
        let c = parse(&["fig", "config.toml"]).unwrap();
        let CliActionOptions::External(e) = c.options else { panic!() };
        assert!(e.program.is_none());
    }

    #[test]
    fn external_command_name_accepts_only_words_that_could_name_a_program() {
        assert_eq!(external_command_name("schema"), Some("schema"));
        assert_eq!(external_command_name("a_b-1"), Some("a_b-1"));
        assert_eq!(external_command_name("--colour"), None);
        assert_eq!(external_command_name("x.toml"), None);
        assert_eq!(external_command_name("../x"), None);
        assert_eq!(external_command_name(""), None);
    }

    #[test]
    fn md_names_no_language() {
        assert!(detect_language_from_file_ending("x.md").is_none());
        assert!(detect_language_from_file_ending("x.markdown").is_none());
    }

    #[test]
    fn yml_is_an_accepted_spelling_of_yaml() {
        assert_eq!(parse_format_name("yml"), parse_format_name("yaml"));
        assert_eq!(detect_language_from_file_ending("a.yml").map(|d| d.format), parse_format_name("yaml"));
    }
}
