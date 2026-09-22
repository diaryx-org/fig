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
//! String slices in the result borrow from the source AST (source text or its
//! `owned_strings`); the result must not outlive them. With an arena that holds
//! both, this is automatic.

const std = @import("std");
const Allocator = std.mem.Allocator;
const AST = @import("ast/ast.zig");
// The core schema's number grammar, as the compiled YAML reads a plain
// scalar — pure functions of a lexeme, so a runtime twin's AST is judged by
// the same rules.
const yaml_scalar = @import("languages/yaml/parser.zig");

pub const TagMode = enum { strict, lax };

pub const Error = error{
    AliasCycle,
    UnknownTag,
    TagTypeMismatch,
} || AST.ResolveError || Allocator.Error;

/// Collapse `ast`'s reference layer into a fresh core AST built in `arena`.
pub fn materialize(arena: Allocator, ast: *const AST, mode: TagMode) Error!AST {
    var m: Materializer = .{ .src = ast, .arena = arena, .mode = mode };
    const root = try m.copy(ast.nodes[ast.root]);
    var result: AST = .{
        .allocator = arena,
        .owned_strings = &.{},
        .root = root,
        .nodes = try m.out.toOwnedSlice(arena),
    };
    // Comments ride along: `copy` carried each source node's comments onto its
    // new id, so the parallel table just hands off (only if any were present).
    if (m.any_comments) result.node_comments = try m.out_comments.toOwnedSlice(arena);
    if (m.any_tags) result.node_tags = try m.out_tags.toOwnedSlice(arena);
    return result;
}

const Materializer = struct {
    src: *const AST,
    arena: Allocator,
    mode: TagMode,
    out: std.ArrayList(AST.Node) = .empty,
    /// Parallel to `out`: comments carried from each source node onto its copy.
    /// Default-empty per node; materialized only when `any_comments`.
    out_comments: std.ArrayList(AST.NodeComments) = .empty,
    any_comments: bool = false,
    /// Parallel to `out`: the NORMALIZED type tag kept from an applied core `!!`
    /// scalar tag. Applying the tag coerces the node's kind; keeping the identity
    /// here (as a cross-format `.kind` tag) lets a tag-aware target re-surface it
    /// — a fig `: int =`, a canonical `!!int` — while JSON/TOML/ZON ignore it (the
    /// value is already concrete). Null per node; materialized only when `any_tags`.
    out_tags: std.ArrayList(?AST.Tag) = .empty,
    any_tags: bool = false,
    /// Anchor target ids currently being expanded, to catch structural cycles
    /// (`&a [*a]`) and self-merges (`&m { <<: *m }`).
    path: std.ArrayList(AST.Node.Id) = .empty,

    fn emit(self: *Materializer, kind: AST.Node.Kind) Error!AST.Node.Id {
        const id: AST.Node.Id = @intCast(self.out.items.len);
        try self.out.append(self.arena, .{ .id = id, .kind = kind, .next_sibling = null });
        try self.out_comments.append(self.arena, .{});
        try self.out_tags.append(self.arena, null);
        return id;
    }

    /// Copy the comments bound to source node `src_id` onto the freshly emitted
    /// node `new_id`. The `leading` slice is re-duped into the arena; comment
    /// text borrows the source AST, which the arena outlives.
    fn carry(self: *Materializer, src_id: AST.Node.Id, new_id: AST.Node.Id) Error!void {
        const c = self.src.comments(src_id);
        if (c.isEmpty()) return;
        self.out_comments.items[new_id] = .{
            .leading = try self.arena.dupe(AST.Comment, c.leading),
            .trailing = c.trailing,
            .dangling = try self.arena.dupe(AST.Comment, c.dangling),
        };
        self.any_comments = true;
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
        switch (node.kind) {
            .alias => {
                const target = try self.src.resolveDeep(node);
                try self.enter(target);
                defer self.leave();
                return self.copy(self.src.nodes[target]);
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
                var last: ?AST.Node.Id = null;
                var child = first;
                while (child) |cid| : (child = self.src.nodes[cid].next_sibling) {
                    const new_id = try self.copy(self.src.nodes[cid]);
                    self.link(id, &last, new_id, .sequence);
                }
                return id;
            },
            .mapping => return self.copyMapping(node),
            .keyvalue => unreachable, // keyvalues are only produced inside copyMapping
        }
    }

    /// Append `child_id` to container `container_id`, threading siblings.
    fn link(self: *Materializer, container_id: AST.Node.Id, last: *?AST.Node.Id, child_id: AST.Node.Id, comptime kind: enum { sequence, mapping }) void {
        if (last.*) |p| {
            self.out.items[p].next_sibling = child_id;
        } else {
            self.out.items[container_id].kind = switch (kind) {
                .sequence => .{ .sequence = child_id },
                .mapping => .{ .mapping = child_id },
            };
        }
        last.* = child_id;
    }

    fn copyMapping(self: *Materializer, node: AST.Node) Error!AST.Node.Id {
        try self.checkCollectionTag(node, .map);
        const id = try self.emit(.{ .mapping = null });
        try self.carry(node.id, id);
        var last: ?AST.Node.Id = null;
        var seen: std.ArrayList([]const u8) = .empty;

        // Local entries first (they win); remember any `<<` merge value.
        var merge_value: ?AST.Node.Id = null;
        var entry = node.kind.mapping;
        while (entry) |eid| : (entry = self.src.nodes[eid].next_sibling) {
            const kv = self.src.nodes[eid].kind.keyvalue;
            if (self.stringKey(kv.key)) |ks| if (std.mem.eql(u8, ks, "<<")) {
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
        const new_key = try self.copy(self.src.nodes[key]);
        const new_val = try self.copy(self.src.nodes[value]);
        const kvid = try self.emit(.{ .keyvalue = .{ .key = new_key, .value = new_val } });
        self.link(map_id, last, kvid, .mapping);
        if (key_str) |ks| try seen.append(self.arena, ks);
    }

    /// Splice the entries of a `<<` merge value into mapping `map_id`, skipping
    /// keys already present (`seen`). Handles a single alias/mapping or a sequence
    /// of them (earlier sources win because they are processed first).
    fn mergeInto(self: *Materializer, map_id: AST.Node.Id, last: *?AST.Node.Id, seen: *std.ArrayList([]const u8), mv: AST.Node.Id) Error!void {
        const mvn = self.src.nodes[mv];
        switch (mvn.kind) {
            .sequence => |first| {
                var e = first;
                while (e) |eid| : (e = self.src.nodes[eid].next_sibling) {
                    try self.mergeOneSource(map_id, last, seen, self.src.nodes[eid]);
                }
            },
            else => try self.mergeOneSource(map_id, last, seen, mvn),
        }
    }

    fn mergeOneSource(self: *Materializer, map_id: AST.Node.Id, last: *?AST.Node.Id, seen: *std.ArrayList([]const u8), source: AST.Node) Error!void {
        const target_id = try self.src.resolveDeep(source);
        try self.enter(target_id); // guards a self-referential merge
        defer self.leave();
        const target = self.src.nodes[target_id];
        if (target.kind != .mapping) return; // a non-mapping merge source is ignored

        var entry = target.kind.mapping;
        while (entry) |eid| : (entry = self.src.nodes[eid].next_sibling) {
            const kv = self.src.nodes[eid].kind.keyvalue;
            const key_str = self.stringKey(kv.key);
            if (key_str) |ks| {
                if (std.mem.eql(u8, ks, "<<")) {
                    try self.mergeInto(map_id, last, seen, kv.value); // nested merge in a source
                    continue;
                }
                if (contains(seen.items, ks)) continue; // shadowed by a local or earlier source
            }
            try self.appendEntry(map_id, last, seen, kv.key, kv.value);
        }
    }

    fn stringKey(self: *const Materializer, key: AST.Node.Id) ?[]const u8 {
        return switch (self.src.nodes[key].kind) {
            .string => |s| s,
            else => null,
        };
    }

    // ── tags ──────────────────────────────────────────────────────────────

    fn applyScalarTag(self: *const Materializer, node: AST.Node) Error!AST.Node.Kind {
        const name = self.coreName(self.src.tagOf(node.id) orelse return node.kind) orelse
            return self.customTag(node.id, node.kind);
        if (std.mem.eql(u8, name, "str")) return .{ .string = self.scalarText(node) };
        if (std.mem.eql(u8, name, "null")) return .null_;
        if (std.mem.eql(u8, name, "bool")) return self.asBool(node);
        if (std.mem.eql(u8, name, "int")) return self.asNumber(node, .integer);
        if (std.mem.eql(u8, name, "float")) return self.asNumber(node, .float);
        if (std.mem.eql(u8, name, "seq") or std.mem.eql(u8, name, "map")) return error.TagTypeMismatch;
        // a `!!`-secondary tag with an unrecognized core name is custom.
        return self.customTag(node.id, node.kind);
    }

    fn checkCollectionTag(self: *const Materializer, node: AST.Node, comptime want: enum { seq, map }) Error!void {
        const tag = self.src.tagOf(node.id) orelse return;
        if (self.coreName(tag)) |name| {
            const ok = if (want == .seq) "seq" else "map";
            if (std.mem.eql(u8, name, ok)) return;
            if (eqlAny(name, &.{ "seq", "map", "str", "int", "float", "bool", "null" }))
                return error.TagTypeMismatch;
        }
        _ = try self.customTag(node.id, node.kind); // strict-errors on unknown; lax no-op
    }

    /// Keep an applied core SCALAR tag on the output node as a normalized `.kind`
    /// type tag, so a tag-aware target re-surfaces it (fig `: int =`, canonical
    /// `!!int`). A custom/collection tag is not carried — it was already applied
    /// (collections) or errored/dropped (custom) by `applyScalarTag`.
    fn carryScalarTag(self: *Materializer, src_id: AST.Node.Id, new_id: AST.Node.Id) Error!void {
        const name = self.coreName(self.src.tagOf(src_id) orelse return) orelse return;
        const kind: AST.Tag.KindTag =
            if (std.mem.eql(u8, name, "str")) .string else if (std.mem.eql(u8, name, "int")) .integer else if (std.mem.eql(u8, name, "float")) .float else if (std.mem.eql(u8, name, "bool")) .boolean else if (std.mem.eql(u8, name, "null")) .null_ else return; // seq/map (collection) or unrecognized — nothing to carry
        self.out_tags.items[new_id] = .{ .kind = kind };
        self.any_tags = true;
    }

    /// The core-schema type name (`str`, `int`, …) a `Tag` denotes, or null when
    /// it is a genuinely custom tag. A `.kind` tag names its type directly; a
    /// `.text` tag is decoded from its YAML spelling via `coreTagName`.
    fn coreName(self: *const Materializer, tag: AST.Tag) ?[]const u8 {
        return switch (tag) {
            .text => |t| coreTagName(self.src.tag_directives, t),
            .kind => |k| switch (k) {
                .null_ => "null",
                .boolean => "bool",
                .string => "str",
                .integer => "int",
                .float => "float",
                .sequence => "seq",
                .mapping => "map",
            },
        };
    }

    /// A non-core tag on node `id`: a normalized `.kind` tag is always valid; a
    /// verbatim non-specific `!` is a no-op; any other custom tag is rejected in
    /// strict mode and dropped (node kept as parsed) in lax mode.
    fn customTag(self: *const Materializer, id: AST.Node.Id, kind: AST.Node.Kind) Error!AST.Node.Kind {
        switch (self.src.tagOf(id) orelse return kind) {
            .kind => return kind,
            .text => |t| if (std.mem.eql(u8, t, "!")) return kind,
        }
        if (self.mode == .strict) return error.UnknownTag;
        return kind;
    }

    /// A `!!int`/`!!float` payload, refused unless its text reads as that
    /// kind — every printer writes a number's text bare, so `!!int 1 - 3`
    /// would print as `1 - 3` in JSON. Read by YAML 1.2's core schema or
    /// 1.1's, since the AST does not say which the document was: `!!int
    /// 0x1A` and `!!int 1_000` both pass. A `!!float` also takes a decimal
    /// integer's text (`!!float 1`), as the core schema's float does.
    fn asNumber(self: *const Materializer, node: AST.Node, comptime want: @FieldType(AST.Node.Kind.Number, "kind")) Error!AST.Node.Kind {
        const t = self.scalarText(node);
        const ok = switch (want) {
            .integer => yaml_scalar.classifyNumber(t) == .integer or yaml_scalar.classify1_1Number(t) == .integer,
            .float => (yaml_scalar.classifyNumber(t) != .not_number and !hasRadixPrefix(t)) or
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

/// If `tag` names a YAML core-schema type, returns the bare name (`str`, `int`,
/// `bool`, `null`, `seq`, `map`, …). Accepts the `!!name` shorthand and the
/// verbose `tag:yaml.org,2002:name` form (with or without `!<>`).
fn coreTagName(directives: []const AST.TagDirective, tag: []const u8) ?[]const u8 {
    const core = "tag:yaml.org,2002:";
    if (tag.len >= 3 and tag[0] == '!' and tag[1] == '<' and tag[tag.len - 1] == '>') {
        const inner = tag[2 .. tag.len - 1];
        if (std.mem.startsWith(u8, inner, core)) return inner[core.len..];
        return null;
    }
    if (std.mem.startsWith(u8, tag, core)) return tag[core.len..];
    if (tag.len == 0 or tag[0] != '!') return null;
    // A shorthand `handle` + `suffix`: `!!int`, `!e!int`, or the primary
    // `!int`. A `%TAG` directive in the document can bind any handle,
    // `!!` and `!` included, to a prefix of its own; unbound, `!!` is the
    // core schema's and `!` a local tag's.
    const close = if (tag.len > 1) std.mem.indexOfScalarPos(u8, tag, 1, '!') else null;
    const handle = if (close) |c| tag[0 .. c + 1] else "!";
    const suffix = tag[handle.len..];
    const prefix = for (directives) |d| {
        if (std.mem.eql(u8, d.handle, handle)) break d.prefix;
    } else if (std.mem.eql(u8, handle, "!!")) core else return null;
    // `prefix ++ suffix` names a core type when it is `core ++ name`. A
    // prefix that runs past `core` into the name is not a spelling anyone
    // writes, and is read as custom.
    if (prefix.len > core.len or !std.mem.startsWith(u8, core, prefix)) return null;
    const rest = core[prefix.len..];
    if (!std.mem.startsWith(u8, suffix, rest)) return null;
    const name = suffix[rest.len..];
    return if (name.len == 0) null else name;
}

/// Whether `t` spells an integer by radix (`0x1A`, `0o17`, `0b101`), which a
/// `!!float` does not take.
fn hasRadixPrefix(t: []const u8) bool {
    const s = if (t.len > 0 and (t[0] == '+' or t[0] == '-')) t[1..] else t;
    return s.len > 2 and s[0] == '0' and std.mem.indexOfScalar(u8, "xXoObB", s[1]) != null;
}

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
// By path, like `conformance.zig`'s: these exercise the pass over what the
// compiled YAML parser builds, and skip when it is gated out of the build.
const Parser = @import("languages/yaml/parser.zig");

fn hasAlias(ast: AST) bool {
    for (ast.nodes) |n| if (n.kind == .alias) return true;
    return false;
}

test "materialize: alias expands to a copied subtree" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const doc = try Parser.parse(testing.allocator, "a: &x [1, 2]\nb: *x\n", .v1_2_2);
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
    const doc = try Parser.parse(testing.allocator, src, .v1_2_2);
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
    const doc = try Parser.parse(testing.allocator, "a: !!str 123\nb: !!int \"42\"\nc: !!null x\nd: 7\n", .v1_2_2);
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
    try testing.expect(mat.node_tags[a.id].?.kind == .string);
    try testing.expect(mat.node_tags[b.id].?.kind == .integer);
    // An untagged scalar carries no tag.
    try testing.expect((try mat.getValByPath(&.{.{ .key = "d" }})).id >= mat.node_tags.len or
        mat.node_tags[(try mat.getValByPath(&.{.{ .key = "d" }})).id] == null);
}

test "materialize: unknown tag strict errors, lax drops" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const doc = try Parser.parse(testing.allocator, "x: !custom 1\n", .v1_2_2);
    defer doc.deinit(testing.allocator);
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    try testing.expectError(error.UnknownTag, materialize(arena.allocator(), &doc.ast, .strict));
    const mat = try materialize(arena.allocator(), &doc.ast, .lax);
    try testing.expectEqualSlices(u8, "1", (try mat.getValByPath(&.{.{ .key = "x" }})).kind.number.raw);
}

test "materialize: collection-tag mismatch and cyclic alias error" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    {
        const doc = try Parser.parse(testing.allocator, "x: !!str [1, 2]\n", .v1_2_2);
        defer doc.deinit(testing.allocator);
        var arena = std.heap.ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        try testing.expectError(error.TagTypeMismatch, materialize(arena.allocator(), &doc.ast, .strict));
    }
    {
        const doc = try Parser.parse(testing.allocator, "- &a [*a]\n", .v1_2_2);
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
        const doc = try Parser.parse(testing.allocator, c.src, .v1_2_2);
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
        const doc = try Parser.parse(testing.allocator, "%TAG !! tag:example.com,2000:app/\n---\n!!int 1 - 3\n", .v1_2_2);
        defer doc.deinit(testing.allocator);
        var arena = std.heap.ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        try testing.expectError(error.UnknownTag, materialize(arena.allocator(), &doc.ast, .strict));
    }
    {
        // `!` bound to the core prefix makes `!int` the core int.
        const doc = try Parser.parse(testing.allocator, "%TAG ! tag:yaml.org,2002:\n---\nx: !int \"7\"\n", .v1_2_2);
        defer doc.deinit(testing.allocator);
        var arena = std.heap.ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        const mat = try materialize(arena.allocator(), &doc.ast, .strict);
        const x = try mat.getValByPath(&.{.{ .key = "x" }});
        try testing.expect(x.kind == .number and x.kind.number.kind == .integer);
    }
}
