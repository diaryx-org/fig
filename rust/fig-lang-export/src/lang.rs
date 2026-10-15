//! What the C library and the wasm module both do with the language: name
//! its dialect rows, parse into a table of their own, print a table fig
//! hands over, render and split a stream — `kit/src/contract.zig`'s
//! `VTable` thunks, over the `Language` trait.

use std::sync::Arc;

use fig_abi::{FigStreamDoc, FigTableView};
use fig_core::kit::convert;
use fig_core::kit::language::{Dialect, Language, Literal, RenderRequest, Renderer};
use fig_core::kit::table::{self, Table};
use fig_core::kit::ast::SerializeOptions;
use fig_core::kit::Error;

/// A dialect row, as `kit/contract.zig`'s `VTable` lists them: each
/// declared dialect, then each `--spec` version of one, named
/// `<dialect>-<version>`, which claims no extension of its own.
// The wasm module reads a row's name alone; the C library all of it.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub struct Row {
    pub name: Vec<u8>,
    /// The declared row it is, or is a version of.
    pub of: usize,
    pub dialect: Dialect,
    pub declared: bool,
}

/// `l`'s rows.
pub fn rows(l: &dyn Language) -> Vec<Row> {
    let dialects = l.dialects();
    let mut out: Vec<Row> = dialects
        .iter()
        .enumerate()
        .map(|(i, d)| Row { name: d.name.as_bytes().to_vec(), of: i, dialect: d.dialect, declared: true })
        .collect();
    for (i, d) in dialects.iter().enumerate() {
        for sp in d.specs {
            let mut name = d.name.as_bytes().to_vec();
            name.push(b'-');
            name.extend_from_slice(sp.name.as_bytes());
            out.push(Row { name, of: i, dialect: sp.dialect, declared: false });
        }
    }
    out
}

/// The dialect the row named `name` selects (`typeOf`).
pub fn dialect_of(rows: &[Row], name: &[u8]) -> Option<Dialect> {
    rows.iter().find(|r| r.name == name).map(|r| r.dialect)
}

/// A refusal: the message and where it points.
pub struct Refusal {
    pub message: Vec<u8>,
    pub byte_offset: usize,
    /// A printer's refusal, as fig.h's `FigRefusal` numbers it; 0 for any
    /// other.
    pub code: i32,
}

impl Refusal {
    pub fn of(message: &[u8]) -> Refusal {
        Refusal { message: message.to_vec(), byte_offset: 0, code: 0 }
    }
}

/// Parse `input` in `dialect` into a table of this library's own. A parser
/// with a report has written its errors (or, on success, its warnings) into
/// the table's diagnostics; a refusal is the first error it wrote, else the
/// parser's error name (`kit/contract.zig`'s `parse`).
pub fn parse(l: &dyn Language, dialect: Dialect, input: &[u8]) -> (Table, Result<(), Refusal>) {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    let parsed = l.parse_table(&source, dialect, &mut t);
    let refused = parsed.map_err(|e| {
        match t.diagnostics.iter().find(|d| d.severity == table::DIAGNOSTIC_ERROR) {
            Some(d) => Refusal { message: t.text_of(d.message).unwrap_or(b"").to_vec(), byte_offset: d.start as usize, code: 0 },
            None => Refusal::of(e.0.as_bytes()),
        }
    });
    (t, refused)
}

/// The view `v` describes, borrowing what it points to.
///
/// # Safety
///
/// Every pointer in `v` must be null with a zero count, or point to that
/// many readable elements, for `'a`.
pub unsafe fn view_of<'a>(v: &FigTableView) -> table::View<'a> {
    /// # Safety
    ///
    /// As `view_of`, for the one list.
    unsafe fn list<'a, T>(p: *const T, n: usize) -> &'a [T] {
        // SAFETY: the caller's contract.
        if p.is_null() || n == 0 { &[] } else { unsafe { core::slice::from_raw_parts(p, n) } }
    }
    // SAFETY: the caller's contract, for each list.
    unsafe {
        table::View {
            source: list(v.source, v.source_len),
            strings: list(v.strings, v.strings_len),
            rows: list(v.rows, v.row_count),
            anchors: list(v.anchors, v.anchor_count),
            tags: list(v.tags, v.tag_count),
            markers: list(v.markers, v.marker_count),
            seps: list(v.seps, v.sep_count),
            item_seps: list(v.item_seps, v.item_sep_count),
            comments: list(v.comments, v.comment_count),
            regions: list(v.regions, v.region_count),
            mentions: list(v.mentions, v.mention_count),
            directives: list(v.directives, v.directive_count),
            diagnostics: list(v.diagnostics, v.diagnostic_count),
        }
    }
}

/// The options a print is handed, as the language's printer reads them.
pub fn serialize_options(pretty: bool, strip_comments: bool, indent: u8, width: u16, splice: bool, flow: bool) -> SerializeOptions {
    SerializeOptions { pretty, strip_comments, indent, width, splice, flow, ..SerializeOptions::default() }
}

/// Print the table `view` shows in `dialect` (`kit/contract.zig`'s `print`).
pub fn print(l: &dyn Language, dialect: Dialect, view: &table::View<'_>, options: &SerializeOptions) -> Result<Vec<u8>, Refusal> {
    let doc = convert::view_to_document(Arc::from(&b""[..]), view).map_err(|_| Refusal::of(b"a malformed table"))?;
    let mut out = Vec::new();
    l.print(dialect, &doc.ast, options, &mut out).map_err(|e| Refusal { code: refusal_code(e), ..Refusal::of(e.0.as_bytes()) })?;
    Ok(out)
}

/// `refusalOf`: the `FigRefusal` a printer's error is.
fn refusal_code(e: Error) -> i32 {
    match e.0 {
        "NullUnsupported" => 1,
        "NonStringKey" => 2,
        "UnrepresentableRoot" => 3,
        "UnsupportedValue" => 4,
        "InvalidKey" => 5,
        "NonFiniteNumber" => 6,
        "UnresolvedAlias" => 7,
        "NestingTooDeep" => 8,
        "UnexpectedNodeKind" => 9,
        _ => 0,
    }
}

/// The renderers, in fig.h's slot order: `value`, `entry`, `item`, `tail`,
/// `key`, `key_name`, `string`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub const RENDERERS: [Renderer; 7] =
    [Renderer::Value, Renderer::Entry, Renderer::Item, Renderer::Tail, Renderer::Key, Renderer::KeyName, Renderer::String];

/// The literal kind a request names; a name fig does not know is a string
/// (`Literal.parse(...) orelse .string`).
pub fn literal_of(name: &[u8]) -> Literal {
    Literal::parse(name).unwrap_or(Literal::String)
}

/// Render `which`; `None` when the language has no such renderer. A
/// refusal is the renderer's error name.
pub fn render(l: &dyn Language, which: Renderer, dialect: Dialect, r: &RenderRequest<'_>) -> Option<Result<Vec<u8>, Refusal>> {
    let mut out = Vec::new();
    Some(l.render(which, dialect, &mut out, r)?.map(|()| out).map_err(|e| Refusal::of(e.0.as_bytes())))
}

/// Split `input` into its documents, as the `FigStreamDoc` array fig.h's
/// `split_stream` returns; `None` for a language without streams.
pub fn split_stream(l: &dyn Language, dialect: Dialect, input: &[u8]) -> Option<Result<Vec<FigStreamDoc>, Refusal>> {
    let docs = match l.split_stream(input, dialect)? {
        Ok(d) => d,
        Err(e) => return Some(Err(Refusal::of(e.0.as_bytes()))),
    };
    Some(Ok(docs.iter().map(|d| FigStreamDoc { start: d.start, end: d.end, marked: d.explicit }).collect()))
}
