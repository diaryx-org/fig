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
//!     the node table (`table.zig`) is what `parse` fills and `print` reads:
//!     one row per node in pre-order plus sparse side facts, which is
//!     `Document` restated as values (proposal §4.9). They are `extern
//!     struct`s because
//!     they ARE the C ABI: `c_api.zig` re-exports them under their `Fig*`
//!     names, fig.h states them, and `zig build abi-check` holds the two
//!     together. A Zig host fills the same structs.
//!   * **The registry.** `register` validates a vtable by the rules
//!     `Language.validate` applies at comptime, runs the harness of
//!     `harness.zig` over the samples it declares, and — only then — appends
//!     one `Entry` per dialect row and returns the first's ABI integer: the
//!     row's index plus one, so that 0 is never a format. Append-only under
//!     a mutex; an entry
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
//! The conversions between the node table (`table.zig`) and `Document` are
//! the boundary's cost. Fig owns the table a `parse` fills, so no memory a
//! language allocated outlives the call; `tableToDocument` copies what the
//! AST keeps out of it, and `documentToTable` builds the one a `print`
//! reads. What a runtime
//! format cannot do is add a node kind: `ExtKind` is closed, and a table row
//! naming one that does not exist is refused at conversion.

const std = @import("std");
const builtin = @import("builtin");
const build_options = @import("build_options");
const Allocator = std.mem.Allocator;
const AST = @import("kit").AST;
const Serialize = @import("../serialize.zig");
const Document = @import("kit").Document;
const Span = @import("kit").Span;
pub const manifest = @import("kit").manifest;
const ParseDiagnostic = @import("kit").parse_diagnostic;
const Languages = @import("language.zig");
const editor = @import("../editor.zig");

const Node = AST.Node;
const ExtKind = Node.Kind.Extended.ExtKind;

// The C shapes are the kit's (`kit/abi.zig`), re-exported.
const C = @import("kit").abi;
const convert = @import("kit").convert;
pub const Str = C.Str;
pub const CSpan = C.CSpan;
pub const table = C.table;
pub const Table = C.Table;
pub const no_node = C.no_node;
pub const no_depth_limit = C.no_depth_limit;
pub const ErrorInfo = C.ErrorInfo;
pub const PrintOptions = C.PrintOptions;
pub const CommentDelimiterDesc = C.CommentDelimiterDesc;
pub const CommentsDesc = C.CommentsDesc;
pub const SectionHeaderDesc = C.SectionHeaderDesc;
pub const ClosedContainersDesc = C.ClosedContainersDesc;
pub const SyntaxDesc = C.SyntaxDesc;
pub const lossless_envelope = C.lossless_envelope;
pub const native_null = C.native_null;
pub const nativeExt = C.nativeExt;
pub const DialectDesc = C.DialectDesc;
pub const prints_text_scalars = C.prints_text_scalars;
pub const prints_null_at_root = C.prints_null_at_root;
pub const prints_root_container = C.prints_root_container;
pub const vtable_version = C.vtable_version;
pub const ParseFn = C.ParseFn;
pub const PrintFn = C.PrintFn;
pub const FreeBytesFn = C.FreeBytesFn;
pub const RenderRequest = C.RenderRequest;
pub const RenderFn = C.RenderFn;
pub const SplitStreamFn = C.SplitStreamFn;
pub const StreamDoc = C.StreamDoc;
pub const VTable = C.VTable;
pub const cap_read = C.cap_read;
pub const cap_edit = C.cap_edit;
pub const cap_serialize = C.cap_serialize;
pub const cap_references = C.cap_references;
pub const cap_known = C.cap_known;
pub const requiredSize = C.requiredSize;
pub const gated = C.gated;

// ============================================================================
// THE REGISTRY
// ============================================================================

/// One registered dialect: `Language.Type`'s value `index` names it, and
/// `abi` is the integer a C caller holds. Immutable once appended.
/// What a tree is printed as (`serialize.zig`).
pub const Target = Serialize.Target;

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
    /// What the dialect's printer makes of another format's tree, if it
    /// says.
    prints: ?manifest.Prints = null,

    /// The name handed to every vtable function for this row.
    pub fn dialectZ(self: *const Entry) [*:0]const u8 {
        return self.name.ptr;
    }
    pub fn typeOf(self: *const Entry) Language.Type {
        return @fromBackingInt(@intCast(self.index));
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

/// As many dialect rows as the registry holds; `Language.Type` is a u16, so
/// the bound is that space less headroom. Static so that a read needs no
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
threadlocal var last_refusal: [512]u8 = undefined;
threadlocal var last_refusal_len: usize = 0;

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

/// Forget the last refusal, for a host about to ask something that may
/// write one.
pub fn clearRefusal() void {
    last_refusal_len = 0;
}

/// Register `vt`. Returns the ABI integer of its first dialect row. A row
/// named after a format compiled out of this build takes that format's
/// integer; every other row takes the next one at or above
/// `runtime_abi_base`, so the rows' integers are not consecutive in
/// general — `entryByName` finds each. See the module doc for what happens
/// in between.
pub fn register(allocator: Allocator, raw: *const VTable) RegisterError!c_int {
    return registerWith(allocator, raw, .{});
}

pub const RegisterOptions = struct {
    /// Run the harness over the samples. A language its build has already
    /// held to it — one a consumer bundles — may skip it at startup.
    harness: bool = true,
};

/// `register`, with `options`.
pub fn registerWith(allocator: Allocator, raw: *const VTable, options: RegisterOptions) RegisterError!c_int {
    // The bundle first, outside the lock and in this call's allocator: a
    // lookup made while this registration holds the lock must not start
    // the bundle's, and the registry takes one allocator.
    ensureBundledWith(allocator);
    last_refusal_len = 0;

    // Build the registration in an arena so a refusal below frees it whole.
    // The record is read into it first, at this fig's own layout
    // (`normalize`), and everything after reads that copy.
    var arena_state = std.heap.ArenaAllocator.init(allocator);
    errdefer arena_state.deinit();
    const arena = arena_state.allocator();
    const vt = (try normalize(arena, raw)) orelse return error.InvalidLanguage;
    if (!validateVTable(vt)) return error.InvalidLanguage;

    mutex.lock();
    defer mutex.unlock();

    if (registry_allocator) |a| {
        // By vtable: a stateless allocator (`c_allocator`) has an undefined
        // `ptr`, and two of them are the same allocator.
        if (a.vtable != allocator.vtable) return error.AllocatorMismatch;
    } else registry_allocator = allocator;

    // A name is taken once.
    const name = std.mem.span(vt.name);
    if (findByNameLocked(name) != null) {
        refuse("a format named '{s}' is already registered", .{name});
        return error.NameTaken;
    }
    for (vt.dialects[0..vt.dialect_count]) |d| {
        const dn = std.mem.span(d.name);
        if (findByNameLocked(dn) != null) {
            refuse("a format named '{s}' is already registered", .{dn});
            return error.NameTaken;
        }
    }
    const first_index: usize = published.load(.acquire);
    if (first_index + vt.dialect_count > max_entries) return error.RegistryFull;

    const reg = try arena.create(Registered);
    reg.vt = vt.*;
    reg.name = try arena.dupeSentinel(u8, name, 0);
    reg.vt.name = reg.name.ptr;
    reg.caps = .{
        .read = vt.caps & cap_read != 0,
        .edit = vt.caps & cap_edit != 0,
        .serialize = vt.caps & cap_serialize != 0,
        .references = vt.caps & cap_references != 0,
        .max_mapping_depth = if (vt.max_mapping_depth < 0) null else @intCast(vt.max_mapping_depth),
        .lossless = nativeKindsOf(vt.lossless),
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
            while (arr[k]) |x| : (k += 1) try exts.append(arena, try arena.dupeSentinel(u8, std.mem.span(x), 0));
        }
        const index = first_index + i;
        e.* = .{
            .index = @intCast(index),
            .abi = @intCast(index + 1),
            .language = reg,
            .name = try arena.dupeSentinel(u8, std.mem.span(d.name), 0),
            .extensions = try exts.toOwnedSlice(arena),
            .splice = @fromBackingInt(@intCast(d.splice)),
            .empty_doc_seed = if (d.empty_doc_seed) |s| try arena.dupe(u8, std.mem.span(s)) else null,
            .syntax = if (d.syntax) |s| try syntaxOf(arena, s) else null,
            .prints = try printsOf(arena, &d),
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
    if (options.harness) checkRegistration(allocator, reg) catch |err| {
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

/// Drop every registration — in a test build only, where it is how one
/// test's languages leave the registry before the next. A registered
/// integer is otherwise valid for the life of the process, and a host that
/// called this while another thread held a `Language.Type` would be
/// reading freed memory, so outside a test the name is `void`.
pub const deinitAll = if (builtin.is_test) deinitAllImpl else void;

fn deinitAllImpl() void {
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
    bundle_state.store(0, .release);
}

fn findByNameLocked(name: []const u8) ?*const Entry {
    const n = published.load(.acquire);
    for (slots[0..n]) |e| if (std.mem.eql(u8, e.name, name)) return e;
    return null;
}

/// The entry a `Language.Type` names. Panics on a value never handed out:
/// a `Type` is only ever minted by this registry.
pub fn entryOf(t: Language.Type) *const Entry {
    return entryAt(@backingInt(t)) orelse @panic("runtime language index was never registered");
}

pub fn entryAt(index: usize) ?*const Entry {
    ensureBundled();
    if (index < published.load(.acquire)) return slots[index];
    if (checking) |c| if (index >= c.from and index < c.to) return slots[index];
    return null;
}

/// The entry an ABI integer names — its index plus one — or null for one
/// never handed out.
pub fn entryByAbi(abi: c_int) ?*const Entry {
    ensureBundled();
    if (abi < 1) return null;
    return entryAt(@intCast(abi - 1));
}

/// The entry named `name` — a dialect row's name — or null.
pub fn entryByName(name: []const u8) ?*const Entry {
    ensureBundled();
    mutex.lock();
    defer mutex.unlock();
    return findByNameLocked(name);
}

/// `entryByName`, without registering the bundle first: for the bundle's
/// own registration.
pub fn registeredByName(name: []const u8) ?*const Entry {
    mutex.lock();
    defer mutex.unlock();
    return findByNameLocked(name);
}

/// Content sniffing in the consumer's order: the first registered dialect
/// `order` names whose parser accepts `input`, or null. The order is the
/// consumer's to choose — the grammars overlap, so it decides what ambiguous
/// input means — and a language never ranks itself against another. A name
/// no dialect has, or one that cannot read, is skipped.
pub fn detectAmong(allocator: Allocator, input: []const u8, order: []const []const u8) ?*const Entry {
    for (order) |name| {
        const e = entryByName(name) orelse continue;
        if (!e.language.caps.read) continue;
        const doc = Language.Parser.parse(allocator, input, e.typeOf()) catch continue;
        doc.deinit(allocator);
        return e;
    }
    return null;
}

/// How many dialect rows are registered.
pub fn count() usize {
    ensureBundled();
    return published.load(.acquire);
}

// ── the bundle, registered on first use ────────────────────────────────────
//
// The languages a build compiles in (`bundle.zig`, `-D<name>`) are
// registered the first time anything looks a format up: the CLI's, the
// LSP's, the tests' and a conformance reference module's. The libraries —
// `libfig`, the npm module, fig-sys — compile none in, so there it finds
// nothing to register, and a consumer registers each language it links
// (`docs/proposals/core-only.md` §5.2).

/// 0 not yet, 1 registering (a lookup the registration itself makes
/// returns), 2 done.
var bundle_state: std.atomic.Value(u8) = .init(0);

fn ensureBundled() void {
    ensureBundledWith(registry_allocator orelse defaultAllocator());
}

/// What the C library allocates with (`c_api.zig`'s `activeAllocator`), so
/// that a registration through `fig_language_register` after the bundle's
/// is in the same allocator.
fn defaultAllocator() Allocator {
    if (builtin.cpu.arch.isWasm()) return std.heap.wasm_allocator;
    if (builtin.abi.isAndroid() or !builtin.link_libc) return std.heap.smp_allocator;
    return std.heap.c_allocator;
}

fn ensureBundledWith(allocator: Allocator) void {
    if (bundle_state.load(.acquire) == 2 or bundling_here) return;
    if (bundle_state.cmpxchgStrong(0, 1, .acq_rel, .acquire) != null) {
        // Another thread is registering the bundle: what this one is about
        // to look up may be in it.
        while (bundle_state.load(.acquire) != 2) std.atomic.spinLoopHint();
        return;
    }
    bundling_here = true;
    defer bundling_here = false;
    @import("bundle.zig").registerAll(registry_allocator orelse allocator) catch {};
    bundle_state.store(2, .release);
}

/// Set on the thread registering the bundle, whose own lookups return.
threadlocal var bundling_here: bool = false;

// ── validation ─────────────────────────────────────────────────────────────

/// `raw` read at this fig's own layout, into `arena`: the vtable, its
/// dialect rows and every `SyntaxDesc` they reach, each through `gated` at
/// the size its writer declared, and each re-pointed at the copy, so that
/// `validateVTable` and `register` read full records whatever header the
/// language was built against. Null, with `last_refusal` filled, for a
/// record whose version this fig does not speak or whose sizes are too
/// short to hold its required fields.
fn normalize(arena: Allocator, raw: *const VTable) Allocator.Error!?*VTable {
    // `version` and `size` lead every layout this fig reads.
    const head: [*]const u8 = @ptrCast(raw);
    const version = std.mem.bytesToValue(u32, head[0..4]);
    if (version != vtable_version) {
        refuse("vtable version {d} is not the {d} this fig speaks", .{ version, vtable_version });
        return null;
    }
    const size = std.mem.bytesToValue(u32, head[4..8]);
    if (size < requiredSize(VTable)) {
        refuse("the vtable's size {d} is too short to hold its required fields ({d})", .{ size, requiredSize(VTable) });
        return null;
    }
    const vt = try arena.create(VTable);
    vt.* = gated(VTable, head[0..size]);
    vt.size = @sizeOf(VTable);

    if (vt.dialect_count > 0) {
        if (vt.dialect_size < requiredSize(DialectDesc)) {
            refuse("the vtable's dialect_size {d} is too short to hold a dialect row ({d})", .{ vt.dialect_size, requiredSize(DialectDesc) });
            return null;
        }
        const rows = try arena.alloc(DialectDesc, vt.dialect_count);
        const base: [*]const u8 = @ptrCast(vt.dialects);
        for (rows, 0..) |*d, i| {
            d.* = gated(DialectDesc, base[i * vt.dialect_size ..][0..vt.dialect_size]);
            if (d.syntax) |sd| d.syntax = (try normalizeSyntax(arena, sd)) orelse return null;
        }
        vt.dialects = rows.ptr;
    }
    vt.dialect_size = @sizeOf(DialectDesc);
    if (vt.syntax) |sd| vt.syntax = (try normalizeSyntax(arena, sd)) orelse return null;
    return vt;
}

fn normalizeSyntax(arena: Allocator, raw: *const SyntaxDesc) Allocator.Error!?*const SyntaxDesc {
    const head: [*]const u8 = @ptrCast(raw);
    const size = std.mem.bytesToValue(u32, head[0..4]);
    if (size < requiredSize(SyntaxDesc)) {
        refuse("a syntax record's size {d} is too short to hold its required fields ({d})", .{ size, requiredSize(SyntaxDesc) });
        return null;
    }
    const out = try arena.create(SyntaxDesc);
    out.* = gated(SyntaxDesc, head[0..size]);
    out.size = @sizeOf(SyntaxDesc);
    return out;
}

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
    if (vt.caps & ~cap_known != 0) {
        refuse("'{s}' declares capability bits 0x{x} this fig does not know", .{ name, vt.caps & ~cap_known });
        return false;
    }
    if (vt.lossless & ~knownLosslessBits() != 0) {
        refuse("'{s}' declares lossless bits 0x{x} this fig does not know", .{ name, vt.lossless & ~knownLosslessBits() });
        return false;
    }
    if (vt.lossless != 0 and vt.lossless & lossless_envelope == 0) {
        refuse("'{s}' declares native kinds in lossless without the envelope bit", .{name});
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
        if (d.splice < 0 or d.splice >= @typeInfo(manifest.SpliceStyle).@"enum".field_names.len) {
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
    if (vt.caps & cap_serialize == 0 and vt.lossless != 0) {
        refuse("'{s}' declares lossless (an envelope target for serialized output) but no serialize capability", .{name});
        return false;
    }
    const edit = vt.caps & cap_edit != 0;
    if (edit and vt.syntax == null) {
        refuse("'{s}' declares the edit capability and must supply syntax", .{name});
        return false;
    }
    if (!edit) {
        if (vt.render_value != null or vt.render_entry != null or vt.render_item != null or vt.render_tail != null or vt.render_key != null or vt.render_key_name != null) {
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
    if (s.flow_entry_sep) |sep| if (sep[0] == 0) {
        refuse("'{s}': flow_entry_sep is empty; null means `,`", .{name});
        return false;
    };
    if (s.flow_root and !s.flow_containers) {
        refuse("'{s}' declares flow_root but flow_containers = false", .{name});
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
            .line = try delimiterOf(arena, s.comments.line),
            .trailing = try delimiterOf(arena, s.comments.trailing),
            .block = try delimiterOf(arena, s.comments.block),
        },
        .kv_sep = try dupeZstr(arena, s.kv_sep),
        .flow_kv_sep_from_siblings = s.flow_kv_sep_from_siblings,
        .flow_map_pad = try dupeZstr(arena, s.flow_map_pad) orelse defaults.flow_map_pad,
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
        .section_noun = try dupeZstr(arena, s.section_noun),
        .merge_key = try dupeZstr(arena, s.merge_key),
        .flow_entry_sep = try dupeZstr(arena, s.flow_entry_sep) orelse defaults.flow_entry_sep,
        .flow_root = s.flow_root,
        .flow_maps_only = s.flow_maps_only,
        .line_prefix_marks = try dupeZstr(arena, s.line_prefix_marks) orelse defaults.line_prefix_marks,
        .flow_map_kv_sep = try dupeZstr(arena, s.flow_map_kv_sep) orelse defaults.flow_map_kv_sep,
        .flow_map_entry_sep = try dupeZstr(arena, s.flow_map_entry_sep) orelse defaults.flow_map_entry_sep,
        .line_continuation = if (s.line_continuation == 0) null else s.line_continuation,
        .empty_seq_literal = try dupeZstr(arena, s.empty_seq_literal),
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

fn nativeKindsOf(bits: u32) ?manifest.NativeKinds {
    if (bits & lossless_envelope == 0) return null;
    var out: manifest.NativeKinds = .{ .null = bits & native_null != 0 };
    inline for (@typeInfo(ExtKind).@"enum".field_names) |k_name| {
        @field(out, k_name) = bits & nativeExt(@field(ExtKind, k_name)) != 0;
    }
    return out;
}

/// `nativeKindsOf`'s inverse, for a host that holds a `manifest.NativeKinds`
/// and fills a vtable from it.
/// A dialect row's `manifest.Prints`, or null when it declares none.
fn printsOf(arena: Allocator, d: *const DialectDesc) Allocator.Error!?manifest.Prints {
    const holds = nativeKindsOf(d.prints_holds) orelse return null;
    return .{
        .holds = holds,
        .typed_scalars = d.prints_flags & prints_text_scalars == 0,
        .null_at_root = d.prints_flags & prints_null_at_root != 0,
        .root_container = d.prints_flags & prints_root_container != 0,
        .char_as = try dupeZstr(arena, d.prints_char_as) orelse "string",
        .number_special_as = try dupeZstr(arena, d.prints_number_special_as) orelse "string",
        .comments = switch (d.prints_comments) {
            1 => .pretty,
            2 => .never,
            else => .always,
        },
    };
}

pub const losslessBits = C.losslessBits;

fn knownLosslessBits() u32 {
    var bits: u32 = lossless_envelope | native_null;
    inline for (@typeInfo(ExtKind).@"enum".field_names) |f_name| bits |= nativeExt(@field(ExtKind, f_name));
    return bits;
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
    for (doc.ast.region_facts) |r| {
        if (r.node >= doc.ast.nodeCount()) return error.MalformedRegion;
        const kind = doc.ast.node(r.node).kind;
        if (kind != .mapping and kind != .sequence) return error.MalformedRegion;
        if (!(r.start < r.end and r.end <= source.len)) return error.MalformedRegion;
        if (!(r.start == 0 or source[r.start - 1] == '\n')) return error.MalformedRegion;
        if (!(r.end == source.len or source[r.end - 1] == '\n')) return error.MalformedRegion;
        if (prev) |p| {
            if (!(p.node < r.node or (p.node == r.node and p.start < r.start))) return error.MalformedRegion;
        }
        prev = r;
    }
}

// ============================================================================
// TABLE ↔ DOCUMENT
// ============================================================================

pub const TableError = convert.TableError;
pub const tableToDocument = convert.tableToDocument;
pub const documentToTable = convert.documentToTable;
pub const fullTable = convert.fullTable;

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
    // Stripping is core's, as it is for a compiled printer: the language
    // is handed a table with no comments rather than asked to drop them.
    var stripped: AST = ast.*;
    if (options.strip_comments) {
        stripped.comment_runs = &.{};
        stripped.comment_list = &.{};
    }
    const built = try documentToTable(arena_state.allocator(), &stripped, root);
    const view = built.view();
    const opts: PrintOptions = .{
        .pretty = options.pretty,
        .strip_comments = options.strip_comments,
        .indent = options.indent,
        .width = options.width,
        .splice = options.splice,
        .flow = options.flow,
    };
    var out: Str = .{};
    var err: ErrorInfo = .empty;
    const rc = print(reg.vt.ctx, e.dialectZ(), &view, &opts, &out, &err);
    if (rc != 0) return switch (@as(C.Refusal, @fromBackingInt(@intCast(err.code)))) {
        .null_unsupported => error.NullUnsupported,
        .non_string_key => error.NonStringKey,
        .unrepresentable_root => error.UnrepresentableRoot,
        .unsupported_value => error.UnsupportedValue,
        .invalid_key => error.InvalidKey,
        .non_finite_number => error.NonFiniteNumber,
        .unresolved_alias => error.UnresolvedAlias,
        .nesting_too_deep => error.NestingTooDeep,
        .unexpected_node_kind => error.UnexpectedNodeKind,
        else => error.RuntimePrintFailed,
    };
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
        /// Where a caller wants the parse's diagnostics — the errors a failed
        /// parse left and the warnings a clean one raised — each message
        /// copied in `allocator`. Null keeps none.
        errors: ?*std.ArrayList(ParseDiagnostic.Rendered) = null,
        warnings: ?*std.ArrayList(ParseDiagnostic.Rendered) = null,

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

    pub const default_type: Type = @fromBackingInt(@intCast(0));

    pub fn parse(parser: *Parser, input: []const u8, t: Type) !Document {
        const e = entryOf(t);
        const reg = e.language;
        var tbl = Table.init(parser.allocator, input);
        defer tbl.deinit();
        parser.last_error = .empty;
        const rc = reg.vt.parse(reg.vt.ctx, e.dialectZ(), Str.of(input), &tbl, &parser.last_error);
        try collectDiagnostics(parser, &tbl);
        if (rc != 0) return error.RuntimeParseFailed;
        var doc = tableToDocument(parser.allocator, input, &tbl) catch |err| {
            parser.last_error.set(switch (err) {
                error.UnknownKind => "the parser returned a node kind this fig does not know",
                error.MalformedTable => "the parser returned a malformed node table",
                error.OutOfMemory => return error.OutOfMemory,
            });
            return error.RuntimeParseFailed;
        };
        if (e.syntax orelse reg.syntax) |sx| doc.ast.merge_key = sx.merge_key;
        return doc;
    }

    /// The table's diagnostics, into the sinks `parser` names.
    fn collectDiagnostics(parser: *Parser, tbl: *const Table) Allocator.Error!void {
        for (tbl.diagnostics.items) |d| {
            const sink = (if (d.severity == table.diagnostic_warning) parser.warnings else parser.errors) orelse continue;
            try sink.append(parser.allocator, .{
                .offset = d.start,
                .end = if (d.end == table.none) null else d.end,
                .message = try parser.allocator.dupe(u8, tbl.textOf(d.message) orelse ""),
                .short_label = try parser.allocator.dupe(u8, tbl.textOf(d.label) orelse ""),
            });
        }
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
            .{ .comments = .{ .line = null, .trailing = null }, .kv_sep = null, .empty_map_literal = null };
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
            .key_name => vt.render_key_name != null,
        };
    }

    pub fn renderValue(t: Type, allocator: Allocator, out: *std.ArrayList(u8), r: manifest.RenderRequest) !void {
        return render(t, allocator, out, .value, r);
    }
    pub fn renderEntry(t: Type, allocator: Allocator, out: *std.ArrayList(u8), r: manifest.RenderRequest) !void {
        return render(t, allocator, out, .entry, r);
    }
    pub fn renderItem(t: Type, allocator: Allocator, out: *std.ArrayList(u8), r: manifest.RenderRequest) !void {
        return render(t, allocator, out, .item, r);
    }
    pub fn renderTail(t: Type, allocator: Allocator, out: *std.ArrayList(u8), r: manifest.RenderRequest) !void {
        return render(t, allocator, out, .tail, r);
    }
    pub fn renderKey(t: Type, allocator: Allocator, out: *std.ArrayList(u8), r: manifest.RenderRequest) !void {
        return render(t, allocator, out, .key, r);
    }
    pub fn renderKeyName(t: Type, allocator: Allocator, out: *std.ArrayList(u8), r: manifest.RenderRequest) !void {
        return render(t, allocator, out, .key_name, r);
    }

    /// The entry's `split_stream`, or `UnsupportedOperation` for a language
    /// without one. The documents are fig's, in `allocator`.
    pub fn splitStream(allocator: Allocator, input: []const u8, t: Type) ![]manifest.StreamDocument {
        const e = entryOf(t);
        const vt = &e.language.vt;
        const f = vt.split_stream orelse return error.UnsupportedOperation;
        var s: Str = .{};
        var err: ErrorInfo = .empty;
        if (f(vt.ctx, e.dialectZ(), Str.of(input), &s, &err) != 0) return error.RuntimeParseFailed;
        defer vt.free_bytes(vt.ctx, s);
        const bytes = s.slice() orelse "";
        const n = bytes.len / @sizeOf(StreamDoc);
        const out = try allocator.alloc(manifest.StreamDocument, n);
        for (out, 0..) |*d, i| {
            const r = std.mem.bytesToValue(StreamDoc, bytes[i * @sizeOf(StreamDoc) ..][0..@sizeOf(StreamDoc)]);
            if (r.start > r.end or r.end > input.len) {
                allocator.free(out);
                return error.RuntimeParseFailed;
            }
            d.* = .{ .start = r.start, .end = r.end, .explicit = r.marked };
        }
        return out;
    }

    /// One renderer call: the request in the C shape, the slot `which`
    /// names, and the fragment appended to `out`.
    fn render(t: Type, allocator: Allocator, out: *std.ArrayList(u8), comptime which: manifest.Renderer, r: manifest.RenderRequest) !void {
        const e = entryOf(t);
        const vt = &e.language.vt;
        const f = switch (which) {
            .value => vt.render_value,
            .entry => vt.render_entry,
            .item => vt.render_item,
            .tail => vt.render_tail,
            .key => vt.render_key,
            .key_name => vt.render_key_name,
        } orelse return error.UnsupportedShape;
        const request: RenderRequest = .{
            .dialect = e.dialectZ(),
            .indent = Str.of(r.indent),
            .key = Str.of(r.key),
            .value = Str.of(r.value),
            .literal = @tagName(r.literal).ptr,
            .old_key = Str.of(r.old_key),
            .parent_key = Str.of(r.parent_key),
            .parent_tag = Str.of(r.parent_tag),
        };
        var s: Str = .{};
        var err: ErrorInfo = .empty;
        if (f(vt.ctx, &request, &s, &err) != 0) return rendererError(&err);
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

/// A row for a test table: its text, if any, goes into the buffer.
const TestRow = struct { kind: u8, parent: u32 = no_node, start: u32, end: u32, text: ?[]const u8 = null, flags: u8 = 0 };

/// A table over `src` with `rows`, in the testing allocator.
fn testTable(src: []const u8, rows: []const TestRow) !Table {
    var t = Table.init(testing.allocator, src);
    errdefer t.deinit();
    for (rows) |r| _ = try t.addRow(.{
        .kind = r.kind,
        .parent = r.parent,
        .start = r.start,
        .end = r.end,
        .flags = r.flags,
        .text = if (r.text) |x| try t.addString(x) else .absent,
    });
    return t;
}

const TK = table.Kind;

/// `t` adopted as a document, from a copy, so that a test can go on to
/// change `t`.
fn adoptCopy(src: []const u8, t: *const Table) TableError!Document {
    var copy = try t.clone(testing.allocator);
    defer copy.deinit();
    return tableToDocument(testing.allocator, src, &copy);
}

test "a null-row table is the empty document and an empty one is refused" {
    var empty = Table.init(testing.allocator, "");
    defer empty.deinit();
    try testing.expectError(error.MalformedTable, adoptCopy("", &empty));
    var t = try testTable("", &.{.{ .kind = TK.null_, .start = 0, .end = 0 }});
    defer t.deinit();
    const doc = try adoptCopy("", &t);
    defer doc.deinit(testing.allocator);
    try testing.expect(doc.ast.node(doc.ast.root).kind == .null_);
}

test "a mapping table round-trips through Document and back" {
    // The round trip is checked by printing through compiled JSON.
    if (comptime !@import("build_options").lang_json) return error.SkipZigTest;
    // { a: 1, b: [true, "x"] } as `a: 1\nb: [true, x]\n`
    const src = "a: 1\nb: [true, x]\n";
    const rows = [_]TestRow{
        .{ .kind = TK.mapping, .start = 0, .end = src.len },
        .{ .kind = TK.keyvalue, .parent = 0, .start = 0, .end = 4 },
        .{ .kind = TK.string, .parent = 1, .start = 0, .end = 1, .text = "a" },
        .{ .kind = TK.int, .parent = 1, .start = 3, .end = 4, .text = "1" },
        .{ .kind = TK.keyvalue, .parent = 0, .start = 5, .end = 17 },
        .{ .kind = TK.string, .parent = 4, .start = 5, .end = 6, .text = "b" },
        .{ .kind = TK.sequence, .parent = 4, .start = 8, .end = 17 },
        .{ .kind = TK.true_, .parent = 6, .start = 9, .end = 13 },
        .{ .kind = TK.string, .parent = 6, .start = 15, .end = 16, .text = "x" },
    };
    var t = try testTable(src, &rows);
    defer t.deinit();
    try t.seps.appendSlice(testing.allocator, &.{ .{ .node = 1, .start = 1, .end = 2 }, .{ .node = 4, .start = 6, .end = 7 } });
    try t.comments.append(testing.allocator, .{ .node = 2, .slot = table.comment_leading, .style = table.comment_line, .text = try t.addString("hello") });
    const doc = try adoptCopy(src, &t);
    defer doc.deinit(testing.allocator);

    var out: std.Io.Writer.Allocating = .init(testing.allocator);
    defer out.deinit();
    try Serialize.serializeWith(&doc.ast, &out.writer, Serialize.Target.named("json").?, .{ .pretty = false });
    try testing.expectEqualStrings("{\"a\":1,\"b\":[true,\"x\"]}\n", out.written());
    try testing.expectEqualStrings("hello", doc.ast.comments(2).leading[0].text);
    try testing.expect(doc.sepSpan(doc.ast.node(1)) != null);
    try testing.expectEqual(@as(usize, 6), doc.sepSpan(doc.ast.node(4)).?.start);

    // And back: the same rows, minus the spans, plus nothing.
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const built = try documentToTable(arena.allocator(), &doc.ast, doc.ast.root);
    try testing.expectEqual(rows.len, built.rows.items.len);
    for (rows, built.rows.items) |want, got| {
        try testing.expectEqual(want.kind, got.kind);
        try testing.expectEqual(want.parent, got.parent);
        try testing.expectEqualStrings(want.text orelse "", built.textOf(got.text) orelse "");
        try testing.expectEqual(table.none, got.start);
    }
    try testing.expectEqual(@as(usize, 1), built.comments.items.len);

    // `fullTable` is the parse's own answer back: every span, the
    // separators, and the comment, as they were given.
    const full = try fullTable(arena.allocator(), &doc);
    try testing.expectEqual(rows.len, full.rows.items.len);
    for (rows, full.rows.items) |want, got| {
        try testing.expectEqual(want.start, got.start);
        try testing.expectEqual(want.end, got.end);
    }
    try testing.expectEqual(@as(usize, 2), full.seps.items.len);
    try testing.expectEqual(@as(u32, 6), full.seps.items[1].start);
    try testing.expectEqual(@as(usize, 0), full.markers.items.len);
    try testing.expectEqual(@as(usize, 0), full.regions.items.len);
    try testing.expectEqual(@as(usize, 0), full.mentions.items.len);
    try testing.expectEqual(@as(usize, 1), full.comments.items.len);
    try testing.expectEqual(@as(u32, 2), full.comments.items[0].node);
}

test "a tag is read by its resolution, or by its text when the language left it unstated" {
    // The encoder spells a `.kind` tag as YAML's core schema does
    // (`!!int`); reading it back as verbatim text lost the assertion — a
    // runtime fig's `port: int = 5432` reached the printers untagged.
    const src = "a: 1\nb: 2\nc: 3\n";
    const rows = [_]TestRow{
        .{ .kind = TK.mapping, .start = 0, .end = src.len },
        .{ .kind = TK.keyvalue, .parent = 0, .start = 0, .end = 4 },
        .{ .kind = TK.string, .parent = 1, .start = 0, .end = 1, .text = "a" },
        .{ .kind = TK.int, .parent = 1, .start = 3, .end = 4, .text = "1" },
        .{ .kind = TK.keyvalue, .parent = 0, .start = 5, .end = 9 },
        .{ .kind = TK.string, .parent = 4, .start = 5, .end = 6, .text = "b" },
        .{ .kind = TK.int, .parent = 4, .start = 8, .end = 9, .text = "2" },
        .{ .kind = TK.keyvalue, .parent = 0, .start = 10, .end = 14 },
        .{ .kind = TK.string, .parent = 7, .start = 10, .end = 11, .text = "c" },
        .{ .kind = TK.int, .parent = 7, .start = 13, .end = 14, .text = "3" },
    };
    var t = try testTable(src, &rows);
    defer t.deinit();
    try t.tags.append(testing.allocator, .{ .node = 3, .text = try t.addString("!!int") });
    try t.tags.append(testing.allocator, .{ .node = 6, .text = try t.addString("!custom"), .resolved = table.tag_custom });
    try t.tags.append(testing.allocator, .{ .node = 9, .text = try t.addString("tag:yaml.org,2002:int"), .resolved = table.tag_kind + 3 });
    const doc = try adoptCopy(src, &t);
    defer doc.deinit(testing.allocator);
    try testing.expectEqual(AST.Tag{ .kind = .integer }, doc.ast.tagOf(3).?);
    try testing.expectEqualStrings("!custom", doc.ast.tagOf(6).?.text);
    // Resolved to a core type but spelled otherwise, it keeps its spelling
    // for a printer to write back, and its resolution for materialize.
    try testing.expectEqualStrings("tag:yaml.org,2002:int", doc.ast.tagOf(9).?.text);
    try testing.expectEqual(table.tag_kind + 3, doc.ast.tagFact(9).?.resolved);

    // And back out: a kind tag resolved and spelled as the core schema does.
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const built = try documentToTable(arena.allocator(), &doc.ast, doc.ast.root);
    try testing.expectEqualStrings("!!int", built.textOf(built.tags.items[0].text).?);
    try testing.expectEqual(table.tag_kind + 3, built.tags.items[0].resolved);
    try testing.expectEqualStrings("!custom", built.textOf(built.tags.items[1].text).?);
}

test "a tag directive travels with the rows, and only a whole document's print gets it back" {
    // `!e!foo` is legal only in a document declaring `!e!`, so a twin of
    // YAML that read the `%TAG` line has to hand it over for the printer
    // to write back; a fragment (a splice's text) has no directives prefix.
    const src = "%TAG !e! tag:x/\n---\n!e!foo bar\n";
    var t = try testTable(src, &.{.{ .kind = TK.string, .start = 20, .end = 29, .text = "bar" }});
    defer t.deinit();
    try t.tags.append(testing.allocator, .{ .node = 0, .text = try t.addString("!e!foo") });
    try t.directives.append(testing.allocator, .{ .handle = try t.addString("!e!"), .prefix = try t.addString("tag:x/") });
    const doc = try adoptCopy(src, &t);
    defer doc.deinit(testing.allocator);
    try testing.expectEqual(@as(usize, 1), doc.ast.tag_directives.len);
    try testing.expectEqualStrings("!e!", doc.ast.tag_directives[0].handle);
    try testing.expectEqualStrings("tag:x/", doc.ast.tag_directives[0].prefix);

    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const whole = try fullTable(arena.allocator(), &doc);
    try testing.expectEqual(@as(usize, 1), whole.directives.items.len);
    try testing.expectEqualStrings("tag:x/", whole.textOf(whole.directives.items[0].prefix).?);

    // A directive with no prefix is not one.
    t.directives.items[0].prefix = .absent;
    try testing.expectError(error.MalformedTable, adoptCopy(src, &t));
}

test "a malformed table is refused, not read" {
    const src = "x";
    // A child before its parent.
    var t = try testTable(src, &.{
        .{ .kind = TK.sequence, .start = 0, .end = 1 },
        .{ .kind = TK.string, .parent = 2, .start = 0, .end = 1, .text = "x" },
        .{ .kind = TK.string, .parent = 0, .start = 0, .end = 1, .text = "x" },
    });
    defer t.deinit();
    const rows = t.rows.items;
    try testing.expectError(error.MalformedTable, adoptCopy(src, &t));
    // A mapping child that is not a keyvalue.
    rows[0].kind = TK.mapping;
    rows[1].parent = 0;
    try testing.expectError(error.MalformedTable, adoptCopy(src, &t));
    // An unknown kind.
    rows[0].kind = 99;
    try testing.expectError(error.UnknownKind, adoptCopy(src, &t));
    // A row with no span, and one past the source.
    rows[0].kind = TK.sequence;
    rows[1].start = table.none;
    try testing.expectError(error.MalformedTable, adoptCopy(src, &t));
    rows[1].start = 0;
    rows[1].end = 2;
    try testing.expectError(error.MalformedTable, adoptCopy(src, &t));
    // A text past the buffer.
    rows[1].end = 1;
    rows[1].text = .{ .off = 0, .len = 99 | table.Text.buffer_bit };
    try testing.expectError(error.MalformedTable, adoptCopy(src, &t));
    // The open flag on a row that is not a sequence.
    rows[1].text = .absent;
    rows[1].flags = table.row_open;
    try testing.expectError(error.MalformedTable, adoptCopy(src, &t));
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
/// `tinykv`, for the tests of other files that register a language. `void`
/// outside a test build.
pub const test_language = if (builtin.is_test) TinyKv else void;

const TinyKv = struct {
    pub const Alloc = struct { allocator: Allocator };

    fn parse(ctx: ?*anyopaque, dialect: [*:0]const u8, input: Str, out: *Table, err: *ErrorInfo) callconv(.c) c_int {
        _ = dialect;
        _ = ctx;
        return parseInto(input.slice() orelse "", out, err) catch 255;
    }

    fn parseInto(src: []const u8, out: *Table, err: *ErrorInfo) !c_int {
        _ = try out.addRow(.{ .kind = TK.mapping, .start = 0, .end = @intCast(src.len) });
        var pending: std.ArrayList([]const u8) = .empty;
        defer pending.deinit(out.allocator);
        var at: usize = 0;
        while (at < src.len) {
            const nl = std.mem.indexOfScalarPos(u8, src, at, '\n') orelse src.len;
            const line = src[at..nl];
            if (line.len == 0) {
                at = nl + 1;
                continue;
            }
            if (line[0] == '#') {
                try pending.append(out.allocator, std.mem.trim(u8, line[1..], " "));
                at = nl + 1;
                continue;
            }
            const eq = std.mem.indexOfScalar(u8, line, '=') orelse {
                err.set("expected key=value");
                err.byte_offset = at;
                return 2;
            };
            const kv = try out.addRow(.{ .kind = TK.keyvalue, .parent = 0, .start = @intCast(at), .end = @intCast(nl) });
            try out.seps.append(out.allocator, .{ .node = kv, .start = @intCast(at + eq), .end = @intCast(at + eq + 1) });
            const key = try out.addRow(.{ .kind = TK.string, .parent = kv, .start = @intCast(at), .end = @intCast(at + eq), .text = try out.text(line[0..eq]) });
            _ = try out.addRow(.{ .kind = TK.string, .parent = kv, .start = @intCast(at + eq + 1), .end = @intCast(nl), .text = try out.text(line[eq + 1 ..]) });
            for (pending.items) |c| try out.comments.append(out.allocator, .{ .node = key, .slot = table.comment_leading, .style = table.comment_line, .text = try out.text(c) });
            pending.clearRetainingCapacity();
            at = nl + 1;
        }
        return 0;
    }

    fn print(ctx: ?*anyopaque, dialect: [*:0]const u8, view: *const table.View, options: *const PrintOptions, out: *Str, err: *ErrorInfo) callconv(.c) c_int {
        _ = dialect;
        _ = options;
        const a: *Alloc = @ptrCast(@alignCast(ctx.?));
        const rows = table.slice(table.Row, view.rows, view.row_count);
        const comments = table.slice(table.Comment, view.comments, view.comment_count);
        const source = if (view.source) |p| p[0..view.source_len] else "";
        const strings = if (view.strings) |p| p[0..view.strings_len] else "";
        var buf: std.ArrayList(u8) = .empty;
        var i: usize = 1;
        while (i < rows.len) {
            // A mapping is three rows per entry: keyvalue, key, value.
            if (rows[i].kind != TK.keyvalue or i + 2 >= rows.len) {
                err.set("tinykv holds a flat string map");
                buf.deinit(a.allocator);
                return 4;
            }
            const key = rows[i + 1];
            const val = rows[i + 2];
            if (key.kind != TK.string or val.kind != TK.string) {
                err.set("tinykv holds a flat string map");
                buf.deinit(a.allocator);
                return 4;
            }
            for (comments) |c| {
                if (c.node == i + 1 and c.slot == table.comment_leading) {
                    buf.appendSlice(a.allocator, "# ") catch return 255;
                    buf.appendSlice(a.allocator, table.viewText(source, strings, c.text) orelse "") catch return 255;
                    buf.append(a.allocator, '\n') catch return 255;
                }
            }
            buf.appendSlice(a.allocator, table.viewText(source, strings, key.text) orelse "") catch return 255;
            buf.append(a.allocator, '=') catch return 255;
            buf.appendSlice(a.allocator, table.viewText(source, strings, val.text) orelse "") catch return 255;
            buf.append(a.allocator, '\n') catch return 255;
            i += 3;
        }
        const out_bytes = buf.toOwnedSlice(a.allocator) catch return 255;
        out.* = Str.of(out_bytes);
        return 0;
    }

    fn freeBytes(ctx: ?*anyopaque, bytes: Str) callconv(.c) void {
        const a: *Alloc = @ptrCast(@alignCast(ctx.?));
        if (bytes.slice()) |s| if (s.len > 0) a.allocator.free(s);
    }

    const syntax: SyntaxDesc = .{
        .comments = .{ .line = .{ .open = "#" }, .trailing = .{} },
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
            .free_bytes = freeBytes,
        };
    }
};

test "a language registered from Zig parses, edits and prints through the contract" {
    // The round trip is checked by printing through compiled JSON.
    if (comptime !@import("build_options").lang_json) return error.SkipZigTest;
    deinitAll();
    defer deinitAll();
    var alloc: TinyKv.Alloc = .{ .allocator = testing.allocator };
    const vt = TinyKv.vtable(&alloc);
    const abi = try register(testing.allocator, &vt);
    try testing.expectEqual(@as(c_int, @intCast(entryByName("tinykv").?.index + 1)), abi);
    const e = entryByAbi(abi).?;
    try testing.expectEqualStrings("tinykv", e.name);
    try testing.expect(entryByName("tinykv") == e);
    try testing.expect(e.language.caps.edit);
    try testing.expectEqualStrings("tkv", e.extensions[0]);
    // The last registered, after whatever the build bundles.
    try testing.expectEqual(e.index + 1, count());

    // Parse through the Language, as the C API and CLI do.
    const doc = try Language.Parser.parse(testing.allocator, "# note\nx=1\ny=2\n", e.typeOf());
    defer doc.deinit(testing.allocator);
    var out: std.Io.Writer.Allocating = .init(testing.allocator);
    defer out.deinit();
    try Serialize.serializeWith(&doc.ast, &out.writer, Serialize.Target.named("json").?, .{ .pretty = false });
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

    fn parse(ctx: ?*anyopaque, dialect: [*:0]const u8, input: Str, out: *Table, err: *ErrorInfo) callconv(.c) c_int {
        _ = dialect;
        _ = ctx;
        const src = input.slice() orelse "";
        const body = std.mem.trimEnd(u8, src, " \n");
        if (body.len < 2 or body[0] != '[' or body[body.len - 1] != ']') {
            err.set("expected [a, b]");
            return 2;
        }
        _ = out.addRow(.{ .kind = TK.sequence, .start = 0, .end = @intCast(body.len) }) catch return 255;
        var at: usize = 1;
        while (at < body.len - 1) {
            if (body[at] == ' ' or body[at] == ',') {
                at += 1;
                continue;
            }
            var end = at;
            while (end < body.len - 1 and body[end] != ',' and body[end] != ' ') end += 1;
            _ = out.addRow(.{ .kind = TK.string, .parent = 0, .start = @intCast(at), .end = @intCast(end), .text = out.text(body[at..end]) catch return 255 }) catch return 255;
            at = end;
        }
        return 0;
    }

    fn print(ctx: ?*anyopaque, dialect: [*:0]const u8, view: *const table.View, options: *const PrintOptions, out: *Str, err: *ErrorInfo) callconv(.c) c_int {
        _ = dialect;
        _ = options;
        const a: *Alloc = @ptrCast(@alignCast(ctx.?));
        const rows = view.rowSlice();
        var buf: std.ArrayList(u8) = .empty;
        buf.append(a.allocator, '[') catch return 255;
        for (rows[1..], 0..) |row, i| {
            if (row.kind != TK.string) {
                err.set("tinylist holds a flat list of words");
                buf.deinit(a.allocator);
                return 4;
            }
            if (i > 0) buf.appendSlice(a.allocator, ", ") catch return 255;
            buf.appendSlice(a.allocator, view.textOf(row.text) orelse "") catch return 255;
        }
        buf.appendSlice(a.allocator, "]\n") catch return 255;
        const s = buf.toOwnedSlice(a.allocator) catch return 255;
        out.* = Str.of(s);
        return 0;
    }

    const syntax: SyntaxDesc = .{
        .comments = .{ .line = .{}, .trailing = .{} },
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
            .free_bytes = TinyKv.freeBytes,
        };
    }
};

test "a runtime language's flow root is edited by comma-aware splice, not as a section root" {
    // `Editor.is_section_format` is "may be" for a runtime language; the
    // dialect's `section_noun` settles it at the call. Before it did, a
    // runtime `[a, b, c]` was taken for a section root and its items were
    // deleted by line — the whole document, for a one-line list.
    deinitAll();
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

// `tinydeps`: a header line `name !tag` opens a block list under `name`,
// and each `- word` line below it is an item. The item and value renderers
// spell what they are told about the container — the list's key and its
// tag — so that what the engine tells them is what the edit shows.
const TinyDeps = struct {
    pub const Alloc = TinyKv.Alloc;

    fn parse(ctx: ?*anyopaque, dialect: [*:0]const u8, input: Str, out: *Table, err: *ErrorInfo) callconv(.c) c_int {
        _ = dialect;
        _ = ctx;
        return parseInto(input.slice() orelse "", out, err) catch 255;
    }

    fn parseInto(src: []const u8, out: *Table, err: *ErrorInfo) !c_int {
        _ = try out.addRow(.{ .kind = TK.mapping, .start = 0, .end = @intCast(src.len) });
        var kv: ?u32 = null;
        var seq: ?u32 = null;
        var at: usize = 0;
        while (at < src.len) {
            const nl = std.mem.indexOfScalarPos(u8, src, at, '\n') orelse src.len;
            const line = src[at..nl];
            defer at = nl + 1;
            if (line.len == 0) continue;
            if (std.mem.startsWith(u8, line, "- ")) {
                const q = seq orelse {
                    err.set("an item needs a list header above it");
                    return 2;
                };
                const item = try out.addRow(.{ .kind = TK.string, .parent = q, .start = @intCast(at + 2), .end = @intCast(nl), .text = try out.text(line[2..]) });
                try out.markers.append(out.allocator, .{ .node = item, .start = @intCast(at), .end = @intCast(at + 2) });
                const rows = out.rows.items;
                if (rows[q].start == rows[q].end) rows[q].start = @intCast(at);
                rows[q].end = @intCast(nl);
                rows[kv.?].end = @intCast(nl);
                continue;
            }
            const sp = std.mem.indexOfScalar(u8, line, ' ') orelse line.len;
            kv = try out.addRow(.{ .kind = TK.keyvalue, .parent = 0, .start = @intCast(at), .end = @intCast(nl) });
            _ = try out.addRow(.{ .kind = TK.string, .parent = kv.?, .start = @intCast(at), .end = @intCast(at + sp), .text = try out.text(line[0..sp]) });
            seq = try out.addRow(.{ .kind = TK.sequence, .parent = kv.?, .start = @intCast(nl), .end = @intCast(nl) });
            const tag = std.mem.trim(u8, line[sp..], " ");
            if (tag.len > 0) try out.tags.append(out.allocator, .{ .node = seq.?, .start = @intCast(nl - tag.len), .end = @intCast(nl), .text = try out.text(tag) });
        }
        return 0;
    }

    fn spell(ctx: ?*anyopaque, out: *Str, comptime fmt: []const u8, args: anytype) c_int {
        const a: *Alloc = @ptrCast(@alignCast(ctx.?));
        const s = std.fmt.allocPrint(a.allocator, fmt, args) catch return 255;
        out.* = Str.of(s);
        return 0;
    }

    /// `- <item name>:<list key>:<value>`, the item name read off the tag.
    fn renderItem(ctx: ?*anyopaque, r: *const RenderRequest, out: *Str, err: *ErrorInfo) callconv(.c) c_int {
        _ = err;
        const tag = r.parent_tag.slice() orelse "";
        return spell(ctx, out, "- {s}:{s}:{s}", .{ if (tag.len > 0) tag[1..] else "?", r.parent_key.slice() orelse "", r.value.slice() orelse "" });
    }

    /// `<value>@<list key>`.
    fn renderValue(ctx: ?*anyopaque, r: *const RenderRequest, out: *Str, err: *ErrorInfo) callconv(.c) c_int {
        _ = err;
        return spell(ctx, out, "{s}@{s}", .{ r.value.slice() orelse "", r.parent_key.slice() orelse "" });
    }

    const syntax: SyntaxDesc = .{
        .comments = .{ .line = .{}, .trailing = .{} },
        .kv_sep = " ",
        .empty_map_literal = null,
        .flow_containers = false,
    };
    const extensions = [_]?[*:0]const u8{ "tdeps", null };
    const dialects = [_]DialectDesc{.{ .name = "tinydeps", .extensions = &extensions, .splice = 2, .empty_doc_seed = "" }};
    const samples = [_]Str{Str.of("deps !dep\n- a\n- b\n")};

    pub fn vtable(alloc: *Alloc) VTable {
        return .{
            .version = vtable_version,
            .ctx = alloc,
            .name = "tinydeps",
            .caps = cap_read | cap_edit,
            .syntax = &syntax,
            .dialects = &dialects,
            .dialect_count = dialects.len,
            .samples = &samples,
            .sample_count = samples.len,
            .parse = parse,
            .free_bytes = TinyKv.freeBytes,
            .render_value = renderValue,
            .render_item = renderItem,
        };
    }
};

test "a runtime renderer is told the container its fragment goes into: the list's key and tag" {
    deinitAll();
    defer deinitAll();
    var alloc: TinyDeps.Alloc = .{ .allocator = testing.allocator };
    const vt = TinyDeps.vtable(&alloc);
    const abi = try register(testing.allocator, &vt);
    const e = entryByAbi(abi).?;
    var ed: editor.Editor(Language) = .{ .allocator = testing.allocator, .format = e.typeOf() };
    defer ed.deinit();
    try ed.init("deps !dep\n- a\n- b\n");
    // An appended item: the value renderer, then the item renderer, each
    // told the list is `deps`, tagged `!dep`.
    try ed.appendToSeq(&.{.{ .key = "deps" }}, "c");
    try testing.expectEqualStrings("deps !dep\n- a\n- b\n- dep:deps:c@deps\n", ed.source.items);
    // An item replaced in place reframes through the same two.
    try ed.replaceValAtPath(&.{ .{ .key = "deps" }, .{ .index = 0 } }, "z");
    try testing.expectEqualStrings("deps !dep\n- dep:deps:z@deps\n- b\n- dep:deps:c@deps\n", ed.source.items);
}

// `tinytune`: the shape of dottune's `.tune`. Blocks a blank line apart,
// each a mapping of `Name: bars` lines; a voice's bars joined by `|`, with a
// trailing one allowed; a bar's tokens joined by spaces, and a `[ ]` group's
// too. Four containers, three separators, three of them with no
// delimiters — said per row, since no one `flow_entry_sep` could. `x` is
// not a token, so an edit that writes one is the language's to reject and
// the engine's to roll back. `#` lines are comments, not rows.
const TinyTune = struct {
    pub const Alloc = TinyKv.Alloc;
    /// A separated sequence: its row, its separator, and whether it is open.
    fn seq(t: *Table, parent: u32, start: usize, end: usize, sep: []const u8, open: bool) !u32 {
        const i = try t.addRow(.{ .kind = TK.sequence, .parent = parent, .start = @intCast(start), .end = @intCast(end), .flags = if (open) table.row_open else 0 });
        try t.item_seps.append(t.allocator, .{ .node = i, .text = try t.addString(sep) });
        return i;
    }

    /// A bar's tokens and groups, `src[start..end]`, under row `bar`.
    fn tokens(t: *Table, src: []const u8, start: usize, end: usize, bar: u32, err: *ErrorInfo) !void {
        var stack: [8]u32 = undefined;
        stack[0] = bar;
        var depth: usize = 1;
        var i = start;
        while (i < end) {
            switch (src[i]) {
                ' ' => i += 1,
                '[' => {
                    if (depth == stack.len) return error.Refused;
                    stack[depth] = try seq(t, stack[depth - 1], i, i + 1, " ", false);
                    depth += 1;
                    i += 1;
                },
                ']' => {
                    if (depth == 1) {
                        err.set("a `]` with no `[`");
                        return error.Refused;
                    }
                    depth -= 1;
                    t.rows.items[stack[depth]].end = @intCast(i + 1);
                    i += 1;
                },
                else => {
                    var e = i;
                    while (e < end and src[e] != ' ' and src[e] != '[' and src[e] != ']') e += 1;
                    if (std.mem.eql(u8, src[i..e], "x")) {
                        err.set("x is not a note");
                        return error.Refused;
                    }
                    _ = try t.addRow(.{ .kind = TK.string, .parent = stack[depth - 1], .start = @intCast(i), .end = @intCast(e), .text = try t.text(src[i..e]) });
                    i = e;
                },
            }
        }
        if (depth != 1) {
            err.set("a `[` with no `]`");
            return error.Refused;
        }
    }

    fn parseInto(t: *Table, src: []const u8, err: *ErrorInfo) !void {
        const root = try seq(t, no_node, 0, 0, "\n\n", true);
        var block: ?u32 = null;
        var first = true;
        var at: usize = 0;
        while (at < src.len) {
            const nl = std.mem.indexOfScalarPos(u8, src, at, '\n') orelse src.len;
            defer at = nl + 1;
            const full = src[at..nl];
            if (std.mem.trim(u8, full, " ").len == 0) {
                block = null;
                continue;
            }
            if (full[0] == '#') continue;
            const body = std.mem.trimEnd(u8, full[0 .. std.mem.indexOfScalar(u8, full, '#') orelse full.len], " ");
            const colon = std.mem.indexOfScalar(u8, body, ':') orelse {
                err.set("expected Name: bars");
                return error.Refused;
            };
            const end = at + body.len;
            if (first) t.rows.items[root].start = @intCast(at);
            first = false;
            t.rows.items[root].end = @intCast(end);
            const b = block orelse try t.addRow(.{ .kind = TK.mapping, .parent = root, .start = @intCast(at), .end = @intCast(end) });
            block = b;
            t.rows.items[b].end = @intCast(end);
            const kv = try t.addRow(.{ .kind = TK.keyvalue, .parent = b, .start = @intCast(at), .end = @intCast(end) });
            try t.seps.append(t.allocator, .{ .node = kv, .start = @intCast(at + colon), .end = @intCast(at + colon + 1) });
            _ = try t.addRow(.{ .kind = TK.string, .parent = kv, .start = @intCast(at), .end = @intCast(at + colon), .text = try t.text(body[0..colon]) });
            const bars = try seq(t, kv, end, end, " | ", true);
            // The bars: `|`-separated segments after the colon, the last
            // dropped when it is empty — a trailing `|`.
            const music = body[colon + 1 ..];
            if (std.mem.trim(u8, music, " ").len == 0) continue;
            var segments: std.ArrayList(Span) = .empty;
            defer segments.deinit(t.allocator);
            var s: usize = 0;
            while (true) {
                const bar = std.mem.indexOfScalarPos(u8, music, s, '|') orelse music.len;
                try segments.append(t.allocator, Span.init(s, bar));
                if (bar == music.len) break;
                s = bar + 1;
            }
            const last = segments.items[segments.items.len - 1];
            if (segments.items.len > 1 and std.mem.trim(u8, music[last.start..last.end], " ").len == 0) _ = segments.pop();
            for (segments.items, 0..) |seg, i| {
                const text = music[seg.start..seg.end];
                const lead = text.len - std.mem.trimStart(u8, text, " ").len;
                const start = at + colon + 1 + seg.start + lead;
                const stop = at + colon + 1 + seg.start + std.mem.trimEnd(u8, text, " ").len;
                const bar = try seq(t, bars, start, @max(start, stop), " ", true);
                if (i == 0) t.rows.items[bars].start = @intCast(start);
                try tokens(t, src, start, @max(start, stop), bar, err);
            }
        }
    }

    fn parse(ctx: ?*anyopaque, dialect: [*:0]const u8, input: Str, out: *Table, err: *ErrorInfo) callconv(.c) c_int {
        _ = dialect;
        _ = ctx;
        parseInto(out, input.slice() orelse "", err) catch |e| return if (e == error.Refused) 2 else 255;
        return 0;
    }

    const syntax: SyntaxDesc = .{
        .comments = .{ .line = .{ .open = "#" }, .trailing = .{ .open = "#" } },
        .kv_sep = ": ",
        .empty_map_literal = null,
        .flow_containers = false,
    };
    const extensions = [_]?[*:0]const u8{ "ttune", null };
    const dialects = [_]DialectDesc{.{ .name = "tinytune", .extensions = &extensions, .splice = 2, .empty_doc_seed = "" }};
    const samples = [_]Str{Str.of("RH: 1 2 | [3 4] 5 |\nLH: I | V\n\nRH: 1 |\n")};

    pub fn vtable(alloc: *Alloc) VTable {
        return .{
            .version = vtable_version,
            .ctx = alloc,
            .name = "tinytune",
            .caps = cap_read | cap_edit,
            .syntax = &syntax,
            .dialects = &dialects,
            .dialect_count = dialects.len,
            .samples = &samples,
            .sample_count = samples.len,
            .parse = parse,
            .free_bytes = TinyKv.freeBytes,
        };
    }

    const Edit = union(enum) {
        append: []const u8,
        prepend: []const u8,
        insert: struct { usize, []const u8 },
        remove: usize,
        move: struct { usize, usize },
    };

    /// `input` with `edit` made to the sequence at `path`, which must read
    /// `expected` afterwards.
    fn expectEdit(t: Language.Type, input: []const u8, path: []const AST.PathSegment, edit: Edit, expected: []const u8) !void {
        var ed: editor.Editor(Language) = .{ .allocator = testing.allocator, .format = t };
        defer ed.deinit();
        try ed.init(input);
        switch (edit) {
            .append => |text| try ed.appendToSeq(path, text),
            .prepend => |text| try ed.prependToSeq(path, text),
            .insert => |x| try ed.insertItem(path, x[0], x[1]),
            .remove => |i| try ed.removeSeqItem(path, i),
            .move => |x| try ed.moveItem(path, x[0], x[1]),
        }
        try testing.expectEqualStrings(expected, ed.source.items);
    }
};

test "a sequence its parser gives a separator of its own is edited item by item" {
    deinitAll();
    defer deinitAll();
    var alloc: TinyTune.Alloc = .{ .allocator = testing.allocator };
    const vt = TinyTune.vtable(&alloc);
    const t = entryByAbi(try register(testing.allocator, &vt)).?.typeOf();
    const expectEdit = TinyTune.expectEdit;
    const tune = "RH: 1 2 3 4 | 5 6 7 1' |\nLH: I | V |\n\nRH: 3 - - - |\n";
    const rh: []const AST.PathSegment = &.{ .{ .index = 0 }, .{ .key = "RH" } };
    const bar: []const AST.PathSegment = &.{ .{ .index = 0 }, .{ .key = "RH" }, .{ .index = 0 } };

    // A voice's bars: `|`, open, a trailing `|` that stays trailing.
    try expectEdit(t, tune, rh, .{ .append = "1 - - -" }, "RH: 1 2 3 4 | 5 6 7 1' | 1 - - - |\nLH: I | V |\n\nRH: 3 - - - |\n");
    try expectEdit(t, tune, rh, .{ .prepend = "0" }, "RH: 0 | 1 2 3 4 | 5 6 7 1' |\nLH: I | V |\n\nRH: 3 - - - |\n");
    try expectEdit(t, tune, rh, .{ .insert = .{ 1, "9" } }, "RH: 1 2 3 4 | 9 | 5 6 7 1' |\nLH: I | V |\n\nRH: 3 - - - |\n");
    try expectEdit(t, tune, rh, .{ .remove = 0 }, "RH: 5 6 7 1' |\nLH: I | V |\n\nRH: 3 - - - |\n");
    try expectEdit(t, tune, rh, .{ .remove = 1 }, "RH: 1 2 3 4 |\nLH: I | V |\n\nRH: 3 - - - |\n");
    try expectEdit(t, tune, rh, .{ .move = .{ 1, 0 } }, "RH: 5 6 7 1' | 1 2 3 4 |\nLH: I | V |\n\nRH: 3 - - - |\n");
    // Without the trailing `|`, the same edits, and none is written.
    try expectEdit(t, "LH: I | V\n", &.{ .{ .index = 0 }, .{ .key = "LH" } }, .{ .append = "IV" }, "LH: I | V | IV\n");
    try expectEdit(t, "LH: I | V\n", &.{ .{ .index = 0 }, .{ .key = "LH" } }, .{ .remove = 1 }, "LH: I\n");

    // A bar's tokens: spaces, open, inside the voice's line.
    try expectEdit(t, tune, bar, .{ .append = "5" }, "RH: 1 2 3 4 5 | 5 6 7 1' |\nLH: I | V |\n\nRH: 3 - - - |\n");
    try expectEdit(t, tune, bar, .{ .remove = 0 }, "RH: 2 3 4 | 5 6 7 1' |\nLH: I | V |\n\nRH: 3 - - - |\n");
    try expectEdit(t, tune, bar, .{ .remove = 1 }, "RH: 1 3 4 | 5 6 7 1' |\nLH: I | V |\n\nRH: 3 - - - |\n");
    try expectEdit(t, tune, bar, .{ .insert = .{ 2, "[2 1]" } }, "RH: 1 2 [2 1] 3 4 | 5 6 7 1' |\nLH: I | V |\n\nRH: 3 - - - |\n");
    // A group: spaces, between brackets.
    const group: []const AST.PathSegment = &.{ .{ .index = 0 }, .{ .key = "RH" }, .{ .index = 0 }, .{ .index = 1 } };
    try expectEdit(t, "RH: 1 [2 3] 4 |\n", group, .{ .append = "5" }, "RH: 1 [2 3 5] 4 |\n");
    try expectEdit(t, "RH: 1 [2 3] 4 |\n", group, .{ .remove = 0 }, "RH: 1 [3] 4 |\n");

    // The blocks: a blank line, open, across lines; a block's comment goes
    // with it.
    try expectEdit(t, tune, &.{}, .{ .append = "RH: 1 |" }, tune ++ "\nRH: 1 |\n");
    try expectEdit(t, tune, &.{}, .{ .insert = .{ 1, "RH: 9 |" } }, "RH: 1 2 3 4 | 5 6 7 1' |\nLH: I | V |\n\nRH: 9 |\n\nRH: 3 - - - |\n");
    try expectEdit(t, tune, &.{}, .{ .remove = 0 }, "RH: 3 - - - |\n");
    try expectEdit(t, tune, &.{}, .{ .remove = 1 }, "RH: 1 2 3 4 | 5 6 7 1' |\nLH: I | V |\n");
    try expectEdit(t, tune, &.{}, .{ .move = .{ 1, 0 } }, "RH: 3 - - - |\n\nRH: 1 2 3 4 | 5 6 7 1' |\nLH: I | V |\n");
    try expectEdit(t, "RH: 1 |\n\n# two\nRH: 2 |\n", &.{}, .{ .remove = 1 }, "RH: 1 |\n");
    try expectEdit(t, "RH: 1 |\n\n# two\nRH: 2 |\n", &.{}, .{ .remove = 0 }, "# two\nRH: 2 |\n");
    // And the comment that ends a block's last line; a voice's comment
    // stays with the voice when one of its bars goes.
    const commented = "RH: 1 | # one\n\n# two\nRH: 2 |\n";
    try expectEdit(t, commented, &.{}, .{ .remove = 0 }, "# two\nRH: 2 |\n");
    try expectEdit(t, commented, &.{}, .{ .remove = 1 }, "RH: 1 | # one\n");
    try expectEdit(t, commented, &.{}, .{ .move = .{ 1, 0 } }, "# two\nRH: 2 |\n\nRH: 1 | # one\n");
    try expectEdit(t, "RH: 1 | 2 | # c\n", &.{ .{ .index = 0 }, .{ .key = "RH" } }, .{ .remove = 1 }, "RH: 1 | # c\n");
}

test "an edit to a separated sequence the language rejects is rolled back" {
    deinitAll();
    defer deinitAll();
    var alloc: TinyTune.Alloc = .{ .allocator = testing.allocator };
    const vt = TinyTune.vtable(&alloc);
    const t = entryByAbi(try register(testing.allocator, &vt)).?.typeOf();
    const tune = "RH: 1 2 | 3 |\n";
    var ed: editor.Editor(Language) = .{ .allocator = testing.allocator, .format = t };
    defer ed.deinit();
    try ed.init(tune);
    const bar: []const AST.PathSegment = &.{ .{ .index = 0 }, .{ .key = "RH" }, .{ .index = 0 } };
    try testing.expectError(error.RuntimeParseFailed, ed.appendToSeq(bar, "x"));
    try testing.expectError(error.RuntimeParseFailed, ed.insertItem(bar, 1, "x"));
    try testing.expectError(error.RuntimeParseFailed, ed.prependToSeq(&.{ .{ .index = 0 }, .{ .key = "RH" } }, "x"));
    try testing.expect(ed.splice_rejected);
    try testing.expectEqualStrings(tune, ed.source.items);
    // And the editor is as it was: the next edit lands.
    try ed.appendToSeq(bar, "5");
    try testing.expectEqualStrings("RH: 1 2 5 | 3 |\n", ed.source.items);
    try testing.expectError(error.NotFound, ed.insertItem(bar, 9, "1"));
}

test "an item separator is a sequence's, and an empty one is none" {
    const src = "a b";
    var t = try testTable(src, &.{
        .{ .kind = TK.sequence, .start = 0, .end = 3, .flags = table.row_open },
        .{ .kind = TK.string, .parent = 0, .start = 0, .end = 1, .text = "a" },
        .{ .kind = TK.string, .parent = 0, .start = 2, .end = 3, .text = "b" },
    });
    defer t.deinit();
    try t.item_seps.append(testing.allocator, .{ .node = 0, .text = try t.addString(" ") });
    {
        const doc = try adoptCopy(src, &t);
        defer doc.deinit(testing.allocator);
        const is = doc.itemSep(doc.ast.node(0)).?;
        try testing.expectEqualStrings(" ", is.text.?);
        try testing.expect(is.open);
        try testing.expect(doc.itemSep(doc.ast.node(1)) == null);
        // And back, as `fullTable` answers for the parse.
        var arena = std.heap.ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        const full = try fullTable(arena.allocator(), &doc);
        try testing.expectEqual(@as(usize, 1), full.item_seps.items.len);
        try testing.expectEqualStrings(" ", full.textOf(full.item_seps.items[0].text).?);
        try testing.expect(full.rows.items[0].flags & table.row_open != 0 and full.rows.items[1].flags == 0);
    }
    // An empty separator is none, and a sequence neither open nor
    // separated is not a separated one.
    t.item_seps.items[0].text = try t.addString("");
    t.rows.items[0].flags = 0;
    {
        const doc = try adoptCopy(src, &t);
        defer doc.deinit(testing.allocator);
        try testing.expect(doc.itemSep(doc.ast.node(0)) == null);
    }
    // A separator or the open flag on a row that is not a sequence.
    t.item_seps.items[0] = .{ .node = 1, .text = try t.addString(",") };
    try testing.expectError(error.MalformedTable, adoptCopy(src, &t));
    t.item_seps.clearRetainingCapacity();
    t.rows.items[1].flags = table.row_open;
    try testing.expectError(error.MalformedTable, adoptCopy(src, &t));
}

test "registration refuses a record that fails validation or the harness" {
    deinitAll();
    defer deinitAll();
    var alloc: TinyKv.Alloc = .{ .allocator = testing.allocator };
    var vt = TinyKv.vtable(&alloc);

    vt.version = 99;
    try testing.expectError(error.InvalidLanguage, register(testing.allocator, &vt));
    try testing.expect(std.mem.indexOf(u8, lastRefusal(), "vtable version 99") != null);
    vt.version = vtable_version;
    // What is registered before any refusal: none of them adds to it.
    const before = count();

    vt.samples = &.{};
    vt.sample_count = 0;
    try testing.expectError(error.InvalidLanguage, register(testing.allocator, &vt));
    try testing.expect(std.mem.indexOf(u8, lastRefusal(), "no samples") != null);

    const bad = [_]Str{Str.of("not a pair\n")};
    vt.samples = &bad;
    vt.sample_count = 1;
    try testing.expectError(error.HarnessFailed, register(testing.allocator, &vt));
    try testing.expect(std.mem.indexOf(u8, lastRefusal(), "sample does not parse") != null);
    try testing.expectEqual(before, count());

    vt.samples = &TinyKv.samples;
    vt.sample_count = TinyKv.samples.len;
    _ = try register(testing.allocator, &vt);
    try testing.expectError(error.NameTaken, register(testing.allocator, &vt));

    // A dialect row named as another language's is taken too.
    const taken = [_]DialectDesc{ .{ .name = "tinykv2" }, .{ .name = "tinykv" } };
    var vt2 = TinyKv.vtable(&alloc);
    vt2.name = "tinykv2";
    vt2.dialects = &taken;
    vt2.dialect_count = taken.len;
    try testing.expectError(error.NameTaken, register(testing.allocator, &vt2));
}

test "a record from an older or a newer header is read as far as its size says" {
    deinitAll();
    defer deinitAll();
    var alloc: TinyKv.Alloc = .{ .allocator = testing.allocator };

    // An OLDER header: a vtable that ends at `free_bytes` — no render
    // slots — with garbage where this fig's render slots sit. They must
    // read as absent, not as the garbage.
    const Older = extern struct { vt: VTable, pad: [8]u8 };
    var older: Older = .{ .vt = TinyKv.vtable(&alloc), .pad = undefined };
    older.vt.size = @offsetOf(VTable, "render_value");
    @memset(std.mem.asBytes(&older.vt)[older.vt.size..@sizeOf(VTable)], 0xAB);
    const e = entryByAbi(try register(testing.allocator, &older.vt)).?;
    try testing.expect(e.language.vt.render_value == null);
    try testing.expect(e.language.vt.render_key == null);
    deinitAll();

    // A NEWER header: a vtable, a dialect row and a syntax record each
    // longer than this fig's, their tails what a later fig would read.
    const NewerDialect = extern struct { d: DialectDesc, later: u64 };
    const newer_dialects = [_]NewerDialect{.{ .d = .{ .name = "tinykv", .extensions = &TinyKv.extensions, .splice = 2, .empty_doc_seed = "" }, .later = 0xFFFF }};
    const NewerSyntax = extern struct { s: SyntaxDesc, later: u64 };
    var newer_syntax: NewerSyntax = .{ .s = TinyKv.syntax, .later = 0xFFFF };
    newer_syntax.s.size = @sizeOf(NewerSyntax);
    const Newer = extern struct { vt: VTable, later: u64 };
    var newer: Newer = .{ .vt = TinyKv.vtable(&alloc), .later = 0xFFFF };
    newer.vt.size = @sizeOf(Newer);
    newer.vt.dialects = @ptrCast(&newer_dialects);
    newer.vt.dialect_size = @sizeOf(NewerDialect);
    newer.vt.syntax = &newer_syntax.s;
    const n = entryByAbi(try register(testing.allocator, &newer.vt)).?;
    try testing.expectEqualStrings("tkv", n.extensions[0]);
    try testing.expectEqualStrings("=", n.language.syntax.?.kv_sep.?);
    deinitAll();

    // Too short to hold what is required: refused, by name.
    var short = TinyKv.vtable(&alloc);
    short.size = @offsetOf(VTable, "free_bytes");
    try testing.expectError(error.InvalidLanguage, register(testing.allocator, &short));
    try testing.expect(std.mem.indexOf(u8, lastRefusal(), "too short") != null);
    short = TinyKv.vtable(&alloc);
    short.dialect_size = 4;
    try testing.expectError(error.InvalidLanguage, register(testing.allocator, &short));
    try testing.expect(std.mem.indexOf(u8, lastRefusal(), "dialect_size") != null);
}

test "a capability or lossless bit this fig does not know is refused" {
    deinitAll();
    defer deinitAll();
    var alloc: TinyKv.Alloc = .{ .allocator = testing.allocator };
    var vt = TinyKv.vtable(&alloc);
    vt.caps |= 1 << 20;
    try testing.expectError(error.InvalidLanguage, register(testing.allocator, &vt));
    try testing.expect(std.mem.indexOf(u8, lastRefusal(), "capability bits 0x100000") != null);
    vt = TinyKv.vtable(&alloc);
    vt.lossless = native_null;
    try testing.expectError(error.InvalidLanguage, register(testing.allocator, &vt));
    try testing.expect(std.mem.indexOf(u8, lastRefusal(), "without the envelope bit") != null);
    vt.lossless = 1 << 31;
    try testing.expectError(error.InvalidLanguage, register(testing.allocator, &vt));
    // The envelope with a native null reads back as the manifest says.
    vt.lossless = lossless_envelope | native_null | nativeExt(.local_date);
    const e = entryByAbi(try register(testing.allocator, &vt)).?;
    const kinds = e.language.caps.lossless.?;
    try testing.expect(kinds.null and kinds.local_date and !kinds.offset_datetime);
    try testing.expectEqual(vt.lossless, losslessBits(kinds));
}
