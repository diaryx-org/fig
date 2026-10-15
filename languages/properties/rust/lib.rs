//! Java `.properties` for [fig](https://docs.rs/fig): fig's own parser,
//! printer and editing rules for the language, in pure Rust (this crate's modules, a
//! port of the Zig in `src/`), registered with fig's core, which carries
//! no language until a crate like this one registers it. Typed values go to and
//! from it as they do with `serde_json`:
//!
//! ```ignore
//! let config: Config = fig_properties::from_str(input)?;
//! let text = fig_properties::to_string(&config)?;
//! ```
//!
//! and [`format()`] is its format, for everything else fig does
//! (`fig::Document::parse(input, fig_properties::format()?)`). Neither needs it
//! registered first: the first call does that.
//!
//! It registers `properties`, found by name with [`fig::Format::by_name`].
//!
//! With its default `fig` feature off (`default-features = false`), the
//! crate is the language alone, on `fig-kit`: [`LANGUAGE`] and its modules,
//! with no `fig` binding, for a program that registers it with `fig-core`
//! itself, as fig's CLI does.
//!
//! # The language
//!
//! Port of languages/properties/src/properties.zig.

pub mod tokenizer;
pub mod parser;
pub mod printer;

use fig_kit::ast::{NodeId, SerializeOptions, AST};
use fig_kit::language::{
    self, Caps, CommentDelimiter, Comments, Dialect, DialectInfo, Language, Prints, SpliceStyle, Syntax,
};
use fig_kit::manifest::NativeKinds;
use fig_kit::table::{self, Table};
use fig_kit::{err, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Type {
    /// The one dialect this parser accepts — see `tokenizer.rs`'s module doc
    /// for the full grammar (three interchangeable separators, backslash
    /// escapes on both key and value, line continuation, `#`/`!` comments).
    #[default]
    Properties,
}

impl Type {
    /// The `Dialect` ordinal this is (Zig's enum order).
    pub fn dialect(self) -> Dialect {
        self as Dialect
    }

    /// The `Type` a `Dialect` ordinal names, if any.
    pub fn of(d: Dialect) -> Option<Type> {
        match d {
            0 => Some(Type::Properties),
            _ => None,
        }
    }
}

/// The Java `.properties` language (Zig's `properties.Language`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Properties;

pub static LANGUAGE: Properties = Properties;

/// Flat and untyped, the same representational limits as dotenv.
const DIALECTS: &[DialectInfo] = &[DialectInfo {
    prints: Some(Prints { typed_scalars: false, char_as: "number", ..Prints::new(NativeKinds::NONE) }),
    ..DialectInfo::new("properties", SpliceStyle::Raw, Some(b""))
}];

/// What fig's contract harness round-trips and edits: the three separators
/// the grammar accepts.
const SAMPLES: &[&[u8]] = &[b"a=1\nb: two\nc three\n"];

fn type_of(dialect: Dialect) -> Result<Type> {
    Type::of(dialect).ok_or(err!(FormatDisabled))
}

impl Language for Properties {
    fn name(&self) -> &str {
        "properties"
    }

    fn extensions(&self) -> &[&'static str] {
        &["properties"]
    }

    /// Flat, like dotenv: as much a `Hashtable<String, String>` as dotenv is
    /// a flat environment map.
    fn caps(&self) -> Caps {
        Caps { read: true, edit: true, serialize: true, max_mapping_depth: Some(0), ..Caps::DEFAULT }
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
        let _ = dialect;
        Syntax {
            flow_containers: false,
            // A `\` before a line ending carries the line on
            // (`tokenizer.rs`), so a line written after `k=v\` would be read
            // as the rest of `k`'s value.
            line_continuation: Some(b'\\'),
            ..Syntax::new(
                // The grammar accepts `#` and `!` as comment leaders; the
                // printer writes `#`, so that is what the editor scans for
                // and inserts. As in INI, there is no trailing comment: a `#`
                // after a value is value text (`parser.rs`), so a spliced one
                // would change the value on reread, and trailing ops are
                // refused.
                Comments { line: Some(CommentDelimiter::open(b"#")), trailing: None, block: None },
                // Three separators are legal on read (`=`, `:`, space); the
                // printer always writes a bare `=`.
                Some(b"="),
                // Flat, same reasoning as dotenv.
                Some(b"{}"),
            )
        }
    }

    fn parse_table(&self, input: &[u8], dialect: Dialect, out: &mut Table) -> Result<()> {
        let t = type_of(dialect)?;
        let mut report = parser::Report::default();
        let parsed = parser::parse_collecting_into(out, input, t, &mut report);
        if let Err(e) = parsed {
            if report.errors.len() > 1 {
                for d in &report.errors {
                    let (m, l) = (parser::describe(d.code), parser::short_label(d.code));
                    language::add_diagnostic(out, table::DIAGNOSTIC_ERROR, d.offset, d.end, m.as_bytes(), l.as_bytes())?;
                }
            } else if let Some(d) = report.diag {
                let (m, l) = (parser::describe(d.code), parser::short_label(d.code));
                language::add_diagnostic(out, table::DIAGNOSTIC_ERROR, d.offset, d.end, m.as_bytes(), l.as_bytes())?;
            }
            return Err(e);
        }
        for w in &report.warnings {
            let (m, l) = (parser::Warning::describe_warning(w.code), parser::Warning::short_label(w.code));
            language::add_diagnostic(out, table::DIAGNOSTIC_WARNING, w.offset, w.end, m.as_bytes(), l.as_bytes())?;
        }
        Ok(())
    }

    /// The dialect's document printer (`kit/contract.zig`'s `printWith`:
    /// `.properties` has no splice or fragment printer).
    fn print(&self, dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        type_of(dialect)?;
        printer::print(out, ast, options)
    }

    fn print_node(&self, dialect: Dialect, ast: &AST, node: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        type_of(dialect)?;
        printer::print_node(out, ast, node, 0, options)
    }
}

// ── the `fig` binding (its default `fig` feature) ──────────────────────────

/// Register Java `.properties`, and return the format of its first dialect.
/// Registering it again returns the format it already has.
#[cfg(feature = "fig")]
pub fn register() -> core::result::Result<fig::Format, fig::Error> {
    fig::language::register_core(&LANGUAGE)
}

#[cfg(feature = "fig")]
fig::__language_crate_functions!("properties");

#[cfg(test)]
mod tests {
    use super::*;
    use fig_kit::language::ParseReport;

    #[test]
    fn the_language_validates_and_reads_its_samples() {
        language::validate(&LANGUAGE).unwrap();
        for s in LANGUAGE.samples() {
            let doc = LANGUAGE.parse(s, 0).unwrap();
            let mut out = Vec::new();
            LANGUAGE.print(0, &doc.ast, &SerializeOptions::default(), &mut out).unwrap();
            assert_eq!(&b"a=1\nb=two\nc=three\n"[..], &out[..]);
        }
    }

    #[test]
    fn a_tokenizer_refusal_reaches_the_report() {
        let mut report = ParseReport::default();
        assert_eq!(err!(UnclosedEscape), LANGUAGE.parse_with_report(b"a=b\\", 0, &mut report).unwrap_err());
        assert_eq!(1, report.errors.len());
        assert_eq!(3, report.errors[0].offset);
    }
}
