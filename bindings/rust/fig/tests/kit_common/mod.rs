//! What the kit's tests share: a node table as the helper wire spells one,
//! the form `kit/testdata/` and `fig lang table` write.

#![allow(dead_code)]

use fig::language::{CommentForm, CommentSlot, MentionKind, NodeKind, NodeTable};
use fig::{ExtKind, Span};
use serde_json::{Map, Value, json};

pub const EXT_NAMES: [(&str, ExtKind); 4] = [
    ("offset_datetime", ExtKind::OffsetDateTime),
    ("local_datetime", ExtKind::LocalDateTime),
    ("local_date", ExtKind::LocalDate),
    ("local_time", ExtKind::LocalTime),
];

pub fn ext_name(k: ExtKind) -> &'static str {
    EXT_NAMES
        .iter()
        .find(|(_, e)| *e == k)
        .map(|(n, _)| *n)
        .unwrap_or("other")
}

pub fn span_json(s: Span) -> Value {
    json!([s.start, s.end])
}

/// `t` as the helper wire spells a table.
pub fn wire(t: &NodeTable) -> Value {
    let rows: Vec<Value> = t
        .rows
        .iter()
        .map(|r| {
            let mut o = Map::new();
            let kind = match r.kind {
                NodeKind::Null => "null",
                NodeKind::Bool => "bool",
                NodeKind::Int => "int",
                NodeKind::Float => "float",
                NodeKind::String => "string",
                NodeKind::Sequence => "sequence",
                NodeKind::Mapping => "mapping",
                NodeKind::KeyValue => "keyvalue",
                NodeKind::Alias => "alias",
                _ => "unknown",
            };
            o.insert("kind".into(), json!(kind));
            if let Some(e) = r.ext_kind {
                o.insert("ext_kind".into(), json!(ext_name(e)));
            }
            o.insert("parent".into(), json!(r.parent));
            if let Some(s) = r.span {
                o.insert("span".into(), span_json(s));
            }
            if let Some(t) = &r.text {
                o.insert("text".into(), json!(t));
            }
            if let Some(a) = &r.anchor {
                o.insert("anchor".into(), json!(a));
            }
            if let Some(s) = r.anchor_span {
                o.insert("anchor_span".into(), span_json(s));
            }
            if let Some(a) = &r.tag {
                o.insert("tag".into(), json!(a));
            }
            if let Some(s) = r.tag_span {
                o.insert("tag_span".into(), span_json(s));
            }
            if let Some(s) = r.marker {
                o.insert("marker".into(), span_json(s));
            }
            if let Some(s) = r.sep {
                o.insert("sep".into(), span_json(s));
            }
            if let Some(a) = &r.item_sep {
                o.insert("item_sep".into(), json!(a));
            }
            if r.open {
                o.insert("open".into(), json!(true));
            }
            Value::Object(o)
        })
        .collect();
    let regions: Vec<Value> = t
        .regions
        .iter()
        .map(|r| json!({"node": r.node, "start": r.span.start, "end": r.span.end}))
        .collect();
    let mentions: Vec<Value> = t
        .mentions
        .iter()
        .map(|m| {
            let kind = match m.kind {
                MentionKind::Header => "header",
                MentionKind::Entry => "entry",
                MentionKind::Index => "index",
                _ => "append",
            };
            json!({"node": m.node, "span": span_json(m.span), "kind": kind})
        })
        .collect();
    let comments: Vec<Value> = t
        .comments
        .iter()
        .map(|c| {
            let slot = match c.slot {
                CommentSlot::Leading => "leading",
                CommentSlot::Trailing => "trailing",
                CommentSlot::Dangling => "dangling",
            };
            let style = if c.style == CommentForm::Block {
                "block"
            } else {
                "line"
            };
            json!({"node": c.node, "slot": slot, "style": style, "text": c.text})
        })
        .collect();
    json!({"rows": rows, "regions": regions, "mentions": mentions, "comments": comments})
}

