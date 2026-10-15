//! Port of src/deserialize.zig.
//!
//! Not ported, deliberately. The Zig module reads a document into a Zig
//! type by comptime reflection (`@typeInfo`, `@field`): `deserialize(T,
//! allocator, source, format)` and `fromAst(T, ...)`, with options such as
//! `ignore_unknown_fields`. Rust has no runtime reflection over its types;
//! the equivalent is serde, which the `fig` crate (not this one, which takes
//! no external crates) implements over the AST, reaching a format through
//! `registry::global().entry_by_name(format)` as the Zig does through
//! `Runtime.entryByName` (`error.UnsupportedFormat` when there is none).
