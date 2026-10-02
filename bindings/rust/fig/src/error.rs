use std::fmt;

use crate::{Format, ffi};

/// Errors produced while parsing, serializing, or deserializing.
///
/// Implements [`std::error::Error`], and (with the `serde` feature)
/// [`serde::de::Error`]/[`serde::ser::Error`] so it can flow through serde.
///
/// `#[non_exhaustive]`: new failure modes get their own variant as the core grows
/// them, so a `match` needs a `_` arm (or an `Err(e) => …` catch-all). The struct
/// variants are non-exhaustive too, so a pattern on one takes `..`; the derive
/// macros build them through the constructors below ([`Error::missing_field`]
/// and the rest), which is also how a hand-written `FromValue` impl does.
///
/// `NotFound` carries no path. The core's status says only that something on
/// the way was missing, not which segment, so the one path the binding could
/// attach is the one the caller just passed — and `NotFound` also comes from
/// calls with no path at all ([`Embed::open`](crate::Embed::open) on a host
/// with no region).
#[derive(Debug)]
// `Number` is only constructed on the serde paths.
#[cfg_attr(not(feature = "serde"), allow(dead_code))]
#[non_exhaustive]
pub enum Error {
    /// A null or otherwise invalid argument reached the C ABI.
    InvalidArgument,
    /// The input could not be parsed as the requested format. Carries a
    /// [`ParseError`] with the core's message and (when known) source location.
    Parse(ParseError),
    /// Allocation failed inside the parser.
    OutOfMemory,
    /// The requested format is not supported.
    UnsupportedFormat,
    /// The operation is not defined for these arguments, though each argument
    /// is individually valid — distinct from [`Error::InvalidArgument`] (a
    /// malformed call). Returned by [`Embed::retype`](crate::Embed::retype)
    /// when asked to move a mid-document block to an edge archetype.
    UnsupportedOperation,
    /// A path, key, or embedded region was not found.
    NotFound,
    /// An unexpected internal error occurred.
    Internal,
    /// A scalar's bytes were not valid UTF-8.
    Utf8,
    /// A numeric scalar could not be parsed (message holds the raw text).
    Number(String),
    /// An error with no structure of its own: a serde-level or derive-level
    /// one, e.g. a type mismatch reported by a `Deserialize` impl, or a
    /// derived enum's fixed "expected a string or a mapping"; or a value the
    /// target format has no way to write, e.g. an infinite float into JSON.
    Message(String),
    /// A required field was absent while building a derived `FromValue` type.
    ///
    /// Both parts are compile-time `&'static str`s, so constructing this is
    /// allocation-free and the message text is assembled lazily in `Display`
    /// rather than `format!`-ed at every derived call site.
    #[non_exhaustive]
    MissingField {
        field: &'static str,
        ty: &'static str,
    },
    /// A derived `FromValue` impl expected a mapping but found another kind.
    #[non_exhaustive]
    ExpectedMapping { ty: &'static str },
    /// A derived enum `FromValue` impl saw a variant/tag it doesn't recognize.
    /// `got` is the (runtime) text that didn't match any known variant.
    #[non_exhaustive]
    UnknownVariant {
        enum_name: &'static str,
        got: String,
    },
    /// A derived tuple-variant `FromValue` impl got the wrong element count.
    #[non_exhaustive]
    WrongSeqLen {
        label: &'static str,
        expected: usize,
        got: usize,
    },
    /// A primitive conversion expected one kind of value but found another.
    /// `found` is one of the `&'static str` kind names from `kind_of`.
    #[non_exhaustive]
    TypeMismatch {
        expected: &'static str,
        found: &'static str,
    },
    /// An integer value was outside the range of the target type. Only the
    /// target type name is kept (a `&'static str`) — deliberately *not* the
    /// offending value, since an `i128` payload would force 16-byte alignment
    /// on the whole `Error` enum and bloat every `Result<_, Error>` site.
    #[non_exhaustive]
    IntOutOfRange { ty: &'static str },
    /// A runtime language was refused by [`language::register`](crate::language::register):
    /// the reason, as the core gave it, and where when it names a place.
    Language(LanguageFailure),
}

/// Why a runtime language was refused, from [`Error::Language`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LanguageFailure {
    /// The core's reason — the rule the description broke, the sample that
    /// failed and how, or the name already taken.
    pub message: String,
    /// A byte offset into the text the failure is about, when the failure
    /// names one: the position of the NUL in a declared string that has
    /// one. `None` otherwise, including every refusal the core reports
    /// today, which says which sample failed but not where in it.
    pub byte_offset: Option<usize>,
}

impl fmt::Display for LanguageFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.byte_offset {
            Some(off) => write!(f, "{} (byte offset {off})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for LanguageFailure {}

/// Details of a parse failure, projected from the C ABI's `FigError`.
///
/// Its `Display` is the error's whole sentence, which [`Error::Parse`] prints
/// as is: `failed to parse input: <message>` for the core's diagnostic, or
/// the message alone when it is already a sentence of its own (an edit the
/// document refused, a parse failure the core gave no detail for).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ParseError {
    /// A human-readable message: for a compiled format, the core's error name
    /// (e.g. `"UnclosedObject"`); for a runtime language, the message its
    /// parser gave.
    pub message: String,
    /// Byte offset of the failure within the input, when known. A runtime
    /// language's parse failure carries the offset its parser reported; the
    /// compiled formats do not surface one yet, so for them this is `None`.
    /// An offset of 0 is indistinguishable from "unknown" at the C ABI and
    /// also reads as `None`.
    pub byte_offset: Option<usize>,
    /// 1-based line of the failure, when known (see `byte_offset`).
    pub line: Option<u32>,
    /// 1-based column of the failure, when known (see `byte_offset`).
    pub column: Option<u32>,
    /// `message` is the whole sentence, not a detail to follow
    /// `failed to parse input: `.
    sentence: bool,
}

impl ParseError {
    /// Build from a filled `FigError`. `byte_offset`/`line`/`column` of 0 are the
    /// ABI's "unknown" sentinel and map to `None`. The message is read from
    /// `message[..message_len]` as lossy UTF-8.
    pub(crate) fn from_ffi(e: &ffi::FigError) -> Self {
        let len = e.message_len.min(e.message.len());
        let message = String::from_utf8_lossy(&e.message[..len]).into_owned();
        ParseError {
            message,
            byte_offset: (e.byte_offset != 0).then_some(e.byte_offset),
            line: (e.line != 0).then_some(e.line),
            column: (e.column != 0).then_some(e.column),
            sentence: false,
        }
    }

    /// A detail-free parse error for paths that have no `FigError` (a bare
    /// `PARSE_ERROR` status). Its message is the whole sentence, so it prints
    /// once: it used to be the same words `Display` puts in front of a
    /// message, and read `failed to parse input: failed to parse input`.
    pub(crate) fn generic() -> Self {
        Self::sentence(String::from("failed to parse input"))
    }

    /// An edit the editor refused because the document would no longer parse
    /// as `format` with it applied. fig knows no more than that — the core
    /// reports a bare status — so this says which part of the request did it.
    pub(crate) fn refused_edit(format: Format, what: EditTarget) -> Self {
        let name = format.display_name();
        Self::sentence(match what {
            EditTarget::Value => format!("the new value would not parse as {name}"),
            EditTarget::Key => format!("the new key would not parse as {name}"),
            EditTarget::Comment => format!("the new comment would not parse as {name}"),
            EditTarget::Document => format!("the edited document would not parse as {name}"),
        })
    }

    fn sentence(message: String) -> Self {
        ParseError {
            message,
            byte_offset: None,
            line: None,
            column: None,
            sentence: true,
        }
    }
}

/// What part of an edit request was new text, for the sentence a refused
/// edit reads as (see [`ParseError::refused_edit`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum EditTarget {
    /// A value, item list or container body the caller supplied.
    Value,
    /// A renamed key or container.
    Key,
    /// A comment's text.
    Comment,
    /// A structural edit (delete, move, reorder) that supplied no text.
    Document,
}

/// An editor call's status as a `Result`: a `PARSE_ERROR` is the edit's own
/// refusal, said in a sentence (see [`ParseError::refused_edit`]), and every
/// other status folds as [`Error::from_status`] folds it.
pub(crate) fn edit_status(
    status: ffi::FigStatus,
    format: Format,
    what: EditTarget,
) -> Result<(), Error> {
    if status == ffi::FigStatus::PARSE_ERROR {
        return Err(Error::Parse(ParseError::refused_edit(format, what)));
    }
    Error::from_status(status)
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.sentence {
            f.write_str(&self.message)?;
        } else {
            write!(f, "failed to parse input: {}", self.message)?;
        }
        match (self.line, self.column) {
            (Some(l), Some(c)) => write!(f, " (line {l}, column {c})"),
            _ => match self.byte_offset {
                Some(off) => write!(f, " (byte offset {off})"),
                None => Ok(()),
            },
        }
    }
}

impl std::error::Error for ParseError {}

impl Error {
    /// `#[cold]`/`#[inline(never)]` constructors keep derived `from_value`
    /// bodies tiny: the error-building code lives here (compiled once) instead
    /// of being inlined — with `format!`/`String` machinery — at every field,
    /// variant, and tag site across the whole dependency graph.
    #[cold]
    #[inline(never)]
    pub fn missing_field(field: &'static str, ty: &'static str) -> Self {
        Error::MissingField { field, ty }
    }

    #[cold]
    #[inline(never)]
    pub fn expected_mapping(ty: &'static str) -> Self {
        Error::ExpectedMapping { ty }
    }

    #[cold]
    #[inline(never)]
    pub fn unknown_variant(enum_name: &'static str, got: &str) -> Self {
        Error::UnknownVariant {
            enum_name,
            got: got.to_string(),
        }
    }

    #[cold]
    #[inline(never)]
    pub fn wrong_seq_len(label: &'static str, expected: usize, got: usize) -> Self {
        Error::WrongSeqLen {
            label,
            expected,
            got,
        }
    }

    /// A [`Error::Message`] from a compile-time string — the derive macros'
    /// constructor for their fixed messages. Cold and out of line, so the
    /// allocation is compiled once here rather than at every derived site.
    #[cold]
    #[inline(never)]
    pub fn msg_static(msg: &'static str) -> Self {
        Error::Message(msg.to_owned())
    }

    #[cold]
    #[inline(never)]
    pub fn type_mismatch(expected: &'static str, found: &'static str) -> Self {
        Error::TypeMismatch { expected, found }
    }

    #[cold]
    #[inline(never)]
    pub fn int_out_of_range(ty: &'static str) -> Self {
        Error::IntOutOfRange { ty }
    }

    pub(crate) fn from_status(status: ffi::FigStatus) -> Result<(), Self> {
        match status {
            ffi::FigStatus::OK => Ok(()),
            ffi::FigStatus::INVALID_ARGUMENT => Err(Self::InvalidArgument),
            ffi::FigStatus::PARSE_ERROR => Err(Self::Parse(ParseError::generic())),
            ffi::FigStatus::OUT_OF_MEMORY => Err(Self::OutOfMemory),
            ffi::FigStatus::UNSUPPORTED_FORMAT => Err(Self::UnsupportedFormat),
            ffi::FigStatus::UNSUPPORTED_OPERATION => Err(Self::UnsupportedOperation),
            ffi::FigStatus::NOT_FOUND => Err(Self::NotFound),
            // `INTERNAL_ERROR` and any code fig may add in a later release fold
            // into `Internal`: an unrecognized status is never mistaken for `Ok`.
            _ => Err(Self::Internal),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidArgument => f.write_str("invalid argument"),
            Error::Parse(e) => write!(f, "{e}"),
            Error::OutOfMemory => f.write_str("out of memory"),
            Error::UnsupportedFormat => f.write_str("unsupported format"),
            Error::UnsupportedOperation => f.write_str("unsupported operation"),
            Error::NotFound => f.write_str("path or region not found"),
            Error::Internal => f.write_str("internal error"),
            Error::Language(failure) => write!(f, "runtime language: {failure}"),
            Error::Utf8 => f.write_str("scalar was not valid UTF-8"),
            Error::Number(raw) => write!(f, "invalid number: {raw}"),
            Error::Message(msg) => f.write_str(msg),
            Error::MissingField { field, ty } => {
                write!(f, "missing field `{field}` while building `{ty}`")
            }
            Error::ExpectedMapping { ty } => write!(f, "expected a mapping to build `{ty}`"),
            Error::UnknownVariant { enum_name, got } => {
                write!(f, "unknown variant `{got}` for enum `{enum_name}`")
            }
            Error::WrongSeqLen {
                label,
                expected,
                got,
            } => {
                write!(
                    f,
                    "expected {expected} element(s) for `{label}`, found {got}"
                )
            }
            Error::TypeMismatch { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            Error::IntOutOfRange { ty } => {
                write!(f, "integer out of range for {ty}")
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(feature = "serde")]
impl serde::de::Error for Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Error::Message(msg.to_string())
    }
}

#[cfg(feature = "serde")]
impl serde::ser::Error for Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Error::Message(msg.to_string())
    }
}
