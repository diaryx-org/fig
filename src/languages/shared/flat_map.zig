//! Shared "flat mapping" builder for line-oriented `key = value` formats —
//! INI, dotenv, `.properties`. Each of those parsers needs exactly the glue
//! TOML's parser (`languages/toml/parser.zig`) hand-rolls for itself
//! (`addNode`/`appendKeyValue`/`lookupChild`), minus everything TOML needs
//! that they don't: dotted keys, arrays, inline tables, typed scalars. This
//! module factors out the part that's actually identical — node allocation and
//! "does this mapping already have a child keyed X" — so the three new
//! parsers (and INI's one-level section nesting) share it instead of each
//! re-copying TOML's copy a fourth/fifth/sixth time.
//!
//! What's deliberately NOT here: comment capture (each parser's leading/
//! trailing rules differ enough — see each tokenizer's module doc — to stay
//! bespoke) and value decoding (escape sets conflict across the three
//! formats). Only the tree-shape plumbing is shared.

const std = @import("std");
const AST = @import("../../kit/ast/ast.zig");
const RowWriter = @import("../../kit/row_writer.zig");
const ChildIndex = @import("../../kit/child_index.zig");
const table = @import("../../kit/table.zig");
const Span = @import("../../kit/span.zig");

/// The rows of the table a parse is writing (`RowWriter`), the one
/// operation that adds a node, and an index of each mapping's entries by key
/// and its last entry, so that adding a key is constant time rather than a
/// walk of the mapping. A node's spans and side facts (comments,
/// NestedText's item markers and entry separators) go through `w`.
pub const NodeArena = struct {
    w: RowWriter,
    index: ChildIndex = .{},

    pub fn init(t: *table.Table) NodeArena {
        return .{ .w = .init(t) };
    }

    pub fn deinit(self: *NodeArena) void {
        self.index.deinit(self.w.allocator());
    }

    pub fn addNode(self: *NodeArena, kind: AST.Node.Kind, span: Span) std.mem.Allocator.Error!AST.Node.Id {
        return self.w.add(kind, span);
    }
};

/// The keyvalue child of `map_id` keyed `key`, or null.
pub fn lookupChild(arena: *const NodeArena, map_id: AST.Node.Id, key: []const u8) ?AST.Node.Id {
    return arena.index.get(arena.w, map_id, key);
}

/// How a repeated key is handled. Every INI/dotenv/`.properties` implementation
/// in the wild silently lets the later assignment win — unlike TOML, which
/// hard-errors — so `.overwrite` is what all three callers actually want;
/// `.err` is NestedText's, which refuses a repeat.
pub const DuplicatePolicy = enum { overwrite, err };
pub const DuplicateError = error{DuplicateKey};

/// Whether `putEntry` added a fresh entry or overwrote a repeat.
pub const PutResult = enum { added, overwrote };

/// What `putEntry` did, and the keyvalue node it added or overwrote.
pub const Put = struct { result: PutResult, kv: AST.Node.Id };

/// Add (or, on a repeat with `.overwrite`, update) a `key = value` entry in
/// mapping `map_id`. `key_id` must already be a `.string` node (each caller
/// adds it via its own `arena.addNode` so it can attach leading comments
/// first, same as TOML's `appendKeyValue`).
///
/// `.overwrite` keeps the FIRST-SEEN position but the LAST-ASSIGNED value —
/// repointing the existing keyvalue node's `value` in place — which is the one
/// substantive behavioral difference from TOML's `appendKeyValue` this module
/// exists to capture. The overwritten value's row, and the repeat's key
/// row, are left unlinked: `RowWriter.finish` drops them. The result says
/// whether this was a fresh entry or a repeat, so a caller can turn a repeat
/// into a `DuplicateKey` warning (real content, likely a mistake) without it
/// being a parse *error*.
pub fn putEntry(
    arena: *NodeArena,
    map_id: AST.Node.Id,
    key_id: AST.Node.Id,
    value_id: AST.Node.Id,
    policy: DuplicatePolicy,
) (DuplicateError || std.mem.Allocator.Error)!Put {
    const w = arena.w;
    // Read from the row; the buffer it may point into moves when the table
    // grows, so it is not kept past the lookup.
    if (arena.index.get(w, map_id, w.kindOf(key_id).string)) |existing_kv| {
        switch (policy) {
            .err => return error.DuplicateKey,
            .overwrite => {
                const old_key = w.kindOf(existing_kv).keyvalue.key;
                try w.set(existing_kv, .{ .keyvalue = .{ .key = old_key, .value = value_id } });
                return .{ .result = .overwrote, .kv = existing_kv };
            },
        }
    }
    const kv_id = try w.add(
        .{ .keyvalue = .{ .key = key_id, .value = value_id } },
        Span.init(w.spanOf(key_id).start, w.spanOf(value_id).end),
    );
    try arena.index.append(w.allocator(), w, map_id, kv_id);
    try arena.index.put(w.allocator(), w, kv_id);
    return .{ .result = .added, .kv = kv_id };
}

test "putEntry appends in order and reports fresh vs. repeat" {
    const a = std.testing.allocator;
    const src = "a=1\nb=2\na=3\n";
    var t = table.Table.init(a, src);
    defer t.deinit();
    var arena: NodeArena = .init(&t);
    defer arena.deinit();
    const w = arena.w;

    const root = try arena.addNode(.{ .mapping = null }, Span.init(0, 0));
    const k1 = try arena.addNode(.{ .string = "a" }, Span.init(0, 1));
    const v1 = try arena.addNode(.{ .string = "1" }, Span.init(2, 3));
    try std.testing.expectEqual(PutResult.added, (try putEntry(&arena, root, k1, v1, .overwrite)).result);

    const k2 = try arena.addNode(.{ .string = "b" }, Span.init(4, 5));
    const v2 = try arena.addNode(.{ .string = "2" }, Span.init(6, 7));
    try std.testing.expectEqual(PutResult.added, (try putEntry(&arena, root, k2, v2, .overwrite)).result);

    // Repeat of "a": overwrites in place, first-seen position preserved.
    const k3 = try arena.addNode(.{ .string = "a" }, Span.init(8, 9));
    const v3 = try arena.addNode(.{ .string = "3" }, Span.init(10, 11));
    const put = try putEntry(&arena, root, k3, v3, .overwrite);
    try std.testing.expectEqual(PutResult.overwrote, put.result);

    const first = w.kindOf(root).mapping.?;
    try std.testing.expectEqual(first, put.kv);
    try std.testing.expectEqual(k1, w.kindOf(first).keyvalue.key);
    try std.testing.expectEqual(v3, w.kindOf(first).keyvalue.value); // last value wins
    const second = w.node(first).next_sibling.?;
    try std.testing.expectEqual(k2, w.kindOf(second).keyvalue.key);
    try std.testing.expectEqual(null, w.node(second).next_sibling); // no 3rd sibling appended
    try std.testing.expectEqual(second, lookupChild(&arena, root, "b").?);
    try std.testing.expectEqual(null, lookupChild(&arena, root, "c"));

    // The overwritten value and the repeat's key are unreachable: finished,
    // the table holds the root, two entries and their keys and values.
    try w.finish(root);
    var ast = try AST.adopt(a, src, &t, .required);
    defer ast.deinit();
    try std.testing.expectEqual(@as(usize, 7), ast.nodeCount());
    try std.testing.expectEqualStrings("3", (try ast.getValByPath(&.{.{ .key = "a" }})).kind.string);
}

test "putEntry .err rejects a repeat" {
    const a = std.testing.allocator;
    const src = "a=1\nb=2\na=3\n";
    var t = table.Table.init(a, src);
    defer t.deinit();
    var arena: NodeArena = .init(&t);
    defer arena.deinit();
    const root = try arena.addNode(.{ .mapping = null }, Span.init(0, 0));
    const k1 = try arena.addNode(.{ .string = "a" }, Span.init(0, 1));
    const v1 = try arena.addNode(.{ .string = "1" }, Span.init(2, 3));
    _ = try putEntry(&arena, root, k1, v1, .err);
    const k2 = try arena.addNode(.{ .string = "a" }, Span.init(4, 5));
    const v2 = try arena.addNode(.{ .string = "2" }, Span.init(6, 7));
    try std.testing.expectError(error.DuplicateKey, putEntry(&arena, root, k2, v2, .err));
}
