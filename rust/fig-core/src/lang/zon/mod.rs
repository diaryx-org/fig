//! Port of languages/zon/src/zon.zig.
//!
//! ZON leans on the Zig standard library's tokenizer and parser, which the
//! Zig build imports and the Rust port carries alongside (`zig_tokenizer`,
//! `zig_parse`, `zig_string_literal`; see `parser.rs`).

pub mod parser;
pub mod printer;
pub mod zig_parse;
pub mod zig_string_literal;
pub mod zig_tokenizer;

use crate::kit::ast::{SerializeOptions, AST};
use crate::kit::language::{
    Caps, Comments, Dialect, DialectInfo, Language, NativeKinds, Prints, PrintsComments, RenderRequest, SpliceStyle,
    Syntax,
};
use crate::kit::table::Table;
use crate::Result;

/// The ZON dialects (Zig's `Type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    /// ZON as accepted by the Zig 0.16 `std.zig` parser. There is no
    /// versioned ZON spec; the grammar tracks whatever the pinned compiler
    /// accepts.
    Zon,
}

impl Type {
    /// The `Type` a `Dialect` ordinal names.
    pub fn of(d: Dialect) -> Option<Type> {
        match d {
            0 => Some(Type::Zon),
            _ => None,
        }
    }
}

/// The ZON language (Zig's `Language`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Zon;

pub static LANGUAGE: Zon = Zon;

/// `null`, `.enum_literal` and `'c'` are native Zig syntax; datetimes,
/// non-finite floats and plist's date/data are enveloped.
const LOSSLESS: NativeKinds = NativeKinds { null: true, enum_literal: true, char_literal: true, ..NativeKinds::NONE };

/// What fig's contract harness round-trips and edits: a struct literal with
/// a nested struct and a tuple.
const SAMPLES: &[&[u8]] = &[b".{ .a = 1, .b = .{ .c = \"d\" }, .e = .{ 1, 2 } }\n"];

const DIALECTS: &[DialectInfo] = &[DialectInfo {
    prints: Some(Prints { char_as: "number", comments: PrintsComments::Pretty, ..Prints::new(LOSSLESS) }),
    ..DialectInfo::new("zon", SpliceStyle::Literal, Some(b".{}\n"))
}];

impl Language for Zon {
    fn name(&self) -> &str {
        "zon"
    }

    fn extensions(&self) -> &[&'static str] {
        &["zon"]
    }

    fn caps(&self) -> Caps {
        Caps { read: true, edit: true, serialize: true, lossless: Some(LOSSLESS), ..Caps::DEFAULT }
    }

    fn dialects(&self) -> &[DialectInfo] {
        DIALECTS
    }

    fn samples(&self) -> &[&'static [u8]] {
        SAMPLES
    }

    /// The format that most exercises the manifest's "push traits down into
    /// parameters" rule: three of these fields exist because ZON's surface
    /// syntax differs from the block-mapping formats, not because ZON needs
    /// bespoke logic. Any future format whose keys carry a sigil, or which
    /// has no keyless top-level mapping, reuses them unchanged.
    fn syntax(&self, _: Dialect) -> Syntax {
        Syntax {
            // A struct field's key SPAN starts at the bare identifier, so the
            // leading `.` has to be absorbed explicitly on a flow-entry
            // delete or the splice strands it beside a survivor.
            key_sigil: Some(b'.'),
            // The same `.{}`: ZON's empty tuple is its empty struct.
            empty_seq_literal: Some(b".{}"),
            // No bare `key: value` document form (unlike YAML/JSON5, whose
            // root can be a keyless top-level mapping), so a null promotes
            // in place to `.{ key = value }` — root or nested, no
            // distinction.
            bare_document_mapping: false,
            flow_map_open: b".{",
            flow_map_close: b"}",
            ..Syntax::new(
                // ZON follows Zig: `//` lines, and no block comment.
                Comments::SLASHES,
                // Struct-field syntax.
                Some(b" = "),
                Some(b".{}"),
            )
        }
    }

    /// Write `input`'s rows into `out`, the table fig hands a parse.
    fn parse_table(&self, _input: &[u8], dialect: Dialect, out: &mut Table) -> Result<()> {
        parser::parse_into(out, Type::of(dialect).unwrap_or(Type::Zon))
    }

    fn print(&self, _dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        printer::print(out, ast, options)
    }

    /// A key's name as a struct field: `.name`, or `.@"has space"` when it
    /// is not a bare identifier.
    fn render_key_name(&self, _dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        printer::write_dot_name(out, r.key);
        Some(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::language::{validate, Renderer};

    #[test]
    fn the_language_validates_and_round_trips_its_samples() {
        let l: &dyn Language = &LANGUAGE;
        validate(l).unwrap();
        assert!(l.has_renderer(0, Renderer::KeyName));
        assert!(!l.has_renderer(0, Renderer::Value));
        for s in l.samples() {
            let doc = l.parse(s, 0).unwrap();
            let mut out = Vec::new();
            l.print(0, &doc.ast, &SerializeOptions::default(), &mut out).unwrap();
            let again = l.parse(&out, 0).unwrap();
            assert!(doc.ast.eql(&again.ast));
        }
        let mut out = Vec::new();
        l.render_key_name(0, &mut out, &RenderRequest { key: b"has space", ..Default::default() }).unwrap().unwrap();
        assert_eq!(b".@\"has space\"", &out[..]);
    }
}
