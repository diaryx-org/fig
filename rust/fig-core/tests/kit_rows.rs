//! Port of src/contract/kit_rows_test.zig: the kit's row writer —
//! `RowWriter`, `ChildIndex` and `flat_map` — held to the fixtures every
//! binding's kit is held to (`kit/testdata/rows/`, whose README names the
//! ops): rows written in any order, linked, put by a duplicate policy, and
//! finished into the pre-order table each file gives, compared as the wire
//! spells it.
//!
//! The tests read the fixtures from the repository at run time. The Zig
//! file's second half drives the same fixtures through the C API's
//! `fig_table_*` functions, which the Rust core does not export; it is not
//! ported.

use std::collections::HashMap;
use std::sync::Arc;

use fig_core::kit::ast::{Comment, ExtKind, NodeKind, Number, NumberKind, Style, Tag};
use fig_core::kit::convert;
use fig_core::kit::document::MentionKind;
use fig_core::kit::flat_map::{self, DuplicatePolicy, NodeArena, PutResult};
use fig_core::kit::row_writer::RowWriter;
use fig_core::kit::span::Span;
use fig_core::kit::table::{self, Table};
use fig_core::wire::{self, json::Value};

fn rows_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kit/testdata/rows")
}

/// Every fixture in `rows_dir`, each run by `run`.
fn each_fixture(run: fn(&str, &Value)) {
    let mut n = 0;
    let mut names: Vec<_> = std::fs::read_dir(rows_dir()).unwrap().map(|e| e.unwrap().path()).collect();
    names.sort();
    for path in names {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let fx = wire::json::parse(&bytes).unwrap();
        run(&path.display().to_string(), &fx);
        n += 1;
    }
    assert!(n > 0);
}

/// Whether two JSON values are the same value, objects compared by key.
fn json_eql(x: &Value, y: &Value) -> bool {
    match (x, y) {
        (Value::Object(o), Value::Object(p)) => {
            o.len() == p.len() && o.entries.iter().all(|(k, v)| p.get(k).is_some_and(|w| json_eql(v, w)))
        }
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(p, q)| json_eql(p, q)),
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Integer(a), Value::Integer(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

/// `want`, the fixture's table, against `t` as fig adopts it and the wire
/// spells it: the rows `finish` wrote, and each side fact on its row, in
/// node order — which is what a parse's table means, whatever order its
/// lists were left in.
fn expect_table(name: &str, want: &Value, t: &mut Table) {
    let source = t.source_arc();
    let doc = RowWriter::adopt(source, t).unwrap();
    let full = convert::full_table(&doc).unwrap();
    let mut w = Vec::new();
    wire::table_to_json(&mut w, &full.view());
    let got = wire::json::parse(&w).unwrap();
    assert!(json_eql(want, &got), "{name}\nwant {want:?}\ngot  {}", String::from_utf8_lossy(&w));
}

fn span_of(v: &Value) -> Span {
    let a = v.as_array().unwrap();
    Span::new(a[0].as_i64().unwrap() as usize, a[1].as_i64().unwrap() as usize)
}

fn id_of(v: &Value) -> Option<u32> {
    v.as_i64().map(|i| i as u32)
}

fn slot_of(s: &str) -> u8 {
    match s {
        "trailing" => table::COMMENT_TRAILING,
        "dangling" => table::COMMENT_DANGLING,
        _ => table::COMMENT_LEADING,
    }
}

fn mention_of(s: &str) -> MentionKind {
    match s {
        "header" => MentionKind::Header,
        "entry" => MentionKind::Entry,
        "index" => MentionKind::Index,
        "append" => MentionKind::Append,
        other => panic!("mention kind {other}"),
    }
}

/// A row's kind as the fixture names it, with its text.
fn kind_of<'a>(kind: &str, extra: Option<&'a Value>) -> NodeKind<'a> {
    let text: &[u8] = extra.and_then(|x| x.get("text")).and_then(|t| t.as_str()).unwrap_or("").as_bytes();
    match kind {
        "null" => NodeKind::Null,
        "bool" => NodeKind::Boolean(text == b"true"),
        "int" => NodeKind::Number(Number { raw: text, kind: NumberKind::Integer }),
        "float" => NodeKind::Number(Number { raw: text, kind: NumberKind::Float }),
        "alias" => NodeKind::Alias(text),
        "sequence" => NodeKind::Sequence(None),
        "mapping" => NodeKind::Mapping(None),
        _ => {
            if let Some(ek) = extra.and_then(|x| x.get("ext_kind")).and_then(|e| e.as_str()) {
                let kind = ExtKind::ALL.into_iter().find(|k| k.name() == ek).unwrap();
                return NodeKind::Extended(fig_core::kit::ast::Extended { kind, text });
            }
            NodeKind::String(text)
        }
    }
}

fn policy_of(s: &str) -> DuplicatePolicy {
    match s {
        "last_value" => DuplicatePolicy::Overwrite,
        "error" => DuplicatePolicy::Err,
        "first_value" => DuplicatePolicy::FirstValue,
        "keep" => DuplicatePolicy::Keep,
        other => panic!("policy {other}"),
    }
}

/// The writer under test: the rows of the table, an index of each
/// container's last child and each mapping's entries, and the parts of
/// keyvalues appended so far — a keyvalue is written whole by `RowWriter`,
/// so one built a child at a time is held here until it has both.
struct Rust<'t> {
    arena: NodeArena<'t>,
    pending_key: HashMap<u32, u32>,
}

impl Rust<'_> {
    fn append(&mut self, parent: u32, child: u32) {
        let w = &mut self.arena.w;
        if w.t.rows[parent as usize].kind == table::Kind::KEYVALUE {
            if let Some(k) = self.pending_key.remove(&parent) {
                w.set(parent, NodeKind::KeyValue(fig_core::kit::ast::KeyValue { key: k, value: child })).unwrap();
            } else {
                self.pending_key.insert(parent, child);
            }
            return;
        }
        self.arena.index.append(w, parent, child).unwrap();
    }
}

fn run_rust(name: &str, fx: &Value) {
    let source: Arc<[u8]> = Arc::from(fx.get("source").unwrap().as_str().unwrap().as_bytes());
    let mut t = Table::init(source);
    let mut puts: Vec<Value> = Vec::new();
    {
        let mut z = Rust { arena: NodeArena::init(&mut t), pending_key: HashMap::new() };
        for op_v in fx.get("ops").unwrap().as_array().unwrap() {
            let op = op_v.as_array().unwrap();
            match op[0].as_str().unwrap() {
                "add" => {
                    let extra = op.get(4);
                    let span = span_of(&op[3]);
                    let w = &mut z.arena.w;
                    let id = if op[1].as_str() == Some("keyvalue") {
                        // A keyvalue with no key and value yet: its row, typed.
                        let id = w.add(NodeKind::Null, span).unwrap();
                        w.t.rows[id as usize].kind = table::Kind::KEYVALUE;
                        id
                    } else {
                        w.add(kind_of(op[1].as_str().unwrap(), extra), span).unwrap()
                    };
                    if let Some(x) = extra {
                        if let Some(s) = x.get("marker") {
                            w.marker(id, span_of(s)).unwrap();
                        }
                        if let Some(s) = x.get("sep") {
                            w.sep(id, span_of(s)).unwrap();
                        }
                        if let Some(n) = x.get("anchor") {
                            w.anchor(id, n.as_str().unwrap().as_bytes(), x.get("anchor_span").map(span_of)).unwrap();
                        }
                        if let Some(n) = x.get("tag") {
                            let span = x.get("tag_span").map(span_of);
                            w.tag(id, Tag::Text(n.as_str().unwrap().as_bytes()), table::TAG_UNSTATED, true, span).unwrap();
                        }
                        if let Some(s) = x.get("item_sep") {
                            w.item_sep(id, s.as_str().unwrap().as_bytes()).unwrap();
                        }
                        if x.get("open").and_then(|o| o.as_bool()) == Some(true) {
                            w.set_open(id);
                        }
                    }
                    if let Some(parent) = id_of(&op[2]) {
                        z.append(parent, id);
                    }
                }
                "append" => z.append(id_of(&op[1]).unwrap(), id_of(&op[2]).unwrap()),
                "first" => {
                    let id = id_of(&op[1]).unwrap();
                    let w = &mut z.arena.w;
                    let kind = if w.t.rows[id as usize].kind == table::Kind::SEQUENCE {
                        NodeKind::Sequence(id_of(&op[2]))
                    } else {
                        NodeKind::Mapping(id_of(&op[2]))
                    };
                    w.set(id, kind).unwrap();
                }
                "next" => z.arena.w.set_next(id_of(&op[1]).unwrap(), id_of(&op[2])),
                "put" => {
                    let policy = policy_of(op[4].as_str().unwrap());
                    match flat_map::put_entry(&mut z.arena, id_of(&op[1]).unwrap(), id_of(&op[2]).unwrap(), id_of(&op[3]).unwrap(), policy) {
                        Ok(put) => puts.push(Value::Array(vec![Value::Integer(put.kv as i64), Value::Bool(put.result != PutResult::Added)])),
                        Err(e) if e.is("DuplicateKey") => puts.push(Value::String("error".into())),
                        Err(e) => panic!("{name}: {e:?}"),
                    }
                }
                "child_by_key" => {
                    let got = flat_map::lookup_child(&z.arena, id_of(&op[1]).unwrap(), op[2].as_str().unwrap().as_bytes());
                    assert_eq!(id_of(&op[3]), got, "{name}: child_by_key");
                }
                "comment" => {
                    let style = if op[3].as_str() == Some("block") { Style::Block } else { Style::Line };
                    let c = Comment { text: op[4].as_str().unwrap().as_bytes(), style };
                    z.arena.w.comment(id_of(&op[1]).unwrap(), slot_of(op[2].as_str().unwrap()), c).unwrap();
                }
                "region" => z
                    .arena
                    .w
                    .region(id_of(&op[1]).unwrap(), op[2].as_i64().unwrap() as usize, op[3].as_i64().unwrap() as usize)
                    .unwrap(),
                "mention" => z.arena.w.mention(id_of(&op[1]).unwrap(), span_of(&op[2]), mention_of(op[3].as_str().unwrap())).unwrap(),
                "finish" => z.arena.w.finish(id_of(&op[1]).unwrap()).unwrap(),
                other => panic!("{name}: unknown op {other}"),
            }
        }
    }
    if let Some(want) = fx.get("puts") {
        assert!(json_eql(want, &Value::Array(puts.clone())), "{name}: puts {puts:?}");
    }
    expect_table(name, fx.get("table").unwrap(), &mut t);
}

#[test]
fn the_rust_kits_row_writer_gives_every_rows_fixtures_table() {
    each_fixture(run_rust);
}
