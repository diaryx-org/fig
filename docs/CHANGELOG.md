```fig
title = CHANGELOG
description = Release history for `fig`
author = adammharris
created = 2026-08-17
updated = 2026-08-17
part_of = [docs](docs.md)
```

# fig — changelog

One entry per release, newest first, plus an `Unreleased` section for work that
has landed on `main` and not yet been tagged.

Release notes used to live only in the annotated tag message (`git tag -n20`).
Those are kept, and they are still the record for everything released before
this file existed; nothing has been backfilled here.

## One entry per release, not per artifact

fig ships four independently versioned artifacts off one tree — the core, the
CLI, the Rust crate and the npm package — each with its own tag prefix (see
[VERSIONING](VERSIONING.md)). A release pushes a tag for whichever of them
actually moved, usually several at the same commit.

This file has one section per *release*, not per artifact, with a heading that
names every version that went out together:

```markdown
## core 2.6.0 · cli 3.5.3 · rust 3.2.0 · npm 2.6.0
```

The heading is handwritten. git-cliff owns only the bytes between the markers
inside a section, and it groups commits by what they changed, which is the
question a reader actually has — the scope on each bullet (`**toml**`,
`**c-api**`, `**ts**`) says which surface moved far more usefully than four
parallel lists would.

## Behavioural changes are their own section

Every entry has the usual Added / Fixed / Changed lists and, when it applies, a
**Behavioural changes** list. A behavioural change is one that alters *what an
existing call does* without altering any type, signature, or ABI symbol — the
class of change that compiles clean against the previous version and then
behaves differently at runtime.

The section is mandatory rather than a courtesy, because nothing else catches
this class for fig's consumers. fig is an *editor*: its whole contract is what
bytes come out the other side of an edit. A refusal that used to be a success,
a splice that used to land somewhere else, a status code that used to be
`PARSE_ERROR` — none of those move a symbol, and none of them are visible to a
compiler in any language that binds fig. They show up as a wrong file on disk.

The rule for whether something belongs here: **if a caller who upgrades without
editing a line of their own code would observe a difference, it goes in this
section** — even when the change is a bug fix, and even when the previous
behaviour was plainly wrong. Especially then, in fact: fig's behavioural changes
are mostly operations that used to corrupt a file and now refuse it, which is
exactly the shape a consumer needs warning about, because their code was
"working" before.

## Where a behavioural change is written down

On the commit that causes it, as a `Behavioural-change:` trailer.
`.config/cliff.toml` collects them into the section above.

```
fix(editor): refuse value-replace on a TOML table / INI section header

<the body: why, and how it works>

Behavioural-change: `fig edit` and `fig_editor_replace_val` at a TOML
  `[table]` or INI `[section]` path now refuse with `CannotReplaceTable` /
  `CannotReplaceSection` (`FIG_STATUS_INVALID_ARGUMENT`). They used to report
  success, having rewritten the header's NAME and left the section's entries
  under whatever now precedes them.
```

One trailer per observable difference; a commit may carry several, and most
commits carry none. Continuation lines are indented two spaces and fold into one
paragraph. Write the value for a consumer deciding whether to upgrade — what
used to happen, what happens now — not for a reviewer reading the diff. That is
what the body above it is for.

The judgment "would an unedited caller observe a difference" is not recoverable
from a commit subject: a `fix:` can be behavioural and an `add:` can be silently
behavioural. Only the author knows. That is why it is a trailer rather than a
heuristic over the type — but writing it *at the commit* is also what stops it
drifting from the change it describes, or being forgotten between landing the
work and cutting the release, which is exactly when it is least likely to be
reconstructed.

## How the Unreleased section is written

`zig build changelog` regenerates the marked region below from the commits since
the most recent tag on any track, using `.config/cliff.toml`: one bullet per
commit, grouped Breaking / Added / Fixed / Changed, then the **Behavioural
changes** section gathered from the trailers. Edits inside the markers are
overwritten on the next run. `zig build changelog-check` fails if the region is
stale, without writing.

Commits whose subject does not parse land in an **Uncategorised — triage before
release** bucket rather than being dropped, so they get a decision instead of
disappearing.

What is left to write by hand is a release **intro** — a paragraph or two for a
release that wants a narrative rather than a list. Most releases want none, and
an intro that only restates the bullets below it should be cut. It goes below
the end marker, where regeneration cannot reach it. Cutting a release means
renaming `## Unreleased` to the versions that went out and leaving the generated
region in place.

## Unreleased

<!-- git-cliff:begin — generated; edits here are overwritten -->

### Added

- **c-api** — expose the six whole-container editor ops ([`2aca171`](https://github.com/diaryx-org/fig/commit/2aca171c177c586f14163aa0fa3cdaa851d1ec7c))
- **rust** — wrap the six whole-container editor ops ([`d53b23d`](https://github.com/diaryx-org/fig/commit/d53b23dc9a884eb59bfeb60d17fcfbecac0b1512))
- **ts** — wrap the six whole-container editor ops ([`4ed4e9c`](https://github.com/diaryx-org/fig/commit/4ed4e9c6075b9b8eef5895d5041cba2809ff9070))
- **docs** — a git-cliff changelog, with Behavioural-change trailers ([`fb97e5c`](https://github.com/diaryx-org/fig/commit/fb97e5ccc08830dc07c47069d4758e150d491cee))

### Fixed

- **editor** — refuse value-replace on a TOML table / INI section header ([`3ad5019`](https://github.com/diaryx-org/fig/commit/3ad50195c0d365cf816c2e9bf6c94794762733ad))
- **toml** — rename a table at every line that names it ([`e4a7d4d`](https://github.com/diaryx-org/fig/commit/e4a7d4dcde6c476f49d7117a910b19c6502c6f81))
- **c-api** — map the remaining editor refusals off parse_error ([`11ddc01`](https://github.com/diaryx-org/fig/commit/11ddc01b1a6d92c4bd44895be1b2846dd4e39d23))
- **cli** — stop claiming ini/dotenv/properties/nestedtext have no editor ([`74b0c5f`](https://github.com/diaryx-org/fig/commit/74b0c5f3bec7108e562345a0849016068dd72a5d))
- **toml** — let a root key be inserted into a header-first document ([`d5babce`](https://github.com/diaryx-org/fig/commit/d5babcedd2a5df3cf566f040c5983aea14550c4c))
- **editor** — refuse move/reorder that would rehome a table's entries ([`0bc51f3`](https://github.com/diaryx-org/fig/commit/0bc51f34f4a250f2b47bfd7436a17efe0fad347a))
- **ts** — compile the test suite instead of running .ts through Node ([`18fa48a`](https://github.com/diaryx-org/fig/commit/18fa48a2d7853cfdc03491d68e595ac3bc8bef8d))

### Changed

- refactor(ci): make homebrew workflow depend on shared diaryx-org
homebrew workflow ([`a702829`](https://github.com/diaryx-org/fig/commit/a70282946f5a11d90501499cfd49d13150a8b9d0))
- **gitignore** — more sensible gitignore ([`cd27228`](https://github.com/diaryx-org/fig/commit/cd2722850f3cc982cc8a0372d1f58f7a48044331))
- **docs** — fig.md->README.md; create prov.yaml ([`e1bc2d5`](https://github.com/diaryx-org/fig/commit/e1bc2d5f03d0e346ee9061e627375a14dabc0656))

### Behavioural changes

- `fig edit`/`fig set` and `fig_editor_replace_val` at a
  TOML `[table]` or INI `[section]` path now refuse — `CannotReplaceTable` /
  `CannotReplaceSection`, `FIG_STATUS_INVALID_ARGUMENT`, and from the CLI a
  diagnostic and exit 1. They used to report success, having written the
  replacement over the header's NAME and left the section's entries under
  whatever now precedes them: `[nested]` with a replacement of `"x"` became the
  still-valid `["x"]`. The whole-container ops address those shapes.

- renaming a TOML table now rewrites every line that names
  it. `fig_editor_replace_key` at a block-table path used to rewrite only the
  mention carrying the key node, splitting the table in two — `[a]` + `[a.b]`
  renamed to `q` became `[q]` + `[a.b]`, which still parses, so nothing
  failed and the old name was simply re-created around the leftovers. It also
  renamed only the FIRST element of an array-of-tables, and on a dotted table
  (`a.b = 1`) `renameContainer` was a silent no-op. All three now rewrite
  every mention; `renameContainer` on a scalar or inline table answers
  `NotATable` instead of doing nothing.

- six editor refusals no longer arrive as
  `FIG_STATUS_PARSE_ERROR`. `CannotDeleteSection`, `CannotDeleteContainer`,
  `EmptyInlineContainer`, `KeyRequiresMultilineForm` and `InvalidComment` now
  map to `INVALID_ARGUMENT`, and plist's `NullUnsupported` to
  `UNSUPPORTED_FORMAT`. A caller branching on `PARSE_ERROR` to mean "the file
  is malformed" was being told the wrong thing about its own request.

- `fig insert`/`fig set` and `fig_editor_insert_key` can now
  add a root-level key to a TOML file that OPENS with a `[header]` — which is
  most real TOML, every Cargo.toml among them. The splice used to be rejected by
  the reparse and rolled back, surfacing as "not a valid value" naming the
  caller's value, which was never the problem.

- `fig_editor_move_key` and `fig_editor_reorder_keys` now
  refuse at, or across, a TOML `[header]` table or INI `[section]`, with
  `FIG_STATUS_INVALID_ARGUMENT`. Both used to report success while relocating
  only the header LINE: reordering two root tables could empty one and hand its
  keys to the other, and the result reparsed cleanly, so nothing rolled back.
  `fig_editor_move_container`/`fig_editor_reorder_containers` relocate a
  scattered container whole.

- `zig build changelog` is a new build step; it needs
  git-cliff on PATH (added to the nix dev shell). It is not part of
  `zig build check`, so an absent git-cliff cannot fail an ordinary build.

<!-- git-cliff:end -->
