```fig
title = VERSIONING
description = Versioning policy for `fig`
author = adammharris
created = 2026-06-27
updated = 2026-09-26
part_of = [docs](docs.md)
```

# VERSIONING

`fig` follows SemVer strictly. This means, in practice, that **major releases are not sacred** and will be bumped even on small-size releases if it is functionally a major breaking change according to SemVer.

Therefore, starting with version v2.0.0, the `fig` project now has what some call an "epoch"—a "marketing" version that changes less often than major releases. Each is a fig cultivar: v2.0.0 is "Sierra," and v3.0.0 is "Texas Everbearing" — a core that bears languages without limit, through the runtime carrier and its Lua sister crate. The name lives in `rust/fig-cli/src/main.rs` (`EPOCH`) and is surfaced only by `fig version`; it has carried on through 5.0.0.


## One version, one tag

fig ships its artifacts off one tree, and from 5.0.0 **every one of them carries the same version**:

- the Rust crates — `fig-kit`, `fig-core`, `fig`, `fig-macros` and the ten language crates (`fig-json`, …), all members of the one Cargo workspace at the repository root, whose `[workspace.package] version` in `Cargo.toml` is where the version is decided
- the `fig` CLI binary
- the C library and its header (`bindings/c/include/fig.h`)
- `@diaryx/fig`, the npm library (`bindings/typescript/package.json`), and each language's `@diaryx/fig-<name>` (`languages/<name>/package.json`)
- `@diaryx/fig-wasi`, the npx-able CLI over WASI (`bindings/wasi/package.json`)

`fig version` prints that one number and the epoch: `fig 5.0.0 "Texas Everbearing"`.

A breaking change to any artifact is a major for all of them — a CLI flag's new default moves the crate's major, and an ABI break moves the npm package's. That is the cost, and it is paid knowingly: the Rust crate is the expensive case, since `dx deps fig --all` reaches most of the org and every major is a pin to propagate. The other side of it is that npm and crates.io will sometimes publish a version with nothing new in it, which is harmless. What it buys is that a version number means the same thing wherever it is read, and a release is one tag and one changelog section rather than four tracks whose cursors could strand an artifact that was left out.

The version is copied, never retyped. The root `Cargo.toml` decides it, and every crate of the workspace inherits it; `cargo xtask version-sync` (`rust/xtask/src/version.rs`) copies it into what inheritance does not reach: the workspace's internal `{ path, version }` pins, `fig.h`'s `FIG_VERSION_MAJOR`/`_MINOR`/`_PATCH`, `README.md`'s frontmatter, every `package.json` (the two bindings' and each language's) and their lockfiles, and the nushell plugin's pin on fig and its lockfile. `cargo xtask version-check`, part of `ship ci`, fails on any file that disagrees.

Before 5.0.0 the artifacts were versioned independently, each on its own track and tag prefix (`core/v…`, `cli/v…`, `rust/v…`, `npm/v…`), held together by a floor — no artifact below the core it embedded — and a release tool of fig's own. 5.0.0 sits above every one of those tracks (cli 4.0.1, rust 4.1.0, core 3.1.0, npm 3.1.0), so no artifact's version goes backwards. The prefixed tags stay as history; [CHANGELOG](CHANGELOG.md) keeps their sections, headed by the artifacts that moved. The reasoning is `docs/proposals/cli-5.md` §6.

## The C ABI contract version

`bindings/c/include/fig.h` also defines `FIG_ABI_VERSION` — a monotonic integer, distinct from the marketing version, that identifies the *binary shape* of the C ABI the way an ELF SONAME does. It is bumped **only on a breaking ABI change**. fig's forward-compat design (size-gated structs, decode-unknown enums, add-never-remove functions) makes additions non-breaking, so this number stays put across feature releases and moves only on a true break. The library reports it at runtime via `fig_abi_version()`, so a host that dynamically loads `libfig` can compare it against the `FIG_ABI_VERSION` it compiled with.

Source of truth: `ABI_VERSION` in `rust/fig-capi/src/lib.rs`. `cargo xtask abi-check` pins the `fig.h` macro to it; `cargo xtask semver-check` requires it to increment whenever the C ABI diff against the last release tag is breaking. It is a contract of its own and has no tag; neither `version-sync` nor `ship` touches it. (`semver-check` uses the most recent release tag — see "Release tagging" below — purely as a git revision to diff *against* via `git show <tag>:...`, so it doesn't care what the tag's number itself represents; only `ABI_VERSION` and the workspace version in the current tree matter to it.)

## Release tagging

One annotated tag, `v<version>`, releases everything. Pushing it sets off:

| Workflow | What it does |
|---|---|
| `release.yml` | publishes the Rust crates to crates.io, bottom-up |
| `release-npm.yml` | publishes `@diaryx/fig` |
| `release-npm-wasi.yml` | publishes `@diaryx/fig-wasi` |
| `homebrew.yml` | builds the macOS (native) and Linux (glibc) CLI binaries with cargo, attaches them to the GitHub release, and writes the Homebrew tap |
| `release-binaries.yml` | attaches the Windows x86_64 and WASI CLI binaries to the GitHub release |

Each publishing workflow checks that every file carries the one version (`cargo xtask version-check`) and that the tag names it, and no-ops where the registry already has that version. Until 7.0.0 the tag was also a release in itself, the one Zig programs `zig fetch`ed; fig has no Zig package now, and a Zig program links the C library like any C program.

**Finding the last release.** `semver-check` (in `ship ci`) diffs the C ABI against the most recent release tag reachable from HEAD. Three bare tags from before the per-artifact scheme — `v1.0.0`, `v2.0.0`, `v2.5.1` — are ancestors of every commit since, so a bare `--match 'v*'` would take `v2.5.1` for the latest release. The baseline is found instead with

```
git describe --tags --abbrev=0 --match 'v[5-9].*' --match 'v[1-9][0-9]*'
```

which admits v5 through v9 and every two-or-more-digit major, and leaves those three out. Until the first one-version tag exists, it falls back to its old prefixed line, `core/v*`. `.config/cliff.toml`'s `tag_pattern` makes the same cut for the changelog, keeping the prefixed tags as history.

**The Rust API has no automated guard.** fig's native Rust surface (the `fig` crate's types, its derive macros and serde impls, its cargo features) never crosses the C ABI, so `semver-check` cannot see it. `cargo-semver-checks` used to diff it in CI and before publishing; it was removed when the crates moved onto the Rust port, because which version a release is, is decided by its maintainer, not by a tool. A change to that surface is judged in review and named in its commit (`!`, `BREAKING CHANGE:`, `Behavioural-change:`).

## Releasing

fig releases with [ship](https://github.com/diaryx-org/ship), configured by `.config/ship.figl`. **Which version a release is, is Adam's to name**; `ship` with no spec proposes and writes nothing:

```
ship
```

It prints what `patch`, `minor`, `major` and `as-is` would each move to, and what the commits since the last tag say about the promise — the `!` subjects, `BREAKING CHANGE:` footers, and every `Behavioural-change:` trailer in full. Then, once the number has been said:

```
ship <patch|minor|major|x.y.z|as-is>
```

It stops before the push. The steps, which are also how to do it by hand:

1. **Preflight.** Refuse a release that is already doomed, before anything is written: a dirty tree, a branch that isn't `main`, a `main` behind `origin/main`, a tag that already exists, a crate version crates.io already has, no `git-cliff` on PATH.
2. **Bump.** The root `Cargo.toml`'s `[workspace.package] version`, then `cargo xtask version-sync` (the `post_bump`) for every other file that carries the version, then the lockfile. `as-is` skips this and releases the version already in the tree — the way to release a version bumped in an earlier commit without spending a second number on it.
3. **Verify** with `ship ci`, the `job` list in `.config/ship.figl` (the workspace's tests + version-check + vendor-check + check-figl + abi-check + semver-check + the CLI checks + the edit fixtures + the binding suites) — all green. This runs *after* the bump: `semver-check` and `version-check` judge the versions in the tree, so running them first would judge the versions the release is replacing. A failure restores the tree.
4. **Cut the changelog entry.** The generated region under [CHANGELOG](CHANGELOG.md)'s `## Unreleased` is rendered once more through `.config/cliff.toml` and written as a new `## v<version> — <date>` section directly below the end marker; the region is reset to empty. Check the **Behavioural changes** section covers what an unedited caller will notice — it is gathered from `Behavioural-change:` trailers, so a missing one means a commit didn't carry it — and triage anything in **Uncategorised**. `ship changelog` prints the region, `ship changelog --write` regenerates it, `ship changelog --check` fails if it is stale.
5. **Commit** the version files and the changelog, and nothing else, as `chore: bump to <version>` (or `chore: release <version>` for `as-is`) — subjects `.config/cliff.toml` skips.
6. **Tag** `v<version>`, annotated.
7. **Push** the branch, then the tag, and write the GitHub release body from the changelog section with `ship github v<version>` — the three commands `ship` prints. `--push` runs them, and is Adam's to give.

If the core had a **breaking** ABI change, bump `FIG_ABI_VERSION` (`ABI_VERSION` in `rust/fig-capi/src/lib.rs`, and the macro in `fig.h`) by hand before releasing — it's a deliberate ABI-contract decision `semver-check` guards, not a marketing version.

## Known gaps

- **No automated TypeScript API guard.** The C-ABI integer has no TS analog (npm exposes no C ABI). An automated TS API diff (e.g. an `api-extractor` report committed to git) is an optional follow-up; once it lands, it should baseline against the last release tag the same way `semver-check` does.
- **No `publish` command.** `release.yml`'s crate job inlines its own bottom-up
  `cargo publish` loop over the workspace, so the workflow knows the crate list
  rather than asking the repo for it, and a run that dies halfway is finished by
  re-running the workflow rather than a local command. prov's `cargo xtask
  publish` is the shape to copy if that ever needs a manual recovery path.
- **`ship` reads the three bare tags.** `ship` finds the last release with `git tag --list 'v[0-9]*'`, which takes in `v1.0.0`, `v2.0.0` and `v2.5.1`. Until `v5.0.0` is tagged, its proposal therefore says the version sits "above v2.5.1" and lists every commit since that tag; its candidates are still right, and `as-is` releases 5.0.0. Once `v5.0.0` exists it is the highest tag and the proposal reads correctly.
