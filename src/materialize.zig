//! Materialization: collapse a document's reference/annotation layer (aliases,
//! merge keys, tags, anchors) into the universal core AST, so a printer for a
//! format without one — or any consumer that wants concrete data — never has
//! to know about it.
//!
//! The layer is YAML's, and the rules here are YAML's (the `<<` merge, the
//! core-schema `!!` tags), but the pass reads only the AST and its
//! side-tables, so it collapses the layer whichever language built it — the
//! compiled YAML or a runtime twin of it. Which languages HAVE the layer is
//! declared, as `Caps.references` (`manifest.zig`); the CLI and the C ABI run
//! this pass when a document leaves a language that declares it for one that
//! does not. `Lossless` and `FlatStrip` are its siblings, the other two
//! AST-to-AST passes a conversion may run.
//!
//! `materialize` returns a NEW AST allocated in the given (arena) allocator:
//!   - aliases (`*name`)      → a deep copy of the anchor's subtree
//!   - merge keys (`<<`)      → flattened into the host mapping (host keys win;
//!                              earlier merge sources win over later)
//!   - core `!!` tags         → applied to the node's type, then dropped
//!   - unknown/custom tags    → strict: error; lax: dropped (node kept as parsed)
//!   - anchors                → dropped
//!
//! The third mode, `keep`, collapses only the reference half: every tag rides
//! along on its node's copy, unapplied and unchecked, with the `%TAG`
//! directives that spell it. It is what a binding reads values through
//! (`fig_document_resolve_aliases`), which follows an alias but has never
//! applied a tag.
//!
//! The pass writes its tree as rows, into a table with no source, so the
//! result owns its strings: every text is copied into its own buffer, and it
//! outlives `ast` and the source `ast` was read from.

const std = @import("std");
const Allocator = std.mem.Allocator;
const AST = @import("kit").AST;
const table = @import("kit").table;
const RowWriter = @import("kit").RowWriter;
// The core schema's number grammar, as the compiled YAML reads a plain
// scalar — pure functions of a lexeme, so a runtime twin's AST is judged by
// the same rules.
const yaml_scalar = @import("kit").yaml_schema;

pub const TagMode = enum { strict, lax, keep };

pub const Error = error{
    AliasCycle,
    UnknownTag,
    TagTypeMismatch,
} || AST.ResolveError || Allocator.Error;

/// Collapse `ast`'s reference layer into a fresh core AST built in `arena`.
pub fn materialize(arena: Allocator, ast: *const AST, mode: TagMode) Error!AST {
    var culprit: ?AST.Node.Id = null;
    return materializeReporting(arena, ast, mode, &culprit);
}

/// `materialize`, but on failure also sets `culprit` to the id (in `ast`) of
/// the node the pass failed on — the tagged node an `UnknownTag` or
/// `TagTypeMismatch` is about, the alias an `AliasCycle` came back through —
/// so a caller holding the node's source span can point at it. Untouched on
/// success.
pub fn materializeReporting(arena: Allocator, ast: *const AST, mode: TagMode, culprit: *?AST.Node.Id) Error!AST {
    var m: Materializer = .{ .src = ast, .arena = arena, .mode = mode, .t = .init(arena, "") };
    defer m.path.deinit(arena);
    errdefer m.t.deinit();
    const root = m.copy(ast.node(ast.root)) catch |err| {
        culprit.* = m.culprit;
        return err;
    };
    // A kept tag may be spelled through a `%TAG` handle, so its declaration
    // rides along too.
    if (mode == .keep) for (ast.tag_directives) |d| try m.writer().directive(d.handle, d.prefix);
    return AST.own(arena, &m.t, root, ast.merge_key);
}

const Materializer = struct {
    src: *const AST,
    arena: Allocator,
    mode: TagMode,
    /// The rows of the tree being written, a row per node as `copy` emits it,
    /// with the comments and tags carried onto them. Every text is copied
    /// into its buffer: it has no source.
    t: table.Table,
    /// Anchor target ids currently being expanded, to catch structural cycles
    /// (`&a [*a]`) and self-merges (`&m { <<: *m }`).
    path: std.ArrayList(AST.Node.Id) = .empty,
    /// The innermost source node whose copy failed — the first `copy` frame
    /// an error unwinds through. See `materializeReporting`.
    culprit: ?AST.Node.Id = null,

    fn writer(self: *Materializer) RowWriter {
        return RowWriter.init(&self.t);
    }

    /// A new row of `kind`, with no span: a keyvalue's key and value linked
    /// to it, a container's children added by `link`.
    fn emit(self: *Materializer, kind: AST.Node.Kind) Error!AST.Node.Id {
        const id = try self.t.addRow(.{});
        try self.writer().set(id, kind);
        return id;
    }

    /// Copy the comments bound to source node `src_id` onto the freshly emitted
    /// node `new_id`, their texts into the table's buffer.
    fn carry(self: *Materializer, src_id: AST.Node.Id, new_id: AST.Node.Id) Error!void {
        try self.writer().comments(new_id, self.src.comments(src_id));
    }

    fn enter(self: *Materializer, id: AST.Node.Id) Error!void {
        for (self.path.items) |p| if (p == id) return error.AliasCycle;
        try self.path.append(self.arena, id);
    }
    fn leave(self: *Materializer) void {
        _ = self.path.pop();
    }

    /// Deep-copy `node` into the output, expanding aliases/merges and applying
    /// tags. Returns the new node id.
    fn copy(self: *Materializer, node: AST.Node) Error!AST.Node.Id {
        errdefer if (self.culprit == null) {
            self.culprit = node.id;
        };
        switch (node.kind) {
            .alias => {
                const target = try self.src.resolveDeep(node);
                try self.enter(target);
                defer self.leave();
                return self.copy(self.src.node(target));
            },
            // `.extended` never appears in a YAML AST, but the switch must be
            // exhaustive; treat it as a plain scalar for completeness.
            .null_, .boolean, .number, .extended, .string => {
                const id = try self.emit(try self.applyScalarTag(node));
                try self.carry(node.id, id);
                try self.carryScalarTag(node.id, id);
                return id;
            },
            .sequence => |first| {
                try self.checkCollectionTag(node, .seq);
                const id = try self.emit(.{ .sequence = null });
                try self.carry(node.id, id);
                try self.keepTag(node.id, id);
                var last: ?AST.Node.Id = null;
                var child = first;
                while (child) |cid| : (child = self.src.node(cid).next_sibling) {
                    const new_id = try self.copy(self.src.node(cid));
                    self.link(id, &last, new_id);
                }
                return id;
            },
            .mapping => return self.copyMapping(node),
            .keyvalue => unreachable, // keyvalues are only produced inside copyMapping
        }
    }

    /// Append `child_id` to container `container_id`, after `last`.
    fn link(self: *Materializer, container_id: AST.Node.Id, last: *?AST.Node.Id, child_id: AST.Node.Id) void {
        const rows = self.t.rows.items;
        rows[child_id].parent = container_id;
        if (last.*) |p| rows[p].next_sibling = child_id else rows[container_id].first_child = child_id;
        last.* = child_id;
    }

    fn copyMapping(self: *Materializer, node: AST.Node) Error!AST.Node.Id {
        try self.checkCollectionTag(node, .map);
        const id = try self.emit(.{ .mapping = null });
        try self.carry(node.id, id);
        try self.keepTag(node.id, id);
        var last: ?AST.Node.Id = null;
        var seen: std.ArrayList([]const u8) = .empty;

        // Local entries first (they win); remember any `<<` merge value.
        var merge_value: ?AST.Node.Id = null;
        var entry = node.kind.mapping;
        while (entry) |eid| : (entry = self.src.node(eid).next_sibling) {
            const kv = self.src.node(eid).kind.keyvalue;
            if (self.stringKey(kv.key)) |ks| if (self.isMergeKey(ks)) {
                merge_value = kv.value;
                continue;
            };
            try self.appendEntry(id, &last, &seen, kv.key, kv.value);
        }
        if (merge_value) |mv| try self.mergeInto(id, &last, &seen, mv);
        return id;
    }

    /// Copy a key/value pair into mapping `map_id` and record the (string) key.
    fn appendEntry(self: *Materializer, map_id: AST.Node.Id, last: *?AST.Node.Id, seen: *std.ArrayList([]const u8), key: AST.Node.Id, value: AST.Node.Id) Error!void {
        const key_str = self.stringKey(key);
        const new_key = try self.copy(self.src.node(key));
        const new_val = try self.copy(self.src.node(value));
        const kvid = try self.emit(.{ .keyvalue = .{ .key = new_key, .value = new_val } });
        self.link(map_id, last, kvid);
        if (key_str) |ks| try seen.append(self.arena, ks);
    }

    /// Splice the entries of a `<<` merge value into mapping `map_id`, skipping
    /// keys already present (`seen`). Handles a single alias/mapping or a sequence
    /// of them (earlier sources win because they are processed first).
    fn mergeInto(self: *Materializer, map_id: AST.Node.Id, last: *?AST.Node.Id, seen: *std.ArrayList([]const u8), mv: AST.Node.Id) Error!void {
        const mvn = self.src.node(mv);
        switch (mvn.kind) {
            .sequence => |first| {
                var e = first;
                while (e) |eid| : (e = self.src.node(eid).next_sibling) {
                    try self.mergeOneSource(map_id, last, seen, self.src.node(eid));
                }
            },
            else => try self.mergeOneSource(map_id, last, seen, mvn),
        }
    }

    fn mergeOneSource(self: *Materializer, map_id: AST.Node.Id, last: *?AST.Node.Id, seen: *std.ArrayList([]const u8), source: AST.Node) Error!void {
        const target_id = try self.src.resolveDeep(source);
        try self.enter(target_id); // guards a self-referential merge
        defer self.leave();
        const target = self.src.node(target_id);
        if (target.kind != .mapping) return; // a non-mapping merge source is ignored

        var entry = target.kind.mapping;
        while (entry) |eid| : (entry = self.src.node(eid).next_sibling) {
            const kv = self.src.node(eid).kind.keyvalue;
            const key_str = self.stringKey(kv.key);
            if (key_str) |ks| {
                if (self.isMergeKey(ks)) {
                    try self.mergeInto(map_id, last, seen, kv.value); // nested merge in a source
                    continue;
                }
                if (contains(seen.items, ks)) continue; // shadowed by a local or earlier source
            }
            try self.appendEntry(map_id, last, seen, kv.key, kv.value);
        }
    }

    fn isMergeKey(self: *const Materializer, key: []const u8) bool {
        const mk = self.src.merge_key orelse return false;
        return std.mem.eql(u8, key, mk);
    }

    fn stringKey(self: *const Materializer, key: AST.Node.Id) ?[]const u8 {
        return switch (self.src.node(key).kind) {
            .string => |s| s,
            else => null,
        };
    }

    // ── tags ──────────────────────────────────────────────────────────────

    /// What node `id`'s tag says, as the language resolved it (§4.3 of
    /// docs/proposals/core-only.md): a core type, and whether the node
    /// agrees with it, or custom, or YAML's non-specific `!`. A tag its
    /// language left unstated — a runtime twin's — is read by its text,
    /// with the document's `%TAG` directives, and its agreement is
    /// judged here.
    const Resolved = union(enum) {
        none,
        non_specific,
        custom,
        core: struct { kind: AST.Tag.KindTag, agrees: ?bool },
    };

    fn resolve(self: *const Materializer, id: AST.Node.Id) Resolved {
        const f = self.src.tagFact(id) orelse return .none;
        if (f.resolved >= table.tag_kind) return .{ .core = .{ .kind = @fromBackingInt(@intCast(f.resolved - table.tag_kind)), .agrees = f.agrees != 0 } };
        if (f.resolved == table.tag_custom) return .custom;
        if (f.resolved == table.tag_non_specific) return .non_specific;
        return switch (self.src.tagOfFact(f)) {
            .kind => |k| .{ .core = .{ .kind = k, .agrees = null } },
            .text => |t| blk: {
                if (std.mem.eql(u8, t, "!")) break :blk .non_specific;
                const name = yaml_scalar.coreTagName(self.src.tag_directives, t) orelse break :blk .custom;
                const k = AST.Tag.ofCoreName(name) orelse break :blk .custom;
                break :blk .{ .core = .{ .kind = k, .agrees = null } };
            },
        };
    }

    fn applyScalarTag(self: *const Materializer, node: AST.Node) Error!AST.Node.Kind {
        if (self.mode == .keep) return node.kind;
        const c = switch (self.resolve(node.id)) {
            .none, .non_specific => return node.kind,
            .custom => return self.customTag(node.kind),
            .core => |c| c,
        };
        if (c.agrees) |ok| if (!ok) return error.TagTypeMismatch;
        return switch (c.kind) {
            .string => .{ .string = self.scalarText(node) },
            .null_ => .null_,
            .boolean => self.asBool(node),
            .integer => self.asNumber(node, .integer, c.agrees),
            .float => self.asNumber(node, .float, c.agrees),
            .sequence, .mapping => error.TagTypeMismatch,
        };
    }

    fn checkCollectionTag(self: *const Materializer, node: AST.Node, comptime want: enum { seq, map }) Error!void {
        if (self.mode == .keep) return;
        switch (self.resolve(node.id)) {
            .none, .non_specific => {},
            .custom => _ = try self.customTag(node.kind), // strict-errors on unknown; lax no-op
            .core => |c| {
                const ok: AST.Tag.KindTag = if (want == .seq) .sequence else .mapping;
                if (c.kind != ok) return error.TagTypeMismatch;
            },
        }
    }

    /// Keep an applied core SCALAR tag on the output node as a normalized `.kind`
    /// type tag, so a tag-aware target re-surfaces it (fig `: int =`, canonical
    /// `!!int`), while JSON/TOML/ZON ignore it (the value is already concrete).
    /// A custom/collection tag is not carried — it was already applied
    /// (collections) or errored/dropped (custom) by `applyScalarTag`.
    fn carryScalarTag(self: *Materializer, src_id: AST.Node.Id, new_id: AST.Node.Id) Error!void {
        if (self.mode == .keep) return self.keepTag(src_id, new_id);
        const c = switch (self.resolve(src_id)) {
            .core => |c| c,
            else => return,
        };
        switch (c.kind) {
            .sequence, .mapping => return,
            else => {},
        }
        try self.writer().tag(new_id, .{ .kind = c.kind }, table.tag_unstated, true, null);
    }

    /// Under `keep`, put source node `src_id`'s tag, whatever it is, on its
    /// copy `new_id`. A no-op in the other modes, which apply or drop it.
    fn keepTag(self: *Materializer, src_id: AST.Node.Id, new_id: AST.Node.Id) Error!void {
        if (self.mode != .keep) return;
        const tag = self.src.tagOf(src_id) orelse return;
        try self.writer().tag(new_id, tag, table.tag_unstated, true, null);
    }

    /// A custom tag, which strict mode refuses and lax mode drops, keeping
    /// the node as parsed.
    fn customTag(self: *const Materializer, kind: AST.Node.Kind) Error!AST.Node.Kind {
        if (self.mode == .strict) return error.UnknownTag;
        return kind;
    }

    /// A `!!int`/`!!float` payload, refused unless its text reads as that
    /// kind — every printer writes a number's text bare, so `!!int 1 - 3`
    /// would print as `1 - 3` in JSON. Read by YAML 1.2's core schema or
    /// 1.1's, since the AST does not say which the document was: `!!int
    /// 0x1A` and `!!int 1_000` both pass. A `!!float` also takes a decimal
    /// integer's text (`!!float 1`), as the core schema's float does.
    fn asNumber(self: *const Materializer, node: AST.Node, comptime want: @FieldType(AST.Node.Kind.Number, "kind"), stated: ?bool) Error!AST.Node.Kind {
        const t = self.scalarText(node);
        const ok = if (stated) |x| x else switch (want) {
            .integer => yaml_scalar.classifyNumber(t) == .integer or yaml_scalar.classify1_1Number(t) == .integer,
            .float => (yaml_scalar.classifyNumber(t) != .not_number and !yaml_scalar.hasRadixPrefix(t)) or
                yaml_scalar.classify1_1Number(t) == .float,
        };
        if (!ok) return error.TagTypeMismatch;
        return .{ .number = .{ .raw = t, .kind = want } };
    }

    fn asBool(self: *const Materializer, node: AST.Node) Error!AST.Node.Kind {
        const t = self.scalarText(node);
        if (eqlAny(t, &.{ "true", "True", "TRUE" })) return .{ .boolean = true };
        if (eqlAny(t, &.{ "false", "False", "FALSE" })) return .{ .boolean = false };
        return error.TagTypeMismatch;
    }

    /// The scalar's text for `!!str`-style forcing. Numbers and strings are exact;
    /// bool/null are reconstructed in canonical form (the original presentation is
    /// not retained on the AST, which is source-free).
    fn scalarText(self: *const Materializer, node: AST.Node) []const u8 {
        _ = self;
        return switch (node.kind) {
            .string => |s| s,
            .number => |n| n.raw,
            .boolean => |b| if (b) "true" else "false",
            .null_ => "null",
            else => "",
        };
    }
};

fn contains(haystack: []const []const u8, needle: []const u8) bool {
    for (haystack) |s| if (std.mem.eql(u8, s, needle)) return true;
    return false;
}

fn eqlAny(s: []const u8, options: []const []const u8) bool {
    return contains(options, s);
}

// ── tests ────────────────────────────────────────────────────────────────
const testing = std.testing;
const build_options = @import("build_options");
// By path: these exercise the pass over what the
// compiled YAML parser builds, and skip when it is gated out of the build.
const Parser = @import("fig_yaml").Parser;

fn hasAlias(ast: AST) bool {
    for (0..ast.nodeCount()) |i| if (ast.kindOf(@intCast(i)) == .alias) return true;
    return false;
}

/// `src` as the compiled YAML reads it, as fig adopts it.
fn parseYaml(src: []const u8) !@import("kit").Document {
    return Parser.parse(testing.allocator, src, .v1_2_2);
}

test "materialize: alias expands to a copied subtree" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const doc = try parseYaml("a: &x [1, 2]\nb: *x\n");
    defer doc.deinit(testing.allocator);
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const mat = try materialize(arena.allocator(), &doc.ast, .strict);

    try testing.expect(!hasAlias(mat)); // no alias survives
    const b = try mat.getValByPath(&.{.{ .key = "b" }});
    try testing.expect(b.kind == .sequence);
    const first = try mat.getValByPath(&.{ .{ .key = "b" }, .{ .index = 0 } });
    try testing.expectEqualSlices(u8, "1", first.kind.number.raw);
    // The copy is independent of the original `a` (distinct node ids).
    const a = try mat.getValByPath(&.{.{ .key = "a" }});
    try testing.expect(a.id != b.id);
}

test "materialize: merge flattens with host and earlier-source precedence" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const src =
        \\base: &b
        \\  x: 1
        \\  y: 2
        \\over: &o
        \\  x: 9
        \\d:
        \\  <<: [*o, *b]
        \\  y: 3
        \\
    ;
    const doc = try parseYaml(src);
    defer doc.deinit(testing.allocator);
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const mat = try materialize(arena.allocator(), &doc.ast, .strict);

    try testing.expectEqualSlices(u8, "3", (try mat.getValByPath(&.{ .{ .key = "d" }, .{ .key = "y" } })).kind.number.raw);
    try testing.expectEqualSlices(u8, "9", (try mat.getValByPath(&.{ .{ .key = "d" }, .{ .key = "x" } })).kind.number.raw);
    // The `<<` key itself is gone from the flattened mapping.
    try testing.expectError(error.NotFound, mat.getValByPath(&.{ .{ .key = "d" }, .{ .key = "<<" } }));
}

test "materialize: core tags applied AND kept as normalized kind tags" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const doc = try parseYaml("a: !!str 123\nb: !!int \"42\"\nc: !!null x\nd: 7\n");
    defer doc.deinit(testing.allocator);
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const mat = try materialize(arena.allocator(), &doc.ast, .strict);

    // The tag is applied to the node's kind …
    const a = try mat.getValByPath(&.{.{ .key = "a" }});
    try testing.expectEqualSlices(u8, "123", a.kind.string);
    const b = try mat.getValByPath(&.{.{ .key = "b" }});
    try testing.expect(b.kind == .number and b.kind.number.kind == .integer);
    try testing.expect((try mat.getValByPath(&.{.{ .key = "c" }})).kind == .null_);
    // … and its type identity is KEPT as a normalized `.kind` tag, so a tag-aware
    // target (fig `: type =`, canonical `!!type`) can re-surface it.
    try testing.expect(mat.tagOf(a.id).?.kind == .string);
    try testing.expect(mat.tagOf(b.id).?.kind == .integer);
    // An untagged scalar carries no tag.
    try testing.expect(mat.tagOf((try mat.getValByPath(&.{.{ .key = "d" }})).id) == null);
}

test "materialize: unknown tag strict errors, lax drops" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const doc = try parseYaml("x: !custom 1\n");
    defer doc.deinit(testing.allocator);
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    try testing.expectError(error.UnknownTag, materialize(arena.allocator(), &doc.ast, .strict));
    const mat = try materialize(arena.allocator(), &doc.ast, .lax);
    try testing.expectEqualSlices(u8, "1", (try mat.getValByPath(&.{.{ .key = "x" }})).kind.number.raw);
}

test "materializeReporting names the node a tag failure is about" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const src = "ok: 1\nlist:\n  - 2\n  - !custom 3\n";
    const doc = try parseYaml(src);
    defer doc.deinit(testing.allocator);
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    var culprit: ?AST.Node.Id = null;
    try testing.expectError(error.UnknownTag, materializeReporting(arena.allocator(), &doc.ast, .strict, &culprit));
    // The tagged item itself, not the sequence or mapping around it: its tag
    // span is what a report underlines.
    const span = doc.tagSpan(doc.ast.node(culprit.?)).?;
    try testing.expectEqualStrings("!custom", src[span.start..span.end]);

    // Reached through an alias, the failure is still about the anchored node
    // that carries the tag.
    const aliased = "a: &x !custom 1\nb: *x\n";
    const doc2 = try parseYaml(aliased);
    defer doc2.deinit(testing.allocator);
    culprit = null;
    try testing.expectError(error.UnknownTag, materializeReporting(arena.allocator(), &doc2.ast, .strict, &culprit));
    const span2 = doc2.tagSpan(doc2.ast.node(culprit.?)).?;
    try testing.expectEqualStrings("!custom", aliased[span2.start..span2.end]);
}

test "materialize: collection-tag mismatch and cyclic alias error" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    {
        const doc = try parseYaml("x: !!str [1, 2]\n");
        defer doc.deinit(testing.allocator);
        var arena = std.heap.ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        try testing.expectError(error.TagTypeMismatch, materialize(arena.allocator(), &doc.ast, .strict));
    }
    {
        const doc = try parseYaml("- &a [*a]\n");
        defer doc.deinit(testing.allocator);
        var arena = std.heap.ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        try testing.expectError(error.AliasCycle, materialize(arena.allocator(), &doc.ast, .strict));
    }
}

test "materialize: a numeric tag refuses a payload that is not that kind of number" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const cases = [_]struct { src: []const u8, ok: bool }{
        .{ .src = "x: !!int 1 - 3\n", .ok = false },
        .{ .src = "x: !!int 1.5\n", .ok = false },
        .{ .src = "x: !!float abc\n", .ok = false },
        .{ .src = "x: !!float 0x1A\n", .ok = false },
        .{ .src = "x: !!int 0x1A\n", .ok = true },
        .{ .src = "x: !!int \"42\"\n", .ok = true },
        .{ .src = "x: !!int 1_000\n", .ok = true }, // YAML 1.1's spelling
        .{ .src = "x: !!float 1\n", .ok = true },
        .{ .src = "x: !!float -.inf\n", .ok = true },
    };
    for (cases) |c| {
        const doc = try parseYaml(c.src);
        defer doc.deinit(testing.allocator);
        var arena = std.heap.ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        const got = materialize(arena.allocator(), &doc.ast, .strict);
        if (c.ok) _ = try got else try testing.expectError(error.TagTypeMismatch, got);
    }
}

test "materialize: a %TAG directive that rebinds `!!` or `!` decides whether a tag is core" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    {
        // P76L: `!!` bound elsewhere makes `!!int` a custom tag.
        const doc = try parseYaml("%TAG !! tag:example.com,2000:app/\n---\n!!int 1 - 3\n");
        defer doc.deinit(testing.allocator);
        var arena = std.heap.ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        try testing.expectError(error.UnknownTag, materialize(arena.allocator(), &doc.ast, .strict));
    }
    {
        // `!` bound to the core prefix makes `!int` the core int.
        const doc = try parseYaml("%TAG ! tag:yaml.org,2002:\n---\nx: !int \"7\"\n");
        defer doc.deinit(testing.allocator);
        var arena = std.heap.ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        const mat = try materialize(arena.allocator(), &doc.ast, .strict);
        const x = try mat.getValByPath(&.{.{ .key = "x" }});
        try testing.expect(x.kind == .number and x.kind.number.kind == .integer);
    }
}

test "materialize: keep collapses aliases and merges and leaves every tag as written" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const src =
        \\base: &b {x: !!str 1, y: !custom 2}
        \\d:
        \\  <<: *b
        \\  y: 3
        \\e: *b
        \\
    ;
    const doc = try parseYaml(src);
    defer doc.deinit(testing.allocator);
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const mat = try materialize(arena.allocator(), &doc.ast, .keep);

    try testing.expect(!hasAlias(mat));
    // The merge is flattened, the host's key winning …
    try testing.expectEqualSlices(u8, "3", (try mat.getValByPath(&.{ .{ .key = "d" }, .{ .key = "y" } })).kind.number.raw);
    try testing.expectError(error.NotFound, mat.getValByPath(&.{ .{ .key = "d" }, .{ .key = "<<" } }));
    // … and `!!str 1` is still the number it was read as, its tag carried
    // verbatim onto the merged copy and the alias's copy alike.
    for ([_][]const u8{ "d", "e" }) |host| {
        const x = try mat.getValByPath(&.{ .{ .key = host }, .{ .key = "x" } });
        try testing.expectEqualSlices(u8, "1", x.kind.number.raw);
        try testing.expectEqualStrings("!!str", mat.tagOf(x.id).?.spelling());
    }
    // A custom tag is neither refused nor dropped.
    const ey = try mat.getValByPath(&.{ .{ .key = "e" }, .{ .key = "y" } });
    try testing.expectEqualStrings("!custom", mat.tagOf(ey.id).?.text);
}

test "materialize: a `<<` key merges only where the language declares it as its merge key" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var doc = try parseYaml("base: &b {x: 1}\nd: {<<: *b}\n");
    defer doc.deinit(testing.allocator);
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const merged = try materialize(arena.allocator(), &doc.ast, .strict);
    _ = try merged.getValByPath(&.{ .{ .key = "d" }, .{ .key = "x" } });

    // The same tree from a language with no merge key keeps `<<` as a key.
    doc.ast.merge_key = null;
    const kept = try materialize(arena.allocator(), &doc.ast, .strict);
    try testing.expectError(error.NotFound, kept.getValByPath(&.{ .{ .key = "d" }, .{ .key = "x" } }));
    _ = try kept.getValByPath(&.{ .{ .key = "d" }, .{ .key = "<<" } });
}
