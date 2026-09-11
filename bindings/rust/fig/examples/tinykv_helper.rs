//! A helper executable: the `tinykv` language (`key=value` lines and `#`
//! comments — the one the core's C probe and `tests/language.rs` host)
//! served over stdio for a `fig` binary to spawn. The whole of a helper is
//! an implementation of [`Language`] and a call to [`fig::helper::serve`];
//! what `fig-lua` does with a Lua script is this with the language read
//! from a file.
//!
//! Point a `languages.figl` at it and it is a format the CLI accepts:
//!
//! ```fig
//! language[]
//! > name = tinykv
//! > extensions = [tkv]
//! > command = [target/debug/examples/tinykv_helper]
//! ```
//!
//! `zig build check` builds and runs it that way (`tools/cli-lang-check.sh`).

use fig::language::{
    CommentForm, CommentRow, CommentSlot, Description, Dialect, Language, LanguageError, NodeKind,
    NodeRow, NodeTable, PrintOptions, Splice, Syntax,
};
use fig::{Capabilities, Span};

struct TinyKv;

impl Language for TinyKv {
    fn describe(&self) -> Description {
        let mut d = Description::new("tinykv");
        d.caps = Capabilities::new(true, true, true);
        d.max_mapping_depth = Some(0);
        d.syntax = Some(Syntax {
            kv_sep: Some("=".into()),
            empty_map_literal: Some("{}".into()),
            flow_containers: false,
            ..Default::default()
        });
        d.dialects = vec![Dialect {
            extensions: vec!["tkv".into()],
            splice: Splice::Raw,
            empty_doc_seed: Some(String::new()),
            ..Dialect::new("tinykv")
        }];
        d.samples = vec!["a=1\nb=two\n".into(), "# top\nk=v\n".into()];
        d
    }

    fn parse(&self, _dialect: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        let src = std::str::from_utf8(input).map_err(|_| LanguageError::new("not UTF-8"))?;
        let mut t = NodeTable::new();
        let root = t.push(NodeRow::new(
            NodeKind::Mapping,
            None,
            Span {
                start: 0,
                end: src.len(),
            },
        ));
        let mut pending: Vec<String> = Vec::new();
        let mut at = 0;
        while at < src.len() {
            let end = src[at..].find('\n').map_or(src.len(), |i| at + i);
            let line = &src[at..end];
            if line.is_empty() {
                at = end + 1;
                continue;
            }
            if let Some(rest) = line.strip_prefix('#') {
                pending.push(rest.trim_start().to_owned());
                at = end + 1;
                continue;
            }
            let eq = line
                .find('=')
                .ok_or_else(|| LanguageError::at("expected key=value", at))?;
            let kv = t.push(
                NodeRow::new(NodeKind::KeyValue, Some(root), Span { start: at, end }).with_sep(
                    Span {
                        start: at + eq,
                        end: at + eq + 1,
                    },
                ),
            );
            let key = t.push(
                NodeRow::new(
                    NodeKind::String,
                    Some(kv),
                    Span {
                        start: at,
                        end: at + eq,
                    },
                )
                .with_text(&line[..eq]),
            );
            t.push(
                NodeRow::new(
                    NodeKind::String,
                    Some(kv),
                    Span {
                        start: at + eq + 1,
                        end,
                    },
                )
                .with_text(&line[eq + 1..]),
            );
            for text in pending.drain(..) {
                t.comments.push(CommentRow {
                    node: key,
                    slot: CommentSlot::Leading,
                    style: CommentForm::Line,
                    text,
                });
            }
            at = end + 1;
        }
        Ok(t)
    }

    fn print(
        &self,
        _dialect: &str,
        table: &NodeTable,
        _options: &PrintOptions,
    ) -> Result<Vec<u8>, LanguageError> {
        let rows = &table.rows;
        // A scalar root is a fragment — the text the editor will splice —
        // and its spelling in tinykv is the text itself.
        if rows.len() == 1 && is_scalar(rows[0].kind) {
            return Ok(rows[0].text.clone().unwrap_or_default().into_bytes());
        }
        let mut out = String::new();
        let mut i = 1;
        while i < rows.len() {
            if i + 2 >= rows.len() || rows[i].kind != NodeKind::KeyValue {
                return Err(LanguageError::new("tinykv holds a flat map of scalars"));
            }
            let (key, val) = (&rows[i + 1], &rows[i + 2]);
            // A value converted in from another format may be a number or a
            // bool; every scalar is written as its text, as dotenv does.
            if key.kind != NodeKind::String || !is_scalar(val.kind) {
                return Err(LanguageError::new("tinykv holds a flat map of scalars"));
            }
            for c in table
                .comments
                .iter()
                .filter(|c| c.node == (i + 1) as u32 && c.slot == CommentSlot::Leading)
            {
                out.push_str("# ");
                out.push_str(&c.text);
                out.push('\n');
            }
            out.push_str(key.text.as_deref().unwrap_or(""));
            out.push('=');
            out.push_str(val.text.as_deref().unwrap_or(""));
            out.push('\n');
            i += 3;
        }
        Ok(out.into_bytes())
    }
}

fn is_scalar(kind: NodeKind) -> bool {
    !matches!(
        kind,
        NodeKind::Sequence | NodeKind::Mapping | NodeKind::KeyValue | NodeKind::Alias
    )
}

fn main() -> std::io::Result<()> {
    fig::helper::serve(TinyKv)
}
