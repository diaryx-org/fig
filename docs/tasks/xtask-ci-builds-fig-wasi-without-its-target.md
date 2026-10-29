```fig
title = ship ci builds fig-wasi without the wasm32-wasip1 target
part_of = [Tasks](/docs/tasks/tasks.md)
status = open
description = cargo xtask npm-test wasi skips only for a missing npm, a Node older than 24 or missing node_modules, which fig-wasi never has; the check job's runner now has Node 24 and no wasm32-wasip1, so the job fails on main
created = 2026-10-28
updated = 2026-10-28
```

# ship ci builds fig-wasi without the wasm32-wasip1 target

**Where.** `rust/xtask/src/npm.rs`, `npm_missing`, behind the `wasi` job
in `.config/ship.figl`. It skips the suite, with a note, when npm is
missing, when Node is older than 24, or when a package that declares
dependencies has no `node_modules`. fig-wasi declares none, so the last
check never applies to it. Its `npm run build` runs
`scripts/build-wasm.mjs`, which builds `fig-cli` for `wasm32-wasip1`, and
nothing asks whether that target is installed.

The `check` job in `.github/workflows/ci.yml` (`Rust — cargo xtask ci`)
installs stable with the host target alone and runs `ship ci`. Its comment
says the TypeScript and WASI suites "skip with a note here, with no
node_modules installed". The TypeScript suite does. The WASI suite skipped
only because the runner's Node was older than 24. In run 40445910604, on
3b4c39f, the job passed with `wasi tests: Node <24 — skipping`. In run
40762269186, on 2ea24aa, the runner has Node 24. The TypeScript suite
skips for `node_modules`. fig-wasi goes on to build and fails:

```
$ cargo xtask npm-test wasi
· cargo build -p fig-cli --bin fig --profile wasm --target wasm32-wasip1 --features wasi-host
error[E0463]: can't find crate for `std`
  = note: the `wasm32-wasip1` target may not be installed
xtask npm-test: `npm run build` failed (exit status: 1)
error: `cargo xtask npm-test wasi` failed (exit 1)
```

The `fig-wasi — build + test` job, which installs the target, passes in
the same run.

**Repro.** At 2ea24aa, with Node 24 or later on `PATH` and a toolchain
that has no `wasm32-wasip1` (here 1.98.1, which has the host target
alone):

```sh
RUSTUP_TOOLCHAIN=1.98.1 cargo xtask npm-test wasi    # exit 1, E0463 as above
```

With the target installed, the same command builds and runs the suite.

**What it costs.** `CI / Rust — cargo xtask ci` is red on `main` for every
push from now on, whatever the push changes. A contributor who has Node 24
but has not added the wasm target gets a failing `ship ci` where the
module docs promise a note.

**Done when** `ship ci` passes on a runner with Node 24 or later, no
`node_modules` and no `wasm32-wasip1` target. Either the suite skips with a
note, or the job installs the target and runs it. The repro above passes,
and the `check` job's comment says which of the two it does.
