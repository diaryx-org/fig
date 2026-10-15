//! dotenv, written with `fig::kit`: the twin of fig's compiled `dotenv`,
//! row for row, as `@diaryx/fig/js/dotenv` is in JavaScript.
//!
//! * a key is a bash identifier, `[A-Za-z_][A-Za-z0-9_]*`; an `export `
//!   before one is recognized and discarded;
//! * a value is unquoted (trimmed, to the end of the line or to a `#` that
//!   a space or tab precedes), double-quoted (`\n \t \r \\ \"` decoded, may
//!   span lines) or single-quoted (raw, may span lines); a `\r\n` inside a
//!   quoted value reads as `\n`;
//! * a full-line `#` is a comment, leading for the next key or dangling on
//!   the root when no key follows; a `#` after a value on its line is the
//!   value's trailing comment;
//! * a repeated key keeps the first entry's place and takes the last value.
//!
//! Every value is a string. The parse is a description in
//! `fig::kit::grammar`; the print reads the table through
//! `fig::kit::print`.

use std::sync::{Arc, LazyLock};

use fig::Capabilities;
use fig::kit::grammar::{
    self as g, BareOpts, DocOpts, EntryOpts, Grammar, MapOpts, QuotedOpts, TriviaOpts,
};
use fig::kit::print::{Index, Writer};
use fig::language::{
    CommentDelimiter, Comments, Description, Dialect, Language, LanguageError, NativeKinds,
    NodeKind, NodeTable, PrintOptions, Prints, Splice, Syntax,
};

const BARE_CR: &str = "a bare `\\r` must be followed by `\\n`; line endings must be `\\n` or `\\r\\n`";
const UNCLOSED: &str =
    "unclosed quoted value; expected a matching `\"`/`'` before the end of the file";

fn is_key_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_key(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The grammar, built once.
static GRAMMAR: LazyLock<Grammar> = LazyLock::new(|| {
    // Only spaces, a `#` comment or the line's end may follow a closing
    // quote.
    let after: g::AfterFn = Arc::new(|cx, _span| {
        let save = cx.sc.pos;
        cx.sc.hs();
        if !(cx.sc.eof() || cx.sc.starts("#") || cx.sc.starts("\n") || cx.sc.starts("\r")) {
            return Err(cx.fail(
                "unexpected content after this quoted value; only a `#` comment may follow it on the same line",
            ));
        }
        cx.sc.pos = save;
        Ok(None)
    });
    let value = g::choice(vec![
        g::quoted(
            QuotedOpts::new("\"")
                .escapes(&[(b'n', "\n"), (b't', "\t"), (b'r', "\r"), (b'\\', "\\"), (b'"', "\"")])
                .unclosed(UNCLOSED.into())
                .bad_escape(
                    "invalid escape in a double-quoted value; supported: \\n \\t \\r \\\\ \\\" — use a single-quoted value for raw text with backslashes"
                        .into(),
                )
                .bare_cr(BARE_CR.into())
                .after(after.clone()),
        ),
        g::quoted(QuotedOpts::new("'").unclosed(UNCLOSED.into()).bare_cr(BARE_CR.into()).after(after)),
        // Unquoted: to the line's end, or to a `#` that a space or tab
        // precedes — the one right after `=` counts, since `A= #c` is empty.
        g::bare(BareOpts::default().stop(g::when(|sc| {
            sc.starts("#") && matches!(sc.byte(-1), Some(b' ' | b'\t'))
        }))),
    ]);
    let entry = g::entry(
        EntryOpts::new(g::key(g::ident(is_key_start, is_key)))
            // `export` is a prefix only when a key follows it on the line;
            // alone before `=` it is the key.
            .prefix(g::seq(vec![g::lit("export"), g::hs1(), g::ahead(g::ident(is_key_start, |_| false))]))
            .sep(g::lit("="))
            .value(value)
            .missing_sep("expected `=` after this key; every dotenv line is `KEY=value`".into()),
    );
    let trivia = g::trivia(
        TriviaOpts::default()
            .space(g::choice(vec![g::hs1(), g::eol(), g::fail_if(g::lit("\r"), BARE_CR)]))
            .comment(g::comment_line("#")),
    );
    // Where a key should be.
    let otherwise: g::Hook = Arc::new(|cx| {
        if cx.sc.starts("=") {
            return Err(cx.fail("unexpected content here; expected `KEY=value` (optionally `export KEY=value`)"));
        }
        Err(cx.fail("not a valid key here; a dotenv key is a bash identifier (`[A-Za-z_][A-Za-z0-9_]*`)"))
    });
    g::document(
        DocOpts::new(g::map(MapOpts::new(entry).whole(true).trivia(trivia).otherwise(otherwise))).bom(true),
    )
});

fn is_identifier(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty() && is_key_start(b[0]) && b[1..].iter().all(|&c| is_key(c))
}

fn needs_quoting(v: &str) -> bool {
    if v.is_empty() {
        return false;
    }
    let (first, last) = (v.as_bytes()[0], v.as_bytes()[v.len() - 1]);
    if matches!(first, b' ' | b'\t') || matches!(last, b' ' | b'\t') {
        return true;
    }
    v.bytes().any(|c| matches!(c, b'\n' | b'\r' | b'"' | b'\\' | b'#'))
}

fn write_value(w: &mut Writer, ix: &Index<'_>, id: u32) -> Result<(), LanguageError> {
    let row = ix.row(id);
    match row.kind {
        NodeKind::String => {
            let v = ix.text(id);
            if needs_quoting(v) {
                let mut q = String::with_capacity(v.len() + 2);
                q.push('"');
                for ch in v.chars() {
                    match ch {
                        '\n' => q.push_str("\\n"),
                        '\r' => q.push_str("\\r"),
                        '\t' => q.push_str("\\t"),
                        '"' => q.push_str("\\\""),
                        '\\' => q.push_str("\\\\"),
                        c => q.push(c),
                    }
                }
                q.push('"');
                w.put(&q);
            } else {
                w.put(v);
            }
        }
        NodeKind::Int | NodeKind::Float | NodeKind::Bool => {
            w.put(ix.text(id));
        }
        NodeKind::Null => return Err(LanguageError::new("dotenv has no null; a null value cannot be written")),
        NodeKind::Sequence | NodeKind::Mapping => {
            return Err(LanguageError::new(
                "dotenv holds a flat map of strings; a nested value cannot be written",
            ));
        }
        NodeKind::Alias => {
            return Err(LanguageError::new("an alias must be resolved before it is written as dotenv"));
        }
        _ => return Err(LanguageError::new("a row of this kind is not a value")),
    }
    Ok(())
}

/// The language, under a name of its own.
pub struct KitDotenv {
    pub name: String,
}

impl Language for KitDotenv {
    fn describe(&self) -> Description {
        let mut d = Description::new(&self.name);
        d.caps = Capabilities::new(true, true, true);
        // Flat: no mapping inside the root.
        d.max_mapping_depth = Some(0);
        let mut syntax = Syntax::default();
        syntax.comments = Comments::new(Some(CommentDelimiter::open("#")), Some(CommentDelimiter::open("#")));
        syntax.kv_sep = Some("=".into());
        syntax.empty_map_literal = Some("{}".into());
        syntax.flow_containers = false;
        d.syntax = Some(syntax);
        // `env`: the compiled format owns `.env` too, and a compiled
        // format's extension wins, so this is reached by `--lang`.
        let mut dialect = Dialect::new(&self.name);
        dialect.extensions = vec!["env".into()];
        dialect.splice = Splice::Raw;
        dialect.empty_doc_seed = Some(String::new());
        let mut prints = Prints::new(NativeKinds::default());
        prints.typed_scalars = false;
        prints.char_as = "number".into();
        dialect.prints = Some(prints);
        d.dialects = vec![dialect];
        d.samples = vec![
            "A=1\nB=\"two words\"\n".into(),
            "# top\nexport C='raw \\n'\nD=x # trailing\n".into(),
        ];
        d
    }

    fn parse(&self, _dialect: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        GRAMMAR.parse(input)
    }

    fn print(&self, _dialect: &str, t: &NodeTable, options: &PrintOptions) -> Result<Vec<u8>, LanguageError> {
        let ix = Index::new(t);
        let mut w = Writer::new(options);
        let root = ix.root();
        if ix.row(root).kind != NodeKind::Mapping {
            // A fragment: the scalar as it stands alone.
            write_value(&mut w, &ix, root)?;
            return Ok(w.finish().into_bytes());
        }
        for &kv in ix.children(root) {
            let (key, value) = (ix.key(kv), ix.value(kv));
            if ix.row(key).kind != NodeKind::String {
                return Err(LanguageError::new("a dotenv key must be a string"));
            }
            let name = ix.text(key);
            if !is_identifier(name) {
                return Err(LanguageError::new(format!(
                    "`{name}` is not a dotenv key; a key is a bash identifier"
                )));
            }
            w.comments(ix.leading(key), "#", 0);
            w.put(name).put("=");
            write_value(&mut w, &ix, value)?;
            if let Some(c) = ix.trailing(value).first() {
                w.put(" #");
                if !c.text.is_empty() {
                    w.put(" ").put(&c.text.replace('\n', " "));
                }
            }
            w.put("\n");
        }
        w.comments(ix.dangling(root), "#", 0);
        Ok(w.finish().into_bytes())
    }
}
