//! Port of languages/nestedtext/src/tokenizer.zig.
//!
//! NestedText tokenizer. Turns NestedText bytes into a list of `Line`s — ONE
//! entry per physical line, already classified by the tag character(s) that
//! start its content. This is a deliberate departure from every other
//! tokenizer in this codebase (which emit a character-level token stream via
//! the shared `Token<Kind>`): NestedText's grammar is fundamentally
//! "classify this whole line, then decide how it relates to the lines around
//! it by indentation" (much closer to a pre-split line list than a token
//! stream), and unlike YAML there is no flow-context that needs
//! character-level tokens — inline `{...}`/`[...]` forms are always the
//! entire remainder of one line, parsed by a tiny separate scanner in
//! `parser.rs`, not by this tokenizer.
//!
//! Per line, after stripping a run of leading `' '` (space) bytes:
//!   - nothing left (or only trailing whitespace) → `Blank`
//!   - `#` → `Comment` (content = rest of line after the `#`)
//!   - `-` followed by `' '` or end-of-line → `Dash` (list-item tag)
//!   - `:` followed by `' '` or end-of-line → `Colon` (dict-item tag /
//!     multiline-key line)
//!   - `>` followed by `' '` or end-of-line → `Gt` (string-item tag)
//!   - anything else → `Other` — the parser decides whether this is a
//!     `key: value` dict item (scan for the first `: `/trailing `:`), an
//!     inline `{...}`/`[...]` candidate, or unrecognized.
//!
//! For the four tag kinds, `content` is everything after the tag *and* the
//! one mandatory separating space (if any followed) — i.e. exactly what
//! NestedText calls the "rest-of-line" text for that tag. A bare tag with
//! nothing after it (not even a space) or a tag followed immediately by a
//! single space then end-of-line both yield an EMPTY `content` span, which
//! is how the parser recognizes "this item's value is on nested,
//! more-indented lines" per the spec. For `Other`, `content` is the entire
//! line (the parser does its own scanning).
//!
//! Indentation must be plain `' '` bytes; a literal tab or a non-breaking
//! space (U+00A0, the one non-ASCII whitespace character the official
//! conformance suite tests for) in the leading run is `InvalidIndentChar`.

use crate::err;
use crate::kit::span::Span;
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Blank,
    Comment,
    Dash,
    Colon,
    Gt,
    Other,
    EndOfFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    /// Count of leading `' '` bytes (0 for a blank line with none).
    pub indent: usize,
    /// Byte offset where this physical line begins (the first indentation
    /// byte, or the content byte if indent is 0).
    pub line_start: usize,
    /// See the kind-by-kind breakdown in the module doc comment above.
    pub content: Span,
    /// 1-based, for diagnostics.
    pub line_no: usize,
}

/// `TokenizeError`: `InvalidIndentChar`.
pub struct Tokenizer<'a> {
    pub str: &'a [u8],
    pub i: usize,
    pub line_no: usize,
    pub lines: Vec<Line>,
}

impl<'a> Tokenizer<'a> {
    pub fn new(str: &'a [u8]) -> Tokenizer<'a> {
        Tokenizer { str, i: 0, line_no: 0, lines: Vec::new() }
    }

    /// Every line, ending with an `EndOfFile` line. Takes the lines out of
    /// the tokenizer.
    pub fn tokenize(&mut self) -> Result<Vec<Line>> {
        if self.str.starts_with(b"\xEF\xBB\xBF") {
            self.i = 3; // BOM
        }

        while self.i < self.str.len() {
            if let Err(e) = self.tokenize_line() {
                self.lines.clear();
                return Err(e);
            }
        }

        self.lines.push(Line {
            kind: Kind::EndOfFile,
            indent: 0,
            line_start: self.str.len(),
            content: Span::new(self.str.len(), self.str.len()),
            line_no: self.line_no + 1,
        });
        Ok(std::mem::take(&mut self.lines))
    }

    fn tokenize_line(&mut self) -> Result<()> {
        self.line_no += 1;
        let line_start = self.i;
        let s = self.str;

        let mut j = self.i;
        while j < s.len() && s[j] == b' ' {
            j += 1;
        }

        let mut end = j;
        while end < s.len() && s[end] != b'\n' && s[end] != b'\r' {
            end += 1;
        }

        let indent = j - line_start;

        if j == end {
            self.emit(Kind::Blank, indent, line_start, Span::new(end, end));
            self.advance_line(end);
            return Ok(());
        }

        if s[j] == b'\t' {
            return Err(err!(InvalidIndentChar));
        }
        if j + 1 < s.len() && s[j] == 0xC2 && s[j + 1] == 0xA0 {
            return Err(err!(InvalidIndentChar)); // U+00A0 NO-BREAK SPACE
        }

        let c = s[j];
        if c == b'#' {
            self.emit(Kind::Comment, indent, line_start, Span::new(j + 1, end));
        } else if c == b'-' && (j + 1 == end || s[j + 1] == b' ') {
            self.emit(Kind::Dash, indent, line_start, tag_content(j, end));
        } else if c == b':' && (j + 1 == end || s[j + 1] == b' ') {
            self.emit(Kind::Colon, indent, line_start, tag_content(j, end));
        } else if c == b'>' && (j + 1 == end || s[j + 1] == b' ') {
            self.emit(Kind::Gt, indent, line_start, tag_content(j, end));
        } else {
            self.emit(Kind::Other, indent, line_start, Span::new(j, end));
        }
        self.advance_line(end);
        Ok(())
    }

    fn emit(&mut self, kind: Kind, indent: usize, line_start: usize, content: Span) {
        self.lines.push(Line { kind, indent, line_start, content, line_no: self.line_no });
    }

    /// Advance past the line terminator (if any) following content-end
    /// `end`: `\r\n`, bare `\n`, or bare `\r` are all accepted (lenient —
    /// NestedText's own spec assumes `\n`, but there's no conformance case
    /// exercising `\r` handling, so this mirrors the common lenient practice
    /// elsewhere in fig rather than inventing a new error path).
    fn advance_line(&mut self, end: usize) {
        let mut k = end;
        if k < self.str.len() && self.str[k] == b'\r' {
            k += 1;
        }
        if k < self.str.len() && self.str[k] == b'\n' {
            k += 1;
        }
        self.i = k;
    }
}

/// Content span for a one-byte tag at `tag_pos` in a line ending at `end`
/// (exclusive of the line terminator): consumes the tag plus, if present,
/// the one mandatory separating space, leaving whatever (possibly nothing)
/// comes after as the rest-of-line content.
fn tag_content(tag_pos: usize, end: usize) -> Span {
    let after_tag = tag_pos + 1;
    // If there's at least one more byte before end-of-line, the caller's
    // dispatch condition already guarantees it's the mandatory ' ' — skip it.
    if after_tag < end {
        return Span::new(after_tag + 1, end);
    }
    Span::new(end, end)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize_all(input: &[u8]) -> Result<Vec<Line>> {
        Tokenizer::new(input).tokenize()
    }

    fn text<'a>(src: &'a [u8], l: &Line) -> &'a [u8] {
        &src[l.content.start..l.content.end]
    }

    #[test]
    fn classifies_dict_list_string_tags_and_blank_comment_lines() {
        let src = b"key: value\n- item\n> str\n\n# comment\n";
        let lines = tokenize_all(src).unwrap();
        assert_eq!(Kind::Other, lines[0].kind);
        assert_eq!(b"key: value", text(src, &lines[0]));
        assert_eq!(Kind::Dash, lines[1].kind);
        assert_eq!(b"item", text(src, &lines[1]));
        assert_eq!(Kind::Gt, lines[2].kind);
        assert_eq!(b"str", text(src, &lines[2]));
        assert_eq!(Kind::Blank, lines[3].kind);
        assert_eq!(Kind::Comment, lines[4].kind);
        assert_eq!(b" comment", text(src, &lines[4]));
    }

    #[test]
    fn dash_colon_gt_tags_with_and_without_content() {
        let src = b"- a\n-\n: b\n:\n> c\n>\n";
        let lines = tokenize_all(src).unwrap();
        assert_eq!(Kind::Dash, lines[0].kind);
        assert_eq!(b"a", text(src, &lines[0]));
        assert_eq!(Kind::Dash, lines[1].kind);
        assert_eq!(0, lines[1].content.len());
        assert_eq!(Kind::Colon, lines[2].kind);
        assert_eq!(b"b", text(src, &lines[2]));
        assert_eq!(Kind::Colon, lines[3].kind);
        assert_eq!(0, lines[3].content.len());
        assert_eq!(Kind::Gt, lines[4].kind);
        assert_eq!(b"c", text(src, &lines[4]));
        assert_eq!(Kind::Gt, lines[5].kind);
        assert_eq!(0, lines[5].content.len());
    }

    #[test]
    fn indentation_is_counted_and_tabs_are_rejected() {
        let lines = tokenize_all(b"  - a\n").unwrap();
        assert_eq!(2, lines[0].indent);

        assert_eq!(err!(InvalidIndentChar), tokenize_all(b"\t> a\n").unwrap_err());
        assert_eq!(err!(InvalidIndentChar), tokenize_all(b"  \xC2\xA0 > a\n").unwrap_err());
    }

    #[test]
    fn other_line_carries_the_whole_rest_of_line_for_the_parser_to_scan() {
        let src = b"regex: [+-]?[0-9]+\n";
        let lines = tokenize_all(src).unwrap();
        assert_eq!(Kind::Other, lines[0].kind);
        assert_eq!(b"regex: [+-]?[0-9]+", text(src, &lines[0]));
    }

    #[test]
    fn trailing_end_of_file_token() {
        let lines = tokenize_all(b"a: b\n").unwrap();
        assert_eq!(Kind::EndOfFile, lines[lines.len() - 1].kind);
    }
}
