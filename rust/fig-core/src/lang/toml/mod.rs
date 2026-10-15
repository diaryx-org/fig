//! Port of languages/toml/src/toml.zig.
//!
//! TOML 1.0 and 1.1 (draft): `Toml`, the `Language` fig reads, edits and
//! writes TOML through.
//!
//! Editing has no hooks. Every edit is the generic engine's, driven by
//! `syntax` and by what `parser.rs` records: a table's header lines
//! (`Document::regions_of`, which the whole-container delete, move and
//! reorder gather) and every place a table's name is spelled
//! (`Document::mentions_of`). The engine keeps an inserted entry inside
//! the intended table by skipping children that sit under a header of
//! their own, spells a new `[a.b]` or `[[a.b]]` from `section_header`,
//! and renames a table by rewriting each mention — its own header,
//! every descendant header sharing the prefix, every dotted line. The
//! line-splice ops refuse a section node in this format's vocabulary
//! (`CannotDeleteSection`, …) and point at the whole-container ops.

pub mod tokenizer;
pub mod parser;
pub mod printer;

use crate::kit::ast::{NodeId, SerializeOptions, AST};
use crate::kit::language::{
    Caps, Comments, Dialect, DialectInfo, Language, NativeKinds, Prints, RenderRequest, SectionHeader, SpecName,
    SpliceStyle, Syntax,
};
use crate::kit::table::{self, Table};
use crate::kit::{convert, language};
use crate::{err, Result};

/// Zig's `toml.Type`: the dialect enum, in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    /// TOML 1.0.0 (stable, finalized).
    Toml1_0,
    /// TOML 1.1.0 (draft): newlines + trailing commas in inline tables,
    /// seconds-optional times, `\e` and `\xHH` string escapes.
    Toml1_1,
}

impl Type {
    /// The `Dialect` ordinal.
    pub fn ordinal(self) -> Dialect {
        self as Dialect
    }

    /// The type a `Dialect` ordinal names, or `None` past the enum.
    pub fn of(d: Dialect) -> Option<Type> {
        match d {
            0 => Some(Type::Toml1_0),
            1 => Some(Type::Toml1_1),
            _ => None,
        }
    }
}

/// Zig's `Language.default_type`.
pub const DEFAULT_TYPE: Type = Type::Toml1_1;

/// The four datetimes and `inf`/`nan` floats are native; `null` is not —
/// TOML is the one typed format without one, which makes a `null` the one
/// value the lossy path drops outright rather than degrades. Enum/char
/// literals and plist's date/data are enveloped.
const LOSSLESS: NativeKinds = NativeKinds {
    offset_datetime: true,
    local_datetime: true,
    local_date: true,
    local_time: true,
    number_special: true,
    ..NativeKinds::NONE
};

const CAPS: Caps = Caps { read: true, edit: true, serialize: true, lossless: Some(LOSSLESS), ..Caps::DEFAULT };

/// What fig's contract harness round-trips and edits: a root key, a
/// `[table]` (the section shape the parser records regions for), an array,
/// and an inline table.
const SAMPLES: &[&[u8]] = &[b"a = 1\nb = [1, 2]\n\n[s]\nk = \"v\"\nt = { x = 1 }\n"];

const SPECS: &[SpecName] = &[
    SpecName { name: "1.0", dialect: 0 },
    SpecName { name: "1.0.0", dialect: 0 },
    SpecName { name: "1.1", dialect: 1 },
    SpecName { name: "1.1.0", dialect: 1 },
];

const DIALECTS: &[DialectInfo] = &[DialectInfo {
    dialect: DEFAULT_TYPE as Dialect,
    specs: SPECS,
    prints: Some(Prints { char_as: "integer", ..Prints::new(LOSSLESS) }),
    ..DialectInfo::new("toml", SpliceStyle::Literal, Some(b""))
}];

/// The TOML language.
#[derive(Debug, Clone, Copy, Default)]
pub struct Toml;

pub static LANGUAGE: Toml = Toml;

/// The dialect `d` names, or `FormatDisabled` (the contract's "no such
/// dialect").
fn type_of(d: Dialect) -> Result<Type> {
    Type::of(d).ok_or(err!(FormatDisabled))
}

/// 1.0 and 1.1 differ in what the PARSER accepts (newlines and trailing
/// commas in inline tables, seconds-optional times, `\e`/`\xHH` escapes),
/// not in what the editor writes, so both dialects answer identically.
pub fn syntax(_t: Type) -> Syntax {
    Syntax {
        // `key = value`, in a block table and an inline one alike; an
        // empty inline table takes its first entry padded, `{ k = v }`.
        flow_map_pad: b" ",
        empty_seq_literal: Some(b"[]"),
        // A non-flow TOML sequence is an array-of-tables (use
        // `appendTableToArray`), and TOML has no block scalar array — so
        // the generic block-sequence edits refuse with `NotAnInlineArray`.
        block_seq_editable: false,
        // A section format: a `[table]`, `[[array]]` or dotted table is
        // assembled from scattered lines, which `parser.rs` records in
        // `Document::regions_of`. Its refusals say "table".
        section_noun: Some(b"table"),
        // `[a.b]` opens a table, `[[a.b]]` an element of an array of
        // tables; an index segment is implied and left out.
        section_header: Some(SectionHeader { seq_open: Some(b"[["), seq_close: Some(b"]]"), ..SectionHeader::new(b"[", b"]") }),
        // The dotted-key formats keep the flow `{}` chain as the
        // idiomatic intermediate form — `fig fmt` canonicalizes
        // `a = { b = { c = v }}` back to `a.b.c = v`.
        ..Syntax::new(Comments::HASH, Some(b" = "), Some(b"{}"))
    }
}

/// A key's name as TOML writes it: bare when it can be, else a basic
/// string — for an inserted key, a header path's segments and a renamed
/// table's leaf alike.
pub fn render_key_name(_t: Type, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()> {
    printer::write_key_name(out, r.key);
    Ok(())
}

/// A string replacing a string, in the old one's style where that style
/// can hold it: a literal `'…'` with no `'`, line break or control
/// character; a `'''…'''` within the multi-line rules; a `"""…"""`
/// always. Nothing for a basic `"…"`, which is the printer's own.
/// A multi-line string keeps a line break after its opener.
pub fn render_string(_t: Type, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()> {
    let old = r.old_value;
    if old.starts_with(b"'''") || old.starts_with(b"\"\"\"") {
        let after = &old[3..];
        let newline_first = after.starts_with(b"\n") || after.starts_with(b"\r\n");
        if old[0] == b'"' {
            printer::write_multiline_basic_string(out, r.value, newline_first);
        } else if printer::literal_holds(r.value, true) {
            printer::write_literal_string(out, r.value, true, newline_first);
        }
    } else if old.starts_with(b"'") && printer::literal_holds(r.value, false) {
        printer::write_literal_string(out, r.value, false, false);
    }
    Ok(())
}

impl Language for Toml {
    fn name(&self) -> &str {
        "toml"
    }

    fn extensions(&self) -> &[&'static str] {
        &["toml"]
    }

    fn caps(&self) -> Caps {
        CAPS
    }

    fn dialects(&self) -> &[DialectInfo] {
        DIALECTS
    }

    fn default_dialect(&self) -> Dialect {
        DEFAULT_TYPE.ordinal()
    }

    fn type_count(&self) -> usize {
        2
    }

    fn samples(&self) -> &[&'static [u8]] {
        SAMPLES
    }

    fn syntax(&self, dialect: Dialect) -> Syntax {
        syntax(Type::of(dialect).unwrap_or(DEFAULT_TYPE))
    }

    /// The contract's parse thunk for a parser that collects: every error
    /// the recovering parse found (or the one a single-shot parse stopped
    /// at) as the table's diagnostics, then its warnings on success.
    fn parse_table(&self, input: &[u8], dialect: Dialect, out: &mut Table) -> Result<()> {
        let t = type_of(dialect)?;
        let mut report = parser::Report::default();
        let parsed = parser::parse_collecting_into(out, input, t, &mut report);
        if let Err(e) = parsed {
            if report.errors.len() > 1 {
                for d in &report.errors {
                    language::add_diagnostic(out, table::DIAGNOSTIC_ERROR, d.offset, d.end, parser::describe(d.code).as_bytes(), parser::short_label(d.code).as_bytes())?;
                }
                return Err(e);
            }
            if let Some(d) = report.diag {
                language::add_diagnostic(out, table::DIAGNOSTIC_ERROR, d.offset, d.end, parser::describe(d.code).as_bytes(), parser::short_label(d.code).as_bytes())?;
            }
            return Err(e);
        }
        for w in &report.warnings {
            language::add_diagnostic(
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

    fn print(&self, dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        type_of(dialect)?;
        if options.splice {
            return printer::print_splice(out, ast, options);
        }
        printer::print(out, ast, options)
    }

    fn print_node(&self, dialect: Dialect, ast: &AST, node: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        type_of(dialect)?;
        if options.splice || options.strip_comments {
            // As the runtime registry prints one: the subtree's table, read
            // back and handed to `print` (which takes the splice printer).
            let mut t = convert::document_to_table(ast, node)?;
            if options.strip_comments {
                t.comments.clear();
            }
            let doc = convert::view_to_document(std::sync::Arc::from(&b""[..]), &t.view())?;
            return self.print(dialect, &doc.ast, options, out);
        }
        printer::print_node(out, ast, node, 0, options)
    }

    fn render_key_name(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        Some(type_of(dialect).and_then(|t| render_key_name(t, out, r)))
    }

    fn render_string(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        Some(type_of(dialect).and_then(|t| render_string(t, out, r)))
    }
}

/// `parse`, adopting the parser's table the way Zig's `Language.parse`
/// does (`Parser.parse`), for callers holding the `Type`.
pub fn parse(input: &[u8], t: Type) -> Result<crate::kit::document::Document> {
    parser::parse(input, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::language::ParseReport;

    #[test]
    fn the_language_validates_and_reads_its_sample() {
        language::validate(&LANGUAGE).unwrap();
        for s in SAMPLES {
            let doc = LANGUAGE.parse(s, LANGUAGE.default_dialect()).unwrap();
            let mut out = Vec::new();
            LANGUAGE.print(LANGUAGE.default_dialect(), &doc.ast, &SerializeOptions::default(), &mut out).unwrap();
            let mut report = ParseReport::default();
            LANGUAGE.parse_with_report(&out, 0, &mut report).unwrap();
        }
        let mut report = ParseReport::default();
        assert!(LANGUAGE.parse_with_report(b"a = \nb = 1\nc = \n", 1, &mut report).is_err());
        assert_eq!(2, report.errors.len());
    }

    #[test]
    fn render_string_keeps_the_old_quote_style_where_it_can() {
        let mut out = Vec::new();
        let r = RenderRequest { value: b"it's", old_value: b"'x'", ..RenderRequest::default() };
        LANGUAGE.render_string(1, &mut out, &r).unwrap().unwrap();
        assert!(out.is_empty());
        let r = RenderRequest { value: b"y", old_value: b"'x'", ..RenderRequest::default() };
        LANGUAGE.render_string(1, &mut out, &r).unwrap().unwrap();
        assert_eq!(b"'y'", &out[..]);
        out.clear();
        let r = RenderRequest { value: b"a\"b", old_value: b"\"\"\"\nx\"\"\"", ..RenderRequest::default() };
        LANGUAGE.render_string(1, &mut out, &r).unwrap().unwrap();
        assert_eq!(b"\"\"\"\na\\\"b\"\"\"", &out[..]);
        out.clear();
        let r = RenderRequest { key: b"a b", ..RenderRequest::default() };
        LANGUAGE.render_key_name(1, &mut out, &r).unwrap().unwrap();
        assert_eq!(b"\"a b\"", &out[..]);
    }
}
