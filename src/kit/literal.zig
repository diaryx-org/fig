//! The bare literal: what a value's unquoted text means when nothing else
//! says — `null`, `true`/`false`, a number, a datetime, or else a string. One
//! rule for the whole contract: the editor classifies a caller's value text by
//! it (`lang.Literal`, which a value renderer is told), and figl reads its bare
//! tokens by it.

const std = @import("std");
const datetime = @import("datetime.zig");
const ascii = @import("ascii.zig");

/// The result of sniffing a bare (unquoted, undelimited) token.
pub const Sniffed = union(enum) {
    null_,
    boolean: bool,
    number: NumberResult,
    datetime: struct { kind: datetime.Kind, raw: []const u8 },
    /// Nothing typed matched; the caller keeps the original text verbatim as a
    /// plain string (the literal-else-string fallback).
    string,
};

/// Literal-else-string: classify a trimmed bare token. Never fails — a token
/// that matches nothing typed sniffs as `.string`. Enum atoms and `inf`/`nan`
/// are NOT sniffed here — they are explicit-typing-only (figl's DESIGN.md "Enum:
/// explicit-only"), so a bare `@x`/`inf` correctly falls through to `.string`.
/// `true`/`false`/`null` are matched case-sensitively (lowercase only) — `Yes`,
/// `TRUE`, `on` stay strings (the Norway-problem fix).
pub fn sniffBare(token: []const u8) Sniffed {
    if (token.len == 0) return .string;
    if (std.mem.eql(u8, token, "null")) return .null_;
    if (std.mem.eql(u8, token, "true")) return .{ .boolean = true };
    if (std.mem.eql(u8, token, "false")) return .{ .boolean = false };
    if (sniffNumber(token)) |n| return .{ .number = n };
    if (datetime.classify(token, .{})) |k| {
        return .{ .datetime = .{ .kind = k, .raw = token } };
    } else |_| {}
    return .string;
}

pub const NumberResult = struct { raw: []const u8, kind: enum { integer, float } };

/// Scan a digit run at `start`: `pred` digits with optional `_` separators.
/// Returns the index just past the run, or null if the run is empty or a
/// separator is unanchored. The TOML parser's `validUnderscored` is the same
/// rule, but *validates a known slice*; fig has to scan to a boundary because
/// the `.`/`e` that ends a run is only found by walking the digits.
fn scanUnderscored(s: []const u8, start: usize, pred: *const fn (u8) bool) ?usize {
    var j = start;
    var prev_us = true; // a separator may not open the run...
    while (j < s.len) : (j += 1) {
        const c = s[j];
        if (c == '_') {
            if (prev_us) return null; // ...nor be doubled
            prev_us = true;
        } else if (pred(c)) {
            prev_us = false;
        } else break;
    }
    if (prev_us) return null; // ...nor close it; an empty run trips this too
    return j;
}

/// TOML-style bare-number sniff: decimal integers/floats (with optional
/// exponent) and `0x`/`0o`/`0b` integers. A leading zero on a multi-digit
/// decimal integer part is NOT a number — it stays a string, keeping
/// zero-padded IDs/zip codes intact (figl's DESIGN.md "Leading-zero rule").
///
/// `_` digit separators are accepted in every digit run — radix, integer part,
/// fraction, and exponent (`0xdead_beef`, `1_000`, `1_000.000_1e1_0`) — each
/// anchored between two digits, matching TOML and ZON. They are part of the
/// lexeme (§ 2), so `0xff` and `0xf_f` are distinct numbers.
pub fn sniffNumber(token: []const u8) ?NumberResult {
    if (token.len == 0) return null;
    var i: usize = 0;
    if (token[0] == '+' or token[0] == '-') i = 1;
    if (i >= token.len) return null;
    const body = token[i..];

    if (body.len >= 2 and body[0] == '0' and (body[1] == 'x' or body[1] == 'o' or body[1] == 'b')) {
        // Pick the radix classifier once — `body[1]` is loop-invariant.
        const isValidDigit: *const fn (u8) bool = switch (body[1]) {
            'x' => &ascii.isHex,
            'o' => &ascii.isOctal,
            'b' => &ascii.isBinary,
            else => unreachable,
        };
        const end = scanUnderscored(body, 2, isValidDigit) orelse return null;
        if (end != body.len) return null; // trailing garbage -> not a number
        return .{ .raw = token, .kind = .integer };
    }

    var j = scanUnderscored(token, i, &ascii.isDigit) orelse return null;
    // Separators count toward the length, so `0_1` is a leading zero too.
    if (j - i > 1 and token[i] == '0') return null; // leading zero
    var is_float = false;
    if (j < token.len and token[j] == '.') {
        is_float = true;
        // A dot needs >=1 fractional digit; the fraction may lead with a zero.
        j = scanUnderscored(token, j + 1, &ascii.isDigit) orelse return null;
    }
    if (j < token.len and (token[j] == 'e' or token[j] == 'E')) {
        is_float = true;
        j += 1;
        if (j < token.len and (token[j] == '+' or token[j] == '-')) j += 1;
        j = scanUnderscored(token, j, &ascii.isDigit) orelse return null;
    }
    if (j != token.len) return null; // trailing garbage -> not a clean number
    return .{ .raw = token, .kind = if (is_float) .float else .integer };
}

test "sniffBare types null, bools, numbers and datetimes, and keeps everything else a string" {
    const t = std.testing;
    try t.expect(sniffBare("null") == .null_);
    try t.expect(sniffBare("true").boolean);
    try t.expect(sniffBare("42").number.kind == .integer);
    try t.expect(sniffBare("0x1F").number.kind == .integer);
    try t.expect(sniffBare("2.5e3").number.kind == .float);
    try t.expect(sniffBare("2026-07-01").datetime.kind == .local_date);
    for ([_][]const u8{ "", "Yes", "TRUE", "007", "1_", "inf", "@x" }) |s| try t.expect(sniffBare(s) == .string);
}
