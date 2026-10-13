//! A language written in Rust registers through `fig::language` and is a
//! peer of every compiled format at every entry point: parsed, converted
//! out of and into, and edited. The language is `tinykv` — `key=value`
//! lines and `#` comments — the same one the core's C probe hosts.

mod common;

use fig::language::{
    CommentForm, CommentRow, CommentSlot, Description, Dialect, Language, LanguageError, Literal,
    NodeKind, NodeRow, NodeTable, PrintOptions, RenderArgs, Renderer, Renderers, Splice, Syntax,
};
use fig::{Capabilities, Document, Editor, Error, Format, Segment, Span, Value};

struct TinyKv;

/// A tinykv dialect named `name`: raw splicing, an empty file already a
/// document, and the one extension.
fn raw_dialect(name: &str, extension: &str) -> Dialect {
    let mut d = Dialect::new(name);
    d.extensions = vec![extension.into()];
    d.splice = Splice::Raw;
    d.empty_doc_seed = Some(String::new());
    d
}

impl Language for TinyKv {
    fn describe(&self) -> Description {
        let mut d = Description::new("tinykv");
        d.caps = Capabilities::new(true, true, true);
        d.max_mapping_depth = Some(0);
        let mut syntax = Syntax::default();
        syntax.kv_sep = Some("=".into());
        syntax.empty_map_literal = Some("{}".into());
        syntax.flow_containers = false;
        d.syntax = Some(syntax);
        d.dialects = vec![raw_dialect("tinykv", "tkv")];
        d.samples = vec!["a=1\nb=two\n".into(), "# top\nk=v\n".into()];
        d.renderers = Renderers::default();
        d.renderers.value = true;
        d
    }

    fn parse(&self, _dialect: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        let src = std::str::from_utf8(input).map_err(|_| LanguageError::new("not UTF-8"))?;
        let mut t = NodeTable::new();
        let root = t.push(NodeRow::new(
            NodeKind::Mapping,
            None,
            Span {
                start: 0,
                end: src.len(),
            },
        ));
        let mut pending: Vec<String> = Vec::new();
        let mut at = 0;
        while at < src.len() {
            let end = src[at..].find('\n').map_or(src.len(), |i| at + i);
            let line = &src[at..end];
            if line.is_empty() {
                at = end + 1;
                continue;
            }
            if let Some(rest) = line.strip_prefix('#') {
                pending.push(rest.trim_start().to_owned());
                at = end + 1;
                continue;
            }
            let eq = line
                .find('=')
                .ok_or_else(|| LanguageError::at("expected key=value", at))?;
            let kv = t.push(
                NodeRow::new(NodeKind::KeyValue, Some(root), Span { start: at, end }).with_sep(
                    Span {
                        start: at + eq,
                        end: at + eq + 1,
                    },
                ),
            );
            let key = t.push(
                NodeRow::new(
                    NodeKind::String,
                    Some(kv),
                    Span {
                        start: at,
                        end: at + eq,
                    },
                )
                .with_text(&line[..eq]),
            );
            t.push(
                NodeRow::new(
                    NodeKind::String,
                    Some(kv),
                    Span {
                        start: at + eq + 1,
                        end,
                    },
                )
                .with_text(&line[eq + 1..]),
            );
            for text in pending.drain(..) {
                t.comments.push(CommentRow::new(
                    key,
                    CommentSlot::Leading,
                    CommentForm::Line,
                    text,
                ));
            }
            at = end + 1;
        }
        Ok(t)
    }

    fn print(
        &self,
        _dialect: &str,
        table: &NodeTable,
        _options: &PrintOptions,
    ) -> Result<Vec<u8>, LanguageError> {
        let mut out = String::new();
        let rows = &table.rows;
        // A scalar root is a fragment — the text the editor will splice — and
        // its spelling in tinykv is the text itself.
        if rows.len() == 1 && rows[0].kind == NodeKind::String {
            return Ok(rows[0].text.clone().unwrap_or_default().into_bytes());
        }
        let mut i = 1;
        while i < rows.len() {
            if i + 2 >= rows.len() || rows[i].kind != NodeKind::KeyValue {
                return Err(LanguageError::new("tinykv holds a flat string map"));
            }
            let (key, val) = (&rows[i + 1], &rows[i + 2]);
            if key.kind != NodeKind::String || val.kind != NodeKind::String {
                return Err(LanguageError::new("tinykv holds a flat string map"));
            }
            for c in table
                .comments
                .iter()
                .filter(|c| c.node == (i + 1) as u32 && c.slot == CommentSlot::Leading)
            {
                out.push_str("# ");
                out.push_str(&c.text);
                out.push('\n');
            }
            out.push_str(key.text.as_deref().unwrap_or(""));
            out.push('=');
            out.push_str(val.text.as_deref().unwrap_or(""));
            out.push('\n');
            i += 3;
        }
        Ok(out.into_bytes())
    }

    fn render(&self, which: Renderer, args: RenderArgs<'_>) -> Result<Vec<u8>, LanguageError> {
        // A value is written upper-cased, so the renderer's hand is visible;
        // a number the core classified as one is marked `#`, so the
        // literal's hand is too.
        assert_eq!(which, Renderer::Value);
        let mut out = match args.literal {
            Literal::Int | Literal::Float => b"#".to_vec(),
            _ => Vec::new(),
        };
        out.extend(args.value.to_ascii_uppercase());
        Ok(out)
    }
}

#[test]
fn a_rust_language_is_a_peer_at_every_entry_point() {
    let formats = fig::language::register(TinyKv).expect("registers");
    assert_eq!(formats.len(), 1);
    let tkv = formats[0];
    assert_eq!(tkv.name(), "tinykv");
    assert_eq!(Format::by_name("tinykv"), Some(tkv));
    assert_eq!(by_name("yaml").name(), "yaml");
    assert_eq!(by_name("yaml-1.1").name(), "yaml-1.1");
    assert_ne!(by_name("yaml-1.1"), by_name("yaml"));
    assert_eq!(Format::by_name("json"), Some(by_name("json")));
    assert_eq!(Format::by_name("nosuch"), None);
    // A YAML stream's documents, each a byte range to parse on its own.
    let src = b"a: 1\n---\nb: 2\n...\n";
    let docs = by_name("yaml").split_stream(src).unwrap();
    let texts: Vec<&[u8]> = docs.iter().map(|d| &src[d.start..d.end]).collect();
    assert_eq!(texts, [&b"a: 1\n"[..], &b"---\nb: 2\n"[..]]);
    assert!(docs[1].explicit && !docs[0].explicit);
    assert!(by_name("json").split_stream(b"{}").is_err());
    // Content sniffing tries the formats in the order given.
    assert_eq!(
        Format::detect(b"{\"a\": 1}", &["json", "yaml"]),
        Some(by_name("json"))
    );
    assert_eq!(
        Format::detect(b"{\"a\": 1}", &["yaml", "json"]),
        Some(by_name("yaml"))
    );
    assert_eq!(Format::detect(b"a: [", &["nosuch", "json"]), None);
    assert_eq!(fig::capabilities(tkv), Capabilities::new(true, true, true));

    // Parse, read, convert out.
    let src = "# note\nx=1\ny=two\n";
    let doc = Document::parse(src.as_bytes(), tkv).expect("parses");
    let value = doc.to_value().expect("value");
    assert_eq!(value.get("x").and_then(Value::as_str), Some("1"));
    assert_eq!(
        doc.serialize(by_name("json")).unwrap(),
        "{\n  \"x\": \"1\",\n  \"y\": \"two\"\n}\n"
    );
    assert_eq!(doc.serialize(tkv).unwrap(), src);

    // Convert in: a built value prints through the language.
    let built = Value::Map(vec![("k".into(), "v".into())]);
    assert_eq!(built.serialize(tkv).unwrap(), "k=v\n");

    // A parse failure carries the language's message and offset.
    match Document::parse(b"x=1\nnope\n", tkv) {
        Err(Error::Parse(e)) => {
            assert_eq!(e.message, "expected key=value");
            assert_eq!(e.byte_offset, Some(4));
        }
        other => panic!("expected a parse error, got {other:?}"),
    }

    // Edit: the value renderer upper-cases what is spliced, and is told
    // what fig's bare-literal rules made of it — `42` is a number, `007`
    // and `Yes` are strings — without classifying anything itself.
    let mut ed = Editor::open(src.as_bytes(), tkv).expect("editor");
    ed.replace_value(&[Segment::Key("x")], "ten").unwrap();
    ed.insert_value(&[], "z", "three").unwrap();
    ed.insert_value(&[], "n", "42").unwrap();
    ed.insert_value(&[], "f", "2.5").unwrap();
    ed.insert_value(&[], "id", "007").unwrap();
    ed.insert_value(&[], "yes", "Yes").unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "# note\nx=TEN\ny=two\nz=THREE\nn=#42\nf=#2.5\nid=007\nyes=YES\n"
    );

    // Registering the name again is refused with the reason.
    match fig::language::register(TinyKv) {
        Err(Error::Language(failure)) => {
            assert!(failure.message.contains("already registered"), "{failure}")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// `tinykv` with its root tagged `!kv`, and a value renderer that spells
// the container it is told about — so that the request's `parent_key` and
// `parent_tag` are seen to cross the C boundary intact.
struct TaggedKv;

impl Language for TaggedKv {
    fn describe(&self) -> Description {
        let mut d = TinyKv.describe();
        d.name = "taggedkv".into();
        d.dialects = vec![raw_dialect("taggedkv", "tgkv")];
        d
    }

    fn parse(&self, dialect: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        let mut t = TinyKv.parse(dialect, input)?;
        t.rows[0].tag = Some("!kv".into());
        Ok(t)
    }

    fn print(
        &self,
        dialect: &str,
        table: &NodeTable,
        options: &PrintOptions,
    ) -> Result<Vec<u8>, LanguageError> {
        TinyKv.print(dialect, table, options)
    }

    fn render(&self, _which: Renderer, args: RenderArgs<'_>) -> Result<Vec<u8>, LanguageError> {
        let mut out = args.parent_tag.to_vec();
        out.push(b'|');
        out.extend(args.parent_key);
        out.push(b'|');
        out.extend(args.value);
        Ok(out)
    }
}

#[test]
fn a_renderer_is_told_the_container_its_fragment_goes_into() {
    let tgkv = fig::language::register(TaggedKv).expect("registers")[0];
    let mut ed = Editor::open(b"x=1\n", tgkv).expect("editor");
    ed.replace_value(&[Segment::Key("x")], "ten").unwrap();
    ed.insert_value(&[], "y", "two").unwrap();
    assert_eq!(ed.source().unwrap(), "x=!kv||ten\ny=!kv||two\n");
}

struct Broken;

impl Language for Broken {
    fn describe(&self) -> Description {
        let mut d = Description::new("broken");
        d.samples = vec!["anything".into()];
        d
    }
    fn parse(&self, _dialect: &str, _input: &[u8]) -> Result<NodeTable, LanguageError> {
        Err(LanguageError::new("never parses"))
    }
}

#[test]
fn a_language_whose_sample_fails_is_refused() {
    match fig::language::register(Broken) {
        Err(Error::Language(failure)) => assert!(
            failure.message.contains("sample does not parse"),
            "{failure}"
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(Format::by_name("broken"), None);
}

struct NulName;

impl Language for NulName {
    fn describe(&self) -> Description {
        let mut d = Description::new("nul");
        d.dialects[0].extensions = vec!["n\0l".into()];
        d.samples = vec!["a=1\n".into()];
        d
    }
    fn parse(&self, dialect: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        TinyKv.parse(dialect, input)
    }
}

#[test]
fn a_declared_string_with_a_nul_is_refused_at_the_nul() {
    match fig::language::register(NulName) {
        Err(Error::Language(failure)) => {
            assert!(failure.message.contains("NUL"), "{failure}");
            assert_eq!(failure.byte_offset, Some(1));
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// ── the helper wire ────────────────────────────────────────────────────────
#[cfg(feature = "helper")]
#[test]
fn the_helper_wire_round_trips_describe_parse_print_and_render() {
    use fig::helper;
    // The wire is `serde_json`'s, and needs no language registered.
    use serde_json::{Value, json};
    let lang = TinyKv;

    // describe: what comes back decodes to what was declared.
    let resp = helper::handle(&lang, r#"{"jsonrpc":"2.0","id":1,"method":"describe"}"#);
    assert_eq!(resp.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
    assert_eq!(resp.get("id").and_then(Value::as_u64), Some(1));
    let desc = helper::description_from_value(resp.get("result").unwrap()).unwrap();
    assert_eq!(desc, lang.describe());

    // parse: the table comes back and decodes to what parse gave.
    let resp = helper::handle(
        &lang,
        r##"{"jsonrpc":"2.0","id":2,"method":"parse","params":{"dialect":"tinykv","input":"# c\nk=v\n"}}"##,
    );
    assert!(resp.get("result").is_some(), "{}", helper::encode(&resp));
    let table = helper::table_from_value(resp.get("result").unwrap()).unwrap();
    assert_eq!(table, lang.parse("tinykv", b"# c\nk=v\n").unwrap());
    assert_eq!(table.comments[0].text, "c");
    assert_eq!(table.rows[1].sep, Some(Span { start: 5, end: 6 }));

    // A table's tag directives cross both ways, and are omitted when there
    // are none — which is every table a format without them answers with.
    assert!(!helper::encode(&helper::table_to_value(&table)).contains("directives"));
    let mut with = table.clone();
    with.directives
        .push(fig::language::DirectiveRow::new("!e!", "tag:x/"));
    let encoded = helper::encode(&helper::table_to_value(&with));
    assert!(encoded.contains(r#""directives":[{"handle":"!e!","prefix":"tag:x/"}]"#));
    let decoded = helper::decode(&encoded).unwrap();
    assert_eq!(helper::table_from_value(&decoded).unwrap(), with);

    // print: the same table goes out and the text comes back.
    let req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "print",
        "params": {"dialect": "tinykv", "table": helper::table_to_value(&table)},
    });
    let resp = helper::handle(&lang, &helper::encode(&req));
    assert_eq!(
        resp.get("result").and_then(Value::as_str),
        Some("# c\nk=v\n")
    );

    // render, and a failure.
    let resp = helper::handle(
        &lang,
        r#"{"jsonrpc":"2.0","id":4,"method":"render","params":{"which":"value","dialect":"tinykv","value":"abc"}}"#,
    );
    assert_eq!(resp.get("result").and_then(Value::as_str), Some("ABC"));
    // `literal` rides the request; absent, it is a string.
    let resp = helper::handle(
        &lang,
        r#"{"jsonrpc":"2.0","id":5,"method":"render","params":{"which":"value","dialect":"tinykv","value":"42","literal":"int"}}"#,
    );
    assert_eq!(resp.get("result").and_then(Value::as_str), Some("#42"));
    // The container rides it too.
    let resp = helper::handle(
        &TaggedKv,
        r#"{"jsonrpc":"2.0","id":6,"method":"render","params":{"which":"item","dialect":"taggedkv","value":"v","parent_key":"deps","parent_tag":"!dep"}}"#,
    );
    assert_eq!(
        resp.get("result").and_then(Value::as_str),
        Some("!dep|deps|v")
    );
    let resp = helper::handle(
        &lang,
        r#"{"jsonrpc":"2.0","id":"seven","method":"parse","params":{"dialect":"tinykv","input":"nope"}}"#,
    );
    assert_eq!(resp.get("id").and_then(Value::as_str), Some("seven"));
    let error = resp.get("error").unwrap();
    assert_eq!(
        error.get("code").and_then(Value::as_i64),
        Some(helper::LANGUAGE_REFUSED)
    );
    assert_eq!(
        error.get("message").and_then(Value::as_str),
        Some("expected key=value")
    );
    assert_eq!(
        error
            .get("data")
            .and_then(|d| d.get("byte_offset"))
            .and_then(Value::as_u64),
        Some(0)
    );
    // A request the helper cannot read, or a method it lacks, is one of the
    // codes JSON-RPC reserves.
    let code = |line: &str| {
        helper::handle(&lang, line)
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(Value::as_i64)
    };
    assert_eq!(code("not json"), Some(-32700));
    assert_eq!(code(r#"{"op":"describe"}"#), Some(-32600));
    assert_eq!(
        code(r#"{"jsonrpc":"2.0","id":8,"method":"nope"}"#),
        Some(-32601)
    );
    assert_eq!(
        code(r#"{"jsonrpc":"2.0","id":9,"method":"parse","params":{"dialect":"tinykv"}}"#),
        Some(-32602)
    );

    // The loop itself, over a buffer.
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"describe\"}\n\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"parse\",\"params\":{\"dialect\":\"tinykv\",\"input\":\"a=b\\n\"}}\n";
    let mut out = Vec::new();
    helper::serve_io(&lang, &input[..], &mut out).unwrap();
    let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
    assert_eq!(lines.len(), 2);
    let first = helper::decode(lines[0]).unwrap();
    assert_eq!(first.get("id").and_then(Value::as_u64), Some(1));
    assert!(first.get("result").is_some());
    assert!(lines[1].contains("\"rows\":["));
}

// `tinykv` taking the `$fig` envelope with a local date held natively and
// a null not: the bits the crate builds from `NativeKinds` are the kinds
// the core reads back.
struct DatedKv;

impl Language for DatedKv {
    fn describe(&self) -> Description {
        let mut d = TinyKv.describe();
        d.name = "datedkv".into();
        d.max_mapping_depth = None;
        let mut native = fig::language::NativeKinds::default();
        native.local_date = true;
        d.lossless = Some(native);
        d.dialects = vec![raw_dialect("datedkv", "dkv")];
        d
    }

    fn parse(&self, dialect: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        TinyKv.parse(dialect, input)
    }

    fn print(
        &self,
        dialect: &str,
        table: &NodeTable,
        options: &PrintOptions,
    ) -> Result<Vec<u8>, LanguageError> {
        TinyKv.print(dialect, table, options)
    }
}

#[test]
fn the_lossless_kinds_a_language_declares_are_the_ones_the_core_reads() {
    let dkv = fig::language::register(DatedKv).expect("registers")[0];
    let value = Value::Map(vec![
        ("a".into(), Value::Null),
        (
            "d".into(),
            Value::Extended {
                kind: fig::ExtKind::LocalDate,
                text: "2026-09-26".into(),
            },
        ),
        (
            "t".into(),
            Value::Extended {
                kind: fig::ExtKind::LocalTime,
                text: "12:30:00".into(),
            },
        ),
    ]);
    let warnings = value
        .diagnose(dkv, fig::SerializeOptions::default())
        .unwrap();
    let paths: Vec<&str> = warnings.iter().map(|w| w.path.as_str()).collect();
    // The null is dropped and the local time degraded; the local date,
    // declared native, is neither.
    assert_eq!(paths, ["a", "t"], "{warnings:?}");
}

/// `tinykv` under the name of a format the prebuilt library leaves out,
/// with a second dialect of its own.
struct PlistKv;

impl Language for PlistKv {
    fn describe(&self) -> Description {
        let mut d = TinyKv.describe();
        d.name = "plist".into();
        d.dialects = vec![
            raw_dialect("plist", "plist"),
            raw_dialect("plist-kv", "pkv"),
        ];
        d
    }
    fn parse(&self, dialect: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        TinyKv.parse(dialect, input)
    }
    fn print(
        &self,
        dialect: &str,
        table: &NodeTable,
        options: &PrintOptions,
    ) -> Result<Vec<u8>, LanguageError> {
        TinyKv.print(dialect, table, options)
    }
}

#[test]
fn a_language_named_after_a_format_left_out_stands_in_for_it() {
    // A library that compiles plist in has registered the name already.
    if Format::by_name("plist").is_some() {
        return;
    }
    let formats = fig::language::register(PlistKv).expect("registers");
    // Each row is a format of its own, found by its name.
    assert_eq!(formats[0].name(), "plist");
    assert_eq!(formats[1].name(), "plist-kv");
    assert_ne!(formats[0], formats[1]);
    assert_eq!(Format::by_name("plist"), Some(formats[0]));
    assert_eq!(Format::by_name("plist-kv"), Some(formats[1]));
    let doc = Document::parse(b"a=1\n", formats[0]).expect("parses");
    assert_eq!(
        doc.serialize(by_name("json")).unwrap(),
        "{\n  \"a\": \"1\"\n}\n"
    );
}

/// The registered format named `name`; a test that names one this build
/// does not carry is a test bug.
#[allow(dead_code)]
fn by_name(name: &str) -> fig::Format {
    common::by_name(name)
}
