//! The kit's rules as C functions, for whatever cannot call Zig: the
//! JavaScript kit, through `fig-kit.wasm` (`js.zig`), and a host linking
//! `libfig`, which declares them in `fig.h` as `fig_kit_*`. One set of
//! functions under one set of names, so a runner in any language spells a
//! number, reads a datetime, sniffs a bare literal and quotes a JSON string
//! as fig's own languages do. `kit/testdata/rules/` holds the answers every
//! binding gives through them.
//!
//! The kit defines them and does not export them: every language's library
//! compiles the kit in, and two exported copies would clash where they are
//! linked together. `js.zig` and fig's `c_api.zig` export them.
//!
//! The flag bits are `fig.h`'s `FIG_SPELL_*` and `FIG_DATETIME_*`, and
//! `kit/zig.js` in the TypeScript binding spells them again: a change here
//! is a change in both.

const std = @import("std");
const number = @import("number.zig");
const datetime = @import("datetime.zig");
const literal = @import("literal.zig");
const json_string = @import("json_string.zig");
const table = @import("table.zig");

/// How much longer than its input `numberCanonical`'s answer can be: a
/// canonical number is its input less what it drops, plus a padded `0`
/// before a bare dot and after one, or a radix integer's decimal, which
/// `number` bounds at 512 digits. `FIG_KIT_ANSWER_ROOM`.
pub const answer_room = 520;

/// Export every function under its C name, from the one artifact that
/// should carry them (`js.zig`; `c_api.zig` writes its own `export fn`s,
/// which fig's `abi-check` reads).
pub fn exportAll() void {
    @export(&numberSpellable, .{ .name = "fig_kit_number_spellable" });
    @export(&numberCanonical, .{ .name = "fig_kit_number_canonical" });
    @export(&numberNonFinite, .{ .name = "fig_kit_number_non_finite" });
    @export(&datetimeClassify, .{ .name = "fig_kit_datetime_classify" });
    @export(&datetimeValidDate, .{ .name = "fig_kit_datetime_valid_date" });
    @export(&datetimeValidTime, .{ .name = "fig_kit_datetime_valid_time" });
    @export(&datetimeValidOffset, .{ .name = "fig_kit_datetime_valid_offset" });
    @export(&literalKind, .{ .name = "fig_kit_literal_kind" });
    @export(&literalNumber, .{ .name = "fig_kit_literal_number" });
    @export(&jsonStringQuote, .{ .name = "fig_kit_json_string_quote" });
}

/// A `number.Spelling` as `FIG_SPELL_*` bits: hex, octal, binary,
/// underscores, a leading zero, a bare dot, a leading plus, from the lowest.
pub fn spelling(bits: u32) number.Spelling {
    return .{
        .hex = bits & 1 != 0,
        .octal = bits & 2 != 0,
        .binary = bits & 4 != 0,
        .underscores = bits & 8 != 0,
        .leading_zero = bits & 16 != 0,
        .bare_dot = bits & 32 != 0,
        .plus = bits & 64 != 0,
    };
}

/// Whether a format that spells `bits` reads `raw` back as the same number.
pub fn numberSpellable(raw: [*]const u8, len: usize, bits: u32) callconv(.c) bool {
    return number.spellable(raw[0..len], spelling(bits));
}

/// Writes `raw`'s canonical decimal spelling to `out` and returns its
/// length, or `maxInt(usize)` when `out_cap` is too small to hold it; it
/// never is at `len + answer_room`.
pub fn numberCanonical(raw: [*]const u8, len: usize, out: [*]u8, out_cap: usize) callconv(.c) usize {
    var w: std.Io.Writer = .fixed(out[0..out_cap]);
    number.writeCanonical(&w, raw[0..len]) catch return std.math.maxInt(usize);
    return w.end;
}

/// Which non-finite float `raw` spells, in any format's words: 0 infinity,
/// 1 negative infinity, 2 NaN, or -1 for none.
pub fn numberNonFinite(raw: [*]const u8, len: usize) callconv(.c) i32 {
    const v = number.nonFinite(raw[0..len]) orelse return -1;
    return @intFromEnum(v);
}

/// `datetime.Options` as `FIG_DATETIME_*` bits: minute precision, then a
/// time with no date.
pub fn options(bits: u32) datetime.Options {
    return .{
        .allow_minute_precision = bits & 1 != 0,
        .allow_time_only = bits & 2 != 0,
    };
}

/// `raw`'s `datetime.Kind` as its index, which is its `FIG_EXT_*`, or -1
/// when it is not a datetime.
pub fn datetimeClassify(raw: [*]const u8, len: usize, bits: u32) callconv(.c) i32 {
    const kind = datetime.classify(raw[0..len], options(bits)) catch return -1;
    return @intFromEnum(kind);
}

/// Whether `s` is `YYYY-MM-DD` and a day that exists.
pub fn datetimeValidDate(s: [*]const u8, len: usize) callconv(.c) bool {
    datetime.validateDate(s[0..len]) catch return false;
    return true;
}

/// Whether `s` is `HH:MM[:SS[.frac]]` as `bits` allow.
pub fn datetimeValidTime(s: [*]const u8, len: usize, bits: u32) callconv(.c) bool {
    datetime.validateTime(s[0..len], options(bits)) catch return false;
    return true;
}

/// Whether `s` is `±HH:MM`, an offset within a day.
pub fn datetimeValidOffset(s: [*]const u8, len: usize) callconv(.c) bool {
    datetime.validateOffset(s[0..len]) catch return false;
    return true;
}

/// The row kind a bare token is written as (`literal.sniffBare`): a
/// `table.Kind` — null, false, true, int, float or string — or
/// `table.Kind.ext` plus a datetime's `ExtKind`.
pub fn literalKind(raw: [*]const u8, len: usize) callconv(.c) u8 {
    const K = table.Kind;
    return switch (literal.sniffBare(raw[0..len])) {
        .null_ => K.null_,
        .boolean => |b| if (b) K.true_ else K.false_,
        .number => |n| if (n.kind == .float) K.float else K.int,
        .datetime => |d| K.ext + @as(u8, @intFromEnum(d.kind)),
        .string => K.string,
    };
}

/// The row kind of the number `raw` is (`literal.sniffNumber`): int or
/// float, or -1 when it is not one.
pub fn literalNumber(raw: [*]const u8, len: usize) callconv(.c) i32 {
    const n = literal.sniffNumber(raw[0..len]) orelse return -1;
    return if (n.kind == .float) table.Kind.float else table.Kind.int;
}

/// How long `jsonStringQuote`'s answer can be: every byte a six-byte
/// `\u00XX`, and the two quotes. `FIG_KIT_JSON_STRING_ROOM`.
pub fn jsonStringRoom(len: usize) usize {
    return len * 6 + 2;
}

/// Writes `raw` between `quote`s (`"`, or JSON5's `'`) with the JSON
/// escapes (`json_string.writeQuotedIn`) to `out` and returns its length,
/// or `maxInt(usize)` when `out_cap` is too small or `quote` is neither;
/// `jsonStringRoom(len)` never is too small.
pub fn jsonStringQuote(raw: [*]const u8, len: usize, quote: u8, out: [*]u8, out_cap: usize) callconv(.c) usize {
    var w: std.Io.Writer = .fixed(out[0..out_cap]);
    const r = switch (quote) {
        '"' => json_string.writeQuotedIn(&w, raw[0..len], '"'),
        '\'' => json_string.writeQuotedIn(&w, raw[0..len], '\''),
        else => return std.math.maxInt(usize),
    };
    r catch return std.math.maxInt(usize);
    return w.end;
}

comptime {
    // `FIG_EXT_OFFSET_DATETIME` .. `FIG_EXT_LOCAL_TIME` are 0..3.
    std.debug.assert(@intFromEnum(datetime.Kind.offset_datetime) == 0);
    std.debug.assert(@intFromEnum(datetime.Kind.local_time) == 3);
    std.debug.assert(@intFromEnum(number.NonFinite.nan) == 2);
}

test "canonical reports a buffer too small, and fits in answer_room" {
    const t = std.testing;
    var small: [3]u8 = undefined;
    try t.expectEqual(std.math.maxInt(usize), numberCanonical("0xffff", 6, &small, small.len));
    const raw = "0x" ++ @as([420]u8, @splat('f'));
    var buf: [raw.len + answer_room]u8 = undefined;
    const n = numberCanonical(raw, raw.len, &buf, buf.len);
    try t.expect(n != std.math.maxInt(usize) and n > 500);
}
