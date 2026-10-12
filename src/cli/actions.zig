//! The body of each CLI action — one function per `CliAction`, called from
//! `main.zig`'s thin dispatch switch. Each function takes the arena
//! allocator, the process `Io`, the stdout/stderr terminals, and its own
//! action's option struct (from `types.zig`); together they're exactly the
//! free variables the original monolithic `main()` switch closed over.
const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");

const gron = @import("gron.zig");
const diff = @import("diff.zig");
const types = @import("types.zig");
const help = @import("help.zig");
const args_mod = @import("args.zig");
const fileio = @import("fileio.zig");
const diag_report = @import("diag_report.zig");
const parse_dispatch = @import("parse_dispatch.zig");
const edit_ops = @import("edit_ops.zig");
const value_arg = @import("value_arg.zig");
const patch_ops = @import("patch_ops.zig");
const reformat = @import("reformat.zig");
const languages = @import("languages.zig");
const external = @import("external.zig");

const Help = help.Help;
const Format = types.Format;
const EditOp = types.EditOp;
const append_index = types.append_index;
const Io = std.Io;

pub fn runHelp(stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.HelpOptions) !void {
    if (opts.oneline) {
        try stdout_term.writer.writeAll(help.oneline);
        try stdout_term.writer.flush();
        return;
    }
    try stderr_term.writer.print(help.title_string, .{});
    try Help.general(stderr_term, binary_name);
}

/// The git-style fallback for a word that is not one of fig's own actions:
/// hand it to `fig-<word>` on PATH. All of it lives in `external.zig`; this
/// exists so that `main`'s dispatch switch stays one arm per action, straight
/// into this file.
pub fn runExternal(io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.ExternalOptions) !void {
    return external.run(io, stdout_term, stderr_term, binary_name, opts);
}

/// Print fig's one version — every artifact ships under it (see
/// docs/VERSIONING.md) — and its marketing epoch, a purely cosmetic label with
/// no compatibility meaning: `fig 5.0.0 "Texas Everbearing"`.
pub fn runVersion(stdout_term: *Io.Terminal, binary_name: []const u8, version: []const u8, epoch: []const u8, opts: types.VersionOptions) !void {
    if (opts.requested_help) {
        try Help.version(stdout_term, binary_name);
        return;
    }
    try stdout_term.writer.print("fig {s} \"{s}\"\n", .{ version, epoch });
    try stdout_term.writer.flush();
}

pub fn runEdit(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.EditOptions) !void {
    if (opts.requested_help) {
        try if (opts.key) Help.rename(stdout_term, binary_name) else Help.replace(stdout_term, binary_name);
        return;
    }
    // A replacement VALUE is read before the file is touched, so one that is
    // not a value exits 2 with the file as it was; a replacement key is a
    // name, spelled by the editor.
    const value: edit_ops.OpValue = if (opts.key) .{} else .{ .one = try value_arg.read(a, stderr_term, opts.replacement, opts.value_mode) };
    const input = try fileio.getInput(io, opts.file, .read_write);
    defer if (!std.mem.eql(u8, opts.file, "-")) input.close(io);

    const op: EditOp = if (opts.key) .replace_key else .replace_value;
    // Value/key replacement is the same span splice for every editable format
    // — a value or key node has a tight, contiguous span, so the generic
    // editor handles even a TOML table assembled from scattered headers — so
    // it routes through the shared editor dispatch like every other edit. That
    // is also where the value is spelled in the file's format, and where the
    // refusal of a read-only format and the canonical/gron refusals live; see
    // `edit_ops.route`.
    const resolved = if (opts.detect) try parse_dispatch.detectFileFormat(io, a, opts.file) else opts.format;
    try edit_ops.applyToFileAs(a, io, input, resolved, opts.path, opts.replacement, value, op);
}

pub fn runSet(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.SetOptions) !void {
    if (opts.requested_help) {
        try Help.set(stdout_term, binary_name);
        return;
    }
    if (opts.path.len == 0) {
        try stderr_term.writer.print("error: set needs a path to the key (or, with --seq, the sequence) to upsert.\n", .{});
        try stderr_term.writer.flush();
        std.process.exit(2);
    }
    // Read before any file is created or opened: see `runEdit`.
    const value: edit_ops.OpValue = if (opts.seq) blk: {
        const items = try a.alloc(value_arg.Value, opts.values.len);
        for (opts.values, items) |text, *item| item.* = try value_arg.read(a, stderr_term, text, opts.value_mode);
        break :blk .{ .items = items };
    } else .{ .one = try value_arg.read(a, stderr_term, opts.value, opts.value_mode) };
    // `set` upserts, so it may target a file that doesn't exist yet:
    // create it and seed a minimal valid empty document (see
    // `createSeededFile`/`emptyDocSeed`) so the editor lands the first
    // key into a parseable buffer. The other structural actions keep the
    // plain open — they edit what's already there and have nothing to
    // seed into a blank file. Two cases refuse the create up front,
    // before any file lands on disk:
    //   - a freshly created file is empty, so its format can't be sniffed
    //     (`--detect`): it must carry a known extension;
    //   - a format with no empty-document form (`emptyDocSeed` == null,
    //     e.g. fig) has nothing valid to seed.
    var created = false;
    const input = fileio.getInput(io, opts.file, .read_write) catch |err| switch (err) {
        error.FileNotFound => blk: {
            if (std.mem.eql(u8, opts.file, "-")) return err;
            if (opts.detect) {
                try stderr_term.writer.print("error: cannot create {s}: an unrecognized extension gives no format to seed. Use a known extension (.json/.jsonc/.yaml/.toml/.zon) or an existing file.\n", .{opts.file});
                try stderr_term.writer.flush();
                std.process.exit(2);
            }
            const seed: []const u8 = edit_ops.emptyDocSeed(opts.format) orelse {
                try stderr_term.writer.print("error: cannot create {s}: {s} has no empty-document form to seed a new file. Start from an existing file.\n", .{ opts.file, types.name(opts.format) });
                try stderr_term.writer.flush();
                std.process.exit(2);
            };
            created = true;
            break :blk try fileio.createSeededFile(io, opts.file, seed);
        },
        else => return err,
    };
    defer if (!std.mem.eql(u8, opts.file, "-")) input.close(io);

    const resolved = if (opts.detect) try parse_dispatch.detectFileFormat(io, a, opts.file) else opts.format;
    // `--seq` reconciles the sequence at `path`; otherwise upsert a scalar.
    // Both flow through the shared structural-edit router.
    // A path ending in an index upserts a sequence item: `[-]` appends, the
    // way it does for `insert`, and an index that is there is replaced.
    // An index past the end is refused below, since there is no item
    // between the last and it to fill.
    var path = opts.path;
    const item: ?usize = if (opts.seq) null else switch (path[path.len - 1]) {
        .index => |index| index,
        .key => null,
    };
    const op: EditOp = if (opts.seq)
        .{ .set_sequence = opts.values }
    else if (item == append_index) blk: {
        path = path[0 .. path.len - 1];
        break :blk .append_seq;
    } else if (item != null) .replace_value else .set;
    const text: []const u8 = if (opts.seq) "" else opts.value;
    // On a from-scratch create, roll the new file back if the edit fails
    // (e.g. a path whose parent segment resolves to a scalar, which `set`
    // refuses rather than clobber) so a failed `set` leaves no bare seed
    // behind — matching how it leaves an existing file untouched on
    // failure. A merely-missing parent map is no longer a failure: `set`
    // auto-vivifies it (see `Editor.set`).
    edit_ops.applyToFileAs(a, io, input, resolved, path, text, value, op) catch |err| {
        if (created) fileio.deleteCreatedFile(io, opts.file);
        if (err == error.NotFound and op == .replace_value) {
            try stderr_term.writer.print("error: there is no item [{d}] to set, so nothing was written; set replaces an item that is there, or appends one with [-].\n", .{item.?});
            try stderr_term.writer.flush();
            std.process.exit(1);
        }
        // `set` makes missing maps but not a missing list, so an append to
        // one has nothing to land in; say how to start it, rather than the
        // "no such path" or "not a mapping" the lookup gives.
        if ((err == error.NotFound or err == error.NotAMapping) and op == .append_seq) {
            try stderr_term.writer.print("error: there is no list to append to, so nothing was written; start one with `{s} set {s} <path> '[<value>]'`.\n", .{ binary_name, opts.file });
            try stderr_term.writer.flush();
            std.process.exit(1);
        }
        return err;
    };
}

pub fn runInsert(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.InsertOptions) !void {
    if (opts.requested_help) {
        try Help.insert(stdout_term, binary_name);
        return;
    }
    // Read before the file is opened: see `runEdit`.
    const value: edit_ops.OpValue = .{ .one = try value_arg.read(a, stderr_term, opts.value, opts.value_mode) };
    const input = try fileio.getInput(io, opts.file, .read_write);
    defer if (!std.mem.eql(u8, opts.file, "-")) input.close(io);

    // The destination is the *parent* container plus the trailing slot.
    // A trailing key inserts into a mapping; a trailing index pre/appends
    // to a sequence. An empty parent is the root container.
    if (opts.path.len == 0) {
        try stderr_term.writer.print("error: insert needs a destination path (e.g. a.b.newkey or list[-]).\n", .{});
        try stderr_term.writer.flush();
        std.process.exit(2);
    }
    const parent = opts.path[0 .. opts.path.len - 1];
    const resolved = if (opts.detect) try parse_dispatch.detectFileFormat(io, a, opts.file) else opts.format;
    switch (opts.path[opts.path.len - 1]) {
        .key => |key| try edit_ops.applyToFileAs(a, io, input, resolved, parent, opts.value, value, .{ .insert_key = key }),
        .index => |index| {
            // `[0]` prepends, `[-]`/`[$]` appends, and any other index
            // inserts before the item there; past the end is the editor's
            // `NotFound`, reported as every refused edit is.
            const op: EditOp = if (index == 0)
                .prepend_seq
            else if (index == append_index)
                .append_seq
            else
                .{ .insert_seq_item = index };
            try edit_ops.applyToFileAs(a, io, input, resolved, parent, opts.value, value, op);
        },
    }
}

pub fn runDelete(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.DeleteOptions) !void {
    if (opts.requested_help) {
        try Help.delete(stdout_term, binary_name);
        return;
    }
    const input = try fileio.getInput(io, opts.file, .read_write);
    defer if (!std.mem.eql(u8, opts.file, "-")) input.close(io);

    if (opts.path.len == 0) {
        try stderr_term.writer.print("error: delete needs a path to the entry or item to remove.\n", .{});
        try stderr_term.writer.flush();
        std.process.exit(2);
    }
    const resolved = if (opts.detect) try parse_dispatch.detectFileFormat(io, a, opts.file) else opts.format;
    // A trailing index removes that item from the parent sequence; a
    // trailing key deletes the mapping entry named by the full path.
    switch (opts.path[opts.path.len - 1]) {
        .index => |index| try edit_ops.applyToFileAs(a, io, input, resolved, opts.path[0 .. opts.path.len - 1], "", .{}, .{ .remove_seq_item = index }),
        .key => try edit_ops.applyToFileAs(a, io, input, resolved, opts.path, "", .{}, .delete_key),
    }
}

pub fn runGet(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.GetOptions) !void {
    if (opts.requested_help) {
        try Help.get(stdout_term, binary_name);
        return;
    }
    const input = try fileio.getInput(io, opts.file, .read_only);
    defer if (!std.mem.eql(u8, opts.file, "-")) input.close(io);

    // Resolved input/output formats. They equal the parsed options unless
    // the input format has to be sniffed from the file's contents (no
    // `--input`, unrecognized extension): detection overwrites `from`, and
    // — when no `--output` was given — `to` follows it (an echo round-trip
    // rather than a silent convert-to-JSON).
    var from = opts.from;
    var to = opts.to;

    const doc = blk: {
        // Read once so detection and parsing share the same bytes — a
        // piped stdin can only be consumed a single time.
        const content = try fileio.readAll(a, io, input);
        if (opts.detect) {
            from = try parse_dispatch.resolveFormatFromContent(a, content, opts.file);
            if (!opts.output_explicit) to = from;
        }
        var reports: parse_dispatch.Reports = .{};
        const parsed = parse_dispatch.parseSliceAs(from, a, content, &reports) catch |err| {
            // A parse failure renders as a `file:line:col` teaching
            // message (DESIGN.md: every diagnostic names the fix) and
            // exits cleanly — no error-return trace for a user typo.
            // Every language whose parser has a `parseWithReport` fills
            // one (see `parse_dispatch.reporting`); the rest fall through
            // to the bare `return err` and its generic error name.
            try reports.reportDiagnostics(stderr_term, content, opts.file);
            return err;
        };
        // Authoring-time lints (parse-time warnings) ride the same
        // `--quiet`/`--strict` contract as the serialize-side
        // diagnostics below: quiet silences, strict aborts.
        try reports.reportWarnings(stderr_term, content, opts.file, opts.quiet, opts.strict);
        break :blk parsed;
    };

    // Leaving a language with a reference layer for one without resolves
    // the layer first (aliases → copies, merges → flattened, tags
    // applied/dropped). YAML→YAML keeps it intact for round-trip; JSON
    // never has it. Each side is asked, not named (`Caps.references`).
    const keeps_references = parse_dispatch.carriesReferences(from) and parse_dispatch.carriesReferences(to);
    const base_ast = try parse_dispatch.materializeFor(a, stderr_term, from, to, &doc, opts.file, if (opts.lax_tags) .lax else .strict);

    // Lossless mode: decode any `$fig` envelopes in the input back to
    // their real node kinds, then re-encode for the target format. Skipped
    // when the reference layer is kept (YAML→YAML): it lives in side-tables
    // the core-AST passes would strip — and it round-trips losslessly
    // already. The passes operate on a core AST, so any source without the
    // layer, or one materialized above, is safe.
    const ast: *const fig.AST = if (opts.lossless and !keeps_references) blk: {
        // gron is CLI-only — it has no `SerializeFormat` of its own, so no
        // `caps.lossless` — but its value layer is JSON, so it encodes for
        // JSON's declaration: an unrepresentable value (a TOML datetime,
        // etc.) rides in a `$fig` envelope that prints as a JSON object.
        // Every other format maps through its `SerializeFormat` counterpart,
        // whose language declares the answer (see `manifest.Caps.lossless`
        // for the rest of the rationale: JSON5 reuse, canonical/fig
        // decode-only, INI/dotenv/properties/plist/NestedText's lack of an
        // envelope of their own).
        const maybe_native: ?fig.Lossless.NativeKinds = parse_dispatch.nativeForFormat(to);
        const decoded = try a.create(fig.AST);
        decoded.* = try fig.Lossless.decode(a, base_ast);
        const native = maybe_native orelse break :blk decoded;
        const encoded = try a.create(fig.AST);
        encoded.* = try fig.Lossless.encode(a, decoded, native);
        break :blk encoded;
    } else base_ast;

    const node_id = if (opts.path) |p| (try ast.getValByPath(try edit_ops.resolveLast(a, ast, p))).id else ast.root;

    // A scalar at a path prints as its text and a newline, the way `jq -r`
    // does, whatever the file's format: a string unquoted, anything else as
    // fig spells it (`42`, `true`, `null`, a datetime as written). `-o`
    // asks for a format's own spelling of the fragment and keeps getting it
    // (`fig get f name -o json` → `"hi"`). CLI 5 §2.
    if (opts.path != null and !opts.output_explicit) {
        if (try scalarText(a, ast, node_id)) |text| {
            try stdout_term.writer.writeAll(text);
            try stdout_term.writer.writeByte('\n');
            try stdout_term.writer.flush();
            return;
        }
    }

    // gron is a CLI-only projection that derives straight from the AST,
    // so it has no `SerializeFormat`: print it here and return, bypassing
    // the serializer dispatch, the lossy/lossless diagnostics below, and
    // the C ABI entirely. Aliases are already materialized above.
    if (to == .gron) {
        if (comptime build_options.lang_json) {
            try gron.printNode(stdout_term.writer, ast, node_id, opts.gron_projection);
            try stdout_term.writer.flush();
        } else return error.FormatDisabled;
        return;
    }

    // Surface everything the conversion would silently lose (comments
    // dropped/degraded, values dropped/degraded) — unless `--quiet`;
    // `--strict` turns any warning into a hard failure. A runtime target is
    // held to what its declaration says it holds.
    const diag_target = types.toTarget(to) orelse unreachable; // gron returned above
    try parse_dispatch.reportLoss(a, stderr_term, ast, node_id, diag_target, opts.serialize, opts.lossless, opts.quiet, opts.strict);

    switch (diag_target) {
        // The oracle form holds every value; nothing to strip.
        .canonical => fig.Serialize.serializeNodeWith(ast, stdout_term.writer, .canonical, node_id, opts.serialize) catch |err|
            diag_report.reportSerializeError(stderr_term, err),
        // A language prints through its vtable, after the lossy strips its
        // declarations ask for. A scalar comes back as the fragment the
        // editor would splice — the value as it stands alone, no newline —
        // so `get` ends the line here.
        .language => |e| {
            var out: std.Io.Writer.Allocating = .init(a);
            defer out.deinit();
            parse_dispatch.printRuntime(a, &out.writer, e, ast, node_id, opts.serialize, opts.lossless) catch |err| switch (err) {
                error.OutOfMemory => return err,
                else => |x| diag_report.reportRuntimePrintError(stderr_term, x),
            };
            const text = out.written();
            try stdout_term.writer.writeAll(text);
            const is_container = switch (ast.nodes[node_id].kind) {
                .mapping, .sequence => true,
                else => false,
            };
            if (!is_container and text.len > 0 and text[text.len - 1] != '\n') try stdout_term.writer.writeByte('\n');
        },
    }
    try stdout_term.writer.flush();
}

/// The text `get` prints for the scalar at `id`, or null for a container
/// (and for an unresolved alias, which prints as the format writes it).
fn scalarText(a: std.mem.Allocator, ast: *const fig.AST, id: fig.AST.Node.Id) !?[]const u8 {
    switch (ast.nodes[id].kind) {
        .string => |s| return s,
        .null_, .boolean, .number, .extended => {},
        .sequence, .mapping, .keyvalue, .alias => return null,
    }
    // fig's spelling; JSON's otherwise, which agrees for every scalar but
    // a datetime (a JSON string, unquoted here).
    const has_fig = args_mod.parseFormatName("figl") != null;
    const target = fig.Serialize.Target.named(if (has_fig) "figl" else "json") orelse return error.FormatDisabled;
    var view = ast.*;
    view.root = id;
    var w: std.Io.Writer.Allocating = .init(a);
    try fig.Serialize.serializeFragmentWith(&view, &w.writer, target, .{});
    const text = std.mem.trimEnd(u8, w.written(), "\n");
    if (!has_fig) if (ast.nodes[id].kind == .extended and text.len > 0 and text[0] == '"') return ast.nodes[id].kind.extended.text;
    return text;
}

pub fn runComment(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.CommentOptions) !void {
    if (opts.requested_help) {
        try Help.comment(stdout_term, binary_name);
        return;
    }
    // `--get` only reads: open read-only and never write back.
    const input = try fileio.getInput(io, opts.file, if (opts.get) .read_only else .read_write);
    defer if (!std.mem.eql(u8, opts.file, "-")) input.close(io);

    const resolved = if (opts.detect) try parse_dispatch.detectFileFormat(io, a, opts.file) else opts.format;

    if (opts.get) {
        const comment = blk: {
            // Strict JSON has no comment syntax: there can be nothing to get.
            // The one refusal the shared dispatch can't state, because it is
            // about a DIALECT of an otherwise comment-carrying language — and
            // it is a message-and-exit(1), not an error value: the
            // document's format refuses, as it would a missing path.
            if (types.is(resolved, "json")) {
                try stderr_term.writer.print("error: strict JSON has no comments; use a .jsonc or .json5 file instead.\n", .{});
                try stderr_term.writer.flush();
                std.process.exit(1);
            }
            // Everything else reads its comment through the shared editor
            // dispatch. A format whose grammar has no same-line comment (INI,
            // NestedText) still answers a leading-comment read and surfaces
            // `error.CommentsUnsupported` from the editor under `--inline`.
            break :blk try edit_ops.getCommentAs(a, io, input, resolved, opts.path, opts.inline_comment);
        };
        // A missing comment is a missing path: nothing on stdout and exit 1,
        // the way `get` answers for a path that is not there, so "is there a
        // comment" is `if fig comment --get …`. A present-but-empty one
        // prints just its newline.
        const text = comment orelse {
            try stderr_term.writer.print("error: no {s}comment at that path\n", .{if (opts.inline_comment) "inline " else ""});
            try stderr_term.writer.flush();
            std.process.exit(1);
        };
        try stdout_term.writer.print("{s}\n", .{text});
        try stdout_term.writer.flush();
        return;
    }

    // Pick the op from the two flags: `--inline` selects the trailing
    // (same-line) comment vs the leading block; `--delete` removes it
    // rather than adding/setting. The marker (`#`, `//`) is the editor's
    // job.
    const op: EditOp = if (opts.delete)
        (if (opts.inline_comment) .delete_trailing_comment else .delete_leading_comments)
    else
        (if (opts.inline_comment) .set_trailing_comment else .add_leading_comment);

    // Strict JSON has no comment syntax: fail with a clear message
    // rather than letting the editor surface a bare error. The same
    // refusal, and the same reason, as the `--get` branch above.
    if (types.is(resolved, "json")) {
        try stderr_term.writer.print("error: strict JSON has no comments; use a .jsonc or .json5 file instead.\n", .{});
        try stderr_term.writer.flush();
        std.process.exit(1);
    }
    // Everything else goes through the shared editor dispatch — JSONC/
    // JSON5's `//` comments reparse under their own dialect there, and
    // `--inline` set/delete on a format with no same-line comment syntax
    // surfaces `error.CommentsUnsupported` from the editor.
    try edit_ops.applyToFileAs(a, io, input, resolved, opts.path, opts.text, .{}, op);
}

pub fn runCheck(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.CheckOptions) !void {
    if (opts.requested_help) {
        try Help.check(stdout_term, binary_name);
        return;
    }

    // Validate every file, reporting each independently, so one bad file
    // doesn't hide the status of the rest. Success lines go to stdout
    // (silenced by `--quiet`); failures always go to stderr. A single
    // bad file makes the whole run exit non-zero — the CI contract.
    var any_failed = false;
    for (opts.files) |file| {
        var diag_source: ?[]const u8 = null;
        var diag_errors: ?[]const fig.ParseDiagnostic.Rendered = null;
        var diag_warnings: ?[]const fig.ParseDiagnostic.Rendered = null;
        if (parse_dispatch.checkOne(a, io, file, opts.format, opts.spec, &diag_source, &diag_errors, &diag_warnings)) |fmt| {
            const warned = if (diag_warnings) |ws| ws.len else 0;
            if (opts.strict and warned > 0) {
                // `--strict`: the file parses, but a lint fails it like an
                // error would — each warning, then why the file is not `ok`.
                any_failed = true;
                if (!opts.quiet) for (diag_warnings.?) |w| try diag_report.printDiag(stderr_term, diag_source.?, file, w.offset, w.end, "warning", .yellow, w.message, w.short_label);
                try stderr_term.setColor(.red);
                try stderr_term.writer.writeAll("error");
                try stderr_term.setColor(.reset);
                try stderr_term.writer.print(": {s}: {d} warning(s); --strict fails it\n", .{ file, warned });
                try stderr_term.writer.flush();
                continue;
            }
            if (!opts.quiet) {
                try stdout_term.setColor(.green);
                try stdout_term.writer.writeAll("ok");
                try stdout_term.setColor(.reset);
                // Echo the pinned version alongside the format when one
                // was requested, so `ok` states exactly what was checked.
                if (opts.spec) |spec|
                    try stdout_term.writer.print(": {s} ({s} {s})\n", .{ file, types.name(fmt), spec })
                else
                    try stdout_term.writer.print(": {s} ({s})\n", .{ file, types.name(fmt) });
                // Authoring-time lints: the file is valid (still `ok`),
                // but likely-mistake lines print right below it. Rendered
                // live against the real terminal (not buffered into a
                // string — see `printDiag`), then flushed immediately so
                // it can't interleave with the next file's logging.
                if (diag_warnings) |ws| {
                    for (ws) |w| try diag_report.printDiag(stderr_term, diag_source.?, file, w.offset, w.end, "warning", .yellow, w.message, w.short_label);
                    try stderr_term.writer.flush();
                }
            }
        } else |err| {
            any_failed = true;
            // A covered-language failure renders as a full
            // `file:line:col: error: …` teaching report per error
            // (recovery collects every error in the file, not just the
            // first) instead of the generic `file: ErrorName` line.
            if (diag_errors) |errs| {
                for (errs) |d| try diag_report.printDiag(stderr_term, diag_source.?, file, d.offset, d.end, "error", .red, d.message, d.short_label);
                try stderr_term.writer.flush();
                continue;
            }
            try stderr_term.setColor(.red);
            try stderr_term.writer.writeAll("error");
            try stderr_term.setColor(.reset);
            switch (err) {
                // A spec mismatch is a CLI usage error, not a malformed
                // document — say so plainly with the offending version.
                error.UnsupportedSpec => try stderr_term.writer.print(
                    ": {s}: --spec '{s}' is not valid for this format\n",
                    .{ file, opts.spec.? },
                ),
                else => try stderr_term.writer.print(": {s}: {s}\n", .{ file, @errorName(err) }),
            }
        }
    }
    try stdout_term.writer.flush();
    try stderr_term.writer.flush();
    if (any_failed) std.process.exit(1);
}

/// `fig patch`: merge one document into another, in place.
///
/// The order below is not arbitrary. The TARGET is resolved first — read
/// and format-detected — because the patch document's own
/// preparation depends on it: `--lossless` encodes the patch's values for the
/// format they are about to be rendered into, which is the target's.
pub fn runPatch(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.PatchOptions) !void {
    if (opts.requested_help) {
        try Help.patch(stdout_term, binary_name);
        return;
    }
    // `--dry-run`/`--diff` are preview modes: nothing is written under either,
    // so the target opens read-only and stdin becomes a legal target.
    const preview_only = opts.dry_run or opts.diff;
    const target_is_stdin = std.mem.eql(u8, opts.file, "-");
    if (!preview_only and target_is_stdin) {
        try stderr_term.writer.print(
            "error: cannot patch stdin in place; pass --dry-run or --diff to print the result instead.\n",
            .{},
        );
        try stderr_term.writer.flush();
        std.process.exit(2);
    }

    const input = try fileio.getInput(io, opts.file, if (preview_only) .read_only else .read_write);
    defer if (!target_is_stdin) input.close(io);
    const content = try fileio.readAll(a, io, input);

    // Sniffed from the bytes when the extension didn't say.
    const target_format = if (opts.detect)
        try parse_dispatch.resolveFormatFromContent(a, content, opts.file)
    else
        opts.format;

    const patch = try loadPatch(a, io, stderr_term, opts, target_format);
    const req: patch_ops.Request = .{
        .at = opts.at,
        .patch = patch.ast,
        .from = patch.root,
        .deletes = opts.deletes,
        .options = opts.patch_options,
    };

    const patched = try patch_ops.applyToSlice(a, content, target_format, req);

    // The one thing a silent success would hide: trivia the patch carried that
    // the target has nowhere to put (strict JSON has no comment syntax at all).
    // Said before anything is written, so `--strict` can refuse the result.
    if (patched.stats.comments_dropped > 0) {
        if (!opts.quiet) try stderr_term.writer.print(
            "warning: dropped {d} comment(s) the patch carried — {s} has no place for them here.\n",
            .{ patched.stats.comments_dropped, opts.file },
        );
        if (opts.strict) {
            try stderr_term.writer.print("error: {s} was not patched; --strict aborts on a warning.\n", .{opts.file});
            try stderr_term.writer.flush();
            std.process.exit(1);
        }
        try stderr_term.writer.flush();
    }

    const changed = !std.mem.eql(u8, content, patched.content);
    if (opts.diff) {
        try diff.unifiedDiff(a, stdout_term.writer, opts.file, content, patched.content, 3);
        try stdout_term.writer.flush();
    } else if (opts.dry_run) {
        try stdout_term.writer.writeAll(patched.content);
        try stdout_term.writer.flush();
    } else if (changed) {
        // Read-then-splice-same-handle, as in `fmt`: `content` was read before
        // this write, so there is no truncate-before-read race.
        try input.writePositionalAll(io, patched.content, 0);
        try input.setLength(io, patched.content.len);
    }
}

/// Read, parse and prepare the patch document: the subtree `--from` names, in
/// a form that can be rendered into `target_format` without dangling.
///
/// Two preparation passes, both conditional and both about references rather
/// than values:
///
///   * A YAML patch that declares anchors is materialized (aliases → copies,
///     `<<` merges → flattened). Its reference layer is defined in the PATCH
///     file, which the target has never seen, so a `*name` spliced across
///     verbatim would point at nothing. Skipped when there are no anchors,
///     which is when materializing could only lose things (custom tags) and
///     gain nothing.
///   * `--lossless` decodes any `$fig` envelope the patch carries, then
///     re-encodes for the target's own type system — the same symmetric pair
///     `get`/`convert` run, and for the same reason.
fn loadPatch(
    a: std.mem.Allocator,
    io: Io,
    stderr_term: *Io.Terminal,
    opts: types.PatchOptions,
    target_format: Format,
) !struct { ast: *const fig.AST, root: fig.AST.Node.Id } {
    const is_stdin = std.mem.eql(u8, opts.patch_file, "-");
    const file = try fileio.getInput(io, opts.patch_file, .read_only);
    defer if (!is_stdin) file.close(io);
    const content = try fileio.readAll(a, io, file);

    const format = if (opts.detect_patch)
        try parse_dispatch.resolveFormatFromContent(a, content, opts.patch_file)
    else
        opts.patch_format;

    var reports: parse_dispatch.Reports = .{};
    // In the arena, not on this frame: the AST outlives `loadPatch` — it is
    // what the merge reads all the way through — so a `&doc.ast` into a local
    // would dangle the moment this returns.
    const doc = try a.create(fig.Document);
    doc.* = parse_dispatch.parseSliceAs(format, a, content, &reports) catch |err| {
        // Same contract as `get`'s: a parse failure in a file the user named
        // renders as a `file:line:col` teaching message, not an error name.
        try reports.reportDiagnostics(stderr_term, content, opts.patch_file);
        return err;
    };
    try reports.reportWarnings(stderr_term, content, opts.patch_file, opts.quiet, opts.strict);

    var ast: *const fig.AST = &doc.ast;
    if (doc.ast.anchors.len > 0) {
        // A patch must not carry unresolved aliases (see `patch.zig`).
        // Whichever language produced the anchors, the target has not seen
        // them, so the pass runs on the fact rather than the declaration.
        ast = try parse_dispatch.materialize(a, &doc.ast, .lax);
    }

    const root = if (opts.from.len == 0) ast.root else (try ast.getValByPath(opts.from)).id;
    if (!opts.lossless) return .{ .ast = ast, .root = root };

    // Both envelope passes work from `ast.root`, so the subtree is selected
    // first, by re-rooting a shallow copy: node ids are self-referential, so a
    // copy pointing elsewhere IS that subtree.
    var view = ast.*;
    view.root = root;
    const decoded = try a.create(fig.AST);
    decoded.* = try fig.Lossless.decode(a, &view);
    // gron encodes for JSON's declaration, as `runGet` does — its value
    // layer is JSON.
    const native = parse_dispatch.nativeForFormat(target_format) orelse
        return .{ .ast = decoded, .root = decoded.root };
    const encoded = try a.create(fig.AST);
    encoded.* = try fig.Lossless.encode(a, decoded, native);
    return .{ .ast = encoded, .root = encoded.root };
}

pub fn runFmt(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.FmtOptions) !void {
    if (opts.requested_help) {
        try Help.fmt(stdout_term, binary_name);
        return;
    }
    // `--diff` is a preview mode just like `--dry-run` — nothing is
    // ever written to `file` under either.
    const preview_only = opts.dry_run or opts.diff;
    const is_stdin = std.mem.eql(u8, opts.file, "-");
    if (!preview_only and is_stdin) {
        try stderr_term.writer.print(
            "error: cannot format stdin in place; pass --dry-run or --diff to print the formatted result instead.\n",
            .{},
        );
        try stderr_term.writer.flush();
        std.process.exit(2);
    }

    // Read-only when only previewing (`--dry-run`/`--diff`): no need to
    // open for writing what will never be written. Otherwise read_write,
    // like `replace`/`set`/`insert`/`delete` — read the whole file first,
    // then splice the same handle in place (never via shell redirection,
    // which truncates a `> file` target before this process ever runs).
    const input = try fileio.getInput(io, opts.file, if (preview_only) .read_only else .read_write);
    defer if (!is_stdin) input.close(io);

    const content = try fileio.readAll(a, io, input);

    var from = opts.from;
    if (opts.detect) from = try parse_dispatch.resolveFormatFromContent(a, content, opts.file);

    const reformatted = try reformat.reformatSlice(a, stderr_term, opts.file, from, content, opts.serialize, opts.quiet, opts.strict);
    const changed = !std.mem.eql(u8, content, reformatted);

    if (opts.diff) {
        try diff.unifiedDiff(a, stdout_term.writer, opts.file, content, reformatted, 3);
        try stdout_term.writer.flush();
        if (changed) std.process.exit(1);
    } else if (opts.dry_run) {
        try stdout_term.writer.writeAll(reformatted);
        try stdout_term.writer.flush();
        if (changed) std.process.exit(1);
    } else if (changed) {
        // Read-then-splice-same-handle (never shell redirection): the
        // in-memory `content` above was read before this write touches
        // the file, so there is no truncate-before-read race.
        try input.writePositionalAll(io, reformatted, 0);
        try input.setLength(io, reformatted.len);
    }
}

/// Finish a `convert` invocation given the original `content` and the
/// `result` it converts to: write `result` back to `input` in place when
/// `write` is set (skipped if the bytes are already identical), then print
/// either a unified diff (`show_diff`) or the whole `result` to stdout — the
/// whole result only when neither `write` nor `show_diff` fired, so `--write`
/// alone stays silent (like `fmt`) and `--write --diff` shows what changed
/// without also dumping the full file.
fn finishConvert(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, input: Io.File, file_path: []const u8, content: []const u8, result: []const u8, write: bool, show_diff: bool) !void {
    const changed = !std.mem.eql(u8, content, result);
    if (write and changed) {
        try input.writePositionalAll(io, result, 0);
        try input.setLength(io, result.len);
    }
    if (show_diff) {
        try diff.unifiedDiff(a, stdout_term.writer, file_path, content, result, 3);
        try stdout_term.writer.flush();
    } else if (!write) {
        try stdout_term.writer.writeAll(result);
        try stdout_term.writer.flush();
    }
}

pub fn runLang(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.LangOptions) !void {
    if (opts.requested_help) {
        try Help.lang(stderr_term, binary_name);
        return;
    }
    switch (opts.verb) {
        .list => try languages.list(io, a, stdout_term),
        .check => try languages.check(io, a, stdout_term, stderr_term, opts.name, opts.against, opts.files),
        .table => try languages.printTable(io, a, stdout_term, stderr_term, opts.name, opts.input, opts.spec),
    }
}

pub fn runConvert(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.ConvertOptions) !void {
    if (opts.requested_help) {
        try Help.convert(stdout_term, binary_name);
        return;
    }
    const is_stdin = std.mem.eql(u8, opts.file, "-");
    if (opts.write and is_stdin) {
        try stderr_term.writer.print(
            "error: cannot write stdin in place; omit --write to print the converted result instead.\n",
            .{},
        );
        try stderr_term.writer.flush();
        std.process.exit(2);
    }

    const input = try fileio.getInput(io, opts.file, if (opts.write) .read_write else .read_only);
    defer if (!is_stdin) input.close(io);

    const content = try fileio.readAll(a, io, input);

    var from = opts.from;
    if (opts.detect) from = try parse_dispatch.resolveFormatFromContent(a, content, opts.file);

    const converted = try reformat.convertSlice(
        a,
        stderr_term,
        opts.file,
        from,
        opts.to,
        content,
        opts.serialize,
        opts.lossless,
        opts.lax_tags,
        opts.quiet,
        opts.strict,
    );
    try finishConvert(a, io, stdout_term, input, opts.file, content, converted, opts.write, opts.diff);
}
