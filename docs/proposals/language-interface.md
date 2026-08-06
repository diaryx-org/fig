```fig
title = A declared Language interface
description = Replacing editor.zig's per-language type tests with a manifest each format declares, checked by Language.validate
created = 2026-08-05
updated = 2026-08-06
part_of = [proposals](proposals.md)
```

# A declared Language interface

> **Status: COMPLETE.** Steps 1, 2, 3 and 5 implemented, Step 4 withdrawn
> (§8.1), §7's payoff collected. The manifest exists
> (`src/languages/manifest.zig`), all eleven formats declare one, `editor.zig`
> no longer names a format outside its own tests and §2E's nine `@compileError`
> guards, `validate` enforces a closed declaration set checked by
> `zig build validate-check`, and `caps`/`extensions` are read by their
> consumers rather than restated. See §9 for Steps 1–2, §10 for Step 3, §11 for
> Step 5, §12 for §7's payoff.
>
> The follow-on this proposal names but does not attempt — the five parallel
> format enumerations and the `build_options` switches keyed on them — remains
> open and should be its own proposal (§7, §12.4).
>
> The site catalogue in §2 was taken against `main` at 5c7b97b (fig 2.5.3) and
> remains the reference account of what a format has to supply; its line
> numbers are now stale.
>
> **§8 corrects §3, §4, §5 and §7 against the same commit.** One claim in §3 —
> that exclusive operations can stop existing — does not compile on the pinned
> Zig, and Step 4 rested on it. Read §8 before acting on those sections; §10.3
> corrects one more of §8's own claims.

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

## 8. Review corrections (2026-08-06)

Reviewed against the same commit the body was written against, 5c7b97b.

§2 survives intact. The 65 type-test sites, the 35 public operations, the
182/51/27 `build_options.lang_*` gates, and every line number in the §2 tables
land on the site they name. So does the comptime-cycle argument in §3:
[`ini/editor_helper.zig`][ini_edit] line 46 declares `Editor(Ini)` while
[editor.zig][editor] line 45 imports `ini_edit`. So do the `.env`/`.nt` special
cases §6 proposes to retire ([cli/args.zig][args], lines 102 and 107). Step 3's
coverage claim is real: roughly 295 tests across the eight `editor_helper`
modules, the largest being yaml (94), toml (63) and fig (61).

Four things in §3–§7 are wrong, and three arguments the body could have made
are missing.

### 8.1 Exclusive operations cannot un-declare themselves (§3)

> Under the manifest, `Editor(L)` declares it only when `@hasDecl(L,
> "deleteTable")` — so `@hasDecl(Editor(Yaml), "deleteTable")` is *false*

Zig has no conditional container-level declarations. `usingnamespace` was the
only mechanism that ever provided them, and it was removed in 0.15;
[build.zig.zon][zon_manifest] pins `minimum_zig_version = "0.16.0"`, where the
keyword no longer parses. The nearest workaround is a namespace field —

```zig
pub const tables = if (@hasDecl(L, "deleteTable")) TomlTableOps(Self) else struct {};
```

— which relocates every call site to `Ed.tables.deleteTable(...)`, the C ABI's
included. That is a worse trade than the `@compileError` it replaces.

The claimed *benefit* survives without any of it. `@hasDecl(Language,
"deleteTable")` — on the manifest, where §3 already puts the hook — is the fact
a binding or the C ABI wants to query, and `Editor` never has to change to
provide it. So the capability §3 argues for is real and free; the mechanism it
proposes is neither.

**Consequence for §5: Step 4 drops.** §2E's nine sites are the cheapest in the
document to leave exactly as they are.

### 8.2 Step 1's bridge does not cover the comment markers (§5)

The bridge assertion proves what §5 claims for the four comptime constants. It
cannot work for `lineCommentMarker`/`trailingCommentMarker`, which are the two
sites §2A already singled out as awkward. They branch on the runtime
`self.format` ([editor.zig][editor] line 489), so there is no comptime value to
assert against, and asserting against `syntax(Language.default_type)` inspects
only strict JSON — the single dialect whose answer is `null`, and therefore the
one that proves least.

Covering them honestly needs `inline for (std.meta.tags(Language.Type))` plus a
comptime-callable extraction of the current logic — which is itself a change to
`editor.zig`, and so breaks Step 1's "change nothing" promise for exactly these
two functions. Either exempt them explicitly or move them into Step 2. As
written Step 1 claims a proof it does not deliver, in the one place §2A
predicted the difficulty would be.

### 8.3 The allowlist inventory is incomplete (§4)

§4's required set is `Type, default_type, Parser, parse, print, name,
extensions, caps, syntax`. Every language today also declares `printNode` (all
but plist and xml, whose `print` is written inline), and yaml declares
`materialize` and `TagMode` besides. A closed allowlist built from §4's list
rejects the entire tree on the day it lands. Job 2 is the highest-value item in
the document by its own argument, so its inventory has to be exact — taken from
the decls that exist, not from the ones the design remembers.

Two smaller notes on the same section: §3 labels five declarations "existing
four", and §4 promotes `Parser` to required, which [`validate`][validate] does
not check today. The tightening is right; it should be named as one.

### 8.4 `validate` never runs for a read-only format (§4)

Its only call site is [editor.zig][editor] line 74, inside `Editor()`. XML has
no editor, so `validate(Xml)` is never instantiated and XML's manifest would go
unchecked — the enforcement point would miss the one format whose `caps` field
carries the most information. §4 needs a second call site: a comptime loop over
the compiled-in languages in [`language.zig`][validate], which already names
them all.

Relatedly, requiring `syntax` unconditionally is wrong for a format that cannot
be edited. The required set should key off `caps.edit`, which is an extension of
job 3's coherence rules rather than a new mechanism.

### 8.5 What the manifest can actually generate (§7)

The generated part of `fig_format_capabilities` ([c_api.zig][c_api]) is eleven
hand-written `read | edit | serialize` triples. The `build_options` gate stays,
and so does the `FigFormat`→`Language` mapping — which is the part that has
actually drifted before, as the comment at [cli/args.zig][args] line 546
records. `FigFormat` is per-dialect (json/jsonc/json5 all map to one `Language`)
while `caps` is per-language, so that mapping cannot be generated from this
manifest. A real win, but "cannot drift" claims more than it buys.

### 8.6 Three arguments the body leaves on the table

**The import list is the concrete form of the claim.** `editor.zig` hard-imports
nine language type tags and eight helper modules (lines 21–71). `@hasDecl`
dispatch deletes all seventeen. That is what "generic over a closed set rather
than over an interface" cashes out to, and it is more persuasive than the site
count — a reader can check it in one screen.

**`syntax(Type)`'s cost is lower than §3 concedes.** Every use of `kv_sep` and
`comment_style` is an `appendSlice` call or an argument to
`commentBlockStart`/`entryBlockStart`. None requires a comptime value — no array
lengths, no `++`, no switch prongs. The runtime switch is the entire cost, with
nothing downstream of it.

**The vivify merge is provable, not merely plausible.** `emptyMapLiteral` has
exactly one call site, [editor.zig][editor] line 348, inside the vivify branch
itself. Folding §2B's INI/plist/NestedText exclusion into `empty_map_literal:
?[]const u8 = null` is therefore behaviour-preserving by construction. Worth
stating, because it is the one place in §2 where a capability opt-out and a
syntax parameter collapse into a single field — which is the whole thesis of
§2C's "push traits down into parameters", demonstrated rather than asserted.

### 8.7 Net

Steps 1 and 2 stand, including §5's argument that they are worth doing alone.
Step 3 remains the real risk and the body is honest about it. Step 4 should go
(§8.1). §4 is the right idea and needs §8.3 and §8.4 before it is implementable.

## 9. Outcome (2026-08-06)

Steps 1 and 2 landed together, as two commits so the bridge in §5's Step 1
exists in history with CI green against both halves.

**What is in tree.** [`src/languages/manifest.zig`][manifest] defines `Syntax`,
`Caps`, `CommentStyle` and `KeyStyle`. It is a leaf — it imports nothing, not
even `std` — which is what makes the manifest safe for all eleven
`<lang>/<lang>.zig` files to import while `language.zig` imports each of them
in turn. §3 put these types in `language.zig`; that would have been a cycle.

Each language declares `name`, `extensions`, `caps`, and (when `caps.edit`)
`syntax`. `validate` requires them, adds `Parser` as §8.3 recommended, and runs
from a comptime registry loop over the compiled-in languages as well as from
`Editor()` — so XML is checked, closing §8.4.

**23 of the 65 type-test sites are gone**, not the 26 §5 predicted. The gap is
the three NestedText `.index` sites (§2's 537, 629, 667): §2C files them under
micro-parameters, but each calls `nt_edit.seqItemLineStart`, so they are hooks
and belong to Step 3. Every one of the 41 remaining sites is §2D or §2E.

`editor.zig` now holds **no ZON, dotenv or `.properties` branch at all** —
those three tags survive only in its own tests. That is §8.6's import argument
partially cashed without any hook dispatch.

**Two deviations from the plan.**

*Hook declarations were not added.* §5's Step 1 says every language gains them.
Nothing in Steps 1–2 consumes a hook, and adding them now would put
`@TypeOf(Language.insertKey)` — whose signature names `Editor(Language)` —
within reach of a `validate` that runs inside `Editor()`'s own body, before
that type is complete. `validate`'s typed checks are exactly what §4 wants to
grow, so the safe shape is to keep the in-`Editor` call shallow and do typed
and coherence checks from the registry loop, where no instantiation is in
flight. That is a Step 3 decision; nothing was committed to here.

*The comment markers moved to Step 2 rather than being exempted.* §8.2's fork,
resolved the second way. `lineCommentMarker`'s dialect logic was extracted to a
comptime-callable `fn(Type)` in Step 1 — a mechanical change that does break
Step 1's "change nothing" promise for that one function, and buys a bridge
assertion over `std.meta.tags(Language.Type)`, i.e. every dialect rather than
just `default_type`. plist is exempt from those two assertions alone: all six
comment ops delegate to `plist_edit` before a marker is read, so today's
fall-through `"#"` is dead code. The manifest declares null there instead —
plist has no line-comment leader, only a `<!-- -->` pair — so a dropped
delegation would fail loudly rather than splice a half-open comment.

**§7's payoff is not collected yet.** `fig_format_capabilities` still spells
out its eleven triples by hand and `cli/args.zig` still special-cases `.env`
and `.nt`. `caps` and `extensions` are declared and validated but unread —
wiring them is a follow-on, and per §8.5 it is a smaller win than the body
claims.

Verification: `zig build check` is byte-identical to the pre-change baseline
(2278/2281 tests, one pre-existing TypeScript-binding failure), every
conformance corpus is at baseline, and three language-gating configurations
build. Both bridge and registry loop were confirmed live by deliberately
breaking a manifest value and a required declaration.

## 10. Outcome, Step 3 (2026-08-06)

All 32 §2D sites are gone. `editor.zig` now contains **no `if (Language == X)`
at all**: the nine `Language != Toml`/`!= Fig` guards on §2E's exclusive
operations remain, exactly as §8.1 concluded they should, and the language tags
otherwise survive only for this file's own tests.

**Shape.** A format declares a hook as a `pub` decl on its `Language` struct,
named for the `Editor` method it takes over, in an "Editing hooks" block below
`syntax`. The engine dispatches on presence:

```zig
if (@hasDecl(Language, "insertKey"))
    return Language.insertKey(self, parsed, path, node, span, key_text, value_text);
```

Seventeen distinct hook names, thirty declarations, across six formats —
plist 10, NestedText 8, fig 5, YAML 3, TOML 2, INI 2. The bodies stay in
`<lang>/editor_helper.zig`; what moved onto `<lang>/<lang>.zig` is the
declaration *and the reason*, which is where §5's Step 0 wanted the rationale
comments to land. Each hook's signature is fixed by its single call site and
documented on the method it overrides, in a `**Hook**` paragraph.

The comptime-cycle argument in §3 holds up: `<lang>/<lang>.zig` importing its
own `editor_helper.zig` — which imports `editor.zig`, which imports
`<lang>/<lang>.zig` — resolves, because `@hasDecl` does not force the decl's
value and the declaration itself is lazy.

### 10.1 Four things the plan did not anticipate

**A predicate hook, not an operation override.** §2D filed YAML's two
merge-key sites under "guards and recoveries", and §3 offers only whole-op
replacement for them. But the two sites do *different* things with the same
answer — `replaceValAtPath` shadows the inherited key with a local entry,
`deleteKey` refuses with `MergeOnlyKey` — so what is language-specific is the
QUESTION, not the policy. It landed as `keyIsInherited(parsed, path) !bool`,
consulted through a private `Editor` wrapper that returns false when no hook
exists. That default is the whole reason both `NotFound` recovery sites now
carry no language test: a format with no reference layer answers false, which
is correct rather than merely convenient.

**Three sites collapsed before dispatch, not after.** §2's site count assumes
one dispatch per branch. Two hooks beat that: `seqItemLineStart`'s three
identical guards became one private `leadingCommentLineStart`, and
`keyIsInherited`'s two became one private wrapper. Five §2D/§2C sites, three
dispatch points. That is a real reduction the plan had no way to predict from
counting branches, and it argues for reading the sites before pricing the work.

**`deleteKeyGuard` moved code out of `editor.zig`, not just dispatch.** TOML's
`CannotDeleteTable` and fig's `CannotDeleteContainer` were written *inline* in
`deleteKey`, not delegated — only INI's was a helper call. Unifying the three
under one hook meant lifting TOML's and fig's logic into their
`editor_helper.zig` as `tableDeleteGuard`/`containerDeleteGuard`. The three
turn out to refuse the same hazard in three grammars — an entry whose value is
a SCATTERED container that a line-based delete would half-remove — which is
now stated once, at the dispatch.

**One dispatch position had to move.** plist's `appendToSeq`/`prependToSeq`
branches sat *above* the generic `isFlow` and `block_seq_editable` checks while
fig's and NestedText's sat below, and one hook cannot occupy two positions. It
was unified at the lower position, which is behaviour-preserving for plist by
construction rather than by testing: a plist container's span runs from its
opening `<` (`plist/parser.zig`'s `extent`), so `isFlow` — which tests for `{`,
`[` or `.{` — is false for every plist node, and `block_seq_editable` is the
declared default. This is the only place in Step 3 where the transformation was
not purely mechanical.

### 10.2 Signature normalization

A hook has one call site, so it has one signature, so the per-format helpers
had to converge on it. Five changes, all mechanical:

- The three narrow `insertKey` helpers (INI, plist, NestedText) widened to the
  full `(self, parsed, path, node, span, key_text, value_text)`.
- TOML's and fig's `insertKey` dropped their `is_root: bool` parameter and
  derive it as `path.len == 0`, removing an argument that could disagree with
  the `path` beside it.
- YAML's and fig's `reframeMappingValue` absorbed the
  `activeTag(path[path.len - 1]) == .key` guard that used to sit in the engine,
  falling through to `self.replaceAtSpan` themselves. The guard was never
  generic — it is the statement "only a mapping value can change framing",
  which is a claim about those two grammars.
- plist's and NestedText's replace helpers widened to the same
  `(self, parsed, path, node, span, replacement)`.
- YAML's alias-follow branch became `replaceAliasTarget`, which hands a
  non-alias target back to `self.replaceValAtPath` — which is precisely
  `replaceValAtPathFollowing`'s documented contract, so the fall-through in the
  engine and the fall-through in the hook say the same thing.

Ignored parameters (`_ = path; _ = span;`) are the cost, and they are worth
paying: a uniform signature is what §4's typed checks will have to assert
against, and a hook that receives the engine's full context can be re-scoped
later without touching the call site.

### 10.3 §8.6's import claim was too strong

> `editor.zig` hard-imports nine language type tags and eight helper modules
> (lines 21–71). `@hasDecl` dispatch deletes all seventeen.

Four of seventeen. The helper imports for YAML, INI, plist and NestedText are
gone; `toml_edit` and `fig_edit` stay for §2E's exclusive operations, and
`zon_edit` stays because `appendFieldName` is reached through `key_style` — the
rendering half of a syntax parameter, not an operation override. All nine tags
stay: `Toml` and `Fig` for the §2E guards, the rest because `editor.zig`'s own
tests instantiate `Editor(Yaml)`, `Editor(Zon)` and so on.

The underlying claim survives in the form that matters, and is stronger than a
count: **`editor.zig`'s non-test code names no format except in §2E's nine
compile-error guards.** The remaining imports are not couplings of the engine
to a format — they are the exclusive-operation surface and the test surface,
both of which are supposed to name formats.

### 10.4 §6's open question, answered

> Under that rule only plist and NestedText clearly keep hooks throughout …
> TOML and fig are borderline and should be decided during Step 3.

Both kept hooks, and for different reasons. TOML's two are genuinely
structural: `insertKey` has to scan a table's header region to find where a new
entry attaches without being reparented, which is not a knob. fig's five are
less clear-cut — `appendToSeq`/`prependToSeq` exist to copy the `>` marker-run
prefix, and `structural_indent` already declares that trait as a parameter, so
generalizing `insertSeqLine` to consult it would plausibly retire both. That
was not attempted here: it changes generic code rather than moving it, which is
a different risk than the rest of Step 3, and it should be its own change.

The parameter/hook line held everywhere else. Nothing that landed as a hook
looks like a knob in retrospect, and the ZON case that motivated §2C's
"push traits down" stayed retired — ZON still declares no hooks at all.

### 10.5 What Step 5 now knows

§5 deferred hardening `validate` until "the hook set is empirically known
rather than guessed". It now is: seventeen names, listed above, plus the
required set §9 already enforces and the `printNode`/`materialize`/`TagMode`
decls §8.3 catalogued.

The typo hazard §4 argues from is not hypothetical, and was confirmed live:
renaming NestedText's `insertKey` declaration to `insertkey` **compiles
cleanly**, silently falls back to the generic block insert, and is caught only
because NestedText happens to have tests over that exact path. A format
overriding an operation *because the generic one corrupts its files* is exactly
the case where the fallback is worst and the coverage is least certain. §4's
job 2 is the fix, and it is now specifiable rather than guessable.

Two smaller notes for that work. `keyIsInherited` and `seqItemLineStart` are
not `Editor` method names — they are a predicate and a sub-computation — so an
allowlist cannot be derived mechanically from `Editor`'s public surface. And
§9's reason for not adding typed checks inside `Editor()` still stands:
`@TypeOf(Language.insertKey)` names `Editor(Language)`, so the typed and
coherence checks belong in `language.zig`'s registry loop, where no
instantiation is in flight.

### 10.6 Verification

`zig build check` is byte-identical to the pre-change baseline — 2278/2281,
3 skipped, the one failure the pre-existing TypeScript-binding test, which
fails the same way on the parent commit. Every conformance corpus is at
baseline, plist and NestedText included (both only compile under `zig build
conformance`, which was run after every hook family rather than only at the
end). Four language-gating configurations build: all optional formats off;
everything on including plist, xml and canonical; plist on with NestedText and
INI off; and the default. Configurations that disable json, yaml, toml or fig
fail on the CLI and LSP's hard dependencies on those formats — on clean `main`
as well, so they are not gating configurations this change can be held to.

Dispatch was confirmed live by the typo experiment in §10.5: with one hook
declaration misspelled, three NestedText tests fail with the generic
implementation's output. Nothing about the hook set is inferred from the fact
that the suite passes.

**§7's payoff is still not collected.** `fig_format_capabilities` spells out
its eleven triples by hand and `cli/args.zig` still special-cases `.env` and
`.nt`. That is untouched by Step 3 and remains the cheapest work left.

## 11. Outcome, Step 5 (2026-08-06)

`validate` now enforces a closed declaration set and four coherence rules, and
`zig build validate-check` asserts that it actually refuses. Step 5 was
supposed to be last; it went second-to-last because **Step 3 made the hazard it
closes strictly worse.** Before hooks, a mistyped format name
(`if (Language == Plsit)`) could not compile. After hooks, `pub const insertkey`
compiles, silently runs the generic implementation the format overrode
*because that implementation corrupts its files*, and the first symptom is a
bad write. Step 3's win was not fully paid for until this landed.

### 11.1 The inventory, taken rather than remembered

§8.3's warning — that an allowlist built from §4's remembered list "rejects the
entire tree on the day it lands" — was taken seriously: the set was extracted by
a throwaway comptime probe over `@typeInfo(Lang).@"struct".decls`, not by
reading files. Three things came out of it that a careful reading would have got
wrong:

- **`decls` lists only PUBLIC declarations.** The `const edit =
  @import("editor_helper.zig")` that opens every hooks block is invisible to the
  allowlist and needs no exemption. Had that gone the other way, every format
  would have needed a special case.
- **The distinct hook count is 17, not the 18 §10 first claimed.** Thirty
  declarations across six formats, but `insertKey`, `replaceValAtPath`,
  `appendToSeq`, `prependToSeq` and `deleteKeyGuard` each recur. §10 has been
  corrected.
- **plist has no `printNode`,** matching §8.3 exactly, and xml has neither
  `printNode` nor `syntax` — the read-only case `required_edit` exists for.

### 11.2 §4's own coherence rule was wrong

> `trailing_comment == null` alongside a declared `setTrailingComment` is a
> contradiction.

plist is exactly that pair, and plist is correct. It declares
`trailing_comment = null` because `<!-- ... -->` is a delimiter pair with no
leader, and hooks all six comment ops precisely so the null is never read. A
hook does not consult the marker; a null marker beside a hook is not a
contradiction, it is the hook making the marker irrelevant.

The useful rule is the near-opposite, and it is the one now enforced: with no
line-comment marker in **any** dialect, every comment op is either hooked or
permanently `CommentsUnsupported`, so hooking *some but not all* is a dropped
delegation rather than a decision. `plist.zig` already wrote down the fear this
addresses — "if a delegation were ever dropped, the op fails loudly with
`CommentsUnsupported`" — and this turns that runtime refusal into a compile
error.

The four rules that landed: a trailing marker with no line marker (pre-existing);
`caps.edit = false` beside any editing hook; a block-sequence hook under
`block_seq_editable = false`, which the engine refuses before ever reaching;
and the comment rule above. The last two are dead-code checks, provable from
where §10's dispatch sits — a hook the engine cannot call is a silent no-op,
and silent is the whole problem.

### 11.3 Typed hook checks were considered and not built

§4 job 1 wants declarations "present and correctly typed". Presence is now
enforced; typing is not, and the reason is empirical rather than budgetary. Two
probes:

- Binding a hook to a **wrong-signature** function already fails loudly, at the
  call site: `editor.zig:697: expected 3 argument(s), found 7`. A typed check in
  `validate` would move that message, not create it.
- Binding a hook to the **wrong function of the right signature** —
  `appendToSeq = ntPrependItem` — compiles, and no type system catches it. Three
  NestedText tests do.

So typed checks sit between an error that is already loud and an error they
cannot see. The detection gap was the *name*, and the allowlist closes it. This
is recorded rather than left implicit because "correctly typed" reads like an
omission otherwise.

§9's constraint still holds and is why this is not merely deferred:
`@TypeOf(Language.insertKey)` names `Editor(Language)`, so any typed check must
run from the registry loop, never from inside `Editor()`.

### 11.4 The harness (§6's open question, closed)

> Zig has no built-in way to assert that a `@compileError` fires, so the thing
> §4 rests on cannot be unit-tested in tree.

[`tools/validate-check.zig`][vcheck] is the `tools/` script §6 proposed. Nine
fixture languages, each with one deliberate defect, each compiled with
`zig build-obj` and asserted to fail *with the message its rule should produce*
— matching on a substring chosen to pin the rule rather than the phrasing. Wired
to `zig build validate-check` and into `zig build check`.

Two design points worth keeping:

**The positive control is load-bearing.** Case 0 is a well-formed fixture that
must COMPILE. Without it, a probe that stops building for an unrelated reason
turns every negative case green while testing nothing — the characteristic
failure of compile-failure suites. It is not a hypothetical: it fired twice
during development, both times on module wiring, and reported harness breakage
instead of nine false passes. The harness was also meta-tested by disabling the
allowlist in `validate`, which turned exactly the two cases that rule owns red
and left the other seven alone.

**The fixture module has to be rooted at `src/root.zig`.** A probe in a work
directory cannot `@import` an absolute path outside its module path, and rooting
the module at `languages/language.zig` puts `src/document.zig` and
`src/editor.zig` — which the language modules reach for — outside it. The
fixture's `build_options` compiles every real format out, so `language.zig`'s
registry loop skips all eleven and the fixture is the only thing under test.

### 11.5 Verification

`zig build check` is at baseline plus the new step — 2278/2281, 3 skipped, the
one failure still the TypeScript-binding test, which needs Node 24 (spun off
separately; it is a dev-environment floor, not a product bug, since `dist/*.js`
carries no raw `using`). Conformance at baseline. Four language-gating
configurations build. `validate-check` reports 9/9.

One incidental cost: the closed-set scan is `decls × known-names` per format
across eleven formats, which exceeds Zig's default 1000-branch comptime budget,
so `validate` raises it to 20,000. That is a per-evaluation quota, not a leak.

**What is left of this proposal is §7's payoff and nothing else.**
`fig_format_capabilities` still spells out its eleven `read | edit | serialize`
triples by hand and `cli/args.zig` still special-cases `.env` and `.nt`, while
`caps` and `extensions` sit declared, validated and unread. Per §8.5 it is a
smaller win than the body claims — the `FigFormat`→`Language` mapping is
per-dialect and cannot be generated from here — but it is now the only step
still standing.

## 12. Outcome, §7's payoff (2026-08-06)

`caps` and `extensions` were declared and validated from Step 1 onward but read
by nothing — the two consumers §7 names kept their own copies. Both now read
the manifest, and both are checked.

### 12.1 `caps` → `fig_format_capabilities`

The eleven hand-written `read | edit | serialize` triples in
[`c_api.zig`][c_api] are gone; the function reads each format's declared `caps`.
The drift this closes was silent in *both* directions — a format gaining an
editor without its bit being set reads as uneditable to every C host, and a bit
set for support that doesn't exist sends hosts down a path that ends in
`unsupported_format`.

Two things fell out that §8.5 did not predict:

**The `build_options` gate went away too.** §8.5 says "the `build_options` gate
stays". It doesn't need to: a compiled-out format is already `void` in
`language.zig`, so `if (Lang == void) return 0` expresses the same fact the
eleven `if (comptime build_options.lang_*)` tests did. The function now names no
build option at all.

**The existing test became a real cross-check.** `fig_format_capabilities
reports the per-format matrix` spells its expectations out by hand,
independently of the implementation — so leaving it untouched turns it from a
tautology into a genuine second opinion. Confirmed by flipping TOML's
`caps.serialize` to false: the test fails with `expected 7, found 3`. Flipping
XML's `caps.edit` to true is caught even earlier, by Step 5's coherence rule
(`caps.edit` without `syntax`), which is the two halves of this work
compounding rather than either one alone.

What is NOT generated, exactly as §8.5 says: the `FigFormat`→`Language` mapping.
`FigFormat` is per-dialect (json/jsonc/json5 are three ABI values over one
`Language`) while `caps` is per-language, so they are different tables and only
the second lives in the manifest.

### 12.2 `extensions` → `cli/args.zig`

The `.figl`, `.env` and `.nt` special cases are gone, replaced by a lookup over
each language's declared `extensions`.

The ordering is the whole of the care required here. `std.meta.stringToEnum`
runs FIRST and the manifest lookup is the fallback — which is not the obvious
arrangement, and is what makes the change behaviour-preserving. `cli.Format` has
members no `Language` owns (`canonical`, `gron`) and, more sharply, `yml` is its
own member rather than an alias of `yaml`, handled as a distinct value at eight
sites in `actions.zig`/`diag_report.zig`. A manifest-first lookup would have
quietly rerouted every `.yml` file to `Format.yaml`. Enum-first leaves every
extension that names a member exactly as it was, and lets the manifest answer
only for the three that never did.

Verified end-to-end against the built CLI for `.yaml`, `.yml`, `.env`, `.nt`,
`.figl`, `.fig`, `.toml`, `.ini`, `.json` and `.properties`, and by diffing
`.figl` behaviour against the pre-change binary.

### 12.3 A registry, which is the part that generalizes

Both consumers need a per-language table, and a table can go stale as easily as
a copied value — so `language.zig` now exposes `compiled`, the comptime list of
compiled-in languages, with gated-out formats absent rather than `void`. Its
own `validate` loop iterates it instead of repeating eleven `build_options`
tests, and `cli/args.zig` walks it to fail the build if a language is missing
from the extension table. That check is not decoration: a language absent from
the table would simply stop resolving by extension, and the symptom — "that
file just isn't detected" — is precisely the quiet kind this proposal exists to
remove.

This is the "comptime registry [with] something to iterate" §7 names as what
the manifest unlocks. Worth being precise about its reach: it is per-LANGUAGE,
and the five parallel enumerations are per-DIALECT, so it does not retire them.
What it does is make the per-language half a solved problem, which is the half
both of §7's consumers needed.

### 12.4 Verification, and what is left

`zig build check` at baseline (2278/2281, 3 skipped, the one failure still the
Node-24 TypeScript test), conformance at baseline, `validate-check` 9/9, four
language-gating configurations build, and `abi-check` reports the C ABI
unchanged at 86 symbols — this touched what `fig_format_capabilities` returns
for a *misdeclared* format, never its exported surface. Both new checks were
confirmed to bite: dropping NestedText from the extension table fails the build
by name, and flipping a `caps` bit fails the C ABI matrix test.

**This proposal is done.** What it deliberately leaves is what §7 always said
should be separate: the five parallel format enumerations (`Language.Detected`,
`cli.Format`, `c_api.FigFormat`, `serialize_options.Format`,
`Embed.InnerFormat`) and the `build_options.lang_*` switches keyed on them —
182 sites in `c_api.zig`, 51 in `ast/serialize_options.zig`, 27 in `embed.zig`,
plus roughly 119 in `src/cli` and `src/lsp` that make four of the eleven
`-D<lang>=false` flags fail to build. That is a bigger line-count win than
everything here and a worse place to have started, because it changes the C ABI
surface. It now has a registry to build on.

[vcheck]: /tools/validate-check.zig
[manifest]: /src/languages/manifest.zig
[validate]: /src/languages/language.zig
[editor]: /src/editor.zig
[c_api]: /src/c_api.zig
[args]: /src/cli/args.zig
[ini_edit]: /src/languages/ini/editor_helper.zig
[zon_manifest]: /build.zig.zon
