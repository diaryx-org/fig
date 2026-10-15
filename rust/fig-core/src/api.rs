//! Port of src/c_api.zig: what fig's C ABI does beyond its C plumbing.
//!
//! The C ABI is a set of handles (a document, a value tree, an editor) and
//! integer statuses. This module is that API in Rust, at the same level: the
//! same operations under the same names (`fig_editor_replace_val` is
//! `Editor::replace_val`), the same argument checks, the same reparse and
//! rollback, and the same answer for every error — the `Status` the bindings
//! and fig-conformance compare. What is left out is marshalling: pointers,
//! lengths, size-versioned structs, borrowed out-parameters and allocators.
//!
//! # Public API
//!
//! ```text
//! // Statuses (FigStatus), named as bindings/typescript/src/ffi.ts names them
//! pub enum Status { Ok, InvalidArgument, ParseError, OutOfMemory, UnsupportedFormat,
//!                   NotFound, UnsupportedOperation, InternalError }
//! impl Status { pub fn code(self) -> i32; pub fn from_code(i32) -> Option<Status>; pub fn name(self) -> &'static str }
//! pub fn edit_status(err: Error) -> Status;            // editStatus
//! pub fn serialize_status(err: Error) -> Status;       // serializeStatus
//! pub fn runtime_print_status(err: Error) -> Status;   // runtimePrintStatus
//! pub fn convert_status(err: Error) -> Status;         // convertStatus
//! pub fn parse_failure_status(err: Error) -> Status;   // parseFailureStatus
//! pub struct FigError { pub code: Status, pub byte_offset: usize, pub line: u32, pub column: u32, pub message: Vec<u8> }
//!
//! // Version and formats
//! pub fn version() -> u32; pub fn version_string() -> &'static str;
//! pub const CAP_READ, CAP_EDIT, CAP_SERIALIZE, CAP_REFERENCES: u32;
//! pub fn format_capabilities(format: i32) -> u32;
//! pub fn format_name(format: i32) -> Option<String>;
//! pub fn format_by_name(name: &str) -> i32;            // -1 for none
//! pub fn detect_order(input: &[u8], names: &[&str]) -> Result<i32, Status>;
//! pub struct FigStreamDoc { pub start: usize, pub end: usize, pub marked: bool }
//! pub fn stream_split(input: &[u8], format: i32) -> Result<Vec<FigStreamDoc>, FigError>;
//!
//! // Serialize options (FigSerializeOptions; `None` is the defaults)
//! pub struct FigSerializeOptions { pub pretty, pub indent: u8, pub strip_comments,
//!                                  pub lossless, pub width: u16, pub flow, pub splice }
//! pub fn serialize_options_of(Option<&FigSerializeOptions>) -> SerializeOptions;
//!
//! // Documents (fig_parse*, fig_document_*, fig_node_*, fig_keyvalue_*)
//! pub const NODE_NONE: NodeId;
//! pub enum FigNodeKind { Invalid = -1, Null, Bool, Int, Float, String, Sequence, Mapping, KeyValue, Alias, Extended }
//! impl Document {
//!     pub fn parse(input: &[u8], format: i32) -> Result<Document, Status>;
//!     pub fn parse_ex(input: &[u8], format: i32) -> Result<Document, FigError>;
//!     pub fn root(&self) -> NodeId;
//!     pub fn node_kind(&self, node) -> FigNodeKind;
//!     pub fn first_child / next_sibling / keyvalue_key / keyvalue_value(&self, node) -> NodeId;
//!     pub fn child_count(&self, node) -> usize;
//!     pub fn node_bool(&self, node) -> Option<bool>;
//!     pub fn node_number / node_string(&self, node) -> Option<&[u8]>;
//!     pub fn node_extended(&self, node) -> Option<(ExtKind, &[u8])>;
//!     pub fn resolve_aliases(&self) -> Result<Option<Document>, FigError>;
//!     pub fn serialize(&mut self, format, options) -> Result<&[u8], Status>;
//!     pub fn diagnose(&mut self, format, options) -> Result<usize, Status>;
//!     pub fn warning(&self, index) -> Result<&Warning, Status>;
//!     pub fn document(&self) -> &kit::document::Document; pub fn entry(&self) -> &Arc<Entry>;
//! }
//!
//! // Value trees (fig_value_*)
//! pub struct FigKeyValue { pub key: NodeId, pub value: NodeId }
//! impl Value {
//!     pub fn create() -> Value;
//!     pub fn null / bool / int / uint / number / string / extended / seq / map(...) -> Result<NodeId, Status>;
//!     pub fn serialize(&mut self, root, format) -> Result<&[u8], Status>;
//!     pub fn serialize_opts(&mut self, root, format, options) -> Result<&[u8], Status>;
//!     pub fn diagnose(&mut self, root, format, options) -> Result<usize, Status>;
//!     pub fn warning(&self, index) -> Result<&Warning, Status>;
//! }
//!
//! // Editors (fig_editor_*): every edit is `Result<(), Status>`, a getter
//! // `Result<Vec<u8>, Status>` (`NotFound` when there is no such comment).
//! impl Editor {
//!     pub fn create(input: &[u8], format: i32) -> Result<Editor, Status>;
//!     pub fn source(&self) -> &[u8];
//!     replace_val, replace_key, replace_named_key, set, replace_string, set_string,
//!     add_leading_comment, set_trailing_comment, delete_leading_comments,
//!     delete_trailing_comment, get_leading_comment, get_trailing_comment,
//!     add_dangling_comment, delete_dangling_comments, get_dangling_comment,
//!     comment_out, uncomment_leading, uncomment_dangling, insert_key,
//!     insert_named_key, delete_key, append_seq, prepend_seq, insert_seq_item,
//!     remove_seq_item, move_key, reorder_keys, move_item, reorder_items,
//!     set_sequence, delete_container, insert_container, rename_container,
//!     move_container, reorder_containers, append_container_to_seq
//! }
//! ```
//!
//! A format is the integer the registry hands out (`registry::Entry::abi`),
//! in the process-wide registry (`registry::global()`), as a C caller holds
//! one. Every edit goes through the editor over the registered language as
//! the C ABI's does: Zig's `Editor(Runtime.Language)`, which reads a parse
//! failure as `RuntimeParseFailed` and a renderer's refusal as
//! `RendererRefused` whatever the language called them, and answers "may
//! be, ask the entry" to the two section gates (`RuntimeLanguage` below).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::diagnostics::{self, Warning};
use crate::editor;
use crate::kit::ast::builder::{Builder, Entry as BuilderEntry};
use crate::kit::ast::{ExtKind, NodeId, NodeKind, NumberKind, PathSegment, SerializeOptions, AST};
use crate::kit::document::Document as KitDocument;
use crate::kit::language::{Caps, Dialect, DialectInfo, Language, ParseReport, RenderRequest, Renderer, StreamDocument, Syntax};
use crate::kit::table::Table;
use crate::lossless;
use crate::materialize::{self, TagMode};
use crate::registry::{self, Entry};
use crate::serialize::Target;
use crate::{err, Error};

// ============================================================================
// STATUS
// ============================================================================

/// Translation of fig's errors to the C ABI (`FigStatus`). The names are the
/// ones `bindings/typescript/src/ffi.ts`'s `Status` gives them, which is what
/// fig-conformance compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Ok,
    InvalidArgument,
    ParseError,
    OutOfMemory,
    UnsupportedFormat,
    NotFound,
    /// The operation is not defined for these arguments, though every
    /// argument is individually valid — as distinct from `InvalidArgument`
    /// (a malformed call) and `UnsupportedFormat` (a format this build
    /// cannot handle). Returned, for instance, by `uncomment_leading` when
    /// the lines turn out not to be an entry.
    UnsupportedOperation,
    InternalError,
}

impl Status {
    /// The C ABI's integer.
    pub fn code(self) -> i32 {
        match self {
            Status::Ok => 0,
            Status::InvalidArgument => 1,
            Status::ParseError => 2,
            Status::OutOfMemory => 3,
            Status::UnsupportedFormat => 4,
            Status::NotFound => 5,
            Status::UnsupportedOperation => 6,
            Status::InternalError => 255,
        }
    }

    pub fn from_code(code: i32) -> Option<Status> {
        Some(match code {
            0 => Status::Ok,
            1 => Status::InvalidArgument,
            2 => Status::ParseError,
            3 => Status::OutOfMemory,
            4 => Status::UnsupportedFormat,
            5 => Status::NotFound,
            6 => Status::UnsupportedOperation,
            255 => Status::InternalError,
            _ => return None,
        })
    }

    /// The binding's name for it (`InvalidArgument`, `NotFound`, …).
    pub fn name(self) -> &'static str {
        match self {
            Status::Ok => "Ok",
            Status::InvalidArgument => "InvalidArgument",
            Status::ParseError => "ParseError",
            Status::OutOfMemory => "OutOfMemory",
            Status::UnsupportedFormat => "UnsupportedFormat",
            Status::NotFound => "NotFound",
            Status::UnsupportedOperation => "UnsupportedOperation",
            Status::InternalError => "InternalError",
        }
    }
}

impl core::fmt::Display for Status {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for Status {}

/// Translate the editor/AST error set onto `Status` (`editStatus`).
pub fn edit_status(err: Error) -> Status {
    match err.name() {
        "OutOfMemory" => Status::OutOfMemory,
        "NotFound" => Status::NotFound,
        "NotAMapping" | "NotASequence" | "NotAContainer" | "InvalidDocument" => Status::InvalidArgument,
        // A key rename at a path that names no key: a sequence item, or the root.
        "NotAKey" => Status::InvalidArgument,
        // `set_sequence` declines a shape it can't safely diff.
        "UnsupportedShape" => Status::InvalidArgument,
        // TOML structural edits reject a request that doesn't match the
        // document shape: caller errors, not malformed-source reparse failures.
        "NotASection" | "NotAnInlineArray" | "NotAnArrayOfTables" | "SectionExists" | "DuplicateKey" | "MergeOnlyKey" => {
            Status::InvalidArgument
        }
        // The pre-op guards: a scalar op addressed a whole scattered container.
        "CannotDeleteSection" | "CannotReplaceSection" => Status::InvalidArgument,
        // The same family for the two ops that relocate an entry's block.
        "CannotMoveSection" | "CannotReorderSections" => Status::InvalidArgument,
        // NestedText declines two shapes outright.
        "EmptyInlineContainer" | "KeyRequiresMultilineForm" => Status::InvalidArgument,
        // A runtime language's renderer declined the text: the request, not
        // the source.
        "RendererRefused" => Status::InvalidArgument,
        // A block-spelled value handed to a splice into a flow container.
        "BlockValueIntoFlow" => Status::InvalidArgument,
        // A container that closes on its last entry's line.
        "ContainerClosesOnItsLine" => Status::InvalidArgument,
        // An insert into a section with no header line of its own.
        "ImplicitSection" => Status::InvalidArgument,
        // The target dialect has no comment syntax (strict JSON).
        "CommentsUnsupported" => Status::UnsupportedFormat,
        // The addressed node shares its parent's line inside a flow
        // collection: a fact about the request's path.
        "CommentsUnanchored" => Status::InvalidArgument,
        // The uncommented lines parsed to a document whose other nodes moved;
        // rolled back.
        "CommentNotAnEntry" => Status::UnsupportedOperation,
        // A trailing comment parsed back as part of the value; rolled back.
        "CommentInsideValue" => Status::UnsupportedOperation,
        // A delete whose splice changed more than the one entry; rolled back.
        "DeleteChangedOtherValues" => Status::UnsupportedOperation,
        // An edit whose splice parsed to more changed than asked; rolled back.
        "EditReadsBackWrong" => Status::UnsupportedOperation,
        // Deleting the only item of a root list where the empty document is
        // an empty mapping.
        "NoEmptyRootList" => Status::UnsupportedOperation,
        // A value the format cannot represent at all.
        "NullUnsupported" | "NonFiniteNumber" => Status::UnsupportedFormat,
        // Multi-line trailing comment text, or plist comment text with `--`.
        "MultilineComment" | "InvalidComment" => Status::InvalidArgument,
        "NotInitialized" | "MultipleInit" | "InvalidSpan" => Status::InternalError,
        // A reparse after a malformed edit lands here.
        _ => Status::ParseError,
    }
}

/// Translate a serialize error onto `Status` (`serializeStatus`): a
/// representability failure is `UnsupportedFormat`; the writer's only other
/// failure is allocation.
pub fn serialize_status(err: Error) -> Status {
    match err.name() {
        "WriteFailed" => Status::OutOfMemory,
        _ => Status::UnsupportedFormat,
    }
}

/// `serialize_status` for a printer reached through the registry
/// (`runtimePrintStatus`): its refusal is the format saying it cannot write
/// the tree.
pub fn runtime_print_status(err: Error) -> Status {
    match err.name() {
        "OutOfMemory" | "WriteFailed" => Status::OutOfMemory,
        _ => Status::UnsupportedFormat,
    }
}

/// A materialization failure onto `Status` (`convertStatus`): an
/// un-collapsible reference layer the target cannot represent.
pub fn convert_status(err: Error) -> Status {
    match err.name() {
        "OutOfMemory" => Status::OutOfMemory,
        _ => Status::UnsupportedFormat,
    }
}

/// `parseFailureStatus`.
pub fn parse_failure_status(err: Error) -> Status {
    match err.name() {
        "OutOfMemory" => Status::OutOfMemory,
        _ => Status::ParseError,
    }
}

/// A failure with what the C ABI's `FigError` carries: the status, and for
/// a parse, the message and where. `byte_offset`, `line` and `column` are 0
/// for "unknown"; `message` is at most 255 bytes, as the C struct's inline
/// buffer holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FigError {
    pub code: Status,
    pub byte_offset: usize,
    pub line: u32,
    pub column: u32,
    pub message: Vec<u8>,
}

/// The C struct's message buffer, less its NUL.
const MESSAGE_CAP: usize = 255;

/// `fillError`: `status` with `message`, its location unknown.
fn fill_error(status: Status, message: &[u8]) -> FigError {
    let n = message.len().min(MESSAGE_CAP);
    FigError { code: status, byte_offset: 0, line: 0, column: 0, message: message[..n].to_vec() }
}

impl core::fmt::Display for FigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.message.is_empty() {
            f.write_str(self.code.name())
        } else {
            write!(f, "{}: {}", self.code.name(), String::from_utf8_lossy(&self.message))
        }
    }
}

impl std::error::Error for FigError {}

// ============================================================================
// VERSION + CAPABILITIES
// ============================================================================

/// Packed library version `(major << 16) | (minor << 8) | patch`.
pub fn version() -> u32 {
    let mut parts = env!("CARGO_PKG_VERSION").split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    (major << 16) | (minor << 8) | patch
}

/// The library's semantic version string.
pub fn version_string() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// `parse` accepts this format.
pub const CAP_READ: u32 = 1 << 0;
/// The editor accepts this format.
pub const CAP_EDIT: u32 = 1 << 1;
/// `serialize` can write this format.
pub const CAP_SERIALIZE: u32 = 1 << 2;
/// The format has a reference layer, which `Document::serialize` collapses
/// when a document leaves it for a format without one.
pub const CAP_REFERENCES: u32 = 1 << 3;

fn caps_bits(caps: Caps) -> u32 {
    let mut bits = 0;
    if caps.read {
        bits |= CAP_READ;
    }
    if caps.edit {
        bits |= CAP_EDIT;
    }
    if caps.serialize {
        bits |= CAP_SERIALIZE;
    }
    if caps.references {
        bits |= CAP_REFERENCES;
    }
    bits
}

/// The registered dialect a format handle names, or `None` for one never
/// handed out (`entryOf`).
fn entry_of(format: i32) -> Option<Arc<Entry>> {
    registry::global().entry_by_abi(format)
}

/// What fig can do with `format`, as `CAP_*` bits; 0 for a handle never
/// handed out.
pub fn format_capabilities(format: i32) -> u32 {
    match entry_of(format) {
        Some(e) => caps_bits(e.caps()),
        None => 0,
    }
}

/// The name of the dialect `format` names.
pub fn format_name(format: i32) -> Option<String> {
    entry_of(format).map(|e| e.name.clone())
}

/// The format integer of the dialect named `name`, or -1.
pub fn format_by_name(name: &str) -> i32 {
    registry::global().entry_by_name(name).map_or(-1, |e| e.abi)
}

/// Content sniffing in the caller's order: the format integer of the first
/// of `names` whose parser accepts `input`; `NotFound` when none does.
pub fn detect_order(input: &[u8], names: &[&str]) -> Result<i32, Status> {
    match registry::global().detect_among(input, names) {
        Some(e) => Ok(e.abi),
        None => Err(Status::NotFound),
    }
}

/// One document of a stream: a byte range of the input and whether it
/// opened with a document-start marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FigStreamDoc {
    pub start: usize,
    pub end: usize,
    pub marked: bool,
}

/// Locate the documents of a multi-document stream in `format`.
/// `UnsupportedOperation` for a format without streams; `ParseError` when
/// the language refuses the input.
pub fn stream_split(input: &[u8], format: i32) -> Result<Vec<FigStreamDoc>, FigError> {
    let Some(e) = entry_of(format) else {
        return Err(fill_error(Status::UnsupportedFormat, b""));
    };
    match e.split_stream(input) {
        Ok(docs) => Ok(docs.iter().map(|d| FigStreamDoc { start: d.start, end: d.end, marked: d.explicit }).collect()),
        Err(err) => Err(match err.name() {
            "UnsupportedOperation" => fill_error(Status::UnsupportedOperation, b""),
            "OutOfMemory" => fill_error(Status::OutOfMemory, b""),
            _ => fill_error(Status::ParseError, b"the language could not split the stream"),
        }),
    }
}

// ============================================================================
// SERIALIZE OPTIONS
// ============================================================================

/// Output style for the `serialize` calls (`FigSerializeOptions`). `None`
/// where one is taken selects these defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FigSerializeOptions {
    /// Multi-line, indented output (default); `false`: compact single-line.
    pub pretty: bool,
    /// Spaces per indent level when `pretty`. 0 is read as the default (2).
    pub indent: u8,
    /// Drop comments carried on the value instead of emitting them.
    pub strip_comments: bool,
    /// `Document::serialize` only: preserve values the target cannot hold
    /// through a `$fig` envelope. Ignored by `Value::serialize_opts`.
    pub lossless: bool,
    /// The column budget for TOML, YAML and figl's inline-vs-expanded
    /// layout. 0 is read as the default (80), not as "never inline".
    pub width: u16,
    /// `Value::serialize_opts` only: render a container root inline, in
    /// flow — what the bindings' editor splice paths ask for.
    pub flow: bool,
    /// `Value::serialize_opts` only: render the value as the editor takes
    /// it spliced into a document.
    pub splice: bool,
}

impl Default for FigSerializeOptions {
    fn default() -> Self {
        FigSerializeOptions { pretty: true, indent: 2, strip_comments: false, lossless: false, width: 80, flow: false, splice: false }
    }
}

/// The core options for `options` (`None` ⇒ defaults) (`serializeOptionsOf`).
pub fn serialize_options_of(options: Option<&FigSerializeOptions>) -> SerializeOptions {
    let Some(o) = options else { return SerializeOptions::default() };
    SerializeOptions {
        pretty: o.pretty,
        indent: if o.indent == 0 { 2 } else { o.indent },
        strip_comments: o.strip_comments,
        width: if o.width == 0 { 80 } else { o.width },
        flow: o.flow,
        splice: o.splice,
        ..SerializeOptions::default()
    }
}

/// Whether the caller asked for lossless conversion (`losslessRequested`).
fn lossless_requested(options: Option<&FigSerializeOptions>) -> bool {
    options.is_some_and(|o| o.lossless)
}

/// `Diagnostics.Options` from the serialize options (`diagnoseOptionsOf`).
fn diagnose_options_of(options: Option<&FigSerializeOptions>) -> diagnostics::Options {
    let so = serialize_options_of(options);
    diagnostics::Options {
        pretty: so.pretty,
        strip_comments: so.strip_comments,
        lossless: lossless_requested(options),
        ..diagnostics::Options::default()
    }
}

/// A serialize target: the registered dialect a format handle names, when
/// it can be written (`targetOf`). A handle never handed out, or one whose
/// language does not serialize, is `UnsupportedFormat` to every serialize
/// and diagnose call alike.
fn target_of(format: i32) -> Option<Arc<Entry>> {
    let e = entry_of(format)?;
    if !e.caps().serialize {
        return None;
    }
    Some(e)
}

// ============================================================================
// DOCUMENTS
// ============================================================================

/// `FigNodeId`'s "no node".
pub const NODE_NONE: NodeId = 0xFFFF_FFFF;

/// An AST node's kind as the C ABI reports it (`FigNodeKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FigNodeKind {
    Invalid = -1,
    Null = 0,
    Bool = 1,
    Int = 2,
    Float = 3,
    String = 4,
    Sequence = 5,
    Mapping = 6,
    KeyValue = 7,
    Alias = 8,
    /// A format-specific scalar with no C type of its own (a TOML datetime,
    /// a ZON enum or char literal, a plist date): `node_extended` says
    /// which, and gives its text.
    Extended = 9,
}

/// A parsed document (`FigDocument`): its source, its parse, the dialect it
/// was parsed as, and the warnings of its most recent `diagnose`.
pub struct Document {
    document: KitDocument,
    entry: Arc<Entry>,
    rendered: Vec<u8>,
    diag_warnings: Vec<Warning>,
}

impl core::fmt::Debug for Document {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Document").field("format", &self.entry.name).finish()
    }
}

impl Document {
    /// `fig_parse`: `parse_ex` without the report.
    pub fn parse(input: &[u8], format: i32) -> Result<Document, Status> {
        Self::parse_ex(input, format).map_err(|e| e.code)
    }

    /// `fig_parse_ex`: parse `input` as `format`. Empty input is handed to
    /// the parser, which judges it per format. On a parse failure the error
    /// carries what the language's parser said, and where.
    pub fn parse_ex(input: &[u8], format: i32) -> Result<Document, FigError> {
        let Some(entry) = entry_of(format) else {
            return Err(fill_error(Status::UnsupportedFormat, b"unsupported or unknown format"));
        };
        let mut parser = registry::Parser::new();
        let doc = match parser.parse(input, &entry) {
            Ok(d) => d,
            Err(err) => {
                if err.is("OutOfMemory") {
                    return Err(fill_error(Status::OutOfMemory, b"out of memory"));
                }
                let mut e = fill_error(Status::ParseError, parser.last_message());
                e.byte_offset = parser.last_offset;
                return Err(e);
            }
        };
        Ok(Document { document: doc, entry, rendered: Vec::new(), diag_warnings: Vec::new() })
    }

    /// The parse.
    pub fn document(&self) -> &KitDocument {
        &self.document
    }

    /// The dialect the document was parsed as.
    pub fn entry(&self) -> &Arc<Entry> {
        &self.entry
    }

    fn ast(&self) -> &AST {
        &self.document.ast
    }

    fn in_range(&self, id: NodeId) -> bool {
        (id as usize) < self.ast().node_count()
    }

    /// `fig_document_root`.
    pub fn root(&self) -> NodeId {
        let ast = self.ast();
        if (ast.root as usize) >= ast.node_count() { NODE_NONE } else { ast.root }
    }

    /// `fig_node_kind`.
    pub fn node_kind(&self, node: NodeId) -> FigNodeKind {
        if !self.in_range(node) {
            return FigNodeKind::Invalid;
        }
        match self.ast().kind_of(node) {
            NodeKind::Null => FigNodeKind::Null,
            NodeKind::Boolean(_) => FigNodeKind::Bool,
            NodeKind::String(_) => FigNodeKind::String,
            NodeKind::Number(n) => match n.kind {
                NumberKind::Integer => FigNodeKind::Int,
                NumberKind::Float => FigNodeKind::Float,
            },
            NodeKind::Extended(_) => FigNodeKind::Extended,
            NodeKind::Sequence(_) => FigNodeKind::Sequence,
            NodeKind::Mapping(_) => FigNodeKind::Mapping,
            NodeKind::KeyValue(_) => FigNodeKind::KeyValue,
            NodeKind::Alias(_) => FigNodeKind::Alias,
        }
    }

    /// `fig_node_first_child`.
    pub fn first_child(&self, node: NodeId) -> NodeId {
        if !self.in_range(node) {
            return NODE_NONE;
        }
        match self.ast().kind_of(node) {
            NodeKind::Sequence(first) | NodeKind::Mapping(first) => first.unwrap_or(NODE_NONE),
            _ => NODE_NONE,
        }
    }

    /// `fig_node_next_sibling`.
    pub fn next_sibling(&self, node: NodeId) -> NodeId {
        if !self.in_range(node) {
            return NODE_NONE;
        }
        self.ast().node(node).next_sibling.unwrap_or(NODE_NONE)
    }

    /// `fig_node_child_count`.
    pub fn child_count(&self, node: NodeId) -> usize {
        if !self.in_range(node) {
            return 0;
        }
        let ast = self.ast();
        let mut current = match ast.kind_of(node) {
            NodeKind::Sequence(first) | NodeKind::Mapping(first) => first,
            _ => return 0,
        };
        let mut count = 0;
        while let Some(id) = current {
            if (id as usize) >= ast.node_count() {
                break;
            }
            count += 1;
            current = ast.node(id).next_sibling;
        }
        count
    }

    /// `fig_keyvalue_key`.
    pub fn keyvalue_key(&self, node: NodeId) -> NodeId {
        if !self.in_range(node) {
            return NODE_NONE;
        }
        match self.ast().kind_of(node) {
            NodeKind::KeyValue(kv) => kv.key,
            _ => NODE_NONE,
        }
    }

    /// `fig_keyvalue_value`.
    pub fn keyvalue_value(&self, node: NodeId) -> NodeId {
        if !self.in_range(node) {
            return NODE_NONE;
        }
        match self.ast().kind_of(node) {
            NodeKind::KeyValue(kv) => kv.value,
            _ => NODE_NONE,
        }
    }

    /// `fig_node_bool`.
    pub fn node_bool(&self, node: NodeId) -> Option<bool> {
        if !self.in_range(node) {
            return None;
        }
        match self.ast().kind_of(node) {
            NodeKind::Boolean(b) => Some(b),
            _ => None,
        }
    }

    /// `fig_node_number`: a number's lexeme; a char literal reads out as its
    /// decimal codepoint.
    pub fn node_number(&self, node: NodeId) -> Option<&[u8]> {
        if !self.in_range(node) {
            return None;
        }
        match self.ast().kind_of(node) {
            NodeKind::Number(n) => Some(n.raw),
            NodeKind::Extended(ext) if ext.kind == ExtKind::CharLiteral => Some(ext.text),
            _ => None,
        }
    }

    /// `fig_node_string`: a string; datetimes and enum literals read out as
    /// their text (a char literal is a number, read by `node_number`).
    pub fn node_string(&self, node: NodeId) -> Option<&[u8]> {
        if !self.in_range(node) {
            return None;
        }
        match self.ast().kind_of(node) {
            NodeKind::String(s) => Some(s),
            NodeKind::Extended(ext) if ext.kind != ExtKind::CharLiteral => Some(ext.text),
            _ => None,
        }
    }

    /// `fig_node_extended`: the precise kind and text of an extended scalar.
    pub fn node_extended(&self, node: NodeId) -> Option<(ExtKind, &[u8])> {
        if !self.in_range(node) {
            return None;
        }
        match self.ast().kind_of(node) {
            NodeKind::Extended(ext) => Some((ext.kind, ext.text)),
            _ => None,
        }
    }

    /// `fig_document_resolve_aliases`: a new document holding this one's
    /// values with the reference layer resolved — each alias a copy of the
    /// node its anchor names, each `<<` merge flattened, tags kept as
    /// written. `None` when there is nothing to resolve, so the caller reads
    /// this document itself. An alias with no value is a `ParseError` at the
    /// alias.
    pub fn resolve_aliases(&self) -> Result<Option<Document>, FigError> {
        if !self.entry.caps().references || !has_alias_or_merge(self.ast()) {
            return Ok(None);
        }
        let mut culprit: Option<NodeId> = None;
        let ast = match materialize::materialize_reporting(self.ast(), TagMode::Keep, &mut culprit) {
            Ok(a) => a,
            Err(err) => {
                if err.is("OutOfMemory") {
                    return Err(fill_error(Status::OutOfMemory, b"out of memory"));
                }
                return Err(alias_failed(err, &self.document, culprit));
            }
        };
        Ok(Some(Document {
            document: KitDocument { source: Arc::from(&b""[..]), ast },
            entry: self.entry.clone(),
            rendered: Vec::new(),
            diag_warnings: Vec::new(),
        }))
    }

    /// `fig_document_serialize`: the whole document in `format`, through the
    /// pipeline the CLI's `get` prints from (`prepare_document_ast`).
    pub fn serialize(&mut self, format: i32, options: Option<&FigSerializeOptions>) -> Result<&[u8], Status> {
        let Some(target) = target_of(format) else { return Err(Status::UnsupportedFormat) };
        let opts = serialize_options_of(options);
        let prepared = prepare_document_ast(&self.document.ast, &self.entry, &target, options).map_err(convert_status)?;
        let ast = prepared.as_ref().unwrap_or(&self.document.ast);
        let mut out = Vec::new();
        registry::print_with(&target, ast, &opts, &mut out).map_err(runtime_print_status)?;
        self.rendered = out;
        Ok(&self.rendered)
    }

    /// `fig_document_diagnose`: how many lossy events serializing the
    /// document to `format` would produce, over the same pipeline
    /// `serialize` prints from. The set is kept for `warning`.
    pub fn diagnose(&mut self, format: i32, options: Option<&FigSerializeOptions>) -> Result<usize, Status> {
        let Some(target) = target_of(format) else { return Err(Status::UnsupportedFormat) };
        // A failed analyze must not leave a stale set indexable.
        self.diag_warnings.clear();
        let prepared = prepare_document_ast(&self.document.ast, &self.entry, &target, options).map_err(convert_status)?;
        let ast = prepared.as_ref().unwrap_or(&self.document.ast);
        let warnings = diagnostics::analyze(ast, ast.root, &Target::Language(target), &diagnose_options_of(options));
        self.diag_warnings = warnings;
        Ok(self.diag_warnings.len())
    }

    /// `fig_document_warning`: the warning at `index` from the most recent
    /// `diagnose`; `InvalidArgument` out of range or before any.
    pub fn warning(&self, index: usize) -> Result<&Warning, Status> {
        self.diag_warnings.get(index).ok_or(Status::InvalidArgument)
    }
}

/// Whether `ast` holds anything `resolve_aliases` would collapse: an alias
/// node, or an entry keyed with the language's merge key.
fn has_alias_or_merge(ast: &AST) -> bool {
    for i in 0..ast.node_count() {
        match ast.kind_of(i as NodeId) {
            NodeKind::Alias(_) => return true,
            NodeKind::KeyValue(kv) => {
                if let NodeKind::String(k) = ast.kind_of(kv.key) {
                    if ast.merge_key.is_some_and(|mk| mk == k) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// An alias materialize could not resolve, as a `ParseError` at the node it
/// failed on: the alias itself, or the mapping whose `<<` merge it is.
fn alias_failed(err: Error, doc: &KitDocument, culprit: Option<NodeId>) -> FigError {
    let alias: Option<&[u8]> = culprit.and_then(|id| match doc.ast.kind_of(id) {
        NodeKind::Alias(name) => Some(name),
        _ => None,
    });
    let message: Vec<u8> = match err.name() {
        "UndefinedAlias" => match alias {
            Some(name) => format!("alias `*{}` names an anchor that is not defined before it", String::from_utf8_lossy(name)).into_bytes(),
            None => b"a `<<` merge names an anchor that is not defined before it".to_vec(),
        },
        "AliasCycle" => match alias {
            Some(name) => format!("alias `*{}` refers to a node that contains it, so it has no value", String::from_utf8_lossy(name)).into_bytes(),
            None => b"a `<<` merge refers to a mapping that contains it, so it has no value".to_vec(),
        },
        "TooDeep" => b"aliases refer to aliases too deeply to resolve".to_vec(),
        _ => b"an alias cannot be resolved".to_vec(),
    };
    // Zig formats into a 256-byte buffer and falls back to the generic
    // sentence when the name does not fit.
    let message = if message.len() > 256 {
        match err.name() {
            "UndefinedAlias" => b"an alias names an anchor that is not defined before it".to_vec(),
            _ => b"an alias refers to a node that contains it, so it has no value".to_vec(),
        }
    } else {
        message
    };
    let mut e = fill_error(Status::ParseError, &message);
    let Some(id) = culprit else { return e };
    if (id as usize) >= doc.ast.node_count() {
        return e;
    }
    let offset = doc.span_of(id).start;
    if offset > doc.source.len() {
        return e;
    }
    let before = &doc.source[..offset];
    let line_start = before.iter().rposition(|&b| b == b'\n').map_or(0, |nl| nl + 1);
    e.byte_offset = offset;
    e.line = (before.iter().filter(|&&b| b == b'\n').count() + 1) as u32;
    e.column = (offset - line_start + 1) as u32;
    e
}

/// The source→target AST pipeline `Document::serialize` prints from
/// (`prepareDocumentAst`): collapse the reference layer when leaving a
/// language that has one for one that has not (strict tags), then — under
/// `lossless` — decode any `$fig` envelopes and re-encode for the target.
/// `None` when no transform applies (print `ast` itself).
fn prepare_document_ast(
    ast: &AST,
    source: &Arc<Entry>,
    target: &Arc<Entry>,
    options: Option<&FigSerializeOptions>,
) -> crate::Result<Option<AST>> {
    let source_refs = source.caps().references;
    let ref_layer_round_trip = source_refs && target.caps().references;
    let materialized = if source_refs && !ref_layer_round_trip { Some(materialize::materialize(ast, TagMode::Strict)?) } else { None };
    let base = materialized.as_ref().unwrap_or(ast);
    if lossless_requested(options) && !ref_layer_round_trip {
        let native = lossless::native_for(&Target::Language(target.clone()));
        let decoded = lossless::decode(base)?;
        let Some(n) = native else { return Ok(Some(decoded)) };
        return Ok(Some(lossless::encode(&decoded, n)?));
    }
    Ok(materialized)
}

// ============================================================================
// VALUES
// ============================================================================

/// A `key: value` entry for `Value::map`; both are ids from earlier calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FigKeyValue {
    pub key: NodeId,
    pub value: NodeId,
}

/// A value tree built node by node, bottom-up (`FigValue`), then rendered
/// to any format that serializes.
#[derive(Default)]
pub struct Value {
    builder: Builder,
    rendered: Vec<u8>,
    diag_warnings: Vec<Warning>,
}

impl core::fmt::Debug for Value {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Value").field("nodes", &self.builder.node_count()).finish()
    }
}

/// A builder call's id (`emitNode`): construction fails only on allocation.
fn emit_node(result: crate::Result<NodeId>) -> Result<NodeId, Status> {
    result.map_err(|_| Status::OutOfMemory)
}

impl Value {
    /// `fig_value_create`.
    pub fn create() -> Value {
        Value::default()
    }

    /// How many nodes have been added.
    pub fn node_count(&self) -> usize {
        self.builder.node_count()
    }

    pub fn null(&mut self) -> Result<NodeId, Status> {
        emit_node(self.builder.add_null())
    }

    pub fn bool(&mut self, b: bool) -> Result<NodeId, Status> {
        emit_node(self.builder.add_bool(b))
    }

    pub fn int(&mut self, n: i64) -> Result<NodeId, Status> {
        emit_node(self.builder.add_int(n))
    }

    pub fn uint(&mut self, n: u64) -> Result<NodeId, Status> {
        emit_node(self.builder.add_uint(n))
    }

    /// A numeric scalar from already-formatted text; `is_float` records its
    /// kind. The float entry point, and the escape hatch for integers
    /// outside the i64/u64 range.
    pub fn number(&mut self, raw: &[u8], is_float: bool) -> Result<NodeId, Status> {
        emit_node(self.builder.add_number_raw(raw, is_float))
    }

    pub fn string(&mut self, s: &[u8]) -> Result<NodeId, Status> {
        emit_node(self.builder.add_string(s))
    }

    /// An extended scalar of `kind` (`FigExtKind`, the `ExtKind` ordinal);
    /// `InvalidArgument` for a kind fig does not know.
    pub fn extended(&mut self, kind: i32, text: &[u8]) -> Result<NodeId, Status> {
        let Some(ext) = u8::try_from(kind).ok().and_then(ExtKind::from_ordinal) else {
            return Err(Status::InvalidArgument);
        };
        emit_node(self.builder.add_extended(ext, text))
    }

    /// A sequence of `items`, each an id already added.
    pub fn seq(&mut self, items: &[NodeId]) -> Result<NodeId, Status> {
        let count = self.builder.node_count();
        if items.iter().any(|&id| id as usize >= count) {
            return Err(Status::InvalidArgument);
        }
        emit_node(self.builder.add_sequence(items))
    }

    /// A mapping of `entries`, each naming ids already added.
    pub fn map(&mut self, entries: &[FigKeyValue]) -> Result<NodeId, Status> {
        let count = self.builder.node_count();
        let mut es = Vec::with_capacity(entries.len());
        for c in entries {
            if c.key as usize >= count || c.value as usize >= count {
                return Err(Status::InvalidArgument);
            }
            es.push(BuilderEntry { key: c.key, value: c.value });
        }
        emit_node(self.builder.add_mapping(&es))
    }

    /// `fig_value_serialize`: `serialize_opts` with the defaults.
    pub fn serialize(&mut self, root: NodeId, format: i32) -> Result<&[u8], Status> {
        self.serialize_opts(root, format, None)
    }

    /// `fig_value_serialize_opts`: the subtree at `root` rendered in
    /// `format`. A built value is never a whole document, so it is printed
    /// as a fragment (`print_node_with`): the subtree is the printer's
    /// document.
    pub fn serialize_opts(&mut self, root: NodeId, format: i32, options: Option<&FigSerializeOptions>) -> Result<&[u8], Status> {
        let Some(target) = target_of(format) else { return Err(Status::UnsupportedFormat) };
        if root as usize >= self.builder.node_count() {
            return Err(Status::InvalidArgument);
        }
        let opts = serialize_options_of(options);
        let ast = self.builder.view(root).map_err(|_| Status::OutOfMemory)?;
        let mut out = Vec::new();
        registry::print_node_with(&target, ast, root, &opts, &mut out).map_err(runtime_print_status)?;
        self.rendered = out;
        Ok(&self.rendered)
    }

    /// `fig_value_diagnose`: how many lossy events serializing the subtree
    /// at `root` to `format` would produce (`lossless` is ignored: a built
    /// value has no envelopes). The set is kept for `warning`.
    pub fn diagnose(&mut self, root: NodeId, format: i32, options: Option<&FigSerializeOptions>) -> Result<usize, Status> {
        let Some(target) = target_of(format) else { return Err(Status::UnsupportedFormat) };
        if root as usize >= self.builder.node_count() {
            return Err(Status::InvalidArgument);
        }
        self.diag_warnings.clear();
        let ast = self.builder.view(root).map_err(|_| Status::OutOfMemory)?;
        let warnings = diagnostics::analyze(ast, root, &Target::Language(target), &diagnose_options_of(options));
        self.diag_warnings = warnings;
        Ok(self.diag_warnings.len())
    }

    /// `fig_value_warning`.
    pub fn warning(&self, index: usize) -> Result<&Warning, Status> {
        self.diag_warnings.get(index).ok_or(Status::InvalidArgument)
    }
}

// ============================================================================
// EDITING
// ============================================================================

/// The longest path an edit takes (`max_path_len`).
pub const MAX_PATH_LEN: usize = 128;
/// The longest key, item or container-name list an edit takes
/// (`max_keys_len`).
pub const MAX_KEYS_LEN: usize = 512;

fn check_path(path: &[PathSegment<'_>]) -> Result<(), Status> {
    if path.len() > MAX_PATH_LEN { Err(Status::InvalidArgument) } else { Ok(()) }
}

fn check_keys(keys: &[&[u8]]) -> Result<(), Status> {
    if keys.len() > MAX_KEYS_LEN { Err(Status::InvalidArgument) } else { Ok(()) }
}

fn done(r: crate::Result<()>) -> Result<(), Status> {
    r.map_err(edit_status)
}

/// A comment read's answer: `None` — no such comment — is `NotFound`, which
/// is what tells an absent comment from a present-but-empty one
/// (`fillCommentOut`).
fn comment_out(r: crate::Result<Option<Vec<u8>>>) -> Result<Vec<u8>, Status> {
    match r {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) => Err(Status::NotFound),
        Err(e) => Err(edit_status(e)),
    }
}

/// An editor over a whole document in a registered format (`FigEditor`):
/// edits splice bytes in place and reparse, so everything outside the edit
/// is kept byte for byte. An edit that fails leaves the source as it was.
pub struct Editor {
    inner: editor::Editor<'static>,
    entry: Arc<Entry>,
}

impl core::fmt::Debug for Editor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Editor").field("format", &self.entry.name).finish()
    }
}

impl Editor {
    /// `fig_editor_create`: an editor over a copy of `input` in `format`.
    /// Empty input is a valid empty document. A format never handed out, or
    /// one whose language does not edit, is `UnsupportedFormat`; a source
    /// that does not parse, `ParseError`.
    pub fn create(input: &[u8], format: i32) -> Result<Editor, Status> {
        let Some(entry) = entry_of(format) else { return Err(Status::UnsupportedFormat) };
        if !entry.caps().edit {
            return Err(Status::UnsupportedFormat);
        }
        let lang = runtime_language(&entry);
        let mut inner = editor::Editor::new(lang, entry.dialect);
        inner.init(input).map_err(edit_status)?;
        Ok(Editor { inner, entry })
    }

    /// `fig_editor_source`: the current source, every edit so far applied.
    pub fn source(&self) -> &[u8] {
        self.inner.source()
    }

    /// The dialect the editor reads the document as.
    pub fn entry(&self) -> &Arc<Entry> {
        &self.entry
    }

    /// The engine itself, for what this API does not carry.
    pub fn editor(&self) -> &editor::Editor<'static> {
        &self.inner
    }

    pub fn replace_val(&mut self, path: &[PathSegment<'_>], repl: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.replace_val_at_path(path, repl))
    }

    /// Rename by the key's SYNTAX.
    pub fn replace_key(&mut self, path: &[PathSegment<'_>], repl: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.replace_key_at_path(path, repl))
    }

    /// Rename by the key's NAME, spelled as the format spells a key.
    pub fn replace_named_key(&mut self, path: &[PathSegment<'_>], name: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.replace_named_key(path, name))
    }

    /// Upsert: replace the value at `path`, or insert it when only the
    /// trailing key is absent.
    pub fn set(&mut self, path: &[PathSegment<'_>], val: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.set(path, val))
    }

    /// `replace_val` for a string the caller rendered: `repl` spells the
    /// string `s`, and a string it replaces keeps its quote style.
    pub fn replace_string(&mut self, path: &[PathSegment<'_>], repl: &[u8], s: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.replace_string_at_path(path, repl, s))
    }

    /// `set` for a string the caller rendered.
    pub fn set_string(&mut self, path: &[PathSegment<'_>], val: &[u8], s: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.set_string(path, val, s))
    }

    // ── comments ────────────────────────────────────────────────────────────

    pub fn add_leading_comment(&mut self, path: &[PathSegment<'_>], text: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.add_leading_comment(path, text))
    }

    pub fn set_trailing_comment(&mut self, path: &[PathSegment<'_>], text: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.set_trailing_comment(path, text))
    }

    pub fn delete_leading_comments(&mut self, path: &[PathSegment<'_>]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.delete_leading_comments(path))
    }

    pub fn delete_trailing_comment(&mut self, path: &[PathSegment<'_>]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.delete_trailing_comment(path))
    }

    /// The own-line comment block above the node at `path`, markers and
    /// indentation stripped; `NotFound` when there is none.
    pub fn get_leading_comment(&self, path: &[PathSegment<'_>]) -> Result<Vec<u8>, Status> {
        check_path(path)?;
        comment_out(self.inner.get_leading_comment(path))
    }

    /// The same-line trailing comment on the value at `path`; `NotFound`
    /// when there is none.
    pub fn get_trailing_comment(&self, path: &[PathSegment<'_>]) -> Result<Vec<u8>, Status> {
        check_path(path)?;
        comment_out(self.inner.get_trailing_comment(path))
    }

    pub fn add_dangling_comment(&mut self, path: &[PathSegment<'_>], text: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.add_dangling_comment(path, text))
    }

    pub fn delete_dangling_comments(&mut self, path: &[PathSegment<'_>]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.delete_dangling_comments(path))
    }

    /// The dangling run at the end of the container at `path`'s body;
    /// `NotFound` when there is none.
    pub fn get_dangling_comment(&self, path: &[PathSegment<'_>]) -> Result<Vec<u8>, Status> {
        check_path(path)?;
        comment_out(self.inner.get_dangling_comment(path))
    }

    pub fn comment_out(&mut self, path: &[PathSegment<'_>]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.comment_out(path))
    }

    pub fn uncomment_leading(&mut self, path: &[PathSegment<'_>], first_line: usize, line_count: usize) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.uncomment_leading(path, first_line, line_count))
    }

    pub fn uncomment_dangling(&mut self, path: &[PathSegment<'_>], first_line: usize, line_count: usize) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.uncomment_dangling(path, first_line, line_count))
    }

    // ── keys and items ──────────────────────────────────────────────────────

    /// Insert by the key's SYNTAX.
    pub fn insert_key(&mut self, path: &[PathSegment<'_>], key: &[u8], val: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.insert_key(path, key, val))
    }

    /// Insert by the key's NAME, spelled as the format spells a key.
    pub fn insert_named_key(&mut self, path: &[PathSegment<'_>], name: &[u8], val: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.insert_named_key(path, name, val))
    }

    pub fn delete_key(&mut self, path: &[PathSegment<'_>]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.delete_key(path))
    }

    pub fn append_seq(&mut self, path: &[PathSegment<'_>], val: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.append_to_seq(path, val))
    }

    pub fn prepend_seq(&mut self, path: &[PathSegment<'_>], val: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.prepend_to_seq(path, val))
    }

    pub fn insert_seq_item(&mut self, path: &[PathSegment<'_>], index: usize, val: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.insert_item(path, index, val))
    }

    pub fn remove_seq_item(&mut self, path: &[PathSegment<'_>], index: usize) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.remove_seq_item(path, index))
    }

    pub fn move_key(&mut self, src: &[PathSegment<'_>], dest: &[PathSegment<'_>]) -> Result<(), Status> {
        check_path(src)?;
        check_path(dest)?;
        done(self.inner.move_key(src, dest))
    }

    pub fn reorder_keys(&mut self, path: &[PathSegment<'_>], keys: &[&[u8]]) -> Result<(), Status> {
        check_path(path)?;
        check_keys(keys)?;
        done(self.inner.reorder_keys(path, keys))
    }

    pub fn move_item(&mut self, path: &[PathSegment<'_>], from: usize, to: usize) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.move_item(path, from, to))
    }

    pub fn reorder_items(&mut self, path: &[PathSegment<'_>], indices: &[usize]) -> Result<(), Status> {
        check_path(path)?;
        done(self.inner.reorder_items(path, indices))
    }

    pub fn set_sequence(&mut self, path: &[PathSegment<'_>], items: &[&[u8]]) -> Result<(), Status> {
        check_path(path)?;
        check_keys(items)?;
        done(self.inner.set_sequence(path, items))
    }

    // ── whole containers ────────────────────────────────────────────────────
    //
    // Only the section formats have them; a format that lacks the op answers
    // `UnsupportedFormat`, the same answer `create` gives a format with no
    // editor at all.

    fn container_op(&mut self, op: &str, f: impl FnOnce(&mut editor::Editor<'static>) -> crate::Result<()>) -> Result<(), Status> {
        if !self.inner.has_container_op(op) || !self.inner.dialect_has_container_op(op) {
            return Err(Status::UnsupportedFormat);
        }
        done(f(&mut self.inner))
    }

    /// Delete the whole container at `path`, every scattered region of it.
    pub fn delete_container(&mut self, path: &[PathSegment<'_>]) -> Result<(), Status> {
        check_path(path)?;
        self.container_op("deleteContainer", |e| e.delete_container(path))
    }

    /// Create a container at `path` whose entries are `body`.
    pub fn insert_container(&mut self, path: &[PathSegment<'_>], body: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        self.container_op("insertContainer", |e| e.insert_container(path, body))
    }

    /// Rename the container at `path` to `leaf`, on every line naming it.
    pub fn rename_container(&mut self, path: &[PathSegment<'_>], leaf: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        self.container_op("renameContainer", |e| e.rename_container(path, leaf))
    }

    /// Move the container at `src` before the one at `dest`; `None` moves it
    /// to the end of the document (distinct from an empty path, the root).
    pub fn move_container(&mut self, src: &[PathSegment<'_>], dest: Option<&[PathSegment<'_>]>) -> Result<(), Status> {
        check_path(src)?;
        if let Some(d) = dest {
            check_path(d)?;
        }
        self.container_op("moveContainer", |e| e.move_container(src, dest))
    }

    /// Put the top-level containers named in `order` first, in that order.
    pub fn reorder_containers(&mut self, order: &[&[u8]]) -> Result<(), Status> {
        check_keys(order)?;
        self.container_op("reorderContainers", |e| e.reorder_containers(order))
    }

    /// Append an element with body `body` to the container sequence at
    /// `path` (TOML's `[[header]]`).
    pub fn append_container_to_seq(&mut self, path: &[PathSegment<'_>], body: &[u8]) -> Result<(), Status> {
        check_path(path)?;
        self.container_op("appendContainerToSeq", |e| e.append_container_to_seq(path, body))
    }
}

// ============================================================================
// THE REGISTERED LANGUAGE, AS THE EDITOR SEES IT
// ============================================================================

/// Zig's `Runtime.Language` for one entry: the language the C ABI's editor
/// is instantiated over. Every format reaches the C ABI through the
/// registry, so the editor sees what the registry's thunks make of the
/// language: a parse failure is `RuntimeParseFailed` (whatever the parser
/// named it, so it maps to `ParseError` and never to a refusal status), a
/// renderer's refusal is `RendererRefused`, and the two section gates answer
/// "ask the entry" (`is_runtime`).
pub struct RuntimeLanguage {
    entry: Arc<Entry>,
}

/// One `RuntimeLanguage` per registry entry, for the life of the process,
/// as the entry itself lives.
fn runtime_language(entry: &Arc<Entry>) -> &'static RuntimeLanguage {
    static CACHE: OnceLock<Mutex<HashMap<usize, &'static RuntimeLanguage>>> = OnceLock::new();
    let key = Arc::as_ptr(entry) as usize;
    let mut cache = CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|p| p.into_inner());
    cache.entry(key).or_insert_with(|| Box::leak(Box::new(RuntimeLanguage { entry: entry.clone() })))
}

impl RuntimeLanguage {
    fn lang(&self) -> &'static dyn Language {
        self.entry.lang()
    }
}

impl Language for RuntimeLanguage {
    fn name(&self) -> &str {
        self.lang().name()
    }

    fn extensions(&self) -> &[&'static str] {
        self.lang().extensions()
    }

    fn caps(&self) -> Caps {
        self.entry.caps()
    }

    fn dialects(&self) -> &[DialectInfo] {
        self.lang().dialects()
    }

    fn default_dialect(&self) -> Dialect {
        self.lang().default_dialect()
    }

    fn type_count(&self) -> usize {
        self.lang().type_count()
    }

    fn samples(&self) -> &[&'static [u8]] {
        self.lang().samples()
    }

    fn syntax(&self, dialect: Dialect) -> Syntax {
        if dialect == self.entry.dialect { self.entry.syntax_or_default() } else { self.lang().syntax(dialect) }
    }

    fn is_runtime(&self) -> bool {
        true
    }

    fn parse_table(&self, input: &[u8], dialect: Dialect, out: &mut Table) -> crate::Result<()> {
        self.lang().parse_table(input, dialect, out).map_err(|_| err!(RuntimeParseFailed))
    }

    fn parse(&self, input: &[u8], dialect: Dialect) -> crate::Result<KitDocument> {
        self.parse_with_report(input, dialect, &mut ParseReport::default())
    }

    fn parse_with_report(&self, input: &[u8], dialect: Dialect, report: &mut ParseReport) -> crate::Result<KitDocument> {
        if dialect != self.entry.dialect {
            return self.lang().parse_with_report(input, dialect, report).map_err(|e| {
                if e.is("OutOfMemory") { e } else { err!(RuntimeParseFailed) }
            });
        }
        let mut parser = registry::Parser::new();
        let parsed = parser.parse(input, &self.entry);
        report.errors.extend(parser.errors);
        report.warnings.extend(parser.warnings);
        parsed
    }

    fn split_stream(&self, input: &[u8], dialect: Dialect) -> Option<crate::Result<Vec<StreamDocument>>> {
        self.lang().split_stream(input, dialect)
    }

    fn has_split_stream(&self) -> bool {
        self.lang().has_split_stream()
    }

    fn print(&self, dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> crate::Result<()> {
        self.lang().print(dialect, ast, options, out)
    }

    fn print_node(&self, dialect: Dialect, ast: &AST, node: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> crate::Result<()> {
        self.lang().print_node(dialect, ast, node, options, out)
    }

    fn render_value(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<crate::Result<()>> {
        self.render(Renderer::Value, dialect, out, r)
    }

    fn render_entry(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<crate::Result<()>> {
        self.render(Renderer::Entry, dialect, out, r)
    }

    fn render_item(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<crate::Result<()>> {
        self.render(Renderer::Item, dialect, out, r)
    }

    fn render_tail(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<crate::Result<()>> {
        self.render(Renderer::Tail, dialect, out, r)
    }

    fn render_key(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<crate::Result<()>> {
        self.render(Renderer::Key, dialect, out, r)
    }

    fn render_key_name(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<crate::Result<()>> {
        self.render(Renderer::KeyName, dialect, out, r)
    }

    fn render_string(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<crate::Result<()>> {
        self.render(Renderer::String, dialect, out, r)
    }

    /// The vtable's renderer slot: the fragment, or the refusal as
    /// `RendererRefused` with the language's words in `last_refusal`.
    fn render(&self, which: Renderer, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<crate::Result<()>> {
        let mut buf = Vec::new();
        match self.lang().render(which, dialect, &mut buf, r)? {
            Ok(()) => {
                out.extend_from_slice(&buf);
                Some(Ok(()))
            }
            Err(e) => {
                let text = e.name();
                registry::refuse(if text.is_empty() { "the renderer declined" } else { text });
                Some(Err(err!(RendererRefused)))
            }
        }
    }

    fn has_renderer(&self, dialect: Dialect, which: Renderer) -> bool {
        self.lang().has_renderer(dialect, which)
    }
}

// ============================================================================
// TESTS (src/c_api.zig's that exercise logic, not the ABI's layout)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{Cause, Code};

    /// The handle of the registered dialect `name`.
    fn fmt(name: &str) -> i32 {
        let f = format_by_name(name);
        assert!(f >= 0, "{name} is not registered");
        f
    }

    fn k(s: &str) -> PathSegment<'_> {
        PathSegment::Key(s.as_bytes())
    }

    fn editor(src: &str, format: &str) -> Editor {
        Editor::create(src.as_bytes(), fmt(format)).unwrap()
    }

    fn source(ed: &Editor) -> &str {
        std::str::from_utf8(ed.source()).unwrap()
    }

    fn doc(src: &str, format: &str) -> Document {
        Document::parse(src.as_bytes(), fmt(format)).unwrap()
    }

    /// The value of the root mapping's first entry.
    fn first_value(d: &Document) -> NodeId {
        d.keyvalue_value(d.first_child(d.root()))
    }

    #[test]
    fn traversal_over_a_parsed_mapping() {
        let d = doc("title: Hello\ncount: 42\ntags:\n- a\n- b\n", "yaml");
        let root = d.root();
        assert_ne!(NODE_NONE, root);
        assert_eq!(FigNodeKind::Mapping, d.node_kind(root));
        assert_eq!(3, d.child_count(root));
        let first = d.first_child(root);
        assert_eq!(FigNodeKind::KeyValue, d.node_kind(first));
        assert_eq!(Some(&b"title"[..]), d.node_string(d.keyvalue_key(first)));
        assert_eq!(Some(&b"Hello"[..]), d.node_string(d.keyvalue_value(first)));
        let second = d.next_sibling(first);
        let count = d.keyvalue_value(second);
        assert_eq!(FigNodeKind::Int, d.node_kind(count));
        assert_eq!(Some(&b"42"[..]), d.node_number(count));
        let tags = d.keyvalue_value(d.next_sibling(second));
        assert_eq!(FigNodeKind::Sequence, d.node_kind(tags));
        assert_eq!(2, d.child_count(tags));
        // Out of range reads as nothing.
        assert_eq!(FigNodeKind::Invalid, d.node_kind(9999));
        assert_eq!(NODE_NONE, d.first_child(9999));
    }

    #[test]
    fn a_spec_version_is_a_dialect_of_its_own_found_by_name() {
        for (name, kind) in [("yaml", FigNodeKind::String), ("yaml-1.2.2", FigNodeKind::String), ("yaml-1.1", FigNodeKind::Bool)] {
            let d = doc("a: yes\n", name);
            assert_eq!(kind, d.node_kind(first_value(&d)), "{name}");
        }
        let trailing = b"t = { a = 1, }\n";
        for (name, want) in [("toml", Status::Ok), ("toml-1.1", Status::Ok), ("toml-1.0", Status::ParseError), ("toml-1.0.0", Status::ParseError)] {
            let got = Document::parse(trailing, fmt(name)).map(|_| Status::Ok).unwrap_or_else(|s| s);
            assert_eq!(want, got, "{name}");
        }
        assert_eq!(-1, format_by_name("toml-2.0"));
        assert_eq!(-1, format_by_name("yaml-1.3"));
    }

    #[test]
    fn document_diagnose_reports_a_dropped_null_for_toml() {
        let mut d = doc("a: null\nb: 1\n", "yaml");
        assert_eq!(Ok(1), d.diagnose(fmt("toml"), None));
        let w = d.warning(0).unwrap();
        assert_eq!(Code::ValueDropped, w.code);
        assert_eq!(Cause::FormatLimitation, w.cause);
        assert_eq!(b"a", &w.path[..]);
        assert_eq!(Status::InvalidArgument, d.warning(1).unwrap_err());
        // Lossless preserves the null.
        let lossless = FigSerializeOptions { lossless: true, ..Default::default() };
        assert_eq!(Ok(0), d.diagnose(fmt("toml"), Some(&lossless)));
        assert_eq!(Status::InvalidArgument, d.warning(0).unwrap_err());
        assert_eq!(Ok(0), d.diagnose(fmt("json"), None));
    }

    #[test]
    fn value_diagnose_reports_a_degraded_datetime() {
        let mut v = Value::create();
        let dt = v.extended(ExtKind::OffsetDatetime.ordinal() as i32, b"1979-05-27T07:32:00Z").unwrap();
        assert_eq!(Ok(1), v.diagnose(dt, fmt("json"), None));
        let w = v.warning(0).unwrap();
        assert_eq!(Code::TypeDegraded, w.code);
        assert_eq!("string", w.note);
        assert_eq!(Ok(0), v.diagnose(dt, fmt("toml"), None));
        // An unknown kind, root or format.
        assert_eq!(Status::InvalidArgument, v.extended(42, b"x").unwrap_err());
        assert_eq!(Status::InvalidArgument, v.diagnose(99, fmt("json"), None).unwrap_err());
        assert_eq!(Status::UnsupportedFormat, v.diagnose(dt, 0xBEEF, None).unwrap_err());
    }

    #[test]
    fn parse_ex_reports_a_parse_failure() {
        let bad = b"{ \"a\":";
        assert_eq!(Status::ParseError, Document::parse(bad, fmt("json")).unwrap_err());
        let e = Document::parse_ex(bad, fmt("json")).unwrap_err();
        assert_eq!(Status::ParseError, e.code);
        assert!(!e.message.is_empty());
        assert!(e.byte_offset <= bad.len());
        let e = Document::parse_ex(bad, 0xBEEF).unwrap_err();
        assert_eq!(Status::UnsupportedFormat, e.code);
        assert_eq!(b"unsupported or unknown format", &e.message[..]);
        assert!(Document::parse_ex(b"{\"a\":1}", fmt("json")).is_ok());
    }

    #[test]
    fn parse_reads_toml_zon_and_json5_and_rejects_json5_under_strict_json() {
        let d = doc("name = \"fig\"\ncount = 42\n", "toml");
        assert_eq!(FigNodeKind::Mapping, d.node_kind(d.root()));
        assert_eq!(2, d.child_count(d.root()));
        let d = doc(".{ .name = \"fig\", .count = 42 }", "zon");
        assert_eq!(FigNodeKind::Mapping, d.node_kind(d.root()));
        let src = "{\n  // c\n  host: 'localhost',\n  port: 8080,\n}\n";
        let d = doc(src, "json5");
        assert_eq!(2, d.child_count(d.root()));
        assert_eq!(Status::ParseError, Document::parse(src.as_bytes(), fmt("json")).unwrap_err());
    }

    #[test]
    fn node_extended_recovers_datetime_and_char_literal_scalars() {
        let d = doc("d = 2026-06-18\n", "toml");
        let v = first_value(&d);
        assert_eq!(FigNodeKind::Extended, d.node_kind(v));
        assert_eq!(Some((ExtKind::LocalDate, &b"2026-06-18"[..])), d.node_extended(v));
        assert_eq!(Some(&b"2026-06-18"[..]), d.node_string(v));
        let d = doc(".{ .c = 'a' }", "zon");
        let v = first_value(&d);
        assert_eq!(Some((ExtKind::CharLiteral, &b"97"[..])), d.node_extended(v));
        assert_eq!(Some(&b"97"[..]), d.node_number(v));
        assert_eq!(None, d.node_string(v));
        let d = doc("s = \"hi\"\n", "toml");
        assert_eq!(None, d.node_extended(first_value(&d)));
    }

    #[test]
    fn parse_judges_empty_input_per_format() {
        assert_eq!(Status::ParseError, Document::parse(b"", fmt("json")).unwrap_err());
        let d = doc("", "yaml");
        assert_eq!(FigNodeKind::Null, d.node_kind(d.root()));
        let d = doc("", "toml");
        assert_eq!(FigNodeKind::Mapping, d.node_kind(d.root()));
        assert_eq!(0, d.child_count(d.root()));
    }

    #[test]
    fn editor_insert_round_trips_and_takes_empty_input() {
        let mut ed = editor("a: 1\nb: 2\n", "yaml");
        ed.insert_key(&[], b"c", b"3").unwrap();
        assert_eq!("a: 1\nb: 2\nc: 3\n", source(&ed));
        let mut ed = editor("", "yaml");
        ed.insert_key(&[], b"k", b"v").unwrap();
        assert_eq!("k: v\n", source(&ed));
    }

    #[test]
    fn toml_editor_insert_replace_delete_round_trip() {
        let mut ed = editor("[server]\nhost = \"a\"\nport = 1\n", "toml");
        ed.insert_key(&[k("server")], b"tls", b"true").unwrap();
        ed.replace_val(&[k("server"), k("port")], b"9090").unwrap();
        ed.delete_key(&[k("server"), k("host")]).unwrap();
        assert_eq!("[server]\nport = 9090\ntls = true\n", source(&ed));
    }

    #[test]
    fn toml_editor_adds_a_leading_comment_with_the_hash_marker() {
        let mut ed = editor("a = 1\nb = 2\n", "toml");
        ed.add_leading_comment(&[k("b")], b"note").unwrap();
        assert_eq!("a = 1\n# note\nb = 2\n", source(&ed));
    }

    #[test]
    fn a_duplicate_key_is_invalid_argument_and_rolls_back() {
        let mut ed = editor("a = 1\n", "toml");
        assert_eq!(Status::InvalidArgument, ed.insert_key(&[], b"a", b"2").unwrap_err());
        assert_eq!("a = 1\n", source(&ed));
        let mut ed = editor("a: 1\n", "yaml");
        assert_eq!(Status::InvalidArgument, ed.insert_named_key(&[], b"a", b"2").unwrap_err());
        assert_eq!(Status::InvalidArgument, ed.insert_key(&[], b"a", b"2").unwrap_err());
        assert_eq!("a: 1\n", source(&ed));
    }

    fn name_nums(v: &mut Value) -> NodeId {
        let name = v.string(b"fig").unwrap();
        let n1 = v.int(1).unwrap();
        let n2 = v.int(2).unwrap();
        let nums = v.seq(&[n1, n2]).unwrap();
        let k_name = v.string(b"name").unwrap();
        let k_nums = v.string(b"nums").unwrap();
        v.map(&[FigKeyValue { key: k_name, value: name }, FigKeyValue { key: k_nums, value: nums }]).unwrap()
    }

    #[test]
    fn value_builds_and_serializes_to_multiple_formats() {
        let mut v = Value::create();
        let root = name_nums(&mut v);
        assert_eq!(b"{\n  \"name\": \"fig\",\n  \"nums\": [\n    1,\n    2\n  ]\n}\n", v.serialize(root, fmt("json")).unwrap());
        assert_eq!(b"name: fig\nnums: [1, 2]\n", v.serialize(root, fmt("yaml")).unwrap());
    }

    #[test]
    fn value_maps_an_unrepresentable_value_to_unsupported_format() {
        let mut v = Value::create();
        let null = v.null().unwrap();
        let key = v.string(b"k").unwrap();
        let root = v.map(&[FigKeyValue { key, value: null }]).unwrap();
        assert_eq!(Status::UnsupportedFormat, v.serialize(root, fmt("toml")).unwrap_err());
        assert_eq!(b"{\n  \"k\": null\n}\n", v.serialize(root, fmt("json")).unwrap());
    }

    #[test]
    fn value_spells_a_non_finite_float_in_each_formats_words_and_json_refuses_it() {
        let cases = [
            (".inf", "v = inf\n", "v: .inf\n", "{\n  v: Infinity\n}\n"),
            ("-.inf", "v = -inf\n", "v: -.inf\n", "{\n  v: -Infinity\n}\n"),
            (".nan", "v = nan\n", "v: .nan\n", "{\n  v: NaN\n}\n"),
            ("inf", "v = inf\n", "v: .inf\n", "{\n  v: Infinity\n}\n"),
        ];
        for (raw, toml, yaml, json5) in cases {
            let mut v = Value::create();
            let n = v.number(raw.as_bytes(), true).unwrap();
            let key = v.string(b"v").unwrap();
            let root = v.map(&[FigKeyValue { key, value: n }]).unwrap();
            assert_eq!(toml.as_bytes(), v.serialize(root, fmt("toml")).unwrap());
            assert_eq!(yaml.as_bytes(), v.serialize(root, fmt("yaml")).unwrap());
            assert_eq!(json5.as_bytes(), v.serialize(root, fmt("json5")).unwrap());
            assert_eq!(Status::UnsupportedFormat, v.serialize(root, fmt("json")).unwrap_err());
            assert_eq!(Status::UnsupportedFormat, v.serialize(root, fmt("jsonc")).unwrap_err());
        }
    }

    #[test]
    fn document_converts_a_non_finite_float_into_the_targets_spelling() {
        let cases = [
            ("a: .inf\nb: -.inf\nc: .nan\n", "yaml", "toml", Some("a = inf\nb = -inf\nc = nan\n")),
            ("a = inf\nb = -inf\nc = nan\n", "toml", "yaml", Some("a: .inf\nb: -.inf\nc: .nan\n")),
            ("a: .inf\n", "yaml", "json", None),
        ];
        for (src, from, to, want) in cases {
            let mut d = doc(src, from);
            match want {
                Some(w) => assert_eq!(w.as_bytes(), d.serialize(fmt(to), None).unwrap()),
                None => assert_eq!(Status::UnsupportedFormat, d.serialize(fmt(to), None).unwrap_err()),
            }
        }
    }

    #[test]
    fn toml_editor_takes_inf_and_nan_as_a_replacement_value() {
        let mut ed = editor("name = \"music\"\nversion = 1\n", "toml");
        for repl in ["inf", "-inf", "nan"] {
            ed.replace_val(&[k("version")], repl.as_bytes()).unwrap();
            assert_eq!(format!("name = \"music\"\nversion = {repl}\n"), source(&ed));
        }
    }

    #[test]
    fn value_serialize_options_compact_and_default() {
        let mut v = Value::create();
        let a = v.int(1).unwrap();
        let b = v.int(2).unwrap();
        let root = v.seq(&[a, b]).unwrap();
        let compact = FigSerializeOptions { pretty: false, ..Default::default() };
        assert_eq!(b"[1,2]\n", v.serialize_opts(root, fmt("json"), Some(&compact)).unwrap());
        // Zero indent and width read as their defaults, not as zero.
        let zeroed = FigSerializeOptions { indent: 0, width: 0, ..Default::default() };
        assert_eq!(b"[\n  1,\n  2\n]\n", v.serialize_opts(root, fmt("json"), Some(&zeroed)).unwrap());
    }

    #[test]
    fn value_serialize_options_carry_toml_width_to_the_inline_or_section_choice() {
        let mut v = Value::create();
        let x = v.int(1).unwrap();
        let y = v.int(2).unwrap();
        let kx = v.string(b"x").unwrap();
        let ky = v.string(b"y").unwrap();
        let point = v.map(&[FigKeyValue { key: kx, value: x }, FigKeyValue { key: ky, value: y }]).unwrap();
        let kp = v.string(b"point").unwrap();
        let root = v.map(&[FigKeyValue { key: kp, value: point }]).unwrap();
        assert_eq!(b"point = { x = 1, y = 2 }\n", v.serialize(root, fmt("toml")).unwrap());
        let narrow = FigSerializeOptions { width: 8, ..Default::default() };
        assert_eq!(b"[point]\nx = 1\ny = 2\n", v.serialize_opts(root, fmt("toml"), Some(&narrow)).unwrap());
    }

    #[test]
    fn value_flow_option_renders_figl_container_fragments_inline() {
        let mut v = Value::create();
        let a = v.string(b"a.md").unwrap();
        let b = v.string(b"b.md").unwrap();
        let root = v.seq(&[a, b]).unwrap();
        let flow = FigSerializeOptions { flow: true, ..Default::default() };
        assert_eq!(b"[a.md, b.md]\n", v.serialize_opts(root, fmt("figl"), Some(&flow)).unwrap());
        assert_eq!(b"* a.md\n* b.md\n", v.serialize(root, fmt("figl")).unwrap());
    }

    #[test]
    fn value_rejects_an_out_of_range_child_id() {
        let mut v = Value::create();
        let one = v.int(1).unwrap();
        assert_eq!(Status::InvalidArgument, v.seq(&[one, 99]).unwrap_err());
        assert_eq!(Status::InvalidArgument, v.map(&[FigKeyValue { key: one, value: 99 }]).unwrap_err());
        assert_eq!(Status::InvalidArgument, v.serialize(99, fmt("json")).unwrap_err());
        assert_eq!(Status::UnsupportedFormat, v.serialize(one, 0xBEEF).unwrap_err());
    }

    #[test]
    fn document_serialize_converts_json_to_yaml_toml_and_figl() {
        let mut d = doc("{\"name\":\"fig\",\"nums\":[1,2]}", "json");
        assert_eq!(b"name: fig\nnums: [1, 2]\n", d.serialize(fmt("yaml"), None).unwrap());
        assert_eq!(b"name = \"fig\"\nnums = [1, 2]\n", d.serialize(fmt("toml"), None).unwrap());
        assert_eq!(b"name = fig\nnums = [1, 2]\n", d.serialize(fmt("figl"), None).unwrap());
        let mut d = doc("title = Hello\ncount = 42\n", "figl");
        assert_eq!(b"{\n  \"title\": \"Hello\",\n  \"count\": 42\n}\n", d.serialize(fmt("json"), None).unwrap());
    }

    #[test]
    fn editor_edits_figl_and_zon() {
        let mut ed = editor("title = old\nport = 8080\n", "figl");
        ed.replace_val(&[k("port")], b"9090").unwrap();
        assert_eq!("title = old\nport = 9090\n", source(&ed));
        let mut ed = editor(".{ .title = \"old\", .port = 8080 }", "zon");
        ed.replace_val(&[k("port")], b"9090").unwrap();
        assert_eq!(".{ .title = \"old\", .port = 9090 }", source(&ed));
    }

    #[test]
    fn insert_named_key_spells_the_name_as_the_format_does() {
        let mut ed = editor(".{ .a = 1 }", "zon");
        ed.insert_named_key(&[], b"new", b"true").unwrap();
        ed.insert_named_key(&[], b"has space", b"true").unwrap();
        assert_eq!(".{ .a = 1, .new = true, .@\"has space\" = true }", source(&ed));
        let mut ed = editor("{\"a\": 1}", "json");
        ed.insert_named_key(&[], b"k\"q", b"2").unwrap();
        assert_eq!("{\"a\": 1, \"k\\\"q\": 2}", source(&ed));
    }

    /// A block mapping whose first key is a flow collection sniffs as flow,
    /// and its last member ends past its span's last byte: the Zig's wasm
    /// reads that as a multi-line flow mapping, writes the entry on a line
    /// of its own, and refuses what reads back (`EditReadsBackWrong`), as
    /// fig-conformance's `explicit-keys.yaml` root inserts record.
    #[test]
    fn an_insert_under_an_explicit_flow_key_reads_back_wrong() {
        let src = "? [a, b]\n: c\n? a: b\n: d\n: empty key\n";
        let mut ed = editor(src, "yaml");
        assert_eq!(Status::UnsupportedOperation, ed.insert_named_key(&[], b"added_int", b"42").unwrap_err());
        assert_eq!(src, source(&ed));
    }

    #[test]
    fn replace_named_key_spells_the_new_name_as_the_format_does() {
        let mut ed = editor(".{ .a = 1, .b = 2 }", "zon");
        ed.replace_named_key(&[k("a")], b"k").unwrap();
        ed.replace_named_key(&[k("b")], b"has space").unwrap();
        assert_eq!(".{ .k = 1, .@\"has space\" = 2 }", source(&ed));
        let mut ed = editor("<dict><key>a</key><string>x</string></dict>", "plist");
        ed.replace_named_key(&[k("a")], b"b&c").unwrap();
        assert_eq!("<dict><key>b&amp;c</key><string>x</string></dict>", source(&ed));
    }

    #[test]
    fn replace_key_refuses_a_path_with_no_key_and_a_name_already_held() {
        let src = "{\"l\": [1, 2], \"a\": 1, \"b\": 2}";
        let mut ed = editor(src, "json");
        assert_eq!(Status::InvalidArgument, ed.replace_key(&[k("l"), PathSegment::Index(0)], b"\"z\"").unwrap_err());
        assert_eq!(Status::InvalidArgument, ed.replace_key(&[], b"\"z\"").unwrap_err());
        assert_eq!(Status::InvalidArgument, ed.replace_named_key(&[k("a")], b"b").unwrap_err());
        assert_eq!(Status::InvalidArgument, ed.replace_key(&[k("a")], b"\"b\"").unwrap_err());
        assert_eq!(src, source(&ed));
    }

    #[test]
    fn whole_container_ops_reach_a_toml_table_the_key_ops_cannot() {
        let src = "[a]\nx = 1\n[b]\ny = 2\n";
        let mut ed = editor(src, "toml");
        assert_eq!(Status::InvalidArgument, ed.delete_key(&[k("a")]).unwrap_err());
        ed.delete_container(&[k("a")]).unwrap();
        assert_eq!("[b]\ny = 2\n", source(&ed));

        let mut ed = editor(src, "toml");
        ed.rename_container(&[k("a")], b"q").unwrap();
        assert_eq!("[q]\nx = 1\n[b]\ny = 2\n", source(&ed));

        let mut ed = editor(src, "toml");
        ed.insert_container(&[k("c")], b"z = 3\n").unwrap();
        assert_eq!("[a]\nx = 1\n[b]\ny = 2\n\n[c]\nz = 3\n", source(&ed));

        let mut ed = editor(src, "toml");
        assert_eq!(Status::InvalidArgument, ed.move_key(&[k("a")], &[k("b")]).unwrap_err());
        ed.move_container(&[k("a")], None).unwrap();
        assert_eq!("[b]\ny = 2\n\n[a]\nx = 1\n", source(&ed));

        let mut ed = editor(src, "toml");
        assert_eq!(Status::InvalidArgument, ed.reorder_keys(&[], &[b"b", b"a"]).unwrap_err());
        ed.reorder_containers(&[b"b", b"a"]).unwrap();
        assert_eq!("[b]\ny = 2\n[a]\nx = 1\n", source(&ed));
    }

    #[test]
    fn append_container_to_seq_appends_a_toml_array_of_tables_element() {
        let mut ed = editor("[[bin]]\nname = \"a\"\n", "toml");
        ed.append_container_to_seq(&[k("bin")], b"name = \"b\"\n").unwrap();
        assert_eq!("[[bin]]\nname = \"a\"\n\n[[bin]]\nname = \"b\"\n", source(&ed));
    }

    #[test]
    fn whole_container_ops_answer_unsupported_format_where_the_format_has_none() {
        let mut ed = editor("a:\n  x: 1\nb:\n  y: 2\n", "yaml");
        assert_eq!(Status::UnsupportedFormat, ed.delete_container(&[k("a")]).unwrap_err());
        ed.delete_key(&[k("a")]).unwrap();
        assert_eq!("b:\n  y: 2\n", source(&ed));

        let mut ed = editor("[a]\nx = 1\n[b]\ny = 2\n[a]\nz = 3\n", "ini");
        assert_eq!(Status::UnsupportedFormat, ed.insert_container(&[k("a")], b"").unwrap_err());
        ed.rename_container(&[k("a")], b"q").unwrap();
        assert_eq!("[q]\nx = 1\n[b]\ny = 2\n[q]\nz = 3\n", source(&ed));
        ed.delete_container(&[k("q")]).unwrap();
        assert_eq!("[b]\ny = 2\n", source(&ed));
    }

    #[test]
    fn document_serialize_materializes_the_yaml_reference_layer_when_leaving_yaml() {
        let mut d = doc("a: &x 1\nb: *x\n", "yaml");
        assert_eq!(b"{\n  \"a\": 1,\n  \"b\": 1\n}\n", d.serialize(fmt("json"), None).unwrap());
        let yaml = d.serialize(fmt("yaml"), None).unwrap();
        assert!(String::from_utf8_lossy(yaml).contains("*x"));
    }

    #[test]
    fn resolve_aliases_reads_an_alias_and_a_merge_key_as_values() {
        let d = doc("base: &b {x: 1, y: 2}\nname: &n fig\nd:\n  <<: *b\n  y: 3\nalias: *n\n", "yaml");
        let mut resolved = d.resolve_aliases().unwrap().unwrap();
        drop(d);
        {
            let ast = &resolved.document().ast;
            let val = |p: &[PathSegment<'_>]| ast.get_val_by_path(p).map(|n| n.kind);
            assert_eq!(Ok(NodeKind::String(b"fig")), val(&[k("alias")]));
            assert_eq!(Some(&b"1"[..]), val(&[k("d"), k("x")]).unwrap().as_number().map(|n| n.raw));
            assert_eq!(Some(&b"3"[..]), val(&[k("d"), k("y")]).unwrap().as_number().map(|n| n.raw));
            assert_eq!(err!(NotFound), val(&[k("d"), k("<<")]).unwrap_err());
            let d_id = ast.get_val_by_path(&[k("d")]).unwrap().id;
            assert_eq!(FigNodeKind::Mapping, resolved.node_kind(d_id));
            for i in 0..ast.node_count() {
                assert!(!matches!(ast.kind_of(i as NodeId), NodeKind::Alias(_)));
            }
        }
        let yaml = resolved.serialize(fmt("yaml"), None).unwrap();
        assert!(!yaml.iter().any(|&b| b == b'*' || b == b'&'));
    }

    #[test]
    fn resolve_aliases_answers_none_with_nothing_to_resolve() {
        assert!(doc("a: &x 1\nb: 2\n", "yaml").resolve_aliases().unwrap().is_none());
        assert!(doc("{\"<<\": {\"a\": 1}}", "json").resolve_aliases().unwrap().is_none());
    }

    #[test]
    fn resolve_aliases_refuses_a_cyclic_alias_as_a_parse_error_at_the_alias() {
        let src = "ok: 1\nloop: &a [1, *a]\n";
        let e = doc(src, "yaml").resolve_aliases().unwrap_err();
        assert_eq!(Status::ParseError, e.code);
        assert_eq!(&b"alias `*a` refers to a node that contains it, so it has no value"[..], &e.message[..]);
        assert_eq!(src.find("*a").unwrap(), e.byte_offset);
        assert_eq!(2, e.line);
        assert_eq!(14, e.column);
        let e = doc("m: &m {a: 1, <<: *m}\n", "yaml").resolve_aliases().unwrap_err();
        assert_eq!(Status::ParseError, e.code);
    }

    #[test]
    fn document_serialize_honors_the_lossless_option_for_toml_null() {
        let mut d = doc("{\"k\":null}", "json");
        assert_eq!(Status::UnsupportedFormat, d.serialize(fmt("toml"), None).unwrap_err());
        let lossless = FigSerializeOptions { lossless: true, ..Default::default() };
        let out = d.serialize(fmt("toml"), Some(&lossless)).unwrap();
        assert!(String::from_utf8_lossy(out).contains("$fig"));
    }

    #[test]
    fn document_serialize_preserves_comments_across_formats() {
        let mut d = doc("{\n  // hello\n  a: 1,\n}\n", "json5");
        let out = String::from_utf8_lossy(d.serialize(fmt("yaml"), None).unwrap()).into_owned();
        assert!(out.contains("# hello") && out.contains("a: 1"));
        let strip = FigSerializeOptions { strip_comments: true, ..Default::default() };
        assert!(!String::from_utf8_lossy(d.serialize(fmt("yaml"), Some(&strip)).unwrap()).contains("hello"));
    }

    #[test]
    fn version_is_the_crate_version() {
        let v = version();
        assert_eq!(version_string(), format!("{}.{}.{}", v >> 16, (v >> 8) & 0xff, v & 0xff));
    }

    #[test]
    fn format_capabilities_report_the_per_format_matrix() {
        let rws = CAP_READ | CAP_EDIT | CAP_SERIALIZE;
        for f in ["json", "jsonc", "json5", "toml", "zon", "figl"] {
            assert_eq!(rws, format_capabilities(fmt(f)), "{f}");
        }
        assert_eq!(rws | CAP_REFERENCES, format_capabilities(fmt("yaml")));
        for bad in [0, 9999, -1] {
            assert_eq!(0, format_capabilities(bad));
        }
        assert_eq!(Some("json".to_string()), format_name(fmt("json")));
        assert_eq!(None, format_name(0));
    }

    #[test]
    fn format_capabilities_agree_with_what_each_entry_point_does() {
        let mut v = Value::create();
        let one = v.int(1).unwrap();
        let a = v.string(b"a").unwrap();
        let root = v.map(&[FigKeyValue { key: a, value: one }]).unwrap();
        let cases = [
            ("json", "{\"a\":1}"),
            ("jsonc", "{\"a\":1}"),
            ("json5", "{a:1}"),
            ("yaml", "a: 1\n"),
            ("toml", "a = 1\n"),
            ("zon", ".{ .a = 1 }"),
            ("figl", "a = 1\n"),
        ];
        for (name, sample) in cases {
            let f = fmt(name);
            let caps = format_capabilities(f);
            assert_eq!(caps & CAP_READ != 0, Document::parse(sample.as_bytes(), f).is_ok());
            let edit = Editor::create(sample.as_bytes(), f).map(|_| Status::Ok).unwrap_or_else(|s| s);
            assert_eq!(caps & CAP_EDIT != 0, edit != Status::UnsupportedFormat);
            let ser = v.serialize(root, f).map(|_| Status::Ok).unwrap_or_else(|s| s);
            assert_eq!(caps & CAP_SERIALIZE != 0, ser != Status::UnsupportedFormat);
        }
    }

    #[test]
    fn editor_comment_ops_add_set_and_delete() {
        let mut ed = editor("a: 1\nb: 2\n", "yaml");
        ed.add_leading_comment(&[k("b")], b"why").unwrap();
        ed.set_trailing_comment(&[k("b")], b"two").unwrap();
        assert_eq!("a: 1\n# why\nb: 2 # two\n", source(&ed));
        ed.delete_trailing_comment(&[k("b")]).unwrap();
        ed.delete_leading_comments(&[k("b")]).unwrap();
        assert_eq!("a: 1\nb: 2\n", source(&ed));
    }

    #[test]
    fn set_sequence_reconciles_a_list_preserving_survivors_comments() {
        let mut ed = editor("tags:\n- a # first\n- b # second\n- c # third\n", "yaml");
        ed.set_sequence(&[k("tags")], &[b"c", b"a", b"d"]).unwrap();
        assert_eq!("tags:\n- c # third\n- a # first\n- d\n", source(&ed));
        assert_eq!(Status::InvalidArgument, ed.set_sequence(&[k("tags")], &[]).unwrap_err());
    }

    #[test]
    fn comment_ops_reject_strict_json_with_unsupported_format() {
        let mut ed = editor("{\"a\":1}", "json");
        assert_eq!(Status::UnsupportedFormat, ed.add_leading_comment(&[k("a")], b"x").unwrap_err());
        assert_eq!(Status::UnsupportedFormat, ed.delete_trailing_comment(&[k("a")]).unwrap_err());
        assert_eq!(Status::UnsupportedFormat, ed.get_leading_comment(&[k("a")]).unwrap_err());
        assert_eq!(Status::UnsupportedFormat, ed.get_trailing_comment(&[k("a")]).unwrap_err());
    }

    #[test]
    fn comment_ops_on_a_one_line_flow_item_answer_as_if_it_had_none() {
        let src = "# above members\nmembers = [\"a\", \"b\"] # note\n";
        let mut ed = editor(src, "toml");
        let item0 = [k("members"), PathSegment::Index(0)];
        assert_eq!(Status::InvalidArgument, ed.add_leading_comment(&item0, b"mine").unwrap_err());
        assert_eq!(Status::InvalidArgument, ed.set_trailing_comment(&item0, b"mine").unwrap_err());
        assert_eq!(Status::NotFound, ed.get_leading_comment(&item0).unwrap_err());
        assert_eq!(Status::NotFound, ed.get_trailing_comment(&item0).unwrap_err());
        ed.delete_leading_comments(&item0).unwrap();
        ed.delete_trailing_comment(&item0).unwrap();
        assert_eq!(src, source(&ed));
        assert_eq!(b"above members", &ed.get_leading_comment(&[k("members")]).unwrap()[..]);
        assert_eq!(b"note", &ed.get_trailing_comment(&[k("members")]).unwrap()[..]);
    }

    #[test]
    fn comment_reads_distinguish_absent_from_empty() {
        let ed = editor("# why\na: 1 # two\nb: 2 #\nc: 3\n", "yaml");
        assert_eq!(b"why", &ed.get_leading_comment(&[k("a")]).unwrap()[..]);
        assert_eq!(b"two", &ed.get_trailing_comment(&[k("a")]).unwrap()[..]);
        assert_eq!(b"", &ed.get_trailing_comment(&[k("b")]).unwrap()[..]);
        assert_eq!(Status::NotFound, ed.get_leading_comment(&[k("c")]).unwrap_err());
        assert_eq!(Status::NotFound, ed.get_trailing_comment(&[k("c")]).unwrap_err());
    }

    #[test]
    fn editor_create_refuses_an_unknown_format_and_an_unparseable_source() {
        assert_eq!(Status::UnsupportedFormat, Editor::create(b"a = 1\n", 0xBEEF).unwrap_err());
        assert_eq!(Status::ParseError, Editor::create(b"{", fmt("json")).unwrap_err());
    }

    #[test]
    fn a_reparse_failure_is_a_parse_error_whatever_the_parser_called_it() {
        // A replacement that does not parse is the parser's failure, which
        // reaches the caller as `ParseError` through the registry, never as
        // the refusal status a same-named editor error would map to.
        let mut ed = editor("[a]\nx = 1\n", "toml");
        assert_eq!(Status::ParseError, ed.replace_val(&[k("a"), k("x")], b"1\n[a]").unwrap_err());
        assert_eq!("[a]\nx = 1\n", source(&ed));
        let lang = runtime_language(&registry::global().entry_by_name("toml").unwrap());
        assert_eq!(err!(RuntimeParseFailed), lang.parse(b"a = 1\na = 2\n", lang.entry.dialect).unwrap_err());
    }

    #[test]
    fn a_path_or_list_past_the_limits_is_invalid_argument() {
        let mut ed = editor("a = 1\n", "toml");
        let long: Vec<PathSegment<'_>> = (0..=MAX_PATH_LEN).map(|_| k("a")).collect();
        assert_eq!(Status::InvalidArgument, ed.delete_key(&long).unwrap_err());
        let keys: Vec<&[u8]> = (0..=MAX_KEYS_LEN).map(|_| &b"a"[..]).collect();
        assert_eq!(Status::InvalidArgument, ed.reorder_keys(&[], &keys).unwrap_err());
    }

    #[test]
    fn detect_order_and_stream_split() {
        assert_eq!(Ok(fmt("toml")), detect_order(b"a = 1\n", &["json", "toml"]));
        assert_eq!(Err(Status::NotFound), detect_order(b"{", &["json"]));
        let docs = stream_split(b"a: 1\n---\nb: 2\n", fmt("yaml")).unwrap();
        assert_eq!(vec![(0, 5), (5, 14)], docs.iter().map(|d| (d.start, d.end)).collect::<Vec<_>>());
        assert_eq!(Status::UnsupportedOperation, stream_split(b"a = 1\n", fmt("toml")).unwrap_err().code);
        assert_eq!(Status::UnsupportedFormat, stream_split(b"", 0xBEEF).unwrap_err().code);
    }

    #[test]
    fn edit_status_every_editor_refusal_is_a_caller_error_not_parse_error() {
        let refusals = [
            err!(NotFound), err!(NotAMapping), err!(NotASequence), err!(NotAContainer), err!(UnsupportedShape),
            err!(NotASection), err!(NotAnInlineArray), err!(NotAnArrayOfTables), err!(SectionExists),
            err!(DuplicateKey), err!(MergeOnlyKey), err!(CannotDeleteSection), err!(CannotReplaceSection),
            err!(CannotMoveSection), err!(CannotReorderSections), err!(EmptyInlineContainer),
            err!(KeyRequiresMultilineForm), err!(BlockValueIntoFlow), err!(CommentsUnsupported),
            err!(NullUnsupported), err!(MultilineComment), err!(InvalidComment), err!(CommentsUnanchored),
            err!(RendererRefused), err!(ImplicitSection), err!(ContainerClosesOnItsLine), err!(NotAKey),
            err!(NonFiniteNumber), err!(CommentInsideValue), err!(DeleteChangedOtherValues),
            err!(NoEmptyRootList), err!(EditReadsBackWrong),
        ];
        for e in refusals {
            assert_ne!(Status::ParseError, edit_status(e), "{e:?}");
        }
        assert_eq!(Status::ParseError, edit_status(err!(FigEmptyContainer)));
        assert_eq!(Status::ParseError, edit_status(err!(FigDuplicateKey)));
        assert_eq!(Status::UnsupportedOperation, edit_status(err!(CommentNotAnEntry)));
        assert_eq!(Status::InternalError, edit_status(err!(MultipleInit)));
    }

    #[test]
    fn statuses_carry_the_c_abi_numbers_and_the_bindings_names() {
        for s in [
            Status::Ok, Status::InvalidArgument, Status::ParseError, Status::OutOfMemory, Status::UnsupportedFormat,
            Status::NotFound, Status::UnsupportedOperation, Status::InternalError,
        ] {
            assert_eq!(Some(s), Status::from_code(s.code()));
        }
        assert_eq!(255, Status::InternalError.code());
        assert_eq!("UnsupportedOperation", Status::UnsupportedOperation.name());
    }
}
