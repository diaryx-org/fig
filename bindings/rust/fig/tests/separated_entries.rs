//! Flow members joined by a separator the language names, a root that is a
//! flow mapping with no delimiters, and a block delete that never takes a
//! sibling's line (docs/proposals/separated-entries.md). The languages are
//! a small CSS: `decls`, a `style` attribute's declaration list, and
//! `sheet`, rules of a selector over a `{ … }` declaration block.

use std::sync::OnceLock;

use fig::language::{
    Comments, Description, Dialect, Language, LanguageError, NodeKind, NodeRow, NodeTable,
    PrintOptions, Splice, Syntax,
};
use fig::{Capabilities, Editor, Format, Segment, Span};

fn sp(start: usize, end: usize) -> Span {
    Span { start, end }
}

fn trim_end(b: &[u8], from: usize, mut to: usize) -> usize {
    while to > from && b[to - 1].is_ascii_whitespace() {
        to -= 1;
    }
    to
}

/// The declarations in `src[from..to]`, `name: value` joined by `;`, as
/// keyvalues under `parent`.
fn declarations(
    t: &mut NodeTable,
    src: &str,
    parent: u32,
    from: usize,
    to: usize,
) -> Result<(), LanguageError> {
    let b = src.as_bytes();
    let mut i = from;
    loop {
        while i < to && (b[i].is_ascii_whitespace() || b[i] == b';') {
            i += 1;
        }
        if i >= to {
            return Ok(());
        }
        let key_start = i;
        while i < to && b[i] != b':' && b[i] != b';' {
            i += 1;
        }
        if i >= to || b[i] != b':' {
            return Err(LanguageError::at("expected `:`", i));
        }
        let key_end = trim_end(b, key_start, i);
        i += 1;
        while i < to && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let value_start = i;
        while i < to && b[i] != b';' {
            i += 1;
        }
        let value_end = trim_end(b, value_start, i);
        if value_end == value_start {
            return Err(LanguageError::at("expected a value", value_start));
        }
        let kv = t.push(NodeRow::new(
            NodeKind::KeyValue,
            Some(parent),
            sp(key_start, value_end),
        ));
        t.push(
            NodeRow::new(NodeKind::String, Some(kv), sp(key_start, key_end))
                .with_text(&src[key_start..key_end]),
        );
        t.push(
            NodeRow::new(NodeKind::String, Some(kv), sp(value_start, value_end))
                .with_text(&src[value_start..value_end]),
        );
    }
}

fn print_declarations(rows: &[NodeRow], from: usize, to: usize) -> String {
    let mut out = Vec::new();
    let mut i = from;
    while i + 2 < to {
        let key = rows[i + 1].text.as_deref().unwrap_or("");
        let value = rows[i + 2].text.as_deref().unwrap_or("");
        out.push(format!("{key}: {value}"));
        i += 3;
    }
    out.join("; ")
}

fn syntax() -> Syntax {
    let mut s = Syntax::default();
    s.comments = Comments::none();
    s.kv_sep = Some(": ".into());
    s.flow_entry_sep = ";".into();
    s.flow_map_pad = " ".into();
    s.flow_maps_only = true;
    s
}

fn description(name: &str, syntax: Syntax, samples: &[&str]) -> Description {
    let mut d = Description::new(name);
    d.caps = Capabilities::new(true, true, true);
    d.syntax = Some(syntax);
    let mut dialect = Dialect::new(name);
    dialect.splice = Splice::Raw;
    dialect.empty_doc_seed = Some(String::new());
    d.dialects = vec![dialect];
    d.samples = samples.iter().map(|s| s.to_string()).collect();
    d
}

fn scalar_fragment(table: &NodeTable) -> Option<Vec<u8>> {
    let rows = &table.rows;
    (rows.len() == 1 && rows[0].kind != NodeKind::Mapping)
        .then(|| rows[0].text.clone().unwrap_or_default().into_bytes())
}

struct Decls;

impl Language for Decls {
    fn describe(&self) -> Description {
        let mut s = syntax();
        s.flow_root = true;
        let mut d = description("sep-decls", s, &["fill: red; stroke: blue", "a:1;b:2;", ""]);
        d.max_mapping_depth = Some(0);
        d
    }

    fn parse(&self, _: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        let src = std::str::from_utf8(input).map_err(|_| LanguageError::new("not UTF-8"))?;
        let mut t = NodeTable::new();
        let root = t.push(NodeRow::new(NodeKind::Mapping, None, sp(0, src.len())));
        declarations(&mut t, src, root, 0, src.len())?;
        Ok(t)
    }

    fn print(
        &self,
        _: &str,
        table: &NodeTable,
        _: &PrintOptions,
    ) -> Result<Vec<u8>, LanguageError> {
        if let Some(text) = scalar_fragment(table) {
            return Ok(text);
        }
        Ok(print_declarations(&table.rows, 1, table.rows.len()).into_bytes())
    }
}

struct Sheet;

impl Language for Sheet {
    fn describe(&self) -> Description {
        let mut d = description(
            "sep-sheet",
            syntax(),
            &["rect { fill: none; stroke: red }\n", "[data-x]{a:1;}\n"],
        );
        d.max_mapping_depth = Some(1);
        d
    }

    fn parse(&self, _: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        let src = std::str::from_utf8(input).map_err(|_| LanguageError::new("not UTF-8"))?;
        let b = src.as_bytes();
        let mut t = NodeTable::new();
        let root = t.push(NodeRow::new(NodeKind::Mapping, None, sp(0, src.len())));
        let mut i = 0;
        loop {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= b.len() {
                return Ok(t);
            }
            let selector_start = i;
            let open = src[i..]
                .find('{')
                .map(|o| i + o)
                .ok_or_else(|| LanguageError::at("expected `{`", i))?;
            let close = src[open..]
                .find('}')
                .map(|c| open + c)
                .ok_or_else(|| LanguageError::at("expected `}`", open))?;
            let selector_end = trim_end(b, selector_start, open);
            let kv = t.push(NodeRow::new(
                NodeKind::KeyValue,
                Some(root),
                sp(selector_start, close + 1),
            ));
            t.push(
                NodeRow::new(NodeKind::String, Some(kv), sp(selector_start, selector_end))
                    .with_text(&src[selector_start..selector_end]),
            );
            let block = t.push(NodeRow::new(
                NodeKind::Mapping,
                Some(kv),
                sp(open, close + 1),
            ));
            declarations(&mut t, src, block, open + 1, close)?;
            i = close + 1;
        }
    }

    fn print(
        &self,
        _: &str,
        table: &NodeTable,
        _: &PrintOptions,
    ) -> Result<Vec<u8>, LanguageError> {
        if let Some(text) = scalar_fragment(table) {
            return Ok(text);
        }
        let rows = &table.rows;
        let mut out = String::new();
        let mut i = 1;
        while i < rows.len() {
            // A rule: keyvalue, selector, block, then the block's members,
            // up to the next row under the root.
            let members_end = (i + 3..rows.len())
                .find(|&r| rows[r].parent == Some(0))
                .unwrap_or(rows.len());
            out.push_str(rows[i + 1].text.as_deref().unwrap_or(""));
            out.push_str(" { ");
            out.push_str(&print_declarations(rows, i + 3, members_end));
            out.push_str(" }\n");
            i = members_end;
        }
        Ok(out.into_bytes())
    }
}

fn formats() -> (Format, Format) {
    static FORMATS: OnceLock<(Format, Format)> = OnceLock::new();
    *FORMATS.get_or_init(|| {
        let decls = fig::language::register(Decls).expect("decls registers")[0];
        let sheet = fig::language::register(Sheet).expect("sheet registers")[0];
        (decls, sheet)
    })
}

fn edit(
    format: Format,
    src: &str,
    op: impl FnOnce(&mut Editor) -> Result<(), fig::Error>,
) -> String {
    let mut e = Editor::open(src.as_bytes(), format).expect("opens");
    op(&mut e).unwrap_or_else(|err| panic!("{src:?}: {err}"));
    e.source().expect("source").to_owned()
}

fn at(keys: &[&'static str]) -> Vec<Segment<'static>> {
    keys.iter().map(|k| Segment::Key(k)).collect()
}

#[test]
fn a_declaration_list_deletes_one_member_and_one_separator() {
    let (decls, _) = formats();
    let del = |src: &str, key: &'static str| edit(decls, src, |e| e.delete_key(&at(&[key])));
    assert_eq!(
        del("fill:red;stroke:blue;opacity:1", "fill"),
        "stroke:blue;opacity:1"
    );
    assert_eq!(
        del("fill:red;stroke:blue;opacity:1", "stroke"),
        "fill:red;opacity:1"
    );
    assert_eq!(
        del("fill:red;stroke:blue;opacity:1", "opacity"),
        "fill:red;stroke:blue"
    );
    assert_eq!(del("fill: red; stroke: blue;", "stroke"), "fill: red;");
    assert_eq!(del("fill: red; stroke: blue;", "fill"), "stroke: blue;");
    assert_eq!(del("fill:red", "fill"), "");
}

#[test]
fn a_declaration_list_takes_a_member_after_its_last() {
    let (decls, _) = formats();
    let set = |src: &str| edit(decls, src, |e| e.set_value(&at(&["opacity"]), "0.5"));
    assert_eq!(set("fill:red"), "fill:red; opacity: 0.5");
    assert_eq!(set("fill:red;"), "fill:red; opacity: 0.5;");
    assert_eq!(set(""), "opacity: 0.5");
    assert_eq!(
        set("fill: red;\nstroke: blue"),
        "fill: red;\nstroke: blue;\nopacity: 0.5"
    );
    // Replacing and renaming were span splices before, and stay so.
    assert_eq!(
        edit(decls, "fill:red;stroke:blue", |e| e
            .set_value(&at(&["fill"]), "green")),
        "fill:green;stroke:blue"
    );
    assert_eq!(
        edit(decls, "fill:red;stroke:blue", |e| e
            .rename_key(&at(&["fill"]), "color")),
        "color:red;stroke:blue"
    );
}

#[test]
fn a_declaration_block_is_a_flow_mapping_joined_by_its_separator() {
    let (_, sheet) = formats();
    let set = |src: &str| edit(sheet, src, |e| e.set_value(&at(&["rect", "stroke"]), "red"));
    assert_eq!(
        set("rect { fill: none }\n"),
        "rect { fill: none; stroke: red }\n"
    );
    assert_eq!(set("rect{fill:#FFF;}\n"), "rect{fill:#FFF; stroke: red;}\n");
    assert_eq!(
        set("rect {\n  fill: none;\n}\n"),
        "rect {\n  fill: none;\n  stroke: red;\n}\n"
    );
    // The pad stands inside braces; an open root has none.
    assert_eq!(set("rect {}\n"), "rect { stroke: red }\n");
    assert_eq!(
        edit(sheet, "rect { fill: none; stroke: red }\n", |e| e
            .delete_key(&at(&["rect", "fill"]))),
        "rect { stroke: red }\n"
    );
}

#[test]
fn a_bracket_opens_nothing_when_only_maps_are_flow() {
    let (_, sheet) = formats();
    // The root opens with an attribute selector's `[`; it is a block of
    // rules all the same, and the rule under it a flow mapping.
    let src = "[data-x] { fill: none }\nrect { fill: none }\n";
    assert_eq!(
        edit(sheet, src, |e| e
            .set_value(&at(&["[data-x]", "stroke"]), "red")),
        "[data-x] { fill: none; stroke: red }\nrect { fill: none }\n"
    );
    assert_eq!(
        edit(sheet, src, |e| e.delete_key(&at(&["[data-x]"]))),
        "rect { fill: none }\n"
    );
}

#[test]
fn a_block_delete_leaves_a_sibling_on_its_line() {
    let (_, sheet) = formats();
    let src = "a{x:1} b{y:2}\nc{z:3}\n";
    let del = |key: &'static str| edit(sheet, src, |e| e.delete_key(&at(&[key])));
    assert_eq!(del("a"), "b{y:2}\nc{z:3}\n");
    assert_eq!(del("b"), "a{x:1}\nc{z:3}\n");
    assert_eq!(del("c"), "a{x:1} b{y:2}\n");
}

struct NoSeparator;

impl Language for NoSeparator {
    fn describe(&self) -> Description {
        let mut s = syntax();
        s.flow_entry_sep = String::new();
        description("sep-empty", s, &["a: 1"])
    }

    fn parse(&self, dialect: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        Decls.parse(dialect, input)
    }
}

#[test]
fn an_empty_separator_is_refused() {
    let err = fig::language::register(NoSeparator).expect_err("refused");
    assert!(err.to_string().contains("flow_entry_sep"), "{err}");
}
