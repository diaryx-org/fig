//! In-place editing plumbing shared by `replace`/`rename`/`set`/`insert`/`delete`/
//! `comment`: the single span-splice path (`applyEdit`) behind every
//! structural op, and the small helpers (value rendering, empty-document
//! seeds) it leans on.
const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");

const types = @import("types.zig");
const languages = @import("languages.zig");
const fileio = @import("fileio.zig");
const value_arg = @import("value_arg.zig");

const Format = types.Format;
const EditOp = types.EditOp;
const Io = std.Io;

/// Apply one in-place edit to `content` (a complete document parsed under
/// `dialect`) and return the new bytes. The single span-splice path behind both
/// the `replace`/`rename` and `comment` actions.
pub fn applyEdit(
    comptime Lang: type,
    allocator: std.mem.Allocator,
    content: []const u8,
    path: []fig.AST.PathSegment,
    text: []const u8,
    op: EditOp,
    dialect: Lang.Type,
) ![]u8 {
    var editor: fig.Editor(Lang) = .{ .allocator = allocator, .format = dialect };
    try editor.init(content);
    defer editor.deinit();
    // `[-]` anywhere in the path names the last item that is there. The op
    // receives the parent of the slot it creates, so an append's own `[-]`
    // never reaches here.
    const parsed = try editor.getParsed();
    const resolved = try resolveLast(allocator, &parsed.ast, path);
    defer if (resolved.ptr != path.ptr) allocator.free(resolved);
    applyOp(Lang, &editor, resolved, text, op) catch |err| {
        // `init` parsed `content`, so a parse failure from here on is about
        // the text the caller spliced in, not the file — and it arrives as a
        // plain `InvalidNumber`/`UnexpectedToken`/… that reads as if the FILE
        // were malformed. Re-label it so the CLI can blame (and quote) the
        // argument instead; see `diag_report.printBadEditText`.
        if (editor.splice_rejected) return error.InvalidEditText;
        return err;
    };
    return allocator.dupe(u8, editor.source.items);
}

/// The op dispatch behind `applyEdit`, split out only so its errors can be
/// caught in one place there.
fn applyOp(
    comptime Lang: type,
    editor: *fig.Editor(Lang),
    path: []fig.AST.PathSegment,
    text: []const u8,
    op: EditOp,
) !void {
    switch (op) {
        .replace_value => try editor.replaceValAtPath(path, text),
        // The new key is a NAME too, spelled the same way. A name the
        // mapping already holds is reported as `rename`'s refusal, not as
        // `insert`'s, whose help would send the user to `set`.
        .replace_key => editor.replaceNamedKey(path, text) catch |err| return if (err == error.DuplicateKey) error.RenameTargetExists else err,
        .add_leading_comment => try editor.addLeadingComment(path, text),
        .set_trailing_comment => try editor.setTrailingComment(path, text),
        .delete_leading_comments => try editor.deleteLeadingComments(path),
        .delete_trailing_comment => try editor.deleteTrailingComment(path),
        // The CLI's key is a NAME; `insertKey` takes key syntax, so it is
        // spelled as the format spells one first — `.name` in ZON, quoted
        // for strict JSON — exactly as `set`'s insert branch spells it.
        .insert_key => |key| try editor.insertNamedKey(path, key, text),
        .set => try editor.set(path, text),
        .replace_string => |string| try editor.replaceStringAtPath(path, text, string),
        .set_string => |string| try editor.setString(path, text, string),
        .set_sequence => |items| try editor.setSequence(path, items),
        .append_seq => try editor.appendToSeq(path, text),
        .set_append => try editor.setAppend(path, text),
        .prepend_seq => try editor.prependToSeq(path, text),
        .insert_seq_item => |index| try editor.insertItem(path, index, text),
        .delete_key => if (!try deleteIfSection(Lang, editor, path)) try editor.deleteKey(path),
        .remove_seq_item => |index| {
            // A sequence of sections — TOML's `[[x]]` — has its elements
            // addressed by index, so the section check wants the full path.
            var full: std.ArrayList(fig.AST.PathSegment) = .empty;
            defer full.deinit(editor.allocator);
            try full.appendSlice(editor.allocator, path);
            try full.append(editor.allocator, .{ .index = lastIndex(Lang, editor, path, index) });
            if (!try deleteIfSection(Lang, editor, full.items)) try editor.removeSeqItem(path, index);
        },
    }
}

/// `delete` on a path whose value is a SECTION — a TOML table or
/// array-of-tables element, an INI section, a fig block container, a runtime
/// language's section — deletes the whole container. `deleteKey` and
/// `removeSeqItem` refuse one, because its body is lines they cannot see;
/// `deleteContainer` is the op that gathers every region of it. Returns
/// whether it did, so the caller runs the ordinary op otherwise.
fn deleteIfSection(comptime Lang: type, editor: *fig.Editor(Lang), path: []const fig.AST.PathSegment) !bool {
    if (comptime !fig.Editor(Lang).is_section_format) return false;
    const parsed = try editor.getParsed();
    const node = parsed.ast.getValByPath(path) catch return false;
    if (!parsed.isSection(node)) return false;
    try editor.deleteContainer(path);
    return true;
}

/// `index`, with the end sentinel `[-]`/`[$]` (`maxInt`) resolved to the
/// last item of the sequence at `path`. Anything that is not a non-empty
/// sequence is returned as given, for `removeSeqItem` to refuse.
fn lastIndex(comptime Lang: type, editor: *fig.Editor(Lang), path: []const fig.AST.PathSegment, index: usize) usize {
    if (index != types.append_index) return index;
    const parsed = editor.getParsed() catch return index;
    return lastIndexIn(&parsed.ast, path) orelse index;
}

/// The index of the last item of the sequence at `path` in `ast`, or null
/// when there is no non-empty sequence there.
fn lastIndexIn(ast: *const fig.AST, path: []const fig.AST.PathSegment) ?usize {
    const seq = ast.getValByPath(path) catch return null;
    if (seq.kind != .sequence) return null;
    var item = (ast.child(&seq) catch return null) orelse return null;
    var last: usize = 0;
    while (ast.next(&item)) |nxt| : (last += 1) item = nxt;
    return last;
}

/// `path` with every `[-]`/`[$]` resolved, left to right, to the last item
/// of the sequence it indexes — so `list[-].name` reads, replaces or
/// comments the last item's `name`. A sentinel with no non-empty sequence
/// under it stays as it is, and the lookup that follows reports the path
/// missing. Returns `path` itself when it holds no sentinel, else an owned
/// copy.
pub fn resolveLast(allocator: std.mem.Allocator, ast: *const fig.AST, path: []fig.AST.PathSegment) ![]fig.AST.PathSegment {
    const first = for (path, 0..) |seg, i| {
        if (seg == .index and seg.index == types.append_index) break i;
    } else return path;
    const out = try allocator.dupe(fig.AST.PathSegment, path);
    for (out[first..], first..) |*seg, i| {
        if (seg.* != .index or seg.index != types.append_index) continue;
        if (lastIndexIn(ast, out[0..i])) |last| seg.* = .{ .index = last };
    }
    return out;
}

pub fn applyToFile(
    comptime Lang: type,
    allocator: std.mem.Allocator,
    io: Io,
    file: Io.File,
    path: []fig.AST.PathSegment,
    text: []const u8,
    value: OpValue,
    op: EditOp,
    format: Format,
    dialect: Lang.Type,
) !void {
    const content = try fileio.readAll(allocator, io, file);
    defer allocator.free(content);

    const edited = try applyValueEdit(Lang, allocator, content, path, text, value, op, format, dialect);
    try file.writePositionalAll(io, edited, 0);
    try file.setLength(io, edited.len);
}

/// The text a value argument was written as when the document refused it —
/// set just before `applyValueEdit` returns `error.InvalidEditText` for a
/// rendered value, so the report can show the spelling that was refused
/// rather than only what was typed. Null otherwise. The CLI makes one edit
/// per run, so one slot is all there is to hold.
pub var refused_rendering: ?[]const u8 = null;

/// `applyEdit` with `value` rendered into `format` first — the one place a
/// value argument meets the document (`applyToFile`). Two retries live here, each on
/// the one copy of `content`, so neither reads or parses the file again:
///
///   * a value rendered in the format's own layout (a YAML block) whose site
///     is inside a flow collection (`k: {a: 1}`) is refused having written
///     nothing, and goes again on one line (`Layout.flow`), which is what
///     fits there;
///   * an op that may create a key (`insert_key`, `set`) whose splice is
///     refused goes again with `0` — a value every format spells — and when
///     that is refused too the KEY is what the document would not take:
///     `error.InvalidEditKey`, so the report names the key, not the value.
pub fn applyValueEdit(
    comptime Lang: type,
    allocator: std.mem.Allocator,
    content: []const u8,
    path: []fig.AST.PathSegment,
    text: []const u8,
    value: OpValue,
    op: EditOp,
    format: Format,
    dialect: Lang.Type,
) ![]u8 {
    refused_rendering = null;
    // How deep the value lands: at `path` for a replacement or an upsert,
    // one level below it for a new key or a sequence item.
    const depth = path.len + @as(usize, switch (op) {
        .insert_key, .append_seq, .set_append, .prepend_seq, .insert_seq_item, .set_sequence => 1,
        else => 0,
    });
    if (value.one) |v| try value_arg.refuseDropped(allocator, v, format, depth);
    for (value.items) |v| try value_arg.refuseDropped(allocator, v, format, depth);
    var r = try renderEdit(allocator, text, value, op, format, .natural);
    const first = applyEdit(Lang, allocator, content, path, r.text, r.op, dialect);
    if (first) |edited| return edited else |err| switch (err) {
        error.BlockValueIntoFlow => if (value.any()) {
            r = try renderEdit(allocator, text, value, op, format, .flow);
            if (applyEdit(Lang, allocator, content, path, r.text, r.op, dialect)) |edited| return edited else |e| if (e != error.InvalidEditText) return e;
        } else return err,
        error.InvalidEditText => {},
        else => return err,
    }
    // The splice was refused (`InvalidEditText`): say which half of it.
    switch (op) {
        .insert_key, .set => if (applyEdit(Lang, allocator, content, path, "0", op, dialect)) |probe| {
            allocator.free(probe);
        } else |e| if (e == error.InvalidEditText) return error.InvalidEditKey,
        else => {},
    }
    if (value.one != null) refused_rendering = r.text;
    return error.InvalidEditText;
}

/// Read back a comment from `content` (parsed under `dialect`) without writing:
/// the trailing (same-line) comment on the value at `path` when `inline_comment`,
/// else the own-line block above the node. Returns `null` when there is no such
/// comment (the CLI then prints a blank line). The read-only twin of `applyEdit`'s
/// comment ops.
pub fn getComment(
    comptime Lang: type,
    allocator: std.mem.Allocator,
    content: []const u8,
    path: []fig.AST.PathSegment,
    inline_comment: bool,
    dialect: Lang.Type,
) !?[]u8 {
    var editor: fig.Editor(Lang) = .{ .allocator = allocator, .format = dialect };
    try editor.init(content);
    defer editor.deinit();
    const parsed = try editor.getParsed();
    const resolved = try resolveLast(allocator, &parsed.ast, path);
    defer if (resolved.ptr != path.ptr) allocator.free(resolved);
    return if (inline_comment)
        editor.getTrailingComment(resolved)
    else
        editor.getLeadingComment(resolved);
}

pub fn getCommentFromFile(
    comptime Lang: type,
    allocator: std.mem.Allocator,
    io: Io,
    file: Io.File,
    path: []fig.AST.PathSegment,
    inline_comment: bool,
    dialect: Lang.Type,
) !?[]u8 {
    const content = try fileio.readAll(allocator, io, file);
    defer allocator.free(content);
    return getComment(Lang, allocator, content, path, inline_comment, dialect);
}

/// The value an op carries, read (`value_arg.read`) but not yet rendered:
/// `renderEdit` spells it once the target format is known. `one` is the
/// value of `set`/`insert`/`replace`; `items` are `set --seq`'s. Empty for an op
/// with no value (a comment, a delete, a key rename), whose text is spliced
/// as it stands.
pub const OpValue = struct {
    one: ?value_arg.Value = null,
    items: []const value_arg.Value = &.{},

    fn any(self: OpValue) bool {
        return self.one != null or self.items.len > 0;
    }
};

/// `text`/`op` with `value` rendered into `format` in place of the argument
/// text: the edit's own text for `one`, each `set_sequence` item for `items`.
/// The same for every format — a JSON file gets `5` for the number and
/// `"hello"` for the string, where it used to get every argument quoted.
/// A string `replace` or `set` becomes `replace_string` or `set_string`, so
/// the string it replaces keeps its quote style; a `--raw` value is the
/// user's own spelling, and stays as it is.
pub fn renderEdit(allocator: std.mem.Allocator, text: []const u8, value: OpValue, op: EditOp, format: Format, layout: value_arg.Layout) !struct { text: []const u8, op: EditOp } {
    const text_out = if (value.one) |v| try value_arg.render(allocator, v, format, layout) else text;
    const op_out: EditOp = switch (op) {
        .set_sequence => |items| if (value.items.len == 0) op else blk: {
            std.debug.assert(value.items.len == items.len);
            const rendered = try allocator.alloc([]const u8, value.items.len);
            for (value.items, 0..) |v, i| rendered[i] = try value_arg.render(allocator, v, format, layout);
            break :blk .{ .set_sequence = rendered };
        },
        .replace_value => if (stringOf(value)) |s| .{ .replace_string = s } else op,
        .set => if (stringOf(value)) |s| .{ .set_string = s } else op,
        else => op,
    };
    return .{ .text = text_out, .op = op_out };
}

/// The string `value` holds, when it is one string fig read and renders.
fn stringOf(value: OpValue) ?[]const u8 {
    const one = value.one orelse return null;
    return switch (one) {
        .raw => null,
        .tree => |t| switch (t.node(t.root).kind) {
            .string => |s| s,
            else => null,
        },
    };
}

/// The minimal valid empty document for `format`, used to seed a file `set`
/// creates from scratch before landing its first key. `null` means the format
/// has no empty-document form to seed into, so a from-scratch `set` on it is
/// refused before any file is created — the two non-`Language` projections,
/// canonical and gron.
///
/// Every seed comes from the format registry's `empty_doc_seed`. The seeds themselves and the reasoning behind each
/// (why an empty STRING is a valid seed for YAML/TOML/fig/INI/dotenv/
/// `.properties`/NestedText but `{}`/`.{}` is needed for JSON/ZON) are
/// documented on the entries in `contract/language.zig`.
///
/// Deliberately NOT gated on `d.Lang == void`: a seed is plain build-invariant
/// data, and a gated-out language's from-scratch `set` still fails — with
/// `FormatDisabled` from the edit itself, after the seeded file is rolled back
/// (see `runSet`) — rather than with the "no empty-document form" message,
/// which would be a false statement about the format.
pub fn emptyDocSeed(format: Format) ?[]const u8 {
    return switch (format) {
        // The canonical oracle grammar is a parse/print pair and gron is a
        // projection; neither is a stored format anyone creates from scratch.
        .canonical, .gron => null,
        _ => if (types.runtimeEntry(format)) |e| e.empty_doc_seed else null,
    };
}

/// What the CLI wants done to a document, independent of which language it
/// turns out to be — the payload of `route` below.
pub const EditRequest = union(enum) {
    /// Splice an edit into the file in place. `text`/`op` are exactly what
    /// `applyToFile` takes.
    apply: struct { path: []fig.AST.PathSegment, text: []const u8, value: OpValue = .{}, op: EditOp },
    /// Read one comment back without writing (`comment --get`).
    get_comment: struct { path: []fig.AST.PathSegment, inline_comment: bool },
};

/// THE editor dispatch: the one place a CLI `Format` becomes "which language
/// module, which dialect, and is this format editable at all". Every editing
/// entry point in the CLI goes through it — `replace`'s value and `rename`'s key,
/// `set`/`insert`/`delete`'s structural ops (via `applyToFileAs`), and
/// both halves of `comment` — so the per-format knowledge below is stated once
/// rather than in four parallel switches that could disagree.
///
/// Returns the comment for a `get_comment` request (null when there is none)
/// and always null for an `apply`; the thin wrappers below give each caller the
/// signature it actually wants.
///
/// The three per-format facts, all derived:
///
///   * WHETHER the format can be edited at all — `Lang.caps.edit`. Every
///     registered language can (generic XML, a reader and a writer with no
///     span-splicing editor, was the exception until core 3.0 removed it);
///     the test stays for an out-of-tree `Language` that declares otherwise.
///     plist, despite being XML-based, is a strict typed subset with a real
///     editor (`Editor(Plist)` renders typed value elements and `<!-- -->`
///     comments), which is why it routes here like everything else.
///   * HOW a value argument is spelled — `renderEdit`, which renders the
///     value read from the command line into this format as a binding's
///     value is rendered (`value_arg.render`): `5` is a number and `hello` a
///     string whatever the format. Text with no value behind it — a comment,
///     a key name, a `--raw` argument — is spliced as it stands.
///   * WHICH dialect to reparse under — `Entry.dialect`, which is what keeps a
///     JSONC/JSON5 file's comments valid on reparse and what used to be the
///     hand-written `jsonDialect`.
///
/// The formats' own remaining quirks live with their editors, not here:
/// `Editor(Ini)` refuses to line-delete a scattered `[section]` header and to
/// auto-vivify a brand-new section (INI has no empty-mapping literal);
/// `Editor(NestedText)` declines a same-line trailing comment (no such spelling
/// in the grammar) and an insert into an empty inline `{}`/`[]`.
pub fn route(
    allocator: std.mem.Allocator,
    io: Io,
    file: Io.File,
    format: Format,
    req: EditRequest,
) !?[]u8 {
    @setEvalBranchQuota(30_000);
    switch (format) {
        // The canonical form is a parse/print pair with no span-splicing
        // editor; convert via `get` instead of editing in place.
        .canonical => return error.UnsupportedCanonicalEdit,
        // gron is a CLI-only get/echo projection with no in-place editor.
        .gron => return error.UnsupportedGronEdit,
        // A runtime language: the one `Editor(Runtime.Language)`, with the
        // entry's dialect as its `format`; whether it edits at all and how
        // it takes text are the entry's own facts, read at the call.
        _ => {
            const e = types.runtimeEntry(format) orelse return error.FormatDisabled;
            if (!e.language.caps.edit) return error.FormatNotEditable;
            switch (req) {
                .get_comment => |g| return getCommentFromFile(fig.Runtime.Language, allocator, io, file, g.path, g.inline_comment, e.typeOf()),
                .apply => |ap| {
                    try applyToFile(fig.Runtime.Language, allocator, io, file, ap.path, ap.text, ap.value, ap.op, format, e.typeOf());
                    return null;
                },
            }
        },
    }
}

// `error.FormatNotEditable` is the refusal for a language whose `caps.edit`
// is false. No language in tree declares that any more (generic XML was the
// one, until core 3.0 removed it), so the arm above is unreachable in every
// shipped build; it stays because `caps.edit` is part of the `Language`
// contract an out-of-tree format may write against, and the CLI's answer to
// such a format should be a refusal rather than an `Editor` instantiation
// that fails to compile.

/// Apply an in-place edit to `file` as `format`. The wrapper every writing
/// action uses; see `route` for what it derives.
pub fn applyToFileAs(
    allocator: std.mem.Allocator,
    io: Io,
    file: Io.File,
    format: Format,
    path: []fig.AST.PathSegment,
    text: []const u8,
    value: OpValue,
    op: EditOp,
) !void {
    _ = try route(allocator, io, file, format, .{ .apply = .{ .path = path, .text = text, .value = value, .op = op } });
}

/// Read one comment back from `file` as `format` without writing — the
/// `comment --get` wrapper around `route`.
pub fn getCommentAs(
    allocator: std.mem.Allocator,
    io: Io,
    file: Io.File,
    format: Format,
    path: []fig.AST.PathSegment,
    inline_comment: bool,
) !?[]u8 {
    return route(allocator, io, file, format, .{ .get_comment = .{ .path = path, .inline_comment = inline_comment } });
}

test "applyEdit reads [-] before the end of a path as the last item" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = fig.bundle.Language("yaml");
    const dia = Y.default_type;
    const last = types.append_index;

    var item = [_]fig.AST.PathSegment{ .{ .key = "l" }, .{ .index = last } };
    const replaced = try applyEdit(Y, t.allocator, "l: [a, b]\n", &item, "z", .replace_value, dia);
    defer t.allocator.free(replaced);
    try t.expectEqualStrings("l: [a, z]\n", replaced);
    // The caller's path is left as written, for the retries in
    // `applyValueEdit` that resolve it again.
    try t.expectEqual(last, item[1].index);

    var nested = [_]fig.AST.PathSegment{ .{ .key = "o" }, .{ .index = last }, .{ .key = "n" } };
    const renamed = try applyEdit(Y, t.allocator, "o:\n  - n: a\n  - n: b\n", &nested, "c", .replace_value, dia);
    defer t.allocator.free(renamed);
    try t.expectEqualStrings("o:\n  - n: a\n  - n: c\n", renamed);

    // With no item to be the last, the path is missing.
    try t.expectError(error.NotFound, applyEdit(Y, t.allocator, "l: []\n", &item, "z", .replace_value, dia));
}

test "applyEdit performs the structural ops on YAML" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = fig.bundle.Language("yaml");
    const dia = Y.default_type;

    // insert_key appends a mapping entry.
    {
        const out = try applyEdit(Y, t.allocator, "a: 1\n", &.{}, "2", .{ .insert_key = "b" }, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("a: 1\nb: 2\n", out);
    }
    // append_seq / prepend_seq on a block sequence.
    {
        const app = try applyEdit(Y, t.allocator, "- x\n- y\n", &.{}, "z", .append_seq, dia);
        defer t.allocator.free(app);
        try t.expectEqualStrings("- x\n- y\n- z\n", app);

        const pre = try applyEdit(Y, t.allocator, "- x\n- y\n", &.{}, "w", .prepend_seq, dia);
        defer t.allocator.free(pre);
        try t.expectEqualStrings("- w\n- x\n- y\n", pre);

        const mid = try applyEdit(Y, t.allocator, "- x\n- y\n", &.{}, "m", .{ .insert_seq_item = 1 }, dia);
        defer t.allocator.free(mid);
        try t.expectEqualStrings("- x\n- m\n- y\n", mid);
        try t.expectError(error.NotFound, applyEdit(Y, t.allocator, "- x\n- y\n", &.{}, "m", .{ .insert_seq_item = 3 }, dia));
    }
    // delete_key removes a mapping entry; remove_seq_item drops an item.
    {
        var dk_path = [_]fig.AST.PathSegment{.{ .key = "a" }};
        const dk = try applyEdit(Y, t.allocator, "a: 1\nb: 2\n", &dk_path, "", .delete_key, dia);
        defer t.allocator.free(dk);
        try t.expectEqualStrings("b: 2\n", dk);

        const ri = try applyEdit(Y, t.allocator, "- x\n- y\n- z\n", &.{}, "", .{ .remove_seq_item = 1 }, dia);
        defer t.allocator.free(ri);
        try t.expectEqualStrings("- x\n- z\n", ri);
    }
}

test "applyEdit edits a BOM-led YAML document's first key in place, keeping the BOM" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = fig.bundle.Language("yaml");
    const dia = Y.default_type;
    const bom = "\xEF\xBB\xBF";

    var a = [_]fig.AST.PathSegment{.{ .key = "a" }};
    const Case = struct { op: EditOp, text: []const u8, in: []const u8, want: []const u8 };
    const cases = [_]Case{
        .{ .op = .set, .text = "set", .in = "a: 1\n", .want = "a: set\n" },
        .{ .op = .replace_value, .text = "2", .in = "a: 1\n", .want = "a: 2\n" },
        .{ .op = .replace_key, .text = "q", .in = "a: 1\n", .want = "q: 1\n" },
        .{ .op = .delete_key, .text = "", .in = "a: 1\nb: 2\n", .want = "b: 2\n" },
        .{ .op = .add_leading_comment, .text = "hi", .in = "a: 1\n", .want = "# hi\na: 1\n" },
    };
    inline for (cases) |c| {
        const out = try applyEdit(Y, t.allocator, bom ++ c.in, &a, c.text, c.op, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings(bom ++ c.want, out);
    }
    // The first item's mapping, whose key's column counts from past the BOM.
    var first = [_]fig.AST.PathSegment{ .{ .index = 0 }, .{ .key = "b" } };
    const nested = try applyEdit(Y, t.allocator, bom ++ "- a: 1\n", &first, "5", .set, dia);
    defer t.allocator.free(nested);
    try t.expectEqualStrings(bom ++ "- a: 1\n  b: 5\n", nested);
    const pre = try applyEdit(Y, t.allocator, bom ++ "- x\n", &.{}, "w", .prepend_seq, dia);
    defer t.allocator.free(pre);
    try t.expectEqualStrings(bom ++ "- w\n- x\n", pre);
}

test "applyEdit blames the spliced text, not the file, when the edit won't parse" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    const t = std.testing;
    const T = fig.bundle.Language("toml");
    const dia = T.default_type;

    // The file parses; the replacement (a git sha, unquoted) doesn't. The
    // underlying `UnquotedString` would read as if `rev.toml` were malformed,
    // so it comes back as `InvalidEditText` for `main` to report against the
    // argument instead.
    var p = [_]fig.AST.PathSegment{.{ .key = "rev" }};
    try t.expectError(error.InvalidEditText, applyEdit(T, t.allocator, "rev = \"old\"\n", &p, "cc5e7e51", .replace_value, dia));
    // Same for `set`'s upsert path and for a new key's value.
    var q = [_]fig.AST.PathSegment{.{ .key = "new" }};
    try t.expectError(error.InvalidEditText, applyEdit(T, t.allocator, "rev = \"old\"\n", &q, "cc5e7e51", .set, dia));

    // A failure that ISN'T about the text keeps its own error: nothing was
    // spliced, so there is no argument to blame.
    var missing = [_]fig.AST.PathSegment{.{ .key = "nope" }};
    try t.expectError(error.NotFound, applyEdit(T, t.allocator, "rev = \"old\"\n", &missing, "\"x\"", .replace_value, dia));

    // And a valid literal still lands.
    const ok = try applyEdit(T, t.allocator, "rev = \"old\"\n", &p, "\"cc5e7e51\"", .replace_value, dia);
    defer t.allocator.free(ok);
    try t.expectEqualStrings("rev = \"cc5e7e51\"\n", ok);
}

test "applyEdit set upserts a scalar and reconciles a sequence on YAML" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = fig.bundle.Language("yaml");
    const dia = Y.default_type;

    // set replaces an existing key …
    {
        var p = [_]fig.AST.PathSegment{.{ .key = "a" }};
        const out = try applyEdit(Y, t.allocator, "a: 1\nb: 2\n", &p, "9", .set, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("a: 9\nb: 2\n", out);
    }
    // … and creates an absent one.
    {
        var p = [_]fig.AST.PathSegment{.{ .key = "c" }};
        const out = try applyEdit(Y, t.allocator, "a: 1\n", &p, "3", .set, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("a: 1\nc: 3\n", out);
    }
    // set on an empty document seeds the first key — the open-or-init seed case.
    {
        var p = [_]fig.AST.PathSegment{.{ .key = "k" }};
        const out = try applyEdit(Y, t.allocator, "", &p, "v", .set, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("k: v\n", out);
    }
    // set_sequence reconciles to the target list, keeping survivors' comments.
    {
        var p = [_]fig.AST.PathSegment{.{ .key = "tags" }};
        const items = [_][]const u8{ "c", "a", "d" };
        const out = try applyEdit(Y, t.allocator, "tags:\n- a # first\n- b # second\n- c # third\n", &p, "", .{ .set_sequence = &items }, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("tags:\n- c # third\n- a # first\n- d\n", out);
    }
}

test "applyEdit performs the structural ops on dotenv, including from-empty insert" {
    if (comptime !build_options.lang_dotenv) return error.SkipZigTest;
    const t = std.testing;
    const D = fig.bundle.Language("dotenv");
    const dia = D.default_type;

    // insert_key into a brand-new (empty) document — the from-scratch `set`
    // seed path, and the case that used to panic in `insertBlockKey` before
    // it learned to handle a childless block mapping.
    {
        const out = try applyEdit(D, t.allocator, "", &.{}, "bar", .{ .insert_key = "FOO" }, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("FOO=bar\n", out);
    }
    // insert_key into an existing document uses '=' with no surrounding spaces.
    {
        const out = try applyEdit(D, t.allocator, "FOO=bar\n", &.{}, "qux", .{ .insert_key = "BAZ" }, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("FOO=bar\nBAZ=qux\n", out);
    }
    // delete_key down to empty, then set seeds it again.
    {
        var dk_path = [_]fig.AST.PathSegment{.{ .key = "FOO" }};
        const dk = try applyEdit(D, t.allocator, "FOO=bar\n", &dk_path, "", .delete_key, dia);
        defer t.allocator.free(dk);
        try t.expectEqualStrings("", dk);

        var set_path = [_]fig.AST.PathSegment{.{ .key = "AGAIN" }};
        const out = try applyEdit(D, t.allocator, dk, &set_path, "v2", .set, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("AGAIN=v2\n", out);
    }
}

test "applyEdit deletes a TOML table and an array-of-tables element whole" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    const t = std.testing;
    const T = fig.bundle.Language("toml");
    const dia = T.default_type;
    const src = "[[x]]\na = 1\n[[x]]\na = 2\n[y]\nb = 1\n";
    {
        var path = [_]fig.AST.PathSegment{.{ .key = "y" }};
        const out = try applyEdit(T, t.allocator, src, &path, "", .delete_key, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("[[x]]\na = 1\n[[x]]\na = 2\n", out);
    }
    {
        var path = [_]fig.AST.PathSegment{.{ .key = "x" }};
        const out = try applyEdit(T, t.allocator, src, &path, "", .{ .remove_seq_item = 0 }, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("[[x]]\na = 2\n[y]\nb = 1\n", out);
    }
    {
        var path = [_]fig.AST.PathSegment{.{ .key = "x" }};
        const out = try applyEdit(T, t.allocator, src, &path, "", .{ .remove_seq_item = std.math.maxInt(usize) }, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("[[x]]\na = 1\n[y]\nb = 1\n", out);
    }
    // An inline array still takes the ordinary item delete.
    {
        var path = [_]fig.AST.PathSegment{.{ .key = "a" }};
        const out = try applyEdit(T, t.allocator, "a = [1, 2]\n", &path, "", .{ .remove_seq_item = std.math.maxInt(usize) }, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("a = [1]\n", out);
    }
}

test "applyEdit performs the structural ops on ini, root and section" {
    if (comptime !build_options.lang_ini) return error.SkipZigTest;
    const t = std.testing;
    const I = fig.bundle.Language("ini");
    const dia = I.default_type;

    // insert_key into a brand-new (empty) document.
    {
        const out = try applyEdit(I, t.allocator, "", &.{}, "fig", .{ .insert_key = "name" }, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("name = fig\n", out);
    }
    // insert_key into an existing SECTION uses ' = ' with padding.
    {
        var path = [_]fig.AST.PathSegment{.{ .key = "server" }};
        const out = try applyEdit(I, t.allocator, "[server]\nhost = localhost\n", &path, "80", .{ .insert_key = "port" }, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("[server]\nhost = localhost\nport = 80\n", out);
    }
    // delete_key on a `[section]` deletes the whole section — every
    // occurrence of it — through `deleteContainer`, which `deleteKey`
    // refuses in favour of.
    {
        var dk_path = [_]fig.AST.PathSegment{.{ .key = "server" }};
        const out = try applyEdit(I, t.allocator, "[server]\nhost = localhost\n[other]\nk = v\n[server]\nport = 80\n", &dk_path, "", .delete_key, dia);
        defer t.allocator.free(out);
        try t.expectEqualStrings("[other]\nk = v\n", out);
    }
    // replace_value on the `[section]` header is refused for the same reason —
    // the section's span is its NAME, so this used to rewrite `[server]` into
    // `[REPLACED]` and report success.
    {
        var rv_path = [_]fig.AST.PathSegment{.{ .key = "server" }};
        try t.expectError(error.CannotReplaceSection, applyEdit(I, t.allocator, "[server]\nhost = localhost\n", &rv_path, "REPLACED", .replace_value, dia));
    }
}

test "applyEdit refuses replace_value on a toml [table] header" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    const t = std.testing;
    const T = fig.bundle.Language("toml");
    const dia = T.default_type;

    // The reported shape: `["REPLACED"]` is valid TOML, so the splice into the
    // header key's span reparsed cleanly and the rename went unnoticed.
    var path = [_]fig.AST.PathSegment{.{ .key = "nested" }};
    try t.expectError(error.CannotReplaceSection, applyEdit(T, t.allocator, "[nested]\nk = \"v\"\n", &path, "\"REPLACED\"", .replace_value, dia));
    // A key INSIDE the table still edits normally.
    var inner = [_]fig.AST.PathSegment{ .{ .key = "nested" }, .{ .key = "k" } };
    const out = try applyEdit(T, t.allocator, "[nested]\nk = \"v\"\n", &inner, "\"w\"", .replace_value, dia);
    defer t.allocator.free(out);
    try t.expectEqualStrings("[nested]\nk = \"w\"\n", out);
}

test "applyEdit performs the structural ops on .properties, including from-empty insert" {
    if (comptime !build_options.lang_properties) return error.SkipZigTest;
    const t = std.testing;
    const P = fig.bundle.Language("properties");
    const dia = P.default_type;

    const out = try applyEdit(P, t.allocator, "", &.{}, "bar", .{ .insert_key = "foo" }, dia);
    defer t.allocator.free(out);
    try t.expectEqualStrings("foo=bar\n", out);
}

// The seeds as literals, now that the registry (not the switch above) is where
// they are written. Cheap, build-invariant, and the one thing the round-trip
// tests below can't state: that the seed for a format they don't exercise is
// still exactly these bytes. plist's is fix #1 of the registry work — it was
// `null` here before the switch became a registry read.
test "applyEdit insert_key spells the key as the format does: `.name` in ZON, quoted in JSON" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    // The CLI's `insert` carries a key NAME; before this, ZON got `new = 5`
    // spliced in — no `.` — and every ZON insert was refused as invalid.
    if (comptime build_options.lang_zon) {
        const Z = fig.bundle.Language("zon");
        const dia = comptime fig.bundle.dialect("zon").dialect;
        const out = try applyEdit(Z, a, ".{\n    .a = 1,\n}\n", &.{}, "5", .{ .insert_key = "new" }, dia);
        try t.expectEqualStrings(".{\n    .a = 1,\n    .new = 5,\n}\n", out);
        const quoted = try applyEdit(Z, a, ".{ .a = 1 }", &.{}, "5", .{ .insert_key = "has space" }, dia);
        try t.expectEqualStrings(".{ .a = 1, .@\"has space\" = 5 }", quoted);
    }
    if (comptime build_options.lang_json) {
        const J = fig.bundle.Language("json");
        const dia = comptime fig.bundle.dialect("json").dialect;
        const j = try renderEdit(a, "v", try figValue(a, "v"), .{ .insert_key = "k\"q" }, types.mustNamed("json"), .natural);
        const out = try applyEdit(J, a, "{\"a\": 1}", &.{}, j.text, j.op, dia);
        try t.expectEqualStrings("{\"a\": 1, \"k\\\"q\": \"v\"}", out);
    }
}

test "applyEdit insert_key refuses a key that already exists, and does not blame the value" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    // `fig insert y.yaml a 3` on `a: 1` used to exit 0 with a second `a: 3`
    // in the file (YAML, JSON, INI, ZON, dotenv, .properties), and TOML/fig
    // reported the rejected reparse as a bad VALUE. `set` is the op for an
    // existing key.
    if (comptime build_options.lang_yaml) {
        const Y = fig.bundle.Language("yaml");
        const dia = comptime fig.bundle.dialect("yaml").dialect;
        try t.expectError(error.DuplicateKey, applyEdit(Y, a, "a: 1\n", &.{}, "3", .{ .insert_key = "a" }, dia));
        var p = [_]fig.AST.PathSegment{.{ .key = "a" }};
        const set = try applyEdit(Y, a, "a: 1\n", &p, "3", .set, dia);
        try t.expectEqualStrings("a: 3\n", set);
    }
    if (comptime build_options.lang_json) {
        const J = fig.bundle.Language("json");
        const dia = comptime fig.bundle.dialect("json").dialect;
        const j = try renderEdit(a, "x", try figValue(a, "x"), .{ .insert_key = "s" }, types.mustNamed("json"), .natural);
        try t.expectError(error.DuplicateKey, applyEdit(J, a, "{\"s\":\"hi\"}", &.{}, j.text, j.op, dia));
    }
    if (comptime build_options.lang_ini) {
        const I = fig.bundle.Language("ini");
        const dia = comptime fig.bundle.dialect("ini").dialect;
        var p = [_]fig.AST.PathSegment{.{ .key = "a" }};
        try t.expectError(error.DuplicateKey, applyEdit(I, a, "[a]\nx = 1\n", &p, "2", .{ .insert_key = "x" }, dia));
    }
    if (comptime build_options.lang_toml) {
        const T = fig.bundle.Language("toml");
        try t.expectError(error.DuplicateKey, applyEdit(T, a, "a = 1\n", &.{}, "3", .{ .insert_key = "a" }, T.default_type));
    }
}

test "emptyDocSeed: every seed is the byte string the registry declares" {
    const t = std.testing;
    try t.expectEqualStrings("{}\n", emptyDocSeed(types.mustNamed("json")).?);
    try t.expectEqualStrings("{}\n", emptyDocSeed(types.mustNamed("jsonc")).?);
    try t.expectEqualStrings("{}\n", emptyDocSeed(types.mustNamed("json5")).?);
    try t.expectEqualStrings(".{}\n", emptyDocSeed(types.mustNamed("zon")).?);
    if (types.named("plist")) |plist| try t.expectEqualStrings("<dict>\n</dict>\n", emptyDocSeed(plist).?);
    // An empty file already parses as an empty (but present) root mapping for
    // all of these, so the first key can just be inserted into it.
    for ([_]Format{ types.mustNamed("yaml"), types.mustNamed("toml"), types.mustNamed("figl"), types.mustNamed("dotenv"), types.mustNamed("properties"), types.mustNamed("ini"), types.mustNamed("nestedtext") }) |f|
        try t.expectEqualStrings("", emptyDocSeed(f).?);
    // No empty-document form: a from-scratch `set` is refused before a file
    // lands on disk.
    for ([_]Format{ .canonical, .gron }) |f|
        try t.expectEqual(@as(?[]const u8, null), emptyDocSeed(f));
}

/// `text` read as a figl value, as the CLI reads a value argument.
fn figValue(a: std.mem.Allocator, text: []const u8) !OpValue {
    var sink: std.Io.Writer.Discarding = .init(&.{});
    var term: Io.Terminal = .{ .writer = &sink.writer, .mode = .no_color };
    return .{ .one = try value_arg.read(a, &term, text, .fig) };
}

test "renderEdit spells a value as the format does and leaves a key a name, a delete bare" {
    if (comptime !build_options.lang_json or !build_options.lang_figl) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    // The key stays a name: `applyOp` spells it through the language's `renderKeyName`.
    const ins = try renderEdit(a, "v", try figValue(a, "v"), .{ .insert_key = "k" }, types.mustNamed("json"), .natural);
    try t.expectEqualStrings("\"v\"", ins.text);
    try t.expectEqualStrings("k", ins.op.insert_key);

    // A number is a number in JSON too; it used to arrive as `"5"`.
    const num = try renderEdit(a, "5", try figValue(a, "5"), .append_seq, types.mustNamed("json"), .natural);
    try t.expectEqualStrings("5", num.text);

    const del = try renderEdit(a, "", .{}, .delete_key, types.mustNamed("json"), .natural);
    try t.expectEqualStrings("", del.text);
    try t.expectEqual(EditOp.delete_key, del.op);

    // set_sequence renders each item.
    var sink: std.Io.Writer.Discarding = .init(&.{});
    var term: Io.Terminal = .{ .writer = &sink.writer, .mode = .no_color };
    const raw_items = [_][]const u8{ "x", "true" };
    const items = [_]value_arg.Value{ try value_arg.read(a, &term, "x", .fig), try value_arg.read(a, &term, "true", .fig) };
    const sq = try renderEdit(a, "", .{ .items = &items }, .{ .set_sequence = &raw_items }, types.mustNamed("json"), .natural);
    try t.expectEqualStrings("\"x\"", sq.op.set_sequence[0]);
    try t.expectEqualStrings("true", sq.op.set_sequence[1]);
}

// Regression: `.json5` used to be a hard-refused branch in both `runEdit`
// and `applyToFileAs` (`error.UnsupportedJson5Edit`) even though
// `Editor(json.Language)` with `.format = .JSON5` fully supports every
// structural op (see `languages/json/tests/editor.zig`). These exercise the
// CLI's own dispatch path — `renderEdit` spelling the value, then
// `applyEdit` reparsing under the `json5` registry entry's dialect (`.JSON5`)
// — the same two calls `route` makes, so a re-introduced gate would only be
// caught by hitting this path, not by the lower-level editor tests alone.
test "applyEdit performs the structural ops on JSON5 via the CLI's render+dialect path" {
    if (comptime !build_options.lang_json or !build_options.lang_figl) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const J = fig.bundle.Language("json");
    const dia = comptime fig.bundle.dialect("json5").dialect;
    try t.expectEqual(J.Type.JSON5, dia);

    // insert_key quotes the new key and writes the value as the number it
    // is (unquoted keys elsewhere in the document are untouched).
    {
        const j = try renderEdit(a, "8080", try figValue(a, "8080"), .{ .insert_key = "port" }, types.mustNamed("json5"), .natural);
        const out = try applyEdit(J, a, "{ host: 'localhost' }", &.{}, j.text, j.op, dia);
        try t.expectEqualStrings("{ host: 'localhost', \"port\": 8080 }", out);
    }
    // replace_value (the `replace` action): a string stays one, in the
    // single quotes of the string it replaces.
    {
        var p = [_]fig.AST.PathSegment{.{ .key = "host" }};
        const j = try renderEdit(a, "example.org", try figValue(a, "example.org"), .replace_value, types.mustNamed("json5"), .natural);
        const out = try applyEdit(J, a, "{ host: 'localhost', port: 8080 }", &p, j.text, j.op, dia);
        try t.expectEqualStrings("{ host: 'example.org', port: 8080 }", out);
    }
    // delete_key carries no value and drops the entry outright.
    {
        var p = [_]fig.AST.PathSegment{.{ .key = "port" }};
        const j = try renderEdit(a, "", .{}, .delete_key, types.mustNamed("json5"), .natural);
        const out = try applyEdit(J, a, "{ host: 'localhost', port: 8080 }", &p, j.text, j.op, dia);
        try t.expectEqualStrings("{ host: 'localhost' }", out);
    }
    // append_seq onto a trailing-comma array doesn't double the comma.
    {
        var p = [_]fig.AST.PathSegment{.{ .key = "tags" }};
        const j = try renderEdit(a, "3", try figValue(a, "3"), .append_seq, types.mustNamed("json5"), .natural);
        const out = try applyEdit(J, a, "{ tags: [1, 2,] }", &p, j.text, j.op, dia);
        try t.expectEqualStrings("{ tags: [1, 2, 3,] }", out);
    }
}

test "emptyDocSeed: seedable formats round-trip a first `set`, others refuse" {
    // Names YAML/JSON/TOML/fig dialects and parsers directly, so it has
    // nothing left to assert in a build without them.
    if (comptime !(build_options.lang_yaml and build_options.lang_json and
        build_options.lang_toml and build_options.lang_figl)) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    // The seed a from-scratch `set` writes must parse and accept the first key,
    // reproducing on-disk `createSeededFile` + `applyToFileAs` in memory.
    var path = [_]fig.AST.PathSegment{.{ .key = "hello" }};

    // YAML: empty seed, bare value.
    const yaml = try applyEdit(fig.bundle.Language("yaml"), a, emptyDocSeed(types.mustNamed("yaml")).?, &path, "world", .set, fig.bundle.Language("yaml").default_type);
    try t.expectEqualStrings("hello: world\n", yaml);

    // JSON: `{}` seed, value rendered through the JSON path like the CLI does.
    const jv = try renderEdit(a, "world", try figValue(a, "world"), .set, types.mustNamed("json"), .natural);
    const json = try applyEdit(fig.bundle.Language("json"), a, emptyDocSeed(types.mustNamed("json")).?, &path, jv.text, jv.op, .JSON);
    try t.expect(std.mem.indexOf(u8, json, "\"hello\"") != null);
    try t.expect(std.mem.indexOf(u8, json, "\"world\"") != null);

    // JSON5: same `{}` seed as JSON, reparsed under the JSON5 dialect — its
    // in-place edit is no longer gated off (see the dedicated JSON5 test above).
    const json5 = try applyEdit(fig.bundle.Language("json"), a, emptyDocSeed(types.mustNamed("json5")).?, &path, jv.text, jv.op, comptime fig.bundle.dialect("json5").dialect);
    try t.expect(std.mem.indexOf(u8, json5, "\"hello\"") != null);
    try t.expect(std.mem.indexOf(u8, json5, "\"world\"") != null);

    // TOML: empty seed, value already a TOML literal.
    const toml = try applyEdit(fig.bundle.Language("toml"), a, emptyDocSeed(types.mustNamed("toml")).?, &path, "\"world\"", .set, fig.bundle.Language("toml").default_type);
    try t.expectEqualStrings("hello = \"world\"\n", toml);

    // fig: empty seed (an empty document parses as an empty map), bare value.
    const figc = try applyEdit(fig.bundle.Language("figl"), a, emptyDocSeed(types.mustNamed("figl")).?, &path, "world", .set, fig.bundle.Language("figl").default_type);
    try t.expectEqualStrings("hello = world\n", figc);

    // Projection formats (gron, canonical) have no empty-document form, so
    // the create is refused before a file lands.
    try t.expectEqual(@as(?[]const u8, null), emptyDocSeed(.gron));
    try t.expectEqual(@as(?[]const u8, null), emptyDocSeed(.canonical));
}

// Fix #1 of the format-registry work: `emptyDocSeed(.plist)` used to be null,
// so `fig set missing.plist key value` refused ("plist has no empty-document
// form") even though `Editor(Plist)` can edit one and the parser accepts a bare
// `<dict>` root. The registry now declares the seed, and this reproduces the
// on-disk path in memory — `createSeededFile(seed)` then `applyToFileAs`
// — and checks the result is a plist that reparses with the new key in it.
test "emptyDocSeed: plist seeds a document a from-scratch `set` lands into" {
    if (comptime !build_options.lang_plist) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const P = fig.bundle.Language("plist");

    const seed = emptyDocSeed(types.mustNamed("plist")) orelse return error.TestExpectedSeed;
    // The seed alone must already be a valid plist — `createSeededFile` writes
    // it before anything reads it back.
    _ = try P.Parser.parse(a, seed, P.default_type);

    var path = [_]fig.AST.PathSegment{.{ .key = "somekey" }};
    const out = try applyEdit(P, a, seed, &path, "someval", .set, P.default_type);
    try t.expect(std.mem.indexOf(u8, out, "<key>somekey</key>") != null);

    // And the edited bytes reparse, with the value reachable at its path.
    const doc = try P.Parser.parse(a, out, P.default_type);
    const node = try doc.ast.getValByPath(&path);
    try t.expectEqualStrings("someval", node.kind.string);
}

test "applyValueEdit blames the key when the document takes no value under it" {
    if (comptime !build_options.lang_dotenv or !build_options.lang_figl) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const D = fig.bundle.Language("dotenv");
    const dia = comptime fig.bundle.dialect("dotenv").dialect;

    // `bad key` is no bash identifier: `0` fares no better than `x`, so the
    // key is what was refused — not the value, as the report used to say.
    try t.expectError(error.InvalidEditKey, applyValueEdit(D, a, "A=1\n", &.{}, "x", try figValue(a, "x"), .{ .insert_key = "bad key" }, types.mustNamed("dotenv"), dia));
    try t.expectEqual(@as(?[]const u8, null), refused_rendering);

    // A good key with a value the format takes lands as usual.
    const ok = try applyValueEdit(D, a, "A=1\n", &.{}, "x", try figValue(a, "x"), .{ .insert_key = "B" }, types.mustNamed("dotenv"), dia);
    try t.expectEqualStrings("A=1\nB=x\n", ok);
}
