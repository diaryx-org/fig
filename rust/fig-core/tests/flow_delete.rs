//! A flow member that stands on its own line goes with that line: its
//! comment goes with it, and no neighbour's bytes change. A member that shares
//! its line keeps to its own text, and a comment between two members on one
//! line stays. Each case runs on a real language, since the separator and the
//! comment markers are the format's.

use fig_core::editor::Editor;
use fig_core::kit::ast::PathSegment;
use fig_core::kit::language::Language;

fn key(name: &[u8]) -> PathSegment<'_> {
    PathSegment::Key(name)
}

fn editor<'l>(lang: &'l dyn Language, dialect: &str, source: &str) -> Editor<'l> {
    let info = lang
        .dialects()
        .iter()
        .find(|d| d.name == dialect)
        .unwrap_or_else(|| panic!("no dialect {dialect}"));
    let mut ed = Editor::new(lang, info.dialect);
    ed.init(source.as_bytes()).unwrap();
    ed
}

/// Deleting the key `name` of the root mapping turns `source` into `want`.
fn deletes(lang: &dyn Language, dialect: &str, name: &str, source: &str, want: &str) {
    let mut ed = editor(lang, dialect, source);
    ed.delete_key(&[key(name.as_bytes())])
        .unwrap_or_else(|e| panic!("{dialect}: delete {name} of {source:?}: {e:?}"));
    assert_eq!(
        want,
        String::from_utf8_lossy(ed.source()),
        "{dialect}: delete {name} of {source:?}"
    );
}

/// Deleting item `index` of the sequence at the key `s` turns `source` into
/// `want`.
fn removes(lang: &dyn Language, dialect: &str, index: usize, source: &str, want: &str) {
    let mut ed = editor(lang, dialect, source);
    ed.remove_seq_item(&[key(b"s")], index)
        .unwrap_or_else(|e| panic!("{dialect}: remove {index} of {source:?}: {e:?}"));
    assert_eq!(
        want,
        String::from_utf8_lossy(ed.source()),
        "{dialect}: remove {index} of {source:?}"
    );
}

fn jsonc(name: &str, source: &str, want: &str) {
    deletes(&fig_json::LANGUAGE, "jsonc", name, source, want);
}

fn jsonc_item(index: usize, source: &str, want: &str) {
    removes(&fig_json::LANGUAGE, "jsonc", index, source, want);
}

#[test]
fn the_first_member_takes_the_comment_ending_its_line() {
    jsonc(
        "a",
        "{\n  \"a\": 1, // about a\n  \"b\": 2\n}\n",
        "{\n  \"b\": 2\n}\n",
    );
    deletes(
        &fig_json::LANGUAGE,
        "json5",
        "a",
        "{\n  a: 1, // about a\n  b: 2\n}\n",
        "{\n  b: 2\n}\n",
    );
    deletes(
        &fig_yaml::LANGUAGE,
        "yaml",
        "a",
        "{\n  a: 1, # about a\n  b: 2\n}\n",
        "{\n  b: 2\n}\n",
    );
    deletes(
        &fig_zon::LANGUAGE,
        "zon",
        "a",
        ".{\n    .a = 1, // about a\n    .b = 2,\n}\n",
        ".{\n    .b = 2,\n}\n",
    );
}

#[test]
fn a_later_member_still_takes_the_comment_ending_its_line() {
    jsonc(
        "a",
        "{\n  \"z\": 0,\n  \"a\": 1, // about a\n  \"b\": 2\n}\n",
        "{\n  \"z\": 0,\n  \"b\": 2\n}\n",
    );
}

#[test]
fn the_first_item_takes_the_comment_ending_its_line() {
    jsonc_item(
        0,
        "{\"s\": [\n  1, // one\n  2\n]}\n",
        "{\"s\": [\n  2\n]}\n",
    );
    removes(
        &fig_yaml::LANGUAGE,
        "yaml",
        0,
        "s: [\n  1, # one\n  2\n]\n",
        "s: [\n  2\n]\n",
    );
    removes(
        &fig_toml::LANGUAGE,
        "toml",
        0,
        "s = [\n  1, # one\n  2,\n]\n",
        "s = [\n  2,\n]\n",
    );
    removes(
        &fig_zon::LANGUAGE,
        "zon",
        0,
        ".{\n    .s = .{\n        1, // one\n        2,\n    },\n}\n",
        ".{\n    .s = .{\n        2,\n    },\n}\n",
    );
}

#[test]
fn a_comment_before_the_separator_goes_with_the_first_member() {
    jsonc(
        "a",
        "{\n  \"a\": 1 /* c */,\n  \"b\": 2\n}\n",
        "{\n  \"b\": 2\n}\n",
    );
    jsonc_item(
        0,
        "{\"s\": [\n  1 /* c */,\n  2\n]}\n",
        "{\"s\": [\n  2\n]}\n",
    );
}

#[test]
fn a_comment_before_the_separator_goes_with_a_later_member() {
    jsonc(
        "b",
        "{\n  \"a\": 1,\n  \"b\": 2 /* c */,\n  \"c\": 3\n}\n",
        "{\n  \"a\": 1,\n  \"c\": 3\n}\n",
    );
    jsonc_item(
        1,
        "{\"s\": [\n  1,\n  2 /* c */,\n  3\n]}\n",
        "{\"s\": [\n  1,\n  3\n]}\n",
    );
}

#[test]
fn an_only_member_takes_its_comment() {
    jsonc("a", "{\n  \"a\": 1 // only\n}\n", "{\n}\n");
    jsonc("a", "{\n  \"a\": 1 /* only */\n}\n", "{\n}\n");
    jsonc_item(0, "{\"s\": [\n  1 // only\n]}\n", "{\"s\": [\n]}\n");
}

#[test]
fn the_last_member_takes_its_comment() {
    jsonc(
        "b",
        "{\n  \"a\": 1,\n  \"b\": 2 // about b\n}\n",
        "{\n  \"a\": 1\n}\n",
    );
    jsonc_item(
        1,
        "{\"s\": [\n  1,\n  2 // two\n]}\n",
        "{\"s\": [\n  1\n]}\n",
    );
}

#[test]
fn the_first_member_takes_the_comments_on_both_sides_of_it() {
    jsonc(
        "a",
        "{\n  // over a\n  \"a\": 1, // about a\n  \"b\": 2\n}\n",
        "{\n  \"b\": 2\n}\n",
    );
    jsonc_item(
        0,
        "{\"s\": [\n  // over one\n  1, // one\n  2\n]}\n",
        "{\"s\": [\n  2\n]}\n",
    );
}

#[test]
fn the_next_member_keeps_its_indent() {
    jsonc(
        "a",
        "{\n  \"a\": 1,\n    \"b\": 2\n}\n",
        "{\n    \"b\": 2\n}\n",
    );
    jsonc_item(
        0,
        "{\"s\": [\n  1,\n      2\n]}\n",
        "{\"s\": [\n      2\n]}\n",
    );
}

#[test]
fn the_closing_bracket_keeps_its_indent() {
    jsonc("a", "{\n  \"a\": 1\n}\n", "{\n}\n");
    deletes(
        &fig_json::LANGUAGE,
        "json5",
        "a",
        "{\n  a: 1,\n}\n",
        "{\n}\n",
    );
}

#[test]
fn a_comment_after_the_separator_goes_whichever_kind_it_is() {
    jsonc(
        "a",
        "{\n  \"a\": 1, /* c */\n  \"b\": 2\n}\n",
        "{\n  \"b\": 2\n}\n",
    );
    jsonc(
        "b",
        "{\n  \"a\": 1,\n  \"b\": 2, /* c */\n  \"c\": 3\n}\n",
        "{\n  \"a\": 1,\n  \"c\": 3\n}\n",
    );
}

#[test]
fn a_first_member_on_a_crlf_line_takes_the_line() {
    jsonc(
        "a",
        "{\r\n  \"a\": 1, // about a\r\n  \"b\": 2\r\n}\r\n",
        "{\r\n  \"b\": 2\r\n}\r\n",
    );
}

#[test]
fn a_comment_between_two_members_on_a_line_stays() {
    jsonc(
        "a",
        "{ \"a\": 1, /* c */ \"b\": 2 }\n",
        "{ /* c */ \"b\": 2 }\n",
    );
    jsonc(
        "a",
        "{\n  \"a\": 1, /* c */ \"b\": 2\n}\n",
        "{\n  /* c */ \"b\": 2\n}\n",
    );
}

#[test]
fn a_member_sharing_its_line_leaves_the_next_where_it_stands() {
    jsonc("a", "{\n  \"a\": 1, \"b\": 2\n}\n", "{\n  \"b\": 2\n}\n");
    jsonc("a", "{ \"a\": 1,\n  \"b\": 2 }\n", "{ \"b\": 2 }\n");
}

#[test]
fn a_separator_at_the_start_of_the_next_line_goes_with_the_first_member() {
    jsonc(
        "a",
        "{\n  \"a\": 1\n  , \"b\": 2\n}\n",
        "{\n  \"b\": 2\n}\n",
    );
}
