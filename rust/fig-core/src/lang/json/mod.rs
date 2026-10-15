//! Port of languages/json/src/json.zig.
//!
//! fig's JSON language: one module serving three dialects, strict JSON,
//! JSONC (JSON with `//` and `/* */` comments) and JSON5. Its printer is
//! also what fig writes `-o json` with, from a tree of any format.

pub mod tokenizer;
pub mod parser;
pub mod printer;

use crate::kit::ast::{NodeId, SerializeOptions, AST};
use crate::kit::document::Document;
use crate::kit::json_string;
use crate::kit::language::{
    add_diagnostic, Caps, CommentDelimiter, Comments, Dialect, DialectInfo, Language, NativeKinds, Prints, PrintsComments,
    RenderRequest, SpliceStyle, Syntax,
};
use crate::kit::table::{self, Table};
use crate::{err, Result};

/// Zig's `json.Type`: the dialect a parse or print is in. Its ordinal is
/// the `Dialect` the `Language` trait passes.
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Type {
    #[default]
    JSON,
    JSONC,
    JSON5,
}

impl Type {
    /// The `Type` of ordinal `d`, or `None`.
    pub fn of(d: Dialect) -> Option<Type> {
        match d {
            0 => Some(Type::JSON),
            1 => Some(Type::JSONC),
            2 => Some(Type::JSON5),
            _ => None,
        }
    }

    /// The ordinal.
    pub fn dialect(self) -> Dialect {
        self as Dialect
    }
}

/// The JSON language (Zig's `json.Language`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Json;

pub static LANGUAGE: Json = Json;

/// Beyond the core kinds, JSON holds a `null` and nothing else: every
/// extended scalar rides in a `$fig` envelope. Declared for the strict
/// dialect and shared by JSONC/JSON5 — so JSON5's native `Infinity`/`NaN`
/// are enveloped too, which is conservative but still lossless.
const HOLDS_NULL: NativeKinds = NativeKinds { null: true, ..NativeKinds::NONE };

const CAPS: Caps = Caps { read: true, edit: true, serialize: true, lossless: Some(HOLDS_NULL), ..Caps::DEFAULT };

/// What fig's `src/contract/harness.zig` round-trips and edits: a mapping,
/// a sequence, and the scalar kinds the grammar has.
const SAMPLES: &[&[u8]] = &[b"{\"a\": 1, \"b\": [true, null, \"s\"], \"c\": {\"d\": 2.5}}\n", b"[]\n"];

const EXTENSIONS: &[&str] = &["json", "jsonc", "json5"];

const fn prints(holds: NativeKinds, number_special_as: &'static str, comments: PrintsComments) -> Prints {
    Prints { char_as: "number", number_special_as, comments, ..Prints::new(holds) }
}

/// The three user-facing dialects this one module serves. The only language
/// with more than one: strictness is the format NAME here (json/jsonc/json5
/// select a `Type` each), where TOML and YAML select theirs with `--spec`.
/// Registry order within the language is this order.
const DIALECTS: &[DialectInfo] = &[
    DialectInfo {
        dialect: Type::JSON as Dialect,
        prints: Some(prints(HOLDS_NULL, "quoted string", PrintsComments::Never)),
        ..DialectInfo::new("json", SpliceStyle::Escaped, Some(b"{}\n"))
    },
    DialectInfo {
        dialect: Type::JSONC as Dialect,
        prints: Some(prints(HOLDS_NULL, "quoted string", PrintsComments::Pretty)),
        print_name: "printc",
        print_node_name: "printNodec",
        ..DialectInfo::new("jsonc", SpliceStyle::Escaped, Some(b"{}\n"))
    },
    DialectInfo {
        dialect: Type::JSON5 as Dialect,
        prints: Some(prints(
            NativeKinds { null: true, number_special: true, ..NativeKinds::NONE },
            "string",
            PrintsComments::Pretty,
        )),
        print_name: "print5",
        print_node_name: "printNode5",
        ..DialectInfo::new("json5", SpliceStyle::Escaped, Some(b"{}\n"))
    },
];

fn type_of(dialect: Dialect) -> Result<Type> {
    Type::of(dialect).ok_or(err!(UnknownDialect))
}

impl Json {
    /// The one manifest in tree whose answer genuinely varies by dialect,
    /// and therefore the reason `syntax` is a function of `Type` at all:
    /// strict JSON has no comment syntax, JSONC and JSON5 have `//` lines
    /// and `/* */` blocks. The editor's splices are reparsed under the
    /// dialect it is holding, so writing `//` into a document being read as
    /// strict JSON would produce source that no longer parses — the comment
    /// ops return `CommentsUnsupported` there instead.
    pub fn syntax_of(t: Type) -> Syntax {
        let marker = if t == Type::JSON { None } else { Some(CommentDelimiter::open(b"//")) };
        let block = if t == Type::JSON { None } else { Some(CommentDelimiter::pair(b"/*", b"*/")) };
        Syntax {
            empty_seq_literal: Some(b"[]"),
            ..Syntax::new(Comments { line: marker, trailing: marker, block }, Some(b": "), Some(b"{}"))
        }
    }

    /// Zig's `parseTable`: write `input`'s rows into `out`, the table fig
    /// hands a parse.
    pub fn parse_table_in(input: &[u8], format: Type, out: &mut Table) -> Result<()> {
        parser::parse_into(out, input, format)
    }

    /// Zig's `parse`: the document fig would adopt from a parse.
    pub fn parse_in(input: &[u8], format: Type) -> Result<Document> {
        parser::parse(input, format)
    }

    /// Zig's `print`, `printc` or `print5`: the document printer of `t`.
    pub fn print_in(t: Type, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        match t {
            Type::JSON => printer::print(out, ast, options),
            Type::JSONC => printer::printc(out, ast, options),
            Type::JSON5 => printer::print5(out, ast, options),
        }
    }

    /// Zig's `printNode`, `printNodec` or `printNode5`: the subtree rooted at
    /// `id`, at `depth`, with no trailing newline.
    pub fn print_node_in(t: Type, ast: &AST, id: NodeId, depth: usize, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        match t {
            Type::JSON => printer::print_node(out, ast, id, depth, options),
            Type::JSONC => printer::print_nodec(out, ast, id, depth, options),
            Type::JSON5 => printer::print_node5(out, ast, id, depth, options),
        }
    }
}

impl Language for Json {
    fn name(&self) -> &str {
        "json"
    }

    fn extensions(&self) -> &[&'static str] {
        EXTENSIONS
    }

    fn caps(&self) -> Caps {
        CAPS
    }

    fn dialects(&self) -> &[DialectInfo] {
        DIALECTS
    }

    fn default_dialect(&self) -> Dialect {
        Type::JSON as Dialect
    }

    fn type_count(&self) -> usize {
        3
    }

    fn samples(&self) -> &[&'static [u8]] {
        SAMPLES
    }

    fn syntax(&self, dialect: Dialect) -> Syntax {
        Json::syntax_of(Type::of(dialect).unwrap_or_default())
    }

    /// As `kit/contract.zig`'s parse thunk calls the parser: it recovers
    /// past each error (`parse_collecting_into`), and every error, or the
    /// warnings of a clean parse, go into the table's diagnostics.
    fn parse_table(&self, input: &[u8], dialect: Dialect, out: &mut Table) -> Result<()> {
        let t = type_of(dialect)?;
        let mut report = parser::Report::default();
        // `input` is the table's source; slice the table's own copy so the
        // in-source texts stay there.
        let source = out.source_arc();
        let src: &[u8] = if &source[..] == input { &source } else { input };
        let parsed = parser::parse_collecting_into(out, src, t, &mut report);
        if let Err(e) = parsed {
            let errors: Vec<parser::Diagnostic> =
                if report.errors.len() > 1 { report.errors.clone() } else { report.diag.into_iter().collect() };
            for d in errors {
                add_diagnostic(
                    out,
                    table::DIAGNOSTIC_ERROR,
                    d.offset,
                    d.end,
                    parser::describe(d.code).as_bytes(),
                    parser::short_label(d.code).as_bytes(),
                )?;
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

    /// The dialect's document printer (`print`, `printc`, `print5`): JSON
    /// has no splice or fragment printer, so `kit/contract.zig`'s
    /// `printWith` always reaches this one.
    fn print(&self, dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        let t = Type::of(dialect).ok_or(err!(FormatDisabled))?;
        Json::print_in(t, ast, options, out)
    }

    /// A key's name as JSON writes it: quoted and escaped (`b` -> `"b"`),
    /// which strict JSON requires and JSONC and JSON5 take.
    fn render_key_name(&self, _dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        json_string::write_quoted(out, r.key);
        Some(Ok(()))
    }

    /// A string replacing a JSON5 `'…'` string, in single quotes, which hold
    /// anything: a `'` is escaped. Nothing for a `"…"` string, the printer's
    /// own, nor in strict JSON or JSONC, which have no other.
    fn render_string(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        if Type::of(dialect) != Some(Type::JSON5) || !r.old_value.starts_with(b"'") {
            return Some(Ok(()));
        }
        json_string::write_quoted_in(out, r.value, b'\'');
        Some(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::Spans;
    use crate::kit::language::{validate, Renderer};
    use std::sync::Arc;

    // What the TypeScript binding does with a language module's parse: copies
    // its table's lists into the table fig handed it.
    #[test]
    fn a_modules_table_copied_byte_for_byte_into_a_fresh_one_is_adopted() {
        let src: Arc<[u8]> = Arc::from(&b"{\"a\": 1, \"b\": [true, null, \"s\"]}"[..]);
        let mut a = Table::init(src.clone());
        LANGUAGE.parse_table(&src, Type::JSON as Dialect, &mut a).unwrap();
        let mut b = Table::init(src.clone());
        b.rows.extend_from_slice(&a.rows);
        b.strings.extend_from_slice(&a.strings);
        b.seps.extend_from_slice(&a.seps);
        b.comments.extend_from_slice(&a.comments);
        AST::adopt(src, &mut b, Spans::Required).unwrap();
    }

    #[test]
    fn the_language_validates_and_declares_its_renderers() {
        validate(&LANGUAGE).unwrap();
        assert!(LANGUAGE.has_renderer(0, Renderer::KeyName));
        assert!(LANGUAGE.has_renderer(2, Renderer::String));
        assert!(!LANGUAGE.has_renderer(0, Renderer::Value));
        assert!(LANGUAGE.syntax(0).comments.line.is_none());
        assert_eq!(Some(CommentDelimiter::open(b"//")), LANGUAGE.syntax(1).comments.trailing);

        let mut out = Vec::new();
        let r = RenderRequest { value: b"it's", old_value: b"'x'", ..Default::default() };
        LANGUAGE.render_string(2, &mut out, &r).unwrap().unwrap();
        assert_eq!(b"'it\\'s'", &out[..]);
        out.clear();
        LANGUAGE.render_string(1, &mut out, &r).unwrap().unwrap();
        assert!(out.is_empty());
        LANGUAGE.render_key_name(0, &mut out, &RenderRequest { key: b"b", ..Default::default() }).unwrap().unwrap();
        assert_eq!(b"\"b\"", &out[..]);
    }

    #[test]
    fn samples_round_trip_through_every_dialect() {
        for sample in LANGUAGE.samples() {
            for d in 0..3 {
                let doc = LANGUAGE.parse(sample, d).unwrap();
                let mut out = Vec::new();
                LANGUAGE.print(d, &doc.ast, &SerializeOptions::default(), &mut out).unwrap();
                let again = LANGUAGE.parse(&out, d).unwrap();
                assert!(doc.ast.eql(&again.ast));
            }
        }
    }
}
