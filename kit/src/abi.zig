//! The C shapes of the language contract: the records a language fills to
//! register (`VTable`, `SyntaxDesc`, `DialectDesc`), the ones fig hands it
//! (`PrintOptions`, `RenderRequest`, `ErrorInfo`), and the bits and limits
//! they carry. The kit's, so that a language built apart from core can fill
//! them; `fig.h` states them under the `Fig` prefix, and core's registry
//! (fig's `src/contract/runtime.zig`) reads them.

const std = @import("std");
const Span = @import("span.zig");
const manifest = @import("manifest.zig");
const AST = @import("ast/ast.zig");
const ExtKind = AST.Node.Kind.Extended.ExtKind;

// ============================================================================
// THE C SHAPES
// ============================================================================
//
// Every struct below is `extern`, and every one is stated in fig.h under the
// `Fig` prefix. A field is added at the END of a struct only, and a field
// never changes meaning. Changing a field's meaning is a bump of
// `vtable_version`; appending one is not, because every struct that can
// grow says how much of itself it carries:
//
//   * A record a LANGUAGE writes and fig reads (`VTable`, `SyntaxDesc`,
//     `DialectDesc` through `VTable.dialect_size`) carries its writer's
//     `sizeof`. fig reads the
//     prefix it covers and takes the declared default for every field past
//     it (`gated`), so a language built against an older header registers
//     on a newer fig unchanged, and one built against a newer header on an
//     older fig is read as far as that fig knows.
//   * A record FIG writes and a language reads (`PrintOptions`,
//     `RenderRequest`, the table's `View`) carries fig's `sizeof`,
//     and a language reads a field only when `size` covers it — the rule
//     `FigError` has always had.
//
// A struct embedded by value in one that can grow (`CommentsDesc` inside
// `SyntaxDesc`) and the node table's rows and side facts (`table.zig`) are
// frozen: what they would gain goes at the end of the record that holds
// them instead. `Str` and `CSpan`
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

/// The node table a `parse` fills and a `print` reads: `table.zig`, the
/// ABI 4 shape.
pub const table = @import("table.zig");
pub const Table = table.Table;

/// The row index that says "no row": the root's `parent`.
pub const no_node: u32 = table.none;

/// `FIG_DEPTH_NONE`: a vtable's `max_mapping_depth` when the format holds
/// mappings to any depth.
pub const no_depth_limit: c_int = -1;

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
        .message = @splat(0),
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

/// Why a printer refused a table, in `ErrorInfo.code` when `print` fails.
/// A language that sets none leaves `unspecified`, and its message is the
/// only reason. Each names a `SerializeError` member, which is what core
/// raises for it.
pub const Refusal = enum(c_int) {
    unspecified = 0,
    null_unsupported = 1,
    non_string_key = 2,
    unrepresentable_root = 3,
    unsupported_value = 4,
    invalid_key = 5,
    non_finite_number = 6,
    unresolved_alias = 7,
    nesting_too_deep = 8,
    unexpected_node_kind = 9,
    _,
};

/// The subset of `AST.SerializeOptions` a printer outside core is told. fig
/// writes it and a language reads it: `size` is fig's `sizeof`, and a
/// language reads a field only when `size` covers it, so a field appended
/// here is not a `vtable_version` bump in either direction.
pub const PrintOptions = extern struct {
    size: u32 = @sizeOf(PrintOptions),
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
    /// Print a container root inline — fig's `{ a = 1 }` / `[a, b]` — as a
    /// value the editor splices after `key = ` must be, since a block
    /// spelling re-reads there as a string: `AST.SerializeOptions.flow`,
    /// which the bindings set for fig alone. A language with no such
    /// distinction ignores it.
    flow: bool = false,
};

/// `manifest.CommentDelimiter`.
pub const CommentDelimiterDesc = extern struct {
    /// Null for "no delimiter" where the field is optional.
    open: ?[*:0]const u8 = null,
    close: ?[*:0]const u8 = null,
    forbidden: ?[*:0]const u8 = null,
};

/// `manifest.Comments`.
pub const CommentsDesc = extern struct {
    line: CommentDelimiterDesc,
    trailing: CommentDelimiterDesc,
    block: CommentDelimiterDesc = .{},
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
/// non-optional takes that field's default. `size` is the writer's
/// `sizeof`; a field past it takes its default here.
pub const SyntaxDesc = extern struct {
    size: u32 = @sizeOf(SyntaxDesc),
    /// `comments.style` is required; `line`/`trailing` may be null.
    comments: CommentsDesc,
    kv_sep: ?[*:0]const u8 = null,
    flow_kv_sep_from_siblings: bool = false,
    flow_map_pad: ?[*:0]const u8 = null,
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
    /// Null for a format with no sections.
    section_noun: ?[*:0]const u8 = null,
    section_header: SectionHeaderDesc = .{},
    merge_key: ?[*:0]const u8 = null,
    /// Null for `,`; never empty.
    flow_entry_sep: ?[*:0]const u8 = null,
    flow_root: bool = false,
    flow_maps_only: bool = false,
    line_prefix_marks: ?[*:0]const u8 = null,
    flow_map_kv_sep: ?[*:0]const u8 = null,
    flow_map_entry_sep: ?[*:0]const u8 = null,
    /// 0 for none.
    line_continuation: u8 = 0,
    /// Null for a format with no empty-sequence spelling.
    empty_seq_literal: ?[*:0]const u8 = null,
    opener_line_comment: bool = false,
};

/// `VTable.lossless`: `manifest.NativeKinds` as bits. `lossless_envelope`
/// says the language takes the `$fig` envelope at all (zero is "no
/// envelope", `Caps.lossless == null`); `native_null` is a null the format
/// holds natively; `nativeExt(k)` is `ExtKind` `k`. A bitmask rather than
/// a struct of booleans because an `ExtKind` added later is a bit a language
/// that predates it leaves clear, not a field that grows a struct it wrote.
pub const lossless_envelope: u32 = 1 << 0;
pub const native_null: u32 = 1 << 1;
pub fn nativeExt(k: ExtKind) u32 {
    return @as(u32, 1) << @intCast(2 + @backingInt(k));
}

comptime {
    // `manifest.NativeKinds` is `null` and then one field per `ExtKind`, in
    // `ExtKind`'s order; the bits above assume it, and the bits run out at
    // 30 kinds.
    const fields = @typeInfo(manifest.NativeKinds).@"struct".field_names;
    const kinds = @typeInfo(ExtKind).@"enum".field_names;
    if (fields.len != kinds.len + 1 or !std.mem.eql(u8, fields[0], "null"))
        @compileError("manifest.NativeKinds is not `null` followed by one field per ExtKind");
    for (kinds, fields[1..]) |k, f| if (!std.mem.eql(u8, k, f))
        @compileError("manifest.NativeKinds field '" ++ f ++ "' sits where ExtKind has '" ++ k ++ "'");
    if (kinds.len > 30) @compileError("VTable.lossless has no bit left for another ExtKind");
}

/// `manifest.Dialect`'s runtime half: what a dialect row of a runtime
/// language states. Sniff rank, deserializability and `--spec`
/// names are not here — a runtime format is never sniffed (proposal §8.1)
/// and the rest is deferred until a format asks.
pub const DialectDesc = extern struct {
    /// The member name; the first row's is the language's, and is what
    /// `fig_format_by_name` resolves.
    name: [*:0]const u8,
    /// NULL-terminated array of extensions, or null for none.
    extensions: ?[*]const ?[*:0]const u8 = null,
    /// `manifest.SpliceStyle` ordinal: 0 literal, 1 escaped, 2 raw.
    splice: c_int = 0,
    /// What `set` seeds a missing file with; null refuses creation.
    empty_doc_seed: ?[*:0]const u8 = null,
    /// This dialect's `syntax` where it differs from the language's, else
    /// null.
    syntax: ?*const SyntaxDesc = null,
    /// `manifest.Prints`. `prints_holds` is its `holds` in `VTable.lossless`'s
    /// bits, with `lossless_envelope` meaning "declared": 0 declares none,
    /// and the conversion warnings fall back to `caps`.
    prints_holds: u32 = 0,
    /// `prints_text_scalars` (typed_scalars = false), `prints_null_at_root`,
    /// `prints_root_container`.
    prints_flags: u32 = 0,
    /// Null for "string".
    prints_char_as: ?[*:0]const u8 = null,
    /// Null for "string".
    prints_number_special_as: ?[*:0]const u8 = null,
    /// 0 always, 1 when pretty-printing, 2 never.
    prints_comments: c_int = 0,
};

pub const prints_text_scalars: u32 = 1 << 0;
pub const prints_null_at_root: u32 = 1 << 1;
pub const prints_root_container: u32 = 1 << 2;

/// The fifth versioned surface: bumped when a field of this struct or of
/// any struct it reaches changes meaning, never for an appended field.
pub const vtable_version: u32 = 3;

/// Fill `out`, a table fig allocated, through its `fig_table_*` calls.
pub const ParseFn = *const fn (ctx: ?*anyopaque, dialect: [*:0]const u8, input: Str, out: *Table, err: *ErrorInfo) callconv(.c) c_int;
/// Print the table `view` shows.
pub const PrintFn = *const fn (ctx: ?*anyopaque, dialect: [*:0]const u8, view: *const table.View, options: *const PrintOptions, out: *Str, err: *ErrorInfo) callconv(.c) c_int;
pub const FreeBytesFn = *const fn (ctx: ?*anyopaque, bytes: Str) callconv(.c) void;
/// Everything a renderer is told — `manifest.RenderRequest` in the C
/// shape, with the dialect riding along. fig writes it and a language
/// reads it, gated by `size` as `PrintOptions` is, so a field appended
/// here is not a `vtable_version` bump. `literal` is a `manifest.Literal` tag name, and
/// is filled for every renderer, though only the value's means anything.
pub const RenderRequest = extern struct {
    /// fig's `sizeof(RenderRequest)`: a renderer reads a field only when
    /// `size` covers it.
    size: u32 = @sizeOf(RenderRequest),
    dialect: [*:0]const u8,
    indent: Str = .{},
    key: Str = .{},
    value: Str = .{},
    literal: [*:0]const u8,
    old_key: Str = .{},
    parent_key: Str = .{},
    parent_tag: Str = .{},
};

/// Every renderer slot's shape: the request in, the fragment out.
pub const RenderFn = *const fn (ctx: ?*anyopaque, request: *const RenderRequest, out: *Str, err: *ErrorInfo) callconv(.c) c_int;
/// Locate the documents of a stream: `out` is an array of `StreamDoc`, its
/// `len` in bytes, which fig frees with `free_bytes`.
pub const SplitStreamFn = *const fn (ctx: ?*anyopaque, dialect: [*:0]const u8, input: Str, out: *Str, err: *ErrorInfo) callconv(.c) c_int;

/// `manifest.StreamDocument`, as a language's `split_stream` writes it.
pub const StreamDoc = extern struct {
    start: usize,
    end: usize,
    /// Opened with a document-start marker.
    marked: bool,
};

/// The contract, as a record. `register` copies every string and array it
/// reaches, so the struct and what it points to may be freed after the
/// call; `ctx` and the function pointers must stay valid for the life of
/// the process, since there is no unregistration.
///
/// Every function returns 0 on success. On failure it returns a nonzero
/// value — `FigStatus` values are the convention — and fills `err`, whose
/// `message` is what a caller sees. The table `parse` fills is fig's, filled
/// through fig's calls, so a helper allocates nothing for it. The bytes
/// `print`, a renderer or `split_stream` hand back are the helper's, and
/// core returns them through `free_bytes` once it has copied what it needs.
pub const VTable = extern struct {
    /// `vtable_version`. A record with any other value is refused.
    version: u32,
    /// The writer's `sizeof(VTable)`. fig reads the prefix it covers and
    /// takes each later field's default (every field after `free_bytes`
    /// has one); a record too short to reach `free_bytes` is refused.
    size: u32 = @sizeOf(VTable),
    /// Passed back to every function; opaque to core.
    ctx: ?*anyopaque = null,

    name: [*:0]const u8,
    /// Bits of `FigCapability`: 1 read, 2 edit, 4 serialize.
    caps: u32,
    /// `Caps.max_mapping_depth`; `no_depth_limit` for unbounded. 0 is a
    /// limit: a flat format holds no mapping inside its root.
    max_mapping_depth: c_int = no_depth_limit,
    /// `Caps.lossless` as bits (`lossless_envelope`, `native_null`,
    /// `nativeExt`); 0 for no envelope.
    lossless: u32 = 0,
    /// Required iff `caps` has the edit bit.
    syntax: ?*const SyntaxDesc = null,
    dialects: [*]const DialectDesc,
    dialect_count: usize,
    /// The stride of `dialects`: the writer's `sizeof(DialectDesc)`, read
    /// as `size` is.
    dialect_size: usize = @sizeOf(DialectDesc),
    /// Required, and at least one: registration runs the harness over
    /// them, and a language that offers none has not shown it parses.
    samples: [*]const Str,
    sample_count: usize,

    parse: ParseFn,
    /// Required iff `caps` has the serialize bit.
    print: ?PrintFn = null,
    free_bytes: FreeBytesFn,

    render_value: ?RenderFn = null,
    render_entry: ?RenderFn = null,
    render_item: ?RenderFn = null,
    render_tail: ?RenderFn = null,
    render_key: ?RenderFn = null,
    render_key_name: ?RenderFn = null,
    /// The language's `splitStream`, or null for a language without
    /// multi-document streams.
    split_stream: ?SplitStreamFn = null,
};

/// `FigCapability` bits, restated so this file is a leaf.
pub const cap_read: u32 = 1 << 0;
pub const cap_edit: u32 = 1 << 1;
pub const cap_serialize: u32 = 1 << 2;
pub const cap_references: u32 = 1 << 3;
/// Every bit this fig knows; a record setting another is refused rather
/// than quietly losing a capability it declares.
pub const cap_known: u32 = cap_read | cap_edit | cap_serialize | cap_references;

/// The bytes of `T` up to the end of its last field without a default —
/// the least a writer's `size` must cover.
pub fn requiredSize(comptime T: type) usize {
    var end: usize = 0;
    const info = @typeInfo(T).@"struct";
    inline for (info.field_names, info.field_types, info.field_attrs) |name, FieldT, attrs| {
        if (attrs.default_value_ptr == null) end = @max(end, @offsetOf(T, name) + @sizeOf(FieldT));
    }
    return end;
}

/// A `T` read from `bytes`, the record as a writer with `sizeof(T) ==
/// bytes.len` laid it out: the prefix this fig's `T` shares with it, and
/// the declared default of every field past what `bytes` covers. The
/// caller has checked `bytes.len >= requiredSize(T)`.
pub fn gated(comptime T: type, bytes: []const u8) T {
    var out: T = undefined;
    const n = @min(bytes.len, @sizeOf(T));
    @memcpy(std.mem.asBytes(&out)[0..n], bytes[0..n]);
    const info = @typeInfo(T).@"struct";
    inline for (info.field_names, info.field_types, info.field_attrs) |name, FieldT, attrs| {
        if (@offsetOf(T, name) + @sizeOf(FieldT) > n) {
            if (attrs.defaultValue(FieldT)) |d| @field(out, name) = d;
        }
    }
    return out;
}

pub fn losslessBits(kinds: ?manifest.NativeKinds) u32 {
    const k = kinds orelse return 0;
    var bits: u32 = lossless_envelope;
    if (k.null) bits |= native_null;
    inline for (@typeInfo(ExtKind).@"enum".field_names) |f_name| {
        if (@field(k, f_name)) bits |= nativeExt(@field(ExtKind, f_name));
    }
    return bits;
}
