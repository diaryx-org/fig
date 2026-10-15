//! Documents: `fig_parse`, `fig_parse_ex`, `fig_document_*`, the read-only
//! traversal (`fig_node_*`, `fig_keyvalue_*`), conversion
//! (`fig_document_serialize`) and diagnostics (`fig_document_diagnose`,
//! `fig_document_warning`). A `FigDocument` is a boxed
//! `fig_core::api::Document`; what it does is that type's.

use core::ffi::c_int;

use fig_core::api::{self, FigNodeKind, NODE_NONE};
use fig_core::diagnostics::{Cause, Code, Warning};

use crate::abi::{FigError, FigSerializeOptions, FigStatus, FigWarning};
use crate::{fill_error, init, put, slice_of, write_error};

/// `FigDocument`: opaque to C.
pub struct FigDocument {
    pub(crate) doc: api::Document,
}

pub type FigNodeId = u32;

/// The handle behind `doc`, or `None` for null.
///
/// # Safety
///
/// A non-null `doc` must be a live handle from `fig_parse*` or
/// `fig_document_resolve_aliases`.
unsafe fn handle<'a>(doc: *const FigDocument) -> Option<&'a FigDocument> {
    // SAFETY: by the caller's word a non-null `doc` is a live handle.
    unsafe { doc.as_ref() }
}

/// As `handle`, mutably: fig.h's threading rule is that one handle is never
/// used from two calls at once.
///
/// # Safety
///
/// As `handle`, and no other call is using `doc`.
unsafe fn handle_mut<'a>(doc: *mut FigDocument) -> Option<&'a mut FigDocument> {
    // SAFETY: by the caller's word a non-null `doc` is a live handle, used
    // by this call alone.
    unsafe { doc.as_mut() }
}

/// `fig_parse`: `fig_parse_ex` without a diagnostic.
///
/// # Safety
///
/// `input` must be readable for `input_len` bytes (or null with zero), and
/// `out_doc` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_parse(input: *const u8, input_len: usize, format: c_int, out_doc: *mut *mut FigDocument) -> FigStatus {
    // SAFETY: the same arguments, under the same contract.
    unsafe { fig_parse_ex(input, input_len, format, out_doc, core::ptr::null_mut()) }
}

/// `fig_parse_ex`: parse `input` as `format` into a new document; on a
/// failure, `out_err` (nullable, size-gated) says why and where.
///
/// # Safety
///
/// As `fig_parse`, and `out_err` null or a caller's `FigError` with `size`
/// set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_parse_ex(
    input: *const u8,
    input_len: usize,
    format: c_int,
    out_doc: *mut *mut FigDocument,
    out_err: *mut FigError,
) -> FigStatus {
    if out_doc.is_null() {
        return fill_error(out_err, FigStatus::InvalidArgument, b"out_doc is null");
    }
    // SAFETY: non-null, and writable by the caller's word.
    unsafe { *out_doc = core::ptr::null_mut() };
    // SAFETY: `input` is readable for `input_len` bytes by the caller's word.
    let Some(bytes) = (unsafe { slice_of(input, input_len) }) else {
        return fill_error(out_err, FigStatus::InvalidArgument, b"null input with nonzero length");
    };
    init();
    match api::Document::parse_ex(bytes, format) {
        Ok(doc) => {
            // SAFETY: checked non-null and writable above.
            unsafe { *out_doc = Box::into_raw(Box::new(FigDocument { doc })) };
            FigStatus::Ok
        }
        Err(e) => write_error(out_err, &e),
    }
}

/// `fig_document_destroy`: release a document. Null is a no-op.
///
/// # Safety
///
/// `doc` must be null or a live handle, not used again after.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_document_destroy(doc: *mut FigDocument) {
    if !doc.is_null() {
        // SAFETY: a live handle is a `Box` this library made; the caller
        // gives it up.
        drop(unsafe { Box::from_raw(doc) });
    }
}

/// `fig_document_root`: the root's id, `FIG_NODE_NONE` for an empty
/// document or a null handle.
///
/// # Safety
///
/// `doc` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_document_root(doc: *const FigDocument) -> FigNodeId {
    // SAFETY: the caller's contract.
    unsafe { handle(doc) }.map_or(NODE_NONE, |h| h.doc.root())
}

/// `fig_node_kind`: a `FigNodeKind`, as `int`.
///
/// # Safety
///
/// `doc` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_node_kind(doc: *const FigDocument, node: FigNodeId) -> c_int {
    // SAFETY: the caller's contract.
    (unsafe { handle(doc) }.map_or(FigNodeKind::Invalid, |h| h.doc.node_kind(node))) as c_int
}

/// `fig_node_first_child`.
///
/// # Safety
///
/// `doc` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_node_first_child(doc: *const FigDocument, node: FigNodeId) -> FigNodeId {
    // SAFETY: the caller's contract.
    unsafe { handle(doc) }.map_or(NODE_NONE, |h| h.doc.first_child(node))
}

/// `fig_node_next_sibling`.
///
/// # Safety
///
/// `doc` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_node_next_sibling(doc: *const FigDocument, node: FigNodeId) -> FigNodeId {
    // SAFETY: the caller's contract.
    unsafe { handle(doc) }.map_or(NODE_NONE, |h| h.doc.next_sibling(node))
}

/// `fig_node_child_count`.
///
/// # Safety
///
/// `doc` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_node_child_count(doc: *const FigDocument, node: FigNodeId) -> usize {
    // SAFETY: the caller's contract.
    unsafe { handle(doc) }.map_or(0, |h| h.doc.child_count(node))
}

/// `fig_keyvalue_key`.
///
/// # Safety
///
/// `doc` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_keyvalue_key(doc: *const FigDocument, node: FigNodeId) -> FigNodeId {
    // SAFETY: the caller's contract.
    unsafe { handle(doc) }.map_or(NODE_NONE, |h| h.doc.keyvalue_key(node))
}

/// `fig_keyvalue_value`.
///
/// # Safety
///
/// `doc` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_keyvalue_value(doc: *const FigDocument, node: FigNodeId) -> FigNodeId {
    // SAFETY: the caller's contract.
    unsafe { handle(doc) }.map_or(NODE_NONE, |h| h.doc.keyvalue_value(node))
}

/// `fig_node_bool`: true, with `*out`, for a boolean node.
///
/// # Safety
///
/// `doc` must be null or a live handle, `out` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_node_bool(doc: *const FigDocument, node: FigNodeId, out: *mut bool) -> bool {
    if out.is_null() {
        return false;
    }
    // SAFETY: the caller's contract.
    let Some(b) = unsafe { handle(doc) }.and_then(|h| h.doc.node_bool(node)) else { return false };
    // SAFETY: checked non-null; writable by the caller's word.
    unsafe { *out = b };
    true
}

/// Write `bytes` (borrowed from the document) through the out pair.
///
/// # Safety
///
/// Both pointers non-null and writable.
unsafe fn out_bytes(bytes: &[u8], out_ptr: *mut *const u8, out_len: *mut usize) -> bool {
    // SAFETY: the caller checked both pointers.
    unsafe {
        *out_ptr = bytes.as_ptr();
        *out_len = bytes.len();
    }
    true
}

/// `fig_node_number`: a number's raw text (a char literal's too), borrowed
/// from the document.
///
/// # Safety
///
/// `doc` must be null or a live handle, the out pointers null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_node_number(doc: *const FigDocument, node: FigNodeId, out_ptr: *mut *const u8, out_len: *mut usize) -> bool {
    if out_ptr.is_null() || out_len.is_null() {
        return false;
    }
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(doc) }) else { return false };
    match h.doc.node_number(node) {
        // SAFETY: both out pointers checked non-null above.
        Some(b) => unsafe { out_bytes(b, out_ptr, out_len) },
        None => false,
    }
}

/// `fig_node_string`: a string's text (an extended scalar's, but a char
/// literal's), borrowed from the document.
///
/// # Safety
///
/// `doc` must be null or a live handle, the out pointers null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_node_string(doc: *const FigDocument, node: FigNodeId, out_ptr: *mut *const u8, out_len: *mut usize) -> bool {
    if out_ptr.is_null() || out_len.is_null() {
        return false;
    }
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(doc) }) else { return false };
    match h.doc.node_string(node) {
        // SAFETY: both out pointers checked non-null above.
        Some(b) => unsafe { out_bytes(b, out_ptr, out_len) },
        None => false,
    }
}

/// `fig_node_extended`: an extended scalar's `FigExtKind` and text.
///
/// # Safety
///
/// `doc` must be null or a live handle, the out pointers null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_node_extended(
    doc: *const FigDocument,
    node: FigNodeId,
    out_kind: *mut c_int,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> bool {
    if out_kind.is_null() || out_ptr.is_null() || out_len.is_null() {
        return false;
    }
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(doc) }) else { return false };
    match h.doc.node_extended(node) {
        Some((kind, text)) => {
            // SAFETY: every out pointer checked non-null above.
            unsafe {
                *out_kind = kind.ordinal() as c_int;
                out_bytes(text, out_ptr, out_len)
            }
        }
        None => false,
    }
}

/// `fig_document_resolve_aliases`: a new document with the reference layer
/// resolved, or a null `*out_doc` when there is nothing to resolve.
///
/// # Safety
///
/// `doc` must be null or a live handle, `out_doc` null or writable,
/// `out_err` null or a caller's `FigError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_document_resolve_aliases(
    doc: *const FigDocument,
    out_doc: *mut *mut FigDocument,
    out_err: *mut FigError,
) -> FigStatus {
    if out_doc.is_null() {
        return fill_error(out_err, FigStatus::InvalidArgument, b"out_doc is null");
    }
    // SAFETY: non-null, writable by the caller's word.
    unsafe { *out_doc = core::ptr::null_mut() };
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(doc) }) else {
        return fill_error(out_err, FigStatus::InvalidArgument, b"doc is null");
    };
    match h.doc.resolve_aliases() {
        Ok(None) => FigStatus::Ok,
        Ok(Some(resolved)) => {
            // SAFETY: checked non-null above.
            unsafe { *out_doc = Box::into_raw(Box::new(FigDocument { doc: resolved })) };
            FigStatus::Ok
        }
        Err(e) => write_error(out_err, &e),
    }
}

// ============================================================================
// SERIALIZE OPTIONS
// ============================================================================

/// `serializeOptionsOf` and `losslessRequested`, as the options fig-core
/// takes: each field read only when the caller's `size` covers it, else
/// its default. `None` for a null pointer, the defaults.
///
/// # Safety
///
/// A non-null `options` must point to a caller's record of at least its
/// `size` bytes (or of `size_of::<FigSerializeOptions>()`, whichever is
/// smaller).
pub(crate) unsafe fn options_of(options: *const FigSerializeOptions) -> Option<api::FigSerializeOptions> {
    if options.is_null() {
        return None;
    }
    use core::mem::{offset_of, size_of};
    let mut out = api::FigSerializeOptions::default();
    // SAFETY: non-null; `size` leads every layout, and each field below is
    // read only when the caller's `size` says it laid it out.
    unsafe {
        let size = (*options).size as usize;
        let covers = |offset: usize, len: usize| size >= offset + len;
        if covers(offset_of!(FigSerializeOptions, pretty), 1) {
            out.pretty = (*options).pretty != 0;
        }
        if covers(offset_of!(FigSerializeOptions, indent), 1) {
            out.indent = if (*options).indent == 0 { 2 } else { (*options).indent };
        }
        if covers(offset_of!(FigSerializeOptions, strip_comments), 1) {
            out.strip_comments = (*options).strip_comments != 0;
        }
        if covers(offset_of!(FigSerializeOptions, lossless), 1) {
            out.lossless = (*options).lossless != 0;
        }
        if covers(offset_of!(FigSerializeOptions, width), size_of::<u16>()) {
            out.width = if (*options).width == 0 { 80 } else { (*options).width };
        }
        if covers(offset_of!(FigSerializeOptions, flow), 1) {
            out.flow = (*options).flow != 0;
        }
        if covers(offset_of!(FigSerializeOptions, splice), 1) {
            out.splice = (*options).splice != 0;
        }
    }
    Some(out)
}

/// `fig_document_serialize`: the whole document in `format`, borrowed from
/// the handle until the next serialize on it or its destroy.
///
/// # Safety
///
/// `doc` must be null or a live handle, `options` null or a caller's
/// record, the out pointers null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_document_serialize(
    doc: *mut FigDocument,
    format: c_int,
    options: *const FigSerializeOptions,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> FigStatus {
    if out_ptr.is_null() || out_len.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle_mut(doc) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    let opts = unsafe { options_of(options) };
    init();
    match h.doc.serialize(format, opts.as_ref()) {
        // SAFETY: both out pointers checked non-null above.
        Ok(bytes) => unsafe {
            out_bytes(bytes, out_ptr, out_len);
            FigStatus::Ok
        },
        Err(s) => s.into(),
    }
}

// ============================================================================
// DIAGNOSTICS
// ============================================================================

/// `writeWarning`: `w` into the caller's `FigWarning`, each field its
/// `size` covers. The pointers borrow `w`'s storage.
///
/// # Safety
///
/// `out` must be a caller's `FigWarning` of at least its `size` bytes.
pub(crate) unsafe fn write_warning(out: *mut FigWarning, w: &Warning) {
    use core::mem::{offset_of, size_of};
    let code = match w.code {
        Code::CommentDropped => 0,
        Code::CommentStyleDegraded => 1,
        Code::ValueDropped => 2,
        Code::TypeDegraded => 3,
    };
    let cause = match w.cause {
        Cause::FormatLimitation => 0,
        Cause::ExplicitOption => 1,
    };
    // SAFETY: by the caller's word `out` is a caller's record whose `size`
    // says which fields it laid out; only those are written, by raw pointer.
    unsafe {
        let size = (*out).size as usize;
        let covers = |offset: usize, len: usize| size >= offset + len;
        if covers(offset_of!(FigWarning, code), size_of::<c_int>()) {
            (*out).code = code;
        }
        if covers(offset_of!(FigWarning, cause), size_of::<c_int>()) {
            (*out).cause = cause;
        }
        if covers(offset_of!(FigWarning, path), size_of::<*const u8>()) {
            (*out).path = w.path.as_ptr();
        }
        if covers(offset_of!(FigWarning, path_len), size_of::<usize>()) {
            (*out).path_len = w.path.len();
        }
        if covers(offset_of!(FigWarning, note), size_of::<*const u8>()) {
            (*out).note = w.note.as_ptr();
        }
        if covers(offset_of!(FigWarning, note_len), size_of::<usize>()) {
            (*out).note_len = w.note.len();
        }
    }
}

/// `fig_document_diagnose`: how many lossy events serializing the document
/// to `format` would produce; the set is kept for `fig_document_warning`.
///
/// # Safety
///
/// `doc` must be null or a live handle, `options` null or a caller's
/// record, `out_count` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_document_diagnose(
    doc: *mut FigDocument,
    format: c_int,
    options: *const FigSerializeOptions,
    out_count: *mut usize,
) -> FigStatus {
    if out_count.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: non-null, writable by the caller's word.
    unsafe { *out_count = 0 };
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle_mut(doc) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    let opts = unsafe { options_of(options) };
    init();
    match h.doc.diagnose(format, opts.as_ref()) {
        Ok(n) => {
            // SAFETY: checked non-null above.
            unsafe { put(out_count, n) };
            FigStatus::Ok
        }
        Err(s) => s.into(),
    }
}

/// `fig_document_warning`: the event at `index` of the latest diagnose.
///
/// # Safety
///
/// `doc` must be null or a live handle, `out` null or a caller's
/// `FigWarning` with `size` set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_document_warning(doc: *mut FigDocument, index: usize, out: *mut FigWarning) -> FigStatus {
    if out.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: the caller's contract.
    let Some(h) = (unsafe { handle(doc) }) else { return FigStatus::InvalidArgument };
    match h.doc.warning(index) {
        Ok(w) => {
            // SAFETY: `out` is the caller's record, checked non-null.
            unsafe { write_warning(out, w) };
            FigStatus::Ok
        }
        Err(s) => s.into(),
    }
}
