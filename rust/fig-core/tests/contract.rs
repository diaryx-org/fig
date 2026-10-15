//! Port of src/contract/contract_test.zig: every bundled language, reached
//! through the contract alone — registered under its dialects prefixed `c-`
//! (Zig's `contract.VTable(L, "c-")`; here a `Language` that forwards to the
//! compiled one), held by registration to the harness on its own samples, and
//! then read alike: the table a parse through the registry fills is the
//! table its own parse records, row for row.
//!
//! Beyond the Zig file: the same languages served over the helper wire
//! in-process (`Served`, which answers as `fig::helper::serve` would), so
//! every bundled language's description, tables, prints and renderers cross
//! `wire.rs` both ways and read back as the compiled format does.

use std::sync::Arc;

use fig_core::kit::ast::{NodeId, SerializeOptions, Spans, AST};
use fig_core::kit::convert;
use fig_core::kit::describe_json;
use fig_core::kit::document::Document;
use fig_core::kit::language::{
    Caps, Dialect, DialectInfo, Language, ParseReport, RenderRequest, Renderer, StreamDocument, Syntax,
};
use fig_core::kit::manifest::Literal;
use fig_core::kit::table::Table;
use fig_core::registry::{self, RegisterOptions, Registry};
use fig_core::wire::{self, json::Value, Transport};
use fig_core::{err, lang, Result};

/// The languages this crate bundles (`registry`'s bundle).
fn bundled() -> Vec<&'static dyn Language> {
    vec![&lang::json::LANGUAGE, &lang::toml::LANGUAGE, &lang::ini::LANGUAGE, &lang::dotenv::LANGUAGE, &lang::properties::LANGUAGE]
}

/// The table of `doc` as the wire spells it, for comparing two.
fn table_json(doc: &Document) -> String {
    let t = convert::full_table(doc).unwrap();
    let mut w = Vec::new();
    wire::table_to_json(&mut w, &t.view());
    String::from_utf8(w).unwrap()
}

// ── the contract, prefixed ─────────────────────────────────────────────────

/// A compiled language under its own declarations, every dialect row named
/// `<prefix><name>`: what `contract.VTable(L, prefix)` registers.
struct Prefixed {
    inner: &'static dyn Language,
    name: &'static str,
    dialects: &'static [DialectInfo],
}

fn prefixed(inner: &'static dyn Language, prefix: &str) -> &'static Prefixed {
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let dialects: Vec<DialectInfo> =
        inner.dialects().iter().map(|d| DialectInfo { name: leak(format!("{prefix}{}", d.name)), ..*d }).collect();
    Box::leak(Box::new(Prefixed {
        inner,
        name: leak(format!("{prefix}{}", inner.name())),
        dialects: Box::leak(dialects.into_boxed_slice()),
    }))
}

impl Language for Prefixed {
    fn name(&self) -> &str {
        self.name
    }
    fn extensions(&self) -> &[&'static str] {
        self.inner.extensions()
    }
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn dialects(&self) -> &[DialectInfo] {
        self.dialects
    }
    fn default_dialect(&self) -> Dialect {
        self.inner.default_dialect()
    }
    fn type_count(&self) -> usize {
        self.inner.type_count()
    }
    fn samples(&self) -> &[&'static [u8]] {
        self.inner.samples()
    }
    fn syntax(&self, dialect: Dialect) -> Syntax {
        self.inner.syntax(dialect)
    }
    fn parse_table(&self, input: &[u8], dialect: Dialect, out: &mut Table) -> Result<()> {
        self.inner.parse_table(input, dialect, out)
    }
    fn split_stream(&self, input: &[u8], dialect: Dialect) -> Option<Result<Vec<StreamDocument>>> {
        self.inner.split_stream(input, dialect)
    }
    fn has_split_stream(&self) -> bool {
        self.inner.has_split_stream()
    }
    fn print(&self, dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        self.inner.print(dialect, ast, options, out)
    }
    fn print_node(&self, dialect: Dialect, ast: &AST, node: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        self.inner.print_node(dialect, ast, node, options, out)
    }
    fn render(&self, which: Renderer, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        self.inner.render(which, dialect, out, r)
    }
    fn has_renderer(&self, dialect: Dialect, which: Renderer) -> bool {
        self.inner.has_renderer(dialect, which)
    }
}

#[test]
fn every_compiled_language_registers_through_the_contract_and_reads_its_samples_as_the_compiled_format_does() {
    let r = Registry::new();
    for l in bundled() {
        let p = prefixed(l, "c-");
        if let Err(e) = r.register(p) {
            panic!("{}: {}: {}", l.name(), e.name(), String::from_utf8_lossy(&registry::last_refusal()));
        }
        for d in l.dialects() {
            let e = r.entry_by_name(&format!("c-{}", d.name)).unwrap();
            for &sample in l.samples() {
                let Ok(compiled) = l.parse(sample, d.dialect) else { continue };
                let through = e.parse(sample).unwrap();
                assert_eq!(table_json(&compiled), table_json(&through), "{}: sample {:?}", d.name, String::from_utf8_lossy(sample));
            }
        }
    }
}

#[test]
fn a_languages_diagnostics_cross_the_contract_a_refusals_message_and_offset_and_the_warnings_of_a_clean_parse() {
    let r = Registry::new();
    r.register(prefixed(&lang::dotenv::LANGUAGE, "c-")).unwrap();
    let e = r.entry_by_name("c-dotenv").unwrap();

    let mut p = registry::Parser::new();
    p.parse(b"A=1\nA=2\n", &e).unwrap();
    assert_eq!(0, p.errors.len());
    assert_eq!(1, p.warnings.len());
    assert_eq!(b"duplicate key", &p.warnings[0].short_label[..]);
    assert_eq!(4, p.warnings[0].offset);

    p.warnings.clear();
    assert_eq!(err!(RuntimeParseFailed), p.parse(b"A=1\n=x\n", &e).unwrap_err());
    assert_eq!(1, p.errors.len());
    assert!(String::from_utf8_lossy(&p.errors[0].message).contains("KEY=value"));
    assert_eq!(p.errors[0].message, p.last_message());
    assert_eq!(4, p.errors[0].offset);
}

// ── the same languages, over the wire ──────────────────────────────────────

/// A compiled language answering the helper wire in-process, as a helper
/// built with `fig::helper::serve` answers it over stdio.
struct Served {
    lang: &'static dyn Language,
    /// The dialect each row the description names selects.
    rows: Vec<(String, Dialect)>,
}

fn served(lang: &'static dyn Language) -> Served {
    let mut rows: Vec<(String, Dialect)> = lang.dialects().iter().map(|d| (d.name.to_string(), d.dialect)).collect();
    for d in lang.dialects() {
        for sp in d.specs {
            rows.push((format!("{}-{}", d.name, sp.name), sp.dialect));
        }
    }
    Served { lang, rows }
}

/// A refusal's code, for a printer's: the C `Refusal` it names.
fn refusal_code(e: fig_core::Error) -> i64 {
    const NAMES: [&str; 9] = [
        "NullUnsupported",
        "NonStringKey",
        "UnrepresentableRoot",
        "UnsupportedValue",
        "InvalidKey",
        "NonFiniteNumber",
        "UnresolvedAlias",
        "NestingTooDeep",
        "UnexpectedNodeKind",
    ];
    NAMES.iter().position(|n| *n == e.name()).map_or(-32000, |i| i as i64 + 1)
}

impl Served {
    fn dialect(&self, params: &Value) -> Dialect {
        let name = params.get("dialect").and_then(|d| d.as_str()).unwrap();
        self.rows.iter().find(|(n, _)| n == name).unwrap().1
    }

    /// The member after the id: `"result":…` or `"error":…`.
    fn answer(&self, method: &str, p: &Value) -> String {
        let string = |s: &[u8]| {
            let mut w = Vec::new();
            wire::json_string(&mut w, s);
            String::from_utf8(w).unwrap()
        };
        let refuse = |code: i64, message: &[u8], offset: usize| {
            format!("\"error\":{{\"code\":{code},\"message\":{},\"data\":{{\"byte_offset\":{offset}}}}}", string(message))
        };
        match method {
            "describe" => {
                let mut w = Vec::new();
                describe_json::write(&mut w, self.lang);
                format!("\"result\":{}", String::from_utf8(w).unwrap())
            }
            "parse" => {
                let input = p.get("input").unwrap().as_str().unwrap().as_bytes();
                let mut report = ParseReport::default();
                match self.lang.parse_with_report(input, self.dialect(p), &mut report) {
                    Ok(doc) => format!("\"result\":{}", table_json(&doc)),
                    Err(e) => match report.errors.first() {
                        Some(d) => refuse(-32000, &d.message, d.offset),
                        None => refuse(-32000, e.name().as_bytes(), 0),
                    },
                }
            }
            "print" => {
                let mut t = Table::default();
                wire::table_from_json(p.get("table").unwrap(), &mut t).unwrap();
                // The wire carries each row's parent; adopting the table
                // links its children, as a helper's own reader does.
                let ast = AST::adopt(Arc::from(&b""[..]), &mut t, Spans::Absent).unwrap();
                let doc = Document { source: Arc::from(&b""[..]), ast };
                let o = p.get("options").unwrap();
                let b = |k: &str| o.get(k).and_then(|v| v.as_bool()).unwrap();
                let n = |k: &str| o.get(k).and_then(|v| v.as_i64()).unwrap();
                let options = SerializeOptions {
                    pretty: b("pretty"),
                    strip_comments: b("strip_comments"),
                    indent: n("indent") as u8,
                    width: n("width") as u16,
                    splice: b("splice"),
                    flow: b("flow"),
                    ..SerializeOptions::default()
                };
                let mut out = Vec::new();
                match self.lang.print(self.dialect(p), &doc.ast, &options, &mut out) {
                    Ok(()) => format!("\"result\":{}", string(&out)),
                    Err(e) => refuse(refusal_code(e), e.name().as_bytes(), 0),
                }
            }
            "render" => {
                let s = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").as_bytes().to_vec();
                let which = Renderer::ALL.into_iter().find(|w| w.name().as_bytes() == s("which")).unwrap();
                let (indent, key, value, old_key, parent_key, parent_tag, old_value) =
                    (s("indent"), s("key"), s("value"), s("old_key"), s("parent_key"), s("parent_tag"), s("old_value"));
                let r = RenderRequest {
                    indent: &indent,
                    key: &key,
                    value: &value,
                    literal: Literal::parse(&s("literal")).unwrap_or(Literal::String),
                    old_key: &old_key,
                    parent_key: &parent_key,
                    parent_tag: &parent_tag,
                    old_value: &old_value,
                };
                let mut out = Vec::new();
                match self.lang.render(which, self.dialect(p), &mut out, &r) {
                    Some(Ok(())) => format!("\"result\":{}", string(&out)),
                    Some(Err(e)) => refuse(-32000, e.name().as_bytes(), 0),
                    None => refuse(-32601, b"no such renderer", 0),
                }
            }
            "split_stream" => {
                let input = p.get("input").unwrap().as_str().unwrap().as_bytes();
                match self.lang.split_stream(input, self.dialect(p)) {
                    Some(Ok(docs)) => {
                        let items: Vec<String> = docs
                            .iter()
                            .map(|d| format!("{{\"start\":{},\"end\":{},\"explicit\":{}}}", d.start, d.end, d.explicit))
                            .collect();
                        format!("\"result\":[{}]", items.join(","))
                    }
                    Some(Err(e)) => refuse(-32000, e.name().as_bytes(), 0),
                    None => refuse(-32601, b"no streams", 0),
                }
            }
            _ => refuse(-32601, b"no such method", 0),
        }
    }
}

impl Transport for Served {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>> {
        let req = wire::json::parse(request).unwrap();
        let id = req.get("id").unwrap().as_i64().unwrap();
        let method = req.get("method").unwrap().as_str().unwrap();
        let member = self.answer(method, req.get("params").unwrap_or(&Value::Null));
        Ok(format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},{member}}}").into_bytes())
    }
}

#[test]
fn every_compiled_language_served_over_the_wire_reads_and_prints_its_samples_as_the_compiled_format_does() {
    for l in bundled() {
        let compiled = Registry::new();
        compiled.register(l).unwrap();
        let over = Registry::new();
        let wired = wire::describe(Box::new(served(l))).unwrap();
        assert_eq!(l.caps(), wired.caps(), "{}", l.name());
        if let Err(e) = wire::register(&over, wired, RegisterOptions::default()) {
            panic!("{}: {}: {}", l.name(), e.name(), String::from_utf8_lossy(&registry::last_refusal()));
        }
        for ce in compiled.entries() {
            let we = over.entry_by_name(&ce.name).unwrap();
            assert_eq!(ce.extensions, we.extensions, "{}", ce.name);
            assert_eq!(ce.splice, we.splice, "{}", ce.name);
            assert_eq!(ce.empty_doc_seed, we.empty_doc_seed, "{}", ce.name);
            assert_eq!(ce.prints, we.prints, "{}", ce.name);
            assert_eq!(ce.syntax_or_default(), we.syntax_or_default(), "{}", ce.name);
            for which in Renderer::ALL {
                assert_eq!(ce.has_renderer(which), we.has_renderer(which), "{} {which:?}", ce.name);
            }
            for &sample in l.samples() {
                let Ok(a) = ce.parse(sample) else { continue };
                let b = we.parse(sample).unwrap();
                assert_eq!(table_json(&a), table_json(&b), "{}: sample {:?}", ce.name, String::from_utf8_lossy(sample));
                for options in [SerializeOptions::default(), SerializeOptions { pretty: false, strip_comments: true, ..Default::default() }] {
                    let (mut x, mut y) = (Vec::new(), Vec::new());
                    let rx = registry::print_with(&ce, &a.ast, &options, &mut x);
                    let ry = registry::print_with(&we, &b.ast, &options, &mut y);
                    assert_eq!(rx, ry, "{}", ce.name);
                    assert_eq!(String::from_utf8_lossy(&x), String::from_utf8_lossy(&y), "{}", ce.name);
                }
            }
            // A refusal crosses with its words and offset.
            let mut p = registry::Parser::new();
            let mut q = registry::Parser::new();
            let bad = b"\x01=[\n";
            let (ra, rb) = (p.parse(bad, &ce), q.parse(bad, &we));
            assert_eq!(ra.is_err(), rb.is_err(), "{}", ce.name);
            if ra.is_err() {
                assert_eq!(p.last_message(), q.last_message(), "{}", ce.name);
                assert_eq!(p.last_offset, q.last_offset, "{}", ce.name);
            }
            // Every renderer answers as the compiled one does.
            let r = RenderRequest { key: b"k", value: b"v w", old_key: b"\"o\"", old_value: b"'x'", ..RenderRequest::default() };
            for which in Renderer::ALL {
                let (mut x, mut y) = (Vec::new(), Vec::new());
                let (rx, ry) = (ce.render(which, &mut x, &r), we.render(which, &mut y, &r));
                assert_eq!(rx.is_ok(), ry.is_ok(), "{} {which:?}", ce.name);
                assert_eq!(x, y, "{} {which:?}", ce.name);
            }
        }
    }
}
