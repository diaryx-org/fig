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
    const doc = parse_dispatch.parseSliceAs(format, allocator, content, &reports) catch |err| {
        try reports.reportDiagnostics(term, content, file_path);
        return err;
    };
    try reports.reportWarnings(term, content, file_path, quiet, strict);

    const diag_target = types.toTarget(format) orelse unreachable; // gron rejected up front above
    try parse_dispatch.reportLoss(allocator, term, &doc.ast, doc.ast.root, diag_target, serialize, false, quiet, strict);
    return printTo(allocator, term, diag_target, &doc.ast, serialize, false);
}

/// Parse `content` as `from` and re-emit it as `to` — `convert`'s core, and the
/// in-place-writeback twin of `get`'s cross-format pipeline: materializing
/// YAML's reference layer when leaving YAML (aliases/merges/tags resolved),
/// the optional `--lossless` envelope round-trip, the same lossy-conversion
/// diagnostics, and TOML's lossy null-strip when serializing without
/// `--lossless`. Unlike `get` there is no `--path`/gron projection —
/// `convert` always converts the whole document. Returns the converted bytes
/// (caller-owned).
pub fn convertSlice(
    allocator: std.mem.Allocator,
    term: *Io.Terminal,
    file_path: []const u8,
    from: Format,
    to: Format,
    content: []const u8,
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
    const doc = parse_dispatch.parseSliceAs(from, allocator, content, &reports) catch |err| {
        try reports.reportDiagnostics(term, content, file_path);
        return err;
    };
    try reports.reportWarnings(term, content, file_path, quiet, strict);

    // Leaving a language with a reference layer for one without resolves
    // the layer first (aliases → copies, merges → flattened, tags
    // applied/dropped); YAML→YAML keeps it intact for round-trip. Mirrors
    // `get`.
    const keeps_references = parse_dispatch.carriesReferences(from) and parse_dispatch.carriesReferences(to);
    const base_ast = try parse_dispatch.materializeFor(allocator, term, from, to, &doc, file_path, if (lax_tags) .lax else .strict);

    const ast: *const fig.AST = if (lossless and !keeps_references) blk: {
        // `to` is never `.gron` here (rejected up front above); its language
        // declares what it holds — see `manifest.Caps.lossless` for the
        // per-format rationale (JSON5 reuse, canonical/fig decode-only,
        // INI/dotenv/properties/plist/NestedText's lack of an envelope).
        const maybe_native: ?fig.Lossless.NativeKinds = parse_dispatch.nativeForFormat(to);
        const decoded = try allocator.create(fig.AST);
        decoded.* = try fig.Lossless.decode(allocator, base_ast);
        const native = maybe_native orelse break :blk decoded;
        const encoded = try allocator.create(fig.AST);
        encoded.* = try fig.Lossless.encode(allocator, decoded, native);
        break :blk encoded;
    } else base_ast;

    const diag_target = types.toTarget(to) orelse unreachable; // gron rejected up front above
    try parse_dispatch.reportLoss(allocator, term, ast, ast.root, diag_target, serialize, lossless, quiet, strict);
    return printTo(allocator, term, diag_target, ast, serialize, lossless);
}

/// Print `ast` whole as `target`: the oracle form as it stands, a language
/// through its vtable after the lossy strips its declarations ask for
/// (`parse_dispatch.printRuntime`). A refusal is reported and exits.
fn printTo(allocator: std.mem.Allocator, term: *Io.Terminal, target: fig.Serialize.Target, ast: *const fig.AST, serialize: fig.AST.SerializeOptions, lossless: bool) ![]u8 {
    var out: std.Io.Writer.Allocating = .init(allocator);
    defer out.deinit();
    switch (target) {
        .canonical => fig.Serialize.serializeWith(ast, &out.writer, .canonical, serialize) catch |err|
            diag_report.reportSerializeError(term, err),
        .language => |e| parse_dispatch.printRuntime(allocator, &out.writer, e, ast, ast.root, serialize, lossless) catch |err| switch (err) {
            error.OutOfMemory => return err,
            else => |x| diag_report.reportRuntimePrintError(term, x),
        },
    }
    return out.toOwnedSlice();
}
