//! The tokens of an XML-shaped format: the Zig kit's `xml.zig`, the
//! tokenizer fig's plist reads with. It is lexical and knows nothing of
//! which elements mean what. Comments, processing instructions (the
//! `<?xml …?>` declaration among them) and a DOCTYPE are skipped; a DOCTYPE
//! with an internal subset is refused. Entity references are left in the
//! spans for the language to decode.

use crate::Span;
use crate::ffi;

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
    fn from_c(k: u8) -> TokenKind {
        match k {
            ffi::FIG_XML_LT => TokenKind::Lt,
            ffi::FIG_XML_LT_SLASH => TokenKind::LtSlash,
            ffi::FIG_XML_GT => TokenKind::Gt,
            ffi::FIG_XML_SLASH_GT => TokenKind::SlashGt,
            ffi::FIG_XML_EQ => TokenKind::Eq,
            ffi::FIG_XML_NAME => TokenKind::Name,
            ffi::FIG_XML_ATTR_VALUE => TokenKind::AttrValue,
            ffi::FIG_XML_CHAR_DATA => TokenKind::CharData,
            ffi::FIG_XML_CDATA => TokenKind::CData,
            _ => TokenKind::Eof,
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
    let mut err = 0i32;
    let mut at = 0usize;
    let mut out: Vec<ffi::FigXmlToken> = Vec::new();
    loop {
        // SAFETY: the function reads `input.len()` bytes at `input` and
        // writes at most `out.len()` tokens to `out`.
        let n = unsafe {
            ffi::fig_kit_xml_tokenize(
                input.as_ptr(),
                input.len(),
                out.as_mut_ptr(),
                out.len(),
                &mut err,
                &mut at,
            )
        };
        if n == usize::MAX {
            let kind = match err {
                ffi::FIG_XML_ERR_UNCLOSED_TAG => ErrorKind::UnclosedTag,
                ffi::FIG_XML_ERR_UNCLOSED_ATTRIBUTE_VALUE => ErrorKind::UnclosedAttributeValue,
                ffi::FIG_XML_ERR_UNCLOSED_COMMENT => ErrorKind::UnclosedComment,
                ffi::FIG_XML_ERR_UNCLOSED_CDATA => ErrorKind::UnclosedCData,
                ffi::FIG_XML_ERR_UNCLOSED_PI => ErrorKind::UnclosedPi,
                ffi::FIG_XML_ERR_UNSUPPORTED_DOCTYPE => ErrorKind::UnsupportedDoctype,
                ffi::FIG_XML_ERR_INVALID_MARKUP => ErrorKind::InvalidMarkup,
                ffi::FIG_XML_ERR_UNEXPECTED_CHARACTER => ErrorKind::UnexpectedCharacter,
                _ => ErrorKind::OutOfMemory,
            };
            return Err(Error { kind, at });
        }
        if n <= out.len() {
            return Ok(out[..n]
                .iter()
                .map(|t| Token {
                    kind: TokenKind::from_c(t.kind),
                    span: Span {
                        start: t.start as usize,
                        end: t.end as usize,
                    },
                })
                .collect());
        }
        out.resize(n, ffi::FigXmlToken::default());
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
