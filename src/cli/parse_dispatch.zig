//! Content-based parsing shared by `get`/`fmt`/`convert`/`check`: mapping the
//! CLI's `Format` (plus an optional `--spec` version) to the registered
//! dialect that parses it, and `check`'s per-file validate-and-report entry
//! point.
const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");

const gron = @import("gron.zig");
const types = @import("types.zig");
const fileio = @import("fileio.zig");
const diag_report = @import("diag_report.zig");
const args_mod = @import("args.zig");
const languages = @import("languages.zig");

const Format = types.Format;
const Io = std.Io;

/// The canonical oracle format is opt-in (`-Dcanonical=true`) and otherwise
/// compiled out of the CLI — but always present in a test build. Mirrors the
/// `canonical_enabled` gate in `serialize.zig`.
const canonical_enabled = build_options.lang_canonical or @import("builtin").is_test;

/// The dialect `--spec <version>` selects for `format`: the registered row
/// `<format>-<version>` (`yaml-1.1`, `toml-1.0`), which a language declares
/// for each version it reads. No `--spec` is `format` itself; a version the
/// format does not declare, or a format with none, is `UnsupportedSpec`.
pub fn resolveSpec(format: Format, spec_str: ?[]const u8) error{UnsupportedSpec}!Format {
    const s = spec_str orelse return format;
    const e = types.runtimeEntry(format) orelse return error.UnsupportedSpec;
    var buf: [128]u8 = undefined;
    const row = std.fmt.bufPrint(&buf, "{s}-{s}", .{ e.name, s }) catch return error.UnsupportedSpec;
    const v = fig.Runtime.entryByName(row) orelse return error.UnsupportedSpec;
    return types.runtimeFormat(v);
}

/// What a parse left to report, as the language said it: the first error
/// of a failure (with its span and label), every error of a failure where
/// the language collects them, and the warnings of a clean parse, announced
/// under the dialect's name.
pub const Reports = struct {
    diag: ?fig.ParseDiagnostic.Rendered = null,
    errors: std.ArrayList(fig.ParseDiagnostic.Rendered) = .empty,
    warnings: std.ArrayList(fig.ParseDiagnostic.Rendered) = .empty,
    label: []const u8 = "",

    /// Print a `file:line:col` teaching message for the parse's failure.
    pub fn reportDiagnostics(self: *const Reports, term: *Io.Terminal, source: []const u8, file: []const u8) !void {
        if (self.diag) |d|
            try diag_report.reportParseError(term, source, file, d.offset, d.end, d.message, d.short_label);
    }

    /// Announce authoring-time lints. These ride the same `--quiet`/
    /// `--strict` contract as the serialize-side diagnostics: quiet
    /// silences, strict aborts.
    pub fn reportWarnings(self: *const Reports, term: *Io.Terminal, source: []const u8, file: []const u8, quiet: bool, strict: bool) !void {
        try diag_report.handleRenderedWarnings(term, source, file, self.label, self.warnings.items, quiet, strict);
    }

    /// Every recorded error — the `recover` path, where one parse reports
    /// every error in the file rather than stopping at the first — else the
    /// one diagnostic, else null.
    pub fn renderErrors(self: *const Reports, allocator: std.mem.Allocator) !?[]const fig.ParseDiagnostic.Rendered {
        if (self.errors.items.len > 0) return self.errors.items;
        const d = self.diag orelse return null;
        const one = try allocator.alloc(fig.ParseDiagnostic.Rendered, 1);
        one[0] = d;
        return one;
    }

    /// The warnings twin of `renderErrors`.
    pub fn renderWarnings(self: *const Reports, allocator: std.mem.Allocator) !?[]const fig.ParseDiagnostic.Rendered {
        _ = allocator;
        if (self.warnings.items.len > 0) return self.warnings.items;
        return null;
    }
};

/// Parse already-read `content` as the CLI `format`. The content-based
/// parser the `get` and `check` actions use: reading the input once means
/// detection and parsing share the same bytes, so a piped stdin is consumed
/// only once. `.canonical` is the AST's 1:1 oracle grammar, `.gron` ungron.
///
/// `reports` receives what the language said: its first error and, when it
/// collects them, every error; its warnings on a clean parse.
pub fn parseSliceAs(format: Format, allocator: std.mem.Allocator, content: []const u8, reports: *Reports) !fig.Document {
    return switch (format) {
        .canonical => if (comptime canonical_enabled) fig.Canonical.parse(allocator, content) else error.FormatDisabled,
        // gron ("ungron") reconstructs the AST from its `path = value` lines,
        // reusing the JSON parser for each RHS.
        .gron => gron.parseDocument(allocator, content),
        _ => {
            const e = types.runtimeEntry(format) orelse return error.FormatDisabled;
            var parser: fig.Runtime.Language.Parser = .{
                .allocator = allocator,
                .errors = &reports.errors,
                .warnings = &reports.warnings,
            };
            reports.label = e.name;
            return fig.Runtime.Language.parse(&parser, content, e.typeOf()) catch |err| {
                if (err == error.RuntimeParseFailed) {
                    // The language's own first error where it left one, with
                    // its span and label; else the message and offset it gave.
                    reports.diag = if (reports.errors.items.len > 0) reports.errors.items[0] else .{
                        .offset = parser.last_error.byte_offset,
                        .message = try allocator.dupe(u8, parser.lastMessage()),
                        .short_label = e.name,
                    };
                }
                return err;
            };
        },
    };
}

/// Whether `f`'s language declares a reference layer — anchors, aliases,
/// merges, tags — as `Caps.references`. gron and canonical are projections
/// of the core AST and carry none.
pub fn carriesReferences(f: Format) bool {
    const e = types.runtimeEntry(f) orelse return false;
    return e.language.caps.references;
}

/// `ast` as the printer for `to` must see it: collapsed by `Materialize`
/// when it leaves a language that carries a reference layer for one that
/// does not, and untouched otherwise — a YAML→YAML conversion keeps its
/// anchors, a JSON→anything one has none to collapse. Neither side is named:
/// each is asked (`carriesReferences`), so a runtime twin of YAML answers as
/// YAML does. `mode` is the tag policy — lax keeps unknown tags, strict
/// refuses them. The copy lives in `a`.
///
/// A document the pass refuses — a custom tag under strict mode, a tag its
/// value does not fit, an alias inside its own anchor — is reported to `term`
/// against `file` and the process exits 1, the status the unreported error
/// had. The report points at the node: the document was parsed from the
/// whole of `file`, so its spans are offsets into what `file` shows the user.
pub fn materializeFor(a: std.mem.Allocator, term: *Io.Terminal, from: Format, to: Format, doc: *const fig.Document, file: []const u8, mode: fig.Materialize.TagMode) !*const fig.AST {
    if (!carriesReferences(from) or carriesReferences(to)) return &doc.ast;
    const mat = try a.create(fig.AST);
    var culprit: ?fig.AST.Node.Id = null;
    mat.* = fig.Materialize.materializeReporting(a, &doc.ast, mode, &culprit) catch |err| switch (err) {
        error.UnknownTag, error.TagTypeMismatch, error.AliasCycle => diag_report.reportMaterializeError(
            term,
            err,
            doc,
            culprit,
            file,
            types.name(to),
        ),
        else => return err,
    };
    return mat;
}

/// `Materialize.materialize` into a copy allocated in `a`.
pub fn materialize(a: std.mem.Allocator, ast: *const fig.AST, mode: fig.Materialize.TagMode) !*const fig.AST {
    const mat = try a.create(fig.AST);
    mat.* = try fig.Materialize.materialize(a, ast, mode);
    return mat;
}

/// The lossless-envelope declaration of a CLI output format: what the
/// `$fig` envelope pass encodes for. gron's value layer is JSON, so it takes
/// JSON's; a registered target's is its entry's `caps.lossless`; canonical
/// takes none.
pub fn nativeForFormat(to: Format) ?fig.Lossless.NativeKinds {
    if (to == .gron) return if (fig.Runtime.entryByName("json")) |e| e.language.caps.lossless else null;
    const e = types.runtimeEntry(to) orelse return null;
    return e.language.caps.lossless;
}

/// Print `ast` from `node_id` in the registered language `e` — the tail of
/// `get`, `fmt` and `convert`: the lossy strips, read off the entry's own
/// declarations (`caps.lossless` for the null strip, `max_mapping_depth` for
/// the flat strip — both, when it declares both), then the vtable's `print`.
/// `reportLoss` warns about what the strips drop, from the same declarations.
pub fn printRuntime(allocator: std.mem.Allocator, writer: *Io.Writer, e: *const fig.Runtime.Entry, ast: *const fig.AST, node_id: fig.AST.Node.Id, serialize: fig.AST.SerializeOptions, lossless: bool) !void {
    if (!e.language.caps.serialize) return error.FormatNotSerializable;
    const caps = e.language.caps;
    const null_strip: ?fig.Lossless.NativeKinds = if (!lossless) blk: {
        const native = caps.lossless orelse break :blk null;
        break :blk if (native.null) null else native;
    } else null;
    const depth: ?usize = if (!lossless) (if (caps.max_mapping_depth) |d| @as(usize, d) else null) else null;

    var tree: *const fig.AST = ast;
    var root = node_id;
    var after_nulls: fig.AST = undefined;
    var after_depth: fig.AST = undefined;
    if (null_strip) |native| {
        after_nulls = (try fig.Lossless.lossyStrip(allocator, tree, root, native)).ast orelse return;
        tree = &after_nulls;
        root = tree.root;
    }
    if (depth) |d| {
        after_depth = (try fig.FlatStrip.lossyStrip(allocator, tree, root, d)).ast orelse return;
        tree = &after_depth;
        root = tree.root;
    }
    // A strip re-roots the tree at `node_id`, so what it returns prints whole.
    if (tree == ast)
        try fig.Runtime.printNodeWith(e, writer, ast, node_id, serialize)
    else
        try fig.Runtime.printWith(e, writer, tree, serialize);
}

/// Warn on stderr about everything printing `ast` from `node_id` to `target`
/// would lose — comments dropped or degraded, values dropped or degraded —
/// unless `quiet`; with `strict`, any such loss exits 1. Run on the AST as it
/// will be printed: under `lossless` the lossy nodes are already enveloped,
/// so no value warnings fire. The shared tail of `get`, `convert` and `fmt`,
/// for a compiled target and a runtime one alike.
pub fn reportLoss(
    allocator: std.mem.Allocator,
    term: *Io.Terminal,
    ast: *const fig.AST,
    node_id: fig.AST.Node.Id,
    target: fig.Runtime.Target,
    serialize: fig.AST.SerializeOptions,
    lossless: bool,
    quiet: bool,
    strict: bool,
) !void {
    if (quiet and !strict) return;
    const warnings = try fig.Diagnostics.analyze(allocator, ast, node_id, target, .{
        .pretty = serialize.pretty,
        .strip_comments = serialize.strip_comments,
        .lossless = lossless,
    });
    // The CLI only surfaces losses the FORMAT forced. A loss the user
    // explicitly asked for (e.g. `--strip-comments`) carries
    // `explicit_option` and is not surprising, so it neither warns nor
    // trips `--strict` — it just rides through on the warning layer for a
    // library consumer that wants it.
    var surfaced: usize = 0;
    for (warnings) |w| {
        if (w.cause != .format_limitation) continue;
        surfaced += 1;
        if (!quiet) {
            try term.setColor(.yellow);
            try term.writer.writeAll("warning: ");
            try term.setColor(.reset);
            try w.render(term.writer, target.name());
            try term.writer.writeByte('\n');
        }
    }
    if (!quiet) try term.writer.flush();
    if (strict and surfaced > 0) {
        try term.writer.print("error: {d} lossy conversion warning(s); --strict aborts.\n", .{surfaced});
        try term.writer.flush();
        std.process.exit(1);
    }
}

/// Sniff `content` with `Language.detect` and return what it inferred — the
/// fallback when neither `--input` nor the file extension pinned the format.
/// Errors (after a clear message) if nothing matches.
///
/// A successful guess is logged at DEBUG level only, so a release build says
/// nothing: `cat f | fig get - a` is a pipeline, and a note on stderr for
/// every sniffed stdin is noise in it — as silent as a file whose extension
/// resolved it. The guess matters only when it is wrong, and then the parse
/// fails and says so.
/// The order the CLI sniffs content in: strictest grammar first, since
/// input valid as more than one format means the earliest. JSON before
/// JSON5, which reads every JSON document; ZON and plist, whose openers no
/// other format shares; TOML before figl and INI, which read most `a = 1`
/// files; dotenv before YAML, which reads nearly anything; `.properties`
/// and NestedText last, which read every line as something. JSONC is never
/// sniffed: JSON5 reads every JSONC document.
pub const sniff_order = [_][]const u8{
    "json", "json5",  "zon",  "plist",      "toml",       "figl",
    "ini",  "dotenv", "yaml", "properties", "nestedtext",
};

/// The extensions of host documents whose contents a sniff would misread.
fn documentKind(ext: []const u8) ?[]const u8 {
    const kinds = [_]struct { []const u8, []const u8 }{
        .{ "md", "a Markdown" }, .{ "markdown", "a Markdown" },
        .{ "html", "an HTML" },  .{ "htm", "an HTML" },
    };
    for (kinds) |k| if (std.ascii.eqlIgnoreCase(ext, k[0])) return k[1];
    return null;
}

pub fn resolveFormatFromContent(allocator: std.mem.Allocator, content: []const u8, file_path: []const u8) !Format {
    // A name in the order this build does not bundle is tried through the
    // language configured (or, on WASI, served by the host) under it, which
    // is registered only when asked for.
    // A file whose extension belongs to a language fig ships but this build
    // leaves out is that language's, not whatever a sniff would make of it.
    const dot = std.mem.findLast(u8, file_path, ".");
    if (dot) |d| if (fig.bundle.unbundledOwner(file_path[d + 1 ..])) |owner| {
        std.log.scoped(.detect).err("`{s}` is a {s} file, and this build does not include {s}; configure a language for it in languages.figl (see `fig lang --help`)", .{ file_path, owner, owner });
        return error.UnsupportedFileFormat;
    };
    // A document is not sniffed: its frontmatter would read as a config
    // file and an edit would land in the prose. A language configured for
    // the extension has already claimed it before this point.
    if (dot) |d| if (documentKind(file_path[d + 1 ..])) |kind| {
        std.log.scoped(.detect).err("`{s}` is {s} document, not a config file; fig does not edit embedded blocks (pass an explicit format to read the whole file as one)", .{ file_path, kind });
        return error.UnsupportedFileFormat;
    };
    for (sniff_order) |n| {
        if (fig.Runtime.entryByName(n) == null) _ = languages.resolveName(n);
    }
    const e = fig.Runtime.detectAmong(allocator, content, &sniff_order) orelse {
        std.log.scoped(.detect).err("could not infer the format of `{s}` from its contents; pass an explicit format", .{file_path});
        return error.UnsupportedFileFormat;
    };
    const format = types.runtimeFormat(e);
    std.log.scoped(.detect).debug("inferred format `{s}` for `{s}` from its contents", .{ types.name(format), file_path });
    return format;
}

/// Open `file_path` read-only and sniff its contents. For the in-place edit paths
/// (`replace`/`rename`/`comment`), which then re-open the file read-write to splice it — so
/// detection reads through a separate handle and never disturbs the edit read.
pub fn detectFileFormat(io: Io, allocator: std.mem.Allocator, file_path: []const u8) !Format {
    const probe = try fileio.getInput(io, file_path, .read_only);
    defer if (!std.mem.eql(u8, file_path, "-")) probe.close(io);
    const content = try fileio.readAll(allocator, io, probe);
    return resolveFormatFromContent(allocator, content, file_path);
}

/// Validate that `file` parses cleanly, returning the resolved format on success.
/// Format precedence mirrors `get`: an explicit `--input` `override`, else the
/// file extension, else sniffing the contents. `spec_str` (from `--spec`) pins
/// the language version to validate against and is resolved once the format is
/// known — an unknown/inapplicable version is reported like a parse error.
/// Any IO/parse/spec error propagates to
/// the caller, which reports it — except for the languages in the report layer
/// (`reporting` above): there, a parse failure fills
/// `diag_errors` with every diagnostic rendered into the language-agnostic
/// `ParseDiagnostic.Rendered` shape, and a clean parse may fill `diag_warnings`
/// the same way (both borrow `diag_source`, which is set alongside them). The
/// caller renders these live against the real terminal via `printDiag` rather
/// than a pre-rendered string, so it can color the label — see `printDiag`'s
/// doc comment for why that can't happen in here instead.
pub fn checkOne(allocator: std.mem.Allocator, io: Io, file: []const u8, override: ?Format, spec_str: ?[]const u8, diag_source: *?[]const u8, diag_errors: *?[]const fig.ParseDiagnostic.Rendered, diag_warnings: *?[]const fig.ParseDiagnostic.Rendered) !Format {
    const input = try fileio.getInput(io, file, .read_only);
    defer if (!std.mem.eql(u8, file, "-")) input.close(io);
    const content = try fileio.readAll(allocator, io, input);

    const format: Format = if (override) |f|
        f
    else if (args_mod.detectLanguageFromFileEnding(file)) |d|
        d.format
    else
        try resolveFormatFromContent(allocator, content, file);

    // Resolve `--spec` against the now-known format. This rejects a nonsense
    // version (e.g. `--spec 1.0` on a JSON or markdown file) before parsing.
    const dialect = try resolveSpec(format, spec_str);

    // A parse error is all we need to validate: the parsed result is
    // discarded.
    diag_source.* = content;
    var reports: Reports = .{};
    // A language that collects every error reports them all in one pass
    // (`check` shouldn't hide errors 2..N behind the first); one that
    // does not stops at its first.
    _ = parseSliceAs(dialect, allocator, content, &reports) catch |err| {
        diag_errors.* = try reports.renderErrors(allocator);
        return err;
    };
    diag_warnings.* = try reports.renderWarnings(allocator);
    return format;
}

test "resolveSpec selects a version's dialect row" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const yaml = args_mod.parseFormatName("yaml").?;
    try t.expectEqual(yaml, try resolveSpec(yaml, null));
    try t.expectEqualStrings("yaml-1.2.2", types.name(try resolveSpec(yaml, "1.2.2")));
    try t.expectEqualStrings("yaml-1.1", types.name(try resolveSpec(yaml, "1.1")));
    try t.expectEqualStrings("yaml-1.1.0", types.name(try resolveSpec(yaml, "1.1.0")));
    // `--input yml --spec 1.1` still selects YAML 1.1: `parseFormatName`
    // collapses the spelling to `yaml` first.
    try t.expectEqualStrings("yaml-1.1", types.name(try resolveSpec(args_mod.parseFormatName("yml").?, "1.1")));
    try t.expectError(error.UnsupportedSpec, resolveSpec(yaml, "1.3"));
    try t.expectError(error.UnsupportedSpec, resolveSpec(yaml, "2"));
    try t.expectError(error.UnsupportedSpec, resolveSpec(.canonical, "1.1"));
}

// What the runtime printer in the test below was handed as the root row's tag.
var seen_root_tag: [32]u8 = undefined;
var seen_root_tag_len: usize = 0;
var inner_print: ?fig.Runtime.PrintFn = null;

fn recordRootTag(
    ctx: ?*anyopaque,
    dialect: [*:0]const u8,
    view: *const fig.Runtime.table.View,
    options: *const fig.Runtime.PrintOptions,
    out: *fig.Runtime.Str,
    err: *fig.Runtime.ErrorInfo,
) callconv(.c) c_int {
    const T = fig.Runtime.table;
    var tag: []const u8 = "";
    for (T.slice(T.Tag, view.tags, view.tag_count)) |tg| {
        if (tg.node == 0) tag = view.textOf(tg.text) orelse "";
    }
    @memcpy(seen_root_tag[0..tag.len], tag);
    seen_root_tag_len = tag.len;
    return inner_print.?(ctx, dialect, view, options, out, err);
}

test "printRuntime hands a runtime printer the tags its lossy strip rebuilt the tree with" {
    const t = std.testing;
    const R = fig.Runtime;
    var alloc: R.test_language.Alloc = .{ .allocator = t.allocator };

    var b = fig.AST.Builder.init(t.allocator);
    defer b.deinit();
    const k = try b.addString("a");
    const v = try b.addString("b");
    const root = try b.addMapping(&.{.{ .key = k, .value = v }});
    try b.setTag(root, .{ .text = "!strings" });
    var ast = try b.finish(root);
    defer ast.deinit();

    // Once through each strip `printRuntime` can take: the depth strip the
    // test language's `max_mapping_depth` asks for, then the null strip a
    // `lossless` declaration without `null` asks for.
    for ([_]bool{ false, true }) |declares_lossless| {
        R.deinitAll();
        defer R.deinitAll();
        var vt = R.test_language.vtable(&alloc);
        inner_print = vt.print;
        vt.print = recordRootTag;
        if (declares_lossless) vt.lossless = R.lossless_envelope;
        const e = R.entryByAbi(try R.register(t.allocator, &vt)).?;

        seen_root_tag_len = 0;
        var arena = std.heap.ArenaAllocator.init(t.allocator);
        defer arena.deinit();
        var out: Io.Writer.Allocating = .init(t.allocator);
        defer out.deinit();
        try printRuntime(arena.allocator(), &out.writer, e, &ast, ast.root, .{}, false);
        try t.expectEqualStrings("a=b\n", out.written());
        try t.expectEqualStrings("!strings", seen_root_tag[0..seen_root_tag_len]);
    }
}
