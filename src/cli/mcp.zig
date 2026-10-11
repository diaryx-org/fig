//! `fig mcp` — fig's editor, served to a language model over MCP.
//!
//! JSON-RPC on stdin and stdout, one message per line, and nothing else on
//! stdout. The protocol is MCP's stateless revision of 2026-07-28: no
//! handshake, `server/discover` says what the server speaks, and every
//! request opens its file, does one thing and lets it go. An `initialize`
//! from a client of an earlier revision is answered too, and served the
//! same way.
//!
//! What a model gets that a text-editing tool does not give it:
//!
//!   * **Paths, not quotes.** An edit names `service.ports[1]`, so it never
//!     has to quote a passage that occurs once, match its whitespace, or
//!     read the whole file to find one. `outline` prints the paths `apply`
//!     takes, in the CLI's own path syntax (`args.parsePath`).
//!   * **The editor's guarantees.** Every op is `fig set`/`insert`/`delete`/
//!     `rename`/`comment`/`patch`'s own splice (`edit_ops`, `patch_ops`):
//!     bytes outside the edit are untouched, values are written in the
//!     file's spelling, and a result that does not parse is refused.
//!   * **One write per call.** `apply` makes every edit in memory and
//!     writes once, or writes nothing and says which edit was refused.
//!   * **A version to edit against.** `outline` and `get` return a hash of
//!     the file; `apply` with `base` refuses if the file has changed since.
//!     The hash is the state the protocol wants passed back as an argument,
//!     rather than held in this process.
//!
//! Values arrive as JSON, the one notation every client already writes, and
//! are rendered into the file's format the way `patch` renders a document.
//! `raw` is the escape for what only the target can spell (a YAML anchor, a
//! TOML local datetime), as `--raw` is on the command line.
const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");

const types = @import("types.zig");
const args_mod = @import("args.zig");
const help = @import("help.zig");
const fileio = @import("fileio.zig");
const parse_dispatch = @import("parse_dispatch.zig");
const edit_ops = @import("edit_ops.zig");
const patch_ops = @import("patch_ops.zig");
const value_arg = @import("value_arg.zig");
const diag_report = @import("diag_report.zig");
const diff = @import("diff.zig");

const Io = std.Io;
const Format = types.Format;
const EditOp = types.EditOp;
const PathSegment = fig.AST.PathSegment;
const Node = fig.AST.Node;
const Stringify = std.json.Stringify;
const Allocator = std.mem.Allocator;

/// The revision this server is written to.
const protocol = "2026-07-28";

/// Earlier revisions a client may open with `initialize`.
const legacy = [_][]const u8{ "2025-11-25", "2025-06-18", "2025-03-26" };

/// The `_meta` key a modern request names its revision under.
const version_meta = "io.modelcontextprotocol/protocolVersion";

const instructions =
    \\This server edits configuration files (JSON, JSONC, JSON5, YAML, TOML, INI, dotenv, .properties, plist, ZON, NestedText, fig, XML, and the frontmatter of Markdown files) by path, losslessly: comments, key order, quoting and blank lines outside an edit stay exactly as they were. Prefer it to rewriting or string-replacing a config file. Call outline to see a file's paths and its hash, get for one value in full, then apply with base set to that hash. apply makes all its edits or none. Paths look like service.ports[1], with ["a.b"] for a key that is not a bare word and [-] for the end of a list.
;

/// Serve until stdin closes.
pub fn run(io: Io, stdout_term: *Io.Terminal, stderr_term: *Io.Terminal, binary_name: []const u8, opts: types.McpOptions) !void {
    if (opts.requested_help) {
        try help.Help.mcp(stdout_term, binary_name);
        return;
    }
    // Values arrive as JSON and are read with fig's own JSON parser.
    if (comptime !build_options.lang_json) {
        try stderr_term.writer.writeAll("error: `fig mcp` needs the JSON format, which this build left out\n");
        try stderr_term.writer.flush();
        std.process.exit(1);
    }
    try stderr_term.writer.print("{s} mcp: serving on stdin/stdout\n", .{binary_name});
    try stderr_term.writer.flush();

    var in_buf: [64 * 1024]u8 = undefined;
    var stdin = fileio.stdioReader(Io.File.stdin(), io, &in_buf);
    const r = &stdin.interface;
    const w = stdout_term.writer;

    var arena = std.heap.ArenaAllocator.init(std.heap.smp_allocator);
    defer arena.deinit();
    var server: Server = .{ .io = io, .binary_name = binary_name };

    while (true) {
        _ = arena.reset(.retain_capacity);
        const a = arena.allocator();
        var line: Io.Writer.Allocating = .init(a);
        _ = r.streamDelimiterEnding(&line.writer, '\n') catch break;
        const at_end = r.bufferedLen() == 0;
        if (!at_end) r.toss(1);
        const text = std.mem.trim(u8, line.written(), " \t\r");
        if (text.len > 0) {
            server.answer(a, text, w) catch break;
            w.flush() catch break;
        }
        if (at_end) break;
    }
}

/// A JSON-RPC error, set on the server before a method returns `error.Fault`.
const Fault = struct {
    code: i64,
    message: []const u8,
};

const Server = struct {
    io: Io,
    binary_name: []const u8,
    fault: Fault = .{ .code = 0, .message = "" },
    /// Why a tool failed, set before it returns `error.Refused`: the text the
    /// model reads in an `isError` result.
    refusal: []const u8 = "",

    /// Answer one message, writing the reply (if any) as one line on `w`.
    fn answer(self: *Server, a: Allocator, line: []const u8, w: *Io.Writer) !void {
        const parsed = std.json.parseFromSliceLeaky(std.json.Value, a, line, .{}) catch
            return writeFault(w, .null, .{ .code = -32700, .message = "not JSON" });
        if (parsed != .object) return writeFault(w, .null, .{ .code = -32600, .message = "a request is an object" });
        const msg = parsed.object;
        // A notification, or a response to nothing this server asked: no reply.
        const id = msg.get("id") orelse return;
        const method = switch (msg.get("method") orelse return) {
            .string => |s| s,
            else => return writeFault(w, id, .{ .code = -32600, .message = "method is not a string" }),
        };
        const params: std.json.Value = msg.get("params") orelse .null;

        var body: Io.Writer.Allocating = .init(a);
        var s: Stringify = .{ .writer = &body.writer };
        self.call(a, method, params, &s) catch |err| switch (err) {
            error.Fault => return writeFault(w, id, self.fault),
            else => return writeFault(w, id, .{ .code = -32603, .message = @errorName(err) }),
        };
        try w.writeAll("{\"jsonrpc\":\"2.0\",\"id\":");
        try Stringify.value(id, .{}, w);
        try w.writeAll(",\"result\":");
        try w.writeAll(body.written());
        try w.writeAll("}\n");
    }

    fn call(self: *Server, a: Allocator, method: []const u8, params: std.json.Value, s: *Stringify) !void {
        if (!std.mem.eql(u8, method, "initialize")) if (field(params, "_meta")) |meta| if (str(meta, version_meta)) |v| {
            if (!std.mem.eql(u8, v, protocol) and !isLegacy(v))
                return self.fail(-32022, try std.fmt.allocPrint(a, "unsupported protocol version {s}; this server speaks {s}", .{ v, protocol }));
        };

        if (std.mem.eql(u8, method, "server/discover")) {
            try s.beginObject();
            try s.objectField("resultType");
            try s.write("complete");
            try s.objectField("supportedVersions");
            try writeVersions(s);
            try s.objectField("capabilities");
            try writeCapabilities(s);
            try s.objectField("_meta");
            try s.beginObject();
            try s.objectField("io.modelcontextprotocol/serverInfo");
            try writeServerInfo(s);
            try s.endObject();
            try s.objectField("instructions");
            try s.write(instructions);
            try s.endObject();
        } else if (std.mem.eql(u8, method, "initialize")) {
            const asked = str(params, "protocolVersion");
            const version = if (asked) |v| (if (isLegacy(v)) v else legacy[0]) else legacy[0];
            try s.beginObject();
            try s.objectField("protocolVersion");
            try s.write(version);
            try s.objectField("capabilities");
            try writeCapabilities(s);
            try s.objectField("serverInfo");
            try writeServerInfo(s);
            try s.objectField("instructions");
            try s.write(instructions);
            try s.endObject();
        } else if (std.mem.eql(u8, method, "ping")) {
            try s.beginObject();
            try s.endObject();
        } else if (std.mem.eql(u8, method, "tools/list")) {
            try s.beginWriteRaw();
            try s.writer.writeAll("{\"resultType\":\"complete\",\"tools\":" ++ tools_json ++ "}");
            s.endWriteRaw();
        } else if (std.mem.eql(u8, method, "tools/call")) {
            const name = str(params, "name") orelse return self.fail(-32602, "tools/call needs a tool name");
            const args: std.json.Value = field(params, "arguments") orelse .null;
            if (args != .object and args != .null) return self.fail(-32602, "a tool's arguments are an object");

            var out: Io.Writer.Allocating = .init(a);
            const tool: *const fn (*Server, Allocator, std.json.Value, *Io.Writer) anyerror!void =
                if (std.mem.eql(u8, name, "outline")) toolOutline else if (std.mem.eql(u8, name, "get")) toolGet else if (std.mem.eql(u8, name, "apply")) toolApply else return self.fail(-32602, try std.fmt.allocPrint(a, "there is no tool {s}", .{name}));
            self.refusal = "";
            const failed = if (tool(self, a, args, &out.writer)) false else |err| blk: {
                if (err != error.Refused) self.refusal = try self.describe(a, err);
                break :blk true;
            };
            try s.beginObject();
            try s.objectField("content");
            try s.beginArray();
            try s.beginObject();
            try s.objectField("type");
            try s.write("text");
            try s.objectField("text");
            try s.write(if (failed) self.refusal else out.written());
            try s.endObject();
            try s.endArray();
            try s.objectField("isError");
            try s.write(failed);
            try s.endObject();
        } else return self.fail(-32601, try std.fmt.allocPrint(a, "{s} is not a method here", .{method}));
    }

    fn fail(self: *Server, code: i64, message: []const u8) error{Fault} {
        self.fault = .{ .code = code, .message = message };
        return error.Fault;
    }

    fn refuse(self: *Server, a: Allocator, comptime fmt: []const u8, args: anytype) error{ Refused, OutOfMemory } {
        self.refusal = try std.fmt.allocPrint(a, fmt, args);
        return error.Refused;
    }

    /// An error no tool worded itself, in the CLI's words for it.
    fn describe(self: *Server, a: Allocator, err: anyerror) ![]const u8 {
        switch (err) {
            error.InvalidEditText => return if (edit_ops.refused_rendering) |r|
                std.fmt.allocPrint(a, "the file would not parse with {s} there; for raw, write it in the file's own syntax, or pass value as JSON", .{r})
            else
                "the file would not parse with that value there; for raw, write it in the file's own syntax, or pass value as JSON",
            error.InvalidEditKey => return "the file would not parse with that key there: its format cannot spell it",
            error.UnwritableNull => return "this format has no null; leave the key out, or delete it",
            error.UnwritableNested => return "this format cannot hold a list or object there",
            error.UnwritableKey => return "this format cannot hold that key",
            error.UnwritableNonFinite => return "this format has no spelling for an infinite or NaN number",
            error.PatchRenderRejected => return "the patch has a value the file's format could not read back once written",
            error.UnsupportedValue => return "the file's format cannot hold that value",
            else => {},
        }
        var out: Io.Writer.Allocating = .init(a);
        var term: Io.Terminal = .{ .writer = &out.writer, .mode = .no_color };
        diag_report.reportUnhandledImpl(&term, err, null, self.binary_name) catch {};
        return std.mem.trimEnd(u8, out.written(), "\n");
    }

    // ── Tools ────────────────────────────────────────────────────────────

    fn toolOutline(self: *Server, a: Allocator, args: std.json.Value, w: *Io.Writer) !void {
        const target = try self.load(a, args, false);
        const doc = try self.parse(a, target);
        const path = try self.pathArg(a, args, false);
        const depth: usize = if (int(args, "depth")) |d| @intCast(@max(d, 1)) else 4;

        var path_text: std.ArrayList(u8) = .empty;
        try path_text.appendSlice(a, str(args, "path") orelse "");
        const start = (doc.ast.getNodeByPath(path) catch |err| return self.pathRefusal(a, err, args)).id;

        try writeHeader(w, target);
        var o: Outliner = .{ .a = a, .w = w, .ast = &doc.ast, .depth = path.len + depth };
        try o.entry(start, &path_text, path.len);
        if (o.omitted > 0)
            try w.print("… {d} more lines; outline a narrower path, or a smaller depth\n", .{o.omitted});
    }

    fn toolGet(self: *Server, a: Allocator, args: std.json.Value, w: *Io.Writer) !void {
        const target = try self.load(a, args, false);
        const doc = try self.parse(a, target);
        const path = try self.pathArg(a, args, false);
        const entry_id = (doc.ast.getNodeByPath(path) catch |err| return self.pathRefusal(a, err, args)).id;
        const value_id = switch (doc.ast.nodes[entry_id].kind) {
            .keyvalue => |kv| kv.value,
            else => entry_id,
        };
        const value = doc.ast.nodes[value_id];

        try writeHeader(w, target);
        const comments = doc.ast.comments(doc.ast.leadingCommentAnchor(entry_id));
        for (comments.leading) |c| try writeComment(w, c.text);

        // The value as the file spells it, where its span is the whole of it:
        // not for a section (a TOML table), whose entries are lines of their
        // own, possibly reopened further down.
        if (value_id < doc.node_spans.len and !doc.isSection(value)) {
            const sp = doc.node_spans[value_id];
            if (sp.start <= sp.end and sp.end <= target.text.len) {
                const line = std.mem.count(u8, target.text[0..sp.start], "\n") + 1;
                try w.print("line {d}:\n{s}\n", .{ line, target.text[sp.start..sp.end] });
                if (doc.ast.comments(doc.ast.trailingCommentAnchor(entry_id)).trailing) |c| try writeComment(w, c.text);
                return;
            }
        }
        // Otherwise the value as JSON, which says what it is if not where.
        try w.writeAll("as JSON:\n");
        try writeJson(a, w, &doc.ast, value_id, true);
        try w.writeByte('\n');
    }

    fn toolApply(self: *Server, a: Allocator, args: std.json.Value, w: *Io.Writer) !void {
        const target = try self.load(a, args, true);
        // A file that does not parse is reported where it fails, before any
        // edit is blamed for it.
        _ = try self.parse(a, target);
        if (str(args, "base")) |base| if (!std.mem.eql(u8, base, &target.hash))
            return self.refuse(a, "{s} has changed since hash {s} (it is now {s}); outline or get it again, and apply against the new hash", .{ target.file, base, &target.hash });
        const edits = switch (field(args, "edits") orelse return self.refuse(a, "apply needs edits: a list of {{op, path, …}}", .{})) {
            .array => |arr| arr.items,
            else => return self.refuse(a, "edits is a list", .{}),
        };
        if (edits.len == 0) return self.refuse(a, "edits is empty, so there is nothing to apply", .{});

        var text: []const u8 = target.text;
        for (edits, 0..) |edit, i| {
            text = self.applyOne(a, target, text, edit) catch |err| {
                const why = if (err == error.Refused) self.refusal else try self.describe(a, err);
                return self.refuse(a, "edit {d} ({s}) was refused, so nothing was written: {s}", .{ i, opName(edit), why });
            };
        }
        const new_bytes = try target.assemble(a, text);
        const dry_run = boolean(args, "dry_run") orelse false;

        if (std.mem.eql(u8, new_bytes, target.bytes)) {
            try w.print("{s}: no change; hash {s}\n", .{ target.file, &target.hash });
            return;
        }
        if (!dry_run) try self.write(target.file, new_bytes);
        const new_hash = hashOf(new_bytes);
        try w.print("{s}: {s}; hash {s}\n", .{ target.file, if (dry_run) "dry run, nothing written" else "written", &new_hash });
        try diff.unifiedDiff(a, w, target.file, target.bytes, new_bytes, 1);
    }

    /// One edit of an `apply`, made on `text` (the document, or its embedded
    /// region): the new text.
    fn applyOne(self: *Server, a: Allocator, target: Target, text: []const u8, edit: std.json.Value) ![]const u8 {
        if (edit != .object) return self.refuse(a, "an edit is an object: {{op, path, …}}", .{});
        const op = str(edit, "op") orelse return self.refuse(a, "an edit needs an op (set, insert, delete, rename, comment, patch)", .{});
        const path = try self.pathArg(a, edit, true);
        const format = target.format;

        if (std.mem.eql(u8, op, "set")) {
            const v = try self.valueArg(a, edit);
            return edit_ops.applyToSlice(a, text, format, path, v.text, .{ .one = v.value }, .set);
        } else if (std.mem.eql(u8, op, "insert")) {
            if (path.len == 0) return self.refuse(a, "insert needs a path that ends in the new key, or in [0] or [-] for a list", .{});
            const v = try self.valueArg(a, edit);
            const parent = path[0 .. path.len - 1];
            const edit_op: EditOp = switch (path[path.len - 1]) {
                .key => |key| .{ .insert_key = key },
                .index => |index| if (index == 0)
                    .prepend_seq
                else if (index == types.append_index)
                    .append_seq
                else
                    return self.refuse(a, "insert puts a list item at [0] or [-]; there is no insert in the middle of a list (set the whole list instead)", .{}),
            };
            return edit_ops.applyToSlice(a, text, format, parent, v.text, .{ .one = v.value }, edit_op);
        } else if (std.mem.eql(u8, op, "delete")) {
            if (path.len == 0) return self.refuse(a, "delete needs the path of an entry or list item", .{});
            return switch (path[path.len - 1]) {
                .index => |index| edit_ops.applyToSlice(a, text, format, path[0 .. path.len - 1], "", .{}, .{ .remove_seq_item = index }),
                .key => edit_ops.applyToSlice(a, text, format, path, "", .{}, .delete_key),
            };
        } else if (std.mem.eql(u8, op, "rename")) {
            const to = str(edit, "to") orelse return self.refuse(a, "rename needs to: the new key", .{});
            return edit_ops.applyToSlice(a, text, format, path, to, .{}, .replace_key);
        } else if (std.mem.eql(u8, op, "comment")) {
            const trailing = if (str(edit, "position")) |p| std.mem.eql(u8, p, "trailing") else false;
            const comment = str(edit, "text") orelse "";
            const edit_op: EditOp = if (comment.len == 0)
                (if (trailing) .delete_trailing_comment else .delete_leading_comments)
            else if (trailing) .set_trailing_comment else .add_leading_comment;
            return edit_ops.applyToSlice(a, text, format, path, comment, .{}, edit_op);
        } else if (std.mem.eql(u8, op, "patch")) {
            const json = field(edit, "value") orelse return self.refuse(a, "patch needs value: the JSON to merge in at path", .{});
            const tree = try self.jsonTree(a, json);
            const result = try patch_ops.applyToSlice(a, text, format, .{ .at = path, .patch = &tree, .from = tree.root });
            return result.content;
        }
        return self.refuse(a, "there is no op {s}; the ops are set, insert, delete, rename, comment and patch", .{op});
    }

    // ── Arguments ────────────────────────────────────────────────────────

    fn pathArg(self: *Server, a: Allocator, args: std.json.Value, required: bool) ![]PathSegment {
        const text = str(args, "path") orelse {
            if (required) return self.refuse(a, "an edit needs a path (\"\" for the root)", .{});
            return &.{};
        };
        return args_mod.parsePath(a, text) catch
            self.refuse(a, "{s} is not a path: write keys with dots and indices in brackets, as service.ports[1], [\"a.b\"] for a key with a dot in it, and [-] for the end of a list", .{text});
    }

    const ValueArg = struct { text: []const u8, value: ?value_arg.Value };

    /// `value` (JSON) or `raw` (the file's own syntax), exactly one of them.
    fn valueArg(self: *Server, a: Allocator, edit: std.json.Value) !ValueArg {
        if (str(edit, "raw")) |raw| {
            if (field(edit, "value") != null) return self.refuse(a, "give value or raw, not both", .{});
            return .{ .text = raw, .value = .{ .raw = raw } };
        }
        const json = field(edit, "value") orelse return self.refuse(a, "this edit needs value (JSON) or raw (the file's own syntax)", .{});
        return .{ .text = "", .value = .{ .tree = try self.jsonTree(a, json) } };
    }

    /// A JSON value from the request, as a fig tree to render into the file.
    fn jsonTree(self: *Server, a: Allocator, json: std.json.Value) !fig.AST {
        _ = self;
        const text = try Stringify.valueAlloc(a, json, .{});
        var reports: parse_dispatch.Reports = .{};
        const doc = try parse_dispatch.parseSliceAs(.json, .{}, a, text, false, &reports);
        return doc.ast;
    }

    fn pathRefusal(self: *Server, a: Allocator, err: anyerror, args: std.json.Value) error{ Refused, OutOfMemory } {
        const path = str(args, "path") orelse "";
        return switch (err) {
            error.NotFound, error.IndexOutOfBounds => self.refuse(a, "{s} has nothing at {s}; outline it to see its paths", .{ str(args, "file") orelse "the file", path }),
            else => self.refuse(a, "{s}: {s}", .{ path, self.describe(a, err) catch @errorName(err) }),
        };
    }

    // ── Files ────────────────────────────────────────────────────────────

    /// Read the file `args` names, resolve its format, and cut out its
    /// embedded region if it has one. `for_write` creates an empty region
    /// in a Markdown file that has none, as `fig set` does.
    fn load(self: *Server, a: Allocator, args: std.json.Value, for_write: bool) !Target {
        const file = str(args, "file") orelse return self.refuse(a, "this tool needs file: the path of the file", .{});
        const bytes = std.Io.Dir.cwd().readFileAlloc(self.io, file, a, .limited(64 * 1024 * 1024)) catch |err| return switch (err) {
            error.FileNotFound => self.refuse(a, "there is no file {s}", .{file}),
            error.IsDir => self.refuse(a, "{s} is a directory", .{file}),
            error.AccessDenied => self.refuse(a, "permission denied: {s}", .{file}),
            else => err,
        };
        var target: Target = .{ .file = file, .bytes = bytes, .text = bytes, .format = .json, .hash = hashOf(bytes) };

        var embed_type: ?fig.Embed.Type = null;
        if (str(args, "format")) |name| {
            target.format = args_mod.parseFormatName(name) orelse return self.refuse(a, "{s} is not a format fig knows", .{name});
        } else if (args_mod.detectLanguageFromFileEnding(file)) |d| {
            target.format = d.format;
            if (d.embed_detect) embed_type = args_mod.resolveEmbedTypeFromContent(bytes, null, true);
        } else {
            target.format = parse_dispatch.resolveFormatFromContent(a, bytes, file) catch
                return self.refuse(a, "fig cannot tell what format {s} is from its name or its contents; pass format", .{file});
        }

        const et = embed_type orelse return target;
        target.format = args_mod.embedFormat(et);
        var host: []const u8 = bytes;
        const region = fig.Embed.locateRegion(bytes, et) catch |err| switch (err) {
            error.NotFound => blk: {
                if (!for_write) return self.refuse(a, "{s} has no frontmatter", .{file});
                const created = try fig.Embed.initRegion(a, bytes, et);
                host = created.host;
                break :blk created.region;
            },
            else => return err,
        };
        const codec = fig.Embed.codecOf(et);
        const inner = host[region.content.start..region.content.end];
        const decoded = try fig.Embed.decodeForParse(a, inner, codec);
        target.text = decoded.text;
        target.embedded = .{ .host = host, .region = region, .codec = codec, .decoded = decoded };
        return target;
    }

    fn parse(self: *Server, a: Allocator, target: Target) !fig.Document {
        var reports: parse_dispatch.Reports = .{};
        return parse_dispatch.parseSliceAs(target.format, .{}, a, target.text, false, &reports) catch |err| {
            const d = reports.diagnostic() orelse return err;
            var out: Io.Writer.Allocating = .init(a);
            var term: Io.Terminal = .{ .writer = &out.writer, .mode = .no_color };
            try diag_report.printDiag(&term, target.text, target.file, d.offset, d.end, "error", .red, d.message, d.short_label);
            return self.refuse(a, "{s} does not parse as {s}:\n{s}", .{ target.file, @tagName(target.format), std.mem.trimEnd(u8, out.written(), "\n") });
        };
    }

    /// Write `bytes` over `file` in place, as the CLI's editing actions do,
    /// so the file keeps its permissions and any links to it.
    fn write(self: *Server, file: []const u8, bytes: []const u8) !void {
        const handle = try std.Io.Dir.cwd().openFile(self.io, file, .{ .mode = .read_write });
        defer handle.close(self.io);
        try handle.writePositionalAll(self.io, bytes, 0);
        try handle.setLength(self.io, bytes.len);
    }
};

/// A file, read and resolved: the bytes on disk, and the text the editor
/// works on — the same bytes, or a Markdown file's frontmatter, decoded.
const Target = struct {
    file: []const u8,
    bytes: []const u8,
    format: Format,
    text: []const u8,
    hash: [12]u8,
    embedded: ?Embedded = null,

    const Embedded = struct {
        /// The file, or the file with an empty region added to it.
        host: []const u8,
        region: fig.Embed.Region,
        codec: fig.Embed.Codec,
        decoded: fig.Embed.Decoded,
    };

    /// The whole file, with `edited` in place of `text`.
    fn assemble(self: Target, a: Allocator, edited: []const u8) ![]const u8 {
        const e = self.embedded orelse return edited;
        const inner = e.host[e.region.content.start..e.region.content.end];
        const encoded = try fig.Embed.reencodeEdited(a, e.codec, inner, e.decoded, edited);
        return std.mem.concat(a, u8, &.{ e.host[0..e.region.content.start], encoded, e.host[e.region.content.end..] });
    }
};

fn hashOf(bytes: []const u8) [12]u8 {
    var digest: [std.crypto.hash.sha2.Sha256.digest_length]u8 = undefined;
    std.crypto.hash.sha2.Sha256.hash(bytes, &digest, .{});
    return std.fmt.bytesToHex(digest[0..6].*, .lower);
}

fn writeHeader(w: *Io.Writer, target: Target) !void {
    try w.print("{s} ({s}{s}) hash {s}\n", .{ target.file, @tagName(target.format), if (target.embedded != null) " frontmatter" else "", &target.hash });
}

fn writeComment(w: *Io.Writer, text: []const u8) !void {
    var lines = std.mem.splitScalar(u8, text, '\n');
    while (lines.next()) |l| try w.print("# {s}\n", .{l});
}

/// `id`'s value as JSON: compact for one line of an outline, pretty for `get`.
fn writeJson(a: Allocator, w: *Io.Writer, ast: *const fig.AST, id: Node.Id, pretty: bool) !void {
    var view = ast.*;
    view.root = id;
    view.node_comments = &.{};
    var out: Io.Writer.Allocating = .init(a);
    view.serializeFragmentWith(&out.writer, .json, .{ .pretty = pretty }) catch |err| {
        try w.print("<{s}>", .{@errorName(err)});
        return;
    };
    try w.writeAll(std.mem.trimEnd(u8, out.written(), "\n"));
}

/// The outline: one line per scalar, as `path = json`, with its comments; a
/// container gets a line of its own only when it is empty, past the depth
/// asked for, or has a comment to carry.
const Outliner = struct {
    a: Allocator,
    w: *Io.Writer,
    ast: *const fig.AST,
    /// The path length past which a container is summarized.
    depth: usize,
    lines: usize = 0,
    max_lines: usize = 400,
    omitted: usize = 0,

    /// `id` is a container's child as the AST holds it: a `keyvalue` for a
    /// mapping entry, the value itself for a list item or the root.
    fn entry(self: *Outliner, id: Node.Id, path: *std.ArrayList(u8), level: usize) !void {
        const ast = self.ast;
        const value_id = switch (ast.nodes[id].kind) {
            .keyvalue => |kv| kv.value,
            else => id,
        };
        const leading = ast.comments(ast.leadingCommentAnchor(id)).leading;
        const trailing = ast.comments(ast.trailingCommentAnchor(id)).trailing;
        for (leading) |c| {
            var lines = std.mem.splitScalar(u8, c.text, '\n');
            while (lines.next()) |l| try self.line("# {s}", .{l});
        }
        const shown = if (path.items.len == 0) "(root)" else path.items;

        switch (ast.nodes[value_id].kind) {
            .mapping, .sequence => |first| {
                const is_map = ast.nodes[value_id].kind == .mapping;
                var count: usize = 0;
                var cur = first;
                while (cur) |c| : (count += 1) cur = ast.nodes[c].next_sibling;
                const noun = if (is_map) (if (count == 1) "key" else "keys") else (if (count == 1) "item" else "items");
                const open, const close = if (is_map) .{ "{", "}" } else .{ "[", "]" };
                if (count == 0) return self.lineTrailing(trailing, "{s} = {s}{s}", .{ shown, open, close });
                if (level >= self.depth) return self.lineTrailing(trailing, "{s} = {s}{d} {s}{s}", .{ shown, open, count, noun, close });
                if (leading.len > 0 or trailing != null) try self.lineTrailing(trailing, "{s} = {s}{d} {s}{s}", .{ shown, open, count, noun, close });

                const mark = path.items.len;
                var index: usize = 0;
                cur = first;
                while (cur) |child| : (index += 1) {
                    path.shrinkRetainingCapacity(mark);
                    if (is_map) try self.keySegment(path, ast.nodes[child].kind.keyvalue.key) else try path.print(self.a, "[{d}]", .{index});
                    try self.entry(child, path, level + 1);
                    cur = ast.nodes[child].next_sibling;
                }
                path.shrinkRetainingCapacity(mark);
            },
            .alias => |name| try self.lineTrailing(trailing, "{s} = *{s}", .{ shown, name }),
            else => {
                var v: Io.Writer.Allocating = .init(self.a);
                try writeJson(self.a, &v.writer, ast, value_id, false);
                const text = v.written();
                const cap = 160;
                if (text.len > cap)
                    try self.lineTrailing(trailing, "{s} = {s}… ({d} bytes; get it for the rest)", .{ shown, text[0..cap], text.len })
                else
                    try self.lineTrailing(trailing, "{s} = {s}", .{ shown, text });
            },
        }
    }

    /// A key as a path segment: `.key` for a bare word, `["…"]` otherwise —
    /// both of which `args.parsePath` reads back.
    fn keySegment(self: *Outliner, path: *std.ArrayList(u8), key_id: Node.Id) !void {
        const text = switch (self.ast.nodes[key_id].kind) {
            .string => |s| s,
            else => blk: {
                var v: Io.Writer.Allocating = .init(self.a);
                try writeJson(self.a, &v.writer, self.ast, key_id, false);
                break :blk v.written();
            },
        };
        if (isBareKey(text)) {
            if (path.items.len > 0) try path.append(self.a, '.');
            try path.appendSlice(self.a, text);
        } else {
            try path.append(self.a, '[');
            var v: Io.Writer.Allocating = .init(self.a);
            try Stringify.value(text, .{}, &v.writer);
            try path.appendSlice(self.a, v.written());
            try path.append(self.a, ']');
        }
    }

    fn lineTrailing(self: *Outliner, trailing: ?fig.AST.Comment, comptime fmt: []const u8, args: anytype) !void {
        if (self.lines >= self.max_lines) {
            self.omitted += 1;
            return;
        }
        self.lines += 1;
        try self.w.print(fmt, args);
        if (trailing) |c| try self.w.print("  # {s}", .{c.text});
        try self.w.writeByte('\n');
    }

    fn line(self: *Outliner, comptime fmt: []const u8, args: anytype) !void {
        return self.lineTrailing(null, fmt, args);
    }
};

/// Whether `key` can stand in a path unquoted: what `parsePath` would read
/// back as the same key.
fn isBareKey(key: []const u8) bool {
    if (key.len == 0) return false;
    for (key) |c| switch (c) {
        'a'...'z', 'A'...'Z', '0'...'9', '_', '-' => {},
        else => return false,
    };
    return true;
}

fn opName(edit: std.json.Value) []const u8 {
    return str(edit, "op") orelse "no op";
}

// ── JSON helpers ─────────────────────────────────────────────────────────

fn field(v: std.json.Value, key: []const u8) ?std.json.Value {
    return switch (v) {
        .object => |o| o.get(key),
        else => null,
    };
}

fn str(v: std.json.Value, key: []const u8) ?[]const u8 {
    return switch (field(v, key) orelse return null) {
        .string => |s| s,
        else => null,
    };
}

fn int(v: std.json.Value, key: []const u8) ?i64 {
    return switch (field(v, key) orelse return null) {
        .integer => |i| i,
        else => null,
    };
}

fn boolean(v: std.json.Value, key: []const u8) ?bool {
    return switch (field(v, key) orelse return null) {
        .bool => |b| b,
        else => null,
    };
}

fn isLegacy(version: []const u8) bool {
    for (legacy) |l| if (std.mem.eql(u8, l, version)) return true;
    return false;
}

fn writeFault(w: *Io.Writer, id: std.json.Value, fault: Fault) !void {
    try w.writeAll("{\"jsonrpc\":\"2.0\",\"id\":");
    try Stringify.value(id, .{}, w);
    try w.print(",\"error\":{{\"code\":{d},\"message\":", .{fault.code});
    try Stringify.value(fault.message, .{}, w);
    try w.writeAll("}}\n");
}

fn writeVersions(s: *Stringify) !void {
    try s.beginArray();
    try s.write(protocol);
    for (legacy) |l| try s.write(l);
    try s.endArray();
}

fn writeCapabilities(s: *Stringify) !void {
    try s.beginObject();
    try s.objectField("tools");
    try s.beginObject();
    try s.endObject();
    try s.endObject();
}

fn writeServerInfo(s: *Stringify) !void {
    try s.beginObject();
    try s.objectField("name");
    try s.write("fig");
    try s.objectField("version");
    try s.print("\"{d}.{d}.{d}\"", .{ build_options.version_major, build_options.version_minor, build_options.version_patch });
    try s.endObject();
}

const file_prop =
    \\"file":{"type":"string","description":"The file's path, absolute or relative to where the server was started."},"format":{"type":"string","description":"The file's format (yaml, jsonc, toml, …), when its name does not say. A .md file's frontmatter is found by itself."}
;

/// The tools, as `tools/list` answers. Written across lines to be read, and
/// joined onto one (`oneLine`), since a message on stdio is one line.
const tools_json = oneLine(
    \\[{"name":"outline","title":"Outline a config file","description":"The paths in a config file with their values and comments, one per line as path = JSON, and the file's hash for apply. Containers past depth are summarized as {n keys} or [n items]. Use it instead of reading the file.","annotations":{"readOnlyHint":true},"inputSchema":{"type":"object","properties":{
++ file_prop ++
    \\,"path":{"type":"string","description":"Outline only the value at this path."},"depth":{"type":"integer","minimum":1,"description":"How many levels below path to expand. Default 4."}},"required":["file"]}},
    \\{"name":"get","title":"Get one value","description":"One value in full, exactly as the file spells it, with its line number and comments, and the file's hash.","annotations":{"readOnlyHint":true},"inputSchema":{"type":"object","properties":{
++ file_prop ++
    \\,"path":{"type":"string","description":"The value's path; empty for the whole document."}},"required":["file","path"]}},
    \\{"name":"apply","title":"Edit a config file by path","description":"Make edits to a config file by path, all or none, and write it once. Only the bytes of each edit change; comments, order and formatting elsewhere stay. Values are JSON, written in the file's own syntax. Returns the diff and the new hash. Ops: set {path, value|raw} creates or replaces; insert {path, value|raw} adds a new key (path ends in it) or a list item (path ends in [0] or [-]); delete {path}; rename {path, to}; comment {path, text, position: leading|trailing} (empty text removes); patch {path, value} merges a JSON object in, touching only what differs (a list it changes is rewritten whole, so add to a list with insert [-]).","annotations":{"destructiveHint":true,"idempotentHint":false},"inputSchema":{"type":"object","properties":{
++ file_prop ++
    \\,"base":{"type":"string","description":"The hash outline or get returned. If the file has changed since, nothing is written."},"dry_run":{"type":"boolean","description":"Return the diff without writing."},"edits":{"type":"array","minItems":1,"items":{"type":"object","properties":{"op":{"enum":["set","insert","delete","rename","comment","patch"]},"path":{"type":"string","description":"As outline prints it: service.ports[1], [\"a.b\"] for a key that is not a bare word, [-] for the end of a list, empty for the root."},"value":{"description":"The new value, as JSON."},"raw":{"type":"string","description":"The new value in the file's own syntax, spliced as written, instead of value."},"to":{"type":"string","description":"rename: the new key."},"text":{"type":"string","description":"comment: the comment's text, without its marker."},"position":{"enum":["leading","trailing"]}},"required":["op","path"]}}},"required":["file","edits"]}}]
);

/// `text` with its line breaks taken out, which a multiline literal puts
/// between its lines; each break here falls between two JSON tokens.
fn oneLine(comptime text: []const u8) []const u8 {
    comptime {
        @setEvalBranchQuota(100_000);
        var out: [text.len]u8 = undefined;
        var n: usize = 0;
        for (text) |c| if (c != '\n') {
            out[n] = c;
            n += 1;
        };
        const final = out[0..n].*;
        return &final;
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

const testing = std.testing;

/// Run `requests` (one per line) through a server and return its replies.
fn converse(a: Allocator, requests: []const []const u8) ![]const u8 {
    var server: Server = .{ .io = testing.io, .binary_name = "fig" };
    var out: Io.Writer.Allocating = .init(a);
    for (requests) |req| try server.answer(a, req, &out.writer);
    return out.written();
}

test "tools/list names the three tools, and every reply is one line" {
    if (comptime !build_options.lang_json) return error.SkipZigTest;
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const replies = try converse(arena.allocator(), &.{
        \\{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
        ,
        \\{"jsonrpc":"2.0","method":"notifications/initialized"}
        ,
        \\{"jsonrpc":"2.0","id":"two","method":"nope"}
    });
    try testing.expectEqual(@as(usize, 2), std.mem.count(u8, replies, "\n"));
    const parsed = try std.json.parseFromSliceLeaky(std.json.Value, arena.allocator(), replies[0..std.mem.indexOfScalar(u8, replies, '\n').?], .{});
    const tools = parsed.object.get("result").?.object.get("tools").?.array.items;
    try testing.expectEqual(@as(usize, 3), tools.len);
    try testing.expectEqualStrings("outline", tools[0].object.get("name").?.string);
    try testing.expect(std.mem.indexOf(u8, replies, "\"id\":\"two\",\"error\":{\"code\":-32601") != null);
}

/// The text of a `tools/call` reply, and whether it is an error.
fn toolText(a: Allocator, reply: []const u8) !struct { text: []const u8, is_error: bool } {
    const parsed = try std.json.parseFromSliceLeaky(std.json.Value, a, std.mem.trimEnd(u8, reply, "\n"), .{});
    const result = parsed.object.get("result").?.object;
    return .{ .text = result.get("content").?.array.items[0].object.get("text").?.string, .is_error = result.get("isError").?.bool };
}

test "outline, then apply against its hash, edits only what was asked" {
    if (comptime !build_options.lang_json or !build_options.lang_yaml) return error.SkipZigTest;
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const original =
        \\# Deploy settings
        \\service:
        \\  name: api   # must match the DNS record
        \\  ports: [80, 443]
        \\  replicas: 3
        \\
    ;
    try tmp.dir.writeFile(testing.io, .{ .sub_path = "deploy.yaml", .data = original });
    const file = try tmp.dir.realPathFileAlloc(testing.io, "deploy.yaml", a);

    const outline_req = try std.fmt.allocPrint(a,
        \\{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"outline","arguments":{{"file":"{s}"}}}}}}
    , .{file});
    const outline = try toolText(a, try converse(a, &.{outline_req}));
    try testing.expect(!outline.is_error);
    try testing.expect(std.mem.indexOf(u8, outline.text, "service.name = \"api\"  # must match the DNS record\n") != null);
    try testing.expect(std.mem.indexOf(u8, outline.text, "service.ports[1] = 443\n") != null);
    const hash_at = std.mem.indexOf(u8, outline.text, "hash ").? + 5;
    const hash = outline.text[hash_at .. hash_at + 12];

    const apply_req = try std.fmt.allocPrint(a,
        \\{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"apply","arguments":{{"file":"{s}","base":"{s}","edits":[{{"op":"set","path":"service.replicas","value":5}},{{"op":"insert","path":"service.ports[-]","value":8080}},{{"op":"insert","path":"service.region","value":"eu-west"}}]}}}}}}
    , .{ file, hash });
    const applied = try toolText(a, try converse(a, &.{apply_req}));
    try testing.expect(!applied.is_error);

    const after = try tmp.dir.readFileAlloc(testing.io, "deploy.yaml", a, .unlimited);
    try testing.expectEqualStrings(
        \\# Deploy settings
        \\service:
        \\  name: api   # must match the DNS record
        \\  ports: [80, 443, 8080]
        \\  replicas: 5
        \\  region: eu-west
        \\
    , after);

    // The same request again: the hash is stale now, and nothing is written.
    const stale = try toolText(a, try converse(a, &.{apply_req}));
    try testing.expect(stale.is_error);
    try testing.expect(std.mem.indexOf(u8, stale.text, "has changed since") != null);
}

test "apply writes nothing when one of its edits is refused" {
    if (comptime !build_options.lang_json or !build_options.lang_toml) return error.SkipZigTest;
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const original = "[package]\nname = \"x\" # the crate\nversion = \"0.1.0\"\n";
    try tmp.dir.writeFile(testing.io, .{ .sub_path = "Cargo.toml", .data = original });
    const file = try tmp.dir.realPathFileAlloc(testing.io, "Cargo.toml", a);

    const req = try std.fmt.allocPrint(a,
        \\{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"apply","arguments":{{"file":"{s}","edits":[{{"op":"set","path":"package.version","value":"0.2.0"}},{{"op":"delete","path":"package.missing"}}]}}}}}}
    , .{file});
    const result = try toolText(a, try converse(a, &.{req}));
    try testing.expect(result.is_error);
    try testing.expect(std.mem.startsWith(u8, result.text, "edit 1 (delete) was refused, so nothing was written"));
    try testing.expectEqualStrings(original, try tmp.dir.readFileAlloc(testing.io, "Cargo.toml", a, .unlimited));
}

test "a Markdown file's frontmatter is edited in place, and its body left alone" {
    if (comptime !build_options.lang_json or !build_options.lang_yaml) return error.SkipZigTest;
    var arena = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    try tmp.dir.writeFile(testing.io, .{ .sub_path = "post.md", .data = "---\ntitle: Hello\ntags: [a]\n---\n\n# Hello\n\nbody: not frontmatter\n" });
    const file = try tmp.dir.realPathFileAlloc(testing.io, "post.md", a);

    const req = try std.fmt.allocPrint(a,
        \\{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"apply","arguments":{{"file":"{s}","edits":[{{"op":"patch","path":"","value":{{"title":"Hello again","draft":true}}}}]}}}}}}
    , .{file});
    const result = try toolText(a, try converse(a, &.{req}));
    try testing.expect(!result.is_error);
    try testing.expectEqualStrings(
        "---\ntitle: Hello again\ntags: [a]\ndraft: true\n---\n\n# Hello\n\nbody: not frontmatter\n",
        try tmp.dir.readFileAlloc(testing.io, "post.md", a, .unlimited),
    );
}
