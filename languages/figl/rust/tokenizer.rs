//! Port of languages/figl/src/tokenizer.zig.
//!
//! fig authoring dialect — low-level scanning helpers shared by the block-layer
//! and flow-mode parsers (see `parser.rs`). These are pure functions/structs
//! operating on caller-owned byte slices; no parser state lives here, mirroring
//! how the TOML tokenizer separates lexical classification from tree assembly.
//!
//! Scope (see DESIGN.md "Grammar" / "The context-sensitive layer"):
//!   * `sniff_bare` — the literal-else-string bare-token classifier (rule 3).
//!   * Quoted / multiline string scanning (rule 5, "suspended regions").
//!   * Bare-key character classification.

use fig_kit::err;
use fig_kit::Result;

pub use fig_kit::ast::ExtKind;
/// The literal-else-string classifier is the contract's, not figl's alone:
/// the editor types a caller's value text by the same rule. It lives in
/// the kit's `literal.rs`, shared by core and this tokenizer.
pub use fig_kit::literal::{sniff_bare, sniff_number, NumberResult, Sniffed};

/// A bare key/identifier char: letters, digits, `_`, `-`.
pub fn is_bare_key_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

/// How a `[`/`{`-led block RHS commits (DESIGN.md "Committed values"): commitment
/// is a property of the *whole* RHS, not just the first char.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BracketCommit {
    /// The matching close is the final non-comment token on the line → parse as
    /// flow (committed; a later parse failure is a hard error).
    Flow,
    /// A balanced close is followed by more non-comment content (`[text](url)`, a
    /// glob, a regex) → never flow; the whole line is a bare string.
    BareTrailing,
    /// The bracket does not close on this line → multi-line flow region *or* a
    /// genuine truncation. Handed to the flow parser (it scans across lines and
    /// raises FigUnclosedFlow at EOF).
    Unclosed,
}

/// Decide how a `[`/`{`-led block RHS commits. `start` indexes the opening
/// bracket; scanning stops at the first newline (bare block values are
/// single-line). Quoted *elements* are skipped so brackets inside strings don't
/// count toward matching (`['[Blog](x)']` closes at its final `]`, not the inner
/// one); a quote that the flow grammar would read as ordinary text is ordinary
/// text here too — see `quote_opens_span`.
pub fn classify_bracket_commit(source: &[u8], start: usize) -> BracketCommit {
    let Some(close) = bracket_close_index(source, start) else { return BracketCommit::Unclosed };
    if rest_is_comment_only(source, close + 1) {
        BracketCommit::Flow
    } else {
        BracketCommit::BareTrailing
    }
}

/// May a `'`/`"` at this point open a quoted span? Only where the flow grammar
/// would begin a quoted value or key: at an **element start** — the first
/// non-whitespace byte after the opening `[`/`{`, after a `,`, or after a
/// `:`/`=` pair separator (`parse_flow_scalar_or_nested`/`scan_flow_key_path`
/// in parser.rs peek exactly there). `prev` is the last non-whitespace byte
/// scanned, or 0 at the start of the scan.
///
/// Anywhere else a quote is ordinary bare-value text, because that is how
/// `scan_flow_bare_value` reads it: `[its parent's comment](x.md)` has a bare
/// element with an apostrophe in it, so the apostrophe opens nothing and the
/// bracket still balances at the `]`. Treating a mid-word quote as an opener
/// used to run the span off the end of the line, making the value `Unclosed`
/// and handing a markdown link to the flow parser.
///
/// `:`/`=` are honoured in sequence position too, where the flow grammar has no
/// pair separator, since admitting them keeps the pre-existing reading of a
/// shape (`[a: 'b]c']`) that is malformed under either reading.
///
/// Inside a `{`, whitespace is an element start as well: a compact object's
/// pairs are whitespace-separated, so a quoted key may follow a space
/// (`{a=1 "k}"=2}`). `BracketWalk` tracks which bracket is innermost for that.
fn quote_opens_span(prev: u8) -> bool {
    matches!(prev, 0 | b'[' | b'{' | b',' | b':' | b'=')
}

/// The bracket-matching scanners' state: nesting depth, which open brackets
/// are `{` (one bit per depth; past 64 levels a bracket counts as `[`), the
/// last non-whitespace byte, and whether whitespace came after it.
#[derive(Default)]
struct BracketWalk {
    depth: usize,
    braces: u64,
    prev: u8,
    prev_ws: bool,
}

impl BracketWalk {
    fn in_brace(&self) -> bool {
        if self.depth == 0 || self.depth > 64 {
            return false;
        }
        (self.braces >> (self.depth - 1)) & 1 == 1
    }

    fn quote_opens(&self) -> bool {
        quote_opens_span(self.prev) || (self.prev_ws && self.in_brace())
    }

    fn open(&mut self, c: u8) {
        if self.depth < 64 {
            let bit = 1u64 << self.depth;
            if c == b'{' {
                self.braces |= bit;
            } else {
                self.braces &= !bit;
            }
        }
        self.depth += 1;
    }

    fn byte(&mut self, c: u8) {
        self.prev = c;
        self.prev_ws = false;
    }
}

/// Index of the matching close bracket for the `[`/`{` at `start`, scanning at
/// most to the first newline and skipping quoted spans (`quote_opens_span`
/// decides which quotes open one) — the balanced-prefix span the block-layer
/// commit rule (and its coercion warn) reasons about. `None` when the bracket
/// doesn't close on the line.
pub fn bracket_close_index(source: &[u8], start: usize) -> Option<usize> {
    debug_assert!(source[start] == b'[' || source[start] == b'{');
    let mut w = BracketWalk::default();
    let mut i = start;
    while i < source.len() && source[i] != b'\n' {
        let c = source[i];
        match c {
            b' ' | b'\t' | b'\r' => {
                w.prev_ws = true;
                i += 1;
            }
            b'\'' | b'"' => {
                let opens = w.quote_opens();
                w.byte(c);
                if opens {
                    i = skip_quoted_span(source, i)?;
                } else {
                    i += 1;
                }
            }
            b'[' | b'{' => {
                w.open(c);
                w.byte(c);
                i += 1;
            }
            b']' | b'}' => {
                // Zig's `depth -= 1` traps on an unmatched close; the scan
                // starts on an open bracket, so depth is never 0 here.
                w.depth -= 1;
                w.byte(c);
                i += 1;
                if w.depth == 0 {
                    return Some(i - 1);
                }
            }
            _ => {
                w.byte(c);
                i += 1;
            }
        }
    }
    None
}

/// Skip a single-line quoted span starting at the opening quote `source[start]`.
/// Returns the index just past the close, or `None` if it doesn't close before
/// the newline (an unterminated quote at an element start inside the brackets
/// → treat as `Unclosed`). `"` honors `\`-escapes; `'` is raw (mirroring the
/// value grammar).
fn skip_quoted_span(source: &[u8], start: usize) -> Option<usize> {
    let q = source[start];
    let mut i = start + 1;
    while i < source.len() && source[i] != b'\n' {
        if q == b'"' && source[i] == b'\\' {
            i += 2;
            continue;
        }
        if source[i] == q {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// After a balanced close at `from`, is the rest of the line only whitespace and
/// an optional `#` comment? Honors the `#`-after-whitespace rule: a `#` glued to
/// the close (`[1,2]#x`) is literal, so the value is `BareTrailing`, not flow.
fn rest_is_comment_only(source: &[u8], from: usize) -> bool {
    let mut i = from;
    let mut saw_space = false;
    while i < source.len() && (source[i] == b' ' || source[i] == b'\t' || source[i] == b'\r') {
        saw_space = true;
        i += 1;
    }
    if i >= source.len() || source[i] == b'\n' {
        return true;
    }
    saw_space && source[i] == b'#'
}

/// Flow-element twin of `classify_bracket_commit`. Inside a flow collection an
/// element ends at the next top-level `,`/`]`/`}` (or newline / `#` / EOF), not
/// at end-of-line, and flow spans newlines — so both the terminal test and the
/// scan range differ from the block-layer classifier. A balanced bracket group
/// followed by a flow terminator is genuine nested flow (`[1, 2]` as a list
/// member); one followed by more content is a bare-string element (`[Blog](/x)`,
/// a markdown link) — the flow-position form of the balanced-then-trailing rule
/// that lets markdown links go unquoted. There is no `Unclosed` outcome: an
/// unterminated bracket is left to the flow parser (which raises
/// FigUnclosedFlow), so this reports `Flow`. Quoted spans are skipped on the
/// same element-start rule as the block-layer scanner (`quote_opens_span`).
///
/// `ws_ends` is set for a compact object's value, which is one token: there,
/// whitespace after the close also terminates (`{a=[1, 2] b=3}` is an array
/// and the next pair), and only content glued to the close makes a bare string
/// (`{href=[Blog](/x.md)}`).
pub fn classify_flow_bracket(source: &[u8], start: usize, ws_ends: bool) -> BracketCommit {
    debug_assert!(source[start] == b'[' || source[start] == b'{');
    let mut w = BracketWalk::default();
    let mut i = start;
    while i < source.len() {
        let c = source[i];
        match c {
            // flow spans newlines; neither ends an element start
            b' ' | b'\t' | b'\r' | b'\n' => {
                w.prev_ws = true;
                i += 1;
            }
            b'\'' | b'"' => {
                let opens = w.quote_opens();
                w.byte(c);
                if opens {
                    match skip_quoted_span(source, i) {
                        Some(next) => i = next,
                        None => return BracketCommit::Flow,
                    }
                } else {
                    i += 1;
                }
            }
            b'[' | b'{' => {
                w.open(c);
                w.byte(c);
                i += 1;
            }
            b']' | b'}' => {
                w.depth -= 1;
                w.byte(c);
                i += 1;
                if w.depth == 0 {
                    let terminal = if ws_ends { compact_rest_is_terminator(source, i) } else { flow_rest_is_terminator(source, i) };
                    return if terminal { BracketCommit::Flow } else { BracketCommit::BareTrailing };
                }
            }
            _ => {
                w.byte(c);
                i += 1;
            }
        }
    }
    BracketCommit::Flow
}

/// After a flow element's balanced close at `from`, does the next non-blank byte
/// terminate the element (`,`/`]`/`}`), end the line/input, or open a comment
/// (`#`)? If so the bracket group WAS the whole value → genuine nested flow;
/// otherwise more content follows and it is a bare-string element.
fn flow_rest_is_terminator(source: &[u8], from: usize) -> bool {
    let mut i = from;
    while i < source.len() && (source[i] == b' ' || source[i] == b'\t' || source[i] == b'\r') {
        i += 1;
    }
    if i >= source.len() {
        return true;
    }
    matches!(source[i], b',' | b']' | b'}' | b'\n' | b'#')
}

/// `flow_rest_is_terminator` for a compact object's value: the byte right after
/// the close must end the token — whitespace, `,`/`]`/`}`, or end of input.
fn compact_rest_is_terminator(source: &[u8], from: usize) -> bool {
    if from >= source.len() {
        return true;
    }
    is_compact_stop(source[from])
}

/// Where a compact object's unquoted value (one token) ends.
pub fn is_compact_stop(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}')
}

// ── Quoted / multiline string scanning ──────────────────────────────────────
// Each scanner starts at the OPENING delimiter and returns the decoded text
// plus the index just past the CLOSING delimiter. Decoded text is always
// owned by the caller.
//
// Errors (Zig's `ScanError`): `FigUnclosedString`, `FigBadEscape`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanResult {
    pub text: Vec<u8>,
    pub end: usize,
}

/// `'...'` — raw/literal, no escapes. A newline before the close is an error
/// (single-line only; use `'''...'''` for verbatim multi-line blobs).
pub fn scan_single_quoted(source: &[u8], start: usize) -> Result<ScanResult> {
    debug_assert!(source[start] == b'\'');
    let body = start + 1;
    if let Some(off) = source[body.min(source.len())..].iter().position(|&c| c == b'\'' || c == b'\n') {
        let j = body + off;
        if source[j] == b'\n' {
            return Err(err!(FigUnclosedString));
        }
        // No escapes to decode → the body is one contiguous slice.
        return Ok(ScanResult { text: source[body..j].to_vec(), end: j + 1 });
    }
    Err(err!(FigUnclosedString))
}

/// `"..."` — escaped: `\n \t \r \\ \" \' \uXXXX`. A newline before the close is
/// an error.
pub fn scan_double_quoted(source: &[u8], start: usize) -> Result<ScanResult> {
    debug_assert!(source[start] == b'"');
    let mut out: Vec<u8> = Vec::new();
    let mut i = start + 1;
    // Jump straight to the next quote/backslash/newline, bulk-copying the plain
    // run in between, instead of appending byte-by-byte.
    while let Some(off) = source[i.min(source.len())..].iter().position(|&c| c == b'"' || c == b'\\' || c == b'\n') {
        let j = i + off;
        match source[j] {
            b'\n' => return Err(err!(FigUnclosedString)),
            b'"' => {
                if out.is_empty() {
                    return Ok(ScanResult { text: source[start + 1..j].to_vec(), end: j + 1 });
                }
                out.extend_from_slice(&source[i..j]);
                return Ok(ScanResult { text: out, end: j + 1 });
            }
            _ => {
                // '\\'
                out.extend_from_slice(&source[i..j]); // literal run before the escape
                i = decode_escape(source, j, &mut out)?;
            }
        }
    }
    Err(err!(FigUnclosedString))
}

/// Zig's `std.fmt.parseInt` for an unsigned integer type `bits` wide, in
/// `base` (2..=36): an optional `+`/`-` sign (a negative is only `-0`), digits
/// with single `_` separators anywhere but at either end, and no overflow.
/// `None` is Zig's `InvalidCharacter` or `Overflow`.
pub(crate) fn zig_parse_unsigned(buf: &[u8], base: u32, bits: u32) -> Option<u64> {
    if buf.is_empty() {
        return None;
    }
    let (neg, digits) = match buf[0] {
        b'+' => (false, &buf[1..]),
        b'-' => (true, &buf[1..]),
        _ => (false, buf),
    };
    if digits.is_empty() {
        return None;
    }
    if digits[0] == b'_' || digits[digits.len() - 1] == b'_' {
        return None;
    }
    let max: u128 = if bits >= 64 { u64::MAX as u128 } else { (1u128 << bits) - 1 };
    let mut acc: u128 = 0;
    for &c in digits {
        if c == b'_' {
            continue;
        }
        let d = (c as char).to_digit(36)?;
        if d >= base {
            return None;
        }
        if neg {
            // A negative unsigned is `-0` alone: any nonzero digit overflows.
            if d != 0 || acc != 0 {
                return None;
            }
            continue;
        }
        acc = acc * base as u128 + d as u128;
        if acc > max {
            return None;
        }
    }
    Some(acc as u64)
}

/// Decode one `\...` escape starting at `source[i] == '\\'`, appending the
/// decoded byte(s) to `out`. Returns the index just past the escape.
fn decode_escape(source: &[u8], i: usize, out: &mut Vec<u8>) -> Result<usize> {
    if i + 1 >= source.len() {
        return Err(err!(FigBadEscape));
    }
    let e = source[i + 1];
    match e {
        b'n' => out.push(b'\n'),
        b't' => out.push(b'\t'),
        b'r' => out.push(b'\r'),
        b'\\' => out.push(b'\\'),
        b'"' => out.push(b'"'),
        b'\'' => out.push(b'\''),
        b'u' => {
            if i + 6 > source.len() {
                return Err(err!(FigBadEscape));
            }
            let hex = &source[i + 2..i + 6];
            let cp = zig_parse_unsigned(hex, 16, 21).ok_or(err!(FigBadEscape))? as u32;
            fig_kit::unicode::encode_append(out, cp).map_err(|_| err!(FigBadEscape))?;
            return Ok(i + 6);
        }
        _ => return Err(err!(FigBadEscape)),
    }
    Ok(i + 2)
}

/// `'''` ... `'''` — raw/verbatim multi-line. The opener line may carry a
/// trailing `# comment` (returned separately, per DESIGN.md); content begins
/// at the next line and is copied byte-for-byte (no dedent, no escapes) up to
/// (not including) the final line's trailing newline.
pub fn scan_triple_single(source: &[u8], start: usize) -> Result<MultilineResult<'_>> {
    scan_triple(source, start, b'\'', false)
}

/// `"""` ... `"""` — escaped, with the common leading indentation (measured
/// from the closing delimiter's own line) stripped from every content line.
pub fn scan_triple_double(source: &[u8], start: usize) -> Result<MultilineResult<'_>> {
    scan_triple(source, start, b'"', true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultilineResult<'a> {
    pub text: Vec<u8>,
    pub end: usize,
    /// A `# comment` on the opening delimiter's own line, if any.
    pub opener_comment: Option<&'a [u8]>,
}

/// `std.mem.trim(u8, s, chars)`.
pub(crate) fn trim<'a>(s: &'a [u8], chars: &[u8]) -> &'a [u8] {
    let mut a = 0;
    let mut b = s.len();
    while a < b && chars.contains(&s[a]) {
        a += 1;
    }
    while b > a && chars.contains(&s[b - 1]) {
        b -= 1;
    }
    &s[a..b]
}

fn scan_triple(source: &[u8], start: usize, q: u8, dedent_and_escape: bool) -> Result<MultilineResult<'_>> {
    debug_assert!(source[start] == q && source[start + 1] == q && source[start + 2] == q);
    let mut i = start + 3;
    // Rest of the opener line: only whitespace and an optional `# comment` are
    // meaningful; content capture begins at the next newline.
    let mut opener_comment: Option<&[u8]> = None;
    while i < source.len() && source[i] != b'\n' {
        if source[i] == b'#' {
            let cstart = i + 1;
            let mut j = cstart;
            while j < source.len() && source[j] != b'\n' {
                j += 1;
            }
            opener_comment = Some(trim(&source[cstart..j], b" \t\r"));
            i = j;
            break;
        }
        i += 1;
    }
    if i < source.len() && source[i] == b'\n' {
        i += 1;
    }
    let body_start = i;

    // The closing delimiter is LINE-ANCHORED (spec § 4.6): the block closes at
    // the first line whose first non-whitespace content begins with the triple
    // delimiter. The closer's leading whitespace is not content (for `"""` it
    // defines the dedent), and a delimiter appearing mid-line is ordinary
    // content — never a closer, so no text is ever silently dropped.
    let mut line_start = body_start;
    let close_pos = loop {
        if line_start > source.len() {
            return Err(err!(FigUnclosedString));
        }
        let line_end = source[line_start..].iter().position(|&c| c == b'\n').map_or(source.len(), |n| line_start + n);
        let mut j = line_start;
        while j < line_end && (source[j] == b' ' || source[j] == b'\t') {
            j += 1;
        }
        if j + 3 <= source.len() && source[j] == q && source[j + 1] == q && source[j + 2] == q {
            break j;
        }
        if line_end >= source.len() {
            return Err(err!(FigUnclosedString));
        }
        line_start = line_end + 1;
    };
    let dedent = close_pos - line_start;

    // Body text excludes the closing line and its preceding newline (a CRLF
    // terminator counts as one line break — the `\r` is trivia, spec § 3.1).
    let mut body_end = line_start;
    if body_end > body_start && source[body_end - 1] == b'\n' {
        body_end -= 1;
    }
    if body_end > body_start && source[body_end - 1] == b'\r' {
        body_end -= 1;
    }
    let body: &[u8] = if body_end >= body_start { &source[body_start..body_end] } else { b"" };

    let end = close_pos + 3;

    let mut out: Vec<u8> = Vec::new();
    let mut first = true;
    for raw_line in body.split(|&c| c == b'\n') {
        if !first {
            out.push(b'\n');
        }
        first = false;
        let mut line = raw_line;
        // A line-terminating `\r` (CRLF input) is trivia, so the captured
        // value's line breaks are always LF. A `\r` mid-line stays content.
        if !line.is_empty() && line[line.len() - 1] == b'\r' {
            line = &line[..line.len() - 1];
        }
        if dedent_and_escape {
            let mut stripped = 0usize;
            while stripped < dedent && !line.is_empty() && (line[0] == b' ' || line[0] == b'\t') {
                line = &line[1..];
                stripped += 1;
            }
            let mut k = 0usize;
            while k < line.len() {
                if line[k] == b'\\' {
                    k = decode_escape(line, k, &mut out)?;
                } else {
                    out.push(line[k]);
                    k += 1;
                }
            }
        } else {
            out.extend_from_slice(line);
        }
    }

    Ok(MultilineResult { text: out, end, opener_comment })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fig_kit::datetime;
    use fig_kit::literal::NumberKind;

    #[test]
    fn sniff_number_rejects_leading_zero_accepts_hex_float_exponent() {
        assert!(sniff_number(b"007").is_none());
        assert!(sniff_number(b"0").is_some());
        assert!(sniff_number(b"0.5").is_some());
        assert_eq!(b"0xFF", sniff_number(b"0xFF").unwrap().raw);
        assert!(sniff_number(b"1.5e3").unwrap().kind == NumberKind::Float);
        assert!(sniff_number(b"12 monkeys").is_none());
    }

    #[test]
    fn sniff_number_accepts_separators_in_radix_integers() {
        // Kept verbatim in `raw` — the lexeme is the value (§ 2).
        assert_eq!(b"0xdead_beef", sniff_number(b"0xdead_beef").unwrap().raw);
        assert!(sniff_number(b"0xdead_beef").unwrap().kind == NumberKind::Integer);
        assert_eq!(b"0b1010_1010", sniff_number(b"0b1010_1010").unwrap().raw);
        assert_eq!(b"0o7_5_5", sniff_number(b"0o7_5_5").unwrap().raw);
        assert_eq!(b"-0xf_f", sniff_number(b"-0xf_f").unwrap().raw);

        // A separator must be anchored between two digits.
        assert!(sniff_number(b"0x_ff").is_none()); // leading
        assert!(sniff_number(b"0xff_").is_none()); // trailing
        assert!(sniff_number(b"0xf__f").is_none()); // doubled
        assert!(sniff_number(b"0x_").is_none());
        assert!(sniff_number(b"0b1_2").is_none()); // still radix-checked
        assert!(sniff_number(b"0xff_.5").is_none()); // run ends on `_`
    }

    #[test]
    fn sniff_number_accepts_separators_in_decimal_integers_fractions_exponents() {
        assert_eq!(b"1_000", sniff_number(b"1_000").unwrap().raw);
        assert!(sniff_number(b"1_000").unwrap().kind == NumberKind::Integer);
        assert!(sniff_number(b"1_0.5").unwrap().kind == NumberKind::Float);
        assert!(sniff_number(b"1.000_1").unwrap().kind == NumberKind::Float); // fraction
        assert!(sniff_number(b"1e1_0").unwrap().kind == NumberKind::Float); // exponent
        assert!(sniff_number(b"-1_0.0_1e-1_0").unwrap().kind == NumberKind::Float); // all runs at once
        assert_eq!(b"+9_9", sniff_number(b"+9_9").unwrap().raw);

        // Anchored in every run, independently.
        for bad in [&b"_1"[..], b"1_", b"1__0", b"1_.5", b"1._5", b"1.5_", b"1e_5", b"1e5_", b"1e+_5"] {
            assert!(sniff_number(bad).is_none(), "{:?}", std::str::from_utf8(bad));
        }

        // The leading-zero rule still wins, and separators count toward the run.
        assert!(sniff_number(b"0_1").is_none());
        assert!(sniff_number(b"007_5").is_none());
        assert!(sniff_number(b"0").is_some());
        assert!(sniff_number(b"0.000_1").is_some()); // a lone `0` may lead a float
    }

    #[test]
    fn sniff_bare_classifies_literals() {
        assert!(sniff_bare(b"null") == Sniffed::Null);
        assert!(sniff_bare(b"true") == Sniffed::Boolean(true));
        assert!(sniff_bare(b"Yes") == Sniffed::String);
        assert!(sniff_bare(b"007") == Sniffed::String);
        assert!(matches!(sniff_bare(b"2026-07-01"), Sniffed::Datetime { kind: datetime::Kind::LocalDate, .. }));
    }

    #[test]
    fn scan_double_quoted_decodes_escapes() {
        let r = scan_double_quoted(b"\"a\\tb\\n\\u2603\"", 0).unwrap();
        assert_eq!("a\tb\n☃".as_bytes(), &r.text[..]);
    }

    #[test]
    fn scan_triple_double_dedents_and_decodes() {
        let src = b"\"\"\"\n    hi\n    there\n    \"\"\"";
        let r = scan_triple_double(src, 0).unwrap();
        assert_eq!(b"hi\nthere", &r.text[..]);
    }

    #[test]
    fn classify_bracket_commit_flow_vs_bare_trailing_vs_unclosed() {
        use BracketCommit::*;
        // terminal close → flow
        assert_eq!(Flow, classify_bracket_commit(b"[80, 443]\n", 0));
        assert_eq!(Flow, classify_bracket_commit(b"[80, 443]  # c\n", 0));
        assert_eq!(Flow, classify_bracket_commit(b"{}", 0));
        assert_eq!(Flow, classify_bracket_commit(b"[['a, b']]", 0)); // brackets inside quotes don't count
        // balanced then trailing content → bare string
        assert_eq!(BareTrailing, classify_bracket_commit(b"[Blog](/x)\n", 0));
        assert_eq!(BareTrailing, classify_bracket_commit(b"[a-z]*.md", 0));
        assert_eq!(BareTrailing, classify_bracket_commit(b"[b]x[/b]", 0));
        assert_eq!(BareTrailing, classify_bracket_commit(b"[80]#glued", 0)); // glued # is literal
        // never closes on the line → unclosed (multi-line flow or truncation)
        assert_eq!(Unclosed, classify_bracket_commit(b"[80, 443", 0));
        assert_eq!(Unclosed, classify_bracket_commit(b"[\n", 0));
        assert_eq!(Unclosed, classify_bracket_commit(b"['unterminated]", 0));
    }

    #[test]
    fn classify_bracket_commit_a_mid_word_quote_is_content_not_a_span_opener() {
        use BracketCommit::*;
        // Only an element-start quote opens a span, so an apostrophe inside a bare
        // element leaves the bracket balanced and the RHS a bare string.
        assert_eq!(BareTrailing, classify_bracket_commit(b"[its parent's comment](x.md)\n", 0));
        assert_eq!(BareTrailing, classify_bracket_commit(b"[its parent\"s comment](x.md)\n", 0));
        assert_eq!(BareTrailing, classify_bracket_commit(b"[text](it's.md)\n", 0));
        assert_eq!(Flow, classify_bracket_commit(b"[its parent's comment]\n", 0));
        // Element starts: after `[`/`{`, a `,`, and a `:`/`=` pair separator.
        assert_eq!(Flow, classify_bracket_commit(b"[\"a]b\", c]\n", 0));
        assert_eq!(Flow, classify_bracket_commit(b"[a, 'b]c']\n", 0));
        assert_eq!(Flow, classify_bracket_commit(b"{k = 'a]b'}\n", 0));
        assert_eq!(Flow, classify_bracket_commit(b"{\"a]b\": 1}\n", 0));
    }

    #[test]
    fn classify_flow_bracket_mid_word_quote_in_an_element() {
        use BracketCommit::*;
        assert_eq!(BareTrailing, classify_flow_bracket(b"[Blog](it's/x.md), b]", 0, false));
        assert_eq!(Flow, classify_flow_bracket(b"['a]b'], c]", 0, false));
    }

    #[test]
    fn classify_flow_bracket_nested_flow_vs_bare_trailing_element() {
        use BracketCommit::*;
        // A balanced group followed by a flow terminator is a nested collection.
        assert_eq!(Flow, classify_flow_bracket(b"[1, 2], 3]", 0, false));
        assert_eq!(Flow, classify_flow_bracket(b"[1, 2]]", 0, false));
        assert_eq!(Flow, classify_flow_bracket(b"{ x = 1 }, y]", 0, false));
        assert_eq!(Flow, classify_flow_bracket(b"[a, b]\n", 0, false));
        // A balanced group with trailing content is a bare-string element.
        assert_eq!(BareTrailing, classify_flow_bracket(b"[Blog](/x), [Resume](/y)]", 0, false));
        assert_eq!(BareTrailing, classify_flow_bracket(b"[a-z]*.md, next]", 0, false));
        // Unterminated → left to the flow parser (reported as flow).
        assert_eq!(Flow, classify_flow_bracket(b"[1, 2", 0, false));
    }

    #[test]
    fn classify_flow_bracket_a_compact_objects_value_ends_at_whitespace() {
        use BracketCommit::*;
        // Whitespace after the close ends the token: the group is the value and the
        // rest is the next pair.
        assert_eq!(Flow, classify_flow_bracket(b"[1, 2] b=3}", 0, true));
        assert_eq!(Flow, classify_flow_bracket(b"{x=1}\n}", 0, true));
        assert_eq!(Flow, classify_flow_bracket(b"[1]}", 0, true));
        assert_eq!(Flow, classify_flow_bracket(b"[1]", 0, true));
        // Only glued content makes a bare string.
        assert_eq!(BareTrailing, classify_flow_bracket(b"[Blog](/x.md) b=3}", 0, true));
        assert_eq!(BareTrailing, classify_flow_bracket(b"[1]#x}", 0, true));
    }

    #[test]
    fn a_quote_after_whitespace_inside_a_brace_opens_a_span() {
        // A compact object's quoted key follows a space, so its brackets don't count.
        assert_eq!(Some(11), bracket_close_index(b"{a=1 \"k}\"=2}", 0));
        assert_eq!(BracketCommit::Flow, classify_bracket_commit(b"{a=1 'k]'=2}\n", 0));
        // In a `[`, the same quote is bare-value text, as before.
        assert_eq!(BracketCommit::BareTrailing, classify_bracket_commit(b"[its 'odd] one\n", 0));
    }

    #[test]
    fn scan_triple_single_is_verbatim() {
        let src = b"'''\nraw \\n text\n'''";
        let r = scan_triple_single(src, 0).unwrap();
        assert_eq!(b"raw \\n text", &r.text[..]);
    }

    #[test]
    fn zig_parse_unsigned_follows_std_fmt_parse_int() {
        assert_eq!(Some(0x2603), zig_parse_unsigned(b"2603", 16, 21));
        assert_eq!(Some(0xfff), zig_parse_unsigned(b"+fff", 16, 21));
        assert_eq!(Some(0x123), zig_parse_unsigned(b"1_23", 16, 21));
        assert_eq!(Some(0), zig_parse_unsigned(b"-0", 16, 21));
        assert_eq!(None, zig_parse_unsigned(b"-1", 16, 21));
        assert_eq!(None, zig_parse_unsigned(b"_12", 16, 21));
        assert_eq!(None, zig_parse_unsigned(b"12_", 16, 21));
        assert_eq!(None, zig_parse_unsigned(b"+", 10, 21));
        assert_eq!(None, zig_parse_unsigned(b"2097152", 10, 21));
        assert_eq!(Some(2097151), zig_parse_unsigned(b"2097151", 10, 21));
    }
}
