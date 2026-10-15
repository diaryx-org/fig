//! One of fig's languages as a library of its own, beside fig's: the port
//! of `kit/src/c_language.zig` and `kit/src/wasm_language.zig`.
//!
//! - As a static library (`libfig_<name>.a`), linked beside `libfig.a` by a
//!   C or Rust program: the one export `fig_<name>_register`
//!   (`languages/<name>/include/fig/<name>.h`), which registers the
//!   language through `fig_language_register` as any C host's language is
//!   (`c`).
//! - As a WebAssembly module of its own (`fig-<name>.wasm`, which the
//!   language's npm package inlines): the `fig_lang_*` exports the
//!   TypeScript binding drives (`bindings/typescript/src/native.ts`) to
//!   register it into fig's module and to parse and print through it
//!   (`wasm`).
//!
//! Either carries its own copy of the kit and of the language, so the table
//! fig hands a parse is not this library's: the language parses into a
//! table of its own and each list of it is copied into fig's, the rows as
//! they stand, since every offset in them is into the input or the buffer,
//! which both hold. A print is handed a `FigTableView` and reads it in
//! place.
//!
//! Which language is a cargo feature named after it; see `Cargo.toml` for
//! the build commands.

#[cfg(not(target_arch = "wasm32"))]
mod c;
mod lang;
#[cfg(target_arch = "wasm32")]
mod wasm;

/// Each language: its feature, its crate, and its C library's export.
macro_rules! languages {
    ($($feature:literal => $krate:ident, $register:ident;)*) => {$(
        /// Register the language. On `FIG_STATUS_OK`, `*out_format` is the
        /// format of its first dialect; otherwise `out_err` (optional) says
        /// why. Registered already — a library and the program using it
        /// each registering what they use — it is the format it already
        /// has.
        ///
        /// # Safety
        ///
        /// `out_format` must be writable, and `out_err` null or a
        /// `FigError` of at least its `size` bytes.
        #[cfg(all(feature = $feature, not(target_arch = "wasm32")))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $register(out_format: *mut core::ffi::c_int, out_err: *mut fig_abi::FigError) -> core::ffi::c_int {
            static VTABLE: c::Registration = c::Registration::new();
            // SAFETY: the caller's contract.
            unsafe { VTABLE.register(&$krate::LANGUAGE, out_format, out_err) }
        }

        #[cfg(all(feature = $feature, target_arch = "wasm32"))]
        wasm::exports!(&$krate::LANGUAGE);
    )*};
}

languages! {
    "json" => fig_json, fig_json_register;
    "yaml" => fig_yaml, fig_yaml_register;
    "toml" => fig_toml, fig_toml_register;
    "zon" => fig_zon, fig_zon_register;
    "figl" => fig_figl, fig_figl_register;
    "ini" => fig_ini, fig_ini_register;
    "dotenv" => fig_dotenv, fig_dotenv_register;
    "properties" => fig_properties, fig_properties_register;
    "plist" => fig_plist, fig_plist_register;
    "nestedtext" => fig_nestedtext, fig_nestedtext_register;
}
