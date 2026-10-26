```fig
title = check-figl runs a fig that cargo did not build
part_of = [Tasks](/docs/tasks/tasks.md)
status = open
description = cargo xtask check-figl builds fig where cargo is told to and then runs target/debug/fig, so with CARGO_TARGET_DIR set it fails on every mapping, or checks with a stale fig left there
created = 2026-10-26
updated = 2026-10-26
```

# check-figl runs a fig that cargo did not build

**Where.** `rust/xtask/src/figl.rs`, `run`, behind both `sync-figl` and
`check-figl`. It builds the CLI with `cargo build -p fig-cli --bin fig`,
which writes to `$CARGO_TARGET_DIR` when that is set, and then runs
`root.join("target/debug/fig")` whatever cargo was told. `abi.rs` already
asks where cargo builds, in `target_dir` (`$CARGO_TARGET_DIR`, else the
workspace's `target/`); `figl.rs` does not use it.

**Repro.** At 5700185, in a checkout with no `target/debug/fig`:

```sh
CARGO_TARGET_DIR=$(mktemp -d) cargo xtask check-figl
# · cargo build --quiet -p fig-cli --bin fig
# sync-figl: failed to run `fig get -o yaml …/figl/ci.figl`: No such file or directory (os error 2)
# … the same for every mapping; exit 1
```

The binary it built is at `$CARGO_TARGET_DIR/debug/fig`. Without
`CARGO_TARGET_DIR` the same command passes. A `build.target-dir` in a cargo
config redirects the build the same way, and `abi.rs`'s `target_dir` does not
see that either.

**What it costs.** The failure is the mild half: the `figl` job in
`.config/ship.figl` cannot run under a redirected target directory. The worse
half is a checkout that does have a `target/debug/fig` from an earlier build:
the check then generates the workflows with that older fig, reports them in
sync or drifted by its answers rather than the current source's, and says
nothing about it.

**Done when** `sync-figl` and `check-figl` run the fig their own
`cargo build` just wrote, with and without `CARGO_TARGET_DIR`, and the repro
above passes.
