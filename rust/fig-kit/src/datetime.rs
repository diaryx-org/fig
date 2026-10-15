//! Port of kit/src/datetime.zig: shared RFC-3339-style date/time validation
//! and classification.
//!
//! TOML datetimes and YAML 1.1 `!!timestamp` scalars share the same lexical
//! core — `YYYY-MM-DD`, an optional `T`/`t`/space separator, `HH:MM:SS[.frac]`,
//! and a `Z`/`±HH:MM` zone — so the validation lives here once. Callers map the
//! returned `Kind` onto their own scalar type (both use the AST `ExtKind`, whose
//! first four members line up with `Kind`); this module stays AST-free so it
//! remains a leaf utility.
//!
//! `classify` is total: it returns `error.InvalidDatetime` for anything that is
//! not a well-formed datetime. That lets a parser use it two ways — to validate
//! a token it already believes is a datetime (TOML), or to *probe* an arbitrary
//! plain scalar and treat the error as "not a timestamp" (YAML). Because of the
//! probe use, every field is digit-checked here rather than trusting a tokenizer
//! to have done it.
//!
//! Errors: `InvalidDatetime`.

use super::ascii;
use crate::{Result, err};

/// Which datetime shape a string resolved to. Mirrors the first four members of
/// the AST `ExtKind`, kept independent so this leaf utility doesn't depend on
/// the AST. The discriminants are `FIG_EXT_*` (kit/src/rules.zig).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    OffsetDatetime = 0,
    LocalDatetime = 1,
    LocalDate = 2,
    LocalTime = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Accept `HH:MM` with the seconds omitted. TOML 1.0 requires seconds (pass
    /// false); TOML 1.1 and YAML are lenient.
    pub allow_minute_precision: bool,
    /// Accept a bare `HH:MM:SS[.frac]` with no date as a `local_time`. TOML
    /// allows it; YAML 1.1 does not (there a `:`-run with no date is a
    /// sexagesimal number), so YAML passes false.
    pub allow_time_only: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { allow_minute_precision: true, allow_time_only: true }
    }
}

/// `Options` as `FIG_DATETIME_*` bits: minute precision, then a time with no
/// date. From kit/src/rules.zig.
pub fn options(bits: u32) -> Options {
    Options { allow_minute_precision: bits & 1 != 0, allow_time_only: bits & 2 != 0 }
}

pub fn classify(raw: &[u8], opts: Options) -> Result<Kind> {
    // Time-only: HH:MM...
    if opts.allow_time_only && raw.len() >= 3 && raw[2] == b':' {
        validate_time(raw, opts)?;
        return Ok(Kind::LocalTime);
    }
    // Date present: YYYY-MM-DD.
    if raw.len() < 10 {
        return Err(err!(InvalidDatetime));
    }
    validate_date(&raw[..10])?;
    if raw.len() == 10 {
        return Ok(Kind::LocalDate);
    }

    // Separator, then time (+ optional zone).
    let sep = raw[10];
    if sep != b'T' && sep != b't' && sep != b' ' {
        return Err(err!(InvalidDatetime));
    }
    let rest = &raw[11..];

    // Zone: trailing Z/z, or ±HH:MM at the end.
    let mut time_str = rest;
    let mut has_offset = false;
    let n = rest.len();
    if n > 0 && (rest[n - 1] == b'Z' || rest[n - 1] == b'z') {
        time_str = &rest[..n - 1];
        has_offset = true;
    } else if n >= 6 && (rest[n - 6] == b'+' || rest[n - 6] == b'-') && rest[n - 3] == b':' {
        validate_offset(&rest[n - 6..])?;
        time_str = &rest[..n - 6];
        has_offset = true;
    }
    validate_time(time_str, opts)?;
    Ok(if has_offset { Kind::OffsetDatetime } else { Kind::LocalDatetime })
}

fn two_digit(s: &[u8], at: usize) -> u8 {
    (s[at] - b'0') * 10 + (s[at + 1] - b'0')
}

/// Both bytes at `s[at..at+2]` are ASCII digits.
fn both_digits(s: &[u8], at: usize) -> bool {
    at + 1 < s.len() && ascii::is_digit(s[at]) && ascii::is_digit(s[at + 1])
}

pub fn validate_date(s: &[u8]) -> Result<()> {
    if s.len() != 10 || s[4] != b'-' || s[7] != b'-' {
        return Err(err!(InvalidDatetime));
    }
    if !both_digits(s, 0) || !both_digits(s, 2) || !both_digits(s, 5) || !both_digits(s, 8) {
        return Err(err!(InvalidDatetime));
    }
    let year = two_digit(s, 0) as u16 * 100 + two_digit(s, 2) as u16;
    let month = two_digit(s, 5);
    let day = two_digit(s, 8);
    if !(1..=12).contains(&month) {
        return Err(err!(InvalidDatetime));
    }
    if day < 1 || day > days_in_month(year, month) {
        return Err(err!(InvalidDatetime));
    }
    Ok(())
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

/// `HH:MM[:SS[.fraction]]`; seconds 00-60 (leap second allowed). With
/// `allow_minute_precision` false the seconds are mandatory.
pub fn validate_time(s: &[u8], opts: Options) -> Result<()> {
    if s.len() < 5 || s[2] != b':' {
        return Err(err!(InvalidDatetime));
    }
    if !both_digits(s, 0) || !both_digits(s, 3) {
        return Err(err!(InvalidDatetime));
    }
    if two_digit(s, 0) > 23 || two_digit(s, 3) > 59 {
        return Err(err!(InvalidDatetime));
    }
    if s.len() == 5 {
        if !opts.allow_minute_precision {
            return Err(err!(InvalidDatetime));
        }
        return Ok(());
    }
    if s[5] != b':' || s.len() < 8 {
        return Err(err!(InvalidDatetime));
    }
    if !both_digits(s, 6) || two_digit(s, 6) > 60 {
        return Err(err!(InvalidDatetime));
    }
    if s.len() == 8 {
        return Ok(());
    }
    if s[8] != b'.' || s.len() < 10 {
        return Err(err!(InvalidDatetime));
    }
    if s[9..].iter().any(|&c| !ascii::is_digit(c)) {
        return Err(err!(InvalidDatetime));
    }
    Ok(())
}

/// `±HH:MM` zone offset; hour 00-23, minute 00-59.
pub fn validate_offset(s: &[u8]) -> Result<()> {
    if s.len() != 6 || s[3] != b':' {
        return Err(err!(InvalidDatetime));
    }
    if !both_digits(s, 1) || !both_digits(s, 4) {
        return Err(err!(InvalidDatetime));
    }
    if two_digit(s, 1) > 23 || two_digit(s, 4) > 59 {
        return Err(err!(InvalidDatetime));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_shapes() {
        let cases: &[(&[u8], Kind)] = &[
            (b"1979-05-27T07:32:00Z", Kind::OffsetDatetime),
            (b"1979-05-27T07:32:00.999-07:00", Kind::OffsetDatetime),
            (b"1979-05-27T07:32:00", Kind::LocalDatetime),
            (b"1979-05-27 07:32:00", Kind::LocalDatetime),
            (b"1979-05-27", Kind::LocalDate),
            (b"07:32:00", Kind::LocalTime),
            (b"00:32:00.999999", Kind::LocalTime),
        ];
        for &(src, kind) in cases {
            assert_eq!(classify(src, Options::default()), Ok(kind), "{:?}", String::from_utf8_lossy(src));
        }
    }

    #[test]
    fn rejects_malformed_datetimes() {
        let bad: &[&[u8]] = &[
            b"",
            b"hello",
            b"2002-13-01",
            b"2002-02-30",
            b"20A2-12-14",
            b"1979-05-27X07:32:00",
            b"1979-05-27T07:60:00",
            b"2002-12-14T12:30",
        ];
        let opts = Options { allow_minute_precision: false, ..Options::default() };
        for &s in bad {
            assert_eq!(classify(s, opts), Err(err!(InvalidDatetime)), "{:?}", String::from_utf8_lossy(s));
        }
    }

    #[test]
    fn options_gate_time_only_and_minute_precision() {
        // Time-only refused when disallowed (YAML).
        let no_time_only = Options { allow_time_only: false, ..Options::default() };
        assert_eq!(classify(b"07:32:00", no_time_only), Err(err!(InvalidDatetime)));
        // Minute precision refused when disallowed (TOML 1.0).
        let no_minutes = Options { allow_minute_precision: false, ..Options::default() };
        assert_eq!(classify(b"1979-05-27T07:32", no_minutes), Err(err!(InvalidDatetime)));
        assert_eq!(classify(b"1979-05-27T07:32", Options::default()), Ok(Kind::LocalDatetime));
    }

    #[test]
    fn edges_of_each_field() {
        let d = Options::default();
        assert_eq!(classify(b"2000-02-29", d), Ok(Kind::LocalDate)); // 400-year leap
        assert!(classify(b"1900-02-29", d).is_err()); // 100-year non-leap
        assert_eq!(classify(b"2024-02-29", d), Ok(Kind::LocalDate));
        assert_eq!(classify(b"23:59:60", d), Ok(Kind::LocalTime)); // leap second
        assert!(classify(b"24:00:00", d).is_err());
        assert!(classify(b"07:32:61", d).is_err());
        assert!(classify(b"07:32:00.", d).is_err()); // a dot needs a digit
        assert!(classify(b"07:32:00.1a", d).is_err());
        assert!(classify(b"07:3", d).is_err());
        assert_eq!(classify(b"1979-05-27t07:32:00z", d), Ok(Kind::OffsetDatetime));
        assert_eq!(classify(b"1979-05-27T07:32+23:59", d), Ok(Kind::OffsetDatetime));
        assert!(classify(b"1979-05-27T07:32:00+24:00", d).is_err());
        assert!(classify(b"1979-05-27T", d).is_err());
        assert_eq!(validate_offset(b"-07:00"), Ok(()));
        assert!(validate_offset(b"+7:00").is_err());
        assert!(validate_date(b"1979-5-27").is_err());
        assert!(validate_date(b"1979-05-00").is_err());
        assert_eq!(options(0), Options { allow_minute_precision: false, allow_time_only: false });
        assert_eq!(options(3), Options::default());
        assert_eq!(Kind::LocalTime as u8, 3);
    }
}
