//! The language as a static library of its own (`kit/src/c_language.zig`):
//! a `FigLanguageVTable` built from the `Language` (`kit/src/contract.zig`'s
//! `VTable`, the inverse of `fig-capi`'s `CLanguage`), registered through
//! fig's `fig_language_register`, which `libfig.a` resolves at link.
//!
//! The record is built once, at the first registration, and lives for the
//! process, as fig.h requires of a registered language's. Its `ctx` is the
//! language and its dialect rows, which every function is handed back.

use core::ffi::{c_char, c_int, c_void, CStr};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use fig_abi::*;
use fig_core::kit::ast::ExtKind;
use fig_core::kit::language::{Language, NativeKinds, RenderRequest, Syntax};

use crate::lang::{self, Refusal, Row};

unsafe extern "C" {
    fn fig_language_register(vt: *const FigLanguageVTable, out_format: *mut c_int, out_err: *mut FigError) -> c_int;
    fn fig_table_extend(t: *mut c_void, which: c_int, n: usize) -> *mut c_void;
    fn fig_format_by_name(name: *const c_char) -> c_int;
}

/// `FIG_LANGUAGE_VTABLE_VERSION`.
const VTABLE_VERSION: u32 = 3;

const CAP_READ: u32 = 1 << 0;
const CAP_EDIT: u32 = 1 << 1;
const CAP_SERIALIZE: u32 = 1 << 2;
const CAP_REFERENCES: u32 = 1 << 3;

/// One language's registration: its record, built at the first.
pub struct Registration {
    /// A spin lock: a registration is rare, and two threads registering at
    /// once wait for each other, the second finding the first's.
    locked: AtomicBool,
    vtable: AtomicPtr<FigLanguageVTable>,
}

impl Registration {
    pub const fn new() -> Registration {
        Registration { locked: AtomicBool::new(false), vtable: AtomicPtr::new(core::ptr::null_mut()) }
    }

    /// `fig_<name>_register`.
    ///
    /// # Safety
    ///
    /// As `fig_language_register`'s for `out_format` and `out_err`.
    pub unsafe fn register(&self, l: &'static dyn Language, out_format: *mut c_int, out_err: *mut FigError) -> c_int {
        while self.locked.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        let mut vt = self.vtable.load(Ordering::Relaxed);
        if vt.is_null() {
            vt = Box::leak(Box::new(vtable(l)));
            self.vtable.store(vt, Ordering::Relaxed);
        }
        // SAFETY: `vt` is the record built above, which lives for the
        // process; the out-parameters are the caller's.
        let rc = unsafe {
            let known = fig_format_by_name((*vt).name);
            if known >= 0 {
                *out_format = known;
                0
            } else {
                fig_language_register(vt, out_format, out_err)
            }
        };
        self.locked.store(false, Ordering::Release);
        rc
    }
}

/// What every function is handed back as its `ctx`.
struct Ctx {
    lang: &'static dyn Language,
    rows: Vec<Row>,
}

/// # Safety
///
/// `ctx` is the record's own, which `vtable` made.
unsafe fn ctx<'a>(ctx: *mut c_void) -> &'a Ctx {
    // SAFETY: the caller's contract.
    unsafe { &*ctx.cast::<Ctx>() }
}

/// A NUL-terminated copy of `s` up to its first NUL, for the life of the
/// process (Zig's comptime `z`).
fn z(s: &[u8]) -> *const c_char {
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    CString::new(&s[..end]).map_or(core::ptr::null(), |c| c.into_raw().cast_const())
}

fn z_opt(s: Option<&[u8]>) -> *const c_char {
    s.map_or(core::ptr::null(), z)
}

/// The bytes a `FigStr` fig handed over names.
///
/// # Safety
///
/// `s` names `s.len` readable bytes, or is null or `FIG_LEN_NONE`.
unsafe fn bytes_of<'a>(s: FigStr) -> &'a [u8] {
    if s.ptr.is_null() || s.len == 0 || s.len == FIG_LEN_NONE {
        return &[];
    }
    // SAFETY: the caller's contract.
    unsafe { core::slice::from_raw_parts(s.ptr, s.len) }
}

/// Bytes handed to fig, freed with `free_bytes`.
fn give(bytes: Vec<u8>) -> FigStr {
    if bytes.is_empty() {
        return FigStr::EMPTY;
    }
    let b = Box::leak(bytes.into_boxed_slice());
    FigStr { ptr: b.as_ptr(), len: b.len() }
}

/// `ErrorInfo.set`: the message (cut to what the record holds) and offset.
///
/// # Safety
///
/// `err` is null or a `FigError` of at least its `size` bytes.
unsafe fn fail(err: *mut FigError, r: &Refusal) {
    if err.is_null() {
        return;
    }
    // SAFETY: the caller's contract; fig hands a whole record.
    let e = unsafe { &mut *err };
    let n = r.message.len().min(e.message.len());
    e.message[..n].copy_from_slice(&r.message[..n]);
    e.message_len = n;
    e.byte_offset = r.byte_offset;
    if r.code != 0 {
        e.code = r.code;
    }
}

/// `losslessBits`: the envelope, null, and one bit per `ExtKind`.
fn lossless_bits(k: Option<NativeKinds>) -> u32 {
    let Some(k) = k else { return 0 };
    let mut bits = 1 | if k.null { 1 << 1 } else { 0 };
    for e in ExtKind::ALL {
        if k.ext(e) {
            bits |= 1 << (2 + e.ordinal() as u32);
        }
    }
    bits
}

fn delimiter(d: Option<fig_core::kit::language::CommentDelimiter>) -> FigCommentDelimiter {
    match d {
        Some(d) => FigCommentDelimiter { open: z(d.open), close: z(d.close), forbidden: z_opt(d.forbidden) },
        None => FigCommentDelimiter { open: core::ptr::null(), close: core::ptr::null(), forbidden: core::ptr::null() },
    }
}

/// `syntaxDesc`: the record of `s`, every string copied.
fn syntax_desc(s: &Syntax) -> FigSyntax {
    let mut out = FigSyntax::DEFAULT;
    out.comments = FigComments { line: delimiter(s.comments.line), trailing: delimiter(s.comments.trailing), block: delimiter(s.comments.block) };
    out.kv_sep = z_opt(s.kv_sep);
    out.flow_kv_sep_from_siblings = s.flow_kv_sep_from_siblings as u8;
    out.flow_map_pad = z(s.flow_map_pad);
    out.key_sigil = s.key_sigil.unwrap_or(0);
    out.empty_map_literal = z_opt(s.empty_map_literal);
    out.block_seq_editable = s.block_seq_editable as u8;
    out.flow_containers = s.flow_containers as u8;
    out.indent_unit = z(s.indent_unit);
    out.seq_item_marker = z(s.seq_item_marker);
    if let Some(cc) = &s.closed_containers {
        out.closed_containers =
            FigClosedContainers { map_open: z(cc.map.open), map_close: z(cc.map.close), seq_open: z(cc.seq.open), seq_close: z(cc.seq.close) };
    }
    out.single_line_block_mapping = s.single_line_block_mapping as u8;
    out.bare_document_mapping = s.bare_document_mapping as u8;
    out.flow_map_open = z(s.flow_map_open);
    out.flow_map_close = z(s.flow_map_close);
    out.structural_indent = s.structural_indent as u8;
    out.section_noun = z_opt(s.section_noun);
    if let Some(h) = &s.section_header {
        out.section_header = FigSectionHeader {
            open: z(h.open),
            close: z(h.close),
            seq_open: z_opt(h.seq_open),
            seq_close: z_opt(h.seq_close),
            sep: z(h.sep),
            skip_index: h.skip_index as u8,
        };
    }
    out.merge_key = z_opt(s.merge_key);
    out.flow_entry_sep = z(s.flow_entry_sep);
    out.flow_root = s.flow_root as u8;
    out.flow_maps_only = s.flow_maps_only as u8;
    out.line_prefix_marks = z(s.line_prefix_marks);
    out.flow_map_kv_sep = z(s.flow_map_kv_sep);
    out.flow_map_entry_sep = z(s.flow_map_entry_sep);
    out.line_continuation = s.line_continuation.unwrap_or(0);
    out.empty_seq_literal = z_opt(s.empty_seq_literal);
    out.opener_line_comment = s.opener_line_comment as u8;
    out
}

fn leak<T>(v: Vec<T>) -> *const T {
    Box::leak(v.into_boxed_slice()).as_ptr()
}

/// The record of `l` (`VTable(L, "")`'s `vtable`).
fn vtable(l: &'static dyn Language) -> FigLanguageVTable {
    let caps = l.caps();
    let rows = lang::rows(l);
    let dialects_of = l.dialects();
    let syntaxes: Vec<*const FigSyntax> = rows
        .iter()
        .map(|r| if caps.edit { Box::leak(Box::new(syntax_desc(&l.syntax(r.dialect)))) as *const FigSyntax } else { core::ptr::null() })
        .collect();
    let dialects: Vec<FigDialectDesc> = rows
        .iter()
        .zip(&syntaxes)
        .map(|(r, &syntax)| {
            let d = &dialects_of[r.of];
            // Every extension of the language goes to the dialect named
            // after it, else to the first (`extensionsOf`).
            let extensions = if r.declared {
                let mut exts: Vec<*const c_char> = l
                    .extensions()
                    .iter()
                    .filter(|e| dialects_of.iter().rposition(|dd| dd.name == **e).unwrap_or(0) == r.of)
                    .map(|e| z(e.as_bytes()))
                    .collect();
                exts.push(core::ptr::null());
                leak(exts)
            } else {
                core::ptr::null()
            };
            let p = d.prints;
            let mut flags = 0;
            if let Some(x) = &p {
                if !x.typed_scalars {
                    flags |= FIG_PRINTS_TEXT_SCALARS;
                }
                if x.null_at_root {
                    flags |= FIG_PRINTS_NULL_AT_ROOT;
                }
                if x.root_container {
                    flags |= FIG_PRINTS_ROOT_CONTAINER;
                }
            }
            FigDialectDesc {
                name: z(&r.name),
                extensions,
                splice: d.splice.ordinal() as c_int,
                empty_doc_seed: z_opt(d.empty_doc_seed),
                syntax,
                prints_holds: p.as_ref().map_or(0, |x| lossless_bits(Some(x.holds))),
                prints_flags: flags,
                prints_char_as: p.as_ref().map_or(core::ptr::null(), |x| z(x.char_as.as_bytes())),
                prints_number_special_as: p.as_ref().map_or(core::ptr::null(), |x| z(x.number_special_as.as_bytes())),
                prints_comments: p.as_ref().map_or(0, |x| x.comments.ordinal() as c_int),
            }
        })
        .collect();
    let samples: Vec<FigStr> = l.samples().iter().map(|s| FigStr::of(s)).collect();
    let mut caps_bits = 0;
    for (on, bit) in [(caps.read, CAP_READ), (caps.edit, CAP_EDIT), (caps.serialize, CAP_SERIALIZE), (caps.references, CAP_REFERENCES)] {
        if on {
            caps_bits |= bit;
        }
    }
    let renderer = |i: usize, f: FigRenderFn| if l.has_renderer(l.default_dialect(), lang::RENDERERS[i]) { Some(f) } else { None };
    FigLanguageVTable {
        version: VTABLE_VERSION,
        size: core::mem::size_of::<FigLanguageVTable>() as u32,
        name: z(dialects_of.first().map_or(&b""[..], |d| d.name.as_bytes())),
        caps: caps_bits,
        max_mapping_depth: caps.max_mapping_depth.map_or(FIG_DEPTH_NONE, c_int::from),
        lossless: lossless_bits(caps.lossless),
        syntax: syntaxes.first().copied().unwrap_or(core::ptr::null()),
        dialect_count: dialects.len(),
        dialect_size: core::mem::size_of::<FigDialectDesc>(),
        dialects: leak(dialects),
        sample_count: samples.len(),
        samples: leak(samples),
        parse: Some(parse),
        print: if caps.serialize { Some(print) } else { None },
        free_bytes: Some(free_bytes),
        render_value: renderer(0, render::<0>),
        render_entry: renderer(1, render::<1>),
        render_item: renderer(2, render::<2>),
        render_tail: renderer(3, render::<3>),
        render_key: renderer(4, render::<4>),
        render_key_name: renderer(5, render::<5>),
        render_string: renderer(6, render::<6>),
        split_stream: if l.has_split_stream() { Some(split_stream) } else { None },
        ctx: Box::leak(Box::new(Ctx { lang: l, rows })) as *mut Ctx as *mut c_void,
    }
}

/// The dialect a function was handed, by its row's name.
///
/// # Safety
///
/// `dialect` is null or NUL-terminated.
unsafe fn dialect_of(c: &Ctx, dialect: *const c_char) -> Option<u8> {
    if dialect.is_null() {
        return None;
    }
    // SAFETY: the caller's contract.
    lang::dialect_of(&c.rows, unsafe { CStr::from_ptr(dialect) }.to_bytes())
}

/// Parse into a table of this library's own, then copy each of its lists
/// into `out`, fig's — diagnostics too when the parse refuses.
unsafe extern "C" fn parse(ctx_: *mut c_void, dialect: *const c_char, input: FigStr, out: *mut FigTable, err: *mut FigError) -> c_int {
    // SAFETY: fig hands back the record's own `ctx`, a row name, its
    // input, its table and its error record, as fig.h states.
    unsafe {
        let c = ctx(ctx_);
        let Some(d) = dialect_of(c, dialect) else {
            fail(err, &Refusal::of(b"no such dialect"));
            return 2;
        };
        let (local, parsed) = lang::parse(c.lang, d, bytes_of(input));
        let out = out.cast::<c_void>();
        let copy = |which: c_int, bytes: &[u8], n: usize| -> bool {
            if n == 0 {
                return true;
            }
            let at = fig_table_extend(out, which, n);
            if at.is_null() {
                return false;
            }
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), at.cast::<u8>(), bytes.len());
            true
        };
        fn raw<T>(items: &[T]) -> &[u8] {
            // SAFETY: the table's lists are `#[repr(C)]` records of
            // integers, read as their bytes.
            unsafe { core::slice::from_raw_parts(items.as_ptr().cast::<u8>(), core::mem::size_of_val(items)) }
        }
        let copied = copy(0, raw(&local.rows), local.rows.len())
            && copy(1, &local.strings, local.strings.len())
            && copy(2, raw(&local.anchors), local.anchors.len())
            && copy(3, raw(&local.tags), local.tags.len())
            && copy(4, raw(&local.markers), local.markers.len())
            && copy(5, raw(&local.seps), local.seps.len())
            && copy(6, raw(&local.item_seps), local.item_seps.len())
            && copy(7, raw(&local.comments), local.comments.len())
            && copy(8, raw(&local.regions), local.regions.len())
            && copy(9, raw(&local.mentions), local.mentions.len())
            && copy(10, raw(&local.directives), local.directives.len())
            && copy(11, raw(&local.diagnostics), local.diagnostics.len());
        if !copied {
            return 3;
        }
        match parsed {
            Ok(()) => 0,
            Err(r) => {
                fail(err, &r);
                2
            }
        }
    }
}

unsafe extern "C" fn print(
    ctx_: *mut c_void,
    dialect: *const c_char,
    view: *const FigTableView,
    options: *const FigPrintOptions,
    out: *mut FigStr,
    err: *mut FigError,
) -> c_int {
    // SAFETY: as `parse`; the view and the options are fig's, valid for
    // the call.
    unsafe {
        let c = ctx(ctx_);
        let Some(d) = dialect_of(c, dialect) else {
            fail(err, &Refusal::of(b"no such dialect"));
            return 2;
        };
        let o = &*options;
        let opts = lang::serialize_options(o.pretty, o.strip_comments, o.indent, o.width, o.splice, o.flow);
        match lang::print(c.lang, d, &lang::view_of(&*view), &opts) {
            Ok(bytes) => {
                *out = give(bytes);
                0
            }
            Err(r) => {
                fail(err, &r);
                2
            }
        }
    }
}

unsafe extern "C" fn free_bytes(_ctx: *mut c_void, bytes: FigStr) {
    if bytes.ptr.is_null() || bytes.len == 0 || bytes.len == FIG_LEN_NONE {
        return;
    }
    // SAFETY: fig hands back what `give` leaked, once.
    drop(unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(bytes.ptr.cast_mut(), bytes.len)) });
}

/// The renderer in slot `W` of `lang::RENDERERS`.
unsafe extern "C" fn render<const W: usize>(ctx_: *mut c_void, r: *const FigRenderRequest, out: *mut FigStr, err: *mut FigError) -> c_int {
    // SAFETY: as `parse`; the request is fig's, of its `size` bytes.
    unsafe {
        let c = ctx(ctx_);
        let r = &*r;
        let Some(d) = dialect_of(c, r.dialect) else {
            fail(err, &Refusal::of(b"no such dialect"));
            return 2;
        };
        let literal = if r.literal.is_null() { b"string" as &[u8] } else { CStr::from_ptr(r.literal).to_bytes() };
        // Read only where fig's `size` covers it.
        let covers_old_value = r.size as usize >= core::mem::offset_of!(FigRenderRequest, old_value) + core::mem::size_of::<FigStr>();
        let request = RenderRequest {
            indent: bytes_of(r.indent),
            key: bytes_of(r.key),
            value: bytes_of(r.value),
            literal: lang::literal_of(literal),
            old_key: bytes_of(r.old_key),
            parent_key: bytes_of(r.parent_key),
            parent_tag: bytes_of(r.parent_tag),
            old_value: if covers_old_value { bytes_of(r.old_value) } else { &[] },
        };
        match lang::render(c.lang, lang::RENDERERS[W], d, &request) {
            Some(Ok(bytes)) => {
                *out = give(bytes);
                0
            }
            Some(Err(e)) => {
                fail(err, &e);
                6
            }
            None => {
                fail(err, &Refusal::of(b"this language has no such renderer"));
                6
            }
        }
    }
}

unsafe extern "C" fn split_stream(ctx_: *mut c_void, dialect: *const c_char, input: FigStr, out: *mut FigStr, err: *mut FigError) -> c_int {
    // SAFETY: as `parse`.
    unsafe {
        let c = ctx(ctx_);
        let Some(d) = dialect_of(c, dialect) else {
            fail(err, &Refusal::of(b"no such dialect"));
            return 2;
        };
        match lang::split_stream(c.lang, d, bytes_of(input)) {
            Some(Ok(docs)) => {
                let size = core::mem::size_of::<FigStreamDoc>();
                let mut bytes = vec![0u8; docs.len() * size];
                for (i, doc) in docs.iter().enumerate() {
                    core::ptr::write_unaligned(bytes.as_mut_ptr().add(i * size).cast::<FigStreamDoc>(), *doc);
                }
                *out = give(bytes);
                0
            }
            Some(Err(e)) => {
                fail(err, &e);
                2
            }
            None => {
                fail(err, &Refusal::of(b"this language has no streams"));
                2
            }
        }
    }
}
