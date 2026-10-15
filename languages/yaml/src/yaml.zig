const yaml = @This();
const std = @import("std");
const Document = @import("kit").Document;
const Table = @import("kit").table.Table;
const AST = @import("kit").AST;
const lang = @import("kit").manifest;

pub const Parser = @import("parser.zig");
pub const Tokenizer = @import("tokenizer.zig");
pub const Printer = @import("printer.zig");
pub const Type = enum {
    v1_2_2,
    /// YAML 1.1 (2005). Differs from 1.2 almost entirely in *scalar type
    /// resolution* (the tag repository at yaml.org/type): `yes/no/on/off/y/n`
    /// booleans, leading-zero octal (`0777`) + binary (`0b…`) + sexagesimal
    /// (`190:20:30`) ints, `_` digit separators, mandatory-sign float exponents,
    /// and `!!timestamp` auto-resolution. Structure/syntax is unchanged.
    /// Resolution differences are pinned by fig-conformance's YAML 1.1
    /// suite and implemented by
    /// `scalarKind1_1` in `parser.zig`. Structure/syntax is shared with 1.2.
    v1_1,
};

pub const Language = struct {
    pub const Type = yaml.Type;
    pub const Parser = yaml.Parser;
    pub const Printer = yaml.Printer;
    pub const default_type: yaml.Type = .v1_2_2;
    /// Write `input`'s rows into `out`, the table fig hands a parse.
    pub fn parseTable(parser: *yaml.Parser, input: []const u8, format: yaml.Type, out: *Table) !void {
        _ = parser;
        return yaml.Parser.parseInto(out, input, format);
    }
    /// The document fig would adopt from a parse, with the `<<` merge key
    /// `syntax` names.
    pub fn parse(parser: *yaml.Parser, input: []const u8, format: yaml.Type) !Document {
        var doc = try yaml.Parser.parse(parser.allocator, input, format);
        doc.ast.merge_key = syntax(format).merge_key;
        return doc;
    }
    pub const print = yaml.Printer.print;
    pub const printNode = yaml.Printer.printNode;
    /// A `---`/`...` stream's documents, each to be parsed on its own.
    pub fn splitStream(allocator: std.mem.Allocator, input: []const u8, t: yaml.Type) ![]lang.StreamDocument {
        _ = t;
        return @import("stream.zig").locate(allocator, input);
    }
    pub const name = "yaml";
    pub const extensions: []const []const u8 = &.{ "yaml", "yml" };
    pub const caps: lang.Caps = .{
        .read = true,
        .edit = true,
        .serialize = true,
        // Anchors, aliases, `<<` merges and tags: the layer `Materialize`
        // collapses when a document leaves YAML for a format without one.
        .references = true,
        // The core schema has a `null` and none of the extended scalars
        // (a `!!timestamp` is a 1.1 tag, not a core kind), so those ride in
        // a `$fig` envelope.
        .lossless = .{ .null = true },
    };

    /// What fig's `src/contract/harness.zig` round-trips and edits: block and
    /// flow shapes, a sequence, a nested mapping.
    pub const samples: []const []const u8 = &.{
        "a: 1\nb:\n  - x\n  - y\nc:\n  d: true\n",
        "{a: 1, b: [2, 3]}\n",
    };

    pub const dialects: []const lang.Dialect(@This()) = &.{.{
        .name = "yaml",
        .prints = .{ .holds = Language.caps.lossless.?, .char_as = "number" },
        .splice = .literal,
        // A bare `key:` seed, not `{}`: see `Syntax.empty_map_literal`'s
        // note on why an empty YAML document is the empty string.
        .empty_doc_seed = "",
        .print_name = "printWith",
        .specs = &.{
            .{ .name = "1.2", .dialect = .v1_2_2 },
            .{ .name = "1.2.2", .dialect = .v1_2_2 },
            .{ .name = "1.1", .dialect = .v1_1 },
            .{ .name = "1.1.0", .dialect = .v1_1 },
        },
    }};

    /// 1.1 and 1.2.2 differ only in scalar type RESOLUTION, never in the
    /// syntax the editor splices, so both dialects answer identically. This
    /// is where an editing divergence would land if one ever appeared.
    pub fn syntax(t: yaml.Type) lang.Syntax {
        _ = t;
        return .{
            .comments = .hash,
            // `<<` merges another mapping's entries in; a key resolved only
            // through it is inherited (shadowed on write, refused on delete).
            .merge_key = "<<",
            .kv_sep = ": ",
            // The empty seed rather than `{}` — see `Syntax.empty_map_literal`
            // for why the two are not interchangeable here.
            .empty_map_literal = "",
            // A flow `[]`: a null cannot become a block list as it becomes
            // a block mapping, so the list a `set` starts is a flow one.
            .empty_seq_literal = "[]",
            // The only editable format whose block mapping has a single-line
            // spelling (`k: v`) that reaches the splice paths.
            .single_line_block_mapping = true,
        };
    }

    // ── Editing ──────────────────────────────────────────────────────────────
    //
    // No hooks, and one renderer: YAML is the format the generic engine was
    // written against. Its reference layer is core — the alias node kind,
    // `AST.resolveAlias` and `AST.mergedChild`, the anchor and tag span
    // tables on `Document` — so follow-mode replace and inherited-key
    // detection are the engine's, the latter switched on by `merge_key`
    // above. The value reframe that lets a block collection replace an
    // inline one is the engine's too: the parser records every entry's `:`
    // (`Document.sepSpan`) and the engine rewrites everything after
    // the key. `tests/editor.zig` holds this format's editor tests.

    /// A string replacing a quoted one, in its quotes: `"…"` always, since it
    /// escapes anything, and `'…'` when the new string has no control
    /// character, which the printer would double-quote. Nothing for a plain
    /// scalar, which is the printer's own choice, or a block scalar, which
    /// the printer writes for a string with line breaks.
    pub fn renderString(_: yaml.Type, allocator: std.mem.Allocator, out: *std.ArrayList(u8), r: lang.RenderRequest) !void {
        if (r.old_value.len == 0) return;
        var w = std.Io.Writer.Allocating.init(allocator);
        defer w.deinit();
        switch (r.old_value[0]) {
            '"' => try yaml.Printer.writeDoubleQuoted(&w.writer, r.value),
            '\'' => if (!yaml.Printer.hasControlChar(r.value)) try yaml.Printer.writeSingleQuoted(&w.writer, r.value),
            else => {},
        }
        try out.appendSlice(allocator, w.written());
    }
};

// The package's own tests (`zig build test`): every file's. Its editor
// tests need fig itself, which a language cannot depend on, so they are
// `tests/editor.zig`, which fig's build compiles.
test {
    _ = @import("stream.zig");
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
}
