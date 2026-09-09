//! NestedText-specific editing helpers for `Editor(NestedText)`.
//!
//! The generic span-splice engine lives in `../../editor.zig`; this module
//! holds the NestedText-only logic it delegates to, mirroring TOML/YAML/fig/
//! plist's own `editor_helper.zig` split. NestedText needs MORE of its own
//! logic than any format but plist, for two independent reasons:
//!
//!   1. **Value framing.** Every value here is untyped text (no typed/quoted
//!      literal syntax to splice verbatim the way YAML/TOML/fig do), so a
//!      caller-supplied `value_text` is always a raw scalar that must be
//!      RENDERED into this grammar's own same-line-vs-nested-`>`-block form:
//!      non-empty and free of `\n` stays on the same line (`key: value` /
//!      `- value`); empty or multi-line becomes a nested `>`-block, one line
//!      per physical line, at 4 extra spaces of indent (mirroring
//!      `printer.zig`'s fixed `indent_width` — hardcoded like YAML/TOML's own
//!      `col + 2`, not sniffed from the document). This also means `set`/
//!      `replaceValAtPath` needs a real reframe (like YAML/fig's own): the
//!      NEW value's shape can differ from the OLD one's (inline <-> nested),
//!      so the whole `key`-to-value-end (or `-`-to-value-end) region gets
//!      regenerated rather than splicing into the old value's slot.
//!   2. **Sequence items have no keyvalue-shaped wrapper node.** A mapping
//!      entry's `.keyvalue` node spans from its KEY's own line (always,
//!      regardless of where the value ends up), so the generic engine's
//!      line-position math already works for mapping entries with no
//!      NestedText-specific help. A sequence item is just its bare value
//!      node re-used as the sequence's child directly — and NestedText,
//!      unlike YAML in practice, allows genuinely NOTHING after a `-` on its
//!      own line whenever the value is nested/empty (a same-line `- key:
//!      value` is parsed as the LITERAL string `"key: value"`, never a
//!      nested mapping — see `parser.zig`'s module doc, "region" algorithm).
//!      So an item's OWN span can start on a later line than its `-`. The
//!      parser records every item's `-` in `Document.node_marker_spans`, and
//!      the engine's block-sequence ops — append/prepend's prefix, remove and
//!      reorder's block boundaries, the leading-comment ops when `path` ends
//!      in `.index` — read it, so none of those is NestedText's any more.
//!      What is still this module's is RENDERING: a same-line value versus a
//!      nested `>`-block, which `renderItem` and `renderTail` decide; the
//!      engine reframes an item from its recorded `-` and an entry from its
//!      recorded `:`.
//!
//! An inline `{}`/`[]` container is a FLOW container to the engine, so an
//! insert into one is the generic comma-aware splice (`{a: 1}`), using the
//! `kv_sep` declared for exactly that. `set`'s auto-vivify (`editor.zig`)
//! still excludes NestedText: its printer never writes the inline form, so a
//! vivified ancestor would be a shape the format itself avoids.
//!
//! Out of scope: a `value_text` that's itself a nested container fragment (inserting/setting a whole new
//! sub-mapping/sub-list via CLI text) — every op here treats `value_text` as
//! a raw SCALAR string, matching NestedText's own "strings all the way down"
//! design; structural composition of brand-new nested containers isn't
//! exposed by this editor (existing containers can still be freely
//! inserted-into/deleted-from/reordered).

const std = @import("std");
const testing = std.testing;

const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const Span = @import("../../util/span.zig");
const editor = @import("../../editor.zig");
const splice = @import("../../editor/splice.zig");
const NestedText = @import("nestedtext.zig").Language;

/// The concrete editor these ops drive — the NestedText arm of the generic engine.
const NtEditor = editor.Editor(NestedText);

const lineStartBefore = splice.lineStartBefore;
const lineEndAfter = splice.lineEndAfter;
const firstNonSpace = splice.firstNonSpace;
const columnOf = splice.columnOf;

/// The nesting step, as `nestedtext.zig` declares it — the same fixed step
/// `printer.zig` writes. NestedText only requires a nested region's indent to
/// be GREATER than its parent's, but this editor never sniffs the document's
/// own convention, exactly as YAML's engine path does not.
const indent_unit: []const u8 = NestedText.syntax(.NESTEDTEXT).indent_unit;

// ── Value rendering ──────────────────────────────────────────────────────────

/// Append the tail of a `key:`/`-` line already written up to (not including)
/// its own line terminator: `" " ++ text` when `text` fits on the same line
/// (non-empty, no literal `\n`) and isn't `force_nested`; otherwise a nested
/// `>`-block, one line per physical line of `text` (an empty line becomes a
/// bare `>`), each under `child_indent` — mirroring `printer.zig`'s
/// `writeStringBlock`. Never emits a trailing newline after the last line
/// (the caller decides whether one is needed — see `ntReplaceValue`).
fn appendValueTail(allocator: std.mem.Allocator, out: *std.ArrayList(u8), child_indent: []const u8, text: []const u8, force_nested: bool) !void {
    if (!force_nested and text.len != 0 and std.mem.indexOfScalar(u8, text, '\n') == null) {
        try out.append(allocator, ' ');
        try out.appendSlice(allocator, text);
        return;
    }
    var it = std.mem.splitScalar(u8, text, '\n');
    while (it.next()) |line| {
        try out.append(allocator, '\n');
        try out.appendSlice(allocator, child_indent);
        if (line.len == 0) {
            try out.append(allocator, '>');
        } else {
            try out.appendSlice(allocator, "> ");
            try out.appendSlice(allocator, line);
        }
    }
}

/// Render `text` as a ROOT document value: a `>`-block at column 0, no
/// leading marker (the whole-document root has no `key:`/`-` to follow) and
/// always nested — a bare top-level scalar line has no grammar at all (see
/// `parser.zig`: an unrecognized `.other` line at the top level is a parse
/// error; only dict/list/string(`>`)/inline forms are valid there).
fn appendRootBlock(allocator: std.mem.Allocator, out: *std.ArrayList(u8), text: []const u8) !void {
    var it = std.mem.splitScalar(u8, text, '\n');
    var first = true;
    while (it.next()) |line| {
        if (!first) try out.append(allocator, '\n');
        first = false;
        if (line.len == 0) {
            try out.append(allocator, '>');
        } else {
            try out.appendSlice(allocator, "> ");
            try out.appendSlice(allocator, line);
        }
    }
}

/// Whether `key` needs the `: key` multiline form instead of plain `key:` —
/// ported verbatim from `printer.zig`'s `needsMultilineKey` so a freshly
/// inserted/renamed key round-trips exactly the way the printer would have
/// written it.
fn needsMultilineKey(key: []const u8) bool {
    if (key.len == 0) return true;
    const c0 = key[0];
    if (c0 == '#' or c0 == '{' or c0 == '[' or c0 == ' ' or c0 == '\t') return true;
    if ((c0 == '-' or c0 == ':' or c0 == '>') and (key.len == 1 or key[1] == ' ')) return true;
    if (std.mem.indexOfScalar(u8, key, '\n') != null) return true;
    if (std.mem.indexOf(u8, key, ": ") != null) return true;
    return false;
}

/// Append `key`'s multiline `: line` form (one `: line` — or bare `:` for an
/// empty line — per physical line of `key`), mirroring `printer.zig`'s
/// `writeMultilineKeyLines`. The FIRST line's indent is the caller's (the
/// engine has written it); every later line is prefixed with `indent`. No
/// leading/trailing newline (the caller sequences it against whatever
/// follows), matching `appendValueTail`'s convention.
fn appendMultilineKeyLines(allocator: std.mem.Allocator, out: *std.ArrayList(u8), indent: []const u8, key: []const u8) !void {
    var it = std.mem.splitScalar(u8, key, '\n');
    var first = true;
    while (it.next()) |line| {
        if (!first) {
            try out.append(allocator, '\n');
            try out.appendSlice(allocator, indent);
        }
        first = false;
        if (line.len == 0) {
            try out.append(allocator, ':');
        } else {
            try out.appendSlice(allocator, ": ");
            try out.appendSlice(allocator, line);
        }
    }
}

/// Whether `key` as WRITTEN is in the multiline `: key` form rather than the
/// plain `key:` form: its first line, once you skip its indentation, starts
/// with a `:` tag exactly like a tokenizer `.colon` line (`:` at end-of-line,
/// or `:` followed by a space) — a plain key's text can never start that way
/// (the tokenizer would have dispatched it as a multiline-key line to begin
/// with), so this check is exact, not a heuristic.
fn isMultilineKeyText(key: []const u8) bool {
    const k = std.mem.trimStart(u8, key, " \t");
    if (k.len == 0 or k[0] != ':') return false;
    if (k.len == 1) return true;
    return k[1] == ' ' or k[1] == '\n' or k[1] == '\r';
}

// ── renderEntry / renderItem ─────────────────────────────────────────────────

/// One block-mapping entry after its line's `indent`: `key:` plus the value
/// tail, or the `: key` multiline form (per `needsMultilineKey`) over a value
/// that is then always nested. Continuation lines sit at `indent` plus one
/// `indent_unit`. No trailing newline. See `editor.Editor.writeEntry`.
pub fn renderEntry(allocator: std.mem.Allocator, out: *std.ArrayList(u8), indent: []const u8, key_text: []const u8, value_text: []const u8) !void {
    var child: std.ArrayList(u8) = .empty;
    defer child.deinit(allocator);
    try child.appendSlice(allocator, indent);
    try child.appendSlice(allocator, indent_unit);
    if (needsMultilineKey(key_text)) {
        try appendMultilineKeyLines(allocator, out, indent, key_text);
        // A multiline key's value has no same-line form at all — always nested.
        try appendValueTail(allocator, out, child.items, value_text, true);
    } else {
        try out.appendSlice(allocator, key_text);
        try out.append(allocator, ':');
        try appendValueTail(allocator, out, child.items, value_text, false);
    }
}

/// One block-sequence item after its line's `indent`: `-` plus the value
/// tail (same-line, or a nested `>`-block one `indent_unit` deeper). No
/// trailing newline. See `editor.Editor.writeItem`.
pub fn renderItem(allocator: std.mem.Allocator, out: *std.ArrayList(u8), indent: []const u8, value_text: []const u8) !void {
    var child: std.ArrayList(u8) = .empty;
    defer child.deinit(allocator);
    try child.appendSlice(allocator, indent);
    try child.appendSlice(allocator, indent_unit);
    try out.append(allocator, '-');
    try appendValueTail(allocator, out, child.items, value_text, false);
}

// ── renderTail / renderKey ───────────────────────────────────────────────────

/// What follows a key: `:` and the value tail after a plain key, the tail
/// alone (always nested) after a multiline `: key` — `key_text` is the key
/// as written, so the form is read off it — or, for an empty `key_text`,
/// the DOCUMENT ROOT: a `>`-block at column 0, always nested, since a bare
/// top-level scalar line has no grammar at all (see `parser.zig`: an
/// unrecognized `.other` line at the top level is a parse error). No
/// trailing newline. See `editor.Editor.writeTail`.
pub fn renderTail(allocator: std.mem.Allocator, out: *std.ArrayList(u8), indent: []const u8, key_text: []const u8, value_text: []const u8) !void {
    if (key_text.len == 0) return appendRootBlock(allocator, out, value_text);
    var child: std.ArrayList(u8) = .empty;
    defer child.deinit(allocator);
    try child.appendSlice(allocator, indent);
    try child.appendSlice(allocator, indent_unit);
    const multiline_key = isMultilineKeyText(key_text);
    if (!multiline_key) try out.append(allocator, ':');
    try appendValueTail(allocator, out, child.items, value_text, multiline_key);
}

/// The key `new_key` spelled over the old key `old_key` (as written).
/// Plain-to-plain and multiline-to-multiline rename in place (the colon,
/// when present, sits outside the key's own span either way, so it's
/// untouched); multiline-to-plain adds the trailing `:` a plain key needs (a
/// multiline key's span carries no separator colon anywhere). A multiline
/// key's span starts at its line's indent, so that form carries `indent`
/// itself. Plain-to-multiline is declined (`error.KeyRequiresMultilineForm`):
/// when the current value is on the SAME line as the key, switching key forms
/// would also have to relocate the value onto a nested line (multiline keys
/// never have a same-line value), which is a value reframe this op doesn't
/// attempt — delete and re-insert the entry instead. See
/// `editor.Editor.replaceKeyAtPath`.
pub fn renderKey(allocator: std.mem.Allocator, out: *std.ArrayList(u8), indent: []const u8, new_key: []const u8, old_key: []const u8) !void {
    const was_multiline = isMultilineKeyText(old_key);
    const wants_multiline = needsMultilineKey(new_key);
    if (wants_multiline and !was_multiline) return error.KeyRequiresMultilineForm;
    if (wants_multiline) {
        try out.appendSlice(allocator, indent);
        try appendMultilineKeyLines(allocator, out, indent, new_key);
    } else if (was_multiline) {
        try out.appendSlice(allocator, new_key);
        try out.append(allocator, ':');
    } else {
        try out.appendSlice(allocator, new_key);
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

fn expectEdit(comptime op: []const u8, src: []const u8, args: anytype, expected: []const u8) !void {
    var ed: NtEditor = .{ .allocator = testing.allocator, .format = .NESTEDTEXT };
    try ed.init(src);
    defer ed.deinit();
    try @call(.auto, @field(NtEditor, op), .{&ed} ++ args);
    errdefer std.log.err("actual: \"{s}\"", .{ed.source.items});
    try testing.expectEqualStrings(expected, ed.source.items);
}

test "insertKey: promotes an empty document to the first root key" {
    try expectEdit("set", "", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "fig" }, "name: fig\n");
}

test "insertKey: appends after the last existing entry, matching indent" {
    try expectEdit("insertKey", "a: 1\n", .{ &[_]AST.PathSegment{}, "b", "2" }, "a: 1\nb: 2\n");
    try expectEdit(
        "insertKey",
        "server:\n    host: localhost\n",
        .{ &[_]AST.PathSegment{.{ .key = "server" }}, "port", "80" },
        "server:\n    host: localhost\n    port: 80\n",
    );
}

test "insertKey: empty/multiline value renders as a nested `>`-block" {
    try expectEdit("insertKey", "a: 1\n", .{ &[_]AST.PathSegment{}, "b", "" }, "a: 1\nb:\n    >\n");
    try expectEdit("insertKey", "a: 1\n", .{ &[_]AST.PathSegment{}, "b", "line1\nline2" }, "a: 1\nb:\n    > line1\n    > line2\n");
}

test "insertKey: a key needing multiline form gets the `: key` spelling" {
    try expectEdit("insertKey", "a: 1\n", .{ &[_]AST.PathSegment{}, "- looks like a list tag", "v" }, "a: 1\n: - looks like a list tag\n    > v\n");
}

test "insertKey: fills a childless inline `{}` through the generic flow insert" {
    try expectEdit("insertKey", "{}", .{ &[_]AST.PathSegment{}, "a", "1" }, "{a: 1}");
}

test "set: same-line scalar replace, autodetecting old shape" {
    try expectEdit("set", "name: fig\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "zig" }, "name: zig\n");
}

test "set: switches a same-line value to a nested `>`-block and back" {
    try expectEdit("set", "name: fig\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "line1\nline2" }, "name:\n    > line1\n    > line2\n");
    try expectEdit("set", "name:\n    > line1\n    > line2\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "fig" }, "name: fig\n");
}

test "set: empty value becomes the nested bare `>` block" {
    try expectEdit("set", "name: fig\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "" }, "name:\n    >\n");
    try expectEdit("set", "name:\n    >\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "fig" }, "name: fig\n");
}

test "set: replaces a whole nested container value with a scalar" {
    try expectEdit(
        "set",
        "server:\n    host: localhost\n    port: 80\n",
        .{ &[_]AST.PathSegment{.{ .key = "server" }}, "disabled" },
        "server: disabled\n",
    );
}

test "set: on a multiline key's value stays nested even for a short value" {
    try expectEdit(
        "set",
        ": key 1\n: spread over 2 lines\n    > value 1\n",
        .{ &[_]AST.PathSegment{.{ .key = "key 1\nspread over 2 lines" }}, "v" },
        ": key 1\n: spread over 2 lines\n    > v\n",
    );
}

test "replaceValAtPath: on a list item, by index" {
    // `set` only ever creates/replaces a MAPPING entry (its path must end in
    // `.key`); an `.index`-ending or path-less (root) target goes through
    // `replaceValAtPath` directly, same as every other language.
    try expectEdit("replaceValAtPath", "- a\n- b\n- c\n", .{ &[_]AST.PathSegment{.{ .index = 1 }}, "z" }, "- a\n- z\n- c\n");
}

test "replaceValAtPath: on a list item whose value is nested/empty" {
    try expectEdit(
        "replaceValAtPath",
        "- a\n-\n    nested: 1\n- c\n",
        .{ &[_]AST.PathSegment{.{ .index = 1 }}, "b" },
        "- a\n- b\n- c\n",
    );
}

test "replaceValAtPath: the whole-document root value" {
    try expectEdit("replaceValAtPath", "> hello\n", .{ &[_]AST.PathSegment{}, "goodbye" }, "> goodbye\n");
    try expectEdit("replaceValAtPath", "", .{ &[_]AST.PathSegment{}, "hi" }, "> hi\n");
}

test "replaceKeyAtPath: plain to plain" {
    try expectEdit("replaceKeyAtPath", "name: fig\n", .{ &[_]AST.PathSegment{.{ .key = "name" }}, "lang" }, "lang: fig\n");
}

test "replaceKeyAtPath: multiline to plain adds the separator colon" {
    try expectEdit(
        "replaceKeyAtPath",
        ": - looks like a list tag\n    > v\n",
        .{ &[_]AST.PathSegment{.{ .key = "- looks like a list tag" }}, "plain" },
        "plain:\n    > v\n",
    );
}

test "replaceKeyAtPath: plain to multiline is declined when the value is same-line" {
    var ed: NtEditor = .{ .allocator = testing.allocator, .format = .NESTEDTEXT };
    try ed.init("name: fig\n");
    defer ed.deinit();
    try testing.expectError(error.KeyRequiresMultilineForm, ed.replaceKeyAtPath(&.{.{ .key = "name" }}, "- oops"));
}

test "appendToSeq / prependToSeq put a scalar item on its own line" {
    try expectEdit("appendToSeq", "- a\n- b\n", .{ &[_]AST.PathSegment{}, "c" }, "- a\n- b\n- c\n");
    try expectEdit("prependToSeq", "- a\n- b\n", .{ &[_]AST.PathSegment{}, "z" }, "- z\n- a\n- b\n");
}

test "appendToSeq / prependToSeq render an empty/multiline item as a nested `>`-block" {
    try expectEdit("appendToSeq", "- a\n", .{ &[_]AST.PathSegment{}, "" }, "- a\n-\n    >\n");
    try expectEdit("prependToSeq", "- a\n", .{ &[_]AST.PathSegment{}, "l1\nl2" }, "-\n    > l1\n    > l2\n- a\n");
}

test "appendToSeq: after a nested-value first item, indent still matches the sequence" {
    try expectEdit(
        "appendToSeq",
        "-\n    nested: 1\n",
        .{ &[_]AST.PathSegment{}, "b" },
        "-\n    nested: 1\n- b\n",
    );
}

test "removeSeqItem removes a nested/empty-valued item cleanly, leaving siblings intact" {
    try expectEdit(
        "removeSeqItem",
        "- a\n-\n    nested: 1\n- c\n",
        .{ &[_]AST.PathSegment{}, @as(usize, 1) },
        "- a\n- c\n",
    );
    // Removing the first item (nested/empty) is anchored via the sequence's
    // own span, not a (nonexistent) previous sibling.
    try expectEdit(
        "removeSeqItem",
        "-\n    nested: 1\n- b\n",
        .{ &[_]AST.PathSegment{}, @as(usize, 0) },
        "- b\n",
    );
}

test "removeSeqItem carries a leading comment above a nested item" {
    try expectEdit(
        "removeSeqItem",
        "- a\n# note\n-\n    nested: 1\n- c\n",
        .{ &[_]AST.PathSegment{}, @as(usize, 1) },
        "- a\n- c\n",
    );
}

test "moveItem / reorderItems relocate whole (possibly nested) item blocks" {
    try expectEdit(
        "moveItem",
        "- a\n-\n    nested: 1\n- c\n",
        .{ &[_]AST.PathSegment{}, @as(usize, 2), @as(usize, 0) },
        "- c\n- a\n-\n    nested: 1\n",
    );
    try expectEdit(
        "reorderItems",
        "- a\n- b\n- c\n",
        .{ &[_]AST.PathSegment{}, &[_]usize{ 2, 0 } },
        "- c\n- a\n- b\n",
    );
}

test "addLeadingComment / getLeadingComment / deleteLeadingComments on a nested-valued list item" {
    var ed: NtEditor = .{ .allocator = testing.allocator, .format = .NESTEDTEXT };
    try ed.init("- a\n-\n    nested: 1\n- c\n");
    defer ed.deinit();
    const path = &[_]AST.PathSegment{.{ .index = 1 }};
    try ed.addLeadingComment(path, "note");
    try testing.expectEqualStrings("- a\n# note\n-\n    nested: 1\n- c\n", ed.source.items);
    const got = (try ed.getLeadingComment(path)).?;
    defer testing.allocator.free(got);
    try testing.expectEqualStrings("note", got);
    try ed.deleteLeadingComments(path);
    try testing.expectEqualStrings("- a\n-\n    nested: 1\n- c\n", ed.source.items);
}

test "trailing comments are unsupported (no same-line comment spelling)" {
    var ed: NtEditor = .{ .allocator = testing.allocator, .format = .NESTEDTEXT };
    try ed.init("name: fig\n");
    defer ed.deinit();
    const path = &[_]AST.PathSegment{.{ .key = "name" }};
    try testing.expectError(error.CommentsUnsupported, ed.setTrailingComment(path, "note"));
    try testing.expectError(error.CommentsUnsupported, ed.getTrailingComment(path));
    try testing.expectError(error.CommentsUnsupported, ed.deleteTrailingComment(path));
}

test "deleteKey removes a whole nested-value entry (generic engine, no override needed)" {
    try expectEdit(
        "deleteKey",
        "a: 1\nserver:\n    host: localhost\n    port: 80\nb: 2\n",
        .{&[_]AST.PathSegment{.{ .key = "server" }}},
        "a: 1\nb: 2\n",
    );
}
