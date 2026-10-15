//! Port of languages/json/src/tokenizer.zig.
//!
//! JSON Tokenizer. Turns a `&[u8]` in JSON format into Tokens, one at a
//! time (`next`, which the parser pulls and allocates nothing) or as a slice
//! (`tokenize`).

use super::Type as JsonFormat;
use crate::kit::ascii;
use crate::kit::span::Span;
use crate::kit::token::{self, TokenKind};
use crate::{err, Result};

pub type Token = token::Token<Kind>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    // Structural
    /// {
    OpenBrace,
    /// }
    CloseBrace,
    /// [
    OpenBracket,
    /// ]
    CloseBracket,

    Colon,
    Comma,
    EndOfFile,

    // Literals
    True,
    False,
    Null,

    // variable-length
    String,
    Number,
    Comment,
    Whitespace,
    /// JSON5 only: an unquoted ECMAScript IdentifierName. Used as an object
    /// key, or as a value when it spells `Infinity` / `NaN`.
    Identifier,
}

impl TokenKind for Kind {
    /// Find length of token kind. Returns `None` for variable-length tokens.
    fn len(self) -> Option<usize> {
        match self {
            Kind::EndOfFile => Some(0),
            Kind::OpenBrace | Kind::CloseBrace | Kind::OpenBracket | Kind::CloseBracket | Kind::Colon | Kind::Comma => Some(1),
            Kind::True | Kind::Null => Some(4),
            Kind::False => Some(5),
            _ => None,
        }
    }
}

/// The tokenizer's error names (Zig's `TokenizeError`), which the parser
/// folds into its own `Error` set: `UnexpectedToken`, `MissingToken`,
/// `OutOfMemory`, `UnexpectedSlash`, `MissingCloseBrace`,
/// `MissingOpenQuote`, `MissingColon`, `MissingCloseBracket`,
/// `LeadingZero`, `UnclosedString`, `UnexpectedEndOfInput`,
/// `UnclosedComment`.
pub const TOKENIZE_ERRORS: &[&str] = &[
    "UnexpectedToken",
    "MissingToken",
    "OutOfMemory",
    "UnexpectedSlash",
    "MissingCloseBrace",
    "MissingOpenQuote",
    "MissingColon",
    "MissingCloseBracket",
    "LeadingZero",
    "UnclosedString",
    "UnexpectedEndOfInput",
    "UnclosedComment",
];

#[derive(Debug, Clone)]
pub struct Tokenizer<'a> {
    // State
    pub index: usize,
    /// Whether `next` has stepped over a leading byte-order mark yet.
    pub begun: bool,

    // Initial fields
    pub str: &'a [u8],
    pub kind: JsonFormat,
}

impl<'a> Tokenizer<'a> {
    pub fn new(str: &'a [u8], kind: JsonFormat) -> Tokenizer<'a> {
        Tokenizer { index: 0, begun: false, str, kind }
    }

    /// Every token of the input, through `EndOfFile`. The parser pulls them
    /// one at a time with `next` instead.
    pub fn tokenize(&mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::with_capacity(self.str.len() + 1);
        loop {
            let token = self.next()?;
            tokens.push(token);
            if token.kind == Kind::EndOfFile {
                break;
            }
        }
        Ok(tokens)
    }

    /// The next token: the sequence `tokenize` returns, one at a time,
    /// through `EndOfFile` — which every later call returns again. After an
    /// error `index` is where it was found.
    pub fn next(&mut self) -> Result<Token> {
        if !self.begun {
            self.begun = true;
            if self.str.starts_with(b"\xEF\xBB\xBF") {
                self.index = 3;
            }
        }
        let token = self.scan()?;
        self.index = token.span.end;
        Ok(token)
    }

    fn scan(&mut self) -> Result<Token> {
        let Some(c) = self.char() else { return Ok(Token::fixed(Kind::EndOfFile, self.index)) };
        // JSON5 widens the alphabet: unquoted identifier keys (and the bare
        // `Infinity`/`NaN` values), single-quoted strings, a leading `+` or
        // `.` on numbers, and the extra ES whitespace (vertical tab, form
        // feed).
        if self.kind == JsonFormat::JSON5 {
            if is_ident_start(c) {
                return Ok(self.identifier_or_keyword());
            }
            match c {
                b'\'' => return self.string(b'\''),
                b'+' | b'.' => return self.number(),
                0x0b | 0x0c => return Ok(self.get_whitespace()),
                _ => {}
            }
        }
        let at = self.index;
        match c {
            b'{' => Ok(Token::new(Kind::OpenBrace, Span::new(at, at + 1))),
            b'}' => Ok(Token::new(Kind::CloseBrace, Span::new(at, at + 1))),
            b'[' => Ok(Token::new(Kind::OpenBracket, Span::new(at, at + 1))),
            b']' => Ok(Token::new(Kind::CloseBracket, Span::new(at, at + 1))),
            b':' => Ok(Token::new(Kind::Colon, Span::new(at, at + 1))),
            b',' => Ok(Token::new(Kind::Comma, Span::new(at, at + 1))),
            b't' => self.find_literal(Kind::True),
            b'f' => self.find_literal(Kind::False),
            b'n' => self.find_literal(Kind::Null),
            b'"' => self.string(b'"'),
            b'/' => self.comment(),
            b'0'..=b'9' | b'-' => self.number(),
            b' ' | b'\t' | b'\n' | b'\r' => Ok(self.get_whitespace()),
            _ => Err(err!(UnexpectedToken)),
        }
    }

    fn find_literal(&self, kind: Kind) -> Result<Token> {
        match kind {
            Kind::Null => {
                if self.matches(b"null") {
                    return Ok(Token::fixed(Kind::Null, self.index));
                }
            }
            Kind::True => {
                if self.matches(b"true") {
                    return Ok(Token::fixed(Kind::True, self.index));
                }
            }
            Kind::False => {
                if self.matches(b"false") {
                    return Ok(Token::fixed(Kind::False, self.index));
                }
            }
            _ => return Err(err!(UnexpectedToken)),
        }
        Err(err!(UnexpectedToken))
    }

    /// Collects all whitespace and returns it as a token.
    fn get_whitespace(&mut self) -> Token {
        let start = self.index;
        while let Some(c) = self.char() {
            if !is_whitespace(c) {
                break;
            }
            self.index += 1;
        }
        let end = self.index;
        assert!(start != end, "unreachable");
        Token::new(Kind::Whitespace, Span::new(start, end))
    }

    // =====================
    // CONVENIENCE FUNCTIONS
    // =====================

    /// Convenience function for accessing current character
    fn char(&self) -> Option<u8> {
        self.str.get(self.index).copied()
    }

    /// Checks if the index is on a given sequence of characters.
    fn matches(&self, str: &[u8]) -> bool {
        if str.len() > self.str.len() - self.index {
            return false;
        }
        &self.str[self.index..self.index + str.len()] == str
    }

    /// Scan a run of identifier characters, classifying the three reserved
    /// value keywords (`true`/`false`/`null`) into their dedicated tokens
    /// and everything else (`Infinity`, `NaN`, and bare object keys) into an
    /// `Identifier` token.
    fn identifier_or_keyword(&mut self) -> Token {
        let start = self.index;
        while let Some(c) = self.char() {
            if !is_ident_part(c) {
                break;
            }
            self.index += 1;
        }
        let word = &self.str[start..self.index];
        let kind = match word {
            b"true" => Kind::True,
            b"false" => Kind::False,
            b"null" => Kind::Null,
            _ => Kind::Identifier,
        };
        Token::new(kind, Span::new(start, self.index))
    }

    // ========================
    // TERMINAL TOKEN FUNCTIONS
    // ========================

    /// Collects all the bytes of a string and returns a `String` token.
    /// Can be an empty string. Respects escaped values.
    fn string(&mut self, delimiter: u8) -> Result<Token> {
        let json5 = self.kind == JsonFormat::JSON5;
        let start = self.index;
        self.index += 1; // skip opening quote

        while let Some(c) = self.char() {
            if c == delimiter {
                // Closing quote
                self.index += 1;
                return Ok(Token::new(Kind::String, Span::new(start, self.index)));
            }
            match c {
                b'\\' => {
                    // Escape a character
                    self.index += 1; // skip backslash
                    let escaped = self.char().ok_or(err!(UnclosedString))?;
                    if json5 {
                        // JSON5 accepts any escape: the JSON set, plus `\x`,
                        // `\v`, `\0`, identity escapes (`\'`), and line
                        // continuations (`\` + LF/CR/CRLF). Validation and
                        // decoding are deferred to the parser; here we only
                        // need to consume the escaped unit so a `\'` or
                        // `\<newline>` does not terminate the string.
                        self.index += 1;
                        if escaped == b'\r' && self.char() == Some(b'\n') {
                            self.index += 1;
                        }
                        continue;
                    }
                    match escaped {
                        // Valid escapes: quote, backslash, whitespace
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                            self.index += 1;
                        }
                        // Unicode escape. \u<four hex characters>
                        b'u' => {
                            self.index += 1;
                            for _ in 0..4 {
                                let hex = self.char().ok_or(err!(UnclosedString))?;
                                if !ascii::is_hex(hex) {
                                    return Err(err!(UnexpectedToken));
                                }
                                self.index += 1;
                            }
                        }
                        _ => return Err(err!(UnexpectedToken)),
                    }
                }
                0x00..=0x1f => return Err(err!(UnexpectedToken)),
                _ => self.index += 1,
            }
        }
        Err(err!(UnclosedString))
    }

    /// Collects various kinds of numbers: negative, decimal, exponent.
    /// Checks for leading zero as well.
    fn number(&mut self) -> Result<Token> {
        if self.kind == JsonFormat::JSON5 {
            return self.number_json5();
        }
        let start = self.index;

        // Check for negativity
        if self.char() == Some(b'-') {
            self.index += 1;
        }

        match self.char().ok_or(err!(UnexpectedEndOfInput))? {
            // Either zero, or illegal leading zero
            b'0' => {
                self.index += 1;
                if let Some(c) = self.char() {
                    if ascii::is_digit(c) {
                        return Err(err!(LeadingZero));
                    }
                }
            }
            b'1'..=b'9' => {
                self.index += 1;
                while let Some(c) = self.char() {
                    if !ascii::is_digit(c) {
                        break;
                    }
                    self.index += 1;
                }
            }
            _ => return Err(err!(UnexpectedToken)),
        }

        // Check for decimal
        if self.char() == Some(b'.') {
            self.index += 1;
            let first_fraction = self.char().ok_or(err!(UnexpectedEndOfInput))?;
            if !ascii::is_digit(first_fraction) {
                return Err(err!(UnexpectedToken));
            }
            while let Some(c) = self.char() {
                if !ascii::is_digit(c) {
                    break;
                }
                self.index += 1;
            }
        }

        // Check for exponent
        if let Some(c) = self.char() {
            if c == b'e' || c == b'E' {
                self.index += 1;
                if let Some(sign) = self.char() {
                    if sign == b'+' || sign == b'-' {
                        self.index += 1;
                    }
                }
                let first_exponent = self.char().ok_or(err!(UnexpectedEndOfInput))?;
                if !ascii::is_digit(first_exponent) {
                    return Err(err!(UnexpectedToken));
                }
                while let Some(digit) = self.char() {
                    if !ascii::is_digit(digit) {
                        break;
                    }
                    self.index += 1;
                }
            }
        }

        Ok(Token::new(Kind::Number, Span::new(start, self.index)))
    }

    /// JSON5 number: optional `+`/`-` sign, then one of
    ///   * `Infinity` / `NaN`              (non-finite; the parser lifts these
    ///                                      to an extended `NumberSpecial`)
    ///   * `0x`-prefixed hexadecimal integer
    ///   * a decimal with an optional leading or trailing `.` and optional
    ///     exponent
    ///
    /// Leading zeros on a decimal integer stay illegal (no octal), matching
    /// JSON.
    fn number_json5(&mut self) -> Result<Token> {
        let start = self.index;

        if self.char() == Some(b'+') || self.char() == Some(b'-') {
            self.index += 1;
        }

        if self.matches(b"Infinity") {
            self.index += b"Infinity".len();
            return Ok(Token::new(Kind::Number, Span::new(start, self.index)));
        }
        if self.matches(b"NaN") {
            self.index += b"NaN".len();
            return Ok(Token::new(Kind::Number, Span::new(start, self.index)));
        }

        // Hexadecimal integer: `0x`/`0X` then at least one hex digit.
        if self.char() == Some(b'0') && (self.peek(1) == Some(b'x') || self.peek(1) == Some(b'X')) {
            self.index += 2;
            let hex_start = self.index;
            while let Some(c) = self.char() {
                if !ascii::is_hex(c) {
                    break;
                }
                self.index += 1;
            }
            if self.index == hex_start {
                return Err(err!(UnexpectedToken)); // bare `0x`
            }
            return Ok(Token::new(Kind::Number, Span::new(start, self.index)));
        }

        let mut digits_seen = false;

        // Integer part. A leading zero may not be followed by another digit
        // (octal).
        if self.char() == Some(b'0') {
            self.index += 1;
            digits_seen = true;
            if let Some(c) = self.char() {
                if ascii::is_digit(c) {
                    return Err(err!(LeadingZero));
                }
            }
        } else {
            while let Some(c) = self.char() {
                if !ascii::is_digit(c) {
                    break;
                }
                self.index += 1;
                digits_seen = true;
            }
        }

        // Fractional part. Leading (`.5`) and trailing (`5.`) points are both
        // legal.
        if self.char() == Some(b'.') {
            self.index += 1;
            while let Some(c) = self.char() {
                if !ascii::is_digit(c) {
                    break;
                }
                self.index += 1;
                digits_seen = true;
            }
        }

        // A lone `.` (or sign with no digits at all) is not a number.
        if !digits_seen {
            return Err(err!(UnexpectedToken));
        }

        // Exponent.
        if let Some(c) = self.char() {
            if c == b'e' || c == b'E' {
                self.index += 1;
                if let Some(sign) = self.char() {
                    if sign == b'+' || sign == b'-' {
                        self.index += 1;
                    }
                }
                let exp_start = self.index;
                while let Some(d) = self.char() {
                    if !ascii::is_digit(d) {
                        break;
                    }
                    self.index += 1;
                }
                if self.index == exp_start {
                    return Err(err!(UnexpectedEndOfInput));
                }
            }
        }

        Ok(Token::new(Kind::Number, Span::new(start, self.index)))
    }

    /// Look ahead `n` bytes from the current index without consuming.
    fn peek(&self, n: usize) -> Option<u8> {
        self.str.get(self.index + n).copied()
    }

    /// Collects all bytes until arriving at a newline (a line comment), or
    /// through `*/` (a block comment).
    fn comment(&mut self) -> Result<Token> {
        // Comments are not supported in the canonical JSON format
        if self.kind == JsonFormat::JSON {
            return Err(err!(UnexpectedSlash));
        }
        // Make sure there isn't just a random single slash
        if self.index + 1 >= self.str.len() {
            return Err(err!(UnexpectedSlash));
        }

        let start = self.index;
        let second = self.str[self.index + 1];

        match second {
            b'/' => {
                // Single line comment
                self.index += 2;
                while let Some(c) = self.char() {
                    // A line comment ends at any line terminator. JSON5/ES
                    // allow a bare CR (and CRLF) to close it, not just LF.
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.index += 1;
                }
                Ok(Token::new(Kind::Comment, Span::new(start, self.index)))
            }
            b'*' => {
                // Multi-line comment
                self.index += 2;
                while self.index + 1 < self.str.len() {
                    if self.str[self.index] == b'*' && self.str[self.index + 1] == b'/' {
                        self.index += 2;
                        return Ok(Token::new(Kind::Comment, Span::new(start, self.index)));
                    }
                    self.index += 1;
                }
                Err(err!(UnclosedComment))
            }
            _ => Err(err!(UnexpectedSlash)),
        }
    }
}

/// Zig's `std.ascii.isWhitespace`: space, `\t`, `\n`, `\r`, vertical tab,
/// form feed.
fn is_whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// ASCII subset of an ECMAScript IdentifierStart (`$`, `_`, letters).
/// Unicode and `\u` escapes in identifiers are out of scope (the suite's
/// `todo/` cases).
fn is_ident_start(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_uppercase() || c == b'_' || c == b'$'
}

fn is_ident_part(c: u8) -> bool {
    is_ident_start(c) || ascii::is_digit(c)
}

// =======
// Testing
// =======

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    fn tok(kind: Kind, start: usize, end: usize) -> Token {
        Token::new(kind, Span::new(start, end))
    }

    fn test_tokenizer(input: &[u8], expected: &[Token]) {
        // Zig's default `kind` is `.JSONC`.
        let mut tokenizer = Tokenizer::new(input, JsonFormat::JSONC);
        let tokens = tokenizer.tokenize().unwrap();
        assert_eq!(expected, &tokens[..]);
    }

    fn test_tokenizer_error(input: &[u8], format: JsonFormat, expected_error: Error) {
        let mut tokenizer = Tokenizer::new(input, format);
        assert_eq!(Err(expected_error), tokenizer.tokenize());
    }

    use Kind::*;

    #[test]
    fn array_no_whitespace() {
        test_tokenizer(
            br#"["hello","there"]"#,
            &[
                tok(OpenBracket, 0, 1),
                tok(String, 1, 8),
                tok(Comma, 8, 9),
                tok(String, 9, 16),
                tok(CloseBracket, 16, 17),
                tok(EndOfFile, 17, 17),
            ],
        );
    }

    #[test]
    fn whitespace() {
        test_tokenizer(
            b" [ \"hello\" ,  \"there\" ] ",
            &[
                tok(Whitespace, 0, 1),
                tok(OpenBracket, 1, 2),
                tok(Whitespace, 2, 3),
                tok(String, 3, 10),
                tok(Whitespace, 10, 11),
                tok(Comma, 11, 12),
                tok(Whitespace, 12, 14),
                tok(String, 14, 21),
                tok(Whitespace, 21, 22),
                tok(CloseBracket, 22, 23),
                tok(Whitespace, 23, 24),
                tok(EndOfFile, 24, 24),
            ],
        );
        test_tokenizer(
            b" { \"hello\" :  \"there\" } ",
            &[
                tok(Whitespace, 0, 1),
                tok(OpenBrace, 1, 2),
                tok(Whitespace, 2, 3),
                tok(String, 3, 10),
                tok(Whitespace, 10, 11),
                tok(Colon, 11, 12),
                tok(Whitespace, 12, 14),
                tok(String, 14, 21),
                tok(Whitespace, 21, 22),
                tok(CloseBrace, 22, 23),
                tok(Whitespace, 23, 24),
                tok(EndOfFile, 24, 24),
            ],
        );
    }

    #[test]
    fn object_with_array() {
        test_tokenizer(
            br#"{"array": ["hello" ,  "there"]}"#,
            &[
                tok(OpenBrace, 0, 1),
                tok(String, 1, 8),
                tok(Colon, 8, 9),
                tok(Whitespace, 9, 10),
                tok(OpenBracket, 10, 11),
                tok(String, 11, 18),
                tok(Whitespace, 18, 19),
                tok(Comma, 19, 20),
                tok(Whitespace, 20, 22),
                tok(String, 22, 29),
                tok(CloseBracket, 29, 30),
                tok(CloseBrace, 30, 31),
                tok(EndOfFile, 31, 31),
            ],
        );
    }

    #[test]
    fn primitives() {
        test_tokenizer(
            br#"[true, false, null, "string", 40334]"#,
            &[
                tok(OpenBracket, 0, 1),
                tok(True, 1, 5),
                tok(Comma, 5, 6),
                tok(Whitespace, 6, 7),
                tok(False, 7, 12),
                tok(Comma, 12, 13),
                tok(Whitespace, 13, 14),
                tok(Null, 14, 18),
                tok(Comma, 18, 19),
                tok(Whitespace, 19, 20),
                tok(String, 20, 28),
                tok(Comma, 28, 29),
                tok(Whitespace, 29, 30),
                tok(Number, 30, 35),
                tok(CloseBracket, 35, 36),
                tok(EndOfFile, 36, 36),
            ],
        );
    }

    #[test]
    fn numbers() {
        test_tokenizer(
            b"[1,-1,0,0.2,12e+3,1e10,0e1,-0.2,1e+10]",
            &[
                tok(OpenBracket, 0, 1),
                tok(Number, 1, 2),
                tok(Comma, 2, 3),
                tok(Number, 3, 5),
                tok(Comma, 5, 6),
                tok(Number, 6, 7),
                tok(Comma, 7, 8),
                tok(Number, 8, 11),
                tok(Comma, 11, 12),
                tok(Number, 12, 17),
                tok(Comma, 17, 18),
                tok(Number, 18, 22),
                tok(Comma, 22, 23),
                tok(Number, 23, 26),
                tok(Comma, 26, 27),
                tok(Number, 27, 31),
                tok(Comma, 31, 32),
                tok(Number, 32, 37),
                tok(CloseBracket, 37, 38),
                tok(EndOfFile, 38, 38),
            ],
        );
    }

    #[test]
    fn empty_object_array() {
        test_tokenizer(b"[]", &[tok(OpenBracket, 0, 1), tok(CloseBracket, 1, 2), tok(EndOfFile, 2, 2)]);
        test_tokenizer(b"{}", &[tok(OpenBrace, 0, 1), tok(CloseBrace, 1, 2), tok(EndOfFile, 2, 2)]);
    }

    #[test]
    fn truncated_literals_are_rejected() {
        test_tokenizer_error(b"tru", JsonFormat::JSONC, err!(UnexpectedToken));
        test_tokenizer_error(b"fals", JsonFormat::JSONC, err!(UnexpectedToken));
        test_tokenizer_error(b"n", JsonFormat::JSONC, err!(UnexpectedToken));
    }

    #[test]
    fn strings_reject_invalid_json_escapes_and_control_bytes() {
        test_tokenizer_error(b"\"\\x\"", JsonFormat::JSONC, err!(UnexpectedToken));
        test_tokenizer_error(b"\"line\nbreak\"", JsonFormat::JSONC, err!(UnexpectedToken));
        test_tokenizer_error(b"\"\\u12", JsonFormat::JSONC, err!(UnclosedString));
        test_tokenizer_error(b"\"\\u12g4\"", JsonFormat::JSONC, err!(UnexpectedToken));
    }

    #[test]
    fn strict_json_numbers_reject_invalid_forms() {
        test_tokenizer_error(b"-", JsonFormat::JSONC, err!(UnexpectedEndOfInput));
        test_tokenizer_error(b"-.2", JsonFormat::JSONC, err!(UnexpectedToken));
        test_tokenizer_error(b"1.", JsonFormat::JSONC, err!(UnexpectedEndOfInput));
        test_tokenizer_error(b"1e+", JsonFormat::JSONC, err!(UnexpectedEndOfInput));
        test_tokenizer_error(b"-012", JsonFormat::JSONC, err!(LeadingZero));
        test_tokenizer_error(b"012", JsonFormat::JSONC, err!(LeadingZero));
    }

    #[test]
    fn jsonc_comments() {
        test_tokenizer(
            b"[// hi\n1]",
            &[
                tok(OpenBracket, 0, 1),
                tok(Comment, 1, 6),
                tok(Whitespace, 6, 7),
                tok(Number, 7, 8),
                tok(CloseBracket, 8, 9),
                tok(EndOfFile, 9, 9),
            ],
        );

        test_tokenizer_error(b"/", JsonFormat::JSONC, err!(UnexpectedSlash));
        // multiline test
        test_tokenizer(b"/* hi */", &[tok(Comment, 0, 8), tok(EndOfFile, 8, 8)]);
        // multiline inline test
        test_tokenizer(
            br#"{"hello":/* hi */"world"}"#,
            &[
                tok(OpenBrace, 0, 1),
                tok(String, 1, 8),
                tok(Colon, 8, 9),
                tok(Comment, 9, 17),
                tok(String, 17, 24),
                tok(CloseBrace, 24, 25),
                tok(EndOfFile, 25, 25),
            ],
        );
        // multiline comment test
        test_tokenizer(
            b"{\n  \"hello\": \"world\"\n/*\nhello, this is  a\nmultiline comment\n*/\n}",
            &[
                tok(OpenBrace, 0, 1),
                tok(Whitespace, 1, 4),
                tok(String, 4, 11),
                tok(Colon, 11, 12),
                tok(Whitespace, 12, 13),
                tok(String, 13, 20),
                tok(Whitespace, 20, 21),
                tok(Comment, 21, 62),
                tok(Whitespace, 62, 63),
                tok(CloseBrace, 63, 64),
                tok(EndOfFile, 64, 64),
            ],
        );
        test_tokenizer_error(b"// hi", JsonFormat::JSON, err!(UnexpectedSlash));
    }
}
