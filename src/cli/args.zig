//! Command-line parsing: turns the raw arg iterator into a `CliConfig`
//! (`parseConfig`), plus the small format/path resolvers that
//! parsing (and later, the action handlers) share.
const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");

const gron = @import("gron.zig");
const languages = @import("languages.zig");
const types = @import("types.zig");
const fileio = @import("fileio.zig");

const Format = types.Format;
const CliAction = types.CliAction;
const CliConfig = types.CliConfig;
const ArgError = types.ArgError;
const Detected = types.Detected;
const append_index = types.append_index;
const Io = std.Io;

/// Map a `--input`/`-i` format name to a `Format`. The enum member names cover
/// every accepted token (including `canonical` and `figl`) directly, plus the
/// one alias below. Returns null for an unknown name so callers can emit a
/// tailored error.
pub fn parseFormatName(name: []const u8) ?Format {
    // `yml` is an accepted SPELLING of yaml, not a format of its own, and
    // collapses here, so downstream code only ever sees `yaml`.
    if (std.mem.eql(u8, name, "yml")) return parseFormatName("yaml");
    if (types.named(name)) |f| return f;
    // A language configured in `languages.figl`, spawned and registered on
    // this first ask.
    return languages.resolveName(name);
}

/// JSON: the format a command falls back to where none is named and none
/// is detected, as `convert`'s output.
fn jsonFormat() Format {
    return parseFormatName("json") orelse .canonical;
}

/// Parse a command-line path — `a.b[2].c` — into segments. A `.` separates
/// keys and a `[n]` is an index; `[-]`/`[$]` is the end sentinel.
///
/// A key that holds a `.` or a `[` is written quoted or escaped:
///
///   * `a."b.c"` — double quotes, with JSON's escapes inside (`\"`, `\\`,
///     `\n`, `\u00e9`, …);
///   * `a.'b.c'` — single quotes, verbatim, as a TOML literal key;
///   * `a["b.c"]` / `a['b.c']` — the same quoting in brackets, which is how
///     `-o gron` prints such a key, so a gron line reads back;
///   * `a.b\.c` — a backslash in a bare key takes the next byte as it is.
///
/// Key bytes are allocated from `allocator` where quoting or escaping
/// changed them, and otherwise borrowed from `path`.
pub fn parsePath(allocator: std.mem.Allocator, path: []const u8) ![]fig.AST.PathSegment {
    const log = std.log.scoped(.parsePath);
    var path_in_progress: std.ArrayList(fig.AST.PathSegment) = .empty;
    var i: usize = 0;
    while (i < path.len) {
        switch (path[i]) {
            '.' => {
                // Dot is a separator. Else branch parses the key.
                i += 1;
            },
            '[' => {
                // A quoted key in brackets — gron's spelling of a key that
                // is not a bare identifier.
                if (i + 1 < path.len and (path[i + 1] == '"' or path[i + 1] == '\'')) {
                    i += 1;
                    const key = try quotedKey(allocator, path, &i);
                    if (i >= path.len or path[i] != ']') return ArgError.InvalidPath;
                    i += 1;
                    log.debug("key: {s}", .{key});
                    try path_in_progress.append(allocator, .{ .key = key });
                    continue;
                }
                // Skip open bracket
                const start = i + 1;
                i = start;
                // Loop until end or close bracket
                while (i < path.len and path[i] != ']') : (i += 1) {}
                if (i >= path.len or i == start) return error.InvalidPath;

                // `[-]` and `[$]` are the "end" tokens: `insert` reads the
                // sentinel as "append", and `delete` reads it as "the last
                // item" (`editor.removeSeqItem` special-cases it). Any other
                // caller that walks the path literally (e.g. `get`) just
                // sees an out-of-range index and surfaces NotFound.
                const inner = path[start..i];
                log.debug("number: {s}", .{inner});
                const seg: fig.AST.PathSegment = if (std.mem.eql(u8, inner, "-") or std.mem.eql(u8, inner, "$"))
                    .{ .index = append_index }
                else
                    .{ .index = try std.fmt.parseInt(usize, inner, 10) };
                try path_in_progress.append(allocator, seg);
                // Skip close bracket
                i += 1;
            },
            '"', '\'' => {
                const key = try quotedKey(allocator, path, &i);
                // A quoted key ends its segment.
                if (i < path.len and path[i] != '.' and path[i] != '[') return ArgError.InvalidPath;
                log.debug("key: {s}", .{key});
                try path_in_progress.append(allocator, .{ .key = key });
            },
            else => {
                const start = i;
                var escaped = false;
                // Loop until a dot or open bracket; a backslash carries the
                // byte after it past both.
                while (i < path.len and path[i] != '.' and path[i] != '[') : (i += 1) {
                    if (path[i] == '\\') {
                        if (i + 1 >= path.len) return ArgError.InvalidPath;
                        escaped = true;
                        i += 1;
                    }
                }
                if (i == start) return ArgError.InvalidPath;
                const key = if (!escaped) path[start..i] else blk: {
                    var out: std.ArrayList(u8) = .empty;
                    var j = start;
                    while (j < i) : (j += 1) {
                        if (path[j] == '\\') j += 1;
                        try out.append(allocator, path[j]);
                    }
                    break :blk try out.toOwnedSlice(allocator);
                };

                log.debug("key: {s}", .{key});
                try path_in_progress.append(allocator, .{ .key = key });
            },
        }
    }
    return path_in_progress.toOwnedSlice(allocator);
}

/// `parsePath` for a path on the command line: one that does not parse is a
/// usage error (exit 2) naming it, rather than an error name escaping `main`.
fn pathArg(allocator: std.mem.Allocator, path: []const u8) ArgError![]fig.AST.PathSegment {
    return parsePath(allocator, path) catch |err| switch (err) {
        error.OutOfMemory => return error.OutOfMemory,
        else => {
            std.log.scoped(.parseConfig).err("`{s}` is not a path: keys are separated by `.`, an index is `[n]` (or `[-]` for the end), and a key holding `.` or `[` is quoted: a.\"b.c\"\n", .{path});
            return ArgError.InvalidPath;
        },
    };
}

/// The key quoted at `path[i.*]` — `'…'` verbatim, `"…"` with JSON's
/// escapes decoded — leaving `i.*` just past the closing quote.
fn quotedKey(allocator: std.mem.Allocator, path: []const u8, i: *usize) ![]const u8 {
    const quote = path[i.*];
    const start = i.* + 1;
    var j = start;
    if (quote == '\'') {
        while (j < path.len and path[j] != '\'') : (j += 1) {}
        if (j >= path.len) return ArgError.InvalidPath;
        i.* = j + 1;
        return path[start..j];
    }
    var out: std.ArrayList(u8) = .empty;
    while (true) : (j += 1) {
        if (j >= path.len) return ArgError.InvalidPath;
        const c = path[j];
        if (c == '"') break;
        if (c != '\\') {
            try out.append(allocator, c);
            continue;
        }
        j += 1;
        if (j >= path.len) return ArgError.InvalidPath;
        switch (path[j]) {
            '"', '\\', '/' => |e| try out.append(allocator, e),
            'b' => try out.append(allocator, 0x08),
            'f' => try out.append(allocator, 0x0c),
            'n' => try out.append(allocator, '\n'),
            'r' => try out.append(allocator, '\r'),
            't' => try out.append(allocator, '\t'),
            'u' => {
                var cp: u21 = try hex4(path, j + 1);
                j += 4;
                // A high surrogate takes the `\uXXXX` low half after it.
                if (cp >= 0xD800 and cp <= 0xDBFF) {
                    if (j + 2 >= path.len or path[j + 1] != '\\' or path[j + 2] != 'u') return ArgError.InvalidPath;
                    const lo = try hex4(path, j + 3);
                    if (lo < 0xDC00 or lo > 0xDFFF) return ArgError.InvalidPath;
                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                    j += 6;
                } else if (cp >= 0xDC00 and cp <= 0xDFFF) return ArgError.InvalidPath;
                var buf: [4]u8 = undefined;
                const n = std.unicode.utf8Encode(cp, &buf) catch return ArgError.InvalidPath;
                try out.appendSlice(allocator, buf[0..n]);
            },
            else => return ArgError.InvalidPath,
        }
    }
    i.* = j + 1;
    return out.toOwnedSlice(allocator);
}

fn hex4(path: []const u8, at: usize) !u21 {
    if (at + 4 > path.len) return ArgError.InvalidPath;
    return std.fmt.parseInt(u21, path[at .. at + 4], 16) catch ArgError.InvalidPath;
}

/// Infer the parse strategy from a file's extension, or null when the extension
/// is missing/unrecognized — the caller then falls back to content sniffing
/// (`Language.detect`) rather than failing outright.
pub fn detectLanguageFromFileEnding(file_path: []const u8) ?Detected {
    // `--lang <name>` names the language outright, whatever the extension
    // says — it is how a file a compiled format owns by extension is read
    // through a runtime language instead (`secrets.env --lang lua-dotenv`).
    if (languages.langOverride()) |f| return .{ .format = f };

    const dot = std.mem.findLast(u8, file_path, ".");
    const ext = file_path[(dot orelse 0) + 1 .. file_path.len];

    // An extension that names a format IS that format. Tried first so every
    // name keeps its exact meaning — notably the dialects (`jsonc`, `json5`)
    // that share a language with the default spelling, and the CLI's own
    // `gron`. `yml` is not a name: YAML declares it as an extension, below.
    // A configured language of that name answers to it too.
    if (parseFormatName(ext)) |format| return .{ .format = format };

    // Otherwise ask the languages: each declares the extensions it owns —
    // `.figl`, `.env` (dotenv files are conventionally named exactly `.env`,
    // so the last-dot split gives a literal `env`), `.nt`, `.yml`. Not
    // recognized here: multi-suffix variants like `.env.production`, whose
    // last-dot extension is `production`; pass `--input dotenv` for those.
    if (languages.registeredExtension(ext)) |format| return .{ .format = format };

    // Last, the languages `languages.figl` configures: a registered
    // language's extension always wins, so a configured language is only
    // reached by an extension nothing registered owns (proposal §7.1).
    if (languages.resolveExtension(ext)) |format| return .{ .format = format };

    return null;
}

/// The argument list with `--lang <name>` taken out: the one flag every
/// action accepts, read here once rather than in each action's loop, and
/// recorded for `detectLanguageFromFileEnding` to answer with.
const Args = struct {
    items: []const []const u8,
    i: usize = 0,
    fn next(self: *Args) ?[]const u8 {
        if (self.i >= self.items.len) return null;
        defer self.i += 1;
        return self.items[self.i];
    }
};

/// An action's positional arguments, read strictly. An argument that begins
/// with `-` and is none of the action's flags is a usage error, not a file
/// name; `--` ends the flags, so everything after it is a positional (a file
/// really named `-x` is `-- -x`). `-` alone (stdin) and a negative number
/// (`fig set f.yaml n -5`) are positionals wherever they stand.
///
/// Each action's loop asks `rest` first, so `--` and what follows it never
/// reach the flag comparisons, and hands `add` whatever no flag matched.
const Positionals = struct {
    items: std.ArrayList([]const u8) = .empty,
    /// Set once `--` is seen.
    flags_done: bool = false,

    /// True when `arg` is taken here without being read as a flag: `--`
    /// itself, or anything after it.
    fn rest(self: *Positionals, allocator: std.mem.Allocator, arg: []const u8) ArgError!bool {
        if (self.flags_done) {
            try self.items.append(allocator, arg);
            return true;
        }
        if (std.mem.eql(u8, arg, "--")) {
            self.flags_done = true;
            return true;
        }
        return false;
    }

    /// Take `arg`, which matched none of `action`'s flags, as a positional.
    /// False, having said so, when it is spelled like a flag.
    fn add(self: *Positionals, allocator: std.mem.Allocator, action: []const u8, arg: []const u8) ArgError!bool {
        if (spelledAsFlag(arg)) {
            std.log.scoped(.parseConfig).err("{s} has no flag {s} (a file or value that begins with `-` goes after `--`)\n", .{ action, arg });
            return false;
        }
        try self.items.append(allocator, arg);
        return true;
    }

    /// False, having said so, when there are more than `max` positionals.
    fn atMost(self: *const Positionals, action: []const u8, max: usize, what: []const u8) bool {
        if (self.items.items.len <= max) return true;
        std.log.scoped(.parseConfig).err("{s} takes {s}; unexpected argument: {s}\n", .{ action, what, self.items.items[max] });
        return false;
    }

    fn deinit(self: *Positionals, allocator: std.mem.Allocator) void {
        self.items.deinit(allocator);
    }
};

/// Whether `flag` takes the next argument as its value in `action` — every
/// flag of every action that does, for `parseConfig`'s `--lang` pre-pass,
/// which reads the line before any action does. `--seq` takes a strategy in
/// `patch` and nothing in `set`.
fn takesValue(flag: []const u8, action: []const u8) bool {
    const valued = [_][]const u8{
        "--lang",    "--input", "-i",          "--output",   "-o",
        "--indent",  "--width", "--gron-root", "--gron-sep", "--gron-term",
        "--at",      "--from",  "--delete",    "--comments", "--patch-input",
        "--against", "--spec",  "-s",
    };
    for (valued) |v| if (std.mem.eql(u8, flag, v)) return !(std.mem.eql(u8, flag, "--delete") and !isPatch(action));
    return std.mem.eql(u8, flag, "--seq") and isPatch(action);
}

fn isPatch(action: []const u8) bool {
    return std.mem.eql(u8, action, "patch") or std.mem.eql(u8, action, "p");
}

/// Whether `arg` reads as a flag: a leading `-` with something after it
/// that does not start a number. `-inf` and `-nan` are flags here: fig has
/// no bare non-finite float (docs/spec.md § 5.3 — bare `-inf` is the string
/// it spells), so taking one as a value would quietly write a string where
/// a float was meant. `-- -inf` passes the string on purpose.
fn spelledAsFlag(arg: []const u8) bool {
    if (arg.len < 2 or arg[0] != '-') return false;
    return !(std.ascii.isDigit(arg[1]) or arg[1] == '.');
}

/// Take `arg` as `--string`/`--raw` into `mode`, refusing the pair: true when
/// it was one of them. `null` in `mode` is "not given" (the default reading).
fn valueModeFlag(action: []const u8, arg: []const u8, mode: *?types.ValueMode, usage: ArgError) ArgError!bool {
    const this: types.ValueMode = if (std.mem.eql(u8, arg, "--string"))
        .string
    else if (std.mem.eql(u8, arg, "--raw"))
        .raw
    else
        return false;
    if (mode.*) |prev| if (prev != this) {
        std.log.scoped(.parseConfig).err("{s}: --string and --raw are two readings of the value; pass one.\n", .{action});
        return usage;
    };
    mode.* = this;
    return true;
}

fn isHelp(arg: []const u8) bool {
    return std.mem.eql(u8, arg, "--help") or std.mem.eql(u8, arg, "-h");
}

pub fn parseConfig(allocator: std.mem.Allocator, args_in: anytype) ArgError!CliConfig {
    const log = std.log.scoped(.parseConfig);
    var config = CliConfig{};

    var collected: std.ArrayList([]const u8) = .empty;
    // Past `--` every argument is a positional, `--lang` included — unless
    // that `--` is the value of the flag before it (`--gron-sep --`), which
    // ends nothing.
    var flags_done = false;
    var prev: []const u8 = "";
    while (args_in.next()) |arg| {
        defer prev = arg;
        if (std.mem.eql(u8, arg, "--") and !takesValue(prev, if (collected.items.len > 1) collected.items[1] else "")) flags_done = true;
        if (!flags_done and std.mem.eql(u8, arg, "--lang")) {
            const name = args_in.next() orelse {
                log.err("Missing language name after --lang\n", .{});
                return ArgError.UnsupportedFileFormat;
            };
            const format = parseFormatName(name) orelse {
                // A refused helper has already said why, through `ensure`.
                if (!languages.isConfigured(name)) log.err("No language named `{s}`: not a compiled format, and no languages.figl configures it (see `fig lang --help`)\n", .{name});
                return ArgError.UnsupportedFileFormat;
            };
            languages.setLangOverride(format);
            continue;
        }
        collected.append(allocator, arg) catch return ArgError.OutOfMemory;
    }
    var args_storage: Args = .{ .items = collected.items };
    const args = &args_storage;

    config.binary_name = args.next() orelse "fig";

    const action_str = args.next() orelse {
        config.action = .help;
        config.options = .{ .help = .{} };
        return config;
    };

    if (std.mem.eql(u8, action_str, "help") or std.mem.eql(u8, action_str, "--help") or std.mem.eql(u8, action_str, "-h")) {
        config.action = .help;
        config.options = .{ .help = .{ .requested_help = true } };
        // Nothing follows but a repeated `--help` (`fig help --help`): strict
        // like every other action, so `fig help --bogus` is not a silent 0.
        while (args.next()) |arg| {
            if (isHelp(arg)) continue;
            if (std.mem.eql(u8, arg, "--oneline")) {
                config.options.help.oneline = true;
                continue;
            }
            // `fig help set` reads as asking for `set`'s help: say where it is.
            if (std.meta.stringToEnum(CliAction, arg)) |named| if (named != .external) {
                log.err("{s} takes no arguments; for {s}'s help, run `fig {s} --help`\n", .{ action_str, arg, arg });
                return ArgError.HelpNamesAction;
            };
            log.err("{s} takes no arguments; unexpected argument: {s}\n", .{ action_str, arg });
            return ArgError.SurplusArgument;
        }
    } else if (std.mem.eql(u8, action_str, "version") or std.mem.eql(u8, action_str, "--version") or std.mem.eql(u8, action_str, "-v")) {
        config.action = .version;
        config.options = .{ .version = .{} };
        // `--help` is the one thing that may follow, as for every action.
        while (args.next()) |arg| {
            if (isHelp(arg)) {
                config.options.version.requested_help = true;
                continue;
            }
            log.err("{s} takes no arguments; unexpected argument: {s}\n", .{ action_str, arg });
            return ArgError.SurplusArgument;
        }
    } else if (std.mem.eql(u8, action_str, "replace") or std.mem.eql(u8, action_str, "rename")) {
        // `replace` swaps the value at a path; `rename` renames the key there.
        // One operation each — no flag turns one into the other (cli-5 §7).
        const rename = std.mem.eql(u8, action_str, "rename");
        const usage = if (rename) ArgError.MissingRenameArgument else ArgError.MissingReplaceArgument;
        config.action = if (rename) .rename else .replace;

        var value_mode: ?types.ValueMode = null;
        var requested_help = false;
        var positionals: Positionals = .{};
        defer positionals.deinit(allocator);
        while (args.next()) |arg| {
            if (try positionals.rest(allocator, arg)) continue;
            if (isHelp(arg)) {
                requested_help = true;
            } else if (!rename and try valueModeFlag(action_str, arg, &value_mode, usage)) {
                // taken
            } else if (!try positionals.add(allocator, action_str, arg)) return usage;
        }
        const pos = positionals.items.items;

        var file_path: []const u8 = "";
        var path: []fig.AST.PathSegment = &.{};
        var replacement: []const u8 = "";
        if (!requested_help) {
            if (pos.len < 3) {
                log.err("{s}\n", .{switch (pos.len) {
                    0 => "No file provided.",
                    1 => "No path provided.",
                    else => if (rename) "No new key name provided." else "No value provided.",
                }});
                return usage;
            }
            if (!positionals.atMost(action_str, 3, if (rename) "a file, a path and a new name" else "a file, a path and a value")) return usage;
            file_path = pos[0];
            path = try pathArg(allocator, pos[1]);
            replacement = pos[2];
        }

        // Skip extension detection when the user only asked for help. An
        // unrecognized extension is not an error here: `detect = true`
        // defers to content sniffing in the handler.
        const ext = if (requested_help) null else detectLanguageFromFileEnding(file_path);
        const opts: types.EditOptions = .{
            .file = file_path,
            .path = path,
            .replacement = replacement,
            .key = rename,
            .value_mode = value_mode orelse .fig,
            .requested_help = requested_help,
            .format = if (ext) |d| d.format else .canonical,
            .detect = !requested_help and ext == null,
        };
        config.options = if (rename) .{ .rename = opts } else .{ .replace = opts };
    } else if (std.mem.eql(u8, action_str, "set") or std.mem.eql(u8, action_str, "s")) {
        config.action = .set;

        // Leading flags in any order: `--seq`, `--string`/`--raw`, `--help`.
        // Positionals follow: file, path, then the value (or, with `--seq`, the
        // sequence items).
        var seq = false;
        var value_mode: ?types.ValueMode = null;
        var requested_help = false;
        var positionals: Positionals = .{};
        defer positionals.deinit(allocator);

        while (args.next()) |arg| {
            if (try positionals.rest(allocator, arg)) continue;
            if (isHelp(arg)) {
                requested_help = true;
            } else if (std.mem.eql(u8, arg, "--seq")) {
                seq = true;
            } else if (try valueModeFlag("set", arg, &value_mode, ArgError.MissingSetArgument)) {
                // taken
            } else if (!try positionals.add(allocator, "set", arg)) return ArgError.MissingSetArgument;
        }
        const pos = positionals.items.items;

        if (requested_help) {
            config.options = .{ .set = .{ .file = "", .path = &.{}, .value = "", .requested_help = true, .format = jsonFormat() } };
        } else {
            // Need file, path, and at least one value (a scalar, or one or more
            // sequence items with `--seq`).
            if (pos.len < 3) {
                log.err("set needs a file, a path, and a value (e.g. `fig set f.yaml a.b 1`).\n", .{});
                return ArgError.MissingSetArgument;
            }
            if (!seq and !positionals.atMost("set", 3, "a file, a path and one value (--seq takes several)")) return ArgError.MissingSetArgument;
            const file_path = pos[0];
            const path = try pathArg(allocator, pos[1]);
            const ext = detectLanguageFromFileEnding(file_path);
            config.options = .{
                .set = .{
                    .file = file_path,
                    .path = path,
                    .value = if (seq) "" else pos[2],
                    .seq = seq,
                    .values = if (seq) try allocator.dupe([]const u8, pos[2..]) else &.{},
                    .value_mode = value_mode orelse .fig,
                    .requested_help = false,
                    .format = if (ext) |d| d.format else .canonical,
                    // Sniff the content when the extension resolved nothing.
                    .detect = ext == null,
                },
            };
        }
    } else if (std.mem.eql(u8, action_str, "insert") or std.mem.eql(u8, action_str, "i")) {
        config.action = .insert;

        var value_mode: ?types.ValueMode = null;
        var requested_help = false;
        var positionals: Positionals = .{};
        defer positionals.deinit(allocator);
        while (args.next()) |arg| {
            if (try positionals.rest(allocator, arg)) continue;
            if (isHelp(arg)) {
                requested_help = true;
            } else if (try valueModeFlag("insert", arg, &value_mode, ArgError.MissingInsertArgument)) {
                // taken
            } else if (!try positionals.add(allocator, "insert", arg)) return ArgError.MissingInsertArgument;
        }
        const pos = positionals.items.items;

        var file_path: []const u8 = "";
        var path: []fig.AST.PathSegment = &.{};
        var value: []const u8 = "";
        if (!requested_help) {
            if (pos.len < 3) {
                log.err("{s}\n", .{switch (pos.len) {
                    0 => "No file provided.",
                    1 => "No path provided.",
                    else => "No value provided.",
                }});
                return ArgError.MissingInsertArgument;
            }
            if (!positionals.atMost("insert", 3, "a file, a path and a value")) return ArgError.MissingInsertArgument;
            file_path = pos[0];
            path = try pathArg(allocator, pos[1]);
            value = pos[2];
        }

        const ext = if (requested_help) null else detectLanguageFromFileEnding(file_path);
        config.options = .{ .insert = .{
            .file = file_path,
            .path = path,
            .value = value,
            .value_mode = value_mode orelse .fig,
            .requested_help = requested_help,
            .format = if (ext) |d| d.format else .canonical,
            .detect = !requested_help and ext == null,
        } };
    } else if (std.mem.eql(u8, action_str, "delete") or std.mem.eql(u8, action_str, "d")) {
        config.action = .delete;

        var requested_help = false;
        var positionals: Positionals = .{};
        defer positionals.deinit(allocator);
        while (args.next()) |arg| {
            if (try positionals.rest(allocator, arg)) continue;
            if (isHelp(arg)) {
                requested_help = true;
            } else if (!try positionals.add(allocator, "delete", arg)) return ArgError.MissingDeleteArgument;
        }
        const pos = positionals.items.items;

        var file_path: []const u8 = "";
        var path: []fig.AST.PathSegment = &.{};
        if (!requested_help) {
            if (pos.len < 2) {
                log.err("{s}\n", .{if (pos.len == 0) "No file provided." else "No path provided."});
                return ArgError.MissingDeleteArgument;
            }
            if (!positionals.atMost("delete", 2, "a file and a path")) return ArgError.MissingDeleteArgument;
            file_path = pos[0];
            path = try pathArg(allocator, pos[1]);
        }

        const ext = if (requested_help) null else detectLanguageFromFileEnding(file_path);
        config.options = .{ .delete = .{
            .file = file_path,
            .path = path,
            .requested_help = requested_help,
            .format = if (ext) |d| d.format else .canonical,
            .detect = !requested_help and ext == null,
        } };
    } else if (std.mem.eql(u8, action_str, "comment") or std.mem.eql(u8, action_str, "c")) {
        config.action = .comment;

        // Flags, in any order: `--inline`, `--delete`, `--get`.
        var inline_comment = false;
        var delete = false;
        var get = false;
        var requested_help = false;
        var positionals: Positionals = .{};
        defer positionals.deinit(allocator);
        while (args.next()) |arg| {
            if (try positionals.rest(allocator, arg)) continue;
            if (isHelp(arg)) {
                requested_help = true;
            } else if (std.mem.eql(u8, arg, "--inline")) {
                inline_comment = true;
            } else if (std.mem.eql(u8, arg, "--delete")) {
                delete = true;
            } else if (std.mem.eql(u8, arg, "--get")) {
                get = true;
            } else if (!try positionals.add(allocator, "comment", arg)) return ArgError.MissingCommentArgument;
        }
        const pos = positionals.items.items;

        var file_path: []const u8 = "";
        var path: []fig.AST.PathSegment = &.{};
        var text: []const u8 = "";
        if (!requested_help) {
            // Delete/get need no text; add/set requires it.
            const wants_text = !delete and !get;
            if (pos.len < 2 or (wants_text and pos.len < 3)) {
                log.err("{s}\n", .{switch (pos.len) {
                    0 => "No file provided.",
                    1 => "No path provided.",
                    else => "No comment text provided.",
                }});
                return ArgError.MissingCommentArgument;
            }
            if (!positionals.atMost("comment", if (wants_text) 3 else 2, if (wants_text) "a file, a path and the comment text" else "a file and a path with --get/--delete")) return ArgError.MissingCommentArgument;
            file_path = pos[0];
            path = try pathArg(allocator, pos[1]);
            if (wants_text) text = pos[2];
        }

        const ext = if (requested_help) null else detectLanguageFromFileEnding(file_path);
        config.options = .{ .comment = .{
            .file = file_path,
            .path = path,
            .text = text,
            .inline_comment = inline_comment,
            .delete = delete,
            .get = get,
            .requested_help = requested_help,
            .format = if (ext) |d| d.format else .canonical,
            .detect = !requested_help and ext == null,
        } };
    } else if (std.mem.eql(u8, action_str, "get") or std.mem.eql(u8, action_str, "g")) {
        config.action = .get;

        var input_override: ?Format = null;
        var output_override: ?Format = null;
        var lax_tags = false;
        var lossless = false;
        var quiet = false;
        var strict = false;
        var serialize: fig.AST.SerializeOptions = .{};
        // gron projection overrides; null means "keep the gron default".
        var gron_root: ?[]const u8 = null;
        var gron_sep: ?[]const u8 = null;
        var gron_term: ?[]const u8 = null;
        var requested_help = false;
        var positionals: Positionals = .{};
        defer positionals.deinit(allocator);

        while (args.next()) |arg| {
            if (try positionals.rest(allocator, arg)) continue;
            if (isHelp(arg)) {
                requested_help = true;
            } else if (std.mem.eql(u8, arg, "--lax-tags")) {
                lax_tags = true;
            } else if (std.mem.eql(u8, arg, "--gron-root")) {
                gron_root = args.next() orelse {
                    log.err("Missing value after {s}\n", .{arg});
                    return ArgError.MissingGetArgument;
                };
            } else if (std.mem.eql(u8, arg, "--gron-sep")) {
                gron_sep = args.next() orelse {
                    log.err("Missing value after {s}\n", .{arg});
                    return ArgError.MissingGetArgument;
                };
            } else if (std.mem.eql(u8, arg, "--gron-term")) {
                // Empty is allowed (drop the terminator entirely).
                gron_term = args.next() orelse {
                    log.err("Missing value after {s}\n", .{arg});
                    return ArgError.MissingGetArgument;
                };
            } else if (std.mem.eql(u8, arg, "--lossless")) {
                lossless = true;
            } else if (std.mem.eql(u8, arg, "--lossy")) {
                lossless = false;
            } else if (std.mem.eql(u8, arg, "--compact")) {
                serialize.pretty = false;
            } else if (std.mem.eql(u8, arg, "--pretty")) {
                serialize.pretty = true;
            } else if (std.mem.eql(u8, arg, "--strip-comments")) {
                serialize.strip_comments = true;
            } else if (std.mem.eql(u8, arg, "--quiet") or std.mem.eql(u8, arg, "-q") or std.mem.eql(u8, arg, "--no-warnings")) {
                quiet = true;
            } else if (std.mem.eql(u8, arg, "--strict")) {
                strict = true;
            } else if (std.mem.eql(u8, arg, "--indent")) {
                const n = args.next() orelse {
                    log.err("Missing value after {s}\n", .{arg});
                    return ArgError.MissingGetArgument;
                };
                serialize.indent = std.fmt.parseInt(u8, n, 10) catch {
                    log.err("Invalid --indent value: {s}\n", .{n});
                    return ArgError.MissingGetArgument;
                };
                // fig has no `pretty` gate of its own to read `indent`'s value
                // against (see `SerializeOptions.indent_requested`'s doc comment), so
                // an explicit `--indent` is fig's own on/off signal, independent
                // of the numeric width other formats use it for.
                serialize.indent_requested = true;
            } else if (std.mem.eql(u8, arg, "--width")) {
                const n = args.next() orelse {
                    log.err("Missing value after {s}\n", .{arg});
                    return ArgError.MissingGetArgument;
                };
                serialize.width = std.fmt.parseInt(u16, n, 10) catch {
                    log.err("Invalid --width value: {s}\n", .{n});
                    return ArgError.MissingGetArgument;
                };
            } else if (std.mem.eql(u8, arg, "--input") or std.mem.eql(u8, arg, "-i")) {
                const fmt = args.next() orelse {
                    log.err("Missing format value after {s}\n", .{arg});
                    return ArgError.MissingGetArgument;
                };
                // Delegate to the same enum-driven lookup `check`/`fmt`/`convert`
                // use, so this never again drifts out of sync with `Format` as
                // formats are added (this used to be its own hand-rolled chain
                // of literal comparisons — see git blame — which is exactly how
                // it silently fell behind when ini/dotenv/properties were added).
                input_override = parseFormatName(fmt) orelse {
                    log.err("Unsupported format: {s}\n", .{fmt});
                    return ArgError.UnsupportedFileFormat;
                };
            } else if (std.mem.eql(u8, arg, "--output") or std.mem.eql(u8, arg, "-o")) {
                const fmt = args.next() orelse {
                    log.err("Missing format value after {s}\n", .{arg});
                    return ArgError.MissingGetArgument;
                };
                output_override = parseFormatName(fmt) orelse {
                    log.err("Unsupported format: {s}\n", .{fmt});
                    return ArgError.UnsupportedFileFormat;
                };
            } else if (!try positionals.add(allocator, "get", arg)) return ArgError.MissingGetArgument;
        }
        const pos = positionals.items.items;

        if (!requested_help and pos.len == 0) {
            log.err("No file provided.\n", .{});
            return ArgError.MissingGetArgument;
        }
        if (!requested_help and !positionals.atMost("get", 2, "a file and at most one path")) return ArgError.MissingGetArgument;
        const file_path = if (pos.len > 0) pos[0] else "-";

        var path: ?[]fig.AST.PathSegment = null;
        if (!requested_help and pos.len > 1) {
            path = try pathArg(allocator, pos[1]);
        }

        const detected_input: ?Detected = if (!requested_help and input_override == null)
            detectLanguageFromFileEnding(file_path)
        else
            null;
        // No `--input` and an unrecognized extension ⇒ sniff the contents in the
        // handler. `.json` here is a placeholder `from`/`to`, overwritten once the
        // real format is known.
        const needs_detect = !requested_help and input_override == null and detected_input == null;
        const input_format = input_override orelse
            (if (detected_input) |d| d.format else null) orelse jsonFormat();

        var gron_projection: gron.Projection = .gron;
        if (gron_root) |r| gron_projection.root_name = r;
        if (gron_sep) |s| gron_projection.assign = s;
        if (gron_term) |t| gron_projection.terminator = t;

        config.options = .{ .get = .{
            .file = file_path,
            .path = path,
            .from = input_format,
            .to = output_override orelse input_format,
            .requested_help = requested_help,
            .detect = needs_detect,
            .output_explicit = output_override != null,
            .lax_tags = lax_tags,
            .lossless = lossless,
            .serialize = serialize,
            .quiet = quiet,
            .strict = strict,
            .gron_projection = gron_projection,
        } };
    } else if (std.mem.eql(u8, action_str, "check") or std.mem.eql(u8, action_str, "ck")) {
        config.action = .check;

        var input_override: ?Format = null;
        var spec: ?[]const u8 = null;
        var quiet = false;
        var strict = false;
        var requested_help = false;
        var files: Positionals = .{};
        defer files.deinit(allocator);

        while (args.next()) |arg| {
            if (try files.rest(allocator, arg)) continue;
            if (isHelp(arg)) {
                requested_help = true;
            } else if (std.mem.eql(u8, arg, "--quiet") or std.mem.eql(u8, arg, "-q") or std.mem.eql(u8, arg, "--no-warnings")) {
                quiet = true;
            } else if (std.mem.eql(u8, arg, "--strict")) {
                strict = true;
            } else if (std.mem.eql(u8, arg, "--input") or std.mem.eql(u8, arg, "-i")) {
                const fmt = args.next() orelse {
                    log.err("Missing format value after {s}\n", .{arg});
                    return ArgError.MissingCheckArgument;
                };
                input_override = parseFormatName(fmt) orelse {
                    log.err("Unsupported format: {s}\n", .{fmt});
                    return ArgError.UnsupportedFileFormat;
                };
            } else if (std.mem.eql(u8, arg, "--spec") or std.mem.eql(u8, arg, "-s")) {
                spec = args.next() orelse {
                    log.err("Missing version value after {s}\n", .{arg});
                    return ArgError.MissingCheckArgument;
                };
            } else if (!try files.add(allocator, "check", arg)) return ArgError.MissingCheckArgument;
        }

        if (!requested_help and files.items.items.len == 0) {
            log.err("No file provided.\n", .{});
            return ArgError.MissingCheckArgument;
        }

        config.options = .{
            .check = .{
                // toOwnedSlice: the whole slice (allocated in the arena passed to
                // parseConfig) outlives this function, unlike `get` which only keeps
                // copies of individual positional headers.
                .files = try files.items.toOwnedSlice(allocator),
                .format = input_override,
                .spec = spec,
                .quiet = quiet,
                .strict = strict,
                .requested_help = requested_help,
            },
        };
    } else if (std.mem.eql(u8, action_str, "fmt") or std.mem.eql(u8, action_str, "f")) {
        config.action = .fmt;

        var input_override: ?Format = null;
        var quiet = false;
        var strict = false;
        var dry_run = false;
        var diff_mode = false;
        var requested_help = false;
        var serialize: fig.AST.SerializeOptions = .{};
        var positionals: Positionals = .{};
        defer positionals.deinit(allocator);

        while (args.next()) |arg| {
            if (try positionals.rest(allocator, arg)) continue;
            if (isHelp(arg)) {
                requested_help = true;
            } else if (std.mem.eql(u8, arg, "--dry-run")) {
                dry_run = true;
            } else if (std.mem.eql(u8, arg, "--diff")) {
                diff_mode = true;
            } else if (std.mem.eql(u8, arg, "--compact")) {
                serialize.pretty = false;
            } else if (std.mem.eql(u8, arg, "--pretty")) {
                serialize.pretty = true;
            } else if (std.mem.eql(u8, arg, "--strip-comments")) {
                serialize.strip_comments = true;
            } else if (std.mem.eql(u8, arg, "--quiet") or std.mem.eql(u8, arg, "-q") or std.mem.eql(u8, arg, "--no-warnings")) {
                quiet = true;
            } else if (std.mem.eql(u8, arg, "--strict")) {
                strict = true;
            } else if (std.mem.eql(u8, arg, "--indent")) {
                const n = args.next() orelse {
                    log.err("Missing value after {s}\n", .{arg});
                    return ArgError.MissingFmtArgument;
                };
                serialize.indent = std.fmt.parseInt(u8, n, 10) catch {
                    log.err("Invalid --indent value: {s}\n", .{n});
                    return ArgError.MissingFmtArgument;
                };
                // See the matching comment on `get`'s `--indent` handling above.
                serialize.indent_requested = true;
            } else if (std.mem.eql(u8, arg, "--width")) {
                const n = args.next() orelse {
                    log.err("Missing value after {s}\n", .{arg});
                    return ArgError.MissingFmtArgument;
                };
                serialize.width = std.fmt.parseInt(u16, n, 10) catch {
                    log.err("Invalid --width value: {s}\n", .{n});
                    return ArgError.MissingFmtArgument;
                };
            } else if (std.mem.eql(u8, arg, "--input") or std.mem.eql(u8, arg, "-i")) {
                const fmt_name = args.next() orelse {
                    log.err("Missing format value after {s}\n", .{arg});
                    return ArgError.MissingFmtArgument;
                };
                input_override = parseFormatName(fmt_name) orelse {
                    log.err("Unsupported format: {s}\n", .{fmt_name});
                    return ArgError.UnsupportedFileFormat;
                };
            } else if (!try positionals.add(allocator, "fmt", arg)) return ArgError.MissingFmtArgument;
        }

        if (!requested_help and positionals.items.items.len == 0) {
            log.err("No file provided.\n", .{});
            return ArgError.MissingFmtArgument;
        }
        // `fmt` reformats a whole file — there is no
        // sub-document path argument the way `get`/`replace`/etc. take one.
        if (!requested_help and positionals.items.items.len > 1) {
            log.err("fmt takes a single file, not a path within it: {s}\n", .{positionals.items.items[1]});
            return ArgError.MissingFmtArgument;
        }
        if (!requested_help and dry_run and diff_mode) {
            log.err("--dry-run and --diff are mutually exclusive.\n", .{});
            return ArgError.MissingFmtArgument;
        }

        const file_path = if (positionals.items.items.len > 0) positionals.items.items[0] else "-";

        const detected_input: ?Detected = if (!requested_help and input_override == null)
            detectLanguageFromFileEnding(file_path)
        else
            null;
        const needs_detect = !requested_help and input_override == null and detected_input == null;
        const from = input_override orelse
            (if (detected_input) |d| d.format else null) orelse jsonFormat();

        config.options = .{ .fmt = .{
            .file = file_path,
            .from = from,
            .requested_help = requested_help,
            .detect = needs_detect,
            .serialize = serialize,
            .quiet = quiet,
            .strict = strict,
            .dry_run = dry_run,
            .diff = diff_mode,
        } };
    } else if (std.mem.eql(u8, action_str, "convert") or std.mem.eql(u8, action_str, "cv")) {
        config.action = .convert;

        var input_override: ?Format = null;
        var output_override: ?Format = null;
        var lax_tags = false;
        var lossless = false;
        var quiet = false;
        var strict = false;
        var write = false;
        var diff_mode = false;
        var requested_help = false;
        var serialize: fig.AST.SerializeOptions = .{};
        var positionals: Positionals = .{};
        defer positionals.deinit(allocator);

        while (args.next()) |arg| {
            if (try positionals.rest(allocator, arg)) continue;
            if (isHelp(arg)) {
                requested_help = true;
            } else if (std.mem.eql(u8, arg, "--write") or std.mem.eql(u8, arg, "-w")) {
                write = true;
            } else if (std.mem.eql(u8, arg, "--dry-run")) {
                log.err("convert prints to stdout by default; pass --write/-w to write in place instead.\n", .{});
                return ArgError.MissingConvertArgument;
            } else if (std.mem.eql(u8, arg, "--diff")) {
                diff_mode = true;
            } else if (std.mem.eql(u8, arg, "--compact")) {
                serialize.pretty = false;
            } else if (std.mem.eql(u8, arg, "--pretty")) {
                serialize.pretty = true;
            } else if (std.mem.eql(u8, arg, "--strip-comments")) {
                serialize.strip_comments = true;
            } else if (std.mem.eql(u8, arg, "--lax-tags")) {
                lax_tags = true;
            } else if (std.mem.eql(u8, arg, "--lossless")) {
                lossless = true;
            } else if (std.mem.eql(u8, arg, "--lossy")) {
                lossless = false;
            } else if (std.mem.eql(u8, arg, "--quiet") or std.mem.eql(u8, arg, "-q") or std.mem.eql(u8, arg, "--no-warnings")) {
                quiet = true;
            } else if (std.mem.eql(u8, arg, "--strict")) {
                strict = true;
            } else if (std.mem.eql(u8, arg, "--indent")) {
                const n = args.next() orelse {
                    log.err("Missing value after {s}\n", .{arg});
                    return ArgError.MissingConvertArgument;
                };
                serialize.indent = std.fmt.parseInt(u8, n, 10) catch {
                    log.err("Invalid --indent value: {s}\n", .{n});
                    return ArgError.MissingConvertArgument;
                };
                // See the matching comment on `get`'s `--indent` handling above.
                serialize.indent_requested = true;
            } else if (std.mem.eql(u8, arg, "--width")) {
                const n = args.next() orelse {
                    log.err("Missing value after {s}\n", .{arg});
                    return ArgError.MissingConvertArgument;
                };
                serialize.width = std.fmt.parseInt(u16, n, 10) catch {
                    log.err("Invalid --width value: {s}\n", .{n});
                    return ArgError.MissingConvertArgument;
                };
            } else if (std.mem.eql(u8, arg, "--input") or std.mem.eql(u8, arg, "-i")) {
                const fmt_name = args.next() orelse {
                    log.err("Missing format value after {s}\n", .{arg});
                    return ArgError.MissingConvertArgument;
                };
                input_override = parseFormatName(fmt_name) orelse {
                    log.err("Unsupported format: {s}\n", .{fmt_name});
                    return ArgError.UnsupportedFileFormat;
                };
            } else if (std.mem.eql(u8, arg, "--output") or std.mem.eql(u8, arg, "-o")) {
                const fmt_name = args.next() orelse {
                    log.err("Missing format value after {s}\n", .{arg});
                    return ArgError.MissingConvertArgument;
                };
                output_override = parseFormatName(fmt_name) orelse {
                    log.err("Unsupported format: {s}\n", .{fmt_name});
                    return ArgError.UnsupportedFileFormat;
                };
            } else if (!try positionals.add(allocator, "convert", arg)) return ArgError.MissingConvertArgument;
        }

        if (!requested_help and positionals.items.items.len == 0) {
            log.err("No file provided.\n", .{});
            return ArgError.MissingConvertArgument;
        }
        if (!requested_help and positionals.items.items.len > 1) {
            log.err("convert takes a single file, not a path within it: {s}\n", .{positionals.items.items[1]});
            return ArgError.MissingConvertArgument;
        }
        if (!requested_help and output_override == null) {
            log.err("convert needs a target: pass --output <format>.\n", .{});
            return ArgError.MissingConvertArgument;
        }

        const file_path = if (positionals.items.items.len > 0) positionals.items.items[0] else "-";

        const detected_input: ?Detected = if (!requested_help) detectLanguageFromFileEnding(file_path) else null;

        const needs_detect = !requested_help and input_override == null and detected_input == null;
        const from = input_override orelse (if (detected_input) |d| d.format else null) orelse jsonFormat();
        config.options = .{ .convert = .{
            .file = file_path,
            .requested_help = requested_help,
            .from = from,
            .to = output_override orelse jsonFormat(),
            .detect = needs_detect,
            .lax_tags = lax_tags,
            .lossless = lossless,
            .serialize = serialize,
            .quiet = quiet,
            .strict = strict,
            .write = write,
            .diff = diff_mode,
        } };
    } else if (std.mem.eql(u8, action_str, "patch") or std.mem.eql(u8, action_str, "p")) {
        config.action = .patch;

        var input_override: ?Format = null;
        var patch_input_override: ?Format = null;
        var at: []fig.AST.PathSegment = &.{};
        var from: []fig.AST.PathSegment = &.{};
        var deletes: std.ArrayList(fig.Patch.Deletion) = .empty;
        defer deletes.deinit(allocator);
        var patch_options: fig.Patch.Options = .{};
        var lossless = false;
        var dry_run = false;
        var diff_mode = false;
        var quiet = false;
        var strict = false;
        var strip_comments = false;
        var comments_given = false;
        var requested_help = false;
        var positionals: Positionals = .{};
        defer positionals.deinit(allocator);

        while (args.next()) |arg| {
            if (try positionals.rest(allocator, arg)) continue;
            if (isHelp(arg)) {
                requested_help = true;
            } else if (std.mem.eql(u8, arg, "--dry-run")) {
                dry_run = true;
            } else if (std.mem.eql(u8, arg, "--diff")) {
                diff_mode = true;
            } else if (std.mem.eql(u8, arg, "--quiet") or std.mem.eql(u8, arg, "-q") or std.mem.eql(u8, arg, "--no-warnings")) {
                quiet = true;
            } else if (std.mem.eql(u8, arg, "--strict")) {
                strict = true;
            } else if (std.mem.eql(u8, arg, "--strip-comments")) {
                strip_comments = true;
            } else if (std.mem.eql(u8, arg, "--lossless")) {
                lossless = true;
            } else if (std.mem.eql(u8, arg, "--lossy")) {
                lossless = false;
            } else if (std.mem.eql(u8, arg, "--compact")) {
                patch_options.serialize.pretty = false;
            } else if (std.mem.eql(u8, arg, "--pretty")) {
                patch_options.serialize.pretty = true;
            } else if (std.mem.eql(u8, arg, "--at")) {
                const p = args.next() orelse {
                    log.err("Missing path after {s}\n", .{arg});
                    return ArgError.MissingPatchArgument;
                };
                at = try pathArg(allocator, p);
            } else if (std.mem.eql(u8, arg, "--from")) {
                const p = args.next() orelse {
                    log.err("Missing path after {s}\n", .{arg});
                    return ArgError.MissingPatchArgument;
                };
                from = try pathArg(allocator, p);
            } else if (std.mem.eql(u8, arg, "--delete")) {
                const p = args.next() orelse {
                    log.err("Missing path after {s}\n", .{arg});
                    return ArgError.MissingPatchArgument;
                };
                const parsed_path = try pathArg(allocator, p);
                if (parsed_path.len == 0) {
                    log.err("--delete needs a path within the document; the root cannot be deleted.\n", .{});
                    return ArgError.MissingPatchArgument;
                }
                try deletes.append(allocator, parsed_path);
            } else if (std.mem.eql(u8, arg, "--seq")) {
                const name = args.next() orelse {
                    log.err("Missing strategy after {s} (replace, append, union)\n", .{arg});
                    return ArgError.MissingPatchArgument;
                };
                // `union` is a Zig keyword, so the enum member is `unite`;
                // the CLI keeps the word a user would reach for.
                patch_options.seq = if (std.mem.eql(u8, name, "union"))
                    .unite
                else
                    std.meta.stringToEnum(fig.Patch.SeqStrategy, name) orelse {
                        log.err("Unknown --seq strategy: {s} (replace, append, union)\n", .{name});
                        return ArgError.MissingPatchArgument;
                    };
            } else if (std.mem.eql(u8, arg, "--comments")) {
                const name = args.next() orelse {
                    log.err("Missing strategy after {s} (ours, theirs, none)\n", .{arg});
                    return ArgError.MissingPatchArgument;
                };
                patch_options.comments = std.meta.stringToEnum(fig.Patch.CommentStrategy, name) orelse {
                    log.err("Unknown --comments strategy: {s} (ours, theirs, none)\n", .{name});
                    return ArgError.MissingPatchArgument;
                };
                comments_given = true;
            } else if (std.mem.eql(u8, arg, "--indent")) {
                const n = args.next() orelse {
                    log.err("Missing value after {s}\n", .{arg});
                    return ArgError.MissingPatchArgument;
                };
                patch_options.serialize.indent = std.fmt.parseInt(u8, n, 10) catch {
                    log.err("Invalid --indent value: {s}\n", .{n});
                    return ArgError.MissingPatchArgument;
                };
                patch_options.serialize.indent_requested = true;
            } else if (std.mem.eql(u8, arg, "--width")) {
                const n = args.next() orelse {
                    log.err("Missing value after {s}\n", .{arg});
                    return ArgError.MissingPatchArgument;
                };
                patch_options.serialize.width = std.fmt.parseInt(u16, n, 10) catch {
                    log.err("Invalid --width value: {s}\n", .{n});
                    return ArgError.MissingPatchArgument;
                };
            } else if (std.mem.eql(u8, arg, "--input") or std.mem.eql(u8, arg, "-i")) {
                const fmt_name = args.next() orelse {
                    log.err("Missing format value after {s}\n", .{arg});
                    return ArgError.MissingPatchArgument;
                };
                input_override = parseFormatName(fmt_name) orelse {
                    log.err("Unsupported format: {s}\n", .{fmt_name});
                    return ArgError.UnsupportedFileFormat;
                };
            } else if (std.mem.eql(u8, arg, "--patch-input")) {
                const fmt_name = args.next() orelse {
                    log.err("Missing format value after {s}\n", .{arg});
                    return ArgError.MissingPatchArgument;
                };
                patch_input_override = parseFormatName(fmt_name) orelse {
                    log.err("Unsupported format: {s}\n", .{fmt_name});
                    return ArgError.UnsupportedFileFormat;
                };
            } else if (!try positionals.add(allocator, "patch", arg)) return ArgError.MissingPatchArgument;
        }

        // `--strip-comments` is `get`/`convert`'s word for what `patch` calls
        // `--comments none`: no comment from the patch reaches the target.
        // (The target's own are never stripped — they are what patch keeps.)
        if (strip_comments) {
            if (comments_given and patch_options.comments != .none) {
                log.err("--strip-comments carries no comment from the patch; --comments {s} asks for some. Pass one.\n", .{@tagName(patch_options.comments)});
                return ArgError.MissingPatchArgument;
            }
            patch_options.comments = .none;
        }
        if (!requested_help and positionals.items.items.len < 2) {
            log.err("patch takes two files: the document to change, then the one supplying the change.\n", .{});
            return ArgError.MissingPatchArgument;
        }
        if (!requested_help and positionals.items.items.len > 2) {
            log.err("patch takes two files; use --at/--from to name a path within one: {s}\n", .{positionals.items.items[2]});
            return ArgError.MissingPatchArgument;
        }
        // Both from a pipe would mean reading one stream twice and getting
        // half of each; the second read comes back empty and the failure is
        // an empty patch, which is silent. Refuse it up front.
        const target_path = if (positionals.items.items.len > 0) positionals.items.items[0] else "-";
        const source_path = if (positionals.items.items.len > 1) positionals.items.items[1] else "-";
        if (!requested_help and std.mem.eql(u8, target_path, "-") and std.mem.eql(u8, source_path, "-")) {
            log.err("only one of the two files can be stdin.\n", .{});
            return ArgError.MissingPatchArgument;
        }

        const target_ext: ?Detected = if (!requested_help) detectLanguageFromFileEnding(target_path) else null;
        const source_ext: ?Detected = if (!requested_help) detectLanguageFromFileEnding(source_path) else null;

        config.options = .{ .patch = .{
            .file = target_path,
            .patch_file = source_path,
            .at = at,
            .from = from,
            .deletes = try deletes.toOwnedSlice(allocator),
            .patch_options = patch_options,
            .requested_help = requested_help,
            .format = input_override orelse (if (target_ext) |d| d.format else null) orelse jsonFormat(),
            .detect = !requested_help and input_override == null and target_ext == null,
            .patch_format = patch_input_override orelse (if (source_ext) |d| d.format else null) orelse jsonFormat(),
            .detect_patch = !requested_help and patch_input_override == null and source_ext == null,
            .lossless = lossless,
            .dry_run = dry_run,
            .diff = diff_mode,
            .quiet = quiet,
            .strict = strict,
        } };
    } else if (std.mem.eql(u8, action_str, "lang")) {
        config.action = .lang;
        var opts: types.LangOptions = .{};
        var positionals: Positionals = .{};
        defer positionals.deinit(allocator);
        while (args.next()) |arg| {
            if (try positionals.rest(allocator, arg)) continue;
            if (isHelp(arg)) {
                opts.requested_help = true;
            } else if (std.mem.eql(u8, arg, "--against")) {
                opts.against = args.next() orelse {
                    log.err("Missing format after --against\n", .{});
                    return ArgError.UnsupportedFileFormat;
                };
            } else if (std.mem.eql(u8, arg, "--input") or std.mem.eql(u8, arg, "-i")) {
                const name = args.next() orelse {
                    log.err("Missing format after --input\n", .{});
                    return ArgError.UnsupportedFileFormat;
                };
                opts.input = parseFormatName(name) orelse {
                    log.err("Unsupported input format: {s}\n", .{name});
                    return ArgError.UnsupportedFileFormat;
                };
            } else if (std.mem.eql(u8, arg, "--spec") or std.mem.eql(u8, arg, "-s")) {
                opts.spec = args.next() orelse {
                    log.err("Missing version value after {s}\n", .{arg});
                    return ArgError.MissingLangArgument;
                };
            } else if (!try positionals.add(allocator, "lang", arg)) return ArgError.MissingLangArgument;
        }
        const pos = positionals.items.items;
        if (pos.len == 0 or std.mem.eql(u8, pos[0], "list")) {
            opts.verb = .list;
            if (!opts.requested_help and !positionals.atMost("lang list", 1, "no arguments")) return ArgError.MissingLangArgument;
        } else if (std.mem.eql(u8, pos[0], "check")) {
            opts.verb = .check;
            if (pos.len >= 2) {
                opts.name = pos[1];
                opts.files = try allocator.dupe([]const u8, pos[2..]);
            } else if (!opts.requested_help) {
                log.err("lang check needs a language name (e.g. `fig lang check lua-dotenv --against dotenv`).\n", .{});
                return ArgError.MissingLangArgument;
            }
        } else if (std.mem.eql(u8, pos[0], "table")) {
            opts.verb = .table;
            if (pos.len >= 2) {
                opts.name = pos[1];
                if (!opts.requested_help and !positionals.atMost("lang table", 2, "one file")) return ArgError.MissingLangArgument;
            } else if (!opts.requested_help) {
                log.err("lang table needs a file (e.g. `fig lang table secrets.env`).\n", .{});
                return ArgError.MissingLangArgument;
            }
        } else {
            log.err("Unknown lang verb: {s} (list, check, table)\n", .{pos[0]});
            return ArgError.MissingLangArgument;
        }
        config.options = .{ .lang = opts };
    } else if (externalCommandName(action_str)) |name| {
        // Git's fallback, and the reason fig-schema can grow a CLI without
        // fig growing a `schema` action: a word fig has no verb for is handed
        // to a `fig-<word>` executable. Everything after the word is passed
        // through untouched — see `ExternalOptions.argv` — so this branch
        // consumes the rest of the iterator and parses none of it.
        config.action = .external;
        const program = try std.fmt.allocPrint(allocator, "fig-{s}", .{name});
        var argv: std.ArrayList([]const u8) = .empty;
        try argv.append(allocator, program);
        while (args.next()) |arg| try argv.append(allocator, arg);
        config.options = .{ .external = .{
            .name = name,
            .program = program,
            .argv = try argv.toOwnedSlice(allocator),
        } };
    } else {
        // Not one of fig's verbs, and not a word that could name a program
        // either (`externalCommandName` says which shapes those are). Still
        // an `.external`: a null `program` means the handoff was never on the
        // table, so one place gets to say "no such action" for both halves of
        // the unrecognized case rather than two places saying it differently.
        config.action = .external;
        config.options = .{ .external = .{ .name = action_str, .program = null, .argv = &.{} } };
    }

    return config;
}

/// The `<name>` for a `fig-<name>` handoff, or null if this word can't be one
/// — in which case it stays an ordinary unrecognized action.
///
/// The rule is deliberately narrower than "whatever `execvp` would take".
/// Restricting it to ASCII letters, digits, `-` and `_` keeps two shapes out:
/// a leading `-`, so a mistyped flag (`fig --colour`) reports itself as a flag
/// instead of hunting for `fig---colour`; and anything containing a path
/// separator or a `.`, so a forgotten verb (`fig config.toml`, `fig ../x`)
/// can't reach the filesystem as a program name. That second one is the load-
/// bearing half: `argv[0]` with a separator in it bypasses PATH lookup
/// entirely and runs a file at that path, which is not something a typo should
/// be able to do.
fn externalCommandName(word: []const u8) ?[]const u8 {
    if (word.len == 0 or word[0] == '-') return null;
    for (word) |c| switch (c) {
        'a'...'z', 'A'...'Z', '0'...'9', '-', '_' => {},
        else => return null,
    };
    return word;
}

// A slice-backed stand-in for the process arg iterator `parseConfig` consumes.
const TestArgs = struct {
    items: []const []const u8,
    i: usize = 0,
    fn next(self: *TestArgs) ?[]const u8 {
        if (self.i >= self.items.len) return null;
        defer self.i += 1;
        return self.items[self.i];
    }
};

test "parsePath reads the [-]/[$] append sentinel and literal indices" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    const dash = try parsePath(a, "list[-]");
    try t.expectEqual(@as(usize, 2), dash.len);
    try t.expectEqualStrings("list", dash[0].key);
    try t.expectEqual(append_index, dash[1].index);

    const dollar = try parsePath(a, "list[$]");
    try t.expectEqual(append_index, dollar[1].index);

    const literal = try parsePath(a, "a.b[2]");
    try t.expectEqual(@as(usize, 2), literal[2].index);
}

test "parsePath takes a key with a `.` or `[` quoted, bracketed or escaped" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    for ([_][]const u8{ "a.\"b.c\"", "a.'b.c'", "a[\"b.c\"]", "a['b.c']", "a.b\\.c" }) |spelling| {
        const p = try parsePath(a, spelling);
        try t.expectEqual(@as(usize, 2), p.len);
        try t.expectEqualStrings("a", p[0].key);
        try t.expectEqualStrings("b.c", p[1].key);
    }
    // A quoted segment is followed by more path, and holds a `[` too.
    const deeper = try parsePath(a, "\"x[0]\".y[1]");
    try t.expectEqualStrings("x[0]", deeper[0].key);
    try t.expectEqualStrings("y", deeper[1].key);
    try t.expectEqual(@as(usize, 1), deeper[2].index);
    // Double quotes decode JSON's escapes, which is how gron prints a key.
    const esc = try parsePath(a, "[\"q\\\"\\u00e9\\ud83d\\ude00\"]");
    try t.expectEqualStrings("q\"\u{e9}\u{1f600}", esc[0].key);
    // Single quotes are verbatim.
    const lit = try parsePath(a, "'a\\n'");
    try t.expectEqualStrings("a\\n", lit[0].key);
    // An unclosed quote, a quote run into more key, and a trailing
    // backslash are refused.
    for ([_][]const u8{ "a.\"b", "a.'b", "a.\"b\"c", "a\\", "[\"b\"", "[\"\\x\"]" }) |bad|
        try t.expectError(ArgError.InvalidPath, parsePath(a, bad));
}

test "parseConfig routes insert/delete to the right action and path tail" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    // insert into a mapping: trailing key, value captured.
    var ins = TestArgs{ .items = &.{ "fig", "insert", "f.yaml", "a.newkey", "42" } };
    const ic = try parseConfig(a, &ins);
    try t.expectEqual(CliAction.insert, ic.action);
    try t.expectEqualStrings("newkey", ic.options.insert.path[1].key);
    try t.expectEqualStrings("42", ic.options.insert.value);
    try t.expectEqual(parseFormatName("yaml").?, ic.options.insert.format);

    // insert append onto a sequence: trailing sentinel index.
    var app = TestArgs{ .items = &.{ "fig", "insert", "f.yaml", "list[-]", "z" } };
    const ac = try parseConfig(a, &app);
    try t.expectEqual(append_index, ac.options.insert.path[1].index);

    // delete by index: format sniffed later, path tail is an index.
    var del = TestArgs{ .items = &.{ "fig", "delete", "f.toml", "list[1]" } };
    const dc = try parseConfig(a, &del);
    try t.expectEqual(CliAction.delete, dc.action);
    try t.expectEqual(@as(usize, 1), dc.options.delete.path[1].index);
    try t.expectEqual(parseFormatName("toml").?, dc.options.delete.format);
}

test "parseTarget names the file a failed action is re-parsed from, and its format" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    // An in-place edit: the file and the format its extension named.
    var set = TestArgs{ .items = &.{ "fig", "set", "f.yaml", "a", "1" } };
    const st = types.parseTarget(try parseConfig(a, &set)).?;
    try t.expectEqualStrings("f.yaml", st.file);
    try t.expectEqual(parseFormatName("yaml"), st.format);

    // A read: its `from` format.
    var get = TestArgs{ .items = &.{ "fig", "get", "f.toml" } };
    try t.expectEqual(parseFormatName("toml"), types.parseTarget(try parseConfig(a, &get)).?.format);

    // An extension that names no format: the handler sniffs, so no format
    // is forced onto the file.
    var md = TestArgs{ .items = &.{ "fig", "set", "notes.md", "a", "1" } };
    try t.expectEqual(@as(?Format, null), types.parseTarget(try parseConfig(a, &md)).?.format);

    // stdin has been read once already, and `check` reports for itself.
    var stdin = TestArgs{ .items = &.{ "fig", "get", "-", "-i", "json" } };
    try t.expect(types.parseTarget(try parseConfig(a, &stdin)) == null);
    var check = TestArgs{ .items = &.{ "fig", "check", "f.yaml" } };
    try t.expect(types.parseTarget(try parseConfig(a, &check)) == null);
}

test "parseConfig hands an unknown action to fig-<action> with its arguments untouched" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    // The case this exists for. Note `--help` and `check`: fig owns both
    // words, and neither is parsed here — everything after `schema` is the
    // other tool's to read.
    var ext = TestArgs{ .items = &.{ "fig", "schema", "check", "--help", "-" } };
    const ec = try parseConfig(a, &ext);
    try t.expectEqual(CliAction.external, ec.action);
    try t.expectEqualStrings("schema", ec.options.external.name);
    try t.expectEqualStrings("fig-schema", ec.options.external.program.?);
    try t.expectEqual(@as(usize, 4), ec.options.external.argv.len);
    try t.expectEqualStrings("fig-schema", ec.options.external.argv[0]);
    try t.expectEqualStrings("check", ec.options.external.argv[1]);
    try t.expectEqualStrings("--help", ec.options.external.argv[2]);
    try t.expectEqualStrings("-", ec.options.external.argv[3]);

    // The program name is `fig-`, not `argv[0]-`: what fig was invoked as
    // says nothing about what its siblings are installed as.
    var qualified = TestArgs{ .items = &.{ "./zig-out/bin/fig", "schema" } };
    const qc = try parseConfig(a, &qualified);
    try t.expectEqualStrings("fig-schema", qc.options.external.program.?);

    // A word that can't name a program is still `.external` — with a null
    // `program`, so the report knows PATH was never searched.
    for ([_][]const u8{ "config.toml", "--colour", "../evil", "a/b" }) |word| {
        var bad = TestArgs{ .items = &.{ "fig", word } };
        const bc = try parseConfig(a, &bad);
        try t.expectEqual(CliAction.external, bc.action);
        try t.expectEqualStrings(word, bc.options.external.name);
        try t.expectEqual(@as(?[]const u8, null), bc.options.external.program);
    }

    // ...and an action fig does own never gets there, alias included.
    var owned = TestArgs{ .items = &.{ "fig", "ck", "f.yaml" } };
    try t.expectEqual(CliAction.check, (try parseConfig(a, &owned)).action);
}

test "externalCommandName accepts only words that could name a program" {
    const t = std.testing;
    try t.expectEqualStrings("schema", externalCommandName("schema").?);
    try t.expectEqualStrings("two-words_9", externalCommandName("two-words_9").?);
    // A leading `-` is a mistyped flag, not a subcommand.
    try t.expectEqual(@as(?[]const u8, null), externalCommandName("-x"));
    try t.expectEqual(@as(?[]const u8, null), externalCommandName("--colour"));
    // Anything with a separator or a dot is a path or a filename the user
    // meant to give an action. A separator in argv[0] would skip PATH lookup
    // and execute that file, which no typo should be able to do.
    try t.expectEqual(@as(?[]const u8, null), externalCommandName("./x"));
    try t.expectEqual(@as(?[]const u8, null), externalCommandName("../../tmp/x"));
    try t.expectEqual(@as(?[]const u8, null), externalCommandName("a\\b"));
    try t.expectEqual(@as(?[]const u8, null), externalCommandName("config.toml"));
    try t.expectEqual(@as(?[]const u8, null), externalCommandName(""));
}

test "detectLanguageFromFileEnding: .md/.markdown name no language" {
    // The extension table is derived from each compiled-in language's own
    // `extensions`, so a gated-out language contributes none — `.yaml`/`.figl`
    // below simply have no entry in a build without YAML/fig.
    if (comptime !(build_options.lang_yaml and build_options.lang_figl)) return error.SkipZigTest;
    const t = std.testing;
    // 6.0: the CLI reads no embedded block, so a Markdown or HTML file is
    // one whose format it can't tell, like any other unknown extension.
    try t.expect(detectLanguageFromFileEnding("post.md") == null);
    try t.expect(detectLanguageFromFileEnding("post.markdown") == null);
    try t.expect(detectLanguageFromFileEnding("page.html") == null);

    try t.expectEqual(parseFormatName("yaml").?, detectLanguageFromFileEnding("f.yaml").?.format);
    try t.expectEqual(parseFormatName("figl").?, detectLanguageFromFileEnding("f.figl").?.format);

    // `.fig` is not an extension of the format any more (5.0).
    try t.expect(detectLanguageFromFileEnding("f.fig") == null);
}

test "figl is the authoring dialect's name and extension, and fig is neither" {
    const t = std.testing;
    if (comptime !build_options.lang_figl) return error.SkipZigTest;

    const figl = parseFormatName("figl").?;
    try t.expectEqualStrings("figl", types.name(figl));
    try t.expectEqual(figl, detectLanguageFromFileEnding("f.figl").?.format);
    // 6.0: `fig` is the library's name, and no format's.
    try t.expect(parseFormatName("fig") == null);
}

test "yml is an accepted spelling of yaml, as a --input value and as an extension" {
    const t = std.testing;
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;

    // `--input yml`: the alias line above, not an enum member of its own.
    try t.expectEqual(parseFormatName("yaml"), parseFormatName("yml"));
    try t.expectEqual(parseFormatName("yaml"), parseFormatName("yaml"));

    // `.yml` files: `stringToEnum` no longer answers for the extension, so it
    // falls through to `Language.YAML.extensions`, which owns both spellings —
    // the same `.yaml` parse either way.
    const yml_ext = detectLanguageFromFileEnding("f.yml").?;
    try t.expectEqual(parseFormatName("yaml").?, yml_ext.format);
    try t.expectEqual(parseFormatName("yaml").?, detectLanguageFromFileEnding("f.yaml").?.format);

    // An unknown token is still unknown (the alias must not widen the lookup).
    try t.expectEqual(@as(?Format, null), parseFormatName("yamll"));
}

test "parseConfig routes patch: both files, both sides' formats, and the merge strategies" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    // Two positionals, in order: the file being changed, then the one
    // supplying the change. Each side resolves its own format from its own
    // extension, so a cross-format patch needs no flags at all.
    var basic = TestArgs{ .items = &.{ "fig", "patch", "config.yaml", "overlay.toml" } };
    const b = try parseConfig(a, &basic);
    try t.expectEqual(CliAction.patch, b.action);
    try t.expectEqualStrings("config.yaml", b.options.patch.file);
    try t.expectEqualStrings("overlay.toml", b.options.patch.patch_file);
    try t.expectEqual(parseFormatName("yaml").?, b.options.patch.format);
    try t.expectEqual(parseFormatName("toml").?, b.options.patch.patch_format);
    try t.expect(!b.options.patch.detect);
    try t.expect(!b.options.patch.detect_patch);
    // The defaults are the conservative ones: the target's comments stand and
    // a sequence is taken whole.
    try t.expectEqual(fig.Patch.SeqStrategy.replace, b.options.patch.patch_options.seq);
    try t.expectEqual(fig.Patch.CommentStrategy.ours, b.options.patch.patch_options.comments);

    // `--at`/`--from` are separate paths into separate documents.
    var paths = TestArgs{ .items = &.{ "fig", "patch", "a.yaml", "b.yaml", "--at", "service.db", "--from", "db" } };
    const p = try parseConfig(a, &paths);
    try t.expectEqual(@as(usize, 2), p.options.patch.at.len);
    try t.expectEqualStrings("service", p.options.patch.at[0].key);
    try t.expectEqualStrings("db", p.options.patch.at[1].key);
    try t.expectEqual(@as(usize, 1), p.options.patch.from.len);
    try t.expectEqualStrings("db", p.options.patch.from[0].key);

    // `--delete` accumulates; `union` is the CLI spelling of `.unite` (the
    // enum can't be called `union`).
    var strat = TestArgs{ .items = &.{
        "fig",      "patch", "a.yaml",     "b.yaml",
        "--seq",    "union", "--comments", "theirs",
        "--delete", "x.y",   "--delete",   "z[2]",
    } };
    const s = try parseConfig(a, &strat);
    try t.expectEqual(fig.Patch.SeqStrategy.unite, s.options.patch.patch_options.seq);
    try t.expectEqual(fig.Patch.CommentStrategy.theirs, s.options.patch.patch_options.comments);
    try t.expectEqual(@as(usize, 2), s.options.patch.deletes.len);
    try t.expectEqualStrings("x", s.options.patch.deletes[0][0].key);
    try t.expectEqual(@as(usize, 2), s.options.patch.deletes[1][1].index);

    // Unknown extensions on both sides defer both formats to a content sniff.
    var sniff = TestArgs{ .items = &.{ "fig", "patch", "a.weird", "-" } };
    const sn = try parseConfig(a, &sniff);
    try t.expect(sn.options.patch.detect);
    try t.expect(sn.options.patch.detect_patch);

    // The rejections (fewer or more than two files, both files stdin, an
    // unknown --seq/--comments strategy, a rootless --delete) all return
    // `ArgError.MissingPatchArgument` after a `log.err`, which this test
    // binary's runner counts as a failure regardless — see the note in the
    // `convert` test above.
}

test "parseConfig routes set and --seq" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    // Scalar upsert: path + value captured, format from extension.
    var s = TestArgs{ .items = &.{ "fig", "set", "f.yaml", "a.b", "1" } };
    const sc = try parseConfig(a, &s);
    try t.expectEqual(CliAction.set, sc.action);
    try t.expectEqualStrings("b", sc.options.set.path[1].key);
    try t.expectEqualStrings("1", sc.options.set.value);
    try t.expect(!sc.options.set.seq);
    try t.expectEqual(parseFormatName("yaml").?, sc.options.set.format);

    // --seq collects the trailing items into `values`.
    var sq = TestArgs{ .items = &.{ "fig", "set", "--seq", "f.yaml", "tags", "x", "y", "z" } };
    const sqc = try parseConfig(a, &sq);
    try t.expect(sqc.options.set.seq);
    try t.expectEqual(@as(usize, 3), sqc.options.set.values.len);
    try t.expectEqualStrings("z", sqc.options.set.values[2]);
}

test "parseConfig routes convert" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    // Whole-file mode: --input/--output resolve `from`/`to` directly.
    var wf = TestArgs{ .items = &.{ "fig", "convert", "-i", "yaml", "-o", "toml", "f.yaml" } };
    const wfc = try parseConfig(a, &wf);
    try t.expectEqual(CliAction.convert, wfc.action);
    try t.expectEqual(parseFormatName("yaml").?, wfc.options.convert.from);
    try t.expectEqual(parseFormatName("toml").?, wfc.options.convert.to);
    try t.expect(!wfc.options.convert.detect);

    // Whole-file mode with an unrecognized extension: `--output` alone still
    // needs `from` sniffed at runtime.
    var det = TestArgs{ .items = &.{ "fig", "convert", "-o", "json", "f.weirdext" } };
    const detc = try parseConfig(a, &det);
    try t.expect(detc.options.convert.detect);

    // The rejections (no --output) return `ArgError.MissingConvertArgument`
    // after a `log.err` — verified against the built CLI rather than here,
    // since this test binary's default runner fails any test that logs at
    // `.err` regardless of whether the returned error was expected (see
    // `test_runner.zig`'s `log_err_count`), the same reason no other
    // `parseConfig` error path in this file is exercised as a unit test.

    // An explicit --input reads a file whose extension names no format.
    var mdforced = TestArgs{ .items = &.{ "fig", "convert", "-i", "yaml", "-o", "toml", "post.md" } };
    const mdforcedc = try parseConfig(a, &mdforced);
    try t.expectEqual(parseFormatName("yaml").?, mdforcedc.options.convert.from);
    try t.expect(!mdforcedc.options.convert.detect);
}

test "parseConfig: --lang is taken from anywhere in the line and names the format" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    var args = TestArgs{ .items = &.{ "fig", "get", "secrets.env", "--lang", "json" } };
    const config = try parseConfig(a, &args);
    try t.expectEqual(types.CliAction.get, config.action);
    try t.expectEqualStrings("secrets.env", config.options.get.file);
    try t.expectEqual(parseFormatName("json").?, languages.langOverride().?);
    // The override is what the extension would otherwise decide.
    try t.expectEqual(parseFormatName("json").?, detectLanguageFromFileEnding("secrets.env").?.format);
    languages.setLangOverride(parseFormatName("yaml").?);
    try t.expectEqual(parseFormatName("yaml").?, detectLanguageFromFileEnding("secrets.env").?.format);

    // `--lang nosuch` and a bare `--lang` return `UnsupportedFileFormat`
    // after a `log.err` — the test runner counts a logged error as a
    // failure, so that path is proven by `tools/cli-lang-check.sh` instead.
    languages.setLangOverride(null);
}

test "parseConfig routes lang: list by default, check with a name, --against and files, table with -i" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    var bare = TestArgs{ .items = &.{ "fig", "lang" } };
    const c1 = try parseConfig(a, &bare);
    try t.expectEqual(types.CliAction.lang, c1.action);
    try t.expectEqual(types.LangOptions.Verb.list, c1.options.lang.verb);

    var check = TestArgs{ .items = &.{ "fig", "lang", "check", "lua-dotenv", "--against", "dotenv", "a.env", "b.env" } };
    const c2 = try parseConfig(a, &check);
    try t.expectEqual(types.LangOptions.Verb.check, c2.options.lang.verb);
    try t.expectEqualStrings("lua-dotenv", c2.options.lang.name);
    try t.expectEqualStrings("dotenv", c2.options.lang.against.?);
    try t.expectEqual(@as(usize, 2), c2.options.lang.files.len);
    try t.expectEqualStrings("b.env", c2.options.lang.files[1]);

    var table = TestArgs{ .items = &.{ "fig", "lang", "table", "secrets.env", "-i", "json" } };
    const ct = try parseConfig(a, &table);
    try t.expectEqual(types.LangOptions.Verb.table, ct.options.lang.verb);
    try t.expectEqualStrings("secrets.env", ct.options.lang.name);
    try t.expectEqual(parseFormatName("json").?, ct.options.lang.input.?);
    try t.expect(ct.options.lang.spec == null);

    var versioned = TestArgs{ .items = &.{ "fig", "lang", "table", "a.yaml", "--spec", "1.1" } };
    const cv = try parseConfig(a, &versioned);
    try t.expectEqualStrings("1.1", cv.options.lang.spec.?);

    // `check --help` asks for the help text without a name.
    var helpful = TestArgs{ .items = &.{ "fig", "lang", "check", "--help" } };
    const c3 = try parseConfig(a, &helpful);
    try t.expect(c3.options.lang.requested_help);
}

test "parseConfig: `--` ends the flags, a negative number is a value, and flags stand anywhere" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    // A file whose name begins with `-`, after `--`.
    var dashed = TestArgs{ .items = &.{ "fig", "get", "--", "-x.yaml", "a" } };
    const dc = try parseConfig(a, &dashed);
    try t.expectEqualStrings("-x.yaml", dc.options.get.file);
    try t.expectEqualStrings("a", dc.options.get.path.?[0].key);

    // A value that begins with `-`: a number stands on its own, anything
    // else goes after `--`, and `--help` there is a value too.
    var neg = TestArgs{ .items = &.{ "fig", "set", "f.yaml", "n", "-5" } };
    try t.expectEqualStrings("-5", (try parseConfig(a, &neg)).options.set.value);
    var frac = TestArgs{ .items = &.{ "fig", "insert", "f.yaml", "n", "-.5" } };
    try t.expectEqualStrings("-.5", (try parseConfig(a, &frac)).options.insert.value);
    var word = TestArgs{ .items = &.{ "fig", "replace", "f.yaml", "n", "--", "--help" } };
    const wc = try parseConfig(a, &word);
    try t.expect(!wc.options.replace.requested_help);
    try t.expectEqualStrings("--help", wc.options.replace.replacement);

    // `-` alone is stdin, not a flag.
    var stdin = TestArgs{ .items = &.{ "fig", "delete", "-", "a" } };
    try t.expectEqualStrings("-", (try parseConfig(a, &stdin)).options.delete.file);

    // A flag after the positionals is still a flag.
    var late = TestArgs{ .items = &.{ "fig", "comment", "f.yaml", "a", "--get", "--inline" } };
    const lc = try parseConfig(a, &late);
    try t.expect(lc.options.comment.get and lc.options.comment.inline_comment);
    var key = TestArgs{ .items = &.{ "fig", "rename", "f.yaml", "a", "b" } };
    const kc = try parseConfig(a, &key);
    try t.expectEqual(CliAction.rename, kc.action);
    try t.expect(kc.options.rename.key);
    try t.expectEqualStrings("b", kc.options.rename.replacement);
    var value = TestArgs{ .items = &.{ "fig", "replace", "f.yaml", "a", "b", "--string" } };
    const vc = try parseConfig(a, &value);
    try t.expect(!vc.options.replace.key);
    try t.expectEqual(types.ValueMode.string, vc.options.replace.value_mode);

    // `--lang` after `--` is a positional, not the global flag.
    var lang = TestArgs{ .items = &.{ "fig", "check", "--", "--lang" } };
    const lgc = try parseConfig(a, &lang);
    try t.expectEqualStrings("--lang", lgc.options.check.files[0]);
    try t.expectEqual(@as(?Format, null), languages.langOverride());

    // The refusals (an unknown flag, one positional too many) return the
    // action's usage error after a `log.err`, which this runner counts as a
    // failure — `tools/cli-args-check.sh` drives them through the binary.
}

test "spelledAsFlag: a leading `-` is a flag unless it starts a number or stands alone" {
    const t = std.testing;
    for ([_][]const u8{ "-x", "--bogus", "-h", "--", "-inf", "-nan" }) |s| try t.expect(spelledAsFlag(s));
    for ([_][]const u8{ "-", "-5", "-0.5", "-.5", "x", "" }) |s| try t.expect(!spelledAsFlag(s));
}

test "parseConfig: a `--` that is a flag's value does not end the flags for --lang" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    defer languages.setLangOverride(null);

    var args = TestArgs{ .items = &.{ "fig", "get", "f.txt", "--gron-sep", "--", "--lang", "json" } };
    const config = try parseConfig(a, &args);
    try t.expectEqualStrings("--", config.options.get.gron_projection.assign);
    try t.expectEqual(parseFormatName("json").?, languages.langOverride().?);
}
