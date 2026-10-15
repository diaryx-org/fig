//! Port of languages/figl/src/parser.zig.
//!
//! fig authoring dialect — block-layer + flow-mode parser. See DESIGN.md for
//! the full grammar; this file implements the "Implied implementation shape":
//! a line splitter (markers/key/value dispatch, inline here rather than a
//! separate pass, since committed values can span many lines), a stack-based
//! block builder (depth counts + header baselines), a recursive-descent flow
//! sub-parser, and the literal-else-string value resolver (`tokenizer.rs`).
//!
//! Construction writes rows of the node table fig hands the parse
//! (`RowWriter`) as the lines are read: a value is its row, written when the
//! value is met, and a container's row is written the moment the container
//! exists — an undecided one as an empty mapping, turned into a sequence when
//! a `*` or an index reaches it first. Children are linked through a
//! `ChildIndex`, which also finds a mapping's entry by key, so "navigate to an
//! arbitrary existing path and add a child" (dotted keys, index addressing,
//! header re-entry) stays constant time per step. What a row cannot hold
//! about a container — its shape before a child decides it, whether a flow
//! value closed it, how a sequence is addressed, an addressed sequence's
//! elements in order — sits in maps keyed by row that hold containers only.
//! Comments, tags, markers, separators, header lines and name mentions are
//! written as facts when they are met. A container's row starts at its
//! opening position; once every line is read `finish_spans` widens each to
//! its subtree, and `RowWriter::finish` puts the rows in pre-order.
//!
//! Scope cuts (documented, not silent):
//!   * Of DESIGN.md's WARN diagnostics, the comment-depth-mismatch lint is not
//!     implemented (needs offsets threaded through `PendingComment`), and the
//!     inline-`#`-truncation warn is deliberately skipped (indistinguishable
//!     from an ordinary trailing comment). The rest — the coercion warns, the
//!     indent/marker-count lint, flow-like strings, missing flow commas — are
//!     collected into `Report::warnings` (see `Warning`).
//!   * Comment attachment honors a comment line's own marker depth when
//!     containers close: a comment at (or below) a closing container's child
//!     depth becomes its dangling run; a shallower one stays pending and binds
//!     to the next sibling line as its leading run.
//!   * Line terminators are LF or CRLF: a `\r` immediately before `\n` (or at
//!     EOF) is trivia everywhere — see `at_crlf` — so CRLF documents parse
//!     identically to LF ones. A `\r` elsewhere stays ordinary content bytes.
//!
//! Every node also carries a source `Span` (see `finish_spans`) — the
//! foundation the editor needs to splice edits in place. Whole-container
//! structural ops — the generic `deleteContainer`/`moveContainer`/
//! `reorderContainers` — are built on those spans plus
//! `Document::regions_of`, which this parser fills with every block
//! container's header lines (see `new_block_container` and
//! `resolve_header_final`).
//!
//! **Rust.** Zig's parser struct is also a language's parse handle; here the
//! entry points are free functions and `Parser` is the parse in progress. Its
//! arenas are gone: a decoded text is a `Vec` its row copies, a source text a
//! slice of the table's own source (so `Table::text` keeps it there).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use super::tokenizer as tok;
use super::tokenizer::{BracketCommit, Sniffed};
use super::Type;
use crate::err;
use crate::kit::ast::{Comment, ExtKind, Extended, KeyValue, KindTag, NodeId, NodeKind, Number, NumberKind, Tag, AST};
use crate::kit::child_index::ChildIndex;
use crate::kit::datetime;
use crate::kit::document::{self, Document, MentionKind};
use crate::kit::literal;
use crate::kit::parse_diagnostic;
use crate::kit::row_writer::RowWriter;
use crate::kit::span::Span;
use crate::kit::table::{self, Table, NONE};
use crate::{Error, Result};

/// `Error`: the names this parser fails with (Zig's `Parser.Error`, which
/// includes the tokenizer's `ScanError` and `OutOfMemory`), in declaration
/// order.
pub const ERRORS: &[&str] = &[
    // A container ended up with children of both shapes (`key = v` AND `*`).
    "FigMixedContainerChildren",
    // A sequence mixed `[]`/`[i]` addressing with `*` elements.
    "FigMixedSequenceAddressing",
    // A dotted/index path stepped into an existing scalar key.
    "FigKeyNotContainer",
    // An assignment or header re-defined an existing leaf key.
    "FigDuplicateKey",
    // `xs[i]` skipped ahead of the sequence's current length.
    "FigIndexSkipped",
    // A non-final `[]` referenced an empty sequence.
    "FigEmptyAppendTarget",
    // An assignment-final `[]` (`xs[] = v`) — `[]` appends only as a header
    // (or as a mid-path "last element" step), never as an assignment target.
    "FigAppendAssignment",
    // An assignment addressed an index that was already set.
    "FigIndexAlreadySet",
    // A header/element-opener container closed with zero children.
    "FigEmptyContainer",
    // A `>` marker with no open header above it.
    "FigRootMarker",
    // A marker run jumped more than one level deeper than the last.
    "FigSkippedLevel",
    // A marker run with no whitespace before the key that follows it.
    "FigBadMarkerSeparator",
    // A key was empty, or a bare key began with a structural character.
    "FigBadKey",
    // `key: value` (YAML/JSON habit) — `:` introduces a type, not a value.
    "FigForeignSyntaxColon",
    // `[section]`/`[[x]]` (TOML habit) at line start.
    "FigForeignSyntaxBracket",
    // A `-` element line (`- v`, `>-`, `> -`) — YAML habit; fig elements are `*`.
    "FigForeignSyntaxDash",
    // `* key = value` — an element's RHS looked like an inline field.
    "FigElementInlineField",
    // An assignment/element had no value at all.
    "FigInvalidValue",
    // A typed value (`: int`, `: bool`, ...) didn't match its annotation.
    "FigTypeMismatch",
    // An unrecognized `: type` name.
    "FigUnknownType",
    // Non-comment content survived after a value/header on its line.
    "FigTrailingContent",
    // A single-line quoted string closed at its matching quote, but more
    // non-comment content followed on the same line — the classic
    // wrapped-a-bare-string-in-unescaped-quotes slip (`k = "She said, "Hi""`).
    "FigQuotedTrailingContent",
    // Non-comment content followed a `'''`/`"""` opening delimiter on its own
    // line (`x = '''abc'''`).
    "FigMultilineOpenerContent",
    // A `+` continuation line with no `[]` append header (or `+`) as the most
    // recent zero-marker structural line — nothing to re-run.
    "FigDanglingContinuation",
    // A dotted path / header stepped into a container written as a flow value.
    "FigClosedFlowValue",
    // A `[`/`{` flow region never found its matching close.
    "FigUnclosedFlow",
    // One flow object mixed `=` (fig) and `:` (JSON) pair separators.
    "FigMixedFlowSeparators",
    // A bare key before `:` in a flow object (`{x: 1}`).
    "FigFlowBareKeyColon",
    // A key in a fig-spelled flow object with no `=` after it (`{ x }`).
    "FigFlowMissingEquals",
    // Two flow members with no `,` between them.
    "FigFlowMissingComma",
    // A flow member that is nothing at all (`[a,,b]`, `[,]`).
    "FigFlowEmptyElement",
    // A triple-quoted string inside a flow value.
    "FigFlowMultilineString",
    // A flow value left unclosed because a whitespace-preceded `#` inside it
    // opened a comment that ran past the closing bracket (`[see #3, y]`).
    "FigFlowCommentedClose",
    // The input bytes are not valid UTF-8.
    "FigInvalidUtf8",
    // The tokenizer's `ScanError`.
    "FigUnclosedString",
    "FigBadEscape",
    "OutOfMemory",
];

/// The teaching message for `code` — DESIGN.md's "every diagnostic names the
/// fix": the format teaches its own conventions at the point of failure,
/// because the target user is editing over SSH with no docs open. One sentence,
/// always spelling the valid fig form.
pub fn describe(code: Error) -> &'static str {
    match code.name() {
        "FigForeignSyntaxColon" => "`:` introduces a type, not a value; write `key = value`, or `key: type = value`",
        "FigFlowBareKeyColon" => "a bare key cannot take a `:` pair; write `key = 1` (fig) or `\"key\": 1` (JSON)",
        "FigForeignSyntaxDash" => "`-` is YAML's element marker; fig elements are `*` — write `> *` then `> > host = a.com` (a scalar element is `* value`)",
        "FigForeignSyntaxBracket" => "`[section]` / `[[x]]` is TOML; fig section headers are bare dotted paths — write `section`, or `x[]` to append an element",
        "FigElementInlineField" => "an element's fields go on following lines; write `> *` then `> > host = a.com`, not `* host = a.com`",
        "FigRootMarker" => "root keys carry zero markers; remove the `>` (a marker line needs a parent header above it)",
        "FigSkippedLevel" => "this line skips a nesting level; depth may only grow one `>` at a time — add the missing parent line, or drop the extra `>`",
        "FigBadMarkerSeparator" => "put a space between the marker run and what follows: `> key`, not `>key`",
        "FigBadKey" => "empty or malformed key; a bare key cannot contain `.` `:` `=` `[` or whitespace, or begin with `>`/`-` — quote it: `\"my.key\" = x`",
        "FigDuplicateKey" => "duplicate key: this key already has a value here; remove one of the definitions (re-enter a header only to add NEW keys)",
        "FigMixedContainerChildren" => "a container holds either `key = value` entries or `*` elements, never both",
        "FigMixedSequenceAddressing" => "one sequence cannot mix `[]`/`[i]` addressing with `*` element lines; pick one spelling",
        "FigKeyNotContainer" => "this path steps into an existing non-container value; remove the extra path segment, or restructure the earlier value",
        "FigIndexSkipped" => "sequence indices cannot skip ahead; write the earlier element first (even `[n] = null`)",
        "FigIndexAlreadySet" => "this index already has a value; address a new index, or use a header-final `[]` to append",
        "FigEmptyAppendTarget" => "a non-final `[]` means \"the last element\", but this sequence is empty; append one first with a header-final `[]`",
        "FigAppendAssignment" => "a final `[]` appends a container via a header, never a `= value`; write `key[]` on its own line with `> field = value` lines below, or spell the next index out: `key[N] = value`",
        "FigEmptyContainer" => "this container has no children; write an inline empty value instead: `key = {}` (map) or `key = []` (sequence)",
        "FigInvalidValue" => "missing value; write one after `=`, or `{}`/`[]` for an empty container",
        "FigTypeMismatch" => "the value does not satisfy its `: type` annotation; fix the value, or drop/correct the annotation",
        "FigUnknownType" => "unknown type name; the built-in types are int, float, bool, string, enum, char, datetime, date, time",
        "FigTrailingContent" => "unexpected content after this line's value or header; quote the whole value if it is one string (a `#` comment needs a space before it)",
        "FigQuotedTrailingContent" => "this string ends at its matching quote, and the rest of the line is stray content; fig bare strings need no outer quotes — write `key = She said, \"Hey there!\"`, or escape the inner quotes: `\"She said, \\\"Hey there!\\\"\"`",
        "FigMultilineOpenerContent" => "a multiline string's content begins on the line AFTER the opening `'''`/`\"\"\"`; move this text down a line (only a `# comment` may share the opener line)",
        "FigDanglingContinuation" => "`+` has no `[]` append header to re-run; move it directly after its `a.b[]` group, or repeat the header",
        "FigClosedFlowValue" => "a value written inline as `[…]`/`{…}` is closed and cannot be extended later; write the block or header form if it needs to grow",
        "FigUnclosedFlow" => "this `[`/`{` value never finds its matching close; close it, or quote the whole value to make it a string",
        "FigMixedFlowSeparators" => "a flow object is fig (`=` pairs) or JSON (`:` pairs), never both in one object",
        "FigFlowMissingEquals" => "every member of a `{…}` object is `key=value`, and an unquoted value is one word; quote a value with spaces: `{name=\"Adam Harris\"}`",
        "FigFlowMissingComma" => "separate the members of a `[…]` list (or a JSON-style `{…}` object) with `,`; an unquoted value cannot span lines — quote it",
        "FigFlowEmptyElement" => "empty member; remove the extra `,`",
        "FigFlowMultilineString" => "a `'''`/`\"\"\"` multi-line string cannot appear inside `[…]`/`{…}`; write the container in block form, or use a single-line quoted string with `\\n`",
        "FigFlowCommentedClose" => "inside `[…]`/`{…}`, a `#` after a space starts a comment, and this one hides the closing bracket; quote the value that contains the `#`",
        "FigUnclosedString" => "unclosed string; add the closing quote (a single-line quote cannot span lines — use `'''` for multi-line)",
        "FigBadEscape" => "invalid escape; double quotes support \\n \\t \\r \\\\ \\\" \\uXXXX — use single quotes ('…') for raw text with literal backslashes",
        "FigInvalidUtf8" => "this file is not valid UTF-8; fig documents must be UTF-8 encoded",
        "OutOfMemory" => "out of memory",
        // Not one of this parser's names (a kit error passed through).
        other => other,
    }
}

/// A short (few-word) noun phrase for `code` — distinct from `describe`'s full
/// teaching sentence. Meant to sit next to a caret (`^ duplicate key`), the
/// way rustc annotates a span, so a report scanned at a glance still names the
/// problem even with the longer message above it collapsed/scrolled past.
pub fn short_label(code: Error) -> &'static str {
    match code.name() {
        "FigForeignSyntaxColon" => "not a value assignment",
        "FigFlowBareKeyColon" => "bare key needs `=`",
        "FigForeignSyntaxDash" => "not a fig element marker",
        "FigForeignSyntaxBracket" => "not a fig header",
        "FigElementInlineField" => "field on element line",
        "FigRootMarker" => "stray `>` marker",
        "FigSkippedLevel" => "skipped nesting level",
        "FigBadMarkerSeparator" => "missing space after marker",
        "FigBadKey" => "invalid key",
        "FigDuplicateKey" => "duplicate key",
        "FigMixedContainerChildren" => "mixed container children",
        "FigMixedSequenceAddressing" => "mixed sequence addressing",
        "FigKeyNotContainer" => "not a container",
        "FigIndexSkipped" => "skipped index",
        "FigIndexAlreadySet" => "index already set",
        "FigEmptyAppendTarget" => "append target is empty",
        "FigAppendAssignment" => "cannot assign to `[]`",
        "FigEmptyContainer" => "empty container",
        "FigInvalidValue" => "missing value",
        "FigTypeMismatch" => "type mismatch",
        "FigUnknownType" => "unknown type",
        "FigTrailingContent" => "trailing content",
        "FigQuotedTrailingContent" => "trailing content after quote",
        "FigMultilineOpenerContent" => "content on multiline opener line",
        "FigDanglingContinuation" => "dangling continuation",
        "FigClosedFlowValue" => "value is closed",
        "FigUnclosedFlow" => "unclosed flow",
        "FigMixedFlowSeparators" => "mixed `=`/`:` separators",
        "FigFlowMissingEquals" => "missing `=`",
        "FigFlowMissingComma" => "missing `,`",
        "FigFlowEmptyElement" => "empty member",
        "FigFlowMultilineString" => "multi-line string in flow",
        "FigFlowCommentedClose" => "comment hides the close",
        "FigUnclosedString" => "unclosed string",
        "FigBadEscape" => "invalid escape",
        "FigInvalidUtf8" => "invalid UTF-8",
        "OutOfMemory" => "out of memory",
        other => other,
    }
}

/// Shared byte-offset → line/col + `file:line:col:` report rendering — the
/// half of the diagnostic system that has nothing to do with fig's own error
/// codes (`kit::parse_diagnostic`), re-exported under the names the Zig
/// parser gives them.
pub use parse_diagnostic::{locate_offset, Location};

/// The compiler-style report both severities share: `file:line:col: <label>:
/// <message>`, then the offending source line and a caret marking the column.
fn render_report_alloc(source: &[u8], offset: usize, file: &[u8], label: &[u8], message: &[u8]) -> Vec<u8> {
    parse_diagnostic::render_report_alloc(source, offset, file, label, message)
}

/// A parse failure plus the byte position where the parser stopped. The parser
/// is single-pass, so when an error is returned its cursor sits on (or just
/// after) the offending token — precise enough for `file:line:col` without
/// threading a location through every error site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Error,
    /// Byte offset into the source where parsing stopped — the caret anchor.
    pub offset: usize,
    /// Byte offset just past the offending token, when the parser knows its
    /// exact extent (e.g. a `: type` value that failed its annotation). Lets an
    /// editor squiggle precisely that token instead of the whole rest of the
    /// line (which would swallow a trailing comment); `None` means "no known
    /// end, fall back to end-of-line". Always on `offset`'s line.
    pub end: Option<usize>,
}

impl Diagnostic {
    /// 1-based line/column of `offset`, plus the full offending line.
    pub fn locate<'a>(&self, source: &'a [u8]) -> Location<'a> {
        locate_offset(source, self.offset)
    }

    /// Render `file:line:col: error: <message>` + source line + caret.
    pub fn render_alloc(&self, source: &[u8], file: &[u8]) -> Vec<u8> {
        render_report_alloc(source, self.offset, file, b"error", describe(self.code).as_bytes())
    }
}

/// What an authoring-time lint is about (Zig's `Warning.Code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WarningCode {
    /// A bare token spelling a bool/null from another config language
    /// (`Yes`, `ON`, `TRUE`, `Null`) fell to a plain string — the Norway
    /// class, surfaced instead of silently coerced either way.
    StringLooksLikeLiteral,
    /// A token that would be a valid number but for a leading zero on its
    /// integer part (`007`, `-07`, `01.5`) kept its padding as a string
    /// (the TOML leading-zero rule).
    StringLeadingZero,
    /// A number-shaped token that no valid number spelling accepts — a
    /// trailing-dot float (`1.`, `42.`) or a leading-dot float (`.5`,
    /// `.25`) — fell to a string. Narrow by design (only a clean integer +
    /// a single trailing `.`, or a single leading `.` + clean digits), so
    /// version strings (`1.2.3`), prose (`12 monkeys`), and `.e5`/`..`
    /// never warn.
    StringLooksLikeNumber,
    /// A bare clock time with no date (`10:30`) sniffed to a datetime
    /// value. A bare *date* does not warn — that shape is the overwhelmingly
    /// common, deliberate hand-authored case — but `HH:MM[:SS]` is exactly
    /// the shape of a duration, a ratio, or a sports score too.
    AmbiguousDatetime,
    /// A balanced, well-formed `[…]`/`{…}` separated from trailing content
    /// by whitespace (`[80, 443] x`) fell to a bare string. Glued shapes
    /// (`[Blog](/x)`, `[a-z]*.md`) are the intended bare strings and never
    /// warn.
    FlowLikeString,
    /// A bare flow value contains ` = ` (`{x = 1 y = 2}` → `x = "1 y =
    /// 2"`) — almost always a missing comma between pairs.
    FlowMissingComma,
    /// A line's visual indentation disagrees with its `>` marker count
    /// (convention: 2 spaces per level, no tabs). Meaning follows the
    /// count; the indent is the redundant cross-check that just failed.
    IndentMarkerMismatch,
}

impl WarningCode {
    /// The Zig tag name (`string_looks_like_literal`), as the LSP reports it.
    pub fn name(self) -> &'static str {
        match self {
            WarningCode::StringLooksLikeLiteral => "string_looks_like_literal",
            WarningCode::StringLeadingZero => "string_leading_zero",
            WarningCode::StringLooksLikeNumber => "string_looks_like_number",
            WarningCode::AmbiguousDatetime => "ambiguous_datetime",
            WarningCode::FlowLikeString => "flow_like_string",
            WarningCode::FlowMissingComma => "flow_missing_comma",
            WarningCode::IndentMarkerMismatch => "indent_marker_mismatch",
        }
    }
}

/// An authoring-time lint (DESIGN.md "Authoring-time diagnostics", warn
/// severity): the tree is well-defined, but the line is a likely mistake.
/// Collected during the parse and returned via `Report` — the caller decides
/// presentation (`--quiet` silences, `--strict` promotes to failure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warning {
    pub code: WarningCode,
    /// Byte offset of the surprising token (see `locate_offset`).
    pub offset: usize,
    /// Byte offset just past the surprising token, when the extent is known.
    /// Mirrors `Diagnostic::end`. Always on `offset`'s line; `None` means "no
    /// known end, fall back to end-of-line".
    pub end: Option<usize>,
}

impl Warning {
    /// The teaching message — same name-the-fix contract as `describe`.
    pub fn describe_warning(code: WarningCode) -> &'static str {
        match code {
            WarningCode::StringLooksLikeLiteral => "this spells a boolean/null in other config languages, but fig literals are lowercase-only, so it stays the string it spells; quote it to make the string explicit, or lowercase it for the literal",
            WarningCode::StringLeadingZero => "a leading zero is not a number in fig, so the padding was kept as a string; quote it to make that explicit, or drop the zero for a number",
            WarningCode::StringLooksLikeNumber => "a bare leading or trailing dot is not a valid number in fig, so this was kept as a string; write the missing zero (`0.5`, `1.0`) for a number, or quote it to affirm the string",
            WarningCode::AmbiguousDatetime => "a bare clock time becomes a datetime value, not text (also a duration/ratio shape); quote it if you meant a string",
            WarningCode::FlowLikeString => "this looks like a `[…]`/`{…}` flow value with trailing content, so the whole line was read as one bare string; remove the trailing content for a collection, or quote the value to affirm the string",
            WarningCode::FlowMissingComma => "a bare flow value containing ` = ` usually means a missing comma between pairs; add the comma, or quote the value if it really is text",
            WarningCode::IndentMarkerMismatch => "indentation disagrees with the `>` marker count (convention: 2 spaces per level); meaning follows the count — fix whichever signal is wrong",
        }
    }

    /// A short (few-word) noun phrase for `code` — distinct from
    /// `describe_warning`'s full teaching sentence: a caret annotation,
    /// rustc-style.
    pub fn short_label(code: WarningCode) -> &'static str {
        match code {
            WarningCode::StringLooksLikeLiteral => "looks like a boolean/null",
            WarningCode::StringLeadingZero => "leading zero",
            WarningCode::StringLooksLikeNumber => "leading/trailing dot",
            WarningCode::AmbiguousDatetime => "ambiguous datetime",
            WarningCode::FlowLikeString => "looks like a flow value",
            WarningCode::FlowMissingComma => "missing comma",
            WarningCode::IndentMarkerMismatch => "indent/marker mismatch",
        }
    }

    /// Render `file:line:col: warning: <message>` + source line + caret.
    pub fn render_alloc(&self, source: &[u8], file: &[u8]) -> Vec<u8> {
        render_report_alloc(source, self.offset, file, b"warning", Warning::describe_warning(self.code).as_bytes())
    }
}

/// Everything a parse reports besides the tree: the failure diagnostic (set
/// only on error) and the authoring-time warnings (valid on success AND on
/// failure — warns collected before the error still stand).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub diag: Option<Diagnostic>,
    /// Every parse error, in source order — populated ONLY by the recovering
    /// entry point (`parse_collecting`), which skips past each failure to the
    /// next line and keeps going, so a language server can squiggle a whole
    /// file's mistakes at once (and `check` can list them in one pass). The
    /// single-shot `parse`/`parse_with_report` stop at the first error and
    /// leave this empty, setting `diag` alone. When non-empty, `diag` mirrors
    /// `errors[0]`.
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Warning>,
}

// ── Construction state ──────────────────────────────────────────────────────

/// A container's shape is frozen the moment its first child is added — root
/// and every header/element-opener target start undecided, written as an
/// empty mapping row until a `*` or an index turns it into a sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Shape {
    #[default]
    Undecided,
    Mapping,
    Sequence,
}

/// How a sequence's elements are written: `*` lines, or `[]`/`[i]` paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Style {
    #[default]
    Undecided,
    Element,
    Addressed,
}

#[derive(Debug, Clone, Copy, Default)]
struct ContainerInfo {
    shape: Shape,
    style: Style,
    /// Created by a flow value (`[…]`/`{…}`) — closed to later extension via
    /// dotted paths, headers, or indexing (the TOML inline-table rule).
    closed: bool,
    /// Created by a header-final `[]` append (or a `+` re-run of one). Those
    /// elements are pre-committed to map shape at birth, which would bypass
    /// the `Undecided` → `FigEmptyContainer` check at frame close — this flag
    /// lets the close path apply the same "no empty block containers" rule.
    born_of_append: bool,
}

/// A buffered comment line plus its own marker depth. The depth decides where
/// the comment lands when containers close: a comment at (or below) a closing
/// frame's child depth was written inside that container (its dangling run); a
/// shallower one belongs to whatever sibling line comes next (its leading
/// run). This is DESIGN.md's "a depth-prefixed comment attaches at that depth
/// to the next sibling".
#[derive(Debug, Clone, Copy)]
struct PendingComment<'s> {
    comment: Comment<'s>,
    depth: u32,
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    /// The container row this frame's lines add to — where its trailing and
    /// dangling comments land too.
    container: NodeId,
    /// The depth (relative to the active baseline) that this frame's children
    /// must be written at.
    child_depth: u32,
}

/// An index step: a literal `[n]`, with where `n` is written, or `[]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexKind {
    Literal { n: usize, span: Span },
    AppendOrLast,
}

/// A dotted-path key segment plus its own source span (quotes included when
/// quoted) — carried through path navigation so every entry the path creates
/// (intermediate dotted segments included) gets an accurate key span. `name`
/// is a source slice or a decoded copy.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KeySeg<'s> {
    name: Cow<'s, [u8]>,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step<'s> {
    Key(KeySeg<'s>),
    Index(IndexKind),
}

/// The outcome of resolving a HEADER line's final path segment: the container
/// to push as the next frame, and where the line's leading comments bind —
/// the entry's key when reached via a map key, the container itself when
/// reached via a sequence index (`AST::leading_comment_anchor`).
#[derive(Debug, Clone, Copy)]
struct Resolved {
    container: NodeId,
    leading: NodeId,
}

/// The container a path's steps but the last reach, and whether the line is
/// one of its own (`MentionKind` of the key the last step names). The first
/// step's parent is the line's target, whose body the line is in; past it,
/// the line is a container's own when the line created it, and not when
/// the path re-opened one written earlier, which this line does not
/// re-enter (only a header-final step does, `resolve_header_final`).
#[derive(Debug, Clone, Copy)]
struct Reached {
    container: NodeId,
    own: bool,
}

impl Reached {
    fn kind(self) -> MentionKind {
        if self.own { MentionKind::Entry } else { MentionKind::Header }
    }
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The parse in progress: Zig's `Parser` struct as `parseImpl` makes it.
pub struct Parser<'s, 't> {
    source: &'s [u8],
    pos: usize,
    /// The root mapping's row, written first.
    root: NodeId,
    pending_leading: Vec<PendingComment<'s>>,
    stack: Vec<Frame>,
    /// The path of the most recent zero-marker `[]` append header, kept while
    /// only deeper lines / comments / blanks / `+` lines follow — the target a
    /// `+` continuation line re-runs. Any other zero-marker structural line
    /// clears it.
    last_append_steps: Option<Vec<Step<'s>>>,
    /// Overrides `pos` as the `Diagnostic` offset when set — for error sites
    /// where the cursor has already moved past the token that actually
    /// offends (set via `fail_at`). `pos` is the right answer everywhere else.
    fail_offset: Option<usize>,
    /// The offending token's end offset, paired with `fail_offset` (set via
    /// `fail_span`) — populates `Diagnostic::end`.
    fail_end: Option<usize>,
    /// The `#` of the most recent flow-interior comment whose text holds a
    /// `]`/`}` — the close it may have swallowed (`[see #3, y]`). Reported by
    /// `unclosed_flow` when that flow value then runs out of input.
    flow_comment_close: Option<usize>,
    /// Byte offset of the start of the line currently being processed (set
    /// once per iteration of the main loop in `run`). The fallback span anchor
    /// for a container whose opener carries no more precise position of its
    /// own (a bare `*` element-opener, an `[]`-append-created element, …).
    cur_line_start: usize,
    /// Authoring-time lints collected during the pass. Empty unless a warn
    /// site fired.
    warnings: Vec<Warning>,
    /// Error-recovery mode: when set, `run` catches each per-line failure into
    /// `diagnostics` and resyncs to the next line instead of returning the
    /// first error. `OutOfMemory` is never recovered.
    recover: bool,
    /// Recoverable parse errors, in source order. Populated only in `recover`
    /// mode.
    diagnostics: Vec<Diagnostic>,
    /// The table the rows are written into as the lines are read. Besides the
    /// rows it takes each block container's header lines
    /// (`Document::regions_of`), each `*` marker, each assignment's `=`, and
    /// each block container's name mentions — its creating key and every key
    /// that re-opens it, a path's final step or one mid-way. A key is an
    /// `Entry` mention when its line is one of its parent's own lines — the
    /// line's target, whose body it is in, or a container this line created —
    /// and a `Header` one when the path reached its parent through a
    /// container written elsewhere (`Reached`).
    w: RowWriter<'t>,
    /// Each container's last child, and each mapping's entry by key.
    children: ChildIndex,
    /// What a container's row cannot say about it, by row: only containers are
    /// here.
    containers: HashMap<NodeId, ContainerInfo>,
    /// An index-addressed sequence's elements in order, by its row — `xs[i]`
    /// needs element i. Only sequences addressed that way are here.
    addressed: HashMap<NodeId, Vec<NodeId>>,
}

// ── Entry points ─────────────────────────────────────────────────────────────

/// `input` as an `AST`, alone.
pub fn parse_abstract(input: &[u8], format: Type) -> Result<AST> {
    Ok(parse(input, format)?.ast)
}

/// `input` as a `Document`: its table written and adopted.
pub fn parse(input: &[u8], format: Type) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_into(&mut t, &source, format)?;
    RowWriter::adopt(source, &mut t)
}

/// `parse`, reporting as `parse_with_report_into` does.
pub fn parse_with_report(input: &[u8], format: Type, out: &mut Report) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_with_report_into(&mut t, &source, format, out)?;
    RowWriter::adopt(source, &mut t)
}

/// `parse`, collecting as `parse_collecting_into` does.
pub fn parse_collecting(input: &[u8], format: Type, out: &mut Report) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_collecting_into(&mut t, &source, format, out)?;
    RowWriter::adopt(source, &mut t)
}

/// Write `input`'s rows into `t`, the table fig hands a parse (a table over
/// `input`).
pub fn parse_into(t: &mut Table, input: &[u8], format: Type) -> Result<()> {
    parse_impl(t, input, format, &mut Report::default(), false)
}

/// `parse_into`, but also fills `out`: `diag` on failure (error code + byte
/// offset, for `file:line:col` teaching messages), `warnings` always
/// (authoring-time lints). The hook the CLI (and the C ABI) renders reports
/// from.
pub fn parse_with_report_into(t: &mut Table, input: &[u8], format: Type, out: &mut Report) -> Result<()> {
    parse_impl(t, input, format, out, false)
}

/// `parse_with_report_into`, but recovers past each error to collect the
/// WHOLE file's diagnostics in one pass (`out.errors`, source order) rather
/// than stopping at the first — the entry a language server wants, so every
/// mistake squiggles at once. On any error the return value is still the
/// first error code and the table holds whatever rows the lines before it
/// wrote (both consumers discard it); a clean parse writes it exactly as
/// `parse_with_report_into` would. `out.diag` mirrors `errors[0]`.
pub fn parse_collecting_into(t: &mut Table, input: &[u8], format: Type, out: &mut Report) -> Result<()> {
    parse_impl(t, input, format, out, true)
}

fn parse_impl(t: &mut Table, input: &[u8], format: Type, out: &mut Report, recover: bool) -> Result<()> {
    // `Type` has a single member (`Fig`) — the fig dialect is unversioned — so
    // there is nothing to branch on. The parameter is kept only so this parser
    // matches the uniform `parse(input, format)` signature the multi-format
    // dispatch relies on.
    let _ = format;

    // A fig document must be valid UTF-8 (checked once, up front — mirrors
    // TOML's `parseOnce`).
    if std::str::from_utf8(input).is_err() {
        return Err(err!(FigInvalidUtf8));
    }

    // Scan the table's own copy of the source when it is `input`, so that a
    // text sliced from it stays in the source (`Table::text`).
    let arc = t.source_arc();
    let source: &[u8] = if *arc == *input { &arc } else { input };

    let mut p = Parser::new(source, t, recover);
    // The root: a mapping until a root `*` line makes it a sequence. Its span
    // reaches its last child's end once the lines are read (`finish_spans`).
    p.root = p.new_container(Span::new(0, 0), ContainerInfo::default())?;
    // A leading UTF-8 BOM is permitted and ignored (mirrors TOML's tokenizer /
    // JSON's tokenizer) — skip it before any tokenizing/key parsing happens.
    if input.starts_with(BOM) {
        p.pos = 3;
    }
    let result = p.parse_body(out);
    // Warnings are handed out on every exit path (they are valid alongside a
    // failure too).
    out.warnings = std::mem::take(&mut p.warnings);
    result
}

impl<'s, 't> Parser<'s, 't> {
    fn new(source: &'s [u8], t: &'t mut Table, recover: bool) -> Parser<'s, 't> {
        Parser {
            source,
            pos: 0,
            root: 0,
            pending_leading: Vec::new(),
            stack: Vec::new(),
            last_append_steps: None,
            fail_offset: None,
            fail_end: None,
            flow_comment_close: None,
            cur_line_start: 0,
            warnings: Vec::new(),
            recover,
            diagnostics: Vec::new(),
            w: RowWriter::init(t),
            children: ChildIndex::new(),
            containers: HashMap::new(),
            addressed: HashMap::new(),
        }
    }

    fn parse_body(&mut self, out: &mut Report) -> Result<()> {
        if let Err(e) = self.run() {
            // Non-recover mode (or an unrecoverable OOM in recover mode): the
            // first error is the whole story — report it and stop.
            out.diag = Some(Diagnostic { code: e, offset: self.fail_offset.unwrap_or(self.pos), end: self.fail_end });
            return Err(e);
        }
        // Recover mode collected its errors into `self.diagnostics` instead of
        // returning them. If any fired, the rows may be malformed, so leave
        // them unfinished: surface every diagnostic (source order) and return
        // the first code, so callers that branch on "did it parse?" behave
        // exactly as before.
        if let Some(&first) = self.diagnostics.first() {
            out.diag = Some(first);
            out.errors = self.diagnostics.clone();
            return Err(first.code);
        }
        self.finish_spans()?;
        self.w.finish(self.root)
    }

    /// Return `e` with the diagnostic caret pinned to `offset` — for the sites
    /// where the cursor has already scanned past the token that actually
    /// offends (e.g. the `:` of `key: value`, consumed before the missing `=`
    /// is noticed).
    fn fail_at(&mut self, offset: usize, e: Error) -> Error {
        self.fail_offset = Some(offset);
        e
    }

    /// Like `fail_at`, but also pins the offending token's end (`start..end`)
    /// so the diagnostic carries a tight range.
    fn fail_span(&mut self, start: usize, end: usize, e: Error) -> Error {
        self.fail_offset = Some(start);
        self.fail_end = Some(end);
        e
    }

    /// Record an authoring-time lint spanning `[start, end)` — a precise token
    /// extent so an editor squiggles exactly that token instead of the whole
    /// line. (Zig's point-only `warn` has no caller.)
    fn warn_at(&mut self, code: WarningCode, start: usize, end: usize) -> Result<()> {
        self.warnings.push(Warning { code, offset: start, end: Some(end) });
        Ok(())
    }

    // ── Main line loop ──────────────────────────────────────────────────────

    fn run(&mut self) -> Result<()> {
        while self.pos < self.source.len() {
            let line_start = self.pos;
            self.cur_line_start = line_start;
            if let Err(e) = self.process_line(line_start) {
                // In recover mode, a per-line failure is recorded and the
                // scanner resyncs to the next line so the rest of the file
                // still reports. OOM is never a document defect.
                if !self.recover || e.is("OutOfMemory") {
                    return Err(e);
                }
                self.record_recoverable(e, line_start)?;
            }
        }
        // Container-close and comment-drain that trail the last line. A
        // failure here (e.g. an empty container at EOF) recovers the same way,
        // but with no line left to resync to.
        if let Err(e) = self.finish_run() {
            if !self.recover || e.is("OutOfMemory") {
                return Err(e);
            }
            let end = self.source.len();
            self.record_recoverable(e, end)?;
        }
        Ok(())
    }

    /// One physical line: indent lint, blank/comment shortcuts, then the
    /// content dispatch.
    fn process_line(&mut self, line_start: usize) -> Result<()> {
        let source = self.source;
        self.skip_spaces_tabs(); // cosmetic indent — never load-bearing (linted below)
        let indent = &source[line_start..self.pos];
        let m = self.scan_markers()?;
        // A TRULY blank line (nothing at all, not even a comment) is skipped
        // outright — before the indent lint. This must NOT reuse
        // `at_end_of_content` (which also treats a leading `#` as "content is
        // over") — a comment-only line has to fall through to the `#`
        // handling below.
        if !m.star && self.at_true_line_end() {
            self.skip_to_next_line();
            return Ok(());
        }
        // Indent/count lint (DESIGN.md "Depth diagnostics"): meaning lives in
        // the `>` count alone, but indentation — when present — is the
        // redundant second signal. Convention: `2 × depth` spaces, no tabs.
        // Comment lines are linted too: their depth is load-bearing for
        // attachment.
        if !indent.is_empty() {
            let tabbed = indent.contains(&b'\t');
            // Anchored at the first char AFTER the indent, through the marker
            // run (`self.pos` sits past it after `scan_markers`).
            if tabbed || indent.len() != 2 * m.depth as usize {
                let pos = self.pos;
                self.warn_at(WarningCode::IndentMarkerMismatch, line_start + indent.len(), pos)?;
            }
        }
        // A `*` element marker already committed this to an element line, so
        // skip the comment-only shortcut below.
        if !m.star && self.peek() == Some(b'#') {
            self.advance();
            let mut j = self.pos;
            while j < source.len() && source[j] != b'\n' {
                j += 1;
            }
            let text = tok::trim(&source[self.pos..j], b" \t\r");
            self.pending_leading.push(PendingComment { comment: Comment::line(text), depth: m.depth });
            self.pos = j;
            self.skip_to_next_line();
            return Ok(());
        }
        self.process_content_line(m.depth, m.star_span)
    }

    /// Trailing work after the last line is consumed: close every still-open
    /// frame back to root, and flush any dangling comments to the root.
    fn finish_run(&mut self) -> Result<()> {
        self.close_frames_above(0)?;
        for pc in &self.pending_leading {
            self.w.comment(self.root, table::COMMENT_DANGLING, pc.comment)?;
        }
        self.pending_leading.clear();
        Ok(())
    }

    /// Record a recoverable error at its diagnostic offset, then resync the
    /// scanner past the offending line so the loop keeps making progress.
    fn record_recoverable(&mut self, e: Error, line_start: usize) -> Result<()> {
        self.diagnostics.push(Diagnostic { code: e, offset: self.fail_offset.unwrap_or(self.pos), end: self.fail_end });
        // Consumed — do not leak this failure's anchors into the next line's
        // report.
        self.fail_offset = None;
        self.fail_end = None;
        self.recover_to_next_line(line_start);
        Ok(())
    }

    /// Resync to the start of the line after the one the failure began on, so
    /// the loop keeps reporting. `line_start` anchors this — NOT the raw
    /// cursor, which for a late-detected error (a duplicate key noticed only
    /// after its line's newline was consumed) already sits at the next line's
    /// start; scanning from there would swallow that still-unread line.
    fn recover_to_next_line(&mut self, line_start: usize) {
        let source = self.source;
        let mut eol = line_start;
        while eol < source.len() && source[eol] != b'\n' {
            eol += 1;
        }
        let next = if eol < source.len() { eol + 1 } else { eol };
        if self.pos <= next {
            // A single-line failure — resume at `next`: the failed line is
            // skipped, the following one kept.
            self.pos = next;
        } else {
            // The failing construct (a `'''` string / `[`…`]` flow) already
            // consumed past this line — resume after wherever it stopped.
            let mut at = self.pos;
            while at < source.len() && source[at] != b'\n' {
                at += 1;
            }
            self.pos = if at < source.len() { at + 1 } else { at };
        }
    }

    fn process_content_line(&mut self, depth: u32, star_span: Option<Span>) -> Result<()> {
        let star = star_span.is_some();
        self.close_frames_above(depth)?;
        let required = self.stack.last().map_or(0, |f| f.child_depth);
        if depth != required {
            return Err(if required == 0 { err!(FigRootMarker) } else { err!(FigSkippedLevel) });
        }
        let target = self.stack.last().map_or(self.root, |f| f.container);

        // A lone `+` (optionally with a trailing comment) is a continuation
        // line: it re-runs the most recent zero-marker `[]` append header. Any
        // other zero-marker structural line breaks the chain.
        let is_plus = !star && self.peek() == Some(b'+') && self.is_plus_line();
        if depth == 0 && !is_plus {
            self.last_append_steps = None;
        }
        if is_plus {
            return self.parse_continuation_line(depth);
        }

        // `star` = the prefix carried a `*` element marker (scan_markers
        // consumed it, along with the separator).
        if let Some(s) = star_span {
            return self.parse_element_line(target, depth, s);
        }
        match self.peek().expect("a content line has content") {
            // A `-` element line is the YAML habit (and fig's own pre-`*`
            // spelling) — hard error naming the `*` form.
            b'-' => Err(err!(FigForeignSyntaxDash)),
            b'[' => Err(err!(FigForeignSyntaxBracket)),
            _ => self.parse_key_line(target, depth),
        }
    }

    /// `pos` sits on a `+`: is this a continuation line (`+` alone, or
    /// followed by whitespace / a comment / end-of-line)? Anything glued to
    /// the `+` falls through to the key path (where `+` is not a bare-key char
    /// → `FigBadKey`).
    fn is_plus_line(&self) -> bool {
        let j = self.pos + 1;
        if j >= self.source.len() {
            return true;
        }
        matches!(self.source[j], b' ' | b'\t' | b'\r' | b'\n' | b'#')
    }

    /// A `+` continuation: append another element to the sequence of the most
    /// recent `[]` append header and re-anchor the baseline to it, exactly as
    /// if the header line had been written again.
    fn parse_continuation_line(&mut self, depth: u32) -> Result<()> {
        let plus = Span::new(self.pos, self.pos + 1);
        self.advance(); // '+'
        if depth != 0 {
            return Err(err!(FigDanglingContinuation));
        }
        let steps = self.last_append_steps.clone().ok_or(err!(FigDanglingContinuation))?;
        self.skip_spaces_tabs();
        if !self.at_end_of_content() {
            return Err(err!(FigTrailingContent));
        }
        let parent = self.navigate_intermediate(self.root, &steps, true)?;
        let resolved = self.resolve_header_final(parent, &steps[steps.len() - 1])?;
        // The `+` stands for the header it re-runs: should the element before
        // go, with that header, the header is written here.
        self.w.mention(resolved.container, plus, MentionKind::Append)?;
        self.flush_leading(resolved.leading)?;
        if let Some(cm) = self.scan_trailing_comment_only()? {
            self.trailing(resolved.container, cm)?;
        }
        self.consume_line_end();
        self.stack.push(Frame { container: resolved.container, child_depth: 1 });
        Ok(())
    }

    /// Pop frames deeper than `depth`, validating each closes non-empty.
    /// Buffered comments written at (or below) a closing frame's child depth
    /// were inside that container — they become its dangling run; shallower
    /// ones stay pending for the next sibling line (its leading run).
    fn close_frames_above(&mut self, depth: u32) -> Result<()> {
        while let Some(&top) = self.stack.last()
            && top.child_depth > depth
        {
            let frame = self.stack.pop().unwrap();
            let info = *self.info_of(frame.container).unwrap();
            if info.shape == Shape::Undecided {
                return Err(err!(FigEmptyContainer));
            }
            // A `[]`-appended element was pre-committed to map shape at birth,
            // so the `Undecided` check above never sees it — but one that
            // closes with zero entries is the same authoring mistake.
            if info.born_of_append && self.rows()[frame.container as usize].first_child == NONE {
                return Err(err!(FigEmptyContainer));
            }
            if !self.pending_leading.is_empty() {
                // Partition in place: comments at/below the closing frame's
                // child depth drain to its dangling run; the rest compact to
                // the front of the buffer.
                let mut write = 0usize;
                for i in 0..self.pending_leading.len() {
                    let pc = self.pending_leading[i];
                    if pc.depth >= frame.child_depth {
                        self.w.comment(frame.container, table::COMMENT_DANGLING, pc.comment)?;
                    } else {
                        self.pending_leading[write] = pc;
                        write += 1;
                    }
                }
                self.pending_leading.truncate(write);
            }
        }
        Ok(())
    }

    /// Write the buffered comment lines as `id`'s leading comments, in source
    /// order, and empty the buffer.
    fn flush_leading(&mut self, id: NodeId) -> Result<()> {
        for pc in &self.pending_leading {
            self.w.comment(id, table::COMMENT_LEADING, pc.comment)?;
        }
        self.pending_leading.clear();
        Ok(())
    }

    /// `text` as `id`'s trailing comment, replacing any it had
    /// (`RowWriter::finish` keeps the last).
    fn trailing(&mut self, id: NodeId, text: &[u8]) -> Result<()> {
        self.w.comment(id, table::COMMENT_TRAILING, Comment::line(text))
    }

    // ── Markers ──────────────────────────────────────────────────────────────

    /// Count a run of `>` markers (spaced runs like `> > >` count the same as
    /// `>>>`), consume an optional `*` element marker ending the run (`> *`,
    /// `>*`, or a bare `*` at root), then require exactly one whitespace
    /// separator before the body — unless the rest of the line is empty/a
    /// comment (a marker-only line: `>>` is blank, `*` is a map-element
    /// opener). The `*` is a role suffix, not a counted mark: depth is the `>`
    /// count alone. A `-` where the element marker would sit is the YAML habit
    /// — a hard error naming `*`.
    fn scan_markers(&mut self) -> Result<Markers> {
        let mut count: u32 = 0;
        'scan: while self.peek() == Some(b'>') {
            count += 1;
            self.advance();
            while self.peek() == Some(b' ') || self.peek() == Some(b'\t') {
                let save = self.pos;
                self.skip_spaces_tabs();
                if self.peek() == Some(b'*') {
                    break 'scan; // spaced `> *` ends the run
                }
                if self.peek() != Some(b'>') {
                    self.pos = save;
                    break;
                }
            }
        }
        // `>-` glued to the run — the old dash element spelling.
        if count > 0 && self.peek() == Some(b'-') {
            return Err(err!(FigForeignSyntaxDash));
        }
        if self.peek() == Some(b'*') {
            let star_span = Span::new(self.pos, self.pos + 1);
            self.advance();
            if self.peek() == Some(b' ') || self.peek() == Some(b'\t') {
                self.skip_spaces_tabs();
            } else if self.peek() == Some(b':') {
                // Typed element `*: type = v`: the `:` binds to the
                // positional-key `*` (mirrors `key: type`), so no separator
                // space is required — leave `pos` on the `:`.
            } else if !self.at_end_of_content() {
                return Err(err!(FigBadMarkerSeparator));
            }
            return Ok(Markers { depth: count, star: true, star_span: Some(star_span) });
        }
        if count > 0 {
            if self.peek() == Some(b' ') || self.peek() == Some(b'\t') {
                self.skip_spaces_tabs();
            } else if !self.at_end_of_content() {
                return Err(err!(FigBadMarkerSeparator));
            }
        }
        Ok(Markers { depth: count, star: false, star_span: None })
    }

    // ── Header / assignment lines ────────────────────────────────────────────

    /// A `: type` annotation's name, after the `:` (`pos` on the `:`), and the
    /// `=` that must follow it; `FigBadKey` for an empty name and
    /// `FigForeignSyntaxColon` (pinned to the `:`) for a missing `=`. Leaves
    /// `pos` on the `=`.
    fn scan_type_annotation(&mut self) -> Result<&'s [u8]> {
        let source = self.source;
        let colon_pos = self.pos;
        self.advance();
        self.skip_spaces_tabs();
        let type_start = self.pos;
        while self.pos < source.len() && tok::is_bare_key_char(source[self.pos]) {
            self.pos += 1;
        }
        let type_name = &source[type_start..self.pos];
        if type_name.is_empty() {
            return Err(err!(FigBadKey));
        }
        self.skip_spaces_tabs();
        // `key: value` (the YAML habit) — the `:` is the offending token, not
        // the spot where the missing `=` was noticed, so pin the caret there.
        if self.peek() != Some(b'=') {
            return Err(self.fail_at(colon_pos, err!(FigForeignSyntaxColon)));
        }
        Ok(type_name)
    }

    fn parse_key_line(&mut self, target: NodeId, depth: u32) -> Result<()> {
        let path_start = self.pos;
        let steps = self.scan_key_path()?;
        let path = Span::new(path_start, self.pos);
        self.skip_spaces_tabs();
        if self.peek() == Some(b':') {
            let type_name = self.scan_type_annotation()?;
            let eq = Span::new(self.pos, self.pos + 1);
            self.advance();
            self.finish_assignment(target, &steps, Some(type_name), eq)
        } else if self.peek() == Some(b'=') {
            let eq = Span::new(self.pos, self.pos + 1);
            self.advance();
            self.finish_assignment(target, &steps, None, eq)
        } else {
            self.finish_header(target, &steps, path, depth)
        }
    }

    /// A header line whose key path is `steps`, written at `path`.
    fn finish_header(&mut self, target: NodeId, steps: &[Step<'s>], path: Span, depth: u32) -> Result<()> {
        if !self.at_end_of_content() {
            return Err(err!(FigTrailingContent));
        }
        let parent = self.navigate_intermediate(target, steps, false)?;
        let last = &steps[steps.len() - 1];
        let resolved = self.resolve_header_final(parent, last)?;
        let appends = matches!(last, Step::Index(IndexKind::AppendOrLast));
        // The element a header-final `[]` appends is written by this path,
        // which a `+` after it repeats (`MentionKind::Append`).
        if appends {
            self.w.mention(resolved.container, path, MentionKind::Append)?;
        }
        self.flush_leading(resolved.leading)?;
        if let Some(cm) = self.scan_trailing_comment_only()? {
            self.trailing(resolved.container, cm)?;
        }
        self.consume_line_end();
        self.stack.push(Frame { container: resolved.container, child_depth: depth + 1 });
        // A zero-marker header whose FINAL step is `[]` arms the `+`
        // continuation (process_content_line already cleared any previous
        // chain for this line).
        if depth == 0 && appends {
            self.last_append_steps = Some(steps.to_vec());
        }
        Ok(())
    }

    fn finish_assignment(&mut self, target: NodeId, steps: &[Step<'s>], type_name: Option<&'s [u8]>, sep: Span) -> Result<()> {
        self.skip_spaces_tabs();
        let parent = self.navigate_intermediate(target, steps, false)?.container;
        let last = &steps[steps.len() - 1];
        let value = self.parse_assigned_value(type_name)?;
        self.consume_line_end();
        match last {
            Step::Key(k) => {
                self.as_mapping(parent)?;
                // `consume_line_end` above has already advanced `self.pos` past
                // this line's newline — pin the diagnostic to the actual
                // conflicting key's own span instead.
                if self.children.get(&self.w, parent, &k.name).is_some() {
                    return Err(self.fail_span(k.span.start, k.span.end, err!(FigDuplicateKey)));
                }
                let kv = self.add_entry(parent, k, value, Some(sep))?;
                let key = self.key_of(kv);
                self.flush_leading(key)
            }
            Step::Index(idx) => {
                self.as_sequence(parent)?;
                self.mark_addressed(parent)?;
                match *idx {
                    IndexKind::Literal { n, span } => {
                        let len = self.element_count(parent);
                        if n < len {
                            return Err(err!(FigIndexAlreadySet));
                        }
                        if n > len {
                            return Err(err!(FigIndexSkipped));
                        }
                        self.flush_leading(value)?;
                        self.append_element(parent, value)?;
                        self.index_mention(value, span)
                    }
                    // `xs[] = v` is not assignable: `[]` appends only as a
                    // header (or as a mid-path "last element" step).
                    IndexKind::AppendOrLast => Err(err!(FigAppendAssignment)),
                }
            }
        }
    }

    // ── Element (`*`) lines ──────────────────────────────────────────────────

    /// The element `*` marker (and its separator) has already been consumed by
    /// `scan_markers`; `pos` sits on the element body (or a typing `:`).
    fn parse_element_line(&mut self, target: NodeId, depth: u32, star: Span) -> Result<()> {
        let source = self.source;
        // The position right after the `> *` marker prefix — before any
        // trailing whitespace is skipped — so a bare `*` opener still anchors
        // its container's span at a real position on its own line.
        let body_start = self.pos;
        self.skip_spaces_tabs();
        self.as_sequence(target)?;
        self.mark_element(target)?;

        if self.at_end_of_content() {
            let child = self.new_block_container(Span::new(body_start, body_start), ContainerInfo::default())?;
            self.flush_leading(child)?;
            self.add_element(target, child, star)?;
            if let Some(cm) = self.scan_trailing_comment_only()? {
                self.trailing(child, cm)?;
            }
            self.consume_line_end();
            self.stack.push(Frame { container: child, child_depth: depth + 1 });
            return Ok(());
        }

        if self.peek() == Some(b':') {
            // Same caret pin as `parse_key_line`: the `:` is the offending
            // token.
            let type_name = self.scan_type_annotation()?;
            self.advance();
            let el = self.parse_assigned_value(Some(type_name))?;
            self.consume_line_end();
            self.flush_leading(el)?;
            return self.add_element(target, el, star);
        }

        let c = self.peek().unwrap();
        if c == b'\'' || c == b'"' || c == b'[' || c == b'{' {
            let el = self.parse_untyped_value(false)?;
            self.consume_line_end();
            self.flush_leading(el)?;
            return self.add_element(target, el, star);
        }

        // Bare value: the "no inline field on an element" guardrail runs
        // BEFORE sniffing, on the raw (untrimmed) rest of the line.
        let start = self.pos;
        let mut k = start;
        while k < source.len() && source[k] != b'\n' {
            k += 1;
        }
        if contains(&source[start..k], b" = ") {
            return Err(err!(FigElementInlineField));
        }

        let el = self.parse_untyped_value(false)?;
        self.consume_line_end();
        self.flush_leading(el)?;
        self.add_element(target, el, star)
    }

    // ── Key-path scanning (shared by headers and assignments) ───────────────

    fn scan_key_path(&mut self) -> Result<Vec<Step<'s>>> {
        // Paths are short (usually 1-3 segments).
        let mut steps: Vec<Step<'s>> = Vec::with_capacity(4);
        loop {
            let key = self.scan_key_seg()?;
            steps.push(Step::Key(key));
            while self.peek() == Some(b'[') {
                steps.push(Step::Index(self.scan_index_seg()?));
            }
            if self.peek() == Some(b'.') {
                self.advance();
                continue;
            }
            break;
        }
        Ok(steps)
    }

    fn scan_key_seg(&mut self) -> Result<KeySeg<'s>> {
        let source = self.source;
        let c = self.peek().ok_or(err!(FigBadKey))?;
        let start = self.pos;
        if c == b'"' {
            let r = tok::scan_double_quoted(source, self.pos)?;
            self.pos = r.end;
            return Ok(KeySeg { name: Cow::Owned(r.text), span: Span::new(start, r.end) });
        }
        if c == b'\'' {
            let r = tok::scan_single_quoted(source, self.pos)?;
            self.pos = r.end;
            return Ok(KeySeg { name: Cow::Owned(r.text), span: Span::new(start, r.end) });
        }
        while self.pos < source.len() && tok::is_bare_key_char(source[self.pos]) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(err!(FigBadKey));
        }
        let text = &source[start..self.pos];
        if text[0] == b'-' || text[0] == b'>' {
            return Err(err!(FigBadKey));
        }
        Ok(KeySeg { name: Cow::Borrowed(text), span: Span::new(start, self.pos) })
    }

    fn scan_index_seg(&mut self) -> Result<IndexKind> {
        let source = self.source;
        debug_assert!(self.peek() == Some(b'['));
        self.advance();
        if self.peek() == Some(b']') {
            self.advance();
            return Ok(IndexKind::AppendOrLast);
        }
        let start = self.pos;
        while self.pos < source.len() && source[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == start || self.peek() != Some(b']') {
            return Err(err!(FigBadKey));
        }
        let n = tok::zig_parse_unsigned(&source[start..self.pos], 10, usize::BITS).ok_or(err!(FigBadKey))? as usize;
        let span = Span::new(start, self.pos);
        self.advance(); // ']'
        Ok(IndexKind::Literal { n, span })
    }

    // ── Rows and containers ──────────────────────────────────────────────────

    fn rows(&self) -> &[table::Row] {
        &self.w.t.rows
    }

    /// Container `id`'s state, or `None` when `id` is not a container.
    fn info_of(&mut self, id: NodeId) -> Option<&mut ContainerInfo> {
        self.containers.get_mut(&id)
    }

    /// A new, childless container row over `span` — an empty mapping until its
    /// first child decides it, unless `info` already has.
    fn new_container(&mut self, span: Span, info: ContainerInfo) -> Result<NodeId> {
        let kind = if info.shape == Shape::Sequence { NodeKind::Sequence(None) } else { NodeKind::Mapping(None) };
        let id = self.w.add(kind, span)?;
        self.containers.insert(id, info);
        Ok(id)
    }

    /// `new_container` for a block container — one a header line, a `*`
    /// opener or a dotted path makes, as opposed to a flow value or the root —
    /// which also records the line holding `span.start` as its first header
    /// line (`Document::regions_of`).
    fn new_block_container(&mut self, span: Span, info: ContainerInfo) -> Result<NodeId> {
        let id = self.new_container(span, info)?;
        self.region(id, span.start)?;
        Ok(id)
    }

    /// The line holding `at` as one of block container `id`'s header lines.
    fn region(&mut self, id: NodeId, at: usize) -> Result<()> {
        let line = document::line_region_at(self.source, at);
        self.w.region(id, line.start, line.end)
    }

    /// `span`, a literal index on this line, as where element `id` is
    /// addressed (`MentionKind::Index`). A `+` re-runs a header written on an
    /// earlier line, and that line recorded its indices.
    fn index_mention(&mut self, id: NodeId, span: Span) -> Result<()> {
        if span.start < self.cur_line_start {
            return Ok(());
        }
        self.w.mention(id, span, MentionKind::Index)
    }

    /// Container `id`, unless a flow value made it — those are closed to later
    /// extension (dotted paths, headers, indexing).
    fn open(&mut self, id: NodeId) -> Result<NodeId> {
        if self.info_of(id).unwrap().closed { Err(err!(FigClosedFlowValue)) } else { Ok(id) }
    }

    fn as_mapping(&mut self, id: NodeId) -> Result<()> {
        let info = self.info_of(id).unwrap();
        match info.shape {
            Shape::Undecided => info.shape = Shape::Mapping,
            Shape::Mapping => {}
            Shape::Sequence => return Err(err!(FigMixedContainerChildren)),
        }
        Ok(())
    }

    fn as_sequence(&mut self, id: NodeId) -> Result<()> {
        let info = self.info_of(id).unwrap();
        match info.shape {
            Shape::Undecided => {
                info.shape = Shape::Sequence;
                self.w.set(id, NodeKind::Sequence(None))?;
            }
            Shape::Sequence => {}
            Shape::Mapping => return Err(err!(FigMixedContainerChildren)),
        }
        Ok(())
    }

    fn mark_element(&mut self, id: NodeId) -> Result<()> {
        let info = self.info_of(id).unwrap();
        match info.style {
            Style::Undecided => info.style = Style::Element,
            Style::Element => {}
            Style::Addressed => return Err(err!(FigMixedSequenceAddressing)),
        }
        Ok(())
    }

    fn mark_addressed(&mut self, id: NodeId) -> Result<()> {
        let info = self.info_of(id).unwrap();
        match info.style {
            Style::Undecided => {
                info.style = Style::Addressed;
                // Only a flow array has elements before its style is decided,
                // and a flow array is closed before it can be addressed.
                debug_assert!(self.rows()[id as usize].first_child == NONE);
                self.addressed.insert(id, Vec::new());
            }
            Style::Addressed => {}
            Style::Element => return Err(err!(FigMixedSequenceAddressing)),
        }
        Ok(())
    }

    /// Index-addressed sequence `id`'s elements so far, in order.
    fn elements_of(&self, id: NodeId) -> &[NodeId] {
        self.addressed.get(&id).expect("an addressed sequence")
    }

    fn element_count(&self, id: NodeId) -> usize {
        self.elements_of(id).len()
    }

    /// Link `el` as sequence `seq`'s last element.
    fn append_element(&mut self, seq: NodeId, el: NodeId) -> Result<()> {
        self.children.append(&mut self.w, seq, el)?;
        if self.info_of(seq).unwrap().style == Style::Addressed {
            self.addressed.get_mut(&seq).unwrap().push(el);
        }
        Ok(())
    }

    /// `append_element` for an element a `*` marker introduced.
    fn add_element(&mut self, seq: NodeId, el: NodeId, star: Span) -> Result<()> {
        self.append_element(seq, el)?;
        self.w.marker(el, star)
    }

    /// Link `key = value` as mapping `map`'s last entry, with `sep` its
    /// separator when it has one (`Document::sep_span`), and return the
    /// entry's row. Its span reaches its value's final end in `finish_spans`.
    fn add_entry(&mut self, map: NodeId, key: &KeySeg<'s>, value: NodeId, sep: Option<Span>) -> Result<NodeId> {
        let key_id = self.w.add(NodeKind::String(&key.name), key.span)?;
        let end = self.w.span_of(value).end;
        let kv = self.w.add(NodeKind::KeyValue(KeyValue { key: key_id, value }), Span::new(key.span.start, end))?;
        if let Some(s) = sep {
            self.w.sep(kv, s)?;
        }
        self.children.append(&mut self.w, map, kv)?;
        self.children.put(&self.w, kv)?;
        Ok(kv)
    }

    /// Entry `kv`'s key row — where its leading comments bind.
    fn key_of(&self, kv: NodeId) -> NodeId {
        self.rows()[kv as usize].first_child
    }

    fn value_of(&self, kv: NodeId) -> NodeId {
        self.rows()[self.key_of(kv) as usize].next_sibling
    }

    // ── Path navigation ──────────────────────────────────────────────────────

    /// Resolve every step except the last, creating/reusing intermediate
    /// containers. `[]`/`[i]` here always mean "the last existing element"
    /// (never creates) — only a HEADER line's OVERALL final `[]` appends. Each
    /// key step is a mention of the container it reaches, made or re-opened,
    /// unless the line is a `+` re-running a header (`rerun`): its steps are
    /// that header's, whose keys are already recorded.
    fn navigate_intermediate(&mut self, start: NodeId, steps: &[Step<'s>], rerun: bool) -> Result<Reached> {
        let mut cur = Reached { container: start, own: true };
        for step in &steps[..steps.len() - 1] {
            let len = self.rows().len();
            match step {
                Step::Key(k) => {
                    self.as_mapping(cur.container)?;
                    let kind = if rerun { None } else { Some(cur.kind()) };
                    cur.container = self.get_or_create_map_container(cur.container, k, false, kind)?;
                }
                Step::Index(idx) => {
                    self.as_sequence(cur.container)?;
                    self.mark_addressed(cur.container)?;
                    cur.container = self.get_or_create_seq_container(cur.container, *idx)?;
                }
            }
            // A container made by this step is this line's; one it re-opened
            // was written on an earlier line.
            cur.own = cur.container as usize >= len;
        }
        Ok(cur)
    }

    /// Mapping `m`'s container under `seg`, made when there is none. One a
    /// block path makes is a block container named by `seg`; one a flow
    /// object's dotted key makes (`flow`) is closed with its object, and is
    /// neither. `seg` is a `kind` mention of a block container, the one it
    /// makes or the one it re-opens. A `None` `kind` records none.
    fn get_or_create_map_container(&mut self, m: NodeId, seg: &KeySeg<'s>, flow: bool, kind: Option<MentionKind>) -> Result<NodeId> {
        if let Some(kv) = self.children.get(&self.w, m, &seg.name) {
            let v = self.value_of(kv);
            if self.info_of(v).is_none() {
                return Err(err!(FigKeyNotContainer));
            }
            let c = self.open(v)?;
            if !flow && let Some(k) = kind {
                self.w.mention(c, seg.span, k)?;
            }
            return Ok(c);
        }
        let child = if flow {
            self.new_container(seg.span, ContainerInfo::default())?
        } else {
            self.new_block_container(seg.span, ContainerInfo::default())?
        };
        // A header has no `=`; its container's body hangs under the key, and a
        // replacement value is reframed from the key's end. A zero-width
        // separator there says so.
        self.add_entry(m, seg, child, Some(Span::new(seg.span.end, seg.span.end)))?;
        if !flow && let Some(k) = kind {
            self.w.mention(child, seg.span, k)?;
        }
        Ok(child)
    }

    fn get_or_create_seq_container(&mut self, s: NodeId, idx: IndexKind) -> Result<NodeId> {
        let len = self.element_count(s);
        match idx {
            IndexKind::Literal { n, span } => {
                if n < len {
                    let el = self.elements_of(s)[n];
                    if self.info_of(el).is_none() {
                        return Err(err!(FigKeyNotContainer));
                    }
                    let el = self.open(el)?;
                    self.index_mention(el, span)?;
                    Ok(el)
                } else if n == len {
                    let at = self.cur_line_start;
                    let child = self.new_block_container(Span::new(at, at), ContainerInfo::default())?;
                    self.append_element(s, child)?;
                    self.index_mention(child, span)?;
                    Ok(child)
                } else {
                    Err(err!(FigIndexSkipped))
                }
            }
            IndexKind::AppendOrLast => {
                if len == 0 {
                    return Err(err!(FigEmptyAppendTarget));
                }
                let last = self.elements_of(s)[len - 1];
                if self.info_of(last).is_none() {
                    return Err(err!(FigKeyNotContainer));
                }
                self.open(last)
            }
        }
    }

    /// Resolve a HEADER line's final path segment: get-or-create the container
    /// to push as the next frame. A plain key may be re-entered (fine, per
    /// DESIGN.md); a header-final `[]` always appends a fresh, forced-mapping
    /// element.
    fn resolve_header_final(&mut self, reached: Reached, last: &Step<'s>) -> Result<Resolved> {
        let parent = reached.container;
        match last {
            Step::Key(k) => {
                self.as_mapping(parent)?;
                if let Some(kv) = self.children.get(&self.w, parent, &k.name) {
                    let v = self.value_of(kv);
                    if self.info_of(v).is_none() {
                        return Err(err!(FigDuplicateKey));
                    }
                    let c = self.open(v)?;
                    // A header re-opening an existing container (DESIGN.md
                    // "re-entering a path to add new keys is fine"): this line
                    // is one of its header lines too, and its key one more
                    // place its name is written. Only header-FINAL re-opens
                    // count.
                    self.region(c, k.span.start)?;
                    self.w.mention(c, k.span, reached.kind())?;
                    return Ok(Resolved { container: c, leading: self.key_of(kv) });
                }
                let child = self.new_block_container(k.span, ContainerInfo::default())?;
                // A header line's container hangs under the key; a replacement
                // value reframes from the key's end (a zero-width separator).
                let kv = self.add_entry(parent, k, child, Some(Span::new(k.span.end, k.span.end)))?;
                self.w.mention(child, k.span, reached.kind())?;
                Ok(Resolved { container: child, leading: self.key_of(kv) })
            }
            Step::Index(idx) => {
                self.as_sequence(parent)?;
                self.mark_addressed(parent)?;
                match *idx {
                    IndexKind::Literal { n, span } => {
                        let len = self.element_count(parent);
                        if n < len {
                            let el = self.elements_of(parent)[n];
                            if self.info_of(el).is_none() {
                                return Err(err!(FigKeyNotContainer));
                            }
                            let c = self.open(el)?;
                            // An `xs[i]` header re-opening an existing element
                            // — the index-addressed twin of the key re-open
                            // above. The line start is the same-line anchor
                            // the region gather needs.
                            let at = self.cur_line_start;
                            self.region(c, at)?;
                            self.index_mention(c, span)?;
                            Ok(Resolved { container: c, leading: c })
                        } else if n == len {
                            let at = self.cur_line_start;
                            let child = self.new_block_container(Span::new(at, at), ContainerInfo::default())?;
                            self.append_element(parent, child)?;
                            self.index_mention(child, span)?;
                            Ok(Resolved { container: child, leading: child })
                        } else {
                            Err(err!(FigIndexSkipped))
                        }
                    }
                    IndexKind::AppendOrLast => {
                        // Append-created elements are always map-shaped.
                        let at = self.cur_line_start;
                        let info = ContainerInfo { shape: Shape::Mapping, born_of_append: true, ..ContainerInfo::default() };
                        let child = self.new_block_container(Span::new(at, at), info)?;
                        self.append_element(parent, child)?;
                        Ok(Resolved { container: child, leading: child })
                    }
                }
            }
        }
    }

    // ── Values ───────────────────────────────────────────────────────────────

    /// A value's row, written with its trailing comment and type tag; each
    /// value parser below returns the row it wrote.
    fn parse_assigned_value(&mut self, type_name: Option<&'s [u8]>) -> Result<NodeId> {
        self.skip_spaces_tabs();
        if let Some(t) = type_name {
            if t == b"string" {
                // `: string =` selects the raw-text sub-parser AND records the
                // tag, so the annotation round-trips and `fig fmt` can re-emit
                // the value bare.
                let id = self.parse_untyped_value(true)?;
                self.w.tag(id, Tag::Kind(KindTag::String), 0, true, None)?;
                return Ok(id);
            }
            let res = self.scan_bare_rest_of_line();
            if res.text.is_empty() {
                return Err(err!(FigInvalidValue));
            }
            // `scan_bare_rest_of_line` has parked `pos` at end-of-line (past
            // any trailing comment), so pin the diagnostic to the value's own
            // span instead — that is what failed its annotation.
            let vspan = self.span_of(res.text);
            let typed = match self.apply_known_type(t, res.text) {
                Ok(typed) => typed,
                Err(e) => return Err(self.fail_span(vspan.start, vspan.end, e)),
            };
            let id = match &typed.kind {
                TypedKind::Kind(k) => self.w.add(*k, vspan)?,
                TypedKind::Char(raw) => self.w.add(NodeKind::Extended(Extended { kind: ExtKind::CharLiteral, text: raw }), vspan)?,
            };
            if let Some(tag) = typed.tag {
                self.w.tag(id, Tag::Kind(tag), 0, true, None)?;
            }
            if let Some(cm) = res.comment {
                self.trailing(id, cm)?;
            }
            return Ok(id);
        }
        self.parse_untyped_value(false)
    }

    /// A scalar row of `kind` over `span`, with `comment` as its trailing
    /// comment.
    fn scalar(&mut self, kind: NodeKind<'_>, span: Span, comment: Option<&[u8]>) -> Result<NodeId> {
        let id = self.w.add(kind, span)?;
        if let Some(cm) = comment {
            self.trailing(id, cm)?;
        }
        Ok(id)
    }

    /// `force_string_bare`: set only for `: string =`, where the ENTIRE RHS is
    /// one raw text run — the annotation is a TOTAL SINK that selects the
    /// raw-text sub-parser (DESIGN.md "annotation selects the sub-parser"),
    /// turning off sniffing, flow commitment AND the quote forms. Quote
    /// characters are literal content, backslashes are literal, and a
    /// `'''`/`"""` opener is content too. The trailing-`#`-comment rule is
    /// exactly the bare rule (`scan_bare_rest_of_line`).
    fn parse_untyped_value(&mut self, force_string_bare: bool) -> Result<NodeId> {
        if force_string_bare {
            // So `x: string = [ 1 + 2 ]` is the string `[ 1 + 2 ]`, never a
            // sequence.
            let res = self.scan_bare_rest_of_line();
            if res.text.is_empty() {
                return Err(err!(FigInvalidValue));
            }
            let span = self.span_of(res.text);
            return self.scalar(NodeKind::String(res.text), span, res.comment);
        }
        let c = self.peek().ok_or(err!(FigInvalidValue))?;
        match c {
            b'\'' => self.parse_quoted_or_triple(b'\''),
            b'"' => self.parse_quoted_or_triple(b'"'),
            b'[' | b'{' => {
                // Commitment is decided by the shape of the WHOLE RHS, not just
                // the first char (DESIGN.md "Committed values"): a balanced
                // `[…]`/`{…}` with trailing content — a markdown link, glob,
                // regex — was never flow, so it is a bare string, left
                // unquoted.
                if tok::classify_bracket_commit(self.source, self.pos) == BracketCommit::BareTrailing {
                    let vstart = self.pos;
                    if let Some(close) = self.flow_like_prefix(vstart)? {
                        self.warn_at(WarningCode::FlowLikeString, vstart, close + 1)?;
                    }
                    let res = self.scan_bare_rest_of_line();
                    if res.text.is_empty() {
                        return Err(err!(FigInvalidValue));
                    }
                    // Opened with a delimiter → sniffing is off; it is a string.
                    let span = self.span_of(res.text);
                    return self.scalar(NodeKind::String(res.text), span, res.comment);
                }
                // `Flow` (terminal close) and `Unclosed` (multi-line /
                // truncation) both commit: the flow parser succeeds or raises a
                // hard error.
                //
                // A `# …` after the opening `[`/`{`, with nothing else on the
                // line, is a block-layer trailing comment on the flow value —
                // the multiline-string opener rule's flow twin. An opener
                // comment wins over a close-line one.
                let opener_comment = self.scan_flow_opener_comment();
                let id = self.parse_flow_value()?;
                let close_comment = self.scan_trailing_comment_only()?;
                if let Some(cm) = opener_comment.or(close_comment) {
                    self.trailing(id, cm)?;
                }
                Ok(id)
            }
            _ => {
                let vstart = self.pos;
                let res = self.scan_bare_rest_of_line();
                if res.text.is_empty() {
                    return Err(err!(FigInvalidValue));
                }
                let kind = self.sniff_to_kind_warned(res.text, vstart)?;
                let span = self.span_of(res.text);
                self.scalar(kind, span, res.comment)
            }
        }
    }

    fn parse_quoted_or_triple(&mut self, q: u8) -> Result<NodeId> {
        let source = self.source;
        let start = self.pos;
        if self.is_triple_at(self.pos, q) {
            // The opener line is still block-layer: only whitespace and an
            // optional `# comment` may follow the opening delimiter — content
            // begins on the NEXT line (`x = '''abc'''` is NOT a one-line
            // multiline string).
            let mut j = self.pos + 3;
            while j < source.len() && (source[j] == b' ' || source[j] == b'\t') {
                j += 1;
            }
            if j < source.len() && source[j] != b'\n' && source[j] != b'#' && !self.is_crlf_at(j) {
                return Err(self.fail_at(j, err!(FigMultilineOpenerContent)));
            }
            let r = if q == b'\'' { tok::scan_triple_single(source, self.pos)? } else { tok::scan_triple_double(source, self.pos)? };
            self.pos = r.end;
            let close_comment = self.scan_trailing_comment_only()?;
            return self.scalar(NodeKind::String(&r.text), Span::new(start, r.end), r.opener_comment.or(close_comment));
        }
        let r = if q == b'\'' { tok::scan_single_quoted(source, self.pos)? } else { tok::scan_double_quoted(source, self.pos)? };
        self.pos = r.end;
        // A single-line quote that closed early with more content on the line
        // is almost always a bare string wrapped in unescaped quotes
        // (`k = "a "b""`). Remap the generic trailing-content error to the
        // quote-specific message; `scan_trailing_comment_only` has already
        // parked `self.pos` on the stray content, so the caret lands there.
        let t = match self.scan_trailing_comment_only() {
            Ok(t) => t,
            Err(e) if e.is("FigTrailingContent") => {
                let at = self.pos;
                return Err(self.fail_at(at, err!(FigQuotedTrailingContent)));
            }
            Err(e) => return Err(e),
        };
        self.scalar(NodeKind::String(&r.text), Span::new(start, r.end), t)
    }

    /// `sniff_to_kind` plus the coercion warns (DESIGN.md "Coercion
    /// diagnostics") — the sniffing cost surfaced rather than removed.
    /// `offset` anchors the warn at the value's first byte. Both bare-value
    /// positions (block RHS, flow element) route through here; explicitly
    /// typed values never do (the annotation is the author saying "I know").
    fn sniff_to_kind_warned(&mut self, text: &'s [u8], offset: usize) -> Result<NodeKind<'s>> {
        let kind = sniff_to_kind(text);
        let end = offset + text.len(); // the exact token extent, for a tight squiggle
        match kind {
            NodeKind::String(_) => {
                if looks_like_literal(text) {
                    self.warn_at(WarningCode::StringLooksLikeLiteral, offset, end)?;
                } else if looks_like_leading_zero(text) {
                    self.warn_at(WarningCode::StringLeadingZero, offset, end)?;
                } else if looks_like_trailing_dot_number(text) || looks_like_leading_dot_number(text) {
                    self.warn_at(WarningCode::StringLooksLikeNumber, offset, end)?;
                }
            }
            // Quietly, and only when ambiguous: a bare clock time reads as a
            // duration/ratio/score as easily as a time-of-day. A bare *date*
            // does not warn, and a `T`/zone-carrying timestamp is unambiguous.
            NodeKind::Extended(e) if e.kind == ExtKind::LocalTime => {
                self.warn_at(WarningCode::AmbiguousDatetime, offset, end)?;
            }
            _ => {}
        }
        Ok(kind)
    }

    /// Does a `BareTrailing` value at `start` *look* like it wanted to be
    /// flow? The index of the closing bracket when the balanced bracket prefix
    /// at `start` (a) detaches from the trailing content by whitespace and (b)
    /// is itself well-formed flow — i.e. the `FlowLikeString` warning should
    /// fire spanning `start..=close`. `None` otherwise: `[Blog](/x)` /
    /// `[a-z]*.md` / `[b]x[/b]` (glued: the intended bare-string shapes) and
    /// `[80,, 443] x` (prefix malformed: never parsed as flow in the first
    /// place).
    ///
    /// Well-formedness is decided by a speculative sub-parse of the prefix
    /// alone (which reuses the real flow grammar rather than re-deriving it
    /// here); its rows land in a table of its own, discarded with the
    /// sub-parser and its warnings.
    fn flow_like_prefix(&mut self, start: usize) -> Result<Option<usize>> {
        let source = self.source;
        let Some(close) = tok::bracket_close_index(source, start) else { return Ok(None) };
        if close + 1 >= source.len() {
            return Ok(None);
        }
        let sep = source[close + 1];
        if sep != b' ' && sep != b'\t' {
            return Ok(None); // glued trailing → intended string
        }
        let prefix = &source[start..close + 1];
        let mut scratch_table = Table::init(prefix);
        let mut sub = Parser::new(prefix, &mut scratch_table, false);
        match sub.parse_flow_value() {
            Ok(_) => {}
            Err(e) if e.is("OutOfMemory") => return Err(e),
            Err(_) => return Ok(None),
        }
        Ok(if sub.pos == prefix.len() { Some(close) } else { None })
    }

    /// Explicit typing (`key: type = value`): the annotation is checked and
    /// coercing, and also STORED as a cross-format type tag (`AST::tag_of`) so
    /// it round-trips (DESIGN.md "Explicit typing"). The value's VERBATIM
    /// lexeme is kept as the node's `raw`/`text` — the coercion normalizers
    /// below run only to *validate* (a genuine mismatch still errors), never
    /// to rewrite the bytes, so `1.`/`09` survive exactly. `enum`/`datetime`
    /// map to distinct `Extended` node kinds that self-annotate on print, so
    /// they carry no tag.
    fn apply_known_type(&mut self, type_name: &[u8], text: &'s [u8]) -> Result<Typed<'s>> {
        let integer = |raw| Typed { kind: TypedKind::Kind(NodeKind::Number(Number { raw, kind: NumberKind::Integer })), tag: Some(KindTag::Integer) };
        let float = |raw| Typed { kind: TypedKind::Kind(NodeKind::Number(Number { raw, kind: NumberKind::Float })), tag: Some(KindTag::Float) };
        let ext = |kind, text| Typed { kind: TypedKind::Kind(NodeKind::Extended(Extended { kind, text })), tag: None };
        // Resolve the annotation name once (an unknown name is
        // `FigUnknownType`), then switch.
        match type_name {
            b"int" => {
                if let Some(n) = tok::sniff_number(text) {
                    if n.kind != literal::NumberKind::Integer {
                        return Err(err!(FigTypeMismatch));
                    }
                    return Ok(integer(text));
                }
                // Coercion opt-in: a leading-zero decimal (`09`, `007`) is a
                // *string* when bare (the Leading-zero rule), but `: int` is
                // the author overriding that default. Validate the shape; keep
                // the authored lexeme verbatim so the padding round-trips.
                if normalize_decimal(text).is_some() {
                    return Ok(integer(text));
                }
                Err(err!(FigTypeMismatch))
            }
            b"float" => {
                if text == b"inf" || text == b"-inf" || text == b"nan" {
                    return Ok(ext(ExtKind::NumberSpecial, text));
                }
                if tok::sniff_number(text).is_some() {
                    return Ok(float(text));
                }
                // Coercion opt-in: a trailing-dot (`1.`) or leading-zero (`09`)
                // token is a string when bare, but `: float` overrides.
                if normalize_coerced_float(text).is_some() {
                    return Ok(float(text));
                }
                Err(err!(FigTypeMismatch))
            }
            b"bool" => {
                if text == b"true" {
                    return Ok(Typed { kind: TypedKind::Kind(NodeKind::Boolean(true)), tag: Some(KindTag::Boolean) });
                }
                if text == b"false" {
                    return Ok(Typed { kind: TypedKind::Kind(NodeKind::Boolean(false)), tag: Some(KindTag::Boolean) });
                }
                Err(err!(FigTypeMismatch))
            }
            b"enum" => {
                if text.is_empty() {
                    return Err(err!(FigTypeMismatch));
                }
                Ok(ext(ExtKind::EnumLiteral, text))
            }
            b"char" => {
                // `: char = 'A'` — a single ZON-style char literal. The
                // annotation is the disambiguator: it turns a `'…'` RHS, which
                // is a length-1 string bare, into a `CharLiteral` extended
                // scalar, stored as the decimal codepoint — the cross-format
                // `char_literal` invariant. Only `'…'` is a char literal
                // (§ 5.3).
                if text.len() < 2 || text[0] != b'\'' || text[text.len() - 1] != b'\'' {
                    return Err(err!(FigTypeMismatch));
                }
                let cp = parse_char_literal(text).ok_or(err!(FigTypeMismatch))?;
                Ok(Typed { kind: TypedKind::Char(cp.to_string().into_bytes()), tag: None })
            }
            b"datetime" | b"date" | b"time" => {
                let k = datetime::classify(text, datetime::Options::default()).map_err(|_| err!(FigTypeMismatch))?;
                let e = match k {
                    datetime::Kind::OffsetDatetime => ExtKind::OffsetDatetime,
                    datetime::Kind::LocalDatetime => ExtKind::LocalDatetime,
                    datetime::Kind::LocalDate => ExtKind::LocalDate,
                    datetime::Kind::LocalTime => ExtKind::LocalTime,
                };
                let ok = match type_name {
                    b"date" => e == ExtKind::LocalDate,
                    b"time" => e == ExtKind::LocalTime,
                    _ => true, // "datetime" accepts any of the four shapes
                };
                if !ok {
                    return Err(err!(FigTypeMismatch));
                }
                Ok(ext(e, text))
            }
            _ => Err(err!(FigUnknownType)),
        }
    }

    /// Capture a bare RHS to end of line, honoring the `#`-after-whitespace
    /// rule (a `#` glued to non-whitespace, e.g. a URL fragment, stays
    /// literal). Leaves `self.pos` at the newline/EOF (never past it).
    fn scan_bare_rest_of_line(&mut self) -> BareScan<'s> {
        let source = self.source;
        let start = self.pos;
        let mut i = start;
        let mut prev_space = true; // the separator space before the value was already skipped
        let mut comment_start: Option<usize> = None;
        while i < source.len() && source[i] != b'\n' {
            let ch = source[i];
            if ch == b'#' && prev_space {
                comment_start = Some(i);
                break;
            }
            prev_space = ch == b' ' || ch == b'\t';
            i += 1;
        }
        let value_end = comment_start.unwrap_or(i);
        let text = tok::trim(&source[start..value_end], b" \t\r");
        let mut comment: Option<&'s [u8]> = None;
        let mut line_end = value_end;
        if let Some(cs) = comment_start {
            let mut j = cs + 1;
            while j < source.len() && source[j] != b'\n' {
                j += 1;
            }
            comment = Some(tok::trim(&source[cs + 1..j], b" \t\r"));
            line_end = j;
        }
        self.pos = line_end;
        BareScan { text, comment }
    }

    /// After a committed value (quote/flow/multiline) ends, the rest of the
    /// line may only be whitespace and an optional `# comment` — anything else
    /// is `FigTrailingContent`. Leaves `self.pos` at the newline/EOF.
    fn scan_trailing_comment_only(&mut self) -> Result<Option<&'s [u8]>> {
        let source = self.source;
        self.skip_spaces_tabs();
        if self.pos >= source.len() || source[self.pos] == b'\n' || self.at_crlf() {
            return Ok(None);
        }
        if source[self.pos] == b'#' {
            let cs = self.pos + 1;
            let mut j = cs;
            while j < source.len() && source[j] != b'\n' {
                j += 1;
            }
            let c = tok::trim(&source[cs..j], b" \t\r");
            self.pos = j;
            return Ok(Some(c));
        }
        Err(err!(FigTrailingContent))
    }

    // ── Flow mode (recursive descent; fig-inline ∪ JSON5) ────────────────────

    /// Peek (without consuming) a `# …` comment that is the only thing after
    /// the opening `[`/`{` on its line. Called with `self.pos` on the opening
    /// bracket. Not consumed on purpose: `skip_flow_ws` discards the same span
    /// during the flow parse.
    fn scan_flow_opener_comment(&self) -> Option<&'s [u8]> {
        let source = self.source;
        let mut i = self.pos + 1; // past the opening bracket
        while i < source.len() && (source[i] == b' ' || source[i] == b'\t') {
            i += 1;
        }
        if i >= source.len() || source[i] != b'#' {
            return None;
        }
        let mut j = i + 1;
        while j < source.len() && source[j] != b'\n' {
            j += 1;
        }
        Some(tok::trim(&source[i + 1..j], b" \t\r"))
    }

    fn parse_flow_value(&mut self) -> Result<NodeId> {
        match self.peek().unwrap() {
            b'[' => self.parse_flow_array(),
            b'{' => self.parse_flow_object(),
            _ => unreachable!("a flow value opens with `[` or `{{`"),
        }
    }

    /// Skip whitespace, newlines, and `#` comments between flow tokens.
    /// Comments are discarded (a documented scope cut: flow-interior comments
    /// are not attached to the AST). Every call site is a structural boundary,
    /// so a `#` here is unambiguously a comment.
    fn skip_flow_ws(&mut self) {
        let source = self.source;
        while self.pos < source.len() {
            match source[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                b'#' => {
                    let at = self.pos;
                    while self.pos < source.len() && source[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                    if source[at..self.pos].iter().any(|&c| c == b']' || c == b'}') {
                        self.flow_comment_close = Some(at);
                    }
                }
                _ => return,
            }
        }
    }

    /// Input ran out inside a flow value. When a comment inside it held a
    /// closing bracket, that comment is the likelier culprit than a missing
    /// close, and the diagnostic points at its `#`.
    fn unclosed_flow(&mut self) -> Error {
        if let Some(at) = self.flow_comment_close {
            return self.fail_span(at, at + 1, err!(FigFlowCommentedClose));
        }
        err!(FigUnclosedFlow)
    }

    /// Content where a flow member should have ended: right after a quoted
    /// string it is stray text glued to the quote; a stray close bracket is a
    /// mismatched close; anything else is a second member with no separator
    /// before it.
    fn flow_stray(&mut self, prev: NodeId, comma_mode: bool) -> Error {
        let source = self.source;
        let at = self.pos;
        let span = self.w.span_of(prev);
        let quoted = self.rows()[prev as usize].kind == table::Kind::STRING
            && span.start < source.len()
            && (source[span.start] == b'"' || source[span.start] == b'\'');
        if quoted && at == span.end {
            return self.fail_at(at, err!(FigQuotedTrailingContent));
        }
        if source[at] == b']' || source[at] == b'}' {
            return self.fail_at(at, err!(FigUnclosedFlow));
        }
        self.fail_at(at, if comma_mode { err!(FigFlowMissingComma) } else { err!(FigFlowMissingEquals) })
    }

    fn parse_flow_array(&mut self) -> Result<NodeId> {
        let start = self.pos;
        self.advance(); // '['
        // Its span ends at its close: set when the close is read.
        let seq = self.new_container(Span::new(start, start), ContainerInfo { shape: Shape::Sequence, closed: true, ..ContainerInfo::default() })?;
        self.skip_flow_ws();
        // Each pass starts at a member or the close. A `,` there is a member
        // that is nothing (`[a,,b]`); a trailing `,` before the close is
        // allowed.
        loop {
            let Some(c) = self.peek() else { return Err(self.unclosed_flow()) };
            if c == b']' {
                self.advance();
                break;
            }
            if c == b',' {
                let at = self.pos;
                return Err(self.fail_span(at, at + 1, err!(FigFlowEmptyElement)));
            }
            let v = self.parse_flow_scalar_or_nested(FlowPos::List)?;
            self.append_element(seq, v)?;
            self.skip_flow_ws();
            let Some(p) = self.peek() else { return Err(self.unclosed_flow()) };
            match p {
                b',' => {
                    self.advance();
                    self.skip_flow_ws();
                }
                b']' => {}
                _ => return Err(self.flow_stray(v, true)),
            }
        }
        let end = self.pos;
        self.w.set_span(seq, Span::new(start, end));
        Ok(seq)
    }

    /// A flow object is EITHER fig-spelled (`=` pairs) OR JSON-spelled (`:`
    /// pairs, quoted keys) — the first pair's separator selects the spelling,
    /// and it may not change within one object. `:` with a bare key is the
    /// YAML/JSON5 habit; its own error code so the message can name both
    /// fixes.
    ///
    /// A fig-spelled object is **compact** (docs/proposals/compact-objects.md):
    /// pairs are separated by whitespace (a `,` is accepted too), a value is
    /// one token, and a key is a key path — `{a.b=1 a.c=2}` builds `{a: {b: 1,
    /// c: 2}}` through implicit sub-objects, which stay open to this object's
    /// later pairs and close with it. A JSON-spelled object keeps commas and
    /// values that run to the next `,`/`}`.
    fn parse_flow_object(&mut self) -> Result<NodeId> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Mode {
            Fig,
            Json,
        }
        let start = self.pos;
        self.advance(); // '{'
        // Its span ends at its close: set when the close is read.
        let map = self.new_container(Span::new(start, start), ContainerInfo { shape: Shape::Mapping, closed: true, ..ContainerInfo::default() })?;
        // The implicit sub-objects its dotted keys make, closed with it.
        let mut implicit: Vec<NodeId> = Vec::new();
        let mut mode: Option<Mode> = None;
        self.skip_flow_ws();
        loop {
            let Some(c) = self.peek() else { return Err(self.unclosed_flow()) };
            if c == b'}' {
                self.advance();
                break;
            }
            if c == b',' {
                let at = self.pos;
                return Err(self.fail_span(at, at + 1, err!(FigFlowEmptyElement)));
            }
            let path = self.scan_flow_key_path()?;
            let last = &path[path.len() - 1];
            self.skip_flow_ws();
            let sep_at = self.pos;
            let Some(sep) = self.peek() else { return Err(self.unclosed_flow()) };
            let this_mode = match sep {
                b'=' => Mode::Fig,
                b':' => {
                    if path.len() == 1 && self.is_quoted_seg(last) {
                        Mode::Json
                    } else if path.len() == 1 {
                        return Err(self.fail_at(sep_at, err!(FigFlowBareKeyColon)));
                    } else {
                        return Err(self.fail_span(path[0].span.start, last.span.end, err!(FigBadKey)));
                    }
                }
                _ => return Err(self.fail_span(path[0].span.start, last.span.end, err!(FigFlowMissingEquals))),
            };
            match mode {
                Some(mm) => {
                    if mm != this_mode {
                        return Err(self.fail_at(sep_at, err!(FigMixedFlowSeparators)));
                    }
                }
                None => mode = Some(this_mode),
            }
            self.advance();
            self.skip_flow_ws();
            let v = self.parse_flow_scalar_or_nested(if this_mode == Mode::Fig { FlowPos::Compact } else { FlowPos::List })?;

            let mut target = map;
            for seg in &path[..path.len() - 1] {
                let fresh = self.children.get(&self.w, target, &seg.name).is_none();
                let child = match self.get_or_create_map_container(target, seg, true, None) {
                    Ok(c) => c,
                    Err(e) => return Err(self.fail_span(seg.span.start, seg.span.end, e)),
                };
                if fresh {
                    implicit.push(child);
                }
                if let Err(e) = self.as_mapping(child) {
                    return Err(self.fail_span(seg.span.start, seg.span.end, e));
                }
                target = child;
            }
            if self.children.get(&self.w, target, &last.name).is_some() {
                return Err(self.fail_span(last.span.start, last.span.end, err!(FigDuplicateKey)));
            }
            self.add_entry(target, last, v, None)?;

            if this_mode == Mode::Fig {
                // Compact: the value must be followed by whitespace, `,`, or `}`.
                let Some(nb) = self.peek() else { return Err(self.unclosed_flow()) };
                if !tok::is_compact_stop(nb) || nb == b']' {
                    return Err(self.flow_stray(v, false));
                }
                self.skip_flow_ws();
                if self.peek() == Some(b',') {
                    self.advance();
                    self.skip_flow_ws();
                }
            } else {
                self.skip_flow_ws();
                let Some(p) = self.peek() else { return Err(self.unclosed_flow()) };
                match p {
                    b',' => {
                        self.advance();
                        self.skip_flow_ws();
                    }
                    b'}' => {}
                    _ => return Err(self.flow_stray(v, true)),
                }
            }
        }
        for c in implicit {
            self.info_of(c).unwrap().closed = true;
        }
        let end = self.pos;
        self.w.set_span(map, Span::new(start, end));
        Ok(map)
    }

    fn is_quoted_seg(&self, seg: &KeySeg<'_>) -> bool {
        let c = self.source[seg.span.start];
        c == b'"' || c == b'\''
    }

    /// A flow object's key: a § 5.1 key path — bare segments of key
    /// characters, or quoted segments, joined by `.` — the same grammar as a
    /// block assignment's, less the index steps.
    fn scan_flow_key_path(&mut self) -> Result<Vec<KeySeg<'s>>> {
        let mut segs: Vec<KeySeg<'s>> = Vec::new();
        loop {
            let start = self.pos;
            let seg = match self.scan_key_seg() {
                Ok(s) => s,
                Err(e) if e.is("FigBadKey") => return Err(self.fail_at(start, err!(FigBadKey))),
                Err(e) => return Err(e),
            };
            segs.push(seg);
            if self.peek() != Some(b'.') {
                break;
            }
            self.advance();
        }
        if self.peek() == Some(b'[') {
            let at = self.pos;
            return Err(self.fail_at(at, err!(FigBadKey)));
        }
        Ok(segs)
    }

    fn parse_flow_scalar_or_nested(&mut self, pos: FlowPos) -> Result<NodeId> {
        let source = self.source;
        self.skip_flow_ws();
        let Some(c) = self.peek() else { return Err(self.unclosed_flow()) };
        if c == b'[' || c == b'{' {
            // A balanced bracket group with trailing content — a markdown
            // link, a glob, a regex — is a bare-string element, not a nested
            // collection: the flow twin of the block layer's
            // balanced-then-trailing rule.
            if tok::classify_flow_bracket(source, self.pos, pos == FlowPos::Compact) == BracketCommit::BareTrailing {
                let vstart = self.pos;
                if pos == FlowPos::List
                    && let Some(close) = self.flow_like_prefix(vstart)?
                {
                    self.warn_at(WarningCode::FlowLikeString, vstart, close + 1)?;
                }
                let text = self.scan_flow_bare_bracket(pos);
                if text.is_empty() {
                    return Err(err!(FigInvalidValue));
                }
                // Opened with a delimiter → sniffing is off; it is a string.
                let span = self.span_of(text);
                return self.scalar(NodeKind::String(text), span, None);
            }
            return self.parse_flow_value();
        }
        let rest = &source[self.pos..];
        if rest.starts_with(b"'''") || rest.starts_with(b"\"\"\"") {
            let at = self.pos;
            return Err(self.fail_span(at, at + 3, err!(FigFlowMultilineString)));
        }
        if c == b'"' {
            let start = self.pos;
            let r = tok::scan_double_quoted(source, self.pos)?;
            self.pos = r.end;
            return self.scalar(NodeKind::String(&r.text), Span::new(start, r.end), None);
        }
        if c == b'\'' {
            let start = self.pos;
            let r = tok::scan_single_quoted(source, self.pos)?;
            self.pos = r.end;
            return self.scalar(NodeKind::String(&r.text), Span::new(start, r.end), None);
        }
        let vstart = self.pos;
        let text = match pos {
            FlowPos::List => self.scan_flow_bare_value(),
            FlowPos::Compact => self.scan_compact_bare(),
        };
        if text.is_empty() {
            return Err(err!(FigInvalidValue));
        }
        // The missing-comma smell: a bare list value swallowing a ` = ` almost
        // always means a `key = value` written where an element belongs. A
        // compact value cannot hold one: whitespace ended it.
        if pos == FlowPos::List
            && let Some(i) = find(text, b" = ")
        {
            self.warn_at(WarningCode::FlowMissingComma, vstart + i + 1, vstart + i + 2)?;
        }
        // `Infinity`/`NaN` are NOT recognized here (JSON5 is dropped): they
        // sniff to plain strings, exactly as bare `inf`/`nan` do in the block
        // layer.
        let kind = self.sniff_to_kind_warned(text, vstart)?;
        let span = self.span_of(text);
        self.scalar(kind, span, None)
    }

    /// A bare flow value runs to the next `,`/`]`/`}`/newline — spaces
    /// included, so `[Adam Harris, Makena Harris]` yields two-word strings. A
    /// `#` after whitespace ends the value and opens a comment (skipped by the
    /// following `skip_flow_ws`); a `#` glued to non-whitespace stays literal.
    /// Trailing whitespace is trimmed.
    fn scan_flow_bare_value(&mut self) -> &'s [u8] {
        let source = self.source;
        let start = self.pos;
        let mut prev_space = false;
        while self.pos < source.len() {
            let ch = source[self.pos];
            match ch {
                b',' | b']' | b'}' | b'\n' => break,
                b'#' if prev_space => break,
                _ => {}
            }
            prev_space = ch == b' ' || ch == b'\t';
            self.pos += 1;
        }
        tok::trim(&source[start..self.pos], b" \t\r")
    }

    /// A compact object's bare value is one token: it ends at whitespace or a
    /// `,`/`]`/`}`. A `#` inside it is content — a comment needs whitespace
    /// before it, and whitespace has already ended the token.
    ///
    /// One join: a local date, one space, and a time (`2026-07-01 09:00`) are
    /// one datetime. That is unambiguous, because a time can never be a key
    /// (`:` is not a key character).
    fn scan_compact_bare(&mut self) -> &'s [u8] {
        let source = self.source;
        let start = self.pos;
        self.pos = compact_token_end(source, self.pos);
        if self.pos < source.len() && source[self.pos] == b' ' {
            let first = tok::sniff_bare(&source[start..self.pos]);
            if matches!(first, Sniffed::Datetime { kind: datetime::Kind::LocalDate, .. }) {
                let end = compact_token_end(source, self.pos + 1);
                if end > self.pos + 1 {
                    let joined = tok::sniff_bare(&source[start..end]);
                    if matches!(joined, Sniffed::Datetime { kind: datetime::Kind::LocalDatetime | datetime::Kind::OffsetDatetime, .. }) {
                        self.pos = end;
                    }
                }
            }
        }
        &source[start..self.pos]
    }

    /// Bracket-aware bare flow value for a balanced-then-trailing element (one
    /// whose first char is `[`/`{`, e.g. a markdown link). `[`/`{` raise a
    /// nesting depth and `]`/`}` lower it, so an interior `]` does NOT
    /// terminate the value — only a `,`/`]`/`}` at depth 0, a newline, or a
    /// `#` after whitespace does (and, in a compact object, any whitespace at
    /// depth 0). Trailing whitespace is trimmed.
    fn scan_flow_bare_bracket(&mut self, pos: FlowPos) -> &'s [u8] {
        let source = self.source;
        let start = self.pos;
        let mut depth: usize = 0;
        let mut prev_space = false;
        while self.pos < source.len() {
            let ch = source[self.pos];
            match ch {
                b'[' | b'{' => depth += 1,
                b']' | b'}' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                b',' | b'\n' => {
                    if depth == 0 {
                        break;
                    }
                }
                b' ' | b'\t' | b'\r' => {
                    if depth == 0 && pos == FlowPos::Compact {
                        break;
                    }
                }
                b'#' => {
                    if prev_space && depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            prev_space = ch == b' ' || ch == b'\t';
            self.pos += 1;
        }
        tok::trim(&source[start..self.pos], b" \t\r")
    }

    // ── Spans ───────────────────────────────────────────────────────────────

    /// Once every line is read: widen each container's span to reach its
    /// subtree's end, and end each entry where its value now ends. A
    /// container's row was written over its OPENING position and its children
    /// came later, so only now are the ends known. A post-order walk over the
    /// rows' own links, iterative — documents can nest deeply.
    ///
    /// A nested block container still undecided here never had a child; the
    /// root alone may stay so (an empty document is an empty mapping).
    fn finish_spans(&mut self) -> Result<()> {
        let mut stack: Vec<(NodeId, bool)> = vec![(self.root, false)];
        while let Some((id, done)) = stack.pop() {
            let r = self.rows()[id as usize];
            if !table::Kind::is_container(r.kind) {
                continue;
            }
            if !done {
                stack.push((id, true));
                let mut c = r.first_child;
                while c != NONE {
                    stack.push((c, false));
                    c = self.rows()[c as usize].next_sibling;
                }
                continue;
            }
            if r.kind == table::Kind::KEYVALUE {
                let rows = &mut self.w.t.rows;
                let value = rows[r.first_child as usize].next_sibling;
                rows[id as usize].end = rows[value as usize].end;
                continue;
            }
            if id != self.root && self.info_of(id).unwrap().shape == Shape::Undecided {
                return Err(err!(FigEmptyContainer));
            }
            let rows = &mut self.w.t.rows;
            let mut end = r.end;
            let mut c = r.first_child;
            while c != NONE {
                end = end.max(rows[c as usize].end);
                c = rows[c as usize].next_sibling;
            }
            rows[id as usize].end = end;
        }
        Ok(())
    }

    // ── Char-level helpers ───────────────────────────────────────────────────

    /// Span of `text` within `self.source`, valid only when `text` is
    /// literally a subslice of it — true for every bare/trimmed token but NOT
    /// for a quoted/decoded string (those track their own `(start, r.end)` at
    /// the call site).
    fn span_of(&self, text: &[u8]) -> Span {
        let start = text.as_ptr() as usize - self.source.as_ptr() as usize;
        Span::new(start, start + text.len())
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.pos).copied()
    }

    fn advance(&mut self) {
        if self.pos < self.source.len() {
            self.pos += 1;
        }
    }

    fn skip_spaces_tabs(&mut self) {
        while self.pos < self.source.len() && (self.source[self.pos] == b' ' || self.source[self.pos] == b'\t') {
            self.pos += 1;
        }
    }

    fn at_end_of_content(&self) -> bool {
        match self.peek() {
            None => true,
            Some(p) => p == b'\n' || p == b'#' || self.at_crlf(),
        }
    }

    /// Stricter than `at_end_of_content`: true only for real end-of-line/EOF,
    /// never for a `#` — used where a comment must NOT be swallowed as
    /// "blank".
    fn at_true_line_end(&self) -> bool {
        match self.peek() {
            None => true,
            Some(p) => p == b'\n' || self.at_crlf(),
        }
    }

    /// A `\r` that terminates its line — immediately before `\n` or at EOF —
    /// is trivia, so CRLF documents parse identically to their LF counterparts
    /// (spec § 3.1). A `\r` anywhere else stays ordinary content bytes.
    fn at_crlf(&self) -> bool {
        self.is_crlf_at(self.pos)
    }

    fn is_crlf_at(&self, pos: usize) -> bool {
        pos < self.source.len() && self.source[pos] == b'\r' && (pos + 1 >= self.source.len() || self.source[pos + 1] == b'\n')
    }

    fn skip_to_next_line(&mut self) {
        if self.at_crlf() {
            self.pos += 1; // a line-terminating `\r` is trivia (§ 3.1)
        }
        if self.pos < self.source.len() && self.source[self.pos] == b'\n' {
            self.pos += 1;
        }
    }

    fn consume_line_end(&mut self) {
        while self.pos < self.source.len() && self.source[self.pos] != b'\n' {
            self.pos += 1;
        }
        self.skip_to_next_line();
    }

    fn is_triple_at(&self, pos: usize, q: u8) -> bool {
        pos + 3 <= self.source.len() && self.source[pos] == q && self.source[pos + 1] == q && self.source[pos + 2] == q
    }
}

#[derive(Debug, Clone, Copy)]
struct Markers {
    depth: u32,
    /// A `*` element marker ended the prefix — spaced (`> *`, the normalized
    /// form), glued (`>*`), or, at root, a bare leading `*` (a zero-marker
    /// sequence element). scan_markers consumed it and its separator.
    star: bool,
    /// The `*`'s own span when `star` is set.
    star_span: Option<Span>,
}

/// Where a flow member sits, which decides where an unquoted one ends: a
/// `List` member (an array element, or a JSON-spelled object's value) runs to
/// the next `,`/`]`/`}`, spaces included; a `Compact` one (a fig-spelled
/// object's value) is one token, and whitespace ends it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowPos {
    List,
    Compact,
}

#[derive(Debug, Clone, Copy)]
struct BareScan<'s> {
    text: &'s [u8],
    comment: Option<&'s [u8]>,
}

/// A typed value's kind: a node kind over the source, or a char literal's
/// codepoint, which the annotation computes (Zig allocates it in the line's
/// scratch).
enum TypedKind<'s> {
    Kind(NodeKind<'s>),
    Char(Vec<u8>),
}

/// What `apply_known_type` makes of a value: its row's kind, and its type tag.
struct Typed<'s> {
    kind: TypedKind<'s>,
    tag: Option<KindTag>,
}

fn sniff_to_kind(text: &[u8]) -> NodeKind<'_> {
    match tok::sniff_bare(text) {
        Sniffed::Null => NodeKind::Null,
        Sniffed::Boolean(b) => NodeKind::Boolean(b),
        Sniffed::Number(n) => NodeKind::Number(Number {
            raw: n.raw,
            kind: if n.kind == literal::NumberKind::Float { NumberKind::Float } else { NumberKind::Integer },
        }),
        Sniffed::Datetime { kind, raw } => NodeKind::Extended(Extended {
            kind: match kind {
                datetime::Kind::OffsetDatetime => ExtKind::OffsetDatetime,
                datetime::Kind::LocalDatetime => ExtKind::LocalDatetime,
                datetime::Kind::LocalDate => ExtKind::LocalDate,
                datetime::Kind::LocalTime => ExtKind::LocalTime,
            },
            text: raw,
        }),
        Sniffed::String => NodeKind::String(text),
    }
}

/// Case-variant bool/null spellings from other config languages (`Yes`, `ON`,
/// `TRUE`, `Null`) — never literals in fig (the Norway fix), but surprising
/// enough as silent strings to earn a warn. Exact lowercase `true`/`false`/
/// `null` never reach here (they sniff to literals first).
fn looks_like_literal(text: &[u8]) -> bool {
    const WORDS: [&[u8]; 7] = [b"yes", b"no", b"on", b"off", b"true", b"false", b"null"];
    WORDS.iter().any(|w| text.eq_ignore_ascii_case(w))
}

/// `007`-style: a token (after an optional sign) that would be a valid number
/// but for a leading zero on its integer part — digit-only (`007`, `-07`) or a
/// float shape (`01.5`, `00.5`, `-01e2`) — kept as a string by the TOML
/// leading-zero rule. Decided by re-sniffing with the extra zeros stripped, so
/// anything that still isn't a number (`0x`, `01.5.6`, `007a`) never warns.
fn looks_like_leading_zero(text: &[u8]) -> bool {
    let mut s = text;
    if !s.is_empty() && (s[0] == b'+' || s[0] == b'-') {
        s = &s[1..];
    }
    if s.len() < 2 || s[0] != b'0' || !s[1].is_ascii_digit() {
        return false;
    }
    let mut k = 0usize;
    while k + 1 < s.len() && s[k] == b'0' && s[k + 1].is_ascii_digit() {
        k += 1;
    }
    tok::sniff_number(&s[k..]).is_some()
}

/// `1.`-style: a clean integer followed by a single trailing dot (`1.`, `42.`,
/// `-3.`) — a number that fell to a string because no fig float spelling has a
/// bare trailing dot. Deliberately NARROW: the part before the dot must sniff
/// as a *decimal integer*.
fn looks_like_trailing_dot_number(text: &[u8]) -> bool {
    if text.len() < 2 || text[text.len() - 1] != b'.' {
        return false;
    }
    let head = &text[..text.len() - 1];
    match tok::sniff_number(head) {
        Some(n) => n.kind == literal::NumberKind::Integer,
        None => false,
    }
}

/// `.5`-style: a single leading dot followed by clean digits only (`.5`,
/// `.25`) — the shorthand-float habit no fig number spelling accepts. As
/// deliberately NARROW as the trailing-dot twin above.
fn looks_like_leading_dot_number(text: &[u8]) -> bool {
    if text.len() < 2 || text[0] != b'.' {
        return false;
    }
    text[1..].iter().all(|c| c.is_ascii_digit())
}

/// Normalize a decimal-integer token that `sniff_number` rejected only for its
/// leading zero (`09`, `007`, `-00`) into a valid integer raw: sign preserved
/// (`+` dropped), leading zeros stripped to at least one digit. Returns `None`
/// if the post-sign body isn't purely decimal digits (so a genuine mismatch
/// still errors).
fn normalize_decimal(text: &[u8]) -> Option<Vec<u8>> {
    let mut i = 0usize;
    let negative = !text.is_empty() && text[0] == b'-';
    if !text.is_empty() && (text[0] == b'+' || text[0] == b'-') {
        i = 1;
    }
    let body = &text[i..];
    if body.is_empty() || !body.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut start = 0usize;
    while start + 1 < body.len() && body[start] == b'0' {
        start += 1; // keep ≥1 digit
    }
    let mut out = Vec::new();
    if negative {
        out.push(b'-');
    }
    out.extend_from_slice(&body[start..]);
    Some(out)
}

/// Normalize a token `: float` accepts by coercion but `sniff_number` rejects:
/// a trailing-dot integer (`1.` → `1.0`) or a leading-zero decimal (`09` →
/// `9.0`). Returns `None` when neither shape applies (a real mismatch → error).
fn normalize_coerced_float(text: &[u8]) -> Option<Vec<u8>> {
    // Trailing dot: `1.` — the head must be a clean decimal integer body.
    if text.len() >= 2 && text[text.len() - 1] == b'.' {
        let mut digits = normalize_decimal(&text[..text.len() - 1])?;
        digits.extend_from_slice(b".0");
        return Some(digits);
    }
    // Leading-zero decimal in float position: `09` → `9.0`.
    let mut digits = normalize_decimal(text)?;
    digits.extend_from_slice(b".0");
    Some(digits)
}

fn compact_token_end(source: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < source.len() && !tok::is_compact_stop(source[i]) {
        i += 1;
    }
    i
}

/// The index of `needle` in `hay` (`std.mem.indexOf`).
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    find(hay, needle).is_some()
}

/// Zig's `std.zig.string_literal.parseCharLiteral`, which the `: char`
/// annotation reuses: the codepoint of a `'…'` literal holding exactly one
/// UTF-8 codepoint or one escape (`\n \r \t \\ \' \"`, `\xNN`, `\u{N…}`), or
/// `None` for anything else. The caller has checked the single quotes.
fn parse_char_literal(slice: &[u8]) -> Option<u32> {
    if slice.len() < 3 {
        return None; // empty_char_literal
    }
    debug_assert!(slice[0] == b'\'' && slice[slice.len() - 1] == b'\'');
    match slice[1] {
        b'\\' => {
            let mut offset = 1usize;
            let result = parse_escape_sequence(slice, &mut offset);
            if result.is_some() && (offset + 1 != slice.len() || slice[offset] != b'\'') {
                return None; // expected_single_quote
            }
            result
        }
        0 => None, // invalid_character
        first => {
            let inner = &slice[1..slice.len() - 1];
            let n = match first {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => return None, // invalid_unicode_codepoint
            };
            if inner.len() > n {
                return None; // expected_single_quote
            }
            let s = std::str::from_utf8(inner.get(..n)?).ok()?;
            s.chars().next().map(|c| c as u32)
        }
    }
}

/// Zig's `std.zig.string_literal.parseEscapeSequence`: the escape at
/// `slice[*offset] == '\\'`, advancing `offset` past it on success.
fn parse_escape_sequence(slice: &[u8], offset: &mut usize) -> Option<u32> {
    debug_assert!(slice.len() > *offset && slice[*offset] == b'\\');
    if slice.len() == *offset + 1 {
        return None;
    }
    *offset += 2;
    match slice[*offset - 1] {
        b'n' => Some(b'\n' as u32),
        b'r' => Some(b'\r' as u32),
        b'\\' => Some(b'\\' as u32),
        b't' => Some(b'\t' as u32),
        b'\'' => Some(b'\'' as u32),
        b'"' => Some(b'"' as u32),
        b'x' => {
            let mut value: u32 = 0;
            let mut i = *offset;
            while i < *offset + 2 {
                if i == slice.len() {
                    return None;
                }
                let d = (slice[i] as char).to_digit(16)?;
                value = value * 16 + d;
                i += 1;
            }
            *offset = i;
            Some(value)
        }
        b'u' => {
            let mut i = *offset;
            if i >= slice.len() || slice[i] != b'{' {
                return None;
            }
            i += 1;
            if i >= slice.len() || slice[i] == b'}' {
                return None;
            }
            let mut value: u32 = 0;
            loop {
                if i >= slice.len() {
                    return None; // expected_rbrace
                }
                let c = slice[i];
                if c == b'}' {
                    i += 1;
                    break;
                }
                let d = (c as char).to_digit(16)?;
                value = value * 16 + d;
                if value > 0x10ffff {
                    return None;
                }
                i += 1;
            }
            *offset = i;
            Some(value)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::{Literal, Node, PathSegment};

    fn k(s: &str) -> PathSegment<'_> {
        PathSegment::Key(s.as_bytes())
    }

    fn ix(i: usize) -> PathSegment<'static> {
        PathSegment::Index(i)
    }

    fn ast(src: &str) -> AST {
        parse_abstract(src.as_bytes(), Type::Fig).unwrap()
    }

    fn fails(src: &str) -> Error {
        parse_abstract(src.as_bytes(), Type::Fig).unwrap_err()
    }

    fn string<'a>(a: &'a AST, path: &[PathSegment<'_>]) -> &'a [u8] {
        a.get_val_by_path(path).unwrap().kind.as_string().expect("a string")
    }

    fn raw<'a>(a: &'a AST, path: &[PathSegment<'_>]) -> &'a [u8] {
        a.get_val_by_path(path).unwrap().kind.as_number().expect("a number").raw
    }

    fn ext<'a>(a: &'a AST, path: &[PathSegment<'_>]) -> Extended<'a> {
        match a.get_val_by_path(path).unwrap().kind {
            NodeKind::Extended(e) => e,
            other => panic!("not extended: {other:?}"),
        }
    }

    fn expect_parse(input: &[u8], root: NodeId, nodes: &[Node<'_>]) {
        let a = parse_abstract(input, Type::Fig).unwrap();
        assert!(Literal { root, nodes }.matches(&a));
    }

    fn n(id: NodeId, kind: NodeKind<'_>) -> Node<'_> {
        Node { id, kind, next_sibling: None }
    }

    #[test]
    fn empty_document_is_an_empty_root_map() {
        // No bare-scalar root and no map/sequence ambiguity to resolve, so an
        // empty file is the empty map `set new.figl k v` seeds into.
        expect_parse(b"", 0, &[n(0, NodeKind::Mapping(None))]);
        // A comments-only / whitespace-only file is likewise an empty root
        // map, not the `FigEmptyContainer` a childless *nested* container
        // still raises.
        expect_parse(b"# just a note\n", 0, &[n(0, NodeKind::Mapping(None))]);
        expect_parse(b"\n\n  \n", 0, &[n(0, NodeKind::Mapping(None))]);
    }

    fn x_equals_one() -> [Node<'static>; 4] {
        [
            n(0, NodeKind::String(b"x")),
            n(1, NodeKind::Number(Number { raw: b"1", kind: NumberKind::Integer })),
            n(2, NodeKind::KeyValue(KeyValue { key: 0, value: 1 })),
            n(3, NodeKind::Mapping(Some(2))),
        ]
    }

    #[test]
    fn root_map_with_a_scalar_assignment() {
        expect_parse(b"x = 1\n", 3, &x_equals_one());
    }

    #[test]
    fn literal_else_string_sniffing() {
        let a = ast("a = true\nb = Yes\nc = 007\nd = 12 monkeys\ne = null");
        assert_eq!(NodeKind::Boolean(true), a.get_val_by_path(&[k("a")]).unwrap().kind);
        assert_eq!(b"Yes", string(&a, &[k("b")]));
        assert_eq!(b"007", string(&a, &[k("c")]));
        assert_eq!(b"12 monkeys", string(&a, &[k("d")]));
        assert_eq!(NodeKind::Null, a.get_val_by_path(&[k("e")]).unwrap().kind);
    }

    #[test]
    fn nested_containers_via_markers() {
        let a = ast("database\n> host = localhost\n> pool\n>> size = 10");
        assert_eq!(b"localhost", string(&a, &[k("database"), k("host")]));
        assert_eq!(b"10", raw(&a, &[k("database"), k("pool"), k("size")]));
    }

    #[test]
    fn node_regions_records_every_header_line_of_a_re_entered_container() {
        let src = "database\n> x = 1\nother = 1\ndatabase\n> y = 2\n";
        let parsed = parse(src.as_bytes(), Type::Fig).unwrap();
        let db = parsed.ast.get_val_by_path(&[k("database")]).unwrap();
        let regs = parsed.regions_of(db.id);
        assert_eq!(2, regs.len());
        // The creating header line, then the SECOND "database" line (the
        // re-open), which the creating line's span cannot carry.
        assert_eq!(0, regs[0].start);
        assert_eq!("database\n".len() as u32, regs[0].end);
        let last = src.rfind("database").unwrap();
        assert_eq!(last as u32, regs[1].start);
        assert_eq!((last + "database\n".len()) as u32, regs[1].end);
        // A scalar is never a section; the root never is either.
        assert!(!parsed.is_section(&parsed.ast.node(parsed.ast.root)));
        assert!(!parsed.is_section(&parsed.ast.get_val_by_path(&[k("other")]).unwrap()));
    }

    #[test]
    fn node_regions_holds_one_line_per_block_container_without_re_entry() {
        let parsed = parse(b"database\n> x = 1\ndatabase.pool\n> y = 2\np = { a = 1 }\n", Type::Fig).unwrap();
        // `database.pool` CREATES pool (anchored by pool's own line) — a deeper
        // dotted path is not a re-open of `database` itself. A flow value is
        // not a section at all.
        let db = parsed.ast.get_val_by_path(&[k("database")]).unwrap();
        assert_eq!(1, parsed.regions_of(db.id).len());
        let pool = parsed.ast.get_val_by_path(&[k("database"), k("pool")]).unwrap();
        assert_eq!(1, parsed.regions_of(pool.id).len());
        assert!(!parsed.is_section(&parsed.ast.get_val_by_path(&[k("p")]).unwrap()));
    }

    #[test]
    fn dotted_key_flattener() {
        let a = ast("cache\n> redis.host = 127.0.0.1");
        assert_eq!(b"127.0.0.1", string(&a, &[k("cache"), k("redis"), k("host")]));
    }

    #[test]
    fn sequence_via_star_elements() {
        let a = ast("servers\n> *\n>> host = a.com\n> *\n>> host = b.com");
        assert_eq!(b"a.com", string(&a, &[k("servers"), ix(0), k("host")]));
        assert_eq!(b"b.com", string(&a, &[k("servers"), ix(1), k("host")]));
    }

    #[test]
    fn scalar_sequence() {
        let a = ast("ports\n> * 1\n> * 2");
        assert_eq!(b"2", raw(&a, &[k("ports"), ix(1)]));
    }

    #[test]
    fn dotted_section_header_re_anchors_baseline() {
        let a = ast("services.web.frontend\n> replicas = 3");
        assert_eq!(b"3", raw(&a, &[k("services"), k("web"), k("frontend"), k("replicas")]));
    }

    #[test]
    fn append_header_appends_and_re_anchors() {
        let a = ast("jobs.test.steps[]\n> uses = a\njobs.test.steps[]\n> run = b");
        assert_eq!(b"a", string(&a, &[k("jobs"), k("test"), k("steps"), ix(0), k("uses")]));
        assert_eq!(b"b", string(&a, &[k("jobs"), k("test"), k("steps"), ix(1), k("run")]));
    }

    #[test]
    fn index_addressing_edits_existing_elements() {
        let a = ast("clusters[0].name = alpha\nclusters[0].size = 3\nclusters[1].name = beta");
        assert_eq!(b"alpha", string(&a, &[k("clusters"), ix(0), k("name")]));
        assert_eq!(b"3", raw(&a, &[k("clusters"), ix(0), k("size")]));
        assert_eq!(b"beta", string(&a, &[k("clusters"), ix(1), k("name")]));
    }

    #[test]
    fn skipping_a_sequence_index_errors() {
        assert_eq!(err!(FigIndexSkipped), fails("xs[1].a = 1\n"));
    }

    #[test]
    fn explicit_typing() {
        let a = ast("port: int = 8080\nmode: enum = creative\nhuge: float = inf");
        assert_eq!(b"8080", raw(&a, &[k("port")]));
        let mode = ext(&a, &[k("mode")]);
        assert_eq!(ExtKind::EnumLiteral, mode.kind);
        assert_eq!(b"creative", mode.text);
        assert_eq!(ExtKind::NumberSpecial, ext(&a, &[k("huge")]).kind);
    }

    #[test]
    fn explicit_typing_rejects_a_mismatch() {
        assert_eq!(err!(FigTypeMismatch), fails("port: int = hello\n"));
    }

    #[test]
    fn char_literal_via_explicit_typing_is_a_char_literal_codepoint() {
        let a = ast("letter: char = 'A'\ntab: char = '\\t'\nquote: char = '\\''\nemoji: char = '\\u{1F600}'\nhash: char = '#'");
        // `: char =` reads a ZON-style char literal (escapes included) and
        // stores the decimal Unicode codepoint.
        for (key, cp) in [("letter", "65"), ("tab", "9"), ("quote", "39"), ("emoji", "128512"), ("hash", "35")] {
            let e = ext(&a, &[k(key)]);
            assert_eq!(ExtKind::CharLiteral, e.kind);
            assert_eq!(cp.as_bytes(), e.text);
        }
    }

    #[test]
    fn char_literal_rejects_a_non_char_lexeme() {
        // A bare (unquoted) atom is not a char literal — quotes are the
        // disambiguator.
        assert_eq!(err!(FigTypeMismatch), fails("x: char = A\n"));
        // Two codepoints in one literal is not a char.
        assert_eq!(err!(FigTypeMismatch), fails("x: char = 'ab'\n"));
        // Only single quotes make a char literal (§ 5.3).
        assert_eq!(err!(FigTypeMismatch), fails("x: char = \"A\"\n"));
        assert_eq!(err!(FigTypeMismatch), fails("x: char = 'A\n"));
        assert_eq!(err!(FigTypeMismatch), fails("x: char = A'\n"));
    }

    #[test]
    fn explicit_typing_coerces_number_lookalikes_but_keeps_the_verbatim_lexeme_and_tag() {
        let a = ast("a: float = 1.\nb: int = 09\nc: float = 09\nd: int = -007\ne: string = [ 1 + 2 ]");
        // `: float`/`: int` accept a trailing-dot / leading-zero lexeme, but
        // the VERBATIM bytes are kept as the number raw, and the annotation is
        // stored as a type tag.
        let av = a.get_val_by_path(&[k("a")]).unwrap();
        assert_eq!(NumberKind::Float, av.kind.as_number().unwrap().kind);
        assert_eq!(b"1.", av.kind.as_number().unwrap().raw);
        assert_eq!(Some(Tag::Kind(KindTag::Float)), a.tag_of(av.id));
        let bv = a.get_val_by_path(&[k("b")]).unwrap();
        assert_eq!(NumberKind::Integer, bv.kind.as_number().unwrap().kind);
        assert_eq!(b"09", bv.kind.as_number().unwrap().raw);
        assert_eq!(Some(Tag::Kind(KindTag::Integer)), a.tag_of(bv.id));
        assert_eq!(b"09", raw(&a, &[k("c")]));
        assert_eq!(b"-007", raw(&a, &[k("d")]));
        // `: string` is a total sink: the bracketed RHS is verbatim text, not
        // flow.
        let ev = a.get_val_by_path(&[k("e")]).unwrap();
        assert_eq!(b"[ 1 + 2 ]", ev.kind.as_string().unwrap());
        assert_eq!(Some(Tag::Kind(KindTag::String)), a.tag_of(ev.id));
    }

    #[test]
    fn string_annotation_takes_the_entire_rhs_as_raw_text() {
        let a = ast("a: string = \"hello\"\nb: string = 42\nc: string = a\\nb\nd: string = '''\ne: string = 'single'\nf: string = \"\"\"");
        // Quote characters are literal content.
        assert_eq!(b"\"hello\"", string(&a, &[k("a")]));
        // Sniffing is off: a number lexeme is the string of its digits.
        assert_eq!(b"42", string(&a, &[k("b")]));
        // Backslashes are literal — no escape processing.
        assert_eq!(b"a\\nb", string(&a, &[k("c")]));
        // A triple-quote opener is content too.
        assert_eq!(b"'''", string(&a, &[k("d")]));
        assert_eq!(b"'single'", string(&a, &[k("e")]));
        assert_eq!(b"\"\"\"", string(&a, &[k("f")]));
        // Every one still records the `: string` tag.
        for key in ["a", "b", "c", "d", "e", "f"] {
            let v = a.get_val_by_path(&[k(key)]).unwrap();
            assert_eq!(Some(Tag::Kind(KindTag::String)), a.tag_of(v.id));
        }
    }

    #[test]
    fn string_annotation_quote_led_rhs_follows_the_bare_trailing_comment_rule() {
        let a = ast("a: string = \"hi\" # note\nb: string = \"x\"#y");
        let av = a.get_val_by_path(&[k("a")]).unwrap();
        assert_eq!(b"\"hi\"", av.kind.as_string().unwrap());
        assert_eq!(b"note", a.comments(av.id).trailing.unwrap().text);
        assert_eq!(b"\"x\"#y", string(&a, &[k("b")]));
    }

    #[test]
    fn string_annotation_is_equivalent_to_the_escaped_quote_untagged_spelling() {
        // The spec's §5.3 equivalence: `str: string = "hello"` ≡
        // `str = "\"hello\""` (same VALUE; the first additionally records the
        // type tag).
        let tagged = ast("str: string = \"hello\"\n");
        let quoted = ast("str = \"\\\"hello\\\"\"\n");
        assert_eq!(b"\"hello\"", string(&tagged, &[k("str")]));
        assert_eq!(string(&tagged, &[k("str")]), string(&quoted, &[k("str")]));
        assert!(tagged.eql(&quoted));
    }

    #[test]
    fn type_mismatch_diagnostic_anchors_on_the_value_span_not_the_trailing_comment() {
        for (src, value) in [("x: int = hello # note\n", "hello"), ("y: bool = maybe # note\n", "maybe")] {
            let mut report = Report::default();
            assert_eq!(err!(FigTypeMismatch), parse_with_report(src.as_bytes(), Type::Fig, &mut report).unwrap_err());
            let d = report.diag.unwrap();
            // Caret sits at the value start, and the end stops at the value
            // end — never inside the ` # note` comment.
            let vstart = src.find(value).unwrap();
            assert_eq!(vstart, d.offset);
            assert_eq!(Some(vstart + value.len()), d.end);
            assert!(d.end.unwrap() < src.find('#').unwrap());
        }
    }

    #[test]
    fn parse_collecting_recovers_past_each_error_and_reports_them_all() {
        let src = "good = 1\nbad1: int = hello\nkey: value\nfine = ok\ndup = 1\ndup = 2\n> orphan = 3";
        let mut report = Report::default();
        assert_eq!(err!(FigTypeMismatch), parse_collecting(src.as_bytes(), Type::Fig, &mut report).unwrap_err());
        // FigTypeMismatch (l2), FigForeignSyntaxColon (l3), FigDuplicateKey
        // (l6), FigRootMarker (l7) — the clean lines do not add entries.
        assert_eq!(4, report.errors.len());
        assert_eq!(err!(FigTypeMismatch), report.errors[0].code);
        assert_eq!(err!(FigForeignSyntaxColon), report.errors[1].code);
        assert_eq!(err!(FigDuplicateKey), report.errors[2].code);
        assert_eq!(err!(FigRootMarker), report.errors[3].code);
        // `diag` mirrors the first error for single-error consumers.
        assert_eq!(err!(FigTypeMismatch), report.diag.unwrap().code);
        // Source order: each diagnostic sits on a later line than the last.
        let mut prev = 0usize;
        for d in &report.errors {
            let loc = d.locate(src.as_bytes());
            assert!(loc.line > prev);
            prev = loc.line;
        }
    }

    #[test]
    fn parse_collecting_on_a_clean_file_returns_the_tree_with_no_errors() {
        let mut report = Report::default();
        let doc = parse_collecting(b"a = 1\nb = two\n", Type::Fig, &mut report).unwrap();
        assert!(report.errors.is_empty());
        assert!(report.diag.is_none());
        assert_eq!(b"1", raw(&doc.ast, &[k("a")]));
    }

    #[test]
    fn committed_values_error_rather_than_falling_back_to_string() {
        assert_eq!(err!(FigUnclosedFlow), fails("ports = [80, 443\n"));
    }

    #[test]
    fn a_single_line_quote_that_closes_early_with_trailing_content_is_the_quote_specific_error() {
        assert_eq!(err!(FigQuotedTrailingContent), fails("she = \"She said, \"Hey there!\"\"\n"));
        assert_eq!(err!(FigQuotedTrailingContent), fails("x = 'a' b\n"));
        // A clean quoted string with a genuine trailing comment is still fine.
        let a = ast("x = \"hi\"  # note\n");
        assert_eq!(b"hi", string(&a, &[k("x")]));
    }

    #[test]
    fn balanced_then_trailing_bracket_rhs_is_a_bare_string_not_committed_flow() {
        let a = ast(
            "link = [Blog](/Blog/Blog.md)\nangle = [Archived Documents](</Archive/Archived documents.md>)\nglob = [a-z]*.md\nregex = [0-9]+ years\nbbcode = [b]bold[/b]\nfrag = [x]#anchor",
        );
        assert_eq!(b"[Blog](/Blog/Blog.md)", string(&a, &[k("link")]));
        assert_eq!(b"[Archived Documents](</Archive/Archived documents.md>)", string(&a, &[k("angle")]));
        assert_eq!(b"[a-z]*.md", string(&a, &[k("glob")]));
        assert_eq!(b"[0-9]+ years", string(&a, &[k("regex")]));
        assert_eq!(b"[b]bold[/b]", string(&a, &[k("bbcode")]));
        // `#` glued to the close is literal (not a comment).
        assert_eq!(b"[x]#anchor", string(&a, &[k("frag")]));
    }

    #[test]
    fn terminal_bracket_still_commits_to_flow() {
        let a = ast("ports = [80, 443]  # real comment\nnested = [[a], [b]]");
        assert_eq!(b"443", raw(&a, &[k("ports"), ix(1)]));
        assert_eq!(b"b", string(&a, &[k("nested"), ix(1), ix(0)]));
    }

    #[test]
    fn markdown_links_are_bare_strings_as_sequence_elements_too() {
        let a = ast("links\n> * [Blog](/Blog/Blog.md)\n> * [Resume](/Resume.md)");
        assert_eq!(b"[Blog](/Blog/Blog.md)", string(&a, &[k("links"), ix(0)]));
        assert_eq!(b"[Resume](/Resume.md)", string(&a, &[k("links"), ix(1)]));
    }

    #[test]
    fn a_bracket_that_never_closes_on_its_line_still_errors() {
        assert_eq!(err!(FigUnclosedFlow), fails("x = [oops\n"));
    }

    #[test]
    fn a_mid_word_quote_inside_a_bracket_led_value_is_content() {
        let a = ast(
            "apos = [its parent's comment](x.md)\ndq = [its parent\"s comment](x.md)\nurl = [text](it's.md)\nseq\n> * [its parent's comment](x.md)\nflow = [[Blog](it's/x.md), b]",
        );
        assert_eq!(b"[its parent's comment](x.md)", string(&a, &[k("apos")]));
        assert_eq!(b"[its parent\"s comment](x.md)", string(&a, &[k("dq")]));
        assert_eq!(b"[text](it's.md)", string(&a, &[k("url")]));
        assert_eq!(b"[its parent's comment](x.md)", string(&a, &[k("seq"), ix(0)]));
        assert_eq!(b"[Blog](it's/x.md)", string(&a, &[k("flow"), ix(0)]));
        // The value is a plain bare string, so it earns no flow-like warn.
        expect_warn_codes("apos = [its parent's comment](x.md)\n", &[]);
    }

    #[test]
    fn a_quote_at_an_element_start_still_opens_a_span_the_balancer_skips() {
        let a = ast("inner = ['[Blog](x)']\nbracket = [\"a]b\", c]\npair = {k = 'a]b'}\njson = {\"a]b\": 1}\nbare = [a, it's fine, b]");
        assert_eq!(b"[Blog](x)", string(&a, &[k("inner"), ix(0)]));
        assert_eq!(b"a]b", string(&a, &[k("bracket"), ix(0)]));
        assert_eq!(b"c", string(&a, &[k("bracket"), ix(1)]));
        assert_eq!(b"a]b", string(&a, &[k("pair"), k("k")]));
        assert_eq!(b"1", raw(&a, &[k("json"), k("a]b")]));
        assert_eq!(b"it's fine", string(&a, &[k("bare"), ix(1)]));
        // An unclosed quote at an element start is still `Unclosed` →
        // committed to the flow parser, which raises.
        assert_eq!(err!(FigUnclosedString), fails("x = ['unterminated]\n"));
    }

    // ── Span tests (the editor's foundation) ──

    fn expect_span(source: &str, path: &[PathSegment<'_>], expected: &str) {
        let doc = parse(source.as_bytes(), Type::Fig).unwrap();
        let node = doc.ast.get_val_by_path(path).unwrap();
        let span = doc.span(&node);
        assert_eq!(expected, &source[span.start..span.end]);
    }

    /// Same as `expect_span`, but reads the *keyvalue* wrapper's span — what
    /// the editor's `deleteKey`/`moveKey` splice.
    fn expect_entry_span(source: &str, path: &[PathSegment<'_>], expected: &str) {
        let doc = parse(source.as_bytes(), Type::Fig).unwrap();
        let node = doc.ast.get_node_by_path(path).unwrap();
        let span = doc.span(&node);
        assert_eq!(expected, &source[span.start..span.end]);
    }

    #[test]
    fn span_scalar_assignment_values_are_tight() {
        expect_span("x = 1\n", &[k("x")], "1");
        expect_span("title = My server\n", &[k("title")], "My server");
        expect_span("s = \"hi\\n\"\n", &[k("s")], "\"hi\\n\"");
        expect_span("s = 'raw'\n", &[k("s")], "'raw'");
    }

    #[test]
    fn span_keys_are_tight_quotes_included() {
        expect_span("x = 1\n", &[k("x")], "1");
        let src = "\"my key\" = 1\n";
        let doc = parse(src.as_bytes(), Type::Fig).unwrap();
        let key = doc.ast.get_key_by_path(&[k("my key")]).unwrap();
        let span = doc.span(&key);
        assert_eq!("\"my key\"", &src[span.start..span.end]);
    }

    #[test]
    fn span_nested_marker_block_value() {
        let src = "database\n> host = localhost\n> pool\n> > size = 10\n";
        expect_span(src, &[k("database"), k("host")], "localhost");
        expect_span(src, &[k("database"), k("pool"), k("size")], "10");
    }

    #[test]
    fn span_dotted_flattener_key_segment() {
        expect_span("cache\n> redis.host = 127.0.0.1\n", &[k("cache"), k("redis"), k("host")], "127.0.0.1");
    }

    #[test]
    fn span_flow_container_covers_exactly_its_brackets() {
        expect_span("ports = [80, 443]\n", &[k("ports")], "[80, 443]");
        expect_span("p = { x = 1, y = 2 }\n", &[k("p")], "{ x = 1, y = 2 }");
        expect_span("ports = [80, 443]\n", &[k("ports"), ix(1)], "443");
    }

    #[test]
    fn span_a_nested_block_containers_keyvalue_covers_header_through_last_body_line() {
        let src = "database\n> host = localhost\n> pool\n> > size = 10\nother = 1\n";
        expect_entry_span(src, &[k("database"), k("pool")], "pool\n> > size = 10");
        expect_entry_span(src, &[k("database")], "database\n> host = localhost\n> pool\n> > size = 10");
    }

    #[test]
    fn span_sequence_element_covers_its_own_body_only() {
        let src = "servers\n> *\n>> host = a.com\n> *\n>> host = b.com\n";
        expect_span(src, &[k("servers"), ix(0), k("host")], "a.com");
        expect_span(src, &[k("servers"), ix(1), k("host")], "b.com");
    }

    #[test]
    fn span_append_header_re_anchored_elements() {
        let src = "jobs.test.steps[]\n> uses = a\njobs.test.steps[]\n> run = b\n";
        expect_span(src, &[k("jobs"), k("test"), k("steps"), ix(0), k("uses")], "a");
        expect_span(src, &[k("jobs"), k("test"), k("steps"), ix(1), k("run")], "b");
    }

    #[test]
    fn quotes_raw_vs_escaped() {
        let a = ast("raw = 'C:\\Users\\me'\nesc = \"line1\\nline2\"");
        assert_eq!(b"C:\\Users\\me", string(&a, &[k("raw")]));
        assert_eq!(b"line1\nline2", string(&a, &[k("esc")]));
    }

    #[test]
    fn comment_marker_only_after_whitespace() {
        let a = ast("home = https://example.com\napi = https://example.com/v1#stable");
        assert_eq!(b"https://example.com", string(&a, &[k("home")]));
        assert_eq!(b"https://example.com/v1#stable", string(&a, &[k("api")]));
    }

    #[test]
    fn flow_mode_fig_inline_and_pasted_json() {
        let a = ast("tags = [a, b, c]\npoint = { x = 1, y = 2 }\npasted = { \"x\": 1, \"y\": [2, 3] }\nempty_list = []\nempty_map = {}");
        assert_eq!(b"b", string(&a, &[k("tags"), ix(1)]));
        assert_eq!(b"1", raw(&a, &[k("point"), k("x")]));
        assert_eq!(b"3", raw(&a, &[k("pasted"), k("y"), ix(1)]));
    }

    #[test]
    fn flow_list_values_may_contain_spaces() {
        let a = ast("audience = [friends, Adam Harris, Makena Harris, public]\nwho = { \"name\": Adam Harris }");
        assert_eq!(b"Adam Harris", string(&a, &[k("audience"), ix(1)]));
        assert_eq!(b"Makena Harris", string(&a, &[k("audience"), ix(2)]));
        // A JSON-spelled object keeps multi-word values.
        assert_eq!(b"Adam Harris", string(&a, &[k("who"), k("name")]));
    }

    #[test]
    fn compact_objects_whitespace_separates_pairs_a_value_is_one_token() {
        let a = ast(
            "a = {x=1 y=two z=\"Hello world\"}\nb = { x = 1, y = 2, }\nc = {lang = en  data.count = 3}\nd = {xs=[1, 2] href=[Blog](/x.md) tag=a#b}\ne = {\n  p=1  # a comment\n  q=2\n}",
        );
        assert_eq!(b"two", string(&a, &[k("a"), k("y")]));
        assert_eq!(b"Hello world", string(&a, &[k("a"), k("z")]));
        assert_eq!(b"2", raw(&a, &[k("b"), k("y")]));
        assert_eq!(b"en", string(&a, &[k("c"), k("lang")]));
        assert_eq!(b"3", raw(&a, &[k("c"), k("data"), k("count")]));
        assert_eq!(b"2", raw(&a, &[k("d"), k("xs"), ix(1)]));
        assert_eq!(b"[Blog](/x.md)", string(&a, &[k("d"), k("href")]));
        assert_eq!(b"a#b", string(&a, &[k("d"), k("tag")]));
        assert_eq!(b"2", raw(&a, &[k("e"), k("q")]));
    }

    #[test]
    fn compact_objects_a_date_a_space_and_a_time_are_one_datetime() {
        let a = ast("t = {at=2026-07-01 09:00 on=2026-07-02 n=1}\n");
        assert_eq!(b"2026-07-01 09:00", ext(&a, &[k("t"), k("at")]).text);
        assert_eq!(b"2026-07-02", ext(&a, &[k("t"), k("on")]).text);
    }

    #[test]
    fn compact_objects_dotted_keys_nest_tomls_rules() {
        let a = ast("p = {a.b=1 a.c=2 \"a.b\"=3 x.\"y z\".w=4}\n");
        assert_eq!(b"1", raw(&a, &[k("p"), k("a"), k("b")]));
        assert_eq!(b"2", raw(&a, &[k("p"), k("a"), k("c")]));
        assert_eq!(b"3", raw(&a, &[k("p"), k("a.b")]));
        assert_eq!(b"4", raw(&a, &[k("p"), k("x"), k("y z"), k("w")]));
        // An explicit `{…}` stays closed, inside the object and out of it.
        assert_eq!(err!(FigClosedFlowValue), fails("p = {a={b=1} a.c=2}\n"));
        assert_eq!(err!(FigClosedFlowValue), fails("p = {a.b=1}\np.a.c = 2\n"));
        assert_eq!(err!(FigDuplicateKey), fails("p = {a.b=1 a=2}\n"));
        assert_eq!(err!(FigKeyNotContainer), fails("p = {a=1 a.b=2}\n"));
    }

    #[test]
    fn compact_objects_keys_follow_the_bare_key_rules() {
        assert_eq!(err!(FigBadKey), fails("p = {@x=1}\n"));
        assert_eq!(err!(FigBadKey), fails("p = {-x=1}\n"));
        assert_eq!(err!(FigBadKey), fails("p = {[q=1}\n"));
        assert_eq!(err!(FigBadKey), fails("p = {a.b[0]=1}\n"));
        assert_eq!(err!(FigBadKey), fails("p = {a.b: 1}\n"));
    }

    #[test]
    fn flow_each_malformed_shape_has_its_own_code() {
        assert_eq!(err!(FigFlowMissingEquals), fails("p = {name=Adam Harris}\n"));
        assert_eq!(err!(FigFlowMissingEquals), fails("p = { x }\n"));
        assert_eq!(err!(FigQuotedTrailingContent), fails("p = {a=\"x\"b=2}\n"));
        assert_eq!(err!(FigQuotedTrailingContent), fails("p = [\"x\"y]\n"));
        assert_eq!(err!(FigFlowMissingComma), fails("p = [\"a\" \"b\"]\n"));
        assert_eq!(err!(FigFlowMissingComma), fails("p = [a\nb]\n"));
        assert_eq!(err!(FigFlowEmptyElement), fails("p = [a,,b]\n"));
        assert_eq!(err!(FigFlowEmptyElement), fails("p = [,]\n"));
        assert_eq!(err!(FigFlowEmptyElement), fails("p = {a=1,,b=2}\n"));
        assert_eq!(err!(FigFlowMultilineString), fails("p = {x = \"\"\"\nhi\n\"\"\"}\n"));
        assert_eq!(err!(FigFlowCommentedClose), fails("p = [see #3, y]\n"));
        assert_eq!(err!(FigUnclosedFlow), fails("p = [a, b\n"));
    }

    #[test]
    fn flow_json_vs_fig_split_bare_key_and_colon_is_the_foreign_syntax_error() {
        assert_eq!(err!(FigFlowBareKeyColon), fails("p = {x: 1}\n"));
    }

    #[test]
    fn flow_an_object_may_not_mix_equals_and_colon_separators() {
        assert_eq!(err!(FigMixedFlowSeparators), fails("p = {x = 1, \"y\": 2}\n"));
        assert_eq!(err!(FigMixedFlowSeparators), fails("p = {\"x\": 1, y = 2}\n"));
    }

    #[test]
    fn flow_infinity_and_nan_are_plain_strings() {
        let a = ast("p = [Infinity, NaN, -Infinity]\n");
        assert_eq!(b"Infinity", string(&a, &[k("p"), ix(0)]));
        assert_eq!(b"NaN", string(&a, &[k("p"), ix(1)]));
    }

    #[test]
    fn flow_jsonc_style_trailing_commas_and_hash_comments() {
        let a = ast("nums = [\n  1,   # one\n  2,   # two\n  3,\n]\nobj = { \"a\": 1, \"b\": 2, }   # trailing comma in a JSON object");
        assert_eq!(b"3", raw(&a, &[k("nums"), ix(2)]));
        assert_eq!(b"2", raw(&a, &[k("obj"), k("b")]));
    }

    #[test]
    fn glued_star_element_markers_parse() {
        let a = ast("ports\n>* 25565\n>* 25566\nservers\n>*\n>> host = a.com\n>*\n>> host = b.com");
        assert_eq!(b"25566", raw(&a, &[k("ports"), ix(1)]));
        assert_eq!(b"a.com", string(&a, &[k("servers"), ix(0), k("host")]));
        assert_eq!(b"b.com", string(&a, &[k("servers"), ix(1), k("host")]));
    }

    #[test]
    fn spaced_star_is_accepted_and_equals_glued() {
        let glued = ast("xs\n>* 1\n>* 2\n");
        let spaced = ast("xs\n> * 1\n> * 2\n");
        assert!(glued.eql(&spaced));
    }

    #[test]
    fn root_sequence_elements_are_bare_stars() {
        let a = ast("* first\n* second\n");
        assert_eq!(b"second", string(&a, &[ix(1)]));
    }

    #[test]
    fn dash_element_lines_are_a_foreign_syntax_error() {
        assert_eq!(err!(FigForeignSyntaxDash), fails("- 25565\n"));
        assert_eq!(err!(FigForeignSyntaxDash), fails("xs\n>- 1\n"));
        assert_eq!(err!(FigForeignSyntaxDash), fails("xs\n> - 1\n"));
        assert_eq!(err!(FigForeignSyntaxDash), fails("xs\n> - host = a\n"));
    }

    #[test]
    fn a_star_glued_to_its_value_is_a_bad_separator() {
        assert_eq!(err!(FigBadMarkerSeparator), fails("xs\n>*1\n"));
        assert_eq!(err!(FigBadMarkerSeparator), fails("*1\n"));
    }

    #[test]
    fn multiline_strings_raw_and_dedented() {
        let a = ast("license = '''\nline one\nline two\n'''\nbanner = \"\"\"\n    hi\n    there\n    \"\"\"");
        assert_eq!(b"line one\nline two", string(&a, &[k("license")]));
        assert_eq!(b"hi\nthere", string(&a, &[k("banner")]));
    }

    #[test]
    fn multiline_closer_is_line_anchored() {
        let a = ast("a = '''\n  x\n  '''\nb = '''\nhas ''' inside\n'''\nc = \"\"\"\n    hi\n  low\n    \"\"\"\n");
        // Raw: content lines keep their indentation; the CLOSER line's leading
        // whitespace is not content.
        assert_eq!(b"  x", string(&a, &[k("a")]));
        // A `'''` that is not the first non-whitespace content of its line is
        // ordinary content, never a closer.
        assert_eq!(b"has ''' inside", string(&a, &[k("b")]));
        // Smart-dedent: a line with LESS leading whitespace than the closer's
        // run loses whatever it has.
        assert_eq!(b"hi\nlow", string(&a, &[k("c")]));
    }

    #[test]
    fn content_after_a_multiline_opener_is_a_hard_error_an_opener_comment_is_not() {
        assert_eq!(err!(FigMultilineOpenerContent), fails("x = '''abc'''\n"));
        assert_eq!(err!(FigMultilineOpenerContent), fails("x = \"\"\"abc\nhi\n\"\"\"\n"));
        assert_eq!(err!(FigMultilineOpenerContent), fails("x = ''''\nhi\n'''\n"));
        let a = ast("x = '''  # note\nhi\n'''\n");
        assert_eq!(b"hi", string(&a, &[k("x")]));
    }

    #[test]
    fn crlf_input_parses_identically_to_lf() {
        let a = ast(
            "database\r\n> host = localhost\r\n> quoted = \"v\"  # c\r\n>>\r\nports\r\n> * 1\r\n> * 2\r\nblob = '''\r\na\r\nb\r\n'''\r\nxs[]\r\n> k = 1\r\n+\r\n> k = 2\r\n",
        );
        assert_eq!(b"localhost", string(&a, &[k("database"), k("host")]));
        assert_eq!(b"v", string(&a, &[k("database"), k("quoted")]));
        assert_eq!(b"2", raw(&a, &[k("ports"), ix(1)]));
        // Multiline content captures its line breaks as LF.
        assert_eq!(b"a\nb", string(&a, &[k("blob")]));
        assert_eq!(b"2", raw(&a, &[k("xs"), ix(1), k("k")]));
        // A final line terminated by a lone \r at EOF is closed the same way.
        let a2 = ast("section\r\n> a = 1\r");
        assert_eq!(b"1", raw(&a2, &[k("section"), k("a")]));
    }

    #[test]
    fn element_with_inline_field_is_a_hard_error() {
        assert_eq!(err!(FigElementInlineField), fails("xs\n> * host = a.com\n"));
    }

    #[test]
    fn skipped_level_errors() {
        assert_eq!(err!(FigSkippedLevel), fails("a\n>>> x = 1\n"));
    }

    #[test]
    fn root_marker_errors() {
        assert_eq!(err!(FigRootMarker), fails("> x = 1\n"));
    }

    #[test]
    fn mixed_container_children_errors() {
        assert_eq!(err!(FigMixedContainerChildren), fails("a\n> x = 1\n> *"));
    }

    #[test]
    fn duplicate_key_errors() {
        assert_eq!(err!(FigDuplicateKey), fails("x = 1\nx = 2\n"));
    }

    #[test]
    fn empty_childless_container_errors() {
        assert_eq!(err!(FigEmptyContainer), fails("a\nb = 1\n"));
    }

    #[test]
    fn foreign_syntax_guardrail_key_colon_value() {
        assert_eq!(err!(FigForeignSyntaxColon), fails("key: value\n"));
    }

    #[test]
    fn comments_leading_and_trailing() {
        let a = ast("logging\n> # a comment\n> level = info                # inline comment");
        let level_key = a.get_key_by_path(&[k("logging"), k("level")]).unwrap();
        assert_eq!(b"a comment", a.comments(level_key.id).leading[0].text);
        let level_val = a.get_val_by_path(&[k("logging"), k("level")]).unwrap();
        assert_eq!(b"inline comment", a.comments(level_val.id).trailing.unwrap().text);
    }

    #[test]
    fn plus_continuation_appends_another_element_to_the_last_append_header() {
        let a = ast("replacements[]\n> file = README.md\n> exactly = 0\n+\n> file = CHANGELOG.md\n> exactly = 1");
        assert_eq!(b"README.md", string(&a, &[k("replacements"), ix(0), k("file")]));
        assert_eq!(b"CHANGELOG.md", string(&a, &[k("replacements"), ix(1), k("file")]));
    }

    #[test]
    fn assignment_final_append_is_not_assignable() {
        assert_eq!(err!(FigAppendAssignment), fails("xs[] = 1\n"));
        assert_eq!(err!(FigAppendAssignment), fails("xs[0] = 1\nxs[] = 2\n"));
        // The fixes the message names both work: the explicit next index
        // appends…
        let a = ast("xs[0] = 1\nxs[1] = 2\n");
        assert_eq!(b"2", raw(&a, &[k("xs"), ix(1)]));
        // …and a mid-path `[]` ("the last element") still navigates fine.
        let mid = ast("xs[]\n> a = 1\nxs[].b = 2\n");
        assert_eq!(b"2", raw(&mid, &[k("xs"), ix(0), k("b")]));
    }

    #[test]
    fn a_non_final_append_on_an_empty_sequence_is_still_the_empty_append_error() {
        assert_eq!(err!(FigEmptyAppendTarget), fails("xs[].a = 1\n"));
    }

    #[test]
    fn an_appended_element_that_closes_empty_is_an_empty_container() {
        assert_eq!(err!(FigEmptyContainer), fails("xs[]\nb = 1\n"));
        assert_eq!(err!(FigEmptyContainer), fails("xs[]\n"));
        assert_eq!(err!(FigEmptyContainer), fails("xs[]\n> # note\n"));
        // A `+` re-run creates an element the same way.
        assert_eq!(err!(FigEmptyContainer), fails("xs[]\n> a = 1\n+\n"));
        let a = ast("xs[]\n> a = 1\n+\n> a = 2\n");
        assert_eq!(b"1", raw(&a, &[k("xs"), ix(0), k("a")]));
        assert_eq!(b"2", raw(&a, &[k("xs"), ix(1), k("a")]));
    }

    #[test]
    fn content_glued_to_a_plus_is_a_key_line() {
        assert_eq!(err!(FigBadKey), fails("xs[]\n> a = 1\n+glued = 2\n"));
        assert_eq!(err!(FigBadKey), fails("+x\n"));
    }

    #[test]
    fn plus_continuation_equals_a_repeated_append_header() {
        let plussed = ast("xs[]\n> a = 1\n+\n> a = 2\n");
        let repeated = ast("xs[]\n> a = 1\nxs[]\n> a = 2\n");
        assert!(plussed.eql(&repeated));
    }

    #[test]
    fn plus_re_runs_a_nested_append_header() {
        let a = ast("spec.containers[]\n> name = app\nspec.containers[].ports[]\n> port = 80\n+\n> port = 443");
        assert_eq!(b"443", raw(&a, &[k("spec"), k("containers"), ix(0), k("ports"), ix(1), k("port")]));
    }

    #[test]
    fn plus_chain_survives_blanks_and_comments() {
        let plussed = ast("xs[]\n> a = 1\n\n# note\n+\n> a = 2\n");
        let repeated = ast("xs[]\n> a = 1\n\n# note\nxs[]\n> a = 2\n");
        assert!(plussed.eql(&repeated));
        assert_eq!(b"2", raw(&plussed, &[k("xs"), ix(1), k("a")]));
    }

    #[test]
    fn dangling_plus_errors() {
        assert_eq!(err!(FigDanglingContinuation), fails("+\n> a = 1\n"));
        // A plain header (no `[]`) breaks the chain…
        assert_eq!(err!(FigDanglingContinuation), fails("xs[]\n> a = 1\nother\n> b = 1\n+\n> a = 2\n"));
        // …as does any other zero-marker structural line.
        assert_eq!(err!(FigDanglingContinuation), fails("xs[]\n> a = 1\nb = 2\n+\n> a = 2\n"));
        // A `+` carrying depth markers is never a continuation.
        assert_eq!(err!(FigDanglingContinuation), fails("xs[]\n> a = 1\n> +\n"));
    }

    #[test]
    fn flow_values_are_closed() {
        assert_eq!(err!(FigClosedFlowValue), fails("a = { x = 1 }\na.y = 2\n"));
        assert_eq!(err!(FigClosedFlowValue), fails("a = { x = 1 }\na\n> y = 2\n"));
        assert_eq!(err!(FigClosedFlowValue), fails("xs = [1, 2]\nxs[2] = 3\n"));
    }

    #[test]
    fn full_kitchen_sink_file_parses() {
        let src = include_bytes!("../../../../../languages/figl/src/testdata/kitchen_sink.figl");
        let a = parse_abstract(src, Type::Fig).unwrap();
        assert_eq!(b"2", raw(&a, &[k("version")]));
    }

    #[test]
    fn diagnostic_captures_the_failing_position() {
        // Line 3 (`level: info`) is the YAML-habit error; the caret pins to
        // the `:` (fail_at), not where the cursor stopped.
        let src = b"logging\n> format = json\n> level: info\n";
        let mut report = Report::default();
        assert_eq!(err!(FigForeignSyntaxColon), parse_with_report(src, Type::Fig, &mut report).unwrap_err());
        let d = report.diag.unwrap();
        assert_eq!(err!(FigForeignSyntaxColon), d.code);
        let loc = d.locate(src);
        assert_eq!(3, loc.line);
        assert_eq!(b"> level: info", loc.line_text);
    }

    #[test]
    fn diagnostic_renders_file_line_col_the_source_line_and_a_caret() {
        let src = b"key: value\n";
        let mut report = Report::default();
        let _ = parse_with_report(src, Type::Fig, &mut report);
        let rendered = report.diag.unwrap().render_alloc(src, b"app.figl");
        let rendered = String::from_utf8(rendered).unwrap();
        assert!(rendered.starts_with("app.figl:1:"));
        assert!(rendered.contains("error: `:` introduces a type, not a value"));
        assert!(rendered.contains("\n    key: value\n"));
        assert!(rendered.contains("^\n"));
    }

    #[test]
    fn report_is_empty_on_a_clean_parse() {
        let mut report = Report::default();
        parse_with_report(b"a = 1\n", Type::Fig, &mut report).unwrap();
        assert!(report.diag.is_none());
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn utf8_bom_before_document_is_ignored() {
        // Same tree as "root map with a scalar assignment" — the BOM must not
        // shift any offsets/keys/values.
        expect_parse(b"\xEF\xBB\xBFx = 1\n", 3, &x_equals_one());
    }

    #[test]
    fn invalid_utf8_bytes_are_rejected() {
        assert_eq!(err!(FigInvalidUtf8), parse_abstract(b"x = \xff\xfe\n", Type::Fig).unwrap_err());
    }

    /// Parse `input`, expecting success, and return the warning codes in
    /// order.
    fn collect_warn_codes(input: &str) -> Vec<WarningCode> {
        let mut report = Report::default();
        parse_with_report(input.as_bytes(), Type::Fig, &mut report).unwrap();
        report.warnings.iter().map(|w| w.code).collect()
    }

    fn expect_warn_codes(input: &str, expected: &[WarningCode]) {
        assert_eq!(expected, &collect_warn_codes(input)[..], "{input:?}");
    }

    #[test]
    fn coercion_warns_literal_lookalikes_and_leading_zeros() {
        use WarningCode::*;
        expect_warn_codes("norway = Yes\n", &[StringLooksLikeLiteral]);
        expect_warn_codes("switch = ON\n", &[StringLooksLikeLiteral]);
        expect_warn_codes("shout = TRUE\n", &[StringLooksLikeLiteral]);
        expect_warn_codes("nothing = Null\n", &[StringLooksLikeLiteral]);
        expect_warn_codes("zip = 007\n", &[StringLeadingZero]);
        // Float/exponent shapes with a leading zero fall to strings too.
        expect_warn_codes("f = 01.5\n", &[StringLeadingZero]);
        expect_warn_codes("f = 00.5\n", &[StringLeadingZero]);
        expect_warn_codes("f = -01e2\n", &[StringLeadingZero]);
        // Still not a number even without the zeros → stays silent.
        expect_warn_codes("v = 01.5.6\n", &[]);
        // A trailing-dot number lookalike (`1.`) falls to a string and warns.
        expect_warn_codes("sig = 1.\n", &[StringLooksLikeNumber]);
        expect_warn_codes("neg = -3.\n", &[StringLooksLikeNumber]);
        expect_warn_codes("ver = 1.2.3\n", &[]);
        // The real literals, prose, quotes, and annotations never warn.
        expect_warn_codes("ok = true\nn = null\nx = 0\nhex = 0xFF\n", &[]);
        expect_warn_codes("real = 1.0\n", &[]);
        expect_warn_codes("movie = 12 monkeys\ntitle = Yes Prime Minister\n", &[]);
        expect_warn_codes("flag = \"true\"\n", &[]);
        expect_warn_codes("norway: string = Yes\n", &[]);
        // A `: float` coercion silences the trailing-dot warn.
        expect_warn_codes("sig: float = 1.\n", &[]);
    }

    #[test]
    fn leading_dot_number_lookalike_falls_to_a_string_and_warns() {
        use WarningCode::*;
        expect_warn_codes("dot = .5\n", &[StringLooksLikeNumber]);
        expect_warn_codes("q = .25\n", &[StringLooksLikeNumber]);
        let a = ast("dot = .5\n");
        assert_eq!(b".5", string(&a, &[k("dot")]));
        // As narrow as the trailing-dot detector.
        expect_warn_codes("v = .5.6\n", &[]);
        expect_warn_codes("e = .e5\n", &[]);
        expect_warn_codes("d = ..\n", &[]);
        expect_warn_codes("s = -.5\n", &[]);
        // `: float` does NOT coerce a leading dot.
        assert_eq!(err!(FigTypeMismatch), fails("x: float = .5\n"));
    }

    #[test]
    fn coercion_warns_ambiguous_bare_time_only() {
        use WarningCode::*;
        expect_warn_codes("day = 2026-07-01\n", &[]);
        expect_warn_codes("meet = 10:30\n", &[AmbiguousDatetime]);
        expect_warn_codes("split = 10:30:00\n", &[AmbiguousDatetime]);
        expect_warn_codes("when = 2026-07-01T12:00:00Z\n", &[]);
        expect_warn_codes("local = 2026-07-01T12:00:00\n", &[]);
        expect_warn_codes("note = call mom at 3:30\n", &[]);
    }

    #[test]
    fn flow_like_string_warns_markdown_links_and_globs_stay_silent() {
        use WarningCode::*;
        expect_warn_codes("oops = [80, 443] x\n", &[FlowLikeString]);
        expect_warn_codes("link = [Blog](/Blog.md)\n", &[]);
        expect_warn_codes("glob = [a-z]*.md\n", &[]);
        expect_warn_codes("bb = [b]x[/b]\n", &[]);
        // A malformed prefix never parsed as flow, so it doesn't warn either.
        expect_warn_codes("odd = [80,, 443] x\n", &[]);
        // Flow-element position gets the same treatment.
        expect_warn_codes("xs = [[80, 443] x, y]\n", &[FlowLikeString]);
        expect_warn_codes("links = [[Blog](/Blog.md), [Resume](/Resume.md)]\n", &[]);
    }

    #[test]
    fn missing_comma_flow_list_warns() {
        use WarningCode::*;
        expect_warn_codes("p = [k = v]\n", &[FlowMissingComma]);
        expect_warn_codes("p = { \"k\": 1 y = 2 }\n", &[FlowMissingComma]);
        // A compact object's pairs need no comma.
        expect_warn_codes("p = { x = 1 y = 2 }\n", &[]);
        expect_warn_codes("p = { x = 1, y = 2 }\n", &[]);
    }

    #[test]
    fn indent_count_lint() {
        use WarningCode::*;
        // Correct 2×depth indentation and unindented spaced markers: clean.
        expect_warn_codes("a\n  > b = 1\n", &[]);
        expect_warn_codes("a\n> b\n> > c = 1\n", &[]);
        // Wrong width, indented root line, tab indent: each warns.
        expect_warn_codes("a\n > b = 1\n", &[IndentMarkerMismatch]);
        expect_warn_codes("  a = 1\n", &[IndentMarkerMismatch]);
        expect_warn_codes("a\n\t> b = 1\n", &[IndentMarkerMismatch]);
        // Comment lines participate.
        expect_warn_codes("a\n  # note\n> b = 1\n", &[IndentMarkerMismatch]);
        expect_warn_codes("a\n  > # note\n  > b = 1\n", &[]);
    }
}
