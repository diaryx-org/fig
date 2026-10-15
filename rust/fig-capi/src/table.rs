//! The node table a language's `parse` fills: `fig_table_*`. Fig allocated
//! it (a `fig_core::kit::table::Table`, opaque to C); these are how a
//! language writes into it, so no memory crosses from its allocator to
//! fig's. Every node index names a row already added; every list is
//! appended in node order, or put in it by `fig_table_finish`.

use core::ffi::{c_int, c_void};

use fig_core::kit::row_writer::RowWriter;
use fig_core::kit::table::{self, Anchor, Comment, Diagnostic, Directive, ItemSep, Kind, Mention, Row, SpanFact, Tag, Text};

use crate::abi::{FigStatus, FigTable};
use crate::slice_of;

/// The table behind `t`.
///
/// # Safety
///
/// A non-null `t` must be the table fig handed the running `parse`, used
/// by that call alone.
unsafe fn tbl<'a>(t: *mut FigTable) -> Option<&'a mut FigTable> {
    // SAFETY: the caller's contract.
    unsafe { t.as_mut() }
}

/// `tableNode`: the table, when `node` names one of its rows.
///
/// # Safety
///
/// As `tbl`.
unsafe fn tbl_node<'a>(t: *mut FigTable, node: u32) -> Result<&'a mut FigTable, FigStatus> {
    // SAFETY: the caller's contract.
    let t = unsafe { tbl(t) }.ok_or(FigStatus::InvalidArgument)?;
    if (node as usize) < t.rows.len() { Ok(t) } else { Err(FigStatus::InvalidArgument) }
}

fn status(r: Result<(), FigStatus>) -> FigStatus {
    r.err().unwrap_or(FigStatus::Ok)
}

/// `fig_table_rows`: `n` new rows, each a null with no parent, span or
/// text, to fill in place; null when out of memory. Valid until the next
/// call on `t`.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_rows(t: *mut FigTable, n: usize) -> *mut Row {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { tbl(t) }) else { return core::ptr::null_mut() };
    if t.rows.try_reserve(n).is_err() {
        return core::ptr::null_mut();
    }
    match t.add_rows(n) {
        Ok(rows) => rows.as_mut_ptr(),
        Err(_) => core::ptr::null_mut(),
    }
}

/// `fig_table_row`: row `id`, to read or change in place, or null.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_row(t: *mut FigTable, id: u32) -> *mut Row {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { tbl(t) }) else { return core::ptr::null_mut() };
    t.rows.get_mut(id as usize).map_or(core::ptr::null_mut(), |r| r as *mut Row)
}

/// `fig_table_finish`: rows written in any order put in pre-order from
/// `root` by their links (`RowWriter::finish`).
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_finish(t: *mut FigTable, root: u32) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { tbl(t) }) else { return FigStatus::InvalidArgument };
    let in_range = (root as usize) < t.rows.len();
    let mut w = RowWriter::init(t);
    if !in_range || !w.links_in_range() {
        return FigStatus::InvalidArgument;
    }
    match w.finish(root) {
        Ok(()) => FigStatus::Ok,
        Err(_) => FigStatus::OutOfMemory,
    }
}

/// `fig_table_finish_in_order`: rows written in pre-order from row 0 stay
/// where they are; only the comments are put in order.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_finish_in_order(t: *mut FigTable) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { tbl(t) }) else { return FigStatus::InvalidArgument };
    let mut w = RowWriter::init(t);
    if !w.links_in_range() {
        return FigStatus::InvalidArgument;
    }
    match w.is_in_order() {
        Err(_) => return FigStatus::OutOfMemory,
        Ok(false) => return FigStatus::InvalidArgument,
        Ok(true) => {}
    }
    match w.finish_in_order() {
        Ok(()) => FigStatus::Ok,
        Err(_) => FigStatus::OutOfMemory,
    }
}

/// `fig_table_child_by_key`: mapping `map`'s entry whose key's text is
/// `key`, found by walking its links, or `FIG_ROW_NONE`.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`; `key`
/// readable for `key_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_child_by_key(t: *const FigTable, map: u32, key: *const u8, key_len: usize) -> u32 {
    let none = table::NONE;
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { t.as_ref() }) else { return none };
    // SAFETY: the caller's contract.
    let Some(want) = (unsafe { slice_of(key, key_len) }) else { return none };
    let rows = &t.rows;
    if map as usize >= rows.len() {
        return none;
    }
    let mut e = rows[map as usize].first_child;
    let mut steps = 0usize;
    while e != none && (e as usize) < rows.len() && steps < rows.len() {
        let k = rows[e as usize].first_child;
        if rows[e as usize].kind == Kind::KEYVALUE && k != none && (k as usize) < rows.len() && t.text_of(rows[k as usize].text) == Some(want) {
            return e;
        }
        e = rows[e as usize].next_sibling;
        steps += 1;
    }
    none
}

/// `n` zeroed elements appended to `list`, their address; null when out
/// of memory. Zero elements are the list's start.
fn extend_zeroed<T: Copy>(list: &mut Vec<T>, n: usize) -> *mut c_void {
    if list.try_reserve(n).is_err() {
        return core::ptr::null_mut();
    }
    let len = list.len();
    // SAFETY: `try_reserve` made room for `n` more; every element type a
    // table list holds is integers alone (`#[repr(C)]` rows and facts, and
    // bytes), for which all-zero bytes are a value; so the `n` elements
    // past `len` are initialized before `set_len` counts them.
    unsafe {
        let at = list.as_mut_ptr().add(len);
        core::ptr::write_bytes(at, 0, n);
        list.set_len(len + n);
    }
    if n == 0 {
        list.as_mut_ptr().cast()
    } else {
        // SAFETY: `len` is within the list.
        unsafe { list.as_mut_ptr().add(len).cast() }
    }
}

/// `fig_table_extend`: `n` zeroed elements appended to list `which`
/// (`FigTableList`), returned to fill in place; null when `which` is not
/// a list or out of memory. Valid until the next call on `t`.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_extend(t: *mut FigTable, which: c_int, n: usize) -> *mut c_void {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { tbl(t) }) else { return core::ptr::null_mut() };
    match which {
        0 => {
            if t.rows.len().saturating_add(n) >= table::NONE as usize {
                return core::ptr::null_mut();
            }
            extend_zeroed::<Row>(&mut t.rows, n)
        }
        1 => extend_zeroed::<u8>(&mut t.strings, n),
        2 => extend_zeroed::<Anchor>(&mut t.anchors, n),
        3 => extend_zeroed::<Tag>(&mut t.tags, n),
        4 => extend_zeroed::<SpanFact>(&mut t.markers, n),
        5 => extend_zeroed::<SpanFact>(&mut t.seps, n),
        6 => extend_zeroed::<ItemSep>(&mut t.item_seps, n),
        7 => extend_zeroed::<Comment>(&mut t.comments, n),
        8 => extend_zeroed::<SpanFact>(&mut t.regions, n),
        9 => extend_zeroed::<Mention>(&mut t.mentions, n),
        10 => extend_zeroed::<Directive>(&mut t.directives, n),
        11 => extend_zeroed::<Diagnostic>(&mut t.diagnostics, n),
        _ => core::ptr::null_mut(),
    }
}

/// `fig_table_string`: `bytes` copied into the string buffer, its text to
/// `*out`.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`; `bytes`
/// readable for `len`; `out` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_string(t: *mut FigTable, bytes: *const u8, len: usize, out: *mut Text) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { tbl(t) }) else { return FigStatus::InvalidArgument };
    if out.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: the caller's contract.
    let Some(b) = (unsafe { slice_of(bytes, len) }) else { return FigStatus::InvalidArgument };
    match t.add_string(b) {
        Ok(text) => {
            // SAFETY: checked non-null; writable by the caller's word.
            unsafe { *out = text };
            FigStatus::Ok
        }
        Err(_) => FigStatus::OutOfMemory,
    }
}

/// `fig_table_anchor`: an anchor named `name` on `node`, its `&name` at
/// `[start, end)`.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_anchor(t: *mut FigTable, node: u32, start: u32, end: u32, name: Text) -> FigStatus {
    // SAFETY: the caller's contract.
    status(unsafe { tbl_node(t, node) }.map(|t| t.anchors.push(Anchor { node, start, end, name })))
}

/// `fig_table_tag`: a tag on `node` as written and as resolved.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_tag(t: *mut FigTable, node: u32, start: u32, end: u32, text: Text, resolved: u8, agrees: bool) -> FigStatus {
    // SAFETY: the caller's contract.
    status(unsafe { tbl_node(t, node) }.map(|t| t.tags.push(Tag { node, start, end, text, resolved, agrees: agrees as u8 })))
}

/// `fig_table_marker`: a block-sequence item's marker.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_marker(t: *mut FigTable, node: u32, start: u32, end: u32) -> FigStatus {
    // SAFETY: the caller's contract.
    status(unsafe { tbl_node(t, node) }.map(|t| t.markers.push(SpanFact { node, start, end })))
}

/// `fig_table_sep`: a keyvalue's separator.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_sep(t: *mut FigTable, node: u32, start: u32, end: u32) -> FigStatus {
    // SAFETY: the caller's contract.
    status(unsafe { tbl_node(t, node) }.map(|t| t.seps.push(SpanFact { node, start, end })))
}

/// `fig_table_item_sep`: a separated sequence's item separator.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_item_sep(t: *mut FigTable, node: u32, text: Text) -> FigStatus {
    // SAFETY: the caller's contract.
    status(unsafe { tbl_node(t, node) }.map(|t| t.item_seps.push(ItemSep { node, text })))
}

/// `fig_table_comment`: a comment on `node` in `slot`, of `style`.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_comment(t: *mut FigTable, node: u32, slot: u8, style: u8, text: Text) -> FigStatus {
    // SAFETY: the caller's contract.
    let t = match unsafe { tbl_node(t, node) } {
        Ok(t) => t,
        Err(s) => return s,
    };
    if slot > table::COMMENT_DANGLING || style > table::COMMENT_BLOCK {
        return FigStatus::InvalidArgument;
    }
    t.comments.push(Comment { node, slot, style, text });
    FigStatus::Ok
}

/// `fig_table_region`: a section's whole header line.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_region(t: *mut FigTable, node: u32, start: u32, end: u32) -> FigStatus {
    // SAFETY: the caller's contract.
    status(unsafe { tbl_node(t, node) }.map(|t| t.regions.push(SpanFact { node, start, end })))
}

/// `fig_table_mention`: a place a section's name or an item's path is
/// written.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_mention(t: *mut FigTable, node: u32, start: u32, end: u32, kind: u8) -> FigStatus {
    // SAFETY: the caller's contract.
    let t = match unsafe { tbl_node(t, node) } {
        Ok(t) => t,
        Err(s) => return s,
    };
    if kind > table::MENTION_APPEND {
        return FigStatus::InvalidArgument;
    }
    t.mentions.push(Mention { node, start, end, kind });
    FigStatus::Ok
}

/// `fig_table_directive`: a tag-handle declaration (YAML's `%TAG`).
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_directive(t: *mut FigTable, handle: Text, prefix: Text) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { tbl(t) }) else { return FigStatus::InvalidArgument };
    t.directives.push(Directive { handle, prefix });
    FigStatus::Ok
}

/// `fig_table_diagnostic`: what a parse found, an error or a warning.
///
/// # Safety
///
/// `t` must be null or the table fig handed the running `parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_table_diagnostic(t: *mut FigTable, severity: u8, start: u32, end: u32, message: Text, label: Text) -> FigStatus {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { tbl(t) }) else { return FigStatus::InvalidArgument };
    if severity > table::DIAGNOSTIC_WARNING {
        return FigStatus::InvalidArgument;
    }
    t.diagnostics.push(Diagnostic { severity, start, end, message, label });
    FigStatus::Ok
}
