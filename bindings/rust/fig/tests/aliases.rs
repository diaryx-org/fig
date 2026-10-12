//! Reading values through YAML's aliases and `<<` merge keys: `to_value` and
//! serde both give the anchored node's value wherever an alias stands.
#![cfg(feature = "yaml")]

use fig::{Document, Error, Format, Value};

const SRC: &str = "\
base: &b
  host: localhost
  port: 80
name: &n fig
site:
  <<: *b
  port: 8080
also: *n
";

fn map(pairs: Vec<(&str, Value)>) -> Value {
    Value::Map(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

#[test]
fn to_value_reads_an_alias_and_a_merge_key() {
    let doc = Document::parse(SRC.as_bytes(), Format::Yaml).unwrap();
    let base = map(vec![
        ("host", Value::Str("localhost".into())),
        ("port", Value::Int(80)),
    ]);
    let want = map(vec![
        ("base", base),
        ("name", Value::Str("fig".into())),
        // The mapping's own `port` wins over the merged one, and the merged
        // keys follow its own.
        (
            "site",
            map(vec![
                ("port", Value::Int(8080)),
                ("host", Value::Str("localhost".into())),
            ]),
        ),
        ("also", Value::Str("fig".into())),
    ]);
    assert_eq!(doc.to_value().unwrap(), want);
}

#[test]
fn an_alias_with_no_value_is_a_parse_error() {
    // Undefined: the YAML parser refuses it, before any value is read.
    let err = Document::parse(b"a: *missing\n", Format::Yaml).unwrap_err();
    assert!(matches!(err, Error::Parse(_)), "{err:?}");

    // Inside the node it names: the document parses, and its value is refused.
    let src = "ok: 1\nloop: &a [1, *a]\n";
    let doc = Document::parse(src.as_bytes(), Format::Yaml).unwrap();
    let Err(Error::Parse(p)) = doc.to_value() else {
        panic!("expected a parse error");
    };
    assert_eq!(
        p.message,
        "alias `*a` refers to a node that contains it, so it has no value"
    );
    assert_eq!(p.byte_offset, src.find("*a"));
    assert_eq!((p.line, p.column), (Some(2), Some(14)));
}

#[cfg(feature = "serde")]
#[test]
fn serde_reads_an_alias_and_a_merge_key() {
    use serde::Deserialize;

    #[derive(Deserialize, PartialEq, Debug)]
    struct Server {
        host: String,
        port: u16,
    }
    #[derive(Deserialize, PartialEq, Debug)]
    struct Config {
        base: Server,
        site: Server,
        also: String,
    }
    let cfg: Config = fig::from_yaml_str(SRC).unwrap();
    assert_eq!(
        cfg,
        Config {
            base: Server {
                host: "localhost".into(),
                port: 80
            },
            site: Server {
                host: "localhost".into(),
                port: 8080
            },
            also: "fig".into(),
        }
    );

    let err = fig::from_yaml_str::<Value>("loop: &a [*a]\n").unwrap_err();
    assert!(matches!(err, Error::Parse(_)), "{err:?}");
}
