//! The parser turns a `.properties`-formatted []const u8 into the node table
//! fig hands it, a row per node (`RowWriter`, through `kit/flat_map.zig`).
//!
//! Shape: a single flat root mapping, `key<sep>value` per entry — like
//! dotenv, no sections/nesting; unlike dotenv, keys are NOT restricted to
//! identifiers (any escaped text up to the separator is legal, including the
//! empty string). Every value is a `.string` node: no typed scalars, matching
//! `java.util.Properties`, which is itself just a `Hashtable<String,String>`.
//!
//! Like INI (and unlike dotenv), there is no trailing-comment syntax: a
//! `#`/`!` only starts a comment as the first byte of its OWN physical line
//! (see `tokenizer.zig`) — one appearing later is just part of the key/value
//! text.
//!
//! A repeated key silently keeps the LAST value at the FIRST-seen position
//! (see `kit/flat_map.zig`'s `putEntry`, `.overwrite`) and raises a
//! `duplicate_key` warning, exactly like INI/dotenv.

const Parser = @This();

const std = @import("std");
const testing = std.testing;
const Document = @import("../../kit/document.zig");
const AST = @import("../../kit/ast/ast.zig");
const RowWriter = @import("../../kit/row_writer.zig");
const table = @import("../../kit/table.zig");
const Table = table.Table;

const Type = @import("properties.zig").Type;
const Span = @import("../../kit/span.zig");
const Unicode = @import("../../kit/kit.zig").Unicode;
const flat_map = @import("../../kit/flat_map.zig");
const Tokenizer = @import("tokenizer.zig");
const Token = Tokenizer.Token;

allocator: std.mem.Allocator,
version: Type = .PROPERTIES,
source: []const u8 = "",
tokens: []const Token = &.{},
pos: usize = 0,
arena: flat_map.NodeArena = undefined,
root_id: AST.Node.Id = 0,
/// Decoded keys and values, freed when the parse ends: the table keeps its
/// own copy.
owned_strings: std.ArrayList([]const u8) = .empty,
pending_leading: std.ArrayList(AST.Comment) = .empty,

recover: bool = false,
diagnostics: std.ArrayList(Diagnostic) = .empty,
warnings: std.ArrayList(Warning) = .empty,
fail_offset: ?usize = null,
fail_end: ?usize = null,

pub const ParseError = error{
    InvalidUnicode,
    InvalidUtf8,
    /// Never actually raised — `parseKeyValue` always calls `flat_map.putEntry`
    /// with `.overwrite`, which never returns this — but its STATIC return
    /// type includes it (the same function can be called with `.err`
    /// elsewhere), so it has to be in this set for `try` to type-check.
    DuplicateKey,
};
pub const ParserError = ParseError || Tokenizer.TokenizeError || std.mem.Allocator.Error;
pub const Error = ParserError;

pub fn describe(code: Error) []const u8 {
    return switch (code) {
        error.InvalidUnicode => "invalid \\uXXXX escape; expected exactly 4 hex digits forming a valid Unicode codepoint",
        error.InvalidUtf8 => "this file is not valid UTF-8; fig requires .properties documents to be UTF-8 encoded",
        error.UnexpectedCarriageReturn => "a bare `\\r` must be followed by `\\n`; line endings must be `\\n` or `\\r\\n`",
        error.UnclosedEscape => "a `\\` at the very end of the file has nothing to escape",
        error.DuplicateKey => unreachable, // see the ParseError doc comment
        error.OutOfMemory => "out of memory",
    };
}

pub fn shortLabel(code: Error) []const u8 {
    return switch (code) {
        error.InvalidUnicode => "invalid unicode escape",
        error.InvalidUtf8 => "invalid UTF-8",
        error.UnexpectedCarriageReturn => "bare CR",
        error.UnclosedEscape => "unclosed escape",
        error.DuplicateKey => unreachable, // see the ParseError doc comment
        error.OutOfMemory => "out of memory",
    };
}

const parse_diagnostic = @import("../../kit/parse_diagnostic.zig");

pub const Diagnostic = struct {
    code: Error,
    offset: usize,
    end: ?usize = null,

    pub fn locate(self: Diagnostic, source: []const u8) parse_diagnostic.Location {
        return parse_diagnostic.locateOffset(source, self.offset);
    }
    pub fn renderAlloc(self: Diagnostic, allocator: std.mem.Allocator, source: []const u8, file: []const u8) std.mem.Allocator.Error![]u8 {
        return parse_diagnostic.renderReportAlloc(allocator, source, self.offset, file, "error", describe(self.code));
    }
};

pub const Warning = struct {
    code: Code,
    offset: usize,
    end: ?usize = null,

    pub const Code = enum { duplicate_key };

    pub fn describeWarning(code: Code) []const u8 {
        return switch (code) {
            .duplicate_key => "this key is defined more than once; the last value wins and earlier ones are silently discarded",
        };
    }
    pub fn shortLabel(code: Code) []const u8 {
        return switch (code) {
            .duplicate_key => "duplicate key",
        };
    }
    pub fn locate(self: Warning, source: []const u8) parse_diagnostic.Location {
        return parse_diagnostic.locateOffset(source, self.offset);
    }
    pub fn renderAlloc(self: Warning, allocator: std.mem.Allocator, source: []const u8, file: []const u8) std.mem.Allocator.Error![]u8 {
        return parse_diagnostic.renderReportAlloc(allocator, source, self.offset, file, "warning", describeWarning(self.code));
    }
};

pub const Report = struct {
    diag: ?Diagnostic = null,
    errors: []const Diagnostic = &.{},
    warnings: []const Warning = &.{},
};

/// `input` as a `Document`: its table written and adopted.
pub fn parse(allocator: std.mem.Allocator, input: []const u8, format: Type) ParserError!Document {
    var t = Table.init(allocator, input);
    defer t.deinit();
    try parseInto(&t, input, format);
    return RowWriter.adopt(allocator, input, &t);
}

/// `parse`, reporting as `parseWithReportInto` does.
pub fn parseWithReport(allocator: std.mem.Allocator, input: []const u8, format: Type, out: *Report) ParserError!Document {
    var t = Table.init(allocator, input);
    defer t.deinit();
    try parseWithReportInto(&t, input, format, out);
    return RowWriter.adopt(allocator, input, &t);
}

/// `parse`, collecting as `parseCollectingInto` does.
pub fn parseCollecting(allocator: std.mem.Allocator, input: []const u8, format: Type, out: *Report) ParserError!Document {
    var t = Table.init(allocator, input);
    defer t.deinit();
    try parseCollectingInto(&t, input, format, out);
    return RowWriter.adopt(allocator, input, &t);
}

/// Write `input`'s rows into `t`, the table fig hands a parse.
pub fn parseInto(t: *Table, input: []const u8, format: Type) ParserError!void {
    return parseImpl(t, input, format, null, false);
}

/// `parseInto`, but also fills `out`: `diag` on failure, `warnings` always.
pub fn parseWithReportInto(t: *Table, input: []const u8, format: Type, out: *Report) ParserError!void {
    return parseImpl(t, input, format, out, false);
}

/// `parseWithReportInto`, but recovers past each parser-level error to
/// collect every diagnostic (`out.errors`). On an error the table is not
/// finished.
pub fn parseCollectingInto(t: *Table, input: []const u8, format: Type, out: *Report) ParserError!void {
    return parseImpl(t, input, format, out, true);
}

fn parseImpl(t: *Table, input: []const u8, format: Type, out: ?*Report, recover: bool) ParserError!void {
    const allocator = t.allocator;
    var parser: Parser = .{ .allocator = allocator, .arena = .init(t), .recover = recover };
    defer parser.arena.deinit();
    defer parser.diagnostics.deinit(allocator);
    defer parser.warnings.deinit(allocator);
    defer {
        for (parser.owned_strings.items) |s| allocator.free(s);
        parser.owned_strings.deinit(allocator);
    }
    const result = parser.parseOnce(input, format);
    if (out) |o| o.warnings = allocator.dupe(Warning, parser.warnings.items) catch &.{};
    return result catch |err| {
        if (out) |o| {
            if (parser.diagnostics.items.len > 0) {
                o.diag = parser.diagnostics.items[0];
                if (recover) o.errors = allocator.dupe(Diagnostic, parser.diagnostics.items) catch &.{};
            }
        }
        return err;
    };
}

/// `input` as an `AST`, alone.
pub fn parseAbstract(allocator: std.mem.Allocator, input: []const u8, format: Type) ParserError!AST {
    const doc = try parse(allocator, input, format);
    return doc.ast;
}

fn dispatchStatement(self: *Parser) ParserError!void {
    // The tokenizer never emits anything else at a statement boundary (a
    // comment/newline is consumed by `skipBlank` before this runs).
    try self.parseKeyValue();
}

/// The next `.newline`/`.end_of_file` token from `start`. No lexical state
/// persists across a real newline (a continuation is fully resolved WITHIN
/// one key/value token by the tokenizer — see its module doc), so, like
/// INI/dotenv and unlike TOML, the pre-computed token stream never needs
/// re-tokenizing after a parser-level error. (Nothing in THIS parser
/// currently raises one — `dispatchStatement` cannot fail except by
/// `OutOfMemory` — but `parseCollecting`'s machinery is kept parallel to the
/// other two languages' for a future error that needs it.)
fn resync(self: *Parser, start: usize) usize {
    var j = start;
    while (j < self.tokens.len) : (j += 1) {
        switch (self.tokens[j].kind) {
            .newline, .end_of_file => return j,
            else => {},
        }
    }
    return self.tokens.len - 1;
}

fn parseOnce(self: *Parser, input: []const u8, format: Type) ParserError!void {
    self.version = format;
    self.source = input;

    if (!std.unicode.utf8ValidateSlice(input)) return error.InvalidUtf8;

    var tokenizer: Tokenizer = .{ .allocator = self.allocator, .str = input, .version = format };
    self.tokens = tokenizer.tokenize() catch |err| {
        try self.diagnostics.append(self.allocator, .{ .code = err, .offset = tokenizer.i });
        return err;
    };
    defer self.allocator.free(self.tokens);
    try self.pending_leading.ensureTotalCapacity(self.allocator, self.tokens.len);
    defer self.pending_leading.deinit(self.allocator);

    self.root_id = try self.arena.addNode(.{ .mapping = null }, Span.init(0, input.len));

    self.skipBlank();
    while (!self.atEnd()) {
        self.dispatchStatement() catch |err| {
            if (err == error.OutOfMemory) return err;
            const offset = self.fail_offset orelse self.peek().span.start;
            const end = self.fail_end;
            self.fail_offset = null;
            self.fail_end = null;
            try self.diagnostics.append(self.allocator, .{ .code = err, .offset = offset, .end = end });
            if (!self.recover) return err;
            self.pos = self.resync(self.pos);
        };
        self.skipBlank();
    }
    try self.claimDangling(self.root_id);

    if (self.diagnostics.items.len > 0) return self.diagnostics.items[0].code;

    try self.arena.w.finish(self.root_id);
}

// ── Token cursor ────────────────────────────────────────────────────────────

fn peek(self: *Parser) Token {
    return self.tokens[self.pos];
}
fn advance(self: *Parser) Token {
    const t = self.tokens[self.pos];
    if (self.pos + 1 < self.tokens.len) self.pos += 1;
    return t;
}
fn atEnd(self: *Parser) bool {
    return self.peek().kind == .end_of_file;
}
fn tokenText(self: *Parser, tok: Token) []const u8 {
    return self.source[tok.span.start..tok.span.end];
}

fn skipBlank(self: *Parser) void {
    while (true) switch (self.peek().kind) {
        .comment => {
            self.captureComment(self.peek());
            self.pos += 1;
        },
        .newline => self.pos += 1,
        else => return,
    };
}

// ── Comments ─────────────────────────────────────────────────────────────────
// Full-line only (see the module doc) — every captured comment simply
// buffers as `pending_leading` until the next key claims it, or it dangles at
// EOF, exactly like INI's (simpler) comment model.

fn captureComment(self: *Parser, tok: Token) void {
    const text = std.mem.trim(u8, self.tokenText(tok), " \t\r");
    self.pending_leading.appendAssumeCapacity(.{ .text = text, .style = .line });
}

fn claimLeading(self: *Parser, id: AST.Node.Id) ParserError!void {
    for (self.pending_leading.items) |c| try self.arena.w.comment(id, table.comment_leading, c);
    self.pending_leading.clearRetainingCapacity();
}

fn claimDangling(self: *Parser, id: AST.Node.Id) ParserError!void {
    for (self.pending_leading.items) |c| try self.arena.w.comment(id, table.comment_dangling, c);
    self.pending_leading.clearRetainingCapacity();
}

fn addWarning(self: *Parser, code: Warning.Code, span: Span) ParserError!void {
    try self.warnings.append(self.allocator, .{ .code = code, .offset = span.start, .end = span.end });
}

// ── Statements ──────────────────────────────────────────────────────────────

fn parseKeyValue(self: *Parser) ParserError!void {
    const key_tok = self.peek();
    _ = self.advance();
    const key_str = try self.decodeEscaped(self.tokenText(key_tok));

    // The tokenizer always emits a value, zero-width when the line has
    // none — where one would begin, after the separator.
    const value_tok = self.peek();
    _ = self.advance();
    const value_str = try self.decodeEscaped(self.tokenText(value_tok));
    const value_span = value_tok.span;

    const key_id = try self.arena.addNode(.{ .string = key_str }, key_tok.span);
    try self.claimLeading(key_id);
    const value_id = try self.arena.addNode(.{ .string = value_str }, value_span);
    const put = try flat_map.putEntry(&self.arena, self.root_id, key_id, value_id, .overwrite);
    if (put.result == .overwrote) try self.addWarning(.duplicate_key, key_tok.span);
}

/// Decode a raw key/value span: `\t \n \r \f \\`, `\uXXXX`, a `\<newline>`
/// continuation (drops the backslash, the newline, AND the resumed line's
/// leading whitespace — see the tokenizer), and — matching real
/// `java.util.Properties.load`, which never rejects an unrecognized escape —
/// `\` followed by anything else decodes to that byte literally.
fn decodeEscaped(self: *Parser, raw: []const u8) ParserError![]const u8 {
    if (std.mem.indexOfScalar(u8, raw, '\\') == null) return raw; // nothing to decode; borrow
    var out: std.ArrayList(u8) = .empty;
    errdefer out.deinit(self.allocator);
    var i: usize = 0;
    while (i < raw.len) {
        const c = raw[i];
        if (c != '\\') {
            try out.append(self.allocator, c);
            i += 1;
            continue;
        }
        if (i + 1 < raw.len and raw[i + 1] == '\n') {
            i += 2;
            while (i < raw.len and (raw[i] == ' ' or raw[i] == '\t')) i += 1;
            continue;
        }
        if (i + 2 < raw.len and raw[i + 1] == '\r' and raw[i + 2] == '\n') {
            i += 3;
            while (i < raw.len and (raw[i] == ' ' or raw[i] == '\t')) i += 1;
            continue;
        }
        if (i + 1 >= raw.len) return error.UnclosedEscape; // tokenizer guarantees this can't happen
        switch (raw[i + 1]) {
            't' => try out.append(self.allocator, '\t'),
            'n' => try out.append(self.allocator, '\n'),
            'r' => try out.append(self.allocator, '\r'),
            'f' => try out.append(self.allocator, 0x0c),
            'u' => {
                i = try self.appendUnicode(&out, raw, i + 2) - 2;
            },
            else => |ch| try out.append(self.allocator, ch), // literal passthrough (incl. `\\`)
        }
        i += 2;
    }
    const slice = try out.toOwnedSlice(self.allocator);
    errdefer self.allocator.free(slice);
    try self.owned_strings.append(self.allocator, slice);
    return slice;
}

/// Decode exactly 4 hex digits at `raw[at..]` into a UTF-8 codepoint appended
/// to `out`; returns the index just past the digits.
fn appendUnicode(self: *Parser, out: *std.ArrayList(u8), raw: []const u8, at: usize) ParserError!usize {
    if (at + 4 > raw.len) return error.InvalidUnicode;
    const cp = std.fmt.parseInt(u21, raw[at .. at + 4], 16) catch return error.InvalidUnicode;
    Unicode.encodeAppend(out, self.allocator, cp) catch |err| switch (err) {
        error.InvalidCodepoint => return error.InvalidUnicode,
        error.OutOfMemory => return error.OutOfMemory,
    };
    return at + 4;
}

// ── Tests ───────────────────────────────────────────────────────────────────

fn expectRoot(input: []const u8, key: []const u8, value: []const u8) !void {
    var ast = try parseAbstract(testing.allocator, input, .PROPERTIES);
    defer ast.deinit();
    const v = AST.getValByPath(&ast, &.{.{ .key = key }}) catch |err| {
        std.debug.print("path lookup failed: {}\n", .{err});
        return err;
    };
    try testing.expectEqualStrings(value, v.kind.string);
}

test "equals separator" {
    try expectRoot("db.host=localhost\n", "db.host", "localhost");
}

test "colon separator" {
    try expectRoot("db.host: localhost\n", "db.host", "localhost");
}

test "whitespace-only separator" {
    try expectRoot("db.host localhost\n", "db.host", "localhost");
}

test "surrounding whitespace around the separator is trimmed" {
    try expectRoot("db.host   =   localhost\n", "db.host", "localhost");
}

test "a bare key with no separator has an empty value" {
    try expectRoot("flag\n", "flag", "");
}

test "line continuation joins onto the next physical line, stripping its leading whitespace" {
    try expectRoot("long=part1\\\n   part2\n", "long", "part1part2");
}

test "escapes: \\t \\n \\r \\f \\\\" {
    try expectRoot("v=a\\tb\\nc\\\\d\n", "v", "a\tb\nc\\d");
}

test "unicode escape" {
    try expectRoot("v=\\u00e9\n", "v", "\u{e9}"); // é
}

test "an unrecognized escape decodes to the literal escaped character" {
    try expectRoot("v=\\:\\=\\#\\ x\n", "v", ":=# x");
}

test "escaping a separator character embeds it literally in the key" {
    try expectRoot("a\\:b=1\n", "a:b", "1");
}

test "comments: `#` and `!`, full-line only" {
    var ast = try parseAbstract(testing.allocator,
        \\# a header
        \\! also a comment
        \\name=fig
        \\
    , .PROPERTIES);
    defer ast.deinit();
    const key_node = (try ast.firstChildKey(&ast.node(ast.root))).?;
    const cs = ast.comments(key_node.id);
    try testing.expectEqual(@as(usize, 2), cs.leading.len);
    try testing.expectEqualStrings("a header", cs.leading[0].text);
    try testing.expectEqualStrings("also a comment", cs.leading[1].text);
}

test "a `#` in the middle of a line is literal, not a comment" {
    try expectRoot("v=a#b\n", "v", "a#b");
}

test "empty key" {
    try expectRoot("=value\n", "", "value");
}

test "repeated key keeps first position, last value, and warns" {
    var report: Report = .{};
    const doc = try parseWithReport(testing.allocator, "a=1\nb=2\na=3\n", .PROPERTIES, &report);
    defer doc.deinit(testing.allocator);
    const a = try AST.getValByPath(&doc.ast, &.{.{ .key = "a" }});
    try testing.expectEqualStrings("3", a.kind.string);
    try testing.expectEqual(@as(usize, 1), report.warnings.len);
    try testing.expectEqual(Warning.Code.duplicate_key, report.warnings[0].code);
    testing.allocator.free(report.warnings);
}

test "invalid unicode escape is an error" {
    try testing.expectError(error.InvalidUnicode, parseAbstract(testing.allocator, "v=\\uZZZZ\n", .PROPERTIES));
}
