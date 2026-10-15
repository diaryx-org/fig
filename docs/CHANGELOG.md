```fig
title = CHANGELOG
description = Release history for `fig`
author = adammharris
created = 2026-08-17
updated = 2026-10-14
part_of = [docs](docs.md)
```

# fig — changelog

One entry per release, newest first, plus an `Unreleased` section for work that
has landed on `main` and not yet been tagged.

Release notes used to live only in the annotated tag message (`git tag -n20`).
Those are kept, and they are still the record for everything released before
this file existed; nothing has been backfilled here.

## One entry per release, and one version

From 5.0.0 every artifact fig ships — the core, the CLI, the Rust crates,
`@diaryx/fig` and `@diaryx/fig-wasi` — carries one version and is released by
one `v*` tag (see [VERSIONING](VERSIONING.md)). So each release has one section,
headed by that version and the day it was cut:

```markdown
## v<major>.<minor>.<patch> — <yyyy-mm-dd>
```

(Spelled out rather than shown with a real version, because `dx` reads every
line that starts `## v<version>` as that release's section — a code sample
included.)

git-cliff groups a section's commits by what they changed, which is the
question a reader actually has, and the scope on each bullet (`**toml**`,
`**c-api**`, `**cli**`, `**ts**`) says which surface — and so which artifact —
moved.

Before 5.0.0 the four artifacts were versioned independently, each on its own
tag prefix, and a release pushed a tag for whichever of them moved. Those
sections are kept as they were written, with a heading naming every version
that went out together:

```markdown
## core 2.6.0 · cli 3.5.3 · rust 3.2.0 · npm 2.6.0
```

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

`dx changelog --write` regenerates the marked region below from the commits
since the last release tag, using `.config/cliff.toml`: one bullet per commit,
grouped Breaking / Added / Fixed / Changed, then the **Behavioural changes**
section gathered from the trailers. Edits inside the markers are overwritten on
the next run. `dx changelog --check` fails if the region is stale, without
writing, and `dx changelog` prints it. "The last release tag" is the newest
`v*` from 5.0.0 on, and the newest of the old prefixed tags before the first of
those — see `tag_pattern` in `.config/cliff.toml`.

Commits whose subject does not parse land in an **Uncategorised — triage before
release** bucket rather than being dropped, so they get a decision instead of
disappearing.

What is left to write by hand is a release **intro** — a paragraph or two for a
release that wants a narrative rather than a list. Most releases want none, and
an intro that only restates the bullets below it should be cut. It goes below
the end marker, where regeneration cannot reach it, under a `###` heading of its
own, and it rides down with its section when the release is cut.

Cutting a release — `dx release` (see [VERSIONING](VERSIONING.md)) — renders the
region once more, writes it as a new `## v<version> — <date>` section directly
below the end marker, and resets the region to empty. `## Unreleased` and the
one marker pair stay where they are, so exactly one pair is ever in this file
and no regeneration can reach a released section.

## Unreleased

<!-- git-cliff:begin — generated; edits here are overwritten -->

### Breaking

- **fig** — compact flow objects — whitespace-separated pairs, one-token values, dotted keys ([`acd8701`](https://github.com/diaryx-org/fig/commit/acd87014cb9ec7fcbf26718bd487f1b7be3e3db9))
- a language spells a key's name through renderKeyName, and KeyStyle is gone ([`4660d14`](https://github.com/diaryx-org/fig/commit/4660d145ab54b613f75da36047cd418183219cd2))
- section refusals are one neutral set, and section_noun is the language's own word ([`da5f5de`](https://github.com/diaryx-org/fig/commit/da5f5de788999fb1d19d8583a2817036d057274a))
- the owned-comment scan reads declared delimiters, and CommentStyle is gone ([`99beb20`](https://github.com/diaryx-org/fig/commit/99beb200f7c942182fef11c5e4f594570b815e3b))
- the escaped splice style is named for what it does, not for JSON ([`a0c9dd7`](https://github.com/diaryx-org/fig/commit/a0c9dd7b702a529de94995d0d720fb50b63ff4fc))
- the node table is fig's, 32-byte rows a language fills through fig's calls ([`05b5faf`](https://github.com/diaryx-org/fig/commit/05b5fafef3d1564324c2b3090c7b7f724972359c))
- the tree a language builds, the node table and their conversions move into the kit ([`d952b66`](https://github.com/diaryx-org/fig/commit/d952b66ee44cb17a5f0b4441189190f433e85e99))
- no compiled registry by default; every language reaches core through the contract ([`b0bd6bf`](https://github.com/diaryx-org/fig/commit/b0bd6bfffbc17a5046ae8a107340183856b35ada))
- core keeps no list of languages, and a format is a registered dialect's handle ([`d2ffc82`](https://github.com/diaryx-org/fig/commit/d2ffc82674643ea6d91839b8af021d4792e5b2b7))
- the authoring dialect is figl, and fig is no format's name ([`10a0e9b`](https://github.com/diaryx-org/fig/commit/10a0e9bf1447422226908d58cb6ca7c56f6bd25d))
- **cli** — the CLI reads and writes no embedded blocks ([`40aea3a`](https://github.com/diaryx-org/fig/commit/40aea3aa4860a227d1f4d1110d733fa338f2bd8a))
- **bindings** — the Rust and TypeScript bindings carry no embedding ([`e0cbc83`](https://github.com/diaryx-org/fig/commit/e0cbc83d1cfde882e10d78e9cbeb57841f1e37a1))
- the core and the C ABI carry no embedding ([`b801a52`](https://github.com/diaryx-org/fig/commit/b801a5200a4f08d86edbce2364598f297c766093))
- the helper wire is JSON-RPC 2.0, on a fork of zigjr ([`cf43bfe`](https://github.com/diaryx-org/fig/commit/cf43bfe16ad9c4d1a653e72396974a030b5c4a71))
- the Document is the node table, adopted from a parse with no copy ([`95ea28e`](https://github.com/diaryx-org/fig/commit/95ea28e1db1fcbf449545bebbd3b32af58fbc1e8))
- ParseTree is gone; every parser writes rows, and RowWriter is exported ([`f22a246`](https://github.com/diaryx-org/fig/commit/f22a24671c2c232c8f8e2de7364f13e5bdc580ec))
- **npm** — each language is fig's own Zig, compiled to a WebAssembly module of its own ([`083d022`](https://github.com/diaryx-org/fig/commit/083d02283f1ec3d89e528030cd90e86ba879cbc4))
- fig-sys and libfig carry no language; each is a library and crate of its own ([`9420df9`](https://github.com/diaryx-org/fig/commit/9420df9eaee8ec085bb88050f26b26730ee6bc79))
- **npm** — each language is an npm package of its own, @diaryx/fig-`<name>` ([`edaad38`](https://github.com/diaryx-org/fig/commit/edaad388bfec5b6a7ccaf2134be1a39aab06c758))
- **kit** — the JavaScript kit's number and datetime rules are the Zig kit's, as WebAssembly ([`b55fbc7`](https://github.com/diaryx-org/fig/commit/b55fbc77cc1832f54ca10feda0290514b0dbf1a3))
- **rust** — fig::helper reads and writes the wire with serde_json, behind a helper feature ([`20f22aa`](https://github.com/diaryx-org/fig/commit/20f22aa8642f2e9552a9786ecaba494d2322019a))

### Added

- **cli** — set upserts a sequence item, and [-] is the last item in every path ([`3dd5da2`](https://github.com/diaryx-org/fig/commit/3dd5da216d320811b0bcc25d93fb53a1f55a4e3e))
- **cli** — help --oneline says what fig is for in one line ([`9c5fb06`](https://github.com/diaryx-org/fig/commit/9c5fb06a3a2a9ff267dbc904a071b80c0d9c253c))
- a sequence row names its own item separator, and insertItem inserts at an index ([`ef72fab`](https://github.com/diaryx-org/fig/commit/ef72fab0a27adbb443a4069284ea8a0ee892766f))
- a registered dialect declares prints, and its conversion warnings come from it ([`cd87c10`](https://github.com/diaryx-org/fig/commit/cd87c102b1a5f2179a8cdf9a4f9a6323db93deac))
- content sniffing in the caller's order, and the CLI passes its own ([`ad462b2`](https://github.com/diaryx-org/fig/commit/ad462b2a32d87f6d83a1d59fbcfe7edbb8cd99eb))
- a language splits its own streams, and every binding can ask it to ([`a0ff02c`](https://github.com/diaryx-org/fig/commit/a0ff02c2111ed3d49d91f907679369596f848be4))
- a Zig language's contract vtable is built from its module, and every one registers through it ([`e60a58a`](https://github.com/diaryx-org/fig/commit/e60a58afd707ea048b997c9d9b24f3fd67d7ad22))
- a language's diagnostics cross the contract ([`07fee22`](https://github.com/diaryx-org/fig/commit/07fee2248862fbcba541ac9c690e6da5ee9ff8ad))
- -Dcontract-copies, and a figl value through the contract is a fragment in flow ([`d0eb2a7`](https://github.com/diaryx-org/fig/commit/d0eb2a7e5ff4ad885c46f3db6b24c89883fb4184))
- -Dcompiled-registry=false, where every bundled Zig language registers through its contract ([`c1a8f18`](https://github.com/diaryx-org/fig/commit/c1a8f181ee4e08ba926fb7abdd9ad1f7c69c1e25))
- a build with no compiled registry passes the gate, through the registered languages ([`12b6828`](https://github.com/diaryx-org/fig/commit/12b6828bd490d4615e827fc745f2f9521e128a90))
- **c** — libfig exports the kit's number and datetime rules as fig_kit_* ([`bac358f`](https://github.com/diaryx-org/fig/commit/bac358f2daeda515da4532cef34f4a864afe58a5))
- **rust** — fig::kit::number and fig::kit::datetime, and helper::decode is public ([`de59b16`](https://github.com/diaryx-org/fig/commit/de59b167256374a2ebea98b8a56fad186c75bd8a))
- **rust** — the language crates publish, building against the kit fig-language-build carries ([`f6cf3fe`](https://github.com/diaryx-org/fig/commit/f6cf3feb3ed0687d1628ef5a0edbe0d825ae8a7b))
- **editor** — set starts a missing sequence, from the format's empty one ([`bcde343`](https://github.com/diaryx-org/fig/commit/bcde343ec071c1fab5ecb88f3ec719cd7638cb06))
- **kit** — a mention can say where a sequence item's path is written ([`07b3b1b`](https://github.com/diaryx-org/fig/commit/07b3b1b51097bef0aaffa59ea57f34c48efdeeed))
- plist is built in by default ([`daa25b7`](https://github.com/diaryx-org/fig/commit/daa25b7adc04818fa7e72002907ee5d8ad47c462))
- **docs** — unfinished devlog ([`904649a`](https://github.com/diaryx-org/fig/commit/904649a06a3f77525ed1f75235b293463838f451))
- **devlog** — devlog done ([`f006ec7`](https://github.com/diaryx-org/fig/commit/f006ec747cefc1c1d3b779aed0f7f1fdaaf11506))
- **rust** — prebuilt payloads carry every language, on six more targets ([`8739f24`](https://github.com/diaryx-org/fig/commit/8739f24086e4b3c58e050f7cc7d2ec3ff478bfc9))
- **rust** — Document and Editor are Send ([`867e304`](https://github.com/diaryx-org/fig/commit/867e304932a06b24c194112f9644df9b9b114f02))
- **rust** — a typed value that fails below the root says where ([`a61da1d`](https://github.com/diaryx-org/fig/commit/a61da1d4f801c1f9a99696023583fe6a72b10f49))
- **rust** — each language crate reads and writes typed values like serde_json ([`71656e1`](https://github.com/diaryx-org/fig/commit/71656e13fefc87b26fab4ac0ecce3efb990d440b))
- **editor** — a replaced string keeps its quote style ([`13ce0a8`](https://github.com/diaryx-org/fig/commit/13ce0a8eb598611968ec1f8117427d7aa5a73507))

### Fixed

- **properties** — refuse trailing comment ops, since a `#` after a value is value text ([`a062c32`](https://github.com/diaryx-org/fig/commit/a062c3207ba92249d8788c72995ce191739c5898))
- **zon** — the JS language refuses a control byte in a `\\` string line ([`25ba6ae`](https://github.com/diaryx-org/fig/commit/25ba6aee3fccb43a96d944f591afa51f578bcea2))
- **figl** — refuse a `: char` value that is not single-quoted ([`704238e`](https://github.com/diaryx-org/fig/commit/704238e9569604668211b495d8d1f90b619c8945))
- the bindings read values through a YAML alias and a merge key ([`3835900`](https://github.com/diaryx-org/fig/commit/3835900c14e8fcc331e1caa8143b70949abb14ff))
- a figl splice is flow only when the caller asks, and print options name no language ([`0aa4ff3`](https://github.com/diaryx-org/fig/commit/0aa4ff35df31ab17aced104acfe60cb12c5e4c36))
- an insert by key syntax that a parser refuses as a repeat is DuplicateKey, and a runtime print is handed a stripped table ([`1ef3132`](https://github.com/diaryx-org/fig/commit/1ef3132a97e01b5aa8b1bc34d55e51529d3e2454))
- a dialect that says its printer writes no comments is warned about without a syntax ([`33fac4a`](https://github.com/diaryx-org/fig/commit/33fac4a0a3ad45764617ab0fb6d811b9d46c3977))
- **cli** — a Markdown or HTML file is refused, not sniffed as a config file ([`1622b78`](https://github.com/diaryx-org/fig/commit/1622b78ec5b1189705582381128d7398f5917267))
- **nushell** — the plugin registers figl from fig-figl ([`90116e1`](https://github.com/diaryx-org/fig/commit/90116e18e0219948bfdfd512b378182b4f4a2366))
- **wasi** — copy the JavaScript languages from the library's js/, where they moved ([`bc5943d`](https://github.com/diaryx-org/fig/commit/bc5943d8abd23dadc7baef2c90355e5299534656))
- **kit** — a radix integer past 128 bits converts to decimal, rather than staying as written ([`99b725d`](https://github.com/diaryx-org/fig/commit/99b725d0ae87f16c9e66e16f34a53ae0d85629a9))
- **tree-sitter** — a fig object is compact, with dotted keys and one-token values ([`c798507`](https://github.com/diaryx-org/fig/commit/c79850774e231728432cf20cf7bf1f577d8d9a06))
- **editor** — deleting a block sequence's only item leaves `[]`, not null ([`9fc2b4f`](https://github.com/diaryx-org/fig/commit/9fc2b4f00afcbc41a66e15ec238137cb21ab8797))
- **editor** — an insert into a compact fig object writes it as fmt does ([`40c1175`](https://github.com/diaryx-org/fig/commit/40c1175357571fb30e4a37bca25da6ed54c4df63))
- **editor** — a trailing comment on a CRLF line goes before the CR ([`13478e3`](https://github.com/diaryx-org/fig/commit/13478e3c89c317ad84fa61172e788f5dae165c72))
- **editor** — a trailing comment that would read back as value text is refused ([`aa95dad`](https://github.com/diaryx-org/fig/commit/aa95dad84909865136aa9f8e76b719acc6d5c9ab))
- **properties** — a line written after a continued line no longer joins it ([`f8582f6`](https://github.com/diaryx-org/fig/commit/f8582f607484c18726883b6cd55ff2227ad31536))
- **figl** — a non-finite float prints as `key: float = inf`, and an edit that cannot annotate it refuses ([`e561191`](https://github.com/diaryx-org/fig/commit/e5611918b6bd42aa41101ab1a8413e23423b4044))
- **cli** — an edit with no text of the caller's that leaves the file unparseable is reported, not a panic ([`08fa574`](https://github.com/diaryx-org/fig/commit/08fa574305991fc7c3f320413d3d145d10283822))
- **editor** — a YAML block scalar's trailing comment goes on its header line ([`e6ca1eb`](https://github.com/diaryx-org/fig/commit/e6ca1eb139c58761b945faa02635e7a2d1660d2e))
- **editor** — a YAML delete on a parent's marker line keeps the parent ([`346db34`](https://github.com/diaryx-org/fig/commit/346db346f3ecda4ff65aa3c0c25afab0717d9d4c))
- **editor** — deleting a figl list's only element leaves `key = []`, not no key ([`dcc6617`](https://github.com/diaryx-org/fig/commit/dcc66179f0e3bb08049a598cb01f24b17eee4d59))
- **yaml** — a line under compact nested items belongs to the item at its column ([`0f784cb`](https://github.com/diaryx-org/fig/commit/0f784cb74d8349c2b8e91fe647c3652fda0f1e31))
- **editor** — the delete guard counts what goes with the deleted entry as the entry's ([`6300fac`](https://github.com/diaryx-org/fig/commit/6300facd0166ebdea5fd3a7da19138aae4d3ba97))
- **yaml** — a plain scalar continues onto a more-indented line that starts with a dash ([`df9edca`](https://github.com/diaryx-org/fig/commit/df9edca2a56a4357891326ed64cdac4435e37589))
- **editor** — a figl multi-line string's trailing comment lives on its opener line ([`ce65966`](https://github.com/diaryx-org/fig/commit/ce6596627e23d9a0f60019e35e4250ea3e7b6d10))
- **figl** — the printer writes a multi-line string's comment on its opener line ([`8a1c8e4`](https://github.com/diaryx-org/fig/commit/8a1c8e4855d267f45601f26b67b8c4051035915e))
- **editor** — a dotted key inside a TOML inline table is deleted as the inline table's entry ([`557abcc`](https://github.com/diaryx-org/fig/commit/557abcca05a872b6e9632fa3671d7debaee6f691))
- **editor** — deleting the only element of a TOML array of tables leaves `x = []` ([`d5a4010`](https://github.com/diaryx-org/fig/commit/d5a40104e475e4de5c1c30bfa675781e30fd55ba))
- **editor** — a figl delete keeps the elements after it, renumbering indices and re-spelling a + ([`6621ef0`](https://github.com/diaryx-org/fig/commit/6621ef024f6042312ef3f9e73ac8666b9e73f61c))
- **editor** — a new key in a TOML table with no header goes in as a key path where its entries are ([`dd8d1ba`](https://github.com/diaryx-org/fig/commit/dd8d1bab6e43fcf3db670b6bc4c1949935163c82))
- **editor** — a block value replacing a YAML list item goes under the item's content column ([`f66847e`](https://github.com/diaryx-org/fig/commit/f66847e82dc4ffde7f6e450dfe359856312fc400))
- **editor** — a replace, insert, move or reorder that reads back as anything but what was asked is rolled back ([`5a1a559`](https://github.com/diaryx-org/fig/commit/5a1a559fbc93cca8e6462a612677b694eebb780e))
- **zon** — an enum literal's span covers its leading dot, so a replace takes the whole literal ([`1a8407b`](https://github.com/diaryx-org/fig/commit/1a8407b36d1ccd786cc213f218212380ca3f37c9))
- **editor** — replacing an empty value keeps a blank before a trailing comment ([`cc907ae`](https://github.com/diaryx-org/fig/commit/cc907aeb8f9a614f1a8ab582675580aa6f8c4e85))
- **yaml** — a value after a tab, an empty tagged value and an explicit key's value can be replaced ([`638f6d7`](https://github.com/diaryx-org/fig/commit/638f6d71a3dadc799304e5f3196eca95835434d1))
- **editor** — four deletes take only their own entry or item ([`42fcf0b`](https://github.com/diaryx-org/fig/commit/42fcf0b1f6b9b15b1dff53722615bcf01f70155b))
- **yaml** — the JavaScript twin hands a line under compact nested items to the item at its column ([`7376b24`](https://github.com/diaryx-org/fig/commit/7376b24eb67fccb1ef585fc5caa0eb03a20e6524))
- **wasi** — build the kit's zig-bytes.js into lib/ instead of expecting ../typescript's ([`f682d32`](https://github.com/diaryx-org/fig/commit/f682d32a73e38b8e0c9bd44eaeca472b931f1c8a))
- **editor** — a figl multi-line flow list's or object's trailing comment lives on its opener line ([`0befb95`](https://github.com/diaryx-org/fig/commit/0befb9556c5fea3be72d9800058b9753b6b4716a))
- **editor** — an insert into a figl element goes among its own keys, before a header that opens a child of it ([`07dabc4`](https://github.com/diaryx-org/fig/commit/07dabc48ce36fc4f5f77def0d3e87517d07abc25))
- **editor** — a figl move or reorder re-spells each element's header, + and index for its new place ([`f8f63c4`](https://github.com/diaryx-org/fig/commit/f8f63c44808cf26a20322bfa652f9006a88c775e))
- **patch** — a number the target already holds is left as the target spells it ([`a714436`](https://github.com/diaryx-org/fig/commit/a71443660a752c06df2e8bfef34a460cb93d3aa8))
- **figl** — a key that re-opens a container mid-path is a mention of it ([`ecc6d1d`](https://github.com/diaryx-org/fig/commit/ecc6d1da6cda2213879fd821d2337266ef7a7ddb))
- **editor** — a key renamed against its colon takes a blank after the colon ([`1c79939`](https://github.com/diaryx-org/fig/commit/1c79939217fef96fe3886851b6a6bab34ac3e33b))
- **yaml** — a BOM-led document reads and edits its first key, keeping the BOM ([`284b01f`](https://github.com/diaryx-org/fig/commit/284b01f1cfdbd60b0ec0e11e95786e7b1f826c05))
- **editor** — a delete or rename of a repeated key takes every spelling ([`ce1d359`](https://github.com/diaryx-org/fig/commit/ce1d359b72f7049ca20db7f2dd1c5c0ad8c2f36c))
- **c-api** — the wasm static library builds without the helper wire ([`e98c5de`](https://github.com/diaryx-org/fig/commit/e98c5de789a545edbc110b3a025480330fd094fb))
- **build** — fig's C libraries are position-independent on every target ([`52c8161`](https://github.com/diaryx-org/fig/commit/52c8161c3d44d75859e57549de5ca3216fa836f7))

### Changed

- **toml** — an array of tables appends in constant time, not after walking its elements ([`22301ee`](https://github.com/diaryx-org/fig/commit/22301eee48e8a8b99f7c92d4fbb0856f357c9245))
- core reads the merge key from the language's syntax, not a hardcoded << ([`9356de0`](https://github.com/diaryx-org/fig/commit/9356de09bffb531c3e0b3eab0b6c58151764449a))
- **c-api** — drop the pin that only YAML declares a reference layer ([`26e46ac`](https://github.com/diaryx-org/fig/commit/26e46acf91aa41c5eab1b758efdb60255abccc59))
- **editor** — the flow sniff reads the language's flow_map_open, not ZON's .{ ([`e64e01d`](https://github.com/diaryx-org/fig/commit/e64e01dcd688a12e0f481c29d94a3a03d11a0d42))
- **editor** — a line's prefix marks are declared, not the bytes - * > + ([`9c01f00`](https://github.com/diaryx-org/fig/commit/9c01f00afdec1a1a245b18fda08d6fd630422267))
- **editor** — the bare-literal rule lives in util/literal, not figl's tokenizer ([`f32098f`](https://github.com/diaryx-org/fig/commit/f32098f6eb684316d4f4f687747ab0db4888649b))
- **diagnostics** — conversion warnings read each dialect's prints declaration ([`da59b34`](https://github.com/diaryx-org/fig/commit/da59b3437c07bdc6c338c7c198c4831a31a826fd))
- a dialect names its fragment printer, and figl's splice is flow by its own rule ([`6b89af1`](https://github.com/diaryx-org/fig/commit/6b89af137cc80cd3924309ae782a410751c3e90c))
- YAML's stream splitter moves from core's embed layer into YAML ([`9b7d68d`](https://github.com/diaryx-org/fig/commit/9b7d68dc9a6b0643da94d17d4bfafd719b78b21e))
- SerializeError's root refusal is UnrepresentableRoot, naming no language ([`394dba6`](https://github.com/diaryx-org/fig/commit/394dba68ec4138a2408258731be091f09ab60c64))
- the shared helpers become the kit, which imports nothing of core ([`0cc9cc3`](https://github.com/diaryx-org/fig/commit/0cc9cc308a48d1034d01889e4d3d7c812107c45b))
- a language's own source imports nothing of core ([`f0b3cc6`](https://github.com/diaryx-org/fig/commit/f0b3cc6bfcb5cc2ca0faa9f523b65b91696c879c))
- the contract's declarations and C shapes move into the kit ([`5d7cbed`](https://github.com/diaryx-org/fig/commit/5d7cbed8221bb14706996c5f323f614205fa48e9))
- the JSON parser writes rows into fig's table, through a RowWriter ([`a1a24f3`](https://github.com/diaryx-org/fig/commit/a1a24f3df685b38eae56b7712dc18ebb4405850c))
- **figl** — the parser writes rows into fig's table ([`e47bebc`](https://github.com/diaryx-org/fig/commit/e47bebc0ed79b36629945f4d797532f83013f12e))
- **zon,plist** — the parsers write rows into fig's table ([`0434c25`](https://github.com/diaryx-org/fig/commit/0434c2550fdf377fd43cdb596e27ec54ad34ed1e))
- **toml** — the parser writes rows into fig's table ([`7de288f`](https://github.com/diaryx-org/fig/commit/7de288f8b0e7262b39785932d35a0bc7531cf02b))
- **yaml** — the parser writes rows into fig's table ([`8b89e02`](https://github.com/diaryx-org/fig/commit/8b89e0252eb380a325b984fed62d9bbf343157a6))
- **ini,dotenv,properties,nestedtext** — the parsers write rows into fig's table ([`15bca10`](https://github.com/diaryx-org/fig/commit/15bca10201ff750a0d44ea3e89e8072f3ae2bedc))
- materialize, lossless, flat_strip and canonical write rows ([`cc84675`](https://github.com/diaryx-org/fig/commit/cc84675e9c76492603930563c9cdd4449b6d3368))
- appending an entry to a TOML, INI, dotenv, properties or NestedText table is constant time ([`b78dd25`](https://github.com/diaryx-org/fig/commit/b78dd25dac9485c81c0bb40414041eb334d2f45f))
- **figl** — the parser writes rows as it reads, with no tree of its own ([`3daabb3`](https://github.com/diaryx-org/fig/commit/3daabb313498657ae70a195e477435d01b0e33b1))
- the child index keeps an entry's row id, not a copy of its key ([`8201443`](https://github.com/diaryx-org/fig/commit/82014430798048e0b958d2f845a8c2aabc70c9e9))
- languages and the kit import nothing of core's outside the editor tests ([`3a858d2`](https://github.com/diaryx-org/fig/commit/3a858d20f202babea1e718a823b5656743370d64))
- the kit is a package of its own, at kit/ ([`510a27b`](https://github.com/diaryx-org/fig/commit/510a27b1c79dbb9058f089bf789ac88a8e66b2cd))
- each language is a Zig package of its own, at languages/`<name>`/ ([`3fc70d5`](https://github.com/diaryx-org/fig/commit/3fc70d53806193493f8b330371b876f7edd94293))
- core's side of the language contract is src/contract/, not src/languages/ ([`3194d7a`](https://github.com/diaryx-org/fig/commit/3194d7a7d08d3485b591a3f1eb6435409e4adce8))
- **rust** — each language's crate sits in its language's package, written by hand ([`f2ac3cf`](https://github.com/diaryx-org/fig/commit/f2ac3cf8479be7cf3b67435b7f4c73016b2cdc38))
- **editor** — the write and delete guards compare with the parse the editor already held ([`bb2ea54`](https://github.com/diaryx-org/fig/commit/bb2ea54a1acf8405e69bb3692bb017f211e8e220))
- **diagnostics** — a loss walk spells a node's path only when it warns ([`76f773d`](https://github.com/diaryx-org/fig/commit/76f773d0497be3644a3924d5523c4bfa13a1f854))
- **kit** — a whole document's print table is its rows copied as they stand ([`e476a77`](https://github.com/diaryx-org/fig/commit/e476a77eee7784fdb36e5de2520575903f59e111))
- **kit** — finish walks pre-order without gathering each node's children ([`f145053`](https://github.com/diaryx-org/fig/commit/f1450538af206dcb1c54fefd8b09ef5c5af8cc15))
- **json** — the parser reserves a row for every value, key and entry ([`84fee2f`](https://github.com/diaryx-org/fig/commit/84fee2f4468b826b9d45c66fa4511f7d6ef9aeec))
- **yaml** — the printer's quoting test rules out most strings by length and first byte ([`9fe48ed`](https://github.com/diaryx-org/fig/commit/9fe48ed1271607ccdb34ddb8d2844de23a475115))
- **json** — the parser writes an entry's row before its key, so its rows are already in pre-order ([`4501813`](https://github.com/diaryx-org/fig/commit/4501813261e79ec9eea02af673a56cac92e75f9e))
- **json** — the parser pulls its tokens one at a time instead of gathering them all first ([`727bf62`](https://github.com/diaryx-org/fig/commit/727bf62c69816004b8e4fae67efc1a8c53408536))
- **editor** — the read-back guard compares two subtrees by their rows ([`cb7ac31`](https://github.com/diaryx-org/fig/commit/cb7ac3176b45ba88c9f15b30d189895752b84819))
- **json** — the parser names each row's parent as it writes it, and finish walks nothing ([`039a878`](https://github.com/diaryx-org/fig/commit/039a87844023f1a0ca1bdc65e61bcf1cddcfb2e7))

### Behavioural changes

- `{a.b = 1}` is now `{a: {b: 1}}`. It used to be one
  key, `"a.b"`, without a warning.

- an unquoted value with spaces in a fig flow object,
  `{name = Adam Harris}`, is now FigFlowMissingEquals; quote it, or write
  the object JSON-style. It used to be the string `Adam Harris`.

- `{x = 1 y = 2}` is two pairs. It used to be
  `x = "1 y = 2"` with a `flow_missing_comma` warning, which now fires only
  in arrays and JSON-spelled objects.

- a bare flow key that § 5.1 rejects (`{@x = 1}`,
  `{-x = 1}`, `{[q = 1}`) is now FigBadKey. It used to become the key as
  written.

- `fig fmt` writes a flow object as `{x=1 y=2}`, where it
  wrote `{ x = 1, y = 2 }`, and quotes a value in one that is not a single
  token.

- malformed flow values report FigFlowMissingEquals,
  FigFlowMissingComma, FigFlowEmptyElement, FigFlowMultilineString,
  FigFlowCommentedClose or FigQuotedTrailingContent where they reported
  FigUnclosedFlow or FigInvalidValue.

- setTrailingComment, getTrailingComment and deleteTrailingComment on a .properties document now fail with CommentsUnsupported (UnsupportedFormat in the bindings) instead of splicing a `#` into the value.

- @diaryx/fig/languages/zon now refuses a ZON document whose `\\` multiline string line holds a tab, a lone CR or another control byte, where it read them into the string before.

- `fig set <file> 'list[N]' <value>` replaces item N
  where it used to fail with exit 1 ("a segment of this path is not a
  mapping"); `fig set <file> 'list[-]' <value>` appends. An index past the
  end still exits 1, with a new message.

- `[-]`/`[$]` in a path given to `get`, `replace`,
  `rename`, `comment`, or before the end of a `set`/`insert`/`delete` path,
  now names the last item. It used to report "no such path".

- a `: char` annotation whose value is not wrapped in single quotes (`x: char = "A"`) is now FigTypeMismatch, where a release build read it as a char.

- TypeScript `parse`, `Document.toValue`, `toJS` and `get` on a YAML document with an alias or a `<<` merge key return the aliased and merged values; they used to throw `FigError: fig_node_kind: node kind 8 is not a value`. A cyclic alias throws a `ParseError` FigError located at the alias.

- TypeScript `Document.has` and `get` read through a merge key: `has(["site", "host"])` is true for a key merged in by `<<: *base`, and `has(["site", "<<"])` is false. They used to see `<<` as a key and the merged keys not at all.

- Rust `Document::to_value`, `fig::from_slice` and `from_yaml_str` on a YAML document with an alias or a `<<` merge key return the aliased and merged values; they used to return `Error::Internal` and "malformed document". A cyclic alias is `Error::Parse` located at the alias.

- `fig insert <file> 'list[N]' <value>` for an index
  between the first and the end inserts the value before item N and exits
  0. It used to refuse with exit 1, saying there was no insert in the
  middle of a sequence; an index past the end is still refused.

- a runtime language's table whose sequence row carries
  `item_sep` or `open` is edited item by item with that separator; the
  same fields on any other row now make the table malformed, where an
  older fig ignored them.

- fig_document_resolve_aliases and materialization merge a '<<' key only in a format whose syntax declares merge_key; a runtime-registered language that did not declare it no longer has '<<' entries merged.

- a language registered through the C ABI, Rust or the wire must be rebuilt against vtable version 3; one declaring key_style must declare a key_name renderer instead.

- TOML and ZON escape control characters in an inserted or renamed key's name (TOML "a\nb", ZON .@"a\nb") where they used to write them raw, which did not parse.

- in YAML, NestedText and every other format but figl, a node preceded on its line by '*', '>' or '+' (other than its item marker) no longer counts as owning the line, so commenting it out is refused with CommentsUnanchored.

- the Zig editor's section refusals in TOML and figl are renamed to the neutral set (NotATable and NotAContainer become NotASection, CannotDeleteTable and CannotDeleteContainer become CannotDeleteSection, TableExists and ContainerExists become SectionExists, and likewise for replace, move and reorder); C, Rust and TypeScript callers still see invalid_argument.

- a language registered through C, Rust or the wire declares section_noun as a string instead of an enum ordinal.

- a language registered through C, Rust or the wire declares a block comment delimiter where it used to choose a scanner style; FigComments' layout changes.

- in ZON, which has no block comments, a line ending in `*/` above an entry is no longer taken as part of its comment block.

- the JS twins now report the conversion warnings their compiled formats report: converting to INI, dotenv, .properties or NestedText reports booleans and numbers degraded to strings, JSON reports non-finite numbers as quoted strings, and a runtime target's comments are reported dropped where its printer writes none.

- a figl value serialized with the splice option set is now written in flow even when the caller did not also set flow.

- the Zig module's Embed.extractStream, Embed.Stream and Embed.StreamDoc are removed.

- the Zig module's error.FigUnrepresentableRoot is renamed error.UnrepresentableRoot.

- the Zig module's SerializeOptions.fig_indent is renamed indent_requested.

- a language registered through Rust or the wire declares its splice style as Splice::Escaped or "escaped"; "json_string" is no longer read, and falls back to literal.

- a language registered through C or Rust's ffi fills a FigTable through the fig_table_* calls and reads a FigTableView in print; FigNodeTable, FigNodeRow, FigRegionRow, FigMentionRow, FigCommentRow, FigDirectiveRow and free_table are gone.

- the Zig module's Runtime.NodeTable is replaced by Runtime.table (Table, Row, View, …), and documentToTable and fullTable return a Runtime.Table.

- in the Zig module, ast.serialize, serializeWith, serializeFragmentWith, serializeNode and serializeNodeWith become fig.Serialize.serialize(&ast, …) and the like, and AST.SerializeFormat becomes fig.Serialize.SerializeFormat.

- a figl language registered at runtime (not standing in for the compiled one) has a container value an editor splices written inline, where it was written as block lines that did not parse back.

- figl's Editor.insertKey on a held key returns error.DuplicateKey rather than error.FigDuplicateKey; through the C ABI the status is invalid_argument rather than parse_error.

- SerializeError has a PrintRefused member, so an exhaustive switch over it needs an arm.

- a parse failure through the C ABI and the bindings reports the byte offset the language gives, where a compiled format reported none.

- fig_language_register refuses no name a bundled language holds as compiled; it refuses it as already registered.

- a format handle is per process, assigned in registration order from 1; FIG_FORMAT_* and the integers 1-13 mean nothing, and fig_format_by_name / Format::by_name / formatByName are how a format is found.

- fig_language_register refuses any name already registered, and a language named after a format the build left out no longer takes that format's number.

- Rust Format loses its variants and RuntimeFormat; EmbedType::inner_format returns Result; an embed whose inner language is not registered is Error::UnsupportedFormat before the core is called; error text names the dialect ("as toml").

- TypeScript Format loses its members and is a branded type; Embed.detect, extract, split and retype on a parametric archetype need its inner language registered with embedded spellings; describe() includes embed on every dialect.

- fig_embed_detect reports -1 as the format of a preset container whose language is not registered.

- `fig lang list` says "built in:" and "not built in"; the --strict warning count names the dialect ("3 yaml warning(s)").

- SerializeError has PrintRefused; a registered printer's refusal crosses the contract as the SerializeError it names.

- Zig: fig.Fig, fig.Language.JSON and the other aliases, Language.detect, Serialize.SerializeFormat and deserialize.Format are gone; fig.bundle registers and names the shipped languages.

- the format `fig` is now `figl`, with no alias: `--input fig`, `-o fig`, fig_format_by_name("fig"), Format::by_name("fig") and formatByName("fig") resolve nothing.

- `--embed fenced-fig`, `md-fig`, `html-script-fig` and `html-code-fig` are fenced-figl, md-figl, html-script-figl and html-code-figl; the TypeScript EmbedType values follow.

- the build option -Dfig and the Rust crate feature `fig` are -Dfigl and `figl`; the TypeScript module @diaryx/fig/languages/fig is @diaryx/fig/languages/figl.

- the CLI's messages say "a figl value" and "figl authoring warnings".

- converting a commented document to a registered format whose dialect declares prints.comments = never, and no syntax, now warns that each comment is dropped.

- the CLI flags --embed (get, set, fmt, convert, patch), --to-embed (convert), --patch-embed (patch) and get --body are removed; passing one is a usage error (exit 2).

- a .md/.markdown or .html file is no longer opened through its frontmatter, endmatter or fenced/HTML block; like any file whose extension names no format, its whole contents are sniffed (or read as --input/--lang say), and `set` on a missing .md file is refused as an unrecognized extension.

- `fig help --oneline` no longer claims Markdown frontmatter editing.

- fig get/set/check/convert on a .md, .markdown, .html or .htm file with no -i and no configured language exits 1, naming the file a document, instead of sniffing its contents.

- the Rust crate no longer exports Embed, EmbedType, Extracted, Region, fig::split or fig::detect; code using them stops compiling. Use twig to locate the block and fig::Editor to edit its contents.

- fig-sys no longer declares the fig_embed_* functions, FigEmbed, FigEmbedContainer, FigRegion or FigSpan.

- @diaryx/fig no longer exports Embed, EmbedType, or the Region, Span and EmbedSpellings types, and describe(lang) no longer includes `embed` on a dialect.

- the C ABI no longer exports fig_embed_extract, fig_embed_detect, fig_embed_retype, fig_embed_open, fig_embed_open_or_init, fig_embed_destroy, fig_embed_render, fig_embed_replace_body, fig_embed_set or the per-op fig_embed_* edit and comment functions; fig.h no longer declares FigEmbed, FigEmbedContainer, FigRegion or FigSpan. A host linking them fails to link.

- the Zig module no longer has fig.Embed or bundle.embeddable.

- FigDialectDesc ends at prints_comments; the embed_* fields a 6.0 pre-release added are gone.

- a helper must speak JSON-RPC 2.0; a helper built against 5.x (the {"op":…}/{"ok":…} wire) is refused with a message saying to rebuild it.

- fig::helper::handle and @diaryx/fig's handle answer JSON-RPC requests, returning {"jsonrpc","id","result"} or {"jsonrpc","id","error":{code,message,data}}; describe's result is the description itself, parse's the table, print's and render's the text, split_stream's the documents.

- Zig fig.Wire.Transport has exchangeFn returning the response line instead of callFn returning a std.json.Value; fig.Wire.call(transport, arena, method, params) sends one request; parseLine, okField and failureMessage are gone.

- building fig with the helper wire (the default) fetches the zigjr package; -Dhelper-wire=false does not.

- the TypeScript in-process host call answers a request for an unregistered language as a host failure, not a {"ok":false} line.

- the Zig module's AST has no nodes, node_tags, node_anchors, anchors, node_comments or owned_strings; read through node(id), nodeCount, tagOf, tagFact, anchorOf, anchorIterator and comments.

- the Zig module's Document is { source, ast }; spans through spanOf/span, markerSpan, sepSpan, itemSep, regionsOf (table.SpanFact) and mentionsOf (an iterator).

- Runtime.tableToDocument takes *Table and adopts it, leaving it empty; viewToDocument borrows the view and takes no Spans rule.

- a shipped Zig language module's parse returns the adopted Document; parseTree returns the parser's ParseTree, and a parser's parseAbstract returns the row-backed AST.

- a YAML tag under a %TAG directive that rebinds !! or ! is resolved by the directive in every path, so `%TAG !! tag:example.com,2000:app/` makes !!int custom; a YAML tag spelled `!<tag:yaml.org,2002:int>` reads back as its text, resolved to int, and prints as written.

- the trees materialize, Lossless and FlatStrip return own their strings.

- the Zig module no longer exports ParseTree; a shipped language module has parseTable(parser, input, t, out) in place of parseTree, and its parser parseInto / parseWithReportInto / parseCollectingInto writing into a Table.

- @diaryx/fig/languages/`<name>` is a native module, not a JavaScript Language; the JavaScript languages are at @diaryx/fig/js/<name>.

- @diaryx/fig/languages/json5 is JSON's module, so registering it returns JSON's format; JSON5's is formatByName("json5").

- init(...languages) also compiles the given languages' modules, which a browser main thread needs before registerLanguage.

- the package root has a fig-source export condition, resolving to src/index.ts.

- libfig carries no language: a C program links libfig_`<name>`.a beside it and calls fig_<name>_register before using the format.

- fig-sys and fig have no language features (json, yaml, toml, zon, figl, ini, dotenv, properties, plist, nestedtext); depend on fig-`<name>` and call fig_<name>::register().

- fig::from_yaml_str and fig::to_yaml_string are fig_yaml::from_str and fig_yaml::to_string, behind fig-yaml's serde feature.

- fig::helper reads the wire through the json language, which the helper registers (fig_json::register()).

- a source build for an Apple target is stamped with MACOSX_DEPLOYMENT_TARGET or IPHONEOS_DEPLOYMENT_TARGET, else 11.0 (macOS) or 14.0 (iOS), rather than the build machine's version.

- the `@diaryx/fig/languages/<name>` subpaths are removed; import a language's module from `@diaryx/fig-<name>` instead.

- converting or printing a `0x`/`0o`/`0b` integer wider than 128 bits into a format without that spelling writes its decimal, where it wrote the lexeme as it stood.

- `@diaryx/fig/kit/number` and `@diaryx/fig/kit/datetime` need WebAssembly; a host without it supplies its own `kit/zig.js`.

- `@diaryx/fig/kit/number`'s `canonical` writes a radix integer whose decimal is longer than 512 digits as it stood, where it converted any length.

- a fig object's key in the tree-sitter grammar is a `keypath` node, not a bare `bare_key`; a query matching `(flow_object key: (bare_key))` matches `(flow_object key: (keypath (key)))` instead.

- deleting the last item of a YAML block sequence
(removeSeqItem, `fig delete`, a patch that empties a list) leaves

- inserting a key into a figl `=`-spelled flow object now writes a space between members rather than `, ` (`{a=1}` + b gives `{a=1 b=2}`; `{ x = 1 }` + y gives `{ x = 1 y = 2 }`), and into an empty one writes `{a=1}` rather than `{ a = 1 }`.

- `setTrailingComment` (`fig comment --inline`) on a
  CRLF line writes `a: 1 # note\r\n` where it wrote `a: 1\r # note\n`;
  replacing or deleting a trailing comment on a CRLF line keeps the CR.

- `setTrailingComment` (`fig comment --inline`,
  fig_editor_set_trailing_comment) on a ZON `\\` multiline string now
  fails with CommentInsideValue / FIG_STATUS_UNSUPPORTED_OPERATION and
  leaves the file unchanged. It used to succeed, appending the comment to
  the string's text.

- `fig patch` drops a trailing comment that would land
  on such a line and counts it dropped, where it used to write it into the
  string.

- inserting a comment or entry after a .properties line that ends in a `\` continuation, or moving such an entry before another, now writes a blank line between them; the value is no longer joined.

- FigSyntax gains a trailing `uint8_t line_continuation` (size-gated; 0 for none).

- A response written by `fig::helper` lists its object keys in
  alphabetical order (`{"id":…,"jsonrpc":"2.0","result":…}`), where fig's json
  printer kept the order they were written in. JSON-RPC readers, fig's
  included, do not depend on the order.

- `fig set f 'list[-]' x` and `fig set --seq f list x …` create `list` when it is absent or null, where they exited 1; in INI, dotenv, .properties and NestedText they still exit 1, with a message saying the format has no empty list.

- `Editor.setSequence` on an absent or null path starts the sequence, where it returned `NotFound`.

- figl output writes a non-finite float as `key: float = inf` (`-inf`, `nan`) instead of its source lexeme, and a list or object holding one prints in block, not flow.

- a figl editor replace or insert of an infinite or NaN float, and serializing one as a figl scalar or flow fragment, now fails (unsupported format; in Rust, a message naming the annotated spelling) instead of writing text that reads back as a string.

- a delete (or `comment --delete`) whose result would not parse exits 1 with "the edit at `<path>` in <file> was refused" instead of panicking (or, for `comment --delete`, blaming an empty comment text and exiting 2).

- `setTrailingComment` (`fig comment --inline`) on a YAML
  block scalar writes the comment on its header line (`a: | # note`), where
  it wrote it at the end of the next entry's line. `getTrailingComment` and
  `deleteTrailingComment` on a block scalar read and remove the header
  line's comment, where they read and removed the next entry's.

- deleting the first item of a YAML block sequence
  that starts on its parent item's marker line (`- - a` over `  - c`,
  removeSeqItem, `fig delete`) now leaves `- - c`; it used to delete the
  parent's marker too, and the parent with it. Deleting the first key of
  a mapping on an item's marker line (`- k: v` over `  j: w`) leaves
  `- j: w` where it used to leave `  j: w`, and deleting the only key
  (`- k: v`) leaves `-`, a null item, where the item used to vanish.

- deleteKey and removeSeqItem (`fig delete`, the
  bindings' delete and remove-item calls) refuse with
  DeleteChangedOtherValues (FIG_STATUS_UNSUPPORTED_OPERATION) and leave
  the source unchanged when the result would read back with any value
  other than the deleted one changed. They used to report success.

- in figl, deleting the only element of a block list (removeSeqItem, deleteContainer on the element, `fig delete xs[0]`) leaves `xs = []` with the key's comments, instead of removing the key; nested and dotted keys keep their path (`a[].b = []`, `> b = []`).

- in figl, deleting the only item of a root list now fails with NoEmptyRootList and leaves the file unchanged, where it used to leave an empty document (an empty mapping).

- a YAML line under compact nested items (`- - - a`
  over `  - d`, `- - k: v` over `  - c`) now reads as an item of the
  container that begins at its column, as YAML reads it; it used to read
  as part of the innermost container, or fail to parse. A line indented
  between two such items (`   - d` under `- - - a`) is now a parse error.

- a YAML plain scalar now continues onto a
  more-indented line that begins `- ` or `? ` (`- a` over `  - b` reads
  `["a - b"]`, was `["a", ["b"]]`), measured against its collection's
  column, not its line's indent; a line indented under a sequence item or
  explicit key whose node is complete, or a `: ` inside a plain scalar's
  continuation, is now a parse error where it used to read as a nested
  node.

- `setTrailingComment` (`fig comment --inline`) on a figl
  `'''`/`"""` string writes the comment on its opener line (`x = ''' # note`),
  where it wrote it after the closing quotes; `getTrailingComment` and
  `deleteTrailingComment` read and remove the opener line's comment, and no
  longer see one after the close.

- `FigSyntax` gains `opener_line_comment` at its end, and
  the Rust `Syntax` struct gains the `opener_line_comment` field.

- `fig fmt` and conversion into figl print a multi-line
  string's trailing comment on its opener line (`x = ''' # note`), where they
  printed it after the closing quotes.

- deleting a key under a dotted key in a TOML inline table (`it.a.b` in `it = { a.b = 1, c = 2 }`) deletes that entry, where it was refused with DeleteChangedOtherValues.

- deleteContainer on a dotted table inside a TOML inline table (`it.a`) deletes only the entries that write it, where it deleted the whole line the inline table stands on.

- deleting the only element of a TOML array of tables (`fig delete f.toml 'x[0]'`, `deleteContainer`) leaves `x = []` in the parent table's body, where it removed `x`.

- a figl parse's node table carries `index` and `append` mentions on sequence elements; a reader that assumed every mention is `header` or `entry` sees two more kinds, and the C ABI's fig_table_mention accepts 2 and 3.

- deleting the first element of a figl `+` chain rewrites the next element's `+` as the header (`r[]`), where it was refused.

- deleting a figl element addressed by index renumbers every later element's index (`ys[1]` becomes `ys[0]`), where it was refused.

- `set`/`insertKey` of a new key under a TOML table made only by dotted keys (`a.b = 1`, or `a.b = 1` inside an inline table) writes a dotted key or an inline-table entry in that table (`a.y = 1`, `{ a.b = 1, a.y = 1 }`), where it wrote a bare key into the enclosing table.

- `set`/`insertKey` of a new key under a TOML table named only by headers below it (`[a.b]` alone, then `a.y`) writes `a.y = 1` in the nearest body with a header, where it was refused with `ImplicitSection`.

- an insert into a TOML inline table or fig flow object whose last member is a dotted key goes after that member, where it was refused (TOML) or panicked (fig, `p = {a.b=1, c=2}`); `set` into a dotted table inside a fig flow object writes a key path entry, where it was refused.

- replacing a YAML block-sequence item (replaceValAtPath,
  `fig replace`, the bindings' replaceValue) with a multi-line value writes
  its later lines under the item's content column; they used to be written
  at column 0, which read back as other keys or failed to parse. Replacing
  a block-scalar item writes the line break after the new value.

- replaceValAtPath, set, replaceValAtPathFollowing,
  insertKey, insertNamedKey, appendToSeq, prependToSeq, insertItem,
  setAppend, moveKey, moveItem, reorderKeys, reorderItems, moveContainer
  and reorderContainers (`fig replace`/`set`/`insert`/`append`/`move`, the
  bindings' matching calls) refuse with EditReadsBackWrong
  (FIG_STATUS_UNSUPPORTED_OPERATION) and leave the source unchanged when
  the result would read back with another value changed, or the written
  value reading as something other than the value asked for. They used to
  report success. `--raw` text that adds entries beside the one replaced
  is refused the same way.

- `set` on a missing key whose insert is refused this
  way reports EditReadsBackWrong, not the replace's NotFound.

- replacing a ZON enum-literal value (`.a = .fast`) now
  writes the new value in its place (`.a = true`). It used to be refused,
  and before 5a1a559 it wrote `.a = .true`.

- a ZON enum literal's node-table span now starts at its
  `.` rather than at its name; its text is still the bare name.

- replacing an empty dotenv value followed by a comment
  (`A= #c`) now writes `A= 42 #c`. It used to be refused, and before
  5a1a559 it wrote `A= 42#c`, which reads as the value `42#c`.

- a YAML plain scalar followed by a tab and a comment
  (`a: 1\t# c`) now reads as `1`; it used to read as `1\t`.

- replacing an empty tagged or anchored YAML value
  (`t: !!str`) now writes the new value in place of its properties
  (`t: x`), as replacing a written tagged value does. It used to be
  refused, and before 5a1a559 it wrote `t: x !!str`. Its node-table span
  now covers the properties.

- replacing an explicit key's value (`? k` / `: v`) now
  keeps the `?` line and writes `: 42` under it. It used to be refused.

- the JavaScript YAML twin now reads a line under
  compact nested items (`- - - a` over `  - d`, `- - k: v` over `  - c`)
  as an item of the container that begins at its column, as the compiled
  format does; it used to read it into the innermost container, or fail.
  A line indented between two such items is now refused.

- `setTrailingComment` (`fig comment --inline`) on a figl
  flow list or object over several lines whose `[`/`{` line holds no
  element writes the comment on its opener line (`xs = [ # note`), where it
  wrote it after the close; `getTrailingComment` and `deleteTrailingComment`
  read and remove the opener line's comment, and no longer see one after
  the close.

- inserting a key into a figl element or container followed by a header that opens a child of it (`c[].p[]`, `a.b`) writes the key among its own keys, where it was refused.

- the figl node table records a key that a header line reaches through a container written on an earlier line as a `header` mention, where it was an `entry`.

- moving or reordering the elements of a figl `+` chain writes the header on whichever element comes first and a `+` on the rest, where it was refused.

- moving or reordering figl elements addressed by index renumbers every line that addresses them, where it was refused.

- a default build — the CLI, fig-lsp, libfig's bundled
  languages and a Zig consumer's `fig` dependency — includes plist: `.plist`
  files and `-i plist` are read, edited and converted, `fig lang list` shows
  it under "built in", and `fig_format_by_name("plist")` finds it. Such a
  build used to refuse a `.plist` file unless a `languages.figl` language
  served it; one named `plist` is now refused as a taken name, and the
  built-in serves its files.

- `fig patch` leaves a number alone when the patch holds
  the same value spelled differently (`31` against `0x1F`, `1.5` against
  `1.50`, `.inf` against `Infinity`), where it used to rewrite it in the
  patch's spelling. `--seq union` likewise treats such items as present.

- `fig.Patch.nodesEqual` compares numbers by value, so
  it reports `0x1F` and `31` equal; an integer and a float are still unequal.

- renaming a figl key or container (`fig rename`, `renameKey`, `renameContainer`) rewrites every dotted line and header that names it, not only the first, and a rename it used to refuse because the half-renamed document did not parse (`spec` in `spec.containers[]` / `spec.containers[].ports[]`, `xs` in `xs[0] = a` / `xs[1] = b`) now succeeds.

- a figl document's table (`fig lang table`, the wire) lists a mention for every key that re-opens a block container mid-path, a dotted line's or a header's.

- renaming a key whose closing quote stands against its separator (YAML `{ "k":v }`) to a key with no closing quote now writes a blank after the separator (`{ renamed: v }`, was `{ renamed:v }`). Through the library's `replaceKeyAtPath`, which takes key syntax, a JSON5 `{"a":1}` renamed to `b` likewise gets `{b: 1}`; the CLI's `rename` quotes a JSON5 key and is unchanged there.

- A YAML file that opens with a BOM reads its first key
  without it: `fig get -o json` gives `{"a":1}`, where it gave
  `{"﻿a":1}`, and `set`, `replace`, `rename` and `delete` on that
  key edit it in place, where `set` appended a duplicate and the others
  said there was no such path.

- In every editable format, an edit at the start of a
  BOM-led file's first line leaves the BOM first: a leading comment on the
  first key goes after the BOM, not before it, deleting the first key
  keeps the BOM, and a key inserted above a TOML or INI file's first
  header lands after it, where the edit used to fail to parse. A comment
  above the first key now moves and deletes with it, as it does without
  a BOM.

- The TypeScript binding's Editor.source() keeps a BOM
  the document opened with, and every string it reads back from the core
  keeps a leading U+FEFF, where it dropped it.

- deleting a key written more than once in dotenv, INI,
  .properties or plist (`fig delete`, deleteKey) removes every spelling,
  so the key no longer reads. It used to remove the first and leave the
  key reading with a later spelling's value.

- renaming such a key (`fig rename`, renameKey,
  replaceKeyAtPath) renames every spelling. It used to rename the first,
  leaving the key under both names and the old name with the later value.

- deleting an INI section (`fig delete`, deleteContainer)
  also removes the later spellings of a key the section repeats. They used
  to be left behind and read as entries of the section before them.

- on a tier-1 target the language crates (`fig-json`,
  `fig-yaml`, …) link a prebuilt library and no longer run Zig, so a
  build without Zig succeeds where it used to fail.

- inside a checkout of fig, `fig-sys` builds the core
  from source even when a payload's git-ignored `lib/` holds an archive,
  which it used to link however old; `FIG_SYS_PREBUILT=1` links it.

- the aarch64-apple-darwin payload names macOS 11.0 as
  its minimum, as `fig-sys`'s source build does, rather than Zig's default.

- an error from reading a typed value below the root
  is `Error::At`, wrapping the error that used to be returned, and its
  text leads with the path. A match on `Error::MissingField { .. }` (or
  any mapping error) for a nested field no longer matches; match on
  `e.innermost()` instead.

- `fig-json` and every other language crate turn on
  `serde`, and with it fig's `serde` feature, by default; `fig-yaml`'s
  was opt-in. A build that wants no serde sets `default-features = false`.

- `fig replace`, `fig set` and `fig patch` keep the
  quote style of a string they replace with a string: a YAML `"debug"`
  set to `info` becomes `"info"` (was `info`), a YAML `'x'` stays
  single-quoted, a TOML literal `'…'`, `'''…'''` or `"""…"""` keeps its
  delimiters, and a JSON5 `'…'` stays single-quoted (was `"…"`). Where the
  old style cannot hold the new string, the printer's spelling is written
  as before. `--raw` values are unchanged.

- a C or Rust program linking fig's static libraries
  for a musl target as a PIE now links; it used to fail at the linker.

<!-- git-cliff:end -->

## v5.2.0 — 2026-10-03

### Added

- **rust** — ship prebuilt fig-sys payloads for iOS device and simulator ([`c0cec25`](https://github.com/diaryx-org/fig/commit/c0cec25fe8ae5a14ada4998f13d22ee2df29515e))

### Fixed

- **c-api** — build for iOS under Zig 0.17.0 ([`0e61eef`](https://github.com/diaryx-org/fig/commit/0e61eef47dc5c3dc7f9a41d4f4d2d2a43c46a6bc))

### Behavioural changes

- a default-feature build of fig for aarch64-apple-ios or aarch64-apple-ios-sim links a prebuilt archive with a minimum OS of iOS 14.0 and no longer needs a Zig toolchain, where it used to compile the core from source with Zig.


## v5.1.3 — 2026-10-03

### Breaking

- build with Zig 0.17.0 ([`8ca918d`](https://github.com/diaryx-org/fig/commit/8ca918d7d07ca34e2597c3501083e8dcfd1585fb))

### Behavioural changes

- building fig from source needs Zig 0.17.0 — `zig build`,
  `zig fetch` consumers, and the fig-sys crate with any non-default language
  feature set. Zig 0.16.0 refuses the tree at the first file. The fig-sys
  crate with default features links the prebuilt payload and needs no Zig,
  as before.


## v5.1.2 — 2026-10-01

### Fixed

- **core** — spell an infinite or NaN float in each format's own words ([`8e9f561`](https://github.com/diaryx-org/fig/commit/8e9f5610beacea40d350ab3c9ec79aeabbf723fe))
- **rust** — say a refused edit once, and name what it refused ([`53fb5ec`](https://github.com/diaryx-org/fig/commit/53fb5ec37e5a6883b4ece69a714c09df4da6bacb))

### Behavioural changes

- an infinite or NaN float serialized or converted to
  TOML is written `inf`, `-inf` or `nan`. It used to be written in its
  source's spelling, e.g. `.inf` from YAML or from the Rust and TypeScript
  bindings, which TOML cannot parse; an editor replace with one used to
  fail with a parse error, and now succeeds.

- the same float to ZON is written `inf`, `-inf` or
  `nan`; to YAML `.inf`, `-.inf` or `.nan`; to JSON5 `Infinity`,
  `-Infinity` or `NaN`. A TOML or ZON `inf` used to reach YAML as `inf`,
  which YAML reads as a string, and a YAML `.inf` used to reach JSON5 as
  `.inf`, which JSON5 cannot parse.

- the same float to JSON or JSONC is refused:
  `fig_value_serialize`/`fig_document_serialize` return
  `unsupported_format`, `fig convert` exits 1 with "this output format has
  no way to write infinity or NaN", and `gron` refuses too. They used to
  write `0.inf` or `0.nan`, which no JSON reader accepts.

- `Editor`/`Embed` value, key, comment and structural
  edits the document would not parse with still return `Error::Parse`,
  but it displays as one sentence: "the new value would not parse as
  TOML" (or "the new key …", "the new comment …", "the edited document
  …"), and `ParseError::message` holds that sentence. It used to display
  as `failed to parse input: failed to parse input`.

- `Editor::open` on input that does not parse displays
  the core's diagnostic, e.g. `failed to parse input: UnexpectedToken`,
  and `ParseError::message` holds it. `Embed::open` and the other bare
  `PARSE_ERROR` paths display `failed to parse input` once. Both used to
  display `failed to parse input: failed to parse input`.

- `Value::serialize`, and every editor or embed value
  edit, given an infinite or NaN `Value::Float` for JSON or JSONC returns
  `Error::Message` reading "JSON has no way to write an infinite number"
  (or "… NaN", or "JSONC …"). It used to succeed with `0.inf` from
  `serialize`, and fail an edit with `Error::Parse`. TOML, YAML, JSON5 and
  ZON now accept such a float (the previous commit).


## v5.1.1 — 2026-10-01

### Fixed

- **editor** — reorder and move the members of a flow mapping, so JSON objects reorder ([`2ae850b`](https://github.com/diaryx-org/fig/commit/2ae850b1871c38837c4b8ab6e791b1402c0cee65))

### Behavioural changes

- `reorderKeys` / `fig_editor_reorder_keys` /
  `Editor::reorder_keys` on a flow mapping — any JSON, JSONC or JSON5
  object, a YAML `{…}` mapping, a TOML inline table — now reorders it. It
  used to fail with `NotAMapping` (`invalid argument`).

- `moveKey` / `fig_editor_move_key` / `Editor::move_key`
  between two members of a flow mapping now moves the member and keeps the
  separators valid. It used to fail on a reparse error when the last member
  of a multi-line JSON object was involved, and to succeed without changing
  anything on a packed one-line mapping.


## v5.1.0 — 2026-10-01

### Added

- **editor** — flow members joined by a separator the format names, and a root with no braces ([`13ab3b4`](https://github.com/diaryx-org/fig/commit/13ab3b44605bfaad9cf1c4f65fafac82c48ed8e0))
- **cli** — accept figl as a spelling of fig, as yml is of yaml ([`292a7be`](https://github.com/diaryx-org/fig/commit/292a7beaae1857083d7f9dab3fd7dc46bf57cbf9))

### Behavioural changes

- deleting a block-mapping entry that shares its line
  with a sibling — which a runtime language's table can describe and no
  compiled format produces — now removes that entry alone. It used to
  delete the whole line, sibling included, and report success.


## v5.0.0 — 2026-09-27

### Breaking

- **cli** — arguments are parsed strictly, and a missing comment exits 1 ([`be6bb06`](https://github.com/diaryx-org/fig/commit/be6bb062d045a0a52dd0fdcd0a6646436757c192))
- **cli** — a value argument is a fig value, and get prints a scalar as its text ([`0885a42`](https://github.com/diaryx-org/fig/commit/0885a42a315039c785c5d1748a5cd40d3a5dff8c))
- **cli** — exit codes are written down — 1 for the document, 2 for the command line ([`d3dbbef`](https://github.com/diaryx-org/fig/commit/d3dbbefcddacf6ba806b285d31cda616265ec9fc))
- every renderer takes one request, which names the container its fragment goes into ([`c17a0ab`](https://github.com/diaryx-org/fig/commit/c17a0abfff7c5082c91fd96a752522c43f1644e8))
- every language record carries its size, lossless is a bitmask, and extended scalars have a node kind ([`37ed739`](https://github.com/diaryx-org/fig/commit/37ed739ca2e9c06dbd93814e4d32659e6299845b))
- **cli** — split `edit` into `replace` and `rename` ([`f25cf60`](https://github.com/diaryx-org/fig/commit/f25cf60891258a9ead8200a0236cf34b762d3494))
- **cli** — drop the frontmatter-fig/-json/-toml archetype names ([`2fbe776`](https://github.com/diaryx-org/fig/commit/2fbe77665f9320eb9a1b74f66f4417c14a5366ba))
- **cli** — `help` and `version` refuse arguments after them ([`727de4a`](https://github.com/diaryx-org/fig/commit/727de4adf9510f4ffb36908c7b88af201cd2bfe3))
- **cli** — a mid-sequence `insert` is a refused edit, exit 1 ([`a6d409e`](https://github.com/diaryx-org/fig/commit/a6d409e80e7eb67aceaf2556a9edff74e01a82f4))
- `.figl` is fig's only extension; `.fig` is no longer the format's ([`83ee085`](https://github.com/diaryx-org/fig/commit/83ee085f809aeea8fbe8ed24726b4569301a1e29))
- one version and one v* tag for every artifact, at 5.0.0 ([`94b2eab`](https://github.com/diaryx-org/fig/commit/94b2eab3fe4291097b07453362b83557cf2b2e92))
- **ts** — enums are erasable `as const` objects, not TypeScript enums ([`bf61d06`](https://github.com/diaryx-org/fig/commit/bf61d06a6ed38841bd28a90ed14a8f863fbe5cb8))
- **ts** — `EmbedType` names the CLI's archetypes; embed locators are statics ([`d471dcf`](https://github.com/diaryx-org/fig/commit/d471dcf1c74df341678d7fcfd4d57d1a8939404d))
- **ts** — `stringify` is the one value serializer; export `DirectiveRow` ([`e8575cf`](https://github.com/diaryx-org/fig/commit/e8575cf1b0f9da37d7960a5a97fa4e23e0601196))
- **ts** — every open reports the core's parse message; `FigError` gains `op` ([`1da833e`](https://github.com/diaryx-org/fig/commit/1da833e17bac402af574c472705d1381bd61a125))
- **ts** — Node 22 floor, `./package.json` export, one copy of the wasm payload ([`a2d3a26`](https://github.com/diaryx-org/fig/commit/a2d3a26e543a58188c4f767692b14011b99e7a59))
- **wasi** — @diaryx/fig-wasi requires Node 22, as @diaryx/fig does ([`6163156`](https://github.com/diaryx-org/fig/commit/616315668c55823d88062fb8aa63349d581fa46f))
- **rust** — FigStatus constants are FigStatus, not c_int ([`e875311`](https://github.com/diaryx-org/fig/commit/e8753114fd007dac5703e8d847d94e282977c6e7))
- **rust** — the runtime-language contract types are non_exhaustive ([`cec616a`](https://github.com/diaryx-org/fig/commit/cec616abf33ee70ae0cc21b58ae112e032d41563))
- **rust** — Error::Language carries a LanguageFailure, Error::Static folds into Message ([`3d3fa8a`](https://github.com/diaryx-org/fig/commit/3d3fa8a40b9a98be6e2219579fa9f318c3980a09))
- **rust** — Format no longer converts to fig_sys::FigFormat ([`39c4e5a`](https://github.com/diaryx-org/fig/commit/39c4e5a8c5e0b8318ee8e61f44324d5a0bfba1c7))
- **rust** — one set of editor method names, on Editor and Embed alike ([`677767c`](https://github.com/diaryx-org/fig/commit/677767ca6dd8168b3db352eb2adfcc5e7dcf0415))
- **rust** — embeds split into before, content and after; the one-sided body is gone ([`df73366`](https://github.com/diaryx-org/fig/commit/df73366e77d3a13b5cab6a87e7bd61376275bca8))
- **rust** — from_str and to_string become from_yaml_str and to_yaml_string ([`aedaf7a`](https://github.com/diaryx-org/fig/commit/aedaf7aa40603bf4d5680ff3ca947172c54aa0ac))
- **rust** — declare rust-version 1.88 for every crate in the workspace ([`ac8aec6`](https://github.com/diaryx-org/fig/commit/ac8aec683937fbb47a44d427b443e33dab74c182))
- **rust** — EmbedType variants are the CLI's archetype names ([`d76a81b`](https://github.com/diaryx-org/fig/commit/d76a81b54c59bf437c401e30ca34fb76f2847083))
- **ts** — editor method names match the Rust binding's ([`40275c5`](https://github.com/diaryx-org/fig/commit/40275c5170421f1b80510c0987f71e36af46a14a))
- **typescript** — the published module compiles no format in; a caller imports and registers the formats it uses ([`eb8e7d6`](https://github.com/diaryx-org/fig/commit/eb8e7d63ffe2968146fe2ace6491f4cffc5aa306))
- **wasi** — fig-wasi runs @diaryx/fig's JavaScript languages in-process instead of compiled formats ([`842dfda`](https://github.com/diaryx-org/fig/commit/842dfda20191f843b4f8e68e93a5a8039e4ef3b2))

### Added

- **cli** — patch into a runtime language ([`78bc944`](https://github.com/diaryx-org/fig/commit/78bc944328fda6a92a0a851baa205a7bb98eb744))
- loss diagnostics for a runtime target, and a value refused where the target cannot hold it ([`3693431`](https://github.com/diaryx-org/fig/commit/3693431206fb1cfe8a9f948f6e10fdedf1edf172))
- **cli** — `patch` takes --strict, --no-warnings and --strip-comments; `check` takes --strict ([`1d77f6e`](https://github.com/diaryx-org/fig/commit/1d77f6e7f4dc334f3ca2c9245173e71e8425f984))
- **rust** — Span is Hash and converts to and from Range`<usize>` ([`44ee39d`](https://github.com/diaryx-org/fig/commit/44ee39d8bd6c43e7e4992ec8e05508d8b5d879ac))
- **cli** — `version` takes `--help`, and `help <action>` names that action's help ([`5fede28`](https://github.com/diaryx-org/fig/commit/5fede286b2c350a363cc7a497369caed1ae8cded))
- **core** — a runtime language named after a compiled-out format stands in for it ([`d360e7b`](https://github.com/diaryx-org/fig/commit/d360e7bb572f6cd2189424c65d67e0a5f5614202))
- **cli** — a format compiled out of the build reaches the configured language standing in for it ([`f12b961`](https://github.com/diaryx-org/fig/commit/f12b961bb6c5157f5d350e5503d55d13a2d6228e))
- **typescript** — ship a JavaScript language for each compiled format, and the kit they are written with ([`35edfe3`](https://github.com/diaryx-org/fig/commit/35edfe33de8a1c617f2aef9bdc8f20d1874b8ea4))
- **core** — a runtime printer is told `flow`, and a runtime language without sections has no container ops ([`e9b3cdf`](https://github.com/diaryx-org/fig/commit/e9b3cdf565cde5435128029c71d82ec27bc3b18e))
- **core** — content sniffing tries the language standing in for a compiled-out format ([`edc8c20`](https://github.com/diaryx-org/fig/commit/edc8c20a6dd9c9643e67fbdc754e65709a6a4090))
- **cli** — a WASI build made for a host asks it for the formats it did not compile in ([`3728b60`](https://github.com/diaryx-org/fig/commit/3728b60adc242955a672be18e8c6ee8f1a175759))

### Fixed

- **toml** — a table spliced as a value prints as an inline table ([`edf838b`](https://github.com/diaryx-org/fig/commit/edf838bf14ba4da7f27c1c806867ffbaff427d42))
- **cli** — review of CLI 5 — parse errors not taken for values, spaces kept, the key blamed ([`4dfe30f`](https://github.com/diaryx-org/fig/commit/4dfe30fe03c3f2bd7aa0a7d012ff104e38e2b196))
- **fig-sys** — build for aarch64 and x86_64 Android ([`d75bd67`](https://github.com/diaryx-org/fig/commit/d75bd677febc666b164704128bbd330bac992c7e))
- the lossy strips and the $fig envelope keep tags, anchors and %TAG directives ([`b26ce0b`](https://github.com/diaryx-org/fig/commit/b26ce0bafbc2c40cc2470c2c6e7c3f2df3e52e7e))
- **cli** — an unknown embed archetype is not followed by the list of formats ([`2b144c9`](https://github.com/diaryx-org/fig/commit/2b144c9957bafe11ef5802627599d9444e682290))
- a rename refuses a path with no key, and a name the mapping already holds ([`fe4d471`](https://github.com/diaryx-org/fig/commit/fe4d4714371de6fa6fe211d0c8dc9e1f14f36859))
- **embed** — replace_body refuses a mid-document block, which has no one body ([`5bfc1de`](https://github.com/diaryx-org/fig/commit/5bfc1de58648e030f5693191e4e7018a3e5e3d1c))
- **typescript** — the TOML and INI languages write splice text as their compiled printers do ([`a6c6c87`](https://github.com/diaryx-org/fig/commit/a6c6c87fb9a460c8258ca326dc2685491db8b87e))

### Behavioural changes

- an argument beginning with `-` that is not a flag of
  the action (other than `-` and a negative number) exits 2 with a usage
  error naming it, where it was read as a file, path or value; a file or
  value that begins with `-` goes after `--`.

- a positional past an action's last (`fig get f a b`,
  `fig set f a 1 2`, `fig delete f a b`) exits 2, where it was ignored.

- `fig lang` with an unknown verb, or `lang check` /
  `lang table` without its name, exits 2, where it printed help and
  exited 0.

- `fig comment --get` on a path with no such comment
  prints nothing and exits 1, where it printed a blank line and exited 0.

- `fig_value_serialize_opts` (and the bindings'
  `value_text`/`valueText`) with `splice` set render a TOML mapping as an
  inline table, where they rendered `[header]` sections; `fig patch` of a
  new table into TOML writes `k = { … }` where it was refused.

- a value argument to `set`/`insert`/`edit` in a JSON,
  JSONC or JSON5 file that is a number, boolean, null or bracketed
  structure is written as that value, where it was written as a string.

- in YAML, TOML, ZON, fig and the flat formats, a value
  argument is read as a fig value and written in the file's syntax, where
  it was spliced verbatim: `a: b`, `Yes`, `007`, `'q'` are strings, and
  `[1, 2]` / `{a = 1}` are rendered as the format writes a sequence or a
  mapping. `--raw` restores the verbatim splice.

- a bare word into TOML is written as a string, where it
  was refused.

- a value argument that is not a fig value (`[1, 2`)
  exits 2 with a diagnostic on the argument before the file is opened; a
  value the format cannot hold (`null` into TOML, a sequence into dotenv)
  exits 1 with a sentence, where both were a reparse failure of the spliced
  text.

- `fig get <file> <path>` on a scalar, without `-o`,
  prints its text and one newline in every format — a string unquoted, a
  number/boolean/null as fig spells it — where it printed the format's own
  spelling (and, from JSON, no newline).

- `edit --key` with `--string` or `--raw` is a usage
  error (exit 2).

- `fig get`, `fmt`, `convert`, and `patch` (on its patch
  document) on a file that does not parse exit 1, where they exited 2.

- `--strict` aborting on a parse-time warning exits 1,
  where it exited 2.

- `fig comment` on a strict JSON file, `fig convert
  --to-embed` on a host with no detectable region or a mid-document one,
  and `fig lang check` on a file it cannot read exit 1, where they
  exited 2.

- an edit whose value, read as a fig value, the
  document will not take exits 1, where it exited 2; `--raw` text, a key
  or a comment that does not parse still exits 2.

- a path argument that does not parse exits 2 with a
  message, where it exited 1 with an error trace.

- a value argument to `set`/`insert`/`edit` with a
  space or tab at either end is written as that string, spaces kept.

- `insert`/`set` of a key the format cannot spell
  (`'bad key'` in dotenv) report the key and exit 2.

- `fig patch <target> <overlay>` where the target is in a runtime language (one named in `languages.figl`) now merges the overlay, where it used to exit with `error: UnsupportedRuntimePatch`.

- `fig get`/`convert` into a format without null (TOML, or a runtime language declaring `lossless` without `null`), and anything else that goes through the `$fig` envelope or the flat-format strip (dotenv, INI, `.properties`, a runtime language with `max_mapping_depth`), now hand the printer the source's type tags, anchors and `%TAG` directives. A YAML target writes `!!str 1`, `!custom x` and the `%TAG` line where it used to drop them, and a runtime printer receives the `tag` it recorded on each row.

- `fig get`/`convert`/`fmt` to a runtime language print warnings on stderr for what its declaration says it drops or degrades, and `--strict` now exits 1 on them.

- `fig_document_diagnose` and `fig_value_diagnose` answer `FIG_STATUS_OK` with warnings for a runtime format, where they returned `FIG_STATUS_UNSUPPORTED_OPERATION`.

- `fig set`/`insert`/`patch` of a mapping into dotenv, `.properties`, INI (below a section, or a new section by value) or a runtime language with `max_mapping_depth` now exit 1 with "the value cannot be written", where they wrote the mapping's own document text as the value (`n=x=1`).

- `fig patch` refusing a null or a sequence the target has no spelling for reports "the value cannot be written to <file>: …" instead of `error: NullUnsupported` / `error: UnsupportedValue`.

- `fig.Patch.apply`/`applyTo` return `error.UnsupportedValue` for a mapping the target cannot hold at its depth, before splicing anything.

- serializing a mapping as INI with `splice` set (`fig_value_serialize_opts`, and the bindings' serializers given the `splice` option) fails with `FIG_STATUS_UNSUPPORTED_FORMAT`, where it returned the mapping printed as an INI document (`c = 1`).

- `fig get`/`convert` to a runtime language that declares both a `lossless` without null and a `max_mapping_depth` apply both strips — nested mappings and sequences are dropped before its printer sees them — where only the null strip ran.

- a language registered with `version = 1` is refused ("vtable version 1 is not the 2 this fig speaks"); rebuild it against the new header.

- `fig_abi_version()` returns 3.

- a helper's `render` request line carries `"parent_key"` and `"parent_tag"` for every renderer; a helper that rejects unknown fields must accept them.

- `fig_node_kind` (and `Document.kind` in TypeScript) reports FIG_NODE_EXTENDED for a TOML datetime, a ZON enum or char literal, a JSON5 non-finite number or a plist date or data, where it reported STRING or INT.

- a language declaring a FIG_CAP_* bit or a lossless bit fig does not know, or native kinds without FIG_LOSSLESS_ENVELOPE, is refused at registration.

- a table with a `row_size` shorter than a row's required fields is refused as malformed.

- `fig edit …` and `fig e …` exit 2 with "`edit` is not
  a fig command" (or run a `fig-edit` found on PATH); they used to replace
  the value at the path. `fig replace` does exactly what `fig edit` did.

- `fig edit --key <file> <path> <name>` is now
  `fig rename <file> <path> <name>`; `--key` is not a flag of any action
  and exits 2.

- `--embed frontmatter-json`, `frontmatter-toml` and
  `frontmatter-fig` (and the same names after `--to-embed` and
  `--patch-embed`) exit 2 as an unknown archetype. They used to select
  the `;;;` JSON block, the `+++` TOML block and the fenced fig block.

- an unknown `--to-embed` archetype lists every
  archetype `--embed` takes, where it used to list four names.

- `fig version <arg>` (and `--version`/`-v` with an
  argument) exits 2 with "version takes no arguments; unexpected
  argument: `<arg>`". It used to print the version and exit 0.

- `fig help <arg>` (and `--help`/`-h` with an argument
  other than another `--help`/`-h`) exits 2. It used to print the general
  help and exit 0; `fig help set` never showed `set`'s help, and
  `fig set --help` still does.

- `fig patch` prints its "dropped N comment(s)" warning
  before the patched document or diff reaches stdout, rather than after.

- `fig insert <file> 'list[N]' <value>` with N neither
  0 nor the append index exits 1 (it exited 2), with a message naming
  [N]. The file is still left as it was.

- `fig get x.fig` (and every command given a `.fig` path) no longer infers the fig format from the extension; it falls back to sniffing the contents, as for any unknown extension.

- an unknown embed archetype prints only the archetype error, without the "Try using `--input <format>`" hint and format list; the exit status is still 2.

- `fig version` prints `fig 5.0.0 "Texas Everbearing"`.
  It printed two numbers, `fig 4.0.0 (core 3.1.0 "Texas Everbearing")`; a
  script that parsed the `(core …)` part will find it gone.

- `fig_version()`, `fig_version_string()` and fig.h's
  FIG_VERSION_* report 5.0.0, where they reported 3.1.0. FIG_ABI_VERSION
  is unchanged.

- every crate in the Rust workspace — fig, fig-macros,
  fig-sys and the fig-sys-`<target>` payload crates — is 5.0.0, where it was
  4.1.0; `@diaryx/fig` is 5.0.0, where it was 3.1.0; `@diaryx/fig-wasi` is
  5.0.0, where it was 4.0.0. Each is a major to a caller pinning `^`.

- `Embed.split` on a mid-document HTML island returns
  the host on both sides of the block as its body (it returned only the
  text after it), and on endmatter it includes any host text after the
  closing fence. A failure other than a missing or unterminated region —
  an invalid archetype, say — now throws instead of returning `null`.

- `Editor.open`, `Embed.open` and `Embed.openOrInit` on
  unparseable input throw a `FigError` whose message is the core's
  diagnostic (`fig_editor_create: <reason>`) where it was the bare
  `fig_editor_create: parse error`; the status is unchanged.

- a `Document`, `Editor` or `Embed` used after
  `dispose()` throws `FigError` with `status` `InvalidArgument`, not a
  plain `Error`.

- a derived `FromValue` impl's fixed messages, and
  anything built by `Error::msg_static`, arrive as `Error::Message`
  where they arrived as `Error::Static`, so a `match` with an
  `Error::Message(_)` arm ahead of a `_` arm now takes that arm.

- `Error::Language` for a declared string holding a NUL
  byte displays as `runtime language: <message> (byte offset N)`, where it
  displayed without the offset.

- cargo now refuses to build fig, fig-sys, fig-macros
  and the payload crates on rustc older than 1.88 with a rust-version
  error. Those toolchains already failed, on fig-macros' let chains.

- a FigError thrown by one of the renamed methods carries the new name in `op` and its message.

- `fig rename <file> <path> <name>` where <path> ends in
  an index exits 1 with "that path names a sequence item or the root,
  which has no key to rename" and leaves the file untouched, where it
  exited 0 and replaced the item's value with `<name>`.

- `fig rename` onto a name the mapping already holds
  exits 1 with "the mapping already has a key of that name" and leaves the
  file untouched, where YAML, JSON, INI, ZON, dotenv and .properties wrote
  a second key of that name and exited 0, and TOML exited 2 calling the
  name not a valid key.

- `fig_editor_replace_key`, `fig_editor_replace_named_key`
  and their `fig_embed_*` twins (Rust `rename_key`, TypeScript `renameKey`)
  return FIG_STATUS_INVALID_ARGUMENT for a path ending in an index or the
  empty path, where they returned FIG_STATUS_OK having overwritten the
  node; and for a name another entry of the mapping holds, where they
  returned FIG_STATUS_OK having written a duplicate key (or, for fig by
  name, FIG_STATUS_PARSE_ERROR).

- `fig_embed_replace_body` (Rust `Embed::replace_body`,
  TypeScript `Embed.replaceBody`) on an HTML `<script>` or `<code>` data
  island returns FIG_STATUS_UNSUPPORTED_OPERATION and leaves the render
  unchanged, where it returned FIG_STATUS_OK and the next render replaced
  the host text after the block with the new body.

- `fig version --help` (and `-h`, and after `--version`
  or `-v`) prints `version`'s usage and exits 0; before 5.0 it printed the
  version and exited 0.

- `fig help <action>` exits 2 with "for <action>'s help,
  run `fig <action> --help`" and no "See `fig --help`" line after it.

- fig_language_register (and registerLanguage, fig::language::register) accepts a language or dialect named after a format compiled out of the linked library, where it refused the name before; the row takes that format's FIG_FORMAT_* value, not one at or above FIG_FORMAT_RUNTIME_BASE.

- the rows of one registration no longer always take consecutive integers; a caller that computed a later dialect's format as the first plus its index must look it up with fig_format_by_name.

- fig::language::register returns a stand-in dialect as the compiled format's own variant (Format::Plist), not Format::Runtime.

- in a build with a format compiled out, a languages.figl language named after that format is reached by the format's name (`-i`/`-o`), extension and embed archetypes, where those were FormatDisabled before.

- a runtime language's print is handed `flow` (true when the bindings' editors spell a fig value), in the C struct, the helper wire's options and the Rust and TypeScript `PrintOptions`.

- fig_editor_{delete,insert,rename,move,reorder}_container and fig_editor_append_container_to_seq on a runtime language that declares no `section_noun` return `unsupported_format`, not `invalid_argument`.

- the published module compiles no format in; every format throws `UnsupportedFormat` until its language from `@diaryx/fig/languages/*` is passed to `registerLanguage` (docs/migrating-to-5.md).

- ZON and plist, left out of the 4.x module, are available as `@diaryx/fig/languages/zon` and `/plist`.

- a parse error is the registered language's message, with a byte offset, not the compiled parser's words.

- registering the same `Language` object twice returns its format instead of throwing.

- in a build with formats compiled out, a file whose format is sniffed from its contents can resolve to a registered or configured stand-in, where the sniff skipped those formats before.

- `@diaryx/fig/languages/toml` prints splice text for a table or array of tables inline, where it printed `[section]` lines, so an editor's `setValue` of a mapping into TOML lands as an inline table.

- `@diaryx/fig/languages/ini` refuses a mapping as splice text.

- a WASI build of the CLI no longer reads `languages.figl`, and `lang list` there has no `configured:` section.

- in a build without fig compiled in, a value argument is read by the language standing in for fig where there is one, instead of always as a string, and `get <file> <path>` prints a scalar through it.

- fig-wasi's formats are JavaScript languages: a parse error is the language's message, and `check --strict` fails a file for an error only, since the compiled parsers' lints are not there.

- fig-wasi reads ZON and plist.

- fig-wasi does not read `languages.figl`.


### Since cli 4.0.1

5.0.0 is the first release in which every artifact carries one version and one
`v*` tag, so it is the first CLI release since `cli/v4.0.1` and the first core,
crate and npm release since `core/v3.1.0`, `rust/v4.1.0` and `npm/v3.1.0`. (The
binary built from the `cli/v4.0.1` tag reported itself as 4.0.0: the tag was
cut without the CLI's version being bumped.)

The lists above start at `rust/v4.1.0`, the newest of the old tags. The CLI
fixes that landed between `cli/v4.0.1` and that tag went out in the crate and
the core but never in a CLI release, and are listed only under
`## rust 4.1.0` and `## core 3.1.0 · npm 3.1.0` below: a path key can be
quoted or escaped to hold a `.` or `[`, `delete` removes a whole section, an
edit on a file that does not parse prints the located report `fig check`
would, log lines read `error:`/`warning:`, and a YAML tag a conversion refuses
is pointed at with `--lax-tags` named. They reach `fig` users with 5.0.0.

The command line itself changes in the ways `docs/proposals/cli-5.md` argues:
a value argument is a fig value, written in the file's own syntax, with
`--raw` for the old verbatim splice; `fig get` on a scalar prints its text;
arguments are parsed strictly, so an unknown flag or a surplus positional is a
usage error; `fig comment --get` on a path with no comment exits 1; the exit
codes are written down — 1 for the document, 2 for the command line; and
`edit` is split into `replace` and `rename`. For a runtime language, every
renderer now takes one request that names the container its fragment goes
into, and a renderer written against 4.x has to be rebuilt against it.
[Migrating to 5.0](migrating-to-5.md) says what to write instead of each
break, for every artifact.

## rust 4.1.0

### Fixed

- **editor** — inserting a key the mapping already holds is refused, not written a second time ([`a9123a9`](https://github.com/diaryx-org/fig/commit/a9123a9baa27634d1d1c7d23450403f7e47ae592))
- **embed** — creating a leading block is refused when the host already opens with frontmatter of another archetype ([`acd78f8`](https://github.com/diaryx-org/fig/commit/acd78f8cc244ccf59aedc84b21ab273b266df78a))
- **cli** — log lines a user sees read `error:`/`warning:`, and a sniffed format is not announced ([`00158e7`](https://github.com/diaryx-org/fig/commit/00158e7c4c83fe832eb9f9a767fc6c4fb7e80c1c))
- **yaml** — a parse error carries a byte offset and a teaching message ([`52f65c7`](https://github.com/diaryx-org/fig/commit/52f65c7f98f5b5b6bda6c04e094e769713657233))
- **cli** — a YAML tag a conversion refuses is pointed at, with --lax-tags named ([`d4975c9`](https://github.com/diaryx-org/fig/commit/d4975c9be38ec35937d50bdb43e4cad78d7955f5))
- **cli** — an edit on a file that does not parse says where, not "run fig check" ([`7cebfb5`](https://github.com/diaryx-org/fig/commit/7cebfb52e2fe321519032c9dbd09ee79ad8f828d))

### Behavioural changes

- `fig insert <file> <path>.<key> <value>` where the key already exists exits 1 with "that key already exists" and leaves the file untouched. It used to exit 0 and write a second entry of that name (YAML, JSON/JSONC/JSON5, INI, ZON, dotenv, .properties, NestedText), or, for TOML and fig, report the value as invalid.

- `fig_editor_insert_key`/`fig_editor_insert_named_key`/`fig_embed_insert_key`/`fig_embed_insert_named_key` (Rust `insert_value`, TypeScript `insertValue`) return `FIG_STATUS_INVALID_ARGUMENT` (`DuplicateKey`) for a key the target mapping already holds, where they returned `FIG_STATUS_OK` and wrote a duplicate entry — or, for fig documents via a named key, `FIG_STATUS_PARSE_ERROR`.

- `fig set --embed <archetype>` on a host whose first line opens complete frontmatter of a different archetype — `---`, `---<lang>`, `;;;`, `+++`, or a fenced ```` ```<lang> ```` block — exits 1 and leaves the file untouched, where it used to prepend a second block above the first. `--embed endmatter` is unaffected.

- `fig_embed_open_or_init` (Rust `Embed::open_or_init`, TypeScript `Embed.openOrInit`) returns `FIG_STATUS_UNSUPPORTED_OPERATION` (`UnsupportedOperation`) instead of `FIG_STATUS_OK` when asked for a non-endmatter archetype the host lacks while the host opens with frontmatter of another archetype. `Embed.initRegion` returns `error.FrontmatterExists` in the same case.

- CLI argument errors and warnings on stderr read `error: <message>` / `warning: <message>` instead of `error(parseConfig): <message>` / `warning(languages): <message>`, with no blank line after them. Exit codes are unchanged.

- a release build of the CLI no longer prints `info(detect): inferred format `<fmt>` for `<file>` from its contents` on stderr when it sniffs an input's format (stdin `-`, or an unknown extension).

- `fig get`, `fmt` and `convert` on a YAML file, and
  `fig patch` on a YAML patch document, that does not parse exit 2 with
  a `file:line:col` report, where they exited 1 with
  `error: <ErrorName>` and a note pointing at `fig check`. Exit 2 is
  what every other format's parse failure already returned from these
  actions.

- `fig check` on a YAML file that does not parse
  prints the error's message, `--> file:line:col`, the source line and a
  caret, where it printed `error: file: <ErrorName>`. The exit status is
  unchanged.

- `fig get -o <format>` and `fig convert` from YAML to
  a format without tags, on a document with a custom tag, a tag its value
  does not fit (`!!int abc`), or an alias inside its own anchor, print a
  sentence naming the tag or alias, its `file:line:col` and source line,
  and (for a custom tag) a help line naming `--lax-tags`, where they
  printed `error: UnknownTag` / `TagTypeMismatch` / `AliasCycle` and a
  note pointing at `fig check`. The exit status is unchanged.

- `fig edit`/`set`/`insert`/`delete`/`comment`/`patch`
  on a file that does not parse, in a format with located reports (YAML,
  JSON, TOML, INI, dotenv, `.properties`, NestedText, fig), print the
  `file:line:col` report `fig check` would, where they printed
  `error: <ErrorName>` and a note pointing at `fig check`. The exit
  status is unchanged (1).

- an unhandled error on a single-file action no longer
  carries the `note: if <file> itself does not parse, `fig check <file>`
  says where.` line.

This release brings the Rust crate up to the core. Besides the changes listed
above, it carries everything under *core 3.1.0 · npm 3.1.0* below: splice
text for binding values, key renames that take a name, plist and NestedText
edits from the bindings, and `PrintOptions.splice` for runtime languages.
`fig-sys` adds a `splice` field to `FigSerializeOptions` and `FigPrintOptions`.

## core 3.1.0 · npm 3.1.0

### Added

- **cli** — a path key can be quoted or escaped to hold a . or [ ([`66727cc`](https://github.com/diaryx-org/fig/commit/66727cce0079a263458c6c60a70703b106912511))

### Fixed

- **editor** — set inserts only when the key is absent, not when its replace was refused ([`7f3394f`](https://github.com/diaryx-org/fig/commit/7f3394f7cd1fe0cfc9eb501fa32e16ab9dfcf78f))
- **cli** — delete removes a whole section through deleteContainer ([`96e4a35`](https://github.com/diaryx-org/fig/commit/96e4a3572a93f874d023dcefd2fae124c8246cb7))
- **editor** — an insert into a section with no own entries anchors on its own header, or is refused ([`3c24899`](https://github.com/diaryx-org/fig/commit/3c248990f79d8b2e0fe0cb91748d86199f3fcfcd))
- **editor** — a binding hands the editor a key's name and a value's splice text, not a standalone document ([`88240a7`](https://github.com/diaryx-org/fig/commit/88240a79165425038c0c4fb9063ebba4033e6920))
- **editor** — a new line pads to its anchor's column only past a sequence item marker ([`63c448b`](https://github.com/diaryx-org/fig/commit/63c448b36604502f1fbbda91c88b40e7698a4c07))
- **yaml** — a numeric tag checks its payload, and a %TAG directive can rebind the !! and ! handles ([`4b33d23`](https://github.com/diaryx-org/fig/commit/4b33d23816020f42af97bf2842096c51bb8183ff))
- **nestedtext** — a container or multi-line string is spliced as a nested block, and an empty container prints as {} or [] ([`edb80f6`](https://github.com/diaryx-org/fig/commit/edb80f640f6506d8f799109889d3e106632eb1c4))
- **editor** — an entry into a container that closes on its last entry's line is refused, not appended outside it ([`0304c11`](https://github.com/diaryx-org/fig/commit/0304c1187d2f6710725f22622cb841652cf03e43))
- **editor** — the one-line container refusal needs a close token, and covers sequences too ([`027f8c4`](https://github.com/diaryx-org/fig/commit/027f8c49e4f605953d687e476b79214f040c075a))

### Behavioural changes

- `set` on a path whose replace is refused — a table or section, or replacement text that does not reparse in place — returns that refusal and never inserts a second entry of the same name. Callers reading `splice_rejected` after a vetoed section replace now see it false; the CLI reports the section refusal instead of an invalid value.

- `fig delete <file> <path>` on a table, section, block container or array-of-tables element deletes all of it where it used to fail with `CannotDeleteTable` / `CannotDeleteSection` / `CannotDeleteContainer` (or `NotAnInlineArray` for `x[0]` on a TOML `[[x]]`).

- a command-line path in which a key segment begins with `"` or `'`, or contains a `\`, is read as quoted or escaped rather than as those characters literally; `a."b"` names the key `b`, where it named `"b"`, and an unclosed quote or trailing backslash is an invalid path.

- `set`/`insertKey` of a new key under a section that has no header line of its own (TOML `[a.b]` alone, then `a.y`) fails with `ImplicitSection` / `FIG_STATUS_INVALID_ARGUMENT` where it used to write the key into the first child section. Where the section's own header follows its child's, the key is written under that header.

- Rust `Editor::replace_key`/`Embed::replace_key` and TypeScript `replaceKey` take the new key as a name and spell it as the format spells a key: `.k` in ZON (was `"k"`, refused), `<key>k</key>` in plist (was refused), `"k\"q"` in JSON. A caller that passed key syntax — a quoted TOML key, a ZON `@"…"` — now has it quoted again.

- `fig edit --key` takes the new key as a name, as `fig insert` does since 447bcda: `has space` in TOML lands as `"has space"` (was refused), `x y` in ZON as `.@"x y"` (was refused), `b&c` in plist as `<key>b&amp;c</key>` (was refused), and `k"q` in JSON as `"k\"q"` (was `"k"q"`, refused). A key typed as syntax is spelled again, as it would be by `insert`.

- the Rust and TypeScript editors' `insert_value`/`replace_value`/`set` family now lands values in plist (every one was refused) and splices a NestedText scalar once: `name: h2` where it wrote `name: > h2`.

- `fig patch` into plist lands values (it was refused), and into NestedText writes a scalar once (`name: h2`, was `name: > h2`).

- `fig get <file> <path> -o plist` on a dict or array renders it (it panicked on an integer underflow).

- an entry added to an empty container that sits after its key on the key's line is indented one `indent_unit` under that line, where it was padded out to the container's column — `<key>a</key><dict/>` on a tab-indented line expands to `\t  <key>x</key>`, was a tab and fourteen spaces. An entry anchored on a key that follows anything but an item marker on its line takes that line's indent.

- converting YAML out of YAML (`fig get -o json` and the like, `fig_document_serialize`) refuses an `!!int`/`!!float` whose text is not that kind of number with a tag type mismatch, where it wrote the text bare. `!!float 0x1A` is refused too.

- in a document whose `%TAG` rebinds `!!`, a `!!` tag is custom — refused by a strict conversion, dropped by a lax one — where it was read as the core type. A document that binds `!` to `tag:yaml.org,2002:` has its `!int`/`!str`/… applied as core types, where they were custom.

- converting to NestedText writes an empty nested container as `{}` or `[]` on the line under its key, and an empty root as `{}`, where it wrote a bare `key:` (read back as an empty string) or nothing.

- a value set, inserted or patched into NestedText from a binding or `fig patch` lands as nested entries or items when it is a mapping or list, where it landed as a `>` block string.

- `fig set`/`insert` on NestedText with an argument that begins with a line break and whose rest is NestedText (`$'\nx: 1'`) writes that nested value rather than a `>` block string.

- a runtime language's `print` receives `splice: true` in its options when fig asks for splice text (a binding's editor value, `fig patch`), and the wire's print request carries `"splice"`.

- inserting a key (`fig set`/`fig insert`, the bindings' `insert_value`/`set`) into a plist dict, or a runtime closed-container language's mapping, whose close is on its last entry's line is refused with `ContainerClosesOnItsLine` (`FIG_STATUS_INVALID_ARGUMENT`), where the entry was written into the enclosing dict.

- inserting into a braceless closed-container root (a runtime OpenStep `.strings` file) works again, as it did before 0304c11.

- appending an item to a plist array, or a runtime closed-container language's sequence, that closes on its last item's line is refused with `ContainerClosesOnItsLine` (`FIG_STATUS_INVALID_ARGUMENT`), where the item was written into the container around it.

## core 3.0.1 · npm 3.0.1

### Fixed

- **cli** — align-cast the helper recovered from its transport for wasm32 ([`a11c6d3`](https://github.com/diaryx-org/fig/commit/a11c6d3ef4d200aae28d1b0ae246a88716dba392))

## core 3.0.0

### Breaking

- **languages** — remove generic XML as a selectable format ([`5a25681`](https://github.com/diaryx-org/fig/commit/5a25681a580ba5ccf8a614cd45e6d533928e8d6d))
- **c-api** — select an embed by (container, format) instead of a flat FigEmbedType ([`ecb689f`](https://github.com/diaryx-org/fig/commit/ecb689fafe2149022f5908481b8655bd3b5a459e))
- **root** — drop the deprecated `Native` alias for `Canonical` ([`20c7767`](https://github.com/diaryx-org/fig/commit/20c776794d2517507a70aaffdda4f0ebee60fd53))
- **rust** — drop the no-op `xml` feature ([`93d7a17`](https://github.com/diaryx-org/fig/commit/93d7a17d8c27779bcc56ab4b78f4c447a2fbd316))

### Added

- **rust** — Format gains ini, dotenv, properties, plist and nestedtext; abi-check holds every binding's format enum to the registry ([`aebc2f7`](https://github.com/diaryx-org/fig/commit/aebc2f71e4b3268c70b701beed5975d4984c8b09))
- **c-api** — reserve FIG_FORMAT_RUNTIME_BASE for languages registered at runtime ([`3d8b7b3`](https://github.com/diaryx-org/fig/commit/3d8b7b32eacd88ad1df6bd1e975b85156d67c89d))
- **runtime** — the contract as a vtable, the node table, the registry, and a Language over it ([`0bcbe05`](https://github.com/diaryx-org/fig/commit/0bcbe05c56f55dc1f11929341166f9de5f88825b))
- **c-api** — fig_language_register and fig_format_by_name; a runtime format is a peer at every entry point ([`15f2fb7`](https://github.com/diaryx-org/fig/commit/15f2fb71380b13cc8826f0d405ba72c1f8f39aa7))
- **runtime** — FigNodeTable.owner, the helper's handle on a parse's memory for free_table ([`b2ecb63`](https://github.com/diaryx-org/fig/commit/b2ecb63880f6e64222d836113bb60d8308182e26))
- **rust** — Format::Runtime, the Language trait and register, and the helper wire ([`d7e50e3`](https://github.com/diaryx-org/fig/commit/d7e50e3a5c33994e0d9ec7db3a5764585f20b65f))
- **cli** — languages.figl, the helper runner, fig lang list/check, and --lang ([`eaac04a`](https://github.com/diaryx-org/fig/commit/eaac04ab83ff1b1356ff054a35992a74c6716f08))
- **cli** — fig lang table, the node table a file parses to ([`e206a11`](https://github.com/diaryx-org/fig/commit/e206a11b32e884d188d8e5cc58a27310eca85edc))
- **rust** — RenderArgs is constructible, so a Language's render can be called by its own tests ([`c150d2d`](https://github.com/diaryx-org/fig/commit/c150d2ded19596c7a2bccb65c520451b6eeaa144))
- **editor** — the value renderer is told what fig's bare-literal rules make of the text, as `literal` ([`c80b7f8`](https://github.com/diaryx-org/fig/commit/c80b7f8445416f0352e163f3e2a8d9a35b89a7fe))
- **npm** — runtime languages — a format written in JavaScript ([`daff282`](https://github.com/diaryx-org/fig/commit/daff282c47c116b6f8ade2e46011d2fe5ba012c3))
- **npm** — @diaryx/fig/helper — the wire and serve without the wasm module ([`42ee808`](https://github.com/diaryx-org/fig/commit/42ee8082e71fd55e67e6d65f696591cf649e2cf1))
- **runtime** — a language declares its reference layer, and its tag directives ride the table ([`17ec1e6`](https://github.com/diaryx-org/fig/commit/17ec1e6a45229d926a6193315445c51987728a5b))
- **cli** — `fig lang table --spec` reads a file as a version of its format ([`d9796ea`](https://github.com/diaryx-org/fig/commit/d9796ea931e90532f723a4182556d5308009ca5e))

### Fixed

- **abi** — name plist's two extended kinds in fig.h and both bindings, and hold FigExtKind in abi-check ([`d46db7c`](https://github.com/diaryx-org/fig/commit/d46db7c35b3252f24bae804ba86adfe7c37152c0))
- **runtime** — FIG_DEPTH_NONE, so a vtable can declare a flat format ([`a9e5879`](https://github.com/diaryx-org/fig/commit/a9e5879ccf4749a20af11e676815e3ae6a7d66ab))
- **cli** — name a runtime format in the bad-edit-text report rather than panic on @tagName ([`761db09`](https://github.com/diaryx-org/fig/commit/761db09b9830268709cf0a9f619480e6332724f9))
- **canonical** — print an empty container's trailing comment after the comma, and a value's leading comment at all ([`051cd8b`](https://github.com/diaryx-org/fig/commit/051cd8bc8166449fb399a9aed1420dd2e7199a91))
- **editor** — a runtime language's flow root is a flow container, not a section root ([`504edb7`](https://github.com/diaryx-org/fig/commit/504edb71dc166881d3867e3f6fd360c661efa922))
- **ini** — a `[ ]` header — a name of only whitespace — is an empty name, refused ([`b3976e6`](https://github.com/diaryx-org/fig/commit/b3976e6be4f66a27ab0bdfa2a45c5c81556bce05))
- **runtime** — a core-schema tag on the wire decodes to the kind tag it encoded ([`4b4fd01`](https://github.com/diaryx-org/fig/commit/4b4fd01224a55df9ee7683260dccbddd08dceb6b))
- **properties** — an empty value spans where a value would begin, and `set` on a bare key writes the separator ([`f543684`](https://github.com/diaryx-org/fig/commit/f5436849c447e51cba58d3bc7f40a206a5023a09))
- **editor** — an inserted key is spelled as the format spells a key, at every entry ([`447bcda`](https://github.com/diaryx-org/fig/commit/447bcda394826139db318a174d7d9e859223d626))
- **cli** — `--lang` and `lang check` reach every dialect of a configured language ([`b5cba7a`](https://github.com/diaryx-org/fig/commit/b5cba7a57fbbedcae78081cfb030fc3cdf9fe3d5))
- **runtime** — a renderer's refusal is reported in the helper's own words ([`872f06e`](https://github.com/diaryx-org/fig/commit/872f06e5aa7d3e0345d46fc2e07e2962973d4a9e))

### Changed

- **editor** — decide flow from syntax and the section rule, retiring INI's insertKey hook ([`70dcfdd`](https://github.com/diaryx-org/fig/commit/70dcfdd60f2b7d8eb5d5f96fc71c586b8a0f1cf3))
- **editor** — record item markers and copy prefix bytes, retiring fig's and NestedText's sequence hooks ([`239502c`](https://github.com/diaryx-org/fig/commit/239502cef3b152fbb081e1890921cbae3ca9cfbb))
- **editor** — comment delimiters are an open/close pair, retiring plist's six comment hooks ([`8719d15`](https://github.com/diaryx-org/fig/commit/8719d15bd5c2302f8a3cf2721126ef1d6d303f1a))
- **editor** — value, entry and item renderers, retiring plist's and NestedText's insert hooks ([`bbc6338`](https://github.com/diaryx-org/fig/commit/bbc6338082595c26d68b33b905ed66ef8c040e20))
- **editor** — reframe values from a recorded separator, retiring the fig, YAML and NestedText reframe hooks ([`9b6c34b`](https://github.com/diaryx-org/fig/commit/9b6c34b3c47c262fa0f02af89bfedb981187384a))
- **editor** — name mentions and header syntax retire TOML's five hooks; merge_key and core aliases retire YAML's two — no editing hooks remain ([`f5d96a7`](https://github.com/diaryx-org/fig/commit/f5d96a730c344533c029a58aecd033e35b46e2f6))
- **editor** — renderers take the dialect and the engine asks hasRenderer, so presence can be a runtime answer ([`8cb44fa`](https://github.com/diaryx-org/fig/commit/8cb44faca3b1c390762a4c97abdfa53cceca8bc4))
- **cli** — lift the helper wire's codec into fig.Wire over a Transport ([`5c1258f`](https://github.com/diaryx-org/fig/commit/5c1258f26db07452e6d41899a16bac3839ecefd5))

### Behavioural changes

- `FIG_FORMAT_XML` (6) is gone from fig.h, and a C caller
  passing 6 now gets `FIG_STATUS_UNSUPPORTED_FORMAT` from every entry point
  and 0 from `fig_format_capabilities`, where a `-Dxml=true` build used to
  parse and serialize it.

- `fig get -i xml`, `-o xml` and `fig check -i xml` are no
  longer accepted spellings; a build that had compiled XML in used to accept
  them, and every shipped binary already rejected them.

- `AST.SerializeError` no longer carries
  `RootNotSingleElement`, `NestedSequenceUnsupported`, `InvalidElementName`
  or `NonScalarValue`, and `AST.SerializeFormat`, `cli.Format` and
  `Language.Detected` no longer have an `xml` member; a Zig switch that named
  any of them stops compiling.

- every `fig_embed_*` entry point that took an `int
  embed_type` now takes `int container, int format` (and `fig_embed_detect`
  two out params); a C caller compiled against ABI 1 must be recompiled
  against the new header, and `fig_abi_version()` now reports 2.

- a Rust or TypeScript caller that links a `fig-sys`
  prebuilt payload or an npm wasm from core 2.x against this binding source
  will crash; the payload crates and the wasm module ship rebuilt with the
  same release.

- `fig.Native` no longer exists; a Zig consumer that
  spelled it must write `fig.Canonical`, which has been the name since 2.0.

- a plist leading or trailing comment set to the empty
string is now written `<!-- -->` with one space, where the hook wrote two.

- a plist entry or item appended after a value that
carries a same-line `<!-- -->` comment now lands after the comment's line,
as in every other format, where it used to land between the value and its
comment; an empty plist container expands with the declared two-space unit
rather than a unit sniffed from the file.

- inserting a key into NestedText's empty inline `{}`
now writes `{key: value}` instead of failing with `EmptyInlineContainer`,
and appending to an empty inline `[]` writes `[value]`.

- INI and fig gain `renameContainer` (and
`fig_editor_rename_container` answers `ok` rather than
`unsupported_format` for them); an INI rename reaches every reopening of
the section, and a fig rename every re-entry of the container, where the
generic key splice used to rename only the first mention and split it.

- inserting a root key into a header-first INI file now
lands above the first `[section]`, where it used to land after the last
one and silently join it.

- a Zig consumer with an out-of-tree `Language` that
declared an editing hook (`insertKey`, `replaceValAtPath`, …) is refused
by `Language.validate` as an unknown declaration.

- (Rust) `Document::to_value` on a plist `<date>` or `<data>` now yields `Value::Extended { kind: PlistDate | PlistData, .. }` rather than `Value::Str`. (TypeScript) `asExtended` now returns `ExtKind.PlistDate`/`ExtKind.PlistData` where it returned the unnamed integers 7 and 8.

- (Rust) `Format` has a tuple variant, so `Format::X as
  isize` no longer compiles; the integer it yielded was never the ABI value.

- (Rust) `Capabilities` gains a `Default` impl and
  `Capabilities::new`, and `Error` gains a `Language(String)` variant a
  `match` without a `_` arm did not have to name.

- a compiled `Language` declaring `renderValue` takes a fifth argument, `literal: Literal`, and the engine no longer expects it to classify the text itself. plist is the only one in tree and is updated.

- `FigLanguageVTable.render_value` takes `const char *literal` after `value`; a host that registered a value renderer against the previous header must add the parameter. `FIG_LANGUAGE_VTABLE_VERSION` stays 1 because no release carried the previous shape.

- the helper wire's `render` request for `which:"value"` carries `"literal"`; a helper that ignores it is unaffected, and one reading `RenderArgs` from the Rust crate sees the new field, a string when absent.

- (Rust) `fig` and `fig-sys` no longer declare an `xml`
  feature; a dependent naming it in `features = [...]` fails to resolve and
  should delete the entry, which changed nothing since 3.5.0.

- canonical output for an empty `{}` or `[]` that carries a trailing comment and has a later sibling is `{}, // c` rather than `{} // c,`; the old spelling reparsed with the comma inside the comment text.

- a comment between an entry's `:` and its value survives a canonical print as `"k": /* c */ v` (or `// c` with the value on the next line); it used to be dropped.

- a runtime language whose `syntax` declares
`flow_containers` and no `section_noun` now edits its root container by
comma-aware splice, as the compiled flow formats do. Deleting an item
from, or appending to, a one-line root `[...]` or `{...}` used to fail
with an edit-text refusal (the line splice took the whole document); a
multi-line root was edited by line and could strand a separator comma.
Compiled formats are unchanged.

- an INI document with a section header whose name is
only spaces or tabs (`[ ]`, `[\t]`) is now refused with `InvalidKey`
("a key/section name cannot be empty"), at the `]`. It used to parse to
a section with a garbage name, and printing it could crash the process.

- a runtime language whose table carries a `tag` of
`!!null`, `!!bool`, `!!str`, `!!int`, `!!float`, `!!seq` or `!!map` now
gives a node with a kind tag (`AST.Tag.kind`), which the fig printer
re-emits as `: type =` and `fig_node_tag` reports as such; it used to
give a text tag of that spelling, which fig dropped and YAML re-emitted
verbatim. Any other tag text is unchanged.

- in `.properties`, the value of a key with nothing
after its separator now spans `[end of separator, same)` instead of
`[end of key, same)`, and its keyvalue span ends there too; `fig set` on
such a key, or on a key with no separator, now produces `key=value`
rather than gluing the value to the key.

- `fig insert` spells the new key through the format's
key style — `.k` in ZON (was `k`, refused), `"has space"` in TOML (was
`has space`, refused), `"k\"q"` in JSON (was `"k"q"`, refused). Rust

- `--lang <name>` and `fig lang check <name>` accept
the name of a further dialect of a configured language, spawning helpers
as needed to find it; a name nobody serves is still refused, after every
unspawned helper has been asked.

- an edit that a runtime language's renderer declines
now fails with `RendererRefused` (`FIG_STATUS_INVALID_ARGUMENT` at the
C ABI, as before) and the CLI reports the helper's message rather than
the `--seq` text.

- `fig_format_capabilities` reports `FIG_CAP_REFERENCES` (bit 3) for YAML; a host comparing the mask against exactly `READ|EDIT|SERIALIZE` sees a new value.

- `FigNodeTable` gains `directives`/`directive_count` before `owner`; a C host built against the previous header must recompile (core 3.0 is unreleased, so no released ABI changes).

- a runtime language that declares `references` and returns aliases, merges or tags now has that layer collapsed when converted to a format without one, as YAML is; one that does not declare it is unchanged.

- `fig.Language.YAML.materialize`/`TagMode` are gone from the Zig API; the pass is `fig.Materialize.materialize` with `fig.Materialize.TagMode`.

### Why a major

Core 3.0 is the two breaks that had been listed in BREAKING-CHANGES since
2.4, plus the housekeeping that waited for them. The C ABI moves to version
2: every `fig_embed_*` selector takes a `(FigEmbedContainer, FigFormat)`
pair in place of the flat `FigEmbedType`, and `FIG_FORMAT_XML` (6) is
retired with the generic XML format it named. A C caller recompiles against
the new header; the enum's comment in fig.h maps every old value to its
pair. The Rust and TypeScript bindings keep their public shape — a flat
`EmbedType` in both, now decomposed to the pair inside — and ship rebuilt
native payloads and wasm, which is why they move with the core. The Rust
`Format` enum gains the five formats it was missing, and `zig build
abi-check` now holds every binding's format enum to the registry.

Two things from the [runtime languages](proposals/runtime-languages.md)
proposal ride along. fig.h gains `FIG_FORMAT_RUNTIME_BASE` (4096), the first
integer no compiled-in format may take, reserved now so that a later minor
can hand out an integer to a language registered at runtime without arguing
about whether a future format might have wanted it. And the editing hooks
are gone: the twenty-five per-format overrides of the editor's methods, each
of which ended in one splice and was Zig only for want of a fact the parser
had dropped, an engine constant that was really syntax, or a string
function. Each became one of those — a `Document` table the parser fills
(item markers, entry separators, a section's name mentions), a `Syntax`
field (`indent_unit`, `seq_item_marker`, a comment delimiter pair,
`section_header`, `merge_key`), or one of five pure renderers — and the
compiled formats now edit through exactly the contract a runtime format
will. A Zig consumer with an out-of-tree `Language` that declared a hook
sees it refused by `validate` as an unknown declaration. Along the way INI
and fig gain `renameContainer`, a NestedText rename that dropped an
indented multiline key's indent is fixed, and YAML's materialize no longer
indexes an absent tag table. The carrier itself — registering a language
at runtime — is not in 3.0.

BREAKING-CHANGES.md is retired with this release: everything it listed has
shipped, and a planned break is recorded from now on as a
`Behavioural-change:` trailer, in this file's unreleased section.

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
