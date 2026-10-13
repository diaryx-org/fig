//! The YAML core schema's reading of a plain scalar and a tag: which
//! scalars are numbers under 1.2.2 and under 1.1, and which tags name a
//! core type. Pure functions of a lexeme. The YAML language resolves its
//! tags with them, and core's materializer judges a runtime YAML's tree by
//! the same rules, so they live here rather than in either.

const std = @import("std");
const AST = @import("ast/ast.zig");
const ascii = @import("ascii.zig");
const eqlAny = @import("kit.zig").eqlAny;

/// Every char of `s` is a `pred` digit or a `_` separator, with at least one
/// real digit (so `0x` / `0b` with an empty body is rejected).
fn digitRun(s: []const u8, comptime pred: fn (u8) bool) bool {
    var any = false;
    for (s) |c| {
        if (c == '_') continue;
        if (!pred(c)) return false;
        any = true;
    }
    return any;
}

/// If `tag` names a YAML core-schema type, returns the bare name (`str`, `int`,
/// `bool`, `null`, `seq`, `map`, …). Accepts the `!!name` shorthand and the
/// verbose `tag:yaml.org,2002:name` form (with or without `!<>`).
pub fn coreTagName(directives: []const AST.TagDirective, tag: []const u8) ?[]const u8 {
    const core = "tag:yaml.org,2002:";
    if (tag.len >= 3 and tag[0] == '!' and tag[1] == '<' and tag[tag.len - 1] == '>') {
        const inner = tag[2 .. tag.len - 1];
        if (std.mem.startsWith(u8, inner, core)) return inner[core.len..];
        return null;
    }
    if (std.mem.startsWith(u8, tag, core)) return tag[core.len..];
    if (tag.len == 0 or tag[0] != '!') return null;
    // A shorthand `handle` + `suffix`: `!!int`, `!e!int`, or the primary
    // `!int`. A `%TAG` directive in the document can bind any handle,
    // `!!` and `!` included, to a prefix of its own; unbound, `!!` is the
    // core schema's and `!` a local tag's.
    const close = if (tag.len > 1) std.mem.indexOfScalarPos(u8, tag, 1, '!') else null;
    const handle = if (close) |c| tag[0 .. c + 1] else "!";
    const suffix = tag[handle.len..];
    const prefix = for (directives) |d| {
        if (std.mem.eql(u8, d.handle, handle)) break d.prefix;
    } else if (std.mem.eql(u8, handle, "!!")) core else return null;
    // `prefix ++ suffix` names a core type when it is `core ++ name`. A
    // prefix that runs past `core` into the name is not a spelling anyone
    // writes, and is read as custom.
    if (prefix.len > core.len or !std.mem.startsWith(u8, core, prefix)) return null;
    const rest = core[prefix.len..];
    if (!std.mem.startsWith(u8, suffix, rest)) return null;
    const name = suffix[rest.len..];
    return if (name.len == 0) null else name;
}

/// Whether `t` spells an integer by radix (`0x1A`, `0o17`, `0b101`), which a
/// `!!float` does not take.
pub fn hasRadixPrefix(t: []const u8) bool {
    const s = if (t.len > 0 and (t[0] == '+' or t[0] == '-')) t[1..] else t;
    return s.len > 2 and s[0] == '0' and std.mem.indexOfScalar(u8, "xXoObB", s[1]) != null;
}

/// Classify a plain scalar against the YAML 1.1 int/float tags. Order matters:
/// `.inf`/`.nan` and sexagesimal are checked before the radix prefixes, and the
/// radix prefixes before the decimal/octal/float fork.
pub fn classify1_1Number(source: []const u8) NumberClass {
    if (source.len == 0) return .not_number;
    if (isInfNan(source)) return .float;

    var s = source;
    if (s[0] == '+' or s[0] == '-') s = s[1..];
    if (s.len == 0) return .not_number;

    // Sexagesimal (`H:MM:SS[.frac]`) — int or float depending on a trailing `.`.
    if (std.mem.indexOfScalar(u8, s, ':') != null) return classify1_1Base60(s);

    // Radix-prefixed integers: hex `0x…`, binary `0b…`.
    if (s.len >= 2 and s[0] == '0' and (s[1] == 'x' or s[1] == 'X'))
        return if (digitRun(s[2..], ascii.isHex)) .integer else .not_number;
    if (s.len >= 2 and s[0] == '0' and (s[1] == 'b' or s[1] == 'B'))
        return if (digitRun(s[2..], ascii.isBinary)) .integer else .not_number;

    // A `.` makes it a base-10 float (1.1 requires the dot; `1e3` is a string).
    if (std.mem.indexOfScalar(u8, s, '.') != null)
        return if (is1_1Base10Float(s)) .float else .not_number;

    // Leading-zero octal `0[0-7_]+` (1.1 has no `0o`; `08` is therefore a string).
    if (s.len >= 2 and s[0] == '0')
        return if (digitRun(s[1..], ascii.isOctal)) .integer else .not_number;

    // Plain decimal `0 | [1-9][0-9_]*`.
    if (s.len == 1 and s[0] == '0') return .integer;
    if (s[0] >= '1' and s[0] <= '9') {
        for (s[1..]) |c| if (!(ascii.isDigit(c) or c == '_')) return .not_number;
        return .integer;
    }
    return .not_number;
}

/// 1.1 base-10 float: `([0-9][0-9_]*)?\.[0-9_]*([eE][-+][0-9]+)?` (sign already
/// stripped). The dot is mandatory; an exponent, if present, must carry a sign.
fn is1_1Base10Float(s: []const u8) bool {
    var i: usize = 0;
    // Optional integer part.
    if (i < s.len and ascii.isDigit(s[i])) {
        i += 1;
        while (i < s.len and (ascii.isDigit(s[i]) or s[i] == '_')) i += 1;
    }
    // Mandatory `.`.
    if (i >= s.len or s[i] != '.') return false;
    i += 1;
    // Fractional digits (may be empty: `5.`).
    while (i < s.len and (ascii.isDigit(s[i]) or s[i] == '_')) i += 1;
    // Optional signed exponent.
    if (i < s.len and (s[i] == 'e' or s[i] == 'E')) {
        i += 1;
        if (i >= s.len or (s[i] != '+' and s[i] != '-')) return false;
        i += 1;
        var exp_digits: usize = 0;
        while (i < s.len and ascii.isDigit(s[i])) : (i += 1) exp_digits += 1;
        if (exp_digits == 0) return false;
    }
    return i == s.len;
}

/// Sexagesimal: `[1-9][0-9_]*(:[0-5]?[0-9])+` (int) or the same with a trailing
/// `.[0-9_]*` fractional (float). `s` is sign-stripped and known to contain `:`.
fn classify1_1Base60(s: []const u8) NumberClass {
    var body = s;
    var is_float = false;
    if (std.mem.indexOfScalar(u8, s, '.')) |dot| {
        is_float = true;
        for (s[dot + 1 ..]) |c| if (!(ascii.isDigit(c) or c == '_')) return .not_number;
        body = s[0..dot];
    }

    var groups = std.mem.splitScalar(u8, body, ':');
    var idx: usize = 0;
    var trailing: usize = 0;
    while (groups.next()) |g| : (idx += 1) {
        if (g.len == 0) return .not_number;
        if (idx == 0) {
            // First group `[1-9][0-9_]*` (int) / `[0-9][0-9_]*` (float).
            if (!ascii.isDigit(g[0])) return .not_number;
            if (!is_float and g[0] == '0') return .not_number;
            for (g[1..]) |c| if (!(ascii.isDigit(c) or c == '_')) return .not_number;
        } else {
            // Subsequent groups `[0-5]?[0-9]`.
            if (g.len < 1 or g.len > 2) return .not_number;
            for (g) |c| if (!ascii.isDigit(c)) return .not_number;
            if (g.len == 2 and g[0] > '5') return .not_number;
            trailing += 1;
        }
    }
    if (trailing == 0) return .not_number;
    return if (is_float) .float else .integer;
}

pub const NumberClass = enum { not_number, integer, float };

// The YAML tokenizer doesn't distinguish numbers from other plain scalars, so
// we classify them here against the YAML 1.2.2 core schema: decimal ints with
// an optional sign, hex (0x) and octal (0o) ints, floats with fractions and/or
// exponents, and the special floats .inf / .nan.
pub fn classifyNumber(source: []const u8) NumberClass {
    if (source.len == 0) return .not_number;
    if (isInfNan(source)) return .float;

    // Hex and octal integers take no sign in the core schema.
    if (source.len > 2 and source[0] == '0' and (source[1] == 'x' or source[1] == 'o')) {
        const hex = source[1] == 'x';
        var i: usize = 2;
        while (i < source.len) : (i += 1) {
            const ok = if (hex) ascii.isHex(source[i]) else ascii.isOctal(source[i]);
            if (!ok) return .not_number;
        }
        return .integer;
    }

    var i: usize = 0;
    if (source[i] == '+' or source[i] == '-') i += 1;

    var mantissa_digits: usize = 0;
    while (i < source.len and std.ascii.isDigit(source[i])) : (i += 1) mantissa_digits += 1;

    var is_float = false;
    if (i < source.len and source[i] == '.') {
        is_float = true;
        i += 1;
        while (i < source.len and std.ascii.isDigit(source[i])) : (i += 1) mantissa_digits += 1;
    }
    if (mantissa_digits == 0) return .not_number;

    if (i < source.len and (source[i] == 'e' or source[i] == 'E')) {
        is_float = true;
        i += 1;
        if (i < source.len and (source[i] == '+' or source[i] == '-')) i += 1;
        var exp_digits: usize = 0;
        while (i < source.len and std.ascii.isDigit(source[i])) : (i += 1) exp_digits += 1;
        if (exp_digits == 0) return .not_number;
    }

    if (i != source.len) return .not_number;
    return if (is_float) .float else .integer;
}

fn isInfNan(source: []const u8) bool {
    var body = source;
    if (source[0] == '+' or source[0] == '-') body = source[1..];
    if (eqlAny(body, &.{ ".inf", ".Inf", ".INF" })) return true;
    return eqlAny(source, &.{ ".nan", ".NaN", ".NAN" });
}
