//! fig's C ABI, `bindings/c/include/fig.h`, over fig-core: the port of
//! `src/c_api.zig` (docs/proposals/rust-core.md §4 step 5, "`libfig` and the
//! wasm modules are built from Rust, with `fig.h` unchanged").
//!
//! Every function fig.h declares is exported here under its own name, with
//! its own signature, statuses and ownership rules. What each one does is
//! `fig_core::api`'s (documents, values, editors, statuses) and
//! `fig_core::registry`'s (formats); this crate is the marshalling Zig's
//! file does around them — pointers and lengths, size-versioned records,
//! borrowed out-parameters, handles, and a C-registered language as a
//! `Language` that calls its vtable (`language`). The `unsafe` is at that
//! boundary: each block says why it is sound, and what it relies on the
//! caller for is what fig.h states.
//!
//! The library bundles no language, as Zig's `libfig` does not: the
//! registry starts empty (`fig_core::registry::install_global`), and a
//! program registers each format it uses through `fig_language_register`.
//! The `bundle` feature preregisters fig-core's ten instead.
//!
//! Built for wasm32 the cdylib is the module the TypeScript binding loads
//! (`bindings/typescript/src/ffi.ts`): these exports, its `memory`, and the
//! wasm-only `fig_host_*` pair over the `fig_host` imports (`wasm`).
//!
//! # Modules
//!
//! ```text
//! abi       fig.h's records as #[repr(C)] Rust; FigStatus
//! document  fig_parse*, fig_document_*, fig_node_*, fig_keyvalue_*
//! editor    fig_editor_*
//! value     fig_value_*
//! language  fig_language_register, fig_format_*, fig_detect_order, fig_stream_*
//! table     fig_table_*
//! kit       fig_kit_*
//! wasm      fig_host_language_register, fig_host_native_register (wasm32 only)
//! ```

use core::ffi::{c_char, CStr};
use std::sync::Once;

pub mod abi;
mod document;
mod editor;
mod kit;
mod language;
mod table;
mod value;
#[cfg(target_arch = "wasm32")]
mod wasm;

pub use abi::*;
pub use document::*;
pub use editor::*;
pub use kit::*;
pub use language::*;
pub use table::*;
pub use value::*;
#[cfg(target_arch = "wasm32")]
pub use wasm::*;

// ============================================================================
// THE REGISTRY
// ============================================================================

static INIT: Once = Once::new();

/// Settle the process-wide registry before anything looks a format up:
/// empty, unless the `bundle` feature asks for fig's ten languages, which it
/// registers from their crates (Zig's `ensureBundled`; a refusal stops at the
/// language refused, as its `registerAll(...) catch {}` does).
pub(crate) fn init() {
    INIT.call_once(|| {
        fig_core::registry::install_global(fig_core::registry::Registry::new());
        #[cfg(feature = "bundle")]
        let _ = fig_core::registry::register_bundle(
            fig_core::registry::global(),
            &[
                &fig_json::LANGUAGE,
                &fig_yaml::LANGUAGE,
                &fig_toml::LANGUAGE,
                &fig_zon::LANGUAGE,
                &fig_figl::LANGUAGE,
                &fig_ini::LANGUAGE,
                &fig_dotenv::LANGUAGE,
                &fig_properties::LANGUAGE,
                &fig_plist::LANGUAGE,
                &fig_nestedtext::LANGUAGE,
            ],
        );
    });
}

// ============================================================================
// MARSHALLING
// ============================================================================

/// `sliceOf`: `len` bytes at `ptr`, or `None` for a null pointer with a
/// nonzero length. Zero bytes are the empty slice whatever the pointer.
///
/// # Safety
///
/// A non-null `ptr` must be readable for `len` bytes for `'a`.
pub(crate) unsafe fn slice_of<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 {
        return Some(&[]);
    }
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null, and readable for `len` bytes by the caller's word.
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

/// `len` elements at `ptr`, as `slice_of`.
///
/// # Safety
///
/// A non-null `ptr` must be readable for `len` elements for `'a`.
pub(crate) unsafe fn array_of<'a, T>(ptr: *const T, len: usize) -> Option<&'a [T]> {
    if len == 0 {
        return Some(&[]);
    }
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null, and readable for `len` elements by the caller's word.
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

/// The bytes of a NUL-terminated string, or `None` for a null pointer.
///
/// # Safety
///
/// A non-null `p` must point to a NUL-terminated string readable for `'a`.
pub(crate) unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a [u8]> {
    if p.is_null() {
        return None;
    }
    // SAFETY: non-null and NUL-terminated by the caller's word.
    Some(unsafe { CStr::from_ptr(p) }.to_bytes())
}

/// Whether a caller's `FigError.size` covers the field at `offset` of
/// `size` bytes (`errCovers`).
fn err_covers(size: u32, offset: usize, len: usize) -> bool {
    size as usize >= offset + len
}

/// `fillError`: write `status` and `message` into `out_err` (when non-null)
/// as far as its `size` covers, its location unknown, and return `status`.
pub(crate) fn fill_error(out_err: *mut FigError, status: FigStatus, message: &[u8]) -> FigStatus {
    let e = fig_core::api::FigError { code: status_of(status), byte_offset: 0, line: 0, column: 0, message: message.to_vec() };
    write_error(out_err, &e);
    status
}

/// `api::Status` for `status`.
fn status_of(status: FigStatus) -> fig_core::api::Status {
    fig_core::api::Status::from_code(status as i32).unwrap_or(fig_core::api::Status::InternalError)
}

/// `e` into `out_err` (when non-null), every field its `size` covers; the
/// message and its length only when the whole inline buffer is covered,
/// NUL-terminated and truncated to fit. Returns `e`'s status.
pub(crate) fn write_error(out_err: *mut FigError, e: &fig_core::api::FigError) -> FigStatus {
    let status = FigStatus::from(e.code);
    if out_err.is_null() {
        return status;
    }
    use core::mem::{offset_of, size_of};
    // SAFETY: a non-null `out_err` is the caller's FigError, and its `size`
    // says how much of it the caller laid out; each field is written only
    // when `size` covers it, through a raw pointer, so no reference to a
    // record shorter than ours is made.
    unsafe {
        let size = (*out_err).size;
        if err_covers(size, offset_of!(FigError, code), size_of::<i32>()) {
            (*out_err).code = e.code.code();
        }
        if err_covers(size, offset_of!(FigError, byte_offset), size_of::<usize>()) {
            (*out_err).byte_offset = e.byte_offset;
        }
        if err_covers(size, offset_of!(FigError, line), size_of::<u32>()) {
            (*out_err).line = e.line;
        }
        if err_covers(size, offset_of!(FigError, column), size_of::<u32>()) {
            (*out_err).column = e.column;
        }
        if err_covers(size, offset_of!(FigError, message), 256) {
            let n = e.message.len().min(255);
            let msg = core::ptr::addr_of_mut!((*out_err).message).cast::<u8>();
            core::ptr::copy_nonoverlapping(e.message.as_ptr(), msg, n);
            *msg.add(n) = 0;
            (*out_err).message_len = n;
        }
    }
    status
}

/// Write `value` through `out` when it is non-null.
///
/// # Safety
///
/// A non-null `out` must be writable for a `T`.
pub(crate) unsafe fn put<T>(out: *mut T, value: T) {
    if !out.is_null() {
        // SAFETY: non-null and writable by the caller's word.
        unsafe { out.write(value) };
    }
}

// ============================================================================
// VERSION
// ============================================================================

/// The ABI contract version fig.h's `FIG_ABI_VERSION` names.
pub const ABI_VERSION: u32 = 4;

/// `fig_version`: `(major << 16) | (minor << 8) | patch`.
#[unsafe(no_mangle)]
pub extern "C" fn fig_version() -> u32 {
    fig_core::api::version()
}

/// `fig_version_string`: static storage; the caller must not free it.
#[unsafe(no_mangle)]
pub extern "C" fn fig_version_string() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// `fig_abi_version`.
#[unsafe(no_mangle)]
pub extern "C" fn fig_abi_version() -> u32 {
    ABI_VERSION
}

// ============================================================================
// RAW MEMORY
// ============================================================================

/// `fig_alloc`: `len` uninitialized bytes from the library's allocator,
/// or null on failure or a zero-length request. Release with `fig_free`.
#[unsafe(no_mangle)]
pub extern "C" fn fig_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return core::ptr::null_mut();
    }
    let Ok(layout) = std::alloc::Layout::from_size_align(len, 1) else { return core::ptr::null_mut() };
    // SAFETY: `layout` has a nonzero size.
    unsafe { std::alloc::alloc(layout) }
}

/// `fig_free`: a sized free of what `fig_alloc` returned. A null pointer or
/// zero length is a no-op.
///
/// # Safety
///
/// `ptr` must be null or a `fig_alloc(len)` result not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    let Ok(layout) = std::alloc::Layout::from_size_align(len, 1) else { return };
    // SAFETY: by fig.h's contract `ptr` came from `fig_alloc(len)`, which
    // allocated it with this very layout.
    unsafe { std::alloc::dealloc(ptr, layout) }
}

#[cfg(test)]
mod tests;
