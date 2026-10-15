//! Port of kit/src/ascii.zig: small character-class predicates shared by the
//! format parsers.
//!
//! `is_digit`/`is_hex` are std's ASCII classes, given here so callers have a
//! single import point for the whole set; `is_octal`/`is_binary` aren't in
//! std, so they live here. Each has the `fn(u8) -> bool` shape the parsers
//! pass as a predicate to their underscore/digit-run validators.

pub fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}

pub fn is_hex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

pub fn is_octal(c: u8) -> bool {
    (b'0'..=b'7').contains(&c)
}

pub fn is_binary(c: u8) -> bool {
    c == b'0' || c == b'1'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_predicates() {
        assert!(is_digit(b'0') && is_digit(b'9') && !is_digit(b'a'));
        assert!(is_hex(b'f') && is_hex(b'A') && !is_hex(b'g'));
        assert!(is_octal(b'7') && !is_octal(b'8'));
        assert!(is_binary(b'1') && !is_binary(b'2'));
    }
}
