//! The CLI's half of `fig patch`: the per-format dispatch that turns a target
//! `Format` into "which language module, which dialect, and which serialize
//! format do patch subtrees render as", and the slice-in/slice-out wrapper the
//! `patch` action drives.
//!
//! The merge itself is `fig.Patch` — format-agnostic, and part of the library
//! rather than the binary, because it is the editor-level primitive a binding
//! would want too. What lives here is only the routing, mirroring
//! `edit_ops.route`: the same `caps.edit` gate, the same canonical/gron
//! refusals, the same registry-derived dialect.
//!
//! Slice in, slice out (never a file handle) so the one function serves all
//! four of the action's modes — write in place, `--dry-run`, `--diff`, and the
//! embedded-region path, which patches only the bytes between a host
//! document's fences and splices the result back.
const std = @import("std");
const fig = @import("fig");

const types = @import("types.zig");

const Format = types.Format;

/// One patch, resolved: what to merge, where it lands, and what to remove
/// afterwards. `patch` is borrowed and never edited — the merge only reads it.
pub const Request = struct {
    /// Where in the TARGET the patch lands. Empty is the document root.
    at: []const fig.AST.PathSegment = &.{},
    /// The patch document. Must carry no unresolved YAML aliases; the caller
    /// materializes (see `fig.Patch.apply`).
    patch: *const fig.AST,
    /// Which subtree of `patch` to merge — its root unless `--from` narrowed it.
    from: fig.AST.Node.Id,
    /// Paths to remove from the target after the merge (`--delete`).
    deletes: []const fig.Patch.Deletion = &.{},
    options: fig.Patch.Options = .{},
};

pub const Result = struct {
    /// The patched document. Owned by the caller's allocator.
    content: []u8,
    stats: fig.Patch.Stats,
};

/// Patch `content` — a whole document in `format` — and return the new bytes.
///
/// The three per-format facts are the registry's, exactly as in
/// `edit_ops.route`: whether the format is editable at all (`caps.edit`),
/// which dialect to reparse each splice under, and — the one this dispatch
/// adds — which `SerializeFormat` a patch subtree renders as on the way in.
/// That last one is why `Format` and not just a `Lang` reaches this far: the
/// three JSON dialects share a language but not a spelling.
///
/// Unlike `edit_ops`, there is no `splice`-style requoting step. That exists
/// because `edit`/`set` splice a user's raw argument, which strict JSON needs
/// wrapped as a string; a patch splices a *rendered* subtree, which the
/// target's own printer already spelled correctly.
pub fn applyToSlice(
    allocator: std.mem.Allocator,
    content: []const u8,
    format: Format,
    req: Request,
) !Result {
    @setEvalBranchQuota(30_000);
    switch (format) {
        // Same two refusals `edit_ops.route` makes, for the same reasons: the
        // canonical form is a parse/print pair with no span-splicing editor,
        // and gron is a CLI-only projection.
        .canonical => return error.UnsupportedCanonicalEdit,
        .gron => return error.UnsupportedGronEdit,
        inline else => |f| {
            const d = comptime fig.Language.entryFor(@tagName(f));
            if (comptime d.Lang == void) return error.FormatDisabled;
            if (comptime !d.Lang.caps.edit) return error.FormatNotEditable;
            // `toSerializeFormat` is null only for gron, returned above.
            const target = comptime (types.toSerializeFormat(f) orelse unreachable);
            return patchAs(d.Lang, allocator, content, d.dialect, target, req);
        },
    }
}

fn patchAs(
    comptime Lang: type,
    allocator: std.mem.Allocator,
    content: []const u8,
    dialect: Lang.Type,
    target: fig.AST.SerializeFormat,
    req: Request,
) !Result {
    var editor: fig.Editor(Lang) = .{ .allocator = allocator, .format = dialect };
    try editor.init(content);
    defer editor.deinit();

    const stats = fig.Patch.apply(
        Lang,
        &editor,
        target,
        req.at,
        req.patch,
        req.from,
        req.deletes,
        req.options,
    ) catch |err| {
        // `init` parsed `content`, so a splice the editor rejected from here
        // on is about text this CLI RENDERED, not about either file the user
        // named — a value with no spelling the target's reader accepts back.
        // It arrives as a plain parse error that reads as if the target were
        // malformed, so relabel it; `main` reports it against the patch.
        if (editor.splice_rejected) return error.PatchRenderRejected;
        return err;
    };
    return .{ .content = try allocator.dupe(u8, editor.source.items), .stats = stats };
}

test "applyToSlice merges one YAML document into another, touching only what changed" {
    const build_options = @import("build_options");
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;

    var parser: fig.Language.YAML.Parser = .{ .allocator = t.allocator };
    var patch = try fig.Language.YAML.parse(&parser, "replicas: 5\n", fig.Language.YAML.default_type);
    defer patch.deinit(t.allocator);

    const result = try applyToSlice(
        t.allocator,
        "name: api # keep me\nreplicas: 2\n",
        .yaml,
        .{ .patch = &patch.ast, .from = patch.ast.root },
    );
    defer t.allocator.free(result.content);
    try t.expectEqualStrings("name: api # keep me\nreplicas: 5\n", result.content);
    try t.expect(result.stats.changed());
}

test "applyToSlice refuses the two formats with no in-place editor" {
    const t = std.testing;
    var ast: fig.AST = .{ .allocator = t.allocator, .root = 0, .nodes = &.{} };
    const req: Request = .{ .patch = &ast, .from = 0 };
    try t.expectError(error.UnsupportedCanonicalEdit, applyToSlice(t.allocator, "", .canonical, req));
    try t.expectError(error.UnsupportedGronEdit, applyToSlice(t.allocator, "", .gron, req));
}
