const std = @import("std");
const Allocator = std.mem.Allocator;
const build_options = @import("build_options");
const manifest = @import("manifest.zig");

pub const Language = @This();

// The declared half of the interface, re-exported so a caller needs only this
// file. The definitions live in `manifest.zig` because it is a leaf — every
// `<lang>/<lang>.zig` imports it to spell its own `syntax`, and this file
// imports each of them in turn, so the types cannot live here without the
// manifest depending on the languages that declare it.
pub const CommentStyle = manifest.CommentStyle;
pub const KeyStyle = manifest.KeyStyle;
pub const Caps = manifest.Caps;
pub const Syntax = manifest.Syntax;

// Per-language gates: a compiled-out format resolves to `void`, so its module is
// never referenced and never built. Every call site that touches a gated
// `Language.*` must guard the access behind the same `build_options.lang_*`
// flag (a `comptime` check), or it will fail to compile against `void`. JSON is
// gateable like the rest now that `detect` no longer assumes it as a base.
pub const JSON = if (build_options.lang_json) @import("json/json.zig").Language else void;
pub const YAML = if (build_options.lang_yaml) @import("yaml/yaml.zig").Language else void;
pub const TOML = if (build_options.lang_toml) @import("toml/toml.zig").Language else void;
pub const ZON = if (build_options.lang_zon) @import("zon/zon.zig").Language else void;
pub const XML = if (build_options.lang_xml) @import("xml/xml.zig").Language else void;
pub const FIG = if (build_options.lang_fig) @import("fig/fig.zig").Language else void;
pub const INI = if (build_options.lang_ini) @import("ini/ini.zig").Language else void;
pub const DOTENV = if (build_options.lang_dotenv) @import("dotenv/dotenv.zig").Language else void;
pub const PROPERTIES = if (build_options.lang_properties) @import("properties/properties.zig").Language else void;
pub const PLIST = if (build_options.lang_plist) @import("plist/plist.zig").Language else void;
pub const NESTEDTEXT = if (build_options.lang_nestedtext) @import("nestedtext/nestedtext.zig").Language else void;

/// A format `detect` can recognize. The `jsonc` dialect and `canonical` are
/// deliberately excluded: jsonc overlaps json/json5 on most input, and
/// canonical is an explicit selection rather than something to sniff. `fig`
/// IS included, but slotted just ahead of YAML (see the ordering note on
/// `detect`) since its grammar overlaps TOML/YAML on plain `key = value`
/// content — it only wins detection on input that is either invalid for
/// every stricter format, or uses fig-only structural syntax (`>` section
/// depth, `*` elements, `+` continuations, `[]` group headers).
pub const Detected = enum { json, json5, yaml, toml, zon, xml, fig, ini, dotenv, properties, plist, nestedtext };

/// Best-effort content sniffing: try each COMPILED-IN parser and return the
/// first that accepts `input`, or null if none do (also what an
/// all-languages-disabled build returns). Order matters because the grammars
/// overlap — from most to least strict: JSON/JSON5, ZON, XML, TOML, then fig,
/// then INI, then YAML. fig sits just before INI/YAML, not after: YAML is so
/// permissive (a bare line is a valid plain scalar) that almost anything falls
/// through to it, which would starve fig (and INI) of a turn if it went last.
/// fig itself overlaps TOML heavily (both accept plain `key = value`), so it is
/// tried only after TOML has had first claim — a plain TOML-shaped document
/// still resolves to `.toml`, and fig only wins on content TOML can't parse
/// (its `>`/`*`/`+`/`[]` structural markers) or that is otherwise TOML-invalid.
/// INI overlaps TOML/fig too (same `[section]`/`key = value` shape) but accepts
/// strictly more — any raw, unquoted value text — so it sits right after fig
/// and wins only what both of those reject. dotenv sits last of the four
/// key/value-shaped formats since INI's grammar shadows almost all of it too
/// (see the `dotenv` branch below for the one thing that doesn't). This is a
/// heuristic, not a proof: input valid as more than one format resolves to
/// the earliest candidate in this order.
pub fn detect(allocator: Allocator, input: []const u8) ?Detected {
    if (comptime build_options.lang_json) {
        if (tryParse(JSON, allocator, input, .JSON)) return .json;
        if (tryParse(JSON, allocator, input, .JSON5)) return .json5;
    }
    if (comptime build_options.lang_zon) {
        if (tryParse(ZON, allocator, input, ZON.default_type)) return .zon;
    }
    if (comptime build_options.lang_plist) {
        // plist's DTD vocabulary (`<dict>`/`<array>`/`<key>`/...) is a STRICT
        // SUBSET of well-formed XML: the generic XML reader below would also
        // happily accept any real plist document, just folding it into a
        // differently-shaped AST (attribute/`#text` folding, no typed
        // scalars). So plist must get first claim, or a compiled-in XML
        // reader would starve it completely — the reverse isn't a problem:
        // plist's own grammar rejects anything outside its fixed element
        // vocabulary (`error.UnknownElement`), so ordinary XML falls through
        // to the `.xml` branch below untouched.
        if (tryParse(PLIST, allocator, input, PLIST.default_type)) return .plist;
    }
    if (comptime build_options.lang_xml) {
        if (tryParse(XML, allocator, input, XML.default_type)) return .xml;
    }
    if (comptime build_options.lang_toml) {
        if (tryParse(TOML, allocator, input, TOML.default_type)) return .toml;
    }
    if (comptime build_options.lang_fig) {
        if (tryParse(FIG, allocator, input, FIG.default_type)) return .fig;
    }
    if (comptime build_options.lang_ini) {
        // INI's grammar is also permissive (a bare `key = value` line, or an
        // empty file, both parse), so it's tried only after everything
        // stricter above has had first claim — it wins only on content those
        // reject, e.g. a `[section]` header or an unquoted value with
        // characters no TOML/fig scalar allows (`path = C:\a\b`).
        if (tryParse(INI, allocator, input, INI.default_type)) return .ini;
    }
    if (comptime build_options.lang_dotenv) {
        // dotenv is almost entirely shadowed by INI above: INI's key scanner
        // accepts any non-`=`/newline run (so even `export FOO=bar` parses as
        // one weird INI key) and its value decoding is quote-agnostic, so
        // nearly anything dotenv accepts, INI already claimed first. The one
        // thing only dotenv parses — a `"`/`'`-quoted value spanning a literal
        // embedded newline (INI's value never crosses a physical line) — is
        // this branch's actual reason to exist; `.env`'s real path to
        // selection is its extension (`detectLanguageFromFileEnding`
        // special-cases the `env` extension), not this content sniff.
        if (tryParse(DOTENV, allocator, input, DOTENV.default_type)) return .dotenv;
    }
    if (comptime build_options.lang_yaml) {
        if (tryParse(YAML, allocator, input, YAML.default_type)) return .yaml;
    }
    if (comptime build_options.lang_properties) {
        // `.properties` is even more permissive than YAML: a line with no
        // separator at all is still legal (a bare key, empty value — see
        // `properties/tokenizer.zig`), so nearly any UTF-8 text parses. It
        // therefore sits LAST, after YAML — the one thing this format
        // accepts that YAML rejects outright is a malformed-YAML shape
        // (see the test below); `.properties`'s real path to selection is
        // its extension, same as `.env`.
        if (tryParse(PROPERTIES, allocator, input, PROPERTIES.default_type)) return .properties;
    }
    if (comptime build_options.lang_nestedtext) {
        // NestedText goes LAST, after even `.properties` — not because its
        // own grammar is unusually permissive (it isn't: keys/values have
        // real restrictions, unlike `.properties`'s "nearly any text"), but
        // because a huge, ordinary swath of it — plain `key: value` lines
        // and `- item` lists — is ALSO valid YAML, and parses to a MEANINGFULLY
        // DIFFERENT tree there (YAML types `port: 80` as an integer;
        // NestedText's `port` is the untyped string `"80"`). Trying this
        // before YAML would silently change what today's `detect()` returns
        // for ordinary plain-YAML content already relied upon elsewhere in
        // this codebase — a real regression, not just an academic ambiguity
        // — so NestedText only gets a turn once every stricter-or-equally-
        // plausible format (including YAML) has already rejected the input.
        // Its real path to selection is the `.nt` extension (see
        // `cli/args.zig`), exactly like dotenv/`.properties` above.
        if (tryParse(NESTEDTEXT, allocator, input, NESTEDTEXT.default_type)) return .nestedtext;
    }
    return null;
}

/// Parse with `Lang` and report only whether it succeeded, releasing the document
/// either way. The detection probe — content is parsed, never retained.
fn tryParse(comptime Lang: type, allocator: Allocator, input: []const u8, t: Lang.Type) bool {
    const doc = Lang.Parser.parse(allocator, input, t) catch return false;
    doc.deinit(allocator);
    return true;
}

/// Every declaration a `Language` may carry. `validate` rejects anything not
/// named here, which is the whole point of the list: `@hasDecl` dispatch is
/// silent about names it does not recognize, so without a closed set an author
/// who writes `insertkey` gets a format that COMPILES, quietly runs the generic
/// implementation it meant to override, and corrupts a file on the first edit.
/// That was reproduced on the tree, not imagined — see the proposal's §10.5.
///
/// Adding a hook to `editor.zig` means adding its name here too. That is the
/// deliberate cost of the check, and the compiler charges it immediately: a
/// hook the list does not know is a hook no format can declare.
const Decls = struct {
    /// Required of every format, editable or not.
    const required = [_][]const u8{
        "Type",         "Parser", "default_type", "parse", "print",
        "name", "extensions",     "caps",
    };

    /// Required of an editable format only. `syntax` describes how the generic
    /// splice engine writes this format; asking a read-only format for one is
    /// asking it to describe an editing surface it does not have.
    const required_edit = [_][]const u8{"syntax"};

    /// Permitted, not required.
    ///
    ///   * `printNode` — every format but plist and xml, whose `print` is
    ///     written inline.
    ///   * `materialize`/`TagMode` — YAML only: collapsing the reference layer
    ///     before a non-YAML printer sees the tree. Callers already gate on
    ///     `@hasDecl(Lang, "materialize")`.
    const optional = [_][]const u8{ "printNode", "materialize", "TagMode" };

    /// Editing hooks. Declaring one takes over `editor.Editor`'s method of the
    /// same name — except `keyIsInherited` (a predicate the engine queries) and
    /// `seqItemLineStart` (a sub-computation), which are named for what they
    /// answer rather than for a method. Signatures are documented on the
    /// `Editor` method each overrides; see `editor.zig`.
    const hooks = [_][]const u8{
        "insertKey",         "deleteKeyGuard",
        "replaceValAtPath",  "replaceValAtPathFollowing",
        "replaceKeyAtPath",  "keyIsInherited",
        "seqItemLineStart",  "appendToSeq",
        "prependToSeq",      "removeSeqItem",
        "reorderSeqItems",   "addLeadingComment",
        "deleteLeadingComments", "getLeadingComment",
        "setTrailingComment", "deleteTrailingComment",
        "getTrailingComment",
    };

    fn has(comptime set: []const []const u8, comptime name: []const u8) bool {
        for (set) |k| if (std.mem.eql(u8, k, name)) return true;
        return false;
    }

    fn known(comptime name: []const u8) bool {
        return has(&required, name) or has(&required_edit, name) or
            has(&optional, name) or has(&hooks, name);
    }

    /// The known name `name` differs from only by letter case, or null.
    ///
    /// Not a general edit distance — deliberately. Every name above is
    /// camelCase, so the typo that actually costs something is a capitalization
    /// slip (`insertkey`, `appendtoseq`), and that is the one this catches. A
    /// wilder misspelling still fails; it just fails without a suggestion.
    fn nearest(comptime name: []const u8) ?[]const u8 {
        for ([_][]const []const u8{ &required, &required_edit, &optional, &hooks }) |set| {
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
/// enough on its own — a read-only format has no editor, so `validate(XML)`
/// would never be instantiated and XML's manifest would go unchecked. The
/// `comptime` block below this function closes that gap by running `validate`
/// over every compiled-in language, editable or not.
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
        // and which this now states.
        // `@typeName` rather than `Lang.name` here and in `required_edit`:
        // `name` is itself one of the declarations being checked, so it cannot
        // be relied on to identify the format that is missing it.
        for (Decls.required) |name| {
            if (!@hasDecl(Lang, name))
                @compileError(@typeName(Lang) ++ " must define " ++ name);
        }
        if (@TypeOf(Lang.caps) != Caps)
            @compileError("Language.caps must be a language.Caps");

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
                if (s.trailing_comment != null and s.line_comment == null)
                    @compileError("Language declares a trailing comment marker but no line comment marker");
            }
        }

        // The closed set. `@typeInfo(...).decls` lists only PUBLIC
        // declarations, so a format's private helpers — the
        // `const edit = @import("editor_helper.zig")` each hooks block opens
        // with — are invisible here and need no exemption.
        for (@typeInfo(Lang).@"struct".decls) |d| {
            if (Decls.known(d.name)) continue;
            @compileError("Language '" ++ Lang.name ++ "' declares unknown '" ++ d.name ++ "'" ++
                if (Decls.nearest(d.name)) |near|
                    " — did you mean '" ++ near ++ "'?"
                else
                    ". Editing hooks must be named for the `editor.Editor` method they" ++
                        " override, and added to `Decls.hooks` in language.zig.");
        }

        // Coherence: a format that says it cannot be edited must not declare
        // editing behaviour. Without this, `caps.edit = false` and a live hook
        // can disagree indefinitely — nothing else reads both.
        if (!Lang.caps.edit) {
            for (Decls.hooks) |name| {
                if (@hasDecl(Lang, name))
                    @compileError("Language '" ++ Lang.name ++ "' declares caps.edit = false" ++
                        " but supplies the editing hook '" ++ name ++ "'");
            }
            return;
        }

        // The remaining rules are about hooks being REACHABLE. Both follow from
        // where `editor.zig` dispatches, so both are dead-code checks rather
        // than taste: a hook the engine can never call is a silent no-op, and
        // silent is the failure mode this whole section exists to remove.

        // A block-sequence hook sits below `editor.zig`'s
        // `block_seq_editable` refusal, so a format that declares no editable
        // block sequences in any dialect can never reach one.
        var any_block_seq = false;
        var any_line_comment = false;
        for (std.meta.tags(Lang.Type)) |t| {
            const s: Syntax = Lang.syntax(t);
            if (s.block_seq_editable) any_block_seq = true;
            if (s.line_comment != null) any_line_comment = true;
        }
        if (!any_block_seq) {
            for ([_][]const u8{ "appendToSeq", "prependToSeq", "removeSeqItem", "reorderSeqItems" }) |name| {
                if (@hasDecl(Lang, name))
                    @compileError("Language '" ++ Lang.name ++ "' declares block_seq_editable = false" ++
                        " but supplies '" ++ name ++ "', which the engine refuses before reaching");
            }
        }

        // Comment hooks, the other direction. With no line-comment marker in
        // ANY dialect, every comment op is either hooked or permanently
        // `CommentsUnsupported` — so hooking SOME is almost certainly a
        // dropped delegation rather than a decision. plist is the case this
        // guards: `<!-- ... -->` is a delimiter pair with no leader, so it
        // declares null and hooks all six deliberately (see `plist.zig`), and
        // this makes losing one a compile error instead of a runtime refusal.
        //
        // Note this is the OPPOSITE of the rule the proposal's §4 proposed —
        // "trailing_comment == null alongside a declared setTrailingComment is
        // a contradiction". plist is exactly that pair, and is correct. A hook
        // does not read the marker, so a null marker beside a hook is not a
        // contradiction; it is the hook making the marker irrelevant.
        if (!any_line_comment) {
            const comment_hooks = [_][]const u8{
                "addLeadingComment",  "deleteLeadingComments", "getLeadingComment",
                "setTrailingComment", "deleteTrailingComment", "getTrailingComment",
            };
            var declared = 0;
            for (comment_hooks) |name| {
                if (@hasDecl(Lang, name)) declared += 1;
            }
            if (declared != 0 and declared != comment_hooks.len) {
                for (comment_hooks) |name| {
                    if (!@hasDecl(Lang, name))
                        @compileError("Language '" ++ Lang.name ++ "' has no line-comment marker" ++
                            " in any dialect and hooks some comment ops but not '" ++ name ++
                            "', which can then only ever return CommentsUnsupported");
                }
            }
        }
    }
}

/// Every compiled-in language, as a comptime list to iterate.
///
/// The set of formats written down ONCE, so anything that has to do something
/// per-format — `validate` below, the CLI's extension table — cannot fall out
/// of step with the set that actually exists. A gated-out format is ABSENT
/// here rather than present as `void`, so a consumer needs no gate of its own.
///
/// This is the "comptime registry with something to iterate" the proposal's §7
/// names as what the manifest unlocks. It does not by itself retire the five
/// parallel format enumerations — those are per-DIALECT and this is
/// per-LANGUAGE — but a consumer that is genuinely per-language now has one
/// list to walk instead of eleven `build_options` tests to repeat.
pub const compiled: []const type = blk: {
    var list: []const type = &.{};
    if (build_options.lang_json) list = list ++ [_]type{JSON};
    if (build_options.lang_yaml) list = list ++ [_]type{YAML};
    if (build_options.lang_toml) list = list ++ [_]type{TOML};
    if (build_options.lang_zon) list = list ++ [_]type{ZON};
    if (build_options.lang_xml) list = list ++ [_]type{XML};
    if (build_options.lang_fig) list = list ++ [_]type{FIG};
    if (build_options.lang_ini) list = list ++ [_]type{INI};
    if (build_options.lang_dotenv) list = list ++ [_]type{DOTENV};
    if (build_options.lang_properties) list = list ++ [_]type{PROPERTIES};
    if (build_options.lang_plist) list = list ++ [_]type{PLIST};
    if (build_options.lang_nestedtext) list = list ++ [_]type{NESTEDTEXT};
    break :blk list;
};

// Validate every compiled-in language, including the read-only ones that no
// `Editor()` instantiation would otherwise reach. Runs whenever this file is
// analyzed, which is whenever anything touches a format at all.
comptime {
    for (compiled) |Lang| validate(Lang);
}

test "detect identifies each compiled-in format by content" {
    const a = std.testing.allocator;
    if (comptime build_options.lang_json) {
        try std.testing.expectEqual(Detected.json, detect(a, "{\"x\":1}").?);
    }
    if (comptime build_options.lang_zon) {
        try std.testing.expectEqual(Detected.zon, detect(a, ".{ .x = 1 }").?);
    }
    if (comptime build_options.lang_plist) {
        try std.testing.expectEqual(Detected.plist, detect(a, "<dict><key>a</key><string>b</string></dict>").?);
    }
    if (comptime build_options.lang_xml) {
        try std.testing.expectEqual(Detected.xml, detect(a, "<r/>").?);
    }
    if (comptime build_options.lang_toml) {
        try std.testing.expectEqual(Detected.toml, detect(a, "x = 1\n").?);
    }
    if (comptime build_options.lang_fig) {
        // A bare container header line (no `=`, no `:`, no brackets) followed
        // by a `>`-depth child isn't valid JSON/ZON/XML/TOML, so this resolves
        // to fig even though it's tried before YAML.
        try std.testing.expectEqual(Detected.fig, detect(a, "database\n> host = localhost\n").?);
    }
    if (comptime build_options.lang_ini) {
        // A `;`-led comment line is invalid JSON/ZON/XML/TOML (TOML has no `;`
        // comment leader — its bare-key scanner rejects `;` outright) and not
        // fig syntax either, so this resolves to INI even though it's tried
        // right before YAML.
        try std.testing.expectEqual(Detected.ini, detect(a, "; header\nname = fig\n").?);
    }
    if (comptime build_options.lang_dotenv) {
        // A double-quoted value spanning a literal embedded newline is the one
        // shape only dotenv parses: INI's value never crosses a physical line
        // (it hits the line's `\n` first), so `[a]` on its own next line is a
        // bad INI statement — this falls all the way through INI to dotenv.
        try std.testing.expectEqual(Detected.dotenv, detect(a, "A=\"line1\nline2\"\n").?);
    }
    if (comptime build_options.lang_yaml) {
        // A plain mapping that is not valid JSON/TOML/fig/INI/etc. falls
        // through to YAML, the most permissive grammar and therefore tried
        // second-to-last.
        try std.testing.expectEqual(Detected.yaml, detect(a, "key: value\n").?);
    }
    if (comptime build_options.lang_properties) {
        // Malformed YAML (a scalar followed by unexpectedly-indented content)
        // still parses as `.properties`: worst case, each line is just a bare
        // key with an empty value (see `properties/tokenizer.zig`) — the most
        // permissive grammar of all, so it's tried dead last.
        try std.testing.expectEqual(Detected.properties, detect(a, "a: 1\n b: 2\n").?);
    }
}

test "detect: plain `key = value` prefers TOML over fig despite fig accepting it too" {
    const a = std.testing.allocator;
    if (comptime !build_options.lang_toml or !build_options.lang_fig) return error.SkipZigTest;
    // fig's root-level dotted assignment accepts the exact same shape TOML
    // does; TOML is tried first, so it wins the tie.
    try std.testing.expectEqual(Detected.toml, detect(a, "x = 1\n").?);
}

test "detect: a plist document prefers plist over generic xml despite xml accepting it too" {
    const a = std.testing.allocator;
    if (comptime !build_options.lang_plist or !build_options.lang_xml) return error.SkipZigTest;
    // Any well-formed plist is also well-formed generic XML; plist is tried
    // first, so it wins. Ordinary XML that isn't plist-shaped still falls
    // through to `.xml`.
    try std.testing.expectEqual(Detected.plist, detect(a, "<dict><key>a</key><string>b</string></dict>").?);
    try std.testing.expectEqual(Detected.xml, detect(a, "<r/>").?);
}
