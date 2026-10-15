//! Port of kit/src/contract.zig + src/contract/language.zig (the Language trait).
//!
//! In Zig a language is a module whose `Language` struct declares `Type`
//! (its dialect enum), `default_type`, `parse`/`parseTable`,
//! `print`/`printNode`, `name`, `extensions`, `caps`, `samples`,
//! `dialects`, `syntax(t)`, and — optionally — the fragment renderers the
//! editor dispatches on with `@hasDecl` (`src/editor.zig`'s `hasRenderer`).
//! `src/contract/language.zig`'s `validate` holds a module to that at
//! comptime; `kit/contract.zig` turns a module into the C vtable a runtime
//! registry (`src/contract/runtime.zig`) calls through.
//!
//! In Rust the registry is runtime, so a language is a value implementing
//! the object-safe trait `Language` (`&dyn Language` is how the editor, the
//! serializer and the registry hold one). The trait is the union of the
//! module declarations and the vtable:
//!
//! - a dialect is a `Dialect` (`u8`), the ordinal of Zig's per-language
//!   `Type` enum (JSON: 0 `JSON`, 1 `JSONC`, 2 `JSON5`); a language maps it
//!   to its own enum internally;
//! - `parse_table` is the vtable's `parse` thunk: rows and diagnostics
//!   written into a table; `parse` / `parse_with_report` adopt it;
//! - `print` is the vtable's `print` thunk (`contract.zig`'s `printWith`
//!   dispatch: the splice printer, the fragment printer or the document
//!   printer of the dialect), and `print_node` Zig's `printNode`;
//! - each renderer is a method whose default returns `None`, meaning "this
//!   language has none" (the `@hasDecl` that fails); a language that has
//!   one returns `Some` for EVERY request.
//!
//! Everything a language says about itself (`name`, `extensions`,
//! `dialects`, `samples`, `Syntax`'s strings) is `'static`: a compiled
//! language's are constants, and a runtime language leaks its own at
//! registration, which in Zig lives as long as the process.

use std::sync::Arc;

use super::ast::{NodeId, SerializeOptions, AST};
use super::convert;
use super::document::Document;
use super::table::{self, Table};
use crate::err;
use crate::Result;

pub use super::manifest::{
    Caps, ClosedContainers, CommentDelimiter, Comments, Delimiters, Dialect, DialectInfo, Literal, NativeKinds, Prints,
    PrintsComments, RenderRequest, Renderer, SectionHeader, SpecName, SpliceStyle, StreamDocument, Syntax,
};

/// One diagnostic a parse left, its texts copied out of the table: what
/// Zig's runtime `Language.Parser` collects into its `errors` and
/// `warnings` sinks (`parse_diagnostic.Rendered`'s fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseDiagnostic {
    pub offset: usize,
    pub end: Option<usize>,
    /// A sentence that says what is wrong and how to fix it.
    pub message: Vec<u8>,
    /// A few words for the caret's label.
    pub short_label: Vec<u8>,
}

/// Where `Language::parse_with_report` puts a parse's diagnostics: the
/// errors a failed parse left (every one, for a parser that recovers), and
/// the warnings a clean one raised.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseReport {
    pub errors: Vec<ParseDiagnostic>,
    pub warnings: Vec<ParseDiagnostic>,
}

impl ParseReport {
    /// The table's diagnostics, into the report (Zig's runtime
    /// `collectDiagnostics`).
    pub fn collect(&mut self, t: &Table) {
        for d in &t.diagnostics {
            let rec = ParseDiagnostic {
                offset: d.start as usize,
                end: if d.end == table::NONE { None } else { Some(d.end as usize) },
                message: t.text_of(d.message).unwrap_or(b"").to_vec(),
                short_label: t.text_of(d.label).unwrap_or(b"").to_vec(),
            };
            if d.severity == table::DIAGNOSTIC_WARNING {
                self.warnings.push(rec);
            } else {
                self.errors.push(rec);
            }
        }
    }
}

/// Append a diagnostic to `out`, as `kit/contract.zig`'s `addDiagnostic`
/// does for a parser's report: `severity` is `table::DIAGNOSTIC_*`, `end`
/// `None` for a point.
pub fn add_diagnostic(out: &mut Table, severity: u8, offset: usize, end: Option<usize>, message: &[u8], label: &[u8]) -> Result<()> {
    let message = out.add_string(message)?;
    let label = out.add_string(label)?;
    out.diagnostics.push(table::Diagnostic {
        severity,
        start: offset as u32,
        end: end.map_or(table::NONE, |e| e as u32),
        message,
        label,
    });
    Ok(())
}

thread_local! {
    /// The message and byte offset a refusing parser states apart from its
    /// diagnostics: what a runtime language's `parse` writes into the C
    /// `ErrorInfo` (`setErr`), as the helper wire does for a helper's
    /// refusal. Thread-local: the parse that sets it and the registry's
    /// `Parser` that reads it run on one thread.
    static PARSE_ERROR: std::cell::RefCell<Option<(Vec<u8>, usize)>> = const { std::cell::RefCell::new(None) };
}

/// State why the parse under way refuses, and where, without a diagnostic
/// (the vtable's `ErrorInfo`): the registry's `Parser` reports it ahead of
/// the table's first error. For a language whose refusal is a message, not
/// a diagnostic — a helper's JSON-RPC error.
pub fn set_parse_error(message: &[u8], offset: usize) {
    PARSE_ERROR.with(|e| *e.borrow_mut() = Some((message.to_vec(), offset)));
}

/// Take what `set_parse_error` stated, leaving none.
pub fn take_parse_error() -> Option<(Vec<u8>, usize)> {
    PARSE_ERROR.with(|e| e.borrow_mut().take())
}

/// A language: what a Zig `Language` module declares, as a trait object the
/// registry holds. See the module doc.
pub trait Language: Send + Sync + std::panic::RefUnwindSafe {
    // ── Identity ────────────────────────────────────────────────────────────

    /// Zig `Language.name`: the language's name, which its default dialect
    /// row is named after (`"json"`, `"yaml"`, `"figl"`).
    fn name(&self) -> &str;

    /// Zig `Language.extensions`: the file extensions, without the dot. An
    /// extension named after a dialect row belongs to that dialect, every
    /// other to the first (`kit/contract.zig`'s `extensionsOf`).
    fn extensions(&self) -> &[&'static str];

    /// The extensions of dialect row `row` (an index into `dialects()`), for
    /// a language that states them per row, as a runtime language's C record
    /// does (`DialectDesc.extensions`). `None`, the default, hands out
    /// `extensions()` by name: an extension named after a dialect row belongs
    /// to that row, every other to the first.
    fn dialect_extensions(&self, row: usize) -> Option<Vec<&'static str>> {
        let _ = row;
        None
    }

    /// Zig `Language.caps`.
    fn caps(&self) -> Caps;

    /// Zig `Language.dialects`: the language's rows of the format registry,
    /// at least one, exactly one of which selects `default_dialect` and is
    /// named `name()`.
    fn dialects(&self) -> &[DialectInfo];

    /// Zig `Language.default_type`, as an ordinal. 0 for every language in
    /// tree.
    fn default_dialect(&self) -> Dialect {
        0
    }

    /// How many values Zig's `Type` enum has: every `Dialect` below it is
    /// one (`std.meta.tags(Lang.Type)`). By default, one past the largest
    /// dialect any row or `--spec` selects.
    fn type_count(&self) -> usize {
        let mut max = self.default_dialect();
        for d in self.dialects() {
            max = max.max(d.dialect);
            for s in d.specs {
                max = max.max(s.dialect);
            }
        }
        max as usize + 1
    }

    /// Zig `Language.samples`: a few small documents in the format's own
    /// grammar that the contract harness parses, prints, reparses and edits
    /// to check what the engine assumes of every format.
    fn samples(&self) -> &[&'static [u8]] {
        &[]
    }

    /// Zig `Language.syntax(t)`: how the generic splice engine writes this
    /// format in `dialect`. Required of an editable language; the default is
    /// the runtime registry's answer for a read-only one (no comments, no
    /// separator, no seed), which nothing should ask.
    fn syntax(&self, dialect: Dialect) -> Syntax {
        let _ = dialect;
        Syntax::new(Comments::NONE, None, None)
    }

    /// Zig `Language.runtime`: this language's dialect table is not a
    /// compile-time fact (a registry of runtime formats), so the editor's two
    /// gates that read every dialect's `syntax` (may it have sections, may it
    /// spell a header) answer "ask the entry".
    fn is_runtime(&self) -> bool {
        false
    }

    // ── Parsing ─────────────────────────────────────────────────────────────

    /// Zig `Language.parseTable`, as `kit/contract.zig`'s `parse` thunk
    /// calls it: write `input`'s rows into `out`, a table over `input`
    /// (`Table::init(input)`). Slice `out.source_arc()` — the same bytes — so
    /// that in-source texts stay in the source (`Table::text`).
    ///
    /// A parser with a report says why it refused, where, and what it would
    /// warn about on input it reads: on a refusal it appends each error it
    /// found (`add_diagnostic`, `table::DIAGNOSTIC_ERROR`; every one, for a
    /// parser that recovers past errors, as Zig's `parseCollectingInto`) and
    /// returns its Zig error name; on success it appends its warnings. A
    /// parser need not finish the table on a refusal.
    fn parse_table(&self, input: &[u8], dialect: Dialect, out: &mut Table) -> Result<()>;

    /// Zig `Language.parse`: the document fig would adopt from a parse of
    /// `input` in `dialect`, its `merge_key` the dialect's
    /// `Syntax::merge_key` for an editable language (YAML's `<<`). Fails
    /// with the parser's own error name, or the table's (`MalformedTable`,
    /// `UnknownKind`) when it wrote a malformed one.
    fn parse(&self, input: &[u8], dialect: Dialect) -> Result<Document> {
        self.parse_with_report(input, dialect, &mut ParseReport::default())
    }

    /// `parse`, with the diagnostics the parse left — the errors of a failed
    /// one and the warnings of a clean one — in `report` (Zig's runtime
    /// `Language.Parser` with its `errors` and `warnings` sinks, and the
    /// compiled parsers' `parseWithReport`).
    fn parse_with_report(&self, input: &[u8], dialect: Dialect, report: &mut ParseReport) -> Result<Document> {
        let source: Arc<[u8]> = Arc::from(input);
        let mut t = Table::init(source.clone());
        let parsed = self.parse_table(input, dialect, &mut t);
        report.collect(&t);
        parsed?;
        let mut doc = convert::table_to_document(source, &mut t)?;
        if self.caps().edit {
            doc.ast.merge_key = self.syntax(dialect).merge_key;
        }
        Ok(doc)
    }

    /// Zig `Language.splitStream`: the documents of a multi-document stream
    /// (YAML's `---`/`...`), each one's byte range for a parse of its own;
    /// `None` for a language without streams (the runtime registry's
    /// `UnsupportedOperation`). A language that has one returns `Some` for
    /// every input.
    fn split_stream(&self, input: &[u8], dialect: Dialect) -> Option<Result<Vec<StreamDocument>>> {
        let _ = (input, dialect);
        None
    }

    /// Whether `split_stream` is declared (the vtable's `split_stream` slot
    /// is non-null). By default, `split_stream` asked of an empty input.
    fn has_split_stream(&self) -> bool {
        self.split_stream(b"", self.default_dialect()).is_some()
    }

    // ── Printing ────────────────────────────────────────────────────────────

    /// Zig `Language.print` as `kit/contract.zig`'s `print` thunk reaches it
    /// (`printWith`): write `ast` from its root in `dialect` into `out` — the
    /// dialect's splice printer (`printSplice`) when `options.splice` and the
    /// language has one; else, when the dialect names a fragment printer
    /// (`DialectInfo::print_fragment_name`), that one for a splice, a `flow`
    /// fragment or a root that is not a container; else the dialect's
    /// document printer. Core strips comments before calling when
    /// `options.strip_comments` is set (`serialize`'s comment view), as it
    /// does for a compiled printer. Fails with a `SerializeError` name
    /// (`ast::SERIALIZE_ERRORS`). Required when `caps().serialize`; the
    /// default refuses with `FormatDisabled`.
    fn print(&self, dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        let _ = (dialect, ast, options, out);
        Err(err!(FormatDisabled))
    }

    /// Zig `Language.printNode`: the subtree at `node` as the whole
    /// document. By default, as the runtime registry prints one
    /// (`printNodeWith`): the subtree's table (`convert::document_to_table`,
    /// stripped of comments when asked), read back and handed to `print`.
    fn print_node(&self, dialect: Dialect, ast: &AST, node: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        let mut t = convert::document_to_table(ast, node)?;
        if options.strip_comments {
            t.comments.clear();
        }
        let doc = convert::view_to_document(Arc::from(&b""[..]), &t.view())?;
        self.print(dialect, &doc.ast, options, out)
    }

    // ── Renderers ───────────────────────────────────────────────────────────
    //
    // Each is a pure function from the dialect and a `RenderRequest` to a
    // fragment the engine splices under the reparse net, appended to `out`
    // (Zig: `renderX(t, allocator, out, request) !void`). None receives the
    // editor or performs a splice; the engine calls one at most once per
    // edit. The default `None` is a language without that renderer; one that
    // has it returns `Some` for every request. See `src/editor.zig`'s
    // `renderedValue`, `writeEntry`, `writeItem`, `writeTail` and
    // `replaceKeyAtPath`, and `docs/proposals/runtime-languages.md` §4.4.

    /// Zig `renderValue`: a value spelled in place (plist's typed element),
    /// given what fig's bare-literal rules make of the text (`r.literal`).
    fn render_value(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        let _ = (dialect, out, r);
        None
    }

    /// Zig `renderEntry`: a block-mapping entry past its line's indent
    /// (plist's two-line pair, NestedText's `key:` and `>`-block), for a
    /// format whose entry is not `key`, `kv_sep`, value on one line. No
    /// trailing newline.
    fn render_entry(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        let _ = (dialect, out, r);
        None
    }

    /// Zig `renderItem`: a block-sequence item past its line's indent, for a
    /// format whose item is not marker-then-value. No trailing newline.
    fn render_item(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        let _ = (dialect, out, r);
        None
    }

    /// Zig `renderTail`: what follows a key on its entry — the separator and
    /// the value inline, or the value re-framed as a block on the following
    /// lines. `r.key` is empty for the document root. No trailing newline.
    fn render_tail(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        let _ = (dialect, out, r);
        None
    }

    /// Zig `renderKey`: a renamed key (`r.key`) in the form the old one
    /// (`r.old_key`, as the source spells it) allows.
    fn render_key(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        let _ = (dialect, out, r);
        None
    }

    /// Zig `renderKeyName`: a key's NAME (`r.key`) as the language writes a
    /// key (JSON quotes it, ZON writes `.name`); without it a name is
    /// written as it stands.
    fn render_key_name(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        let _ = (dialect, out, r);
        None
    }

    /// Zig `renderString`: a string (`r.value`, decoded) that replaces a
    /// string (`r.old_value`, as spelled) in the old one's quote style;
    /// nothing written where that style is the printer's own or cannot hold
    /// the new value, and the engine then splices what the caller rendered.
    fn render_string(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        let _ = (dialect, out, r);
        None
    }

    /// The renderer `which`, dispatched (the runtime registry's `render`).
    fn render(&self, which: Renderer, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        match which {
            Renderer::Value => self.render_value(dialect, out, r),
            Renderer::Entry => self.render_entry(dialect, out, r),
            Renderer::Item => self.render_item(dialect, out, r),
            Renderer::Tail => self.render_tail(dialect, out, r),
            Renderer::Key => self.render_key(dialect, out, r),
            Renderer::KeyName => self.render_key_name(dialect, out, r),
            Renderer::String => self.render_string(dialect, out, r),
        }
    }

    /// The editor's `hasRenderer`: whether this language renders `which` in
    /// `dialect` (Zig: `@hasDecl`, or a runtime language's own
    /// `hasRenderer`). By default the renderer is asked an empty request and
    /// answers by being there; a language whose renderers are costly to call
    /// overrides this.
    fn has_renderer(&self, dialect: Dialect, which: Renderer) -> bool {
        self.render(which, dialect, &mut Vec::new(), &RenderRequest::default()).is_some()
    }
}

/// The enforcement point for the `Language` contract (Zig's comptime
/// `validate`, run here at registration): every coherence rule between the
/// declarations that survives the move from a module to a trait. The `Err`
/// is the message Zig's `@compileError` gave.
pub fn validate(l: &dyn Language) -> core::result::Result<(), String> {
    let name = l.name();
    let dialects = l.dialects();
    // The format's registry rows: at least one, names unique within the
    // language, and exactly one row selects `default_dialect` and is named
    // `name`.
    if dialects.is_empty() {
        return Err(format!("Language '{name}' declares no dialects, so no format enum can name it"));
    }
    let mut default_rows = 0;
    for (i, d) in dialects.iter().enumerate() {
        if dialects[i + 1..].iter().any(|o| o.name == d.name) {
            return Err(format!("Language '{name}' declares two dialects named '{}'", d.name));
        }
        if d.dialect == l.default_dialect() {
            default_rows += 1;
            if d.name != name {
                return Err(format!(
                    "Language '{name}' selects its default_type in the dialect row named '{}', which must be named after the language",
                    d.name
                ));
            }
        }
    }
    if default_rows != 1 {
        return Err(format!(
            "Language '{name}' must have exactly one dialect row selecting its default_type (the one named after the language)"
        ));
    }
    let caps = l.caps();
    // `caps.lossless` describes what the `$fig` envelope pass may write INTO
    // this format, so it is only meaningful for a format that can be written.
    if caps.lossless.is_some() && !caps.serialize {
        return Err(format!(
            "Language '{name}' declares caps.lossless (an envelope target for serialized output) but caps.serialize = false, so it never writes the output the envelope would go into"
        ));
    }
    let types = l.type_count();
    if !caps.edit {
        // A format that says it cannot be edited must not declare editing
        // behaviour.
        for t in 0..types {
            for which in Renderer::ALL {
                if l.has_renderer(t as Dialect, which) {
                    return Err(format!(
                        "Language '{name}' declares caps.edit = false but supplies the editing renderer '{}'",
                        which.decl_name()
                    ));
                }
            }
        }
        return Ok(());
    }
    let mut any_block_seq = l.is_runtime();
    for t in 0..types {
        let t = t as Dialect;
        let s = l.syntax(t);
        // A format cannot have a same-line trailing comment marker without
        // having a comment syntax at all.
        if s.comments.trailing.is_some() && s.comments.line.is_none() {
            return Err("Language declares a trailing comment marker but no line comment marker".into());
        }
        // `kv_sep = None` says "the generic engine never writes
        // `key<sep>value` for me", which holds only when the entry is
        // spelled by a `render_entry` renderer.
        if s.kv_sep.is_none() && !l.has_renderer(t, Renderer::Entry) {
            return Err("Language declares kv_sep = null but neither hooks insertKey nor declares renderEntry, so the generic entry-insert paths have no separator to write".into());
        }
        // A flow member separator of no bytes would make every flow insert
        // weld two members together.
        if s.flow_entry_sep.is_empty() {
            return Err("Language declares an empty flow_entry_sep".into());
        }
        if s.flow_root && !s.flow_containers {
            return Err("Language declares flow_root but flow_containers = false".into());
        }
        if s.block_seq_editable {
            any_block_seq = true;
        }
    }
    // The block-sequence item renderer sits below the editor's
    // `block_seq_editable` refusal, so a format that declares no editable
    // block sequences in any dialect can never reach it.
    if !any_block_seq && (0..types).any(|t| l.has_renderer(t as Dialect, Renderer::Item)) {
        return Err(format!(
            "Language '{name}' declares block_seq_editable = false but supplies 'renderItem', which the engine refuses before reaching"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::{KeyValue, NodeKind, PathSegment};
    use crate::kit::row_writer::RowWriter;
    use crate::kit::span::Span;

    /// A one-line `k=v` language: what the trait asks of the smallest one.
    struct TinyKv;

    const TINY_DIALECTS: &[DialectInfo] = &[DialectInfo::new("tinykv", SpliceStyle::Raw, Some(b""))];

    impl Language for TinyKv {
        fn name(&self) -> &str {
            "tinykv"
        }
        fn extensions(&self) -> &[&'static str] {
            &["tkv"]
        }
        fn caps(&self) -> Caps {
            Caps { edit: true, serialize: true, ..Caps::DEFAULT }
        }
        fn dialects(&self) -> &[DialectInfo] {
            TINY_DIALECTS
        }
        fn syntax(&self, _: Dialect) -> Syntax {
            Syntax { flow_containers: false, ..Syntax::new(Comments::HASH, Some(b"="), None) }
        }
        fn parse_table(&self, _input: &[u8], _: Dialect, out: &mut Table) -> Result<()> {
            let src = out.source_arc();
            let mut w = RowWriter::init(out);
            let root = w.add(NodeKind::Mapping(None), Span::new(0, src.len()))?;
            let mut last: Option<u32> = None;
            let mut at = 0usize;
            for line in src.split(|&c| c == b'\n') {
                let start = at;
                at += line.len() + 1;
                if line.is_empty() {
                    continue;
                }
                let Some(eq) = line.iter().position(|&c| c == b'=') else {
                    add_diagnostic(w.t, table::DIAGNOSTIC_ERROR, start, None, b"expected `=`", b"here")?;
                    return Err(err!(MissingEquals));
                };
                let k = w.add(NodeKind::String(&line[..eq]), Span::new(start, start + eq))?;
                let v = w.add(NodeKind::String(&line[eq + 1..]), Span::new(start + eq + 1, start + line.len()))?;
                let kv = w.add(NodeKind::KeyValue(KeyValue { key: k, value: v }), Span::new(start, start + line.len()))?;
                match last {
                    Some(p) => w.set_next(p, Some(kv)),
                    None => w.set(root, NodeKind::Mapping(Some(kv)))?,
                }
                last = Some(kv);
            }
            w.finish(root)
        }
        fn render_key_name(&self, _: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
            out.extend_from_slice(r.key);
            Some(Ok(()))
        }
    }

    #[test]
    fn a_language_parses_through_its_table_and_reports_a_refusal() {
        let l: &dyn Language = &TinyKv;
        validate(l).unwrap();
        let doc = l.parse(b"a=1\nb=2\n", 0).unwrap();
        assert_eq!(b"2", doc.ast.get_val_by_path(&[PathSegment::Key(b"b")]).unwrap().kind.as_string().unwrap());
        // In-source texts stay in the source.
        assert!(doc.ast.strings().is_empty());
        let mut report = ParseReport::default();
        assert_eq!(err!(MissingEquals), l.parse_with_report(b"a=1\nnope\n", 0, &mut report).unwrap_err());
        assert_eq!(4, report.errors[0].offset);
        assert_eq!(b"expected `=`", &report.errors[0].message[..]);
        assert!(l.has_renderer(0, Renderer::KeyName));
        assert!(!l.has_renderer(0, Renderer::Value));
        assert!(!l.has_split_stream());
        assert_eq!(err!(FormatDisabled), l.print(0, &doc.ast, &SerializeOptions::default(), &mut Vec::new()).unwrap_err());
    }
}
