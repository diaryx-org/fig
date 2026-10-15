//! YAML for [fig](https://docs.rs/fig): fig's own parser,
//! printer and editing rules for the language, in pure Rust (this crate's modules, a
//! port of the Zig in `src/`), registered with fig's core, which carries
//! no language until a crate like this one registers it. Typed values go to and
//! from it as they do with `serde_json`:
//!
//! ```ignore
//! let config: Config = fig_yaml::from_str(input)?;
//! let text = fig_yaml::to_string(&config)?;
//! ```
//!
//! and [`format()`] is its format, for everything else fig does
//! (`fig::Document::parse(input, fig_yaml::format()?)`). Neither needs it
//! registered first: the first call does that.
//!
//! It registers `yaml`, and each version as `yaml-<version>`
//! (`yaml-1.1`), each found by name with [`fig::Format::by_name`].
//!
//! With its default `fig` feature off (`default-features = false`), the
//! crate is the language alone, on `fig-kit`: [`LANGUAGE`] and its modules,
//! with no `fig` binding, for a program that registers it with `fig-core`
//! itself, as fig's CLI does.
//!
//! # The language
//!
//! Port of languages/yaml/src/yaml.zig.
//!
//! YAML 1.2.2 and its 1.1 spec variant: the tokenizer, the parser that writes
//! fig's node table, the block-style printer, and the multi-document stream
//! splitter.

pub mod tokenizer;
pub mod parser;
pub mod printer;
pub mod stream;

use fig_kit::ast::{SerializeOptions, AST};
use fig_kit::language::{
    Caps, Comments, Dialect, DialectInfo, Language, NativeKinds, Prints, RenderRequest, SpecName, SpliceStyle, StreamDocument, Syntax,
};
use fig_kit::table::Table;
use fig_kit::Result;

/// Zig's `yaml.Type`: the YAML version a parse resolves scalars by. The
/// ordinal is the trait's `Dialect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Type {
    V1_2_2 = 0,
    /// YAML 1.1 (2005). Differs from 1.2 almost entirely in *scalar type
    /// resolution* (the tag repository at yaml.org/type): `yes/no/on/off/y/n`
    /// booleans, leading-zero octal (`0777`) + binary (`0b…`) + sexagesimal
    /// (`190:20:30`) ints, `_` digit separators, mandatory-sign float exponents,
    /// and `!!timestamp` auto-resolution. Structure/syntax is unchanged.
    /// Resolution differences are pinned by fig-conformance's YAML 1.1 suite
    /// and implemented by `scalar_kind1_1` in `parser.rs`.
    V1_1 = 1,
}

impl Type {
    /// The `Type` a dialect ordinal names; an unknown one reads as the default.
    pub fn of(d: Dialect) -> Type {
        match d {
            1 => Type::V1_1,
            _ => Type::V1_2_2,
        }
    }
}

/// The language's default dialect.
pub const DEFAULT_TYPE: Type = Type::V1_2_2;

/// The core schema has a `null` and none of the extended scalars (a
/// `!!timestamp` is a 1.1 tag, not a core kind), so those ride in a `$fig`
/// envelope; a byte string is `!!binary`.
const LOSSLESS: NativeKinds = NativeKinds { null: true, data: true, ..NativeKinds::NONE };

const CAPS: Caps = Caps {
    read: true,
    edit: true,
    serialize: true,
    // Anchors, aliases, `<<` merges and tags: the layer `Materialize`
    // collapses when a document leaves YAML for a format without one.
    references: true,
    lossless: Some(LOSSLESS),
    max_mapping_depth: None,
};

const SPECS: &[SpecName] = &[
    SpecName { name: "1.2", dialect: Type::V1_2_2 as Dialect },
    SpecName { name: "1.2.2", dialect: Type::V1_2_2 as Dialect },
    SpecName { name: "1.1", dialect: Type::V1_1 as Dialect },
    SpecName { name: "1.1.0", dialect: Type::V1_1 as Dialect },
];

const DIALECTS: &[DialectInfo] = &[DialectInfo {
    prints: Some(Prints { char_as: "number", ..Prints::new(LOSSLESS) }),
    // A bare `key:` seed, not `{}`: see `Syntax::empty_map_literal`'s note on
    // why an empty YAML document is the empty string.
    print_name: "printWith",
    specs: SPECS,
    ..DialectInfo::new("yaml", SpliceStyle::Literal, Some(b""))
}];

/// What fig's contract harness round-trips and edits: block and flow shapes,
/// a sequence, a nested mapping.
const SAMPLES: &[&[u8]] = &[b"a: 1\nb:\n  - x\n  - y\nc:\n  d: true\n", b"{a: 1, b: [2, 3]}\n"];

/// 1.1 and 1.2.2 differ only in scalar type RESOLUTION, never in the syntax
/// the editor splices, so both dialects answer identically.
pub fn syntax(_t: Type) -> Syntax {
    Syntax {
        // `<<` merges another mapping's entries in; a key resolved only
        // through it is inherited (shadowed on write, refused on delete).
        merge_key: Some(b"<<"),
        // A flow `[]`: a null cannot become a block list as it becomes a block
        // mapping, so the list a `set` starts is a flow one.
        empty_seq_literal: Some(b"[]"),
        // The only editable format whose block mapping has a single-line
        // spelling (`k: v`) that reaches the splice paths.
        single_line_block_mapping: true,
        // The empty seed rather than `{}` — see `Syntax::empty_map_literal`.
        ..Syntax::new(Comments::HASH, Some(b": "), Some(b""))
    }
}

/// The YAML language (Zig's `yaml.Language`).
///
/// Editing: no hooks, and one renderer. YAML is the format the generic engine
/// was written against; its reference layer is core (the alias node kind,
/// `resolve_alias`, `merged_child`, the anchor and tag span tables), so
/// follow-mode replace and inherited-key detection are the engine's, the
/// latter switched on by `merge_key`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Yaml;

pub static LANGUAGE: Yaml = Yaml;

impl Language for Yaml {
    fn name(&self) -> &str {
        "yaml"
    }

    fn extensions(&self) -> &[&'static str] {
        &["yaml", "yml"]
    }

    fn caps(&self) -> Caps {
        CAPS
    }

    fn dialects(&self) -> &[DialectInfo] {
        DIALECTS
    }

    fn default_dialect(&self) -> Dialect {
        DEFAULT_TYPE as Dialect
    }

    fn type_count(&self) -> usize {
        2
    }

    fn samples(&self) -> &[&'static [u8]] {
        SAMPLES
    }

    fn syntax(&self, dialect: Dialect) -> Syntax {
        syntax(Type::of(dialect))
    }

    /// Write `input`'s rows into `out`, the table fig hands a parse; on a
    /// refusal, the parse's report as the table's diagnostic.
    fn parse_table(&self, _input: &[u8], dialect: Dialect, out: &mut Table) -> Result<()> {
        let mut report = parser::Report::default();
        let parsed = parser::parse_with_report_into(out, Type::of(dialect), &mut report);
        if parsed.is_err() {
            if let Some(d) = report.diag {
                fig_kit::language::add_diagnostic(
                    out,
                    fig_kit::table::DIAGNOSTIC_ERROR,
                    d.offset,
                    d.end,
                    parser::describe(d.code).as_bytes(),
                    parser::short_label(d.code).as_bytes(),
                )?;
            }
        }
        parsed
    }

    /// A `---`/`...` stream's documents, each to be parsed on its own.
    fn split_stream(&self, input: &[u8], _dialect: Dialect) -> Option<Result<Vec<StreamDocument>>> {
        Some(Ok(stream::locate(input)))
    }

    fn has_split_stream(&self) -> bool {
        true
    }

    fn print(&self, _dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        printer::print_with(out, ast, options);
        Ok(())
    }

    /// A string replacing a quoted one, in its quotes: `"…"` always, since it
    /// escapes anything, and `'…'` when the new string has no control
    /// character, which the printer would double-quote. Nothing for a plain
    /// scalar, which is the printer's own choice, or a block scalar, which
    /// the printer writes for a string with line breaks.
    fn render_string(&self, _dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        if r.old_value.is_empty() {
            return Some(Ok(()));
        }
        match r.old_value[0] {
            b'"' => printer::write_double_quoted(out, r.value),
            b'\'' => {
                if !printer::has_control_char(r.value) {
                    printer::write_single_quoted(out, r.value)
                }
            }
            _ => {}
        }
        Some(Ok(()))
    }
}

// ── the `fig` binding (its default `fig` feature) ──────────────────────────

/// Register YAML, and return the format of its first dialect.
/// Registering it again returns the format it already has.
#[cfg(feature = "fig")]
pub fn register() -> core::result::Result<fig::Format, fig::Error> {
    fig::language::register_core(&LANGUAGE)
}

#[cfg(feature = "fig")]
fig::__language_crate_functions!("YAML");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_validates() {
        fig_kit::language::validate(&LANGUAGE).unwrap();
        assert!(LANGUAGE.has_split_stream());
        assert_eq!(2, LANGUAGE.type_count());
        assert_eq!(Some(&b"<<"[..]), LANGUAGE.syntax(1).merge_key);
    }

    #[test]
    fn render_string_keeps_the_old_quote_style() {
        let mut out = Vec::new();
        let r = RenderRequest { value: b"a'b", old_value: b"'x'", ..Default::default() };
        LANGUAGE.render_string(0, &mut out, &r).unwrap().unwrap();
        assert_eq!(b"'a''b'", &out[..]);
        out.clear();
        let r = RenderRequest { value: b"a\nb", old_value: b"'x'", ..Default::default() };
        LANGUAGE.render_string(0, &mut out, &r).unwrap().unwrap();
        assert!(out.is_empty());
        let r = RenderRequest { value: b"a\nb", old_value: b"\"x\"", ..Default::default() };
        LANGUAGE.render_string(0, &mut out, &r).unwrap().unwrap();
        assert_eq!(b"\"a\\nb\"", &out[..]);
    }
}
