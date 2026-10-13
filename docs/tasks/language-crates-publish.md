```fig
title = The language crates can be published
part_of = [Tasks](/docs/tasks/tasks.md)
status = open
created = '2026-10-13'
description = fig-json, fig-yaml and the rest build their library from fig's Zig source found by walking up the checkout; to publish, each needs the source vendored (or prebuilt archives), a place in the release workflow after fig, and its prebuilt archives decided: in the crate for all seven targets, or payload crates
```

# The language crates can be published

Each language is a crate of its own (`bindings/rust/fig-<name>`, written by
`scripts/gen-language-crates.sh`) whose build script compiles
`libfig_<name>.a` with `zig build c-language-<name>`. Today that works in a
checkout only: the script finds fig's Zig source by walking up from the
crate, and a published crate has no checkout above it.

What publishing needs:

- **The Zig source, or archives.** Either vendor the source into each crate
  as fig-sys does (`zig build vendor-rust`, `zig/` in `include`), or ship
  prebuilt archives — and then decide where they go: inside each crate for
  all seven targets (measured at 0.4–0.8 MB a crate on the measure-crates
  branch), or a payload crate per language and target as fig-sys has now.
  Prebuilt archives also spare a consumer the Zig toolchain the crates need
  today.
- **The release workflow.** `release.yml` publishes the payload crates,
  `fig-sys`, `fig-macros` and `fig`; each language crate depends on `fig`,
  so it is published after it. `fig`'s dev-dependencies on them are by path
  alone, which `cargo publish` drops.
- **The licence text and a README** in each crate, as the others have.

## Done when

`cargo publish --dry-run -p fig-yaml` (and every other language) succeeds
from a clean checkout's vendored tree, the release workflow publishes them,
and a crate outside the repository depending on `fig` and `fig-yaml` builds.
