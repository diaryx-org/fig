```fig
title = The language crates can be published
part_of = [Tasks](/docs/tasks/tasks.md)
status = open
created = '2026-10-13'
description = fig-json, fig-yaml and the rest carry their language's Zig, but its package reaches the kit by path (../../kit), which a published crate does not have; to publish, each needs the kit vendored or fetched (or prebuilt archives), and a place in the release workflow after fig
```

# The language crates can be published

Each language's crate is its Zig package too (`languages/<name>/`: a
`Cargo.toml` beside its `build.zig`), whose build script compiles
`libfig_<name>.a` with `zig build c` through `fig-language-build`. The
package's own source, header and build files are in the crate
(`cargo package -p fig-yaml --list`), but its `build.zig.zon` reaches the kit
by path, `../../kit`, which a published crate has nothing at. plist's also
names `../json`, for its tests alone.

What publishing needs:

- **The kit, or archives.** Either the crate carries the kit (copied in at
  publish time, as `vendor-rust` does for fig-sys, with the path in the
  packaged `build.zig.zon` rewritten to it), or the kit is a package fetched
  by URL and hash from a tagged release, or the crate ships prebuilt
  archives — and then decide where they go: inside each crate for all seven
  targets (measured at 0.4–0.8 MB a crate on the measure-crates branch), or
  a payload crate per language and target as fig-sys has. Prebuilt archives
  also spare a consumer the Zig toolchain the crates need today.
- **plist's test dependency on JSON**, which must not be fetched to build
  plist's library: lazy, or its tests rewritten to need no JSON.
- **The release workflow.** `release.yml` publishes the payload crates,
  `fig-sys`, `fig-macros` and `fig`; `fig-language-build` goes before the
  languages, and each language after `fig`, which it depends on. `fig`'s
  dev-dependencies on them are by path alone, which `cargo publish` drops.
- **A README** in each crate, as `fig` has.

## Done when

`cargo publish --dry-run -p fig-yaml` (and every other language) succeeds
from a clean checkout, the release workflow publishes them, and a crate
outside the repository depending on `fig` and `fig-yaml` builds.
