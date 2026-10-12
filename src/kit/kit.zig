//! The kit: what fig's languages share — the tree a parser builds and a
//! printer reads (`AST`, `Document`), the node table that is the contract and
//! the conversions between the two, the spelling of numbers and datetimes,
//! the JSON-style string escaper several formats borrow, the bare-literal
//! rule, a token and its span, the XML tokenizer, and the shape of a parse
//! diagnostic. It imports nothing of core's, so a
//! language built apart from core can carry a copy of it, which costs bytes
//! and nothing else (docs/proposals/core-only.md §4.8). Core uses it too.

const std = @import("std");

pub const Span = @import("span.zig");
pub const Token = @import("token.zig").Token;
pub const literal = @import("literal.zig");
pub const lines = @import("lines.zig");
pub const parse_diagnostic = @import("parse_diagnostic.zig");
pub const xml = @import("xml.zig");
/// The language-side tree a parser builds and a printer reads, and the
/// node table (the contract) each is turned into and out of.
pub const AST = @import("ast/ast.zig");
pub const Document = @import("document.zig");
pub const table = @import("table.zig");
pub const convert = @import("convert.zig");
/// The contract's declarations and C shapes, which a language fills.
pub const manifest = @import("manifest.zig");
pub const abi = @import("abi.zig");
pub const Unicode = @import("unicode.zig");
pub const json_string = @import("json_string.zig");
pub const ascii = @import("ascii.zig");
pub const datetime = @import("datetime.zig");
pub const width = @import("width.zig");
pub const number = @import("number.zig");

test {
    _ = AST;
    _ = Document;
    _ = table;
    _ = convert;
    _ = manifest;
    _ = abi;
    _ = literal;
    _ = lines;
    _ = parse_diagnostic;
    _ = xml;
    _ = @import("token.zig");
    _ = json_string;
    _ = ascii;
    _ = datetime;
    _ = Unicode;
    _ = width;
    _ = number;
}

/// Like std.mem.eql, but also works if type has a `.eql` function defined.
pub fn eql(comptime T: type, a: []const T, b: []const T) bool {
    if (a.len != b.len) return false;
    switch (@typeInfo(T)) {
        .@"struct", .@"union", .@"opaque", .vector, .array => {
            if (!@hasDecl(T, "eql")) @compileError("No eql defined for this type");
            for (a, b) |aa, bb| if (!aa.eql(bb)) return false;
        },
        else => for (a, b) |aa, bb| if (aa != bb) return false,
    }
    return true;
}

/// True when byte slice `s` exactly equals one of `options`. The shared form of
/// the keyword-set matching the format parsers do (null/bool spellings, the
/// `inf`/`nan` floats, etc.).
pub fn eqlAny(s: []const u8, options: []const []const u8) bool {
    for (options) |o| if (std.mem.eql(u8, s, o)) return true;
    return false;
}

test "util.eql" {
    try std.testing.expect(eql(u8, "hello, world!", "hello, world!"));
}

test "util.eqlAny" {
    try std.testing.expect(eqlAny("yes", &.{ "y", "yes", "true" }));
    try std.testing.expect(!eqlAny("maybe", &.{ "y", "yes", "true" }));
}
