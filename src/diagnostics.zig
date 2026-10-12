//! Serialization diagnostics: a read-only pass that reports what a conversion
//! would silently lose.
//!
//! fig's printers degrade or drop data whenever the target format can't hold a
//! value or a comment natively — a TOML `null` vanishes, a datetime collapses to
//! a string, a block comment becomes a run of `#` lines, plain JSON drops every
//! comment. None of that is an error (the output is still valid), so it happens
//! quietly. `analyze` surfaces it: it walks the AST exactly as a printer would
//! and returns one `Warning` per lossy event, each tagged with a `cause` so a
//! consumer (the CLI, a binding) can choose which to show.
//!
//! This is a separate pass, NOT instrumentation threaded through the printers:
//! the printers stay untouched, and the format-capability knowledge lives in one
//! place here (reusing `Lossless.needsEnvelope`/`isUnrepresentable` for the
//! formats whose capability the lossless layer already models). The walk shape
//! mirrors `Lossless.lossyStrip` — same dotted/`[i]` path building — but emits
//! instead of removing.
//!
//! Scope is serialization only. Read-side lossiness (deserialize's
//! `ignore_unknown_fields`, YAML materialize dropping tags/anchors) is not
//! covered here; the model is additive if that is ever wanted.

const std = @import("std");
const Allocator = std.mem.Allocator;
const AST = @import("kit/ast/ast.zig");
const Serialize = @import("serialize.zig");
const Lossless = @import("lossless.zig");
const Language = @import("languages/language.zig");
const Runtime = @import("languages/runtime.zig");
const Writer = std.Io.Writer;

const ExtKind = AST.Node.Kind.Extended.ExtKind;
const Format = Serialize.SerializeFormat;

/// What the tree is analyzed as being printed to: a compiled format, or a
/// runtime language, whose losses are read off its declaration.
pub const Target = Runtime.Target;

/// One thing a conversion to a given format would lose.
pub const Warning = struct {
    code: Code,
    cause: Cause,
    /// Dotted/`[i]` path to the affected node, relative to the analyzed root.
    /// Empty (`""`) means the document root itself. Arena-owned by `analyze`.
    path: []const u8,
    /// Small extra context — for `type_degraded`, the type the value collapses
    /// to ("string", "number", …); otherwise empty. Static (not arena-owned).
    note: []const u8 = "",

    pub const Code = enum {
        /// A carried comment is not emitted at all (the target/mode has no comment
        /// syntax, or it was explicitly stripped).
        comment_dropped,
        /// A block comment is rendered as a run of line comments (the target has
        /// no block-comment syntax).
        comment_style_degraded,
        /// A node is removed entirely because the target cannot represent it even
        /// degraded (today: a `null` bound for TOML).
        value_dropped,
        /// An `extended` or non-finite value is rendered as a poorer type (a
        /// datetime/enum as a string, a char as a number, `inf`/`nan` quoted).
        type_degraded,
    };

    /// Why the loss happens — so a consumer can keep or ignore each class.
    pub const Cause = enum {
        /// The target format inherently cannot represent it.
        format_limitation,
        /// A caller option forced it (e.g. `strip_comments`).
        explicit_option,
    };

    /// Write the default human-readable message (no trailing newline). Bindings
    /// may instead render their own text from the structured fields.
    pub fn render(self: Warning, writer: *Writer, format: Format) Writer.Error!void {
        return self.renderNamed(writer, @tagName(format));
    }

    /// `render` for a target named `fmt` — `Target.name`, which covers a
    /// runtime language too.
    pub fn renderNamed(self: Warning, writer: *Writer, fmt: []const u8) Writer.Error!void {
        switch (self.code) {
            .value_dropped => {
                try writer.print("dropped {s} value at ", .{self.note});
                try writeLoc(writer, self.path);
                try writer.print(" ({s} cannot represent it)", .{fmt});
            },
            .type_degraded => {
                try writer.writeAll("degraded value at ");
                try writeLoc(writer, self.path);
                try writer.print(" to {s} ({s} has no native type for it)", .{ self.note, fmt });
            },
            .comment_dropped => {
                if (self.cause == .explicit_option) {
                    try writer.writeAll("stripped comment at ");
                    try writeLoc(writer, self.path);
                } else {
                    try writer.writeAll("dropped comment at ");
                    try writeLoc(writer, self.path);
                    try writer.print(" ({s} has no comment syntax here)", .{fmt});
                }
            },
            .comment_style_degraded => {
                try writer.writeAll("block comment at ");
                try writeLoc(writer, self.path);
                try writer.print(" rendered as line comments ({s} has no block comments)", .{fmt});
            },
        }
    }
};

/// Write a node path for messages: backtick-quoted, or "the document root" for
/// the empty (root) path.
fn writeLoc(writer: *Writer, path: []const u8) Writer.Error!void {
    if (path.len == 0) {
        try writer.writeAll("the document root");
    } else {
        try writer.writeByte('`');
        try writer.writeAll(path);
        try writer.writeByte('`');
    }
}

/// What the consumer is asking the printer to do — mirrors the subset of
/// `AST.SerializeOptions` that changes what gets lost, plus the lossless flag
/// (which lives outside `SerializeOptions`).
pub const Options = struct {
    /// Pretty (multi-line) output. Comment-bearing formats with no compact
    /// comment form (JSON5/JSONC/ZON) drop comments when this is false.
    pretty: bool = true,
    /// The caller asked to drop comments. Every comment becomes a
    /// `comment_dropped`/`explicit_option` warning.
    strip_comments: bool = false,
    /// Lossless conversion is in effect: unrepresentable values are preserved
    /// through a `$fig` envelope, so value losses are suppressed. (Comment
    /// losses still apply — the envelope path still prints comments normally.)
    lossless: bool = false,
    /// How deep `root_id` will sit in the document it is printed into: 0 for
    /// a whole document, the path's length for a value spliced at a path. A
    /// format's depth limit counts from the document's root, not the
    /// value's, so a mapping that is fine as a dotenv document is dropped as
    /// a dotenv value.
    depth: usize = 0,
};

/// Walk the subtree rooted at `root_id` as it would be serialized to `format`
/// and collect every lossy event. Warnings (and their `path` strings) are
/// allocated in `arena`; the returned slice is owned by the caller's arena.
pub fn analyze(arena: Allocator, ast: *const AST, root_id: AST.Node.Id, format: Format, options: Options) Allocator.Error![]Warning {
    return analyzeFor(arena, ast, root_id, .{ .compiled = format }, options);
}

/// `analyze` for any `Target`. A runtime language is held to what it
/// declares, which is what the CLI strips before printing to it: a
/// `lossless` declaration's missing kinds (a `null` dropped, an extended
/// scalar degraded), a `max_mapping_depth`'s flat-format drops, and its
/// `syntax`'s comment delimiters. What its printer does with a value, or a
/// comment, it declared nothing about is its own, and not reported.
pub fn analyzeFor(arena: Allocator, ast: *const AST, root_id: AST.Node.Id, target: Target, options: Options) Allocator.Error![]Warning {
    var c = Collector{ .ast = ast, .arena = arena, .target = target, .options = options };
    try c.walk(root_id, "", options.depth);
    return c.warnings.toOwnedSlice(arena);
}

/// What `firstDropped` found: a `null` the target has no spelling for, or a
/// container it cannot hold at that depth (a sequence, a table too deep).
pub const Dropped = enum { null_, container };

/// The first value in the subtree that `target` cannot hold at all once
/// spliced `depth` levels down — what an edit refuses rather than writing
/// text that reparses as something else (a mapping into dotenv as
/// `key=a=1`). Null when every value has a spelling there, degraded or not.
pub fn firstDropped(arena: Allocator, ast: *const AST, root_id: AST.Node.Id, target: Target, depth: usize) Allocator.Error!?Dropped {
    const warnings = try analyzeFor(arena, ast, root_id, target, .{ .depth = depth, .strip_comments = true });
    for (warnings) |w| if (w.code == .value_dropped)
        return if (std.mem.eql(u8, w.note, "null")) .null_ else .container;
    return null;
}

const Collector = struct {
    ast: *const AST,
    arena: Allocator,
    target: Target,
    options: Options,
    warnings: std.ArrayList(Warning) = .empty,

    fn add(self: *Collector, w: Warning) Allocator.Error!void {
        try self.warnings.append(self.arena, w);
    }

    /// Visit one node: check its own value + comments, then recurse into a
    /// container's children (building each child's path). `depth` is the
    /// node's depth in the printed document (`Options.depth` for the node
    /// `analyze` was called with), incrementing by one per container level
    /// — used by `valueLoss`'s INI arm, whose capability depends on nesting
    /// depth (root + one level of `[section]`s are fine; deeper is not) and
    /// not just root-vs-not like every other format needs.
    fn walk(self: *Collector, id: AST.Node.Id, path: []const u8, depth: usize) Allocator.Error!void {
        try self.checkComments(id, path);
        try self.checkValue(id, path, depth);
        switch (self.ast.nodes[id].kind) {
            .mapping => |first| {
                var child = first;
                while (child) |cid| : (child = self.ast.nodes[cid].next_sibling) {
                    const kv = self.ast.nodes[cid].kind.keyvalue;
                    const child_path = try self.keyPath(path, kv.key);
                    // A mapping entry's leading comment binds to its KEY node; its
                    // trailing/value losses ride the value node (checked in walk).
                    try self.checkComments(kv.key, child_path);
                    try self.walk(kv.value, child_path, depth + 1);
                }
            },
            .sequence => |first| {
                var child = first;
                var i: usize = 0;
                while (child) |cid| : (child = self.ast.nodes[cid].next_sibling) {
                    const child_path = try indexPath(self.arena, path, i);
                    i += 1;
                    try self.walk(cid, child_path, depth + 1);
                }
            },
            else => {},
        }
    }

    fn checkValue(self: *Collector, id: AST.Node.Id, path: []const u8, depth: usize) Allocator.Error!void {
        const kind = self.ast.nodes[id].kind;
        const loss = switch (self.target) {
            .compiled => |f| valueLoss(f, kind, depth),
            .runtime => |e| runtimeValueLoss(e, kind, depth),
        } orelse return;
        // Under lossless, unrepresentable values are enveloped, not lost.
        if (self.options.lossless) return;
        try self.add(.{ .code = loss.code, .cause = .format_limitation, .path = path, .note = loss.note });
    }

    fn checkComments(self: *Collector, id: AST.Node.Id, path: []const u8) Allocator.Error!void {
        const nc = self.ast.comments(id);
        if (nc.isEmpty()) return;
        // An explicit `strip_comments` is the user asking for the drop: that's the
        // actionable reason, so it wins even when the target also couldn't hold
        // the comment (a consumer filtering out `explicit_option` then correctly
        // stays silent — it's not a surprising loss).
        if (self.options.strip_comments) {
            try self.add(.{ .code = .comment_dropped, .cause = .explicit_option, .path = path });
            return;
        }
        // Otherwise, a format with no comment syntax here drops it outright.
        const emitted, const block = switch (self.target) {
            .compiled => |f| compiledComments(f, self.options.pretty),
            // A runtime language that declares no `syntax` has said nothing
            // about comments, and its printer receives every comment row, so
            // there is no loss to claim.
            .runtime => |e| runtimeComments(e, self.options.pretty) orelse return,
        };
        if (!emitted) {
            try self.add(.{ .code = .comment_dropped, .cause = .format_limitation, .path = path });
            return;
        }
        if (!block and hasBlock(nc)) {
            try self.add(.{ .code = .comment_style_degraded, .cause = .format_limitation, .path = path });
        }
    }

    /// `parent.key` (or `key` at the root). A non-string key shows as `?` (only
    /// its value can be a loss target, so the key text is a hint).
    fn keyPath(self: *Collector, parent: []const u8, key_id: AST.Node.Id) Allocator.Error![]const u8 {
        const name = switch (self.ast.nodes[key_id].kind) {
            .string => |s| s,
            else => "?",
        };
        if (parent.len == 0) return self.arena.dupe(u8, name);
        return std.fmt.allocPrint(self.arena, "{s}.{s}", .{ parent, name });
    }
};

fn indexPath(arena: Allocator, parent: []const u8, i: usize) Allocator.Error![]const u8 {
    return std.fmt.allocPrint(arena, "{s}[{d}]", .{ parent, i });
}

// ── Capability table ────────────────────────────────────────────────────────
//
// The single source of truth for what each target format can hold. Keyed on the
// full `SerializeFormat` (not the per-LANGUAGE `caps.lossless` declaration the
// envelope pass reads) because the JSON family splits: plain JSON drops
// comments and quotes non-finite floats, while JSON5 keeps comments and has
// native `Infinity`/`NaN`, and JSONC keeps comments but still quotes
// non-finite. For the three formats whose type system the lossless layer
// already models exactly (YAML/TOML/ZON), value capability is delegated to
// `Lossless.needsEnvelope`/`isUnrepresentable` over the language's declaration.

const Loss = struct { code: Warning.Code, note: []const u8 };

/// The value loss serializing `kind` to `format` would cause, or null if none.
/// Lossless suppression is the caller's concern (`checkValue`). `depth` is 0
/// for the node `analyze` was called with, incrementing per container level.
/// Read from the dialect's `prints` declaration and its language's
/// `max_mapping_depth` (`declaredLoss`); canonical, the lossless oracle,
/// holds every kind.
fn valueLoss(format: Format, kind: AST.Node.Kind, depth: usize) ?Loss {
    return switch (format) {
        .canonical => null,
        inline else => |f| declaredLoss(comptime printsOf(@tagName(f)), comptime maxDepthOf(@tagName(f)), kind, depth),
    };
}

/// The compiled dialect `name`'s `prints`, which every one whose language
/// serializes declares.
fn printsOf(comptime name: []const u8) Language.Prints {
    return Language.entryFor(name).prints orelse
        @compileError("'" ++ name ++ "' declares no `prints`, which the conversion warnings read");
}

fn maxDepthOf(comptime name: []const u8) ?u8 {
    return Language.moduleFor(name).Language.caps.max_mapping_depth;
}

/// The loss `prints` and `max_mapping_depth` say serializing `kind` at
/// `depth` causes. A flat format (one with `max_mapping_depth`) holds no
/// sequence, and no mapping past that depth.
fn declaredLoss(p: Language.Prints, max_depth: ?u8, kind: AST.Node.Kind, depth: usize) ?Loss {
    if (p.root_container and depth == 0) switch (kind) {
        .mapping, .sequence => {},
        else => return .{ .code = .value_dropped, .note = rootScalarNote(kind) },
    };
    switch (kind) {
        .null_ => if (!p.holds.null and !(p.null_at_root and depth == 0)) return .{ .code = .value_dropped, .note = "null" },
        .boolean, .number => if (!p.typed_scalars) return .{ .code = .type_degraded, .note = "string" },
        .extended => |e| if (!holdsExt(p.holds, e.kind)) return .{ .code = .type_degraded, .note = degradedNote(p, e.kind) },
        .sequence => if (max_depth != null) return .{ .code = .value_dropped, .note = "array" },
        .mapping => if (max_depth) |m| if (depth > m) return .{ .code = .value_dropped, .note = "table" },
        else => {},
    }
    return null;
}

fn holdsExt(holds: Language.NativeKinds, ext: ExtKind) bool {
    return switch (ext) {
        inline else => |k| @field(holds, @tagName(k)),
    };
}

/// `valueLoss` for a runtime language, from its declaration alone — the
/// same two declarations `printRuntime` strips by, so the warning and the
/// strip cannot disagree. `lossless` names the kinds it holds: a `null` it
/// lacks is dropped, an extended scalar it lacks reaches its printer as
/// text. `max_mapping_depth` makes it a flat format: no `null`, no sequence,
/// no mapping past the depth. Typed scalars are not reported degraded, as a
/// compiled flat format's are, because nothing in the declaration says so.
fn runtimeValueLoss(e: *const Runtime.Entry, kind: AST.Node.Kind, depth: usize) ?Loss {
    const caps = e.language.caps;
    if (e.prints) |p| return declaredLoss(p, caps.max_mapping_depth, kind, depth);
    if (caps.lossless) |native| {
        if (Lossless.isUnrepresentable(native, kind)) return .{ .code = .value_dropped, .note = dropNote(kind) };
        if (Lossless.needsEnvelope(native, kind)) switch (kind) {
            .extended => return .{ .code = .type_degraded, .note = "string" },
            else => {},
        };
    }
    if (caps.max_mapping_depth) |max| switch (kind) {
        .null_ => return .{ .code = .value_dropped, .note = "null" },
        .sequence => return .{ .code = .value_dropped, .note = "array" },
        .mapping => if (depth > max) return .{ .code = .value_dropped, .note = "table" },
        else => {},
    };
    return null;
}

/// Whether a runtime language writes comments at all, and whether it has a
/// spelling for a block one: its `syntax`'s own-line delimiter, and whether
/// that delimiter is a pair (`<!-- -->`) that can hold several lines. Null
/// when it declares no `syntax`, which is only required of an editable
/// language: a read-and-serialize one may print every
/// comment it is handed, and fig cannot know that it does not.
fn runtimeComments(e: *const Runtime.Entry, pretty: bool) ?struct { bool, bool } {
    const syntax = e.syntax orelse return null;
    const block = syntax.comments.block != null or (if (syntax.comments.line) |l| l.close.len > 0 else false);
    if (e.prints) |p| return .{ switch (p.comments) {
        .always => true,
        .pretty => pretty,
        .never => false,
    }, block };
    if (syntax.comments.line == null) return .{ false, false };
    return .{ true, block };
}

/// The type an `extended` value the target does not hold collapses to, as
/// its printer writes it: a char literal and a non-finite float as the
/// dialect declares, every other kind as a string.
fn degradedNote(p: Language.Prints, ext: ExtKind) []const u8 {
    return switch (ext) {
        .char_literal => p.char_as,
        .number_special => p.number_special_as,
        else => "string",
    };
}

/// Describes `kind` for the `value_dropped` note when it is fig's unique
/// root-position loss (see `valueLoss`'s `.fig` branch). Never called for
/// `.mapping`/`.sequence` (the caller already excludes those) or the
/// container-only `.keyvalue` kind, which never reaches a value-loss check.
fn rootScalarNote(kind: AST.Node.Kind) []const u8 {
    return switch (kind) {
        .null_ => "null",
        .boolean => "boolean",
        .number => "number",
        .string => "string",
        .alias => "alias",
        .extended => |e| switch (e.kind) {
            .enum_literal => "enum",
            .char_literal => "char",
            .number_special => "number",
            .offset_datetime, .local_datetime, .local_date, .local_time => "datetime",
            .plist_date => "date",
            .plist_data => "data",
        },
        .mapping, .sequence, .keyvalue => unreachable,
    };
}

fn dropNote(kind: AST.Node.Kind) []const u8 {
    return switch (kind) {
        .null_ => "null",
        else => "",
    };
}

/// Whether `format` writes comments at all in this mode, as its dialect's
/// `prints.comments` says, and whether it has a spelling for a block one: a
/// declared `block` comment, or a paired own-line one. Canonical writes both.
fn compiledComments(format: Format, pretty: bool) struct { bool, bool } {
    return switch (format) {
        .canonical => .{ true, true },
        inline else => |f| blk: {
            const p = comptime printsOf(@tagName(f));
            const emitted = switch (p.comments) {
                .always => true,
                .pretty => pretty,
                .never => false,
            };
            const c = comptime commentsOf(@tagName(f));
            const block = c.block != null or (if (c.line) |l| l.close.len > 0 else false);
            break :blk .{ emitted, block };
        },
    };
}

/// The compiled dialect `name`'s declared comment syntax, read from its
/// language's own row so that a language compiled out still answers.
fn commentsOf(comptime name: []const u8) Language.Comments {
    const L = Language.moduleFor(name).Language;
    inline for (L.dialects) |d| {
        if (comptime std.mem.eql(u8, d.name, name)) return L.syntax(d.dialect).comments;
    }
    unreachable;
}

fn hasBlock(nc: AST.NodeComments) bool {
    for (nc.leading) |c| if (c.style == .block) return true;
    for (nc.dangling) |c| if (c.style == .block) return true;
    if (nc.trailing) |t| if (t.style == .block) return true;
    return false;
}

// ── Tests ───────────────────────────────────────────────────────────────────

const testing = std.testing;

/// Build a small AST via the Builder, run `analyze`, and return the warnings in
/// an arena the caller owns.
fn analyzeBuilt(arena: Allocator, build: anytype, format: Format, options: Options) ![]Warning {
    var b = AST.Builder.init(arena);
    const root = try build(&b);
    var ast = try b.finish(root);
    return analyze(arena, &ast, ast.root, format, options);
}

test "null dropped for TOML, not for JSON/YAML" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    const Build = struct {
        fn f(b: *AST.Builder) !AST.Node.Id {
            const k = try b.addString("a");
            const v = try b.addNull();
            return b.addMapping(&.{.{ .key = k, .value = v }});
        }
    };

    const toml = try analyzeBuilt(arena, Build.f, .toml, .{});
    try testing.expectEqual(@as(usize, 1), toml.len);
    try testing.expectEqual(Warning.Code.value_dropped, toml[0].code);
    try testing.expectEqualStrings("a", toml[0].path);

    const json = try analyzeBuilt(arena, Build.f, .json, .{});
    try testing.expectEqual(@as(usize, 0), json.len);
}

test "datetime degrades to JSON, canonical to TOML" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    const Build = struct {
        fn f(b: *AST.Builder) !AST.Node.Id {
            return b.addExtended(.offset_datetime, "1979-05-27T07:32:00Z");
        }
    };

    const json = try analyzeBuilt(arena, Build.f, .json, .{});
    try testing.expectEqual(@as(usize, 1), json.len);
    try testing.expectEqual(Warning.Code.type_degraded, json[0].code);
    try testing.expectEqualStrings("string", json[0].note);

    const toml = try analyzeBuilt(arena, Build.f, .toml, .{});
    try testing.expectEqual(@as(usize, 0), toml.len);
}

test "non-finite float degrades to JSON but not JSON5" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    const Build = struct {
        fn f(b: *AST.Builder) !AST.Node.Id {
            return b.addExtended(.number_special, "Infinity");
        }
    };

    const json = try analyzeBuilt(arena, Build.f, .json, .{});
    try testing.expectEqual(@as(usize, 1), json.len);
    try testing.expectEqualStrings("quoted string", json[0].note);

    const json5 = try analyzeBuilt(arena, Build.f, .json5, .{});
    try testing.expectEqual(@as(usize, 0), json5.len);
}

test "char literal is native to fig, degrades to a number elsewhere" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    // Wrapped in a mapping (not bare at the root) so this exercises the
    // char-literal KIND's capability, not fig's separate root-position rule
    // (see "fig root scalar is flagged as unrepresentable" below).
    const Build = struct {
        fn f(b: *AST.Builder) !AST.Node.Id {
            const k = try b.addString("a");
            const v = try b.addExtended(.char_literal, "65"); // 'A'
            return b.addMapping(&.{.{ .key = k, .value = v }});
        }
    };

    // fig re-emits it via `: char =` — no loss.
    const fig = try analyzeBuilt(arena, Build.f, .fig, .{});
    try testing.expectEqual(@as(usize, 0), fig.len);

    // JSON has no char; it collapses to the codepoint as a number.
    const json = try analyzeBuilt(arena, Build.f, .json, .{});
    try testing.expectEqual(@as(usize, 1), json.len);
    try testing.expectEqual(Warning.Code.type_degraded, json[0].code);
    try testing.expectEqualStrings("number", json[0].note);
}

test "fig root scalar is flagged as unrepresentable, a root map/sequence is not" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    // A bare number sitting at the document root: the fig dialect's root must
    // be a map or a sequence (see `languages/fig/printer.zig`'s `root`, which
    // hard-errors with `UnrepresentableRoot` for exactly this AST shape) —
    // this is defense in depth so `--strict` catches it independent of the
    // printer's hard error.
    const ScalarRoot = struct {
        fn f(b: *AST.Builder) !AST.Node.Id {
            return b.addNumberRaw("42", false);
        }
    };
    const scalar = try analyzeBuilt(arena, ScalarRoot.f, .fig, .{});
    try testing.expectEqual(@as(usize, 1), scalar.len);
    try testing.expectEqual(Warning.Code.value_dropped, scalar[0].code);
    try testing.expectEqual(Warning.Cause.format_limitation, scalar[0].cause);
    try testing.expectEqualStrings("", scalar[0].path);
    try testing.expectEqualStrings("number", scalar[0].note);

    // Other formats have no such rule — a bare JSON number root is fine.
    const json = try analyzeBuilt(arena, ScalarRoot.f, .json, .{});
    try testing.expectEqual(@as(usize, 0), json.len);

    // A root mapping is unaffected — still fully representable.
    const MapRoot = struct {
        fn f(b: *AST.Builder) !AST.Node.Id {
            const k = try b.addString("a");
            const v = try b.addNumberRaw("1", false);
            return b.addMapping(&.{.{ .key = k, .value = v }});
        }
    };
    const map_fig = try analyzeBuilt(arena, MapRoot.f, .fig, .{});
    try testing.expectEqual(@as(usize, 0), map_fig.len);
}

test "lossless suppresses value warnings" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    const Build = struct {
        fn f(b: *AST.Builder) !AST.Node.Id {
            const k = try b.addString("a");
            const v = try b.addNull();
            return b.addMapping(&.{.{ .key = k, .value = v }});
        }
    };

    const toml = try analyzeBuilt(arena, Build.f, .toml, .{ .lossless = true });
    try testing.expectEqual(@as(usize, 0), toml.len);
}

test "comment dropped, style-degraded, and explicitly stripped" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    const Build = struct {
        fn f(b: *AST.Builder) !AST.Node.Id {
            const k = try b.addString("a");
            const v = try b.addNumberRaw("1", false);
            const m = try b.addMapping(&.{.{ .key = k, .value = v }});
            // A block comment leads the entry (binds to the key node).
            try b.setComments(k, .{ .leading = &.{.{ .text = "note", .style = .block }} });
            return m;
        }
    };

    // YAML emits comments but has no block syntax → style degraded.
    const yaml = try analyzeBuilt(arena, Build.f, .yaml, .{});
    try testing.expectEqual(@as(usize, 1), yaml.len);
    try testing.expectEqual(Warning.Code.comment_style_degraded, yaml[0].code);
    try testing.expectEqualStrings("a", yaml[0].path);

    // Plain JSON has no comments at all → dropped, format limitation.
    const json = try analyzeBuilt(arena, Build.f, .json, .{});
    try testing.expectEqual(@as(usize, 1), json.len);
    try testing.expectEqual(Warning.Code.comment_dropped, json[0].code);
    try testing.expectEqual(Warning.Cause.format_limitation, json[0].cause);

    // JSON5 keeps block comments → no warning.
    const json5 = try analyzeBuilt(arena, Build.f, .json5, .{});
    try testing.expectEqual(@as(usize, 0), json5.len);

    // Explicit strip on a format that would otherwise keep it → explicit_option.
    const stripped = try analyzeBuilt(arena, Build.f, .json5, .{ .strip_comments = true });
    try testing.expectEqual(@as(usize, 1), stripped.len);
    try testing.expectEqual(Warning.Code.comment_dropped, stripped[0].code);
    try testing.expectEqual(Warning.Cause.explicit_option, stripped[0].cause);

    // Explicit strip wins even when the target ALSO can't hold comments: the user
    // asked for the drop, so it's `explicit_option`, not `format_limitation`.
    const stripped_json = try analyzeBuilt(arena, Build.f, .json, .{ .strip_comments = true });
    try testing.expectEqual(@as(usize, 1), stripped_json.len);
    try testing.expectEqual(Warning.Cause.explicit_option, stripped_json[0].cause);
}

test "a runtime target is warned about comments only by what its syntax declares" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    var b = AST.Builder.init(arena);
    const k = try b.addString("a");
    const v = try b.addNumberRaw("1", false);
    const m = try b.addMapping(&.{.{ .key = k, .value = v }});
    try b.setComments(k, .{ .leading = &.{.{ .text = "note", .style = .line }} });
    var ast = try b.finish(m);

    const manifest = @import("kit/manifest.zig");
    var reg: Runtime.Registered = .{ .vt = undefined, .name = "t", .caps = .{ .serialize = true }, .syntax = null, .samples = &.{}, .entries = &.{} };
    var entry: Runtime.Entry = .{ .index = 0, .abi = 0, .language = &reg, .name = "t", .extensions = &.{}, .splice = .literal, .empty_doc_seed = null, .syntax = null };

    // No `syntax`: nothing said about comments, and the printer is handed
    // every one — so no loss is claimed.
    try testing.expectEqual(@as(usize, 0), (try analyzeFor(arena, &ast, ast.root, .{ .runtime = &entry }, .{})).len);

    // A `syntax` with no comment delimiter declares there are none: dropped.
    entry.syntax = .{ .comments = .{ .line = null, .trailing = null }, .kv_sep = null, .empty_map_literal = null };
    const dropped = try analyzeFor(arena, &ast, ast.root, .{ .runtime = &entry }, .{});
    try testing.expectEqual(@as(usize, 1), dropped.len);
    try testing.expectEqual(Warning.Code.comment_dropped, dropped[0].code);

    // One with a delimiter writes them: no warning.
    entry.syntax = .{ .comments = manifest.Comments.hash, .kv_sep = null, .empty_map_literal = null };
    try testing.expectEqual(@as(usize, 0), (try analyzeFor(arena, &ast, ast.root, .{ .runtime = &entry }, .{})).len);
}

test "path shapes match lossyStrip (dotted keys, bracket indices)" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    // { b: null, c: [1, null], d: { e: null } } → toml
    const Build = struct {
        fn f(b: *AST.Builder) !AST.Node.Id {
            const bk = try b.addString("b");
            const bv = try b.addNull();

            const c1 = try b.addNumberRaw("1", false);
            const c2 = try b.addNull();
            const cseq = try b.addSequence(&.{ c1, c2 });
            const ck = try b.addString("c");

            const ek = try b.addString("e");
            const ev = try b.addNull();
            const inner = try b.addMapping(&.{.{ .key = ek, .value = ev }});
            const dk = try b.addString("d");

            return b.addMapping(&.{
                .{ .key = bk, .value = bv },
                .{ .key = ck, .value = cseq },
                .{ .key = dk, .value = inner },
            });
        }
    };

    const w = try analyzeBuilt(arena, Build.f, .toml, .{});
    try testing.expectEqual(@as(usize, 3), w.len);
    try testing.expectEqualStrings("b", w[0].path);
    try testing.expectEqualStrings("c[1]", w[1].path);
    try testing.expectEqualStrings("d.e", w[2].path);
}

test "a depth limit counts from the document's root, not the analyzed value's" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    var b = AST.Builder.init(arena);
    const k = try b.addString("x");
    const v = try b.addString("1");
    const map = try b.addMapping(&.{.{ .key = k, .value = v }});
    var ast = try b.finish(map);

    // As a whole dotenv document the mapping is the root, and fine; as a
    // value one level down it is a table dotenv cannot hold.
    try testing.expectEqual(@as(usize, 0), (try analyze(arena, &ast, ast.root, .dotenv, .{})).len);
    const nested = try analyze(arena, &ast, ast.root, .dotenv, .{ .depth = 1 });
    try testing.expectEqual(@as(usize, 1), nested.len);
    try testing.expectEqual(Warning.Code.value_dropped, nested[0].code);

    // INI holds one level of `[section]`s, so the same value is fine one
    // level down and dropped two.
    try testing.expectEqual(@as(?Dropped, null), try firstDropped(arena, &ast, ast.root, .{ .compiled = .ini }, 1));
    try testing.expectEqual(@as(?Dropped, .container), try firstDropped(arena, &ast, ast.root, .{ .compiled = .ini }, 2));
    // A degraded value is not a dropped one: dotenv writes the number as text.
    var num_ast = ast;
    num_ast.root = v;
    try testing.expectEqual(@as(?Dropped, null), try firstDropped(arena, &num_ast, num_ast.root, .{ .compiled = .dotenv }, 1));
}

test "firstDropped tells a null from a container" {
    var arena_state = std.heap.ArenaAllocator.init(testing.allocator);
    defer arena_state.deinit();
    const arena = arena_state.allocator();

    var b = AST.Builder.init(arena);
    const n = try b.addNull();
    var ast = try b.finish(n);
    try testing.expectEqual(@as(?Dropped, .null_), try firstDropped(arena, &ast, ast.root, .{ .compiled = .toml }, 1));
    try testing.expectEqual(@as(?Dropped, null), try firstDropped(arena, &ast, ast.root, .{ .compiled = .yaml }, 1));
}
