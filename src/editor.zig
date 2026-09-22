//! Editor module, generic over Language.

const std = @import("std");
const build_options = @import("build_options");

const AST = @import("ast/ast.zig");
const Document = @import("document.zig");
const Span = @import("util/span.zig");
const json = @import("languages/json/json.zig");
const json_string = @import("util/json_string.zig");
const regions = @import("editor/regions.zig");
/// The source-coordinate utilities the engine and the regions module share.
/// Re-exported here for the engine's own use; a format's editor tests import
/// `editor/splice.zig` directly.
const splice = @import("editor/splice.zig");
const lineStartBefore = splice.lineStartBefore;
const lineEndAfter = splice.lineEndAfter;
const firstNonSpace = splice.firstNonSpace;
const columnOf = splice.columnOf;
const isFlow = splice.isFlow;
const CommentStyle = splice.CommentStyle;
const commentBlockStart = splice.commentBlockStart;
const appendBlockSep = splice.appendBlockSep;
const Block = splice.Block;
const tileBlocks = splice.tileBlocks;
const fullOrder = splice.fullOrder;
const Region = regions.Region;
const log = std.log.scoped(.editor);

// The declared half of the Language interface — `Syntax`, `Caps`,
// `CommentStyle`. Every surface-syntax parameter this engine used to select
// with an `if (Language == X)` branch now comes from `Language.syntax(t)`
// instead; see `languages/manifest.zig` and
// `docs/proposals/language-interface.md`. A leaf module, so importing it here
// (and from every `<lang>/<lang>.zig`) pulls in nothing else.
const lang = @import("languages/manifest.zig");
const sniff = @import("languages/fig/tokenizer.zig");

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
// implementations live in `<lang>/editor_helper.zig` (which also holds that
// language's editor tests), and the DECLARATION of which renderers a format
// supplies, with the reason, sits in the "Renderers" block of its
// `<lang>/<lang>.zig`.
//
// What remains below is what renderer dispatch does not reach:
//
//   * `zon_edit.appendFieldName` is reached through `key_style`, as the
//     rendering half of a syntax parameter.
//   * The bare language tags below are used by this file's OWN tests, nothing
//     else. The whole-container ops are generic over `section_noun`,
//     `section_header` and what the parser records, and refuse at comptime
//     for a format that declares none of it; Zig has no conditional
//     container-level declarations (`usingnamespace` was removed in 0.15),
//     so the methods exist for every format.
const zon_edit = @import("languages/zon/editor_helper.zig");
const Toml = @import("languages/toml/toml.zig").Language;
const Fig = @import("languages/fig/fig.zig").Language;
const Yaml = @import("languages/yaml/yaml.zig").Language;
const Zon = @import("languages/zon/zon.zig").Language;
const Dotenv = @import("languages/dotenv/dotenv.zig").Language;
const Properties = @import("languages/properties/properties.zig").Language;
const Ini = @import("languages/ini/ini.zig").Language;
const Plist = @import("languages/plist/plist.zig").Language;
const NestedText = @import("languages/nestedtext/nestedtext.zig").Language;

/// What fig's bare-literal rules make of `text`, trimmed of whitespace:
/// the `Literal` a value renderer is handed. The `.fig` dialect's own
/// classifier (`sniffBare`), so `true`, `42`, `2.5`, `2026-09-10` and
/// `null` are typed and `Yes`, `007` and `TRUE` stay strings, in every
/// format alike.
pub fn literalOf(text: []const u8) lang.Literal {
    return switch (sniff.sniffBare(std.mem.trim(u8, text, " \t\r\n"))) {
        .null_ => .null,
        .boolean => .bool,
        .number => |n| if (n.kind == .integer) .int else .float,
        .datetime => .datetime,
        .string => .string,
    };
}

pub fn Editor(comptime Language: type) type {
    @import("languages/language.zig").validate(Language);
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
        /// The one place the engine asks. Three answers, in order:
        ///
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
            if (!self.syntax().flow_containers) return false;
            // `is_section_format` is "may be" for a runtime language, so the
            // dialect's own `section_noun` settles it at the call: a runtime
            // JSON's `[1, 2]` root is a flow sequence, not a section root.
            if (is_section_format and self.syntax().section_noun != null and (node.id == parsed.ast.root or parsed.isSection(node))) return false;
            return isFlow(self.source.items, parsed.span(node));
        }

        /// Where `node` BEGINS on its own line: its item marker's start when
        /// the parser recorded one (`Document.node_marker_spans`), else its
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
        /// stays tab-indented, padded with spaces out to `at`'s column when
        /// `at` sits past some other token on its line — a key inside a
        /// `- key: v` item, whose siblings align under the key and not under
        /// the dash. The engine used to count a column and write that many
        /// spaces, which lost tabs and was fig's whole reason for hooking
        /// three insert operations.
        pub fn indentAt(self: *const Self, buf: *std.ArrayList(u8), at: usize) ![]const u8 {
            const source = self.source.items;
            const line_start = lineStartBefore(source, at);
            const from = buf.items.len;
            if (self.syntax().structural_indent) {
                try buf.appendSlice(self.allocator, source[line_start..at]);
            } else {
                const ws_end = @min(firstNonSpace(source, line_start), at);
                try buf.appendSlice(self.allocator, source[line_start..ws_end]);
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
        /// **Renderer** `renderValue(t, allocator, out, value_text, literal)
        /// !void`, where `literal` is `literalOf(value_text)`: the engine
        /// classifies the text once, by fig's own bare-literal rules, so a
        /// renderer spells a kind it is told rather than deciding one.
        fn renderedValue(self: *const Self, buf: *std.ArrayList(u8), value_text: []const u8) ![]const u8 {
            if (!self.hasRenderer(.value)) return value_text;
            const from = buf.items.len;
            try Language.renderValue(self.format, self.allocator, buf, value_text, literalOf(value_text));
            return buf.items[from..];
        }

        /// Write one block-mapping entry after its line's `indent` has been
        /// written: the key, the separator and the value, with any further
        /// lines the entry spans prefixed by `indent` (plist's value element
        /// on a second line, NestedText's `>`-block). No trailing newline.
        ///
        /// **Renderer** `renderEntry(t, allocator, out, indent, key_text,
        /// value_text) !void` — declared by a format whose entry is not
        /// `key`, `kv_sep`, value on one line. Without it the entry is the
        /// key followed by `writeMapValue`. `value_text` has been through
        /// `renderedValue` already.
        fn writeEntry(self: *Self, out: *std.ArrayList(u8), indent: []const u8, key_text: []const u8, value_text: []const u8) !void {
            if (self.hasRenderer(.entry))
                return Language.renderEntry(self.format, self.allocator, out, indent, key_text, value_text);
            try out.appendSlice(self.allocator, key_text);
            try self.writeTail(out, indent, key_text, value_text);
        }

        /// Write everything that follows a key on its entry: the separator
        /// and the value inline (`: v`, ` = v`), or the value re-framed onto
        /// the following lines as a block (`:` then indented lines; fig's
        /// bare header over a `>` body). `key_text` is the key as written —
        /// so a renderer can tell a NestedText multiline key, which takes no
        /// separator — or empty for the DOCUMENT ROOT, where the value stands
        /// alone. No trailing newline.
        ///
        /// **Renderer** `renderTail(t, allocator, out, indent, key_text,
        /// value_text) !void`. Without it the tail is `writeMapValue`, which
        /// is YAML's answer and the default for every line-structured format.
        /// `value_text` has been through `renderedValue`.
        fn writeTail(self: *Self, out: *std.ArrayList(u8), indent: []const u8, key_text: []const u8, value_text: []const u8) !void {
            if (self.hasRenderer(.tail))
                return Language.renderTail(self.format, self.allocator, out, indent, key_text, value_text);
            try self.writeMapValue(out, indent, value_text);
        }

        /// Write one block-sequence item after its line's `indent`: the
        /// `seq_item_marker` and the value, with continuation lines of a
        /// multi-line value re-indented past the marker. No trailing newline.
        ///
        /// **Renderer** `renderItem(t, allocator, out, indent, value_text)
        /// !void` — declared by a format whose item is not marker-then-value
        /// (NestedText renders an empty or multi-line value as a `>`-block
        /// under a bare `-`). `value_text` has been through `renderedValue`.
        fn writeItem(self: *Self, out: *std.ArrayList(u8), indent: []const u8, value_text: []const u8) !void {
            if (self.hasRenderer(.item))
                return Language.renderItem(self.format, self.allocator, out, indent, value_text);
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
        /// `Document.node_mentions` and `insertBlockKey`.
        fn outOfRegion(self: *const Self, parsed: Document, kv: AST.Node) bool {
            if (kv.kind != .keyvalue) return false;
            const val = parsed.ast.nodes[kv.kind.keyvalue.value];
            if (!parsed.isSection(val)) return false;
            const source = self.source.items;
            const line = lineStartBefore(source, parsed.span(kv).start);
            for (parsed.mentionsOf(val.id)) |m| {
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
        /// raw text through — the CLI's `edit`/`set`/`insert` — read this to
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
        pub fn replaceAtSpan(self: *Self, span: Span, replacement: []const u8) !void {
            // Snapshot the whole source so a failed reparse can be undone. The
            // edit already costs a full reparse, so an O(n) copy is negligible.
            const backup = try self.allocator.dupe(u8, self.source.items);
            defer self.allocator.free(backup);

            self.splice_rejected = false;
            try self.replaceSource(span, replacement);
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
        ///     entry's separator (`Document.node_sep_spans` — YAML, fig,
        ///     NestedText), everything after the key is rewritten through
        ///     `writeTail`, so a block collection can descend onto the
        ///     following lines where the old value was inline (`k: []` → a
        ///     block list), which a span splice cannot express. An item
        ///     reframes the same way from its recorded marker when the format
        ///     renders items.
        ///
        /// **Engine rule**: a SECTION node — one the parser recorded header
        /// lines for (`Document.node_regions`) — is refused before the generic
        /// splice runs (`CannotReplaceTable` / `CannotReplaceSection` /
        /// `CannotReplaceContainer`, in the format's own vocabulary). In TOML
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
        pub fn replaceValAtPath(self: *Self, path: []const AST.PathSegment, replacement: []const u8) !void {
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
            var buf: std.ArrayList(u8) = .empty;
            defer buf.deinit(self.allocator);
            const rendered = try self.renderedValue(&buf, replacement);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            var indent_buf: std.ArrayList(u8) = .empty;
            defer indent_buf.deinit(self.allocator);

            // The document root: the value stands alone. A format that
            // renders tails spells it (NestedText's `>` block); the rest
            // splice it as written.
            if (path.len == 0) {
                if (!self.hasRenderer(.tail)) return self.replaceAtSpan(span, rendered);
                try self.writeTail(&out, "", "", rendered);
                try self.terminateLine(&out, span.end);
                return self.replaceAtSpan(Span.init(0, span.end), out.items);
            }
            // A mapping value whose entry records its separator is REFRAMED:
            // everything after the key is rewritten through `writeTail`, so
            // the value may change shape between inline and block. This runs
            // ahead of the section veto because a format whose block
            // container hangs under a bare header (fig) records a zero-width
            // separator there, and its reframe rewrites the container's body
            // in place. See `Document.node_sep_spans`.
            if (std.meta.activeTag(path[path.len - 1]) == .key) {
                const kv = try parsed.ast.getNodeByPath(path);
                if (parsed.sepSpan(kv)) |sep| {
                    const key_span = parsed.span(parsed.ast.nodes[kv.kind.keyvalue.key]);
                    const indent = try self.indentAt(&indent_buf, key_span.start);
                    try self.writeTail(&out, indent, source[key_span.start..key_span.end], rendered);
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
                const key_span = parsed.span(parsed.ast.nodes[kv.kind.keyvalue.key]);
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
                    try self.writeItem(&out, indent, rendered);
                    try self.terminateLine(&out, span.end);
                    return self.replaceAtSpan(Span.init(m.start, span.end), out.items);
                }
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
                // (`CannotReplaceTable` …), or a splice the reparse rolled back
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
                    // with no line of its own to take the entry under.
                    if (insert_err == error.BlockValueIntoFlow or insert_err == error.ImplicitSection) return insert_err;
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
                    } else return replace_err;
                };
            };
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
            return parsed.ast.nodes[parsed.ast.root].kind == .null_;
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

        /// Render a logical mapping key into this format's key syntax — for
        /// the `set` insert branch, and for a caller of `insertKey` that has
        /// a key NAME rather than key syntax — as the format's declared
        /// `key_style` says; see `Syntax.KeyStyle` for what each spelling is
        /// and why. Always returns an owned slice (the caller frees it).
        pub fn formatInsertKey(self: *Self, key: []const u8) ![]u8 {
            switch (self.syntax().key_style) {
                .json_quoted => {
                    var w = std.Io.Writer.Allocating.init(self.allocator);
                    defer w.deinit();
                    try json_string.writeQuoted(&w.writer, key);
                    return self.allocator.dupe(u8, w.written());
                },
                .zon_field => {
                    var out: std.ArrayList(u8) = .empty;
                    defer out.deinit(self.allocator);
                    try zon_edit.appendFieldName(&out, self.allocator, key);
                    return out.toOwnedSlice(self.allocator);
                },
                .verbatim => return self.allocator.dupe(u8, key),
                .bare_or_quoted => {
                    var bare = key.len > 0;
                    for (key) |c| {
                        const ok = (c >= 'A' and c <= 'Z') or (c >= 'a' and c <= 'z') or
                            (c >= '0' and c <= '9') or c == '_' or c == '-';
                        if (!ok) bare = false;
                    }
                    if (bare) return self.allocator.dupe(u8, key);
                    var out: std.ArrayList(u8) = .empty;
                    defer out.deinit(self.allocator);
                    try out.append(self.allocator, '"');
                    for (key) |ch| switch (ch) {
                        '"' => try out.appendSlice(self.allocator, "\\\""),
                        '\\' => try out.appendSlice(self.allocator, "\\\\"),
                        else => try out.append(self.allocator, ch),
                    };
                    try out.append(self.allocator, '"');
                    return out.toOwnedSlice(self.allocator);
                },
            }
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
                const target = parsed.ast.nodes[try parsed.ast.resolveAlias(node)];
                var buf: std.ArrayList(u8) = .empty;
                defer buf.deinit(self.allocator);
                return self.replaceAtSpan(self.valueSpanWithoutProps(parsed, target), try self.renderedValue(&buf, replacement));
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
        pub fn replaceKeyAtPath(self: *Self, path: []const AST.PathSegment, replacement: []const u8) !void {
            const parsed = try self.getParsed();
            // A section node's name is written wherever the format spells
            // it — every `[a.b]` sharing the prefix, every dotted line, every
            // reopening — and only the first has a key node. Splicing that
            // one span would leave the rest behind and SPLIT the container,
            // so a section renames through its recorded mentions.
            if (path.len > 0) {
                if (parsed.ast.getValByPath(path)) |value| {
                    const mentions = parsed.mentionsOf(value.id);
                    if (parsed.isSection(value) and mentions.len > 0)
                        return self.rewriteMentions(mentions, replacement);
                } else |_| {}
            }
            const node = try parsed.ast.getKeyByPath(path);
            const span = parsed.span(node);
            // **Renderer** `renderKey(t, allocator, out, indent, key_text,
            // old_key) !void` — spells the new key in the form the old one's
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
                try Language.renderKey(self.format, self.allocator, &out, indent, replacement, source[span.start..span.end]);
                return self.replaceAtSpan(span, out.items);
            }
            try self.replaceAtSpan(span, replacement);
        }

        /// `replaceKeyAtPath` for a caller that has the new key's NAME rather
        /// than its syntax, as `insertNamedKey` is to `insertKey`: the name is
        /// spelled by `formatInsertKey` and the rename goes on as before. A
        /// ZON key's span is the field name after its `.`, so the dot that
        /// spelling leads with is left where it already stands.
        pub fn replaceNamedKey(self: *Self, path: []const AST.PathSegment, name: []const u8) !void {
            const rendered = try self.formatInsertKey(name);
            defer self.allocator.free(rendered);
            const text = if (self.syntax().key_style == .zon_field) rendered[1..] else rendered;
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
        /// key. `start` always sits before any comment marker and after the value
        /// (scalar) or key (block), so a `#`/`//` inside the value can't false-match.
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
            const start = if (is_block_collection)
                parsed.span(try parsed.ast.getKeyByPath(path)).end
            else
                val_span.end;
            const line_end = std.mem.indexOfScalarPos(u8, source, start, '\n') orelse source.len;
            return .{ .start = start, .line_end = line_end };
        }

        /// Set the same-line trailing comment on the value at `path`: replace an
        /// existing trailing comment on that line, or append one if there is none.
        /// `text` must be a single line. Returns `CommentsUnsupported` for a
        /// dialect without comment syntax (strict JSON), `MultilineComment` if
        /// `text` contains a newline, and `CommentsUnanchored` for a node that
        /// shares its parent's line inside a flow collection (see
        /// `commentsUnanchored`) — the end of that line is the parent's.
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
            try self.replaceAtSpan(Span.init(cut, win.line_end), buf.items);
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
            const block_start = commentBlockStart(source, line_start, self.syntax().comments.style);
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
            const block_start = commentBlockStart(source, line_start, self.syntax().comments.style);
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
                const val = if (c.kind == .keyvalue) parsed.ast.nodes[c.kind.keyvalue.value] else c;
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
            for (source[line_start..span.start]) |c| switch (c) {
                ' ', '\t', '-', '*', '>', '+' => {},
                else => return false,
            };
            const line_end = lineEndAfter(source, span.end -| 1);
            var i = span.end;
            var comma = false;
            while (i < line_end) : (i += 1) {
                switch (source[i]) {
                    ' ', '\t', '\r', '\n' => {},
                    ',' => {
                        if (comma) return false;
                        comma = true;
                    },
                    else => return std.mem.startsWith(u8, source[i..line_end], marker),
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
        /// (`CannotDeleteTable` / `CannotDeleteSection` /
        /// `CannotDeleteContainer`) for the reason `deleteKey` refuses it: its
        /// span is a name inside a header, and its body is lines this op cannot
        /// see.
        pub fn commentOut(self: *Self, path: []const AST.PathSegment) !void {
            const marker = self.prefixCommentMarker() orelse return error.CommentsUnsupported;
            if (path.len == 0) return error.UnsupportedShape;
            const parsed = try self.getParsed();
            const node = try parsed.ast.getNodeByPath(path);
            const source = self.source.items;
            const val = if (node.kind == .keyvalue) parsed.ast.nodes[node.kind.keyvalue.value] else node;
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
            const block_start = commentBlockStart(source, line_start, self.syntax().comments.style);
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
            if (!addedOnlyAt(before, try self.getParsed(), container_path)) {
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
        pub fn insertKey(self: *Self, path: []const AST.PathSegment, key_text: []const u8, value_text: []const u8) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            const span = parsed.span(node);
            switch (node.kind) {
                .mapping => |first| {
                    if (self.isFlowNode(parsed, node)) {
                        try self.insertFlowMapEntry(parsed, node, span, first != null, key_text, value_text);
                    } else {
                        try self.insertBlockKey(parsed, node, key_text, value_text);
                    }
                },
                .null_ => try self.promoteNullToMapping(span, node.id == parsed.ast.root, key_text, value_text),
                else => return error.NotAMapping,
            }
        }

        /// `insertKey` for a caller that has the key's NAME rather than its
        /// syntax: the name is spelled as this format spells a key
        /// (`formatInsertKey` — `.name` in ZON, quoted in strict JSON, quoted
        /// when it must be in TOML) and inserted. What every binding and the
        /// CLI want, since a name is what a user types.
        pub fn insertNamedKey(self: *Self, path: []const AST.PathSegment, name: []const u8, value_text: []const u8) !void {
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
        pub fn deleteKey(self: *Self, path: []const AST.PathSegment) !void {
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
            if (parsed.isSection(parsed.ast.nodes[node.kind.keyvalue.value])) return self.refuse(.delete);
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
                // When the entry begins its own physical line, absorb any owned
                // leading comment block above it — trivia `removeFlowItem` can't
                // find on its own but the block path would have carried. A packed
                // entry (a sibling or the opening brace shares its line) is not
                // first-on-line, so this is skipped and `removeFlowItem`'s own
                // whitespace scan handles the separator and indentation. The
                // comment extension applies only when a block was actually found
                // (`cbs` climbed above `line_start`); with none, splicing from
                // the entry itself keeps the survivors' indentation intact.
                const line_start = lineStartBefore(source, entry_start);
                const on_own_line = firstNonSpace(source, line_start) == entry_start;
                const cbs = if (on_own_line) commentBlockStart(source, line_start, self.syntax().comments.style) else line_start;
                const del_start = if (cbs < line_start) cbs else entry_start;
                return self.removeFlowItem(Span.init(del_start, span.end), prev == null);
            }
            const line_start = lineStartBefore(source, span.start);
            const del_start = commentBlockStart(source, line_start, self.syntax().comments.style);
            const del_end = lineEndAfter(source, span.end -| 1);
            try self.replaceAtSpan(Span.init(del_start, del_end), "");
        }

        /// Append `value_text` as a new item to the sequence at `path`.
        ///
        /// A flow (`[…]`) sequence is comma-delimited the same way in every
        /// format that has one and takes the generic splice; a block item is
        /// spelled through `writeItem` (a `renderItem` where the format
        /// declares one) after the first item's line prefix.
        pub fn appendToSeq(self: *Self, path: []const AST.PathSegment, value_text: []const u8) !void {
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
            try self.insertSeqLine(insert_at, markerStart(parsed, first_item), value_text);
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
            const rendered = try self.renderedValue(&val_buf, value_text);
            var body: std.ArrayList(u8) = .empty;
            defer body.deinit(self.allocator);
            try self.writeItem(&body, child.items, rendered);
            try self.expandEmptyContainer(span, closed.seq, base, body.items);
        }

        /// Insert `value_text` before the first item of the sequence at `path`.
        ///
        /// The block-arm twin of `appendToSeq`'s, on the same terms.
        pub fn prependToSeq(self: *Self, path: []const AST.PathSegment, value_text: []const u8) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .sequence) return error.NotASequence;
            const span = parsed.span(node);
            const source = self.source.items;
            if (self.isFlowNode(parsed, node)) {
                try self.prependFlowItem(parsed, node, span, node.kind.sequence != null, value_text);
                return;
            }
            if (!self.syntax().block_seq_editable) return error.NotAnInlineArray;
            const first_item = (try parsed.ast.child(&node)) orelse return self.expandEmptySeq(parsed, node, value_text);
            const first_start = markerStart(parsed, first_item);
            try self.insertSeqLine(lineStartBefore(source, first_start), first_start, value_text);
        }

        /// Remove the item at `index` from the sequence at `path`. `index ==
        /// std.math.maxInt(usize)` is the "end" sentinel — the same one
        /// `parsePath` produces for the `[-]`/`[$]` append token — and means
        /// "the last item" here, so `contents[-]` deletes symmetrically with
        /// how it appends.
        ///
        /// The block arm takes the item's whole owned block, from its leading
        /// comment run above its marker's line (see `markerStart`) through
        /// its last line.
        pub fn removeSeqItem(self: *Self, path: []const AST.PathSegment, index: usize) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .sequence) return error.NotASequence;
            const source = self.source.items;
            // Walk to the target item, keeping `prev` (its immediate
            // preceding sibling, or null when it's first) alongside: the flow
            // path's `is_first` needs it.
            var item = (try parsed.ast.child(&node)) orelse return error.NotFound;
            var prev: ?AST.Node = null;
            if (index == std.math.maxInt(usize)) {
                while (parsed.ast.next(&item)) |nxt| {
                    prev = item;
                    item = nxt;
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
                try self.removeFlowItem(item_span, is_first);
                return;
            }
            if (!self.syntax().block_seq_editable) return error.NotAnInlineArray;
            const line_start = commentBlockStart(source, lineStartBefore(source, markerStart(parsed, item)), self.syntax().comments.style);
            const del_end = lineEndAfter(source, item_span.end -| 1);
            try self.replaceAtSpan(Span.init(line_start, del_end), "");
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
        pub fn setSequence(self: *Self, path: []const AST.PathSegment, items: []const []const u8) !void {
            if (items.len == 0) return error.UnsupportedShape;

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
                const k = d.ast.nodes[d.ast.root].kind;
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
        /// splice (`CannotMoveTable` / `CannotMoveSection` /
        /// `CannotMoveContainer`). An entry's block is not always the region
        /// it owns: a `[header]` table's block is the header LINE, so moving
        /// it would relocate the name and strand the body, and moving anything
        /// else to sit before such a header lands it at the tail of the
        /// *preceding* table's body, silently reparenting it. `moveContainer`
        /// is the op that relocates a scattered container whole.
        pub fn moveKey(self: *Self, src_path: []const AST.PathSegment, dest_path: []const AST.PathSegment) !void {
            const parsed = try self.getParsed();
            const src = try parsed.ast.getNodeByPath(src_path);
            if (src.kind != .keyvalue) return error.NotAMapping;
            const dest = try parsed.ast.getNodeByPath(dest_path);
            if (dest.kind != .keyvalue) return error.NotAMapping;
            if (parsed.isSection(parsed.ast.nodes[src.kind.keyvalue.value]) or
                parsed.isSection(parsed.ast.nodes[dest.kind.keyvalue.value]))
                return self.refuse(.move);
            const source = self.source.items;
            try self.moveBlock(
                entryBlockStart(source, parsed.span(src), self.syntax().comments.style),
                entryBlockEnd(source, parsed.span(src)),
                entryBlockStart(source, parsed.span(dest), self.syntax().comments.style),
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
            try self.reorderSeqNode(parsed, node, order);
        }

        /// Reorder the entries of the block mapping at `path` (empty path =
        /// root) so the keys listed in `keys` come first, in that order; entries
        /// whose key is not listed keep their original relative order and follow.
        /// Keys in `keys` that the mapping does not contain are ignored. Each
        /// entry's owned comments — and any interleaved blank lines / orphan
        /// comments, which ride with the entry that precedes them — are
        /// preserved, so no bytes are dropped. Errors on a flow mapping (`{…}`).
        ///
        /// **Engine rule**: once the new order is known and before any splice,
        /// an entry whose position actually changes and whose value is a
        /// SECTION node is refused (`CannotReorderTables` /
        /// `CannotReorderSections` / `CannotReorderContainers`). `[header]`
        /// entries tile into blocks that stop at the *next* sibling's line —
        /// so the last entry's block excludes its own body, and any container
        /// that changes place strands or absorbs entries. `reorderContainers`
        /// is the op that reorders scattered containers whole. Reordering an
        /// inner mapping's scalar keys around a sub-table that stays put is
        /// unaffected, which is why only the MOVED entries are checked.
        pub fn reorderKeys(self: *Self, path: []const AST.PathSegment, keys: []const []const u8) !void {
            const parsed = try self.getParsed();
            const node = try parsed.ast.getValByPath(path);
            if (node.kind != .mapping) return error.NotAMapping;
            const first_id = node.kind.mapping orelse return; // empty mapping
            const source = self.source.items;
            if (self.isFlowNode(parsed, node)) return error.NotAMapping;

            // Gather each entry's key (for matching) and block, in document order.
            var entry_keys: std.ArrayList([]const u8) = .empty;
            defer entry_keys.deinit(self.allocator);
            var blocks: std.ArrayList(Block) = .empty;
            defer blocks.deinit(self.allocator);

            var cur = parsed.ast.nodes[first_id];
            var last_end: usize = 0;
            while (true) {
                if (cur.kind != .keyvalue) return error.InvalidDocument;
                const key_node = parsed.ast.nodes[cur.kind.keyvalue.key];
                const key = switch (key_node.kind) {
                    .string => |s| s,
                    else => return error.InvalidDocument,
                };
                try entry_keys.append(self.allocator, key);
                try blocks.append(self.allocator, .{ .start = entryBlockStart(source, parsed.span(cur), self.syntax().comments.style), .end = 0 });
                last_end = entryBlockEnd(source, parsed.span(cur));
                cur = parsed.ast.next(&cur) orelse break;
            }
            tileBlocks(blocks.items, last_end);

            // Translate the requested keys into entry indices (first unused match
            // wins), then reorder the blocks by that index list.
            var order: std.ArrayList(usize) = .empty;
            defer order.deinit(self.allocator);
            const chosen = try self.allocator.alloc(bool, blocks.items.len);
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
            // The engine's veto, before anything is spliced — over exactly the
            // entries whose position this reorder changes, since an entry left
            // where it was is never at risk. See the rule note above. Skipped
            // at comptime for a format with no sections, where nothing could
            // be refused.
            if (comptime is_section_format) {
                // Where each entry ends up: the listed keys first, in `order`,
                // then the unlisted ones in their original relative order.
                const final_pos = try self.allocator.alloc(usize, blocks.items.len);
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
                var entry = parsed.ast.nodes[first_id];
                var idx: usize = 0;
                while (true) : (idx += 1) {
                    if (final_pos[idx] != idx and parsed.isSection(parsed.ast.nodes[entry.kind.keyvalue.value]))
                        return self.refuse(.reorder);
                    entry = parsed.ast.next(&entry) orelse break;
                }
            }
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
            try self.reorderSeqNode(parsed, node, indices);
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
            if (self.isFlowNode(parsed, node)) {
                try self.reorderFlowItems(spans.items, order);
                return;
            }
            if (!self.syntax().block_seq_editable) return error.NotAnInlineArray;
            var blocks: std.ArrayList(Block) = .empty;
            defer blocks.deinit(self.allocator);
            for (starts.items) |at| {
                try blocks.append(self.allocator, .{ .start = commentBlockStart(source, lineStartBefore(source, at), self.syntax().comments.style), .end = 0 });
            }
            const last_end = entryBlockEnd(source, spans.items[spans.items.len - 1]);
            tileBlocks(blocks.items, last_end);
            try self.reorderBlocks(blocks.items[0].start, last_end, blocks.items, order);
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
            for (perm) |i| try appendBlockSep(&out, self.allocator, source[blocks[i].start..blocks[i].end]);
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

        /// Move the block `[src_start, src_end)` so it begins at `dest_start`,
        /// preserving the bytes between source and destination. No-op when the
        /// destination falls within the source block.
        fn moveBlock(self: *Self, src_start: usize, src_end: usize, dest_start: usize) !void {
            if (dest_start >= src_start and dest_start <= src_end) return;
            const source = self.source.items;
            const moved = source[src_start..src_end];
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            if (src_end <= dest_start) {
                // src precedes dest: [src][between][dest..] -> [between][src][dest..]
                try appendBlockSep(&out, self.allocator, source[src_end..dest_start]);
                try appendBlockSep(&out, self.allocator, moved);
                try self.replaceAtSpan(Span.init(src_start, dest_start), out.items);
            } else {
                // dest precedes src: [dest..][between][src] -> [src][dest..][between]
                try appendBlockSep(&out, self.allocator, moved);
                try appendBlockSep(&out, self.allocator, source[dest_start..src_start]);
                try self.replaceAtSpan(Span.init(dest_start, src_end), out.items);
            }
        }

        // --- insert helpers (build text, then splice) ---

        /// Insert `key_text<kv_sep>value_text` as a new entry in the block
        /// (non-flow) mapping `mapping`, after its last existing entry (or,
        /// if it has none, at a language-appropriate fallback point — see
        /// below). `pub` so `ini/editor_helper.zig`'s `iniInsertKey` (INI's
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
            // `Document.node_mentions`.
            var maybe_last: ?AST.Node = null;
            var first_key: ?AST.Node = null;
            var skipped = false;
            var cur = try parsed.ast.child(&mapping);
            while (cur) |kv| : (cur = parsed.ast.next(&kv)) {
                if (self.outOfRegion(parsed, kv)) {
                    skipped = true;
                    continue;
                }
                if (first_key == null) first_key = if (kv.kind == .keyvalue) parsed.ast.nodes[kv.kind.keyvalue.key] else kv;
                maybe_last = kv;
            }
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            var val_buf: std.ArrayList(u8) = .empty;
            defer val_buf.deinit(self.allocator);
            const rendered = try self.renderedValue(&val_buf, value_text);
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
                try self.writeEntry(&out, child.items, key_text, rendered);
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

            if (insert_at > 0 and source[insert_at - 1] != '\n') try out.append(self.allocator, '\n');
            try out.appendSlice(self.allocator, indent);
            try self.writeEntry(&out, indent, key_text, rendered);
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
        // section node's header lines (`Document.node_regions`). Everything
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
        // lines), which `Document.node_mentions` now records. Both are
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
        // `[section]` are the same thing here, and the format's own words
        // survive in its errors (`NotATable` vs `NotAContainer`) through
        // `Syntax.section_noun`.

        /// Whether this format has section nodes at all — a non-null
        /// `section_noun` in any dialect. Comptime, so the generic ops and the
        /// line-splice rule can be compiled out for every other format.
        ///
        /// A runtime language (`languages/runtime.zig`) has no comptime
        /// dialect table; it declares `runtime` and the answer is "may be",
        /// with `syntax()` deciding per entry at the call.
        pub const is_section_format = @hasDecl(Language, "runtime") or blk: {
            var any = false;
            for (std.meta.tags(Language.Type)) |t| {
                if (Language.syntax(t).section_noun != null) any = true;
            }
            break :blk any;
        };

        /// The line-splice ops the section rule refuses, and the error each
        /// refusal is spelled with per `SectionNoun`.
        const SectionOp = enum { delete, replace, move, reorder, not_a_section, exists };

        /// The engine's refusal in this format's vocabulary. Returns an error
        /// VALUE (not a union) so a caller writes `return self.refuse(.delete)`.
        fn refuse(self: *const Self, comptime op: SectionOp) error{
            NotATable,
            NotAContainer,
            CannotDeleteTable,
            CannotDeleteSection,
            CannotDeleteContainer,
            CannotReplaceTable,
            CannotReplaceSection,
            CannotReplaceContainer,
            CannotMoveTable,
            CannotMoveSection,
            CannotMoveContainer,
            CannotReorderTables,
            CannotReorderSections,
            CannotReorderContainers,
            TableExists,
            SectionExists,
            ContainerExists,
        } {
            // A format with no section nodes never reaches here (`isSection`
            // is false for all its nodes), so `.container` is only the type's
            // default, never a real answer.
            const noun = self.syntax().section_noun orelse .container;
            return switch (op) {
                .not_a_section => switch (noun) {
                    .table => error.NotATable,
                    .section, .container => error.NotAContainer,
                },
                .delete => switch (noun) {
                    .table => error.CannotDeleteTable,
                    .section => error.CannotDeleteSection,
                    .container => error.CannotDeleteContainer,
                },
                .replace => switch (noun) {
                    .table => error.CannotReplaceTable,
                    .section => error.CannotReplaceSection,
                    .container => error.CannotReplaceContainer,
                },
                .move => switch (noun) {
                    .table => error.CannotMoveTable,
                    .section => error.CannotMoveSection,
                    .container => error.CannotMoveContainer,
                },
                .reorder => switch (noun) {
                    .table => error.CannotReorderTables,
                    .section => error.CannotReorderSections,
                    .container => error.CannotReorderContainers,
                },
                .exists => switch (noun) {
                    .table => error.TableExists,
                    .section => error.SectionExists,
                    .container => error.ContainerExists,
                },
            };
        }

        /// The section node at `path`, or the format's "not a section"
        /// refusal (`NotATable` / `NotAContainer`) — for a scalar, a flow
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
            return regions.gatherNormalized(parsed, self.source.items, self.allocator, node, self.syntax().comments.style, merge_touching);
        }

        /// Byte offset just past the last line of the section node `node`'s
        /// subtree — where a new sibling section can be spliced without
        /// splitting it. `insertContainer` and `appendContainerToSeq` place a
        /// header there.
        pub fn sectionExtentEnd(self: *const Self, parsed: Document, node: AST.Node) !usize {
            return regions.extentEnd(parsed, self.source.items, self.allocator, node, self.syntax().comments.style);
        }

        /// Delete the whole container named by `path` — every scattered region
        /// of its subtree, leaving interleaved foreign content untouched. For
        /// TOML that is a table, array-of-tables, or single AoT element; for
        /// fig a block container (a path may end in an index, deleting one
        /// sequence element entire); for INI a `[section]`, reopened
        /// occurrences included. A scalar or flow-valued target is refused
        /// (`NotATable` / `NotAContainer`): `deleteKey`/`removeSeqItem` is the
        /// op for those.
        pub fn deleteContainer(self: *Self, path: []const AST.PathSegment) !void {
            comptime requireSectionFormat("deleteContainer");
            const parsed = try self.getParsed();
            const node = try self.sectionAt(parsed, path);
            var used = try self.gatherRegions(parsed, node, true);
            defer used.deinit(self.allocator);
            try regions.spliceOut(self, used.items);
        }

        /// Splice `rendered` over every one of `mentions` (sorted by start,
        /// as `Document.mentionsOf` returns them) in one whole-document
        /// rewrite, so a rename either lands everywhere or rolls back.
        fn rewriteMentions(self: *Self, mentions: []const Document.NodeMention, rendered: []const u8) !void {
            const source = self.source.items;
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            var pos: usize = 0;
            for (mentions) |m| {
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
        /// format's vocabulary (`TableExists`). Comptime-refused for a format
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
        /// key `new_leaf`, rendered per `key_style` and written over every
        /// place the format spells the name (`Document.node_mentions`): the
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
            if (mentions.len == 0) return self.refuse(.not_a_section);
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
                    break :blk regions.headerLineRegion(self.source.items, first, self.syntax().comments.style).start;
                }
                break :blk self.source.items.len;
            };
            try regions.relocate(self, used.items, dest_at);
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
            try regions.reorderBundles(self, all.items[0..total], bundles.items);
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
                if (parsed.ast.nodes[elem].kind != .mapping) return error.NotAnArrayOfTables;
                last_elem = elem;
                elem = parsed.ast.nodes[elem].next_sibling orelse break;
            }
            const hdr = self.syntax().section_header.?;
            const seq_open = hdr.seq_open orelse return error.NotAnArrayOfTables;
            const insert_at = try self.sectionExtentEnd(parsed, parsed.ast.nodes[last_elem]);
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
            return doc.ast.nodes[doc.ast.root].kind == .mapping;
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
        fn insertSeqLine(self: *Self, insert_at: usize, sibling_marker: usize, value_text: []const u8) !void {
            const source = self.source.items;
            var indent_buf: std.ArrayList(u8) = .empty;
            defer indent_buf.deinit(self.allocator);
            const indent = try self.indentAt(&indent_buf, sibling_marker);
            var val_buf: std.ArrayList(u8) = .empty;
            defer val_buf.deinit(self.allocator);
            const rendered = try self.renderedValue(&val_buf, value_text);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            if (insert_at > 0 and source[insert_at - 1] != '\n') try out.append(self.allocator, '\n');
            try out.appendSlice(self.allocator, indent);
            try self.writeItem(&out, indent, rendered);
            try out.append(self.allocator, '\n');
            try self.replaceAtSpan(Span.init(insert_at, insert_at), out.items);
        }

        fn promoteNullToMapping(self: *Self, null_span: Span, is_root: bool, key_text: []const u8, value_text: []const u8) !void {
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
            const rendered = try self.renderedValue(&val_buf, value_text);
            if (is_root) {
                // Empty document: the whole source becomes a single entry.
                try self.writeEntry(&out, "", key_text, rendered);
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
            try self.writeEntry(&out, child.items, key_text, rendered);
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
                const last = (try parsed.ast.lastChild(&node)).?;
                const last_end = parsed.span(last).end;
                const close = span.end - 1; // the '}'
                // The separator: `kv_sep`, or the bytes the first entry
                // writes between its key and value for a format whose flow
                // objects fix their own mode. See
                // `Syntax.flow_kv_sep_from_siblings`.
                const sep: []const u8 = if (self.syntax().flow_kv_sep_from_siblings) blk: {
                    const first = parsed.ast.nodes[node.kind.mapping.?];
                    const key_end = parsed.span(parsed.ast.nodes[first.kind.keyvalue.key]).end;
                    const value_start = parsed.span(parsed.ast.nodes[first.kind.keyvalue.value]).start;
                    break :blk source[key_end..value_start];
                } else try self.kvSep();
                // Multi-line layout: the closing brace is separated from the last
                // member by a newline. Splice after the last member's value so the
                // new entry lands on its own line, not jammed before the brace.
                if (std.mem.indexOfScalar(u8, source[last_end..close], '\n') != null) {
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
                    try out.appendSlice(self.allocator, ",\n");
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
                try out.appendSlice(self.allocator, ", ");
                try out.appendSlice(self.allocator, key_text);
                try out.appendSlice(self.allocator, sep);
                try out.appendSlice(self.allocator, value_text);
                try self.replaceAtSpan(Span.init(last_end, last_end), out.items);
                return;
            }
            return self.insertFlowEntry(span, key_text, value_text);
        }

        /// Insert the first entry into an EMPTY flow mapping (`{}` / ZON's
        /// `.{}`): a tight `{key: value}` splice, unpadded — matching how
        /// `insertFlowItem`'s empty-array case and the pre-existing JSON/YAML
        /// empty-flow-map tests already splice (no space added around a
        /// freshly-created single member).
        fn insertFlowEntry(self: *Self, span: Span, key_text: []const u8, value_text: []const u8) !void {
            try self.requireFlowValue(value_text);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            const pad = self.syntax().flow_map_pad;
            try out.appendSlice(self.allocator, pad);
            try out.appendSlice(self.allocator, key_text);
            try out.appendSlice(self.allocator, try self.kvSep());
            try out.appendSlice(self.allocator, value_text);
            try out.appendSlice(self.allocator, pad);
            const at = flowOpenEnd(self.source.items, span); // just after '{' (or ZON's '.{')
            try self.replaceAtSpan(Span.init(at, at), out.items);
        }

        /// Insert `value_text` as the new last item of the flow sequence
        /// `node`/`span`. Splices immediately after the current last
        /// element rather than before the closing `]`, so a pre-existing
        /// trailing comma (legal in fig/JSON5 flow arrays) isn't doubled
        /// into an empty element that fails to reparse. When the array is
        /// laid out one item per line, the new item follows that same
        /// one-per-line style, indented to match the first item — mirroring
        /// `insertFlowMapEntry`'s multi-line handling.
        fn insertFlowItem(self: *Self, parsed: Document, node: AST.Node, span: Span, non_empty: bool, value_text: []const u8) !void {
            try self.requireFlowValue(value_text);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            const source = self.source.items;
            if (non_empty) {
                const last = (try parsed.ast.lastChild(&node)).?;
                const last_end = parsed.span(last).end;
                const close = span.end - 1; // the ']'
                if (std.mem.indexOfScalar(u8, source[last_end..close], '\n') != null) {
                    const first_item = (try parsed.ast.child(&node)).?;
                    const col = columnOf(source, parsed.span(first_item).start);
                    try out.appendSlice(self.allocator, ",\n");
                    try out.appendNTimes(self.allocator, ' ', col);
                } else {
                    try out.appendSlice(self.allocator, ", ");
                }
                try out.appendSlice(self.allocator, value_text);
                try self.replaceAtSpan(Span.init(last_end, last_end), out.items);
                return;
            }
            try out.appendSlice(self.allocator, value_text);
            const at = flowOpenEnd(self.source.items, span); // just after '[' (or ZON's '.{')
            try self.replaceAtSpan(Span.init(at, at), out.items);
        }

        fn prependFlowItem(self: *Self, parsed: Document, node: AST.Node, span: Span, non_empty: bool, value_text: []const u8) !void {
            try self.requireFlowValue(value_text);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(self.allocator);
            try out.appendSlice(self.allocator, value_text);
            // Splice right before the CURRENT first item's own span — not
            // `flowOpenEnd` — so any padding space between the delimiter and that
            // item (`[ a, b ]`) stays between the delimiter and the newly
            // prepended item rather than being swallowed against it (`[0,  a,
            // b]`, an extra space where the old padding and the new separator
            // collided).
            const at = if (non_empty) blk: {
                try out.appendSlice(self.allocator, ", ");
                const first = (try parsed.ast.child(&node)).?;
                break :blk parsed.span(first).start;
            } else flowOpenEnd(self.source.items, span); // just after '[' (or ZON's '.{')
            try self.replaceAtSpan(Span.init(at, at), out.items);
        }

        /// Whitespace the flow-item scan may cross while hunting for the
        /// adjoining comma: spaces/tabs plus newlines, so a one-item-per-line
        /// layout (item on its own indented line) is treated the same as a
        /// packed single-line layout.
        fn isFlowItemWs(c: u8) bool {
            return c == ' ' or c == '\t' or c == '\n';
        }

        fn removeFlowItem(self: *Self, item_span: Span, is_first: bool) !void {
            const source = self.source.items;
            if (is_first) {
                // Drop the item and a following ", " if present. Consuming
                // forward through trailing whitespace *including newlines*
                // means a standalone-line item's own indentation-and-newline
                // goes with it, leaving the next item on the line the
                // removed item's leading indent occupied — not a stray
                // blank line.
                var e = item_span.end;
                while (e < source.len and isFlowItemWs(source[e])) e += 1;
                if (e < source.len and source[e] == ',') {
                    e += 1;
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
                while (s > 0 and isFlowItemWs(source[s - 1])) s -= 1;
                if (s > 0 and source[s - 1] == ',') {
                    s -= 1;
                    while (s > 0 and isFlowItemWs(source[s - 1])) s -= 1;
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

/// Byte index just past a flow container's opening delimiter (`{`, `[`, or
/// ZON's two-byte `.{`). Used to splice the first entry/item into an empty
/// container, where `span.start` alone isn't past the delimiter for ZON.
fn flowOpenEnd(source: []const u8, span: Span) usize {
    const i = firstNonSpace(source, span.start);
    if (i < source.len and source[i] == '.') return i + 2; // '.' + '{'
    return i + 1;
}

/// Start of a mapping entry's full block: its owned leading comment block
/// (`commentBlockStart`) at the start of the key's line. Mirrors the span math
/// `deleteKey` uses, factored out for move/reorder.
fn entryBlockStart(source: []const u8, kv_span: Span, style: CommentStyle) usize {
    return commentBlockStart(source, lineStartBefore(source, kv_span.start), style);
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

// ── "nothing else moved": the uncomment guard ───────────────────────────────
//
// `uncommentLeading`/`uncommentDangling` strip a marker off lines the CALLER
// judged to be an entry, so the editor's half of the bargain is to prove the
// result is what was claimed: a document that parses, and that differs from
// the one before the splice only by what was ADDED to the container the lines
// sit in. Anything else — text that merged into a neighbouring block scalar,
// a line that re-opened a section and reparented the entries below it — is
// rolled back byte-for-byte with `CommentNotAnEntry`.
//
// The comparison is over the two parses, not their bytes: the after-tree is
// walked to the target container along the same path the caller named, every
// node passed on the way compared whole, and only that container is allowed
// to have grown. Comments and layout are outside the AST and so outside the
// check, which is right — a comment run losing a line is exactly what the op
// does.

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

/// Structural equality of two subtrees across two parses: same kinds, same
/// scalar bytes, same children in the same order. Node ids differ between the
/// two documents, so `AST.Node.Kind.eql` (which compares child ids) cannot be
/// used for containers — only for the leaves.
fn nodesEqualDeep(b: Document, bid: AST.Node.Id, a: Document, aid: AST.Node.Id) bool {
    const bn = b.ast.nodes[bid];
    const an = a.ast.nodes[aid];
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

/// Whether `after` differs from `before` only by nodes added to the container
/// at `path` (the root for an empty path). Every other node — including every
/// container passed through on the way down, and every sibling of the ones
/// that are — must be structurally identical.
fn addedOnlyAt(before: Document, after: Document, path: []const AST.PathSegment) bool {
    return addedOnlyUnder(before, before.ast.root, after, after.ast.root, path);
}

fn addedOnlyUnder(
    b: Document,
    bid: AST.Node.Id,
    a: Document,
    aid: AST.Node.Id,
    path: []const AST.PathSegment,
) bool {
    const bn = b.ast.nodes[bid];
    const an = a.ast.nodes[aid];
    if (std.meta.activeTag(bn.kind) != std.meta.activeTag(an.kind)) return false;
    if (path.len == 0) return childrenOnlyGrew(b, bn, a, an);

    // Not the target yet: walk the two child lists in lockstep. They must have
    // the same length and pair up, with only the child the next path segment
    // names allowed to differ — and only by what is added deeper down.
    var b_cur = (b.ast.child(&bn) catch return false);
    var a_cur = (a.ast.child(&an) catch return false);
    var index: usize = 0;
    while (b_cur) |bc| {
        const ac = a_cur orelse return false;
        const on_path = switch (path[0]) {
            .key => bc.kind == .keyvalue and ac.kind == .keyvalue and
                keyNodeIs(b, bc.kind.keyvalue.key, path[0].key),
            .index => index == path[0].index,
        };
        const b_next = if (bc.kind == .keyvalue) bc.kind.keyvalue.value else bc.id;
        const a_next = if (ac.kind == .keyvalue) ac.kind.keyvalue.value else ac.id;
        if (on_path) {
            if (bc.kind == .keyvalue and !nodesEqualDeep(b, bc.kind.keyvalue.key, a, ac.kind.keyvalue.key))
                return false;
            if (!addedOnlyUnder(b, b_next, a, a_next, path[1..])) return false;
        } else if (!nodesEqualDeep(b, bc.id, a, ac.id)) return false;
        b_cur = b.ast.next(&bc);
        a_cur = a.ast.next(&ac);
        index += 1;
    }
    return a_cur == null;
}

/// Whether the key node `id` is the string `name` — the physical spelling a
/// path segment matches against while walking two parses side by side.
fn keyNodeIs(doc: Document, id: AST.Node.Id, name: []const u8) bool {
    const k = doc.ast.nodes[id].kind;
    return k == .string and std.mem.eql(u8, k.string, name);
}

/// Drop a single trailing '\n' (the serializer ends every value with one).
fn stripTrailingNewline(text: []const u8) []const u8 {
    if (text.len > 0 and text[text.len - 1] == '\n') return text[0 .. text.len - 1];
    return text;
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

test "addLeadingComment on TOML uses #" {
    if (comptime !build_options.lang_toml) return error.SkipZigTest;
    try expectCommentEdit(Toml, .TOML_1_1, "a = 1\nb = 2\n", "a = 1\n# note\nb = 2\n", .leading, &.{.{ .key = "b" }}, "note");
}

// This instantiation (plus every `Editor(Fig)` call below) is what pulls
// `Editor(Fig)`'s own methods into the test build's reachability graph — `zig
// test` analyzes a generic struct's methods only once something calls them, so
// an `Editor(Fig)` that nothing instantiates is an `Editor(Fig)` nothing
// type-checks. (`fig/editor_helper.zig`'s `test` blocks are discovered through
// `fig.zig`'s own `test {}`, and carry the rest of `Editor(Fig)`'s coverage
// rather than duplicating it here; the same split holds for TOML above.)
test "addLeadingComment on fig uses # at the target's own marker depth" {
    if (comptime !build_options.lang_fig) return error.SkipZigTest;
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

test "ZON owned-comment scan uses // (comments.style fix)" {
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
    if (comptime !build_options.lang_fig) return error.SkipZigTest;
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
    if (comptime build_options.lang_fig) {
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
    if (comptime !build_options.lang_fig) return error.SkipZigTest;
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
    if (comptime !build_options.lang_fig) return error.SkipZigTest;
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
    if (comptime !build_options.lang_fig) return error.SkipZigTest;
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
    if (comptime build_options.lang_fig) {
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
        try testing.expectError(error.CannotDeleteTable, ed.commentOut(&.{.{ .key = "server" }}));
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

test "an insert into a section with no own entries takes a header line of its own, or is refused" {
    if (comptime build_options.lang_toml) {
        // `a` is implicit: its one header line is `[a.b]`'s, and an entry
        // written after it would belong to `a.b`.
        var ed: Editor(Toml) = .{ .allocator = testing.allocator, .format = .TOML_1_1 };
        try ed.init("[a.b]\nx = 1\n");
        defer ed.deinit();
        try testing.expectError(error.ImplicitSection, ed.set(&.{ .{ .key = "a" }, .{ .key = "y" } }, "2"));
        try testing.expectEqualStrings("[a.b]\nx = 1\n", ed.source.items);
        // Deeper: `a` and `a.b` both pass through `[a.b.c]`.
        var deep: Editor(Toml) = .{ .allocator = testing.allocator, .format = .TOML_1_1 };
        try deep.init("[a.b.c]\nx = 1\n");
        defer deep.deinit();
        try testing.expectError(error.ImplicitSection, deep.set(&.{ .{ .key = "a" }, .{ .key = "b" }, .{ .key = "y" } }, "2"));
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
        try testing.expectError(error.CannotReplaceTable, ed.set(&.{.{ .key = "a" }}, "2"));
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
    if (comptime !build_options.lang_fig) return error.SkipZigTest;
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

// ── INI (`[section]` + flat `key = value`) ──────────────────────────────────
//
// Basic root-level/parameter sanity checks only — the same level of coverage
// TOML/fig/ZON leave here (one "uses the right marker" test apiece). The
// section-nesting behavior (`iniInsertKey`/`isSectionHeaderLine`,
// the `set` auto-vivify exclusion) is exercised in `ini/editor_helper.zig`,
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
