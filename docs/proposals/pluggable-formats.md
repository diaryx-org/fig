```fig
title = Pluggable formats
description = What fig 3.0 is for — the format set written down once, every per-format fact declared by the format, and the editing contract a format writes against named in one place
created = 2026-09-04
status = implemented
updated = 2026-09-07
part_of = [proposals](proposals.md)
```

# Pluggable formats

> **Status: IMPLEMENTED.** The argument for core 3.0, written against `main`
> at c9761cd (cli 4.0.0, core 2.7.0 plus the unreleased derived-regions
> work). §5.1, §5.2, §5.3, §5.4, §5.5 and §5.7 landed on `main` on
> 2026-09-04 as six commits (8b34254, 2ef10aa, 6a1f6b3, 449e900, 2226998,
> 47bcaac), each verified as §7 says, and shipped in core 2.8.0. §5.6 — the
> major itself — landed on `main` on 2026-09-07 and is core 3.0.0, ABI 2,
> unreleased at the time of writing; §10 records it. §9 records what the 2.x
> steps built, where they depart from §5 as argued, and the review that
> forced the departures; read it before acting on §5.1, §5.4 or §5.7, each
> of which asked for something the pinned Zig cannot do or the tree does not
> have.
>
> "fig 3.0" in the body means **core 3.0** (`.version` in `build.zig.zon`,
> with `abi_version` 2). The crates.io `fig` crate is already at 3.3.0 and
> the CLI at 4.0.0; each moves on its own track (see VERSIONING).

## 1. The claim

Adding a format to fig should mean writing one directory and adding one line
to one list. Today it means writing one directory and editing eleven other
places — five in core, three in the build, three in the bindings — each of
which restates a fact the format's own `<lang>.zig` already declares, or a
fact that nothing declares because it only ever lived in a `switch`.

Two preliminary refactors have already moved most of the *content* of the
format contract out of the engine and onto the format: the
[Language interface](language-interface.md) put the syntax and capability
facts on a manifest that `validate` checks, and [derived regions](derived-regions.md)
made the whole-container ops generic over a table the parser fills. What is
left is the *membership* — where the set of formats is written — and the
handful of facts that still live in core because no manifest field exists for
them yet. That is what this proposal is about, and it is smaller than the
phrase "pluggable formats" suggests. It is also the last thing to settle before
the 3.0 breaks that have been waiting in
`BREAKING-CHANGES.md` (deleted in core 3.0) since 2.4, because both of those
change the C ABI's format enum, and that enum should be derived from the one
list before it is changed.

## 2. Where 2.x stands

A format is a directory `src/languages/<lang>/` whose entry file declares a
`Language` struct. `language.zig`'s `validate` enforces a closed contract:

- **Required of every format** — `Type`, `Parser`, `Printer`, `default_type`,
  `parse`, `print`, `name`, `extensions`, `caps`, `dialects`.
- **Required of an editable format** — `syntax(t: Type) Syntax`.
- **Optional** — `printNode`; `materialize` and `TagMode` (YAML alone).
- **Sixteen editing hooks**, each overriding the `Editor` method of its name,
  dispatched by `@hasDecl`.
- **Three whole-container hooks** a section format may supply
  (`insertContainer`, `renameContainer`, `appendContainerToSeq` — TOML's
  three), the rest being generic over `Document.node_regions`.

`Language.dialects` is each format's own rows of the format registry:
thirteen dialects over eleven languages, each carrying a frozen C ABI value,
splice style, embed spellings, `--spec` strings, and the printer entry
points. The six enums that used to restate this by hand — `cli.Format`,
`AST.SerializeFormat`, `Language.Detected`, `deserialize.Format`,
`Embed.InnerFormat`, `c_api.FigFormat` — are reified from it.

`Document` is format-neutral: source, AST, spans, the anchor and tag span
tables YAML fills, and `node_regions`. `Editor(Language)` calls `validate`
and reads `Language.syntax` and `caps` rather than asking which language it
is. `tools/validate-check.zig` proves the contract is checkable from outside:
its fixture is a `Language` declared in a generated file that never touches
`src/languages/`, and `validate` accepts it.

So the contract exists, is declared, and is enforced. What follows is the
list of places that do not yet read it.

## 3. What still names the format set

Every place below is a hand-maintained enumeration of the formats, or a
per-format fact stated in core rather than on the format. Each one is a
place a new format has to be added, and a place two formats can disagree.

**In `src/languages/language.zig`:**

1. `slots` — eleven `@import`s, each paired by hand with its
   `build_options.lang_*` flag.
2. Eleven `pub const JSON = gated(slots[0])` aliases, indexed by position.
3. `registry_order` — the eleven names as a literal, pinning the order of
   `slots`.
4. `detect` — eleven hand-ordered probe branches. The order is a real
   argument about grammar overlap (JSON before ZON before plist before XML
   before TOML before fig before INI before dotenv before YAML before
   `.properties` before NestedText) and is the one thing here that is
   genuinely a fact *between* formats rather than about one.
5. `compiled` — eleven `build_options` gates.

**In the build:** `src/build/Options.zig` declares twelve `lang_*` flags and
six `*_conformance` flags, each written three times — as a struct field, as
a `b.option` call with its help text and default, and as an `addOption`
call — plus once more in `all_on`. `root.zig`'s test block imports the
eleven language modules by name for test discovery.

**Per-format facts still in core:**

6. `flat_strip.zig` — its own `Format { ini, dotenv, properties }` enum and a
   `maxMappingDepth` switch (INI 1, the other two 0), which
   `diagnostics.zig`'s `valueLoss` arms must match by hand; the module doc
   says so in capitals.
7. `deserialize.zig` — imports four parsers by name and switches on its
   `Format` to call each one's `parseAbstract`, gated on four build flags.
8. `embed.zig`'s `parseYamlSlice` — the multi-document splitter parses each
   segment as YAML by naming `Language.YAML`. (It is a YAML feature, but it
   is spelled as a build gate in a generic module.)
9. `cli/actions.zig` and `cli/reformat.zig` — `build_options.lang_yaml`
   gates around `Language.YAML.materialize`, and a `lang_json` gate around
   `Language.JSON.caps.lossless`. Both facts are already optional decls a
   registry entry could answer.
10. `docs/zig.md`'s format table, its list of `build_options.lang_*` flags,
    and its prose copy of the detection order.

**In the bindings:**

11. `bindings/rust/fig/src/lib.rs` — a hand-written `Format` enum with seven
    members (it has never gained INI, dotenv, `.properties`, plist or
    NestedText) and a `From` impl mapping each onto `ffi::FigFormat`.
12. `bindings/typescript/src/types.ts` — a hand-written `Format` enum with
    the ABI values as literals.
13. `bindings/c/include/fig.h` — `FIG_FORMAT_*` enumerators, checked against
    the registry by `abi-check` in both directions. This one is already
    guarded; it is listed because the guard is the model for the other two.

Not on the list, deliberately: `cli/gron.zig` (gron *is* a JSON feature),
`lsp/main.zig` (the language server *is* fig's), and the per-format
`conformance.zig` files (a corpus is a property of a format). A format
naming itself is not the problem; core naming a format is.

## 4. What "pluggable" means here

Three readings were on the table. This proposal picks the first, gets the
second as a consequence, and rejects the third.

**One list, in tree.** A format is added by writing `src/languages/<lang>/`
and adding one entry to one list. Everything in §3 derives from that entry
or from the format's own declarations. This is the deliverable.

**Out-of-tree `Language` for the Zig library.** A Zig consumer can already
hand `Editor(L)` any `L` that passes `validate` — nothing in `editor.zig`
reaches the registry. §5.4 makes that a stated contract by naming the engine
surface a hook may use. What a consumer's `L` does *not* get is registry
membership: no `SerializeFormat` member, no CLI selector, no C ABI value, no
embed spelling. Those enums are global types the AST, the embed layer and the
C ABI are written against, and making them parametric over a consumer's
list would multiply every generic in the crate for a use nobody has asked
for. The boundary is: the *engine* is generic over any `Language`; the
*registry* is fig's.

**One file per format.** Rejected as a literal goal. YAML's parser is 3,759
lines and its tokenizer 1,808; TOML's editor helper alone is 1,324. Folding
those into one file would make the contract harder to see, not easier. The
unit that matters is the entry file, `<lang>.zig`, which already *is* the
whole contract for a format — the rest of the directory is implementation
the entry file points at. "One file" is what a reader opens; "one directory"
is what a format occupies. Both are true today. What is not true today is
"one line", and that is §5.1.

**Runtime plugins** — a format loaded through `dlopen` or supplied across
the C ABI — are out of scope. The C ABI's format enum is closed by design
(`abi_value` is frozen per dialect), the editor is a comptime generic, and
nothing downstream has asked.

## 5. The design

Each step is independently landable on `main`, in the staging discipline the
last two proposals used: add the derived thing beside the hand-written one
with a comptime assert that they agree, move consumers over, delete the
hand-written one.

### 5.1 One list

A new leaf, `src/languages/list.zig`, holding one row per format:

```zig
pub const Row = struct {
    /// Directory and entry file: `src/languages/<dir>/<dir>.zig`.
    dir: [:0]const u8,
    /// The `-D<flag>` build option and `build_options.lang_<flag>` decl.
    flag: [:0]const u8,
    help: []const u8,
    default_on: bool,
    /// Which `*_conformance` option gates this format's suite, if it has one.
    conformance: ?[:0]const u8 = null,
};

pub const rows = [_]Row{
    .{ .dir = "json", .flag = "json", .help = "Include JSON/JSONC/JSON5 support", .default_on = true, .conformance = "json" },
    // …
};
```

It is a leaf because both sides of the build read it: `src/build/Options.zig`
is already imported by `build.zig`, so it can iterate `rows` to declare each
`b.option` and `addOption` — the struct-field, option-call, `addOption`,
`all_on` quadruple collapses to one loop — and `language.zig` iterates the
same `rows` to build `slots` with `@import(row.dir ++ "/" ++ row.dir ++ ".zig")`
gated on `@field(build_options, "lang_" ++ row.flag)`. `root.zig`'s test
block iterates it too. The eleven positional aliases (`Language.JSON = gated(slots[0])`)
become one `Language.of("json")` — or stay, generated from the rows, if the
names are worth keeping for readers; §8 asks.

`registry_order` retires. It existed to pin the order `slots` were assembled
in, and the only consumer that cared was the ABI, which is now keyed by
`abi_value` and pinned by `c_api.zig`'s literal thirteen-pair assert and
`abi-check`. A test that the assembled registry's `abi_value`s are unique and
that no built-in row is missing from `rows` replaces it.

`compiled` and `dialects` derive from `rows` as they derive from `slots`
today. The `canonical` flag is not a format — it gates the AST's own oracle
encoding — and stays declared by hand in `Options.zig`, which then holds
exactly the flags that are *not* a format.

### 5.2 Declared detection order

`detect` stays a loop in one place, but the order comes from the formats.
Each `Dialect` row gains `sniff_rank: ?u8` — null for a dialect that is not
detectable, otherwise its position in the probe order — and `validate`
requires the ranks across the registry to be unique. `detect` iterates
`dialects` sorted by rank.

The counter-argument, which `language.zig` makes in its own doc comment, is
that the order is *one* argument about grammar overlap and belongs in one
place. It is a fair point and the reason this is a separate step: the
argument is relational (INI must follow TOML *and* fig; NestedText must
follow YAML) and a rank on each row spreads it across eleven files. The
answer is that the rank is the *fact* and the reasoning is a comment on the
row that carries it — each of the eleven branches already has its own
paragraph explaining why it sits where it does, and those paragraphs move
with the rank. A test pins the resulting order to today's, so a new format
choosing a rank cannot silently reorder the existing ones; the test changes
only when someone means it to.

### 5.3 The facts still in core move onto the manifest

Each of §3's items 6–9 becomes a declaration:

- **Flat depth.** `Caps` gains `max_mapping_depth: ?u8 = null` — null for a
  format with no depth limit, 1 for INI, 0 for dotenv and `.properties`.
  `flat_strip.zig`'s private `Format` enum and `maxMappingDepth` go;
  `diagnostics.zig`'s three hand-matched arms read the same field, and the
  "MUST stay in sync" comment has nothing left to say.
- **Deserialization.** `parseAbstract` joins `Decls.optional`; a dialect with
  `deserializable = true` must belong to a language that declares it
  (`validate` rule, `validate-check` case), and `deserialize.zig` dispatches
  `inline else` through `entryFor` as the CLI already does. Its four named
  parser imports go.
- **YAML's multi-document split.** `Embed`'s `parseYamlSlice` becomes a call
  through the `frontmatter` dialect's `Lang.parse`, which is what it already
  is with the name removed. Whether the *stream* splitter itself (`---`
  markers inside an embed) is a YAML fact or an embed fact is §8's second
  question.
- **CLI gates.** `materialize` is already an optional decl; the two CLI sites
  test `@hasDecl(entry.Lang, "materialize")` on the dialect in hand instead
  of `build_options.lang_yaml`. The `lang_json` gate around
  `JSON.caps.lossless` reads `entryFor("json")` — or, if what it wants is
  "the lossless target for this output format", `Lossless.nativeFor`, which
  exists for exactly that.

### 5.4 The editing contract is named

The hooks reach into the engine, but the surface they reach is small and
already stable. Grepping the eight `editor_helper.zig` files, a hook uses
seven members of `Editor` — `allocator`, `source`, `replaceAtSpan`,
`getParsed`, `sectionExtentEnd`, `gatherRegions`, `writeMapValue` — and ten
module-level helpers — `lineStartBefore`, `lineEndAfter`, `firstNonSpace`,
`columnOf`, `isFlow`, `commentBlockStart`, `appendBlockSep`, `tileBlocks`,
`fullOrder`, `Block`. Nothing else.

That set becomes `src/editor/splice.zig`, documented as *the API a hook is
written against*, and every other `pub fn` on `Editor` that exists only for
the engine's own use loses its `pub`. The hook signatures do not change; what
changes is that a format author can read one file and know what they may
call, and a change to anything outside it cannot break a format. This is the
"format contract made of values plus a small engine surface" the last two
proposals were reaching for, stated as a file boundary rather than a new
abstraction.

**The three fragment hooks stay hooks.** Derived regions §6 isolated
`insertContainer`, `renameContainer` and `appendContainerToSeq` to TOML and
named what a generic replacement would need: a printer that spells a header
line for a path, and a name-occurrence table for renames. That is a
fragment-printer contract, and three functions in one format do not justify
designing it. If a second section format needs `insertContainer` — fig
does not, its containers are written by `insertKey` — that is the time.
Recording this as a decision rather than an omission is the point of the
paragraph.

### 5.5 Out-of-tree `Language` is a stated contract

With §5.4, the Zig guide can say: *a type that passes `Language.validate` can
be given to `Editor`; its hooks may use `editor.splice`; it will not appear
in any registry-derived enum.* `validate-check` already generates a `Language`
outside the tree and checks it; one more case instantiates `Editor` over the
well-formed fixture and runs a `set`, so the claim is proven rather than
described.

### 5.6 What the major breaks

3.0 is a core and C ABI major; the CLI, Rust and npm artifacts move as their
own contracts require. The breaks, all of which have been waiting on a
major:

- **`FigEmbedType` becomes parametric** — the entry already written in
  `BREAKING-CHANGES.md` (deleted in core 3.0). Its migration table is
  generated from `dialects` and `Embed.Type` rather than written by hand,
  which is only possible once §5.1 makes both derivations total.
- **Generic XML is removed as a selectable format** — the other entry. Its
  `FigFormat.xml = 6` slot is retired, never reused. The `xml/` directory
  keeps `tokenizer.zig` as the substrate plist sits on.
- **`abi_version` goes from 1 to 2**, and `semver-check` will demand it.
- **`root.Native`**, the deprecated alias for `Canonical`, goes. Its comment
  says it is kept for the Diaryx git dependency; `dx deps` shows diaryx
  consumes fig through crates.io, so nothing reaches it.
- **The Rust `Format` enum gains the six missing dialects.** The enum is
  `#[non_exhaustive]`, so this is additive, but the major is the moment to
  generate it — and the TypeScript enum — from the registry the way
  `fig.h`'s enumerators already are, with a check each way. That closes §3's
  items 11 and 12.
- **Zig API renames from §5.1**, if the positional `Language.JSON` aliases
  go (§8).

Everything else in §5 is additive and ships in a 2.x minor before the major
is cut; a change that turns out to break the Zig API gets a
`Behavioural-change:` trailer and waits with this list.

### 5.7 A per-format harness

Derived regions §10 asked for a parser-side conformance test — every node
`isSection` reports must have its header line in `node_regions` — and noted
it is cheap once a per-format harness exists. There is none: each language
that has a corpus wires it up in its own `conformance.zig`.

`src/languages/harness.zig` runs, for every entry in `compiled`, the checks
that hold for *any* format: parse each file in `testdata/<lang>/`; print and
reparse to the same AST; the regions invariant above; `Editor` construction
and a no-op splice for an editable format; `validate`. A format opts in by
having a testdata directory, which every format with a corpus already does.
This is where the §5.2 order-pinning test and the §5.1 registry-completeness
test live too.

## 6. Sequencing and versions

Steps 5.1, 5.2, 5.3, 5.4, 5.5 and 5.7 are refactors with no ABI change and
no intended API change. They land on `main` one at a time, each verified as
§7 says, and ship in core 2.8 (or whatever the next minor is). That is
deliberate: the derived-regions work is already unreleased, and stacking the
whole major on top of it would make the eventual diff unreviewable.

3.0 is then §5.6 alone, cut when the last of the above has shipped and
settled. The order inside 5.6: XML removal first (it shrinks the enum the
embed change then reshapes), the embed ABI second, the alias and the binding
enums with either. The changelog's 3.0 intro is written from this section.

A `docs/tasks/` directory is opened when the first step is deferred rather
than started, per the org's rule; until then this document is the plan.

## 7. Verification, per step

What the last two proposals did, and what each step here repeats:

- `zig build check` green — tests, conformance scoreboards, `abi-check`,
  `semver-check` (expected verdict: patch or minor until §5.6, then major),
  `validate-check`, `version-floor`, `check-figl`, cargo-semver-checks, and
  the Rust and TypeScript suites.
- At least four gating configurations built, including the everything-on
  build `zig build conformance` forces and a build with only one language.
- `fig.h` unchanged until §5.6 (`abi-check`'s header diff proves it).
- A CLI byte-diff against the parent commit's binary over the existing
  corpus (derived regions used 689 cases), since every step touches dispatch
  that the CLI exercises and byte-identical output is the standard.
- For §5.1 specifically: the `dialects` tuple, `compiled`, and every reified
  enum compared member-for-member against the hand-written versions in a
  comptime assert *before* the hand-written versions are deleted.

## 8. Open questions

1. **Keep the named aliases?** `Language.JSON`, `Language.TOML` and the rest
   are how every guide, test and consumer reaches a format. Generating them
   from `rows` keeps the names at the cost of a comptime loop that declares
   eleven consts; `Language.of("toml")` drops the loop at the cost of every
   call site. The guides favour keeping them. Decide before §5.1.
2. **Is the YAML multi-document stream an embed fact or a YAML fact?** The
   `---` splitter in `embed.zig` is written generically but only YAML has
   streams. If it is YAML's, it becomes a `Language` decl (`splitStream`,
   optional) and embed dispatches on `@hasDecl`; if it is embed's, only the
   parse call changes (§5.3). Decide when §5.3 reaches it.
3. **Does the TypeScript binding want a generated `Format`?** It has no API
   guard today (VERSIONING's known gaps), so a generated enum would be the
   first check on that surface. Cheap if the generator is the one `fig.h`
   already has; otherwise it can wait.
4. **What does the fig CLI's major look like?** Removing `-i xml`/`-o xml`
   is a CLI break too. Whether the CLI cuts 5.0 alongside core 3.0 or
   deprecates the selectors first is a CLI question this proposal does not
   answer.

## 9. Outcome (2026-09-04)

Six of the seven steps in §5 are on `main`. Each commit's message carries
its own verification; this section records where the built thing differs
from the argued one, and answers three of §8's four questions. The body
above is left as written.

### 9.1 The review

A review against the code at c9761cd found, before anything was built:

- **§5.1's mechanism does not compile.** Zig 0.16 rejects any `@import`
  whose operand is not a string literal, so `language.zig` cannot iterate
  `rows` and import `row.dir ++ "/" ++ row.dir ++ ".zig"`; a build-time
  generated file cannot rescue it either, since a generated module lands in
  the cache directory and its relative imports cannot reach
  `src/languages/`. The same limitation answers §8.1: Zig cannot declare a
  named constant from a loop, so `Language.JSON` cannot be generated.
- **§5.7 assumed a corpus shape that does not exist.** `testdata/json/` is
  `accept/`, `edgecase/`, `reject/`; `testdata/toml/` is `valid/`,
  `invalid/`; `testdata/yaml/` has `accept/`, `reject/`, `reject-stream/` and
  a skiplist; `testdata/nestedtext/` is one `tests.json`; fig's corpus is one
  file under `src/languages/fig/testdata/`. "Parse each file in
  `testdata/<lang>/`" fails on every reject case.
- **§3 undercounts.** `diagnostics.zig`'s `valueLoss` has an arm for every
  format, and the flat three encode "no typed scalars" and "no sequences"
  as well as depth; `commentsEmitted`, `blockComments`, `degradedNote`,
  `cli/types.zig`'s `toSerializeFormat` and `cli/parse_dispatch.zig`'s
  `mapDetected` are exhaustive switches a new format must edit.
- **§5.4 miscounts the hook surface.** INI's `insertKey` hook calls
  `self.insertBlockKey`, which is not among the seven; and the `Editor`
  members are methods on the generic type, so they cannot move to a file.
- §5.1's `conformance` field is one string per row, but JSON has two suites;
  §5.2's rank uniqueness is a registry check, not a `validate` one; §5.3's
  YAML gate also reaches `TagMode`; `root.Native` is confirmed dead across
  the org; §1's "eleven places" does not match §3's thirteen items.

### 9.2 What was built

**§5.1** (8b34254). `src/languages/list.zig` holds one `Row` per format —
`name`, `help`, `default_on`; `flag` and `dir` collapsed into `name`, since
they were the same string for all eleven — and a separate `suites` list for
the conformance flags, one per suite rather than one per row. `Options.zig`
loops both; `BuildOptions` is `langs: [rows.len]bool`, `suites`, and a
hand-declared `lang_canonical`, with `cfg.lang("fig")` for the build graph.
`language.zig` keeps one `@import` slot per row, named, and a comptime block
that refuses to build unless `slots` and `rows` agree in order and each
module's `Language.name` matches its slot. So a format is one row **and one
slot**, not one line; the slot is what only a string literal can spell.
`Language.of("json")` is the lookup by name; the named aliases stay,
written as `pub const JSON = of("json")`. `registry_order` is gone: the
row order is the language order and the slot check is the pin.
`root.zig` names no language — `language.zig`'s own test block references
every slot, which discovers each module's tests — and `validate-check`
writes its all-off `build_options` stub from the list. Test count 1251 →
1252, the one being that block.

**§5.2** (2ef10aa). `Dialect.sniff_rank: ?u8` replaces `detectable`; every
row declares its rank with the reasoning paragraph moved beside it (jsonc
declares null). `Language.sniff_order` is the sorted result and `detect` is
one loop over it. The registry refuses a duplicate rank and a language with
no ranked dialect — the guard that makes a default of null safe. A test pins
the sorted order to the sequence the hand-written function spelled.
`Detected` keeps registry-order members.

**§5.3** (6a1f6b3). `Caps.max_mapping_depth: ?u8` (INI 1, dotenv and
`.properties` 0); `flat_strip.zig` takes the depth, `flatStripDepth` reads it
off the registry, and `valueLoss`'s three flat arms are one `inline` arm
reading the same field. `parseAbstract` joined `Decls.optional`, `validate`
requires it of a language with a `deserializable` row (validate-check case
20), and `deserialize.zig` dispatches through `entryFor`. `embed.zig`'s
splitter parses each segment through `entryFor("yaml")`. The CLI's two
`materialize` sites became one `materializeFor` that dispatches on
`@hasDecl(Lang, "materialize")` for the source format, as the C ABI's
`prepareDocumentAst` does; the `lang_json` gate became
`Lossless.nativeFor(.json)`. **What stayed, by decision:** the typed-scalar
and no-sequence halves of the flat arm, `commentsEmitted`, `blockComments`,
`degradedNote`, `toSerializeFormat` and `mapDetected`. Each is an exhaustive
switch, so a new format cannot be added without the compiler naming the
site; moving them onto `Caps` would need three more fields for one reader
each, and none of them is a gate. They are listed here so that §3 is
complete, not so that they are done.

**§5.4** (449e900). `src/editor/splice.zig` holds the ten free functions
(`lineStartBefore`, `lineEndAfter`, `firstNonSpace`, `columnOf`, `isFlow`,
`commentBlockStart`, `appendBlockSep`, `Block`, `tileBlocks`, `fullOrder`)
and re-exports `CommentStyle`; its module doc names the `Editor` members a
hook may call — `allocator`, `source`, `replaceAtSpan`, `getParsed`,
`sectionExtentEnd`, `gatherRegions`, `writeMapValue`, **and
`insertBlockKey`**. Nothing on `Editor` lost its `pub`: every hook-only
method is reached from another file, and the rest is the public editing API.
`flowOpenEnd`, engine-only, did. The surface is a documented file boundary,
not an enforced one; a hook still receives `*Editor` and Zig has no way to
restrict what it calls short of a wrapper type, which would change hook
signatures.

**§5.5** (2226998). `docs/zig.md` states the out-of-tree contract, and
`validate-check` proves it with a case that is *run* rather than compiled: a
`Language` declared in the tool's work directory — its own name, extension,
caps, dialect row and syntax, borrowing dotenv's parser and printer through
`Language.moduleFor` — is compiled with `zig test`, drives `set` and
`deleteKey` through `Editor`, and asserts `SerializeFormat` gained no
member. The case fails when its expectation is wrong (checked by hand).

**§5.7** (47bcaac). `src/languages/harness.zig` runs over
`Language.dialects`: every `empty_doc_seed` parses and round-trips; every
`sample` a format declares parses, prints, and reparses to the same tree;
`Document.node_regions` is whole-line, container-only and sorted, and a
section format's parser fills it; `Editor` constructs over every sample of
an editable format with a no-op splice. A format opts in by declaring
`samples` (an optional decl); the corpora are left to the suites that know
their shape. Two findings on the way: `AST.eql` is positional over node
ids, so the round trip is compared through the canonical encoding (TOML
prints a short `[table]` inline and the reparse numbers it differently);
and the registry invariants stayed in `language.zig` beside the tables.

### 9.3 §8, answered

1. **Aliases kept**, as `of(name)` lookups; generating them is impossible
   (§9.1), and `of` serves a caller with the name as a string.
2. **The YAML stream is embed's.** Only the parse call changed; the `---`
   splitter stays in `embed.zig` and names the `yaml` entry once, through
   the registry rather than a build flag. Making it a `Language` decl would
   add a member to the closed set for one format and one caller.
3. Open. Nothing here touched the TypeScript binding.
4. Open. The CLI's major is still a CLI question.

### 9.4 What 3.0 still is

§5.6 as written. Nothing in §9.2 changed the C ABI (`abi-check` and
`semver-check` were green at every step, verdict `patch`), and the two
Zig-visible changes — `Dialect.detectable` → `sniff_rank`, and
`FlatStrip.lossyStrip` taking a depth in place of `FlatStrip.Format` — are
recorded as `Behavioural-change:` trailers on their commits for the core
release that carries them.

Two defects found while verifying, neither introduced here, are filed in
[tasks](/docs/tasks/tasks.md): the YAML printer panics on thirty of the
accept-corpus documents, and the everything-on `zig build test` fails to
compile in `patch.zig`.

## 10. Outcome: the major (2026-09-07)

§5.6 as written, in §6's order, on `main` as four commits plus a version
bump: XML removal (5a25681), the embed ABI (ecb689f), the `Native` alias
(20c7767), the binding enums and their check, then `core 3.0.0 · rust 3.5.0
· npm 3.0.0`. Where it departs from §5.6:

- **The embed ABI's container enum has seven members, not six.** §5.6 quoted
  BREAKING-CHANGES' list, written before `html_code` existed; the enum
  mirrors `Embed.Type`'s tags as they are. A preset ignores the format
  argument, as promised, and `fig_embed_detect` *reports* the format a preset
  pins, so the pair a caller reads back is meaningful without a table. The
  migration table BREAKING-CHANGES said a generator would write is instead
  five lines of comment on the enum in fig.h — one per group, since every
  member of a group maps the same way — and no shim table shipped on 2.9:
  the promise was withdrawn rather than kept, on the grounds that a caller
  recompiling against ABI 2 has the mapping in the header it is compiling
  against.
- **The bindings keep their flat `EmbedType`.** BREAKING-CHANGES offered
  either shape; one name per archetype is what a caller wants to write, so
  the Rust wrapper and the TypeScript binding each meet the pair in one
  function (`EmbedType::parts`, `embedParts`) and their public APIs do not
  change. Neither binding cuts a major for this.
- **Value 6 is retired by a table, not a comment.** `c_api.zig`'s literal pin
  gained a `retired` list beside `pinned`; the build refuses the name coming
  back or the value being handed to another dialect.
- **The Rust `Format` enum grew, and the check came with it.** §8.3 asked
  whether the TypeScript binding wanted a generated enum. Neither binding's
  enum is generated — a hand-written enum with doc comments per member reads
  better than a generated one — but `abi-check` now diffs `fig-sys`'s
  `FigFormat`, the TypeScript `Format` and the Rust wrapper's `Format`
  against the registry, name and value, the way it diffs fig.h. That closes
  §3's items 11 and 12 and answers §8.3: no generator, one check. The Rust
  crate's five new variants and features are a minor (3.5.0); its `xml`
  feature stays as a documented no-op so a dependent's feature list resolves,
  and leaves at the next Rust major.
- **The CLI did not cut a major.** §8.4 stays a CLI question. Removing `-i
  xml`/`-o xml` is a break on paper, but no shipped binary ever compiled the
  format in, so no user of a shipped binary observes it; the selectors are
  gone from `--help` and the CLI stays at 4.0.0 until something a user can
  see changes.
- **BREAKING-CHANGES.md is gone**, not emptied. Everything it listed shipped
  here, and a planned break is now a `Behavioural-change:` trailer on the
  commit that lands it, gathered into the changelog's unreleased section —
  the same place a shipped one is recorded, which is where a consumer looks.

What 3.0 is not: an extensibility story beyond the tree. §4's boundary — the
engine is generic over any `Language` that passes `validate`, the registry
is fig's — is unchanged, and so is the answer on runtime plugins. That is
the next argument, and it is a different proposal.

