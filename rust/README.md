```fig
title = fig in Rust
part_of = [fig docs](/docs/docs.md)
description = The Rust port of fig's core, kit, languages, editor and CLI, and how it is held to the Zig implementation
author = adammharris
created = 2026-10-15
updated = 2026-10-15
```

# fig in Rust

This directory is a port of fig from Zig to Rust, following
[the proposal](/docs/proposals/rust-core.md). The port is file for file:
every `.rs` file starts with `//! Port of <zig path>`, keeps the Zig
file's names and doc comments, and does what it does, byte for byte. It
needs no Zig and no C to build, and uses no crates outside `std`.

| Crate | Ports | Lines |
|---|---|---|
| `fig-core` | `kit/` (the node table, AST, `Document`, the `Language` trait, the shared rules), `languages/*/src/` (all ten), `src/editor*`, `src/serialize.zig`, `src/canonical/`, `src/materialize.zig`, `src/lossless.zig`, `src/flat_strip.zig`, `src/diagnostics.zig`, `src/patch.zig`, `src/contract/` (the registry, the helper wire, the host), and the logic of `src/c_api.zig` (`api`) | 66,000 |
| `fig-cli` | `src/cli/*.zig` (the `fig` binary) and `src/lsp/main.zig` (`fig-lsp`) | 9,600 |
| `fig-conform` | new: fig-conformance's JSON-lines adapter over `fig-core` | 1,000 |

`src/deserialize.zig` is not ported: it reads a document into a Zig type
by reflection, which in Rust is serde's job, in the `fig` crate.

## How it is held to the Zig

Nothing below imports either implementation: each check runs both through
an interface they share and compares what comes out.

| Check | What it compares | Rust vs Zig |
|---|---|---|
| fig-conformance parse suites | parse, refuse, reprint and convert, against each format's own spec suite (yaml-test-suite, toml-test, JSONTestSuite, json5-tests, nestedtext_tests, fig's own) | every score equal to the Zig's (26 checks over 7 suites) |
| fig-conformance edit suites | 27,509 edits recorded from the Zig, exact bytes or refusal status | 27,509 / 27,509 |
| Edit fixtures (`languages/*/tests/edits.json`) | the Zig editor tests' own expectations, 753 cases | 753 / 753 |
| CLI differential (`parity/cli-diff.mjs`) | 21,546 command lines over 746 documents: exit status, stdout, the file written | 21,541 / 21,546 |
| LSP | an LSP session with diagnostics, byte for byte | identical |
| Helper wire | every line sent to and received from real helper processes | identical |

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

```sh
cargo test --workspace                       # 949 tests
cargo build --release

# The edit fixtures, from the repository root:
node tools/edit-fixtures.mjs --command rust/target/release/fig-conform

# fig-conformance (https://github.com/diaryx-org/fig-conformance), from its checkout:
node runner/edits.mjs --command <fig>/rust/target/release/fig-conform
node runner/parse.mjs --command <fig>/rust/target/release/fig-conform

# The CLI against a Zig build of fig (`zig build -Doptimize=ReleaseSafe`):
node rust/parity/cli-diff.mjs --reference zig-out/bin/fig --candidate rust/target/release/fig
```

CI's `rust-port` job runs the tests and the edit fixtures on every push,
with no Zig installed.

## Zig bugs the port found

Differential testing turned up these in the Zig, none of them fixed here:

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
