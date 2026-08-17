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

- **c-api** — expose the six whole-container editor ops ([`c4b5326`](https://github.com/diaryx-org/fig/commit/c4b532696551e5aa94b321eac51112992fa66ec1))
- **rust** — wrap the six whole-container editor ops ([`86debb4`](https://github.com/diaryx-org/fig/commit/86debb4997490c95a6a9c7cbdb9a6c408ff47d68))
- **ts** — wrap the six whole-container editor ops ([`cf52151`](https://github.com/diaryx-org/fig/commit/cf5215148c5a73540d23135e59a19e3dd79ffd51))

### Fixed

- **editor** — refuse value-replace on a TOML table / INI section header ([`7c9e09d`](https://github.com/diaryx-org/fig/commit/7c9e09dc0caaa5059cb86f2abee31af1a7b7e269))
- **toml** — rename a table at every line that names it ([`1f2bb3f`](https://github.com/diaryx-org/fig/commit/1f2bb3ff0a8fa6bb0801b0253659edfdb5c7613e))
- **c-api** — map the remaining editor refusals off parse_error ([`6949364`](https://github.com/diaryx-org/fig/commit/6949364c37dd088bc82191f0a23a407824b3265c))
- **toml** — let a root key be inserted into a header-first document ([`299020d`](https://github.com/diaryx-org/fig/commit/299020d47fc2b0cba4e38f9bffe57fb2e0950d98))
- **editor** — refuse move/reorder that would rehome a table's entries ([`8231aba`](https://github.com/diaryx-org/fig/commit/8231aba2ab01041b6d5fc4c9ed8c155c4e045cf2))
- **ts** — compile the test suite instead of running .ts through Node ([`c70b874`](https://github.com/diaryx-org/fig/commit/c70b874f499daa6c9814667e7574c8980fa9efdf))

### Changed

- refactor(ci): make homebrew workflow depend on shared diaryx-org
homebrew workflow ([`a702829`](https://github.com/diaryx-org/fig/commit/a70282946f5a11d90501499cfd49d13150a8b9d0))
- **gitignore** — more sensible gitignore ([`6f8ea23`](https://github.com/diaryx-org/fig/commit/6f8ea23a5fa48cd4afc11bbf849e8198e367d04d))

<!-- git-cliff:end -->

### Behavioural changes

These predate the `Behavioural-change:` trailer, which this release introduces,
so they are written here by hand rather than gathered from the commits. From the
next release on, this section is generated.

- **`edit`/`set`/`delete` at a TOML `[table]` or INI `[section]` path now
  refuse.** They used to report success, having rewritten the header's NAME and
  left the section's entries under whatever now precedes them — `[nested]` with
  a replacement of `"x"` became the still-valid `["x"]`. Across the C ABI this
  is `FIG_STATUS_INVALID_ARGUMENT`; from the CLI it is a diagnostic and exit 1,
  where before it was exit 0 and a corrupted file. The whole-container ops
  (`fig_editor_*_container`) are what address those shapes.

- **`move_key` and `reorder_keys` over a TOML table or INI section now refuse**,
  for the same reason and with the same status. A header entry's *block* is its
  header line, so moving it relocated the name and left the body for whichever
  container landed above it: reordering `[a]` and `[b]` at the root could empty
  one table and hand its keys to the other, silently and reparseably. Use
  `fig_editor_move_container` / `fig_editor_reorder_containers`.

- **`fig insert` / `fig set` of a root-level key into a header-first TOML file
  now works.** It used to fail with "not a valid value" naming the caller's
  value, which was never the problem — the root's span starts at the file's
  first byte, so `[package]`'s `[` was read as an inline table's opening
  delimiter. Any file opening with a `[header]`, which is to say most real TOML,
  could not take a new root key at all.

- **Editor refusals no longer arrive as `FIG_STATUS_PARSE_ERROR`.**
  `CannotDeleteSection`, `CannotDeleteContainer`, `EmptyInlineContainer`,
  `KeyRequiresMultilineForm` and `InvalidComment` now map to
  `INVALID_ARGUMENT`, and plist's `NullUnsupported` to `UNSUPPORTED_FORMAT`. A
  caller that branched on `PARSE_ERROR` to mean "the file is malformed" was
  being told the wrong thing about its own request.

- **`fig get --help` no longer claims ini/dotenv/properties/nestedtext have no
  in-place editor.** All four have been editable for some time; only `xml` still
  has no editor. No behaviour changed here — the documentation was wrong, which
  is worth listing because it was load-bearing for anyone choosing a format.
