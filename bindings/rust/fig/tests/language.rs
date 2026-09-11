//! A language written in Rust registers through `fig::language` and is a
//! peer of every compiled format at every entry point: parsed, converted
//! out of and into, and edited. The language is `tinykv` — `key=value`
//! lines and `#` comments — the same one the core's C probe hosts.

use fig::language::{
    CommentForm, CommentRow, CommentSlot, Description, Dialect, Language, LanguageError, Literal,
    NodeKind, NodeRow, NodeTable, PrintOptions, RenderArgs, Renderer, Renderers, Splice, Syntax,
};
use fig::{Capabilities, Document, Editor, Error, Format, Segment, Span, Value};

struct TinyKv;

impl Language for TinyKv {
    fn describe(&self) -> Description {
        let mut d = Description::new("tinykv");
        d.caps = Capabilities::new(true, true, true);
        d.max_mapping_depth = Some(0);
        d.syntax = Some(Syntax {
            kv_sep: Some("=".into()),
            empty_map_literal: Some("{}".into()),
            flow_containers: false,
            ..Default::default()
        });
        d.dialects = vec![Dialect {
            extensions: vec!["tkv".into()],
            splice: Splice::Raw,
            empty_doc_seed: Some(String::new()),
            ..Dialect::new("tinykv")
        }];
        d.samples = vec!["a=1\nb=two\n".into(), "# top\nk=v\n".into()];
        d.renderers = Renderers {
            value: true,
            ..Default::default()
        };
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
                t.comments.push(CommentRow {
                    node: key,
                    slot: CommentSlot::Leading,
                    style: CommentForm::Line,
                    text,
                });
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
    assert!(matches!(tkv, Format::Runtime(_)));
    assert_eq!(Format::by_name("tinykv"), Some(tkv));
    assert_eq!(Format::by_name("json"), Some(Format::Json));
    assert_eq!(Format::by_name("nosuch"), None);
    assert_eq!(fig::capabilities(tkv), Capabilities::new(true, true, true));

    // Parse, read, convert out.
    let src = "# note\nx=1\ny=two\n";
    let doc = Document::parse(src.as_bytes(), tkv).expect("parses");
    let value = doc.to_value().expect("value");
    assert_eq!(value.get("x").and_then(Value::as_str), Some("1"));
    assert_eq!(
        doc.serialize(Format::Json).unwrap(),
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
        Err(Error::Language(msg)) => assert!(msg.contains("already registered"), "{msg}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
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
        Err(Error::Language(msg)) => assert!(msg.contains("sample does not parse"), "{msg}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(Format::by_name("broken"), None);
}

// ── the helper wire ────────────────────────────────────────────────────────

#[cfg(feature = "json")]
#[test]
fn the_helper_wire_round_trips_describe_parse_print_and_render() {
    use fig::helper;
    let lang = TinyKv;

    // describe: what comes back decodes to what was declared.
    let resp = helper::handle(&lang, r#"{"op":"describe"}"#);
    assert_eq!(resp.get("ok").and_then(Value::as_bool), Some(true));
    let desc = helper::description_from_value(resp.get("description").unwrap()).unwrap();
    assert_eq!(desc, lang.describe());

    // parse: the table comes back and decodes to what parse gave.
    let resp = helper::handle(
        &lang,
        r##"{"op":"parse","dialect":"tinykv","input":"# c\nk=v\n"}"##,
    );
    assert_eq!(
        resp.get("ok").and_then(Value::as_bool),
        Some(true),
        "{}",
        helper::encode(&resp)
    );
    let table = helper::table_from_value(resp.get("table").unwrap()).unwrap();
    assert_eq!(table, lang.parse("tinykv", b"# c\nk=v\n").unwrap());
    assert_eq!(table.comments[0].text, "c");
    assert_eq!(table.rows[1].sep, Some(Span { start: 5, end: 6 }));

    // print: the same table goes out and the text comes back.
    let req = Value::Map(vec![
        ("op".into(), "print".into()),
        ("dialect".into(), "tinykv".into()),
        ("table".into(), helper::table_to_value(&table)),
    ]);
    let resp = helper::handle(&lang, &helper::encode(&req));
    assert_eq!(
        resp.get("output").and_then(Value::as_str),
        Some("# c\nk=v\n")
    );

    // render, and a failure.
    let resp = helper::handle(
        &lang,
        r#"{"op":"render","which":"value","dialect":"tinykv","value":"abc"}"#,
    );
    assert_eq!(resp.get("output").and_then(Value::as_str), Some("ABC"));
    // `literal` rides the request; absent, it is a string.
    let resp = helper::handle(
        &lang,
        r#"{"op":"render","which":"value","dialect":"tinykv","value":"42","literal":"int"}"#,
    );
    assert_eq!(resp.get("output").and_then(Value::as_str), Some("#42"));
    let resp = helper::handle(&lang, r#"{"op":"parse","dialect":"tinykv","input":"nope"}"#);
    assert_eq!(resp.get("ok").and_then(Value::as_bool), Some(false));
    assert_eq!(
        resp.get("message").and_then(Value::as_str),
        Some("expected key=value")
    );
    assert_eq!(resp.get("byte_offset").and_then(Value::as_u64), Some(0));

    // The loop itself, over a buffer.
    let input = b"{\"op\":\"describe\"}\n\n{\"op\":\"parse\",\"dialect\":\"tinykv\",\"input\":\"a=b\\n\"}\n";
    let mut out = Vec::new();
    helper::serve_io(&lang, &input[..], &mut out).unwrap();
    let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("{\"ok\":true,\"description\":"));
    assert!(lines[1].contains("\"rows\":["));
}
