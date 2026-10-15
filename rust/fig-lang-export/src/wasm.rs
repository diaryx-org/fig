//! The language as a WebAssembly module of its own
//! (`kit/src/wasm_language.zig`): the exports the TypeScript binding drives
//! (`bindings/typescript/src/native.ts`) to register it into fig's module
//! and to parse and print through it.
//!
//! The two modules do not share memory. A parse writes its table here
//! (`fig_lang_parse`), and the binding copies the table's lists into the
//! table fig handed its own parse. A print is the same in the other
//! direction (`fig_lang_print`). Everything rarer — the description, a
//! renderer, a stream split — goes as the helper wire's JSON-RPC
//! (`fig_lang_call`), the way a helper answers it.

use std::alloc::{alloc, dealloc, Layout};
use std::cell::RefCell;

use fig_abi::{FigError, FigPrintOptions, FigStr, FigTableView};
use fig_core::kit::describe_json;
use fig_core::kit::language::{Language, RenderRequest, Renderer};
use fig_core::kit::table::Table;
use fig_core::wire::{json, json_string};

use crate::lang::{self, Refusal};

/// The byte sizes the binding copies each list by (`native.ts`'s
/// `LIST_SIZES`), in the order `fig_table_extend` numbers them, and the size
/// of the view it lays out: a change here is a change there.
const _: () = {
    use core::mem::size_of;
    use fig_core::kit::table::{Anchor, Comment, Diagnostic, Directive, ItemSep, Mention, Row, SpanFact, Tag};
    let sizes = [
        size_of::<Row>(),
        1,
        size_of::<Anchor>(),
        size_of::<Tag>(),
        size_of::<SpanFact>(),
        size_of::<SpanFact>(),
        size_of::<ItemSep>(),
        size_of::<Comment>(),
        size_of::<SpanFact>(),
        size_of::<Mention>(),
        size_of::<Directive>(),
        size_of::<Diagnostic>(),
    ];
    let want = [32, 1, 20, 24, 12, 12, 12, 16, 12, 16, 16, 28];
    let mut i = 0;
    while i < sizes.len() {
        assert!(sizes[i] == want[i], "a table list's element size changed; change native.ts with it");
        i += 1;
    }
    assert!(size_of::<FigTableView>() == 108, "FigTableView's layout changed; change native.ts with it");
};

/// Everything the binding allocates here, and everything handed to it, is
/// laid out alike, so `fig_lang_free` frees either by its length.
fn layout(n: usize) -> Layout {
    Layout::from_size_align(n.max(1), 8).expect("an allocation fits wasm32's address space")
}

/// `fig_lang_alloc`: `n` bytes the binding writes into, freed with
/// `fig_lang_free`.
pub fn alloc_bytes(n: usize) -> *mut u8 {
    // SAFETY: `layout` is never zero-sized.
    unsafe { alloc(layout(n)) }
}

/// `fig_lang_free`.
///
/// # Safety
///
/// `p` is null, or what `alloc_bytes(n)` returned, freed once.
pub unsafe fn free_bytes(p: *mut u8, n: usize) {
    if !p.is_null() {
        // SAFETY: the caller's contract.
        unsafe { dealloc(p, layout(n)) }
    }
}

/// `bytes` copied into memory the binding frees with `fig_lang_free`, or
/// `None` without the memory.
fn give(bytes: &[u8]) -> Option<FigStr> {
    let p = alloc_bytes(bytes.len());
    if p.is_null() {
        return None;
    }
    // SAFETY: `p` is at least `bytes.len()` fresh bytes.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
    Some(FigStr { ptr: p, len: bytes.len() })
}

/// # Safety
///
/// `err` is a whole `FigError` (the binding lays one out at fig.h's size).
unsafe fn fail(err: *mut FigError, r: &Refusal) {
    // SAFETY: the caller's contract.
    let e = unsafe { &mut *err };
    let n = r.message.len().min(e.message.len());
    e.message[..n].copy_from_slice(&r.message[..n]);
    e.message_len = n;
    e.byte_offset = r.byte_offset;
    if r.code != 0 {
        e.code = r.code;
    }
}

/// # Safety
///
/// `p` is null with `n == 0`, or `n` readable bytes.
unsafe fn bytes<'a>(p: *const u8, n: usize) -> &'a [u8] {
    // SAFETY: the caller's contract.
    if p.is_null() || n == 0 { &[] } else { unsafe { core::slice::from_raw_parts(p, n) } }
}

thread_local! {
    /// The table of the last parse, kept until the next, so the binding can
    /// read it after `fig_lang_parse` returns.
    static LAST: RefCell<Option<Table>> = const { RefCell::new(None) };
}

/// `fig_lang_parse`: parse `input` in `dialect` into this module's table and
/// describe it in `out_view`, valid until the next parse. A refusal fills
/// `err`, and the table's diagnostics say more.
///
/// # Safety
///
/// Every pointer is the binding's, into this module's memory, as
/// `native.ts` lays them out.
pub unsafe fn parse(l: &dyn Language, dialect: &[u8], input: &[u8], out_view: *mut FigTableView, err: *mut FigError) -> i32 {
    let rows = lang::rows(l);
    let Some(d) = lang::dialect_of(&rows, dialect) else {
        // SAFETY: the caller's contract.
        unsafe { fail(err, &Refusal::of(b"no such dialect")) };
        return 2;
    };
    let (t, parsed) = lang::parse(l, d, input);
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        let t = last.insert(t);
        // SAFETY: the caller's contract; the view borrows the table kept
        // in `LAST` until the next parse.
        unsafe { *out_view = FigTableView::of(&t.view()) };
    });
    match parsed {
        Ok(()) => 0,
        Err(r) => {
            // SAFETY: the caller's contract.
            unsafe { fail(err, &r) };
            2
        }
    }
}

/// `fig_lang_print`: print the table `view` shows in `dialect`; `out` is
/// bytes the binding frees with `fig_lang_free`.
///
/// # Safety
///
/// As `parse`.
pub unsafe fn print(
    l: &dyn Language,
    dialect: &[u8],
    view: *const FigTableView,
    options: *const FigPrintOptions,
    out: *mut FigStr,
    err: *mut FigError,
) -> i32 {
    let rows = lang::rows(l);
    // SAFETY: the caller's contract, for every pointer.
    unsafe {
        let Some(d) = lang::dialect_of(&rows, dialect) else {
            fail(err, &Refusal::of(b"no such dialect"));
            return 2;
        };
        if !l.caps().serialize {
            fail(err, &Refusal::of(b"this language does not print"));
            return 2;
        }
        let o = &*options;
        let opts = lang::serialize_options(o.pretty, o.strip_comments, o.indent, o.width, o.splice, o.flow);
        match lang::print(l, d, &lang::view_of(&*view), &opts) {
            Ok(b) => match give(&b) {
                Some(s) => {
                    *out = s;
                    0
                }
                None => 3,
            },
            Err(r) => {
                fail(err, &r);
                2
            }
        }
    }
}

/// `fig_lang_call`: answer one JSON-RPC request line — `describe`,
/// `render`, `split_stream` — with one response line in `out`, which the
/// binding frees with `fig_lang_free`. Nonzero only when there is no memory
/// for an answer.
///
/// # Safety
///
/// As `parse`.
pub unsafe fn call(l: &dyn Language, request: &[u8], out: *mut FigStr) -> i32 {
    let mut w = Vec::new();
    answer(l, request, &mut w);
    match give(&w) {
        Some(s) => {
            // SAFETY: the caller's contract.
            unsafe { *out = s };
            0
        }
        None => 1,
    }
}

/// A JSON-RPC request with an integer id and an object of string params —
/// all a language module is asked. A param that is not a string is skipped.
struct Request {
    id: i64,
    method: String,
    params: Vec<(String, String)>,
}

impl Request {
    fn read(line: &[u8]) -> Option<Request> {
        let v = json::parse(line).ok()?;
        let o = v.as_object()?;
        let id = match o.get("id") {
            None => 0,
            Some(id) => id.as_i64()?,
        };
        let method = match o.get("method") {
            None => String::new(),
            Some(m) => m.as_str()?.to_owned(),
        };
        let params = match o.get("params") {
            None => Vec::new(),
            Some(p) => p
                .as_object()?
                .entries
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect(),
        };
        Some(Request { id, method, params })
    }

    fn str(&self, name: &str) -> &[u8] {
        self.params.iter().find(|(n, _)| n == name).map_or(&[], |(_, v)| v.as_bytes())
    }

    fn has(&self, name: &str) -> bool {
        self.params.iter().any(|(n, _)| n == name)
    }
}

fn answer(l: &dyn Language, line: &[u8], w: &mut Vec<u8>) {
    let Some(req) = Request::read(line) else {
        w.extend_from_slice(
            b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32700,\"message\":\"the request is not a JSON-RPC request\"}}",
        );
        return;
    };
    w.extend_from_slice(format!("{{\"jsonrpc\":\"2.0\",\"id\":{},", req.id).as_bytes());
    if req.method == "describe" {
        w.extend_from_slice(b"\"result\":");
        describe_json::write(w, l);
        w.push(b'}');
        return;
    }
    let rows = lang::rows(l);
    // A row this language does not have is the language's refusal, as the
    // vtable's functions answer it.
    let dialect = lang::dialect_of(&rows, req.str("dialect"));
    let result: Result<Vec<u8>, Refusal> = match req.method.as_str() {
        "render" => {
            let which = Renderer::ALL.into_iter().find(|r| r.name().as_bytes() == req.str("which"));
            let Some(which) = which.filter(|&r| l.has_renderer(l.default_dialect(), r)) else {
                return refuse(w, -32601, "this language has no such renderer");
            };
            let literal = if req.has("literal") { req.str("literal") } else { b"string" };
            let r = RenderRequest {
                indent: req.str("indent"),
                key: req.str("key"),
                value: req.str("value"),
                literal: lang::literal_of(literal),
                old_key: req.str("old_key"),
                parent_key: req.str("parent_key"),
                parent_tag: req.str("parent_tag"),
                old_value: req.str("old_value"),
            };
            match dialect {
                None => Err(Refusal::of(b"no such dialect")),
                Some(d) => lang::render(l, which, d, &r).unwrap_or_else(|| Err(Refusal::of(b"this language has no such renderer"))),
            }
        }
        "split_stream" => {
            if !l.has_split_stream() {
                return refuse(w, -32601, "this language has no streams");
            }
            match dialect {
                None => Err(Refusal::of(b"no such dialect")),
                Some(d) => match lang::split_stream(l, d, req.str("input")) {
                    Some(Ok(docs)) => {
                        let mut out = vec![b'['];
                        for (i, doc) in docs.iter().enumerate() {
                            if i > 0 {
                                out.push(b',');
                            }
                            out.extend_from_slice(
                                format!("{{\"start\":{},\"end\":{},\"explicit\":{}}}", doc.start, doc.end, doc.marked).as_bytes(),
                            );
                        }
                        out.push(b']');
                        Ok(out)
                    }
                    Some(Err(e)) => Err(e),
                    None => Err(Refusal::of(b"this language has no streams")),
                },
            }
        }
        _ => return refuse(w, -32601, "no such method"),
    };
    match result {
        Ok(bytes) => {
            w.extend_from_slice(b"\"result\":");
            if req.method == "render" {
                json_string(w, &bytes);
            } else {
                w.extend_from_slice(&bytes);
            }
            w.push(b'}');
        }
        Err(r) => {
            w.extend_from_slice(b"\"error\":{\"code\":-32000,\"message\":");
            json_string(w, &r.message);
            w.extend_from_slice(format!(",\"data\":{{\"byte_offset\":{}}}}}}}", r.byte_offset).as_bytes());
        }
    }
}

fn refuse(w: &mut Vec<u8>, code: i32, message: &str) {
    w.extend_from_slice(format!("\"error\":{{\"code\":{code},\"message\":\"{message}\"}}}}").as_bytes());
}

/// The module's exports for the language `$lang`, a `&'static dyn
/// Language`.
macro_rules! exports {
    ($lang:expr) => {
        const _: () = {
            use fig_abi::{FigError, FigPrintOptions, FigStr, FigTableView};
            use $crate::wasm;

            #[unsafe(no_mangle)]
            pub extern "C" fn fig_lang_alloc(n: usize) -> *mut u8 {
                wasm::alloc_bytes(n)
            }

            /// # Safety
            ///
            /// `p` is what `fig_lang_alloc(n)` returned, freed once.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn fig_lang_free(p: *mut u8, n: usize) {
                // SAFETY: the caller's contract.
                unsafe { wasm::free_bytes(p, n) }
            }

            /// `sizeof(FigTableView)` here, so the binding lays one out.
            #[unsafe(no_mangle)]
            pub extern "C" fn fig_lang_view_size() -> usize {
                core::mem::size_of::<FigTableView>()
            }

            /// # Safety
            ///
            /// The binding's pointers into this module's memory.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn fig_lang_parse(
                dialect: *const u8,
                dialect_len: usize,
                input: *const u8,
                input_len: usize,
                out_view: *mut FigTableView,
                err: *mut FigError,
            ) -> i32 {
                // SAFETY: the caller's contract.
                unsafe { wasm::parse($lang, wasm::slice(dialect, dialect_len), wasm::slice(input, input_len), out_view, err) }
            }

            /// # Safety
            ///
            /// As `fig_lang_parse`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn fig_lang_print(
                dialect: *const u8,
                dialect_len: usize,
                view: *const FigTableView,
                options: *const FigPrintOptions,
                out: *mut FigStr,
                err: *mut FigError,
            ) -> i32 {
                // SAFETY: the caller's contract.
                unsafe { wasm::print($lang, wasm::slice(dialect, dialect_len), view, options, out, err) }
            }

            /// # Safety
            ///
            /// As `fig_lang_parse`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn fig_lang_call(request: *const u8, request_len: usize, out: *mut FigStr) -> i32 {
                // SAFETY: the caller's contract.
                unsafe { wasm::call($lang, wasm::slice(request, request_len), out) }
            }
        };
    };
}
pub(crate) use exports;

/// `n` bytes at `p`, for the exports.
///
/// # Safety
///
/// As `bytes`.
pub unsafe fn slice<'a>(p: *const u8, n: usize) -> &'a [u8] {
    // SAFETY: the caller's contract.
    unsafe { bytes(p, n) }
}
