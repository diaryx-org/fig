//! Port of src/cli/host_languages.zig.
//!
//! Languages the CLI's host serves, in the WASI build: `@diaryx/fig-wasi`'s
//! `bin/fig.mjs` runs the CLI under Node and holds `@diaryx/fig`'s
//! JavaScript languages, and a format this build did not compile in is
//! asked of it before `languages.figl` is — a WASI module cannot spawn the
//! helper a `languages.figl` names.
//!
//! The host answers three imports: `resolve` and `resolve_extension` name
//! the language serving a dialect name or a file extension (0 for none),
//! and `call` carries the wire to it (`fig_core::host`). A response is
//! written into this module's memory through the `fig_alloc` export below.
//! A host that does not provide the imports cannot instantiate this module,
//! so only a build for one asks: a WASI build with the `wasi-host` feature
//! (Zig's `-Dwasi-host=true`). Every other build answers nothing.

use std::sync::Arc;

use fig_core::registry::Entry;

/// Whether this build asks a host at all: a WASI build made for one
/// (`wasi-host`), since no other WASI runtime provides the imports.
pub const ENABLED: bool = cfg!(all(target_os = "wasi", feature = "wasi-host"));

/// The language the host serves for the dialect `name`, registered on this
/// first ask; `None` when the host has none, or refused to register it.
pub fn by_name(name: &str) -> Option<Arc<Entry>> {
    #[cfg(all(target_os = "wasi", feature = "wasi-host"))]
    {
        imp::by_name(name)
    }
    #[cfg(not(all(target_os = "wasi", feature = "wasi-host")))]
    {
        let _ = name;
        None
    }
}

/// The language the host serves for files ending `.ext`, registered on this
/// first ask.
pub fn by_extension(ext: &str) -> Option<Arc<Entry>> {
    #[cfg(all(target_os = "wasi", feature = "wasi-host"))]
    {
        imp::by_extension(ext)
    }
    #[cfg(not(all(target_os = "wasi", feature = "wasi-host")))]
    {
        let _ = ext;
        None
    }
}

#[cfg(all(target_os = "wasi", feature = "wasi-host"))]
mod imp {
    use std::alloc::{alloc, dealloc, Layout};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;

    use fig_core::host;
    use fig_core::registry::{self, Entry};

    #[link(wasm_import_module = "fig_host")]
    unsafe extern "C" {
        fn call(lang: u32, request: *const u8, request_len: usize, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32;
        fn resolve(name: *const u8, name_len: usize) -> u32;
        fn resolve_extension(ext: *const u8, ext_len: usize) -> u32;
    }

    /// Where the host writes a response: `len` bytes, freed once the
    /// response is read (`host_call`).
    #[unsafe(no_mangle)]
    pub extern "C" fn fig_alloc(len: usize) -> *mut u8 {
        if len == 0 {
            return std::ptr::null_mut();
        }
        let Ok(layout) = Layout::array::<u8>(len) else { return std::ptr::null_mut() };
        unsafe { alloc(layout) }
    }

    /// `fig_host.call`, as `fig_core::host::Call`: the response read and its
    /// memory freed.
    fn host_call(lang: u32, request: &[u8]) -> Option<Vec<u8>> {
        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let rc = unsafe { call(lang, request.as_ptr(), request.len(), &mut out_ptr, &mut out_len) };
        if rc != 0 || out_ptr.is_null() {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(out_ptr, out_len) }.to_vec();
        if let Ok(layout) = Layout::array::<u8>(out_len) {
            unsafe { dealloc(out_ptr, layout) };
        }
        Some(bytes)
    }

    thread_local! {
        /// The host languages registered so far, by the host's id: each is
        /// registered once, whichever of its names or extensions asked first.
        static KNOWN: RefCell<HashMap<u32, i32>> = RefCell::new(HashMap::new());
    }

    pub fn by_name(name: &str) -> Option<Arc<Entry>> {
        if let Some(e) = registry::global().entry_by_name(name) {
            return Some(e);
        }
        let lang = unsafe { resolve(name.as_ptr(), name.len()) };
        if lang == 0 {
            return None;
        }
        registered(lang, Some(name))
    }

    pub fn by_extension(ext: &str) -> Option<Arc<Entry>> {
        let lang = unsafe { resolve_extension(ext.as_ptr(), ext.len()) };
        if lang == 0 {
            return None;
        }
        registered(lang, None)
    }

    /// Register host language `lang` if it is not yet, and hand back the
    /// entry for `name`, or for its first dialect when asked by extension.
    fn registered(lang: u32, name: Option<&str>) -> Option<Arc<Entry>> {
        let abi = match KNOWN.with(|k| k.borrow().get(&lang).copied()) {
            Some(abi) => abi,
            None => {
                registry::clear_refusal();
                let abi = match host::register(host_call, lang) {
                    Ok(abi) => abi,
                    Err(err) => {
                        let why = if err.is("Refused") {
                            String::from_utf8_lossy(&registry::last_refusal()).into_owned()
                        } else {
                            err.name().to_string()
                        };
                        crate::log_err!("the language the host serves for `{}`: {why}", name.unwrap_or("an extension"));
                        return None;
                    }
                };
                KNOWN.with(|k| k.borrow_mut().insert(lang, abi));
                abi
            }
        };
        match name {
            Some(n) => registry::global().entry_by_name(n),
            None => registry::global().entry_by_abi(abi),
        }
    }
}
