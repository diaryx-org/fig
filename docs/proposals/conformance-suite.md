```fig
title = Conformance suite
description = The conformance corpora and their runner leave the fig library for a repository of their own, and score any implementation of a language through the contract rather than by importing its parser
created = 2026-10-11
updated = 2026-10-11
status = draft
part_of = [proposals](proposals.md)
```

# Conformance suite

> **Status: DRAFT.** Written against `main` at acd8701 (5.2.0). It is the
> gate that [core only](core-only.md) §8 step 5 refers to. The edit suites
> of §4.1 are built, in a local `fig-conformance` repository with no
> remote yet. The parse suites moved there on 2026-10-11, and fig keeps
> no corpus or scoreboard (§5). `fig lang serve` (§4) is not built; the
> runner reaches the compiled formats through a wasm module that compiles
> them all in.

## 1. The claim

The conformance corpora prove a *language*, not the library. Today they
are wired to the library instead. `src/languages/yaml/conformance.zig`
imports `parser.zig`, `printer.zig`, `materialize` and `Embed`, and the
TOML, JSON, JSON5, plist and NestedText scoreboards do the same, seven
in all. A
scoreboard therefore tests one implementation, the compiled Zig one. To
score the JS twin of the same format took a separate runner written for
the purpose, and a Lua or Rust implementation has nothing at all.

Once every language reaches core only through the contract
([core only](core-only.md) §5.1), the contract is also the only thing a
conformance check needs. This proposal moves the corpora and a runner
that speaks the contract into a repository of their own:

> **A conformance suite scores a language, by name, through the wire.**
> It never imports an implementation, and it says nothing about which
> language that implementation is written in.

## 2. Where 5.2 stands

| Format | Corpus in `testdata/` | Upstream | What is checked |
|---|---|---|---|
| YAML 1.2 | `yaml/` accept, reject, stream, reject-stream | yaml-test-suite | parses or refuses; reprints; converts to JSON |
| YAML 1.1 | `yaml-1.1/` | fig's own | tagged values |
| TOML 1.0, 1.1 | `toml/`, `toml-1.1.0/` | toml-test | tagged values; refuses |
| JSON | `json/` accept, reject, edgecase | JSONTestSuite | parses or refuses |
| JSON5 | `json5/` | json5-tests | parses or refuses |
| plist | `plist/` valid, invalid | fig's own, with `plutil` JSON oracles | plain values where an oracle exists; refuses |
| NestedText | `nestedtext/tests.json` | nestedtext_tests | plain values; refuses |

The scoreboards ratchet, except JSON's, which fails on any miss: each
holds a baseline in its source and fails below it.

Two things are weaker than they look:

- **A YAML accept case is only checked to parse.** The yaml-test-suite
  ships the expected JSON (`in.json`) for most cases, but
  `tools/gen_yaml_conformance.zig` vendors only the input. A parser
  that returned the wrong tree would score 289/289.
- **The generator parses the suite with fig.** Its own header says so,
  and calls it "somewhat circular": after a YAML parser change the
  vendored corpus has to be cross-checked against another parser. The
  suite's `data` branch already has every case expanded to files, and
  reading those needs no YAML parser.

## 3. The repository

`diaryx-org/fig-conformance`, public, MIT/Apache-2.0 like the other
public repos, with each upstream corpus under its own licence as now.

```
fig-conformance/
  suites/
    yaml/        suite.figl, corpus/, LICENSE   (from yaml-test-suite's data branch)
    yaml-1.1/    suite.figl, corpus/
    toml/        suite.figl, corpus/, LICENSE   (from toml-test)
    json/  json5/  plist/  nestedtext/
  runner/        the runner (§4)
  scripts/       one vendoring script per upstream, none of which parses
                 with fig
```

A suite is a manifest and a corpus. The manifest names the dialect it
targets and lists its checks, each check a directory and a kind:

```fig
dialect = yaml
spec = 1.2.2
check[]
> kind = parses
> corpus = corpus/accept
> expect = json        # each case's sibling .json is the tree
check[]
> kind = refuses
> corpus = corpus/reject
check[]
> kind = reprints
> corpus = corpus/accept
check[]
> kind = stream
> corpus = corpus/stream
```

The manifest holds no baseline. A score is a fact about an
implementation, so it lives with the implementation (§5).

## 4. The runner

The runner is a small JavaScript program with no dependencies beyond
Node. It reaches an implementation in one of two ways, both of them the
contract:

- **A command**, spoken to over the helper wire, as `fig lang check`
  does: `fig-conform yaml --command "fig-lua yaml.lua"`.
- **A module** exporting a `Language` object, called through the same
  wire in-process: `fig-conform yaml --module ./languages/yaml.js`.

That covers every implementation fig has or plans: the JS twins as
modules; fig-lua, fig-quickjs and Rust helpers as commands; and the
compiled formats through a new `fig lang serve <name>`, which runs a
compiled language behind the wire as `serve(lang)` does for a JS one.
After core only, a Zig language is registered behind the same vtable,
and `fig lang serve` reaches it the same way.

### 4.1 Two tiers of check

**Wire checks** need nothing but the implementation:

| Kind | Asks the wire | Passes when |
|---|---|---|
| `parses` | `parse` | a table comes back; with `expect`, its tree matches the expected one |
| `refuses` | `parse` | a refusal comes back |
| `reprints` | `parse`, `print`, `parse` | the printed text parses again; with `expect`, to the same tree |
| `table` | `parse` | the table equals a recorded `.table.json`, row for row |
| `stream` | `split`, then `parse` per document | every document parses (needs core only's `splitStream`) |

Comparing trees needs scalar *values*, and a row carries a lexeme. The
runner needs nothing per-format for that once a row carries its value
whenever the lexeme is not in the canonical spelling ([core
only](core-only.md) §4.2). Until then, the runner compares only what the
lexeme makes unambiguous, and reports the rest as skipped, not passed.

**Library checks** need fig, and take it as an ordinary dependency, the
published `@diaryx/fig`. They are what a language has to do *with* fig:
convert to JSON and back, keep comments through a conversion, and edit.
Editing had no corpus, so the suite started one, and it is built
(fig-conformance 665c268, local). Each source document's cases are
generated from its tree: every entry renamed, deleted, replaced by each
kind of scalar and container, and given or relieved of each kind of
comment; every mapping given new entries, reordered, and edited as a
whole container; every sequence appended to, prepended to, cut,
reordered. A case is one `Editor` method with its arguments as JSON. The
expectation is the compiled format's answer: the edit as a byte splice of
the source, or the status it refused with. Messages are not compared.
The twelve suites hold 27,503 cases in 4.9 MB.

Recording from the compiled formats makes the suite a parity check, and a
fault both sides share passes it. So the recorder also checks an
invariant that needs no reference: a comment edit changes no value. A
recorded answer that breaks it is marked `suspect`, and the runner counts
how many of its matches are to one. The first recording marked six.

The tiers are separate so that the wire tier stays usable by someone
who has no interest in fig. A YAML library in any language could score
itself against it by speaking the wire.

### 4.2 Output

One line per check, `correct/total`, and the failing cases with a reason
on request, as the scratch runner printed them. `--json` prints the
scores for a caller to compare against its baseline.

Some cases are outside the contract and are marked so in the manifest,
not counted as failures. Invalid UTF-8 is the one known today: the wire
carries text, so such a case is the host's to refuse, and fig's core
does.

## 5. What fig keeps

Nothing of conformance. The corpora, their runners and the scores all live
in fig-conformance, which holds one scores file per implementation it
knows (the compiled formats, the JS languages) and fails a run that falls
below one. fig's `zig build check` does not run the suites. Raising a
score is an edit to that file in the fig-conformance commit that records
it.

Deleted from fig on 2026-10-11:
`testdata/{yaml,yaml-1.1,toml,toml-1.1.0,json,json5,plist,nestedtext}`,
the seven scoreboards (`conformance*.zig`, `json5_conformance.zig`), the
`-D<suite>-conformance` options, `tools/gen_*_conformance.zig` and
`tools/check_yaml_trees.zig`. The `conformance` build step also ran the
library's and the CLI's tests with every format compiled in; that part
stays, as `zig build all-on`. The fixtures in
`bindings/typescript/test/fixtures/` stay: they test the twins against the
compiled formats' tables, not a format against its spec, and they go once
there is one implementation of each format.

fig-lua and fig-quickjs keep their own scores files in fig-conformance
the same way. A language added later starts with one.

## 6. Sequencing

1. **`fig lang serve`.** The CLI serves a compiled language over the
   wire. It is small, and it gives the runner the compiled formats to
   score against.
2. **The repository.** Vendor the corpora from upstream with scripts that
   do not parse with fig. Add the YAML suite's expected JSON. Port the
   scratch runner's wire checks.
3. **Baselines, both ways.** Run the suite against `fig lang serve` and
   the twins. The compiled scores should match the Zig scoreboards, and
   any difference is a bug in the runner. The YAML `expect` checks are
   new, so their first scores are new facts.
4. **Switch fig over.** Delete what §5 lists. Done, without a check in
   fig: the suites run from fig-conformance.
5. **The editing corpus.** Done for every language fig edits (§4.1).
   More invariants belong beside the first: a value edit changes nothing
   outside its path, and a reorder changes no value.

## 7. Open questions

- **The name.** `fig-conformance` says whose suite it is. Its wire tier
  is useful outside fig, which argues for a name that does not say so.
- **The runner's language.** JavaScript because the wire is JSON lines,
  the twins are JS, and Node is already a dev dependency of fig's
  checks. A single static binary would spare a consumer Node, at the
  cost of the library tier, which wants `@diaryx/fig`.
- **Whether fig vendors or fetches.** A pinned tag fetched at check time
  needs the network in CI. A git submodule, or vendoring at the pinned
  tag, does not.
