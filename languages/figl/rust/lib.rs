//! Figl, fig's authoring dialect for [fig](https://docs.rs/fig): fig's own parser,
//! printer and editing rules for the language, in pure Rust (this crate's modules, a
//! port of the Zig in `src/`), registered with fig's core, which carries
//! no language until a crate like this one registers it. Typed values go to and
//! from it as they do with `serde_json`:
//!
//! ```ignore
//! let config: Config = fig_figl::from_str(input)?;
//! let text = fig_figl::to_string(&config)?;
//! ```
//!
//! and [`format()`] is its format, for everything else fig does
//! (`fig::Document::parse(input, fig_figl::format()?)`). Neither needs it
//! registered first: the first call does that.
//!
//! It registers `figl`, found by name with [`fig::Format::by_name`].
//!
//! With its default `fig` feature off (`default-features = false`), the
//! crate is the language alone, on `fig-kit`: [`LANGUAGE`] and its modules,
//! with no `fig` binding, for a program that registers it with `fig-core`
//! itself, as fig's CLI does.
//!
//! # The language
//!
//! Port of languages/figl/src/figl.zig.
//!
//! The fig authoring dialect: a memorable, typeable, whitespace-insensitive
//! surface over the AST (`kit::ast`), parsed by `fig fmt` into the same tree
//! the lossless `canonical` form encodes. It is NOT the oracle — it is allowed
//! to be lossy at the edges (the canonical form and `$fig-envelope` are the
//! faithful fallback). See `languages/figl/DESIGN.md` for the full spec.
//!
//! Single grammar (no versions to select), so `Type` has one member, mirroring
//! ZON's `Type = enum { ZON }` pattern.

pub mod tokenizer;
pub mod parser;
pub mod printer;
/// The renderers: how this language spells what the editor splices.
pub mod edit;

use fig_kit::ast::{NodeKind, SerializeOptions, AST};
use fig_kit::language::{
    add_diagnostic, Caps, Comments, Dialect, DialectInfo, Language, Prints, RenderRequest, SpliceStyle, Syntax,
};
use fig_kit::manifest::HOLDS_EVERYTHING;
use fig_kit::table::{self, Table};
use fig_kit::Result;

/// figl's dialect enum (Zig's `Type`): one member, ordinal 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    Fig,
}

impl Type {
    /// The `Type` a `Dialect` ordinal names (every ordinal is `Fig`: there is
    /// one).
    pub fn of(d: Dialect) -> Type {
        let _ = d;
        Type::Fig
    }
}

/// The figl language: Zig's `figl.Language`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Figl;

pub static LANGUAGE: Figl = Figl;

/// `.figl` is the authoring dialect's extension. (The canonical form
/// deliberately owns no extension — select it with `--input canonical`.)
const EXTENSIONS: &[&str] = &["figl"];

/// What fig's contract harness round-trips and edits: a root key, a block
/// container (the section shape the parser records regions for) with a `>`
/// child, and a list.
const SAMPLES: &[&[u8]] = &[b"a = 1\ndatabase\n> host = localhost\n> port = 5432\n"];

/// The native authoring dialect (`DESIGN.md`): read, written and edited by
/// every surface.
const DIALECTS: &[DialectInfo] = &[DialectInfo {
    prints: Some(Prints { root_container: true, ..Prints::new(HOLDS_EVERYTHING) }),
    // A whole figl document must be a container; a value need not be.
    print_fragment_name: Some("printFragment"),
    ..DialectInfo::new("figl", SpliceStyle::Literal, Some(b""))
}];

impl Figl {
    /// Zig `Language.syntax(t)`.
    pub const fn syntax_of(_: Type) -> Syntax {
        Syntax {
            // `key = value`. A flow object is `=`-mode or `:`-mode
            // (JSON-embedded) and may not mix, so an insert into a non-empty
            // one copies the separators it already uses. An `=`-mode one is
            // compact, as `fig fmt` writes it — `{x=1 y=2}`, unpadded — and
            // an empty one takes that form.
            flow_kv_sep_from_siblings: true,
            flow_map_kv_sep: b"=",
            flow_map_entry_sep: b" ",
            // A dotted-key format: the flow chain is the idiomatic
            // intermediate form, and `fig fmt` canonicalizes it back.
            empty_seq_literal: Some(b"[]"),
            // The `>` marker run that opens a line is section depth, not
            // whitespace — a comment inserted above a line must repeat it or
            // it detaches. See `Syntax::structural_indent`.
            structural_indent: true,
            // One nesting level is one marker cell; an element line is the
            // `>` run of its container plus `* `. Both are copied by the
            // engine's prefix-bytes policy, so a glued `>>*` file stays glued.
            indent_unit: b"> ",
            seq_item_marker: b"* ",
            // A line may open with its `>` depth run and a `+` line's mark
            // before the node it holds.
            line_prefix_marks: b">+",
            // A section format: a block container may be re-entered and
            // scattered, and `parser.rs` records every block container's
            // header lines in `Document::regions_of`. Its refusals say
            // "container".
            section_noun: Some(b"container"),
            // A `'''`/`"""` string's opener line holds nothing but the
            // delimiter and its comment (spec § 4.6), and that is where the
            // comment lives: `x = ''' # note`; so does a flow list or object
            // over several lines whose `[`/`{` line holds no element
            // (`xs = [ # note`). See `Syntax::opener_line_comment`.
            opener_line_comment: true,
            ..Syntax::new(Comments::HASH, Some(b" = "), Some(b"{}"))
        }
    }
}

impl Language for Figl {
    fn name(&self) -> &str {
        "figl"
    }

    fn extensions(&self) -> &[&'static str] {
        EXTENSIONS
    }

    fn caps(&self) -> Caps {
        Caps { read: true, edit: true, serialize: true, ..Caps::DEFAULT }
    }

    fn dialects(&self) -> &[DialectInfo] {
        DIALECTS
    }

    fn type_count(&self) -> usize {
        1
    }

    fn samples(&self) -> &[&'static [u8]] {
        SAMPLES
    }

    fn syntax(&self, dialect: Dialect) -> Syntax {
        Figl::syntax_of(Type::of(dialect))
    }

    /// Write `input`'s rows into `out`, the table fig hands a parse, as
    /// `kit/contract.zig`'s `parse` thunk does for a parser with a report:
    /// the recovering parse (`parse_collecting_into`), every error it found
    /// into the table's diagnostics on a refusal, its warnings on success.
    fn parse_table(&self, input: &[u8], dialect: Dialect, out: &mut Table) -> Result<()> {
        let mut report = parser::Report::default();
        let parsed = parser::parse_collecting_into(out, input, Type::of(dialect), &mut report);
        if let Err(e) = parsed {
            if report.errors.len() > 1 {
                for d in &report.errors {
                    add_diagnostic(out, table::DIAGNOSTIC_ERROR, d.offset, d.end, parser::describe(d.code).as_bytes(), parser::short_label(d.code).as_bytes())?;
                }
                return Err(e);
            }
            if let Some(d) = report.diag {
                add_diagnostic(out, table::DIAGNOSTIC_ERROR, d.offset, d.end, parser::describe(d.code).as_bytes(), parser::short_label(d.code).as_bytes())?;
            }
            return Err(e);
        }
        for w in &report.warnings {
            add_diagnostic(
                out,
                table::DIAGNOSTIC_WARNING,
                w.offset,
                w.end,
                parser::Warning::describe_warning(w.code).as_bytes(),
                parser::Warning::short_label(w.code).as_bytes(),
            )?;
        }
        Ok(())
    }

    /// `contract.zig`'s `printWith` for figl: the fragment printer for a value
    /// the editor splices (`splice`, and `flow`, which only a fragment reads)
    /// or a root a whole document cannot have; else the document printer.
    ///
    /// The thunk rebuilds the options from the C ABI's `PrintOptions`, which
    /// has no `indent_requested`, so through it the cosmetic indent is never
    /// asked for (`fig fmt --indent` leaves a figl file unindented); call
    /// `printer::print` directly for it.
    fn print(&self, dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        let _ = dialect;
        let opts = SerializeOptions { indent_requested: false, ..*options };
        let scalar_root = !matches!(ast.kind_of(ast.root), NodeKind::Mapping(_) | NodeKind::Sequence(_));
        if opts.splice || opts.flow || scalar_root {
            return printer::print_fragment(out, ast, opts);
        }
        printer::print(out, ast, opts)
    }

    // `print_node` is the trait's default, as fig's core prints a node of any
    // language (`runtime.printNodeWith`: the subtree's table, handed to
    // `print`). Zig's `Language.printNode` is `printer.printNode`, which
    // takes a marker depth the trait has no room for; it is
    // `printer::print_node` here, which `edit::render_tail` calls.

    /// ` = value` for an inline value; for a block map or sequence value,
    /// which has no inline `key = <block>` spelling — section headers, `> `
    /// and `* ` lines only parse standalone — a newline and the value
    /// re-printed as a nested section one marker level below the key. The
    /// document root takes the value as written.
    fn render_tail(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        Some(edit::render_tail(Type::of(dialect), out, r))
    }

    // No delete guard: the engine's section rule covers it. Every block
    // container is a section node (`Document::regions_of`), and a section
    // node cannot be line-spliced. A block container's VALUE is still
    // replaceable: its entry records its `=`, so the engine reframes it
    // through `render_tail` rather than splicing the section's span.
    //
    // Whole-container ops are all generic: no fig code at all.
}

// ── the `fig` binding (its default `fig` feature) ──────────────────────────

/// Register figl, fig's authoring dialect, and return the format of its first dialect.
/// Registering it again returns the format it already has.
#[cfg(feature = "fig")]
pub fn register() -> core::result::Result<fig::Format, fig::Error> {
    fig::language::register_core(&LANGUAGE)
}

#[cfg(feature = "fig")]
fig::__language_crate_functions!("figl");

#[cfg(test)]
mod tests {
    use super::*;
    use fig_kit::language::validate;

    #[test]
    fn the_language_validates_and_parses_its_sample() {
        let l: &dyn Language = &LANGUAGE;
        validate(l).unwrap();
        assert!(l.has_renderer(0, fig_kit::language::Renderer::Tail));
        assert!(!l.has_renderer(0, fig_kit::language::Renderer::Value));
        let doc = l.parse(SAMPLES[0], 0).unwrap();
        let mut out = Vec::new();
        l.print(0, &doc.ast, &SerializeOptions::default(), &mut out).unwrap();
        assert_eq!(&b"a = 1\ndatabase = {host=localhost port=5432}\n"[..], &out[..]);
    }

    #[test]
    fn parse_with_report_carries_errors_and_warnings() {
        let l: &dyn Language = &LANGUAGE;
        let mut report = fig_kit::language::ParseReport::default();
        let e = l.parse_with_report(b"good = 1\nbad1: int = hello\nkey: value\n", 0, &mut report).unwrap_err();
        assert_eq!(fig_kit::err!(FigTypeMismatch), e);
        assert_eq!(2, report.errors.len());
        assert_eq!(b"type mismatch", &report.errors[0].short_label[..]);

        let mut report = fig_kit::language::ParseReport::default();
        l.parse_with_report(b"norway = Yes\n", 0, &mut report).unwrap();
        assert_eq!(1, report.warnings.len());
        assert_eq!(9, report.warnings[0].offset);
        assert_eq!(Some(12), report.warnings[0].end);
    }

    #[test]
    fn render_tail_reframes_a_block_value_and_splices_an_inline_one() {
        let l: &dyn Language = &LANGUAGE;
        let mut out = Vec::new();
        let r = RenderRequest { indent: b"> ", key: b"k", value: b"a = 1\nb\n> c = 2\n", ..RenderRequest::default() };
        l.render_tail(0, &mut out, &r).unwrap().unwrap();
        assert_eq!(&b"\n> > a = 1\n> > b.c = 2"[..], &out[..]);
        out.clear();
        let r = RenderRequest { key: b"k", value: b"[1, 2]", ..RenderRequest::default() };
        l.render_tail(0, &mut out, &r).unwrap().unwrap();
        assert_eq!(&b" = [1, 2]"[..], &out[..]);
        out.clear();
        let r = RenderRequest { value: b"x = 1\n", ..RenderRequest::default() };
        l.render_tail(0, &mut out, &r).unwrap().unwrap();
        assert_eq!(&b"x = 1\n"[..], &out[..]);
    }
}
