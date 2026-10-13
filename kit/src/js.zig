//! The kit's rules as a WebAssembly module of their own, `fig-kit.wasm`,
//! for the JavaScript kit: `@diaryx/fig/kit/number` and `/datetime` answer
//! through these exports, so a language written in JavaScript spells a
//! number and reads a datetime by the same code a Zig language does.
//!
//! The module imports nothing, allocates nothing and keeps no state. The
//! caller writes its input from `fig_kit_heap()` on, growing the memory to
//! hold it, and passes where and how long it is; a call that answers with
//! text writes it where the caller says, which has room for `answer_room`
//! more bytes than the input. `kit/zig.js` is the one caller, and the flag
//! bits below are spelled again there: a change here is a change there.

const std = @import("std");
const number = @import("number.zig");
const datetime = @import("datetime.zig");

extern const __heap_base: u8;

comptime {
    @export(&heap, .{ .name = "fig_kit_heap" });
    @export(&numberSpellable, .{ .name = "fig_kit_number_spellable" });
    @export(&numberCanonical, .{ .name = "fig_kit_number_canonical" });
    @export(&numberNonFinite, .{ .name = "fig_kit_number_non_finite" });
    @export(&datetimeClassify, .{ .name = "fig_kit_datetime_classify" });
    @export(&datetimeValidDate, .{ .name = "fig_kit_datetime_valid_date" });
    @export(&datetimeValidTime, .{ .name = "fig_kit_datetime_valid_time" });
    @export(&datetimeValidOffset, .{ .name = "fig_kit_datetime_valid_offset" });
}

/// How much longer than its input an answer can be: a canonical number is
/// its input less what it drops, plus a padded `0` before a bare dot and
/// after one, or a radix integer's decimal, which `number` bounds at 512
/// digits.
pub const answer_room = 520;

/// The first byte no static data holds, where the caller's input goes.
fn heap() callconv(.c) usize {
    return @intFromPtr(&__heap_base);
}

/// A `number.Spelling` as bits: hex, octal, binary, underscores, a
/// leading zero, a bare dot, a leading plus, from the lowest.
fn spelling(bits: u32) number.Spelling {
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

fn numberSpellable(ptr: [*]const u8, len: usize, bits: u32) callconv(.c) bool {
    return number.spellable(ptr[0..len], spelling(bits));
}

/// Writes the canonical decimal spelling at `out` and returns its length.
fn numberCanonical(ptr: [*]const u8, len: usize, out: [*]u8) callconv(.c) usize {
    var w: std.Io.Writer = .fixed(out[0 .. len + answer_room]);
    number.writeCanonical(&w, ptr[0..len]) catch unreachable;
    return w.end;
}

/// Which non-finite float the input spells: 0 infinity, 1 negative
/// infinity, 2 NaN, or -1 for none.
fn numberNonFinite(ptr: [*]const u8, len: usize) callconv(.c) i32 {
    const v = number.nonFinite(ptr[0..len]) orelse return -1;
    return @intFromEnum(v);
}

/// `datetime.Options` as bits: minute precision, then a time with no date.
fn options(bits: u32) datetime.Options {
    return .{
        .allow_minute_precision = bits & 1 != 0,
        .allow_time_only = bits & 2 != 0,
    };
}

/// The input's `datetime.Kind` as its index, or -1 when it is not a
/// datetime.
fn datetimeClassify(ptr: [*]const u8, len: usize, bits: u32) callconv(.c) i32 {
    const kind = datetime.classify(ptr[0..len], options(bits)) catch return -1;
    return @intFromEnum(kind);
}

fn datetimeValidDate(ptr: [*]const u8, len: usize) callconv(.c) bool {
    datetime.validateDate(ptr[0..len]) catch return false;
    return true;
}

fn datetimeValidTime(ptr: [*]const u8, len: usize, bits: u32) callconv(.c) bool {
    datetime.validateTime(ptr[0..len], options(bits)) catch return false;
    return true;
}

fn datetimeValidOffset(ptr: [*]const u8, len: usize) callconv(.c) bool {
    datetime.validateOffset(ptr[0..len]) catch return false;
    return true;
}
