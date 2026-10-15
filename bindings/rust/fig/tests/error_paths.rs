//! A typed value that fails to build below the root says where: an
//! `Error::At` whose path is spelled as a diagnostic's is, through both the
//! `derive` and the `serde` readers.
//! Run with `cargo test -p fig --features derive,serde`.
#![cfg(all(feature = "derive", feature = "serde"))]

mod common;

use std::collections::BTreeMap;

use fig::{Error, FromValue};

#[derive(Debug, FromValue, serde::Deserialize)]
#[allow(dead_code)]
struct Config {
    name: String,
    servers: Vec<Server>,
    #[fig(default)]
    #[serde(default)]
    limits: BTreeMap<String, u8>,
}

#[derive(Debug, FromValue, serde::Deserialize)]
#[allow(dead_code)]
struct Server {
    host: String,
    #[fig(alias = "p")]
    #[serde(alias = "p")]
    port: u16,
}

#[derive(Debug, FromValue, serde::Deserialize)]
#[allow(dead_code)]
enum Shape {
    Circle { radius: u8 },
    Pair(u8, u8),
}

const BAD_PORT: &str =
    "name: fig\nservers:\n  - host: a\n    port: 80\n  - host: b\n    port: high\n";

fn derived<T: FromValue + std::fmt::Debug>(yaml: &str) -> Error {
    let doc = fig::Document::parse(yaml.as_bytes(), common::by_name("yaml")).unwrap();
    T::from_value(&doc.to_value().unwrap()).unwrap_err()
}

fn deserialized<T: serde::de::DeserializeOwned + std::fmt::Debug>(yaml: &str) -> Error {
    fig::from_slice::<T>(yaml.as_bytes(), common::by_name("yaml")).unwrap_err()
}

#[test]
fn a_derived_struct_names_the_item_and_field() {
    let e = derived::<Config>(BAD_PORT);
    assert_eq!(e.path(), Some("servers[1].port"));
    assert!(matches!(e.innermost(), Error::TypeMismatch { .. }), "{e:?}");
    assert_eq!(
        e.to_string(),
        "servers[1].port: expected integer, found string"
    );
}

#[test]
fn a_serde_struct_names_the_item_and_field() {
    let e = deserialized::<Config>(BAD_PORT);
    assert_eq!(e.path(), Some("servers[1].port"));
}

#[test]
fn a_missing_field_is_located_at_its_struct() {
    let yaml = "name: fig\nservers:\n  - host: a\n";
    let e = derived::<Config>(yaml);
    assert_eq!(e.path(), Some("servers[0]"));
    assert!(
        matches!(e.innermost(), Error::MissingField { field: "port", .. }),
        "{e:?}"
    );
    assert_eq!(deserialized::<Config>(yaml).path(), Some("servers[0]"));
}

#[test]
fn an_alias_is_named_as_the_document_spells_it() {
    let yaml = "name: fig\nservers:\n  - host: a\n    p: -1\n";
    assert_eq!(derived::<Config>(yaml).path(), Some("servers[0].p"));
    assert_eq!(deserialized::<Config>(yaml).path(), Some("servers[0].p"));
}

#[test]
fn a_map_value_is_named_by_its_key() {
    let yaml = "name: fig\nservers: []\nlimits:\n  cpu: 300\n";
    assert_eq!(derived::<Config>(yaml).path(), Some("limits.cpu"));
    assert_eq!(deserialized::<Config>(yaml).path(), Some("limits.cpu"));
}

#[test]
fn an_enum_variant_is_a_key() {
    let yaml = "Circle:\n  radius: big\n";
    assert_eq!(derived::<Shape>(yaml).path(), Some("Circle.radius"));
    assert_eq!(deserialized::<Shape>(yaml).path(), Some("Circle.radius"));
    let yaml = "Pair: [1, x]\n";
    assert_eq!(derived::<Shape>(yaml).path(), Some("Pair[1]"));
    assert_eq!(deserialized::<Shape>(yaml).path(), Some("Pair[1]"));
}

#[test]
fn an_error_at_the_root_has_no_path() {
    let e = derived::<Config>("just a string\n");
    assert_eq!(e.path(), None);
    assert!(matches!(e, Error::ExpectedMapping { .. }), "{e:?}");
    assert!(std::ptr::eq(e.innermost(), &e));
}

#[test]
fn a_read_error_is_never_located() {
    let e = Error::Io(std::io::Error::other("gone")).at_key("a");
    assert!(matches!(e, Error::Io(_)), "{e:?}");
}
