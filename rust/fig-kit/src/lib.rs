//! Port of kit/src/kit.zig: what fig's languages share — the node table,
//! the AST, `Document`, the `Language` trait and the shared rules — and
//! fig's error type ([`Error`], [`Result`], [`err!`]). fig's core
//! (`fig-core`, which re-exports this crate as `fig_core::kit`) and each
//! language crate (`fig-json`, …) build on it; it depends on nothing.

pub mod error;

pub use error::{Error, Result};

pub mod span;
pub mod token;
pub mod table;
pub mod document;
pub mod row_writer;
pub mod child_index;
pub mod flat_map;
pub mod convert;
pub mod manifest;
pub mod language;
pub mod literal;
pub mod lines;
pub mod parse_diagnostic;
pub mod xml;
pub mod unicode;
pub mod json_string;
pub mod ascii;
pub mod scanner;
pub mod writer;
pub mod grammar;
pub mod datetime;
pub mod width;
pub mod number;
pub mod yaml_schema;
pub mod describe_json;
pub mod ast;

/// `kit.zig`'s `eql`: slices of equal length with equal elements, by the
/// element type's own equality (Zig's dispatches to an `eql` method).
pub fn eql<T: PartialEq>(a: &[T], b: &[T]) -> bool {
    a == b
}

/// True when byte slice `s` exactly equals one of `options`. The shared
/// form of the keyword-set matching the format parsers do (null/bool
/// spellings, the `inf`/`nan` floats, etc.).
pub fn eql_any(s: &[u8], options: &[&[u8]]) -> bool {
    options.iter().any(|o| *o == s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn util_eql() {
        assert!(eql(b"hello, world!", b"hello, world!"));
    }

    #[test]
    fn util_eql_any() {
        assert!(eql_any(b"yes", &[b"y", b"yes", b"true"]));
        assert!(!eql_any(b"maybe", &[b"y", b"yes", b"true"]));
    }
}
