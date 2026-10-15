//! Port of kit/src/xml.zig: the XML tokenizer. Turns XML bytes into a list of
//! `Token`s.
//!
//! XML is mode-dependent: the same byte means different things in *content*
//! (between tags) versus inside a *tag* (between `<name` and `>`). A space is
//! literal text in content but an attribute separator in a tag; a name is only a
//! name inside a tag. The scanner tracks this with `in_tag`.
//!
//! Constructs with no place in the config-oriented data model are consumed and
//! discarded here, so the parser's grammar stays clean:
//!   * comments `<!-- … -->`, processing instructions / the `<?xml … ?>`
//!     declaration `<? … ?>`, and a `<!DOCTYPE … >` are skipped (emit nothing);
//!   * an internal DTD subset (`<!DOCTYPE … [ … ]>`) is rejected
//!     (`UnsupportedDoctype`) — entity declarations are out of scope for v1.
//!
//! CDATA is preserved as a `cdata` token (its contents are literal text).
//!
//! Entity references (`&amp;`, `&#10;`) inside `char_data`/`attr_value` are left
//! intact in the token span; the parser decodes them.
//!
//! Errors (`TokenizeError`):
//!   * `UnclosedTag` — a start/end tag that never closed before end of input;
//!   * `UnclosedAttributeValue` — an attribute value with no closing quote;
//!   * `UnclosedComment` — a `<!--` with no `-->`;
//!   * `UnclosedCData` — a `<![CDATA[` with no `]]>`;
//!   * `UnclosedPI` — a `<?` with no `?>`;
//!   * `UnsupportedDoctype` — a DOCTYPE with an internal subset `[ … ]`;
//!   * `InvalidMarkup` — markup that isn't well-formed (`<` not followed by a
//!     name/`/`/`!`/`?`, a stray `<!`, a literal `<` inside an attribute
//!     value, …);
//!   * `UnexpectedCharacter` — a byte that may not appear where it does inside
//!     a tag.

use super::span::Span;
use super::token::{self, TokenKind};
use crate::{Result, err};

pub type Token = token::Token<Kind>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// `<` opening an element start-tag (span is just the `<`).
    Lt,
    /// `</` opening an element end-tag.
    LtSlash,
    /// `>` closing a tag.
    Gt,
    /// `/>` closing an empty-element tag.
    SlashGt,
    /// `=` between an attribute name and its value.
    Eq,
    /// An element or attribute name.
    Name,
    /// A quoted attribute value; span covers the bytes *inside* the quotes.
    AttrValue,
    /// A run of character data between tags; span is the raw bytes.
    CharData,
    /// The contents of a `<![CDATA[ … ]]>` section (literal text, no entities).
    Cdata,
    /// End of input.
    Eof,
}

impl TokenKind for Kind {
    /// Length of fixed-width token kinds; `None` for variable-length.
    fn len(self) -> Option<usize> {
        match self {
            Kind::Eof => Some(0),
            Kind::Lt | Kind::Gt | Kind::Eq => Some(1),
            Kind::LtSlash | Kind::SlashGt => Some(2),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Tokenizer<'a> {
    pub str: &'a [u8],
    pub pos: usize,
    /// True while scanning the interior of a start/end tag (after `<name`/`</`,
    /// before the closing `>`/`/>`), where whitespace separates attributes rather
    /// than forming text.
    pub in_tag: bool,
    pub tokens: Vec<Token>,
}

impl<'a> Tokenizer<'a> {
    pub fn new(str: &'a [u8]) -> Tokenizer<'a> {
        Tokenizer { str, pos: 0, in_tag: false, tokens: Vec::new() }
    }

    /// Tokenize the whole input, ending with an `Eof` token. Takes the tokens
    /// out of the tokenizer.
    pub fn tokenize(&mut self) -> Result<Vec<Token>> {
        let r = self.tokenize_inner();
        let tokens = std::mem::take(&mut self.tokens);
        r.map(|()| tokens)
    }

    fn tokenize_inner(&mut self) -> Result<()> {
        while !self.at_end() {
            if self.in_tag {
                self.lex_in_tag()?;
            } else {
                self.lex_content()?;
            }
        }
        if self.in_tag {
            return Err(err!(UnclosedTag));
        }
        self.emit(Kind::Eof, self.pos, self.pos);
        Ok(())
    }

    // ── content mode ────────────────────────────────────────────────────────

    fn lex_content(&mut self) -> Result<()> {
        if self.peek() == b'<' {
            return self.lex_left_angle();
        }
        // A run of character data up to the next `<`.
        let start = self.pos;
        while !self.at_end() && self.peek() != b'<' {
            self.pos += 1;
        }
        self.emit(Kind::CharData, start, self.pos);
        Ok(())
    }

    /// At a `<` in content mode: dispatch on what follows.
    fn lex_left_angle(&mut self) -> Result<()> {
        let start = self.pos;
        if self.starts_with(b"<!--") {
            return self.skip_comment();
        }
        if self.starts_with(b"<![CDATA[") {
            return self.lex_cdata();
        }
        if self.starts_with(b"<!DOCTYPE") {
            return self.skip_doctype();
        }
        if self.starts_with(b"<!") {
            return Err(err!(InvalidMarkup)); // stray DTD-style declaration
        }
        if self.starts_with(b"<?") {
            return self.skip_pi();
        }
        if self.starts_with(b"</") {
            self.pos += 2;
            self.emit(Kind::LtSlash, start, self.pos);
            self.in_tag = true;
            return Ok(());
        }
        // `<` + name-start ⇒ an element start-tag.
        if self.pos + 1 < self.str.len() && is_name_start(self.str[self.pos + 1]) {
            self.pos += 1;
            self.emit(Kind::Lt, start, self.pos);
            self.in_tag = true;
            return Ok(());
        }
        Err(err!(InvalidMarkup))
    }

    fn lex_cdata(&mut self) -> Result<()> {
        self.pos += b"<![CDATA[".len();
        let start = self.pos;
        let idx = index_of(&self.str[self.pos..], b"]]>").ok_or(err!(UnclosedCData))?;
        self.pos += idx;
        self.emit(Kind::Cdata, start, self.pos);
        self.pos += b"]]>".len();
        Ok(())
    }

    fn skip_comment(&mut self) -> Result<()> {
        self.pos += b"<!--".len();
        let idx = index_of(&self.str[self.pos..], b"-->").ok_or(err!(UnclosedComment))?;
        self.pos += idx + b"-->".len();
        Ok(())
    }

    fn skip_pi(&mut self) -> Result<()> {
        self.pos += b"<?".len();
        let idx = index_of(&self.str[self.pos..], b"?>").ok_or(err!(UnclosedPI))?;
        self.pos += idx + b"?>".len();
        Ok(())
    }

    fn skip_doctype(&mut self) -> Result<()> {
        self.pos += b"<!DOCTYPE".len();
        while !self.at_end() {
            match self.peek() {
                b'[' => return Err(err!(UnsupportedDoctype)), // internal subset
                b'>' => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => {}
            }
            self.pos += 1;
        }
        Err(err!(UnclosedTag)) // unterminated DOCTYPE
    }

    // ── tag mode ────────────────────────────────────────────────────────────

    fn lex_in_tag(&mut self) -> Result<()> {
        self.skip_space();
        if self.at_end() {
            return Err(err!(UnclosedTag));
        }
        let c = self.peek();
        match c {
            b'>' => {
                self.emit(Kind::Gt, self.pos, self.pos + 1);
                self.pos += 1;
                self.in_tag = false;
            }
            b'/' => {
                if !self.starts_with(b"/>") {
                    return Err(err!(UnclosedTag));
                }
                self.emit(Kind::SlashGt, self.pos, self.pos + 2);
                self.pos += 2;
                self.in_tag = false;
            }
            b'=' => {
                self.emit(Kind::Eq, self.pos, self.pos + 1);
                self.pos += 1;
            }
            b'"' | b'\'' => self.lex_attr_value(c)?,
            _ => {
                if !is_name_start(c) {
                    return Err(err!(UnexpectedCharacter));
                }
                self.lex_name();
            }
        }
        Ok(())
    }

    fn lex_name(&mut self) {
        let start = self.pos;
        self.pos += 1; // first char is a known name-start
        while !self.at_end() && is_name_char(self.peek()) {
            self.pos += 1;
        }
        self.emit(Kind::Name, start, self.pos);
    }

    fn lex_attr_value(&mut self, quote: u8) -> Result<()> {
        self.pos += 1; // opening quote
        let start = self.pos;
        while !self.at_end() && self.peek() != quote {
            if self.peek() == b'<' {
                return Err(err!(InvalidMarkup)); // `<` is illegal in an attribute value
            }
            self.pos += 1;
        }
        if self.at_end() {
            return Err(err!(UnclosedAttributeValue));
        }
        self.emit(Kind::AttrValue, start, self.pos); // span excludes the quotes
        self.pos += 1; // closing quote
        Ok(())
    }

    // ── helpers ─────────────────────────────────────────────────────────────

    fn at_end(&self) -> bool {
        self.pos >= self.str.len()
    }

    fn peek(&self) -> u8 {
        self.str[self.pos]
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.str[self.pos..].starts_with(prefix)
    }

    fn skip_space(&mut self) {
        while !self.at_end() && is_space(self.peek()) {
            self.pos += 1;
        }
    }

    fn emit(&mut self, kind: Kind, start: usize, end: usize) {
        self.tokens.push(Token::new(kind, Span::new(start, end)));
    }
}

/// `std.mem.indexOf` for a non-empty needle.
fn index_of(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn is_space(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\r' || c == b'\n'
}

/// XML NameStartChar, ASCII subset plus any high byte (so UTF-8 names pass
/// through without strict Unicode-class validation — deferred). `pub`: the
/// printer reuses this exact grammar to validate a mapping key before emitting
/// it as an element/attribute name, so the reader's and writer's notion of
/// "valid Name" can never drift apart.
pub fn is_name_start(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_lowercase() || c == b'_' || c == b':' || c >= 0x80
}

pub fn is_name_char(c: u8) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == b'-' || c == b'.'
}

#[cfg(test)]
mod tests {
    use super::Kind::*;
    use super::*;
    use crate::Error;

    /// Tokenize `src` and return just the kind sequence (for terse assertions).
    fn kinds_of(src: &[u8]) -> core::result::Result<Vec<Kind>, Error> {
        let mut t = Tokenizer::new(src);
        Ok(t.tokenize()?.iter().map(|tok| tok.kind).collect())
    }

    fn expect_kinds(src: &[u8], expected: &[Kind]) {
        assert_eq!(kinds_of(src).unwrap(), expected);
    }

    #[test]
    fn empty_element() {
        expect_kinds(b"<a/>", &[Lt, Name, SlashGt, Eof]);
    }

    #[test]
    fn element_with_text() {
        expect_kinds(b"<a>hi</a>", &[Lt, Name, Gt, CharData, LtSlash, Name, Gt, Eof]);
    }

    #[test]
    fn attributes_both_quote_styles() {
        expect_kinds(b"<a x=\"1\" y='2'>", &[Lt, Name, Name, Eq, AttrValue, Name, Eq, AttrValue, Gt, Eof]);
    }

    #[test]
    fn attr_value_span_excludes_quotes() {
        let src = b"<a x=\"hello\">";
        let mut t = Tokenizer::new(src);
        let toks = t.tokenize().unwrap();
        // toks: lt, name(a), name(x), eq, attr_value(hello), gt, eof
        assert_eq!(toks[4].kind, AttrValue);
        assert_eq!(toks[4].source(src), b"hello");
    }

    #[test]
    fn comment_is_skipped() {
        expect_kinds(b"<a><!--c--></a>", &[Lt, Name, Gt, LtSlash, Name, Gt, Eof]);
    }

    #[test]
    fn processing_instruction_declaration_skipped() {
        expect_kinds(b"<?xml version=\"1.0\"?><a/>", &[Lt, Name, SlashGt, Eof]);
    }

    #[test]
    fn doctype_skipped() {
        expect_kinds(b"<!DOCTYPE a><a/>", &[Lt, Name, SlashGt, Eof]);
    }

    #[test]
    fn cdata_preserved_as_one_token_may_contain_lt() {
        let src = b"<a><![CDATA[x<y]]></a>";
        let toks = Tokenizer::new(src).tokenize().unwrap();
        assert_eq!(toks[3].kind, Cdata);
        assert_eq!(toks[3].source(src), b"x<y");
    }

    #[test]
    fn entity_left_intact_in_char_data() {
        let src = b"<a>x&amp;y</a>";
        let toks = Tokenizer::new(src).tokenize().unwrap();
        assert_eq!(toks[3].source(src), b"x&amp;y");
    }

    #[test]
    fn errors() {
        assert_eq!(kinds_of(b"<!DOCTYPE a [ <!ENTITY x \"y\"> ]><a/>"), Err(err!(UnsupportedDoctype)));
        assert_eq!(kinds_of(b"<a><!--unterminated"), Err(err!(UnclosedComment)));
        assert_eq!(kinds_of(b"<a><![CDATA[oops"), Err(err!(UnclosedCData)));
        assert_eq!(kinds_of(b"<?pi never"), Err(err!(UnclosedPI)));
        assert_eq!(kinds_of(b"<a x=\"oops"), Err(err!(UnclosedAttributeValue)));
        assert_eq!(kinds_of(b"<a"), Err(err!(UnclosedTag)));
        assert_eq!(kinds_of(b"< a/>"), Err(err!(InvalidMarkup)));
        assert_eq!(kinds_of(b"<a b=\"<\"/>"), Err(err!(InvalidMarkup)));
    }

    #[test]
    fn more_errors_and_spans() {
        assert_eq!(kinds_of(b"<!ELEMENT a>"), Err(err!(InvalidMarkup)));
        assert_eq!(kinds_of(b"<!DOCTYPE a"), Err(err!(UnclosedTag)));
        assert_eq!(kinds_of(b"<a / >"), Err(err!(UnclosedTag)));
        assert_eq!(kinds_of(b"<a 1>"), Err(err!(UnexpectedCharacter)));
        assert_eq!(kinds_of(b"<"), Err(err!(InvalidMarkup)));
        expect_kinds(b"", &[Eof]);
        let src = b"<x:a-1.b \xc3\xa9='v'/>tail";
        let toks = Tokenizer::new(src).tokenize().unwrap();
        assert_eq!(toks.iter().map(|t| t.kind).collect::<Vec<_>>(), [Lt, Name, Name, Eq, AttrValue, SlashGt, CharData, Eof]);
        assert_eq!(toks[1].source(src), b"x:a-1.b");
        assert_eq!(toks[2].source(src), b"\xc3\xa9");
        assert_eq!(toks[4].span, Span::new(13, 14));
        assert_eq!(toks[5].span, Span::new(15, 17));
        assert_eq!(toks[7].span, Span::new(src.len(), src.len()));
        assert_eq!(Kind::LtSlash.len(), Some(2));
        assert_eq!(Kind::Name.len(), None);
    }
}
