//! Lossless cross-format conversion via a reserved "$fig" envelope.
//!
//! Converting between formats normally degrades any value the target format has
//! no native form for: a `null` bound for TOML, a TOML datetime bound for JSON,
//! a ZON enum/char literal bound for anything else. Lossless mode preserves these
//! by wrapping the value in an ordinary mapping carrying a reserved sentinel key,
//! which a later fig run recognizes and reconstructs.
//!
//! Two AST passes implement it, both format-agnostic:
//!   - `encode` runs BEFORE printing. It rewrites every node the *target* format
//!     can't represent natively (`needsEnvelope`) into a `{ "$fig": { … } }`
//!     mapping. Nodes the target can hold (a `null` for JSON, a datetime for
//!     TOML) are left untouched, so idiomatic output is preserved where possible.
//!   - `decode` runs AFTER parsing. It recognizes the envelope shape in the
//!     parsed AST and rebuilds the original node kind.
//!
//! The CLI gates BOTH on `--lossless` (symmetric): a round-trip is `--lossless`
//! on each leg. By default neither runs, so a literal `$fig` key in real data is
//! treated as ordinary data both ways.
//!
//! A third pass, `lossyStrip`, is the LOSSY counterpart used when `--lossless` is
//! off: rather than the printer aborting mid-document on a value the target can't
//! represent at all (a `null` for TOML — datetimes/enums merely degrade, they
//! don't error), it removes that value up front and returns the dropped paths so
//! the CLI can warn. Output stays valid and complete; nothing is half-written.
//!
//! SCOPE (this pass): `null` and the `extended` scalars (TOML datetimes, ZON
//! enum/char literals). The YAML reference layer (custom tags, anchors, aliases)
//! is a separate future pass — those need side-table reconstruction; these are
//! pure node-kind transforms.
//!
//! Envelope schema, as it appears once parsed back into an AST:
//!   { "$fig": { "t": "<type>" } }            // a null
//!   { "$fig": { "t": "<extkind>", "v": "<text>" } }   // an extended scalar
//! where `<type>` is `"null"` and `<extkind>` is the `ExtKind` field name
//! (`offset_datetime`, `enum_literal`, …). `v` carries the scalar's intrinsic
//! text verbatim (for a char literal, its decimal codepoint, as fig stores it).

const std = @import("std");
const Allocator = std.mem.Allocator;
const AST = @import("kit").AST;
const table = @import("kit").table;
const RowWriter = @import("kit").RowWriter;
const Serialize = @import("serialize.zig");
const Language = @import("languages/language.zig");

const Id = AST.Node.Id;
const ExtKind = AST.Node.Kind.Extended.ExtKind;

/// The reserved key marking a lossless envelope. Chosen to be unlikely in real
/// config data; a real mapping that exactly mimics the envelope shape would be
/// mis-decoded (a documented limitation — escaping is a future concern).
const sentinel = "$fig";

/// What an `encode` pass targets: the scalar kinds the output format spells
/// natively, as the format itself declares them (`Language.caps.lossless`).
/// Everything not marked is enveloped. This module never names a format — it
/// reads the declaration and applies one rule to it.
pub const NativeKinds = Language.NativeKinds;

/// The native-kinds declaration the envelope pass should encode for when the
/// output is `target`, or null when it takes no envelope at all (see
/// `Caps.lossless` for which formats say null and why): what the language
/// registered declares. `canonical`, the AST's own oracle grammar, takes none.
pub fn nativeFor(target: Serialize.Target) ?NativeKinds {
    return switch (target) {
        .canonical => null,
        .language => |e| e.language.caps.lossless,
    };
}

pub const Error = Allocator.Error;

/// Does a format that holds `native` need the lossless envelope to represent
/// `kind` without loss? True only for the scalar kinds the format did not mark
/// native; every other kind (and any container) is copied through verbatim.
pub fn needsEnvelope(native: NativeKinds, kind: AST.Node.Kind) bool {
    return switch (kind) {
        .null_ => !native.null,
        // One field per `ExtKind`, named identically — pinned below.
        .extended => |e| switch (e.kind) {
            inline else => |k| !@field(native, @tagName(k)),
        },
        else => false,
    };
}

/// Whether a format that holds `native` cannot represent `kind` AT ALL (even
/// degraded) — the values the lossy `lossyStrip` pass removes. Distinct from
/// `needsEnvelope`: a TOML datetime → JSON is a `needsEnvelope` case (degrades
/// to a string in lossy mode, no data type lost beyond the tag) but NOT
/// unrepresentable. A `null` is the only kind with nothing to degrade TO — an
/// extended scalar has its text — so it is the only kind this is true for,
/// and only where the format has no `null` (TOML, today).
pub fn isUnrepresentable(native: NativeKinds, kind: AST.Node.Kind) bool {
    return kind == .null_ and !native.null;
}

// `NativeKinds` (declared in the leaf `manifest.zig`, which cannot name the
// AST) must have exactly one field per kind the envelope carries: `null`, and
// one per `ExtKind` member under the same name. Both directions, so a new
// extended scalar cannot be added without every format being asked whether it
// holds it, and a stale field cannot outlive the kind it described.
comptime {
    const fields = @typeInfo(NativeKinds).@"struct".field_names;
    const ext = @typeInfo(ExtKind).@"enum".field_names;
    if (fields.len != ext.len + 1)
        @compileError("manifest.NativeKinds must have exactly one field per ExtKind member plus `null`");
    if (!@hasField(NativeKinds, "null"))
        @compileError("manifest.NativeKinds must have a `null` field");
    for (ext) |e| {
        if (!@hasField(NativeKinds, e))
            @compileError("ExtKind." ++ e ++ " has no `manifest.NativeKinds` field — every format" ++
                " must be able to say whether it holds the new scalar natively");
    }
    for (fields, @typeInfo(NativeKinds).@"struct".field_types) |name, T| {
        if (T != bool)
            @compileError("manifest.NativeKinds." ++ name ++ " must be a bool");
    }
}

// ── Public entry points ─────────────────────────────────────────────────────

/// Build a fresh AST in `arena` where every node a format holding `native`
/// can't represent is wrapped in a `$fig` envelope. The result owns its
/// strings.
pub fn encode(arena: Allocator, ast: *const AST, native: NativeKinds) Error!AST {
    var e = Encoder{ .src = ast, .native = native, .out = .init(arena) };
    errdefer e.out.deinit();
    const root = try e.copy(ast.node(ast.root));
    return e.out.finish(ast, root);
}

/// Build a fresh AST in `arena` where every `$fig` envelope is reconstructed to
/// the node kind it encodes. A mapping that doesn't match the envelope shape (or
/// carries an unknown type tag) is copied through as ordinary data.
pub fn decode(arena: Allocator, ast: *const AST) Error!AST {
    var d = Decoder{ .src = ast, .out = .init(arena) };
    errdefer d.out.deinit();
    const root = try d.copy(ast.node(ast.root));
    return d.out.finish(ast, root);
}

/// The result of a `lossyStrip`: a new AST with every node the target can't
/// represent removed, plus the dot/bracket paths of what was dropped (for
/// warnings). `ast` is null only when the navigated root node was ITSELF
/// unrepresentable (a bare `null` → TOML), in which case there is nothing to
/// print and `dropped` names it.
pub const StripResult = struct {
    ast: ?AST,
    dropped: []const []const u8,
};

/// Build a fresh AST in `arena` rooted at the subtree `root_id`, dropping every
/// mapping entry and sequence element the target can't represent at all (today:
/// a `null` for TOML). Used in lossy mode so the printer never aborts a document
/// partway through. Dropped paths are reported relative to `root_id`.
pub fn lossyStrip(arena: Allocator, ast: *const AST, root_id: Id, native: NativeKinds) Error!StripResult {
    var s = Stripper{ .src = ast, .arena = arena, .native = native, .out = .init(arena) };
    errdefer s.out.deinit();
    if (isUnrepresentable(native, ast.node(root_id).kind)) {
        try s.dropped.append(arena, "(value)");
        return .{ .ast = null, .dropped = try s.dropped.toOwnedSlice(arena) };
    }
    const root = try s.copy(ast.node(root_id), "");
    return .{
        .ast = try s.out.finish(ast, root),
        .dropped = try s.dropped.toOwnedSlice(arena),
    };
}

// ── Encoder ─────────────────────────────────────────────────────────────────

const Encoder = struct {
    src: *const AST,
    native: NativeKinds,
    out: Rows,

    fn copy(self: *Encoder, node: AST.Node) Error!Id {
        switch (node.kind) {
            .sequence => return copySeq(self, node),
            .mapping => return copyMap(self, node),
            .keyvalue => unreachable, // keyvalues are only reached inside copyMap
            else => {},
        }
        // A leaf scalar. If it needs enveloping, the value's comments ride on the
        // wrapping mapping; otherwise they ride on the copied scalar.
        const enveloped = (node.kind == .null_ or node.kind == .extended) and needsEnvelope(self.native, node.kind);
        const id = if (enveloped) try self.envelope(node.kind) else try self.out.emit(node.kind);
        // The envelope records the scalar's kind in its `t`; a tag carried
        // onto the wrapper would assert a type on the MAPPING, which is a
        // different claim, and a custom tag has nowhere in the envelope to
        // go. Leave the wrapper untagged.
        try self.out.carry(self.src, node.id, id, .{ .tag = !enveloped });
        return id;
    }

    /// Emit `{ "$fig": { "t": <type>, ["v": <text>] } }` for a null/extended
    /// node, leaves first so child ids precede their containers.
    fn envelope(self: *Encoder, kind: AST.Node.Kind) Error!Id {
        const type_name: []const u8 = switch (kind) {
            .null_ => "null",
            .extended => |e| @tagName(e.kind),
            else => unreachable,
        };
        const text: ?[]const u8 = switch (kind) {
            .extended => |e| e.text,
            else => null,
        };

        const o = &self.out;
        const t_key = try o.emit(.{ .string = "t" });
        const t_val = try o.emit(.{ .string = type_name });
        const t_kv = try o.emit(.{ .keyvalue = .{ .key = t_key, .value = t_val } });
        const v_kv: ?Id = if (text) |vt| blk: {
            const v_key = try o.emit(.{ .string = "v" });
            const v_val = try o.emit(.{ .string = vt });
            break :blk try o.emit(.{ .keyvalue = .{ .key = v_key, .value = v_val } });
        } else null;
        const inner = try o.emit(.{ .mapping = null });
        var last: ?Id = null;
        o.link(inner, &last, t_kv);
        if (v_kv) |x| o.link(inner, &last, x);
        const s_key = try o.emit(.{ .string = sentinel });
        const fig_kv = try o.emit(.{ .keyvalue = .{ .key = s_key, .value = inner } });
        const outer = try o.emit(.{ .mapping = null });
        last = null;
        o.link(outer, &last, fig_kv);
        return outer;
    }
};

// ── Decoder ─────────────────────────────────────────────────────────────────

const Decoder = struct {
    src: *const AST,
    out: Rows,

    fn copy(self: *Decoder, node: AST.Node) Error!Id {
        switch (node.kind) {
            .mapping => {
                // A decoded envelope collapses the wrapper mapping back to a
                // scalar; the wrapper's comments move onto that scalar.
                if (self.envelopeKind(node)) |kind| {
                    const id = try self.out.emit(kind);
                    try self.out.carry(self.src, node.id, id, .{});
                    return id;
                }
                return copyMap(self, node);
            },
            .sequence => return copySeq(self, node),
            .keyvalue => unreachable,
            else => {
                const id = try self.out.emit(node.kind);
                try self.out.carry(self.src, node.id, id, .{});
                return id;
            },
        }
    }

    /// If `node` is a well-formed `$fig` envelope, return the node kind it
    /// encodes; otherwise null (so it is copied through as ordinary data). The
    /// envelope must be a mapping whose ONLY entry is the sentinel key mapping to
    /// an inner mapping carrying a string `t` (and, for extended scalars, `v`).
    fn envelopeKind(self: *Decoder, node: AST.Node) ?AST.Node.Kind {
        const first = node.kind.mapping orelse return null;
        const entry = self.src.node(first);
        if (entry.next_sibling != null) return null; // more than one key → not an envelope
        const kv = entry.kind.keyvalue;
        const key = self.src.node(kv.key);
        if (key.kind != .string or !std.mem.eql(u8, key.kind.string, sentinel)) return null;
        const inner = self.src.node(kv.value);
        if (inner.kind != .mapping) return null;

        var type_name: ?[]const u8 = null;
        var text: ?[]const u8 = null;
        var c = inner.kind.mapping;
        while (c) |cid| : (c = self.src.node(cid).next_sibling) {
            const ikv = self.src.node(cid).kind.keyvalue;
            const ik = self.src.node(ikv.key);
            if (ik.kind != .string) continue;
            const val: ?[]const u8 = switch (self.src.node(ikv.value).kind) {
                .string => |s| s,
                else => null,
            };
            if (std.mem.eql(u8, ik.kind.string, "t")) {
                type_name = val;
            } else if (std.mem.eql(u8, ik.kind.string, "v")) {
                text = val;
            }
        }

        const t = type_name orelse return null;
        if (std.mem.eql(u8, t, "null")) return .null_;
        if (std.meta.stringToEnum(ExtKind, t)) |ek|
            return .{ .extended = .{ .kind = ek, .text = text orelse "" } };
        return null; // unknown type tag → leave as ordinary mapping
    }
};

// ── Stripper (lossy) ─────────────────────────────────────────────────────────

const Stripper = struct {
    src: *const AST,
    arena: Allocator,
    native: NativeKinds,
    out: Rows,
    dropped: std.ArrayList([]const u8) = .empty,

    fn copy(self: *Stripper, node: AST.Node, path: []const u8) Error!Id {
        switch (node.kind) {
            .mapping => return self.copyMap(node, path),
            .sequence => return self.copySeq(node, path),
            .keyvalue => unreachable,
            else => {
                const id = try self.out.emit(node.kind);
                try self.out.carry(self.src, node.id, id, .{});
                return id;
            },
        }
    }

    fn copyMap(self: *Stripper, src_node: AST.Node, path: []const u8) Error!Id {
        const id = try self.out.emit(.{ .mapping = null });
        try self.out.carry(self.src, src_node.id, id, .{});
        var last: ?Id = null;
        var c = src_node.kind.mapping;
        while (c) |cid| : (c = self.src.node(cid).next_sibling) {
            const kv = self.src.node(cid).kind.keyvalue;
            const child_path = try self.keyPath(path, kv.key);
            if (isUnrepresentable(self.native, self.src.node(kv.value).kind)) {
                try self.dropped.append(self.arena, child_path);
                continue;
            }
            const new_key = try self.copy(self.src.node(kv.key), path);
            const new_val = try self.copy(self.src.node(kv.value), child_path);
            const kvid = try self.out.emit(.{ .keyvalue = .{ .key = new_key, .value = new_val } });
            self.out.link(id, &last, kvid);
        }
        return id;
    }

    fn copySeq(self: *Stripper, src_node: AST.Node, path: []const u8) Error!Id {
        const id = try self.out.emit(.{ .sequence = null });
        try self.out.carry(self.src, src_node.id, id, .{});
        var last: ?Id = null;
        var c = src_node.kind.sequence;
        var i: usize = 0;
        while (c) |cid| : (c = self.src.node(cid).next_sibling) {
            const child_path = try indexPath(self.arena, path, i);
            i += 1;
            if (isUnrepresentable(self.native, self.src.node(cid).kind)) {
                try self.dropped.append(self.arena, child_path);
                continue;
            }
            const new_id = try self.copy(self.src.node(cid), child_path);
            self.out.link(id, &last, new_id);
        }
        return id;
    }

    /// `parent.key` (or just `key` at the root). A non-string key shows as `?`
    /// (it can't be a drop target — only its value can — so this is purely a hint).
    fn keyPath(self: *Stripper, parent: []const u8, key_id: Id) Error![]const u8 {
        const name = switch (self.src.node(key_id).kind) {
            .string => |s| s,
            else => "?",
        };
        if (parent.len == 0) return self.arena.dupe(u8, name);
        return std.fmt.allocPrint(self.arena, "{s}.{s}", .{ parent, name });
    }
};

fn indexPath(arena: Allocator, parent: []const u8, i: usize) Error![]const u8 {
    return std.fmt.allocPrint(arena, "{s}[{d}]", .{ parent, i });
}

// ── Shared row-writing helpers (duck-typed over Encoder/Decoder/Stripper) ──
// `pub` so `flat_strip.zig`'s own Stripper (INI/dotenv/`.properties`'s
// depth-based capability model doesn't fit `NativeKinds`' scalar-kind shape —
// see that file's module doc) can reuse this tree-copying plumbing instead of
// a third copy of it.

/// The table a rebuilding pass writes its tree into, a row per node as it is
/// emitted, linked as it goes, and handed to `AST.own` by `finish`. It has
/// no source: every text is copied into its buffer, so the AST a pass
/// returns owns its strings and outlives the one it was read from.
///
/// Besides the nodes it carries what is bound to them — type tags, anchors,
/// comments — and the document's `%TAG` directives: a printer that spells
/// tags (YAML's `!!str`, a runtime language's own) reads them off the AST it
/// is handed, so a pass that rebuilt the tree without them would print a
/// different document. Not carried: the marker and separator spans, which
/// describe the SOURCE text — a rebuilt tree has none.
pub const Rows = struct {
    t: table.Table,

    pub fn init(arena: Allocator) Rows {
        return .{ .t = table.Table.init(arena, "") };
    }

    fn writer(self: *Rows) RowWriter {
        return RowWriter.init(&self.t);
    }

    /// A new node of `kind`, with no span: a keyvalue's key and value linked
    /// to it, a container's children added by `link`.
    pub fn emit(self: *Rows, kind: AST.Node.Kind) Error!Id {
        const id = try self.t.addRow(.{});
        try self.writer().set(id, kind);
        return id;
    }

    /// Append `child` to container `container`, after `last`.
    pub fn link(self: *Rows, container: Id, last: *?Id, child: Id) void {
        const rows = self.t.rows.items;
        rows[child].parent = container;
        if (last.*) |p| rows[p].next_sibling = child else rows[container].first_child = child;
        last.* = child;
    }

    /// Copy the type tag (unless `tag` is false), anchor and comments bound
    /// to `src`'s node `src_id` onto the freshly emitted node `new_id`. A
    /// tag is carried as `tagOf` reads it, its resolution left unstated.
    pub fn carry(self: *Rows, src: *const AST, src_id: Id, new_id: Id, opts: struct { tag: bool = true }) Error!void {
        const w = self.writer();
        if (opts.tag) if (src.tagOf(src_id)) |tag| try w.tag(new_id, tag, table.tag_unstated, true, null);
        if (src.anchorOf(src_id)) |name| try w.anchor(new_id, name, null);
        try w.comments(new_id, src.comments(src_id));
    }

    /// The tree rooted at `root` as an AST, with `src`'s `%TAG` directives
    /// and merge key.
    pub fn finish(self: *Rows, src: *const AST, root: Id) Error!AST {
        const w = self.writer();
        for (src.tag_directives) |d| try w.directive(d.handle, d.prefix);
        return AST.own(self.t.allocator, &self.t, root, src.merge_key);
    }

    pub fn deinit(self: *Rows) void {
        self.t.deinit();
    }
};

fn copySeq(self: anytype, src_node: AST.Node) Error!Id {
    const id = try self.out.emit(.{ .sequence = null });
    try self.out.carry(self.src, src_node.id, id, .{});
    var last: ?Id = null;
    var c = src_node.kind.sequence;
    while (c) |cid| : (c = self.src.node(cid).next_sibling) {
        const new_id = try self.copy(self.src.node(cid));
        self.out.link(id, &last, new_id);
    }
    return id;
}

fn copyMap(self: anytype, src_node: AST.Node) Error!Id {
    const id = try self.out.emit(.{ .mapping = null });
    try self.out.carry(self.src, src_node.id, id, .{});
    var last: ?Id = null;
    var c = src_node.kind.mapping;
    while (c) |cid| : (c = self.src.node(cid).next_sibling) {
        const kv = self.src.node(cid).kind.keyvalue;
        const new_key = try self.copy(self.src.node(kv.key));
        const new_val = try self.copy(self.src.node(kv.value));
        const kvid = try self.out.emit(.{ .keyvalue = .{ .key = new_key, .value = new_val } });
        self.out.link(id, &last, kvid);
    }
    return id;
}

// ── Tests ────────────────────────────────────────────────────────────────────

const testing = std.testing;
const JsonParser = @import("languages/json/parser.zig");
const JsonPrinter = @import("languages/json/printer.zig");
// The canonical parser is the AST-literal syntax for tests whose subject isn't a
// particular format (here: null-stripping), so they don't depend on JSON reading.
const CanonicalParser = @import("canonical/parser.zig");
const TomlParser = @import("languages/toml/parser.zig");
const TomlPrinter = @import("languages/toml/printer.zig");
const ZonParser = @import("languages/zon/parser.zig");
const ZonPrinter = @import("languages/zon/printer.zig");

// The declarations the tests encode for, read straight off each language
// module rather than through `nativeFor`: these tests import the parsers and
// printers directly too, so they run in a build that has the language gated
// out of `Language.*` (where `nativeFor` would answer null).
const json_native: NativeKinds = @import("languages/json/json.zig").Language.caps.lossless.?;
const toml_native: NativeKinds = @import("languages/toml/toml.zig").Language.caps.lossless.?;
const zon_native: NativeKinds = @import("languages/zon/zon.zig").Language.caps.lossless.?;
const yaml_native: NativeKinds = @import("languages/yaml/yaml.zig").Language.caps.lossless.?;

/// Parse `input` with `Parser`, run `decode` then `encode(native)`, print with
/// `Printer`, all inside `arena`. Returns the printed bytes (arena-owned).
fn convert(
    arena: Allocator,
    comptime Parser: type,
    src_type: anytype,
    comptime Printer: type,
    native: NativeKinds,
    input: []const u8,
) ![]const u8 {
    var ast = try Parser.parseAbstract(arena, input, src_type);
    const decoded = try decode(arena, &ast);
    const encoded = try encode(arena, &decoded, native);
    var out: std.Io.Writer.Allocating = .init(arena);
    // The JSON, ZON, and TOML printers take serialization options; YAML doesn't (yet).
    if (Printer == JsonPrinter or Printer == ZonPrinter or Printer == TomlPrinter) {
        try Printer.print(&out.writer, &encoded, .{});
    } else {
        try Printer.print(&out.writer, &encoded);
    }
    return out.written();
}

test "null round-trips through TOML" {
    var a = std.heap.ArenaAllocator.init(testing.allocator);
    defer a.deinit();
    const arena = a.allocator();

    // JSON null → TOML (must wrap; TOML has no null) → JSON (must restore).
    const toml = try convert(arena, JsonParser, .JSON, TomlPrinter, toml_native, "{\"k\": null}");
    try testing.expect(std.mem.indexOf(u8, toml, sentinel) != null); // wrapped
    const json = try convert(arena, TomlParser, .TOML_1_1, JsonPrinter, json_native, toml);
    try testing.expectEqualStrings("{\n  \"k\": null\n}\n", json);
}

test "null stays native for JSON/YAML/ZON targets" {
    var a = std.heap.ArenaAllocator.init(testing.allocator);
    defer a.deinit();
    const arena = a.allocator();

    // JSON → JSON lossless leaves a null bare (JSON has one).
    const json = try convert(arena, JsonParser, .JSON, JsonPrinter, json_native, "{\"k\": null}");
    try testing.expectEqualStrings("{\n  \"k\": null\n}\n", json);
    try testing.expect(std.mem.indexOf(u8, json, sentinel) == null);
}

test "TOML datetime round-trips through JSON" {
    var a = std.heap.ArenaAllocator.init(testing.allocator);
    defer a.deinit();
    const arena = a.allocator();

    const src = "t = 1979-05-27T07:32:00Z\n";
    const json = try convert(arena, TomlParser, .TOML_1_1, JsonPrinter, json_native, src);
    try testing.expect(std.mem.indexOf(u8, json, "offset_datetime") != null);
    const toml = try convert(arena, JsonParser, .JSON, TomlPrinter, toml_native, json);
    try testing.expectEqualStrings(src, toml);
}

test "TOML datetime stays native for a TOML target" {
    var a = std.heap.ArenaAllocator.init(testing.allocator);
    defer a.deinit();
    const arena = a.allocator();

    const src = "t = 1979-05-27T07:32:00Z\n";
    const toml = try convert(arena, TomlParser, .TOML_1_1, TomlPrinter, toml_native, src);
    try testing.expectEqualStrings(src, toml); // not wrapped
}

test "ZON enum and char literals round-trip through JSON" {
    var a = std.heap.ArenaAllocator.init(testing.allocator);
    defer a.deinit();
    const arena = a.allocator();

    const src = ".{ .mode = .fast, .ch = 'A' }";
    const json = try convert(arena, ZonParser, .ZON, JsonPrinter, json_native, src);
    try testing.expect(std.mem.indexOf(u8, json, "enum_literal") != null);
    try testing.expect(std.mem.indexOf(u8, json, "char_literal") != null);

    const zon = try convert(arena, JsonParser, .JSON, ZonPrinter, zon_native, json);
    try testing.expectEqualStrings(
        \\.{
        \\    .mode = .fast,
        \\    .ch = 'A',
        \\}
        \\
    , zon);
}

test "lossyStrip drops nulls for a TOML target and reports paths" {
    var a = std.heap.ArenaAllocator.init(testing.allocator);
    defer a.deinit();
    const arena = a.allocator();

    var ast = try CanonicalParser.parseAbstract(arena, "{\"a\": 1, \"b\": null, \"c\": [1, null, 2]}");
    const result = try lossyStrip(arena, &ast, ast.root, toml_native);
    try testing.expect(result.ast != null);

    var out: std.Io.Writer.Allocating = .init(arena);
    try TomlPrinter.print(&out.writer, &result.ast.?, .{});
    try testing.expectEqualStrings("a = 1\nc = [1, 2]\n", out.written());

    try testing.expectEqual(@as(usize, 2), result.dropped.len);
    try testing.expectEqualStrings("b", result.dropped[0]);
    try testing.expectEqualStrings("c[1]", result.dropped[1]);
}

test "lossyStrip reports a bare-null root and yields no AST" {
    var a = std.heap.ArenaAllocator.init(testing.allocator);
    defer a.deinit();
    const arena = a.allocator();

    var ast = try CanonicalParser.parseAbstract(arena, "null");
    const result = try lossyStrip(arena, &ast, ast.root, toml_native);
    try testing.expect(result.ast == null);
    try testing.expectEqual(@as(usize, 1), result.dropped.len);
}

test "lossyStrip on a nested null reports a dotted path" {
    var a = std.heap.ArenaAllocator.init(testing.allocator);
    defer a.deinit();
    const arena = a.allocator();

    var ast = try CanonicalParser.parseAbstract(arena, "{\"outer\": {\"inner\": null, \"keep\": 2}}");
    const result = try lossyStrip(arena, &ast, ast.root, toml_native);
    try testing.expect(result.ast != null);
    try testing.expectEqual(@as(usize, 1), result.dropped.len);
    try testing.expectEqualStrings("outer.inner", result.dropped[0]);
}

test "encode, decode and lossyStrip keep a document's tags and %TAG directives" {
    var a = std.heap.ArenaAllocator.init(testing.allocator);
    defer a.deinit();
    const arena = a.allocator();
    const YamlParser = @import("languages/yaml/parser.zig");
    const YamlPrinter = @import("languages/yaml/printer.zig");

    // `!e!foo` prints only beside the `%TAG` line declaring `!e!`, and
    // `!!str` is a kind tag: all three rebuilds must hand both on, or the
    // printer writes a different document (`1` an integer, `bar` untagged).
    const src = "%TAG !e! tag:x/\n---\nk: !e!foo bar\nt: !!str 1\nn: null\n";
    var ast = try YamlParser.parseAbstract(arena, src, .v1_2_2);

    const encoded = try encode(arena, &ast, toml_native);
    try testing.expect(encoded.tag_directives.len == 1);
    const decoded = try decode(arena, &encoded);
    var out: std.Io.Writer.Allocating = .init(arena);
    try YamlPrinter.print(&out.writer, &decoded);
    try testing.expectEqualStrings("%TAG !e! tag:x/\n---\nk: !e!foo bar\nt: !!str 1\nn: null\n", out.written());

    const stripped = (try lossyStrip(arena, &ast, ast.root, toml_native)).ast.?;
    var out2: std.Io.Writer.Allocating = .init(arena);
    try YamlPrinter.print(&out2.writer, &stripped);
    try testing.expectEqualStrings("%TAG !e! tag:x/\n---\nk: !e!foo bar\nt: !!str 1\n", out2.written());
}

test "an envelope carries no tag of the scalar it wraps" {
    var a = std.heap.ArenaAllocator.init(testing.allocator);
    defer a.deinit();
    const arena = a.allocator();
    const YamlParser = @import("languages/yaml/parser.zig");

    // A tag on the wrapper would assert a type on a mapping. The envelope
    // already records the kind in its `t`.
    var ast = try YamlParser.parseAbstract(arena, "k: !!null null\n", .v1_2_2);
    const encoded = try encode(arena, &ast, toml_native);
    const kv = encoded.node(encoded.node(encoded.root).kind.mapping.?).kind.keyvalue;
    try testing.expect(encoded.node(kv.value).kind == .mapping);
    try testing.expect(encoded.tagOf(kv.value) == null);
}

test "decode leaves a non-envelope $fig mapping untouched" {
    var a = std.heap.ArenaAllocator.init(testing.allocator);
    defer a.deinit();
    const arena = a.allocator();

    // value is a number, not the inner mapping shape → ordinary data.
    const json = try convert(arena, JsonParser, .JSON, JsonPrinter, json_native, "{\"$fig\": 1}");
    try testing.expectEqualStrings("{\n  \"$fig\": 1\n}\n", json);
}

// The envelope table as it was written before the formats declared it, pinned
// against the declarations that replaced it. `needsEnvelope` used to be a
// hand-written switch over `enum { json, yaml, toml, zon }` × kind; the four
// `caps.lossless` declarations are now the only source, so this is the one
// place that still states the OLD answers — a declaration that drifts from
// them fails here, not in a round-trip that quietly changed shape.
test "declared native kinds reproduce the pre-declaration envelope table" {
    const K = AST.Node.Kind;
    const Row = struct { native: NativeKinds, kind: K, envelope: bool, unrepresentable: bool };
    const ext = struct {
        fn of(k: ExtKind) K {
            return .{ .extended = .{ .kind = k, .text = "" } };
        }
    };
    const rows = [_]Row{
        // Only TOML lacks a null; JSON/YAML/ZON all have one.
        .{ .native = json_native, .kind = .null_, .envelope = false, .unrepresentable = false },
        .{ .native = yaml_native, .kind = .null_, .envelope = false, .unrepresentable = false },
        .{ .native = zon_native, .kind = .null_, .envelope = false, .unrepresentable = false },
        .{ .native = toml_native, .kind = .null_, .envelope = true, .unrepresentable = true },
        // TOML: datetimes and inf/nan native; enum/char and plist's two not.
        .{ .native = toml_native, .kind = ext.of(.offset_datetime), .envelope = false, .unrepresentable = false },
        .{ .native = toml_native, .kind = ext.of(.local_datetime), .envelope = false, .unrepresentable = false },
        .{ .native = toml_native, .kind = ext.of(.local_date), .envelope = false, .unrepresentable = false },
        .{ .native = toml_native, .kind = ext.of(.local_time), .envelope = false, .unrepresentable = false },
        .{ .native = toml_native, .kind = ext.of(.number_special), .envelope = false, .unrepresentable = false },
        .{ .native = toml_native, .kind = ext.of(.enum_literal), .envelope = true, .unrepresentable = false },
        .{ .native = toml_native, .kind = ext.of(.char_literal), .envelope = true, .unrepresentable = false },
        .{ .native = toml_native, .kind = ext.of(.plist_date), .envelope = true, .unrepresentable = false },
        .{ .native = toml_native, .kind = ext.of(.plist_data), .envelope = true, .unrepresentable = false },
        // ZON: enum/char native; everything else not.
        .{ .native = zon_native, .kind = ext.of(.enum_literal), .envelope = false, .unrepresentable = false },
        .{ .native = zon_native, .kind = ext.of(.char_literal), .envelope = false, .unrepresentable = false },
        .{ .native = zon_native, .kind = ext.of(.offset_datetime), .envelope = true, .unrepresentable = false },
        .{ .native = zon_native, .kind = ext.of(.local_datetime), .envelope = true, .unrepresentable = false },
        .{ .native = zon_native, .kind = ext.of(.local_date), .envelope = true, .unrepresentable = false },
        .{ .native = zon_native, .kind = ext.of(.local_time), .envelope = true, .unrepresentable = false },
        .{ .native = zon_native, .kind = ext.of(.number_special), .envelope = true, .unrepresentable = false },
        .{ .native = zon_native, .kind = ext.of(.plist_date), .envelope = true, .unrepresentable = false },
        .{ .native = zon_native, .kind = ext.of(.plist_data), .envelope = true, .unrepresentable = false },
        // Containers and the core scalars never envelope anywhere.
        .{ .native = toml_native, .kind = .{ .mapping = null }, .envelope = false, .unrepresentable = false },
        .{ .native = toml_native, .kind = .{ .sequence = null }, .envelope = false, .unrepresentable = false },
        .{ .native = toml_native, .kind = .{ .string = "" }, .envelope = false, .unrepresentable = false },
        .{ .native = toml_native, .kind = .{ .boolean = true }, .envelope = false, .unrepresentable = false },
        .{ .native = json_native, .kind = .{ .mapping = null }, .envelope = false, .unrepresentable = false },
    };
    for (rows) |r| {
        try testing.expectEqual(r.envelope, needsEnvelope(r.native, r.kind));
        try testing.expectEqual(r.unrepresentable, isUnrepresentable(r.native, r.kind));
    }
    // JSON and YAML: every extended kind is enveloped, none is dropped.
    inline for (@typeInfo(ExtKind).@"enum".field_names) |f_name| {
        const k = ext.of(@field(ExtKind, f_name));
        try testing.expect(needsEnvelope(json_native, k));
        try testing.expect(needsEnvelope(yaml_native, k));
        try testing.expect(!isUnrepresentable(json_native, k));
        try testing.expect(!isUnrepresentable(yaml_native, k));
    }
}

// `targetFor` used to answer `.json` for the three JSON dialects, `.yaml`,
// `.toml`, `.zon` for their own, and null for the other formats. Pinned by
// name against what each language registers: a format gaining or losing an
// envelope target is a behaviour change to be made here on purpose.
test "nativeFor reproduces the pre-declaration targetFor table" {
    const Expect = struct { fmt: []const u8, native: ?NativeKinds };
    const cases = [_]Expect{
        .{ .fmt = "json", .native = json_native },
        .{ .fmt = "jsonc", .native = json_native },
        .{ .fmt = "json5", .native = json_native },
        .{ .fmt = "yaml", .native = yaml_native },
        .{ .fmt = "toml", .native = toml_native },
        .{ .fmt = "zon", .native = zon_native },
        .{ .fmt = "canonical", .native = null },
        .{ .fmt = "fig", .native = null },
        .{ .fmt = "ini", .native = null },
        .{ .fmt = "dotenv", .native = null },
        .{ .fmt = "properties", .native = null },
        .{ .fmt = "plist", .native = null },
        .{ .fmt = "nestedtext", .native = null },
    };
    for (cases) |e| {
        // A language this build does not bundle has nothing registered to ask.
        const target = Serialize.Target.named(e.fmt) orelse continue;
        try testing.expectEqual(e.native, nativeFor(target));
    }
}
