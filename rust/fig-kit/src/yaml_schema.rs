//! Port of kit/src/yaml_schema.zig.
//!
//! The YAML core schema's reading of a plain scalar and a tag: which scalars
//! are numbers under 1.2.2 and under 1.1, and which tags name a core type.
//! Pure functions of a lexeme. The YAML language resolves its tags with
//! them, and core's materializer judges a runtime YAML's tree by the same
//! rules, so they live here rather than in either.

use super::ast::TagDirective;
use super::eql_any;

use super::ascii;

/// Every char of `s` is a `pred` digit or a `_` separator, with at least
/// one real digit (so `0x` / `0b` with an empty body is rejected).
fn digit_run(s: &[u8], pred: fn(u8) -> bool) -> bool {
    let mut any = false;
    for &c in s {
        if c == b'_' {
            continue;
        }
        if !pred(c) {
            return false;
        }
        any = true;
    }
    any
}

/// If `tag` names a YAML core-schema type, returns the bare name (`str`,
/// `int`, `bool`, `null`, `seq`, `map`, …). Accepts the `!!name` shorthand
/// and the verbose `tag:yaml.org,2002:name` form (with or without `!<>`).
pub fn core_tag_name<'a>(directives: &[TagDirective<'_>], tag: &'a [u8]) -> Option<&'a [u8]> {
    const CORE: &[u8] = b"tag:yaml.org,2002:";
    if tag.len() >= 3 && tag[0] == b'!' && tag[1] == b'<' && tag[tag.len() - 1] == b'>' {
        let inner = &tag[2..tag.len() - 1];
        if inner.starts_with(CORE) {
            return Some(&inner[CORE.len()..]);
        }
        return None;
    }
    if tag.starts_with(CORE) {
        return Some(&tag[CORE.len()..]);
    }
    if tag.is_empty() || tag[0] != b'!' {
        return None;
    }
    // A shorthand `handle` + `suffix`: `!!int`, `!e!int`, or the primary
    // `!int`. A `%TAG` directive in the document can bind any handle, `!!`
    // and `!` included, to a prefix of its own; unbound, `!!` is the core
    // schema's and `!` a local tag's.
    let close = if tag.len() > 1 { tag[1..].iter().position(|&c| c == b'!').map(|i| i + 1) } else { None };
    let handle: &[u8] = match close {
        Some(c) => &tag[..c + 1],
        None => b"!",
    };
    let suffix = &tag[handle.len()..];
    let prefix: &[u8] = match directives.iter().find(|d| d.handle == handle) {
        Some(d) => d.prefix,
        None if handle == b"!!" => CORE,
        None => return None,
    };
    // `prefix ++ suffix` names a core type when it is `core ++ name`. A
    // prefix that runs past `core` into the name is not a spelling anyone
    // writes, and is read as custom.
    if prefix.len() > CORE.len() || !CORE.starts_with(prefix) {
        return None;
    }
    let rest = &CORE[prefix.len()..];
    if !suffix.starts_with(rest) {
        return None;
    }
    let name = &suffix[rest.len()..];
    if name.is_empty() { None } else { Some(name) }
}

/// Whether `t` spells an integer by radix (`0x1A`, `0o17`, `0b101`), which
/// a `!!float` does not take.
pub fn has_radix_prefix(t: &[u8]) -> bool {
    let s = if !t.is_empty() && (t[0] == b'+' || t[0] == b'-') { &t[1..] } else { t };
    s.len() > 2 && s[0] == b'0' && b"xXoObB".contains(&s[1])
}

/// Classify a plain scalar against the YAML 1.1 int/float tags. Order
/// matters: `.inf`/`.nan` and sexagesimal are checked before the radix
/// prefixes, and the radix prefixes before the decimal/octal/float fork.
pub fn classify1_1_number(source: &[u8]) -> NumberClass {
    if source.is_empty() {
        return NumberClass::NotNumber;
    }
    if is_inf_nan(source) {
        return NumberClass::Float;
    }

    let mut s = source;
    if s[0] == b'+' || s[0] == b'-' {
        s = &s[1..];
    }
    if s.is_empty() {
        return NumberClass::NotNumber;
    }

    // Sexagesimal (`H:MM:SS[.frac]`) — int or float depending on a trailing `.`.
    if s.contains(&b':') {
        return classify1_1_base60(s);
    }

    // Radix-prefixed integers: hex `0x…`, binary `0b…`.
    if s.len() >= 2 && s[0] == b'0' && (s[1] == b'x' || s[1] == b'X') {
        return if digit_run(&s[2..], ascii::is_hex) { NumberClass::Integer } else { NumberClass::NotNumber };
    }
    if s.len() >= 2 && s[0] == b'0' && (s[1] == b'b' || s[1] == b'B') {
        return if digit_run(&s[2..], ascii::is_binary) { NumberClass::Integer } else { NumberClass::NotNumber };
    }

    // A `.` makes it a base-10 float (1.1 requires the dot; `1e3` is a string).
    if s.contains(&b'.') {
        return if is1_1_base10_float(s) { NumberClass::Float } else { NumberClass::NotNumber };
    }

    // Leading-zero octal `0[0-7_]+` (1.1 has no `0o`; `08` is therefore a string).
    if s.len() >= 2 && s[0] == b'0' {
        return if digit_run(&s[1..], ascii::is_octal) { NumberClass::Integer } else { NumberClass::NotNumber };
    }

    // Plain decimal `0 | [1-9][0-9_]*`.
    if s.len() == 1 && s[0] == b'0' {
        return NumberClass::Integer;
    }
    if (b'1'..=b'9').contains(&s[0]) {
        for &c in &s[1..] {
            if !(ascii::is_digit(c) || c == b'_') {
                return NumberClass::NotNumber;
            }
        }
        return NumberClass::Integer;
    }
    NumberClass::NotNumber
}

/// 1.1 base-10 float: `([0-9][0-9_]*)?\.[0-9_]*([eE][-+][0-9]+)?` (sign
/// already stripped). The dot is mandatory; an exponent, if present, must
/// carry a sign.
fn is1_1_base10_float(s: &[u8]) -> bool {
    let mut i = 0usize;
    // Optional integer part.
    if i < s.len() && ascii::is_digit(s[i]) {
        i += 1;
        while i < s.len() && (ascii::is_digit(s[i]) || s[i] == b'_') {
            i += 1;
        }
    }
    // Mandatory `.`.
    if i >= s.len() || s[i] != b'.' {
        return false;
    }
    i += 1;
    // Fractional digits (may be empty: `5.`).
    while i < s.len() && (ascii::is_digit(s[i]) || s[i] == b'_') {
        i += 1;
    }
    // Optional signed exponent.
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        i += 1;
        if i >= s.len() || (s[i] != b'+' && s[i] != b'-') {
            return false;
        }
        i += 1;
        let mut exp_digits = 0usize;
        while i < s.len() && ascii::is_digit(s[i]) {
            exp_digits += 1;
            i += 1;
        }
        if exp_digits == 0 {
            return false;
        }
    }
    i == s.len()
}

/// Sexagesimal: `[1-9][0-9_]*(:[0-5]?[0-9])+` (int) or the same with a
/// trailing `.[0-9_]*` fractional (float). `s` is sign-stripped and known to
/// contain `:`.
fn classify1_1_base60(s: &[u8]) -> NumberClass {
    let mut body = s;
    let mut is_float = false;
    if let Some(dot) = s.iter().position(|&c| c == b'.') {
        is_float = true;
        for &c in &s[dot + 1..] {
            if !(ascii::is_digit(c) || c == b'_') {
                return NumberClass::NotNumber;
            }
        }
        body = &s[..dot];
    }

    let mut trailing = 0usize;
    for (idx, g) in body.split(|&c| c == b':').enumerate() {
        if g.is_empty() {
            return NumberClass::NotNumber;
        }
        if idx == 0 {
            // First group `[1-9][0-9_]*` (int) / `[0-9][0-9_]*` (float).
            if !ascii::is_digit(g[0]) {
                return NumberClass::NotNumber;
            }
            if !is_float && g[0] == b'0' {
                return NumberClass::NotNumber;
            }
            for &c in &g[1..] {
                if !(ascii::is_digit(c) || c == b'_') {
                    return NumberClass::NotNumber;
                }
            }
        } else {
            // Subsequent groups `[0-5]?[0-9]`.
            if g.is_empty() || g.len() > 2 {
                return NumberClass::NotNumber;
            }
            for &c in g {
                if !ascii::is_digit(c) {
                    return NumberClass::NotNumber;
                }
            }
            if g.len() == 2 && g[0] > b'5' {
                return NumberClass::NotNumber;
            }
            trailing += 1;
        }
    }
    if trailing == 0 {
        return NumberClass::NotNumber;
    }
    if is_float { NumberClass::Float } else { NumberClass::Integer }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumberClass {
    NotNumber,
    Integer,
    Float,
}

/// Classify a plain scalar against the YAML 1.2.2 core schema: decimal ints
/// with an optional sign, hex (0x) and octal (0o) ints, floats with
/// fractions and/or exponents, and the special floats .inf / .nan. (The
/// YAML tokenizer doesn't distinguish numbers from other plain scalars.)
pub fn classify_number(source: &[u8]) -> NumberClass {
    if source.is_empty() {
        return NumberClass::NotNumber;
    }
    if is_inf_nan(source) {
        return NumberClass::Float;
    }

    // Hex and octal integers take no sign in the core schema.
    if source.len() > 2 && source[0] == b'0' && (source[1] == b'x' || source[1] == b'o') {
        let hex = source[1] == b'x';
        for &c in &source[2..] {
            let ok = if hex { ascii::is_hex(c) } else { ascii::is_octal(c) };
            if !ok {
                return NumberClass::NotNumber;
            }
        }
        return NumberClass::Integer;
    }

    let mut i = 0usize;
    if source[i] == b'+' || source[i] == b'-' {
        i += 1;
    }

    let mut mantissa_digits = 0usize;
    while i < source.len() && source[i].is_ascii_digit() {
        mantissa_digits += 1;
        i += 1;
    }

    let mut is_float = false;
    if i < source.len() && source[i] == b'.' {
        is_float = true;
        i += 1;
        while i < source.len() && source[i].is_ascii_digit() {
            mantissa_digits += 1;
            i += 1;
        }
    }
    if mantissa_digits == 0 {
        return NumberClass::NotNumber;
    }

    if i < source.len() && (source[i] == b'e' || source[i] == b'E') {
        is_float = true;
        i += 1;
        if i < source.len() && (source[i] == b'+' || source[i] == b'-') {
            i += 1;
        }
        let mut exp_digits = 0usize;
        while i < source.len() && source[i].is_ascii_digit() {
            exp_digits += 1;
            i += 1;
        }
        if exp_digits == 0 {
            return NumberClass::NotNumber;
        }
    }

    if i != source.len() {
        return NumberClass::NotNumber;
    }
    if is_float { NumberClass::Float } else { NumberClass::Integer }
}

fn is_inf_nan(source: &[u8]) -> bool {
    let mut body = source;
    if source[0] == b'+' || source[0] == b'-' {
        body = &source[1..];
    }
    if eql_any(body, &[b".inf", b".Inf", b".INF"]) {
        return true;
    }
    eql_any(source, &[b".nan", b".NaN", b".NAN"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_tag_names_by_shorthand_verbatim_and_directive() {
        assert_eq!(Some(&b"int"[..]), core_tag_name(&[], b"!!int"));
        assert_eq!(Some(&b"str"[..]), core_tag_name(&[], b"!<tag:yaml.org,2002:str>"));
        assert_eq!(None, core_tag_name(&[], b"!foo"));
        let d = [TagDirective { handle: b"!e!", prefix: b"tag:yaml.org,2002:" }];
        assert_eq!(Some(&b"map"[..]), core_tag_name(&d, b"!e!map"));
    }

    #[test]
    fn numbers_under_both_schemas() {
        assert_eq!(NumberClass::Integer, classify_number(b"-12"));
        assert_eq!(NumberClass::Float, classify_number(b"1e3"));
        assert_eq!(NumberClass::NotNumber, classify_number(b"0b101"));
        assert_eq!(NumberClass::Integer, classify1_1_number(b"0b101"));
        assert_eq!(NumberClass::NotNumber, classify1_1_number(b"1e3"));
        assert_eq!(NumberClass::Integer, classify1_1_number(b"190:20:30"));
        assert_eq!(NumberClass::Float, classify1_1_number(b"-.inf"));
        assert!(has_radix_prefix(b"+0x1A"));
    }
}
