//! Port of kit/src/lines.zig: positions in source text by line — where a line
//! starts and ends, its first non-blank byte, a byte's column, and whether a
//! container opens in flow. Shared by the editor's splices and the languages'
//! renderers.

use super::span::Span;

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Byte index of the start of the line containing `at` (just past the previous
/// '\n', or 0). The first line starts after a leading UTF-8 BOM, which is
/// not text of the line: a column counts from past it, and a splice at the
/// line's start leaves it first in the file.
pub fn line_start_before(source: &[u8], at: usize) -> usize {
    let mut i = at;
    while i > 0 {
        if source[i - 1] == b'\n' {
            return i;
        }
        i -= 1;
    }
    let text = text_start(source);
    if at >= text { text } else { 0 }
}

/// Whether `at` is the start of its line: just past a '\n', or the start of
/// the text (`text_start`), or 0.
pub fn at_line_start(source: &[u8], at: usize) -> bool {
    line_start_before(source, at) == at
}

/// Where the text of `source` begins: past a leading UTF-8 BOM, else 0.
pub fn text_start(source: &[u8]) -> usize {
    if source.starts_with(BOM) { BOM.len() } else { 0 }
}

/// Byte index just past the next '\n' at or after `at`, or `source.len()`.
pub fn line_end_after(source: &[u8], at: usize) -> usize {
    match source[at..].iter().position(|&c| c == b'\n') {
        Some(nl) => at + nl + 1,
        None => source.len(),
    }
}

/// Index of the first non-space/non-tab byte at or after `from`.
pub fn first_non_space(source: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < source.len() && (source[i] == b' ' || source[i] == b'\t') {
        i += 1;
    }
    i
}

/// Column (0-based) of the byte at `at` within its line.
pub fn column_of(source: &[u8], at: usize) -> usize {
    at - line_start_before(source, at)
}

/// Whether the container at `span` is written in flow style: it opens with
/// `{`, `[`, or the language's own `flow_map_open` (ZON's `.{`, which opens
/// its arrays too). The AST records no flow/block flag, so we sniff the
/// first content bytes.
pub fn is_flow(source: &[u8], span: Span, flow_map_open: &[u8]) -> bool {
    let i = first_non_space(source, span.start);
    if i >= source.len() {
        return false;
    }
    if source[i] == b'{' || source[i] == b'[' {
        return true;
    }
    source[i..].starts_with(flow_map_open)
}

/// Byte index just past the flow container's opener at `span`: the
/// language's `flow_map_open` where it opens with that, else the one-byte
/// `{` or `[`.
pub fn flow_open_end(source: &[u8], span: Span, flow_map_open: &[u8]) -> usize {
    let i = first_non_space(source, span.start);
    if source[i..].starts_with(flow_map_open) {
        return i + flow_map_open.len();
    }
    i + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_bom_is_not_text_of_the_first_line() {
        let src = b"\xEF\xBB\xBFa: 1\nb: 2\n";
        assert_eq!(line_start_before(src, 3), 3);
        assert_eq!(line_start_before(src, 5), 3);
        assert_eq!(column_of(src, 3), 0);
        assert_eq!(column_of(src, 5), 2);
        assert_eq!(line_start_before(src, 9), 8);
        assert!(at_line_start(src, 3) && at_line_start(src, 8) && !at_line_start(src, 4));
        // Inside the BOM itself, the line is the file's start.
        assert_eq!(line_start_before(src, 2), 0);
        assert_eq!(text_start(b"a: 1\n"), 0);
    }

    #[test]
    fn line_ends_and_flow_openers() {
        let src = b"a: 1\nb: [2]\n";
        assert_eq!(line_end_after(src, 0), 5);
        assert_eq!(line_end_after(src, 5), 12);
        assert_eq!(line_end_after(b"abc", 1), 3);
        assert_eq!(first_non_space(b"  \tx", 0), 3);
        assert!(is_flow(src, Span::new(7, 10), b"{"));
        assert!(!is_flow(src, Span::new(0, 4), b"{"));
        assert!(!is_flow(src, Span::new(12, 12), b"{"));
        assert!(is_flow(b"x = .{ 1 }", Span::new(3, 10), b".{"));
        assert_eq!(flow_open_end(b"x = .{ 1 }", Span::new(3, 10), b".{"), 6);
        assert_eq!(flow_open_end(b"  [1]", Span::new(0, 5), b".{"), 3);
    }
}
