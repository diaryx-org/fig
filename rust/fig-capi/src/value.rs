//! Values: `fig_value_*`, a tree built node by node and rendered to any
//! format. A `FigValue` is a boxed `fig_core::api::Value`.

use core::ffi::c_int;

use fig_core::api;

use crate::abi::{FigKeyValue, FigSerializeOptions, FigStatus, FigWarning};
use crate::document::{options_of, write_warning, FigNodeId};
use crate::{array_of, init, slice_of};

/// `FigValue`: opaque to C.
pub struct FigValue {
    value: api::Value,
}

/// The handle behind `value`.
///
/// # Safety
///
/// A non-null `value` must be a live handle from `fig_value_create`, used
/// by this call alone.
unsafe fn handle<'a>(value: *mut FigValue) -> Option<&'a mut FigValue> {
    // SAFETY: by the caller's word a non-null `value` is live and unshared.
    unsafe { value.as_mut() }
}

/// `emitNode`: a builder call's id through `out_id`.
///
/// # Safety
///
/// `out_id` must be null or writable.
unsafe fn emit(out_id: *mut FigNodeId, f: impl FnOnce() -> Result<FigNodeId, api::Status>) -> FigStatus {
    if out_id.is_null() {
        return FigStatus::InvalidArgument;
    }
    match f() {
        Ok(id) => {
            // SAFETY: checked non-null; writable by the caller's word.
            unsafe { *out_id = id };
            FigStatus::Ok
        }
        Err(s) => s.into(),
    }
}

/// `fig_value_create`.
///
/// # Safety
///
/// `out_value` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_create(out_value: *mut *mut FigValue) -> FigStatus {
    if out_value.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: checked non-null; writable by the caller's word.
    unsafe { *out_value = Box::into_raw(Box::new(FigValue { value: api::Value::create() })) };
    FigStatus::Ok
}

/// `fig_value_destroy`. Null is a no-op.
///
/// # Safety
///
/// `value` must be null or a live handle, not used again after.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_destroy(value: *mut FigValue) {
    if !value.is_null() {
        // SAFETY: a live handle is a `Box` this library made; the caller
        // gives it up.
        drop(unsafe { Box::from_raw(value) });
    }
}

/// `fig_value_null`.
///
/// # Safety
///
/// `value` must be null or a live handle, `out_id` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_null(value: *mut FigValue, out_id: *mut FigNodeId) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    unsafe { emit(out_id, || h.value.null()) }
}

/// `fig_value_bool`.
///
/// # Safety
///
/// `value` must be null or a live handle, `out_id` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_bool(value: *mut FigValue, b: bool, out_id: *mut FigNodeId) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    unsafe { emit(out_id, || h.value.bool(b)) }
}

/// `fig_value_int`.
///
/// # Safety
///
/// `value` must be null or a live handle, `out_id` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_int(value: *mut FigValue, n: i64, out_id: *mut FigNodeId) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    unsafe { emit(out_id, || h.value.int(n)) }
}

/// `fig_value_uint`.
///
/// # Safety
///
/// `value` must be null or a live handle, `out_id` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_uint(value: *mut FigValue, n: u64, out_id: *mut FigNodeId) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    unsafe { emit(out_id, || h.value.uint(n)) }
}

/// `fig_value_number`: a number from formatted text.
///
/// # Safety
///
/// `value` must be null or a live handle, `raw` readable for `raw_len`
/// bytes, `out_id` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_number(value: *mut FigValue, raw: *const u8, raw_len: usize, is_float: bool, out_id: *mut FigNodeId) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    let Some(raw) = (unsafe { slice_of(raw, raw_len) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    unsafe { emit(out_id, || h.value.number(raw, is_float)) }
}

/// `fig_value_string`.
///
/// # Safety
///
/// `value` must be null or a live handle, `ptr` readable for `len` bytes,
/// `out_id` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_string(value: *mut FigValue, ptr: *const u8, len: usize, out_id: *mut FigNodeId) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    let Some(s) = (unsafe { slice_of(ptr, len) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    unsafe { emit(out_id, || h.value.string(s)) }
}

/// `fig_value_extended`: `kind` a `FigExtKind`.
///
/// # Safety
///
/// `value` must be null or a live handle, `text` readable for `text_len`
/// bytes, `out_id` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_extended(value: *mut FigValue, kind: c_int, text: *const u8, text_len: usize, out_id: *mut FigNodeId) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    if u8::try_from(kind).ok().and_then(fig_core::kit::ast::ExtKind::from_ordinal).is_none() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { slice_of(text, text_len) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    unsafe { emit(out_id, || h.value.extended(kind, t)) }
}

/// `fig_value_seq`: from ids already created.
///
/// # Safety
///
/// `value` must be null or a live handle, `items` readable for `items_len`
/// ids, `out_id` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_seq(value: *mut FigValue, items: *const FigNodeId, items_len: usize, out_id: *mut FigNodeId) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    let Some(ids) = (unsafe { array_of(items, items_len) }) else { return FigStatus::InvalidArgument };
    if ids.iter().any(|&id| id as usize >= h.value.node_count()) {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: the caller's contract.
    unsafe { emit(out_id, || h.value.seq(ids)) }
}

/// `fig_value_map`: from keyvalue pairs of ids already created.
///
/// # Safety
///
/// `value` must be null or a live handle, `entries` readable for
/// `entries_len` pairs, `out_id` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_map(value: *mut FigValue, entries: *const FigKeyValue, entries_len: usize, out_id: *mut FigNodeId) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    let Some(es) = (unsafe { array_of(entries, entries_len) }) else { return FigStatus::InvalidArgument };
    let count = h.value.node_count();
    if es.iter().any(|e| e.key as usize >= count || e.value as usize >= count) {
        return FigStatus::InvalidArgument;
    }
    let kvs: Vec<api::FigKeyValue> = es.iter().map(|e| api::FigKeyValue { key: e.key, value: e.value }).collect();
    // SAFETY: the caller's contract.
    unsafe { emit(out_id, || h.value.map(&kvs)) }
}

/// `fig_value_serialize`: `fig_value_serialize_opts` with the defaults.
///
/// # Safety
///
/// As `fig_value_serialize_opts`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_serialize(value: *mut FigValue, root: FigNodeId, format: c_int, out_ptr: *mut *const u8, out_len: *mut usize) -> FigStatus {
    // SAFETY: the same arguments, under the same contract.
    unsafe { fig_value_serialize_opts(value, root, format, core::ptr::null(), out_ptr, out_len) }
}

/// `fig_value_serialize_opts`: the subtree at `root` in `format`, borrowed
/// from the handle until the next serialize on it or its destroy.
///
/// # Safety
///
/// `value` must be null or a live handle, `options` null or a caller's
/// record, the out pointers null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_serialize_opts(
    value: *mut FigValue,
    root: FigNodeId,
    format: c_int,
    options: *const FigSerializeOptions,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> FigStatus {
    if out_ptr.is_null() || out_len.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    let opts = unsafe { options_of(options) };
    init();
    match h.value.serialize_opts(root, format, opts.as_ref()) {
        Ok(bytes) => {
            // SAFETY: both checked non-null above.
            unsafe {
                *out_ptr = bytes.as_ptr();
                *out_len = bytes.len();
            }
            FigStatus::Ok
        }
        Err(s) => s.into(),
    }
}

/// `fig_value_diagnose`: how many lossy events serializing the subtree at
/// `root` to `format` would produce.
///
/// # Safety
///
/// `value` must be null or a live handle, `options` null or a caller's
/// record, `out_count` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_diagnose(
    value: *mut FigValue,
    root: FigNodeId,
    format: c_int,
    options: *const FigSerializeOptions,
    out_count: *mut usize,
) -> FigStatus {
    if out_count.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: checked non-null; writable by the caller's word.
    unsafe { *out_count = 0 };
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    let opts = unsafe { options_of(options) };
    init();
    match h.value.diagnose(root, format, opts.as_ref()) {
        Ok(n) => {
            // SAFETY: checked non-null above.
            unsafe { *out_count = n };
            FigStatus::Ok
        }
        Err(s) => s.into(),
    }
}

/// `fig_value_warning`: the event at `index` of the latest diagnose.
///
/// # Safety
///
/// `value` must be null or a live handle, `out` null or a caller's
/// `FigWarning` with `size` set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_value_warning(value: *mut FigValue, index: usize, out: *mut FigWarning) -> FigStatus {
    if out.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(value) }) else { return FigStatus::InvalidArgument };
    match h.value.warning(index) {
        Ok(w) => {
            // SAFETY: the caller's record, checked non-null.
            unsafe { write_warning(out, w) };
            FigStatus::Ok
        }
        Err(s) => s.into(),
    }
}
