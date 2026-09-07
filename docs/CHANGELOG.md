```fig
title = CHANGELOG
description = Release history for `fig`
author = adammharris
created = 2026-08-17
updated = 2026-08-20
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
the end marker, where regeneration cannot reach it, and it rides down with its
section when the release is cut.

Cutting a release renames `## Unreleased` to the versions that went out, strips
the two markers from the section that just became history, and opens a fresh
empty `## Unreleased` above it. `zig build release` does that (see
[VERSIONING](VERSIONING.md)); the markers come out because exactly one pair
should ever be in this file — a second pair left behind in a released section is
one that the next `zig build changelog` would overwrite with unreleased work.

## Unreleased

<!-- git-cliff:begin — generated; edits here are overwritten -->

_No commits since the last release tag._

<!-- git-cliff:end -->

## core 2.9.0 · rust 3.4.0 · npm 2.9.0

### Added

- **editor** — the dangling comment anchor, and comment-out and back ([`48b6f42`](https://github.com/diaryx-org/fig/commit/48b6f42243f6460089760b37d50aaf5b9ca06d5e))

### Fixed

- **patch** — compare comment-op errors instead of switching on them; test the CLI everything-on ([`125e826`](https://github.com/diaryx-org/fig/commit/125e8263f75b55a92c6db19a456fdd96f67f72f4))
- **yaml** — spell every mapping key kind; fix two explicit-key parser gaps ([`0a0d646`](https://github.com/diaryx-org/fig/commit/0a0d64611ec494d7009cc86c1f36cce5d531ee34))
- **yaml** — no trailing space after the dash of a nested block sequence ([`3bd426a`](https://github.com/diaryx-org/fig/commit/3bd426a7d62b413617c2140281fad7a039fac370))
- **fig** — a mid-word quote inside a bracket-led value no longer opens a quoted span ([`cd64b53`](https://github.com/diaryx-org/fig/commit/cd64b533410ab3a1d52a4544f0e7bea8d9e6e377))
- **yaml** — print %TAG directives and root/item collection properties ([`76dc108`](https://github.com/diaryx-org/fig/commit/76dc108a714dc5a52bf08ef4e586d156be0c69c3))
- **editor** — a flow item on its parent's line owns no comment ([`13f8822`](https://github.com/diaryx-org/fig/commit/13f8822a66f67223b6378f99f112c138fcb77080))
- **json** — spell a scalar mapping key as a JSON string, refuse a collection key ([`fece77b`](https://github.com/diaryx-org/fig/commit/fece77ba7b76853a852a4909b0463a744c1584aa))

### Changed

- **languages** — derive the format set from one list, src/languages/list.zig ([`8b34254`](https://github.com/diaryx-org/fig/commit/8b34254d49f27560a2b6d2201f4e919c99a31beb))
- **languages** — declare the detection order per dialect as `sniff_rank` ([`2ef10aa`](https://github.com/diaryx-org/fig/commit/2ef10aacd575785e9abccffb44f7303bc9c3fd51))
- **languages** — move the last per-format facts in core onto the manifest ([`6a1f6b3`](https://github.com/diaryx-org/fig/commit/6a1f6b30942d4846c91439f02a36b745eacab409))
- **editor** — name the hook-facing surface as src/editor/splice.zig ([`449e900`](https://github.com/diaryx-org/fig/commit/449e900f007c85da7cd87e8687fdafe6c2cf6b62))

### Behavioural changes

- `manifest.Dialect.detectable` is replaced by `sniff_rank: ?u8` (null means not sniffed). A `Language` declared outside the tree that set `.detectable` no longer compiles; one that relied on the default is still sniffed only if it declares a rank, and the registry now refuses a language with no ranked dialect.

- `FlatStrip.Format` is removed and `FlatStrip.lossyStrip` takes the mapping-depth limit (`usize`) in its place; read it from `Language.<L>.caps.max_mapping_depth`. `cli/parse_dispatch.flatStripFormat` is `flatStripDepth`. No CLI or C ABI change.

- Printing a YAML document whose mapping key is not a
  string — null, number, boolean, alias, sequence, or mapping — now
  succeeds with a spelling of that key. It used to panic (`fig get`, `fig
  fmt`, `fig get -o yaml`, and every library serialize to YAML).

- `&a a: b` and `!!str a: b` now anchor/tag the key `a`.
  They used to anchor/tag the mapping, so `*a` resolved to the mapping
  (and `-o json` failed with `AliasCycle`), and a tagged first key failed
  materialization with `TagTypeMismatch`. `&m` on its own line above the
  first key still decorates the mapping.

- A nested explicit key (`?\n  ? a\n  : b\n: x`) now
  parses as the key `{a: b}` with value `x`. It used to parse as the key
  `{a: null}` with value `b`, then reject the trailing `: x`.

- `? &a` followed by a block collection on the next
  lines now anchors that collection as the key. It used to produce an
  anchored null key whose value was the collection, followed by a second
  null-key entry.

- A nested block sequence prints its parent dash as `-`
  with nothing after it. It used to be `- ` with a trailing space, so `fig
  fmt` output (and any YAML serialize) of such a document changes by that
  one byte per nested-sequence item.

- a `[`/`{`-led value whose bare text contains a `'` or `"` —
  `link = [it's here](x.md)`, a markdown link with an apostrophe in its text —
  now parses as the bare string it spells. It used to be committed to flow and
  fail with FigTrailingContent. `fig fmt` and the fig printer emit such a value
  unquoted now, where they previously quoted it.

- `fig fmt` and `fig get -o yaml` on a YAML document
  whose tags use a `%TAG` handle now emit the `%TAG` line and a `---`
  marker ahead of the body. The output used to carry the tags without the
  declaration, which made it a document fig itself rejected with
  `UndefinedTagHandle`.

- `fig fmt` and `fig get -o yaml` now emit an anchor or
  tag that sits on the root collection (`&m` on its own line above the
  first key) or on a sequence item that is a collection (`- &a`). Both used
  to be dropped, silently turning any `*m`/`*a` alias to them into an
  undefined-alias error on re-read.

- `getLeadingComment`/`getTrailingComment` at an element
  or entry of a one-line flow collection now return NONE (`not_found`
  across the C ABI, `None`/`null` in the Rust and TypeScript bindings).
  They used to return the enclosing entry's comment — the block above
  `members = ["a", "b"]` came back once for `members` and again for each
  of its items.

- `deleteLeadingComments`/`deleteTrailingComment` at such
  a path are now a no-op, still reporting success. They used to delete the
  enclosing entry's comment, so a caller that "cleared" an item's comment
  removed the whole block above the collection.

- `addLeadingComment`/`setTrailingComment` at such a path
  now refuse with `CommentsUnanchored` (`FIG_STATUS_INVALID_ARGUMENT`,
  `Error::InvalidArgument`, TypeScript `InvalidArgument`) and leave the
  source byte-identical. They used to write onto the parent's line, where
  the comment became the parent's — and on fig produced a corrupt document.

- `fig.Patch` now counts a leading or trailing comment
  destined for such a path as dropped (`stats.comments_dropped`) instead
  of writing it onto the enclosing entry's line.

- `fig get -o json` (and `-o jsonc`/`-o json5`, and the
  library's `serialize` to those formats) on a document with a non-string
  mapping key no longer emits the key bare. A null, boolean, number or
  datetime key is now written as a JSON string of its source text —
  `null: "a"` becomes `"null": "a"`, `23: false` becomes `"23": false` —
  where the old output was not JSON at all. A sequence or mapping key now
  fails with `NonStringKey` ("a non-string mapping key has no representation
  in this output format", exit 1) instead of writing an unparseable object;
  the 15 accept-corpus documents that hit this used to produce output.

## cli 4.0.0

### Added

- **nushell** — add a nushell plugin providing `from figl` / `to figl` ([`4b1420f`](https://github.com/diaryx-org/fig/commit/4b1420f235e2739cb8e86d20d2da479a05b8b977))
- **patch** — merge one document into another through the editor's splices ([`024dfb0`](https://github.com/diaryx-org/fig/commit/024dfb07b412ff703ec8ab0cf7f576ab9f7cf739))
- **cli** — `fig patch`, merging one file (or part of one) into another ([`fbb675c`](https://github.com/diaryx-org/fig/commit/fbb675c77b290240d1fbed2862f1ef19aa979f17))
- **editors** — add a Helix-flavoured highlight query and Helix setup docs ([`282f309`](https://github.com/diaryx-org/fig/commit/282f30933fff831eac954afbacfbde1f65b550ba))
- **cli** — hand an unknown action to a `fig-<action>` program, git-style ([`06efdf7`](https://github.com/diaryx-org/fig/commit/06efdf79847797f452e71fe8033ce5792fc86443))

### Fixed

- **wasi** — assert the shape of fig's missing-file error, not its wording ([`2a16ff1`](https://github.com/diaryx-org/fig/commit/2a16ff1672003bb30deeda36527d95567c598c08))
- **ci** — track stable for cargo-semver-checks, and test fig-wasi on every push ([`5268c8a`](https://github.com/diaryx-org/fig/commit/5268c8a3b0b161ad3807e3802383fb1a53eb7ad2))
- **tools** — run changelog.sh from the repo root, whatever the cwd ([`ec63ad6`](https://github.com/diaryx-org/fig/commit/ec63ad60081b473c736c6c1d11a151276889f852))
- **rust** — read radix-prefixed and separated number lexemes ([`17f1f6a`](https://github.com/diaryx-org/fig/commit/17f1f6a709f00f0e84cca76c5c6dd5c724172b45))
- **ts** — read number lexemes exactly instead of via Number() ([`eed7104`](https://github.com/diaryx-org/fig/commit/eed7104b3ee02def2caff70e3fbd79521781f666))
- **cli** — write the standard streams streaming, so a redirect isn't clobbered ([`48e0f52`](https://github.com/diaryx-org/fig/commit/48e0f52382897ef7409918ef7baaab6fd2d92a6c))
- **lsp** — read and write the stdio transport streaming ([`f15ec5f`](https://github.com/diaryx-org/fig/commit/f15ec5f8ecb401d000737d70dd144454dda5a55b))

### Changed

- **release** — take the shared cliff config, one style for every repo ([`1e828f3`](https://github.com/diaryx-org/fig/commit/1e828f37e557a56d01a87b21b9e5f4a0312220df))
- **lossless** — read the envelope's native kinds off each language's caps ([`5238c41`](https://github.com/diaryx-org/fig/commit/5238c41b542f2901aa7a87f6f2aa0f749e80e9ef))
- **languages** — each language declares its own registry rows ([`1b6c7a1`](https://github.com/diaryx-org/fig/commit/1b6c7a1c641d996b0b42cf1129f78cd403bd7033))
- **editor** — derive section regions from Document.node_regions instead of three per-format gathers ([`7d1c1fe`](https://github.com/diaryx-org/fig/commit/7d1c1fe42661d916702dc13bb410e14ec30c8204))

### Uncategorised — triage before release

- each language declares its lossless kinds and registry rows ([`fa68bce`](https://github.com/diaryx-org/fig/commit/fa68bce117e3e6c736ba6fdd2111f19482f4d507))
- derive section regions from Document.node_regions ([`b8d10e1`](https://github.com/diaryx-org/fig/commit/b8d10e1a7fe5305ada538adba44a11674034d9d5))

### Behavioural changes

- `Document::to_value` now succeeds on a document holding a
  hex/octal/binary or `_`-separated number, returning the integer. It used to
  fail the entire read with `Error::Number` carrying that lexeme, so a figl or
  ZON file containing one could not be read into a `Value` at all.

- `parse`/`Document` traversal now return an exact `int`/`uint`
  for a hex, octal, binary or `_`-separated integer. They used to return a
  `float`: `0xFF` as `255` typed float, `1_000` as `NaN`, and any value past
  2^53 rounded — all without raising.

- `fig`'s output to a REDIRECTED REGULAR FILE now appends
  at the stream's shared offset instead of starting at byte 0. A script
  running `fig` more than once under one redirection (`> out`, `>> log`, or a
  redirected block) used to get output written over the front of the file and
  over the output of neighbouring commands; it now gets all of it, in order.
  Pipes and terminals are unaffected — they always took this path.

- TOML: a dotted table (`a.b = 1`) is a section node, so
`deleteKey`, `moveKey` and `reorderKeys` on its entry now refuse with
`CannotDeleteTable`/`CannotMoveTable`/`CannotReorderTables`; they used to
line-splice, which was correct for a one-line table and silently left the
other lines behind otherwise. `deleteContainer` handles every case.

- TOML: `deleteContainer`/`moveContainer` of a table whose
dotted child spans several lines (`[a]` / `x.y = 1` / `x.z = 2`) now takes
every line; the `[`-sniffing gather took the child's first line only.

- TOML: `moveContainer` accepts a dotted table as the
destination; it refused with `NotATable`.

- fig: `moveKey` and `reorderKeys` on a block-container
entry now refuse with the new `CannotMoveContainer`/`CannotReorderContainers`;
they used to relocate the node's widened span, which for a re-entered
container is its first fragment alone. `moveContainer`/`reorderContainers`
carry every fragment.

- fig: `moveContainer` with a scalar destination now
refuses with `NotAContainer` rather than landing before the scalar's line.

- Zig API: `Document.reentry_headers`/`ReentryHeader` are
replaced by `Document.node_regions`/`NodeRegion`, `regionsOf`, `isSection`;
`languages/shared/sections.zig` is `editor/regions.zig`; a `Language` may no
longer declare `deleteContainer`/`moveContainer`/`reorderContainers` or any
`*Guard` hook, and declares `Syntax.section_noun` instead.

- `fig <unknown-action>` now exits 2. It used to print the
  general help and exit 0, so a typo'd action in a script reported success.
  The action list is still printed, under an `error:` line naming the word —
  and, when the word could name a program, only after no `fig-<word>` was
  found on PATH.

## core 2.7.0 · cli 3.6.0 · rust 3.3.0 · npm 2.7.0

### Added

- **embed** — report both host sides of a region, and rebuild from both ([`c2d1797`](https://github.com/diaryx-org/fig/commit/c2d17973452654f45b77848be1b35116e9f7a63b))
- **c-api** — export `retype` — re-house an embed under another archetype ([`f19b12e`](https://github.com/diaryx-org/fig/commit/f19b12ede104401c28c4b2be01775bdb3bbc5856))

### Behavioural changes

- `fig convert --to-embed` now refuses, with exit 2, to
  convert a mid-document embed (`html-script-*`, `html-code-*`) to an
  archetype that sits at an edge of the file. It used to emit a file with
  every byte before the block silently deleted — for an HTML page, the
  whole document head above the block.

- `fig convert --to-embed` from `endmatter` now keeps text
  that followed the closing fence. It used to drop it.

- a UTF-8 BOM now survives `fig convert --to-embed`, and
  stays at offset 0 when the block moves to the other end of the file. It
  used to be dropped.

- `fig get --body` now prints the host text on both sides of
  the block, in file order. For frontmatter and endmatter that is the same
  output as before, bar a leading BOM, which is now included; for a
  mid-document embed it used to print only the text after the block.

- `Embed.initRegion` / `fig_embed_open_or_init` on a source
  starting with a UTF-8 BOM now insert the new block after the BOM. They
  used to insert it before, leaving the BOM mid-file, where it is no longer
  a byte-order mark but a stray zero-width no-break space.

## rust 3.2.0 · npm 2.6.0

### Added

- **rust** — the scalar text parser, so text edits round-trip through fig ([`4ad9936`](https://github.com/diaryx-org/fig/commit/4ad9936370dff40efcceba9557d8a33dfaa2825a))
- **rust** — Value::eq_canonical, a comparison a dirty check can converge on ([`b966d7a`](https://github.com/diaryx-org/fig/commit/b966d7a9137ebfb628b552db4754e7808aaf0d2a))
- **build** — `zig build release`, the whole release as one command ([`200e9e4`](https://github.com/diaryx-org/fig/commit/200e9e471ac5cf931b9530b57c859eb8c9d45250))
- **build** — `as-is`, for releasing a version that is already in the tree ([`89f3e4a`](https://github.com/diaryx-org/fig/commit/89f3e4a6e23ff7e3aa421013e6763828de993237))

### Fixed

- **ci** — attach tangled artifacts by AT-URI, not the knot URL ([`f664b2a`](https://github.com/diaryx-org/fig/commit/f664b2a81c32faa229a537fe61cf9fa2f6ab1a50))
- **build** — vendor the crate README from README.md, not the fig.md that moved ([`71bffcb`](https://github.com/diaryx-org/fig/commit/71bffcb137c8e01e5c6c77750bb89a3ff6b88160))

### Changed

- **rust** — a small integer is Int, whichever Rust type it came from ([`667ed09`](https://github.com/diaryx-org/fig/commit/667ed098baec848273c8869956cee9b13255ff29))

### Behavioural changes

- released artifacts now show on the tangled tag page

- an unsigned integer that fits in `i64` now builds as
  `Value::Int`, not `Value::Uint` — from `Value::from(3u64)`, a `u8`/`u16`/
  `u32`/`u64`/`usize` field via `ToValue` or serde, or `from_str::`<Value>``.
  `Uint` now appears only past `i64::MAX`. Code matching `Value::Uint(_)`
  to catch small unsigned values needs an `Int` arm; the `as_i64`/`as_u64`/
  `as_f64` accessors are unaffected, and `Value::from(3u64)` is now `==` to
  a `3` read from a document.

## 2.6.0

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
