//! Formats and runtime languages: `fig_language_register` and the lookups
//! over the registry (`fig_format_*`, `fig_detect_order`), and the stream
//! split (`fig_stream_*`).
//!
//! A `FigLanguageVTable` becomes a `CLanguage`, a `fig_core` `Language`
//! whose methods call the vtable: the record is read at the size its writer
//! states (`normalize`, `abi.gated`), held to `src/contract/runtime.zig`'s
//! `validateVTable` rules, every string it points to copied, and registered
//! through `fig_core::registry` — whose own validation and harness then run
//! over it, calling back into the language as any caller will.

use core::ffi::{c_char, c_int, c_void, CStr};
use std::ffi::CString;
use std::sync::{Arc, Mutex};

use fig_core::api;
use fig_core::err;
use fig_core::kit::ast::{ExtKind, NodeId, SerializeOptions, AST};
use fig_core::kit::convert;
use fig_core::kit::language::{
    self as language, Caps, ClosedContainers, CommentDelimiter, Comments, Delimiters, Dialect, DialectInfo, Language, Literal,
    NativeKinds, Prints, PrintsComments, RenderRequest, Renderer, SectionHeader, SpliceStyle, StreamDocument, Syntax,
};
use fig_core::kit::table::Table;
use fig_core::registry::{self, RegisterOptions};

use crate::abi::*;
use crate::{array_of, cstr, fill_error, init, slice_of, write_error};

/// `FIG_LANGUAGE_VTABLE_VERSION`.
pub const VTABLE_VERSION: u32 = 3;

const CAP_READ: u32 = 1 << 0;
const CAP_EDIT: u32 = 1 << 1;
const CAP_SERIALIZE: u32 = 1 << 2;
const CAP_REFERENCES: u32 = 1 << 3;
const CAP_KNOWN: u32 = CAP_READ | CAP_EDIT | CAP_SERIALIZE | CAP_REFERENCES;

const LOSSLESS_ENVELOPE: u32 = 1 << 0;
const NATIVE_NULL: u32 = 1 << 1;

fn native_ext(k: ExtKind) -> u32 {
    1 << (2 + k.ordinal() as u32)
}

/// `knownLosslessBits`: the envelope, null, and one bit per `ExtKind`.
fn known_lossless_bits() -> u32 {
    ExtKind::ALL.iter().fold(LOSSLESS_ENVELOPE | NATIVE_NULL, |bits, &k| bits | native_ext(k))
}

/// `nativeKindsOf`: the kinds `bits` holds natively, or `None` without the
/// envelope bit.
fn native_kinds_of(bits: u32) -> Option<NativeKinds> {
    if bits & LOSSLESS_ENVELOPE == 0 {
        return None;
    }
    let mut out = NativeKinds { null: bits & NATIVE_NULL != 0, ..NativeKinds::NONE };
    for k in ExtKind::ALL {
        out.set_ext(k, bits & native_ext(k) != 0);
    }
    Some(out)
}

// ============================================================================
// READING THE RECORD
// ============================================================================

/// A vtable read at this library's layout: the record, each dialect row,
/// and each syntax record, every one gated by its writer's size.
struct Normalized {
    vt: FigLanguageVTable,
    syntax: Option<FigSyntax>,
    dialects: Vec<FigDialectDesc>,
    dialect_syntax: Vec<Option<FigSyntax>>,
}

/// `normalizeSyntax`.
///
/// # Safety
///
/// `raw` must point to a syntax record of at least its `size` bytes.
unsafe fn normalize_syntax(raw: *const FigSyntax) -> Result<FigSyntax, String> {
    // SAFETY: `size` leads the record, which the caller states is readable.
    let size = unsafe { core::ptr::read_unaligned(raw.cast::<u32>()) } as usize;
    if size < SYNTAX_REQUIRED {
        return Err(format!("a syntax record's size {size} is too short to hold its required fields ({SYNTAX_REQUIRED})"));
    }
    let mut s = gated!(
        FigSyntax,
        raw,
        size,
        FigSyntax::DEFAULT,
        [
            size,
            comments,
            kv_sep,
            flow_kv_sep_from_siblings,
            flow_map_pad,
            key_sigil,
            empty_map_literal,
            block_seq_editable,
            flow_containers,
            indent_unit,
            seq_item_marker,
            closed_containers,
            single_line_block_mapping,
            bare_document_mapping,
            flow_map_open,
            flow_map_close,
            structural_indent,
            section_noun,
            section_header,
            merge_key,
            flow_entry_sep,
            flow_root,
            flow_maps_only,
            line_prefix_marks,
            flow_map_kv_sep,
            flow_map_entry_sep,
            line_continuation,
            empty_seq_literal,
            opener_line_comment,
        ]
    );
    s.size = core::mem::size_of::<FigSyntax>() as u32;
    Ok(s)
}

/// `normalize`: the record at this library's layout, or the refusal for a
/// version it does not speak or a size too short for the required fields.
///
/// # Safety
///
/// `raw` must point to a vtable of at least its `size` bytes, its
/// `dialects` to `dialect_count` rows of `dialect_size` bytes, and every
/// syntax record to one of at least its `size` bytes (fig.h's contract).
unsafe fn normalize(raw: *const FigLanguageVTable) -> Result<Normalized, String> {
    let head = raw.cast::<u8>();
    // SAFETY: `version` and `size` lead every layout of the record.
    let (version, size) = unsafe { (core::ptr::read_unaligned(head.cast::<u32>()), core::ptr::read_unaligned(head.add(4).cast::<u32>())) };
    if version != VTABLE_VERSION {
        return Err(format!("vtable version {version} is not the {VTABLE_VERSION} this fig speaks"));
    }
    let size = size as usize;
    if size < VTABLE_REQUIRED {
        return Err(format!("the vtable's size {size} is too short to hold its required fields ({VTABLE_REQUIRED})"));
    }
    let mut vt = gated!(
        FigLanguageVTable,
        head,
        size,
        FigLanguageVTable::DEFAULT,
        [
            version,
            size,
            ctx,
            name,
            caps,
            max_mapping_depth,
            lossless,
            syntax,
            dialects,
            dialect_count,
            dialect_size,
            samples,
            sample_count,
            parse,
            print,
            free_bytes,
            render_value,
            render_entry,
            render_item,
            render_tail,
            render_key,
            render_key_name,
            split_stream,
            render_string,
        ]
    );
    vt.size = core::mem::size_of::<FigLanguageVTable>() as u32;

    let mut dialects = Vec::new();
    let mut dialect_syntax = Vec::new();
    if vt.dialect_count > 0 {
        if vt.dialect_size < DIALECT_REQUIRED {
            return Err(format!("the vtable's dialect_size {} is too short to hold a dialect row ({DIALECT_REQUIRED})", vt.dialect_size));
        }
        if vt.dialects.is_null() {
            return Err("the vtable's dialects is null".into());
        }
        let base = vt.dialects.cast::<u8>();
        for i in 0..vt.dialect_count {
            // Row `i` is `dialect_size` bytes at `i * dialect_size` into the
            // caller's array (fig.h's stride-gated rows), which `gated!`
            // reads, by the caller's word, as readable.
            let row = base.wrapping_add(i * vt.dialect_size);
            let d = gated!(
                FigDialectDesc,
                row,
                vt.dialect_size,
                FigDialectDesc::DEFAULT,
                [
                    name,
                    extensions,
                    splice,
                    empty_doc_seed,
                    syntax,
                    prints_holds,
                    prints_flags,
                    prints_char_as,
                    prints_number_special_as,
                    prints_comments,
                ]
            );
            // SAFETY: a non-null syntax is a record of its `size` bytes.
            let s = if d.syntax.is_null() { None } else { Some(unsafe { normalize_syntax(d.syntax) }?) };
            dialects.push(d);
            dialect_syntax.push(s);
        }
    }
    vt.dialect_size = core::mem::size_of::<FigDialectDesc>();
    // SAFETY: a non-null syntax is a record of its `size` bytes.
    let syntax = if vt.syntax.is_null() { None } else { Some(unsafe { normalize_syntax(vt.syntax) }?) };
    Ok(Normalized { vt, syntax, dialects, dialect_syntax })
}

/// A string of the record, lossily as text; null is the empty string.
///
/// # Safety
///
/// A non-null `p` must be a NUL-terminated string.
unsafe fn text(p: *const c_char) -> String {
    // SAFETY: the caller's contract.
    String::from_utf8_lossy(unsafe { cstr(p) }.unwrap_or(b"")).into_owned()
}

/// `validateVTable`: the rules a compiled language is held to, over the
/// record. The `Err` is the refusal.
///
/// # Safety
///
/// Every string the record points to is NUL-terminated (fig.h's contract).
unsafe fn validate(n: &Normalized) -> Result<(), String> {
    let vt = &n.vt;
    // SAFETY: the caller's contract, for every string read below.
    let name = unsafe { text(vt.name) };
    if name.is_empty() {
        return Err("a language must have a name".into());
    }
    if vt.caps & CAP_READ == 0 {
        return Err(format!("'{name}' declares no read capability; a language must at least parse"));
    }
    if vt.caps & !CAP_KNOWN != 0 {
        return Err(format!("'{name}' declares capability bits 0x{:x} this fig does not know", vt.caps & !CAP_KNOWN));
    }
    let known = known_lossless_bits();
    if vt.lossless & !known != 0 {
        return Err(format!("'{name}' declares lossless bits 0x{:x} this fig does not know", vt.lossless & !known));
    }
    if vt.lossless != 0 && vt.lossless & LOSSLESS_ENVELOPE == 0 {
        return Err(format!("'{name}' declares native kinds in lossless without the envelope bit"));
    }
    if vt.max_mapping_depth > u8::MAX as c_int {
        return Err(format!(
            "'{name}' declares max_mapping_depth {}; a limit is at most 255, and FIG_DEPTH_NONE is no limit",
            vt.max_mapping_depth
        ));
    }
    if vt.dialect_count == 0 {
        return Err(format!("'{name}' declares no dialects, so no format could name it"));
    }
    if vt.dialect_count > Dialect::MAX as usize + 1 {
        return Err(format!("'{name}' declares {} dialects; a language has at most {}", vt.dialect_count, Dialect::MAX as usize + 1));
    }
    // SAFETY: as above.
    let first = unsafe { text(n.dialects[0].name) };
    if first != name {
        return Err(format!("'{name}': the first dialect row must be named after the language, and is '{first}'"));
    }
    for (i, d) in n.dialects.iter().enumerate() {
        // SAFETY: as above.
        let dn = unsafe { cstr(d.name) }.unwrap_or(b"");
        if dn.is_empty() {
            return Err(format!("'{name}': dialect row {i} has no name"));
        }
        for other in &n.dialects[i + 1..] {
            // SAFETY: as above.
            if unsafe { cstr(other.name) } == Some(dn) {
                return Err(format!("'{name}' declares two dialects named '{}'", String::from_utf8_lossy(dn)));
            }
        }
        if !(0..3).contains(&d.splice) {
            return Err(format!("'{name}': dialect '{}' has an unknown splice style {}", String::from_utf8_lossy(dn), d.splice));
        }
    }
    if vt.sample_count == 0 {
        return Err(format!("'{name}' declares no samples; registration checks a language against what it says it parses"));
    }
    if vt.samples.is_null() {
        return Err(format!("'{name}': the vtable's samples is null"));
    }
    if vt.parse.is_none() {
        return Err(format!("'{name}' supplies no parse function"));
    }
    if vt.free_bytes.is_none() {
        return Err(format!("'{name}' supplies no free_bytes function"));
    }
    if vt.caps & CAP_SERIALIZE != 0 && vt.print.is_none() {
        return Err(format!("'{name}' declares the serialize capability but no print function"));
    }
    if vt.caps & CAP_SERIALIZE == 0 && vt.lossless != 0 {
        return Err(format!("'{name}' declares lossless (an envelope target for serialized output) but no serialize capability"));
    }
    let edit = vt.caps & CAP_EDIT != 0;
    let Some(syntax) = &n.syntax else {
        if edit {
            return Err(format!("'{name}' declares the edit capability and must supply syntax"));
        }
        return no_renderers(&name, vt);
    };
    if !edit {
        return no_renderers(&name, vt);
    }
    // The syntax coherence rules, over the language's syntax and every
    // dialect's own.
    // SAFETY: as above.
    unsafe { validate_syntax(&name, vt, syntax) }?;
    for s in n.dialect_syntax.iter().flatten() {
        // SAFETY: as above.
        unsafe { validate_syntax(&name, vt, s) }?;
    }
    Ok(())
}

/// A language with no edit capability supplies no editing renderer.
fn no_renderers(name: &str, vt: &FigLanguageVTable) -> Result<(), String> {
    if vt.caps & CAP_EDIT == 0
        && (vt.render_value.is_some()
            || vt.render_entry.is_some()
            || vt.render_item.is_some()
            || vt.render_tail.is_some()
            || vt.render_key.is_some()
            || vt.render_key_name.is_some()
            || vt.render_string.is_some())
    {
        return Err(format!("'{name}' declares no edit capability but supplies an editing renderer"));
    }
    Ok(())
}

/// `validateSyntax`.
///
/// # Safety
///
/// Every string the record points to is NUL-terminated.
unsafe fn validate_syntax(name: &str, vt: &FigLanguageVTable, s: &FigSyntax) -> Result<(), String> {
    if !s.comments.trailing.open.is_null() && s.comments.line.open.is_null() {
        return Err(format!("'{name}' declares a trailing comment marker but no line comment marker"));
    }
    if s.kv_sep.is_null() && vt.render_entry.is_none() {
        return Err(format!(
            "'{name}' declares kv_sep = null but no render_entry, so the generic entry-insert paths have no separator to write"
        ));
    }
    if s.block_seq_editable == 0 && vt.render_item.is_some() {
        return Err(format!(
            "'{name}' declares block_seq_editable = false but supplies render_item, which the engine refuses before reaching"
        ));
    }
    let cc = &s.closed_containers;
    if !cc.map_open.is_null() && (cc.map_close.is_null() || cc.seq_open.is_null() || cc.seq_close.is_null()) {
        return Err(format!("'{name}': closed_containers needs all four tokens"));
    }
    if !s.section_header.open.is_null() && s.section_header.close.is_null() {
        return Err(format!("'{name}': section_header needs a close token"));
    }
    // SAFETY: the caller's contract.
    if unsafe { cstr(s.flow_entry_sep) }.is_some_and(|sep| sep.is_empty()) {
        return Err(format!("'{name}': flow_entry_sep is empty; null means `,`"));
    }
    if s.flow_root != 0 && s.flow_containers == 0 {
        return Err(format!("'{name}' declares flow_root but flow_containers = false"));
    }
    Ok(())
}

fn leak_bytes(b: &[u8]) -> &'static [u8] {
    Box::leak(b.to_vec().into_boxed_slice())
}

fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// A copy of the string at `p`, kept for the life of the process as a
/// registration is; `None` for null.
///
/// # Safety
///
/// A non-null `p` must be NUL-terminated.
unsafe fn dupe(p: *const c_char) -> Option<&'static [u8]> {
    // SAFETY: the caller's contract.
    unsafe { cstr(p) }.map(leak_bytes)
}

/// `delimiterOf`.
///
/// # Safety
///
/// As `dupe`, for each string.
unsafe fn delimiter_of(d: &FigCommentDelimiter) -> Option<CommentDelimiter> {
    // SAFETY: the caller's contract.
    unsafe {
        let open = dupe(d.open)?;
        Some(CommentDelimiter { open, close: dupe(d.close).unwrap_or(b""), forbidden: dupe(d.forbidden) })
    }
}

/// `syntaxOf`: the record as a `Syntax`, every string copied, every null
/// where a value is required its default.
///
/// # Safety
///
/// Every string the record points to is NUL-terminated.
unsafe fn syntax_of(s: &FigSyntax) -> Syntax {
    let defaults = Syntax::new(Comments::NONE, None, None);
    // SAFETY: the caller's contract, for every string read.
    unsafe {
        let mut out = Syntax {
            comments: Comments {
                line: delimiter_of(&s.comments.line),
                trailing: delimiter_of(&s.comments.trailing),
                block: delimiter_of(&s.comments.block),
            },
            kv_sep: dupe(s.kv_sep),
            flow_kv_sep_from_siblings: s.flow_kv_sep_from_siblings != 0,
            flow_map_pad: dupe(s.flow_map_pad).unwrap_or(defaults.flow_map_pad),
            key_sigil: if s.key_sigil == 0 { None } else { Some(s.key_sigil) },
            empty_map_literal: dupe(s.empty_map_literal),
            block_seq_editable: s.block_seq_editable != 0,
            flow_containers: s.flow_containers != 0,
            indent_unit: dupe(s.indent_unit).unwrap_or(defaults.indent_unit),
            seq_item_marker: dupe(s.seq_item_marker).unwrap_or(defaults.seq_item_marker),
            closed_containers: None,
            single_line_block_mapping: s.single_line_block_mapping != 0,
            bare_document_mapping: s.bare_document_mapping != 0,
            flow_map_open: dupe(s.flow_map_open).unwrap_or(defaults.flow_map_open),
            flow_map_close: dupe(s.flow_map_close).unwrap_or(defaults.flow_map_close),
            structural_indent: s.structural_indent != 0,
            section_noun: dupe(s.section_noun),
            section_header: None,
            merge_key: dupe(s.merge_key),
            flow_entry_sep: dupe(s.flow_entry_sep).unwrap_or(defaults.flow_entry_sep),
            flow_root: s.flow_root != 0,
            flow_maps_only: s.flow_maps_only != 0,
            line_prefix_marks: dupe(s.line_prefix_marks).unwrap_or(defaults.line_prefix_marks),
            flow_map_kv_sep: dupe(s.flow_map_kv_sep).unwrap_or(defaults.flow_map_kv_sep),
            flow_map_entry_sep: dupe(s.flow_map_entry_sep).unwrap_or(defaults.flow_map_entry_sep),
            line_continuation: if s.line_continuation == 0 { None } else { Some(s.line_continuation) },
            empty_seq_literal: dupe(s.empty_seq_literal),
            opener_line_comment: s.opener_line_comment != 0,
        };
        let cc = &s.closed_containers;
        if let (Some(mo), Some(mc), Some(so), Some(sc)) = (dupe(cc.map_open), dupe(cc.map_close), dupe(cc.seq_open), dupe(cc.seq_close)) {
            out.closed_containers =
                Some(ClosedContainers { map: Delimiters { open: mo, close: mc }, seq: Delimiters { open: so, close: sc } });
        }
        let h = &s.section_header;
        if let (Some(open), Some(close)) = (dupe(h.open), dupe(h.close)) {
            out.section_header = Some(SectionHeader {
                open,
                close,
                seq_open: dupe(h.seq_open),
                seq_close: dupe(h.seq_close),
                sep: dupe(h.sep).unwrap_or(b"."),
                skip_index: h.skip_index != 0,
            });
        }
        out
    }
}

/// `printsOf`: a dialect row's `Prints`, or `None` when it declares none.
///
/// # Safety
///
/// Every string the row points to is NUL-terminated.
unsafe fn prints_of(d: &FigDialectDesc) -> Option<Prints> {
    let holds = native_kinds_of(d.prints_holds)?;
    // SAFETY: the caller's contract.
    let (char_as, special_as) = unsafe { (cstr(d.prints_char_as), cstr(d.prints_number_special_as)) };
    let name = |b: Option<&[u8]>| b.map_or("string", |b| leak_str(String::from_utf8_lossy(b).into_owned()));
    Some(Prints {
        holds,
        typed_scalars: d.prints_flags & FIG_PRINTS_TEXT_SCALARS == 0,
        null_at_root: d.prints_flags & FIG_PRINTS_NULL_AT_ROOT != 0,
        root_container: d.prints_flags & FIG_PRINTS_ROOT_CONTAINER != 0,
        char_as: name(char_as),
        number_special_as: name(special_as),
        comments: match d.prints_comments {
            1 => PrintsComments::Pretty,
            2 => PrintsComments::Never,
            _ => PrintsComments::Always,
        },
    })
}

// ============================================================================
// THE LANGUAGE
// ============================================================================

/// A language registered through its vtable: the record's declarations,
/// copied, and its functions, called.
struct CLanguage {
    ctx: *mut c_void,
    parse: FigParseFn,
    print: Option<FigPrintFn>,
    free_bytes: FigFreeBytesFn,
    /// `value`, `entry`, `item`, `tail`, `key`, `key_name`, `string`.
    renderers: [Option<FigRenderFn>; 7],
    split_stream: Option<FigSplitStreamFn>,
    name: &'static str,
    caps: Caps,
    syntax: Option<Syntax>,
    dialects: &'static [DialectInfo],
    /// Each row's name as the vtable's functions are handed it.
    dialect_z: Vec<CString>,
    dialect_syntax: Vec<Option<Syntax>>,
    dialect_extensions: Vec<Vec<&'static str>>,
    extensions: &'static [&'static str],
    samples: &'static [&'static [u8]],
}

// SAFETY: fig.h makes `ctx` and the functions valid for the life of the
// process, and its threading rule is the language's to keep: calls on
// different handles — and so on one registered language — may run on
// different threads, as they may for a compiled one.
unsafe impl Send for CLanguage {}
// SAFETY: as above; `CLanguage` itself is immutable once built.
unsafe impl Sync for CLanguage {}

fn renderer_index(which: Renderer) -> usize {
    match which {
        Renderer::Value => 0,
        Renderer::Entry => 1,
        Renderer::Item => 2,
        Renderer::Tail => 3,
        Renderer::Key => 4,
        Renderer::KeyName => 5,
        Renderer::String => 6,
    }
}

/// The literal kind as the `FigRenderRequest` names it.
fn literal_z(l: Literal) -> &'static CStr {
    match l {
        Literal::Null => c"null",
        Literal::Bool => c"bool",
        Literal::Int => c"int",
        Literal::Float => c"float",
        Literal::Datetime => c"datetime",
        Literal::String => c"string",
    }
}

/// The C `Refusal` codes a printer names, 1 to 9, as the errors they are.
fn refusal(code: c_int) -> fig_core::Error {
    match code {
        1 => err!(NullUnsupported),
        2 => err!(NonStringKey),
        3 => err!(UnrepresentableRoot),
        4 => err!(UnsupportedValue),
        5 => err!(InvalidKey),
        6 => err!(NonFiniteNumber),
        7 => err!(UnresolvedAlias),
        8 => err!(NestingTooDeep),
        9 => err!(UnexpectedNodeKind),
        _ => err!(RuntimePrintFailed),
    }
}

/// The bytes a `FigStr` a language returned names (`Str.slice() orelse ""`).
///
/// # Safety
///
/// The language's word that `s` names `s.len` readable bytes.
unsafe fn returned<'a>(s: FigStr) -> &'a [u8] {
    if s.len == FIG_LEN_NONE {
        return &[];
    }
    // SAFETY: the caller's contract.
    unsafe { slice_of(s.ptr, s.len) }.unwrap_or(&[])
}

impl CLanguage {
    /// The language a vtable describes, or the refusal.
    ///
    /// # Safety
    ///
    /// `raw` must be a vtable as fig.h describes one: every pointer in it
    /// valid for what it points to, `ctx` and the functions for the life of
    /// the process.
    unsafe fn from_record(raw: *const FigLanguageVTable) -> Result<CLanguage, String> {
        // SAFETY: the caller's contract.
        let n = unsafe { normalize(raw) }?;
        // SAFETY: the caller's contract.
        unsafe { validate(&n) }?;
        let vt = &n.vt;
        let (Some(parse), Some(free_bytes)) = (vt.parse, vt.free_bytes) else { unreachable!("validate refuses a null parse or free_bytes") };

        // SAFETY: the caller's contract, for every string and array read.
        unsafe {
            let name = leak_str(text(vt.name));
            let caps = Caps {
                read: vt.caps & CAP_READ != 0,
                edit: vt.caps & CAP_EDIT != 0,
                serialize: vt.caps & CAP_SERIALIZE != 0,
                references: vt.caps & CAP_REFERENCES != 0,
                lossless: native_kinds_of(vt.lossless),
                max_mapping_depth: if vt.max_mapping_depth < 0 { None } else { Some(vt.max_mapping_depth as u8) },
            };
            let syntax = n.syntax.as_ref().map(|s| syntax_of(s));
            let mut dialects = Vec::with_capacity(n.dialects.len());
            let mut dialect_z = Vec::with_capacity(n.dialects.len());
            let mut dialect_syntax = Vec::with_capacity(n.dialects.len());
            let mut dialect_extensions = Vec::with_capacity(n.dialects.len());
            let mut extensions = Vec::new();
            for (i, (d, s)) in n.dialects.iter().zip(&n.dialect_syntax).enumerate() {
                let dn = cstr(d.name).unwrap_or(b"");
                let splice = match d.splice {
                    1 => SpliceStyle::Escaped,
                    2 => SpliceStyle::Raw,
                    _ => SpliceStyle::Literal,
                };
                dialects.push(DialectInfo {
                    dialect: i as Dialect,
                    prints: prints_of(d),
                    ..DialectInfo::new(leak_str(String::from_utf8_lossy(dn).into_owned()), splice, dupe(d.empty_doc_seed))
                });
                dialect_z.push(CString::new(dn).unwrap_or_default());
                dialect_syntax.push(s.as_ref().map(|s| syntax_of(s)));
                let mut exts = Vec::new();
                if !d.extensions.is_null() {
                    let mut k = 0;
                    while !(*d.extensions.add(k)).is_null() {
                        exts.push(leak_str(text(*d.extensions.add(k))));
                        k += 1;
                    }
                }
                extensions.extend(exts.iter().copied());
                dialect_extensions.push(exts);
            }
            let samples: Vec<&'static [u8]> =
                array_of(vt.samples, vt.sample_count).unwrap_or(&[]).iter().map(|&s| leak_bytes(returned(s))).collect();
            Ok(CLanguage {
                ctx: vt.ctx,
                parse,
                print: vt.print,
                free_bytes,
                renderers: [vt.render_value, vt.render_entry, vt.render_item, vt.render_tail, vt.render_key, vt.render_key_name, vt.render_string],
                split_stream: vt.split_stream,
                name,
                caps,
                syntax,
                dialects: Box::leak(dialects.into_boxed_slice()),
                dialect_z,
                dialect_syntax,
                dialect_extensions,
                extensions: Box::leak(extensions.into_boxed_slice()),
                samples: Box::leak(samples.into_boxed_slice()),
            })
        }
    }

    /// The row's name, NUL-terminated, as the functions are handed it.
    fn dialect_z(&self, d: Dialect) -> *const c_char {
        self.dialect_z.get(d as usize).or(self.dialect_z.first()).map_or(c"".as_ptr(), |s| s.as_ptr())
    }

    /// Hand bytes a function returned back to the language.
    fn free(&self, s: FigStr) {
        // SAFETY: `s` is what the language's own function returned, handed
        // back once, as fig.h's `free_bytes` contract states.
        unsafe { (self.free_bytes)(self.ctx, s) }
    }

    /// `printNodeWith`'s call: the tree from `root` as its table, through
    /// the vtable's `print`.
    fn print_from(&self, dialect: Dialect, ast: &AST, root: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> fig_core::Result<()> {
        let Some(print) = self.print else { return Err(err!(FormatDisabled)) };
        if !self.caps.serialize {
            return Err(err!(FormatDisabled));
        }
        let mut built = convert::document_to_table(ast, root)?;
        if options.strip_comments {
            built.comments.clear();
        }
        let view = FigTableView::of(&built.view());
        let opts = FigPrintOptions {
            size: core::mem::size_of::<FigPrintOptions>() as u32,
            pretty: options.pretty,
            strip_comments: options.strip_comments,
            indent: options.indent,
            width: options.width,
            splice: options.splice,
            flow: options.flow,
        };
        let mut s = FigStr::EMPTY;
        let mut e = FigError::EMPTY;
        // SAFETY: the view borrows `built`, which outlives the call; the
        // rest are this frame's, and the function is the language's own.
        let rc = unsafe { print(self.ctx, self.dialect_z(dialect), &view, &opts, &mut s, &mut e) };
        if rc != 0 {
            return Err(refusal(e.code));
        }
        // SAFETY: the language's word on what it returned.
        out.extend_from_slice(unsafe { returned(s) });
        self.free(s);
        Ok(())
    }
}

impl Language for CLanguage {
    fn name(&self) -> &str {
        self.name
    }

    fn extensions(&self) -> &[&'static str] {
        self.extensions
    }

    fn dialect_extensions(&self, row: usize) -> Option<Vec<&'static str>> {
        Some(self.dialect_extensions.get(row).cloned().unwrap_or_default())
    }

    fn caps(&self) -> Caps {
        self.caps
    }

    fn dialects(&self) -> &[DialectInfo] {
        self.dialects
    }

    fn samples(&self) -> &[&'static [u8]] {
        self.samples
    }

    /// The row's syntax, else the language's, else none for a read-only
    /// format (the runtime registry's `Language.syntax`).
    fn syntax(&self, dialect: Dialect) -> Syntax {
        if let Some(Some(s)) = self.dialect_syntax.get(dialect as usize) {
            return *s;
        }
        self.syntax.unwrap_or(Syntax::new(Comments::NONE, None, None))
    }

    fn is_runtime(&self) -> bool {
        true
    }

    /// The vtable's `parse` into fig's table; a refusal's message and offset
    /// stated with `set_parse_error`, as the registry's `Parser` reports it.
    fn parse_table(&self, input: &[u8], dialect: Dialect, out: &mut Table) -> fig_core::Result<()> {
        let mut e = FigError::EMPTY;
        // SAFETY: `input` and `out` outlive the call, and `out` is lent to
        // the language for it alone (it writes through `fig_table_*`); the
        // function is the language's own.
        let rc = unsafe { (self.parse)(self.ctx, self.dialect_z(dialect), FigStr::of(input), out as *mut Table, &mut e) };
        if rc != 0 {
            language::set_parse_error(e.text(), e.byte_offset);
            return Err(err!(RuntimeParseFailed));
        }
        Ok(())
    }

    fn print(&self, dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> fig_core::Result<()> {
        self.print_from(dialect, ast, ast.root, options, out)
    }

    fn print_node(&self, dialect: Dialect, ast: &AST, node: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> fig_core::Result<()> {
        self.print_from(dialect, ast, node, options, out)
    }

    /// One renderer call, when the vtable fills the slot. A refusal keeps
    /// the language's words in `last_refusal` (`rendererError`).
    fn render(&self, which: Renderer, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<fig_core::Result<()>> {
        let f = self.renderers[renderer_index(which)]?;
        let request = FigRenderRequest {
            size: core::mem::size_of::<FigRenderRequest>() as u32,
            dialect: self.dialect_z(dialect),
            indent: FigStr::of(r.indent),
            key: FigStr::of(r.key),
            value: FigStr::of(r.value),
            literal: literal_z(r.literal).as_ptr(),
            old_key: FigStr::of(r.old_key),
            parent_key: FigStr::of(r.parent_key),
            parent_tag: FigStr::of(r.parent_tag),
            old_value: FigStr::of(r.old_value),
        };
        let mut s = FigStr::EMPTY;
        let mut e = FigError::EMPTY;
        // SAFETY: the request borrows `r`, which outlives the call; the
        // function is the language's own.
        if unsafe { f(self.ctx, &request, &mut s, &mut e) } != 0 {
            let text = String::from_utf8_lossy(e.text()).into_owned();
            registry::refuse(if text.is_empty() { "the renderer declined" } else { &text });
            return Some(Err(err!(RendererRefused)));
        }
        // SAFETY: the language's word on what it returned.
        out.extend_from_slice(unsafe { returned(s) });
        self.free(s);
        Some(Ok(()))
    }

    fn render_value(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<fig_core::Result<()>> {
        self.render(Renderer::Value, dialect, out, r)
    }
    fn render_entry(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<fig_core::Result<()>> {
        self.render(Renderer::Entry, dialect, out, r)
    }
    fn render_item(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<fig_core::Result<()>> {
        self.render(Renderer::Item, dialect, out, r)
    }
    fn render_tail(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<fig_core::Result<()>> {
        self.render(Renderer::Tail, dialect, out, r)
    }
    fn render_key(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<fig_core::Result<()>> {
        self.render(Renderer::Key, dialect, out, r)
    }
    fn render_key_name(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<fig_core::Result<()>> {
        self.render(Renderer::KeyName, dialect, out, r)
    }
    fn render_string(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<fig_core::Result<()>> {
        self.render(Renderer::String, dialect, out, r)
    }

    /// Whether the vtable fills the slot, without calling it.
    fn has_renderer(&self, _dialect: Dialect, which: Renderer) -> bool {
        self.renderers[renderer_index(which)].is_some()
    }

    /// The vtable's `split_stream`: an array of `FigStreamDoc` in the bytes
    /// it returns.
    fn split_stream(&self, input: &[u8], dialect: Dialect) -> Option<fig_core::Result<Vec<StreamDocument>>> {
        let f = self.split_stream?;
        let mut s = FigStr::EMPTY;
        let mut e = FigError::EMPTY;
        // SAFETY: `input` outlives the call; the function is the language's.
        if unsafe { f(self.ctx, self.dialect_z(dialect), FigStr::of(input), &mut s, &mut e) } != 0 {
            return Some(Err(err!(RuntimeParseFailed)));
        }
        // SAFETY: the language's word on what it returned.
        let bytes = unsafe { returned(s) };
        let size = core::mem::size_of::<FigStreamDoc>();
        let mut docs = Vec::with_capacity(bytes.len() / size);
        for chunk in bytes.chunks_exact(size) {
            // SAFETY: `chunk` is `size_of::<FigStreamDoc>()` bytes, read
            // unaligned; `marked` is read as the byte it is, so no byte
            // the language wrote is an invalid `bool`.
            let (start, end, marked) = unsafe {
                let p = chunk.as_ptr();
                (
                    core::ptr::read_unaligned(p.add(core::mem::offset_of!(FigStreamDoc, start)).cast::<usize>()),
                    core::ptr::read_unaligned(p.add(core::mem::offset_of!(FigStreamDoc, end)).cast::<usize>()),
                    *p.add(core::mem::offset_of!(FigStreamDoc, marked)) != 0,
                )
            };
            docs.push(StreamDocument { start, end, explicit: marked });
        }
        self.free(s);
        if docs.iter().any(|d| d.start > d.end || d.end > input.len()) {
            return Some(Err(err!(RuntimeParseFailed)));
        }
        Some(Ok(docs))
    }

    fn has_split_stream(&self) -> bool {
        self.split_stream.is_some()
    }
}

/// `Runtime.register`: the record read, held to the rules and registered
/// with the harness over its samples. The ABI integer of its first row.
///
/// # Safety
///
/// As `CLanguage::from_record`.
unsafe fn register(raw: *const FigLanguageVTable) -> fig_core::Result<i32> {
    registry::clear_refusal();
    // SAFETY: the caller's contract.
    let lang = match unsafe { CLanguage::from_record(raw) } {
        Ok(l) => l,
        Err(message) => {
            registry::refuse(&message);
            return Err(err!(InvalidLanguage));
        }
    };
    registry::global().register_arc(Arc::new(lang), RegisterOptions::default())
}

// ============================================================================
// THE EXPORTS
// ============================================================================

/// `fig_language_vtable_version`.
#[unsafe(no_mangle)]
pub extern "C" fn fig_language_vtable_version() -> u32 {
    VTABLE_VERSION
}

/// The status and message a registration's error reports (`c_api.zig`'s
/// and `wasm_host.zig`'s switches).
pub(crate) fn register_failed(out_err: *mut FigError, e: fig_core::Error) -> FigStatus {
    match e.name() {
        "OutOfMemory" => fill_error(out_err, FigStatus::OutOfMemory, b"out of memory"),
        "RegistryFull" | "AllocatorMismatch" => {
            fill_error(out_err, FigStatus::UnsupportedOperation, b"the language registry cannot take another registration")
        }
        "Malformed" => fill_error(out_err, FigStatus::InvalidArgument, b"the language's description is not the shape the wire documents"),
        _ => fill_error(out_err, FigStatus::InvalidArgument, &registry::last_refusal()),
    }
}

/// `fig_language_register`: register the language `vt` describes; its
/// first dialect row's format to `*out_format`.
///
/// # Safety
///
/// `vt` must be null or a vtable as fig.h describes one; `out_format` null
/// or writable; `out_err` null or a caller's `FigError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_language_register(vt: *const FigLanguageVTable, out_format: *mut c_int, out_err: *mut FigError) -> FigStatus {
    if out_format.is_null() {
        return fill_error(out_err, FigStatus::InvalidArgument, b"out_format is null");
    }
    // SAFETY: checked non-null; writable by the caller's word.
    unsafe { *out_format = -1 };
    if vt.is_null() {
        return fill_error(out_err, FigStatus::InvalidArgument, b"vtable is null");
    }
    init();
    // SAFETY: the caller's contract.
    match unsafe { register(vt) } {
        Ok(abi) => {
            // SAFETY: checked non-null above.
            unsafe { *out_format = abi };
            FigStatus::Ok
        }
        Err(e) => register_failed(out_err, e),
    }
}

/// `fig_format_capabilities`: the `FIG_CAP_*` bits `format`'s language
/// declares; 0 for a handle never handed out.
#[unsafe(no_mangle)]
pub extern "C" fn fig_format_capabilities(format: c_int) -> u32 {
    init();
    api::format_capabilities(format)
}

/// Each format's name, NUL-terminated, kept for the life of the process.
static NAMES: Mutex<Vec<(c_int, &'static CStr)>> = Mutex::new(Vec::new());

/// `fig_format_name`: the dialect's name, valid for the life of the
/// process; null for a handle never handed out.
#[unsafe(no_mangle)]
pub extern "C" fn fig_format_name(format: c_int) -> *const c_char {
    init();
    let Some(name) = api::format_name(format) else { return core::ptr::null() };
    let mut names = NAMES.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((_, n)) = names.iter().find(|(f, _)| *f == format) {
        return n.as_ptr();
    }
    let z: &'static CStr = Box::leak(CString::new(name).unwrap_or_default().into_boxed_c_str());
    names.push((format, z));
    z.as_ptr()
}

/// `fig_format_by_name`: the format of the dialect named `name`, or -1.
///
/// # Safety
///
/// `name` must be null or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_format_by_name(name: *const c_char) -> c_int {
    // SAFETY: the caller's contract.
    let Some(n) = (unsafe { cstr(name) }) else { return -1 };
    init();
    match core::str::from_utf8(n) {
        Ok(n) => api::format_by_name(n),
        Err(_) => -1,
    }
}

/// `fig_detect_order`: the first of `names` whose parser accepts `input`.
///
/// # Safety
///
/// `input` readable for `input_len` bytes; `names` readable for `count`
/// pointers, each null or NUL-terminated; `out_format` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_detect_order(
    input: *const u8,
    input_len: usize,
    names: *const *const c_char,
    count: usize,
    out_format: *mut c_int,
) -> FigStatus {
    if out_format.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: the caller's contract.
    let Some(bytes) = (unsafe { slice_of(input, input_len) }) else { return FigStatus::InvalidArgument };
    // SAFETY: the caller's contract.
    let Some(list) = (unsafe { array_of(names, count) }) else { return FigStatus::InvalidArgument };
    let mut order = Vec::with_capacity(list.len());
    for &n in list {
        // SAFETY: the caller's contract.
        let Some(n) = (unsafe { cstr(n) }) else { return FigStatus::InvalidArgument };
        order.push(String::from_utf8_lossy(n).into_owned());
    }
    let order: Vec<&str> = order.iter().map(String::as_str).collect();
    init();
    match api::detect_order(bytes, &order) {
        Ok(f) => {
            // SAFETY: checked non-null above.
            unsafe { *out_format = f };
            FigStatus::Ok
        }
        Err(s) => s.into(),
    }
}

/// `fig_stream_split`: the documents of a multi-document stream, an array
/// freed with `fig_stream_free`.
///
/// # Safety
///
/// `input` readable for `input_len` bytes; the out pointers null or
/// writable; `out_err` null or a caller's `FigError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_stream_split(
    input: *const u8,
    input_len: usize,
    format: c_int,
    out_docs: *mut *mut FigStreamDoc,
    out_count: *mut usize,
    out_err: *mut FigError,
) -> FigStatus {
    if out_docs.is_null() || out_count.is_null() {
        return FigStatus::InvalidArgument;
    }
    // SAFETY: both checked non-null; writable by the caller's word.
    unsafe {
        *out_docs = core::ptr::null_mut();
        *out_count = 0;
    }
    // SAFETY: the caller's contract.
    let Some(bytes) = (unsafe { slice_of(input, input_len) }) else { return FigStatus::InvalidArgument };
    init();
    match api::stream_split(bytes, format) {
        Ok(docs) => {
            let out: Box<[FigStreamDoc]> = docs.iter().map(|d| FigStreamDoc { start: d.start, end: d.end, marked: d.marked }).collect();
            let n = out.len();
            // SAFETY: both checked non-null above; the array is the caller's
            // until `fig_stream_free`.
            unsafe {
                *out_docs = Box::into_raw(out).cast::<FigStreamDoc>();
                *out_count = n;
            }
            FigStatus::Ok
        }
        // Only the language's refusal carries a message.
        Err(e) if e.code == api::Status::ParseError => write_error(out_err, &e),
        Err(e) => e.code.into(),
    }
}

/// `fig_stream_free`: release what `fig_stream_split` returned.
///
/// # Safety
///
/// `docs` must be null or a `fig_stream_split` array of `count` documents,
/// not freed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_stream_free(docs: *mut FigStreamDoc, count: usize) {
    if docs.is_null() {
        return;
    }
    // SAFETY: by the caller's word this is the boxed slice of `count`
    // documents `fig_stream_split` leaked.
    drop(unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(docs, count)) });
}
