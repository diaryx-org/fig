const nestedtext = @This();
const Document = @import("../../kit/parse_tree.zig");
const lang = @import("../../kit/manifest.zig");

pub const Parser = @import("parser.zig");
pub const Tokenizer = @import("tokenizer.zig");
pub const Printer = @import("printer.zig");

pub const Type = enum {
    /// NestedText (https://nestedtext.org) — the one dialect this parser
    /// accepts (the format has no versioned spec the way TOML does; recent
    /// releases (3.x) haven't changed the on-disk grammar this reads).
    NESTEDTEXT,
};

pub const Language = struct {
    pub const Type = nestedtext.Type;
    pub const Parser = nestedtext.Parser;
    pub const Printer = nestedtext.Printer;
    pub const default_type: nestedtext.Type = .NESTEDTEXT;
    /// The parser's tree, which the contract writes into fig's table.
    pub fn parseTree(parser: *nestedtext.Parser, input: []const u8, format: nestedtext.Type) !Document {
        return nestedtext.Parser.parse(parser.allocator, input, format);
    }
    /// The document fig would adopt from a parse.
    pub fn parse(parser: *nestedtext.Parser, input: []const u8, format: nestedtext.Type) !Document.RowDocument {
        var tree = try parseTree(parser, input, format);
        return tree.intoDocument(parser.allocator);
    }
    pub const print = nestedtext.Printer.print;
    pub const printNode = nestedtext.Printer.printNode;

    pub const name = "nestedtext";
    pub const extensions: []const []const u8 = &.{"nt"};
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    /// What `languages/harness.zig` round-trips and edits: a dict with a
    /// nested list.
    pub const samples: []const []const u8 = &.{
        "a: 1\nb:\n  - x\n  - y\n",
    };

    /// Nested (dict/list) but deliberately untyped — every leaf is a string.
    pub const dialects: []const lang.Dialect(@This()) = &.{.{
        .name = "nestedtext",
        .prints = .{ .holds = .{}, .typed_scalars = false, .null_at_root = true, .char_as = "number" },
        .splice = .raw,
        .empty_doc_seed = "",
    }};

    pub fn syntax(t: nestedtext.Type) lang.Syntax {
        _ = t;
        return .{
            // Joins INI: a `#` after a value on the SAME line is literal
            // rest-of-line value text, not a comment (see `parser.zig`,
            // "rest-of-line values are 100% literal"). A trailing comment can
            // only ever be its own `#` line immediately after the entry.
            .comments = .{ .line = .{ .open = "#" }, .trailing = null },
            // The inline dict form, `{a: 1, b: 2}`, is what the generic
            // flow-entry insert writes; a block entry goes through
            // `renderEntry`.
            .kv_sep = ": ",
            // No literal spelling for an empty nested dict — every value is
            // either rest-of-line text or a nested block — so `set` cannot
            // auto-vivify a missing ancestor.
            .empty_map_literal = null,
            // Four-space nesting; an item is `- value`.
            .indent_unit = "    ",
            .seq_item_marker = "- ",
        };
    }

    // ── Renderers ────────────────────────────────────────────────────────────
    //
    // No hooks. Four renderers say how NestedText spells an entry, an item,
    // what follows a key, and a renamed key; every operation is the generic
    // engine's. The engine dispatches on PRESENCE — `Editor.hasRenderer`,
    // which is `@hasDecl` here — and a renderer it finds is called with the
    // dialect and strings, and returns a string.
    // The logic lives in `edit.zig` (and `editor_test.zig` holds this format's
    // editor tests too), not here: this block is the DECLARATION of what
    // this format supplies.
    const edit = @import("edit.zig");

    /// A value is framed either rest-of-line or as a nested `>`-block, on
    /// a four-space nesting convention, and a key that cannot be spelled
    /// plain takes the `: key` multiline form — none of which `key`,
    /// `kv_sep`, value on one line can write.
    pub const renderEntry = edit.renderEntry;

    /// The item twin: `- value`, or a bare `-` over a nested `>`-block for
    /// an empty or multi-line value.
    pub const renderItem = edit.renderItem;

    /// A value is always a raw scalar (this format has no typed or quoted
    /// literal syntax to splice verbatim) RENDERED same-line or as a nested
    /// `>`-block per its shape, so a replacement is reframed from the key —
    /// `:` and the tail after a plain key, the tail alone after a multiline
    /// `: key`, which has no separator — and the document root is a `>`
    /// block at column 0. The parser records each plain key's `:` so the
    /// engine knows to reframe.
    pub const renderTail = edit.renderTail;

    /// A plain key's span excludes its trailing `:` while a multiline key has
    /// no separator colon at all and starts at its line's indent, so a rename
    /// spells the new key in whichever form it needs, adding or dropping the
    /// colon — and refuses plain-to-multiline, which would also have to move
    /// a same-line value.
    pub const renderKey = edit.renderKey;
};

// Test discovery: importing `nestedtext.zig` (from root.zig) pulls in every
// submodule's tests, so the module owns its own test surface.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_test.zig");
}
