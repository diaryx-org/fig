//! An insert into a flow mapping that copies its neighbours
//! (`flow_kv_sep_from_siblings`) when a neighbour has nothing to copy. The
//! languages are a small CSS with nesting: a block's member is a declaration,
//! `name: value`, or a rule, a selector over a block of its own, which has no
//! separator after its selector and needs none after its `}`. `nest-sheet`
//! reads rules at its root, and `nest-decls` is a `style` attribute's list.

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

/// The `}` closing the `{` at `open`.
fn close_of(b: &[u8], open: usize, to: usize) -> Result<usize, LanguageError> {
    let mut depth = 0;
    for (i, c) in b.iter().enumerate().take(to).skip(open) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
    }
    Err(LanguageError::at("expected `}`", open))
}

fn string(t: &mut NodeTable, src: &str, parent: u32, from: usize, to: usize) {
    t.push(NodeRow::new(NodeKind::String, Some(parent), sp(from, to)).with_text(&src[from..to]));
}

/// The members in `src[from..to]` as keyvalues under `parent`. What comes
/// before a `{` is a selector, a colon in it included, so `&:hover { b: c }`
/// is a rule; a member with no `{` before its `;` is a declaration.
fn members(
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
        let start = i;
        while i < to && b[i] != b'{' && b[i] != b';' {
            i += 1;
        }
        if i < to && b[i] == b'{' {
            let close = close_of(b, i, to)?;
            let kv = t.push(NodeRow::new(
                NodeKind::KeyValue,
                Some(parent),
                sp(start, close + 1),
            ));
            string(t, src, kv, start, trim_end(b, start, i));
            let block = t.push(NodeRow::new(NodeKind::Mapping, Some(kv), sp(i, close + 1)));
            members(t, src, block, i + 1, close)?;
            i = close + 1;
            continue;
        }
        let colon = src[start..i]
            .find(':')
            .map(|c| start + c)
            .ok_or_else(|| LanguageError::at("expected `:`", start))?;
        let mut value = colon + 1;
        while value < i && b[value].is_ascii_whitespace() {
            value += 1;
        }
        let value_end = trim_end(b, value, i);
        if value_end == value {
            return Err(LanguageError::at("expected a value", value));
        }
        let kv = t.push(NodeRow::new(
            NodeKind::KeyValue,
            Some(parent),
            sp(start, value_end),
        ));
        string(t, src, kv, start, trim_end(b, start, colon));
        string(t, src, kv, value, value_end);
    }
}

fn print_members(rows: &[NodeRow], parent: u32) -> String {
    let text = |r: usize| rows[r].text.as_deref().unwrap_or("");
    let members: Vec<String> = (0..rows.len())
        .filter(|&r| rows[r].parent == Some(parent) && rows[r].kind == NodeKind::KeyValue)
        .map(|r| {
            if rows[r + 2].kind == NodeKind::Mapping {
                let block = print_members(rows, (r + 2) as u32);
                format!("{} {{ {block} }}", text(r + 1))
            } else {
                format!("{}: {}", text(r + 1), text(r + 2))
            }
        })
        .collect();
    members.join("; ")
}

struct Nested {
    name: &'static str,
    flow_root: bool,
    samples: &'static [&'static str],
}

impl Language for Nested {
    fn describe(&self) -> Description {
        let mut s = Syntax::default();
        s.comments = Comments::none();
        s.kv_sep = Some(": ".into());
        s.flow_kv_sep_from_siblings = true;
        s.flow_entry_sep = ";".into();
        s.flow_map_pad = " ".into();
        s.flow_maps_only = true;
        s.flow_root = self.flow_root;
        let mut d = Description::new(self.name);
        d.caps = Capabilities::new(true, true, true);
        d.syntax = Some(s);
        let mut dialect = Dialect::new(self.name);
        dialect.splice = Splice::Raw;
        dialect.empty_doc_seed = Some(String::new());
        d.dialects = vec![dialect];
        d.samples = self.samples.iter().map(|s| s.to_string()).collect();
        d
    }

    fn parse(&self, _: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        let src = std::str::from_utf8(input).map_err(|_| LanguageError::new("not UTF-8"))?;
        let mut t = NodeTable::new();
        let root = t.push(NodeRow::new(NodeKind::Mapping, None, sp(0, src.len())));
        members(&mut t, src, root, 0, src.len())?;
        Ok(t)
    }

    fn print(
        &self,
        _: &str,
        table: &NodeTable,
        _: &PrintOptions,
    ) -> Result<Vec<u8>, LanguageError> {
        let rows = &table.rows;
        if rows.len() == 1 && rows[0].kind != NodeKind::Mapping {
            return Ok(rows[0].text.clone().unwrap_or_default().into_bytes());
        }
        Ok(print_members(rows, 0).into_bytes())
    }
}

fn formats() -> (Format, Format) {
    static FORMATS: OnceLock<(Format, Format)> = OnceLock::new();
    *FORMATS.get_or_init(|| {
        let sheet = fig::language::register(Nested {
            name: "nest-sheet",
            flow_root: false,
            samples: &[".a { b: c; &:hover { d: e } }\n"],
        })
        .expect("sheet registers")[0];
        let decls = fig::language::register(Nested {
            name: "nest-decls",
            flow_root: true,
            samples: &["b: c; &:hover { d: e }"],
        })
        .expect("decls registers")[0];
        (sheet, decls)
    })
}

/// `src` with `x` set to `y` in the block of `.a`.
fn set_in_a(src: &str) -> String {
    let (sheet, _) = formats();
    let mut e = Editor::open(src.as_bytes(), sheet).expect("opens");
    e.set_value(&[Segment::Key(".a"), Segment::Key("x")], "y")
        .unwrap_or_else(|err| panic!("{src:?}: {err}"));
    e.source().expect("source").to_owned()
}

#[test]
fn a_block_opening_with_a_rule_takes_a_declaration_after_it() {
    assert_eq!(
        set_in_a(".a { &:hover { b: c } }"),
        ".a { &:hover { b: c }; x: y }"
    );
    assert_eq!(set_in_a(".a { &:hover{b:c} }"), ".a { &:hover{b:c}; x: y }");
    assert_eq!(
        set_in_a(".a {\n  &:hover { b: c }\n}\n"),
        ".a {\n  &:hover { b: c };\n  x: y\n}\n"
    );
    assert_eq!(
        set_in_a(".a { &:hover { b: c } &:focus { d: e } }"),
        ".a { &:hover { b: c } &:focus { d: e }; x: y }"
    );
}

#[test]
fn a_rule_before_a_declaration_leaves_the_declaration_to_be_copied() {
    // The gap after a rule holds no separator, so the joint is the default.
    assert_eq!(
        set_in_a(".a { &:hover { b: c } z: w }"),
        ".a { &:hover { b: c } z: w; x: y }"
    );
    // The declaration's own `:` is the spelling to copy, past the rule.
    assert_eq!(
        set_in_a(".a { &:hover { b: c } z:w }"),
        ".a { &:hover { b: c } z:w; x:y }"
    );
    // And so is the gap after it, the first that has a member with a
    // separator of its own before it.
    assert_eq!(
        set_in_a(".a { &:hover { b: c } z: w;q: r }"),
        ".a { &:hover { b: c } z: w;q: r;x: y }"
    );
}

#[test]
fn a_block_opening_with_a_declaration_is_copied_as_it_was() {
    assert_eq!(
        set_in_a(".a { z: w; &:hover { b: c } }"),
        ".a { z: w; &:hover { b: c }; x: y }"
    );
    assert_eq!(set_in_a(".a { z:w;q:r }"), ".a { z:w;q:r;x:y }");
}

#[test]
fn a_declaration_list_opening_with_a_rule_takes_a_declaration_after_it() {
    let (_, decls) = formats();
    let mut e = Editor::open(b"&:hover{b:c}", decls).expect("opens");
    e.set_value(&[Segment::Key("a")], "blue").expect("sets");
    assert_eq!(e.source().expect("source"), "&:hover{b:c}; a: blue");
}
