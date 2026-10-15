//! C ABI for `fig`
//!
//! This file is the entry point for programs accessing `fig` outside of the Zig language.

const std = @import("std");
const builtin = @import("builtin");
const Document = @import("kit").Document;
const AST = @import("kit").AST;
const Serialize = @import("serialize.zig");
const Span = @import("kit").Span;
const Editor = @import("editor.zig").Editor;
const build_options = @import("build_options");

/// Whether `std.Io.Threaded` fails to compile for this target. On iOS and its
/// siblings Zig 0.17.0's `Threaded` reaches its process spawning, which reads a
/// `NullFile.fd` those targets do not have, so anything that touches
/// `std.Options.debug_io` (the default panic and log handlers) breaks the build.
const threaded_io_broken = builtin.os.tag.isDarwin() and builtin.os.tag != .macos;

/// The panic handler for the library: the default, except where
/// `threaded_io_broken`, where a panic traps without printing.
pub const panic = if (threaded_io_broken)
    std.debug.FullPanic(trapPanic)
else
    std.debug.FullPanic(std.debug.defaultPanic);

fn trapPanic(msg: []const u8, first_trace_addr: ?usize) noreturn {
    @branchHint(.cold);
    _ = msg;
    _ = first_trace_addr;
    @trap();
}

// Cross-format conversion helpers used by `fig_document_serialize`. Both are
// format-agnostic AST passes, always compiled in: `Lossless` for the `$fig`
// envelope, `Materialize` for collapsing a reference layer on the way out of
// a language that declares one (`Caps.references` — see `prepareDocumentAst`).
const Lossless = @import("lossless.zig");
const Materialize = @import("materialize.zig");
const Diagnostics = @import("diagnostics.zig");
/// The language registry: every format is a language registered through
/// `fig_language_register`, its dialects named by the integer handles the
/// registry hands out. The vtable and node-table shapes this file re-exports
/// under their `Fig*` names are its, and so is the one `Language` every
/// format is read, edited and printed through.
const Runtime = @import("contract/runtime.zig");
const manifest = @import("kit").manifest;

// The wasm module's half of the runtime-language carrier: a language
// whose functions are JavaScript, reached through a wasm import. Exported
// by the wasm build alone — it is the TypeScript binding's bridge, not a
// C ABI entry point, and `fig.h` does not declare it. `abi-check` reads
// this file's `export fn` markers, so the export lives in its own file.
// The module is an executable (a reactor); the static library a Rust or C
// program links on wasm is not, and carries no JavaScript import, so it
// builds without the wire's zigjr.
comptime {
    if (builtin.cpu.arch.isWasm() and builtin.output_mode == .Exe) _ = @import("wasm_host.zig");
}

/// Logging for the C ABI build (this file is the static-lib root, so its
/// `std_options` wins). The default `std.log` handler writes to stderr via
/// `std.Io.Threaded`, which does not exist on `wasm32-freestanding` (no posix
/// I/O) — referencing it fails to compile, and does not compile on iOS either
/// (`threaded_io_broken`). A library has no business writing to stderr
/// regardless, so drop logs on those targets and defer to the default elsewhere.
pub const std_options: std.Options = .{ .logFn = figLogFn };

fn figLogFn(
    comptime level: std.log.Level,
    comptime scope: @EnumLiteral(),
    comptime format: []const u8,
    args: anytype,
) void {
    if (builtin.cpu.arch.isWasm() or threaded_io_broken) return;
    std.log.defaultLog(level, scope, format, args);
}

/// Translation of `fig` errors to C ABI.
pub const FigStatus = enum(c_int) {
    ok = 0,
    invalid_argument = 1,
    parse_error = 2,
    out_of_memory = 3,
    unsupported_format = 4,
    not_found = 5,
    /// The operation is not defined for these arguments, though every argument
    /// is individually valid — as distinct from `invalid_argument` (a malformed
    /// call) and `unsupported_format` (a format this build cannot handle).
    /// Returned, for instance, by `fig_editor_uncomment_leading` when the
    /// lines turn out not to be an entry.
    unsupported_operation = 6,
    internal_error = 255,
};

// ==================
// VERSION + CAPABILITIES
// ==================
//
// The stable query surface a host uses to interrogate the linked library before
// trusting it: which version it is, and what a registered format can do. Pure
// functions: no handle, no allocation, safe to call from any thread at any time.

/// Packed library version `(major << 16) | (minor << 8) | patch`. A host can
/// compare this against the `FIG_VERSION_*` macros it compiled with to detect a
/// header/library skew. Sourced from `build.zig` (kept in sync with build.zig.zon).
pub export fn fig_version() u32 {
    return (@as(u32, build_options.version_major) << 16) |
        (@as(u32, build_options.version_minor) << 8) |
        @as(u32, build_options.version_patch);
}

/// Null-terminated semantic version string of the linked library (e.g. "0.0.0").
/// Static storage — the caller must NOT free the returned pointer.
pub export fn fig_version_string() [*:0]const u8 {
    const s = std.fmt.comptimePrint("{d}.{d}.{d}", .{
        build_options.version_major,
        build_options.version_minor,
        build_options.version_patch,
    });
    return s;
}

/// Binary C ABI contract version (see `FIG_ABI_VERSION` in fig.h) — a monotonic
/// counter that moves only on a breaking ABI change, distinct from the marketing
/// version. Sourced from `build.zig`; `zig build abi-check` asserts the fig.h
/// macro matches this, and `semver-check` requires it to increment on any
/// breaking ABI diff.
pub export fn fig_abi_version() u32 {
    return build_options.abi_version;
}

/// Capability bits returned (OR-combined) by `fig_format_capabilities`.
pub const FigCapability = enum(u32) {
    /// `fig_parse` accepts this format.
    read = 1 << 0,
    /// `fig_editor_*` accepts this format.
    edit = 1 << 1,
    /// `fig_*_serialize` can write this format.
    serialize = 1 << 2,
    /// The format has a reference layer — anchors, aliases, merges, tags —
    /// which `fig_document_serialize` collapses when a document leaves it for
    /// a format without one, and keeps when the target has one too.
    references = 1 << 3,
    _,
};

/// Report what `fig` can do with the registered format `format` as a
/// bitmask of `FigCapability` (read | edit | serialize | references): its
/// language's `caps`. A handle never handed out reports 0.
pub export fn fig_format_capabilities(format: c_int) u32 {
    const e = entryOf(format) orelse return 0;
    return capsBits(e.language.caps);
}

/// The name of the dialect `format` names, or null for a handle never
/// handed out. Registry entries live as long as the process.
pub export fn fig_format_name(format: c_int) ?[*:0]const u8 {
    const e = entryOf(format) orelse return null;
    return e.name.ptr;
}

// ==================
// THE KIT'S RULES
// ==================
//
// The kit's rules — numbers, datetimes, bare literals and JSON strings —
// for a runner whose languages are not Zig (`fig.h`, "The kit's rules"). `kit/src/rules.zig` defines them,
// and `fig-kit.wasm` exports the same functions under the same names.

const rules = @import("kit").rules;

pub export fn fig_kit_number_spellable(raw: [*]const u8, len: usize, spelling: u32) bool {
    return rules.numberSpellable(raw, len, spelling);
}

pub export fn fig_kit_number_canonical(raw: [*]const u8, len: usize, out: [*]u8, out_cap: usize) usize {
    return rules.numberCanonical(raw, len, out, out_cap);
}

pub export fn fig_kit_number_non_finite(raw: [*]const u8, len: usize) i32 {
    return rules.numberNonFinite(raw, len);
}

pub export fn fig_kit_datetime_classify(raw: [*]const u8, len: usize, flags: u32) i32 {
    return rules.datetimeClassify(raw, len, flags);
}

pub export fn fig_kit_datetime_valid_date(s: [*]const u8, len: usize) bool {
    return rules.datetimeValidDate(s, len);
}

pub export fn fig_kit_datetime_valid_time(s: [*]const u8, len: usize, flags: u32) bool {
    return rules.datetimeValidTime(s, len, flags);
}

pub export fn fig_kit_datetime_valid_offset(s: [*]const u8, len: usize) bool {
    return rules.datetimeValidOffset(s, len);
}

pub export fn fig_kit_literal_kind(raw: [*]const u8, len: usize) u8 {
    return rules.literalKind(raw, len);
}

pub export fn fig_kit_literal_number(raw: [*]const u8, len: usize) i32 {
    return rules.literalNumber(raw, len);
}

pub export fn fig_kit_json_string_quote(raw: [*]const u8, len: usize, quote: u8, out: [*]u8, out_cap: usize) usize {
    return rules.jsonStringQuote(raw, len, quote, out, out_cap);
}

/// The registered dialect a format handle names, or null for one never
/// handed out. The checked lookup every entry point taking `int format`
/// opens with.
fn entryOf(format: c_int) ?*const Runtime.Entry {
    return Runtime.entryByAbi(format);
}

// ==================
// RUNTIME LANGUAGES
// ==================
//
// A format fig did not compile in, registered by a host as a vtable. The
// shapes are `contract/runtime.zig`'s, stated in fig.h under these names
// and held to it by `zig build abi-check`; the registry, the validation and
// the load-time harness are that file's too. This section is the two entry
// points and the re-exports.

pub const FigLanguageVTable = Runtime.VTable;
pub const FigTable = Runtime.Table;
pub const FigTableView = Runtime.table.View;
pub const FigRow = Runtime.table.Row;
pub const FigText = Runtime.table.Text;
pub const FigSyntax = Runtime.SyntaxDesc;
pub const FigComments = Runtime.CommentsDesc;
pub const FigCommentDelimiter = Runtime.CommentDelimiterDesc;
pub const FigSectionHeader = Runtime.SectionHeaderDesc;
pub const FigClosedContainers = Runtime.ClosedContainersDesc;
pub const FigDialectDesc = Runtime.DialectDesc;
pub const FigPrintOptions = Runtime.PrintOptions;

/// The `version` a `FigLanguageVTable` must carry; fig.h's
/// `FIG_LANGUAGE_VTABLE_VERSION`, held equal by abi-check.
pub export fn fig_language_vtable_version() u32 {
    return Runtime.vtable_version;
}

/// Register a language. On `.ok`, `*out_format` is the format integer of
/// its first dialect row. A row takes an integer at or above
/// `FIG_FORMAT_RUNTIME_BASE`, assigned per process — except a row named
/// after a format compiled out of this build, which takes that format's own
/// integer and stands in for it (`FIG_FORMAT_YAML` for a row named "yaml").
/// So the rows' integers are not consecutive in general; `fig_format_by_name`
/// finds each. Every entry point taking a format accepts them from then on,
/// at the tier the record's `caps` declare.
///
/// The record is validated by the rules a compiled format is held to, and
/// its `samples` are parsed, printed, reparsed and edited before anything is
/// registered; a record that fails either is refused with the reason in
/// `out_err` and registers nothing. `fig_language_register` copies what
/// `vt` points to; `vt->ctx` and the function pointers must stay valid for
/// the life of the process, since a format cannot be unregistered.
///
/// A name already registered — or the name of a format compiled into this
/// build — is refused as `invalid_argument`. Registration takes a lock; every other call on a
/// registered format is lock-free and reads only settled values, so the
/// threading note at the top of fig.h holds.
pub export fn fig_language_register(vt: ?*const FigLanguageVTable, out_format: ?*c_int, out_err: ?*FigError) FigStatus {
    const out = out_format orelse return fillError(out_err, .invalid_argument, "out_format is null");
    out.* = -1;
    const record = vt orelse return fillError(out_err, .invalid_argument, "vtable is null");
    const abi = Runtime.register(activeAllocator(), record) catch |err| return switch (err) {
        error.OutOfMemory => fillError(out_err, .out_of_memory, "out of memory"),
        error.InvalidLanguage, error.HarnessFailed, error.NameTaken => fillError(out_err, .invalid_argument, Runtime.lastRefusal()),
        error.AllocatorMismatch, error.RegistryFull => fillError(out_err, .unsupported_operation, "the language registry cannot take another registration"),
    };
    out.* = abi;
    return .ok;
}

/// The format integer of the dialect named `name` — a compiled format's
/// registry name (`json5`, `yaml`, …) or a registered language's — or -1.
/// The only stable way to persist a runtime format: its integer is assigned
/// per process, its name is not.
pub export fn fig_format_by_name(name: ?[*:0]const u8) c_int {
    const n = std.mem.span(name orelse return -1);
    const e = Runtime.entryByName(n) orelse return -1;
    return e.abi;
}

/// Content sniffing in the caller's order: the format integer of the first
/// of `names` whose parser accepts `input`. `not_found` when none does.
pub export fn fig_detect_order(input: ?[*]const u8, input_len: usize, names: ?[*]const ?[*:0]const u8, count: usize, out_format: ?*c_int) FigStatus {
    const out = out_format orelse return .invalid_argument;
    const bytes: []const u8 = if (input_len == 0) "" else (input orelse return .invalid_argument)[0..input_len];
    const list = if (count == 0) &[_]?[*:0]const u8{} else (names orelse return .invalid_argument)[0..count];
    const allocator = activeAllocator();
    var order = std.ArrayList([]const u8).initCapacity(allocator, list.len) catch return .out_of_memory;
    defer order.deinit(allocator);
    for (list) |n| order.appendAssumeCapacity(std.mem.span(n orelse return .invalid_argument));
    const e = Runtime.detectAmong(allocator, bytes, order.items) orelse return .not_found;
    out.* = e.abi;
    return .ok;
}

// The node table a language's `parse` fills (`languages/table.zig`). Fig
// allocated it; these are how a language writes into it, so no memory
// crosses from a language's allocator to fig's. Every node index names a
// row already added; every list is appended in node order.

/// `n` new rows, each a null with no parent, span or text, to be filled in
/// place; null when out of memory. Valid until the next call on `t`.
pub export fn fig_table_rows(t: ?*FigTable, n: usize) ?[*]FigRow {
    const tbl = t orelse return null;
    const rows = tbl.addRows(n) catch return null;
    return rows.ptr;
}

/// `n` zeroed elements appended to list `which` of `t` (`FigTableList`),
/// returned for the caller to fill in place — the whole of a list a
/// language wrote elsewhere, copied in at once. The string buffer's
/// elements are bytes, so a buffer copied into an empty table keeps every
/// `FigText` in it. Null when `which` is not a list or out of memory; valid
/// until the next call on `t`.
pub export fn fig_table_extend(t: ?*FigTable, which: c_int, n: usize) ?*anyopaque {
    const tbl = t orelse return null;
    const a = tbl.allocator;
    const T = Runtime.table;
    return switch (which) {
        0 => extendList(T.Row, &tbl.rows, a, n),
        1 => extendList(u8, &tbl.strings, a, n),
        2 => extendList(T.Anchor, &tbl.anchors, a, n),
        3 => extendList(T.Tag, &tbl.tags, a, n),
        4 => extendList(T.SpanFact, &tbl.markers, a, n),
        5 => extendList(T.SpanFact, &tbl.seps, a, n),
        6 => extendList(T.ItemSep, &tbl.item_seps, a, n),
        7 => extendList(T.Comment, &tbl.comments, a, n),
        8 => extendList(T.SpanFact, &tbl.regions, a, n),
        9 => extendList(T.Mention, &tbl.mentions, a, n),
        10 => extendList(T.Directive, &tbl.directives, a, n),
        11 => extendList(T.Diagnostic, &tbl.diagnostics, a, n),
        else => null,
    };
}

fn extendList(comptime E: type, list: *std.ArrayList(E), a: std.mem.Allocator, n: usize) ?*anyopaque {
    const out = list.addManyAsSlice(a, n) catch return null;
    @memset(std.mem.sliceAsBytes(out), 0);
    return if (n == 0) @ptrCast(list.items.ptr) else @ptrCast(out.ptr);
}

/// `bytes` copied into the table's string buffer, its `FigText` to `out`.
pub export fn fig_table_string(t: ?*FigTable, bytes: ?[*]const u8, len: usize, out: ?*FigText) FigStatus {
    const tbl = t orelse return .invalid_argument;
    const o = out orelse return .invalid_argument;
    const b: []const u8 = if (len == 0) "" else (bytes orelse return .invalid_argument)[0..len];
    o.* = tbl.addString(b) catch return .out_of_memory;
    return .ok;
}

fn tableNode(t: ?*FigTable, node: u32) ?*FigTable {
    const tbl = t orelse return null;
    return if (node < tbl.rows.items.len) tbl else null;
}

/// An anchor named `name` on `node`, its `&name` at `[start, end)`.
pub export fn fig_table_anchor(t: ?*FigTable, node: u32, start: u32, end: u32, name: FigText) FigStatus {
    const tbl = tableNode(t, node) orelse return .invalid_argument;
    tbl.anchors.append(tbl.allocator, .{ .node = node, .start = start, .end = end, .name = name }) catch return .out_of_memory;
    return .ok;
}

/// A tag on `node`, as written (`text`) and as the language resolved it:
/// `resolved` is FIG_TAG_UNSTATED, _CUSTOM, _NON_SPECIFIC or FIG_TAG_KIND +
/// a core type; `agrees` whether the node's text agrees with it.
pub export fn fig_table_tag(t: ?*FigTable, node: u32, start: u32, end: u32, text: FigText, resolved: u8, agrees: bool) FigStatus {
    const tbl = tableNode(t, node) orelse return .invalid_argument;
    tbl.tags.append(tbl.allocator, .{ .node = node, .start = start, .end = end, .text = text, .resolved = resolved, .agrees = @intFromBool(agrees) }) catch return .out_of_memory;
    return .ok;
}

/// A block-sequence item's marker (`- `) at `[start, end)`.
pub export fn fig_table_marker(t: ?*FigTable, node: u32, start: u32, end: u32) FigStatus {
    const tbl = tableNode(t, node) orelse return .invalid_argument;
    tbl.markers.append(tbl.allocator, .{ .node = node, .start = start, .end = end }) catch return .out_of_memory;
    return .ok;
}

/// A keyvalue's separator at `[start, end)`; zero-width for a value that
/// hangs under a bare key.
pub export fn fig_table_sep(t: ?*FigTable, node: u32, start: u32, end: u32) FigStatus {
    const tbl = tableNode(t, node) orelse return .invalid_argument;
    tbl.seps.append(tbl.allocator, .{ .node = node, .start = start, .end = end }) catch return .out_of_memory;
    return .ok;
}

/// A separated sequence's item separator.
pub export fn fig_table_item_sep(t: ?*FigTable, node: u32, text: FigText) FigStatus {
    const tbl = tableNode(t, node) orelse return .invalid_argument;
    tbl.item_seps.append(tbl.allocator, .{ .node = node, .text = text }) catch return .out_of_memory;
    return .ok;
}

/// A comment on `node`: `slot` FIG_COMMENT_LEADING, _TRAILING or
/// _DANGLING; `style` FIG_COMMENT_LINE or _BLOCK.
pub export fn fig_table_comment(t: ?*FigTable, node: u32, slot: u8, style: u8, text: FigText) FigStatus {
    const tbl = tableNode(t, node) orelse return .invalid_argument;
    if (slot > Runtime.table.comment_dangling or style > Runtime.table.comment_block) return .invalid_argument;
    tbl.comments.append(tbl.allocator, .{ .node = node, .slot = slot, .style = style, .text = text }) catch return .out_of_memory;
    return .ok;
}

/// A whole header line of the section `node`, `end` just past its newline.
pub export fn fig_table_region(t: ?*FigTable, node: u32, start: u32, end: u32) FigStatus {
    const tbl = tableNode(t, node) orelse return .invalid_argument;
    tbl.regions.append(tbl.allocator, .{ .node = node, .start = start, .end = end }) catch return .out_of_memory;
    return .ok;
}

/// A place the section `node`'s name is written, FIG_MENTION_HEADER or
/// FIG_MENTION_ENTRY, or the sequence item `node`'s path, FIG_MENTION_INDEX
/// or FIG_MENTION_APPEND.
pub export fn fig_table_mention(t: ?*FigTable, node: u32, start: u32, end: u32, kind: u8) FigStatus {
    const tbl = tableNode(t, node) orelse return .invalid_argument;
    if (kind > Runtime.table.mention_append) return .invalid_argument;
    tbl.mentions.append(tbl.allocator, .{ .node = node, .start = start, .end = end, .kind = kind }) catch return .out_of_memory;
    return .ok;
}

/// What a parse found: FIG_DIAGNOSTIC_ERROR (a failing parse may leave the
/// errors it found) or FIG_DIAGNOSTIC_WARNING, at `[start, end)` (`end`
/// FIG_ROW_NONE for a point), with a message and a short label.
pub export fn fig_table_diagnostic(t: ?*FigTable, severity: u8, start: u32, end: u32, message: FigText, label: FigText) FigStatus {
    const tbl = t orelse return .invalid_argument;
    if (severity > Runtime.table.diagnostic_warning) return .invalid_argument;
    tbl.diagnostics.append(tbl.allocator, .{ .severity = severity, .start = start, .end = end, .message = message, .label = label }) catch return .out_of_memory;
    return .ok;
}

/// A tag-handle declaration of the document (YAML's `%TAG`).
pub export fn fig_table_directive(t: ?*FigTable, handle: FigText, prefix: FigText) FigStatus {
    const tbl = t orelse return .invalid_argument;
    tbl.directives.append(tbl.allocator, .{ .handle = handle, .prefix = prefix }) catch return .out_of_memory;
    return .ok;
}

/// One document of a stream, as `fig_stream_split` reports it: a byte range
/// of the input and whether it opened with a document-start marker.
pub const FigStreamDoc = extern struct {
    start: usize,
    end: usize,
    marked: bool,
};

/// Locate the documents of a multi-document stream in `format`, each one's
/// byte range to be parsed as a document of its own. `unsupported_operation`
/// for a format without streams; `parse_error` when the language refuses the
/// input. Free the array with `fig_stream_free`.
pub export fn fig_stream_split(input: ?[*]const u8, input_len: usize, format: c_int, out_docs: ?*?[*]FigStreamDoc, out_count: ?*usize, out_err: ?*FigError) FigStatus {
    @setEvalBranchQuota(30_000);
    const docs_out = out_docs orelse return .invalid_argument;
    const count_out = out_count orelse return .invalid_argument;
    docs_out.* = null;
    count_out.* = 0;
    const bytes: []const u8 = if (input_len == 0) "" else (input orelse return .invalid_argument)[0..input_len];
    const allocator = activeAllocator();
    const e = entryOf(format) orelse return .unsupported_format;
    const located = Runtime.Language.splitStream(allocator, bytes, e.typeOf()) catch |err| return switch (err) {
        error.UnsupportedOperation => .unsupported_operation,
        error.OutOfMemory => .out_of_memory,
        else => fillError(out_err, .parse_error, "the language could not split the stream"),
    };
    defer allocator.free(located);
    const out = allocator.alloc(FigStreamDoc, located.len) catch return .out_of_memory;
    for (located, out) |l, *o| o.* = .{ .start = l.start, .end = l.end, .marked = l.explicit };
    docs_out.* = out.ptr;
    count_out.* = out.len;
    return .ok;
}

/// Free what `fig_stream_split` returned.
pub export fn fig_stream_free(docs: ?[*]FigStreamDoc, count: usize) void {
    const p = docs orelse return;
    activeAllocator().free(p[0..count]);
}

fn capsBits(caps: manifest.Caps) u32 {
    var bits: u32 = 0;
    if (caps.read) bits |= @backingInt(FigCapability.read);
    if (caps.edit) bits |= @backingInt(FigCapability.edit);
    if (caps.serialize) bits |= @backingInt(FigCapability.serialize);
    if (caps.references) bits |= @backingInt(FigCapability.references);
    return bits;
}

/// A handle to a `fig` document. (See `DocumentHandle` and `handle.*` declaration in `fig_parse`)
pub const FigDocument = opaque {};
const DocumentHandle = struct {
    allocator: std.mem.Allocator,
    source: []u8,
    document: Document,
    /// The dialect `source` was parsed as. `fig_document_serialize` reads its
    /// `caps.references` to decide whether to collapse a reference layer
    /// before printing.
    entry: *const Runtime.Entry,
    /// Reused across `fig_document_serialize` calls; holds the bytes the most
    /// recent call returned (cleared and refilled each time). Mirrors
    /// `ValueHandle.rendered`.
    rendered: std.Io.Writer.Allocating,
    /// Backs the warnings (and their path strings) the most recent
    /// `fig_document_diagnose` produced; reset (not freed) each call, so the
    /// `path`/`note` bytes a `FigWarning` borrows are valid only until the next
    /// diagnose on this handle or `fig_document_destroy`.
    diag_arena: std.heap.ArenaAllocator,
    /// The warning set the most recent `fig_document_diagnose` computed (stored
    /// in `diag_arena`); `fig_document_warning` indexes into it. Empty until the
    /// first diagnose. Replaced (not appended) each diagnose call.
    diag_warnings: []const Diagnostics.Warning = &.{},
    /// Set on a document `fig_document_resolve_aliases` made, whose AST — and
    /// every string it holds — lives in this arena rather than in a parse of
    /// `source` (which is empty). Destroying the handle frees the arena in
    /// place of `document`.
    arena: ?*std.heap.ArenaAllocator = null,
};

fn activeAllocator() std.mem.Allocator {
    // wasm: no libc, `wasm_allocator` is the only option.
    if (builtin.cpu.arch.isWasm()) return std.heap.wasm_allocator;
    // Android: the shared library is built WITHOUT libc, because Zig ships
    // glibc/musl but not Bionic, so a self-contained (NDK-free) `.so` cannot
    // link one (see `addCApiLibrary` in build.zig). That rules out
    // `c_allocator` (it wraps libc `malloc`), so use the libc-free
    // `smp_allocator`. This stays internally consistent — every fig allocation
    // and its matching `fig_free`/`fig_document_free` route back through here —
    // so the swapped allocator never crosses the ABI boundary.
    if (builtin.abi.isAndroid()) return std.heap.smp_allocator;
    // Everywhere else: the process libc allocator.
    return std.heap.c_allocator;
}

// ==================
// RAW MEMORY (for hosts without a shared allocator)
// ==================
//
// A caller that does not share this library's address space — chiefly the
// WebAssembly bindings — cannot otherwise place input bytes where the API can
// read them, nor read borrowed output without first copying it into a buffer it
// owns. These two entry points expose `activeAllocator()` for exactly that: in
// the wasm build they let JavaScript allocate inside linear memory, write the
// input, hand the pointer to `fig_parse`/`fig_editor_*`/…, then release it.
// Buffers obtained here MUST be released with `fig_free`, passing the same
// length that was requested.

/// Allocate `len` bytes and return a pointer to them, or null on failure / a
/// zero-length request. Bytes are uninitialized. Release with `fig_free`.
pub export fn fig_alloc(len: usize) ?[*]u8 {
    if (len == 0) return null;
    const mem = activeAllocator().alloc(u8, len) catch return null;
    return mem.ptr;
}

/// Release a buffer obtained from `fig_alloc`. `len` must equal the length that
/// was requested. A null pointer or zero length is a no-op.
pub export fn fig_free(ptr: ?[*]u8, len: usize) void {
    const p = ptr orelse return;
    if (len == 0) return;
    activeAllocator().free(p[0..len]);
}

/// Caller-allocated parse diagnostic. Mirrors `FigError` in `include/fig.h`.
/// Caller-allocated + size-versioned for the same reason `FigSerializeOptions`
/// is: the library writes only the fields the caller's `size` covers, so fields
/// may be appended later without breaking an older layout. Caller-owned (no
/// allocation, no handle lifetime) is what lets it carry a message for a failure
/// that happens *before* any document handle exists.
///
/// One definition: `Runtime.ErrorInfo` is this struct, since a runtime
/// language's `parse` fills the same record a C caller reads.
pub const FigError = Runtime.ErrorInfo;

/// Whether the caller-reported `FigError.size` covers `field` (same rule as
/// `optionCovers`). A field past `size` is absent in the caller's layout and
/// must not be written.
fn errCovers(size: u32, comptime field: []const u8) bool {
    const end = @offsetOf(FigError, field) + @sizeOf(@FieldType(FigError, field));
    return size >= end;
}

/// Fill `out_err` (if non-null) with `status` + `message`, then return `status`
/// so a failure path can `return fillError(...)`. Every field is gated on the
/// caller's `size`, so an older/smaller struct receives only the fields it
/// declared. `byte_offset`/`line`/`column` are 0 ("unknown") in this release —
/// surfacing the failing span from each parser is a planned follow-up.
/// Public for `wasm_host.zig`, whose one export reports the same way.
pub fn fillError(out_err: ?*FigError, status: FigStatus, message: []const u8) FigStatus {
    const e = out_err orelse return status;
    const size = e.size;
    if (errCovers(size, "code")) e.code = @backingInt(status);
    if (errCovers(size, "byte_offset")) e.byte_offset = 0;
    if (errCovers(size, "line")) e.line = 0;
    if (errCovers(size, "column")) e.column = 0;
    // `message_len` and `message` are written together, and only when `size`
    // covers the whole inline array — a partially-covered buffer gets nothing
    // rather than a string truncated without its NUL terminator.
    if (errCovers(size, "message")) {
        const n = @min(message.len, e.message.len - 1);
        @memcpy(e.message[0..n], message[0..n]);
        e.message[n] = 0;
        e.message_len = n;
    }
    return status;
}

pub export fn fig_parse(
    input_ptr: ?[*]const u8,
    input_len: usize,
    format: c_int,
    out_doc: ?*?*FigDocument,
) FigStatus {
    return fig_parse_ex(input_ptr, input_len, format, out_doc, null);
}

/// As `fig_parse`, but on a nonzero return also fills `out_err` (caller-allocated;
/// nullable — NULL makes this identical to `fig_parse`) with a diagnostic. On
/// `.ok` the contents of `out_err` are left unspecified.
pub export fn fig_parse_ex(
    input_ptr: ?[*]const u8,
    input_len: usize,
    format: c_int,
    out_doc: ?*?*FigDocument,
    out_err: ?*FigError,
) FigStatus {
    @setEvalBranchQuota(30_000);
    const out = out_doc orelse return fillError(out_err, .invalid_argument, "out_doc is null");
    out.* = null;

    // Empty input (len 0, null pointer or not) is a valid slice handed to the
    // parser, which judges it per format (YAML → null document, TOML → empty
    // table, JSON/JSON5/ZON → parse_error). Only a null pointer paired with a
    // nonzero length is a malformed argument. This mirrors `fig_editor_create`.
    const input = sliceOf(input_ptr, input_len) orelse
        return fillError(out_err, .invalid_argument, "null input with nonzero length");

    const entry = entryOf(format) orelse
        return fillError(out_err, .unsupported_format, "unsupported or unknown format");

    const allocator = activeAllocator();

    const source = allocator.dupe(u8, input) catch
        return fillError(out_err, .out_of_memory, "out of memory");
    const handle = allocator.create(DocumentHandle) catch {
        allocator.free(source);
        return fillError(out_err, .out_of_memory, "out of memory");
    };

    // On a parse error: free the not-yet-installed source/handle and report
    // what the language's parser said, and where.
    var parser: Runtime.Language.Parser = .{ .allocator = allocator };
    const doc = Runtime.Language.parse(&parser, source, entry.typeOf()) catch |err| {
        allocator.free(source);
        allocator.destroy(handle);
        if (err == error.OutOfMemory) return fillError(out_err, .out_of_memory, "out of memory");
        const status = fillError(out_err, .parse_error, parser.lastMessage());
        if (out_err) |oe| if (errCovers(oe.size, "byte_offset")) {
            oe.byte_offset = parser.last_error.byte_offset;
        };
        return status;
    };

    handle.* = .{
        .allocator = allocator,
        .source = source,
        .document = doc,
        .entry = entry,
        .rendered = std.Io.Writer.Allocating.init(allocator),
        .diag_arena = std.heap.ArenaAllocator.init(allocator),
    };

    out.* = @ptrCast(handle);
    return .ok;
}

fn parseFailureStatus(err: anyerror) FigStatus {
    return switch (err) {
        error.OutOfMemory => .out_of_memory,
        else => .parse_error,
    };
}

/// Shared cleanup + diagnostic for a failed parse: release the not-yet-installed
/// `source`/`handle`, then report the error (its name as the message).
fn parseFailed(out_err: ?*FigError, err: anyerror, source: []u8, handle: *DocumentHandle) FigStatus {
    const allocator = activeAllocator();
    allocator.free(source);
    allocator.destroy(handle);
    return fillError(out_err, parseFailureStatus(err), @errorName(err));
}

/// Cleanup + diagnostic for a format compiled out of this build.
fn formatDisabled(out_err: ?*FigError, source: []u8, handle: *DocumentHandle) FigStatus {
    const allocator = activeAllocator();
    allocator.free(source);
    allocator.destroy(handle);
    return fillError(out_err, .unsupported_format, "format not compiled into this build");
}

/// Memory allocated by this API should be freed by this API.
pub export fn fig_document_destroy(doc: ?*FigDocument) void {
    const public_doc = doc orelse return;
    const handle: *DocumentHandle = @ptrCast(@alignCast(public_doc));
    handle.rendered.deinit();
    handle.diag_arena.deinit();
    if (handle.arena) |a| {
        a.deinit();
        handle.allocator.destroy(a);
    } else handle.document.deinit(handle.allocator);
    handle.allocator.free(handle.source);
    handle.allocator.destroy(handle);
}

// ==================
// DOCUMENT TRAVERSAL
// ==================

pub const FigNodeId = u32;
const fig_node_none: FigNodeId = 0xFFFFFFFF;

/// Translation of an AST node's kind to the C ABI. Mirrors `FigNodeKind` in
/// `include/fig.h`.
pub const FigNodeKind = enum(c_int) {
    invalid = -1,
    null_ = 0,
    bool_ = 1,
    int = 2,
    float = 3,
    string = 4,
    sequence = 5,
    mapping = 6,
    keyvalue = 7,
    alias = 8,
    /// A format-specific scalar with no C type of its own (a TOML datetime,
    /// a ZON enum or char literal, a plist date): `fig_node_extended` says
    /// which, and gives its text.
    extended = 9,
};

fn handleFrom(doc: ?*const FigDocument) ?*const DocumentHandle {
    const public_doc = doc orelse return null;
    return @ptrCast(@alignCast(public_doc));
}

/// Returns the node at `id`, or null if `doc` is null or `id` is out of range.
fn nodeAt(doc: ?*const FigDocument, id: FigNodeId) ?AST.Node {
    const handle = handleFrom(doc) orelse return null;
    const ast = &handle.document.ast;
    if (id >= ast.nodeCount()) return null;
    return ast.node(id);
}

pub export fn fig_document_root(doc: ?*const FigDocument) FigNodeId {
    const handle = handleFrom(doc) orelse return fig_node_none;
    const ast = handle.document.ast;
    if (ast.root >= ast.nodeCount()) return fig_node_none;
    return ast.root;
}

/// A `FigNodeKind` value, as the `int` fig.h returns so that a binding
/// never decodes a kind a later fig adds into a closed enum.
pub export fn fig_node_kind(doc: ?*const FigDocument, node: FigNodeId) c_int {
    return @backingInt(nodeKind(doc, node));
}

fn nodeKind(doc: ?*const FigDocument, node: FigNodeId) FigNodeKind {
    const n = nodeAt(doc, node) orelse return .invalid;
    return switch (n.kind) {
        .null_ => .null_,
        .boolean => .bool_,
        .string => .string,
        .number => |num| switch (num.kind) {
            .integer => .int,
            .float => .float,
        },
        .extended => .extended,
        .sequence => .sequence,
        .mapping => .mapping,
        .keyvalue => .keyvalue,
        .alias => .alias,
    };
}

pub export fn fig_node_first_child(doc: ?*const FigDocument, node: FigNodeId) FigNodeId {
    const n = nodeAt(doc, node) orelse return fig_node_none;
    const child_id = switch (n.kind) {
        .sequence, .mapping => |first| first,
        else => return fig_node_none,
    };
    return child_id orelse fig_node_none;
}

pub export fn fig_node_next_sibling(doc: ?*const FigDocument, node: FigNodeId) FigNodeId {
    const n = nodeAt(doc, node) orelse return fig_node_none;
    return n.next_sibling orelse fig_node_none;
}

pub export fn fig_node_child_count(doc: ?*const FigDocument, node: FigNodeId) usize {
    const handle = handleFrom(doc) orelse return 0;
    const ast = &handle.document.ast;
    if (node >= ast.nodeCount()) return 0;
    var current: ?FigNodeId = switch (ast.kindOf(node)) {
        .sequence, .mapping => |first| first,
        else => return 0,
    };
    var count: usize = 0;
    while (current) |id| {
        if (id >= ast.nodeCount()) break;
        count += 1;
        current = ast.node(id).next_sibling;
    }
    return count;
}

pub export fn fig_keyvalue_key(doc: ?*const FigDocument, node: FigNodeId) FigNodeId {
    const n = nodeAt(doc, node) orelse return fig_node_none;
    return switch (n.kind) {
        .keyvalue => |kv| kv.key,
        else => fig_node_none,
    };
}

pub export fn fig_keyvalue_value(doc: ?*const FigDocument, node: FigNodeId) FigNodeId {
    const n = nodeAt(doc, node) orelse return fig_node_none;
    return switch (n.kind) {
        .keyvalue => |kv| kv.value,
        else => fig_node_none,
    };
}

pub export fn fig_node_bool(doc: ?*const FigDocument, node: FigNodeId, out: ?*bool) bool {
    const out_ptr = out orelse return false;
    const n = nodeAt(doc, node) orelse return false;
    switch (n.kind) {
        .boolean => |b| {
            out_ptr.* = b;
            return true;
        },
        else => return false,
    }
}

pub export fn fig_node_number(
    doc: ?*const FigDocument,
    node: FigNodeId,
    out_ptr: ?*[*c]const u8,
    out_len: ?*usize,
) bool {
    const p = out_ptr orelse return false;
    const l = out_len orelse return false;
    const n = nodeAt(doc, node) orelse return false;
    switch (n.kind) {
        .number => |num| {
            p.* = num.raw.ptr;
            l.* = num.raw.len;
            return true;
        },
        // A char literal reads out as its decimal codepoint (see fig_node_kind).
        .extended => |ext| switch (ext.kind) {
            .char_literal => {
                p.* = ext.text.ptr;
                l.* = ext.text.len;
                return true;
            },
            else => return false,
        },
        else => return false,
    }
}

pub export fn fig_node_string(
    doc: ?*const FigDocument,
    node: FigNodeId,
    out_ptr: ?*[*c]const u8,
    out_len: ?*usize,
) bool {
    const p = out_ptr orelse return false;
    const l = out_len orelse return false;
    const n = nodeAt(doc, node) orelse return false;
    switch (n.kind) {
        .string => |s| {
            p.* = s.ptr;
            l.* = s.len;
            return true;
        },
        // Datetimes and enum literals read out as their text (see fig_node_kind);
        // a char literal is a number, handled by fig_node_number instead.
        .extended => |ext| switch (ext.kind) {
            .char_literal => return false,
            else => {
                p.* = ext.text.ptr;
                l.* = ext.text.len;
                return true;
            },
        },
        else => return false,
    }
}

/// Recover the precise kind and text of a format-specific extended scalar (TOML
/// datetime, ZON enum/char literal). Returns true and writes its `FigExtKind` to
/// `out_kind` and source text to `out_ptr`/`out_len` when `node` is extended;
/// otherwise returns false, leaving the out-params untouched.
///
/// `fig_node_kind` reports these nodes as `FIG_NODE_EXTENDED`; this accessor
/// says which kind and gives the text. `fig_node_string` (and, for a char
/// literal, `fig_node_number`) still yield it too.
pub export fn fig_node_extended(
    doc: ?*const FigDocument,
    node: FigNodeId,
    out_kind: ?*c_int,
    out_ptr: ?*[*c]const u8,
    out_len: ?*usize,
) bool {
    const k = out_kind orelse return false;
    const p = out_ptr orelse return false;
    const l = out_len orelse return false;
    const n = nodeAt(doc, node) orelse return false;
    switch (n.kind) {
        .extended => |ext| {
            k.* = @backingInt(figExtKindOf(ext.kind));
            p.* = ext.text.ptr;
            l.* = ext.text.len;
            return true;
        },
        else => return false,
    }
}

/// Map an AST extended kind to its C ABI enumerator. The two enums carry the
/// same cases in the same order; the explicit switch keeps them pinned together.
fn figExtKindOf(kind: AST.Node.Kind.Extended.ExtKind) FigExtKind {
    return switch (kind) {
        .offset_datetime => .offset_datetime,
        .local_datetime => .local_datetime,
        .local_date => .local_date,
        .local_time => .local_time,
        .enum_literal => .enum_literal,
        .char_literal => .char_literal,
        .number_special => .number_special,
        .plist_date => .plist_date,
        .plist_data => .plist_data,
    };
}

// ==================
// ALIASES
// ==================

/// A new document holding `doc`'s values with the reference layer resolved:
/// each alias is a copy of the node its anchor names, and each `<<` merge key
/// is flattened into its mapping (the mapping's own keys win, then earlier
/// sources). Tags stay as written, unapplied — this is what the bindings read
/// values through, and they have never applied one. Sets `*out_doc` to NULL
/// when `doc` has no alias and no merge key, so the caller reads `doc` itself.
///
/// An alias whose anchor is not defined before it, or one inside the node it
/// refers to, has no value: `parse_error`, with the alias's offset in `out_err`.
/// The new document is independent of `doc` (either may be destroyed first).
pub export fn fig_document_resolve_aliases(
    doc: ?*const FigDocument,
    out_doc: ?*?*FigDocument,
    out_err: ?*FigError,
) FigStatus {
    const out = out_doc orelse return fillError(out_err, .invalid_argument, "out_doc is null");
    out.* = null;
    const src = handleFrom(doc) orelse return fillError(out_err, .invalid_argument, "doc is null");
    if (!sourceCarriesReferences(src) or !hasAliasOrMerge(&src.document.ast)) return .ok;

    const allocator = src.allocator;
    const arena = allocator.create(std.heap.ArenaAllocator) catch
        return fillError(out_err, .out_of_memory, "out of memory");
    arena.* = std.heap.ArenaAllocator.init(allocator);
    var culprit: ?AST.Node.Id = null;
    const ast = resolvedAst(arena.allocator(), &src.document.ast, &culprit) catch |err| {
        arena.deinit();
        allocator.destroy(arena);
        if (err == error.OutOfMemory) return fillError(out_err, .out_of_memory, "out of memory");
        return aliasFailed(out_err, err, &src.document, culprit);
    };
    const handle = allocator.create(DocumentHandle) catch {
        arena.deinit();
        allocator.destroy(arena);
        return fillError(out_err, .out_of_memory, "out of memory");
    };
    handle.* = .{
        .allocator = allocator,
        .source = &.{},
        .document = .{ .source = "", .ast = ast },
        .entry = src.entry,
        .rendered = std.Io.Writer.Allocating.init(allocator),
        .diag_arena = std.heap.ArenaAllocator.init(allocator),
        .arena = arena,
    };
    out.* = @ptrCast(handle);
    return .ok;
}

/// `ast` with its aliases and merges collapsed and its tags kept, built in
/// `arena` and owning its strings there: a materialized tree's texts are
/// all in its own buffer.
fn resolvedAst(arena: std.mem.Allocator, ast: *const AST, culprit: *?AST.Node.Id) Materialize.Error!AST {
    return Materialize.materializeReporting(arena, ast, .keep, culprit);
}

/// Whether `ast` holds anything `fig_document_resolve_aliases` would
/// collapse: an alias node, or an entry keyed with the language's merge key.
fn hasAliasOrMerge(ast: *const AST) bool {
    for (0..ast.nodeCount()) |i| switch (ast.kindOf(@intCast(i))) {
        .alias => return true,
        .keyvalue => |kv| switch (ast.node(kv.key).kind) {
            .string => |k| if (ast.merge_key) |mk| if (std.mem.eql(u8, k, mk)) return true,
            else => {},
        },
        else => {},
    };
    return false;
}

/// Report an alias `Materialize` could not resolve as a `parse_error` at the
/// node it failed on: the alias itself, or the mapping whose `<<` merge it is.
fn aliasFailed(out_err: ?*FigError, err: anyerror, doc: *const Document, culprit: ?AST.Node.Id) FigStatus {
    const alias: ?[]const u8 = if (culprit) |id| switch (doc.ast.node(id).kind) {
        .alias => |name| name,
        else => null,
    } else null;
    var buf: [256]u8 = undefined;
    const message = switch (err) {
        error.UndefinedAlias => if (alias) |name|
            std.fmt.bufPrint(&buf, "alias `*{s}` names an anchor that is not defined before it", .{name}) catch "an alias names an anchor that is not defined before it"
        else
            "a `<<` merge names an anchor that is not defined before it",
        error.AliasCycle => if (alias) |name|
            std.fmt.bufPrint(&buf, "alias `*{s}` refers to a node that contains it, so it has no value", .{name}) catch "an alias refers to a node that contains it, so it has no value"
        else
            "a `<<` merge refers to a mapping that contains it, so it has no value",
        error.TooDeep => "aliases refer to aliases too deeply to resolve",
        else => "an alias cannot be resolved",
    };
    const status = fillError(out_err, .parse_error, message);
    const e = out_err orelse return status;
    const id = culprit orelse return status;
    if (id >= doc.ast.nodeCount()) return status;
    const offset = doc.spanOf(id).start;
    if (offset > doc.source.len) return status;
    const before = doc.source[0..offset];
    const line_start = if (std.mem.lastIndexOfScalar(u8, before, '\n')) |nl| nl + 1 else 0;
    if (errCovers(e.size, "byte_offset")) e.byte_offset = offset;
    if (errCovers(e.size, "line")) e.line = @intCast(std.mem.countScalar(u8, before, '\n') + 1);
    if (errCovers(e.size, "column")) e.column = @intCast(offset - line_start + 1);
    return status;
}

// ======
// EDITING
// ======
//
// The write path mirrors the read path: an opaque handle owns the source +
// parse, and edits splice bytes in place (preserving comments/formatting) then
// reparse. `fig_editor_*` works on a whole document. Path segments cross the
// boundary as an array of `FigPathSegment` (key string | sequence index),
// mirroring `AST.PathSegment`.

/// One step of a path: `kind == 0` selects mapping key `key_ptr[0..key_len]`;
/// `kind == 1` selects sequence element `index`. `kind` is a C `int` (not a
/// fixed-width `int32_t`), matching the other small discriminants crossing this
/// ABI — `format` and the `kind` of `fig_value_extended` /
/// `fig_node_extended` — so every enum-like field is the one integer type.
pub const FigPathSegment = extern struct {
    kind: c_int,
    key_ptr: ?[*]const u8,
    key_len: usize,
    index: usize,
};

const max_path_len = 128;
const max_keys_len = 512;

/// A borrowed UTF-8 string slice passed across the C ABI: `ptr[0..len]`. Used
/// for the key list of `fig_*_reorder_keys`.
pub const FigStr = extern struct {
    ptr: ?[*]const u8,
    len: usize,
};

/// Decode a C array of `FigStr` into `buf`. Returns the populated slice, or
/// null on a malformed entry / over-long list. A zero-length entry decodes to
/// an empty key regardless of its (possibly null) pointer.
fn decodeKeys(
    keys_ptr: ?[*]const FigStr,
    keys_len: usize,
    buf: [][]const u8,
) ?[][]const u8 {
    if (keys_len == 0) return buf[0..0];
    if (keys_len > buf.len) return null;
    const ks = keys_ptr orelse return null;
    for (0..keys_len) |i| {
        buf[i] = if (ks[i].len == 0) &.{} else (ks[i].ptr orelse return null)[0..ks[i].len];
    }
    return buf[0..keys_len];
}

/// View a C `usize` array as a Zig slice. Returns null only when the pointer is
/// null for a non-empty length (a zero length is a valid empty list).
fn decodeIndices(ptr: ?[*]const usize, len: usize) ?[]const usize {
    if (len == 0) return &.{};
    const p = ptr orelse return null;
    return p[0..len];
}

/// Decode a C path array into `buf`. Returns the populated slice, or null on a
/// malformed segment / over-long path.
fn decodePath(
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    buf: []AST.PathSegment,
) ?[]AST.PathSegment {
    if (path_len == 0) return buf[0..0];
    if (path_len > buf.len) return null;
    const segs = path_ptr orelse return null;
    for (0..path_len) |i| {
        buf[i] = switch (segs[i].kind) {
            0 => .{ .key = (segs[i].key_ptr orelse return null)[0..segs[i].key_len] },
            1 => .{ .index = segs[i].index },
            else => return null,
        };
    }
    return buf[0..path_len];
}

/// Translate the editor/AST error set onto `FigStatus`.
fn editStatus(err: anyerror) FigStatus {
    return switch (err) {
        error.OutOfMemory => .out_of_memory,
        error.NotFound => .not_found,
        error.NotAMapping, error.NotASequence, error.NotAContainer, error.InvalidDocument => .invalid_argument,
        // A key rename at a path that names no key: a sequence item, or the root.
        error.NotAKey => .invalid_argument,
        // `setSequence` declines a shape it can't safely diff (empty target,
        // empty/non-scalar list, a format whose scalars can't stand alone).
        error.UnsupportedShape => .invalid_argument,
        // TOML structural edits reject a request that doesn't match the document
        // shape (e.g. appending to a non-array-of-tables, deleting a table by the
        // scalar ops, inserting a key/table that already exists). These are caller
        // errors, not malformed-source reparse failures.
        error.NotASection, error.NotAnInlineArray, error.NotAnArrayOfTables, error.SectionExists, error.DuplicateKey, error.MergeOnlyKey => .invalid_argument,
        // The pre-op guards: a scalar op addressed a whole scattered container,
        // whose node span covers only the header line (delete) or just the name
        // inside it (replace), so the generic splice would orphan or rename what
        // it can't see. One error for every format's sections, and a caller
        // error about the request, not malformed source: the whole-container
        // ops (`deleteContainer`, `renameContainer`, …) handle these shapes.
        error.CannotDeleteSection => .invalid_argument,
        error.CannotReplaceSection => .invalid_argument,
        // The same family for the two ops that RELOCATE an entry's block: the
        // block of a `[header]`/`[section]` entry is its header line, so a move
        // strands the body and a reorder hands it to the entry that lands
        // before it. `fig_editor_move_container`/`fig_editor_reorder_containers`
        // are the ops that relocate a scattered container whole.
        error.CannotMoveSection => .invalid_argument,
        error.CannotReorderSections => .invalid_argument,
        // NestedText declines two shapes outright: inserting into an inline
        // `{}`/`[]` (expanding one into block form is out of scope) and renaming
        // a key to text that needs the multiline `: key` form.
        error.EmptyInlineContainer, error.KeyRequiresMultilineForm => .invalid_argument,
        // A runtime language's renderer declined the text (its reason is in
        // `Runtime.lastRefusal()`): the request, not the source.
        error.RendererRefused => .invalid_argument,
        // A block-spelled value (`- a`, `k: v`, `|`) was handed to a splice into
        // a flow `{…}`/`[…]` container, which has no way to hold it.
        error.BlockValueIntoFlow => .invalid_argument,
        // A container that closes on its last entry's line has no line to
        // append a new entry after (`<key>o</key><dict>…</dict>`).
        error.ContainerClosesOnItsLine => .invalid_argument,
        // An insert into a section with no header line of its own — an
        // implicit TOML table — where every line it is named on opens a child.
        error.ImplicitSection => .invalid_argument,
        // The target dialect has no comment syntax (strict JSON).
        error.CommentsUnsupported => .unsupported_format,
        // The addressed node shares its parent's line inside a flow collection
        // (`members = ["a", "b"]`), so it owns neither the line above nor that
        // line's end — a fact about the request's PATH, not about the format,
        // which is why this is `invalid_argument` and `CommentsUnsupported`
        // above is not.
        error.CommentsUnanchored => .invalid_argument,
        // `fig_editor_uncomment_*` stripped the markers and the result parsed,
        // but to a document whose other nodes had moved — so those lines were
        // not the entry the caller took them for, and the splice was rolled
        // back. Every argument was individually valid, which is what
        // `unsupported_operation` says and `invalid_argument` would not.
        error.CommentNotAnEntry => .unsupported_operation,
        // A trailing comment on a value that runs to its line's end (a ZON
        // `\\` string) parsed back as part of the value, and the splice was
        // rolled back — the same kind of answer as `CommentNotAnEntry`.
        error.CommentInsideValue => .unsupported_operation,
        // A delete whose splice parsed to a document with more than the one
        // entry or item changed, rolled back: the same kind of answer again.
        error.DeleteChangedOtherValues => .unsupported_operation,
        // A replace, insert, move or reorder whose splice parsed to a
        // document with more changed than asked, or with the value written
        // reading as another, rolled back: the same kind of answer again.
        error.EditReadsBackWrong => .unsupported_operation,
        // Deleting the only item of a root list in a format whose empty
        // document is an empty mapping (figl), which has no spelling of an
        // empty root list: the request names a valid item, so not
        // `invalid_argument`.
        error.NoEmptyRootList => .unsupported_operation,
        // A value the format cannot represent at all — plist has no null. Same
        // answer `serializeStatus` gives it, since it is the same fact about the
        // format either way. Infinity or NaN into strict JSON is the same kind
        // of fact.
        error.NullUnsupported, error.NonFiniteNumber => .unsupported_format,
        // A trailing comment was given multi-line text, or (plist) comment text
        // carrying `--`, which cannot go inside `<!-- … -->`.
        error.MultilineComment, error.InvalidComment => .invalid_argument,
        error.NotInitialized, error.MultipleInit, error.InvalidSpan => .internal_error,
        // A reparse after a malformed edit lands here.
        else => .parse_error,
    };
}

pub const FigEditor = opaque {};

// The editor backend:
// the `Editor` over `Runtime.Language`, which every format is edited
// through, its `format` payload naming the registered dialect. A union of
// one, so that the editor operations below read the same as when there was
// one variant per compiled language.
const EditorUnion = union(enum) {
    runtime: Editor(Runtime.Language),
};

const EditorHandle = struct {
    allocator: std.mem.Allocator,
    inner: EditorUnion,
    /// Reused buffer backing the borrowed bytes returned by the comment-read
    /// exports (`fig_editor_get_*_comment`). Refilled per call; valid until the
    /// next read call on this handle or `fig_editor_destroy`.
    scratch: std.ArrayList(u8) = .empty,

    fn deinit(self: *EditorHandle) void {
        switch (self.inner) {
            inline else => |*e| e.deinit(),
        }
        self.scratch.deinit(self.allocator);
    }
};

fn editorFrom(ed: ?*FigEditor) ?*EditorHandle {
    const p = ed orelse return null;
    return @ptrCast(@alignCast(p));
}

pub export fn fig_editor_create(
    input_ptr: ?[*]const u8,
    input_len: usize,
    format: c_int,
    out_editor: ?*?*FigEditor,
) FigStatus {
    @setEvalBranchQuota(30_000);
    const out = out_editor orelse return .invalid_argument;
    out.* = null;

    // Empty input (len 0, with or without a null pointer) is a valid empty
    // document; a non-null pointer with a length is read as-is.
    const slice = sliceOf(input_ptr, input_len) orelse return .invalid_argument;

    const allocator = activeAllocator();
    // The format is checked BEFORE the handle is allocated, so the two ways
    // it can be refused — an unknown handle, no in-place editor — return
    // without anything to free.
    const entry = entryOf(format) orelse return .unsupported_format;
    if (!entry.language.caps.edit) return .unsupported_format;
    const inner: EditorUnion = .{ .runtime = .{ .allocator = allocator, .format = entry.typeOf() } };

    const handle = allocator.create(EditorHandle) catch return .out_of_memory;
    handle.allocator = allocator;
    // Field-by-field init (not a struct literal), so set the read scratch buffer's
    // default explicitly — otherwise destroy frees uninitialized memory.
    handle.scratch = .empty;
    handle.inner = inner;

    switch (handle.inner) {
        inline else => |*e| e.init(slice) catch |err| {
            e.deinit();
            allocator.destroy(handle);
            return editStatus(err);
        },
    }

    out.* = @ptrCast(handle);
    return .ok;
}

pub export fn fig_editor_destroy(ed: ?*FigEditor) void {
    const handle = editorFrom(ed) orelse return;
    const allocator = handle.allocator;
    handle.deinit();
    allocator.destroy(handle);
}

pub export fn fig_editor_replace_val(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    repl_ptr: ?[*]const u8,
    repl_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const repl = sliceOf(repl_ptr, repl_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.replaceValAtPath(path, repl)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_replace_key(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    repl_ptr: ?[*]const u8,
    repl_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const repl = sliceOf(repl_ptr, repl_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.replaceKeyAtPath(path, repl)) .ok else |err| editStatus(err),
    };
}

/// `fig_editor_replace_key` with the new key's NAME rather than its syntax,
/// spelled as the document's format spells a key — as
/// `fig_editor_insert_named_key` is to `fig_editor_insert_key`.
pub export fn fig_editor_replace_named_key(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    name_ptr: ?[*]const u8,
    name_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const name = sliceOf(name_ptr, name_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.replaceNamedKey(path, name)) .ok else |err| editStatus(err),
    };
}

/// Upsert: replace the value at `path`, or insert it when only the trailing key
/// is absent (the `path` must end in a key). Folds replace-or-insert into one
/// op; see `Editor.set`.
pub export fn fig_editor_set(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    val_ptr: ?[*]const u8,
    val_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const val = sliceOf(val_ptr, val_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.set(path, val)) .ok else |err| editStatus(err),
    };
}

/// `fig_editor_replace_val` for a string the caller rendered: `repl` is its
/// spelling of the string `str`. Where the value replaced is a string, the
/// new one keeps the old one's quote style when that style can hold it (a
/// YAML `"debug"` replaced by `info` stays `"info"`); see
/// `Editor.replaceStringAtPath`.
pub export fn fig_editor_replace_string(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    repl_ptr: ?[*]const u8,
    repl_len: usize,
    str_ptr: ?[*]const u8,
    str_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const repl = sliceOf(repl_ptr, repl_len) orelse return .invalid_argument;
    const str = sliceOf(str_ptr, str_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.replaceStringAtPath(path, repl, str)) .ok else |err| editStatus(err),
    };
}

/// `fig_editor_set` for a string the caller rendered, as
/// `fig_editor_replace_string` is to `fig_editor_replace_val`.
pub export fn fig_editor_set_string(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    val_ptr: ?[*]const u8,
    val_len: usize,
    str_ptr: ?[*]const u8,
    str_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const val = sliceOf(val_ptr, val_len) orelse return .invalid_argument;
    const str = sliceOf(str_ptr, str_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.setString(path, val, str)) .ok else |err| editStatus(err),
    };
}

// ── Comment editing ─────────────────────────────────────────────────────────
// Splice comment trivia around the node at `path`, preserving the rest of the
// document byte-for-byte. The marker (`#`, `//`) is added by the editor; a
// dialect without comment syntax (strict JSON) returns `unsupported_format`.
//
// An element or entry of a one-line flow collection owns no comment at all —
// the line is its parent's — so the two writes return `invalid_argument`, the
// deletes are a no-op, and the reads answer `not_found`. See
// `Editor.commentsUnanchored`.

/// Add an own-line comment ABOVE the node at `path`. `text` may be multi-line
/// (one comment line per row), at the node's indentation, nearest the node.
pub export fn fig_editor_add_leading_comment(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    text_ptr: ?[*]const u8,
    text_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const text = sliceOf(text_ptr, text_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.addLeadingComment(path, text)) .ok else |err| editStatus(err),
    };
}

/// Set the same-line trailing comment on the value at `path` (replace existing
/// or append). `text` must be single-line (else `invalid_argument`).
pub export fn fig_editor_set_trailing_comment(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    text_ptr: ?[*]const u8,
    text_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const text = sliceOf(text_ptr, text_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.setTrailingComment(path, text)) .ok else |err| editStatus(err),
    };
}

/// Remove the own-line comment block immediately above the node at `path` (no-op
/// when there is none).
pub export fn fig_editor_delete_leading_comments(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.deleteLeadingComments(path)) .ok else |err| editStatus(err),
    };
}

/// Remove the same-line trailing comment on the value at `path` (no-op when
/// there is none).
pub export fn fig_editor_delete_trailing_comment(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.deleteTrailingComment(path)) .ok else |err| editStatus(err),
    };
}

// ── Comment reading ─────────────────────────────────────────────────────────
// Read back a comment without mutating the document. The returned bytes (marker
// stripped) are BORROWED from the editor handle's scratch buffer: valid until the
// next read call on this handle or `fig_editor_destroy`. The distinction between
// an ABSENT comment and a PRESENT-BUT-EMPTY one (a bare `#`/`//`) is carried by
// the status: `not_found` means absent; `ok` with `out_len == 0` means present and
// empty. Strict JSON (no comment syntax) returns `unsupported_format`.

/// Hand a comment read's bytes back across the ABI: copy them into `scratch`
/// (borrowed until the next read on this handle), free the original, and set
/// the out-parameters. A null `maybe` — no such comment — is `not_found`, which
/// is what distinguishes an ABSENT comment from a present-but-empty one.
/// Shared by both handles' reads, and by the dangling read's own entry point.
fn fillCommentOut(
    allocator: std.mem.Allocator,
    scratch: *std.ArrayList(u8),
    maybe: ?[]u8,
    p: *[*c]const u8,
    l: *usize,
) FigStatus {
    const bytes = maybe orelse return .not_found;
    defer allocator.free(bytes);
    scratch.clearRetainingCapacity();
    // Keep a valid (non-dangling) pointer even for a zero-length present comment.
    scratch.ensureTotalCapacity(allocator, bytes.len + 1) catch return .out_of_memory;
    scratch.appendSliceAssumeCapacity(bytes);
    p.* = scratch.items.ptr;
    l.* = scratch.items.len;
    return .ok;
}

fn editorGetComment(
    handle: *EditorHandle,
    path: []const AST.PathSegment,
    trailing: bool,
    out_ptr: ?*[*c]const u8,
    out_len: ?*usize,
) FigStatus {
    const p = out_ptr orelse return .invalid_argument;
    const l = out_len orelse return .invalid_argument;
    const maybe = (switch (handle.inner) {
        inline else => |*e| if (trailing) e.getTrailingComment(path) else e.getLeadingComment(path),
    }) catch |err| return editStatus(err);
    return fillCommentOut(handle.allocator, &handle.scratch, maybe, p, l);
}

/// Read the own-line comment block immediately ABOVE the node at `path`, joined by
/// '\n' with markers and indentation stripped. `not_found` when there is no block.
pub export fn fig_editor_get_leading_comment(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    out_ptr: ?*[*c]const u8,
    out_len: ?*usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return editorGetComment(handle, path, false, out_ptr, out_len);
}

/// Read the same-line trailing comment on the value at `path`, marker stripped.
/// `not_found` when there is none.
pub export fn fig_editor_get_trailing_comment(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    out_ptr: ?*[*c]const u8,
    out_len: ?*usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return editorGetComment(handle, path, true, out_ptr, out_len);
}

// ── The dangling anchor, and comment-out ────────────────────────────────────
// A container's third comment anchor — the run at the end of its body, after
// its last entry — plus the pair that turns an entry into a comment run and
// back. See `Editor.getDanglingComment` and `Editor.commentOut`.

/// Add own-line comment line(s) at the END of the container at `path`'s body
/// (empty path = the root), at the body's child depth.
pub export fn fig_editor_add_dangling_comment(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    text_ptr: ?[*]const u8,
    text_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const text = sliceOf(text_ptr, text_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.addDanglingComment(path, text)) .ok else |err| editStatus(err),
    };
}

/// Remove the whole dangling run at the end of the container at `path`'s body
/// (no-op when there is none).
pub export fn fig_editor_delete_dangling_comments(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.deleteDanglingComments(path)) .ok else |err| editStatus(err),
    };
}

/// Read the dangling run at the end of the container at `path`'s body, joined
/// by '\n' with markers and indentation stripped. Same borrowed-bytes and
/// `not_found`-means-absent contract as the other comment reads.
pub export fn fig_editor_get_dangling_comment(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    out_ptr: ?*[*c]const u8,
    out_len: ?*usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const p = out_ptr orelse return .invalid_argument;
    const l = out_len orelse return .invalid_argument;
    const maybe = (switch (handle.inner) {
        inline else => |*e| e.getDanglingComment(path),
    }) catch |err| return editStatus(err);
    return fillCommentOut(handle.allocator, &handle.scratch, maybe, p, l);
}

/// Turn the node at `path` into a comment run: every line of its span gains the
/// line marker at that line's own indentation. Its own leading comment block
/// stays above it; afterwards the tree no longer has the node.
pub export fn fig_editor_comment_out(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.commentOut(path)) .ok else |err| editStatus(err),
    };
}

/// Bring `line_count` lines of the LEADING comment block above the node at
/// `path`, starting at `first_line`, back as entries. `unsupported_operation`
/// when the result parses but is not the entry it was taken for; the document
/// is unchanged on every failure.
pub export fn fig_editor_uncomment_leading(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    first_line: usize,
    line_count: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.uncommentLeading(path, first_line, line_count)) .ok else |err| editStatus(err),
    };
}

/// The dangling twin of `fig_editor_uncomment_leading`: the run at the end of
/// the container at `path`'s body — where a commented-out LAST entry lands.
pub export fn fig_editor_uncomment_dangling(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    first_line: usize,
    line_count: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.uncommentDangling(path, first_line, line_count)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_insert_key(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    key_ptr: ?[*]const u8,
    key_len: usize,
    val_ptr: ?[*]const u8,
    val_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const key = sliceOf(key_ptr, key_len) orelse return .invalid_argument;
    const val = sliceOf(val_ptr, val_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.insertKey(path, key, val)) .ok else |err| editStatus(err),
    };
}

/// `fig_editor_insert_key` with the key's NAME rather than its syntax: the
/// name is spelled as the document's format spells a key — `.name` in ZON,
/// quoted in strict JSON, quoted when it must be in TOML — which is what a
/// binding that took the name from its caller wants. `fig_editor_insert_key`
/// stays for a caller that has the syntax already.
pub export fn fig_editor_insert_named_key(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    name_ptr: ?[*]const u8,
    name_len: usize,
    val_ptr: ?[*]const u8,
    val_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const name = sliceOf(name_ptr, name_len) orelse return .invalid_argument;
    const val = sliceOf(val_ptr, val_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.insertNamedKey(path, name, val)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_delete_key(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.deleteKey(path)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_append_seq(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    val_ptr: ?[*]const u8,
    val_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const val = sliceOf(val_ptr, val_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.appendToSeq(path, val)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_prepend_seq(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    val_ptr: ?[*]const u8,
    val_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const val = sliceOf(val_ptr, val_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.prependToSeq(path, val)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_insert_seq_item(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    index: usize,
    val_ptr: ?[*]const u8,
    val_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const val = sliceOf(val_ptr, val_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.insertItem(path, index, val)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_remove_seq_item(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    index: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.removeSeqItem(path, index)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_move_key(
    ed: ?*FigEditor,
    src_ptr: ?[*]const FigPathSegment,
    src_len: usize,
    dest_ptr: ?[*]const FigPathSegment,
    dest_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var src_buf: [max_path_len]AST.PathSegment = undefined;
    var dest_buf: [max_path_len]AST.PathSegment = undefined;
    const src = decodePath(src_ptr, src_len, &src_buf) orelse return .invalid_argument;
    const dest = decodePath(dest_ptr, dest_len, &dest_buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.moveKey(src, dest)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_reorder_keys(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    keys_ptr: ?[*]const FigStr,
    keys_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var path_buf: [max_path_len]AST.PathSegment = undefined;
    var keys_buf: [max_keys_len][]const u8 = undefined;
    const path = decodePath(path_ptr, path_len, &path_buf) orelse return .invalid_argument;
    const keys = decodeKeys(keys_ptr, keys_len, &keys_buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.reorderKeys(path, keys)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_move_item(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    from: usize,
    to: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.moveItem(path, from, to)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_reorder_items(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    indices_ptr: ?[*]const usize,
    indices_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const indices = decodeIndices(indices_ptr, indices_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.reorderItems(path, indices)) .ok else |err| editStatus(err),
    };
}

pub export fn fig_editor_set_sequence(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    items_ptr: ?[*]const FigStr,
    items_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var path_buf: [max_path_len]AST.PathSegment = undefined;
    var items_buf: [max_keys_len][]const u8 = undefined;
    const path = decodePath(path_ptr, path_len, &path_buf) orelse return .invalid_argument;
    const items = decodeKeys(items_ptr, items_len, &items_buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (e.setSequence(path, items)) .ok else |err| editStatus(err),
    };
}

// ============
// EDITOR — WHOLE-CONTAINER OPS
// ============
//
// The six ops for containers that are SCATTERED through the source: a TOML
// `[header]` table (whose body is the lines after it, extended by every
// `[a.b]` header elsewhere in the file), an INI `[section]`, a fig block
// container. Such a container owns no single range to splice, so the key ops
// above cannot address it — `fig_editor_delete_key` and
// `fig_editor_replace_val` at a table path return `invalid_argument` (the
// editor's pre-op guards), and these are where that request goes instead.
//
// Only the section formats have them (`Editor.hasContainerOp`): TOML all six,
// INI and fig the four that need no header syntax (delete, move, reorder and
// rename, over the header lines and name mentions their parsers record; the
// two that write a `[header]` line need a `section_header`), and everything else none
// — YAML, JSON and the rest nest their containers in one contiguous region, so
// the key ops already handle them and there is nothing to derive. A format
// that lacks the op answers `unsupported_format`, the same answer
// `fig_editor_create` gives for a format with no editor at all.
//
// Paths are the usual `FigPathSegment` array; `body_text` is verbatim entry
// lines for the target format (`ip = "10.0.0.1"\n`), spliced and reparsed like
// every other edit, so a body that doesn't parse rolls back.

/// Delete the whole container at `path` — every scattered region of its
/// subtree, leaving interleaved foreign content in place.
pub export fn fig_editor_delete_container(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (comptime @TypeOf(e.*).hasContainerOp("deleteContainer"))
            if (!e.dialectHasContainerOp("deleteContainer")) .unsupported_format else (if (e.deleteContainer(path)) .ok else |err| editStatus(err))
        else
            .unsupported_format,
    };
}

/// Create a new container at `path` with `body_text` as its entries, spliced
/// past the parent's whole subtree so no existing key is reparented.
pub export fn fig_editor_insert_container(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    body_ptr: ?[*]const u8,
    body_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const body = sliceOf(body_ptr, body_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (comptime @TypeOf(e.*).hasContainerOp("insertContainer"))
            if (!e.dialectHasContainerOp("insertContainer")) .unsupported_format else (if (e.insertContainer(path, body)) .ok else |err| editStatus(err))
        else
            .unsupported_format,
    };
}

/// Rename the container at `path` to `new_leaf`, rewriting every line that
/// names it — `[a]`, `[a.b]`, `[[a.c]]` all follow an `a` → `q` rename.
///
/// This is what `fig_editor_replace_key` at a block-table path routes to
/// internally; called directly it also reaches a container the key op cannot
/// address.
pub export fn fig_editor_rename_container(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    leaf_ptr: ?[*]const u8,
    leaf_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const leaf = sliceOf(leaf_ptr, leaf_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (comptime @TypeOf(e.*).hasContainerOp("renameContainer"))
            if (!e.dialectHasContainerOp("renameContainer")) .unsupported_format else (if (e.renameContainer(path, leaf)) .ok else |err| editStatus(err))
        else
            .unsupported_format,
    };
}

/// Move the container at `src_path` to sit before the one at `dest_path`,
/// re-emitting its scattered fragments contiguously; interleaved foreign
/// containers stay put.
///
/// A NULL `dest_ptr` means "to the end of the document" — distinct from a
/// zero-length destination path, which every other export reads as the root.
pub export fn fig_editor_move_container(
    ed: ?*FigEditor,
    src_ptr: ?[*]const FigPathSegment,
    src_len: usize,
    dest_ptr: ?[*]const FigPathSegment,
    dest_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var src_buf: [max_path_len]AST.PathSegment = undefined;
    var dest_buf: [max_path_len]AST.PathSegment = undefined;
    const src = decodePath(src_ptr, src_len, &src_buf) orelse return .invalid_argument;
    const dest: ?[]AST.PathSegment = if (dest_ptr == null)
        null
    else
        (decodePath(dest_ptr, dest_len, &dest_buf) orelse return .invalid_argument);
    return switch (handle.inner) {
        inline else => |*e| if (comptime @TypeOf(e.*).hasContainerOp("moveContainer"))
            if (!e.dialectHasContainerOp("moveContainer")) .unsupported_format else (if (e.moveContainer(src, dest)) .ok else |err| editStatus(err))
        else
            .unsupported_format,
    };
}

/// Reorder top-level containers among themselves so the ones named in `order`
/// come first, in that order; each is re-emitted contiguously at the position
/// the earliest of them currently occupies. Containers not named keep their
/// places.
pub export fn fig_editor_reorder_containers(
    ed: ?*FigEditor,
    order_ptr: ?[*]const FigStr,
    order_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_keys_len][]const u8 = undefined;
    const order = decodeKeys(order_ptr, order_len, &buf) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (comptime @TypeOf(e.*).hasContainerOp("reorderContainers"))
            if (!e.dialectHasContainerOp("reorderContainers")) .unsupported_format else (if (e.reorderContainers(order)) .ok else |err| editStatus(err))
        else
            .unsupported_format,
    };
}

/// Append a new element with body `body_text` to the container sequence at
/// `path` — TOML's `[[header]]` array-of-tables append — past every line of the
/// current last element's subtree.
pub export fn fig_editor_append_container_to_seq(
    ed: ?*FigEditor,
    path_ptr: ?[*]const FigPathSegment,
    path_len: usize,
    body_ptr: ?[*]const u8,
    body_len: usize,
) FigStatus {
    const handle = editorFrom(ed) orelse return .invalid_argument;
    var buf: [max_path_len]AST.PathSegment = undefined;
    const path = decodePath(path_ptr, path_len, &buf) orelse return .invalid_argument;
    const body = sliceOf(body_ptr, body_len) orelse return .invalid_argument;
    return switch (handle.inner) {
        inline else => |*e| if (comptime @TypeOf(e.*).hasContainerOp("appendContainerToSeq"))
            if (!e.dialectHasContainerOp("appendContainerToSeq")) .unsupported_format else (if (e.appendContainerToSeq(path, body)) .ok else |err| editStatus(err))
        else
            .unsupported_format,
    };
}

/// Borrow the editor's current source bytes. Valid until the next mutation or
/// `fig_editor_destroy`.
pub export fn fig_editor_source(
    ed: ?*const FigEditor,
    out_ptr: ?*[*c]const u8,
    out_len: ?*usize,
) FigStatus {
    const p = out_ptr orelse return .invalid_argument;
    const l = out_len orelse return .invalid_argument;
    const handle: *const EditorHandle = @ptrCast(@alignCast(ed orelse return .invalid_argument));
    switch (handle.inner) {
        inline else => |*e| {
            p.* = e.source.items.ptr;
            l.* = e.source.items.len;
        },
    }
    return .ok;
}

fn sliceOf(ptr: ?[*]const u8, len: usize) ?[]const u8 {
    if (len == 0) return &.{};
    const p = ptr orelse return null;
    return p[0..len];
}

// ==============================
// VALUE CONSTRUCTION + SERIALIZE
// ==============================
//
// The build/serialize counterpart to the read-side traversal API: construct a
// fresh value tree node-by-node (the mirror of `AST.Builder`), then render it to
// any supported format. Construction is bottom-up — build children, then the
// container from their ids — and every `fig_value_*` builder call returns the new
// node's id through `out_id`. A built value owns no source, so all input bytes
// are copied; the rendered output is borrowed (valid until the next
// `fig_value_serialize` or `fig_value_destroy`), so callers copy it out.

pub const FigValue = opaque {};

/// A `key: value` entry for `fig_value_map`; both are ids returned by earlier
/// `fig_value_*` calls. Mirrors `AST.Builder.Entry`.
pub const FigKeyValue = extern struct {
    key: FigNodeId,
    value: FigNodeId,
};

/// The format-specific scalar kinds, mirroring `AST.Node.Kind.Extended.ExtKind`.
pub const FigExtKind = enum(c_int) {
    offset_datetime = 0,
    local_datetime = 1,
    local_date = 2,
    local_time = 3,
    enum_literal = 4,
    char_literal = 5,
    number_special = 6,
    plist_date = 7,
    plist_data = 8,
};

const ValueHandle = struct {
    allocator: std.mem.Allocator,
    builder: AST.Builder,
    /// Reused across `fig_value_serialize` calls; holds the bytes the most recent
    /// call returned (cleared and refilled each time).
    rendered: std.Io.Writer.Allocating,
    /// Backs the warnings the most recent `fig_value_diagnose` produced; reset
    /// (not freed) each call. Mirrors `DocumentHandle.diag_arena`.
    diag_arena: std.heap.ArenaAllocator,
    /// The warning set the most recent `fig_value_diagnose` computed (stored in
    /// `diag_arena`); `fig_value_warning` indexes into it. Mirrors
    /// `DocumentHandle.diag_warnings`.
    diag_warnings: []const Diagnostics.Warning = &.{},
};

fn valueFrom(value: ?*FigValue) ?*ValueHandle {
    const p = value orelse return null;
    return @ptrCast(@alignCast(p));
}

fn extKindOf(kind: c_int) ?AST.Node.Kind.Extended.ExtKind {
    return switch (kind) {
        @backingInt(FigExtKind.offset_datetime) => .offset_datetime,
        @backingInt(FigExtKind.local_datetime) => .local_datetime,
        @backingInt(FigExtKind.local_date) => .local_date,
        @backingInt(FigExtKind.local_time) => .local_time,
        @backingInt(FigExtKind.enum_literal) => .enum_literal,
        @backingInt(FigExtKind.char_literal) => .char_literal,
        @backingInt(FigExtKind.number_special) => .number_special,
        @backingInt(FigExtKind.plist_date) => .plist_date,
        @backingInt(FigExtKind.plist_data) => .plist_data,
        else => null,
    };
}

/// A serialize target: the registered dialect a format handle names, when
/// it can be written. The one policy point that keeps `fig_*_serialize`,
/// `fig_*_diagnose` and `fig_format_capabilities` in agreement: a handle
/// never handed out, or one whose language does not serialize, is
/// `unsupported_format` to all three.
const Target = Runtime.Target;

fn targetOf(format: c_int) ?Target {
    const e = entryOf(format) orelse return null;
    if (!e.language.caps.serialize) return null;
    return .{ .language = e };
}

/// Translate the canonical serialize error set onto `FigStatus`. Exhaustive over
/// `AST.SerializeError`: a representability failure (alias/null/non-string key in
/// a format that cannot hold it) maps to `unsupported_format`; the writer's only
/// other failure is allocation, surfaced as `WriteFailed`.
fn serializeStatus(err: AST.SerializeError) FigStatus {
    return switch (err) {
        error.UnresolvedAlias,
        error.NullUnsupported,
        error.NonStringKey,
        error.FormatDisabled,
        error.NestingTooDeep,
        error.UnexpectedNodeKind,
        error.UnrepresentableRoot,
        error.UnsupportedValue,
        error.InvalidKey,
        error.NonFiniteNumber,
        error.PrintRefused,
        => .unsupported_format,
        error.WriteFailed => .out_of_memory,
    };
}

/// `serializeStatus` for a runtime printer: the vtable's refusal is the
/// format saying it cannot write the tree, which is `unsupported_format`
/// like every compiled printer's representability failure; building the
/// table it was handed can only fail on allocation.
fn runtimePrintStatus(err: anyerror) FigStatus {
    return switch (err) {
        error.OutOfMemory, error.WriteFailed => .out_of_memory,
        else => .unsupported_format,
    };
}

/// Write the id produced by a builder call to `out_id`, mapping allocation
/// failure to a status. Builder construction can only fail on OOM.
fn emitNode(out_id: ?*FigNodeId, result: std.mem.Allocator.Error!AST.Node.Id) FigStatus {
    const out = out_id orelse return .invalid_argument;
    const id = result catch return .out_of_memory;
    out.* = id;
    return .ok;
}

pub export fn fig_value_create(out_value: ?*?*FigValue) FigStatus {
    const out = out_value orelse return .invalid_argument;
    out.* = null;
    const allocator = activeAllocator();
    const handle = allocator.create(ValueHandle) catch return .out_of_memory;
    handle.* = .{
        .allocator = allocator,
        .builder = AST.Builder.init(allocator),
        .rendered = std.Io.Writer.Allocating.init(allocator),
        .diag_arena = std.heap.ArenaAllocator.init(allocator),
    };
    out.* = @ptrCast(handle);
    return .ok;
}

pub export fn fig_value_destroy(value: ?*FigValue) void {
    const handle = valueFrom(value) orelse return;
    const allocator = handle.allocator;
    handle.builder.deinit();
    handle.rendered.deinit();
    handle.diag_arena.deinit();
    allocator.destroy(handle);
}

pub export fn fig_value_null(value: ?*FigValue, out_id: ?*FigNodeId) FigStatus {
    const handle = valueFrom(value) orelse return .invalid_argument;
    return emitNode(out_id, handle.builder.addNull());
}

pub export fn fig_value_bool(value: ?*FigValue, b: bool, out_id: ?*FigNodeId) FigStatus {
    const handle = valueFrom(value) orelse return .invalid_argument;
    return emitNode(out_id, handle.builder.addBool(b));
}

pub export fn fig_value_int(value: ?*FigValue, n: i64, out_id: ?*FigNodeId) FigStatus {
    const handle = valueFrom(value) orelse return .invalid_argument;
    return emitNode(out_id, handle.builder.addInt(n));
}

pub export fn fig_value_uint(value: ?*FigValue, n: u64, out_id: ?*FigNodeId) FigStatus {
    const handle = valueFrom(value) orelse return .invalid_argument;
    return emitNode(out_id, handle.builder.addUint(n));
}

/// Add a numeric scalar from already-formatted text. `is_float` records the
/// `number.kind`. This is the float entry point (the canonical float-text policy
/// is the caller's for now — see `AST.Builder.addNumberRaw`) and the escape hatch
/// for integers outside the i64/u64 range.
pub export fn fig_value_number(
    value: ?*FigValue,
    raw_ptr: ?[*]const u8,
    raw_len: usize,
    is_float: bool,
    out_id: ?*FigNodeId,
) FigStatus {
    const handle = valueFrom(value) orelse return .invalid_argument;
    const raw = sliceOf(raw_ptr, raw_len) orelse return .invalid_argument;
    return emitNode(out_id, handle.builder.addNumberRaw(raw, is_float));
}

pub export fn fig_value_string(
    value: ?*FigValue,
    ptr: ?[*]const u8,
    len: usize,
    out_id: ?*FigNodeId,
) FigStatus {
    const handle = valueFrom(value) orelse return .invalid_argument;
    const s = sliceOf(ptr, len) orelse return .invalid_argument;
    return emitNode(out_id, handle.builder.addString(s));
}

pub export fn fig_value_extended(
    value: ?*FigValue,
    kind: c_int,
    text_ptr: ?[*]const u8,
    text_len: usize,
    out_id: ?*FigNodeId,
) FigStatus {
    const handle = valueFrom(value) orelse return .invalid_argument;
    const ext_kind = extKindOf(kind) orelse return .invalid_argument;
    const text = sliceOf(text_ptr, text_len) orelse return .invalid_argument;
    return emitNode(out_id, handle.builder.addExtended(ext_kind, text));
}

pub export fn fig_value_seq(
    value: ?*FigValue,
    items_ptr: ?[*]const FigNodeId,
    items_len: usize,
    out_id: ?*FigNodeId,
) FigStatus {
    const handle = valueFrom(value) orelse return .invalid_argument;
    // FigNodeId, AST.Node.Id are both u32, so the C array is already the Zig
    // slice the builder wants — no copy. Every id must name an existing node.
    const items: []const FigNodeId = if (items_len == 0) &.{} else (items_ptr orelse return .invalid_argument)[0..items_len];
    const count = handle.builder.nodeCount();
    for (items) |id| if (id >= count) return .invalid_argument;
    return emitNode(out_id, handle.builder.addSequence(items));
}

pub export fn fig_value_map(
    value: ?*FigValue,
    entries_ptr: ?[*]const FigKeyValue,
    entries_len: usize,
    out_id: ?*FigNodeId,
) FigStatus {
    const handle = valueFrom(value) orelse return .invalid_argument;
    if (entries_len == 0) return emitNode(out_id, handle.builder.addMapping(&.{}));
    const c_entries = (entries_ptr orelse return .invalid_argument)[0..entries_len];
    const count = handle.builder.nodeCount();
    // FigKeyValue is extern, Builder.Entry is not, so decode rather than cast.
    const entries = handle.allocator.alloc(AST.Builder.Entry, entries_len) catch return .out_of_memory;
    defer handle.allocator.free(entries);
    for (c_entries, entries) |c, *e| {
        if (c.key >= count or c.value >= count) return .invalid_argument;
        e.* = .{ .key = c.key, .value = c.value };
    }
    return emitNode(out_id, handle.builder.addMapping(entries));
}

/// Controls output style for `fig_value_serialize_opts`. A NULL pointer selects
/// the defaults below (the same output as `fig_value_serialize`). `pretty` is
/// honored by JSON, ZON, and TOML (array wrapping); `indent` by JSON and TOML's
/// wrapped arrays; `width` by TOML's inline-vs-section layout. YAML renders with
/// its own fixed layout.
pub const FigSerializeOptions = extern struct {
    /// The caller-reported size of this struct (set to `sizeof`). Acts as a
    /// version tag: fields may be appended in later releases, and a given field
    /// is read only when `size` covers it (see `serializeOptionsOf`), so a
    /// struct laid out by an older caller stays valid with new fields defaulted.
    size: u32 = @sizeOf(FigSerializeOptions),
    /// Nonzero (default): multi-line, indented output. Zero: compact single-line.
    /// For TOML, zero keeps every array on one line; nonzero lets a wide array
    /// wrap (see `width`).
    pretty: u8 = 1,
    /// Spaces per indent level when `pretty` is nonzero (JSON, and TOML's wrapped
    /// arrays). 0 is treated as the default (2).
    indent: u8 = 2,
    /// Nonzero: drop comments carried on the value instead of emitting them.
    /// Zero (default): preserve them where the target format allows. Appended
    /// after `indent`, so older callers (whose `size` predates this field) keep
    /// the preserve-comments default.
    strip_comments: u8 = 0,
    /// `fig_document_serialize` only. Nonzero: preserve values the target format
    /// cannot represent natively (a null in TOML, a TOML datetime in JSON, …)
    /// through a `$fig` envelope, and decode any such envelope found in the
    /// source. Zero (default): lossy — an unrepresentable value yields
    /// `unsupported_format`. Ignored by `fig_value_serialize_opts` (the value
    /// builder has no source envelopes to decode). Appended after
    /// `strip_comments`, so older callers keep the lossy default.
    lossless: u8 = 0,
    /// TOML, YAML, and fig: the column budget driving their inline-vs-expanded
    /// layout. A mapping/array that renders within `width` columns stays inline
    /// (`k = { ... }` / `[a, b]`); a wider one expands to a `[section]` / a
    /// wrapped array / block lines. For YAML the budget governs NESTED containers
    /// only — a root mapping/sequence always renders block.
    ///
    /// 0 is treated as the default (80), NOT as "never inline": a C caller that
    /// zero-initializes this struct (`{0}` plus a valid `size`) must not silently
    /// get block layout everywhere. Pass 1 to force block. Appended after `lossless`,
    /// so older callers (smaller `size`) keep the 80-column default. Two bytes, so
    /// the struct gains trailing padding to a 12-byte size.
    width: u16 = 80,
    /// `fig_value_serialize_opts` + `FIG_FORMAT_FIG` only: nonzero renders a
    /// container root as inline *flow* (`[a, b]` / `{ k = v }`) instead of the
    /// block spelling. This is the option the language bindings' editor splice
    /// paths set — a fragment spliced after `key = ` has no valid block
    /// spelling in the fig dialect, so flow is the only round-trippable form.
    /// Zero (default): unchanged block rendering. Appended after `width`; it
    /// occupies what was the 12-byte layout's trailing padding, so the struct
    /// size does not change — a zero-initialized older caller reads as the
    /// default (off), matching the other appended fields' forward-compat rule.
    flow: u8 = 0,
    /// `fig_value_serialize_opts` only: nonzero renders the value as the
    /// editor takes it spliced into a document — see `AST.SerializeOptions.
    /// splice`. The bindings' editor splice paths set it. Appended after
    /// `flow`, in the last byte of the 12-byte layout's padding, so the size
    /// does not change and an older caller reads as the default (off).
    splice: u8 = 0,
};

/// Whether a caller-reported options `size` fully covers `field`. Fields beyond
/// `size` are absent in the caller's (possibly older) layout and must not be
/// read; they take their defaults instead.
fn optionCovers(size: u32, comptime field: []const u8) bool {
    const end = @offsetOf(FigSerializeOptions, field) + @sizeOf(@FieldType(FigSerializeOptions, field));
    return size >= end;
}

/// Translate the C options struct (NULL ⇒ defaults) into the core options.
/// Each field is honored only when the caller's `size` includes it, so the
/// struct can gain fields without breaking callers compiled against an older
/// layout (and a zeroed/under-sized `size` reads as all-defaults, not garbage).
fn serializeOptionsOf(options: ?*const FigSerializeOptions) AST.SerializeOptions {
    const o = options orelse return .{};
    var out: AST.SerializeOptions = .{};
    if (optionCovers(o.size, "pretty")) out.pretty = o.pretty != 0;
    if (optionCovers(o.size, "indent")) out.indent = if (o.indent == 0) 2 else o.indent;
    if (optionCovers(o.size, "strip_comments")) out.strip_comments = o.strip_comments != 0;
    if (optionCovers(o.size, "width")) out.width = if (o.width == 0) 80 else o.width;
    if (optionCovers(o.size, "flow")) out.flow = o.flow != 0;
    if (optionCovers(o.size, "splice")) out.splice = o.splice != 0;
    return out;
}

/// Whether the caller asked for lossless conversion (`fig_document_serialize`).
/// NULL options, or a `size` that predates the `lossless` field, read as lossy —
/// the same forward-compat rule the other option fields follow.
fn losslessRequested(options: ?*const FigSerializeOptions) bool {
    const o = options orelse return false;
    return optionCovers(o.size, "lossless") and o.lossless != 0;
}

/// Render the value subtree rooted at `root` in `format`. Output bytes are
/// borrowed from the handle and valid until the next `fig_value_serialize` or
/// `fig_value_destroy`.
pub export fn fig_value_serialize(
    value: ?*FigValue,
    root: FigNodeId,
    format: c_int,
    out_ptr: ?*[*c]const u8,
    out_len: ?*usize,
) FigStatus {
    return fig_value_serialize_opts(value, root, format, null, out_ptr, out_len);
}

/// As `fig_value_serialize`, but `options` (NULL ⇒ defaults) controls output
/// style such as compact vs. pretty-printed JSON.
pub export fn fig_value_serialize_opts(
    value: ?*FigValue,
    root: FigNodeId,
    format: c_int,
    options: ?*const FigSerializeOptions,
    out_ptr: ?*[*c]const u8,
    out_len: ?*usize,
) FigStatus {
    const p = out_ptr orelse return .invalid_argument;
    const l = out_len orelse return .invalid_argument;
    const handle = valueFrom(value) orelse return .invalid_argument;
    const target = targetOf(format) orelse return .unsupported_format;
    if (root >= handle.builder.nodeCount()) return .invalid_argument;

    handle.rendered.clearRetainingCapacity();
    const ast = handle.builder.view(root) catch return .out_of_memory; // borrows the builder; never deinit'd
    // Fragment mode: this is a caller-built `Value`, never a whole document, so
    // a fig scalar/null root renders its ordinary spelling instead of erroring
    // `UnrepresentableRoot` (that rule is for `fig_document_serialize`/CLI
    // whole-document output — see `AST.serializeFragmentWith`). A runtime
    // printer is handed the subtree from `root` and decides for itself.
    switch (target) {
        .canonical => unreachable, // `targetOf` never yields it
        .language => |e| Runtime.printNodeWith(e, &handle.rendered.writer, &ast, root, serializeOptionsOf(options)) catch |err| return runtimePrintStatus(err),
    }

    const bytes = handle.rendered.written();
    p.* = bytes.ptr;
    l.* = bytes.len;
    return .ok;
}

// ==================
// DOCUMENT SERIALIZE (CONVERT)
// ==================
//
// Render a parsed `FigDocument` to any writable format — the cross-format
// conversion primitive. Unlike `fig_value_serialize` (which prints a value the
// caller built), this runs the same pipeline the CLI's `get` does over a source
// document: when leaving YAML it collapses the reference layer (aliases → copies,
// merges → flattened, tags applied/dropped) so a non-YAML printer never meets one,
// and — when `options->lossless` is set — round-trips values the target cannot
// hold natively through a `$fig` envelope. Comments carried on the source survive
// where the target allows, which the parse→rebuild→`fig_value_serialize` detour
// cannot do.

/// Translate a materialization failure onto `FigStatus`. Allocation failure is
/// `out_of_memory`; every other case is an un-collapsible reference layer
/// (undefined/cyclic alias, unknown/mismatched tag) that the target cannot
/// represent — mirroring the `UnresolvedAlias → unsupported_format` convention in
/// `serializeStatus`.
fn convertStatus(err: anyerror) FigStatus {
    return switch (err) {
        error.OutOfMemory => .out_of_memory,
        else => .unsupported_format,
    };
}

/// Render the whole parsed document in `format`. `options` (NULL ⇒ defaults)
/// controls output style and, via `lossless`, whether unrepresentable values are
/// preserved through a `$fig` envelope (default: lossy, so such a value yields
/// `unsupported_format`). Output bytes are borrowed from the document handle and
/// valid until the next `fig_document_serialize` on it or `fig_document_destroy`.
pub export fn fig_document_serialize(
    doc: ?*FigDocument,
    format: c_int,
    options: ?*const FigSerializeOptions,
    out_ptr: ?*[*c]const u8,
    out_len: ?*usize,
) FigStatus {
    const p = out_ptr orelse return .invalid_argument;
    const l = out_len orelse return .invalid_argument;
    const public_doc = doc orelse return .invalid_argument;
    const handle: *DocumentHandle = @ptrCast(@alignCast(public_doc));
    const target = targetOf(format) orelse return .unsupported_format;
    const opts = serializeOptionsOf(options);

    // Intermediate ASTs (materialized / lossless-decoded / -encoded) live in this
    // arena. Their string slices borrow from the source AST, which outlives the
    // call; the rendered bytes are copied into `handle.rendered` before the arena
    // is freed, so nothing dangles.
    var arena_state = std.heap.ArenaAllocator.init(handle.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    const ast = prepareDocumentAst(handle, target, options, arena) catch |err| return convertStatus(err);

    handle.rendered.clearRetainingCapacity();
    switch (target) {
        .canonical => unreachable, // `targetOf` never yields it
        .language => |e| Runtime.printWith(e, &handle.rendered.writer, ast, opts) catch |err| return runtimePrintStatus(err),
    }
    const bytes = handle.rendered.written();
    p.* = bytes.ptr;
    l.* = bytes.len;
    return .ok;
}

/// Whether a source or target carries a reference layer (`Caps.references`).
fn sourceCarriesReferences(handle: *const DocumentHandle) bool {
    return handle.entry.language.caps.references;
}

fn targetCarriesReferences(target: Target) bool {
    return switch (target) {
        .language => |e| e.language.caps.references,
        .canonical => false,
    };
}

/// Run the same source→target AST pipeline `fig_document_serialize` prints from:
/// collapse the reference layer when leaving a language that has one for one
/// that has not (strict tags), then — under `lossless` — decode any `$fig`
/// envelopes and re-encode for the target. All intermediate ASTs live in
/// `arena` (their strings borrow the source AST, which outlives the call).
/// Returns the source AST unchanged when no transform applies. Errors:
/// `OutOfMemory`, or an un-collapsible reference layer from `Materialize` —
/// both mapped via `convertStatus` at the call site.
fn prepareDocumentAst(handle: *DocumentHandle, target: Target, options: ?*const FigSerializeOptions, arena: std.mem.Allocator) !*const AST {
    @setEvalBranchQuota(30_000);

    // Whether a reference layer is being written back out to a language that
    // spells one — the YAML→YAML case, where the layer already round-trips
    // and both passes below would only strip it. Each side is asked, not
    // named: `Caps.references` is the declaration, so a runtime twin of YAML
    // answers as the compiled one does, on either side.
    const source_refs = sourceCarriesReferences(handle);
    const ref_layer_round_trip = source_refs and targetCarriesReferences(target);

    // Leaving that layer for a language without one: collapse it first
    // (strict tag mode, matching the CLI default — unknown/custom tags become
    // `unsupported_format`).
    const base_ast: *const AST = if (source_refs and !ref_layer_round_trip) blk: {
        const mat = try arena.create(AST);
        mat.* = try Materialize.materialize(arena, &handle.document.ast, .strict);
        break :blk mat;
    } else &handle.document.ast;

    // Lossless: decode any `$fig` envelopes in the source back to real kinds, then
    // re-encode for the target. Skipped for YAML→YAML (its reference layer already
    // round-trips, and the core-AST passes would strip it).
    if (losslessRequested(options) and !ref_layer_round_trip) {
        // A null answer is the format's own `caps.lossless` declaration (see
        // `manifest.Caps.lossless` for the per-format rationale — figl,
        // INI/dotenv/.properties/plist/NestedText have no envelope of their
        // own to encode into).
        const native = Lossless.nativeFor(target);
        const decoded = try arena.create(AST);
        decoded.* = try Lossless.decode(arena, base_ast);
        const n = native orelse return decoded;
        const encoded = try arena.create(AST);
        encoded.* = try Lossless.encode(arena, decoded, n);
        return encoded;
    }
    return base_ast;
}

// ==================
// DIAGNOSTICS
// ==================
//
// Report what a conversion would silently lose (comments dropped/degraded,
// values dropped/degraded). It runs the SAME pipeline `*_serialize` prints from,
// so the warnings reflect the actual output. Read-only on the document/value
// *content*, but NOT a const operation: it resets and fills the handle's
// `diag_arena` scratch, so for the threading rules it counts as a mutating call
// on the handle (hence the non-const handle pointer) — do not run it concurrently
// with any other call on the same handle, including the genuinely-const reads.
// The returned array — and its `path` strings — are borrowed from the handle and
// valid only until the next diagnose on it or its destroy.

/// Mirrors `Diagnostics.Warning.Code`.
pub const FigWarningCode = enum(c_int) {
    /// A carried comment is not emitted at all.
    comment_dropped = 0,
    /// A block comment is rendered as a run of line comments.
    comment_style_degraded = 1,
    /// A node is removed entirely (the target cannot represent it even degraded).
    value_dropped = 2,
    /// An extended/non-finite value is rendered as a poorer type.
    type_degraded = 3,
};

/// Mirrors `Diagnostics.Warning.Cause`.
pub const FigWarningCause = enum(c_int) {
    /// The target format inherently cannot represent it.
    format_limitation = 0,
    /// A caller option forced it (e.g. `strip_comments`).
    explicit_option = 1,
};

/// One lossy event, retrieved by index via `fig_*_warning`. Caller-allocated and
/// size-versioned (mirrors `FigError`/`FigSerializeOptions`): the library writes
/// only the fields `out.size` covers, so the struct can gain fields without
/// breaking an older caller's layout. `path`/`note` are NOT null-terminated — use
/// the paired `*_len`. `path` is the dotted/`[i]` location ("" / len 0 = document
/// root); `note` is the degraded-to type for `type_degraded`, else empty. Both
/// borrow the producing handle's diagnostics arena.
pub const FigWarning = extern struct {
    size: u32,
    code: c_int,
    cause: c_int,
    path: [*c]const u8,
    path_len: usize,
    note: [*c]const u8,
    note_len: usize,
};

/// Whether the caller-reported `FigWarning.size` covers `field` (same rule as
/// `optionCovers`/`errCovers`).
fn warnCovers(size: u32, comptime field: []const u8) bool {
    const end = @offsetOf(FigWarning, field) + @sizeOf(@FieldType(FigWarning, field));
    return size >= end;
}

/// Copy `w` into caller-allocated `out`, writing only the fields its `size`
/// covers (preserving `out.size`). The `path`/`note` pointers borrow the same
/// storage `w` does — the producing handle's `diag_arena`.
fn writeWarning(out: *FigWarning, w: Diagnostics.Warning) void {
    const size = out.size;
    if (warnCovers(size, "code")) out.code = warningCodeInt(w.code);
    if (warnCovers(size, "cause")) out.cause = warningCauseInt(w.cause);
    if (warnCovers(size, "path")) out.path = w.path.ptr;
    if (warnCovers(size, "path_len")) out.path_len = w.path.len;
    if (warnCovers(size, "note")) out.note = w.note.ptr;
    if (warnCovers(size, "note_len")) out.note_len = w.note.len;
}

fn warningCodeInt(code: Diagnostics.Warning.Code) c_int {
    return @backingInt(@as(FigWarningCode, switch (code) {
        .comment_dropped => .comment_dropped,
        .comment_style_degraded => .comment_style_degraded,
        .value_dropped => .value_dropped,
        .type_degraded => .type_degraded,
    }));
}

fn warningCauseInt(cause: Diagnostics.Warning.Cause) c_int {
    return @backingInt(@as(FigWarningCause, switch (cause) {
        .format_limitation => .format_limitation,
        .explicit_option => .explicit_option,
    }));
}

/// Build `Diagnostics.Options` from the serialize options (NULL ⇒ defaults).
fn diagnoseOptionsOf(options: ?*const FigSerializeOptions) Diagnostics.Options {
    const so = serializeOptionsOf(options);
    return .{
        .pretty = so.pretty,
        .strip_comments = so.strip_comments,
        .lossless = losslessRequested(options),
    };
}

/// Report HOW MANY events serializing the parsed document to `format` would
/// produce, using the same pipeline (YAML collapse, lossless envelopes)
/// `fig_document_serialize` would. `options` (NULL ⇒ defaults) supplies
/// `pretty`/`strip_comments`/`lossless`, which change what is lost. On success
/// writes the count to `*out_count` (0 if nothing is lost) and retains the set on
/// the handle for `fig_document_warning` to index, valid until the next
/// `fig_document_diagnose` on `doc` or its destroy.
pub export fn fig_document_diagnose(
    doc: ?*FigDocument,
    format: c_int,
    options: ?*const FigSerializeOptions,
    out_count: ?*usize,
) FigStatus {
    const oc = out_count orelse return .invalid_argument;
    // Clear the out-param up front so an early error return (unsupported_format,
    // out_of_memory) never leaves a caller reading a stale count. Mirrors
    // `fig_parse` nulling its out-handle.
    oc.* = 0;
    const public_doc = doc orelse return .invalid_argument;
    const handle: *DocumentHandle = @ptrCast(@alignCast(public_doc));
    // A runtime target is held to what its record declares — see
    // `Diagnostics.analyzeFor`.
    const target = targetOf(format) orelse return .unsupported_format;

    _ = handle.diag_arena.reset(.retain_capacity);
    handle.diag_warnings = &.{}; // a failed analyze must not leave a stale set indexable
    const arena = handle.diag_arena.allocator();
    const ast = prepareDocumentAst(handle, target, options, arena) catch |err| return convertStatus(err);
    const warnings = Diagnostics.analyze(arena, ast, ast.root, target, diagnoseOptionsOf(options)) catch return .out_of_memory;
    handle.diag_warnings = warnings;
    oc.* = warnings.len;
    return .ok;
}

/// Copy the warning at `index` from the most recent `fig_document_diagnose` on
/// `doc` into caller-allocated `*out` (set `out->size` first). An out-of-range
/// `index`, or a call with no prior diagnose, is `invalid_argument`. The
/// `path`/`note` pointers written borrow `doc` (see `fig_document_diagnose`).
pub export fn fig_document_warning(doc: ?*FigDocument, index: usize, out: ?*FigWarning) FigStatus {
    const o = out orelse return .invalid_argument;
    const public_doc = doc orelse return .invalid_argument;
    const handle: *DocumentHandle = @ptrCast(@alignCast(public_doc));
    if (index >= handle.diag_warnings.len) return .invalid_argument;
    writeWarning(o, handle.diag_warnings[index]);
    return .ok;
}

/// Report how many events serializing the built value subtree rooted at `root` to
/// `format` would produce. The value builder has no source envelopes, so
/// `lossless` in `options` is ignored here (it only affects
/// `fig_document_diagnose`). Retention rules match `fig_document_diagnose`.
pub export fn fig_value_diagnose(
    value: ?*FigValue,
    root: FigNodeId,
    format: c_int,
    options: ?*const FigSerializeOptions,
    out_count: ?*usize,
) FigStatus {
    const oc = out_count orelse return .invalid_argument;
    // Clear up front so an early error return never leaves a stale count exposed
    // (see `fig_document_diagnose`).
    oc.* = 0;
    const handle = valueFrom(value) orelse return .invalid_argument;
    const target = targetOf(format) orelse return .unsupported_format; // as fig_document_diagnose
    if (root >= handle.builder.nodeCount()) return .invalid_argument;

    _ = handle.diag_arena.reset(.retain_capacity);
    handle.diag_warnings = &.{};
    const arena = handle.diag_arena.allocator();
    const ast = handle.builder.view(root) catch return .out_of_memory; // borrows the builder; never deinit'd
    const warnings = Diagnostics.analyze(arena, &ast, root, target, diagnoseOptionsOf(options)) catch return .out_of_memory;
    handle.diag_warnings = warnings;
    oc.* = warnings.len;
    return .ok;
}

/// Copy the warning at `index` from the most recent `fig_value_diagnose` on
/// `value` into caller-allocated `*out` (set `out->size` first). Out-of-range
/// `index` or no prior diagnose is `invalid_argument`.
pub export fn fig_value_warning(value: ?*FigValue, index: usize, out: ?*FigWarning) FigStatus {
    const o = out orelse return .invalid_argument;
    const handle = valueFrom(value) orelse return .invalid_argument;
    if (index >= handle.diag_warnings.len) return .invalid_argument;
    writeWarning(o, handle.diag_warnings[index]);
    return .ok;
}

/// The handle of the registered dialect `name`, for a test.
fn fmtOf(name: [:0]const u8) c_int {
    const f = fig_format_by_name(name);
    std.debug.assert(f >= 0);
    return f;
}

test "traversal over a parsed mapping" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const src = "title: Hello\ncount: 42\ntags:\n- a\n- b\n";

    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("yaml"), &out_doc));
    defer fig_document_destroy(out_doc);

    const doc: ?*const FigDocument = out_doc;
    const root = fig_document_root(doc);
    try std.testing.expect(root != fig_node_none);
    try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.mapping)), fig_node_kind(doc, root));
    try std.testing.expectEqual(@as(usize, 3), fig_node_child_count(doc, root));

    // First entry: title -> "Hello"
    const first = fig_node_first_child(doc, root);
    try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.keyvalue)), fig_node_kind(doc, first));
    const key = fig_keyvalue_key(doc, first);
    const val = fig_keyvalue_value(doc, first);

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expect(fig_node_string(doc, key, &ptr, &len));
    try std.testing.expectEqualStrings("title", ptr[0..len]);
    try std.testing.expect(fig_node_string(doc, val, &ptr, &len));
    try std.testing.expectEqualStrings("Hello", ptr[0..len]);

    // Second entry: count -> 42 (integer)
    const second = fig_node_next_sibling(doc, first);
    const count_val = fig_keyvalue_value(doc, second);
    try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.int)), fig_node_kind(doc, count_val));
    try std.testing.expect(fig_node_number(doc, count_val, &ptr, &len));
    try std.testing.expectEqualStrings("42", ptr[0..len]);

    // Third entry: tags -> [a, b]
    const third = fig_node_next_sibling(doc, second);
    const tags_val = fig_keyvalue_value(doc, third);
    try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.sequence)), fig_node_kind(doc, tags_val));
    try std.testing.expectEqual(@as(usize, 2), fig_node_child_count(doc, tags_val));
}

test "a spec version is a dialect of its own, found by name" {
    if (comptime !(build_options.lang_yaml and build_options.lang_toml)) return error.SkipZigTest;
    // YAML 1.1 resolves `yes` as a boolean; the default, 1.2.2, as a string.
    const yes = "a: yes\n";
    for ([_]struct { [:0]const u8, FigNodeKind }{ .{ "yaml", .string }, .{ "yaml-1.2.2", .string }, .{ "yaml-1.1", .bool_ } }) |c| {
        var out_doc: ?*FigDocument = null;
        try std.testing.expectEqual(FigStatus.ok, fig_parse(yes.ptr, yes.len, fmtOf(c[0]), &out_doc));
        defer fig_document_destroy(out_doc);
        const doc: ?*const FigDocument = out_doc;
        const value = fig_keyvalue_value(doc, fig_node_first_child(doc, fig_document_root(doc)));
        try std.testing.expectEqual(@as(c_int, @backingInt(c[1])), fig_node_kind(doc, value));
    }

    // A trailing comma in an inline table is TOML 1.1's; 1.0 refuses it.
    const trailing = "t = { a = 1, }\n";
    for ([_]struct { [:0]const u8, FigStatus }{ .{ "toml", .ok }, .{ "toml-1.1", .ok }, .{ "toml-1.0", .parse_error }, .{ "toml-1.0.0", .parse_error } }) |c| {
        var out_doc: ?*FigDocument = null;
        try std.testing.expectEqual(c[1], fig_parse(trailing.ptr, trailing.len, fmtOf(c[0]), &out_doc));
        fig_document_destroy(out_doc);
    }

    // A version neither language declares is no dialect.
    try std.testing.expectEqual(@as(c_int, -1), fig_format_by_name("toml-2.0"));
    try std.testing.expectEqual(@as(c_int, -1), fig_format_by_name("yaml-1.3"));
}

test "fig_document_diagnose reports a dropped null for TOML" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    if (comptime !(build_options.lang_yaml and build_options.lang_toml)) return error.SkipZigTest;
    const src = "a: null\nb: 1\n";

    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("yaml"), &out_doc));
    defer fig_document_destroy(out_doc);

    var count: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_document_diagnose(out_doc, fmtOf("toml"), null, &count));
    try std.testing.expectEqual(@as(usize, 1), count);
    var w: FigWarning = undefined;
    w.size = @sizeOf(FigWarning);
    try std.testing.expectEqual(FigStatus.ok, fig_document_warning(out_doc, 0, &w));
    try std.testing.expectEqual(@backingInt(FigWarningCode.value_dropped), w.code);
    try std.testing.expectEqual(@backingInt(FigWarningCause.format_limitation), w.cause);
    try std.testing.expectEqualStrings("a", w.path[0..w.path_len]);
    // An out-of-range index is rejected.
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_document_warning(out_doc, 1, &w));

    // Lossless preserves the null → no warnings.
    var opts: FigSerializeOptions = .{ .lossless = 1 };
    try std.testing.expectEqual(FigStatus.ok, fig_document_diagnose(out_doc, fmtOf("toml"), &opts, &count));
    try std.testing.expectEqual(@as(usize, 0), count);
    // With nothing reported, even index 0 is out of range.
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_document_warning(out_doc, 0, &w));

    // A representable target loses nothing.
    try std.testing.expectEqual(FigStatus.ok, fig_document_diagnose(out_doc, fmtOf("json"), null, &count));
    try std.testing.expectEqual(@as(usize, 0), count);
}

test "fig_value_diagnose reports a degraded datetime" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    var v: ?*FigValue = null;
    try std.testing.expectEqual(FigStatus.ok, fig_value_create(&v));
    defer fig_value_destroy(v);

    const ts = "1979-05-27T07:32:00Z";
    var dt: FigNodeId = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_extended(v, @backingInt(FigExtKind.offset_datetime), ts.ptr, ts.len, &dt));

    var count: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_diagnose(v, dt, fmtOf("json"), null, &count));
    try std.testing.expectEqual(@as(usize, 1), count);
    var w: FigWarning = undefined;
    w.size = @sizeOf(FigWarning);
    try std.testing.expectEqual(FigStatus.ok, fig_value_warning(v, 0, &w));
    try std.testing.expectEqual(@backingInt(FigWarningCode.type_degraded), w.code);
    try std.testing.expectEqualStrings("string", w.note[0..w.note_len]);

    // TOML holds datetimes natively → no warning.
    if (comptime build_options.lang_toml) {
        try std.testing.expectEqual(FigStatus.ok, fig_value_diagnose(v, dt, fmtOf("toml"), null, &count));
        try std.testing.expectEqual(@as(usize, 0), count);
    }
}

test "fig_value_warning honors a truncated (size-gated) FigWarning" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    var v: ?*FigValue = null;
    try std.testing.expectEqual(FigStatus.ok, fig_value_create(&v));
    defer fig_value_destroy(v);

    const ts = "1979-05-27T07:32:00Z";
    var dt: FigNodeId = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_extended(v, @backingInt(FigExtKind.offset_datetime), ts.ptr, ts.len, &dt));

    var count: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_diagnose(v, dt, fmtOf("json"), null, &count));
    try std.testing.expectEqual(@as(usize, 1), count);

    // A caller whose layout stops after `cause` (an older/smaller struct) gets
    // code+cause but not the path/note fields, which keep their prior contents.
    var w: FigWarning = undefined;
    w.size = @offsetOf(FigWarning, "cause") + @sizeOf(c_int);
    w.path = null;
    w.path_len = 12345;
    try std.testing.expectEqual(FigStatus.ok, fig_value_warning(v, 0, &w));
    try std.testing.expectEqual(@backingInt(FigWarningCode.type_degraded), w.code);
    try std.testing.expectEqual(@as(usize, 12345), w.path_len); // beyond `size`: untouched
}

test "fig_parse_ex fills FigError on a parse failure" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    const bad = "{ \"a\":"; // truncated JSON object

    // No out_err: behaves exactly like fig_parse.
    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(
        FigStatus.parse_error,
        fig_parse_ex(bad.ptr, bad.len, fmtOf("json"), &out_doc, null),
    );
    try std.testing.expectEqual(@as(?*FigDocument, null), out_doc);

    // With out_err: code mirrors the status, a non-empty NUL-terminated message
    // is written, and the offset, where the language reports one, is within
    // the input.
    var err: FigError = undefined;
    err.size = @sizeOf(FigError);
    try std.testing.expectEqual(
        FigStatus.parse_error,
        fig_parse_ex(bad.ptr, bad.len, fmtOf("json"), &out_doc, &err),
    );
    try std.testing.expectEqual(@backingInt(FigStatus.parse_error), err.code);
    try std.testing.expect(err.message_len > 0);
    try std.testing.expectEqual(@as(u8, 0), err.message[err.message_len]); // NUL-terminated
    try std.testing.expect(err.byte_offset <= bad.len);

    // An unsupported format reports through the same struct.
    err.size = @sizeOf(FigError);
    try std.testing.expectEqual(
        FigStatus.unsupported_format,
        fig_parse_ex(bad.ptr, bad.len, 0xBEEF, &out_doc, &err),
    );
    try std.testing.expectEqual(@backingInt(FigStatus.unsupported_format), err.code);
}

test "fig_parse_ex honors a truncated (size-gated) FigError" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    const bad = "[1,";

    // A caller whose layout stops before `message` gets `code` filled but the
    // message fields left as they were — no out-of-bounds write into a struct
    // that does not declare them.
    var err: FigError = undefined;
    err.size = @offsetOf(FigError, "byte_offset"); // covers size+code only
    err.message_len = 999;
    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(
        FigStatus.parse_error,
        fig_parse_ex(bad.ptr, bad.len, fmtOf("json"), &out_doc, &err),
    );
    try std.testing.expectEqual(@backingInt(FigStatus.parse_error), err.code);
    try std.testing.expectEqual(@as(usize, 999), err.message_len); // beyond `size`: untouched
}

test "fig_parse_ex leaves out_doc null and succeeds on a valid parse" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    const src = "{\"a\":1}";
    var out_doc: ?*FigDocument = null;
    var err: FigError = undefined;
    err.size = @sizeOf(FigError);
    try std.testing.expectEqual(
        FigStatus.ok,
        fig_parse_ex(src.ptr, src.len, fmtOf("json"), &out_doc, &err),
    );
    defer fig_document_destroy(out_doc);
    try std.testing.expect(out_doc != null);
}

test "a compiled-out format is unsupported in serialize and diagnose alike" {
    // Only meaningful when a writable format is gated out of this build; the
    // default all-on build has nothing to probe. TOML is the stand-in.
    if (comptime build_options.lang_toml) return error.SkipZigTest;

    const src = "{\"a\": 1}"; // JSON is always compiled in, so the parse succeeds.
    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("json"), &out_doc));
    defer fig_document_destroy(out_doc);

    // Serialize rejects the gated-out target...
    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(
        FigStatus.unsupported_format,
        fig_document_serialize(out_doc, fmtOf("toml"), null, &ptr, &len),
    );

    // ...and diagnose agrees, instead of silently accepting it. The count is
    // cleared on the error path (pre-initialized up front).
    var count: usize = 999;
    try std.testing.expectEqual(
        FigStatus.unsupported_format,
        fig_document_diagnose(out_doc, fmtOf("toml"), null, &count),
    );
    try std.testing.expectEqual(@as(usize, 0), count);

    // Capabilities report the same verdict: nothing for a compiled-out format.
    try std.testing.expectEqual(@as(u32, 0), fig_format_capabilities(fmtOf("toml")));
}

test "parse c abi reads toml and zon" {
    if (comptime !(build_options.lang_toml and build_options.lang_zon)) return error.SkipZigTest;
    // TOML
    {
        var out_doc: ?*FigDocument = null;
        const src = "name = \"fig\"\ncount = 42\n";
        try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("toml"), &out_doc));
        defer fig_document_destroy(out_doc);
        const root = fig_document_root(out_doc);
        try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.mapping)), fig_node_kind(out_doc, root));
        try std.testing.expectEqual(@as(usize, 2), fig_node_child_count(out_doc, root));
    }
    // ZON
    {
        var out_doc: ?*FigDocument = null;
        const src = ".{ .name = \"fig\", .count = 42 }";
        try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("zon"), &out_doc));
        defer fig_document_destroy(out_doc);
        const root = fig_document_root(out_doc);
        try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.mapping)), fig_node_kind(out_doc, root));
    }
}

test "parse c abi reads json5 and rejects it under strict json" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    // Regression: the `fig_parse` format switch once dropped `json5`, so the
    // `.json5` reader arm was dead and every JSON5 parse returned
    // `unsupported_format`. JSON5-only syntax (unquoted keys, trailing comma,
    // `//` comment) must parse under `.json5` and fail under strict `.json`.
    const src = "{\n  // c\n  host: 'localhost',\n  port: 8080,\n}\n";

    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("json5"), &out_doc));
    defer fig_document_destroy(out_doc);
    const root = fig_document_root(out_doc);
    try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.mapping)), fig_node_kind(out_doc, root));
    try std.testing.expectEqual(@as(usize, 2), fig_node_child_count(out_doc, root));

    var strict_doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.parse_error, fig_parse(src.ptr, src.len, fmtOf("json"), &strict_doc));
}

test "fig_node_extended recovers datetime and char-literal scalars" {
    if (comptime !(build_options.lang_toml and build_options.lang_zon)) return error.SkipZigTest;

    var kind: c_int = undefined;
    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;

    // TOML local date: kind reports EXTENDED; fig_node_extended says which.
    {
        var out_doc: ?*FigDocument = null;
        const src = "d = 2026-06-18\n";
        try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("toml"), &out_doc));
        defer fig_document_destroy(out_doc);
        const val = fig_keyvalue_value(out_doc, fig_node_first_child(out_doc, fig_document_root(out_doc)));
        try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.extended)), fig_node_kind(out_doc, val));
        try std.testing.expect(fig_node_extended(out_doc, val, &kind, &ptr, &len));
        try std.testing.expectEqual(@backingInt(FigExtKind.local_date), kind);
        try std.testing.expectEqualStrings("2026-06-18", ptr[0..len]);
    }

    // ZON char literal: kind reports EXTENDED; fig_node_extended gives the codepoint.
    {
        var out_doc: ?*FigDocument = null;
        const src = ".{ .c = 'a' }";
        try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("zon"), &out_doc));
        defer fig_document_destroy(out_doc);
        const val = fig_keyvalue_value(out_doc, fig_node_first_child(out_doc, fig_document_root(out_doc)));
        try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.extended)), fig_node_kind(out_doc, val));
        try std.testing.expect(fig_node_extended(out_doc, val, &kind, &ptr, &len));
        try std.testing.expectEqual(@backingInt(FigExtKind.char_literal), kind);
        try std.testing.expectEqualStrings("97", ptr[0..len]);
    }

    // A plain string is not extended.
    {
        var out_doc: ?*FigDocument = null;
        const src = "s = \"hi\"\n";
        try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("toml"), &out_doc));
        defer fig_document_destroy(out_doc);
        const val = fig_keyvalue_value(out_doc, fig_node_first_child(out_doc, fig_document_root(out_doc)));
        try std.testing.expect(!fig_node_extended(out_doc, val, &kind, &ptr, &len));
    }
}

fn keySeg(s: []const u8) FigPathSegment {
    return .{ .kind = 0, .key_ptr = s.ptr, .key_len = s.len, .index = 0 };
}

test "editor c abi insert + source round-trip" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const src = "a: 1\nb: 2\n";
    var out_ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("yaml"), &out_ed));
    defer fig_editor_destroy(out_ed);

    const c = "c";
    const three = "3";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_insert_key(out_ed, null, 0, c.ptr, c.len, three.ptr, three.len));

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(out_ed, &ptr, &len));
    try std.testing.expectEqualStrings("a: 1\nb: 2\nc: 3\n", ptr[0..len]);
}

test "editor c abi accepts empty input as an empty document" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var out_ed: ?*FigEditor = null;
    // Null pointer + zero length is a valid empty document.
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(null, 0, fmtOf("yaml"), &out_ed));
    defer fig_editor_destroy(out_ed);

    const k = "k";
    const v = "v";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_insert_key(out_ed, null, 0, k.ptr, k.len, v.ptr, v.len));

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(out_ed, &ptr, &len));
    try std.testing.expectEqualStrings("k: v\n", ptr[0..len]);
}

test "toml editor c abi insert + replace + delete round-trip" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    const src = "[server]\nhost = \"a\"\nport = 1\n";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("toml"), &ed));
    defer fig_editor_destroy(ed);

    // Insert a key into the [server] table (lands at the end of the table region).
    const server = [_]FigPathSegment{keySeg("server")};
    const tls = "tls";
    const tval = "true";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_insert_key(ed, &server, 1, tls.ptr, tls.len, tval.ptr, tval.len));

    // Replace a nested value through a multi-segment C path.
    const port = [_]FigPathSegment{ keySeg("server"), keySeg("port") };
    const nine = "9090";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_replace_val(ed, &port, 2, nine.ptr, nine.len));

    // Delete a key.
    const host = [_]FigPathSegment{ keySeg("server"), keySeg("host") };
    try std.testing.expectEqual(FigStatus.ok, fig_editor_delete_key(ed, &host, 2));

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings("[server]\nport = 9090\ntls = true\n", ptr[0..len]);
}

test "toml editor c abi add leading comment uses the # marker" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    const src = "a = 1\nb = 2\n";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("toml"), &ed));
    defer fig_editor_destroy(ed);

    const b = [_]FigPathSegment{keySeg("b")};
    const note = "note";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_add_leading_comment(ed, &b, 1, note.ptr, note.len));

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings("a = 1\n# note\nb = 2\n", ptr[0..len]);
}

test "toml editor c abi maps a shape-mismatch edit to invalid_argument" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    const src = "a = 1\n";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("toml"), &ed));
    defer fig_editor_destroy(ed);

    // Inserting a key that already exists rolls back with error.DuplicateKey,
    // which must surface as invalid_argument (a caller error), NOT parse_error.
    const a = "a";
    const two = "2";
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_insert_key(ed, null, 0, a.ptr, a.len, two.ptr, two.len));
    // The editor is still usable after the rolled-back edit.
    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings("a = 1\n", ptr[0..len]);
}

test "yaml editor c abi refuses inserting a key that already exists" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // YAML's parser accepts a repeated key, so this used to return ok and
    // leave `a: 1\na: 2\n`; the engine refuses it now, by name or by syntax.
    const src = "a: 1\n";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("yaml"), &ed));
    defer fig_editor_destroy(ed);
    const a = "a";
    const two = "2";
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_insert_named_key(ed, null, 0, a.ptr, a.len, two.ptr, two.len));
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_insert_key(ed, null, 0, a.ptr, a.len, two.ptr, two.len));
    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings("a: 1\n", ptr[0..len]);
}

fn figStr(s: []const u8) FigStr {
    return .{ .ptr = s.ptr, .len = s.len };
}

test "value c abi builds and serializes to multiple formats" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    var out_value: ?*FigValue = null;
    try std.testing.expectEqual(FigStatus.ok, fig_value_create(&out_value));
    defer fig_value_destroy(out_value);

    // Build { "name": "fig", "nums": [1, 2] } bottom-up.
    var id: FigNodeId = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(out_value, "fig", 3, &id));
    const v_name = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_int(out_value, 1, &id));
    const n1 = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_int(out_value, 2, &id));
    const n2 = id;
    const items = [_]FigNodeId{ n1, n2 };
    try std.testing.expectEqual(FigStatus.ok, fig_value_seq(out_value, &items, items.len, &id));
    const v_nums = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(out_value, "name", 4, &id));
    const k_name = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(out_value, "nums", 4, &id));
    const k_nums = id;
    const entries = [_]FigKeyValue{ .{ .key = k_name, .value = v_name }, .{ .key = k_nums, .value = v_nums } };
    try std.testing.expectEqual(FigStatus.ok, fig_value_map(out_value, &entries, entries.len, &id));
    const root = id;

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_serialize(out_value, root, fmtOf("json"), &ptr, &len));
    try std.testing.expectEqualStrings("{\n  \"name\": \"fig\",\n  \"nums\": [\n    1,\n    2\n  ]\n}\n", ptr[0..len]);

    // Same value, different format — the borrowed bytes are refreshed in place.
    if (comptime build_options.lang_yaml) {
        try std.testing.expectEqual(FigStatus.ok, fig_value_serialize(out_value, root, fmtOf("yaml"), &ptr, &len));
        try std.testing.expectEqualStrings("name: fig\nnums: [1, 2]\n", ptr[0..len]);
    }
}

test "value c abi maps an unrepresentable value to unsupported_format" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    var out_value: ?*FigValue = null;
    try std.testing.expectEqual(FigStatus.ok, fig_value_create(&out_value));
    defer fig_value_destroy(out_value);

    // { "k": null } — TOML has no null, so serializing to TOML must fail cleanly.
    var id: FigNodeId = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_null(out_value, &id));
    const v_null = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(out_value, "k", 1, &id));
    const k = id;
    const entries = [_]FigKeyValue{.{ .key = k, .value = v_null }};
    try std.testing.expectEqual(FigStatus.ok, fig_value_map(out_value, &entries, entries.len, &id));
    const root = id;

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.unsupported_format, fig_value_serialize(out_value, root, fmtOf("toml"), &ptr, &len));
    // The same value serializes fine to a format that has null.
    try std.testing.expectEqual(FigStatus.ok, fig_value_serialize(out_value, root, fmtOf("json"), &ptr, &len));
    try std.testing.expectEqualStrings("{\n  \"k\": null\n}\n", ptr[0..len]);
}

test "value c abi spells a non-finite float in each format's own words, and JSON refuses it" {
    if (comptime !build_options.lang_json or !build_options.lang_toml or !build_options.lang_yaml) return error.SkipZigTest;
    // A binding hands a float's text to `fig_value_number` in YAML's spelling
    // (`.inf`). Each printer used to write that verbatim: TOML got `.inf` and
    // JSON `0.inf`, neither of which the format reads back.
    const cases = [_]struct { raw: []const u8, toml: []const u8, yaml: []const u8, json5: []const u8 }{
        .{ .raw = ".inf", .toml = "v = inf\n", .yaml = "v: .inf\n", .json5 = "{\n  v: Infinity\n}\n" },
        .{ .raw = "-.inf", .toml = "v = -inf\n", .yaml = "v: -.inf\n", .json5 = "{\n  v: -Infinity\n}\n" },
        .{ .raw = ".nan", .toml = "v = nan\n", .yaml = "v: .nan\n", .json5 = "{\n  v: NaN\n}\n" },
        // TOML's own spelling, as a TOML document's parse stores it.
        .{ .raw = "inf", .toml = "v = inf\n", .yaml = "v: .inf\n", .json5 = "{\n  v: Infinity\n}\n" },
    };
    for (cases) |c| {
        var out_value: ?*FigValue = null;
        try std.testing.expectEqual(FigStatus.ok, fig_value_create(&out_value));
        defer fig_value_destroy(out_value);
        var id: FigNodeId = undefined;
        try std.testing.expectEqual(FigStatus.ok, fig_value_number(out_value, c.raw.ptr, c.raw.len, true, &id));
        const v = id;
        try std.testing.expectEqual(FigStatus.ok, fig_value_string(out_value, "v", 1, &id));
        const entries = [_]FigKeyValue{.{ .key = id, .value = v }};
        try std.testing.expectEqual(FigStatus.ok, fig_value_map(out_value, &entries, entries.len, &id));
        const root = id;

        var ptr: [*c]const u8 = undefined;
        var len: usize = undefined;
        try std.testing.expectEqual(FigStatus.ok, fig_value_serialize(out_value, root, fmtOf("toml"), &ptr, &len));
        try std.testing.expectEqualStrings(c.toml, ptr[0..len]);
        try std.testing.expectEqual(FigStatus.ok, fig_value_serialize(out_value, root, fmtOf("yaml"), &ptr, &len));
        try std.testing.expectEqualStrings(c.yaml, ptr[0..len]);
        try std.testing.expectEqual(FigStatus.ok, fig_value_serialize(out_value, root, fmtOf("json5"), &ptr, &len));
        try std.testing.expectEqualStrings(c.json5, ptr[0..len]);
        // JSON and JSONC have no infinity or NaN: refused, as TOML refuses a null.
        try std.testing.expectEqual(FigStatus.unsupported_format, fig_value_serialize(out_value, root, fmtOf("json"), &ptr, &len));
        try std.testing.expectEqual(FigStatus.unsupported_format, fig_value_serialize(out_value, root, fmtOf("jsonc"), &ptr, &len));
    }
}

test "document c abi converts a non-finite float into the target's spelling" {
    if (comptime !build_options.lang_json or !build_options.lang_toml or !build_options.lang_yaml) return error.SkipZigTest;
    const cases = [_]struct { src: []const u8, from: [:0]const u8, to: [:0]const u8, want: ?[]const u8 }{
        .{ .src = "a: .inf\nb: -.inf\nc: .nan\n", .from = "yaml", .to = "toml", .want = "a = inf\nb = -inf\nc = nan\n" },
        .{ .src = "a = inf\nb = -inf\nc = nan\n", .from = "toml", .to = "yaml", .want = "a: .inf\nb: -.inf\nc: .nan\n" },
        .{ .src = "a: .inf\n", .from = "yaml", .to = "json", .want = null },
    };
    for (cases) |c| {
        var doc: ?*FigDocument = null;
        try std.testing.expectEqual(FigStatus.ok, fig_parse(c.src.ptr, c.src.len, fmtOf(c.from), &doc));
        defer fig_document_destroy(doc);
        var ptr: [*c]const u8 = undefined;
        var len: usize = undefined;
        const status = fig_document_serialize(doc, fmtOf(c.to), null, &ptr, &len);
        if (c.want) |want| {
            try std.testing.expectEqual(FigStatus.ok, status);
            try std.testing.expectEqualStrings(want, ptr[0..len]);
        } else try std.testing.expectEqual(FigStatus.unsupported_format, status);
    }
}

test "toml editor c abi takes inf and nan as a replacement value" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    const src = "name = \"music\"\nversion = 1\n";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("toml"), &ed));
    defer fig_editor_destroy(ed);
    const path = [_]FigPathSegment{keySeg("version")};
    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    inline for (.{ "inf", "-inf", "nan" }) |repl| {
        try std.testing.expectEqual(FigStatus.ok, fig_editor_replace_val(ed, &path, 1, repl.ptr, repl.len));
        try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
        try std.testing.expectEqualStrings("name = \"music\"\nversion = " ++ repl ++ "\n", ptr[0..len]);
    }
}

test "value c abi serialize options honor the size/version field" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    var out_value: ?*FigValue = null;
    try std.testing.expectEqual(FigStatus.ok, fig_value_create(&out_value));
    defer fig_value_destroy(out_value);

    // [ 1, 2 ] — exercise pretty on/off via the options struct.
    var id: FigNodeId = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_int(out_value, 1, &id));
    const a = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_int(out_value, 2, &id));
    const b = id;
    const items = [_]FigNodeId{ a, b };
    try std.testing.expectEqual(FigStatus.ok, fig_value_seq(out_value, &items, items.len, &id));
    const root = id;

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;

    // A fully-populated options struct: compact output.
    var opts: FigSerializeOptions = .{ .pretty = 0 };
    try std.testing.expectEqual(FigStatus.ok, fig_value_serialize_opts(out_value, root, fmtOf("json"), &opts, &ptr, &len));
    try std.testing.expectEqualStrings("[1,2]\n", ptr[0..len]);

    // A `size` that does not reach `pretty` must leave it (and `indent`) at the
    // default — i.e. behave as if those fields were absent, not read as garbage.
    // This is the forward-compat contract: an older/under-sized layout defaults.
    opts.size = @offsetOf(FigSerializeOptions, "pretty"); // covers only `size`
    try std.testing.expectEqual(FigStatus.ok, fig_value_serialize_opts(out_value, root, fmtOf("json"), &opts, &ptr, &len));
    try std.testing.expectEqualStrings("[\n  1,\n  2\n]\n", ptr[0..len]);
}

test "value c abi serialize options carry TOML width through to the inline/section choice" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    var out_value: ?*FigValue = null;
    try std.testing.expectEqual(FigStatus.ok, fig_value_create(&out_value));
    defer fig_value_destroy(out_value);

    // { point: { x = 1, y = 2 } } — a small mapping value whose layout flips on
    // the width budget.
    var id: FigNodeId = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_int(out_value, 1, &id));
    const x_val = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_int(out_value, 2, &id));
    const y_val = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(out_value, "x", 1, &id));
    const x_key = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(out_value, "y", 1, &id));
    const y_key = id;
    const inner = [_]FigKeyValue{ .{ .key = x_key, .value = x_val }, .{ .key = y_key, .value = y_val } };
    try std.testing.expectEqual(FigStatus.ok, fig_value_map(out_value, &inner, inner.len, &id));
    const point_val = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(out_value, "point", 5, &id));
    const point_key = id;
    const outer = [_]FigKeyValue{.{ .key = point_key, .value = point_val }};
    try std.testing.expectEqual(FigStatus.ok, fig_value_map(out_value, &outer, outer.len, &id));
    const root = id;

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;

    // Default width (80): the mapping fits, so it stays an inline table.
    try std.testing.expectEqual(FigStatus.ok, fig_value_serialize(out_value, root, fmtOf("toml"), &ptr, &len));
    try std.testing.expectEqualStrings("point = { x = 1, y = 2 }\n", ptr[0..len]);

    // A tight width budget forces it to expand to a [section].
    const opts: FigSerializeOptions = .{ .width = 8 };
    try std.testing.expectEqual(FigStatus.ok, fig_value_serialize_opts(out_value, root, fmtOf("toml"), &opts, &ptr, &len));
    try std.testing.expectEqualStrings("[point]\nx = 1\ny = 2\n", ptr[0..len]);
}

test "value c abi flow option renders fig-dialect container fragments inline" {
    if (comptime !build_options.lang_figl) return error.SkipZigTest;
    var out_value: ?*FigValue = null;
    try std.testing.expectEqual(FigStatus.ok, fig_value_create(&out_value));
    defer fig_value_destroy(out_value);

    // ["a.md", "b.md"] — the shape the editors splice after `key = `.
    var id: FigNodeId = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(out_value, "a.md", 4, &id));
    const a = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(out_value, "b.md", 4, &id));
    const b = id;
    const items = [_]FigNodeId{ a, b };
    try std.testing.expectEqual(FigStatus.ok, fig_value_seq(out_value, &items, items.len, &id));
    const root = id;

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;

    // flow set: the inline spelling — the only one that survives a splice.
    const flow_opts: FigSerializeOptions = .{ .flow = 1 };
    try std.testing.expectEqual(FigStatus.ok, fig_value_serialize_opts(out_value, root, fmtOf("figl"), &flow_opts, &ptr, &len));
    try std.testing.expectEqualStrings("[a.md, b.md]\n", ptr[0..len]);

    // flow unset (default): unchanged block rendering.
    try std.testing.expectEqual(FigStatus.ok, fig_value_serialize(out_value, root, fmtOf("figl"), &ptr, &len));
    try std.testing.expectEqualStrings("* a.md\n* b.md\n", ptr[0..len]);
}

test "value c abi rejects an out-of-range child id" {
    var out_value: ?*FigValue = null;
    try std.testing.expectEqual(FigStatus.ok, fig_value_create(&out_value));
    defer fig_value_destroy(out_value);

    var id: FigNodeId = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_int(out_value, 1, &id));
    // id 99 was never created.
    const items = [_]FigNodeId{ id, 99 };
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_value_seq(out_value, &items, items.len, &id));
}

test "fig_parse empty input is judged per format" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    // (null ptr, len 0) reaches the parser — same as (ptr, len 0). YAML treats
    // an empty stream as a null document and TOML as an empty table (both ok);
    // JSON requires a value (parse_error). A null ptr with a nonzero len is a
    // malformed argument regardless of format.
    {
        var out_doc: ?*FigDocument = null;
        try std.testing.expectEqual(FigStatus.parse_error, fig_parse(null, 0, fmtOf("json"), &out_doc));
        try std.testing.expect(out_doc == null);
    }
    {
        var out_doc: ?*FigDocument = null;
        try std.testing.expectEqual(FigStatus.invalid_argument, fig_parse(null, 5, fmtOf("json"), &out_doc));
        try std.testing.expect(out_doc == null);
    }
    if (comptime build_options.lang_yaml) {
        var out_doc: ?*FigDocument = null;
        try std.testing.expectEqual(FigStatus.ok, fig_parse(null, 0, fmtOf("yaml"), &out_doc));
        defer fig_document_destroy(out_doc);
        try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.null_)), fig_node_kind(out_doc, fig_document_root(out_doc)));
    }
    if (comptime build_options.lang_toml) {
        var out_doc: ?*FigDocument = null;
        try std.testing.expectEqual(FigStatus.ok, fig_parse(null, 0, fmtOf("toml"), &out_doc));
        defer fig_document_destroy(out_doc);
        try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.mapping)), fig_node_kind(out_doc, fig_document_root(out_doc)));
        try std.testing.expectEqual(@as(usize, 0), fig_node_child_count(out_doc, fig_document_root(out_doc)));
    }
}

test "fig_document_serialize converts JSON to YAML" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const src = "{\"name\":\"fig\",\"nums\":[1,2]}";
    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("json"), &out_doc));
    defer fig_document_destroy(out_doc);

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(out_doc, fmtOf("yaml"), null, &ptr, &len));
    try std.testing.expectEqualStrings("name: fig\nnums: [1, 2]\n", ptr[0..len]);

    // Same handle, re-serialize to TOML — the borrowed bytes refresh in place.
    if (comptime build_options.lang_toml) {
        try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(out_doc, fmtOf("toml"), null, &ptr, &len));
        try std.testing.expectEqualStrings("name = \"fig\"\nnums = [1, 2]\n", ptr[0..len]);
    }
}

test "fig_parse parses the fig authoring dialect" {
    if (comptime !build_options.lang_figl) return error.SkipZigTest;
    const src = "title = Hello\ncount = 42\n";
    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("figl"), &out_doc));
    defer fig_document_destroy(out_doc);

    if (comptime build_options.lang_json) {
        var ptr: [*c]const u8 = undefined;
        var len: usize = undefined;
        try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(out_doc, fmtOf("json"), null, &ptr, &len));
        try std.testing.expectEqualStrings("{\n  \"title\": \"Hello\",\n  \"count\": 42\n}\n", ptr[0..len]);
    }
}

test "fig_document_serialize converts JSON to the fig authoring dialect" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    if (comptime !build_options.lang_figl) return error.SkipZigTest;
    const src = "{\"name\":\"fig\",\"nums\":[1,2]}";
    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("json"), &out_doc));
    defer fig_document_destroy(out_doc);

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(out_doc, fmtOf("figl"), null, &ptr, &len));
    try std.testing.expectEqualStrings("name = fig\nnums = [1, 2]\n", ptr[0..len]);
}

test "fig_editor_create edits the fig authoring dialect" {
    if (comptime !build_options.lang_figl) return error.SkipZigTest;
    const src = "title = old\nport = 8080\n";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("figl"), &ed));
    defer fig_editor_destroy(ed);

    const path = [_]FigPathSegment{keySeg("port")};
    const repl = "9090";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_replace_val(ed, &path, 1, repl.ptr, repl.len));

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings("title = old\nport = 9090\n", ptr[0..len]);
}

test "fig_editor_create edits ZON through the C ABI" {
    if (comptime !build_options.lang_zon) return error.SkipZigTest;
    const src = ".{ .title = \"old\", .port = 8080 }";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("zon"), &ed));
    defer fig_editor_destroy(ed);

    const path = [_]FigPathSegment{keySeg("port")};
    const repl = "9090";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_replace_val(ed, &path, 1, repl.ptr, repl.len));

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings(".{ .title = \"old\", .port = 9090 }", ptr[0..len]);
}

test "fig_editor_insert_named_key spells the name as the format does" {
    // `insert_key` takes key SYNTAX; a binding that has a name from its
    // caller used to spell it as a string value, which is `"new"` in ZON —
    // a string where a `.new` field must be. The named entry spells it.
    if (comptime build_options.lang_zon) {
        const src = ".{ .a = 1 }";
        var ed: ?*FigEditor = null;
        try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("zon"), &ed));
        defer fig_editor_destroy(ed);
        const name = "new";
        const val = "true";
        try std.testing.expectEqual(FigStatus.ok, fig_editor_insert_named_key(ed, null, 0, name.ptr, name.len, val.ptr, val.len));
        const spaced = "has space";
        try std.testing.expectEqual(FigStatus.ok, fig_editor_insert_named_key(ed, null, 0, spaced.ptr, spaced.len, val.ptr, val.len));
        try expectEditorSource(ed, ".{ .a = 1, .new = true, .@\"has space\" = true }");
    }
    if (comptime build_options.lang_json) {
        const src = "{\"a\": 1}";
        var ed: ?*FigEditor = null;
        try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("json"), &ed));
        defer fig_editor_destroy(ed);
        const name = "k\"q";
        const val = "2";
        try std.testing.expectEqual(FigStatus.ok, fig_editor_insert_named_key(ed, null, 0, name.ptr, name.len, val.ptr, val.len));
        try expectEditorSource(ed, "{\"a\": 1, \"k\\\"q\": 2}");
    }
}

test "fig_editor_replace_named_key spells the new name as the format does" {
    // ZON's key span is the field name after its `.`, so a rename keeps
    // the dot where it stands and spells only the name.
    if (comptime build_options.lang_zon) {
        const src = ".{ .a = 1, .b = 2 }";
        var ed: ?*FigEditor = null;
        try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("zon"), &ed));
        defer fig_editor_destroy(ed);
        const a = [_]FigPathSegment{keySeg("a")};
        const b = [_]FigPathSegment{keySeg("b")};
        const k = "k";
        const spaced = "has space";
        try std.testing.expectEqual(FigStatus.ok, fig_editor_replace_named_key(ed, &a, 1, k.ptr, k.len));
        try std.testing.expectEqual(FigStatus.ok, fig_editor_replace_named_key(ed, &b, 1, spaced.ptr, spaced.len));
        try expectEditorSource(ed, ".{ .k = 1, .@\"has space\" = 2 }");
    }
    if (comptime build_options.lang_plist) {
        const src = "<dict><key>a</key><string>x</string></dict>";
        var ed: ?*FigEditor = null;
        try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("plist"), &ed));
        defer fig_editor_destroy(ed);
        const a = [_]FigPathSegment{keySeg("a")};
        const name = "b&c";
        try std.testing.expectEqual(FigStatus.ok, fig_editor_replace_named_key(ed, &a, 1, name.ptr, name.len));
        try expectEditorSource(ed, "<dict><key>b&amp;c</key><string>x</string></dict>");
    }
}

test "fig_editor_replace_key refuses a path with no key and a name already held" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    const src = "{\"l\": [1, 2], \"a\": 1, \"b\": 2}";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("json"), &ed));
    defer fig_editor_destroy(ed);
    // A sequence item has no key: the old splice overwrote the item's value.
    const item = [_]FigPathSegment{ keySeg("l"), .{ .kind = 1, .key_ptr = null, .key_len = 0, .index = 0 } };
    const z = "\"z\"";
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_replace_key(ed, &item, item.len, z.ptr, z.len));
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_replace_key(ed, null, 0, z.ptr, z.len));
    // JSON's parser accepts a repeated key, so the engine refuses it.
    const a = [_]FigPathSegment{keySeg("a")};
    const b = "b";
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_replace_named_key(ed, &a, 1, b.ptr, b.len));
    const b_syntax = "\"b\"";
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_replace_key(ed, &a, 1, b_syntax.ptr, b_syntax.len));
    try expectEditorSource(ed, src);
}

/// Read back an editor's source, for the whole-container tests below.
fn expectEditorSource(ed: ?*FigEditor, expected: []const u8) !void {
    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings(expected, ptr[0..len]);
}

test "fig_editor whole-container ops reach a TOML table the key ops cannot" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    // The pairing these exports exist for: at a `[header]` path the key ops
    // refuse (the editor's pre-op guards), and the container op does the job.
    const src = "[a]\nx = 1\n[b]\ny = 2\n";
    const path_a = [_]FigPathSegment{keySeg("a")};
    const path_b = [_]FigPathSegment{keySeg("b")};

    {
        var ed: ?*FigEditor = null;
        try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("toml"), &ed));
        defer fig_editor_destroy(ed);
        // Refused by the key op…
        try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_delete_key(ed, &path_a, 1));
        // …done by the container op, which takes the body with the header.
        try std.testing.expectEqual(FigStatus.ok, fig_editor_delete_container(ed, &path_a, 1));
        try expectEditorSource(ed, "[b]\ny = 2\n");
    }
    {
        var ed: ?*FigEditor = null;
        try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("toml"), &ed));
        defer fig_editor_destroy(ed);
        const leaf = "q";
        try std.testing.expectEqual(FigStatus.ok, fig_editor_rename_container(ed, &path_a, 1, leaf.ptr, leaf.len));
        try expectEditorSource(ed, "[q]\nx = 1\n[b]\ny = 2\n");
    }
    {
        var ed: ?*FigEditor = null;
        try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("toml"), &ed));
        defer fig_editor_destroy(ed);
        const body = "z = 3\n";
        const path_c = [_]FigPathSegment{keySeg("c")};
        try std.testing.expectEqual(FigStatus.ok, fig_editor_insert_container(ed, &path_c, 1, body.ptr, body.len));
        try expectEditorSource(ed, "[a]\nx = 1\n[b]\ny = 2\n\n[c]\nz = 3\n");
    }
    {
        var ed: ?*FigEditor = null;
        try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("toml"), &ed));
        defer fig_editor_destroy(ed);
        // The generic move is refused; the container move relocates the whole
        // table. A NULL destination is "to EOF", which is why it cannot be
        // spelled as a zero-length path (that is the root).
        try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_move_key(ed, &path_a, 1, &path_b, 1));
        try std.testing.expectEqual(FigStatus.ok, fig_editor_move_container(ed, &path_a, 1, null, 0));
        try expectEditorSource(ed, "[b]\ny = 2\n\n[a]\nx = 1\n");
    }
    {
        var ed: ?*FigEditor = null;
        try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("toml"), &ed));
        defer fig_editor_destroy(ed);
        const order = [_]FigStr{ figStr("b"), figStr("a") };
        // `fig_editor_reorder_keys` at the root would have moved the header
        // lines alone; this moves the tables.
        try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_reorder_keys(ed, &.{}, 0, &order, order.len));
        try std.testing.expectEqual(FigStatus.ok, fig_editor_reorder_containers(ed, &order, order.len));
        try expectEditorSource(ed, "[b]\ny = 2\n[a]\nx = 1\n");
    }
}

test "fig_editor_append_container_to_seq appends a TOML array-of-tables element" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    const src = "[[bin]]\nname = \"a\"\n";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("toml"), &ed));
    defer fig_editor_destroy(ed);
    const path = [_]FigPathSegment{keySeg("bin")};
    const body = "name = \"b\"\n";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_append_container_to_seq(ed, &path, 1, body.ptr, body.len));
    try expectEditorSource(ed, "[[bin]]\nname = \"a\"\n\n[[bin]]\nname = \"b\"\n");
}

test "fig_editor whole-container ops answer unsupported_format where the format has none" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    if (comptime !build_options.lang_ini) return error.SkipZigTest;
    const path = [_]FigPathSegment{keySeg("a")};
    // YAML nests its containers in one contiguous region, so it declares none of
    // the six — the key ops already handle a YAML block mapping (which is why
    // `fig_editor_delete_key` below succeeds where TOML's refuses).
    {
        const src = "a:\n  x: 1\nb:\n  y: 2\n";
        var ed: ?*FigEditor = null;
        try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("yaml"), &ed));
        defer fig_editor_destroy(ed);
        try std.testing.expectEqual(FigStatus.unsupported_format, fig_editor_delete_container(ed, &path, 1));
        try std.testing.expectEqual(FigStatus.ok, fig_editor_delete_key(ed, &path, 1));
        try expectEditorSource(ed, "b:\n  y: 2\n");
    }
    // INI has the delete/move/reorder three and rename — its parser records
    // every `[section]` header's name, reopenings included, so a rename
    // reaches each — but not the two that write a header line: it declares
    // no `section_header`, and a section has no `[[array]]` form.
    {
        const src = "[a]\nx = 1\n[b]\ny = 2\n[a]\nz = 3\n";
        var ed: ?*FigEditor = null;
        try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("ini"), &ed));
        defer fig_editor_destroy(ed);
        const leaf = "q";
        const body = "";
        try std.testing.expectEqual(FigStatus.unsupported_format, fig_editor_insert_container(ed, &path, 1, body.ptr, body.len));
        try std.testing.expectEqual(FigStatus.ok, fig_editor_rename_container(ed, &path, 1, leaf.ptr, leaf.len));
        try expectEditorSource(ed, "[q]\nx = 1\n[b]\ny = 2\n[q]\nz = 3\n");
        const q = [_]FigPathSegment{keySeg("q")};
        try std.testing.expectEqual(FigStatus.ok, fig_editor_delete_container(ed, &q, 1));
        try expectEditorSource(ed, "[b]\ny = 2\n");
    }
}

test "fig_document_serialize materializes the YAML reference layer when leaving YAML" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // `b` aliases the anchor defined on `a`; converting to JSON must expand it to
    // a copied value, not leak `*x` or fail with unsupported_format.
    const src = "a: &x 1\nb: *x\n";
    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("yaml"), &out_doc));
    defer fig_document_destroy(out_doc);

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(out_doc, fmtOf("json"), null, &ptr, &len));
    try std.testing.expectEqualStrings("{\n  \"a\": 1,\n  \"b\": 1\n}\n", ptr[0..len]);

    // YAML→YAML keeps the reference layer intact (no materialize).
    try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(out_doc, fmtOf("yaml"), null, &ptr, &len));
    try std.testing.expect(std.mem.indexOf(u8, ptr[0..len], "*x") != null);
}

/// The AST behind a document handle, for a test to walk by path.
fn testAst(doc: ?*FigDocument) *const AST {
    return &handleFrom(doc).?.document.ast;
}

test "fig_document_resolve_aliases reads an alias and a merge key as values" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const src =
        \\base: &b {x: 1, y: 2}
        \\name: &n fig
        \\d:
        \\  <<: *b
        \\  y: 3
        \\alias: *n
        \\
    ;
    var doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("yaml"), &doc));
    var resolved: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_document_resolve_aliases(doc, &resolved, null));
    defer fig_document_destroy(resolved);
    // The new document owns what it reads: the parse it came from may go first.
    fig_document_destroy(doc);

    const ast = testAst(resolved);
    try std.testing.expectEqualStrings("fig", (try ast.getValByPath(&.{.{ .key = "alias" }})).kind.string);
    try std.testing.expectEqualStrings("1", (try ast.getValByPath(&.{ .{ .key = "d" }, .{ .key = "x" } })).kind.number.raw);
    try std.testing.expectEqualStrings("3", (try ast.getValByPath(&.{ .{ .key = "d" }, .{ .key = "y" } })).kind.number.raw);
    try std.testing.expectError(error.NotFound, ast.getValByPath(&.{ .{ .key = "d" }, .{ .key = "<<" } }));
    const d = fig_node_kind(resolved, (try ast.getValByPath(&.{.{ .key = "d" }})).id);
    try std.testing.expectEqual(@backingInt(FigNodeKind.mapping), d);
    for (0..ast.nodeCount()) |i| try std.testing.expect(ast.kindOf(@intCast(i)) != .alias);

    // It is a document like any other: it serializes, with no `*` or `&` left.
    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(resolved, fmtOf("yaml"), null, &ptr, &len));
    try std.testing.expect(std.mem.indexOfAny(u8, ptr[0..len], "*&") == null);
}

test "fig_document_resolve_aliases answers NULL for a document with nothing to resolve" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    const cases = [_]struct { src: []const u8, format: [:0]const u8 }{
        .{ .src = "a: &x 1\nb: 2\n", .format = "yaml" }, // an anchor no alias names
        .{ .src = "{\"<<\": {\"a\": 1}}", .format = "json" }, // `<<` is a merge only where there are references
    };
    for (cases) |c| {
        var doc: ?*FigDocument = null;
        try std.testing.expectEqual(FigStatus.ok, fig_parse(c.src.ptr, c.src.len, fmtOf(c.format), &doc));
        defer fig_document_destroy(doc);
        var resolved: ?*FigDocument = @ptrFromInt(0x10);
        try std.testing.expectEqual(FigStatus.ok, fig_document_resolve_aliases(doc, &resolved, null));
        try std.testing.expect(resolved == null);
    }
}

test "fig_document_resolve_aliases refuses a cyclic alias as a parse error at the alias" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const src = "ok: 1\nloop: &a [1, *a]\n";
    var doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("yaml"), &doc));
    defer fig_document_destroy(doc);
    var resolved: ?*FigDocument = null;
    var err: FigError = .empty;
    try std.testing.expectEqual(FigStatus.parse_error, fig_document_resolve_aliases(doc, &resolved, &err));
    try std.testing.expect(resolved == null);
    try std.testing.expectEqual(@backingInt(FigStatus.parse_error), err.code);
    try std.testing.expectEqualStrings("alias `*a` refers to a node that contains it, so it has no value", err.message[0..err.message_len]);
    try std.testing.expectEqual(std.mem.indexOf(u8, src, "*a").?, err.byte_offset);
    try std.testing.expectEqual(@as(u32, 2), err.line);
    try std.testing.expectEqual(@as(u32, 14), err.column);

    // A mapping merged into itself has no value either.
    const self_merge = "m: &m {a: 1, <<: *m}\n";
    var doc2: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(self_merge.ptr, self_merge.len, fmtOf("yaml"), &doc2));
    defer fig_document_destroy(doc2);
    try std.testing.expectEqual(FigStatus.parse_error, fig_document_resolve_aliases(doc2, &resolved, null));
    try std.testing.expect(resolved == null);
}

test "fig_document_serialize honors the lossless option for TOML null" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    // A JSON null has no TOML representation. Lossy (default) reports it; lossless
    // wraps it in a `$fig` envelope so the document still serializes.
    const src = "{\"k\":null}";
    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("json"), &out_doc));
    defer fig_document_destroy(out_doc);

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.unsupported_format, fig_document_serialize(out_doc, fmtOf("toml"), null, &ptr, &len));

    var opts: FigSerializeOptions = .{ .lossless = 1 };
    try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(out_doc, fmtOf("toml"), &opts, &ptr, &len));
    try std.testing.expect(std.mem.indexOf(u8, ptr[0..len], "$fig") != null);
}

test "fig_document_serialize preserves comments across formats" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // The motivating case the parse→rebuild→fig_value_serialize detour could not
    // serve: a comment captured from JSON5 re-emitted into YAML.
    const src = "{\n  // hello\n  a: 1,\n}\n";
    var out_doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, fmtOf("json5"), &out_doc));
    defer fig_document_destroy(out_doc);

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(out_doc, fmtOf("yaml"), null, &ptr, &len));
    try std.testing.expect(std.mem.indexOf(u8, ptr[0..len], "# hello") != null);
    try std.testing.expect(std.mem.indexOf(u8, ptr[0..len], "a: 1") != null);

    // strip_comments drops it.
    var opts: FigSerializeOptions = .{ .strip_comments = 1 };
    try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(out_doc, fmtOf("yaml"), &opts, &ptr, &len));
    try std.testing.expect(std.mem.indexOf(u8, ptr[0..len], "hello") == null);
}

test "fig_version matches build options and string form" {
    const v = fig_version();
    try std.testing.expectEqual(@as(u32, build_options.version_major), v >> 16);
    try std.testing.expectEqual(@as(u32, build_options.version_minor), (v >> 8) & 0xFF);
    try std.testing.expectEqual(@as(u32, build_options.version_patch), v & 0xFF);

    const s = fig_version_string();
    const expected = std.fmt.comptimePrint("{d}.{d}.{d}", .{
        build_options.version_major,
        build_options.version_minor,
        build_options.version_patch,
    });
    try std.testing.expectEqualStrings(expected, std.mem.span(s));
}

test "a runtime language registers through the C ABI and is a peer at every entry point" {
    Runtime.deinitAll();
    defer Runtime.deinitAll();
    var alloc: Runtime.test_language.Alloc = .{ .allocator = std.testing.allocator };
    const vt = Runtime.test_language.vtable(&alloc);

    // Register; the integer resolves by name, and reports the record's
    // capabilities.
    var format: c_int = 0;
    var err: FigError = Runtime.ErrorInfo.empty;
    try std.testing.expectEqual(FigStatus.ok, fig_language_register(&vt, &format, &err));
    try std.testing.expect(format >= 1);
    try std.testing.expectEqual(format, fig_format_by_name("tinykv"));
    try std.testing.expectEqual(@as(c_int, -1), fig_format_by_name("nosuch"));
    const all = @backingInt(FigCapability.read) | @backingInt(FigCapability.edit) | @backingInt(FigCapability.serialize);
    try std.testing.expectEqual(all, fig_format_capabilities(format));
    try std.testing.expectEqual(@as(u32, 0), fig_format_capabilities(format + 7));

    // A second registration of the same name is refused with the reason.
    var again: c_int = 0;
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_language_register(&vt, &again, &err));
    try std.testing.expect(std.mem.indexOf(u8, err.text(), "already registered") != null);
    try std.testing.expectEqual(@as(c_int, -1), again);

    // Parse, walk, convert out — and convert a JSON document in.
    const src = "# note\nx=1\ny=two\n";
    var doc: ?*FigDocument = null;
    try std.testing.expectEqual(FigStatus.ok, fig_parse(src.ptr, src.len, format, &doc));
    defer fig_document_destroy(doc.?);
    const root = fig_document_root(doc);
    try std.testing.expectEqual(@as(c_int, @backingInt(FigNodeKind.mapping)), fig_node_kind(doc, root));
    try std.testing.expectEqual(@as(usize, 2), fig_node_child_count(doc, root));
    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    if (comptime build_options.lang_json) {
        try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(doc, fmtOf("json"), null, &ptr, &len));
        try std.testing.expectEqualStrings("{\n  \"x\": \"1\",\n  \"y\": \"two\"\n}\n", ptr[0..len]);
        const json = "{\"k\": \"v\"}";
        var jdoc: ?*FigDocument = null;
        try std.testing.expectEqual(FigStatus.ok, fig_parse(json.ptr, json.len, fmtOf("json"), &jdoc));
        defer fig_document_destroy(jdoc.?);
        try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(jdoc, format, null, &ptr, &len));
        try std.testing.expectEqualStrings("k=v\n", ptr[0..len]);
        // Diagnose against a runtime target answers from its record:
        // nothing lost from a flat string map, and a table nested past its
        // `max_mapping_depth` of 0 dropped, as the print drops it.
        var n: usize = 9;
        try std.testing.expectEqual(FigStatus.ok, fig_document_diagnose(jdoc, format, null, &n));
        try std.testing.expectEqual(@as(usize, 0), n);
        const nested = "{\"a\": {\"b\": \"1\"}, \"c\": \"2\"}";
        var ndoc: ?*FigDocument = null;
        try std.testing.expectEqual(FigStatus.ok, fig_parse(nested.ptr, nested.len, fmtOf("json"), &ndoc));
        defer fig_document_destroy(ndoc.?);
        try std.testing.expectEqual(FigStatus.ok, fig_document_diagnose(ndoc, format, null, &n));
        try std.testing.expectEqual(@as(usize, 1), n);
        var w: FigWarning = .{ .size = @sizeOf(FigWarning), .code = 0, .cause = 0, .path = null, .path_len = 0, .note = null, .note_len = 0 };
        try std.testing.expectEqual(FigStatus.ok, fig_document_warning(ndoc, 0, &w));
        try std.testing.expectEqual(warningCodeInt(.value_dropped), w.code);
        try std.testing.expectEqualStrings("a", w.path[0..w.path_len]);
    }
    // Print it as itself, comment and all.
    try std.testing.expectEqual(FigStatus.ok, fig_document_serialize(doc, format, null, &ptr, &len));
    try std.testing.expectEqualStrings(src, ptr[0..len]);

    // A parse failure carries the helper's message and offset.
    const bad = "x=1\nnope\n";
    var bad_doc: ?*FigDocument = null;
    var perr: FigError = Runtime.ErrorInfo.empty;
    try std.testing.expectEqual(FigStatus.parse_error, fig_parse_ex(bad.ptr, bad.len, format, &bad_doc, &perr));
    try std.testing.expectEqualStrings("expected key=value", perr.text());
    try std.testing.expectEqual(@as(usize, 4), perr.byte_offset);

    // Edit through the one runtime editor arm.
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, format, &ed));
    defer fig_editor_destroy(ed);
    const seg = [_]FigPathSegment{.{ .kind = 0, .key_ptr = "x", .key_len = 1, .index = 0 }};
    const ten = "10";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_replace_val(ed, &seg, 1, ten.ptr, ten.len));
    const z = "z";
    const three = "3";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_insert_key(ed, null, 0, z.ptr, z.len, three.ptr, three.len));
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings("# note\nx=10\ny=two\nz=3\n", ptr[0..len]);
    // tinykv declares no sections, so the whole-container ops are not its,
    // as they are not YAML's: `unsupported_format`, and nothing changed.
    try std.testing.expectEqual(FigStatus.unsupported_format, fig_editor_delete_container(ed, &seg, 1));
    try std.testing.expectEqual(FigStatus.unsupported_format, fig_editor_insert_container(ed, &seg, 1, three.ptr, three.len));
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings("# note\nx=10\ny=two\nz=3\n", ptr[0..len]);

    // A value built by hand prints through the vtable too.
    var value: ?*FigValue = null;
    try std.testing.expectEqual(FigStatus.ok, fig_value_create(&value));
    defer fig_value_destroy(value);
    var k: FigNodeId = undefined;
    var v: FigNodeId = undefined;
    var m: FigNodeId = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(value, "a", 1, &k));
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(value, "b", 1, &v));
    const pair = [_]FigKeyValue{.{ .key = k, .value = v }};
    try std.testing.expectEqual(FigStatus.ok, fig_value_map(value, &pair, 1, &m));
    try std.testing.expectEqual(FigStatus.ok, fig_value_serialize(value, m, format, &ptr, &len));
    try std.testing.expectEqualStrings("a=b\n", ptr[0..len]);
}

test "fig_format_capabilities reports the per-format matrix" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    const read = @backingInt(FigCapability.read);
    const edit = @backingInt(FigCapability.edit);
    const serialize = @backingInt(FigCapability.serialize);
    const references = @backingInt(FigCapability.references);

    // JSON family: always fully supported, regardless of build options.
    for ([_][:0]const u8{ "json", "jsonc", "json5" }) |f| {
        try std.testing.expectEqual(read | edit | serialize, fig_format_capabilities(fmtOf(f)));
    }

    // Gated formats: capabilities track both inherent support and the build gate.
    // YAML alone has a reference layer.
    try std.testing.expectEqual(
        if (build_options.lang_yaml) read | edit | serialize | references else 0,
        fig_format_capabilities(fmtOf("yaml")),
    );
    try std.testing.expectEqual(
        if (build_options.lang_toml) read | edit | serialize else 0,
        fig_format_capabilities(fmtOf("toml")),
    );
    try std.testing.expectEqual(
        if (build_options.lang_zon) read | edit | serialize else 0,
        fig_format_capabilities(fmtOf("zon")),
    );
    try std.testing.expectEqual(
        if (build_options.lang_figl) read | edit | serialize else 0,
        fig_format_capabilities(fmtOf("figl")),
    );

    // Unknown / out-of-range format values report no capabilities.
    try std.testing.expectEqual(@as(u32, 0), fig_format_capabilities(0));
    try std.testing.expectEqual(@as(u32, 0), fig_format_capabilities(9999));
    try std.testing.expectEqual(@as(u32, 0), fig_format_capabilities(-1));
}

test "fig_format_capabilities agrees with actual READ/EDIT/SERIALIZE behavior" {
    // The capability matrix is only useful if it matches reality. Rather than
    // re-encode the matrix as a hand-maintained constant (which drifts silently
    // when core gains a capability the ABI hasn't surfaced — exactly how TOML
    // editing stayed hidden), assert each advertised bit against what the
    // corresponding entry point actually does on a valid input. This fails the
    // build the moment a format's real capability and its bit diverge — in either
    // direction, and under any build-flag combination.
    const read = @backingInt(FigCapability.read);
    const edit = @backingInt(FigCapability.edit);
    const serialize = @backingInt(FigCapability.serialize);

    const Case = struct { fmt: [:0]const u8, sample: []const u8 };
    const cases = [_]Case{
        .{ .fmt = "json", .sample = "{\"a\":1}" },
        .{ .fmt = "jsonc", .sample = "{\"a\":1}" },
        .{ .fmt = "json5", .sample = "{a:1}" },
        .{ .fmt = "yaml", .sample = "a: 1\n" },
        .{ .fmt = "toml", .sample = "a = 1\n" },
        .{ .fmt = "zon", .sample = ".{ .a = 1 }" },
        .{ .fmt = "figl", .sample = "a = 1\n" },
    };

    // A value every writable format can represent ({"a": 1}); used for the
    // SERIALIZE probe so a failure can only mean "format not writable", never
    // "value unrepresentable in this format".
    var value: ?*FigValue = null;
    try std.testing.expectEqual(FigStatus.ok, fig_value_create(&value));
    defer fig_value_destroy(value);
    var id: FigNodeId = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_value_int(value, 1, &id));
    const v_one = id;
    try std.testing.expectEqual(FigStatus.ok, fig_value_string(value, "a", 1, &id));
    const k_a = id;
    const entries = [_]FigKeyValue{.{ .key = k_a, .value = v_one }};
    try std.testing.expectEqual(FigStatus.ok, fig_value_map(value, &entries, entries.len, &id));
    const root = id;

    for (cases) |c| {
        const fmt = fmtOf(c.fmt);
        const caps = fig_format_capabilities(fmt);

        // READ: a valid sample parses iff the format is compiled in (which is
        // exactly when the read bit is set).
        var doc: ?*FigDocument = null;
        const parse_status = fig_parse(c.sample.ptr, c.sample.len, fmt, &doc);
        defer if (doc != null) fig_document_destroy(doc);
        try std.testing.expectEqual((caps & read) != 0, parse_status == .ok);

        // EDIT: create rejects a non-editable or gated format with
        // unsupported_format before it ever parses, so a valid sample never
        // yields parse_error here — the only non-ok outcome is unsupported_format.
        var ed: ?*FigEditor = null;
        const edit_status = fig_editor_create(c.sample.ptr, c.sample.len, fmt, &ed);
        defer if (ed != null) fig_editor_destroy(ed);
        try std.testing.expectEqual((caps & edit) != 0, edit_status != .unsupported_format);

        // SERIALIZE: rendering a representable value succeeds iff the format is
        // writable in this build.
        var ptr: [*c]const u8 = undefined;
        var len: usize = undefined;
        const ser_status = fig_value_serialize(value, root, fmt, &ptr, &len);
        try std.testing.expectEqual((caps & serialize) != 0, ser_status != .unsupported_format);
    }
}

test "fig_editor comment ops add, set, and delete through the C ABI" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const src = "a: 1\nb: 2\n";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("yaml"), &ed));
    defer fig_editor_destroy(ed);

    // path = ["b"]
    var key = [_]u8{'b'};
    const path = [_]FigPathSegment{.{ .kind = 0, .key_ptr = &key, .key_len = 1, .index = 0 }};

    const leading = "why";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_add_leading_comment(ed, &path, 1, leading.ptr, leading.len));
    const trailing = "two";
    try std.testing.expectEqual(FigStatus.ok, fig_editor_set_trailing_comment(ed, &path, 1, trailing.ptr, trailing.len));

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings("a: 1\n# why\nb: 2 # two\n", ptr[0..len]);

    // Delete both back out.
    try std.testing.expectEqual(FigStatus.ok, fig_editor_delete_trailing_comment(ed, &path, 1));
    try std.testing.expectEqual(FigStatus.ok, fig_editor_delete_leading_comments(ed, &path, 1));
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings("a: 1\nb: 2\n", ptr[0..len]);
}

test "fig_editor_set_sequence reconciles a list, preserving survivors' comments" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const src = "tags:\n- a # first\n- b # second\n- c # third\n";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("yaml"), &ed));
    defer fig_editor_destroy(ed);

    var key = [_]u8{ 't', 'a', 'g', 's' };
    const path = [_]FigPathSegment{.{ .kind = 0, .key_ptr = &key, .key_len = key.len, .index = 0 }};

    // -> [c, a, d]: drop b, add d, reorder. a and c keep their comments.
    const items = [_]FigStr{
        .{ .ptr = "c", .len = 1 },
        .{ .ptr = "a", .len = 1 },
        .{ .ptr = "d", .len = 1 },
    };
    try std.testing.expectEqual(FigStatus.ok, fig_editor_set_sequence(ed, &path, path.len, &items, items.len));

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings("tags:\n- c # third\n- a # first\n- d\n", ptr[0..len]);

    // An empty target is declined with invalid_argument (caller falls back).
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_set_sequence(ed, &path, path.len, &items, 0));
}

test "fig_editor comment ops reject strict JSON with unsupported_format" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    const src = "{\"a\":1}";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("json"), &ed));
    defer fig_editor_destroy(ed);
    var key = [_]u8{'a'};
    const path = [_]FigPathSegment{.{ .kind = 0, .key_ptr = &key, .key_len = 1, .index = 0 }};
    const text = "x";
    try std.testing.expectEqual(FigStatus.unsupported_format, fig_editor_add_leading_comment(ed, &path, 1, text.ptr, text.len));
    try std.testing.expectEqual(FigStatus.unsupported_format, fig_editor_delete_trailing_comment(ed, &path, 1));
    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.unsupported_format, fig_editor_get_leading_comment(ed, &path, 1, &ptr, &len));
    try std.testing.expectEqual(FigStatus.unsupported_format, fig_editor_get_trailing_comment(ed, &path, 1, &ptr, &len));
}

test "fig_editor comment ops on a one-line flow item answer as if it had none" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    // The item sits on `members`' line, so the block above and the comment at
    // the line's end are `members`'. Every op used to reach them through the
    // item; across the ABI that is `invalid_argument` for the writes and
    // `not_found` (= absent) for the reads.
    const src = "# above members\nmembers = [\"a\", \"b\"] # note\n";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("toml"), &ed));
    defer fig_editor_destroy(ed);

    var key = [_]u8{ 'm', 'e', 'm', 'b', 'e', 'r', 's' };
    const item0 = [_]FigPathSegment{
        .{ .kind = 0, .key_ptr = &key, .key_len = key.len, .index = 0 },
        .{ .kind = 1, .key_ptr = null, .key_len = 0, .index = 0 },
    };
    const text = "mine";
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_add_leading_comment(ed, &item0, item0.len, text.ptr, text.len));
    try std.testing.expectEqual(FigStatus.invalid_argument, fig_editor_set_trailing_comment(ed, &item0, item0.len, text.ptr, text.len));

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    try std.testing.expectEqual(FigStatus.not_found, fig_editor_get_leading_comment(ed, &item0, item0.len, &ptr, &len));
    try std.testing.expectEqual(FigStatus.not_found, fig_editor_get_trailing_comment(ed, &item0, item0.len, &ptr, &len));

    // The deletes report success and change nothing.
    try std.testing.expectEqual(FigStatus.ok, fig_editor_delete_leading_comments(ed, &item0, item0.len));
    try std.testing.expectEqual(FigStatus.ok, fig_editor_delete_trailing_comment(ed, &item0, item0.len));
    try std.testing.expectEqual(FigStatus.ok, fig_editor_source(ed, &ptr, &len));
    try std.testing.expectEqualStrings(src, ptr[0..len]);

    // `members` itself still owns both.
    const parent = [_]FigPathSegment{item0[0]};
    try std.testing.expectEqual(FigStatus.ok, fig_editor_get_leading_comment(ed, &parent, 1, &ptr, &len));
    try std.testing.expectEqualStrings("above members", ptr[0..len]);
    try std.testing.expectEqual(FigStatus.ok, fig_editor_get_trailing_comment(ed, &parent, 1, &ptr, &len));
    try std.testing.expectEqualStrings("note", ptr[0..len]);
}

test "fig_editor comment reads return bytes, distinguishing absent from empty" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const src = "# why\na: 1 # two\nb: 2 #\nc: 3\n";
    var ed: ?*FigEditor = null;
    try std.testing.expectEqual(FigStatus.ok, fig_editor_create(src.ptr, src.len, fmtOf("yaml"), &ed));
    defer fig_editor_destroy(ed);

    var ptr: [*c]const u8 = undefined;
    var len: usize = undefined;
    const at = struct {
        fn key(k: *[1]u8) [1]FigPathSegment {
            return .{.{ .kind = 0, .key_ptr = k, .key_len = 1, .index = 0 }};
        }
    };

    // a: present leading ("why") and present trailing ("two").
    var ka = [_]u8{'a'};
    const pa = at.key(&ka);
    try std.testing.expectEqual(FigStatus.ok, fig_editor_get_leading_comment(ed, &pa, 1, &ptr, &len));
    try std.testing.expectEqualStrings("why", ptr[0..len]);
    try std.testing.expectEqual(FigStatus.ok, fig_editor_get_trailing_comment(ed, &pa, 1, &ptr, &len));
    try std.testing.expectEqualStrings("two", ptr[0..len]);

    // b: a bare `#` trailing → present but EMPTY (ok, len 0), not absent.
    var kb = [_]u8{'b'};
    const pb = at.key(&kb);
    try std.testing.expectEqual(FigStatus.ok, fig_editor_get_trailing_comment(ed, &pb, 1, &ptr, &len));
    try std.testing.expectEqual(@as(usize, 0), len);

    // c: no comment either way → not_found (absent).
    var kc = [_]u8{'c'};
    const pc = at.key(&kc);
    try std.testing.expectEqual(FigStatus.not_found, fig_editor_get_leading_comment(ed, &pc, 1, &ptr, &len));
    try std.testing.expectEqual(FigStatus.not_found, fig_editor_get_trailing_comment(ed, &pc, 1, &ptr, &len));
}

test "editStatus: every editor refusal is a caller error, not parse_error" {
    // `editStatus`'s fall-through is `.parse_error` — the right answer for a
    // reparse failure after a splice, and the wrong one for every REFUSAL, which
    // is about the request rather than the source. A new refusal error that
    // nobody adds here inherits that fall-through silently, so this pins the set:
    // each of these is raised by an editor op (not by a parser) and must map to a
    // caller-facing status. INI's and fig's delete guards had drifted this way.
    const refusals = [_]anyerror{
        error.NotFound,                 error.NotAMapping,
        error.NotASequence,             error.NotAContainer,
        error.UnsupportedShape,         error.NotASection,
        error.NotAnInlineArray,         error.NotAnArrayOfTables,
        error.SectionExists,            error.DuplicateKey,
        error.MergeOnlyKey,             error.CannotDeleteSection,
        error.CannotReplaceSection,     error.CannotMoveSection,
        error.CannotReorderSections,    error.EmptyInlineContainer,
        error.KeyRequiresMultilineForm, error.BlockValueIntoFlow,
        error.CommentsUnsupported,      error.NullUnsupported,
        error.MultilineComment,         error.InvalidComment,
        error.CommentsUnanchored,       error.RendererRefused,
        error.ImplicitSection,          error.ContainerClosesOnItsLine,
        error.NotAKey,                  error.NonFiniteNumber,
        error.CommentInsideValue,       error.DeleteChangedOtherValues,
        error.NoEmptyRootList,          error.EditReadsBackWrong,
    };
    for (refusals) |err| {
        const status = editStatus(err);
        if (status == .parse_error) {
            std.log.err("editStatus({s}) is parse_error; add it to the mapping", .{@errorName(err)});
            return error.TestUnexpectedResult;
        }
    }
    // The two fig PARSER errors an edit's reparse surfaces stay parse errors:
    // they describe source that no longer parses, which is what the caller needs
    // to hear.
    try std.testing.expectEqual(FigStatus.parse_error, editStatus(error.FigEmptyContainer));
    try std.testing.expectEqual(FigStatus.parse_error, editStatus(error.FigDuplicateKey));
}
