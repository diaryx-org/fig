//! plist-specific editing helpers for `Editor(Plist)`.
//!
//! The generic span-splice engine lives in `../../editor.zig`; this module holds
//! the plist-only logic it delegates to. plist needs far MORE of its own logic
//! than TOML/YAML/fig/INI, because it is the one editable format that isn't
//! line-oriented `key<sep>value`: it's XML, so a dict entry is a *pair of
//! sibling elements* — `<key>name</key>` then a typed value element
//! (`<string>fig</string>`) — on two separate lines, and a value has no bare
//! literal spelling at all. That breaks every assumption the generic block-map
//! helpers make (`kv_sep`, one-line entries, splice-a-literal-value), so the
//! two string renderers below say how plist spells them, and the generic
//! engine does the rest:
//!
//!   - `renderValue`: the crux. A CLI value string (`fig set app.plist k=42`)
//!     has no plist meaning until it's wrapped in a typed element. We reuse the
//!     `.fig` dialect's own literal-else-string classifier — the engine's
//!     `literalOf`, handed in as `Literal` — to pick
//!     the type — `true`/`false` → `<true/>`/`<false/>`, integer → `<integer>`,
//!     float → `<real>`, any datetime shape → `<date>`, everything else →
//!     `<string>` (XML-escaped). `null` has no plist type (`NullUnsupported`).
//!     A value that already starts with `<` is spliced VERBATIM — the escape
//!     hatch for `<data>` (which can't be sniffed from bare text), an explicit
//!     `<date>`, a nested `<dict>`/`<array>`, or forcing a type (`<string>2.0`).
//!   - `renderEntry`: a dict entry is two lines, `<key>k</key>` then the
//!     value element at the same indent.
//!
//! Replace, insert, append and prepend are the engine's: it renders the value
//! through `renderValue`, writes an entry through `renderEntry` or an item as
//! the bare element (`seq_item_marker = ""`), and expands an empty `<dict/>`
//! or `<array/>` from `Syntax.closed_containers`.
//!
//! Delete-key, remove-seq-item and every comment op are generic too.
//! Once `comments` is `.xml_comment` (see `editor.zig`), the generic
//! line-based delete already does the right thing — a plist entry/item
//! occupies whole lines, and the keyvalue's full-extent span (recorded by
//! `parser.zig`) covers both the key and value lines, so
//! `lineStartBefore(span.start)`→`lineEndAfter(span.end)` removes the entry
//! cleanly, owned `<!-- -->` block and all. And `<!-- … -->` is declared as a
//! `CommentDelimiter` pair in `plist.zig`, so the generic leading and
//! trailing comment ops write and strip it themselves. No plist branch
//! needed for any of them.

const std = @import("std");

const AST = @import("../../kit/ast/ast.zig");
const Document = @import("../../kit/document.zig");
const Span = @import("../../kit/span.zig");
const splice = @import("../../kit/lines.zig");
const Plist = @import("plist.zig").Language;
// The `.fig` dialect's bare-token classifier — reused so plist value typing
// obeys the exact same literal-else-string rules the fig language documents
// (Norway-safe booleans, leading-zero-stays-string, datetime sniffing). Pure
// functions on caller-owned slices; compiles regardless of `-Dfig`.
const lang = @import("../../kit/manifest.zig");

// ── value rendering ────────────────────────────────────────────────────────────

/// Render a CLI value string into a plist typed element, appended to `out`.
/// See the module header for the typing rules and the `<`-prefix escape hatch.
pub fn renderValue(_: Plist.Type, allocator: std.mem.Allocator, out: *std.ArrayList(u8), r: lang.RenderRequest) !void {
    const value_text = r.value;
    const literal = r.literal;
    const t = std.mem.trim(u8, value_text, " \t\r\n");
    if (t.len > 0 and t[0] == '<') {
        // Explicit element (or element tree): the caller has spelled the plist
        // syntax themselves — splice it as-is. Reparse validates it; a bogus
        // `<foo` rolls back via `replaceAtSpan`.
        try out.appendSlice(allocator, t);
        return;
    }
    switch (literal) {
        .null => return error.NullUnsupported, // plist has no null primitive
        .bool => try out.appendSlice(allocator, if (std.mem.eql(u8, t, "true")) "<true/>" else "<false/>"),
        .int => try wrapText(allocator, out, "integer", t, false),
        .float => try wrapText(allocator, out, "real", t, false),
        .datetime => try wrapText(allocator, out, "date", t, false),
        .string => try wrapText(allocator, out, "string", t, true),
    }
}

/// `<tag>text</tag>`, XML-escaping `text` when `escape` (PCDATA content, i.e.
/// strings/keys — numbers and dates are already lexically safe).
fn wrapText(allocator: std.mem.Allocator, out: *std.ArrayList(u8), tag: []const u8, text: []const u8, escape: bool) !void {
    try out.append(allocator, '<');
    try out.appendSlice(allocator, tag);
    try out.append(allocator, '>');
    if (escape) try appendEscaped(allocator, out, text) else try out.appendSlice(allocator, text);
    try out.appendSlice(allocator, "</");
    try out.appendSlice(allocator, tag);
    try out.append(allocator, '>');
}

/// Escape `&`/`<`/`>` in PCDATA content (mirrors the printer's `writeEscaped`).
fn appendEscaped(allocator: std.mem.Allocator, out: *std.ArrayList(u8), s: []const u8) !void {
    for (s) |c| switch (c) {
        '&' => try out.appendSlice(allocator, "&amp;"),
        '<' => try out.appendSlice(allocator, "&lt;"),
        '>' => try out.appendSlice(allocator, "&gt;"),
        else => try out.append(allocator, c),
    };
}

// ── entry rendering ────────────────────────────────────────────────────────────

/// `<key>key</key>\n<indent><value…>` — a dict entry's two lines, the key
/// and value at the same indent (matching the printer's layout). The first
/// line's indent is the engine's; `rendered_value` has been through
/// `renderValue`, and a container's further lines (a `<dict>` fragment's
/// entries and close tag) are moved under `indent` as well. See
/// `editor.Editor.writeEntry`.
pub fn renderEntry(_: Plist.Type, allocator: std.mem.Allocator, out: *std.ArrayList(u8), r: lang.RenderRequest) !void {
    const indent = r.indent;
    const key = r.key;
    const rendered_value = r.value;
    try out.appendSlice(allocator, "<key>");
    try appendEscaped(allocator, out, key);
    try out.appendSlice(allocator, "</key>\n");
    try out.appendSlice(allocator, indent);
    var lines = std.mem.splitScalar(u8, rendered_value, '\n');
    try out.appendSlice(allocator, lines.first());
    while (lines.next()) |line| {
        try out.append(allocator, '\n');
        if (line.len > 0) try out.appendSlice(allocator, indent);
        try out.appendSlice(allocator, line);
    }
}

/// A renamed key: `<key>new_key</key>`, escaped. A key's span is the whole
/// element, so a bare name spliced over it would drop the tags. See
/// `editor.Editor.replaceKeyAtPath`.
pub fn renderKey(_: Plist.Type, allocator: std.mem.Allocator, out: *std.ArrayList(u8), r: lang.RenderRequest) !void {
    const new_key = r.key;
    try out.appendSlice(allocator, "<key>");
    try appendEscaped(allocator, out, new_key);
    try out.appendSlice(allocator, "</key>");
}
