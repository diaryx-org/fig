//! Port of src/contract/runtime.zig + src/contract/list.zig (the format registry).
//!
//! Also the logic of `src/contract/bundle.zig` (`register_bundle`). The C ABI
//! plumbing of `runtime.zig` — the `VTable` record and its thunks, `Str`,
//! `CSpan`, `normalize`/`gated` header-size reading, `ErrorInfo` marshalling
//! — is not ported: a language here is a `&'static dyn Language`
//! (`crate::kit::language`), and the registry calls it directly.
//!
//! # Public API
//!
//! The registry is an explicit value, [`Registry`], plus a process-wide one,
//! [`global()`], which registers the bundled languages ([`register_bundle`]
//! over `bundled_languages()`) the first time it is reached, as Zig's
//! `ensureBundled` does. The bundle is fig's ten languages with the `bundle`
//! feature (the default), and none without it, as a Zig build with every
//! `-D<name>` off: what the `fig` binding crate links.
//!
//! ```text
//! // The one list of formats (list.zig)
//! pub struct ListRow { pub name: &'static str, pub help: &'static str, pub default_on: bool }
//! pub const LIST_ROWS: [ListRow; 10];
//! pub fn list_index_of(name: &str) -> Option<usize>;
//! pub fn list_has(name: &str) -> bool;
//!
//! // The registry
//! pub const MAX_ENTRIES: usize = 4096;
//! pub struct Registry;                       // Registry::new()
//! pub fn global() -> &'static Registry;      // with the bundle registered
//! impl Registry {
//!     pub fn register(&self, lang: &'static dyn Language) -> Result<i32>;
//!     pub fn register_with(&self, lang: &'static dyn Language, options: RegisterOptions) -> Result<i32>;
//!     pub fn register_arc(&self, lang: Arc<dyn Language>, options: RegisterOptions) -> Result<i32>;
//!     pub fn entry_at(&self, index: usize) -> Option<Arc<Entry>>;
//!     pub fn entry_by_abi(&self, abi: i32) -> Option<Arc<Entry>>;
//!     pub fn entry_by_name(&self, name: &str) -> Option<Arc<Entry>>;
//!     pub fn registered_by_name(&self, name: &str) -> Option<Arc<Entry>>;
//!     pub fn entry_by_extension(&self, ext: &str) -> Option<Arc<Entry>>;
//!     pub fn detect_among(&self, input: &[u8], order: &[&str]) -> Option<Arc<Entry>>;
//!     pub fn count(&self) -> usize;
//!     pub fn entries(&self) -> Vec<Arc<Entry>>;
//! }
//! pub struct RegisterOptions { pub harness: bool }   // default: harness on
//! // Errors of register*: InvalidLanguage, HarnessFailed, NameTaken, RegistryFull.
//! pub fn refuse(message: &str); pub fn last_refusal() -> Vec<u8>; pub fn clear_refusal();
//!
//! // One registered dialect row, and the registration it belongs to
//! pub struct Entry { index, abi, language: Arc<Registered>, name: String, dialect,
//!                    print_dialect, extensions, splice, empty_doc_seed, syntax, prints }
//! pub struct Registered { lang: &'static dyn Language, name: String, caps: Caps,
//!                         syntax: Option<Syntax>, samples, first_index, entry_count }
//! impl Entry {
//!     pub fn lang(&self) -> &'static dyn Language;
//!     pub fn caps(&self) -> Caps;
//!     pub fn parse(&self, input: &[u8]) -> Result<Document>;      // Parser::new().parse
//!     pub fn syntax_or_default(&self) -> Syntax;                  // Zig `Language.syntax(t)`
//!     pub fn has_renderer(&self, which: Renderer) -> bool;
//!     pub fn render(&self, which: Renderer, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()>;
//!     pub fn split_stream(&self, input: &[u8]) -> Result<Vec<StreamDocument>>;
//! }
//! pub struct Parser { pub last_error: Vec<u8>, pub last_offset: usize,
//!                     pub errors: Vec<ParseDiagnostic>, pub warnings: Vec<ParseDiagnostic> }
//! impl Parser { pub fn new() -> Parser; pub fn parse(&mut self, input: &[u8], e: &Entry) -> Result<Document>;
//!               pub fn last_message(&self) -> &[u8]; }
//! pub fn print_with(e: &Entry, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()>;
//! pub fn print_node_with(e: &Entry, ast: &AST, root: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()>;
//! pub fn validate_language(lang: &dyn Language) -> core::result::Result<(), String>;
//!
//! // The bundle (bundle.zig)
//! pub fn register_bundle(registry: &Registry, languages: &[&'static dyn Language]) -> Result<()>;
//! pub fn register_bundled(registry: &Registry, lang: &'static dyn Language) -> Result<i32>;
//! pub fn all_dialects(languages: &[&'static dyn Language]) -> Vec<&'static str>;
//! ```
//!
//! Behaviour kept from the Zig, which callers see:
//!
//! - **Rows.** A registration appends one `Entry` per dialect row: the
//!   language's declared `dialects()` in order, then each declared row's
//!   `--spec` versions, named `<dialect>-<spec>` (`yaml-1.1`), which claim no
//!   extension. Every extension goes to the row named after it, else to the
//!   first row. An entry's `index` is its position in the registry and its
//!   `abi` that plus one, so 0 is never a format.
//! - **Parsing through an entry** fails with `RuntimeParseFailed` whatever the
//!   parser's own error name, its message in `Parser::last_message` (the first
//!   error diagnostic's message, else the parser's error name, truncated to
//!   255 bytes as the C `ErrorInfo` holds); every diagnostic lands in
//!   `Parser::errors`/`warnings`.
//! - **Printing through an entry** (`print_with`/`print_node_with`) hands the
//!   language the subtree rebuilt from its node table (no spans, comments
//!   dropped when `strip_comments`), with the C `PrintOptions` subset of the
//!   options (`indent_requested` is not carried), and a `--spec` row prints
//!   as the declared dialect it is a version of. A refusal keeps its name
//!   when it is one of the nine the C ABI carries (`NullUnsupported`,
//!   `NonStringKey`, `UnrepresentableRoot`, `UnsupportedValue`, `InvalidKey`,
//!   `NonFiniteNumber`, `UnresolvedAlias`, `NestingTooDeep`,
//!   `UnexpectedNodeKind`), and is `RuntimePrintFailed` otherwise; a language
//!   without the serialize capability is `FormatDisabled`. Nothing is written
//!   to `out` on failure.
//! - **The harness** (on unless `RegisterOptions::harness` is false) parses
//!   every sample in every row, checks its regions, where the language
//!   prints, prints and reparses it to the same tree, and where it edits,
//!   constructs an `Editor` over it and checks that a no-op splice is the
//!   identity. `register_bundle`
//!   runs it only in this crate's own test build, as Zig's bundle does in a
//!   test build, and stops at the first language refused.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::err;
use crate::kit::ast::{NodeId, NodeKind, SerializeOptions, AST};
use crate::kit::convert;
use crate::kit::document::Document;
use crate::kit::language::{self as language, Language, ParseDiagnostic};
use crate::kit::manifest::{
    Caps, Comments, Dialect, Prints, RenderRequest, Renderer, SpliceStyle, StreamDocument, Syntax,
};
use crate::kit::span::Span;
use crate::kit::table::{self, Table};
use crate::Result;

// ============================================================================
// THE ONE LIST OF FORMATS (list.zig)
// ============================================================================

/// One format. `name` is the package (`languages/<name>/`, a dependency of
/// fig's named `fig_<name>`), its module (`fig_<name>`), the `-D<name>`
/// build option and the `build_options.lang_<name>` decl the option becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListRow {
    pub name: &'static str,
    /// The `-D<name>` option's help text.
    pub help: &'static str,
    /// Whether the format is compiled in when the option is not given.
    pub default_on: bool,
}

/// The formats, in registry order. The order is load-bearing: it is the
/// member order of every enum `language.zig` derives from the registry, so a
/// new format is appended, never inserted.
pub const LIST_ROWS: [ListRow; 10] = [
    ListRow { name: "json", help: "Include JSON/JSONC/JSON5 support", default_on: true },
    ListRow { name: "yaml", help: "Include YAML support", default_on: true },
    ListRow { name: "toml", help: "Include TOML support", default_on: true },
    ListRow { name: "zon", help: "Include ZON support", default_on: true },
    // There is no `xml` row. Generic XML — a best-effort fold of attributes
    // and `#text` into a mapping, with no typed scalars and a single-root-key
    // printer — was a selectable, opt-in format through core 2.x and was
    // removed in core 3.0; its C ABI value, 6, is retired and never reused.
    // What survives is the shared XML *lexing substrate*, `xml/tokenizer.zig`,
    // which typed flavors (plist, and future `.csproj`/manifest readers) sit
    // on top of and which compiles whenever one of them does.
    ListRow { name: "figl", help: "Include the figl authoring dialect support", default_on: true },
    ListRow { name: "ini", help: "Include INI support", default_on: true },
    ListRow { name: "dotenv", help: "Include dotenv (.env) support", default_on: true },
    ListRow { name: "properties", help: "Include Java .properties support", default_on: true },
    // plist (XML variant only so far): a first-class typed flavor (typed
    // scalars, round-trips, in-place editor), held to its oracle and edit
    // suites in fig-conformance, and the intended long-term home for
    // structured XML config. On by default from 6.0; opt-in before.
    ListRow { name: "plist", help: "Include Apple XML property list support", default_on: true },
    // NestedText (nestedtext.org): reader + printer + editor, untyped-string
    // scalars like INI, held to the official test suite (in fig-conformance).
    // On by default like TOML/ZON/INI.
    ListRow { name: "nestedtext", help: "Include NestedText support", default_on: true },
];

/// The position of the row named `name` (Zig's `indexOf`, a compile error
/// for a name that is not a format; `None` here).
pub fn list_index_of(name: &str) -> Option<usize> {
    LIST_ROWS.iter().position(|r| r.name == name)
}

/// Whether `name` is a row.
pub fn list_has(name: &str) -> bool {
    list_index_of(name).is_some()
}

// ============================================================================
// THE REGISTRY
// ============================================================================

/// One registered dialect: the registry's row `index` names it, and `abi` is
/// the integer a C caller holds. Immutable once appended.
pub struct Entry {
    pub index: u16,
    pub abi: i32,
    /// The registration this row belongs to, shared by its dialect rows.
    pub language: Arc<Registered>,
    pub name: String,
    /// The language's own dialect this row selects (Zig's `typeOf`, the
    /// `L.Type` the vtable's thunks map the row's name to).
    pub dialect: Dialect,
    /// The dialect whose printer the row prints with: its own for a declared
    /// row, and for a `--spec` row the declared dialect it is a version of
    /// (`kit/contract.zig`'s `printWith`).
    pub print_dialect: Dialect,
    pub extensions: Vec<&'static str>,
    pub splice: SpliceStyle,
    pub empty_doc_seed: Option<&'static [u8]>,
    /// The dialect's own `syntax`, or the language's.
    pub syntax: Option<Syntax>,
    /// What the dialect's printer makes of another format's tree, if it
    /// says.
    pub prints: Option<Prints>,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry").field("index", &self.index).field("abi", &self.abi).field("name", &self.name).finish()
    }
}

/// One registration: the language, and what was read off it once.
pub struct Registered {
    pub lang: &'static dyn Language,
    pub name: String,
    pub caps: Caps,
    pub syntax: Option<Syntax>,
    pub samples: Vec<&'static [u8]>,
    /// The registry index of the row named after the language; its rows are
    /// `first_index..first_index + entry_count`, in declaration order.
    pub first_index: usize,
    pub entry_count: usize,
}

impl std::fmt::Debug for Registered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registered").field("name", &self.name).field("caps", &self.caps).finish()
    }
}

/// As many dialect rows as the registry holds; `Language.Type` is a u16, so
/// the bound is that space less headroom.
pub const MAX_ENTRIES: usize = 4096;

/// The format registry: append-only, one writer at a time, every read of a
/// settled value. There is no unregistration: an integer is valid for the
/// life of the registry.
#[derive(Default)]
pub struct Registry {
    entries: RwLock<Vec<Arc<Entry>>>,
    /// Serializes writers only.
    write: Mutex<()>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("count", &self.count()).finish()
    }
}

/// `register_with`'s options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterOptions {
    /// Run the harness over the samples. A language its build has already
    /// held to it — one a consumer bundles — may skip it at startup.
    pub harness: bool,
}

impl Default for RegisterOptions {
    fn default() -> Self {
        RegisterOptions { harness: true }
    }
}

// ── refusals ───────────────────────────────────────────────────────────────

const REFUSAL_CAP: usize = 512;

thread_local! {
    /// Why the most recent `register` on this thread was refused, for the C
    /// API to report. Thread-local so two hosts registering at once do not
    /// read each other's reason.
    static LAST_REFUSAL: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Record why a registration is about to be refused. Public so a host
/// that builds a language from something else (the CLI's helper runner) can
/// report through the same channel when that something fails to load. Kept
/// to the 512 bytes Zig's buffer holds.
pub fn refuse(message: &str) {
    let bytes = message.as_bytes();
    let n = bytes.len().min(REFUSAL_CAP);
    LAST_REFUSAL.with(|r| {
        let mut r = r.borrow_mut();
        r.clear();
        r.extend_from_slice(&bytes[..n]);
    });
}

pub fn last_refusal() -> Vec<u8> {
    LAST_REFUSAL.with(|r| r.borrow().clone())
}

/// Forget the last refusal, for a host about to ask something that may
/// write one.
pub fn clear_refusal() {
    LAST_REFUSAL.with(|r| r.borrow_mut().clear());
}

/// A dialect row of a language: one of `dialects()`, or one of their
/// `--spec` versions, named `<dialect>-<spec>` (`yaml-1.1`), which claims no
/// extension of its own (`kit/contract.zig`'s `rows`).
struct Row {
    name: String,
    /// The declared row it is, or is a version of.
    of: usize,
    dialect: Dialect,
    declared: bool,
}

fn rows_of(lang: &dyn Language) -> Vec<Row> {
    let dialects = lang.dialects();
    let mut out: Vec<Row> = Vec::new();
    for (i, d) in dialects.iter().enumerate() {
        out.push(Row { name: d.name.to_string(), of: i, dialect: d.dialect, declared: true });
    }
    for (i, d) in dialects.iter().enumerate() {
        for sp in d.specs {
            out.push(Row { name: format!("{}-{}", d.name, sp.name), of: i, dialect: sp.dialect, declared: false });
        }
    }
    out
}

/// Every extension of `lang` goes to the dialect named after it, else to the
/// first (`kit/contract.zig`'s `extensionsOf`).
fn extensions_of(lang: &dyn Language, row: usize) -> Vec<&'static str> {
    if let Some(own) = lang.dialect_extensions(row) {
        return own;
    }
    let mut out = Vec::new();
    for &e in lang.extensions() {
        let mut owner = 0usize;
        for (i, d) in lang.dialects().iter().enumerate() {
            if d.name == e {
                owner = i;
            }
        }
        if owner == row {
            out.push(e);
        }
    }
    out
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Register `lang`. Returns the ABI integer of its first dialect row.
    /// See the module doc for what happens in between.
    pub fn register(&self, lang: &'static dyn Language) -> Result<i32> {
        self.register_with(lang, RegisterOptions::default())
    }

    /// `register`, for a language held by an `Arc`: it is kept for the life
    /// of the process, as a registration is in Zig.
    pub fn register_arc(&self, lang: Arc<dyn Language>, options: RegisterOptions) -> Result<i32> {
        let held: &'static Arc<dyn Language> = Box::leak(Box::new(lang));
        self.register_with(&**held, options)
    }

    /// `register`, with `options`.
    ///
    /// Errors: `InvalidLanguage` (the language failed `validate_language`;
    /// `last_refusal` has the reason), `HarnessFailed` (the harness failed
    /// over a declared sample; `last_refusal` says which), `NameTaken` (a
    /// name already registered), `RegistryFull`.
    pub fn register_with(&self, lang: &'static dyn Language, options: RegisterOptions) -> Result<i32> {
        clear_refusal();
        if let Err(msg) = validate_language(lang) {
            refuse(&msg);
            return Err(err!(InvalidLanguage));
        }

        let _w = self.write.lock().unwrap_or_else(|p| p.into_inner());

        // A name is taken once.
        let name = lang.name();
        if self.find_by_name(name).is_some() {
            refuse(&format!("a format named '{name}' is already registered"));
            return Err(err!(NameTaken));
        }
        let rows = rows_of(lang);
        for r in &rows {
            if self.find_by_name(&r.name).is_some() {
                refuse(&format!("a format named '{}' is already registered", r.name));
                return Err(err!(NameTaken));
            }
        }
        let first_index = self.count();
        if first_index + rows.len() > MAX_ENTRIES {
            return Err(err!(RegistryFull));
        }

        let caps = lang.caps();
        let dialects = lang.dialects();
        // The language's syntax is its first row's, as the vtable states it.
        let row_syntax = |d: Dialect| if caps.edit { Some(lang.syntax(d)) } else { None };
        let reg = Arc::new(Registered {
            lang,
            name: name.to_string(),
            caps,
            syntax: row_syntax(rows[0].dialect),
            samples: lang.samples().to_vec(),
            first_index,
            entry_count: rows.len(),
        });
        let mut row_entries: Vec<Arc<Entry>> = Vec::with_capacity(rows.len());
        for (i, r) in rows.iter().enumerate() {
            let d = &dialects[r.of];
            let index = first_index + i;
            row_entries.push(Arc::new(Entry {
                index: index as u16,
                abi: (index + 1) as i32,
                language: reg.clone(),
                name: r.name.clone(),
                dialect: r.dialect,
                print_dialect: d.dialect,
                extensions: if r.declared { extensions_of(lang, r.of) } else { Vec::new() },
                splice: d.splice,
                empty_doc_seed: d.empty_doc_seed,
                syntax: row_syntax(r.dialect),
                prints: d.prints,
            }));
        }

        // The harness, before the rows are published: nothing else can
        // observe them meanwhile, since the writer lock is held and the rows
        // are not in the list.
        if options.harness {
            check_registration(&reg, &row_entries)?;
        }

        let mut entries = self.entries.write().unwrap_or_else(|p| p.into_inner());
        let abi = row_entries[0].abi;
        entries.extend(row_entries);
        Ok(abi)
    }

    fn find_by_name(&self, name: &str) -> Option<Arc<Entry>> {
        let entries = self.entries.read().unwrap_or_else(|p| p.into_inner());
        entries.iter().find(|e| e.name == name).cloned()
    }

    /// The entry at registry row `index`, or `None` past the end.
    pub fn entry_at(&self, index: usize) -> Option<Arc<Entry>> {
        let entries = self.entries.read().unwrap_or_else(|p| p.into_inner());
        entries.get(index).cloned()
    }

    /// The entry an ABI integer names — its index plus one — or `None` for
    /// one never handed out.
    pub fn entry_by_abi(&self, abi: i32) -> Option<Arc<Entry>> {
        if abi < 1 {
            return None;
        }
        self.entry_at((abi - 1) as usize)
    }

    /// The entry named `name` — a dialect row's name — or `None`.
    pub fn entry_by_name(&self, name: &str) -> Option<Arc<Entry>> {
        self.find_by_name(name)
    }

    /// `entry_by_name`, as the bundle's own registration asks it (in Zig,
    /// without registering the bundle first; a `Registry` value has no
    /// bundle of its own, so the two are one).
    pub fn registered_by_name(&self, name: &str) -> Option<Arc<Entry>> {
        self.find_by_name(name)
    }

    /// The first registered row, in registry order, that claims extension
    /// `ext` (without the dot), as the CLI resolves a file's format
    /// (`cli/languages.zig`).
    pub fn entry_by_extension(&self, ext: &str) -> Option<Arc<Entry>> {
        let entries = self.entries.read().unwrap_or_else(|p| p.into_inner());
        entries.iter().find(|e| e.extensions.contains(&ext)).cloned()
    }

    /// Content sniffing in the consumer's order: the first registered dialect
    /// `order` names whose parser accepts `input`, or `None`. The order is
    /// the consumer's to choose — the grammars overlap, so it decides what
    /// ambiguous input means — and a language never ranks itself against
    /// another. A name no dialect has, or one that cannot read, is skipped.
    pub fn detect_among(&self, input: &[u8], order: &[&str]) -> Option<Arc<Entry>> {
        for name in order {
            let Some(e) = self.entry_by_name(name) else { continue };
            if !e.language.caps.read {
                continue;
            }
            if Parser::new().parse(input, &e).is_err() {
                continue;
            }
            return Some(e);
        }
        None
    }

    /// How many dialect rows are registered.
    pub fn count(&self) -> usize {
        self.entries.read().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Every entry, in registry order.
    pub fn entries(&self) -> Vec<Arc<Entry>> {
        self.entries.read().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

// ── the bundle, registered on first use ────────────────────────────────────
//
// The languages a build bundles are registered the first time anything
// looks a format up in the global registry: the CLI's, the tests' and a
// conformance adapter's.

/// The languages fig bundles, in `LIST_ROWS` order: what `global()`
/// registers on first use. None without the `bundle` feature, as a Zig
/// build with every `-D<name>` off bundles none (the `libfig` the bindings
/// link, whose languages each register from a library of their own).
#[cfg(not(feature = "bundle"))]
fn bundled_languages() -> Vec<&'static dyn Language> {
    Vec::new()
}

/// The languages fig bundles, in `LIST_ROWS` order: what `global()`
/// registers on first use.
#[cfg(feature = "bundle")]
fn bundled_languages() -> Vec<&'static dyn Language> {
    use crate::lang;
    vec![
        &lang::json::LANGUAGE,
        &lang::yaml::LANGUAGE,
        &lang::toml::LANGUAGE,
        &lang::zon::LANGUAGE,
        &lang::figl::LANGUAGE,
        &lang::ini::LANGUAGE,
        &lang::dotenv::LANGUAGE,
        &lang::properties::LANGUAGE,
        &lang::plist::LANGUAGE,
        &lang::nestedtext::LANGUAGE,
    ]
}

static GLOBAL: OnceLock<Registry> = OnceLock::new();

/// The process-wide registry, with the bundle registered (Zig's module-level
/// registry and its `ensureBundled`). A refusal while registering the bundle
/// is swallowed, as Zig's `registerAll(...) catch {}` does, leaving the
/// languages registered before it.
pub fn global() -> &'static Registry {
    GLOBAL.get_or_init(|| {
        let r = Registry::new();
        let _ = register_bundle(&r, &bundled_languages());
        r
    })
}

// ============================================================================
// THE BUNDLE (bundle.zig)
// ============================================================================

/// Register the bundled language `lang`, unless it is already, and return
/// the format number of its first dialect (bundle.zig's `register`). Its
/// build ran the harness over its samples; this crate's own test build runs
/// it again here.
pub fn register_bundled(registry: &Registry, lang: &'static dyn Language) -> Result<i32> {
    if let Some(first) = lang.dialects().first() {
        if let Some(e) = registry.registered_by_name(first.name) {
            return Ok(e.abi);
        }
    }
    registry.register_with(lang, RegisterOptions { harness: cfg!(test) })
}

/// Register every language of `languages`, in order (bundle.zig's
/// `registerAll`): fig's ten, in `LIST_ROWS` order, are what the CLI, the LSP
/// and the conformance adapter bundle. Stops at the first refusal.
pub fn register_bundle(registry: &Registry, languages: &[&'static dyn Language]) -> Result<()> {
    for &lang in languages {
        register_bundled(registry, lang)?;
    }
    Ok(())
}

/// The declared dialects of `languages`, in order — what a consumer lists
/// as the formats it knows of (bundle.zig's `all_dialects`). `--spec`
/// versions are rows of their own once registered, but are not listed here.
pub fn all_dialects(languages: &[&'static dyn Language]) -> Vec<&'static str> {
    languages.iter().flat_map(|l| l.dialects().iter().map(|d| d.name)).collect()
}

// ── validation ─────────────────────────────────────────────────────────────

/// `Language.validate`, at registration, over a language rather than a type
/// (Zig's `validateVTable`, the runtime registry's rules, then the
/// trait's own `kit::language::validate`). The `Err` is the refusal's
/// message. The rules of the C record that a trait cannot break — a
/// version, unknown capability or lossless bits, a depth over 255, an
/// unknown splice style, a `serialize` capability with no print slot, an
/// editable language with no syntax, a half-stated `closed_containers` or
/// `section_header` — have no counterpart.
pub fn validate_language(lang: &dyn Language) -> core::result::Result<(), String> {
    let name = lang.name();
    if name.is_empty() {
        return Err("a language must have a name".into());
    }
    let caps = lang.caps();
    if !caps.read {
        return Err(format!("'{name}' declares no read capability; a language must at least parse"));
    }
    let rows = rows_of(lang);
    if rows.is_empty() {
        return Err(format!("'{name}' declares no dialects, so no format could name it"));
    }
    if rows[0].name != name {
        return Err(format!(
            "'{name}': the first dialect row must be named after the language, and is '{}'",
            rows[0].name
        ));
    }
    for (i, d) in rows.iter().enumerate() {
        if d.name.is_empty() {
            return Err(format!("'{name}': dialect row {i} has no name"));
        }
        if rows[i + 1..].iter().any(|o| o.name == d.name) {
            return Err(format!("'{name}' declares two dialects named '{}'", d.name));
        }
    }
    if lang.samples().is_empty() {
        return Err(format!(
            "'{name}' declares no samples; registration checks a language against what it says it parses"
        ));
    }
    if !caps.serialize && caps.lossless.is_some() {
        return Err(format!(
            "'{name}' declares lossless (an envelope target for serialized output) but no serialize capability"
        ));
    }
    let types = lang.type_count().max(rows.iter().map(|r| r.dialect as usize + 1).max().unwrap_or(0));
    // A vtable slot is the language's, whatever the dialect: present when
    // any dialect has the renderer.
    let has = |which: Renderer| (0..types).any(|t| lang.has_renderer(t as Dialect, which));
    if !caps.edit {
        if Renderer::ALL.into_iter().any(has) {
            return Err(format!("'{name}' declares no edit capability but supplies an editing renderer"));
        }
        return language::validate(lang);
    }
    // The syntax coherence rules, over the language's syntax and every
    // dialect's own.
    validate_syntax(name, &lang.syntax(rows[0].dialect), &has)?;
    for r in &rows {
        validate_syntax(name, &lang.syntax(r.dialect), &has)?;
    }
    language::validate(lang)
}

fn validate_syntax(name: &str, s: &Syntax, has: &dyn Fn(Renderer) -> bool) -> core::result::Result<(), String> {
    if s.comments.trailing.is_some() && s.comments.line.is_none() {
        return Err(format!("'{name}' declares a trailing comment marker but no line comment marker"));
    }
    if s.kv_sep.is_none() && !has(Renderer::Entry) {
        return Err(format!(
            "'{name}' declares kv_sep = null but no render_entry, so the generic entry-insert paths have no separator to write"
        ));
    }
    if !s.block_seq_editable && has(Renderer::Item) {
        return Err(format!(
            "'{name}' declares block_seq_editable = false but supplies render_item, which the engine refuses before reaching"
        ));
    }
    if s.flow_entry_sep.is_empty() {
        return Err(format!("'{name}': flow_entry_sep is empty; null means `,`"));
    }
    if s.flow_root && !s.flow_containers {
        return Err(format!("'{name}' declares flow_root but flow_containers = false"));
    }
    Ok(())
}

// ── the load-time harness ──────────────────────────────────────────────────

/// `harness.zig`'s checks over one registration: every sample parses;
/// prints and reparses to the same tree where the language prints; its
/// regions are well-formed; and, where it edits, an `Editor` constructs
/// over it and a no-op splice is the identity. The editor reads the rows as
/// a caller's will (`api::RuntimeLanguage`), though they are not published
/// yet.
fn check_registration(reg: &Registered, entries: &[Arc<Entry>]) -> Result<()> {
    for e in entries {
        for &sample in &reg.samples {
            let mut parser = Parser::new();
            let first = match parser.parse(sample, e) {
                Ok(d) => d,
                Err(err) => {
                    refuse(&format!(
                        "'{}': sample does not parse ({}): {}",
                        e.name,
                        err.name(),
                        String::from_utf8_lossy(parser.last_message())
                    ));
                    return Err(err!(HarnessFailed));
                }
            };
            if expect_regions_well_formed(&first, sample).is_err() {
                refuse(&format!("'{}': a sample's regions table is malformed", e.name));
                return Err(err!(HarnessFailed));
            }
            if reg.caps.serialize {
                let mut out = Vec::new();
                if let Err(err) = print_with(e, &first.ast, &SerializeOptions::default(), &mut out) {
                    refuse(&format!("'{}': sample does not print ({})", e.name, err.name()));
                    return Err(err!(HarnessFailed));
                }
                let second = match parser.parse(&out, e) {
                    Ok(d) => d,
                    Err(err) => {
                        refuse(&format!(
                            "'{}': printed sample does not reparse ({}): {}",
                            e.name,
                            err.name(),
                            String::from_utf8_lossy(parser.last_message())
                        ));
                        return Err(err!(HarnessFailed));
                    }
                };
                // Positional: a table's ids are its pre-order, so the same
                // tree reparsed is the same rows in the same order.
                if !first.ast.eql(&second.ast) || !first.ast.comments_eql(&second.ast) {
                    refuse(&format!("'{}': print then reparse changed a sample's tree", e.name));
                    return Err(err!(HarnessFailed));
                }
            }
            if reg.caps.edit {
                let lang = crate::api::RuntimeLanguage::new(e.clone());
                let mut ed = crate::editor::Editor::new(&lang, e.dialect);
                if let Err(err) = ed.init(sample) {
                    refuse(&format!("'{}': Editor does not construct over a sample ({})", e.name, err.name()));
                    return Err(err!(HarnessFailed));
                }
                if let Err(err) = ed.replace_at_span(Span::new(0, 0), b"") {
                    refuse(&format!("'{}': a no-op splice fails ({})", e.name, err.name()));
                    return Err(err!(HarnessFailed));
                }
                if sample != ed.source() {
                    refuse(&format!("'{}': a no-op splice changed the source", e.name));
                    return Err(err!(HarnessFailed));
                }
            }
        }
    }
    Ok(())
}

fn expect_regions_well_formed(doc: &Document, source: &[u8]) -> Result<()> {
    let mut prev: Option<table::SpanFact> = None;
    for &r in doc.ast.region_facts() {
        if r.node as usize >= doc.ast.node_count() {
            return Err(err!(MalformedRegion));
        }
        match doc.ast.kind_of(r.node) {
            NodeKind::Mapping(_) | NodeKind::Sequence(_) => {}
            _ => return Err(err!(MalformedRegion)),
        }
        let (start, end) = (r.start as usize, r.end as usize);
        if !(start < end && end <= source.len()) {
            return Err(err!(MalformedRegion));
        }
        if !(start == 0 || source[start - 1] == b'\n') {
            return Err(err!(MalformedRegion));
        }
        if !(end == source.len() || source[end - 1] == b'\n') {
            return Err(err!(MalformedRegion));
        }
        if let Some(p) = prev {
            if !(p.node < r.node || (p.node == r.node && p.start < r.start)) {
                return Err(err!(MalformedRegion));
            }
        }
        prev = Some(r);
    }
    Ok(())
}

// ============================================================================
// THE LANGUAGE, THROUGH AN ENTRY
// ============================================================================

/// The error names a printer's refusal keeps across the C ABI
/// (`abi.Refusal`); any other is `RuntimePrintFailed`.
const PRINT_REFUSALS: [&str; 9] = [
    "NullUnsupported",
    "NonStringKey",
    "UnrepresentableRoot",
    "UnsupportedValue",
    "InvalidKey",
    "NonFiniteNumber",
    "UnresolvedAlias",
    "NestingTooDeep",
    "UnexpectedNodeKind",
];

/// Print `ast` in the dialect `e` names.
pub fn print_with(e: &Entry, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
    print_node_with(e, ast, ast.root, options, out)
}

/// `print_with` from `root`: the subtree as the whole table, which is what
/// `fig_value_serialize` asks for. A runtime printer is not told it is a
/// fragment; the table's root is its document.
pub fn print_node_with(e: &Entry, ast: &AST, root: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
    let reg = &e.language;
    if !reg.caps.serialize {
        return Err(err!(FormatDisabled));
    }
    // Stripping is core's, as it is for a compiled printer: the language
    // is handed a table with no comments rather than asked to drop them.
    let mut built = convert::document_to_table(ast, root)?;
    if options.strip_comments {
        built.comments.clear();
    }
    // The language reads the table back (the vtable's `print` thunk).
    let Ok(doc) = convert::view_to_document(Arc::from(&b""[..]), &built.view()) else {
        return Err(err!(RuntimePrintFailed));
    };
    // What the C `PrintOptions` carries; everything else is its default.
    let opts = SerializeOptions {
        pretty: options.pretty,
        strip_comments: options.strip_comments,
        indent: options.indent,
        width: options.width,
        splice: options.splice,
        flow: options.flow,
        ..SerializeOptions::default()
    };
    let mut buf = Vec::new();
    match reg.lang.print(e.print_dialect, &doc.ast, &opts, &mut buf) {
        Ok(()) => {
            out.extend_from_slice(&buf);
            Ok(())
        }
        Err(err) if PRINT_REFUSALS.contains(&err.name()) => Err(err),
        Err(_) => Err(err!(RuntimePrintFailed)),
    }
}

/// The C `ErrorInfo`'s message buffer, less its NUL.
const MESSAGE_CAP: usize = 255;

/// A parse through an entry, with what it left behind (Zig's runtime
/// `Language.Parser`): the message of the most recent failure, and every
/// diagnostic — the errors a failed parse left and the warnings a clean one
/// raised.
#[derive(Debug, Clone, Default)]
pub struct Parser {
    /// The most recent failed parse's message (Zig's `last_error.text()`).
    pub last_error: Vec<u8>,
    /// The most recent failed parse's byte offset (`last_error.byte_offset`).
    pub last_offset: usize,
    pub errors: Vec<ParseDiagnostic>,
    pub warnings: Vec<ParseDiagnostic>,
}

impl Parser {
    pub fn new() -> Parser {
        Parser::default()
    }

    pub fn last_message(&self) -> &[u8] {
        &self.last_error
    }

    fn set_error(&mut self, message: &[u8], offset: usize) {
        let n = message.len().min(MESSAGE_CAP);
        self.last_error.clear();
        self.last_error.extend_from_slice(&message[..n]);
        self.last_offset = offset;
    }

    /// A `Document` over `input` in the dialect `e` names. Fails with
    /// `RuntimeParseFailed`, its message in `last_message`.
    pub fn parse(&mut self, input: &[u8], e: &Entry) -> Result<Document> {
        let reg = &e.language;
        let source: Arc<[u8]> = Arc::from(input);
        let mut tbl = Table::init(source.clone());
        self.last_error.clear();
        self.last_offset = 0;
        language::take_parse_error();
        let parsed = reg.lang.parse_table(&source, e.dialect, &mut tbl);
        let stated = language::take_parse_error();
        // The parse thunk's report: what the language stated (a runtime
        // language's `ErrorInfo`), else a refusal's first error, or its name.
        let refusal = parsed.err().map(|err| {
            if let Some(s) = stated {
                return s;
            }
            match tbl.diagnostics.iter().find(|d| d.severity == table::DIAGNOSTIC_ERROR) {
                Some(d) => (tbl.text_of(d.message).unwrap_or(b"").to_vec(), d.start as usize),
                None => (err.name().as_bytes().to_vec(), 0),
            }
        });
        let mut report = language::ParseReport::default();
        report.collect(&tbl);
        self.errors.extend(report.errors);
        self.warnings.extend(report.warnings);
        if let Some((message, offset)) = refusal {
            self.set_error(&message, offset);
            return Err(err!(RuntimeParseFailed));
        }
        let mut doc = match convert::table_to_document(source, &mut tbl) {
            Ok(d) => d,
            Err(err) => {
                let message: &[u8] = match err.name() {
                    "UnknownKind" => b"the parser returned a node kind this fig does not know",
                    "MalformedTable" => b"the parser returned a malformed node table",
                    _ => return Err(err),
                };
                self.set_error(message, 0);
                return Err(err!(RuntimeParseFailed));
            }
        };
        if let Some(sx) = e.syntax.or(reg.syntax) {
            doc.ast.merge_key = sx.merge_key;
        }
        Ok(doc)
    }
}

impl Entry {
    /// The language this row is a dialect of.
    pub fn lang(&self) -> &'static dyn Language {
        self.language.lang
    }

    pub fn caps(&self) -> Caps {
        self.language.caps
    }

    /// A `Document` over `input` in this dialect (`Parser.parse`).
    pub fn parse(&self, input: &[u8]) -> Result<Document> {
        Parser::new().parse(input, self)
    }

    /// The dialect's syntax, else the language's (Zig's runtime
    /// `Language.syntax(t)`). A read-only format has no syntax; the editor
    /// over it is refused before it is asked, so the default is unreachable
    /// in practice and a safe answer in principle.
    pub fn syntax_or_default(&self) -> Syntax {
        if let Some(s) = self.syntax {
            return s;
        }
        self.language.syntax.unwrap_or(Syntax::new(Comments::NONE, None, None))
    }

    /// `Editor.hasRenderer`'s answer: whether the language renders `which`.
    pub fn has_renderer(&self, which: Renderer) -> bool {
        self.language.lang.has_renderer(self.dialect, which)
    }

    /// One renderer call: the fragment appended to `out`. Fails with
    /// `UnsupportedShape` for a language without the renderer, and with
    /// `RendererRefused` for a renderer's refusal, whose words are kept in
    /// `last_refusal`.
    pub fn render(&self, which: Renderer, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()> {
        let mut buf = Vec::new();
        match self.language.lang.render(which, self.dialect, &mut buf, r) {
            None => Err(err!(UnsupportedShape)),
            Some(Err(e)) => {
                // A renderer's refusal: the format saying the text has no
                // spelling here. A language that refuses with
                // `RendererRefused` has put its own words in `last_refusal`
                // already (a runtime language's `ErrorInfo`, which the helper
                // wire fills from the helper's message).
                if e != err!(RendererRefused) {
                    let text = e.name();
                    refuse(if text.is_empty() { "the renderer declined" } else { text });
                }
                Err(err!(RendererRefused))
            }
            Some(Ok(())) => {
                out.extend_from_slice(&buf);
                Ok(())
            }
        }
    }

    /// The language's `split_stream`, or `UnsupportedOperation` for a
    /// language without one. A failure, or a document range outside `input`,
    /// is `RuntimeParseFailed`.
    pub fn split_stream(&self, input: &[u8]) -> Result<Vec<StreamDocument>> {
        let Some(split) = self.language.lang.split_stream(input, self.dialect) else {
            return Err(err!(UnsupportedOperation));
        };
        let Ok(docs) = split else {
            return Err(err!(RuntimeParseFailed));
        };
        for d in &docs {
            if d.start > d.end || d.end > input.len() {
                return Err(err!(RuntimeParseFailed));
            }
        }
        Ok(docs)
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::kit::ast::{KeyValue, PathSegment};
    use crate::kit::language::add_diagnostic;
    use crate::kit::manifest::{DialectInfo, NativeKinds, SpecName};
    use crate::kit::row_writer::RowWriter;
    use crate::kit::span::Span;

    // `tinykv`: `key=value` lines, `#` comments, nothing else — enough to
    // drive every path above from Rust. The test registers it, parses through
    // an entry, prints, and registers broken languages to see them refused.

    pub(crate) struct TinyKv {
        pub samples: &'static [&'static [u8]],
        pub name: &'static str,
        pub dialects: &'static [DialectInfo],
        pub caps: Caps,
    }

    pub(crate) const TINY_DIALECTS: &[DialectInfo] = &[DialectInfo::new("tinykv", SpliceStyle::Raw, Some(b""))];
    pub(crate) const TINY_SAMPLES: &[&[u8]] = &[b"a=1\nb=two\n", b"# top\nk=v\n"];

    pub(crate) const TINY: TinyKv = TinyKv {
        samples: TINY_SAMPLES,
        name: "tinykv",
        dialects: TINY_DIALECTS,
        caps: Caps { read: true, edit: true, serialize: true, ..Caps::DEFAULT },
    };

    impl Language for TinyKv {
        fn name(&self) -> &str {
            self.name
        }
        fn extensions(&self) -> &[&'static str] {
            &["tkv"]
        }
        fn caps(&self) -> Caps {
            self.caps
        }
        fn dialects(&self) -> &[DialectInfo] {
            self.dialects
        }
        fn samples(&self) -> &[&'static [u8]] {
            self.samples
        }
        fn syntax(&self, _: Dialect) -> Syntax {
            Syntax { flow_containers: false, ..Syntax::new(Comments::HASH, Some(b"="), Some(b"{}")) }
        }
        fn parse_table(&self, _input: &[u8], _: Dialect, out: &mut Table) -> Result<()> {
            let src = out.source_arc();
            let mut w = RowWriter::init(out);
            let root = w.add(NodeKind::Mapping(None), Span::new(0, src.len()))?;
            let mut last: Option<u32> = None;
            let mut pending: Vec<&[u8]> = Vec::new();
            let mut at = 0usize;
            while at < src.len() {
                let nl = src[at..].iter().position(|&c| c == b'\n').map_or(src.len(), |p| at + p);
                let line = &src[at..nl];
                let start = at;
                at = nl + 1;
                if line.is_empty() {
                    continue;
                }
                if line[0] == b'#' {
                    let c = &line[1..];
                    let s = c.iter().position(|&b| b != b' ').unwrap_or(c.len());
                    let e = c.iter().rposition(|&b| b != b' ').map_or(s, |p| p + 1);
                    pending.push(&c[s..e.max(s)]);
                    continue;
                }
                let Some(eq) = line.iter().position(|&c| c == b'=') else {
                    add_diagnostic(w.t, table::DIAGNOSTIC_ERROR, start, None, b"expected key=value", b"here")?;
                    return Err(err!(MissingEquals));
                };
                let k = w.add(NodeKind::String(&line[..eq]), Span::new(start, start + eq))?;
                let v = w.add(NodeKind::String(&line[eq + 1..]), Span::new(start + eq + 1, nl))?;
                let kv = w.add(NodeKind::KeyValue(KeyValue { key: k, value: v }), Span::new(start, nl))?;
                w.sep(kv, Span::new(start + eq, start + eq + 1))?;
                for c in pending.drain(..) {
                    w.comment(k, table::COMMENT_LEADING, crate::kit::ast::Comment::line(c))?;
                }
                match last {
                    Some(p) => w.set_next(p, Some(kv)),
                    None => w.set(root, NodeKind::Mapping(Some(kv)))?,
                }
                last = Some(kv);
            }
            w.finish(root)
        }
        fn print(&self, _: Dialect, ast: &AST, _: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
            let NodeKind::Mapping(mut c) = ast.kind_of(ast.root) else {
                return Err(err!(UnrepresentableRoot));
            };
            while let Some(id) = c {
                let NodeKind::KeyValue(kv) = ast.kind_of(id) else { unreachable!() };
                let (NodeKind::String(k), NodeKind::String(v)) = (ast.kind_of(kv.key), ast.kind_of(kv.value)) else {
                    return Err(err!(TinyKvHoldsAFlatStringMap));
                };
                for cm in ast.comments(kv.key).leading.iter() {
                    out.extend_from_slice(b"# ");
                    out.extend_from_slice(cm.text);
                    out.push(b'\n');
                }
                out.extend_from_slice(k);
                out.push(b'=');
                out.extend_from_slice(v);
                out.push(b'\n');
                c = ast.node(id).next_sibling;
            }
            Ok(())
        }
    }

    fn leak(t: TinyKv) -> &'static TinyKv {
        Box::leak(Box::new(t))
    }

    #[test]
    fn a_language_registered_parses_and_prints_through_its_entry() {
        let r = Registry::new();
        let abi = r.register(&TINY).unwrap();
        assert_eq!(r.entry_by_name("tinykv").unwrap().index as i32 + 1, abi);
        let e = r.entry_by_abi(abi).unwrap();
        assert_eq!("tinykv", e.name);
        assert!(Arc::ptr_eq(&r.entry_by_name("tinykv").unwrap(), &e));
        assert!(e.language.caps.edit);
        assert_eq!("tkv", e.extensions[0]);
        assert_eq!(e.index as usize + 1, r.count());
        assert!(Arc::ptr_eq(&r.entry_by_extension("tkv").unwrap(), &e));
        assert!(r.entry_by_extension("json").is_none());

        // Parse through the entry, as the C API and CLI do.
        let doc = e.parse(b"# note\nx=1\ny=2\n").unwrap();
        assert_eq!(b"2", doc.ast.get_val_by_path(&[PathSegment::Key(b"y")]).unwrap().kind.as_string().unwrap());

        // Print through the entry.
        let mut out = Vec::new();
        print_with(&e, &doc.ast, &SerializeOptions::default(), &mut out).unwrap();
        assert_eq!(&b"# note\nx=1\ny=2\n"[..], &out[..]);
        out.clear();
        print_with(&e, &doc.ast, &SerializeOptions { strip_comments: true, ..Default::default() }, &mut out).unwrap();
        assert_eq!(&b"x=1\ny=2\n"[..], &out[..]);

        // A parse failure carries the language's message.
        let mut parser = Parser::new();
        assert_eq!(err!(RuntimeParseFailed), parser.parse(b"no equals\n", &e).unwrap_err());
        assert_eq!(b"expected key=value", parser.last_message());
        assert_eq!(1, parser.errors.len());

        // Sniffing, in the consumer's order.
        assert!(r.detect_among(b"a=1\n", &["nope", "tinykv"]).is_some());
        assert!(r.detect_among(b"[1]\n", &["tinykv"]).is_none());

        // No stream splitting, no renderers but none asked.
        assert_eq!(err!(UnsupportedOperation), e.split_stream(b"").unwrap_err());
        assert_eq!(err!(UnsupportedShape), e.render(Renderer::Value, &mut Vec::new(), &RenderRequest::default()).unwrap_err());
    }

    #[test]
    fn registration_refuses_a_language_that_fails_validation_or_the_harness() {
        let r = Registry::new();
        // What is registered before any refusal: none of them adds to it.
        let before = r.count();

        let no_samples = leak(TinyKv { samples: &[], ..TINY });
        assert_eq!(err!(InvalidLanguage), r.register(no_samples).unwrap_err());
        assert!(String::from_utf8_lossy(&last_refusal()).contains("no samples"));

        let bad = leak(TinyKv { samples: &[b"not a pair\n"], ..TINY });
        assert_eq!(err!(HarnessFailed), r.register(bad).unwrap_err());
        assert!(String::from_utf8_lossy(&last_refusal()).contains("sample does not parse"));
        assert_eq!(before, r.count());

        // Without the harness, the same language registers.
        let r2 = Registry::new();
        r2.register_with(bad, RegisterOptions { harness: false }).unwrap();

        r.register(&TINY).unwrap();
        assert_eq!(err!(NameTaken), r.register(&TINY).unwrap_err());

        // A dialect row named as another language's is taken too.
        const TAKEN: &[DialectInfo] =
            &[DialectInfo::new("tinykv2", SpliceStyle::Raw, Some(b"")), DialectInfo { dialect: 1, ..DialectInfo::new("tinykv", SpliceStyle::Raw, Some(b"")) }];
        let two = leak(TinyKv { name: "tinykv2", dialects: TAKEN, ..TINY });
        assert_eq!(err!(NameTaken), r.register(two).unwrap_err());

        // No read capability.
        const TINYR: &[DialectInfo] = &[DialectInfo::new("tinyr", SpliceStyle::Raw, None)];
        const TINYL: &[DialectInfo] = &[DialectInfo::new("tinyl", SpliceStyle::Raw, None)];
        let unread = leak(TinyKv { name: "tinyr", dialects: TINYR, caps: Caps { read: false, ..TINY.caps }, ..TINY });
        assert_eq!(err!(InvalidLanguage), r.register(unread).unwrap_err());
        assert!(String::from_utf8_lossy(&last_refusal()).contains("no read capability"));

        // A lossless declaration without serialize.
        let lossless = leak(TinyKv {
            name: "tinyl",
            dialects: TINYL,
            caps: Caps { serialize: false, lossless: Some(NativeKinds::NONE), ..TINY.caps },
            ..TINY
        });
        assert_eq!(err!(InvalidLanguage), r.register(lossless).unwrap_err());
    }

    #[test]
    fn spec_rows_are_rows_of_their_own_and_claim_no_extension() {
        const SPECS: &[SpecName] = &[SpecName { name: "1.0", dialect: 1 }];
        const ROWS: &[DialectInfo] = &[DialectInfo { specs: SPECS, ..DialectInfo::new("tinys", SpliceStyle::Raw, Some(b"")) }];
        let l = leak(TinyKv { name: "tinys", dialects: ROWS, ..TINY });
        let r = Registry::new();
        let abi = r.register(l).unwrap();
        assert_eq!(1, abi);
        assert_eq!(2, r.count());
        let spec = r.entry_by_name("tinys-1.0").unwrap();
        assert_eq!(1, spec.dialect);
        assert_eq!(0, spec.print_dialect);
        assert!(spec.extensions.is_empty());
        assert_eq!(vec!["tkv"], r.entry_by_name("tinys").unwrap().extensions);
    }

    #[test]
    fn the_list_names_ten_formats_once_each() {
        assert_eq!(Some(0), list_index_of("json"));
        assert_eq!(Some(9), list_index_of("nestedtext"));
        assert!(!list_has("xml"));
        for (i, a) in LIST_ROWS.iter().enumerate() {
            assert!(LIST_ROWS[i + 1..].iter().all(|b| b.name != a.name));
        }
    }

    #[test]
    fn the_bundle_registers_each_language_once() {
        let r = Registry::new();
        register_bundle(&r, &[&TINY, &TINY]).unwrap();
        assert_eq!(1, r.count());
        assert_eq!(vec!["tinykv"], all_dialects(&[&TINY]));
    }
}
