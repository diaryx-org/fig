//! Editors: `fig_editor_*`. A `FigEditor` is a boxed
//! `fig_core::api::Editor` and the buffer the comment reads lend out; each
//! export decodes its arguments as `src/c_api.zig` does (`decodePath`,
//! `decodeKeys`, `decodeIndices`, `sliceOf`, every malformed one
//! `FIG_STATUS_INVALID_ARGUMENT`) and calls the editor.

use core::ffi::c_int;

use fig_core::api::{self, MAX_KEYS_LEN, MAX_PATH_LEN};
use fig_core::kit::ast::PathSegment;

use crate::abi::{FigPathSegment, FigStatus, FigStr, FIG_SEGMENT_INDEX, FIG_SEGMENT_KEY};
use crate::{array_of, init, slice_of};

/// `FigEditor`: opaque to C.
pub struct FigEditor {
    ed: api::Editor,
    /// What the comment reads lend out: refilled per read, valid until the
    /// next read on this handle or its destroy.
    scratch: Vec<u8>,
}

/// Run an export's body, its `Err` the status.
fn run(f: impl FnOnce() -> Result<(), FigStatus>) -> FigStatus {
    match f() {
        Ok(()) => FigStatus::Ok,
        Err(s) => s,
    }
}

fn done(r: Result<(), api::Status>) -> Result<(), FigStatus> {
    r.map_err(FigStatus::from)
}

/// The handle behind `ed`, or `InvalidArgument` for null.
///
/// # Safety
///
/// A non-null `ed` must be a live handle from `fig_editor_create`, used by
/// this call alone (fig.h's threading rule).
unsafe fn editor<'a>(ed: *mut FigEditor) -> Result<&'a mut FigEditor, FigStatus> {
    // SAFETY: by the caller's word a non-null `ed` is live and unshared.
    unsafe { ed.as_mut() }.ok_or(FigStatus::InvalidArgument)
}

/// `decodePath`: the C path as fig-core's, or `InvalidArgument` for a
/// path over `MAX_PATH_LEN`, a null array with a length, a key segment
/// with no key pointer, or an unknown kind.
///
/// # Safety
///
/// A non-null `ptr` must be readable for `len` segments, each key
/// readable for its `key_len` bytes, for `'a`.
unsafe fn path_of<'a>(ptr: *const FigPathSegment, len: usize) -> Result<Vec<PathSegment<'a>>, FigStatus> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if len > MAX_PATH_LEN {
        return Err(FigStatus::InvalidArgument);
    }
    // SAFETY: the caller's contract.
    let segs = unsafe { array_of(ptr, len) }.ok_or(FigStatus::InvalidArgument)?;
    let mut out = Vec::with_capacity(len);
    for s in segs {
        out.push(match s.kind {
            FIG_SEGMENT_KEY => {
                if s.key_ptr.is_null() {
                    return Err(FigStatus::InvalidArgument);
                }
                // SAFETY: non-null, and readable for `key_len` bytes by the
                // caller's word.
                PathSegment::Key(unsafe { core::slice::from_raw_parts(s.key_ptr, s.key_len) })
            }
            FIG_SEGMENT_INDEX => PathSegment::Index(s.index),
            _ => return Err(FigStatus::InvalidArgument),
        });
    }
    Ok(out)
}

/// `decodeKeys`: a `FigStr` array as byte slices, or `InvalidArgument` for
/// a list over `MAX_KEYS_LEN`, a null array with a length, or an entry
/// with bytes and no pointer. A zero-length entry is the empty key.
///
/// # Safety
///
/// A non-null `ptr` must be readable for `len` entries, each readable for
/// its `len` bytes, for `'a`.
unsafe fn keys_of<'a>(ptr: *const FigStr, len: usize) -> Result<Vec<&'a [u8]>, FigStatus> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if len > MAX_KEYS_LEN {
        return Err(FigStatus::InvalidArgument);
    }
    // SAFETY: the caller's contract.
    let ks = unsafe { array_of(ptr, len) }.ok_or(FigStatus::InvalidArgument)?;
    // SAFETY: each entry is readable for its length by the caller's word.
    ks.iter().map(|k| unsafe { slice_of(k.ptr, k.len) }.ok_or(FigStatus::InvalidArgument)).collect()
}

/// `sliceOf`, as a `Result`.
///
/// # Safety
///
/// As `slice_of`.
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Result<&'a [u8], FigStatus> {
    // SAFETY: the caller's contract.
    unsafe { slice_of(ptr, len) }.ok_or(FigStatus::InvalidArgument)
}

/// `fig_editor_create`: an editor over a copy of `input` in `format`.
///
/// # Safety
///
/// `input` must be readable for `input_len` bytes (or null with zero),
/// `out_editor` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_create(input: *const u8, input_len: usize, format: c_int, out_editor: *mut *mut FigEditor) -> FigStatus {
    if out_editor.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: non-null, writable by the caller's word.
    unsafe { *out_editor = core::ptr::null_mut() };
    // SAFETY: the caller's contract.
    let Some(src) = (unsafe { slice_of(input, input_len) }) else { return FigStatus::InvalidArgument };
    init();
    match api::Editor::create(src, format) {
        Ok(ed) => {
            // SAFETY: checked non-null above.
            unsafe { *out_editor = Box::into_raw(Box::new(FigEditor { ed, scratch: Vec::new() })) };
            FigStatus::Ok
        }
        Err(s) => s.into(),
    }
}

/// `fig_editor_destroy`. Null is a no-op.
///
/// # Safety
///
/// `ed` must be null or a live handle, not used again after.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_destroy(ed: *mut FigEditor) {
    if !ed.is_null() {
        // SAFETY: a live handle is a `Box` this library made; the caller
        // gives it up.
        drop(unsafe { Box::from_raw(ed) });
    }
}

/// `fig_editor_source`: the current source, borrowed until the next
/// mutation or destroy.
///
/// # Safety
///
/// `ed` must be null or a live handle, the out pointers null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_source(ed: *const FigEditor, out_ptr: *mut *const u8, out_len: *mut usize) -> FigStatus {
    if out_ptr.is_null() || out_len.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: by the caller's word a non-null `ed` is live.
    let Some(h) = (unsafe { ed.as_ref() }) else { return FigStatus::InvalidArgument };
    let src = h.ed.source();
    // SAFETY: both checked non-null above.
    unsafe {
        *out_ptr = src.as_ptr();
        *out_len = src.len();
    }
    FigStatus::Ok
}

/// One edit at a path with one byte argument.
macro_rules! path_bytes_edit {
    ($(#[$doc:meta])* $name:ident, $method:ident, $arg:ident) => {
        $(#[$doc])*
        ///
        /// # Safety
        ///
        /// `ed` must be null or a live handle; `path` readable for
        /// `path_len` segments; the byte argument readable for its length.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            ed: *mut FigEditor,
            path: *const FigPathSegment,
            path_len: usize,
            $arg: *const u8,
            len: usize,
        ) -> FigStatus {
            run(|| {
                // SAFETY: the caller's contract, for each argument.
                let h = unsafe { editor(ed) }?;
                let p = unsafe { path_of(path, path_len) }?;
                let b = unsafe { bytes($arg, len) }?;
                done(h.ed.$method(&p, b))
            })
        }
    };
}

/// One edit at a path with no other argument.
macro_rules! path_edit {
    ($(#[$doc:meta])* $name:ident, $method:ident) => {
        $(#[$doc])*
        ///
        /// # Safety
        ///
        /// `ed` must be null or a live handle; `path` readable for
        /// `path_len` segments.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(ed: *mut FigEditor, path: *const FigPathSegment, path_len: usize) -> FigStatus {
            run(|| {
                // SAFETY: the caller's contract, for each argument.
                let h = unsafe { editor(ed) }?;
                let p = unsafe { path_of(path, path_len) }?;
                done(h.ed.$method(&p))
            })
        }
    };
}

path_bytes_edit!(
    /// `fig_editor_replace_val`: splice `repl` as the value at `path`.
    fig_editor_replace_val, replace_val, repl
);
path_bytes_edit!(
    /// `fig_editor_replace_key`: rename the key at `path` by its syntax.
    fig_editor_replace_key, replace_key, repl
);
path_bytes_edit!(
    /// `fig_editor_replace_named_key`: rename the key at `path` by its name.
    fig_editor_replace_named_key, replace_named_key, name
);
path_bytes_edit!(
    /// `fig_editor_set`: upsert the value at `path`.
    fig_editor_set, set, val
);
path_bytes_edit!(
    /// `fig_editor_add_leading_comment`.
    fig_editor_add_leading_comment, add_leading_comment, text
);
path_bytes_edit!(
    /// `fig_editor_set_trailing_comment`.
    fig_editor_set_trailing_comment, set_trailing_comment, text
);
path_bytes_edit!(
    /// `fig_editor_add_dangling_comment`.
    fig_editor_add_dangling_comment, add_dangling_comment, text
);
path_bytes_edit!(
    /// `fig_editor_append_seq`.
    fig_editor_append_seq, append_seq, val
);
path_bytes_edit!(
    /// `fig_editor_prepend_seq`.
    fig_editor_prepend_seq, prepend_seq, val
);
path_bytes_edit!(
    /// `fig_editor_insert_container`: a scattered container at `path`
    /// whose entries are `body`.
    fig_editor_insert_container, insert_container, body
);
path_bytes_edit!(
    /// `fig_editor_rename_container`: on every line that names it.
    fig_editor_rename_container, rename_container, new_leaf
);
path_bytes_edit!(
    /// `fig_editor_append_container_to_seq`: TOML's `[[header]]` append.
    fig_editor_append_container_to_seq, append_container_to_seq, body
);

path_edit!(
    /// `fig_editor_delete_leading_comments`.
    fig_editor_delete_leading_comments, delete_leading_comments
);
path_edit!(
    /// `fig_editor_delete_trailing_comment`.
    fig_editor_delete_trailing_comment, delete_trailing_comment
);
path_edit!(
    /// `fig_editor_delete_dangling_comments`.
    fig_editor_delete_dangling_comments, delete_dangling_comments
);
path_edit!(
    /// `fig_editor_comment_out`.
    fig_editor_comment_out, comment_out
);
path_edit!(
    /// `fig_editor_delete_key`.
    fig_editor_delete_key, delete_key
);
path_edit!(
    /// `fig_editor_delete_container`: every scattered region of it.
    fig_editor_delete_container, delete_container
);

/// `fig_editor_replace_string`: `repl` is the caller's spelling of `str`;
/// a replaced string keeps its quote style where that style holds it.
///
/// # Safety
///
/// `ed` must be null or a live handle; `path` readable for `path_len`
/// segments; `repl` and `str` readable for their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_replace_string(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    repl: *const u8,
    repl_len: usize,
    str: *const u8,
    str_len: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        let r = unsafe { bytes(repl, repl_len) }?;
        let s = unsafe { bytes(str, str_len) }?;
        done(h.ed.replace_string(&p, r, s))
    })
}

/// `fig_editor_set_string`: `fig_editor_set` for a string the caller
/// rendered.
///
/// # Safety
///
/// As `fig_editor_replace_string`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_set_string(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    val: *const u8,
    val_len: usize,
    str: *const u8,
    str_len: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        let v = unsafe { bytes(val, val_len) }?;
        let s = unsafe { bytes(str, str_len) }?;
        done(h.ed.set_string(&p, v, s))
    })
}

/// Which comment a read asks for.
enum Anchor {
    Leading,
    Trailing,
    Dangling,
}

/// `editorGetComment` and `fillCommentOut`: the comment at `path`, copied
/// into the handle's scratch buffer and lent out; `NotFound` when there is
/// none.
///
/// # Safety
///
/// As the three exports below.
unsafe fn get_comment(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
    which: Anchor,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        if out_ptr.is_null() || out_len.is_null() {
            return Err(FigStatus::InvalidArgument);
        }
        let text = match which {
            Anchor::Leading => h.ed.get_leading_comment(&p),
            Anchor::Trailing => h.ed.get_trailing_comment(&p),
            Anchor::Dangling => h.ed.get_dangling_comment(&p),
        }?;
        h.scratch.clear();
        // A valid pointer even for a present but empty comment.
        h.scratch.reserve(text.len() + 1);
        h.scratch.extend_from_slice(&text);
        // SAFETY: both checked non-null above.
        unsafe {
            *out_ptr = h.scratch.as_ptr();
            *out_len = h.scratch.len();
        }
        Ok(())
    })
}

/// `fig_editor_get_leading_comment`: the own-line block above `path`.
///
/// # Safety
///
/// `ed` must be null or a live handle; `path` readable for `path_len`
/// segments; the out pointers null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_get_leading_comment(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> FigStatus {
    // SAFETY: the same arguments, under the same contract.
    unsafe { get_comment(ed, path, path_len, out_ptr, out_len, Anchor::Leading) }
}

/// `fig_editor_get_trailing_comment`: the value's same-line comment.
///
/// # Safety
///
/// As `fig_editor_get_leading_comment`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_get_trailing_comment(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> FigStatus {
    // SAFETY: the same arguments, under the same contract.
    unsafe { get_comment(ed, path, path_len, out_ptr, out_len, Anchor::Trailing) }
}

/// `fig_editor_get_dangling_comment`: the run at the end of a container's
/// body.
///
/// # Safety
///
/// As `fig_editor_get_leading_comment`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_get_dangling_comment(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> FigStatus {
    // SAFETY: the same arguments, under the same contract.
    unsafe { get_comment(ed, path, path_len, out_ptr, out_len, Anchor::Dangling) }
}

/// `fig_editor_uncomment_leading`.
///
/// # Safety
///
/// `ed` must be null or a live handle; `path` readable for `path_len`
/// segments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_uncomment_leading(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    first_line: usize,
    line_count: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        done(h.ed.uncomment_leading(&p, first_line, line_count))
    })
}

/// `fig_editor_uncomment_dangling`.
///
/// # Safety
///
/// As `fig_editor_uncomment_leading`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_uncomment_dangling(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    first_line: usize,
    line_count: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        done(h.ed.uncomment_dangling(&p, first_line, line_count))
    })
}

/// `fig_editor_insert_key`: `key` is key syntax, `val` value syntax.
///
/// # Safety
///
/// `ed` must be null or a live handle; `path` readable for `path_len`
/// segments; `key` and `val` readable for their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_insert_key(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    key: *const u8,
    key_len: usize,
    val: *const u8,
    val_len: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        let k = unsafe { bytes(key, key_len) }?;
        let v = unsafe { bytes(val, val_len) }?;
        done(h.ed.insert_key(&p, k, v))
    })
}

/// `fig_editor_insert_named_key`: `name` is the key's name.
///
/// # Safety
///
/// As `fig_editor_insert_key`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_insert_named_key(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    name: *const u8,
    name_len: usize,
    val: *const u8,
    val_len: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        let n = unsafe { bytes(name, name_len) }?;
        let v = unsafe { bytes(val, val_len) }?;
        done(h.ed.insert_named_key(&p, n, v))
    })
}

/// `fig_editor_insert_seq_item`: `val` becomes the item at `index`.
///
/// # Safety
///
/// `ed` must be null or a live handle; `path` readable for `path_len`
/// segments; `val` readable for `val_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_insert_seq_item(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    index: usize,
    val: *const u8,
    val_len: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        let v = unsafe { bytes(val, val_len) }?;
        done(h.ed.insert_seq_item(&p, index, v))
    })
}

/// `fig_editor_remove_seq_item`.
///
/// # Safety
///
/// `ed` must be null or a live handle; `path` readable for `path_len`
/// segments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_remove_seq_item(ed: *mut FigEditor, path: *const FigPathSegment, path_len: usize, index: usize) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        done(h.ed.remove_seq_item(&p, index))
    })
}

/// `fig_editor_move_key`: the entry at `src` to just before `dest`.
///
/// # Safety
///
/// `ed` must be null or a live handle; each path readable for its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_move_key(
    ed: *mut FigEditor,
    src_path: *const FigPathSegment,
    src_path_len: usize,
    dest_path: *const FigPathSegment,
    dest_path_len: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let src = unsafe { path_of(src_path, src_path_len) }?;
        let dest = unsafe { path_of(dest_path, dest_path_len) }?;
        done(h.ed.move_key(&src, &dest))
    })
}

/// `fig_editor_reorder_keys`: `keys` first, in order.
///
/// # Safety
///
/// `ed` must be null or a live handle; `path` readable for `path_len`
/// segments; `keys` readable for `keys_len` entries, each for its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_reorder_keys(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    keys: *const FigStr,
    keys_len: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        let k = unsafe { keys_of(keys, keys_len) }?;
        done(h.ed.reorder_keys(&p, &k))
    })
}

/// `fig_editor_move_item`: array-move semantics.
///
/// # Safety
///
/// `ed` must be null or a live handle; `path` readable for `path_len`
/// segments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_move_item(ed: *mut FigEditor, path: *const FigPathSegment, path_len: usize, from: usize, to: usize) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        done(h.ed.move_item(&p, from, to))
    })
}

/// `fig_editor_reorder_items`: the items at `indices` first, in order.
///
/// # Safety
///
/// `ed` must be null or a live handle; `path` readable for `path_len`
/// segments; `indices` readable for `indices_len` entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_reorder_items(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    indices: *const usize,
    indices_len: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        let i = unsafe { array_of(indices, indices_len) }.ok_or(FigStatus::InvalidArgument)?;
        done(h.ed.reorder_items(&p, i))
    })
}

/// `fig_editor_set_sequence`: the sequence at `path` made exactly `items`.
///
/// # Safety
///
/// `ed` must be null or a live handle; `path` readable for `path_len`
/// segments; `items` readable for `items_len` entries, each for its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_set_sequence(
    ed: *mut FigEditor,
    path: *const FigPathSegment,
    path_len: usize,
    items: *const FigStr,
    items_len: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let p = unsafe { path_of(path, path_len) }?;
        let i = unsafe { keys_of(items, items_len) }?;
        done(h.ed.set_sequence(&p, &i))
    })
}

/// `fig_editor_move_container`: a null `dest_path` is the end of the
/// document, distinct from a zero-length path (the root).
///
/// # Safety
///
/// `ed` must be null or a live handle; each path null or readable for its
/// length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_move_container(
    ed: *mut FigEditor,
    src_path: *const FigPathSegment,
    src_path_len: usize,
    dest_path: *const FigPathSegment,
    dest_path_len: usize,
) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let src = unsafe { path_of(src_path, src_path_len) }?;
        let dest = if dest_path.is_null() { None } else { Some(unsafe { path_of(dest_path, dest_path_len) }?) };
        done(h.ed.move_container(&src, dest.as_deref()))
    })
}

/// `fig_editor_reorder_containers`: those named in `order` first.
///
/// # Safety
///
/// `ed` must be null or a live handle; `order` readable for `order_len`
/// entries, each for its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_editor_reorder_containers(ed: *mut FigEditor, order: *const FigStr, order_len: usize) -> FigStatus {
    run(|| {
        // SAFETY: the caller's contract, for each argument.
        let h = unsafe { editor(ed) }?;
        let o = unsafe { keys_of(order, order_len) }?;
        done(h.ed.reorder_containers(&o))
    })
}
