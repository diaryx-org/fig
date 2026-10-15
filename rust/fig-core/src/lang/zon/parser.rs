//! Port of languages/zon/src/parser.zig.
//!
//! ZON (Zig Object Notation) parser.
//!
//! Unlike the JSON/YAML parsers, this one hand-rolls NO tokenizer. ZON is a
//! subset of Zig source, so we lean on the Zig standard library's own
//! parser: `std.zig.Ast.parse(.zon)` produces a lossless syntax tree — token
//! spans and raw source bytes preserved — which we walk into fig's AST. This
//! is the one format where the stdlib carries the lexing/parsing for us,
//! precisely because `std.zig.Ast` is a formatter-grade *concrete* syntax
//! tree (it backs `zig fmt`), so it keeps the source fidelity fig's
//! Document/span model needs. (In Rust that tokenizer and parser are ported
//! alongside: `zig_tokenizer.rs`, `zig_parse.rs`, `zig_string_literal.rs`.)
//!
//! Enum and char literals — types the abstract model has no plain variant
//! for — are preserved as `extended` nodes (`ExtKind::EnumLiteral` /
//! `CharLiteral`), so they round-trip through ZON losslessly. A non-ZON
//! printer renders them best-effort: an enum literal as a string (`"foo"`), a
//! char as its codepoint.
//!
//! Remaining best-effort, niche behavior:
//!   * `inf` / `nan` become float numbers with verbatim raw text; a JSON
//!     printer will then emit non-conformant `inf`/`nan` tokens.
//!   * Non-decimal / underscored numbers (`0x1F`, `1_000`) keep their raw
//!     text, which is valid ZON but NOT valid JSON when converted.
//!
//! This parser is coupled to the Zig compiler's internal `std.zig.Ast` API,
//! which shifts between releases — the deliberate trade for the cheap impl.
//!
//! Errors: `InvalidZon` (the Zig parser found an error, or a literal does
//! not decode), `UnsupportedZon` (a Zig expression ZON has no value for).

use std::sync::Arc;

use super::zig_parse::{self, Kind, NodeIndex, TokenIndex};
use super::zig_string_literal;
use super::Type;
use crate::err;
use crate::kit::ast::{Comment, ExtKind, Extended, KeyValue, NodeId, NodeKind, Number, NumberKind, AST};
use crate::kit::document::Document;
use crate::kit::row_writer::RowWriter;
use crate::kit::span::Span;
use crate::kit::table::{self, Table};
use crate::Result;

struct Parser<'t> {
    /// The original input. AST string/number payloads that point into
    /// source slice this (the table's own, so they stay in the source).
    source: Arc<[u8]>,
    tree: zig_parse::Ast,
    /// The table being written. A decoded string is copied into it by
    /// `w.add`.
    w: RowWriter<'t>,
    // Comment layer. `std.zig.Ast` discards `//` comments (its tokenizer
    // treats them as whitespace), so they're recovered by byte-scanning the
    // source *gaps* between nodes — `absorb_comments_up_to(pos)` scans
    // `[scan_pos, pos)`, which holds only punctuation/whitespace/comments
    // (never a string or char literal, since those are values the walk jumps
    // `scan_pos` over). A comment on the same line as a just-finished value
    // trails it; one on its own line buffers as leading.
    scan_pos: usize,
    /// The pending leading comments, by their text's span in `source`.
    pending_leading: Vec<Span>,
    last_value_id: Option<NodeId>,
}

/// `input` as an `AST`, alone.
pub fn parse_abstract(input: &[u8], format: Type) -> Result<AST> {
    Ok(parse(input, format)?.ast)
}

/// `input` as a `Document`: its table written and adopted.
pub fn parse(input: &[u8], format: Type) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_into(&mut t, format)?;
    RowWriter::adopt(source, &mut t)
}

/// Write the rows of `t`'s source into `t`, the table fig hands a parse.
pub fn parse_into(t: &mut Table, format: Type) -> Result<()> {
    let _ = format;
    let source = t.source_arc();

    let tree = zig_parse::parse_zon(&source).map_err(|_| err!(InvalidZon))?;
    let root_decl = tree.root;

    let mut p = Parser {
        source: source.clone(),
        tree,
        w: RowWriter::init(t),
        scan_pos: 0,
        pending_leading: Vec::new(),
        last_value_id: None,
    };

    // A leading comment block above the whole document binds to the root (a
    // container root passes it through to its first key inside `walk`).
    let start = p.node_span(root_decl).start;
    p.absorb_comments_up_to(start)?;
    let root = p.walk(root_decl)?;
    p.claim_leading(root)?;
    p.last_value_id = Some(root);
    p.scan_pos = p.w.span_of(root).end;
    p.absorb_comments_up_to(source.len())?; // a trailing comment after the root

    p.w.finish(root)
}

impl<'t> Parser<'t> {
    // =========
    // THE WALK
    // =========

    fn walk(&mut self, node: NodeIndex) -> Result<NodeId> {
        match &self.tree.node(node).kind {
            Kind::NumberLiteral => self.number(node),
            Kind::CharLiteral => self.char_literal(node),
            Kind::Identifier => self.identifier(node),
            Kind::EnumLiteral => self.enum_literal(node),
            Kind::StringLiteral | Kind::MultilineStringLiteral => self.string_literal(node),
            Kind::Negation(_) => self.negation(node),
            // ZON forbids parenthesized grouping, but the parser still builds
            // the node; transparently descend (best-effort).
            Kind::GroupedExpression(child) => self.walk(*child),
            Kind::ArrayInit { .. } | Kind::StructInit { .. } => self.container(node),
            _ => Err(err!(UnsupportedZon)),
        }
    }

    /// Dispatch an init node to a sequence (array literal) or mapping
    /// (struct literal). An empty `.{}` parses as an empty struct init →
    /// empty mapping.
    fn container(&mut self, node: NodeIndex) -> Result<NodeId> {
        match self.tree.node(node).kind.clone() {
            Kind::ArrayInit { type_expr, elements } => {
                if type_expr.is_some() {
                    return Err(err!(UnsupportedZon)); // `T{...}` typed init
                }
                self.sequence(node, &elements)
            }
            Kind::StructInit { type_expr, fields } => {
                if type_expr.is_some() {
                    return Err(err!(UnsupportedZon));
                }
                self.mapping(node, &fields)
            }
            _ => Err(err!(UnsupportedZon)),
        }
    }

    fn sequence(&mut self, node: NodeIndex, elements: &[NodeIndex]) -> Result<NodeId> {
        let span = self.node_span(node);
        let seq_id = self.add_node(NodeKind::Sequence(None), span)?;
        self.capture_open_trailing(seq_id, node)?;
        let mut first: Option<NodeId> = None;
        let mut prev: Option<NodeId> = None;
        for &elem in elements {
            let at = self.tree.token_start(self.tree.node(elem).first_token);
            self.absorb_comments_up_to(at)?;
            let child_id = self.walk(elem)?;
            // A scalar element claims its own leading; a container element
            // passed it through to its first key inside `walk` (so this is
            // then a no-op).
            self.claim_leading(child_id)?;
            self.last_value_id = Some(child_id);
            self.scan_pos = self.w.span_of(child_id).end;
            match prev {
                Some(p) => self.w.set_next(p, Some(child_id)),
                None => first = Some(child_id),
            }
            prev = Some(child_id);
        }
        self.absorb_comments_up_to(span.end)?; // trailing on the last element
        self.claim_dangling(seq_id)?; // own-line orphans before the closing brace
        self.w.set(seq_id, NodeKind::Sequence(first))?;
        Ok(seq_id)
    }

    fn mapping(&mut self, node: NodeIndex, fields: &[NodeIndex]) -> Result<NodeId> {
        let span = self.node_span(node);
        let map_id = self.add_node(NodeKind::Mapping(None), span)?;
        self.capture_open_trailing(map_id, node)?;
        let mut first: Option<NodeId> = None;
        let mut prev: Option<NodeId> = None;
        for &field in fields {
            // `.name = value`: the value's first token is `value`; stepping
            // back two tokens lands on the `name` identifier (skipping the
            // `=`).
            let name_token = self.tree.node(field).first_token - 2;
            // Leading comments above the entry bind to the key.
            self.absorb_comments_up_to(self.tree.token_start(name_token))?;
            let key_id = self.field_name(name_token)?;
            self.claim_leading(key_id)?;
            // Don't re-scan the key name / `=` while walking the value.
            self.scan_pos = self.w.span_of(key_id).end;
            let value_id = self.walk(field)?;
            self.last_value_id = Some(value_id);
            self.scan_pos = self.w.span_of(value_id).end;
            let key_span = self.w.span_of(key_id);
            let value_span = self.w.span_of(value_id);
            let kv_id =
                self.add_node(NodeKind::KeyValue(KeyValue { key: key_id, value: value_id }), Span::new(key_span.start, value_span.end))?;
            match prev {
                Some(p) => self.w.set_next(p, Some(kv_id)),
                None => first = Some(kv_id),
            }
            prev = Some(kv_id);
        }
        self.absorb_comments_up_to(span.end)?; // trailing on the last entry
        self.claim_dangling(map_id)?; // own-line orphans before the closing brace
        self.w.set(map_id, NodeKind::Mapping(first))?;
        Ok(map_id)
    }

    // =========
    // SCALARS
    // =========

    fn number(&mut self, node: NodeIndex) -> Result<NodeId> {
        let span = self.node_span(node);
        let source = self.source.clone();
        let raw = &source[span.start..span.end];
        self.add_node(NodeKind::Number(Number { raw, kind: classify_number(raw) }), span)
    }

    fn negation(&mut self, node: NodeIndex) -> Result<NodeId> {
        let Kind::Negation(child) = self.tree.node(node).kind else { unreachable!() };
        let span = self.node_span(node); // covers the `-` through the operand
        match self.tree.node(child).kind {
            // `-123`, `-0x1.5p3`: raw text (incl. the sign) is preserved
            // verbatim.
            Kind::NumberLiteral => {
                let source = self.source.clone();
                let raw = &source[span.start..span.end];
                self.add_node(NodeKind::Number(Number { raw, kind: classify_number(raw) }), span)
            }
            // `-inf` is the only other legal negation in ZON.
            Kind::Identifier => {
                let ident = self.token_slice(self.tree.node(child).main_token);
                if ident == b"inf" {
                    return self.add_node(NodeKind::Number(Number { raw: b"-inf", kind: NumberKind::Float }), span);
                }
                Err(err!(UnsupportedZon))
            }
            _ => Err(err!(UnsupportedZon)),
        }
    }

    fn identifier(&mut self, node: NodeIndex) -> Result<NodeId> {
        let span = self.node_span(node);
        let ident = self.token_slice(self.tree.node(node).main_token).to_vec();
        match &ident[..] {
            b"true" => self.add_node(NodeKind::Boolean(true), span),
            b"false" => self.add_node(NodeKind::Boolean(false), span),
            b"null" => self.add_node(NodeKind::Null, span),
            b"inf" => self.add_node(NodeKind::Number(Number { raw: b"inf", kind: NumberKind::Float }), span),
            b"nan" => self.add_node(NodeKind::Number(Number { raw: b"nan", kind: NumberKind::Float }), span),
            _ => Err(err!(UnsupportedZon)), // ZON allows no other bare identifiers
        }
    }

    /// `.foo` enum literal → `extended` enum_literal whose text is the bare
    /// name (no leading `.`). The name is a valid identifier in the common
    /// case (slice straight from source); the rare `.@"quoted"` form is
    /// decoded into an owned string. The node's span covers the leading `.`
    /// too, so an edit that splices over the value replaces the whole literal
    /// rather than leaving the dot behind.
    fn enum_literal(&mut self, node: NodeIndex) -> Result<NodeId> {
        let n = self.tree.node(node);
        let token = n.main_token;
        let name = self.token_span(token);
        let span = Span::new(self.tree.token_start(n.first_token), name.end);
        let source = self.source.clone();
        let slice = &source[name.start..name.end];
        if slice.starts_with(b"@\"") {
            let decoded = zig_string_literal::parse_alloc(&slice[1..]).ok_or(err!(InvalidZon))?;
            return self.add_node(NodeKind::Extended(Extended { kind: ExtKind::EnumLiteral, text: &decoded }), span);
        }
        self.add_node(NodeKind::Extended(Extended { kind: ExtKind::EnumLiteral, text: slice }), span)
    }

    fn string_literal(&mut self, node: NodeIndex) -> Result<NodeId> {
        let span = self.node_span(node);

        // Let stdlib decode escapes and join multiline `\\` lines for us.
        let mut decoded: Vec<u8> = Vec::new();
        // `Ast.parse` already validated syntax, so a decode failure here is
        // not expected; treat defensively as malformed input.
        self.parse_str_lit(node, &mut decoded).map_err(|_| err!(InvalidZon))?;

        self.add_node(NodeKind::String(&decoded), span)
    }

    /// `std.zig.ZonGen.parseStrLit`: a string literal's decoded bytes, or a
    /// multiline string's lines joined by `\n`.
    fn parse_str_lit(&self, node: NodeIndex, out: &mut Vec<u8>) -> std::result::Result<(), zig_string_literal::Error> {
        let n = self.tree.node(node);
        match n.kind {
            Kind::StringLiteral => zig_string_literal::parse_write(out, self.token_slice(n.main_token)),
            Kind::MultilineStringLiteral => {
                let (first_tok, last_tok) = (n.main_token, n.last_token);
                // First line: do not append a newline.
                out.extend_from_slice(&self.token_slice(first_tok)[2..]);
                // Following lines: each line prepends a newline.
                for tok_idx in first_tok + 1..last_tok + 1 {
                    out.push(b'\n');
                    out.extend_from_slice(&self.token_slice(tok_idx)[2..]);
                }
                Ok(())
            }
            // Node must represent a string
            _ => unreachable!(),
        }
    }

    /// `'a'` char literal → `extended` char_literal whose text is the decimal
    /// Unicode codepoint. Storing the codepoint (not the raw `'a'`) keeps the
    /// Zig-specific char codec confined to this parser and the ZON printer:
    /// other formats treat the text as a plain number. The ZON printer
    /// re-encodes `'a'`.
    fn char_literal(&mut self, node: NodeIndex) -> Result<NodeId> {
        let span = self.node_span(node);
        let slice = self.token_slice(self.tree.node(node).main_token);
        let codepoint = zig_string_literal::parse_char_literal(slice).map_err(|_| err!(InvalidZon))?;
        let raw = codepoint.to_string(); // a u21 has at most 7 digits
        self.add_node(NodeKind::Extended(Extended { kind: ExtKind::CharLiteral, text: raw.as_bytes() }), span)
    }

    /// Build a string key node from a struct-field name token.
    fn field_name(&mut self, token: TokenIndex) -> Result<NodeId> {
        let span = self.token_span(token);
        let source = self.source.clone();
        let slice = &source[span.start..span.end];
        if slice.starts_with(b"@\"") {
            let decoded = zig_string_literal::parse_alloc(&slice[1..]).ok_or(err!(InvalidZon))?;
            return self.add_node(NodeKind::String(&decoded), span);
        }
        self.add_node(NodeKind::String(slice), span)
    }

    // =========
    // HELPERS
    // =========

    fn add_node(&mut self, kind: NodeKind<'_>, span: Span) -> Result<NodeId> {
        self.w.add(kind, span)
    }

    // ── Comments ─────────────────────────────────────────────────────────

    /// Scan the source gap `[scan_pos, pos)` for `//` line comments,
    /// classifying each: one on the same line as the last value
    /// (`last_value_id` set, no newline since) trails it, others buffer as
    /// leading. Only `//` and newlines are significant; every other byte
    /// (structural punctuation, whitespace) is skipped.
    fn absorb_comments_up_to(&mut self, pos: usize) -> Result<()> {
        let source = self.source.clone();
        let mut i = self.scan_pos;
        while i < pos {
            let c = source[i];
            if c == b'\n' {
                self.last_value_id = None;
                i += 1;
            } else if c == b'/' && i + 1 < source.len() && source[i + 1] == b'/' {
                let start = i + 2;
                let mut j = start;
                while j < source.len() && source[j] != b'\n' {
                    j += 1;
                }
                let text = trim_span(&source, Span::new(start, j), b" \t\r");
                if let Some(id) = self.last_value_id {
                    // A comment on a multi-line container's closing line is a
                    // bottom comment → its `dangling` run; a scalar / inline
                    // container keeps the same-line `trailing`.
                    if self.multiline_container(id, i) {
                        self.append_dangling(id, text)?;
                    } else {
                        let c = Comment::line(&source[text.start..text.end]);
                        self.w.comment(id, table::COMMENT_TRAILING, c)?;
                    }
                    self.last_value_id = None;
                } else {
                    self.pending_leading.push(text);
                }
                i = j;
            } else {
                i += 1;
            }
        }
        if i > self.scan_pos {
            self.scan_pos = i;
        }
        Ok(())
    }

    /// Hand the buffered leading comments to node `id`, then clear the
    /// buffer. No-op when nothing is buffered.
    fn claim_leading(&mut self, id: NodeId) -> Result<()> {
        let pending = std::mem::take(&mut self.pending_leading);
        let source = self.source.clone();
        for c in &pending {
            self.w.comment(id, table::COMMENT_LEADING, Comment::line(&source[c.start..c.end]))?;
        }
        Ok(())
    }

    /// Capture a line comment immediately after a container's `.{` that ends
    /// its line (`.{ // c`) as container `id`'s own trailing (the head
    /// comment). A field or block comment on the open line is left for the
    /// per-field absorb.
    fn capture_open_trailing(&mut self, id: NodeId, node: NodeIndex) -> Result<()> {
        let source = self.source.clone();
        let mut brace = self.node_span(node).start;
        while brace < source.len() && source[brace] != b'{' {
            brace += 1;
        }
        let mut i = brace + 1;
        while i < source.len() {
            match source[i] {
                b'\n' => return Ok(()), // open line ended without a comment
                b' ' | b'\t' | b'\r' => {}
                b'/' => {
                    if i + 1 >= source.len() || source[i + 1] != b'/' {
                        return Ok(());
                    }
                    let start = i + 2;
                    let mut j = start;
                    while j < source.len() && source[j] != b'\n' {
                        j += 1;
                    }
                    let text = trim_span(&source, Span::new(start, j), b" \t\r");
                    self.w.comment(id, table::COMMENT_TRAILING, Comment::line(&source[text.start..text.end]))?;
                    if j > self.scan_pos {
                        self.scan_pos = j; // skip it in later absorbs
                    }
                    return Ok(());
                }
                _ => return Ok(()), // a field begins on the open line → no head comment
            }
            i += 1;
        }
        Ok(())
    }

    /// Whether `id` is a container whose `.{` precedes `cpos` on an earlier
    /// line — a multi-line container whose close is on `cpos`'s line.
    fn multiline_container(&self, id: NodeId, cpos: usize) -> bool {
        match self.w.kind_of(id) {
            NodeKind::Sequence(_) | NodeKind::Mapping(_) => {}
            _ => return false,
        }
        let open = self.w.span_of(id).start;
        if cpos <= open {
            return false;
        }
        self.source[open..cpos].contains(&b'\n')
    }

    /// Append one comment to `id`'s `dangling` run, after any orphans
    /// already claimed at the close.
    fn append_dangling(&mut self, id: NodeId, text: Span) -> Result<()> {
        let source = self.source.clone();
        self.w.comment(id, table::COMMENT_DANGLING, Comment::line(&source[text.start..text.end]))
    }

    /// Hand buffered orphan comments (own-line comments at the end of a
    /// container's body, before its closing brace) to container `id` as its
    /// `dangling` run.
    fn claim_dangling(&mut self, id: NodeId) -> Result<()> {
        let pending = std::mem::take(&mut self.pending_leading);
        let source = self.source.clone();
        for c in &pending {
            self.w.comment(id, table::COMMENT_DANGLING, Comment::line(&source[c.start..c.end]))?;
        }
        Ok(())
    }

    fn node_span(&self, node: NodeIndex) -> Span {
        let n = self.tree.node(node);
        Span::new(self.tree.token_start(n.first_token), self.tree.token_end(n.last_token))
    }

    fn token_span(&self, token: TokenIndex) -> Span {
        Span::new(self.tree.token_start(token), self.tree.token_end(token))
    }

    fn token_slice(&self, token: TokenIndex) -> &[u8] {
        &self.source[self.tree.token_start(token)..self.tree.token_end(token)]
    }
}

/// `std.mem.trim(u8, source[span], set)`, as a span.
fn trim_span(source: &[u8], span: Span, set: &[u8]) -> Span {
    let s = &source[span.start..span.end];
    let start = s.iter().position(|c| !set.contains(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !set.contains(c)).map_or(start, |e| e + 1).max(start);
    Span::new(span.start + start, span.start + end)
}

/// fig's `Number.kind` is just a hint (the raw text is authoritative).
/// Classify by inspecting the verbatim ZON literal: anything with a
/// fractional point, a hex `p`-exponent, or a decimal `e`-exponent is a
/// float; everything else (incl. `0x`/`0o`/`0b` integers and `_`-separated
/// digits) is an integer.
fn classify_number(raw: &[u8]) -> NumberKind {
    if raw.contains(&b'.') {
        return NumberKind::Float;
    }
    let body = raw.strip_prefix(b"-").unwrap_or(raw);
    let is_hex = body.len() >= 2 && body[0] == b'0' && (body[1] == b'x' || body[1] == b'X');
    if is_hex {
        // In hex, `e`/`E` are digits; only `p`/`P` introduces an exponent.
        return if body.iter().any(|&c| c == b'p' || c == b'P') { NumberKind::Float } else { NumberKind::Integer };
    }
    if raw.iter().any(|&c| c == b'e' || c == b'E') { NumberKind::Float } else { NumberKind::Integer }
}

// =========
// TESTS
// =========

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::{Literal, Node, PathSegment};

    fn expect_parse(input: &str, root: NodeId, nodes: &[Node<'_>]) {
        let ast = parse_abstract(input.as_bytes(), Type::Zon).unwrap();
        assert!(Literal { root, nodes }.matches(&ast), "{input:?}");
    }

    fn n(id: NodeId, kind: NodeKind<'static>) -> Node<'static> {
        Node { id, kind, next_sibling: None }
    }

    fn num(raw: &'static str, kind: NumberKind) -> NodeKind<'static> {
        NodeKind::Number(Number { raw: raw.as_bytes(), kind })
    }

    fn key(k: &str) -> PathSegment<'_> {
        PathSegment::Key(k.as_bytes())
    }

    #[test]
    fn scalar_literals() {
        expect_parse("true", 0, &[n(0, NodeKind::Boolean(true))]);
        expect_parse("null", 0, &[n(0, NodeKind::Null)]);
        expect_parse("42", 0, &[n(0, num("42", NumberKind::Integer))]);
        expect_parse("-3.5", 0, &[n(0, num("-3.5", NumberKind::Float))]);
    }

    #[test]
    fn struct_literal_becomes_a_mapping() {
        expect_parse(
            ".{ .name = \"Ada\", .age = 36 }",
            0,
            &[
                n(0, NodeKind::Mapping(Some(3))),
                n(1, NodeKind::String(b"name")),
                n(2, NodeKind::String(b"Ada")),
                Node { id: 3, kind: NodeKind::KeyValue(KeyValue { key: 1, value: 2 }), next_sibling: Some(6) },
                n(4, NodeKind::String(b"age")),
                n(5, num("36", NumberKind::Integer)),
                n(6, NodeKind::KeyValue(KeyValue { key: 4, value: 5 })),
            ],
        );
    }

    #[test]
    fn array_literal_becomes_a_sequence() {
        expect_parse(
            ".{ 1, 2, 3 }",
            0,
            &[
                n(0, NodeKind::Sequence(Some(1))),
                Node { id: 1, kind: num("1", NumberKind::Integer), next_sibling: Some(2) },
                Node { id: 2, kind: num("2", NumberKind::Integer), next_sibling: Some(3) },
                n(3, num("3", NumberKind::Integer)),
            ],
        );
    }

    #[test]
    fn empty_dot_brace_is_an_empty_mapping() {
        expect_parse(".{}", 0, &[n(0, NodeKind::Mapping(None))]);
    }

    #[test]
    fn string_escapes_are_decoded() {
        let ast = parse_abstract(b"\"tab:\\tnl:\\n\"", Type::Zon).unwrap();
        assert_eq!(b"tab:\tnl:\n", ast.node(ast.root).kind.as_string().unwrap());
    }

    #[test]
    fn multiline_string_literal_is_joined() {
        let ast = parse_abstract(b".{\n    .body =\n        \\\\line one\n        \\\\line two\n    ,\n}", Type::Zon).unwrap();
        let value = ast.get_val_by_path(&[key("body")]).unwrap();
        assert_eq!(b"line one\nline two", value.kind.as_string().unwrap());
    }

    #[test]
    fn enum_literal_preserved_as_extended() {
        let ast = parse_abstract(b".{ .mode = .fast }", Type::Zon).unwrap();
        let value = ast.get_val_by_path(&[key("mode")]).unwrap();
        assert_eq!(NodeKind::Extended(Extended { kind: ExtKind::EnumLiteral, text: b"fast" }), value.kind);
    }

    #[test]
    fn char_literal_preserved_as_extended_codepoint() {
        let ast = parse_abstract(b".{ .c = 'A' }", Type::Zon).unwrap();
        let value = ast.get_val_by_path(&[key("c")]).unwrap();
        assert_eq!(NodeKind::Extended(Extended { kind: ExtKind::CharLiteral, text: b"65" }), value.kind);
    }

    #[test]
    fn nested_containers() {
        let ast = parse_abstract(b".{ .items = .{ .{ .id = 1 }, .{ .id = 2 } } }", Type::Zon).unwrap();
        let second = ast.get_val_by_path(&[key("items"), PathSegment::Index(1), key("id")]).unwrap();
        assert_eq!(b"2", second.kind.as_number().unwrap().raw);
    }

    #[test]
    fn hex_and_underscored_numbers_keep_raw_text() {
        let ast = parse_abstract(b".{ .a = 0xFF, .b = 1_000, .c = 0x1.5p3 }", Type::Zon).unwrap();
        let a = ast.get_val_by_path(&[key("a")]).unwrap().kind.as_number().unwrap();
        assert_eq!(b"0xFF", a.raw);
        assert_eq!(NumberKind::Integer, a.kind);
        assert_eq!(b"1_000", ast.get_val_by_path(&[key("b")]).unwrap().kind.as_number().unwrap().raw);
        assert_eq!(NumberKind::Float, ast.get_val_by_path(&[key("c")]).unwrap().kind.as_number().unwrap().kind);
    }

    #[test]
    fn syntax_errors_are_rejected() {
        assert_eq!(err!(InvalidZon), parse_abstract(b".{ .a = }", Type::Zon).unwrap_err());
        assert_eq!(err!(InvalidZon), parse_abstract(b"", Type::Zon).unwrap_err());
    }

    #[test]
    fn a_zig_expression_with_no_zon_value_is_unsupported() {
        assert_eq!(err!(UnsupportedZon), parse_abstract(b".{ .a = 1 + 2 }", Type::Zon).unwrap_err());
        assert_eq!(err!(UnsupportedZon), parse_abstract(b"T{ .a = 1 }", Type::Zon).unwrap_err());
        assert_eq!(err!(UnsupportedZon), parse_abstract(b".{ foo }", Type::Zon).unwrap_err());
    }
}
