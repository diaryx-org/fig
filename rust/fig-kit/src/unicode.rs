//! Port of kit/src/unicode.zig: helper functions for parsing Unicode sequences.
//!
//! A codepoint is Zig's `u21`, here a `u32`.

use crate::{Result, err};

pub fn is_high_surrogate(p: u32) -> bool {
    (0xD800..=0xDBFF).contains(&p)
}

pub fn is_low_surrogate(p: u32) -> bool {
    (0xDC00..=0xDFFF).contains(&p)
}

pub fn is_surrogate(p: u32) -> bool {
    (0xD800..=0xDFFF).contains(&p)
}

/// UTF-8 encode `codepoint` and append it to `out`. Rejects the surrogate range
/// and out-of-range values (anything UTF-8 can't represent) as
/// `error.InvalidCodepoint`. This is the shared tail of the JSON/YAML/TOML
/// `\u`/`\U`/`\x` escape decoders, which differ only in how they *parse* the
/// codepoint, not in how they emit it.
///
/// Errors: `InvalidCodepoint`.
pub fn encode_append(out: &mut Vec<u8>, codepoint: u32) -> Result<()> {
    if is_surrogate(codepoint) {
        return Err(err!(InvalidCodepoint));
    }
    let c = char::from_u32(codepoint).ok_or(err!(InvalidCodepoint))?;
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_append_emits_utf8_and_rejects_surrogates() {
        let mut out = Vec::new();
        encode_append(&mut out, 0x00E9).unwrap(); // é
        encode_append(&mut out, 0x1D11E).unwrap(); // 𝄞
        assert_eq!(out, b"\xc3\xa9\xf0\x9d\x84\x9e");
        assert_eq!(encode_append(&mut out, 0xD800), Err(err!(InvalidCodepoint)));
        // Past U+10FFFF: a u21 holds it, UTF-8 cannot.
        assert_eq!(encode_append(&mut out, 0x110000), Err(err!(InvalidCodepoint)));
        assert_eq!(out, b"\xc3\xa9\xf0\x9d\x84\x9e");
    }

    #[test]
    fn surrogate_ranges() {
        assert!(is_high_surrogate(0xD800) && is_high_surrogate(0xDBFF) && !is_high_surrogate(0xDC00));
        assert!(is_low_surrogate(0xDC00) && is_low_surrogate(0xDFFF) && !is_low_surrogate(0xDBFF));
        assert!(is_surrogate(0xD800) && is_surrogate(0xDFFF) && !is_surrogate(0xE000) && !is_surrogate(0xD7FF));
    }
}
