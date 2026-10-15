//! A string written as a JSON string literal: fig-core's port of the Zig kit's
//! `json_string.zig`, the escaper fig's JSON and canonical printers write
//! with.

use fig_core::kit::json_string as core_json_string;

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
    let mut out = Vec::with_capacity(core_json_string::json_string_room(s.len()));
    let byte = match q {
        Quote::Double => b'"',
        Quote::Single => b'\'',
    };
    core_json_string::write_quoted_in(&mut out, s.as_bytes(), byte);
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
