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

    pub fn syntax(t: nestedtext.Type) lang.Syntax {
        _ = t;
        return .{
            .comment_style = .hash,
            .line_comment = "#",
            // Joins INI: a `#` after a value on the SAME line is literal
            // rest-of-line value text, not a comment (see `parser.zig`,
            // "rest-of-line values are 100% literal"). A trailing comment can
            // only ever be its own `#` line immediately after the entry.
            .trailing_comment = null,
            .kv_sep = ": ",
            // No literal spelling for an empty nested dict — every value is
            // either rest-of-line text or a nested block — so `set` cannot
            // auto-vivify a missing ancestor.
            .empty_map_literal = null,
        };
    }

    // ── Editing hooks ────────────────────────────────────────────────────────
    //
    // Operations this format takes over from the generic splice engine.
    // `Editor` dispatches on PRESENCE — `@hasDecl(Language, "insertKey")` — so
    // declaring one here is the whole of opting in, and every operation not
    // named below runs the generic implementation. Each signature is fixed by
    // the `editor.Editor` method of the same name; see its doc comment.
    //
    // The logic lives in `editor_helper.zig` (which holds this format's editor
    // tests too), not here: this block is the DECLARATION of which operations
    // are overridden, so a reader can see a format's whole answer in one struct
    // without opening the helper.
    const edit = @import("editor_helper.zig");

    /// Values are framed either rest-of-line or as a nested `>`-block, on a
    /// 4-space nesting convention — neither of which the YAML/fig-shaped
    /// generic block insert (2-space, bare `:` continuation) can write.
    pub const insertKey = edit.ntInsertKey;

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

    /// A nested or empty item's value span can begin on a later line than its
    /// own `-` dash, so a leading comment keyed by `.index` anchors on the
    /// dash's line rather than on the span's.
    pub const seqItemLineStart = edit.seqItemLineStart;

    /// `dashColumn` reads the item column off the first item's OWN line, which
    /// a nested or empty-valued item doesn't have — so the dash's line is
    /// derived from the sequence node's span instead. Also renders a multiline
    /// or empty value as a nested `>`-block rather than `insertSeqLine`'s bare
    /// reindent.
    pub const appendToSeq = edit.ntAppendItem;
    pub const prependToSeq = edit.ntPrependItem;
    pub const removeSeqItem = edit.ntRemoveSeqItem;

    /// Item block boundaries can't be recovered from each item span's start
    /// here, so this computes its own; the tiling, permutation and splice
    /// underneath are the generic engine's, reused as-is.
    pub const reorderSeqItems = edit.ntReorderSeqItems;
};

// Test discovery: importing `nestedtext.zig` (from root.zig) pulls in every
// submodule's tests, so the module owns its own test surface.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_helper.zig");
}
