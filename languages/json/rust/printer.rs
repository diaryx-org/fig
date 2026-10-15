//! Port of languages/json/src/printer.zig.
//!
//! Errors, by Zig name (`Error`): the writer's own, and
//!
//! - `UnresolvedAlias`: JSON cannot represent a YAML alias. A materialized
//!   AST contains none (aliases are expanded to copied subtrees by
//!   `yaml.materialize`), so reaching one here means an unmaterialized YAML
//!   AST was handed to the JSON printer.
//! - `NonStringKey`: a sequence or mapping was used as an object key. See
//!   `key` for what the printer does with the other key kinds and why.
//! - `NonFiniteNumber`: a number whose value is infinity or NaN (`.inf` from
//!   YAML, `inf` from TOML), which JSON and JSONC have no way to write. JSON5
//!   writes it as `Infinity`/`NaN`. A JSON5 `NumberSpecial` scalar is not
//!   this case: it degrades to a quoted string, as it always has.

use fig_kit::ast::{Comment, ExtKind, NodeId, NodeKind, SerializeOptions, Style, AST};
use fig_kit::json_string;
use fig_kit::number as num;
use fig_kit::{err, Result};

/// Which dialect to emit. JSON5 differs from JSON in exactly two places:
/// object keys print unquoted when they are bare identifiers, and the
/// non-finite `NumberSpecial` scalars (`Infinity`/`NaN`) print verbatim
/// rather than degrading to quoted strings. JSONC is plain JSON syntax
/// (quoted keys, normalized numbers) but, like JSON5, carries `//` and
/// `/* */` comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dialect {
    #[default]
    Json,
    Jsonc,
    Json5,
}

pub struct Printer<'w, 'a> {
    writer: &'w mut Vec<u8>,
    ast: &'a AST,
    options: SerializeOptions,
    dialect: Dialect,
}

/// Prints a given document in JSON format.
pub fn print(writer: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    let mut p = Printer { writer, ast, options: *options, dialect: Dialect::Json };
    p.node(ast.root, 0)?;
    p.writer.push(b'\n');
    Ok(())
}

/// Prints the subtree rooted at `id`. Used for partial renders; unlike
/// `print` it adds no trailing newline.
pub fn print_node(writer: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize, options: &SerializeOptions) -> Result<()> {
    let mut p = Printer { writer, ast, options: *options, dialect: Dialect::Json };
    p.node(id, depth)
}

/// `print`, emitting JSON5: unquoted bare-identifier keys and verbatim
/// `Infinity`/`NaN`.
pub fn print5(writer: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    let mut p = Printer { writer, ast, options: *options, dialect: Dialect::Json5 };
    p.leading_comments(ast.leading_comment_anchor(ast.root), 0);
    p.node(ast.root, 0)?;
    p.root_trailing();
    p.writer.push(b'\n');
    Ok(())
}

/// `print_node`, emitting JSON5.
pub fn print_node5(writer: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize, options: &SerializeOptions) -> Result<()> {
    let mut p = Printer { writer, ast, options: *options, dialect: Dialect::Json5 };
    p.node(id, depth)
}

/// `print`, emitting JSONC: plain-JSON syntax with `//`/`/* */` comments.
pub fn printc(writer: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    let mut p = Printer { writer, ast, options: *options, dialect: Dialect::Jsonc };
    p.leading_comments(ast.leading_comment_anchor(ast.root), 0);
    p.node(ast.root, 0)?;
    p.root_trailing();
    p.writer.push(b'\n');
    Ok(())
}

/// `print_node`, emitting JSONC.
pub fn print_nodec(writer: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize, options: &SerializeOptions) -> Result<()> {
    let mut p = Printer { writer, ast, options: *options, dialect: Dialect::Jsonc };
    p.node(id, depth)
}

impl Printer<'_, '_> {
    /// Emit the root's trailing comment — but only when the root is a
    /// scalar. A container root already emitted its own trailing beside its
    /// opening delimiter.
    fn root_trailing(&mut self) {
        let anchor = self.ast.trailing_comment_anchor(self.ast.root);
        if !self.is_container(anchor) {
            self.trailing_comment(anchor);
        }
    }

    fn node(&mut self, id: NodeId, depth: usize) -> Result<()> {
        let n = self.ast.node(id);
        match n.kind {
            NodeKind::Null => self.writer.extend_from_slice(b"null"),
            NodeKind::Boolean(value) => self.writer.extend_from_slice(if value { b"true" } else { b"false" }),
            NodeKind::Number(value) => self.number(value.raw)?,
            // JSON has none of these types. Datetimes and enum literals
            // render as strings (the timestamp / the bare name); a char
            // literal renders as its codepoint number.
            NodeKind::Extended(value) => match value.kind {
                ExtKind::CharLiteral => self.writer.extend_from_slice(value.text),
                // JSON5 has native non-finite floats; JSON must degrade to a
                // string.
                ExtKind::NumberSpecial => {
                    if self.dialect == Dialect::Json5 {
                        self.writer.extend_from_slice(value.text)
                    } else {
                        json_string::write_quoted(self.writer, value.text)
                    }
                }
                _ => json_string::write_quoted(self.writer, value.text),
            },
            NodeKind::String(value) => json_string::write_quoted(self.writer, value),
            NodeKind::Sequence(first_child) => self.container(id, b'[', b']', first_child, depth)?,
            NodeKind::Mapping(first_child) => self.container(id, b'{', b'}', first_child, depth)?,
            NodeKind::KeyValue(kv) => {
                self.key(kv.key)?;
                // Compact output omits the space after the colon.
                self.writer.extend_from_slice(if self.options.pretty { b": " } else { b":" });
                self.node(kv.value, depth)?;
            }
            NodeKind::Alias(_) => return Err(err!(UnresolvedAlias)),
        }
        Ok(())
    }

    /// Render an object key.
    ///
    /// A JSON object key is a string and nothing else, but the AST's key node
    /// can hold any kind: YAML writes `null: a`, `23: x`, `true: x`, and even
    /// `? [a, b] : c`. **The decision, for all three dialects: a non-string
    /// SCALAR key is spelled as the JSON string of its source text —
    /// `"null"`, `"23"`, `"true"` — and a key with no such spelling (a
    /// sequence or a mapping) is refused with `NonStringKey`.** `-o json` is
    /// a *conversion*: a scalar key has one obvious faithful reading, while a
    /// collection key has no reading at all.
    ///
    /// Spelling can collide — a YAML document with both `null:` and
    /// `"null":` emits the same name twice. That is still JSON: RFC 8259
    /// permits a repeated name.
    ///
    /// An `Alias` key stays `UnresolvedAlias`, not `NonStringKey`: like an
    /// alias *value* it means an unmaterialized YAML AST reached a non-YAML
    /// printer.
    ///
    /// Dialect: in JSON5 a *source* string key that is a bare ECMAScript
    /// identifier prints unquoted (`foo: 1`); every spelled scalar key is
    /// quoted in every dialect.
    fn key(&mut self, id: NodeId) -> Result<()> {
        match self.ast.node(id).kind {
            NodeKind::String(s) => {
                if self.dialect == Dialect::Json5 && is_bare_identifier(s) {
                    self.writer.extend_from_slice(s);
                    return Ok(());
                }
                json_string::write_quoted(self.writer, s);
            }
            NodeKind::Null => json_string::write_quoted(self.writer, b"null"),
            NodeKind::Boolean(value) => json_string::write_quoted(self.writer, if value { b"true" } else { b"false" }),
            // The lexeme verbatim rather than the normalized number: the
            // string is the key's source text, not a number being re-spelled.
            NodeKind::Number(value) => json_string::write_quoted(self.writer, value.raw),
            // Every extended scalar's `text` IS its value — the same bytes
            // the value path prints, quoted because this is a key.
            NodeKind::Extended(value) => json_string::write_quoted(self.writer, value.text),
            NodeKind::Alias(_) => return Err(err!(UnresolvedAlias)),
            NodeKind::Sequence(_) | NodeKind::Mapping(_) | NodeKind::KeyValue(_) => return Err(err!(NonStringKey)),
        }
        Ok(())
    }

    /// Render a number. JSON5 keeps the source lexeme where it can read it
    /// back — hex, leading/trailing `.`, and leading `+` are all valid JSON5 —
    /// but NOT `0o`/`0b`, `_`, or a leading zero, which its own tokenizer
    /// rejects; those canonicalize, as does every non-decimal lexeme under
    /// plain JSON. A non-finite number is JSON5's `Infinity`/`NaN`, and
    /// `NonFiniteNumber` under JSON and JSONC.
    fn number(&mut self, raw: &[u8]) -> Result<()> {
        match self.dialect {
            Dialect::Json5 => num::write(self.writer, raw, num::JSON5),
            Dialect::Json | Dialect::Jsonc => num::write(self.writer, raw, num::JSON),
        }
    }

    /// Sequences and mappings differ only in their delimiters and in how each
    /// child renders (a bare node vs. a `key: value`), the latter dispatched
    /// by `node`.
    fn container(&mut self, node_id: NodeId, open: u8, close: u8, first_child: Option<NodeId>, depth: usize) -> Result<()> {
        // Dangling comments (orphans at the end of the body) force the block
        // form so they have somewhere to print; only an empty-and-comment-free
        // container is inline. Comments only print in pretty JSON5/JSONC, so
        // an empty container with dangling comments in a comment-less mode
        // still prints inline.
        let comments = self.ast.comments(node_id);
        let dangling: &[Comment<'_>] = if self.comments_on() { &comments.dangling } else { &[] };
        if first_child.is_none() && dangling.is_empty() {
            self.writer.push(open);
            self.writer.push(close);
            self.trailing_comment(node_id); // empty inline container: `[] // c`
            return Ok(());
        }

        let pretty = self.options.pretty;
        self.writer.push(open);
        // A container's own trailing comment rides the line it opened on, so
        // it sits beside the `[`/`{` (next to its key), not after the distant
        // close.
        self.trailing_comment(node_id);
        if pretty {
            self.writer.push(b'\n');
        }

        let mut current_id = first_child;
        while let Some(id) = current_id {
            self.leading_comments(self.ast.leading_comment_anchor(id), depth + 1);
            if pretty {
                self.write_indent(depth + 1);
            }
            self.node(id, depth + 1)?;
            current_id = self.ast.node(id).next_sibling;
            if current_id.is_some() {
                self.writer.push(b',');
            }
            // A scalar child's trailing prints here; a container child emits
            // its own (beside its opener, above), so skip it to avoid a
            // double / misplacement.
            let anchor = self.ast.trailing_comment_anchor(id);
            if !self.is_container(anchor) {
                self.trailing_comment(anchor);
            }
            if pretty {
                self.writer.push(b'\n');
            }
        }
        self.dangling_comments(dangling, depth + 1);

        if pretty {
            self.write_indent(depth);
        }
        self.writer.push(close);
        Ok(())
    }

    /// Whether `id` is a container node (whose own trailing comment is
    /// emitted beside its opening delimiter, not by its parent).
    fn is_container(&self, id: NodeId) -> bool {
        matches!(self.ast.node(id).kind, NodeKind::Sequence(_) | NodeKind::Mapping(_))
    }

    // ── comments (JSON5 and JSONC only) ─────────────────────────────────────
    // Plain JSON has no comment syntax, so comments are emitted only in the
    // JSON5 and JSONC dialects and only when pretty-printing (a `//` line
    // comment can't survive on a minified single line). Both predicates are
    // checked in the helpers, so callers can invoke them unconditionally.

    /// True when comments may be emitted: a comment-bearing dialect (JSON5 or
    /// JSONC) and multi-line output.
    fn comments_on(&self) -> bool {
        (self.dialect == Dialect::Json5 || self.dialect == Dialect::Jsonc) && self.options.pretty
    }

    /// Emit a node's leading comments, one per line at `depth`.
    fn leading_comments(&mut self, id: NodeId, depth: usize) {
        if !self.comments_on() {
            return;
        }
        let comments = self.ast.comments(id);
        for c in comments.leading.iter() {
            self.write_indent(depth);
            self.write_comment(c);
            self.writer.push(b'\n');
        }
    }

    /// Emit a node's trailing comment (if any) after a leading space, no
    /// newline.
    fn trailing_comment(&mut self, id: NodeId) {
        if !self.comments_on() {
            return;
        }
        if let Some(c) = self.ast.comments(id).trailing {
            self.writer.push(b' ');
            self.write_comment(&c);
        }
    }

    /// Emit a container's dangling comments (end of body), one per line at
    /// `depth`. The caller has already gated on `comments_on` via the passed
    /// slice.
    fn dangling_comments(&mut self, dangling: &[Comment<'_>], depth: usize) {
        for c in dangling {
            self.write_indent(depth);
            self.write_comment(c);
            self.writer.push(b'\n');
        }
    }

    /// Render one comment. JSON5 and JSONC have both forms, so the stored
    /// `style` is honored directly with no degradation.
    fn write_comment(&mut self, c: &Comment<'_>) {
        match c.style {
            Style::Line => {
                self.writer.extend_from_slice(b"//");
                if !c.text.is_empty() {
                    self.writer.push(b' ');
                    self.writer.extend_from_slice(c.text);
                }
            }
            Style::Block => {
                self.writer.extend_from_slice(b"/*");
                if !c.text.is_empty() {
                    self.writer.push(b' ');
                    self.writer.extend_from_slice(c.text);
                    self.writer.push(b' ');
                }
                self.writer.extend_from_slice(b"*/");
            }
        }
    }

    fn write_indent(&mut self, depth: usize) {
        let n = depth * self.options.indent as usize;
        self.writer.resize(self.writer.len() + n, b' ');
    }
}

/// The ASCII subset of an ECMAScript IdentifierName, matching what the JSON5
/// tokenizer accepts unquoted. Reserved words are intentionally allowed
/// (JSON5 permits `while: 1`); only the lexical shape matters.
fn is_bare_identifier(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    for (i, &c) in name.iter().enumerate() {
        let start_ok = c.is_ascii_lowercase() || c.is_ascii_uppercase() || c == b'_' || c == b'$';
        let part_ok = start_ok || c.is_ascii_digit();
        if !(if i == 0 { start_ok } else { part_ok }) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::super::parser::parse_abstract;
    use super::super::Type;
    use super::*;
    use fig_kit::ast::{Builder, Entry, Extended, KeyValue, Node, NodeComments, Number, NumberKind};
    use fig_kit::Error;
    use std::borrow::Cow;

    fn opts(pretty: bool) -> SerializeOptions {
        SerializeOptions { pretty, ..SerializeOptions::default() }
    }

    fn s(out: &[u8]) -> &str {
        std::str::from_utf8(out).unwrap()
    }

    #[test]
    fn prints_json_document() {
        let doc = parse_abstract(br#"{"name":"Ada","tags":["zig",true,null]}"#, Type::JSON).unwrap();
        let mut output = Vec::new();
        print(&mut output, &doc, &SerializeOptions::default()).unwrap();
        assert_eq!("{\n  \"name\": \"Ada\",\n  \"tags\": [\n    \"zig\",\n    true,\n    null\n  ]\n}\n", s(&output));
    }

    #[test]
    fn prints_compact_json_document() {
        let doc = parse_abstract(br#"{"name":"Ada","tags":["zig",true,null],"empty":{}}"#, Type::JSON).unwrap();
        let mut output = Vec::new();
        print(&mut output, &doc, &opts(false)).unwrap();
        assert_eq!("{\"name\":\"Ada\",\"tags\":[\"zig\",true,null],\"empty\":{}}\n", s(&output));
    }

    #[test]
    fn json5_unquoted_keys_infinity_nan_pretty() {
        let doc = parse_abstract(b"{ a: 1, 'b c': 2, while: true, n: NaN, inf: -Infinity }", Type::JSON5).unwrap();
        let mut output = Vec::new();
        print5(&mut output, &doc, &SerializeOptions::default()).unwrap();
        assert_eq!("{\n  a: 1,\n  \"b c\": 2,\n  while: true,\n  n: NaN,\n  inf: -Infinity\n}\n", s(&output));
    }

    #[test]
    fn json5_compact_output() {
        let doc = parse_abstract(b"{a:1,b:[2,Infinity,'x'],$_:3}", Type::JSON5).unwrap();
        let mut output = Vec::new();
        print5(&mut output, &doc, &opts(false)).unwrap();
        assert_eq!("{a:1,b:[2,Infinity,\"x\"],$_:3}\n", s(&output));
    }

    #[test]
    fn json5_round_trips_through_serialize_and_reparse() {
        let doc =
            parse_abstract(b"{ a: .5, b: 0xC8, c: [+1, -Infinity, NaN], 'has space': null, while: 'kw' }", Type::JSON5).unwrap();
        let mut output = Vec::new();
        print5(&mut output, &doc, &opts(false)).unwrap();
        let reparsed = parse_abstract(&output, Type::JSON5).unwrap();
        assert!(doc.eql(&reparsed));
    }

    #[test]
    fn json_dialect_normalizes_json5_number_lexemes_to_valid_json() {
        let doc = parse_abstract(b"[0xFF, -0xa, .5, 5., +15, 1.5e3, -.25]", Type::JSON5).unwrap();
        let mut output = Vec::new();
        print(&mut output, &doc, &opts(false)).unwrap();
        assert_eq!("[255,-10,0.5,5.0,15,1.5e3,-0.25]\n", s(&output));
    }

    #[test]
    fn json_dialect_degrades_infinity_to_a_quoted_string() {
        let doc = parse_abstract(b"Infinity", Type::JSON5).unwrap();
        let mut output = Vec::new();
        // The same extended node, rendered by the plain-JSON dialect.
        print(&mut output, &doc, &opts(false)).unwrap();
        assert_eq!("\"Infinity\"\n", s(&output));
    }

    #[test]
    fn json5_emits_leading_and_trailing_comments_plain_json_drops_them() {
        let mut b = Builder::init();
        // { name: "fig" } with a leading line comment on the entry, a trailing
        // line comment on the value, and a leading block comment on the
        // document.
        let v = b.add_string(b"fig").unwrap();
        b.set_comments(v, &NodeComments { trailing: Some(Comment { text: b"inline", style: Style::Line }), ..Default::default() })
            .unwrap();
        let k = b.add_string(b"name").unwrap();
        let greeting = [Comment { text: b"greeting", style: Style::Line }];
        b.set_comments(k, &NodeComments { leading: Cow::Borrowed(&greeting), ..Default::default() }).unwrap();
        let root = b.add_mapping(&[Entry { key: k, value: v }]).unwrap();
        let doc_c = [Comment { text: b"doc", style: Style::Block }];
        b.set_comments(root, &NodeComments { leading: Cow::Borrowed(&doc_c), ..Default::default() }).unwrap();
        let ast = b.finish(root).unwrap();

        let mut j5 = Vec::new();
        print5(&mut j5, &ast, &SerializeOptions::default()).unwrap();
        assert_eq!("/* doc */\n{\n  // greeting\n  name: \"fig\" // inline\n}\n", s(&j5));

        // Plain JSON has no comment syntax: same AST emits clean JSON.
        let mut j = Vec::new();
        print(&mut j, &ast, &SerializeOptions::default()).unwrap();
        assert_eq!("{\n  \"name\": \"fig\"\n}\n", s(&j));

        // Compact JSON5 also drops comments (a `//` can't survive one line).
        let mut c = Vec::new();
        print5(&mut c, &ast, &opts(false)).unwrap();
        assert_eq!("{name:\"fig\"}\n", s(&c));
    }

    #[test]
    fn json5_a_container_values_trailing_comment_rides_its_opening_bracket() {
        // `key: [ // note` round-trips: the comment stays beside the `[`.
        let input = "{\n  contents: [ // the note\n    \"a\",\n    \"b\"\n  ]\n}";
        let doc = parse_abstract(input.as_bytes(), Type::JSON5).unwrap();
        let mut out = Vec::new();
        print5(&mut out, &doc, &SerializeOptions::default()).unwrap();
        assert_eq!(format!("{input}\n"), s(&out));
    }

    #[test]
    fn json5_opening_line_comment_stays_at_top_closing_line_at_bottom() {
        // A comment on the `]` line is a bottom comment: it normalizes to the
        // last line of the body (before the close).
        let doc =
            parse_abstract(b"{\n  a: [ // top\n    1\n  ],\n  b: [\n    2\n  ] // bottom\n}", Type::JSON5).unwrap();
        let mut out = Vec::new();
        print5(&mut out, &doc, &SerializeOptions::default()).unwrap();
        assert_eq!("{\n  a: [ // top\n    1\n  ],\n  b: [\n    2\n    // bottom\n  ]\n}\n", s(&out));
    }

    #[test]
    fn jsonc_emits_comments_with_quoted_keys_unlike_json5() {
        let mut b = Builder::init();
        let v = b.add_string(b"fig").unwrap();
        b.set_comments(v, &NodeComments { trailing: Some(Comment { text: b"inline", style: Style::Line }), ..Default::default() })
            .unwrap();
        let k = b.add_string(b"name").unwrap();
        let greeting = [Comment { text: b"greeting", style: Style::Block }];
        b.set_comments(k, &NodeComments { leading: Cow::Borrowed(&greeting), ..Default::default() }).unwrap();
        let root = b.add_mapping(&[Entry { key: k, value: v }]).unwrap();
        let ast = b.finish(root).unwrap();

        let mut out = Vec::new();
        printc(&mut out, &ast, &SerializeOptions::default()).unwrap();
        // JSON syntax (quoted key) + JSON5-style comments.
        assert_eq!("{\n  /* greeting */\n  \"name\": \"fig\" // inline\n}\n", s(&out));
    }

    #[test]
    fn honors_custom_indent_width() {
        let doc = parse_abstract(br#"{"a":[1]}"#, Type::JSON).unwrap();
        let mut output = Vec::new();
        print(&mut output, &doc, &SerializeOptions { indent: 4, ..SerializeOptions::default() }).unwrap();
        assert_eq!("{\n    \"a\": [\n        1\n    ]\n}\n", s(&output));
    }

    // ── Mapping keys (see `key`) ──────────────────────────────────────────
    // A JSON object key is a string, but the AST's key node can be any kind.
    // These build the AST literally rather than parsing, because no JSON
    // dialect can *write* the keys under test — they come from YAML.

    /// `{ <key node> : "v" }`, given the key node's kind. Node 0 is the
    /// mapping, 1 the entry, 2 the key, 3 the value.
    fn key_kind_ast(kind: NodeKind<'_>) -> AST {
        let nodes = [
            Node { id: 0, kind: NodeKind::Mapping(Some(1)), next_sibling: None },
            Node { id: 1, kind: NodeKind::KeyValue(KeyValue { key: 2, value: 3 }), next_sibling: None },
            Node { id: 2, kind, next_sibling: None },
            Node { id: 3, kind: NodeKind::String(b"v"), next_sibling: None },
        ];
        AST::from_nodes(0, &nodes).unwrap()
    }

    /// Print `{ <key> : "v" }` compactly in `dialect` and return the bytes,
    /// or the printer's error.
    fn print_key_kind(kind: NodeKind<'_>, dialect: Dialect) -> Result<Vec<u8>> {
        let ast = key_kind_ast(kind);
        let mut out = Vec::new();
        match dialect {
            Dialect::Json => print(&mut out, &ast, &opts(false))?,
            Dialect::Jsonc => printc(&mut out, &ast, &opts(false))?,
            Dialect::Json5 => print5(&mut out, &ast, &opts(false))?,
        }
        Ok(out)
    }

    fn expect_key_kind(kind: NodeKind<'_>, dialect: Dialect, expected: &str) {
        assert_eq!(expected, s(&print_key_kind(kind, dialect).unwrap()));
    }

    fn expect_key_kind_error(kind: NodeKind<'_>, dialect: Dialect, expected: Error) {
        assert_eq!(Err(expected), print_key_kind(kind, dialect));
    }

    #[test]
    fn key_kind_a_string_key_quotes_except_a_json5_bare_identifier() {
        expect_key_kind(NodeKind::String(b"k"), Dialect::Json, "{\"k\":\"v\"}\n");
        expect_key_kind(NodeKind::String(b"k"), Dialect::Jsonc, "{\"k\":\"v\"}\n");
        expect_key_kind(NodeKind::String(b"k"), Dialect::Json5, "{k:\"v\"}\n");
        // Not an identifier: quoted even in JSON5.
        expect_key_kind(NodeKind::String(b"a b"), Dialect::Json5, "{\"a b\":\"v\"}\n");
    }

    #[test]
    fn key_kind_a_null_key_spells_as_the_string_null() {
        expect_key_kind(NodeKind::Null, Dialect::Json, "{\"null\":\"v\"}\n");
        expect_key_kind(NodeKind::Null, Dialect::Jsonc, "{\"null\":\"v\"}\n");
        // Quoted in JSON5 too: a spelled key reads the same in every dialect.
        expect_key_kind(NodeKind::Null, Dialect::Json5, "{\"null\":\"v\"}\n");
    }

    #[test]
    fn key_kind_a_boolean_key_spells_as_true_false() {
        expect_key_kind(NodeKind::Boolean(true), Dialect::Json, "{\"true\":\"v\"}\n");
        expect_key_kind(NodeKind::Boolean(false), Dialect::Json, "{\"false\":\"v\"}\n");
        expect_key_kind(NodeKind::Boolean(true), Dialect::Json5, "{\"true\":\"v\"}\n");
    }

    #[test]
    fn key_kind_a_number_key_spells_as_its_source_lexeme_quoted() {
        expect_key_kind(NodeKind::Number(Number { raw: b"23", kind: NumberKind::Integer }), Dialect::Json, "{\"23\":\"v\"}\n");
        expect_key_kind(
            NodeKind::Number(Number { raw: b"-1.5e3", kind: NumberKind::Float }),
            Dialect::Json,
            "{\"-1.5e3\":\"v\"}\n",
        );
        // The lexeme verbatim, NOT the JSON-normalized number a value would get.
        expect_key_kind(NodeKind::Number(Number { raw: b"0x1F", kind: NumberKind::Integer }), Dialect::Json, "{\"0x1F\":\"v\"}\n");
    }

    #[test]
    fn key_kind_an_extended_scalar_key_spells_as_its_text() {
        expect_key_kind(
            NodeKind::Extended(Extended { kind: ExtKind::LocalDate, text: b"1979-05-27" }),
            Dialect::Json,
            "{\"1979-05-27\":\"v\"}\n",
        );
        // Even in JSON5, where the same scalar as a VALUE prints bare.
        expect_key_kind(
            NodeKind::Extended(Extended { kind: ExtKind::NumberSpecial, text: b"-Infinity" }),
            Dialect::Json5,
            "{\"-Infinity\":\"v\"}\n",
        );
    }

    #[test]
    fn key_kind_an_alias_key_is_unresolved_alias_like_an_alias_value() {
        expect_key_kind_error(NodeKind::Alias(b"a"), Dialect::Json, err!(UnresolvedAlias));
        expect_key_kind_error(NodeKind::Alias(b"a"), Dialect::Json5, err!(UnresolvedAlias));
    }

    #[test]
    fn key_kind_a_sequence_key_is_non_string_key() {
        expect_key_kind_error(NodeKind::Sequence(None), Dialect::Json, err!(NonStringKey));
        expect_key_kind_error(NodeKind::Sequence(None), Dialect::Jsonc, err!(NonStringKey));
        expect_key_kind_error(NodeKind::Sequence(None), Dialect::Json5, err!(NonStringKey));
    }

    #[test]
    fn key_kind_a_mapping_key_is_non_string_key() {
        expect_key_kind_error(NodeKind::Mapping(None), Dialect::Json, err!(NonStringKey));
        expect_key_kind_error(NodeKind::Mapping(None), Dialect::Jsonc, err!(NonStringKey));
        expect_key_kind_error(NodeKind::Mapping(None), Dialect::Json5, err!(NonStringKey));
    }
}
