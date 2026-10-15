//! Port of languages/yaml/src/tokenizer.zig.
//!
//! YAML tokenizer.

use super::Type;
use crate::err;
use crate::kit::span::Span;
use crate::kit::token::TokenKind;
use crate::{Error, Result};

pub type Token = crate::kit::token::Token<Kind>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    // Structural
    Indent,
    Dedent,
    Newline,
    Dash,
    Colon,
    /// `?` explicit (complex) key indicator
    ExplicitKey,
    /// `,` flow entry separator
    Comma,
    /// `[`
    FlowSeqStart,
    /// `]`
    FlowSeqEnd,
    /// `{`
    FlowMapStart,
    /// `}`
    FlowMapEnd,
    Scalar,
    Comment,
    Whitespace,
    /// `|`/`>` plus chomping/indent indicators
    BlockHeader,
    /// raw body lines of a block scalar
    BlockScalar,
    /// `!!type`, `!suffix`, `!handle!suffix`, or verbatim `!<...>` node property
    Tag,
    /// `&name` node property
    Anchor,
    /// `*name` whole node (a reference to an anchored node)
    Alias,
    /// `---` document start marker
    DocStart,
    /// `...` document end marker
    DocEnd,
    /// `%YAML`/`%TAG`/reserved directive line (whole line, sans newline)
    Directive,
    EndOfFile,
}

impl TokenKind for Kind {
    fn len(self) -> Option<usize> {
        use Kind::*;
        match self {
            EndOfFile | Dedent => Some(0),
            Newline | Dash | Colon | ExplicitKey | Comma | FlowSeqStart | FlowSeqEnd | FlowMapStart | FlowMapEnd => Some(1),
            _ => None,
        }
    }
}

/// Where a tokenize failure fired, recorded by `fail` at the site that
/// returns it: the byte the report points at, and — when the offender has
/// an extent worth underlining — where it ends. The tokenizer reads a whole
/// line before it judges it, so `self.i` has usually moved on by the time an
/// error returns; this is the only record of where the problem was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    pub offset: usize,
    pub end: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct Line {
    start: usize,
    /// if blank line, equals end
    content_start: usize,
    /// excludes newline
    end: usize,
    /// includes newline if present
    newline_end: usize,
}

impl Line {
    fn is_blank(&self) -> bool {
        self.content_start == self.end
    }
}

/// The exclusive end of a physical line's content given the position `nl` of
/// its terminating `\n` (or `source.len` if it has none): trims a trailing
/// `\r` so `\r\n` is treated as one line break rather than a break preceded by
/// a `\r` of content.
fn trim_cr(source: &[u8], start: usize, nl: usize) -> usize {
    if nl > start && source[nl - 1] == b'\r' {
        nl - 1
    } else {
        nl
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingBlock {
    /// Indentation (column) of the line the header sits on. Block content must
    /// be more indented than this, and the explicit-indent indicator is counted
    /// relative to it.
    header_indent: usize,
    explicit_indent: Option<usize>,
    /// True when the block scalar is the document root (`>`/`|` or `--- >`),
    /// whose content may sit at column 0; it ends only at a doc marker or EOF.
    root: bool,
}

pub struct Tokenizer<'a> {
    pub tokens: Vec<Token>,
    pub i: usize,
    /// The location of the error `tokenize` returned, if it returned one (see
    /// `Failure`). `None` after a clean tokenize.
    pub failure: Option<Failure>,
    pending_block: Option<PendingBlock>,
    pub flow_depth: usize,
    /// Indentation of the line on which the outermost flow collection opened, and
    /// whether that collection is the document root. Continuation lines inside flow
    /// must be more indented than this (except the closing bracket, and the root,
    /// which has no indentation floor).
    flow_open_indent: usize,
    flow_root: bool,
    pub source: &'a [u8],
    pub ty: Type,
}

/// A line whose only content is node properties (`&a`/`!tag`) does not bracket
/// structure (see `tokenize`).
#[derive(Debug, Clone, Copy)]
struct Level {
    indent: usize,
    prop_only: bool,
    entry_dash: Option<usize>,
}

struct PlainContinuation {
    content_end: usize,
    last_line: Line,
}

impl<'a> Tokenizer<'a> {
    pub fn new(source: &'a [u8]) -> Self {
        Self::with_type(source, Type::V1_2_2)
    }

    pub fn with_type(source: &'a [u8], ty: Type) -> Self {
        Tokenizer {
            tokens: Vec::new(),
            i: 0,
            failure: None,
            pending_block: None,
            flow_depth: 0,
            flow_open_indent: 0,
            flow_root: false,
            source,
            ty,
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>> {
        self.tokens.reserve(self.source.len() + 1);

        // A line whose only content is node properties (`&a`/`!tag`) does not bracket
        // structure: the node it decorates sits on a following line, possibly at a
        // different column. Such an indent level is marked `prop_only` so a dedent
        // into it lets the node take that shallower column instead of erroring.
        // `entry_dash` is where the last block-sequence entry on this level begins
        // (its first `-`). Each `- ` on that line establishes an indent context for
        // the content after it that it never pushes (the dash line's own indent is
        // the dash column), so a later line returning to one of those columns — a
        // sibling key after the first key's block value (`- k:\n    v: 1\n  s: 2`),
        // or the next item of a compact nested sequence (`- - - a\n    - b\n  - c`)
        // — would otherwise look like an unmatched dedent. Recording it lets the
        // dedent handler admit those columns (`entry_content_column`).
        let mut current_indent: usize = 0;
        let mut indent_stack: Vec<Level> = vec![Level { indent: 0, prop_only: false, entry_dash: None }];

        // A leading UTF-8 BOM is not content: the first line starts after it,
        // so its indentation and its first key are read as without one.
        if self.i == 0 && self.source.starts_with(b"\xEF\xBB\xBF") {
            self.i = 3;
        }

        // YAML is whitespace-sensitive, so we parse line-by-line.
        while let Some(line) = self.get_line() {
            if line.is_blank() {
                continue;
            }

            // A line that is only whitespace (including tabs after the leading
            // spaces) is blank in any context.
            if line_all_whitespace(self.source, line) {
                continue;
            }

            // Inside a flow collection (`[`/`{`), indentation is not significant in
            // the block sense, but a few constraints still apply.
            if self.flow_depth > 0 {
                // A document marker may not appear inside a flow collection; emit it
                // so the parser rejects it where it stands.
                if line.content_start == line.start {
                    if let Some(kind) = self.doc_marker(line) {
                        let cs = line.content_start;
                        self.add_token(Token::new(kind, Span::new(cs, cs + 3)));
                        if line.newline_end > line.end {
                            self.add_token(Token::new(Kind::Newline, Span::new(line.end, line.newline_end)));
                        }
                        continue;
                    }
                }
                // Flow content must be more indented than the line the collection
                // opened on; only the closing bracket may sit at that indentation.
                let indent = line.content_start - line.start;
                let first = self.source[line.content_start];
                if !self.flow_root && first != b']' && first != b'}' && indent <= self.flow_open_indent {
                    return Err(self.fail(err!(InvalidIndent), line.content_start, None));
                }
                self.tokenize_line_content(line)?;
                if self.i == line.newline_end && line.newline_end > line.end {
                    self.add_token(Token::new(Kind::Newline, Span::new(line.end, line.newline_end)));
                }
                continue;
            }

            // A comment-only line does not participate in indentation structure;
            // emit just the comment (the parser treats it as trivia) and move on,
            // so an indented comment produces no spurious indent/dedent.
            if self.source[line.content_start] == b'#' {
                self.add_token(Token::new(Kind::Comment, Span::new(line.content_start, line.end)));
                if line.newline_end > line.end {
                    self.add_token(Token::new(Kind::Newline, Span::new(line.end, line.newline_end)));
                }
                continue;
            }

            let indent = line.content_start - line.start;
            let prop_only = self.line_is_property_only(line);
            if indent > current_indent {
                indent_stack.push(Level { indent, prop_only, entry_dash: None });
                self.add_token(Token::new(Kind::Indent, Span::new(line.start, line.content_start)));
                current_indent = indent;
            } else if indent < current_indent {
                while indent < current_indent {
                    let popped = indent_stack.pop().unwrap();
                    current_indent = indent_stack.last().unwrap().indent;
                    // Dedenting out of a property-only level into a column still
                    // deeper than the level below means the property's node lives at
                    // that shallower column (`folded:\n   !foo\n  >1`): replace the
                    // level rather than requiring an exact match, and emit no dedent.
                    if popped.prop_only && indent > current_indent {
                        indent_stack.push(Level { indent, prop_only, entry_dash: None });
                        current_indent = indent;
                        break;
                    }
                    self.add_token(Token::fixed(Kind::Dedent, line.content_start));
                }
                // The dedent may land between two levels at a block-sequence entry's
                // content column. The `-` established that column but never
                // pushed it; admit it as a fresh level. The parser hands the line to
                // the item container that begins there.
                let last = *indent_stack.last().unwrap();
                if indent != current_indent && indent > current_indent && entry_content_column(self.source, last.entry_dash, last.indent, indent) {
                    indent_stack.push(Level { indent, prop_only, entry_dash: None });
                    current_indent = indent;
                }
                if indent != current_indent {
                    return Err(self.fail(err!(InvalidIndent), line.content_start, None));
                }
            }

            // Record where a block-sequence entry begins on its level, so a later
            // dedent can return to the content column of any `- ` on its line (see
            // the dedent handler).
            if self.source[line.content_start] == b'-' && followed_by_blank(self.source, line.content_start, line.end) {
                let n = indent_stack.len();
                indent_stack[n - 1].entry_dash = Some(line.content_start);
            }

            // Document markers (`---`, `...`) are only recognized at column 0.
            // Any dedents above have already closed open containers.
            if indent == 0 {
                if let Some(kind) = self.doc_marker(line) {
                    let cs = line.content_start;
                    self.add_token(Token::new(kind, Span::new(cs, cs + 3)));
                    // Tokenize any content sharing the marker's line (e.g. `--- foo`).
                    let mut rest = cs + 3;
                    while rest < line.end && is_blank(self.source[rest]) {
                        rest += 1;
                    }
                    if rest < line.end {
                        self.tokenize_line_content(Line { start: rest, content_start: rest, end: line.end, newline_end: line.newline_end })?;
                    }
                    if self.i == line.newline_end {
                        if line.newline_end > line.end {
                            self.add_token(Token::new(Kind::Newline, Span::new(line.end, line.newline_end)));
                        }
                        self.flush_pending_block()?;
                    }
                    continue;
                }
            }

            // A directive line (`%YAML`/`%TAG`/reserved) sits at column 0. `%` is an
            // indicator that cannot begin a plain scalar, and a genuine plain-scalar
            // continuation starting with `%` is consumed by `handle_plain` before it
            // reaches here, so a column-0 `%` line is always a directive. The parser
            // validates it and enforces that it precedes a `---` document.
            if indent == 0 && self.source[line.content_start] == b'%' {
                self.add_token(Token::new(Kind::Directive, Span::new(line.content_start, line.end)));
                if line.newline_end > line.end {
                    self.add_token(Token::new(Kind::Newline, Span::new(line.end, line.newline_end)));
                }
                continue;
            }

            // A tab in the indentation of a block mapping key or sequence entry is
            // using the tab as structural indentation, which YAML forbids. (A tab
            // before plain-scalar content is separation and is allowed.)
            if self.tab_indented_structure(line) {
                return Err(self.fail(err!(TabIndent), line.content_start, None));
            }

            self.tokenize_line_content(line)?;
            if self.i == line.newline_end {
                if line.newline_end > line.end {
                    self.add_token(Token::new(Kind::Newline, Span::new(line.end, line.newline_end)));
                }
                self.flush_pending_block()?;
            }
        }

        while !indent_stack.is_empty() {
            indent_stack.pop();
            if indent_stack.is_empty() {
                break;
            }
            self.add_token(Token::fixed(Kind::Dedent, self.i));
        }

        self.add_token(Token::fixed(Kind::EndOfFile, self.i));
        Ok(std::mem::take(&mut self.tokens))
    }

    fn get_line(&mut self) -> Option<Line> {
        if self.i >= self.source.len() {
            return None;
        }

        let start = self.i;
        while self.i < self.source.len() && self.source[self.i] != b'\n' {
            self.i += 1;
        }

        let raw_end = self.i;
        if self.i < self.source.len() && self.source[self.i] == b'\n' {
            self.i += 1;
        }
        // `\r\n` is a single line break, not a line break plus a trailing `\r` of
        // content.
        let end = trim_cr(self.source, start, raw_end);

        // Indentation is leading spaces only. A tab after them is separation or
        // scalar content, so it is handled downstream, not here.
        let mut content_start = start;
        while content_start < end && self.source[content_start] == b' ' {
            content_start += 1;
        }

        Some(Line { start, content_start, end, newline_end: self.i })
    }

    fn tokenize_line_content(&mut self, line_in: Line) -> Result<()> {
        let mut line = line_in;
        let mut cursor = line.content_start;
        let mut at_content_start = true;

        // The indentation of the node that owns a block scalar value on this line —
        // the column of the mapping key (for `key: |`) or the sequence dash (for
        // `- |`). An explicit indentation indicator (`|2`) counts from here, so for
        // a nested block (`- k: |2`) it must be the key's column, not the line's
        // leading indent. Tracked as we scan: a `:` adopts the preceding key's
        // column, a `-` its own.
        let mut block_owner_indent = line.content_start - line.start;
        let mut node_start = line.content_start;

        while cursor < line.end {
            // Record where the current node begins, so a following `:` can recover
            // its key's column.
            if at_content_start && self.source[cursor] != b' ' && self.source[cursor] != b'\t' {
                node_start = cursor;
            }

            match self.source[cursor] {
                b' ' | b'\t' => {
                    let end = whitespace_end(self.source, cursor, line.end);
                    self.add_token(Token::new(Kind::Whitespace, Span::new(cursor, end)));
                    cursor = end;
                }
                b'#' => {
                    // A `#` starts a comment only at the start of content or when
                    // preceded by whitespace; otherwise (e.g. `]#x`, `,#x`) it is
                    // ordinary scalar text.
                    if cursor == line.content_start || is_blank(self.source[cursor - 1]) {
                        self.add_token(Token::new(Kind::Comment, Span::new(cursor, line.end)));
                        return Ok(());
                    }
                    self.handle_plain(cursor, &mut line, &mut cursor, &mut at_content_start)?;
                }
                b':' => {
                    // A colon is a mapping value indicator only when followed by
                    // whitespace or the end of line. In flow, a `:` may also abut a
                    // JSON-style key (a quoted scalar or a flow collection) with no
                    // separating space (`"key":value`).
                    if colon_is_indicator(self.source, cursor, line.end, self.flow_depth > 0) || self.json_key_colon() {
                        // The value introduced by this `:` is owned by the key node,
                        // so a block scalar value indents relative to the key column.
                        // (Skip if the key began on an earlier line.)
                        if self.flow_depth == 0 && node_start >= line.start {
                            block_owner_indent = node_start - line.start;
                        }
                        self.add_token(Token::fixed(Kind::Colon, cursor));
                        cursor += 1;
                        // After a value indicator we are at the start of the value
                        // node, so a following `-`/`?` is an indicator.
                        at_content_start = true;
                    } else {
                        self.handle_plain(cursor, &mut line, &mut cursor, &mut at_content_start)?;
                    }
                }
                b'[' => {
                    if self.flow_depth == 0 {
                        self.open_flow(line);
                    }
                    self.add_token(Token::fixed(Kind::FlowSeqStart, cursor));
                    self.flow_depth += 1;
                    cursor += 1;
                    at_content_start = false;
                }
                b'{' => {
                    if self.flow_depth == 0 {
                        self.open_flow(line);
                    }
                    self.add_token(Token::fixed(Kind::FlowMapStart, cursor));
                    self.flow_depth += 1;
                    cursor += 1;
                    at_content_start = false;
                }
                b']' => {
                    self.add_token(Token::fixed(Kind::FlowSeqEnd, cursor));
                    if self.flow_depth > 0 {
                        self.flow_depth -= 1;
                    }
                    cursor += 1;
                    at_content_start = false;
                }
                b'}' => {
                    self.add_token(Token::fixed(Kind::FlowMapEnd, cursor));
                    if self.flow_depth > 0 {
                        self.flow_depth -= 1;
                    }
                    cursor += 1;
                    at_content_start = false;
                }
                b',' => {
                    if self.flow_depth > 0 {
                        self.add_token(Token::fixed(Kind::Comma, cursor));
                        cursor += 1;
                        at_content_start = false;
                    } else {
                        self.handle_plain(cursor, &mut line, &mut cursor, &mut at_content_start)?;
                    }
                }
                b'\'' | b'"' => {
                    // Continuation lines of a multi-line quoted scalar must out-indent
                    // the line it sits on — except a root scalar, or one written on
                    // its own line.
                    let floor: Option<usize> = if self.preceded_only_by_trivia() || cursor == line.content_start {
                        None
                    } else {
                        Some(line.content_start - line.start)
                    };
                    let end = self.multiline_quoted_end(cursor, floor)?;
                    if end > line.end {
                        // The quoted scalar ran onto following lines. Emit it whole,
                        // then continue tokenizing the rest of its closing line.
                        self.add_token(Token::new(Kind::Scalar, Span::new(cursor, end)));
                        line = self.remainder_line(end);
                        self.i = line.newline_end;
                        cursor = end;
                        at_content_start = false;
                        continue;
                    }
                    self.add_scalar(cursor, end);
                    cursor = end;
                    at_content_start = false;
                }
                b'|' | b'>' => {
                    // A `|`/`>` in value position begins a block scalar. Block
                    // scalars do not occur inside flow, so there it is plain text.
                    if self.flow_depth == 0 {
                        if let Some(hdr_end) = block_header_end(self.source, cursor, line.end) {
                            // A header preceded only by a doc marker / trivia is the
                            // document root (`>` or `--- >`); its body may sit at col 0.
                            let root = self.preceded_only_by_trivia();
                            // A header on its own line is a deferred value; an explicit
                            // indentation indicator counts from the OWNING key/dash
                            // column (recovered from the token stream).
                            let owner = if cursor == line.content_start {
                                self.deferred_owner_indent().unwrap_or(block_owner_indent)
                            } else {
                                block_owner_indent
                            };
                            self.add_token(Token::new(Kind::BlockHeader, Span::new(cursor, hdr_end)));
                            let mut rest = hdr_end;
                            while rest < line.end && is_blank(self.source[rest]) {
                                rest += 1;
                            }
                            if rest < line.end && self.source[rest] == b'#' {
                                self.add_token(Token::new(Kind::Comment, Span::new(rest, line.end)));
                            }
                            self.pending_block = Some(PendingBlock {
                                header_indent: owner,
                                explicit_indent: explicit_indent(self.source, cursor, hdr_end),
                                root,
                            });
                            return Ok(());
                        }
                        // At value start (outside flow) a `|`/`>` can only be a block
                        // header; one with a malformed indicator is invalid.
                        let e = non_blank_end(self.source, cursor, line.end);
                        return Err(self.fail(err!(InvalidBlockHeader), cursor, Some(e)));
                    }
                    self.handle_plain(cursor, &mut line, &mut cursor, &mut at_content_start)?;
                }
                b'-' => {
                    // A dash is a sequence entry indicator only at the start of a
                    // node and when followed by whitespace or end of line. It is
                    // never an entry indicator inside flow.
                    if self.flow_depth == 0 && at_content_start && followed_by_blank(self.source, cursor, line.end) {
                        // A block sequence `-` separated from a preceding `?`/`:` by a
                        // tab uses the tab as the new collection's indentation, which
                        // is forbidden (`?\t-`, `:\t-`).
                        if cursor > line.content_start && self.source[cursor - 1] == b'\t' {
                            return Err(self.fail(err!(TabIndent), cursor - 1, None));
                        }
                        // A block scalar that is this sequence entry's value (`- |2`)
                        // indents relative to the dash's column.
                        block_owner_indent = cursor - line.start;
                        self.add_token(Token::fixed(Kind::Dash, cursor));
                        cursor += 1;
                        // A dash starts a node, so a nested `-`/`?` right after it is
                        // also an indicator (`- - c` is a nested sequence).
                        at_content_start = true;
                    } else {
                        self.handle_plain(cursor, &mut line, &mut cursor, &mut at_content_start)?;
                    }
                }
                b'?' => {
                    // A `?` is an explicit-key indicator at the start of a node (or
                    // anywhere in flow) when followed by whitespace or end of line.
                    // `at_content_start` stays true.
                    if (self.flow_depth > 0 || at_content_start) && followed_by_blank(self.source, cursor, line.end) {
                        self.add_token(Token::fixed(Kind::ExplicitKey, cursor));
                        cursor += 1;
                    } else {
                        self.handle_plain(cursor, &mut line, &mut cursor, &mut at_content_start)?;
                    }
                }
                b'!' => {
                    // A `!` at node start (or anywhere in flow) introduces a tag
                    // property on the node that follows. `at_content_start` stays
                    // true: the tagged node is still to come.
                    if self.flow_depth > 0 || at_content_start {
                        if let Some(end) = tag_end(self.source, cursor, line.end, self.flow_depth > 0) {
                            // The tag must be separated from its node.
                            if !tag_separated(self.source, end, line.end, self.flow_depth > 0) {
                                let e = non_blank_end(self.source, cursor, line.end);
                                return Err(self.fail(err!(InvalidTag), cursor, Some(e)));
                            }
                            self.add_token(Token::new(Kind::Tag, Span::new(cursor, end)));
                            cursor = end;
                        } else {
                            let e = non_blank_end(self.source, cursor, line.end);
                            return Err(self.fail(err!(InvalidTag), cursor, Some(e)));
                        }
                    } else {
                        self.handle_plain(cursor, &mut line, &mut cursor, &mut at_content_start)?;
                    }
                }
                b'&' => {
                    // `&name` anchors the node that follows — a property, like a tag,
                    // so `at_content_start` stays true.
                    if self.flow_depth > 0 || at_content_start {
                        let end = anchor_name_end(self.source, cursor + 1, line.end);
                        if end == cursor + 1 || !tag_separated(self.source, end, line.end, self.flow_depth > 0) {
                            let e = non_blank_end(self.source, cursor, line.end);
                            return Err(self.fail(err!(InvalidAnchor), cursor, Some(e)));
                        }
                        self.add_token(Token::new(Kind::Anchor, Span::new(cursor, end)));
                        cursor = end;
                    } else {
                        self.handle_plain(cursor, &mut line, &mut cursor, &mut at_content_start)?;
                    }
                }
                b'*' => {
                    // `*name` is an alias: a whole node referencing an earlier anchor.
                    // Unlike a property it IS the node, so `at_content_start` flips to
                    // false afterward.
                    if self.flow_depth > 0 || at_content_start {
                        let end = anchor_name_end(self.source, cursor + 1, line.end);
                        if end == cursor + 1 || !tag_separated(self.source, end, line.end, self.flow_depth > 0) {
                            let e = non_blank_end(self.source, cursor, line.end);
                            return Err(self.fail(err!(InvalidAlias), cursor, Some(e)));
                        }
                        self.add_token(Token::new(Kind::Alias, Span::new(cursor, end)));
                        cursor = end;
                        at_content_start = false;
                    } else {
                        self.handle_plain(cursor, &mut line, &mut cursor, &mut at_content_start)?;
                    }
                }
                _ => self.handle_plain(cursor, &mut line, &mut cursor, &mut at_content_start)?,
            }
        }

        // When a multi-line scalar advanced us onto a later line, that line's break
        // belongs to us; emit it here. Callers detect this via `self.i` having moved
        // past the original line and skip their own newline handling.
        if line.start != line_in.start && line.newline_end > line.end {
            self.add_token(Token::new(Kind::Newline, Span::new(line.end, line.newline_end)));
        }
        Ok(())
    }

    /// True when `line` puts a tab in its indentation region (right after the
    /// leading spaces) and then begins a block mapping key or sequence entry — i.e.
    /// the tab is being used as structural indentation, which is invalid.
    fn tab_indented_structure(&self, line: Line) -> bool {
        let cs = line.content_start;
        if cs >= line.end || self.source[cs] != b'\t' {
            return false;
        }

        let mut ws = cs;
        while ws < line.end && is_blank(self.source[ws]) {
            ws += 1;
        }
        if ws >= line.end {
            return false;
        }

        let fc = self.source[ws];
        if fc == b'-' && (ws + 1 >= line.end || is_blank(self.source[ws + 1])) {
            return true;
        }
        let se = scalar_end(self.source, ws, line.end, false);
        se < line.end && self.source[se] == b':'
    }

    /// Builds a Line for the remainder of the source starting at `pos` (used to
    /// resume tokenizing the closing line of a multi-line scalar).
    fn remainder_line(&self, pos: usize) -> Line {
        let mut nl = pos;
        while nl < self.source.len() && self.source[nl] != b'\n' {
            nl += 1;
        }
        let newline_end = if nl < self.source.len() { nl + 1 } else { nl };
        let end = trim_cr(self.source, pos, nl);
        Line { start: pos, content_start: pos, end, newline_end }
    }

    /// Emits a plain scalar starting at `start`. If it reaches end of line in block
    /// context, it may continue onto more-indented following lines (a multi-line
    /// plain scalar), which are gathered into one token; the parser folds the breaks.
    fn handle_plain(&mut self, start: usize, line: &mut Line, cursor: &mut usize, at_content_start: &mut bool) -> Result<()> {
        let flow = self.flow_depth > 0;
        if flow {
            // A plain scalar inside flow may fold across line breaks; gather it whole.
            let fend = self.flow_plain_end(start);
            if fend > start {
                self.add_token(Token::new(Kind::Scalar, Span::new(start, fend)));
            }
            if fend > line.end {
                // The scalar ran onto following lines; resume on the line it ended on.
                *line = self.remainder_line(fend);
                self.i = line.newline_end;
            }
            *cursor = fend;
            *at_content_start = false;
            return Ok(());
        }
        let end = scalar_end(self.source, start, line.end, flow);
        // Only a genuine plain scalar continues; a "scalar" that begins with a block
        // or flow indicator is not plain text, so gathering it would mask the error.
        if !flow && end == line.end && plain_continuation_start(self.source[start]) {
            let floor = self.plain_floor(*line);
            if let Some(g) = self.gather_plain_continuation(*line, floor) {
                self.add_token(Token::new(Kind::Scalar, Span::new(start, g.content_end)));
                *line = g.last_line;
                *cursor = g.last_line.end;
                *at_content_start = false;
                return Ok(());
            }
        }
        self.add_scalar(start, end);
        *cursor = end;
        *at_content_start = false;
        Ok(())
    }

    /// Scans the lines following a plain scalar to see whether it continues onto
    /// more-indented plain-text lines. Returns the span end (trimmed end of the last
    /// continuation line) and that line, or `None` if there is no continuation. Sets
    /// `self.i` to resume after the last continuation line.
    fn gather_plain_continuation(&mut self, line: Line, floor: Option<usize>) -> Option<PlainContinuation> {
        let mut probe = line.newline_end;
        let mut result: Option<PlainContinuation> = None;
        let src = self.source;

        while probe < src.len() {
            let line_start = probe;
            let mut nl = line_start;
            while nl < src.len() && src[nl] != b'\n' {
                nl += 1;
            }
            let newline_end = if nl < src.len() { nl + 1 } else { nl };
            let end = trim_cr(src, line_start, nl);

            let mut spaces = line_start;
            while spaces < end && src[spaces] == b' ' {
                spaces += 1;
            }
            let mut ws = line_start;
            while ws < end && is_blank(src[ws]) {
                ws += 1;
            }

            // Blank lines are interior fold breaks; include them only if a real
            // continuation line follows (handled by not advancing `result`).
            if ws == end {
                probe = newline_end;
                continue;
            }

            // Indentation is the leading spaces; a following tab is separation that
            // the parser's fold strips. Under-indentation ends the continuation.
            if let Some(f) = floor {
                if spaces - line_start <= f {
                    break;
                }
            }
            let fc = src[ws];
            if !plain_continuation_line_start(fc) {
                break;
            }
            // Past the floor a line is the scalar's whatever it begins with: a `- `
            // or `? ` is text. A `: ` is not plain text anywhere, so the scalar ends
            // before it. A tab before `- `/`? ` is left to `tab_indented_structure`.
            if (fc == b'-' || fc == b'?' || fc == b':') && (ws + 1 >= end || is_blank(src[ws + 1])) && (fc == b':' || ws != spaces) {
                break;
            }
            let se = scalar_end(src, ws, end, false);
            if se != end {
                // Stopped early: a `:` value indicator ends the scalar, and a key
                // cannot span lines, so the line is left for the parser to refuse.
                // A trailing `#` comment still leaves a valid final continuation
                // line, so gather its content up to the comment, then stop.
                if src[se] == b'#' {
                    result = Some(PlainContinuation {
                        content_end: trim_right_blanks(src, ws, se),
                        last_line: Line { start: line_start, content_start: spaces, end, newline_end },
                    });
                }
                break;
            }

            result = Some(PlainContinuation {
                content_end: trim_right_blanks(src, ws, end),
                last_line: Line { start: line_start, content_start: spaces, end, newline_end },
            });
            probe = newline_end;
        }

        if let Some(r) = &result {
            self.i = r.last_line.newline_end;
        }
        result
    }

    /// Scans a plain scalar inside a flow collection, which (unlike a block plain
    /// scalar) may fold across line breaks freely. Returns the trimmed end of the
    /// scalar's content — which may lie on a later line. The scan stops at a flow
    /// indicator (`,`/`[`/`]`/`{`/`}` or a `:` value indicator), a comment, a
    /// document marker at column 0, or EOF. The parser folds the captured breaks.
    fn flow_plain_end(&self, start: usize) -> usize {
        let source = self.source;
        let mut i = start;
        let mut last_content = start; // exclusive end of the last non-blank byte seen
        while i < source.len() {
            match source[i] {
                b'\n' => {
                    if self.doc_marker_at(i + 1) {
                        break;
                    }
                    i += 1;
                    while i < source.len() && (source[i] == b' ' || source[i] == b'\t') {
                        i += 1;
                    }
                    // A comment line breaks a flow plain scalar.
                    if i < source.len() && source[i] == b'#' {
                        break;
                    }
                }
                b',' | b'[' | b']' | b'{' | b'}' => break,
                b'\r' => {
                    // A lone `\r` or the `\r` of a `\r\n` break is never scalar
                    // content.
                    i += 1;
                }
                b':' => {
                    // A `:` is a value indicator (and so ends the scalar) when
                    // followed by whitespace/EOL or, in flow, a `,`/`]`/`}`.
                    let next: u8 = if i + 1 < source.len() { source[i + 1] } else { 0 };
                    if next == 0 || next == b' ' || next == b'\t' || next == b'\n' || next == b'\r' || next == b',' || next == b']' || next == b'}' {
                        break;
                    }
                    i += 1;
                    last_content = i;
                }
                b'#' => {
                    if i > start && is_blank(source[i - 1]) {
                        break;
                    }
                    i += 1;
                    last_content = i;
                }
                c => {
                    i += 1;
                    if !is_blank(c) {
                        last_content = i;
                    }
                }
            }
        }
        last_content
    }

    fn add_token(&mut self, token: Token) {
        self.tokens.push(token);
    }

    fn add_scalar(&mut self, start: usize, end: usize) {
        let trimmed_end = trim_right_blanks(self.source, start, end);
        if trimmed_end > start {
            self.add_token(Token::new(Kind::Scalar, Span::new(start, trimmed_end)));
        }
    }

    /// In flow context, a `:` directly following a JSON-style key — a quoted scalar
    /// or a closed flow collection — is a value indicator even without a separating
    /// space (`{"a":1}`). Checks the previous emitted token so a literal quote inside
    /// a plain scalar (`ab":c`) is not mistaken for a quoted key.
    fn json_key_colon(&self) -> bool {
        if self.flow_depth == 0 {
            return false;
        }
        // Skip trivia back to the last real token.
        let mut idx = self.tokens.len();
        while idx > 0 {
            match self.tokens[idx - 1].kind {
                Kind::Whitespace | Kind::Newline | Kind::Comment => {}
                _ => break,
            }
            idx -= 1;
        }
        if idx == 0 {
            return false;
        }
        let prev = self.tokens[idx - 1];
        match prev.kind {
            Kind::FlowSeqEnd | Kind::FlowMapEnd => true,
            Kind::Scalar => {
                let s = prev.source(self.source);
                !s.is_empty() && (s[0] == b'"' || s[0] == b'\'')
            }
            _ => false,
        }
    }

    /// Detects a document marker (`---` or `...`) occupying its own column-0 line.
    /// The three marker bytes must be followed by whitespace or end of line.
    fn doc_marker(&self, line: Line) -> Option<Kind> {
        let cs = line.content_start;
        if line.end - cs < 3 {
            return None;
        }
        let c = self.source[cs];
        if c != b'-' && c != b'.' {
            return None;
        }
        if self.source[cs + 1] != c || self.source[cs + 2] != c {
            return None;
        }
        if cs + 3 != line.end && !is_blank(self.source[cs + 3]) {
            return None;
        }
        Some(if c == b'-' { Kind::DocStart } else { Kind::DocEnd })
    }

    fn open_flow(&mut self, line: Line) {
        self.flow_open_indent = line.content_start - line.start;
        // A flow collection that is the whole document — nothing but a `---` marker
        // and trivia precedes it — is the root node, which has no indentation floor
        // for its content.
        self.flow_root = self.preceded_only_by_trivia();
    }

    fn flush_pending_block(&mut self) -> Result<()> {
        let Some(info) = self.pending_block.take() else { return Ok(()) };
        self.consume_block_body(info)
    }

    /// Consumes the indented body lines of a block scalar (already past the header
    /// line's newline) and emits a single `block_scalar` token spanning them. The
    /// body runs until the first non-blank line indented no more than the content
    /// indentation; blank lines are always included so chomping can see them.
    fn consume_block_body(&mut self, info: PendingBlock) -> Result<()> {
        let body_start = self.i;
        let mut content_indent: Option<usize> = info.explicit_indent.map(|d| info.header_indent + d);
        // While auto-detecting, the largest indentation seen on a leading empty
        // line: it is an error for one to be more indented than the first content
        // line (YAML 1.2.2 §8.1.1.1).
        let mut max_leading_blank: usize = 0;
        // Smallest column at which a tab appears in a leading empty line. A tab is
        // invalid only inside the indentation zone, i.e. before the content indent.
        let mut min_blank_tab: Option<usize> = None;
        // Where that tab is, for the report.
        let mut blank_tab_at: Option<usize> = None;
        let mut last_end = body_start;
        let src = self.source;

        while self.i < src.len() {
            let line_start = self.i;
            let mut nl = line_start;
            while nl < src.len() && src[nl] != b'\n' {
                nl += 1;
            }
            let newline_end = if nl < src.len() { nl + 1 } else { nl };
            // Only affects the indent/blank classification below, not the token
            // span (`last_end`), which still runs through `newline_end`.
            let end = trim_cr(src, line_start, nl);

            let mut spaces = line_start;
            while spaces < end && src[spaces] == b' ' {
                spaces += 1;
            }
            let mut ws = line_start;
            while ws < end && is_blank(src[ws]) {
                ws += 1;
            }
            let blank = ws == end;
            let indent = spaces - line_start;

            if blank {
                if content_indent.is_none() {
                    // A tab is never block indentation, so an otherwise-blank line
                    // whose tab sits past the indentation zone is actually the first
                    // content line — the tab is content — and it fixes the
                    // auto-detected content indent. A tab inside the indentation
                    // zone (`foo: |\n\t...`) stays an error.
                    if ws > spaces && (info.root || indent > info.header_indent) {
                        if max_leading_blank > indent {
                            return Err(self.fail(err!(InvalidIndent), spaces, None));
                        }
                        content_indent = Some(indent);
                    } else {
                        if ws > spaces {
                            let tab_col = spaces - line_start;
                            if min_blank_tab.is_none() || tab_col < min_blank_tab.unwrap() {
                                min_blank_tab = Some(tab_col);
                                blank_tab_at = Some(spaces);
                            }
                        }
                        if indent > max_leading_blank {
                            max_leading_blank = indent;
                        }
                    }
                }
            } else {
                // A document marker at column 0 ends a root block scalar.
                if info.root && indent == 0 && self.doc_marker_at(line_start) {
                    break;
                }
                if let Some(ci) = content_indent {
                    if indent < ci {
                        break;
                    }
                } else {
                    // A root block scalar's content may sit at column 0.
                    if !info.root && indent <= info.header_indent {
                        break;
                    }
                    if max_leading_blank > indent {
                        return Err(self.fail(err!(InvalidIndent), spaces, None));
                    }
                    if let Some(col) = min_blank_tab {
                        if col < indent {
                            return Err(self.fail(err!(TabIndent), blank_tab_at.unwrap(), None));
                        }
                    }
                    content_indent = Some(indent);
                }
            }

            self.i = newline_end;
            last_end = newline_end;
        }

        // An empty block scalar whose only lines carried tabs has tabs in the
        // indentation zone (there is no content to measure against).
        if content_indent.is_none() && min_blank_tab.is_some() {
            return Err(self.fail(err!(TabIndent), blank_tab_at.unwrap(), None));
        }

        if last_end > body_start {
            self.add_token(Token::new(Kind::BlockScalar, Span::new(body_start, last_end)));
        }
        Ok(())
    }

    /// Quoted scalars may span multiple lines; this scans to the closing quote
    /// across line breaks (the quote delimits them unambiguously) and validates each
    /// continuation line: it may not be a document marker, carry a tab in its
    /// indentation, or — when `floor` is set — be indented no more than `floor`.
    /// The parser folds the captured line breaks when it decodes the string.
    fn multiline_quoted_end(&mut self, start: usize, floor: Option<usize>) -> Result<usize> {
        let source = self.source;
        let double = source[start] == b'"';
        let mut i = start + 1;
        while i < source.len() {
            let c = source[i];
            if c == b'\n' {
                let line_start = i + 1;
                if self.doc_marker_at(line_start) {
                    return Err(self.fail(err!(UnclosedString), start, None));
                }
                let mut spaces = line_start;
                while spaces < source.len() && source[spaces] == b' ' {
                    spaces += 1;
                }
                let mut ws = line_start;
                while ws < source.len() && is_blank(source[ws]) {
                    ws += 1;
                }
                // The `\r` of a `\r\n` break makes an otherwise-blank line look
                // like it has content; treat it as blank too.
                let blank = ws >= source.len() || source[ws] == b'\n' || source[ws] == b'\r';
                if !blank {
                    // Indentation is measured in spaces; a tab after them is
                    // separation. Under-indentation (too few spaces) is the error.
                    if let Some(f) = floor {
                        if spaces - line_start <= f {
                            return Err(self.fail(err!(InvalidIndent), spaces, None));
                        }
                    }
                }
                i += 1;
                continue;
            }
            if double {
                if c == b'\\' {
                    // A backslash escapes the next char; if that is a newline (an
                    // escaped line break) let the newline branch validate the line.
                    if i + 1 < source.len() && source[i + 1] == b'\n' {
                        i += 1;
                    } else {
                        i += 2;
                    }
                    continue;
                }
                if c == b'"' {
                    return Ok(i + 1);
                }
            } else if c == b'\'' {
                if i + 1 < source.len() && source[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                return Ok(i + 1);
            }
            i += 1;
        }
        Err(self.fail(err!(UnclosedString), start, None))
    }

    /// Record where `err` fired (see `Failure`) and hand it back to return.
    fn fail(&mut self, e: Error, offset: usize, end: Option<usize>) -> Error {
        self.failure = Some(Failure { offset, end });
        e
    }

    /// True when a document marker (`---`/`...`) begins at column-0 position `pos`.
    fn doc_marker_at(&self, pos: usize) -> bool {
        if pos + 3 > self.source.len() {
            return false;
        }
        let c = self.source[pos];
        if c != b'-' && c != b'.' {
            return false;
        }
        if self.source[pos + 1] != c || self.source[pos + 2] != c {
            return false;
        }
        let after = pos + 3;
        // `\r` is accepted here too so a marker followed by a `\r\n` break (not
        // just a bare `\n`) is still recognized.
        after >= self.source.len() || self.source[after] == b'\n' || self.source[after] == b'\r' || is_blank(self.source[after])
    }

    /// True when a line's only content is node properties (`&anchor`/`!tag`),
    /// optionally followed by a comment — e.g. a tag sitting on its own line above
    /// the block scalar it decorates. Such a line does not establish structure.
    fn line_is_property_only(&self, line: Line) -> bool {
        if self.flow_depth > 0 {
            return false;
        }
        let mut i = line.content_start;
        let mut saw_property = false;
        while i < line.end {
            match self.source[i] {
                b' ' | b'\t' => i += 1,
                b'#' => return saw_property, // a trailing comment after properties
                b'&' => {
                    let end = anchor_name_end(self.source, i + 1, line.end);
                    if end == i + 1 {
                        return false; // empty name → not a property
                    }
                    i = end;
                    saw_property = true;
                }
                b'!' => {
                    let Some(end) = tag_end(self.source, i, line.end, false) else { return false };
                    i = end;
                    saw_property = true;
                }
                _ => return false, // any other content
            }
        }
        saw_property
    }

    /// Column (0-based) of `pos` on its source line.
    fn column_of(&self, pos: usize) -> usize {
        crate::kit::lines::column_of(self.source, pos)
    }

    /// For a block header that opens a key/dash's deferred value (it sits alone on a
    /// line below the `key:`/`-`), returns the owning node's column — the key before
    /// the introducing `:`, or the `-` itself — by scanning back past intervening
    /// trivia, indents, and node properties. `None` if no such introducer is found.
    fn deferred_owner_indent(&self) -> Option<usize> {
        let mut idx = self.tokens.len();
        while idx > 0 {
            match self.tokens[idx - 1].kind {
                Kind::Whitespace | Kind::Newline | Kind::Comment | Kind::Indent | Kind::Dedent | Kind::Tag | Kind::Anchor => {}
                Kind::Colon => {
                    // The owner is the key written before this colon.
                    let mut k = idx - 1;
                    while k > 0 {
                        match self.tokens[k - 1].kind {
                            Kind::Whitespace | Kind::Comment => {}
                            _ => break,
                        }
                        k -= 1;
                    }
                    if k == 0 {
                        return None;
                    }
                    return Some(self.column_of(self.tokens[k - 1].span.start));
                }
                Kind::Dash => return Some(self.column_of(self.tokens[idx - 1].span.start)),
                _ => return None,
            }
            idx -= 1;
        }
        None
    }

    /// The column a block plain scalar about to be emitted must out-indent on its
    /// continuation lines: the column of the innermost block collection it is a
    /// node of — the `-` of its sequence entry, the `?` or `:` of an explicit
    /// entry, or the start of the key before an implicit `:` (`- k: a` continues
    /// past column 2, not column 0). Found by scanning back past trivia and node
    /// properties. `None` for the document's root node, which continues onto
    /// every line.
    fn plain_floor(&self, line: Line) -> Option<usize> {
        let tokens = &self.tokens;
        let mut idx = tokens.len();
        while idx > 0 {
            match tokens[idx - 1].kind {
                Kind::Whitespace | Kind::Newline | Kind::Comment | Kind::Indent | Kind::Dedent | Kind::Tag | Kind::Anchor => {}
                Kind::DocStart | Kind::Directive => return None,
                Kind::Dash | Kind::ExplicitKey => return Some(self.column_of(tokens[idx - 1].span.start)),
                Kind::Colon => {
                    // The key begins at the first token after the indicator or line
                    // start before it; a `:` that begins its line has no key.
                    let colon = tokens[idx - 1].span.start;
                    let line_start = colon - self.column_of(colon);
                    let mut key = colon;
                    let mut k = idx - 1;
                    while k > 0 && tokens[k - 1].span.start >= line_start {
                        match tokens[k - 1].kind {
                            Kind::Dash | Kind::ExplicitKey | Kind::Colon | Kind::Newline | Kind::Indent | Kind::Dedent | Kind::DocStart => break,
                            Kind::Whitespace => {}
                            _ => key = tokens[k - 1].span.start,
                        }
                        k -= 1;
                    }
                    return Some(self.column_of(key));
                }
                // A scalar after a node that is not its introducer is refused by the
                // parser; the line's own indent stands in until then.
                _ => return Some(line.content_start - line.start),
            }
            idx -= 1;
        }
        None
    }

    /// True when only a document-start marker and trivia have been emitted so far —
    /// i.e. the next node is the document root, which has no indentation floor.
    fn preceded_only_by_trivia(&self) -> bool {
        for t in &self.tokens {
            match t.kind {
                // A `.tag`/`.anchor` is a node property, not structural content; a
                // `.directive` is document prefix, likewise not content.
                Kind::DocStart | Kind::Newline | Kind::Whitespace | Kind::Comment | Kind::Tag | Kind::Anchor | Kind::Directive => {}
                _ => return false,
            }
        }
        true
    }
}

/// True when `c` can begin a plain-scalar continuation line (i.e. it is not an
/// indicator that would start a different kind of node).
fn plain_continuation_start(c: u8) -> bool {
    !matches!(c, b'#' | b'[' | b']' | b'{' | b'}' | b',' | b'\'' | b'"' | b'&' | b'*' | b'!' | b'|' | b'>' | b'@' | b'`' | b'%')
}

/// Whether a continuation line of an already-open block plain scalar may begin
/// with `c`. Only a line-leading `#` (a comment) breaks the scalar.
fn plain_continuation_line_start(c: u8) -> bool {
    c != b'#'
}

/// If a tag property begins at `start` (`source[start] == '!'`), returns the
/// index just past it. Handles the verbatim form `!<...>` (scanned to its
/// closing `>`, since it may contain `,`/`:`), the shorthand forms (`!`, `!!type`,
/// `!suffix`, `!handle!suffix`) which run to a whitespace/EOL terminator (and, in
/// flow, to a flow indicator). A bare `!` (the non-specific tag) is valid.
/// Returns `None` only for a malformed verbatim tag (unclosed or empty `!<>`).
fn tag_end(source: &[u8], start: usize, line_end: usize, _flow: bool) -> Option<usize> {
    let mut end = start + 1;
    if end < line_end && source[end] == b'<' {
        end += 1;
        let verbatim_start = end;
        while end < line_end && source[end] != b'>' {
            end += 1;
        }
        if end >= line_end || end == verbatim_start {
            return None;
        }
        return Some(end + 1); // include the closing `>`
    }
    while end < line_end {
        match source[end] {
            // Whitespace ends a tag in any context; flow indicators are never
            // valid tag characters, so they end the shorthand too.
            b' ' | b'\t' | b',' | b'[' | b']' | b'{' | b'}' => break,
            _ => {}
        }
        end += 1;
    }
    Some(end)
}

/// Scans an anchor/alias name (`ns-anchor-char+`) starting at `start` (just past
/// the `&`/`*`). The name runs to whitespace, end-of-line, or a flow indicator.
/// Returns `start` for an empty name (the caller rejects it).
fn anchor_name_end(source: &[u8], start: usize, line_end: usize) -> usize {
    let mut end = start;
    while end < line_end {
        match source[end] {
            b' ' | b'\t' | b',' | b'[' | b']' | b'{' | b'}' => break,
            _ => {}
        }
        end += 1;
    }
    end
}

/// A tag property must be separated from the node it decorates: by whitespace or
/// end-of-line in any context, or — inside flow — by a flow indicator.
fn tag_separated(source: &[u8], end: usize, line_end: usize, flow: bool) -> bool {
    if end >= line_end {
        return true;
    }
    match source[end] {
        b' ' | b'\t' => true,
        b',' | b'[' | b']' | b'{' | b'}' => flow,
        _ => false,
    }
}

fn scalar_end(source: &[u8], start: usize, line_end: usize, flow: bool) -> usize {
    let mut end = start;
    while end < line_end {
        match source[end] {
            // A colon ends the scalar only when it is a value indicator.
            b':' => {
                if colon_is_indicator(source, end, line_end, flow) {
                    break;
                }
            }
            // A `#` begins a comment only when preceded by whitespace.
            b'#' => {
                if end > start && is_blank(source[end - 1]) {
                    break;
                }
            }
            // Flow indicators terminate a plain scalar inside a flow collection.
            b',' | b'[' | b']' | b'{' | b'}' => {
                if flow {
                    break;
                }
            }
            _ => {}
        }
        end += 1;
    }
    end
}

/// A `:` is a mapping value indicator when followed by whitespace/EOL, or — in
/// flow context — when followed by a flow indicator (`,`/`]`/`}`).
fn colon_is_indicator(source: &[u8], i: usize, line_end: usize, flow: bool) -> bool {
    if followed_by_blank(source, i, line_end) {
        return true;
    }
    if !flow {
        return false;
    }
    matches!(source[i + 1], b',' | b']' | b'}')
}

fn is_blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

fn line_all_whitespace(source: &[u8], line: Line) -> bool {
    source[line.content_start..line.end].iter().all(|&c| is_blank(c))
}

/// True when the byte after `i` is whitespace or `i` is the last byte on the line.
fn followed_by_blank(source: &[u8], i: usize, line_end: usize) -> bool {
    i + 1 >= line_end || is_blank(source[i + 1])
}

/// True when `col` is the content column of one of the `- ` that begin the
/// line whose first dash is at `entry_dash`, in column `dash_col`: the column
/// just past the first dash, and for a compact nested sequence (`- - - a`)
/// past each later one. Content after the last dash ends the walk.
fn entry_content_column(source: &[u8], entry_dash: Option<usize>, dash_col: usize, col: usize) -> bool {
    let Some(mut c) = entry_dash else { return false };
    let line_start = c - dash_col;
    let line_end = source[c..].iter().position(|&b| b == b'\n').map_or(source.len(), |p| c + p);
    while c < line_end && source[c] == b'-' && followed_by_blank(source, c, line_end) {
        c += 1;
        while c < line_end && source[c] == b' ' {
            c += 1;
        }
        if c == line_end {
            return false;
        }
        if c - line_start == col {
            return true;
        }
    }
    false
}

/// If a block-scalar header (`|`/`>` plus optional chomping/indent indicators)
/// occupies the rest of the line, returns the index just past the indicators.
/// The indicators may be followed only by whitespace and/or a comment.
fn block_header_end(source: &[u8], start: usize, line_end: usize) -> Option<usize> {
    let mut i = start + 1;
    let mut seen_chomp = false;
    let mut seen_indent = false;
    while i < line_end {
        match source[i] {
            b'+' | b'-' => {
                if seen_chomp {
                    return None;
                }
                seen_chomp = true;
            }
            b'1'..=b'9' => {
                if seen_indent {
                    return None;
                }
                seen_indent = true;
            }
            _ => break,
        }
        i += 1;
    }
    let indicators_end = i;
    while i < line_end && is_blank(source[i]) {
        i += 1;
    }
    if i < line_end {
        // The only thing allowed after the indicators is a comment, and a `#`
        // is a comment only when separated from the indicators by whitespace.
        if source[i] != b'#' || i == indicators_end {
            return None;
        }
    }
    Some(indicators_end)
}

/// Extracts the explicit indentation indicator digit from a header, if present.
fn explicit_indent(source: &[u8], start: usize, header_end: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < header_end {
        if (b'1'..=b'9').contains(&source[i]) {
            return Some((source[i] - b'0') as usize);
        }
        i += 1;
    }
    None
}

/// The end of the run of non-blank bytes starting at `start` (bounded by
/// `limit`) — the extent a report underlines for a malformed tag, anchor, or
/// block header.
fn non_blank_end(source: &[u8], start: usize, limit: usize) -> usize {
    let mut i = start;
    while i < limit && !is_blank(source[i]) {
        i += 1;
    }
    i
}

/// A plain scalar never ends in a blank: trailing spaces and tabs before a
/// comment or the line's end are not its text (`a: 1\t# c` is `1`).
fn trim_right_blanks(source: &[u8], start: usize, end: usize) -> usize {
    let mut trimmed = end;
    while trimmed > start && is_blank(source[trimmed - 1]) {
        trimmed -= 1;
    }
    trimmed
}

fn whitespace_end(source: &[u8], start: usize, line_end: usize) -> usize {
    let mut end = start;
    while end < line_end && (source[end] == b' ' || source[end] == b'\t') {
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::Kind::*;
    use super::*;

    fn test_tokenizer(input: &str, expected: &[Token]) {
        let mut t = Tokenizer::new(input.as_bytes());
        let tokens = t.tokenize().unwrap();
        assert_eq!(expected, &tokens[..]);
    }

    fn test_tokenizer_error(input: &str, expected: Error) {
        let mut t = Tokenizer::new(input.as_bytes());
        assert_eq!(Err(expected), t.tokenize().map(|_| ()));
    }

    fn tok(kind: Kind, start: usize, end: usize) -> Token {
        Token::new(kind, Span::new(start, end))
    }

    #[test]
    fn yaml_flat_mapping() {
        test_tokenizer(
            "name: Ada\nage: 37\n",
            &[
                tok(Scalar, 0, 4),
                tok(Colon, 4, 5),
                tok(Whitespace, 5, 6),
                tok(Scalar, 6, 9),
                tok(Newline, 9, 10),
                tok(Scalar, 10, 13),
                tok(Colon, 13, 14),
                tok(Whitespace, 14, 15),
                tok(Scalar, 15, 17),
                tok(Newline, 17, 18),
                tok(EndOfFile, 18, 18),
            ],
        );
    }

    #[test]
    fn yaml_tag_core_shorthand_on_mapping_value() {
        test_tokenizer(
            "k: !!int 5\n",
            &[tok(Scalar, 0, 1), tok(Colon, 1, 2), tok(Whitespace, 2, 3), tok(Tag, 3, 8), tok(Whitespace, 8, 9), tok(Scalar, 9, 10), tok(Newline, 10, 11), tok(EndOfFile, 11, 11)],
        );
    }

    #[test]
    fn yaml_tag_local_tag_at_node_start() {
        test_tokenizer("!foo bar\n", &[tok(Tag, 0, 4), tok(Whitespace, 4, 5), tok(Scalar, 5, 8), tok(Newline, 8, 9), tok(EndOfFile, 9, 9)]);
    }

    #[test]
    fn yaml_tag_verbatim_form_scans_to_closing_bracket() {
        test_tokenizer("!<tag:x> y\n", &[tok(Tag, 0, 8), tok(Whitespace, 8, 9), tok(Scalar, 9, 10), tok(Newline, 10, 11), tok(EndOfFile, 11, 11)]);
    }

    #[test]
    fn yaml_tag_bare_non_specific_tag() {
        test_tokenizer("!\n", &[tok(Tag, 0, 1), tok(Newline, 1, 2), tok(EndOfFile, 2, 2)]);
    }

    #[test]
    fn yaml_tag_in_flow_collection() {
        test_tokenizer(
            "[!!str x]\n",
            &[tok(FlowSeqStart, 0, 1), tok(Tag, 1, 6), tok(Whitespace, 6, 7), tok(Scalar, 7, 8), tok(FlowSeqEnd, 8, 9), tok(Newline, 9, 10), tok(EndOfFile, 10, 10)],
        );
    }

    #[test]
    fn yaml_tag_a_bang_mid_scalar_stays_plain_content() {
        test_tokenizer("a!b\n", &[tok(Scalar, 0, 3), tok(Newline, 3, 4), tok(EndOfFile, 4, 4)]);
    }

    #[test]
    fn yaml_anchor_property_before_a_value() {
        test_tokenizer(
            "k: &a 1\n",
            &[tok(Scalar, 0, 1), tok(Colon, 1, 2), tok(Whitespace, 2, 3), tok(Anchor, 3, 5), tok(Whitespace, 5, 6), tok(Scalar, 6, 7), tok(Newline, 7, 8), tok(EndOfFile, 8, 8)],
        );
    }

    #[test]
    fn yaml_alias_whole_node_reference() {
        test_tokenizer("*a\n", &[tok(Alias, 0, 2), tok(Newline, 2, 3), tok(EndOfFile, 3, 3)]);
    }

    #[test]
    fn yaml_anchor_alias_empty_name_and_unseparated_forms_are_invalid() {
        test_tokenizer_error("k: & x\n", err!(InvalidAnchor));
        test_tokenizer_error("k: *\n", err!(InvalidAlias));
        test_tokenizer_error("- &a,b\n", err!(InvalidAnchor));
    }

    #[test]
    fn yaml_anchor_alias_ampersand_and_star_mid_scalar_stay_plain() {
        test_tokenizer("a&b*c\n", &[tok(Scalar, 0, 5), tok(Newline, 5, 6), tok(EndOfFile, 6, 6)]);
    }

    #[test]
    fn yaml_tag_not_separated_from_a_flow_indicator_in_block_is_invalid() {
        test_tokenizer_error("!!str,xxx\n", err!(InvalidTag));
        test_tokenizer_error("!a{}b x\n", err!(InvalidTag));
    }

    #[test]
    fn yaml_flat_sequence() {
        test_tokenizer(
            "- one\n- two\n",
            &[
                tok(Dash, 0, 1),
                tok(Whitespace, 1, 2),
                tok(Scalar, 2, 5),
                tok(Newline, 5, 6),
                tok(Dash, 6, 7),
                tok(Whitespace, 7, 8),
                tok(Scalar, 8, 11),
                tok(Newline, 11, 12),
                tok(EndOfFile, 12, 12),
            ],
        );
    }

    #[test]
    fn yaml_nested_indentation() {
        test_tokenizer(
            "root:\n  child: value\nnext: value\n",
            &[
                tok(Scalar, 0, 4),
                tok(Colon, 4, 5),
                tok(Newline, 5, 6),
                tok(Indent, 6, 8),
                tok(Scalar, 8, 13),
                tok(Colon, 13, 14),
                tok(Whitespace, 14, 15),
                tok(Scalar, 15, 20),
                tok(Newline, 20, 21),
                tok(Dedent, 21, 21),
                tok(Scalar, 21, 25),
                tok(Colon, 25, 26),
                tok(Whitespace, 26, 27),
                tok(Scalar, 27, 32),
                tok(Newline, 32, 33),
                tok(EndOfFile, 33, 33),
            ],
        );
    }

    #[test]
    fn yaml_quoted_scalars_are_single_tokens() {
        test_tokenizer(
            "quoted: 'a: # b'\n",
            &[tok(Scalar, 0, 6), tok(Colon, 6, 7), tok(Whitespace, 7, 8), tok(Scalar, 8, 16), tok(Newline, 16, 17), tok(EndOfFile, 17, 17)],
        );
        test_tokenizer(
            "\"quoted:key\": \"value # not comment\"\n",
            &[tok(Scalar, 0, 12), tok(Colon, 12, 13), tok(Whitespace, 13, 14), tok(Scalar, 14, 35), tok(Newline, 35, 36), tok(EndOfFile, 36, 36)],
        );
    }

    #[test]
    fn yaml_quoted_scalars_must_be_closed() {
        test_tokenizer_error("'unterminated", err!(UnclosedString));
        test_tokenizer_error("\"unterminated", err!(UnclosedString));
        test_tokenizer_error("key: \"a\n  b", err!(UnclosedString));
    }

    #[test]
    fn yaml_multi_line_quoted_scalar_is_one_token() {
        test_tokenizer(
            "k: \"a\n  b\"\n",
            &[tok(Scalar, 0, 1), tok(Colon, 1, 2), tok(Whitespace, 2, 3), tok(Scalar, 3, 10), tok(Newline, 10, 11), tok(EndOfFile, 11, 11)],
        );
    }

    #[test]
    fn yaml_tab_is_separation_not_an_error() {
        test_tokenizer(
            "key:\tvalue\n",
            &[tok(Scalar, 0, 3), tok(Colon, 3, 4), tok(Whitespace, 4, 5), tok(Scalar, 5, 10), tok(Newline, 10, 11), tok(EndOfFile, 11, 11)],
        );
    }

    #[test]
    fn yaml_tab_as_structural_indentation_is_rejected() {
        test_tokenizer_error("a:\n\tb: 1\n", err!(TabIndent));
        test_tokenizer_error("a:\n  \tb: 1\n", err!(TabIndent));
        Tokenizer::new(b"a:\n \tb\n").tokenize().unwrap();
    }

    #[test]
    fn yaml_tab_as_block_scalar_content_vs_indentation() {
        Tokenizer::new(b"foo: |\n \t\nbar: 1\n").tokenize().unwrap();
        test_tokenizer_error("foo: |\n\t\nbar: 1\n", err!(TabIndent));
    }

    #[test]
    fn yaml_tab_only_blank_line_inside_flow_is_skipped() {
        Tokenizer::new(b"- [\n\t\n foo\n ]\n").tokenize().unwrap();
    }

    #[test]
    fn yaml_blank_line_with_a_tab_is_skipped() {
        test_tokenizer(
            "a: 1\n \t\nb: 2\n",
            &[
                tok(Scalar, 0, 1),
                tok(Colon, 1, 2),
                tok(Whitespace, 2, 3),
                tok(Scalar, 3, 4),
                tok(Newline, 4, 5),
                tok(Scalar, 8, 9),
                tok(Colon, 9, 10),
                tok(Whitespace, 10, 11),
                tok(Scalar, 11, 12),
                tok(Newline, 12, 13),
                tok(EndOfFile, 13, 13),
            ],
        );
    }

    #[test]
    fn yaml_multi_line_plain_scalar_is_gathered_into_one_token() {
        test_tokenizer(
            "k: a\n  b\nnext: x\n",
            &[
                tok(Scalar, 0, 1),
                tok(Colon, 1, 2),
                tok(Whitespace, 2, 3),
                tok(Scalar, 3, 8),
                tok(Newline, 8, 9),
                tok(Scalar, 9, 13),
                tok(Colon, 13, 14),
                tok(Whitespace, 14, 15),
                tok(Scalar, 15, 16),
                tok(Newline, 16, 17),
                tok(EndOfFile, 17, 17),
            ],
        );
    }

    #[test]
    fn yaml_multi_line_quoted_scalar_continuation_rules() {
        test_tokenizer_error("\"a\n---\nb\"\n", err!(UnclosedString));
        test_tokenizer_error("k: \"a\nb\"\n", err!(InvalidIndent));
        test_tokenizer_error("k: \"a\n\tb\"\n", err!(InvalidIndent));
    }

    #[test]
    fn yaml_colon_is_an_indicator_only_before_whitespace() {
        test_tokenizer(
            "url: http://example.com\n",
            &[tok(Scalar, 0, 3), tok(Colon, 3, 4), tok(Whitespace, 4, 5), tok(Scalar, 5, 23), tok(Newline, 23, 24), tok(EndOfFile, 24, 24)],
        );
        test_tokenizer("a:b\n", &[tok(Scalar, 0, 3), tok(Newline, 3, 4), tok(EndOfFile, 4, 4)]);
    }

    #[test]
    fn yaml_dash_is_an_indicator_only_before_whitespace() {
        test_tokenizer(
            "value: -3\n",
            &[tok(Scalar, 0, 5), tok(Colon, 5, 6), tok(Whitespace, 6, 7), tok(Scalar, 7, 9), tok(Newline, 9, 10), tok(EndOfFile, 10, 10)],
        );
    }

    #[test]
    fn yaml_explicit_key_indicator() {
        test_tokenizer(
            "? key\n: value\n",
            &[
                tok(ExplicitKey, 0, 1),
                tok(Whitespace, 1, 2),
                tok(Scalar, 2, 5),
                tok(Newline, 5, 6),
                tok(Colon, 6, 7),
                tok(Whitespace, 7, 8),
                tok(Scalar, 8, 13),
                tok(Newline, 13, 14),
                tok(EndOfFile, 14, 14),
            ],
        );
        test_tokenizer("?x\n", &[tok(Scalar, 0, 2), tok(Newline, 2, 3), tok(EndOfFile, 3, 3)]);
    }

    #[test]
    fn yaml_document_markers() {
        test_tokenizer(
            "---\nname: Ada\n...\n",
            &[
                tok(DocStart, 0, 3),
                tok(Newline, 3, 4),
                tok(Scalar, 4, 8),
                tok(Colon, 8, 9),
                tok(Whitespace, 9, 10),
                tok(Scalar, 10, 13),
                tok(Newline, 13, 14),
                tok(DocEnd, 14, 17),
                tok(Newline, 17, 18),
                tok(EndOfFile, 18, 18),
            ],
        );
    }

    #[test]
    fn yaml_directive_line_is_a_single_directive_token() {
        test_tokenizer(
            "%YAML 1.2\n---\n",
            &[tok(Directive, 0, 9), tok(Newline, 9, 10), tok(DocStart, 10, 13), tok(Newline, 13, 14), tok(EndOfFile, 14, 14)],
        );
    }

    #[test]
    fn yaml_flow_collection_indicators_are_distinct_tokens() {
        test_tokenizer(
            "tags: [a, b]\n",
            &[
                tok(Scalar, 0, 4),
                tok(Colon, 4, 5),
                tok(Whitespace, 5, 6),
                tok(FlowSeqStart, 6, 7),
                tok(Scalar, 7, 8),
                tok(Comma, 8, 9),
                tok(Whitespace, 9, 10),
                tok(Scalar, 10, 11),
                tok(FlowSeqEnd, 11, 12),
                tok(Newline, 12, 13),
                tok(EndOfFile, 13, 13),
            ],
        );
    }

    #[test]
    fn yaml_flow_spans_lines_and_ignores_indentation() {
        test_tokenizer(
            "x: [\n  a,\n]\n",
            &[
                tok(Scalar, 0, 1),
                tok(Colon, 1, 2),
                tok(Whitespace, 2, 3),
                tok(FlowSeqStart, 3, 4),
                tok(Newline, 4, 5),
                tok(Scalar, 7, 8),
                tok(Comma, 8, 9),
                tok(Newline, 9, 10),
                tok(FlowSeqEnd, 10, 11),
                tok(Newline, 11, 12),
                tok(EndOfFile, 12, 12),
            ],
        );
    }

    #[test]
    fn yaml_block_scalar_header_and_body_are_single_tokens() {
        test_tokenizer(
            "text: |\n  a\n  b\nnext: x\n",
            &[
                tok(Scalar, 0, 4),
                tok(Colon, 4, 5),
                tok(Whitespace, 5, 6),
                tok(BlockHeader, 6, 7),
                tok(Newline, 7, 8),
                tok(BlockScalar, 8, 16),
                tok(Scalar, 16, 20),
                tok(Colon, 20, 21),
                tok(Whitespace, 21, 22),
                tok(Scalar, 22, 23),
                tok(Newline, 23, 24),
                tok(EndOfFile, 24, 24),
            ],
        );
    }

    #[test]
    fn yaml_block_scalar_header_carries_indicators() {
        test_tokenizer(
            "k: |2-\n    x\n",
            &[tok(Scalar, 0, 1), tok(Colon, 1, 2), tok(Whitespace, 2, 3), tok(BlockHeader, 3, 6), tok(Newline, 6, 7), tok(BlockScalar, 7, 13), tok(EndOfFile, 13, 13)],
        );
    }

    #[test]
    fn yaml_block_scalar_rejects_tabs_and_bad_indentation() {
        test_tokenizer_error("foo: |\n\t\nbar: 1\n", err!(TabIndent));
        test_tokenizer_error("foo: |\n     \n  ok\n", err!(InvalidIndent));
    }

    #[test]
    fn yaml_hash_starts_a_comment_only_after_whitespace() {
        test_tokenizer("a#b\n", &[tok(Scalar, 0, 3), tok(Newline, 3, 4), tok(EndOfFile, 4, 4)]);
        test_tokenizer("a #b\n", &[tok(Scalar, 0, 1), tok(Comment, 2, 4), tok(Newline, 4, 5), tok(EndOfFile, 5, 5)]);
    }

    #[test]
    fn yaml_crlf_line_endings_are_a_single_break_not_a_trailing_cr() {
        test_tokenizer(
            "value: -3\r\n",
            &[tok(Scalar, 0, 5), tok(Colon, 5, 6), tok(Whitespace, 6, 7), tok(Scalar, 7, 9), tok(Newline, 9, 11), tok(EndOfFile, 11, 11)],
        );
        test_tokenizer("a #b\r\n", &[tok(Scalar, 0, 1), tok(Comment, 2, 4), tok(Newline, 4, 6), tok(EndOfFile, 6, 6)]);
        test_tokenizer("---\r\nx\r\n", &[tok(DocStart, 0, 3), tok(Newline, 3, 5), tok(Scalar, 5, 6), tok(Newline, 6, 8), tok(EndOfFile, 8, 8)]);
    }
}
