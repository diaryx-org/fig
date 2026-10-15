//! The Zig language contract: what a Zig language module declares, and
//! `validate`, which holds a module to it at comptime. A Zig language
//! reaches core only through the contract vtable built from these
//! declarations (`kit/contract.zig`) and registered (`runtime.zig`); core
//! keeps no list of languages. The consumer that bundles them is
//! `bundle.zig`.

const std = @import("std");
const manifest = @import("kit").manifest;

pub const Language = @This();

// The declared half of the interface, re-exported so a caller needs only this
// file. The definitions live in `manifest.zig`, the kit's leaf, which every
// `<lang>/<lang>.zig` imports to spell its own declarations.
pub const Caps = manifest.Caps;
pub const NativeKinds = manifest.NativeKinds;
pub const Prints = manifest.Prints;
pub const StreamDocument = manifest.StreamDocument;
pub const Comments = manifest.Comments;
pub const Syntax = manifest.Syntax;
pub const Literal = manifest.Literal;
pub const RenderRequest = manifest.RenderRequest;
pub const SpliceStyle = manifest.SpliceStyle;
pub const SpecName = manifest.SpecName;
pub const Dialect = manifest.Dialect;

/// The one list of the languages fig ships — `src/contract/list.zig` —
/// re-exported for the consumers that are genuinely per-language (the
/// build, `validate-check`, the bundle).
pub const list = @import("list.zig");

/// Every declaration a `Language` may carry. `validate` rejects anything not
/// named here, which is the whole point of the list: `@hasDecl` dispatch is
/// silent about names it does not recognize, so without a closed set an author
/// who writes `insertkey` gets a format that COMPILES, quietly runs the generic
/// implementation it meant to override, and corrupts a file on the first edit.
/// That was reproduced on the tree, not imagined — see the proposal's §10.5.
///
/// Adding a renderer to `editor.zig` means adding its name here too. That is
/// the deliberate cost of the check, and the compiler charges it immediately:
/// a renderer the list does not know is one no format can declare.
const Decls = struct {
    /// Required of every format, editable or not.
    ///
    /// `Printer` is the format's printer MODULE, and is distinct from the
    /// optional `printNode` decl below: the module is what the serializer
    /// dispatches through (`serialize.zig` reaches
    /// `@field(d.Lang.Printer, d.print_name)` for every registry entry), so
    /// every format must expose one even when — as with plist — its
    /// `Language` wraps only the module's `print`.
    ///
    /// `dialects` is the format's own rows of the format registry — see
    /// `manifest.Dialect` — which `language.zig` assembles rather than writes.
    const required = [_][]const u8{
        "Type",  "Parser", "Printer",    "default_type", "parse",
        "print", "name",   "extensions", "caps",         "dialects",
    };

    /// Required of an editable format only. `syntax` describes how the generic
    /// splice engine writes this format; asking a read-only format for one is
    /// asking it to describe an editing surface it does not have.
    const required_edit = [_][]const u8{"syntax"};

    /// Permitted, not required.
    ///
    ///   * `printNode` — every format but plist, whose `print` is
    ///     written inline.
    ///   * `samples` — a few small documents in the format's own grammar
    ///     that `contract/harness.zig` parses, prints, reparses and edits
    ///     to check what the engine assumes of every format. Every in-tree
    ///     format declares some; an out-of-tree one may.
    ///   * `hasRenderer` — a language whose renderers are resolved at
    ///     runtime (`contract/runtime.zig`) answers `Editor.hasRenderer`
    ///     itself; a compiled format never declares it, since presence is
    ///     `@hasDecl` there.
    ///   * `runtime` — the same language's marker that its dialect table is
    ///     not a comptime fact: the editor's two gates that read every
    ///     dialect's `syntax` at comptime (may it have sections, may it
    ///     spell a header) answer "ask the entry" instead.
    /// `splitStream(allocator, input, t) ![]StreamDocument` locates the
    /// documents of a multi-document stream (YAML's `---`/`...`), each one's
    /// byte range for a parse of its own; a language without streams leaves
    /// it out.
    ///   * `parseTable(parser, input, t, out)` — a Zig parser writing its
    ///     rows into the table fig hands it; `parse` is that table adopted.
    const optional = [_][]const u8{ "printNode", "samples", "hasRenderer", "runtime", "splitStream", "parseTable" };

    /// Editing hooks: none. A hook was a `pub` decl that took over an
    /// `editor.Editor` method wholesale, with the editor in hand. Twenty-five
    /// existed across six formats, and every one ended in a single splice
    /// and was Zig only for want of a fact the parser had dropped (now a
    /// `Document` table — markers, separators, regions, mentions), an engine
    /// constant that was really syntax (now a `Syntax` field), or a string
    /// function (now a renderer below). The set is kept, empty, so that the
    /// closed-set check names a hook a format still spells as unknown
    /// rather than silently never dispatching. See
    /// `docs/proposals/runtime-languages.md` §4.4.
    ///
    /// There are no `*Guard` vetoes either. The four that existed
    /// (`deleteKeyGuard`, `replaceValGuard`, `moveKeyGuard`,
    /// `reorderKeysGuard`) each refused a generic op on a SCATTERED container
    /// (a TOML `[header]` table, an INI `[section]`, a fig block container),
    /// and that is one engine rule over `Document.regionsOf` — a section
    /// node cannot be line-spliced — spelled in the format's vocabulary
    /// through `Syntax.section_noun`. See
    /// `docs/proposals/derived-regions.md`.
    const hooks = [_][]const u8{};

    /// Fragment renderers. Each is a pure function from the dialect and a
    /// `manifest.RenderRequest` to a string the engine splices under the
    /// reparse net, `renderX(t, allocator, out, request) !void`:
    /// `renderValue` spells a value (plist's typed element) given what
    /// fig's bare-literal rules make of the text (`Literal`, classified
    /// once by the engine — the one rule every format's `set` shares),
    /// `renderEntry` spells a block-mapping entry past its line's indent
    /// (plist's two-line pair, NestedText's `key:` and `>`-block),
    /// `renderItem` a block-sequence item, `renderTail` what follows a key
    /// — the separator and the value inline, or the value re-framed as a
    /// block on the following lines — and `renderKey` a renamed key in the
    /// form the old one allows, and `renderKeyName` a key's NAME as the
    /// language writes a key (JSON quotes it, ZON writes `.name`); without
    /// it a name is written as it stands. `renderString` spells a string
    /// that replaces a string in the old one's quote style (`old_value`),
    /// and writes nothing where that style is the printer's own or cannot
    /// hold the new value; the engine then splices what the caller rendered.
    /// Every one is told the container its
    /// fragment goes into (`parent_key`, `parent_tag`). `t` is the editor's dialect; no
    /// compiled renderer varies by it, and a runtime language's must
    /// (`docs/proposals/runtime-languages.md` §8.4). None receives the
    /// editor or performs a splice; the engine calls one at most once per
    /// edit and splices the result. The engine asks whether a renderer is
    /// present through `Editor.hasRenderer` — `@hasDecl` for a compiled
    /// language; a language may declare `hasRenderer(t, which)` itself and
    /// answer at runtime, which is how a vtable's null slot is an absent
    /// renderer. See `editor.Editor.renderedValue`, `writeEntry`,
    /// `writeItem`, `writeTail` and `replaceKeyAtPath`, and the proposal's
    /// §4.4.
    const renderers = [_][]const u8{ "renderValue", "renderEntry", "renderItem", "renderTail", "renderKey", "renderKeyName", "renderString" };

    fn has(comptime set: []const []const u8, comptime name: []const u8) bool {
        for (set) |k| if (std.mem.eql(u8, k, name)) return true;
        return false;
    }

    fn known(comptime name: []const u8) bool {
        return has(&required, name) or has(&required_edit, name) or
            has(&optional, name) or has(&hooks, name) or has(&renderers, name);
    }

    /// The known name `name` differs from only by letter case, or null.
    ///
    /// Not a general edit distance — deliberately. Every name above is
    /// camelCase, so the typo that actually costs something is a capitalization
    /// slip (`insertkey`, `appendtoseq`), and that is the one this catches. A
    /// wilder misspelling still fails; it just fails without a suggestion.
    fn nearest(comptime name: []const u8) ?[]const u8 {
        for ([_][]const []const u8{ &required, &required_edit, &optional, &hooks, &renderers }) |set| {
            for (set) |k| if (std.ascii.eqlIgnoreCase(k, name)) return k;
        }
        return null;
    }
};

/// The enforcement point for the `Language` contract: every declaration a
/// format must supply, the closed set it may supply, plus the coherence rules
/// between them.
///
/// `Editor()` calls this for the format it is generic over, but that is not
/// enough on its own — a read-only format (generic XML was one, through core
/// 2.x) has no editor, so `validate` of it would never be instantiated and its
/// manifest would go unchecked. The `comptime` block below this function
/// closes that gap by running `validate` over every compiled-in language,
/// editable or not.
pub fn validate(comptime Lang: type) void {
    comptime {
        // Every check here is a linear scan over a name list, and the closed-set
        // check runs one such scan PER declaration — so the work is roughly
        // `decls × known-names` per format, times eleven formats from the
        // registry loop below. That clears the default 1000-branch budget
        // comfortably; the quota is per-evaluation, not a leak.
        @setEvalBranchQuota(20_000);

        // The original four, plus `Parser` — which `tryParse` and `Editor`
        // have both required in practice for as long as they have existed,
        // and which this now states — plus `Printer`, which the serializer's
        // registry-derived dispatch requires in exactly the same way.
        // `@typeName` rather than `Lang.name` here and in `required_edit`:
        // `name` is itself one of the declarations being checked, so it cannot
        // be relied on to identify the format that is missing it.
        for (Decls.required) |name| {
            if (!@hasDecl(Lang, name))
                @compileError(@typeName(Lang) ++ " must define " ++ name);
        }
        if (@TypeOf(Lang.caps) != Caps)
            @compileError("Language.caps must be a language.Caps");

        // The format's registry rows. Their type is fixed (the registry lifts
        // exactly `Dialect(Lang)`), there must be at least one (a language
        // with no dialect is a format no derived enum can name), names are
        // unique within the language (the registry checks them across it),
        // and exactly one row selects `default_type` and is named
        // `Lang.name` — that is what ties the language's identity to the
        // member every consumer reaches it by, and what the JSON trio's
        // `jsonc`/`json5` rows are the exception to (they select the other
        // two `Type` members).
        if (@TypeOf(Lang.dialects) != []const Dialect(Lang))
            @compileError("Language '" ++ Lang.name ++ "'.dialects must be a []const language.Dialect(Language)");
        if (Lang.dialects.len == 0)
            @compileError("Language '" ++ Lang.name ++ "' declares no dialects, so no format enum can name it");
        var default_rows = 0;
        for (Lang.dialects, 0..) |d, i| {
            for (Lang.dialects[i + 1 ..]) |other| {
                if (std.mem.eql(u8, d.name, other.name))
                    @compileError("Language '" ++ Lang.name ++ "' declares two dialects named '" ++ d.name ++ "'");
            }
            if (d.Lang != Lang)
                @compileError("Language '" ++ Lang.name ++ "'.dialects row '" ++ d.name ++
                    "' names a different language");
            if (d.dialect == Lang.default_type) {
                default_rows += 1;
                if (!std.mem.eql(u8, d.name, Lang.name))
                    @compileError("Language '" ++ Lang.name ++ "' selects its default_type in the dialect" ++
                        " row named '" ++ d.name ++ "', which must be named after the language");
            }
        }
        if (default_rows != 1)
            @compileError("Language '" ++ Lang.name ++ "' must have exactly one dialect row selecting" ++
                " its default_type (the one named after the language)");

        // Coherence: `caps.lossless` describes what the `$fig` envelope pass
        // may write INTO this format, so it is only meaningful for a format
        // that can be written at all. A read-only format declaring one would
        // be describing output it never produces — the same shape of
        // contradiction as an editing hook under `caps.edit = false`.
        if (Lang.caps.lossless != null and !Lang.caps.serialize)
            @compileError("Language '" ++ Lang.name ++ "' declares caps.lossless (an envelope" ++
                " target for serialized output) but caps.serialize = false, so it never writes" ++
                " the output the envelope would go into");

        // `syntax` describes how the generic splice engine writes this
        // format, so it is required exactly when there is an editor to read
        // it. Requiring it unconditionally would be asking a read-only
        // format to describe an editing surface it does not have.
        if (Lang.caps.edit) {
            for (Decls.required_edit) |name| {
                if (!@hasDecl(Lang, name))
                    @compileError(@typeName(Lang) ++ " has caps.edit and must define " ++ name);
            }

            // Coherence: a format cannot have a same-line trailing comment
            // marker without having a comment syntax at all. Checked over
            // every dialect, since `syntax` is indexed by one.
            for (std.meta.tags(Lang.Type)) |t| {
                const s: Syntax = Lang.syntax(t);
                if (s.comments.trailing != null and s.comments.line == null)
                    @compileError("Language declares a trailing comment marker but no line comment marker");

                // Coherence: `kv_sep = null` says "the generic engine never
                // writes `key<sep>value` for me". Every path that would is
                // under the generic `insertKey`, so the claim holds exactly
                // when that op is hooked or the entry is spelled by a
                // `renderEntry` renderer — and `Editor.kvSep` refuses rather
                // than fabricating a separator if one is ever reached anyway.
                // Without this the null would be a silent `UnsupportedShape`
                // on an ordinary `set` instead of a compile error here.
                if (s.kv_sep == null and !@hasDecl(Lang, "insertKey") and !@hasDecl(Lang, "renderEntry"))
                    @compileError("Language declares kv_sep = null but neither hooks insertKey nor" ++
                        " declares renderEntry, so the generic entry-insert paths have no separator to write");

                // A flow member separator of no bytes would make every
                // flow insert weld two members together.
                if (s.flow_entry_sep.len == 0)
                    @compileError("Language declares an empty flow_entry_sep");
                if (s.flow_root and !s.flow_containers)
                    @compileError("Language declares flow_root but flow_containers = false");
            }
        }

        // The closed set. `@typeInfo(...).decls` lists only PUBLIC
        // declarations, so a format's private helpers — the
        // `const edit = @import("edit.zig")` a renderers block opens
        // with — are invisible here and need no exemption.
        for (@typeInfo(Lang).@"struct".decl_names) |d_name| {
            if (Decls.known(d_name)) continue;
            @compileError("Language '" ++ Lang.name ++ "' declares unknown '" ++ d_name ++ "'" ++
                if (Decls.nearest(d_name)) |near|
                    " — did you mean '" ++ near ++ "'?"
                else
                    ". A fragment renderer must be one of `Decls.renderers` in language.zig;" ++
                        " there are no editing hooks.");
        }

        // Coherence: a format that says it cannot be edited must not declare
        // editing behaviour. Without this, `caps.edit = false` and a live
        // renderer can disagree indefinitely — nothing else reads both.
        if (!Lang.caps.edit) {
            for (Decls.hooks ++ Decls.renderers) |name| {
                if (@hasDecl(Lang, name))
                    @compileError("Language '" ++ Lang.name ++ "' declares caps.edit = false" ++
                        " but supplies the editing renderer '" ++ name ++ "'");
            }
            return;
        }

        // The remaining rules are about hooks being REACHABLE. Both follow from
        // where `editor.zig` dispatches, so both are dead-code checks rather
        // than taste: a hook the engine can never call is a silent no-op, and
        // silent is the failure mode this whole section exists to remove.

        // The block-sequence item renderer sits below `editor.zig`'s
        // `block_seq_editable` refusal, so a format that declares no editable
        // block sequences in any dialect can never reach it.
        //
        // A runtime language has no dialect table to read here; its record
        // is held to the same rule by `runtime.validateVTable`.
        var any_block_seq = @hasDecl(Lang, "runtime");
        for (std.meta.tags(Lang.Type)) |t| {
            const s: Syntax = Lang.syntax(t);
            if (s.block_seq_editable) any_block_seq = true;
        }
        if (!any_block_seq) {
            for ([_][]const u8{"renderItem"}) |name| {
                if (@hasDecl(Lang, name))
                    @compileError("Language '" ++ Lang.name ++ "' declares block_seq_editable = false" ++
                        " but supplies '" ++ name ++ "', which the engine refuses before reaching");
            }
        }
    }
}
