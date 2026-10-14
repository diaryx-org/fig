//! Number-lexeme rendering shared by the printers.
//!
//! A number's `raw` is the source lexeme — the payload IS the value (`ast.zig`)
//! — so a printer's instinct is to write it back verbatim. That is only correct
//! when the target format reads the lexeme back as the *same number*. It often
//! doesn't: YAML 1.2 resolves `0b1010` to a **string**, and JSON5 rejects
//! `0o755` outright. Writing raw there emits a silent type change, or output
//! the target cannot parse at all.
//!
//! So a printer asks `spellable(raw, <target>)` first, and falls back to
//! `writeCanonical` — decimal, the one spelling every format shares. This is a
//! *spelling* degrade: the value is unchanged, only its notation.
//!
//! The non-finite floats are the exception: every format that has them spells
//! them its own way (`inf` in TOML and ZON, `.inf` in YAML, `Infinity` in
//! JSON5), a `raw` carries whichever spelling its source used, and decimal has
//! none. So `nonFinite` reads any of them, and `write` respells one in the
//! target's own words — or refuses with `error.NonFiniteNumber` when the target
//! has no words for it (strict JSON).

const std = @import("std");

/// The number spellings a target format reads back as a number. A `false` field
/// means the printer must canonicalize that spelling to decimal instead.
pub const Spelling = struct {
    /// `0x1F`
    hex: bool = false,
    /// `0o17`
    octal: bool = false,
    /// `0b1010`
    binary: bool = false,
    /// `1_000` — digit separators
    underscores: bool = false,
    /// `0755` — a multi-digit integer part led by a zero
    leading_zero: bool = false,
    /// `.5` / `5.`
    bare_dot: bool = false,
    /// a leading `+`
    plus: bool = false,
    /// How the format spells infinity and NaN, or null when it cannot.
    non_finite: ?NonFiniteNames = null,
};

/// A non-finite float, whatever its source spelling.
pub const NonFinite = enum { inf, neg_inf, nan };

/// A format's own spelling of each `NonFinite`.
pub const NonFiniteNames = struct {
    inf: []const u8,
    neg_inf: []const u8,
    nan: []const u8,

    pub fn of(self: NonFiniteNames, v: NonFinite) []const u8 {
        return switch (v) {
            .inf => self.inf,
            .neg_inf => self.neg_inf,
            .nan => self.nan,
        };
    }
};

/// TOML 1.0 and ZON: `inf`, `-inf`, `nan`.
pub const plain_non_finite: NonFiniteNames = .{ .inf = "inf", .neg_inf = "-inf", .nan = "nan" };

/// Strict JSON: none of it. Every non-decimal spelling canonicalizes.
pub const json: Spelling = .{};

/// JSON5 numbers are ES5.1 `NumericLiteral`: hex, leading `+`, and bare dots —
/// but NOT `0o`/`0b` (ES6), `_` (ES2021), or a leading zero. Verified against
/// this repo's own JSON5 tokenizer, which rejects all four.
pub const json5: Spelling = .{
    .hex = true,
    .bare_dot = true,
    .plus = true,
    .non_finite = .{ .inf = "Infinity", .neg_inf = "-Infinity", .nan = "NaN" },
};

/// YAML 1.2 core: int is `[-+]?[0-9]+ | 0o[0-7]+ | 0x[0-9a-fA-F]+`, float takes
/// a bare dot. No `0b` and no `_` — 1.2 resolves both to a *string*.
///
/// 1.1's extra spellings (`0b`, `_`, base-60) are deliberately NOT claimed: the
/// YAML printer has no version parameter, so it targets the default type
/// (`yaml.zig`: `default_type = .v1_2_2`). `leading_zero` IS claimed because
/// 1.2 reads `0755` as decimal 755 — note 1.1 would read it as octal 493, an
/// ambiguity the AST cannot represent (the parser stores the lexeme and leaves
/// resolution to the consumer), so it is left alone rather than guessed at.
pub const yaml_1_2: Spelling = .{
    .hex = true,
    .octal = true,
    .leading_zero = true,
    .bare_dot = true,
    .plus = true,
    .non_finite = .{ .inf = ".inf", .neg_inf = "-.inf", .nan = ".nan" },
};

/// Which non-finite float `raw` spells, if it spells one, in any of the ways a
/// parser or a caller stores it: `inf`/`nan` (TOML, ZON), `.inf`/`.nan` in
/// YAML's three cases, and `Infinity`/`NaN` (JSON5) — each with an optional
/// sign, which a NaN ignores.
pub fn nonFinite(raw: []const u8) ?NonFinite {
    var body = raw;
    var negative = false;
    if (body.len > 0 and (body[0] == '+' or body[0] == '-')) {
        negative = body[0] == '-';
        body = body[1..];
    }
    if (body.len > 0 and body[0] == '.') body = body[1..];
    const eq = std.ascii.eqlIgnoreCase;
    if (eq(body, "inf") or eq(body, "infinity")) return if (negative) .neg_inf else .inf;
    if (eq(body, "nan")) return .nan;
    return null;
}

/// Whether a format that can spell `s` reads `raw` back as the same number.
pub fn spellable(raw: []const u8, s: Spelling) bool {
    var body = raw;
    if (body.len > 0 and (body[0] == '+' or body[0] == '-')) {
        if (body[0] == '+' and !s.plus) return false;
        body = body[1..];
    }
    if (body.len == 0) return false;
    if (!s.underscores and std.mem.indexOfScalar(u8, body, '_') != null) return false;

    if (body.len >= 2 and body[0] == '0') switch (body[1] | 0x20) {
        'x' => return s.hex,
        'o' => return s.octal,
        'b' => return s.binary,
        else => {},
    };

    // Decimal from here.
    if (!s.bare_dot and (body[0] == '.' or body[body.len - 1] == '.')) return false;
    if (!s.leading_zero) {
        const int_end = std.mem.indexOfAny(u8, body, ".eE") orelse body.len;
        if (int_end > 1 and body[0] == '0') return false;
    }
    return true;
}

/// Write `raw` verbatim when a format spelling `s` reads it back unchanged,
/// else canonicalized to decimal. A non-finite `raw` is respelled in `s`'s own
/// words, and is `error.NonFiniteNumber` when `s` has none.
pub fn write(writer: anytype, raw: []const u8, comptime s: Spelling) !void {
    if (nonFinite(raw)) |v| {
        if (s.non_finite) |names| return writer.writeAll(names.of(v)) else return error.NonFiniteNumber;
    }
    if (spellable(raw, s)) return writer.writeAll(raw);
    return writeCanonical(writer, raw);
}

/// Write `raw` as a decimal lexeme every format can read: radix converted, `_`
/// and `+` dropped, bare dots padded (`.5` -> `0.5`, `5.` -> `5.0`), leading
/// zeros stripped. Plain decimal digits are copied rather than re-formatted, so
/// arbitrary precision and significant figures survive (`1.10` stays `1.10`).
pub fn writeCanonical(writer: anytype, raw: []const u8) !void {
    var s = raw;
    if (s.len > 0 and s[0] == '-') {
        try writer.writeByte('-');
        s = s[1..];
    } else if (s.len > 0 and s[0] == '+') {
        s = s[1..];
    }

    // Radix integers convert; a lexeme that is not one, or whose decimal
    // would outgrow `buf`, falls back to the sign-stripped source rather
    // than emitting nothing.
    if (s.len >= 2 and s[0] == '0' and (s[1] | 0x20 == 'x' or s[1] | 0x20 == 'o' or s[1] | 0x20 == 'b')) {
        const base: u8 = switch (s[1] | 0x20) {
            'x' => 16,
            'o' => 8,
            else => 2,
        };
        var buf: [512]u8 = undefined;
        if (radixToDecimal(s[2..], base, &buf)) |digits| {
            try writer.writeAll(digits);
            return;
        }
        try writer.writeAll(s);
        return;
    }

    const e_idx = std.mem.indexOfAny(u8, s, "eE");
    const mantissa = if (e_idx) |i| s[0..i] else s;
    const exponent = if (e_idx) |i| s[i..] else "";
    const dot = std.mem.indexOfScalar(u8, mantissa, '.');

    var int_digits: usize = 0;
    var seen_nonzero = false;
    for (if (dot) |d| mantissa[0..d] else mantissa) |c| {
        if (c == '_') continue;
        if (c == '0' and !seen_nonzero) continue; // leading zero
        seen_nonzero = true;
        try writer.writeByte(c);
        int_digits += 1;
    }
    if (int_digits == 0) try writer.writeByte('0'); // `0`, `000`, `.5`

    if (dot) |d| {
        try writer.writeByte('.');
        var frac_digits: usize = 0;
        for (mantissa[d + 1 ..]) |c| {
            if (c == '_') continue;
            try writer.writeByte(c);
            frac_digits += 1;
        }
        if (frac_digits == 0) try writer.writeByte('0'); // `5.` -> `5.0`
    }

    for (exponent) |c| {
        if (c != '_') try writer.writeByte(c);
    }
}

/// What a multi-digit integer led by a zero (`0755`) means where it was
/// read: decimal 755 to YAML 1.2 (and to `writeCanonical`), octal 493 to YAML
/// 1.1. `unknown` when the reader's dialect is not to hand.
pub const LeadingZero = enum { decimal, octal, unknown };

/// A number lexeme as `eqlValue` reads it.
pub const Lexeme = struct {
    raw: []const u8,
    float: bool,
    leading_zero: LeadingZero = .unknown,
};

/// Whether two number lexemes spell the same value, whatever their source
/// formats: what `fig patch` asks before it rewrites a number, so that a
/// value the patch and the target agree on keeps the target's spelling.
///
/// An integer is its exact mathematical value, read across bases (`0x1F`,
/// `0o37`, `0b11111`), signs, `_` separators and digits past 64 bits. A float
/// is its exact decimal value, so `1.50` is `1.5` and `1e3` is `1000.0`, but two
/// lexemes that round to one double while differing in the digits written are
/// different. A non-finite float equals one of the same kind in any format's
/// spelling, a NaN included: the question is whether a rewrite would change
/// the document, not IEEE's. `-0.0` is not `0.0`; an integer has no negative
/// zero, so `-0` is `0`.
///
/// An integer never equals a float, `1` and `1.0` included: YAML and TOML type
/// them apart, so a patch that turns one into the other changes the document.
///
/// A lexeme this cannot read for certain is compared by its bytes alone,
/// which can only ever report a real change as one: a leading-zero integer
/// whose `LeadingZero` is `unknown`, a YAML 1.1 sexagesimal (`1:30`), and
/// anything else outside these spellings or too long for its buffers.
pub fn eqlValue(a: Lexeme, b: Lexeme) bool {
    if (a.float != b.float) return false;
    if (std.mem.eql(u8, a.raw, b.raw)) return true;
    const x_nf = nonFinite(a.raw);
    const y_nf = nonFinite(b.raw);
    if (x_nf != null or y_nf != null) return x_nf == y_nf;

    var a_buf: [512]u8 = undefined;
    var b_buf: [512]u8 = undefined;
    const x = Exact.read(a, &a_buf) orelse return false;
    const y = Exact.read(b, &b_buf) orelse return false;
    if (x.digits.len == 0 and y.digits.len == 0) return !a.float or x.negative == y.negative;
    return x.negative == y.negative and x.exponent == y.exponent and std.mem.eql(u8, x.digits, y.digits);
}

/// A finite number's exact value: `digits` × 10^`exponent`, with neither a
/// leading nor a trailing zero in `digits`, so equal values read equal. Zero
/// is empty `digits`.
const Exact = struct {
    negative: bool,
    digits: []const u8,
    exponent: i64,

    /// `raw`'s value, its digits written into `buf`; null for a lexeme
    /// `eqlValue` compares by bytes instead.
    fn read(lexeme: Lexeme, buf: []u8) ?Exact {
        const float = lexeme.float;
        var s = lexeme.raw;
        var negative = false;
        if (s.len > 0 and (s[0] == '+' or s[0] == '-')) {
            negative = s[0] == '-';
            s = s[1..];
        }
        if (s.len == 0) return null;

        if (s.len >= 2 and s[0] == '0' and (s[1] | 0x20 == 'x' or s[1] | 0x20 == 'o' or s[1] | 0x20 == 'b')) {
            if (float) return null;
            const base: u8 = switch (s[1] | 0x20) {
                'x' => 16,
                'o' => 8,
                else => 2,
            };
            const digits = radixToDecimal(s[2..], base, buf) orelse return null;
            return normalize(negative, digits, 0);
        }

        const e_idx = std.mem.indexOfAny(u8, s, "eE");
        const mantissa = if (e_idx) |i| s[0..i] else s;
        const dot = std.mem.indexOfScalar(u8, mantissa, '.');
        if (!float) {
            if (e_idx != null or dot != null) return null;
            if (s.len > 1 and s[0] == '0') switch (lexeme.leading_zero) {
                .decimal => {},
                .octal => return normalize(negative, radixToDecimal(s[1..], 8, buf) orelse return null, 0),
                .unknown => return null,
            };
        }

        var len: usize = 0;
        var frac_digits: i64 = 0;
        var any = false;
        for (mantissa, 0..) |c, i| {
            if (c == '_' or (dot != null and i == dot.?)) continue;
            if (!std.ascii.isDigit(c)) return null;
            if (len == buf.len) return null;
            buf[len] = c;
            len += 1;
            any = true;
            if (dot != null and i > dot.?) frac_digits += 1;
        }
        if (!any) return null;

        var exponent: i64 = 0;
        if (e_idx) |i| {
            var e = s[i + 1 ..];
            var e_negative = false;
            if (e.len > 0 and (e[0] == '+' or e[0] == '-')) {
                e_negative = e[0] == '-';
                e = e[1..];
            }
            var e_any = false;
            for (e) |c| {
                if (c == '_') continue;
                if (!std.ascii.isDigit(c)) return null;
                exponent = std.math.mul(i64, exponent, 10) catch return null;
                exponent = std.math.add(i64, exponent, c - '0') catch return null;
                e_any = true;
            }
            if (!e_any) return null;
            if (e_negative) exponent = -exponent;
        }
        return normalize(negative, buf[0..len], exponent - frac_digits);
    }

    fn normalize(negative: bool, digits: []const u8, exponent: i64) Exact {
        var d = std.mem.trimStart(u8, digits, "0");
        var e = exponent;
        while (d.len > 0 and d[d.len - 1] == '0') {
            d = d[0 .. d.len - 1];
            e += 1;
        }
        return .{ .negative = negative, .digits = d, .exponent = e };
    }
};

/// `digits`, in `base` with `_` separators, as decimal digits at the end of
/// `buf`; null when a digit is not one of `base`'s, there is none, or the
/// decimal outgrows `buf`. Digit by digit rather than through an integer
/// type, so no width bounds it but `buf`'s.
fn radixToDecimal(digits: []const u8, base: u8, buf: []u8) ?[]const u8 {
    // `buf[start..]` holds the decimal so far, most significant first,
    // each byte a digit's value until the end.
    var start = buf.len;
    var any = false;
    for (digits) |c| {
        if (c == '_') continue;
        const d: u8 = switch (c) {
            '0'...'9' => c - '0',
            'a'...'f' => c - 'a' + 10,
            'A'...'F' => c - 'A' + 10,
            else => return null,
        };
        if (d >= base) return null;
        any = true;
        var carry: u16 = d;
        var i = buf.len;
        while (i > start) {
            i -= 1;
            const x = @as(u16, buf[i]) * base + carry;
            buf[i] = @intCast(x % 10);
            carry = x / 10;
        }
        while (carry > 0) {
            if (start == 0) return null;
            start -= 1;
            buf[start] = @intCast(carry % 10);
            carry /= 10;
        }
    }
    if (!any) return null;
    if (start == buf.len) {
        start -= 1;
        buf[start] = 0;
    }
    for (buf[start..]) |*b| b.* += '0';
    return buf[start..];
}

test "spellable: strict JSON takes only plain decimal" {
    const t = std.testing;
    try t.expect(spellable("1000", json));
    try t.expect(spellable("-1.5e3", json));
    try t.expect(spellable("1.10", json));
    try t.expect(spellable("0", json));
    try t.expect(spellable("0.5", json));
    try t.expect(!spellable("0xff", json));
    try t.expect(!spellable("0o755", json));
    try t.expect(!spellable("0b1010", json));
    try t.expect(!spellable("1_000", json));
    try t.expect(!spellable("+5", json));
    try t.expect(!spellable(".5", json));
    try t.expect(!spellable("5.", json));
    try t.expect(!spellable("0755", json));
}

test "spellable: JSON5 takes hex but not 0o/0b/_" {
    const t = std.testing;
    try t.expect(spellable("0xff", json5));
    try t.expect(spellable("0XFF", json5));
    try t.expect(spellable("+5", json5));
    try t.expect(spellable(".5", json5));
    try t.expect(spellable("5.", json5));
    try t.expect(!spellable("0o755", json5));
    try t.expect(!spellable("0b1010", json5));
    try t.expect(!spellable("0xf_f", json5));
    try t.expect(!spellable("1_000", json5));
    try t.expect(!spellable("0755", json5));
}

test "spellable: YAML 1.2 takes hex and 0o but not 0b/_" {
    const t = std.testing;
    try t.expect(spellable("0xff", yaml_1_2));
    try t.expect(spellable("0o755", yaml_1_2));
    try t.expect(spellable("0755", yaml_1_2)); // 1.2 reads decimal 755
    try t.expect(spellable("1.5e3", yaml_1_2));
    try t.expect(!spellable("0b1010", yaml_1_2)); // resolves to a string
    try t.expect(!spellable("1_000", yaml_1_2)); // resolves to a string
    try t.expect(!spellable("0xdead_beef", yaml_1_2));
}

test "nonFinite reads every source spelling, and nothing else" {
    const t = std.testing;
    inline for (.{ "inf", "+inf", ".inf", ".Inf", ".INF", "+.inf", "Infinity", "+Infinity" }) |raw|
        try t.expectEqual(@as(?NonFinite, .inf), nonFinite(raw));
    inline for (.{ "-inf", "-.inf", "-.INF", "-Infinity" }) |raw|
        try t.expectEqual(@as(?NonFinite, .neg_inf), nonFinite(raw));
    inline for (.{ "nan", "+nan", "-nan", ".nan", ".NaN", ".NAN", "NaN" }) |raw|
        try t.expectEqual(@as(?NonFinite, .nan), nonFinite(raw));
    inline for (.{ "1", "1e999999", "0x1f", ".5", "info", "-", "", "." }) |raw|
        try t.expectEqual(@as(?NonFinite, null), nonFinite(raw));
}

test "write respells a non-finite float, or refuses where the format has none" {
    const t = std.testing;
    var buf: [32]u8 = undefined;
    var w = std.Io.Writer.fixed(&buf);
    try write(&w, "inf", yaml_1_2);
    try t.expectEqualStrings(".inf", w.buffered());
    w = std.Io.Writer.fixed(&buf);
    try write(&w, "-.inf", json5);
    try t.expectEqualStrings("-Infinity", w.buffered());
    w = std.Io.Writer.fixed(&buf);
    try write(&w, ".nan", json5);
    try t.expectEqualStrings("NaN", w.buffered());
    w = std.Io.Writer.fixed(&buf);
    try t.expectError(error.NonFiniteNumber, write(&w, ".inf", json));
    try t.expectError(error.NonFiniteNumber, write(&w, "nan", json));
}

test "writeCanonical" {
    const t = std.testing;
    var buf: [64]u8 = undefined;
    const canon = struct {
        fn f(b: []u8, raw: []const u8) ![]const u8 {
            var w = std.Io.Writer.fixed(b);
            try writeCanonical(&w, raw);
            return w.buffered();
        }
    }.f;

    // Radix -> decimal, separators dropped.
    try t.expectEqualStrings("255", try canon(&buf, "0xff"));
    try t.expectEqualStrings("3735928559", try canon(&buf, "0xdead_beef"));
    try t.expectEqualStrings("493", try canon(&buf, "0o755"));
    try t.expectEqualStrings("10", try canon(&buf, "0b1010"));
    try t.expectEqualStrings("-255", try canon(&buf, "-0xf_f"));
    try t.expectEqualStrings("1000", try canon(&buf, "1_000"));

    // Bare dots padded; `+` dropped; leading zeros stripped.
    try t.expectEqualStrings("0.5", try canon(&buf, ".5"));
    try t.expectEqualStrings("5.0", try canon(&buf, "5."));
    try t.expectEqualStrings("15", try canon(&buf, "+15"));
    try t.expectEqualStrings("755", try canon(&buf, "0755"));
    try t.expectEqualStrings("0", try canon(&buf, "0"));
    try t.expectEqualStrings("0", try canon(&buf, "000"));
    try t.expectEqualStrings("0.5", try canon(&buf, "00.5"));

    // Significant figures and precision survive — never re-formatted.
    try t.expectEqualStrings("1.10", try canon(&buf, "1.10"));
    try t.expectEqualStrings("1e2", try canon(&buf, "1e2"));
    try t.expectEqualStrings("10.5", try canon(&buf, "1_0.5"));
    try t.expectEqualStrings(
        "123456789012345678901234567890",
        try canon(&buf, "123456789012345678901234567890"),
    );

    // Past 128 bits, and the edges of the radix path.
    try t.expectEqualStrings("340282366920938463463374607431768211456", try canon(&buf, "0x1_0000_0000_0000_0000_0000_0000_0000_0000"));
    try t.expectEqualStrings("0", try canon(&buf, "0x0"));
    try t.expectEqualStrings("0", try canon(&buf, "0b000"));
    try t.expectEqualStrings("0x", try canon(&buf, "0x"));
    try t.expectEqualStrings("0o89", try canon(&buf, "0o89"));
    try t.expectEqualStrings("0xg", try canon(&buf, "0xg"));
}

test "eqlValue: an integer is its value, across bases, signs and separators" {
    const t = std.testing;
    const int = struct {
        fn f(a: []const u8, b: []const u8) bool {
            return eqlValue(.{ .raw = a, .float = false }, .{ .raw = b, .float = false });
        }
    }.f;
    try t.expect(int("0x1F", "31"));
    try t.expect(int("0X1f", "31"));
    try t.expect(int("0o37", "0x1f"));
    try t.expect(int("0b11111", "+31"));
    try t.expect(int("-0x1F", "-31"));
    try t.expect(int("1_000_000", "1000000"));
    try t.expect(int("0xdead_beef", "3735928559"));
    try t.expect(int("-0", "0"));
    try t.expect(int("+0", "0x0"));
    // Past 64 and 128 bits, exact to the last digit.
    try t.expect(int("12345678901234567890123", "12345678901234567890123"));
    try t.expect(!int("12345678901234567890123", "12345678901234567890124"));
    try t.expect(int("0x1_0000_0000_0000_0000", "18446744073709551616"));
    try t.expect(!int("18446744073709551617", "18446744073709551616"));
    try t.expect(!int("0x1F", "32"));
    try t.expect(!int("-31", "31"));
    try t.expect(!int("1:30", "90"));
}

test "eqlValue: a leading-zero integer by the dialect that read it" {
    const t = std.testing;
    const lz = struct {
        fn f(a: []const u8, zero: LeadingZero, b: []const u8) bool {
            return eqlValue(.{ .raw = a, .float = false, .leading_zero = zero }, .{ .raw = b, .float = false });
        }
    }.f;
    try t.expect(lz("0755", .decimal, "755"));
    try t.expect(!lz("0755", .decimal, "493"));
    try t.expect(lz("0755", .octal, "493"));
    try t.expect(lz("-0_755", .octal, "-0o755"));
    try t.expect(!lz("0755", .octal, "755"));
    // Not known: only the same bytes agree.
    try t.expect(lz("0755", .unknown, "0755"));
    try t.expect(!lz("0755", .unknown, "755"));
    try t.expect(!lz("0755", .unknown, "493"));
    // `08` is not octal, so it is not read as one.
    try t.expect(!lz("08", .octal, "8"));
}

test "eqlValue: a float is its exact decimal value" {
    const t = std.testing;
    const flt = struct {
        fn f(a: []const u8, b: []const u8) bool {
            return eqlValue(.{ .raw = a, .float = true }, .{ .raw = b, .float = true });
        }
    }.f;
    try t.expect(flt("1.50", "1.5"));
    try t.expect(flt("1e3", "1000.0"));
    try t.expect(flt("1E+3", "1e3"));
    try t.expect(flt("1.5E+300", "15e299"));
    try t.expect(flt(".5", "0.5"));
    try t.expect(flt("5.", "5.0"));
    try t.expect(flt("+1.5", "1.5"));
    try t.expect(flt("1_000.5", "1000.5"));
    try t.expect(flt("1e1_0", "1e10"));
    try t.expect(flt("0.0", "0e5"));
    try t.expect(flt("100000000000000000000.0", "1e20"));
    try t.expect(!flt("1.5", "1.25"));
    try t.expect(!flt("-1.5", "1.5"));
    // One double, two values: the digits the source wrote are the value.
    try t.expect(!flt("0.1", "0.1000000000000000055511151231257827"));
    try t.expect(!flt("9007199254740993.0", "9007199254740992.0"));
    // A signed zero is a different float.
    try t.expect(!flt("-0.0", "0.0"));
    try t.expect(flt("-0.0", "-0e0"));
    try t.expect(flt("+0.0", "0.0"));
}

test "eqlValue: non-finite floats in every spelling, NaN included" {
    const t = std.testing;
    try t.expect(eqlValue(.{ .raw = ".inf", .float = true }, .{ .raw = "Infinity", .float = true }));
    try t.expect(eqlValue(.{ .raw = "inf", .float = true }, .{ .raw = "+.Inf", .float = true }));
    try t.expect(eqlValue(.{ .raw = "-inf", .float = true }, .{ .raw = "-Infinity", .float = true }));
    try t.expect(eqlValue(.{ .raw = "nan", .float = true }, .{ .raw = "NaN", .float = true }));
    try t.expect(eqlValue(.{ .raw = ".NaN", .float = true }, .{ .raw = "-nan", .float = true }));
    try t.expect(!eqlValue(.{ .raw = "inf", .float = true }, .{ .raw = "-inf", .float = true }));
    try t.expect(!eqlValue(.{ .raw = "inf", .float = true }, .{ .raw = "nan", .float = true }));
    try t.expect(!eqlValue(.{ .raw = "inf", .float = true }, .{ .raw = "1e999999", .float = true }));
}

test "eqlValue: an integer never equals a float" {
    const t = std.testing;
    try t.expect(!eqlValue(.{ .raw = "1", .float = false }, .{ .raw = "1.0", .float = true }));
    try t.expect(!eqlValue(.{ .raw = "1000", .float = false }, .{ .raw = "1e3", .float = true }));
    try t.expect(!eqlValue(.{ .raw = "0", .float = false }, .{ .raw = "0.0", .float = true }));
    try t.expect(!eqlValue(.{ .raw = "1", .float = true }, .{ .raw = "1", .float = false }));
}
