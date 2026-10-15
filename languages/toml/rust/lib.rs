//! TOML for [fig](https://docs.rs/fig): fig's own parser,
//! printer and editing rules for the language, as `fig-core` implements
//! them in Rust (its `lang::toml`, a port of the Zig in `src/`),
//! registered with fig's core, which carries no language until a crate
//! like this one registers it. Typed values go to and
//! from it as they do with `serde_json`:
//!
//! ```ignore
//! let config: Config = fig_toml::from_str(input)?;
//! let text = fig_toml::to_string(&config)?;
//! ```
//!
//! and [`format()`] is its format, for everything else fig does
//! (`fig::Document::parse(input, fig_toml::format()?)`). Neither needs it
//! registered first: the first call does that.
//!
//! It registers `toml`, and each version as `toml-<version>`
//! (`toml-1.0`), each found by name with [`fig::Format::by_name`].

/// Register TOML, and return the format of its first dialect.
/// Registering it again returns the format it already has.
pub fn register() -> Result<fig::Format, fig::Error> {
    fig::language::register_core(&fig_core::lang::toml::LANGUAGE)
}

fig::__language_crate_functions!("TOML");
