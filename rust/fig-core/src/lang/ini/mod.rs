//! Port of languages/ini/src/ini.zig.

pub mod tokenizer;
pub mod parser;
pub mod printer;

use crate::kit::ast::{NodeId, SerializeOptions, AST};
use crate::kit::language::{
    self, Caps, CommentDelimiter, Comments, Dialect, DialectInfo, Language, Prints, SpliceStyle, Syntax,
};
use crate::kit::manifest::NativeKinds;
use crate::kit::table::{self, Table};
use crate::{err, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Type {
    /// The one dialect this parser accepts: `=`-separated `key = value`
    /// lines, `[section]` headers, `;`/`#` full-line comments. See
    /// `tokenizer.rs`'s module doc for exactly what's deliberately excluded
    /// from the many incompatible things "INI" means in the wild.
    #[default]
    Ini,
}

impl Type {
    /// The `Dialect` ordinal this is (Zig's enum order).
    pub fn dialect(self) -> Dialect {
        self as Dialect
    }

    /// The `Type` a `Dialect` ordinal names, if any.
    pub fn of(d: Dialect) -> Option<Type> {
        match d {
            0 => Some(Type::Ini),
            _ => None,
        }
    }
}

/// The INI language (Zig's `ini.Language`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Ini;

pub static LANGUAGE: Ini = Ini;

/// Untyped scalars: the grammar carries no type information, so
/// `port = 8080` reads back as the STRING "8080".
const DIALECTS: &[DialectInfo] = &[DialectInfo {
    prints: Some(Prints { typed_scalars: false, char_as: "number", ..Prints::new(NativeKinds::NONE) }),
    ..DialectInfo::new("ini", SpliceStyle::Raw, Some(b""))
}];

/// What fig's contract harness round-trips and edits: a root key and a
/// `[section]` (the shape the parser records regions for).
const SAMPLES: &[&[u8]] = &[b"a = 1\n\n[s]\nk = v\n"];

fn type_of(dialect: Dialect) -> Result<Type> {
    Type::of(dialect).ok_or(err!(FormatDisabled))
}

impl Language for Ini {
    fn name(&self) -> &str {
        "ini"
    }

    fn extensions(&self) -> &[&'static str] {
        &["ini"]
    }

    /// A root mapping and one level of `[section]` mappings; a sequence at
    /// any depth, or a mapping nested two or more levels deep, has no INI
    /// spelling (`printer.rs` hard-errors on both rather than degrading).
    fn caps(&self) -> Caps {
        Caps { read: true, edit: true, serialize: true, max_mapping_depth: Some(1), ..Caps::DEFAULT }
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
            // No literal spelling for an empty nested mapping — `{}` in INI
            // is the two-character STRING `{}`, not a container — so `set`
            // cannot auto-vivify a missing ancestor here at all
            // (`empty_map_literal` None).
            //
            // No `{…}`/`[…]` spelling at all: a `[` opening the file is the
            // first section's header, which the flow sniff would misread.
            flow_containers: false,
            // A section format: a `[section]` may be reopened and so
            // scattered, and `parser.rs` records every section's header
            // lines in `Document::regions_of`. Its refusals say "section".
            section_noun: Some(b"section"),
            ..Syntax::new(
                // The printer accepts a leading `#` on read but always
                // WRITES `;`, so `;` is what the editor's inserts and scans
                // use. No same-line trailing comment syntax at all: a `;`/`#`
                // after a value on the SAME line is literal value text (see
                // `parser.rs`, "a value runs to end of line"). Splicing one
                // in would corrupt the value on reread, so trailing ops are
                // refused.
                Comments { line: Some(CommentDelimiter::open(b";")), trailing: None, block: None },
                Some(b" = "),
                None,
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

    /// The dialect's splice printer for a value the editor splices, else its
    /// document printer (`kit/contract.zig`'s `printWith`).
    fn print(&self, dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        type_of(dialect)?;
        if options.splice {
            return printer::print_splice(out, ast, options);
        }
        printer::print(out, ast, options)
    }

    fn print_node(&self, dialect: Dialect, ast: &AST, node: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        type_of(dialect)?;
        printer::print_node(out, ast, node, 0, options)
    }

    // ── Editing ─────────────────────────────────────────────────────────────
    //
    // No hooks. Every edit is the generic engine's, driven by `syntax` above:
    // `insertKey` used to be hooked for one reason — to skip the flow sniff
    // on a root whose first byte is a `[section]`'s bracket — and that is
    // now `flow_containers = false` plus the engine's own rule that a
    // section format's root is block (`editor.Editor.isFlowNode`).
    //
    // No delete/replace/move/reorder guards either: the engine's section rule
    // covers them. A `[section]` is a section node (`Document::regions_of`)
    // — its span is anchored at its first header's name token alone — and a
    // section node cannot be line-spliced: `deleteKey`, `replaceValAtPath`,
    // `moveKey` and `reorderKeys` refuse it (`CannotDeleteSection`, …) and
    // point at the whole-container ops.
    //
    // ── Whole-container ops ─────────────────────────────────────────────────
    //
    // All generic (see `editor.Editor`'s block of the same name). A reopened
    // `[section]` is scattered through the file exactly as a TOML table or
    // fig container is, and `deleteContainer`/`moveContainer`/
    // `reorderContainers` derive its regions — every header occurrence plus
    // its entries — from `Document::regions_of`, with no INI code at all. No
    // `insertContainer` (INI cannot auto-vivify at all —
    // `syntax().empty_map_literal` is None) and no `renameContainer` (a
    // header's key is one tight span the generic `replaceKeyAtPath`
    // rewrites, with no dotted descendants to follow).
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
            assert_eq!(*s, &out[..]);
        }
    }

    #[test]
    fn a_refusal_and_a_warning_reach_the_report() {
        let mut report = ParseReport::default();
        assert_eq!(err!(TrailingContent), LANGUAGE.parse_with_report(b"[a] x\n", 0, &mut report).unwrap_err());
        assert_eq!(1, report.errors.len());
        let mut report = ParseReport::default();
        LANGUAGE.parse_with_report(b"a = 1\na = 2\n", 0, &mut report).unwrap();
        assert_eq!(1, report.warnings.len());
        assert_eq!(b"duplicate key", &report.warnings[0].short_label[..]);
    }
}
