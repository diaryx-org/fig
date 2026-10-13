//! A Zig language as a static library of its own (`libfig_<name>.a`),
//! linked beside fig's (`libfig.a`) by a C or Rust program: the one export
//! `fig_<name>_register`, which registers the language through
//! `fig_language_register` as any C host's language is
//! (`bindings/c/include/fig/<name>.h`).
//!
//! The library carries its own copy of the kit, a separate compilation from
//! fig's, so the table fig hands a parse is opaque here: the language parses
//! into a table of its own and each list of it is copied into fig's through
//! `fig_table_extend`, the rows as they stand, since every offset in them is
//! into the input or the buffer, which both hold. A print is handed a
//! `FigTableView`, an extern record, and reads it in place.

const std = @import("std");
const abi = @import("abi.zig");
const table = @import("table.zig");
const contract = @import("contract.zig");

/// fig's C ABI, resolved by `libfig.a` at link.
extern fn fig_language_register(vt: *const abi.VTable, out_format: *c_int, out_err: ?*abi.ErrorInfo) c_int;
extern fn fig_table_extend(t: *anyopaque, which: c_int, n: usize) ?*anyopaque;
extern fn fig_format_by_name(name: [*:0]const u8) c_int;

/// The exports for the Zig language `L`, registered as `fig_<name>_register`.
pub fn Exports(comptime L: type, comptime name: []const u8) type {
    return struct {
        const V = contract.VTable(L, "");

        const vtable: abi.VTable = blk: {
            var v = V.vtable;
            v.parse = parse;
            break :blk v;
        };

        /// A spin lock: no C entry point has the `Io` a mutex wants, and a
        /// registration is rare.
        var locked: std.atomic.Value(bool) = .init(false);

        comptime {
            @export(&register, .{ .name = "fig_" ++ name ++ "_register" });
        }

        /// Register the language. On success, `*out_format` is the format
        /// of its first dialect; otherwise `out_err` says why. Registered
        /// already — a library and the program using it each registering
        /// what they use — it is the format it already has.
        fn register(out_format: *c_int, out_err: ?*abi.ErrorInfo) callconv(.c) c_int {
            // Two threads registering at once: the second waits, then finds
            // the first's.
            while (locked.swap(true, .acquire)) std.atomic.spinLoopHint();
            defer locked.store(false, .release);
            const known = fig_format_by_name(vtable.name);
            if (known >= 0) {
                out_format.* = known;
                return 0;
            }
            return fig_language_register(&vtable, out_format, out_err);
        }

        /// Parse into a table of this library's own, then copy each of its
        /// lists into `out`, fig's — diagnostics too when the parse refuses.
        fn parse(ctx: ?*anyopaque, dialect: [*:0]const u8, input: abi.Str, out: *table.Table, err: *abi.ErrorInfo) callconv(.c) c_int {
            var local = table.Table.init(contract.gpa, input.slice() orelse "");
            defer local.deinit();
            const rc = V.vtable.parse(ctx, dialect, input, &local, err);
            const lists = .{ local.rows.items, local.strings.items, local.anchors.items, local.tags.items, local.markers.items, local.seps.items, local.item_seps.items, local.comments.items, local.regions.items, local.mentions.items, local.directives.items, local.diagnostics.items };
            inline for (lists, 0..) |items, which| {
                if (items.len > 0) {
                    const at = fig_table_extend(@ptrCast(out), which, items.len) orelse return 3;
                    const bytes = std.mem.sliceAsBytes(items);
                    @memcpy(@as([*]u8, @ptrCast(at))[0..bytes.len], bytes);
                }
            }
            return rc;
        }
    };
}
