//! Port of src/canonical/printer.zig.
//!
//! The canonical-form printer: a total, 1:1 text encoding of the AST.
//! (Formerly "native"; the human-facing `fig` authoring dialect is separate — see
//! languages/figl/DESIGN.md.)
//!
//! Unlike the format printers (JSON/YAML/TOML/ZON), this one is a *bijection*
//! with the AST — every `NodeKind` arm and the YAML reference layer (anchors,
//! tags, aliases) has an unambiguous surface form, so any AST round-trips
//! through it unchanged. It is the default/debug representation and the
//! comparison oracle: canonicalize two documents to canonical text and `strcmp`.
//!
//! Grammar (informal):
//!   node      ::= prefix* value
//!   prefix    ::= '&' name  |  '!' tagtext        (anchor / tag, space-separated)
//!   value     ::= 'null' | 'true' | 'false'
//!               | string                          ("…", JSON escapes)
//!               | number                          (raw lexeme, see `number`)
//!               | '@' extkind ' ' string          (extended scalar)
//!               | '[' (node (',' node)*)? ']'     (sequence)
//!               | '{' (node ':' node (',' …)*)? '}'  (mapping; keys are nodes)
//!               | '*' name                        (alias)
//!
//! A `number`'s `kind` (integer vs float) is normally implied by its lexeme. On
//! the rare node whose stored kind disagrees with the lexeme (constructible via
//! `Builder::add_number_raw`), a `~i`/`~f` sigil pins it. Common data never
//! triggers it.
//!
//! The canonical encoding is total over every AST *node kind* — it rejects no
//! variant. The only failure is `NestingTooDeep`, a recursion guard: a
//! pathologically nested AST (e.g. a fuzzer feeding the oracle, or a
//! hand-built `Builder` tree) would otherwise overflow the stack. `MAX_DEPTH`
//! matches the canonical parser's guard, so any AST the parser accepts prints
//! without hitting it. What was written before a failure stays in `out`.

use crate::err;
use crate::kit::ast::{Comment, NodeId, NodeKind, Number, NumberKind, Style, Tag, AST};
use crate::kit::json_string;
use crate::Result;

/// Maximum container-nesting depth, shared with `canonical/parser.rs`. Bounds the
/// recursion in both directions of the bijection.
pub const MAX_DEPTH: usize = 512;

/// Spaces per indentation level. The canonical encoding is an oracle —
/// one document has exactly one spelling — so this is a fixed constant, not a
/// knob: a configurable canonical form would defeat the comparison oracle.
const INDENT_WIDTH: usize = 2;

struct Printer<'a> {
    out: &'a mut Vec<u8>,
    ast: &'a AST,
}

/// Print the whole AST into `out`, with a trailing newline.
pub fn print(out: &mut Vec<u8>, ast: &AST) -> Result<()> {
    let mut p = Printer { out, ast };
    p.leading_comments(ast.leading_comment_anchor(ast.root), 0);
    p.node(ast.root, 0)?;
    // A block container root emitted its own trailing beside its opening
    // delimiter; a scalar root's, or an inline `{}`/`[]` root's, goes here.
    if !p.is_block_container(ast.root) {
        p.trailing_comment(ast.root);
    }
    // A container root emits its own dangling run inside its closing delimiter
    // (see `container`); a non-container root has no body for EOF orphans to
    // live in, so they print here, one per line, after the value.
    if !p.is_container(ast.root) {
        for c in ast.comments(ast.root).dangling.iter() {
            p.out.push(b'\n');
            p.write_comment(c);
        }
    }
    p.out.push(b'\n');
    Ok(())
}

/// Print the subtree rooted at `id`. Adds no trailing newline (used for
/// partial renders and by `print`).
pub fn print_node(out: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize) -> Result<()> {
    let mut p = Printer { out, ast };
    p.node(id, depth)
}

impl Printer<'_> {
    fn node(&mut self, id: NodeId, depth: usize) -> Result<()> {
        self.prefixes(id);
        match self.ast.kind_of(id) {
            NodeKind::Null => self.out.extend_from_slice(b"null"),
            NodeKind::Boolean(value) => self.out.extend_from_slice(if value { b"true" } else { b"false" }),
            NodeKind::Number(value) => self.number(value),
            NodeKind::String(value) => json_string::write_quoted(self.out, value),
            NodeKind::Extended(value) => {
                self.out.push(b'@');
                self.out.extend_from_slice(value.kind.name().as_bytes());
                self.out.push(b' ');
                json_string::write_quoted(self.out, value.text);
            }
            NodeKind::Alias(name) => {
                self.out.push(b'*');
                self.out.extend_from_slice(name);
            }
            NodeKind::Sequence(first_child) => self.container(id, b'[', b']', first_child, depth)?,
            NodeKind::Mapping(first_child) => self.container(id, b'{', b'}', first_child, depth)?,
            NodeKind::KeyValue(kv) => {
                self.node(kv.key, depth)?;
                self.out.extend_from_slice(b": ");
                // A comment between the `:` and the value is the value's leading
                // comment (the parser's `claim_leading(value)`), and it stays
                // there: a block comment on the line, a line comment ending it
                // with the value on the next. An entry's leading comments proper
                // are on its key, written by the container above the line.
                for c in self.ast.comments(kv.value).leading.iter() {
                    self.write_comment(c);
                    match c.style {
                        Style::Block => self.out.push(b' '),
                        Style::Line => {
                            self.out.push(b'\n');
                            self.write_indent(depth + 1);
                        }
                    }
                }
                self.node(kv.value, depth)?;
            }
        }
        Ok(())
    }

    /// Emit the YAML reference-layer prefixes attached to this node id: an anchor
    /// (`&name `) and/or a tag (`!!str `). Both are stored in side-tables that are
    /// empty for non-YAML documents.
    fn prefixes(&mut self, id: NodeId) {
        let a = self.ast;
        if let Some(name) = a.anchor_of(id) {
            self.out.push(b'&');
            self.out.extend_from_slice(name);
            self.out.push(b' ');
        }
        // A `.text` tag prints verbatim (leading `!` included, e.g. `!!str`); a
        // normalized `.kind` tag (fig/builder origin) prints as its core shorthand.
        if let Some(tag) = a.tag_of(id) {
            let spelled: &[u8] = match tag {
                Tag::Text(t) => t,
                Tag::Kind(_) => tag.spelling(),
            };
            self.out.extend_from_slice(spelled);
            self.out.push(b' ');
        }
    }

    /// Render a number's raw lexeme verbatim. If the lexeme's implied kind disagrees
    /// with the stored kind, prefix `~i`/`~f` so the parser can restore it exactly.
    fn number(&mut self, value: Number<'_>) {
        if implied_number_kind(value.raw) != value.kind {
            self.out.extend_from_slice(if value.kind == NumberKind::Float { b"~f" } else { b"~i" });
        }
        self.out.extend_from_slice(value.raw);
    }

    /// Sequences and mappings differ only in delimiters and in how each child
    /// renders (a bare node vs. a `key: value`), the latter handled by `node`.
    fn container(&mut self, node_id: NodeId, open: u8, close: u8, first_child: Option<NodeId>, depth: usize) -> Result<()> {
        let dangling = self.ast.comments(node_id).dangling;
        // Only a truly empty container (no children, no trailing orphan comments)
        // prints inline; otherwise it opens a block so the dangling run has a home.
        // An inline one's trailing comment is written by whoever writes the line
        // — the parent, after the comma, as for a scalar — so that `[] // c,`
        // never happens, which the parser would read as a comment `c,`.
        if first_child.is_none() && dangling.is_empty() {
            self.out.push(open);
            self.out.push(close);
            return Ok(());
        }
        // Guard the recursion below; the inline fast path above never descends.
        if depth >= MAX_DEPTH {
            return Err(err!(NestingTooDeep));
        }
        self.out.push(open);
        // The container's own trailing comment rides the line it opened on.
        self.trailing_comment(node_id);
        self.out.push(b'\n');
        let mut current_id = first_child;
        while let Some(id) = current_id {
            // A mapping child is a `keyvalue`: its leading comment sits above the
            // key, its trailing comment after the value. A sequence child is the
            // value node itself, so both anchors collapse to `id`.
            self.leading_comments(self.ast.leading_comment_anchor(id), depth + 1);
            self.write_indent(depth + 1);
            self.node(id, depth + 1)?;
            current_id = self.ast.node(id).next_sibling;
            if current_id.is_some() {
                self.out.push(b',');
            }
            // A block container child emitted its own trailing beside its opener;
            // a scalar's or an inline container's goes here, after the comma.
            let anchor = self.ast.trailing_comment_anchor(id);
            if !self.is_block_container(anchor) {
                self.trailing_comment(anchor);
            }
            self.out.push(b'\n');
        }
        // Comments dangling at the end of the body (after the last child, or the
        // entire body of an otherwise-empty container).
        for c in dangling.iter() {
            self.write_indent(depth + 1);
            self.write_comment(c);
            self.out.push(b'\n');
        }
        self.write_indent(depth);
        self.out.push(close);
        Ok(())
    }

    /// Whether `id` is a container node.
    fn is_container(&self, id: NodeId) -> bool {
        matches!(self.ast.kind_of(id), NodeKind::Sequence(_) | NodeKind::Mapping(_))
    }

    /// Whether `id` is a container that prints as a block — one with children or
    /// dangling comments — and so emits its own trailing comment beside its
    /// opening delimiter, rather than leaving it to whoever writes its line.
    fn is_block_container(&self, id: NodeId) -> bool {
        let first_child = match self.ast.kind_of(id) {
            NodeKind::Sequence(first) | NodeKind::Mapping(first) => first,
            _ => return false,
        };
        first_child.is_some() || !self.ast.comments(id).dangling.is_empty()
    }

    /// Emit a node's leading comments, one per line at `depth`, each terminated by a
    /// newline (so the node's own indented line follows).
    fn leading_comments(&mut self, id: NodeId, depth: usize) {
        for c in self.ast.comments(id).leading.iter() {
            self.write_indent(depth);
            self.write_comment(c);
            self.out.push(b'\n');
        }
    }

    /// Emit a node's trailing comment (if any) after a leading space. No newline —
    /// the caller closes the line.
    fn trailing_comment(&mut self, id: NodeId) {
        if let Some(c) = self.ast.comments(id).trailing {
            self.out.push(b' ');
            self.write_comment(&c);
        }
    }

    /// Render one comment in native syntax: `// text` for a line comment, `/* text
    /// */` for a block. Mirrors the marker set the canonical parser accepts.
    fn write_comment(&mut self, c: &Comment<'_>) {
        match c.style {
            Style::Line => {
                self.out.extend_from_slice(b"//");
                if !c.text.is_empty() {
                    self.out.push(b' ');
                    self.out.extend_from_slice(c.text);
                }
            }
            Style::Block => {
                self.out.extend_from_slice(b"/*");
                if !c.text.is_empty() {
                    self.out.push(b' ');
                    self.out.extend_from_slice(c.text);
                    self.out.push(b' ');
                }
                self.out.extend_from_slice(b"*/");
            }
        }
    }

    fn write_indent(&mut self, depth: usize) {
        self.out.resize(self.out.len() + depth * INDENT_WIDTH, b' ');
    }
}

/// Classify a numeric lexeme the same way the JSON parser's `getNumber` does, so
/// a bare number round-trips to the same `kind`. Hex (`0x…`) is an integer; a
/// dot or an `e`/`E` exponent makes it a float; otherwise integer. Total (never
/// errors): a malformed lexeme that can't come from a printer is treated as the
/// nearest of the two.
pub fn implied_number_kind(raw: &[u8]) -> NumberKind {
    let body = if !raw.is_empty() && (raw[0] == b'+' || raw[0] == b'-') { &raw[1..] } else { raw };
    if body.len() >= 2 && body[0] == b'0' && (body[1] == b'x' || body[1] == b'X') {
        return NumberKind::Integer;
    }
    if raw.contains(&b'.') {
        return NumberKind::Float;
    }
    if raw.iter().any(|&c| c == b'e' || c == b'E') {
        return NumberKind::Float;
    }
    NumberKind::Integer
}

// Strings are quoted/escaped by the shared `json_string::write_quoted` (the native
// parser's escape decoder accepts exactly that set).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::parser::parse_abstract;
    use crate::kit::ast::{Builder, Entry, NodeComments};
    use std::borrow::Cow;

    fn printed(ast: &AST) -> String {
        let mut out = Vec::new();
        print(&mut out, ast).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn prints_scalars_sequences_and_mappings_1_1() {
        let ast = parse_abstract(br#"{ "name": "fig", "port": 8080, "ratio": 1.0, "tags": ["a", true, null] }"#).unwrap();
        assert_eq!(
            "{\n  \"name\": \"fig\",\n  \"port\": 8080,\n  \"ratio\": 1.0,\n  \"tags\": [\n    \"a\",\n    true,\n    null\n  ]\n}\n",
            printed(&ast)
        );
    }

    #[test]
    fn prints_empty_containers_inline() {
        let ast = parse_abstract(b"{ \"a\": [], \"b\": {} }").unwrap();
        assert_eq!("{\n  \"a\": [],\n  \"b\": {}\n}\n", printed(&ast));
    }

    #[test]
    fn emits_leading_and_trailing_comments() {
        let mut b = Builder::init();
        // { "name": "fig" } with a leading comment on the entry and a trailing
        // comment on its value, plus a block comment leading the whole document.
        let v_name = b.add_string(b"fig").unwrap();
        b.set_comments(v_name, &NodeComments { trailing: Some(Comment { text: b"inline", style: Style::Line }), ..Default::default() })
            .unwrap();
        let k_name = b.add_string(b"name").unwrap();
        b.set_comments(k_name, &NodeComments { leading: Cow::Owned(vec![Comment { text: b"greeting", style: Style::Line }]), ..Default::default() })
            .unwrap();
        let root = b.add_mapping(&[Entry { key: k_name, value: v_name }]).unwrap();
        b.set_comments(root, &NodeComments { leading: Cow::Owned(vec![Comment { text: b"doc", style: Style::Block }]), ..Default::default() })
            .unwrap();
        let ast = b.finish(root).unwrap();
        assert_eq!("/* doc */\n{\n  // greeting\n  \"name\": \"fig\" // inline\n}\n", printed(&ast));
    }

    #[test]
    fn an_empty_containers_trailing_comment_follows_its_comma() {
        let ast = parse_abstract(b"{ \"a\": {}, // e\n  \"b\": [], \"c\": 1 }").unwrap();
        assert_eq!("{\n  \"a\": {}, // e\n  \"b\": [],\n  \"c\": 1\n}\n", printed(&ast));
    }

    #[test]
    fn a_values_leading_comment_stays_between_the_colon_and_the_value() {
        let ast = parse_abstract(b"{ \"a\": /* b */ 1, \"c\": // l\n 2 }").unwrap();
        assert_eq!("{\n  \"a\": /* b */ 1,\n  \"c\": // l\n    2\n}\n", printed(&ast));
    }

    #[test]
    fn bounds_nesting_depth() {
        // Build `levels` nested one-element sequences around a null.
        fn nest(levels: usize) -> AST {
            let mut b = Builder::init();
            // Innermost holds a scalar so it isn't the empty-container fast path
            // (which prints inline and never recurses, escaping the guard).
            let n = b.add_null().unwrap();
            let mut id = b.add_sequence(&[n]).unwrap(); // 1 level
            for _ in 1..levels {
                id = b.add_sequence(&[id]).unwrap();
            }
            b.finish(id).unwrap()
        }
        // Exactly `MAX_DEPTH` levels prints; one deeper is rejected, not a crash.
        let ok = nest(MAX_DEPTH);
        let mut out = Vec::new();
        print(&mut out, &ok).unwrap();
        let too_deep = nest(MAX_DEPTH + 1);
        assert_eq!(err!(NestingTooDeep), print(&mut out, &too_deep).unwrap_err());
    }
}
