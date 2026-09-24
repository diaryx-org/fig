//! In-place editing plumbing shared by `edit`/`set`/`insert`/`delete`/
//! `comment`: the single span-splice path (`applyEdit`) behind every
//! structural op, its embed-aware twin (`applyToEmbed`), and the small
//! helpers (value rendering, empty-document seeds) those two lean on.
const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");

const types = @import("types.zig");
const fileio = @import("fileio.zig");
const value_arg = @import("value_arg.zig");

const Format = types.Format;
const EditOp = types.EditOp;
const Io = std.Io;

/// Extract the embedded config of `embed_type` from a host file and parse it.
/// The returned document's node spans are relative to the embedded region.
pub fn parseEmbeddedFromFile(allocator: std.mem.Allocator, io: Io, file: Io.File, embed_type: fig.Embed.Type) !fig.Document {
    const content = try fileio.readAll(allocator, io, file);
    const embedded = try fig.Embed.extract(allocator, content, embed_type);
    return embedded.document;
}

/// Apply one in-place edit to `content` (a complete document parsed under
/// `dialect`) and return the new bytes. The single span-splice path behind both
/// the `edit` and `comment` actions.
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
    applyOp(Lang, &editor, path, text, op) catch |err| {
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
        // The new key is a NAME too, spelled the same way.
        .replace_key => try editor.replaceNamedKey(path, text),
        .add_leading_comment => try editor.addLeadingComment(path, text),
        .set_trailing_comment => try editor.setTrailingComment(path, text),
        .delete_leading_comments => try editor.deleteLeadingComments(path),
        .delete_trailing_comment => try editor.deleteTrailingComment(path),
        // The CLI's key is a NAME; `insertKey` takes key syntax, so it is
        // spelled as the format spells one first — `.name` in ZON, quoted
        // for strict JSON — exactly as `set`'s insert branch spells it.
        .insert_key => |key| try editor.insertNamedKey(path, key, text),
        .set => try editor.set(path, text),
        .set_sequence => |items| try editor.setSequence(path, items),
        .append_seq => try editor.appendToSeq(path, text),
        .prepend_seq => try editor.prependToSeq(path, text),
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
    if (index != std.math.maxInt(usize)) return index;
    const parsed = editor.getParsed() catch return index;
    const seq = parsed.ast.getValByPath(path) catch return index;
    if (seq.kind != .sequence) return index;
    var item = (parsed.ast.child(&seq) catch return index) orelse return index;
    var last: usize = 0;
    while (parsed.ast.next(&item)) |nxt| : (last += 1) item = nxt;
    return last;
}

pub fn applyToFile(
    comptime Lang: type,
    allocator: std.mem.Allocator,
    io: Io,
    file: Io.File,
    path: []fig.AST.PathSegment,
    text: []const u8,
    op: EditOp,
    dialect: Lang.Type,
) !void {
    const content = try fileio.readAll(allocator, io, file);
    defer allocator.free(content);

    const edited = try applyEdit(Lang, allocator, content, path, text, op, dialect);
    try file.writePositionalAll(io, edited, 0);
    try file.setLength(io, edited.len);
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
    return if (inline_comment)
        editor.getTrailingComment(path)
    else
        editor.getLeadingComment(path);
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

/// Read a comment from the embedded config of a host file: extract the region,
/// parse only that slice as its inner format, and read the comment from it. The
/// read-only twin of `applyToEmbed`.
pub fn getCommentFromEmbed(
    allocator: std.mem.Allocator,
    io: Io,
    file: Io.File,
    embed_type: fig.Embed.Type,
    path: []fig.AST.PathSegment,
    inline_comment: bool,
) !?[]u8 {
    const content = try fileio.readAll(allocator, io, file);
    defer allocator.free(content);

    const embedded = try fig.Embed.extract(allocator, content, embed_type);
    defer embedded.deinit(allocator);
    // The DECODED content (entity-decoded for `<code>`, a borrow otherwise) — the
    // bytes the parser saw, so comments read correctly through the codec.
    const inner = embedded.decoded.text;

    // Keyed on the archetype's inner FORMAT, not the archetype itself, so every
    // archetype sharing a format (`---`/endmatter/```yaml ⇒ YAML, `;;;`/```json
    // ⇒ JSON, `+++`/```toml ⇒ TOML) routes to the same reader. The reader
    // itself is the registry's — the same `Lang`/`dialect` pair `route` uses
    // for a whole file of that format. (No `caps.edit` test, unlike `route`:
    // every language in tree is editable, and `Embed.InnerFormat`'s own
    // membership assert pins which of them have an embedded spelling. So
    // every format reachable here is editable.)
    return switch (fig.Embed.innerFormat(embed_type)) {
        inline else => |f| {
            const d = comptime fig.Language.entryFor(@tagName(f));
            if (comptime d.Lang == void) return error.FormatDisabled;
            // Strict JSON frontmatter has no comment syntax, so a read here
            // finds nothing — the editor's own answer, not a special case.
            return getComment(d.Lang, allocator, inner, path, inline_comment, d.dialect);
        },
    };
}

/// Apply an edit to the embedded config of a host file in place: extract the
/// region, edit only that slice as its inner format, then splice it back between
/// the retained fences so the rest of the host file is byte-identical.
pub fn applyToEmbed(
    allocator: std.mem.Allocator,
    io: Io,
    file: Io.File,
    embed_type: fig.Embed.Type,
    path: []fig.AST.PathSegment,
    text: []const u8,
    value: OpValue,
    op: EditOp,
) !void {
    const content = try fileio.readAll(allocator, io, file);
    defer allocator.free(content);

    // Locate the region; when it's absent and the op can seed a fresh block
    // (`set` / insert-a-key), synthesize an empty one — the CLI's open-or-init.
    // `base` is the document the edited content splices back into: the original
    // file, or the synthesized host carrying the new empty block.
    var base: []const u8 = content;
    var created_host: ?[]u8 = null;
    defer if (created_host) |h| allocator.free(h);
    const region = reg: {
        if (fig.Embed.locateRegion(content, embed_type)) |r| {
            break :reg r;
        } else |err| switch (err) {
            error.NotFound => {
                if (!opSeedsEmptyRegion(op)) return err;
                const created = try fig.Embed.initRegion(allocator, content, embed_type);
                created_host = created.host;
                base = created.host;
                break :reg created.region;
            },
            else => return err,
        }
    };
    const inner = base[region.content.start..region.content.end];

    // For a codec archetype (`<code>`), the on-disk content is entity-encoded:
    // decode it (with provenance) so the editor sees real config, then re-encode
    // span-aware so only the edited bytes change. Identity archetypes borrow
    // `inner` unchanged and `reencodeEdited` is a passthrough.
    const codec = fig.Embed.codecOf(embed_type);
    const decoded = try fig.Embed.decodeForParse(allocator, inner, codec);
    defer decoded.deinit(allocator);

    // Keyed on the archetype's inner FORMAT, not the archetype itself: a fenced
    // ```yaml block edits identically to `---` YAML, `+++`/```toml to TOML, etc.
    // The per-format facts are `route`'s, read from the same registry entries:
    // which language module and dialect to reparse under, and the format a
    // value argument is rendered into (`renderEdit`).
    const edited_decoded = switch (fig.Embed.innerFormat(embed_type)) {
        inline else => |f| blk: {
            const d = comptime fig.Language.entryFor(@tagName(f));
            if (comptime d.Lang == void) return error.FormatDisabled;
            const format = @field(Format, @tagName(f));
            const r = try renderEdit(allocator, text, value, op, format, .natural);
            break :blk applyEdit(d.Lang, allocator, decoded.text, path, r.text, r.op, d.dialect) catch |err| {
                // See `route`: a block value landing in a flow collection.
                if (err != error.BlockValueIntoFlow or !value.any()) return err;
                const flow = try renderEdit(allocator, text, value, op, format, .flow);
                break :blk try applyEdit(d.Lang, allocator, decoded.text, path, flow.text, flow.op, d.dialect);
            };
        },
    };
    defer allocator.free(edited_decoded);
    const edited_inner = try fig.Embed.reencodeEdited(allocator, codec, inner, decoded, edited_decoded);
    defer allocator.free(edited_inner);

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(allocator);
    try out.appendSlice(allocator, base[0..region.content.start]);
    try out.appendSlice(allocator, edited_inner);
    try out.appendSlice(allocator, base[region.content.end..]);

    try file.writePositionalAll(io, out.items, 0);
    try file.setLength(io, out.items.len);
}

/// Whether `op` can seed a freshly-created empty embed region — only the ops
/// that establish a first entry (`set` upserts it; `insert_key` adds it). Other
/// ops (replace/delete/comment/sequence) require an already-present region.
pub fn opSeedsEmptyRegion(op: EditOp) bool {
    return switch (op) {
        .set, .insert_key => true,
        else => false,
    };
}

/// The value an op carries, read (`value_arg.read`) but not yet rendered:
/// `renderEdit` spells it once the target format is known. `one` is the
/// value of `set`/`insert`/`edit`; `items` are `set --seq`'s. Empty for an op
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
pub fn renderEdit(allocator: std.mem.Allocator, text: []const u8, value: OpValue, op: EditOp, format: Format, layout: value_arg.Layout) !struct { text: []const u8, op: EditOp } {
    const text_out = if (value.one) |v| try value_arg.render(allocator, v, format, layout) else text;
    const op_out: EditOp = switch (op) {
        .set_sequence => |items| if (value.items.len == 0) op else blk: {
            std.debug.assert(value.items.len == items.len);
            const rendered = try allocator.alloc([]const u8, value.items.len);
            for (value.items, 0..) |v, i| rendered[i] = try value_arg.render(allocator, v, format, layout);
            break :blk .{ .set_sequence = rendered };
        },
        else => op,
    };
    return .{ .text = text_out, .op = op_out };
}

/// The minimal valid empty document for `format`, used to seed a file `set`
/// creates from scratch before landing its first key. `null` means the format
/// has no empty-document form to seed into, so a from-scratch `set` on it is
/// refused before any file is created — the two non-`Language` projections,
/// canonical and gron.
///
/// Every seed now comes from the format registry's `empty_doc_seed`, which is
/// also what `Embed.initRegion` writes into a freshly created region, so the
/// two can no longer drift. The seeds themselves and the reasoning behind each
/// (why an empty STRING is a valid seed for YAML/TOML/fig/INI/dotenv/
/// `.properties`/NestedText but `{}`/`.{}` is needed for JSON/ZON) are
/// documented on the entries in `languages/language.zig`.
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
        inline else => |f| comptime fig.Language.entryFor(@tagName(f)).empty_doc_seed,
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
/// entry point in the CLI goes through it — `edit`'s value/key replacement,
/// `set`/`insert`/`delete`'s structural ops (via `applyStructuralEdit`), and
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
                    const r = try renderEdit(allocator, ap.text, ap.value, ap.op, format, .natural);
                    applyToFile(fig.Runtime.Language, allocator, io, file, ap.path, r.text, r.op, e.typeOf()) catch |err| {
                        if (err != error.BlockValueIntoFlow or !ap.value.any()) return err;
                        const flow = try renderEdit(allocator, ap.text, ap.value, ap.op, format, .flow);
                        try applyToFile(fig.Runtime.Language, allocator, io, file, ap.path, flow.text, flow.op, e.typeOf());
                    };
                    return null;
                },
            }
        },
        inline else => |f| {
            const d = comptime fig.Language.entryFor(@tagName(f));
            if (comptime d.Lang == void) return error.FormatDisabled;
            if (comptime !d.Lang.caps.edit) return error.FormatNotEditable;
            switch (req) {
                .get_comment => |g| return getCommentFromFile(d.Lang, allocator, io, file, g.path, g.inline_comment, d.dialect),
                .apply => |ap| {
                    const r = try renderEdit(allocator, ap.text, ap.value, ap.op, format, .natural);
                    applyToFile(d.Lang, allocator, io, file, ap.path, r.text, r.op, d.dialect) catch |err| {
                        // A value rendered in the format's own layout (a
                        // YAML block) whose site is inside a flow collection
                        // (`k: {a: 1}`) is refused, having written nothing;
                        // it goes again on one line, which is what fits.
                        if (err != error.BlockValueIntoFlow or !ap.value.any()) return err;
                        const flow = try renderEdit(allocator, ap.text, ap.value, ap.op, format, .flow);
                        try applyToFile(d.Lang, allocator, io, file, ap.path, flow.text, flow.op, d.dialect);
                    };
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

/// Shared per-format routing for the structural `set`/`insert`/`delete`
/// actions: `embed` routes through the host-document splicer, everything else
/// through `route`. `op` already encodes which editor primitive runs and `path`
/// is the container path it operates on.
pub fn applyStructuralEdit(
    allocator: std.mem.Allocator,
    io: Io,
    input: Io.File,
    resolved: Format,
    embed: ?fig.Embed.Type,
    path: []fig.AST.PathSegment,
    text: []const u8,
    value: OpValue,
    op: EditOp,
) !void {
    if (embed) |embed_type| return applyToEmbed(allocator, io, input, embed_type, path, text, value, op);
    return applyToFileAs(allocator, io, input, resolved, path, text, value, op);
}

test "applyEdit performs the structural ops on YAML" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const Y = fig.Language.YAML;
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

test "applyEdit blames the spliced text, not the file, when the edit won't parse" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    const t = std.testing;
    const T = fig.Language.TOML;
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
    const Y = fig.Language.YAML;
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
    const D = fig.Language.DOTENV;
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
    const T = fig.Language.TOML;
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
    const I = fig.Language.INI;
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
    const T = fig.Language.TOML;
    const dia = T.default_type;

    // The reported shape: `["REPLACED"]` is valid TOML, so the splice into the
    // header key's span reparsed cleanly and the rename went unnoticed.
    var path = [_]fig.AST.PathSegment{.{ .key = "nested" }};
    try t.expectError(error.CannotReplaceTable, applyEdit(T, t.allocator, "[nested]\nk = \"v\"\n", &path, "\"REPLACED\"", .replace_value, dia));
    // A key INSIDE the table still edits normally.
    var inner = [_]fig.AST.PathSegment{ .{ .key = "nested" }, .{ .key = "k" } };
    const out = try applyEdit(T, t.allocator, "[nested]\nk = \"v\"\n", &inner, "\"w\"", .replace_value, dia);
    defer t.allocator.free(out);
    try t.expectEqualStrings("[nested]\nk = \"w\"\n", out);
}

test "applyEdit performs the structural ops on .properties, including from-empty insert" {
    if (comptime !build_options.lang_properties) return error.SkipZigTest;
    const t = std.testing;
    const P = fig.Language.PROPERTIES;
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
        const Z = fig.Language.ZON;
        const dia = comptime fig.Language.entryFor("zon").dialect;
        const out = try applyEdit(Z, a, ".{\n    .a = 1,\n}\n", &.{}, "5", .{ .insert_key = "new" }, dia);
        try t.expectEqualStrings(".{\n    .a = 1,\n    .new = 5,\n}\n", out);
        const quoted = try applyEdit(Z, a, ".{ .a = 1 }", &.{}, "5", .{ .insert_key = "has space" }, dia);
        try t.expectEqualStrings(".{ .a = 1, .@\"has space\" = 5 }", quoted);
    }
    if (comptime build_options.lang_json) {
        const J = fig.Language.JSON;
        const dia = comptime fig.Language.entryFor("json").dialect;
        const j = try renderEdit(a, "v", try figValue(a, "v"), .{ .insert_key = "k\"q" }, .json, .natural);
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
        const Y = fig.Language.YAML;
        const dia = comptime fig.Language.entryFor("yaml").dialect;
        try t.expectError(error.DuplicateKey, applyEdit(Y, a, "a: 1\n", &.{}, "3", .{ .insert_key = "a" }, dia));
        var p = [_]fig.AST.PathSegment{.{ .key = "a" }};
        const set = try applyEdit(Y, a, "a: 1\n", &p, "3", .set, dia);
        try t.expectEqualStrings("a: 3\n", set);
    }
    if (comptime build_options.lang_json) {
        const J = fig.Language.JSON;
        const dia = comptime fig.Language.entryFor("json").dialect;
        const j = try renderEdit(a, "x", try figValue(a, "x"), .{ .insert_key = "s" }, .json, .natural);
        try t.expectError(error.DuplicateKey, applyEdit(J, a, "{\"s\":\"hi\"}", &.{}, j.text, j.op, dia));
    }
    if (comptime build_options.lang_ini) {
        const I = fig.Language.INI;
        const dia = comptime fig.Language.entryFor("ini").dialect;
        var p = [_]fig.AST.PathSegment{.{ .key = "a" }};
        try t.expectError(error.DuplicateKey, applyEdit(I, a, "[a]\nx = 1\n", &p, "2", .{ .insert_key = "x" }, dia));
    }
    if (comptime build_options.lang_toml) {
        const T = fig.Language.TOML;
        try t.expectError(error.DuplicateKey, applyEdit(T, a, "a = 1\n", &.{}, "3", .{ .insert_key = "a" }, T.default_type));
    }
}

test "applyToEmbed refuses a new leading block in front of another archetype's frontmatter" {
    if (comptime !build_options.lang_json or !build_options.lang_yaml) return error.SkipZigTest;
    const t = std.testing;
    const io = t.io;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var tmp = t.tmpDir(.{});
    defer tmp.cleanup();

    // `fig set --embed frontmatter-json p.md k 1` on YAML frontmatter used to
    // prepend a `;;;` block, leaving the `---` one under it — two metadata
    // blocks, the original no longer on the first line.
    const md = "---\ntitle: x\n---\nbody\n";
    const file = try tmp.dir.createFile(io, "p.md", .{ .read = true });
    defer file.close(io);
    try file.writeStreamingAll(io, md);
    var p = [_]fig.AST.PathSegment{.{ .key = "k" }};
    try t.expectError(error.FrontmatterExists, applyToEmbed(a, io, file, .semicolons_json, &p, "1", .{}, .set));
    const back = try tmp.dir.readFileAlloc(io, "p.md", a, .limited(1 << 20));
    try t.expectEqualStrings(md, back);

    // Endmatter goes at the bottom and displaces nothing.
    try applyToEmbed(a, io, file, .endmatter_yaml, &p, "1", .{}, .set);
    const both = try tmp.dir.readFileAlloc(io, "p.md", a, .limited(1 << 20));
    try t.expectEqualStrings(md ++ "```endmatter\nk: 1\n```\n", both);
}

test "emptyDocSeed: every seed is the byte string the registry declares" {
    const t = std.testing;
    try t.expectEqualStrings("{}\n", emptyDocSeed(.json).?);
    try t.expectEqualStrings("{}\n", emptyDocSeed(.jsonc).?);
    try t.expectEqualStrings("{}\n", emptyDocSeed(.json5).?);
    try t.expectEqualStrings(".{}\n", emptyDocSeed(.zon).?);
    try t.expectEqualStrings("<dict>\n</dict>\n", emptyDocSeed(.plist).?);
    // An empty file already parses as an empty (but present) root mapping for
    // all of these, so the first key can just be inserted into it.
    for ([_]Format{ .yaml, .toml, .fig, .dotenv, .properties, .ini, .nestedtext }) |f|
        try t.expectEqualStrings("", emptyDocSeed(f).?);
    // No empty-document form: a from-scratch `set` is refused before a file
    // lands on disk.
    for ([_]Format{ .canonical, .gron }) |f|
        try t.expectEqual(@as(?[]const u8, null), emptyDocSeed(f));
}

/// `text` read as a fig value, as the CLI reads a value argument.
fn figValue(a: std.mem.Allocator, text: []const u8) !OpValue {
    var sink: std.Io.Writer.Discarding = .init(&.{});
    var term: Io.Terminal = .{ .writer = &sink.writer, .mode = .no_color };
    return .{ .one = try value_arg.read(a, &term, text, .fig) };
}

test "renderEdit spells a value as the format does and leaves a key a name, a delete bare" {
    if (comptime !build_options.lang_json or !build_options.lang_fig) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    // The key stays a name: `applyOp` spells it through `key_style`.
    const ins = try renderEdit(a, "v", try figValue(a, "v"), .{ .insert_key = "k" }, .json, .natural);
    try t.expectEqualStrings("\"v\"", ins.text);
    try t.expectEqualStrings("k", ins.op.insert_key);

    // A number is a number in JSON too; it used to arrive as `"5"`.
    const num = try renderEdit(a, "5", try figValue(a, "5"), .append_seq, .json, .natural);
    try t.expectEqualStrings("5", num.text);

    const del = try renderEdit(a, "", .{}, .delete_key, .json, .natural);
    try t.expectEqualStrings("", del.text);
    try t.expectEqual(EditOp.delete_key, del.op);

    // set_sequence renders each item.
    var sink: std.Io.Writer.Discarding = .init(&.{});
    var term: Io.Terminal = .{ .writer = &sink.writer, .mode = .no_color };
    const raw_items = [_][]const u8{ "x", "true" };
    const items = [_]value_arg.Value{ try value_arg.read(a, &term, "x", .fig), try value_arg.read(a, &term, "true", .fig) };
    const sq = try renderEdit(a, "", .{ .items = &items }, .{ .set_sequence = &raw_items }, .json, .natural);
    try t.expectEqualStrings("\"x\"", sq.op.set_sequence[0]);
    try t.expectEqualStrings("true", sq.op.set_sequence[1]);
}

// Regression: `.json5` used to be a hard-refused branch in both `runEdit`
// and `applyStructuralEdit` (`error.UnsupportedJson5Edit`) even though
// `Editor(json.Language)` with `.format = .JSON5` fully supports every
// structural op (see `languages/json/editor_helper.zig`). These exercise the
// CLI's own dispatch path — `renderEdit` spelling the value, then
// `applyEdit` reparsing under the `json5` registry entry's dialect (`.JSON5`)
// — the same two calls `route` makes, so a re-introduced gate would only be
// caught by hitting this path, not by the lower-level editor tests alone.
test "applyEdit performs the structural ops on JSON5 via the CLI's render+dialect path" {
    if (comptime !build_options.lang_json or !build_options.lang_fig) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const J = fig.Language.JSON;
    const dia = comptime fig.Language.entryFor("json5").dialect;
    try t.expectEqual(J.Type.JSON5, dia);

    // insert_key quotes the new key and writes the value as the number it
    // is (unquoted keys elsewhere in the document are untouched).
    {
        const j = try renderEdit(a, "8080", try figValue(a, "8080"), .{ .insert_key = "port" }, .json5, .natural);
        const out = try applyEdit(J, a, "{ host: 'localhost' }", &.{}, j.text, j.op, dia);
        try t.expectEqualStrings("{ host: 'localhost', \"port\": 8080 }", out);
    }
    // replace_value (the `edit` action, without --key): a string stays one.
    {
        var p = [_]fig.AST.PathSegment{.{ .key = "host" }};
        const j = try renderEdit(a, "example.org", try figValue(a, "example.org"), .replace_value, .json5, .natural);
        const out = try applyEdit(J, a, "{ host: 'localhost', port: 8080 }", &p, j.text, j.op, dia);
        try t.expectEqualStrings("{ host: \"example.org\", port: 8080 }", out);
    }
    // delete_key carries no value and drops the entry outright.
    {
        var p = [_]fig.AST.PathSegment{.{ .key = "port" }};
        const j = try renderEdit(a, "", .{}, .delete_key, .json5, .natural);
        const out = try applyEdit(J, a, "{ host: 'localhost', port: 8080 }", &p, j.text, j.op, dia);
        try t.expectEqualStrings("{ host: 'localhost' }", out);
    }
    // append_seq onto a trailing-comma array doesn't double the comma.
    {
        var p = [_]fig.AST.PathSegment{.{ .key = "tags" }};
        const j = try renderEdit(a, "3", try figValue(a, "3"), .append_seq, .json5, .natural);
        const out = try applyEdit(J, a, "{ tags: [1, 2,] }", &p, j.text, j.op, dia);
        try t.expectEqualStrings("{ tags: [1, 2, 3,] }", out);
    }
}

test "emptyDocSeed: seedable formats round-trip a first `set`, others refuse" {
    // Names YAML/JSON/TOML/fig dialects and parsers directly, so it has
    // nothing left to assert in a build without them.
    if (comptime !(build_options.lang_yaml and build_options.lang_json and
        build_options.lang_toml and build_options.lang_fig)) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    // The seed a from-scratch `set` writes must parse and accept the first key,
    // reproducing on-disk `createSeededFile` + `applyStructuralEdit` in memory.
    var path = [_]fig.AST.PathSegment{.{ .key = "hello" }};

    // YAML: empty seed, bare value.
    const yaml = try applyEdit(fig.Language.YAML, a, emptyDocSeed(.yaml).?, &path, "world", .set, fig.Language.YAML.default_type);
    try t.expectEqualStrings("hello: world\n", yaml);

    // JSON: `{}` seed, value rendered through the JSON path like the CLI does.
    const jv = try renderEdit(a, "world", try figValue(a, "world"), .set, .json, .natural);
    const json = try applyEdit(fig.Language.JSON, a, emptyDocSeed(.json).?, &path, jv.text, jv.op, .JSON);
    try t.expect(std.mem.indexOf(u8, json, "\"hello\"") != null);
    try t.expect(std.mem.indexOf(u8, json, "\"world\"") != null);

    // JSON5: same `{}` seed as JSON, reparsed under the JSON5 dialect — its
    // in-place edit is no longer gated off (see the dedicated JSON5 test above).
    const json5 = try applyEdit(fig.Language.JSON, a, emptyDocSeed(.json5).?, &path, jv.text, jv.op, comptime fig.Language.entryFor("json5").dialect);
    try t.expect(std.mem.indexOf(u8, json5, "\"hello\"") != null);
    try t.expect(std.mem.indexOf(u8, json5, "\"world\"") != null);

    // TOML: empty seed, value already a TOML literal.
    const toml = try applyEdit(fig.Language.TOML, a, emptyDocSeed(.toml).?, &path, "\"world\"", .set, fig.Language.TOML.default_type);
    try t.expectEqualStrings("hello = \"world\"\n", toml);

    // fig: empty seed (an empty document parses as an empty map), bare value.
    const figc = try applyEdit(fig.Language.FIG, a, emptyDocSeed(.fig).?, &path, "world", .set, fig.Language.FIG.default_type);
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
// on-disk path in memory — `createSeededFile(seed)` then `applyStructuralEdit`
// — and checks the result is a plist that reparses with the new key in it.
test "emptyDocSeed: plist seeds a document a from-scratch `set` lands into" {
    if (comptime !build_options.lang_plist) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const P = fig.Language.PLIST;

    const seed = emptyDocSeed(.plist) orelse return error.TestExpectedSeed;
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
