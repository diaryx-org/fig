//! fig.h's records, as `#[repr(C)]` Rust (`kit/src/abi.zig` and the
//! `extern struct`s of `src/c_api.zig`). Field for field and in order, so
//! a C caller and this library agree on every offset. A C `bool` a caller
//! writes is read as a `u8` here — nonzero is true — so that no byte a
//! caller hands over is an invalid Rust value. The node table's rows and
//! side facts are fig-core's own (`fig_core::kit::table`), which are
//! `#[repr(C)]` in fig.h's layout.

use core::ffi::{c_char, c_int, c_void};

use fig_core::kit::table::{self, Anchor, Comment, Diagnostic, Directive, ItemSep, Mention, Row, SpanFact, Table, Tag};

// ============================================================================
// STATUS
// ============================================================================

/// `FigStatus`.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigStatus {
    Ok = 0,
    InvalidArgument = 1,
    ParseError = 2,
    OutOfMemory = 3,
    UnsupportedFormat = 4,
    NotFound = 5,
    UnsupportedOperation = 6,
    InternalError = 255,
}

impl From<fig_core::api::Status> for FigStatus {
    fn from(s: fig_core::api::Status) -> FigStatus {
        use fig_core::api::Status as S;
        match s {
            S::Ok => FigStatus::Ok,
            S::InvalidArgument => FigStatus::InvalidArgument,
            S::ParseError => FigStatus::ParseError,
            S::OutOfMemory => FigStatus::OutOfMemory,
            S::UnsupportedFormat => FigStatus::UnsupportedFormat,
            S::NotFound => FigStatus::NotFound,
            S::UnsupportedOperation => FigStatus::UnsupportedOperation,
            S::InternalError => FigStatus::InternalError,
        }
    }
}

// ============================================================================
// THE CALLER'S RECORDS
// ============================================================================

/// `FigError`: caller-allocated, its `size` the version tag.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigError {
    pub size: u32,
    pub code: c_int,
    pub byte_offset: usize,
    pub line: u32,
    pub column: u32,
    pub message_len: usize,
    pub message: [u8; 256],
}

impl FigError {
    /// `ErrorInfo.empty`: what fig hands a language's function to fill.
    pub const EMPTY: FigError =
        FigError { size: core::mem::size_of::<FigError>() as u32, code: 0, byte_offset: 0, line: 0, column: 0, message_len: 0, message: [0; 256] };

    /// `ErrorInfo.text`: the message a language wrote.
    pub fn text(&self) -> &[u8] {
        &self.message[..self.message_len.min(self.message.len())]
    }
}

/// `FigPathSegment`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigPathSegment {
    pub kind: c_int,
    pub key_ptr: *const u8,
    pub key_len: usize,
    pub index: usize,
}

pub const FIG_SEGMENT_KEY: c_int = 0;
pub const FIG_SEGMENT_INDEX: c_int = 1;

/// `FigStr`: a borrowed byte slice, `len == FIG_LEN_NONE` for none.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FigStr {
    pub ptr: *const u8,
    pub len: usize,
}

pub const FIG_LEN_NONE: usize = usize::MAX;

impl FigStr {
    pub const EMPTY: FigStr = FigStr { ptr: core::ptr::null(), len: 0 };

    pub fn of(s: &[u8]) -> FigStr {
        FigStr { ptr: s.as_ptr(), len: s.len() }
    }
}

/// `FigKeyValue`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigKeyValue {
    pub key: u32,
    pub value: u32,
}

/// `FigSerializeOptions`, size-gated.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigSerializeOptions {
    pub size: u32,
    pub pretty: u8,
    pub indent: u8,
    pub strip_comments: u8,
    pub lossless: u8,
    pub width: u16,
    pub flow: u8,
    pub splice: u8,
}

/// `FigWarning`, size-gated.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigWarning {
    pub size: u32,
    pub code: c_int,
    pub cause: c_int,
    pub path: *const u8,
    pub path_len: usize,
    pub note: *const u8,
    pub note_len: usize,
}

/// `FigStreamDoc`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FigStreamDoc {
    pub start: usize,
    pub end: usize,
    pub marked: bool,
}

/// `FigXmlToken`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FigXmlToken {
    pub kind: u8,
    pub reserved: [u8; 3],
    pub start: u32,
    pub end: u32,
}

// ============================================================================
// THE NODE TABLE
// ============================================================================

/// `FigTable`: fig-core's table, opaque to C.
pub type FigTable = Table;
pub type FigRow = Row;
pub type FigText = table::Text;

/// `FigTableView`: what a print is handed, fig's `sizeof` in `size`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigTableView {
    pub size: u32,
    pub source: *const u8,
    pub source_len: usize,
    pub strings: *const u8,
    pub strings_len: usize,
    pub rows: *const Row,
    pub row_count: usize,
    pub anchors: *const Anchor,
    pub anchor_count: usize,
    pub tags: *const Tag,
    pub tag_count: usize,
    pub markers: *const SpanFact,
    pub marker_count: usize,
    pub seps: *const SpanFact,
    pub sep_count: usize,
    pub item_seps: *const ItemSep,
    pub item_sep_count: usize,
    pub comments: *const Comment,
    pub comment_count: usize,
    pub regions: *const SpanFact,
    pub region_count: usize,
    pub mentions: *const Mention,
    pub mention_count: usize,
    pub directives: *const Directive,
    pub directive_count: usize,
    pub diagnostics: *const Diagnostic,
    pub diagnostic_count: usize,
}

impl FigTableView {
    /// The C view of `v`, borrowing what `v` borrows.
    pub fn of(v: &table::View<'_>) -> FigTableView {
        FigTableView {
            size: core::mem::size_of::<FigTableView>() as u32,
            source: v.source.as_ptr(),
            source_len: v.source.len(),
            strings: v.strings.as_ptr(),
            strings_len: v.strings.len(),
            rows: v.rows.as_ptr(),
            row_count: v.rows.len(),
            anchors: v.anchors.as_ptr(),
            anchor_count: v.anchors.len(),
            tags: v.tags.as_ptr(),
            tag_count: v.tags.len(),
            markers: v.markers.as_ptr(),
            marker_count: v.markers.len(),
            seps: v.seps.as_ptr(),
            sep_count: v.seps.len(),
            item_seps: v.item_seps.as_ptr(),
            item_sep_count: v.item_seps.len(),
            comments: v.comments.as_ptr(),
            comment_count: v.comments.len(),
            regions: v.regions.as_ptr(),
            region_count: v.regions.len(),
            mentions: v.mentions.as_ptr(),
            mention_count: v.mentions.len(),
            directives: v.directives.as_ptr(),
            directive_count: v.directives.len(),
            diagnostics: v.diagnostics.as_ptr(),
            diagnostic_count: v.diagnostics.len(),
        }
    }
}

/// `FigPrintOptions`, fig's `sizeof` in `size`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigPrintOptions {
    pub size: u32,
    pub pretty: bool,
    pub strip_comments: bool,
    pub indent: u8,
    pub width: u16,
    pub splice: bool,
    pub flow: bool,
}

impl FigPrintOptions {
    pub fn of(o: &fig_core::wire::PrintOptions) -> FigPrintOptions {
        FigPrintOptions {
            size: core::mem::size_of::<FigPrintOptions>() as u32,
            pretty: o.pretty,
            strip_comments: o.strip_comments,
            indent: o.indent,
            width: o.width,
            splice: o.splice,
            flow: o.flow,
        }
    }
}

// ============================================================================
// THE LANGUAGE'S RECORDS
// ============================================================================

/// `FigCommentDelimiter`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigCommentDelimiter {
    pub open: *const c_char,
    pub close: *const c_char,
    pub forbidden: *const c_char,
}

/// `FigComments`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigComments {
    pub line: FigCommentDelimiter,
    pub trailing: FigCommentDelimiter,
    pub block: FigCommentDelimiter,
}

/// `FigSectionHeader`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigSectionHeader {
    pub open: *const c_char,
    pub close: *const c_char,
    pub seq_open: *const c_char,
    pub seq_close: *const c_char,
    pub sep: *const c_char,
    pub skip_index: u8,
}

/// `FigClosedContainers`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigClosedContainers {
    pub map_open: *const c_char,
    pub map_close: *const c_char,
    pub seq_open: *const c_char,
    pub seq_close: *const c_char,
}

/// `FigSyntax`, its writer's `sizeof` in `size`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigSyntax {
    pub size: u32,
    pub comments: FigComments,
    pub kv_sep: *const c_char,
    pub flow_kv_sep_from_siblings: u8,
    pub flow_map_pad: *const c_char,
    pub key_sigil: u8,
    pub empty_map_literal: *const c_char,
    pub block_seq_editable: u8,
    pub flow_containers: u8,
    pub indent_unit: *const c_char,
    pub seq_item_marker: *const c_char,
    pub closed_containers: FigClosedContainers,
    pub single_line_block_mapping: u8,
    pub bare_document_mapping: u8,
    pub flow_map_open: *const c_char,
    pub flow_map_close: *const c_char,
    pub structural_indent: u8,
    pub section_noun: *const c_char,
    pub section_header: FigSectionHeader,
    pub merge_key: *const c_char,
    pub flow_entry_sep: *const c_char,
    pub flow_root: u8,
    pub flow_maps_only: u8,
    pub line_prefix_marks: *const c_char,
    pub flow_map_kv_sep: *const c_char,
    pub flow_map_entry_sep: *const c_char,
    pub line_continuation: u8,
    pub empty_seq_literal: *const c_char,
    pub opener_line_comment: u8,
}

const NO_DELIMITER: FigCommentDelimiter =
    FigCommentDelimiter { open: core::ptr::null(), close: core::ptr::null(), forbidden: core::ptr::null() };

impl FigSyntax {
    /// Every field's declared default (`SyntaxDesc`'s initializers).
    pub const DEFAULT: FigSyntax = FigSyntax {
        size: core::mem::size_of::<FigSyntax>() as u32,
        comments: FigComments { line: NO_DELIMITER, trailing: NO_DELIMITER, block: NO_DELIMITER },
        kv_sep: core::ptr::null(),
        flow_kv_sep_from_siblings: 0,
        flow_map_pad: core::ptr::null(),
        key_sigil: 0,
        empty_map_literal: core::ptr::null(),
        block_seq_editable: 1,
        flow_containers: 1,
        indent_unit: core::ptr::null(),
        seq_item_marker: core::ptr::null(),
        closed_containers: FigClosedContainers {
            map_open: core::ptr::null(),
            map_close: core::ptr::null(),
            seq_open: core::ptr::null(),
            seq_close: core::ptr::null(),
        },
        single_line_block_mapping: 0,
        bare_document_mapping: 1,
        flow_map_open: core::ptr::null(),
        flow_map_close: core::ptr::null(),
        structural_indent: 0,
        section_noun: core::ptr::null(),
        section_header: FigSectionHeader {
            open: core::ptr::null(),
            close: core::ptr::null(),
            seq_open: core::ptr::null(),
            seq_close: core::ptr::null(),
            sep: core::ptr::null(),
            skip_index: 1,
        },
        merge_key: core::ptr::null(),
        flow_entry_sep: core::ptr::null(),
        flow_root: 0,
        flow_maps_only: 0,
        line_prefix_marks: core::ptr::null(),
        flow_map_kv_sep: core::ptr::null(),
        flow_map_entry_sep: core::ptr::null(),
        line_continuation: 0,
        empty_seq_literal: core::ptr::null(),
        opener_line_comment: 0,
    };
}

/// `FigDialectDesc`, read at `FigLanguageVTable.dialect_size`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigDialectDesc {
    pub name: *const c_char,
    pub extensions: *const *const c_char,
    pub splice: c_int,
    pub empty_doc_seed: *const c_char,
    pub syntax: *const FigSyntax,
    pub prints_holds: u32,
    pub prints_flags: u32,
    pub prints_char_as: *const c_char,
    pub prints_number_special_as: *const c_char,
    pub prints_comments: c_int,
}

impl FigDialectDesc {
    pub const DEFAULT: FigDialectDesc = FigDialectDesc {
        name: core::ptr::null(),
        extensions: core::ptr::null(),
        splice: 0,
        empty_doc_seed: core::ptr::null(),
        syntax: core::ptr::null(),
        prints_holds: 0,
        prints_flags: 0,
        prints_char_as: core::ptr::null(),
        prints_number_special_as: core::ptr::null(),
        prints_comments: 0,
    };
}

pub const FIG_PRINTS_TEXT_SCALARS: u32 = 1 << 0;
pub const FIG_PRINTS_NULL_AT_ROOT: u32 = 1 << 1;
pub const FIG_PRINTS_ROOT_CONTAINER: u32 = 1 << 2;

/// `FigRenderRequest`, fig's `sizeof` in `size`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigRenderRequest {
    pub size: u32,
    pub dialect: *const c_char,
    pub indent: FigStr,
    pub key: FigStr,
    pub value: FigStr,
    pub literal: *const c_char,
    pub old_key: FigStr,
    pub parent_key: FigStr,
    pub parent_tag: FigStr,
    pub old_value: FigStr,
}

pub type FigParseFn = unsafe extern "C" fn(ctx: *mut c_void, dialect: *const c_char, input: FigStr, out: *mut FigTable, err: *mut FigError) -> c_int;
pub type FigPrintFn = unsafe extern "C" fn(
    ctx: *mut c_void,
    dialect: *const c_char,
    table: *const FigTableView,
    options: *const FigPrintOptions,
    out: *mut FigStr,
    err: *mut FigError,
) -> c_int;
pub type FigFreeBytesFn = unsafe extern "C" fn(ctx: *mut c_void, bytes: FigStr);
pub type FigRenderFn = unsafe extern "C" fn(ctx: *mut c_void, request: *const FigRenderRequest, out: *mut FigStr, err: *mut FigError) -> c_int;
pub type FigSplitStreamFn =
    unsafe extern "C" fn(ctx: *mut c_void, dialect: *const c_char, input: FigStr, out: *mut FigStr, err: *mut FigError) -> c_int;

/// `FigLanguageVTable`, its writer's `sizeof` in `size`. The two slots
/// fig.h requires are `Option` here so that a null one is refused rather
/// than called.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FigLanguageVTable {
    pub version: u32,
    pub size: u32,
    pub ctx: *mut c_void,
    pub name: *const c_char,
    pub caps: u32,
    pub max_mapping_depth: c_int,
    pub lossless: u32,
    pub syntax: *const FigSyntax,
    pub dialects: *const FigDialectDesc,
    pub dialect_count: usize,
    pub dialect_size: usize,
    pub samples: *const FigStr,
    pub sample_count: usize,

    pub parse: Option<FigParseFn>,
    pub print: Option<FigPrintFn>,
    pub free_bytes: Option<FigFreeBytesFn>,

    pub render_value: Option<FigRenderFn>,
    pub render_entry: Option<FigRenderFn>,
    pub render_item: Option<FigRenderFn>,
    pub render_tail: Option<FigRenderFn>,
    pub render_key: Option<FigRenderFn>,
    pub render_key_name: Option<FigRenderFn>,
    pub split_stream: Option<FigSplitStreamFn>,
    pub render_string: Option<FigRenderFn>,
}

pub const FIG_DEPTH_NONE: c_int = -1;

impl FigLanguageVTable {
    /// Every field's declared default (`VTable`'s initializers).
    pub const DEFAULT: FigLanguageVTable = FigLanguageVTable {
        version: 0,
        size: core::mem::size_of::<FigLanguageVTable>() as u32,
        ctx: core::ptr::null_mut(),
        name: core::ptr::null(),
        caps: 0,
        max_mapping_depth: FIG_DEPTH_NONE,
        lossless: 0,
        syntax: core::ptr::null(),
        dialects: core::ptr::null(),
        dialect_count: 0,
        dialect_size: core::mem::size_of::<FigDialectDesc>(),
        samples: core::ptr::null(),
        sample_count: 0,
        parse: None,
        print: None,
        free_bytes: None,
        render_value: None,
        render_entry: None,
        render_item: None,
        render_tail: None,
        render_key: None,
        render_key_name: None,
        split_stream: None,
        render_string: None,
    };
}

// ============================================================================
// SIZE-GATED READS
// ============================================================================

/// `abi.gated`: a `$T` read from the `$n` bytes at `$src`, a record its
/// writer laid out with `sizeof == $n` — the prefix this `$T` shares with
/// it, and `$default`'s value for every listed field past what the bytes
/// cover. The caller states that `$src` is readable for `$n` bytes.
macro_rules! gated {
    ($T:ty, $src:expr, $n:expr, $default:expr, [$($field:ident),* $(,)?]) => {{
        let default: $T = $default;
        let mut out: $T = default;
        let n = core::cmp::min($n, core::mem::size_of::<$T>());
        // SAFETY: the caller states `$src` is readable for `$n` bytes, and
        // `out` is a `$T` of at least `n` bytes. Every field of the records
        // read this way is an integer, a raw pointer or an `Option` of a
        // function pointer, for which any bytes are a value.
        unsafe { core::ptr::copy_nonoverlapping($src as *const u8, (&mut out as *mut $T).cast::<u8>(), n) };
        $(
            if core::mem::offset_of!($T, $field) + core::mem::size_of_val(&default.$field) > n {
                out.$field = default.$field;
            }
        )*
        out
    }};
}
pub(crate) use gated;

/// `abi.requiredSize`: where the last field with no declared default ends.
pub const VTABLE_REQUIRED: usize = core::mem::offset_of!(FigLanguageVTable, free_bytes) + core::mem::size_of::<Option<FigFreeBytesFn>>();
pub const DIALECT_REQUIRED: usize = core::mem::offset_of!(FigDialectDesc, name) + core::mem::size_of::<*const c_char>();
pub const SYNTAX_REQUIRED: usize = core::mem::offset_of!(FigSyntax, comments) + core::mem::size_of::<FigComments>();

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    /// The layouts fig.h states, on a 64-bit target (the TypeScript
    /// binding's sizes are the wasm32 ones, `bindings/typescript/src`).
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_records_have_fig_h_layout() {
        assert_eq!(288, size_of::<FigError>());
        assert_eq!(32, offset_of!(FigError, message));
        assert_eq!(12, size_of::<FigSerializeOptions>());
        assert_eq!(10, offset_of!(FigSerializeOptions, flow));
        assert_eq!(48, size_of::<FigWarning>());
        assert_eq!(24, size_of::<FigStreamDoc>());
        assert_eq!(12, size_of::<FigXmlToken>());
        assert_eq!(32, size_of::<Row>());
        assert_eq!(16, offset_of!(Row, text));
        assert_eq!(20, size_of::<Anchor>());
        assert_eq!(24, size_of::<Tag>());
        assert_eq!(12, size_of::<SpanFact>());
        assert_eq!(12, size_of::<ItemSep>());
        assert_eq!(16, size_of::<Comment>());
        assert_eq!(16, size_of::<Mention>());
        assert_eq!(16, size_of::<Directive>());
        assert_eq!(28, size_of::<Diagnostic>());
        assert_eq!(4 + 4 + 13 * 16, size_of::<FigTableView>());
        assert_eq!(12, size_of::<FigPrintOptions>());
        assert_eq!(8, offset_of!(FigPrintOptions, width));
        assert_eq!(24, offset_of!(FigLanguageVTable, caps));
        assert_eq!(104, offset_of!(FigLanguageVTable, free_bytes));
        assert_eq!(112, VTABLE_REQUIRED);
        assert_eq!(176, size_of::<FigLanguageVTable>());
        assert_eq!(72, size_of::<FigDialectDesc>());
        assert_eq!(8, DIALECT_REQUIRED);
        assert_eq!(80, SYNTAX_REQUIRED);
        assert_eq!(136, size_of::<FigRenderRequest>());
    }

    #[test]
    fn a_short_record_reads_its_prefix_and_the_defaults_past_it() {
        let mut full = FigSyntax::DEFAULT;
        full.block_seq_editable = 0;
        full.opener_line_comment = 1;
        // Covering everything up to `block_seq_editable`, and no further.
        let n = offset_of!(FigSyntax, block_seq_editable) + 1;
        let read = gated!(FigSyntax, &full as *const FigSyntax, n, FigSyntax::DEFAULT, [flow_containers, opener_line_comment]);
        assert_eq!(0, read.block_seq_editable);
        assert_eq!(1, read.flow_containers);
        assert_eq!(0, read.opener_line_comment);
    }
}
