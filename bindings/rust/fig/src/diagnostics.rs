//! Serialization diagnostics: what a cross-format conversion would silently lose.
//!
//! [`crate::Document::diagnose`] and [`crate::Value::diagnose`] run the same
//! pipeline the serializers print from and report each lossy event as a
//! [`Warning`], so a host can warn/block/ignore. Mirrors the C ABI's
//! `fig_*_diagnose` + `FigWarning`.

use std::os::raw::c_int;

use fig_core::diagnostics::{Cause, Code, Warning as CoreWarning};

/// What kind of loss a [`Warning`] describes. Mirrors `FigWarningCode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WarningCode {
    /// A carried comment is not emitted at all.
    CommentDropped,
    /// A block comment is rendered as a run of line comments.
    CommentStyleDegraded,
    /// A node is removed entirely (the target cannot represent it even degraded).
    ValueDropped,
    /// An extended/non-finite value is rendered as a poorer type.
    TypeDegraded,
    /// A code this binding does not recognize (a future core may add codes).
    Unknown(c_int),
}

impl WarningCode {
    fn from_core(c: Code) -> Self {
        match c {
            Code::CommentDropped => WarningCode::CommentDropped,
            Code::CommentStyleDegraded => WarningCode::CommentStyleDegraded,
            Code::ValueDropped => WarningCode::ValueDropped,
            Code::TypeDegraded => WarningCode::TypeDegraded,
        }
    }
}

/// Why a [`Warning`]'s loss happens. Mirrors `FigWarningCause`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WarningCause {
    /// The target format inherently cannot represent it.
    FormatLimitation,
    /// A caller option forced it (e.g. `strip_comments`).
    ExplicitOption,
    /// A cause this binding does not recognize (forward-compat).
    Unknown(c_int),
}

impl WarningCause {
    fn from_core(c: Cause) -> Self {
        match c {
            Cause::FormatLimitation => WarningCause::FormatLimitation,
            Cause::ExplicitOption => WarningCause::ExplicitOption,
        }
    }
}

/// One lossy event a serialization would produce.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Warning {
    pub code: WarningCode,
    pub cause: WarningCause,
    /// Dotted / `[i]` location of the node; empty for the document root.
    pub path: String,
    /// For [`WarningCode::TypeDegraded`], the degraded-to type (e.g. `"string"`);
    /// empty otherwise.
    pub note: String,
}

impl Warning {
    /// One warning the core's `diagnose` found, its path and note copied
    /// into owned (lossy-UTF-8) `String`s.
    pub(crate) fn from_core(w: &CoreWarning) -> Self {
        Warning {
            code: WarningCode::from_core(w.code),
            cause: WarningCause::from_core(w.cause),
            path: String::from_utf8_lossy(&w.path).into_owned(),
            note: w.note.to_owned(),
        }
    }
}
