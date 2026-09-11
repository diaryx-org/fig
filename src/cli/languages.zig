//! Languages the CLI did not compile in: the `languages.figl` configuration
//! that names them, the helper runner that speaks to each as a child
//! process, and the registration that makes one a `Format` every action
//! accepts. `docs/proposals/runtime-languages.md` §7.1.
//!
//! A configured language is a name, the extensions it owns, and a command.
//! Nothing is spawned until a name or extension the CLI cannot resolve
//! itself is asked for: `parseFormatName` and `detectLanguageFromFileEnding`
//! (in `args.zig`) fall through to `resolveName` and `resolveExtension` here,
//! which read the configuration once and spawn only the helper that answers.
//! `fig lang list` and `fig lang check` are the two actions that spawn on
//! purpose.
//!
//! The helper runner is a `fig.Runtime.VTable` whose functions talk to a
//! process over the wire `bindings/rust/fig/src/helper.rs` documents:
//! newline-delimited JSON, one request line to one response line —
//! `describe` once at load, then `parse`, `print` and `render` per call. The
//! runner is therefore a vtable like any other, and it registers through
//! the same `Runtime.register` a host's own vtable does; there is no second
//! way in. The child's stderr stays connected to the terminal, so a helper
//! that wants to say why it refused something can.
//!
//! Configuration is fig format, found in this order and merged with the
//! earlier file winning a name: `$FIG_LANGUAGES` (a file path — the way a
//! test points at one); `.fig/languages.figl` in the working directory and
//! each ancestor; `$XDG_CONFIG_HOME/fig/languages.figl`, which is
//! `~/.config/fig/languages.figl` when the variable is unset.
//!
//! ```fig
//! language[]
//! > name = lua-dotenv
//! > extensions = [tkv]
//! > command = [fig-lua, ~/.config/fig/languages/dotenv.lua]
//! ```
//!
//! `name` is the spelling `--input`, `--output` and `--lang` accept, and
//! what the helper must describe itself as; `extensions` is what resolves a
//! file to it (a compiled format's extension wins, which is why the Lua
//! twin of dotenv is not reached from `.env` without `--lang`); `command` is
//! argv, with a leading `~` in any argument expanded to `$HOME`.

const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");
const Io = std.Io;
const Allocator = std.mem.Allocator;

const types = @import("types.zig");
const Format = types.Format;
const Runtime = fig.Runtime;

const log = std.log.scoped(.languages);

/// One configured language, as `languages.figl` states it.
pub const Configured = struct {
    name: []const u8,
    extensions: []const []const u8,
    command: []const []const u8,
    /// Which file it came from, for `fig lang list` and for a message.
    source: []const u8,
    /// Set once the helper has been spawned and registered.
    entry: ?*const Runtime.Entry = null,
    /// Why registration failed, when it did, so a second ask does not
    /// spawn again.
    failure: ?[]const u8 = null,
};

/// The process-wide state: set by `init` before arguments are parsed, read
/// by the two resolvers. One CLI invocation, one configuration.
var state: struct {
    io: ?Io = null,
    allocator: Allocator = undefined,
    environ: ?*const std.process.Environ.Map = null,
    loaded: bool = false,
    configured: std.ArrayList(Configured) = .empty,
    /// The `--lang` selection, resolved, so every action's `--input`
    /// resolution can read it without each parser threading the flag.
    lang_override: ?Format = null,
} = .{};

/// Give the module what it needs to spawn: called once from `main` before
/// `parseConfig`, which is where names and extensions are resolved.
pub fn init(io: Io, allocator: Allocator, environ: *const std.process.Environ.Map) void {
    state.io = io;
    state.allocator = allocator;
    state.environ = environ;
}

// ── configuration ──────────────────────────────────────────────────────────

/// Read every `languages.figl` in the search order once. Errors in one file
/// are reported and the file skipped; a missing file is not an error.
pub fn load() void {
    if (state.loaded) return;
    state.loaded = true;
    const io = state.io orelse return;
    const a = state.allocator;
    const env = state.environ orelse return;

    if (env.get("FIG_LANGUAGES")) |path| loadFile(io, a, path);

    // `.fig/languages.figl` from the working directory upward.
    if (std.process.currentPathAlloc(io, a)) |cwd| {
        var dir: []const u8 = cwd;
        while (true) {
            const candidate = std.fmt.allocPrint(a, "{s}/.fig/languages.figl", .{dir}) catch break;
            loadFile(io, a, candidate);
            const parent = std.fs.path.dirname(dir) orelse break;
            if (parent.len == dir.len) break;
            dir = parent;
        }
    } else |_| {}

    const config_home = env.get("XDG_CONFIG_HOME");
    const user_path = if (config_home) |h|
        std.fmt.allocPrint(a, "{s}/fig/languages.figl", .{h}) catch return
    else if (env.get("HOME")) |home|
        std.fmt.allocPrint(a, "{s}/.config/fig/languages.figl", .{home}) catch return
    else
        return;
    loadFile(io, a, user_path);
}

fn loadFile(io: Io, a: Allocator, path: []const u8) void {
    const content = Io.Dir.cwd().readFileAlloc(io, path, a, .limited(1 << 20)) catch |err| switch (err) {
        error.FileNotFound => return,
        else => {
            log.warn("could not read {s}: {s}", .{ path, @errorName(err) });
            return;
        },
    };
    parseConfigFile(a, path, content) catch |err| {
        log.warn("could not parse {s}: {s}", .{ path, @errorName(err) });
    };
}

/// One `language[]` block per configured language. Read with fig's own
/// reader; a build without the fig format compiled in has no way to read
/// the file and says so.
fn parseConfigFile(a: Allocator, path: []const u8, content: []const u8) !void {
    if (comptime !build_options.lang_fig) {
        log.warn("{s}: this build has no fig-format reader, so the file cannot be read", .{path});
        return;
    }
    const Fig = fig.Language.FIG;
    var parser: Fig.Parser = .{ .allocator = a };
    const doc = try Fig.parse(&parser, content, Fig.default_type);
    const ast = &doc.ast;
    const seq = ast.getValByPath(&.{.{ .key = "language" }}) catch return error.NoLanguageList;
    if (seq.kind != .sequence) return error.NoLanguageList;
    var next = seq.kind.sequence;
    while (next) |id| {
        const item = ast.nodes[id];
        next = item.next_sibling;
        if (item.kind != .mapping) return error.MalformedLanguage;
        const name = stringField(ast, item, "name") orelse return error.MalformedLanguage;
        const command = try stringList(a, ast, item, "command") orelse return error.MalformedLanguage;
        if (command.len == 0) return error.MalformedLanguage;
        const extensions = try stringList(a, ast, item, "extensions") orelse &.{};
        if (findConfigured(name) != null) continue; // the earlier file wins
        try state.configured.append(a, .{
            .name = name,
            .extensions = extensions,
            .command = command,
            .source = path,
        });
    }
}

fn stringField(ast: *const fig.AST, mapping: fig.AST.Node, key: []const u8) ?[]const u8 {
    var next = mapping.kind.mapping;
    while (next) |id| {
        const kv = ast.nodes[id];
        next = kv.next_sibling;
        const k = ast.nodes[kv.kind.keyvalue.key];
        if (k.kind == .string and std.mem.eql(u8, k.kind.string, key)) {
            const v = ast.nodes[kv.kind.keyvalue.value];
            return switch (v.kind) {
                .string => |s| s,
                .number => |n| n.raw,
                else => null,
            };
        }
    }
    return null;
}

fn stringList(a: Allocator, ast: *const fig.AST, mapping: fig.AST.Node, key: []const u8) !?[]const []const u8 {
    var next = mapping.kind.mapping;
    while (next) |id| {
        const kv = ast.nodes[id];
        next = kv.next_sibling;
        const k = ast.nodes[kv.kind.keyvalue.key];
        if (k.kind == .string and std.mem.eql(u8, k.kind.string, key)) {
            const v = ast.nodes[kv.kind.keyvalue.value];
            var out: std.ArrayList([]const u8) = .empty;
            switch (v.kind) {
                .string => |s| try out.append(a, s),
                .sequence => |first| {
                    var item = first;
                    while (item) |iid| {
                        const n = ast.nodes[iid];
                        item = n.next_sibling;
                        switch (n.kind) {
                            .string => |s| try out.append(a, s),
                            .number => |num| try out.append(a, num.raw),
                            else => return error.MalformedLanguage,
                        }
                    }
                },
                else => return error.MalformedLanguage,
            }
            return try out.toOwnedSlice(a);
        }
    }
    return null;
}

fn findConfigured(name: []const u8) ?*Configured {
    for (state.configured.items) |*c| if (std.mem.eql(u8, c.name, name)) return c;
    return null;
}

/// Every configured language, loaded. For `fig lang list`.
pub fn configured() []Configured {
    load();
    return state.configured.items;
}

// ── resolution ─────────────────────────────────────────────────────────────

/// Record `--lang <name>`, resolved: the format every action reads and
/// writes its file in, whatever the extension says. Read by `args.zig`
/// where it would otherwise consult the extension.
pub fn setLangOverride(format: ?Format) void {
    state.lang_override = format;
}

pub fn langOverride() ?Format {
    return state.lang_override;
}

/// Whether some `languages.figl` names `name` at all — for the message when
/// `--lang` cannot be resolved: a name nobody configured is a different
/// mistake from a helper that was refused, which `ensure` has reported.
pub fn isConfigured(name: []const u8) bool {
    load();
    return findConfigured(name) != null;
}

/// The `Format` of the configured language named `name`, spawning and
/// registering its helper on first ask; null when no configured language
/// has the name, or when it was registered already under another
/// mechanism and the registry knows it.
pub fn resolveName(name: []const u8) ?Format {
    // Already in the registry — a name registered by whatever means.
    if (Runtime.entryByName(name)) |e| return types.runtimeFormat(e);
    load();
    const c = findConfigured(name) orelse return null;
    return ensureLogged(c);
}

/// The `Format` of the configured language owning `ext`, or null.
pub fn resolveExtension(ext: []const u8) ?Format {
    load();
    for (state.configured.items) |*c| {
        for (c.extensions) |x| {
            if (std.mem.eql(u8, x, ext)) return ensureLogged(c);
        }
    }
    return null;
}

/// Spawn and register `c` if it has not been, and hand back its `Format`.
/// A failure is remembered in `c.failure` and not retried; it is not
/// reported here — `fig lang list` prints it as a line of its own, and
/// the resolvers log it once, on the ask that met it.
pub fn ensure(c: *Configured) ?Format {
    if (c.entry) |e| return types.runtimeFormat(e);
    if (c.failure != null) return null;
    Runtime.last_refusal_len = 0;
    const e = register(c) catch |err| {
        // Whatever refused it wrote the reason, on either side of the
        // registry; an error that reached here without one is its own name.
        const reason = if (Runtime.lastRefusal().len > 0) Runtime.lastRefusal() else @errorName(err);
        c.failure = state.allocator.dupe(u8, reason) catch reason;
        return null;
    };
    c.entry = e;
    return types.runtimeFormat(e);
}

/// `ensure`, logging the failure the first time a resolver meets it.
fn ensureLogged(c: *Configured) ?Format {
    const already_failed = c.failure != null;
    const format = ensure(c);
    if (format == null and !already_failed) log.err("language `{s}` ({s}): {s}", .{ c.name, c.source, c.failure.? });
    return format;
}

// ── the helper runner ──────────────────────────────────────────────────────

/// One spawned helper: the process and its pipes, and the arena every
/// string the vtable points at lives in. Lives for the process; `ctx` of
/// the vtable.
const Helper = struct {
    io: Io,
    allocator: Allocator,
    name: []const u8,
    child: std.process.Child,
    reader: Io.File.Reader,
    writer: Io.File.Writer,
    read_buf: [64 * 1024]u8 = undefined,
    write_buf: [64 * 1024]u8 = undefined,
    /// Filled from `describe`: what the vtable's description pointers reach.
    arena: std.heap.ArenaAllocator,

    /// Send one request line and take one response line. The response is
    /// parsed into a `std.json.Value` tree in `out_arena`, which the caller
    /// owns.
    fn call(self: *Helper, out_arena: Allocator, request: []const u8) !std.json.Value {
        try self.writer.interface.writeAll(request);
        try self.writer.interface.writeByte('\n');
        try self.writer.interface.flush();
        var line: std.ArrayList(u8) = .empty;
        defer line.deinit(self.allocator);
        try readLine(&self.reader.interface, self.allocator, &line);
        if (line.items.len == 0) return error.HelperExited;
        const parsed = std.json.parseFromSliceLeaky(std.json.Value, out_arena, line.items, .{}) catch return error.HelperSpokeNoJson;
        return parsed;
    }
};

/// A whole line, however long: the reader's buffer bounds one take, not the
/// line.
fn readLine(r: *Io.Reader, allocator: Allocator, out: *std.ArrayList(u8)) !void {
    while (true) {
        const chunk = r.takeDelimiterInclusive('\n') catch |err| switch (err) {
            error.StreamTooLong => {
                const buf = r.buffered();
                try out.appendSlice(allocator, buf);
                r.toss(buf.len);
                continue;
            },
            error.EndOfStream => {
                const rest = r.buffered();
                try out.appendSlice(allocator, rest);
                r.toss(rest.len);
                return;
            },
            else => return err,
        };
        try out.appendSlice(allocator, chunk);
        if (out.items[out.items.len - 1] == '\n') out.items.len -= 1;
        return;
    }
}

const RegisterError = error{
    HelperRefused,
    HelperExited,
    HelperSpokeNoJson,
    HelperNameMismatch,
    InvalidLanguage,
    HarnessFailed,
    NameTaken,
    OutOfMemory,
    SpawnFailed,
};

/// Spawn `c.command`, ask it to describe itself, build the vtable and
/// register it.
fn register(c: *const Configured) RegisterError!*const Runtime.Entry {
    const io = state.io orelse return error.SpawnFailed;
    const a = state.allocator;
    const helper = a.create(Helper) catch return error.OutOfMemory;
    helper.* = .{
        .io = io,
        .allocator = a,
        .name = c.name,
        .child = undefined,
        .reader = undefined,
        .writer = undefined,
        .arena = std.heap.ArenaAllocator.init(a),
    };
    const argv = expandArgv(helper.arena.allocator(), c.command) catch return error.OutOfMemory;
    helper.child = std.process.spawn(io, .{
        .argv = argv,
        .stdin = .pipe,
        .stdout = .pipe,
        .stderr = .inherit,
    }) catch |err| {
        Runtime.refuse("could not start `{s}`: {s}", .{ argv[0], @errorName(err) });
        return error.HelperRefused;
    };
    helper.reader = helper.child.stdout.?.readerStreaming(io, &helper.read_buf);
    helper.writer = helper.child.stdin.?.writerStreaming(io, &helper.write_buf);

    // `describe`, into the helper's arena — the vtable points into it.
    const arena = helper.arena.allocator();
    const resp = helper.call(arena, "{\"op\":\"describe\"}") catch |err| {
        Runtime.refuse("the helper did not describe itself: {s}", .{@errorName(err)});
        return error.HelperRefused;
    };
    const desc = okField(resp, "description") orelse {
        Runtime.refuse("the helper refused to describe itself: {s}", .{failureMessage(resp)});
        return error.HelperRefused;
    };
    const vt = try vtableFromDescription(arena, helper, desc);
    if (!std.mem.eql(u8, std.mem.span(vt.name), c.name)) {
        Runtime.refuse("the helper describes itself as `{s}`, but languages.figl calls it `{s}`", .{ std.mem.span(vt.name), c.name });
        return error.HelperNameMismatch;
    }
    const abi = Runtime.register(a, &vt) catch |err| return switch (err) {
        error.InvalidLanguage => error.InvalidLanguage,
        error.HarnessFailed => error.HarnessFailed,
        error.NameTaken => error.NameTaken,
        else => error.OutOfMemory,
    };
    return Runtime.entryByAbi(abi).?;
}

/// `command` with a leading `~` in any argument expanded to `$HOME`.
fn expandArgv(a: Allocator, command: []const []const u8) ![]const []const u8 {
    const out = try a.alloc([]const u8, command.len);
    for (command, out) |arg, *o| {
        if (arg.len > 0 and arg[0] == '~') {
            if (state.environ.?.get("HOME")) |home| {
                o.* = try std.fmt.allocPrint(a, "{s}{s}", .{ home, arg[1..] });
                continue;
            }
        }
        o.* = arg;
    }
    return out;
}

fn okField(resp: std.json.Value, name: []const u8) ?std.json.Value {
    if (resp != .object) return null;
    const ok = resp.object.get("ok") orelse return null;
    if (ok != .bool or !ok.bool) return null;
    return resp.object.get(name);
}

fn failureMessage(resp: std.json.Value) []const u8 {
    if (resp != .object) return "malformed response";
    const m = resp.object.get("message") orelse return "no reason given";
    return if (m == .string) m.string else "no reason given";
}

fn failureOffset(resp: std.json.Value) usize {
    if (resp != .object) return 0;
    const m = resp.object.get("byte_offset") orelse return 0;
    return if (m == .integer and m.integer >= 0) @intCast(m.integer) else 0;
}

// ── description → vtable ───────────────────────────────────────────────────

fn zstr(a: Allocator, s: []const u8) ![*:0]const u8 {
    return (try a.dupeZ(u8, s)).ptr;
}

fn optString(a: Allocator, obj: std.json.ObjectMap, key: []const u8) !?[*:0]const u8 {
    const v = obj.get(key) orelse return null;
    return switch (v) {
        .string => |s| try zstr(a, s),
        else => null,
    };
}

fn boolOr(obj: std.json.ObjectMap, key: []const u8, default: bool) bool {
    const v = obj.get(key) orelse return default;
    return if (v == .bool) v.bool else default;
}

fn enumOrdinal(comptime E: type, obj: std.json.ObjectMap, key: []const u8, default: c_int) c_int {
    const v = obj.get(key) orelse return default;
    if (v != .string) return default;
    inline for (@typeInfo(E).@"enum".fields) |f| {
        if (std.mem.eql(u8, f.name, v.string)) return @intCast(f.value);
    }
    return default;
}

fn delimiterOf(a: Allocator, v: ?std.json.Value) !Runtime.CommentDelimiterDesc {
    const d = v orelse return .{};
    if (d != .object) return .{};
    return .{
        .open = try optString(a, d.object, "open"),
        .close = try optString(a, d.object, "close"),
        .forbidden = try optString(a, d.object, "forbidden"),
    };
}

fn syntaxOf(a: Allocator, v: std.json.Value) !*const Runtime.SyntaxDesc {
    if (v != .object) return error.HelperSpokeNoJson;
    const o = v.object;
    const s = try a.create(Runtime.SyntaxDesc);
    var comments: Runtime.CommentsDesc = .{ .style = 0, .line = .{}, .trailing = .{} };
    if (o.get("comments")) |c| if (c == .object) {
        comments.style = enumOrdinal(fig.Language.CommentStyle, c.object, "style", 0);
        comments.line = try delimiterOf(a, c.object.get("line"));
        comments.trailing = try delimiterOf(a, c.object.get("trailing"));
    };
    var closed: Runtime.ClosedContainersDesc = .{};
    if (o.get("closed_containers")) |c| if (c == .object) {
        closed = .{
            .map_open = try optString(a, c.object, "map_open"),
            .map_close = try optString(a, c.object, "map_close"),
            .seq_open = try optString(a, c.object, "seq_open"),
            .seq_close = try optString(a, c.object, "seq_close"),
        };
    };
    var header: Runtime.SectionHeaderDesc = .{};
    if (o.get("section_header")) |h| if (h == .object) {
        header = .{
            .open = try optString(a, h.object, "open"),
            .close = try optString(a, h.object, "close"),
            .seq_open = try optString(a, h.object, "seq_open"),
            .seq_close = try optString(a, h.object, "seq_close"),
            .sep = try optString(a, h.object, "sep"),
            .skip_index = boolOr(h.object, "skip_index", true),
        };
    };
    const sigil: u8 = if (o.get("key_sigil")) |k| (if (k == .integer and k.integer > 0 and k.integer < 256) @intCast(k.integer) else 0) else 0;
    s.* = .{
        .comments = comments,
        .kv_sep = try optString(a, o, "kv_sep"),
        .flow_kv_sep_from_siblings = boolOr(o, "flow_kv_sep_from_siblings", false),
        .flow_map_pad = try optString(a, o, "flow_map_pad"),
        .key_style = enumOrdinal(fig.Language.KeyStyle, o, "key_style", 0),
        .key_sigil = sigil,
        .empty_map_literal = try optString(a, o, "empty_map_literal"),
        .block_seq_editable = boolOr(o, "block_seq_editable", true),
        .flow_containers = boolOr(o, "flow_containers", true),
        .indent_unit = try optString(a, o, "indent_unit"),
        .seq_item_marker = try optString(a, o, "seq_item_marker"),
        .closed_containers = closed,
        .single_line_block_mapping = boolOr(o, "single_line_block_mapping", false),
        .bare_document_mapping = boolOr(o, "bare_document_mapping", true),
        .flow_map_open = try optString(a, o, "flow_map_open"),
        .flow_map_close = try optString(a, o, "flow_map_close"),
        .structural_indent = boolOr(o, "structural_indent", false),
        .section_noun = enumOrdinal(fig.Language.SectionNoun, o, "section_noun", -1),
        .section_header = header,
        .merge_key = try optString(a, o, "merge_key"),
    };
    return s;
}

fn vtableFromDescription(a: Allocator, helper: *Helper, desc: std.json.Value) !Runtime.VTable {
    if (desc != .object) return error.HelperSpokeNoJson;
    const o = desc.object;
    const name = o.get("name") orelse return error.HelperSpokeNoJson;
    if (name != .string) return error.HelperSpokeNoJson;

    var caps: u32 = 0;
    if (o.get("caps")) |c| if (c == .object) {
        if (boolOr(c.object, "read", false)) caps |= Runtime.cap_read;
        if (boolOr(c.object, "edit", false)) caps |= Runtime.cap_edit;
        if (boolOr(c.object, "serialize", false)) caps |= Runtime.cap_serialize;
    };
    // `null` (or absent) is unbounded; 0 is a flat format.
    const depth: c_int = if (o.get("max_mapping_depth")) |d| (if (d == .integer and d.integer >= 0 and d.integer < 256) @intCast(d.integer) else Runtime.no_depth_limit) else Runtime.no_depth_limit;

    var lossless: ?*const Runtime.NativeKindsDesc = null;
    if (o.get("lossless")) |l| if (l == .object) {
        const nk = try a.create(Runtime.NativeKindsDesc);
        nk.* = .{};
        inline for (@typeInfo(Runtime.NativeKindsDesc).@"struct".fields) |f| {
            @field(nk, f.name) = boolOr(l.object, f.name, false);
        }
        lossless = nk;
    };

    var syntax: ?*const Runtime.SyntaxDesc = null;
    if (o.get("syntax")) |sv| if (sv == .object) {
        syntax = try syntaxOf(a, sv);
    };

    var dialects: std.ArrayList(Runtime.DialectDesc) = .empty;
    if (o.get("dialects")) |dv| if (dv == .array) {
        for (dv.array.items) |d| {
            if (d != .object) continue;
            const dn = d.object.get("name") orelse continue;
            if (dn != .string) continue;
            var exts: std.ArrayList(?[*:0]const u8) = .empty;
            if (d.object.get("extensions")) |ev| if (ev == .array) {
                for (ev.array.items) |x| if (x == .string) try exts.append(a, try zstr(a, x.string));
            };
            try exts.append(a, null);
            const splice: c_int = if (d.object.get("splice")) |sp| (if (sp == .string) (if (std.mem.eql(u8, sp.string, "json_string")) 1 else if (std.mem.eql(u8, sp.string, "raw")) 2 else 0) else 0) else 0;
            try dialects.append(a, .{
                .name = try zstr(a, dn.string),
                .extensions = (try exts.toOwnedSlice(a)).ptr,
                .splice = splice,
                .empty_doc_seed = try optString(a, d.object, "empty_doc_seed"),
                .syntax = if (d.object.get("syntax")) |sv| (if (sv == .object) try syntaxOf(a, sv) else null) else null,
            });
        }
    };

    var samples: std.ArrayList(Runtime.Str) = .empty;
    if (o.get("samples")) |sv| if (sv == .array) {
        for (sv.array.items) |x| if (x == .string) try samples.append(a, Runtime.Str.of(try a.dupe(u8, x.string)));
    };

    var renderers: [5]bool = .{ false, false, false, false, false };
    if (o.get("renderers")) |rv| if (rv == .array) {
        for (rv.array.items) |x| if (x == .string) {
            inline for ([_][]const u8{ "value", "entry", "item", "tail", "key" }, 0..) |n, i| {
                if (std.mem.eql(u8, x.string, n)) renderers[i] = true;
            }
        };
    };

    const dialect_slice = try dialects.toOwnedSlice(a);
    const sample_slice = try samples.toOwnedSlice(a);
    return .{
        .version = Runtime.vtable_version,
        .ctx = helper,
        .name = try zstr(a, name.string),
        .caps = caps,
        .max_mapping_depth = depth,
        .lossless = lossless,
        .syntax = syntax,
        .dialects = dialect_slice.ptr,
        .dialect_count = dialect_slice.len,
        .samples = sample_slice.ptr,
        .sample_count = sample_slice.len,
        .parse = parseThunk,
        .print = if (caps & Runtime.cap_serialize != 0) printThunk else null,
        .free_table = freeTableThunk,
        .free_bytes = freeBytesThunk,
        .render_value = if (renderers[0]) renderValueThunk else null,
        .render_entry = if (renderers[1]) renderEntryThunk else null,
        .render_item = if (renderers[2]) renderItemThunk else null,
        .render_tail = if (renderers[3]) renderTailThunk else null,
        .render_key = if (renderers[4]) renderKeyThunk else null,
    };
}

// ── the wire, per call ─────────────────────────────────────────────────────

fn helperOf(ctx: ?*anyopaque) *Helper {
    return @ptrCast(@alignCast(ctx.?));
}

fn setErr(err: *Runtime.ErrorInfo, message: []const u8, offset: usize) void {
    err.set(message);
    err.byte_offset = offset;
}

/// Write a JSON string literal.
fn jsonString(w: *Io.Writer, s: []const u8) !void {
    try std.json.Stringify.encodeJsonString(s, .{}, w);
}

/// What a parse's memory is: the arena its rows and strings were decoded
/// into. `NodeTable.owner`.
const TableOwner = struct { arena: std.heap.ArenaAllocator };

fn parseThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, input: Runtime.Str, out: *Runtime.NodeTable, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    const h = helperOf(ctx);
    const src = input.slice() orelse "";
    if (!std.unicode.utf8ValidateSlice(src)) {
        setErr(err, "the input is not UTF-8, which the helper wire carries as text", 0);
        return 2;
    }
    var req: Io.Writer.Allocating = .init(h.allocator);
    defer req.deinit();
    const w = &req.writer;
    w.writeAll("{\"op\":\"parse\",\"dialect\":") catch return 3;
    jsonString(w, std.mem.span(dialect)) catch return 3;
    w.writeAll(",\"input\":") catch return 3;
    jsonString(w, src) catch return 3;
    w.writeAll("}") catch return 3;

    const owner = h.allocator.create(TableOwner) catch return 3;
    owner.* = .{ .arena = std.heap.ArenaAllocator.init(h.allocator) };
    const a = owner.arena.allocator();
    const resp = h.call(a, req.written()) catch |e| {
        owner.arena.deinit();
        h.allocator.destroy(owner);
        setErr(err, @errorName(e), 0);
        return 2;
    };
    const table_v = okField(resp, "table") orelse {
        setErr(err, failureMessage(resp), failureOffset(resp));
        owner.arena.deinit();
        h.allocator.destroy(owner);
        return 2;
    };
    const table = tableFromJson(a, table_v) catch |e| {
        setErr(err, switch (e) {
            error.MalformedTable => "the helper returned a malformed table",
            else => @errorName(e),
        }, 0);
        owner.arena.deinit();
        h.allocator.destroy(owner);
        return 2;
    };
    out.* = table;
    out.owner = owner;
    return 0;
}

fn freeTableThunk(ctx: ?*anyopaque, table: *Runtime.NodeTable) callconv(.c) void {
    const h = helperOf(ctx);
    const owner: *TableOwner = @ptrCast(@alignCast(table.owner orelse return));
    owner.arena.deinit();
    h.allocator.destroy(owner);
}

fn freeBytesThunk(ctx: ?*anyopaque, bytes: Runtime.Str) callconv(.c) void {
    const h = helperOf(ctx);
    if (bytes.slice()) |s| if (s.len > 0) h.allocator.free(s);
}

fn printThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, table: *const Runtime.NodeTable, options: *const Runtime.PrintOptions, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    const h = helperOf(ctx);
    var req: Io.Writer.Allocating = .init(h.allocator);
    defer req.deinit();
    const w = &req.writer;
    w.writeAll("{\"op\":\"print\",\"dialect\":") catch return 3;
    jsonString(w, std.mem.span(dialect)) catch return 3;
    w.writeAll(",\"table\":") catch return 3;
    tableToJson(w, table) catch return 3;
    w.print(",\"options\":{{\"pretty\":{},\"strip_comments\":{},\"indent\":{d},\"width\":{d}}}}}", .{ options.pretty, options.strip_comments, options.indent, options.width }) catch return 3;
    return outputCall(h, req.written(), out, err);
}

/// Send `request` and hand back its `output` string.
fn outputCall(h: *Helper, request: []const u8, out: *Runtime.Str, err: *Runtime.ErrorInfo) c_int {
    var arena = std.heap.ArenaAllocator.init(h.allocator);
    defer arena.deinit();
    const resp = h.call(arena.allocator(), request) catch |e| {
        setErr(err, @errorName(e), 0);
        return 4;
    };
    const output = okField(resp, "output") orelse {
        setErr(err, failureMessage(resp), 0);
        return 4;
    };
    if (output != .string) {
        setErr(err, "the helper's output is not a string", 0);
        return 4;
    }
    const copy = h.allocator.dupe(u8, output.string) catch return 3;
    out.* = Runtime.Str.of(copy);
    return 0;
}

fn renderCall(ctx: ?*anyopaque, which: []const u8, dialect: [*:0]const u8, indent: Runtime.Str, key: Runtime.Str, value: Runtime.Str, literal: ?[*:0]const u8, old_key: Runtime.Str, out: *Runtime.Str, err: *Runtime.ErrorInfo) c_int {
    const h = helperOf(ctx);
    var req: Io.Writer.Allocating = .init(h.allocator);
    defer req.deinit();
    const w = &req.writer;
    w.print("{{\"op\":\"render\",\"which\":\"{s}\",\"dialect\":", .{which}) catch return 3;
    jsonString(w, std.mem.span(dialect)) catch return 3;
    inline for (.{ .{ "indent", indent }, .{ "key", key }, .{ "value", value } }) |pair| {
        w.print(",\"{s}\":", .{pair[0]}) catch return 3;
        jsonString(w, pair[1].slice() orelse "") catch return 3;
    }
    // What fig made of the value, for the value renderer alone.
    if (literal) |l| {
        w.writeAll(",\"literal\":") catch return 3;
        jsonString(w, std.mem.span(l)) catch return 3;
    }
    w.writeAll(",\"old_key\":") catch return 3;
    jsonString(w, old_key.slice() orelse "") catch return 3;
    w.writeAll("}") catch return 3;
    return outputCall(h, req.written(), out, err);
}

fn renderValueThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, value: Runtime.Str, literal: [*:0]const u8, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "value", dialect, .{}, .{}, value, literal, .{}, out, err);
}
fn renderEntryThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Runtime.Str, key: Runtime.Str, value: Runtime.Str, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "entry", dialect, indent, key, value, null, .{}, out, err);
}
fn renderItemThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Runtime.Str, value: Runtime.Str, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "item", dialect, indent, .{}, value, null, .{}, out, err);
}
fn renderTailThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Runtime.Str, key: Runtime.Str, value: Runtime.Str, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "tail", dialect, indent, key, value, null, .{}, out, err);
}
fn renderKeyThunk(ctx: ?*anyopaque, dialect: [*:0]const u8, indent: Runtime.Str, key: Runtime.Str, old_key: Runtime.Str, out: *Runtime.Str, err: *Runtime.ErrorInfo) callconv(.c) c_int {
    return renderCall(ctx, "key", dialect, indent, key, .{}, null, old_key, out, err);
}

// ── the table on the wire ──────────────────────────────────────────────────

const kind_names = [_][]const u8{ "null", "bool", "int", "float", "string", "sequence", "mapping", "keyvalue", "alias" };

fn kindOf(name: []const u8) ?c_int {
    for (kind_names, 0..) |k, i| if (std.mem.eql(u8, k, name)) return @intCast(i);
    return null;
}

const ExtKind = fig.AST.Node.Kind.Extended.ExtKind;

fn extKindOf(name: []const u8) ?c_int {
    inline for (@typeInfo(ExtKind).@"enum".fields) |f| {
        if (std.mem.eql(u8, f.name, name)) return @intCast(f.value);
    }
    return null;
}

fn spanOf(v: ?std.json.Value) !Runtime.CSpan {
    const s = v orelse return .none;
    if (s == .null) return .none;
    if (s != .array or s.array.items.len != 2) return error.MalformedTable;
    const a = s.array.items[0];
    const b = s.array.items[1];
    if (a != .integer or b != .integer or a.integer < 0 or b.integer < 0) return error.MalformedTable;
    return .{ .start = @intCast(a.integer), .end = @intCast(b.integer) };
}

fn strOf(a: Allocator, v: ?std.json.Value) !Runtime.Str {
    const s = v orelse return .none;
    return switch (s) {
        .null => .none,
        .string => |x| Runtime.Str.of(try a.dupe(u8, x)),
        else => error.MalformedTable,
    };
}

fn u32Of(v: ?std.json.Value) !u32 {
    const s = v orelse return error.MalformedTable;
    if (s != .integer or s.integer < 0 or s.integer > std.math.maxInt(u32)) return error.MalformedTable;
    return @intCast(s.integer);
}

fn usizeOf(v: ?std.json.Value) !usize {
    const s = v orelse return error.MalformedTable;
    if (s != .integer or s.integer < 0) return error.MalformedTable;
    return @intCast(s.integer);
}

fn tableFromJson(a: Allocator, v: std.json.Value) !Runtime.NodeTable {
    if (v != .object) return error.MalformedTable;
    const o = v.object;
    var rows: std.ArrayList(Runtime.NodeRow) = .empty;
    if (o.get("rows")) |rv| {
        if (rv != .array) return error.MalformedTable;
        for (rv.array.items) |r| {
            if (r != .object) return error.MalformedTable;
            const ro = r.object;
            const kind_v = ro.get("kind") orelse return error.MalformedTable;
            if (kind_v != .string) return error.MalformedTable;
            const kind = kindOf(kind_v.string) orelse return error.MalformedTable;
            const ext: c_int = if (ro.get("ext_kind")) |e| (if (e == .string) (extKindOf(e.string) orelse return error.MalformedTable) else Runtime.no_ext_kind) else Runtime.no_ext_kind;
            const parent: u32 = if (ro.get("parent")) |p| (if (p == .null) Runtime.no_node else try u32Of(p)) else Runtime.no_node;
            try rows.append(a, .{
                .kind = kind,
                .ext_kind = ext,
                .parent = parent,
                .span = try spanOf(ro.get("span")),
                .text = try strOf(a, ro.get("text")),
                .anchor = try strOf(a, ro.get("anchor")),
                .anchor_span = try spanOf(ro.get("anchor_span")),
                .tag = try strOf(a, ro.get("tag")),
                .tag_span = try spanOf(ro.get("tag_span")),
                .marker = try spanOf(ro.get("marker")),
                .sep = try spanOf(ro.get("sep")),
            });
        }
    }
    var regions: std.ArrayList(Runtime.RegionRow) = .empty;
    if (o.get("regions")) |rv| if (rv == .array) {
        for (rv.array.items) |r| {
            if (r != .object) return error.MalformedTable;
            try regions.append(a, .{ .node = try u32Of(r.object.get("node")), .start = try usizeOf(r.object.get("start")), .end = try usizeOf(r.object.get("end")) });
        }
    };
    var mentions: std.ArrayList(Runtime.MentionRow) = .empty;
    if (o.get("mentions")) |mv| if (mv == .array) {
        for (mv.array.items) |m| {
            if (m != .object) return error.MalformedTable;
            const k = m.object.get("kind") orelse return error.MalformedTable;
            try mentions.append(a, .{
                .node = try u32Of(m.object.get("node")),
                .span = try spanOf(m.object.get("span")),
                .kind = if (k == .string and std.mem.eql(u8, k.string, "entry")) Runtime.mention_entry else Runtime.mention_header,
            });
        }
    };
    var comments: std.ArrayList(Runtime.CommentRow) = .empty;
    if (o.get("comments")) |cv| if (cv == .array) {
        for (cv.array.items) |c| {
            if (c != .object) return error.MalformedTable;
            const slot_v = c.object.get("slot") orelse return error.MalformedTable;
            const style_v = c.object.get("style") orelse return error.MalformedTable;
            if (slot_v != .string or style_v != .string) return error.MalformedTable;
            const slot: c_int = if (std.mem.eql(u8, slot_v.string, "trailing")) Runtime.comment_trailing else if (std.mem.eql(u8, slot_v.string, "dangling")) Runtime.comment_dangling else Runtime.comment_leading;
            const style: c_int = if (std.mem.eql(u8, style_v.string, "block")) 1 else 0;
            const text = try strOf(a, c.object.get("text"));
            try comments.append(a, .{ .node = try u32Of(c.object.get("node")), .slot = slot, .style = style, .text = if (text.slice() == null) Runtime.Str.of("") else text });
        }
    };
    const row_slice = try rows.toOwnedSlice(a);
    const region_slice = try regions.toOwnedSlice(a);
    const mention_slice = try mentions.toOwnedSlice(a);
    const comment_slice = try comments.toOwnedSlice(a);
    return .{
        .rows = row_slice.ptr,
        .row_count = row_slice.len,
        .regions = region_slice.ptr,
        .region_count = region_slice.len,
        .mentions = mention_slice.ptr,
        .mention_count = mention_slice.len,
        .comments = comment_slice.ptr,
        .comment_count = comment_slice.len,
    };
}

fn writeSpan(w: *Io.Writer, key: []const u8, s: Runtime.CSpan) !void {
    if (s.span()) |sp| try w.print(",\"{s}\":[{d},{d}]", .{ key, sp.start, sp.end });
}

fn writeStr(w: *Io.Writer, key: []const u8, s: Runtime.Str) !void {
    if (s.slice()) |x| {
        try w.print(",\"{s}\":", .{key});
        try jsonString(w, x);
    }
}

pub fn tableToJson(w: *Io.Writer, t: *const Runtime.NodeTable) !void {
    try w.writeAll("{\"rows\":[");
    for (t.rowSlice(), 0..) |r, i| {
        if (i > 0) try w.writeByte(',');
        const kind: usize = @intCast(r.kind);
        try w.print("{{\"kind\":\"{s}\"", .{if (kind < kind_names.len) kind_names[kind] else "null"});
        if (r.ext_kind != Runtime.no_ext_kind) {
            const ek: ExtKind = @enumFromInt(r.ext_kind);
            try w.print(",\"ext_kind\":\"{s}\"", .{@tagName(ek)});
        }
        if (r.parent == Runtime.no_node) try w.writeAll(",\"parent\":null") else try w.print(",\"parent\":{d}", .{r.parent});
        try writeSpan(w, "span", r.span);
        try writeStr(w, "text", r.text);
        try writeStr(w, "anchor", r.anchor);
        try writeSpan(w, "anchor_span", r.anchor_span);
        try writeStr(w, "tag", r.tag);
        try writeSpan(w, "tag_span", r.tag_span);
        try writeSpan(w, "marker", r.marker);
        try writeSpan(w, "sep", r.sep);
        try w.writeByte('}');
    }
    try w.writeAll("],\"regions\":[");
    for (t.regionSlice(), 0..) |r, i| {
        if (i > 0) try w.writeByte(',');
        try w.print("{{\"node\":{d},\"start\":{d},\"end\":{d}}}", .{ r.node, r.start, r.end });
    }
    try w.writeAll("],\"mentions\":[");
    for (t.mentionSlice(), 0..) |m, i| {
        if (i > 0) try w.writeByte(',');
        const sp = m.span.span() orelse fig.Span.init(0, 0);
        try w.print("{{\"node\":{d},\"span\":[{d},{d}],\"kind\":\"{s}\"}}", .{ m.node, sp.start, sp.end, if (m.kind == Runtime.mention_entry) "entry" else "header" });
    }
    try w.writeAll("],\"comments\":[");
    for (t.commentSlice(), 0..) |c, i| {
        if (i > 0) try w.writeByte(',');
        const slot: []const u8 = switch (c.slot) {
            Runtime.comment_trailing => "trailing",
            Runtime.comment_dangling => "dangling",
            else => "leading",
        };
        try w.print("{{\"node\":{d},\"slot\":\"{s}\",\"style\":\"{s}\",\"text\":", .{ c.node, slot, if (c.style == 1) "block" else "line" });
        try jsonString(w, c.text.slice() orelse "");
        try w.writeByte('}');
    }
    try w.writeAll("]}");
}

// ── `fig lang` ─────────────────────────────────────────────────────────────

/// `fig lang list`: every compiled format and every configured language,
/// with what each can do. A configured language is spawned to be asked.
pub fn list(io: Io, a: Allocator, out: *Io.Terminal) !void {
    _ = io;
    _ = a;
    try out.writer.writeAll("compiled:\n");
    inline for (fig.Language.dialects) |d| {
        const caps = if (comptime d.Lang == void) "compiled out" else capsWord(d.Lang.caps);
        try out.writer.print("  {s:<14} {s}\n", .{ d.name, caps });
    }
    const langs = configured();
    if (langs.len == 0) {
        try out.writer.writeAll("configured: none (no languages.figl found)\n");
    } else {
        try out.writer.writeAll("configured:\n");
        for (langs) |*c| {
            if (ensure(c)) |_| {
                const e = c.entry.?;
                try out.writer.print("  {s:<14} {s}", .{ c.name, capsWord(e.language.caps) });
                if (c.extensions.len > 0) {
                    try out.writer.writeAll("  .");
                    for (c.extensions, 0..) |x, i| {
                        if (i > 0) try out.writer.writeAll(" .");
                        try out.writer.writeAll(x);
                    }
                }
                try out.writer.print("  ({s})\n", .{c.source});
            } else {
                try out.writer.print("  {s:<14} refused: {s}  ({s})\n", .{ c.name, c.failure orelse "?", c.source });
            }
        }
    }
    try out.writer.flush();
}

fn capsWord(caps: fig.Language.Caps) []const u8 {
    if (caps.read and caps.edit and caps.serialize) return "read edit serialize";
    if (caps.read and caps.serialize) return "read serialize";
    if (caps.read and caps.edit) return "read edit";
    if (caps.read) return "read";
    return "none";
}

/// `fig lang check <name> [--against <compiled>] [files…]`: load the
/// language — which is the harness over its samples — and then, given a
/// compiled format to hold it to, parse each file (and each of the
/// language's samples) with both and compare the tables row for row.
/// Exits nonzero on the first difference.
pub fn check(io: Io, a: Allocator, out: *Io.Terminal, err_term: *Io.Terminal, name: []const u8, against: ?[]const u8, files: []const []const u8) !void {
    // Resolve without the resolver's log line: a refusal is `check`'s own
    // finding, and it reports it as one.
    load();
    const format = if (Runtime.entryByName(name)) |e|
        types.runtimeFormat(e)
    else if (findConfigured(name)) |c|
        ensure(c) orelse {
            try out.writer.print("{s}: refused: {s}\n", .{ name, c.failure.? });
            try out.writer.flush();
            std.process.exit(1);
        }
    else {
        try err_term.writer.print("error: no language named `{s}` is configured (see `fig lang --help`)\n", .{name});
        try err_term.writer.flush();
        std.process.exit(2);
    };
    const e = types.runtimeEntry(format).?;
    try out.writer.print("{s}: registered ({s}); every sample parsed", .{ e.name, capsWord(e.language.caps) });
    if (e.language.caps.serialize) try out.writer.writeAll(", printed and reparsed to the same tree");
    if (e.language.caps.edit) try out.writer.writeAll(", and took a no-op edit");
    try out.writer.writeAll("\n");

    if (against) |sibling_name| {
        const sibling = std.meta.stringToEnum(Format, sibling_name) orelse {
            try err_term.writer.print("error: `--against` names a compiled format; `{s}` is not one\n", .{sibling_name});
            try err_term.writer.flush();
            std.process.exit(2);
        };
        var inputs: std.ArrayList(struct { label: []const u8, content: []const u8 }) = .empty;
        for (e.language.samples, 0..) |s, i| {
            try inputs.append(a, .{ .label = try std.fmt.allocPrint(a, "sample {d}", .{i + 1}), .content = s });
        }
        for (files) |f| {
            const content = Io.Dir.cwd().readFileAlloc(io, f, a, .limited(64 << 20)) catch |rerr| {
                try err_term.writer.print("error: could not read {s}: {s}\n", .{ f, @errorName(rerr) });
                try err_term.writer.flush();
                std.process.exit(2);
            };
            try inputs.append(a, .{ .label = f, .content = content });
        }
        const parse_dispatch = @import("parse_dispatch.zig");
        for (inputs.items) |in| {
            var reports: parse_dispatch.Reports = .{};
            const mine = parse_dispatch.parseSliceAs(format, .{}, a, in.content, false, &reports) catch |perr| {
                try err_term.writer.print("{s}: `{s}` does not parse it: {s}\n", .{ in.label, e.name, if (reports.runtime) |d| d.message else @errorName(perr) });
                try err_term.writer.flush();
                std.process.exit(1);
            };
            var sib_reports: parse_dispatch.Reports = .{};
            const theirs = parse_dispatch.parseSliceAs(sibling, .{}, a, in.content, false, &sib_reports) catch |perr| {
                try err_term.writer.print("{s}: `{s}` does not parse it: {s}\n", .{ in.label, sibling_name, @errorName(perr) });
                try err_term.writer.flush();
                std.process.exit(1);
            };
            if (try diffDocuments(a, err_term, in.label, e.name, sibling_name, mine, theirs)) {
                try err_term.writer.flush();
                std.process.exit(1);
            }
            try out.writer.print("{s}: same table as `{s}` ({d} rows)\n", .{ in.label, sibling_name, mine.ast.nodes.len });
        }
    }
    try out.writer.flush();
}

/// `fig lang table <file> [-i <format>]`: the file's node table as the
/// JSON a helper answers `parse` with. What an implementor of a twin reads
/// to see what the compiled format produces, and what `check --against`
/// then holds the twin to.
pub fn printTable(io: Io, a: Allocator, out: *Io.Terminal, err_term: *Io.Terminal, file: []const u8, input: ?Format) !void {
    const args = @import("args.zig");
    const parse_dispatch = @import("parse_dispatch.zig");
    const fileio = @import("fileio.zig");
    const handle = try fileio.getInput(io, file, .read_only);
    defer if (!std.mem.eql(u8, file, "-")) handle.close(io);
    const content = try fileio.readAll(a, io, handle);
    const format = input orelse
        (if (args.detectLanguageFromFileEnding(file)) |d| d.format else try parse_dispatch.resolveFormatFromContent(a, content, file));
    var reports: parse_dispatch.Reports = .{};
    const doc = parse_dispatch.parseSliceAs(format, .{}, a, content, false, &reports) catch |err| {
        try reports.reportDiagnostics(err_term, content, file);
        return err;
    };
    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();
    const t = try Runtime.fullTable(arena.allocator(), &doc);
    try tableToJson(out.writer, &t.table);
    try out.writer.writeAll("\n");
    try out.writer.flush();
}

/// Compare two documents of the same source as the tables a helper would
/// produce for them — `Runtime.fullTable` of each, which is exactly what
/// `fig lang table` prints — row for row: kind, parent, text, span,
/// marker, separator, anchor, tag, then the regions, mentions and
/// comments. Reports the first difference and returns true on one.
fn diffDocuments(a: Allocator, term: *Io.Terminal, label: []const u8, mine_name: []const u8, theirs_name: []const u8, mine: fig.Document, theirs: fig.Document) !bool {
    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();
    const x = (try Runtime.fullTable(arena.allocator(), &mine)).table;
    const y = (try Runtime.fullTable(arena.allocator(), &theirs)).table;
    const xr = x.rowSlice();
    const yr = y.rowSlice();
    if (xr.len != yr.len) {
        try term.writer.print("{s}: `{s}` produces {d} rows, `{s}` {d}\n", .{ label, mine_name, xr.len, theirs_name, yr.len });
        return true;
    }
    for (xr, yr, 0..) |p, q, i| {
        if (p.kind != q.kind or p.ext_kind != q.ext_kind or p.parent != q.parent) {
            try term.writer.print("{s}: row {d} differs in kind or parent ({s}: kind {d} parent {d}; {s}: kind {d} parent {d})\n", .{ label, i, mine_name, p.kind, p.parent, theirs_name, q.kind, q.parent });
            return true;
        }
        const texts = .{
            .{ "text", p.text, q.text },
            .{ "anchor", p.anchor, q.anchor },
            .{ "tag", p.tag, q.tag },
        };
        inline for (texts) |col| {
            if (!optStrEql(col[1].slice(), col[2].slice())) {
                try term.writer.print("{s}: row {d} {s} differs ({s}: {s}; {s}: {s})\n", .{ label, i, col[0], mine_name, col[1].slice() orelse "none", theirs_name, col[2].slice() orelse "none" });
                return true;
            }
        }
        const spans = .{
            .{ "span", p.span, q.span },
            .{ "item marker", p.marker, q.marker },
            .{ "separator", p.sep, q.sep },
            .{ "anchor span", p.anchor_span, q.anchor_span },
            .{ "tag span", p.tag_span, q.tag_span },
        };
        inline for (spans) |col| {
            if (!optSpanEql(col[1].span(), col[2].span())) {
                try term.writer.print("{s}: row {d} {s} differs ({s}: {f}; {s}: {f})\n", .{ label, i, col[0], mine_name, fmtSpan(col[1].span()), theirs_name, fmtSpan(col[2].span()) });
                return true;
            }
        }
    }
    const xg = x.regionSlice();
    const yg = y.regionSlice();
    if (xg.len != yg.len) {
        try term.writer.print("{s}: `{s}` records {d} header lines, `{s}` {d}\n", .{ label, mine_name, xg.len, theirs_name, yg.len });
        return true;
    }
    for (xg, yg, 0..) |r1, r2, i| if (r1.node != r2.node or r1.start != r2.start or r1.end != r2.end) {
        try term.writer.print("{s}: header line {d} differs ({s}: row {d} [{d},{d}); {s}: row {d} [{d},{d}))\n", .{ label, i, mine_name, r1.node, r1.start, r1.end, theirs_name, r2.node, r2.start, r2.end });
        return true;
    };
    const xm = x.mentionSlice();
    const ym = y.mentionSlice();
    if (xm.len != ym.len) {
        try term.writer.print("{s}: `{s}` records {d} name mentions, `{s}` {d}\n", .{ label, mine_name, xm.len, theirs_name, ym.len });
        return true;
    }
    for (xm, ym, 0..) |m1, m2, i| if (m1.node != m2.node or m1.kind != m2.kind or !optSpanEql(m1.span.span(), m2.span.span())) {
        try term.writer.print("{s}: name mention {d} differs ({s}: row {d} {f}; {s}: row {d} {f})\n", .{ label, i, mine_name, m1.node, fmtSpan(m1.span.span()), theirs_name, m2.node, fmtSpan(m2.span.span()) });
        return true;
    };
    const xc = x.commentSlice();
    const yc = y.commentSlice();
    if (xc.len != yc.len) {
        try term.writer.print("{s}: `{s}` records {d} comments, `{s}` {d}\n", .{ label, mine_name, xc.len, theirs_name, yc.len });
        return true;
    }
    for (xc, yc, 0..) |c1, c2, i| if (c1.node != c2.node or c1.slot != c2.slot or c1.style != c2.style or !optStrEql(c1.text.slice(), c2.text.slice())) {
        try term.writer.print("{s}: comment {d} differs ({s}: row {d} slot {d} `{s}`; {s}: row {d} slot {d} `{s}`)\n", .{ label, i, mine_name, c1.node, c1.slot, c1.text.slice() orelse "", theirs_name, c2.node, c2.slot, c2.text.slice() orelse "" });
        return true;
    };
    return false;
}

/// An optional span as `[s,e)` or `none`, for a difference report.
fn fmtSpan(span: ?fig.Span) std.fmt.Alt(?fig.Span, formatSpan) {
    return .{ .data = span };
}

fn formatSpan(span: ?fig.Span, w: *Io.Writer) Io.Writer.Error!void {
    if (span) |s| try w.print("[{d},{d})", .{ s.start, s.end }) else try w.writeAll("none");
}

fn optSpanEql(a: ?fig.Span, b: ?fig.Span) bool {
    if (a == null and b == null) return true;
    if (a == null or b == null) return false;
    return a.?.eql(b.?);
}

fn optStrEql(a: ?[]const u8, b: ?[]const u8) bool {
    if (a == null and b == null) return true;
    if (a == null or b == null) return false;
    return std.mem.eql(u8, a.?, b.?);
}

// ── tests ──────────────────────────────────────────────────────────────────

/// Empty the process-wide state between tests; nothing is spawned by these.
fn resetForTest(a: Allocator) void {
    state.configured = .empty;
    state.loaded = true;
    state.allocator = a;
    state.lang_override = null;
}

test "languages.figl: one language[] block per language; an earlier file wins a name" {
    if (comptime !build_options.lang_fig) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    resetForTest(a);

    try parseConfigFile(a, "first.figl",
        \\language[]
        \\> name = lua-dotenv
        \\> extensions = [env, dotenv]
        \\> command = [fig-lua, ~/.config/fig/languages/dotenv.lua]
        \\language[]
        \\> name = tinykv
        \\> command = tinykv_helper
        \\
    );
    try parseConfigFile(a, "second.figl",
        \\language[]
        \\> name = tinykv
        \\> command = [other]
        \\language[]
        \\> name = hcl
        \\> extensions = [hcl, tf]
        \\> command = [fig-hcl]
        \\
    );
    const langs = state.configured.items;
    try t.expectEqual(@as(usize, 3), langs.len);
    try t.expectEqualStrings("lua-dotenv", langs[0].name);
    try t.expectEqual(@as(usize, 2), langs[0].extensions.len);
    try t.expectEqualStrings("dotenv", langs[0].extensions[1]);
    try t.expectEqualStrings("~/.config/fig/languages/dotenv.lua", langs[0].command[1]);
    // A bare string is a one-word command; the first file's `tinykv` kept
    // its command and its source.
    try t.expectEqualStrings("tinykv_helper", langs[1].command[0]);
    try t.expectEqualStrings("first.figl", langs[1].source);
    try t.expectEqualStrings("hcl", langs[2].name);
    try t.expectEqualStrings("second.figl", langs[2].source);
    try t.expect(isConfigured("hcl"));
    try t.expect(!isConfigured("nosuch"));
    try t.expect(findConfigured("lua-dotenv") != null);
}

test "languages.figl: a block without a name or a command is refused whole" {
    if (comptime !build_options.lang_fig) return error.SkipZigTest;
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    resetForTest(a);
    try t.expectError(error.MalformedLanguage, parseConfigFile(a, "x.figl", "language[]\n> name = nameless-command\n"));
    try t.expectError(error.MalformedLanguage, parseConfigFile(a, "x.figl", "language[]\n> command = [x]\n"));
    try t.expectError(error.NoLanguageList, parseConfigFile(a, "x.figl", "other = 1\n"));
}

test "expandArgv: a leading ~ in any argument is $HOME" {
    const t = std.testing;
    var arena = std.heap.ArenaAllocator.init(t.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var env = std.process.Environ.Map.init(a);
    try env.put("HOME", "/home/adam");
    state.environ = &env;
    defer state.environ = null;
    const argv = try expandArgv(a, &.{ "fig-lua", "~/.config/fig/languages/dotenv.lua", "not~here" });
    try t.expectEqualStrings("fig-lua", argv[0]);
    try t.expectEqualStrings("/home/adam/.config/fig/languages/dotenv.lua", argv[1]);
    try t.expectEqualStrings("not~here", argv[2]);
}
