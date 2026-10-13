//! A language's description as the helper wire carries it
//! (docs/helper-protocol.md, "The description"): the JSON a helper answers
//! `describe` with, written from the vtable records a Zig language builds
//! (`contract.zig`). The inverse of fig's `src/contract/wire.zig`'s
//! `vtableFromDescription`, field for field, so a Zig language built into a
//! module of its own — a wasm module beside fig's — can describe itself to
//! fig the way any helper does.

const std = @import("std");
const Writer = std.Io.Writer;
const abi = @import("abi.zig");
const ExtKind = @import("ast/ast.zig").Node.Kind.Extended.ExtKind;

/// Write `vt`'s description.
pub fn write(w: *Writer, vt: *const abi.VTable) Writer.Error!void {
    try w.writeAll("{\"name\":");
    try str(w, std.mem.span(vt.name));
    try w.print(",\"caps\":{{\"read\":{},\"edit\":{},\"serialize\":{},\"references\":{}}}", .{
        vt.caps & abi.cap_read != 0,
        vt.caps & abi.cap_edit != 0,
        vt.caps & abi.cap_serialize != 0,
        vt.caps & abi.cap_references != 0,
    });
    try w.writeAll(",\"max_mapping_depth\":");
    if (vt.max_mapping_depth == abi.no_depth_limit) try w.writeAll("null") else try w.print("{d}", .{vt.max_mapping_depth});
    try w.writeAll(",\"lossless\":");
    if (vt.lossless & abi.lossless_envelope == 0) try w.writeAll("null") else try kinds(w, vt.lossless);
    try w.writeAll(",\"syntax\":");
    if (vt.syntax) |s| try syntax(w, s) else try w.writeAll("null");

    try w.writeAll(",\"dialects\":[");
    for (vt.dialects[0..vt.dialect_count], 0..) |d, i| {
        if (i > 0) try w.writeByte(',');
        try dialect(w, &d);
    }
    try w.writeAll("],\"samples\":[");
    for (vt.samples[0..vt.sample_count], 0..) |s, i| {
        if (i > 0) try w.writeByte(',');
        try str(w, s.slice() orelse "");
    }
    try w.writeAll("],\"renderers\":[");
    var first = true;
    inline for (.{ "value", "entry", "item", "tail", "key", "key_name" }) |name| {
        if (@field(vt, "render_" ++ name) != null) {
            if (!first) try w.writeByte(',');
            first = false;
            try w.writeAll("\"" ++ name ++ "\"");
        }
    }
    try w.print("],\"split_stream\":{}}}", .{vt.split_stream != null});
}

fn dialect(w: *Writer, d: *const abi.DialectDesc) Writer.Error!void {
    try w.writeAll("{\"name\":");
    try str(w, std.mem.span(d.name));
    try w.writeAll(",\"extensions\":[");
    if (d.extensions) |exts| {
        var i: usize = 0;
        while (exts[i]) |e| : (i += 1) {
            if (i > 0) try w.writeByte(',');
            try str(w, std.mem.span(e));
        }
    }
    try w.print("],\"splice\":\"{s}\"", .{switch (d.splice) {
        1 => "escaped",
        2 => "raw",
        else => "literal",
    }});
    try w.writeAll(",\"empty_doc_seed\":");
    try optStr(w, d.empty_doc_seed);
    try w.writeAll(",\"syntax\":");
    if (d.syntax) |s| try syntax(w, s) else try w.writeAll("null");
    try w.writeAll(",\"prints\":");
    if (d.prints_holds & abi.lossless_envelope == 0) {
        try w.writeAll("null");
    } else {
        try w.writeAll("{\"holds\":");
        try kinds(w, d.prints_holds);
        try w.print(",\"typed_scalars\":{},\"null_at_root\":{},\"root_container\":{},\"char_as\":", .{
            d.prints_flags & abi.prints_text_scalars == 0,
            d.prints_flags & abi.prints_null_at_root != 0,
            d.prints_flags & abi.prints_root_container != 0,
        });
        try optStr(w, d.prints_char_as);
        try w.writeAll(",\"number_special_as\":");
        try optStr(w, d.prints_number_special_as);
        try w.print(",\"comments\":\"{s}\"}}", .{switch (d.prints_comments) {
            1 => "pretty",
            2 => "never",
            else => "always",
        }});
    }
    try w.writeByte('}');
}

/// `manifest.NativeKinds` from the bits `abi.losslessBits` makes.
fn kinds(w: *Writer, bits: u32) Writer.Error!void {
    try w.print("{{\"null\":{}", .{bits & abi.native_null != 0});
    inline for (@typeInfo(ExtKind).@"enum".field_names) |name| {
        try w.print(",\"" ++ name ++ "\":{}", .{bits & abi.nativeExt(@field(ExtKind, name)) != 0});
    }
    try w.writeByte('}');
}

fn syntax(w: *Writer, s: *const abi.SyntaxDesc) Writer.Error!void {
    try w.writeAll("{\"comments\":{\"line\":");
    try delimiter(w, s.comments.line);
    try w.writeAll(",\"trailing\":");
    try delimiter(w, s.comments.trailing);
    try w.writeAll(",\"block\":");
    try delimiter(w, s.comments.block);
    try w.writeByte('}');
    inline for (.{ "kv_sep", "flow_map_pad", "empty_map_literal", "indent_unit", "seq_item_marker", "flow_map_open", "flow_map_close", "section_noun", "merge_key", "flow_entry_sep", "line_prefix_marks", "flow_map_kv_sep", "flow_map_entry_sep", "empty_seq_literal" }) |name| {
        try w.writeAll(",\"" ++ name ++ "\":");
        try optStr(w, @field(s, name));
    }
    inline for (.{ "flow_kv_sep_from_siblings", "block_seq_editable", "flow_containers", "single_line_block_mapping", "bare_document_mapping", "structural_indent", "flow_root", "flow_maps_only", "opener_line_comment" }) |name| {
        try w.print(",\"" ++ name ++ "\":{}", .{@field(s, name)});
    }
    try w.print(",\"key_sigil\":{d}", .{s.key_sigil});
    try w.print(",\"line_continuation\":{d}", .{s.line_continuation});
    const c = s.closed_containers;
    if (c.map_open != null) {
        try w.writeAll(",\"closed_containers\":{");
        inline for (.{ "map_open", "map_close", "seq_open", "seq_close" }, 0..) |name, i| {
            if (i > 0) try w.writeByte(',');
            try w.writeAll("\"" ++ name ++ "\":");
            try optStr(w, @field(c, name));
        }
        try w.writeByte('}');
    }
    const h = s.section_header;
    if (h.open != null) {
        try w.writeAll(",\"section_header\":{");
        inline for (.{ "open", "close", "seq_open", "seq_close", "sep" }, 0..) |name, i| {
            if (i > 0) try w.writeByte(',');
            try w.writeAll("\"" ++ name ++ "\":");
            try optStr(w, @field(h, name));
        }
        try w.print(",\"skip_index\":{}}}", .{h.skip_index});
    }
    try w.writeByte('}');
}

fn delimiter(w: *Writer, d: abi.CommentDelimiterDesc) Writer.Error!void {
    if (d.open == null) return w.writeAll("null");
    try w.writeAll("{\"open\":");
    try optStr(w, d.open);
    try w.writeAll(",\"close\":");
    try optStr(w, d.close);
    try w.writeAll(",\"forbidden\":");
    try optStr(w, d.forbidden);
    try w.writeByte('}');
}

fn optStr(w: *Writer, s: ?[*:0]const u8) Writer.Error!void {
    if (s) |x| try str(w, std.mem.span(x)) else try w.writeAll("null");
}

fn str(w: *Writer, s: []const u8) Writer.Error!void {
    try std.json.Stringify.encodeJsonString(s, .{}, w);
}
