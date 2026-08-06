```fig
title = A declared Language interface
description = Replacing editor.zig's per-language type tests with a manifest each format declares, checked by Language.validate
created = 2026-08-05
updated = 2026-08-05
part_of = [proposals](proposals.md)
```

# A declared Language interface

> **Status: proposed, unimplemented.** Nothing here has shipped. The site
> catalogue in §2 was taken against `main` at 5c7b97b (fig 2.5.3) and is the
> part of this document worth keeping even if the rest is rejected — it is the
> first written-down account of what a format actually has to supply.

`Language.validate` ([src/languages/language.zig][validate]) states a
four-declaration contract: `Type`, `default_type`, `parse`, `print`. That is
the whole of the *written* interface, and if it were the whole of the real one,
adding a format would be adding a directory.

It isn't. `Editor(Language)` is generic over a **closed set** rather than over
an interface: 65 lines of `if (Language == Toml)` / `== Plist` /
`!= NestedText` spread across its 35 public operations. Those lines are the
unwritten half of the contract. A format's real obligations are discoverable
only by reading them.

This proposes making that half explicit: each language declares a manifest, and
`validate` grows into the enforcement point. The goal is that **the interface
lives in one place** — `language.zig` says what may be declared and what each
declaration means; `<lang>/<lang>.zig` says what this format chooses.

## 1. Scope

In scope: the `Language ==` type-testing in `src/editor.zig`, and the eight
`editor_helper.zig` modules it delegates to.

Explicitly **not** in scope, though the manifest is what would eventually make
it tractable — see §7:

- The `build_options.lang_*` gates (182 sites in `c_api.zig`, 51 in
  `ast/serialize_options.zig`, 27 in `embed.zig`). These are a *different*
  coupling: a build-configuration axis, not an interface one.
- The five parallel format enumerations (`Language.Detected`, `cli.Format`,
  `c_api.FigFormat`, `serialize_options.Format`, `Embed.InnerFormat`).
- `detect()` ordering. See §6.
- Any form of dynamic or third-party format loading. This proposal is a
  precondition for that conversation, not that conversation.

This must be behaviour-preserving. `zig build conformance` — the only build
that compiles xml, plist and canonical together — is the gate.

## 2. What is actually coupled

65 type-test sites in [src/editor.zig][editor], by kind. Line numbers are
against 5c7b97b.

### A. Syntax parameters (12 sites)

Pure constants selected by format. No logic.

| Lines | What | Values |
|---|---|---|
| 86–93 | `comment_style` | `.slashes` (JSON/ZON), `.semicolon` (INI), `.xml_comment` (plist), else `.hash` |
| 102–109 | `kv_sep` | `" = "` (ZON, INI), `"="` (dotenv, .properties), else `": "` |
| 417–419 | `emptyMapLiteral` | `".{}"` (ZON), `""` (YAML), else `"{}"` |
| 490–497 | `lineCommentMarker` | `//` (JSONC/JSON5, ZON), `;` (INI), `#` else, `null` for strict JSON |
| 516 | `trailingCommentMarker` | `null` for INI and NestedText, else as leading |

`lineCommentMarker` is the awkward one: it branches on `self.format` — the
runtime `Language.Type` — not on the language. Strict JSON has no comment
syntax; JSONC and JSON5 do. Any manifest has to survive that. See §3.

### B. Capability opt-outs (6 sites)

Booleans, phrased today as exclusions.

| Line | What | Excluded |
|---|---|---|
| 329 | `set` auto-vivifies missing ancestors | INI, plist, NestedText |
| 871, 900, 945, 1286 | block sequences are editable | TOML (`NotAnInlineArray`) |
| 1533 | `singleLineIsBlockMapping` | YAML only |

Each of these carries a substantial rationale comment. INI's exclusion, for
instance, is not "INI is limited" — it is that the vivify seed `{}` is a
*value literal* that happens to mean "empty mapping" in every other editable
format, and in INI means the two-character string `{}`. That reasoning has to
survive the move; see §5, Step 0.

### C. Micro-parameters (8 sites)

Today these read as one-offs. Most are not — they are unnamed properties that
any format with the same trait would need.

| Lines | Today | Actually |
|---|---|---|
| 429–441 | JSON quotes keys, ZON prefixes `.` | how a logical key renders as syntax |
| 550 | fig copies its `>` marker run as "indent" | the line prefix is structural, not whitespace |
| 831 | ZON backs up over a leading `.` on delete | keys carry a sigil |
| 1603 | ZON promotes a null to flow `.{ … }` in place | no bare `key: value` document form |
| 537, 629, 667 | NestedText anchors on the `-` line | an item's span may not start on its own line |

The ZON sigil at 831 was the site that changed my mind about where the
parameter/hook line falls. Read as "ZON is weird" it is a hook. Read as "this
format's keys carry a sigil" it is a `key_sigil: ?u8 = null` field — and then
it costs nothing and covers a format nobody has written yet. Pushing traits
*down* into parameters is what keeps the hook table small enough to be an
interface rather than a list of exceptions.

### D. Operation overrides (30 sites)

The bulk. `if (Language == X) return x_edit.someOp(...)`, delegating to that
language's `editor_helper.zig`.

| Operation | Overridden by |
|---|---|
| `replaceValAtPath` | plist (187), NestedText (197), YAML reframe (204), fig reframe (213) |
| `replaceValAtPathFollowing` | YAML alias (454) |
| `replaceKeyAtPath` | NestedText (469) |
| `insertKey` | TOML (726), fig (728), INI (730), plist (732), NestedText (734) |
| all six comment ops | plist (527, 590, 623, 642, 661, 696) |
| `appendToSeq` | plist (861), fig (872), NestedText (880) |
| `prependToSeq` | plist (893), fig (901), NestedText (902) |
| `removeSeqItem` | NestedText (946) |
| `reorderSeqItems` | NestedText (1293) |
| pre-op guards | TOML `CannotDeleteTable` (778), INI `CannotDeleteSection` (783), fig `CannotDeleteContainer` (792) |
| `NotFound` recovery | YAML merge-key COW (176), YAML `MergeOnlyKey` (765) |

Two shapes hide in here that are worth naming separately, because a manifest
needs both and they are not the same thing:

- **Replacements** — the generic path never runs (`insertKey`).
- **Guards and recoveries** — the generic path runs, but the language gets a
  veto before it (778, 783, 792) or a second chance after it fails (176, 765).

### E. Exclusive operations (9 sites)

Operations only one format has, guarded by `@compileError`.

- TOML: `appendTableToArray`, `deleteTable`, `insertTable`, `renameTable`,
  `moveTable`, `reorderTables` (1422–1452)
- fig: `deleteContainer`, `moveContainer`, `reorderContainers` (1466–1480)

## 3. Proposed shape

Everything a format supplies goes on its `Language` struct. `language.zig`
defines `Syntax`, `Caps`, and the hook allowlist; nothing else does.

```zig
// src/languages/ini/ini.zig
pub const Language = struct {
    // existing four
    pub const Type = ini.Type;
    pub const Parser = ini.Parser;
    pub const default_type: ini.Type = .INI;
    pub const parse = ...;
    pub const print = Printer.print;

    // identity
    pub const name = "ini";
    pub const extensions = &.{"ini"};
    pub const caps: lang.Caps = .{ .read = true, .edit = true, .serialize = true };

    // §2A / §2B / §2C, dialect-indexed
    pub fn syntax(t: Type) lang.Syntax {
        _ = t; // INI has one dialect
        return .{
            .comment_style      = .semicolon,
            .kv_sep             = " = ",
            .line_comment       = ";",
            .trailing_comment   = null, // `;` after a value is literal text
            .empty_map_literal  = null, // no literal spelling → no vivify
            .has_flow           = false,
        };
    }

    // §2D — presence IS the capability
    pub const insertKey      = @import("editor_helper.zig").iniInsertKey;
    pub const deleteKeyGuard = @import("editor_helper.zig").sectionHeaderGuard;
};
```

Three choices, each with a reason.

**`syntax` is a function of `Type`, not a constant.** Forced by
`lineCommentMarker` (§2A): the marker already depends on the dialect, because
the splice is reparsed under that dialect and strict JSON would reject a `//`
we wrote ourselves. The alternatives are per-field `fn(Type)` types — which
scatters the dialect question across the struct — or a single indexed lookup.
The latter is one place, and is where TOML 1.0/1.1 or YAML 1.1/1.2.2 editing
divergence would land if it ever appears. Cost: a runtime switch where there is
a comptime constant today, folded away wherever the dialect is comptime-known.

**Hooks are declarations on `Language`, not a side module.** So

```zig
if (Language == Plist) return plist_edit.plistInsertKey(self, parsed, node, k, v);
```

becomes

```zig
if (@hasDecl(Language, "insertKey")) return Language.insertKey(self, parsed, node, k, v);
```

This is what puts a format's whole answer in one struct. It looks like it
should create a comptime dependency loop — `Language.insertKey`'s type mentions
`Editor(Language)`, and `Editor(Language)` calls `validate(Language)` — but it
does not, and we know because **the cycle already exists and resolves today**:
`ini/editor_helper.zig` declares `const IniEditor = editor.Editor(Ini)` while
`editor.zig` imports `ini_edit`. The constraint this does impose is that
`language.zig` must not import `editor.zig`, so `CommentStyle` moves out of the
editor and into `language.zig` with `Syntax`.

**Exclusive operations stop existing rather than `@compileError`-ing.** Today
`Editor(Yaml).deleteTable` exists and fails with a hand-written message.
Under the manifest, `Editor(L)` declares it only when `@hasDecl(L,
"deleteTable")` — so `@hasDecl(Editor(Yaml), "deleteTable")` is *false*, which
is a fact a binding or the C ABI can query. That is a capability gained, not
just a tidier compile error.

## 4. `validate` as the enforcement point

The manifest is only worth having if something checks it. `validate` grows
three jobs:

1. **Required declarations**, present and correctly typed — `Type`,
   `default_type`, `Parser`, `parse`, `print`, `name`, `extensions`, `caps`,
   `syntax`.
2. **A closed allowlist on optional declarations.** Iterate
   `@typeInfo(Language).@"struct".decls` and reject anything not in the known
   hook set.
3. **Coherence rules.** `trailing_comment == null` alongside a declared
   `setTrailingComment` is a contradiction. So is `caps.edit == false` with any
   edit hook present.

Job 2 is the highest-value item in this document, and it is worth being
explicit about why. The failure mode it prevents is the one `@hasDecl`
dispatch otherwise introduces for free: an author writes `insertkey`,
`@hasDecl(L, "insertKey")` returns false, the format silently gets generic
behaviour, and the first symptom is a corrupted file. A typo becomes silent
data loss. `@compileError("unknown declaration 'insertkey' — did you mean
'insertKey'?")` costs a few lines and removes the entire class.

This is also the part that matters most if a third-party format story ever
happens, because it is the difference between an interface and a convention.

## 5. Staging

**Step 0 — inventory.** §2 of this document, extended with each site's
rationale. This is the real risk in the whole exercise: the comments in
`editor.zig` and `language.zig` are the best documentation in the repository,
and most of them are load-bearing argument, not description — *why* NestedText
is tried after YAML, *why* INI cannot vivify, *why* a flow-mapping delete
cannot use the line path. A refactor that lands the behaviour and loses the
reasoning is a net loss. The rationale moves onto the manifest fields it
justifies; it does not evaporate.

**Step 1 — add the manifest, change nothing.** Every language gains `syntax`,
`caps`, `name`, `extensions`, and hook declarations. No call site moves.
`editor.zig` keeps its branches, and gains comptime bridge assertions:

```zig
comptime std.debug.assert(comment_style ==
    Language.syntax(Language.default_type).comment_style);
```

The compiler now proves the manifest reproduces current behaviour *before*
anything is deleted. This is what turns the rest from a refactor into a
mechanical transformation with a checked bridge, and it is cheap to abandon: if
the manifest turns out to be the wrong shape, Step 1 reverts as a clean delete.

**Step 2 — retire A, B, C** (26 sites). Delete the parameter branches; read the
manifest. The bridge assertions from Step 1 are what is being deleted, so each
removal is individually proven.

**Step 3 — retire D** (30 sites), one operation at a time. Coverage is already
in place: the per-language editor tests deliberately live in the
`editor_helper` modules next to the logic they exercise.

**Step 4 — retire E** (9 sites), the exclusive operations.

**Step 5 — harden `validate`**, once the hook set is empirically known rather
than guessed. Doing this earlier would be codifying a guess.

Steps 1 and 2 are worth doing even if 3–5 never happen: they are where the
"one place" claim is cashed, and they are the low-risk half.

## 6. Open questions

**Should the manifest carry detection order?** It could — a `detect_rank`
field. I think it should not. The ordering argument in `language.zig`'s
`detect` doc comment is *global*: it is about how eleven grammars overlap, and
it is currently comprehensible precisely because it is written once, in order,
in one place. Splitting it into eleven `detect_rank = 7` fields would preserve
the behaviour and destroy the explanation. `extensions` should still move to the
manifest — that retires the `.env` and `.nt` special cases in `cli/args.zig`.

**Where exactly does the parameter/hook line fall?** Proposed criterion: it is
a *parameter* when a hypothetical second format with the same trait would need
the same knob (ZON's key sigil, fig's structural indent, NestedText's dash
anchoring); it is a *hook* when the logic is genuinely structural. Under that
rule only plist and NestedText clearly keep hooks throughout — plist because an
entry is a pair of sibling elements rather than a `key<sep>value` line, and
NestedText because its value framing has no analogue elsewhere. TOML and fig
are borderline and should be decided during Step 3, not in advance.

**How is `validate` itself tested?** Zig has no built-in way to assert that a
`@compileError` fires, so the thing §4 rests on cannot be unit-tested in tree.
The honest answer is a `tools/` script that invokes `zig build-obj` against
deliberately-broken fixture languages and asserts failure, in the spirit of
`tools/fuzz_test_runner.zig`. Worth building; not worth blocking Step 1 on.

## 7. What this unlocks, and what it does not

It does **not** deliver third-party formats. It is a precondition for that
conversation: an interface that is written down and machine-checked is the
thing a plugin could target, and a closed allowlist is the thing that makes a
plugin's mistakes loud instead of silent.

The nearer payoff is in-tree. `caps` is already hand-maintained in a second
place — `fig_format_capabilities` ([src/c_api.zig][c_api]) spells out
`read | edit | serialize` per format in a switch that has to be remembered
whenever a format's support changes. Generated from the manifest, it cannot
drift. The same is true of `extensions` versus `cli/args.zig`. Those two are
the concrete argument that this pays for itself before any plugin exists.

The larger cleanup it makes tractable — and which should stay a separate
proposal — is the five parallel format enumerations and the `build_options`
switches keyed on them. A manifest gives a comptime registry something to
iterate. That is a bigger line-count win than everything above, and a worse
place to start, because it changes the C ABI surface.

[validate]: /src/languages/language.zig
[editor]: /src/editor.zig
[c_api]: /src/c_api.zig
