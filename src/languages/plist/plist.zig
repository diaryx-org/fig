const plist = @This();
const std = @import("std");
const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const Writer = std.Io.Writer;
const lang = @import("../manifest.zig");

pub const Parser = @import("parser.zig");
pub const Printer = @import("printer.zig");

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
/// `AST.SerializeFormat` member (`.plist`), so `ast.serialize` routes here like
/// every other format. It also HAS an in-place (span-splicing) editor —
/// `Editor(Plist)` via `editor_helper.zig` — so `fig edit`/`set`/`insert`/
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

    pub fn parse(parser: *plist.Parser, input: []const u8, format: plist.Type) !Document {
        return plist.Parser.parse(parser.allocator, input, format);
    }

    pub fn print(writer: *Writer, ast: *const AST) !void {
        return plist.Printer.print(writer, ast, .{});
    }

    pub const name = "plist";
    pub const extensions: []const []const u8 = &.{"plist"};
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    /// What `languages/harness.zig` round-trips and edits: a dict with a
    /// string, an integer and an array.
    pub const samples: []const []const u8 = &.{
        "<plist version=\"1.0\"><dict><key>a</key><string>b</string><key>n</key><integer>1</integer><key>l</key><array><true/><false/></array></dict></plist>\n",
    };

    /// Genuinely typed and nested (dict/array/string/integer/real/bool, with
    /// date/data carried on the `extended` scalar) — the one XML-shaped
    /// format that is also a full value model.
    pub const dialects: []const lang.Dialect(@This()) = &.{.{
        .name = "plist",
        .abi_value = 12,
        // After ZON and before the key/value grammars, none of which accept a
        // `<tag>`. Rank 4 was generic XML's until core 3.0 removed it; the
        // gap is left so that any typed XML flavor added later (a `.csproj`
        // reader, a manifest reader) has a slot in the XML-shaped part of the
        // order without renumbering. plist's own grammar rejects anything
        // outside its fixed element vocabulary (`error.UnknownElement`), so
        // it cannot starve such a flavor.
        .sniff_rank = 3,
        .splice = .raw,
        // A bare `<dict>` IS a document this parser accepts (see its `detect`
        // probe), so `fig set` on a nonexistent `.plist` can create one.
        .empty_doc_seed = "<dict>\n</dict>\n",
    }};

    /// plist declares the least of any editable format, because it delegates
    /// the most: an entry is a PAIR of sibling elements (`<key>k</key>` then
    /// a typed value element), not a `key<sep>value` line, so almost nothing
    /// in the line-oriented generic engine applies and the structural ops go
    /// wholesale to `plist/editor_helper.zig`. What is declared here is what
    /// the line-based delete/remove paths — which do ride the generic code —
    /// actually consult.
    pub fn syntax(t: plist.Type) lang.Syntax {
        _ = t;
        return .{
            // A comment is the `<!-- … -->` PAIR, leading and trailing alike,
            // and XML forbids `--` inside one. The owned-block scanner
            // (`.xml_comment`) recognizes an own-line pair; the leading and
            // trailing ops write and strip the pair; the dangling and
            // comment-out ops, which need a bare prefix, refuse it.
            .comments = .{
                .style = .xml_comment,
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
    // logic lives in `editor_helper.zig` (which holds this format's editor
    // tests too).
    const edit = @import("editor_helper.zig");

    /// A CLI value string rendered into a typed element: fig `sniffBare`
    /// typing, or spliced verbatim when it already looks like `<…>`. The
    /// engine passes every value it splices through this.
    pub const renderValue = edit.renderValue;

    /// `<key>k</key>`, a newline, the indent, and the rendered value.
    pub const renderEntry = edit.renderEntry;
};

// Test discovery: importing `plist.zig` (from root.zig) pulls in every plist
// submodule's tests, so the module owns its own test surface.
test {
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_helper.zig");
    // The shared XML lexing substrate has no entry module of its own since
    // generic XML stopped being a format (core 3.0); plist is its one consumer
    // in tree, so plist discovers its tests.
    _ = @import("../xml/tokenizer.zig");
}
