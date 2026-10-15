//! Port of languages/nestedtext/src/nestedtext.zig.

pub mod tokenizer;
pub mod parser;
pub mod printer;
pub mod edit;

use crate::kit::ast::{SerializeOptions, AST};
use crate::kit::language::{
    add_diagnostic, Caps, CommentDelimiter, Comments, Dialect, DialectInfo, Language, NativeKinds, Prints,
    RenderRequest, SpliceStyle, Syntax,
};
use crate::kit::table::{self, Table};
use crate::Result;

/// The NestedText dialects (Zig's `Type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    /// NestedText (https://nestedtext.org) — the one dialect this parser
    /// accepts (the format has no versioned spec the way TOML does; recent
    /// releases (3.x) haven't changed the on-disk grammar this reads).
    NestedText,
}

impl Type {
    /// The `Type` a `Dialect` ordinal names.
    pub fn of(d: Dialect) -> Option<Type> {
        match d {
            0 => Some(Type::NestedText),
            _ => None,
        }
    }
}

/// The NestedText language (Zig's `Language`).
#[derive(Debug, Clone, Copy, Default)]
pub struct NestedText;

pub static LANGUAGE: NestedText = NestedText;

/// What fig's contract harness round-trips and edits: a dict with a nested
/// list.
const SAMPLES: &[&[u8]] = &[b"a: 1\nb:\n  - x\n  - y\n"];

/// Nested (dict/list) but deliberately untyped — every leaf is a string.
const DIALECTS: &[DialectInfo] = &[DialectInfo {
    prints: Some(Prints { typed_scalars: false, null_at_root: true, char_as: "number", ..Prints::new(NativeKinds::NONE) }),
    ..DialectInfo::new("nestedtext", SpliceStyle::Raw, Some(b""))
}];

impl NestedText {
    fn ty(dialect: Dialect) -> Type {
        Type::of(dialect).unwrap_or(Type::NestedText)
    }
}

impl Language for NestedText {
    fn name(&self) -> &str {
        "nestedtext"
    }

    fn extensions(&self) -> &[&'static str] {
        &["nt"]
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

    fn syntax(&self, _: Dialect) -> Syntax {
        Syntax {
            // Four-space nesting; an item is `- value`.
            indent_unit: b"    ",
            seq_item_marker: b"- ",
            ..Syntax::new(
                // Joins INI: a `#` after a value on the SAME line is literal
                // rest-of-line value text, not a comment (see `parser.rs`,
                // "rest-of-line values are 100% literal"). A trailing comment
                // can only ever be its own `#` line immediately after the
                // entry.
                Comments { line: Some(CommentDelimiter::open(b"#")), trailing: None, block: None },
                // The inline dict form, `{a: 1, b: 2}`, is what the generic
                // flow-entry insert writes; a block entry goes through
                // `render_entry`.
                Some(b": "),
                // No literal spelling for an empty nested dict — every value
                // is either rest-of-line text or a nested block — so `set`
                // cannot auto-vivify a missing ancestor.
                None,
            )
        }
    }

    /// Write `input`'s rows into `out`, the table fig hands a parse. On a
    /// refusal the report's diagnostic goes into the table (the parse thunk
    /// of `kit/contract.zig` for a parser with `parseWithReportInto`).
    fn parse_table(&self, _input: &[u8], dialect: Dialect, out: &mut Table) -> Result<()> {
        let mut report = parser::Report::default();
        let parsed = parser::parse_with_report_into(out, Self::ty(dialect), &mut report);
        if let (Err(_), Some(d)) = (&parsed, report.diag) {
            add_diagnostic(out, table::DIAGNOSTIC_ERROR, d.offset, d.end, parser::describe(d.code), parser::short_label(d.code))?;
        }
        parsed
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
    // No hooks. Four renderers say how NestedText spells an entry, an item,
    // what follows a key, and a renamed key; every operation is the generic
    // engine's. The logic lives in `edit.rs`.

    /// A value is framed either rest-of-line or as a nested `>`-block, on a
    /// four-space nesting convention, and a key that cannot be spelled plain
    /// takes the `: key` multiline form — none of which `key`, `kv_sep`,
    /// value on one line can write.
    fn render_entry(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        Some(edit::render_entry(Self::ty(dialect), out, r))
    }

    /// The item twin: `- value`, or a bare `-` over a nested `>`-block for
    /// an empty or multi-line value.
    fn render_item(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        Some(edit::render_item(Self::ty(dialect), out, r))
    }

    /// A value is always a raw scalar (this format has no typed or quoted
    /// literal syntax to splice verbatim) RENDERED same-line or as a nested
    /// `>`-block per its shape, so a replacement is reframed from the key —
    /// `:` and the tail after a plain key, the tail alone after a multiline
    /// `: key`, which has no separator — and the document root is a `>`
    /// block at column 0. The parser records each plain key's `:` so the
    /// engine knows to reframe.
    fn render_tail(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        Some(edit::render_tail(Self::ty(dialect), out, r))
    }

    /// A plain key's span excludes its trailing `:` while a multiline key
    /// has no separator colon at all and starts at its line's indent, so a
    /// rename spells the new key in whichever form it needs, adding or
    /// dropping the colon — and refuses plain-to-multiline, which would also
    /// have to move a same-line value.
    fn render_key(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        Some(edit::render_key(Self::ty(dialect), out, r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::err;
    use crate::kit::language::{validate, ParseReport, Renderer};

    #[test]
    fn the_language_validates_and_round_trips_its_samples() {
        let l: &dyn Language = &LANGUAGE;
        validate(l).unwrap();
        for which in [Renderer::Entry, Renderer::Item, Renderer::Tail, Renderer::Key] {
            assert!(l.has_renderer(0, which));
        }
        assert!(!l.has_renderer(0, Renderer::Value));
        for s in l.samples() {
            let doc = l.parse(s, 0).unwrap();
            let mut out = Vec::new();
            l.print(0, &doc.ast, &SerializeOptions::default(), &mut out).unwrap();
            let again = l.parse(&out, 0).unwrap();
            assert!(doc.ast.eql(&again.ast));
        }
    }

    #[test]
    fn a_refusal_reports_where_and_why() {
        let mut report = ParseReport::default();
        let e = LANGUAGE.parse_with_report(b"a: 1\na: 2\n", 0, &mut report).unwrap_err();
        assert_eq!(err!(DuplicateKey), e);
        assert_eq!(1, report.errors.len());
        assert_eq!(b"duplicate key", &report.errors[0].short_label[..]);
    }
}
