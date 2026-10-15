//! dotenv, written with the kit: the twin of fig's compiled `dotenv`, row
//! for row, as `@diaryx/fig/js/dotenv` is in JavaScript and Rust's
//! `examples/kit_dotenv_helper` is with `fig::kit`. Its parse is a
//! `kit.grammar` description; its print writes through `kit.Writer`. fig's
//! core tests register it through the contract (`kit/contract.zig`) and
//! hold it to the compiled dotenv table for table.
//!
//!   * a key is a bash identifier, `[A-Za-z_][A-Za-z0-9_]*`; an `export `
//!     before one is recognized and discarded;
//!   * a value is unquoted (trimmed, to the end of the line or to a `#`
//!     that a space or tab precedes), double-quoted (`\n \t \r \\ \"`
//!     decoded, may span lines) or single-quoted (raw, may span lines);
//!   * a full-line `#` is a comment, leading for the next key or dangling
//!     on the root; a `#` after a value on its line is its trailing one;
//!   * a repeated key keeps the first entry's place and takes the last
//!     value.

const twin = @This();
const std = @import("std");
const kit = @import("kit");
const g = kit.grammar;
const AST = kit.AST;
const lang = kit.manifest;

const BARE_CR = "a bare `\\r` must be followed by `\\n`; line endings must be `\\n` or `\\r\\n`";
const UNCLOSED = "unclosed quoted value; expected a matching `\"`/`'` before the end of the file";

fn isKeyStart(c: u8) bool {
    return c == '_' or std.ascii.isAlphabetic(c);
}

fn isKey(c: u8) bool {
    return c == '_' or std.ascii.isAlphanumeric(c);
}

fn never(_: u8) bool {
    return false;
}

/// Only spaces, a `#` comment or the line's end may follow a closing quote.
fn afterQuoted(cx: *g.Ctx, _: kit.Span) g.Error!?usize {
    const save = cx.sc.pos;
    _ = cx.sc.hs();
    if (!(cx.sc.eof() or cx.sc.starts("#") or cx.sc.starts("\n") or cx.sc.starts("\r")))
        return cx.failHere("unexpected content after this quoted value; only a `#` comment may follow it on the same line");
    cx.sc.pos = save;
    return null;
}

/// A `#` that a space or tab precedes ends a bare value — the one right
/// after `=` counts, since `A= #c` is empty.
fn hashAfterSpace(sc: *const kit.Scanner) bool {
    const before = sc.byte(-1) orelse return false;
    return sc.starts("#") and (before == ' ' or before == '\t');
}

const value = g.choice(.{
    g.quoted(.{
        .open = "\"",
        .escapes = &.{
            .{ .byte = 'n', .means = "\n" }, .{ .byte = 't', .means = "\t" },  .{ .byte = 'r', .means = "\r" },
            .{ .byte = '\\', .means = "\\" }, .{ .byte = '"', .means = "\"" },
        },
        .unclosed = UNCLOSED,
        .bad_escape = "invalid escape in a double-quoted value; supported: \\n \\t \\r \\\\ \\\" — use a single-quoted value for raw text with backslashes",
        .bare_cr = BARE_CR,
        .after = afterQuoted,
    }),
    g.quoted(.{ .open = "'", .unclosed = UNCLOSED, .bare_cr = BARE_CR, .after = afterQuoted }),
    g.bare(.{ .stop = g.when(hashAfterSpace) }),
});

const entry = g.entry(.{
    // `export` is a prefix only when a key follows it on the line; alone
    // before `=` it is the key.
    .prefix = g.seq(.{ g.lit("export"), g.hs1, g.ahead(g.ident(isKeyStart, never)) }),
    .key = g.key(g.ident(isKeyStart, isKey)),
    .sep = g.lit("="),
    .value = value,
    .missing_sep = "expected `=` after this key; every dotenv line is `KEY=value`",
});

/// Where a key should be.
fn otherwise(cx: *g.Ctx) g.Error!void {
    if (cx.sc.starts("=")) return cx.failHere("unexpected content here; expected `KEY=value` (optionally `export KEY=value`)");
    return cx.failHere("not a valid key here; a dotenv key is a bash identifier (`[A-Za-z_][A-Za-z0-9_]*`)");
}

const Grammar = g.Document(.{
    .bom = true,
    .root = g.map(.{
        .whole = true,
        .entry = entry,
        .trivia = g.trivia(.{
            .space = g.choice(.{ g.hs1, g.eol, g.failIf(g.lit("\r"), BARE_CR) }),
            .comment = g.comment(.{ .open = "#" }),
        }),
        .otherwise = otherwise,
    }),
});

pub const Type = enum { DOTENV };

pub const Parser = g.Parser(Grammar, Type);

pub const Printer = struct {
    pub const Error = std.Io.Writer.Error || error{ NullUnsupported, NonStringKey, InvalidKey, UnsupportedValue, UnresolvedAlias };

    fn isIdentifier(name: []const u8) bool {
        if (name.len == 0 or !isKeyStart(name[0])) return false;
        for (name[1..]) |c| if (!isKey(c)) return false;
        return true;
    }

    fn needsQuoting(v: []const u8) bool {
        if (v.len == 0) return false;
        if (kit.Scanner.isHs(v[0]) or kit.Scanner.isHs(v[v.len - 1])) return true;
        return std.mem.indexOfAny(u8, v, "\n\r\"\\#") != null;
    }

    fn writeValue(w: kit.Writer, ast: *const AST, id: AST.Node.Id) Error!void {
        switch (ast.node(id).kind) {
            .string => |v| if (needsQuoting(v)) {
                try w.put("\"");
                for (v) |c| switch (c) {
                    '\n' => try w.put("\\n"),
                    '\r' => try w.put("\\r"),
                    '\t' => try w.put("\\t"),
                    '"' => try w.put("\\\""),
                    '\\' => try w.put("\\\\"),
                    else => try w.out.writeByte(c),
                };
                try w.put("\"");
            } else try w.put(v),
            .number => |n| try w.put(n.raw),
            .boolean => |b| try w.put(if (b) "true" else "false"),
            .extended => |e| try w.put(e.text),
            .null_ => return error.NullUnsupported,
            .alias => return error.UnresolvedAlias,
            .sequence, .mapping, .keyvalue => return error.UnsupportedValue,
        }
    }

    pub fn print(out: *std.Io.Writer, ast: *const AST, options: AST.SerializeOptions) Error!void {
        const w = kit.Writer.init(out, options.pretty, options.indent);
        const first = switch (ast.node(ast.root).kind) {
            .mapping => |f| f,
            else => {
                // A fragment: the scalar as it stands alone.
                try writeValue(w, ast, ast.root);
                return out.flush();
            },
        };
        var cur = first;
        while (cur) |id| : (cur = ast.node(id).next_sibling) {
            const kv = ast.node(id).kind.keyvalue;
            const name = switch (ast.node(kv.key).kind) {
                .string => |s| s,
                else => return error.NonStringKey,
            };
            if (!isIdentifier(name)) return error.InvalidKey;
            _ = try w.comments(ast.comments(kv.key).leading, "#", 0);
            try w.put(name);
            try w.put("=");
            try writeValue(w, ast, kv.value);
            if (ast.comments(kv.value).trailing) |c| {
                try w.put(" #");
                if (c.text.len > 0) {
                    try w.put(" ");
                    for (c.text) |ch| try w.out.writeByte(if (ch == '\n') ' ' else ch);
                }
            }
            try w.put("\n");
        }
        _ = try w.comments(ast.comments(ast.root).dangling, "#", 0);
        try out.flush();
    }
};

pub const Language = struct {
    pub const Type = twin.Type;
    pub const Parser = twin.Parser;
    pub const Printer = twin.Printer;
    pub const default_type: twin.Type = .DOTENV;

    pub fn parse(parser: *twin.Parser, input: []const u8, t: twin.Type) !kit.Document {
        return twin.Parser.parse(parser.allocator, input, t);
    }

    pub const name = "dotenv";
    pub const extensions: []const []const u8 = &.{"env"};
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true, .max_mapping_depth = 0 };
    pub const samples: []const []const u8 = &.{
        "A=1\nB=\"two words\"\n",
        "# top\nexport C='raw \\n'\nD=x # trailing\n",
    };
    pub const dialects: []const lang.Dialect(@This()) = &.{.{
        .name = "dotenv",
        .prints = .{ .holds = .{}, .typed_scalars = false, .char_as = "number" },
        .splice = .raw,
        .empty_doc_seed = "",
    }};

    pub fn syntax(_: twin.Type) lang.Syntax {
        return .{ .comments = .hash, .kv_sep = "=", .empty_map_literal = "{}", .flow_containers = false };
    }
};
