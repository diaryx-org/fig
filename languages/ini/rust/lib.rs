//! INI for [fig](https://docs.rs/fig): fig's own parser,
//! printer and editing rules for the language, written in Zig (`src/`)
//! and compiled into a library of its own that links beside fig's, which
//! carries no language. Register it once, before reaching for a format of
//! it:
//!
//! ```ignore
//! let format = fig_ini::register()?;
//! let doc = fig::Document::parse(input, format)?;
//! ```
//!
//! It registers `ini`, found by name with [`fig::Format::by_name`].

use std::os::raw::c_int;

// The library's one export (`include/fig/ini.h`).
unsafe extern "C" {
    fn fig_ini_register(
        out_format: *mut c_int,
        out_err: *mut fig_sys::FigError,
    ) -> fig_sys::FigStatus;
}

/// Register INI, and return the format of its first dialect.
/// Registering it again returns the format it already has.
pub fn register() -> Result<fig::Format, fig::Error> {
    // SAFETY: `fig_ini_register` is this crate's library's, which its
    // build script links.
    unsafe { fig::language::register_library(fig_ini_register) }
}
