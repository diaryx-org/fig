//! Port of lib/std/zig/string_literal.zig (Zig 0.17.0), and of
//! `std.zig.ZonGen.parseStrLit`: decoding a Zig string or character
//! literal, as fig's ZON parser has the standard library do it.
//!
//! The error payloads (`Error`, which says where a literal went wrong) are
//! kept, though fig only asks whether a literal decodes.

/// Why a literal does not decode, and where (Zig's `Error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The character after backslash is missing or not recognized.
    InvalidEscapeCharacter(usize),
    /// Expected hex digit at this index.
    ExpectedHexDigit(usize),
    /// Unicode escape sequence had no digits with rbrace at this index.
    EmptyUnicodeEscapeSequence(usize),
    /// Expected hex digit or '}' at this index.
    ExpectedHexDigitOrRbrace(usize),
    /// Invalid unicode codepoint at this index.
    InvalidUnicodeCodepoint(usize),
    /// Expected '{' at this index.
    ExpectedLbrace(usize),
    /// Expected '}' at this index.
    ExpectedRbrace(usize),
    /// Expected '\'' at this index.
    ExpectedSingleQuote(usize),
    /// The character at this index cannot be represented without an escape
    /// sequence.
    InvalidCharacter(usize),
    /// `''`. Not returned for string literals.
    EmptyCharLiteral,
}

/// The codepoint of a UTF-8 sequence of `bytes.len()` bytes, as Zig's
/// `utf8Decode2`/`3`/`4` check it (continuation bytes, overlongs,
/// surrogates and the Unicode range), or `None`.
fn utf8_decode(bytes: &[u8]) -> Option<u32> {
    std::str::from_utf8(bytes).ok().and_then(|s| {
        let mut chars = s.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        Some(c as u32)
    })
}

/// Zig's `std.unicode.utf8ByteSequenceLength`.
fn utf8_byte_sequence_length(first: u8) -> Option<usize> {
    match first {
        0x00..=0x7f => Some(1),
        0xc0..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf7 => Some(4),
        _ => None,
    }
}

/// The codepoint a character literal (`'a'`, `'\n'`, `'\u{1F600}'`) spells.
/// Asserts the slice starts and ends with single-quotes; there must be
/// exactly one UTF-8 codepoint in between.
pub fn parse_char_literal(slice: &[u8]) -> Result<u32, Error> {
    if slice.len() < 3 {
        return Err(Error::EmptyCharLiteral);
    }
    debug_assert!(slice[0] == b'\'' && slice[slice.len() - 1] == b'\'');

    match slice[1] {
        b'\\' => {
            let mut offset = 1usize;
            let result = parse_escape_sequence(slice, &mut offset);
            if result.is_ok() && (offset + 1 != slice.len() || slice[offset] != b'\'') {
                return Err(Error::ExpectedSingleQuote(offset));
            }
            result
        }
        0 => Err(Error::InvalidCharacter(1)),
        _ => {
            let inner = &slice[1..slice.len() - 1];
            let n = utf8_byte_sequence_length(inner[0]).ok_or(Error::InvalidUnicodeCodepoint(1))?;
            if inner.len() > n {
                return Err(Error::ExpectedSingleQuote(1 + n));
            }
            if n == 1 {
                return Ok(inner[0] as u32);
            }
            // A sequence cut short by the closing quote reads past it in Zig;
            // here it is a codepoint that does not decode.
            if inner.len() < n {
                return Err(Error::InvalidUnicodeCodepoint(1));
            }
            utf8_decode(&inner[..n]).ok_or(Error::InvalidUnicodeCodepoint(1))
        }
    }
}

/// Parse an escape sequence from `slice[*offset..]`. If parsing is
/// successful, `offset` is updated to reflect the characters consumed.
pub fn parse_escape_sequence(slice: &[u8], offset: &mut usize) -> Result<u32, Error> {
    debug_assert!(slice.len() > *offset);
    debug_assert!(slice[*offset] == b'\\');

    if slice.len() == *offset + 1 {
        return Err(Error::InvalidEscapeCharacter(*offset + 1));
    }

    *offset += 2;
    match slice[*offset - 1] {
        b'n' => Ok(b'\n' as u32),
        b'r' => Ok(b'\r' as u32),
        b'\\' => Ok(b'\\' as u32),
        b't' => Ok(b'\t' as u32),
        b'\'' => Ok(b'\'' as u32),
        b'"' => Ok(b'"' as u32),
        b'x' => {
            let mut value: u32 = 0;
            let mut i = *offset;
            while i < *offset + 2 {
                if i == slice.len() {
                    return Err(Error::ExpectedHexDigit(i));
                }
                let c = slice[i];
                let d = match c {
                    b'0'..=b'9' => c - b'0',
                    b'a'..=b'f' => c - b'a' + 10,
                    b'A'..=b'F' => c - b'A' + 10,
                    _ => return Err(Error::ExpectedHexDigit(i)),
                };
                value = value * 16 + d as u32;
                i += 1;
            }
            *offset = i;
            Ok(value)
        }
        b'u' => {
            let mut i = *offset;
            if i >= slice.len() || slice[i] != b'{' {
                return Err(Error::ExpectedLbrace(i));
            }
            i += 1;
            if i >= slice.len() {
                return Err(Error::ExpectedHexDigitOrRbrace(i));
            }
            if slice[i] == b'}' {
                return Err(Error::EmptyUnicodeEscapeSequence(i));
            }

            let mut value: u32 = 0;
            let mut closed = false;
            while i < slice.len() {
                let c = slice[i];
                let d = match c {
                    b'0'..=b'9' => c - b'0',
                    b'a'..=b'f' => c - b'a' + 10,
                    b'A'..=b'F' => c - b'A' + 10,
                    b'}' => {
                        i += 1;
                        closed = true;
                        break;
                    }
                    _ => return Err(Error::ExpectedHexDigitOrRbrace(i)),
                };
                value = value * 16 + d as u32;
                if value > 0x10ffff {
                    return Err(Error::InvalidUnicodeCodepoint(i));
                }
                i += 1;
            }
            if !closed {
                return Err(Error::ExpectedRbrace(i));
            }
            *offset = i;
            Ok(value)
        }
        _ => Err(Error::InvalidEscapeCharacter(*offset - 1)),
    }
}

/// Parses `bytes` as a Zig string literal and appends the result to `out`
/// (Zig's `parseWrite`). Asserts `bytes` has '"' at beginning and end.
pub fn parse_write(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error> {
    debug_assert!(bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"');

    let mut index = 1usize;
    loop {
        let b = bytes[index];

        match b {
            b'\\' => {
                let escape_char_index = index + 1;
                let codepoint = parse_escape_sequence(bytes, &mut index)?;
                if bytes[escape_char_index] == b'u' {
                    let c = char::from_u32(codepoint).ok_or(Error::InvalidUnicodeCodepoint(escape_char_index + 1))?;
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                } else {
                    out.push(codepoint as u8);
                }
            }
            b'\n' => return Err(Error::InvalidCharacter(index)),
            b'"' => return Ok(()),
            _ => {
                out.push(b);
                index += 1;
            }
        }
    }
}

/// Higher level API: the decoded string, or `None` (Zig's `parseAlloc`,
/// whose `InvalidLiteral` carries no detail).
pub fn parse_alloc(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    parse_write(&mut out, bytes).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_char_literal_cases() {
        assert_eq!(Ok(b'a' as u32), parse_char_literal(b"'a'"));
        assert_eq!(Ok('ä' as u32), parse_char_literal("'ä'".as_bytes()));
        assert_eq!(Ok(0), parse_char_literal(b"'\\x00'"));
        assert_eq!(Ok(0x4f), parse_char_literal(b"'\\x4f'"));
        assert_eq!(Ok(0x4f), parse_char_literal(b"'\\x4F'"));
        assert_eq!(Ok(0x3041), parse_char_literal("'ぁ'".as_bytes()));
        assert_eq!(Ok(0), parse_char_literal(b"'\\u{0}'"));
        assert_eq!(Ok(0x3041), parse_char_literal(b"'\\u{3041}'"));
        assert_eq!(Ok(0x7f), parse_char_literal(b"'\\u{7f}'"));
        assert_eq!(Ok(0x7fff), parse_char_literal(b"'\\u{7FFF}'"));
        assert_eq!(Err(Error::ExpectedHexDigit(4)), parse_char_literal(b"'\\x0'"));
        assert_eq!(Err(Error::ExpectedSingleQuote(5)), parse_char_literal(b"'\\x000'"));
        assert_eq!(Err(Error::InvalidEscapeCharacter(2)), parse_char_literal(b"'\\y'"));
        assert_eq!(Err(Error::ExpectedLbrace(3)), parse_char_literal(b"'\\u'"));
        assert_eq!(Err(Error::ExpectedLbrace(3)), parse_char_literal(b"'\\uFFFF'"));
        assert_eq!(Err(Error::EmptyUnicodeEscapeSequence(4)), parse_char_literal(b"'\\u{}'"));
        assert_eq!(Err(Error::InvalidUnicodeCodepoint(9)), parse_char_literal(b"'\\u{FFFFFF}'"));
        assert_eq!(Err(Error::ExpectedHexDigitOrRbrace(8)), parse_char_literal(b"'\\u{FFFF'"));
        assert_eq!(Err(Error::ExpectedSingleQuote(9)), parse_char_literal(b"'\\u{FFFF}x'"));
        assert_eq!(Err(Error::InvalidCharacter(1)), parse_char_literal(b"'\x00'"));
        assert_eq!(Err(Error::EmptyCharLiteral), parse_char_literal(b"''"));
    }

    #[test]
    fn parse_alloc_decodes_escapes() {
        assert_eq!(Some(b"tab:\tnl:\n".to_vec()), parse_alloc(b"\"tab:\\tnl:\\n\""));
        assert_eq!(Some("A\u{e9}\u{1F600}".as_bytes().to_vec()), parse_alloc(b"\"\\x41\\u{e9}\\u{1F600}\""));
        assert_eq!(Some(vec![0xff]), parse_alloc(b"\"\\xff\""));
        assert_eq!(None, parse_alloc(b"\"\\q\""));
        assert_eq!(None, parse_alloc(b"\"\\u{d800}\""));
    }
}
