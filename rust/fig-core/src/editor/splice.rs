//! Port of src/editor/splice.zig.
//!
//! Source-coordinate utilities shared by the editor engine, the regions
//! module and the formats' editor tests.
//!
//! This file used to be the API a format's editing hooks were written
//! against, stated as a file boundary: what a hook could reach was the free
//! functions here and a handful of `Editor` members. There are no hooks any
//! more — a format states facts (`syntax`, what its parser records on
//! `Document`) and spells fragments (the renderers, which are pure string
//! functions) and never receives the editor — so the boundary is simply the
//! engine's own. See `docs/proposals/runtime-languages.md` §4.4.
//!
//! **Source-coordinate utilities.** Editing reframes splice text against the
//! raw source, because indentation, trailing newlines, and comments live
//! *outside* any AST node span (node spans are tight: they exclude leading
//! indent and, except for block scalars, the trailing newline; comments are
//! not represented in the AST at all). The functions below are that
//! arithmetic, shared by the engine and `editor/regions.rs`.
//!
//! The byte-slice helpers at the bottom (`find`, `trim`, …) are Zig's
//! `std.mem` functions the engine leans on, with Zig's semantics (an empty
//! needle is found at the start; `is_whitespace` is `std.ascii`'s set).

use crate::kit::manifest::Syntax;

// The line helpers languages share are the kit's (`kit/lines.rs`).
pub use crate::kit::lines::{
    at_line_start, column_of, first_non_space, flow_open_end, is_flow, line_end_after, line_start_before, text_start,
};

/// The comment syntax the owned-comment scan reads: the language's declared
/// delimiters. Re-exported from the language manifest, where it lives so a
/// language can name it in its own `syntax` without importing the editor.
pub use crate::kit::manifest::Comments;

/// Grow `line_start` upward to absorb an entry's owned comment block: the
/// contiguous run of comment lines immediately above, with no intervening blank
/// line (trivia policy "comment-above-belongs-to-key"). A blank line or any
/// non-comment content stops the scan. A comment line is one that opens with
/// the declared `line` delimiter, or holds the whole of it when it is a pair
/// (`<!-- ... -->`, recognized on one line only, so a delete never
/// half-swallows a block); a declared `block` pair (`/* ... */`) is walked as
/// a unit across lines, so a delete or move carries the whole block, not just
/// its closing line.
pub fn comment_block_start(source: &[u8], line_start: usize, comments: Comments) -> usize {
    let mut ls = line_start;
    // Set while scanning upward through the interior of a `block` comment
    // whose opener has not been reached yet.
    let mut in_block = false;
    while ls > 0 {
        let prev_start = line_start_before(source, ls - 1);
        let line = &source[prev_start..ls];
        let trimmed = trim_start(trim_end(line, b"\r\n"), b" \t");
        let is_comment = 'blk: {
            if let Some(b) = comments.block {
                if in_block {
                    // Inside a block comment, moving up: every line belongs to
                    // it until we reach the line bearing the opener.
                    if find(trimmed, b.open).is_some() {
                        in_block = false;
                    }
                    break 'blk true;
                }
                // A line ending a block: enter block-scan mode unless it is a
                // self-contained single-line one.
                if !b.close.is_empty() && trimmed.ends_with(b.close) {
                    if !trimmed.starts_with(b.open) {
                        in_block = true;
                    }
                    break 'blk true;
                }
            }
            let Some(l) = comments.line else { break 'blk false };
            if l.open.is_empty() || !trimmed.starts_with(l.open) {
                break 'blk false;
            }
            l.close.is_empty() || trimmed.ends_with(l.close)
        };
        if is_comment {
            ls = prev_start;
        } else {
            break;
        }
    }
    ls
}

/// Whether the line that ends at `end` — `text[0..end]`'s last line, its
/// line ending included — carries on to the next physical line: it ends in
/// an odd run of `syn.line_continuation`, and is not a comment line. Always
/// false for a format whose lines never continue.
pub fn continues_onto(text: &[u8], end: usize, syn: &Syntax) -> bool {
    let Some(byte) = syn.line_continuation else { return false };
    if end == 0 || text[end - 1] != b'\n' {
        return false;
    }
    let start = line_start_before(text, end - 1);
    let line = trim_end(&text[start..end], b"\r\n");
    if let Some(l) = syn.comments.line {
        if !l.open.is_empty() && trim_start(line, b" \t").starts_with(l.open) {
            return false;
        }
    }
    let mut run = 0usize;
    while run < line.len() && line[line.len() - 1 - run] == byte {
        run += 1;
    }
    run % 2 == 1
}

/// Whether `text` opens on a blank line, which ends a continuation the line
/// before it left open.
fn opens_blank(text: &[u8]) -> bool {
    text.starts_with(b"\n") || text.starts_with(b"\r\n")
}

/// The blank line, or none, that must stand between `before` — the bytes
/// up to a line start — and a line `next` written there, so that `next` is
/// not read as the rest of a line `before` leaves continued.
pub fn continuation_end(before: &[u8], next: &[u8], syn: &Syntax) -> &'static [u8] {
    if next.is_empty() || opens_blank(next) || !continues_onto(before, before.len(), syn) {
        return b"";
    }
    b"\n"
}

/// Append a relocated entry `block` to `out`, guaranteeing a single '\n'
/// separator from whatever precedes it. The block's own bytes are appended
/// verbatim (its trailing newline, if any, is preserved), so concatenating
/// blocks in a new order never welds two entries onto one line, nor writes
/// one after a line that continues (`continuation_end`).
pub fn append_block_sep(out: &mut Vec<u8>, block: &[u8], syn: &Syntax) {
    if !out.is_empty() && out[out.len() - 1] != b'\n' {
        out.push(b'\n');
    }
    let end = continuation_end(out, block, syn);
    out.extend_from_slice(end);
    out.extend_from_slice(block);
}

/// A relocatable entry block: a byte range `[start, end)` covering one mapping
/// entry or sequence item (its owned comment block through its last line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub start: usize,
    pub end: usize,
}

/// Fill each block's `end` from the next block's `start` so the blocks tile a
/// contiguous region; the final block runs to `last_end`. Trailing trivia (a
/// blank line, an orphan comment) thus rides with the preceding entry.
pub fn tile_blocks(blocks: &mut [Block], last_end: usize) {
    let n = blocks.len();
    for i in 0..n {
        blocks[i].end = if i + 1 < n { blocks[i + 1].start } else { last_end };
    }
}

/// Build a full permutation of `0..n`: the valid, de-duplicated indices in
/// `order` first (in the given order), then every remaining index in ascending
/// (original) order. An empty `order` yields the identity, so a reorder with
/// nothing to bring forward is a no-op.
pub fn full_order(order: &[usize], n: usize) -> Vec<usize> {
    let mut result = Vec::with_capacity(n);
    let mut used = vec![false; n];
    for &idx in order {
        if idx < n && !used[idx] {
            result.push(idx);
            used[idx] = true;
        }
    }
    for (i, u) in used.iter().enumerate() {
        if !u {
            result.push(i);
        }
    }
    result
}

// ── Zig's `std.mem` helpers, with Zig's semantics ───────────────────────────

/// `std.mem.indexOf`: the first index of `needle` in `hay`; an empty needle
/// is found at 0.
pub fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    find_from(hay, 0, needle)
}

/// `std.mem.indexOfPos`: the first index at or after `start` of `needle`.
pub fn find_from(hay: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() || start > hay.len() {
        return None;
    }
    if needle.is_empty() {
        return Some(start);
    }
    let last = hay.len() - needle.len();
    let mut i = start;
    while i <= last {
        if &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// `std.mem.indexOfScalar`.
pub fn find_byte(hay: &[u8], b: u8) -> Option<usize> {
    hay.iter().position(|&c| c == b)
}

/// `std.mem.indexOfScalarPos`.
pub fn find_byte_from(hay: &[u8], start: usize, b: u8) -> Option<usize> {
    if start >= hay.len() {
        return None;
    }
    hay[start..].iter().position(|&c| c == b).map(|i| start + i)
}

/// `std.mem.indexOfAny`.
pub fn find_any(hay: &[u8], set: &[u8]) -> Option<usize> {
    hay.iter().position(|c| set.contains(c))
}

/// `std.mem.trimStart`.
pub fn trim_start<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let mut i = 0;
    while i < s.len() && set.contains(&s[i]) {
        i += 1;
    }
    &s[i..]
}

/// `std.mem.trimEnd`.
pub fn trim_end<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let mut e = s.len();
    while e > 0 && set.contains(&s[e - 1]) {
        e -= 1;
    }
    &s[..e]
}

/// `std.mem.trim`.
pub fn trim<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    trim_start(trim_end(s, set), set)
}

/// `std.ascii.isWhitespace`: space, `\t`, `\n`, `\r`, VT and FF.
pub fn is_whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::manifest::{CommentDelimiter, Comments};

    #[test]
    fn comment_block_start_climbs_a_run_and_stops_at_a_blank() {
        let src = b"a: 1\n\n# one\n# two\nb: 2\n";
        let b_line = 18;
        assert_eq!(b"b: 2\n", &src[b_line..]);
        assert_eq!(6, comment_block_start(src, b_line, Comments::HASH));
        assert_eq!(0, comment_block_start(src, 0, Comments::HASH));
    }

    #[test]
    fn comment_block_start_walks_a_block_comment_whole() {
        let c = Comments {
            line: Some(CommentDelimiter::open(b"//")),
            trailing: Some(CommentDelimiter::open(b"//")),
            block: Some(CommentDelimiter::pair(b"/*", b"*/")),
        };
        let src = b"{\n  /* a\n     b */\n  \"k\": 1\n}";
        let k_line = 19;
        assert_eq!(b"  \"k\"", &src[k_line..k_line + 5]);
        assert_eq!(2, comment_block_start(src, k_line, c));
    }

    #[test]
    fn a_continued_line_is_ended_before_a_line_written_after_it() {
        let syn = Syntax { line_continuation: Some(b'\\'), ..Syntax::new(Comments::HASH, Some(b"="), None) };
        assert!(continues_onto(b"a=1 \\\n", 6, &syn));
        assert!(!continues_onto(b"a=1 \\\\\n", 7, &syn));
        assert!(!continues_onto(b"# x \\\n", 6, &syn));
        assert_eq!(b"\n", continuation_end(b"a=1 \\\n", b"b=2\n", &syn));
        assert_eq!(b"", continuation_end(b"a=1 \\\n", b"\nb=2\n", &syn));
        let mut out = b"a=1 \\".to_vec();
        append_block_sep(&mut out, b"b=2\n", &syn);
        assert_eq!(b"a=1 \\\n\nb=2\n", &out[..]);
    }

    #[test]
    fn full_order_and_tiles() {
        assert_eq!(vec![2, 0, 1, 3], full_order(&[2, 9, 2, 0], 4));
        assert_eq!(vec![0, 1, 2], full_order(&[], 3));
        let mut b = [Block { start: 0, end: 0 }, Block { start: 5, end: 0 }];
        tile_blocks(&mut b, 9);
        assert_eq!((5, 9), (b[0].end, b[1].end));
    }

    #[test]
    fn std_mem_helpers() {
        assert_eq!(Some(0), find(b"abc", b""));
        assert_eq!(Some(2), find_from(b"abcabc", 1, b"ca"));
        assert_eq!(None, find_from(b"abc", 4, b""));
        assert_eq!(b"x", trim(b" \tx\n", b" \t\n"));
        assert!(is_whitespace(0x0B) && !is_whitespace(b'x'));
    }
}
