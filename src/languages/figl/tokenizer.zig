//! fig authoring dialect — low-level scanning helpers shared by the block-layer
//! and flow-mode parsers (see `parser.zig`). These are pure functions/structs
//! operating on caller-owned byte slices; no parser state lives here, mirroring
//! how the TOML tokenizer separates lexical classification from tree assembly.
//!
//! Scope (see DESIGN.md "Grammar" / "The context-sensitive layer"):
//!   * `sniffBare` — the literal-else-string bare-token classifier (rule 3).
//!   * Quoted / multiline string scanning (rule 5, "suspended regions").
//!   * Bare-key character classification.

const std = @import("std");
const Allocator = std.mem.Allocator;
const AST = @import("../../kit/ast/ast.zig");
const literal = @import("../../kit/literal.zig");

pub const ExtKind = AST.Node.Kind.Extended.ExtKind;

/// The literal-else-string classifier is the contract's, not figl's alone:
/// the editor types a caller's value text by the same rule. It lives in
/// the kit's `literal.zig`, shared by core and this tokenizer.
pub const Sniffed = literal.Sniffed;
pub const sniffBare = literal.sniffBare;
pub const NumberResult = literal.NumberResult;
pub const sniffNumber = literal.sniffNumber;

/// A bare key/identifier char: letters, digits, `_`, `-`.
pub fn isBareKeyChar(c: u8) bool {
    return std.ascii.isAlphanumeric(c) or c == '_' or c == '-';
}

/// How a `[`/`{`-led block RHS commits (DESIGN.md "Committed values"): commitment
/// is a property of the *whole* RHS, not just the first char.
pub const BracketCommit = enum {
    /// The matching close is the final non-comment token on the line → parse as
    /// flow (committed; a later parse failure is a hard error).
    flow,
    /// A balanced close is followed by more non-comment content (`[text](url)`, a
    /// glob, a regex) → never flow; the whole line is a bare string.
    bare_trailing,
    /// The bracket does not close on this line → multi-line flow region *or* a
    /// genuine truncation. Handed to the flow parser (it scans across lines and
    /// raises FigUnclosedFlow at EOF).
    unclosed,
};

/// Decide how a `[`/`{`-led block RHS commits. `start` indexes the opening
/// bracket; scanning stops at the first newline (bare block values are
/// single-line). Quoted *elements* are skipped so brackets inside strings don't
/// count toward matching (`['[Blog](x)']` closes at its final `]`, not the inner
/// one); a quote that the flow grammar would read as ordinary text is ordinary
/// text here too — see `quoteOpensSpan`.
pub fn classifyBracketCommit(source: []const u8, start: usize) BracketCommit {
    const close = bracketCloseIndex(source, start) orelse return .unclosed;
    return if (restIsCommentOnly(source, close + 1)) .flow else .bare_trailing;
}

/// May a `'`/`"` at this point open a quoted span? Only where the flow grammar
/// would begin a quoted value or key: at an **element start** — the first
/// non-whitespace byte after the opening `[`/`{`, after a `,`, or after a
/// `:`/`=` pair separator (`parseFlowScalarOrNested`/`parseFlowKey` in
/// parser.zig peek exactly there). `prev` is the last non-whitespace byte
/// scanned, or 0 at the start of the scan.
///
/// Anywhere else a quote is ordinary bare-value text, because that is how
/// `scanFlowBareValue` reads it: `[its parent's comment](x.md)` has a bare
/// element with an apostrophe in it, so the apostrophe opens nothing and the
/// bracket still balances at the `]`. Treating a mid-word quote as an opener
/// used to run the span off the end of the line, making the value `.unclosed`
/// and handing a markdown link to the flow parser.
///
/// `:`/`=` are honoured in sequence position too, where the flow grammar has no
/// pair separator, since admitting them keeps the pre-existing reading of a
/// shape (`[a: 'b]c']`) that is malformed under either reading.
///
/// Inside a `{`, whitespace is an element start as well: a compact object's
/// pairs are whitespace-separated, so a quoted key may follow a space
/// (`{a=1 "k}"=2}`). `BracketWalk` tracks which bracket is innermost for that.
fn quoteOpensSpan(prev: u8) bool {
    return switch (prev) {
        0, '[', '{', ',', ':', '=' => true,
        else => false,
    };
}

/// The bracket-matching scanners' state: nesting depth, which open brackets
/// are `{` (one bit per depth; past 64 levels a bracket counts as `[`), the
/// last non-whitespace byte, and whether whitespace came after it.
const BracketWalk = struct {
    depth: usize = 0,
    braces: u64 = 0,
    prev: u8 = 0,
    prev_ws: bool = false,

    fn inBrace(w: *const BracketWalk) bool {
        if (w.depth == 0 or w.depth > 64) return false;
        return (w.braces >> @intCast(w.depth - 1)) & 1 == 1;
    }

    fn quoteOpens(w: *const BracketWalk) bool {
        return quoteOpensSpan(w.prev) or (w.prev_ws and w.inBrace());
    }

    fn open(w: *BracketWalk, c: u8) void {
        if (w.depth < 64) {
            const bit = @as(u64, 1) << @intCast(w.depth);
            if (c == '{') w.braces |= bit else w.braces &= ~bit;
        }
        w.depth += 1;
    }

    fn byte(w: *BracketWalk, c: u8) void {
        w.prev = c;
        w.prev_ws = false;
    }
};

/// Index of the matching close bracket for the `[`/`{` at `start`, scanning at
/// most to the first newline and skipping quoted spans (`quoteOpensSpan` decides
/// which quotes open one) — the balanced-prefix span the block-layer commit rule
/// (and its coercion warn) reasons about. Null when the bracket doesn't close on
/// the line.
pub fn bracketCloseIndex(source: []const u8, start: usize) ?usize {
    std.debug.assert(source[start] == '[' or source[start] == '{');
    var w: BracketWalk = .{};
    var i = start;
    while (i < source.len and source[i] != '\n') {
        const c = source[i];
        switch (c) {
            ' ', '\t', '\r' => {
                w.prev_ws = true;
                i += 1;
            },
            '\'', '"' => {
                const opens = w.quoteOpens();
                w.byte(c);
                if (opens) {
                    i = skipQuotedSpan(source, i) orelse return null;
                } else i += 1;
            },
            '[', '{' => {
                w.open(c);
                w.byte(c);
                i += 1;
            },
            ']', '}' => {
                w.depth -= 1;
                w.byte(c);
                i += 1;
                if (w.depth == 0) return i - 1;
            },
            else => {
                w.byte(c);
                i += 1;
            },
        }
    }
    return null;
}

/// Skip a single-line quoted span starting at the opening quote `source[start]`.
/// Returns the index just past the close, or null if it doesn't close before the
/// newline (an unterminated quote at an element start inside the brackets →
/// treat as `.unclosed`). `"` honors `\`-escapes; `'` is raw (mirroring the
/// value grammar).
fn skipQuotedSpan(source: []const u8, start: usize) ?usize {
    const q = source[start];
    var i = start + 1;
    while (i < source.len and source[i] != '\n') : (i += 1) {
        if (q == '"' and source[i] == '\\') {
            i += 1;
            continue;
        }
        if (source[i] == q) return i + 1;
    }
    return null;
}

/// After a balanced close at `from`, is the rest of the line only whitespace and
/// an optional `#` comment? Honors the `#`-after-whitespace rule: a `#` glued to
/// the close (`[1,2]#x`) is literal, so the value is `.bare_trailing`, not flow.
fn restIsCommentOnly(source: []const u8, from: usize) bool {
    var i = from;
    var saw_space = false;
    while (i < source.len and (source[i] == ' ' or source[i] == '\t' or source[i] == '\r')) : (i += 1) saw_space = true;
    if (i >= source.len or source[i] == '\n') return true;
    return saw_space and source[i] == '#';
}

/// Flow-element twin of `classifyBracketCommit`. Inside a flow collection an
/// element ends at the next top-level `,`/`]`/`}` (or newline / `#` / EOF), not
/// at end-of-line, and flow spans newlines — so both the terminal test and the
/// scan range differ from the block-layer classifier. A balanced bracket group
/// followed by a flow terminator is genuine nested flow (`[1, 2]` as a list
/// member); one followed by more content is a bare-string element (`[Blog](/x)`,
/// a markdown link) — the flow-position form of the balanced-then-trailing rule
/// that lets markdown links go unquoted. There is no `.unclosed` outcome: an
/// unterminated bracket is left to the flow parser (which raises
/// FigUnclosedFlow), so this reports `.flow`. Quoted spans are skipped on the
/// same element-start rule as the block-layer scanner (`quoteOpensSpan`).
///
/// `ws_ends` is set for a compact object's value, which is one token: there,
/// whitespace after the close also terminates (`{a=[1, 2] b=3}` is an array
/// and the next pair), and only content glued to the close makes a bare string
/// (`{href=[Blog](/x.md)}`).
pub fn classifyFlowBracket(source: []const u8, start: usize, ws_ends: bool) BracketCommit {
    std.debug.assert(source[start] == '[' or source[start] == '{');
    var w: BracketWalk = .{};
    var i = start;
    while (i < source.len) {
        const c = source[i];
        switch (c) {
            // flow spans newlines; neither ends an element start
            ' ', '\t', '\r', '\n' => {
                w.prev_ws = true;
                i += 1;
            },
            '\'', '"' => {
                const opens = w.quoteOpens();
                w.byte(c);
                if (opens) {
                    i = skipQuotedSpan(source, i) orelse return .flow;
                } else i += 1;
            },
            '[', '{' => {
                w.open(c);
                w.byte(c);
                i += 1;
            },
            ']', '}' => {
                w.depth -= 1;
                w.byte(c);
                i += 1;
                if (w.depth == 0) {
                    const terminal = if (ws_ends) compactRestIsTerminator(source, i) else flowRestIsTerminator(source, i);
                    return if (terminal) .flow else .bare_trailing;
                }
            },
            else => {
                w.byte(c);
                i += 1;
            },
        }
    }
    return .flow;
}

/// After a flow element's balanced close at `from`, does the next non-blank byte
/// terminate the element (`,`/`]`/`}`), end the line/input, or open a comment
/// (`#`)? If so the bracket group WAS the whole value → genuine nested flow;
/// otherwise more content follows and it is a bare-string element.
fn flowRestIsTerminator(source: []const u8, from: usize) bool {
    var i = from;
    while (i < source.len and (source[i] == ' ' or source[i] == '\t' or source[i] == '\r')) : (i += 1) {}
    if (i >= source.len) return true;
    return switch (source[i]) {
        ',', ']', '}', '\n', '#' => true,
        else => false,
    };
}

/// `flowRestIsTerminator` for a compact object's value: the byte right after the
/// close must end the token — whitespace, `,`/`]`/`}`, or end of input.
fn compactRestIsTerminator(source: []const u8, from: usize) bool {
    if (from >= source.len) return true;
    return isCompactStop(source[from]);
}

/// Where a compact object's unquoted value (one token) ends.
pub fn isCompactStop(c: u8) bool {
    return switch (c) {
        ' ', '\t', '\r', '\n', ',', ']', '}' => true,
        else => false,
    };
}

// ── Quoted / multiline string scanning ──────────────────────────────────────
// Each scanner starts at the OPENING delimiter and returns the decoded text
// plus the index just past the CLOSING delimiter. Decoded text is always
// caller-owned (allocated with `allocator`).

pub const ScanError = error{
    FigUnclosedString,
    FigBadEscape,
} || Allocator.Error;

pub const ScanResult = struct { text: []const u8, end: usize };

/// `'...'` — raw/literal, no escapes. A newline before the close is an error
/// (single-line only; use `'''...'''` for verbatim multi-line blobs).
pub fn scanSingleQuoted(allocator: Allocator, source: []const u8, start: usize) ScanError!ScanResult {
    std.debug.assert(source[start] == '\'');
    const body = start + 1;
    if (std.mem.indexOfAnyPos(u8, source, body, "'\n")) |j| {
        if (source[j] == '\n') return error.FigUnclosedString;
        // No escapes to decode → the body is one contiguous slice.
        return .{ .text = try allocator.dupe(u8, source[body..j]), .end = j + 1 };
    }
    return error.FigUnclosedString;
}

/// `"..."` — escaped: `\n \t \r \\ \" \' \uXXXX`. A newline before the close is
/// an error.
pub fn scanDoubleQuoted(allocator: Allocator, source: []const u8, start: usize) ScanError!ScanResult {
    std.debug.assert(source[start] == '"');
    var out: std.ArrayList(u8) = .empty;
    errdefer out.deinit(allocator);
    var i = start + 1;
    // Jump straight to the next quote/backslash/newline, bulk-copying the plain
    // run in between, instead of appending byte-by-byte. A string with no
    // escapes never grows `out` — it is duped from the source in one shot.
    while (std.mem.indexOfAnyPos(u8, source, i, "\"\\\n")) |j| {
        switch (source[j]) {
            '\n' => return error.FigUnclosedString,
            '"' => {
                if (out.items.len == 0)
                    return .{ .text = try allocator.dupe(u8, source[start + 1 .. j]), .end = j + 1 };
                try out.appendSlice(allocator, source[i..j]);
                return .{ .text = try out.toOwnedSlice(allocator), .end = j + 1 };
            },
            else => { // '\\'
                try out.appendSlice(allocator, source[i..j]); // literal run before the escape
                i = try decodeEscape(allocator, source, j, &out);
            },
        }
    }
    return error.FigUnclosedString;
}

/// Decode one `\...` escape starting at `source[i] == '\\'`, appending the
/// decoded byte(s) to `out`. Returns the index just past the escape.
fn decodeEscape(allocator: Allocator, source: []const u8, i: usize, out: *std.ArrayList(u8)) ScanError!usize {
    if (i + 1 >= source.len) return error.FigBadEscape;
    const e = source[i + 1];
    switch (e) {
        'n' => try out.append(allocator, '\n'),
        't' => try out.append(allocator, '\t'),
        'r' => try out.append(allocator, '\r'),
        '\\' => try out.append(allocator, '\\'),
        '"' => try out.append(allocator, '"'),
        '\'' => try out.append(allocator, '\''),
        'u' => {
            if (i + 6 > source.len) return error.FigBadEscape;
            const hex = source[i + 2 .. i + 6];
            const cp = std.fmt.parseInt(u21, hex, 16) catch return error.FigBadEscape;
            var buf: [4]u8 = undefined;
            const len = std.unicode.utf8Encode(cp, &buf) catch return error.FigBadEscape;
            try out.appendSlice(allocator, buf[0..len]);
            return i + 6;
        },
        else => return error.FigBadEscape,
    }
    return i + 2;
}

/// `'''` ... `'''` — raw/verbatim multi-line. The opener line may carry a
/// trailing `# comment` (returned separately, per DESIGN.md); content begins
/// at the next line and is copied byte-for-byte (no dedent, no escapes) up to
/// (not including) the final line's trailing newline.
pub fn scanTripleSingle(allocator: Allocator, source: []const u8, start: usize) ScanError!MultilineResult {
    return scanTriple(allocator, source, start, '\'', false);
}

/// `"""` ... `"""` — escaped, with the common leading indentation (measured
/// from the closing delimiter's own line) stripped from every content line.
pub fn scanTripleDouble(allocator: Allocator, source: []const u8, start: usize) ScanError!MultilineResult {
    return scanTriple(allocator, source, start, '"', true);
}

pub const MultilineResult = struct {
    text: []const u8,
    end: usize,
    /// A `# comment` on the opening delimiter's own line, if any.
    opener_comment: ?[]const u8,
};

fn scanTriple(allocator: Allocator, source: []const u8, start: usize, q: u8, dedent_and_escape: bool) ScanError!MultilineResult {
    std.debug.assert(source[start] == q and source[start + 1] == q and source[start + 2] == q);
    var i = start + 3;
    // Rest of the opener line: only whitespace and an optional `# comment` are
    // meaningful; content capture begins at the next newline.
    var opener_comment: ?[]const u8 = null;
    while (i < source.len and source[i] != '\n') : (i += 1) {
        if (source[i] == '#') {
            const cstart = i + 1;
            var j = cstart;
            while (j < source.len and source[j] != '\n') j += 1;
            opener_comment = std.mem.trim(u8, source[cstart..j], " \t\r");
            i = j;
            break;
        }
    }
    if (i < source.len and source[i] == '\n') i += 1;
    const body_start = i;

    // The closing delimiter is LINE-ANCHORED (spec § 4.6): the block closes at
    // the first line whose first non-whitespace content begins with the triple
    // delimiter. The closer's leading whitespace is not content (for `"""` it
    // defines the dedent), and a delimiter appearing mid-line is ordinary
    // content — never a closer, so no text is ever silently dropped.
    var line_start = body_start;
    const close_pos = while (line_start <= source.len) {
        const line_end = std.mem.indexOfScalarPos(u8, source, line_start, '\n') orelse source.len;
        var j = line_start;
        while (j < line_end and (source[j] == ' ' or source[j] == '\t')) j += 1;
        if (j + 3 <= source.len and source[j] == q and source[j + 1] == q and source[j + 2] == q)
            break j;
        if (line_end >= source.len) return error.FigUnclosedString;
        line_start = line_end + 1;
    } else return error.FigUnclosedString;
    const dedent = close_pos - line_start;

    // Body text excludes the closing line and its preceding newline (a CRLF
    // terminator counts as one line break — the `\r` is trivia, spec § 3.1).
    var body_end = line_start;
    if (body_end > body_start and source[body_end - 1] == '\n') body_end -= 1;
    if (body_end > body_start and source[body_end - 1] == '\r') body_end -= 1;
    const body = if (body_end >= body_start) source[body_start..body_end] else "";

    const end = close_pos + 3;

    var out: std.ArrayList(u8) = .empty;
    errdefer out.deinit(allocator);
    var it = std.mem.splitScalar(u8, body, '\n');
    var first = true;
    while (it.next()) |raw_line| {
        if (!first) try out.append(allocator, '\n');
        first = false;
        var line = raw_line;
        // A line-terminating `\r` (CRLF input) is trivia, so the captured
        // value's line breaks are always LF. A `\r` mid-line stays content.
        if (line.len > 0 and line[line.len - 1] == '\r') line = line[0 .. line.len - 1];
        if (dedent_and_escape) {
            var stripped: usize = 0;
            while (stripped < dedent and line.len > 0 and (line[0] == ' ' or line[0] == '\t')) : (stripped += 1) line = line[1..];
        }
        if (dedent_and_escape) {
            var k: usize = 0;
            while (k < line.len) {
                if (line[k] == '\\') {
                    k = try decodeEscape(allocator, line, k, &out);
                } else {
                    try out.append(allocator, line[k]);
                    k += 1;
                }
            }
        } else {
            try out.appendSlice(allocator, line);
        }
    }

    return .{ .text = try out.toOwnedSlice(allocator), .end = end, .opener_comment = opener_comment };
}

test "sniffNumber rejects leading zero, accepts hex/float/exponent" {
    try std.testing.expect(sniffNumber("007") == null);
    try std.testing.expect(sniffNumber("0") != null);
    try std.testing.expect(sniffNumber("0.5") != null);
    try std.testing.expectEqualStrings("0xFF", sniffNumber("0xFF").?.raw);
    try std.testing.expect(sniffNumber("1.5e3").?.kind == .float);
    try std.testing.expect(sniffNumber("12 monkeys") == null);
}

test "sniffNumber accepts `_` separators in radix integers" {
    // Kept verbatim in `raw` — the lexeme is the value (§ 2).
    try std.testing.expectEqualStrings("0xdead_beef", sniffNumber("0xdead_beef").?.raw);
    try std.testing.expect(sniffNumber("0xdead_beef").?.kind == .integer);
    try std.testing.expectEqualStrings("0b1010_1010", sniffNumber("0b1010_1010").?.raw);
    try std.testing.expectEqualStrings("0o7_5_5", sniffNumber("0o7_5_5").?.raw);
    try std.testing.expectEqualStrings("-0xf_f", sniffNumber("-0xf_f").?.raw);

    // A separator must be anchored between two digits.
    try std.testing.expect(sniffNumber("0x_ff") == null); // leading
    try std.testing.expect(sniffNumber("0xff_") == null); // trailing
    try std.testing.expect(sniffNumber("0xf__f") == null); // doubled
    try std.testing.expect(sniffNumber("0x_") == null);
    try std.testing.expect(sniffNumber("0b1_2") == null); // still radix-checked
    try std.testing.expect(sniffNumber("0xff_.5") == null); // run ends on `_`
}

test "sniffNumber accepts `_` separators in decimal integers, fractions, exponents" {
    try std.testing.expectEqualStrings("1_000", sniffNumber("1_000").?.raw);
    try std.testing.expect(sniffNumber("1_000").?.kind == .integer);
    try std.testing.expect(sniffNumber("1_0.5").?.kind == .float);
    try std.testing.expect(sniffNumber("1.000_1").?.kind == .float); // fraction
    try std.testing.expect(sniffNumber("1e1_0").?.kind == .float); // exponent
    try std.testing.expect(sniffNumber("-1_0.0_1e-1_0").?.kind == .float); // all runs at once
    try std.testing.expectEqualStrings("+9_9", sniffNumber("+9_9").?.raw);

    // Anchored in every run, independently.
    try std.testing.expect(sniffNumber("_1") == null);
    try std.testing.expect(sniffNumber("1_") == null);
    try std.testing.expect(sniffNumber("1__0") == null);
    try std.testing.expect(sniffNumber("1_.5") == null); // integer part
    try std.testing.expect(sniffNumber("1._5") == null); // fraction
    try std.testing.expect(sniffNumber("1.5_") == null);
    try std.testing.expect(sniffNumber("1e_5") == null); // exponent
    try std.testing.expect(sniffNumber("1e5_") == null);
    try std.testing.expect(sniffNumber("1e+_5") == null);

    // The leading-zero rule still wins, and separators count toward the run.
    try std.testing.expect(sniffNumber("0_1") == null);
    try std.testing.expect(sniffNumber("007_5") == null);
    try std.testing.expect(sniffNumber("0") != null);
    try std.testing.expect(sniffNumber("0.000_1") != null); // a lone `0` may lead a float
}

test "sniffBare classifies literals" {
    try std.testing.expect(sniffBare("null") == .null_);
    try std.testing.expect(sniffBare("true").boolean == true);
    try std.testing.expect(sniffBare("Yes") == .string);
    try std.testing.expect(sniffBare("007") == .string);
    try std.testing.expect(sniffBare("2026-07-01").datetime.kind == .local_date);
}

test "scanDoubleQuoted decodes escapes" {
    const a = std.testing.allocator;
    const r = try scanDoubleQuoted(a, "\"a\\tb\\n\\u2603\"", 0);
    defer a.free(r.text);
    try std.testing.expectEqualStrings("a\tb\n☃", r.text);
}

test "scanTripleDouble dedents and decodes" {
    const a = std.testing.allocator;
    const src =
        \\"""
        \\    hi
        \\    there
        \\    """
    ;
    const r = try scanTripleDouble(a, src, 0);
    defer a.free(r.text);
    try std.testing.expectEqualStrings("hi\nthere", r.text);
}

test "classifyBracketCommit: flow vs bare-trailing vs unclosed" {
    // terminal close → flow
    try std.testing.expect(classifyBracketCommit("[80, 443]\n", 0) == .flow);
    try std.testing.expect(classifyBracketCommit("[80, 443]  # c\n", 0) == .flow);
    try std.testing.expect(classifyBracketCommit("{}", 0) == .flow);
    try std.testing.expect(classifyBracketCommit("[['a, b']]", 0) == .flow); // brackets inside quotes don't count
    // balanced then trailing content → bare string
    try std.testing.expect(classifyBracketCommit("[Blog](/x)\n", 0) == .bare_trailing);
    try std.testing.expect(classifyBracketCommit("[a-z]*.md", 0) == .bare_trailing);
    try std.testing.expect(classifyBracketCommit("[b]x[/b]", 0) == .bare_trailing);
    try std.testing.expect(classifyBracketCommit("[80]#glued", 0) == .bare_trailing); // glued # is literal
    // never closes on the line → unclosed (multi-line flow or truncation)
    try std.testing.expect(classifyBracketCommit("[80, 443", 0) == .unclosed);
    try std.testing.expect(classifyBracketCommit("[\n", 0) == .unclosed);
    try std.testing.expect(classifyBracketCommit("['unterminated]", 0) == .unclosed);
}

test "classifyBracketCommit: a mid-word quote is content, not a span opener" {
    // Only an element-start quote opens a span, so an apostrophe inside a bare
    // element leaves the bracket balanced and the RHS a bare string.
    try std.testing.expect(classifyBracketCommit("[its parent's comment](x.md)\n", 0) == .bare_trailing);
    try std.testing.expect(classifyBracketCommit("[its parent\"s comment](x.md)\n", 0) == .bare_trailing);
    try std.testing.expect(classifyBracketCommit("[text](it's.md)\n", 0) == .bare_trailing);
    try std.testing.expect(classifyBracketCommit("[its parent's comment]\n", 0) == .flow);
    // Element starts: after `[`/`{`, a `,`, and a `:`/`=` pair separator.
    try std.testing.expect(classifyBracketCommit("[\"a]b\", c]\n", 0) == .flow);
    try std.testing.expect(classifyBracketCommit("[a, 'b]c']\n", 0) == .flow);
    try std.testing.expect(classifyBracketCommit("{k = 'a]b'}\n", 0) == .flow);
    try std.testing.expect(classifyBracketCommit("{\"a]b\": 1}\n", 0) == .flow);
}

test "classifyFlowBracket: mid-word quote in an element" {
    try std.testing.expect(classifyFlowBracket("[Blog](it's/x.md), b]", 0, false) == .bare_trailing);
    try std.testing.expect(classifyFlowBracket("['a]b'], c]", 0, false) == .flow);
}

test "classifyFlowBracket: nested flow vs bare-trailing element" {
    // A balanced group followed by a flow terminator is a nested collection.
    try std.testing.expect(classifyFlowBracket("[1, 2], 3]", 0, false) == .flow);
    try std.testing.expect(classifyFlowBracket("[1, 2]]", 0, false) == .flow);
    try std.testing.expect(classifyFlowBracket("{ x = 1 }, y]", 0, false) == .flow);
    try std.testing.expect(classifyFlowBracket("[a, b]\n", 0, false) == .flow);
    // A balanced group with trailing content is a bare-string element (a
    // markdown link, glob, regex) — the flow twin of the block bare-trailing.
    try std.testing.expect(classifyFlowBracket("[Blog](/x), [Resume](/y)]", 0, false) == .bare_trailing);
    try std.testing.expect(classifyFlowBracket("[a-z]*.md, next]", 0, false) == .bare_trailing);
    // Unterminated → left to the flow parser (reported as flow).
    try std.testing.expect(classifyFlowBracket("[1, 2", 0, false) == .flow);
}

test "classifyFlowBracket: a compact object's value ends at whitespace" {
    // Whitespace after the close ends the token: the group is the value and the
    // rest is the next pair.
    try std.testing.expect(classifyFlowBracket("[1, 2] b=3}", 0, true) == .flow);
    try std.testing.expect(classifyFlowBracket("{x=1}\n}", 0, true) == .flow);
    try std.testing.expect(classifyFlowBracket("[1]}", 0, true) == .flow);
    try std.testing.expect(classifyFlowBracket("[1]", 0, true) == .flow);
    // Only glued content makes a bare string.
    try std.testing.expect(classifyFlowBracket("[Blog](/x.md) b=3}", 0, true) == .bare_trailing);
    try std.testing.expect(classifyFlowBracket("[1]#x}", 0, true) == .bare_trailing);
}

test "a quote after whitespace inside a brace opens a span" {
    // A compact object's quoted key follows a space, so its brackets don't count.
    try std.testing.expect(bracketCloseIndex("{a=1 \"k}\"=2}", 0) == 11);
    try std.testing.expect(classifyBracketCommit("{a=1 'k]'=2}\n", 0) == .flow);
    // In a `[`, the same quote is bare-value text, as before.
    try std.testing.expect(classifyBracketCommit("[its 'odd] one\n", 0) == .bare_trailing);
}

test "scanTripleSingle is verbatim" {
    const a = std.testing.allocator;
    const src = "'''\nraw \\n text\n'''";
    const r = try scanTripleSingle(a, src, 0);
    defer a.free(r.text);
    try std.testing.expectEqualStrings("raw \\n text", r.text);
}
