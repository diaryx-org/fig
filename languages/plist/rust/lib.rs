//! The Apple XML property list for [fig](https://docs.rs/fig): fig's own parser,
//! printer and editing rules for the language, in pure Rust (this crate's modules, a
//! port of the Zig in `src/`), registered with fig's core, which carries
//! no language until a crate like this one registers it. Typed values go to and
//! from it as they do with `serde_json`:
//!
//! ```ignore
//! let config: Config = fig_plist::from_str(input)?;
//! let text = fig_plist::to_string(&config)?;
//! ```
//!
//! and [`format()`] is its format, for everything else fig does
//! (`fig::Document::parse(input, fig_plist::format()?)`). Neither needs it
//! registered first: the first call does that.
//!
//! It registers `plist`, found by name with [`fig::Format::by_name`].
//!
//! With its default `fig` feature off (`default-features = false`), the
//! crate is the language alone, on `fig-kit`: [`LANGUAGE`] and its modules,
//! with no `fig` binding, for a program that registers it with `fig-core`
//! itself, as fig's CLI does.
//!
//! # The language
//!
//! Port of languages/plist/src/plist.zig.
//!
//! plist reads into the shared AST (so plist converts *into* JSON/YAML/TOML/
//! ZON/…) and writes back out via `printer` — the documented inverse mapping
//! described in `parser.rs`'s header (a `dict` is a real mapping, an `array`
//! a real sequence, `date`/`data` ride the `extended` scalar). It IS a
//! serialize target, so core's serializer routes here like every other
//! format. It also HAS an in-place (span-splicing) editor via `edit.rs`, so
//! `fig replace`/`rename`/`set`/`insert`/`delete`/`comment` work on a
//! `.plist`; unlike the line-oriented formats it renders typed value
//! elements (fig `sniffBare` typing) and uses `<!-- -->` comments. The
//! generic XML format (`.xml`) still has none — a document syntax whose
//! ambiguous edit surface (attributes vs text vs mixed content) is a
//! separate, deferred effort; plist works because its DTD gives every
//! element a fixed, unambiguous typed meaning.

pub mod parser;
pub mod printer;
pub mod edit;

use fig_kit::ast::{SerializeOptions, AST};
use fig_kit::language::{
    Caps, ClosedContainers, CommentDelimiter, Comments, Delimiters, Dialect, DialectInfo, Language, NativeKinds, Prints,
    PrintsComments, RenderRequest, SpliceStyle, Syntax,
};
use fig_kit::table::Table;
use fig_kit::Result;

/// The plist dialects (Zig's `Type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    /// Apple's XML property list — `<plist><dict>...</dict></plist>`, the
    /// only variant this reader/printer supports so far. Old-style ASCII
    /// (NeXTSTEP/OpenStep `{ key = value; }`) and binary plist (`bplist00…`)
    /// are separate, larger efforts (a different grammar, and for binary, a
    /// wholly different byte-level format) — see `parser.rs`'s module doc.
    Xml,
}

impl Type {
    /// The `Type` a `Dialect` ordinal names.
    pub fn of(d: Dialect) -> Option<Type> {
        match d {
            0 => Some(Type::Xml),
            _ => None,
        }
    }
}

/// The plist language (Zig's `Language`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Plist;

pub static LANGUAGE: Plist = Plist;

/// What fig's contract harness round-trips and edits: a dict with a string,
/// an integer and an array.
const SAMPLES: &[&[u8]] = &[
    b"<plist version=\"1.0\"><dict><key>a</key><string>b</string><key>n</key><integer>1</integer><key>l</key><array><true/><false/></array></dict></plist>\n",
];

/// Genuinely typed and nested (dict/array/string/integer/real/bool, with
/// date/data carried on the `extended` scalar) — the one XML-shaped format
/// that is also a full value model.
const DIALECTS: &[DialectInfo] = &[DialectInfo {
    prints: Some(Prints {
        comments: PrintsComments::Never,
        ..Prints::new(NativeKinds { plist_date: true, data: true, ..NativeKinds::NONE })
    }),
    // A bare `<dict>` IS a document this parser accepts (see its `detect`
    // probe), so `fig set` on a nonexistent `.plist` can create one.
    ..DialectInfo::new("plist", SpliceStyle::Raw, Some(b"<dict>\n</dict>\n"))
}];

const XML_COMMENT: CommentDelimiter = CommentDelimiter { open: b"<!--", close: b"-->", forbidden: Some(b"--") };

impl Plist {
    fn ty(dialect: Dialect) -> Type {
        Type::of(dialect).unwrap_or(Type::Xml)
    }
}

impl Language for Plist {
    fn name(&self) -> &str {
        "plist"
    }

    fn extensions(&self) -> &[&'static str] {
        &["plist"]
    }

    fn caps(&self) -> Caps {
        Caps { read: true, edit: true, serialize: true, ..Caps::DEFAULT }
    }

    fn dialects(&self) -> &[DialectInfo] {
        DIALECTS
    }

    fn samples(&self) -> &[&'static [u8]] {
        SAMPLES
    }

    /// plist declares the least of any editable format, because it delegates
    /// the most: an entry is a PAIR of sibling elements (`<key>k</key>` then
    /// a typed value element), not a `key<sep>value` line, so almost nothing
    /// in the line-oriented generic engine applies. What is declared here is
    /// what the line-based delete/remove paths — which do ride the generic
    /// code — actually consult.
    fn syntax(&self, _: Dialect) -> Syntax {
        Syntax {
            // A `<dict>`/`<array>` is an element, never an inline `{…}`, and
            // it closes itself, so a trailing comment on one follows
            // `</dict>` rather than riding the `<key>` line.
            flow_containers: false,
            closed_containers: Some(ClosedContainers {
                map: Delimiters { open: b"<dict>", close: b"</dict>" },
                seq: Delimiters { open: b"<array>", close: b"</array>" },
            }),
            // An item is a bare element on its own line.
            seq_item_marker: b"",
            // An empty array is an element too, which the closed-container
            // insert expands around its first item.
            empty_seq_literal: Some(b"<array/>"),
            ..Syntax::new(
                // A comment is the `<!-- … -->` PAIR, leading and trailing
                // alike, and XML forbids `--` inside one. The owned-block
                // scan recognizes an own-line pair; the leading and trailing
                // ops write and strip the pair; the dangling and comment-out
                // ops, which need a bare prefix, refuse it.
                Comments { line: Some(XML_COMMENT), trailing: Some(XML_COMMENT), block: None },
                // An entry is a PAIR OF SIBLING ELEMENTS, not a
                // `key<sep>value` line, so no generic path writes one. See
                // `Syntax.kv_sep`.
                None,
                // No bare literal for an empty dict that the generic seed
                // could splice — a value is always a typed wrapper element.
                None,
            )
        }
    }

    /// Write `input`'s rows into `out`, the table fig hands a parse.
    fn parse_table(&self, _input: &[u8], dialect: Dialect, out: &mut Table) -> Result<()> {
        parser::parse_into(out, Self::ty(dialect))
    }

    /// The dialect's printer: the splice printer for a value the editor
    /// splices, else the document printer.
    fn print(&self, _dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        if options.splice {
            return printer::print_splice(out, ast, options);
        }
        printer::print(out, ast, options)
    }

    // ── Renderers ───────────────────────────────────────────────────────
    //
    // No hooks. Everything structural about editing a plist is the generic
    // engine's, driven by `syntax` above; what the engine cannot know is how
    // this format SPELLS a value and an entry. A value is always a typed
    // element, never a bare literal, and an entry is a PAIR OF SIBLING
    // ELEMENTS (`<key>k</key>` then the value element) on two lines rather
    // than a `key<sep>value` line. The logic lives in `edit.rs`.

    /// A CLI value string rendered into a typed element: fig `sniffBare`
    /// typing, or spliced verbatim when it already looks like `<…>`. The
    /// engine passes every value it splices through this.
    fn render_value(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        Some(edit::render_value(Self::ty(dialect), out, r))
    }

    /// `<key>k</key>`, a newline, the indent, and the rendered value.
    fn render_entry(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        Some(edit::render_entry(Self::ty(dialect), out, r))
    }

    /// A renamed key, as `<key>k</key>`: the key's span is the whole element.
    fn render_key(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        Some(edit::render_key(Self::ty(dialect), out, r))
    }
}

// ── the `fig` binding (its default `fig` feature) ──────────────────────────

/// Register the Apple XML property list, and return the format of its first dialect.
/// Registering it again returns the format it already has.
#[cfg(feature = "fig")]
pub fn register() -> core::result::Result<fig::Format, fig::Error> {
    fig::language::register_core(&LANGUAGE)
}

#[cfg(feature = "fig")]
fig::__language_crate_functions!("plist");

#[cfg(test)]
mod tests {
    use super::*;
    use fig_kit::language::{validate, Renderer};

    #[test]
    fn the_language_validates_and_round_trips_its_samples() {
        let l: &dyn Language = &LANGUAGE;
        validate(l).unwrap();
        assert!(l.has_renderer(0, Renderer::Value));
        assert!(l.has_renderer(0, Renderer::Entry));
        assert!(l.has_renderer(0, Renderer::Key));
        assert!(!l.has_renderer(0, Renderer::Tail));
        for s in l.samples() {
            let doc = l.parse(s, 0).unwrap();
            let mut out = Vec::new();
            l.print(0, &doc.ast, &SerializeOptions::default(), &mut out).unwrap();
            let again = l.parse(&out, 0).unwrap();
            assert!(doc.ast.eql(&again.ast));
        }
    }
}
