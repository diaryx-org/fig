//! NestedText for [fig](https://docs.rs/fig): fig's own parser,
//! printer and editing rules for the language, written in Zig (`src/`)
//! and compiled into a library of its own that links beside fig's, which
//! carries no language. Typed values go to and
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

use std::os::raw::c_int;

// The library's one export (`include/fig/nestedtext.h`).
unsafe extern "C" {
    fn fig_nestedtext_register(
        out_format: *mut c_int,
        out_err: *mut fig_sys::FigError,
    ) -> fig_sys::FigStatus;
}

/// Register NestedText, and return the format of its first dialect.
/// Registering it again returns the format it already has.
pub fn register() -> Result<fig::Format, fig::Error> {
    // SAFETY: `fig_nestedtext_register` is this crate's library's, which its
    // build script links.
    unsafe { fig::language::register_library(fig_nestedtext_register) }
}

fig::__language_crate_functions!("NestedText");
