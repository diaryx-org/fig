//! The kit's rules as C functions, for whatever cannot call Zig: the
//! JavaScript kit, through `fig-kit.wasm` (`js.zig`), and a host linking
//! `libfig`, which declares them in `fig.h` as `fig_kit_*`. One set of
//! functions under one set of names, so a runner in any language spells a
//! number and reads a datetime as fig's own languages do.
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
