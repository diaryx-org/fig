```fig
title = Rust core
part_of = [Proposals](/docs/proposals/proposals.md)
description = fig's core, kit and languages move from Zig to Rust a layer at a time, each held to what the Zig implementation answers through checks that never import either one
author = adammharris
created = 2026-10-15
status = implemented
updated = 2026-10-15
```

# Rust core

> **Status: SWITCHED OVER, in 7.0.0.** Written against `main` at
> 0afdab4 (6.0.0). Every step of §4 is done. The kit (`rust/fig-kit`), the
> core (`rust/fig-core`), the ten languages (each in its own crate, in
> `languages/<name>/rust/`), the CLI and LSP (`rust/fig-cli`), the helper
> wire and the C ABI (`rust/fig-capi`) were ported and agreed with the Zig
> on every check of §3 ([rust/README.md](/rust/README.md) has the numbers),
> and then the Zig was removed: the release binaries, the npm packages'
> wasm modules (`rust/fig-capi`, `rust/fig-kit-capi`,
> `rust/fig-lang-export`) and CI are built from the Rust, in one Cargo
> workspace at the repository root, and `zig build` is `cargo xtask`. The
> Zig package went with it, which is what makes 7.0.0 a major: Zig gets
> fig through `fig.h`, as §1 says ([Migrating to 7.0](/docs/migrating-to-7.md)).
> §6's open questions are as they stood.

## 1. The claim

fig ships to Rust, TypeScript, C and Zig, and most of the work of shipping
is getting Zig out of the way of the first three. For Rust that is
thirteen `fig-sys-<target>` payload crates; `fig-sys` with a cfg
dependency and a `build.rs` arm for each; `prebuilt-targets.tsv`, which
four other files mirror by hand; `fig-language-build` and a `build.rs` in
each of the ten language crates that runs `zig build`; and a fallback that
needs Zig anyway on any target not in the table. Fixes like the iOS panic
trap (afffa04) and position-independent archives (52c8161) exist only
because of it.

Nothing in fig needs Zig in particular. The parsers, printers and editor
are hand-written byte-level code, and the reflection in them (`@typeInfo`,
`@field`, `inline for`, rarely more than a dozen uses in a file) is what
traits, `macro_rules!` and the `fig-macros` derive already do. So:

> **fig's core is written in Rust.** Rust users get ordinary crates;
> TypeScript gets the same wasm module through the same C-shaped exports;
> C gets the same `fig.h`; Zig gets fig through that header.

## 2. What it costs

- **Size.** About 54,000 lines of library (`editor.zig` alone is 8,000;
  the YAML and figl parsers 4,500 and 3,500), and 1,464 Zig tests.
- **Zig consumers.** `docs/zig.md` and the Zig kit are public. After the
  port, Zig reaches fig through the C ABI, as any other C consumer does.
  How much that costs depends on who imports fig as a Zig package (§6).
- **Kit parity.** The kit was just made the same in Zig, C, Rust and
  JavaScript ([kit parity](kit-parity.md)). The Rust kit becomes the
  implementation; the C kit becomes its exports; the Zig kit a wrapper
  over those.
- **Regressions.** Lossless editing is where a port goes subtly wrong: a
  byte moved, a comment orphaned. §3 is the answer to this one.

## 3. The gate

A layer is ported when the Rust implementation answers what the Zig one
does, on checks that import neither. Three exist; the third is new.

### 3.1 fig-conformance

The parse suites score a dialect against its upstream's own corpus, and
the edit suites hold 27,503 cases recorded from the compiled formats
([conformance suite](conformance-suite.md) §4.1). Both reach an
implementation through an adapter, a command speaking JSON lines
(fig-conformance's `docs/adapters.md`). A port is scored by building one
such command and running the suites unchanged. The bar is the Zig
implementation's scores: `scores/compiled.json` for parsing, and every
edit case exact (today, for example, INI 1253/1253 and dotenv 709/709).

### 3.2 Edit fixtures

The edit suites are generated from each source's tree: every entry
renamed, deleted, replaced. They do not hold the cases somebody wrote
because a bug was found, which is most of what the 460 tests in
`languages/*/tests/editor.zig` and the 91 in `src/editor.zig` are. Those
are Zig, and drive `Editor(Lang)` directly, so they would prove nothing
about a Rust editor.

Each language's editor tests become `languages/<name>/tests/edits.json`:

```json
{
  "id": "deleteKey refuses a whole [section], and still deletes a key within it",
  "note": "Zig error CannotDeleteSection. …",
  "source": "[server]\nhost = localhost\n",
  "steps": [
    { "op": "deleteKey", "args": [["server"]], "expect": { "ok": false, "status": "InvalidArgument" } },
    { "op": "deleteKey", "args": [["server", "host"]], "expect": { "ok": true, "source": "[server]\n" } }
  ]
}
```

A case is a source and edits applied in turn, each to the one before's
output. An op is an `Editor` method as the bindings name it, with JSON
arguments; most are in the adapter protocol under the same names. The
expectation is the whole edited source, or a refusal's status; one
without them holds only that the edit was taken, or refused, where the
Zig test asserted no more of that step. A fixture names one dialect, and
a case may name another (`json5`, `jsonc` in the JSON fixture). The
`note` carries what the Zig test's comments said, since the reason a
case exists is most of its value. The expectations are the ones the Zig
test asserted, copied, not recorded: a fixture that disagrees with fig
is a finding, not something to re-record.

`tools/edit-fixtures.mjs` runs them, against fig's editor through the
TypeScript binding and each language's wasm module by default, or with
`--command` against any adapter that answers `edit`. `zig build check`
runs it beside the TypeScript suite.

All of them are converted: 551 tests, 753 cases (a test that pinned
several sources is a case per source), every one passing against the Zig
editor. `src/editor.zig`'s tests went to the fixture of the language each
drives, which is where dotenv's and `.properties`'s fixtures come from.
23 tests stay Zig-only, and two more lost a source each: they start a
list with `setAppend` or edit through an alias, which the C ABI does not
export; they pass a key's raw spelling to `insertKey` or
`replaceKeyAtPath`, where the binding names the key and spells it itself;
they only read comments back, or call a renderer directly; or they
assert a substring of the output, or a value read from the tree, and
never the bytes. Three things showed up in converting them:

- **Raw edits.** A Zig test passes text the format spells; a binding
  takes a value. Where the value serializer cannot write it (INI cannot
  serialize a nested mapping), the case uses the `*Raw` op, as the
  binding does.
- **Statuses are coarse.** `CannotDeleteSection`, `NotASection` and
  `CannotMoveSection` all reach the wire as `InvalidArgument`. A case
  names the Zig error in its `note`, and the Rust port keeps the
  distinction in its own error type even where the wire does not.
- **Binding gaps.** No binding op starts a list (`setAppend` is the CLI's
  alone), edits through an alias, or splices a key's raw spelling. And in
  a figl JSON-mode flow mapping (`p = { "x": 1 }`), `insertValue` of a
  new name is refused with `ParseError` where the raw `"y"` lands.

A test that reads the editor's internals (`Document.regionsOf`, the
parsed AST after an edit) converts to its edit alone when the output
pins the same fact, and otherwise stays a Zig unit test until its module
is ported, then becomes a Rust one.

### 3.3 The twins and fuzzing

`bindings/typescript/test/fixtures/` holds the JS twins to the compiled
formats' tables, and serves the same purpose for a Rust language's
tables. The nightly fuzz targets move to `cargo fuzz`, seeded with the
Zig corpora.

## 4. Sequencing

Every step ships, and the work can stop after any of them.

1. **Freeze the behaviour.** Convert every language's editor tests and
   `src/editor.zig`'s to fixtures (§3.2): INI (done), then dotenv and
   .properties from `src/editor.zig`, ZON, plist, NestedText, JSON,
   TOML, figl, YAML. The test names say which spec each pins; a
   language's Zig tests are deleted only when its fixtures pass and its
   module is ported.
2. **An adapter for the Rust side.** A binary in the Rust workspace
   speaking the protocol's `hello`, `parse`, `print` and `edit` over the
   `fig` crate. At first it serves the Zig implementation through
   `fig-sys`, which proves the adapter before anything is ported: it must
   score what the in-process runner scores.
3. **The kit and core.** `kit/` and `src/` (without the CLI) become the
   `fig` crate's own code, behind its current public API, so its callers
   see nothing. The C ABI the Zig languages link against stays, served
   from Rust, so the not-yet-ported languages keep working, linked as
   `libfig_<name>.a` beside it.
4. **One language at a time.** dotenv first (`fig::kit` already has a
   dotenv written with it, f8c7218), then INI, .properties, ZON, plist,
   NestedText, JSON, TOML, figl, YAML. Each one's crate drops its
   `build.rs` when it is ported. A language is done when its fixtures,
   its conformance scores and its twin tables all match.
5. **The exports.** `libfig` and the wasm modules are built from Rust,
   with `fig.h` unchanged, so `zig build abi-check`'s probes and the
   TypeScript binding hold them as they hold the Zig ones. Then
   `fig-sys-*`, `fig-language-build`, `prebuilt-targets.tsv` and the
   payload CI go.
6. **The CLI.** Last: it has the least to gain, and `tools/cli-*-check.sh`
   already hold it from outside.

## 5. What stays

- `fig.h` and its ABI, held by `abi-check` and `semver-check` as now.
- The TypeScript API, and the wasm module's exports it calls.
- The adapter protocol and both conformance tiers.
- The version: the port is not a major version unless an API changes.

## 6. Open questions

- **Who imports fig as a Zig package?** twig locates the blocks fig
  edits; if it also imports fig, it moves to the C ABI or to Rust with
  it. Anyone else on `docs/zig.md` does the same.
- **Where the fixtures live.** They sit beside each language now, because
  they are the language's tests. Their format is close enough to
  fig-conformance's edit cases that they could join those as a
  hand-written tier, scored the same way, once nothing in fig needs them
  at hand.
- **The build tool.** `zig build check` is the one gate today. During the
  port it runs both sides; after it, a `cargo xtask check` or the
  existing `figl/` tooling takes the gate, and Zig leaves CI.
