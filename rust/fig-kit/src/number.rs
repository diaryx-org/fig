//! Port of kit/src/number.zig: number-lexeme rendering shared by the printers.
//!
//! A number's `raw` is the source lexeme — the payload IS the value (`ast`) —
//! so a printer's instinct is to write it back verbatim. That is only correct
//! when the target format reads the lexeme back as the *same number*. It often
//! doesn't: YAML 1.2 resolves `0b1010` to a **string**, and JSON5 rejects
//! `0o755` outright. Writing raw there emits a silent type change, or output
//! the target cannot parse at all.
//!
//! So a printer asks `spellable(raw, <target>)` first, and falls back to
//! `write_canonical` — decimal, the one spelling every format shares. This is a
//! *spelling* degrade: the value is unchanged, only its notation.
//!
//! The non-finite floats are the exception: every format that has them spells
//! them its own way (`inf` in TOML and ZON, `.inf` in YAML, `Infinity` in
//! JSON5), a `raw` carries whichever spelling its source used, and decimal has
//! none. So `non_finite` reads any of them, and `write` respells one in the
//! target's own words — or refuses with `error.NonFiniteNumber` when the target
//! has no words for it (strict JSON).
//!
//! Nothing here formats a float: every spelling is the source's own digits,
//! moved, dropped or padded byte by byte, or a radix integer's exact decimal.

use crate::{Result, err};

/// The number spellings a target format reads back as a number. A `false` field
/// means the printer must canonicalize that spelling to decimal instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Spelling {
    /// `0x1F`
    pub hex: bool,
    /// `0o17`
    pub octal: bool,
    /// `0b1010`
    pub binary: bool,
    /// `1_000` — digit separators
    pub underscores: bool,
    /// `0755` — a multi-digit integer part led by a zero
    pub leading_zero: bool,
    /// `.5` / `5.`
    pub bare_dot: bool,
    /// a leading `+`
    pub plus: bool,
    /// How the format spells infinity and NaN, or `None` when it cannot.
    pub non_finite: Option<NonFiniteNames>,
}

impl Spelling {
    /// Every field false (Zig's `.{}`), usable in a `const`.
    pub const NONE: Spelling = Spelling {
        hex: false,
        octal: false,
        binary: false,
        underscores: false,
        leading_zero: false,
        bare_dot: false,
        plus: false,
        non_finite: None,
    };
}

/// A `Spelling` as `FIG_SPELL_*` bits: hex, octal, binary, underscores, a
/// leading zero, a bare dot, a leading plus, from the lowest. No bit spells
/// the non-finite names. From kit/src/rules.zig.
pub fn spelling(bits: u32) -> Spelling {
    Spelling {
        hex: bits & 1 != 0,
        octal: bits & 2 != 0,
        binary: bits & 4 != 0,
        underscores: bits & 8 != 0,
        leading_zero: bits & 16 != 0,
        bare_dot: bits & 32 != 0,
        plus: bits & 64 != 0,
        non_finite: None,
    }
}

/// How much longer than its input `write_canonical`'s answer can be: a
/// canonical number is its input less what it drops, plus a padded `0`
/// before a bare dot and after one, or a radix integer's decimal, which
/// this module bounds at 512 digits. `FIG_KIT_ANSWER_ROOM`, from
/// kit/src/rules.zig.
pub const ANSWER_ROOM: usize = 520;

/// A non-finite float, whatever its source spelling. The discriminants are
/// what `fig_kit_number_non_finite` answers (kit/src/rules.zig).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum NonFinite {
    Inf = 0,
    NegInf = 1,
    Nan = 2,
}

/// A format's own spelling of each `NonFinite`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonFiniteNames {
    pub inf: &'static [u8],
    pub neg_inf: &'static [u8],
    pub nan: &'static [u8],
}

impl NonFiniteNames {
    pub fn of(self, v: NonFinite) -> &'static [u8] {
        match v {
            NonFinite::Inf => self.inf,
            NonFinite::NegInf => self.neg_inf,
            NonFinite::Nan => self.nan,
        }
    }
}

/// TOML 1.0 and ZON: `inf`, `-inf`, `nan`.
pub const PLAIN_NON_FINITE: NonFiniteNames = NonFiniteNames { inf: b"inf", neg_inf: b"-inf", nan: b"nan" };

/// Strict JSON: none of it. Every non-decimal spelling canonicalizes.
pub const JSON: Spelling = Spelling::NONE;

/// JSON5 numbers are ES5.1 `NumericLiteral`: hex, leading `+`, and bare dots —
/// but NOT `0o`/`0b` (ES6), `_` (ES2021), or a leading zero. Verified against
/// this repo's own JSON5 tokenizer, which rejects all four.
pub const JSON5: Spelling = Spelling {
    hex: true,
    bare_dot: true,
    plus: true,
    non_finite: Some(NonFiniteNames { inf: b"Infinity", neg_inf: b"-Infinity", nan: b"NaN" }),
    ..Spelling::NONE
};

/// YAML 1.2 core: int is `[-+]?[0-9]+ | 0o[0-7]+ | 0x[0-9a-fA-F]+`, float takes
/// a bare dot. No `0b` and no `_` — 1.2 resolves both to a *string*.
///
/// 1.1's extra spellings (`0b`, `_`, base-60) are deliberately NOT claimed: the
/// YAML printer has no version parameter, so it targets the default type
/// (`yaml.zig`: `default_type = .v1_2_2`). `leading_zero` IS claimed because
/// 1.2 reads `0755` as decimal 755 — note 1.1 would read it as octal 493, an
/// ambiguity the AST cannot represent (the parser stores the lexeme and leaves
/// resolution to the consumer), so it is left alone rather than guessed at.
pub const YAML_1_2: Spelling = Spelling {
    hex: true,
    octal: true,
    leading_zero: true,
    bare_dot: true,
    plus: true,
    non_finite: Some(NonFiniteNames { inf: b".inf", neg_inf: b"-.inf", nan: b".nan" }),
    ..Spelling::NONE
};

/// Which non-finite float `raw` spells, if it spells one, in any of the ways a
/// parser or a caller stores it: `inf`/`nan` (TOML, ZON), `.inf`/`.nan` in
/// YAML's three cases, and `Infinity`/`NaN` (JSON5) — each with an optional
/// sign, which a NaN ignores.
pub fn non_finite(raw: &[u8]) -> Option<NonFinite> {
    let mut body = raw;
    let mut negative = false;
    if !body.is_empty() && (body[0] == b'+' || body[0] == b'-') {
        negative = body[0] == b'-';
        body = &body[1..];
    }
    if !body.is_empty() && body[0] == b'.' {
        body = &body[1..];
    }
    if body.eq_ignore_ascii_case(b"inf") || body.eq_ignore_ascii_case(b"infinity") {
        return Some(if negative { NonFinite::NegInf } else { NonFinite::Inf });
    }
    if body.eq_ignore_ascii_case(b"nan") {
        return Some(NonFinite::Nan);
    }
    None
}

/// Whether a format that can spell `s` reads `raw` back as the same number.
pub fn spellable(raw: &[u8], s: Spelling) -> bool {
    let mut body = raw;
    if !body.is_empty() && (body[0] == b'+' || body[0] == b'-') {
        if body[0] == b'+' && !s.plus {
            return false;
        }
        body = &body[1..];
    }
    if body.is_empty() {
        return false;
    }
    if !s.underscores && body.contains(&b'_') {
        return false;
    }

    if body.len() >= 2 && body[0] == b'0' {
        match body[1] | 0x20 {
            b'x' => return s.hex,
            b'o' => return s.octal,
            b'b' => return s.binary,
            _ => {}
        }
    }

    // Decimal from here.
    if !s.bare_dot && (body[0] == b'.' || body[body.len() - 1] == b'.') {
        return false;
    }
    if !s.leading_zero {
        let int_end = index_of_any(body, b".eE").unwrap_or(body.len());
        if int_end > 1 && body[0] == b'0' {
            return false;
        }
    }
    true
}

/// Write `raw` verbatim when a format spelling `s` reads it back unchanged,
/// else canonicalized to decimal. A non-finite `raw` is respelled in `s`'s own
/// words, and is `error.NonFiniteNumber` when `s` has none.
///
/// Errors: `NonFiniteNumber`.
pub fn write(writer: &mut Vec<u8>, raw: &[u8], s: Spelling) -> Result<()> {
    if let Some(v) = non_finite(raw) {
        return match s.non_finite {
            Some(names) => {
                writer.extend_from_slice(names.of(v));
                Ok(())
            }
            None => Err(err!(NonFiniteNumber)),
        };
    }
    if spellable(raw, s) {
        writer.extend_from_slice(raw);
        return Ok(());
    }
    write_canonical(writer, raw);
    Ok(())
}

/// Write `raw` as a decimal lexeme every format can read: radix converted, `_`
/// and `+` dropped, bare dots padded (`.5` -> `0.5`, `5.` -> `5.0`), leading
/// zeros stripped. Plain decimal digits are copied rather than re-formatted, so
/// arbitrary precision and significant figures survive (`1.10` stays `1.10`).
pub fn write_canonical(writer: &mut Vec<u8>, raw: &[u8]) {
    let mut s = raw;
    if !s.is_empty() && s[0] == b'-' {
        writer.push(b'-');
        s = &s[1..];
    } else if !s.is_empty() && s[0] == b'+' {
        s = &s[1..];
    }

    // Radix integers convert; a lexeme that is not one, or whose decimal
    // would outgrow `buf`, falls back to the sign-stripped source rather
    // than emitting nothing.
    if let Some(base) = radix_prefix(s) {
        let mut buf = [0u8; 512];
        match radix_to_decimal(&s[2..], base, &mut buf) {
            Some(digits) => writer.extend_from_slice(digits),
            None => writer.extend_from_slice(s),
        }
        return;
    }

    let e_idx = index_of_any(s, b"eE");
    let mantissa = match e_idx {
        Some(i) => &s[..i],
        None => s,
    };
    let exponent: &[u8] = match e_idx {
        Some(i) => &s[i..],
        None => b"",
    };
    let dot = mantissa.iter().position(|&c| c == b'.');

    let mut int_digits = 0usize;
    let mut seen_nonzero = false;
    let int_part = match dot {
        Some(d) => &mantissa[..d],
        None => mantissa,
    };
    for &c in int_part {
        if c == b'_' {
            continue;
        }
        if c == b'0' && !seen_nonzero {
            continue; // leading zero
        }
        seen_nonzero = true;
        writer.push(c);
        int_digits += 1;
    }
    if int_digits == 0 {
        writer.push(b'0'); // `0`, `000`, `.5`
    }

    if let Some(d) = dot {
        writer.push(b'.');
        let mut frac_digits = 0usize;
        for &c in &mantissa[d + 1..] {
            if c == b'_' {
                continue;
            }
            writer.push(c);
            frac_digits += 1;
        }
        if frac_digits == 0 {
            writer.push(b'0'); // `5.` -> `5.0`
        }
    }

    for &c in exponent {
        if c != b'_' {
            writer.push(c);
        }
    }
}

/// What a multi-digit integer led by a zero (`0755`) means where it was
/// read: decimal 755 to YAML 1.2 (and to `write_canonical`), octal 493 to YAML
/// 1.1. `Unknown` when the reader's dialect is not to hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LeadingZero {
    Decimal,
    Octal,
    #[default]
    Unknown,
}

/// A number lexeme as `eql_value` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lexeme<'a> {
    pub raw: &'a [u8],
    pub float: bool,
    /// Zig's default is `.unknown`.
    pub leading_zero: LeadingZero,
}

impl<'a> Lexeme<'a> {
    /// A lexeme whose `leading_zero` is `Unknown`, Zig's default.
    pub fn new(raw: &'a [u8], float: bool) -> Lexeme<'a> {
        Lexeme { raw, float, leading_zero: LeadingZero::Unknown }
    }
}

/// Whether two number lexemes spell the same value, whatever their source
/// formats: what `fig patch` asks before it rewrites a number, so that a
/// value the patch and the target agree on keeps the target's spelling.
///
/// An integer is its exact mathematical value, read across bases (`0x1F`,
/// `0o37`, `0b11111`), signs, `_` separators and digits past 64 bits. A float
/// is its exact decimal value, so `1.50` is `1.5` and `1e3` is `1000.0`, but two
/// lexemes that round to one double while differing in the digits written are
/// different. A non-finite float equals one of the same kind in any format's
/// spelling, a NaN included: the question is whether a rewrite would change
/// the document, not IEEE's. `-0.0` is not `0.0`; an integer has no negative
/// zero, so `-0` is `0`.
///
/// An integer never equals a float, `1` and `1.0` included: YAML and TOML type
/// them apart, so a patch that turns one into the other changes the document.
///
/// A lexeme this cannot read for certain is compared by its bytes alone,
/// which can only ever report a real change as one: a leading-zero integer
/// whose `LeadingZero` is `Unknown`, a YAML 1.1 sexagesimal (`1:30`), and
/// anything else outside these spellings or too long for its buffers.
pub fn eql_value(a: Lexeme<'_>, b: Lexeme<'_>) -> bool {
    if a.float != b.float {
        return false;
    }
    if a.raw == b.raw {
        return true;
    }
    let x_nf = non_finite(a.raw);
    let y_nf = non_finite(b.raw);
    if x_nf.is_some() || y_nf.is_some() {
        return x_nf == y_nf;
    }

    let mut a_buf = [0u8; 512];
    let mut b_buf = [0u8; 512];
    let Some(x) = Exact::read(a, &mut a_buf) else { return false };
    let Some(y) = Exact::read(b, &mut b_buf) else { return false };
    if x.digits.is_empty() && y.digits.is_empty() {
        return !a.float || x.negative == y.negative;
    }
    x.negative == y.negative && x.exponent == y.exponent && x.digits == y.digits
}

/// A finite number's exact value: `digits` × 10^`exponent`, with neither a
/// leading nor a trailing zero in `digits`, so equal values read equal. Zero
/// is empty `digits`.
struct Exact<'b> {
    negative: bool,
    digits: &'b [u8],
    exponent: i64,
}

impl<'b> Exact<'b> {
    /// `raw`'s value, its digits written into `buf`; `None` for a lexeme
    /// `eql_value` compares by bytes instead.
    fn read(lexeme: Lexeme<'_>, buf: &'b mut [u8]) -> Option<Exact<'b>> {
        let (negative, digits, exponent) = Exact::read_into(lexeme, buf)?;
        let buf: &'b [u8] = buf;
        Some(Exact::normalize(negative, &buf[digits], exponent))
    }

    /// `read`'s work: the sign, where in `buf` the digits are, and the
    /// exponent, before `normalize`.
    fn read_into(lexeme: Lexeme<'_>, buf: &mut [u8]) -> Option<(bool, core::ops::Range<usize>, i64)> {
        let float = lexeme.float;
        let mut s = lexeme.raw;
        let mut negative = false;
        if !s.is_empty() && (s[0] == b'+' || s[0] == b'-') {
            negative = s[0] == b'-';
            s = &s[1..];
        }
        if s.is_empty() {
            return None;
        }

        if let Some(base) = radix_prefix(s) {
            if float {
                return None;
            }
            let start = radix_to_decimal_at(&s[2..], base, buf)?;
            return Some((negative, start..buf.len(), 0));
        }

        let e_idx = index_of_any(s, b"eE");
        let mantissa = match e_idx {
            Some(i) => &s[..i],
            None => s,
        };
        let dot = mantissa.iter().position(|&c| c == b'.');
        if !float {
            if e_idx.is_some() || dot.is_some() {
                return None;
            }
            if s.len() > 1 && s[0] == b'0' {
                match lexeme.leading_zero {
                    LeadingZero::Decimal => {}
                    LeadingZero::Octal => {
                        let start = radix_to_decimal_at(&s[1..], 8, buf)?;
                        return Some((negative, start..buf.len(), 0));
                    }
                    LeadingZero::Unknown => return None,
                }
            }
        }

        let mut len = 0usize;
        let mut frac_digits: i64 = 0;
        let mut any = false;
        for (i, &c) in mantissa.iter().enumerate() {
            if c == b'_' || dot == Some(i) {
                continue;
            }
            if !c.is_ascii_digit() {
                return None;
            }
            if len == buf.len() {
                return None;
            }
            buf[len] = c;
            len += 1;
            any = true;
            if dot.is_some_and(|d| i > d) {
                frac_digits += 1;
            }
        }
        if !any {
            return None;
        }

        let mut exponent: i64 = 0;
        if let Some(i) = e_idx {
            let mut e = &s[i + 1..];
            let mut e_negative = false;
            if !e.is_empty() && (e[0] == b'+' || e[0] == b'-') {
                e_negative = e[0] == b'-';
                e = &e[1..];
            }
            let mut e_any = false;
            for &c in e {
                if c == b'_' {
                    continue;
                }
                if !c.is_ascii_digit() {
                    return None;
                }
                exponent = exponent.checked_mul(10)?;
                exponent = exponent.checked_add((c - b'0') as i64)?;
                e_any = true;
            }
            if !e_any {
                return None;
            }
            if e_negative {
                exponent = -exponent;
            }
        }
        Some((negative, 0..len, exponent - frac_digits))
    }

    fn normalize(negative: bool, digits: &'b [u8], exponent: i64) -> Exact<'b> {
        let lead = digits.iter().take_while(|&&c| c == b'0').count();
        let mut d = &digits[lead..];
        let mut e = exponent;
        while !d.is_empty() && d[d.len() - 1] == b'0' {
            d = &d[..d.len() - 1];
            e += 1;
        }
        Exact { negative, digits: d, exponent: e }
    }
}

/// The base a `0x`/`0o`/`0b` prefix (either case) names, if `s` starts with one.
fn radix_prefix(s: &[u8]) -> Option<u8> {
    if s.len() >= 2 && s[0] == b'0' {
        match s[1] | 0x20 {
            b'x' => return Some(16),
            b'o' => return Some(8),
            b'b' => return Some(2),
            _ => {}
        }
    }
    None
}

/// `std.mem.indexOfAny`: the first index in `s` of any byte of `set`.
fn index_of_any(s: &[u8], set: &[u8]) -> Option<usize> {
    s.iter().position(|c| set.contains(c))
}

/// `digits`, in `base` with `_` separators, as decimal digits at the end of
/// `buf`; `None` when a digit is not one of `base`'s, there is none, or the
/// decimal outgrows `buf`. Digit by digit rather than through an integer
/// type, so no width bounds it but `buf`'s.
fn radix_to_decimal<'b>(digits: &[u8], base: u8, buf: &'b mut [u8]) -> Option<&'b [u8]> {
    let start = radix_to_decimal_at(digits, base, buf)?;
    Some(&buf[start..])
}

/// `radix_to_decimal`, answering where in `buf` the decimal starts (it ends
/// at `buf`'s end).
fn radix_to_decimal_at(digits: &[u8], base: u8, buf: &mut [u8]) -> Option<usize> {
    // `buf[start..]` holds the decimal so far, most significant first,
    // each byte a digit's value until the end.
    let mut start = buf.len();
    let mut any = false;
    for &c in digits {
        if c == b'_' {
            continue;
        }
        let d: u8 = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return None,
        };
        if d >= base {
            return None;
        }
        any = true;
        let mut carry: u16 = d as u16;
        let mut i = buf.len();
        while i > start {
            i -= 1;
            let x = buf[i] as u16 * base as u16 + carry;
            buf[i] = (x % 10) as u8;
            carry = x / 10;
        }
        while carry > 0 {
            if start == 0 {
                return None;
            }
            start -= 1;
            buf[start] = (carry % 10) as u8;
            carry /= 10;
        }
    }
    if !any {
        return None;
    }
    if start == buf.len() {
        start -= 1;
        buf[start] = 0;
    }
    for b in buf[start..].iter_mut() {
        *b += b'0';
    }
    Some(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellable_strict_json_takes_only_plain_decimal() {
        assert!(spellable(b"1000", JSON));
        assert!(spellable(b"-1.5e3", JSON));
        assert!(spellable(b"1.10", JSON));
        assert!(spellable(b"0", JSON));
        assert!(spellable(b"0.5", JSON));
        assert!(!spellable(b"0xff", JSON));
        assert!(!spellable(b"0o755", JSON));
        assert!(!spellable(b"0b1010", JSON));
        assert!(!spellable(b"1_000", JSON));
        assert!(!spellable(b"+5", JSON));
        assert!(!spellable(b".5", JSON));
        assert!(!spellable(b"5.", JSON));
        assert!(!spellable(b"0755", JSON));
    }

    #[test]
    fn spellable_json5_takes_hex_but_not_0o_0b_underscore() {
        assert!(spellable(b"0xff", JSON5));
        assert!(spellable(b"0XFF", JSON5));
        assert!(spellable(b"+5", JSON5));
        assert!(spellable(b".5", JSON5));
        assert!(spellable(b"5.", JSON5));
        assert!(!spellable(b"0o755", JSON5));
        assert!(!spellable(b"0b1010", JSON5));
        assert!(!spellable(b"0xf_f", JSON5));
        assert!(!spellable(b"1_000", JSON5));
        assert!(!spellable(b"0755", JSON5));
    }

    #[test]
    fn spellable_yaml_1_2_takes_hex_and_0o_but_not_0b_underscore() {
        assert!(spellable(b"0xff", YAML_1_2));
        assert!(spellable(b"0o755", YAML_1_2));
        assert!(spellable(b"0755", YAML_1_2)); // 1.2 reads decimal 755
        assert!(spellable(b"1.5e3", YAML_1_2));
        assert!(!spellable(b"0b1010", YAML_1_2)); // resolves to a string
        assert!(!spellable(b"1_000", YAML_1_2)); // resolves to a string
        assert!(!spellable(b"0xdead_beef", YAML_1_2));
    }

    #[test]
    fn non_finite_reads_every_source_spelling_and_nothing_else() {
        for raw in [&b"inf"[..], b"+inf", b".inf", b".Inf", b".INF", b"+.inf", b"Infinity", b"+Infinity"] {
            assert_eq!(non_finite(raw), Some(NonFinite::Inf), "{:?}", raw);
        }
        for raw in [&b"-inf"[..], b"-.inf", b"-.INF", b"-Infinity"] {
            assert_eq!(non_finite(raw), Some(NonFinite::NegInf), "{:?}", raw);
        }
        for raw in [&b"nan"[..], b"+nan", b"-nan", b".nan", b".NaN", b".NAN", b"NaN"] {
            assert_eq!(non_finite(raw), Some(NonFinite::Nan), "{:?}", raw);
        }
        for raw in [&b"1"[..], b"1e999999", b"0x1f", b".5", b"info", b"-", b"", b"."] {
            assert_eq!(non_finite(raw), None, "{:?}", raw);
        }
    }

    fn written(raw: &[u8], s: Spelling) -> crate::Result<Vec<u8>> {
        let mut w = Vec::new();
        write(&mut w, raw, s)?;
        Ok(w)
    }

    #[test]
    fn write_respells_a_non_finite_float_or_refuses_where_the_format_has_none() {
        assert_eq!(written(b"inf", YAML_1_2).unwrap(), b".inf");
        assert_eq!(written(b"-.inf", JSON5).unwrap(), b"-Infinity");
        assert_eq!(written(b".nan", JSON5).unwrap(), b"NaN");
        assert_eq!(written(b".inf", JSON), Err(err!(NonFiniteNumber)));
        assert_eq!(written(b"nan", JSON), Err(err!(NonFiniteNumber)));
        let toml = Spelling { non_finite: Some(PLAIN_NON_FINITE), ..Spelling::NONE };
        assert_eq!(written(b"-Infinity", toml).unwrap(), b"-inf");
        assert_eq!(written(b"+.NaN", toml).unwrap(), b"nan");
    }

    #[test]
    fn write_keeps_what_the_target_spells_and_canonicalizes_the_rest() {
        assert_eq!(written(b"0xff", JSON5).unwrap(), b"0xff");
        assert_eq!(written(b"0xff", JSON).unwrap(), b"255");
        assert_eq!(written(b"0o755", YAML_1_2).unwrap(), b"0o755");
        assert_eq!(written(b"0o755", JSON5).unwrap(), b"493");
        assert_eq!(written(b"0b1010", YAML_1_2).unwrap(), b"10");
        assert_eq!(written(b"+.5", JSON).unwrap(), b"0.5");
        assert_eq!(written(b"+.5", JSON5).unwrap(), b"+.5");
        assert_eq!(written(b"0755", JSON5).unwrap(), b"755");
        assert_eq!(written(b"0755", YAML_1_2).unwrap(), b"0755");
    }

    fn canon(raw: &[u8]) -> Vec<u8> {
        let mut w = Vec::new();
        write_canonical(&mut w, raw);
        w
    }

    fn eq_str(got: Vec<u8>, want: &str) {
        assert_eq!(String::from_utf8_lossy(&got), want);
    }

    /// Zig's test "writeCanonical".
    #[test]
    fn write_canonical_cases() {
        // Radix -> decimal, separators dropped.
        eq_str(canon(b"0xff"), "255");
        eq_str(canon(b"0xdead_beef"), "3735928559");
        eq_str(canon(b"0o755"), "493");
        eq_str(canon(b"0b1010"), "10");
        eq_str(canon(b"-0xf_f"), "-255");
        eq_str(canon(b"1_000"), "1000");

        // Bare dots padded; `+` dropped; leading zeros stripped.
        eq_str(canon(b".5"), "0.5");
        eq_str(canon(b"5."), "5.0");
        eq_str(canon(b"+15"), "15");
        eq_str(canon(b"0755"), "755");
        eq_str(canon(b"0"), "0");
        eq_str(canon(b"000"), "0");
        eq_str(canon(b"00.5"), "0.5");

        // Significant figures and precision survive — never re-formatted.
        eq_str(canon(b"1.10"), "1.10");
        eq_str(canon(b"1e2"), "1e2");
        eq_str(canon(b"1_0.5"), "10.5");
        eq_str(canon(b"123456789012345678901234567890"), "123456789012345678901234567890");

        // Past 128 bits, and the edges of the radix path.
        eq_str(canon(b"0x1_0000_0000_0000_0000_0000_0000_0000_0000"), "340282366920938463463374607431768211456");
        eq_str(canon(b"0x0"), "0");
        eq_str(canon(b"0b000"), "0");
        eq_str(canon(b"0x"), "0x");
        eq_str(canon(b"0o89"), "0o89");
        eq_str(canon(b"0xg"), "0xg");
    }

    #[test]
    fn write_canonical_more_edges() {
        eq_str(canon(b""), "0");
        eq_str(canon(b"-"), "-0");
        eq_str(canon(b"+"), "0");
        eq_str(canon(b"-.5e-3"), "-0.5e-3");
        eq_str(canon(b"1.5E+1_0"), "1.5E+10");
        eq_str(canon(b"."), "0.0");
        eq_str(canon(b"0X1F"), "31");
        eq_str(canon(b"0B11"), "3");
        eq_str(canon(b"0O17"), "15");
        eq_str(canon(b"1.2.3"), "1.2.3");
        eq_str(canon(b"-0x"), "-0x");
        eq_str(canon(b"0x_"), "0x_");
        // A decimal past 512 digits does not fit `buf`: the source, unsigned.
        let mut big = b"-0x".to_vec();
        big.extend(std::iter::repeat_n(b'f', 430));
        eq_str(canon(&big), &String::from_utf8_lossy(&big));
        // 420 hex digits are 506 decimal digits: they fit.
        let mut raw = b"0x".to_vec();
        raw.extend(std::iter::repeat_n(b'f', 420));
        let out = canon(&raw);
        assert!(out.len() > 500 && out.len() <= raw.len() + ANSWER_ROOM);
        assert!(out.iter().all(u8::is_ascii_digit));
    }

    /// kit/src/rules.zig's test: the canonical spelling fits in `ANSWER_ROOM`.
    #[test]
    fn canonical_fits_in_answer_room() {
        let mut raw = b"0x".to_vec();
        raw.extend(std::iter::repeat_n(b'f', 420));
        let n = canon(&raw).len();
        assert!(n > 500 && n <= raw.len() + ANSWER_ROOM);
        assert!(canon(b"0xffff").len() > 3);
    }

    #[test]
    fn spelling_bits() {
        assert_eq!(spelling(0), JSON);
        assert_eq!(spelling(1 | 32 | 64), Spelling { non_finite: None, ..JSON5 });
        assert_eq!(spelling(1 | 2 | 16 | 32 | 64), Spelling { non_finite: None, ..YAML_1_2 });
        assert_eq!(spelling(4 | 8), Spelling { binary: true, underscores: true, ..Spelling::NONE });
        assert_eq!(NonFinite::Nan as u8, 2);
    }

    fn int(a: &[u8], b: &[u8]) -> bool {
        eql_value(Lexeme::new(a, false), Lexeme::new(b, false))
    }

    #[test]
    fn eql_value_an_integer_is_its_value_across_bases_signs_and_separators() {
        assert!(int(b"0x1F", b"31"));
        assert!(int(b"0X1f", b"31"));
        assert!(int(b"0o37", b"0x1f"));
        assert!(int(b"0b11111", b"+31"));
        assert!(int(b"-0x1F", b"-31"));
        assert!(int(b"1_000_000", b"1000000"));
        assert!(int(b"0xdead_beef", b"3735928559"));
        assert!(int(b"-0", b"0"));
        assert!(int(b"+0", b"0x0"));
        // Past 64 and 128 bits, exact to the last digit.
        assert!(int(b"12345678901234567890123", b"12345678901234567890123"));
        assert!(!int(b"12345678901234567890123", b"12345678901234567890124"));
        assert!(int(b"0x1_0000_0000_0000_0000", b"18446744073709551616"));
        assert!(!int(b"18446744073709551617", b"18446744073709551616"));
        assert!(!int(b"0x1F", b"32"));
        assert!(!int(b"-31", b"31"));
        assert!(!int(b"1:30", b"90"));
    }

    fn lz(a: &[u8], zero: LeadingZero, b: &[u8]) -> bool {
        eql_value(Lexeme { raw: a, float: false, leading_zero: zero }, Lexeme::new(b, false))
    }

    #[test]
    fn eql_value_a_leading_zero_integer_by_the_dialect_that_read_it() {
        assert!(lz(b"0755", LeadingZero::Decimal, b"755"));
        assert!(!lz(b"0755", LeadingZero::Decimal, b"493"));
        assert!(lz(b"0755", LeadingZero::Octal, b"493"));
        assert!(lz(b"-0_755", LeadingZero::Octal, b"-0o755"));
        assert!(!lz(b"0755", LeadingZero::Octal, b"755"));
        // Not known: only the same bytes agree.
        assert!(lz(b"0755", LeadingZero::Unknown, b"0755"));
        assert!(!lz(b"0755", LeadingZero::Unknown, b"755"));
        assert!(!lz(b"0755", LeadingZero::Unknown, b"493"));
        // `08` is not octal, so it is not read as one.
        assert!(!lz(b"08", LeadingZero::Octal, b"8"));
    }

    fn flt(a: &[u8], b: &[u8]) -> bool {
        eql_value(Lexeme::new(a, true), Lexeme::new(b, true))
    }

    #[test]
    fn eql_value_a_float_is_its_exact_decimal_value() {
        assert!(flt(b"1.50", b"1.5"));
        assert!(flt(b"1e3", b"1000.0"));
        assert!(flt(b"1E+3", b"1e3"));
        assert!(flt(b"1.5E+300", b"15e299"));
        assert!(flt(b".5", b"0.5"));
        assert!(flt(b"5.", b"5.0"));
        assert!(flt(b"+1.5", b"1.5"));
        assert!(flt(b"1_000.5", b"1000.5"));
        assert!(flt(b"1e1_0", b"1e10"));
        assert!(flt(b"0.0", b"0e5"));
        assert!(flt(b"100000000000000000000.0", b"1e20"));
        assert!(!flt(b"1.5", b"1.25"));
        assert!(!flt(b"-1.5", b"1.5"));
        // One double, two values: the digits the source wrote are the value.
        assert!(!flt(b"0.1", b"0.1000000000000000055511151231257827"));
        assert!(!flt(b"9007199254740993.0", b"9007199254740992.0"));
        // A signed zero is a different float.
        assert!(!flt(b"-0.0", b"0.0"));
        assert!(flt(b"-0.0", b"-0e0"));
        assert!(flt(b"+0.0", b"0.0"));
    }

    #[test]
    fn eql_value_non_finite_floats_in_every_spelling_nan_included() {
        assert!(flt(b".inf", b"Infinity"));
        assert!(flt(b"inf", b"+.Inf"));
        assert!(flt(b"-inf", b"-Infinity"));
        assert!(flt(b"nan", b"NaN"));
        assert!(flt(b".NaN", b"-nan"));
        assert!(!flt(b"inf", b"-inf"));
        assert!(!flt(b"inf", b"nan"));
        assert!(!flt(b"inf", b"1e999999"));
    }

    #[test]
    fn eql_value_an_integer_never_equals_a_float() {
        assert!(!eql_value(Lexeme::new(b"1", false), Lexeme::new(b"1.0", true)));
        assert!(!eql_value(Lexeme::new(b"1000", false), Lexeme::new(b"1e3", true)));
        assert!(!eql_value(Lexeme::new(b"0", false), Lexeme::new(b"0.0", true)));
        assert!(!eql_value(Lexeme::new(b"1", true), Lexeme::new(b"1", false)));
    }

    #[test]
    fn eql_value_unreadable_lexemes_compare_by_bytes() {
        // A radix float, an exponent overflowing i64, an empty exponent, a
        // bare sign, digits past the buffer: none reads, so only equal bytes agree.
        assert!(!flt(b"0x10", b"16.0"));
        assert!(!flt(b"1e99999999999999999999", b"1e99999999999999999999_"));
        assert!(!flt(b"1e", b"1"));
        assert!(!int(b"-", b"+"));
        let long = vec![b'1'; 513];
        let mut long2 = long.clone();
        long2.insert(0, b'+');
        assert!(!int(&long, &long2));
        let fits = vec![b'1'; 512];
        let mut fits2 = fits.clone();
        fits2.insert(0, b'+');
        assert!(int(&fits, &fits2));
        // Floats with an integer's exponent forms.
        assert!(flt(b"1.0e-1", b"0.1"));
        assert!(flt(b"0.00e0", b"0"));
        assert!(!int(b"1e3", b"1000"));
        assert!(!int(b"1.0", b"1"));
    }
}
