//! The `fmt` and `convert` actions' cores: parse a slice under one format and
//! re-emit it, either in the same format (`reformatSlice`) or a different one
//! (`convertSlice`), sharing `get`'s parse-error/authoring-warning reporting
//! and lossy-conversion diagnostics so both actions' stderr output matches
//! `get`'s for the same file.
const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");

const types = @import("types.zig");
const parse_dispatch = @import("parse_dispatch.zig");
const diag_report = @import("diag_report.zig");

const Format = types.Format;
const Io = std.Io;

/// Parse `content` as `format` and re-emit it in the *same* format — the `fmt`
/// action's core, and `get`'s twin minus the cross-format machinery: since the
/// output format always equals the input, there's no YAML reference-layer
/// materialization and no `--lossless` envelope pass to consider (those only
/// matter when `from != to`). Returns the reformatted bytes (caller-owned).
pub fn reformatSlice(
    allocator: std.mem.Allocator,
    term: *Io.Terminal,
    file_path: []const u8,
    format: Format,
    content: []const u8,
    serialize: fig.AST.SerializeOptions,
    quiet: bool,
    strict: bool,
) !([]u8) {
    if (format == .gron) {
        try term.writer.print("error: cannot format gron (a CLI projection, not a stored document format).\n", .{});
        try term.writer.flush();
        return error.UnsupportedGronFmt;
    }

    var reports: parse_dispatch.Reports = .{};
    const doc = parse_dispatch.parseSliceAs(format, .{}, allocator, content, false, &reports) catch |err| {
        try reports.reportDiagnostics(term, content, file_path);
        return err;
    };
    try reports.reportWarnings(term, content, file_path, quiet, strict);

    const diag_target: fig.Runtime.Target = if (types.runtimeEntry(format)) |e|
        .{ .runtime = e }
    else
        .{ .compiled = types.toSerializeFormat(format) orelse unreachable }; // gron rejected up front above
    try parse_dispatch.reportLoss(allocator, term, &doc.ast, doc.ast.root, diag_target, serialize, false, quiet, strict);

    const target: fig.AST.SerializeFormat = switch (diag_target) {
        .compiled => |f| f,
        .runtime => |e| {
            var out: std.Io.Writer.Allocating = .init(allocator);
            defer out.deinit();
            parse_dispatch.printRuntime(allocator, &out.writer, e, &doc.ast, doc.ast.root, serialize, false) catch |err| switch (err) {
                error.OutOfMemory => return err,
                else => |x| diag_report.reportRuntimePrintError(term, x),
            };
            return out.toOwnedSlice();
        },
    };

    var out: std.Io.Writer.Allocating = .init(allocator);
    defer out.deinit();
    if (parse_dispatch.nullStripTarget(target)) |native| {
        // Same lossy-strip-then-print path `get` uses for a lossy target with
        // no null (TOML, by its own `caps.lossless`): an unrepresentable
        // value is dropped up front (already reported above) rather than
        // aborting mid-print.
        const result = try fig.Lossless.lossyStrip(allocator, &doc.ast, doc.ast.root, native);
        if (result.ast) |stripped| try stripped.serializeWith(&out.writer, target, serialize);
    } else if (parse_dispatch.flatStripDepth(target)) |depth| {
        // `fmt` never converts format (always reads and writes the same one),
        // so — unlike `convertSlice`'s twin below — there's no `--lossless` to
        // gate this on: a value already unrepresentable in the source can only
        // have gotten there via a lossless-envelope decode from a PRIOR
        // conversion, and re-emitting the same format always strips it again.
        const result = try fig.FlatStrip.lossyStrip(allocator, &doc.ast, doc.ast.root, depth);
        if (result.ast) |stripped| try stripped.serializeWith(&out.writer, target, serialize);
    } else {
        doc.ast.serializeWith(&out.writer, target, serialize) catch |err| switch (err) {
            error.FigUnrepresentableRoot => diag_report.reportFigUnrepresentableRoot(term),
            else => |e| diag_report.reportSerializeError(term, e),
        };
    }
    return out.toOwnedSlice();
}

/// Parse `content` as `from` and re-emit it as `to` — `convert`'s core, and the
/// in-place-writeback twin of `get`'s cross-format pipeline: materializing
/// YAML's reference layer when leaving YAML (aliases/merges/tags resolved),
/// the optional `--lossless` envelope round-trip, the same lossy-conversion
/// diagnostics, and TOML's lossy null-strip when serializing without
/// `--lossless`. Unlike `get` there is no `--path`/gron/`--body` projection —
/// `convert` always converts the whole document (or, under embed-archetype
/// mode, the whole embedded region's content). Returns the converted bytes
/// (caller-owned).
pub fn convertSlice(
    allocator: std.mem.Allocator,
    term: *Io.Terminal,
    file_path: []const u8,
    from: Format,
    to: Format,
    content: []const u8,
    /// `content` is the whole of `file_path`, not a region extracted from
    /// it — so a span in it names the line the user sees.
    whole_file: bool,
    serialize: fig.AST.SerializeOptions,
    lossless: bool,
    lax_tags: bool,
    quiet: bool,
    strict: bool,
) !([]u8) {
    if (to == .gron) {
        try term.writer.print("error: cannot convert to gron (a CLI projection, not a stored document format).\n", .{});
        try term.writer.flush();
        return error.UnsupportedGronFmt;
    }

    var reports: parse_dispatch.Reports = .{};
    const doc = parse_dispatch.parseSliceAs(from, .{}, allocator, content, false, &reports) catch |err| {
        try reports.reportDiagnostics(term, content, file_path);
        return err;
    };
    try reports.reportWarnings(term, content, file_path, quiet, strict);

    // Leaving a language with a reference layer for one without resolves
    // the layer first (aliases → copies, merges → flattened, tags
    // applied/dropped); YAML→YAML keeps it intact for round-trip. Mirrors
    // `get`.
    const keeps_references = parse_dispatch.carriesReferences(from) and parse_dispatch.carriesReferences(to);
    const base_ast = try parse_dispatch.materializeFor(allocator, term, from, to, &doc, file_path, whole_file, if (lax_tags) .lax else .strict);

    const ast: *const fig.AST = if (lossless and !keeps_references) blk: {
        // `to` is never `.gron` here (rejected up front above), so it always
        // has a `SerializeFormat` counterpart, whose language declares what
        // it holds — see `manifest.Caps.lossless` for the per-format
        // rationale (JSON5 reuse, canonical/fig decode-only, INI/dotenv/
        // properties/plist/NestedText's lack of an envelope).
        const maybe_native: ?fig.Lossless.NativeKinds = parse_dispatch.nativeForFormat(to);
        const decoded = try allocator.create(fig.AST);
        decoded.* = try fig.Lossless.decode(allocator, base_ast);
        const native = maybe_native orelse break :blk decoded;
        const encoded = try allocator.create(fig.AST);
        encoded.* = try fig.Lossless.encode(allocator, decoded, native);
        break :blk encoded;
    } else base_ast;

    const diag_target: fig.Runtime.Target = if (types.runtimeEntry(to)) |e|
        .{ .runtime = e }
    else
        .{ .compiled = types.toSerializeFormat(to) orelse unreachable }; // gron rejected up front above
    try parse_dispatch.reportLoss(allocator, term, ast, ast.root, diag_target, serialize, lossless, quiet, strict);

    const target: fig.AST.SerializeFormat = switch (diag_target) {
        .compiled => |f| f,
        .runtime => |e| {
            var out: std.Io.Writer.Allocating = .init(allocator);
            defer out.deinit();
            parse_dispatch.printRuntime(allocator, &out.writer, e, ast, ast.root, serialize, lossless) catch |err| switch (err) {
                error.OutOfMemory => return err,
                else => |x| diag_report.reportRuntimePrintError(term, x),
            };
            return out.toOwnedSlice();
        },
    };

    const flat_strip_depth: ?usize = if (!lossless) parse_dispatch.flatStripDepth(target) else null;

    var out: std.Io.Writer.Allocating = .init(allocator);
    defer out.deinit();
    if (if (!lossless) parse_dispatch.nullStripTarget(target) else null) |native| {
        // Same lossy-strip-then-print path `get` uses for a lossy target with
        // no null (TOML, by its own `caps.lossless`): an unrepresentable
        // value is dropped up front (already reported above) rather than
        // aborting mid-print.
        const result = try fig.Lossless.lossyStrip(allocator, ast, ast.root, native);
        if (result.ast) |stripped| try stripped.serializeWith(&out.writer, target, serialize);
    } else if (flat_strip_depth) |depth| {
        // A flat format (by its own `caps.max_mapping_depth`): same idea as
        // TOML's null-stripping above, but depth-based (see
        // `fig.FlatStrip`'s module doc); gated on `!lossless` for the same
        // reason `get`'s twin path is — see there.
        const result = try fig.FlatStrip.lossyStrip(allocator, ast, ast.root, depth);
        if (result.ast) |stripped| try stripped.serializeWith(&out.writer, target, serialize);
    } else {
        ast.serializeWith(&out.writer, target, serialize) catch |err| switch (err) {
            error.FigUnrepresentableRoot => diag_report.reportFigUnrepresentableRoot(term),
            else => |e| diag_report.reportSerializeError(term, e),
        };
    }
    return out.toOwnedSlice();
}
