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
const patch_ops = @import("patch_ops.zig");
const reformat = @import("reformat.zig");
const languages = @import("languages.zig");
const external = @import("external.zig");

const Help = help.Help;
const Format = types.Format;
const EditOp = types.EditOp;
const append_index = types.append_index;
const Io = std.Io;

pub fn runHelp(stderr_term: *Io.Terminal, binary_name: []const u8) !void {
    try stderr_term.writer.print(help.title_string, .{});
    try Help.general(stderr_term, binary_name);
}

/// Print the CLI's own version alongside the core library version it embeds
/// and its marketing epoch — two independent SemVer tracks (see
/// docs/VERSIONING.md; a CLI-only breaking change bumps `cli_version`
/// without requiring a `core_version`/ABI release, and vice versa) plus one
/// purely cosmetic label (`epoch` has no compatibility meaning — it's the
/// core's marketing name, not a version number).
/// The git-style fallback for a word that is not one of fig's own actions:
/// hand it to `fig-<word>` on PATH. All of it lives in `external.zig`; this
/// exists so that `main`'s dispatch switch stays one arm per action, straight
/// into this file.
pub fn runExternal(io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.ExternalOptions) !void {
    return external.run(io, stdout_term, stderr_term, binary_name, opts);
}

pub fn runVersion(stdout_term: *Io.Terminal, cli_version: []const u8, core_version: []const u8, epoch: []const u8) !void {
    try stdout_term.writer.print("fig {s} (core {s} \"{s}\")\n", .{ cli_version, core_version, epoch });
    try stdout_term.writer.flush();
}

pub fn runEdit(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, binary_name: []const u8, opts: types.EditOptions) !void {
    if (opts.requested_help) {
        try Help.edit(stdout_term, binary_name);
        return;
    }
    const input = try fileio.getInput(io, opts.file, .read_write);
    defer if (!std.mem.eql(u8, opts.file, "-")) input.close(io);

    const op: EditOp = if (opts.key) .replace_key else .replace_value;
    // Value/key replacement is the same span splice for every editable format
    // — a value or key node has a tight, contiguous span, so the generic
    // editor handles even a TOML table assembled from scattered headers — so
    // it routes through the shared editor dispatch like every other edit. That
    // is also where the JSON family's requoting of the replacement, the
    // refusal of a read-only format and the canonical/gron refusals now live;
    // see `edit_ops.route`.
    if (try args_mod.resolveEmbedType(io, a, input, opts.embed, opts.detect_embed)) |embed_type| {
        try edit_ops.applyToEmbed(a, io, input, embed_type, opts.path, opts.replacement, op);
    } else {
        const resolved = if (opts.detect) try parse_dispatch.detectFileFormat(io, a, opts.file) else opts.format;
        try edit_ops.applyToFileAs(a, io, input, resolved, opts.path, opts.replacement, op);
    }
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
    // When targeting an embed, the host is seeded empty ("") and the
    // embed machinery (`initRegion`) synthesizes the inner block itself.
    var created = false;
    const input = fileio.getInput(io, opts.file, .read_write) catch |err| switch (err) {
        error.FileNotFound => blk: {
            if (std.mem.eql(u8, opts.file, "-")) return err;
            if (opts.detect) {
                try stderr_term.writer.print("error: cannot create {s}: an unrecognized extension gives no format to seed. Use a known extension (.json/.jsonc/.yaml/.toml/.zon) or an existing file.\n", .{opts.file});
                try stderr_term.writer.flush();
                std.process.exit(2);
            }
            const seed: []const u8 = if (opts.embed != null or opts.detect_embed) "" else edit_ops.emptyDocSeed(opts.format) orelse {
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
    // Both flow through the shared structural-edit router, so the embed
    // path (which open-or-inits a missing block) is reused for free.
    const op: EditOp = if (opts.seq) .{ .set_sequence = opts.values } else .set;
    const text: []const u8 = if (opts.seq) "" else opts.value;
    // On a from-scratch create, roll the new file back if the edit fails
    // (e.g. a path whose parent segment resolves to a scalar, which `set`
    // refuses rather than clobber) so a failed `set` leaves no bare seed
    // behind — matching how it leaves an existing file untouched on
    // failure. A merely-missing parent map is no longer a failure: `set`
    // auto-vivifies it (see `Editor.set`).
    const embed = try args_mod.resolveEmbedType(io, a, input, opts.embed, opts.detect_embed);
    edit_ops.applyStructuralEdit(a, io, input, resolved, embed, opts.path, text, op) catch |err| {
        if (created) fileio.deleteCreatedFile(io, opts.file);
        return err;
    };
}

pub fn runInsert(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.InsertOptions) !void {
    if (opts.requested_help) {
        try Help.insert(stdout_term, binary_name);
        return;
    }
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
    const embed = try args_mod.resolveEmbedType(io, a, input, opts.embed, opts.detect_embed);
    switch (opts.path[opts.path.len - 1]) {
        .key => |key| try edit_ops.applyStructuralEdit(a, io, input, resolved, embed, parent, opts.value, .{ .insert_key = key }),
        .index => |index| {
            // The editor only prepends or appends; an addressable middle
            // index has no primitive, so reject it rather than guess.
            const op: EditOp = if (index == 0)
                .prepend_seq
            else if (index == append_index)
                .append_seq
            else {
                try stderr_term.writer.print("error: sequence insert supports only [0] (prepend) or [-]/[$] (append); mid-sequence insert is not yet available.\n", .{});
                try stderr_term.writer.flush();
                std.process.exit(2);
            };
            try edit_ops.applyStructuralEdit(a, io, input, resolved, embed, parent, opts.value, op);
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
    const embed = try args_mod.resolveEmbedType(io, a, input, opts.embed, opts.detect_embed);
    // A trailing index removes that item from the parent sequence; a
    // trailing key deletes the mapping entry named by the full path.
    switch (opts.path[opts.path.len - 1]) {
        .index => |index| try edit_ops.applyStructuralEdit(a, io, input, resolved, embed, opts.path[0 .. opts.path.len - 1], "", .{ .remove_seq_item = index }),
        .key => try edit_ops.applyStructuralEdit(a, io, input, resolved, embed, opts.path, "", .delete_key),
    }
}

pub fn runGet(a: std.mem.Allocator, io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.GetOptions) !void {
    if (opts.requested_help) {
        try Help.get(stdout_term, binary_name);
        return;
    }
    const input = try fileio.getInput(io, opts.file, .read_only);
    defer if (!std.mem.eql(u8, opts.file, "-")) input.close(io);

    // `--body`: print the host prose OUTSIDE the fences — the complement of
    // extracting the embed content. Both sides, in file order: for frontmatter
    // or endmatter one of them is empty (bar a BOM), so this is the block's
    // own side and nothing else; for a mid-document block (an HTML `<script>`
    // island) it is the whole host with just the block cut out, rather than
    // the arbitrary half `region.body` can name. With no such region the whole
    // file is the body.
    if (opts.body) {
        const content = try fileio.readAll(a, io, input);
        const embed_type = args_mod.resolveEmbedTypeFromContent(content, opts.embed, opts.detect_embed) orelse fig.Embed.Type{ .frontmatter = .yaml };
        if (fig.Embed.locateRegion(content, embed_type)) |region| {
            try stdout_term.writer.writeAll(content[region.body_before.start..region.body_before.end]);
            try stdout_term.writer.writeAll(content[region.body_after.start..region.body_after.end]);
        } else |err| switch (err) {
            error.NotFound => try stdout_term.writer.writeAll(content),
            else => return err,
        }
        try stdout_term.writer.flush();
        return;
    }

    // Resolved input/output formats. They equal the parsed options unless
    // the input format has to be sniffed from the file's contents (no
    // `--input`, unrecognized extension): detection overwrites `from`, and
    // — when no `--output` was given — `to` follows it (an echo round-trip
    // rather than a silent convert-to-JSON).
    var from = opts.from;
    var to = opts.to;

    // Whether `doc`'s spans are offsets into the file itself — false for an
    // embedded region, whose spans count from the region's own start.
    var whole_file = true;
    const doc = if (try args_mod.resolveEmbedType(io, a, input, opts.embed, opts.detect_embed)) |embed_type| blk_embed: {
        // `embed_type` may have just been sniffed at runtime
        // (`detect_embed`), in which case the parse-time
        // placeholder `from`/`to` (the extension's guess, not the
        // real archetype) needs correcting: the inner format
        // always follows the archetype outright, and — same as
        // the whole-file `detect` echo below — an unpinned
        // output follows it too rather than silently defaulting
        // to something else (e.g. `.yaml` for a `.md` file whose
        // actual frontmatter turned out to be fig or JSON).
        from = args_mod.embedFormat(embed_type);
        if (!opts.output_explicit) to = from;
        whole_file = false;
        break :blk_embed try edit_ops.parseEmbeddedFromFile(a, io, input, embed_type);
    } else blk: {
        // Read once so detection and parsing share the same bytes — a
        // piped stdin can only be consumed a single time.
        const content = try fileio.readAll(a, io, input);
        if (opts.detect) {
            from = try parse_dispatch.resolveFormatFromContent(a, content, opts.file);
            if (!opts.output_explicit) to = from;
        }
        var reports: parse_dispatch.Reports = .{};
        const parsed = parse_dispatch.parseSliceAs(from, .{}, a, content, false, &reports) catch |err| {
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
    const base_ast = try parse_dispatch.materializeFor(a, stderr_term, from, to, &doc, opts.file, whole_file, if (opts.lax_tags) .lax else .strict);

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

    const node_id = if (opts.path) |p| (try ast.getValByPath(p)).id else ast.root;

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

    // A runtime target prints through its vtable, with the same lossy
    // strips a compiled one gets; it has no loss diagnostics yet. A scalar
    // comes back as the fragment the editor would splice — the value as it
    // stands alone, no newline — where a compiled printer ends the line
    // itself, so `get` ends it here to print the same as one.
    if (types.runtimeEntry(to)) |e| {
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
        try stdout_term.writer.flush();
        return;
    }

    const target: fig.AST.SerializeFormat = types.toSerializeFormat(to) orelse unreachable; // handled by the early returns above

    // Surface everything the conversion would silently lose (comments
    // dropped/degraded, values dropped/degraded) — unless `--quiet`. The
    // pass is read-only and runs on the AST as it will be printed: under
    // `--lossless` the lossy nodes are already enveloped, so no value
    // warnings fire. `--strict` turns any warning into a hard failure.
    if (!opts.quiet or opts.strict) {
        const warnings = try fig.Diagnostics.analyze(a, ast, node_id, target, .{
            .pretty = opts.serialize.pretty,
            .strip_comments = opts.serialize.strip_comments,
            .lossless = opts.lossless,
        });
        // The CLI only surfaces losses the FORMAT forced. A loss the user
        // explicitly asked for (e.g. `--strip-comments`) carries
        // `explicit_option` and is not surprising, so it neither warns nor
        // trips `--strict` — it just rides through on the warning layer for
        // a library consumer that wants it.
        var surfaced: usize = 0;
        for (warnings) |w| {
            if (w.cause != .format_limitation) continue;
            surfaced += 1;
            if (!opts.quiet) {
                try stderr_term.setColor(.yellow);
                try stderr_term.writer.writeAll("warning: ");
                try stderr_term.setColor(.reset);
                try w.render(stderr_term.writer, target);
                try stderr_term.writer.writeByte('\n');
            }
        }
        if (!opts.quiet) try stderr_term.writer.flush();
        if (opts.strict and surfaced > 0) {
            try stderr_term.writer.print("error: {d} lossy conversion warning(s); --strict aborts.\n", .{surfaced});
            try stderr_term.writer.flush();
            std.process.exit(1);
        }
    }

    // `!opts.lossless` only: under `--lossless` these formats fall through to
    // the plain print below and, having no envelope of their own (see
    // `fig.FlatStrip`'s module doc), surface whatever they can't hold as a
    // real error via `reportSerializeError` — `--lossless` means "don't
    // silently drop data," so silently stripping under it would defeat the
    // flag's whole point.
    const flat_strip_depth: ?usize = if (!opts.lossless) parse_dispatch.flatStripDepth(target) else null;

    if (if (!opts.lossless) parse_dispatch.nullStripTarget(target) else null) |native| {
        // The target has no null (TOML, by its own `caps.lossless`). In lossy
        // mode, rather than the printer aborting mid-document on one, strip
        // unrepresentable values up front so output stays valid and complete
        // (the warnings above already reported them). `lossyStrip` re-roots
        // at `node_id`, so the result serializes whole.
        const result = try fig.Lossless.lossyStrip(a, ast, node_id, native);
        if (result.ast) |stripped| {
            try stripped.serializeWith(stdout_term.writer, target, opts.serialize);
        }
    } else if (flat_strip_depth) |depth| {
        // A flat format (INI/dotenv/.properties, by their own
        // `caps.max_mapping_depth`): same idea as TOML's null-stripping
        // above, but the capability rule is DEPTH-based, not
        // scalar-kind-based — an array, or a table nested past what the
        // format allows, would otherwise abort the printer mid-document
        // (already warned about above).
        const result = try fig.FlatStrip.lossyStrip(a, ast, node_id, depth);
        if (result.ast) |stripped| {
            try stripped.serializeWith(stdout_term.writer, target, opts.serialize);
        }
    } else if (opts.path == null) {
        ast.serializeWith(stdout_term.writer, target, opts.serialize) catch |err| switch (err) {
            error.FigUnrepresentableRoot => diag_report.reportFigUnrepresentableRoot(stderr_term),
            else => |e| diag_report.reportSerializeError(stderr_term, e),
        };
    } else {
        ast.serializeNodeWith(stdout_term.writer, target, node_id, opts.serialize) catch |err| switch (err) {
            error.FigUnrepresentableRoot => diag_report.reportFigUnrepresentableRoot(stderr_term),
            else => |e| diag_report.reportSerializeError(stderr_term, e),
        };
    }
    try stdout_term.writer.flush();
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
        const comment = if (try args_mod.resolveEmbedType(io, a, input, opts.embed, opts.detect_embed)) |embed_type|
            try edit_ops.getCommentFromEmbed(a, io, input, embed_type, opts.path, opts.inline_comment)
        else switch (resolved) {
            // Strict JSON has no comment syntax: there can be nothing to get.
            // The one refusal the shared dispatch can't state, because it is
            // about a DIALECT of an otherwise comment-carrying language — and
            // it is a message-and-exit(2), not an error value.
            .json => {
                try stderr_term.writer.print("error: strict JSON has no comments; use a .jsonc or .json5 file instead.\n", .{});
                try stderr_term.writer.flush();
                std.process.exit(2);
            },
            // Everything else reads its comment through the shared editor
            // dispatch. A format whose grammar has no same-line comment (INI,
            // NestedText) still answers a leading-comment read and surfaces
            // `error.CommentsUnsupported` from the editor under `--inline`.
            else => try edit_ops.getCommentAs(a, io, input, resolved, opts.path, opts.inline_comment),
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

    if (try args_mod.resolveEmbedType(io, a, input, opts.embed, opts.detect_embed)) |embed_type| {
        try edit_ops.applyToEmbed(a, io, input, embed_type, opts.path, opts.text, op);
    } else switch (resolved) {
        // Strict JSON has no comment syntax: fail with a clear message
        // rather than letting the editor surface a bare error. Same explicit
        // arm, and the same reason, as the `--get` branch above.
        .json => {
            try stderr_term.writer.print("error: strict JSON has no comments; use a .jsonc or .json5 file instead.\n", .{});
            try stderr_term.writer.flush();
            std.process.exit(2);
        },
        // Everything else goes through the shared editor dispatch — JSONC/
        // JSON5's `//` comments reparse under their own dialect there, and
        // `--inline` set/delete on a format with no same-line comment syntax
        // surfaces `error.CommentsUnsupported` from the editor.
        else => try edit_ops.applyToFileAs(a, io, input, resolved, opts.path, opts.text, op),
    }
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
/// The order below is not arbitrary. The TARGET is resolved first — read,
/// format-detected, embed-sniffed — because the patch document's own
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

    // An embedded target is patched as its inner format; a whole-file one as
    // itself, sniffed from the bytes when the extension didn't say.
    const embed_type = args_mod.resolveEmbedTypeFromContent(content, opts.embed, opts.detect_embed);
    const target_format = if (embed_type) |et|
        args_mod.embedFormat(et)
    else if (opts.detect)
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

    const patched: patch_ops.Result = if (embed_type) |et|
        try patchEmbed(a, content, et, target_format, req)
    else
        try patch_ops.applyToSlice(a, content, target_format, req);

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

    // The one thing a silent success would hide: trivia the patch carried that
    // the target has nowhere to put (strict JSON has no comment syntax at all).
    if (patched.stats.comments_dropped > 0 and !opts.quiet) {
        try stderr_term.writer.print(
            "warning: dropped {d} comment(s) the patch carried — {s} has no place for them here.\n",
            .{ patched.stats.comments_dropped, opts.file },
        );
        try stderr_term.writer.flush();
    }
}

/// Patch only the bytes between a host document's fences, and splice the
/// result back. The prose either side stays byte-identical; a codec archetype
/// (`<pre><code>`) is decoded for the merge and re-encoded span-aware after,
/// exactly as `edit_ops.applyToEmbed` does it for a single edit.
fn patchEmbed(
    a: std.mem.Allocator,
    content: []const u8,
    embed_type: fig.Embed.Type,
    format: Format,
    req: patch_ops.Request,
) !patch_ops.Result {
    const region = try fig.Embed.locateRegion(content, embed_type);
    const inner = content[region.content.start..region.content.end];
    const codec = fig.Embed.codecOf(embed_type);
    const decoded = try fig.Embed.decodeForParse(a, inner, codec);

    const patched = try patch_ops.applyToSlice(a, decoded.text, format, req);
    const edited_inner = try fig.Embed.reencodeEdited(a, codec, inner, decoded, patched.content);

    var out: std.ArrayList(u8) = .empty;
    try out.appendSlice(a, content[0..region.content.start]);
    try out.appendSlice(a, edited_inner);
    try out.appendSlice(a, content[region.content.end..]);
    return .{ .content = try out.toOwnedSlice(a), .stats = patched.stats };
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

    // The patch document may itself be a host document (one post's frontmatter
    // merged into another's), so it gets the same embed resolution the target
    // does — via `--patch-embed`, or sniffed for a `.md` extension.
    var format = opts.patch_format;
    var source: []const u8 = content;
    if (args_mod.resolveEmbedTypeFromContent(content, opts.patch_embed, opts.detect_patch_embed)) |et| {
        const region = try fig.Embed.locateRegion(content, et);
        const decoded = try fig.Embed.decodeForParse(a, content[region.content.start..region.content.end], fig.Embed.codecOf(et));
        source = decoded.text;
        format = args_mod.embedFormat(et);
    } else if (opts.detect_patch) {
        format = try parse_dispatch.resolveFormatFromContent(a, content, opts.patch_file);
    }

    var reports: parse_dispatch.Reports = .{};
    // In the arena, not on this frame: the AST outlives `loadPatch` — it is
    // what the merge reads all the way through — so a `&doc.ast` into a local
    // would dangle the moment this returns.
    const doc = try a.create(fig.Document);
    doc.* = parse_dispatch.parseSliceAs(format, .{}, a, source, false, &reports) catch |err| {
        // Same contract as `get`'s: a parse failure in a file the user named
        // renders as a `file:line:col` teaching message, not an error name.
        try reports.reportDiagnostics(stderr_term, source, opts.patch_file);
        return err;
    };
    try reports.reportWarnings(stderr_term, source, opts.patch_file, opts.quiet, false);

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
    // gron (no `SerializeFormat` of its own) encodes for JSON's declaration,
    // as `runGet` does — its value layer is JSON.
    const native = fig.Lossless.nativeFor(types.toSerializeFormat(target_format) orelse .json) orelse
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
    // like `edit`/`set`/`insert`/`delete` — read the whole file first,
    // then splice the same handle in place (never via shell redirection,
    // which truncates a `> file` target before this process ever runs).
    const input = try fileio.getInput(io, opts.file, if (preview_only) .read_only else .read_write);
    defer if (!is_stdin) input.close(io);

    const content = try fileio.readAll(a, io, input);

    // `--embed`: only the region between the fences is reformatted; the
    // rest of the host document is carried through byte-identical.
    if (args_mod.resolveEmbedTypeFromContent(content, opts.embed, opts.detect_embed)) |embed_type| {
        const region = try fig.Embed.locateRegion(content, embed_type);
        const inner = content[region.content.start..region.content.end];
        const reformatted_inner = try reformat.reformatSlice(a, stderr_term, opts.file, args_mod.embedFormat(embed_type), inner, opts.serialize, opts.quiet, opts.strict);

        var out: std.ArrayList(u8) = .empty;
        defer out.deinit(a);
        try out.appendSlice(a, content[0..region.content.start]);
        try out.appendSlice(a, reformatted_inner);
        try out.appendSlice(a, content[region.content.end..]);

        const changed = !std.mem.eql(u8, content, out.items);
        if (opts.diff) {
            try diff.unifiedDiff(a, stdout_term.writer, opts.file, content, out.items, 3);
            try stdout_term.writer.flush();
            if (changed) std.process.exit(1);
        } else if (opts.dry_run) {
            try stdout_term.writer.writeAll(out.items);
            try stdout_term.writer.flush();
            if (changed) std.process.exit(1);
        } else if (changed) {
            try input.writePositionalAll(io, out.items, 0);
            try input.setLength(io, out.items.len);
        }
        return;
    }

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
/// without also dumping the full file. Shared by both of `runConvert`'s modes
/// (whole-file and `--to-embed`), which differ only in how `result` is produced.
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

    if (opts.to_embed) |to_embed_type| {
        // Embed-archetype mode: resolve the SOURCE archetype (--embed,
        // else content-sniffed with `Embed.detect` — extension-derived
        // defaults were already folded into `opts.embed` at parse time),
        // convert the region's inner content, then rehouse it under the
        // target archetype's fences, preserving the host prose exactly.
        const source_type = opts.embed orelse (if (opts.detect_embed) fig.Embed.detect(content) else null) orelse {
            try stderr_term.writer.print(
                "error: could not detect an embedded region in `{s}`; pass --embed explicitly.\n",
                .{opts.file},
            );
            try stderr_term.writer.flush();
            std.process.exit(2);
        };
        const region = try fig.Embed.locateRegion(content, source_type);
        const inner = content[region.content.start..region.content.end];
        const converted_inner = try reformat.convertSlice(
            a,
            stderr_term,
            opts.file,
            args_mod.embedFormat(source_type),
            args_mod.embedFormat(to_embed_type),
            inner,
            false,
            opts.serialize,
            opts.lossless,
            opts.lax_tags,
            opts.quiet,
            opts.strict,
        );
        const out = fig.Embed.retype(a, content, region, source_type, to_embed_type, converted_inner) catch |err| switch (err) {
            // A mid-document block (an HTML `<script>`/`<pre><code>` island)
            // cannot become an edge archetype without either mangling the host
            // or silently dropping the half of it on the wrong side of the
            // block. Say so, and name the two conversions that do work.
            error.MidDocumentRegionCannotMove => {
                try stderr_term.writer.print(
                    "error: `{s}` keeps its config in a mid-document block, which cannot move to `{s}`" ++
                        " without rewriting the host around it.\n" ++
                        "note: convert between mid-document archetypes (html-script-*, html-code-*) instead," ++
                        " or extract the config to its own file with `fig get`.\n",
                    .{ opts.file, args_mod.embedTypeName(to_embed_type) },
                );
                try stderr_term.writer.flush();
                std.process.exit(2);
            },
            else => |e| return e,
        };
        try finishConvert(a, io, stdout_term, input, opts.file, content, out, opts.write, opts.diff);
        return;
    }

    var from = opts.from;
    if (opts.detect) from = try parse_dispatch.resolveFormatFromContent(a, content, opts.file);

    const converted = try reformat.convertSlice(
        a,
        stderr_term,
        opts.file,
        from,
        opts.to,
        content,
        true,
        opts.serialize,
        opts.lossless,
        opts.lax_tags,
        opts.quiet,
        opts.strict,
    );
    try finishConvert(a, io, stdout_term, input, opts.file, content, converted, opts.write, opts.diff);
}
