//! The exports, called as a C program calls them: the surface against
//! fig.h, a language written against the vtable (`tinykv`, the twin of
//! `tools/abi_runtime_probe.c`'s), the handles over it, and the kit.

use core::ffi::{c_char, c_int, c_void, CStr};
use core::ptr::{null, null_mut};

use fig_core::kit::table::{self, Kind, Row, Text};

use crate::*;

// ============================================================================
// THE SURFACE
// ============================================================================

const HEADER: &str = include_str!("../../../bindings/c/include/fig.h");

/// `abi-check`'s `collectDeclared`: every `fig_x(` on a non-comment line.
fn declared() -> Vec<String> {
    let mut out = Vec::new();
    for line in HEADER.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let b = line.as_bytes();
        let mut i = 0;
        while let Some(pos) = line[i..].find("fig_").map(|p| p + i) {
            let name_char = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
            if pos > 0 && name_char(b[pos - 1]) {
                i = pos + 4;
                continue;
            }
            let mut end = pos + 4;
            while end < b.len() && name_char(b[end]) {
                end += 1;
            }
            if end < b.len() && b[end] == b'(' {
                out.push(line[pos..end].to_string());
            }
            i = end;
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every `extern "C" fn fig_*` this crate exports to C (the wasm-only pair,
/// and the kit module's `fig_kit_alloc` and `fig_kit_free`, excepted, as
/// fig.h does not declare them).
fn exported() -> Vec<String> {
    let sources = [
        include_str!("lib.rs"),
        include_str!("document.rs"),
        include_str!("editor.rs"),
        include_str!("value.rs"),
        include_str!("language.rs"),
        include_str!("table.rs"),
        include_str!("../../fig-kit-capi/src/lib.rs"),
    ];
    let mut out = Vec::new();
    for src in sources {
        let mut rest = src;
        while let Some(pos) = rest.find("extern \"C\" fn fig_") {
            let start = pos + "extern \"C\" fn ".len();
            let end = rest[start..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(rest.len(), |e| start + e);
            out.push(rest[start..end].to_string());
            rest = &rest[end..];
        }
        // The edits `path_bytes_edit!` and `path_edit!` stamp out, each
        // named on the line after its doc comment.
        for line in src.lines() {
            let t = line.trim_start();
            if t.starts_with("fig_editor_") && t.contains(',') {
                out.push(t[..t.find(',').unwrap()].to_string());
            }
        }
    }
    out.retain(|n| n != "fig_kit_alloc" && n != "fig_kit_free");
    out.sort();
    out.dedup();
    out
}

#[test]
fn every_function_fig_h_declares_is_exported_and_nothing_else() {
    let declared = declared();
    let exported = exported();
    let missing: Vec<_> = declared.iter().filter(|n| !exported.contains(n)).collect();
    let extra: Vec<_> = exported.iter().filter(|n| !declared.contains(n)).collect();
    assert!(missing.is_empty(), "declared in fig.h but not exported: {missing:?}");
    assert!(extra.is_empty(), "exported but not declared in fig.h: {extra:?}");
    assert_eq!(113, declared.len());
}

fn macro_int(name: &str) -> u32 {
    let line = HEADER.lines().find(|l| l.starts_with(&format!("#define {name} "))).unwrap();
    line[format!("#define {name} ").len()..].trim().parse().unwrap()
}

#[test]
fn the_versions_are_fig_h_s() {
    let v = (macro_int("FIG_VERSION_MAJOR") << 16) | (macro_int("FIG_VERSION_MINOR") << 8) | macro_int("FIG_VERSION_PATCH");
    assert_eq!(v, fig_version());
    let s = format!("{}.{}.{}", macro_int("FIG_VERSION_MAJOR"), macro_int("FIG_VERSION_MINOR"), macro_int("FIG_VERSION_PATCH"));
    // SAFETY: static storage, NUL-terminated.
    assert_eq!(s.as_bytes(), unsafe { CStr::from_ptr(fig_version_string()) }.to_bytes());
    assert_eq!(macro_int("FIG_ABI_VERSION"), fig_abi_version());
    assert_eq!(macro_int("FIG_LANGUAGE_VTABLE_VERSION"), fig_language_vtable_version());
}

// ============================================================================
// TINYKV, A LANGUAGE WRITTEN AGAINST THE VTABLE
// ============================================================================

fn set_message(err: *mut FigError, m: &str) {
    // SAFETY: fig hands the function its own `FigError`.
    unsafe {
        let n = m.len().min(255);
        (&mut (*err).message)[..n].copy_from_slice(&m.as_bytes()[..n]);
        (*err).message[n] = 0;
        (*err).message_len = n;
    }
}

/// `key=value` lines and `#` comments, each comment leading the next key.
unsafe extern "C" fn tinykv_parse(_ctx: *mut c_void, _dialect: *const c_char, input: FigStr, out: *mut FigTable, err: *mut FigError) -> c_int {
    // SAFETY: fig hands the parse its input and its table.
    unsafe {
        let src = core::slice::from_raw_parts(input.ptr, input.len);
        let add = |kind: u8, parent: u32, start: usize, end: usize| -> *mut Row {
            let r = fig_table_rows(out, 1);
            (*r).kind = kind;
            (*r).parent = parent;
            (*r).start = start as u32;
            (*r).end = end as u32;
            r
        };
        add(Kind::MAPPING, table::NONE, 0, src.len());
        let mut rows = 1u32;
        let mut pending: Vec<(usize, usize)> = Vec::new();
        let mut at = 0;
        while at < src.len() {
            let end = src[at..].iter().position(|&c| c == b'\n').map_or(src.len(), |p| at + p);
            if end == at {
                at = end + 1;
                continue;
            }
            if src[at] == b'#' {
                let mut s = at + 1;
                while s < end && src[s] == b' ' {
                    s += 1;
                }
                pending.push((s, end));
                at = end + 1;
                continue;
            }
            let Some(eq) = src[at..end].iter().position(|&c| c == b'=').map(|p| at + p) else {
                set_message(err, "expected key=value");
                (*err).byte_offset = at;
                return FigStatus::ParseError as c_int;
            };
            let kv = rows;
            add(Kind::KEYVALUE, 0, at, end);
            assert_eq!(FigStatus::Ok, fig_table_sep(out, kv, eq as u32, eq as u32 + 1));
            let key = rows + 1;
            (*add(Kind::STRING, kv, at, eq)).text = Text { off: at as u32, len: (eq - at) as u32 };
            (*add(Kind::STRING, kv, eq + 1, end)).text = Text { off: eq as u32 + 1, len: (end - eq - 1) as u32 };
            rows += 3;
            for (s, e) in pending.drain(..) {
                let mut text = Text::ABSENT;
                assert_eq!(FigStatus::Ok, fig_table_string(out, src[s..].as_ptr(), e - s, &mut text));
                assert_eq!(FigStatus::Ok, fig_table_comment(out, key, table::COMMENT_LEADING, table::COMMENT_LINE, text));
            }
            at = end + 1;
        }
        0
    }
}

/// The bytes a text names in a view.
///
/// # Safety
///
/// `v` is the view fig handed the print.
unsafe fn text_of<'a>(v: &'a FigTableView, t: Text) -> &'a [u8] {
    // SAFETY: the view's buffers are what the text indexes.
    unsafe {
        if t.len & Text::BUFFER_BIT != 0 {
            core::slice::from_raw_parts(v.strings.add(t.off as usize), (t.len & !Text::BUFFER_BIT) as usize)
        } else {
            core::slice::from_raw_parts(v.source.add(t.off as usize), t.len as usize)
        }
    }
}

unsafe extern "C" fn tinykv_print(
    _ctx: *mut c_void,
    _dialect: *const c_char,
    view: *const FigTableView,
    _options: *const FigPrintOptions,
    out: *mut FigStr,
    err: *mut FigError,
) -> c_int {
    // SAFETY: fig hands the print its view and out-parameters.
    unsafe {
        let v = &*view;
        assert!(v.size as usize >= core::mem::size_of::<FigTableView>());
        let rows = core::slice::from_raw_parts(v.rows, v.row_count);
        let comments = core::slice::from_raw_parts(v.comments, v.comment_count);
        let mut buf = Vec::new();
        let mut i = 1;
        while i < rows.len() {
            if i + 2 >= rows.len() || rows[i].kind != Kind::KEYVALUE || rows[i + 1].kind != Kind::STRING || rows[i + 2].kind != Kind::STRING {
                set_message(err, "tinykv holds a flat string map");
                (*err).code = 4; // FIG_REFUSAL unsupported_value
                return FigStatus::UnsupportedFormat as c_int;
            }
            for c in comments.iter().filter(|c| c.node as usize == i + 1 && c.slot == table::COMMENT_LEADING) {
                buf.extend_from_slice(b"# ");
                buf.extend_from_slice(text_of(v, c.text));
                buf.push(b'\n');
            }
            buf.extend_from_slice(text_of(v, rows[i + 1].text));
            buf.push(b'=');
            buf.extend_from_slice(text_of(v, rows[i + 2].text));
            buf.push(b'\n');
            i += 3;
        }
        let bytes = Box::leak(buf.into_boxed_slice());
        *out = FigStr { ptr: bytes.as_ptr(), len: bytes.len() };
        0
    }
}

unsafe extern "C" fn tinykv_free(_ctx: *mut c_void, bytes: FigStr) {
    // SAFETY: what `tinykv_print` leaked, handed back once.
    drop(unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(bytes.ptr.cast_mut(), bytes.len)) });
}

static SAMPLES: [&[u8]; 2] = [b"a=1\nb=two\n", b"# top\nk=v\n"];

/// Register tinykv under `name` (NUL-terminated), as a C host does.
fn register_tinykv(name: &'static CStr, out_err: *mut FigError) -> (FigStatus, c_int) {
    let mut syntax = FigSyntax::DEFAULT;
    syntax.comments.line.open = c"#".as_ptr();
    syntax.kv_sep = c"=".as_ptr();
    syntax.empty_map_literal = c"{}".as_ptr();
    syntax.flow_containers = 0;
    let extensions: [*const c_char; 2] = [c"tkv".as_ptr(), null()];
    let mut dialect = FigDialectDesc::DEFAULT;
    dialect.name = name.as_ptr();
    dialect.extensions = extensions.as_ptr();
    dialect.splice = 2;
    dialect.empty_doc_seed = c"".as_ptr();
    let samples: Vec<FigStr> = SAMPLES.iter().map(|s| FigStr::of(s)).collect();
    let mut vt = FigLanguageVTable::DEFAULT;
    vt.version = VTABLE_VERSION;
    vt.name = name.as_ptr();
    vt.caps = 1 | 2 | 4;
    vt.max_mapping_depth = 0;
    vt.syntax = &syntax;
    vt.dialects = &dialect;
    vt.dialect_count = 1;
    vt.samples = samples.as_ptr();
    vt.sample_count = samples.len();
    vt.parse = Some(tinykv_parse);
    vt.print = Some(tinykv_print);
    vt.free_bytes = Some(tinykv_free);
    let mut format = 0;
    // SAFETY: every record is this frame's, valid for the call, which copies
    // what it keeps; the functions live for the process.
    let st = unsafe { fig_language_register(&vt, &mut format, out_err) };
    (st, format)
}

fn new_error() -> FigError {
    FigError { size: core::mem::size_of::<FigError>() as u32, ..FigError::EMPTY }
}

fn message(e: &FigError) -> String {
    String::from_utf8_lossy(e.text()).into_owned()
}

/// `bytes` borrowed through an out pair, copied.
unsafe fn copied(ptr: *const u8, len: usize) -> Vec<u8> {
    // SAFETY: the out pair a call just wrote.
    unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec()
}

#[test]
fn a_language_written_against_the_vtable_registers_parses_converts_and_edits() {
    let mut err = new_error();
    let (st, f) = register_tinykv(c"capi-tinykv", &mut err);
    assert_eq!(FigStatus::Ok, st, "{}", message(&err));
    assert!(f >= 1);
    // SAFETY: the name is static storage for the life of the process.
    assert_eq!(b"capi-tinykv", unsafe { CStr::from_ptr(fig_format_name(f)) }.to_bytes());
    // SAFETY: a NUL-terminated name.
    assert_eq!(f, unsafe { fig_format_by_name(c"capi-tinykv".as_ptr()) });
    assert_eq!(7, fig_format_capabilities(f));
    assert_eq!(0, fig_format_capabilities(9999));
    assert!(fig_format_name(9999).is_null());

    // SAFETY: every pointer below is this test's, valid for each call.
    unsafe {
        let src = b"# note\nx=1\ny=two\n";
        let mut doc = null_mut();
        assert_eq!(FigStatus::Ok, fig_parse(src.as_ptr(), src.len(), f, &mut doc));
        let root = fig_document_root(doc);
        assert_eq!(6, fig_node_kind(doc, root));
        assert_eq!(2, fig_node_child_count(doc, root));
        let kv = fig_node_first_child(doc, root);
        let (mut p, mut n) = (null(), 0usize);
        assert!(fig_node_string(doc, fig_keyvalue_key(doc, kv), &mut p, &mut n));
        assert_eq!(b"x", &copied(p, n)[..]);
        assert!(!fig_node_bool(doc, kv, null_mut()));
        assert_eq!(-1, fig_node_kind(null(), 0));

        assert_eq!(FigStatus::Ok, fig_document_serialize(doc, f, null(), &mut p, &mut n));
        assert_eq!(&src[..], &copied(p, n)[..]);
        let mut count = 9;
        assert_eq!(FigStatus::Ok, fig_document_diagnose(doc, f, null(), &mut count));
        assert_eq!(0, count);
        let mut w = FigWarning { size: core::mem::size_of::<FigWarning>() as u32, code: 0, cause: 0, path: null(), path_len: 0, note: null(), note_len: 0 };
        assert_eq!(FigStatus::InvalidArgument, fig_document_warning(doc, 0, &mut w));
        let mut resolved = null_mut();
        assert_eq!(FigStatus::Ok, fig_document_resolve_aliases(doc, &mut resolved, null_mut()));
        assert!(resolved.is_null());
        fig_document_destroy(doc);

        // A refusal, with the language's words and offset.
        let mut perr = new_error();
        let bad = b"x=1\nnope\n";
        assert_eq!(FigStatus::ParseError, fig_parse_ex(bad.as_ptr(), bad.len(), f, &mut doc, &mut perr));
        assert!(doc.is_null());
        assert_eq!("expected key=value", message(&perr));
        assert_eq!(4, perr.byte_offset);
        assert_eq!(2, perr.code);

        // A FigError too short for its message keeps its code alone.
        let mut short = FigError { size: 8, code: 0, byte_offset: 77, ..FigError::EMPTY };
        assert_eq!(FigStatus::ParseError, fig_parse_ex(bad.as_ptr(), bad.len(), f, &mut doc, &mut short));
        assert_eq!(2, short.code);
        assert_eq!(77, short.byte_offset);
        assert_eq!(0, short.message_len);

        let mut ed = null_mut();
        assert_eq!(FigStatus::Ok, fig_editor_create(src.as_ptr(), src.len(), f, &mut ed));
        let seg = FigPathSegment { kind: FIG_SEGMENT_KEY, key_ptr: b"x".as_ptr(), key_len: 1, index: 0 };
        assert_eq!(FigStatus::Ok, fig_editor_replace_val(ed, &seg, 1, b"10".as_ptr(), 2));
        assert_eq!(FigStatus::Ok, fig_editor_insert_key(ed, null(), 0, b"z".as_ptr(), 1, b"3".as_ptr(), 1));
        assert_eq!(FigStatus::Ok, fig_editor_get_leading_comment(ed, &seg, 1, &mut p, &mut n));
        assert_eq!(b"note", &copied(p, n)[..]);
        let y = FigPathSegment { kind: FIG_SEGMENT_KEY, key_ptr: b"y".as_ptr(), key_len: 1, index: 0 };
        assert_eq!(FigStatus::UnsupportedFormat, fig_editor_get_trailing_comment(ed, &y, 1, &mut p, &mut n));
        assert_eq!(FigStatus::Ok, fig_editor_source(ed, &mut p, &mut n));
        assert_eq!(b"# note\nx=10\ny=two\nz=3\n", &copied(p, n)[..]);
        // Malformed calls.
        let bad_seg = FigPathSegment { kind: 7, key_ptr: null(), key_len: 0, index: 0 };
        assert_eq!(FigStatus::InvalidArgument, fig_editor_delete_key(ed, &bad_seg, 1));
        let null_key = FigPathSegment { kind: FIG_SEGMENT_KEY, key_ptr: null(), key_len: 0, index: 0 };
        assert_eq!(FigStatus::InvalidArgument, fig_editor_delete_key(ed, &null_key, 1));
        assert_eq!(FigStatus::InvalidArgument, fig_editor_replace_val(null_mut(), &seg, 1, b"1".as_ptr(), 1));
        assert_eq!(FigStatus::InvalidArgument, fig_editor_replace_val(ed, &seg, 1, null(), 1));
        assert_eq!(FigStatus::UnsupportedFormat, fig_editor_delete_container(ed, &seg, 1));
        fig_editor_destroy(ed);

        // A value built and rendered in it.
        let mut v = null_mut();
        assert_eq!(FigStatus::Ok, fig_value_create(&mut v));
        let (mut k, mut s, mut m) = (0, 0, 0);
        assert_eq!(FigStatus::Ok, fig_value_string(v, b"k".as_ptr(), 1, &mut k));
        assert_eq!(FigStatus::Ok, fig_value_string(v, b"v".as_ptr(), 1, &mut s));
        assert_eq!(FigStatus::InvalidArgument, fig_value_map(v, &FigKeyValue { key: k, value: 99 }, 1, &mut m));
        assert_eq!(FigStatus::Ok, fig_value_map(v, &FigKeyValue { key: k, value: s }, 1, &mut m));
        assert_eq!(FigStatus::Ok, fig_value_serialize(v, m, f, &mut p, &mut n));
        assert_eq!(b"k=v\n", &copied(p, n)[..]);
        let mut id = 0;
        assert_eq!(FigStatus::InvalidArgument, fig_value_extended(v, 42, b"x".as_ptr(), 1, &mut id));
        assert_eq!(FigStatus::Ok, fig_value_extended(v, 2, b"2024-01-02".as_ptr(), 10, &mut id));
        assert_eq!(FigStatus::UnsupportedFormat, fig_value_serialize(v, m, 0, &mut p, &mut n));
        fig_value_destroy(v);

        // No stream splitter; content sniffing skips what it cannot find.
        let (mut docs, mut count) = (null_mut(), 5usize);
        assert_eq!(FigStatus::UnsupportedOperation, fig_stream_split(src.as_ptr(), src.len(), f, &mut docs, &mut count, null_mut()));
        assert!(docs.is_null() && count == 0);
        let names = [c"nope".as_ptr(), c"capi-tinykv".as_ptr()];
        let mut found = 0;
        assert_eq!(FigStatus::Ok, fig_detect_order(src.as_ptr(), src.len(), names.as_ptr(), 2, &mut found));
        assert_eq!(f, found);
        assert_eq!(FigStatus::NotFound, fig_detect_order(b"nope".as_ptr(), 4, names.as_ptr(), 2, &mut found));
    }

    // The name is taken now.
    let mut again = new_error();
    let (st, f2) = register_tinykv(c"capi-tinykv", &mut again);
    assert_eq!(FigStatus::InvalidArgument, st);
    assert_eq!(-1, f2);
    assert!(message(&again).contains("already registered"), "{}", message(&again));
}

#[test]
fn a_record_the_rules_refuse_registers_nothing() {
    let mut err = new_error();
    let mut syntax = FigSyntax::DEFAULT;
    syntax.comments.trailing.open = c"#".as_ptr();
    syntax.kv_sep = c"=".as_ptr();
    let mut dialect = FigDialectDesc::DEFAULT;
    dialect.name = c"capi-refused".as_ptr();
    let samples = [FigStr::of(b"a=1\n")];
    let mut vt = FigLanguageVTable::DEFAULT;
    vt.version = VTABLE_VERSION;
    vt.name = c"capi-refused".as_ptr();
    vt.caps = 1 | 2;
    vt.syntax = &syntax;
    vt.dialects = &dialect;
    vt.dialect_count = 1;
    vt.samples = samples.as_ptr();
    vt.sample_count = 1;
    vt.parse = Some(tinykv_parse);
    vt.free_bytes = Some(tinykv_free);
    let mut format = 0;
    // SAFETY: the records are this frame's.
    let st = unsafe { fig_language_register(&vt, &mut format, &mut err) };
    assert_eq!(FigStatus::InvalidArgument, st);
    assert_eq!("'capi-refused' declares a trailing comment marker but no line comment marker", message(&err));
    // SAFETY: as above.
    assert_eq!(-1, unsafe { fig_format_by_name(c"capi-refused".as_ptr()) });

    vt.version = 2;
    // SAFETY: as above.
    assert_eq!(FigStatus::InvalidArgument, unsafe { fig_language_register(&vt, &mut format, &mut err) });
    assert_eq!("vtable version 2 is not the 3 this fig speaks", message(&err));

    vt.version = VTABLE_VERSION;
    vt.size = 16;
    // SAFETY: as above.
    assert_eq!(FigStatus::InvalidArgument, unsafe { fig_language_register(&vt, &mut format, &mut err) });
    assert!(message(&err).starts_with("the vtable's size 16 is too short"), "{}", message(&err));

    // SAFETY: null arguments, refused before anything is read.
    unsafe {
        assert_eq!(FigStatus::InvalidArgument, fig_language_register(null(), &mut format, &mut err));
        assert_eq!("vtable is null", message(&err));
        assert_eq!(FigStatus::InvalidArgument, fig_language_register(&vt, null_mut(), null_mut()));
    }
}

#[test]
fn the_registry_starts_with_no_language() {
    if cfg!(feature = "bundle") {
        return;
    }
    for name in [c"json", c"yaml", c"toml", c"dotenv"] {
        // SAFETY: NUL-terminated names.
        assert_eq!(-1, unsafe { fig_format_by_name(name.as_ptr()) });
    }
    let mut doc = null_mut();
    let mut err = new_error();
    // SAFETY: this test's pointers.
    assert_eq!(FigStatus::UnsupportedFormat, unsafe { fig_parse_ex(b"{}".as_ptr(), 2, 12345, &mut doc, &mut err) });
    assert_eq!("unsupported or unknown format", message(&err));
    // SAFETY: as above.
    assert_eq!(FigStatus::InvalidArgument, unsafe { fig_parse_ex(null(), 3, 1, &mut doc, &mut err) });
    assert_eq!("null input with nonzero length", message(&err));
}

// ============================================================================
// THE TABLE
// ============================================================================

#[test]
fn rows_written_out_of_order_are_put_in_pre_order() {
    let mut t = fig_core::kit::table::Table::init(&b"k=v"[..]);
    let tp: *mut FigTable = &mut t;
    // SAFETY: the table is this test's, lent to each call.
    unsafe {
        // Rows: 0 the key, 1 the value, 2 the keyvalue, 3 the mapping.
        let rows = fig_table_rows(tp, 4);
        let rows = core::slice::from_raw_parts_mut(rows, 4);
        rows[0] = Row { kind: Kind::STRING, text: Text { off: 0, len: 1 }, next_sibling: 1, first_child: table::NONE, ..Row::default() };
        rows[1] = Row { kind: Kind::STRING, text: Text { off: 2, len: 1 }, first_child: table::NONE, next_sibling: table::NONE, ..Row::default() };
        rows[2] = Row { kind: Kind::KEYVALUE, first_child: 0, next_sibling: table::NONE, ..Row::default() };
        rows[3] = Row { kind: Kind::MAPPING, first_child: 2, next_sibling: table::NONE, ..Row::default() };
        assert_eq!(2, fig_table_child_by_key(tp, 3, b"k".as_ptr(), 1));
        assert_eq!(table::NONE, fig_table_child_by_key(tp, 3, b"q".as_ptr(), 1));
        assert_eq!(FigStatus::InvalidArgument, fig_table_finish_in_order(tp));
        assert_eq!(FigStatus::InvalidArgument, fig_table_finish(tp, 9));
        assert_eq!(FigStatus::Ok, fig_table_finish(tp, 3));
        assert_eq!(FigStatus::InvalidArgument, fig_table_sep(tp, 9, 0, 1));
        assert_eq!(FigStatus::InvalidArgument, fig_table_comment(tp, 0, 3, 0, Text::ABSENT));
        assert!(fig_table_extend(tp, 12, 1).is_null());
        let anchors = fig_table_extend(tp, 2, 2).cast::<u8>();
        assert!(core::slice::from_raw_parts(anchors, 40).iter().all(|&b| b == 0));
    }
    let kinds: Vec<u8> = t.rows.iter().map(|r| r.kind).collect();
    assert_eq!(vec![Kind::MAPPING, Kind::KEYVALUE, Kind::STRING, Kind::STRING], kinds);
    assert_eq!(1, t.rows[2].parent);
    assert_eq!(2, t.anchors.len());
}

// ============================================================================
// THE KIT
// ============================================================================

#[test]
fn the_kit_rules_answer_as_rules_zig_does() {
    // SAFETY: every pointer is this test's, valid for each call.
    unsafe {
        let mut out = [0u8; 64];
        let n = fig_kit_number_canonical(b"0x1F".as_ptr(), 4, out.as_mut_ptr(), out.len());
        assert_eq!(b"31", &out[..n]);
        assert_eq!(usize::MAX, fig_kit_number_canonical(b"0xffff".as_ptr(), 6, out.as_mut_ptr(), 3));
        assert!(fig_kit_number_spellable(b"0x1F".as_ptr(), 4, 1));
        assert!(!fig_kit_number_spellable(b"0x1F".as_ptr(), 4, 0));
        assert_eq!(1, fig_kit_number_non_finite(b"-inf".as_ptr(), 4));
        assert_eq!(-1, fig_kit_number_non_finite(b"1".as_ptr(), 1));
        assert_eq!(2, fig_kit_datetime_classify(b"2024-02-29".as_ptr(), 10, 0));
        assert_eq!(-1, fig_kit_datetime_classify(b"nope".as_ptr(), 4, 3));
        assert!(!fig_kit_datetime_valid_date(b"2023-02-29".as_ptr(), 10));
        assert!(fig_kit_datetime_valid_time(b"12:30".as_ptr(), 5, 1));
        assert!(fig_kit_datetime_valid_offset(b"+05:30".as_ptr(), 6));
        assert_eq!(Kind::TRUE, fig_kit_literal_kind(b"true".as_ptr(), 4));
        assert_eq!(Kind::EXT + 2, fig_kit_literal_kind(b"2024-01-01".as_ptr(), 10));
        assert_eq!(Kind::STRING, fig_kit_literal_kind(b"inf".as_ptr(), 3));
        assert_eq!(Kind::FLOAT as i32, fig_kit_literal_number(b"1.5".as_ptr(), 3));
        assert_eq!(-1, fig_kit_literal_number(b"x".as_ptr(), 1));
        let n = fig_kit_json_string_quote(b"a\"b\n".as_ptr(), 4, b'"', out.as_mut_ptr(), out.len());
        assert_eq!(b"\"a\\\"b\\n\"", &out[..n]);
        assert_eq!(usize::MAX, fig_kit_json_string_quote(b"a".as_ptr(), 1, b'`', out.as_mut_ptr(), out.len()));

        let xml = b"<a b=\"c\">d</a>";
        let count = fig_kit_xml_tokenize(xml.as_ptr(), xml.len(), null_mut(), 0, null_mut(), null_mut());
        let mut toks = vec![FigXmlToken { kind: 0, reserved: [0; 3], start: 0, end: 0 }; count];
        assert_eq!(count, fig_kit_xml_tokenize(xml.as_ptr(), xml.len(), toks.as_mut_ptr(), count, null_mut(), null_mut()));
        assert_eq!(vec![0, 5, 5, 4, 6, 2, 7, 1, 5, 2, 9], toks.iter().map(|t| t.kind).collect::<Vec<_>>());
        let (mut code, mut at) = (0, 0);
        assert_eq!(usize::MAX, fig_kit_xml_tokenize(b"<a".as_ptr(), 2, null_mut(), 0, &mut code, &mut at));
        assert_eq!(1, code);
    }
}

#[test]
fn raw_memory_round_trips_and_zero_is_nothing() {
    assert!(fig_alloc(0).is_null());
    let p = fig_alloc(16);
    assert!(!p.is_null());
    // SAFETY: `p` is 16 bytes from `fig_alloc`, freed once with its length.
    unsafe {
        p.write_bytes(7, 16);
        fig_free(p, 16);
        fig_free(null_mut(), 16);
    }
}
