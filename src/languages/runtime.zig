//! Runtime languages: the `Language` contract of `language.zig`, carried as a
//! table of function pointers filled when the program runs rather than as
//! declarations the compiler reads. See `docs/proposals/runtime-languages.md`.
//!
//! Three things live here, and they are the whole of what core adds for a
//! format it did not compile in:
//!
//!   * **The C shapes.** `VTable` is the record a host fills to register a
//!     language — its declarations as fields, its `parse` and `print` as
//!     function pointers, its renderers as pointers that may be null — and
//!     `NodeTable` is what `parse` returns and `print` receives: one `NodeRow`
//!     per node in pre-order plus three side tables, which is `Document`
//!     restated as values (proposal §4.1). They are `extern struct`s because
//!     they ARE the C ABI: `c_api.zig` re-exports them under their `Fig*`
//!     names, fig.h states them, and `zig build abi-check` holds the two
//!     together. A Zig host fills the same structs.
//!   * **The registry.** `register` validates a vtable by the rules
//!     `Language.validate` applies at comptime, runs the harness of
//!     `harness.zig` over the samples it declares, and — only then — appends
//!     one `Entry` per dialect row and returns the first's ABI integer, at or
//!     above `Language.runtime_abi_base`. Append-only under a mutex; an entry
//!     is immutable once its integer is out, so every read is of a settled
//!     value and the threading promise in fig.h holds. There is no
//!     unregistration: an integer is valid for the life of the process.
//!   * **`Language`.** A `Language` in the sense of `language.zig` — the same
//!     `Type`/`Parser`/`parse`/`print`/`syntax` surface every compiled format
//!     has — whose `Type` indexes the registry and whose every function reads
//!     the entry it names and calls through the vtable. `Editor(Language)`
//!     therefore instantiates ONCE, the C API's editor union gains one arm,
//!     and the splice engine, the reparse net, the regions, the section
//!     rules and the comment ops are the compiled ones, unchanged. The
//!     renderers answer `Editor.hasRenderer` with a null check.
//!
//! The conversions between `NodeTable` and `Document` are the boundary's
//! cost. `tableToDocument` copies every string a table carries into the
//! AST's `owned_strings` and calls the vtable's `free_table` once, so a
//! helper's memory never outlives the call that produced it; `documentToTable`
//! borrows the AST's strings for the duration of a `print`. What a runtime
//! format cannot do is add a node kind: `ExtKind` is closed, and a table row
//! naming one that does not exist is refused at conversion.

const std = @import("std");
const Allocator = std.mem.Allocator;
const AST = @import("../ast/ast.zig");
const Document = @import("../document.zig");
const Span = @import("../util/span.zig");
const manifest = @import("manifest.zig");
const Languages = @import("language.zig");
const editor = @import("../editor.zig");

const Node = AST.Node;
const ExtKind = Node.Kind.Extended.ExtKind;

// ============================================================================
// THE C SHAPES
// ============================================================================
//
// Every struct below is `extern`, and every one is stated in fig.h under the
// `Fig` prefix. A field is added at the END of a struct only, and a field
// never changes meaning. Changing a field's meaning is a bump of
// `vtable_version`; appending one is not, where only fig writes the struct
// and a language only reads it (`PrintOptions`) — an older language never
// reads past what it knows. `Str` and `CSpan`
// are the same two-word records fig.h has carried as `FigStr` and `FigSpan`
// since 2.x, restated here so this file is a leaf.

/// A borrowed byte slice: `ptr[0..len]`. A null `ptr` with `len == 0` is the
/// empty string; the `none` value below — null pointer, `none_len` — is how
/// an optional column says "absent", which an empty string is not (an empty
/// scalar is a value; an absent anchor is not an anchor).
pub const Str = extern struct {
    ptr: ?[*]const u8 = null,
    len: usize = 0,

    pub const none_len: usize = std.math.maxInt(usize);
    pub const none: Str = .{ .ptr = null, .len = none_len };

    pub fn of(s: []const u8) Str {
        return .{ .ptr = s.ptr, .len = s.len };
    }

    /// The bytes, or null for `none` (or for a null pointer with a length,
    /// which is malformed and read as absent rather than dereferenced).
    pub fn slice(self: Str) ?[]const u8 {
        if (self.len == none_len) return null;
        if (self.len == 0) return &.{};
        const p = self.ptr orelse return null;
        return p[0..self.len];
    }
};

/// `[start, end)` byte offsets. `none` — both `none_offset` — is an absent
/// optional column.
pub const CSpan = extern struct {
    start: usize,
    end: usize,

    pub const none_offset: usize = std.math.maxInt(usize);
    pub const none: CSpan = .{ .start = none_offset, .end = none_offset };

    pub fn of(s: Span) CSpan {
        return .{ .start = s.start, .end = s.end };
    }

    pub fn span(self: CSpan) ?Span {
        if (self.start == none_offset) return null;
        return Span.init(self.start, self.end);
    }
};

/// The row index that says "no row": the root's `parent`.
pub const no_node: u32 = std.math.maxInt(u32);

/// `NodeRow.kind` — the values of fig.h's `FigNodeKind`, which
/// `fig_node_kind` has reported since 1.0.
pub const RowKind = enum(c_int) {
    null = 0,
    bool = 1,
    int = 2,
    float = 3,
    string = 4,
    sequence = 5,
    mapping = 6,
    keyvalue = 7,
    alias = 8,
    _,
};

/// `NodeRow.ext_kind`: a `FigExtKind` value (the ordinal of `ExtKind`), or
/// `no_ext_kind`.
pub const no_ext_kind: c_int = -1;
/// `FIG_DEPTH_NONE`: a vtable's `max_mapping_depth` when the format holds
/// mappings to any depth.
pub const no_depth_limit: c_int = -1;

/// One node. Row index is node id; rows are in pre-order, so a parent
/// precedes its children and a keyvalue is followed by its key row and then
/// its value row.
pub const NodeRow = extern struct {
    /// `RowKind`. For an extended scalar, the kind `fig_node_kind` would
    /// report (`string`, or `int` for a char literal); `ext_kind` decides.
    kind: c_int,
    /// `FigExtKind`, or `no_ext_kind`.
    ext_kind: c_int = no_ext_kind,
    /// Row index of the parent, or `no_node` for the root.
    parent: u32,
    /// Required of every row on the parse side; `CSpan.none` on the print
    /// side, where there is no source.
    span: CSpan,
    /// A scalar's decoded bytes; an int or float's lexeme; `true`/`false`
    /// for a bool; an alias's target anchor name; an extended kind's payload.
    /// `Str.none` for a container or a null.
    text: Str = .none,
    /// The anchor name this row defines and where the `&name` token is, or
    /// `none`.
    anchor: Str = .none,
    anchor_span: CSpan = .none,
    /// The tag on this row, verbatim (`!foo`, `!!str`), and where it is
    /// written, or `none`.
    tag: Str = .none,
    tag_span: CSpan = .none,
    /// For a block-sequence item: the span of the `-`/`*` that introduces
    /// it. `Document.node_marker_spans`.
    marker: CSpan = .none,
    /// For a keyvalue: the span of the token separating key from value; a
    /// zero-width one marks a value hanging under a bare key.
    /// `Document.node_sep_spans`.
    sep: CSpan = .none,
};

/// One header line of a section node. `Document.NodeRegion`.
pub const RegionRow = extern struct { node: u32, start: usize, end: usize };

/// One place a section node's name is written. `Document.NodeMention`;
/// `kind` is 0 for a header line of the node's own, 1 for a mention on the
/// parent's entry line.
pub const MentionRow = extern struct { node: u32, span: CSpan, kind: c_int };

pub const mention_header: c_int = 0;
pub const mention_entry: c_int = 1;

/// One comment bound to a row. `slot` is 0 leading, 1 trailing, 2 dangling
/// (`AST.NodeComments`); `style` is 0 line, 1 block (`AST.Comment.Style`).
/// Rows are grouped by node and in source order within a slot; at most one
/// trailing per node.
pub const CommentRow = extern struct { node: u32, slot: c_int, style: c_int, text: Str };

pub const comment_leading: c_int = 0;
pub const comment_trailing: c_int = 1;
pub const comment_dangling: c_int = 2;

/// One tag-handle declaration of the document — a YAML `%TAG` directive's
/// handle (`!e!`, or a redefined `!`/`!!`) and the prefix it expands to.
/// `AST.TagDirective`. A tag spelled with a named handle is legal only in a
/// document that declares it, so the declarations travel with the rows: a
/// parse returns those it read, in source order, and a print of a whole
/// document receives them back to re-emit above any tag that uses one.
/// Empty for every format without directives.
pub const DirectiveRow = extern struct { handle: Str, prefix: Str };

/// What `parse` returns and `print` receives. Zero rows is the empty
/// document — a format whose empty input is a null document returns one
/// `null` row instead.
pub const NodeTable = extern struct {
    rows: ?[*]const NodeRow = null,
    row_count: usize = 0,
    regions: ?[*]const RegionRow = null,
    region_count: usize = 0,
    mentions: ?[*]const MentionRow = null,
    mention_count: usize = 0,
    comments: ?[*]const CommentRow = null,
    comment_count: usize = 0,
    directives: ?[*]const DirectiveRow = null,
    directive_count: usize = 0,
    /// The helper's own handle on the memory behind the table, set by
    /// `parse` and read back by `free_table`; core never touches it. A
    /// helper whose rows and strings live in one allocation it can find
    /// from `rows` may leave it null.
    owner: ?*anyopaque = null,

    pub fn rowSlice(self: *const NodeTable) []const NodeRow {
        const p = self.rows orelse return &.{};
        return p[0..self.row_count];
    }
    pub fn regionSlice(self: *const NodeTable) []const RegionRow {
        const p = self.regions orelse return &.{};
        return p[0..self.region_count];
    }
    pub fn mentionSlice(self: *const NodeTable) []const MentionRow {
        const p = self.mentions orelse return &.{};
        return p[0..self.mention_count];
    }
    pub fn commentSlice(self: *const NodeTable) []const CommentRow {
        const p = self.comments orelse return &.{};
        return p[0..self.comment_count];
    }
    pub fn directiveSlice(self: *const NodeTable) []const DirectiveRow {
        const p = self.directives orelse return &.{};
        return p[0..self.directive_count];
    }
};

/// A diagnostic a vtable function fills on failure. Field for field the
/// `FigError` of fig.h — `c_api.zig` aliases that name to this — so a helper
/// writes the same record a C caller reads. `size` is the caller's
/// `sizeof`; the library and a helper write only what it covers.
pub const ErrorInfo = extern struct {
    size: u32,
    code: c_int,
    byte_offset: usize,
    line: u32,
    column: u32,
    message_len: usize,
    message: [256]u8,

    pub const empty: ErrorInfo = .{
        .size = @sizeOf(ErrorInfo),
        .code = 0,
        .byte_offset = 0,
        .line = 0,
        .column = 0,
        .message_len = 0,
        .message = [_]u8{0} ** 256,
    };

    pub fn text(self: *const ErrorInfo) []const u8 {
        return self.message[0..@min(self.message_len, self.message.len)];
    }

    /// Fill `message`, truncating to what the inline buffer holds.
    pub fn set(self: *ErrorInfo, message: []const u8) void {
        const n = @min(message.len, self.message.len - 1);
        @memcpy(self.message[0..n], message[0..n]);
        self.message[n] = 0;
        self.message_len = n;
    }
};

/// The subset of `AST.SerializeOptions` a printer outside core is told. fig
/// writes it and a language reads it, so a field appended here reaches a
/// language that knows it and is never read by one that does not — not a
/// `vtable_version` bump.
pub const PrintOptions = extern struct {
    pretty: bool = true,
    strip_comments: bool = false,
    indent: u8 = 2,
    width: u16 = 80,
    /// Print the value as the editor takes it spliced into a document, not
    /// as a document of its own — `AST.SerializeOptions.splice`. A language
    /// whose document wraps its root (plist) or spells a scalar root
    /// differently from a scalar in place (NestedText's `>` block) answers
    /// it; every other prints the same either way.
    splice: bool = false,
};

/// `manifest.CommentDelimiter`.
pub const CommentDelimiterDesc = extern struct {
    /// Null for "no delimiter" where the field is optional.
    open: ?[*:0]const u8 = null,
    close: ?[*:0]const u8 = null,
    forbidden: ?[*:0]const u8 = null,
};

/// `manifest.Comments`. `style` is `manifest.CommentStyle`'s ordinal.
pub const CommentsDesc = extern struct {
    style: c_int,
    line: CommentDelimiterDesc,
    trailing: CommentDelimiterDesc,
};

/// `manifest.SectionHeader`; `open == null` means no header syntax.
pub const SectionHeaderDesc = extern struct {
    open: ?[*:0]const u8 = null,
    close: ?[*:0]const u8 = null,
    seq_open: ?[*:0]const u8 = null,
    seq_close: ?[*:0]const u8 = null,
    sep: ?[*:0]const u8 = null,
    skip_index: bool = true,
};

/// `manifest.ClosedContainers`; `map_open == null` means none.
pub const ClosedContainersDesc = extern struct {
    map_open: ?[*:0]const u8 = null,
    map_close: ?[*:0]const u8 = null,
    seq_open: ?[*:0]const u8 = null,
    seq_close: ?[*:0]const u8 = null,
};

/// `manifest.Syntax`, field for field, with each `?[]const u8` a nullable C
/// string and each enum its ordinal. A `null` string where the Zig field is
/// non-optional takes that field's default.
pub const SyntaxDesc = extern struct {
    /// `comments.style` is required; `line`/`trailing` may be null.
    comments: CommentsDesc,
    kv_sep: ?[*:0]const u8 = null,
    flow_kv_sep_from_siblings: bool = false,
    flow_map_pad: ?[*:0]const u8 = null,
    /// `manifest.KeyStyle` ordinal.
    key_style: c_int = 0,
    /// 0 for none.
    key_sigil: u8 = 0,
    empty_map_literal: ?[*:0]const u8 = null,
    block_seq_editable: bool = true,
    flow_containers: bool = true,
    indent_unit: ?[*:0]const u8 = null,
    seq_item_marker: ?[*:0]const u8 = null,
    closed_containers: ClosedContainersDesc = .{},
    single_line_block_mapping: bool = false,
    bare_document_mapping: bool = true,
    flow_map_open: ?[*:0]const u8 = null,
    flow_map_close: ?[*:0]const u8 = null,
    structural_indent: bool = false,
    /// `manifest.SectionNoun` ordinal, or -1 for none.
    section_noun: c_int = -1,
    section_header: SectionHeaderDesc = .{},
    merge_key: ?[*:0]const u8 = null,
};

/// `manifest.NativeKinds`: the ten booleans, one per `ExtKind` plus `null`,
/// in `NativeKinds`' field order. Pinned below.
pub const NativeKindsDesc = extern struct {
    null: bool = false,
    offset_datetime: bool = false,
    local_datetime: bool = false,
    local_date: bool = false,
    local_time: bool = false,
    enum_literal: bool = false,
    char_literal: bool = false,
    number_special: bool = false,
    plist_date: bool = false,
    plist_data: bool = false,
};

comptime {
    // `NativeKindsDesc` is `manifest.NativeKinds` by another name: same
    // fields, same order, so the conversion below is a field-by-field copy
    // and a kind added to one without the other fails here.
    const a = @typeInfo(NativeKindsDesc).@"struct".fields;
    const b = @typeInfo(manifest.NativeKinds).@"struct".fields;
    if (a.len != b.len) @compileError("runtime.NativeKindsDesc and manifest.NativeKinds differ in field count");
    for (a, b) |x, y| {
        if (!std.mem.eql(u8, x.name, y.name))
            @compileError("runtime.NativeKindsDesc field '" ++ x.name ++ "' sits where manifest.NativeKinds has '" ++ y.name ++ "'");
    }
}

/// `manifest.Dialect`'s runtime half: what a dialect row of a runtime
/// language states. Sniff rank, deserializability, embedding and `--spec`
/// names are not here — a runtime format is never sniffed (proposal §8.1)
/// and the rest is deferred until a format asks.
pub const DialectDesc = extern struct {
    /// The member name; the first row's is the language's, and is what
    /// `fig_format_by_name` resolves.
    name: [*:0]const u8,
    /// NULL-terminated array of extensions, or null for none.
    extensions: ?[*]const ?[*:0]const u8 = null,
    /// `manifest.SpliceStyle` ordinal: 0 literal, 1 json_string, 2 raw.
    splice: c_int = 0,
    /// What `set` seeds a missing file with; null refuses creation.
    empty_doc_seed: ?[*:0]const u8 = null,
    /// This dialect's `syntax` where it differs from the language's, else
    /// null.
    syntax: ?*const SyntaxDesc = null,
};

/// The fifth versioned surface: bumped when a field of this struct or of
/// any struct it reaches changes meaning, never for an appended field.
pub const vtable_version: u32 = 1;

pub const ParseFn = *const fn (ctx: ?*anyopaque, dialect: [*:0]const u8, input: Str, out: *NodeTable, err: *ErrorInfo) callconv(.c) c_int;
pub const PrintFn = *const fn (ctx: ?*anyopaque, dialect: [*:0]const u8, table: *const NodeTable, options: *const PrintOptions, out: *Str, err: *ErrorInfo) callconv(.c) c_int;
pub const FreeTableFn = *const fn (ctx: ?*anyopaque, table: *NodeTable) callconv(.c) void;
pub const FreeBytesFn = *const fn (ctx: ?*anyopaque, bytes: Str) callconv(.c) void;
pub const RenderValueFn = *const fn (ctx: ?*anyopaque, dialect: [*:0]const u8, value: Str, literal: [*:0]const u8, out: *Str, err: *ErrorInfo) callconv(.c) c_int;
pub const RenderEntryFn = *const fn (ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Str, key: Str, value: Str, out: *Str, err: *ErrorInfo) callconv(.c) c_int;
pub const RenderItemFn = *const fn (ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Str, value: Str, out: *Str, err: *ErrorInfo) callconv(.c) c_int;
pub const RenderTailFn = *const fn (ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Str, key: Str, value: Str, out: *Str, err: *ErrorInfo) callconv(.c) c_int;
pub const RenderKeyFn = *const fn (ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Str, key: Str, old_key: Str, out: *Str, err: *ErrorInfo) callconv(.c) c_int;

/// The contract, as a record. `register` copies every string and array it
/// reaches, so the struct and what it points to may be freed after the
/// call; `ctx` and the function pointers must stay valid for the life of
/// the process, since there is no unregistration.
///
/// Every function returns 0 on success. On failure it returns a nonzero
/// value — `FigStatus` values are the convention — and fills `err`, whose
/// `message` is what a caller sees. Memory a function hands back (`out` of
/// `parse`, `print` and each renderer) is the helper's, and core returns
/// it through `free_table` or `free_bytes` once it has copied what it
/// needs; a helper that allocates nothing may pass a no-op.
pub const VTable = extern struct {
    /// `vtable_version`. A record with any other value is refused.
    version: u32,
    /// Passed back to every function; opaque to core.
    ctx: ?*anyopaque = null,

    name: [*:0]const u8,
    /// Bits of `FigCapability`: 1 read, 2 edit, 4 serialize.
    caps: u32,
    /// `Caps.max_mapping_depth`; `no_depth_limit` for unbounded. 0 is a
    /// limit: a flat format holds no mapping inside its root.
    max_mapping_depth: c_int = no_depth_limit,
    /// `Caps.lossless`, or null for no envelope.
    lossless: ?*const NativeKindsDesc = null,
    /// Required iff `caps` has the edit bit.
    syntax: ?*const SyntaxDesc = null,
    dialects: [*]const DialectDesc,
    dialect_count: usize,
    /// Required, and at least one: registration runs the harness over
    /// them, and a language that offers none has not shown it parses.
    samples: [*]const Str,
    sample_count: usize,

    parse: ParseFn,
    /// Required iff `caps` has the serialize bit.
    print: ?PrintFn = null,
    free_table: FreeTableFn,
    free_bytes: FreeBytesFn,

    render_value: ?RenderValueFn = null,
    render_entry: ?RenderEntryFn = null,
    render_item: ?RenderItemFn = null,
    render_tail: ?RenderTailFn = null,
    render_key: ?RenderKeyFn = null,
};

/// `FigCapability` bits, restated so this file is a leaf.
pub const cap_read: u32 = 1 << 0;
pub const cap_edit: u32 = 1 << 1;
pub const cap_serialize: u32 = 1 << 2;
pub const cap_references: u32 = 1 << 3;

// ============================================================================
// THE REGISTRY
// ============================================================================

/// One registered dialect: `Language.Type`'s value `index` names it, and
/// `abi` is the integer a C caller holds. Immutable once appended.
pub const Entry = struct {
    index: u16,
    abi: c_int,
    /// The registration this row belongs to, shared by its dialect rows.
    language: *const Registered,
    name: [:0]const u8,
    extensions: []const [:0]const u8,
    splice: manifest.SpliceStyle,
    empty_doc_seed: ?[]const u8,
    /// The dialect's own `syntax`, or the language's.
    syntax: ?manifest.Syntax,

    /// The name handed to every vtable function for this row.
    pub fn dialectZ(self: *const Entry) [*:0]const u8 {
        return self.name.ptr;
    }
    pub fn typeOf(self: *const Entry) Language.Type {
        return @enumFromInt(self.index);
    }
};

/// One registration: the vtable, copied, with its strings owned here.
pub const Registered = struct {
    vt: VTable,
    name: [:0]const u8,
    caps: manifest.Caps,
    syntax: ?manifest.Syntax,
    samples: []const []const u8,
    /// The `Entry` of each dialect row, in declaration order; `entries[0]`
    /// is the row named after the language.
    entries: []const *const Entry,
};

/// As many dialect rows as the registry holds; `Language.Type` is a u16 and
/// the ABI integers run from `runtime_abi_base`, so the bound is the
/// smaller of the two spaces less headroom. Static so that a read needs no
/// lock: `slots[i]` for `i < count` was written before `count` was
/// published and is never written again.
pub const max_entries: usize = 4096;
var slots: [max_entries]*const Entry = undefined;
/// Published with release ordering after the slots it covers are written;
/// read with acquire.
var published: std.atomic.Value(u32) = .init(0);
/// Serializes writers only. A spin lock rather than `std.Io.Mutex`, which
/// wants an `Io` no C entry point has; registration is rare and short.
var write_lock: std.atomic.Value(bool) = .init(false);
const mutex = struct {
    fn lock() void {
        while (write_lock.swap(true, .acquire)) std.atomic.spinLoopHint();
    }
    fn unlock() void {
        write_lock.store(false, .release);
    }
};
var registrations: std.ArrayList(*const Registered) = .empty;
var arenas: std.ArrayList(*std.heap.ArenaAllocator) = .empty;
/// The allocator every registration's memory came from — the first
/// `register`'s. A later call with a different allocator is refused: the
/// lists above are one allocation each, and an entry lives forever.
var registry_allocator: ?Allocator = null;

pub const RegisterError = error{
    /// The record failed `validateVTable`; `last_refusal` has the reason.
    InvalidLanguage,
    /// The harness failed over a declared sample; `last_refusal` says which.
    HarnessFailed,
    /// A name already registered, or one of a compiled-in format.
    NameTaken,
    /// The second registration used a different allocator from the first.
    AllocatorMismatch,
    /// `runtime_abi_base + entries.len` has left `c_int`, or the u16 index
    /// space is full.
    RegistryFull,
    OutOfMemory,
};

/// Why the most recent `register` on this thread was refused, for the C
/// API to report. Thread-local so two hosts registering at once do not
/// read each other's reason.
pub threadlocal var last_refusal: [512]u8 = undefined;
pub threadlocal var last_refusal_len: usize = 0;

/// Record why a registration is about to be refused. Public so a host
/// that builds a vtable from something else (the CLI's helper runner) can
/// report through the same channel when that something fails to load.
pub fn refuse(comptime fmt: []const u8, args: anytype) void {
    const s = std.fmt.bufPrint(&last_refusal, fmt, args) catch &last_refusal;
    last_refusal_len = s.len;
}

pub fn lastRefusal() []const u8 {
    return last_refusal[0..last_refusal_len];
}

/// Register `vt`. Returns the ABI integer of its first dialect row; the
/// rest follow it consecutively. See the module doc for what happens in
/// between.
pub fn register(allocator: Allocator, vt: *const VTable) RegisterError!c_int {
    last_refusal_len = 0;
    if (!validateVTable(vt)) return error.InvalidLanguage;

    mutex.lock();
    defer mutex.unlock();

    if (registry_allocator) |a| {
        // By vtable: a stateless allocator (`c_allocator`) has an undefined
        // `ptr`, and two of them are the same allocator.
        if (a.vtable != allocator.vtable) return error.AllocatorMismatch;
    } else registry_allocator = allocator;

    const name = std.mem.span(vt.name);
    if (findByNameLocked(name) != null or Languages.isCompiledName(name)) {
        refuse("a format named '{s}' is already registered", .{name});
        return error.NameTaken;
    }
    for (vt.dialects[0..vt.dialect_count]) |d| {
        const dn = std.mem.span(d.name);
        if (findByNameLocked(dn) != null or Languages.isCompiledName(dn)) {
            refuse("a format named '{s}' is already registered", .{dn});
            return error.NameTaken;
        }
    }
    const first_index: usize = published.load(.acquire);
    if (first_index + vt.dialect_count > max_entries) return error.RegistryFull;
    const first_abi: i64 = @as(i64, Languages.runtime_abi_base) + @as(i64, @intCast(first_index));

    // Build the registration in an arena so a refusal below frees it whole.
    var arena_state = std.heap.ArenaAllocator.init(allocator);
    errdefer arena_state.deinit();
    const arena = arena_state.allocator();

    const reg = try arena.create(Registered);
    reg.vt = vt.*;
    reg.name = try arena.dupeZ(u8, name);
    reg.vt.name = reg.name.ptr;
    reg.caps = .{
        .read = vt.caps & cap_read != 0,
        .edit = vt.caps & cap_edit != 0,
        .serialize = vt.caps & cap_serialize != 0,
        .references = vt.caps & cap_references != 0,
        .max_mapping_depth = if (vt.max_mapping_depth < 0) null else @intCast(vt.max_mapping_depth),
        .lossless = if (vt.lossless) |l| nativeKindsOf(l) else null,
    };
    reg.syntax = if (vt.syntax) |s| try syntaxOf(arena, s) else null;
    const samples = try arena.alloc([]const u8, vt.sample_count);
    for (vt.samples[0..vt.sample_count], samples) |s, *out| {
        out.* = try arena.dupe(u8, s.slice() orelse "");
    }
    reg.samples = samples;

    const row_entries = try arena.alloc(*const Entry, vt.dialect_count);
    for (vt.dialects[0..vt.dialect_count], row_entries, 0..) |d, *out, i| {
        const e = try arena.create(Entry);
        var exts: std.ArrayList([:0]const u8) = .empty;
        if (d.extensions) |arr| {
            var k: usize = 0;
            while (arr[k]) |x| : (k += 1) try exts.append(arena, try arena.dupeZ(u8, std.mem.span(x)));
        }
        e.* = .{
            .index = @intCast(first_index + i),
            .abi = @intCast(first_abi + @as(i64, @intCast(i))),
            .language = reg,
            .name = try arena.dupeZ(u8, std.mem.span(d.name)),
            .extensions = try exts.toOwnedSlice(arena),
            .splice = @enumFromInt(d.splice),
            .empty_doc_seed = if (d.empty_doc_seed) |s| try arena.dupe(u8, std.mem.span(s)) else null,
            .syntax = if (d.syntax) |s| try syntaxOf(arena, s) else null,
        };
        out.* = e;
    }
    reg.entries = row_entries;

    // The harness, before the rows are published: `Language` reads the
    // registry by index, so the rows are written into their final slots,
    // checked through the same `entryAt` a caller will use — which admits
    // an index below `published` OR one this thread is checking — and
    // published only if every check passes. Nothing else can observe them
    // meanwhile: the mutex is held, and `published` has not moved.
    try registrations.ensureUnusedCapacity(allocator, 1);
    try arenas.ensureUnusedCapacity(allocator, 1);
    for (row_entries, 0..) |e, i| slots[first_index + i] = e;
    checking = .{ .from = first_index, .to = first_index + vt.dialect_count };
    defer checking = null;
    checkRegistration(allocator, reg) catch |err| {
        if (err == error.OutOfMemory) return error.OutOfMemory;
        return error.HarnessFailed;
    };

    // Committed: the arena is the registration's for the life of the
    // process. `deinitAll` (tests only) is the one thing that frees it.
    const keep = try allocator.create(std.heap.ArenaAllocator);
    keep.* = arena_state;
    arenas.appendAssumeCapacity(keep);
    registrations.appendAssumeCapacity(reg);
    published.store(@intCast(first_index + vt.dialect_count), .release);
    return row_entries[0].abi;
}

/// The slot range the registering thread is checking, visible to
/// `entryAt` on that thread only.
threadlocal var checking: ?struct { from: usize, to: usize } = null;

/// TESTS ONLY. Drop every registration. A registered integer is otherwise
/// valid for the life of the process, and a host that called this while
/// another thread held a `Language.Type` would be reading freed memory.
pub fn deinitAll() void {
    mutex.lock();
    defer mutex.unlock();
    const allocator = registry_allocator orelse return;
    published.store(0, .release);
    for (arenas.items) |a| {
        a.deinit();
        allocator.destroy(a);
    }
    arenas.deinit(allocator);
    arenas = .empty;
    registrations.deinit(allocator);
    registrations = .empty;
    registry_allocator = null;
}

fn findByNameLocked(name: []const u8) ?*const Entry {
    const n = published.load(.acquire);
    for (slots[0..n]) |e| if (std.mem.eql(u8, e.name, name)) return e;
    return null;
}

/// The entry a `Language.Type` names. Panics on a value never handed out:
/// a `Type` is only ever minted by this registry.
pub fn entryOf(t: Language.Type) *const Entry {
    return entryAt(@intFromEnum(t)) orelse @panic("runtime language index was never registered");
}

pub fn entryAt(index: usize) ?*const Entry {
    if (index < published.load(.acquire)) return slots[index];
    if (checking) |c| if (index >= c.from and index < c.to) return slots[index];
    return null;
}

/// The entry an ABI integer names, or null for one below the runtime base
/// or never handed out.
pub fn entryByAbi(abi: c_int) ?*const Entry {
    if (abi < Languages.runtime_abi_base) return null;
    return entryAt(@intCast(abi - Languages.runtime_abi_base));
}

/// The entry named `name` — a dialect row's name — or null.
pub fn entryByName(name: []const u8) ?*const Entry {
    mutex.lock();
    defer mutex.unlock();
    return findByNameLocked(name);
}

/// How many dialect rows are registered.
pub fn count() usize {
    return published.load(.acquire);
}

// ── validation ─────────────────────────────────────────────────────────────

/// `Language.validate`, at runtime, over a record rather than a type. The
/// rules are the same rules; where the comptime check reads `@hasDecl`
/// this reads a pointer for null. Fills `last_refusal` and returns false
/// on the first rule broken.
pub fn validateVTable(vt: *const VTable) bool {
    if (vt.version != vtable_version) {
        refuse("vtable version {d} is not the {d} this fig speaks", .{ vt.version, vtable_version });
        return false;
    }
    const name = std.mem.span(vt.name);
    if (name.len == 0) {
        refuse("a language must have a name", .{});
        return false;
    }
    if (vt.caps & cap_read == 0) {
        refuse("'{s}' declares no read capability; a language must at least parse", .{name});
        return false;
    }
    if (vt.max_mapping_depth > std.math.maxInt(u8)) {
        refuse("'{s}' declares max_mapping_depth {d}; a limit is at most 255, and FIG_DEPTH_NONE is no limit", .{ name, vt.max_mapping_depth });
        return false;
    }
    if (vt.dialect_count == 0) {
        refuse("'{s}' declares no dialects, so no format could name it", .{name});
        return false;
    }
    if (!std.mem.eql(u8, std.mem.span(vt.dialects[0].name), name)) {
        refuse("'{s}': the first dialect row must be named after the language, and is '{s}'", .{ name, std.mem.span(vt.dialects[0].name) });
        return false;
    }
    for (vt.dialects[0..vt.dialect_count], 0..) |d, i| {
        const dn = std.mem.span(d.name);
        if (dn.len == 0) {
            refuse("'{s}': dialect row {d} has no name", .{ name, i });
            return false;
        }
        for (vt.dialects[i + 1 .. vt.dialect_count]) |other| {
            if (std.mem.eql(u8, dn, std.mem.span(other.name))) {
                refuse("'{s}' declares two dialects named '{s}'", .{ name, dn });
                return false;
            }
        }
        if (d.splice < 0 or d.splice >= @typeInfo(manifest.SpliceStyle).@"enum".fields.len) {
            refuse("'{s}': dialect '{s}' has an unknown splice style {d}", .{ name, dn, d.splice });
            return false;
        }
    }
    if (vt.sample_count == 0) {
        refuse("'{s}' declares no samples; registration checks a language against what it says it parses", .{name});
        return false;
    }
    if (vt.caps & cap_serialize != 0 and vt.print == null) {
        refuse("'{s}' declares the serialize capability but no print function", .{name});
        return false;
    }
    if (vt.caps & cap_serialize == 0 and vt.lossless != null) {
        refuse("'{s}' declares lossless (an envelope target for serialized output) but no serialize capability", .{name});
        return false;
    }
    const edit = vt.caps & cap_edit != 0;
    if (edit and vt.syntax == null) {
        refuse("'{s}' declares the edit capability and must supply syntax", .{name});
        return false;
    }
    if (!edit) {
        if (vt.render_value != null or vt.render_entry != null or vt.render_item != null or vt.render_tail != null or vt.render_key != null) {
            refuse("'{s}' declares no edit capability but supplies an editing renderer", .{name});
            return false;
        }
        return true;
    }
    // The syntax coherence rules, over the language's syntax and every
    // dialect's own.
    if (!validateSyntax(name, vt, vt.syntax.?)) return false;
    for (vt.dialects[0..vt.dialect_count]) |d| {
        if (d.syntax) |s| if (!validateSyntax(name, vt, s)) return false;
    }
    return true;
}

fn validateSyntax(name: []const u8, vt: *const VTable, s: *const SyntaxDesc) bool {
    if (s.comments.style < 0 or s.comments.style >= @typeInfo(manifest.CommentStyle).@"enum".fields.len) {
        refuse("'{s}': unknown comment style {d}", .{ name, s.comments.style });
        return false;
    }
    if (s.key_style < 0 or s.key_style >= @typeInfo(manifest.KeyStyle).@"enum".fields.len) {
        refuse("'{s}': unknown key style {d}", .{ name, s.key_style });
        return false;
    }
    if (s.section_noun != -1 and (s.section_noun < 0 or s.section_noun >= @typeInfo(manifest.SectionNoun).@"enum".fields.len)) {
        refuse("'{s}': unknown section noun {d}", .{ name, s.section_noun });
        return false;
    }
    if (s.comments.trailing.open != null and s.comments.line.open == null) {
        refuse("'{s}' declares a trailing comment marker but no line comment marker", .{name});
        return false;
    }
    if (s.kv_sep == null and vt.render_entry == null) {
        refuse("'{s}' declares kv_sep = null but no render_entry, so the generic entry-insert paths have no separator to write", .{name});
        return false;
    }
    if (!s.block_seq_editable and vt.render_item != null) {
        refuse("'{s}' declares block_seq_editable = false but supplies render_item, which the engine refuses before reaching", .{name});
        return false;
    }
    if (s.closed_containers.map_open != null and (s.closed_containers.map_close == null or s.closed_containers.seq_open == null or s.closed_containers.seq_close == null)) {
        refuse("'{s}': closed_containers needs all four tokens", .{name});
        return false;
    }
    if (s.section_header.open != null and s.section_header.close == null) {
        refuse("'{s}': section_header needs a close token", .{name});
        return false;
    }
    return true;
}

fn zstr(p: ?[*:0]const u8) ?[]const u8 {
    return if (p) |s| std.mem.span(s) else null;
}

fn dupeZstr(arena: Allocator, p: ?[*:0]const u8) Allocator.Error!?[]const u8 {
    return if (p) |s| try arena.dupe(u8, std.mem.span(s)) else null;
}

fn delimiterOf(arena: Allocator, d: CommentDelimiterDesc) Allocator.Error!?manifest.CommentDelimiter {
    const open = try dupeZstr(arena, d.open) orelse return null;
    return .{
        .open = open,
        .close = try dupeZstr(arena, d.close) orelse "",
        .forbidden = try dupeZstr(arena, d.forbidden),
    };
}

/// `manifest.Syntax` from its C description, every string copied into
/// `arena`, every null-where-required field its Zig default.
fn syntaxOf(arena: Allocator, s: *const SyntaxDesc) Allocator.Error!manifest.Syntax {
    const defaults: manifest.Syntax = .{ .comments = undefined, .kv_sep = null, .empty_map_literal = null };
    var out: manifest.Syntax = .{
        .comments = .{
            .style = @enumFromInt(s.comments.style),
            .line = try delimiterOf(arena, s.comments.line),
            .trailing = try delimiterOf(arena, s.comments.trailing),
        },
        .kv_sep = try dupeZstr(arena, s.kv_sep),
        .flow_kv_sep_from_siblings = s.flow_kv_sep_from_siblings,
        .flow_map_pad = try dupeZstr(arena, s.flow_map_pad) orelse defaults.flow_map_pad,
        .key_style = @enumFromInt(s.key_style),
        .key_sigil = if (s.key_sigil == 0) null else s.key_sigil,
        .empty_map_literal = try dupeZstr(arena, s.empty_map_literal),
        .block_seq_editable = s.block_seq_editable,
        .flow_containers = s.flow_containers,
        .indent_unit = try dupeZstr(arena, s.indent_unit) orelse defaults.indent_unit,
        .seq_item_marker = try dupeZstr(arena, s.seq_item_marker) orelse defaults.seq_item_marker,
        .single_line_block_mapping = s.single_line_block_mapping,
        .bare_document_mapping = s.bare_document_mapping,
        .flow_map_open = try dupeZstr(arena, s.flow_map_open) orelse defaults.flow_map_open,
        .flow_map_close = try dupeZstr(arena, s.flow_map_close) orelse defaults.flow_map_close,
        .structural_indent = s.structural_indent,
        .section_noun = if (s.section_noun == -1) null else @enumFromInt(s.section_noun),
        .merge_key = try dupeZstr(arena, s.merge_key),
    };
    if (s.closed_containers.map_open != null) {
        out.closed_containers = .{
            .map = .{ .open = (try dupeZstr(arena, s.closed_containers.map_open)).?, .close = (try dupeZstr(arena, s.closed_containers.map_close)).? },
            .seq = .{ .open = (try dupeZstr(arena, s.closed_containers.seq_open)).?, .close = (try dupeZstr(arena, s.closed_containers.seq_close)).? },
        };
    }
    if (s.section_header.open != null) {
        const h = s.section_header;
        out.section_header = .{
            .open = (try dupeZstr(arena, h.open)).?,
            .close = (try dupeZstr(arena, h.close)).?,
            .seq_open = try dupeZstr(arena, h.seq_open),
            .seq_close = try dupeZstr(arena, h.seq_close),
            .sep = try dupeZstr(arena, h.sep) orelse ".",
            .skip_index = h.skip_index,
        };
    }
    return out;
}

fn nativeKindsOf(d: *const NativeKindsDesc) manifest.NativeKinds {
    var out: manifest.NativeKinds = .{};
    inline for (@typeInfo(NativeKindsDesc).@"struct".fields) |f| {
        @field(out, f.name) = @field(d, f.name);
    }
    return out;
}

// ── the load-time harness ──────────────────────────────────────────────────

/// `harness.zig`'s checks over one registration: every sample parses;
/// prints and reparses to the same tree where the language prints; its
/// regions are well-formed; and, where it edits, an `Editor` constructs
/// over it and a no-op splice is the identity. The rows are in the registry
/// (provisionally) when this runs, so `Language` works as it will for a
/// caller.
fn checkRegistration(allocator: Allocator, reg: *const Registered) !void {
    for (reg.entries) |e| {
        const t = e.typeOf();
        for (reg.samples) |sample| {
            var parser: Language.Parser = .{ .allocator = allocator };
            const first = Language.parse(&parser, sample, t) catch |err| {
                refuse("'{s}': sample does not parse ({s}): {s}", .{ e.name, @errorName(err), parser.lastMessage() });
                return error.HarnessFailed;
            };
            defer first.deinit(allocator);
            expectRegionsWellFormed(first, sample) catch {
                refuse("'{s}': a sample's regions table is malformed", .{e.name});
                return error.HarnessFailed;
            };
            if (reg.caps.serialize) {
                var out: std.Io.Writer.Allocating = .init(allocator);
                defer out.deinit();
                printWith(e, &out.writer, &first.ast, .{}) catch |err| {
                    refuse("'{s}': sample does not print ({s})", .{ e.name, @errorName(err) });
                    return error.HarnessFailed;
                };
                const printed = out.written();
                const second = Language.parse(&parser, printed, t) catch |err| {
                    refuse("'{s}': printed sample does not reparse ({s}): {s}", .{ e.name, @errorName(err), parser.lastMessage() });
                    return error.HarnessFailed;
                };
                defer second.deinit(allocator);
                // Positional: a table's ids are its pre-order, so the same
                // tree reparsed is the same rows in the same order — which
                // is not true of every compiled parser, and why
                // `harness.zig` compares canonical text instead. (The
                // canonical printer is a build option, not a given.)
                if (!first.ast.eql(second.ast) or !first.ast.commentsEql(second.ast)) {
                    refuse("'{s}': print then reparse changed a sample's tree", .{e.name});
                    return error.HarnessFailed;
                }
            }
            if (reg.caps.edit) {
                var ed: editor.Editor(Language) = .{ .allocator = allocator, .format = t };
                defer ed.deinit();
                ed.init(sample) catch |err| {
                    refuse("'{s}': Editor does not construct over a sample ({s})", .{ e.name, @errorName(err) });
                    return error.HarnessFailed;
                };
                ed.replaceAtSpan(Span.init(0, 0), "") catch |err| {
                    refuse("'{s}': a no-op splice fails ({s})", .{ e.name, @errorName(err) });
                    return error.HarnessFailed;
                };
                if (!std.mem.eql(u8, sample, ed.source.items)) {
                    refuse("'{s}': a no-op splice changed the source", .{e.name});
                    return error.HarnessFailed;
                }
            }
        }
    }
}

fn expectRegionsWellFormed(doc: Document, source: []const u8) !void {
    var prev: ?Document.NodeRegion = null;
    for (doc.node_regions) |r| {
        if (r.node_id >= doc.ast.nodes.len) return error.MalformedRegion;
        const kind = doc.ast.nodes[r.node_id].kind;
        if (kind != .mapping and kind != .sequence) return error.MalformedRegion;
        if (!(r.start < r.end and r.end <= source.len)) return error.MalformedRegion;
        if (!(r.start == 0 or source[r.start - 1] == '\n')) return error.MalformedRegion;
        if (!(r.end == source.len or source[r.end - 1] == '\n')) return error.MalformedRegion;
        if (prev) |p| {
            if (!(p.node_id < r.node_id or (p.node_id == r.node_id and p.start < r.start))) return error.MalformedRegion;
        }
        prev = r;
    }
}

// ============================================================================
// TABLE ↔ DOCUMENT
// ============================================================================

pub const TableError = error{
    /// A row's `kind` or `ext_kind` is not a value core knows.
    UnknownKind,
    /// A parent index past the table, a child before its parent, a keyvalue
    /// without exactly a key and a value, a mapping child that is not a
    /// keyvalue, a scalar with children, a bool whose text is not
    /// `true`/`false`, a row with no span, a second root.
    MalformedTable,
    OutOfMemory,
};

/// A `Document` over `source` from a parse's table. Every string is copied
/// into the AST; the table is not retained.
pub fn tableToDocument(allocator: Allocator, source: []const u8, table: *const NodeTable) TableError!Document {
    const rows = table.rowSlice();
    var ast: AST = .{ .allocator = allocator, .root = 0, .nodes = &.{} };
    var owned: std.ArrayList([]const u8) = .empty;
    errdefer {
        for (owned.items) |s| allocator.free(s);
        owned.deinit(allocator);
    }
    if (rows.len == 0) {
        // The empty document: no root. `Document.ast.root` must name a node,
        // so an empty table is spelled as one null row by the format; a
        // table with no rows at all is refused as the ambiguity it is.
        return error.MalformedTable;
    }
    if (rows.len > std.math.maxInt(u32) - 1) return error.MalformedTable;

    const nodes = try allocator.alloc(Node, rows.len);
    errdefer allocator.free(nodes);
    const spans = try allocator.alloc(Span, rows.len);
    errdefer allocator.free(spans);
    // The AST's side tables are assigned as they are built; on any later
    // failure each is freed here (an unassigned one is the empty slice,
    // which frees as nothing).
    errdefer {
        allocator.free(ast.node_anchors);
        allocator.free(ast.anchors);
        allocator.free(ast.node_tags);
        allocator.free(ast.tag_directives);
        for (ast.node_comments) |x| {
            allocator.free(x.leading);
            allocator.free(x.dangling);
        }
        allocator.free(ast.node_comments);
    }

    // Pass 1: kinds, text, spans, and the parent check — a parent precedes
    // its child (pre-order) and only row 0 has none.
    var first_child = try allocator.alloc(?u32, rows.len);
    defer allocator.free(first_child);
    var last_child = try allocator.alloc(?u32, rows.len);
    defer allocator.free(last_child);
    @memset(first_child, null);
    @memset(last_child, null);

    var any_anchor = false;
    var any_tag = false;
    var any_marker = false;
    var any_sep = false;
    for (rows, 0..) |r, i| {
        if (i == 0) {
            if (r.parent != no_node) return error.MalformedTable;
        } else {
            if (r.parent == no_node or r.parent >= i) return error.MalformedTable;
        }
        spans[i] = r.span.span() orelse return error.MalformedTable;
        if (r.anchor.slice() != null) any_anchor = true;
        if (r.tag.slice() != null) any_tag = true;
        if (r.marker.span() != null) any_marker = true;
        if (r.sep.span() != null) any_sep = true;

        const kind: Node.Kind = blk: {
            if (r.ext_kind != no_ext_kind) {
                if (r.ext_kind < 0 or r.ext_kind >= @typeInfo(ExtKind).@"enum".fields.len) return error.UnknownKind;
                const ek: ExtKind = @enumFromInt(r.ext_kind);
                break :blk .{ .extended = .{ .kind = ek, .text = try own(allocator, &owned, r.text.slice() orelse "") } };
            }
            const rk: RowKind = @enumFromInt(r.kind);
            break :blk switch (rk) {
                .null => .null_,
                .bool => b: {
                    const t = r.text.slice() orelse return error.MalformedTable;
                    if (std.mem.eql(u8, t, "true")) break :b .{ .boolean = true };
                    if (std.mem.eql(u8, t, "false")) break :b .{ .boolean = false };
                    return error.MalformedTable;
                },
                .int => .{ .number = .{ .raw = try own(allocator, &owned, r.text.slice() orelse return error.MalformedTable), .kind = .integer } },
                .float => .{ .number = .{ .raw = try own(allocator, &owned, r.text.slice() orelse return error.MalformedTable), .kind = .float } },
                .string => .{ .string = try own(allocator, &owned, r.text.slice() orelse "") },
                .sequence => .{ .sequence = null },
                .mapping => .{ .mapping = null },
                .keyvalue => .{ .keyvalue = .{ .key = 0, .value = 0 } },
                .alias => .{ .alias = try own(allocator, &owned, r.text.slice() orelse return error.MalformedTable) },
                _ => return error.UnknownKind,
            };
        };
        nodes[i] = .{ .id = @intCast(i), .kind = kind, .next_sibling = null };
    }

    // Pass 2: children. A container's children are linked as siblings in
    // row order; a keyvalue takes exactly two, its key then its value.
    for (rows, 0..) |r, i| {
        if (i == 0) continue;
        const p: usize = r.parent;
        switch (nodes[p].kind) {
            .sequence => {
                if (nodes[i].kind == .keyvalue) return error.MalformedTable;
            },
            .mapping => {
                if (nodes[i].kind != .keyvalue) return error.MalformedTable;
            },
            .keyvalue => {
                if (nodes[i].kind == .keyvalue) return error.MalformedTable;
            },
            else => return error.MalformedTable,
        }
        if (last_child[p]) |lc| {
            nodes[lc].next_sibling = @intCast(i);
        } else {
            first_child[p] = @intCast(i);
        }
        last_child[p] = @intCast(i);
    }
    for (nodes, 0..) |*n, i| {
        switch (n.kind) {
            .sequence => n.kind = .{ .sequence = first_child[i] },
            .mapping => n.kind = .{ .mapping = first_child[i] },
            .keyvalue => {
                const k = first_child[i] orelse return error.MalformedTable;
                const v = nodes[k].next_sibling orelse return error.MalformedTable;
                if (nodes[v].next_sibling != null) return error.MalformedTable;
                // A keyvalue's key and value are not siblings in the AST:
                // the entry names them and the sibling chain is the
                // mapping's, over its keyvalues.
                nodes[k].next_sibling = null;
                n.kind = .{ .keyvalue = .{ .key = k, .value = v } };
            },
            else => {
                if (first_child[i] != null) return error.MalformedTable;
            },
        }
    }

    // Side tables.
    var anchor_spans: []const ?Span = &.{};
    errdefer allocator.free(anchor_spans);
    var tag_spans: []const ?Span = &.{};
    errdefer allocator.free(tag_spans);
    if (any_anchor) {
        const names = try allocator.alloc(?[]const u8, rows.len);
        ast.node_anchors = names;
        var anchors: std.ArrayList(AST.Anchor) = .empty;
        errdefer anchors.deinit(allocator);
        const aspans = try allocator.alloc(?Span, rows.len);
        anchor_spans = aspans;
        for (rows, 0..) |r, i| {
            names[i] = null;
            aspans[i] = r.anchor_span.span();
            if (r.anchor.slice()) |a| {
                const name = try own(allocator, &owned, a);
                names[i] = name;
                try anchors.append(allocator, .{ .name = name, .node = @intCast(i) });
            }
        }
        ast.anchors = try anchors.toOwnedSlice(allocator);
    }
    if (any_tag) {
        const tags = try allocator.alloc(?AST.Tag, rows.len);
        ast.node_tags = tags;
        const tspans = try allocator.alloc(?Span, rows.len);
        tag_spans = tspans;
        for (rows, 0..) |r, i| {
            tags[i] = if (r.tag.slice()) |t| try tagOfText(allocator, &owned, t) else null;
            tspans[i] = r.tag_span.span();
        }
    }
    var markers: std.ArrayList(Document.SpanEntry) = .empty;
    defer markers.deinit(allocator);
    var seps: std.ArrayList(Document.SpanEntry) = .empty;
    defer seps.deinit(allocator);
    if (any_marker or any_sep) {
        for (rows, 0..) |r, i| {
            if (r.marker.span()) |m| try markers.append(allocator, .{ .node_id = @intCast(i), .span = m });
            if (r.sep.span()) |s| try seps.append(allocator, .{ .node_id = @intCast(i), .span = s });
        }
    }
    const marker_spans = try Document.buildSpanTable(allocator, rows.len, markers.items);
    errdefer allocator.free(marker_spans);
    const sep_spans = try Document.buildSpanTable(allocator, rows.len, seps.items);
    errdefer allocator.free(sep_spans);

    const regions = try allocator.alloc(Document.NodeRegion, table.region_count);
    errdefer allocator.free(regions);
    for (table.regionSlice(), regions) |r, *out| {
        if (r.node >= rows.len) return error.MalformedTable;
        out.* = .{ .node_id = r.node, .start = r.start, .end = r.end };
    }
    Document.sortRegions(regions);
    const mentions = try allocator.alloc(Document.NodeMention, table.mention_count);
    errdefer allocator.free(mentions);
    for (table.mentionSlice(), mentions) |m, *out| {
        if (m.node >= rows.len) return error.MalformedTable;
        out.* = .{
            .node_id = m.node,
            .span = m.span.span() orelse return error.MalformedTable,
            .kind = switch (m.kind) {
                mention_header => .header,
                mention_entry => .entry,
                else => return error.MalformedTable,
            },
        };
    }
    Document.sortMentions(mentions);

    // Comments: a per-node table only when any exist, grown from the flat
    // rows. The runs are built in row order, which is the source order the
    // AST asks for.
    if (table.comment_count > 0) {
        const nc = try allocator.alloc(AST.NodeComments, rows.len);
        @memset(nc, .{});
        ast.node_comments = nc;
        var leading = try allocator.alloc(std.ArrayList(AST.Comment), rows.len);
        defer allocator.free(leading);
        var dangling = try allocator.alloc(std.ArrayList(AST.Comment), rows.len);
        defer allocator.free(dangling);
        @memset(leading, .empty);
        @memset(dangling, .empty);
        defer for (leading) |*l| l.deinit(allocator);
        defer for (dangling) |*d| d.deinit(allocator);
        for (table.commentSlice()) |c| {
            if (c.node >= rows.len) return error.MalformedTable;
            const comment: AST.Comment = .{
                .text = try own(allocator, &owned, c.text.slice() orelse ""),
                .style = switch (c.style) {
                    0 => .line,
                    1 => .block,
                    else => return error.MalformedTable,
                },
            };
            switch (c.slot) {
                comment_leading => try leading[c.node].append(allocator, comment),
                comment_trailing => {
                    if (nc[c.node].trailing != null) return error.MalformedTable;
                    nc[c.node].trailing = comment;
                },
                comment_dangling => try dangling[c.node].append(allocator, comment),
                else => return error.MalformedTable,
            }
        }
        for (nc, 0..) |*x, i| {
            x.leading = try leading[i].toOwnedSlice(allocator);
            x.dangling = try dangling[i].toOwnedSlice(allocator);
        }
    }

    const directive_rows = table.directiveSlice();
    if (directive_rows.len > 0) {
        const directives = try allocator.alloc(AST.TagDirective, directive_rows.len);
        errdefer allocator.free(directives);
        for (directive_rows, directives) |r, *d| {
            d.* = .{
                .handle = try own(allocator, &owned, r.handle.slice() orelse return error.MalformedTable),
                .prefix = try own(allocator, &owned, r.prefix.slice() orelse return error.MalformedTable),
            };
        }
        ast.tag_directives = directives;
    }

    ast.nodes = nodes;
    ast.owned_strings = try owned.toOwnedSlice(allocator);
    return .{
        .source = source,
        .ast = ast,
        .node_spans = spans,
        .node_anchor_spans = anchor_spans,
        .node_tag_spans = tag_spans,
        .node_marker_spans = marker_spans,
        .node_sep_spans = sep_spans,
        .node_regions = regions,
        .node_mentions = mentions,
    };
}

/// A tag as the wire spells it, decoded to what `tableFromDocument` encoded:
/// the seven core-schema spellings (`!!int`, `!!str`, …) are *kind* tags —
/// a runtime language's `: int =` is the same type assertion a compiled
/// fig's is, and the printers honour only that form — and anything else is
/// a verbatim text tag, kept for the format that can spell it.
fn tagOfText(allocator: Allocator, owned: *std.ArrayList([]const u8), t: []const u8) Allocator.Error!AST.Tag {
    const kinds = [_]struct { text: []const u8, kind: AST.Tag.KindTag }{
        .{ .text = "!!null", .kind = .null_ },
        .{ .text = "!!bool", .kind = .boolean },
        .{ .text = "!!str", .kind = .string },
        .{ .text = "!!int", .kind = .integer },
        .{ .text = "!!float", .kind = .float },
        .{ .text = "!!seq", .kind = .sequence },
        .{ .text = "!!map", .kind = .mapping },
    };
    for (kinds) |k| if (std.mem.eql(u8, t, k.text)) return .{ .kind = k.kind };
    return .{ .text = try own(allocator, owned, t) };
}

fn own(allocator: Allocator, owned: *std.ArrayList([]const u8), s: []const u8) Allocator.Error![]const u8 {
    const copy = try allocator.dupe(u8, s);
    errdefer allocator.free(copy);
    try owned.append(allocator, copy);
    return copy;
}

/// The table a `print` receives: `ast` from `root` in pre-order, strings
/// borrowed from the AST, spans none. The document's tag directives ride
/// along only when `root` is the document's own root: a fragment printed
/// for a splice has no directives prefix, as a compiled printer's
/// `printNode` writes none. Allocated in `arena`; nothing to free but the
/// arena.
pub fn documentToTable(arena: Allocator, ast: *const AST, root: Node.Id) Allocator.Error!Table {
    var rows: std.ArrayList(NodeRow) = .empty;
    var comments: std.ArrayList(CommentRow) = .empty;
    var strings: std.ArrayList([]const u8) = .empty;
    try appendRows(arena, ast, root, no_node, &rows, &comments, &strings);
    const row_slice = try rows.toOwnedSlice(arena);
    const comment_slice = try comments.toOwnedSlice(arena);
    var directives: std.ArrayList(DirectiveRow) = .empty;
    if (root == ast.root) {
        for (ast.tag_directives) |d| try directives.append(arena, .{ .handle = Str.of(d.handle), .prefix = Str.of(d.prefix) });
    }
    const directive_slice = try directives.toOwnedSlice(arena);
    return .{
        .table = .{
            .rows = row_slice.ptr,
            .row_count = row_slice.len,
            .comments = if (comment_slice.len == 0) null else comment_slice.ptr,
            .comment_count = comment_slice.len,
            .directives = if (directive_slice.len == 0) null else directive_slice.ptr,
            .directive_count = directive_slice.len,
        },
        .strings = try strings.toOwnedSlice(arena),
    };
}

/// A built table and the strings minted for it (kind-tag spellings), which
/// live in the arena `documentToTable` was given.
pub const Table = struct { table: NodeTable, strings: []const []const u8 };

/// The table a `parse` of `doc`'s source would have to answer with: rows
/// as `documentToTable` builds them, plus every source-coupled column the
/// document records — node spans, item markers, entry separators, anchor
/// and tag spans, header regions and name mentions — and the document's
/// tag directives. What a twin of a compiled format is held to, row for
/// row.
pub fn fullTable(arena: Allocator, doc: *const Document) Allocator.Error!Table {
    const ast = &doc.ast;
    var built = try documentToTable(arena, ast, ast.root);
    const rows = @constCast(built.table.rows.?[0..built.table.row_count]);
    // Pre-order row i is node `ids[i]`; the walk is the one `appendRows`
    // makes.
    var ids: std.ArrayList(Node.Id) = .empty;
    try preorder(arena, ast, ast.root, &ids);
    var regions: std.ArrayList(RegionRow) = .empty;
    var mentions: std.ArrayList(MentionRow) = .empty;
    for (ids.items, rows, 0..) |id, *row, i| {
        const node = ast.nodes[id];
        row.span = CSpan.of(doc.span(node));
        if (doc.markerSpan(node)) |sp| row.marker = CSpan.of(sp);
        if (doc.sepSpan(node)) |sp| row.sep = CSpan.of(sp);
        if (doc.anchorSpan(node)) |sp| row.anchor_span = CSpan.of(sp);
        if (doc.tagSpan(node)) |sp| row.tag_span = CSpan.of(sp);
        for (doc.regionsOf(id)) |r| try regions.append(arena, .{ .node = @intCast(i), .start = r.start, .end = r.end });
        for (doc.mentionsOf(id)) |m| try mentions.append(arena, .{
            .node = @intCast(i),
            .span = CSpan.of(m.span),
            .kind = if (m.kind == .header) mention_header else mention_entry,
        });
    }
    const region_slice = try regions.toOwnedSlice(arena);
    const mention_slice = try mentions.toOwnedSlice(arena);
    built.table.regions = if (region_slice.len == 0) null else region_slice.ptr;
    built.table.region_count = region_slice.len;
    built.table.mentions = if (mention_slice.len == 0) null else mention_slice.ptr;
    built.table.mention_count = mention_slice.len;
    return built;
}

fn preorder(arena: Allocator, ast: *const AST, id: Node.Id, out: *std.ArrayList(Node.Id)) Allocator.Error!void {
    try out.append(arena, id);
    switch (ast.nodes[id].kind) {
        .sequence, .mapping => |first| {
            var next = first;
            while (next) |child| {
                try preorder(arena, ast, child, out);
                next = ast.nodes[child].next_sibling;
            }
        },
        .keyvalue => |kv| {
            try preorder(arena, ast, kv.key, out);
            try preorder(arena, ast, kv.value, out);
        },
        else => {},
    }
}

fn appendRows(arena: Allocator, ast: *const AST, id: Node.Id, parent: u32, rows: *std.ArrayList(NodeRow), comments: *std.ArrayList(CommentRow), strings: *std.ArrayList([]const u8)) Allocator.Error!void {
    const node = ast.nodes[id];
    const row_index: u32 = @intCast(rows.items.len);
    var row: NodeRow = .{ .kind = 0, .parent = parent, .span = .none };
    switch (node.kind) {
        .null_ => row.kind = @intFromEnum(RowKind.null),
        .boolean => |b| {
            row.kind = @intFromEnum(RowKind.bool);
            row.text = Str.of(if (b) "true" else "false");
        },
        .string => |s| {
            row.kind = @intFromEnum(RowKind.string);
            row.text = Str.of(s);
        },
        .number => |n| {
            row.kind = @intFromEnum(if (n.kind == .float) RowKind.float else RowKind.int);
            row.text = Str.of(n.raw);
        },
        .extended => |e| {
            row.kind = @intFromEnum(if (e.kind == .char_literal) RowKind.int else RowKind.string);
            row.ext_kind = @intFromEnum(e.kind);
            row.text = Str.of(e.text);
        },
        .sequence => row.kind = @intFromEnum(RowKind.sequence),
        .mapping => row.kind = @intFromEnum(RowKind.mapping),
        .keyvalue => row.kind = @intFromEnum(RowKind.keyvalue),
        .alias => |a| {
            row.kind = @intFromEnum(RowKind.alias);
            row.text = Str.of(a);
        },
    }
    if (id < ast.node_anchors.len) {
        if (ast.node_anchors[id]) |a| row.anchor = Str.of(a);
    }
    if (ast.tagOf(id)) |t| {
        row.tag = switch (t) {
            .text => |s| Str.of(s),
            // A cross-format kind tag spelled as YAML's core schema spells
            // it: the one spelling every format that has tags at all can
            // read, and what YAML's own printer writes.
            .kind => |k| blk: {
                const spelled = try arena.dupe(u8, switch (k) {
                    .null_ => "!!null",
                    .boolean => "!!bool",
                    .string => "!!str",
                    .integer => "!!int",
                    .float => "!!float",
                    .sequence => "!!seq",
                    .mapping => "!!map",
                });
                try strings.append(arena, spelled);
                break :blk Str.of(spelled);
            },
        };
    }
    try rows.append(arena, row);
    const nc = ast.comments(id);
    for (nc.leading) |c| try comments.append(arena, .{ .node = row_index, .slot = comment_leading, .style = @intFromEnum(c.style), .text = Str.of(c.text) });
    if (nc.trailing) |c| try comments.append(arena, .{ .node = row_index, .slot = comment_trailing, .style = @intFromEnum(c.style), .text = Str.of(c.text) });
    for (nc.dangling) |c| try comments.append(arena, .{ .node = row_index, .slot = comment_dangling, .style = @intFromEnum(c.style), .text = Str.of(c.text) });

    switch (node.kind) {
        .sequence, .mapping => |first| {
            var next = first;
            while (next) |child| {
                try appendRows(arena, ast, child, row_index, rows, comments, strings);
                next = ast.nodes[child].next_sibling;
            }
        },
        .keyvalue => |kv| {
            try appendRows(arena, ast, kv.key, row_index, rows, comments, strings);
            try appendRows(arena, ast, kv.value, row_index, rows, comments, strings);
        },
        else => {},
    }
}

// ============================================================================
// THE LANGUAGE
// ============================================================================

/// Print `ast` in the dialect `e` names, through the vtable.
pub fn printWith(e: *const Entry, writer: *std.Io.Writer, ast: *const AST, options: AST.SerializeOptions) !void {
    return printNodeWith(e, writer, ast, ast.root, options);
}

/// `printWith` from `root`: the subtree as the whole table, which is what
/// `fig_value_serialize` asks for. A runtime printer is not told it is a
/// fragment; the table's root is its document.
pub fn printNodeWith(e: *const Entry, writer: *std.Io.Writer, ast: *const AST, root: Node.Id, options: AST.SerializeOptions) !void {
    const reg = e.language;
    const print = reg.vt.print orelse return error.FormatDisabled;
    var arena_state = std.heap.ArenaAllocator.init(ast.allocator);
    defer arena_state.deinit();
    const built = try documentToTable(arena_state.allocator(), ast, root);
    const opts: PrintOptions = .{
        .pretty = options.pretty,
        .strip_comments = options.strip_comments,
        .indent = options.indent,
        .width = options.width,
        .splice = options.splice,
    };
    var out: Str = .{};
    var err: ErrorInfo = .empty;
    const rc = print(reg.vt.ctx, e.dialectZ(), &built.table, &opts, &out, &err);
    if (rc != 0) return error.RuntimePrintFailed;
    defer reg.vt.free_bytes(reg.vt.ctx, out);
    try writer.writeAll(out.slice() orelse "");
}

/// The `Language` of `language.zig`, over the registry. Each of its
/// functions reads the entry `t` names; none of its declarations is a fact
/// about any one runtime format, since there may be many, and the
/// per-format facts — `caps`, `extensions`, `syntax` — are read off the
/// entry by the callers that need them (`c_api.zig`, the CLI).
pub const Language = struct {
    /// An index into the registry. Non-exhaustive: the values are minted
    /// by `register`.
    pub const Type = enum(u16) { _ };

    /// Marks this language as resolved at runtime, for the two engine
    /// gates that are otherwise comptime facts of a language's dialect
    /// table: whether it may have sections and whether it may spell a
    /// header. Both are "yes, ask the entry".
    pub const runtime = true;

    pub const Parser = struct {
        allocator: Allocator,
        /// The vtable's diagnostic for the most recent failed parse.
        last_error: ErrorInfo = .empty,

        pub fn lastMessage(self: *const Parser) []const u8 {
            return self.last_error.text();
        }

        /// A `Document` over `input` in the dialect `t` names.
        pub fn parse(allocator: Allocator, input: []const u8, t: Type) !Document {
            var p: Parser = .{ .allocator = allocator };
            return Language.parse(&p, input, t);
        }
    };

    /// Present for the contract; the serializer reaches a runtime format
    /// through `printWith`, since which format is a runtime fact.
    pub const Printer = struct {
        pub fn print(writer: *std.Io.Writer, ast: *const AST, options: AST.SerializeOptions) !void {
            _ = writer;
            _ = ast;
            _ = options;
            return error.FormatDisabled;
        }
        pub fn printNode(writer: *std.Io.Writer, ast: *const AST, node: Node.Id, options: AST.SerializeOptions) !void {
            _ = writer;
            _ = ast;
            _ = node;
            _ = options;
            return error.FormatDisabled;
        }
    };

    pub const default_type: Type = @enumFromInt(0);

    pub fn parse(parser: *Parser, input: []const u8, t: Type) !Document {
        const e = entryOf(t);
        const reg = e.language;
        var table: NodeTable = .{};
        parser.last_error = .empty;
        const rc = reg.vt.parse(reg.vt.ctx, e.dialectZ(), Str.of(input), &table, &parser.last_error);
        if (rc != 0) return error.RuntimeParseFailed;
        defer reg.vt.free_table(reg.vt.ctx, &table);
        return tableToDocument(parser.allocator, input, &table) catch |err| {
            parser.last_error.set(switch (err) {
                error.UnknownKind => "the parser returned a node kind this fig does not know",
                error.MalformedTable => "the parser returned a malformed node table",
                error.OutOfMemory => return error.OutOfMemory,
            });
            return error.RuntimeParseFailed;
        };
    }

    pub const print = Printer.print;
    pub const printNode = Printer.printNode;

    pub const name = "runtime";
    pub const extensions: []const []const u8 = &.{};
    /// The type-level "may": every runtime format is read through this
    /// one `Language`, so the engine is instantiated for every capability
    /// and the entry's own `caps` decide at the call.
    pub const caps: manifest.Caps = .{ .read = true, .edit = true, .serialize = true };

    /// One row, so `validate` sees a dialect table; never in the registry
    /// of `language.zig`, whose rows are compiled formats.
    pub const dialects: []const manifest.Dialect(@This()) = &.{.{
        .name = "runtime",
        .abi_value = Languages.runtime_abi_base,
        .sniff_rank = null,
        .splice = .raw,
        .empty_doc_seed = null,
    }};

    pub fn syntax(t: Type) manifest.Syntax {
        const e = entryOf(t);
        if (e.syntax) |s| return s;
        return e.language.syntax orelse
            // A read-only format has no syntax; the editor over it is
            // refused before it is asked (`c_api.zig`, the CLI), so this is
            // unreachable in practice and a safe default in principle.
            .{ .comments = .{ .style = .hash, .line = null, .trailing = null }, .kv_sep = null, .empty_map_literal = null };
    }

    /// `Editor.hasRenderer`'s runtime answer: whether the entry's vtable
    /// fills the slot.
    pub fn hasRenderer(t: Type, which: manifest.Renderer) bool {
        const vt = &entryOf(t).language.vt;
        return switch (which) {
            .value => vt.render_value != null,
            .entry => vt.render_entry != null,
            .item => vt.render_item != null,
            .tail => vt.render_tail != null,
            .key => vt.render_key != null,
        };
    }

    pub fn renderValue(t: Type, allocator: Allocator, out: *std.ArrayList(u8), value_text: []const u8, literal: manifest.Literal) !void {
        const e = entryOf(t);
        const vt = &e.language.vt;
        var s: Str = .{};
        var err: ErrorInfo = .empty;
        if ((vt.render_value orelse return error.UnsupportedShape)(vt.ctx, e.dialectZ(), Str.of(value_text), @tagName(literal).ptr, &s, &err) != 0) return rendererError(&err);
        defer vt.free_bytes(vt.ctx, s);
        try out.appendSlice(allocator, s.slice() orelse "");
    }

    pub fn renderEntry(t: Type, allocator: Allocator, out: *std.ArrayList(u8), indent: []const u8, key_text: []const u8, value_text: []const u8) !void {
        const e = entryOf(t);
        const vt = &e.language.vt;
        var s: Str = .{};
        var err: ErrorInfo = .empty;
        if ((vt.render_entry orelse return error.UnsupportedShape)(vt.ctx, e.dialectZ(), Str.of(indent), Str.of(key_text), Str.of(value_text), &s, &err) != 0) return rendererError(&err);
        defer vt.free_bytes(vt.ctx, s);
        try out.appendSlice(allocator, s.slice() orelse "");
    }

    pub fn renderItem(t: Type, allocator: Allocator, out: *std.ArrayList(u8), indent: []const u8, value_text: []const u8) !void {
        const e = entryOf(t);
        const vt = &e.language.vt;
        var s: Str = .{};
        var err: ErrorInfo = .empty;
        if ((vt.render_item orelse return error.UnsupportedShape)(vt.ctx, e.dialectZ(), Str.of(indent), Str.of(value_text), &s, &err) != 0) return rendererError(&err);
        defer vt.free_bytes(vt.ctx, s);
        try out.appendSlice(allocator, s.slice() orelse "");
    }

    pub fn renderTail(t: Type, allocator: Allocator, out: *std.ArrayList(u8), indent: []const u8, key_text: []const u8, value_text: []const u8) !void {
        const e = entryOf(t);
        const vt = &e.language.vt;
        var s: Str = .{};
        var err: ErrorInfo = .empty;
        if ((vt.render_tail orelse return error.UnsupportedShape)(vt.ctx, e.dialectZ(), Str.of(indent), Str.of(key_text), Str.of(value_text), &s, &err) != 0) return rendererError(&err);
        defer vt.free_bytes(vt.ctx, s);
        try out.appendSlice(allocator, s.slice() orelse "");
    }

    pub fn renderKey(t: Type, allocator: Allocator, out: *std.ArrayList(u8), indent: []const u8, key_text: []const u8, old_key: []const u8) !void {
        const e = entryOf(t);
        const vt = &e.language.vt;
        var s: Str = .{};
        var err: ErrorInfo = .empty;
        if ((vt.render_key orelse return error.UnsupportedShape)(vt.ctx, e.dialectZ(), Str.of(indent), Str.of(key_text), Str.of(old_key), &s, &err) != 0) return rendererError(&err);
        defer vt.free_bytes(vt.ctx, s);
        try out.appendSlice(allocator, s.slice() orelse "");
    }

    /// A renderer's refusal: the format saying the text has no spelling
    /// here — a key that needs a form the slot cannot take, a null where
    /// the format has none. The helper's own words are kept in
    /// `last_refusal`, for a caller that reports them (`lastRefusal`).
    fn rendererError(err: *const ErrorInfo) error{RendererRefused} {
        const text = err.text();
        refuse("{s}", .{if (text.len > 0) text else "the renderer declined"});
        return error.RendererRefused;
    }
};

// ============================================================================
// TESTS
// ============================================================================

const testing = std.testing;

test "a null-row table is the empty document and an empty one is refused" {
    var table: NodeTable = .{};
    try testing.expectError(error.MalformedTable, tableToDocument(testing.allocator, "", &table));
    const rows = [_]NodeRow{.{ .kind = @intFromEnum(RowKind.null), .parent = no_node, .span = .{ .start = 0, .end = 0 } }};
    table = .{ .rows = &rows, .row_count = rows.len };
    const doc = try tableToDocument(testing.allocator, "", &table);
    defer doc.deinit(testing.allocator);
    try testing.expect(doc.ast.nodes[doc.ast.root].kind == .null_);
}

test "a mapping table round-trips through Document and back" {
    // { a: 1, b: [true, "x"] } as `a: 1\nb: [true, x]\n`
    const src = "a: 1\nb: [true, x]\n";
    const rows = [_]NodeRow{
        .{ .kind = @intFromEnum(RowKind.mapping), .parent = no_node, .span = .{ .start = 0, .end = src.len } },
        .{ .kind = @intFromEnum(RowKind.keyvalue), .parent = 0, .span = .{ .start = 0, .end = 4 }, .sep = .{ .start = 1, .end = 2 } },
        .{ .kind = @intFromEnum(RowKind.string), .parent = 1, .span = .{ .start = 0, .end = 1 }, .text = Str.of("a") },
        .{ .kind = @intFromEnum(RowKind.int), .parent = 1, .span = .{ .start = 3, .end = 4 }, .text = Str.of("1") },
        .{ .kind = @intFromEnum(RowKind.keyvalue), .parent = 0, .span = .{ .start = 5, .end = 17 }, .sep = .{ .start = 6, .end = 7 } },
        .{ .kind = @intFromEnum(RowKind.string), .parent = 4, .span = .{ .start = 5, .end = 6 }, .text = Str.of("b") },
        .{ .kind = @intFromEnum(RowKind.sequence), .parent = 4, .span = .{ .start = 8, .end = 17 } },
        .{ .kind = @intFromEnum(RowKind.bool), .parent = 6, .span = .{ .start = 9, .end = 13 }, .text = Str.of("true") },
        .{ .kind = @intFromEnum(RowKind.string), .parent = 6, .span = .{ .start = 15, .end = 16 }, .text = Str.of("x") },
    };
    const comments = [_]CommentRow{.{ .node = 2, .slot = comment_leading, .style = 0, .text = Str.of("hello") }};
    const table: NodeTable = .{ .rows = &rows, .row_count = rows.len, .comments = &comments, .comment_count = 1 };
    const doc = try tableToDocument(testing.allocator, src, &table);
    defer doc.deinit(testing.allocator);

    var out: std.Io.Writer.Allocating = .init(testing.allocator);
    defer out.deinit();
    try doc.ast.serializeWith(&out.writer, .json, .{ .pretty = false });
    try testing.expectEqualStrings("{\"a\":1,\"b\":[true,\"x\"]}\n", out.written());
    try testing.expectEqualStrings("hello", doc.ast.comments(2).leading[0].text);
    try testing.expect(doc.sepSpan(doc.ast.nodes[1]) != null);
    try testing.expectEqual(@as(usize, 6), doc.sepSpan(doc.ast.nodes[4]).?.start);

    // And back: the same rows, minus the spans, plus nothing.
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const built = try documentToTable(arena.allocator(), &doc.ast, doc.ast.root);
    const back = built.table.rowSlice();
    try testing.expectEqual(rows.len, back.len);
    for (rows, back) |want, got| {
        try testing.expectEqual(want.kind, got.kind);
        try testing.expectEqual(want.parent, got.parent);
        try testing.expectEqualStrings(want.text.slice() orelse "", got.text.slice() orelse "");
        try testing.expect(got.span.span() == null);
    }
    try testing.expectEqual(@as(usize, 1), built.table.comment_count);

    // `fullTable` is the parse's own answer back: every span, the
    // separators, and the comment, as they were given.
    const full = try fullTable(arena.allocator(), &doc);
    const full_rows = full.table.rowSlice();
    try testing.expectEqual(rows.len, full_rows.len);
    for (rows, full_rows) |want, got| {
        try testing.expectEqual(want.span.start, got.span.start);
        try testing.expectEqual(want.span.end, got.span.end);
        try testing.expectEqual(want.sep.span() == null, got.sep.span() == null);
        if (want.sep.span()) |sp| try testing.expectEqual(sp.start, got.sep.span().?.start);
        try testing.expect(got.marker.span() == null);
    }
    try testing.expectEqual(@as(usize, 0), full.table.region_count);
    try testing.expectEqual(@as(usize, 0), full.table.mention_count);
    try testing.expectEqual(@as(usize, 1), full.table.comment_count);
    try testing.expectEqual(@as(u32, 2), full.table.commentSlice()[0].node);
}

test "a core-schema tag on the wire is a kind tag, and any other a text tag" {
    // The encoder spells a `.kind` tag as YAML's core schema does
    // (`!!int`); reading it back as verbatim text lost the assertion — a
    // runtime fig's `port: int = 5432` reached the printers untagged.
    const src = "a: 1\nb: 2\n";
    const rows = [_]NodeRow{
        .{ .kind = @intFromEnum(RowKind.mapping), .parent = no_node, .span = .{ .start = 0, .end = src.len } },
        .{ .kind = @intFromEnum(RowKind.keyvalue), .parent = 0, .span = .{ .start = 0, .end = 4 } },
        .{ .kind = @intFromEnum(RowKind.string), .parent = 1, .span = .{ .start = 0, .end = 1 }, .text = Str.of("a") },
        .{ .kind = @intFromEnum(RowKind.int), .parent = 1, .span = .{ .start = 3, .end = 4 }, .text = Str.of("1"), .tag = Str.of("!!int") },
        .{ .kind = @intFromEnum(RowKind.keyvalue), .parent = 0, .span = .{ .start = 5, .end = 9 } },
        .{ .kind = @intFromEnum(RowKind.string), .parent = 4, .span = .{ .start = 5, .end = 6 }, .text = Str.of("b") },
        .{ .kind = @intFromEnum(RowKind.int), .parent = 4, .span = .{ .start = 8, .end = 9 }, .text = Str.of("2"), .tag = Str.of("!custom") },
    };
    const table: NodeTable = .{ .rows = &rows, .row_count = rows.len };
    const doc = try tableToDocument(testing.allocator, src, &table);
    defer doc.deinit(testing.allocator);
    try testing.expectEqual(AST.Tag{ .kind = .integer }, doc.ast.tagOf(3).?);
    try testing.expectEqualStrings("!custom", doc.ast.tagOf(6).?.text);

    // And back out as it came in.
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const built = try documentToTable(arena.allocator(), &doc.ast, doc.ast.root);
    const back = built.table.rowSlice();
    try testing.expectEqualStrings("!!int", back[3].tag.slice().?);
    try testing.expectEqualStrings("!custom", back[6].tag.slice().?);
}

test "a tag directive travels with the rows, and only a whole document's print gets it back" {
    // `!e!foo` is legal only in a document declaring `!e!`, so a twin of
    // YAML that read the `%TAG` line has to hand it over for the printer
    // to write back; a fragment (a splice's text) has no directives prefix.
    const src = "%TAG !e! tag:x/\n---\n!e!foo bar\n";
    const rows = [_]NodeRow{
        .{ .kind = @intFromEnum(RowKind.string), .parent = no_node, .span = .{ .start = 20, .end = 29 }, .text = Str.of("bar"), .tag = Str.of("!e!foo") },
    };
    const directives = [_]DirectiveRow{.{ .handle = Str.of("!e!"), .prefix = Str.of("tag:x/") }};
    const table: NodeTable = .{ .rows = &rows, .row_count = rows.len, .directives = &directives, .directive_count = 1 };
    const doc = try tableToDocument(testing.allocator, src, &table);
    defer doc.deinit(testing.allocator);
    try testing.expectEqual(@as(usize, 1), doc.ast.tag_directives.len);
    try testing.expectEqualStrings("!e!", doc.ast.tag_directives[0].handle);
    try testing.expectEqualStrings("tag:x/", doc.ast.tag_directives[0].prefix);

    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const whole = try fullTable(arena.allocator(), &doc);
    try testing.expectEqual(@as(usize, 1), whole.table.directiveSlice().len);
    try testing.expectEqualStrings("tag:x/", whole.table.directiveSlice()[0].prefix.slice().?);

    // A directive with no prefix is not one.
    const bad = [_]DirectiveRow{.{ .handle = Str.of("!e!"), .prefix = .none }};
    const bad_table: NodeTable = .{ .rows = &rows, .row_count = rows.len, .directives = &bad, .directive_count = 1 };
    try testing.expectError(error.MalformedTable, tableToDocument(testing.allocator, src, &bad_table));
}

test "a malformed table is refused, not read" {
    const src = "x";
    // A child before its parent.
    var rows = [_]NodeRow{
        .{ .kind = @intFromEnum(RowKind.sequence), .parent = no_node, .span = .{ .start = 0, .end = 1 } },
        .{ .kind = @intFromEnum(RowKind.string), .parent = 2, .span = .{ .start = 0, .end = 1 }, .text = Str.of("x") },
        .{ .kind = @intFromEnum(RowKind.string), .parent = 0, .span = .{ .start = 0, .end = 1 }, .text = Str.of("x") },
    };
    var table: NodeTable = .{ .rows = &rows, .row_count = rows.len };
    try testing.expectError(error.MalformedTable, tableToDocument(testing.allocator, src, &table));
    // A mapping child that is not a keyvalue.
    rows[0].kind = @intFromEnum(RowKind.mapping);
    rows[1].parent = 0;
    try testing.expectError(error.MalformedTable, tableToDocument(testing.allocator, src, &table));
    // An unknown kind.
    rows[0].kind = 99;
    try testing.expectError(error.UnknownKind, tableToDocument(testing.allocator, src, &table));
    // A row with no span.
    rows[0].kind = @intFromEnum(RowKind.sequence);
    rows[1].span = .none;
    try testing.expectError(error.MalformedTable, tableToDocument(testing.allocator, src, &table));
}

// ── a format in Zig, through the vtable ─────────────────────────────────────
//
// `tinykv`: `key=value` lines, `#` comments, nothing else — enough to drive
// every path above through the C shapes from Zig, which is what a host in
// any language does. The test registers it, parses through `Language`,
// edits through `Editor(Language)`, prints, and then registers two broken
// records to see them refused.

/// TESTS ONLY: `tinykv`, for `c_api.zig`'s tests to register through the
/// exports. Nothing outside a `test` block references it.
pub const test_language = TinyKv;

const TinyKv = struct {
    pub const Alloc = struct { allocator: Allocator };

    fn parse(ctx: ?*anyopaque, dialect: [*:0]const u8, input: Str, out: *NodeTable, err: *ErrorInfo) callconv(.c) c_int {
        _ = dialect;
        const a: *Alloc = @ptrCast(@alignCast(ctx.?));
        const src = input.slice() orelse "";
        var rows: std.ArrayList(NodeRow) = .empty;
        var comments: std.ArrayList(CommentRow) = .empty;
        rows.append(a.allocator, .{ .kind = @intFromEnum(RowKind.mapping), .parent = no_node, .span = .{ .start = 0, .end = src.len } }) catch return 255;
        var pending: std.ArrayList(Str) = .empty;
        defer pending.deinit(a.allocator);
        var at: usize = 0;
        while (at < src.len) {
            const nl = std.mem.indexOfScalarPos(u8, src, at, '\n') orelse src.len;
            const line = src[at..nl];
            if (line.len == 0) {
                at = nl + 1;
                continue;
            }
            if (line[0] == '#') {
                pending.append(a.allocator, Str.of(std.mem.trim(u8, line[1..], " "))) catch return 255;
                at = nl + 1;
                continue;
            }
            const eq = std.mem.indexOfScalar(u8, line, '=') orelse {
                err.set("expected key=value");
                err.byte_offset = at;
                rows.deinit(a.allocator);
                comments.deinit(a.allocator);
                return 2;
            };
            const kv: u32 = @intCast(rows.items.len);
            rows.append(a.allocator, .{ .kind = @intFromEnum(RowKind.keyvalue), .parent = 0, .span = .{ .start = at, .end = nl }, .sep = .{ .start = at + eq, .end = at + eq + 1 } }) catch return 255;
            const key: u32 = @intCast(rows.items.len);
            rows.append(a.allocator, .{ .kind = @intFromEnum(RowKind.string), .parent = kv, .span = .{ .start = at, .end = at + eq }, .text = Str.of(line[0..eq]) }) catch return 255;
            rows.append(a.allocator, .{ .kind = @intFromEnum(RowKind.string), .parent = kv, .span = .{ .start = at + eq + 1, .end = nl }, .text = Str.of(line[eq + 1 ..]) }) catch return 255;
            for (pending.items) |c| comments.append(a.allocator, .{ .node = key, .slot = comment_leading, .style = 0, .text = c }) catch return 255;
            pending.clearRetainingCapacity();
            at = nl + 1;
        }
        const r = rows.toOwnedSlice(a.allocator) catch return 255;
        const c = comments.toOwnedSlice(a.allocator) catch return 255;
        out.* = .{ .rows = r.ptr, .row_count = r.len, .comments = if (c.len == 0) null else c.ptr, .comment_count = c.len };
        return 0;
    }

    fn freeTable(ctx: ?*anyopaque, table: *NodeTable) callconv(.c) void {
        const a: *Alloc = @ptrCast(@alignCast(ctx.?));
        a.allocator.free(table.rowSlice());
        a.allocator.free(table.commentSlice());
    }

    fn print(ctx: ?*anyopaque, dialect: [*:0]const u8, table: *const NodeTable, options: *const PrintOptions, out: *Str, err: *ErrorInfo) callconv(.c) c_int {
        _ = dialect;
        _ = options;
        const a: *Alloc = @ptrCast(@alignCast(ctx.?));
        const rows = table.rowSlice();
        var buf: std.ArrayList(u8) = .empty;
        var i: usize = 1;
        while (i < rows.len) {
            // A mapping is three rows per entry: keyvalue, key, value.
            if (rows[i].kind != @intFromEnum(RowKind.keyvalue) or i + 2 >= rows.len) {
                err.set("tinykv holds a flat string map");
                buf.deinit(a.allocator);
                return 4;
            }
            const key = rows[i + 1];
            const val = rows[i + 2];
            if (key.kind != @intFromEnum(RowKind.string) or val.kind != @intFromEnum(RowKind.string)) {
                err.set("tinykv holds a flat string map");
                buf.deinit(a.allocator);
                return 4;
            }
            for (table.commentSlice()) |c| {
                if (c.node == i + 1 and c.slot == comment_leading) {
                    buf.appendSlice(a.allocator, "# ") catch return 255;
                    buf.appendSlice(a.allocator, c.text.slice() orelse "") catch return 255;
                    buf.append(a.allocator, '\n') catch return 255;
                }
            }
            buf.appendSlice(a.allocator, key.text.slice() orelse "") catch return 255;
            buf.append(a.allocator, '=') catch return 255;
            buf.appendSlice(a.allocator, val.text.slice() orelse "") catch return 255;
            buf.append(a.allocator, '\n') catch return 255;
            i += 3;
        }
        const s = buf.toOwnedSlice(a.allocator) catch return 255;
        out.* = Str.of(s);
        return 0;
    }

    fn freeBytes(ctx: ?*anyopaque, bytes: Str) callconv(.c) void {
        const a: *Alloc = @ptrCast(@alignCast(ctx.?));
        if (bytes.slice()) |s| if (s.len > 0) a.allocator.free(s);
    }

    const syntax: SyntaxDesc = .{
        .comments = .{ .style = @intFromEnum(manifest.CommentStyle.hash), .line = .{ .open = "#" }, .trailing = .{} },
        .kv_sep = "=",
        .empty_map_literal = "{}",
        .flow_containers = false,
    };
    const extensions = [_]?[*:0]const u8{ "tkv", null };
    const dialects = [_]DialectDesc{.{ .name = "tinykv", .extensions = &extensions, .splice = 2, .empty_doc_seed = "" }};
    const samples = [_]Str{ Str.of("a=1\nb=two\n"), Str.of("# top\nk=v\n") };

    pub fn vtable(alloc: *Alloc) VTable {
        return .{
            .version = vtable_version,
            .ctx = alloc,
            .name = "tinykv",
            .caps = cap_read | cap_edit | cap_serialize,
            .max_mapping_depth = 0,
            .syntax = &syntax,
            .dialects = &dialects,
            .dialect_count = dialects.len,
            .samples = &samples,
            .sample_count = samples.len,
            .parse = parse,
            .print = print,
            .free_table = freeTable,
            .free_bytes = freeBytes,
        };
    }
};

test "a language registered from Zig parses, edits and prints through the contract" {
    defer deinitAll();
    var alloc: TinyKv.Alloc = .{ .allocator = testing.allocator };
    const vt = TinyKv.vtable(&alloc);
    const abi = try register(testing.allocator, &vt);
    try testing.expect(abi >= Languages.runtime_abi_base);
    const e = entryByAbi(abi).?;
    try testing.expectEqualStrings("tinykv", e.name);
    try testing.expect(entryByName("tinykv") == e);
    try testing.expect(e.language.caps.edit);
    try testing.expectEqualStrings("tkv", e.extensions[0]);
    try testing.expectEqual(@as(usize, 1), count());

    // Parse through the Language, as the C API and CLI do.
    const doc = try Language.Parser.parse(testing.allocator, "# note\nx=1\ny=2\n", e.typeOf());
    defer doc.deinit(testing.allocator);
    var out: std.Io.Writer.Allocating = .init(testing.allocator);
    defer out.deinit();
    try doc.ast.serializeWith(&out.writer, .json, .{ .pretty = false });
    try testing.expectEqualStrings("{\"x\":\"1\",\"y\":\"2\"}\n", out.written());

    // Edit through the engine: replace, insert, a comment, delete.
    var ed: editor.Editor(Language) = .{ .allocator = testing.allocator, .format = e.typeOf() };
    defer ed.deinit();
    try ed.init("x=1\ny=2\n");
    try ed.replaceValAtPath(&.{.{ .key = "x" }}, "10");
    try ed.set(&.{.{ .key = "z" }}, "3");
    try ed.addLeadingComment(&.{.{ .key = "y" }}, "why");
    try ed.deleteKey(&.{.{ .key = "x" }});
    try testing.expectEqualStrings("# why\ny=2\nz=3\n", ed.source.items);

    // Print through the vtable.
    out.clearRetainingCapacity();
    try printWith(e, &out.writer, &doc.ast, .{});
    try testing.expectEqualStrings("# note\nx=1\ny=2\n", out.written());

    // A parse failure carries the helper's message.
    var parser: Language.Parser = .{ .allocator = testing.allocator };
    try testing.expectError(error.RuntimeParseFailed, Language.parse(&parser, "no equals\n", e.typeOf()));
    try testing.expectEqualStrings("expected key=value", parser.lastMessage());
}

// `tinylist`: `[a, b, c]`, bare words in brackets, nothing else — a FLOW
// format, so that the engine's one runtime-shaped question, whether a
// runtime language's root is a flow container or a section root, is asked
// and answered from Zig.
const TinyList = struct {
    pub const Alloc = TinyKv.Alloc;

    fn parse(ctx: ?*anyopaque, dialect: [*:0]const u8, input: Str, out: *NodeTable, err: *ErrorInfo) callconv(.c) c_int {
        _ = dialect;
        const a: *Alloc = @ptrCast(@alignCast(ctx.?));
        const src = input.slice() orelse "";
        const body = std.mem.trimEnd(u8, src, " \n");
        if (body.len < 2 or body[0] != '[' or body[body.len - 1] != ']') {
            err.set("expected [a, b]");
            return 2;
        }
        var rows: std.ArrayList(NodeRow) = .empty;
        rows.append(a.allocator, .{ .kind = @intFromEnum(RowKind.sequence), .parent = no_node, .span = .{ .start = 0, .end = body.len } }) catch return 255;
        var at: usize = 1;
        while (at < body.len - 1) {
            if (body[at] == ' ' or body[at] == ',') {
                at += 1;
                continue;
            }
            var end = at;
            while (end < body.len - 1 and body[end] != ',' and body[end] != ' ') end += 1;
            rows.append(a.allocator, .{ .kind = @intFromEnum(RowKind.string), .parent = 0, .span = .{ .start = at, .end = end }, .text = Str.of(body[at..end]) }) catch return 255;
            at = end;
        }
        const r = rows.toOwnedSlice(a.allocator) catch return 255;
        out.* = .{ .rows = r.ptr, .row_count = r.len };
        return 0;
    }

    fn freeTable(ctx: ?*anyopaque, table: *NodeTable) callconv(.c) void {
        const a: *Alloc = @ptrCast(@alignCast(ctx.?));
        a.allocator.free(table.rowSlice());
    }

    fn print(ctx: ?*anyopaque, dialect: [*:0]const u8, table: *const NodeTable, options: *const PrintOptions, out: *Str, err: *ErrorInfo) callconv(.c) c_int {
        _ = dialect;
        _ = options;
        const a: *Alloc = @ptrCast(@alignCast(ctx.?));
        const rows = table.rowSlice();
        var buf: std.ArrayList(u8) = .empty;
        buf.append(a.allocator, '[') catch return 255;
        for (rows[1..], 0..) |row, i| {
            if (row.kind != @intFromEnum(RowKind.string)) {
                err.set("tinylist holds a flat list of words");
                buf.deinit(a.allocator);
                return 4;
            }
            if (i > 0) buf.appendSlice(a.allocator, ", ") catch return 255;
            buf.appendSlice(a.allocator, row.text.slice() orelse "") catch return 255;
        }
        buf.appendSlice(a.allocator, "]\n") catch return 255;
        const s = buf.toOwnedSlice(a.allocator) catch return 255;
        out.* = Str.of(s);
        return 0;
    }

    const syntax: SyntaxDesc = .{
        .comments = .{ .style = @intFromEnum(manifest.CommentStyle.hash), .line = .{}, .trailing = .{} },
        // Never written — the format has no mapping — but a null needs a
        // `render_entry` beside it, and the list is what is under test.
        .kv_sep = ": ",
        .empty_map_literal = null,
        .flow_containers = true,
    };
    const extensions = [_]?[*:0]const u8{ "tlist", null };
    const dialects = [_]DialectDesc{.{ .name = "tinylist", .extensions = &extensions, .splice = 0, .empty_doc_seed = "[]\n" }};
    const samples = [_]Str{Str.of("[a, b]\n")};

    pub fn vtable(alloc: *Alloc) VTable {
        return .{
            .version = vtable_version,
            .ctx = alloc,
            .name = "tinylist",
            .caps = cap_read | cap_edit | cap_serialize,
            .max_mapping_depth = 0,
            .syntax = &syntax,
            .dialects = &dialects,
            .dialect_count = dialects.len,
            .samples = &samples,
            .sample_count = samples.len,
            .parse = parse,
            .print = print,
            .free_table = freeTable,
            .free_bytes = TinyKv.freeBytes,
        };
    }
};

test "a runtime language's flow root is edited by comma-aware splice, not as a section root" {
    // `Editor.is_section_format` is "may be" for a runtime language; the
    // dialect's `section_noun` settles it at the call. Before it did, a
    // runtime `[a, b, c]` was taken for a section root and its items were
    // deleted by line — the whole document, for a one-line list.
    defer deinitAll();
    var alloc: TinyList.Alloc = .{ .allocator = testing.allocator };
    const vt = TinyList.vtable(&alloc);
    const abi = try register(testing.allocator, &vt);
    const e = entryByAbi(abi).?;
    var ed: editor.Editor(Language) = .{ .allocator = testing.allocator, .format = e.typeOf() };
    defer ed.deinit();
    try ed.init("[a, b, c]\n");
    try ed.removeSeqItem(&.{}, 1);
    try testing.expectEqualStrings("[a, c]\n", ed.source.items);
    try ed.removeSeqItem(&.{}, 1);
    try testing.expectEqualStrings("[a]\n", ed.source.items);
    try ed.appendToSeq(&.{}, "d");
    try testing.expectEqualStrings("[a, d]\n", ed.source.items);
}

test "registration refuses a record that fails validation or the harness" {
    defer deinitAll();
    var alloc: TinyKv.Alloc = .{ .allocator = testing.allocator };
    var vt = TinyKv.vtable(&alloc);

    vt.version = 99;
    try testing.expectError(error.InvalidLanguage, register(testing.allocator, &vt));
    try testing.expect(std.mem.indexOf(u8, lastRefusal(), "vtable version 99") != null);
    vt.version = vtable_version;

    vt.samples = &.{};
    vt.sample_count = 0;
    try testing.expectError(error.InvalidLanguage, register(testing.allocator, &vt));
    try testing.expect(std.mem.indexOf(u8, lastRefusal(), "no samples") != null);

    const bad = [_]Str{Str.of("not a pair\n")};
    vt.samples = &bad;
    vt.sample_count = 1;
    try testing.expectError(error.HarnessFailed, register(testing.allocator, &vt));
    try testing.expect(std.mem.indexOf(u8, lastRefusal(), "sample does not parse") != null);
    try testing.expectEqual(@as(usize, 0), count());

    vt.samples = &TinyKv.samples;
    vt.sample_count = TinyKv.samples.len;
    _ = try register(testing.allocator, &vt);
    try testing.expectError(error.NameTaken, register(testing.allocator, &vt));

    const taken = [_]DialectDesc{.{ .name = "yaml" }};
    var vt2 = TinyKv.vtable(&alloc);
    vt2.name = "yaml";
    vt2.dialects = &taken;
    try testing.expectError(error.NameTaken, register(testing.allocator, &vt2));
}
