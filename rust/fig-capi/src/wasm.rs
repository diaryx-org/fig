//! The wasm module's half of the runtime-language carrier (`src/wasm_host.zig`):
//! a language whose functions are JavaScript, or a language in a
//! WebAssembly module of its own, registered from the TypeScript binding.
//!
//! The binding cannot hand the module a C function pointer, so such a
//! language is fig-core's helper wire over a wasm import, `fig_host.call`
//! (`fig_core::host`), and a language in a module of its own parses and
//! prints through two more, `fig_host.parse` and `fig_host.print`, which
//! move the node table between the two modules' memories as its rows.
//! Exported by the wasm build alone: fig.h does not declare them, and the
//! imports mean nothing outside a wasm instance.

use core::ffi::{c_int, c_void};

use fig_core::host;
use fig_core::kit::table::{self, Table};
use fig_core::wire::{ErrorInfo, PrintOptions};

use crate::abi::{FigError, FigPrintOptions, FigStatus, FigStr, FigTableView};
use crate::language::register_failed;
use crate::{fig_free, fill_error, init, slice_of};

#[link(wasm_import_module = "fig_host")]
unsafe extern "C" {
    /// One request line in, one response line out, in bytes the host
    /// allocated with `fig_alloc`; nonzero when the host cannot answer.
    #[link_name = "call"]
    fn host_call(lang: u32, request: *const u8, request_len: usize, out_ptr: *mut *mut u8, out_len: *mut usize) -> c_int;
    /// Parse in the language's module and write the table into `out`
    /// (through `fig_table_extend`), or fill `err`.
    #[link_name = "parse"]
    fn host_parse(lang: u32, dialect: *const u8, dialect_len: usize, input: *const u8, input_len: usize, out: *mut c_void, err: *mut FigError) -> c_int;
    /// Print what `view` shows in the language's module, the text handed
    /// back in `fig_alloc`ed bytes.
    #[link_name = "print"]
    fn host_print(
        lang: u32,
        dialect: *const u8,
        dialect_len: usize,
        view: *const FigTableView,
        options: *const FigPrintOptions,
        out: *mut FigStr,
        err: *mut FigError,
    ) -> c_int;
}

/// `fig_core::host::Call` over the import: the response copied out of the
/// host's allocation, which is then freed.
fn call(lang: u32, request: &[u8]) -> Option<Vec<u8>> {
    let mut ptr: *mut u8 = core::ptr::null_mut();
    let mut len = 0usize;
    // SAFETY: the request is borrowed for the call; the out pointers are
    // this frame's. The host writes a `fig_alloc` allocation of `len`
    // bytes to them.
    if unsafe { host_call(lang, request.as_ptr(), request.len(), &mut ptr, &mut len) } != 0 || ptr.is_null() {
        return None;
    }
    // SAFETY: the host's answer is `len` bytes at `ptr`, ours to free.
    let line = unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec();
    // SAFETY: allocated by `fig_alloc(len)`, freed once.
    unsafe { fig_free(ptr, len) };
    Some(line)
}

/// A `FigError` the host filled, as the wire's `ErrorInfo`.
fn error_info(e: &FigError) -> ErrorInfo {
    ErrorInfo { message: e.text()[..e.text().len().min(255)].to_vec(), byte_offset: e.byte_offset, code: e.code }
}

/// `fig_core::host::Parse` over the import.
fn parse(lang: u32, dialect: &str, input: &[u8], out: &mut Table) -> Result<(), ErrorInfo> {
    let mut e = FigError::EMPTY;
    // SAFETY: every argument outlives the call; `out` is lent to the host
    // for it alone, which writes through `fig_table_extend`.
    let rc = unsafe { host_parse(lang, dialect.as_ptr(), dialect.len(), input.as_ptr(), input.len(), (out as *mut Table).cast::<c_void>(), &mut e) };
    if rc != 0 {
        let mut info = error_info(&e);
        info.code = rc;
        return Err(info);
    }
    Ok(())
}

/// `fig_core::host::Print` over the import.
fn print(lang: u32, dialect: &str, view: &table::View<'_>, options: &PrintOptions) -> Result<Vec<u8>, ErrorInfo> {
    let v = FigTableView::of(view);
    let o = FigPrintOptions::of(options);
    let mut s = FigStr::EMPTY;
    let mut e = FigError::EMPTY;
    // SAFETY: the view borrows `view`, which outlives the call; the rest are
    // this frame's.
    let rc = unsafe { host_print(lang, dialect.as_ptr(), dialect.len(), &v, &o, &mut s, &mut e) };
    if rc != 0 {
        return Err(error_info(&e));
    }
    // SAFETY: the host's text is `s.len` bytes at `s.ptr`.
    let text = unsafe { slice_of(s.ptr, s.len) }.unwrap_or(&[]).to_vec();
    if !s.ptr.is_null() {
        // SAFETY: the host allocated it with `fig_alloc(max(len, 1))`.
        unsafe { fig_free(s.ptr.cast_mut(), s.len.max(1)) };
    }
    Ok(text)
}

/// Write the outcome of a host registration.
///
/// # Safety
///
/// `out_format` non-null and writable; `out_err` null or a caller's
/// `FigError`.
unsafe fn registered(r: fig_core::Result<i32>, out_format: *mut c_int, out_err: *mut FigError) -> FigStatus {
    match r {
        Ok(abi) => {
            // SAFETY: the caller's contract.
            unsafe { *out_format = abi };
            FigStatus::Ok
        }
        Err(e) => register_failed(out_err, e),
    }
}

/// `fig_host_language_register`: register the language the host knows as
/// `lang`, described and called over `fig_host.call`.
///
/// # Safety
///
/// `out_format` null or writable; `out_err` null or a caller's `FigError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_host_language_register(lang: u32, out_format: *mut c_int, out_err: *mut FigError) -> FigStatus {
    if out_format.is_null() {
        return fill_error(out_err, FigStatus::InvalidArgument, b"out_format is null");
    }
    // SAFETY: checked non-null; writable by the caller's word.
    unsafe { *out_format = -1 };
    init();
    // SAFETY: as checked above.
    unsafe { registered(host::register(call, lang), out_format, out_err) }
}

/// `fig_host_native_register`: as `fig_host_language_register`, for a
/// language in a WebAssembly module of its own, parsed and printed through
/// `fig_host.parse` and `fig_host.print`.
///
/// # Safety
///
/// As `fig_host_language_register`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_host_native_register(lang: u32, out_format: *mut c_int, out_err: *mut FigError) -> FigStatus {
    if out_format.is_null() {
        return fill_error(out_err, FigStatus::InvalidArgument, b"out_format is null");
    }
    // SAFETY: checked non-null; writable by the caller's word.
    unsafe { *out_format = -1 };
    init();
    // SAFETY: as checked above.
    unsafe { registered(host::register_native(call, parse, print, lang), out_format, out_err) }
}
