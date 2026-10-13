const plist = @This();
const std = @import("std");
const AST = @import("kit").AST;
const Document = @import("kit").Document;
const Table = @import("kit").table.Table;
const Writer = std.Io.Writer;
const lang = @import("kit").manifest;

pub const Parser = @import("parser.zig");
pub const Printer = @import("printer.zig");
/// The renderers: how this language spells what the editor splices.
pub const edit = @import("edit.zig");

pub const Type = enum {
    /// Apple's XML property list — `<plist><dict>...</dict></plist>`, the
    /// only variant this reader/printer supports so far. Old-style ASCII
    /// (NeXTSTEP/OpenStep `{ key = value; }`) and binary plist (`bplist00…`)
    /// are separate, larger efforts (a different grammar, and for binary, a
    /// wholly different byte-level format) — see `parser.zig`'s module doc.
    XML,
};

/// plist reads into the shared AST (so plist converts *into* JSON/YAML/TOML/
/// ZON/…) and writes back out via `Printer` — the documented inverse mapping
/// described in `parser.zig`'s header (a `dict` is a real mapping, an `array`
/// a real sequence, `date`/`data` ride the `extended` scalar). It IS an
/// `Serialize.SerializeFormat` member (`.plist`), so core's serializer routes here like
/// every other format. It also HAS an in-place (span-splicing) editor —
/// `Editor(Plist)` via `edit.zig` — so `fig replace`/`rename`/`set`/`insert`/
/// `delete`/`comment` work on a `.plist`; unlike the line-oriented formats it
/// renders typed value elements (fig `sniffBare` typing) and uses `<!-- -->`
/// comments. The generic XML format (`.xml`) still has none — a document syntax
/// whose ambiguous edit surface (attributes vs text vs mixed content) is a
/// separate, deferred effort; plist works because its DTD gives every element a
/// fixed, unambiguous typed meaning.
pub const Language = struct {
    pub const Type = plist.Type;
    pub const Parser = plist.Parser;
    pub const Printer = plist.Printer;
    pub const default_type: plist.Type = .XML;

    /// Write `input`'s rows into `out`, the table fig hands a parse.
    pub fn parseTable(parser: *plist.Parser, input: []const u8, format: plist.Type, out: *Table) !void {
        _ = parser;
        return plist.Parser.parseInto(out, input, format);
    }
    /// The document fig would adopt from a parse.
    pub fn parse(parser: *plist.Parser, input: []const u8, format: plist.Type) !Document {
        return plist.Parser.parse(parser.allocator, input, format);
    }

    pub fn print(writer: *Writer, ast: *const AST) !void {
        return plist.Printer.print(writer, ast, .{});
    }

    pub const name = "plist";
    pub const extensions: []const []const u8 = &.{"plist"};
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    /// What fig's `src/contract/harness.zig` round-trips and edits: a dict with
    /// a string, an integer and an array.
    pub const samples: []const []const u8 = &.{
        "<plist version=\"1.0\"><dict><key>a</key><string>b</string><key>n</key><integer>1</integer><key>l</key><array><true/><false/></array></dict></plist>\n",
    };

    /// Genuinely typed and nested (dict/array/string/integer/real/bool, with
    /// date/data carried on the `extended` scalar) — the one XML-shaped
    /// format that is also a full value model.
    pub const dialects: []const lang.Dialect(@This()) = &.{.{
        .name = "plist",
        .prints = .{ .holds = .{ .plist_date = true, .plist_data = true }, .comments = .never },
        .splice = .raw,
        // A bare `<dict>` IS a document this parser accepts (see its `detect`
        // probe), so `fig set` on a nonexistent `.plist` can create one.
        .empty_doc_seed = "<dict>\n</dict>\n",
    }};

    /// plist declares the least of any editable format, because it delegates
    /// the most: an entry is a PAIR of sibling elements (`<key>k</key>` then
    /// a typed value element), not a `key<sep>value` line, so almost nothing
    /// in the line-oriented generic engine applies and the structural ops go
    /// wholesale to `plist/edit.zig`. What is declared here is what
    /// the line-based delete/remove paths — which do ride the generic code —
    /// actually consult.
    pub fn syntax(t: plist.Type) lang.Syntax {
        _ = t;
        return .{
            // A comment is the `<!-- … -->` PAIR, leading and trailing alike,
            // and XML forbids `--` inside one. The owned-block scan
            // recognizes an own-line pair; the leading and
            // trailing ops write and strip the pair; the dangling and
            // comment-out ops, which need a bare prefix, refuse it.
            .comments = .{
                .line = .{ .open = "<!--", .close = "-->", .forbidden = "--" },
                .trailing = .{ .open = "<!--", .close = "-->", .forbidden = "--" },
            },
            // An entry is a PAIR OF SIBLING ELEMENTS, not a `key<sep>value`
            // line, so no generic path writes one. See `Syntax.kv_sep`.
            .kv_sep = null,
            // No bare literal for an empty dict that the generic seed could
            // splice — a value is always a typed wrapper element.
            .empty_map_literal = null,
            // A `<dict>`/`<array>` is an element, never an inline `{…}`, and
            // it closes itself, so a trailing comment on one follows
            // `</dict>` rather than riding the `<key>` line.
            .flow_containers = false,
            .closed_containers = .{
                .map = .{ .open = "<dict>", .close = "</dict>" },
                .seq = .{ .open = "<array>", .close = "</array>" },
            },
            // An item is a bare element on its own line.
            .seq_item_marker = "",
            // An empty array is an element too, which the closed-container
            // insert expands around its first item.
            .empty_seq_literal = "<array/>",
        };
    }

    // ── Renderers ────────────────────────────────────────────────────────────
    //
    // No hooks. Everything structural about editing a plist is the generic
    // engine's, driven by `syntax` above; what the engine cannot know is how
    // this format SPELLS a value and an entry, and those are the two string
    // renderers below. A value is always a typed element, never a bare
    // literal, and an entry is a PAIR OF SIBLING ELEMENTS (`<key>k</key>` then
    // the value element) on two lines rather than a `key<sep>value` line. The
    // logic lives in `edit.zig` (and `tests/editor.zig` holds this format's editor
    // tests too).

    /// A CLI value string rendered into a typed element: fig `sniffBare`
    /// typing, or spliced verbatim when it already looks like `<…>`. The
    /// engine passes every value it splices through this.
    pub const renderValue = edit.renderValue;

    /// `<key>k</key>`, a newline, the indent, and the rendered value.
    pub const renderEntry = edit.renderEntry;

    /// A renamed key, as `<key>k</key>`: the key's span is the whole element.
    pub const renderKey = edit.renderKey;
};

// The package's own tests (`zig build test`): every file's. Its editor
// tests need fig itself, which a language cannot depend on, so they are
// `tests/editor.zig`, which fig's build compiles.
test {
    _ = @import("parser.zig");
    _ = @import("printer.zig");
}
