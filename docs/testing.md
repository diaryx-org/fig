```fig
title = fig testing
author = adammharris
part_of = [fig docs](docs.md)
```

# Testing `fig`

`fig`'s job is to read bytes it did not write, in eleven formats, without
misreading them. Three layers guard that, in increasing order of paranoia:

| Layer | Where | Asks |
| --- | --- | --- |
| Unit tests | `cargo test --workspace --all-features` | Does each piece do what it says? |
| Conformance | [fig-conformance](https://github.com/diaryx-org/fig-conformance) | Do we agree with the format's own spec suite? |
| Fuzzing | `cargo fuzz` in `fuzz/` | Is there *any* input that breaks us? |

The unit tests run on every push, via `ship ci`. Fuzzing runs nightly —
see [Fuzzing](#Fuzzing) for why it cannot sensibly run on a pull request.

`ci` also runs the built CLI end to end (`tools/cli-*-check.sh`), every
language's edit fixtures through the Rust editor (`rust/fig-conform`) and
through the TypeScript binding, and the TypeScript and WASI suites. Each npm
suite skips with a note when Node or its `node_modules` is missing, so a
contributor with Rust alone still gets a useful gate. The TypeScript suite additionally skips on Node older than 24 — see
[Developing the binding](typescript.md#developing-the-binding) for why.

## Every feature on, and the defaults

`ci` runs the workspace's tests twice: once with every feature of every crate
on (`--all-features`, which brings in the opt-in `canonical` format and each
language crate's `fig` and `serde` APIs), and again for the kit, the core,
the CLI, the C library and the conformance adapter with their default
features, which take each language alone, without its `fig` binding. Cargo
unifies features across a workspace build, so only the second run proves those
crates compile as they ship.

Between the unit tests and conformance sits one more layer:
`rust/fig-core/tests/harness.rs`, the checks that hold for *any* format, run over
every compiled-in dialect. Each format declares a few `samples` in its own
grammar; the harness parses each, prints it, and reparses it to the same
tree, checks that `Document.regions_of` is well-formed and that a section
format's parser fills it, and constructs an `Editor` over it with a no-op
splice. It also parses every registry row's `empty_doc_seed`. A new format is
covered by declaring `samples`.

# Conformance

Conformance suites answer a question unit tests cannot: *does `fig` agree with
everyone else?* They live in their own repository, fig-conformance, beside
this one, because they prove a *language* and not this library: a suite
scores any implementation of a format through the language contract, the
Rust languages and the JavaScript languages in `@diaryx/fig` alike.
There are parse suites (yaml-test-suite, toml-test, JSONTestSuite,
json5-tests, the NestedText suite, plist and figl) and an edit suite for
every format fig edits. Its README says how to run and record them.

# Fuzzing

Conformance proves `fig` handles the inputs somebody thought to write down.
Fuzzing goes looking for the ones nobody did.

A fuzzer generates inputs, runs a parser on them, and reports anything that
crashes, hangs, or violates a stated property. It is **coverage-guided**: it
instruments the parser, watches which branches each input reaches, keeps the
inputs that reach new code, and mutates those. It is not random noise — it hill
climbs into the corners of the grammar. This matters here because `fig` is
thousands of lines of hand-written tokenizer and parser whose entire job is
consuming untrusted bytes; that is the textbook target.

The bug classes it finds are ones conformance structurally cannot: panics on
truncated input, overflow in span arithmetic (the fuzz build keeps overflow
checks and debug assertions on), unbounded recursion, infinite loops, and
silent mis-parses.

The fuzzer is libFuzzer, through [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz),
which needs a nightly toolchain:

```bash
cargo install cargo-fuzz --locked
cargo run -p fig-fuzz --bin fig-fuzz-seed -- fuzz/corpus   # optional: start from the seeds
cargo +nightly fuzz run detect                              # fuzz until you stop it
cargo +nightly fuzz run canonical_round_trip -- -max_total_time=60 -dict=fuzz/dict/canonical_round_trip.dict
```

Everything after `--` is libFuzzer's: `-max_total_time=<s>` ends the run on
the clock, `-max_len=2048` is what the nightly uses, `-timeout=<s>` sets how
slow one input may be before it counts as a hang, and `-dict=` hands it the
target's tokens. A run grows `fuzz/corpus/<target>/` and writes each failing
input to `fuzz/artifacts/<target>/`; both are git-ignored.

## The targets

Each target's body is a plain function in `rust/fig-fuzz`; `fuzz/` is the
cargo-fuzz project whose `fuzz_target!`s call them. The two are split by what
they can prove:

- **`detect`** — *breadth*. Detection tries every registered parser over the
  same bytes, so one target sweeps all ten languages for panics, stack
  overflows and hangs. It cannot check correctness: the result is a format
  name or none.
- **`canonical_round_trip`** — *depth*. The canonical form is a bijection with
  the AST, so for any input it accepts, `parse → print → parse` must land on
  the same tree, comments included (`AST::eql` ignores them, so they are
  checked apart with `comments_eql`), and printing that tree again must give
  the same bytes. This is the one property strong enough to catch a **silent
  mis-parse** — a parser that accepts input and builds the wrong tree, which
  `detect` would never notice.

Note what is *not* asserted: `parse → print == original`. Printers render from
the AST, not from `Document.source`, so byte-identical output is not a property
of this codebase — asserting it would just freeze today's formatting. The real
claim is idempotence from the second print on.

Leaks are not asserted either: ownership frees every Document on every path,
error paths included.

libFuzzer hands a target raw bytes, so what steers it toward input the parsers
walk into, rather than reject at the first byte, is coverage plus a
dictionary per target in `fuzz/dict/`: the structural tokens the grammars
branch on (newlines, indentation, brackets, quotes, `=`, `:`, `&`, `*`, …) for
`detect`, and the canonical grammar's own (`@offset_datetime`, `~i`, `//`,
`/*`, …) for `canonical_round_trip`.

## Seeds, and on pull requests

`rust/fig-fuzz` has a seed set per target: every language's own samples (the
ones its registration already checks it parses) plus hand-written edges for
`detect`; those samples printed as canonical text, plus hand-written canonical
text for extended scalars, anchors, tags, comments and nesting at the depth
guard, for `canonical_round_trip`. They serve twice:

- `cargo test -p fig-fuzz` runs each target over its seeds, and `cargo xtask
  ci` runs it with the rest of the workspace. **This is not fuzz coverage.** It
  exists so a target cannot silently stop compiling or have its property broken
  between nightly runs. A green PR says the targets are alive, nothing more.
- `fig-fuzz-seed <dir>` writes them out as `<dir>/<target>/seed-<n>`, so a
  fuzz run starts from inputs every parser walks deep into rather than from
  the empty input. The nightly runs it before fuzzing.

When a nightly finding is fixed, add its input to the target's seeds, so every
PR keeps it fixed.

## Nightly

`.github/workflows/fuzz.yml` (generated from `figl/fuzz.figl`) fuzzes for 15
minutes each night, half per target, and, on a finding, files an issue with the
failing inputs and the corpus attached. It is deduped on the `fuzz` label — an
unfixed crash is deterministic and would otherwise refile every morning. A
manual run takes a `budget` (`90s`, `1m`, …) for a quick check of the workflow.

Fuzzing has no natural stopping point, which is the whole reason it is not on
the gate: you cannot block a merge on an unbounded search. The clock bounds the
run (`-max_total_time`), not an iteration count, because throughput is a
property of the machine.

The exit code is the signal. libFuzzer exits nonzero on a panic, a sanitizer
report, a stack overflow, an input over its memory limit, or an input slower
than `-timeout` (30 seconds in the nightly), so **a parser that loops forever is
a finding**, not something indistinguishable from a clean run. Both targets run
even when the first fails, so one finding cannot hide another.

To reproduce a nightly failure: download the run's `fuzz-corpus-*` artifact,
unzip it into `fuzz/` so its `artifacts` and `corpus` land there, and run the
failing input alone:

```bash
cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<file>
cargo +nightly fuzz tmin <target> fuzz/artifacts/<target>/<file>   # shrink it first
```

The issue lists the failing files, and the target's log in the artifact
carries the panic or the stack.

## Adding a target

Write its body as a `pub fn(&[u8])` in `rust/fig-fuzz` that panics on a
failure, with a seed set and a `#[test]` running it over them; add a
`fuzz_target!` calling it under `fuzz/fuzz_targets/`, a `[[bin]]` for it in
`fuzz/Cargo.toml`, a dictionary in `fuzz/dict/`, and its name to the nightly's
`targets` list in `figl/fuzz.figl` (then `cargo xtask sync-figl`).

Prefer a target that asserts a **property** over one that only checks for
crashes; "does not crash" is a low bar, and the canonical round-trip shows what
a real oracle buys.
