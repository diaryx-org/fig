//! The Apple XML property list for [fig](https://docs.rs/fig): fig's own parser,
//! printer and editing rules for the language, written in Zig (`src/`)
//! and compiled into a library of its own that links beside fig's, which
//! carries no language. Typed values go to and
//! from it as they do with `serde_json`:
//!
//! ```ignore
//! let config: Config = fig_plist::from_str(input)?;
//! let text = fig_plist::to_string(&config)?;
//! ```
//!
//! and [`format()`] is its format, for everything else fig does
//! (`fig::Document::parse(input, fig_plist::format()?)`). Neither needs it
//! registered first: the first call does that.
//!
//! It registers `plist`, found by name with [`fig::Format::by_name`].

use std::os::raw::c_int;

// The library's one export (`include/fig/plist.h`).
unsafe extern "C" {
    fn fig_plist_register(
        out_format: *mut c_int,
        out_err: *mut fig_sys::FigError,
    ) -> fig_sys::FigStatus;
}

/// Register the Apple XML property list, and return the format of its first dialect.
/// Registering it again returns the format it already has.
pub fn register() -> Result<fig::Format, fig::Error> {
    // SAFETY: `fig_plist_register` is this crate's library's, which its
    // build script links.
    unsafe { fig::language::register_library(fig_plist_register) }
}

fig::__language_crate_functions!("plist");
