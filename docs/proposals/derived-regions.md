```fig
title = Derived regions
description = Moving "where are this node's regions" from three editor_helper.zig gathers into Document, so the whole-container ops and the section guards are generic
created = 2026-08-28
updated = 2026-08-28
part_of = [proposals](proposals.md)
```

# Derived regions

> **Status: IMPLEMENTED** on branch `worktree-agent-a45ed60ef4bf25740`, against
> `main` at 013ea08, as the first preliminary refactor for fig 3.0's pluggable
> formats. `Document.reentry_headers` is gone; `Document.node_regions`
> replaces it, filled by the fig, TOML and INI parsers. `deleteContainer`,
> `moveContainer`, `reorderContainers` and the four line-splice guards are
> generic in `src/editor.zig` over `src/editor/regions.zig`. `insertContainer`,
> `renameContainer` and `appendContainerToSeq` stay hooks, for the reasons in
> §6. No C ABI change (`zig build abi-check` green, `fig.h` untouched); CLI
> output byte-identical for every shape the old code handled correctly; the
> library-level behaviour changes are listed in §8.

The language-interface proposal's §14 moved everything *downstream* of the
region gather into one shared module and left the gather itself per format,
with the sentence: "which lines belong to this container" is the one genuinely
per-format question. This proposal takes that sentence apart. The gather was
per-format only because each parser kept the one fact the editor needs —
which physical lines introduced a container — to itself, and each editor
helper then re-derived it by sniffing source (`[`-lines in TOML, `isFlow` plus
a re-entry side-table in fig, section membership in INI). Record the fact once,
in `Document`, and the gather is the same function three times.

## 1. Scope

In scope: `Document.reentry_headers`, the six whole-container ops and four
`*Guard` hooks in `src/languages/{toml,fig,ini}/editor_helper.zig`, the
`Decls.hooks`/`Decls.exclusive` sets in `src/languages/language.zig`, and
`src/languages/shared/sections.zig`.

Not in scope, deliberately: how a format *spells* a new fragment. Two of the
three ops that stay hooked exist only to write a `[header]` line, and one
could imagine a "fragment printer" contract that would make them generic too.
That is the harder half of pluggable formats (see the fig 3.0 planning note)
and this proposal does not design it — §6 records what such a contract would
have to answer, and stops there.

## 2. Site catalogue

Line numbers are against `main` at 013ea08. Each site is classified by what it
needs from the source that the generic engine cannot see:

- **(a)** *where are this node's regions* — the physical lines a logical
  container is assembled from;
- **(b)** *how to spell a new fragment* — a `[header]` line, a rendered key;
- **(c)** *a veto rule* — a refusal the format states about a generic op.

A fourth kind turned up in TOML and is kept apart because the table does not
answer it: **(a′)** *where is this node's NAME written* — every source
position that spells a table's key, which for a dotted table is one per line.

### 2.1 TOML — `src/languages/toml/editor_helper.zig`

| Site | Lines | Needs | Kind |
|---|---|---|---|
| `subtreeMaxEnd` | 50–65 | max span end over a subtree, then its line end — where a parent's extent stops | (a) |
| `headerLineRegion` | 81–91 | the `[`-line a table's key segment sits on, with owned comment block | (a) |
| `headerLineAtOrAbove`/`AtOrAfter` | 99–121 | an AoT element's `[[…]]` line, recovered by scanning because every element shares the array's span | (a) |
| `gatherTableRegions` | 129–156 | classify each child by whether its line starts with `[`: recurse or take the entry line | (a) |
| `gatherAotRegions`/`gatherElementRegions` | 158–199 | the same for `[[…]]` elements, header by scan | (a) |
| `headerSegmentSpan`/`keySegmentSpan`/`dottedIndexOfKey` | 206–283 | the span of segment *n* of a dotted path on a given line | (a′) |
| `normalizeRegions` | 287–289 | coalesce on overlap only, so rename can address each header region's start | — |
| `appendTomlHeaderPath`/`isTomlBareKey` | 295–327 | render a path as `a."b c".d` | (b) |
| `tableDeleteGuard`/`opensHeaderLine` | 357–370 | refuse `deleteKey` when the entry's line starts with `[` | (c) |
| `tableMoveGuard` | 386–391 | refuse `moveKey` when src *or dest* line starts with `[` | (c) |
| `tableReorderGuard` | 408–413 | refuse `reorderKeys` when a moved entry's line starts with `[` | (c) |
| `tableReplaceGuard` | 434–446 | refuse `replaceValAtPath` on a non-root block container (`!isFlow`) — which, unlike the other three, also catches a dotted table | (c) |
| `tomlReplaceKey` | 465–475 | route a block container's key rename to the multi-mention rewrite | (a′) |
| `appendTableToArray` | 566–594 | (a) `subtreeMaxEnd` of the last element; (b) spell `[[path]]` | (a)+(b) |
| `deleteTable`/`aotElementSearchFrom` | 609–657 | (a) gather; the index-path case needs the previous element's end to find an empty element's header | (a) |
| `insertTable` | 659–694 | (a) `subtreeMaxEnd` of the parent; (b) spell `[path]`; existence check | (a)+(b) |
| `renameTable`/`renameTableSegments`/`appendDottedNameSpans` | 696–821 | (b) render the new leaf; (a) gather regions; (a′) the name's span in each header region and on each dotted line | (a)+(a′)+(b) |
| `moveTable` | 823–854 | (a) gather src; dest is the `[`-line of the dest table (a dotted dest is refused) | (a) |
| `reorderTables` | 856–899 | (a) gather per name | (a) |

### 2.2 fig — `src/languages/fig/editor_helper.zig`, `parser.zig`

| Site | Lines | Needs | Kind |
|---|---|---|---|
| `containerDeleteGuard` | 208–213 | refuse `deleteKey` when the value is a block (non-flow) mapping or sequence — *every* one, contiguous or not | (c) |
| `headerLineRegion`/`entryLineRegion`/`commentBlockStart` | 373–391 | the line a node's span starts on, with owned `#` block | (a) |
| `gatherContainerRegions`/`gatherChild` | 403–452 | classify each child by value kind plus `isFlow`: recurse (adding its header line and re-entries) or take the entry line | (a) |
| `appendReentryHeaderLines` | 454–458 | the later header lines that re-opened this node — `Document.reentry_headers`, the only fact not derivable from spans | (a) |
| `gatherKeyedContainer` | 473–491 | resolve the path; refuse root, scalar, flow | (a)+(c) |
| `deleteContainer`/`moveContainer`/`reorderContainers` | 495–568 | (a) gather; move's dest is any non-flow node's line | (a) |
| `parser.zig` `reentries.append` | 1282, 1308 | a header-final re-open, recorded at `resolveHeaderFinal` | parser side of (a) |
| `parser.zig` `recordReentries` | 2108–2112 | `PendingContainer` → `Document.reentry_headers` | parser side of (a) |

fig has no `moveKeyGuard`, `reorderKeysGuard` or `replaceValGuard`. Its node
spans are *widened* to the subtree's end at AST assembly (`buildNode`), so the
generic line ops are correct for a contiguous container; for a re-entered one
they silently move or reorder the first fragment only. The delete guard was
the one defensive refusal.

### 2.3 INI — `src/languages/ini/editor_helper.zig`, `parser.zig`

| Site | Lines | Needs | Kind |
|---|---|---|---|
| `sectionDeleteGuard`/`isSectionHeaderLine` | 81–152 | refuse `deleteKey` when the entry's line starts with `[` | (c) |
| `sectionReplaceGuard` | 98–105 | refuse `replaceValAtPath` on any non-root mapping (INI has no flow syntax, so that is always a section) | (c) |
| `sectionMoveGuard` | 115–120 | refuse `moveKey` when src or dest is on a `[` line | (c) |
| `sectionReorderGuard` | 133–138 | refuse `reorderKeys` when a moved entry is on a `[` line | (c) |
| `headerLineRegion`/`gatherSection` | 179–207 | the section's header line, every reopened header from `reentry_headers`, each entry's line; no recursion | (a) |
| `deleteContainer`/`moveContainer`/`reorderContainers` | 211–269 | (a) gather | (a) |
| `parser.zig` `built_reentries.append` | 368 | a reopened `[a]`, at the merge branch | parser side of (a) |

### 2.4 The engine — `src/editor.zig`, `language.zig`

| Site | Lines | What |
|---|---|---|
| `replaceValGuard` dispatch | 188–214 | `@hasDecl` veto before the splice |
| `deleteKeyGuard` dispatch | 769 | same |
| `moveKeyGuard` dispatch | 1138–1153 | same |
| `reorderKeysGuard` dispatch | 1197–1278 | computes the `moved` list only to hand it to the hook |
| six `requireSectionOp` wrappers | 1470–1535 | thin `@hasDecl` dispatch; `hasContainerOp` for the C ABI |
| `Decls.hooks`/`Decls.exclusive` | 851–879 | four guard names; six op names |

### 2.5 What the catalogue says

Every (a) site is one of two things: "the header lines of this node" — which
only the parser knows for a re-entered header, and which TOML's AoT code
*recovers by scanning* because the parser did not say — or "the line of this
contiguous child", which is its span. Every (c) site refuses a generic op on
the same class of node, with one difference in how the class is recognized
(§7). The (b) sites are TOML's alone, and small. The (a′) sites are TOML's
alone, and are not about regions.

## 3. `Document.node_regions`

```zig
pub const NodeRegion = struct { node_id: AST.Node.Id, start: usize, end: usize };
node_regions: []const NodeRegion = &.{},
pub fn regionsOf(self: Document, id: AST.Node.Id) []const NodeRegion
pub fn isSection(self: Document, node: AST.Node) bool
```

One entry per **header line** of every **section node** — a container whose
span does not describe its physical extent — in source order: the line that
created it, then every line that re-opened it. Each entry is a whole line,
`[start, end)` with `end` past the newline. Sorted by `(node_id, start)`, so
`regionsOf` is a binary search and a document with no sections carries an empty
slice. `isSection` is presence in the table, and it is the predicate the rest
of this proposal hangs off.

What the parsers record:

- **fig** (`parser.zig`, `recordRegions`): every block (non-flow, non-root)
  container, at AST assembly — its creating line from the node's own span
  start, then the `PendingContainer.reentries` that `reentry_headers` used to
  carry. The `> *` element mapping of a block sequence is one; a flow
  `{…}`/`[…]` value is not.
- **TOML** (`parser.zig`, `recordHeader`): every table `createTable` makes —
  explicit, implicit, *and dotted* — on its creating line; the `[a]` line that
  promotes an implicit table; each `[[a]]` line on both the array and the new
  element (elements share the array's span, which is why the old code scanned
  for the header); and each later dotted line that extends an existing dotted
  table (`navigateDottedPath`'s existing-child branch). Inline tables and
  static arrays are never recorded.
- **INI** (`parser.zig`, `recordHeader`): every `[section]` line, creating or
  reopening.

This deviates from the brief's phrasing — "a scattered container lists every
physical region of its subtree" — on purpose. Entries' lines are their spans,
so recording them would be a second copy of `node_spans`; and which comment
lines ride with a line is the editor's policy (`commentBlockStart`, per
`Syntax.comments.style`), not the parser's. The table records the one thing
spans cannot carry, and the rest is **derived** — hence the name.

## 4. The derived gather — `src/editor/regions.zig`

`languages/shared/sections.zig` moved here (with `git mv`, so history follows)
and gained the gather that used to be three:

```zig
pub fn gather(parsed, source, allocator, node, style, out) !void {
    for (parsed.regionsOf(node.id)) |h| out.append(headerLineRegion(source, h, style));
    for each child:
        if (parsed.isSection(child_value)) gather(child_value)
        else out.append(entryLineRegion(source, span(child), style));
}
```

That is TOML's `gatherTableRegions`, fig's `gatherContainerRegions` and INI's
`gatherSection` with the classification replaced by `isSection`. The three
were already this shape; only the predicate differed, and the predicate is now
a recorded fact. `gatherNormalized` and `extentEnd` (the replacement for
`subtreeMaxEnd`) sit beside it. The comment style comes from
`Syntax.comments.style`, so INI's `;` blocks and fig's `#` blocks ride along as
before.

One consequence worth stating: the table-driven gather recurses into a dotted
table where the `[`-sniffing one took its first line only. `[a]` /
`x.y = 1` / `x.z = 2` deleted as `a` used to leave `x.z = 2` behind to become a
root key. It is now a test.

## 5. What became generic

In `src/editor.zig`'s "Whole-container structural editing" block, live for
every format whose `Syntax.section_noun` is non-null (§7):

- `deleteContainer`, `moveContainer`, `reorderContainers` — `sectionAt` (path
  resolution plus the "not a section" refusal), `gatherRegions`, then
  `regions.spliceOut`/`relocate`/`reorderBundles`. No format code.
- The four line-splice refusals, as one rule (§7).
- `gatherRegions` and `sectionExtentEnd`, `pub` for the hooks that still need
  the region set: TOML's rename addresses each header region's start, and its
  two inserts place a header past a parent's extent.

`hasContainerOp` answers `is_section_format` for the three generic names and
`@hasDecl` for the three hooks, so the C ABI's `unsupported_format` answers
are unchanged: TOML six, fig and INI three, everyone else none.

Net in `src/languages/*/editor_helper.zig`: TOML 1681 → 1284, fig 1145 → 904,
INI 479 → 279 — **3305 → 2467 lines**, including the tests each keeps and the
new ones in §8. The engine grew by 191 lines and `regions.zig` by 73 over
`sections.zig`.

## 6. What stayed hooked, and why

`insertContainer`, `renameContainer`, `appendContainerToSeq` — TOML's three.
`Decls.exclusive` is now exactly this list, with a new `validate` rule that a
format declaring one must be a section format (and a `validate-check` case
for it).

- `insertContainer` and `appendContainerToSeq` are (a)+(b). The (a) half is
  generic now (`sectionExtentEnd`), and each is a dozen lines of TOML: the
  existence check, a blank line, `[` or `[[`, `appendTomlHeaderPath`, `]`.
  Making them generic means a hook that *spells a header line for a path* —
  which is the fragment-printer contract this proposal is not designing. What
  it would have to answer, for the record: a header for a mapping path versus
  an array element; whether the format wants a blank line before it; whether
  the body is verbatim entry lines or needs re-indenting (fig's `>` prefixes
  say yes). That is a printer, and it belongs with 3.0's one-file-per-format
  work.
- `renameContainer` is (a′)+(b). The region set feeds it (it is the one hook
  that calls `gatherRegions`, with `merge_touching = false`), but its work is
  finding every place a *name* is spelled — segment *n* of each `[a.b.c]`
  header, and of each dotted `a.b = 1` line, whose index is relative to the
  enclosing header and so cannot come from the AST path. A name-occurrence
  table would be a second, different table; `renameTableSegments` reads it off
  the source instead, as before.

TOML's `tomlReplaceKey` hook is unchanged in role; its block-table test is now
`isSection` rather than `!isFlow`.

## 7. The one guard rule, and what it is not

The brief suggested the guards might all be "a node with more than one region
cannot be line-spliced". The catalogue says they are close, but not that:

- TOML and INI refuse every header node, including one with a single
  coalesced region (`[a]` followed directly by its entries) and an empty one.
  Their reason is the *span*, not the region count: a header node's span is
  its key segment, so the line op would take the header line and leave the
  body, and the value op would overwrite the name.
- fig's one guard refuses every block container, contiguous or not, though
  its widened spans make the line ops correct for a contiguous one. It is a
  defensive refusal — "may be re-entered" — and it is test-pinned.
- TOML's move guard also refuses a *destination* on a header line, because
  "before `[b]`" is the tail of the preceding table's body. That is a fact
  about open-ended header scope the engine cannot see in any region set.

The rule that is actually common to all of them is the one the table states
directly: **a section node cannot be line-spliced; use the container op.**
`deleteKey`, `moveKey` (either end) and `reorderKeys` (a moved entry) refuse
when the entry's *value* is a section node; `replaceValAtPath` refuses a
non-root section node before the engine's own splice. A format that hooks
`replaceValAtPath` has taken that splice over and owns its targets — fig
re-frames a block container's value in place, and keeps doing so — so the
rule guards the engine's splice only. The words in the error come from
`Syntax.section_noun` (`.table`, `.section`, `.container`), a declared value
rather than a hook, because only the noun differs: `CannotDeleteTable` /
`CannotDeleteSection` / `CannotDeleteContainer`, and likewise for replace,
move, reorder and "not a section" (`NotATable` / `NotAContainer`). CLI output
and C ABI status codes are therefore what they were.

A more precise rule was considered and not built: refuse only when the node's
derived regions differ from the lines the op would splice. It handles
scatteredness exactly, but it lets an empty `[a]` be moved (adopting whatever
follows it, per the dest hazard above) and it flips fig's delete refusal.
Encoding open-ended header scope as another `Syntax` value would fix the
first; nothing short of a per-format veto fixes the second. The section rule
is simpler, conservative in the same direction the old guards were, and
matches every test they pinned.

## 8. Behavioural changes

All library-level (`Editor(…)` and the C ABI's status code is unchanged), all
in the direction of refusing an op that used to succeed partially:

- **TOML**: a dotted table (`a.b = 1`) is a section node. `deleteKey`,
  `moveKey` and `reorderKeys` on its entry now refuse (`CannotDeleteTable`
  etc.); they used to line-splice, correctly for a one-line table and losing
  the other lines otherwise. `deleteContainer(a)` does what they did.
  `moveContainer` accepts a dotted table as the destination (it refused,
  `NotATable`).
- **fig**: `moveKey` and `reorderKeys` on a block-container entry now refuse
  (`CannotMoveContainer`, `CannotReorderContainers` — two new error names);
  they used to move the widened span, which for a re-entered container is
  the first fragment alone. `moveContainer` with a scalar destination now
  refuses (`NotAContainer`) rather than landing before the scalar's line.
- **INI**: none observed; the tests are unchanged.

Each is a test in the relevant `editor_helper.zig`, and each is a
`Behavioural-change:` trailer on the commit that makes it.

## 9. Verification

`zig build check` (test + conformance + abi-check + semver-check +
version-floor + validate-check + check-figl + vendor-check) green;
`zig build test -Dtoml=false`, `-Dfig=false`, `-Dini=false` green. `fig.h`
unchanged. Test counts: 1240 → 1247 (three parser tests reshaped around the
new table, two `Document` tests, seven editor tests for §4 and §8).

## 10. What this leaves for 3.0

- The fragment-printer question in §6 is now isolated to three TOML
  functions and a printer call each would make.
- `Syntax.section_noun` is the third value on the manifest that a format
  declares about editing rather than syntax (`kv_sep`, `empty_map_literal`).
  If pluggable formats grow a separate editing manifest, it goes there.
- `validate` cannot check that a section format's parser fills the table. A
  parser-side conformance test — parse each format's testdata and assert that
  every `isSection` node has its header line in the table — would close that,
  and is cheap once the per-format test harness exists.
