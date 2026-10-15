//! Port of src/cli/parse_dispatch.zig.
//!
//! Content-based parsing shared by `get`/`fmt`/`convert`/`check`: mapping the
//! CLI's `Format` (plus an optional `--spec` version) to the registered
//! dialect that parses it, and `check`'s per-file validate-and-report entry
//! point.

use std::borrow::Cow;

use fig_core::diagnostics::{self, Cause};
use fig_core::flat_strip;
use fig_core::kit::ast::{NodeId, SerializeOptions, AST};
use fig_core::kit::document::Document;
use fig_core::kit::language::ParseDiagnostic;
use fig_core::lossless::{self, NativeKinds};
use fig_core::materialize::{self, TagMode};
use fig_core::registry::{self, Entry, Parser};
use fig_core::serialize::Target;
use fig_core::{canonical, err, Result};

use crate::fileio::{self, Color, OpenMode, Terminal};
use crate::types::{self, Format};
use crate::{args, diag_report, gron, languages};

/// The canonical oracle format is opt-in (the `canonical` feature, Zig's
/// `-Dcanonical=true`) and otherwise compiled out of the CLI — but always
/// present in a test build. Mirrors the `canonical_enabled` gate in Zig's
/// `serialize.zig`.
pub const CANONICAL_ENABLED: bool = cfg!(feature = "canonical") || cfg!(test);

/// The dialect `--spec <version>` selects for `format`: the registered row
/// `<format>-<version>` (`yaml-1.1`, `toml-1.0`), which a language declares
/// for each version it reads. No `--spec` is `format` itself; a version the
/// format does not declare, or a format with none, is `UnsupportedSpec`.
pub fn resolve_spec(format: Format, spec_str: Option<&str>) -> Result<Format> {
    let Some(s) = spec_str else { return Ok(format) };
    let e = types::runtime_entry(format).ok_or(err!(UnsupportedSpec))?;
    let row = format!("{}-{}", e.name, s);
    if row.len() > 128 {
        return Err(err!(UnsupportedSpec));
    }
    let v = registry::global().entry_by_name(&row).ok_or(err!(UnsupportedSpec))?;
    Ok(types::runtime_format(&v))
}

/// What a parse left to report, as the language said it: the first error
/// of a failure (with its span and label), every error of a failure where
/// the language collects them, and the warnings of a clean parse, announced
/// under the dialect's name.
#[derive(Debug, Clone, Default)]
pub struct Reports {
    pub diag: Option<ParseDiagnostic>,
    pub errors: Vec<ParseDiagnostic>,
    pub warnings: Vec<ParseDiagnostic>,
    pub label: String,
}

impl Reports {
    /// Print a `file:line:col` teaching message for the parse's failure.
    pub fn report_diagnostics(&self, term: &mut Terminal, source: &[u8], file: &str) -> Result<()> {
        if let Some(d) = &self.diag {
            diag_report::report_parse_error(term, source, file, d.offset, d.end, &d.message, &d.short_label)?;
        }
        Ok(())
    }

    /// Announce authoring-time lints. These ride the same `--quiet`/
    /// `--strict` contract as the serialize-side diagnostics: quiet
    /// silences, strict aborts.
    pub fn report_warnings(&self, term: &mut Terminal, source: &[u8], file: &str, quiet: bool, strict: bool) -> Result<()> {
        diag_report::handle_rendered_warnings(term, source, file, &self.label, &self.warnings, quiet, strict)
    }

    /// Every recorded error — the `recover` path, where one parse reports
    /// every error in the file rather than stopping at the first — else the
    /// one diagnostic, else `None`.
    pub fn render_errors(&self) -> Option<Vec<ParseDiagnostic>> {
        if !self.errors.is_empty() {
            return Some(self.errors.clone());
        }
        self.diag.clone().map(|d| vec![d])
    }

    /// The warnings twin of `render_errors`.
    pub fn render_warnings(&self) -> Option<Vec<ParseDiagnostic>> {
        if !self.warnings.is_empty() {
            return Some(self.warnings.clone());
        }
        None
    }
}

/// Parse already-read `content` as the CLI `format`. The content-based
/// parser the `get` and `check` actions use: reading the input once means
/// detection and parsing share the same bytes, so a piped stdin is consumed
/// only once. `CANONICAL` is the AST's 1:1 oracle grammar, `GRON` ungron.
///
/// `reports` receives what the language said: its first error and, when it
/// collects them, every error; its warnings on a clean parse.
pub fn parse_slice_as(format: Format, content: &[u8], reports: &mut Reports) -> Result<Document> {
    match format {
        Format::CANONICAL => {
            if CANONICAL_ENABLED {
                canonical::parse(content)
            } else {
                Err(err!(FormatDisabled))
            }
        }
        // gron ("ungron") reconstructs the AST from its `path = value` lines,
        // reusing the JSON parser for each RHS.
        Format::GRON => gron::parse_document(content),
        _ => {
            let e = types::runtime_entry(format).ok_or(err!(FormatDisabled))?;
            let mut parser = Parser::new();
            reports.label = e.name.clone();
            let r = parser.parse(content, &e);
            reports.errors.append(&mut parser.errors);
            reports.warnings.append(&mut parser.warnings);
            match r {
                Ok(doc) => Ok(doc),
                Err(error) => {
                    if error.is("RuntimeParseFailed") {
                        // The language's own first error where it left one, with
                        // its span and label; else the message and offset it gave.
                        reports.diag = Some(match reports.errors.first() {
                            Some(d) => d.clone(),
                            None => ParseDiagnostic {
                                offset: parser.last_offset,
                                end: None,
                                message: parser.last_message().to_vec(),
                                short_label: e.name.as_bytes().to_vec(),
                            },
                        });
                    }
                    Err(error)
                }
            }
        }
    }
}

/// Whether `f`'s language declares a reference layer — anchors, aliases,
/// merges, tags — as `Caps.references`. gron and canonical are projections
/// of the core AST and carry none.
pub fn carries_references(f: Format) -> bool {
    match types::runtime_entry(f) {
        Some(e) => e.language.caps.references,
        None => false,
    }
}

/// `doc.ast` as the printer for `to` must see it: collapsed by `materialize`
/// when it leaves a language that carries a reference layer for one that
/// does not, and untouched otherwise — a YAML→YAML conversion keeps its
/// anchors, a JSON→anything one has none to collapse. Neither side is named:
/// each is asked (`carries_references`), so a runtime twin of YAML answers as
/// YAML does. `mode` is the tag policy — lax keeps unknown tags, strict
/// refuses them.
///
/// A document the pass refuses — a custom tag under strict mode, a tag its
/// value does not fit, an alias inside its own anchor — is reported to `term`
/// against `file` and the process exits 1, the status the unreported error
/// had. The report points at the node: the document was parsed from the
/// whole of `file`, so its spans are offsets into what `file` shows the user.
pub fn materialize_for<'d>(
    term: &mut Terminal,
    from: Format,
    to: Format,
    doc: &'d Document,
    file: &str,
    mode: TagMode,
) -> Result<Cow<'d, AST>> {
    if !carries_references(from) || carries_references(to) {
        return Ok(Cow::Borrowed(&doc.ast));
    }
    let mut culprit: Option<NodeId> = None;
    match materialize::materialize_reporting(&doc.ast, mode, &mut culprit) {
        Ok(m) => Ok(Cow::Owned(m)),
        Err(e) if e.is("UnknownTag") || e.is("TagTypeMismatch") || e.is("AliasCycle") => {
            diag_report::report_materialize_error(term, e, doc, culprit, file, &types::name(to))
        }
        Err(e) => Err(e),
    }
}

/// `materialize::materialize`, as a new tree.
pub fn materialize(ast: &AST, mode: TagMode) -> Result<AST> {
    materialize::materialize(ast, mode)
}

/// The lossless-envelope declaration of a CLI output format: what the
/// `$fig` envelope pass encodes for. gron's value layer is JSON, so it takes
/// JSON's; a registered target's is its entry's `caps.lossless`; canonical
/// takes none.
pub fn native_for_format(to: Format) -> Option<NativeKinds> {
    if to == Format::GRON {
        return registry::global().entry_by_name("json").and_then(|e| e.language.caps.lossless);
    }
    let e = types::runtime_entry(to)?;
    e.language.caps.lossless
}

/// Print `ast` from `node_id` in the registered language `e` — the tail of
/// `get`, `fmt` and `convert`: the lossy strips, read off the entry's own
/// declarations (`caps.lossless` for the null strip, `max_mapping_depth` for
/// the flat strip — both, when it declares both), then the vtable's `print`.
/// `report_loss` warns about what the strips drop, from the same declarations.
pub fn print_runtime(
    out: &mut Vec<u8>,
    e: &Entry,
    ast: &AST,
    node_id: NodeId,
    serialize: &SerializeOptions,
    lossless: bool,
) -> Result<()> {
    if !e.language.caps.serialize {
        return Err(err!(FormatNotSerializable));
    }
    let caps = e.language.caps;
    let null_strip: Option<NativeKinds> = if !lossless {
        match caps.lossless {
            Some(native) => {
                if native.null {
                    None
                } else {
                    Some(native)
                }
            }
            None => None,
        }
    } else {
        None
    };
    let depth: Option<usize> = if !lossless { caps.max_mapping_depth.map(|d| d as usize) } else { None };

    let mut tree: Option<AST> = None;
    let mut root = node_id;
    if let Some(native) = null_strip {
        let Some(a) = lossless::lossy_strip(ast, root, native)?.ast else { return Ok(()) };
        root = a.root;
        tree = Some(a);
    }
    if let Some(d) = depth {
        let src = tree.as_ref().unwrap_or(ast);
        let Some(a) = flat_strip::lossy_strip(src, root, d)?.ast else { return Ok(()) };
        root = a.root;
        tree = Some(a);
    }
    let _ = root;
    // A strip re-roots the tree at `node_id`, so what it returns prints whole.
    match &tree {
        None => registry::print_node_with(e, ast, node_id, serialize, out),
        Some(t) => registry::print_with(e, t, serialize, out),
    }
}

/// Warn on stderr about everything printing `ast` from `node_id` to `target`
/// would lose — comments dropped or degraded, values dropped or degraded —
/// unless `quiet`; with `strict`, any such loss exits 1. Run on the AST as it
/// will be printed: under `lossless` the lossy nodes are already enveloped,
/// so no value warnings fire. The shared tail of `get`, `convert` and `fmt`,
/// for a compiled target and a runtime one alike.
#[allow(clippy::too_many_arguments)]
pub fn report_loss(
    term: &mut Terminal,
    ast: &AST,
    node_id: NodeId,
    target: &Target,
    serialize: &SerializeOptions,
    lossless: bool,
    quiet: bool,
    strict: bool,
) -> Result<()> {
    if quiet && !strict {
        return Ok(());
    }
    let warnings = diagnostics::analyze(
        ast,
        node_id,
        target,
        &diagnostics::Options { pretty: serialize.pretty, strip_comments: serialize.strip_comments, lossless, depth: 0 },
    );
    // The CLI only surfaces losses the FORMAT forced. A loss the user
    // explicitly asked for (e.g. `--strip-comments`) carries
    // `ExplicitOption` and is not surprising, so it neither warns nor
    // trips `--strict` — it just rides through on the warning layer for a
    // library consumer that wants it.
    let mut surfaced = 0usize;
    for w in &warnings {
        if w.cause != Cause::FormatLimitation {
            continue;
        }
        surfaced += 1;
        if !quiet {
            term.set_color(Color::Yellow)?;
            term.write_str("warning: ")?;
            term.set_color(Color::Reset)?;
            let mut text = Vec::new();
            w.render(&mut text, target.name());
            term.write_all(&text)?;
            term.write_byte(b'\n')?;
        }
    }
    if !quiet {
        term.flush()?;
    }
    if strict && surfaced > 0 {
        term.print(format_args!("error: {surfaced} lossy conversion warning(s); --strict aborts.\n"))?;
        term.flush()?;
        fileio::exit(1);
    }
    Ok(())
}

/// The order the CLI sniffs content in: strictest grammar first, since
/// input valid as more than one format means the earliest. JSON before
/// JSON5, which reads every JSON document; ZON and plist, whose openers no
/// other format shares; TOML before figl and INI, which read most `a = 1`
/// files; dotenv before YAML, which reads nearly anything; `.properties`
/// and NestedText last, which read every line as something. JSONC is never
/// sniffed: JSON5 reads every JSONC document.
pub const SNIFF_ORDER: [&str; 11] =
    ["json", "json5", "zon", "plist", "toml", "figl", "ini", "dotenv", "yaml", "properties", "nestedtext"];

/// The extensions of host documents whose contents a sniff would misread.
fn document_kind(ext: &str) -> Option<&'static str> {
    const KINDS: [(&str, &str); 4] =
        [("md", "a Markdown"), ("markdown", "a Markdown"), ("html", "an HTML"), ("htm", "an HTML")];
    KINDS.iter().find(|k| ext.eq_ignore_ascii_case(k.0)).map(|k| k.1)
}

/// Sniff `content` with `Language.detect` and return what it inferred — the
/// fallback when neither `--input` nor the file extension pinned the format.
/// Errors (after a clear message) if nothing matches.
///
/// A successful guess is logged at DEBUG level only, so a release build says
/// nothing: `cat f | fig get - a` is a pipeline, and a note on stderr for
/// every sniffed stdin is noise in it — as silent as a file whose extension
/// resolved it. The guess matters only when it is wrong, and then the parse
/// fails and says so.
pub fn resolve_format_from_content(content: &[u8], file_path: &str) -> Result<Format> {
    // A file whose extension belongs to a language fig ships but this build
    // leaves out is that language's, not whatever a sniff would make of it.
    let ext = file_path.rfind('.').map(|d| &file_path[d + 1..]);
    if let Some(ext) = ext {
        if let Some(owner) = languages::unbundled_owner(ext) {
            crate::log_err!(
                "`{file_path}` is a {owner} file, and this build does not include {owner}; configure a language for it in languages.figl (see `fig lang --help`)"
            );
            return Err(err!(UnsupportedFileFormat));
        }
        // A document is not sniffed: its frontmatter would read as a config
        // file and an edit would land in the prose. A language configured for
        // the extension has already claimed it before this point.
        if let Some(kind) = document_kind(ext) {
            crate::log_err!(
                "`{file_path}` is {kind} document, not a config file; fig does not edit embedded blocks (pass an explicit format to read the whole file as one)"
            );
            return Err(err!(UnsupportedFileFormat));
        }
    }
    // A name in the order this build does not bundle is tried through the
    // language configured under it, which is registered only when asked for.
    for n in SNIFF_ORDER {
        if registry::global().entry_by_name(n).is_none() {
            let _ = languages::resolve_name(n);
        }
    }
    let Some(e) = registry::global().detect_among(content, &SNIFF_ORDER) else {
        crate::log_err!("could not infer the format of `{file_path}` from its contents; pass an explicit format");
        return Err(err!(UnsupportedFileFormat));
    };
    Ok(types::runtime_format(&e))
}

/// Open `file_path` read-only and sniff its contents. For the in-place edit paths
/// (`replace`/`rename`/`comment`), which then re-open the file read-write to splice it — so
/// detection reads through a separate handle and never disturbs the edit read.
pub fn detect_file_format(file_path: &str) -> Result<Format> {
    let mut probe = fileio::get_input(Some(file_path), OpenMode::ReadOnly)?;
    let content = fileio::read_all(&mut probe)?;
    resolve_format_from_content(&content, file_path)
}

/// Validate that `file` parses cleanly, returning the resolved format on success.
/// Format precedence mirrors `get`: an explicit `--input` `override_`, else the
/// file extension, else sniffing the contents. `spec_str` (from `--spec`) pins
/// the language version to validate against and is resolved once the format is
/// known — an unknown/inapplicable version is reported like a parse error.
/// A parse failure fills `diag_errors` with every diagnostic, and a clean parse
/// may fill `diag_warnings` the same way (both read against `diag_source`,
/// which is set alongside them).
pub fn check_one(
    file: &str,
    override_: Option<Format>,
    spec_str: Option<&str>,
    diag_source: &mut Option<Vec<u8>>,
    diag_errors: &mut Option<Vec<ParseDiagnostic>>,
    diag_warnings: &mut Option<Vec<ParseDiagnostic>>,
) -> Result<Format> {
    let mut input = fileio::get_input(Some(file), OpenMode::ReadOnly)?;
    let content = fileio::read_all(&mut input)?;

    let format: Format = match override_ {
        Some(f) => f,
        None => match args::detect_language_from_file_ending(file) {
            Some(d) => d.format,
            None => resolve_format_from_content(&content, file)?,
        },
    };

    // Resolve `--spec` against the now-known format. This rejects a nonsense
    // version (e.g. `--spec 1.0` on a JSON file) before parsing.
    let dialect = resolve_spec(format, spec_str)?;

    // A parse error is all we need to validate: the parsed result is
    // discarded.
    let mut reports = Reports::default();
    let r = parse_slice_as(dialect, &content, &mut reports);
    *diag_source = Some(content);
    match r {
        Ok(_) => {
            *diag_warnings = reports.render_warnings();
            Ok(format)
        }
        Err(e) => {
            *diag_errors = reports.render_errors();
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_spec_selects_a_versions_dialect_row() {
        let Some(yaml) = args::parse_format_name("yaml") else { return };
        assert_eq!(yaml, resolve_spec(yaml, None).unwrap());
        assert_eq!("yaml-1.2.2", types::name(resolve_spec(yaml, Some("1.2.2")).unwrap()));
        assert_eq!("yaml-1.1", types::name(resolve_spec(yaml, Some("1.1")).unwrap()));
        assert_eq!("yaml-1.1", types::name(resolve_spec(args::parse_format_name("yml").unwrap(), Some("1.1")).unwrap()));
        assert_eq!(err!(UnsupportedSpec), resolve_spec(yaml, Some("1.3")).unwrap_err());
        assert_eq!(err!(UnsupportedSpec), resolve_spec(Format::CANONICAL, Some("1.1")).unwrap_err());
    }

    #[test]
    fn resolve_spec_selects_toml_1_0_which_refuses_what_1_1_added() {
        let Some(toml) = args::parse_format_name("toml") else { return };
        let v1_0 = resolve_spec(toml, Some("1.0")).unwrap();
        assert_eq!("toml-1.0", types::name(v1_0));
        assert_eq!(err!(UnsupportedSpec), resolve_spec(toml, Some("0.5")).unwrap_err());
        let src = b"t = { a = 1, }\n";
        let mut reports = Reports::default();
        assert_eq!(err!(RuntimeParseFailed), parse_slice_as(v1_0, src, &mut reports).unwrap_err());
        parse_slice_as(toml, src, &mut reports).unwrap();
    }
}
