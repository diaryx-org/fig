//! Port of kit/src/literal.zig: the bare literal — what a value's unquoted
//! text means when nothing else says: `null`, `true`/`false`, a number, a
//! datetime, or else a string. One rule for the whole contract: the editor
//! classifies a caller's value text by it (`lang.Literal`, which a value
//! renderer is told), and figl reads its bare tokens by it.

use super::{ascii, datetime};

/// The result of sniffing a bare (unquoted, undelimited) token. Zig's `null_`
/// is `Null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sniffed<'a> {
    Null,
    Boolean(bool),
    Number(NumberResult<'a>),
    Datetime { kind: datetime::Kind, raw: &'a [u8] },
    /// Nothing typed matched; the caller keeps the original text verbatim as a
    /// plain string (the literal-else-string fallback).
    String,
}

/// Literal-else-string: classify a trimmed bare token. Never fails — a token
/// that matches nothing typed sniffs as `String`. Enum atoms and `inf`/`nan`
/// are NOT sniffed here — they are explicit-typing-only (figl's DESIGN.md "Enum:
/// explicit-only"), so a bare `@x`/`inf` correctly falls through to `String`.
/// `true`/`false`/`null` are matched case-sensitively (lowercase only) — `Yes`,
/// `TRUE`, `on` stay strings (the Norway-problem fix).
pub fn sniff_bare(token: &[u8]) -> Sniffed<'_> {
    if token.is_empty() {
        return Sniffed::String;
    }
    if token == b"null" {
        return Sniffed::Null;
    }
    if token == b"true" {
        return Sniffed::Boolean(true);
    }
    if token == b"false" {
        return Sniffed::Boolean(false);
    }
    if let Some(n) = sniff_number(token) {
        return Sniffed::Number(n);
    }
    if let Ok(k) = datetime::classify(token, datetime::Options::default()) {
        return Sniffed::Datetime { kind: k, raw: token };
    }
    Sniffed::String
}

/// Zig's anonymous `enum { integer, float }` in `NumberResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumberKind {
    Integer,
    Float,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumberResult<'a> {
    pub raw: &'a [u8],
    pub kind: NumberKind,
}

/// Scan a digit run at `start`: `pred` digits with optional `_` separators.
/// Returns the index just past the run, or `None` if the run is empty or a
/// separator is unanchored. The TOML parser's `validUnderscored` is the same
/// rule, but *validates a known slice*; fig has to scan to a boundary because
/// the `.`/`e` that ends a run is only found by walking the digits.
fn scan_underscored(s: &[u8], start: usize, pred: fn(u8) -> bool) -> Option<usize> {
    let mut j = start;
    let mut prev_us = true; // a separator may not open the run...
    while j < s.len() {
        let c = s[j];
        if c == b'_' {
            if prev_us {
                return None; // ...nor be doubled
            }
            prev_us = true;
        } else if pred(c) {
            prev_us = false;
        } else {
            break;
        }
        j += 1;
    }
    if prev_us {
        return None; // ...nor close it; an empty run trips this too
    }
    Some(j)
}

/// TOML-style bare-number sniff: decimal integers/floats (with optional
/// exponent) and `0x`/`0o`/`0b` integers. A leading zero on a multi-digit
/// decimal integer part is NOT a number — it stays a string, keeping
/// zero-padded IDs/zip codes intact (figl's DESIGN.md "Leading-zero rule").
///
/// `_` digit separators are accepted in every digit run — radix, integer part,
/// fraction, and exponent (`0xdead_beef`, `1_000`, `1_000.000_1e1_0`) — each
/// anchored between two digits, matching TOML and ZON. They are part of the
/// lexeme (§ 2), so `0xff` and `0xf_f` are distinct numbers.
pub fn sniff_number(token: &[u8]) -> Option<NumberResult<'_>> {
    if token.is_empty() {
        return None;
    }
    let mut i = 0;
    if token[0] == b'+' || token[0] == b'-' {
        i = 1;
    }
    if i >= token.len() {
        return None;
    }
    let body = &token[i..];

    if body.len() >= 2 && body[0] == b'0' && (body[1] == b'x' || body[1] == b'o' || body[1] == b'b') {
        // Pick the radix classifier once — `body[1]` is loop-invariant.
        let is_valid_digit: fn(u8) -> bool = match body[1] {
            b'x' => ascii::is_hex,
            b'o' => ascii::is_octal,
            b'b' => ascii::is_binary,
            _ => unreachable!(),
        };
        let end = scan_underscored(body, 2, is_valid_digit)?;
        if end != body.len() {
            return None; // trailing garbage -> not a number
        }
        return Some(NumberResult { raw: token, kind: NumberKind::Integer });
    }

    let mut j = scan_underscored(token, i, ascii::is_digit)?;
    // Separators count toward the length, so `0_1` is a leading zero too.
    if j - i > 1 && token[i] == b'0' {
        return None; // leading zero
    }
    let mut is_float = false;
    if j < token.len() && token[j] == b'.' {
        is_float = true;
        // A dot needs >=1 fractional digit; the fraction may lead with a zero.
        j = scan_underscored(token, j + 1, ascii::is_digit)?;
    }
    if j < token.len() && (token[j] == b'e' || token[j] == b'E') {
        is_float = true;
        j += 1;
        if j < token.len() && (token[j] == b'+' || token[j] == b'-') {
            j += 1;
        }
        j = scan_underscored(token, j, ascii::is_digit)?;
    }
    if j != token.len() {
        return None; // trailing garbage -> not a clean number
    }
    Some(NumberResult { raw: token, kind: if is_float { NumberKind::Float } else { NumberKind::Integer } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number_kind(s: &[u8]) -> Option<NumberKind> {
        match sniff_bare(s) {
            Sniffed::Number(n) => Some(n.kind),
            _ => None,
        }
    }

    #[test]
    fn sniff_bare_types_null_bools_numbers_and_datetimes_and_keeps_everything_else_a_string() {
        assert_eq!(sniff_bare(b"null"), Sniffed::Null);
        assert_eq!(sniff_bare(b"true"), Sniffed::Boolean(true));
        assert_eq!(sniff_bare(b"false"), Sniffed::Boolean(false));
        assert_eq!(number_kind(b"42"), Some(NumberKind::Integer));
        assert_eq!(number_kind(b"0x1F"), Some(NumberKind::Integer));
        assert_eq!(number_kind(b"2.5e3"), Some(NumberKind::Float));
        assert_eq!(
            sniff_bare(b"2026-07-01"),
            Sniffed::Datetime { kind: datetime::Kind::LocalDate, raw: b"2026-07-01" }
        );
        for s in [&b""[..], b"Yes", b"TRUE", b"007", b"1_", b"inf", b"@x"] {
            assert_eq!(sniff_bare(s), Sniffed::String, "{:?}", String::from_utf8_lossy(s));
        }
    }

    #[test]
    fn sniff_number_separators_signs_and_radixes() {
        let int = Some(NumberKind::Integer);
        let float = Some(NumberKind::Float);
        let k = |s: &[u8]| sniff_number(s).map(|n| n.kind);
        assert_eq!(k(b"0xdead_beef"), int);
        assert_eq!(k(b"1_000"), int);
        assert_eq!(k(b"1_000.000_1e1_0"), float);
        assert_eq!(k(b"-0"), int);
        assert_eq!(k(b"+1.5"), float);
        assert_eq!(k(b"1e+5"), float);
        assert_eq!(k(b"0.5"), float);
        assert_eq!(k(b"-0x1"), int);
        for s in [
            &b"+"[..], b"-", b"0_1", b"01", b"1__0", b"_1", b"1.", b".5", b"1e", b"1e+", b"0x", b"0x_1", b"0xg",
            b"0X1", b"0b2", b"0o8", b"1.5.5", b"1x",
        ] {
            assert_eq!(k(s), None, "{:?}", String::from_utf8_lossy(s));
        }
        assert_eq!(sniff_number(b"-1.5").unwrap().raw, b"-1.5");
    }
}
