const yaml = @This();
const std = @import("std");
const Document = @import("../../kit/document.zig");
const Table = @import("../../kit/table.zig").Table;
const AST = @import("../../kit/ast/ast.zig");
const lang = @import("../../kit/manifest.zig");

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

    /// What `languages/harness.zig` round-trips and edits: block and flow
    /// shapes, a sequence, a nested mapping.
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
            // The only editable format whose block mapping has a single-line
            // spelling (`k: v`) that reaches the splice paths.
            .single_line_block_mapping = true,
        };
    }

    // ── Editing ──────────────────────────────────────────────────────────────
    //
    // No hooks and no renderers: YAML is the format the generic engine was
    // written against. Its reference layer is core — the alias node kind,
    // `AST.resolveAlias` and `AST.mergedChild`, the anchor and tag span
    // tables on `Document` — so follow-mode replace and inherited-key
    // detection are the engine's, the latter switched on by `merge_key`
    // above. The value reframe that lets a block collection replace an
    // inline one is the engine's too: the parser records every entry's `:`
    // (`Document.sepSpan`) and the engine rewrites everything after
    // the key. `editor_test.zig` holds this format's editor tests.
};

// Test discovery: importing `yaml.zig` (from root.zig) pulls in every YAML
// submodule's tests, so the module owns its own test surface. The conformance
// suite is build-option-gated and stays in root.zig.
test {
    _ = @import("stream.zig");
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
    _ = @import("editor_test.zig");
}
