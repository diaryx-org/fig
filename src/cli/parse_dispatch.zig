//! Content-based parsing shared by `get`/`fmt`/`convert`/`check`: mapping the
//! CLI's `Format` (plus an optional `--spec` version) to the right language
//! parser, and `check`'s per-file validate-and-report entry point.
const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");

const gron = @import("gron.zig");
const types = @import("types.zig");
const fileio = @import("fileio.zig");
const diag_report = @import("diag_report.zig");
const args_mod = @import("args.zig");

const Format = types.Format;
const Io = std.Io;
/// The language registry. A gated-out format is `void` here, which is what the
/// `DialectOf`/`ReportOf` helpers below key off instead of repeating a
/// `build_options` test per field.
const L = fig.Language;

/// The canonical oracle format is opt-in (`-Dcanonical=true`) and otherwise
/// compiled out of the CLI — but always present in a test build. Mirrors the
/// `canonical_enabled` gate in `ast/serialize_options.zig`.
const canonical_enabled = build_options.lang_canonical or @import("builtin").is_test;

/// Per-language version/dialect to parse under. Each field defaults to its
/// language's `default_type`, so `parseSliceAs(fmt, .{}, …)` behaves exactly as
/// before — only `check --spec` overrides a field. JSON strictness is carried by
/// the `Format` itself (json/jsonc/json5); ZON/XML/native have one grammar each,
/// so they need no field here.
pub const Spec = struct {
    toml: DialectOf(L.TOML) = defaultDialect(L.TOML),
    yaml: DialectOf(L.YAML) = defaultDialect(L.YAML),
};

/// `Lang.Type` when `Lang` is compiled in, `void` when it is gated out — and
/// its `default_type` twin. Both now live in `languages/language.zig`, beside
/// the format registry that needs the same `void` protocol one layer below the
/// CLI; re-exported here under their original names so every consumer in this
/// file (and the `Spec` fields above) reads exactly as it did.
pub const DialectOf = fig.Language.DialectOf;
pub const defaultDialect = fig.Language.defaultDialect;

/// Resolve a `--spec` version string against the format it will parse. Null
/// `spec_str` yields the default spec. Errors when the version is unknown for
/// that format, or when the format exposes no selectable version (then `--spec`
/// doesn't apply — JSON strictness is the format name, ZON/XML/native are
/// single-grammar). YAML selects 1.2.2 (default) or 1.1; the versions differ in
/// scalar type resolution (see `scalarKind1_1` in the YAML parser).
pub fn resolveSpec(format: Format, spec_str: ?[]const u8) error{UnsupportedSpec}!Spec {
    @setEvalBranchQuota(30_000);
    const s = spec_str orelse return .{};
    return switch (format) {
        // Neither is a registry entry, and neither has a version to name: the
        // canonical oracle grammar is the AST's own 1:1 encoding, and gron is a
        // projection of JSON.
        .canonical, .gron => error.UnsupportedSpec,
        inline else => |f| {
            const d = comptime L.entryFor(@tagName(f));
            // Two ways `--spec` is inapplicable, and they report identically:
            // a gated-out language has no dialect to select at all, and a
            // single-grammar one (the JSON family — strictness is the format
            // NAME here — plus ZON/XML/fig/INI/dotenv/.properties/plist/
            // NestedText) has no version to select between.
            if (comptime d.Lang == void or d.specs.len == 0) return error.UnsupportedSpec;
            inline for (d.specs) |v| {
                if (std.mem.eql(u8, s, v.name)) {
                    // `Spec`'s field per language is named for the registry
                    // entry, which for both multi-version languages is also
                    // the language's own name.
                    var out: Spec = .{};
                    @field(out, d.name) = v.dialect;
                    return out;
                }
            }
            return error.UnsupportedSpec;
        },
    };
}

// `Spec`'s completeness against the registry, both directions: the hand-written
// fields above are what `resolveSpec` fills and what `parseSliceAs` reads, so
// an entry that grew a `--spec` table without a field here would resolve into
// nothing, and a field here with no registry versions would never be written.
comptime {
    @setEvalBranchQuota(20_000);
    for (fig.Language.dialects) |d| {
        if (d.specs.len == 0) continue;
        if (!@hasField(Spec, d.name))
            @compileError("the format registry lists `--spec` versions for '" ++ d.name ++
                "', but `Spec` in this file has no field of that name for `resolveSpec` to fill");
        if (@FieldType(Spec, d.name) != DialectOf(d.Lang))
            @compileError("`Spec." ++ d.name ++ "` is not the dialect type the format registry's" ++
                " versions for '" ++ d.name ++ "' select");
    }
    for (@typeInfo(Spec).@"struct".fields) |f| {
        if (fig.Language.entryFor(f.name).specs.len == 0)
            @compileError("`Spec` has the field '" ++ f.name ++ "', but the format registry lists no" ++
                " `--spec` versions for it — nothing would ever fill it");
    }
}

// The version STRINGS themselves, pinned as literals. `resolveSpec` now derives
// its behaviour from the registry, so asserting the two agree would be circular
// — what is left to state is the user-facing contract the registry is now the
// only home for: exactly these spellings are accepted, in this order, for these
// two languages and no others. (The dialect each selects is checked by the
// registry's own asserts in `language.zig`; here only the names are build-
// invariant, since a gated-out language still lists them.)
comptime {
    @setEvalBranchQuota(20_000);
    const expected = .{
        .{ "toml", [_][]const u8{ "1.0", "1.0.0", "1.1", "1.1.0" } },
        .{ "yaml", [_][]const u8{ "1.2", "1.2.2", "1.1", "1.1.0" } },
    };
    for (expected) |e| {
        const got = fig.Language.entryFor(e[0]).specs;
        if (got.len != e[1].len)
            @compileError("the format registry no longer lists exactly " ++
                std.fmt.comptimePrint("{d}", .{e[1].len}) ++ " `--spec` versions for '" ++ e[0] ++ "'");
        for (got, e[1]) |g, w| {
            if (!std.mem.eql(u8, g.name, w))
                @compileError("the format registry's `--spec` versions for '" ++ e[0] ++
                    "' no longer read `" ++ w ++ "` where they did — `fig check --spec` is a" ++
                    " user-facing contract, so a spelling cannot change or move silently");
        }
    }
    for (fig.Language.dialects) |d| {
        if (d.specs.len == 0) continue;
        var known = false;
        for (expected) |e| {
            if (std.mem.eql(u8, e[0], d.name)) known = true;
        }
        if (!known)
            @compileError("'" ++ d.name ++ "' grew a `--spec` version table — add its expected" ++
                " spellings to the pin above (and a `Spec` field, which the assert before this" ++
                " one already demanded)");
    }
}

/// The languages that have grown the rich diagnostic layer — position +
/// teaching message, authoring-time warnings (see `languages/fig/parser.zig`'s
/// `Report` and its JSON/TOML twins) — each with the label its warnings are
/// announced under. Every other format still reports a bare error name.
///
/// One list, walked by all four of `Reports`' methods, so a language joining
/// the diagnostic layer is added here and nowhere else.
///
/// MEMBERSHIP IS NOT DECIDED HERE. `parseSliceAs` routes a language through the
/// report layer exactly when its `Parser` declares `parseWithReport`, so this
/// list only supplies the LABEL each language's authoring-time lints are
/// announced under — the completeness assert below fails the build if the two
/// ever disagree. That is how INI/dotenv/`.properties`/NestedText joined: their
/// parsers already produced positions and teaching messages, and nothing but
/// this table's silence was keeping the CLI from printing them.
///
/// Not every member fills every column: NestedText's `Report` is a lone `diag`
/// (no error list, no `Warning` type), so the methods below skip what a given
/// language does not have rather than demanding one report shape from all.
const reporting = .{
    .{ "fig", L.FIG, "fig authoring" },
    .{ "json", L.JSON, "JSON authoring" },
    .{ "toml", L.TOML, "TOML authoring" },
    .{ "ini", L.INI, "INI authoring" },
    .{ "dotenv", L.DOTENV, "dotenv authoring" },
    .{ "properties", L.PROPERTIES, ".properties authoring" },
    .{ "nestedtext", L.NESTEDTEXT, "NestedText authoring" },
};

/// A language's `Parser.Report` when it is compiled in, `void` when gated out.
/// Same reasoning as `DialectOf`.
fn ReportOf(comptime Lang: type) type {
    return if (Lang == void) void else Lang.Parser.Report;
}

fn emptyReport(comptime Lang: type) ReportOf(Lang) {
    return if (Lang == void) {} else .{};
}

/// The parse reports the CLI collects, as one value a caller declares, hands to
/// `parseSliceAs`, and then asks to render itself.
///
/// This was three loose locals plus ten lines of hand-written rendering,
/// repeated at four call sites — which was also four places for a gated-out
/// language's `Report` type to fail to exist. Bundling puts the `void` handling
/// in `ReportOf` and the rendering in the methods below, so a `-D<lang>=false`
/// build drops that language's arm from every consumer at once.
pub const Reports = struct {
    fig: ReportOf(L.FIG) = emptyReport(L.FIG),
    json: ReportOf(L.JSON) = emptyReport(L.JSON),
    toml: ReportOf(L.TOML) = emptyReport(L.TOML),
    ini: ReportOf(L.INI) = emptyReport(L.INI),
    dotenv: ReportOf(L.DOTENV) = emptyReport(L.DOTENV),
    properties: ReportOf(L.PROPERTIES) = emptyReport(L.PROPERTIES),
    nestedtext: ReportOf(L.NESTEDTEXT) = emptyReport(L.NESTEDTEXT),

    /// Print a `file:line:col` teaching message for each language that recorded
    /// a single parse diagnostic. At most one ever has: only the format that
    /// was actually parsed fills a report.
    pub fn reportDiagnostics(self: *const Reports, term: *Io.Terminal, source: []const u8, file: []const u8) !void {
        inline for (reporting) |r| {
            if (comptime r[1] != void) {
                const P = r[1].Parser;
                if (@field(self, r[0]).diag) |d|
                    try diag_report.reportParseError(term, source, file, d.offset, d.end, P.describe(d.code), P.shortLabel(d.code));
            }
        }
    }

    /// Announce authoring-time lints under each language's label. These ride
    /// the same `--quiet`/`--strict` contract as the serialize-side
    /// diagnostics: quiet silences, strict aborts.
    ///
    /// Skips a language whose parser has no `Warning` type at all (NestedText):
    /// it reports errors but has grown no authoring lints, so there is nothing
    /// to announce rather than an empty list to walk.
    pub fn reportWarnings(self: *const Reports, term: *Io.Terminal, source: []const u8, file: []const u8, quiet: bool, strict: bool) !void {
        inline for (reporting) |r| {
            if (comptime r[1] != void and @hasDecl(r[1].Parser, "Warning")) {
                const W = r[1].Parser.Warning;
                try diag_report.handleParseWarnings(term, source, file, r[2], @field(self, r[0]).warnings, W.describeWarning, W.shortLabel, quiet, strict);
            }
        }
    }

    /// Every recorded error, rendered into the language-agnostic shape
    /// `checkOne`'s caller prints — the `recover` path, where one parse reports
    /// every error in the file rather than stopping at the first. Falls back to
    /// the single `diag` when a language recorded one but no error list. Null
    /// when no language recorded anything.
    pub fn renderErrors(self: *const Reports, allocator: std.mem.Allocator) !?[]const fig.ParseDiagnostic.Rendered {
        inline for (reporting) |r| {
            if (comptime r[1] != void) {
                const P = r[1].Parser;
                const rep = @field(self, r[0]);
                // A language whose parser has no `parseCollecting` never fills
                // an error list (NestedText), so its `Report` doesn't carry
                // one — the single `diag` below is all it can offer, which is
                // exactly the fallback this arm already had.
                if (comptime @hasField(@TypeOf(rep), "errors")) {
                    if (rep.errors.len > 0)
                        return try diag_report.renderAll(allocator, rep.errors, P.describe, P.shortLabel);
                }
                if (rep.diag) |d|
                    return try diag_report.renderAll(allocator, &[_]P.Diagnostic{d}, P.describe, P.shortLabel);
            }
        }
        return null;
    }

    /// The warnings twin of `renderErrors`. Skips the languages with no
    /// `Warning` type, same as `reportWarnings`.
    pub fn renderWarnings(self: *const Reports, allocator: std.mem.Allocator) !?[]const fig.ParseDiagnostic.Rendered {
        inline for (reporting) |r| {
            if (comptime r[1] != void and @hasDecl(r[1].Parser, "Warning")) {
                const W = r[1].Parser.Warning;
                const rep = @field(self, r[0]);
                if (rep.warnings.len > 0)
                    return try diag_report.renderAll(allocator, rep.warnings, W.describeWarning, W.shortLabel);
            }
        }
        return null;
    }
};

// `Reports`/`reporting` completeness against the registry. `parseSliceAs` picks
// its parse flavour by DECLARATION — a `Parser` with `parseWithReport` gets a
// report sink — so a language that grows one joins the diagnostic layer whether
// or not anybody remembered to give it a field here. This is what turns that
// silent omission (the bug fixed in this stage: four languages produced
// positions and messages the CLI threw away) into a build failure that names
// the language.
comptime {
    @setEvalBranchQuota(20_000);
    for (fig.Language.dialects) |d| {
        if (d.Lang == void) continue;
        if (!@hasDecl(d.Lang.Parser, "parseWithReport")) continue;
        if (!@hasField(Reports, d.Lang.name))
            @compileError("`" ++ d.Lang.name ++ "`'s parser declares `parseWithReport`, so `parseSliceAs`" ++
                " routes it through the report layer — but `Reports` has no `" ++ d.Lang.name ++
                "` field for the report to land in");
        var listed = false;
        for (reporting) |r| {
            if (std.mem.eql(u8, r[0], d.Lang.name)) listed = true;
        }
        if (!listed)
            @compileError("`" ++ d.Lang.name ++ "` fills a parse report, but `reporting` has no row for" ++
                " it — its diagnostics would be collected and then never printed. Add one, with the" ++
                " label its authoring-time warnings should be announced under");
    }
    for (reporting) |r| {
        if (!@hasField(Reports, r[0]))
            @compileError("`reporting` lists '" ++ r[0] ++ "', which `Reports` has no field for");
    }
}

/// Parse already-read `content` as the CLI `format` under `spec`. The
/// content-based parser the `get` and `check` actions use: reading the input
/// once means detection and parsing share the same bytes, so a piped stdin is
/// consumed only once. `.jsonc`/`.json5` select the JSON dialect; `.canonical`
/// is the AST's 1:1 oracle grammar. `spec` picks the version where one is
/// selectable (TOML 1.0 vs 1.1, YAML version).
///
/// `reports` (fields optional) receives each covered language's own parse
/// report — `diag` on failure (position + teaching message), `warnings`
/// (authoring-time lints) where the language has them; `errors` (every failure,
/// source order) when `recover` and the parser can recover. Which languages
/// those are is not a list anybody maintains: it is every one whose `Parser`
/// declares `parseWithReport`, resolved per arm below.
pub fn parseSliceAs(format: Format, spec: Spec, allocator: std.mem.Allocator, content: []const u8, recover: bool, reports: *Reports) !fig.Document {
    @setEvalBranchQuota(30_000);
    return switch (format) {
        // The AST's own 1:1 oracle grammar — not a `Language`, so not a
        // registry entry, so its own arm.
        .canonical => if (comptime canonical_enabled) fig.Canonical.parse(allocator, content) else error.FormatDisabled,
        // gron ("ungron") reconstructs the AST from its `path = value` lines,
        // reusing the JSON parser for each RHS — so it needs JSON compiled in.
        .gron => if (comptime build_options.lang_json) gron.parseDocument(allocator, content) else error.FormatDisabled,
        inline else => |f| {
            const d = comptime L.entryFor(@tagName(f));
            if (comptime d.Lang == void) return error.FormatDisabled;
            const P = d.Lang.Parser;

            // Which grammar version to parse under. `--spec` picks it for the
            // languages that expose one (`Spec` carries a field named for the
            // entry); otherwise the entry's own dialect — which is exactly
            // what splits one JSON parser into the json/jsonc/json5 formats.
            const dialect = if (comptime @hasField(Spec, d.name)) @field(spec, d.name) else d.dialect;

            // Parse flavour, chosen by what the parser DECLARES rather than by
            // a list of languages: no report at all, a report, or a report
            // that keeps going after the first error. `recover` collects the
            // whole file's errors (`check`); otherwise the parse stops at the
            // first (`get`/`fmt`/`convert` only need to fail once).
            if (comptime !@hasDecl(P, "parseWithReport")) return P.parse(allocator, content, dialect);
            // The sink is keyed by LANGUAGE, not by dialect: the three JSON
            // formats share one parser and one `Report`.
            const sink = &@field(reports, d.Lang.name);
            if (comptime @hasDecl(P, "parseCollecting")) {
                if (recover) return P.parseCollecting(allocator, content, dialect, sink);
            }
            return P.parseWithReport(allocator, content, dialect, sink);
        },
    };
}

/// Map a `Language.detect` result to the CLI `Format`. `Detected` has no
/// `jsonc` or `canonical` (neither is content-sniffed), so the mapping is
/// total.
pub fn mapDetected(d: fig.Language.Detected) Format {
    return switch (d) {
        .json => .json,
        .json5 => .json5,
        .yaml => .yaml,
        .toml => .toml,
        .zon => .zon,
        .xml => .xml,
        .fig => .fig,
        .ini => .ini,
        .dotenv => .dotenv,
        .properties => .properties,
        .plist => .plist,
        .nestedtext => .nestedtext,
    };
}

/// Map a document-serialize `target` to its `fig.FlatStrip.Format` counterpart,
/// or null for every format that isn't one of the three flat/shallow-only
/// ones `FlatStrip` covers. `get`/`convert`'s lossy path use this to decide
/// whether to run `FlatStrip.lossyStrip` before printing (mirroring how they
/// hardcode `.toml` for `Lossless.lossyStrip`, just over three formats instead
/// of one).
pub fn flatStripFormat(target: fig.AST.SerializeFormat) ?fig.FlatStrip.Format {
    return switch (target) {
        .ini => .ini,
        .dotenv => .dotenv,
        .properties => .properties,
        else => null,
    };
}

/// Sniff `content` with `Language.detect`, emit an info-level log of what was
/// inferred, and return it — the fallback when neither `--input` nor the file
/// extension pinned the format. Errors (after a clear message) if nothing matches.
pub fn resolveFormatFromContent(allocator: std.mem.Allocator, content: []const u8, file_path: []const u8) !Format {
    const detected = fig.Language.detect(allocator, content) orelse {
        std.log.scoped(.detect).err("could not infer the format of `{s}` from its contents; pass an explicit format", .{file_path});
        return error.UnsupportedFileFormat;
    };
    const format = mapDetected(detected);
    std.log.scoped(.detect).info("inferred format `{s}` for `{s}` from its contents", .{ @tagName(format), file_path });
    return format;
}

/// Open `file_path` read-only and sniff its contents. For the in-place edit paths
/// (`edit`/`comment`), which then re-open the file read-write to splice it — so
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
/// known — an unknown/inapplicable version is reported like a parse error. When
/// the extension implies an embedded region (e.g. markdown frontmatter) the
/// inner document is extracted and parsed. Any IO/parse/spec error propagates to
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

    var format: Format = undefined;
    var embed: ?fig.Embed.Type = null;
    if (override) |f| {
        // An explicit format is taken at face value: no extension-driven embed
        // extraction, so `--input yaml file.md` parses the whole file as YAML.
        format = f;
    } else if (args_mod.detectLanguageFromFileEnding(file)) |d| {
        format = d.format;
        embed = args_mod.resolveEmbedTypeFromContent(content, null, d.embed_detect);
    } else {
        format = try resolveFormatFromContent(allocator, content, file);
    }

    // Resolve `--spec` against the now-known format. This rejects a nonsense
    // version (e.g. `--spec 1.0` on a JSON or markdown file) before parsing.
    const spec = try resolveSpec(format, spec_str);

    // `Embed.extract` parses the inner region; `parseSliceAs` parses the whole
    // file. Either surfaces a parse error — all we need to validate. The parsed
    // result is discarded; we only care that it parsed. (Embed extraction uses
    // the inner format's default version; `spec` was still validated above.)
    if (embed) |embed_type| {
        _ = try fig.Embed.extract(allocator, content, embed_type);
    } else {
        diag_source.* = content;
        var reports: Reports = .{};
        // `recover` so a file reports EVERY error in one pass (a language
        // server squiggles them all; `check` shouldn't hide errors 2..N behind
        // the first). Formats without a report yet stop at their first error
        // and fall back to the generic `file: ErrorName` line below.
        _ = parseSliceAs(format, spec, allocator, content, true, &reports) catch |err| {
            diag_errors.* = try reports.renderErrors(allocator);
            return err;
        };
        diag_warnings.* = try reports.renderWarnings(allocator);
    }
    return format;
}

test "resolveSpec maps YAML version strings" {
    // Every assertion below names a YAML dialect, which a `-Dyaml=false` build
    // has no enum to spell — the version STRINGS survive gating (the registry
    // still lists them), but `resolveSpec` refuses them, so there is nothing
    // here left to check.
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    // Default (no --spec) yields each language's default; YAML default is 1.2.2.
    try t.expectEqual(fig.Language.YAML.default_type, (try resolveSpec(.yaml, null)).yaml);
    try t.expectEqual(@as(fig.Language.YAML.Type, .v1_2_2), (try resolveSpec(.yaml, "1.2.2")).yaml);
    try t.expectEqual(@as(fig.Language.YAML.Type, .v1_2_2), (try resolveSpec(.yaml, "1.2")).yaml);
    // 1.1 is now selectable (was previously rejected as UnsupportedSpec).
    try t.expectEqual(@as(fig.Language.YAML.Type, .v1_1), (try resolveSpec(.yaml, "1.1")).yaml);
    try t.expectEqual(@as(fig.Language.YAML.Type, .v1_1), (try resolveSpec(.yaml, "1.1.0")).yaml);
    // `--input yml --spec 1.1` still selects YAML 1.1. `yml` is no longer a
    // `Format` member of its own; `parseFormatName` collapses the spelling to
    // `.yaml` before anything downstream — including this — ever sees it.
    try t.expectEqual(
        @as(fig.Language.YAML.Type, .v1_1),
        (try resolveSpec(args_mod.parseFormatName("yml").?, "1.1")).yaml,
    );
    // Unknown YAML versions still error.
    try t.expectError(error.UnsupportedSpec, resolveSpec(.yaml, "1.3"));
    try t.expectError(error.UnsupportedSpec, resolveSpec(.yaml, "2"));
}
