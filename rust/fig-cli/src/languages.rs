//! Port of src/cli/languages.zig.
//!
//! Languages the CLI did not compile in: the `languages.figl` configuration
//! that names them, the helper runner that speaks to each as a child
//! process, and the registration that makes one a `Format` every action
//! accepts. `docs/proposals/runtime-languages.md` §7.1.
//!
//! A configured language is a name, the extensions it owns, and a command.
//! Nothing is spawned until a name or extension the CLI cannot resolve
//! itself is asked for: `parse_format_name` and `detect_language_from_file_ending`
//! (in `args.rs`) fall through to `resolve_name` and `resolve_extension` here,
//! which read the configuration once and spawn only the helper that answers.
//! `fig lang list` and `fig lang check` are the two actions that spawn on
//! purpose. A format this build does not bundle is not something the CLI can
//! resolve itself either: its name and extension reach the configured
//! language of that name.
//!
//! The helper runner is a `wire::Process` — the protocol
//! `docs/helper-protocol.md` specifies, JSON-RPC 2.0, one request line to
//! one response line, `describe` once at load, then `parse`, `print` and
//! `render` per call — whose lines cross a child process's stdin and
//! stdout. `wire::describe` builds the `Language` over it, and it registers
//! through the same registry a host's own language does; there is no second
//! way in. The child's stderr stays connected to the terminal, so a helper
//! that wants to say why it refused something can.
//!
//! Configuration is fig format, found in this order and merged with the
//! earlier file winning a name: `$FIG_LANGUAGES` (a file path — the way a
//! test points at one); `.fig/languages.figl` in the working directory and
//! each ancestor; `$XDG_CONFIG_HOME/fig/languages.figl`, which is
//! `~/.config/fig/languages.figl` when the variable is unset.
//!
//! ```fig
//! language[]
//! > name = lua-dotenv
//! > extensions = [tkv]
//! > command = [fig-lua, ~/.config/fig/languages/dotenv.lua]
//! ```

use std::cell::RefCell;
use std::sync::Arc;

use fig_core::kit::ast::{Node, NodeKind, PathSegment, AST};
use fig_core::kit::convert;
use fig_core::kit::document::Document;
use fig_core::kit::manifest::Caps;
use fig_core::kit::span::Span;
use fig_core::kit::table;
use fig_core::kit::language::Language;
use fig_core::registry::{self, Entry, RegisterOptions, LIST_ROWS};
use fig_core::wire;
use fig_core::{err, Result};

use crate::fileio::{self, OpenMode, Terminal};
use crate::types::{self, Format};
use crate::{args, host_languages, parse_dispatch};

/// One configured language, as `languages.figl` states it.
#[derive(Debug, Clone)]
pub struct Configured {
    pub name: String,
    pub extensions: Vec<String>,
    pub command: Vec<String>,
    /// Which file it came from, for `fig lang list` and for a message.
    pub source: String,
    /// Set once the helper has been spawned and registered.
    pub entry: Option<Arc<Entry>>,
    /// Why registration failed, when it did, so a second ask does not
    /// spawn again.
    pub failure: Option<String>,
}

/// The process-wide state: read by the two resolvers. One CLI invocation,
/// one configuration.
#[derive(Default)]
struct State {
    loaded: bool,
    configured: Vec<Configured>,
    /// The `--lang` selection, resolved, so every action's `--input`
    /// resolution can read it without each parser threading the flag.
    lang_override: Option<Format>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Called once from `main` before `parse_config`, which is where names and
/// extensions are resolved: the languages this binary carries, registered
/// before anything looks one up (Zig's `ensureBundled`). Once per process;
/// a refusal stops at the language refused, as Zig's `registerAll(...)
/// catch {}` does.
pub fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = registry::register_bundle(registry::global(), &BUNDLED);
    });
}

/// The languages this binary carries, each from its crate, in `LIST_ROWS`
/// order.
const BUNDLED: [&'static dyn Language; 10] = [
    &fig_json::LANGUAGE,
    &fig_yaml::LANGUAGE,
    &fig_toml::LANGUAGE,
    &fig_zon::LANGUAGE,
    &fig_figl::LANGUAGE,
    &fig_ini::LANGUAGE,
    &fig_dotenv::LANGUAGE,
    &fig_properties::LANGUAGE,
    &fig_plist::LANGUAGE,
    &fig_nestedtext::LANGUAGE,
];

// ── the languages fig ships (Zig's `fig.bundle`) ───────────────────────────

/// Every language fig ships, in the registry order of `LIST_ROWS`, with the
/// dialects and extensions each declares — what Zig's `fig.bundle` knows at
/// compile time of the languages a build leaves out.
const SHIPPED: [(&str, &[&str], &[&str]); 10] = [
    ("json", &["json", "jsonc", "json5"], &["json", "jsonc", "json5"]),
    ("yaml", &["yaml"], &["yaml", "yml"]),
    ("toml", &["toml"], &["toml"]),
    ("zon", &["zon"], &["zon"]),
    ("figl", &["figl"], &["figl"]),
    ("ini", &["ini"], &["ini"]),
    ("dotenv", &["dotenv"], &["env"]),
    ("properties", &["properties"], &["properties"]),
    ("plist", &["plist"], &["plist"]),
    ("nestedtext", &["nestedtext"], &["nt"]),
];

/// Whether this build bundles the language `name` (Zig's comptime
/// `bundles`): the global registry has it, registered with the bundle.
fn bundles(name: &str) -> bool {
    debug_assert!(registry::list_has(name));
    registry::global().entry_by_name(name).is_some_and(|e| e.language.first_index == e.index as usize)
}

/// Every dialect fig ships, bundled or not (`bundle.all_dialects`).
pub fn all_dialects() -> Vec<&'static str> {
    debug_assert_eq!(SHIPPED.len(), LIST_ROWS.len());
    SHIPPED.iter().flat_map(|r| r.1.iter().copied()).collect()
}

/// `all_dialects`, less the languages this build does not bundle
/// (`bundle.dialect_names`).
pub fn dialect_names() -> Vec<String> {
    SHIPPED.iter().filter(|r| bundles(r.0)).flat_map(|r| r.1.iter().map(|d| d.to_string())).collect()
}

/// The language fig ships that owns extension `ext` but this build does not
/// bundle, or `None`: what a consumer names when such a file meets no
/// language, rather than reading it as something else.
pub fn unbundled_owner(ext: &str) -> Option<&'static str> {
    SHIPPED.iter().find(|r| !bundles(r.0) && r.2.contains(&ext)).map(|r| r.0)
}

// ── configuration ──────────────────────────────────────────────────────────

/// Read every `languages.figl` in the search order once. Errors in one file
/// are reported and the file skipped; a missing file is not an error.
pub fn load() {
    let already = STATE.with(|s| std::mem::replace(&mut s.borrow_mut().loaded, true));
    if already {
        return;
    }
    // A WASI module cannot spawn the helper a block names, so there is
    // nothing to read the configuration for.
    if cfg!(target_os = "wasi") {
        return;
    }

    if let Some(path) = std::env::var_os("FIG_LANGUAGES") {
        load_file(&path.to_string_lossy());
    }

    // `.fig/languages.figl` from the working directory upward.
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir: Option<&std::path::Path> = Some(&cwd);
        while let Some(d) = dir {
            let candidate = format!("{}/.fig/languages.figl", d.display());
            load_file(&candidate);
            dir = d.parent();
        }
    }

    let user_path = if let Some(h) = std::env::var_os("XDG_CONFIG_HOME") {
        format!("{}/fig/languages.figl", h.to_string_lossy())
    } else if let Some(home) = std::env::var_os("HOME") {
        format!("{}/.config/fig/languages.figl", home.to_string_lossy())
    } else {
        return;
    };
    load_file(&user_path);
}

fn load_file(path: &str) {
    let content = match std::fs::read(path) {
        Ok(c) => c,
        Err(e) => {
            let name = fileio::io_error(&e);
            if name.is("FileNotFound") || name.is("NotDir") {
                return;
            }
            crate::log_warn!("could not read {path}: {}", name.name());
            return;
        }
    };
    if content.len() > 1 << 20 {
        crate::log_warn!("could not read {path}: StreamTooLong");
        return;
    }
    if let Err(e) = parse_config_file(path, &content) {
        crate::log_warn!("could not parse {path}: {}", e.name());
    }
}

/// One `language[]` block per configured language. Read with fig's own
/// reader; a build without the fig format has no way to read the file and
/// says so.
fn parse_config_file(path: &str, content: &[u8]) -> Result<()> {
    let Some(fig) = registry::global().entry_by_name("figl") else {
        crate::log_warn!("{path}: this build has no fig-format reader, so the file cannot be read");
        return Ok(());
    };
    let doc = fig.parse(content)?;
    let ast = &doc.ast;
    let seq = ast.get_val_by_path(&[PathSegment::Key(b"language")]).map_err(|_| err!(NoLanguageList))?;
    let NodeKind::Sequence(mut next) = seq.kind else { return Err(err!(NoLanguageList)) };
    while let Some(id) = next {
        let item = ast.node(id);
        next = item.next_sibling;
        if !matches!(item.kind, NodeKind::Mapping(_)) {
            return Err(err!(MalformedLanguage));
        }
        let name = string_field(ast, &item, b"name").ok_or(err!(MalformedLanguage))?;
        let command = string_list(ast, &item, b"command")?.ok_or(err!(MalformedLanguage))?;
        if command.is_empty() {
            return Err(err!(MalformedLanguage));
        }
        let extensions = string_list(ast, &item, b"extensions")?.unwrap_or_default();
        if find_configured(&name).is_some() {
            continue; // the earlier file wins
        }
        STATE.with(|s| {
            s.borrow_mut().configured.push(Configured {
                name,
                extensions,
                command,
                source: path.to_string(),
                entry: None,
                failure: None,
            })
        });
    }
    Ok(())
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn string_field(ast: &AST, mapping: &Node<'_>, key: &[u8]) -> Option<String> {
    let NodeKind::Mapping(mut next) = mapping.kind else { return None };
    while let Some(id) = next {
        let kv_node = ast.node(id);
        next = kv_node.next_sibling;
        let kv = kv_node.kind.as_keyvalue()?;
        if let NodeKind::String(k) = ast.node(kv.key).kind {
            if k == key {
                return match ast.node(kv.value).kind {
                    NodeKind::String(s) => Some(lossy(s)),
                    NodeKind::Number(n) => Some(lossy(n.raw)),
                    _ => None,
                };
            }
        }
    }
    None
}

fn string_list(ast: &AST, mapping: &Node<'_>, key: &[u8]) -> Result<Option<Vec<String>>> {
    let NodeKind::Mapping(mut next) = mapping.kind else { return Ok(None) };
    while let Some(id) = next {
        let kv_node = ast.node(id);
        next = kv_node.next_sibling;
        let Some(kv) = kv_node.kind.as_keyvalue() else { continue };
        if let NodeKind::String(k) = ast.node(kv.key).kind {
            if k == key {
                let mut out = Vec::new();
                match ast.node(kv.value).kind {
                    NodeKind::String(s) => out.push(lossy(s)),
                    NodeKind::Sequence(first) => {
                        let mut item = first;
                        while let Some(iid) = item {
                            let n = ast.node(iid);
                            item = n.next_sibling;
                            match n.kind {
                                NodeKind::String(s) => out.push(lossy(s)),
                                NodeKind::Number(num) => out.push(lossy(num.raw)),
                                _ => return Err(err!(MalformedLanguage)),
                            }
                        }
                    }
                    _ => return Err(err!(MalformedLanguage)),
                }
                return Ok(Some(out));
            }
        }
    }
    Ok(None)
}

fn find_configured(name: &str) -> Option<usize> {
    STATE.with(|s| s.borrow().configured.iter().position(|c| c.name == name))
}

fn configured_at(i: usize) -> Configured {
    STATE.with(|s| s.borrow().configured[i].clone())
}

fn configured_len() -> usize {
    STATE.with(|s| s.borrow().configured.len())
}

/// Every configured language, loaded. For `fig lang list`.
pub fn configured() -> Vec<Configured> {
    load();
    STATE.with(|s| s.borrow().configured.clone())
}

// ── resolution ─────────────────────────────────────────────────────────────

/// Record `--lang <name>`, resolved: the format every action reads and
/// writes its file in, whatever the extension says. Read by `args.rs`
/// where it would otherwise consult the extension.
pub fn set_lang_override(format: Option<Format>) {
    STATE.with(|s| s.borrow_mut().lang_override = format);
}

pub fn lang_override() -> Option<Format> {
    STATE.with(|s| s.borrow().lang_override)
}

/// Whether some `languages.figl` names `name` at all — for the message when
/// `--lang` cannot be resolved: a name nobody configured is a different
/// mistake from a helper that was refused, which `ensure` has reported. A
/// name that turned out to be a further dialect of a configured language
/// (`resolve_name` found it in the registry) counts as configured.
pub fn is_configured(name: &str) -> bool {
    load();
    find_configured(name).is_some() || registry::global().entry_by_name(name).is_some()
}

/// The `Format` of the configured language named `name`, spawning and
/// registering its helper on first ask; `None` when no configured language
/// has the name.
///
/// A `languages.figl` block names a language, and a language may serve
/// more than one dialect — `lua-json5` and `lua-jsonc` from one helper —
/// whose names only its `describe` knows. A name no block carries is
/// therefore looked for among the dialects of the languages not yet
/// spawned, spawning each in turn until one answers to it. A helper that
/// fails along the way is not reported here — the name asked for is not
/// its — and `fig lang list` shows the refusal.
pub fn resolve_name(name: &str) -> Option<Format> {
    // Already in the registry — a name registered by whatever means.
    if let Some(e) = registry::global().entry_by_name(name) {
        return Some(types::runtime_format(&e));
    }
    // The WASI build's host, which serves languages in-process.
    if let Some(e) = host_languages::by_name(name) {
        return Some(types::runtime_format(&e));
    }
    load();
    if let Some(i) = find_configured(name) {
        return ensure_logged(i);
    }
    for i in 0..configured_len() {
        let c = configured_at(i);
        if c.entry.is_some() || c.failure.is_some() {
            continue;
        }
        let _ = ensure(i);
        if let Some(e) = registry::global().entry_by_name(name) {
            return Some(types::runtime_format(&e));
        }
    }
    None
}

/// The `Format` of the configured language owning `ext`, or `None`.
pub fn resolve_extension(ext: &str) -> Option<Format> {
    if let Some(e) = host_languages::by_extension(ext) {
        return Some(types::runtime_format(&e));
    }
    load();
    for i in 0..configured_len() {
        let c = configured_at(i);
        if c.extensions.iter().any(|x| x == ext) {
            return ensure_logged(i);
        }
    }
    None
}

/// The registered language that owns extension `ext`, or `None`.
pub fn registered_extension(ext: &str) -> Option<Format> {
    registry::global().entry_by_extension(ext).map(|e| types::runtime_format(&e))
}

/// Spawn and register configured language `i` if it has not been, and hand
/// back its `Format`. A failure is remembered in its `failure` and not
/// retried; it is not reported here — `fig lang list` prints it as a line of
/// its own, and the resolvers log it once, on the ask that met it.
pub fn ensure(i: usize) -> Option<Format> {
    let c = configured_at(i);
    if let Some(e) = &c.entry {
        return Some(types::runtime_format(e));
    }
    if c.failure.is_some() {
        return None;
    }
    registry::clear_refusal();
    match register(&c) {
        Ok(e) => {
            let f = types::runtime_format(&e);
            STATE.with(|s| s.borrow_mut().configured[i].entry = Some(e));
            Some(f)
        }
        Err(error) => {
            // Whatever refused it wrote the reason, on either side of the
            // registry; an error that reached here without one is its own name.
            let refusal = registry::last_refusal();
            let reason = if !refusal.is_empty() { lossy(&refusal) } else { error.name().to_string() };
            STATE.with(|s| s.borrow_mut().configured[i].failure = Some(reason));
            None
        }
    }
}

/// `ensure`, logging the failure the first time a resolver meets it.
fn ensure_logged(i: usize) -> Option<Format> {
    let already_failed = configured_at(i).failure.is_some();
    let format = ensure(i);
    if format.is_none() && !already_failed {
        let c = configured_at(i);
        crate::log_err!("language `{}` ({}): {}", c.name, c.source, c.failure.as_deref().unwrap_or("?"));
    }
    format
}

// ── the helper runner ──────────────────────────────────────────────────────

/// Spawn `c.command`, ask it to describe itself, build the language and
/// register it.
fn register(c: &Configured) -> Result<Arc<Entry>> {
    let argv = expand_argv(&c.command);
    let process = wire::Process::spawn(&argv)?;
    // `describe` — what the language's description is read from.
    let lang = wire::describe(Box::new(process))?;
    if lang.name() != c.name {
        registry::refuse(&format!("the helper describes itself as `{}`, but languages.figl calls it `{}`", lang.name(), c.name));
        return Err(err!(HelperNameMismatch));
    }
    let abi = wire::register(registry::global(), lang, RegisterOptions::default())?;
    registry::global().entry_by_abi(abi).ok_or(err!(InvalidLanguage))
}

/// `command` with a leading `~` in any argument expanded to `$HOME`.
fn expand_argv(command: &[String]) -> Vec<String> {
    let home = std::env::var("HOME").ok();
    expand_argv_with(command, home.as_deref())
}

fn expand_argv_with(command: &[String], home: Option<&str>) -> Vec<String> {
    command
        .iter()
        .map(|arg| match (arg.strip_prefix('~'), home) {
            (Some(rest), Some(h)) => format!("{h}{rest}"),
            _ => arg.clone(),
        })
        .collect()
}

// ── `fig lang` ─────────────────────────────────────────────────────────────

/// `fig lang list`: every format fig ships and every configured language,
/// with what each can do. A configured language is spawned to be asked.
pub fn list(out: &mut Terminal) -> Result<()> {
    out.write_str(if host_languages::ENABLED { "formats:\n" } else { "built in:\n" })?;
    for n in all_dialects() {
        if let Some(e) = host_languages::by_name(n) {
            out.print(format_args!("  {:<14} {}  (a JavaScript language, served by the host)\n", n, caps_word(e.language.caps)))?;
        } else if let Some(e) = registry::global().entry_by_name(n) {
            out.print(format_args!("  {:<14} {}\n", n, caps_word(e.language.caps)))?;
        } else {
            out.print(format_args!("  {n:<14} not built in\n"))?;
        }
    }
    // A WASI build spawns nothing (`load`).
    if cfg!(target_os = "wasi") {
        return out.flush();
    }
    let langs = configured();
    if langs.is_empty() {
        out.write_str("configured: none (no languages.figl found)\n")?;
    } else {
        out.write_str("configured:\n")?;
        for i in 0..langs.len() {
            if ensure(i).is_some() {
                let c = configured_at(i);
                let e = c.entry.as_ref().unwrap();
                out.print(format_args!("  {:<14} {}", c.name, caps_word(e.language.caps)))?;
                if !c.extensions.is_empty() {
                    out.write_str("  .")?;
                    for (j, x) in c.extensions.iter().enumerate() {
                        if j > 0 {
                            out.write_str(" .")?;
                        }
                        out.write_str(x)?;
                    }
                }
                out.print(format_args!("  ({})\n", c.source))?;
            } else {
                let c = configured_at(i);
                out.print(format_args!("  {:<14} refused: {}  ({})\n", c.name, c.failure.as_deref().unwrap_or("?"), c.source))?;
            }
        }
    }
    out.flush()
}

fn caps_word(caps: Caps) -> &'static str {
    if caps.read && caps.edit && caps.serialize {
        return "read edit serialize";
    }
    if caps.read && caps.serialize {
        return "read serialize";
    }
    if caps.read && caps.edit {
        return "read edit";
    }
    if caps.read {
        return "read";
    }
    "none"
}

/// `fig lang check <name> [--against <compiled>] [files…]`: load the
/// language — which is the harness over its samples — and then, given a
/// compiled format to hold it to, parse each file (and each of the
/// language's samples) with both and compare the tables row for row.
/// Exits nonzero on the first difference.
pub fn check(out: &mut Terminal, err_term: &mut Terminal, name: &str, against: Option<&str>, files: &[String]) -> Result<()> {
    // Resolve without the resolver's log line: a refusal is `check`'s own
    // finding, and it reports it as one.
    load();
    let format = if let Some(e) = registry::global().entry_by_name(name) {
        types::runtime_format(&e)
    } else if let Some(i) = find_configured(name) {
        match ensure(i) {
            Some(f) => f,
            None => {
                out.print(format_args!("{name}: refused: {}\n", configured_at(i).failure.unwrap_or_default()))?;
                out.flush()?;
                fileio::exit(1);
            }
        }
    } else {
        match resolve_name(name) {
            Some(f) => f,
            None => {
                // Not a block's name, and not a further dialect of any language
                // that could be spawned.
                err_term.print(format_args!("error: no language named `{name}` is configured (see `fig lang --help`)\n"))?;
                err_term.flush()?;
                fileio::exit(2);
            }
        }
    };
    let e = types::runtime_entry(format).expect("a registered format");
    out.print(format_args!("{}: registered ({}); every sample parsed", e.name, caps_word(e.language.caps)))?;
    if e.language.caps.serialize {
        out.write_str(", printed and reparsed to the same tree")?;
    }
    if e.language.caps.edit {
        out.write_str(", and took a no-op edit")?;
    }
    out.write_str("\n")?;

    if let Some(sibling_name) = against {
        let Some(sibling) = types::named(sibling_name) else {
            err_term.print(format_args!("error: `--against` names a format; `{sibling_name}` is not one\n"))?;
            err_term.flush()?;
            fileio::exit(2);
        };
        let mut inputs: Vec<(String, Vec<u8>)> = Vec::new();
        for (i, s) in e.language.samples.iter().enumerate() {
            inputs.push((format!("sample {}", i + 1), s.to_vec()));
        }
        for f in files {
            match std::fs::read(f) {
                Ok(content) => inputs.push((f.clone(), content)),
                Err(rerr) => {
                    err_term.print(format_args!("error: could not read {f}: {}\n", fileio::io_error(&rerr).name()))?;
                    err_term.flush()?;
                    fileio::exit(1);
                }
            }
        }
        for (label, content) in &inputs {
            let mut reports = parse_dispatch::Reports::default();
            let mine = match parse_dispatch::parse_slice_as(format, content, &mut reports) {
                Ok(d) => d,
                Err(perr) => {
                    err_term.print(format_args!("{label}: `{}` does not parse it: ", e.name))?;
                    match &reports.diag {
                        Some(d) => err_term.write_all(&d.message)?,
                        None => err_term.write_str(perr.name())?,
                    }
                    err_term.write_str("\n")?;
                    err_term.flush()?;
                    fileio::exit(1);
                }
            };
            let mut sib_reports = parse_dispatch::Reports::default();
            let theirs = match parse_dispatch::parse_slice_as(sibling, content, &mut sib_reports) {
                Ok(d) => d,
                Err(perr) => {
                    err_term.print(format_args!("{label}: `{sibling_name}` does not parse it: {}\n", perr.name()))?;
                    err_term.flush()?;
                    fileio::exit(1);
                }
            };
            if diff_documents(err_term, label, &e.name, sibling_name, &mine, &theirs)? {
                err_term.flush()?;
                fileio::exit(1);
            }
            out.print(format_args!("{label}: same table as `{sibling_name}` ({} rows)\n", mine.ast.node_count()))?;
        }
    }
    out.flush()
}

/// `fig lang table <file> [-i <format>] [--spec <version>]`: the file's
/// node table as the JSON a helper answers `parse` with. What an
/// implementor of a twin reads to see what the compiled format produces,
/// and what `check --against` then holds the twin to. `--spec` selects a
/// version of the format as `check`'s does (YAML 1.1's resolution, TOML
/// 1.0's grammar), so a twin of that version has a table to read too.
pub fn print_table(out: &mut Terminal, err_term: &mut Terminal, file: &str, input: Option<Format>, spec_str: Option<&str>) -> Result<()> {
    let mut handle = fileio::get_input(Some(file), OpenMode::ReadOnly)?;
    let content = fileio::read_all(&mut handle)?;
    let format = match input {
        Some(f) => f,
        None => match args::detect_language_from_file_ending(file) {
            Some(d) => d.format,
            None => parse_dispatch::resolve_format_from_content(&content, file)?,
        },
    };
    let dialect = match parse_dispatch::resolve_spec(format, spec_str) {
        Ok(d) => d,
        Err(_) => {
            err_term.print(format_args!("error: --spec {} is not a version of {}\n", spec_str.unwrap_or(""), types::name(format)))?;
            err_term.flush()?;
            return Err(err!(UnsupportedSpec));
        }
    };
    let mut reports = parse_dispatch::Reports::default();
    let doc = match parse_dispatch::parse_slice_as(dialect, &content, &mut reports) {
        Ok(d) => d,
        Err(e) => {
            reports.report_diagnostics(err_term, &content, file)?;
            return Err(e);
        }
    };
    let t = convert::full_table(&doc)?;
    let view = t.view();
    let mut w = Vec::new();
    wire::table_to_json(&mut w, &view);
    out.write_all(&w)?;
    out.write_str("\n")?;
    out.flush()
}

/// Compare two documents of the same source as the tables a helper would
/// produce for them — `convert::full_table` of each, which is exactly what
/// `fig lang table` prints — row for row: kind, parent, text, span,
/// marker, separator, anchor, tag, then the regions, mentions, comments
/// and tag directives. Reports the first difference and returns true on
/// one.
fn diff_documents(term: &mut Terminal, label: &str, mine_name: &str, theirs_name: &str, mine: &Document, theirs: &Document) -> Result<bool> {
    let xt = convert::full_table(mine)?;
    let yt = convert::full_table(theirs)?;
    let x = xt.view();
    let y = yt.view();
    let xr = x.row_slice();
    let yr = y.row_slice();
    if xr.len() != yr.len() {
        term.print(format_args!("{label}: `{mine_name}` produces {} rows, `{theirs_name}` {}\n", xr.len(), yr.len()))?;
        return Ok(true);
    }
    for (i, (p, q)) in xr.iter().zip(yr.iter()).enumerate() {
        if p.kind != q.kind || p.parent != q.parent || p.flags != q.flags {
            term.print(format_args!(
                "{label}: row {i} differs in kind or parent ({mine_name}: kind {} parent {}; {theirs_name}: kind {} parent {})\n",
                p.kind, p.parent, q.kind, q.parent
            ))?;
            return Ok(true);
        }
        let pf = x.facts(i as u32);
        let qf = y.facts(i as u32);
        let texts: [(&str, Option<&[u8]>, Option<&[u8]>); 4] = [
            ("text", x.text_of(p.text), y.text_of(q.text)),
            ("anchor", pf.anchor.and_then(|f| x.text_of(f.name)), qf.anchor.and_then(|f| y.text_of(f.name))),
            ("tag", pf.tag.and_then(|f| x.text_of(f.text)), qf.tag.and_then(|f| y.text_of(f.text))),
            ("item separator", pf.item_sep.and_then(|f| x.text_of(f.text)), qf.item_sep.and_then(|f| y.text_of(f.text))),
        ];
        for (col, a, b) in texts {
            if a != b {
                term.print(format_args!("{label}: row {i} {col} differs ({mine_name}: "))?;
                term.write_all(a.unwrap_or(b"none"))?;
                term.print(format_args!("; {theirs_name}: "))?;
                term.write_all(b.unwrap_or(b"none"))?;
                term.write_str(")\n")?;
                return Ok(true);
            }
        }
        let spans: [(&str, Option<Span>, Option<Span>); 5] = [
            ("span", span_of(p.start, p.end), span_of(q.start, q.end)),
            ("item marker", fact_span(pf.marker), fact_span(qf.marker)),
            ("separator", fact_span(pf.sep), fact_span(qf.sep)),
            ("anchor span", pf.anchor.and_then(|f| span_of(f.start, f.end)), qf.anchor.and_then(|f| span_of(f.start, f.end))),
            ("tag span", pf.tag.and_then(|f| span_of(f.start, f.end)), qf.tag.and_then(|f| span_of(f.start, f.end))),
        ];
        for (col, a, b) in spans {
            if a != b {
                term.print(format_args!("{label}: row {i} {col} differs ({mine_name}: {}; {theirs_name}: {})\n", fmt_span(a), fmt_span(b)))?;
                return Ok(true);
            }
        }
    }
    let (xg, yg) = (x.regions, y.regions);
    if xg.len() != yg.len() {
        term.print(format_args!("{label}: `{mine_name}` records {} header lines, `{theirs_name}` {}\n", xg.len(), yg.len()))?;
        return Ok(true);
    }
    for (i, (r1, r2)) in xg.iter().zip(yg.iter()).enumerate() {
        if r1.node != r2.node || r1.start != r2.start || r1.end != r2.end {
            term.print(format_args!(
                "{label}: header line {i} differs ({mine_name}: row {} [{},{}); {theirs_name}: row {} [{},{}))\n",
                r1.node, r1.start, r1.end, r2.node, r2.start, r2.end
            ))?;
            return Ok(true);
        }
    }
    let (xm, ym) = (x.mentions, y.mentions);
    if xm.len() != ym.len() {
        term.print(format_args!("{label}: `{mine_name}` records {} name mentions, `{theirs_name}` {}\n", xm.len(), ym.len()))?;
        return Ok(true);
    }
    for (i, (m1, m2)) in xm.iter().zip(ym.iter()).enumerate() {
        if m1.node != m2.node || m1.kind != m2.kind || m1.start != m2.start || m1.end != m2.end {
            term.print(format_args!(
                "{label}: name mention {i} differs ({mine_name}: row {} {}; {theirs_name}: row {} {})\n",
                m1.node,
                fmt_span(span_of(m1.start, m1.end)),
                m2.node,
                fmt_span(span_of(m2.start, m2.end))
            ))?;
            return Ok(true);
        }
    }
    let (xc, yc) = (x.comments, y.comments);
    if xc.len() != yc.len() {
        term.print(format_args!("{label}: `{mine_name}` records {} comments, `{theirs_name}` {}\n", xc.len(), yc.len()))?;
        return Ok(true);
    }
    for (i, (c1, c2)) in xc.iter().zip(yc.iter()).enumerate() {
        if c1.node != c2.node || c1.slot != c2.slot || c1.style != c2.style || x.text_of(c1.text) != y.text_of(c2.text) {
            term.print(format_args!("{label}: comment {i} differs ({mine_name}: row {} slot {} `", c1.node, c1.slot))?;
            term.write_all(x.text_of(c1.text).unwrap_or(b""))?;
            term.print(format_args!("`; {theirs_name}: row {} slot {} `", c2.node, c2.slot))?;
            term.write_all(y.text_of(c2.text).unwrap_or(b""))?;
            term.write_str("`)\n")?;
            return Ok(true);
        }
    }
    let (xd, yd) = (x.directives, y.directives);
    if xd.len() != yd.len() {
        term.print(format_args!("{label}: `{mine_name}` records {} tag directives, `{theirs_name}` {}\n", xd.len(), yd.len()))?;
        return Ok(true);
    }
    for (i, (d1, d2)) in xd.iter().zip(yd.iter()).enumerate() {
        if x.text_of(d1.handle) != y.text_of(d2.handle) || x.text_of(d1.prefix) != y.text_of(d2.prefix) {
            term.print(format_args!("{label}: tag directive {i} differs ({mine_name}: `"))?;
            term.write_all(x.text_of(d1.handle).unwrap_or(b""))?;
            term.write_str("` → `")?;
            term.write_all(x.text_of(d1.prefix).unwrap_or(b""))?;
            term.print(format_args!("`; {theirs_name}: `"))?;
            term.write_all(y.text_of(d2.handle).unwrap_or(b""))?;
            term.write_str("` → `")?;
            term.write_all(y.text_of(d2.prefix).unwrap_or(b""))?;
            term.write_str("`)\n")?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// A table span, or `None` for an absent one.
fn span_of(start: u32, end: u32) -> Option<Span> {
    if start == table::NONE {
        return None;
    }
    Some(Span::new(start as usize, end as usize))
}

fn fact_span(f: Option<table::SpanFact>) -> Option<Span> {
    let x = f?;
    span_of(x.start, x.end)
}

/// An optional span as `[s,e)` or `none`, for a difference report.
fn fmt_span(span: Option<Span>) -> String {
    match span {
        Some(s) => format!("[{},{})", s.start, s.end),
        None => "none".to_string(),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_argv_a_leading_tilde_in_any_argument_is_home() {
        crate::languages::init();
        let argv = expand_argv_with(
            &["fig-lua".to_string(), "~/.config/fig/languages/dotenv.lua".to_string(), "not~here".to_string()],
            Some("/home/adam"),
        );
        assert_eq!(argv, ["fig-lua", "/home/adam/.config/fig/languages/dotenv.lua", "not~here"]);
    }

    #[test]
    fn every_shipped_dialect_is_a_list_row_language() {
        crate::languages::init();
        for (name, _, _) in SHIPPED {
            assert!(registry::list_has(name));
        }
    }
}
