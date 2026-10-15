//! fig-conformance's adapter (its `docs/adapters.md`) over fig-core: a
//! command speaking JSON lines on stdin and stdout, which answers `hello`,
//! `parse`, `print`, `split` and `edit` exactly as fig's TypeScript binding
//! does when fig-conformance scores fig in-process (`lib/impl/fig.mjs`,
//! `lib/edits.mjs`), and as fig's own `tools/edit-fixtures.mjs` drives it.
//!
//! - `parse` reads the document node by node (`readTree`): a number keeps
//!   its lexeme, an alias stays an alias.
//! - `print` serializes the parsed document to `to` with the default
//!   options (`Document.serialize`).
//! - `split` is `fig_stream_split`'s byte ranges.
//! - `edit` opens an editor (`Editor.open`; a refusal there carries
//!   `"at": "open"`) and calls the op as the binding's `Editor` method of
//!   that name does (`edit-ops.ts`, `editor.ts`): a value is converted as
//!   `fromJS` converts it (an integer is an int, any other number a float,
//!   an object's keys in JavaScript's order), rendered through fig's
//!   serializer in the document's format as the splice paths render it
//!   (flow, splice, one trailing newline stripped), and a string goes to
//!   the `*_string` exports beside its rendering. Indices cross as a wasm32
//!   `usize`, so `-1` is the largest index there is.

mod json;

use std::io::{BufRead, Write};

use fig_core::api::{self, Document, Editor, FigNodeKind, FigSerializeOptions, Status, Value, NODE_NONE};
use fig_core::kit::ast::{NodeId, PathSegment};
use fig_core::registry;
use json::Json;

fn main() {
    // fig's ten languages, from their crates, registered before anything
    // looks a format up.
    let _ = registry::register_bundle(
        registry::global(),
        &[
            &fig_json::LANGUAGE,
            &fig_yaml::LANGUAGE,
            &fig_toml::LANGUAGE,
            &fig_zon::LANGUAGE,
            &fig_figl::LANGUAGE,
            &fig_ini::LANGUAGE,
            &fig_dotenv::LANGUAGE,
            &fig_properties::LANGUAGE,
            &fig_plist::LANGUAGE,
            &fig_nestedtext::LANGUAGE,
        ],
    );
    // Deep documents recurse deep; give the reader room.
    let worker = std::thread::Builder::new().stack_size(1 << 30).spawn(serve);
    match worker {
        Ok(h) => {
            let _ = h.join();
        }
        Err(e) => eprintln!("fig-conform: {e}"),
    }
}

fn serve() {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut line = Vec::new();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let answer = match json::parse(&line) {
            Ok(req) => answer(&req),
            Err(e) => {
                eprintln!("fig-conform: a request that is not JSON: {e}");
                continue;
            }
        };
        let mut out = stdout.lock();
        let _ = out.write_all(&answer);
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
}

/// One response line: `{"id": …, …fields}`.
fn reply(req: &Json, fields: Vec<(&str, Json)>) -> Vec<u8> {
    let mut members = vec![(b"id".to_vec(), req.get("id").cloned().unwrap_or(Json::Null))];
    for (k, v) in fields {
        members.push((k.as_bytes().to_vec(), v));
    }
    let mut out = Vec::new();
    json::write(&mut out, &Json::Obj(members));
    out
}

fn ok_with(req: &Json, key: &str, v: Json) -> Vec<u8> {
    reply(req, vec![("ok", Json::Bool(true)), (key, v)])
}

fn error(req: &Json, why: &str) -> Vec<u8> {
    reply(req, vec![("ok", Json::Bool(false)), ("error", Json::Str(why.as_bytes().to_vec()))])
}

fn answer(req: &Json) -> Vec<u8> {
    let method = req.get("method").and_then(Json::as_str).unwrap_or(b"");
    match method {
        b"hello" => hello(req),
        b"parse" => parse(req),
        b"print" => print(req),
        b"split" => split(req),
        b"edit" => edit(req),
        _ => error(req, &format!("unknown method {}", String::from_utf8_lossy(method))),
    }
}

fn hello(req: &Json) -> Vec<u8> {
    let mut dialects = Vec::new();
    for e in registry::global().entries() {
        if e.caps().read {
            dialects.push(Json::Str(e.name.as_bytes().to_vec()));
        }
    }
    let methods = ["parse", "print", "split", "edit"].iter().map(|m| Json::Str(m.as_bytes().to_vec())).collect();
    reply(
        req,
        vec![
            ("name", Json::Str(format!("fig-core (Rust) {}", api::version_string()).into_bytes())),
            ("dialects", Json::Arr(dialects)),
            ("methods", Json::Arr(methods)),
        ],
    )
}

/// The request's document: `source` as UTF-8, or `source_base64`'s bytes.
fn source_of(req: &Json) -> Vec<u8> {
    if let Some(s) = req.get("source").and_then(Json::as_str) {
        return s.to_vec();
    }
    req.get("source_base64").and_then(Json::as_str).and_then(json::base64_decode).unwrap_or_default()
}

fn format_of(req: &Json, key: &str) -> i32 {
    let name = req.get(key).and_then(Json::as_str).unwrap_or(b"");
    // `formatByName` of an unknown name is null, which crosses as 0: never
    // a format.
    let f = api::format_by_name(&String::from_utf8_lossy(name));
    if f < 0 { 0 } else { f }
}

/// A refusal's message as the in-process runner keeps it: the first line,
/// at most 200 characters.
fn message(e: &api::FigError) -> String {
    let text = if e.message.is_empty() {
        format!("fig_parse failed: {}", e.code.name())
    } else {
        String::from_utf8_lossy(&e.message).into_owned()
    };
    text.lines().next().unwrap_or("").chars().take(200).collect()
}

// ── parse ───────────────────────────────────────────────────────────────────

fn parse(req: &Json) -> Vec<u8> {
    let doc = match Document::parse_ex(&source_of(req), format_of(req, "dialect")) {
        Ok(d) => d,
        Err(e) => return error(req, &message(&e)),
    };
    let root = doc.root();
    let tree = if root == NODE_NONE { tagged("null", vec![]) } else { read_tree(&doc, root) };
    ok_with(req, "tree", tree)
}

fn tagged(kind: &str, mut rest: Vec<(&str, Json)>) -> Json {
    let mut members = vec![(b"kind".to_vec(), Json::Str(kind.as_bytes().to_vec()))];
    for (k, v) in rest.drain(..) {
        members.push((k.as_bytes().to_vec(), v));
    }
    Json::Obj(members)
}

fn text(bytes: Option<&[u8]>) -> Json {
    match bytes {
        Some(b) => Json::Str(String::from_utf8_lossy(b).into_owned().into_bytes()),
        None => Json::Null,
    }
}

/// fig's `ExtKind`, by ordinal, as the tree names it.
const EXT_NAMES: [&str; 9] = ["datetime", "datetime-local", "date-local", "time-local", "enum", "char", "number-special", "date", "data"];

/// `readTree`: the document as the protocol's tree.
fn read_tree(doc: &Document, id: NodeId) -> Json {
    match doc.node_kind(id) {
        FigNodeKind::Null => tagged("null", vec![]),
        FigNodeKind::Bool => tagged("bool", vec![("value", doc.node_bool(id).map_or(Json::Null, Json::Bool))]),
        FigNodeKind::Int => tagged("int", vec![("raw", text(doc.node_number(id)))]),
        FigNodeKind::Float => tagged("float", vec![("raw", text(doc.node_number(id)))]),
        FigNodeKind::String => tagged("string", vec![("value", text(doc.node_string(id)))]),
        FigNodeKind::Extended => match doc.node_extended(id) {
            Some((ext, t)) => {
                let o = ext.ordinal() as usize;
                let name = EXT_NAMES.get(o).map_or(format!("fig-{o}"), |n| n.to_string());
                tagged("ext", vec![("ext", Json::Str(name.into_bytes())), ("text", text(Some(t)))])
            }
            None => tagged("ext", vec![]),
        },
        FigNodeKind::Alias => tagged("alias", vec![]),
        FigNodeKind::Sequence => {
            let mut items = Vec::new();
            let mut c = doc.first_child(id);
            while c != NODE_NONE {
                items.push(read_tree(doc, c));
                c = doc.next_sibling(c);
            }
            tagged("seq", vec![("items", Json::Arr(items))])
        }
        FigNodeKind::Mapping => {
            let mut entries = Vec::new();
            let mut kv = doc.first_child(id);
            while kv != NODE_NONE {
                let k = doc.keyvalue_key(kv);
                let v = doc.keyvalue_value(kv);
                let key = if k == NODE_NONE { tagged("node kind -1", vec![]) } else { read_tree(doc, k) };
                let value = if v == NODE_NONE { tagged("null", vec![]) } else { read_tree(doc, v) };
                entries.push(Json::Arr(vec![key, value]));
                kv = doc.next_sibling(kv);
            }
            tagged("map", vec![("entries", Json::Arr(entries))])
        }
        k => tagged(&format!("node kind {}", k as i32), vec![]),
    }
}

// ── print, split ────────────────────────────────────────────────────────────

fn print(req: &Json) -> Vec<u8> {
    let mut doc = match Document::parse_ex(&source_of(req), format_of(req, "dialect")) {
        Ok(d) => d,
        Err(e) => return error(req, &message(&e)),
    };
    let to = format_of(req, "to");
    match doc.serialize(to, Some(&FigSerializeOptions::default())) {
        Ok(bytes) => ok_with(req, "text", text(Some(bytes))),
        Err(s) => error(req, &format!("fig_document_serialize failed: {}", s.name())),
    }
}

fn split(req: &Json) -> Vec<u8> {
    match api::stream_split(&source_of(req), format_of(req, "dialect")) {
        Ok(docs) => {
            let parts = docs.iter().map(|d| Json::Arr(vec![Json::Num(d.start as f64), Json::Num(d.end as f64)])).collect();
            ok_with(req, "parts", Json::Arr(parts))
        }
        Err(e) => error(req, &message(&e)),
    }
}

// ── edit ────────────────────────────────────────────────────────────────────

fn refusal(req: &Json, status: Status, at: Option<&str>) -> Vec<u8> {
    let mut fields = vec![("ok", Json::Bool(false)), ("status", Json::Str(status.name().as_bytes().to_vec()))];
    if let Some(at) = at {
        fields.push(("at", Json::Str(at.as_bytes().to_vec())));
    }
    reply(req, fields)
}

fn edit(req: &Json) -> Vec<u8> {
    let source = req.get("source").and_then(Json::as_str).map(<[u8]>::to_vec).unwrap_or_else(|| source_of(req));
    let format = format_of(req, "dialect");
    let mut ed = match Editor::create(&source, format) {
        Ok(e) => e,
        Err(s) => return refusal(req, s, Some("open")),
    };
    let op = String::from_utf8_lossy(req.get("op").and_then(Json::as_str).unwrap_or(b"")).into_owned();
    let empty = Vec::new();
    let args = match req.get("args") {
        Some(Json::Arr(a)) => a,
        _ => &empty,
    };
    match run(&mut ed, format, &op, args) {
        Ok(()) => {
            let out = String::from_utf8_lossy(ed.source()).into_owned().into_bytes();
            reply(req, vec![("ok", Json::Bool(true)), ("source", Json::Str(out))])
        }
        Err(Refused::Status(s)) => refusal(req, s, None),
        Err(Refused::Threw(why)) => reply(
            req,
            vec![("ok", Json::Bool(false)), ("status", Json::Str(format!("threw {why}").into_bytes())), ("error", Json::Str(why.into_bytes()))],
        ),
    }
}

/// Why an op did not land: a `FigError`'s status, or something the binding
/// would have thrown before reaching fig (a `TypeError`).
enum Refused {
    Status(Status),
    Threw(String),
}

impl From<Status> for Refused {
    fn from(s: Status) -> Refused {
        Refused::Status(s)
    }
}

type Op = Result<(), Refused>;

fn arg(args: &[Json], i: usize) -> &Json {
    args.get(i).unwrap_or(&Json::Null)
}

/// A JavaScript number as a wasm32 `usize` (`DataView.setUint32`):
/// ToUint32, with 2³² − 1 the largest index there is.
fn index(v: &Json) -> Result<usize, Refused> {
    let Json::Num(f) = v else { return Err(Refused::Threw("TypeError".into())) };
    let u = if f.is_finite() { f.trunc().rem_euclid(4_294_967_296.0) as u32 } else { 0 };
    Ok(if u == u32::MAX { usize::MAX } else { u as usize })
}

/// A path as the binding encodes it (`encodePath`). An empty key crosses
/// as a null pointer, which the C ABI refuses (`InvalidArgument`).
fn path(v: &Json) -> Result<Vec<PathSegment<'_>>, Refused> {
    let Json::Arr(segs) = v else { return Err(Refused::Threw("TypeError".into())) };
    let mut out = Vec::with_capacity(segs.len());
    for s in segs {
        out.push(match s {
            Json::Str(k) if k.is_empty() => return Err(Refused::Status(Status::InvalidArgument)),
            Json::Str(k) => PathSegment::Key(k),
            Json::Num(_) => PathSegment::Index(index(s)?),
            _ => return Err(Refused::Status(Status::InvalidArgument)),
        });
    }
    Ok(out)
}

/// A string argument (`frame.str`), coerced as JavaScript coerces one.
fn string(v: &Json) -> Vec<u8> {
    match v {
        Json::Str(s) => s.clone(),
        Json::Num(n) => json::js_number_string(*n).into_bytes(),
        Json::Bool(b) => b.to_string().into_bytes(),
        Json::Null => b"null".to_vec(),
        _ => Vec::new(),
    }
}

fn strings(v: &Json) -> Result<Vec<Vec<u8>>, Refused> {
    match v {
        Json::Arr(items) => Ok(items.iter().map(string).collect()),
        _ => Err(Refused::Threw("TypeError".into())),
    }
}

fn indices(v: &Json) -> Result<Vec<usize>, Refused> {
    match v {
        Json::Arr(items) => items.iter().map(index).collect(),
        _ => Err(Refused::Threw("TypeError".into())),
    }
}

/// `fromJS` + `build`: the value `v` as a node of `value`.
fn build(value: &mut Value, v: &Json) -> Result<NodeId, Status> {
    match v {
        Json::Null => value.null(),
        Json::Bool(b) => value.bool(*b),
        Json::Num(f) => {
            if f.is_finite() && f.fract() == 0.0 {
                // An integer: `BigInt(f)`, an i64 when it fits.
                if *f >= -9_223_372_036_854_775_808.0 && *f < 9_223_372_036_854_775_808.0 {
                    value.int(*f as i64)
                } else {
                    value.number(json::integer_decimal(*f).as_bytes(), false)
                }
            } else {
                value.number(format_float(*f).as_bytes(), true)
            }
        }
        Json::Str(s) => value.string(s),
        Json::Arr(items) => {
            let mut ids = Vec::with_capacity(items.len());
            for x in items {
                ids.push(build(value, x)?);
            }
            value.seq(&ids)
        }
        Json::Obj(members) => {
            let mut entries = Vec::with_capacity(members.len());
            for (k, x) in members {
                let key = value.string(k)?;
                let val = build(value, x)?;
                entries.push(api::FigKeyValue { key, value: val });
            }
            value.map(&entries)
        }
    }
}

/// `formatFloat`: `.nan`/`.inf`, and always a fractional part or an
/// exponent so a float stays a float.
fn format_float(f: f64) -> String {
    if f.is_nan() {
        return ".nan".into();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-.inf".into() } else { ".inf".into() };
    }
    let s = json::js_number_string(f);
    let body = s.strip_prefix('-').unwrap_or(&s);
    if body.bytes().all(|b| b.is_ascii_digit()) {
        return format!("{s}.0");
    }
    if let Some(e) = body.find('e') {
        if body[..e].bytes().all(|b| b.is_ascii_digit()) {
            let sign = if s.starts_with('-') { "-" } else { "" };
            return format!("{sign}{}.0{}", &body[..e], &body[e..]);
        }
    }
    s
}

/// The JavaScript `SerializeOptions` of a `*With` op.
fn options_of(v: &Json) -> FigSerializeOptions {
    let mut o = FigSerializeOptions::default();
    if let Some(Json::Bool(false)) = v.get("pretty") {
        o.pretty = false;
    }
    if let Some(Json::Num(n)) = v.get("indent") {
        o.indent = (*n as i64 & 0xff) as u8;
    }
    if let Some(Json::Bool(b)) = v.get("stripComments") {
        o.strip_comments = *b;
    }
    if let Some(Json::Bool(b)) = v.get("lossless") {
        o.lossless = *b;
    }
    if let Some(Json::Num(n)) = v.get("width") {
        o.width = (*n as i64 & 0xffff) as u16;
    }
    o
}

/// `render`: the value as fig spells it in `format`, through
/// `fig_value_serialize_opts`, with one trailing newline stripped
/// (`valueText`, or `valueTextWith` without the forced flow).
fn value_text(v: &Json, format: i32, options: Option<&Json>, flow: bool) -> Result<Vec<u8>, Status> {
    let mut value = Value::create();
    let root = build(&mut value, v)?;
    let mut opts = options.map(options_of).unwrap_or_default();
    opts.flow = flow;
    opts.splice = true;
    let bytes = value.serialize_opts(root, format, Some(&opts))?;
    // The binding reads the bytes back as a JavaScript string.
    let mut s = String::from_utf8_lossy(bytes).into_owned().into_bytes();
    if s.last() == Some(&b'\n') {
        s.pop();
    }
    Ok(s)
}

/// `stringOf`: the string a value is, when it is one.
fn string_of(v: &Json) -> Option<&[u8]> {
    v.as_str()
}

fn run(ed: &mut Editor, format: i32, op: &str, a: &[Json]) -> Op {
    match op {
        // ── value edits ──
        "replaceValue" | "replaceValueWith" => {
            let p = path(arg(a, 0))?;
            let with = (op == "replaceValueWith").then(|| arg(a, 2));
            let t = value_text(arg(a, 1), format, with, with.is_none())?;
            match string_of(arg(a, 1)) {
                Some(s) => ed.replace_string(&p, &t, s)?,
                None => ed.replace_val(&p, &t)?,
            }
        }
        "setValue" | "setValueWith" => {
            let p = path(arg(a, 0))?;
            let with = (op == "setValueWith").then(|| arg(a, 2));
            let t = value_text(arg(a, 1), format, with, with.is_none())?;
            match string_of(arg(a, 1)) {
                Some(s) => ed.set_string(&p, &t, s)?,
                None => ed.set(&p, &t)?,
            }
        }
        "insertValue" | "insertValueWith" => {
            let p = path(arg(a, 0))?;
            let with = (op == "insertValueWith").then(|| arg(a, 3));
            let t = value_text(arg(a, 2), format, with, with.is_none())?;
            ed.insert_named_key(&p, &string(arg(a, 1)), &t)?;
        }
        "renameKey" => ed.replace_named_key(&path(arg(a, 0))?, &string(arg(a, 1)))?,
        "appendValue" | "appendValueWith" => {
            let p = path(arg(a, 0))?;
            let with = (op == "appendValueWith").then(|| arg(a, 2));
            ed.append_seq(&p, &value_text(arg(a, 1), format, with, with.is_none())?)?;
        }
        "prependValue" | "prependValueWith" => {
            let p = path(arg(a, 0))?;
            let with = (op == "prependValueWith").then(|| arg(a, 2));
            ed.prepend_seq(&p, &value_text(arg(a, 1), format, with, with.is_none())?)?;
        }
        "insertItem" | "insertItemWith" => {
            let p = path(arg(a, 0))?;
            let with = (op == "insertItemWith").then(|| arg(a, 3));
            let t = value_text(arg(a, 2), format, with, with.is_none())?;
            ed.insert_seq_item(&p, index(arg(a, 1))?, &t)?;
        }
        "setSequence" => {
            let p = path(arg(a, 0))?;
            let Json::Arr(items) = arg(a, 1) else { return Err(Refused::Threw("TypeError".into())) };
            let mut texts = Vec::with_capacity(items.len());
            for x in items {
                texts.push(value_text(x, format, None, true)?);
            }
            let refs: Vec<&[u8]> = texts.iter().map(Vec::as_slice).collect();
            ed.set_sequence(&p, &refs)?;
        }
        // ── raw edits ──
        "replaceValueRaw" => ed.replace_val(&path(arg(a, 0))?, &string(arg(a, 1)))?,
        "setValueRaw" => ed.set(&path(arg(a, 0))?, &string(arg(a, 1)))?,
        "insertValueRaw" => ed.insert_named_key(&path(arg(a, 0))?, &string(arg(a, 1)), &string(arg(a, 2)))?,
        "appendValueRaw" => ed.append_seq(&path(arg(a, 0))?, &string(arg(a, 1)))?,
        "prependValueRaw" => ed.prepend_seq(&path(arg(a, 0))?, &string(arg(a, 1)))?,
        "insertItemRaw" => ed.insert_seq_item(&path(arg(a, 0))?, index(arg(a, 1))?, &string(arg(a, 2)))?,
        // ── structural edits ──
        "deleteKey" => ed.delete_key(&path(arg(a, 0))?)?,
        "deleteItem" => ed.remove_seq_item(&path(arg(a, 0))?, index(arg(a, 1))?)?,
        "moveKey" => ed.move_key(&path(arg(a, 0))?, &path(arg(a, 1))?)?,
        "reorderKeys" => {
            let p = path(arg(a, 0))?;
            let keys = strings(arg(a, 1))?;
            let refs: Vec<&[u8]> = keys.iter().map(Vec::as_slice).collect();
            ed.reorder_keys(&p, &refs)?;
        }
        "moveItem" => ed.move_item(&path(arg(a, 0))?, index(arg(a, 1))?, index(arg(a, 2))?)?,
        "reorderItems" => ed.reorder_items(&path(arg(a, 0))?, &indices(arg(a, 1))?)?,
        // ── comments ──
        "addLeadingComment" => ed.add_leading_comment(&path(arg(a, 0))?, &string(arg(a, 1)))?,
        "setTrailingComment" => ed.set_trailing_comment(&path(arg(a, 0))?, &string(arg(a, 1)))?,
        "deleteLeadingComment" => ed.delete_leading_comments(&path(arg(a, 0))?)?,
        "deleteTrailingComment" => ed.delete_trailing_comment(&path(arg(a, 0))?)?,
        "addDanglingComment" => ed.add_dangling_comment(&path(arg(a, 0))?, &string(arg(a, 1)))?,
        "deleteDanglingComment" => ed.delete_dangling_comments(&path(arg(a, 0))?)?,
        "getLeadingComment" | "getTrailingComment" | "getDanglingComment" => {
            let p = path(arg(a, 0))?;
            let got = match op {
                "getLeadingComment" => ed.get_leading_comment(&p),
                "getTrailingComment" => ed.get_trailing_comment(&p),
                _ => ed.get_dangling_comment(&p),
            };
            // `NotFound` is the answer "none" (null), not a failure.
            match got {
                Ok(_) | Err(Status::NotFound) => {}
                Err(s) => return Err(s.into()),
            }
        }
        "commentOut" => ed.comment_out(&path(arg(a, 0))?)?,
        "uncommentLeading" => ed.uncomment_leading(&path(arg(a, 0))?, index(arg(a, 1))?, index(arg(a, 2))?)?,
        "uncommentDangling" => ed.uncomment_dangling(&path(arg(a, 0))?, index(arg(a, 1))?, index(arg(a, 2))?)?,
        // ── whole containers ──
        "deleteContainer" => ed.delete_container(&path(arg(a, 0))?)?,
        "insertContainer" => ed.insert_container(&path(arg(a, 0))?, &string(arg(a, 1)))?,
        "renameContainer" => ed.rename_container(&path(arg(a, 0))?, &string(arg(a, 1)))?,
        "moveContainer" => {
            let src = path(arg(a, 0))?;
            match arg(a, 1) {
                Json::Null => ed.move_container(&src, None)?,
                // The root is no destination; the binding refuses it before
                // the call, since an empty path would cross as "to the end".
                Json::Arr(d) if d.is_empty() => return Err(Status::InvalidArgument.into()),
                d => {
                    let dest = path(d)?;
                    ed.move_container(&src, Some(&dest))?;
                }
            }
        }
        "reorderContainers" => {
            let order = strings(arg(a, 0))?;
            let refs: Vec<&[u8]> = order.iter().map(Vec::as_slice).collect();
            ed.reorder_containers(&refs)?;
        }
        "appendContainerToSeq" => ed.append_container_to_seq(&path(arg(a, 0))?, &string(arg(a, 1)))?,
        _ => return Err(Refused::Threw(format!("Error: fig's Editor has no op {op}"))),
    }
    Ok(())
}
