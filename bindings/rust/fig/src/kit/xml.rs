//! The tokens of an XML-shaped format: fig-core's port of the Zig kit's `xml.zig`, the
//! tokenizer fig's plist reads with. It is lexical and knows nothing of
//! which elements mean what. Comments, processing instructions (the
//! `<?xml …?>` declaration among them) and a DOCTYPE are skipped; a DOCTYPE
//! with an internal subset is refused. Entity references are left in the
//! spans for the language to decode.

use fig_core::kit::xml as core_xml;

use crate::Span;

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TokenKind {
    /// `<` opening a start tag.
    Lt,
    /// `</` opening an end tag.
    LtSlash,
    /// `>` closing a tag.
    Gt,
    /// `/>` closing an empty element.
    SlashGt,
    /// `=` between an attribute's name and its value.
    Eq,
    /// An element or attribute name.
    Name,
    /// A quoted attribute value; its span is inside the quotes.
    AttrValue,
    /// A run of character data between tags, as written.
    CharData,
    /// A CDATA section's contents, inside its markers.
    CData,
    /// The end of the input.
    Eof,
}

impl TokenKind {
    fn from_core(k: core_xml::Kind) -> TokenKind {
        match k {
            core_xml::Kind::Lt => TokenKind::Lt,
            core_xml::Kind::LtSlash => TokenKind::LtSlash,
            core_xml::Kind::Gt => TokenKind::Gt,
            core_xml::Kind::SlashGt => TokenKind::SlashGt,
            core_xml::Kind::Eq => TokenKind::Eq,
            core_xml::Kind::Name => TokenKind::Name,
            core_xml::Kind::AttrValue => TokenKind::AttrValue,
            core_xml::Kind::CharData => TokenKind::CharData,
            core_xml::Kind::Cdata => TokenKind::CData,
            core_xml::Kind::Eof => TokenKind::Eof,
        }
    }
}

/// One token: its kind and its byte span in the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Why an input was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    UnclosedTag,
    UnclosedAttributeValue,
    UnclosedComment,
    UnclosedCData,
    UnclosedPi,
    /// A DOCTYPE with an internal subset.
    UnsupportedDoctype,
    /// `<` with nothing markup after it, a `<` in an attribute value, …
    InvalidMarkup,
    /// A byte a tag may not hold where it is.
    UnexpectedCharacter,
    OutOfMemory,
}

/// A refusal, and the byte offset where the tokenizer stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Error {
    pub kind: ErrorKind,
    pub at: usize,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.kind {
            ErrorKind::UnclosedTag => "a tag is not closed",
            ErrorKind::UnclosedAttributeValue => "an attribute value has no closing quote",
            ErrorKind::UnclosedComment => "a comment has no `-->`",
            ErrorKind::UnclosedCData => "a CDATA section has no `]]>`",
            ErrorKind::UnclosedPi => "a processing instruction has no `?>`",
            ErrorKind::UnsupportedDoctype => "a DOCTYPE with an internal subset is not supported",
            ErrorKind::InvalidMarkup => "markup is not well-formed here",
            ErrorKind::UnexpectedCharacter => "unexpected character inside a tag",
            ErrorKind::OutOfMemory => "out of memory",
        };
        write!(f, "{what} (at byte {})", self.at)
    }
}

impl std::error::Error for Error {}

/// `input`'s tokens, the last [`TokenKind::Eof`], or why it is refused.
pub fn tokenize(input: &[u8]) -> Result<Vec<Token>, Error> {
    // The C call refuses an input past `u32` offsets as it refuses one
    // out of memory.
    if input.len() > u32::MAX as usize {
        return Err(Error {
            kind: ErrorKind::OutOfMemory,
            at: 0,
        });
    }
    let mut tz = core_xml::Tokenizer::new(input);
    match tz.tokenize() {
        Ok(tokens) => Ok(tokens
            .iter()
            .map(|t| Token {
                kind: TokenKind::from_core(t.kind),
                span: Span {
                    start: t.span.start,
                    end: t.span.end,
                },
            })
            .collect()),
        Err(e) => {
            let kind = match e.name() {
                "UnclosedTag" => ErrorKind::UnclosedTag,
                "UnclosedAttributeValue" => ErrorKind::UnclosedAttributeValue,
                "UnclosedComment" => ErrorKind::UnclosedComment,
                "UnclosedCData" => ErrorKind::UnclosedCData,
                "UnclosedPI" => ErrorKind::UnclosedPi,
                "UnsupportedDoctype" => ErrorKind::UnsupportedDoctype,
                "InvalidMarkup" => ErrorKind::InvalidMarkup,
                "UnexpectedCharacter" => ErrorKind::UnexpectedCharacter,
                _ => ErrorKind::OutOfMemory,
            };
            Err(Error { kind, at: tz.pos })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_refusals() {
        let t = tokenize(b"<a>x</a>").unwrap();
        assert_eq!(t.len(), 8);
        assert_eq!(t[3].kind, TokenKind::CharData);
        assert_eq!(t[3].span, Span { start: 3, end: 4 });
        assert_eq!(
            tokenize(b"<a").unwrap_err(),
            Error {
                kind: ErrorKind::UnclosedTag,
                at: 2
            }
        );
    }
}
