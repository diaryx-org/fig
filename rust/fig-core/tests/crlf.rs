//! A line the editor writes into a CRLF file ends in CRLF: one edit must not
//! leave a file with mixed line endings. Each case runs on a real language,
//! since the splice a format takes (block entry, flow entry, section header,
//! comment) is the format's.

use fig_core::editor::Editor;
use fig_core::kit::ast::PathSegment;
use fig_core::kit::language::Language;

fn editor<'l>(lang: &'l dyn Language, name: &str, source: &[u8]) -> Editor<'l> {
    let info = lang.dialects().iter().find(|d| d.name == name).unwrap_or_else(|| panic!("no dialect {name}"));
    let mut ed = Editor::new(lang, info.dialect);
    ed.init(source).unwrap();
    ed
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn key(name: &[u8]) -> PathSegment<'_> {
    PathSegment::Key(name)
}

/// `edit` on `source` writes `want`, and no line of it ends in a bare LF.
fn writes(lang: &dyn Language, name: &str, source: &str, edit: impl Fn(&mut Editor<'_>), want: &str) {
    let mut ed = editor(lang, name, source.as_bytes());
    edit(&mut ed);
    assert_eq!(want, text(ed.source()), "{name}");
}

#[test]
fn a_block_entry_ends_as_the_file_does() {
    let set = |ed: &mut Editor<'_>| ed.set(&[key(b"c")], b"3").unwrap();
    writes(&fig_yaml::LANGUAGE, "yaml", "a: 1\r\nb: 2\r\n", set, "a: 1\r\nb: 2\r\nc: 3\r\n");
    writes(&fig_toml::LANGUAGE, "toml", "a = 1\r\n", set, "a = 1\r\nc = 3\r\n");
    writes(&fig_ini::LANGUAGE, "ini", "a = 1\r\n", set, "a = 1\r\nc = 3\r\n");
    writes(&fig_dotenv::LANGUAGE, "dotenv", "a=1\r\n", set, "a=1\r\nc=3\r\n");
    writes(&fig_properties::LANGUAGE, "properties", "a=1\r\n", set, "a=1\r\nc=3\r\n");
    writes(&fig_figl::LANGUAGE, "figl", "a = 1\r\n", set, "a = 1\r\nc = 3\r\n");
    writes(&fig_nestedtext::LANGUAGE, "nestedtext", "a: 1\r\n", set, "a: 1\r\nc: 3\r\n");
}

#[test]
fn an_entry_under_a_section_header_ends_as_the_file_does() {
    let set = |ed: &mut Editor<'_>| ed.set(&[key(b"t"), key(b"c")], b"3").unwrap();
    writes(&fig_toml::LANGUAGE, "toml", "[t]\r\na = 1\r\n", set, "[t]\r\na = 1\r\nc = 3\r\n");
    writes(&fig_ini::LANGUAGE, "ini", "[t]\r\na = 1\r\n", set, "[t]\r\na = 1\r\nc = 3\r\n");
}

#[test]
fn a_flow_entry_on_a_line_of_its_own_ends_as_the_file_does() {
    let set = |ed: &mut Editor<'_>| ed.set(&[key(b"c")], b"3").unwrap();
    writes(&fig_json::LANGUAGE, "json", "{\r\n  \"a\": 1\r\n}\r\n", set, "{\r\n  \"a\": 1,\r\n  \"c\": 3\r\n}\r\n");
}

#[test]
fn a_block_value_written_over_several_lines_ends_each_as_the_file_does() {
    let set = |ed: &mut Editor<'_>| ed.set(&[key(b"c")], b"x: 1\ny: 2").unwrap();
    writes(&fig_yaml::LANGUAGE, "yaml", "a: 1\r\n", set, "a: 1\r\nc:\r\n  x: 1\r\n  y: 2\r\n");
}

#[test]
fn a_sequence_item_ends_as_the_file_does() {
    let append = |ed: &mut Editor<'_>| ed.append_to_seq(&[key(b"l")], b"2").unwrap();
    writes(&fig_yaml::LANGUAGE, "yaml", "l:\r\n  - 1\r\n", append, "l:\r\n  - 1\r\n  - 2\r\n");
    writes(&fig_json::LANGUAGE, "json", "{\"l\": [\r\n  1\r\n]}\r\n", append, "{\"l\": [\r\n  1,\r\n  2\r\n]}\r\n");
}

#[test]
fn a_comment_line_ends_as_the_file_does() {
    let lead = |ed: &mut Editor<'_>| ed.add_leading_comment(&[key(b"b")], b"note").unwrap();
    writes(&fig_yaml::LANGUAGE, "yaml", "a: 1\r\nb: 2\r\n", lead, "a: 1\r\n# note\r\nb: 2\r\n");
    let dangling = |ed: &mut Editor<'_>| ed.add_dangling_comment(&[], b"note").unwrap();
    writes(&fig_toml::LANGUAGE, "toml", "a = 1\r\n", dangling, "a = 1\r\n# note\r\n");
}

#[test]
fn a_new_section_ends_as_the_file_does() {
    let add = |ed: &mut Editor<'_>| ed.insert_container(&[key(b"t")], b"x = 1\n").unwrap();
    writes(&fig_toml::LANGUAGE, "toml", "a = 1\r\n", add, "a = 1\r\n\r\n[t]\r\nx = 1\r\n");
}

#[test]
fn a_file_with_both_endings_takes_the_ending_of_the_line_it_follows() {
    let set = |ed: &mut Editor<'_>| ed.set(&[key(b"c")], b"3").unwrap();
    writes(&fig_yaml::LANGUAGE, "yaml", "a: 1\r\nb: 2\n", set, "a: 1\r\nb: 2\nc: 3\n");
    writes(&fig_yaml::LANGUAGE, "yaml", "a: 1\nb: 2\r\n", set, "a: 1\nb: 2\r\nc: 3\r\n");
}

#[test]
fn a_last_line_with_no_ending_takes_the_one_before_it() {
    let set = |ed: &mut Editor<'_>| ed.set(&[key(b"c")], b"3").unwrap();
    writes(&fig_yaml::LANGUAGE, "yaml", "a: 1\r\nb: 2", set, "a: 1\r\nb: 2\r\nc: 3\r\n");
}

#[test]
fn an_lf_file_keeps_lf() {
    let set = |ed: &mut Editor<'_>| ed.set(&[key(b"c")], b"3").unwrap();
    writes(&fig_yaml::LANGUAGE, "yaml", "a: 1\nb: 2\n", set, "a: 1\nb: 2\nc: 3\n");
}

/// plist's `<string>` keeps a carriage return as part of the string, so
/// spelling the value's own line break CRLF would change what it reads as.
#[test]
fn a_string_that_would_read_otherwise_keeps_its_line_break() {
    let source =
        "<plist version=\"1.0\">\r\n<dict>\r\n  <key>a</key>\r\n  <string>x</string>\r\n</dict>\r\n</plist>\r\n";
    let mut ed = editor(&fig_plist::LANGUAGE, "plist", source.as_bytes());
    ed.set(&[key(b"a")], b"<string>x\ny</string>").unwrap();
    assert!(text(ed.source()).contains("x\ny"), "{:?}", text(ed.source()));
}

#[test]
fn deleting_the_first_item_takes_its_whole_line_break() {
    let delete = |ed: &mut Editor<'_>| ed.remove_seq_item(&[key(b"b")], 0).unwrap();
    let source = "{\r\n  \"b\": [\r\n    1,\r\n    2\r\n  ]\r\n}\r\n";
    writes(&fig_json::LANGUAGE, "jsonc", source, delete, "{\r\n  \"b\": [\r\n    2\r\n  ]\r\n}\r\n");
}
