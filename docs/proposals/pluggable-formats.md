```fig
title = Pluggable formats
description = What fig 3.0 is for — the format set written down once, every per-format fact declared by the format, and the editing contract a format writes against named in one place
created = 2026-09-04
status = draft
updated = 2026-09-04
part_of = [proposals](proposals.md)
```

# Pluggable formats

> **Status: DRAFT.** The argument for fig 3.0, written against `main` at
> c9761cd (cli 4.0.0, core 2.7.0 plus the unreleased derived-regions work).
> Nothing here has been built. §6 says what lands in 2.x minors first and what
> the major itself is; §8 lists the questions still open.

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
[BREAKING-CHANGES](/docs/BREAKING-CHANGES.md) since 2.4, because both of those
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
  [BREAKING-CHANGES](/docs/BREAKING-CHANGES.md). Its migration table is
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
