const dotenv = @This();
const Document = @import("../../document.zig");
const lang = @import("../manifest.zig");

pub const Parser = @import("parser.zig");
pub const Tokenizer = @import("tokenizer.zig");
pub const Printer = @import("printer.zig");

pub const Type = enum {
    /// The one dialect this parser accepts — see `tokenizer.zig`'s module doc
    /// for exactly what it does and doesn't accept (bash-identifier keys,
    /// optional `export`, real `"`/`'` quoting, no `$VAR` interpolation).
    DOTENV,
};

pub const Language = struct {
    pub const Type = dotenv.Type;
    pub const Parser = dotenv.Parser;
    pub const Printer = dotenv.Printer;
    pub const default_type: dotenv.Type = .DOTENV;
    pub fn parse(parser: *dotenv.Parser, input: []const u8, format: dotenv.Type) !Document {
        return dotenv.Parser.parse(parser.allocator, input, format);
    }
    pub const print = dotenv.Printer.print;
    pub const printNode = dotenv.Printer.printNode;

    pub const name = "dotenv";
    /// A dotenv file is conventionally named exactly `.env`, whose last-dot
    /// "extension" is the literal `env`. (`.env.production` is not recognized
    /// by extension — pass `--input dotenv`.)
    pub const extensions: []const []const u8 = &.{"env"};
    /// Flat: even a depth-1 mapping (INI's `[section]`) has no dotenv
    /// spelling, since dotenv has no nesting concept at all.
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true, .max_mapping_depth = 0 };

    /// A flat string map and nothing more: no nesting, untyped scalars. A
    /// nested value tree cannot be represented, and serializing one warns.
    pub const dialects: []const lang.Dialect(@This()) = &.{.{
        .name = "dotenv",
        .abi_value = 10,
        // Last of the four key/value-shaped formats. dotenv is almost entirely
        // shadowed by INI: INI's key scanner accepts any non-`=`/newline run
        // (so even `export FOO=bar` parses as one weird INI key) and its value
        // decoding is quote-agnostic, so nearly anything dotenv accepts, INI
        // already claimed first. The one thing only dotenv parses — a
        // `"`/`'`-quoted value spanning a literal embedded newline (INI's
        // value never crosses a physical line) — is this rank's actual reason
        // to exist; `.env`'s real path to selection is its extension
        // (`cli/args.zig`'s `detectLanguageFromFileEnding` special-cases
        // `env`), not this content sniff.
        .sniff_rank = 8,
        .splice = .raw,
        .empty_doc_seed = "",
    }};

    pub fn syntax(t: dotenv.Type) lang.Syntax {
        _ = t;
        return .{
            .comments = .hash,
            // A bare `=` with no surrounding spaces — see `printer.zig`.
            .kv_sep = "=",
            // Flat: no nesting to vivify into, but `{}` is still the literal
            // the generic seed would splice, and the format has no nested
            // path for it to seed. Kept as the shared default rather than
            // null, which would change the error a nested `set` reports.
            .empty_map_literal = "{}",
        };
    }
};

// Test discovery: importing `dotenv.zig` (from root.zig) pulls in every
// dotenv submodule's tests, so the module owns its own test surface.
test {
    _ = @import("tokenizer.zig");
    _ = @import("parser.zig");
    _ = @import("printer.zig");
}
