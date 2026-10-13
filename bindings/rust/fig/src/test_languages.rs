//! The languages fig's own unit tests read and write, each registered from
//! its library — the language crates are dev-dependencies, which link them —
//! through its C call: the crates' `register()` returns another copy of this
//! crate's `Format`, a unit test's being the crate under test.

use crate::Format;
use std::os::raw::c_int;
use std::sync::Once;

// Linked for their libraries alone.
use fig_dotenv as _;
use fig_figl as _;
use fig_ini as _;
use fig_json as _;
use fig_nestedtext as _;
use fig_plist as _;
use fig_properties as _;
use fig_toml as _;
use fig_yaml as _;
use fig_zon as _;

unsafe extern "C" {
    fn fig_json_register(
        out_format: *mut c_int,
        out_err: *mut crate::ffi::FigError,
    ) -> crate::ffi::FigStatus;
    fn fig_yaml_register(
        out_format: *mut c_int,
        out_err: *mut crate::ffi::FigError,
    ) -> crate::ffi::FigStatus;
    fn fig_toml_register(
        out_format: *mut c_int,
        out_err: *mut crate::ffi::FigError,
    ) -> crate::ffi::FigStatus;
    fn fig_zon_register(
        out_format: *mut c_int,
        out_err: *mut crate::ffi::FigError,
    ) -> crate::ffi::FigStatus;
    fn fig_figl_register(
        out_format: *mut c_int,
        out_err: *mut crate::ffi::FigError,
    ) -> crate::ffi::FigStatus;
    fn fig_ini_register(
        out_format: *mut c_int,
        out_err: *mut crate::ffi::FigError,
    ) -> crate::ffi::FigStatus;
    fn fig_dotenv_register(
        out_format: *mut c_int,
        out_err: *mut crate::ffi::FigError,
    ) -> crate::ffi::FigStatus;
    fn fig_properties_register(
        out_format: *mut c_int,
        out_err: *mut crate::ffi::FigError,
    ) -> crate::ffi::FigStatus;
    fn fig_plist_register(
        out_format: *mut c_int,
        out_err: *mut crate::ffi::FigError,
    ) -> crate::ffi::FigStatus;
    fn fig_nestedtext_register(
        out_format: *mut c_int,
        out_err: *mut crate::ffi::FigError,
    ) -> crate::ffi::FigStatus;
}

/// The registered format named `name`, every language fig ships registered
/// first.
pub fn by_name(name: &str) -> Format {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for register in [
            fig_json_register as crate::language::LibraryRegister,
            fig_yaml_register as crate::language::LibraryRegister,
            fig_toml_register as crate::language::LibraryRegister,
            fig_zon_register as crate::language::LibraryRegister,
            fig_figl_register as crate::language::LibraryRegister,
            fig_ini_register as crate::language::LibraryRegister,
            fig_dotenv_register as crate::language::LibraryRegister,
            fig_properties_register as crate::language::LibraryRegister,
            fig_plist_register as crate::language::LibraryRegister,
            fig_nestedtext_register as crate::language::LibraryRegister,
        ] {
            // SAFETY: each is its language library's, linked above.
            unsafe { crate::language::register_library(register) }.expect("a language registers");
        }
    });
    Format::by_name(name).unwrap_or_else(|| panic!("no format named {name:?}"))
}
