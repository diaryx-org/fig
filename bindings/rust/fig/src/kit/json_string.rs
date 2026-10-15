//! A string written as a JSON string literal: the Zig kit's
//! `json_string.zig`, the escaper fig's JSON and canonical printers write
//! with.

use crate::ffi;

/// The quote a JSON string is written between.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Quote {
    /// `"`, as JSON writes every string.
    #[default]
    Double,
    /// `'`, which JSON5 also takes: it escapes `'` and lets `"` through.
    Single,
}

/// `s` between two quotes, escaped as JSON escapes it: `\\`, the quote,
/// `\b \f \n \r \t`, and `\u00XX` for any other control character. Every
/// other character, non-ASCII included, is written as it is.
pub fn quote(s: &str, q: Quote) -> String {
    let mut out = vec![0u8; ffi::fig_kit_json_string_room(s.len())];
    let byte = match q {
        Quote::Double => b'"',
        Quote::Single => b'\'',
    };
    // SAFETY: `out` has the room `fig.h` promises is always enough, and the
    // function reads `s.len()` bytes at `s`.
    let n = unsafe {
        ffi::fig_kit_json_string_quote(s.as_ptr(), s.len(), byte, out.as_mut_ptr(), out.len())
    };
    assert!(n <= out.len(), "fig_kit_json_string_quote outgrew its room");
    out.truncate(n);
    // `s`'s own characters, ASCII escapes and two ASCII quotes.
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes() {
        assert_eq!(quote("a\"b\n", Quote::Double), "\"a\\\"b\\n\"");
        assert_eq!(quote("it's", Quote::Single), "'it\\'s'");
        assert_eq!(quote("é", Quote::Double), "\"é\"");
    }
}
