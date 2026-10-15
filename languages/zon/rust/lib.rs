//! ZON, Zig Object Notation for [fig](https://docs.rs/fig): fig's own parser,
//! printer and editing rules for the language, as `fig-core` implements
//! them in Rust (its `lang::zon`, a port of the Zig in `src/`),
//! registered with fig's core, which carries no language until a crate
//! like this one registers it. Typed values go to and
//! from it as they do with `serde_json`:
//!
//! ```ignore
//! let config: Config = fig_zon::from_str(input)?;
//! let text = fig_zon::to_string(&config)?;
//! ```
//!
//! and [`format()`] is its format, for everything else fig does
//! (`fig::Document::parse(input, fig_zon::format()?)`). Neither needs it
//! registered first: the first call does that.
//!
//! It registers `zon`, found by name with [`fig::Format::by_name`].

/// Register ZON, Zig Object Notation, and return the format of its first dialect.
/// Registering it again returns the format it already has.
pub fn register() -> Result<fig::Format, fig::Error> {
    fig::language::register_core(&fig_core::lang::zon::LANGUAGE)
}

fig::__language_crate_functions!("ZON");
