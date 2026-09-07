const Printer = @This();
const std = @import("std");
const AST = @import("../../ast/ast.zig");
const width = @import("../../util/util.zig").width;
const num = @import("../../util/number.zig");
const Writer = std.Io.Writer;

/// Prints a document in YAML block style, using default serialize options.
pub fn print(writer: *Writer, ast: *const AST) Writer.Error!void {
    return printWith(writer, ast, .{});
}

/// Prints a document in YAML block style. A short collection value inlines as
/// flow (`[a, b]`, `{ k: v }`) when the whole line fits `opts.width` and the
/// collection carries no comments; otherwise it stays block. The AST records no
/// flow-vs-block memory (no CST), so this is a pure width heuristic, not a replay
/// of the source's own choice.
pub fn printWith(writer: *Writer, ast: *const AST, opts: AST.SerializeOptions) Writer.Error!void {
    // Document-level leading comments (those bound to the root) sit at column 0
    // above everything else — above the directives too, which is where YAML's
    // document prefix allows a comment.
    try leadingComments(writer, ast, ast.leadingCommentAnchor(ast.root), 0);
    try printDirectives(writer, ast);
    try printRootNode(writer, ast, opts);
    // End-of-document comments dangling off the root, at column 0.
    try danglingComments(writer, ast, ast.root, 0);
    try writer.flush();
}

/// The root node, preceded by its own anchor/tag. Every other node's properties
/// are written by whatever wrote its lead-in (`- `, `key:`, `?`); the root has
/// no lead-in, so they are written here — on a line of their own above a block
/// collection (`&m` / `!!seq`, which the parser binds to the collection that
/// opens beneath), inline for anything with a single-line spelling (`&m {}`,
/// `!!str foo`). Dropping them used to leave every alias to an anchored root
/// dangling.
fn printRootNode(writer: *Writer, ast: *const AST, opts: AST.SerializeOptions) Writer.Error!void {
    const root = ast.root;
    if (!hasProps(ast, root)) return printNode(writer, ast, root, 0, opts);
    switch (ast.nodes[root].kind) {
        .sequence => |first| if (first) |child| {
            try writePropsOwnLine(writer, ast, root);
            return printSequence(writer, ast, child, 0, opts);
        },
        .mapping => |first| if (first) |child| {
            try writePropsOwnLine(writer, ast, root);
            return printMapping(writer, ast, child, 0, opts);
        },
        else => {},
    }
    try writeProps(writer, ast, root);
    try printNode(writer, ast, root, 0, opts);
}

// ── Directives ──────────────────────────────────────────────────────────────
// A `%TAG` handle is the one part of a document's directives prefix its body
// depends on: `!e!foo` is a parse error in a document that does not declare
// `!e!`, so a printer that keeps the tag must keep the declaration. Everything
// else in the prefix is inert — a `%YAML` version fig neither acts on nor
// re-emits (the output is 1.2-compatible whatever the source said), and a
// reserved directive, which by definition means nothing to a processor — so it
// is not carried through the AST at all.

/// Emit each `%TAG` directive whose handle a tag in this document actually uses,
/// then the `---` that ends the directives prefix (mandatory once any directive
/// is written). An unused declaration is dropped rather than written back: it
/// would force a `---` marker onto a document that needs neither.
fn printDirectives(writer: *Writer, ast: *const AST) Writer.Error!void {
    var wrote_any = false;
    for (ast.tag_directives) |directive| {
        if (!handleIsUsed(ast, directive.handle)) continue;
        try writer.writeAll("%TAG ");
        try writer.writeAll(directive.handle);
        try writer.writeByte(' ');
        try writer.writeAll(directive.prefix);
        try writer.writeByte('\n');
        wrote_any = true;
    }
    if (wrote_any) try writer.writeAll("---\n");
}

/// Whether any node's tag is spelled with `handle`.
fn handleIsUsed(ast: *const AST, handle: []const u8) bool {
    for (ast.node_tags) |maybe_tag| {
        const tag = maybe_tag orelse continue;
        const used = tagHandleOf(tagText(tag)) orelse continue;
        if (std.mem.eql(u8, used, handle)) return true;
    }
    return false;
}

/// The handle a tag spelling uses, including both `!`s: `!e!foo` → `!e!`, `!!str`
/// → `!!`, and a local `!foo` (or the bare non-specific `!`) → the primary
/// `!`. A verbatim `!<uri>` tag uses none, and needs no declaration.
fn tagHandleOf(text: []const u8) ?[]const u8 {
    if (text.len == 0 or text[0] != '!') return null;
    if (text.len > 1 and text[1] == '<') return null;
    const close = std.mem.indexOfScalarPos(u8, text, 1, '!') orelse return text[0..1];
    return text[0 .. close + 1];
}

pub fn printNode(writer: *Writer, ast: *const AST, id: AST.Node.Id, depth: usize, opts: AST.SerializeOptions) Writer.Error!void {
    const node = ast.nodes[id];
    switch (node.kind) {
        .null_ => try writer.writeAll("null\n"),
        .boolean => |value| {
            try writer.writeAll(if (value) "true" else "false");
            try writer.writeByte('\n');
        },
        .number => |value| {
            try num.write(writer, value.raw, num.yaml_1_2);
            try writer.writeByte('\n');
        },
        .extended => |value| {
            // YAML's core schema has none of these types (timestamps were YAML
            // 1.1). Enum literals become string scalars (may need quoting);
            // datetimes and char codepoints emit verbatim as plain scalars.
            switch (value.kind) {
                .enum_literal => try printScalar(writer, value.text),
                else => try writer.writeAll(value.text),
            }
            try writer.writeByte('\n');
        },
        .string => |value| {
            try printScalar(writer, value);
            try writer.writeByte('\n');
        },
        .sequence => |first_child| try printSequence(writer, ast, first_child, depth, opts),
        .mapping => |first_child| try printMapping(writer, ast, first_child, depth, opts),
        .keyvalue => |kv| try printKeyValue(writer, ast, kv, depth, false, opts),
        .alias => |name| {
            try writer.writeByte('*');
            try writer.writeAll(name);
            try writer.writeByte('\n');
        },
    }
}

fn printSequence(writer: *Writer, document: *const AST, first_child: ?AST.Node.Id, depth: usize, opts: AST.SerializeOptions) Writer.Error!void {
    if (first_child == null) {
        try writer.writeAll("[]\n");
        return;
    }

    var current_id = first_child;
    while (current_id) |id| {
        const item = document.nodes[id];
        // A scalar element owns its leading directly. A block-mapping element
        // (`- key: val`) parked its leading on its first key (the parser's
        // `parking_container`), which still renders above the `- ` line.
        try leadingComments(writer, document, seqItemLeadAnchor(document, id), depth);
        try writeIndent(writer, depth);
        switch (item.kind) {
            // Sequence items stay block even when short: `- uses: x` reads far
            // better than `- { uses: x }`, and that idiom dominates config files.
            // Flow inlining is reserved for mapping *values* (`key: [a, b]`).
            .mapping => |child| {
                try writer.writeAll("- ");
                if (child) |first_pair| {
                    // An item mapping's own props cannot ride the `- key: v`
                    // line: the parser reads `- &a key: v` as the KEY's anchor
                    // (see its `holdKeyLineProps`). They take the rest of the
                    // dash line instead, and the mapping starts beneath.
                    if (hasProps(document, id)) {
                        try writePropsOwnLine(writer, document, id);
                        try printMapping(writer, document, first_pair, depth + 1, opts);
                    } else {
                        try printSequenceMapping(writer, document, first_pair, depth, opts);
                    }
                } else {
                    try writeProps(writer, document, id); // `- &a {}`
                    try writer.writeAll("{}\n");
                }
            },
            .sequence => |child| {
                if (child == null) {
                    try writer.writeAll("- ");
                    try writeProps(writer, document, id); // `- &a []`
                    try writer.writeAll("[]\n");
                } else if (hasProps(document, id)) {
                    // `- &a` — the props are all that follows the dash, and the
                    // nested block sits under them.
                    try writer.writeAll("- ");
                    try writePropsOwnLine(writer, document, id);
                    try printSequence(writer, document, child, depth + 1, opts);
                } else {
                    // A bare dash, not `- ` — nothing follows on the line.
                    try writer.writeAll("-\n");
                    try printSequence(writer, document, child, depth + 1, opts);
                }
            },
            else => {
                try writer.writeAll("- ");
                if (!try tryWriteBlockStringValue(writer, document, id, depth + 1)) {
                    try printInlineValue(writer, document, id);
                    try trailingComment(writer, document, id);
                    try writer.writeByte('\n');
                }
            },
        }
        current_id = item.next_sibling;
    }
}

/// Where a sequence item's leading comment lives. A block-mapping item parks it
/// on the item-mapping's first key (so it sits above the `- ` line); every other
/// item owns its leading on the item node itself.
fn seqItemLeadAnchor(ast: *const AST, id: AST.Node.Id) AST.Node.Id {
    return switch (ast.nodes[id].kind) {
        .mapping => |first| if (first) |kv| ast.nodes[kv].kind.keyvalue.key else id,
        else => id,
    };
}

fn printMapping(writer: *Writer, document: *const AST, first_child: ?AST.Node.Id, depth: usize, opts: AST.SerializeOptions) Writer.Error!void {
    if (first_child == null) {
        try writer.writeAll("{}\n");
        return;
    }

    var current_id = first_child;
    while (current_id) |id| {
        try leadingComments(writer, document, document.leadingCommentAnchor(id), depth);
        try printKeyValue(writer, document, document.nodes[id].kind.keyvalue, depth, false, opts);
        current_id = document.nodes[id].next_sibling;
    }
}

fn printSequenceMapping(writer: *Writer, document: *const AST, first_pair: AST.Node.Id, depth: usize, opts: AST.SerializeOptions) Writer.Error!void {
    // Every pair sits one level deeper than the sequence: the `- ` prefix already
    // occupies that level's two columns. The first pair is written right after the
    // `- `, so its own leading indent is suppressed (`skip_indent`) — but its base
    // depth is still `depth + 1`, so a nested block value (mapping/sequence) indents
    // relative to the key's real column, not the sequence's. Passing `0` here would
    // emit nested children a level too shallow, turning them into siblings.
    try printKeyValue(writer, document, document.nodes[first_pair].kind.keyvalue, depth + 1, true, opts);

    var current_id = document.nodes[first_pair].next_sibling;
    while (current_id) |id| {
        try leadingComments(writer, document, document.leadingCommentAnchor(id), depth + 1);
        try printKeyValue(writer, document, document.nodes[id].kind.keyvalue, depth + 1, false, opts);
        current_id = document.nodes[id].next_sibling;
    }
}

fn printKeyValue(writer: *Writer, document: *const AST, kv: anytype, depth: usize, skip_indent: bool, opts: AST.SerializeOptions) Writer.Error!void {
    const value = document.nodes[kv.value];
    if (!skip_indent) try writeIndent(writer, depth);
    // Columns already on the value's line — indent, key (with props), and `: ` —
    // the budget the flow form must fit within.
    const value_prefix: usize = switch (document.nodes[kv.key].kind) {
        // A collection key has no single-line spelling to put before a `:`,
        // so it takes the explicit form: `? key` on its own line(s), then the
        // value on a `: ` line at the key's column — where the value's prefix
        // is just the indent and the `: `.
        .sequence, .mapping => blk: {
            try writeExplicitKey(writer, document, kv.key, depth, opts);
            break :blk 2 * depth + 2;
        },
        else => blk: {
            try writeKeyLead(writer, document, kv.key); // `&k key:` / `!!str key:`
            break :blk 2 * depth + keyCols(document, kv.key) + 2;
        },
    };
    switch (value.kind) {
        .mapping => |child| {
            if (child != null and flowFits(document, kv.value, value_prefix, opts)) {
                try writer.writeAll(": ");
                try writeFlow(writer, document, kv.value);
                try writer.writeByte('\n');
                return;
            }
            try writer.writeByte(':');
            try writePropsAfterColon(writer, document, kv.value); // `: &a` before the block
            if (child == null) {
                try writer.writeAll(" {}\n");
            } else {
                // A comment on the `key:` line rides here, above the block value.
                try trailingComment(writer, document, kv.value);
                try writer.writeByte('\n');
                try printMapping(writer, document, child, depth + 1, opts);
            }
        },
        .sequence => |child| {
            if (child != null and flowFits(document, kv.value, value_prefix, opts)) {
                try writer.writeAll(": ");
                try writeFlow(writer, document, kv.value);
                try writer.writeByte('\n');
                return;
            }
            try writer.writeByte(':');
            try writePropsAfterColon(writer, document, kv.value);
            if (child == null) {
                try writer.writeAll(" []\n");
            } else {
                try trailingComment(writer, document, kv.value);
                try writer.writeByte('\n');
                // Indentless: a sequence value's dashes sit at the key's column.
                try printSequence(writer, document, child, depth, opts);
            }
        },
        else => {
            try writer.writeAll(": ");
            if (!try tryWriteBlockStringValue(writer, document, kv.value, depth + 1)) {
                try printInlineValue(writer, document, kv.value);
                // Trailing comment rides the value's line (`key: value # note`).
                // Block-scalar values omit it (no single line to ride).
                try trailingComment(writer, document, kv.value);
                try writer.writeByte('\n');
            }
        },
    }
}

// ── Flow (inline) collections ───────────────────────────────────────────────
// A collection value/element may render inline (`[a, b]` / `{ k: v }`) instead
// of as block lines when it fits the width budget and carries no comments. The
// width is measured by rendering the real flow form (see `util.width`), so the
// fit test can never drift from the bytes emitted.

/// Whether the collection at `id` may render as flow starting at column
/// `prefix`, within `opts.width`.
fn flowFits(ast: *const AST, id: AST.Node.Id, prefix: usize, opts: AST.SerializeOptions) bool {
    if (!flowEligible(ast, id)) return false;
    const w = width.rendered(writeFlow, .{ ast, id }) orelse return false;
    return prefix + w <= opts.width;
}

/// True when `id`'s whole subtree can render as flow with no loss: nothing in it
/// carries a comment (flow has nowhere to put one) or an anchor/tag property, and
/// every leaf has a flow spelling (no multi-line strings, aliases, or extended
/// scalars — those need block position).
fn flowEligible(ast: *const AST, id: AST.Node.Id) bool {
    if (!ast.comments(id).isEmpty()) return false;
    if (hasProps(ast, id)) return false;
    return switch (ast.nodes[id].kind) {
        .null_, .boolean, .number => true,
        .string => |s| std.mem.indexOfScalar(u8, s, '\n') == null,
        .extended, .alias, .keyvalue => false,
        .sequence => |first| {
            var cur = first;
            while (cur) |el| : (cur = ast.nodes[el].next_sibling) {
                // A list of mappings reads far better as block `- key: value`
                // lines than as `[{ ... }, { ... }]`; keep it block. Lists of
                // scalars (or of sequences) still inline.
                if (ast.nodes[el].kind == .mapping) return false;
                if (!flowEligible(ast, el)) return false;
            }
            return true;
        },
        .mapping => |first| {
            var cur = first;
            while (cur) |kv_id| : (cur = ast.nodes[kv_id].next_sibling) {
                if (!ast.comments(kv_id).isEmpty()) return false;
                const kv = ast.nodes[kv_id].kind.keyvalue;
                if (!ast.comments(kv.key).isEmpty()) return false;
                // A key must have a one-line flow spelling: a scalar (a
                // string without a newline; `{ 1: a, null: b }` is fine
                // YAML), never a collection or an alias.
                switch (ast.nodes[kv.key].kind) {
                    .string => |s| if (std.mem.indexOfScalar(u8, s, '\n') != null) return false,
                    .null_, .boolean, .number => {},
                    else => return false,
                }
                // A mapping directly nested in a mapping is too much structure
                // for one flow line; keep it block (matches the fig printer).
                if (ast.nodes[kv.value].kind == .mapping) return false;
                if (!flowEligible(ast, kv.value)) return false;
            }
            return true;
        },
    };
}

fn hasProps(ast: *const AST, id: AST.Node.Id) bool {
    if (id < ast.node_anchors.len and ast.node_anchors[id] != null) return true;
    if (id < ast.node_tags.len and ast.node_tags[id] != null) return true;
    return false;
}

/// Rendered width of a key (its anchor/tag props plus the key scalar), the
/// left-hand columns of a `key: value` line.
fn keyCols(ast: *const AST, key_id: AST.Node.Id) usize {
    return width.rendered(writeKeyLead, .{ ast, key_id }) orelse 0;
}

/// The left-hand side of an implicit `key: value` line: the key's props and
/// its single-line spelling. Every non-collection key has one — a YAML mapping
/// key is any node, not just a string, and the parser produces each of these
/// (`null: a`, `23: x`, `true: x`, `*ref : x`, `2001-12-14: x`). A collection
/// key is the explicit `?` form instead (`writeExplicitKey`).
fn writeKeyLead(writer: *Writer, ast: *const AST, key_id: AST.Node.Id) Writer.Error!void {
    try writeProps(writer, ast, key_id);
    try writeScalarKey(writer, ast, key_id);
}

/// A non-collection key's own spelling, without props.
fn writeScalarKey(writer: *Writer, ast: *const AST, key_id: AST.Node.Id) Writer.Error!void {
    switch (ast.nodes[key_id].kind) {
        .string => |s| try printScalar(writer, s),
        // An empty key (`: a`) reads back as a null key; spelling it `null`
        // says so, and re-parses to the same node.
        .null_ => try writer.writeAll("null"),
        .boolean => |b| try writer.writeAll(if (b) "true" else "false"),
        .number => |n| try num.write(writer, n.raw, num.yaml_1_2),
        .extended => |value| switch (value.kind) {
            .enum_literal => try printScalar(writer, value.text),
            else => try writer.writeAll(value.text),
        },
        // `*a : b` — the space is load-bearing: `:` is a legal anchor-name
        // character, so `*a:` would be an alias to an anchor called `a:`.
        .alias => |name| {
            try writer.writeByte('*');
            try writer.writeAll(name);
            try writer.writeByte(' ');
        },
        .sequence, .mapping, .keyvalue => unreachable, // writeExplicitKey's
    }
}

/// A collection key, in explicit form. Everything up to and including the
/// indent of the `:` line is written, so the caller continues with the `:` and
/// the value exactly as for an implicit key. The key inlines as `? [a, b]` when
/// it fits the width like any other collection; otherwise it is `?` alone on
/// the line — with the key's own props after it, the way a block value's ride
/// after the `:` — and the block form indented beneath:
///
///     ? &a
///       - a
///       - b
///     : value
fn writeExplicitKey(writer: *Writer, ast: *const AST, key_id: AST.Node.Id, depth: usize, opts: AST.SerializeOptions) Writer.Error!void {
    if (flowFits(ast, key_id, 2 * depth + 2, opts)) {
        try writer.writeAll("? ");
        try writeFlow(writer, ast, key_id);
        try writer.writeByte('\n');
    } else {
        try writer.writeByte('?');
        try writePropsAfterColon(writer, ast, key_id);
        try writer.writeByte('\n');
        try printNode(writer, ast, key_id, depth + 1, opts);
    }
    try writeIndent(writer, depth);
}

/// Emit `id` as inline flow. Assumes `flowEligible(ast, id)`, so every node has a
/// flow spelling and the only possible error is the writer's.
fn writeFlow(writer: *Writer, ast: *const AST, id: AST.Node.Id) Writer.Error!void {
    switch (ast.nodes[id].kind) {
        .null_ => try writer.writeAll("null"),
        .boolean => |b| try writer.writeAll(if (b) "true" else "false"),
        .number => |n| try num.write(writer, n.raw, num.yaml_1_2),
        .string => |s| try printFlowScalar(writer, s),
        .sequence => |first| {
            if (first == null) {
                try writer.writeAll("[]");
                return;
            }
            try writer.writeByte('[');
            var cur = first;
            var i: usize = 0;
            while (cur) |el| : ({
                cur = ast.nodes[el].next_sibling;
                i += 1;
            }) {
                if (i > 0) try writer.writeAll(", ");
                try writeFlow(writer, ast, el);
            }
            try writer.writeByte(']');
        },
        .mapping => |first| {
            if (first == null) {
                try writer.writeAll("{}");
                return;
            }
            try writer.writeAll("{ ");
            var cur = first;
            var i: usize = 0;
            while (cur) |kv_id| : ({
                cur = ast.nodes[kv_id].next_sibling;
                i += 1;
            }) {
                if (i > 0) try writer.writeAll(", ");
                const kv = ast.nodes[kv_id].kind.keyvalue;
                switch (ast.nodes[kv.key].kind) {
                    .string => |s| try printFlowScalar(writer, s),
                    else => try writeScalarKey(writer, ast, kv.key),
                }
                try writer.writeAll(": ");
                try writeFlow(writer, ast, kv.value);
            }
            try writer.writeAll(" }");
        },
        .extended, .alias, .keyvalue => unreachable,
    }
}

/// Like `printScalar`, but also single-quotes a plain scalar carrying a flow
/// indicator (`,` `[` `]` `{` `}`), which would otherwise close or split the
/// surrounding flow collection.
fn printFlowScalar(writer: *Writer, raw: []const u8) Writer.Error!void {
    if (!hasControlChar(raw) and !needsQuoting(raw) and containsFlowIndicator(raw)) {
        try writeSingleQuoted(writer, raw);
    } else {
        try printScalar(writer, raw);
    }
}

fn containsFlowIndicator(s: []const u8) bool {
    for (s) |c| switch (c) {
        ',', '[', ']', '{', '}' => return true,
        else => {},
    };
    return false;
}

fn printInlineValue(writer: *Writer, document: *const AST, id: AST.Node.Id) Writer.Error!void {
    const node = document.nodes[id];
    try writeProps(writer, document, id);
    switch (node.kind) {
        .null_ => try writer.writeAll("null"),
        .boolean => |value| try writer.writeAll(if (value) "true" else "false"),
        .number => |value| try num.write(writer, value.raw, num.yaml_1_2),
        .extended => |value| switch (value.kind) {
            .enum_literal => try printScalar(writer, value.text),
            else => try writer.writeAll(value.text),
        },
        .string => |value| try printScalar(writer, value),
        .sequence => |child| if (child == null) try writer.writeAll("[]") else try writer.writeAll("[...]"),
        .mapping => |child| if (child == null) try writer.writeAll("{}") else try writer.writeAll("{...}"),
        .alias => |name| {
            try writer.writeByte('*');
            try writer.writeAll(name);
        },
        .keyvalue => unreachable,
    }
}

/// Emit a node's anchor/tag properties (`&name `, `!tag `) from the AST
/// side-tables, so a full reserialize keeps the reference layer intact (an
/// anchored value stays anchored, rather than leaving any alias to it dangling).
/// Order matches YAML's `c-ns-properties`: anchor then tag, both optional.
fn writeProps(writer: *Writer, ast: *const AST, id: AST.Node.Id) Writer.Error!void {
    if (id < ast.node_anchors.len) if (ast.node_anchors[id]) |name| {
        try writer.writeByte('&');
        try writer.writeAll(name);
        try writer.writeByte(' ');
    };
    if (id < ast.node_tags.len) if (ast.node_tags[id]) |tag| {
        try writer.writeAll(tagText(tag));
        try writer.writeByte(' ');
    };
}

/// Render a type tag as YAML: a verbatim `.text` tag as-is (its exact spelling),
/// or a normalized `.kind` tag as the core-schema shorthand (`!!int`, `!!str`, …)
/// — the latter is what a fig/canonical-origin tag becomes when reserialized to
/// YAML.
fn tagText(tag: AST.Tag) []const u8 {
    return switch (tag) {
        .text => |t| t,
        .kind => |k| switch (k) {
            .null_ => "!!null",
            .boolean => "!!bool",
            .string => "!!str",
            .integer => "!!int",
            .float => "!!float",
            .sequence => "!!seq",
            .mapping => "!!map",
        },
    };
}

/// Like `writeProps` but for a line of its own — `&a !tag` with no trailing
/// space, then the newline — the form a block collection's props take when there
/// is no `key:`/`- ` lead-in to hang them off (a document root) or when hanging
/// them off one would rebind them (a sequence item's mapping). The parser binds
/// a property line to the collection that opens beneath it.
fn writePropsOwnLine(writer: *Writer, ast: *const AST, id: AST.Node.Id) Writer.Error!void {
    var wrote_any = false;
    if (id < ast.node_anchors.len) if (ast.node_anchors[id]) |name| {
        try writer.writeByte('&');
        try writer.writeAll(name);
        wrote_any = true;
    };
    if (id < ast.node_tags.len) if (ast.node_tags[id]) |tag| {
        if (wrote_any) try writer.writeByte(' ');
        try writer.writeAll(tagText(tag));
    };
    try writer.writeByte('\n');
}

/// Like `writeProps` but for the position right after a mapping value's `:`,
/// before a block collection or `{}`/`[]`: emits ` &name`/` !tag` with a leading
/// (not trailing) space, so `key:` becomes `key: &a` and a propless value keeps
/// its original `key:` / `key: {}` framing.
fn writePropsAfterColon(writer: *Writer, ast: *const AST, id: AST.Node.Id) Writer.Error!void {
    if (id < ast.node_anchors.len) if (ast.node_anchors[id]) |name| {
        try writer.writeAll(" &");
        try writer.writeAll(name);
    };
    if (id < ast.node_tags.len) if (ast.node_tags[id]) |tag| {
        try writer.writeByte(' ');
        try writer.writeAll(tagText(tag));
    };
}

// ── Scalar emission ─────────────────────────────────────────────────────────

/// Emit a scalar inline — plain, single-quoted, or double-quoted as the value
/// requires so it reads back unchanged. Used for keys and single-line values; a
/// multi-line string *value* becomes a `|` block scalar instead (see
/// `tryWriteBlockStringValue`), so a newline reaching here (e.g. in a key) is
/// double-quoted.
fn printScalar(writer: *Writer, raw: []const u8) Writer.Error!void {
    if (hasControlChar(raw)) {
        try writeDoubleQuoted(writer, raw);
    } else if (needsQuoting(raw)) {
        try writeSingleQuoted(writer, raw);
    } else {
        try writer.writeAll(raw);
    }
}

/// True if any byte is an ASCII control character (newline/tab included). Such a
/// scalar can only be represented inline by double-quoting.
fn hasControlChar(s: []const u8) bool {
    for (s) |c| if (c < 0x20 or c == 0x7f) return true;
    return false;
}

/// Whether a plain (unquoted) scalar would be misread — as another type, or as
/// YAML structure — and so needs single-quoting. Assumes no control characters
/// (those force double-quoting upstream).
fn needsQuoting(s: []const u8) bool {
    if (s.len == 0) return true; // empty plain scalar reads back as null
    if (resolvesToNonString(s)) return true;
    if (s[0] == ' ' or s[s.len - 1] == ' ') return true; // leading/trailing space is lost
    switch (s[0]) {
        // A plain scalar may not begin with an indicator character.
        '!', '&', '*', '?', '|', '>', '%', '@', '`', '"', '\'', '#', ',', '[', ']', '{', '}' => return true,
        // `-`/`:` are unsafe as the first char only before a space (or alone).
        '-', ':' => if (s.len == 1 or s[1] == ' ') return true,
        else => {},
    }
    // Interior `: ` (mapping indicator), trailing `:`, or ` #` (comment) force quoting.
    if (std.mem.indexOf(u8, s, ": ") != null) return true;
    if (s[s.len - 1] == ':') return true;
    if (std.mem.indexOf(u8, s, " #") != null) return true;
    return false;
}

/// Plain scalars that YAML 1.2's core schema resolves to a non-string type
/// (null, bool, the special floats) or a number.
fn resolvesToNonString(s: []const u8) bool {
    const keywords = [_][]const u8{
        "null",  "Null",  "NULL",  "~",
        "true",  "True",  "TRUE",  "false",
        "False", "FALSE", ".inf",  ".Inf",
        ".INF",  "-.inf", "-.Inf", "-.INF",
        "+.inf", ".nan",  ".NaN",  ".NAN",
    };
    for (keywords) |kw| if (std.mem.eql(u8, s, kw)) return true;
    return looksNumeric(s);
}

/// Whether `s` would parse as a YAML number. Zig's float parser also accepts
/// bare `inf`/`nan`, which the core schema does not, so those are excluded.
fn looksNumeric(s: []const u8) bool {
    if (asciiContains(s, "inf") or asciiContains(s, "nan")) return false;
    if (std.fmt.parseInt(i64, s, 10)) |_| return true else |_| {}
    if (std.fmt.parseInt(u64, s, 10)) |_| return true else |_| {}
    if (std.fmt.parseFloat(f64, s)) |_| return true else |_| {}
    return false;
}

/// Case-insensitive ASCII substring test.
fn asciiContains(haystack: []const u8, needle: []const u8) bool {
    if (needle.len == 0) return true;
    if (haystack.len < needle.len) return false;
    var i: usize = 0;
    while (i + needle.len <= haystack.len) : (i += 1) {
        if (std.ascii.eqlIgnoreCase(haystack[i .. i + needle.len], needle)) return true;
    }
    return false;
}

fn writeSingleQuoted(writer: *Writer, s: []const u8) Writer.Error!void {
    try writer.writeByte('\'');
    for (s) |c| {
        if (c == '\'') try writer.writeByte('\''); // '' escapes a quote
        try writer.writeByte(c);
    }
    try writer.writeByte('\'');
}

fn writeDoubleQuoted(writer: *Writer, s: []const u8) Writer.Error!void {
    try writer.writeByte('"');
    for (s) |c| {
        switch (c) {
            '"' => try writer.writeAll("\\\""),
            '\\' => try writer.writeAll("\\\\"),
            '\n' => try writer.writeAll("\\n"),
            '\t' => try writer.writeAll("\\t"),
            '\r' => try writer.writeAll("\\r"),
            else => if (c < 0x20 or c == 0x7f)
                try writer.print("\\x{x:0>2}", .{c})
            else
                try writer.writeByte(c),
        }
    }
    try writer.writeByte('"');
}

// ── Block scalars (multi-line strings) ──────────────────────────────────────

/// Emit `id` as a `|` block scalar when it is a multi-line string a block scalar
/// can round-trip; returns whether it did (the caller falls back to an inline
/// scalar otherwise). The caller has already written the `key: ` / `- ` lead-in;
/// on success this writes any anchor/tag props, the indicator, and the indented
/// content lines.
fn tryWriteBlockStringValue(writer: *Writer, ast: *const AST, id: AST.Node.Id, indent: usize) Writer.Error!bool {
    const node = ast.nodes[id];
    if (node.kind != .string) return false;
    const s = node.kind.string;
    if (!blockScalarOk(s)) return false;
    try writeProps(writer, ast, id);
    try writeBlockScalar(writer, s, indent);
    return true;
}

/// Whether a multi-line string can be faithfully emitted as a `|` block scalar.
/// Conservative: rejects carriage returns and other non-newline controls, lines
/// with trailing whitespace (invisible, editor-fragile), leading whitespace on
/// the FIRST non-empty line only (it anchors YAML's indentation auto-detection;
/// deeper-indented later lines round-trip fine — think shell-script bodies),
/// and 2+ trailing newlines (which would need the `|+` keep indicator). Anything
/// rejected here is double-quoted instead, which always round-trips.
fn blockScalarOk(s: []const u8) bool {
    if (std.mem.indexOfScalar(u8, s, '\n') == null) return false;
    if (std.mem.endsWith(u8, s, "\n\n")) return false;
    const body = if (std.mem.endsWith(u8, s, "\n")) s[0 .. s.len - 1] else s;
    var it = std.mem.splitScalar(u8, body, '\n');
    var first_content = true;
    while (it.next()) |line| {
        if (line.len == 0) continue; // interior blank lines are fine (emitted empty)
        if (first_content and (line[0] == ' ' or line[0] == '\t')) return false;
        first_content = false;
        if (line[line.len - 1] == ' ' or line[line.len - 1] == '\t') return false;
        for (line) |c| if (c != '\t' and (c < 0x20 or c == 0x7f)) return false;
    }
    return true;
}

/// Write the block-scalar indicator and indented content for a string that
/// passed `blockScalarOk`. `|` clips a single trailing newline, `|-` strips a
/// missing one. Blank lines are emitted empty (no trailing-indent ambiguity).
fn writeBlockScalar(writer: *Writer, s: []const u8, indent: usize) Writer.Error!void {
    const clip = std.mem.endsWith(u8, s, "\n");
    try writer.writeAll(if (clip) "|\n" else "|-\n");
    const body = if (clip) s[0 .. s.len - 1] else s;
    var it = std.mem.splitScalar(u8, body, '\n');
    while (it.next()) |line| {
        if (line.len == 0) {
            try writer.writeByte('\n');
        } else {
            try writeIndent(writer, indent);
            try writer.writeAll(line);
            try writer.writeByte('\n');
        }
    }
}

fn writeIndent(writer: *Writer, depth: usize) Writer.Error!void {
    for (0..depth) |_| try writer.writeAll("  ");
}

// ── Comments ────────────────────────────────────────────────────────────────
// YAML has only `#` line comments. A `block` comment captured from another
// format (`/* … */`) therefore degrades to a run of `#` lines, one per content
// line — content survives, block-ness does not (it can't, in YAML).

/// Emit a node's leading comments above its line, at `depth`. Each comment (line
/// or block) becomes one or more `# …` lines; a multi-line block yields one `#`
/// line per content line.
fn leadingComments(writer: *Writer, ast: *const AST, id: AST.Node.Id, depth: usize) Writer.Error!void {
    for (ast.comments(id).leading) |c| {
        var it = std.mem.splitScalar(u8, c.text, '\n');
        while (it.next()) |line| {
            try writeIndent(writer, depth);
            try writeHashLine(writer, std.mem.trim(u8, line, " \t"));
            try writer.writeByte('\n');
        }
    }
}

/// Emit a node's trailing comment after its value (` # …`). A multi-line block
/// is flattened to one line (newlines → spaces) since a YAML trailing comment
/// must stay on the value's line.
fn trailingComment(writer: *Writer, ast: *const AST, id: AST.Node.Id) Writer.Error!void {
    const c = ast.comments(id).trailing orelse return;
    try writer.writeAll(" #");
    if (c.text.len != 0) {
        try writer.writeByte(' ');
        for (c.text) |ch| try writer.writeByte(if (ch == '\n') ' ' else ch);
    }
}

/// Emit a container's dangling comments (orphans at the end of its body) at
/// `depth`, each as one or more `#` lines.
fn danglingComments(writer: *Writer, ast: *const AST, id: AST.Node.Id, depth: usize) Writer.Error!void {
    for (ast.comments(id).dangling) |c| {
        var it = std.mem.splitScalar(u8, c.text, '\n');
        while (it.next()) |line| {
            try writeIndent(writer, depth);
            try writeHashLine(writer, std.mem.trim(u8, line, " \t"));
            try writer.writeByte('\n');
        }
    }
}

/// Write `# text` (or a bare `#` for an empty comment).
fn writeHashLine(writer: *Writer, text: []const u8) Writer.Error!void {
    try writer.writeByte('#');
    if (text.len != 0) {
        try writer.writeByte(' ');
        try writer.writeAll(text);
    }
}

test "yaml emits comments; block degrades to a # run" {
    const a = std.testing.allocator;
    var b = AST.Builder.init(a);
    defer b.deinit();

    // { name: "fig", nums: [1, 2] } with assorted comments, including a block
    // comment that must degrade to two `#` lines in YAML.
    const v_name = try b.addString("fig");
    try b.setComments(v_name, .{ .trailing = .{ .text = "inline", .style = .line } });
    const k_name = try b.addString("name");
    try b.setComments(k_name, .{ .leading = &.{.{ .text = "greeting", .style = .line }} });

    const n1 = try b.addInt(1);
    try b.setComments(n1, .{ .leading = &.{.{ .text = "first\nsecond", .style = .block }} });
    const n2 = try b.addInt(2);
    try b.setComments(n2, .{ .trailing = .{ .text = "two", .style = .line } });
    const v_nums = try b.addSequence(&.{ n1, n2 });
    const k_nums = try b.addString("nums");

    const root = try b.addMapping(&.{
        .{ .key = k_name, .value = v_name },
        .{ .key = k_nums, .value = v_nums },
    });
    try b.setComments(root, .{ .leading = &.{.{ .text = "config", .style = .line }} });

    var ast = try b.finish(root);
    defer ast.deinit();

    var out: Writer.Allocating = .init(a);
    defer out.deinit();
    try print(&out.writer, &ast);
    try std.testing.expectEqualStrings(
        \\# config
        \\# greeting
        \\name: fig # inline
        \\nums:
        \\# first
        \\# second
        \\- 1
        \\- 2 # two
        \\
    , out.written());
}

test "a comment on a key: line with a block value rides the key line" {
    const Parser = @import("parser.zig");
    // The comment trails the entry (`contents:`), not the first sequence item.
    const cases = [_][]const u8{
        "contents: # note\n- a\n- b\n",
        "outer: # note\n  inner: 1\n",
    };
    for (cases) |src| {
        const doc = try Parser.parse(std.testing.allocator, src, .v1_2_2);
        defer doc.deinit(std.testing.allocator);
        var out: Writer.Allocating = .init(std.testing.allocator);
        defer out.deinit();
        try print(&out.writer, &doc.ast);
        try std.testing.expectEqualStrings(src, out.written());
    }
}

test "prints YAML document" {
    // Native is the AST-literal syntax here — this test's subject is YAML
    // printing, not JSON reading.
    const Parser = @import("../../canonical/parser.zig");
    const input = "{\"name\":\"Ada\",\"tags\":[\"zig\",true,null]}";
    var doc = try Parser.parseAbstract(std.testing.allocator, input);
    defer doc.deinit();

    var output: Writer.Allocating = .init(std.testing.allocator);
    defer output.deinit();

    try print(&output.writer, &doc);
    // The short, comment-free `tags` sequence inlines as flow within the width.
    try std.testing.expectEqualSlices(u8,
        \\name: Ada
        \\tags: [zig, true, null]
        \\
    , output.written());
}

test "yaml printer: nested block value on a sequence item's first pair indents under the key" {
    const a = std.testing.allocator;
    var b = AST.Builder.init(a);
    defer b.deinit();

    // { audience: [ { outer: { inner: 1 } }, { last: 3 } ] }
    // The nested mapping AND a nested sequence both hang off the FIRST pair of a
    // sequence-item mapping — the case that previously emitted children a level too
    // shallow, collapsing them into siblings (`outer: null` + `inner: 1`).
    // Rendered with width 0 to force block layout (the short `outer` value would
    // otherwise inline as flow), so the under-key indentation is what's tested.
    const inner = try b.addMapping(&.{.{ .key = try b.addString("inner"), .value = try b.addInt(1) }});
    const item0 = try b.addMapping(&.{.{ .key = try b.addString("outer"), .value = inner }});
    const item1 = try b.addMapping(&.{.{ .key = try b.addString("last"), .value = try b.addInt(3) }});
    const seq = try b.addSequence(&.{ item0, item1 });
    const root = try b.addMapping(&.{.{ .key = try b.addString("audience"), .value = seq }});

    var ast = try b.finish(root);
    defer ast.deinit();

    var out: Writer.Allocating = .init(a);
    defer out.deinit();
    try printWith(&out.writer, &ast, .{ .width = 0 });
    try std.testing.expectEqualStrings(
        \\audience:
        \\- outer:
        \\    inner: 1
        \\- last: 3
        \\
    , out.written());
}

test "yaml printer: nested sequence on a sequence item's first pair indents under the key" {
    const a = std.testing.allocator;
    var b = AST.Builder.init(a);
    defer b.deinit();

    // { audience: [ { outer: [1, 2] } ] } — a nested indentless sequence whose
    // dashes must sit at the key's column, not escape to the parent sequence's.
    // Rendered with width 0 to force block layout (this small a structure would
    // otherwise inline as flow), so the indentless-sequence indent is under test.
    const nums = try b.addSequence(&.{ try b.addInt(1), try b.addInt(2) });
    const item0 = try b.addMapping(&.{.{ .key = try b.addString("outer"), .value = nums }});
    const seq = try b.addSequence(&.{item0});
    const root = try b.addMapping(&.{.{ .key = try b.addString("audience"), .value = seq }});

    var ast = try b.finish(root);
    defer ast.deinit();

    var out: Writer.Allocating = .init(a);
    defer out.deinit();
    try printWith(&out.writer, &ast, .{ .width = 0 });
    try std.testing.expectEqualStrings(
        \\audience:
        \\- outer:
        \\  - 1
        \\  - 2
        \\
    , out.written());
}

/// Parse `src` as YAML and re-emit it with default options, asserting the exact
/// output. Covers the flow (inline collection) layout decisions end to end.
fn expectRoundTrip(src: []const u8, expected: []const u8) !void {
    const Parser = @import("parser.zig");
    const doc = try Parser.parse(std.testing.allocator, src, .v1_2_2);
    defer doc.deinit(std.testing.allocator);
    var out: Writer.Allocating = .init(std.testing.allocator);
    defer out.deinit();
    try print(&out.writer, &doc.ast);
    try std.testing.expectEqualStrings(expected, out.written());
}

test "yaml flow: short comment-free mapping values inline; sequence items stay block" {
    // A short sequence value and a short mapping value both inline.
    try expectRoundTrip("branches:\n- main\n", "branches: [main]\n");
    try expectRoundTrip(
        "concurrency:\n  group: ci\n  cancel: true\n",
        "concurrency: { group: ci, cancel: true }\n",
    );
    // Sequence-item mappings do NOT inline — `- uses: x`, not `- { uses: x }`.
    try expectRoundTrip("steps:\n- uses: checkout\n", "steps:\n- uses: checkout\n");
}

test "yaml flow: a comment or overflow keeps a collection block" {
    // A comment anywhere in the subtree disqualifies flow (nowhere to put it).
    try expectRoundTrip("nums:\n# note\n- 1\n- 2\n", "nums:\n# note\n- 1\n- 2\n");
    // A value past the width budget stays block.
    const long = "x" ** 90;
    try expectRoundTrip(
        "items:\n- " ++ long ++ "\n",
        "items:\n- " ++ long ++ "\n",
    );
}

test "yaml printer: a nested block sequence leaves no trailing space after the dash" {
    try expectRoundTrip("- - a\n  - b\n- c\n", "-\n  - a\n  - b\n- c\n");
    try expectRoundTrip("- []\n", "- []\n");
}

test "yaml printer: every key kind has a spelling that re-parses" {
    // A YAML mapping key is any node. The printer used to read every key as a
    // `.string` and crash on the rest (30 yaml-test-suite accept documents —
    // docs/tasks/yaml-printer-panics-on-accept-corpus.md); each kind now has
    // a spelling the parser reads back to the same node.
    // null (2JQS: `: a`), number (74H7, with its tag kept), boolean.
    try expectRoundTrip(": a\n", "null: a\n");
    try expectRoundTrip("23: x\n!!str 23: y\n", "23: x\n!!str 23: y\n");
    try expectRoundTrip("true: x\n", "true: x\n");
    // alias (26DV/E76Z) — the space before `:` is what keeps `*n` an alias.
    try expectRoundTrip("x: &n v\n*n : w\n", "x: &n v\n*n : w\n");
    // sequence (4FJ6/SBG9): explicit `?`, inlined as flow when it fits …
    try expectRoundTrip("? - a\n  - b\n: c\n", "? [a, b]\n: c\n");
    try expectRoundTrip("? []\n: c\n", "? []\n: c\n");
    // … and block beneath a bare `?` when it cannot (X38W: props inside).
    try expectRoundTrip(
        "{ &a [a, &b b]: *b, *a : c }\n",
        "? &a\n  - a\n  - &b b\n: *b\n*a : c\n",
    );
    // mapping (9MMW/M2N8-1), including one whose own key is a collection.
    try expectRoundTrip("? a: b\n: c\n", "? { a: b }\n: c\n");
    try expectRoundTrip("? []: x\n", "?\n  ? []\n  : x\n: null\n");
    // A collection key on a sequence item's first pair rides the `- `.
    try expectRoundTrip("- ? [[b, c]]\n  : d\n", "- ? [[b, c]]\n  : d\n");
    // Scalar keys other than strings inline in a flow mapping.
    try expectRoundTrip("x:\n  1: a\n  null: b\n", "x: { 1: a, null: b }\n");
}

test "yaml printer: a re-printed key re-parses to the same node kind" {
    const Parser = @import("parser.zig");
    const cases = [_][]const u8{
        ": a\n",
        "23: x\n",
        "x: &n v\n*n : w\n",
        "? - a\n  - b\n: c\n",
        "? a: b\n: c\n",
        "? []: x\n",
    };
    for (cases) |src| {
        const doc = try Parser.parse(std.testing.allocator, src, .v1_2_2);
        defer doc.deinit(std.testing.allocator);
        var out: Writer.Allocating = .init(std.testing.allocator);
        defer out.deinit();
        try print(&out.writer, &doc.ast);
        const again = try Parser.parse(std.testing.allocator, out.written(), .v1_2_2);
        defer again.deinit(std.testing.allocator);
        const want = doc.ast.nodes[doc.ast.nodes[doc.ast.nodes[doc.ast.root].kind.mapping.?].kind.keyvalue.key].kind;
        const got = again.ast.nodes[again.ast.nodes[again.ast.nodes[again.ast.root].kind.mapping.?].kind.keyvalue.key].kind;
        try std.testing.expectEqual(std.meta.activeTag(want), std.meta.activeTag(got));
    }
}

test "yaml flow: a flow indicator in a scalar forces quoting" {
    // `${{ github.ref }}` carries `{`/`}`, which would break the flow map — so the
    // value is single-quoted (it was safe bare in block position).
    try expectRoundTrip(
        "concurrency:\n  group: ci-${{ github.ref }}\n  x: 1\n",
        "concurrency: { group: 'ci-${{ github.ref }}', x: 1 }\n",
    );
}

test "yaml flow: output is idempotent (reparse + reprint is a fixed point)" {
    const Parser = @import("parser.zig");
    const src = "on:\n  push:\n    branches: [main]\nconcurrency: { group: ci, cancel: true }\n";
    const doc = try Parser.parse(std.testing.allocator, src, .v1_2_2);
    defer doc.deinit(std.testing.allocator);
    var first: Writer.Allocating = .init(std.testing.allocator);
    defer first.deinit();
    try print(&first.writer, &doc.ast);

    const doc2 = try Parser.parse(std.testing.allocator, first.written(), .v1_2_2);
    defer doc2.deinit(std.testing.allocator);
    var second: Writer.Allocating = .init(std.testing.allocator);
    defer second.deinit();
    try print(&second.writer, &doc2.ast);
    try std.testing.expectEqualStrings(first.written(), second.written());
}

/// Build `{ s: value }`, serialize it, and return the owned YAML (caller frees).
fn emitStringValue(allocator: std.mem.Allocator, value: []const u8) ![]u8 {
    var b = AST.Builder.init(allocator);
    defer b.deinit();
    const v = try b.addString(value);
    const k = try b.addString("s");
    const root = try b.addMapping(&.{.{ .key = k, .value = v }});
    var ast = try b.finish(root);
    defer ast.deinit();

    var out: Writer.Allocating = .init(allocator);
    defer out.deinit();
    try print(&out.writer, &ast);
    return allocator.dupe(u8, out.written());
}

test "yaml printer: multi-line string value emits a |- block scalar" {
    const yaml = try emitStringValue(std.testing.allocator, "multi\nline\ntext");
    defer std.testing.allocator.free(yaml);
    try std.testing.expectEqualStrings("s: |-\n  multi\n  line\n  text\n", yaml);
}

test "yaml printer: a trailing newline clips to |, two fall back to double-quote" {
    const clip = try emitStringValue(std.testing.allocator, "a\nb\n");
    defer std.testing.allocator.free(clip);
    try std.testing.expectEqualStrings("s: |\n  a\n  b\n", clip);

    const keep = try emitStringValue(std.testing.allocator, "a\nb\n\n");
    defer std.testing.allocator.free(keep);
    try std.testing.expectEqualStrings("s: \"a\\nb\\n\\n\"\n", keep);
}

test "yaml printer: indented interior lines still block-scalar (shell script shape)" {
    const yaml = try emitStringValue(std.testing.allocator, "if x; then\n  echo hi\nfi\n");
    defer std.testing.allocator.free(yaml);
    try std.testing.expectEqualStrings("s: |\n  if x; then\n    echo hi\n  fi\n", yaml);
}

test "yaml printer: block scalars round-trip through the parser" {
    const Parser = @import("parser.zig");
    const cases = [_][]const u8{
        "multi\nline\ntext", // clean -> |-
        "a\nb\n", // one trailing newline -> | (clip)
        "a\n\nb", // interior blank line
        "a\nb\n\n", // 2+ trailing newlines -> double-quote fallback
        "trailing \nspace", // trailing space on a line -> fallback
        " leading\nline", // leading space on FIRST line -> fallback (indent detection)
        "if x; then\n  echo hi\nfi\n", // indented INTERIOR lines -> | is fine
        "tab\there\nx", // interior tab is content -> | is fine
        "\rreturn\nx", // \r control char -> double-quote
    };
    for (cases) |s| {
        const yaml = try emitStringValue(std.testing.allocator, s);
        defer std.testing.allocator.free(yaml);

        var doc = try Parser.parse(std.testing.allocator, yaml, .v1_2_2);
        defer doc.deinit(std.testing.allocator);
        const root = doc.ast.nodes[doc.ast.root];
        const kv = doc.ast.nodes[root.kind.mapping.?].kind.keyvalue;
        try std.testing.expectEqualStrings(s, doc.ast.nodes[kv.value].kind.string);
    }
}

test "yaml printer: a %TAG directive is written back above the tags that use it" {
    // 6CK3/Z9M4: the tags are kept verbatim, so the handle they are spelled
    // with has to be declared, or the printer's own output is a document that
    // uses an undeclared handle (`UndefinedTagHandle`).
    const tagged =
        \\%TAG !e! tag:example.com,2000:app/
        \\---
        \\- !local foo
        \\- !!str bar
        \\- !e!tag%21 baz
        \\
    ;
    try expectRoundTrip(tagged, tagged);
    // A redefined default handle is just as load-bearing as a named one.
    const secondary =
        \\%TAG !! tag:example.com,2000:app/
        \\---
        \\- !!foo bar
        \\
    ;
    try expectRoundTrip(secondary, secondary);
    // A declaration nothing uses is dropped: writing it back would drag a `---`
    // marker onto a document that needs neither.
    try expectRoundTrip("%TAG !e! tag:example.com,2000:app/\n---\na: b\n", "a: b\n");
    // `%YAML` and reserved directives are inert — the output is 1.2-compatible
    // whatever the source declared — so they are not carried through either.
    try expectRoundTrip("%YAML 1.2\n---\na: b\n", "a: b\n");
}

test "yaml printer: a root collection's own props sit on a line above it" {
    // `&m\na: b` — the anchor is the ROOT mapping's (`node_anchors[root]`), and
    // dropping it left every `*m` alias to it dangling.
    try expectRoundTrip("&m\na: b\n", "&m\na: b\n");
    try expectRoundTrip("!!seq\n- a\n", "!!seq\n- a\n");
    try expectRoundTrip("&m !!map\na: b\n", "&m !!map\na: b\n");
    // A root with a single-line spelling keeps them inline instead.
    try expectRoundTrip("&m {}\n", "&m {}\n");
    try expectRoundTrip("!!str foo\n", "!!str foo\n");
}

test "yaml printer: a collection sequence item keeps its props on the dash line" {
    try expectRoundTrip("- &a\n  - x\n", "- &a\n  - x\n");
    // `- &a k: v` would bind `&a` to the KEY, so the props take the dash line
    // and the mapping starts beneath.
    try expectRoundTrip("- &a\n  k: v\n", "- &a\n  k: v\n");
    try expectRoundTrip("- !!map\n  k: v\n", "- !!map\n  k: v\n");
    try expectRoundTrip("- &a {}\n", "- &a {}\n");
    try expectRoundTrip("- &a []\n", "- &a []\n");
}

test "yaml printer: reprinted root/item props re-parse onto the same nodes" {
    const Parser = @import("parser.zig");
    const cases = [_][]const u8{
        "&m\na: b\n",
        "!!seq\n- a\n",
        "&m !!map\na: b\n",
        "- &a\n  - x\n",
        "- &a\n  k: v\n",
        "%TAG !e! tag:example.com,2000:app/\n---\n&m\na: !e!foo b\n",
    };
    for (cases) |src| {
        const doc = try Parser.parse(std.testing.allocator, src, .v1_2_2);
        defer doc.deinit(std.testing.allocator);
        var out: Writer.Allocating = .init(std.testing.allocator);
        defer out.deinit();
        try print(&out.writer, &doc.ast);
        const again = try Parser.parse(std.testing.allocator, out.written(), .v1_2_2);
        defer again.deinit(std.testing.allocator);
        try std.testing.expect(doc.ast.eql(again.ast));
        try std.testing.expect(doc.ast.tagsEql(again.ast));
        for (doc.ast.node_anchors, 0..) |want, id| {
            const got: ?[]const u8 = if (id < again.ast.node_anchors.len) again.ast.node_anchors[id] else null;
            try std.testing.expectEqual(want == null, got == null);
            if (want) |name| try std.testing.expectEqualStrings(name, got.?);
        }
    }
}
