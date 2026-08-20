//! The public types that grow are `#[non_exhaustive]`, so a new format, status,
//! diagnostic, or render knob is a minor release rather than a major one. This
//! file is an EXTERNAL crate, so it sees exactly what a downstream sees — its job
//! is to prove the sweep locked nobody out: every option field is still settable,
//! every value is still constructible, and every returned field is still readable.
//! If a future field/variant has no way to reach it from here, this test is where
//! that shows up.

use fig::{Document, Format, SerializeOptions, Value};

#[test]
fn every_serialize_options_field_is_reachable_without_a_struct_literal() {
    // Three constructors, then chainable setters — one per field.
    let opts = SerializeOptions::default()
        .indent(4)
        .width(120)
        .strip_comments()
        .lossless();
    assert!(opts.pretty && opts.indent == 4 && opts.width == 120);
    assert!(opts.strip_comments && opts.lossless);

    // `pretty: false` comes from the compact constructor, and composes.
    let compact = SerializeOptions::compact().width(1);
    assert!(!compact.pretty && compact.width == 1);
    assert_eq!(SerializeOptions::pretty(8).indent, 8);

    // And they still reach the serializer.
    let v = Value::Map(vec![(Value::Str("k".into()), Value::Str("v".into()))]);
    assert_eq!(
        v.serialize_with(Format::Json, SerializeOptions::compact())
            .unwrap(),
        "{\"k\":\"v\"}\n"
    );
}

#[test]
fn growable_enums_are_matchable_with_a_wildcard_and_still_constructible() {
    // Constructing a variant is unaffected by `non_exhaustive` — only exhaustive
    // matching needs the wildcard, which is the point: a new variant can't break
    // a downstream match.
    let format = Format::Yaml;
    let described = match format {
        Format::Yaml => "yaml",
        Format::Json | Format::Jsonc | Format::Json5 => "json-family",
        _ => "other",
    };
    assert_eq!(described, "yaml");

    let err = Document::parse(b"{ not valid", Format::Json).unwrap_err();
    let msg = match err {
        fig::Error::Parse(ref detail) => detail.message.clone(),
        ref other => format!("{other}"),
    };
    assert!(!msg.is_empty());

    let _embed = fig::EmbedType::FrontmatterYaml;
}

#[test]
fn returned_struct_fields_are_still_readable() {
    // `non_exhaustive` blocks construction, not field access — these are the
    // types the library hands back.
    let v = fig::version();
    assert!(v.major >= 2 || v.minor > 0 || v.patch > 0);
    let caps = fig::capabilities(Format::Yaml);
    assert!(caps.read || caps.edit || caps.serialize);

    let doc = Document::parse(b"a: null\n", Format::Yaml).unwrap();
    let warns = doc
        .diagnose(Format::Toml, SerializeOptions::default())
        .unwrap();
    assert_eq!(warns[0].path, "a");
    let _ = (&warns[0].code, &warns[0].cause, &warns[0].note);

    if let fig::Error::Parse(detail) = Document::parse(b"{ oops", Format::Json).unwrap_err() {
        let _ = (
            detail.message,
            detail.byte_offset,
            detail.line,
            detail.column,
        );
    }
}

#[test]
fn value_stays_exhaustively_matchable_on_purpose() {
    // `Value` is deliberately NOT non_exhaustive: it is the data model, and
    // matching it exhaustively is the primary way callers consume it. This match
    // has no wildcard — if a variant is ever added, this fails to compile, which
    // is the intended signal to think about it rather than a silent break.
    let v = Value::Seq(vec![Value::Null, Value::Bool(true)]);
    let name = match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Uint(_) => "uint",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::Extended { .. } => "extended",
        Value::Seq(_) => "seq",
        Value::Map(_) => "map",
    };
    assert_eq!(name, "seq");
}
