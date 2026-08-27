//! Behavioural tests for `from figl` / `to figl`, driven in-process through
//! `nu-plugin-test-support` (a real engine and a fake plugin registration, so
//! the commands run exactly as the shell would run them).
//!
//! The engine here is `nu-cmd-lang`'s core context, not the full command set, so
//! these pipelines deliberately use no nushell built-ins beyond the plugin's own
//! commands; assertions are made against the returned `Value` in Rust instead.

use chrono::{FixedOffset, TimeZone, Utc};
use nu_plugin_fig::{FigPlugin, from_figl::FromFigl, to_figl::ToFigl};
use nu_plugin_test_support::PluginTest;
use nu_protocol::{ShellError, Span, Value};

// `ShellError` is a big enum, and clippy would rather it were boxed — but it is
// nushell's error type on nushell's trait, not ours to reshape, and this is a
// test helper.
#[allow(clippy::result_large_err)]
fn eval(source: &str) -> Result<Value, ShellError> {
    PluginTest::new("fig", FigPlugin.into())?
        .eval(source)?
        .into_value(Span::test_data())
}

fn field<'a>(value: &'a Value, column: &str) -> &'a Value {
    value
        .as_record()
        .unwrap_or_else(|_| panic!("expected a record, got {}", value.get_type()))
        .get(column)
        .unwrap_or_else(|| panic!("no column `{column}`"))
}

#[test]
fn examples_hold() {
    PluginTest::new("fig", FigPlugin.into())
        .unwrap()
        .test_command_examples(&FromFigl)
        .unwrap();
    PluginTest::new("fig", FigPlugin.into())
        .unwrap()
        .test_command_examples(&ToFigl)
        .unwrap();
}

#[test]
fn scalars_and_containers_map_over() {
    let value = eval(
        r#"'i = 1
f = 1.5
s = "x"
t = true
l = [1, 2]
m = { a = 1 }' | from figl"#,
    )
    .unwrap();

    assert_eq!(field(&value, "i").as_int().unwrap(), 1);
    assert_eq!(field(&value, "f").as_float().unwrap(), 1.5);
    assert_eq!(field(&value, "s").as_str().unwrap(), "x");
    assert!(field(&value, "t").as_bool().unwrap());
    assert_eq!(field(&value, "l").as_list().unwrap().len(), 2);
    assert_eq!(field(&value, "m").as_record().unwrap().len(), 1);
}

/// The failure that ruled out routing through TOML: TOML has no null, so
/// `fig get -o toml` drops the *key* along with the value. `figl/homebrew.figl`
/// and `figl/release-binaries.figl` in this repo both hit it on
/// `on.workflow_dispatch`. Reading the tree directly, the key survives with a
/// `nothing` in it.
#[test]
fn null_survives_as_nothing() {
    let value = eval(
        r#"'workflow_dispatch = null
push = 1' | from figl"#,
    )
    .unwrap();

    let record = value.as_record().unwrap();
    assert!(
        record.contains("workflow_dispatch"),
        "the null-valued key must still exist as a column"
    );
    assert_eq!(field(&value, "workflow_dispatch"), &Value::test_nothing());
}

/// The failure that ruled out routing through JSON (and YAML): neither carries a
/// date type, so every figl datetime arrives stringly. Here all three
/// zone-bearing shapes become real `datetime`s.
#[test]
fn datetimes_become_datetimes() {
    let value = eval(
        r#"'date = 2026-05-08
local = 2026-05-08T07:32:00
offset = 2026-05-08T07:32:00-06:00
clock = 10:30' | from figl"#,
    )
    .unwrap();

    // A bare date is midnight UTC, and a zoneless datetime is read as UTC —
    // both matching what nushell's own `from toml` does with the same spellings.
    assert_eq!(
        field(&value, "date").as_date().unwrap(),
        Utc.with_ymd_and_hms(2026, 5, 8, 0, 0, 0)
            .unwrap()
            .fixed_offset()
    );
    assert_eq!(
        field(&value, "local").as_date().unwrap(),
        Utc.with_ymd_and_hms(2026, 5, 8, 7, 32, 0)
            .unwrap()
            .fixed_offset()
    );
    assert_eq!(
        field(&value, "offset").as_date().unwrap(),
        FixedOffset::west_opt(6 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 5, 8, 7, 32, 0)
            .unwrap()
    );
    // A bare clock time has no nushell type to land in, so it stays text.
    assert_eq!(field(&value, "clock").as_str().unwrap(), "10:30");
}

/// nushell's `int` is an i64. Above that the digits are kept as text rather than
/// rounded into a float.
#[test]
fn integers_past_i64_keep_their_digits() {
    let value = eval("'big = 18446744073709551615' | from figl").unwrap();
    assert_eq!(
        field(&value, "big").as_str().unwrap(),
        "18446744073709551615"
    );
}

/// Comments are dropped — a nushell record has nowhere to put them. Asserted
/// rather than merely documented, because it is the one property that makes
/// `from figl | to figl` unsuitable for editing a real file.
#[test]
fn comments_are_dropped_but_parse_cleanly() {
    let value = eval(
        r#"'# a leading comment
port = 8080 # an inline one' | from figl"#,
    )
    .unwrap();

    let record = value.as_record().unwrap();
    assert_eq!(record.len(), 1);
    assert_eq!(field(&value, "port").as_int().unwrap(), 8080);
}

#[test]
fn to_figl_renders_a_record() {
    let rendered = eval("{name: 'api', replicas: 2, ports: [80, 443]} | to figl").unwrap();
    let text = rendered.as_str().unwrap();

    // figl writes an unambiguous string bare — no quotes needed, which is
    // rather the point of the dialect.
    assert!(text.contains("name = api"), "got: {text}");
    assert!(text.contains("replicas = 2"), "got: {text}");
    assert!(text.contains("ports = [80, 443]"), "got: {text}");
}

/// The two types that no carrier format preserved, surviving a full round trip.
#[test]
fn round_trip_keeps_nulls_and_datetimes() {
    let value =
        eval("{released: 2026-05-08T07:32:00Z, workflow_dispatch: null} | to figl | from figl")
            .unwrap();

    assert_eq!(field(&value, "workflow_dispatch"), &Value::test_nothing());
    assert_eq!(
        field(&value, "released").as_date().unwrap(),
        Utc.with_ymd_and_hms(2026, 5, 8, 7, 32, 0)
            .unwrap()
            .fixed_offset()
    );
}

#[test]
fn a_parse_failure_is_reported_not_swallowed() {
    let error = eval(r#"'key = [1, 2' | from figl"#).unwrap_err();
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("parse figl"),
        "expected a figl parse error, got: {rendered}"
    );
}

/// nushell's value space is wider than any config format's; a closure has no
/// figl spelling and must say so rather than serialize to something wrong.
#[test]
fn unsupported_values_are_refused() {
    let error = eval("{f: {|| 1 }} | to figl").unwrap_err();
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("figl"),
        "expected a figl-specific error, got: {rendered}"
    );
}
