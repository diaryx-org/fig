//! dotenv written with `fig::kit` (`examples/kit_dotenv_helper`) held to
//! fig's compiled dotenv, table for table and byte for byte: each document
//! in the dotenv twin fixtures (`bindings/typescript/test/fixtures/dotenv/`)
//! parses to the table the compiled format gives for it (`*.table.json`,
//! from `fig lang table`), and prints to what the compiled printer wrote
//! (`*.printed`). `tools/cli-lang-check.sh` holds the same language to the
//! compiled one through the CLI, with `fig lang check`.

mod kit_common;

#[path = "../examples/kit_dotenv_helper/language.rs"]
mod language;

use std::path::PathBuf;

use fig::language::{Language, PrintOptions};
use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../typescript/test/fixtures/dotenv")
}

#[test]
fn every_fixture_parses_to_the_compiled_table_and_prints_as_the_compiled_printer() {
    let lang = language::KitDotenv { name: "kit-dotenv".into() };
    let mut names: Vec<_> = std::fs::read_dir(fixtures())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "env"))
        .collect();
    names.sort();
    assert!(!names.is_empty());
    for p in names {
        let input = std::fs::read(&p).unwrap();
        let t = lang
            .parse("kit-dotenv", &input)
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let want: Value = serde_json::from_str(&std::fs::read_to_string(p.with_extension("table.json")).unwrap()).unwrap();
        assert_eq!(kit_common::wire(&t), want, "{}", p.display());
        let printed = p.with_extension("printed");
        if printed.exists() {
            let got = lang.print("kit-dotenv", &t, &PrintOptions::default()).unwrap();
            assert_eq!(String::from_utf8(got).unwrap(), std::fs::read_to_string(&printed).unwrap(), "{}", p.display());
        }
    }
}

/// Each input, and the byte the compiled dotenv refuses it at (`fig lang
/// table -i dotenv` reports it as a line and column).
#[test]
fn refusals_are_at_the_compiled_offsets() {
    let lang = language::KitDotenv { name: "kit-dotenv".into() };
    for (input, at) in [
        ("A=\"x", 4),
        ("A='x\ny", 6),
        ("=x\n", 0),
        ("1A=x\n", 0),
        ("A x\n", 2),
        ("A=\"x\" y\n", 6),
        ("A=\"\\q\"\n", 6),
        ("A=1\rB=2\n", 3),
    ] {
        let e = lang.parse("kit-dotenv", input.as_bytes()).unwrap_err();
        assert_eq!(e.byte_offset, Some(at), "{input:?}: {}", e.message);
    }
}

#[test]
fn registers_and_reads_as_fig_does() {
    let f = fig::language::register(language::KitDotenv { name: "kit-dotenv-reg".into() }).unwrap()[0];
    let doc = fig::Document::parse(b"# c\nexport A = 'x y' # t\nA=2\n", f).unwrap();
    let v = doc.to_value().unwrap();
    assert_eq!(v.get("A").and_then(|a| a.as_str()), Some("2"));
}
