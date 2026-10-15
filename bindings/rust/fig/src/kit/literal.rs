//! What a bare token means when nothing else says: fig-kit's port of the Zig kit's
//! `literal.zig`, the rule fig's editor classifies a caller's value by and
//! figl reads its bare tokens by.
//!
//! The rule is literal-else-string: `null`, `true` and `false` (lowercase
//! only), a number, a datetime, and otherwise the text as a string. A
//! number is decimal with an optional sign, fraction and exponent, or a
//! `0x`/`0o`/`0b` integer, with `_` between digits; a multi-digit integer
//! part led by a zero (`007`), `inf` and `nan` stay strings, as `Yes` and
//! `TRUE` do.

use fig_core::kit::literal::{self as core_literal, NumberKind, Sniffed};

use crate::ExtKind;
use crate::language::NodeKind;

/// The row bare `raw` is written as: its kind, and a datetime's
/// [`ExtKind`], whose row is a [`NodeKind::String`]. A bool's text is
/// `raw`.
pub fn kind(raw: &str) -> (NodeKind, Option<ExtKind>) {
    match core_literal::sniff_bare(raw.as_bytes()) {
        Sniffed::Null => (NodeKind::Null, None),
        Sniffed::Boolean(_) => (NodeKind::Bool, None),
        Sniffed::Number(n) => (number_kind(n.kind), None),
        // The datetime kind's index is its `ExtKind`'s.
        Sniffed::Datetime { kind, .. } => (NodeKind::String, ExtKind::from_c(kind as i32)),
        Sniffed::String => (NodeKind::String, None),
    }
}

/// Whether `raw` is a number by the same rule: [`NodeKind::Int`],
/// [`NodeKind::Float`], or `None`.
pub fn number(raw: &str) -> Option<NodeKind> {
    core_literal::sniff_number(raw.as_bytes()).map(|n| number_kind(n.kind))
}

fn number_kind(k: NumberKind) -> NodeKind {
    match k {
        NumberKind::Integer => NodeKind::Int,
        NumberKind::Float => NodeKind::Float,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind("null"), (NodeKind::Null, None));
        assert_eq!(kind("true"), (NodeKind::Bool, None));
        assert_eq!(kind("0x1F"), (NodeKind::Int, None));
        assert_eq!(
            kind("2026-07-01"),
            (NodeKind::String, Some(ExtKind::LocalDate))
        );
        assert_eq!(kind("007"), (NodeKind::String, None));
        assert_eq!(number("1e5"), Some(NodeKind::Float));
        assert_eq!(number("true"), None);
    }
}
