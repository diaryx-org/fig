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

Therefore, starting with version v2.0.0, the `fig` project now has what some call an "epoch"—a "marketing" version that changes less often than major releases. Each is a fig cultivar: v2.0.0 is "Sierra," and v3.0.0 is "Texas Everbearing" — a core that bears languages without limit, through the runtime carrier and its Lua sister crate. The name lives in `build.zig` (`epoch`) and is surfaced only by `fig version`; it has carried on through 5.0.0.


## One version, one tag

fig ships five artifacts off one tree, and from 5.0.0 **every one of them carries the same version**:

- the Zig core + C ABI — `.version` in `build.zig.zon`, where the version is decided
- the `fig` CLI binary
- the Rust crates — `fig`, `fig-macros`, `fig-sys` and the seven `fig-sys-<target>` payload crates (`bindings/rust/Cargo.toml`)
- `@diaryx/fig`, the npm library (`bindings/typescript/package.json`)
- `@diaryx/fig-wasi`, the npx-able CLI over WASI (`bindings/wasi/package.json`)

`fig version` prints that one number and the epoch: `fig 5.0.0 "Texas Everbearing"`.

A breaking change to any artifact is a major for all of them — a CLI flag's new default moves the crate's major, and an ABI break moves the npm package's. That is the cost, and it is paid knowingly: the Rust crate is the expensive case, since `dx deps fig --all` reaches most of the org and every major is a pin to propagate. The other side of it is that npm and crates.io will sometimes publish a version with nothing new in it, which is harmless. What it buys is that a version number means the same thing wherever it is read, and a release is one tag and one changelog section rather than four tracks whose cursors could strand an artifact that was left out.

The version is copied, never retyped. `build.zig.zon` decides it; `zig build version-sync` (`tools/version-sync.zig`) copies it into `figl/build.zig.figl` (the source `build.zig.zon` is generated from), `fig.h`'s `FIG_VERSION_MAJOR`/`_MINOR`/`_PATCH`, `README.md`'s frontmatter, the Rust workspace and its internal pins, and both `package.json` files and their lockfiles. `zig build version-check`, part of `zig build check`, fails on any file that disagrees.

Before 5.0.0 the artifacts were versioned independently, each on its own track and tag prefix (`core/v…`, `cli/v…`, `rust/v…`, `npm/v…`), held together by a floor — no artifact below the core it embedded — and a release tool of fig's own. 5.0.0 sits above every one of those tracks (cli 4.0.1, rust 4.1.0, core 3.1.0, npm 3.1.0), so no artifact's version goes backwards. The prefixed tags stay as history; [CHANGELOG](CHANGELOG.md) keeps their sections, headed by the artifacts that moved. The reasoning is `docs/proposals/cli-5.md` §6.

## The C ABI contract version

`bindings/c/include/fig.h` also defines `FIG_ABI_VERSION` — a monotonic integer, distinct from the marketing version, that identifies the *binary shape* of the C ABI the way an ELF SONAME does. It is bumped **only on a breaking ABI change**. fig's forward-compat design (size-gated structs, decode-unknown enums, add-never-remove functions) makes additions non-breaking, so this number stays put across feature releases and moves only on a true break. The library reports it at runtime via `fig_abi_version()`, so a host that dynamically loads `libfig` can compare it against the `FIG_ABI_VERSION` it compiled with.

Source of truth: `abi_version` in `build.zig`. `zig build abi-check` pins the `fig.h` macro to it; `zig build semver-check` requires it to increment whenever the C ABI diff against the last release tag is breaking. It is a contract of its own and has no tag; neither `version-sync` nor `dx release` touches it. (`semver-check` uses the most recent release tag — see "Release tagging" below — purely as a git revision to diff *against* via `git show <tag>:...`, so it doesn't care what the tag's number itself represents; only `abi_version`/`.version` in the current tree matter to it.)

## Release tagging

One annotated tag, `v<version>`, releases everything. Pushing it sets off:

| Workflow | What it does |
|---|---|
| `release.yml` | publishes the Rust crates to crates.io, bottom-up |
| `release-npm.yml` | publishes `@diaryx/fig` |
| `release-npm-wasi.yml` | publishes `@diaryx/fig-wasi` |
| `homebrew.yml` | builds the macOS and Linux CLI binaries, attaches them to the GitHub release, and writes the Homebrew tap |
| `release-binaries.yml` | attaches the Windows and WASI CLI binaries (and `build_options.zig`) to the GitHub release |
| `.tangled/workflows/release.yml` | the same binaries, published as artifacts against the tag on the tangled.org mirror |

The tag is also what Zig consumers `zig fetch`: Zig has no registry, so pushing the tag *is* the Zig release. Each publishing workflow checks that every file carries the one version (`zig build version-check`) and that the tag names it, and no-ops where the registry already has that version.

**Finding the last release.** `semver-check` and both `cargo-semver-checks` runs (in `zig build check`, in `ci.yml`, and in `release.yml` before publishing) diff against the most recent release tag reachable from HEAD. Three bare tags from before the per-artifact scheme — `v1.0.0`, `v2.0.0`, `v2.5.1` — are ancestors of every commit since, so a bare `--match 'v*'` would take `v2.5.1` for the latest release. The baseline is found instead with

```
git describe --tags --abbrev=0 --match 'v[5-9].*' --match 'v[1-9][0-9]*'
```

which admits v5 through v9 and every two-or-more-digit major, and leaves those three out. Until the first one-version tag exists, each falls back to its old prefixed line: `core/v*` for `semver-check`, `rust/v*` for `cargo-semver-checks`. `.config/cliff.toml`'s `tag_pattern` makes the same cut for the changelog, keeping the prefixed tags as history.

**After a Zig bump.** A release tag's Zig source builds only with the Zig it was pinned to, and `cargo semver-checks --baseline-rev` builds the baseline crate from that source. So on the first release after `minimum_zig_version` moves, every `cargo-semver-checks` run compares the tag's `build.zig.zon` pin with the tree's and, where they differ, takes the baseline from crates.io instead (`--baseline-version`, the version the tag names) with `--default-features`: the default language set links the prebuilt payload crate and needs no Zig. That run says so in its output. What it leaves unchecked until the next release is the opt-in surface — `serde`, `derive`, `indexmap`, and the `zon`/`plist` languages — so a change behind those features in that window gets its SemVer judgement from review rather than the tool.

## Releasing

fig releases through the org's shared tooling, `dx release`, configured by `.config/release.toml`. **Which version a release is, is Adam's to name**; `dx release` with no spec proposes and writes nothing:

```
dx release
```

It prints what `patch`, `minor`, `major` and `as-is` would each move to, and what the commits since the last tag say about the promise — the `!` subjects, `BREAKING CHANGE:` footers, and every `Behavioural-change:` trailer in full. Then, once the number has been said:

```
dx release <patch|minor|major|x.y.z|as-is>
```

It stops before the push. The steps, which are also how to do it by hand:

1. **Preflight.** Refuse a release that is already doomed, before anything is written: a dirty tree, a branch that isn't `main`, a `main` behind `origin/main`, a tag that already exists, a crate version crates.io already has, no `git-cliff` on PATH.
2. **Bump.** `build.zig.zon`'s `.version`, then `bindings/rust/Cargo.toml` and its internal pins, then `zig build version-sync` (the `post_bump`) for every other file that carries the version, then the Rust lockfile. `as-is` skips this and releases the version already in the tree — the way to release a version bumped in an earlier commit without spending a second number on it.
3. **Verify** with `zig build check` (test + conformance + abi-check + semver-check + version-check + check-figl + cargo-semver-checks + the binding suites) — all green. This runs *after* the bump: `semver-check` and `version-check` judge the versions in the tree, so running them first would judge the versions the release is replacing. A failure restores the tree.
4. **Cut the changelog entry.** The generated region under [CHANGELOG](CHANGELOG.md)'s `## Unreleased` is rendered once more through `.config/cliff.toml` and written as a new `## v<version> — <date>` section directly below the end marker; the region is reset to empty. Check the **Behavioural changes** section covers what an unedited caller will notice — it is gathered from `Behavioural-change:` trailers, so a missing one means a commit didn't carry it — and triage anything in **Uncategorised**. `dx changelog` prints the region, `dx changelog --write` regenerates it, `dx changelog --check` fails if it is stale.
5. **Commit** the version files and the changelog, and nothing else, as `chore: bump to <version>` (or `chore: release <version>` for `as-is`) — subjects `.config/cliff.toml` skips.
6. **Tag** `v<version>`, annotated.
7. **Push** the branch, then the tag, and write the GitHub release body from the changelog section with `dx github-release v<version>` — the three commands `dx release` prints. `--push` runs them, and is Adam's to give.

If the core had a **breaking** ABI change, bump `FIG_ABI_VERSION` (`abi_version` in `build.zig`) by hand before releasing — it's a deliberate ABI-contract decision `semver-check` guards, not a marketing version.

## Known gaps

- **No automated TypeScript API guard.** There is no turnkey `cargo-semver-checks` equivalent for the TS public surface, and the C-ABI integer has no TS analog (npm exposes no C ABI). An automated TS API diff (e.g. an `api-extractor` report committed to git) is an optional follow-up; once it lands, it should baseline against the last release tag the same way the other two do.
- **No `publish` command.** `release.yml`'s crate job inlines its own bottom-up
  `cargo publish` loop over the workspace, so the workflow knows the crate list
  rather than asking the repo for it, and a run that dies halfway is finished by
  re-running the workflow rather than a local command. prov's `cargo xtask
  publish` is the shape to copy if that ever needs a manual recovery path.
- **`dx` reads the three bare tags.** `dx release` finds the last release with `git tag --list 'v[0-9]*'`, which takes in `v1.0.0`, `v2.0.0` and `v2.5.1`. Until `v5.0.0` is tagged, its proposal therefore says the version sits "above v2.5.1" and lists every commit since that tag; its candidates are still right, and `as-is` releases 5.0.0. Once `v5.0.0` exists it is the highest tag and the proposal reads correctly.
