//! INI for [fig](https://docs.rs/fig): fig's own parser,
//! printer and editing rules for the language, as `fig-core` implements
//! them in Rust (its `lang::ini`, a port of the Zig in `src/`),
//! registered with fig's core, which carries no language until a crate
//! like this one registers it. Typed values go to and
//! from it as they do with `serde_json`:
//!
//! ```ignore
//! let config: Config = fig_ini::from_str(input)?;
//! let text = fig_ini::to_string(&config)?;
//! ```
//!
//! and [`format()`] is its format, for everything else fig does
//! (`fig::Document::parse(input, fig_ini::format()?)`). Neither needs it
//! registered first: the first call does that.
//!
//! It registers `ini`, found by name with [`fig::Format::by_name`].

/// Register INI, and return the format of its first dialect.
/// Registering it again returns the format it already has.
pub fn register() -> Result<fig::Format, fig::Error> {
    fig::language::register_core(&fig_core::lang::ini::LANGUAGE)
}

fig::__language_crate_functions!("INI");
