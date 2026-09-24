```fig
title = CLI 5
description = One reading of a value argument, a scalar printed as its text, strict arguments, one version and one tag — the command-line changes a fig 5.0 would carry
created = 2026-09-22
status = draft
updated = 2026-09-24
part_of = [proposals](proposals.md)
```

# CLI 5

> **Status: DRAFT.** Written against `main` at 45b6b10 (core 3.1.0 · npm
> 3.1.0), after a pass over every action's help and behaviour. The bugs that
> pass found — `insert` writing a duplicate key, `set --embed` adding a second
> frontmatter block, YAML errors with no location, log prefixes in user
> output, help text that disagrees with the parser — are fixed as ordinary
> commits and are not argued here. What is left changes what an existing
> command line does, which is what a major version is for.
>
> **Progress.** §1 (a value argument is a fig value), §2 (`get` prints a
> scalar as its text), §3 (strict arguments), §4 (a missing comment exits
> 1) and §5 (exit codes written down) have landed, with
> `Behavioural-change:` trailers; `tools/cli-args-check.sh` holds them to
> the built binary, §5 as one case per action per row. The line §5 draws:
> 2 is what the command line alone gets wrong, before the file is read; 1
> is everything the document decides. `patch --dry-run`/`--diff` stay 0
> on a change, as their help always said — a patch is expected to change
> the file, where `fmt --dry-run` is a check that it need not. Two details §1
> left open were settled in the code: an argument that is empty, holds a
> line break or space at either end, or whose `#` would start a comment is
> a string as written;
> and a value whose natural layout cannot land where it is going (a YAML
> block inside a flow mapping) is written on one line instead. A TOML
> table given as a value is written as an inline table, which also fixed
> `fig patch` of a new table into TOML.

The CLI's compatibility contract is its own — flags, defaults, exit codes
([VERSIONING](/docs/VERSIONING.md)) — and it has drifted in ways no single
commit chose. The same command means different things in different formats,
the same question gets differently shaped answers, and a mistyped flag is read
as a file. Each section below is one change, the case for it, and what breaks.

## 1. A value argument is a fig value, in every format

`edit`, `set` and `insert` take a value, and today how that text is read
depends on the file it lands in:

| `fig set f v …` | JSON | YAML | TOML |
|---|---|---|---|
| `5` | `"5"` | `5` | `5` |
| `hello` | `"hello"` | `hello` | refused |
| `null` | `"null"` | `null` | refused |
| `[1,2]` | `"[1,2]"` | `[1,2]` | `[1,2]` |
| `'"q"'` | refused | `"q"` | `"q"` |

JSON and JSON5 quote every argument as a string (`Syntax.splice =
.json_string`), so the CLI cannot write a number, a boolean, a null or an array
into a JSON file at all. Every other format splices the argument verbatim, so
TOML refuses a bare word and YAML takes `a: b` as whatever YAML makes of it. A
script that sets a version number has to know the target format to know how to
quote it — the one thing fig exists to spare it.

The bindings already answered this. A Rust or TypeScript editor is handed a
`Value`, and fig renders it into the target as splice text (`value_text`,
`FigSerializeOptions.splice`); the format decides the spelling, never the
meaning. The CLI should do the same: **read the argument as a value in fig's
own dialect, then render it into the target the way a binding's value is
rendered.** fig's bare-literal rules are already the engine's (`literalOf`,
the `.figl` classifier `sniffBare`), and the dialect has flow collections:

- `5`, `2.5`, `true`, `null`, `2026-09-22` are typed, in every format;
- `hello`, `hello world`, `Yes`, `007` are strings — `Yes` and `007` with the
  lint the dialect already gives them;
- `[1, 2]` and `{a = 1, b = [x]}` are structures, rendered in the target's
  syntax (a YAML block, a TOML inline table, a JSON object);
- `'"5"'` is the string `5`.

Two flags cover what the rule cannot:

- `--string` takes the argument as a string whatever it looks like, so
  `fig set f version --string 1.10` needs no nested shell quoting.
- `--raw` splices the argument verbatim as source text in the target format,
  reparsed in place — today's YAML/TOML behaviour, kept for the text only the
  target can spell (a YAML anchor, a TOML local datetime, a ZON enum literal).

**What breaks.** JSON: a bare number, boolean, null or bracketed argument
becomes that value instead of a string; a bare word is still a string.
YAML/TOML/ZON: an argument that fig reads differently from the target — `a: b`,
`Yes`, `007`, `0x1A`, `'q'` — is now a fig value (a string, mostly), where it
was the target's; `--raw` restores the old reading. TOML: a bare word is
written as a string rather than refused.

## 2. `get` prints a scalar as its text

`fig get f name`, for the string `hi`, prints `hi` from YAML, `"hi"` from TOML,
and `"hi"` with no trailing newline from JSON; a JSON container does end in a
newline. `$(fig get f name)` is the commonest thing anyone does with fig, and
its answer depends on the file.

Proposed: **a scalar at `<path>` prints as its text and a newline**, the way
`jq -r` and `yq` do — a string unquoted, a number, boolean or null as fig
spells it (`42`, `true`, `null`), a datetime as written. A container prints as
the document it is, as now, always ending in a newline. `-o <format>`, given
explicitly, keeps asking for that format's spelling of the fragment
(`fig get f name -o json` → `"hi"`), so the quoted form is one flag away and
means the same thing in every format.

**What breaks.** Scripts that strip the quotes themselves (`| tr -d '"'`,
`jq -r` over `fig get` output) keep working; ones that relied on the quotes, or
on the missing newline, do not.

## 3. Arguments are parsed strictly

Today `fig get --bogus f.yaml` reports `no such file: --bogus`,
`fig set --bogus f.yaml a 1` tries to *create* a file named `--bogus`, and
`fig get f.yaml a b` ignores `b` and exits 0. An unknown flag is read as a
positional and a surplus positional is dropped.

Proposed: an argument beginning with `-` that is not a flag of that action is
a usage error (exit 2) naming it; a positional past the action's last is a
usage error; `--` ends the flags, so a file really named `-x` is `fig get -- -x`,
and `-` alone stays stdin.

**What breaks.** Command lines that were silently wrong now fail, loudly —
which is the point, but it is still a script that stops working.

## 4. A missing comment is a missing path

`fig comment --get f path` with no comment there prints a blank line and exits
0, where `fig get` on a missing path exits 1. Proposed: exit 1 with nothing on
stdout, so "is there a comment" is `if fig comment --get …`. (`--delete` of a
missing comment stays a no-op: deleting what is absent is not an error for
`patch --delete` either.)

## 5. Exit codes are written down

VERSIONING names exit codes as part of the CLI's contract, and nothing states
them — and the codes in use disagree. A file that does not parse exits 2 from
`get` and `fmt` but 1 from `check` and `set`, so the same broken file is a
"wrong command line" to one action and a "failed operation" to the next.
Proposed as the contract:

| code | meaning |
|---|---|
| 0 | done (for `fmt --dry-run`/`--diff` and `check`: nothing to change, everything parses) |
| 1 | the operation failed on the document: a parse error, a missing path, a refused edit, `fmt --dry-run` finding a change |
| 2 | the command line is wrong: unknown action or flag, missing or surplus argument, a value that is not a value |

A parse error is a failure on the document, so `get`, `fmt` and `convert` move
from 2 to 1. `fig --help` carries the table, and the CLI tests hold each row to
at least one case per action.

## 6. One version, one tag

fig releases four artifacts — core, CLI, Rust crate, npm package — each with
its own version and tag prefix, and a changelog cursor at "the newest tag on
any track". A release that names some artifacts consumes every commit since
the last tag, so an artifact left out can never catch up: `zig build release
-- rust minor` after `core minor npm minor` finds an empty changelog and
refuses. The four tracks exist so an artifact need not move when it did not
change, and the price has been a release tool, a floor checker and a version
setter that only fig has, and a release that is easy to get wrong.

Proposed: **one version for all four artifacts, one `v*` tag**, starting at
5.0.0 (above every current artifact: cli 4.0.1, rust 4.0.0, core 3.1.0, npm
3.1.0). A tag releases everything; `fig version` prints one number and the
codename. `abi_version` stays what it is, a separate integer for the C ABI,
because that is a contract of its own and it does not need a tag.

- The four `*/v*` workflow triggers (`release-binaries`, `release-npm-wasi` and
  `homebrew` on `cli/`, `release.yml` on `rust/`, `release-npm` on `npm/`)
  become `v*.*.*`.
- `semver-check` (baseline `core/v*`) and both `cargo-semver-checks` jobs
  (baseline `rust/v*`) look for `v*` first and fall back to the prefixed tags
  until the first `v5` exists.
- `tools/release.zig`, `version-floor` and most of `version-set` go; fig moves
  onto `dx release` with a `.config/release.toml` like the org's other repos.
  VERSIONING and the changelog's "one entry per release, not per artifact"
  section are rewritten for one track.

**What it costs.** A breaking change to any artifact is a major for all of
them. The expensive case is the Rust crate: `dx deps fig --all` reaches most of
the org, so each major is a pin propagation like fig 4's, now also when only
the CLI broke. npm and the crate will sometimes publish a version with nothing
new in it; that is harmless.

**The middle road, not taken:** one tag per release while each manifest keeps
its own version and each workflow publishes whatever its registry lacks. It
fixes the cursor and keeps the versions independent, but the tag's number then
means nothing for three of four artifacts, and most of the tooling stays.

## 7. Optional: `edit` becomes `replace` and `rename`

`edit` replaces a value and `edit --key` renames a key; beside `set`, `insert`
and `delete` it is the one verb that does not say what it does, and the one
action whose flag turns it into a different operation. If 5.0 breaks the CLI
anyway, `replace <file> <path> <value>` and `rename <file> <path> <name>` read
as what they are. `edit` stays as an alias that warns, and goes in 6.0.

This is naming, not a fix, and it is the item to drop if 5.0 should be as small
as it can be.

## Order of work

1. §3 and §4 first: small, and they make the CLI tests trustworthy for the rest.
2. §1 and §2 together, since both are the question "what is a value on the
   command line"; the CLI's value path becomes the bindings' (`literalOf`,
   splice text), and the JSON `json_string` special case leaves the CLI.
3. §5 as the tests for 1–4 land.
4. §6 as the release that ships them, with each `Behavioural-change:` trailer
   written on the commit that causes it.
