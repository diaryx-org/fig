//! A Zig language as the contract sees it: the vtable a language fills to
//! register, built from the declarations and functions a Zig language module
//! already has (`docs/proposals/core-only.md` §5.1).
//!
//! `VTable(L, prefix)` reads `L` — a module's `Language`: its `name`,
//! `caps`, `dialects`, `syntax`, `samples` and renderers — into the records
//! of `abi.zig`, and its thunks reach `L` through the contract alone. `parse`
//! runs `L`'s parser into its own tree and emits that tree into the table fig
//! handed it (`convert.emitDocument`); `print` reads the view fig hands it
//! back into a tree (`convert.viewToDocument`) and runs `L`'s printer.
//! `prefix` is prepended to every dialect name, so that a test can register
//! a language beside the compiled format of the same name.
//!
//! The thunks write the table through its own methods, so they run in the
//! image that owns it — a language registered in the same process as core.
//! A language built into an archive of its own writes it through the
//! `fig_table_*` calls instead.

const std = @import("std");
const builtin = @import("builtin");
const abi = @import("abi.zig");
const manifest = @import("manifest.zig");
const table = @import("table.zig");
const convert = @import("convert.zig");
const Document = @import("document.zig");
const AST = @import("ast/ast.zig");

/// The working memory of a thunk that has no table to borrow from, and of
/// the bytes it hands back through `free_bytes`.
const gpa = if (builtin.cpu.arch.isWasm()) std.heap.wasm_allocator else std.heap.page_allocator;

/// `s` with a NUL after it, at comptime.
fn z(comptime s: []const u8) [*:0]const u8 {
    return std.fmt.comptimePrint("{s}", .{s});
}

fn zOpt(comptime s: ?[]const u8) ?[*:0]const u8 {
    return if (s) |x| z(x) else null;
}

fn delimiter(comptime d: ?manifest.CommentDelimiter) abi.CommentDelimiterDesc {
    const x = d orelse return .{};
    return .{ .open = z(x.open), .close = if (x.close.len == 0) null else z(x.close), .forbidden = zOpt(x.forbidden) };
}

/// `manifest.Syntax` as the C record states it.
pub fn syntaxDesc(comptime s: manifest.Syntax) abi.SyntaxDesc {
    return .{
        .comments = .{ .line = delimiter(s.comments.line), .trailing = delimiter(s.comments.trailing), .block = delimiter(s.comments.block) },
        .kv_sep = zOpt(s.kv_sep),
        .flow_kv_sep_from_siblings = s.flow_kv_sep_from_siblings,
        .flow_map_pad = z(s.flow_map_pad),
        .key_sigil = s.key_sigil orelse 0,
        .empty_map_literal = zOpt(s.empty_map_literal),
        .block_seq_editable = s.block_seq_editable,
        .flow_containers = s.flow_containers,
        .indent_unit = z(s.indent_unit),
        .seq_item_marker = z(s.seq_item_marker),
        .closed_containers = if (s.closed_containers) |c| .{
            .map_open = z(c.map.open),
            .map_close = z(c.map.close),
            .seq_open = z(c.seq.open),
            .seq_close = z(c.seq.close),
        } else .{},
        .single_line_block_mapping = s.single_line_block_mapping,
        .bare_document_mapping = s.bare_document_mapping,
        .flow_map_open = z(s.flow_map_open),
        .flow_map_close = z(s.flow_map_close),
        .structural_indent = s.structural_indent,
        .section_noun = zOpt(s.section_noun),
        .section_header = if (s.section_header) |h| .{
            .open = z(h.open),
            .close = z(h.close),
            .seq_open = zOpt(h.seq_open),
            .seq_close = zOpt(h.seq_close),
            .sep = z(h.sep),
            .skip_index = h.skip_index,
        } else .{},
        .merge_key = zOpt(s.merge_key),
        .flow_entry_sep = z(s.flow_entry_sep),
        .flow_root = s.flow_root,
        .flow_maps_only = s.flow_maps_only,
        .line_prefix_marks = z(s.line_prefix_marks),
    };
}

fn eql(a: []const u8, b: []const u8) bool {
    return std.mem.eql(u8, a, b);
}

/// The contract vtable of the Zig language `L`, its dialects named
/// `prefix ++ name`.
pub fn VTable(comptime L: type, comptime prefix: []const u8) type {
    return struct {
        const editable = L.caps.edit;

        /// The dialect row named `name`, as this vtable names it.
        fn typeOf(dialect: [*:0]const u8) ?L.Type {
            @setEvalBranchQuota(100_000);
            const n = std.mem.span(dialect);
            inline for (L.dialects) |d| {
                if (eql(prefix ++ d.name, n)) return d.dialect;
            }
            return null;
        }

        /// Every extension of `L` goes to the dialect named after it, else
        /// to the first.
        fn extensionsOf(comptime row: usize) []const ?[*:0]const u8 {
            @setEvalBranchQuota(100_000);
            comptime var out: []const ?[*:0]const u8 = &.{};
            inline for (L.extensions) |e| {
                comptime var owner: usize = 0;
                inline for (L.dialects, 0..) |d, i| {
                    if (comptime eql(d.name, e)) owner = i;
                }
                if (owner == row) out = out ++ [_]?[*:0]const u8{z(e)};
            }
            return out ++ [_]?[*:0]const u8{null};
        }

        const dialect_syntaxes = blk: {
            @setEvalBranchQuota(100_000);
            var out: [L.dialects.len]abi.SyntaxDesc = undefined;
            if (editable) {
                for (L.dialects, 0..) |d, i| out[i] = syntaxDesc(L.syntax(d.dialect));
            }
            break :blk out;
        };

        const dialects = blk: {
            @setEvalBranchQuota(100_000);
            var out: [L.dialects.len]abi.DialectDesc = undefined;
            for (L.dialects, 0..) |d, i| {
                const p = d.prints;
                var flags: u32 = 0;
                if (p) |x| {
                    if (!x.typed_scalars) flags |= abi.prints_text_scalars;
                    if (x.null_at_root) flags |= abi.prints_null_at_root;
                    if (x.root_container) flags |= abi.prints_root_container;
                }
                out[i] = .{
                    .name = z(prefix ++ d.name),
                    .extensions = extensionsOf(i).ptr,
                    .splice = @backingInt(d.splice),
                    .empty_doc_seed = zOpt(d.empty_doc_seed),
                    .syntax = if (editable) &dialect_syntaxes[i] else null,
                    .prints_holds = if (p) |x| abi.losslessBits(x.holds) else 0,
                    .prints_flags = flags,
                    .prints_char_as = if (p) |x| z(x.char_as) else null,
                    .prints_number_special_as = if (p) |x| z(x.number_special_as) else null,
                    .prints_comments = if (p) |x| @backingInt(x.comments) else 0,
                };
            }
            break :blk out;
        };

        const samples = blk: {
            const src: []const []const u8 = if (@hasDecl(L, "samples")) L.samples else &.{};
            var out: [src.len]abi.Str = undefined;
            for (src, 0..) |x, i| out[i] = abi.Str.of(x);
            break :blk out;
        };

        fn fail(err: *abi.ErrorInfo, message: []const u8) c_int {
            err.set(message);
            return 2;
        }

        const P = L.Parser;
        const reports = @hasDecl(P, "parseWithReport");

        fn parse(ctx: ?*anyopaque, dialect: [*:0]const u8, input: abi.Str, out: *table.Table, err: *abi.ErrorInfo) callconv(.c) c_int {
            _ = ctx;
            const t = typeOf(dialect) orelse return fail(err, "no such dialect");
            const src = input.slice() orelse "";
            if (comptime !reports) {
                var p: L.Parser = .{ .allocator = out.allocator };
                var doc = L.parse(&p, src, t) catch |e| return fail(err, @errorName(e));
                defer doc.deinit(out.allocator);
                convert.emitDocument(out, &doc) catch return 3;
                return 0;
            }
            // A parser with a report says why it refused, where, and what it
            // would warn about on input it reads: each goes into the table's
            // diagnostics, and a refusal's into `err` as well.
            var report: P.Report = .{};
            const parsed = P.parseWithReport(out.allocator, src, t, &report);
            defer if (comptime @hasField(P.Report, "warnings")) out.allocator.free(report.warnings);
            var doc = parsed catch |e| {
                if (report.diag) |d| {
                    addDiagnostic(out, table.diagnostic_error, d.offset, d.end, P.describe(d.code), P.shortLabel(d.code)) catch return 3;
                    err.set(P.describe(d.code));
                    err.byte_offset = d.offset;
                    return 2;
                }
                return fail(err, @errorName(e));
            };
            defer doc.deinit(out.allocator);
            convert.emitDocument(out, &doc) catch return 3;
            if (comptime @hasField(P.Report, "warnings") and @hasDecl(P, "Warning")) {
                for (report.warnings) |w| addDiagnostic(out, table.diagnostic_warning, w.offset, w.end, P.Warning.describeWarning(w.code), P.Warning.shortLabel(w.code)) catch return 3;
            }
            return 0;
        }

        fn addDiagnostic(out: *table.Table, severity: u8, offset: usize, end: ?usize, message: []const u8, label: []const u8) !void {
            try out.diagnostics.append(out.allocator, .{
                .severity = severity,
                .start = @intCast(offset),
                .end = if (end) |e| @intCast(e) else table.none,
                .message = try out.addString(message),
                .label = try out.addString(label),
            });
        }

        fn print(ctx: ?*anyopaque, dialect: [*:0]const u8, view: *const table.View, options: *const abi.PrintOptions, out: *abi.Str, err: *abi.ErrorInfo) callconv(.c) c_int {
            _ = ctx;
            const t = typeOf(dialect) orelse return fail(err, "no such dialect");
            var arena = std.heap.ArenaAllocator.init(gpa);
            defer arena.deinit();
            const doc = convert.viewToDocument(arena.allocator(), "", view, .absent) catch return fail(err, "a malformed table");
            const opts: AST.SerializeOptions = .{
                .pretty = options.pretty,
                .strip_comments = options.strip_comments,
                .indent = options.indent,
                .width = options.width,
                .splice = options.splice,
                .flow = options.flow,
            };
            var w: std.Io.Writer.Allocating = .init(gpa);
            defer w.deinit();
            printWith(t, &w.writer, &doc.ast, opts) catch |e| return fail(err, @errorName(e));
            out.* = abi.Str.of(gpa.dupe(u8, w.written()) catch return 3);
            return 0;
        }

        /// The printer of the dialect `t`: its splice printer for a value the
        /// editor splices, its fragment printer for a fragment, else its
        /// document printer.
        fn printWith(t: L.Type, w: *std.Io.Writer, ast: *const AST, options: AST.SerializeOptions) !void {
            inline for (L.dialects) |d| {
                if (d.dialect == t) {
                    if (comptime @hasDecl(L.Printer, "printSplice")) {
                        if (options.splice) return L.Printer.printSplice(w, ast, options);
                    }
                    // A fragment: a value the editor splices (`splice`, and
                    // `flow`, which only a fragment reads), or a root a whole
                    // document cannot have.
                    if (comptime d.print_fragment_name) |fragment| {
                        const scalar_root = switch (ast.nodes[ast.root].kind) {
                            .mapping, .sequence => false,
                            else => true,
                        };
                        if (options.splice or options.flow or scalar_root) return @field(L.Printer, fragment)(w, ast, options);
                    }
                    return @field(L.Printer, d.print_name)(w, ast, options);
                }
            }
            return error.FormatDisabled;
        }

        fn freeBytes(ctx: ?*anyopaque, bytes: abi.Str) callconv(.c) void {
            _ = ctx;
            if (bytes.slice()) |s| if (s.len > 0) gpa.free(s);
        }

        fn renderThunk(comptime which: manifest.Renderer) abi.RenderFn {
            return struct {
                fn f(ctx: ?*anyopaque, r: *const abi.RenderRequest, out: *abi.Str, err: *abi.ErrorInfo) callconv(.c) c_int {
                    _ = ctx;
                    const t = typeOf(r.dialect) orelse return fail(err, "no such dialect");
                    var list: std.ArrayList(u8) = .empty;
                    defer list.deinit(gpa);
                    const request: manifest.RenderRequest = .{
                        .indent = r.indent.slice() orelse "",
                        .key = r.key.slice() orelse "",
                        .value = r.value.slice() orelse "",
                        .literal = manifest.Literal.parse(std.mem.span(r.literal)) orelse .string,
                        .old_key = r.old_key.slice() orelse "",
                        .parent_key = r.parent_key.slice() orelse "",
                        .parent_tag = r.parent_tag.slice() orelse "",
                    };
                    @field(L, which.declName())(t, gpa, &list, request) catch |e| {
                        err.set(@errorName(e));
                        return 6;
                    };
                    out.* = abi.Str.of(list.toOwnedSlice(gpa) catch return 3);
                    return 0;
                }
            }.f;
        }

        fn renderer(comptime which: manifest.Renderer) ?abi.RenderFn {
            return if (@hasDecl(L, which.declName())) renderThunk(which) else null;
        }

        fn splitStream(ctx: ?*anyopaque, dialect: [*:0]const u8, input: abi.Str, out: *abi.Str, err: *abi.ErrorInfo) callconv(.c) c_int {
            _ = ctx;
            const t = typeOf(dialect) orelse return fail(err, "no such dialect");
            const docs = L.splitStream(gpa, input.slice() orelse "", t) catch |e| return fail(err, @errorName(e));
            defer gpa.free(docs);
            const size = @sizeOf(abi.StreamDoc);
            const bytes = gpa.alloc(u8, docs.len * size) catch return 3;
            for (docs, 0..) |d, i| {
                const row: abi.StreamDoc = .{ .start = d.start, .end = d.end, .marked = d.explicit };
                @memcpy(bytes[i * size ..][0..size], std.mem.asBytes(&row));
            }
            out.* = abi.Str.of(bytes);
            return 0;
        }

        fn capsBits() u32 {
            var bits: u32 = 0;
            if (L.caps.read) bits |= abi.cap_read;
            if (L.caps.edit) bits |= abi.cap_edit;
            if (L.caps.serialize) bits |= abi.cap_serialize;
            if (L.caps.references) bits |= abi.cap_references;
            return bits;
        }

        const language_syntax: ?*const abi.SyntaxDesc = if (editable) &dialect_syntaxes[0] else null;

        pub const vtable: abi.VTable = .{
            .version = abi.vtable_version,
            .name = z(prefix ++ L.dialects[0].name),
            .caps = capsBits(),
            .max_mapping_depth = if (L.caps.max_mapping_depth) |m| m else abi.no_depth_limit,
            .lossless = abi.losslessBits(L.caps.lossless),
            .syntax = language_syntax,
            .dialects = &dialects,
            .dialect_count = dialects.len,
            .samples = &samples,
            .sample_count = samples.len,
            .parse = parse,
            .print = if (L.caps.serialize) print else null,
            .free_bytes = freeBytes,
            .render_value = renderer(.value),
            .render_entry = renderer(.entry),
            .render_item = renderer(.item),
            .render_tail = renderer(.tail),
            .render_key = renderer(.key),
            .render_key_name = renderer(.key_name),
            .split_stream = if (@hasDecl(L, "splitStream")) splitStream else null,
        };
    };
}
