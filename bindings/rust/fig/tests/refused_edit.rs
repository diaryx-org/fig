//! What an editor says when it refuses an edit: one sentence, each piece of it
//! once, naming the problem where fig knows it. An editor such as flower shows
//! this text under the field the user typed in.
//!
//! The non-finite floats are the case that found it: a field typed `1e999999`
//! or `nan` parses to an infinite or NaN `f64`, which TOML spells `inf`/`nan`,
//! JSON5 `Infinity`/`NaN`, and JSON not at all. The splice used to carry YAML's
//! `.inf` into every format, and the refusal read
//! `failed to parse input: failed to parse input`.
#![cfg(all(feature = "toml", feature = "json", feature = "yaml"))]

use fig::{Document, Editor, ExtKind, Format, Segment, Value};

const TOML: &str = "name = \"music\"\nversion = 1\n";
const JSON: &str = "{\"name\": \"music\", \"version\": 1}\n";

/// `text` as flower turns a typed field into a value: `str::parse::<f64>`.
fn typed(text: &str) -> Value {
    Value::Float(text.parse::<f64>().unwrap())
}

fn replace(input: &str, format: Format, value: Value) -> Result<String, fig::Error> {
    let mut ed = Editor::open(input.as_bytes(), format).unwrap();
    ed.replace_value(&[Segment::Key("version")], value)?;
    Ok(ed.source().unwrap().to_owned())
}

/// No piece of `message` follows itself: `a: a`, the shape the old refusal had.
fn assert_said_once(message: &str) {
    if let Some((head, tail)) = message.split_once(": ") {
        assert!(!tail.starts_with(head), "doubled: {message:?}");
    }
    assert!(
        message.matches("failed to parse input").count() <= 1,
        "{message:?}"
    );
}

#[test]
fn toml_takes_an_infinite_or_nan_float() {
    for (typed_text, written) in [
        ("1e999999", "inf"),
        ("-1e999999", "-inf"),
        ("nan", "nan"),
        ("inf", "inf"),
    ] {
        let out = replace(TOML, Format::Toml, typed(typed_text)).unwrap();
        assert_eq!(out, format!("name = \"music\"\nversion = {written}\n"));
        // And it reads back as the same float.
        let back = Document::parse(out.as_bytes(), Format::Toml)
            .unwrap()
            .to_value()
            .unwrap();
        let Some(Value::Float(f)) = back.get("version") else {
            panic!("{back:?}")
        };
        let want = typed_text.parse::<f64>().unwrap();
        assert!(
            f.to_bits() == want.to_bits() || (f.is_nan() && want.is_nan()),
            "{f} vs {want}"
        );
    }
}

#[test]
fn json5_and_yaml_take_an_infinite_or_nan_float() {
    assert_eq!(
        replace(JSON, Format::Json5, typed("1e999999")).unwrap(),
        "{\"name\": \"music\", \"version\": Infinity}\n"
    );
    assert_eq!(
        replace(JSON, Format::Json5, typed("nan")).unwrap(),
        "{\"name\": \"music\", \"version\": NaN}\n"
    );
    assert_eq!(
        replace(
            "name: music\nversion: 1\n",
            Format::Yaml,
            typed("-1e999999")
        )
        .unwrap(),
        "name: music\nversion: -.inf\n"
    );
}

#[test]
fn json_refuses_an_infinite_float_and_says_why() {
    for format in [Format::Json, Format::Jsonc] {
        let err = replace(JSON, format, typed("1e999999")).unwrap_err();
        let name = if format == Format::Json {
            "JSON"
        } else {
            "JSONC"
        };
        assert_eq!(
            err.to_string(),
            format!("{name} has no way to write an infinite number")
        );
        assert_said_once(&err.to_string());
    }
}

#[test]
fn json_refuses_nan_and_says_why() {
    let err = replace(JSON, Format::Json, typed("nan")).unwrap_err();
    assert_eq!(err.to_string(), "JSON has no way to write NaN");
}

#[test]
fn a_refused_json_edit_leaves_the_document_unchanged() {
    let mut ed = Editor::open(JSON.as_bytes(), Format::Json).unwrap();
    ed.replace_value(&[Segment::Key("version")], typed("1e999999"))
        .unwrap_err();
    assert_eq!(ed.source().unwrap(), JSON);
}

#[test]
fn serializing_an_infinite_float_to_json_says_why() {
    let v = Value::Map(vec![(
        Value::Str("k".into()),
        Value::Seq(vec![Value::Float(f64::NEG_INFINITY)]),
    )]);
    assert_eq!(
        v.serialize(Format::Json).unwrap_err().to_string(),
        "JSON has no way to write an infinite number"
    );
    // Other refusals keep their own words.
    let null = Value::Map(vec![(Value::Str("k".into()), Value::Null)]);
    assert!(matches!(
        null.serialize(Format::Toml),
        Err(fig::Error::UnsupportedFormat)
    ));
}

#[test]
fn a_value_the_document_would_not_parse_with_is_one_sentence() {
    // Text fig cannot vouch for: an extended scalar's text is spliced as is.
    let bad = Value::Extended {
        kind: ExtKind::LocalDate,
        text: "not a date".into(),
    };
    let err = replace(TOML, Format::Toml, bad).unwrap_err();
    assert!(matches!(err, fig::Error::Parse(_)), "{err:?}");
    assert_eq!(err.to_string(), "the new value would not parse as TOML");
    assert_said_once(&err.to_string());
}

#[test]
fn a_key_the_document_would_not_parse_with_is_one_sentence() {
    let mut ed = Editor::open(TOML.as_bytes(), Format::Toml).unwrap();
    let err = ed.rename_key(&[Segment::Key("name")], "a\nb").unwrap_err();
    assert_eq!(err.to_string(), "the new key would not parse as TOML");
}

#[test]
fn opening_a_document_that_does_not_parse_names_the_failure_once() {
    let err = Editor::open(b"{\"a\": ", Format::Json).unwrap_err();
    let message = err.to_string();
    assert!(
        message.starts_with("failed to parse input: "),
        "{message:?}"
    );
    assert_ne!(message, "failed to parse input: failed to parse input");
    assert_said_once(&message);
}
