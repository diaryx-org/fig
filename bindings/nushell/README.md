# `nu_plugin_fig`

A [nushell](https://www.nushell.sh) plugin for `.figl`, the fig authoring dialect.

```nu
> cargo install --path bindings/nushell
> plugin add ~/.cargo/bin/nu_plugin_fig
> plugin use fig

> open figl/homebrew.figl | get on
╭───────────────────┬──────────────────╮
│ push              │ {record 1 field} │
│ workflow_dispatch │                  │
╰───────────────────┴──────────────────╯
```

Two commands, `from figl` and `to figl`. Registering the first is also what
teaches `open` about the format: nushell dispatches `open foo.figl` to whatever
`from figl` is in scope, so no further setup is needed.

## Why a plugin and not a one-line `def`

Because nushell already dispatches `open` to any `from figl` in scope, a wrapper
over the CLI gets you most of the way in a single line:

```nu
def "from figl" []: [string -> any] { $in | fig get - -i fig -o json | from json }
```

What it cannot do is carry everything, because a carrier format only forwards
what *it* can represent, and no format fig emits covers figl's types:

| via | `null` | datetime |
|---|---|---|
| `-o json` | `nothing` | **string** — JSON has no date type |
| `-o toml` | **key dropped entirely** — TOML has no null | `datetime` |
| `-o yaml` | `nothing` | **string** — nushell's `from yaml` does not resolve timestamps |

The TOML row is not theoretical: `figl/homebrew.figl` and
`figl/release-binaries.figl` in this repo both set `on.workflow_dispatch = null`,
and converting them through TOML makes that key vanish rather than arrive empty.

This plugin reads `fig::Value` directly through the native Rust binding, so both
land correctly in one pass with no carrier in between.

## What maps to what

| figl | nushell |
|---|---|
| `null` | `nothing` |
| bool, string, int, float | the same |
| integer above `i64::MAX` | `string` — nushell's `int` is an `i64`, and keeping the digits beats rounding them |
| `2026-05-08` | `datetime` at midnight **UTC** |
| `2026-05-08T07:32:00` | `datetime`, read as **UTC** |
| `2026-05-08T07:32:00-06:00` | `datetime` at that offset |
| `10:30` (bare clock time) | `string` — nushell has no time-of-day type, and `duration` means something else |
| `Infinity` / `NaN` | `float` |
| enum and char literals | `string` |
| list, map | `list`, `record` |

The two UTC assumptions match what nushell's own `from toml` does with the same
spellings, on the grounds that agreeing with the shell beats being novel.

## Comments are not carried

A nushell record has nowhere to put a comment, so `from figl` drops them — as
`from yaml` and `from toml` do, and as any `from` must. It follows that
`open x.figl | update port 8080 | to figl | save x.figl` writes back a file
stripped of every comment it had.

Preserving comments across an edit is the entire point of fig, so for that use
the CLI, which rewrites in place and leaves every other byte alone:

```nu
> fig set config.figl service.replicas 5
> fig comment --inline config.figl service.replicas "bumped for Black Friday"
```

## Versioning

**A plugin binary only works with the nushell minor it was built against.** The
plugin protocol is versioned with the shell, so a binary built for 0.115 will be
refused by 0.116 at `plugin add` time. The nu dependencies are pinned with `=`
rather than `^` for that reason — a caret range would let `cargo update` quietly
produce a binary the target shell rejects.

Upgrading nushell therefore means bumping the three `=0.x.y` pins in
`Cargo.toml`, rebuilding, and running `plugin add` again.

This crate is versioned and released independently of fig's other artifacts (see
[VERSIONING](../../docs/VERSIONING.md)) — it moves on nushell's cadence, not
fig's. It is likewise a standalone cargo workspace, deliberately not a member of
`bindings/rust`: the nu dependency tree has no business in that workspace's
`cargo test --workspace` or in cargo-semver-checks' run over the published fig
surface.

## Building

Needs no Zig toolchain on a tier-1 target — `fig-sys` links a prebuilt
`libfig.a` for the default language set. Anywhere else it builds the core from
the vendored Zig source, which does.

```nu
> cargo test          # 10 behavioural tests, in-process via nu-plugin-test-support
> cargo clippy --all-targets
```

## Known limitations

- **Parse errors point at the whole input, not the offending byte.** fig's
  `ParseError` has `line`/`column`/`byte_offset` fields, but the core does not
  yet fill them through the C ABI. When it does, this plugin gets precise
  labels for free.
- **No editing commands.** `fig set` / `fig comment` are the lossless way to
  change a file, and they are not wrapped here yet.
