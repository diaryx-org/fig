//! The out-of-process carrier: a [`Language`] served to a `fig` binary
//! over stdin and stdout. [`serve`] is the request loop a helper
//! executable runs, and [`handle`] answers one request.
//!
//! This is the git remote-helper model. The CLI spawns the helper, asks it
//! `describe` once, and then sends it a `parse`, `print` or `render` per
//! call; the helper answers each on one line. A helper is any executable
//! that speaks this; a helper written against this crate is [`serve`] over
//! an implementation of [`Language`].
//!
//! The protocol is JSON-RPC 2.0, one message per line, with fig's methods
//! and their payloads: `docs/helper-protocol.md` in the fig repository is
//! its specification, and what a helper in another language implements. A
//! language's [`LanguageError`] is answered as the error
//! [`LANGUAGE_REFUSED`], its byte offset in the error's `data`.
//!
//! The JSON is read and written with `serde_json`, so a helper needs no
//! language registered, the `json` one included; the module and the
//! dependency come with the crate's `helper` feature. Its functions trade
//! [`serde_json::Value`].

use std::io::{BufRead, Write};

use crate::language::{
    ClosedContainers, CommentDelimiter, CommentForm, CommentRow, CommentSlot, Comments,
    CommentsPrinted, Description, DiagnosticRow, Dialect, DirectiveRow, Language, LanguageError,
    Literal, MentionKind, MentionRow, NativeKinds, NodeKind, NodeRow, NodeTable, PrintOptions,
    Prints, RegionRow, RenderArgs, Renderer, Renderers, SectionHeader, Severity, Splice, Syntax,
};
use crate::{Capabilities, ExtKind, Span};
use serde_json::{Map, Value};

/// Serve `lang` on this process's stdin and stdout until stdin closes.
/// What a helper executable's `main` calls.
pub fn serve(lang: impl Language) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve_io(&lang, stdin.lock(), stdout.lock())
}

/// [`serve`] over any reader and writer — for a test, or a host that speaks
/// the wire over something other than stdio.
pub fn serve_io(
    lang: &dyn Language,
    reader: impl BufRead,
    mut writer: impl Write,
) -> std::io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = handle(lang, &line);
        writer.write_all(encode(&response).as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    Ok(())
}

/// The code of an error a language raises — a parse it refuses, a printer
/// or renderer that cannot spell what it was given. JSON-RPC leaves -32000
/// to -32099 to the server.
pub const LANGUAGE_REFUSED: i64 = -32000;

/// A JSON-RPC error object: a code JSON-RPC reserves, for a request that
/// could not be read, or [`LANGUAGE_REFUSED`] with the language's offset.
struct Protocol {
    code: i64,
    message: String,
    byte_offset: Option<usize>,
}

/// A request field `answer` could not read: invalid params. A language's own
/// refusal goes through [`refused`] instead.
impl From<LanguageError> for Protocol {
    fn from(e: LanguageError) -> Self {
        Protocol::new(-32602, e.message)
    }
}

impl Protocol {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Protocol {
            code,
            message: message.into(),
            byte_offset: None,
        }
    }
}

/// One request, as a line of JSON-RPC 2.0, to one response. Never fails: a
/// refusal, a malformed request and a request for a method the language
/// lacks alike come back as an `error` response carrying the request's id.
pub fn handle(lang: &dyn Language, request: &str) -> Value {
    let mut id = Value::Null;
    let outcome = decode(request)
        .map_err(|e| Protocol::new(-32700, e.message))
        .and_then(|req| {
            match req.get("id") {
                Some(v @ Value::Number(n)) if n.is_i64() || n.is_u64() => id = v.clone(),
                Some(v @ Value::String(_)) => id = v.clone(),
                _ => {}
            }
            if req.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                return Err(Protocol::new(
                    -32600,
                    r#"a request carries "jsonrpc":"2.0""#,
                ));
            }
            let method = req
                .get("method")
                .and_then(Value::as_str)
                .ok_or_else(|| Protocol::new(-32600, "a request names its method"))?
                .to_owned();
            let params = match req.get("params") {
                None => Value::Object(Map::new()),
                Some(p @ Value::Object(_)) => p.clone(),
                Some(_) => return Err(Protocol::new(-32602, "params is an object")),
            };
            Ok((method, params))
        });
    let answered = outcome.and_then(|(method, params)| answer(lang, &method, &params));
    let mut entries = vec![("jsonrpc", s("2.0")), ("id", id)];
    match answered {
        Ok(result) => entries.push(("result", result)),
        Err(e) => {
            let mut error = vec![
                ("code", Value::from(e.code)),
                ("message", Value::String(e.message)),
            ];
            if let Some(off) = e.byte_offset {
                error.push(("data", map(vec![("byte_offset", Value::from(off as u64))])));
            }
            entries.push(("error", map(error)));
        }
    }
    map(entries)
}

fn answer(lang: &dyn Language, method: &str, req: &Value) -> Result<Value, Protocol> {
    match method {
        "describe" => Ok(description_to_value(&lang.describe())),
        "parse" => {
            let dialect = str_field(req, "dialect")?;
            let input = str_field(req, "input")?;
            lang.parse(dialect, input.as_bytes())
                .map(|table| table_to_value(&table))
                .map_err(refused)
        }
        "print" => {
            let dialect = str_field(req, "dialect")?;
            let table = table_from_value(
                req.get("table")
                    .ok_or_else(|| Protocol::new(-32602, "print: no table"))?,
            )
            .map_err(|e| Protocol::new(-32602, e.message))?;
            let options = match req.get("options") {
                Some(o) => PrintOptions {
                    pretty: o.get("pretty").and_then(Value::as_bool).unwrap_or(true),
                    strip_comments: o
                        .get("strip_comments")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    indent: o.get("indent").and_then(Value::as_u64).unwrap_or(2) as u8,
                    width: o.get("width").and_then(Value::as_u64).unwrap_or(80) as u16,
                    splice: o.get("splice").and_then(Value::as_bool).unwrap_or(false),
                    flow: o.get("flow").and_then(Value::as_bool).unwrap_or(false),
                },
                None => PrintOptions::default(),
            };
            let out = lang.print(dialect, &table, &options).map_err(refused)?;
            let out = String::from_utf8(out).map_err(|_| {
                Protocol::new(LANGUAGE_REFUSED, "print returned bytes that are not UTF-8")
            })?;
            Ok(Value::String(out))
        }
        "split_stream" => {
            let dialect = str_field(req, "dialect")?;
            let input = str_field(req, "input")?;
            let docs = lang
                .split_stream(dialect, input.as_bytes())
                .map_err(refused)?;
            Ok(Value::Array(
                docs.iter()
                    .map(|d| {
                        map(vec![
                            ("start", Value::from(d.start as i64)),
                            ("end", Value::from(d.end as i64)),
                            ("explicit", Value::Bool(d.explicit)),
                        ])
                    })
                    .collect(),
            ))
        }
        "render" => {
            let dialect = str_field(req, "dialect")?;
            let which = match str_field(req, "which")? {
                "value" => Renderer::Value,
                "entry" => Renderer::Entry,
                "item" => Renderer::Item,
                "tail" => Renderer::Tail,
                "key" => Renderer::Key,
                "key_name" => Renderer::KeyName,
                "string" => Renderer::String,
                other => {
                    return Err(Protocol::new(
                        -32602,
                        format!("render: unknown renderer {other:?}"),
                    ));
                }
            };
            let field = |name: &str| req.get(name).and_then(Value::as_str).unwrap_or("");
            let args = RenderArgs {
                dialect,
                indent: field("indent").as_bytes(),
                key: field("key").as_bytes(),
                value: field("value").as_bytes(),
                // Absent or unknown: a string, which is what a renderer does
                // with any text it cannot type.
                literal: Literal::from_name(field("literal")).unwrap_or_default(),
                old_key: field("old_key").as_bytes(),
                parent_key: field("parent_key").as_bytes(),
                parent_tag: field("parent_tag").as_bytes(),
                old_value: field("old_value").as_bytes(),
            };
            let out = lang.render(which, args).map_err(refused)?;
            let out = String::from_utf8(out).map_err(|_| {
                Protocol::new(LANGUAGE_REFUSED, "render returned bytes that are not UTF-8")
            })?;
            Ok(Value::String(out))
        }
        other => Err(Protocol::new(-32601, format!("unknown method {other:?}"))),
    }
}

/// A language's refusal, its offset kept for the error's `data`.
fn refused(e: LanguageError) -> Protocol {
    Protocol {
        code: LANGUAGE_REFUSED,
        message: e.message,
        byte_offset: e.byte_offset,
    }
}

// ── JSON in and out ────────────────────────────────────────────────────────

/// One line of JSON as a [`Value`], read as [`handle`] reads a request:
/// [`encode`]'s counterpart, for a helper that answers a line itself.
pub fn decode(line: &str) -> Result<Value, LanguageError> {
    serde_json::from_str(line).map_err(|e| LanguageError::new(format!("request is not JSON: {e}")))
}

/// One line of compact JSON.
pub fn encode(value: &Value) -> String {
    // A `Value`'s keys are strings, so writing one cannot fail.
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}

fn str_field<'a>(v: &'a Value, name: &str) -> Result<&'a str, LanguageError> {
    v.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| LanguageError::new(format!("missing string field {name:?}")))
}

fn s(v: &str) -> Value {
    Value::String(v.to_owned())
}

fn opt_s(v: &Option<String>) -> Value {
    match v {
        Some(v) => Value::String(v.clone()),
        None => Value::Null,
    }
}

fn map(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

fn span_v(sp: Span) -> Value {
    Value::Array(vec![
        Value::from(sp.start as u64),
        Value::from(sp.end as u64),
    ])
}

fn span_of(v: &Value) -> Result<Span, LanguageError> {
    let seq = v
        .as_array()
        .ok_or_else(|| LanguageError::new("a span is a [start, end] pair"))?;
    if seq.len() != 2 {
        return Err(LanguageError::new("a span is a [start, end] pair"));
    }
    let n = |v: &Value| {
        v.as_u64()
            .ok_or_else(|| LanguageError::new("a span offset is an integer"))
    };
    Ok(Span {
        start: n(&seq[0])? as usize,
        end: n(&seq[1])? as usize,
    })
}

fn opt_span_of(v: Option<&Value>) -> Result<Option<Span>, LanguageError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(v) => span_of(v).map(Some),
    }
}

fn opt_str_of(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_owned)
}

// ── the description ────────────────────────────────────────────────────────

/// A description as the wire carries it.
pub fn description_to_value(d: &Description) -> Value {
    let mut renderers = Vec::new();
    for (on, name) in [
        (d.renderers.value, "value"),
        (d.renderers.entry, "entry"),
        (d.renderers.item, "item"),
        (d.renderers.tail, "tail"),
        (d.renderers.key, "key"),
        (d.renderers.key_name, "key_name"),
        (d.renderers.string, "string"),
    ] {
        if on {
            renderers.push(s(name));
        }
    }
    map(vec![
        ("name", s(&d.name)),
        (
            "caps",
            map(vec![
                ("read", Value::Bool(d.caps.read)),
                ("edit", Value::Bool(d.caps.edit)),
                ("serialize", Value::Bool(d.caps.serialize)),
                ("references", Value::Bool(d.caps.references)),
            ]),
        ),
        (
            "max_mapping_depth",
            d.max_mapping_depth
                .map_or(Value::Null, |n| Value::from(n as u64)),
        ),
        (
            "lossless",
            d.lossless
                .as_ref()
                .map_or(Value::Null, native_kinds_to_value),
        ),
        (
            "syntax",
            d.syntax.as_ref().map_or(Value::Null, syntax_to_value),
        ),
        (
            "dialects",
            Value::Array(
                d.dialects
                    .iter()
                    .map(|dl| {
                        map(vec![
                            ("name", s(&dl.name)),
                            (
                                "extensions",
                                Value::Array(dl.extensions.iter().map(|e| s(e)).collect()),
                            ),
                            (
                                "splice",
                                s(match dl.splice {
                                    Splice::Literal => "literal",
                                    Splice::Escaped => "escaped",
                                    Splice::Raw => "raw",
                                }),
                            ),
                            ("empty_doc_seed", opt_s(&dl.empty_doc_seed)),
                            (
                                "syntax",
                                dl.syntax.as_ref().map_or(Value::Null, syntax_to_value),
                            ),
                            (
                                "prints",
                                dl.prints.as_ref().map_or(Value::Null, prints_to_value),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "samples",
            Value::Array(d.samples.iter().map(|x| s(x)).collect()),
        ),
        ("renderers", Value::Array(renderers)),
        ("split_stream", Value::Bool(d.streams)),
    ])
}

/// A description from the wire.
pub fn description_from_value(v: &Value) -> Result<Description, LanguageError> {
    let caps = v
        .get("caps")
        .ok_or_else(|| LanguageError::new("description: no caps"))?;
    let flag = |name: &str| caps.get(name).and_then(Value::as_bool).unwrap_or(false);
    let mut dialects = Vec::new();
    for dl in v
        .get("dialects")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        dialects.push(Dialect {
            name: str_field(dl, "name")?.to_owned(),
            extensions: dl
                .get("extensions")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            splice: match dl
                .get("splice")
                .and_then(Value::as_str)
                .unwrap_or("literal")
            {
                "escaped" => Splice::Escaped,
                "raw" => Splice::Raw,
                _ => Splice::Literal,
            },
            empty_doc_seed: opt_str_of(dl.get("empty_doc_seed")),
            syntax: match dl.get("syntax") {
                None | Some(Value::Null) => None,
                Some(sv) => Some(syntax_from_value(sv)?),
            },
            prints: match dl.get("prints") {
                None | Some(Value::Null) => None,
                Some(pv) => Some(prints_from_value(pv)),
            },
        });
    }
    let mut renderers = Renderers::default();
    for r in v
        .get("renderers")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        match r.as_str() {
            Some("value") => renderers.value = true,
            Some("entry") => renderers.entry = true,
            Some("item") => renderers.item = true,
            Some("tail") => renderers.tail = true,
            Some("key") => renderers.key = true,
            Some("key_name") => renderers.key_name = true,
            Some("string") => renderers.string = true,
            _ => {}
        }
    }
    Ok(Description {
        name: str_field(v, "name")?.to_owned(),
        caps: Capabilities::new(flag("read"), flag("edit"), flag("serialize"))
            .with_references(flag("references")),
        max_mapping_depth: v
            .get("max_mapping_depth")
            .and_then(Value::as_u64)
            .map(|n| n as u8),
        lossless: match v.get("lossless") {
            None | Some(Value::Null) => None,
            Some(l) => Some(native_kinds_from_value(l)),
        },
        syntax: match v.get("syntax") {
            None | Some(Value::Null) => None,
            Some(sv) => Some(syntax_from_value(sv)?),
        },
        dialects,
        samples: v
            .get("samples")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        renderers,
        streams: v
            .get("split_stream")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn prints_to_value(p: &Prints) -> Value {
    map(vec![
        ("holds", native_kinds_to_value(&p.holds)),
        ("typed_scalars", Value::Bool(p.typed_scalars)),
        ("null_at_root", Value::Bool(p.null_at_root)),
        ("root_container", Value::Bool(p.root_container)),
        ("char_as", s(&p.char_as)),
        ("number_special_as", s(&p.number_special_as)),
        (
            "comments",
            s(match p.comments {
                CommentsPrinted::Always => "always",
                CommentsPrinted::Pretty => "pretty",
                CommentsPrinted::Never => "never",
            }),
        ),
    ])
}

fn prints_from_value(v: &Value) -> Prints {
    let b = |name: &str, default: bool| v.get(name).and_then(Value::as_bool).unwrap_or(default);
    let st = |name: &str| {
        v.get(name)
            .and_then(Value::as_str)
            .unwrap_or("string")
            .to_owned()
    };
    let mut p = Prints::new(
        v.get("holds")
            .map_or(NativeKinds::default(), native_kinds_from_value),
    );
    p.typed_scalars = b("typed_scalars", true);
    p.null_at_root = b("null_at_root", false);
    p.root_container = b("root_container", false);
    p.char_as = st("char_as");
    p.number_special_as = st("number_special_as");
    p.comments = match v.get("comments").and_then(Value::as_str) {
        Some("pretty") => CommentsPrinted::Pretty,
        Some("never") => CommentsPrinted::Never,
        _ => CommentsPrinted::Always,
    };
    p
}

fn native_kinds_to_value(k: &NativeKinds) -> Value {
    map(vec![
        ("null", Value::Bool(k.null)),
        ("offset_datetime", Value::Bool(k.offset_datetime)),
        ("local_datetime", Value::Bool(k.local_datetime)),
        ("local_date", Value::Bool(k.local_date)),
        ("local_time", Value::Bool(k.local_time)),
        ("enum_literal", Value::Bool(k.enum_literal)),
        ("char_literal", Value::Bool(k.char_literal)),
        ("number_special", Value::Bool(k.number_special)),
        ("plist_date", Value::Bool(k.plist_date)),
        ("data", Value::Bool(k.data)),
    ])
}

fn native_kinds_from_value(v: &Value) -> NativeKinds {
    let f = |name: &str| v.get(name).and_then(Value::as_bool).unwrap_or(false);
    NativeKinds {
        null: f("null"),
        offset_datetime: f("offset_datetime"),
        local_datetime: f("local_datetime"),
        local_date: f("local_date"),
        local_time: f("local_time"),
        enum_literal: f("enum_literal"),
        char_literal: f("char_literal"),
        number_special: f("number_special"),
        plist_date: f("plist_date"),
        data: f("data"),
    }
}

fn delimiter_to_value(d: &Option<CommentDelimiter>) -> Value {
    match d {
        Some(d) => map(vec![
            ("open", s(&d.open)),
            ("close", s(&d.close)),
            ("forbidden", opt_s(&d.forbidden)),
        ]),
        None => Value::Null,
    }
}

fn delimiter_from_value(v: Option<&Value>) -> Option<CommentDelimiter> {
    let v = v?;
    let open = v.get("open").and_then(Value::as_str)?;
    Some(CommentDelimiter {
        open: open.to_owned(),
        close: v
            .get("close")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        forbidden: opt_str_of(v.get("forbidden")),
    })
}

/// A syntax as the wire carries it.
pub fn syntax_to_value(x: &Syntax) -> Value {
    map(vec![
        (
            "comments",
            map(vec![
                ("line", delimiter_to_value(&x.comments.line)),
                ("trailing", delimiter_to_value(&x.comments.trailing)),
                ("block", delimiter_to_value(&x.comments.block)),
            ]),
        ),
        ("kv_sep", opt_s(&x.kv_sep)),
        (
            "flow_kv_sep_from_siblings",
            Value::Bool(x.flow_kv_sep_from_siblings),
        ),
        ("flow_map_pad", s(&x.flow_map_pad)),
        (
            "key_sigil",
            x.key_sigil.map_or(Value::Null, |b| Value::from(b as u64)),
        ),
        ("empty_map_literal", opt_s(&x.empty_map_literal)),
        ("block_seq_editable", Value::Bool(x.block_seq_editable)),
        ("flow_containers", Value::Bool(x.flow_containers)),
        ("indent_unit", s(&x.indent_unit)),
        ("seq_item_marker", s(&x.seq_item_marker)),
        (
            "closed_containers",
            x.closed_containers.as_ref().map_or(Value::Null, |c| {
                map(vec![
                    ("map_open", s(&c.map_open)),
                    ("map_close", s(&c.map_close)),
                    ("seq_open", s(&c.seq_open)),
                    ("seq_close", s(&c.seq_close)),
                ])
            }),
        ),
        (
            "single_line_block_mapping",
            Value::Bool(x.single_line_block_mapping),
        ),
        (
            "bare_document_mapping",
            Value::Bool(x.bare_document_mapping),
        ),
        ("flow_map_open", s(&x.flow_map_open)),
        ("flow_map_close", s(&x.flow_map_close)),
        ("structural_indent", Value::Bool(x.structural_indent)),
        ("section_noun", opt_s(&x.section_noun)),
        (
            "section_header",
            x.section_header.as_ref().map_or(Value::Null, |h| {
                map(vec![
                    ("open", s(&h.open)),
                    ("close", s(&h.close)),
                    ("seq_open", opt_s(&h.seq_open)),
                    ("seq_close", opt_s(&h.seq_close)),
                    ("sep", s(&h.sep)),
                    ("skip_index", Value::Bool(h.skip_index)),
                ])
            }),
        ),
        ("merge_key", opt_s(&x.merge_key)),
        ("flow_entry_sep", s(&x.flow_entry_sep)),
        ("flow_root", Value::Bool(x.flow_root)),
        ("flow_maps_only", Value::Bool(x.flow_maps_only)),
        ("line_prefix_marks", s(&x.line_prefix_marks)),
        ("flow_map_kv_sep", s(&x.flow_map_kv_sep)),
        ("flow_map_entry_sep", s(&x.flow_map_entry_sep)),
        (
            "line_continuation",
            x.line_continuation.map_or(Value::Null, Value::from),
        ),
        ("empty_seq_literal", opt_s(&x.empty_seq_literal)),
        ("opener_line_comment", Value::Bool(x.opener_line_comment)),
    ])
}

/// A syntax from the wire. An absent field takes [`Syntax::default`]'s.
pub fn syntax_from_value(v: &Value) -> Result<Syntax, LanguageError> {
    let d = Syntax::default();
    let b = |name: &str, default: bool| v.get(name).and_then(Value::as_bool).unwrap_or(default);
    let st = |name: &str, default: &str| {
        v.get(name)
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_owned()
    };
    let comments = v.get("comments");
    Ok(Syntax {
        comments: Comments {
            line: delimiter_from_value(comments.and_then(|c| c.get("line"))),
            trailing: delimiter_from_value(comments.and_then(|c| c.get("trailing"))),
            block: delimiter_from_value(comments.and_then(|c| c.get("block"))),
        },
        kv_sep: opt_str_of(v.get("kv_sep")),
        flow_kv_sep_from_siblings: b("flow_kv_sep_from_siblings", d.flow_kv_sep_from_siblings),
        flow_map_pad: st("flow_map_pad", &d.flow_map_pad),
        key_sigil: v.get("key_sigil").and_then(Value::as_u64).map(|n| n as u8),
        empty_map_literal: opt_str_of(v.get("empty_map_literal")),
        block_seq_editable: b("block_seq_editable", d.block_seq_editable),
        flow_containers: b("flow_containers", d.flow_containers),
        indent_unit: st("indent_unit", &d.indent_unit),
        seq_item_marker: st("seq_item_marker", &d.seq_item_marker),
        closed_containers: v
            .get("closed_containers")
            .filter(|c| !matches!(c, Value::Null))
            .map(|c| ClosedContainers {
                map_open: c
                    .get("map_open")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                map_close: c
                    .get("map_close")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                seq_open: c
                    .get("seq_open")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                seq_close: c
                    .get("seq_close")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            }),
        single_line_block_mapping: b("single_line_block_mapping", d.single_line_block_mapping),
        bare_document_mapping: b("bare_document_mapping", d.bare_document_mapping),
        flow_map_open: st("flow_map_open", &d.flow_map_open),
        flow_map_close: st("flow_map_close", &d.flow_map_close),
        structural_indent: b("structural_indent", d.structural_indent),
        section_noun: opt_str_of(v.get("section_noun")),
        section_header: v
            .get("section_header")
            .filter(|h| !matches!(h, Value::Null))
            .map(|h| SectionHeader {
                open: h
                    .get("open")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                close: h
                    .get("close")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                seq_open: opt_str_of(h.get("seq_open")),
                seq_close: opt_str_of(h.get("seq_close")),
                sep: h
                    .get("sep")
                    .and_then(Value::as_str)
                    .unwrap_or(".")
                    .to_owned(),
                skip_index: h.get("skip_index").and_then(Value::as_bool).unwrap_or(true),
            }),
        merge_key: opt_str_of(v.get("merge_key")),
        flow_entry_sep: st("flow_entry_sep", &d.flow_entry_sep),
        flow_root: b("flow_root", d.flow_root),
        flow_maps_only: b("flow_maps_only", d.flow_maps_only),
        line_prefix_marks: st("line_prefix_marks", &d.line_prefix_marks),
        flow_map_kv_sep: st("flow_map_kv_sep", &d.flow_map_kv_sep),
        flow_map_entry_sep: st("flow_map_entry_sep", &d.flow_map_entry_sep),
        line_continuation: v
            .get("line_continuation")
            .and_then(Value::as_u64)
            .filter(|&n| n > 0 && n < 256)
            .map(|n| n as u8),
        empty_seq_literal: opt_str_of(v.get("empty_seq_literal")),
        opener_line_comment: b("opener_line_comment", d.opener_line_comment),
    })
}

// ── the table ──────────────────────────────────────────────────────────────

fn kind_name(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Null => "null",
        NodeKind::Bool => "bool",
        NodeKind::Int => "int",
        NodeKind::Float => "float",
        NodeKind::String => "string",
        NodeKind::Sequence => "sequence",
        NodeKind::Mapping => "mapping",
        NodeKind::KeyValue => "keyvalue",
        NodeKind::Alias => "alias",
    }
}

fn kind_of(name: &str) -> Option<NodeKind> {
    Some(match name {
        "null" => NodeKind::Null,
        "bool" => NodeKind::Bool,
        "int" => NodeKind::Int,
        "float" => NodeKind::Float,
        "string" => NodeKind::String,
        "sequence" => NodeKind::Sequence,
        "mapping" => NodeKind::Mapping,
        "keyvalue" => NodeKind::KeyValue,
        "alias" => NodeKind::Alias,
        _ => return None,
    })
}

fn ext_kind_name(k: ExtKind) -> &'static str {
    match k {
        ExtKind::OffsetDateTime => "offset_datetime",
        ExtKind::LocalDateTime => "local_datetime",
        ExtKind::LocalDate => "local_date",
        ExtKind::LocalTime => "local_time",
        ExtKind::EnumLiteral => "enum_literal",
        ExtKind::CharLiteral => "char_literal",
        ExtKind::NumberSpecial => "number_special",
        ExtKind::PlistDate => "plist_date",
        ExtKind::Data => "data",
    }
}

fn ext_kind_of(name: &str) -> Option<ExtKind> {
    Some(match name {
        "offset_datetime" => ExtKind::OffsetDateTime,
        "local_datetime" => ExtKind::LocalDateTime,
        "local_date" => ExtKind::LocalDate,
        "local_time" => ExtKind::LocalTime,
        "enum_literal" => ExtKind::EnumLiteral,
        "char_literal" => ExtKind::CharLiteral,
        "number_special" => ExtKind::NumberSpecial,
        "plist_date" => ExtKind::PlistDate,
        "data" => ExtKind::Data,
        _ => return None,
    })
}

/// A table as the wire carries it.
pub fn table_to_value(t: &NodeTable) -> Value {
    let rows = t
        .rows
        .iter()
        .map(|r| {
            let mut e = vec![("kind", s(kind_name(r.kind)))];
            if let Some(k) = r.ext_kind {
                e.push(("ext_kind", s(ext_kind_name(k))));
            }
            e.push((
                "parent",
                r.parent.map_or(Value::Null, |p| Value::from(p as u64)),
            ));
            if let Some(sp) = r.span {
                e.push(("span", span_v(sp)));
            }
            if let Some(t) = &r.text {
                e.push(("text", Value::String(t.clone())));
            }
            if let Some(a) = &r.anchor {
                e.push(("anchor", Value::String(a.clone())));
            }
            if let Some(sp) = r.anchor_span {
                e.push(("anchor_span", span_v(sp)));
            }
            if let Some(t) = &r.tag {
                e.push(("tag", Value::String(t.clone())));
            }
            if let Some(sp) = r.tag_span {
                e.push(("tag_span", span_v(sp)));
            }
            if let Some(sp) = r.marker {
                e.push(("marker", span_v(sp)));
            }
            if let Some(sp) = r.sep {
                e.push(("sep", span_v(sp)));
            }
            if let Some(t) = &r.item_sep {
                e.push(("item_sep", Value::String(t.clone())));
            }
            if r.open {
                e.push(("open", Value::Bool(true)));
            }
            map(e)
        })
        .collect();
    let regions = t
        .regions
        .iter()
        .map(|r| {
            map(vec![
                ("node", Value::from(r.node as u64)),
                ("start", Value::from(r.span.start as u64)),
                ("end", Value::from(r.span.end as u64)),
            ])
        })
        .collect();
    let mentions = t
        .mentions
        .iter()
        .map(|m| {
            map(vec![
                ("node", Value::from(m.node as u64)),
                ("span", span_v(m.span)),
                (
                    "kind",
                    s(match m.kind {
                        MentionKind::Header => "header",
                        MentionKind::Entry => "entry",
                        MentionKind::Index => "index",
                        MentionKind::Append => "append",
                    }),
                ),
            ])
        })
        .collect();
    let comments = t
        .comments
        .iter()
        .map(|c| {
            map(vec![
                ("node", Value::from(c.node as u64)),
                (
                    "slot",
                    s(match c.slot {
                        CommentSlot::Leading => "leading",
                        CommentSlot::Trailing => "trailing",
                        CommentSlot::Dangling => "dangling",
                    }),
                ),
                (
                    "style",
                    s(match c.style {
                        CommentForm::Line => "line",
                        CommentForm::Block => "block",
                    }),
                ),
                ("text", Value::String(c.text.clone())),
            ])
        })
        .collect();
    let mut fields = vec![
        ("rows", Value::Array(rows)),
        ("regions", Value::Array(regions)),
        ("mentions", Value::Array(mentions)),
        ("comments", Value::Array(comments)),
    ];
    if !t.directives.is_empty() {
        let directives = t
            .directives
            .iter()
            .map(|d| {
                map(vec![
                    ("handle", Value::String(d.handle.clone())),
                    ("prefix", Value::String(d.prefix.clone())),
                ])
            })
            .collect();
        fields.push(("directives", Value::Array(directives)));
    }
    if !t.diagnostics.is_empty() {
        let diagnostics = t
            .diagnostics
            .iter()
            .map(|d| {
                map(vec![
                    (
                        "severity",
                        s(match d.severity {
                            Severity::Error => "error",
                            Severity::Warning => "warning",
                        }),
                    ),
                    ("span", span_v(d.span)),
                    ("message", Value::String(d.message.clone())),
                    ("label", Value::String(d.label.clone())),
                ])
            })
            .collect();
        fields.push(("diagnostics", Value::Array(diagnostics)));
    }
    map(fields)
}

/// A table from the wire.
pub fn table_from_value(v: &Value) -> Result<NodeTable, LanguageError> {
    let mut t = NodeTable::new();
    for r in v
        .get("rows")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        let kind = kind_of(str_field(r, "kind")?)
            .ok_or_else(|| LanguageError::new("row: unknown kind"))?;
        let parent = r.get("parent").and_then(Value::as_u64).map(|p| p as u32);
        let mut row = NodeRow {
            kind,
            ext_kind: None,
            parent,
            span: None,
            text: None,
            anchor: None,
            anchor_span: None,
            tag: None,
            tag_span: None,
            marker: None,
            sep: None,
            item_sep: None,
            open: false,
        };
        row.ext_kind = r
            .get("ext_kind")
            .and_then(Value::as_str)
            .and_then(ext_kind_of);
        row.span = opt_span_of(r.get("span"))?;
        row.text = opt_str_of(r.get("text"));
        row.anchor = opt_str_of(r.get("anchor"));
        row.anchor_span = opt_span_of(r.get("anchor_span"))?;
        row.tag = opt_str_of(r.get("tag"));
        row.tag_span = opt_span_of(r.get("tag_span"))?;
        row.marker = opt_span_of(r.get("marker"))?;
        row.sep = opt_span_of(r.get("sep"))?;
        row.item_sep = opt_str_of(r.get("item_sep"));
        row.open = r.get("open").and_then(Value::as_bool).unwrap_or(false);
        t.rows.push(row);
    }
    let n = |v: &Value, name: &str| -> Result<u64, LanguageError> {
        v.get(name)
            .and_then(Value::as_u64)
            .ok_or_else(|| LanguageError::new(format!("missing integer field {name:?}")))
    };
    for r in v
        .get("regions")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        t.regions.push(RegionRow {
            node: n(r, "node")? as u32,
            // The wire keeps a region's offsets flat, as it always has.
            span: Span {
                start: n(r, "start")? as usize,
                end: n(r, "end")? as usize,
            },
        });
    }
    for m in v
        .get("mentions")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        t.mentions.push(MentionRow {
            node: n(m, "node")? as u32,
            span: span_of(
                m.get("span")
                    .ok_or_else(|| LanguageError::new("mention: no span"))?,
            )?,
            kind: match m.get("kind").and_then(Value::as_str) {
                Some("entry") => MentionKind::Entry,
                Some("index") => MentionKind::Index,
                Some("append") => MentionKind::Append,
                _ => MentionKind::Header,
            },
        });
    }
    for c in v
        .get("comments")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        t.comments.push(CommentRow {
            node: n(c, "node")? as u32,
            slot: match c.get("slot").and_then(Value::as_str) {
                Some("trailing") => CommentSlot::Trailing,
                Some("dangling") => CommentSlot::Dangling,
                _ => CommentSlot::Leading,
            },
            style: if c.get("style").and_then(Value::as_str) == Some("block") {
                CommentForm::Block
            } else {
                CommentForm::Line
            },
            text: str_field(c, "text")?.to_owned(),
        });
    }
    for d in v
        .get("directives")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        t.directives.push(DirectiveRow {
            handle: str_field(d, "handle")?.to_owned(),
            prefix: str_field(d, "prefix")?.to_owned(),
        });
    }
    for d in v
        .get("diagnostics")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        t.diagnostics.push(DiagnosticRow {
            severity: if d.get("severity").and_then(Value::as_str) == Some("warning") {
                Severity::Warning
            } else {
                Severity::Error
            },
            span: opt_span_of(d.get("span"))?
                .ok_or_else(|| LanguageError::new("a diagnostic has no span"))?,
            message: str_field(d, "message")?.to_owned(),
            label: d
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        });
    }
    Ok(t)
}
