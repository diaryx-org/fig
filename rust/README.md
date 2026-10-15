```fig
title = fig in Rust
part_of = [fig docs](/docs/docs.md)
description = fig's implementation in Rust: the crates of the workspace, and how the port from Zig was held to the Zig before it replaced it
author = adammharris
created = 2026-10-15
updated = 2026-10-15
```

# fig in Rust

This directory, with each language's `rust/` in its package under
`languages/`, is fig. It began as a port of fig from Zig to Rust, following
[the proposal](/docs/proposals/rust-core.md), and replaced the Zig in 7.0,
when the Zig was deleted. The port was file for file: many `.rs` files still
start with `//! Port of <zig path>`, naming a file that is now only in the
history, and keep the Zig file's names and doc comments. It needs no C to
build, and uses no crates outside `std` but one: `fig-cli` reads its command
line with clap, so its usage errors and `--help` pages are clap's (the rules
they enforce, and everything else the CLI prints, were the Zig's).

| Crate | Where | What it is (and what it ported) | Lines |
|---|---|---|---|
| `fig-kit` | `rust/fig-kit` | `kit/` (the node table, AST, `Document`, the `Language` trait, the shared rules) and fig's error type; depends on nothing | 10,600 |
| `fig-json`, `fig-yaml`, … (ten) | `languages/<name>/rust` | each language's `languages/<name>/src/`, on `fig-kit` alone; its `fig` binding API behind its default `fig` feature | 35,300 |
| `fig-core` | `rust/fig-core` | `src/editor*`, `src/serialize.zig`, `src/canonical/`, `src/materialize.zig`, `src/lossless.zig`, `src/flat_strip.zig`, `src/diagnostics.zig`, `src/patch.zig`, `src/contract/` (the registry, the helper wire, the host), and the logic of `src/c_api.zig` (`api`), on `fig-kit` (re-exported as `fig_core::kit`); it carries no language | 18,400 |
| `fig-cli` | `rust/fig-cli` | `src/cli/*.zig` (the `fig` binary) and `src/lsp/main.zig` (`fig-lsp`) | 9,600 |
| `fig-conform` | `rust/fig-conform` | new: fig-conformance's JSON-lines adapter over `fig-core` | 1,000 |
| `fig-capi` | `rust/fig-capi` | the C ABI: every function in `bindings/c/include/fig.h`, as a static and shared library (`libfig`) and as the wasm module the TypeScript binding loads | 4,800 |
| `fig-abi` | `rust/fig-abi` | new: `fig.h`'s records, shared by the C ABI's crates | 600 |
| `fig-kit-capi` | `rust/fig-kit-capi` | the kit's `fig_kit_*` exports; built alone for wasm32 it is `fig-kit.wasm`, which the JavaScript kit answers through (`bindings/typescript/kit/rules.js`) | 300 |
| `fig-lang-export` | `rust/fig-lang-export` | one language as a library of its own: `libfig_<name>.a` with `fig_<name>_register`, or the wasm module its `@diaryx/fig-<name>` package inlines | 1,100 |
| `xtask` | `rust/xtask` | fig's own tasks, `cargo xtask <task>`: the pre-release gate (`ci`) and the release guards | 2,500 |

Every crate is a member of one Cargo workspace, the repository root's
`Cargo.toml`, with the published binding (`fig`, `fig-macros`, in
`bindings/rust/`), at fig's one version.
`fig-core`'s registry starts empty: the CLI, the conformance adapter and
`fig-capi`'s `bundle` feature register the ten language crates at startup
(`registry::register_bundle`), and the `fig` binding registers each from
its crate (`fig_yaml::register()`). A program that wants a language alone
takes its crate with `default-features = false`, and gets no `fig` binding.
`fig-sys`, its thirteen prebuilt payload crates and `fig-language-build`
are gone.

`src/deserialize.zig` was not ported: it reads a document into a Zig type
by reflection, which in Rust is serde's job, in the `fig` crate.

## How it was held to the Zig

This is the record of how the port was verified before it replaced the Zig.
The Zig is gone, so the comparisons can no longer be rerun; the suites that
stand on their own (fig-conformance, the edit fixtures, the probes, the
bindings' tests) still run, against the Rust alone, in `cargo xtask ci`.

Nothing below imported either implementation: each check runs both through
an interface they share and compares what comes out.

| Check | What it compares | Rust vs Zig |
|---|---|---|
| fig-conformance parse suites | parse, refuse, reprint and convert, against each format's own spec suite (yaml-test-suite, toml-test, JSONTestSuite, json5-tests, nestedtext_tests, fig's own) | every score equal to the Zig's (26 checks over 7 suites) |
| fig-conformance edit suites | 27,509 edits recorded from the Zig, exact bytes or refusal status | 27,509 / 27,509 |
| Edit fixtures (`languages/*/tests/edits.json`) | the Zig editor tests' own expectations, 753 cases | 753 / 753 |
| CLI differential (`parity/cli-diff.mjs`) | 21,546 command lines over 746 documents: exit status, stdout, the file written | 21,541 / 21,546 |
| LSP | an LSP session with diagnostics, byte for byte | identical |
| Helper wire | every line sent to and received from real helper processes | identical |
| C ABI | `nm` against `fig.h`; `tools/abi_probe.c`, `abi_probe.cpp`, `abi_runtime_probe.c`, `lang_archive_probe.c` linked against `fig-capi` | 113 / 113 symbols; every probe passes |
| TypeScript binding | its own test suite, on the Rust wasm in place of the Zig one | 117 / 117, as on the Zig wasm |
| Rust binding | `bindings/rust`'s own tests, with no Zig on PATH | 199 / 199 |

The five CLI commands that differ are ones the Zig binary panics on: four
`convert`s of a YAML document with complex keys to a flat format, where
both implementations reach the same `unreachable` in `flat_strip`, and one
`insert` the Rust answers with a usage error instead.

Each language was also checked on its own while it was ported, against
the Zig CLI or a Zig driver over the Zig sources, on the corpora above
plus generated and mutated documents (over 250,000 inputs in all):
parse/refuse, the error and its line and column, the reprint, and the
full node table. Where they differ, the Zig crashed.

## Reproducing

From the repository root:

```sh
cargo test --workspace --all-features
cargo build --release -p fig-cli -p fig-conform

# The edit fixtures:
node tools/edit-fixtures.mjs --command target/release/fig-conform

# fig-conformance (https://github.com/diaryx-org/fig-conformance), from its checkout:
node runner/edits.mjs --command <fig>/target/release/fig-conform
node runner/parse.mjs --command <fig>/target/release/fig-conform

# The CLI against another build of fig, such as a 6.x release (the Zig):
node rust/parity/cli-diff.mjs --reference <a 6.x fig> --candidate target/release/fig
```

`cargo xtask ci` runs the tests and the edit fixtures, and CI runs it on
every push.

## Zig bugs the port found

Differential testing turned up these in the Zig, none of them fixed in it
before it was removed (6.x releases still carry them):

- **TOML tokenizer reads past the end.** A string opener in the last two
  bytes of a file (`a = "`, `a = ''`) panics in `lexStringValue`, and
  `scanMultiLineString`'s closing check `self.i + 2 <= self.str.len` reads
  `self.str[self.i + 2]`.
- **A reversed slice in `flowIsMultiLine`.** A block mapping whose first
  key is a flow collection (`? [a, b]`) is sniffed as a flow mapping, and
  `source[last_end..span.end - 1]` is reversed: a Debug build panics, and
  the ReleaseSmall wasm reads past the end. The port matches what the
  shipped wasm answers.
- **`flat_strip` reaches `unreachable`** converting a YAML document with
  complex keys to INI, dotenv or `.properties`.
- **ZON char literals.** `'\xC3'` (a cut-off UTF-8 sequence) panics in
  std's `parseCharLiteral`; the port refuses it as `InvalidZon`.
- **figl by-name insert into a JSON-style map.** `insertValue(["p"], "y", 2)`
  on `p = { "x": 1 }` is refused with `ParseError`, where a raw insert lands.
