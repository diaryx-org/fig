//! Port of src/canonical/parser.zig.
//!
//! The canonical-form parser: text → AST, the exact inverse of
//! `canonical/printer.rs`. (This "canonical form" was formerly called "native";
//! the human-facing `fig` authoring dialect is a separate surface — see languages/figl/DESIGN.md.)
//!
//! Recursive-descent over the byte stream (the grammar is small enough that the
//! token framework the format parsers share would be overkill). It writes
//! every `NodeKind` arm as a row of fig's node table (through a `RowWriter`,
//! in the order it meets them), with the YAML reference layer (anchors
//! `&name`, tags `!tag`, aliases `*name`) and the comments as the table's
//! side facts, so `print` ∘ `parse` is the identity on any AST.
//!
//! Strings, numbers, anchor names, tag and comment text name the input where
//! they can (only an escaped string is copied, into the table's buffer).
//!
//! Errors (`ParseError`): `UnexpectedToken`, `UnexpectedEnd`,
//! `UnclosedString`, `UnclosedArray`, `UnclosedObject`,
//! `UnterminatedComment`, `ExpectedColon`, `InvalidExtended`,
//! `InvalidEscape`, `InvalidUnicodeEscape`, `EmptyName`, `TrailingGarbage`,
//! `NestingTooDeep`; and `OutOfMemory`.

use std::sync::Arc;

use super::printer::{self, MAX_DEPTH};
use crate::err;
use crate::error::Error;
use crate::kit::ast::{Comment, ExtKind, KeyValue, NodeId, NodeKind, Number, NumberKind, Style, Tag, AST};
use crate::kit::document::Document;
use crate::kit::row_writer::RowWriter;
use crate::kit::span::Span;
use crate::kit::table::{self, Table};
use crate::Result;

/// Parse `input` into an owned `AST`.
pub fn parse_abstract(input: &[u8]) -> Result<AST> {
    Ok(parse(input)?.ast)
}

/// Parse `input` into a `Document` (AST + source spans).
pub fn parse(input: &[u8]) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut parser = Parser {
        src: source.clone(),
        pos: 0,
        depth: 0,
        t: Table::init(source.clone()),
        scratch: Vec::new(),
        pending_leading: Vec::new(),
    };
    let root = parser.parse_once()?;
    RowWriter::init(&mut parser.t).finish(root)?;
    RowWriter::adopt(source, &mut parser.t)
}

/// A buffered comment: its text's range in `src` (comments never contain
/// escapes) and its style.
#[derive(Debug, Clone, Copy)]
struct PendingComment {
    start: usize,
    end: usize,
    style: Style,
}

/// Where a string value's bytes are: in the source, or decoded into
/// `scratch` (valid until the next decode, so the row copies it at once).
#[derive(Debug, Clone, Copy)]
enum Str {
    Src(usize, usize),
    Scratch,
}

struct Parser {
    /// The input, the same allocation as `t`'s source, so a slice of it is
    /// written as in-source text.
    src: Arc<[u8]>,
    pos: usize,
    /// Current container-nesting depth, bounded by `printer::MAX_DEPTH` so a
    /// pathologically nested input can't overflow the recursive descent's stack.
    depth: usize,
    /// The rows written so far, over `src`; anchors, tags and comments are its
    /// side facts, appended as they are met.
    t: Table,
    /// An escaped string's decoded bytes, until its row copies them.
    scratch: Vec<u8>,
    /// Own-line comments seen while skipping trivia, until the next node claims
    /// them.
    pending_leading: Vec<PendingComment>,
}

impl Parser {
    /// Parse the document, returning its root's row.
    fn parse_once(&mut self) -> Result<NodeId> {
        let root = self.parse_node()?;
        self.claim_leading(root)?;
        // A trailing comment on the same line as the root, then the document's final
        // trivia; any end-of-file orphan comments become the root's dangling run.
        self.skip_inline();
        if let Some(c) = self.try_comment()? {
            self.set_trailing(root, c)?;
        }
        self.collect_leading()?;
        self.claim_dangling(root)?;
        if self.peek().is_some() {
            return Err(err!(TrailingGarbage));
        }
        Ok(root)
    }

    // ── node construction ───────────────────────────────────────────────────

    /// A new row of `kind`, spanning `start` to the current position.
    fn add_node(&mut self, kind: NodeKind<'_>, start: usize) -> Result<NodeId> {
        RowWriter::init(&mut self.t).add(kind, Span::new(start, self.pos))
    }

    /// A new string row, its bytes where `s` says.
    fn add_string(&mut self, s: Str, start: usize) -> Result<NodeId> {
        let span = Span::new(start, self.pos);
        let bytes: &[u8] = match s {
            Str::Src(a, b) => &self.src[a..b],
            Str::Scratch => &self.scratch,
        };
        RowWriter::init(&mut self.t).add(NodeKind::String(bytes), span)
    }

    /// End node `id`'s span at the current position.
    fn end_span(&mut self, id: NodeId) {
        let mut w = RowWriter::init(&mut self.t);
        let start = w.span_of(id).start;
        w.set_span(id, Span::new(start, self.pos));
    }

    // ── grammar ─────────────────────────────────────────────────────────────

    /// A node is any reference-layer prefixes (`&anchor`, `!tag`) followed by a
    /// value. Leading comments encountered here accumulate in `pending_leading`;
    /// the enclosing container (or the document root) claims them onto the right
    /// node via `claim_leading` once it knows which node they belong to.
    fn parse_node(&mut self) -> Result<NodeId> {
        self.collect_leading()?;
        let mut anchor: Option<(usize, usize)> = None;
        let mut tag: Option<(usize, usize)> = None;
        loop {
            match self.peek().ok_or(err!(UnexpectedEnd))? {
                b'&' => {
                    if anchor.is_some() {
                        return Err(err!(UnexpectedToken));
                    }
                    anchor = Some(self.parse_anchor_name()?);
                    self.skip_ws();
                }
                b'!' => {
                    if tag.is_some() {
                        return Err(err!(UnexpectedToken));
                    }
                    tag = Some(self.parse_tag());
                    self.skip_ws();
                }
                _ => break,
            }
        }

        let id = self.parse_value()?;

        if let Some((a, b)) = anchor {
            RowWriter::init(&mut self.t).anchor(id, &self.src[a..b], None)?;
        }
        if let Some((a, b)) = tag {
            // Canonical is the exact-byte oracle: keep the tag VERBATIM in `.text`
            // so its spelling round-trips (a `.kind` tag appears in canonical output
            // only for a fig/builder-origin AST — see the printer).
            RowWriter::init(&mut self.t).tag(id, Tag::Text(&self.src[a..b]), table::TAG_UNSTATED, true, None)?;
        }
        Ok(id)
    }

    fn parse_value(&mut self) -> Result<NodeId> {
        let start = self.pos;
        match self.peek().ok_or(err!(UnexpectedEnd))? {
            b'{' => self.parse_mapping(),
            b'[' => self.parse_sequence(),
            b'"' => {
                let s = self.parse_string_value()?;
                self.add_string(s, start)
            }
            b'@' => self.parse_extended(),
            b'*' => self.parse_alias(),
            b'0'..=b'9' | b'+' | b'-' | b'.' | b'~' => self.parse_number(),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.parse_bareword(),
            _ => Err(err!(UnexpectedToken)),
        }
    }

    fn parse_bareword(&mut self) -> Result<NodeId> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if !is_name_char(c) {
                break;
            }
            self.pos += 1;
        }
        let word = &self.src[start..self.pos];
        let kind = if word == b"null" {
            NodeKind::Null
        } else if word == b"true" {
            NodeKind::Boolean(true)
        } else if word == b"false" {
            NodeKind::Boolean(false)
        } else {
            return Err(err!(UnexpectedToken));
        };
        self.add_node(kind, start)
    }

    fn parse_number(&mut self) -> Result<NodeId> {
        let start = self.pos;
        // Optional `~i`/`~f` kind override for the rare lexeme/kind mismatch.
        let mut over: Option<bool> = None;
        if self.peek() == Some(b'~') {
            self.pos += 1;
            match self.peek().ok_or(err!(UnexpectedEnd))? {
                b'f' => over = Some(true),
                b'i' => over = Some(false),
                _ => return Err(err!(UnexpectedToken)),
            }
            self.pos += 1;
        }
        let raw_start = self.pos;
        while let Some(c) = self.peek() {
            if !is_number_char(c) {
                break;
            }
            self.pos += 1;
        }
        let src = self.src.clone();
        let raw = &src[raw_start..self.pos];
        if raw.is_empty() || !looks_like_number(raw) {
            return Err(err!(UnexpectedToken));
        }
        let kind = match over {
            Some(true) => NumberKind::Float,
            Some(false) => NumberKind::Integer,
            None => printer::implied_number_kind(raw),
        };
        self.add_node(NodeKind::Number(Number { raw, kind }), start)
    }

    fn parse_extended(&mut self) -> Result<NodeId> {
        let start = self.pos;
        self.pos += 1; // '@'
        let kind_start = self.pos;
        while let Some(c) = self.peek() {
            if !(c.is_ascii_lowercase() || c == b'_') {
                break;
            }
            self.pos += 1;
        }
        let kind_name = &self.src[kind_start..self.pos];
        let kind = ExtKind::ALL.into_iter().find(|k| k.name().as_bytes() == kind_name).ok_or(err!(InvalidExtended))?;
        self.skip_ws();
        if self.peek() != Some(b'"') {
            return Err(err!(InvalidExtended));
        }
        let s = self.parse_string_value()?;
        let span = Span::new(start, self.pos);
        let text: &[u8] = match s {
            Str::Src(a, b) => &self.src[a..b],
            Str::Scratch => &self.scratch,
        };
        RowWriter::init(&mut self.t).add(NodeKind::Extended(crate::kit::ast::Extended { kind, text }), span)
    }

    fn parse_alias(&mut self) -> Result<NodeId> {
        let start = self.pos;
        self.pos += 1; // '*'
        let name_start = self.pos;
        while let Some(c) = self.peek() {
            if !is_name_char(c) {
                break;
            }
            self.pos += 1;
        }
        if self.pos == name_start {
            return Err(err!(EmptyName));
        }
        let src = self.src.clone();
        self.add_node(NodeKind::Alias(&src[name_start..self.pos]), start)
    }

    fn parse_sequence(&mut self) -> Result<NodeId> {
        let start = self.pos;
        self.pos += 1; // '['
        self.depth += 1;
        let r = self.parse_sequence_body(start);
        self.depth -= 1;
        r
    }

    fn parse_sequence_body(&mut self, start: usize) -> Result<NodeId> {
        if self.depth > MAX_DEPTH {
            return Err(err!(NestingTooDeep));
        }
        let id = self.add_node(NodeKind::Sequence(None), start)?;
        self.capture_open_trailing(id)?;
        self.collect_leading()?;
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.claim_dangling(id)?; // orphan comments inside an empty `[ … ]`
            self.end_span(id);
            return Ok(id);
        }
        let mut first: Option<NodeId> = None;
        let mut prev: Option<NodeId> = None;
        loop {
            let child = self.parse_node()?;
            // A sequence element is its own value node: leading and trailing
            // comments both bind directly to it.
            self.claim_leading(child)?;
            match prev {
                Some(p) => RowWriter::init(&mut self.t).set_next(p, Some(child)),
                None => first = Some(child),
            }
            prev = Some(child);
            if self.finish_element(b']', err!(UnclosedArray), child)? {
                self.claim_dangling(id)?; // orphan comments before the `]`
                break;
            }
        }
        RowWriter::init(&mut self.t).set(id, NodeKind::Sequence(first))?;
        self.end_span(id);
        Ok(id)
    }

    fn parse_mapping(&mut self) -> Result<NodeId> {
        let start = self.pos;
        self.pos += 1; // '{'
        self.depth += 1;
        let r = self.parse_mapping_body(start);
        self.depth -= 1;
        r
    }

    fn parse_mapping_body(&mut self, start: usize) -> Result<NodeId> {
        if self.depth > MAX_DEPTH {
            return Err(err!(NestingTooDeep));
        }
        let id = self.add_node(NodeKind::Mapping(None), start)?;
        self.capture_open_trailing(id)?;
        self.collect_leading()?;
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.claim_dangling(id)?; // orphan comments inside an empty `{ … }`
            self.end_span(id);
            return Ok(id);
        }
        let mut first: Option<NodeId> = None;
        let mut prev: Option<NodeId> = None;
        loop {
            let key = self.parse_node()?;
            // An entry's leading comment sits above the key; its trailing comment
            // follows the value. This mirrors the canonical printer's anchors.
            self.claim_leading(key)?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(err!(ExpectedColon));
            }
            self.pos += 1;
            let value = self.parse_node()?;
            // Normally empty; claims any (non-canonical) comment between `:` and the
            // value so it can't leak onto the next entry's key.
            self.claim_leading(value)?;
            let key_start = RowWriter::init(&mut self.t).span_of(key).start;
            let kv = self.add_node(NodeKind::KeyValue(KeyValue { key, value }), key_start)?;
            match prev {
                Some(p) => RowWriter::init(&mut self.t).set_next(p, Some(kv)),
                None => first = Some(kv),
            }
            prev = Some(kv);
            if self.finish_element(b'}', err!(UnclosedObject), value)? {
                self.claim_dangling(id)?; // orphan comments before the `}`
                break;
            }
        }
        RowWriter::init(&mut self.t).set(id, NodeKind::Mapping(first))?;
        self.end_span(id);
        Ok(id)
    }

    // ── lexical helpers ─────────────────────────────────────────────────────

    fn parse_anchor_name(&mut self) -> Result<(usize, usize)> {
        self.pos += 1; // '&'
        let start = self.pos;
        while let Some(c) = self.peek() {
            if !is_name_char(c) {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return Err(err!(EmptyName));
        }
        Ok((start, self.pos))
    }

    /// A tag is the verbatim `!`-led token (e.g. `!!str`, `!foo`), stored leading
    /// `!` included to match how the format parsers fill their tags. It runs to the
    /// next whitespace or structural delimiter.
    fn parse_tag(&mut self) -> (usize, usize) {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if !is_tag_char(c) {
                break;
            }
            self.pos += 1;
        }
        (start, self.pos)
    }

    fn parse_string_value(&mut self) -> Result<Str> {
        // self.peek() == '"'
        self.pos += 1; // opening quote
        let inner_start = self.pos;
        let mut i = self.pos;
        let mut has_escape = false;
        while i < self.src.len() {
            let ch = self.src[i];
            if ch == b'"' {
                break;
            }
            if ch == b'\\' {
                has_escape = true;
                i += 2; // skip the escaped char; the bounds check re-runs next loop
                continue;
            }
            i += 1;
        }
        if i >= self.src.len() {
            return Err(err!(UnclosedString));
        }
        self.pos = i + 1; // past closing quote

        if !has_escape {
            return Ok(Str::Src(inner_start, i)); // fast path: borrow from source
        }
        self.decode_escapes(inner_start, i)?;
        Ok(Str::Scratch)
    }

    /// `src[from..to]` with its escapes decoded, into `scratch`.
    fn decode_escapes(&mut self, from: usize, to: usize) -> Result<()> {
        let inner = &self.src[from..to];
        let decoded = &mut self.scratch;
        decoded.clear();
        let mut i = 0usize;
        while i < inner.len() {
            let c = inner[i];
            if c != b'\\' {
                decoded.push(c);
                i += 1;
                continue;
            }
            i += 1;
            if i >= inner.len() {
                return Err(err!(UnclosedString));
            }
            match inner[i] {
                b'"' => decoded.push(b'"'),
                b'\\' => decoded.push(b'\\'),
                b'/' => decoded.push(b'/'),
                b'b' => decoded.push(0x08),
                b'f' => decoded.push(0x0c),
                b'n' => decoded.push(b'\n'),
                b'r' => decoded.push(b'\r'),
                b't' => decoded.push(b'\t'),
                b'u' => {
                    if i + 4 >= inner.len() {
                        return Err(err!(InvalidUnicodeEscape));
                    }
                    let unit = zig_parse_hex(&inner[i + 1..i + 5]).ok_or(err!(InvalidUnicodeEscape))?;
                    let ch = char::from_u32(unit).ok_or(err!(InvalidUnicodeEscape))?;
                    let mut buf = [0u8; 4];
                    decoded.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    i += 4;
                }
                _ => return Err(err!(InvalidEscape)),
            }
            i += 1;
        }
        Ok(())
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    /// Skip spaces and tabs only — never a newline, never a comment. Used to reach a
    /// same-line trailing comment or a separator without crossing into the next
    /// line's leading trivia.
    fn skip_inline(&mut self) {
        while let Some(c) = self.peek() {
            if c == b' ' || c == b'\t' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Skip whitespace (newlines included) and comments, buffering each comment in
    /// `pending_leading` for the next node to claim. Returns at the next value byte.
    fn collect_leading(&mut self) -> Result<()> {
        loop {
            self.skip_ws();
            let Some(c) = self.try_comment()? else { return Ok(()) };
            self.pending_leading.push(c);
            // A comment is written only once it binds to a node (in
            // `claim_leading`/`set_trailing`), so a buffered-then-dropped orphan
            // leaves the document comment-free.
        }
    }

    /// If positioned at a comment marker, consume the whole comment and return it;
    /// otherwise leave `pos` unchanged and return `None`. Its text is a range of
    /// `src` (marker stripped, surrounding whitespace trimmed).
    fn try_comment(&mut self) -> Result<Option<PendingComment>> {
        if self.peek() != Some(b'/') || self.pos + 1 >= self.src.len() {
            return Ok(None);
        }
        match self.src[self.pos + 1] {
            b'/' => {
                self.pos += 2;
                let s = self.pos;
                while let Some(c) = self.peek() {
                    if c == b'\n' {
                        break;
                    }
                    self.pos += 1;
                }
                let (a, b) = trim(&self.src, s, self.pos, b" \t\r");
                Ok(Some(PendingComment { start: a, end: b, style: Style::Line }))
            }
            b'*' => {
                self.pos += 2;
                let s = self.pos;
                let len = self.src[self.pos..].windows(2).position(|w| w == b"*/").ok_or(err!(UnterminatedComment))?;
                self.pos += len + 2; // past the closing `*/`
                let (a, b) = trim(&self.src, s, s + len, b" \t\r\n");
                Ok(Some(PendingComment { start: a, end: b, style: Style::Block }))
            }
            _ => Ok(None),
        }
    }

    /// Hand the buffered leading comments to node `id`, after any it has. No-op
    /// when nothing is buffered.
    fn claim_leading(&mut self, id: NodeId) -> Result<()> {
        self.claim_pending(id, table::COMMENT_LEADING)
    }

    /// Write the buffered comments onto node `id` in `slot`, after any already
    /// there, and empty the buffer.
    fn claim_pending(&mut self, id: NodeId, slot: u8) -> Result<()> {
        let pending = std::mem::take(&mut self.pending_leading);
        for c in &pending {
            self.write_comment(id, slot, *c)?;
        }
        self.pending_leading = pending;
        self.pending_leading.clear();
        Ok(())
    }

    fn write_comment(&mut self, id: NodeId, slot: u8, c: PendingComment) -> Result<()> {
        let text = &self.src[c.start..c.end];
        RowWriter::init(&mut self.t).comment(id, slot, Comment { text, style: c.style })
    }

    /// Bind a same-line trailing comment to node `id`, replacing any it had.
    fn set_trailing(&mut self, id: NodeId, c: PendingComment) -> Result<()> {
        self.write_comment(id, table::COMMENT_TRAILING, c)
    }

    /// Capture a line comment immediately after an opening delimiter that ends its
    /// line (`[ // c`) as container `id`'s own trailing (the head comment). A block
    /// comment is left for `collect_leading` — it may lead the first child.
    fn capture_open_trailing(&mut self, id: NodeId) -> Result<()> {
        self.skip_inline();
        if self.peek() == Some(b'/') && self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'/' {
            if let Some(c) = self.try_comment()? {
                self.set_trailing(id, c)?;
            }
        }
        Ok(())
    }

    /// Whether `id` is a container whose opening delimiter precedes `cpos` on an
    /// earlier line — a multi-line `[ … ]`/`{ … }` whose close is on `cpos`'s line.
    fn multiline_container(&mut self, id: NodeId, cpos: usize) -> bool {
        let w = RowWriter::init(&mut self.t);
        match w.kind_of(id) {
            NodeKind::Sequence(_) | NodeKind::Mapping(_) => {}
            _ => return false,
        }
        let open = w.span_of(id).start;
        if cpos <= open {
            return false;
        }
        self.src[open..cpos].contains(&b'\n')
    }

    /// Append one comment to `id`'s `dangling` run, after any orphans already
    /// claimed at the close.
    fn append_dangling(&mut self, id: NodeId, c: PendingComment) -> Result<()> {
        self.write_comment(id, table::COMMENT_DANGLING, c)
    }

    /// Hand buffered orphan comments (no node followed them) to container `id` as its
    /// `dangling` run — they sit at the end of its body. No-op when nothing buffered.
    /// `id` may already carry a `dangling` run (e.g. the document root, claimed once
    /// for orphans before its closing bracket and again for trailing EOF orphans), so
    /// this appends rather than overwrites.
    fn claim_dangling(&mut self, id: NodeId) -> Result<()> {
        self.claim_pending(id, table::COMMENT_DANGLING)
    }

    /// After an element's value is parsed (and its leading claimed), consume an
    /// optional same-line trailing comment plus the separator. Binds the trailing
    /// comment to `trailing_target`. Returns true when the container closed (`close`
    /// consumed), false when another element follows. The canonical native form
    /// puts a trailing comment after the comma (`v, // c`); a last element carries
    /// it directly (`v // c`).
    fn finish_element(&mut self, close: u8, unclosed: Error, trailing_target: NodeId) -> Result<bool> {
        self.skip_inline();
        let mut had_comma = false;
        if self.peek() == Some(b',') {
            self.pos += 1;
            had_comma = true;
            self.skip_inline();
        }
        let cpos = self.pos;
        if let Some(c) = self.try_comment()? {
            // A comment after a multi-line container's close is a bottom comment →
            // its `dangling` run; a scalar or inline container keeps the trailing.
            if self.multiline_container(trailing_target, cpos) {
                self.append_dangling(trailing_target, c)?;
            } else {
                self.set_trailing(trailing_target, c)?;
            }
        }
        self.collect_leading()?;
        let c = self.peek().ok_or(unclosed)?;
        if c == close {
            self.pos += 1;
            // Any orphan comments before the close stay buffered; the caller claims
            // them as the container's `dangling` run.
            return Ok(true);
        }
        // More content is only legal across a comma separator.
        if had_comma {
            return Ok(false);
        }
        Err(err!(UnexpectedToken))
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }
}

/// `src[from..to]` less any of `set` at either end, as a range.
fn trim(src: &[u8], mut from: usize, mut to: usize, set: &[u8]) -> (usize, usize) {
    while from < to && set.contains(&src[from]) {
        from += 1;
    }
    while to > from && set.contains(&src[to - 1]) {
        to -= 1;
    }
    (from, to)
}

/// Zig's `std.fmt.parseInt(u21, s, 16)`: an optional sign, hex digits with
/// `_` allowed between them (not first or last), and a `-` only before a
/// zero value.
fn zig_parse_hex(s: &[u8]) -> Option<u32> {
    let (neg, digits) = match s.first()? {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || digits[0] == b'_' || digits[digits.len() - 1] == b'_' {
        return None;
    }
    let mut v: u32 = 0;
    for &c in digits {
        if c == b'_' {
            continue;
        }
        let d = (c as char).to_digit(16)?;
        v = v.checked_mul(16)?.checked_add(d)?;
        if v > 0x1F_FFFF || (neg && v != 0) {
            return None;
        }
    }
    Some(v)
}

fn is_name_char(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_' || c == b'-'
}

fn is_number_char(c: u8) -> bool {
    c.is_ascii_digit()
        || (b'a'..=b'f').contains(&c)
        || (b'A'..=b'F').contains(&c)
        || matches!(c, b'x' | b'X' | b'o' | b'O' | b'b' | b'B' | b'.' | b'_' | b'+' | b'-')
}

fn is_tag_char(c: u8) -> bool {
    !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b',' | b':' | b'{' | b'}' | b'[' | b']' | b'"')
}

/// Whether `raw` (already filtered through `is_number_char`, so drawn from
/// `0-9a-fA-FxXoObB._+-`) can round-trip as a number. `parse_value` only ever
/// routes a *fresh* dispatch (no `~i`/`~f` marker in front) into `parse_number`
/// when the first byte is a digit or `.`; if the printer omits the override
/// marker (because the lexeme's implied kind already matches), that first
/// byte is all reparsing has to go on. A lexeme like a lone hex letter (`A`,
/// no `0x` prefix) sails through `is_number_char` but starts with a letter, so
/// it prints back out looking like — and reparses as — a bareword instead.
fn looks_like_number(raw: &[u8]) -> bool {
    let body = if !raw.is_empty() && (raw[0] == b'+' || raw[0] == b'-') { &raw[1..] } else { raw };
    match body.first() {
        None => false,
        Some(c) => c.is_ascii_digit() || *c == b'.',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::printer::print;
    use crate::kit::ast::{Builder, PathSegment};

    /// Assert that `input` round-trips: parse → print → parse yields an equal AST,
    /// and the re-printed text is byte-identical to the first print.
    fn expect_round_trip(input: &[u8]) {
        let ast = parse_abstract(input).unwrap();
        let mut out1 = Vec::new();
        print(&mut out1, &ast).unwrap();
        let reparsed = parse_abstract(&out1).unwrap();
        assert!(ast.eql(&reparsed));
        // Comments are excluded from `eql`, so assert them separately.
        assert!(ast.comments_eql(&reparsed));
        let mut out2 = Vec::new();
        print(&mut out2, &reparsed).unwrap();
        assert_eq!(String::from_utf8_lossy(&out1), String::from_utf8_lossy(&out2));
    }

    #[test]
    fn round_trips_comments_leading_trailing_line_block() {
        expect_round_trip(
            b"{\n  // leading on first entry\n  \"name\": \"fig\", // trailing on a value\n  /* a block comment */\n  \"port\": 8080,\n  \"nums\": [\n    // leading on a sequence element\n    1,\n    2 // trailing on the last element\n  ]\n}",
        );
    }

    #[test]
    fn round_trips_a_trailing_comment_on_an_empty_container_that_has_a_sibling() {
        expect_round_trip(b"{\n  \"a\": {}, // on an empty mapping\n  \"b\": [], // on an empty sequence\n  \"c\": 1\n}");
        expect_round_trip(b"[\n  [], // first\n  {} // last\n]");
        expect_round_trip(b"{} // on an inline root");
    }

    #[test]
    fn round_trips_a_comment_between_an_entrys_colon_and_its_value() {
        expect_round_trip(b"{\n  \"a\": /* block */ 1,\n  \"b\": // line\n    2,\n  \"c\": /* before a block */ [\n    3\n  ]\n}");
    }

    #[test]
    fn captures_comment_attachment_onto_the_right_nodes() {
        let ast = parse_abstract(b"{\n  // c1\n  \"a\": 1, // c2\n  \"b\": [2 /* c3 */]\n}").unwrap();
        let root = ast.node(ast.root);
        let first = root.kind.as_mapping().unwrap().unwrap();
        let kv_a = ast.kind_of(first).as_keyvalue().unwrap();
        // Leading binds to the key node, trailing to the value node.
        assert_eq!(b"c1", ast.comments(kv_a.key).leading[0].text);
        assert_eq!(Style::Line, ast.comments(kv_a.key).leading[0].style);
        assert_eq!(b"c2", ast.comments(kv_a.value).trailing.unwrap().text);

        let kv_b = ast.kind_of(ast.node(first).next_sibling.unwrap()).as_keyvalue().unwrap();
        let elem = ast.kind_of(kv_b.value).as_sequence().unwrap().unwrap();
        assert_eq!(b"c3", ast.comments(elem).trailing.unwrap().text);
        assert_eq!(Style::Block, ast.comments(elem).trailing.unwrap().style);
    }

    #[test]
    fn comment_free_document_carries_no_comment_table() {
        let ast = parse_abstract(b"[1, 2, 3]").unwrap();
        assert!(!ast.has_comments());
    }

    #[test]
    fn a_kind_override_with_no_actual_digits_is_rejected_not_silently_accepted() {
        assert_eq!(err!(UnexpectedToken), parse_abstract(b"~iA").unwrap_err());
        assert_eq!(err!(UnexpectedToken), parse_abstract(b"~fA").unwrap_err());
        // Legitimate hex, and overrides on real digits, still parse.
        expect_round_trip(b"0xFF");
        expect_round_trip(b"~i0xFF");
    }

    #[test]
    fn dangling_comments_before_a_root_containers_close_merge_with_trailing_eof_orphans() {
        let ast = parse_abstract(b"[\n  1,\n  // before close\n]\n// eof orphan\n").unwrap();
        let dangling = ast.comments(ast.root).dangling;
        assert_eq!(2, dangling.len());
        assert_eq!(b"before close", dangling[0].text);
        assert_eq!(b"eof orphan", dangling[1].text);
        expect_round_trip(b"[\n  1,\n  // before close\n]\n// eof orphan\n");
    }

    #[test]
    fn orphan_comments_are_captured_as_the_containers_dangling_run() {
        expect_round_trip(b"[]");
        let ast = parse_abstract(b"[\n  // orphan\n]").unwrap();
        assert_eq!(b"orphan", ast.comments(ast.root).dangling[0].text);
        expect_round_trip(b"{\n  \"a\": 1\n  // dangling at end\n}");
    }

    #[test]
    fn container_comment_opening_line_is_head_closing_line_is_bottom() {
        expect_round_trip(b"{\n  \"a\": [ // head\n    1\n  ],\n  \"b\": [\n    2\n    // tail\n  ]\n}");
    }

    #[test]
    fn round_trips_scalars_containers_and_node_keys() {
        expect_round_trip(
            b"{\n  \"name\": \"fig\",\n  \"port\": 8080,\n  \"ratio\": 1.0,\n  \"hex\": 0xFF,\n  \"grouped\": 1_000,\n  \"signed\": +42,\n  \"nums\": [1, 2.5, .5, 5., 1e9],\n  \"missing\": null,\n  \"flag\": true,\n  \"nested\": { \"a\": [true, false] }\n}",
        );
    }

    #[test]
    fn round_trips_extended_scalars() {
        expect_round_trip(
            b"{\n  \"dt\": @offset_datetime \"1979-05-27T07:32:00Z\",\n  \"d\": @local_date \"1979-05-27\",\n  \"mode\": @enum_literal \"fast\",\n  \"ch\": @char_literal \"65\",\n  \"inf\": @number_special \"Infinity\"\n}",
        );
    }

    #[test]
    fn round_trips_anchors_aliases_and_tags() {
        expect_round_trip(b"[&base { \"retries\": 3 }, *base, !!str \"tagged\"]");
    }

    #[test]
    fn string_escapes_decode_and_re_encode() {
        let ast = parse_abstract(b"\"tab:\\t quote:\\\" backslash:\\\\ ctrl:\\u0007\"").unwrap();
        assert_eq!(b"tab:\t quote:\" backslash:\\ ctrl:\x07", ast.kind_of(ast.root).as_string().unwrap());
    }

    #[test]
    fn non_string_mapping_keys() {
        let ast = parse_abstract(b"{ [1, 2]: \"tuple\" }").unwrap();
        let first = ast.kind_of(ast.root).as_mapping().unwrap().unwrap();
        let kv = ast.kind_of(first).as_keyvalue().unwrap();
        assert!(matches!(ast.kind_of(kv.key), NodeKind::Sequence(_)));
    }

    #[test]
    fn kind_override_survives_a_lexeme_kind_mismatch() {
        // A float whose lexeme reads as an integer: only `~f` preserves it.
        let mut b = Builder::init();
        let root = b.add_number_raw(b"1", true).unwrap(); // raw "1", but kind=float
        let ast = b.finish(root).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast).unwrap();
        assert_eq!(b"~f1\n", &out[..]);
        let reparsed = parse_abstract(&out).unwrap();
        assert_eq!(NumberKind::Float, reparsed.kind_of(reparsed.root).as_number().unwrap().kind);
    }

    #[test]
    fn alias_resolves_to_its_anchor() {
        let ast = parse_abstract(b"[&a 1, *a]").unwrap();
        let first = ast.kind_of(ast.root).as_sequence().unwrap().unwrap();
        let alias_node = ast.node(ast.node(first).next_sibling.unwrap());
        assert!(matches!(alias_node.kind, NodeKind::Alias(_)));
        let target = ast.resolve_alias(&alias_node).unwrap();
        assert_eq!(b"1", ast.kind_of(target).as_number().unwrap().raw);
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert_eq!(err!(TrailingGarbage), parse_abstract(b"1 2").unwrap_err());
    }

    #[test]
    fn a_non_container_roots_trailing_eof_orphan_comments_round_trip_as_dangling() {
        expect_round_trip(b"1\n// eof orphan");
        let ast = parse_abstract(b"1\n// a\n// b").unwrap();
        let dangling = ast.comments(ast.root).dangling;
        assert_eq!(2, dangling.len());
        assert_eq!(b"a", dangling[0].text);
        assert_eq!(b"b", dangling[1].text);
    }

    #[test]
    fn a_claimed_dangling_run_doesnt_leak_when_a_later_error_unwinds_the_parser() {
        assert_eq!(err!(TrailingGarbage), parse_abstract(b"[\n  // c\n]x").unwrap_err());
    }

    #[test]
    fn bounds_nesting_depth() {
        // `n` nested sequences: `[`×n then `]`×n (innermost is an empty `[]`).
        fn nest(n: usize) -> Vec<u8> {
            let mut buf = vec![b'['; n];
            buf.resize(n * 2, b']');
            buf
        }
        // At the limit it parses; one level past it is rejected (not a stack crash).
        parse_abstract(&nest(MAX_DEPTH)).unwrap();
        assert_eq!(err!(NestingTooDeep), parse_abstract(&nest(MAX_DEPTH + 1)).unwrap_err());
    }

    #[test]
    fn a_unicode_escape_reads_as_zig_parse_int_does() {
        // `std.fmt.parseInt` takes a sign and `_` between digits.
        assert_eq!(b"A", parse_abstract(b"\"\\u+041\"").unwrap().kind_of(0).as_string().unwrap());
        assert_eq!(b"A", parse_abstract(b"\"\\u0_41\"").unwrap().kind_of(0).as_string().unwrap());
        assert_eq!(err!(InvalidUnicodeEscape), parse_abstract(b"\"\\u_041\"").unwrap_err());
        assert_eq!(err!(InvalidUnicodeEscape), parse_abstract(b"\"\\uD800\"").unwrap_err());
        let _ = PathSegment::Index(0);
    }
}
