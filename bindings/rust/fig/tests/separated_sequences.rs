//! A sequence whose parser names its separator per container, with or
//! without delimiters, edited item by item: a line of bars joined by `|`
//! (a trailing one allowed), each bar its tokens joined by spaces — the
//! shape of dottune's `.tune`, through the Rust carrier's `NodeRow`.

use std::sync::OnceLock;

use fig::language::{
    Description, Dialect, Language, LanguageError, NodeKind, NodeRow, NodeTable, PrintOptions,
    Splice, Syntax,
};
use fig::{Capabilities, Editor, Format, Segment, Span};

fn sp(start: usize, end: usize) -> Span {
    Span { start, end }
}

/// `src[from..to]` with the spaces at either end left out.
fn trimmed(src: &str, from: usize, to: usize) -> (usize, usize) {
    let s = &src[from..to];
    let start = from + (s.len() - s.trim_start().len());
    (start, start.max(from + s.trim_end().len()))
}

struct Bars;

impl Language for Bars {
    fn describe(&self) -> Description {
        let mut d = Description::new("sep-bars");
        d.caps = Capabilities::new(true, true, true);
        // Never written — the format has no mapping — but a null needs a
        // `render_entry` beside it.
        let mut syntax = Syntax::default();
        syntax.kv_sep = Some(": ".into());
        d.syntax = Some(syntax);
        let mut dialect = Dialect::new("sep-bars");
        dialect.splice = Splice::Raw;
        dialect.empty_doc_seed = Some(String::new());
        d.dialects = vec![dialect];
        d.samples = vec!["1 2 | 3 |\n".into(), "1\n".into()];
        d
    }

    fn parse(&self, _: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        let src = std::str::from_utf8(input).map_err(|_| LanguageError::new("not UTF-8"))?;
        let line = src.trim_end_matches('\n');
        let mut t = NodeTable::new();
        let (start, end) = trimmed(src, 0, line.len());
        let root = t.push(
            NodeRow::new(NodeKind::Sequence, None, sp(start, end)).with_item_sep(" | ", true),
        );
        let mut segments: Vec<(usize, usize)> = Vec::new();
        let mut from = 0;
        for (i, c) in line.char_indices() {
            if c == '|' {
                segments.push((from, i));
                from = i + 1;
            }
        }
        segments.push((from, line.len()));
        if segments.len() > 1 && line[from..].trim().is_empty() {
            segments.pop();
        }
        for (from, to) in segments {
            let (start, end) = trimmed(src, from, to);
            let bar = t.push(
                NodeRow::new(NodeKind::Sequence, Some(root), sp(start, end))
                    .with_item_sep(" ", true),
            );
            let mut i = start;
            for word in src[start..end].split(' ') {
                if word == "x" {
                    return Err(LanguageError::at("x is not a note", i));
                }
                if !word.is_empty() {
                    t.push(
                        NodeRow::new(NodeKind::String, Some(bar), sp(i, i + word.len()))
                            .with_text(word),
                    );
                }
                i += word.len() + 1;
            }
        }
        Ok(t)
    }

    /// A token for a fragment the editor splices (the value edits spell
    /// their value through here); a line of bars for a whole table.
    fn print(
        &self,
        _: &str,
        table: &NodeTable,
        _: &PrintOptions,
    ) -> Result<Vec<u8>, LanguageError> {
        let rows = &table.rows;
        if rows[0].kind != NodeKind::Sequence {
            return Ok(rows[0].text.clone().unwrap_or_default().into_bytes());
        }
        let kids = |p: usize| (0..rows.len()).filter(move |&i| rows[i].parent == Some(p as u32));
        let bars: Vec<String> = kids(0)
            .map(|b| {
                kids(b)
                    .map(|t| rows[t].text.clone().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        Ok(format!("{} |\n", bars.join(" | ")).into_bytes())
    }
}

fn format() -> Format {
    static FORMAT: OnceLock<Format> = OnceLock::new();
    *FORMAT.get_or_init(|| fig::language::register(Bars).expect("bars registers")[0])
}

fn edit(src: &str, op: impl FnOnce(&mut Editor) -> Result<(), fig::Error>) -> String {
    let mut e = Editor::open(src.as_bytes(), format()).expect("opens");
    op(&mut e).unwrap_or_else(|err| panic!("{src:?}: {err}"));
    e.source().expect("source").to_owned()
}

const LINE: &str = "1 2 3 4 | 5 6 7 1' |\n";

#[test]
fn bars_take_an_item_at_either_end_and_between_and_keep_the_trailing_bar() {
    assert_eq!(
        edit(LINE, |e| e.append_value(&[], "1 - - -")),
        "1 2 3 4 | 5 6 7 1' | 1 - - - |\n"
    );
    assert_eq!(
        edit(LINE, |e| e.prepend_value(&[], "0")),
        "0 | 1 2 3 4 | 5 6 7 1' |\n"
    );
    assert_eq!(
        edit(LINE, |e| e.insert_item(&[], 1, "9")),
        "1 2 3 4 | 9 | 5 6 7 1' |\n"
    );
    assert_eq!(edit("1 | 2\n", |e| e.append_value(&[], "3")), "1 | 2 | 3\n");
}

#[test]
fn a_delete_takes_one_item_and_one_separator() {
    assert_eq!(edit(LINE, |e| e.delete_item(&[], 0)), "5 6 7 1' |\n");
    assert_eq!(edit(LINE, |e| e.delete_item(&[], 1)), "1 2 3 4 |\n");
    assert_eq!(
        edit(LINE, |e| e.delete_item(&[Segment::Index(0)], 1)),
        "1 3 4 | 5 6 7 1' |\n"
    );
    assert_eq!(
        edit(LINE, |e| e.delete_item(&[Segment::Index(1)], 3)),
        "1 2 3 4 | 5 6 7 |\n"
    );
}

#[test]
fn a_move_keeps_each_slot_s_separator() {
    assert_eq!(
        edit(LINE, |e| e.move_item(&[], 1, 0)),
        "5 6 7 1' | 1 2 3 4 |\n"
    );
    assert_eq!(
        edit(LINE, |e| e.move_item(&[Segment::Index(0)], 0, 3)),
        "2 3 4 1 | 5 6 7 1' |\n"
    );
}

#[test]
fn an_edit_the_language_rejects_is_rolled_back() {
    let mut e = Editor::open(LINE.as_bytes(), format()).unwrap();
    assert!(e.append_value(&[Segment::Index(0)], "x").is_err());
    assert!(e.insert_item(&[], 1, "x").is_err());
    assert_eq!(e.source().unwrap(), LINE);
    assert!(matches!(
        e.insert_item(&[], 5, "1"),
        Err(fig::Error::NotFound)
    ));
}
