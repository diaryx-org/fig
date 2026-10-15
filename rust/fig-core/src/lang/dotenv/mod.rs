//! Port of languages/dotenv/src/dotenv.zig.

pub mod tokenizer;
pub mod parser;
pub mod printer;

use crate::kit::ast::{NodeId, SerializeOptions, AST};
use crate::kit::language::{self, Caps, Comments, Dialect, DialectInfo, Language, Prints, SpliceStyle, Syntax};
use crate::kit::manifest::NativeKinds;
use crate::kit::table::{self, Table};
use crate::{err, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Type {
    /// The one dialect this parser accepts — see `tokenizer.rs`'s module doc
    /// for exactly what it does and doesn't accept (bash-identifier keys,
    /// optional `export`, real `"`/`'` quoting, no `$VAR` interpolation).
    #[default]
    Dotenv,
}

impl Type {
    /// The `Dialect` ordinal this is (Zig's enum order).
    pub fn dialect(self) -> Dialect {
        self as Dialect
    }

    /// The `Type` a `Dialect` ordinal names, if any.
    pub fn of(d: Dialect) -> Option<Type> {
        match d {
            0 => Some(Type::Dotenv),
            _ => None,
        }
    }
}

/// The dotenv language (Zig's `dotenv.Language`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Dotenv;

pub static LANGUAGE: Dotenv = Dotenv;

/// A flat string map and nothing more: no nesting, untyped scalars. A
/// nested value tree cannot be represented, and serializing one warns.
const DIALECTS: &[DialectInfo] = &[DialectInfo {
    prints: Some(Prints { typed_scalars: false, char_as: "number", ..Prints::new(NativeKinds::NONE) }),
    ..DialectInfo::new("dotenv", SpliceStyle::Raw, Some(b""))
}];

/// What fig's contract harness round-trips and edits: bare and quoted
/// values.
const SAMPLES: &[&[u8]] = &[b"A=1\nB=\"two words\"\n"];

fn type_of(dialect: Dialect) -> Result<Type> {
    Type::of(dialect).ok_or(err!(FormatDisabled))
}

impl Language for Dotenv {
    fn name(&self) -> &str {
        "dotenv"
    }

    /// A dotenv file is conventionally named exactly `.env`, whose last-dot
    /// "extension" is the literal `env`. (`.env.production` is not
    /// recognized by extension — pass `--input dotenv`.)
    fn extensions(&self) -> &[&'static str] {
        &["env"]
    }

    /// Flat: even a depth-1 mapping (INI's `[section]`) has no dotenv
    /// spelling, since dotenv has no nesting concept at all.
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
            // Flat `KEY=value` lines; `{` is value text, never a container.
            flow_containers: false,
            ..Syntax::new(
                Comments::HASH,
                // A bare `=` with no surrounding spaces — see `printer.rs`.
                Some(b"="),
                // Flat: no nesting to vivify into, but `{}` is still the
                // literal the generic seed would splice, and the format has
                // no nested path for it to seed. Kept as the shared default
                // rather than None, which would change the error a nested
                // `set` reports.
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
    /// dotenv has no splice or fragment printer).
    fn print(&self, dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        type_of(dialect)?;
        printer::print(out, ast, options)
    }

    fn print_node(&self, dialect: Dialect, ast: &AST, node: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        type_of(dialect)?;
        printer::print_node(out, ast, node, 0, options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::language::ParseReport;

    #[test]
    fn the_language_validates_and_round_trips_its_samples() {
        language::validate(&LANGUAGE).unwrap();
        for s in LANGUAGE.samples() {
            let doc = LANGUAGE.parse(s, 0).unwrap();
            let mut out = Vec::new();
            LANGUAGE.print(0, &doc.ast, &SerializeOptions::default(), &mut out).unwrap();
            assert_eq!(&b"A=1\nB=two words\n"[..], &out[..]);
        }
    }

    #[test]
    fn every_recovered_error_reaches_the_report() {
        let mut report = ParseReport::default();
        assert_eq!(err!(MissingEquals), LANGUAGE.parse_with_report(b"a\nB=1\nc\n", 0, &mut report).unwrap_err());
        assert_eq!(2, report.errors.len());
        assert_eq!(b"missing `=`", &report.errors[1].short_label[..]);
    }
}
