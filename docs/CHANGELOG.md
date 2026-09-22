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
