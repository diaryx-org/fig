//! `fig::kit` held to the fixtures every binding's kit is held to
//! (`kit/testdata/` in the repository): the rules, the XML tokenizer, and
//! the row writer, whose finished table is compared as the helper wire
//! spells one.

mod kit_common;

use std::path::PathBuf;

use kit_common::{EXT_NAMES, ext_name, wire};

use fig::kit::rows::{DuplicatePolicy, RowWriter};
use fig::kit::{datetime, json_string, literal, number, xml};
use fig::language::{CommentForm, CommentSlot, MentionKind, NodeKind, NodeRow};
use fig::Span;
use serde_json::{Value, json};

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../kit/testdata")
}

fn fixture(path: &str) -> Value {
    let p = testdata().join(path);
    serde_json::from_str(
        &std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
    )
    .unwrap()
}

fn cases<'a>(fx: &'a Value, name: &str) -> &'a Vec<Value> {
    fx[name].as_array().unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn spelling(names: &Value) -> number::Spelling {
    let mut sp = number::Spelling::NONE;
    for n in names.as_array().unwrap() {
        match s(n) {
            "hex" => sp.hex = true,
            "octal" => sp.octal = true,
            "binary" => sp.binary = true,
            "underscores" => sp.underscores = true,
            "leading_zero" => sp.leading_zero = true,
            "bare_dot" => sp.bare_dot = true,
            "plus" => sp.plus = true,
            other => panic!("unknown spelling {other}"),
        }
    }
    sp
}

fn options(o: &Value) -> datetime::Options {
    let mut opts = datetime::Options::default();
    opts.minute_precision = o["minute_precision"].as_bool().unwrap();
    opts.time_only = o["time_only"].as_bool().unwrap();
    opts
}

#[test]
fn rules_number() {
    let fx = fixture("rules/number.json");
    for c in cases(&fx, "spellable") {
        assert_eq!(
            number::spellable(s(&c[0]), spelling(&c[1])),
            c[2].as_bool().unwrap(),
            "spellable {}",
            c[0]
        );
    }
    for c in cases(&fx, "canonical") {
        assert_eq!(number::canonical(s(&c[0])), s(&c[1]), "canonical {}", c[0]);
    }
    for c in cases(&fx, "non_finite") {
        let got = number::non_finite(s(&c[0])).map(|n| match n {
            number::NonFinite::Inf => "inf",
            number::NonFinite::NegInf => "-inf",
            number::NonFinite::NaN => "nan",
        });
        assert_eq!(got, c[1].as_str(), "non_finite {}", c[0]);
    }
}

#[test]
fn rules_datetime() {
    let fx = fixture("rules/datetime.json");
    for c in cases(&fx, "classify") {
        assert_eq!(
            datetime::classify(s(&c[0]), options(&c[1])).map(ext_name),
            c[2].as_str(),
            "classify {}",
            c[0]
        );
    }
    for c in cases(&fx, "valid_date") {
        assert_eq!(
            datetime::valid_date(s(&c[0])),
            c[1].as_bool().unwrap(),
            "valid_date {}",
            c[0]
        );
    }
    for c in cases(&fx, "valid_time") {
        assert_eq!(
            datetime::valid_time(s(&c[0]), options(&c[1])),
            c[2].as_bool().unwrap(),
            "valid_time {}",
            c[0]
        );
    }
    for c in cases(&fx, "valid_offset") {
        assert_eq!(
            datetime::valid_offset(s(&c[0])),
            c[1].as_bool().unwrap(),
            "valid_offset {}",
            c[0]
        );
    }
}

fn kind_name(raw: &str, k: NodeKind) -> &'static str {
    match k {
        NodeKind::Null => "null",
        NodeKind::Bool if raw == "true" => "true",
        NodeKind::Bool => "false",
        NodeKind::Int => "int",
        NodeKind::Float => "float",
        _ => "string",
    }
}

#[test]
fn rules_literal() {
    let fx = fixture("rules/literal.json");
    for c in cases(&fx, "kind") {
        let raw = s(&c[0]);
        let got = match literal::kind(raw) {
            (_, Some(ext)) => ext_name(ext),
            (k, None) => kind_name(raw, k),
        };
        assert_eq!(got, s(&c[1]), "kind {raw}");
    }
    for c in cases(&fx, "number") {
        let raw = s(&c[0]);
        assert_eq!(
            literal::number(raw).map(|k| kind_name(raw, k)),
            c[1].as_str(),
            "number {raw}"
        );
    }
}

#[test]
fn rules_json_string() {
    let fx = fixture("rules/json-string.json");
    for c in cases(&fx, "quote") {
        let q = if s(&c[1]) == "'" {
            json_string::Quote::Single
        } else {
            json_string::Quote::Double
        };
        assert_eq!(
            json_string::quote(s(&c[0]), q),
            s(&c[2]),
            "quote {:?}",
            c[0]
        );
    }
}

#[test]
fn xml_tokens() {
    let fx = fixture("xml/tokens.json");
    let errors = [
        (xml::ErrorKind::UnclosedTag, "unclosed_tag"),
        (
            xml::ErrorKind::UnclosedAttributeValue,
            "unclosed_attribute_value",
        ),
        (xml::ErrorKind::UnclosedComment, "unclosed_comment"),
        (xml::ErrorKind::UnclosedCData, "unclosed_cdata"),
        (xml::ErrorKind::UnclosedPi, "unclosed_pi"),
        (xml::ErrorKind::UnsupportedDoctype, "unsupported_doctype"),
        (xml::ErrorKind::InvalidMarkup, "invalid_markup"),
        (xml::ErrorKind::UnexpectedCharacter, "unexpected_character"),
    ];
    let kinds = [
        (xml::TokenKind::Lt, "lt"),
        (xml::TokenKind::LtSlash, "lt_slash"),
        (xml::TokenKind::Gt, "gt"),
        (xml::TokenKind::SlashGt, "slash_gt"),
        (xml::TokenKind::Eq, "eq"),
        (xml::TokenKind::Name, "name"),
        (xml::TokenKind::AttrValue, "attr_value"),
        (xml::TokenKind::CharData, "char_data"),
        (xml::TokenKind::CData, "cdata"),
        (xml::TokenKind::Eof, "eof"),
    ];
    for c in cases(&fx, "cases") {
        let input = s(&c["input"]);
        match xml::tokenize(input.as_bytes()) {
            Ok(tokens) => {
                let got: Vec<Value> = tokens
                    .iter()
                    .map(|t| {
                        json!([
                            kinds.iter().find(|k| k.0 == t.kind).unwrap().1,
                            t.span.start,
                            t.span.end
                        ])
                    })
                    .collect();
                assert_eq!(Value::Array(got), c["tokens"], "{input}");
            }
            Err(e) => {
                assert_eq!(
                    errors.iter().find(|k| k.0 == e.kind).unwrap().1,
                    s(&c["error"]),
                    "{input}"
                );
                assert_eq!(e.at as u64, c["at"].as_u64().unwrap(), "{input}");
            }
        }
    }
}

// ── rows ────────────────────────────────────────────────────────────────────

fn span(v: &Value) -> Span {
    Span::from(v[0].as_u64().unwrap() as usize..v[1].as_u64().unwrap() as usize)
}

fn id(v: &Value) -> Option<u32> {
    v.as_u64().map(|n| n as u32)
}

fn node_kind(name: &str) -> NodeKind {
    match name {
        "null" => NodeKind::Null,
        "bool" => NodeKind::Bool,
        "int" => NodeKind::Int,
        "float" => NodeKind::Float,
        "sequence" => NodeKind::Sequence,
        "mapping" => NodeKind::Mapping,
        "keyvalue" => NodeKind::KeyValue,
        "alias" => NodeKind::Alias,
        _ => NodeKind::String,
    }
}

fn run_rows(name: &str, fx: &Value) {
    let mut w = RowWriter::new();
    let mut puts = Vec::new();
    let mut finished = None;
    for op in fx["ops"].as_array().unwrap() {
        let op = op.as_array().unwrap();
        match s(&op[0]) {
            "add" => {
                let mut row = NodeRow::new(node_kind(s(&op[1])), id(&op[2]), span(&op[3]));
                if let Some(x) = op.get(4).and_then(Value::as_object) {
                    row.text = x.get("text").and_then(Value::as_str).map(str::to_owned);
                    row.ext_kind = x
                        .get("ext_kind")
                        .map(|e| EXT_NAMES.iter().find(|n| n.0 == s(e)).unwrap().1);
                    row.marker = x.get("marker").map(span);
                    row.sep = x.get("sep").map(span);
                    row.anchor = x.get("anchor").and_then(Value::as_str).map(str::to_owned);
                    row.anchor_span = x.get("anchor_span").map(span);
                    row.tag = x.get("tag").and_then(Value::as_str).map(str::to_owned);
                    row.tag_span = x.get("tag_span").map(span);
                    row.item_sep = x.get("item_sep").and_then(Value::as_str).map(str::to_owned);
                    row.open = x.get("open").and_then(Value::as_bool).unwrap_or(false);
                }
                w.add(row);
            }
            "append" => w.append(id(&op[1]).unwrap(), id(&op[2]).unwrap()),
            "first" => w.set_first(id(&op[1]).unwrap(), id(&op[2])),
            "next" => w.set_next(id(&op[1]).unwrap(), id(&op[2])),
            "put" => {
                let policy = match s(&op[4]) {
                    "last_value" => DuplicatePolicy::LastValue,
                    "first_value" => DuplicatePolicy::FirstValue,
                    "keep" => DuplicatePolicy::Keep,
                    _ => DuplicatePolicy::Error,
                };
                match w.put(
                    id(&op[1]).unwrap(),
                    id(&op[2]).unwrap(),
                    id(&op[3]).unwrap(),
                    policy,
                ) {
                    Ok(p) => puts.push(json!([p.entry, p.repeat])),
                    Err(_) => puts.push(json!("error")),
                }
            }
            "child_by_key" => assert_eq!(
                w.child_by_key(id(&op[1]).unwrap(), s(&op[2])),
                id(&op[3]),
                "{name}: child_by_key {}",
                op[2]
            ),
            "comment" => {
                let slot = match s(&op[2]) {
                    "trailing" => CommentSlot::Trailing,
                    "dangling" => CommentSlot::Dangling,
                    _ => CommentSlot::Leading,
                };
                let style = if s(&op[3]) == "block" {
                    CommentForm::Block
                } else {
                    CommentForm::Line
                };
                w.comment(id(&op[1]).unwrap(), slot, style, s(&op[4]));
            }
            "region" => {
                let (a, b) = (
                    op[2].as_u64().unwrap() as usize,
                    op[3].as_u64().unwrap() as usize,
                );
                w.region(id(&op[1]).unwrap(), Span::from(a..b));
            }
            "mention" => {
                let kind = match s(&op[3]) {
                    "header" => MentionKind::Header,
                    "entry" => MentionKind::Entry,
                    "index" => MentionKind::Index,
                    _ => MentionKind::Append,
                };
                w.mention(id(&op[1]).unwrap(), span(&op[2]), kind);
            }
            "finish" => {
                finished = Some(std::mem::take(&mut w).finish(id(&op[1]).unwrap()));
            }
            other => panic!("{name}: unknown op {other}"),
        }
    }
    if let Some(want) = fx.get("puts") {
        assert_eq!(&Value::Array(puts), want, "{name}: puts");
    }
    assert_eq!(
        wire(&finished.expect("a fixture finishes")),
        fx["table"],
        "{name}"
    );
}

#[test]
fn rows() {
    let dir = testdata().join("rows");
    let mut n = 0;
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    for p in names {
        if p.extension().is_some_and(|e| e == "json") {
            let fx: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            run_rows(&p.file_name().unwrap().to_string_lossy(), &fx);
            n += 1;
        }
    }
    assert!(n > 0);
}
