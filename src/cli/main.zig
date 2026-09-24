//! Main entrypoint for `fig` CLI
//! Design:
//! fig <action> [action options] [--flags]
//!
//! This file is deliberately thin: process/terminal setup, `std.log` routing,
//! and `parseConfig`'s error-to-help mapping, then a dispatch switch straight
//! into `actions.zig`. Everything else (arg parsing, format/embed detection,
//! parse dispatch, in-place editing, reformat/convert, diagnostic rendering,
//! the `fig-<action>` handoff) lives in its own sibling module — see each
//! file's own doc comment.

const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");
const Io = std.Io;

const types = @import("types.zig");
const help = @import("help.zig");
const args_mod = @import("args.zig");
const actions = @import("actions.zig");
const value_arg = @import("value_arg.zig");
const languages = @import("languages.zig");

// CLI-only sibling modules pulled in only through `actions.zig`/`args.zig`'s
// imports; referenced again in the `test {}` block at the bottom of this file
// so every leaf module's tests are guaranteed to land in the `exe_tests`
// binary regardless of Zig's lazy per-decl analysis.
const gron = @import("gron.zig");
const diff = @import("diff.zig");
const fileio = @import("fileio.zig");
const diag_report = @import("diag_report.zig");
const parse_dispatch = @import("parse_dispatch.zig");
const edit_ops = @import("edit_ops.zig");
const patch_ops = @import("patch_ops.zig");
const reformat = @import("reformat.zig");
const external = @import("external.zig");

const Help = help.Help;
const ArgError = types.ArgError;

// Logging for the CLI binary. `std.log`'s default handler (`std.log.defaultLog`)
// writes to stderr through `std.Options.debug_io` — a statically initialized,
// globally-shared `Io.Threaded` singleton, deliberately independent from the
// application's own `Io` instance (see the doc comment on `debug_io`). That
// means it opens its own positional `Io.File.Writer` over fd 2, separate from
// `stderr_terminal` below, each tracking its own `pos` from 0. When stderr is a
// regular file (redirected to disk rather than a tty), both writers do
// `pwrite`-style positional writes, so whichever one flushes second overwrites
// bytes the other already wrote instead of appending after them — corrupting
// the output. (Interleaving on a tty is harmless because tty writes are
// non-positional appends; the corruption only bites on redirection, which is
// why this was easy to miss.) Route `std.log` through `stderr_terminal` once
// `main` has constructed it, so there is only ever one `Io.File.Writer`/one
// `pos` counter over stderr.
//
// That is half of the hazard. The other half is between PROCESSES rather than
// within one — a standard stream's offset is the shell's, shared with every
// sibling command on the same redirection — and it is why the two writers
// below are built through `fileio.stdioWriter`, which explains it in full.
pub const std_options: std.Options = .{ .logFn = logFn };

/// Set by `main` right after `stderr_terminal` is constructed. `null` before
/// that point (there are no `std.log` call sites that early), in which case we
/// fall back to the stdlib default.
var g_log_terminal: ?*Io.Terminal = null;

///
/// What reaches a user is written in the CLI's own voice — `error: …`,
/// `warning: …`, `note: …`, coloured like every diagnostic `diag_report`
/// prints — not the stdlib's `error(parseConfig): …`, whose scope is the name
/// of the function that logged it. A format's trailing newline (several
/// `parseConfig` messages carry one) is dropped so each message is one line.
/// Debug lines keep the stdlib's `debug(scope):` shape: they exist only in a
/// Debug build, for whoever is developing fig, and the scope is what they
/// are for.
fn logFn(
    comptime level: std.log.Level,
    comptime scope: @EnumLiteral(),
    comptime format: []const u8,
    args: anytype,
) void {
    const t = g_log_terminal orelse return std.log.defaultLog(level, scope, format, args);
    // `std.log.defaultLog` flushes before returning (its `unlockStderr` does
    // so implicitly); match that so a log line right before `process.exit`
    // isn't lost sitting in `stderr_terminal`'s buffer.
    defer t.writer.flush() catch {};
    const label: []const u8, const color: Io.Terminal.Color = switch (level) {
        .debug => return std.log.defaultLogFileTerminal(level, scope, format, args, t.*) catch {},
        .err => .{ "error", .red },
        .warn => .{ "warning", .yellow },
        .info => .{ "note", .blue },
    };
    const body = comptime std.mem.trimEnd(u8, format, "\n");
    t.setColor(color) catch {};
    t.writer.writeAll(label) catch return;
    t.setColor(.reset) catch {};
    t.writer.print(": " ++ body ++ "\n", args) catch return;
}

// The core library's version — the same numbers `fig_version` exposes over
// the C ABI — sourced from `build.zig`'s `version` (parsed from
// build.zig.zon). Independent of `cli_version` below; see
// docs/VERSIONING.md's "Independent versioning" section for why the CLI and
// the core it embeds move on separate SemVer tracks.
const core_version = std.fmt.comptimePrint("{d}.{d}.{d}", .{
    build_options.version_major,
    build_options.version_minor,
    build_options.version_patch,
});

// The CLI binary's OWN version (`cli_version` in build.zig) — its
// compatibility contract is flags/defaults/exit codes, not the library API,
// so it moves independently of `core_version` above (only ever floored by
// it — see `zig build version-floor`).
const cli_version = std.fmt.comptimePrint("{d}.{d}.{d}", .{
    build_options.cli_version_major,
    build_options.cli_version_minor,
    build_options.cli_version_patch,
});

// The current marketing epoch (`epoch` in build.zig) — purely cosmetic, no
// compatibility meaning; see `fig version`'s output.
const epoch = build_options.epoch;

pub fn main(init: std.process.Init) !void {
    // Respected environment variables
    const NO_COLOR = init.environ_map.contains("NO_COLOR");
    const CLICOLOR_FORCE = init.environ_map.contains("CLICOLOR_FORCE");

    // Setting up arena allocator, io, terminal/stderr writer
    const io = init.io;
    const stderr_color_mode = try Io.Terminal.Mode.detect(io, Io.File.stderr(), NO_COLOR, CLICOLOR_FORCE);
    const stdout_color_mode = try Io.Terminal.Mode.detect(io, Io.File.stdout(), NO_COLOR, CLICOLOR_FORCE);
    var stdout_buf: [512]u8 = undefined;
    var stderr_buf: [512]u8 = undefined;
    var stdout = fileio.stdioWriter(Io.File.stdout(), io, &stdout_buf);
    var stderr = fileio.stdioWriter(Io.File.stderr(), io, &stderr_buf);
    var stderr_terminal = std.Io.Terminal{ .writer = &stderr.interface, .mode = stderr_color_mode };
    var stdout_terminal = std.Io.Terminal{ .writer = &stdout.interface, .mode = stdout_color_mode };
    // From here on, route `std.log` through this same writer (see `logFn`) so
    // it can't clobber `stderr_terminal`'s bytes when stderr is redirected.
    g_log_terminal = &stderr_terminal;

    // The languages the CLI did not compile in are resolved while arguments
    // are parsed (`--input <name>`, an extension, `--lang`), and resolving
    // one spawns its helper, so the module that does it needs `io` first.
    languages.init(io, init.arena.allocator(), init.environ_map);

    // Accessing command line arguments:
    var args = try init.minimal.args.iterateAllocator(init.gpa);
    defer args.deinit();

    const config = args_mod.parseConfig(init.arena.allocator(), &args) catch |err| switch (err) {
        ArgError.UnsupportedFileFormat => {
            try stderr_terminal.writer.print("Try using `--input <format>` to manually specify a format.\n", .{});
            comptime var supported_formats: []const u8 = "";
            inline for (@typeInfo(types.Format).@"enum".fields) |field|
                supported_formats = supported_formats ++ std.fmt.comptimePrint("\n- {s}", .{field.name});
            try stderr_terminal.writer.print("Supported formats:{s}\n", .{supported_formats});
            // And the languages a `languages.figl` configures, by name only —
            // listing is not the moment to spawn each one.
            const configured = languages.configured();
            if (configured.len > 0) {
                try stderr_terminal.writer.writeAll("Configured languages (`fig lang list`):");
                for (configured) |c| try stderr_terminal.writer.print("\n- {s}", .{c.name});
                try stderr_terminal.writer.writeAll("\n");
            }
            try stderr_terminal.writer.flush();
            std.process.exit(2);
        },
        ArgError.MissingEditArgument => {
            try Help.edit(&stderr_terminal, "fig");
            std.process.exit(2);
        },
        ArgError.MissingSetArgument => {
            try Help.set(&stderr_terminal, "fig");
            std.process.exit(2);
        },
        ArgError.MissingInsertArgument => {
            try Help.insert(&stderr_terminal, "fig");
            std.process.exit(2);
        },
        ArgError.MissingDeleteArgument => {
            try Help.delete(&stderr_terminal, "fig");
            std.process.exit(2);
        },
        ArgError.MissingGetArgument => {
            try Help.get(&stderr_terminal, "fig");
            std.process.exit(2);
        },
        ArgError.MissingCommentArgument => {
            try Help.comment(&stderr_terminal, "fig");
            std.process.exit(2);
        },
        ArgError.MissingCheckArgument => {
            try Help.check(&stderr_terminal, "fig");
            std.process.exit(2);
        },
        ArgError.MissingFmtArgument => {
            try Help.fmt(&stderr_terminal, "fig");
            std.process.exit(2);
        },
        ArgError.MissingConvertArgument => {
            try Help.convert(&stderr_terminal, "fig");
            std.process.exit(2);
        },
        ArgError.MissingPatchArgument => {
            try Help.patch(&stderr_terminal, "fig");
            std.process.exit(2);
        },
        ArgError.MissingLangArgument => {
            try Help.lang(&stderr_terminal, "fig");
            std.process.exit(2);
        },
        // Said what was wrong with it already (`args.pathArg`).
        ArgError.InvalidPath => std.process.exit(2),
        else => return err,
    };

    const a = init.arena.allocator();

    // Now, act on config. One failure is intercepted here rather than left to
    // escape as a bare `error: <ErrorName>`: an edit whose spliced TEXT is
    // what doesn't parse (`fig edit c.toml dep.rev cc5e7e51` — a git sha the
    // TOML parser can only read as a malformed number). `edit_ops.applyEdit`
    // tags that case; this is the only place that knows which argument the
    // text came from, so it is where the report is written. See
    // `diag_report.reportBadEditText`.
    dispatch(a, io, &stdout_terminal, &stderr_terminal, config) catch |err| switch (err) {
        error.InvalidEditText => {
            const spliced = types.splicedText(config).?;
            diag_report.reportBadEditText(&stderr_terminal, spliced.file, spliced.format, spliced.kind, spliced.text, edit_ops.refused_rendering);
        },
        // The document took `0` no better than the value, so the KEY is what
        // it refused (`edit_ops.applyValueEdit`): the user typed it wrong.
        error.InvalidEditKey => {
            const spliced = types.splicedText(config).?;
            diag_report.reportBadEditText(&stderr_terminal, spliced.file, spliced.format, .key, types.createdKey(config), null);
        },
        // A value argument the file's format cannot hold, from rendering it
        // (`value_arg.render`, which names these so a parse error the file
        // raises never lands here); nothing was written.
        error.UnwritableNull, error.UnwritableNested, error.UnwritableKey => diag_report.reportUnwritableValue(&stderr_terminal, types.targetFile(config) orelse "the file", err),
        // Everything an action didn't report itself stops HERE with a plain
        // line, instead of escaping to the Zig runtime's default handler.
        // Letting it escape printed a bare `error: <ErrorName>` plus an
        // unreadable stack trace through the runtime's OWN stderr writer —
        // which, sharing fd 2 positionally with `stderr_terminal`, clobbers
        // whatever this process already wrote (the same hazard documented for
        // `std.log` above, and in `diag_report`). The most common one by far
        // is a parse failure on the target file itself, which is reported
        // where it is, below, whenever its format can say where.
        else => {
            // An action that parses its file without a report (the in-place
            // editors) lets a parse failure escape as a bare error name.
            // Parse the file again the way `check` does, and when that
            // locates the failure, the located report IS the answer — it
            // replaces the error name rather than pointing the user at a
            // second command.
            if (types.parseTarget(config)) |t|
                if (reportParseFailure(a, io, &stderr_terminal, t)) std.process.exit(1);
            diag_report.reportUnhandled(&stderr_terminal, err, types.targetFile(config), config.binary_name);
        },
    };
}

/// Re-parse `t.file` as `check` would and print every located parse error it
/// finds. True when there was at least one to print; false when the file
/// parses, cannot be read, or fails in a format with no located report — in
/// which case the caller's generic report is all there is to say.
fn reportParseFailure(a: std.mem.Allocator, io: std.Io, term: *std.Io.Terminal, t: types.ParseTarget) bool {
    var source: ?[]const u8 = null;
    var errors: ?[]const fig.ParseDiagnostic.Rendered = null;
    var warnings: ?[]const fig.ParseDiagnostic.Rendered = null;
    _ = parse_dispatch.checkOne(a, io, t.file, t.format, null, &source, &errors, &warnings) catch {
        const errs = errors orelse return false;
        for (errs) |d| diag_report.printDiag(term, source.?, t.file, d.offset, d.end, "error", .red, d.message, d.short_label) catch return true;
        term.writer.flush() catch {};
        return true;
    };
    return false;
}

fn dispatch(a: std.mem.Allocator, io: Io, stdout_terminal: *Io.Terminal, stderr_terminal: *Io.Terminal, config: types.CliConfig) !void {
    return switch (config.action) {
        .help => actions.runHelp(stderr_terminal, config.binary_name),
        .version => actions.runVersion(stdout_terminal, cli_version, core_version, epoch),
        .edit => actions.runEdit(a, io, stdout_terminal, stderr_terminal, config.binary_name, config.options.edit),
        .set => actions.runSet(a, io, stdout_terminal, stderr_terminal, config.binary_name, config.options.set),
        .insert => actions.runInsert(a, io, stdout_terminal, stderr_terminal, config.binary_name, config.options.insert),
        .delete => actions.runDelete(a, io, stdout_terminal, stderr_terminal, config.binary_name, config.options.delete),
        .get => actions.runGet(a, io, stdout_terminal, stderr_terminal, config.binary_name, config.options.get),
        .comment => actions.runComment(a, io, stdout_terminal, stderr_terminal, config.binary_name, config.options.comment),
        .check => actions.runCheck(a, io, stdout_terminal, stderr_terminal, config.binary_name, config.options.check),
        .fmt => actions.runFmt(a, io, stdout_terminal, stderr_terminal, config.binary_name, config.options.fmt),
        .convert => actions.runConvert(a, io, stdout_terminal, stderr_terminal, config.binary_name, config.options.convert),
        .patch => actions.runPatch(a, io, stdout_terminal, stderr_terminal, config.binary_name, config.options.patch),
        .lang => actions.runLang(a, io, stdout_terminal, stderr_terminal, config.binary_name, config.options.lang),
        .external => actions.runExternal(io, stdout_terminal, stderr_terminal, config.binary_name, config.options.external),
    };
}

test "logFn writes a user-facing line in the CLI's voice, not std.log's `level(scope):`" {
    var out: Io.Writer.Allocating = .init(std.testing.allocator);
    defer out.deinit();
    var term: Io.Terminal = .{ .writer = &out.writer, .mode = .no_color };
    const saved = g_log_terminal;
    g_log_terminal = &term;
    defer g_log_terminal = saved;

    // A trailing newline in the format (as `parseConfig`'s carry) still
    // makes one line, not a line and a blank one.
    logFn(.err, .parseConfig, "No path provided.\n", .{});
    logFn(.warn, .languages, "could not read {s}", .{"x.figl"});
    logFn(.info, .detect, "a {s}", .{"notice"});
    try std.testing.expectEqualStrings(
        "error: No path provided.\nwarning: could not read x.figl\nnote: a notice\n",
        out.written(),
    );
}

// Pull every CLI-only leaf module's tests into the exe test binary. `gron`/
// `diff` are CLI-only formats that live here in the binary, never in the
// `fig` library, so `root.zig`'s test graph never reaches them; the rest are
// this binary's own split-out modules.
test {
    _ = types;
    _ = help;
    _ = fileio;
    _ = diag_report;
    _ = parse_dispatch;
    _ = edit_ops;
    _ = patch_ops;
    _ = reformat;
    _ = external;
    _ = args_mod;
    _ = actions;
    _ = gron;
    _ = diff;
    _ = value_arg;
}
