//! Structure-aware patching: merge one document into another, in place.
//!
//! A patch is an ordinary document. `apply` walks it against the target and
//! emits the editor's existing span-splice ops — `set`, `appendToSeq`, the
//! comment ops — one per node it actually has to change. It is a PLANNER over
//! `Editor`, not a second editing engine: every byte it writes is written by
//! the same primitives `fig set` uses, so every guarantee they carry (atomic
//! reparse, ancestor auto-vivification, the block-into-flow refusal) carries
//! here unchanged.
//!
//! What that buys, and what it costs:
//!
//!   * Bytes outside the paths the patch names are **identical**. That is the
//!     whole point, and it is why this walks and splices rather than
//!     parse-merge-reprint.
//!   * Bytes inside them are rendered in the **target's** format and style,
//!     not the patch's. Two documents' formatting cannot both survive into one
//!     file; the target's wins, because the target is the file being kept.
//!     For a scalar that is the whole story. For a CONTAINER the patch
//!     changes, "the target's style" means the target format's PRINTER — the
//!     AST records no flow-vs-block memory, so a YAML `[80, 443]` the patch
//!     rewrites comes back as a block sequence. The value is right and the
//!     spelling is the printer's; a container the patch leaves alone keeps its
//!     bytes, flow spelling included.
//!   * A value the patch and the target already agree on is not spliced at all
//!     (`nodesEqual`), so re-applying a patch is a no-op on disk rather than a
//!     re-render of every value it mentions. This is load-bearing, not an
//!     optimization: without it a patch that changes one key would rewrite
//!     every other key it names into fig's spelling of it.
//!
//! The merge rule, per node, is:
//!
//!   | patch    | target             | result                                |
//!   |----------|--------------------|---------------------------------------|
//!   | anything | absent             | `set` the whole subtree (one splice)  |
//!   | mapping  | mapping or null    | recurse, entry by entry               |
//!   | sequence | sequence           | `Options.seq` decides                 |
//!   | anything | anything else      | `set` (the patch wins)                |
//!
//! "null" is in the mapping row because a bare `key:` (and an empty document's
//! root) is a container waiting to exist, which the editor's `set` promotes —
//! the same distinction `Editor.blockedByEmptyNode` draws for its own vivify.
//!
//! What this deliberately refuses rather than guesses at:
//!
//!   * Patching THROUGH a YAML alias (`error.PatchThroughAlias`). A `*name`
//!     target is a reference to a node defined elsewhere; writing "into" it
//!     either edits every other user of that anchor or silently shadows it,
//!     and there is no way to tell from here which the caller meant. The
//!     aliased key case is different and is NOT refused: the editor already
//!     has a policy for a key supplied by a `<<` merge (shadow it with a local
//!     entry), and that policy is a good one.
//!   * A non-mapping patch against the document ROOT
//!     (`error.PatchRootNotMergeable`) — replacing a whole document is a copy,
//!     not a patch.
//!   * A non-string mapping key (`error.NonStringPatchKey`). Path navigation
//!     is string-keyed, so there is nowhere to address such an entry.

const std = @import("std");
const AST = @import("ast/ast.zig");
const Document = @import("document.zig");
const editor_mod = @import("editor.zig");

const Id = AST.Node.Id;

/// What to do when the patch and the target both hold a sequence at the same
/// path. There is no defensible default beyond `replace`: a sequence is
/// sometimes a set (union), sometimes a log (append), and sometimes just a
/// value (replace), and only the caller knows which.
pub const SeqStrategy = enum {
    /// Take the patch's sequence whole. The default.
    replace,
    /// Append every item of the patch's sequence to the target's.
    append,
    /// Append only the items the target does not already hold — compared
    /// structurally (`nodesEqual`), not by source text, so `1` and `1` match
    /// across formats while `1` and `1.0` do not.
    unite,
};

/// Whose comments win where both documents carry one on the same entry. Note
/// that comments NESTED inside a subtree the patch contributes whole always
/// ride along with it (they are part of what was rendered); this decides only
/// the entry-level conflict, plus whether patch comments are carried at all.
pub const CommentStrategy = enum {
    /// Keep the target's comment; contribute the patch's only where the target
    /// has none. The default — same principle as the rest of this module,
    /// which is that the target is the document being kept.
    ours,
    /// The patch's comment replaces the target's wherever the patch has one.
    theirs,
    /// Carry no comment from the patch at all, at any depth. The target's own
    /// comments are still untouched.
    none,
};

pub const Options = struct {
    seq: SeqStrategy = .replace,
    comments: CommentStrategy = .ours,
    /// Style knobs for rendering a patch subtree into the target's syntax —
    /// the same options `fig get`/`fmt` expose, honored by whichever formats
    /// honor them there. `strip_comments` is forced on under
    /// `CommentStrategy.none`.
    serialize: AST.SerializeOptions = .{},
};

/// What `apply` did, for the caller to report. `changed` is the question a CLI
/// actually asks; the individual counts are for a summary line.
pub const Stats = struct {
    /// Values that existed and were rewritten.
    replaced: usize = 0,
    /// Keys (or whole subtrees) that did not exist and were created.
    added: usize = 0,
    /// Sequence items appended under `SeqStrategy.append`/`.unite`.
    appended: usize = 0,
    /// Paths removed by `apply`'s `deletes`.
    deleted: usize = 0,
    /// Values the patch names that the target already agreed on, so nothing
    /// was spliced.
    unchanged: usize = 0,
    /// Comments the patch carried that the TARGET FORMAT cannot hold (strict
    /// JSON), or that its syntax cannot hold in that position (a multi-line
    /// trailing comment). Dropped rather than made a hard error: losing trivia
    /// should not fail an otherwise-good patch, but it should be reportable.
    comments_dropped: usize = 0,

    pub fn changed(self: Stats) bool {
        return self.replaced + self.added + self.appended + self.deleted > 0;
    }
};

/// One path to remove from the target after the merge. The final segment
/// decides which removal it is, exactly as `fig delete` does: a key deletes
/// that mapping entry (with the comments it owns), an index removes that item
/// from the parent sequence.
pub const Deletion = []const AST.PathSegment;

/// Merge `patch`'s subtree at `from` into the document `editor` holds, landing
/// it at `at` (the document root when empty), then apply `deletes`.
///
/// `target_format` is what patch subtrees are RENDERED as before being
/// spliced. It must be the format `editor` is editing — a mismatch produces
/// text that either fails to reparse (caught by the editor, which rolls back)
/// or, worse, reparses as something else.
///
/// `patch` must not carry unresolved YAML aliases: materialize it first
/// (`Language.YAML.materialize`). An alias in a patch refers to an anchor in
/// the PATCH file, which the target has never seen, so splicing one verbatim
/// would produce a dangling reference.
pub fn apply(
    comptime Language: type,
    editor: *editor_mod.Editor(Language),
    target_format: AST.SerializeFormat,
    at: []const AST.PathSegment,
    patch: *const AST,
    from: Id,
    deletes: []const Deletion,
    options: Options,
) !Stats {
    var w: Walker(Language) = .{
        .editor = editor,
        .allocator = editor.allocator,
        .target_format = target_format,
        .options = options,
    };
    defer w.path.deinit(w.allocator);
    try w.path.appendSlice(w.allocator, at);
    try w.mergeValue(patch, from);

    // Deletions run last, so a patch can both write a key and remove a
    // different one without their order mattering to the caller.
    for (deletes) |path| try w.delete(path);
    return w.stats;
}

fn Walker(comptime Language: type) type {
    return struct {
        const Self = @This();
        const Ed = editor_mod.Editor(Language);

        editor: *Ed,
        allocator: std.mem.Allocator,
        target_format: AST.SerializeFormat,
        options: Options,
        /// The target path currently being merged into. Grows and shrinks as
        /// `mergeMapping` descends; its backing memory is this walker's, while
        /// the key slices inside it borrow from the patch AST (stable for the
        /// whole walk — the patch is never edited).
        path: std.ArrayList(AST.PathSegment) = .empty,
        stats: Stats = .{},

        /// The target's value at the current path, or null when the path does
        /// not resolve. Never held across a splice: the `Document` it comes
        /// from is freed and rebuilt by every `replaceAtSpan`.
        fn targetValue(self: *Self) !?AST.Node {
            const parsed = try self.editor.getParsed();
            return parsed.ast.getValByPath(self.path.items) catch null;
        }

        fn mergeValue(self: *Self, patch: *const AST, id: Id) !void {
            const kind = patch.nodes[id].kind;
            const target = try self.targetValue();

            // A `*name` in the TARGET is a reference to a node defined
            // elsewhere in it; see this module's header for why that is a
            // refusal rather than a policy.
            if (target) |t| if (t.kind == .alias) return error.PatchThroughAlias;

            switch (kind) {
                .mapping => if (target) |t| switch (t.kind) {
                    // A null target is a container waiting to exist, which
                    // `set` promotes on the way down — so descend into it
                    // rather than clobbering it with a rendered `{}`.
                    .mapping, .null_ => return self.mergeMapping(patch, id),
                    else => {},
                },
                .sequence => if (self.options.seq != .replace) {
                    if (target) |t| if (t.kind == .sequence) return self.mergeSequence(patch, id);
                },
                else => {},
            }
            try self.setValue(patch, id, target);
        }

        /// `mergeValue` and this are mutually recursive, so one of the two has
        /// to state its error set outright — two inferred sets that depend on
        /// each other are a comptime dependency loop. There is no smaller
        /// honest set to name than `anyerror`: every editor op called from
        /// here carries an inferred set of its own, whose members vary with
        /// `Language`.
        fn mergeMapping(self: *Self, patch: *const AST, id: Id) anyerror!void {
            var next: ?Id = patch.nodes[id].kind.mapping;
            while (next) |entry_id| {
                const entry = patch.nodes[entry_id];
                next = entry.next_sibling;
                const kv = switch (entry.kind) {
                    .keyvalue => |kv| kv,
                    // A mapping whose children are not entries is not a
                    // document any reader in this library produces.
                    else => return error.InvalidPatchDocument,
                };
                const key = switch (patch.nodes[kv.key].kind) {
                    .string => |s| s,
                    else => return error.NonStringPatchKey,
                };

                try self.path.append(self.allocator, .{ .key = key });
                defer _ = self.path.pop();
                try self.mergeValue(patch, kv.value);
                try self.applyComments(patch, entry_id);
            }
        }

        fn mergeSequence(self: *Self, patch: *const AST, id: Id) !void {
            var next: ?Id = patch.nodes[id].kind.sequence;
            while (next) |item_id| {
                next = patch.nodes[item_id].next_sibling;
                if (self.options.seq == .unite and try self.targetHasItem(patch, item_id)) {
                    self.stats.unchanged += 1;
                    continue;
                }
                const text = try self.render(patch, item_id);
                defer self.allocator.free(text);
                try self.editor.appendToSeq(self.path.items, text);
                self.stats.appended += 1;
            }
        }

        /// Whether the target's sequence at the current path already holds an
        /// item structurally equal to `item_id`. Re-read on every call rather
        /// than once: each append reparses, and comparing against the sequence
        /// as it now stands is what makes `unite` dedupe WITHIN the patch too.
        fn targetHasItem(self: *Self, patch: *const AST, item_id: Id) !bool {
            const parsed = try self.editor.getParsed();
            const seq = parsed.ast.getValByPath(self.path.items) catch return false;
            if (seq.kind != .sequence) return false;
            var next = seq.kind.sequence;
            while (next) |id| {
                if (nodesEqual(patch, item_id, &parsed.ast, id)) return true;
                next = parsed.ast.nodes[id].next_sibling;
            }
            return false;
        }

        fn setValue(self: *Self, patch: *const AST, id: Id, target: ?AST.Node) !void {
            // Already agreed on: splice nothing. See the header — this is what
            // keeps a patch's diff the size of what it actually changes.
            if (target) |t| {
                const parsed = try self.editor.getParsed();
                if (nodesEqual(patch, id, &parsed.ast, t.id)) {
                    self.stats.unchanged += 1;
                    return;
                }
            }
            if (self.path.items.len == 0) return error.PatchRootNotMergeable;

            const text = try self.render(patch, id);
            defer self.allocator.free(text);

            // `set` upserts and vivifies missing ancestors, but it addresses a
            // mapping key. An indexed tail names an existing item instead —
            // there is nothing to create — so it goes through the plain value
            // replacement.
            switch (self.path.items[self.path.items.len - 1]) {
                .key => try self.editor.set(self.path.items, text),
                .index => try self.editor.replaceValAtPath(self.path.items, text),
            }
            if (target == null) self.stats.added += 1 else self.stats.replaced += 1;
        }

        /// Render the patch subtree at `id` as a value fragment in the target's
        /// format. The AST is shallow-copied and re-rooted rather than rebuilt:
        /// node ids are self-referential, so a copy pointing at a different
        /// root IS the subtree, at no allocation.
        fn render(self: *Self, patch: *const AST, id: Id) ![]u8 {
            var view = patch.*;
            view.root = id;

            var options = self.options.serialize;
            if (self.options.comments == .none) options.strip_comments = true;
            // fig's block spellings (`* ` items, section headers) only parse as
            // standalone lines, so a fragment spliced after `key = ` has to be
            // flow — the same reason the C ABI's value serializer sets this.
            if (self.target_format == .fig) options.flow = true;
            // What the editor takes, not a document: plist's bare element
            // rather than a wrapped `<plist>`, a NestedText scalar's plain
            // text rather than a `>` block.
            options.splice = true;

            var w = std.Io.Writer.Allocating.init(self.allocator);
            defer w.deinit();
            try view.serializeFragmentWith(&w.writer, self.target_format, options);
            // Every printer terminates a DOCUMENT with a newline. What the
            // editor takes is a VALUE — the same text a `fig set` argument
            // supplies, which never carries one — and it splices what it is
            // given: leave the newline on and an appended flow item lands
            // before the `]` on a line of its own.
            return self.allocator.dupe(u8, std.mem.trimEnd(u8, w.written(), "\n"));
        }

        /// Carry the patch entry's own leading/trailing comments onto the node
        /// just merged, as `Options.comments` directs. Trivia: a format that
        /// cannot hold the comment counts it dropped rather than failing the
        /// patch.
        fn applyComments(self: *Self, patch: *const AST, entry_id: Id) !void {
            if (self.options.comments == .none) return;
            const leading = patch.comments(patch.leadingCommentAnchor(entry_id)).leading;
            const trailing = patch.comments(patch.trailingCommentAnchor(entry_id)).trailing;
            if (leading.len == 0 and trailing == null) return;

            if (leading.len > 0) try self.carryLeading(leading);
            if (trailing) |c| try self.carryTrailing(c);
        }

        // The comment ops' errors are COMPARED below rather than switched on.
        // Each op's error set is inferred per language, and a format that
        // hooks the op (plist, whose `<!-- -->` has no line marker) has a set
        // without `CommentsUnsupported` in it — a `switch` arm naming an error
        // outside the set is a compile error, which only the everything-on
        // build ever saw. `==` against an error not in the set is fine.

        fn carryLeading(self: *Self, leading: []const AST.Comment) !void {
            const existing = self.editor.getLeadingComment(self.path.items) catch |err| {
                // No comment syntax in this format, or the entry isn't there
                // to comment on (an empty mapping the merge contributed
                // nothing for).
                if (err == error.CommentsUnsupported) return self.dropComment();
                if (err == error.NotFound) return;
                return err;
            };
            defer if (existing) |e| self.allocator.free(e);
            if (self.options.comments == .ours and existing != null) return;

            var text: std.ArrayList(u8) = .empty;
            defer text.deinit(self.allocator);
            for (leading, 0..) |c, i| {
                if (i > 0) try text.append(self.allocator, '\n');
                try text.appendSlice(self.allocator, c.text);
            }
            if (existing != null) try self.editor.deleteLeadingComments(self.path.items);
            self.editor.addLeadingComment(self.path.items, text.items) catch |err| {
                // The entry landed inside a one-line flow collection, where a
                // comment has no line of its own to sit on (and would be
                // discarded on the next parse anyway). Trivia: count it
                // dropped, like a format with no comment syntax at all.
                if (err == error.CommentsUnanchored) return self.dropComment();
                return err;
            };
        }

        fn carryTrailing(self: *Self, comment: AST.Comment) !void {
            // A same-line comment is one line by definition; a block comment
            // that spans several has no trailing spelling to downgrade to.
            if (std.mem.indexOfScalar(u8, comment.text, '\n') != null) return self.dropComment();
            const existing = self.editor.getTrailingComment(self.path.items) catch |err| {
                if (err == error.CommentsUnsupported) return self.dropComment();
                if (err == error.NotFound) return;
                return err;
            };
            defer if (existing) |e| self.allocator.free(e);
            if (self.options.comments == .ours and existing != null) return;
            self.editor.setTrailingComment(self.path.items, comment.text) catch |err| {
                if (err == error.CommentsUnsupported or err == error.MultilineComment or err == error.CommentsUnanchored) return self.dropComment();
                return err;
            };
        }

        fn dropComment(self: *Self) void {
            self.stats.comments_dropped += 1;
        }

        /// Remove one path from the target, keyed on its final segment exactly
        /// as `fig delete` is. A path that isn't there is not an error: a
        /// patch that removes a key twice, or removes one an earlier run
        /// already removed, has still arrived at what it asked for.
        fn delete(self: *Self, path: []const AST.PathSegment) !void {
            if (path.len == 0) return error.PatchRootNotMergeable;
            const result = switch (path[path.len - 1]) {
                .key => self.editor.deleteKey(path),
                .index => |i| self.editor.removeSeqItem(path[0 .. path.len - 1], i),
            };
            result catch |err| switch (err) {
                error.NotFound => return,
                else => return err,
            };
            self.stats.deleted += 1;
        }
    };
}

/// Structural equality of two subtrees, possibly from different documents in
/// different formats. Comments, tags and anchors are excluded — they are
/// trivia and side-tables, not the value — so this answers exactly the
/// question the merge asks: "would splicing this change what the document
/// means?"
///
/// Numbers compare by their source lexeme, not by numeric value: `1` and `1.0`
/// are different bytes, so splicing one over the other IS a change, and
/// reporting them equal would silently drop an edit the caller asked for.
pub fn nodesEqual(a: *const AST, a_id: Id, b: *const AST, b_id: Id) bool {
    const x = a.nodes[a_id].kind;
    const y = b.nodes[b_id].kind;
    if (std.meta.activeTag(x) != std.meta.activeTag(y)) return false;
    return switch (x) {
        .null_ => true,
        .boolean => |v| v == y.boolean,
        .string => |v| std.mem.eql(u8, v, y.string),
        .number => |v| v.eql(y.number),
        .extended => |v| v.eql(y.extended),
        .alias => |v| std.mem.eql(u8, v, y.alias),
        .keyvalue => |kv| nodesEqual(a, kv.key, b, y.keyvalue.key) and
            nodesEqual(a, kv.value, b, y.keyvalue.value),
        .sequence => |first| childrenEqual(a, first, b, y.sequence),
        .mapping => |first| childrenEqual(a, first, b, y.mapping),
    };
}

/// Walk two containers' sibling chains in step. Order-sensitive, for both
/// kinds: a reordered sequence is a different value, and a reordered mapping
/// is a different set of BYTES — and since the merge recurses into a mapping
/// it finds on both sides, this only ever compares mappings it is about to
/// splice whole anyway.
fn childrenEqual(a: *const AST, a_first: ?Id, b: *const AST, b_first: ?Id) bool {
    var p = a_first;
    var q = b_first;
    while (p != null and q != null) {
        if (!nodesEqual(a, p.?, b, q.?)) return false;
        p = a.nodes[p.?].next_sibling;
        q = b.nodes[q.?].next_sibling;
    }
    return p == null and q == null;
}

// ── Tests ───────────────────────────────────────────────────────────────────

const build_options = @import("build_options");

/// Parse `source` as `Lang`, merge `patch_source` into it, and return the
/// edited bytes. The shape every test below wants: two documents in, one
/// document out.
fn patchForTest(
    comptime Lang: type,
    comptime PatchLang: type,
    allocator: std.mem.Allocator,
    target_format: AST.SerializeFormat,
    source: []const u8,
    patch_source: []const u8,
    options: Options,
) ![]u8 {
    var patch_parser: PatchLang.Parser = .{ .allocator = allocator };
    var patch_doc = try PatchLang.parse(&patch_parser, patch_source, PatchLang.default_type);
    defer patch_doc.deinit(allocator);

    var editor: editor_mod.Editor(Lang) = .{ .allocator = allocator, .format = Lang.default_type };
    try editor.init(source);
    defer editor.deinit();

    _ = try apply(Lang, &editor, target_format, &.{}, &patch_doc.ast, patch_doc.ast.root, &.{}, options);
    return allocator.dupe(u8, editor.source.items);
}

test "a scalar patch touches only the keys it names" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = @import("languages/yaml/yaml.zig").Language;

    const out = try patchForTest(Y, Y, t.allocator, .yaml,
        \\name: api          # must match the DNS record
        \\replicas: 2
        \\ports: [80, 443]
        \\
    ,
        \\replicas: 5
        \\
    , .{});
    defer t.allocator.free(out);
    // `name`'s trailing comment and `ports`' flow style are untouched: they
    // were never spliced, because the patch never named them.
    try t.expectEqualStrings(
        \\name: api          # must match the DNS record
        \\replicas: 5
        \\ports: [80, 443]
        \\
    , out);
}

test "a patch adds absent keys and recurses into shared mappings" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = @import("languages/yaml/yaml.zig").Language;

    const out = try patchForTest(Y, Y, t.allocator, .yaml,
        \\service:
        \\  name: api
        \\  replicas: 2
        \\
    ,
        \\service:
        \\  replicas: 5
        \\  region: us-west
        \\
    , .{});
    defer t.allocator.free(out);
    try t.expectEqualStrings(
        \\service:
        \\  name: api
        \\  replicas: 5
        \\  region: us-west
        \\
    , out);
}

test "a value both sides already agree on is not spliced" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = @import("languages/yaml/yaml.zig").Language;

    var patch_parser: Y.Parser = .{ .allocator = t.allocator };
    var patch_doc = try Y.parse(&patch_parser, "name: 'api'\nreplicas: 5\n", Y.default_type);
    defer patch_doc.deinit(t.allocator);

    var editor: editor_mod.Editor(Y) = .{ .allocator = t.allocator, .format = Y.default_type };
    // The target spells the same string unquoted. Re-rendering it would be a
    // spurious diff, so an equal value is skipped entirely.
    try editor.init("name: api\nreplicas: 2\n");
    defer editor.deinit();

    const stats = try apply(Y, &editor, .yaml, &.{}, &patch_doc.ast, patch_doc.ast.root, &.{}, .{});
    try t.expectEqualStrings("name: api\nreplicas: 5\n", editor.source.items);
    try t.expectEqual(@as(usize, 1), stats.replaced);
    try t.expectEqual(@as(usize, 1), stats.unchanged);
    try t.expect(stats.changed());
}

test "sequence strategies: replace, append, unite" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = @import("languages/yaml/yaml.zig").Language;
    const target = "tags:\n- a\n- b\n";

    {
        // A CONTAINER the patch changes is re-rendered by the target format's
        // printer, which for YAML means block style regardless of how the
        // patch spelled it — the AST carries no flow-vs-block memory. Scalars
        // and untouched values keep their bytes; a contributed container gets
        // the printer's house style. See this module's header.
        const out = try patchForTest(Y, Y, t.allocator, .yaml, target, "tags: [b, c]\n", .{ .seq = .replace });
        defer t.allocator.free(out);
        try t.expectEqualStrings("tags:\n- b\n- c\n", out);
    }
    {
        const out = try patchForTest(Y, Y, t.allocator, .yaml, target, "tags: [b, c]\n", .{ .seq = .append });
        defer t.allocator.free(out);
        try t.expectEqualStrings("tags:\n- a\n- b\n- b\n- c\n", out);
    }
    {
        const out = try patchForTest(Y, Y, t.allocator, .yaml, target, "tags: [b, c]\n", .{ .seq = .unite });
        defer t.allocator.free(out);
        try t.expectEqualStrings("tags:\n- a\n- b\n- c\n", out);
    }
}

test "an appended item lands inside a flow sequence, on its line" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = @import("languages/yaml/yaml.zig").Language;

    // The append path splices a VALUE, not a document, so the render's
    // trailing newline has to come off first — with it on, the new item lands
    // before the `]` but a line down, turning a one-line diff into two.
    const out = try patchForTest(Y, Y, t.allocator, .yaml, "tags: [notes, zig]\n", "tags: [systems]\n", .{ .seq = .append });
    defer t.allocator.free(out);
    try t.expectEqualStrings("tags: [notes, zig, systems]\n", out);
}

test "comment strategies decide the entry-level conflict" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = @import("languages/yaml/yaml.zig").Language;
    const target = "replicas: 2 # bumped for Black Friday\n";
    const patch = "# how many api pods\nreplicas: 5 # from the overlay\nregion: us-west # new\n";

    {
        // `ours` decides each POSITION on its own: the target's trailing
        // comment on `replicas` stands, but it has no leading one, so the
        // patch's lands there uncontested — as does everything on the new key.
        const out = try patchForTest(Y, Y, t.allocator, .yaml, target, patch, .{ .comments = .ours });
        defer t.allocator.free(out);
        try t.expectEqualStrings("# how many api pods\nreplicas: 5 # bumped for Black Friday\nregion: us-west # new\n", out);
    }
    {
        const out = try patchForTest(Y, Y, t.allocator, .yaml, target, patch, .{ .comments = .theirs });
        defer t.allocator.free(out);
        try t.expectEqualStrings("# how many api pods\nreplicas: 5 # from the overlay\nregion: us-west # new\n", out);
    }
    {
        const out = try patchForTest(Y, Y, t.allocator, .yaml, target, patch, .{ .comments = .none });
        defer t.allocator.free(out);
        try t.expectEqualStrings("replicas: 5 # bumped for Black Friday\nregion: us-west\n", out);
    }
}

test "a patch crosses formats, rendering in the target's syntax" {
    if (comptime !(build_options.lang_yaml and build_options.lang_toml)) return error.SkipZigTest;
    const t = std.testing;
    const Y = @import("languages/yaml/yaml.zig").Language;
    const T = @import("languages/toml/toml.zig").Language;

    // A TOML patch into a YAML target: the value arrives, spelled as YAML.
    const out = try patchForTest(Y, T, t.allocator, .yaml,
        \\service:
        \\  name: api
        \\
    ,
        \\[service]
        \\replicas = 5
        \\ports = [80, 443]
        \\
    , .{});
    defer t.allocator.free(out);
    try t.expectEqualStrings(
        \\service:
        \\  name: api
        \\  replicas: 5
        \\  ports:
        \\  - 80
        \\  - 443
        \\
    , out);
}

test "patching through a YAML alias is refused, not guessed at" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = @import("languages/yaml/yaml.zig").Language;

    var patch_parser: Y.Parser = .{ .allocator = t.allocator };
    var patch_doc = try Y.parse(&patch_parser, "worker:\n  replicas: 4\n", Y.default_type);
    defer patch_doc.deinit(t.allocator);

    var editor: editor_mod.Editor(Y) = .{ .allocator = t.allocator, .format = Y.default_type };
    try editor.init("defaults: &d\n  replicas: 1\nworker: *d\n");
    defer editor.deinit();

    try t.expectError(error.PatchThroughAlias, apply(Y, &editor, .yaml, &.{}, &patch_doc.ast, patch_doc.ast.root, &.{}, .{}));
    // Refused means refused: nothing was written on the way to finding out.
    try t.expectEqualStrings("defaults: &d\n  replicas: 1\nworker: *d\n", editor.source.items);
}

test "deletions run after the merge and tolerate an absent path" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = @import("languages/yaml/yaml.zig").Language;

    var patch_parser: Y.Parser = .{ .allocator = t.allocator };
    var patch_doc = try Y.parse(&patch_parser, "b: 9\n", Y.default_type);
    defer patch_doc.deinit(t.allocator);

    var editor: editor_mod.Editor(Y) = .{ .allocator = t.allocator, .format = Y.default_type };
    try editor.init("a: 1\nb: 2\nc: 3\n");
    defer editor.deinit();

    const gone = [_]AST.PathSegment{.{ .key = "a" }};
    const missing = [_]AST.PathSegment{.{ .key = "nope" }};
    const deletes = [_]Deletion{ &gone, &missing };
    const stats = try apply(Y, &editor, .yaml, &.{}, &patch_doc.ast, patch_doc.ast.root, &deletes, .{});
    try t.expectEqualStrings("b: 9\nc: 3\n", editor.source.items);
    try t.expectEqual(@as(usize, 1), stats.deleted);
}

test "a patch merges into an empty document" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = @import("languages/yaml/yaml.zig").Language;

    const out = try patchForTest(Y, Y, t.allocator, .yaml, "", "a: 1\nb:\n  c: 2\n", .{});
    defer t.allocator.free(out);
    try t.expectEqualStrings("a: 1\nb:\n  c: 2\n", out);
}

test "nodesEqual compares structure, not spelling or trivia" {
    if (comptime !(build_options.lang_yaml and build_options.lang_json)) return error.SkipZigTest;
    const t = std.testing;
    const Y = @import("languages/yaml/yaml.zig").Language;
    const J = @import("languages/json/json.zig").Language;

    var yp: Y.Parser = .{ .allocator = t.allocator };
    var ydoc = try Y.parse(&yp, "a: [1, 'x'] # a comment\n", Y.default_type);
    defer ydoc.deinit(t.allocator);

    var jp: J.Parser = .{ .allocator = t.allocator };
    var jdoc = try J.parse(&jp, "{\"a\": [1, \"x\"]}", J.default_type);
    defer jdoc.deinit(t.allocator);

    try t.expect(nodesEqual(&ydoc.ast, ydoc.ast.root, &jdoc.ast, jdoc.ast.root));

    // A different lexeme for the same number is a different value here, since
    // splicing it would change the file.
    var jp2: J.Parser = .{ .allocator = t.allocator };
    var jdoc2 = try J.parse(&jp2, "{\"a\": [1.0, \"x\"]}", J.default_type);
    defer jdoc2.deinit(t.allocator);
    try t.expect(!nodesEqual(&ydoc.ast, ydoc.ast.root, &jdoc2.ast, jdoc2.ast.root));
}
