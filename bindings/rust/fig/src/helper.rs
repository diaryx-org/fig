//! The out-of-process carrier: a [`Language`] served to a `fig` binary as
//! JSON over stdin and stdout. [`serve`] is the request loop a helper
//! executable runs; the wire format below is what the `fig` CLI's helper
//! runner speaks, and what a helper in any other language implements.
//!
//! This is the git remote-helper model. The CLI spawns the helper, asks it
//! `describe` once, and then sends it a `parse`, `print` or `render` per
//! call; the helper answers each on one line. A helper is any executable
//! that speaks this; a helper written against this crate is [`serve`] over
//! an implementation of [`Language`].
//!
//! # The wire
//!
//! Newline-delimited JSON, one request per line on stdin, one response per
//! line on stdout, in order. Every string is UTF-8 — a document a helper is
//! asked to parse arrives as a JSON string, so a helper never sees bytes
//! that are not text.
//!
//! Requests:
//!
//! ```json
//! {"op":"describe"}
//! {"op":"parse","dialect":"hcl","input":"…"}
//! {"op":"print","dialect":"hcl","table":{…},"options":{"pretty":true,"strip_comments":false,"indent":2,"width":80}}
//! {"op":"render","which":"value","dialect":"hcl","indent":"","key":"","value":"…","old_key":""}
//! ```
//!
//! Responses:
//!
//! ```json
//! {"ok":true,"description":{…}}
//! {"ok":true,"table":{…}}
//! {"ok":true,"output":"…"}
//! {"ok":false,"message":"…","byte_offset":12}
//! ```
//!
//! A description is `{"name","caps":{"read","edit","serialize"},
//! "max_mapping_depth":n|null,"lossless":{…}|null,"syntax":{…}|null,
//! "dialects":[{"name","extensions":[…],"splice":"literal|json_string|raw",
//! "empty_doc_seed":"…"|null,"syntax":{…}|null}],"samples":[…],
//! "renderers":["value",…]}`; a syntax is the fields of
//! [`Syntax`] by name, with enums as their lower-case names and absent
//! optionals as `null`. A table is `{"rows":[…],"regions":[…],"mentions":[…],
//! "comments":[…]}`: a row is `{"kind":"mapping","parent":0,"span":[s,e],
//! "text":"…"}` plus `ext_kind`, `anchor`, `anchor_span`, `tag`,
//! `tag_span`, `marker` and `sep` where present (an absent optional is
//! omitted, not `null`); a region `{"node","start","end"}`; a mention
//! `{"node","span":[s,e],"kind":"header|entry"}`; a comment `{"node",
//! "slot":"leading|trailing|dangling","style":"line|block","text"}`.
//!
//! The JSON is read and written by fig itself; this module needs no JSON
//! library and adds none.

use std::io::{BufRead, Write};

use crate::language::{
    ClosedContainers, CommentDelimiter, CommentForm, CommentRow, CommentSlot, CommentStyle,
    Comments, Description, Dialect, KeyStyle, Language, LanguageError, MentionKind, MentionRow,
    NativeKinds, NodeKind, NodeRow, NodeTable, PrintOptions, RegionRow, RenderArgs, Renderer,
    Renderers, SectionHeader, SectionNoun, Splice, Syntax,
};
use crate::{Capabilities, Document, ExtKind, Format, SerializeOptions, Span, Value};

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

/// One request, as a line of JSON, to one response.
pub fn handle(lang: &dyn Language, request: &str) -> Value {
    match handle_inner(lang, request) {
        Ok(v) => v,
        Err(e) => failure(&e),
    }
}

fn handle_inner(lang: &dyn Language, request: &str) -> Result<Value, LanguageError> {
    let req = decode(request)?;
    let op = str_field(&req, "op")?;
    match op {
        "describe" => Ok(ok(vec![(
            "description",
            description_to_value(&lang.describe()),
        )])),
        "parse" => {
            let dialect = str_field(&req, "dialect")?;
            let input = str_field(&req, "input")?;
            let table = lang.parse(dialect, input.as_bytes())?;
            Ok(ok(vec![("table", table_to_value(&table))]))
        }
        "print" => {
            let dialect = str_field(&req, "dialect")?;
            let table = table_from_value(
                req.get("table")
                    .ok_or_else(|| LanguageError::new("print: no table"))?,
            )?;
            let options = match req.get("options") {
                Some(o) => PrintOptions {
                    pretty: o.get("pretty").and_then(Value::as_bool).unwrap_or(true),
                    strip_comments: o
                        .get("strip_comments")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    indent: o.get("indent").and_then(Value::as_u64).unwrap_or(2) as u8,
                    width: o.get("width").and_then(Value::as_u64).unwrap_or(80) as u16,
                },
                None => PrintOptions::default(),
            };
            let out = lang.print(dialect, &table, &options)?;
            let out = String::from_utf8(out)
                .map_err(|_| LanguageError::new("print returned bytes that are not UTF-8"))?;
            Ok(ok(vec![("output", Value::Str(out))]))
        }
        "render" => {
            let dialect = str_field(&req, "dialect")?;
            let which = match str_field(&req, "which")? {
                "value" => Renderer::Value,
                "entry" => Renderer::Entry,
                "item" => Renderer::Item,
                "tail" => Renderer::Tail,
                "key" => Renderer::Key,
                other => {
                    return Err(LanguageError::new(format!(
                        "render: unknown renderer {other:?}"
                    )));
                }
            };
            let field = |name: &str| req.get(name).and_then(Value::as_str).unwrap_or("");
            let args = RenderArgs {
                dialect,
                indent: field("indent").as_bytes(),
                key: field("key").as_bytes(),
                value: field("value").as_bytes(),
                old_key: field("old_key").as_bytes(),
            };
            let out = lang.render(which, args)?;
            let out = String::from_utf8(out)
                .map_err(|_| LanguageError::new("render returned bytes that are not UTF-8"))?;
            Ok(ok(vec![("output", Value::Str(out))]))
        }
        other => Err(LanguageError::new(format!("unknown op {other:?}"))),
    }
}

// ── JSON in and out, through fig ───────────────────────────────────────────

fn decode(line: &str) -> Result<Value, LanguageError> {
    let doc = Document::parse(line.as_bytes(), Format::Json)
        .map_err(|e| LanguageError::new(format!("request is not JSON: {e}")))?;
    doc.to_value()
        .map_err(|e| LanguageError::new(format!("request is not JSON: {e}")))
}

/// One line of compact JSON.
pub fn encode(value: &Value) -> String {
    let opts = SerializeOptions {
        pretty: false,
        ..Default::default()
    };
    let mut s = value
        .serialize_with(Format::Json, opts)
        .unwrap_or_else(|_| "null".to_owned());
    while s.ends_with('\n') {
        s.pop();
    }
    s
}

fn ok(fields: Vec<(&str, Value)>) -> Value {
    let mut entries = vec![(Value::Str("ok".into()), Value::Bool(true))];
    for (k, v) in fields {
        entries.push((Value::Str(k.into()), v));
    }
    Value::Map(entries)
}

fn failure(e: &LanguageError) -> Value {
    let mut entries = vec![
        (Value::Str("ok".into()), Value::Bool(false)),
        (Value::Str("message".into()), Value::Str(e.message.clone())),
    ];
    if let Some(off) = e.byte_offset {
        entries.push((Value::Str("byte_offset".into()), Value::Uint(off as u64)));
    }
    Value::Map(entries)
}

fn str_field<'a>(v: &'a Value, name: &str) -> Result<&'a str, LanguageError> {
    v.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| LanguageError::new(format!("missing string field {name:?}")))
}

fn s(v: &str) -> Value {
    Value::Str(v.to_owned())
}

fn opt_s(v: &Option<String>) -> Value {
    match v {
        Some(v) => Value::Str(v.clone()),
        None => Value::Null,
    }
}

fn map(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(k, v)| (Value::Str(k.to_owned()), v))
            .collect(),
    )
}

fn span_v(sp: Span) -> Value {
    Value::Seq(vec![
        Value::Uint(sp.start as u64),
        Value::Uint(sp.end as u64),
    ])
}

fn span_of(v: &Value) -> Result<Span, LanguageError> {
    let seq = v
        .as_seq()
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
            ]),
        ),
        (
            "max_mapping_depth",
            d.max_mapping_depth
                .map_or(Value::Null, |n| Value::Uint(n as u64)),
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
            Value::Seq(
                d.dialects
                    .iter()
                    .map(|dl| {
                        map(vec![
                            ("name", s(&dl.name)),
                            (
                                "extensions",
                                Value::Seq(dl.extensions.iter().map(|e| s(e)).collect()),
                            ),
                            (
                                "splice",
                                s(match dl.splice {
                                    Splice::Literal => "literal",
                                    Splice::JsonString => "json_string",
                                    Splice::Raw => "raw",
                                }),
                            ),
                            ("empty_doc_seed", opt_s(&dl.empty_doc_seed)),
                            (
                                "syntax",
                                dl.syntax.as_ref().map_or(Value::Null, syntax_to_value),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "samples",
            Value::Seq(d.samples.iter().map(|x| s(x)).collect()),
        ),
        ("renderers", Value::Seq(renderers)),
    ])
}

/// A description from the wire.
pub fn description_from_value(v: &Value) -> Result<Description, LanguageError> {
    let caps = v
        .get("caps")
        .ok_or_else(|| LanguageError::new("description: no caps"))?;
    let flag = |name: &str| caps.get(name).and_then(Value::as_bool).unwrap_or(false);
    let mut dialects = Vec::new();
    for dl in v.get("dialects").and_then(Value::as_seq).unwrap_or(&[]) {
        dialects.push(Dialect {
            name: str_field(dl, "name")?.to_owned(),
            extensions: dl
                .get("extensions")
                .and_then(Value::as_seq)
                .unwrap_or(&[])
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            splice: match dl
                .get("splice")
                .and_then(Value::as_str)
                .unwrap_or("literal")
            {
                "json_string" => Splice::JsonString,
                "raw" => Splice::Raw,
                _ => Splice::Literal,
            },
            empty_doc_seed: opt_str_of(dl.get("empty_doc_seed")),
            syntax: match dl.get("syntax") {
                None | Some(Value::Null) => None,
                Some(sv) => Some(syntax_from_value(sv)?),
            },
        });
    }
    let mut renderers = Renderers::default();
    for r in v.get("renderers").and_then(Value::as_seq).unwrap_or(&[]) {
        match r.as_str() {
            Some("value") => renderers.value = true,
            Some("entry") => renderers.entry = true,
            Some("item") => renderers.item = true,
            Some("tail") => renderers.tail = true,
            Some("key") => renderers.key = true,
            _ => {}
        }
    }
    Ok(Description {
        name: str_field(v, "name")?.to_owned(),
        caps: Capabilities::new(flag("read"), flag("edit"), flag("serialize")),
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
            .and_then(Value::as_seq)
            .unwrap_or(&[])
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        renderers,
    })
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
        ("plist_data", Value::Bool(k.plist_data)),
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
        plist_data: f("plist_data"),
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
                (
                    "style",
                    s(match x.comments.style {
                        CommentStyle::Hash => "hash",
                        CommentStyle::Slashes => "slashes",
                        CommentStyle::Semicolon => "semicolon",
                        CommentStyle::XmlComment => "xml_comment",
                    }),
                ),
                ("line", delimiter_to_value(&x.comments.line)),
                ("trailing", delimiter_to_value(&x.comments.trailing)),
            ]),
        ),
        ("kv_sep", opt_s(&x.kv_sep)),
        (
            "flow_kv_sep_from_siblings",
            Value::Bool(x.flow_kv_sep_from_siblings),
        ),
        ("flow_map_pad", s(&x.flow_map_pad)),
        (
            "key_style",
            s(match x.key_style {
                KeyStyle::Verbatim => "verbatim",
                KeyStyle::JsonQuoted => "json_quoted",
                KeyStyle::ZonField => "zon_field",
                KeyStyle::BareOrQuoted => "bare_or_quoted",
            }),
        ),
        (
            "key_sigil",
            x.key_sigil.map_or(Value::Null, |b| Value::Uint(b as u64)),
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
        (
            "section_noun",
            match x.section_noun {
                None => Value::Null,
                Some(SectionNoun::Table) => s("table"),
                Some(SectionNoun::Section) => s("section"),
                Some(SectionNoun::Container) => s("container"),
            },
        ),
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
            style: match comments
                .and_then(|c| c.get("style"))
                .and_then(Value::as_str)
                .unwrap_or("hash")
            {
                "slashes" => CommentStyle::Slashes,
                "semicolon" => CommentStyle::Semicolon,
                "xml_comment" => CommentStyle::XmlComment,
                _ => CommentStyle::Hash,
            },
            line: delimiter_from_value(comments.and_then(|c| c.get("line"))),
            trailing: delimiter_from_value(comments.and_then(|c| c.get("trailing"))),
        },
        kv_sep: opt_str_of(v.get("kv_sep")),
        flow_kv_sep_from_siblings: b("flow_kv_sep_from_siblings", d.flow_kv_sep_from_siblings),
        flow_map_pad: st("flow_map_pad", &d.flow_map_pad),
        key_style: match v
            .get("key_style")
            .and_then(Value::as_str)
            .unwrap_or("verbatim")
        {
            "json_quoted" => KeyStyle::JsonQuoted,
            "zon_field" => KeyStyle::ZonField,
            "bare_or_quoted" => KeyStyle::BareOrQuoted,
            _ => KeyStyle::Verbatim,
        },
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
        section_noun: match v.get("section_noun").and_then(Value::as_str) {
            Some("table") => Some(SectionNoun::Table),
            Some("section") => Some(SectionNoun::Section),
            Some("container") => Some(SectionNoun::Container),
            _ => None,
        },
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
        ExtKind::PlistData => "plist_data",
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
        "plist_data" => ExtKind::PlistData,
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
                r.parent.map_or(Value::Null, |p| Value::Uint(p as u64)),
            ));
            if let Some(sp) = r.span {
                e.push(("span", span_v(sp)));
            }
            if let Some(t) = &r.text {
                e.push(("text", Value::Str(t.clone())));
            }
            if let Some(a) = &r.anchor {
                e.push(("anchor", Value::Str(a.clone())));
            }
            if let Some(sp) = r.anchor_span {
                e.push(("anchor_span", span_v(sp)));
            }
            if let Some(t) = &r.tag {
                e.push(("tag", Value::Str(t.clone())));
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
            map(e)
        })
        .collect();
    let regions = t
        .regions
        .iter()
        .map(|r| {
            map(vec![
                ("node", Value::Uint(r.node as u64)),
                ("start", Value::Uint(r.start as u64)),
                ("end", Value::Uint(r.end as u64)),
            ])
        })
        .collect();
    let mentions = t
        .mentions
        .iter()
        .map(|m| {
            map(vec![
                ("node", Value::Uint(m.node as u64)),
                ("span", span_v(m.span)),
                (
                    "kind",
                    s(match m.kind {
                        MentionKind::Header => "header",
                        MentionKind::Entry => "entry",
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
                ("node", Value::Uint(c.node as u64)),
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
                ("text", Value::Str(c.text.clone())),
            ])
        })
        .collect();
    map(vec![
        ("rows", Value::Seq(rows)),
        ("regions", Value::Seq(regions)),
        ("mentions", Value::Seq(mentions)),
        ("comments", Value::Seq(comments)),
    ])
}

/// A table from the wire.
pub fn table_from_value(v: &Value) -> Result<NodeTable, LanguageError> {
    let mut t = NodeTable::new();
    for r in v.get("rows").and_then(Value::as_seq).unwrap_or(&[]) {
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
        t.rows.push(row);
    }
    let n = |v: &Value, name: &str| -> Result<u64, LanguageError> {
        v.get(name)
            .and_then(Value::as_u64)
            .ok_or_else(|| LanguageError::new(format!("missing integer field {name:?}")))
    };
    for r in v.get("regions").and_then(Value::as_seq).unwrap_or(&[]) {
        t.regions.push(RegionRow {
            node: n(r, "node")? as u32,
            start: n(r, "start")? as usize,
            end: n(r, "end")? as usize,
        });
    }
    for m in v.get("mentions").and_then(Value::as_seq).unwrap_or(&[]) {
        t.mentions.push(MentionRow {
            node: n(m, "node")? as u32,
            span: span_of(
                m.get("span")
                    .ok_or_else(|| LanguageError::new("mention: no span"))?,
            )?,
            kind: if m.get("kind").and_then(Value::as_str) == Some("entry") {
                MentionKind::Entry
            } else {
                MentionKind::Header
            },
        });
    }
    for c in v.get("comments").and_then(Value::as_seq).unwrap_or(&[]) {
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
    Ok(t)
}
