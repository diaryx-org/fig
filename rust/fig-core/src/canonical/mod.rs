//! Port of src/canonical/canonical.zig.
//!
//! The canonical form: a total, 1:1 text encoding of the AST.
//!
//! (Formerly "native". This is the AST's own oracle serialization, NOT the
//! human-facing `fig` authoring dialect — that is a separate, ergonomic surface
//! over the same AST, specified in DESIGN.md living under `languages/figl/`.)
//!
//! It is a bijection with `ast.zig`'s `Node.Kind` plus the YAML reference layer,
//! so any document round-trips through it unchanged. Two roles:
//!   * default/debug representation (`printer::print`), available without any
//!     format-specific codec;
//!   * comparison oracle: serialize two documents to native text and compare.
//!
//! Unlike JSON, it can represent every AST variant (int/float distinction,
//! extended scalars, non-string keys, anchors/tags/aliases), so test fixtures
//! that previously borrowed the JSON parser as an AST-literal syntax can use
//! `parse_abstract` instead — letting JSON *reading* become optional.
//!
//! It is not a registry entry: `serialize::Target::Canonical` names it as an
//! output (`Target::named("canonical")`), and the CLI reads it as an input
//! with `parse`, as Zig's `serialize.zig` and `cli/parse_dispatch.zig` do.

pub mod parser;
pub mod printer;

/// Parse native text into an owned AST.
pub use parser::parse_abstract;
/// Parse native text into a Document.
pub use parser::parse;
/// Print an AST as native text (trailing newline).
pub use printer::print;
/// Print a subtree (no trailing newline).
pub use printer::print_node;
