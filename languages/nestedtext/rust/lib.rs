//! NestedText for [fig](https://docs.rs/fig): fig's own parser,
//! printer and editing rules for the language, as `fig-core` implements
//! them in Rust (its `lang::nestedtext`, a port of the Zig in `src/`),
//! registered with fig's core, which carries no language until a crate
//! like this one registers it. Typed values go to and
//! from it as they do with `serde_json`:
//!
//! ```ignore
//! let config: Config = fig_nestedtext::from_str(input)?;
//! let text = fig_nestedtext::to_string(&config)?;
//! ```
//!
//! and [`format()`] is its format, for everything else fig does
//! (`fig::Document::parse(input, fig_nestedtext::format()?)`). Neither needs it
//! registered first: the first call does that.
//!
//! It registers `nestedtext`, found by name with [`fig::Format::by_name`].

/// Register NestedText, and return the format of its first dialect.
/// Registering it again returns the format it already has.
pub fn register() -> Result<fig::Format, fig::Error> {
    fig::language::register_core(&fig_core::lang::nestedtext::LANGUAGE)
}

fig::__language_crate_functions!("NestedText");
