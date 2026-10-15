//! Port of kit/src/parse_diagnostic.zig: shared parse-diagnostic rendering —
//! the language-agnostic half of fig's diagnostic system (see
//! `languages/figl/parser.zig`'s `Location`/`Diagnostic`/`Warning`, and
//! DESIGN.md's "Authoring-time diagnostics"): locating a byte offset in source
//! text, and rendering the compiler-style `file:line:col: label: message`
//! report.
//!
//! Each language keeps its OWN `Error`/`Warning.Code` enums and `describe()`/
//! `short_label()` teaching-message functions (the failure modes differ per
//! format) and its own trivial `Diagnostic { code, offset, end }` /
//! `Report { diag, errors, warnings }` shape. Only the offset-independent-of-
//! code machinery lives here, so there is exactly one implementation of "find
//! line/col for a byte offset" in the whole codebase, not one per language.
//!
//! `Rendered` is the CLI/LSP-facing product: a language's `Diagnostic`/
//! `Warning` (which carries a typed `code` only that language's `describe`/
//! `short_label` know how to read) gets converted to a plain `Rendered` once,
//! right after parsing — so any generic consumer needs no per-language
//! knowledge at all.

/// 1-based line/column plus the full offending source line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location<'a> {
    pub line: usize,
    pub column: usize,
    pub line_text: &'a [u8],
}

/// Locate `offset` in `source`. A cursor resting exactly past a newline is
/// ambiguous from the offset alone: it could mean "the problem was noticed
/// while finishing the previous line" (an unclosed container discovered only
/// at EOF) — report end-of-that-line, not column 1 of an empty next one — OR
/// it could be a token that genuinely STARTS at column 1 of a real next line
/// (e.g. a duplicate TOML key/table). Only backtrack in the FORMER case, when
/// there is nothing at `at` to point at instead (EOF, or another blank line) —
/// a real token at column 1 must be attributed to ITS OWN line, never silently
/// reattributed to the one before it.
pub fn locate_offset(source: &[u8], offset: usize) -> Location<'_> {
    let mut at = offset.min(source.len());
    if at > 0
        && source[at - 1] == b'\n'
        && (at >= source.len() || source[at] == b'\n' || source[at] == b'\r')
    {
        at -= 1;
    }
    let mut line = 1;
    let mut line_start = 0;
    for (i, &c) in source[..at].iter().enumerate() {
        if c == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    let line_end = source[line_start..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(source.len(), |n| line_start + n);
    Location { line, column: at - line_start + 1, line_text: &source[line_start..line_end] }
}

/// The compiler-style report every language shares: `file:line:col: <label>:
/// <message>`, then the offending source line and a caret marking the column.
/// Plain/colorless — for plain-text consumers (library callers, tests,
/// anything that just wants a string).
pub fn render_report(w: &mut Vec<u8>, loc: Location<'_>, file: &[u8], label: &[u8], message: &[u8]) {
    w.extend_from_slice(file);
    w.extend_from_slice(format!(":{}:{}: ", loc.line, loc.column).as_bytes());
    w.extend_from_slice(label);
    w.extend_from_slice(b": ");
    w.extend_from_slice(message);
    w.push(b'\n');
    // The offending line, capped so a pathological line can't flood the
    // terminal. The caret line mirrors tabs so it stays aligned under them.
    const MAX_SHOWN: usize = 160;
    let shown = &loc.line_text[..loc.line_text.len().min(MAX_SHOWN)];
    if shown.is_empty() {
        return; // EOF/blank line: nothing to point into
    }
    w.extend_from_slice(b"    ");
    w.extend_from_slice(shown);
    if shown.len() < loc.line_text.len() {
        w.extend_from_slice("…".as_bytes());
    }
    w.push(b'\n');
    if loc.column - 1 <= shown.len() {
        w.extend_from_slice(b"    ");
        for &c in &shown[..loc.column - 1] {
            w.push(if c == b'\t' { b'\t' } else { b' ' });
        }
        w.extend_from_slice(b"^\n");
    }
}

/// `render_report`, returning the result.
pub fn render_report_alloc(source: &[u8], offset: usize, file: &[u8], label: &[u8], message: &[u8]) -> Vec<u8> {
    let loc = locate_offset(source, offset);
    let mut out = Vec::new();
    render_report(&mut out, loc, file, label, message);
    out
}

/// A fully-resolved diagnostic ready for CLI/LSP rendering — language-agnostic.
/// Pairs a parse-time `[offset, end)` span with the language's own
/// `describe(code)` (long teaching message) / `short_label(code)` (a few
/// words, for a caret annotation) strings, computed once at the call site
/// right after parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rendered<'a> {
    pub offset: usize,
    pub end: Option<usize>,
    pub message: &'a [u8],
    pub short_label: &'a [u8],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_offset_finds_line_column_and_backtracks_past_a_trailing_newline() {
        let src = b"abc\ndef\nghi";
        let loc = locate_offset(src, 5); // 'e' on line 2
        assert_eq!(loc.line, 2);
        assert_eq!(loc.column, 2);
        assert_eq!(loc.line_text, b"def");

        // Resting exactly past a newline at true EOF (nothing follows) reports
        // the end of the PREVIOUS line, not column 1 of an empty next one.
        let loc = locate_offset(b"abc\n", 4);
        assert_eq!(loc.line, 1);
        assert_eq!(loc.column, 4);
        assert_eq!(loc.line_text, b"abc");

        // Same trailing-newline offset, but a BLANK line (not EOF) follows —
        // still nothing there to point at, so this also backtracks.
        let loc = locate_offset(b"abc\n\ndef", 4);
        assert_eq!(loc.line, 1);
        assert_eq!(loc.column, 4);

        // But when the offset right after a newline is the start of a line
        // with REAL content, it must NOT be backtracked onto the previous line.
        let loc = locate_offset(b"abc\ndef\n", 4);
        assert_eq!(loc.line, 2);
        assert_eq!(loc.column, 1);
        assert_eq!(loc.line_text, b"def");

        // Past the end clamps.
        let loc = locate_offset(b"ab", 99);
        assert_eq!((loc.line, loc.column), (1, 3));
        let loc = locate_offset(b"", 0);
        assert_eq!((loc.line, loc.column, loc.line_text), (1, 1, &b""[..]));
    }

    #[test]
    fn render_report_alloc_renders_file_line_col_plus_the_source_line_and_caret() {
        let rendered = render_report_alloc(b"x = 1\ny = ?\n", 4, b"test.txt", b"error", b"bad value");
        assert_eq!(rendered, b"test.txt:1:5: error: bad value\n    x = 1\n        ^\n".to_vec());
    }

    #[test]
    fn render_report_mirrors_tabs_caps_long_lines_and_skips_blank_ones() {
        let r = render_report_alloc(b"\tx = ?", 5, b"f", b"error", b"m");
        assert_eq!(r, b"f:1:6: error: m\n    \tx = ?\n    \t    ^\n".to_vec());
        let r = render_report_alloc(b"a\n\n", 2, b"f", b"warning", b"m");
        assert_eq!(r, b"f:1:2: warning: m\n    a\n     ^\n".to_vec());
        let r = render_report_alloc(b"", 0, b"f", b"error", b"m");
        assert_eq!(r, b"f:1:1: error: m\n".to_vec());
        let long = vec![b'x'; 200];
        let r = render_report_alloc(&long, 180, b"f", b"error", b"m");
        let mut want = b"f:1:181: error: m\n    ".to_vec();
        want.extend_from_slice(&long[..160]);
        want.extend_from_slice("…\n".as_bytes());
        assert_eq!(r, want);
    }
}
