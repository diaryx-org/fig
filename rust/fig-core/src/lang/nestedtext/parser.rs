//! Port of languages/nestedtext/src/parser.zig.
//!
//! The parser turns NestedText-formatted bytes into the node table fig hands
//! it, a row per node (`RowWriter`, through `kit::flat_map`).
//!
//! Every scalar is an untyped `string` — NestedText, like INI, never coerces
//! a value's type at the format layer (see `nestedtext.org`'s design
//! philosophy: "strings all the way down"). Containers nest to arbitrary
//! depth via indentation (`mapping`/`sequence`), same as YAML/TOML, but with
//! none of their lexical complexity: rest-of-line values are 100% literal
//! (no escapes, no quoting, no reinterpretation — confirmed by the official
//! conformance suite: a value starting with `[` after `key: ` stays the
//! literal string, never an inline list).
//!
//! ── Algorithm sketch ─────────────────────────────────────────────────────
//! `parse_region(parent_indent)` parses "the value that belongs here" —
//! either the whole top-level document (`parent_indent == None`, value
//! indent must be exactly 0) or the nested block under an item whose
//! same-line value was left empty (`parent_indent = Some(item's own
//! indent)`, value indent must be greater). Two cases:
//!   1. The region's first line is `Other`-kind and starts with `{`/`[`
//!      (never a valid dict-item key — NestedText forbids a key from
//!      starting with those chars) → parse it as a single-line inline value
//!      (`parse_inline_line`), then require nothing else follows in this
//!      region (else `ExtraContent`).
//!   2. Otherwise → `parse_container_at`, a normal block dispatch to a list
//!      (`Dash` siblings), dict (`Colon` multiline-key runs and/or `Other`
//!      `key: value` siblings, freely intermixed), or string (`Gt` siblings)
//!      block.
//!
//! Partial-dedent detection needs no explicit indent stack: each block's
//! sibling loop only continues while `line.indent == indent`, breaks
//! (returns to its caller) when `line.indent < indent`, and errors when
//! `line.indent > indent` (a line that skipped past this level without
//! landing on it) — recursion through the call stack does the rest, exactly
//! as it would for hand-nested parens.
//!
//! Inline `{...}`/`[...]` forms get their own tiny scanner
//! (`parse_inline_*`/`scan_inline_value`) operating on absolute byte offsets
//! into `source`; see its section below for the delimiter-error rules
//! reverse-engineered from the official conformance suite (`{a:0,}` and
//! friends).

use std::sync::Arc;

use super::tokenizer::{Kind, Line, Tokenizer};
use super::Type;
use crate::err;
use crate::kit::ast::{Comment, NodeId, NodeKind, AST};
use crate::kit::document::Document;
use crate::kit::flat_map::{self, DuplicatePolicy, NodeArena};
use crate::kit::parse_diagnostic;
use crate::kit::row_writer::RowWriter;
use crate::kit::span::Span;
use crate::kit::table::{self, Table};
use crate::{Error, Result};

/// `ParseError` (with the tokenizer's `InvalidIndentChar` and
/// `OutOfMemory`): `InvalidUtf8`, `TopLevelIndent`, `InvalidIndentation`,
/// `ExpectedDictItem`, `ExpectedListItem`, `UnrecognizedLine`,
/// `MissingValue`, `MultilineKeyNoValue`, `MultilineKeyBadValue`,
/// `DuplicateKey`, `ExtraContent`, `ExtraCharsAfterDelim`, `UnclosedDelim`,
/// `ExpectedColon`, `ExpectedCommaOrClose`.
pub const PARSE_ERRORS: &[&str] = &[
    "InvalidUtf8",
    "TopLevelIndent",
    "InvalidIndentation",
    "ExpectedDictItem",
    "ExpectedListItem",
    "UnrecognizedLine",
    "MissingValue",
    "MultilineKeyNoValue",
    "MultilineKeyBadValue",
    "DuplicateKey",
    "ExtraContent",
    "ExtraCharsAfterDelim",
    "UnclosedDelim",
    "ExpectedColon",
    "ExpectedCommaOrClose",
];

pub fn describe(code: Error) -> &'static [u8] {
    match code.name() {
        "InvalidUtf8" => b"this file is not valid UTF-8; NestedText documents must be UTF-8 encoded",
        "InvalidIndentChar" => b"indentation must use plain spaces; a tab or other whitespace character is not allowed here",
        "TopLevelIndent" => b"top-level content must start in column 1",
        "InvalidIndentation" => b"this line's indentation does not match any enclosing block (partial dedent)",
        "ExpectedDictItem" => b"expected a dictionary item (`key: value` or a `: multiline key` line) here",
        "ExpectedListItem" => b"expected a list item (`- value`) here",
        "UnrecognizedLine" => b"this line is not a valid dictionary item, list item, string item, or comment",
        "MissingValue" => b"expected a value here",
        "MultilineKeyNoValue" => b"a multiline key requires a value on a more-indented line",
        "MultilineKeyBadValue" => b"the value of a multiline key must be on a more-indented line",
        "DuplicateKey" => b"this key is already defined in this mapping",
        "ExtraContent" => b"unexpected content after the document's value",
        "ExtraCharsAfterDelim" => b"unexpected content after the closing `}`/`]`",
        "UnclosedDelim" => b"this line ended without a closing `}`/`]`",
        "ExpectedColon" => b"expected `:` after this inline dictionary key",
        "ExpectedCommaOrClose" => b"expected `,` or a closing `}`/`]` here",
        _ => b"out of memory",
    }
}

pub fn short_label(code: Error) -> &'static [u8] {
    match code.name() {
        "InvalidUtf8" => b"invalid UTF-8",
        "InvalidIndentChar" => b"invalid indentation",
        "TopLevelIndent" => b"bad top-level indent",
        "InvalidIndentation" => b"bad indentation",
        "ExpectedDictItem" => b"expected dictionary item",
        "ExpectedListItem" => b"expected list item",
        "UnrecognizedLine" => b"unrecognized line",
        "MissingValue" => b"expected value",
        "MultilineKeyNoValue" => b"missing value",
        "MultilineKeyBadValue" => b"missing value",
        "DuplicateKey" => b"duplicate key",
        "ExtraContent" => b"extra content",
        "ExtraCharsAfterDelim" => b"extra characters",
        "UnclosedDelim" => b"unclosed delimiter",
        "ExpectedColon" => b"expected `:`",
        "ExpectedCommaOrClose" => b"expected `,` or close",
        _ => b"out of memory",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Error,
    pub offset: usize,
    pub end: Option<usize>,
}

impl Diagnostic {
    pub fn locate(self, source: &[u8]) -> parse_diagnostic::Location<'_> {
        parse_diagnostic::locate_offset(source, self.offset)
    }

    pub fn render_alloc(self, source: &[u8], file: &[u8]) -> Vec<u8> {
        parse_diagnostic::render_report_alloc(source, self.offset, file, b"error", describe(self.code))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    pub diag: Option<Diagnostic>,
}

/// `input` as a `Document`: its table written and adopted.
pub fn parse(input: &[u8], format: Type) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_into(&mut t, format)?;
    RowWriter::adopt(source, &mut t)
}

/// `parse`, reporting as `parse_with_report_into` does.
pub fn parse_with_report(input: &[u8], format: Type, out: &mut Report) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_with_report_into(&mut t, format, out)?;
    RowWriter::adopt(source, &mut t)
}

/// Write the rows of `t`'s source into `t`, the table fig hands a parse.
pub fn parse_into(t: &mut Table, format: Type) -> Result<()> {
    parse_impl(t, format, None)
}

/// `parse_into`, but also fills `out.diag` on failure. On an error the table
/// is not finished.
pub fn parse_with_report_into(t: &mut Table, format: Type, out: &mut Report) -> Result<()> {
    parse_impl(t, format, Some(out))
}

/// `input` as an `AST`, alone.
pub fn parse_abstract(input: &[u8], format: Type) -> Result<AST> {
    Ok(parse(input, format)?.ast)
}

fn parse_impl(t: &mut Table, format: Type, out: Option<&mut Report>) -> Result<()> {
    let source = t.source_arc();
    let mut parser = Parser {
        version: format,
        source,
        lines: Vec::new(),
        pos: 0,
        arena: NodeArena::init(t),
        pending_leading: Vec::new(),
        ipos: 0,
        iend: 0,
        fail_offset: 0,
        fail_end: None,
    };
    let result = parser.parse_once(format);
    result.inspect_err(|&err| {
        if let Some(o) = out {
            o.diag = Some(Diagnostic { code: err, offset: parser.fail_offset, end: parser.fail_end });
        }
    })
}

struct Parser<'t> {
    version: Type,
    source: Arc<[u8]>,
    lines: Vec<Line>,
    pos: usize,
    arena: NodeArena<'t>,
    /// The pending leading comments, by their text's span in `source`.
    pending_leading: Vec<Span>,

    /// Absolute cursor into `source` used only while scanning an inline
    /// `{...}`/`[...]` line (see the "Inline values" section). `iend` is the
    /// exclusive end of that line's content span.
    ipos: usize,
    iend: usize,

    fail_offset: usize,
    fail_end: Option<usize>,
}

/// What a region with no deeper content resolves to: the whole document
/// (empty file → `null`), a plain item's value (empty rest-of-line AND no
/// nested block → the empty STRING, per the official conformance suite —
/// `key:`/`-` alone is not an error, only a *multiline* key strictly
/// requires a value; see `MissingBehavior::Err` below), or a multiline key's
/// value (which DOES strictly require one — the caller pre-checks and picks
/// the right of the two dedicated error codes, so `Err` here is only ever a
/// defensive fallback, never actually reached).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissingBehavior {
    NullDocument,
    EmptyString,
    Err,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Close {
    Brace,
    Bracket,
    None,
}

fn close_char(ctx: Close) -> u8 {
    match ctx {
        Close::Brace => b'}',
        Close::Bracket => b']',
        Close::None => 0,
    }
}

fn is_inline_stop(c: u8) -> bool {
    c == b',' || c == b'{' || c == b'}' || c == b'[' || c == b']'
}

/// `std.mem.trimEnd(u8, s, set)` for a set of single bytes and the
/// two-byte NBSP (`trimEnd` trims any byte in the set, so `\xC2\xA0` is the
/// bytes 0xC2 and 0xA0 each).
fn trim_end<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let end = s.iter().rposition(|c| !set.contains(c)).map_or(0, |e| e + 1);
    &s[..end]
}

impl<'t> Parser<'t> {
    fn parse_once(&mut self, format: Type) -> Result<()> {
        self.version = format;

        if std::str::from_utf8(&self.source).is_err() {
            return Err(err!(InvalidUtf8));
        }

        let source = self.source.clone();
        let mut tokenizer = Tokenizer::new(&source);
        self.lines = match tokenizer.tokenize() {
            Ok(l) => l,
            Err(e) => {
                self.fail_offset = tokenizer.i;
                return Err(e);
            }
        };

        let root_id = self.parse_region(None, MissingBehavior::NullDocument)?;
        self.skip_blank();
        if !self.at_end() {
            return Err(self.fail_at_line(self.peek(), err!(ExtraContent)));
        }
        self.claim_dangling(root_id)?;

        self.arena.w.finish(root_id)
    }

    // ── Line cursor ──────────────────────────────────────────────────────

    fn peek(&self) -> Line {
        self.lines[self.pos]
    }

    fn at_end(&self) -> bool {
        self.peek().kind == Kind::EndOfFile
    }

    fn skip_blank(&mut self) {
        loop {
            match self.peek().kind {
                Kind::Comment => {
                    self.capture_comment(self.peek());
                    self.pos += 1;
                }
                Kind::Blank => self.pos += 1,
                _ => return,
            }
        }
    }

    fn capture_comment(&mut self, l: Line) {
        self.pending_leading.push(l.content);
    }

    fn write_comment(&mut self, id: NodeId, slot: u8, text: Span) -> Result<()> {
        let source = self.source.clone();
        self.arena.w.comment(id, slot, Comment::line(&source[text.start..text.end]))
    }

    fn claim_leading(&mut self, id: NodeId) -> Result<()> {
        let pending = std::mem::take(&mut self.pending_leading);
        for c in &pending {
            self.write_comment(id, table::COMMENT_LEADING, *c)?;
        }
        Ok(())
    }

    /// Snapshot-and-clear the currently pending leading comments, WITHOUT
    /// attaching them to a node yet. Split out of `claim_leading` for
    /// `parse_list_block`: a list item has no node of its own to claim onto
    /// until AFTER `parse_item_value` returns (unlike a dict item, which
    /// creates its key node — and calls `claim_leading` on it — before
    /// parsing its value), and if that value is itself a nested container,
    /// its own first entry would otherwise call `claim_leading` FIRST and
    /// steal the comment meant for the outer list item. Callers on this path
    /// snapshot before recursing into `parse_item_value`, then
    /// `attach_leading` once the item's real node id is known.
    fn take_leading(&mut self) -> Option<Vec<Span>> {
        if self.pending_leading.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut self.pending_leading))
    }

    /// Write a `take_leading` snapshot as `id`'s leading comments.
    fn attach_leading(&mut self, id: NodeId, leading: Option<Vec<Span>>) -> Result<()> {
        let Some(owned) = leading else { return Ok(()) };
        for c in &owned {
            self.write_comment(id, table::COMMENT_LEADING, *c)?;
        }
        Ok(())
    }

    fn claim_dangling(&mut self, id: NodeId) -> Result<()> {
        let pending = std::mem::take(&mut self.pending_leading);
        for c in &pending {
            self.write_comment(id, table::COMMENT_DANGLING, *c)?;
        }
        Ok(())
    }

    fn fail_at_line(&mut self, l: Line, err: Error) -> Error {
        self.fail_offset = l.line_start;
        self.fail_end = Some(l.content.end);
        err
    }

    fn fail_here(&mut self, err: Error) -> Error {
        self.fail_offset = self.peek().line_start;
        err
    }

    /// Link `value_id` after `last`, sequence `seq_id`'s last item so far, or
    /// as its first when `last` is `None`; returns the new last item.
    fn append_seq_item(&mut self, seq_id: NodeId, last: Option<NodeId>, value_id: NodeId) -> Result<NodeId> {
        match last {
            Some(l) => self.arena.w.set_next(l, Some(value_id)),
            None => self.arena.w.set(seq_id, NodeKind::Sequence(Some(value_id)))?,
        }
        Ok(value_id)
    }

    /// Set node `id`'s span to end at `end`.
    fn set_end(&mut self, id: NodeId, end: usize) {
        let start = self.arena.w.span_of(id).start;
        self.arena.w.set_span(id, Span::new(start, end));
    }

    fn add_string(&mut self, text: Span, span: Span) -> Result<NodeId> {
        let source = self.source.clone();
        self.arena.add_node(NodeKind::String(&source[text.start..text.end]), span)
    }

    // ── Regions and blocks ───────────────────────────────────────────────

    /// Parse the value belonging at this position: the whole document
    /// (`parent_indent == None`) or a nested value-region under an item
    /// whose same-line value was empty (`parent_indent = Some(item
    /// indent)`). See the module doc comment for the algorithm.
    fn parse_region(&mut self, parent_indent: Option<usize>, on_missing: MissingBehavior) -> Result<NodeId> {
        self.skip_blank();
        let no_content = self.at_end() || parent_indent.is_some_and(|p| self.peek().indent <= p);
        if no_content {
            let len = self.source.len();
            return match on_missing {
                MissingBehavior::NullDocument => self.arena.add_node(NodeKind::Null, Span::new(len, len)),
                MissingBehavior::EmptyString => {
                    let at = if self.at_end() { len } else { self.peek().line_start };
                    self.arena.add_node(NodeKind::String(b""), Span::new(at, at))
                }
                MissingBehavior::Err => Err(self.fail_here(err!(MissingValue))),
            };
        }

        let first = self.peek();
        let region_indent = first.indent;
        if parent_indent.is_none() && region_indent != 0 {
            return Err(self.fail_at_line(first, err!(TopLevelIndent)));
        }

        if first.kind == Kind::Other && self.starts_with_bracket(first) {
            let node = self.parse_inline_line(first)?;
            self.pos += 1;
            self.skip_blank();
            let still_in_region = !self.at_end() && parent_indent.is_none_or(|p| self.peek().indent > p);
            if still_in_region {
                return Err(self.fail_at_line(self.peek(), err!(ExtraContent)));
            }
            return Ok(node);
        }

        self.parse_container_at(region_indent)
    }

    fn starts_with_bracket(&self, l: Line) -> bool {
        if l.content.is_empty() {
            return false;
        }
        let c = self.source[l.content.start];
        c == b'{' || c == b'['
    }

    /// First occurrence of `: ` or a trailing `:` at end-of-line within an
    /// `Other` line's content — the dict-item key/value split point. Never
    /// matches at offset 0 (an `Other` line never starts with `:`; that's
    /// the dedicated `Colon` tag kind instead). `(key_end, val_start)`.
    fn split_dict_item(&self, l: Line) -> Option<(usize, usize)> {
        let mut i = l.content.start;
        while i < l.content.end {
            if self.source[i] == b':' {
                if i + 1 == l.content.end {
                    return Some((i, l.content.end));
                }
                if self.source[i + 1] == b' ' {
                    return Some((i, i + 2));
                }
            }
            i += 1;
        }
        None
    }

    fn is_dict_item_line(&self, l: Line) -> bool {
        if self.starts_with_bracket(l) {
            return false; // a key can never start with `{`/`[`
        }
        self.split_dict_item(l).is_some()
    }

    fn parse_container_at(&mut self, indent: usize) -> Result<NodeId> {
        let first = self.peek();
        match first.kind {
            Kind::Dash => self.parse_list_block(indent),
            Kind::Gt => self.parse_string_block(indent),
            Kind::Colon => self.parse_dict_block(indent),
            Kind::Other => {
                if self.is_dict_item_line(first) {
                    self.parse_dict_block(indent)
                } else {
                    Err(self.fail_at_line(first, err!(UnrecognizedLine)))
                }
            }
            // skip_blank already advanced past these
            Kind::Blank | Kind::Comment | Kind::EndOfFile => unreachable!(),
        }
    }

    /// The value of a dash/dict-item whose rest-of-line content is
    /// `content`: literal same-line text if non-empty; else the nested
    /// value-region at `container_indent` if one follows (deeper indent);
    /// else the empty STRING — a bare `key:`/`-` with nothing more indented
    /// after it is NOT an error (confirmed by the official conformance
    /// suite, e.g. `key2:` at end-of-file loads as `""`); only a *multiline*
    /// key strictly requires a value (see `parse_dict_block`'s `Colon`
    /// branch).
    fn parse_item_value(&mut self, content: Span, container_indent: usize) -> Result<NodeId> {
        if !content.is_empty() {
            return self.add_string(content, content);
        }
        self.parse_region(Some(container_indent), MissingBehavior::EmptyString)
    }

    fn parse_list_block(&mut self, indent: usize) -> Result<NodeId> {
        let start = self.peek().line_start;
        let seq_id = self.arena.add_node(NodeKind::Sequence(None), Span::new(start, start))?;
        let mut end = start;
        let mut last: Option<NodeId> = None;
        loop {
            self.skip_blank();
            if self.at_end() {
                break;
            }
            let l = self.peek();
            if l.indent < indent {
                break;
            }
            if l.indent > indent {
                return Err(self.fail_at_line(l, err!(InvalidIndentation)));
            }
            if l.kind != Kind::Dash {
                return Err(self.fail_at_line(l, err!(ExpectedListItem)));
            }
            self.pos += 1;
            // Snapshot the item's own leading comment BEFORE recursing into
            // its value: a list item has no node of its own until
            // `parse_item_value` returns, and if the value is itself a nested
            // container, its first entry would otherwise claim this comment
            // first (see `take_leading`'s doc comment).
            let leading = self.take_leading();
            let value_id = self.parse_item_value(l.content, indent)?;
            self.attach_leading(value_id, leading)?;
            // The item's `-` sits right after its indentation; its value may
            // begin lines below (nested or empty), so record the marker.
            let dash = l.line_start + l.indent;
            self.arena.w.marker(value_id, Span::new(dash, dash + 1))?;
            last = Some(self.append_seq_item(seq_id, last, value_id)?);
            end = self.arena.w.span_of(value_id).end;
        }
        self.set_end(seq_id, end);
        Ok(seq_id)
    }

    fn parse_dict_block(&mut self, indent: usize) -> Result<NodeId> {
        let start = self.peek().line_start;
        let map_id = self.arena.add_node(NodeKind::Mapping(None), Span::new(start, start))?;
        let mut end = start;
        loop {
            self.skip_blank();
            if self.at_end() {
                break;
            }
            let l = self.peek();
            if l.indent < indent {
                break;
            }
            if l.indent > indent {
                return Err(self.fail_at_line(l, err!(InvalidIndentation)));
            }

            if l.kind == Kind::Colon {
                let (mk_text, mk_span) = self.collect_multiline_key(indent);
                let key_id = self.arena.add_node(NodeKind::String(&mk_text), mk_span)?;
                self.claim_leading(key_id)?;
                if self.at_end() {
                    return Err(self.fail_here(err!(MultilineKeyNoValue)));
                }
                if self.peek().indent <= indent {
                    return Err(self.fail_at_line(self.peek(), err!(MultilineKeyBadValue)));
                }
                // unreachable fallback: already validated deeper content exists
                let value_id = self.parse_region(Some(indent), MissingBehavior::Err)?;
                let put = flat_map::put_entry(&mut self.arena, map_id, key_id, value_id, DuplicatePolicy::Err)?;
                // A multiline key has no separator token; its value's tail
                // starts right after the key, so record a zero-width
                // separator there — the editor still reframes the value (see
                // the plain arm below).
                self.arena.w.sep(put.kv, Span::new(mk_span.end, mk_span.end))?;
                end = self.arena.w.span_of(value_id).end;
            } else if l.kind == Kind::Other && self.is_dict_item_line(l) {
                let (key_end, val_start) = self.split_dict_item(l).expect("a dict item line");
                self.pos += 1;
                // "Spaces between key and tag are ignored" — trim trailing
                // whitespace (space, tab, AND a trailing U+00A0 NO-BREAK
                // SPACE — the official suite exercises a tab and an NBSP
                // right before the `:`, e.g. `k4\t: v4`/`key 3\xc2\xa0: value
                // 3`) the raw key/tag scan left in (leading is already
                // impossible: `l.content.start` is the line's first non-space
                // byte). The VALUE is never trimmed — rest-of-line text is
                // always 100% literal, tabs included.
                let key_len = trim_end(&self.source[l.content.start..key_end], b" \t\xC2\xA0").len();
                let key_text = Span::new(l.content.start, l.content.start + key_len);
                let key_id = self.add_string(key_text, key_text)?;
                self.claim_leading(key_id)?;
                let value_id = self.parse_item_value(Span::new(val_start, l.content.end), indent)?;
                let put = flat_map::put_entry(&mut self.arena, map_id, key_id, value_id, DuplicatePolicy::Err)?;
                // The entry's `:` is its separator, which is what lets the
                // editor reframe the value.
                self.arena.w.sep(put.kv, Span::new(key_end, key_end + 1))?;
                end = self.arena.w.span_of(value_id).end;
            } else {
                return Err(self.fail_at_line(l, err!(ExpectedDictItem)));
            }
        }
        self.set_end(map_id, end);
        Ok(map_id)
    }

    /// The next line INDEX at exactly `indent` with kind `kind`, skipping
    /// over any number of intervening blank/comment lines (which are
    /// transparent inside a `>`/`: ` multiline run — confirmed by the
    /// official conformance suite: a `#`-comment or blank line strictly
    /// between two `>` lines contributes nothing to the joined value, not
    /// even an extra blank line). Returns `None`, WITHOUT moving `self.pos`,
    /// when no such line follows (the run has genuinely ended — the skipped
    /// blank/comment lines are left for the enclosing container's own
    /// `skip_blank` to claim).
    fn probe_next(&self, kind: Kind, indent: usize) -> Option<usize> {
        let mut i = self.pos;
        while i < self.lines.len() && (self.lines[i].kind == Kind::Blank || self.lines[i].kind == Kind::Comment) {
            i += 1;
        }
        if i >= self.lines.len() {
            return None;
        }
        let l = self.lines[i];
        if l.kind != kind || l.indent != indent {
            return None;
        }
        Some(i)
    }

    fn parse_string_block(&mut self, indent: usize) -> Result<NodeId> {
        let mut buf: Vec<u8> = Vec::new();
        let start = self.peek().line_start;
        let mut end = start;
        let mut first = true;
        while let Some(i) = self.probe_next(Kind::Gt, indent) {
            self.pos = i;
            let l = self.lines[self.pos];
            if !first {
                buf.push(b'\n');
            }
            first = false;
            buf.extend_from_slice(&self.source[l.content.start..l.content.end]);
            end = l.content.end;
            self.pos += 1;
        }
        self.arena.add_node(NodeKind::String(&buf), Span::new(start, end))
    }

    /// Consecutive `Colon`-tagged lines at `indent` — see `probe_next` for
    /// how intervening blank/comment lines are skipped transparently.
    fn collect_multiline_key(&mut self, indent: usize) -> (Vec<u8>, Span) {
        let mut buf: Vec<u8> = Vec::new();
        let start = self.peek().line_start;
        let mut end = start;
        let mut first = true;
        while let Some(i) = self.probe_next(Kind::Colon, indent) {
            self.pos = i;
            let l = self.lines[self.pos];
            if !first {
                buf.push(b'\n');
            }
            first = false;
            buf.extend_from_slice(&self.source[l.content.start..l.content.end]);
            end = l.content.end;
            self.pos += 1;
        }
        (buf, Span::new(start, end))
    }

    // ── Inline values (`{...}`/`[...]`) ──────────────────────────────────
    // Only ever reached as the ENTIRE content of one line: a top-level
    // document that's a single `{...}`/`[...]` line, or the sole line of a
    // nested value-region under an item that left its same-line value empty.
    // Never recognized after `key: ` on the same line (rest-of-line values
    // are always literal — see the module doc comment). Delimiter-error rules
    // below are reverse-engineered from the official conformance suite (in
    // fig-conformance, cases `emanate`/`conclude`/`baptism`/`protrude`/
    // `collate` etc.): a trailing comma before a closing delimiter is
    // `MissingValue` ("expected a value here"), a value/key that runs off the
    // end of the line without a delimiter is `UnclosedDelim`, a raw string
    // value/key that hits a stray bracket/brace mid-scan (not at its very
    // start, where it would instead recurse into a nested container) is
    // `ExpectedCommaOrClose`/`ExpectedColon`.

    fn parse_inline_line(&mut self, l: Line) -> Result<NodeId> {
        self.ipos = l.content.start;
        self.iend = l.content.end;
        let node = self.parse_inline_value(Close::None)?;
        self.skip_inline_ws();
        if self.ipos < self.iend {
            self.fail_offset = self.ipos;
            return Err(err!(ExtraCharsAfterDelim));
        }
        Ok(node)
    }

    fn skip_inline_ws(&mut self) {
        // Space AND tab — the official suite exercises tabs as free-form
        // whitespace around every inline delimiter (`,`/`:`/`{`/`[`/`}`/`]`).
        while self.ipos < self.iend && (self.source[self.ipos] == b' ' || self.source[self.ipos] == b'\t') {
            self.ipos += 1;
        }
    }

    fn fail_inline(&mut self, err: Error) -> Error {
        self.fail_offset = self.ipos;
        err
    }

    fn parse_inline_value(&mut self, ctx: Close) -> Result<NodeId> {
        self.skip_inline_ws();
        if self.ipos >= self.iend {
            return Err(self.fail_inline(err!(UnclosedDelim)));
        }
        match self.source[self.ipos] {
            b'{' => self.parse_inline_dict(),
            b'[' => self.parse_inline_list(),
            _ => self.scan_inline_value(ctx),
        }
    }

    fn scan_inline_value(&mut self, ctx: Close) -> Result<NodeId> {
        let start = self.ipos;
        while self.ipos < self.iend && !is_inline_stop(self.source[self.ipos]) {
            self.ipos += 1;
        }
        let raw_end = self.ipos;
        if self.ipos < self.iend {
            let stop = self.source[self.ipos];
            if stop != b',' && stop != close_char(ctx) {
                return Err(self.fail_inline(err!(ExpectedCommaOrClose)));
            }
        }
        let text_len = trim_end(&self.source[start..raw_end], b" \t").len();
        self.add_string(Span::new(start, start + text_len), Span::new(start, raw_end))
    }

    fn parse_inline_entry(&mut self, map_id: NodeId) -> Result<()> {
        self.skip_inline_ws();
        let key_start = self.ipos;
        while self.ipos < self.iend && self.source[self.ipos] != b':' && !is_inline_stop(self.source[self.ipos]) {
            self.ipos += 1;
        }
        if self.ipos >= self.iend {
            return Err(self.fail_inline(err!(UnclosedDelim)));
        }
        if self.source[self.ipos] != b':' {
            return Err(self.fail_inline(err!(ExpectedColon)));
        }
        let key_len = trim_end(&self.source[key_start..self.ipos], b" \t").len();
        let key_id = self.add_string(Span::new(key_start, key_start + key_len), Span::new(key_start, self.ipos))?;
        self.ipos += 1; // consume ':'
        let value_id = self.parse_inline_value(Close::Brace)?;
        flat_map::put_entry(&mut self.arena, map_id, key_id, value_id, DuplicatePolicy::Err)?;
        Ok(())
    }

    fn parse_inline_dict(&mut self) -> Result<NodeId> {
        let start = self.ipos;
        self.ipos += 1; // consume '{'
        let map_id = self.arena.add_node(NodeKind::Mapping(None), Span::new(start, start))?;
        if self.ipos < self.iend && self.source[self.ipos] == b'}' {
            self.ipos += 1;
            self.set_end(map_id, self.ipos);
            return Ok(map_id);
        }
        let mut first = true;
        loop {
            if !first {
                self.skip_inline_ws();
                if self.ipos < self.iend && self.source[self.ipos] == b'}' {
                    return Err(self.fail_inline(err!(MissingValue)));
                }
            }
            first = false;
            self.parse_inline_entry(map_id)?;
            self.skip_inline_ws();
            if self.ipos >= self.iend {
                return Err(self.fail_inline(err!(UnclosedDelim)));
            }
            let d = self.source[self.ipos];
            if d == b',' {
                self.ipos += 1;
                continue;
            }
            if d == b'}' {
                self.ipos += 1;
                break;
            }
            return Err(self.fail_inline(err!(ExpectedCommaOrClose)));
        }
        self.set_end(map_id, self.ipos);
        Ok(map_id)
    }

    fn parse_inline_list(&mut self) -> Result<NodeId> {
        let start = self.ipos;
        self.ipos += 1; // consume '['
        let seq_id = self.arena.add_node(NodeKind::Sequence(None), Span::new(start, start))?;
        if self.ipos < self.iend && self.source[self.ipos] == b']' {
            self.ipos += 1;
            self.set_end(seq_id, self.ipos);
            return Ok(seq_id);
        }
        // Unlike a dict entry (which needs at least a key), a list element
        // has no minimum content — a comma boundary, including one
        // immediately before `]`, is simply an empty-string element. So
        // (unlike `parse_inline_dict`) there is no "trailing comma"
        // pre-check here: `[,]` is `["", ""]` and `[a,]` is `["a", ""]`,
        // confirmed by the official conformance suite (`epoch`/`geyser`).
        let mut last: Option<NodeId> = None;
        loop {
            let value_id = self.parse_inline_value(Close::Bracket)?;
            last = Some(self.append_seq_item(seq_id, last, value_id)?);
            self.skip_inline_ws();
            if self.ipos >= self.iend {
                return Err(self.fail_inline(err!(UnclosedDelim)));
            }
            let d = self.source[self.ipos];
            if d == b',' {
                self.ipos += 1;
                continue;
            }
            if d == b']' {
                self.ipos += 1;
                break;
            }
            return Err(self.fail_inline(err!(ExpectedCommaOrClose)));
        }
        self.set_end(seq_id, self.ipos);
        Ok(seq_id)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::PathSegment;

    const NT: Type = Type::NestedText;

    fn expect_root(input: &str, key: &str, value: &str) {
        let ast = parse_abstract(input.as_bytes(), NT).unwrap();
        let v = ast.get_val_by_path(&[PathSegment::Key(key.as_bytes())]).unwrap();
        assert_eq!(value.as_bytes(), v.kind.as_string().unwrap());
    }

    fn val(ast: &AST, path: &[PathSegment<'_>]) -> Vec<u8> {
        ast.get_val_by_path(path).unwrap().kind.as_string().unwrap().to_vec()
    }

    fn fails(input: &str) -> Error {
        parse_abstract(input.as_bytes(), NT).unwrap_err()
    }

    #[test]
    fn root_level_key_value() {
        expect_root("name: fig\n", "name", "fig");
    }

    #[test]
    fn rest_of_line_value_is_100_percent_literal_even_one_that_looks_like_an_inline_list() {
        expect_root("regex  : [+-]?([0-9]*[.])?[0-9]+\n", "regex", "[+-]?([0-9]*[.])?[0-9]+");
    }

    #[test]
    fn empty_document_is_null() {
        let ast = parse_abstract(b"", NT).unwrap();
        assert_eq!(NodeKind::Null, ast.node(ast.root).kind);

        let ast2 = parse_abstract(b"# just a comment\n\n", NT).unwrap();
        assert_eq!(NodeKind::Null, ast2.node(ast2.root).kind);
    }

    #[test]
    fn single_string_item() {
        let ast = parse_abstract(b"> hello\n", NT).unwrap();
        assert_eq!(b"hello", ast.node(ast.root).kind.as_string().unwrap());
    }

    #[test]
    fn two_blank_string_items_join_with_a_newline() {
        let ast = parse_abstract(b">\n>\n", NT).unwrap();
        assert_eq!(b"\n", ast.node(ast.root).kind.as_string().unwrap());
    }

    #[test]
    fn list_of_scalars() {
        let ast = parse_abstract(b"- a\n- b\n- c\n", NT).unwrap();
        assert_eq!(b"b", &val(&ast, &[PathSegment::Index(1)])[..]);
    }

    #[test]
    fn nested_block_under_an_empty_content_dict_item() {
        let ast = parse_abstract(b"server:\n    host: localhost\n    port: 80\n", NT).unwrap();
        assert_eq!(b"localhost", &val(&ast, &[PathSegment::Key(b"server"), PathSegment::Key(b"host")])[..]);
    }

    #[test]
    fn multiline_key() {
        let ast = parse_abstract(b": key 1\n: spread over 2 lines\n    > value 1\n", NT).unwrap();
        assert_eq!(b"value 1", &val(&ast, &[PathSegment::Key(b"key 1\nspread over 2 lines")])[..]);
    }

    #[test]
    fn empty_key_via_bare_multiline_key_form() {
        let ast = parse_abstract(b":\n  >\n", NT).unwrap();
        assert_eq!(b"", &val(&ast, &[PathSegment::Key(b"")])[..]);
    }

    #[test]
    fn simple_key_and_multiline_key_can_be_siblings_in_the_same_dict() {
        let ast = parse_abstract(
            b"here is a simple key: with a simple value\n: Here is a multiline key\n: with a list value.\n    - 0\n    - 1\n",
            NT,
        )
        .unwrap();
        assert_eq!(b"with a simple value", &val(&ast, &[PathSegment::Key(b"here is a simple key")])[..]);
        assert_eq!(
            b"1",
            &val(&ast, &[PathSegment::Key(b"Here is a multiline key\nwith a list value."), PathSegment::Index(1)])[..]
        );
    }

    #[test]
    fn top_level_inline_dict_list() {
        let ast = parse_abstract(b"{key 1: value 1, key 2: [value 2a, value 2b]}", NT).unwrap();
        assert_eq!(b"value 2b", &val(&ast, &[PathSegment::Key(b"key 2"), PathSegment::Index(1)])[..]);
    }

    #[test]
    fn empty_inline_dict_and_list() {
        let ast = parse_abstract(b"{}", NT).unwrap();
        assert_eq!(NodeKind::Mapping(None), ast.node(ast.root).kind);
        let ast2 = parse_abstract(b"[]", NT).unwrap();
        assert_eq!(NodeKind::Sequence(None), ast2.node(ast2.root).kind);
    }

    #[test]
    fn bracket_space_bracket_is_a_single_element_list_containing_one_empty_string() {
        let ast = parse_abstract(b"[ ]", NT).unwrap();
        assert_eq!(b"", &val(&ast, &[PathSegment::Index(0)])[..]);
        let first = ast.node(ast.root).kind.as_sequence().unwrap().unwrap();
        assert_eq!(None, ast.node(first).next_sibling);
    }

    #[test]
    fn brace_colon_brace_is_a_single_entry_with_an_empty_key_and_empty_value() {
        let ast = parse_abstract(b"{:}", NT).unwrap();
        assert_eq!(b"", &val(&ast, &[PathSegment::Key(b"")])[..]);
    }

    #[test]
    fn inline_value_nested_under_an_empty_content_list_item() {
        let ast = parse_abstract(b"-\n    {a:0}\n", NT).unwrap();
        assert_eq!(b"0", &val(&ast, &[PathSegment::Index(0), PathSegment::Key(b"a")])[..]);
    }

    #[test]
    fn duplicate_key_is_an_error() {
        assert_eq!(err!(DuplicateKey), fails("key: value 1\nkey: value 2\n"));
    }

    #[test]
    fn tab_in_indentation_is_an_error() {
        assert_eq!(err!(InvalidIndentChar), fails("ingredients:\n\t> green chilies\n"));
    }

    #[test]
    fn top_level_content_must_start_in_column_1() {
        assert_eq!(err!(TopLevelIndent), fails("  key: value\n"));
    }

    #[test]
    fn mixing_list_and_dict_siblings_at_the_same_indentation_is_an_error() {
        assert_eq!(err!(ExpectedDictItem), fails("ingredients:\n- green chilies\n"));
        assert_eq!(err!(ExpectedListItem), fails("ingredients:\n  - green chilies\n  cannot mix list with: dictionary\n"));
    }

    #[test]
    fn partial_dedent_to_an_indentation_that_matches_no_open_block_is_an_error() {
        assert_eq!(err!(InvalidIndentation), fails("a:\n    b:\n        c: 1\n  d: 2\n"));
    }

    #[test]
    fn inline_trailing_comma_before_a_closing_delimiter_is_expected_value() {
        assert_eq!(err!(MissingValue), fails("{a:0,}"));
    }

    #[test]
    fn inline_mismatched_bracket_brace_inside_a_value_is_a_delimiter_error() {
        assert_eq!(err!(ExpectedColon), fails("-\n    {a}\n"));
        assert_eq!(err!(UnclosedDelim), fails("-\n    {\n"));
    }

    #[test]
    fn top_level_singleton_bracket_line_followed_by_more_content_is_extra_content() {
        assert_eq!(err!(ExtraContent), fails("[]\nnutz: truck\n"));
    }

    #[test]
    fn leading_comment_attaches_to_the_following_key() {
        let ast = parse_abstract(b"# a header comment\nname: fig\n", NT).unwrap();
        let key_node = ast.first_child_key(&ast.node(ast.root)).unwrap().unwrap();
        let cs = ast.comments(key_node.id);
        assert_eq!(1, cs.leading.len());
        assert_eq!(b" a header comment", cs.leading[0].text);
    }

    #[test]
    fn leading_comment_above_a_list_item_attaches_to_the_item_not_a_descendant_of_its_nested_value() {
        // Regression: a list item has no node of its own until its value is
        // fully parsed, so a comment captured just above it must be
        // snapshotted BEFORE recursing into a nested value — otherwise that
        // value's own first entry (here, the `nested` key) claims it first.
        // See `take_leading`'s doc comment.
        let ast = parse_abstract(b"- a\n# middle item note\n-\n    nested: 1\n- c\n", NT).unwrap();
        let item = ast.get_val_by_path(&[PathSegment::Index(1)]).unwrap();
        let cs = ast.comments(item.id);
        assert_eq!(1, cs.leading.len());
        assert_eq!(b" middle item note", cs.leading[0].text);

        let nested_key = ast.first_child_key(&item).unwrap().unwrap();
        assert_eq!(0, ast.comments(nested_key.id).leading.len());
    }

    #[test]
    fn a_report_says_where_a_parse_failed() {
        let mut report = Report::default();
        let e = parse_with_report(b"a: 1\n  b: 2\n", NT, &mut report).unwrap_err();
        assert_eq!(err!(InvalidIndentation), e);
        let d = report.diag.unwrap();
        assert_eq!(5, d.offset);
        assert_eq!(Some(11), d.end);
        assert_eq!(b"bad indentation", short_label(d.code));
    }
}
