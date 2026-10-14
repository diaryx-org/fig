//! Editor module, generic over Language.

const std = @import("std");
const build_options = @import("build_options");

const AST = @import("kit").AST;
const Document = @import("kit").Document;
const Span = @import("kit").Span;
const json = @import("fig_json");
const regions = @import("editor/regions.zig");
/// The source-coordinate utilities the engine and the regions module share.
/// Public so a language's editor tests (`languages/<name>/tests/editor.zig`)
/// can read them through `fig.editor.splice`.
pub const splice = @import("editor/splice.zig");
const lineStartBefore = splice.lineStartBefore;
const lineEndAfter = splice.lineEndAfter;
const firstNonSpace = splice.firstNonSpace;
const columnOf = splice.columnOf;
const isFlow = splice.isFlow;
const flowOpenEnd = splice.flowOpenEnd;
const Comments = splice.Comments;
const commentBlockStart = splice.commentBlockStart;
const appendBlockSep = splice.appendBlockSep;
const continuationEnd = splice.continuationEnd;
const Block = splice.Block;
const tileBlocks = splice.tileBlocks;
const fullOrder = splice.fullOrder;
const Region = regions.Region;
const log = std.log.scoped(.editor);

// The declared half of the Language interface — `Syntax`, `Caps`,
// `Comments`. Every surface-syntax parameter this engine used to select
// with an `if (Language == X)` branch now comes from `Language.syntax(t)`
// instead; see `languages/manifest.zig` and
// `docs/proposals/language-interface.md`. A leaf module, so importing it here
// (and from every `<lang>/<lang>.zig`) pulls in nothing else.
const lang = @import("kit").manifest;
const bare_literal = @import("kit").literal;

// The RENDERING half of the interface — the part `syntax` can't express.
//
// Where a format's editing need is a value (a separator, a marker, whether
// block sequences are editable), it is declared on `Language.syntax` and read
// above. Where it is a FACT of the parse — where an item's marker is, where
// an entry's separator is, which lines and name mentions belong to a section
// — the parser records it on `Document` and the engine reads it. What is
// left is how a format SPELLS a fragment, and for that a format declares a
// renderer: a `pub` decl on its `Language` struct — `renderValue`,
// `renderEntry`, `renderItem`, `renderTail`, `renderKey` — that is a pure
// function from strings to a string, given the dialect it is spelling for.
// This engine dispatches on presence —
//
//     if (self.hasRenderer(.tail)) return Language.renderTail(self.format, ...);
//
// — so it names no format at all, and it splices what a renderer returns
// under the same reparse net as every other edit. `hasRenderer` is
// `@hasDecl` for a compiled language, where presence is a fact of the
// source, and the language's own `hasRenderer(t, which)` for one whose
// renderers are function pointers filled at runtime, where presence is a
// null check; either way the answer gates the same branch, so the two
// carriers of `docs/proposals/runtime-languages.md` §4.3 drive one engine.
// No renderer receives the
// editor, performs a splice, or is called more than once per edit. There
// are no editing hooks any more: the twenty-five that existed each ended in
// one splice and needed only a fact the parser had dropped, an engine
// constant that was really syntax, or a string function, and each became
// one of those. See `docs/proposals/runtime-languages.md` §4.4. The
// implementations live in `<lang>/edit.zig` (and the language's editor tests
// in `languages/<lang>/tests/editor.zig`; was: `<lang>/editor_helper.zig`, which also held that
// language's editor tests), and the DECLARATION of which renderers a format
// supplies, with the reason, sits in the "Renderers" block of its
// `<lang>/<lang>.zig`.
//
// The bare language tags below are used by this file's OWN tests, nothing
// else. The whole-container ops are generic over `section_noun`,
// `section_header` and what the parser records, and refuse at comptime for a
// format that declares none of it; Zig has no conditional container-level
// declarations (`usingnamespace` was removed in 0.15), so the methods exist
// for every format.
const Toml = @import("fig_toml").Language;
const Fig = @import("fig_figl").Language;
const Yaml = @import("fig_yaml").Language;
const Zon = @import("fig_zon").Language;
const Dotenv = @import("fig_dotenv").Language;
const Properties = @import("fig_properties").Language;
const Ini = @import("fig_ini").Language;
const Plist = @import("fig_plist").Language;
const NestedText = @import("fig_nestedtext").Language;

/// What the bare-literal rule (`kit/literal.zig`) makes of `text`, trimmed
/// of whitespace: the `Literal` a value renderer is handed. `true`, `42`,
/// `2.5`, `2026-09-10` and `null` are typed and `Yes`, `007` and `TRUE`
/// stay strings, in every format alike.
pub fn literalOf(text: []const u8) lang.Literal {
    return switch (bare_literal.sniffBare(std.mem.trim(u8, text, " \t\r\n"))) {
        .null_ => .null,
        .boolean => .bool,
        .number => |n| if (n.kind == .integer) .int else .float,
        .datetime => .datetime,
        .string => .string,
    };
}

pub fn Editor(comptime Language: type) type {
    @import("contract/language.zig").validate(Language);
    return struct {
        const Self = @This();

        /// Everything this format declares about its own surface syntax, for
        /// the dialect the document is currently being read as. Replaces the
        /// per-language `if (Language == X)` parameter branches this file used
        /// to carry — the values now live on `<lang>/<lang>.zig`, checked by
        /// `language.validate`. See `languages/manifest.zig`.
        ///
        /// Indexed by `self.format` rather than fixed at comptime because the
        /// comment markers genuinely vary by dialect (strict JSON has no
        /// comment syntax; JSONC and JSON5 do), and every splice is reparsed
        /// under whichever dialect the editor is holding. The rest of the
        /// struct is dialect-invariant today; a TOML 1.0/1.1 or YAML
        /// 1.1/1.2.2 *editing* divergence would land here if one appeared.
        ///
        /// Cost is one runtime switch and it stops there: every consumer is an
        /// `appendSlice` or an argument to `commentBlockStart`/
        /// `entryBlockStart`, none of which needs a comptime value.
        fn syntax(self: *const Self) lang.Syntax {
            return Language.syntax(self.format);
        }

        /// Whether `node` is a FLOW container — spelled inline, edited by
        /// comma-aware splice — rather than a block one edited by line.
        ///
        /// The one place the engine asks. Four answers, in order:
        ///
        ///   0. A sequence the parser gave a separator of its own
        ///      (`Document.itemSep`) is flow: the language has said
        ///      how its items are joined, which is all the flow arm needs,
        ///      and its first byte may be an item's (an open sequence) or
        ///      any delimiter the language likes.
        ///   1. A format with no flow syntax (`Syntax.flow_containers ==
        ///      false`) has no flow containers, whatever a span begins with.
        ///   2. A section format's root, and any section node, is block: the
        ///      root's span opens on the first `[header]` (TOML, INI), and a
        ///      section node's span is its header's name token, so the
        ///      first-byte sniff would read either as a bracket-delimited
        ///      flow container. No section format spells a flow root — fig,
        ///      TOML and INI each reject one at parse.
        ///   3. Otherwise the first-byte sniff, `splice.isFlow`, which is
        ///      exact for every format that has both shapes.
        ///
        /// INI and TOML used to hook `insertKey` for no other reason than to
        /// skip the sniff on their root; this is that skip, stated once as an
        /// engine rule.
        fn isFlowNode(self: *const Self, parsed: Document, node: AST.Node) bool {
            if (parsed.itemSep(node) != null) return true;
            if (!self.syntax().flow_containers) return false;
            // `is_section_format` is "may be" for a runtime language, so the
            // dialect's own `section_noun` settles it at the call: a runtime
            // JSON's `[1, 2]` root is a flow sequence, not a section root.
            if (is_section_format and self.syntax().section_noun != null and (node.id == parsed.ast.root or parsed.isSection(node))) return false;
            if (self.isOpenRoot(parsed, node)) return true;
            const source = self.source.items;
            if (!isFlow(source, parsed.span(node), self.syntax().flow_map_open)) return false;
            // `flow_maps_only`: a `[` is not an opener, so what it begins
            // (a CSS attribute selector) is read as block.
            return !(self.syntax().flow_maps_only and source[firstNonSpace(source, parsed.span(node).start)] == '[');
        }

        /// Whether `node` is the root of a `flow_root` format: a flow mapping
        /// with no delimiters, whose members run from the source's first
        /// byte to its last (a CSS `style` attribute). The flow helpers ask
        /// it where they would otherwise step past an opener or look for a
        /// closer.
        fn isOpenRoot(self: *const Self, parsed: Document, node: AST.Node) bool {
            return self.syntax().flow_root and node.id == parsed.ast.root and node.kind == .mapping;
        }

        /// Whether the flow container `node` has no delimiters: an open root,
        /// or a sequence the parser said is open (a `.tune` voice's bars,
        /// running from after `Name:` to the end of the line). Its items
        /// start at its span's start, and nothing past its span is its.
        fn isOpen(self: *const Self, parsed: Document, node: AST.Node) bool {
            if (parsed.itemSep(node)) |is| return is.open;
            return self.isOpenRoot(parsed, node);
        }

        /// How the items of the flow sequence `node` are joined: the bytes
        /// written between two of them on one line, split around the token
        /// a delete consumes. The dialect's is `flow_entry_sep` and a space
        /// after it (`, `); a separated sequence's is the one its parser
        /// named (`Document.itemSep`), split at its whitespace
        /// (` | ` is `|` with a space either side; ` ` is all whitespace and
        /// no token, so a delete consumes only the run of whitespace). An
        /// insert into a flow mapping is joined by one too (`flowMapJoint`).
        const Joint = struct {
            lead: []const u8,
            token: []const u8,
            trail: []const u8,
            /// The language named this separator for this container. Such a
            /// sequence's items may be anything the language reads there, so
            /// no flow-only rule refuses a value before the reparse does.
            declared: bool,

            /// Whether the separator itself breaks the line (`"\n\n"`), so
            /// an item written after it starts a line of its own.
            fn breaksLine(j: Joint) bool {
                return std.mem.indexOfScalar(u8, j.lead, '\n') != null or std.mem.indexOfScalar(u8, j.trail, '\n') != null;
            }

            /// `text` split at its whitespace: what comes before its first
            /// other byte, those bytes, and what comes after.
            fn split(text: []const u8, declared: bool) Joint {
                const ws = " \t\r\n";
                const start = std.mem.indexOfNone(u8, text, ws) orelse return .{ .lead = text, .token = "", .trail = "", .declared = declared };
                const end = std.mem.lastIndexOfNone(u8, text, ws).? + 1;
                return .{ .lead = text[0..start], .token = text[start..end], .trail = text[end..], .declared = declared };
            }
        };

        fn jointOf(self: *const Self, parsed: Document, node: AST.Node) Joint {
            const dialect: Joint = .{ .lead = "", .token = self.syntax().flow_entry_sep, .trail = " ", .declared = false };
            const is = parsed.itemSep(node) orelse return dialect;
            const text = is.text orelse return .{ .lead = "", .token = dialect.token, .trail = " ", .declared = true };
            return Joint.split(text, true);
        }

        /// Append the bytes that go between an item and the next one of the
        /// flow sequence `node` to `out`, laid out as the sequence is: the
        /// separator as written when it breaks the line itself, then the
        /// first item's indent; the token, a newline and that indent when
        /// the sequence is one item per line; else the separator as written.
        fn writeJoint(self: *Self, out: *std.ArrayList(u8), parsed: Document, node: AST.Node, joint: Joint) !void {
            const source = self.source.items;
            const first = (try parsed.ast.child(&node)).?;
            const last = (try parsed.ast.lastChild(&node)).?;
            const col = columnOf(source, parsed.span(first).start);
            if (joint.breaksLine()) {
                try out.appendSlice(self.allocator, joint.lead);
                try out.appendSlice(self.allocator, joint.token);
                try out.appendSlice(self.allocator, joint.trail);
                try out.appendNTimes(self.allocator, ' ', col);
            } else if (try self.flowIsMultiLine(parsed, node, parsed.span(node), parsed.span(last).end)) {
                try out.appendSlice(self.allocator, joint.token);
                try out.append(self.allocator, '\n');
                try out.appendNTimes(self.allocator, ' ', col);
            } else {
                try out.appendSlice(self.allocator, joint.lead);
                try out.appendSlice(self.allocator, joint.token);
                try out.appendSlice(self.allocator, joint.trail);
            }
        }

        /// Where `node` BEGINS on its own line: its item marker's start when
        /// the parser recorded one (`Document.markerSpan`), else its
        /// span's. This is the byte every line-anchored computation starts
        /// from — the line an item lives on, the prefix a sibling copies, the
        /// block a delete or reorder takes — because a nested or empty item's
        /// span can start lines below the `-` that introduces it.
        fn markerStart(parsed: Document, node: AST.Node) usize {
            if (parsed.markerSpan(node)) |m| return m.start;
            return parsed.span(node).start;
        }

        /// The prefix a new line at the same nesting as the byte `at` needs,
        /// appended to `buf` and returned as a slice of it.
        ///
        /// For a format whose line prefix is STRUCTURAL (fig's `>` marker
        /// run, `Syntax.structural_indent`) it is the raw bytes from the line
        /// start to `at`, which reproduces the depth and the file's spaced or
        /// glued marker style. For every other format it is the line's
        /// leading whitespace, kept as the bytes it is so a tab-indented file
        /// stays tab-indented, padded with spaces out to `at`'s column only
        /// when what stands between is sequence item markers — a key inside
        /// a `- key: v` item, whose siblings align under the key and not
        /// under the dash. Past any other token — a value's `{` after its
        /// key, a section key inside `[…]`, a plist `<key>…</key>` — the
        /// line's own indent is the answer. The engine used to count a
        /// column and write that many spaces, which lost tabs and was fig's
        /// whole reason for hooking three insert operations.
        pub fn indentAt(self: *const Self, buf: *std.ArrayList(u8), at: usize) ![]const u8 {
            const source = self.source.items;
            const line_start = lineStartBefore(source, at);
            const from = buf.items.len;
            if (self.syntax().structural_indent) {
                try buf.appendSlice(self.allocator, source[line_start..at]);
            } else {
                const ws_end = @min(firstNonSpace(source, line_start), at);
                try buf.appendSlice(self.allocator, source[line_start..ws_end]);
                if (onlyItemMarkers(source[ws_end..at], self.syntax().seq_item_marker))
                    try buf.appendNTimes(self.allocator, ' ', at - ws_end);
            }
            return buf.items[from..];
        }

        /// Whether this format renders `which` for the editor's dialect.
        /// Comptime-known for a compiled language — the decl is there or it
        /// is not, and the branch it gates is not analyzed when it is not —
        /// and a runtime answer from a language that declares `hasRenderer`
        /// itself, which is how a vtable says which of its slots are null.
        /// `inline` is what makes the compiled answer comptime at the call
        /// site.
        inline fn hasRenderer(self: *const Self, comptime which: lang.Renderer) bool {
            if (@hasDecl(Language, "hasRenderer")) return Language.hasRenderer(self.format, which);
            return @hasDecl(Language, which.declName());
        }

        /// `value_text` as this format spells a value in place — rendered
        /// through the format's `renderValue` when it declares one (plist
        /// wraps every literal in a typed element), else verbatim. Appended to
        /// `buf` when rendered; the returned slice is what to splice.
        ///
        /// **Renderer** `renderValue(t, allocator, out, request) !void`,
        /// where the request's `literal` is `literalOf(value_text)`: the
        /// engine classifies the text once, by fig's own bare-literal rules,
        /// so a renderer spells a kind it is told rather than deciding one.
        /// `place` is the container the value is written into.
        fn renderedValue(self: *const Self, buf: *std.ArrayList(u8), value_text: []const u8, place: Place) ![]const u8 {
            if (!self.hasRenderer(.value)) return value_text;
            const from = buf.items.len;
            try Language.renderValue(self.format, self.allocator, buf, .{
                .value = value_text,
                .literal = literalOf(value_text),
                .parent_key = place.key,
                .parent_tag = place.tag,
            });
            return buf.items[from..];
        }

        /// Where a fragment is written, as a renderer is told it: the
        /// container it goes into, by the name of the key that container
        /// stands under and by its tag. See `lang.RenderRequest`.
        const Place = struct {
            key: []const u8 = "",
            tag: []const u8 = "",
        };

        /// `container` as a `Place`: the key of the entry holding it —
        /// empty at the root and for a sequence item — and its tag.
        fn placeOf(parsed: Document, container: AST.Node) Place {
            const ast = &parsed.ast;
            const key: []const u8 = if (ast.parentOf(container.id)) |p| switch (ast.kindOf(p)) {
                .keyvalue => |kv| if (kv.value == container.id) keyName(ast.node(kv.key)) else "",
                else => "",
            } else "";
            return .{
                .key = key,
                .tag = if (ast.tagOf(container.id)) |t| t.spelling() else "",
            };
        }

        /// The `Place` of the container holding node `id` — a value, an
        /// item, an entry or its key — or the empty place for the root.
        fn placeOfChild(parsed: Document, id: AST.Node.Id) Place {
            const ast = &parsed.ast;
            // The container is the parent, or — for an entry's key or
            // value — the entry's parent.
            var p = ast.parentOf(id) orelse return .{};
            if (ast.kindOf(p) == .keyvalue) p = ast.parentOf(p) orelse return .{};
            return switch (ast.kindOf(p)) {
                .mapping, .sequence => placeOf(parsed, ast.node(p)),
                else => .{},
            };
        }

        /// A key's name, decoded: what a path segment names it by.
        fn keyName(key: AST.Node) []const u8 {
            return switch (key.kind) {
                .string => |t| t,
                .number => |n| n.raw,
                .extended => |x| x.text,
                .boolean => |b| if (b) "true" else "false",
                else => "",
            };
        }

        /// Write one block-mapping entry after its line's `indent` has been
        /// written: the key, the separator and the value, with any further
        /// lines the entry spans prefixed by `indent` (plist's value element
        /// on a second line, NestedText's `>`-block). No trailing newline.
        ///
        /// **Renderer** `renderEntry(t, allocator, out, request) !void` —
        /// told the indent, key and value, and declared by a format whose entry is not
        /// `key`, `kv_sep`, value on one line. Without it the entry is the
        /// key followed by `writeMapValue`. `value_text` has been through
        /// `renderedValue` already.
        fn writeEntry(self: *Self, out: *std.ArrayList(u8), indent: []const u8, key_text: []const u8, value_text: []const u8, place: Place) !void {
            if (self.hasRenderer(.entry))
                return Language.renderEntry(self.format, self.allocator, out, .{ .indent = indent, .key = key_text, .value = value_text, .parent_key = place.key, .parent_tag = place.tag });
            try out.appendSlice(self.allocator, key_text);
            try self.writeTail(out, indent, key_text, value_text, place);
        }

        /// Write everything that follows a key on its entry: the separator
        /// and the value inline (`: v`, ` = v`), or the value re-framed onto
        /// the following lines as a block (`:` then indented lines; fig's
        /// bare header over a `>` body). `key_text` is the key as written —
        /// so a renderer can tell a NestedText multiline key, which takes no
        /// separator — or empty for the DOCUMENT ROOT, where the value stands
        /// alone. No trailing newline.
        ///
        /// **Renderer** `renderTail(t, allocator, out, request) !void`,
        /// told the indent, key and value. Without it the tail is `writeMapValue`, which
        /// is YAML's answer and the default for every line-structured format.
        /// `value_text` has been through `renderedValue`.
        fn writeTail(self: *Self, out: *std.ArrayList(u8), indent: []const u8, key_text: []const u8, value_text: []const u8, place: Place) !void {
            if (self.hasRenderer(.tail))
                return Language.renderTail(self.format, self.allocator, out, .{ .indent = indent, .key = key_text, .value = value_text, .parent_key = place.key, .parent_tag = place.tag });
            try self.writeMapValue(out, indent, value_text);
        }

        /// Write one block-sequence item after its line's `indent`: the
        /// `seq_item_marker` and the value, with continuation lines of a
        /// multi-line value re-indented past the marker. No trailing newline.
        ///
        /// **Renderer** `renderItem(t, allocator, out, request) !void` — told
        /// the indent, the value and the sequence (`place`), and declared by a format whose item is not marker-then-value
        /// (NestedText renders an empty or multi-line value as a `>`-block
        /// under a bare `-`). `value_text` has been through `renderedValue`.
        fn writeItem(self: *Self, out: *std.ArrayList(u8), indent: []const u8, value_text: []const u8, place: Place) !void {
            if (self.hasRenderer(.item))
                return Language.renderItem(self.format, self.allocator, out, .{ .indent = indent, .value = value_text, .parent_key = place.key, .parent_tag = place.tag });
            const marker = self.syntax().seq_item_marker;
            var cont: std.ArrayList(u8) = .empty;
            defer cont.deinit(self.allocator);
            try cont.appendSlice(self.allocator, indent);
            try cont.appendNTimes(self.allocator, ' ', marker.len);
            try out.appendSlice(self.allocator, marker);
            try reindentInto(out, self.allocator, value_text, cont.items);
        }

        /// Whether the entry `kv` of a block mapping is written OUTSIDE the
        /// mapping's own lines: its value is a section whose name on that
        /// line is a `.header` mention — the line opens the child's own
        /// region rather than sitting on the parent's. See
        /// `Document.mentionsOf` and `insertBlockKey`.
        fn outOfRegion(self: *const Self, parsed: Document, kv: AST.Node) bool {
            if (kv.kind != .keyvalue) return false;
            const val = parsed.ast.node(kv.kind.keyvalue.value);
            if (!parsed.isSection(val)) return false;
            const source = self.source.items;
            const line = lineStartBefore(source, parsed.span(kv).start);
            var mentions = parsed.mentionsOf(val.id);
            while (mentions.next()) |m| {
                if (m.kind == .header and lineStartBefore(source, m.span.start) == line) return true;
            }
            return false;
        }

        /// The end of the first header line recorded for the section
        /// `mapping` that is its OWN — not the header of one of its
        /// children. A section whose every header line opens a child — an
        /// implicit TOML table `a` made by `[a.b]`, a git config `[a "b"]`
        /// passing through `a` — has no line to take an entry under, and an
        /// entry spliced after the first of them would land in that child;
        /// it is refused (`ImplicitSection`). `insertContainer` is the op
        /// that can write it a header.
        fn ownHeaderLineEnd(self: *const Self, parsed: Document, mapping: AST.Node) !usize {
            const source = self.source.items;
            header: for (parsed.regionsOf(mapping.id)) |region| {
                const line = lineStartBefore(source, region.start);
                var cur = try parsed.ast.child(&mapping);
                while (cur) |kv| : (cur = parsed.ast.next(&kv)) {
                    if (self.outOfRegion(parsed, kv) and lineStartBefore(source, parsed.span(kv).start) == line)
                        continue :header;
                }
                return lineEndAfter(source, region.start);
            }
            return error.ImplicitSection;
        }

        /// Rewrite the EMPTY closed container at `span` (`<dict/>`,
        /// `<array></array>`) into its multi-line form around `body`, one
        /// entry or item already rendered against the child indent: the open
        /// token, the body one `indent_unit` under the container's own line
        /// prefix, and the close token back at that prefix. See
        /// `Syntax.closed_containers`.
        fn expandEmptyContainer(self: *Self, span: Span, delims: lang.Delimiters, base: []const u8, body: []const u8) !void {
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            try out.appendSlice(self.allocator, delims.open);
            try out.append(self.allocator, '\n');
            try out.appendSlice(self.allocator, base);
            try out.appendSlice(self.allocator, self.syntax().indent_unit);
            try out.appendSlice(self.allocator, body);
            try out.append(self.allocator, '\n');
            try out.appendSlice(self.allocator, base);
            try out.appendSlice(self.allocator, delims.close);
            try self.replaceAtSpan(span, out.items);
        }

        /// Whether `path`'s final segment names a key the document RESOLVES but
        /// no physical entry declares — one supplied by the format's reference
        /// layer, which path navigation does not follow and therefore reports
        /// as `NotFound`.
        ///
        /// Only a format with a `Syntax.merge_key` (YAML's `<<`) has such
        /// keys; for every other the answer is false, so the two `NotFound`
        /// recovery sites below need no language test of their own. The
        /// resolution itself is core (`AST.mergedChild`), and only the
        /// QUESTION is asked here: `replaceValAtPath` answers it by shadowing
        /// the inherited key with a local entry (copy-on-write) and
        /// `deleteKey` by refusing outright, and both of those policies are
        /// generic.
        fn keyIsInherited(self: *const Self, parsed: Document, path: []const AST.PathSegment) !bool {
            if (self.syntax().merge_key == null) return false;
            if (path.len == 0 or std.meta.activeTag(path[path.len - 1]) != .key) return false;
            const parent = parsed.ast.getValByPath(path[0 .. path.len - 1]) catch return false;
            if (parent.kind != .mapping) return false;
            return (parsed.ast.mergedChild(parent, path[path.len - 1].key) catch return false) != null;
        }

        allocator: std.mem.Allocator,
        source: std.ArrayList(u8) = .empty,
        document: ?Document = null,
        format: Language.Type = Language.default_type,
        /// Set by `replaceAtSpan` when the reparse it does after splicing
        /// FAILED — i.e. the caller's replacement text, not the document it
        /// went into, is what doesn't parse (the document parsed at `init`,
        /// and every edit routes through that one splice gate). The error
        /// itself can't say this: it is an ordinary parse error, identical to
        /// what a malformed input file produces. Callers that hand a user's
        /// raw text through — the CLI's `replace`/`set`/`insert` — read this to
        /// blame the text instead of the file. Cleared at the top of each
        /// splice, so it always describes the most recent one.
        splice_rejected: bool = false,

        pub fn getParsed(self: *const Self) !Document {
            return self.document orelse {
                log.err("Not initialized!", .{});
                return error.NotInitialized;
            };
        }

        pub fn init(self: *Self, input: []const u8) !void {
            if (self.source.items.len != 0 or self.document != null) return error.MultipleInit;
            try self.source.appendSlice(self.allocator, input);
            self.document = try self.parseSource();
        }

        /// Replace a span with a new span. Atomic: on success `self.document` is
        /// the reparse of the edited source; if the edit produces source that no
        /// longer parses, the source is rolled back and the prior `self.document`
        /// stays valid, so a failed edit leaves the editor exactly as it was.
        ///
        /// A line inserted where the line before it continues
        /// (`Syntax.line_continuation`) is written after a blank line, which
        /// ends the continuation; written straight after it, the new line
        /// would be read as the rest of it. Every op that adds a line — an
        /// entry, a dangling or leading comment — inserts it here.
        pub fn replaceAtSpan(self: *Self, span: Span, replacement: []const u8) !void {
            // Snapshot the whole source so a failed reparse can be undone. The
            // edit already costs a full reparse, so an O(n) copy is negligible.
            const backup = try self.allocator.dupe(u8, self.source.items);
            defer self.allocator.free(backup);

            self.splice_rejected = false;
            const end_continuation = if (span.len() == 0)
                continuationEnd(self.source.items[0..span.start], replacement, self.syntax())
            else
                "";
            try self.replaceSource(span, replacement);
            try self.replaceSource(Span.init(span.start, span.start), end_continuation);
            self.reparse() catch |err| {
                // Restore byte-for-byte. Capacity is retained from before the
                // edit (>= backup.len), so the refill cannot fail.
                self.source.clearRetainingCapacity();
                self.source.appendSliceAssumeCapacity(backup);
                // `backup` parsed at `init`, so the only new thing in the
                // buffer was `replacement`: it is what `err` is about.
                self.splice_rejected = true;
                return err;
            };
        }

        /// Replace the value at `path`. Reference-layer behavior is copy-on-write:
        /// editing a value that is an alias (`b: *x`) replaces the `*x` text with
        /// the new literal (severing only that alias — its anchor and any other
        /// alias are untouched), which falls out of splicing the alias node's own
        /// span. A key supplied only by a `<<` merge is materialized locally,
        /// shadowing the merge. Use `replaceValAtPathFollowing` to edit through to
        /// a shared anchor instead.
        ///
        /// Two things stand between `replacement` and the old value's slot,
        /// each a fact the format states rather than logic it supplies:
        ///
        ///   * The slot is not a bare literal. plist wraps every value in a
        ///     typed element (`<integer>42</integer>`), so the text is
        ///     RENDERED through the format's `renderValue` first.
        ///   * The slot's shape can change. Where the parser recorded the
        ///     entry's separator (`Document.sepSpan` — YAML, fig,
        ///     NestedText), everything after the key is rewritten through
        ///     `writeTail`, so a block collection can descend onto the
        ///     following lines where the old value was inline (`k: []` → a
        ///     block list), which a span splice cannot express. An item
        ///     reframes the same way from its recorded marker when the format
        ///     renders items.
        ///
        /// **Engine rule**: a SECTION node — one the parser recorded header
        /// lines for (`Document.regionsOf`) — is refused before the generic
        /// splice runs (`CannotReplaceSection` / `CannotReplaceSection` /
        /// `CannotReplaceSection`, in the format's own vocabulary). In TOML
        /// and INI a container's node span is only its KEY segment inside a
        /// `[header]` (or a dotted `a.b = 1` key) rather than any value text:
        /// a container's body is assembled from lines the span never covers,
        /// so splicing `replacement` into that span would rewrite the header's
        /// NAME and silently rename the section. A format whose block
        /// container hangs under a bare header (fig) records a zero-width
        /// separator there, and the reframe — which runs first — rewrites the
        /// container's body in place. The whole-container ops
        /// (`deleteContainer`, `renameContainer`, …) are what handle a section
        /// otherwise.
        ///
        /// The result is held to what a replace promises (`readsBack`): the
        /// value reads back as `replacement` reads, and nothing else changed,
        /// or `EditReadsBackWrong` and the source as it was.
        pub fn replaceValAtPath(self: *Self, path: []const AST.PathSegment, replacement: []const u8) !void {
            const parsed = try self.getParsed();
            // A path that resolves to nothing is the splice's to report, or
            // an inherited key's insert, which is checked as an insert.
            const node = parsed.ast.getValByPath(path) catch return self.spliceValue(path, replacement);
            var guard = try self.replaceGuard(parsed, node, path.len == 0, replacement);
            defer guard.deinit(self.allocator);
            try self.spliceValue(path, replacement);
            try self.readsBack(&guard);
        }

        fn spliceValue(self: *Self, path: []const AST.PathSegment, replacement: []const u8) !void {
            const parsed = try self.getParsed();
            const node = parsed.ast.getValByPath(path) catch |err| {
                // An inherited key surfaces as NotFound (path nav doesn't follow
                // the reference layer); copy-on-write it by inserting a local
                // `key: value` entry that shadows what it inherits from.
                if (err == error.NotFound and try self.keyIsInherited(parsed, path)) {
                    try self.insertKey(path[0 .. path.len - 1], path[path.len - 1].key, replacement);
                    return;
                }
                return err;
            };
            const span = parsed.span(node);
            const source = self.source.items;
            const place = placeOfChild(parsed, node.id);
            var buf: std.ArrayList(u8) = .empty;
            defer buf.deinit(self.allocator);
            const rendered = try self.renderedValue(&buf, replacement, place);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            var indent_buf: std.ArrayList(u8) = .empty;
            defer indent_buf.deinit(self.allocator);

            // The document root: the value stands alone. A format that
            // renders tails spells it (NestedText's `>` block); the rest
            // splice it as written.
            if (path.len == 0) {
                if (!self.hasRenderer(.tail)) return self.replaceAtSpan(span, rendered);
                try self.writeTail(&out, "", "", rendered, place);
                try self.terminateLine(&out, span.end);
                return self.replaceAtSpan(Span.init(0, span.end), out.items);
            }
            // A mapping value whose entry records its separator is REFRAMED:
            // everything after the key is rewritten through `writeTail`, so
            // the value may change shape between inline and block. This runs
            // ahead of the section veto because a format whose block
            // container hangs under a bare header (fig) records a zero-width
            // separator there, and its reframe rewrites the container's body
            // in place. See `Document.sepSpan`.
            if (std.meta.activeTag(path[path.len - 1]) == .key) {
                const kv = try parsed.ast.getNodeByPath(path);
                if (parsed.sepSpan(kv)) |sep| {
                    const key_span = parsed.span(parsed.ast.node(kv.kind.keyvalue.key));
                    const indent = try self.indentAt(&indent_buf, key_span.start);
                    try self.writeTail(&out, indent, source[key_span.start..key_span.end], rendered, place);
                    // A null value is a zero-width span at the separator.
                    const end = @max(span.end, sep.end);
                    try self.terminateLine(&out, end);
                    return self.replaceAtSpan(Span.init(key_span.end, end), out.items);
                }
            }
            // The engine's veto, before anything is spliced — a target whose
            // span is a header/dotted KEY, not a value slot.
            if (parsed.isSection(node)) return self.refuse(.replace);
            // An empty value whose span sits exactly at the key's end was
            // written with NO separator — a bare `.properties` key, `flag`
            // alone on its line — so the value cannot simply take the slot:
            // `flagx` would read back as a longer key. The separator goes in
            // with it, as an insert would write it.
            if (span.start == span.end and std.meta.activeTag(path[path.len - 1]) == .key) {
                const kv = try parsed.ast.getNodeByPath(path);
                const key_span = parsed.span(parsed.ast.node(kv.kind.keyvalue.key));
                if (span.start == key_span.end) {
                    if (self.syntax().kv_sep) |sep| {
                        try out.appendSlice(self.allocator, sep);
                        try out.appendSlice(self.allocator, rendered);
                        return self.replaceAtSpan(span, out.items);
                    }
                }
            }
            // A sequence item is reframed the same way when the format
            // renders items and the parser recorded the item's marker: the
            // marker and value are rewritten together through `writeItem`.
            if (self.hasRenderer(.item) and std.meta.activeTag(path[path.len - 1]) == .index) {
                if (parsed.markerSpan(node)) |m| {
                    const indent = try self.indentAt(&indent_buf, m.start);
                    try self.writeItem(&out, indent, rendered, place);
                    try self.terminateLine(&out, span.end);
                    return self.replaceAtSpan(Span.init(m.start, span.end), out.items);
                }
            }
            // A block item spliced in place keeps its marker and what stands
            // between that and the value (a comment, an anchor's line), and
            // a block value's later lines go under the item's content column
            // (`- x: 1` over `  y: two`). At column 0 they would leave the
            // item: `y` read as a key of the mapping around the list. A
            // block scalar's span takes its last newline, which the item
            // gives back (`terminateLine`). Where a line's prefix is
            // structure (figl's `>`), a value's later lines are its own: a
            // `'''` string's text stays as written.
            if (std.meta.activeTag(path[path.len - 1]) == .index and parsed.markerSpan(node) != null and
                !self.syntax().structural_indent)
            {
                const indent = try self.indentAt(&indent_buf, span.start);
                try reindentInto(&out, self.allocator, rendered, indent);
                try self.terminateLine(&out, span.end);
                return self.replaceAtSpan(span, out.items);
            }
            try self.replaceAtSpan(span, rendered);
        }

        /// A reframe ends at `end`; when that byte sits at a LINE START —
        /// the old value was an implicit empty whose zero-width span the
        /// parser anchored on the following line, an empty document, or the
        /// file's end after its last newline — the rewritten text has
        /// consumed the old line's terminator (or never had one) and
        /// supplies its own. Everywhere else the original newline still
        /// follows the splice.
        fn terminateLine(self: *Self, out: *std.ArrayList(u8), end: usize) !void {
            const source = self.source.items;
            const at_line_start = end == 0 or source[end - 1] == '\n';
            if (at_line_start and !std.mem.endsWith(u8, out.items, "\n"))
                try out.append(self.allocator, '\n');
        }

        /// Upsert a mapping value: replace the value at `path`, or — when only
        /// the trailing key is absent — insert it as a fresh `key: value` entry
        /// in the parent mapping. This is the "set this key, creating it if
        /// missing" primitive every config editor reaches for; it folds the
        /// usual `replaceValAtPath` → (on `NotFound`) `insertKey` two-step into
        /// one op.
        ///
        /// The path's last segment MUST name a key — `set` only ever *creates* a
        /// mapping entry, never a sequence item, so a path ending in an index is
        /// rejected with `NotAMapping`. Missing *intermediate* containers are
        /// auto-vivified (`mkdir -p` for config): if the parent mapping doesn't
        /// exist yet, `set` seeds it — and any of ITS missing ancestors, deepest
        /// first — as an empty map, then lands the leaf. Vivification fires when
        /// an intermediate key is genuinely absent (`NotFound`), or when what
        /// stands in its place is an EMPTY node — a null, i.e. a bare `key:` or an
        /// empty document's root, which is a container waiting to exist. A
        /// segment that resolves to a non-map SCALAR is a real type error
        /// (`NotAMapping`) and is never clobbered. See `Syntax.empty_map_literal`
        /// for what a seed is spelled as per format (YAML block, flow `{}`
        /// elsewhere).
        ///
        /// Vivify-then-land is atomic as a whole: if the leaf cannot be placed
        /// after a seed has been spliced, the seed is rolled back, so a failed
        /// `set` leaves the document byte-for-byte as it was. Where the seeds are
        /// FLOW containers (every format but YAML) that has one consequence
        /// callers see: a block-spelled value cannot land inside one, and is
        /// refused with `BlockValueIntoFlow` rather than spliced into text that
        /// no longer means what was written.
        ///
        /// Delegates to `replaceValAtPath`, so the replace case inherits that
        /// op's YAML value reframing (inline↔block) and merge-key COW.
        ///
        /// Key duality: the replace branch matches the trailing segment
        /// *logically* (against decoded key names), but the insert branch needs
        /// the key as *syntax*. `set` bridges the two — when it inserts, it
        /// renders the logical key into the format's key syntax (quoting/escaping
        /// it for strict JSON, verbatim for YAML/TOML where a simple key already
        /// is its own syntax) — so creating a not-yet-present key works for every
        /// editable format, JSON included.
        pub fn set(self: *Self, path: []const AST.PathSegment, value_text: []const u8) !void {
            if (path.len == 0 or std.meta.activeTag(path[path.len - 1]) != .key)
                return error.NotAMapping;
            self.replaceValAtPath(path, value_text) catch |replace_err| {
                // The value isn't there to replace — create it. The trailing key
                // is logical (it just matched against decoded names), so render it
                // into the format's key syntax before splicing. `insertKey`
                // re-validates the parent (a mapping, or an empty/null root it
                // promotes), so a non-mapping parent still errors; surface the
                // original replace error when the insert can't proceed.
                // `NotAMapping` falls back beside `NotFound` because it is what
                // navigating into a freshly-created, still-empty document says.
                //
                // Every other replace error means the key IS there and the
                // replace was refused for cause — a section veto
                // (`CannotReplaceSection` …), or a splice the reparse rolled back
                // — and an insert does not cure that: it would write a second
                // entry of the same name beside the first. Those surface as
                // they are.
                if (replace_err != error.NotFound and replace_err != error.NotAMapping)
                    return replace_err;
                const key_text = try self.formatInsertKey(path[path.len - 1].key);
                defer self.allocator.free(key_text);
                self.insertKey(path[0 .. path.len - 1], key_text, value_text) catch |insert_err| {
                    // One insert failure is more informative than any replace
                    // error can be, so it is NOT swallowed by the fallback
                    // below: the parent was found and IS a mapping, just a flow
                    // one that cannot hold this value. Reporting the replace's
                    // `NotFound` there would send the caller looking for a
                    // missing key that isn't the problem.
                    // `ImplicitSection` is the same: the parent is there,
                    // with no line of its own to take the entry under, and
                    // `EditReadsBackWrong`: the entry went in and read wrong.
                    if (insert_err == error.BlockValueIntoFlow or insert_err == error.ImplicitSection or
                        insert_err == error.EditReadsBackWrong) return insert_err;
                    // The parent mapping itself is missing (an intermediate key
                    // is absent — `NotFound`, NOT the `NotAMapping` of a scalar
                    // standing where a map should be, which must never be
                    // clobbered). Auto-vivify it as an empty map — recursing to
                    // seed any missing ancestor deepest-first — then retry the
                    // leaf insert into the now-existing parent.
                    //
                    // INI can't take part: `Syntax.empty_map_literal` is a value-literal
                    // sentinel (`{}`/`.{}`), and that spelling is only safe
                    // because it's ALSO genuinely valid syntax for "an empty
                    // mapping" in every other editable format (a real JSON
                    // object / YAML-TOML-fig flow map / ZON struct literal) —
                    // so vivifying through it can't produce anything wrong
                    // regardless of why it was reached. INI has no such
                    // literal (`{}` there is just a two-character STRING
                    // value; only a `[section]` header introduces real
                    // nesting), so blindly reusing the sentinel would splice
                    // a nonsense `section = {}` root key instead of a real
                    // section. Skip the vivify and surface the original
                    // `NotFound` — "no such section" — rather than corrupt.
                    // plist joins INI in opting out: `set`'s vivify seed is the
                    // flow `{}` literal, which plist has no reader for (an empty
                    // dict is `<dict/>`, not a value literal you can splice as
                    // `value_text`). Rather than teach the seed a plist spelling,
                    // surface the original `NotFound` — an intermediate `<dict>`
                    // must already exist to land a nested key.
                    // NestedText joins INI/plist here: its printer never
                    // writes the inline `{}` form, so a vivified ancestor
                    // would be a shape the format itself avoids, holding a
                    // value that then has to stay inline.
                    //
                    // `NotAMapping` joins `NotFound` as vivifiable in exactly
                    // one shape: when what stands in the way is an EMPTY node (a
                    // null), not a scalar. Navigating *through* a null fails the
                    // same way navigating through a scalar does, but they are
                    // opposite cases — a null is "nothing here yet" (an empty
                    // document's root, or a bare `key:`), which `insertKey`
                    // promotes to a real mapping, while a scalar is real data.
                    // Without this, a nested `set` on an empty document failed
                    // outright, though `set` is documented to seed one.
                    const vivifiable = insert_err == error.NotFound or
                        (insert_err == error.NotAMapping and
                            self.blockedByEmptyNode(path[0 .. path.len - 1]));
                    // A format with no `empty_map_literal` has no literal
                    // spelling for "an empty nested mapping" to seed WITH, so
                    // it cannot vivify at all — INI, plist and NestedText.
                    // The opt-out and the seed are one declaration because
                    // this is the literal's only consumer.
                    const seed = self.syntax().empty_map_literal;
                    if (seed != null and path.len >= 2 and vivifiable) {
                        // Vivify-then-insert is TWO splices, so it needs a
                        // snapshot of its own: each `replaceAtSpan` is
                        // individually atomic, but if the leaf insert fails
                        // after the ancestor seed landed, that seed is a
                        // half-finished edit no caller asked for — one that
                        // makes `Err` mean "nothing happened" a lie, and a
                        // retry an edit against an unexpected document.
                        // Restore, and report the failure as the whole `set`
                        // failing.
                        //
                        // Written as two `catch`es rather than one helper
                        // wrapping both splices: `set` recurses through the seed
                        // below, and routing that recursion through a helper
                        // makes the two functions' inferred error sets depend on
                        // each other (a comptime dependency loop).
                        const parent = path[0 .. path.len - 1];
                        const backup = try self.allocator.dupe(u8, self.source.items);
                        defer self.allocator.free(backup);
                        self.set(parent, seed.?) catch |seed_err| {
                            try self.restoreSource(backup);
                            return seed_err;
                        };
                        self.insertKey(parent, key_text, value_text) catch |leaf_err| {
                            try self.restoreSource(backup);
                            return leaf_err;
                        };
                    } else return if (insert_err == error.ContainerClosesOnItsLine) insert_err else replace_err;
                };
            };
        }

        /// Append `value_text` to the sequence at `path`, starting the
        /// sequence first when it is not there: `set`'s upsert for a list,
        /// as `set` is `insertKey`'s. A sequence that is there takes the item
        /// exactly as `appendToSeq` would place it.
        ///
        /// An absent key or a null stands in for the list, and is replaced
        /// by `Syntax.empty_seq_literal` through `set` — so any missing
        /// parent mapping is vivified with it — before the item lands. A
        /// scalar or a mapping there is real data, refused as `appendToSeq`
        /// refuses it (`NotASequence`), and never clobbered. A format with
        /// no empty-sequence spelling cannot start one, and reports the
        /// lookup's own `NotFound`.
        ///
        /// Seed-then-append is atomic as a whole, as `set`'s vivify-then-land
        /// is: an item the new sequence refuses takes the seed with it.
        pub fn setAppend(self: *Self, path: []const AST.PathSegment, value_text: []const u8) !void {
            if (!self.seqMissing(path)) return self.appendToSeq(path, value_text);
            const backup = try self.allocator.dupe(u8, self.source.items);
            defer self.allocator.free(backup);
            if (!try self.seedSeq(path)) return self.appendToSeq(path, value_text);
            self.appendToSeed(path, value_text) catch |err| {
                try self.restoreSource(backup);
                return err;
            };
        }

        /// Whether the sequence at `path` is yet to exist: the path does not
        /// resolve, or resolves to a null. What blocks it may still be a
        /// scalar further up, which `set` refuses when `seedSeq` asks it.
        fn seqMissing(self: *Self, path: []const AST.PathSegment) bool {
            const parsed = self.getParsed() catch return false;
            const node = parsed.ast.getValByPath(path) catch return true;
            return node.kind == .null_;
        }

        /// Write `Syntax.empty_seq_literal` at `path` with `set`, returning
        /// whether there was one to write. A path `set` cannot create — the
        /// root, or one ending in an index — is left for the caller's op to
        /// refuse, as is a format with no literal.
        fn seedSeq(self: *Self, path: []const AST.PathSegment) !bool {
            const seed = self.syntax().empty_seq_literal orelse return false;
            if (path.len == 0 or std.meta.activeTag(path[path.len - 1]) != .key) return false;
            try self.set(path, seed);
            return true;
        }

        /// `appendToSeq` into a sequence `seedSeq` started. ZON's `.{}` is
        /// its empty tuple and its empty struct at once, and parses as the
        /// struct; the seed is a list by declaration, so an empty flow
        /// mapping in its place takes the item as a list's first, and parses
        /// as a list from then on.
        fn appendToSeed(self: *Self, path: []const AST.PathSegment, value_text: []const u8) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind == .mapping and node.kind.mapping == null and self.isFlowNode(parsed, node))
                return self.insertFlowItem(parsed, node, parsed.span(node), false, value_text);
            return self.appendToSeq(path, value_text);
        }

        /// Whether what blocks navigation to `path` is an EMPTY node — a null —
        /// rather than real data. Walks back from `path` to the deepest prefix
        /// that still resolves and reports whether that node is a null; an
        /// unresolvable path with a null root (an empty document) counts too.
        ///
        /// This is the distinction `set`'s vivify guard needs and `NotAMapping`
        /// alone cannot make: a null is a container waiting to exist, which
        /// `insertKey` promotes, while a scalar standing where a mapping should
        /// be is data that must never be clobbered.
        fn blockedByEmptyNode(self: *Self, path: []const AST.PathSegment) bool {
            const parsed = self.getParsed() catch return false;
            var len = path.len;
            while (len > 0) : (len -= 1) {
                const node = parsed.ast.getValByPath(path[0..len]) catch continue;
                return node.kind == .null_;
            }
            return parsed.ast.node(parsed.ast.root).kind == .null_;
        }

        /// Restore the source to `backup` and reparse, undoing a compound
        /// (multi-splice) op. `backup` is a snapshot of source that parsed, so
        /// the reparse only fails on OOM. Capacity is retained from before the
        /// edits (>= `backup.len`), so the refill cannot fail — the same
        /// reasoning `replaceAtSpan`'s own rollback rests on.
        fn restoreSource(self: *Self, backup: []const u8) !void {
            self.source.clearRetainingCapacity();
            self.source.appendSliceAssumeCapacity(backup);
            try self.reparse();
        }

        /// Spell a logical mapping key as this format writes a key — for
        /// the `set` insert branch, and for a caller of `insertKey` that has
        /// a key NAME rather than key syntax. A language that declares
        /// `renderKeyName` spells it (JSON quotes it, ZON writes `.name`,
        /// TOML quotes what is not bare); otherwise the name is the syntax.
        /// Always returns an owned slice (the caller frees it).
        ///
        /// **Renderer** `renderKeyName(t, allocator, out, request) !void`,
        /// told the name as `key`.
        pub fn formatInsertKey(self: *Self, key: []const u8) ![]u8 {
            if (!self.hasRenderer(.key_name)) return self.allocator.dupe(u8, key);
            var out: std.ArrayList(u8) = .empty;
            errdefer out.deinit(self.allocator);
            try Language.renderKeyName(self.format, self.allocator, &out, .{ .key = key });
            return out.toOwnedSlice(self.allocator);
        }

        /// Like `replaceValAtPath`, but follow into the reference layer: when the
        /// target value is an alias, edit the *anchored node* (the shared source),
        /// so every alias to that anchor reflects the change. The `&name` (and any
        /// tag) prefix is preserved — only the anchored value's bytes are
        /// replaced. A non-alias target behaves exactly like `replaceValAtPath`.
        ///
        /// The alias kind, its resolution (`AST.resolveAlias`) and the anchor
        /// and tag span tables are all core, so this is generic: a format
        /// without a reference layer never produces an alias node, and for
        /// it "following" and not following are the same operation.
        pub fn replaceValAtPathFollowing(self: *Self, path: []const AST.PathSegment, replacement: []const u8) !void {
            const parsed = try self.getParsed();
            const node = parsed.ast.getValByPath(path) catch {
                return self.replaceValAtPath(path, replacement);
            };
            if (node.kind == .alias) {
                const target = parsed.ast.node(try parsed.ast.resolveAlias(node));
                // Held to `readsBack` at the anchored node, as a replace
                // is at its own: the aliases read as they did, by name.
                var guard = try self.replaceGuard(parsed, target, target.id == parsed.ast.root, replacement);
                defer guard.deinit(self.allocator);
                var buf: std.ArrayList(u8) = .empty;
                defer buf.deinit(self.allocator);
                try self.replaceAtSpan(self.valueSpanWithoutProps(parsed, target), try self.renderedValue(&buf, replacement, placeOfChild(parsed, target.id)));
                return self.readsBack(&guard);
            }
            try self.replaceValAtPath(path, replacement);
        }

        /// The span of `node`'s value bytes, excluding any leading anchor or
        /// tag property (the node's stored span starts at the property), so
        /// that editing an anchored value keeps its anchor intact.
        fn valueSpanWithoutProps(self: *const Self, parsed: Document, node: AST.Node) Span {
            const source = self.source.items;
            const full = parsed.span(node);
            var start = full.start;
            if (parsed.anchorSpan(node)) |a| start = @max(start, a.end);
            if (parsed.tagSpan(node)) |t| start = @max(start, t.end);
            while (start < full.end and (source[start] == ' ' or source[start] == '\t')) start += 1;
            return Span.init(start, full.end);
        }

        /// Rename the key at `path`, leaving its value untouched. `replacement`
        /// is key SYNTAX, spliced as given; a section's name is rewritten
        /// wherever the format spells it, and a format whose key syntax has
        /// more than one form spells the new key through `renderKey`.
        ///
        /// A path that names no key — a sequence item, or the root — is
        /// refused with `NotAKey`: `getKeyByPath` hands back the node itself
        /// there, and the splice would overwrite its VALUE. A rename that
        /// gives the mapping a second entry of one name is rolled back with
        /// `DuplicateKey`, counted as `insertKey` counts, because most
        /// parsers here accept the repeat and the reparse cannot be the net.
        pub fn replaceKeyAtPath(self: *Self, path: []const AST.PathSegment, replacement: []const u8) !void {
            if (path.len == 0 or path[path.len - 1] == .index) return error.NotAKey;
            const parent_path = path[0 .. path.len - 1];
            const before = try self.getParsed();
            const repeated_before = if (before.ast.getValByPath(parent_path)) |parent| try self.repeatedKeyCount(before.ast, parent) else |_| 0;
            const backup = try self.allocator.dupe(u8, self.source.items);
            defer self.allocator.free(backup);
            try self.spliceKeyAtPath(path, replacement);
            const after = try self.getParsed();
            const parent_after = after.ast.getValByPath(parent_path) catch return;
            if (try self.repeatedKeyCount(after.ast, parent_after) > repeated_before) {
                try self.restoreSource(backup);
                return error.DuplicateKey;
            }
        }

        /// The splice behind `replaceKeyAtPath`, once the path is known to
        /// name a key.
        fn spliceKeyAtPath(self: *Self, path: []const AST.PathSegment, replacement: []const u8) !void {
            const parsed = try self.getParsed();
            // A section node's name is written wherever the format spells
            // it — every `[a.b]` sharing the prefix, every dotted line, every
            // reopening — and only the first has a key node. Splicing that
            // one span would leave the rest behind and SPLIT the container,
            // so a section renames through its recorded mentions.
            if (path.len > 0) {
                if (parsed.ast.getValByPath(path)) |value| {
                    const mentions = parsed.mentionsOf(value.id);
                    if (parsed.isSection(value) and namesIn(mentions) > 0)
                        return self.rewriteMentions(mentions, replacement);
                } else |_| {}
            }
            const node = try parsed.ast.getKeyByPath(path);
            const span = parsed.span(node);
            // **Renderer** `renderKey(t, allocator, out, request) !void`,
            // told the indent, the new key and the old one — spells the new key in the form the old one's
            // syntax allows, given the old key as written: NestedText's
            // plain `key:` versus multiline `: key`, whose span carries no
            // separator and starts at its line's indent. Without it the key
            // is spliced verbatim.
            if (self.hasRenderer(.key)) {
                const source = self.source.items;
                var out: std.ArrayList(u8) = .empty;
                defer out.deinit(self.allocator);
                var indent_buf: std.ArrayList(u8) = .empty;
                defer indent_buf.deinit(self.allocator);
                const line_start = lineStartBefore(source, span.start);
                const indent = try self.indentAt(&indent_buf, firstNonSpace(source, line_start));
                const place = placeOfChild(parsed, node.id);
                try Language.renderKey(self.format, self.allocator, &out, .{ .indent = indent, .key = replacement, .old_key = source[span.start..span.end], .parent_key = place.key, .parent_tag = place.tag });
                return self.replaceAtSpan(span, out.items);
            }
            try self.replaceAtSpan(span, replacement);
        }

        /// `replaceKeyAtPath` for a caller that has the new key's NAME rather
        /// than its syntax, as `insertNamedKey` is to `insertKey`: the name is
        /// spelled by `formatInsertKey` and the rename goes on as before. A
        /// ZON key's span is the field name after its `.`, so the dot that
        /// spelling leads with is left where it already stands.
        ///
        /// A name another entry of the mapping already holds is refused with
        /// `DuplicateKey` before anything is spliced, as `insertNamedKey`
        /// refuses one — a TOML or fig reparse would otherwise report the
        /// repeat as a key that does not parse. Renaming a key to its own
        /// name is not a repeat.
        pub fn replaceNamedKey(self: *Self, path: []const AST.PathSegment, name: []const u8) !void {
            if (path.len == 0 or path[path.len - 1] == .index) return error.NotAKey;
            const parsed = try self.getParsed();
            if (parsed.ast.getValByPath(path[0 .. path.len - 1])) |parent| {
                if (parsed.ast.getNodeByPath(path)) |own| {
                    if (holdsName(parsed.ast, parent, name, own.id)) return error.DuplicateKey;
                } else |_| {} // `replaceKeyAtPath` reports a missing key in its own terms
            } else |_| {}
            const rendered = try self.formatInsertKey(name);
            defer self.allocator.free(rendered);
            // A key's span leaves out the sigil it carries (`key_sigil`,
            // ZON's `.`), which stays where it already stands.
            const sigil = self.syntax().key_sigil;
            const text = if (sigil != null and rendered.len > 0 and rendered[0] == sigil.?) rendered[1..] else rendered;
            return self.replaceKeyAtPath(path, text);
        }

        // ========
        // COMMENTS
        // ========
        //
        // Comments are trivia — they live OUTSIDE every AST node span — so these
        // ops reuse the same splice + reparse machinery as the structural edits:
        // compute a byte position from a node's span, splice the comment text,
        // reparse. The reparse is the safety net (`replaceAtSpan` rolls back if
        // the result no longer parses).
        //
        // **Hooks.** Each of the six ops below dispatches to a `Language`
        // declaration of the same name when one exists, passing exactly its own
        // arguments (`self, path` — plus `text` for the two setters) and handing
        // over the operation entirely; the marker lookup and the whole generic
        // body are skipped. plist declares all six, because a plist comment is a
        // `<!-- ... -->` PAIR rather than a line carrying a marker, so none of
        // the marker-scanning below has anything to scan for.

        /// The line-comment marker for the dialect this document is being read
        /// as, or null when that dialect forbids comments (strict JSON) — in
        /// which case the comment ops return `CommentsUnsupported`. Indexed by
        /// `self.format` because the splice is reparsed under that same
        /// dialect. See `Comments.line`.
        fn lineCommentMarker(self: *const Self) ?lang.CommentDelimiter {
            return self.syntax().comments.line;
        }

        /// The own-line comment marker as a bare PREFIX, or null for a format
        /// with no line comment or a paired one. The dangling and comment-out
        /// ops prefix existing lines with a marker and strip one off; a pair
        /// (`<!-- -->`) cannot be written that way, so they refuse it with
        /// `CommentsUnsupported`, as plist's always did.
        fn prefixCommentMarker(self: *const Self) ?[]const u8 {
            const d = self.lineCommentMarker() orelse return null;
            return if (d.close.len == 0) d.open else null;
        }

        /// The marker for a same-line TRAILING comment specifically, or null
        /// for a format that has no such syntax (INI, NestedText — where a
        /// `;`/`#` after a value is literal value text). Distinct from
        /// `lineCommentMarker`, which those two formats do have. See
        /// `Comments.trailing` for the full reasoning.
        fn trailingCommentMarker(self: *const Self) ?lang.CommentDelimiter {
            return self.syntax().comments.trailing;
        }

        /// The key/value separator the GENERIC entry-insert helpers splice.
        ///
        /// Every caller sits under the generic `insertKey`/`promoteNullToMapping`
        /// dispatch, so a format that declares `kv_sep = null` — fig, TOML,
        /// plist, NestedText — cannot reach one: `language.validate` requires a
        /// null to come with an `insertKey` hook, and the hook replaces this
        /// whole path. The error is the same "unreachable by construction"
        /// answer `setSequence` gives on its own impossible branch, rather than
        /// a fabricated separator that would splice syntax the format's own
        /// parser rejects — fig's `": "` was exactly that.
        fn kvSep(self: *const Self) ![]const u8 {
            return self.syntax().kv_sep orelse error.UnsupportedShape;
        }

        /// Whether a comment op at `path` has no line of its own to work with:
        /// the node is an element or entry of a flow collection that shares its
        /// parent's line (`members = ["a", "b"]`, `members: [a, b]`,
        /// `nested = { k = "v" }`).
        ///
        /// Per § 3.4/§ 6.3 a comment written *inside* a flow collection is
        /// discarded at parse, so such a node can never own a leading block or
        /// a same-line trailing comment. Anchoring on the node's line anyway
        /// made every comment op reach the PARENT's through the item: the read
        /// returned the parent's block, `deleteLeadingComments` removed it, and
        /// `addLeadingComment` inserted above the parent's line (on fig, whose
        /// indent is the raw line prefix, it spliced the prefix `members = [`
        /// back in as well). See
        /// `docs/tasks/closed/flow-item-leading-comment-is-the-parents.md`.
        ///
        /// The test is what SHARES the line, not the node's column: the parent
        /// is flow, and either the parent's opener (`key = [`) or a preceding
        /// element sits on the node's own line. Asking instead whether the node
        /// begins its line would be wrong for every format that decorates the
        /// line before the node's span — ZON's `.` in `.n = 3`, a block
        /// sequence's `- `, fig's `> ` marker run — and an item of a MULTI-LINE
        /// flow collection (one element per line) has to keep working, since
        /// each of those does own its line.
        ///
        /// The document root is excluded: `isFlow`'s first-character test reads
        /// a TOML file that opens with `[table]` as a flow root, and a root
        /// entry has no parent key line for its comment to be confused with
        /// anyway.
        ///
        /// Infallible: an unresolvable path answers "anchored" so the caller's
        /// own `getNodeByPath`/`getValByPath` raises the real `NotFound`.
        fn commentsUnanchored(self: *const Self, parsed: Document, path: []const AST.PathSegment) bool {
            if (path.len == 0) return false;
            const source = self.source.items;
            const parent = parsed.ast.getValByPath(path[0 .. path.len - 1]) catch return false;
            if (parent.id == parsed.ast.root) return false;
            const parent_span = parsed.span(parent);
            if (!self.isFlowNode(parsed, parent)) return false;
            const node = parsed.ast.getNodeByPath(path) catch return false;
            const line = lineStartBefore(source, parsed.span(node).start);
            // The opener is on this line, so what precedes the node is the
            // parent's own `key = [` — the line, and the block above it, are
            // the parent's.
            if (lineStartBefore(source, parent_span.start) == line) return true;
            // Otherwise the node is unanchored only if a preceding element ends
            // on the same line (`[\n  "a", "b",\n]` — "b" shares "a"'s line).
            var prev_end: ?usize = null;
            var cur = (parsed.ast.child(&parent) catch return false) orelse return false;
            while (cur.id != node.id) {
                prev_end = parsed.span(cur).end;
                cur = parsed.ast.next(&cur) orelse return false;
            }
            const end = prev_end orelse return false;
            return lineStartBefore(source, end -| 1) == line;
        }

        /// The line a leading-comment op anchors on: the node's own line start,
        /// which for a mapping entry is its key's line and for a sequence item
        /// is its marker's line — a nested or empty item's value span can begin
        /// on a LATER line than the `-` that introduces it, and a comment
        /// anchored on the span's line would land inside the item rather than
        /// above it. See `markerStart`.
        fn leadingCommentLineStart(self: *const Self, parsed: Document, node: AST.Node) usize {
            return lineStartBefore(self.source.items, markerStart(parsed, node));
        }

        /// Add an own-line comment ABOVE the node at `path` — the key's line for a
        /// mapping entry, else the node's own line — matched to that line's
        /// indentation. It lands at the BOTTOM of any existing leading comment
        /// block (the comment line nearest the node). `text` may be multi-line;
        /// each line becomes its own comment line. Returns `CommentsUnsupported`
        /// for a dialect without comment syntax (strict JSON), and
        /// `CommentsUnanchored` for a node that shares its parent's line inside
        /// a flow collection (see `commentsUnanchored`) — there is no line to
        /// put a comment on that the node would own.
        pub fn addLeadingComment(self: *Self, path: []const AST.PathSegment, text: []const u8) !void {
            const marker = self.lineCommentMarker() orelse return error.CommentsUnsupported;
            if (marker.forbidden) |f| if (std.mem.indexOf(u8, text, f) != null) return error.InvalidComment;
            const parsed = try self.getParsed();
            if (self.commentsUnanchored(parsed, path)) return error.CommentsUnanchored;
            const node = try parsed.ast.getNodeByPath(path);
            const span = parsed.span(node);
            const source = self.source.items;
            const line_start = self.leadingCommentLineStart(parsed, node);
            // A format whose line prefix is STRUCTURAL (fig's `>` marker run)
            // copies the raw prefix; everywhere else it is pure whitespace.
            // See `Syntax.structural_indent`.
            const indent = if (self.syntax().structural_indent)
                source[line_start..span.start]
            else
                source[line_start..firstNonSpace(source, line_start)];

            var buf: std.ArrayList(u8) = .empty;
            defer buf.deinit(self.allocator);
            try renderLineComments(self.allocator, &buf, indent, marker, text);
            try self.replaceAtSpan(Span.init(line_start, line_start), buf.items);
        }

        /// The byte window `[start, line_end)` on the entry-at-`path`'s line where a
        /// same-line trailing comment lives, shared by the set/delete/get trailing
        /// ops. For a scalar or flow value the window runs from just past the value
        /// to that line's newline. For a BLOCK-style mapping/sequence value — whose
        /// node span begins at its first child on a later line — the trailing
        /// comment instead rides the key's line (e.g. `contents: # note` above a
        /// block sequence), so the window is the key line, starting just past the
        /// key. A scalar that ends with its last line's newline — a YAML block
        /// scalar — likewise takes it on its first line, the header's, since
        /// the line its span ends on belongs to whatever follows; so does a
        /// scalar over several lines in a format whose opener line is its
        /// comment's (`Syntax.opener_line_comment`: figl's `x = ''' # note`),
        /// and a flow list or object over several lines whose opener line
        /// holds no element (`xs = [ # note`); the comment after its close
        /// is not its own. `start` always sits before any comment marker and
        /// after the value (scalar) or key (block), or at a multi-line
        /// value's opener, which holds no marker of its own, so a `#`/`//`
        /// inside the value can't false-match.
        ///
        /// `null` when the node has no such window at all: a flow element or
        /// entry sharing its parent's line, whose line-end comment is the
        /// PARENT's (see `commentsUnanchored`). Each caller turns that into its
        /// own "as if there were none" answer.
        fn trailingCommentWindow(self: *Self, path: []const AST.PathSegment) !?struct { start: usize, line_end: usize } {
            const parsed = try self.getParsed();
            if (self.commentsUnanchored(parsed, path)) return null;
            const val = try parsed.ast.getValByPath(path);
            const val_span = parsed.span(val);
            const source = self.source.items;
            // A block collection with no closing token of its own (see
            // `Syntax.closed_containers`) has its trailing comment on the
            // key's line; one that closes itself (`</dict>`) takes it after
            // the close, like any scalar.
            const is_block_collection = switch (std.meta.activeTag(val.kind)) {
                .mapping, .sequence => !self.isFlowNode(parsed, val) and self.syntax().closed_containers == null,
                else => false,
            };
            // A value whose span takes its last line's newline with it (a
            // YAML `|`/`>` block scalar, whose body runs through its final
            // line break) leaves nothing of that line: the line its span ends
            // on is the next entry's. Its comment rides the line it opens
            // on, after the header (`a: | # note`).
            const takes_line_end = val_span.end > val_span.start and source[val_span.end - 1] == '\n';
            // A format may put a multi-line value's comment on its opener
            // line instead of after its close: figl's `x = ''' # note`, whose
            // opener line holds only the delimiter and the comment, and
            // `xs = [ # note` for a flow list or object whose opener line
            // holds no element.
            const is_scalar = switch (std.meta.activeTag(val.kind)) {
                .mapping, .sequence => false,
                else => true,
            };
            const spans_lines = std.mem.indexOfScalar(u8, source[val_span.start..val_span.end], '\n') != null;
            const on_opener_line = self.syntax().opener_line_comment and spans_lines and
                (is_scalar or (!is_block_collection and self.flowOpenerLineIsEmpty(val_span.start)));
            const start = if (is_block_collection)
                parsed.span(try parsed.ast.getKeyByPath(path)).end
            else if (takes_line_end or on_opener_line)
                val_span.start
            else
                val_span.end;
            var line_end = std.mem.indexOfScalarPos(u8, source, start, '\n') orelse source.len;
            // The line's content ends before a CRLF's CR, which stays with the
            // LF: a comment appended after it would leave a bare CR in the
            // value (YAML reads `a: 1\r # x` as the string "1\r").
            if (line_end > start and source[line_end - 1] == '\r') line_end -= 1;
            return .{ .start = start, .line_end = line_end };
        }

        /// Whether the flow container opening at `open` (its `[`/`{`) has
        /// nothing after the opener on its line but blanks and a comment —
        /// figl's `xs = [ # note`, whose first element starts on the next
        /// line. Only then is the opener line the comment's (as the parser
        /// reads it); one that holds an element there keeps its comment
        /// after the close, since a `#` on that line is flow-interior.
        fn flowOpenerLineIsEmpty(self: *const Self, open: usize) bool {
            const source = self.source.items;
            var i = open + 1;
            while (i < source.len and (source[i] == ' ' or source[i] == '\t')) i += 1;
            if (i >= source.len or source[i] == '\n' or source[i] == '\r') return true;
            const marker = self.trailingCommentMarker() orelse return false;
            return std.mem.startsWith(u8, source[i..], marker.open);
        }

        /// Set the same-line trailing comment on the value at `path`: replace an
        /// existing trailing comment on that line, or append one if there is none.
        /// `text` must be a single line. Returns `CommentsUnsupported` for a
        /// dialect without comment syntax (strict JSON), `MultilineComment` if
        /// `text` contains a newline, `CommentsUnanchored` for a node that
        /// shares its parent's line inside a flow collection (see
        /// `commentsUnanchored`) — the end of that line is the parent's — and
        /// `CommentInsideValue` for a value that runs to the end of its line,
        /// where the comment would be read back as value text (see
        /// `replaceKeepingValues`).
        pub fn setTrailingComment(self: *Self, path: []const AST.PathSegment, text: []const u8) !void {
            const marker = self.trailingCommentMarker() orelse return error.CommentsUnsupported;
            if (std.mem.indexOfScalar(u8, text, '\n') != null) return error.MultilineComment;
            if (marker.forbidden) |f| if (std.mem.indexOf(u8, text, f) != null) return error.InvalidComment;
            const win = try self.trailingCommentWindow(path) orelse return error.CommentsUnanchored;
            const source = self.source.items;

            // If a comment marker already follows on this line, splice from it
            // (replace); otherwise splice from the line's end (append).
            var cut = if (std.mem.indexOf(u8, source[win.start..win.line_end], marker.open)) |rel|
                win.start + rel
            else
                win.line_end;
            // Drop the run of spaces/tabs just before the splice so the rebuilt
            // " <marker> text" controls its own single leading space.
            while (cut > win.start and (source[cut - 1] == ' ' or source[cut - 1] == '\t')) cut -= 1;

            var buf: std.ArrayList(u8) = .empty;
            defer buf.deinit(self.allocator);
            try buf.appendSlice(self.allocator, " ");
            try renderComment(self.allocator, &buf, marker, text);
            try self.replaceKeepingValues(Span.init(cut, win.line_end), buf.items);
        }

        /// The trailing-comment splice, held to what a comment edit promises:
        /// the document's values are the same after it as before. The window
        /// ends at the line's end, and a value with no end delimiter of its
        /// own runs to exactly there — a ZON `\\` line is string text up to
        /// its newline — so a comment appended to that line parses, as part of
        /// the value. Nothing in the splice can see that; the two parses can.
        /// A splice that changed any value is rolled back byte-for-byte with
        /// `CommentInsideValue`. Comments are outside the AST and so outside
        /// the check, which is right: changing one is what the op does.
        fn replaceKeepingValues(self: *Self, span: Span, replacement: []const u8) !void {
            const backup = try self.allocator.dupe(u8, self.source.items);
            defer self.allocator.free(backup);
            // A splice that doesn't parse rolls itself back (`replaceAtSpan`).
            try self.replaceAtSpan(span, replacement);

            var parser: Language.Parser = .{ .allocator = self.allocator };
            const before = Language.parse(&parser, backup, self.format) catch {
                // `backup` parsed before the splice, as in `uncommentBlock`.
                try self.restoreSource(backup);
                return error.CommentInsideValue;
            };
            defer before.deinit(self.allocator);
            const after = try self.getParsed();
            if (!nodesEqualDeep(before, before.ast.root, after, after.ast.root)) {
                try self.restoreSource(backup);
                return error.CommentInsideValue;
            }
        }

        /// Remove the run of own-line comments immediately ABOVE the node at
        /// `path` — its owned leading block (contiguous comment lines with no
        /// blank line between, the same block `deleteKey` carries). A no-op when
        /// the node has none — including a node that shares its parent's line
        /// inside a flow collection, whose block above is the parent's (see
        /// `commentsUnanchored`). Returns `CommentsUnsupported` for a dialect
        /// without comment syntax (strict JSON).
        pub fn deleteLeadingComments(self: *Self, path: []const AST.PathSegment) !void {
            _ = self.lineCommentMarker() orelse return error.CommentsUnsupported;
            const parsed = try self.getParsed();
            // Not the node's block to remove (the path resolved: an
            // unresolvable one answers "anchored" and falls through to the
            // `getNodeByPath` below, which raises `NotFound`).
            if (self.commentsUnanchored(parsed, path)) return;
            const node = try parsed.ast.getNodeByPath(path);
            const source = self.source.items;
            const line_start = self.leadingCommentLineStart(parsed, node);
            const block_start = commentBlockStart(source, line_start, self.syntax().comments);
            if (block_start == line_start) return; // nothing above to remove
            try self.replaceAtSpan(Span.init(block_start, line_start), "");
        }

        /// Remove the same-line trailing comment on the value at `path`, if any.
        /// A no-op when there is none — including a node that shares its
        /// parent's line inside a flow collection, whose line-end comment is the
        /// parent's (see `commentsUnanchored`). Returns `CommentsUnsupported`
        /// for a dialect without comment syntax (strict JSON).
        pub fn deleteTrailingComment(self: *Self, path: []const AST.PathSegment) !void {
            const marker = self.trailingCommentMarker() orelse return error.CommentsUnsupported;
            const win = try self.trailingCommentWindow(path) orelse return; // not this node's line-end
            const source = self.source.items;
            const rel = std.mem.indexOf(u8, source[win.start..win.line_end], marker.open) orelse return; // none
            var cut = win.start + rel;
            // Take the whitespace separating the value from the comment with it.
            while (cut > win.start and (source[cut - 1] == ' ' or source[cut - 1] == '\t')) cut -= 1;
            try self.replaceAtSpan(Span.init(cut, win.line_end), "");
        }

        /// Read back the own-line comment block immediately ABOVE the node at
        /// `path` — the same owned block `deleteLeadingComments` removes — with each
        /// line's indentation and `marker` (and one following space) stripped, lines
        /// rejoined by '\n'. Returns `null` when there is no block above the node
        /// (distinct from a present-but-empty comment — a bare `#` — which yields
        /// ""), which includes a node that shares its parent's line inside a flow
        /// collection: the block above that line is the parent's, not the node's
        /// (see `commentsUnanchored`). The caller owns the returned bytes. Returns
        /// `CommentsUnsupported` for a dialect without comment syntax (strict JSON).
        pub fn getLeadingComment(self: *Self, path: []const AST.PathSegment) !?[]u8 {
            const marker = self.lineCommentMarker() orelse return error.CommentsUnsupported;
            const parsed = try self.getParsed();
            if (self.commentsUnanchored(parsed, path)) return null;
            const node = try parsed.ast.getNodeByPath(path);
            const source = self.source.items;
            const line_start = self.leadingCommentLineStart(parsed, node);
            const block_start = commentBlockStart(source, line_start, self.syntax().comments);
            if (block_start == line_start) return null; // no block above

            var out: std.ArrayList(u8) = .empty;
            errdefer out.deinit(self.allocator);
            var it = std.mem.splitScalar(u8, source[block_start..line_start], '\n');
            var first = true;
            while (it.next()) |raw| {
                const line = std.mem.trimEnd(u8, raw, "\r");
                const trimmed = std.mem.trimStart(u8, line, " \t");
                if (trimmed.len == 0) continue; // skip a trailing empty split slice
                if (!first) try out.append(self.allocator, '\n');
                first = false;
                try out.appendSlice(self.allocator, stripLineCommentMarker(trimmed, marker));
            }
            return try out.toOwnedSlice(self.allocator);
        }

        /// Read back the same-line trailing comment on the value at `path` — the
        /// one `setTrailingComment` sets and `deleteTrailingComment` removes — with
        /// its `marker` (and one following space) stripped. Returns `null` when
        /// there is no trailing comment (distinct from a present-but-empty bare `#`,
        /// which yields ""), which includes a node that shares its parent's line
        /// inside a flow collection: the comment at that line's end is the parent's
        /// (see `commentsUnanchored`). The caller owns the returned bytes. Returns
        /// `CommentsUnsupported` for a dialect without comment syntax (strict JSON).
        pub fn getTrailingComment(self: *Self, path: []const AST.PathSegment) !?[]u8 {
            const marker = self.trailingCommentMarker() orelse return error.CommentsUnsupported;
            const win = try self.trailingCommentWindow(path) orelse return null;
            const source = self.source.items;
            const rel = std.mem.indexOf(u8, source[win.start..win.line_end], marker.open) orelse
                return null; // none
            const after = std.mem.trimEnd(u8, source[win.start + rel .. win.line_end], " \t\r");
            return try self.allocator.dupe(u8, stripLineCommentMarker(after, marker));
        }

        // ── The dangling anchor ─────────────────────────────────────────────
        //
        // A container's THIRD comment anchor (spec § 3.4): the run of own-line
        // comments at the END of its body, after its last entry, which the AST
        // side-table carries as `NodeComments.dangling` and every printer
        // emits. The leading trio addresses a comment by the node it sits
        // above; nothing addressed this one, because it sits above nothing —
        // and a commented-out LAST entry is exactly a dangling run.
        //
        // Where the run is, in source: from just past the last line of the
        // container's body, forward over each contiguous own-line comment
        // whose line carries at least the body's child indent. That is spec
        // § 3.4's own rule — "a comment at or deeper than the closing
        // container's child depth becomes that container's dangling run; a
        // shallower one stays pending as a leading comment on the next
        // sibling" — read off the source rather than the parse, which is what
        // makes it the exact inverse of what `addDanglingComment` writes.
        //
        // One ambiguity is inherent and shared with the leading trio: a
        // comment line at child depth with an entry directly below it is both
        // this container's dangling run and that entry's leading block, and
        // both ops report it. `commentBlockStart` has always worked that way;
        // resolving it would need the parse's attribution, and the parse
        // discards where a comment came from.

        /// Where a container's dangling run lives: the offset it starts at
        /// (just past the container body's last line) and the line prefix its
        /// lines carry (the body's child depth).
        const DanglingAnchor = struct { at: usize, indent: []const u8 };

        /// Resolve the dangling anchor of the container at `path` (the root for
        /// an empty path). `UnsupportedShape` for a scalar — a dangling run is
        /// a container's — and for a flow container with no room for an
        /// own-line comment: one written on a single line (`{ "a": 1 }`, where
        /// the run would have to break it) or with no children at all.
        ///
        /// A flow container that DOES span lines is fine and is the shape this
        /// matters for: a pretty-printed JSONC object's `// note` before the
        /// closing brace is the root's dangling run, and nothing else can
        /// address it. (fig discards its own flow collections' interior
        /// comments at parse, spec § 6.3 — the bytes still land, but the tree
        /// will not carry them. The comment-out pair refuses flow outright,
        /// where a marker would swallow a separator.)
        ///
        /// The body ends after the last child written on the container's OWN
        /// lines. A child that is itself a SECTION is skipped: its lines are
        /// assembled from elsewhere in the file, and the run belongs before the
        /// sub-tables, not after them — the same place `toml/printer.zig` emits
        /// it. A block container with no such child (an empty `[table]`)
        /// anchors just past its header line(s).
        fn danglingAnchor(self: *const Self, parsed: Document, path: []const AST.PathSegment) !DanglingAnchor {
            const node = try parsed.ast.getValByPath(path);
            switch (node.kind) {
                .mapping, .sequence => {},
                else => return error.UnsupportedShape,
            }
            const source = self.source.items;
            const span = parsed.span(node);
            const flow = self.isFlowNode(parsed, node);
            const structural = self.syntax().structural_indent;

            var last: ?AST.Node = null;
            var maybe = try parsed.ast.child(&node);
            while (maybe) |c| {
                const val = if (c.kind == .keyvalue) parsed.ast.node(c.kind.keyvalue.value) else c;
                if (!parsed.isSection(val)) last = c;
                maybe = parsed.ast.next(&c);
            }
            if (last) |c| {
                const c_span = parsed.span(c);
                const line_start = lineStartBefore(source, c_span.start);
                const at = lineEndAfter(source, c_span.end -| 1);
                // A flow container whose closing delimiter shares the last
                // child's line has no line for a run to sit on, and `at` is
                // already past the container.
                if (flow and at >= span.end) return error.UnsupportedShape;
                return .{
                    .at = at,
                    .indent = source[line_start..commentColumn(source, line_start, structural)],
                };
            }
            // Nothing on its own lines. A flow container has no body line to
            // anchor on at all; a section container still has its
            // header line(s) recorded; anything else falls back to its own
            // line, which for a childless block container is where its body
            // would begin. The indent is that line's, which is a level short
            // for a format whose depth is structural (fig): an empty
            // container is the one shape where the child depth cannot be read
            // off a child, and a format that needs better hooks these ops.
            if (flow) return error.UnsupportedShape; // `{}` — no body to end
            const regs = parsed.regionsOf(node.id);
            const anchor_at = if (regs.len > 0) regs[regs.len - 1].end else lineEndAfter(source, span.end -| 1);
            const own_line = lineStartBefore(source, if (regs.len > 0) regs[regs.len - 1].start else span.start);
            return .{
                .at = anchor_at,
                .indent = source[own_line..commentColumn(source, own_line, structural)],
            };
        }

        /// Byte offset just past the dangling run starting at `anchor.at`: the
        /// contiguous own-line `marker` comments carrying at least
        /// `anchor.indent`. Stops at a blank line, at content, at a dedent, and
        /// at end of input. Equal to `anchor.at` when there is no run.
        fn danglingRunEnd(self: *const Self, anchor: DanglingAnchor, marker: []const u8) usize {
            const source = self.source.items;
            const structural = self.syntax().structural_indent;
            var pos = anchor.at;
            while (pos < source.len) {
                const line_end = lineEndAfter(source, pos);
                if (!std.mem.startsWith(u8, source[pos..line_end], anchor.indent)) break;
                const col = commentColumn(source, pos, structural);
                if (!std.mem.startsWith(u8, source[col..line_end], marker)) break;
                pos = line_end;
            }
            return pos;
        }

        /// Read back the dangling comment run at the end of the container at
        /// `path`'s body (the root for an empty path) — the third anchor beside
        /// `getLeadingComment`/`getTrailingComment` — with each line's prefix
        /// and `marker` (and one following space) stripped, lines rejoined by
        /// '\n'. `null` when there is no run (distinct from a bare marker,
        /// which yields ""). The caller owns the returned bytes.
        /// `CommentsUnsupported` for a dialect without comment syntax (strict
        /// JSON); `UnsupportedShape` for a scalar, or a flow container with no
        /// line for a run to sit on (see `danglingAnchor`).
        ///
        /// A format whose comment syntax is a paired delimiter rather than a
        /// bare prefix (plist's `<!-- -->`) answers `CommentsUnsupported`: the
        /// dangling and comment-out ops write a marker onto existing lines
        /// and strip one off, which a pair cannot be.
        pub fn getDanglingComment(self: *Self, path: []const AST.PathSegment) !?[]u8 {
            const marker = self.prefixCommentMarker() orelse return error.CommentsUnsupported;
            const parsed = try self.getParsed();
            const anchor = try self.danglingAnchor(parsed, path);
            const end = self.danglingRunEnd(anchor, marker);
            if (end == anchor.at) return null;

            const source = self.source.items;
            const structural = self.syntax().structural_indent;
            var out: std.ArrayList(u8) = .empty;
            errdefer out.deinit(self.allocator);
            var pos = anchor.at;
            var first = true;
            while (pos < end) {
                const line_end = lineEndAfter(source, pos);
                const body = std.mem.trimEnd(u8, source[commentColumn(source, pos, structural)..line_end], "\r\n");
                if (!first) try out.append(self.allocator, '\n');
                first = false;
                try out.appendSlice(self.allocator, stripLineCommentMarker(body, .{ .open = marker }));
                pos = line_end;
            }
            return try out.toOwnedSlice(self.allocator);
        }

        /// Add own-line comment line(s) at the END of the container at `path`'s
        /// body, at the body's child depth — the dangling twin of
        /// `addLeadingComment`, landing at the bottom of any run already there.
        /// `text` may be multi-line; each line becomes its own comment line.
        /// `CommentsUnsupported` for a dialect without comment syntax (strict
        /// JSON); `UnsupportedShape` for a scalar, or a flow container with no
        /// line for a run to sit on (see `danglingAnchor`).
        pub fn addDanglingComment(self: *Self, path: []const AST.PathSegment, text: []const u8) !void {
            const marker = self.prefixCommentMarker() orelse return error.CommentsUnsupported;
            const parsed = try self.getParsed();
            const anchor = try self.danglingAnchor(parsed, path);
            const at = self.danglingRunEnd(anchor, marker);

            var buf: std.ArrayList(u8) = .empty;
            defer buf.deinit(self.allocator);
            // A final line with no newline of its own would otherwise weld the
            // first comment onto it.
            if (at > 0 and self.source.items[at - 1] != '\n') try buf.append(self.allocator, '\n');
            try renderLineComments(self.allocator, &buf, anchor.indent, .{ .open = marker }, text);
            try self.replaceAtSpan(Span.init(at, at), buf.items);
        }

        /// Remove the whole dangling run at the end of the container at
        /// `path`'s body. A no-op when there is none. `CommentsUnsupported` for
        /// a dialect without comment syntax (strict JSON); `UnsupportedShape`
        /// for a scalar, or a flow container with no line for a run to sit on.
        pub fn deleteDanglingComments(self: *Self, path: []const AST.PathSegment) !void {
            const marker = self.prefixCommentMarker() orelse return error.CommentsUnsupported;
            const parsed = try self.getParsed();
            const anchor = try self.danglingAnchor(parsed, path);
            const end = self.danglingRunEnd(anchor, marker);
            if (end == anchor.at) return;
            try self.replaceAtSpan(Span.init(anchor.at, end), "");
        }

        // ── Comment out, and back ───────────────────────────────────────────
        //
        // A commented-out entry — `# port = 8080` under a `[server]` — is what
        // a structural editor shows as a DISABLED row with a toggle. Both
        // directions are one splice over the node's own span: re-serializing
        // the value and calling `addLeadingComment` would lose the entry's
        // spelling (its quoting, its layout, the comments inside it), and
        // stripping the markers and calling `insertValue` would lose the same
        // going the other way.

        /// Whether the node at `span`, whose own line begins at `line_start`,
        /// has those lines TO ITSELF — the precondition both halves of the
        /// comment-out pair rest on, since a line marker hides everything after
        /// it to end of line.
        ///
        /// Before the node, only indentation and the format's own line
        /// introducers may stand (a `- `/`* ` item dash, a fig `>` marker run,
        /// a `+` continuation). After it, only whitespace, one separating
        /// comma, and a trailing comment. Anything else means a SIBLING shares
        /// the line — the entries of `{ "a": 1, "b": 2 }`, the items of
        /// `[a, b]` — or that a closing delimiter does, and commenting the line
        /// out would take that with it.
        ///
        /// Stated as line ownership rather than as "is the parent a flow
        /// container", because those are not the same question: a
        /// pretty-printed JSONC object is flow-spelled and one entry per line,
        /// and commenting one of its members out is exactly what a JSONC editor
        /// wants; a one-line YAML flow sequence is the shape that cannot.
        fn ownsItsLines(self: *const Self, line_start: usize, span: Span, marker: []const u8) bool {
            const source = self.source.items;
            const syn = self.syntax();
            for (source[line_start..span.start]) |c| {
                if (c == ' ' or c == '\t') continue;
                if (std.mem.indexOfScalar(u8, syn.seq_item_marker, c) != null) continue;
                if (std.mem.indexOfScalar(u8, syn.line_prefix_marks, c) != null) continue;
                return false;
            }
            const line_end = lineEndAfter(source, span.end -| 1);
            const sep = self.syntax().flow_entry_sep;
            var i = span.end;
            var comma = false;
            while (i < line_end) : (i += 1) {
                switch (source[i]) {
                    ' ', '\t', '\r', '\n' => {},
                    else => {
                        if (std.mem.startsWith(u8, source[i..line_end], sep)) {
                            if (comma) return false;
                            comma = true;
                            i += sep.len - 1;
                            continue;
                        }
                        return std.mem.startsWith(u8, source[i..line_end], marker);
                    },
                }
            }
            return true;
        }

        /// Turn the node at `path` into a comment run: every line of its source
        /// span — the key through the end of its value for a mapping entry, the
        /// item for a sequence item — gains the line marker at that line's own
        /// indentation (past a structural prefix, so a nested fig line keeps
        /// its depth). The node's own leading comment block stays above it,
        /// untouched, so a `# why` above a commented-out entry survives as a
        /// note on the note.
        ///
        /// Afterwards the tree no longer has the node: the run is the leading
        /// block of what followed it, or — when it was last — the parent's
        /// dangling run, which `uncommentLeading`/`uncommentDangling` address
        /// to bring it back.
        ///
        /// `CommentsUnsupported` for a dialect without comment syntax (strict
        /// JSON). `UnsupportedShape` for the root (an empty path);
        /// `CommentsUnanchored` — the answer the six comment ops give the same
        /// node — for one that does not have its lines to itself: an item of a
        /// one-line flow collection (`tags = [a, b]`), where the marker would
        /// swallow a separator and where fig discards an interior comment at
        /// parse anyway (spec § 6.3). See `ownsItsLines`. A node whose value is a
        /// SECTION is refused with the format's own vocabulary
        /// (`CannotDeleteSection` / `CannotDeleteSection` /
        /// `CannotDeleteSection`) for the reason `deleteKey` refuses it: its
        /// span is a name inside a header, and its body is lines this op cannot
        /// see.
        pub fn commentOut(self: *Self, path: []const AST.PathSegment) !void {
            const marker = self.prefixCommentMarker() orelse return error.CommentsUnsupported;
            if (path.len == 0) return error.UnsupportedShape;
            const parsed = try self.getParsed();
            const node = try parsed.ast.getNodeByPath(path);
            const source = self.source.items;
            const val = if (node.kind == .keyvalue) parsed.ast.node(node.kind.keyvalue.value) else node;
            if (parsed.isSection(val)) return self.refuse(.delete);

            const span = parsed.span(node);
            const start = self.leadingCommentLineStart(parsed, node);
            const end = lineEndAfter(source, span.end -| 1);
            if (!self.ownsItsLines(start, span, marker)) return error.CommentsUnanchored;
            const structural = self.syntax().structural_indent;

            var buf: std.ArrayList(u8) = .empty;
            defer buf.deinit(self.allocator);
            var pos = start;
            while (pos < end) {
                const line_end = lineEndAfter(source, pos);
                const col = commentColumn(source, pos, structural);
                try buf.appendSlice(self.allocator, source[pos..col]);
                try buf.appendSlice(self.allocator, marker);
                // No trailing space on an otherwise empty line — the same rule
                // `renderLineComments` follows, and what keeps the round trip
                // byte-exact through a blank line inside a block scalar.
                const rest = source[col..line_end];
                if (rest.len > 0 and rest[0] != '\n' and rest[0] != '\r') try buf.append(self.allocator, ' ');
                try buf.appendSlice(self.allocator, rest);
                pos = line_end;
            }
            try self.replaceAtSpan(Span.init(start, end), buf.items);
        }

        /// Bring `line_count` lines of the LEADING comment block above the node
        /// at `path`, starting at `first_line` (0-based within that block),
        /// back as entries: strip the marker and one following space from each,
        /// then reparse.
        ///
        /// Lines are addressed by index rather than matched by content because
        /// WHICH lines look like an entry is the caller's judgement — it parses
        /// the block as a fragment and decides. The editor's part is the byte
        /// edit and the guarantee that it landed: if the result does not parse,
        /// or parses to a document whose other nodes changed, the splice is
        /// rolled back and the call fails (the parse error, or
        /// `CommentNotAnEntry`) with the document byte-for-byte as it was.
        ///
        /// The block is exactly the one `getLeadingComment` reports — the same
        /// `commentBlockStart` scan — so a caller can read the block, decide
        /// which of its lines are an entry, and name them by the index it read
        /// them at. (That scan does not climb past a structural prefix, so a
        /// fig comment line carrying a `>` marker run is not part of any node's
        /// leading block for either op. A commented-out nested fig entry is the
        /// container's dangling run when it was last, which
        /// `uncommentDangling` does address.)
        ///
        /// `NotFound` when the block has fewer than `first_line + line_count`
        /// lines; a `line_count` of 0 is a no-op. `CommentsUnsupported` for a
        /// dialect without comment syntax (strict JSON); `UnsupportedShape` for
        /// the root, and `CommentsUnanchored` for a node that does not own its
        /// lines (`ownsItsLines`), as the six comment ops answer it.
        pub fn uncommentLeading(self: *Self, path: []const AST.PathSegment, first_line: usize, line_count: usize) !void {
            const marker = self.prefixCommentMarker() orelse return error.CommentsUnsupported;
            if (path.len == 0) return error.UnsupportedShape;
            const parsed = try self.getParsed();
            const node = try parsed.ast.getNodeByPath(path);
            const source = self.source.items;
            const parent_path = path[0 .. path.len - 1];
            const span = parsed.span(node);
            const line_start = self.leadingCommentLineStart(parsed, node);
            if (!self.ownsItsLines(line_start, span, marker)) return error.CommentsUnanchored;
            const block_start = commentBlockStart(source, line_start, self.syntax().comments);
            return self.uncommentBlock(block_start, line_start, first_line, line_count, parent_path);
        }

        /// The dangling twin of `uncommentLeading`: bring `line_count` lines of
        /// the DANGLING run at the end of the container at `container_path`'s
        /// body, starting at `first_line`, back as entries of that container.
        /// The op that re-enables a commented-out LAST entry, which has no
        /// following sibling to be the leading block of.
        ///
        /// Same guarantee, same errors — see `uncommentLeading`.
        pub fn uncommentDangling(self: *Self, container_path: []const AST.PathSegment, first_line: usize, line_count: usize) !void {
            const marker = self.prefixCommentMarker() orelse return error.CommentsUnsupported;
            const parsed = try self.getParsed();
            const anchor = try self.danglingAnchor(parsed, container_path);
            const end = self.danglingRunEnd(anchor, marker);
            return self.uncommentBlock(anchor.at, end, first_line, line_count, container_path);
        }

        /// The shared body of the two uncomment ops: strip the marker (and one
        /// following space) from lines `[first_line, first_line + line_count)`
        /// of the comment block `[block_start, block_end)`, splice the whole
        /// block back in one edit, and keep the result only if it parses AND
        /// adds nodes to nothing but the container at `container_path`.
        ///
        /// The marker is looked for where `commentOut` writes it
        /// (`commentColumn`), so a line that is not a comment at all — or one
        /// whose marker sits somewhere else entirely — is `CommentNotAnEntry`
        /// before anything is spliced.
        fn uncommentBlock(
            self: *Self,
            block_start: usize,
            block_end: usize,
            first_line: usize,
            line_count: usize,
            container_path: []const AST.PathSegment,
        ) !void {
            if (line_count == 0) return;
            const marker = self.prefixCommentMarker() orelse return error.CommentsUnsupported;
            const structural = self.syntax().structural_indent;
            const source = self.source.items;

            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            var pos = block_start;
            var index: usize = 0;
            var stripped: usize = 0;
            while (pos < block_end) {
                const line_end = @min(lineEndAfter(source, pos), block_end);
                if (index >= first_line and index < first_line + line_count) {
                    const col = commentColumn(source, pos, structural);
                    if (!std.mem.startsWith(u8, source[col..line_end], marker)) return error.CommentNotAnEntry;
                    try out.appendSlice(self.allocator, source[pos..col]);
                    var rest = source[col + marker.len .. line_end];
                    if (rest.len > 0 and rest[0] == ' ') rest = rest[1..];
                    try out.appendSlice(self.allocator, rest);
                    stripped += 1;
                } else {
                    try out.appendSlice(self.allocator, source[pos..line_end]);
                }
                index += 1;
                pos = line_end;
            }
            if (stripped != line_count) return error.NotFound; // fewer lines than asked for

            // Snapshot before the splice: the check below compares the parse of
            // these bytes against the parse of the edited ones, and a rollback
            // has to be byte-exact, not a re-render.
            const backup = try self.allocator.dupe(u8, self.source.items);
            defer self.allocator.free(backup);
            // A splice that doesn't parse rolls itself back (`replaceAtSpan`).
            try self.replaceAtSpan(Span.init(block_start, block_end), out.items);

            var parser: Language.Parser = .{ .allocator = self.allocator };
            const before = Language.parse(&parser, backup, self.format) catch {
                // `backup` parsed at `init` and every edit since kept it
                // parsing, so this cannot happen; take the refusal rather than
                // commit an edit nothing checked.
                try self.restoreSource(backup);
                return error.CommentNotAnEntry;
            };
            defer before.deinit(self.allocator);
            if (!changedOnlyAt(before, try self.getParsed(), container_path, .added)) {
                try self.restoreSource(backup);
                return error.CommentNotAnEntry;
            }
        }

        // ===============
        // INSERT / DELETE
        // ===============
        //
        // These ops never reserialize the document: each computes a byte span +
        // replacement text and reuses `replaceAtSpan` (splice + reparse). Inserts
        // splice at a zero-length span; deletes splice an empty replacement.
        // `value_text`/`key_text` arrive already serialized (single-line scalars,
        // or multi-line block text indented from column 0); the editor only
        // re-frames indentation and newline/comma context for the splice site.

        /// Insert `key_text: value_text` into the mapping at `path` (empty path =
        /// root). Appends after the mapping's last entry for block mappings, or
        /// inside the braces for flow `{}`. If `path` resolves to a `null` value
        /// (a bare `key:`), promotes it to a one-entry nested mapping.
        ///
        /// The flow/block decision is `isFlowNode`'s; a block entry is spelled
        /// through `writeEntry` (a `renderEntry` where the format declares
        /// one), a flow entry with `kv_sep` or the separator its siblings use.
        /// A table with no header of its own (TOML's `a` in `a.b = 1`) has
        /// no lines to append to: its entry is a key path in the lines or
        /// braces that hold it (`insertHeaderless`).
        ///
        /// **Engine rule**: an insert never gives a mapping a second entry of
        /// a name it already holds. Most parsers here accept a repeated key
        /// (YAML, JSON, INI, ZON, dotenv, .properties), so the reparse after
        /// the splice cannot be the net: the mapping's repeated names are
        /// counted before and after, and an insert that adds one is rolled
        /// back with `DuplicateKey` — the error TOML's parser already raised
        /// for the same request. Counting rather than looking the key up is
        /// what lets this work from key SYNTAX, whose name only the reparse
        /// knows; a document that already repeated a name is not refused an
        /// unrelated insert. `insertNamedKey` refuses up front, by name.
        ///
        /// The result is held to what an insert promises (`readsBack`): the
        /// mapping gained one entry, reading as `key_text` and `value_text`
        /// read, and nothing else changed, or `EditReadsBackWrong` and the
        /// source as it was.
        pub fn insertKey(self: *Self, path: []const AST.PathSegment, key_text: []const u8, value_text: []const u8) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            var guard = try self.addGuard(parsed, node, null, key_text, value_text);
            defer guard.deinit(self.allocator);
            try self.spliceEntry(path, key_text, value_text);
            try self.readsBack(&guard);
        }

        fn spliceEntry(self: *Self, path: []const AST.PathSegment, key_text: []const u8, value_text: []const u8) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            const span = parsed.span(node);
            const repeated_before = try self.repeatedKeyCount(parsed.ast, node);
            // Snapshot for the duplicate rollback below; the splice's own
            // rollback covers a reparse failure.
            const backup = try self.allocator.dupe(u8, self.source.items);
            defer self.allocator.free(backup);
            const inserted = switch (node.kind) {
                .mapping => |first| if (self.isFlowNode(parsed, node))
                    self.insertFlowMapEntry(parsed, node, span, first != null, key_text, value_text)
                else if (self.headerlessHolder(parsed, node)) |owner|
                    self.insertHeaderless(parsed, path, node, owner, key_text, value_text)
                else
                    self.insertBlockKey(parsed, node, key_text, value_text),
                .null_ => self.promoteNullToMapping(span, node.id == parsed.ast.root, key_text, value_text, placeOf(parsed, node)),
                else => return error.NotAMapping,
            };
            inserted catch |err| {
                // A parser that refuses a repeated key (TOML's) fails the
                // reparse, and through the contract its reason is only a
                // message. The splice has rolled back; if the key syntax is
                // how this format spells a name the mapping already holds,
                // that is the refusal to report.
                if (err == error.OutOfMemory) return err;
                const now = self.getParsed() catch return err;
                const held = now.ast.getValByPath(path) catch return err;
                if (try self.spellsHeldName(now.ast, held, key_text)) return error.DuplicateKey;
                return err;
            };
            const after = try self.getParsed();
            const node_after = after.ast.getValByPath(path) catch return;
            if (try self.repeatedKeyCount(after.ast, node_after) > repeated_before) {
                try self.restoreSource(backup);
                return error.DuplicateKey;
            }
        }

        /// How many direct entries of `mapping` repeat a string key an
        /// earlier entry already has — 0 for a node that is not a mapping.
        fn repeatedKeyCount(self: *const Self, ast: AST, mapping: AST.Node) !usize {
            const first = switch (mapping.kind) {
                .mapping => |f| f,
                else => return 0,
            };
            var seen: std.StringHashMapUnmanaged(void) = .empty;
            defer seen.deinit(self.allocator);
            var repeated: usize = 0;
            var entry = first;
            while (entry) |id| : (entry = ast.node(id).next_sibling) {
                const kv = switch (ast.node(id).kind) {
                    .keyvalue => |kv| kv,
                    else => continue,
                };
                const name = switch (ast.node(kv.key).kind) {
                    .string => |s| s,
                    else => continue,
                };
                if ((try seen.getOrPut(self.allocator, name)).found_existing) repeated += 1;
            }
            return repeated;
        }

        /// Whether `key_text` is how this format spells the key of a direct
        /// entry of `mapping`.
        fn spellsHeldName(self: *Self, ast: AST, mapping: AST.Node, key_text: []const u8) !bool {
            var entry = switch (mapping.kind) {
                .mapping => |first| first,
                else => return false,
            };
            const want = std.mem.trim(u8, key_text, " \t");
            while (entry) |id| : (entry = ast.node(id).next_sibling) {
                const kv = switch (ast.node(id).kind) {
                    .keyvalue => |kv| kv,
                    else => continue,
                };
                const name = switch (ast.node(kv.key).kind) {
                    .string => |s| s,
                    else => continue,
                };
                if (std.mem.eql(u8, name, want)) return true;
                const spelled = try self.formatInsertKey(name);
                defer self.allocator.free(spelled);
                if (std.mem.eql(u8, spelled, want)) return true;
            }
            return false;
        }

        /// Whether `mapping` has a direct entry whose key is `name`, other
        /// than the entry `except` — false for a node that is not a mapping.
        fn holdsName(ast: AST, mapping: AST.Node, name: []const u8, except: ?AST.Node.Id) bool {
            var entry = switch (mapping.kind) {
                .mapping => |first| first,
                else => return false,
            };
            while (entry) |id| : (entry = ast.node(id).next_sibling) {
                if (except) |e| if (id == e) continue;
                const kv = switch (ast.node(id).kind) {
                    .keyvalue => |kv| kv,
                    else => continue,
                };
                switch (ast.node(kv.key).kind) {
                    .string => |s| if (std.mem.eql(u8, s, name)) return true,
                    else => {},
                }
            }
            return false;
        }

        /// `insertKey` for a caller that has the key's NAME rather than its
        /// syntax: the name is spelled as this format spells a key
        /// (`formatInsertKey` — `.name` in ZON, quoted in strict JSON, quoted
        /// when it must be in TOML) and inserted. What every binding and the
        /// CLI want, since a name is what a user types.
        ///
        /// A name the mapping at `path` already holds is refused with
        /// `DuplicateKey` before anything is spliced — for every format
        /// alike, where `insertKey` would otherwise report whatever the
        /// reparse said (fig's parser rejects the repeat as a parse error,
        /// which reads as if the VALUE were malformed). `set` is the op
        /// that replaces an existing key's value.
        pub fn insertNamedKey(self: *Self, path: []const AST.PathSegment, name: []const u8, value_text: []const u8) !void {
            const parsed = try self.getParsed();
            if (parsed.ast.getValByPath(path)) |parent| {
                if (holdsName(parsed.ast, parent, name, null)) return error.DuplicateKey;
            } else |_| {} // `insertKey` reports a bad parent path in its own terms
            const rendered = try self.formatInsertKey(name);
            defer self.allocator.free(rendered);
            return self.insertKey(path, rendered, value_text);
        }

        /// Delete the mapping entry at `path` (which must name a key). For a
        /// *block* mapping, removes the entry's full line(s) plus any owned
        /// leading comment block (a run of comment lines — `#` for YAML/TOML,
        /// `//` or `/* */` for JSON5/JSONC — with no intervening blank line),
        /// leaving no blank gap. For a *flow* (`{…}`) mapping — any JSON/JSON5/
        /// JSONC object, or a YAML/TOML/fig/ZON inline mapping — routes through a
        /// comma-aware entry splice instead (see the comment at that branch),
        /// since a flow mapping's entries are comma-separated rather than
        /// one-per-line and the block path's line delete cannot handle them
        /// safely at any arity or position.
        ///
        /// Whatever the arm, the result is held to what a delete promises
        /// (`keepOthers`): a document that reads back with that entry gone
        /// and nothing else changed, or `DeleteChangedOtherValues` and the
        /// source as it was.
        pub fn deleteKey(self: *Self, path: []const AST.PathSegment) !void {
            const removed = self.entryIndex(path);
            const backup = try self.allocator.dupe(u8, self.source.items);
            defer self.allocator.free(backup);
            try self.spliceOutEntry(path);
            if (removed) |at| try self.keepOthers(backup, path[0 .. path.len - 1], at);
        }

        /// Where the entry at `path` stands among its container's children,
        /// for `keepOthers` — or null when `path` names no physical entry of
        /// a container the path also reaches, which leaves the check nothing
        /// to compare.
        fn entryIndex(self: *Self, path: []const AST.PathSegment) ?usize {
            if (path.len == 0) return null;
            const parsed = self.getParsed() catch return null;
            const node = parsed.ast.getNodeByPath(path) catch return null;
            const parent = parsed.ast.getValByPath(path[0 .. path.len - 1]) catch return null;
            var i: usize = 0;
            var sib = parsed.ast.child(&parent) catch return null;
            while (sib) |c| : (sib = parsed.ast.next(&c)) {
                if (c.id == node.id) return i;
                i += 1;
            }
            return null;
        }

        /// The delete guard. A delete is a splice chosen by reading the
        /// source around an entry, and a wrong reading still parses: the
        /// line delete that removed `a` from `- - a` over `  - c` took the
        /// parent's marker with it, and `[[a, c]]` read back as `[c]`. So
        /// the two parses are compared, as the uncomment guard compares
        /// them (`changedOnlyAt`): the container at `container_path` must
        /// have lost its child at `removed` and nothing else, every other
        /// node must be as it was, and otherwise the splice is rolled back
        /// byte-for-byte with `DeleteChangedOtherValues`.
        ///
        /// What the entry's going takes with it is the entry's, not another
        /// value: a container whose only child went may read back as the
        /// format reads an empty value (`k:` over `  a: 1` becomes `k:`,
        /// null; ZON's emptied tuple is `.{}`, a struct); an implicit table
        /// that held only the entry goes too (TOML's `a.b.c = 1` is all of
        /// `a.b`); and a duplicate of its key shows through (dotenv's `A=1`
        /// over `A=3`). Scalars compare by their lexemes, so a `NaN` is
        /// equal to itself.
        fn keepOthers(self: *Self, backup: []const u8, container_path: []const AST.PathSegment, removed: usize) !void {
            var parser: Language.Parser = .{ .allocator = self.allocator };
            const before = Language.parse(&parser, backup, self.format) catch {
                // `backup` parsed before the splice, as in `uncommentBlock`.
                try self.restoreSource(backup);
                return error.DeleteChangedOtherValues;
            };
            defer before.deinit(self.allocator);
            const removal: Removal = .{ .at = removed, .edit = .between(backup, self.source.items) };
            if (!changedOnlyAt(before, try self.getParsed(), container_path, .{ .removed = removal })) {
                try self.restoreSource(backup);
                return error.DeleteChangedOtherValues;
            }
        }

        /// What a write, move or reorder is checked against once its splice
        /// is in: the source before it, the container it changes (`route`,
        /// from the root), and the one change it may make there.
        const Guard = struct {
            backup: []u8,
            route: []usize,
            write: Write,

            fn deinit(g: *Guard, allocator: std.mem.Allocator) void {
                allocator.free(g.backup);
                allocator.free(g.route);
                switch (g.write) {
                    .whole => |w| if (w) |r| r.deinit(allocator),
                    .replaced => |r| if (r.want) |w| w.deinit(allocator),
                    .added => |r| if (r.want) |w| w.deinit(allocator),
                    .moved => |order| allocator.free(order),
                    .regrouped => {},
                }
            }
        };

        /// A `Guard` for `write` to `container`, taking ownership of what
        /// `write` holds.
        fn guardAt(self: *Self, parsed: Document, container: AST.Node, write: Write) !Guard {
            var held: Guard = .{ .backup = &.{}, .route = &.{}, .write = write };
            errdefer held.deinit(self.allocator);
            held.route = try routeTo(self.allocator, parsed, container.id);
            held.backup = try self.allocator.dupe(u8, self.source.items);
            return held;
        }

        /// The guard for replacing the value `node` with `replacement`: the
        /// node reads back as the replacement reads (`readingOf`). `root`
        /// says the node is the document, whose replacement is a document.
        fn replaceGuard(self: *Self, parsed: Document, node: AST.Node, root: bool, replacement: []const u8) !Guard {
            const place = placeOfChild(parsed, node.id);
            var buf: std.ArrayList(u8) = .empty;
            defer buf.deinit(self.allocator);
            const in: WriteSlot = if (root) .root else if (parsed.ast.parentOf(node.id)) |p|
                (if (parsed.ast.kindOf(p) == .sequence) self.itemSlot(parsed, parsed.ast.node(p)) else .entry)
            else
                .entry;
            const want = try self.readingOf(null, try self.renderedValue(&buf, replacement, place), place, in);
            const slot = slotOf(parsed, node.id) orelse return self.guardAt(parsed, node, .{ .whole = want });
            return self.guardAt(parsed, parsed.ast.node(slot.container), .{ .replaced = .{ .at = slot.at, .want = want } });
        }

        /// The guard for adding one child to `container`: an item at `at`,
        /// or an entry (`at` null) reading as `key_text` and `value_text`
        /// read.
        fn addGuard(self: *Self, parsed: Document, container: AST.Node, at: ?usize, key_text: ?[]const u8, value_text: []const u8) !Guard {
            const place = placeOf(parsed, container);
            var buf: std.ArrayList(u8) = .empty;
            defer buf.deinit(self.allocator);
            const in: WriteSlot = if (container.kind == .sequence) self.itemSlot(parsed, container) else .entry;
            const want = try self.readingOf(key_text, try self.renderedValue(&buf, value_text, place), place, in);
            return self.guardAt(parsed, container, .{ .added = .{ .at = at, .want = want } });
        }

        /// Where a write puts its value, as `readingOf` reads it: the
        /// document, a mapping entry, an item of a block list (one per line,
        /// after `seq_item_marker`), or an item of any other list.
        const WriteSlot = enum { root, entry, block_item, flow_item };

        /// The `WriteSlot` of an item of the sequence `seq`. A format whose
        /// line prefix is structure (figl) writes an item's later lines as
        /// its own, not under `writeItem`'s indent, so it has no block item
        /// to read one in.
        fn itemSlot(self: *const Self, parsed: Document, seq: AST.Node) WriteSlot {
            if (self.syntax().structural_indent) return .flow_item;
            return if (self.isFlowNode(parsed, seq)) .flow_item else .block_item;
        }

        /// How `rendered` reads in this format, or null when that is not
        /// known. It is read in an otherwise empty document, in the slot the
        /// write put it in — one entry under `key_text` (or `k`, when only
        /// the value is wanted), the one item of a root block list — through
        /// the renderers that spelled it there, with the least around it to
        /// misread; and on its own, as a document, which is all there is for
        /// the root and for an item of a flow list. A reading is only known
        /// when every one of these that reads as one value reads as the same
        /// value: a slot a format cannot write at the root reads as something
        /// else (a `.properties` value `a b` read alone is the entry `a` =
        /// `b`, a list item of a language whose list needs a header above it
        /// is no item at all), and two readings that disagree say only that
        /// one of them is not the slot's. Null leaves the value unchecked and
        /// the rest of the document checked still.
        fn readingOf(self: *Self, key_text: ?[]const u8, rendered: []const u8, place: Place, in: WriteSlot) !?Reading {
            var found: ?Reading = null;
            errdefer if (found) |r| r.deinit(self.allocator);
            inline for (.{ ReadSlot.item, ReadSlot.entry, ReadSlot.alone }) |slot| {
                const wanted = switch (slot) {
                    .item => in == .block_item,
                    .entry => in == .entry,
                    .alone => true,
                };
                if (wanted) if (try self.readingIn(slot, key_text, rendered, place)) |c| {
                    if (found) |f| {
                        const agree = readsAs(f.value(), c.doc, c.value().id);
                        c.deinit(self.allocator);
                        if (!agree) {
                            f.deinit(self.allocator);
                            found = null;
                            return null;
                        }
                    } else found = c;
                };
            }
            return found;
        }

        const ReadSlot = enum { item, entry, alone };

        /// `readingOf` in one slot: a document of one `item`, of one `entry`,
        /// or of the text `alone`; null when it does not read as that.
        fn readingIn(self: *Self, comptime slot: ReadSlot, key_text: ?[]const u8, rendered: []const u8, place: Place) !?Reading {
            var text: std.ArrayList(u8) = .empty;
            errdefer text.deinit(self.allocator);
            switch (slot) {
                .item => try self.writeItem(&text, "", rendered, place),
                .entry => try self.writeEntry(&text, "", key_text orelse "k", rendered, place),
                .alone => try text.appendSlice(self.allocator, rendered),
            }
            if (slot != .alone) try text.append(self.allocator, '\n');
            const source = try text.toOwnedSlice(self.allocator);
            var parser: Language.Parser = .{ .allocator = self.allocator };
            const doc = Language.parse(&parser, source, self.format) catch {
                self.allocator.free(source);
                return null;
            };
            const top = doc.ast.node(doc.ast.root);
            const only: ?AST.Node = if (top.kind == .mapping or top.kind == .sequence) only: {
                const child = (doc.ast.child(&top) catch null) orelse break :only null;
                break :only if (doc.ast.next(&child) == null) child else null;
            } else null;
            const read: ?Reading = switch (slot) {
                .item => if (top.kind == .sequence and only != null)
                    .{ .source = source, .doc = doc, .id = only.?.id, .entry = false }
                else
                    null,
                .entry => if (top.kind == .mapping and only != null and only.?.kind == .keyvalue)
                    (if (key_text != null)
                        .{ .source = source, .doc = doc, .id = only.?.id, .entry = true }
                    else
                        .{ .source = source, .doc = doc, .id = only.?.kind.keyvalue.value, .entry = false })
                else
                    null,
                // A root container with nothing of its own around its one
                // child — a TOML or `.properties` document of one entry, a
                // bar list's open root of one item — is the format reading
                // the text as an entry or an item of the root, not as the
                // value: no reading.
                .alone => if (only != null and onlyBlankAround(source, doc.span(only.?)))
                    null
                else
                    .{ .source = source, .doc = doc, .id = doc.ast.root, .entry = false },
            };
            if (read == null) {
                doc.deinit(self.allocator);
                self.allocator.free(source);
            }
            return read;
        }

        /// The write guard. A write is a splice chosen by reading the source
        /// around its slot, and a wrong reading still parses: a map written
        /// over a YAML item's scalar with its second line at column 0 put
        /// `y: two` in the mapping around the list, and the replace reported
        /// success. So the two parses are compared, as `keepOthers` compares
        /// them for a delete (`writtenOnlyAt`): the container on
        /// `guard.route` must have changed only as `guard.write` says, every
        /// other node must be as it was, and otherwise the splice is rolled
        /// back byte-for-byte with `EditReadsBackWrong`.
        fn readsBack(self: *Self, guard: *const Guard) !void {
            var parser: Language.Parser = .{ .allocator = self.allocator };
            const before = Language.parse(&parser, guard.backup, self.format) catch {
                // `backup` parsed before the splice, as in `keepOthers`.
                try self.restoreSource(guard.backup);
                return error.EditReadsBackWrong;
            };
            defer before.deinit(self.allocator);
            const after = try self.getParsed();
            if (!writtenOnlyAt(before, before.ast.root, after, after.ast.root, guard.route, guard.write)) {
                try self.restoreSource(guard.backup);
                return error.EditReadsBackWrong;
            }
        }

        fn spliceOutEntry(self: *Self, path: []const AST.PathSegment) !void {
            const parsed = try self.getParsed();
            const node = parsed.ast.getNodeByPath(path) catch |err| {
                // A key with no physical entry of its own — inherited through
                // the format's reference layer — has no line to delete, and
                // there is no syntax to un-inherit it; deleting the source it
                // comes from is a different operation. Refuse explicitly rather
                // than report it missing.
                if (err == error.NotFound and try self.keyIsInherited(parsed, path))
                    return error.MergeOnlyKey;
                return err;
            };
            if (node.kind != .keyvalue) return error.NotAMapping;
            const span = parsed.span(node);
            const source = self.source.items;
            // The engine's veto, before anything is spliced: an entry whose
            // value is a SECTION node, assembled from lines elsewhere in the
            // file, so the line-based delete below would remove only the piece
            // it can see and orphan or misparse the rest. `deleteContainer`
            // is the op for it. See "Whole-container structural editing".
            if (parsed.isSection(parsed.ast.node(node.kind.keyvalue.value))) return self.refuse(.delete);
            // A flow (`{...}`) mapping stores its entries comma-separated, not
            // one-per-line, so the line-based delete below (sized for a *block*
            // mapping's one-entry-per-line shape) mishandles it in three ways:
            //   * packed `{ a: 1, b: 2 }` (the only shape JSON/JSON5/JSONC
            //     objects ever have, and one inline YAML/TOML/fig/ZON mappings
            //     also allow) — deleting the whole line swallows the sibling;
            //   * the *last* entry of a one-per-line flow mapping — the line
            //     delete strands the predecessor's separator comma before the
            //     closing `}`, invalid in strict JSON and in TOML inline tables;
            //   * a *single-entry* flow mapping — the line delete removes the
            //     whole `{ … }` down to nothing, which YAML/TOML/fig (whose
            //     grammar reads an empty document as an empty mapping) then
            //     silently "succeed" at reparsing, committing a wiped file to
            //     disk.
            // So route every flow-mapping entry through `removeFlowItem` — the
            // same comma-aware splice a flow *sequence* delete uses — which
            // drops exactly one adjoining separator (the following comma for the
            // first entry, the preceding comma otherwise) and always leaves the
            // enclosing braces intact, correct for every arity and position.
            const parent = try parsed.ast.getValByPath(path[0 .. path.len - 1]);
            if (parent.kind == .mapping and self.isFlowNode(parsed, parent)) {
                // Find the entry's immediate predecessor: `removeFlowItem` drops
                // the *following* comma for the first entry and the *preceding*
                // comma for any later one, so it needs to know which this is.
                var item = (try parsed.ast.child(&parent)).?;
                var prev: ?AST.Node = null;
                while (item.id != node.id) {
                    prev = item;
                    item = parsed.ast.next(&item) orelse return error.NotFound;
                }
                // A key span that EXCLUDES the format's key sigil — ZON's
                // leading `.`, whose span starts at the bare identifier (see
                // `insertFlowMapEntry`'s doc on the same quirk) — is backed up
                // over so the splice carries `.name` as a unit rather than
                // stranding a bare `.` next to a survivor.
                const entry_start = if (self.syntax().key_sigil) |sigil|
                    if (span.start > 0 and source[span.start - 1] == sigil) span.start - 1 else span.start
                else
                    span.start;
                return self.removeFlowEntry(Span.init(entry_start, span.end), prev == null);
            }
            // An entry of a table made by dotted keys inside a flow mapping
            // (TOML's `b` in `it = { a.b = 1 }`) is that flow mapping's
            // entry `a.b = 1`, not a line: the table is a section, read as
            // block, and the line delete took the whole `it` line.
            if (parent.kind == .mapping) if (self.dottedInFlow(parsed, parent)) |owner| {
                const entry = dottedEntrySpan(parsed, owner.top, node);
                return self.removeFlowEntry(entry, entry.start <= flowFirstEntryStart(parsed, owner.flow));
            };
            const line_start = lineStartBefore(source, span.start);
            // A block entry's lines are its own in every compiled format, but
            // a runtime one may put two on a line (a minified stylesheet's
            // `a{} b{}`). Deleting the lines then would take the sibling with
            // them, and the reparse would not notice: one entry fewer still
            // parses. Such an entry is deleted by its span, with the spaces
            // that part it from whichever neighbour stays on its line.
            var prev_end: ?usize = null;
            var next_start: ?usize = null;
            var sib = try parsed.ast.child(&parent);
            while (sib) |c| : (sib = parsed.ast.next(&c)) {
                if (c.id == node.id) {
                    if (parsed.ast.next(&c)) |n| next_start = markerStart(parsed, n);
                    break;
                }
                prev_end = parsed.span(c).end;
            }
            const shares_start = if (prev_end) |e| lineStartBefore(source, e -| 1) == line_start else false;
            const shares_end = if (next_start) |s| lineStartBefore(source, s) == lineStartBefore(source, span.end -| 1) else false;
            if (shares_start or shares_end) {
                var s = span.start;
                var e = span.end;
                if (shares_end) {
                    while (e < source.len and (source[e] == ' ' or source[e] == '\t')) e += 1;
                } else {
                    while (s > 0 and (source[s - 1] == ' ' or source[s - 1] == '\t')) s -= 1;
                }
                return self.replaceAtSpan(Span.init(s, e), "");
            }
            if (prev_end == null) {
                if (self.sharedLineDelete(parsed, parent, span.start, span.end, next_start)) |shared|
                    return self.replaceAtSpan(shared, "");
            }
            const del_start = commentBlockStart(source, line_start, self.syntax().comments);
            const del_end = lineEndAfter(source, span.end -| 1);
            try self.replaceAtSpan(Span.init(del_start, del_end), "");
        }

        /// Delete the entry of a flow mapping that runs over `entry`, and
        /// one separator beside it (`removeFlowItem`); `is_first` says
        /// whether any entry stands before it.
        ///
        /// When the entry begins its own physical line, any owned leading
        /// comment block above it goes too — trivia `removeFlowItem` can't
        /// find on its own but the block path would have carried. A packed
        /// entry (a sibling or the opening brace shares its line) is not
        /// first-on-line, so this is skipped and `removeFlowItem`'s own
        /// whitespace scan handles the separator and indentation. The
        /// comment extension applies only when a block was actually found
        /// (`cbs` climbed above `line_start`); with none, splicing from the
        /// entry itself keeps the survivors' indentation intact.
        fn removeFlowEntry(self: *Self, entry: Span, is_first: bool) !void {
            const source = self.source.items;
            const line_start = lineStartBefore(source, entry.start);
            const on_own_line = firstNonSpace(source, line_start) == entry.start;
            const cbs = if (on_own_line) commentBlockStart(source, line_start, self.syntax().comments) else line_start;
            const del_start = if (cbs < line_start) cbs else entry.start;
            return self.removeFlowItem(Span.init(del_start, entry.end), is_first, self.syntax().flow_entry_sep, Span.init(0, source.len));
        }

        /// A table made by dotted keys inside a flow mapping, as
        /// `dottedInFlow` finds it: `flow` is the flow mapping, and `top`
        /// the table among its children whose name begins each entry that
        /// writes the table (`a` in `{ a.b.c = 1 }`, for `a` or `a.b`).
        const DottedInFlow = struct { flow: AST.Node, top: AST.Node };

        /// Whether the mapping `node` is a table made by dotted keys inside
        /// a flow mapping — TOML's `a` in `it = { a.b = 1 }` — and which.
        /// Such a table is a section, its name written on the flow
        /// mapping's line as an `.entry` mention and never on a header of
        /// its own, so it reads as block; but what writes it is entries of
        /// the flow mapping, each a key path and a value, taken apart by
        /// the flow splice. Null for any other node: a table with a header,
        /// a dotted table among block lines (its chain reaches a block
        /// table), a mapping that is not a section.
        fn dottedInFlow(self: *const Self, parsed: Document, node: AST.Node) ?DottedInFlow {
            var cur = node;
            while (true) {
                if (cur.kind != .mapping or !parsed.isSection(cur)) return null;
                var mentions = parsed.mentionsOf(cur.id);
                if (mentions.count() == 0) return null;
                while (mentions.next()) |m| if (m.kind != .entry) return null;
                const kv_id = parsed.ast.parentOf(cur.id) orelse return null;
                if (parsed.ast.kindOf(kv_id) != .keyvalue) return null;
                const up = parsed.ast.node(parsed.ast.parentOf(kv_id) orelse return null);
                if (up.kind == .mapping and self.isFlowNode(parsed, up)) return .{ .flow = up, .top = cur };
                cur = up;
            }
        }

        /// The span of the flow mapping's entry that writes `leaf`, an entry
        /// of a table under the dotted table `top` (`DottedInFlow`): from the
        /// last place `top`'s name is written before `leaf`'s key, which
        /// begins the entry's key path, to the end of `leaf`'s value.
        fn dottedEntrySpan(parsed: Document, top: AST.Node, leaf: AST.Node) Span {
            const span = parsed.span(leaf);
            var start = span.start;
            var mentions = parsed.mentionsOf(top.id);
            while (mentions.next()) |m| {
                if (m.span.start > span.start) break;
                start = m.span.start;
            }
            return Span.init(start, span.end);
        }

        /// Where the first entry of the flow mapping `flow` begins: its
        /// first child's span, or the first place its name is written when
        /// it is a table made by dotted keys.
        fn flowFirstEntryStart(parsed: Document, flow: AST.Node) usize {
            const first = (parsed.ast.child(&flow) catch null) orelse return parsed.span(flow).end;
            return flowEntryStart(parsed, first);
        }

        /// Where the first entry of a flow mapping that writes its child
        /// `kv` begins: `kv`'s span, or the first place its name is written
        /// when it is a table made by dotted keys.
        fn flowEntryStart(parsed: Document, kv: AST.Node) usize {
            if (kv.kind == .keyvalue) {
                var mentions = parsed.mentionsOf(kv.kind.keyvalue.value);
                if (mentions.count() > 0) return @min(mentions.at(0).span.start, parsed.span(kv).start);
            }
            return parsed.span(kv).start;
        }

        /// Where the last entry of a flow mapping that writes its child
        /// `kv` ends: `kv`'s span, or the end of the last entry under it
        /// when it is a table made by dotted keys, whose own span is only
        /// its name (`a` in `{ c = 2, a.b = 1 }`).
        fn flowEntryEnd(self: *const Self, parsed: Document, kv: AST.Node) !usize {
            if (dottedValue(parsed, kv)) |value| {
                if (try self.lastWrittenLeaf(parsed, value, true)) |leaf| return parsed.span(leaf).end;
            }
            return parsed.span(kv).end;
        }

        /// The value of the entry `kv` when it is a table made by dotted
        /// keys, written by entries of its parent rather than by a value of
        /// its own: a section whose name is written on its parent's lines
        /// or in its braces (TOML's), or a mapping whose span begins where
        /// its key does (fig's `{a.b=1}`). Null for any other entry.
        fn dottedValue(parsed: Document, kv: AST.Node) ?AST.Node {
            if (kv.kind != .keyvalue) return null;
            const value = parsed.ast.node(kv.kind.keyvalue.value);
            if (value.kind != .mapping) return null;
            if (parsed.isSection(value) or parsed.span(value).start <= parsed.span(parsed.ast.node(kv.kind.keyvalue.key)).start) return value;
            return null;
        }

        /// The first entry written with a key and a value under the child
        /// `kv` of a flow mapping: `kv`, or the first entry of the table
        /// made by dotted keys that it is, and so on down.
        fn firstWrittenLeaf(parsed: Document, kv: AST.Node) AST.Node {
            var cur = kv;
            while (dottedValue(parsed, cur)) |value| {
                cur = (parsed.ast.child(&value) catch break) orelse break;
            }
            return cur;
        }

        /// Append the span of every entry of the flow mapping `owner.flow`
        /// that writes the dotted table `node` to `out`, in the tree's
        /// order rather than the source's: one per entry of the tables
        /// under it that is not itself such a table.
        fn dottedEntrySpans(self: *const Self, parsed: Document, owner: DottedInFlow, node: AST.Node, out: *std.ArrayList(Span)) !void {
            var cur = try parsed.ast.child(&node);
            while (cur) |kv| : (cur = parsed.ast.next(&kv)) {
                if (kv.kind != .keyvalue) continue;
                const value = parsed.ast.node(kv.kind.keyvalue.value);
                if (value.kind == .mapping and parsed.isSection(value))
                    try self.dottedEntrySpans(parsed, owner, value, out)
                else
                    try out.append(self.allocator, dottedEntrySpan(parsed, owner.top, kv));
            }
        }

        /// Append `value_text` as a new item to the sequence at `path`.
        ///
        /// A flow sequence — `[…]`, an open root, or a sequence its parser
        /// gave a separator of its own (`Document.itemSep`) — takes
        /// the separator-aware splice after its last item; a block item is
        /// spelled through `writeItem` (a `renderItem` where the format
        /// declares one) after the first item's line prefix.
        ///
        /// The result is held to what an insert promises (`readsBack`): the
        /// sequence gained one item, last, reading as `value_text` reads, and
        /// nothing else changed, or `EditReadsBackWrong` and the source as it
        /// was. `prependToSeq` and `insertItem` are held to the same.
        pub fn appendToSeq(self: *Self, path: []const AST.PathSegment, value_text: []const u8) !void {
            return self.addItem(path, std.math.maxInt(usize), value_text, .append);
        }

        fn spliceAppend(self: *Self, path: []const AST.PathSegment, value_text: []const u8) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .sequence) return error.NotASequence;
            const span = parsed.span(node);
            const source = self.source.items;
            if (self.isFlowNode(parsed, node)) {
                const first = node.kind.sequence;
                try self.insertFlowItem(parsed, node, span, first != null, value_text);
                return;
            }
            // A non-flow TOML sequence is an array-of-tables; use
            // `appendContainerToSeq` for those. (TOML has no block scalar array.)
            if (!self.syntax().block_seq_editable) return error.NotAnInlineArray;
            const last = (try parsed.ast.lastChild(&node)) orelse return self.expandEmptySeq(parsed, node, value_text);
            const first_item = (try parsed.ast.child(&node)).?;
            const insert_at = lineEndAfter(source, parsed.span(last).end -| 1);
            if (self.syntax().closed_containers) |closed| {
                if (closesOnLastLine(source, parsed.span(node), parsed.span(last), closed.seq.close, insert_at))
                    return error.ContainerClosesOnItsLine;
            }
            try self.insertSeqLine(insert_at, markerStart(parsed, first_item), value_text, placeOf(parsed, node));
        }

        /// The first item into an EMPTY block sequence: only a format whose
        /// containers close themselves has a spelling for one (`<array/>`,
        /// expanded); any other has no line to splice after.
        fn expandEmptySeq(self: *Self, parsed: Document, node: AST.Node, value_text: []const u8) !void {
            const closed = self.syntax().closed_containers orelse return error.NotASequence;
            const span = parsed.span(node);
            var base_buf: std.ArrayList(u8) = .empty;
            defer base_buf.deinit(self.allocator);
            const base = try self.indentAt(&base_buf, span.start);
            var child: std.ArrayList(u8) = .empty;
            defer child.deinit(self.allocator);
            try child.appendSlice(self.allocator, base);
            try child.appendSlice(self.allocator, self.syntax().indent_unit);
            var val_buf: std.ArrayList(u8) = .empty;
            defer val_buf.deinit(self.allocator);
            const place = placeOf(parsed, node);
            const rendered = try self.renderedValue(&val_buf, value_text, place);
            var body: std.ArrayList(u8) = .empty;
            defer body.deinit(self.allocator);
            try self.writeItem(&body, child.items, rendered, place);
            try self.expandEmptyContainer(span, closed.seq, base, body.items);
        }

        /// Insert `value_text` before the first item of the sequence at `path`.
        ///
        /// The twin of `appendToSeq`, on the same terms; `insertItem` at 0.
        pub fn prependToSeq(self: *Self, path: []const AST.PathSegment, value_text: []const u8) !void {
            return self.addItem(path, 0, value_text, .prepend);
        }

        fn splicePrepend(self: *Self, path: []const AST.PathSegment, value_text: []const u8) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .sequence) return error.NotASequence;
            const span = parsed.span(node);
            const source = self.source.items;
            if (self.isFlowNode(parsed, node)) {
                try self.insertFlowItemBefore(parsed, node, span, try parsed.ast.child(&node), value_text, false);
                return;
            }
            if (!self.syntax().block_seq_editable) return error.NotAnInlineArray;
            const first_item = (try parsed.ast.child(&node)) orelse return self.expandEmptySeq(parsed, node, value_text);
            const first_start = markerStart(parsed, first_item);
            try self.insertSeqLine(lineStartBefore(source, first_start), first_start, value_text, placeOf(parsed, node));
        }

        /// Insert `value_text` into the sequence at `path` so that it is the
        /// item at `index` afterwards: before the item now there, or after
        /// the last one when `index` is the length. `index == std.math.maxInt(usize)`
        /// is the "end" sentinel `parsePath` makes of `[-]`/`[$]`, as for
        /// `removeSeqItem`, and appends. An index past the length is
        /// `NotFound`, and nothing is written.
        ///
        /// 0 is `prependToSeq` and the length is `appendToSeq`, exactly.
        /// Between them, a flow item goes in before the item at `index`, its
        /// separator laid out as the sequence is (`writeJoint`); a block item
        /// goes on a line of its own above the item at `index` and the
        /// comment block that item owns, which stays with it.
        pub fn insertItem(self: *Self, path: []const AST.PathSegment, index: usize, value_text: []const u8) !void {
            return self.addItem(path, index, value_text, .insert);
        }

        /// An item insert by `op`, at `index`, held to `readsBack`. A path
        /// that names no sequence is the splice's to report.
        fn addItem(self: *Self, path: []const AST.PathSegment, index: usize, value_text: []const u8, comptime op: enum { append, prepend, insert }) !void {
            const parsed = try self.getParsed();
            const node = parsed.ast.getValByPath(path) catch null;
            var guard = if (node != null and node.?.kind == .sequence)
                try self.addGuard(parsed, node.?, @min(index, try seqLen(parsed, node.?)), null, value_text)
            else
                null;
            defer if (guard) |*g| g.deinit(self.allocator);
            switch (op) {
                .append => try self.spliceAppend(path, value_text),
                .prepend => try self.splicePrepend(path, value_text),
                .insert => try self.spliceItem(path, index, value_text),
            }
            if (guard) |*g| try self.readsBack(g);
        }

        fn spliceItem(self: *Self, path: []const AST.PathSegment, index: usize, value_text: []const u8) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .sequence) return error.NotASequence;
            const n = try seqLen(parsed, node);
            if (index == std.math.maxInt(usize) or index == n) return self.spliceAppend(path, value_text);
            if (index > n) return error.NotFound;
            if (index == 0) return self.splicePrepend(path, value_text);
            var item = (try parsed.ast.child(&node)).?;
            for (0..index) |_| item = parsed.ast.next(&item).?;
            if (self.isFlowNode(parsed, node)) {
                try self.insertFlowItemBefore(parsed, node, parsed.span(node), item, value_text, true);
                return;
            }
            if (!self.syntax().block_seq_editable) return error.NotAnInlineArray;
            const source = self.source.items;
            const at = markerStart(parsed, item);
            const insert_at = commentBlockStart(source, lineStartBefore(source, at), self.syntax().comments);
            try self.insertSeqLine(insert_at, at, value_text, placeOf(parsed, node));
        }

        /// Remove the item at `index` from the sequence at `path`. `index ==
        /// std.math.maxInt(usize)` is the "end" sentinel — the same one
        /// `parsePath` produces for the `[-]`/`[$]` append token — and means
        /// "the last item" here, so `contents[-]` deletes symmetrically with
        /// how it appends.
        ///
        /// The block arm takes the item's whole owned block, from its leading
        /// comment run above its marker's line (see `markerStart`) through
        /// its last line. The flow arm takes the item and one separator
        /// beside it (`removeFlowItem`), the scan staying inside an open
        /// sequence's span. In a sequence the parser gave a separator of its
        /// own, an item that begins its line also takes the comment block
        /// above it, as a flow mapping's member does in `deleteKey`, and
        /// the comment that ends its last line when the next item is on a
        /// later one: a `.tune` block, a blank line from the next, goes with
        /// its comments.
        ///
        /// The first item of a block sequence that shares its line with the
        /// sequence's own item marker (`- - a`) takes only its own text
        /// (`sharedLineDelete`). Every arm is held to `keepOthers`, as
        /// `deleteKey` is.
        pub fn removeSeqItem(self: *Self, path: []const AST.PathSegment, index: usize) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .sequence) return error.NotASequence;
            const n = try seqLen(parsed, node);
            const removed = if (index == std.math.maxInt(usize)) n -| 1 else index;
            const backup = try self.allocator.dupe(u8, self.source.items);
            defer self.allocator.free(backup);
            try self.spliceOutItem(path, index);
            try self.keepOthers(backup, path, removed);
        }

        fn spliceOutItem(self: *Self, path: []const AST.PathSegment, index: usize) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .sequence) return error.NotASequence;
            const source = self.source.items;
            // Walk to the target item, keeping `prev` (its immediate
            // preceding sibling, or null when it's first) alongside: the flow
            // path's `is_first` needs it.
            var item = (try parsed.ast.child(&node)) orelse return error.NotFound;
            var prev: ?AST.Node = null;
            var at = index;
            if (index == std.math.maxInt(usize)) {
                at = 0;
                while (parsed.ast.next(&item)) |nxt| {
                    prev = item;
                    item = nxt;
                    at += 1;
                }
            } else {
                for (0..index) |_| {
                    prev = item;
                    item = parsed.ast.next(&item) orelse return error.NotFound;
                }
            }
            const is_first = prev == null;
            const item_span = parsed.span(item);
            if (self.isFlowNode(parsed, node)) {
                const joint = self.jointOf(parsed, node);
                const span = parsed.span(node);
                const limit = if (self.isOpen(parsed, node)) span else Span.init(0, source.len);
                var start = item_span.start;
                var end = item_span.end;
                if (joint.declared) {
                    const line_start = lineStartBefore(source, start);
                    if (firstNonSpace(source, line_start) == start) {
                        const cbs = commentBlockStart(source, line_start, self.syntax().comments);
                        if (cbs < line_start) start = @max(firstNonSpace(source, cbs), limit.start);
                        const next_on_later_line = if (parsed.ast.next(&item)) |n|
                            lineStartBefore(source, parsed.span(n).start) > lineStartBefore(source, end -| 1)
                        else
                            true;
                        if (next_on_later_line) {
                            if (self.lineEndComment(end, joint.token)) |trail| end = trail.end;
                        }
                    }
                }
                try self.removeFlowItem(Span.init(start, end), is_first, joint.token, Span.init(limit.start, @max(limit.end, end)));
                return;
            }
            if (!self.syntax().block_seq_editable) return error.NotAnInlineArray;
            const line_start = commentBlockStart(source, lineStartBefore(source, markerStart(parsed, item)), self.syntax().comments);
            const del_end = lineEndAfter(source, item_span.end -| 1);
            const next = parsed.ast.next(&item);
            if (is_first and next == null) {
                if (try self.emptiedSectionSeq(parsed, node)) return;
                var out: std.ArrayList(u8) = .empty;
                defer out.deinit(self.allocator);
                if (try self.emptiedBlockSeq(&out, parsed, node, item, line_start, del_end)) |span|
                    return self.replaceAtSpan(span, out.items);
            }
            if (is_first) {
                const next_start = if (next) |n| markerStart(parsed, n) else null;
                if (self.sharedLineDelete(parsed, node, markerStart(parsed, item), item_span.end, next_start)) |span|
                    return self.replaceAtSpan(span, "");
            }
            // A section item may be written in several places (figl's `ys[0]`
            // over `ys[1]` over `ys[0]` again), and its span runs from its
            // first line to its last, over the items between: its own lines
            // are its gathered regions, as `deleteContainer` takes them.
            var arena: std.heap.ArenaAllocator = .init(self.allocator);
            defer arena.deinit();
            const edits = try self.followingEdits(arena.allocator(), parsed, item, at);
            if (parsed.isSection(item)) {
                var used = try self.gatherRegions(parsed, item, true);
                defer used.deinit(self.allocator);
                return self.spliceOutEdited(used.items, edits);
            }
            try self.spliceOutEdited(&.{.{ .start = line_start, .end = del_end }}, edits);
        }

        /// One rewrite `followingEdits` asks for: `text` over `span`.
        const PathEdit = struct { span: Span, text: []const u8 };

        /// What keeps each item after `item`, which stands at `index` in its
        /// block sequence, the item it was once `item`'s lines are deleted,
        /// as rewrites in source order, their text in `arena`. Every other
        /// format records no item path and gets none.
        ///
        /// An item addressed by its index is renumbered: figl's index is a
        /// position (spec § 7.4), so the lines saying `ys[1]` said "the
        /// second item", and once the first is gone that item is `ys[0]`;
        /// left alone they would address one past the end, or the item
        /// after. Each `index` mention of a later item takes its new index
        /// (`Document.MentionKind`).
        ///
        /// The next item's `append` mention takes `item`'s, when the two
        /// differ: a line that repeats the header before it (figl's `+`,
        /// spec § 8) repeated `item`'s, which goes with `item`, and left
        /// alone it would repeat an earlier header or none — a `+` under
        /// `a[].b[]` re-running `a[]` appends to `a`. Written out it means
        /// what it meant. Two headers spelled alike are left as they are.
        fn followingEdits(self: *const Self, arena: std.mem.Allocator, parsed: Document, item: AST.Node, index: usize) ![]PathEdit {
            const source = self.source.items;
            var edits: std.ArrayList(PathEdit) = .empty;
            var header: ?[]const u8 = null;
            var own = parsed.mentionsOf(item.id);
            while (own.next()) |m| if (m.kind == .append) {
                header = source[m.span.start..m.span.end];
            };
            var at = index + 1;
            var later = parsed.ast.next(&item);
            while (later) |n| : ({
                later = parsed.ast.next(&n);
                at += 1;
            }) {
                var mentions = parsed.mentionsOf(n.id);
                while (mentions.next()) |m| switch (m.kind) {
                    .index => try edits.append(arena, .{ .span = m.span, .text = try std.fmt.allocPrint(arena, "{d}", .{at - 1}) }),
                    .append => if (at == index + 1) if (header) |h| {
                        if (!std.mem.eql(u8, source[m.span.start..m.span.end], h))
                            try edits.append(arena, try padded(arena, source, m.span, h));
                    },
                    .header, .entry => {},
                };
            }
            std.mem.sortUnstable(PathEdit, edits.items, {}, struct {
                fn lessThan(_: void, a: PathEdit, b: PathEdit) bool {
                    return a.span.start < b.span.start;
                }
            }.lessThan);
            return edits.items;
        }

        /// `text` over `span`, and over the spaces after it when something
        /// follows them on the line — a trailing comment — so that it stays
        /// at the column it stood at, or as many spaces after `text` as were
        /// after `span` when `text` reaches it.
        fn padded(arena: std.mem.Allocator, source: []const u8, span: Span, text: []const u8) !PathEdit {
            var end = span.end;
            while (end < source.len and source[end] == ' ') end += 1;
            if (end == span.end or end == source.len or source[end] == '\n' or source[end] == '\r')
                return .{ .span = span, .text = text };
            const width = end - span.start;
            const pad = if (width > text.len) width - text.len else end - span.end;
            const out = try arena.alloc(u8, text.len + pad);
            @memcpy(out[0..text.len], text);
            @memset(out[text.len..], ' ');
            return .{ .span = Span.init(span.start, end), .text = out };
        }

        /// Delete `removed` (ascending, disjoint, as `regions.normalize`
        /// leaves them) and write each of `edits` (ascending, from
        /// `followingEdits`) over its span, in one splice. An edit inside a
        /// removed region goes with it.
        fn spliceOutEdited(self: *Self, removed: []const Region, edits: []const PathEdit) !void {
            if (edits.len == 0) {
                if (removed.len == 1) return self.replaceAtSpan(Span.init(removed[0].start, removed[0].end), "");
                return regions.spliceOut(self, removed);
            }
            const source = self.source.items;
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            var pos: usize = 0;
            var e: usize = 0;
            for (removed) |r| {
                while (e < edits.len and edits[e].span.start < r.start) : (e += 1) {
                    if (edits[e].span.start < pos) continue;
                    try out.appendSlice(self.allocator, source[pos..edits[e].span.start]);
                    try out.appendSlice(self.allocator, edits[e].text);
                    pos = edits[e].span.end;
                }
                try out.appendSlice(self.allocator, source[pos..r.start]);
                pos = r.end;
            }
            for (edits[e..]) |edit| {
                if (edit.span.start < pos) continue;
                try out.appendSlice(self.allocator, source[pos..edit.span.start]);
                try out.appendSlice(self.allocator, edit.text);
                pos = edit.span.end;
            }
            try out.appendSlice(self.allocator, source[pos..]);
            try self.replaceAtSpan(Span.init(0, source.len), out.items);
        }

        /// The span to delete to remove the first entry or item of the
        /// block container `node`, running from `start` to `end`, when it
        /// shares its line with `node`'s own item marker — `- - a`, `- k: v`
        /// — or null when it begins its line and the line delete is right.
        /// `next` is where the sibling after it starts, if there is one.
        ///
        /// The line is not the entry's to delete: the parent item's marker
        /// is on it, and deleting the line took the parent with it, so
        /// `[[a, c]]` read back as `[c]`. Only the entry's own text goes.
        /// The sibling after it moves up into its place, at the column it
        /// already stands at (`- - c`, `- j: w`), when nothing but the
        /// entry's trailing comment and blank lines stand between them;
        /// with a comment line between, which is the sibling's, the
        /// sibling stays where it is and the parent's marker ends its line
        /// (`-` over `  - c`), as it does when there is no sibling.
        fn sharedLineDelete(self: *const Self, parsed: Document, node: AST.Node, start: usize, end: usize, next: ?usize) ?Span {
            const source = self.source.items;
            const marker = parsed.markerSpan(node) orelse return null;
            const line_start = lineStartBefore(source, start);
            if (marker.end > start or lineStartBefore(source, marker.start) != line_start) return null;
            if (next) |n| lift: {
                const next_line = lineStartBefore(source, n);
                if (next_line <= line_start or n - next_line != start - line_start) break :lift;
                var gap = if (self.lineEndComment(end, "")) |c| c.end else end;
                while (gap < n and std.ascii.isWhitespace(source[gap])) gap += 1;
                if (gap == n) return Span.init(start, n);
            }
            var s = start;
            while (s > line_start and (source[s - 1] == ' ' or source[s - 1] == '\t')) s -= 1;
            var e = lineEndAfter(source, end -| 1);
            if (e > 0 and source[e - 1] == '\n') e -= 1;
            if (e > 0 and source[e - 1] == '\r') e -= 1;
            return Span.init(s, e);
        }

        /// The span to splice `out` over to remove the ONLY item of the
        /// block sequence `node`, whose lines run from `line_start` to
        /// `del_end`, and leave `[]` in its place — or null when the plain
        /// line delete is right.
        ///
        /// A block sequence is nothing but its items, so deleting the lines
        /// of the last one deletes the sequence too: `links:` over `- a`
        /// would read back as `links:`, a null, where a flow list, JSON and
        /// TOML all keep the list and empty it. A format with flow syntax
        /// spells the empty list `[]`, and it goes where the value began:
        ///
        ///   * in place of the item when something else shares its marker's
        ///     line — a parent item's `-` in `- - a` — since that line is
        ///     not the item's to delete;
        ///   * after the entry's separator, or the parent item's marker, and
        ///     any anchor or tag the sequence carries (`links: &x`), with
        ///     the rest of that line — the key's trailing comment — and
        ///     anything between it and the item's block kept;
        ///   * alone, for the document root.
        ///
        /// A sequence whose span outlives its items' lines has delimiters of
        /// its own (plist's `<array>`) and empties without help; a format
        /// with no flow syntax has no `[]` to write, and one that renders
        /// its own tails does not write it after `: ` (NestedText reads
        /// `links: []` as the text `[]`). These take the line delete, as
        /// does a sequence with no recorded place to write into.
        fn emptiedBlockSeq(self: *const Self, out: *std.ArrayList(u8), parsed: Document, node: AST.Node, item: AST.Node, line_start: usize, del_end: usize) !?Span {
            if (!self.syntax().flow_containers or self.hasRenderer(.tail)) return null;
            const source = self.source.items;
            // The sequence's own text starts past its anchor and tag, which
            // stand on the line the value began on, and past the comment
            // that may end that line.
            const seq_span = parsed.span(node);
            if (seq_span.end > del_end) return null;
            var body = seq_span.start;
            for ([_]?Span{ parsed.anchorSpan(node), parsed.tagSpan(node) }) |prop| {
                if (prop) |p| body = @max(body, p.end);
            }
            if (self.lineEndComment(body, "")) |c| body = c.end;
            const ends_line = del_end > 0 and source[del_end - 1] == '\n';
            const marker = markerStart(parsed, item);
            if (firstNonSpace(source, lineStartBefore(source, marker)) != marker) {
                if (body < marker) return null;
                try out.appendSlice(self.allocator, if (ends_line) "[]\n" else "[]");
                return Span.init(marker, del_end);
            }
            if (body < line_start and std.mem.trim(u8, source[body..line_start], " \t\r\n").len > 0) return null;
            var point: ?usize = null;
            if (parsed.ast.parentOf(node.id)) |p| switch (parsed.ast.kindOf(p)) {
                .keyvalue => if (parsed.sepSpan(parsed.ast.node(p))) |sep| {
                    if (sep.end > sep.start) point = sep.end;
                },
                else => {},
            };
            for ([_]?Span{ parsed.markerSpan(node), parsed.anchorSpan(node), parsed.tagSpan(node) }) |prop| {
                if (prop) |p| point = @max(point orelse p.end, p.end);
            }
            if (point) |at| {
                if (at > line_start) return null;
                try out.appendSlice(self.allocator, " []");
                // A file that ended without a newline still does.
                const kept = source[at..line_start];
                try out.appendSlice(self.allocator, if (ends_line) kept else std.mem.trimEnd(u8, kept, "\r\n"));
                return Span.init(at, del_end);
            }
            if (node.id != parsed.ast.root) return null;
            try out.appendSlice(self.allocator, if (ends_line) "[]\n" else "[]");
            return Span.init(line_start, del_end);
        }

        /// Delete the only item of the sequence `node` when the sequence
        /// hangs under a header key, leaving the key and the format's empty
        /// list after it — `xs = []` — and return whether it did; false
        /// leaves the delete to the caller.
        ///
        /// A list under a header is a section: figl's `xs` over `> * a`, or
        /// the `xs[]` that appends an element and is that element's header
        /// too. Its key stands on a line of the sequence's own, or of its
        /// only item's, so deleting the item's lines took the key with them,
        /// where every flow list and a YAML block list keep the key and
        /// empty it. The parser says a value hangs under its key by
        /// recording a zero-width separator at the key's end
        /// (`Document.sepSpan`), and `Syntax.empty_seq_literal` goes there
        /// through `writeTail`, as a replace reframes a value.
        ///
        /// The key's line keeps what stands before the key's end — its
        /// marker run, a dotted or indexed path into the parent (`a[].b[]`
        /// becomes `a[].b = []`) — the comment block above it, and the
        /// comment ending it when that is the sequence's or its item's,
        /// the line being their header. The path past the key named the
        /// item, and an assignment's value there was the item's, so they
        /// go. Every other line of the sequence — the item's, wherever they
        /// are scattered, and any re-entry of the key — goes as
        /// `deleteContainer` takes them.
        ///
        /// The document root has no key. A format that renders its own
        /// tails, which `emptiedBlockSeq` leaves alone, writes the empty
        /// list in place of the item, and refuses with `NoEmptyRootList`
        /// when it does not read that back: figl reads an empty document as
        /// an empty mapping, and has no other spelling of an empty root list.
        fn emptiedSectionSeq(self: *Self, parsed: Document, node: AST.Node) !bool {
            const empty = self.syntax().empty_seq_literal orelse return false;
            const source = self.source.items;
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            if (node.id == parsed.ast.root) {
                if (!self.hasRenderer(.tail)) return false;
                var used = try self.gatherRegions(parsed, node, true);
                defer used.deinit(self.allocator);
                if (used.items.len == 0) return false;
                var pos: usize = 0;
                for (used.items, 0..) |r, i| {
                    try out.appendSlice(self.allocator, source[pos..r.start]);
                    if (i == 0) {
                        try out.appendSlice(self.allocator, empty);
                        try out.append(self.allocator, '\n');
                    }
                    pos = r.end;
                }
                try out.appendSlice(self.allocator, source[pos..]);
                self.replaceAtSpan(Span.init(0, source.len), out.items) catch |err| {
                    if (err == error.OutOfMemory) return err;
                    // The text was the engine's, not a caller's.
                    self.splice_rejected = false;
                    return error.NoEmptyRootList;
                };
                return true;
            }
            if (!parsed.isSection(node)) return false;
            const kv_id = parsed.ast.parentOf(node.id) orelse return false;
            if (parsed.ast.kindOf(kv_id) != .keyvalue) return false;
            const kv = parsed.ast.node(kv_id);
            const sep = parsed.sepSpan(kv) orelse return false;
            const key_span = parsed.span(parsed.ast.node(kv.kind.keyvalue.key));
            if (sep.start != key_span.end or sep.end != sep.start) return false;

            const key_line = lineStartBefore(source, key_span.start);
            const key_line_end = lineEndAfter(source, key_span.end);
            var content_end = key_line_end;
            if (content_end > key_line and source[content_end - 1] == '\n') content_end -= 1;
            if (content_end > key_line and source[content_end - 1] == '\r') content_end -= 1;

            var line: std.ArrayList(u8) = .empty;
            defer line.deinit(self.allocator);
            var buf: std.ArrayList(u8) = .empty;
            defer buf.deinit(self.allocator);
            const place = placeOfChild(parsed, node.id);
            const indent = try self.indentAt(&buf, key_span.start);
            const rendered = try self.renderedValue(&buf, empty, place);
            try line.appendSlice(self.allocator, source[key_line..key_span.end]);
            try self.writeTail(&line, indent, source[key_span.start..key_span.end], rendered, place);
            if (self.headerComment(parsed, node, key_span.end, content_end)) |c|
                try line.appendSlice(self.allocator, source[c.start..c.end]);
            try line.appendSlice(self.allocator, source[content_end..key_line_end]);

            var used = try self.gatherRegions(parsed, node, true);
            defer used.deinit(self.allocator);
            var pos: usize = 0;
            var written = false;
            for (used.items) |r| {
                if (!written and r.start <= key_line and key_line < r.end) {
                    // The comment block above the key's line is the key's.
                    try out.appendSlice(self.allocator, source[pos..key_line]);
                    try out.appendSlice(self.allocator, line.items);
                    written = true;
                } else {
                    try out.appendSlice(self.allocator, source[pos..r.start]);
                }
                pos = r.end;
            }
            if (!written) return false;
            try out.appendSlice(self.allocator, source[pos..]);
            try self.replaceAtSpan(Span.init(0, source.len), out.items);
            return true;
        }

        /// Delete the only element of the sequence `node`, at `path`, when
        /// each element is a header of its own (TOML's `[[x]]`), leaving
        /// the format's empty list in the body of the table that holds the
        /// sequence — `x = []` — and return whether it did; false leaves the
        /// delete to the caller.
        ///
        /// Such a sequence has no line of its own to empty: every element
        /// is a `[[x]]` header, and taking the only one took the key, where
        /// an inline array emptied the same way keeps it. Nor does its key
        /// stand where the header did: `x = []` there would be an entry of
        /// the table above it. It goes in the body of the table that holds
        /// `x`, through `insertKey`, as any new entry of that table does —
        /// the root's entries above the first header, `[t]`'s under its own
        /// header, an element's under its `[[a]]`.
        ///
        /// A table with no header of its own has no body to take it: `t`
        /// made by `[[t.x]]` alone, or by a dotted line `t.k = 1`, whose
        /// lines are its parent's. `insertKey` refuses such a table
        /// (`ImplicitSection`), so the nearest table up that has a body
        /// takes the key path from there, spelled as a header spells it
        /// (`t.x = []`): a dotted key, as `t.k = 1` is, which a later
        /// `[t.u]` may still extend.
        ///
        /// The comment block above the element's header and the comment
        /// ending its line were the list's, and stay with `x = []`. The
        /// element's lines go as `deleteContainer` takes them, with the
        /// blank lines that set its header apart when a blank line or the
        /// end of the file follows it. The result must read back with an
        /// empty list at `path`, or nothing is written
        /// (`DeleteChangedOtherValues`).
        fn emptiedHeaderSeq(self: *Self, parsed: Document, node: AST.Node, path: []const AST.PathSegment) !bool {
            const empty = self.syntax().empty_seq_literal orelse return false;
            const hdr = self.syntax().section_header orelse return false;
            if (path.len == 0 or !parsed.isSection(node)) return false;
            const kv_id = parsed.ast.parentOf(node.id) orelse return false;
            if (parsed.ast.kindOf(kv_id) != .keyvalue) return false;
            var holder = parsed.ast.node(parsed.ast.parentOf(kv_id) orelse return false);
            var depth: usize = 1;
            while (!self.ownsHeaderLine(parsed, holder)) : (depth += 1) {
                const up = parsed.ast.parentOf(holder.id) orelse return false;
                if (parsed.ast.kindOf(up) != .keyvalue) return false;
                holder = parsed.ast.node(parsed.ast.parentOf(up) orelse return false);
            }
            if (depth > path.len) return false;
            const key_path = path[path.len - depth ..];
            for (key_path) |seg| if (seg != .key) return false;

            const source = self.source.items;
            const heads = parsed.regionsOf(node.id);
            const line = lineStartBefore(source, heads[0].start);
            const above = try self.allocator.dupe(u8, source[commentBlockStart(source, line, self.syntax().comments)..line]);
            defer self.allocator.free(above);
            const trail = try self.allocator.dupe(u8, if (self.headerLineComment(parsed, node, hdr, line)) |c| source[c.start..c.end] else "");
            defer self.allocator.free(trail);
            var key_text: std.ArrayList(u8) = .empty;
            defer key_text.deinit(self.allocator);
            try self.writeHeaderPath(&key_text, hdr, key_path);

            const backup = try self.allocator.dupe(u8, source);
            defer self.allocator.free(backup);
            var used = try self.gatherRegions(parsed, node, true);
            defer used.deinit(self.allocator);
            // The blank lines that set the element's header apart go with
            // it when a blank line or the end of the file follows it, so
            // the header's going does not leave two runs of them together.
            var prev_end: usize = 0;
            for (used.items) |*r| {
                if (r.end == source.len or source[r.end] == '\n') {
                    while (r.start >= prev_end + 2 and source[r.start - 1] == '\n' and source[r.start - 2] == '\n') r.start -= 1;
                }
                prev_end = r.end;
            }
            try regions.spliceOut(self, used.items);
            self.writeEmptied(path, path[0 .. path.len - depth], key_text.items, empty, above, trail) catch |err| {
                try self.restoreSource(backup);
                if (err == error.OutOfMemory) return err;
                self.splice_rejected = false;
                return error.DeleteChangedOtherValues;
            };
            return true;
        }

        /// The comment ending the element header of the section sequence
        /// `node` that stands on the line at `line` — `# c` in `[[x]] # c`
        /// — as the span from the spaces before it, or null. A parser may
        /// hand that comment to the next entry as a leading one (TOML's
        /// does), so it is found where it is written: past the sequence's
        /// name on that line and the header's close.
        fn headerLineComment(self: *const Self, parsed: Document, node: AST.Node, hdr: lang.SectionHeader, line: usize) ?Span {
            const source = self.source.items;
            const close = hdr.seq_close orelse hdr.close;
            var mentions = parsed.mentionsOf(node.id);
            while (mentions.next()) |m| {
                if (lineStartBefore(source, m.span.start) != line) continue;
                var i = firstNonSpace(source, m.span.end);
                if (!std.mem.startsWith(u8, source[i..], close)) return null;
                i += close.len;
                return self.lineEndComment(i, "");
            }
            return null;
        }

        /// Write `key_text = empty` into the table at `holder`, with `above`
        /// over its line and `trail` after it, and check that the list at
        /// `path` reads back empty. `emptiedHeaderSeq`'s second half.
        fn writeEmptied(self: *Self, path: []const AST.PathSegment, holder: []const AST.PathSegment, key_text: []const u8, empty: []const u8, above: []const u8, trail: []const u8) !void {
            try self.insertKey(holder, key_text, empty);
            const parsed = try self.getParsed();
            const seq = try parsed.ast.getValByPath(path);
            if (seq.kind != .sequence or seq.kind.sequence != null) return error.DeleteChangedOtherValues;
            if (above.len == 0 and trail.len == 0) return;
            const source = self.source.items;
            const span = parsed.span(try parsed.ast.getNodeByPath(path));
            const line = lineStartBefore(source, span.start);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            try out.appendSlice(self.allocator, above);
            try out.appendSlice(self.allocator, source[line..span.end]);
            try out.appendSlice(self.allocator, trail);
            try self.replaceAtSpan(Span.init(line, span.end), out.items);
        }

        /// Whether the mapping `mapping` has a body of its own to take a
        /// new entry: it is the root, an element of a sequence of sections
        /// (its `[[a]]` line), or a section with a header line that is its
        /// own rather than one of its children's (`[t]`, not the `t` in
        /// `[t.u]` or `[[t.x]]`). A dotted table's lines are its parent's.
        fn ownsHeaderLine(self: *const Self, parsed: Document, mapping: AST.Node) bool {
            if (mapping.kind != .mapping) return false;
            const parent = parsed.ast.parentOf(mapping.id) orelse return true;
            if (parsed.ast.kindOf(parent) == .sequence) return true;
            const source = self.source.items;
            var mentions = parsed.mentionsOf(mapping.id);
            header: while (mentions.next()) |m| {
                if (m.kind != .header) continue;
                const line = lineStartBefore(source, m.span.start);
                var cur = parsed.ast.child(&mapping) catch return false;
                while (cur) |kv| : (cur = parsed.ast.next(&kv)) {
                    if (self.outOfRegion(parsed, kv) and lineStartBefore(source, parsed.span(kv).start) == line)
                        continue :header;
                }
                return true;
            }
            return false;
        }

        /// A table with no header of its own, as `headerlessHolder` finds
        /// it: `holder` is the nearest mapping up that has lines or braces
        /// of its own, which write the table's entries; `top` is the table
        /// among the holder's children on the way down, whose name begins
        /// each of those entries; `depth` is how many keys down from the
        /// holder the table is; `flow` is whether the holder is a flow
        /// mapping.
        const Headerless = struct { holder: AST.Node, top: AST.Node, depth: usize, flow: bool };

        /// Whether the mapping `node` is a table with no header line of its
        /// own — TOML's `a` made by `a.b = 1` alone, by `[a.b]` alone, or by
        /// `a.b = 1` inside an inline table, and fig's `a` in `{a.b=1}` —
        /// and what holds it. Null for any other node: the root, a table
        /// with a header, an element of an array of tables, a mapping that
        /// is a value of its own.
        fn headerlessHolder(self: *const Self, parsed: Document, node: AST.Node) ?Headerless {
            if (!is_section_format) return null;
            if (!self.writtenByParent(parsed, node)) return null;
            var cur = node;
            var depth: usize = 1;
            while (true) : (depth += 1) {
                const up = parsed.ast.node(parsed.ast.parentOf(parsed.ast.parentOf(cur.id).?).?);
                if (up.kind != .mapping) return null;
                if (self.isFlowNode(parsed, up))
                    return .{ .holder = up, .top = cur, .depth = depth, .flow = true };
                if (!self.writtenByParent(parsed, up)) {
                    // A dotted table among the block lines of a format
                    // whose line prefix is structural (fig's) is left to
                    // `insertBlockKey`, which copies the bytes up to the
                    // first entry's key, key path and all, as that prefix.
                    if (!parsed.isSection(cur) or self.syntax().structural_indent) return null;
                    return .{ .holder = up, .top = cur, .depth = depth, .flow = false };
                }
                cur = up;
            }
        }

        /// Whether the mapping `node` is a table made by dotted keys, whose
        /// entries are its parent's lines or entries: a section with no
        /// header line of its own, or a value `dottedValue` names.
        fn writtenByParent(self: *const Self, parsed: Document, node: AST.Node) bool {
            if (node.kind != .mapping) return false;
            const kv = parsed.ast.parentOf(node.id) orelse return false;
            if (parsed.ast.kindOf(kv) != .keyvalue) return false;
            if (parsed.isSection(node)) return !self.ownsHeaderLine(parsed, node);
            return dottedValue(parsed, parsed.ast.node(kv)) != null;
        }

        /// The entry under the headerless table `node` that is written last
        /// among its holder's lines or braces, or null when none is: one
        /// that is not itself a table written there, found down through
        /// the tables that are. An entry under a header of its own
        /// (`[a.b]`'s) is in another body and is passed over.
        fn lastWrittenLeaf(self: *const Self, parsed: Document, node: AST.Node, flow: bool) !?AST.Node {
            var last: ?AST.Node = null;
            var cur = try parsed.ast.child(&node);
            while (cur) |kv| : (cur = parsed.ast.next(&kv)) {
                if (kv.kind != .keyvalue) continue;
                if (!flow and self.outOfRegion(parsed, kv)) continue;
                const leaf = if (dottedValue(parsed, kv)) |value|
                    try self.lastWrittenLeaf(parsed, value, flow)
                else
                    kv;
                if (leaf) |l| if (last == null or parsed.span(l).end > parsed.span(last.?).end) {
                    last = l;
                };
            }
            return last;
        }

        /// The first entry of the headerless table `node` written among its
        /// holder's lines or braces, or null when none is.
        fn firstWrittenChild(self: *const Self, parsed: Document, node: AST.Node, flow: bool) ?AST.Node {
            var cur = parsed.ast.child(&node) catch return null;
            while (cur) |kv| : (cur = parsed.ast.next(&kv)) {
                if (kv.kind != .keyvalue) continue;
                if (!flow and self.outOfRegion(parsed, kv)) continue;
                return kv;
            }
            return null;
        }

        /// Where the dotted table `table`'s name begins the entry at `at`:
        /// the last place it is written up to there, or its key's span for
        /// a parser that records no such place (fig's), which is right for
        /// the table's first entry.
        fn dottedEntryStart(parsed: Document, table: AST.Node, at: usize) usize {
            var start: ?usize = null;
            var mentions = parsed.mentionsOf(table.id);
            while (mentions.next()) |m| {
                if (m.span.start > at) break;
                start = m.span.start;
            }
            if (start) |s| return s;
            const kv = parsed.ast.parentOf(table.id) orelse return at;
            return switch (parsed.ast.kindOf(kv)) {
                .keyvalue => |e| @min(at, parsed.span(parsed.ast.node(e.key)).start),
                else => at,
            };
        }

        /// Insert `key_text` and `value_text` into the table `node` at
        /// `path`, which has no header of its own (`headerlessHolder`).
        ///
        /// Its entries are lines of the holder's body or entries of the
        /// holder's braces, each a key path from the holder down (`a.b = 1`
        /// for `a`), and a new one is written the same way: an entry `y`
        /// spelled as a bare key there would be the holder's own. The key
        /// path is copied from the table's first entry as written, up to
        /// the key under the table, so its segments keep their quoting and
        /// spacing, and the new entry goes after the line of the table's
        /// last entry in a body, or after the last entry inside the braces
        /// with the flow mapping's own separator. A table with no entry there (`a` made
        /// by `[a.b]` alone) takes the path spelled as a header spells it,
        /// in the holder's body as any of its entries goes (`a.y = 1` at
        /// the root, above the first header): valid TOML, which a later
        /// `[a.b]` may still extend. A format with no header spelling
        /// leaves such a table to `insertBlockKey`, which refuses it
        /// (`ImplicitSection`).
        ///
        /// The result must read back with the table grown and nothing else
        /// changed, or nothing is written (`ImplicitSection`).
        fn insertHeaderless(self: *Self, parsed: Document, path: []const AST.PathSegment, node: AST.Node, owner: Headerless, key_text: []const u8, value_text: []const u8) !void {
            const source = self.source.items;
            var key: std.ArrayList(u8) = .empty;
            defer key.deinit(self.allocator);
            const leaf = try self.lastWrittenLeaf(parsed, node, owner.flow);
            if (self.firstWrittenChild(parsed, node, owner.flow)) |below| {
                // From the place the top table's name begins the table's
                // first entry to the key just under `node` on it.
                const first = firstWrittenLeaf(parsed, below);
                const from = dottedEntryStart(parsed, owner.top, parsed.span(first).start);
                const to = if (dottedValue(parsed, below)) |table|
                    dottedEntryStart(parsed, table, parsed.span(first).start)
                else
                    parsed.span(below).start;
                try key.appendSlice(self.allocator, source[from..to]);
            } else {
                const hdr = self.syntax().section_header orelse return self.insertBlockKey(parsed, node, key_text, value_text);
                if (owner.depth > path.len) return error.ImplicitSection;
                const key_path = path[path.len - owner.depth ..];
                for (key_path) |seg| if (seg != .key) return error.ImplicitSection;
                try self.writeHeaderPath(&key, hdr, key_path);
                try key.appendSlice(self.allocator, hdr.sep);
            }
            try key.appendSlice(self.allocator, key_text);

            const backup = try self.allocator.dupe(u8, source);
            defer self.allocator.free(backup);
            if (owner.flow) {
                try self.insertFlowMapEntry(parsed, owner.holder, parsed.span(owner.holder), true, key.items, value_text);
            } else if (leaf) |l| {
                var out: std.ArrayList(u8) = .empty;
                defer out.deinit(self.allocator);
                var val_buf: std.ArrayList(u8) = .empty;
                defer val_buf.deinit(self.allocator);
                const place = placeOf(parsed, node);
                const rendered = try self.renderedValue(&val_buf, value_text, place);
                var indent_buf: std.ArrayList(u8) = .empty;
                defer indent_buf.deinit(self.allocator);
                const indent = try self.indentAt(&indent_buf, dottedEntryStart(parsed, owner.top, parsed.span(l).start));
                const insert_at = lineEndAfter(source, parsed.span(l).end -| 1);
                if (insert_at > 0 and source[insert_at - 1] != '\n') try out.append(self.allocator, '\n');
                try out.appendSlice(self.allocator, indent);
                try self.writeEntry(&out, indent, key.items, rendered, place);
                try out.append(self.allocator, '\n');
                try self.replaceAtSpan(Span.init(insert_at, insert_at), out.items);
            } else {
                try self.insertBlockKey(parsed, owner.holder, key.items, value_text);
            }

            var parser: Language.Parser = .{ .allocator = self.allocator };
            const before = Language.parse(&parser, backup, self.format) catch {
                try self.restoreSource(backup);
                return error.ImplicitSection;
            };
            defer before.deinit(self.allocator);
            if (!changedOnlyAt(before, try self.getParsed(), path, .added)) {
                try self.restoreSource(backup);
                return error.ImplicitSection;
            }
        }

        /// The comment ending the header line of the section sequence
        /// `node` between `from` and `to` — the sequence's own trailing
        /// comment, or its only item's when the item's first header line is
        /// that line — as the span from the spaces before it; null when
        /// neither has one there.
        fn headerComment(self: *const Self, parsed: Document, node: AST.Node, from: usize, to: usize) ?Span {
            const marker = self.trailingCommentMarker() orelse return null;
            const source = self.source.items;
            const line = lineStartBefore(source, from);
            var text = parsed.ast.comments(node.id).trailing;
            if (text == null) if (parsed.ast.child(&node) catch null) |item| {
                const heads = parsed.regionsOf(item.id);
                if (heads.len > 0 and heads[0].start == line) text = parsed.ast.comments(item.id).trailing;
            };
            const want = (text orelse return null).text;
            var i = from;
            while (std.mem.indexOfPos(u8, source[0..to], i, marker.open)) |at| : (i = at + 1) {
                if (at == from or (source[at - 1] != ' ' and source[at - 1] != '\t')) continue;
                const c = self.lineEndComment(at, "") orelse continue;
                const body = std.mem.trim(u8, source[at + marker.open.len .. c.end], " \t");
                if (!std.mem.eql(u8, body, want)) continue;
                var start = at;
                while (start > from and (source[start - 1] == ' ' or source[start - 1] == '\t')) start -= 1;
                return Span.init(start, c.end);
            }
            return null;
        }

        /// Reconcile the sequence at `path` so its items are exactly `items` —
        /// each an already-serialized *scalar* value in this document's format —
        /// while preserving the comments on items that survive the change.
        ///
        /// Items are matched to the current items by abstract value (kind +
        /// value, honoring multiplicity), so an item that is kept or merely
        /// reordered keeps its leading and trailing comments; only a genuinely
        /// new value is inserted and only a genuinely dropped value is deleted.
        /// The final item order matches `items`. This is the comment-preserving
        /// alternative to replacing the whole list value (which would blow every
        /// item's comments away).
        ///
        /// It is a thin orchestration over `appendToSeq` / `removeSeqItem` /
        /// `reorderItems`: append the new values, delete the dropped ones, then
        /// reorder to `items`. The compound edit is atomic — on any error the
        /// document is restored byte-for-byte.
        ///
        /// Declines (errors) rather than guessing when the shape isn't a flat
        /// scalar list it can safely diff:
        ///   * a target that isn't a sequence value -> `NotASequence`;
        ///   * empty `items`, an empty current list, or any non-scalar item on
        ///     either side -> `UnsupportedShape` — the caller should fall back to
        ///     replacing the whole value (e.g. with `[]` for the empty case).
        /// A format whose scalars cannot stand alone as a document (TOML) also
        /// surfaces as `UnsupportedShape`; reconciling a TOML inline array buys
        /// nothing anyway, as it carries no per-element comments.
        ///
        /// A sequence that is not there — an absent key or a null — is
        /// started as `setAppend` starts one, and `items` appended to it in
        /// order, there being no item to keep; atomic as the rest is.
        pub fn setSequence(self: *Self, path: []const AST.PathSegment, items: []const []const u8) !void {
            if (items.len == 0) return error.UnsupportedShape;

            if (self.seqMissing(path)) {
                const backup = try self.allocator.dupe(u8, self.source.items);
                defer self.allocator.free(backup);
                if (try self.seedSeq(path)) {
                    for (items) |text| self.appendToSeed(path, text) catch |err| {
                        try self.restoreSource(backup);
                        return err;
                    };
                    return;
                }
            }

            // ---- plan against the current parse (no mutation yet) ----
            // Current item kinds. These borrow `self.document`, so the plan must
            // be reduced to plain indices before the first edit reparses.
            var cur: std.ArrayList(AST.Node.Kind) = .empty;
            defer cur.deinit(self.allocator);
            {
                const parsed = try self.getParsed();
                const node = try parsed.ast.getValByPath(path);
                if (node.kind != .sequence) return error.NotASequence;
                var maybe = try parsed.ast.child(&node);
                while (maybe) |item| {
                    if (!isScalarKind(item.kind)) return error.UnsupportedShape;
                    try cur.append(self.allocator, item.kind);
                    maybe = parsed.ast.next(&item);
                }
            }
            if (cur.items.len == 0) return error.UnsupportedShape;

            // Target item kinds: parse each serialized value back to a scalar so
            // matching is by abstract value, not formatting (`1` != `'1'`). A
            // format whose scalar can't stand alone as a document (TOML) fails
            // the parse and is declined here.
            var tdocs: std.ArrayList(Document) = .empty;
            defer {
                for (tdocs.items) |d| d.deinit(self.allocator);
                tdocs.deinit(self.allocator);
            }
            var tgt: std.ArrayList(AST.Node.Kind) = .empty;
            defer tgt.deinit(self.allocator);
            for (items) |text| {
                var parser: Language.Parser = .{ .allocator = self.allocator };
                const d = Language.parse(&parser, text, self.format) catch return error.UnsupportedShape;
                const k = d.ast.node(d.ast.root).kind;
                if (!isScalarKind(k)) {
                    d.deinit(self.allocator);
                    return error.UnsupportedShape;
                }
                try tdocs.append(self.allocator, d);
                try tgt.append(self.allocator, k);
            }

            const m = cur.items.len;
            const t = tgt.items.len;

            // Occurrence index of element `i` = how many earlier elements share
            // its value. (kind, occ) is the per-item identity used for matching,
            // so duplicate values are paired up by their order of appearance.
            const occ = struct {
                fn at(kinds: []const AST.Node.Kind, i: usize) usize {
                    var c: usize = 0;
                    for (kinds[0..i]) |k| {
                        if (k.eql(kinds[i])) c += 1;
                    }
                    return c;
                }
            }.at;

            // A current item survives iff some target item shares its identity.
            const removed = try self.allocator.alloc(bool, m);
            defer self.allocator.free(removed);
            var removed_count: usize = 0;
            for (0..m) |i| {
                removed[i] = true;
                for (0..t) |j| {
                    if (cur.items[i].eql(tgt.items[j]) and occ(cur.items, i) == occ(tgt.items, j)) {
                        removed[i] = false;
                        break;
                    }
                }
                if (removed[i]) removed_count += 1;
            }

            // A target item is an addition iff no current item shares its identity.
            var additions: std.ArrayList(usize) = .empty;
            defer additions.deinit(self.allocator);
            for (0..t) |j| {
                var present = false;
                for (0..m) |i| {
                    if (cur.items[i].eql(tgt.items[j]) and occ(cur.items, i) == occ(tgt.items, j)) {
                        present = true;
                        break;
                    }
                }
                if (!present) try additions.append(self.allocator, j);
            }

            // The physical order after append+remove is survivors (old order)
            // then additions (target order). `slots[s]` says what sits at index
            // `s`: a kept current item or an appended target item.
            const Slot = union(enum) { keep: usize, add: usize };
            var slots: std.ArrayList(Slot) = .empty;
            defer slots.deinit(self.allocator);
            for (0..m) |i| {
                if (!removed[i]) try slots.append(self.allocator, .{ .keep = i });
            }
            for (additions.items) |j| try slots.append(self.allocator, .{ .add = j });

            // `order[k]` = the slot holding target item `k`, so a reorder by
            // `order` (a full permutation) lands the sequence in target order.
            const order = try self.allocator.alloc(usize, t);
            defer self.allocator.free(order);
            const used = try self.allocator.alloc(bool, slots.items.len);
            defer self.allocator.free(used);
            @memset(used, false);
            for (0..t) |k| {
                var found: ?usize = null;
                for (slots.items, 0..) |slot, s| {
                    if (used[s]) continue;
                    const hit = switch (slot) {
                        .keep => |i| cur.items[i].eql(tgt.items[k]) and occ(cur.items, i) == occ(tgt.items, k),
                        .add => |j| j == k,
                    };
                    if (hit) {
                        found = s;
                        break;
                    }
                }
                const s = found orelse return error.UnsupportedShape; // unreachable by construction
                order[k] = s;
                used[s] = true;
            }

            // No-op: same items, same order — leave the bytes untouched so a
            // redundant set never churns formatting.
            var needs_reorder = false;
            for (order, 0..) |o, k| {
                if (o != k) {
                    needs_reorder = true;
                    break;
                }
            }
            if (removed_count == 0 and additions.items.len == 0 and !needs_reorder) return;

            // ---- apply: append, remove, reorder — atomic across all steps ----
            const backup = try self.allocator.dupe(u8, self.source.items);
            defer self.allocator.free(backup);
            errdefer {
                // Capacity only grew during the edits, so the refill cannot fail;
                // `backup` parsed before, so the reparse cannot fail either.
                self.source.clearRetainingCapacity();
                self.source.appendSliceAssumeCapacity(backup);
                self.reparse() catch {};
            }

            // Append first so a full replacement never empties the block mid-edit
            // (an empty block sequence has no valid syntax). Appends land at the
            // tail, leaving the original items' indices valid for removal.
            for (additions.items) |j| try self.appendToSeq(path, items[j]);

            // Remove dropped originals high-index-first so lower indices stay put.
            var di: usize = m;
            while (di > 0) {
                di -= 1;
                if (removed[di]) try self.removeSeqItem(path, di);
            }

            if (needs_reorder) try self.reorderItems(path, order);
        }

        // ============
        // MOVE / REORDER
        // ============
        //
        // Like insert/delete, these never reserialize: they relocate whole entry
        // blocks (a mapping key's owned comment block + line(s), or a sequence
        // item's) and reuse `replaceAtSpan` to splice + reparse. The moved bytes
        // are the originals, so comments, quoting, and formatting ride along.
        // Block containers tile into per-entry blocks (trailing trivia rides with
        // the preceding entry); a flow sequence (`[a, b]`) reuses its original
        // separators so only the items move.

        /// Move the mapping entry named by `src_path` to sit immediately before
        /// the entry named by `dest_path`. Both paths must name keys in the
        /// *same* block mapping. The moved entry carries its owned leading
        /// comment block and any trailing same-line comment; the bytes between
        /// the two entries are preserved. Moving an entry to before itself (or
        /// into its own comment block) is a no-op.
        ///
        /// **Engine rule**: a SECTION node at either end is refused before any
        /// splice (`CannotMoveSection` / `CannotMoveSection` /
        /// `CannotMoveSection`). An entry's block is not always the region
        /// it owns: a `[header]` table's block is the header LINE, so moving
        /// it would relocate the name and strand the body, and moving anything
        /// else to sit before such a header lands it at the tail of the
        /// *preceding* table's body, silently reparenting it. `moveContainer`
        /// is the op that relocates a scattered container whole.
        ///
        /// Every move and reorder is held to what it promises (`readsBack`):
        /// the container's children read back as they were, in the order
        /// asked for, and nothing else changed, or `EditReadsBackWrong` and
        /// the source as it was.
        pub fn moveKey(self: *Self, src_path: []const AST.PathSegment, dest_path: []const AST.PathSegment) !void {
            var guard = try self.moveGuard(src_path, dest_path);
            defer if (guard) |*g| g.deinit(self.allocator);
            try self.spliceMoveKey(src_path, dest_path);
            if (guard) |*g| try self.readsBack(g);
        }

        /// `moveKey`'s guard: the entry at `src_path` lifted out of its
        /// mapping and set down before the one at `dest_path`. A `dest` that
        /// is not its sibling leaves the order as it was, which is what a
        /// move out of the mapping must not change. Null when either path
        /// names no entry, which the splice reports.
        fn moveGuard(self: *Self, src_path: []const AST.PathSegment, dest_path: []const AST.PathSegment) !?Guard {
            const parsed = try self.getParsed();
            const src = parsed.ast.getNodeByPath(src_path) catch return null;
            const dest = parsed.ast.getNodeByPath(dest_path) catch return null;
            const from = slotOf(parsed, src.id) orelse return null;
            const to = slotOf(parsed, dest.id) orelse return null;
            const before = if (to.container == from.container) to.at else from.at;
            const container = parsed.ast.node(from.container);
            const n = try seqLen(parsed, container);
            const order = try self.allocator.alloc(usize, n);
            var i: usize = 0;
            for (0..n) |j| {
                if (before != from.at and j == from.at) continue;
                if (before != from.at and j == before) {
                    order[i] = from.at;
                    i += 1;
                }
                order[i] = j;
                i += 1;
            }
            return try self.guardAt(parsed, container, .{ .moved = order });
        }

        fn spliceMoveKey(self: *Self, src_path: []const AST.PathSegment, dest_path: []const AST.PathSegment) !void {
            const parsed = try self.getParsed();
            const src = try parsed.ast.getNodeByPath(src_path);
            if (src.kind != .keyvalue) return error.NotAMapping;
            const dest = try parsed.ast.getNodeByPath(dest_path);
            if (dest.kind != .keyvalue) return error.NotAMapping;
            if (parsed.isSection(parsed.ast.node(src.kind.keyvalue.value)) or
                parsed.isSection(parsed.ast.node(dest.kind.keyvalue.value)))
                return self.refuse(.move);
            // A flow mapping's members are not one per line and are joined by
            // separators, the last having none: moving lines would strand a
            // comma or drop one. Move by member instead, as `reorderKeys` does.
            if (src_path.len > 0) {
                const parent = try parsed.ast.getValByPath(src_path[0 .. src_path.len - 1]);
                if (parent.kind == .mapping and self.isFlowNode(parsed, parent)) {
                    var entries: std.ArrayList(AST.Node) = .empty;
                    defer entries.deinit(self.allocator);
                    var src_idx: ?usize = null;
                    var dest_idx: ?usize = null;
                    var maybe = try parsed.ast.child(&parent);
                    while (maybe) |entry| : (maybe = parsed.ast.next(&entry)) {
                        if (entry.id == src.id) src_idx = entries.items.len;
                        if (entry.id == dest.id) dest_idx = entries.items.len;
                        try entries.append(self.allocator, entry);
                    }
                    const from = src_idx orelse return error.NotFound;
                    const before = dest_idx orelse return error.NotFound;
                    if (from == before) return;
                    // Every member in its place, with `from` lifted out and
                    // set down just before `before`.
                    var order: std.ArrayList(usize) = .empty;
                    defer order.deinit(self.allocator);
                    for (0..entries.items.len) |i| {
                        if (i == from) continue;
                        if (i == before) try order.append(self.allocator, from);
                        try order.append(self.allocator, i);
                    }
                    return self.reorderFlowEntries(parsed, entries.items, order.items, self.syntax().flow_entry_sep);
                }
            }
            const source = self.source.items;
            try self.moveBlock(
                entryBlockStart(source, parsed.span(src), self.syntax().comments),
                entryBlockEnd(source, parsed.span(src)),
                entryBlockStart(source, parsed.span(dest), self.syntax().comments),
            );
        }

        /// Move the sequence item at index `from` to index `to` (both positions
        /// in the current order; standard array-move semantics — the item is
        /// removed and reinserted, shifting the others to fill). A block item
        /// carries its owned leading comment block. No-op when `from == to`.
        pub fn moveItem(self: *Self, path: []const AST.PathSegment, from: usize, to: usize) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .sequence) return error.NotASequence;
            const n = try seqLen(parsed, node);
            if (from >= n or to >= n) return error.NotFound;
            if (from == to) return;
            // Build the post-move index order, then reorder by it.
            const order = try self.allocator.alloc(usize, n);
            defer self.allocator.free(order);
            for (order, 0..) |*o, i| o.* = i;
            const val = order[from];
            if (from < to) {
                var i = from;
                while (i < to) : (i += 1) order[i] = order[i + 1];
            } else {
                var i = from;
                while (i > to) : (i -= 1) order[i] = order[i - 1];
            }
            order[to] = val;
            var guard = try self.guardAt(parsed, node, .{ .moved = try self.allocator.dupe(usize, order) });
            defer guard.deinit(self.allocator);
            try self.reorderSeqNode(parsed, node, order);
            try self.readsBack(&guard);
        }

        /// Reorder the entries of the block mapping at `path` (empty path =
        /// root) so the keys listed in `keys` come first, in that order; entries
        /// whose key is not listed keep their original relative order and follow.
        /// Keys in `keys` that the mapping does not contain are ignored. Each
        /// entry's owned comments — and any interleaved blank lines / orphan
        /// comments, which ride with the entry that precedes them — are
        /// preserved, so no bytes are dropped. A flow mapping (`{…}`, every
        /// JSON object) takes `reorderFlowEntries` instead: its members move
        /// and each slot keeps its separator, so the last member never gains
        /// a comma it cannot have.
        ///
        /// **Engine rule**: once the new order is known and before any splice,
        /// an entry whose position actually changes and whose value is a
        /// SECTION node is refused (`CannotReorderSections` /
        /// `CannotReorderSections` / `CannotReorderSections`). `[header]`
        /// entries tile into blocks that stop at the *next* sibling's line —
        /// so the last entry's block excludes its own body, and any container
        /// that changes place strands or absorbs entries. `reorderContainers`
        /// is the op that reorders scattered containers whole. Reordering an
        /// inner mapping's scalar keys around a sub-table that stays put is
        /// unaffected, which is why only the MOVED entries are checked.
        pub fn reorderKeys(self: *Self, path: []const AST.PathSegment, keys: []const []const u8) !void {
            const parsed = try self.getParsed();
            const node = parsed.ast.getValByPath(path) catch return self.spliceKeyOrder(path, keys);
            if (node.kind != .mapping) return self.spliceKeyOrder(path, keys);
            // The order asked for: the listed keys first, each its first
            // entry not yet taken, then the rest as they stand.
            var listed: std.ArrayList(usize) = .empty;
            defer listed.deinit(self.allocator);
            const n = try seqLen(parsed, node);
            const taken = try self.allocator.alloc(bool, n);
            defer self.allocator.free(taken);
            @memset(taken, false);
            for (keys) |k| {
                var maybe = try parsed.ast.child(&node);
                var i: usize = 0;
                while (maybe) |entry| : ({
                    maybe = parsed.ast.next(&entry);
                    i += 1;
                }) {
                    if (!taken[i] and entry.kind == .keyvalue and keyNodeIs(parsed, entry.kind.keyvalue.key, k)) {
                        taken[i] = true;
                        try listed.append(self.allocator, i);
                        break;
                    }
                }
            }
            var guard = try self.guardAt(parsed, node, .{ .moved = try fullOrder(self.allocator, listed.items, n) });
            defer guard.deinit(self.allocator);
            try self.spliceKeyOrder(path, keys);
            try self.readsBack(&guard);
        }

        fn spliceKeyOrder(self: *Self, path: []const AST.PathSegment, keys: []const []const u8) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .mapping) return error.NotAMapping;
            const first_id = node.kind.mapping orelse return; // empty mapping
            const source = self.source.items;

            // Gather each entry and its key (for matching), in document order.
            var entry_keys: std.ArrayList([]const u8) = .empty;
            defer entry_keys.deinit(self.allocator);
            var entries: std.ArrayList(AST.Node) = .empty;
            defer entries.deinit(self.allocator);

            var cur = parsed.ast.node(first_id);
            while (true) {
                if (cur.kind != .keyvalue) return error.InvalidDocument;
                const key_node = parsed.ast.node(cur.kind.keyvalue.key);
                const key = switch (key_node.kind) {
                    .string => |s| s,
                    else => return error.InvalidDocument,
                };
                try entry_keys.append(self.allocator, key);
                try entries.append(self.allocator, cur);
                cur = parsed.ast.next(&cur) orelse break;
            }

            // Translate the requested keys into entry indices (first unused match
            // wins), then reorder the blocks by that index list.
            var order: std.ArrayList(usize) = .empty;
            defer order.deinit(self.allocator);
            const chosen = try self.allocator.alloc(bool, entries.items.len);
            defer self.allocator.free(chosen);
            @memset(chosen, false);
            for (keys) |k| {
                for (entry_keys.items, 0..) |seen, i| {
                    if (!chosen[i] and std.mem.eql(u8, seen, k)) {
                        try order.append(self.allocator, i);
                        chosen[i] = true;
                        break;
                    }
                }
            }
            // A flow mapping holds no section, so the veto below has nothing
            // to refuse there; its members are spliced comma-aware.
            if (self.isFlowNode(parsed, node)) return self.reorderFlowEntries(parsed, entries.items, order.items, self.syntax().flow_entry_sep);
            // The engine's veto, before anything is spliced — over exactly the
            // entries whose position this reorder changes, since an entry left
            // where it was is never at risk. See the rule note above. Skipped
            // at comptime for a format with no sections, where nothing could
            // be refused.
            if (comptime is_section_format) {
                // Where each entry ends up: the listed keys first, in `order`,
                // then the unlisted ones in their original relative order.
                const final_pos = try self.allocator.alloc(usize, entries.items.len);
                defer self.allocator.free(final_pos);
                var pos: usize = 0;
                for (order.items) |i| {
                    final_pos[i] = pos;
                    pos += 1;
                }
                for (chosen, 0..) |c, i| if (!c) {
                    final_pos[i] = pos;
                    pos += 1;
                };
                var entry = parsed.ast.node(first_id);
                var idx: usize = 0;
                while (true) : (idx += 1) {
                    if (final_pos[idx] != idx and parsed.isSection(parsed.ast.node(entry.kind.keyvalue.value)))
                        return self.refuse(.reorder);
                    entry = parsed.ast.next(&entry) orelse break;
                }
            }
            var blocks: std.ArrayList(Block) = .empty;
            defer blocks.deinit(self.allocator);
            for (entries.items) |entry| {
                try blocks.append(self.allocator, .{ .start = entryBlockStart(source, parsed.span(entry), self.syntax().comments), .end = 0 });
            }
            const last_end = entryBlockEnd(source, parsed.span(entries.items[entries.items.len - 1]));
            tileBlocks(blocks.items, last_end);
            try self.reorderBlocks(blocks.items[0].start, last_end, blocks.items, order.items);
        }

        /// Reorder the items of the sequence at `path` (block or flow) so the
        /// items at the indices listed in `indices` (positions in the current
        /// order) come first, in that order; items not listed keep their
        /// original relative order and follow. Out-of-range indices are ignored.
        /// Block items carry their owned comments; a flow sequence keeps its
        /// original separators so only the items move.
        pub fn reorderItems(self: *Self, path: []const AST.PathSegment, indices: []const usize) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .sequence) return error.NotASequence;
            var guard = try self.guardAt(parsed, node, .{ .moved = try fullOrder(self.allocator, indices, try seqLen(parsed, node)) });
            defer guard.deinit(self.allocator);
            try self.reorderSeqNode(parsed, node, indices);
            try self.readsBack(&guard);
        }

        // --- move / reorder internals ---

        /// Reorder a sequence node's items by `order` (bring-to-front indices),
        /// dispatching on flow vs block style.
        fn reorderSeqNode(self: *Self, parsed: Document, node: AST.Node, order: []const usize) !void {
            const source = self.source.items;
            var spans: std.ArrayList(Span) = .empty;
            defer spans.deinit(self.allocator);
            // Each item's block starts on its MARKER's line (see `markerStart`),
            // which for a nested or empty item is above its span's.
            var starts: std.ArrayList(usize) = .empty;
            defer starts.deinit(self.allocator);
            var maybe = try parsed.ast.child(&node);
            while (maybe) |item| {
                try spans.append(self.allocator, parsed.span(item));
                try starts.append(self.allocator, markerStart(parsed, item));
                maybe = parsed.ast.next(&item);
            }
            if (spans.items.len == 0) return;
            // A sequence the parser gave a separator of its own moves its
            // items as a flow mapping moves its members, so a `.tune` block
            // takes its comments with it; a sequence with the dialect's
            // separator keeps its slots' trivia where they are.
            const joint = self.jointOf(parsed, node);
            if (joint.declared) {
                var items: std.ArrayList(AST.Node) = .empty;
                defer items.deinit(self.allocator);
                var it = try parsed.ast.child(&node);
                while (it) |item| : (it = parsed.ast.next(&item)) try items.append(self.allocator, item);
                return self.reorderFlowEntries(parsed, items.items, order, joint.token);
            }
            if (self.isFlowNode(parsed, node)) {
                try self.reorderFlowItems(spans.items, order);
                return;
            }
            if (!self.syntax().block_seq_editable) return error.NotAnInlineArray;
            var blocks: std.ArrayList(Block) = .empty;
            defer blocks.deinit(self.allocator);
            for (starts.items) |at| {
                try blocks.append(self.allocator, .{ .start = commentBlockStart(source, lineStartBefore(source, at), self.syntax().comments), .end = 0 });
            }
            const last_end = entryBlockEnd(source, spans.items[spans.items.len - 1]);
            tileBlocks(blocks.items, last_end);
            const perm = try fullOrder(self.allocator, order, blocks.items.len);
            defer self.allocator.free(perm);
            var arena: std.heap.ArenaAllocator = .init(self.allocator);
            defer arena.deinit();
            const edits = try self.reorderEdits(arena.allocator(), parsed, node, perm);
            if (edits.len == 0) return self.reorderBlocks(blocks.items[0].start, last_end, blocks.items, perm);
            // The whole source is rewritten, since an item may be addressed
            // on a line outside the blocks (figl's `ys[0]` written again
            // after `ys[1]`).
            var region: std.ArrayList(u8) = .empty;
            defer region.deinit(self.allocator);
            var block: std.ArrayList(u8) = .empty;
            defer block.deinit(self.allocator);
            for (perm) |i| {
                block.clearRetainingCapacity();
                try appendEdited(&block, self.allocator, source, blocks.items[i].start, blocks.items[i].end, edits);
                try appendBlockSep(&region, self.allocator, block.items, self.syntax());
            }
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            try appendEdited(&out, self.allocator, source, 0, blocks.items[0].start, edits);
            try out.appendSlice(self.allocator, region.items);
            try appendEdited(&out, self.allocator, source, last_end, source.len, edits);
            try self.replaceAtSpan(Span.init(0, source.len), out.items);
        }

        /// What keeps each item of the block sequence `seq` addressing
        /// itself once its items stand in the order `perm` (`perm[k]` the
        /// item that goes to position `k`), as rewrites in source order,
        /// their text in `arena`; `followingEdits` is the delete's. Every
        /// other format records no item path and gets none.
        ///
        /// An item addressed by its index takes its new one at each `index`
        /// mention: figl's index is a position (spec § 7.4), and the lines
        /// saying `xs[1]` move with the item now at 0. A line that appends
        /// an item (figl's `a[]`, or the `+` that repeats it, spec § 8) is
        /// how the position it stands in is written, so the item that comes
        /// to stand there is written with it: the first item of a `+` chain
        /// carries the header and the rest a `+`, whichever item is first.
        /// The item's trailing comment stays with it, in its column.
        fn reorderEdits(self: *const Self, arena: std.mem.Allocator, parsed: Document, seq: AST.Node, perm: []const usize) ![]PathEdit {
            const source = self.source.items;
            var items: std.ArrayList(AST.Node) = .empty;
            var it = try parsed.ast.child(&seq);
            while (it) |item| : (it = parsed.ast.next(&item)) try items.append(arena, item);
            if (items.items.len != perm.len) return &.{};
            // Each item's one `append` mention, by its place in the source.
            const appended = try arena.alloc(?Span, perm.len);
            for (items.items, appended) |item, *a| {
                a.* = null;
                var count: usize = 0;
                var mentions = parsed.mentionsOf(item.id);
                while (mentions.next()) |m| if (m.kind == .append) {
                    a.* = m.span;
                    count += 1;
                };
                if (count > 1) a.* = null;
            }
            var edits: std.ArrayList(PathEdit) = .empty;
            for (perm, 0..) |from, to| {
                if (from == to) continue;
                var mentions = parsed.mentionsOf(items.items[from].id);
                while (mentions.next()) |m| if (m.kind == .index)
                    try edits.append(arena, .{ .span = m.span, .text = try std.fmt.allocPrint(arena, "{d}", .{to}) });
                const own = appended[from] orelse continue;
                const slot = appended[to] orelse continue;
                const text = source[slot.start..slot.end];
                if (!std.mem.eql(u8, source[own.start..own.end], text))
                    try edits.append(arena, try padded(arena, source, own, text));
            }
            std.mem.sortUnstable(PathEdit, edits.items, {}, struct {
                fn lessThan(_: void, a: PathEdit, b: PathEdit) bool {
                    return a.span.start < b.span.start;
                }
            }.lessThan);
            return edits.items;
        }

        /// `source[from..to]` appended to `out`, with each of `edits`
        /// (ascending) that lies inside it written over its span.
        fn appendEdited(out: *std.ArrayList(u8), allocator: std.mem.Allocator, source: []const u8, from: usize, to: usize, edits: []const PathEdit) !void {
            var pos = from;
            for (edits) |edit| {
                if (edit.span.start < pos or edit.span.end > to) continue;
                try out.appendSlice(allocator, source[pos..edit.span.start]);
                try out.appendSlice(allocator, edit.text);
                pos = edit.span.end;
            }
            try out.appendSlice(allocator, source[pos..to]);
        }

        /// Splice a block container's region so the entries indexed by `order`
        /// (in document order) come first, then the rest in original order.
        /// `blocks` must be in document order and tile `[region_start, region_end)`.
        fn reorderBlocks(self: *Self, region_start: usize, region_end: usize, blocks: []const Block, order: []const usize) !void {
            const perm = try fullOrder(self.allocator, order, blocks.len);
            defer self.allocator.free(perm);
            const source = self.source.items;
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            for (perm) |i| try appendBlockSep(&out, self.allocator, source[blocks[i].start..blocks[i].end], self.syntax());
            try self.replaceAtSpan(Span.init(region_start, region_end), out.items);
        }

        /// Splice a flow sequence (`[a, b, …]`) so its items follow `order`,
        /// reusing each slot's original separator bytes so the comma/space
        /// framing is preserved while only the item contents move.
        fn reorderFlowItems(self: *Self, items: []const Span, order: []const usize) !void {
            const perm = try fullOrder(self.allocator, order, items.len);
            defer self.allocator.free(perm);
            const source = self.source.items;
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            for (perm, 0..) |src_idx, slot| {
                try out.appendSlice(self.allocator, source[items[src_idx].start..items[src_idx].end]);
                // Reuse the separator that originally sat after position `slot`.
                if (slot + 1 < perm.len) {
                    try out.appendSlice(self.allocator, source[items[slot].end..items[slot + 1].start]);
                }
            }
            try self.replaceAtSpan(Span.init(items[0].start, items[items.len - 1].end), out.items);
        }

        /// Splice a flow mapping's members (`entries`, in document order) so
        /// they follow `order` (bring-to-front indices, as `reorderBlocks`).
        /// Also a separated sequence's items: `sep` is the token between
        /// two of them (`flow_entry_sep`, or the sequence's own).
        ///
        /// Each slot keeps the separator bytes that followed it, as a flow
        /// sequence's do in `reorderFlowItems`, so the comma count and the
        /// layout stay where they were: a member moved off the end gains the
        /// comma of the slot it lands in, and the one moved to the end has
        /// none. A member's own comments travel with it: the comment block on
        /// the lines above it, when it begins its line (the block `deleteKey`
        /// carries), and the comment that closes its line after its comma.
        /// The second only when every member starts a line of its own — in a
        /// packed `{ a: 1, b: 2 }` a moved line comment would swallow what
        /// follows it, so there each slot keeps whatever trivia it holds.
        fn reorderFlowEntries(self: *Self, parsed: Document, entries: []const AST.Node, order: []const usize, sep: []const u8) !void {
            const n = entries.len;
            const perm = try fullOrder(self.allocator, order, n);
            defer self.allocator.free(perm);
            const source = self.source.items;
            const has_comments = self.lineCommentMarker() != null;

            // `body` is the member with its leading comment block; `trail` the
            // comment closing its line, with the spaces before it (empty when
            // it has none).
            const Member = struct { body: Span, trail: Span };
            const members = try self.allocator.alloc(Member, n);
            defer self.allocator.free(members);
            var prev_end: usize = 0;
            for (entries, members) |entry, *m| {
                const span = parsed.span(entry);
                // Back over a key sigil the span leaves out (ZON's `.`), as
                // `deleteKey` does, so `.name` moves as a unit.
                var start = if (self.syntax().key_sigil) |sigil|
                    if (entry.kind == .keyvalue and span.start > 0 and source[span.start - 1] == sigil) span.start - 1 else span.start
                else
                    span.start;
                if (has_comments) {
                    const line_start = lineStartBefore(source, start);
                    if (firstNonSpace(source, line_start) == start) {
                        const cbs = commentBlockStart(source, line_start, self.syntax().comments);
                        const lead = firstNonSpace(source, cbs);
                        if (cbs < line_start and lead >= prev_end) start = lead;
                    }
                }
                m.* = .{ .body = Span.init(start, span.end), .trail = self.lineEndComment(span.end, sep) orelse Span.init(span.end, span.end) };
                prev_end = @max(m.body.end, m.trail.end);
            }
            // Trails ride only when every gap between members crosses a line.
            for (members[0 .. n - 1], members[1..]) |m, next| {
                const gap_from = @max(m.body.end, m.trail.end);
                if (std.mem.indexOfScalar(u8, source[gap_from..next.body.start], '\n') == null) {
                    for (members) |*each| each.trail = Span.init(each.body.end, each.body.end);
                    break;
                }
            }

            // The region runs from the first member to the end of the last
            // member's trail, or else of the trailing separator a JSON5 or
            // TOML 1.1 member may carry, so the slot keeps it and a trail
            // landing there goes after it. Past that (the closer) is untouched.
            const last = members[n - 1];
            const region_end = if (last.trail.len() > 0) last.trail.end else blk: {
                const i = firstNonSpace(source, last.body.end);
                break :blk if (sep.len > 0 and std.mem.startsWith(u8, source[i..], sep)) i + sep.len else last.body.end;
            };
            const region = Span.init(members[0].body.start, region_end);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            for (perm, 0..) |src_idx, slot| {
                const at = members[slot];
                const moved = members[src_idx];
                const gap_end = if (slot + 1 < n) members[slot + 1].body.start else region.end;
                // Split the slot's own gap where its trail sat, or else at the
                // end of its line: before the split is the separator, after it
                // the line break and the next member's indent.
                var pre: []const u8 = undefined;
                var post: []const u8 = undefined;
                if (at.trail.len() > 0) {
                    pre = source[at.body.end..at.trail.start];
                    post = source[at.trail.end..gap_end];
                } else {
                    const gap = source[at.body.end..gap_end];
                    var split = std.mem.indexOfScalar(u8, gap, '\n') orelse gap.len;
                    if (split > 0 and gap[split - 1] == '\r') split -= 1;
                    pre = gap[0..split];
                    post = gap[split..];
                }
                try out.appendSlice(self.allocator, source[moved.body.start..moved.body.end]);
                if (moved.trail.len() > 0) {
                    try out.appendSlice(self.allocator, std.mem.trimEnd(u8, pre, " \t"));
                    try out.appendSlice(self.allocator, source[moved.trail.start..moved.trail.end]);
                } else {
                    try out.appendSlice(self.allocator, pre);
                }
                try out.appendSlice(self.allocator, post);
            }
            try self.replaceAtSpan(region, out.items);
        }

        /// Move the block `[src_start, src_end)` so it begins at `dest_start`,
        /// preserving the bytes between source and destination. No-op when the
        /// destination falls within the source block.
        fn moveBlock(self: *Self, src_start: usize, src_end: usize, dest_start: usize) !void {
            if (dest_start >= src_start and dest_start <= src_end) return;
            const source = self.source.items;
            const moved = source[src_start..src_end];
            const syn = self.syntax();
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            if (src_end <= dest_start) {
                // src precedes dest: [src][between][dest..] -> [between][src][dest..]
                try appendBlockSep(&out, self.allocator, source[src_end..dest_start], syn);
                try appendBlockSep(&out, self.allocator, moved, syn);
                // `dest` is not spliced, but now follows `src`'s last line.
                try out.appendSlice(self.allocator, continuationEnd(out.items, source[dest_start..], syn));
                try self.replaceAtSpan(Span.init(src_start, dest_start), out.items);
            } else {
                // dest precedes src: [dest..][between][src] -> [src][dest..][between]
                try appendBlockSep(&out, self.allocator, moved, syn);
                try appendBlockSep(&out, self.allocator, source[dest_start..src_start], syn);
                try self.replaceAtSpan(Span.init(dest_start, src_end), out.items);
            }
        }

        // --- insert helpers (build text, then splice) ---

        /// Insert `key_text<kv_sep>value_text` as a new entry in the block
        /// (non-flow) mapping `mapping`, after its last existing entry (or,
        /// if it has none, at a language-appropriate fallback point — see
        /// below). `pub` so `languages/ini/tests/editor.zig`'s `iniInsertKey` (INI's
        /// `isFlow`-bypassing `insertKey`) can reuse it directly rather than
        /// duplicate it.
        pub fn insertBlockKey(self: *Self, parsed: Document, mapping: AST.Node, key_text: []const u8, value_text: []const u8) !void {
            const source = self.source.items;
            // Every currently-supported block-mapping language represents an
            // empty mapping as `.null_` (promoted via `promoteNullToMapping`),
            // never as a childless `.mapping` — so `lastChild`/`firstChildKey`
            // have always found a real entry to anchor on. dotenv/.properties
            // break that assumption: their root is unconditionally `.mapping`
            // even for a totally empty (or comment-only) file, so the very
            // first `insertKey` into a fresh file lands here with zero
            // children. Fall back to column 0 and the mapping's own span end
            // (its whole-file span for the flat formats' root) rather than
            // unwrapping a null.
            // The children written on this mapping's OWN lines. A child
            // reached through a header line of its own (`[a.b]` under `[a]`,
            // a `[[x]]` element) sits outside the mapping's region, and an
            // entry spliced after it would be silently reparented; a dotted
            // child (`b.c = 1`) or a fig header at the parent's depth is on
            // the parent's lines and anchors the insert like any scalar. See
            // `Document.mentionsOf`.
            var maybe_last: ?AST.Node = null;
            var first_key: ?AST.Node = null;
            var skipped = false;
            var cur = try parsed.ast.child(&mapping);
            while (cur) |kv| : (cur = parsed.ast.next(&kv)) {
                if (self.outOfRegion(parsed, kv)) {
                    skipped = true;
                    continue;
                }
                if (first_key == null) first_key = if (kv.kind == .keyvalue) parsed.ast.node(kv.kind.keyvalue.key) else kv;
                maybe_last = kv;
            }
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            var val_buf: std.ArrayList(u8) = .empty;
            defer val_buf.deinit(self.allocator);
            const place = placeOf(parsed, mapping);
            const rendered = try self.renderedValue(&val_buf, value_text, place);
            // The new entry copies the first in-region key's line prefix (see
            // `indentAt`); a mapping with no such key starts at column 0.
            var indent_buf: std.ArrayList(u8) = .empty;
            defer indent_buf.deinit(self.allocator);
            const indent: []const u8 = if (first_key) |key_node|
                try self.indentAt(&indent_buf, parsed.span(key_node).start)
            else
                "";
            const insert_at = if (maybe_last) |last|
                lineEndAfter(source, parsed.span(last).end -| 1)
            else if (self.syntax().closed_containers) |closed| {
                // A childless closed container (`<dict/>`, root or nested):
                // rewrite it in its multi-line form around the new entry.
                const span = parsed.span(mapping);
                var base_buf: std.ArrayList(u8) = .empty;
                defer base_buf.deinit(self.allocator);
                const base = try self.indentAt(&base_buf, span.start);
                var child: std.ArrayList(u8) = .empty;
                defer child.deinit(self.allocator);
                try child.appendSlice(self.allocator, base);
                try child.appendSlice(self.allocator, self.syntax().indent_unit);
                try self.writeEntry(&out, child.items, key_text, rendered, place);
                return self.expandEmptyContainer(span, closed.map, base, out.items);
            } else if (mapping.id == parsed.ast.root)
                // A root with every child under a header of its own (a
                // header-first TOML or INI file) takes its first own entry at
                // the top, above the first header; an empty (or
                // comments-only) document takes it at the end of whatever is
                // there. For the flat formats the root's span is the whole
                // input anyway; for fig it is not (a comments-only file has a
                // zero-width root).
                (if (skipped) 0 else source.len)
            else if (parsed.isSection(mapping))
                // A SECTION with no entry on its own lines (INI's empty
                // `[section]` with nothing under it yet): its span is
                // anchored at just the header's name token (see
                // `ini/parser.zig`'s `parseSectionHeader`), not the section's
                // body extent — splicing at `.end` would land inside the
                // `[section]` line itself. Anchor on the end of a header
                // LINE of its own instead.
                try self.ownHeaderLineEnd(parsed, mapping)
            else
                // A childless block mapping with no closing token has no line
                // to splice after and no spelling for its first entry
                // (NestedText's inline `{}` reached as a block target).
                return error.EmptyInlineContainer;

            // A container that closes on its last entry's line (plist's
            // `<key>o</key><dict>…</dict>`, OpenStep's `o = {a = 1; };`)
            // has no line of its own to append after: the line end is past
            // its close, and an entry there lands in the enclosing mapping.
            // See `closesOnLastLine`.
            if (self.syntax().closed_containers) |closed| if (maybe_last) |last| {
                if (closesOnLastLine(source, parsed.span(mapping), parsed.span(last), closed.map.close, insert_at))
                    return error.ContainerClosesOnItsLine;
            };

            if (insert_at > 0 and source[insert_at - 1] != '\n') try out.append(self.allocator, '\n');
            try out.appendSlice(self.allocator, indent);
            try self.writeEntry(&out, indent, key_text, rendered, place);
            try out.append(self.allocator, '\n');
            try self.replaceAtSpan(Span.init(insert_at, insert_at), out.items);
        }

        // --- Whole-container structural editing (section formats) ---
        //
        // The ops for a format whose logical containers are SCATTERED through
        // the source (TOML's `[table]` headers, fig's `>` marker runs and
        // re-entered headers, INI's reopened `[section]`): the generic
        // line-splice ops above have no counterpart for them, because there is
        // no single `[min,max)` range to splice. What there is instead is a
        // DERIVED region set — `editor/regions.zig`'s `gather` — built from
        // the one fact the parser records that spans cannot carry: each
        // section node's header lines (`Document.regionsOf`). Everything
        // that only needs "where are this node's regions" is generic here, for
        // every format whose `syntax().section_noun` is non-null:
        //
        //   * `deleteContainer`, `moveContainer`, `reorderContainers` — the
        //     gather plus the splice-out / relocate / reorder machinery in
        //     `editor/regions.zig`, no format code at all;
        //   * the four line-splice refusals above (`deleteKey`, `moveKey`,
        //     `reorderKeys`, `replaceValAtPath`), which are ONE rule — a
        //     section node cannot be line-spliced; use the container op —
        //     spelled in the format's own vocabulary through `refuse`.
        //
        // The other three used to be hooks, because each had to SPELL a
        // fragment or find a name. `insertContainer` and
        // `appendContainerToSeq` write a new `[header]` line, which
        // `Syntax.section_header` now declares; `renameContainer` rewrites
        // every place a table's name is spelled (its headers AND its dotted
        // lines), which `Document.mentionsOf` now records. Both are
        // generic below, with a comptime refusal for a format that declares
        // no header syntax. See `docs/proposals/derived-regions.md` and
        // `docs/proposals/runtime-languages.md` §4.4.
        //
        // The methods exist for every format either way — Zig has no
        // conditional container-level declarations since `usingnamespace`
        // went away in 0.15 (see the language-interface proposal's §8.1) —
        // so a `@compileError` remains the answer for a format that is not a
        // section format. The vocabulary is the operation's, not any one
        // format's: TOML's `[table]`, fig's block container and INI's
        // `[section]` are the same thing here, and so are their errors
        // (`NotASection`, `CannotDeleteSection`, …).

        /// Whether this format has section nodes at all — a non-null
        /// `section_noun` in any dialect. Comptime, so the generic ops and the
        /// line-splice rule can be compiled out for every other format.
        ///
        /// A runtime language (`contract/runtime.zig`) has no comptime
        /// dialect table; it declares `runtime` and the answer is "may be",
        /// with `syntax()` deciding per entry at the call.
        pub const is_section_format = @hasDecl(Language, "runtime") or blk: {
            var any = false;
            for (std.meta.tags(Language.Type)) |t| {
                if (Language.syntax(t).section_noun != null) any = true;
            }
            break :blk any;
        };

        /// The line-splice ops the section rule refuses.
        const SectionOp = enum { delete, replace, move, reorder, not_a_section, exists };

        /// The engine's refusal of `op` on a section. Returns an error VALUE
        /// (not a union) so a caller writes `return self.refuse(.delete)`.
        /// The errors are the operation's, not any one format's: TOML's
        /// `[table]`, figl's block container and INI's `[section]` are one
        /// thing here, and a format's own word for it is its declared
        /// `section_noun`, for a consumer's message.
        fn refuse(self: *const Self, comptime op: SectionOp) error{
            NotASection,
            CannotDeleteSection,
            CannotReplaceSection,
            CannotMoveSection,
            CannotReorderSections,
            SectionExists,
        } {
            _ = self;
            return switch (op) {
                .not_a_section => error.NotASection,
                .delete => error.CannotDeleteSection,
                .replace => error.CannotReplaceSection,
                .move => error.CannotMoveSection,
                .reorder => error.CannotReorderSections,
                .exists => error.SectionExists,
            };
        }

        /// The section node at `path`, or the format's "not a section"
        /// refusal (`NotASection` / `NotAContainer`) — for a scalar, a flow
        /// container, the root, or a path that resolves to no section.
        fn sectionAt(self: *const Self, parsed: Document, path: []const AST.PathSegment) !AST.Node {
            const node = try parsed.ast.getValByPath(path);
            if (!parsed.isSection(node)) return self.refuse(.not_a_section);
            return node;
        }

        /// The coalesced region set of the section node `node`: its header
        /// lines (creating line and every re-entry, each with its owned
        /// comment block) plus every line of its subtree, in one ascending,
        /// disjoint list the caller owns. `merge_touching = false` keeps two
        /// regions that meet exactly apart, for a caller that addresses each
        /// region's own start.
        pub fn gatherRegions(self: *const Self, parsed: Document, node: AST.Node, merge_touching: bool) !std.ArrayList(Region) {
            return regions.gatherNormalized(parsed, self.source.items, self.allocator, node, self.syntax().comments, merge_touching);
        }

        /// Byte offset just past the last line of the section node `node`'s
        /// subtree — where a new sibling section can be spliced without
        /// splitting it. `insertContainer` and `appendContainerToSeq` place a
        /// header there.
        pub fn sectionExtentEnd(self: *const Self, parsed: Document, node: AST.Node) !usize {
            return regions.extentEnd(parsed, self.source.items, self.allocator, node, self.syntax().comments);
        }

        /// Delete the whole container named by `path` — every scattered region
        /// of its subtree, leaving interleaved foreign content untouched. For
        /// TOML that is a table, array-of-tables, or single AoT element; for
        /// fig a block container (a path may end in an index, deleting one
        /// sequence element entire); for INI a `[section]`, reopened
        /// occurrences included. A scalar or flow-valued target is refused
        /// (`NotASection` / `NotAContainer`): `deleteKey`/`removeSeqItem` is the
        /// op for those.
        pub fn deleteContainer(self: *Self, path: []const AST.PathSegment) !void {
            comptime requireSectionFormat("deleteContainer");
            const parsed = try self.getParsed();
            const node = try self.sectionAt(parsed, path);
            // A list's only item leaves the list, emptied, as
            // `removeSeqItem` leaves it; the items after any other keep
            // what addresses them (`followingEdits`).
            var arena: std.heap.ArenaAllocator = .init(self.allocator);
            defer arena.deinit();
            var edits: []PathEdit = &.{};
            if (parsed.ast.parentOf(node.id)) |p| {
                const seq = parsed.ast.node(p);
                if (seq.kind == .sequence and seq.kind.sequence == node.id and parsed.ast.next(&node) == null) {
                    if (try self.emptiedSectionSeq(parsed, seq)) return;
                    if (try self.emptiedHeaderSeq(parsed, seq, path[0 .. path.len - 1])) return;
                }
                if (seq.kind == .sequence) {
                    var at: usize = 0;
                    var sib = try parsed.ast.child(&seq);
                    while (sib) |c| : (sib = parsed.ast.next(&c)) {
                        if (c.id == node.id) break;
                        at += 1;
                    }
                    edits = try self.followingEdits(arena.allocator(), parsed, node, at);
                }
            }
            // A table made by dotted keys inside a flow mapping is written
            // by entries of that mapping, not by lines: its regions are the
            // whole line the flow mapping stands on. Its entries go, each
            // with a separator, and the delete is held to what one promises.
            if (self.dottedInFlow(parsed, node)) |owner| {
                var entries: std.ArrayList(Span) = .empty;
                defer entries.deinit(self.allocator);
                try self.dottedEntrySpans(parsed, owner, node, &entries);
                std.mem.sort(Span, entries.items, {}, struct {
                    fn later(_: void, a: Span, b: Span) bool {
                        return a.start > b.start;
                    }
                }.later);
                const first = flowFirstEntryStart(parsed, owner.flow);
                const removed = self.entryIndex(path);
                const backup = try self.allocator.dupe(u8, self.source.items);
                defer self.allocator.free(backup);
                // Last first, so the spans before each splice still stand.
                for (entries.items) |entry| self.removeFlowEntry(entry, entry.start <= first) catch |err| {
                    try self.restoreSource(backup);
                    return err;
                };
                if (removed) |at| try self.keepOthers(backup, path[0 .. path.len - 1], at);
                return;
            }
            var used = try self.gatherRegions(parsed, node, true);
            defer used.deinit(self.allocator);
            try self.spliceOutEdited(used.items, edits);
        }

        /// Splice `rendered` over every one of `mentions` (sorted by start,
        /// as `Document.mentionsOf` returns them) in one whole-document
        /// rewrite, so a rename either lands everywhere or rolls back.
        fn rewriteMentions(self: *Self, mentions_: Document.MentionIterator, rendered: []const u8) !void {
            const source = self.source.items;
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            var pos: usize = 0;
            var mentions = mentions_;
            while (mentions.next()) |m| {
                if (!m.kind.isName()) continue; // an item's path, not a name
                if (m.span.start < pos) continue; // the same token reached twice
                try out.appendSlice(self.allocator, source[pos..m.span.start]);
                try out.appendSlice(self.allocator, rendered);
                pos = m.span.end;
            }
            try out.appendSlice(self.allocator, source[pos..]);
            try self.replaceAtSpan(Span.init(0, source.len), out.items);
        }

        /// Render `path` as a header's dotted path: each key through
        /// `formatInsertKey`, joined by the header's `sep`, index segments
        /// left out when the header says so.
        fn writeHeaderPath(self: *Self, out: *std.ArrayList(u8), hdr: lang.SectionHeader, path: []const AST.PathSegment) !void {
            var first = true;
            for (path) |seg| switch (seg) {
                .index => |i| if (!hdr.skip_index) {
                    if (!first) try out.appendSlice(self.allocator, hdr.sep);
                    first = false;
                    try out.print(self.allocator, "{d}", .{i});
                },
                .key => |k| {
                    if (!first) try out.appendSlice(self.allocator, hdr.sep);
                    first = false;
                    const rendered = try self.formatInsertKey(k);
                    defer self.allocator.free(rendered);
                    try out.appendSlice(self.allocator, rendered);
                },
            };
        }

        /// Splice a new header line (`open` + path + `close`) and `body_text`
        /// (verbatim entry lines, possibly empty) at `insert_at`, on a line
        /// of its own with a blank line before it when it does not open the
        /// file.
        fn spliceHeader(self: *Self, insert_at: usize, open: []const u8, close: []const u8, path: []const AST.PathSegment, body_text: []const u8) !void {
            const hdr = self.syntax().section_header.?;
            const source = self.source.items;
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            if (insert_at > 0 and source[insert_at - 1] != '\n') try out.append(self.allocator, '\n');
            if (insert_at > 0) try out.append(self.allocator, '\n');
            try out.appendSlice(self.allocator, open);
            try self.writeHeaderPath(&out, hdr, path);
            try out.appendSlice(self.allocator, close);
            try out.append(self.allocator, '\n');
            if (body_text.len > 0) {
                try out.appendSlice(self.allocator, body_text);
                if (body_text[body_text.len - 1] != '\n') try out.append(self.allocator, '\n');
            }
            try self.replaceAtSpan(Span.init(insert_at, insert_at), out.items);
        }

        /// Create a new container at `path` whose body is `body_text` (verbatim
        /// entry lines, possibly empty): a header line spelled per
        /// `Syntax.section_header`, spliced past the parent's whole derived
        /// extent — or at end-of-file for a root-level container — so no
        /// existing key is reparented. Refuses an existing target in the
        /// format's vocabulary (`SectionExists`). Comptime-refused for a format
        /// with no header syntax.
        pub fn insertContainer(self: *Self, path: []const AST.PathSegment, body_text: []const u8) !void {
            comptime requireHeaderSyntax("insertContainer");
            if (path.len == 0) return self.refuse(.not_a_section);
            const parsed = try self.getParsed();
            if (parsed.ast.getValByPath(path)) |_| {
                return self.refuse(.exists);
            } else |_| {}
            const hdr = self.syntax().section_header.?;
            const insert_at = blk: {
                if (path.len > 1) {
                    if (parsed.ast.getValByPath(path[0 .. path.len - 1])) |parent| {
                        if (parent.kind == .mapping) break :blk try self.sectionExtentEnd(parsed, parent);
                    } else |_| {}
                }
                break :blk self.source.items.len;
            };
            try self.spliceHeader(insert_at, hdr.open, hdr.close, path, body_text);
        }

        /// Rename the leaf segment of the container at `path` to the logical
        /// key `new_leaf`, spelled by `formatInsertKey` and written over every
        /// place the format spells the name (`Document.mentionsOf`): the
        /// container's own header, every descendant header sharing the
        /// prefix, every dotted line, every reopening. Refuses a target that
        /// is not a section, or whose format recorded no mentions, in the
        /// format's vocabulary.
        pub fn renameContainer(self: *Self, path: []const AST.PathSegment, new_leaf: []const u8) !void {
            comptime requireSectionFormat("renameContainer");
            if (path.len == 0) return self.refuse(.not_a_section);
            const parsed = try self.getParsed();
            const node = try self.sectionAt(parsed, path);
            const mentions = parsed.mentionsOf(node.id);
            if (namesIn(mentions) == 0) return self.refuse(.not_a_section);
            const rendered = try self.formatInsertKey(new_leaf);
            defer self.allocator.free(rendered);
            try self.rewriteMentions(mentions, rendered);
        }

        /// Move the container at `src_path` before the container at
        /// `dest_path` (or to EOF if null), re-emitting its scattered
        /// fragments contiguously, separated from surrounding content by a
        /// blank line; interleaved foreign content stays put. Both ends must
        /// be section nodes. A no-op when the destination falls inside the
        /// source's own region.
        pub fn moveContainer(self: *Self, src_path: []const AST.PathSegment, dest_path: ?[]const AST.PathSegment) !void {
            comptime requireSectionFormat("moveContainer");
            const parsed = try self.getParsed();
            const node = try self.sectionAt(parsed, src_path);
            var used = try self.gatherRegions(parsed, node, true);
            defer used.deinit(self.allocator);
            // Destination: the start of the dest container's own first header
            // line (owned comment block included), or EOF.
            const dest_at = blk: {
                if (dest_path) |dp| {
                    const dn = try self.sectionAt(parsed, dp);
                    const first = parsed.regionsOf(dn.id)[0];
                    break :blk regions.headerLineRegion(self.source.items, first, self.syntax().comments).start;
                }
                break :blk self.source.items.len;
            };
            var guard = try self.guardAt(parsed, parsed.ast.node(parsed.ast.root), .regrouped);
            defer guard.deinit(self.allocator);
            try regions.relocate(self, used.items, dest_at);
            try self.readsBack(&guard);
        }

        /// Reorder top-level containers to the order given by `order` (their
        /// keys). Each named container's scattered fragments are removed and
        /// re-emitted contiguously, in `order`, at the position the earliest
        /// of them currently occupies. Containers not named are untouched.
        /// Each name must resolve to a section node.
        pub fn reorderContainers(self: *Self, order: []const []const u8) !void {
            comptime requireSectionFormat("reorderContainers");
            if (order.len == 0) return;
            const parsed = try self.getParsed();
            const source = self.source.items;

            var all: std.ArrayList(Region) = .empty;
            defer all.deinit(self.allocator);
            var bundles: std.ArrayList([]u8) = .empty;
            defer {
                for (bundles.items) |b| self.allocator.free(b);
                bundles.deinit(self.allocator);
            }

            for (order) |name| {
                const path: [1]AST.PathSegment = .{.{ .key = name }};
                const node = try self.sectionAt(parsed, &path);
                var used = try self.gatherRegions(parsed, node, true);
                defer used.deinit(self.allocator);
                const owned = try regions.captureBundle(self.allocator, source, used.items, &all);
                errdefer self.allocator.free(owned);
                try bundles.append(self.allocator, owned);
            }
            const total = regions.normalize(all.items, true);
            var guard = try self.guardAt(parsed, parsed.ast.node(parsed.ast.root), .regrouped);
            defer guard.deinit(self.allocator);
            try regions.reorderBundles(self, all.items[0..total], bundles.items, self.syntax());
            try self.readsBack(&guard);
        }

        /// Append a new element (body `body_text`) to the container sequence at
        /// `path` — TOML's `[[header]]` array-of-tables append. The element's
        /// header is spelled per `Syntax.section_header`'s sequence form and
        /// spliced past the current last element's whole extent, so a nested
        /// sub-table inside it is not split. A target that is not a sequence
        /// of containers is `NotAnArrayOfTables`. Comptime-refused for a
        /// format with no sequence-header syntax.
        pub fn appendContainerToSeq(self: *Self, path: []const AST.PathSegment, body_text: []const u8) !void {
            comptime requireHeaderSyntax("appendContainerToSeq");
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .sequence) return error.NotAnArrayOfTables;
            var elem = node.kind.sequence orelse return error.NotAnArrayOfTables;
            var last_elem = elem;
            while (true) {
                if (parsed.ast.node(elem).kind != .mapping) return error.NotAnArrayOfTables;
                last_elem = elem;
                elem = parsed.ast.node(elem).next_sibling orelse break;
            }
            const hdr = self.syntax().section_header.?;
            const seq_open = hdr.seq_open orelse return error.NotAnArrayOfTables;
            const insert_at = try self.sectionExtentEnd(parsed, parsed.ast.node(last_elem));
            try self.spliceHeader(insert_at, seq_open, hdr.seq_close.?, path, body_text);
        }

        /// Whether any dialect declares a `section_header`, and whether any
        /// declares its sequence form: the comptime gates on the two ops
        /// that write a header line.
        const has_section_header = @hasDecl(Language, "runtime") or blk: {
            var any = false;
            for (std.meta.tags(Language.Type)) |t| {
                if (Language.syntax(t).section_header != null) any = true;
            }
            break :blk any;
        };
        const has_seq_header = @hasDecl(Language, "runtime") or blk: {
            var any = false;
            for (std.meta.tags(Language.Type)) |t| {
                if (Language.syntax(t).section_header) |h| {
                    if (h.seq_open != null) any = true;
                }
            }
            break :blk any;
        };

        /// Whether this format has the whole-container op `op` — the
        /// runtime-dispatchable form of the comptime refusals, for callers
        /// that must ANSWER rather than fail to compile.
        ///
        /// The C ABI is the one such caller: its exports switch `inline else`
        /// over every language at once, so naming `e.deleteContainer(…)` in
        /// that switch would instantiate it for YAML and JSON too and stop the
        /// build. Guarding each arm with this turns "this format has no such
        /// operation" into `unsupported_format`, which is what a C caller can
        /// act on. Delete, move, reorder and rename are had by every section
        /// format; the two that write a header line by a format declaring
        /// `section_header`. No format is named.
        pub fn hasContainerOp(comptime op: []const u8) bool {
            if (comptime std.mem.eql(u8, op, "insertContainer")) return has_section_header;
            if (comptime std.mem.eql(u8, op, "appendContainerToSeq")) return has_seq_header;
            return is_section_format;
        }

        /// `hasContainerOp` for this editor's own dialect, asked at the
        /// call: the same answer for a compiled format, and for a runtime
        /// language — which `hasContainerOp` can only answer "may be" —
        /// whether the dialect it registered declares sections (and, for
        /// the two ops that write a header, a header to write).
        pub fn dialectHasContainerOp(self: *const Self, comptime op: []const u8) bool {
            if (comptime !hasContainerOp(op)) return false;
            const syn = self.syntax();
            if (syn.section_noun == null) return false;
            if (comptime std.mem.eql(u8, op, "insertContainer")) return syn.section_header != null;
            if (comptime std.mem.eql(u8, op, "appendContainerToSeq")) {
                const h = syn.section_header orelse return false;
                return h.seq_open != null;
            }
            return true;
        }

        /// The comptime refusal shared by the three generic ops: a format with
        /// no section nodes has nothing for them to address.
        fn requireSectionFormat(comptime op: []const u8) void {
            if (!is_section_format)
                @compileError("'" ++ op ++ "' is a whole-container op, and '" ++ Language.name ++
                    "' is not a section format — its `syntax().section_noun` is null in every dialect");
        }

        /// The comptime refusal for the two ops that write a header line: a
        /// format that declares no `section_header` (or no sequence form of
        /// it) has no spelling for one.
        fn requireHeaderSyntax(comptime op: []const u8) void {
            const has = if (comptime std.mem.eql(u8, op, "appendContainerToSeq")) has_seq_header else has_section_header;
            if (!has)
                @compileError("'" ++ op ++ "' writes a header line, and '" ++ Language.name ++
                    "' declares no `syntax().section_header` for it in any dialect");
        }

        /// How a splice's `value_text` is *spelled* — whether it stands as an
        /// inline value right after `key<sep>`, or is a block construct that has
        /// to descend onto the following lines.
        ///
        /// A splice carries only text, so this is the classification the framing
        /// decisions hang off. The container cases are settled by PARSING the
        /// text rather than sniffing its shape: a one-entry block mapping
        /// (`k: v`) is a single line with no dash and no line break, so shape
        /// alone cannot tell it from a scalar — and splicing it inline yields
        /// `key: k: v`, which is not YAML at all. That indistinguishability is
        /// the whole reason this enum exists.
        const ValueShape = enum {
            /// A scalar, a flow container (`[a, b]` / `{k: v}`), or a quoted
            /// string: splices directly after the separator.
            inline_,
            /// A block-scalar header (`|`/`>`): also splices after the separator
            /// (its body is already indented), but has no flow spelling.
            block_scalar,
            /// A block sequence (`- a`), which descends at the KEY's own column.
            block_seq,
            /// A block mapping (`k: v`), which descends indented under the key.
            block_map,
        };

        /// Classify `value_text` for the framing decisions (see `ValueShape`).
        fn valueShape(self: *Self, value_text: []const u8) ValueShape {
            const v = stripTrailingNewline(value_text);
            const nl = std.mem.indexOfScalar(u8, v, '\n');
            const first_line = std.mem.trimStart(u8, if (nl) |i| v[0..i] else v, " ");
            if (first_line.len == 0) return .inline_;
            if (first_line[0] == '|' or first_line[0] == '>') return .block_scalar;
            // A block sequence is recognizable even on a single line (`- a`); it
            // must still descend, since `key: - a` is invalid. (A serialized
            // scalar that would read as a dash is quoted, so this is safe.)
            if (std.mem.startsWith(u8, first_line, "- ") or std.mem.eql(u8, first_line, "-")) return .block_seq;
            if (nl != null) return .block_map;
            return if (self.singleLineIsBlockMapping(first_line)) .block_map else .inline_;
        }

        /// Whether single-line `text` is a block MAPPING entry (`k: v`) rather
        /// than a scalar — the one shape that cannot be told apart by sniffing,
        /// so it is read by the language's own parser.
        ///
        /// Only asked of a format that declares `single_line_block_mapping`
        /// (YAML alone today); everywhere else a `k: v` value is genuinely
        /// just scalar text and must keep splicing inline. See that field.
        fn singleLineIsBlockMapping(self: *Self, text: []const u8) bool {
            if (!self.syntax().single_line_block_mapping) return false;
            // A flow container also parses as a mapping/sequence but must stay
            // inline; a quoted scalar parses as a string. Both are settled by
            // the opening byte, cheaper than a parse.
            switch (text[0]) {
                '{', '[', '"', '\'' => return false,
                else => {},
            }
            var parser: Language.Parser = .{ .allocator = self.allocator };
            var doc = Language.parse(&parser, text, self.format) catch return false;
            defer doc.deinit(self.allocator);
            return doc.ast.node(doc.ast.root).kind == .mapping;
        }

        /// Append `: value` for a mapping entry whose key is already written at
        /// column `col`. Scalars and block scalars stay inline (`key: value`);
        /// a block collection goes on the following lines, indented (a nested
        /// mapping at `col + 2`, an indentless sequence at `col`).
        pub fn writeMapValue(self: *Self, out: *std.ArrayList(u8), indent: []const u8, value_text: []const u8) !void {
            const v = stripTrailingNewline(value_text);
            const shape = self.valueShape(v);
            const is_seq = shape == .block_seq;
            // No value at all — a bare `key:` (YAML's null, and the seed `set`
            // vivifies missing ancestors with). The separator's padding is
            // trimmed so the line doesn't end in whitespace; formats whose
            // separator carries no padding (`KEY=`) are unaffected.
            if (v.len == 0) {
                try out.appendSlice(self.allocator, std.mem.trimEnd(u8, try self.kvSep(), " "));
                return;
            }
            if (shape == .inline_ or shape == .block_scalar) {
                try out.appendSlice(self.allocator, try self.kvSep());
                try reindentInto(out, self.allocator, v, indent);
                return;
            }
            // Block collection value: descend onto the next lines. Only
            // reachable for languages with real nested block containers
            // (YAML/JSON5/fig) — dotenv/.properties values are always a
            // single line, so they never take this branch; the literal `:`
            // below is that block-mapping syntax, not `kv_sep`. A nested
            // mapping sits one `indent_unit` deeper than its key; a block
            // sequence may sit at the key's own column (YAML's `key:\n- a`).
            var child: std.ArrayList(u8) = .empty;
            defer child.deinit(self.allocator);
            try child.appendSlice(self.allocator, indent);
            if (!is_seq) try child.appendSlice(self.allocator, self.syntax().indent_unit);
            try out.append(self.allocator, ':');
            var it = std.mem.splitScalar(u8, v, '\n');
            while (it.next()) |line| {
                try out.append(self.allocator, '\n');
                if (line.len > 0) try out.appendSlice(self.allocator, child.items);
                try out.appendSlice(self.allocator, line);
            }
        }

        /// Splice a new block-sequence item line at `insert_at`, shaped like
        /// the item introduced at `sibling_marker`: that item's line prefix
        /// (`indentAt`), then the item as `writeItem` spells it.
        fn insertSeqLine(self: *Self, insert_at: usize, sibling_marker: usize, value_text: []const u8, place: Place) !void {
            const source = self.source.items;
            var indent_buf: std.ArrayList(u8) = .empty;
            defer indent_buf.deinit(self.allocator);
            const indent = try self.indentAt(&indent_buf, sibling_marker);
            var val_buf: std.ArrayList(u8) = .empty;
            defer val_buf.deinit(self.allocator);
            const rendered = try self.renderedValue(&val_buf, value_text, place);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            if (insert_at > 0 and source[insert_at - 1] != '\n') try out.append(self.allocator, '\n');
            try out.appendSlice(self.allocator, indent);
            try self.writeItem(&out, indent, rendered, place);
            try out.append(self.allocator, '\n');
            try self.replaceAtSpan(Span.init(insert_at, insert_at), out.items);
        }

        fn promoteNullToMapping(self: *Self, null_span: Span, is_root: bool, key_text: []const u8, value_text: []const u8, place: Place) !void {
            const source = self.source.items;
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            // A format with no bare `key: value` document form (ZON — unlike
            // YAML/JSON5, whose root can be a keyless top-level mapping)
            // promotes a `null` value in place to a flow container, root or
            // nested alike, so this needs no `is_root`/descend distinction.
            const syn = self.syntax();
            if (!syn.bare_document_mapping) {
                try out.appendSlice(self.allocator, syn.flow_map_open);
                try out.append(self.allocator, ' ');
                try out.appendSlice(self.allocator, key_text);
                try out.appendSlice(self.allocator, try self.kvSep());
                try out.appendSlice(self.allocator, value_text);
                try out.append(self.allocator, ' ');
                try out.appendSlice(self.allocator, syn.flow_map_close);
                try self.replaceAtSpan(null_span, out.items);
                return;
            }
            // `writeMapValue` emits the separator itself, so a block value
            // (`- a`, `k: v`) descends onto the following lines instead of being
            // jammed inline after the `:` — where it would not parse.
            var val_buf: std.ArrayList(u8) = .empty;
            defer val_buf.deinit(self.allocator);
            const rendered = try self.renderedValue(&val_buf, value_text, place);
            if (is_root) {
                // Empty document: the whole source becomes a single entry.
                try self.writeEntry(&out, "", key_text, rendered, place);
                try out.append(self.allocator, '\n');
                try self.replaceAtSpan(Span.init(0, source.len), out.items);
                return;
            }
            // The promoted mapping's first entry sits one `indent_unit` under
            // the key that owned the null: that key's line prefix plus one.
            const line_start = lineStartBefore(source, null_span.start);
            var child: std.ArrayList(u8) = .empty;
            defer child.deinit(self.allocator);
            _ = try self.indentAt(&child, firstNonSpace(source, line_start));
            try child.appendSlice(self.allocator, self.syntax().indent_unit);
            try out.append(self.allocator, '\n');
            try out.appendSlice(self.allocator, child.items);
            try self.writeEntry(&out, child.items, key_text, rendered, place);
            try self.replaceAtSpan(null_span, out.items);
        }

        /// Reject `value_text` that has no flow spelling before it is spliced
        /// into a `{…}`/`[…]` container. A flow container holds one line of
        /// comma-separated members: a block sequence (`- a`), a block mapping
        /// (`k: v`), a block scalar (`|`), or anything multi-line means something
        /// else entirely — or nothing at all — once wrapped in braces.
        ///
        /// The reparse in `replaceAtSpan` is the general safety net for a bad
        /// splice, but it cannot be the one that catches this: `{b: - a}` is
        /// text a lenient reader may well accept as the STRING `"- a"`, so the
        /// document still parses and the sequence is silently gone. Refuse up
        /// front instead, and refuse with a name that says why, rather than
        /// leaving the caller a generic parse failure to interpret.
        ///
        /// A caller that needs a block value under a flow container has to
        /// render the value in flow (fig's binding: `flow = 1`) or expand the
        /// container to block form first.
        fn requireFlowValue(self: *Self, value_text: []const u8) !void {
            if (self.valueShape(value_text) != .inline_) return error.BlockValueIntoFlow;
        }

        /// Whether the flow container `node` is laid out one member per line:
        /// its closer is separated from the last member (ending at `last_end`)
        /// by a newline — or, for an open container, which has no closer, its
        /// first and last members are on different lines.
        fn flowIsMultiLine(self: *const Self, parsed: Document, node: AST.Node, span: Span, last_end: usize) !bool {
            const source = self.source.items;
            if (self.isOpen(parsed, node)) {
                const first = (try parsed.ast.child(&node)).?;
                return std.mem.indexOfScalar(u8, source[parsed.span(first).start..last_end], '\n') != null;
            }
            const close = span.end - 1; // the '}' or ']'
            return std.mem.indexOfScalar(u8, source[last_end..close], '\n') != null;
        }

        /// Insert a `key: value` entry into a brace-delimited (flow) mapping,
        /// matching its layout. A pretty-printed mapping — one whose closing `}`
        /// sits on its own line below the members — gets the new entry on its own
        /// line, indented to match the existing members (a trailing comma after
        /// the last member's value, newline, member indent, `key: value`). A
        /// compact single-line mapping keeps the inline `", key: value"` style.
        fn insertFlowMapEntry(self: *Self, parsed: Document, node: AST.Node, span: Span, non_empty: bool, key_text: []const u8, value_text: []const u8) !void {
            try self.requireFlowValue(value_text);
            const source = self.source.items;
            if (non_empty) {
                // The member written last, which a table made by dotted
                // keys may be though it is not the last child.
                var last_end: usize = 0;
                var each = try parsed.ast.child(&node);
                while (each) |kv| : (each = parsed.ast.next(&kv)) last_end = @max(last_end, try self.flowEntryEnd(parsed, kv));
                // The separator: `kv_sep`, or the bytes the first entry
                // writes between its key and value for a format whose flow
                // objects fix their own mode. See
                // `Syntax.flow_kv_sep_from_siblings`.
                const sep: []const u8 = if (self.syntax().flow_kv_sep_from_siblings) blk: {
                    const first = firstWrittenLeaf(parsed, parsed.ast.node(node.kind.mapping.?));
                    const key_end = parsed.span(parsed.ast.node(first.kind.keyvalue.key)).end;
                    const value_start = parsed.span(parsed.ast.node(first.kind.keyvalue.value)).start;
                    break :blk source[key_end..value_start];
                } else try self.kvSep();
                const joint = try self.flowMapJoint(parsed, node, sep);
                // Multi-line layout: the closing brace is separated from the last
                // member by a newline (an open root, which has no brace: its
                // first and last members are on different lines). Splice after
                // the last member's value so the new entry lands on its own
                // line, not jammed before the brace.
                if (try self.flowIsMultiLine(parsed, node, span, last_end)) {
                    // Column of the key's line, not the key node's own span start:
                    // for ZON the key span covers only the bare identifier after
                    // its leading `.` (the dot is a separate token), so anchoring
                    // on the span would misindent by one column. Every other
                    // format's key span already starts at that line's first
                    // content byte, so this is equivalent for them.
                    const key_node = (try parsed.ast.firstChildKey(&node)).?;
                    const col = columnOf(source, firstNonSpace(source, lineStartBefore(source, parsed.span(key_node).start)));
                    var out: std.ArrayList(u8) = .empty;
                    defer out.deinit(self.allocator);
                    try out.appendSlice(self.allocator, joint.token);
                    try out.append(self.allocator, '\n');
                    try out.appendNTimes(self.allocator, ' ', col);
                    try out.appendSlice(self.allocator, key_text);
                    try out.appendSlice(self.allocator, sep);
                    try out.appendSlice(self.allocator, value_text);
                    try self.replaceAtSpan(Span.init(last_end, last_end), out.items);
                    return;
                }
                // Single-line layout: splice right after the last member's own
                // value — NOT right before the closing brace (`span.end - 1`),
                // which would land inside any padding space before `}` (`{ a: 1
                // }` -> `{ a: 1 , b: 2}`, swallowing the closing pad and leaving
                // none before the new entry). Landing at `last_end` keeps any such
                // padding after the new entry instead.
                var out: std.ArrayList(u8) = .empty;
                defer out.deinit(self.allocator);
                try out.appendSlice(self.allocator, joint.lead);
                try out.appendSlice(self.allocator, joint.token);
                try out.appendSlice(self.allocator, joint.trail);
                try out.appendSlice(self.allocator, key_text);
                try out.appendSlice(self.allocator, sep);
                try out.appendSlice(self.allocator, value_text);
                try self.replaceAtSpan(Span.init(last_end, last_end), out.items);
                return;
            }
            return self.insertFlowEntry(parsed, node, span, key_text, value_text);
        }

        /// The bytes an insert writes between the last member of the
        /// non-empty flow mapping `node` and a new one on its line, the new
        /// one's key and value parted by `kv`. A format that copies its flow
        /// separators (`Syntax.flow_kv_sep_from_siblings`) takes the bytes
        /// between the first two members when they share a line and hold
        /// nothing but spaces and at most one `flow_entry_sep`, so
        /// `{a=1, b=2}` stays comma-joined. Otherwise an object whose
        /// entries are spelled with `flow_map_kv_sep` is joined by
        /// `flow_map_entry_sep` (fig's `{a=1 b=2}`), and any other by
        /// `flow_entry_sep` and a space. A multi-line insert writes only the
        /// joint's token.
        fn flowMapJoint(self: *const Self, parsed: Document, node: AST.Node, kv: []const u8) !Joint {
            const source = self.source.items;
            const syn = self.syntax();
            if (syn.flow_kv_sep_from_siblings) copy: {
                const first = (try parsed.ast.child(&node)).?;
                const second = parsed.ast.next(&first) orelse break :copy;
                const from = parsed.span(firstWrittenLeaf(parsed, first)).end;
                const to = flowEntryStart(parsed, second);
                if (to <= from) break :copy;
                const gap = source[from..to];
                const joint = Joint.split(gap, false);
                if (std.mem.indexOfAny(u8, gap, "\r\n") != null) break :copy;
                if (joint.token.len > 0 and !std.mem.eql(u8, joint.token, syn.flow_entry_sep)) break :copy;
                return joint;
            }
            const trim = " \t";
            if (syn.flow_map_entry_sep.len > 0 and syn.flow_map_kv_sep.len > 0 and
                std.mem.eql(u8, std.mem.trim(u8, kv, trim), std.mem.trim(u8, syn.flow_map_kv_sep, trim)))
                return Joint.split(syn.flow_map_entry_sep, false);
            return .{ .lead = "", .token = syn.flow_entry_sep, .trail = " ", .declared = false };
        }

        /// Insert the first entry into an EMPTY flow mapping (`{}` / ZON's
        /// `.{}`): a tight `{key: value}` splice, unpadded — matching how
        /// `insertFlowItem`'s empty-array case and the pre-existing JSON/YAML
        /// empty-flow-map tests already splice (no space added around a
        /// freshly-created single member).
        fn insertFlowEntry(self: *Self, parsed: Document, node: AST.Node, span: Span, key_text: []const u8, value_text: []const u8) !void {
            try self.requireFlowValue(value_text);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            // An open root has no braces for the pad to stand inside.
            const open_root = self.isOpenRoot(parsed, node);
            const pad = if (open_root) "" else self.syntax().flow_map_pad;
            try out.appendSlice(self.allocator, pad);
            try out.appendSlice(self.allocator, key_text);
            // A format that spells flow entries apart from block ones says
            // how: `Syntax.flow_map_kv_sep`.
            const kv = self.syntax().flow_map_kv_sep;
            try out.appendSlice(self.allocator, if (kv.len > 0) kv else try self.kvSep());
            try out.appendSlice(self.allocator, value_text);
            try out.appendSlice(self.allocator, pad);
            // Just after '{' (or ZON's '.{'); an open root has no opener, and
            // its first member starts the source.
            const at = if (open_root) firstNonSpace(self.source.items, span.start) else flowOpenEnd(self.source.items, span, self.syntax().flow_map_open);
            try self.replaceAtSpan(Span.init(at, at), out.items);
        }

        /// Insert `value_text` as the new last item of the flow sequence
        /// `node`/`span`. Splices immediately after the current last
        /// element rather than before the closing `]`, so a pre-existing
        /// trailing comma (legal in fig/JSON5 flow arrays) isn't doubled
        /// into an empty element that fails to reparse — and so a separated
        /// sequence's trailing separator stays trailing (`1 | 2 |` takes `3`
        /// as `1 | 2 | 3 |`). The separator before the new item is laid out
        /// as the sequence is (`writeJoint`).
        fn insertFlowItem(self: *Self, parsed: Document, node: AST.Node, span: Span, non_empty: bool, value_text: []const u8) !void {
            const joint = self.jointOf(parsed, node);
            if (!joint.declared) try self.requireFlowValue(value_text);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            if (non_empty) {
                const last = (try parsed.ast.lastChild(&node)).?;
                const last_end = parsed.span(last).end;
                try self.writeJoint(&out, parsed, node, joint);
                try out.appendSlice(self.allocator, value_text);
                try self.replaceAtSpan(Span.init(last_end, last_end), out.items);
                return;
            }
            const at = self.flowBodyStart(parsed, node, span);
            try self.replaceAtSpan(Span.init(at, at), value_text);
        }

        /// Insert `value_text` into the flow sequence `node`/`span` as the
        /// item before `before` — or, with none, as the first item of an
        /// empty one. Splices right before `before`'s own span, so any
        /// padding or indent before it (`[ a, b ]`, an item on a line of its
        /// own) stays before the new item rather than being swallowed
        /// against it, and `before` follows the new item after the
        /// separator `writeJoint` lays out — or, when not `laid_out`, after
        /// the separator as written on one line, which is what a prepend
        /// into a sequence with the dialect's separator has always written
        /// (`[\n  z, a,\n  b\n]`).
        fn insertFlowItemBefore(self: *Self, parsed: Document, node: AST.Node, span: Span, before: ?AST.Node, value_text: []const u8, laid_out: bool) !void {
            const joint = self.jointOf(parsed, node);
            if (!joint.declared) try self.requireFlowValue(value_text);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            try out.appendSlice(self.allocator, value_text);
            const at = if (before) |b| blk: {
                if (laid_out or joint.declared) {
                    try self.writeJoint(&out, parsed, node, joint);
                } else {
                    try out.appendSlice(self.allocator, joint.lead);
                    try out.appendSlice(self.allocator, joint.token);
                    try out.appendSlice(self.allocator, joint.trail);
                }
                break :blk parsed.span(b).start;
            } else self.flowBodyStart(parsed, node, span);
            try self.replaceAtSpan(Span.init(at, at), out.items);
        }

        /// Where the first item of an EMPTY flow container goes: just after
        /// its opener (`[`, ZON's `.{`), or for an open one, which has none,
        /// its span's own start.
        fn flowBodyStart(self: *const Self, parsed: Document, node: AST.Node, span: Span) usize {
            if (self.isOpen(parsed, node)) return firstNonSpace(self.source.items, span.start);
            return flowOpenEnd(self.source.items, span, self.syntax().flow_map_open);
        }

        /// Whitespace the flow-item scan may cross while hunting for the
        /// adjoining comma: spaces/tabs plus newlines, so a one-item-per-line
        /// layout (item on its own indented line) is treated the same as a
        /// packed single-line layout.
        fn isFlowItemWs(c: u8) bool {
            return c == ' ' or c == '\t' or c == '\n';
        }

        /// The trailing comment that ends the line a flow member ending at
        /// `from` is on, past spaces and one `sep` (`a, // c`), as the span
        /// from the spaces before it to the line's end; null when anything
        /// else comes first, or follows a paired comment on its line
        /// (`/* x */ }`).
        fn lineEndComment(self: *const Self, from: usize, sep: []const u8) ?Span {
            const marker = self.trailingCommentMarker() orelse return null;
            const source = self.source.items;
            var i = firstNonSpace(source, from);
            if (sep.len > 0 and std.mem.startsWith(u8, source[i..], sep)) i = firstNonSpace(source, i + sep.len);
            if (!std.mem.startsWith(u8, source[i..], marker.open)) return null;
            var line_end = std.mem.indexOfScalarPos(u8, source, i, '\n') orelse source.len;
            if (line_end > i and source[line_end - 1] == '\r') line_end -= 1;
            if (marker.close.len > 0) {
                const close = std.mem.indexOfPos(u8, source[0..line_end], i + marker.open.len, marker.close) orelse return null;
                const after = close + marker.close.len;
                if (firstNonSpace(source, after) != line_end) return null;
                line_end = after;
            }
            var ts = i;
            while (ts > from and (source[ts - 1] == ' ' or source[ts - 1] == '\t')) ts -= 1;
            return Span.init(ts, line_end);
        }

        /// Delete the flow item or member at `item_span` and exactly one
        /// separator beside it: the one after it when it is first, else the
        /// one before it, so the last of `a, b,` leaves `a,` and keeps a
        /// trailing separator trailing. `sep` is the token the separator
        /// is spelled with (`,`, a separated sequence's `|`), or empty for
        /// items joined by whitespace alone, where the whitespace is the
        /// separator. Nothing outside `limit` is taken: an open container
        /// has no closer to stop the scan, so its span does.
        fn removeFlowItem(self: *Self, item_span: Span, is_first: bool, sep: []const u8, limit: Span) !void {
            const source = self.source.items[0..limit.end];
            if (is_first) {
                // Drop the item and a following ", " if present. Consuming
                // forward through trailing whitespace *including newlines*
                // means a standalone-line item's own indentation-and-newline
                // goes with it, leaving the next item on the line the
                // removed item's leading indent occupied — not a stray
                // blank line.
                var e = item_span.end;
                while (e < source.len and isFlowItemWs(source[e])) e += 1;
                if (sep.len > 0 and std.mem.startsWith(u8, source[e..], sep)) {
                    e += sep.len;
                    while (e < source.len and isFlowItemWs(source[e])) e += 1;
                }
                try self.replaceAtSpan(Span.init(item_span.start, e), "");
            } else {
                // Drop a preceding ", " and the item. Scanning backward
                // across newlines (not just spaces/tabs) is what lets this
                // find the *previous* item's separator comma when the
                // removed item sits alone on its own line — otherwise the
                // scan stops at the newline and leaves the removed item's
                // own trailing comma (if the array uses a trailing-comma
                // style) dangling with nothing before it, which fails to
                // reparse as an empty element.
                var s = item_span.start;
                while (s > limit.start and isFlowItemWs(source[s - 1])) s -= 1;
                if (sep.len > 0 and std.mem.endsWith(u8, source[limit.start..s], sep)) {
                    s -= sep.len;
                    while (s > limit.start and isFlowItemWs(source[s - 1])) s -= 1;
                }
                try self.replaceAtSpan(Span.init(s, item_span.end), "");
            }
        }

        /// Replace a span of bytes with a new span of bytes.
        /// Not aware of self.format. Invalidates self.parsed until reparsed.
        fn replaceSource(self: *Self, old_span: Span, text: []const u8) !void {
            if (old_span.end < old_span.start or old_span.end > self.source.items.len) {
                return error.InvalidSpan;
            }
            try self.source.replaceRange(self.allocator, old_span.start, old_span.len(), text);
        }

        /// After an edit, restores self.parsed so node spans are valid again.
        fn reparse(self: *Self) !void {
            const parsed = try self.parseSource();
            self.freeDocument();
            self.document = parsed;
        }

        fn parseSource(self: *Self) !Document {
            var parser: Language.Parser = .{ .allocator = self.allocator };
            return Language.parse(&parser, self.source.items, self.format);
        }

        fn freeDocument(self: *Self) void {
            if (self.document) |parsed| {
                parsed.deinit(self.allocator);
                self.document = null;
            }
        }

        pub fn deinit(self: *Self) void {
            self.freeDocument();
            self.source.deinit(self.allocator);
        }
    };
}

// ======================
// SOURCE-COORDINATE UTILS
// ======================
//
// The shared ones live in `editor/splice.zig` and are re-imported at the top
// of this file; what remains here is used by the engine alone.

/// Start of a mapping entry's full block: its owned leading comment block
/// (`commentBlockStart`) at the start of the key's line. Mirrors the span math
/// `deleteKey` uses, factored out for move/reorder.
fn entryBlockStart(source: []const u8, kv_span: Span, comments: Comments) usize {
    return commentBlockStart(source, lineStartBefore(source, kv_span.start), comments);
}

/// End of a mapping entry's full block: just past the newline ending its last
/// line (or `source.len` when the final line is unterminated).
fn entryBlockEnd(source: []const u8, kv_span: Span) usize {
    return lineEndAfter(source, kv_span.end -| 1);
}

/// Strip a leading line-comment `marker` (and one following space) from `line`,
/// the inverse of how `renderComment` emits a comment.
/// `line` must already have its leading whitespace trimmed. A line that doesn't
/// start with `marker` is returned unchanged.
fn stripLineCommentMarker(line: []const u8, marker: lang.CommentDelimiter) []const u8 {
    if (!std.mem.startsWith(u8, line, marker.open)) return line;
    var rest = line[marker.open.len..];
    if (rest.len > 0 and rest[0] == ' ') rest = rest[1..];
    // A paired delimiter: drop the close and the one space before it, the
    // inverse of `renderComment`.
    if (marker.close.len > 0 and std.mem.endsWith(u8, rest, marker.close)) {
        rest = rest[0 .. rest.len - marker.close.len];
        if (rest.len > 0 and rest[rest.len - 1] == ' ') rest = rest[0 .. rest.len - 1];
    }
    return rest;
}

/// The column on the line starting at `line_start` where an own-line comment's
/// marker belongs — past the line's leading whitespace, and (for a format whose
/// line prefix is STRUCTURAL, `Syntax.structural_indent`) past that prefix too.
///
/// The one place `commentOut` writes a marker and the one place `uncomment*`
/// looks for it again, so the two are inverses by construction. It matters for
/// fig alone: its `>` marker run is section depth, not indentation, so a
/// commented-out `> > size = 10` has to become `> > # size = 10` to stay
/// attached where it was — `# > > size = 10` would read as a root-level
/// comment. The `*` of a fig sequence item is NOT skipped: it introduces the
/// item, so it is part of what a comment-out has to hide.
fn commentColumn(source: []const u8, line_start: usize, structural: bool) usize {
    var i = firstNonSpace(source, line_start);
    if (!structural) return i;
    while (i < source.len and source[i] == '>') i = firstNonSpace(source, i + 1);
    return i;
}

/// Render one comment into `out`: `marker.open`, then a space and `line`
/// unless `line` is empty, then — for a paired delimiter — a space and
/// `marker.close`. `# text`, `<!-- text -->`, a bare `#`, `<!-- -->`.
fn renderComment(allocator: std.mem.Allocator, out: *std.ArrayList(u8), marker: lang.CommentDelimiter, line: []const u8) !void {
    try out.appendSlice(allocator, marker.open);
    if (line.len > 0) {
        try out.append(allocator, ' ');
        try out.appendSlice(allocator, line);
    }
    if (marker.close.len > 0) {
        try out.append(allocator, ' ');
        try out.appendSlice(allocator, marker.close);
    }
}

/// Render `text` as one or more own-line comments into `out`, each line being
/// `indent` + one `renderComment` + '\n'. A single trailing newline in `text`
/// is ignored so it never yields a stray empty comment line.
fn renderLineComments(
    allocator: std.mem.Allocator,
    out: *std.ArrayList(u8),
    indent: []const u8,
    marker: lang.CommentDelimiter,
    text: []const u8,
) !void {
    const body = if (std.mem.endsWith(u8, text, "\n")) text[0 .. text.len - 1] else text;
    var it = std.mem.splitScalar(u8, body, '\n');
    while (it.next()) |line| {
        try out.appendSlice(allocator, indent);
        try renderComment(allocator, out, marker, line);
        try out.append(allocator, '\n');
    }
}

/// Whether a node is a leaf scalar — the kinds `setSequence` can match by value.
fn isScalarKind(kind: AST.Node.Kind) bool {
    return switch (kind) {
        .null_, .boolean, .string, .number, .extended => true,
        .sequence, .mapping, .keyvalue, .alias => false,
    };
}

/// Count the children of a container node.
fn seqLen(parsed: Document, node: AST.Node) !usize {
    var n: usize = 0;
    var maybe = try parsed.ast.child(&node);
    while (maybe) |c| {
        n += 1;
        maybe = parsed.ast.next(&c);
    }
    return n;
}

// ── "nothing else moved": the uncomment and delete guards ───────────────────
//
// `uncommentLeading`/`uncommentDangling` strip a marker off lines the CALLER
// judged to be an entry, so the editor's half of the bargain is to prove the
// result is what was claimed: a document that parses, and that differs from
// the one before the splice only by what was ADDED to the container the lines
// sit in. Anything else — text that merged into a neighbouring block scalar,
// a line that re-opened a section and reparented the entries below it — is
// rolled back byte-for-byte with `CommentNotAnEntry`.
//
// A delete makes the same promise the other way round (`keepOthers`): the
// document differs only by the one child REMOVED from its container, or the
// splice is rolled back with `DeleteChangedOtherValues`. What reads back
// differently only because that child is gone counts as the child: its
// container read as empty (`readsEmpty`), an implicit table that held only it
// (`impliedOnlyBy`), a duplicate of its key (`duplicateSurfaced`).
//
// The comparison is over the two parses, not their bytes: the after-tree is
// walked to the target container along the same path the caller named, every
// node passed on the way compared whole, and only that container is allowed
// to have changed, and only in the one way. Comments and layout are outside
// the AST and so outside the check, which is right — a comment run losing a
// line is exactly what the op does.

/// Whether every child of `bn` still appears, in order and unchanged, among
/// the children of `an` — i.e. `an` differs from `bn` only by insertions.
/// Matching is greedy: each before-child claims the earliest after-child it
/// equals. Greedy can only fail early, never accept something it shouldn't, so
/// the failure direction is the safe one (a rollback, not a bad commit).
fn childrenOnlyGrew(b: Document, bn: AST.Node, a: Document, an: AST.Node) bool {
    var b_cur = (b.ast.child(&bn) catch return false);
    var a_cur = (a.ast.child(&an) catch return false);
    while (b_cur) |bc| {
        var matched = false;
        while (a_cur) |ac| {
            a_cur = a.ast.next(&ac);
            if (nodesEqualDeep(b, bc.id, a, ac.id)) {
                matched = true;
                break;
            }
        }
        if (!matched) return false;
        b_cur = b.ast.next(&bc);
    }
    return true;
}

/// Whether the children of `an` are those of `bn` with the one at `removed`
/// left out, each of the rest unchanged and in its order.
fn childrenLostOne(b: Document, bn: AST.Node, a: Document, an: AST.Node, removed: usize) bool {
    var b_cur = (b.ast.child(&bn) catch return false);
    var a_cur = (a.ast.child(&an) catch return false);
    var index: usize = 0;
    var lost = false;
    while (b_cur) |bc| : ({
        b_cur = b.ast.next(&bc);
        index += 1;
    }) {
        if (index == removed) {
            lost = true;
            continue;
        }
        const ac = a_cur orelse return false;
        if (!nodesEqualDeep(b, bc.id, a, ac.id)) return false;
        a_cur = a.ast.next(&ac);
    }
    return lost and a_cur == null;
}

/// Structural equality of two subtrees across two parses: same kinds, same
/// scalar bytes, same children in the same order. Node ids differ between the
/// two documents, so `AST.Node.Kind.eql` (which compares child ids) cannot be
/// used for containers — only for the leaves.
fn nodesEqualDeep(b: Document, bid: AST.Node.Id, a: Document, aid: AST.Node.Id) bool {
    const bn = b.ast.node(bid);
    const an = a.ast.node(aid);
    if (std.meta.activeTag(bn.kind) != std.meta.activeTag(an.kind)) return false;
    switch (bn.kind) {
        .mapping, .sequence => {
            var b_cur = (b.ast.child(&bn) catch return false);
            var a_cur = (a.ast.child(&an) catch return false);
            while (b_cur) |bc| {
                const ac = a_cur orelse return false;
                if (!nodesEqualDeep(b, bc.id, a, ac.id)) return false;
                b_cur = b.ast.next(&bc);
                a_cur = a.ast.next(&ac);
            }
            return a_cur == null;
        },
        .keyvalue => |kv| return nodesEqualDeep(b, kv.key, a, an.kind.keyvalue.key) and
            nodesEqualDeep(b, kv.value, a, an.kind.keyvalue.value),
        else => return bn.kind.eql(an.kind),
    }
}

/// The one change a guarded op may make to its container: nodes added
/// (uncomment), or the child at an index removed (delete).
const Change = union(enum) {
    added,
    removed: Removal,
};

/// What a delete took: the child at `at` of its container, by a splice that
/// changed `edit`'s bytes.
const Removal = struct {
    at: usize,
    edit: Edit,
};

/// The one run of bytes two sources differ in: `before[start..before_end]`
/// became `after[start..after_end]`, everything either side of it the same
/// bytes in both. The run starts at a line's start, which is where a delete's
/// splice starts unless it shares a line: a run found byte by byte would
/// start wherever the text the splice took happens to repeat the text after
/// it (`A=1` over `A=2` is a run from `1` to `2`), and say a duplicate's
/// surviving spelling is the deleted one's.
const Edit = struct {
    start: usize,
    before_end: usize,
    after_end: usize,

    fn between(before: []const u8, after: []const u8) Edit {
        const shorter = @min(before.len, after.len);
        var head: usize = 0;
        while (head < shorter and before[head] == after[head]) head += 1;
        const start = lineStartBefore(before, head);
        var tail: usize = 0;
        while (tail < shorter - start and before[before.len - 1 - tail] == after[after.len - 1 - tail]) tail += 1;
        return .{ .start = start, .before_end = before.len - tail, .after_end = after.len - tail };
    }

    /// Whether the splice took or rewrote the bytes `span` names in the
    /// source before.
    fn took(self: Edit, span: Span) bool {
        return span.start >= self.start and span.end <= self.before_end;
    }

    /// Where the byte at `at` in the source after stood in the source
    /// before, or null for a byte the splice wrote.
    fn beforeOffset(self: Edit, at: usize) ?usize {
        if (at < self.start) return at;
        if (at >= self.after_end) return at - self.after_end + self.before_end;
        return null;
    }
};

/// Whether `after` differs from `before` only by `change` to the container
/// at `path` (the root for an empty path). Every other node — including every
/// container passed through on the way down, and every sibling of the ones
/// that are — must be structurally identical.
fn changedOnlyAt(before: Document, after: Document, path: []const AST.PathSegment, change: Change) bool {
    return changedOnlyUnder(before, before.ast.root, after, after.ast.root, path, change);
}

fn changedOnlyUnder(
    b: Document,
    bid: AST.Node.Id,
    a: Document,
    aid: AST.Node.Id,
    path: []const AST.PathSegment,
    change: Change,
) bool {
    const bn = b.ast.node(bid);
    const an = a.ast.node(aid);
    // A container emptied of its only child reads back as the format reads
    // an empty value: null (a block container, `k:`), an empty string
    // (NestedText's `-` or `k:`), or an empty container of either kind
    // (ZON's `.{}` is the one literal for both).
    if (path.len == 0 and change == .removed and readsEmpty(a, an) and
        std.meta.activeTag(bn.kind) != std.meta.activeTag(an.kind))
        return change.removed.at == 0 and (seqLen(b, bn) catch return false) == 1;
    if (std.meta.activeTag(bn.kind) != std.meta.activeTag(an.kind)) return false;
    if (path.len == 0) return switch (change) {
        .added => childrenOnlyGrew(b, bn, a, an),
        .removed => |r| childrenLostOne(b, bn, a, an, r.at) or duplicateSurfaced(b, bn, a, an, r),
    };

    // Not the target yet: walk the two child lists in lockstep. They must have
    // the same length and pair up, with only the child the next path segment
    // names allowed to differ — and only by what is added deeper down, or
    // taken: an implicit table that held nothing but the deleted entry goes
    // with it (`impliedOnlyBy`).
    var b_cur = (b.ast.child(&bn) catch return false);
    var a_cur = (a.ast.child(&an) catch return false);
    var index: usize = 0;
    while (b_cur) |bc| : ({
        b_cur = b.ast.next(&bc);
        index += 1;
    }) {
        const on_path = switch (path[0]) {
            .key => bc.kind == .keyvalue and keyNodeIs(b, bc.kind.keyvalue.key, path[0].key),
            .index => index == path[0].index,
        };
        const paired = if (a_cur) |ac| !on_path or path[0] == .index or
            (ac.kind == .keyvalue and keyNodeIs(a, ac.kind.keyvalue.key, path[0].key)) else false;
        if (!paired) {
            if (on_path and change == .removed and impliedOnlyBy(b, bc, path[1..], change.removed.at)) continue;
            return false;
        }
        const ac = a_cur.?;
        a_cur = a.ast.next(&ac);
        const b_next = if (bc.kind == .keyvalue) bc.kind.keyvalue.value else bc.id;
        const a_next = if (ac.kind == .keyvalue) ac.kind.keyvalue.value else ac.id;
        if (on_path) {
            if (bc.kind == .keyvalue and !nodesEqualDeep(b, bc.kind.keyvalue.key, a, ac.kind.keyvalue.key))
                return false;
            if (!changedOnlyUnder(b, b_next, a, a_next, path[1..], change)) return false;
        } else if (!nodesEqualDeep(b, bc.id, a, ac.id)) return false;
    }
    return a_cur == null;
}

/// Whether `node` is what an emptied container may read back as: null, an
/// empty string, or a container with no children.
fn readsEmpty(doc: Document, node: AST.Node) bool {
    return switch (node.kind) {
        .null_ => true,
        .string => |s| s.len == 0,
        .mapping, .sequence => (doc.ast.child(&node) catch return false) == null,
        else => false,
    };
}

/// How many of `mentions` are a section's name, which a rename rewrites;
/// the rest are an item's path (`Document.MentionKind.isName`).
fn namesIn(mentions: Document.MentionIterator) usize {
    var n: usize = 0;
    var it = mentions;
    while (it.next()) |m| {
        if (m.kind.isName()) n += 1;
    }
    return n;
}

/// Whether the entry `kv` of the parse before a delete names an IMPLICIT
/// table that held nothing but the path down to the deleted entry, so that
/// the entry's going takes it too: TOML's `a.b.c = 1` is the only thing
/// that makes `a.b` a table, and with that line gone `a.b` is not empty but
/// absent, as it would be had it never been written. A table is implicit
/// when every place its name is written is on an entry line (a `.entry`
/// mention: a dotted key, a fig header at its parent's depth), none of them
/// a header of its own (`[a.b]`, an INI `[section]`, which stays and reads
/// back empty) — and a container whose format records no mentions, a YAML
/// block mapping, is never implicit: `k:` stays when its last entry goes.
/// `rest` is the path below `kv`, and `at` the deleted child's index in the
/// container at its end.
fn impliedOnlyBy(b: Document, kv: AST.Node, rest: []const AST.PathSegment, at: usize) bool {
    if (kv.kind != .keyvalue) return false;
    const table = b.ast.node(kv.kind.keyvalue.value);
    if (table.kind != .mapping) return false;
    var mentions = b.mentionsOf(table.id);
    if (mentions.count() == 0) return false;
    while (mentions.next()) |m| if (m.kind != .entry) return false;
    const only = (b.ast.child(&table) catch return false) orelse return false;
    if (b.ast.next(&only) != null) return false;
    if (rest.len == 0) return at == 0;
    return rest[0] == .key and only.kind == .keyvalue and
        keyNodeIs(b, only.kind.keyvalue.key, rest[0].key) and
        impliedOnlyBy(b, only, rest[1..], at);
}

/// Whether the mapping `an` is `bn` with the entry at `r.at` deleted and a
/// DUPLICATE of its key showing through: a format that lets a key repeat
/// (dotenv, INI, `.properties`, plist) reads `A=1` … `A=3` as one `A` whose
/// value is the last, so deleting the first spelling leaves `A` there, with
/// the value its other spelling gives it and at that spelling's place.
/// Every other entry must be as it was and in its order. The deleted key must
/// be one the splice took, and the `A` after it one the splice did not
/// write — a spelling that stood elsewhere before — so a splice that left the
/// key it was meant to take is not mistaken for a duplicate.
fn duplicateSurfaced(b: Document, bn: AST.Node, a: Document, an: AST.Node, r: Removal) bool {
    if (bn.kind != .mapping) return false;
    var b_cur = (b.ast.child(&bn) catch return false);
    var gone: ?AST.Node = null;
    var index: usize = 0;
    while (b_cur) |bc| : (b_cur = b.ast.next(&bc)) {
        if (index == r.at) gone = bc;
        index += 1;
    }
    const kv = (gone orelse return false).kind;
    if (kv != .keyvalue) return false;
    const key = switch (b.ast.node(kv.keyvalue.key).kind) {
        .string => |s| s,
        else => return false,
    };
    if (!r.edit.took(b.spanOf(kv.keyvalue.key))) return false;

    b_cur = (b.ast.child(&bn) catch return false);
    var a_cur = (a.ast.child(&an) catch return false);
    var surfaced = false;
    index = 0;
    while (true) {
        if (b_cur != null and index == r.at) {
            b_cur = b.ast.next(&b_cur.?);
            index += 1;
            continue;
        }
        if (a_cur) |ac| {
            if (ac.kind == .keyvalue and keyNodeIs(a, ac.kind.keyvalue.key, key)) {
                // A spelling the splice wrote is not one that stood before.
                if (surfaced or r.edit.beforeOffset(a.spanOf(ac.kind.keyvalue.key).start) == null) return false;
                surfaced = true;
                a_cur = a.ast.next(&ac);
                continue;
            }
        }
        const bc = b_cur orelse return surfaced and a_cur == null;
        const ac = a_cur orelse return false;
        if (!nodesEqualDeep(b, bc.id, a, ac.id)) return false;
        b_cur = b.ast.next(&bc);
        a_cur = a.ast.next(&ac);
        index += 1;
    }
}

/// Whether the key node `id` is the string `name` — the physical spelling a
/// path segment matches against while walking two parses side by side.
fn keyNodeIs(doc: Document, id: AST.Node.Id, name: []const u8) bool {
    const k = doc.ast.node(id).kind;
    return k == .string and std.mem.eql(u8, k.string, name);
}

// ── "only what was asked": the write guard ──────────────────────────────────
//
// A replace, an insert, a move and a reorder make the delete's promise in
// their own terms (`readsBack`): the document after the splice reads as the
// one before with one change to one container, and that change is what was
// asked for. The value written must read as the value asked for, which is
// its text read in the same format (`readingOf`), so nothing a format cannot
// spell is held against it: the text already says what the format makes of
// the value, a float written as an integer included. The container is named
// by a route of child indices rather than by the caller's path, so a key
// written twice on the way down is passed as the one the edit reached.

/// How the value or entry a write asked for reads: node `id` of `doc`, the
/// parse of `source`, an entry (key and value) when `entry`, else a value.
const Reading = struct {
    source: []u8,
    doc: Document,
    id: AST.Node.Id,
    entry: bool,

    fn deinit(r: Reading, allocator: std.mem.Allocator) void {
        r.doc.deinit(allocator);
        allocator.free(r.source);
    }

    /// The value read: the entry's, for an entry.
    fn value(r: Reading) Reading {
        if (!r.entry) return r;
        return .{ .source = r.source, .doc = r.doc, .id = r.doc.ast.node(r.id).kind.keyvalue.value, .entry = false };
    }
};

/// The one change a guarded write may make to the container its route
/// reaches. A `null` reading is a value that does not read on its own, an
/// alias say, and is taken as written.
const Write = union(enum) {
    /// The container is the node written, the document root.
    whole: ?Reading,
    /// The child at `at` now reads as `want`; an entry keeps its key.
    replaced: struct { at: usize, want: ?Reading },
    /// One child was added, reading as `want`: at `at` for an item, and
    /// anywhere for an entry, which a format may place where it sorts.
    added: struct { at: ?usize, want: ?Reading },
    /// The children are those before, child `i` the one that stood at
    /// `order[i]`.
    moved: []usize,
    /// Every mapping reads with the entries it had, in any order: a section
    /// moved whole, whose fragments the format gathers.
    regrouped,
};

/// Where a node stands: child `at` of `container`, the child being the
/// node's entry when it is an entry's value.
const ChildSlot = struct { container: AST.Node.Id, at: usize };

/// The `ChildSlot` of node `id`, or null for the root.
fn slotOf(doc: Document, id: AST.Node.Id) ?ChildSlot {
    const ast = &doc.ast;
    var child = id;
    var parent = ast.parentOf(id) orelse return null;
    if (ast.kindOf(parent) == .keyvalue) {
        child = parent;
        parent = ast.parentOf(parent) orelse return null;
    }
    const container = ast.node(parent);
    var cur = ast.child(&container) catch return null;
    var at: usize = 0;
    while (cur) |c| : (cur = ast.next(&c)) {
        if (c.id == child) return .{ .container = parent, .at = at };
        at += 1;
    }
    return null;
}

/// The child indices from the root down to node `id`, each a child of the
/// container the ones before reach (an entry's value, for an entry).
/// Caller owns the slice.
fn routeTo(allocator: std.mem.Allocator, doc: Document, id: AST.Node.Id) ![]usize {
    var route: std.ArrayList(usize) = .empty;
    errdefer route.deinit(allocator);
    var cur = id;
    while (slotOf(doc, cur)) |slot| {
        try route.append(allocator, slot.at);
        cur = slot.container;
    }
    if (cur != doc.ast.root) return error.NotFound;
    std.mem.reverse(usize, route.items);
    return route.toOwnedSlice(allocator);
}

/// Whether `after` (node `aid`) is `before` (node `bid`) with only `write`
/// made to the container at the end of `route`: every node the route passes
/// on the way down, and every sibling of those, must be as it was.
fn writtenOnlyAt(b: Document, bid: AST.Node.Id, a: Document, aid: AST.Node.Id, route: []const usize, write: Write) bool {
    const bn = b.ast.node(bid);
    const an = a.ast.node(aid);
    if (route.len == 0) return containerWritten(b, bn, a, an, write);
    if (std.meta.activeTag(bn.kind) != std.meta.activeTag(an.kind)) return false;
    var b_cur = (b.ast.child(&bn) catch return false);
    var a_cur = (a.ast.child(&an) catch return false);
    var index: usize = 0;
    while (b_cur) |bc| : ({
        b_cur = b.ast.next(&bc);
        index += 1;
    }) {
        const ac = a_cur orelse return false;
        a_cur = a.ast.next(&ac);
        if (index != route[0]) {
            if (!nodesEqualDeep(b, bc.id, a, ac.id)) return false;
        } else if (bc.kind == .keyvalue) {
            if (ac.kind != .keyvalue or !nodesEqualDeep(b, bc.kind.keyvalue.key, a, ac.kind.keyvalue.key)) return false;
            if (!writtenOnlyAt(b, bc.kind.keyvalue.value, a, ac.kind.keyvalue.value, route[1..], write)) return false;
        } else if (!writtenOnlyAt(b, bc.id, a, ac.id, route[1..], write)) return false;
    }
    return a_cur == null;
}

/// Whether the container `an` is `bn` with `write` made to it.
fn containerWritten(b: Document, bn: AST.Node, a: Document, an: AST.Node, write: Write) bool {
    switch (write) {
        .whole => |want| return if (want) |w| readsAs(w, a, an.id) else true,
        .regrouped => return nodesEqualUnordered(b, bn.id, a, an.id),
        .replaced => |r| {
            if (std.meta.activeTag(bn.kind) != std.meta.activeTag(an.kind)) return false;
            var b_cur = (b.ast.child(&bn) catch return false);
            var a_cur = (a.ast.child(&an) catch return false);
            var index: usize = 0;
            while (b_cur) |bc| : ({
                b_cur = b.ast.next(&bc);
                index += 1;
            }) {
                const ac = a_cur orelse return false;
                a_cur = a.ast.next(&ac);
                if (index != r.at) {
                    if (!nodesEqualDeep(b, bc.id, a, ac.id)) return false;
                    continue;
                }
                if (bc.kind == .keyvalue and (ac.kind != .keyvalue or
                    !nodesEqualDeep(b, bc.kind.keyvalue.key, a, ac.kind.keyvalue.key))) return false;
                if (r.want) |w| if (!readsAs(w, a, ac.id)) return false;
            }
            return a_cur == null;
        },
        .added => |r| return childrenGainedOne(b, bn, a, an, r.at, r.want),
        .moved => |order| {
            if (std.meta.activeTag(bn.kind) != std.meta.activeTag(an.kind)) return false;
            if ((seqLen(b, bn) catch return false) != order.len) return false;
            var a_cur = (a.ast.child(&an) catch return false);
            for (order) |from| {
                const ac = a_cur orelse return false;
                a_cur = a.ast.next(&ac);
                var bc = (b.ast.child(&bn) catch return false) orelse return false;
                for (0..from) |_| bc = b.ast.next(&bc) orelse return false;
                if (!nodesEqualDeep(b, bc.id, a, ac.id)) return false;
            }
            return a_cur == null;
        },
    }
}

/// Whether the children of `an` are those of `bn`, each unchanged and in
/// its order, with one more: at `at`, or (null) anywhere, reading as `want`.
/// A container that read as empty (`readsEmpty`) may come back as the
/// container its first child makes of it: YAML's `k:` given an entry is a
/// mapping, ZON's `.{}` given an item a tuple.
fn childrenGainedOne(b: Document, bn: AST.Node, a: Document, an: AST.Node, at: ?usize, want: ?Reading) bool {
    if (an.kind != .mapping and an.kind != .sequence) return false;
    const same = std.meta.activeTag(bn.kind) == std.meta.activeTag(an.kind);
    if (!same and !readsEmpty(b, bn)) return false;
    if (same and at == null and (seqLen(b, bn) catch return false) == (seqLen(a, an) catch return false))
        return if (want) |w| entryMerged(b, bn, a, an, w) else false;
    var b_cur = if (same) (b.ast.child(&bn) catch return false) else null;
    var a_cur = (a.ast.child(&an) catch return false);
    var index: usize = 0;
    var extra: ?AST.Node = null;
    while (a_cur) |ac| : ({
        a_cur = a.ast.next(&ac);
        index += 1;
    }) {
        // The first child that is not the next one before is the new one;
        // one equal to its neighbour is either, and reads the same.
        if (extra == null) {
            const new = if (at) |i| index == i else if (b_cur) |bc| !nodesEqualDeep(b, bc.id, a, ac.id) else true;
            if (new) {
                extra = ac;
                continue;
            }
        }
        const bc = b_cur orelse return false;
        if (!nodesEqualDeep(b, bc.id, a, ac.id)) return false;
        b_cur = b.ast.next(&bc);
    }
    const added = extra orelse return false;
    if (b_cur != null) return false;
    return if (want) |w| readsAs(w, a, added.id) else true;
}

/// Whether the mapping `an` is `bn` with the entry `want` written into an
/// entry it already has: a dotted key names tables it passes through, and
/// TOML's `t.x = []` beside `t.k = 1` adds `x` to the table `t`, which
/// gains it as `childrenGainedOne` says. Every other entry is unchanged.
fn entryMerged(b: Document, bn: AST.Node, a: Document, an: AST.Node, want: Reading) bool {
    if (!want.entry or bn.kind != .mapping) return false;
    const wkv = want.doc.ast.node(want.id);
    if (wkv.kind != .keyvalue) return false;
    const inner = want.doc.ast.node(wkv.kind.keyvalue.value);
    if (inner.kind != .mapping) return false;
    const only = (want.doc.ast.child(&inner) catch return false) orelse return false;
    if (want.doc.ast.next(&only) != null) return false;
    var b_cur = (b.ast.child(&bn) catch return false);
    var a_cur = (a.ast.child(&an) catch return false);
    var merged = false;
    while (b_cur) |bc| : (b_cur = b.ast.next(&bc)) {
        const ac = a_cur orelse return false;
        a_cur = a.ast.next(&ac);
        if (nodesEqualDeep(b, bc.id, a, ac.id)) continue;
        if (merged or bc.kind != .keyvalue or ac.kind != .keyvalue) return false;
        if (!nodesEqualDeep(b, bc.kind.keyvalue.key, a, ac.kind.keyvalue.key) or
            !nodesEqualDeep(want.doc, wkv.kind.keyvalue.key, a, ac.kind.keyvalue.key)) return false;
        const deeper: Reading = .{ .source = want.source, .doc = want.doc, .id = only.id, .entry = true };
        if (!childrenGainedOne(b, b.ast.node(bc.kind.keyvalue.value), a, a.ast.node(ac.kind.keyvalue.value), null, deeper)) return false;
        merged = true;
    }
    return merged and a_cur == null;
}

/// Whether node `aid` of `a` reads as `want`: the whole entry, or the value
/// (an entry's value, for an entry). Two values that each read as empty
/// (`readsEmpty`) are equal: the format reads an empty container one way
/// wherever it is written.
fn readsAs(want: Reading, a: Document, aid: AST.Node.Id) bool {
    const node = a.ast.node(aid);
    if (want.entry) return nodesEqualDeep(want.doc, want.id, a, aid);
    const value = if (node.kind == .keyvalue) a.ast.node(node.kind.keyvalue.value) else node;
    if (readsEmpty(want.doc, want.doc.ast.node(want.id)) and readsEmpty(a, value)) return true;
    return nodesEqualDeep(want.doc, want.id, a, value.id);
}

/// Whether nothing but whitespace stands in `source` outside `span`.
fn onlyBlankAround(source: []const u8, span: Span) bool {
    const ws = " \t\r\n";
    return std.mem.trim(u8, source[0..span.start], ws).len == 0 and
        std.mem.trim(u8, source[span.end..], ws).len == 0;
}

/// `nodesEqualDeep`, with a mapping's entries matched in any order, each
/// to one of the other's: the reading a section move must leave alone.
fn nodesEqualUnordered(b: Document, bid: AST.Node.Id, a: Document, aid: AST.Node.Id) bool {
    const bn = b.ast.node(bid);
    const an = a.ast.node(aid);
    if (std.meta.activeTag(bn.kind) != std.meta.activeTag(an.kind)) return false;
    switch (bn.kind) {
        .sequence => {
            var b_cur = (b.ast.child(&bn) catch return false);
            var a_cur = (a.ast.child(&an) catch return false);
            while (b_cur) |bc| {
                const ac = a_cur orelse return false;
                if (!nodesEqualUnordered(b, bc.id, a, ac.id)) return false;
                b_cur = b.ast.next(&bc);
                a_cur = a.ast.next(&ac);
            }
            return a_cur == null;
        },
        .mapping => {
            if ((seqLen(b, bn) catch return false) != (seqLen(a, an) catch return false)) return false;
            // Each entry as many times on either side: with the counts equal,
            // the entries pair up one to one.
            var b_cur = (b.ast.child(&bn) catch return false);
            while (b_cur) |bc| : (b_cur = b.ast.next(&bc)) {
                if (countEqual(b, bn, bc.id, b) != countEqual(a, an, bc.id, b)) return false;
            }
            return true;
        },
        .keyvalue => |kv| return nodesEqualUnordered(b, kv.key, a, an.kind.keyvalue.key) and
            nodesEqualUnordered(b, kv.value, a, an.kind.keyvalue.value),
        else => return bn.kind.eql(an.kind),
    }
}

/// How many children of `container` (in `doc`) equal node `id` of `of`, as
/// `nodesEqualUnordered` compares.
fn countEqual(doc: Document, container: AST.Node, id: AST.Node.Id, of: Document) usize {
    var n: usize = 0;
    var cur = doc.ast.child(&container) catch return 0;
    while (cur) |c| : (cur = doc.ast.next(&c)) {
        if (nodesEqualUnordered(of, id, doc, c.id)) n += 1;
    }
    return n;
}

/// Drop a single trailing '\n' (the serializer ends every value with one).
fn stripTrailingNewline(text: []const u8) []const u8 {
    if (text.len > 0 and text[text.len - 1] == '\n') return text[0 .. text.len - 1];
    return text;
}

/// Whether a closed-container format's container closes on the line its last
/// child ends on (plist's `<key>o</key><dict>…</dict>`, OpenStep's
/// `o = {a = 1; };`, `(a, b)`): its span ends on its `close` token, after
/// that child and no later than the line (`insert_at`, the line end an
/// append splices at). Such a container has no line of its own to append
/// after, and an entry or item there lands in the container around it. A
/// braceless root (an OpenStep `.strings` file) ends at the end of input,
/// not on a close, and is appended to like any flat root.
fn closesOnLastLine(source: []const u8, span: Span, last: Span, close: []const u8, insert_at: usize) bool {
    if (span.end <= last.end or span.end > insert_at) return false;
    return close.len > 0 and std.mem.endsWith(u8, source[0..span.end], close);
}

/// Whether `gap` — the bytes between a line's indent and a token on it — is
/// one or more sequence item markers and the blanks after them (`- `, or
/// `- - ` for an item of an item), which is when the token's siblings align
/// under the token rather than under the line's indent. See `indentAt`.
fn onlyItemMarkers(gap: []const u8, seq_item_marker: []const u8) bool {
    const marker = std.mem.trim(u8, seq_item_marker, " \t");
    if (marker.len == 0) return false;
    var i: usize = 0;
    var saw = false;
    while (i < gap.len) {
        if (gap[i] == ' ' or gap[i] == '\t') {
            i += 1;
        } else if (std.mem.startsWith(u8, gap[i..], marker)) {
            i += marker.len;
            saw = true;
        } else return false;
    }
    return saw;
}

/// Append `value_text` to `out`, re-indented so its lines sit under `indent`.
/// The first line is emitted verbatim (it follows `key: ` or `- `); every
/// subsequent non-blank line is prefixed with the `indent` bytes, preserving
/// the serializer's own relative indentation. One trailing '\n' is stripped.
fn reindentInto(out: *std.ArrayList(u8), allocator: std.mem.Allocator, value_text: []const u8, indent: []const u8) !void {
    const text = stripTrailingNewline(value_text);
    var it = std.mem.splitScalar(u8, text, '\n');
    var first = true;
    while (it.next()) |line| {
        if (!first) {
            try out.append(allocator, '\n');
            if (line.len > 0) try out.appendSlice(allocator, indent);
        }
        try out.appendSlice(allocator, line);
        first = false;
    }
}

// ── Comment-editing tests ──────────────────────────────────────────────────
const testing = std.testing;

fn expectCommentEdit(
    comptime Lang: type,
    format: Lang.Type,
    input: []const u8,
    expected: []const u8,
    op: enum { leading, trailing },
    path: []const AST.PathSegment,
    text: []const u8,
) !void {
    var ed: Editor(Lang) = .{ .allocator = testing.allocator, .format = format };
    try ed.init(input);
    defer ed.deinit();
    switch (op) {
        .leading => try ed.addLeadingComment(path, text),
        .trailing => try ed.setTrailingComment(path, text),
    }
    try testing.expectEqualStrings(expected, ed.source.items);
}

test "addLeadingComment inserts an own-line comment above a YAML key" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    try expectCommentEdit(Yaml, .v1_2_2, "a: 1\nb: 2\n", "a: 1\n# note\nb: 2\n", .leading, &.{.{ .key = "b" }}, "note");
}

test "addLeadingComment matches indentation and lands nearest the key" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // Nested key: comment takes the key's 2-space indent and sits just above it,
    // below the pre-existing comment.
    try expectCommentEdit(
        Yaml,
        .v1_2_2,
        "outer:\n  # kept\n  inner: 1\n",
        "outer:\n  # kept\n  # new\n  inner: 1\n",
        .leading,
        &.{ .{ .key = "outer" }, .{ .key = "inner" } },
        "new",
    );
}

test "setTrailingComment appends and then replaces a YAML same-line comment" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    try expectCommentEdit(Yaml, .v1_2_2, "a: 1\n", "a: 1 # done\n", .trailing, &.{.{ .key = "a" }}, "done");
    // Re-setting replaces the existing trailing comment rather than nesting it.
    try expectCommentEdit(Yaml, .v1_2_2, "a: 1 # old\n", "a: 1 # new\n", .trailing, &.{.{ .key = "a" }}, "new");
}

// A CRLF line's content ends before its CR: a new trailing comment goes there,
// and the line keeps its CRLF. Spliced after the CR, it read back in YAML as
// the string "1\r".
test "setTrailingComment on a CRLF line goes before the CR" {
    if (comptime build_options.lang_yaml) {
        try expectCommentEdit(Yaml, .v1_2_2, "a: 1\r\nb: 2\r\n", "a: 1 # note\r\nb: 2\r\n", .trailing, &.{.{ .key = "a" }}, "note");
        try expectCommentEdit(Yaml, .v1_2_2, "a: 1 # old\r\nb: 2\r\n", "a: 1 # new\r\nb: 2\r\n", .trailing, &.{.{ .key = "a" }}, "new");
        try expectCommentDelete(Yaml, .v1_2_2, "a: 1 # gone\r\nb: 2\r\n", "a: 1\r\nb: 2\r\n", .trailing, &.{.{ .key = "a" }});
    }
    if (comptime build_options.lang_zon)
        try expectCommentEdit(Zon, .ZON, ".{\r\n    .a = 1,\r\n}\r\n", ".{\r\n    .a = 1, // note\r\n}\r\n", .trailing, &.{.{ .key = "a" }}, "note");
}

// A block scalar's body runs through its last line's newline, so the end of the
// line its span ends on is the next entry's: the comment went there and read
// back as `b`'s (`b: 1 # note`). It goes on the header line, and get and
// delete find it there rather than reaching `b`'s.
test "a trailing comment on a YAML block scalar goes on its header line" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const a: []const AST.PathSegment = &.{.{ .key = "a" }};
    const headers = [_][]const u8{ "|", ">", "|-", ">+", "|2", ">1-", "|+2" };
    for (headers) |h| {
        var in_buf: [64]u8 = undefined;
        var out_buf: [64]u8 = undefined;
        const input = try std.fmt.bufPrint(&in_buf, "a: {s}\n  text\n  more\n\nb: 1\n", .{h});
        const expected = try std.fmt.bufPrint(&out_buf, "a: {s} # note\n  text\n  more\n\nb: 1\n", .{h});
        try expectCommentEdit(Yaml, .v1_2_2, input, expected, .trailing, a, "note");
    }
    // Replaced in place, read back, and deleted from the header line; the
    // next entry's own comment is not the block scalar's.
    try expectCommentEdit(Yaml, .v1_2_2, "a: |  # old\n  text\nb: 1 # b's\n", "a: | # new\n  text\nb: 1 # b's\n", .trailing, a, "new");
    try expectCommentDelete(Yaml, .v1_2_2, "a: | # gone\n  text\nb: 1 # b's\n", "a: |\n  text\nb: 1 # b's\n", .trailing, a);
    try expectCommentDelete(Yaml, .v1_2_2, "a: |\n  text\nb: 1 # b's\n", "a: |\n  text\nb: 1 # b's\n", .trailing, a);
    {
        var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
        try ed.init("a: >- # kept\n  text\nb: 1 # b's\n");
        defer ed.deinit();
        const got = (try ed.getTrailingComment(a)).?;
        defer testing.allocator.free(got);
        try testing.expectEqualStrings("kept", got);
    }
    {
        var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
        try ed.init("a: |\n  text\nb: 1 # b's\n");
        defer ed.deinit();
        try testing.expectEqual(null, try ed.getTrailingComment(a));
    }
    // A sequence item's block scalar, and a CRLF header line.
    try expectCommentEdit(Yaml, .v1_2_2, "s:\n  - |\n    text\n  - x\n", "s:\n  - | # note\n    text\n  - x\n", .trailing, &.{ .{ .key = "s" }, .{ .index = 0 } }, "note");
    try expectCommentEdit(Yaml, .v1_2_2, "a: |\r\n  text\r\nb: 1\r\n", "a: | # note\r\n  text\r\nb: 1\r\n", .trailing, a, "note");
}

// A ZON `\\` line is string text up to its newline, so there is no place on it
// for a comment: one appended there parsed, as part of the string. The edit is
// refused and rolled back, LF or CRLF.
test "setTrailingComment refuses a ZON multiline string's line" {
    if (comptime !build_options.lang_zon) return error.SkipZigTest;
    const inputs = [_][]const u8{
        ".{\n    .ml =\n        \\\\two\n    ,\n}\n",
        ".{\r\n    .ml =\r\n        \\\\two\r\n    ,\r\n}\r\n",
        ".{\n    .ml =\n        \\\\two // kept\n    ,\n}\n",
    };
    for (inputs) |input| {
        var ed: Editor(Zon) = .{ .allocator = testing.allocator, .format = .ZON };
        try ed.init(input);
        defer ed.deinit();
        try testing.expectError(error.CommentInsideValue, ed.setTrailingComment(&.{.{ .key = "ml" }}, "note"));
        try testing.expectEqualStrings(input, ed.source.items);
    }
    // The `// kept` is the string's own text, inside its span: not a
    // trailing comment to delete.
    try expectCommentDelete(Zon, .ZON, inputs[2], inputs[2], .trailing, &.{.{ .key = "ml" }});
    // A one-line string on the same document still takes one.
    try expectCommentEdit(Zon, .ZON, ".{\n    .s = \"two\",\n}\n", ".{\n    .s = \"two\", // note\n}\n", .trailing, &.{.{ .key = "s" }}, "note");
}

test "addLeadingComment on TOML uses #" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    try expectCommentEdit(Toml, .TOML_1_1, "a = 1\nb = 2\n", "a = 1\n# note\nb = 2\n", .leading, &.{.{ .key = "b" }}, "note");
}

// This instantiation (plus every `Editor(Fig)` call below) is what pulls
// `Editor(Fig)`'s own methods into the test build's reachability graph — `zig
// test` analyzes a generic struct's methods only once something calls them, so
// an `Editor(Fig)` that nothing instantiates is an `Editor(Fig)` nothing
// type-checks. (`languages/figl/tests/editor.zig`'s `test` blocks are discovered through
// `fig.zig`'s own `test {}`, and carry the rest of `Editor(Fig)`'s coverage
// rather than duplicating it here; the same split holds for TOML above.)
test "addLeadingComment on fig uses # at the target's own marker depth" {
    if (comptime !build_options.lang_figl) return error.SkipZigTest;
    try expectCommentEdit(Fig, .Fig, "a = 1\nb = 2\n", "a = 1\n# note\nb = 2\n", .leading, &.{.{ .key = "b" }}, "note");
    try expectCommentEdit(
        Fig,
        .Fig,
        "database\n> pool\n> > size = 10\n",
        "database\n> pool\n> > # note\n> > size = 10\n",
        .leading,
        &.{ .{ .key = "database" }, .{ .key = "pool" }, .{ .key = "size" } },
        "note",
    );
}

test "comment ops on JSONC use // and respect indentation" {
    try expectCommentEdit(
        json.Language,
        .JSONC,
        "{\n  \"a\": 1\n}",
        "{\n  // note\n  \"a\": 1\n}",
        .leading,
        &.{.{ .key = "a" }},
        "note",
    );
}

test "set inserts into a pretty-printed JSON object on its own line, indented" {
    var ed: Editor(json.Language) = .{ .allocator = testing.allocator, .format = .JSON };
    try ed.init("{\n  \"a\": 1,\n  \"b\": 2\n}");
    defer ed.deinit();
    try ed.set(&.{.{ .key = "c" }}, "3");
    try testing.expectEqualStrings("{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}", ed.source.items);
}

test "set keeps compact single-line JSON objects inline" {
    var ed: Editor(json.Language) = .{ .allocator = testing.allocator, .format = .JSON };
    try ed.init("{\"a\": 1, \"b\": 2}");
    defer ed.deinit();
    try ed.set(&.{.{ .key = "c" }}, "3");
    try testing.expectEqualStrings("{\"a\": 1, \"b\": 2, \"c\": 3}", ed.source.items);
}

test "comment ops are rejected for strict JSON" {
    var ed: Editor(json.Language) = .{ .allocator = testing.allocator, .format = .JSON };
    try ed.init("{\"a\":1}");
    defer ed.deinit();
    try testing.expectError(error.CommentsUnsupported, ed.addLeadingComment(&.{.{ .key = "a" }}, "x"));
    try testing.expectError(error.CommentsUnsupported, ed.setTrailingComment(&.{.{ .key = "a" }}, "x"));
}

test "multi-line leading comment becomes one line per row" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    try expectCommentEdit(Yaml, .v1_2_2, "a: 1\n", "# one\n# two\na: 1\n", .leading, &.{.{ .key = "a" }}, "one\ntwo");
}

test "setTrailingComment rejects a multi-line comment" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init("a: 1\n");
    defer ed.deinit();
    try testing.expectError(error.MultilineComment, ed.setTrailingComment(&.{.{ .key = "a" }}, "x\ny"));
}

fn expectCommentDelete(
    comptime Lang: type,
    format: Lang.Type,
    input: []const u8,
    expected: []const u8,
    op: enum { leading, trailing },
    path: []const AST.PathSegment,
) !void {
    var ed: Editor(Lang) = .{ .allocator = testing.allocator, .format = format };
    try ed.init(input);
    defer ed.deinit();
    switch (op) {
        .leading => try ed.deleteLeadingComments(path),
        .trailing => try ed.deleteTrailingComment(path),
    }
    try testing.expectEqualStrings(expected, ed.source.items);
}

test "deleteLeadingComments removes the owned block above a YAML key" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // Only the block touching the key goes; a blank line breaks ownership, so the
    // earlier comment (above the blank) stays.
    try expectCommentDelete(
        Yaml,
        .v1_2_2,
        "# top\n\n# a\n# b\nkey: 1\n",
        "# top\n\nkey: 1\n",
        .leading,
        &.{.{ .key = "key" }},
    );
    // No leading comment → no-op.
    try expectCommentDelete(Yaml, .v1_2_2, "key: 1\n", "key: 1\n", .leading, &.{.{ .key = "key" }});
}

test "deleteTrailingComment removes a same-line comment (YAML/JSONC), else no-op" {
    if (comptime build_options.lang_yaml)
        try expectCommentDelete(Yaml, .v1_2_2, "a: 1 # gone\nb: 2\n", "a: 1\nb: 2\n", .trailing, &.{.{ .key = "a" }});
    // No trailing comment → no-op.
    if (comptime build_options.lang_yaml)
        try expectCommentDelete(Yaml, .v1_2_2, "a: 1\n", "a: 1\n", .trailing, &.{.{ .key = "a" }});
    // JSONC `//` trailing.
    try expectCommentDelete(json.Language, .JSONC, "{\n  \"a\": 1 // x\n}", "{\n  \"a\": 1\n}", .trailing, &.{.{ .key = "a" }});
}

test "ZON owned-comment scan uses // (comments fix)" {
    if (comptime !build_options.lang_zon) return error.SkipZigTest;
    try expectCommentDelete(
        Zon,
        .ZON,
        ".{\n    // note\n    .n = 3,\n}\n",
        ".{\n    .n = 3,\n}\n",
        .leading,
        &.{.{ .key = "n" }},
    );
}

test "comment delete ops are rejected for strict JSON" {
    var ed: Editor(json.Language) = .{ .allocator = testing.allocator, .format = .JSON };
    try ed.init("{\"a\":1}");
    defer ed.deinit();
    try testing.expectError(error.CommentsUnsupported, ed.deleteLeadingComments(&.{.{ .key = "a" }}));
    try testing.expectError(error.CommentsUnsupported, ed.deleteTrailingComment(&.{.{ .key = "a" }}));
}

fn expectCommentGet(
    comptime Lang: type,
    format: Lang.Type,
    input: []const u8,
    /// `null` asserts the comment is ABSENT; a string asserts it is present with
    /// exactly those bytes (`""` = a present-but-empty bare marker).
    expected: ?[]const u8,
    op: enum { leading, trailing },
    path: []const AST.PathSegment,
) !void {
    var ed: Editor(Lang) = .{ .allocator = testing.allocator, .format = format };
    try ed.init(input);
    defer ed.deinit();
    const got = switch (op) {
        .leading => try ed.getLeadingComment(path),
        .trailing => try ed.getTrailingComment(path),
    };
    defer if (got) |g| testing.allocator.free(g);
    if (expected) |want| {
        try testing.expect(got != null);
        try testing.expectEqualStrings(want, got.?);
    } else {
        try testing.expect(got == null);
    }
}

test "getLeadingComment returns the owned block above a key, markers stripped" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    try expectCommentGet(Yaml, .v1_2_2, "# one\n# two\na: 1\n", "one\ntwo", .leading, &.{.{ .key = "a" }});
    // No block above → absent (null).
    try expectCommentGet(Yaml, .v1_2_2, "a: 1\nb: 2\n", null, .leading, &.{.{ .key = "b" }});
}

test "getTrailingComment returns the same-line comment, marker stripped" {
    if (comptime build_options.lang_yaml) {
        try expectCommentGet(Yaml, .v1_2_2, "a: 1 # done\n", "done", .trailing, &.{.{ .key = "a" }});
        // No trailing comment → absent (null).
        try expectCommentGet(Yaml, .v1_2_2, "a: 1\n", null, .trailing, &.{.{ .key = "a" }});
    }
    // JSONC `//` trailing.
    try expectCommentGet(json.Language, .JSONC, "{\n  \"a\": 1 // x\n}", "x", .trailing, &.{.{ .key = "a" }});
}

test "trailing comment on a block-collection key rides the key line" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const seq = "contents: # note\n- one\n- two\n";
    // get: the comment after the colon on the key's line, not after the last item.
    try expectCommentGet(Yaml, .v1_2_2, seq, "note", .trailing, &.{.{ .key = "contents" }});
    // set: replaces the key-line comment in place (does not append after `two`).
    try expectCommentEdit(Yaml, .v1_2_2, seq, "contents: # new\n- one\n- two\n", .trailing, &.{.{ .key = "contents" }}, "new");
    // set on a block key with no existing comment lands on the key line.
    try expectCommentEdit(Yaml, .v1_2_2, "k:\n- a\n- b\n", "k: # added\n- a\n- b\n", .trailing, &.{.{ .key = "k" }}, "added");
    // delete: removes the key-line comment.
    try expectCommentDelete(Yaml, .v1_2_2, seq, "contents:\n- one\n- two\n", .trailing, &.{.{ .key = "contents" }});
}

test "trailing comment on a parent key ignores a child's same-line comment" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // The `# bc` belongs to child `b`; the parent `a` has no trailing comment.
    try expectCommentGet(Yaml, .v1_2_2, "a:\n  b: 1 # bc\n", null, .trailing, &.{.{ .key = "a" }});
    try expectCommentGet(Yaml, .v1_2_2, "a:\n  b: 1 # bc\n", "bc", .trailing, &.{ .{ .key = "a" }, .{ .key = "b" } });
}

test "getLeadingComment round-trips an empty comment line" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // A bare `#` (no text) decodes to an empty line within the block.
    try expectCommentGet(Yaml, .v1_2_2, "# one\n#\n# three\na: 1\n", "one\n\nthree", .leading, &.{.{ .key = "a" }});
}

test "get distinguishes a present-but-empty comment from an absent one" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // A bare `#` is PRESENT with empty text → "" (not null).
    try expectCommentGet(Yaml, .v1_2_2, "a: 1 #\n", "", .trailing, &.{.{ .key = "a" }});
    try expectCommentGet(Yaml, .v1_2_2, "#\na: 1\n", "", .leading, &.{.{ .key = "a" }});
    // No marker at all → absent (null).
    try expectCommentGet(Yaml, .v1_2_2, "a: 1\n", null, .trailing, &.{.{ .key = "a" }});
}

test "get comment ops are rejected for strict JSON" {
    var ed: Editor(json.Language) = .{ .allocator = testing.allocator, .format = .JSON };
    try ed.init("{\"a\":1}");
    defer ed.deinit();
    try testing.expectError(error.CommentsUnsupported, ed.getLeadingComment(&.{.{ .key = "a" }}));
    try testing.expectError(error.CommentsUnsupported, ed.getTrailingComment(&.{.{ .key = "a" }}));
}

// ── Flow elements own no comment (see `commentsUnanchored`) ────────────────
//
// An element or entry of a ONE-LINE flow collection sits on its parent's line,
// so the block above that line and the comment at its end are the PARENT's.
// Every op used to reach them through the item: the read returned the parent's
// text, the delete removed it, and the add spliced a comment onto the parent's
// line (on fig, whose indent is the raw line prefix, it duplicated `members = [`
// as well). Per § 3.4/§ 6.3 a comment written inside a flow collection is
// discarded at parse, so there is nothing for an item to own.
//
// Each format's test asserts all three leading ops on such an item, that the
// parent still answers with its own block, and that the two shapes whose
// elements DO begin their own lines — a multi-line flow collection, and a block
// sequence — are untouched.

/// `addLeadingComment` at `path` is refused as unanchored, and the source is
/// left byte-identical.
fn expectAddLeadingUnanchored(
    comptime Lang: type,
    format: Lang.Type,
    input: []const u8,
    path: []const AST.PathSegment,
) !void {
    var ed: Editor(Lang) = .{ .allocator = testing.allocator, .format = format };
    try ed.init(input);
    defer ed.deinit();
    try testing.expectError(error.CommentsUnanchored, ed.addLeadingComment(path, "new"));
    try testing.expectEqualStrings(input, ed.source.items);
}

/// `setTrailingComment` at `path` is refused as unanchored, and the source is
/// left byte-identical.
fn expectSetTrailingUnanchored(
    comptime Lang: type,
    format: Lang.Type,
    input: []const u8,
    path: []const AST.PathSegment,
) !void {
    var ed: Editor(Lang) = .{ .allocator = testing.allocator, .format = format };
    try ed.init(input);
    defer ed.deinit();
    try testing.expectError(error.CommentsUnanchored, ed.setTrailingComment(path, "new"));
    try testing.expectEqualStrings(input, ed.source.items);
}

const flow_item0: []const AST.PathSegment = &.{ .{ .key = "members" }, .{ .index = 0 } };
const flow_item1: []const AST.PathSegment = &.{ .{ .key = "members" }, .{ .index = 1 } };
const flow_members: []const AST.PathSegment = &.{.{ .key = "members" }};
const flow_entry: []const AST.PathSegment = &.{ .{ .key = "nested" }, .{ .key = "k" } };

test "TOML: a flow item on its parent's line owns no leading comment" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    const one_line = "# above members\nmembers = [\"a\", \"b\"]\n";
    // Read: null through either item; the block is still `members`' own.
    try expectCommentGet(Toml, .TOML_1_1, one_line, null, .leading, flow_item0);
    try expectCommentGet(Toml, .TOML_1_1, one_line, null, .leading, flow_item1);
    try expectCommentGet(Toml, .TOML_1_1, one_line, "above members", .leading, flow_members);
    // Delete: a no-op, not a delete of the parent's block.
    try expectCommentDelete(Toml, .TOML_1_1, one_line, one_line, .leading, flow_item0);
    // Add: refused; there is no line the item owns to put one on.
    try expectAddLeadingUnanchored(Toml, .TOML_1_1, one_line, flow_item0);
    // An inline table's entry shares the line the same way.
    const inline_tbl = "# above nested\nnested = { k = \"v\" }\n";
    try expectCommentGet(Toml, .TOML_1_1, inline_tbl, null, .leading, flow_entry);
    try expectCommentDelete(Toml, .TOML_1_1, inline_tbl, inline_tbl, .leading, flow_entry);
    try expectAddLeadingUnanchored(Toml, .TOML_1_1, inline_tbl, flow_entry);
    // A multi-line array's items each begin their own line: unchanged.
    const multi = "# above members\nmembers = [\n  \"a\",\n  \"b\",\n]\n";
    try expectCommentGet(Toml, .TOML_1_1, multi, null, .leading, flow_item0);
    try expectCommentDelete(Toml, .TOML_1_1, multi, multi, .leading, flow_item0);
    try expectCommentEdit(
        Toml,
        .TOML_1_1,
        multi,
        "# above members\nmembers = [\n  # new\n  \"a\",\n  \"b\",\n]\n",
        .leading,
        flow_item0,
        "new",
    );
}

test "YAML: a flow item on its parent's line owns no leading comment" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    const one_line = "# above\nmembers: [a, b]\n";
    try expectCommentGet(Yaml, .v1_2_2, one_line, null, .leading, flow_item0);
    try expectCommentGet(Yaml, .v1_2_2, one_line, null, .leading, flow_item1);
    try expectCommentGet(Yaml, .v1_2_2, one_line, "above", .leading, flow_members);
    try expectCommentDelete(Yaml, .v1_2_2, one_line, one_line, .leading, flow_item0);
    try expectAddLeadingUnanchored(Yaml, .v1_2_2, one_line, flow_item0);
    // A flow mapping's entry shares the line the same way.
    const flow_map = "# above nested\nnested: {k: v}\n";
    try expectCommentGet(Yaml, .v1_2_2, flow_map, null, .leading, flow_entry);
    try expectCommentDelete(Yaml, .v1_2_2, flow_map, flow_map, .leading, flow_entry);
    try expectAddLeadingUnanchored(Yaml, .v1_2_2, flow_map, flow_entry);
    // A block sequence's items each begin their own line: unchanged.
    const block = "# above\nmembers:\n  - a\n  - b\n";
    try expectCommentGet(Yaml, .v1_2_2, block, null, .leading, flow_item0);
    try expectCommentDelete(Yaml, .v1_2_2, block, block, .leading, flow_item0);
    try expectCommentEdit(Yaml, .v1_2_2, block, "# above\nmembers:\n  # new\n  - a\n  - b\n", .leading, flow_item0, "new");
}

test "fig: a flow item on its parent's line owns no leading comment" {
    if (comptime !build_options.lang_figl) return error.SkipZigTest;
    const one_line = "# above\nmembers = [a, b]\n";
    try expectCommentGet(Fig, .Fig, one_line, null, .leading, flow_item0);
    try expectCommentGet(Fig, .Fig, one_line, null, .leading, flow_item1);
    try expectCommentGet(Fig, .Fig, one_line, "above", .leading, flow_members);
    try expectCommentDelete(Fig, .Fig, one_line, one_line, .leading, flow_item0);
    // Refused rather than spliced: fig's indent is the raw line prefix, so the
    // add used to emit `members = [# new` and duplicate the key line.
    try expectAddLeadingUnanchored(Fig, .Fig, one_line, flow_item0);
    // A flow object's entry shares the line the same way.
    const flow_obj = "# above nested\nnested = { k = v }\n";
    try expectCommentGet(Fig, .Fig, flow_obj, null, .leading, flow_entry);
    try expectCommentDelete(Fig, .Fig, flow_obj, flow_obj, .leading, flow_entry);
    try expectAddLeadingUnanchored(Fig, .Fig, flow_obj, flow_entry);
    // A multi-line (stacked flow) list's items each begin their own line.
    const multi = "# above\nmembers = [\n  a,\n  b,\n]\n";
    try expectCommentGet(Fig, .Fig, multi, null, .leading, flow_item0);
    try expectCommentDelete(Fig, .Fig, multi, multi, .leading, flow_item0);
    try expectCommentEdit(Fig, .Fig, multi, "# above\nmembers = [\n  # new\n  a,\n  b,\n]\n", .leading, flow_item0, "new");
}

test "JSONC: a flow item on its parent's line owns no leading comment" {
    const one_line = "{\n  // above\n  \"members\": [\"a\", \"b\"]\n}";
    try expectCommentGet(json.Language, .JSONC, one_line, null, .leading, flow_item0);
    try expectCommentGet(json.Language, .JSONC, one_line, null, .leading, flow_item1);
    try expectCommentGet(json.Language, .JSONC, one_line, "above", .leading, flow_members);
    try expectCommentDelete(json.Language, .JSONC, one_line, one_line, .leading, flow_item0);
    try expectAddLeadingUnanchored(json.Language, .JSONC, one_line, flow_item0);
    // A one-line nested object's entry shares the line the same way.
    const nested = "{\n  // above\n  \"nested\": { \"k\": \"v\" }\n}";
    try expectCommentGet(json.Language, .JSONC, nested, null, .leading, flow_entry);
    try expectCommentDelete(json.Language, .JSONC, nested, nested, .leading, flow_entry);
    try expectAddLeadingUnanchored(json.Language, .JSONC, nested, flow_entry);
    // Pretty-printed, one element per line: unchanged.
    const multi = "{\n  // above\n  \"members\": [\n    \"a\",\n    \"b\"\n  ]\n}";
    try expectCommentGet(json.Language, .JSONC, multi, null, .leading, flow_item0);
    try expectCommentDelete(json.Language, .JSONC, multi, multi, .leading, flow_item0);
    try expectCommentEdit(
        json.Language,
        .JSONC,
        multi,
        "{\n  // above\n  \"members\": [\n    // new\n    \"a\",\n    \"b\"\n  ]\n}",
        .leading,
        flow_item0,
        "new",
    );
}

test "a flow item on its parent's line owns no trailing comment either" {
    // The same defect, one line to the right: the comment after `]` closes the
    // parent's line, and the item's window used to run right through the rest
    // of the collection to reach it.
    if (comptime build_options.lang_toml) {
        const src = "members = [\"a\", \"b\"] # note\n";
        try expectCommentGet(Toml, .TOML_1_1, src, null, .trailing, flow_item0);
        try expectCommentGet(Toml, .TOML_1_1, src, "note", .trailing, flow_members);
        try expectCommentDelete(Toml, .TOML_1_1, src, src, .trailing, flow_item0);
        try expectSetTrailingUnanchored(Toml, .TOML_1_1, src, flow_item0);
    }
    if (comptime build_options.lang_yaml) {
        const src = "members: [a, b] # note\n";
        try expectCommentGet(Yaml, .v1_2_2, src, null, .trailing, flow_item0);
        try expectCommentGet(Yaml, .v1_2_2, src, "note", .trailing, flow_members);
        try expectCommentDelete(Yaml, .v1_2_2, src, src, .trailing, flow_item0);
        try expectSetTrailingUnanchored(Yaml, .v1_2_2, src, flow_item0);
    }
    if (comptime build_options.lang_figl) {
        const src = "members = [a, b] # note\n";
        try expectCommentGet(Fig, .Fig, src, null, .trailing, flow_item0);
        try expectCommentGet(Fig, .Fig, src, "note", .trailing, flow_members);
        try expectCommentDelete(Fig, .Fig, src, src, .trailing, flow_item0);
        try expectSetTrailingUnanchored(Fig, .Fig, src, flow_item0);
    }
    const src = "{\n  \"members\": [\"a\", \"b\"] // note\n}";
    try expectCommentGet(json.Language, .JSONC, src, null, .trailing, flow_item0);
    try expectCommentGet(json.Language, .JSONC, src, "note", .trailing, flow_members);
    try expectCommentDelete(json.Language, .JSONC, src, src, .trailing, flow_item0);
    try expectSetTrailingUnanchored(json.Language, .JSONC, src, flow_item0);
}

// ── dangling-anchor tests ───────────────────────────────────────────────────

/// Write a dangling comment at `path`, read it back, then delete it — the
/// three-op cycle the anchor exists for. Asserts the source after the write,
/// the text read back, and that the delete restores `input` byte-for-byte.
fn expectDanglingCycle(
    comptime Lang: type,
    format: Lang.Type,
    input: []const u8,
    path: []const AST.PathSegment,
    text: []const u8,
    after_add: []const u8,
) !void {
    var ed: Editor(Lang) = .{ .allocator = testing.allocator, .format = format };
    try ed.init(input);
    defer ed.deinit();
    try testing.expect((try ed.getDanglingComment(path)) == null);
    try ed.addDanglingComment(path, text);
    try testing.expectEqualStrings(after_add, ed.source.items);
    const got = (try ed.getDanglingComment(path)).?;
    defer testing.allocator.free(got);
    try testing.expectEqualStrings(text, got);
    try ed.deleteDanglingComments(path);
    try testing.expectEqualStrings(input, ed.source.items);
}

test "dangling comment round-trips at a YAML container's end and at the root" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // Nested: at the child depth of the container's body, after its last entry.
    try expectDanglingCycle(
        Yaml,
        .v1_2_2,
        "server:\n  port: 8080\nclient:\n  x: 1\n",
        &.{.{ .key = "server" }},
        "was: here",
        "server:\n  port: 8080\n  # was: here\nclient:\n  x: 1\n",
    );
    // The root (empty path): the run at the end of the document.
    try expectDanglingCycle(
        Yaml,
        .v1_2_2,
        "a: 1\nb: 2\n",
        &.{},
        "end of file",
        "a: 1\nb: 2\n# end of file\n",
    );
    // Multi-line text becomes one comment line per row.
    try expectDanglingCycle(Yaml, .v1_2_2, "a: 1\n", &.{}, "one\ntwo", "a: 1\n# one\n# two\n");
}

test "dangling comment round-trips in a TOML table, before any sub-table" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    try expectDanglingCycle(
        Toml,
        .TOML_1_1,
        "[server]\nport = 8080\n\n[client]\nx = 1\n",
        &.{.{ .key = "server" }},
        "note",
        "[server]\nport = 8080\n# note\n\n[client]\nx = 1\n",
    );
    // A table whose only child is a sub-table anchors after its header line —
    // where `toml/printer.zig` emits the run, before the sub-tables.
    try expectDanglingCycle(
        Toml,
        .TOML_1_1,
        "[a]\n[a.b]\nx = 1\n",
        &.{.{ .key = "a" }},
        "note",
        "[a]\n# note\n[a.b]\nx = 1\n",
    );
}

test "dangling comment round-trips at a fig container's marker depth" {
    if (comptime !build_options.lang_figl) return error.SkipZigTest;
    try expectDanglingCycle(
        Fig,
        .Fig,
        "database\n> host = local\n> port = 5432\n",
        &.{.{ .key = "database" }},
        "note",
        "database\n> host = local\n> port = 5432\n> # note\n",
    );
}

test "dangling comment round-trips inside a multi-line JSONC object" {
    // A `//` line before the closing brace is the object's dangling run; the
    // editor can address it even though the container is flow-spelled.
    try expectDanglingCycle(
        json.Language,
        .JSONC,
        "{\n  \"a\": 1\n}\n",
        &.{},
        "note",
        "{\n  \"a\": 1\n  // note\n}\n",
    );
}

test "dangling ops decline a scalar, a one-line flow container and strict JSON" {
    var ed: Editor(json.Language) = .{ .allocator = testing.allocator, .format = .JSONC };
    try ed.init("{\"a\": 1, \"b\": {\"c\": 2}}");
    defer ed.deinit();
    // No line of its own to sit on.
    try testing.expectError(error.UnsupportedShape, ed.getDanglingComment(&.{}));
    try testing.expectError(error.UnsupportedShape, ed.addDanglingComment(&.{}, "x"));
    // A scalar has no body to end.
    try testing.expectError(error.UnsupportedShape, ed.getDanglingComment(&.{.{ .key = "a" }}));

    var strict: Editor(json.Language) = .{ .allocator = testing.allocator, .format = .JSON };
    try strict.init("{\n  \"a\": 1\n}\n");
    defer strict.deinit();
    try testing.expectError(error.CommentsUnsupported, strict.getDanglingComment(&.{}));
    try testing.expectError(error.CommentsUnsupported, strict.addDanglingComment(&.{}, "x"));
    try testing.expectError(error.CommentsUnsupported, strict.deleteDanglingComments(&.{}));
}

test "addDanglingComment lands below a run already there" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init("a: 1\n# first\n");
    defer ed.deinit();
    try ed.addDanglingComment(&.{}, "second");
    try testing.expectEqualStrings("a: 1\n# first\n# second\n", ed.source.items);
    const got = (try ed.getDanglingComment(&.{})).?;
    defer testing.allocator.free(got);
    try testing.expectEqualStrings("first\nsecond", got);
}

test "a dedented comment is the next entry's leading block, not the container's dangling run" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init("server:\n  port: 8080\n# about client\nclient: 1\n");
    defer ed.deinit();
    // Spec § 3.4: shallower than the body's child depth → it stays pending for
    // the next sibling, and `server` has no dangling run at all.
    try testing.expect((try ed.getDanglingComment(&.{.{ .key = "server" }})) == null);
    const leading = (try ed.getLeadingComment(&.{.{ .key = "client" }})).?;
    defer testing.allocator.free(leading);
    try testing.expectEqualStrings("about client", leading);
}

test "a document with no trailing newline still takes a dangling comment" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init("a: 1");
    defer ed.deinit();
    try ed.addDanglingComment(&.{}, "note");
    try testing.expectEqualStrings("a: 1\n# note\n", ed.source.items);
}

// ── comment-out / uncomment tests ───────────────────────────────────────────

/// `commentOut` at `path`, assert the source, then bring it back through the
/// leading block of `next` and assert the source is `input` byte-for-byte.
fn expectCommentOutRoundTrip(
    comptime Lang: type,
    format: Lang.Type,
    input: []const u8,
    path: []const AST.PathSegment,
    commented: []const u8,
    next: []const AST.PathSegment,
    first_line: usize,
    line_count: usize,
) !void {
    var ed: Editor(Lang) = .{ .allocator = testing.allocator, .format = format };
    try ed.init(input);
    defer ed.deinit();
    try ed.commentOut(path);
    try testing.expectEqualStrings(commented, ed.source.items);
    // The node is gone from the tree: the entry is trivia now. (Asserted for a
    // key only — commenting out item `i` leaves whatever followed it at `i`.)
    if (path.len > 0 and std.meta.activeTag(path[path.len - 1]) == .key) {
        const parsed = try ed.getParsed();
        try testing.expectError(error.NotFound, parsed.ast.getNodeByPath(path));
    }
    try ed.uncommentLeading(next, first_line, line_count);
    try testing.expectEqualStrings(input, ed.source.items);
}

/// The same round trip for a LAST entry, which becomes the parent's dangling
/// run rather than any sibling's leading block.
fn expectCommentOutLastRoundTrip(
    comptime Lang: type,
    format: Lang.Type,
    input: []const u8,
    path: []const AST.PathSegment,
    commented: []const u8,
    container: []const AST.PathSegment,
    dangling_text: []const u8,
) !void {
    var ed: Editor(Lang) = .{ .allocator = testing.allocator, .format = format };
    try ed.init(input);
    defer ed.deinit();
    try ed.commentOut(path);
    try testing.expectEqualStrings(commented, ed.source.items);
    const got = (try ed.getDanglingComment(container)).?;
    defer testing.allocator.free(got);
    try testing.expectEqualStrings(dangling_text, got);
    var lines: usize = 1;
    for (dangling_text) |c| {
        if (c == '\n') lines += 1;
    }
    try ed.uncommentDangling(container, 0, lines);
    try testing.expectEqualStrings(input, ed.source.items);
}

test "commentOut then uncommentLeading round-trips a YAML entry" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    try expectCommentOutRoundTrip(
        Yaml,
        .v1_2_2,
        "server:\n  port: 8080\n  host: local\n",
        &.{ .{ .key = "server" }, .{ .key = "port" } },
        "server:\n  # port: 8080\n  host: local\n",
        &.{ .{ .key = "server" }, .{ .key = "host" } },
        0,
        1,
    );
}

test "commentOut leaves the entry's own leading block above it" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // `# why` is a note ON the note afterwards, and the uncomment addresses
    // line 1 of the block — the caller decides which lines are an entry.
    try expectCommentOutRoundTrip(
        Yaml,
        .v1_2_2,
        "# why\na: 1\nb: 2\n",
        &.{.{ .key = "a" }},
        "# why\n# a: 1\nb: 2\n",
        &.{.{ .key = "b" }},
        1,
        1,
    );
}

test "commentOut round-trips a multi-line value whole" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // Every line of the block scalar takes the marker at its own indentation,
    // the blank line included (bare marker, no trailing space).
    try expectCommentOutRoundTrip(
        Yaml,
        .v1_2_2,
        "text: |\n  one\n\n  two\nb: 2\n",
        &.{.{ .key = "text" }},
        "# text: |\n  # one\n#\n  # two\nb: 2\n",
        &.{.{ .key = "b" }},
        0,
        4,
    );
}

test "commentOut round-trips a TOML entry and a JSONC member" {
    if (comptime build_options.lang_toml) {
        try expectCommentOutRoundTrip(
            Toml,
            .TOML_1_1,
            "[server]\nport = 8080\nhost = \"local\"\n",
            &.{ .{ .key = "server" }, .{ .key = "port" } },
            "[server]\n# port = 8080\nhost = \"local\"\n",
            &.{ .{ .key = "server" }, .{ .key = "host" } },
            0,
            1,
        );
    }
    // JSONC: the member's own trailing comma is commented out with it, so what
    // is left is still valid.
    try expectCommentOutRoundTrip(
        json.Language,
        .JSONC,
        "{\n  \"a\": 1,\n  \"b\": 2\n}\n",
        &.{.{ .key = "a" }},
        "{\n  // \"a\": 1,\n  \"b\": 2\n}\n",
        &.{.{ .key = "b" }},
        0,
        1,
    );
}

test "commentOut round-trips a fig entry" {
    if (comptime !build_options.lang_figl) return error.SkipZigTest;
    try expectCommentOutRoundTrip(
        Fig,
        .Fig,
        "host = local\nport = 5432\n",
        &.{.{ .key = "host" }},
        "# host = local\nport = 5432\n",
        &.{.{ .key = "port" }},
        0,
        1,
    );
}

test "commentOut writes a nested fig entry's marker after its `>` run" {
    if (comptime !build_options.lang_figl) return error.SkipZigTest;
    // The `>` run is section depth, not indentation: the marker goes after it,
    // or the comment detaches to the root. Coming BACK is
    // `uncommentDangling`'s here — `commentBlockStart`, which is the block
    // `getLeadingComment` reports, does not climb past the `>` prefix.
    var ed: Editor(Fig) = .{ .allocator = testing.allocator, .format = .Fig };
    try ed.init("database\n> host = local\n> port = 5432\n");
    defer ed.deinit();
    try ed.commentOut(&.{ .{ .key = "database" }, .{ .key = "host" } });
    try testing.expectEqualStrings("database\n> # host = local\n> port = 5432\n", ed.source.items);
}

test "commentOut round-trips a block sequence item, dash and all" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    try expectCommentOutRoundTrip(
        Yaml,
        .v1_2_2,
        "tags:\n  - a\n  - b\n",
        &.{ .{ .key = "tags" }, .{ .index = 0 } },
        "tags:\n  # - a\n  - b\n",
        &.{ .{ .key = "tags" }, .{ .index = 0 } },
        0,
        1,
    );
}

test "commentOut of the last entry lands in the parent's dangling run" {
    if (comptime build_options.lang_yaml) {
        try expectCommentOutLastRoundTrip(
            Yaml,
            .v1_2_2,
            "server:\n  port: 8080\n  host: local\nclient: 1\n",
            &.{ .{ .key = "server" }, .{ .key = "host" } },
            "server:\n  port: 8080\n  # host: local\nclient: 1\n",
            &.{.{ .key = "server" }},
            "host: local",
        );
        // The last entry of the document: the root's dangling run.
        try expectCommentOutLastRoundTrip(
            Yaml,
            .v1_2_2,
            "a: 1\nb: 2\n",
            &.{.{ .key = "b" }},
            "a: 1\n# b: 2\n",
            &.{},
            "b: 2",
        );
    }
    if (comptime build_options.lang_toml) {
        try expectCommentOutLastRoundTrip(
            Toml,
            .TOML_1_1,
            "[server]\nport = 8080\nhost = \"local\"\n",
            &.{ .{ .key = "server" }, .{ .key = "host" } },
            "[server]\nport = 8080\n# host = \"local\"\n",
            &.{.{ .key = "server" }},
            "host = \"local\"",
        );
    }
    if (comptime build_options.lang_figl) {
        try expectCommentOutLastRoundTrip(
            Fig,
            .Fig,
            "database\n> host = local\n> port = 5432\n",
            &.{ .{ .key = "database" }, .{ .key = "port" } },
            "database\n> host = local\n> # port = 5432\n",
            &.{.{ .key = "database" }},
            "port = 5432",
        );
    }
}

test "commentOut and uncomment decline strict JSON, the root, flow items and sections" {
    var strict: Editor(json.Language) = .{ .allocator = testing.allocator, .format = .JSON };
    try strict.init("{\n  \"a\": 1\n}\n");
    defer strict.deinit();
    try testing.expectError(error.CommentsUnsupported, strict.commentOut(&.{.{ .key = "a" }}));
    try testing.expectError(error.CommentsUnsupported, strict.uncommentLeading(&.{.{ .key = "a" }}, 0, 1));
    try testing.expectError(error.CommentsUnsupported, strict.uncommentDangling(&.{}, 0, 1));

    if (comptime build_options.lang_yaml) {
        var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
        try ed.init("tags: [a, b]\nk: 1\n");
        defer ed.deinit();
        // Inside a flow collection a marker would swallow the separator, and
        // the parse discards interior comments anyway.
        try testing.expectError(error.CommentsUnanchored, ed.commentOut(&.{ .{ .key = "tags" }, .{ .index = 0 } }));
        try testing.expectError(error.CommentsUnanchored, ed.uncommentLeading(&.{ .{ .key = "tags" }, .{ .index = 1 } }, 0, 1));
        try testing.expectError(error.UnsupportedShape, ed.uncommentDangling(&.{.{ .key = "tags" }}, 0, 1));
        // The root is not an entry anything can carry.
        try testing.expectError(error.UnsupportedShape, ed.commentOut(&.{}));
        try testing.expectEqualStrings("tags: [a, b]\nk: 1\n", ed.source.items);
    }
    if (comptime build_options.lang_toml) {
        var ed: Editor(Toml) = .{ .allocator = testing.allocator, .format = .TOML_1_1 };
        try ed.init("[server]\nport = 8080\n");
        defer ed.deinit();
        // A table's span is the name inside its header; its body is lines this
        // op cannot see. Same refusal `deleteKey` makes, in TOML's words.
        try testing.expectError(error.CannotDeleteSection, ed.commentOut(&.{.{ .key = "server" }}));
    }
}

test "uncomment refuses lines that are not an entry, byte-exactly" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // The `# two` here is CONTENT of the block scalar, not a comment: stripping
    // the marker parses, but it changes `text`'s value — a node the caller
    // never named. Refused and rolled back.
    const inside_scalar = "text: |\n  one\n  # two\nb: 2\n";
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init(inside_scalar);
    defer ed.deinit();
    try testing.expectError(error.CommentNotAnEntry, ed.uncommentLeading(&.{.{ .key = "b" }}, 0, 1));
    try testing.expectEqualStrings(inside_scalar, ed.source.items);

    // Prose that does not parse as an entry: the reparse fails and
    // `replaceAtSpan` rolls the splice back on its own.
    const prose = "a: 1\n# just a note\nb: 2\n";
    var ed2: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed2.init(prose);
    defer ed2.deinit();
    try testing.expect(std.meta.isError(ed2.uncommentLeading(&.{.{ .key = "b" }}, 0, 1)));
    try testing.expectEqualStrings(prose, ed2.source.items);

    // A line with no marker at all, and a range past the end of the block.
    var ed3: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed3.init("a: 1\n# x: 2\nb: 3\n");
    defer ed3.deinit();
    try testing.expectError(error.NotFound, ed3.uncommentLeading(&.{.{ .key = "b" }}, 1, 1));
    try testing.expectError(error.NotFound, ed3.uncommentLeading(&.{.{ .key = "b" }}, 0, 2));
    // A zero-length range is a no-op.
    try ed3.uncommentLeading(&.{.{ .key = "b" }}, 0, 0);
    try testing.expectEqualStrings("a: 1\n# x: 2\nb: 3\n", ed3.source.items);
}

test "uncomment brings back a middle line of a block, leaving the rest commented" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init("# note\n# x: 2\nb: 3\n");
    defer ed.deinit();
    try ed.uncommentLeading(&.{.{ .key = "b" }}, 1, 1);
    try testing.expectEqualStrings("# note\nx: 2\nb: 3\n", ed.source.items);
    const parsed = try ed.getParsed();
    _ = try parsed.ast.getNodeByPath(&.{.{ .key = "x" }});
}

// ── set (upsert) tests ──────────────────────────────────────────────────────

fn expectSet(
    comptime Lang: type,
    format: Lang.Type,
    input: []const u8,
    path: []const AST.PathSegment,
    value: []const u8,
    expected: []const u8,
) !void {
    var ed: Editor(Lang) = .{ .allocator = testing.allocator, .format = format };
    try ed.init(input);
    defer ed.deinit();
    try ed.set(path, value);
    try testing.expectEqualStrings(expected, ed.source.items);
}

test "set replaces an existing YAML value" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    try expectSet(Yaml, .v1_2_2, "a: 1\nb: 2\n", &.{.{ .key = "a" }}, "9", "a: 9\nb: 2\n");
}

test "set inserts a missing top-level YAML key" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    try expectSet(Yaml, .v1_2_2, "a: 1\n", &.{.{ .key = "b" }}, "2", "a: 1\nb: 2\n");
}

test "set inserts a missing nested key under an existing mapping" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    try expectSet(
        Yaml,
        .v1_2_2,
        "outer:\n  inner: 1\n",
        &.{ .{ .key = "outer" }, .{ .key = "added" } },
        "2",
        "outer:\n  inner: 1\n  added: 2\n",
    );
}

test "set reframes a YAML value inline->block on replace" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // Inherits replaceValAtPath's reframing: a scalar becomes a block list
    // (fig writes indentless block sequences under a key).
    try expectSet(Yaml, .v1_2_2, "a: 1\n", &.{.{ .key = "a" }}, "- x\n- y", "a:\n- x\n- y\n");
}

test "set replaces an existing JSON value (replace branch is format-agnostic)" {
    // The replace branch matches keys logically, so it works for strict JSON.
    try expectSet(json.Language, .JSON, "{\"a\": 1}", &.{.{ .key = "a" }}, "\"x\"", "{\"a\": \"x\"}");
}

test "set creates a new JSON key, quoting it for the format" {
    // The insert branch renders the logical key into JSON syntax (`b` -> `"b"`),
    // so creating a not-yet-present key produces valid JSON.
    try expectSet(json.Language, .JSON, "{\"a\": 1}", &.{.{ .key = "b" }}, "2", "{\"a\": 1, \"b\": 2}");
    // A key needing escaping is escaped, not spliced raw.
    try expectSet(json.Language, .JSON, "{}", &.{.{ .key = "a\"b" }}, "1", "{\"a\\\"b\": 1}");
}

test "set rejects a path that does not end in a key" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init("a:\n  - 1\n");
    defer ed.deinit();
    try testing.expectError(error.NotAMapping, ed.set(&.{ .{ .key = "a" }, .{ .index = 0 } }, "9"));
    try testing.expectError(error.NotAMapping, ed.set(&.{}, "9"));
}

test "set auto-vivifies missing intermediate containers" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init("a: 1\n");
    defer ed.deinit();
    // Parent `missing` does not exist: `set` seeds it as an empty map, then
    // lands the leaf. The existing `a: 1` is untouched.
    try ed.set(&.{ .{ .key = "missing" }, .{ .key = "leaf" } }, "2");
    try testing.expect(std.mem.indexOf(u8, ed.source.items, "a: 1") != null);
    const leaf = try ed.getParsed();
    const v = try leaf.ast.getValByPath(&.{ .{ .key = "missing" }, .{ .key = "leaf" } });
    try testing.expectEqualStrings("2", v.kind.number.raw);
}

test "set vivifies a nested path through an empty node" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // A null is a container waiting to exist, not data — so navigation failing
    // through one is vivifiable, unlike failing through a scalar (next test).
    // An empty document's root:
    try expectSet(Yaml, .v1_2_2, "", &.{ .{ .key = "a" }, .{ .key = "b" } }, "1", "a:\n  b: 1\n");
    try expectSet(
        Yaml,
        .v1_2_2,
        "",
        &.{ .{ .key = "a" }, .{ .key = "b" }, .{ .key = "c" } },
        "1",
        "a:\n  b:\n    c: 1\n",
    );
    // And a bare `key:` standing where an intermediate mapping should be.
    try expectSet(
        Yaml,
        .v1_2_2,
        "title: t\na:\n",
        &.{ .{ .key = "a" }, .{ .key = "b" }, .{ .key = "c" } },
        "1",
        "title: t\na:\n  b:\n    c: 1\n",
    );
}

test "set does not clobber a scalar standing where a parent map should be" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init("a: 1\n");
    defer ed.deinit();
    // `a` is a scalar, not a map: descending into it for `b` is a real type
    // error (`NotAMapping`), not a missing key to vivify — `a: 1` stays intact.
    try testing.expectError(error.NotAMapping, ed.set(&.{ .{ .key = "a" }, .{ .key = "b" } }, "2"));
    try testing.expectEqualStrings("a: 1\n", ed.source.items);
}

test "set lands a single-entry mapping value like any other mapping value" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // A one-entry map renders `k: v` — the shape a scalar has — so every splice
    // path used to read it as one and fail. All three branches of `set` (insert,
    // nested insert, replace) must treat it as the block mapping it is, exactly
    // as they already treated a two-entry map.
    try expectSet(Yaml, .v1_2_2, "a: 1\n", &.{.{ .key = "fresh" }}, "k: v\n", "a: 1\nfresh:\n  k: v\n");
    try expectSet(
        Yaml,
        .v1_2_2,
        "outer:\n  inner: 1\n",
        &.{ .{ .key = "outer" }, .{ .key = "added" } },
        "k: v\n",
        "outer:\n  inner: 1\n  added:\n    k: v\n",
    );
    try expectSet(Yaml, .v1_2_2, "a: 1\n", &.{.{ .key = "a" }}, "k: v\n", "a:\n  k: v\n");
}

test "set rolls back a vivified ancestor when the leaf insert fails" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init("title: t\n");
    defer ed.deinit();
    // Malformed value text: `a` is seeded, then the leaf insert's reparse
    // rejects it. Two splices, one outcome — `Err` has to mean the document is
    // untouched, or a caller that retries is editing a document it never asked
    // for. Without the rollback the seeded `a:` would survive.
    try testing.expectError(
        error.UnexpectedToken,
        ed.set(&.{ .{ .key = "a" }, .{ .key = "b" } }, "[unclosed"),
    );
    try testing.expectEqualStrings("title: t\n", ed.source.items);
}

test "an insert into a section with no own entries takes a header line of its own, or the nearest one up" {
    if (comptime build_options.lang_toml) {
        // `a` is implicit: its one header line is `[a.b]`'s, and an entry
        // written after it would belong to `a.b`. The root's body takes
        // it as a dotted key.
        try expectSet(Toml, .TOML_1_1, "[a.b]\nx = 1\n", &.{ .{ .key = "a" }, .{ .key = "y" } }, "2", "a.y = 2\n[a.b]\nx = 1\n");
        // Deeper: `a` and `a.b` both pass through `[a.b.c]`.
        try expectSet(Toml, .TOML_1_1, "[a.b.c]\nx = 1\n", &.{ .{ .key = "a" }, .{ .key = "b" }, .{ .key = "y" } }, "2", "a.b.y = 2\n[a.b.c]\nx = 1\n");
        // A header of its own, before or after the child's, is the anchor.
        try expectSet(Toml, .TOML_1_1, "[a.b]\nx = 1\n[a]\n", &.{ .{ .key = "a" }, .{ .key = "y" } }, "2", "[a.b]\nx = 1\n[a]\ny = 2\n");
        try expectSet(Toml, .TOML_1_1, "[a]\n[a.b]\nx = 1\n", &.{ .{ .key = "a" }, .{ .key = "y" } }, "2", "[a]\ny = 2\n[a.b]\nx = 1\n");
    }
    if (comptime build_options.lang_ini) {
        // The case the header-line anchor was written for stays right.
        try expectSet(Ini, .INI, "[a]\n[b]\nk = 1\n", &.{ .{ .key = "a" }, .{ .key = "y" } }, "2", "[a]\ny = 2\n[b]\nk = 1\n");
    }
}

test "set surfaces a section veto rather than inserting a second entry of that name" {
    // The key exists and the replace was refused for cause; an insert
    // does not cure that, and trying it is what sent the CLI blaming the
    // value (`splice_rejected`) for a veto the value had nothing to do with.
    if (comptime build_options.lang_toml) {
        var ed: Editor(Toml) = .{ .allocator = testing.allocator, .format = .TOML_1_1 };
        try ed.init("[a]\nx = 1\n");
        defer ed.deinit();
        try testing.expectError(error.CannotReplaceSection, ed.set(&.{.{ .key = "a" }}, "2"));
        try testing.expect(!ed.splice_rejected);
        try testing.expectEqualStrings("[a]\nx = 1\n", ed.source.items);
    }
    if (comptime build_options.lang_ini) {
        var ed: Editor(Ini) = .{ .allocator = testing.allocator, .format = .INI };
        try ed.init("[user]\nname = x\n");
        defer ed.deinit();
        try testing.expectError(error.CannotReplaceSection, ed.set(&.{.{ .key = "user" }}, "1"));
        try testing.expect(!ed.splice_rejected);
        try testing.expectEqualStrings("[user]\nname = x\n", ed.source.items);
    }
}

test "set surfaces a rolled-back replace rather than inserting a second entry of that name" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init("a: 1\nb: 2\n");
    defer ed.deinit();
    try testing.expect(std.meta.isError(ed.set(&.{.{ .key = "a" }}, "\"open")));
    try testing.expect(ed.splice_rejected);
    try testing.expectEqualStrings("a: 1\nb: 2\n", ed.source.items);
}

test "insertNamedKey refuses a key the mapping already holds, in every format" {
    // Most parsers accept a repeated key, so without the engine's own check
    // the insert went through and the file gained a second entry.
    if (comptime build_options.lang_yaml) {
        var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
        try ed.init("a: 1\nm:\n  x: 1\n");
        defer ed.deinit();
        try testing.expectError(error.DuplicateKey, ed.insertNamedKey(&.{}, "a", "3"));
        try testing.expectError(error.DuplicateKey, ed.insertNamedKey(&.{.{ .key = "m" }}, "x", "3"));
        try testing.expect(!ed.splice_rejected);
        try testing.expectEqualStrings("a: 1\nm:\n  x: 1\n", ed.source.items);
        // A new name still lands.
        try ed.insertNamedKey(&.{}, "b", "2");
        try testing.expectEqualStrings("a: 1\nm:\n  x: 1\nb: 2\n", ed.source.items);
    }
    if (comptime build_options.lang_json) {
        var ed: Editor(json.Language) = .{ .allocator = testing.allocator, .format = .JSON };
        try ed.init("{\"s\": \"hi\"}");
        defer ed.deinit();
        try testing.expectError(error.DuplicateKey, ed.insertNamedKey(&.{}, "s", "\"x\""));
        try testing.expectEqualStrings("{\"s\": \"hi\"}", ed.source.items);
    }
    if (comptime build_options.lang_ini) {
        var ed: Editor(Ini) = .{ .allocator = testing.allocator, .format = .INI };
        try ed.init("[a]\nx = 1\n");
        defer ed.deinit();
        try testing.expectError(error.DuplicateKey, ed.insertNamedKey(&.{.{ .key = "a" }}, "x", "2"));
        try testing.expectEqualStrings("[a]\nx = 1\n", ed.source.items);
    }
    if (comptime build_options.lang_figl) {
        // fig's parser rejects the repeat itself; the name check answers
        // first, so it is `DuplicateKey` here too, not a blamed value.
        var ed: Editor(Fig) = .{ .allocator = testing.allocator, .format = .Fig };
        try ed.init("a = 1\n");
        defer ed.deinit();
        try testing.expectError(error.DuplicateKey, ed.insertNamedKey(&.{}, "a", "3"));
        try testing.expect(!ed.splice_rejected);
    }
}

test "a rename refuses a path with no key, and a name the mapping already holds" {
    if (comptime build_options.lang_yaml) {
        var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
        try ed.init("l:\n  - 1\na: 1\nb: 2\n");
        defer ed.deinit();
        // An item's "key" is the item itself; the splice overwrote its value.
        try testing.expectError(error.NotAKey, ed.replaceNamedKey(&.{ .{ .key = "l" }, .{ .index = 0 } }, "z"));
        try testing.expectError(error.NotAKey, ed.replaceKeyAtPath(&.{}, "z"));
        // YAML's parser accepts the repeat, so the engine is the net: by name
        // up front, and by count from key syntax.
        try testing.expectError(error.DuplicateKey, ed.replaceNamedKey(&.{.{ .key = "a" }}, "b"));
        try testing.expectError(error.DuplicateKey, ed.replaceKeyAtPath(&.{.{ .key = "a" }}, "b"));
        try testing.expect(!ed.splice_rejected);
        try testing.expectEqualStrings("l:\n  - 1\na: 1\nb: 2\n", ed.source.items);
        // Its own name is not a repeat, and a new one lands.
        try ed.replaceNamedKey(&.{.{ .key = "a" }}, "a");
        try ed.replaceNamedKey(&.{.{ .key = "a" }}, "c");
        try testing.expectEqualStrings("l:\n  - 1\nc: 1\nb: 2\n", ed.source.items);
    }
    if (comptime build_options.lang_toml) {
        // TOML's parser refuses the repeat itself, which read as a key that
        // does not parse; the name check answers first.
        var ed: Editor(Toml) = .{ .allocator = testing.allocator, .format = .TOML_1_1 };
        try ed.init("a = 1\nb = 2\n");
        defer ed.deinit();
        try testing.expectError(error.DuplicateKey, ed.replaceNamedKey(&.{.{ .key = "a" }}, "b"));
        try testing.expect(!ed.splice_rejected);
        try testing.expectEqualStrings("a = 1\nb = 2\n", ed.source.items);
    }
}

test "insertKey rolls back an insert that repeats a name, from key syntax" {
    if (comptime build_options.lang_json) {
        // The key arrives as syntax (`"s"`), so only the reparse knows its name.
        var ed: Editor(json.Language) = .{ .allocator = testing.allocator, .format = .JSON };
        try ed.init("{\"s\": \"hi\"}");
        defer ed.deinit();
        try testing.expectError(error.DuplicateKey, ed.insertKey(&.{}, "\"s\"", "\"x\""));
        try testing.expectEqualStrings("{\"s\": \"hi\"}", ed.source.items);
    }
    if (comptime build_options.lang_yaml) {
        // A document that already repeats a name is not refused an
        // unrelated insert — only an insert that adds a repeat is.
        var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
        try ed.init("a: 1\na: 2\n");
        defer ed.deinit();
        try ed.insertKey(&.{}, "b", "3");
        try testing.expectEqualStrings("a: 1\na: 2\nb: 3\n", ed.source.items);
        try testing.expectError(error.DuplicateKey, ed.insertKey(&.{}, "b", "4"));
        try testing.expectEqualStrings("a: 1\na: 2\nb: 3\n", ed.source.items);
    }
}

test "set reports the flow container, not a missing key, when a block value can't land" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    var ed: Editor(Yaml) = .{ .allocator = testing.allocator, .format = .v1_2_2 };
    try ed.init("a: {b: 1}\n");
    defer ed.deinit();
    // The parent exists and IS a mapping — just a flow one. Falling back to the
    // replace branch's `NotFound` would send the caller after a key that isn't
    // the problem.
    try testing.expectError(
        error.BlockValueIntoFlow,
        ed.set(&.{ .{ .key = "a" }, .{ .key = "c" } }, "- q\n"),
    );
    try testing.expectEqualStrings("a: {b: 1}\n", ed.source.items);
}

test "yaml set vivifies a fresh nested path as BLOCK containers" {
    if (comptime !build_options.lang_yaml) return error.SkipZigTest;
    // YAML seeds missing ancestors as bare `key:` (null), which `insertKey`
    // promotes to a real block mapping — so a fresh nested path reads like
    // hand-written YAML instead of a flow chain (`a: {b: {c: 1}}`).
    try expectSet(
        Yaml,
        .v1_2_2,
        "title: t\n",
        &.{ .{ .key = "a" }, .{ .key = "b" }, .{ .key = "c" } },
        "1",
        "title: t\na:\n  b:\n    c: 1\n",
    );
    // And the payoff: a BLOCK value now lands at a fresh nested path, which no
    // flow seed could ever hold.
    try expectSet(
        Yaml,
        .v1_2_2,
        "title: t\n",
        &.{ .{ .key = "a" }, .{ .key = "b" } },
        "- q\n",
        "title: t\na:\n  b:\n  - q\n",
    );
    try expectSet(
        Yaml,
        .v1_2_2,
        "title: t\n",
        &.{ .{ .key = "a" }, .{ .key = "b" } },
        "k: v\n",
        "title: t\na:\n  b:\n    k: v\n",
    );
    // The seeded line carries no trailing whitespace.
    try testing.expect(std.mem.indexOf(u8, "title: t\na:\n  b:\n    c: 1\n", " \n") == null);
}

test "non-YAML formats keep vivifying through flow seeds" {
    // The empty seed is YAML-specific: `a =` is not a TOML value, and the flow
    // chain is the idiomatic intermediate form for the dotted-key formats.
    if (comptime build_options.lang_toml) {
        try expectSet(
            Toml,
            .TOML_1_0,
            "title = 't'\n",
            &.{ .{ .key = "a" }, .{ .key = "b" } },
            "1",
            "title = 't'\na = { b = 1 }\n",
        );
    }
    try expectSet(
        json.Language,
        .JSON,
        "{\"t\": 1}",
        &.{ .{ .key = "a" }, .{ .key = "b" } },
        "2",
        "{\"t\": 1, \"a\": {\"b\": 2}}",
    );
}

test "fig set vivifies a nested path from an empty document" {
    if (comptime !build_options.lang_figl) return error.SkipZigTest;
    var ed: Editor(Fig) = .{ .allocator = testing.allocator, .format = .Fig };
    try ed.init(""); // empty fig doc = empty root map (seedable from scratch)
    defer ed.deinit();
    try ed.set(&.{ .{ .key = "a" }, .{ .key = "b" }, .{ .key = "c" } }, "hi");
    // The seeded parents nest as flow maps; the leaf reads back through the path.
    const parsed = try ed.getParsed();
    const v = try parsed.ast.getValByPath(&.{ .{ .key = "a" }, .{ .key = "b" }, .{ .key = "c" } });
    try testing.expectEqualStrings("hi", v.kind.string);
}

// ── dotenv / .properties (flat `KEY=value`) ─────────────────────────────────
//
// Both are flat-only (root mapping, no nesting/sequences), so unlike
// YAML/TOML/fig their root is never `.null_` — even a totally empty file
// parses as an empty (childless) `.mapping`. That's exactly the case
// `insertBlockKey` used to `.?`-unwrap into a panic on (see its comment);
// these tests exercise that path directly via `set`'s from-empty seed, which
// is also how the CLI's `set` on a freshly created file behaves.

test "dotenv set seeds the first key into an empty document" {
    if (comptime !build_options.lang_dotenv) return error.SkipZigTest;
    try expectSet(Dotenv, .DOTENV, "", &.{.{ .key = "FOO" }}, "bar", "FOO=bar\n");
}

test "dotenv set inserts a second key using '=', no spaces" {
    if (comptime !build_options.lang_dotenv) return error.SkipZigTest;
    try expectSet(Dotenv, .DOTENV, "FOO=bar\n", &.{.{ .key = "BAZ" }}, "qux", "FOO=bar\nBAZ=qux\n");
}

test "dotenv set replaces an existing value" {
    if (comptime !build_options.lang_dotenv) return error.SkipZigTest;
    try expectSet(Dotenv, .DOTENV, "FOO=bar\n", &.{.{ .key = "FOO" }}, "baz", "FOO=baz\n");
}

test "dotenv deleteKey removes the only entry, leaving an empty file" {
    if (comptime !build_options.lang_dotenv) return error.SkipZigTest;
    var ed: Editor(Dotenv) = .{ .allocator = testing.allocator, .format = .DOTENV };
    try ed.init("FOO=bar\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "FOO" }});
    try testing.expectEqualStrings("", ed.source.items);
    // Deleting down to empty round-trips back through the same from-empty
    // insert path a from-scratch `set` would use.
    try ed.set(&.{.{ .key = "AGAIN" }}, "v2");
    try testing.expectEqualStrings("AGAIN=v2\n", ed.source.items);
}

test "dotenv: an entry appended after an `export` entry starts at column 0" {
    // The entry's span starts at its key, past `export `; a new sibling
    // takes the line's indent, not the key's column (see `indentAt`).
    if (comptime !build_options.lang_dotenv) return error.SkipZigTest;
    var ed: Editor(Dotenv) = .{ .allocator = testing.allocator, .format = .DOTENV };
    try ed.init("export D=p\n");
    defer ed.deinit();
    try ed.set(&.{.{ .key = "NEW" }}, "v");
    try testing.expectEqualStrings("export D=p\nNEW=v\n", ed.source.items);
}

test "dotenv comment ops use # and round-trip" {
    if (comptime !build_options.lang_dotenv) return error.SkipZigTest;
    var ed: Editor(Dotenv) = .{ .allocator = testing.allocator, .format = .DOTENV };
    try ed.init("FOO=bar\n");
    defer ed.deinit();
    try ed.addLeadingComment(&.{.{ .key = "FOO" }}, "explaining foo");
    try ed.setTrailingComment(&.{.{ .key = "FOO" }}, "inline note");
    try testing.expectEqualStrings("# explaining foo\nFOO=bar # inline note\n", ed.source.items);
    const leading = (try ed.getLeadingComment(&.{.{ .key = "FOO" }})).?;
    defer testing.allocator.free(leading);
    try testing.expectEqualStrings("explaining foo", leading);
    const trailing = (try ed.getTrailingComment(&.{.{ .key = "FOO" }})).?;
    defer testing.allocator.free(trailing);
    try testing.expectEqualStrings("inline note", trailing);
}

test "properties set seeds the first key into an empty document" {
    if (comptime !build_options.lang_properties) return error.SkipZigTest;
    try expectSet(Properties, .PROPERTIES, "", &.{.{ .key = "foo" }}, "bar", "foo=bar\n");
}

test "properties set inserts a second key using '=', no spaces" {
    if (comptime !build_options.lang_properties) return error.SkipZigTest;
    try expectSet(Properties, .PROPERTIES, "foo=bar\n", &.{.{ .key = "baz" }}, "qux", "foo=bar\nbaz=qux\n");
}

test "properties set on a bare key — no separator — writes the separator with the value" {
    if (comptime !build_options.lang_properties) return error.SkipZigTest;
    // `flag` alone on its line is a key with an empty value at the key's
    // end; the value cannot take that slot alone, or `flagx` reads back as
    // the key `flagx`.
    try expectSet(Properties, .PROPERTIES, "flag\nb=1\n", &.{.{ .key = "flag" }}, "x", "flag=x\nb=1\n");
    // A value that is empty after a separator keeps the separator it has.
    try expectSet(Properties, .PROPERTIES, "k=\n", &.{.{ .key = "k" }}, "x", "k=x\n");
    try expectSet(Properties, .PROPERTIES, "k :   \n", &.{.{ .key = "k" }}, "x", "k :   x\n");
}

test "properties deleteKey removes the only entry, leaving an empty file" {
    if (comptime !build_options.lang_properties) return error.SkipZigTest;
    var ed: Editor(Properties) = .{ .allocator = testing.allocator, .format = .PROPERTIES };
    try ed.init("foo=bar\n");
    defer ed.deinit();
    try ed.deleteKey(&.{.{ .key = "foo" }});
    try testing.expectEqualStrings("", ed.source.items);
    try ed.set(&.{.{ .key = "again" }}, "v2");
    try testing.expectEqualStrings("again=v2\n", ed.source.items);
}

test "properties comment ops: leading works with '#', trailing is unsupported" {
    if (comptime !build_options.lang_properties) return error.SkipZigTest;
    var ed: Editor(Properties) = .{ .allocator = testing.allocator, .format = .PROPERTIES };
    try ed.init("flag\nk=v\n");
    defer ed.deinit();
    try ed.addLeadingComment(&.{.{ .key = "k" }}, "note");
    try testing.expectEqualStrings("flag\n# note\nk=v\n", ed.source.items);

    // A `#` after a value is value text, so `flag # note` would read back as
    // `flag` = `# note`. Refused, and nothing is written.
    try testing.expectError(error.CommentsUnsupported, ed.setTrailingComment(&.{.{ .key = "flag" }}, "note"));
    try testing.expectError(error.CommentsUnsupported, ed.setTrailingComment(&.{.{ .key = "k" }}, "note"));
    try testing.expectEqualStrings("flag\n# note\nk=v\n", ed.source.items);
}

/// `ed`'s `.properties` value at `key`, as the document now reads it.
fn expectPropertiesValue(ed: *Editor(Properties), key: []const u8, expected: []const u8) !void {
    const parsed = try ed.getParsed();
    const node = try parsed.ast.getValByPath(&.{.{ .key = key }});
    try testing.expectEqualStrings(expected, node.kind.string);
}

test "properties: a comment or entry written after a continued last line ends the continuation first" {
    if (comptime !build_options.lang_properties) return error.SkipZigTest;
    // `trail=v\` continues onto the next line, so a line written straight
    // after it would be read as the rest of `trail`'s value.
    {
        var ed: Editor(Properties) = .{ .allocator = testing.allocator, .format = .PROPERTIES };
        try ed.init("trail=v\\\n");
        defer ed.deinit();
        try ed.addDanglingComment(&.{}, "note");
        try testing.expectEqualStrings("trail=v\\\n\n# note\n", ed.source.items);
        try expectPropertiesValue(&ed, "trail", "v");
    }
    {
        var ed: Editor(Properties) = .{ .allocator = testing.allocator, .format = .PROPERTIES };
        try ed.init("a=1\ntrail=v\\\r\n");
        defer ed.deinit();
        try ed.set(&.{.{ .key = "n" }}, "1");
        try testing.expectEqualStrings("a=1\ntrail=v\\\r\n\nn=1\n", ed.source.items);
        try expectPropertiesValue(&ed, "trail", "v");
        try expectPropertiesValue(&ed, "n", "1");
    }
    // A blank line already ends it, but the entry's span stops before that
    // line, so the insert lands between the two.
    {
        var ed: Editor(Properties) = .{ .allocator = testing.allocator, .format = .PROPERTIES };
        try ed.init("x=1\ntrail=v\\\n\n");
        defer ed.deinit();
        try ed.set(&.{.{ .key = "n" }}, "1");
        try testing.expectEqualStrings("x=1\ntrail=v\\\n\nn=1\n\n", ed.source.items);
        try expectPropertiesValue(&ed, "trail", "v");
    }
    // An even run of `\` is an escaped one, and a comment never continues:
    // neither takes a blank line.
    try expectSet(Properties, .PROPERTIES, "a=v\\\\\n", &.{.{ .key = "n" }}, "1", "a=v\\\\\nn=1\n");
    {
        var ed: Editor(Properties) = .{ .allocator = testing.allocator, .format = .PROPERTIES };
        try ed.init("a=1\n# c\\\n");
        defer ed.deinit();
        try ed.addDanglingComment(&.{}, "note");
        try testing.expectEqualStrings("a=1\n# c\\\n# note\n", ed.source.items);
    }
}

test "properties: an entry moved after a continued line does not join it" {
    if (comptime !build_options.lang_properties) return error.SkipZigTest;
    {
        var ed: Editor(Properties) = .{ .allocator = testing.allocator, .format = .PROPERTIES };
        try ed.init("a=1\ntrail=v\\\n");
        defer ed.deinit();
        try ed.reorderKeys(&.{}, &.{"trail"});
        try testing.expectEqualStrings("trail=v\\\n\na=1\n", ed.source.items);
        try expectPropertiesValue(&ed, "trail", "v");
        try expectPropertiesValue(&ed, "a", "1");
    }
    // `moveKey` sets `trail` down before `b`, whose line is not spliced.
    {
        var ed: Editor(Properties) = .{ .allocator = testing.allocator, .format = .PROPERTIES };
        try ed.init("trail=v\\\n\na=1\nb=2\n");
        defer ed.deinit();
        try ed.moveKey(&.{.{ .key = "trail" }}, &.{.{ .key = "b" }});
        try testing.expectEqualStrings("\na=1\ntrail=v\\\n\nb=2\n", ed.source.items);
        try expectPropertiesValue(&ed, "trail", "v");
        try expectPropertiesValue(&ed, "b", "2");
    }
    {
        var ed: Editor(Properties) = .{ .allocator = testing.allocator, .format = .PROPERTIES };
        try ed.init("a=1\ntrail=v\\\n");
        defer ed.deinit();
        try ed.moveKey(&.{.{ .key = "trail" }}, &.{.{ .key = "a" }});
        try testing.expectEqualStrings("trail=v\\\n\na=1\n", ed.source.items);
        try expectPropertiesValue(&ed, "a", "1");
    }
}

// ── INI (`[section]` + flat `key = value`) ──────────────────────────────────
//
// Basic root-level/parameter sanity checks only — the same level of coverage
// TOML/fig/ZON leave here (one "uses the right marker" test apiece). The
// section-nesting behavior (`iniInsertKey`/`isSectionHeaderLine`,
// the `set` auto-vivify exclusion) is exercised in `languages/ini/tests/editor.zig`,
// next to that logic.

test "ini set seeds the first root key into an empty document" {
    if (comptime !build_options.lang_ini) return error.SkipZigTest;
    try expectSet(Ini, .INI, "", &.{.{ .key = "name" }}, "fig", "name = fig\n");
}

test "ini set inserts a second root key using ' = '" {
    if (comptime !build_options.lang_ini) return error.SkipZigTest;
    try expectSet(Ini, .INI, "name = fig\n", &.{.{ .key = "lang" }}, "zig", "name = fig\nlang = zig\n");
}

test "ini comment ops: leading works with ';', trailing is unsupported" {
    if (comptime !build_options.lang_ini) return error.SkipZigTest;
    var ed: Editor(Ini) = .{ .allocator = testing.allocator, .format = .INI };
    try ed.init("name = fig\n");
    defer ed.deinit();
    try ed.addLeadingComment(&.{.{ .key = "name" }}, "a language");
    try testing.expectEqualStrings("; a language\nname = fig\n", ed.source.items);
    const leading = (try ed.getLeadingComment(&.{.{ .key = "name" }})).?;
    defer testing.allocator.free(leading);
    try testing.expectEqualStrings("a language", leading);

    try testing.expectError(error.CommentsUnsupported, ed.setTrailingComment(&.{.{ .key = "name" }}, "note"));
    try testing.expectError(error.CommentsUnsupported, ed.getTrailingComment(&.{.{ .key = "name" }}));
    try testing.expectError(error.CommentsUnsupported, ed.deleteTrailingComment(&.{.{ .key = "name" }}));
}
