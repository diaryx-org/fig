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
| Unit tests | `zig build test`, `zig build all-on` | Does each piece do what it says? |
| Conformance | [fig-conformance](https://github.com/diaryx-org/fig-conformance) | Do we agree with the format's own spec suite? |
| Fuzzing | `zig build fuzz --fuzz=1G` | Is there *any* input that breaks us? |

The unit tests run on every push, via `zig build check`. Fuzzing runs nightly —
see [Fuzzing](#Fuzzing) for why it cannot sensibly run on a pull request.

`check` also runs the Rust and TypeScript binding suites, each of which skips
with a note when its toolchain is missing, so the gate stays useful on a partial
checkout. The TypeScript suite additionally skips on Node older than 24 — see
[Developing the binding](typescript.md#developing-the-binding) for why.

## Every format on

`zig build all-on` runs the library's tests and the CLI's again in a build
with every format compiled in. plist and canonical are off by default, so
this is the only build that proves the everything-on configuration still
compiles, and the CLI is what instantiates the generic engines for every
language. `check` depends on it.

Between the unit tests and conformance sits one more layer:
`src/languages/harness.zig`, the checks that hold for *any* format, run over
every compiled-in dialect. Each format declares a few `samples` in its own
grammar; the harness parses each, prints it, and reparses it to the same
tree, checks that `Document.node_regions` is well-formed and that a section
format's parser fills it, and constructs an `Editor` over it with a no-op
splice. It also parses every registry row's `empty_doc_seed`. A new format is
covered by declaring `samples`.

# Conformance

Conformance suites answer a question unit tests cannot: *does `fig` agree with
everyone else?* They live in their own repository, fig-conformance, beside
this one, because they prove a *language* and not this library: a suite
scores any implementation of a format through the language contract, the
compiled Zig formats and the JavaScript languages in `@diaryx/fig` alike.
There are parse suites (yaml-test-suite, toml-test, JSONTestSuite,
json5-tests, the NestedText suite, plist and figl) and an edit suite for
every format fig edits. Its README says how to run and record them.

# Fuzzing

Conformance proves `fig` handles the inputs somebody thought to write down.
Fuzzing goes looking for the ones nobody did.

A fuzzer generates inputs, runs a parser on them, and reports anything that
crashes, leaks, or violates a stated property. It is **coverage-guided**: it
instruments the parser, watches which branches each input reaches, keeps the
inputs that reach new code, and mutates those. It is not random noise — it hill
climbs into the corners of the grammar. This matters here because `fig` is
~12,000 lines of hand-written tokenizer and parser whose entire job is consuming
untrusted bytes; that is the textbook target.

The bug classes it finds are ones conformance structurally cannot: panics on
truncated input, overflow in span arithmetic, unbounded recursion, leaks on
error paths (the `parseCollecting` recovery paths especially), and silent
mis-parses.

```bash
zig build fuzz --fuzz=1G      # fuzz until you stop it
zig build fuzz --fuzz=50K     # or cap the iterations
```

Two flags, not one: `zig build fuzz` alone just runs the targets once. `--fuzz`
is a build-*runner* flag, so it cannot be turned on from `build.zig`. Its
argument is an **iteration cap, not a time budget** — and omitting it implies
`--webui`, a live coverage view served until you interrupt it. Locally that is
excellent; in CI it is a hang, which is why the workflow always passes `=<n>`.

## The targets

Both live in `src/fuzz.zig`, split by what they can prove:

- **`detect`** — *breadth*. `Language.detect` runs every compiled-in parser over
  the same bytes, so one target sweeps all eleven grammars for crashes and
  leaks. It cannot check correctness: the result is just an enum.
- **canonical round-trip** — *depth*. The canonical form is a bijection with the
  AST, so for any input it accepts, `parse → print → parse` must land on the
  same tree. This is the one property strong enough to catch a **silent
  mis-parse** — a parser that accepts input and builds the wrong tree, which
  `detect` would never notice.

Leaks come free: the test runner gives each iteration a fresh
`std.testing.allocator` and fails on a leak.

Note what is *not* asserted: `parse → print == original`. Printers render from
the AST, not from `Document.source`, so byte-identical output is not a property
of this codebase — asserting it would just freeze today's formatting. The real
claim is idempotence from the second print on.

## On pull requests

The targets run as ordinary tests on every `zig build test`, because
`src/root.zig` imports `src/fuzz.zig` unconditionally. **This is not fuzz
coverage.** Outside fuzz mode, Zig runs each target over a single empty input,
so both pass trivially. It exists so a target cannot silently stop compiling or
have its property broken between nightly runs. A green PR says the targets are
alive, nothing more.

## Nightly

`.github/workflows/fuzz.yml` fuzzes for 15 minutes each night and, on a finding,
files an issue with the corpus attached. It is deduped on the `fuzz` label — an
unfixed crash is deterministic and would otherwise refile every morning.

Fuzzing has no natural stopping point, which is the whole reason it is not on
the gate: you cannot block a merge on an unbounded search.

The budget is enforced by `timeout(1)`, not by `--fuzz`, because iteration
throughput is a property of the machine — ~50K iterations took over 10 minutes
on an M-series laptop, and a CI runner is slower again. Any count tuned to fill
15 minutes there is wrong everywhere else. So the workflow sets an iteration cap
it will never reach and lets the clock end the run.

## Why the nightly greps instead of trusting `$?`

The workflow decides pass/fail by **scanning the fuzzer's output for a failure
marker**, and only then looks at the exit code. That inversion is deliberate.

On Zig 0.16.0, `zig build fuzz` does not reliably exit non-zero when a target
fails. Verified on aarch64-macos with a deliberately-always-failing target:
`zig build test` exits 1 correctly, but under `--fuzz` the same failure prints
`error: '<test>' failed:` with a full error return trace and then **hangs** —
so `timeout` reaps it and reports 124, which is exactly what a clean run
reports. A nightly trusting the exit code would be permanently, silently green
while sitting on a crash. That is worse than no nightly, because it looks like
coverage.

Relatedly, `--fuzz=<n>` does not appear to terminate at its limit at all:
`--fuzz=100` ran for five minutes without printing a report. This is why the
budget is a clock rather than a count.

Both may well be macOS-only — the fuzzer is least exercised there, and CI runs
Linux — but a redundant check costs nothing, and being wrong about it would be
invisible. The marker is `^error: `, and only that. If a future Zig makes the
exit code trustworthy, the grep can go.

`^failed command: ` was part of the pattern too, on the reading that it meant
"the build system reporting the run step died". It does not mean that. Zig
0.16's build runner prints a step's report whenever the step wrote anything to
stderr — `build_runner.zig`'s *"No matter the result, we want to display
error/warning messages"* — and a `failed command: <argv>` line comes with it,
because `Run` fills in `result_failed_command` before spawning the child and
clears it only on two paths that success is not one of. A green `zig build
check` printed one, for the conformance scoreboards it then ran, and still exited 0;
that line is context, not a verdict. The nightly passes `--error-style minimal`,
which suppresses it (the flag gates `error_style.verboseContext()`) while
leaving `error:` lines and their return traces intact, so the marker cannot fire
on a clean run. The fuzz targets print nothing to stderr today, so this was a
false positive waiting to happen rather than one already happening — but it was
one stray `std.log` line away from filing an issue against a green nightly.

To reproduce a nightly failure: download the run's `fuzz-corpus-*` artifact,
unzip it so `f` lands at `.zig-cache/f`, and re-run `zig build fuzz --fuzz=1G`.
The fuzzer resumes from that corpus rather than starting cold. That is a head
start, not a stored repro case — the run log names the failing target and
carries the error return trace, so read that first. `-Dtest-filter` narrows to a
single target.

**Known limitation:** an input that makes a parser loop forever is
indistinguishable from a clean run, since both end at the budget. Zig 0.16's
fuzzer has no per-input timeout. Crashes, leaks and broken invariants are
caught; hangs are not.

## Adding a target

Add it to `src/fuzz.zig` next to the two there; `root.zig` already imports the
module, and the `fuzz` step filters on `fuzz`, so nothing else needs wiring.
Start the body with `@disableInstrumentation()` — the fuzzer should be measuring
the parser's coverage, not the harness's — and use `std.testing.allocator` so
leaks are caught.

Prefer a target that asserts a **property** over one that only checks for
crashes; "does not crash" is a low bar, and the canonical round-trip shows what
a real oracle buys. One trap worth knowing: `FuzzInputOptions.corpus` entries
are `Smith` *decision tapes*, not source text, so a conformance corpus cannot
be dropped in as seeds — its cases are ordinary files.
