const nestedtext = @This();
const Document = @import("../../document.zig");
const lang = @import("../manifest.zig");

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
    pub fn parse(parser: *nestedtext.Parser, input: []const u8, format: nestedtext.Type) !Document {
        return nestedtext.Parser.parse(parser.allocator, input, format);
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
        .abi_value = 13,
        // LAST, after even `.properties` — not because its own grammar is
        // unusually permissive (it isn't: keys/values have real restrictions,
        // unlike `.properties`'s "nearly any text"), but because a huge,
        // ordinary swath of it — plain `key: value` lines and `- item` lists —
        // is ALSO valid YAML, and parses to a MEANINGFULLY DIFFERENT tree there
        // (YAML types `port: 80` as an integer; NestedText's `port` is the
        // untyped string `"80"`). Trying this before YAML would silently change
        // what `detect()` returns for ordinary plain-YAML content already relied
        // upon elsewhere in this codebase — a real regression, not just an
        // academic ambiguity — so NestedText only gets a turn once every
        // stricter-or-equally-plausible format (including YAML) has rejected
        // the input. Its real path to selection is the `.nt` extension (see
        // `cli/args.zig`), exactly like dotenv/`.properties`.
        .sniff_rank = 11,
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
            .comments = .{ .style = .hash, .line = .{ .open = "#" }, .trailing = null },
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

    // ── Editing hooks and renderers ──────────────────────────────────────────
    //
    // Two renderers say how NestedText spells an entry and an item, and two
    // hooks remain for the reframes the engine cannot yet express. The
    // engine dispatches on PRESENCE — `@hasDecl(Language, "renderEntry")` —
    // and every operation not named below runs the generic implementation.
    // The logic lives in `editor_helper.zig` (which holds this format's
    // editor tests too), not here: this block is the DECLARATION of what
    // this format supplies.
    const edit = @import("editor_helper.zig");

    /// A value is framed either rest-of-line or as a nested `>`-block, on
    /// a four-space nesting convention, and a key that cannot be spelled
    /// plain takes the `: key` multiline form — none of which `key`,
    /// `kv_sep`, value on one line can write.
    pub const renderEntry = edit.renderEntry;

    /// The item twin: `- value`, or a bare `-` over a nested `>`-block for
    /// an empty or multi-line value.
    pub const renderItem = edit.renderItem;

    /// `replacement` is always a raw scalar (this format has no typed or quoted
    /// literal syntax to splice verbatim) and has to be RENDERED same-line or
    /// as a nested `>`-block per its shape — and since that shape may differ
    /// from the old value's, the reframe runs from the key or dash through the
    /// old value's end rather than over the value span alone.
    pub const replaceValAtPath = edit.ntReplaceValue;

    /// A plain key's span excludes its trailing `:` while a multiline key has
    /// no separator colon at all, so converting between the two forms means
    /// adding or dropping one.
    pub const replaceKeyAtPath = edit.ntReplaceKey;
};

// Test discovery: importing `nestedtext.zig` (from root.zig) pulls in every
// submodule's tests, so the module owns its own test surface.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_helper.zig");
}
