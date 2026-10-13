//! The node table: what a language's `parse` writes and its `print` reads,
//! as ABI 4 spells it (docs/proposals/core-only.md §4.9).
//!
//! Fig owns the table. `parse` is handed one fig allocated and fills it
//! through fig's calls — `addRows` hands back rows to write in place,
//! `addString` copies decoded text into the table's buffer, and each side
//! fact is appended by a call of its own — so no memory crosses from a
//! language's allocator to fig's. `print` is handed one fig filled, and
//! reads it through a `View`.
//!
//! A row is 32 bytes. A language writes its `parent`; fig derives the child
//! links (`first_child`, `next_sibling`) while it checks the table. Every
//! side fact is sparse: a document with no anchors, tags or comments has no
//! row in those lists. Every offset is a `u32`, so a source is at most
//! 4 GiB.
//!
//! This module is a leaf: it names nothing of core's, so the same types are
//! the C ABI's (`fig.h`'s `FigRow`, `FigText`, `FigTableView`, …).

const std = @import("std");
const Allocator = std.mem.Allocator;

/// No node, no offset: an absent parent, span or text.
pub const none: u32 = std.math.maxInt(u32);

/// A row's text: `len` bytes at `off` in the source, or — `len`'s high bit
/// set — in the table's string buffer. `len == none` is no text at all,
/// which is not the empty string.
pub const Text = extern struct {
    off: u32 = 0,
    len: u32 = none,

    pub const absent: Text = .{};
    pub const buffer_bit: u32 = 1 << 31;

    /// `source[off .. off + len]`.
    pub fn inSource(off: usize, len: usize) Text {
        return .{ .off = @intCast(off), .len = @intCast(len) };
    }

    pub fn isAbsent(self: Text) bool {
        return self.len == none;
    }
};

/// `Row.kind`: a scalar kind, a container kind, or `ext + ExtKind`.
pub const Kind = struct {
    pub const null_: u8 = 0;
    pub const false_: u8 = 1;
    pub const true_: u8 = 2;
    pub const string: u8 = 3;
    pub const int: u8 = 4;
    pub const float: u8 = 5;
    pub const sequence: u8 = 6;
    pub const mapping: u8 = 7;
    pub const keyvalue: u8 = 8;
    pub const alias: u8 = 9;
    /// An extended scalar: `ext` plus the `ExtKind` ordinal.
    pub const ext: u8 = 16;

    /// Whether a row of this kind carries text: a string's decoded bytes,
    /// a number's lexeme, an alias's anchor name, an extended scalar's
    /// payload.
    pub fn hasText(k: u8) bool {
        return k == string or k == int or k == float or k == alias or k >= ext;
    }

    pub fn isContainer(k: u8) bool {
        return k == sequence or k == mapping or k == keyvalue;
    }
};

/// `Row.flags`: a sequence with no delimiters, whose items run from its
/// span's start to its end.
pub const row_open: u8 = 1 << 0;

/// One node. Row index is node id; rows are in pre-order, so a parent
/// precedes its children, and a keyvalue's two children are its key and
/// then its value.
pub const Row = extern struct {
    kind: u8 = Kind.null_,
    flags: u8 = 0,
    reserved: u16 = 0,
    /// The parent's row, or `none` for the root, which is row 0.
    parent: u32 = none,
    /// The node's span in the source; `none` on the print side, where there
    /// is no source.
    start: u32 = none,
    end: u32 = none,
    text: Text = .{},
    /// Filled by fig while it checks the table; a language leaves them.
    first_child: u32 = none,
    next_sibling: u32 = none,
};

comptime {
    std.debug.assert(@sizeOf(Row) == 32);
}

/// An anchor defined on `node`, and where its `&name` is written.
pub const Anchor = extern struct { node: u32, start: u32 = none, end: u32 = none, name: Text };

/// `Tag.resolved`: what the language made of a tag.
pub const tag_unstated: u8 = 0;
/// A tag with no core meaning (`!foo`), kept for a language that spells it.
pub const tag_custom: u8 = 1;
/// The non-specific tag (YAML's `!`), which asserts nothing.
pub const tag_non_specific: u8 = 2;
/// A core type: `tag_kind + KindTag ordinal` (null, boolean, string,
/// integer, float, sequence, mapping).
pub const tag_kind: u8 = 3;

/// A tag on `node`: its text as written, for a language that re-emits it,
/// what the language resolved it to (`tag_*`), and whether the node's text
/// agrees with that resolution (`!!int` on `1 - 3` does not). Where the
/// `!tag` is written, or `none`.
pub const Tag = extern struct {
    node: u32,
    start: u32 = none,
    end: u32 = none,
    text: Text,
    resolved: u8 = tag_unstated,
    agrees: u8 = 1,
    reserved: u16 = 0,
};

/// A fact with a span: a block-sequence item's marker, a keyvalue's
/// separator, a section's header line.
pub const SpanFact = extern struct { node: u32, start: u32, end: u32 };

/// The bytes a separated sequence writes between two items on one line,
/// or absent for the dialect's `flow_entry_sep`.
pub const ItemSep = extern struct { node: u32, text: Text };

pub const comment_leading: u8 = 0;
pub const comment_trailing: u8 = 1;
pub const comment_dangling: u8 = 2;
pub const comment_line: u8 = 0;
pub const comment_block: u8 = 1;

/// A comment bound to `node`, in its `slot`, in source order within it; at
/// most one trailing per node.
pub const Comment = extern struct { node: u32, slot: u8, style: u8, reserved: u16 = 0, text: Text };

pub const mention_header: u8 = 0;
pub const mention_entry: u8 = 1;
pub const mention_index: u8 = 2;
pub const mention_append: u8 = 3;

/// A place a section node's name is written: a header line of its own, or
/// its parent's entry line (a dotted key). Or a place a sequence item's
/// path is written: its index (`mention_index`), or the header that
/// appended it (`mention_append`). See `Document.MentionKind`.
pub const Mention = extern struct { node: u32, start: u32, end: u32, kind: u8, reserved: [3]u8 = .{ 0, 0, 0 } };

/// A tag-handle declaration of the document (YAML's `%TAG`).
pub const Directive = extern struct { handle: Text, prefix: Text };

pub const diagnostic_error: u8 = 0;
pub const diagnostic_warning: u8 = 1;

/// What a parse found worth telling: an error (a parse that fails may still
/// leave the ones it found) or a warning (a lint on input that parses), its
/// span — `end` may be `none` for a point — a sentence that says what is
/// wrong and how to fix it, and a few words for the caret's label.
pub const Diagnostic = extern struct {
    severity: u8,
    reserved: [3]u8 = .{ 0, 0, 0 },
    start: u32,
    end: u32 = none,
    message: Text,
    label: Text = .{},
};

/// What a `print` is handed, and what any reader of a table reads: every
/// list as a pointer and a count. `size` is fig's `sizeof`, so a reader
/// built against an older header reads the prefix it knows.
pub const View = extern struct {
    size: u32 = @sizeOf(View),
    source: ?[*]const u8 = null,
    source_len: usize = 0,
    strings: ?[*]const u8 = null,
    strings_len: usize = 0,
    rows: ?[*]const Row = null,
    row_count: usize = 0,
    anchors: ?[*]const Anchor = null,
    anchor_count: usize = 0,
    tags: ?[*]const Tag = null,
    tag_count: usize = 0,
    markers: ?[*]const SpanFact = null,
    marker_count: usize = 0,
    seps: ?[*]const SpanFact = null,
    sep_count: usize = 0,
    item_seps: ?[*]const ItemSep = null,
    item_sep_count: usize = 0,
    comments: ?[*]const Comment = null,
    comment_count: usize = 0,
    regions: ?[*]const SpanFact = null,
    region_count: usize = 0,
    mentions: ?[*]const Mention = null,
    mention_count: usize = 0,
    directives: ?[*]const Directive = null,
    directive_count: usize = 0,
    diagnostics: ?[*]const Diagnostic = null,
    diagnostic_count: usize = 0,

    pub fn rowSlice(self: *const View) []const Row {
        return slice(Row, self.rows, self.row_count);
    }

    /// The bytes `t` names, or null when it is absent or out of range.
    pub fn textOf(self: *const View, t: Text) ?[]const u8 {
        const source = if (self.source) |p| p[0..self.source_len] else "";
        const strings = if (self.strings) |p| p[0..self.strings_len] else "";
        return viewText(source, strings, t);
    }

    /// Row `node`'s side facts, each found by binary search: every list is
    /// in node order, as every writer in fig appends it.
    pub fn facts(self: *const View, node: u32) Facts {
        return .{
            .anchor = factOf(Anchor, slice(Anchor, self.anchors, self.anchor_count), node),
            .tag = factOf(Tag, slice(Tag, self.tags, self.tag_count), node),
            .marker = factOf(SpanFact, slice(SpanFact, self.markers, self.marker_count), node),
            .sep = factOf(SpanFact, slice(SpanFact, self.seps, self.sep_count), node),
            .item_sep = factOf(ItemSep, slice(ItemSep, self.item_seps, self.item_sep_count), node),
        };
    }
};

/// A row's side facts, gathered (`View.facts`).
pub const Facts = struct {
    anchor: ?Anchor = null,
    tag: ?Tag = null,
    marker: ?SpanFact = null,
    sep: ?SpanFact = null,
    item_sep: ?ItemSep = null,
};

/// The first fact of `items` naming `node`, by binary search over a list in
/// node order.
pub fn factOf(comptime F: type, items: []const F, node: u32) ?F {
    var lo: usize = 0;
    var hi: usize = items.len;
    while (lo < hi) {
        const mid = lo + (hi - lo) / 2;
        if (items[mid].node < node) lo = mid + 1 else hi = mid;
    }
    return if (lo < items.len and items[lo].node == node) items[lo] else null;
}

/// The table itself, fig's. Opaque to a language, which writes it through
/// the calls below (`fig_table_*` in C) and reads it through `view`.
pub const Table = struct {
    allocator: Allocator,
    /// What row spans and source texts index; empty on the print side.
    source: []const u8 = "",
    rows: std.ArrayList(Row) = .empty,
    strings: std.ArrayList(u8) = .empty,
    anchors: std.ArrayList(Anchor) = .empty,
    tags: std.ArrayList(Tag) = .empty,
    markers: std.ArrayList(SpanFact) = .empty,
    seps: std.ArrayList(SpanFact) = .empty,
    item_seps: std.ArrayList(ItemSep) = .empty,
    comments: std.ArrayList(Comment) = .empty,
    regions: std.ArrayList(SpanFact) = .empty,
    mentions: std.ArrayList(Mention) = .empty,
    directives: std.ArrayList(Directive) = .empty,
    diagnostics: std.ArrayList(Diagnostic) = .empty,

    pub fn init(allocator: Allocator, source: []const u8) Table {
        return .{ .allocator = allocator, .source = source };
    }

    pub fn deinit(self: *Table) void {
        const a = self.allocator;
        self.rows.deinit(a);
        self.strings.deinit(a);
        self.anchors.deinit(a);
        self.tags.deinit(a);
        self.markers.deinit(a);
        self.seps.deinit(a);
        self.item_seps.deinit(a);
        self.comments.deinit(a);
        self.regions.deinit(a);
        self.mentions.deinit(a);
        self.directives.deinit(a);
        self.diagnostics.deinit(a);
        self.* = undefined;
    }

    /// A copy of every list, in `allocator`.
    pub fn clone(self: *const Table, allocator: Allocator) Allocator.Error!Table {
        var out = Table.init(allocator, self.source);
        errdefer out.deinit();
        inline for (.{ "rows", "strings", "anchors", "tags", "markers", "seps", "item_seps", "comments", "regions", "mentions", "directives", "diagnostics" }) |name| {
            try @field(out, name).appendSlice(allocator, @field(self, name).items);
        }
        return out;
    }

    /// `n` new rows, each a null with no parent, span or text, to be filled
    /// in place. Valid until the next `addRows`.
    pub fn addRows(self: *Table, n: usize) Allocator.Error![]Row {
        if (self.rows.items.len + n >= none) return error.OutOfMemory;
        const out = try self.rows.addManyAsSlice(self.allocator, n);
        @memset(out, .{});
        return out;
    }

    /// One new row: `addRows(1)[0]`, returning its index.
    pub fn addRow(self: *Table, row: Row) Allocator.Error!u32 {
        const i: u32 = @intCast(self.rows.items.len);
        (try self.addRows(1))[0] = row;
        return i;
    }

    /// `bytes` copied into the string buffer.
    pub fn addString(self: *Table, bytes: []const u8) Allocator.Error!Text {
        if (bytes.len >= Text.buffer_bit or self.strings.items.len + bytes.len >= Text.buffer_bit) return error.OutOfMemory;
        const off: u32 = @intCast(self.strings.items.len);
        try self.strings.appendSlice(self.allocator, bytes);
        return .{ .off = off, .len = @as(u32, @intCast(bytes.len)) | Text.buffer_bit };
    }

    /// `bytes` as text: in the source, when `bytes` lies within it, else
    /// copied into the buffer.
    pub fn text(self: *Table, bytes: []const u8) Allocator.Error!Text {
        if (self.source.len > 0 and bytes.len > 0) {
            const base = @intFromPtr(self.source.ptr);
            const at = @intFromPtr(bytes.ptr);
            if (at >= base and at + bytes.len <= base + self.source.len) return Text.inSource(at - base, bytes.len);
        }
        return self.addString(bytes);
    }

    /// The bytes `t` names, or null when it is absent or out of range.
    pub fn textOf(self: *const Table, t: Text) ?[]const u8 {
        return viewText(self.source, self.strings.items, t);
    }

    pub fn view(self: *const Table) View {
        return .{
            .source = if (self.source.len == 0) null else self.source.ptr,
            .source_len = self.source.len,
            .strings = if (self.strings.items.len == 0) null else self.strings.items.ptr,
            .strings_len = self.strings.items.len,
            .rows = ptrOf(Row, self.rows.items),
            .row_count = self.rows.items.len,
            .anchors = ptrOf(Anchor, self.anchors.items),
            .anchor_count = self.anchors.items.len,
            .tags = ptrOf(Tag, self.tags.items),
            .tag_count = self.tags.items.len,
            .markers = ptrOf(SpanFact, self.markers.items),
            .marker_count = self.markers.items.len,
            .seps = ptrOf(SpanFact, self.seps.items),
            .sep_count = self.seps.items.len,
            .item_seps = ptrOf(ItemSep, self.item_seps.items),
            .item_sep_count = self.item_seps.items.len,
            .comments = ptrOf(Comment, self.comments.items),
            .comment_count = self.comments.items.len,
            .regions = ptrOf(SpanFact, self.regions.items),
            .region_count = self.regions.items.len,
            .mentions = ptrOf(Mention, self.mentions.items),
            .mention_count = self.mentions.items.len,
            .directives = ptrOf(Directive, self.directives.items),
            .directive_count = self.directives.items.len,
            .diagnostics = ptrOf(Diagnostic, self.diagnostics.items),
            .diagnostic_count = self.diagnostics.items.len,
        };
    }
};

fn ptrOf(comptime T: type, items: []const T) ?[*]const T {
    return if (items.len == 0) null else items.ptr;
}

/// The bytes `t` names in `source` or `strings`, or null when it is absent
/// or out of range.
pub fn viewText(source: []const u8, strings: []const u8, t: Text) ?[]const u8 {
    if (t.isAbsent()) return null;
    if (t.len & Text.buffer_bit != 0) {
        const len = t.len & ~Text.buffer_bit;
        if (@as(u64, t.off) + len > strings.len) return null;
        return strings[t.off..][0..len];
    }
    if (@as(u64, t.off) + t.len > source.len) return null;
    return source[t.off..][0..t.len];
}

/// A view's lists as slices.
pub fn slice(comptime T: type, ptr: ?[*]const T, count: usize) []const T {
    const p = ptr orelse return &.{};
    return p[0..count];
}

test "a text names the source or the buffer, and an absent one is neither" {
    var t = Table.init(std.testing.allocator, "hello world");
    defer t.deinit();
    const src = try t.text(t.source[6..11]);
    try std.testing.expectEqual(@as(u32, 6), src.off);
    try std.testing.expectEqualStrings("world", t.textOf(src).?);
    const buf = try t.text("decoded");
    try std.testing.expect(buf.len & Text.buffer_bit != 0);
    try std.testing.expectEqualStrings("decoded", t.textOf(buf).?);
    try std.testing.expect(t.textOf(.absent) == null);
    try std.testing.expectEqualStrings("", t.textOf(try t.addString("")).?);
}
