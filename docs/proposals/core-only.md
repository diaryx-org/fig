```fig
title = Core only
description = Every format moves out of core, figl included — core names no language in its source or its C ABI, every language reaches core only through the contract, and every consumer registers the languages it uses
created = 2026-10-09
updated = 2026-10-12
status = draft
part_of = [proposals](proposals.md)
```

# Core only

> **Status: DRAFT.** Written against `main` at f459cce (5.2.0, core ABI
> 3). Nothing here is built. §7 lists what breaks; §8 orders the work.
> All of it lands in 6.0, ABI 4, staged in §8's order.
>
> Progress (2026-10-12): §8 step 2 is mostly built — the merge key, the
> flow opener, key spelling (`renderKeyName`), line prefix marks, section
> refusals, comment delimiters, the bare-literal rule, conversion warnings
> (`prints`), the fragment printer, the consumer's sniff order, and YAML's
> stream splitter moved into YAML. §4.6 needed no named options: `flow`
> means "a container root inline" for any format, and `fig_indent` became
> `indent_requested`; of `SerializeError` only `FigUnrepresentableRoot`
> named a format. `splitStream` is a declaration, a `split_stream` vtable
> slot and wire op, and `fig_stream_split` / `Format::split_stream` /
> `splitStream` in the bindings; step 2 is done. Step 3 (the kit, `src/kit/`)
> is done. Step 6's first stage — the contract's table as ABI 4 spells it —
> is done (05b5faf). Step 7 is done in substance: the tree a language
> builds, `Document`, the table and its conversions, the contract's
> declarations and C shapes are the kit's; a language's own source imports
> nothing of core (its editor tests may); `kit/contract.zig` builds a Zig
> language's vtable from its module; diagnostics cross the contract; and
> every compiled language, registered through its vtable alone
> (`-Dcontract-copies`), scores as the compiled format on every parse suite
> and all 27,503 edit cases. Step 8 is under way: the compiled registry is
> gone (`language.zig` is the Zig contract and `validate` alone); every Zig
> language registers through its contract vtable, from `bundle.zig`, the CLI
> at startup and the C library on first use; a format is a registered
> dialect's handle, from 1, with no `FigFormat`, `FIG_FORMAT_*`, stand-ins,
> `SerializeFormat` or `Detected`; a language's `--spec` versions are dialect
> rows (`yaml-1.1`); the authoring dialect is `figl`, with no alias.
> Embedding, its preset containers and §4.5 with it, is no longer fig's: it
> moved to twig. Left in step 8: the §6 check. Step 6's last stage is
> built: the `AST` is the table — rows, a string buffer and sparse
> facts — read through `node(id)` and the accessors, and a parse's table is
> checked and adopted with no copy; the compiled YAML resolves its own tags
> and materialize reads the resolution. The Zig parsers still build a
> `ParseTree` and emit it as rows ([task](/docs/tasks/closed/zig-parsers-write-rows.md)).
> The lexeme's value (§4.2) is not built. Then step 9.
>
> §8 step 1's measurements are done (2026-10-12). They settled the third
> revision: the contract's node table becomes the `Document`'s own
> storage, adopted without a copy (§4.9), and a language can ship as a
> crate of its own (§5.2).
>
> The second draft. The first let a Zig language import core's internals
> and shipped the languages as a second prebuilt payload. A language built
> into its own archive compiles its own copy of every core module it
> imports, so the two could not both hold. This draft keeps the separate
> payload and gives up the imports (§5.1). It also records how far the JS
> twins match the compiled formats on the conformance corpora and on an
> edit corpus (§5.6), since that is what the plan stands on.

## 1. The claim

The [runtime languages](runtime-languages.md) contract has been crossed
in both directions. Twelve JS twins drive the npm build, which compiles
no format in, and the compiled formats edit through the same contract as
a registered one. A format no longer needs to be inside core to be
complete. What still puts the formats there is packaging: `list.zig`'s
rows, `language.zig`'s `slots`, and the `default_on` flags that decide
what every Rust binary, static payload and C consumer links.

This proposal moves all of them out, **figl included**, and makes that a
rule rather than a default:

> **Core names no language.** No file under `src/` that core builds
> imports a language, switches on one, or spells one's name, and neither
> does `fig.h`. A language is something a consumer registers, and it
> reaches core only through the contract.

The authoring dialect is named `figl` from 6.0, after its extension, so
that the library and the language stop sharing a word. §6 depends on the
difference.

"Core" means what the library builds: the value model, `Document`, the
editor engine, regions, patch, diagnostics, the contract (`language.zig`,
`manifest.zig`, `runtime.zig`, `wire.zig`, `harness.zig`) and `c_api.zig`.
The CLI, the LSP and the formats themselves become consumers of it.

## 2. Where 5.2 stands

The npm package already works this way: the wasm is language-free, and
`registerLanguage(json)` is how a caller gets JSON. Rust has
`fig::Language` and `fig::register`, and a registered row named after a
compiled-out format stands in for it. Every other consumer links the
compiled formats whether it uses them or not: nine are `default_on` in
`list.zig` (plist is opt-in), and Rust's `default` features take eight
of those, leaving out ZON.

A survey of core outside tests found about forty-five couplings. They
fall into nine groups, and §4 takes each in turn:

| Group | Where | Example |
|---|---|---|
| Identity | `list.zig`, `language.zig`, `c_api.zig`, `fig.h` | `FIG_FORMAT_YAML = 3`; `slots`; `standInAbi` |
| Value model | `ast.zig`, `manifest.zig`, `fig.h` | `FIG_EXT_PLIST_DATE`; `NativeKinds` |
| References | `materialize.zig`, `ast/reader.zig`, `c_api.zig` | imports `languages/yaml/parser.zig`; `"<<"`; `tag:yaml.org,2002:`; the pin that only YAML may declare `caps.references` |
| Editor | `editor.zig`, `editor/splice.zig`, `manifest.zig` | `KeyStyle.zon_field`; `literalOf` through figl's tokenizer; hardcoded `.{`; closed `CommentStyle` |
| Embed | `embed.zig`, `fig.h` | the comptime pin requiring exactly `json`, `yaml`, `toml` and `fig`; bare `---` means YAML; `+++`/`;;;`/endmatter presets; YAML stream splitting |
| Serialization | `serialize_options.zig`, `patch.zig` | `fig_indent`; a `.fig` arm for fragments; `FigUnrepresentableRoot` |
| Diagnostics | `diagnostics.zig` | `valueLoss`, `degradedNote` and `commentsEmitted` switch over every format |
| Shared helpers | `util/number.zig`, `util/datetime.zig`, `util/json_string.zig` | `Spelling.yaml_1_2`; TOML/YAML leniency flags |
| Wire and ABI text | `wire.zig`, `fig.h` | splice style `"json_string"`; prose naming formats |

Most of these are pins and switches written before the contract could
say the thing itself, so their fix is a declaration a language makes.
Three groups are more than that: identity, the value model and embed.

The coupling also runs the other way, from the languages into core and
into each other. YAML imports `document`, `materialize` and `token`;
TOML, figl, JSON and plist import `editor.zig`, and TOML and plist
`editor/splice.zig`; YAML, TOML and plist import JSON's parser. §5.1
removes all of it.

## 3. What a consumer sees

```rust
let yaml = fig_yaml::register()?;          // -> Format, a handle
let doc = fig::parse(input, yaml)?;
```

```ts
import yaml from "@diaryx/fig/languages/yaml";   // unchanged
const Yaml = registerLanguage(yaml);
```

```c
#include <fig.h>
#include <fig/yaml.h>                       // ships with the language, not core
int yaml = fig_lang_yaml_register(&err);
```

A format is a name plus a handle that lives for the process. A name is
the only thing worth persisting, and `fig_format_by_name` already
resolves one. Nothing is registered implicitly. The CLI and LSP register
every language they ship at startup, so their behaviour is unchanged.

A language with several dialects registers once and gets a handle per
dialect: JSON's registration yields `json`, `jsonc` and `json5`, and
YAML's yields `yaml` and `yaml-1.1`. The register call returns the
language's primary dialect; the rest are found by name. That is how a
registered JS language with dialect rows already behaves.

## 4. Each coupling, and what it becomes

### 4.1 Identity

`FigFormat`, the `FIG_FORMAT_*` enumerators and every `abi_value` are
deleted, along with the derived enums (`Detected`, `SerializeFormat`,
`deserialize.Format`, `Embed.InnerFormat`). The handle range starts at
1, not at `FIG_FORMAT_RUNTIME_BASE`. The stand-in mechanism goes too,
since there is no compiled format for anything to stand in for.

Rust's `Format` becomes an opaque `Copy` handle with `name()` and
`by_name()`, and loses its `Yaml`/`Json` variants and its `Runtime` arm.
TypeScript's `Format` keeps its members only if the languages module
exports them, as `yaml.format` after registration. Core's `Format` export
does not keep them.

Detection by extension already reads each language's `extensions`, and
stays. Two registrations claiming one extension are refused, as a second
registration of a name already is.

Detection by content does not stay as it is. Today only compiled formats
are sniffed, in the frozen `sniff_rank` order, and a runtime format never
is ([runtime languages](runtime-languages.md) §8.1), because a helper that
claimed too much would take files from the built-ins. Once every format
is registered, that rule would end sniffing. What replaces it is that the
order belongs to the consumer, not the language: a language may declare a
`sniff` predicate, and a consumer opts in by passing the order it wants
(`fig_detect_order`). A language never ranks itself against another. The
CLI passes 5.x's order, so `fig get -` on stdin behaves as it does now. A
consumer that passes no order gets no sniffing. The pin that `jsonc` is
never sniffed becomes the CLI's order leaving it out.

### 4.2 The value model

`ExtKind` stays closed, because a closed kind set is what makes
conversion between formats meaningful. Its members are renamed by what
they *are* rather than where they came from:

| Now | Becomes |
|---|---|
| `offset_datetime`, `local_datetime`, `local_date`, `local_time` | unchanged; they never named TOML |
| `enum_literal` | `symbol` |
| `char_literal` | `char` |
| `number_special` | unchanged |
| `plist_date` | `offset_datetime`, with the lexeme kept verbatim as now |
| `plist_data` | `bytes` (base64 payload, whitespace stripped) |

Only the plist rows are required by §1's rule. Decided 2026-10-12: the
plist rows change in 6.0 (dates merge into `offset_datetime`, data becomes
`bytes`), and the ZON rows stay as they are; `enum_literal` and
`char_literal` name what a value is, if in Zig's words.

Merging plist dates is the one real semantic change: a plist date then
converts to a TOML datetime instead of degrading to a string. It costs
work in the other direction. Plist spells only `YYYY-MM-DDTHH:MM:SSZ`, so
its printer has to normalise any offset or fractional second and report
the loss as a diagnostic. `bytes` has a second user waiting: YAML's
`!!binary`.

A row carries a scalar's lexeme, not its value, and today the value is
decoded by whoever reads the row. The bindings decode with one family of
spellings (`0x`/`0o`/`0b`, decimal), so a YAML 1.1 `0777` reads as 777
rather than 511 and `190:20:30` as NaN. §5.6 found this; the Zig
scoreboard never saw it because it decodes the raw text itself. A row
whose lexeme is not in the canonical spelling carries its value as well,
which the language computes. This belongs with `resolveScalar` in §4.3.

### 4.3 References

`caps.references` stays: the anchor/alias/tag layer of `Document` is a
generic concept that YAML happens to be the only user of. Its YAML parts
move into YAML through declarations:

- `syntax.merge_key` (already present) replaces the hardcoded `"<<"` in
  `ast/reader.zig` and `materialize.zig`.
- Tag resolution moves into the language, as data rather than a
  callback. Core today decodes tag text itself: `coreTagName` reads
  `!!int`, `tag:yaml.org,2002:int` and `%TAG` handles, `asNumber` judges
  a `!!int` payload by YAML's `classifyNumber`, and `asBool` knows YAML's
  bool spellings. Instead, a row's tag carries its text verbatim, for a
  language that re-emits it, and what the language resolved it to: a
  core kind, custom, or non-specific (YAML's `!`), and whether the
  scalar's text agrees with the kind. Materialize reads the resolution,
  never the text. A callback was the alternative, but it would cross the
  wire per tagged scalar on every conversion, and a JS language would
  answer it from another process. This is a row field, so it lands with
  §4.9's layout (§8 step 6), not with the other declarations. The
  contract's spelling of a kind tag (`!!int`, from `Tag.spelling`) goes
  with it.
- The `c_api.zig` pin that only YAML may declare references is deleted.

### 4.4 The editor

| Coupling | Becomes |
|---|---|
| `KeyStyle` {verbatim, json_quoted, zon_field, bare_or_quoted} | a `render_key` renderer; every language spells its own keys |
| hardcoded `.{` in `flowOpenEnd` and `isFlow` | `syntax.flow_map_open` |
| `- * > +` in `ownsItsLines` | the declared `seq_item_marker` and related syntax |
| closed `CommentStyle` scanners | scanning driven by the delimiters `syntax.comments` declares |
| `SectionNoun` choosing `NotATable`/`NotASection`/`NotAContainer` | one neutral error carrying a noun the language declares |
| `literalOf` classifying a caller's literal with figl's `sniffBare` | the caller supplies a typed `Literal` (the type `RenderArgs.literal` already carries); the CLI, which reads value arguments as figl (CLI 5 §1), parses them with the figl language |

As built (2026-10-12), the last row went another way. The bare-literal
rule moved out of figl's tokenizer into `util/literal.zig`, which core and
figl share, so the editor still types a caller's value text itself and no
entry point changed. The rule is the contract's, as `Literal` already
said, and it moves to the kit with the other shared helpers (§4.8). What
follows is the design it replaced.

The last row is how figl leaves core without taking the CLI's value
grammar with it: that grammar belongs to the CLI, which now depends on
the figl language like any other consumer. `literalOf` reaches only a
language that declares a value renderer (plist today), but its removal
changes the set-by-text entry points for those languages, and §7 lists
it.

### 4.5 Embed

The strict reading of the rule takes Markdown and HTML out as well, and
this proposal takes the strict reading. Core keeps the region machinery:
locating, the span model of `FigRegion`, editing through a region, and
re-housing. Containers are registered, as languages are, through a
vtable of the same kind and the same wire. A container declares its open
and close delimiters, how an info string or attribute names the inner
language, an optional codec (HTML entities), and an optional default for
an untagged block.

- Bare `---` meaning YAML becomes the frontmatter container's declared
  default, set by whoever registers that container.
- `+++` (TOML), `;;;` (JSON) and endmatter (YAML) become containers that
  pin an inner language by *name*. `FIG_EMBED_SEMICOLONS_JSON`,
  `FIG_EMBED_PLUS_TOML` and `FIG_EMBED_ENDMATTER_YAML` are deleted.
- `fig_embed_detect` iterates the registered containers.
- YAML's multi-document splitting and `%YAML` directive handling
  (`extractStream`, `segmentIsDirectives`) move into YAML as a
  `splitStream` declaration. The wire gains the operation. §5.6 shows the
  JS twin cannot read a stream without it.
- The comptime pin that the embeddable set is exactly four is deleted.

The standard containers ship as a package of their own, `fig-embed`,
built and versioned in this repository with the languages. For a
frontmatter user that costs one registration call, not a new dependency
to track.

### 4.6 Serialization

`FigPrintOptions` keeps what every printer reads: width, indent, pretty,
comments. One-language options (`fig_indent`, `flow`) leave the struct
for a list of named options that the language declares and `register`
validates, so a misspelled option is refused rather than ignored.
Fragment printing becomes a declaration in place of the `.fig` arm in
`serializeFragmentWith` and the `.fig` check in `patch.zig`.
`SerializeError` reduces to a neutral set: unrepresentable value,
unrepresentable key, unrepresentable root, unresolved reference.

### 4.7 Diagnostics

`valueLoss`, `degradedNote` and `commentsEmitted` collapse into the
declaration-driven path that `runtimeValueLoss` already takes for a
registered format. Two new declarations cover what the switches knew and
the contract cannot yet say: `caps.root_container` (the root must be a
mapping or a sequence, which is figl's rule) and a per-kind `degrades_to`
(TOML's `integer`, plist's `string`, JSON's `quoted string`).

### 4.8 Shared helpers and the wire

`util/number.zig`'s spelling presets, the leniency flags in
`util/datetime.zig` and `util/json_string.zig` move to a Zig kit module
that the languages import. It is the Zig counterpart of the npm package's
`kit/`. The pieces of JSON that YAML, TOML and plist borrow (the string
scanner and escaper) move there too. The kit is small and holds no state,
so a copy of it in more than one archive costs bytes and nothing else.
The wire's splice style `"json_string"` becomes `"escaped"`, and its
escape grammar is defined in the spec, not by reference to a format.
`lossless.zig`'s `nativeFor` reads the registry entry, not the module.

`canonical` stays, because the harness checks every registration against
it. It is never registered, has no handle, and is absent from `fig.h`.
It is the harness's oracle, not a format.

### 4.9 The Document is the table

Through the contract, every format would pay a copy: the language's
node table is rebuilt into `Document`, an AST of tagged-union nodes with
seven side arrays beside it. Measured on a prototype (§8 step 1), the
copy costs 21–49% of the compiled parse, an edit round costs 1.5–2× (an
edit reparses), and peak memory rises 2.5–3.6× (the 6 MB YAML file:
134 MB compiled, 361 MB through the table).

So the table and the `Document` become one structure. Each node is a
row of fixed columns, stored column by column:

| Column | Type | |
|---|---|---|
| `kind` | u8 | null, boolean, string, number, extended, sequence, mapping, keyvalue, alias |
| `flags` | u8 | which sparse tables hold a fact for this node |
| `start`, `end` | u32 | the node's span |
| `parent`, `first_child`, `next_sibling` | u32 | the tree; a keyvalue's key and value are its two children |
| `text` | u32 offset, u32 length | a scalar's text, in the source or, with the length's high bit set, in the document's string buffer |

Everything rarer is a sparse table sorted by node: anchor names and
spans, tags and their spans, markers, separators, item separators,
regions, mentions, directives and comments. A node with none costs a
flag bit. The prototype carried every corpus through
`Document` → rows → `Document` with every field equal, at 30–38 bytes a
node against today's 64–208.

A language writes rows straight into memory core hands it, and core
validates and adopts them: the tree's links, kinds, spans in bounds,
sparse tables in order. Validation measured 4–9% of the compiled parse,
against the copy's 21–49%. The projected peak for the 6 MB YAML file is
about 43 MB, a third of today's compiled path. Core allocates everything
a `Document` owns, so a language in an archive of its own never frees, or
is freed by, another archive's allocator. A JS or out-of-process
language still pays the wire's decode, which writes the same rows.

The row's layout is ABI: it is `fig.h`'s `FigNodeRow` and its sparse
tables, versioned with ABI 4 and frozen for the major version. A builder
call per node would keep it private, at a cost per node that crosses an
archive boundary uninlined; a frozen layout is the cheaper price, since
the ABI only breaks at a major version anyway. A later version may add
columns and sparse tables, never change one.

Decided 2026-10-12: a scalar's text points into the source where the
source spells it as is, and into the string buffer only when the language
decoded it (an escape, a folded block). A row's tag carries both its text
as written, for a language that re-emits it, and the language's
resolution of it (§4.3). `Document` keeps its methods, now a view over the
rows, so the Zig API reads as it did.

Core's own code reads the rows through `Document`'s accessors. A
`view(id)` returns today's tagged union for a node, so a `switch` over a
node's kind stays exhaustive and most of core reads as it does now. The
port touches about 1,080 lines in 23 files, a fifth of them on the
editor; the languages' 1,700 are ported by §5.1 regardless. A walk that
reads every span and every scalar costs the same on both layouts.

Offsets are u32, so a source is limited to 4 GiB.

#### The table, as ABI 4 spells it

Fig owns the table; a language fills it. `parse` is handed a `FigTable *`
that fig allocated, and writes into it through calls fig exports, so no
memory crosses an allocator boundary and there is no `free_table`:

| Call | What it does |
|---|---|
| `fig_table_rows(t, n)` | appends `n` zeroed rows and returns them, to be filled in place |
| `fig_table_string(t, bytes, len)` | copies decoded text into the table's buffer, returning its `FigText` |
| `fig_table_anchor(t, node, name, span)` | an anchor on a node |
| `fig_table_tag(t, node, text, resolved, agrees, span)` | a tag, as written and as the language resolved it (§4.3) |
| `fig_table_marker`, `fig_table_sep` | an item's marker, an entry's separator |
| `fig_table_item_sep(t, node, text)` | a separated sequence's separator |
| `fig_table_comment(t, node, slot, style, text)` | a comment |
| `fig_table_region`, `fig_table_mention` | a section's header lines and mentions |
| `fig_table_directive(t, handle, prefix)` | a `%TAG` declaration |

A row is 32 bytes:

```c
typedef struct FigText { uint32_t off, len; } FigText; // len's high bit: the buffer, else the source
typedef struct FigRow {
    uint8_t  kind;        // FIG_ROW_NULL … FIG_ROW_ALIAS, or FIG_ROW_EXT + an ExtKind
    uint8_t  flags;       // FIG_ROW_OPEN: a separated sequence with no delimiters
    uint16_t reserved;
    uint32_t parent;      // FIG_NONE at the root; rows are in pre-order
    uint32_t start, end;  // the span, in the source
    FigText  text;        // a scalar's text, an alias's anchor name
    uint32_t first_child; // filled by fig
    uint32_t next_sibling;// filled by fig
} FigRow;
```

A language writes `parent`, never the child links: fig derives them while it
validates, in the same pass. A keyvalue's two children are its key and its
value, in that order. Booleans are two kinds rather than a kind and a text.
Every side fact is sparse, so a document with no anchors, tags or comments
carries no table for them.

`print` is handed the same `FigTable`, read-only, and reads the rows and
side facts through `fig_table_*` getters. A JS or out-of-process language
answers over the wire as now; fig decodes its JSON through the same calls.


Two savings need none of this, and are worth taking first: YAML
allocates its anchor and tag arrays at full length for a file with none
(96 bytes a node), and TOML its comment array for a file with one
comment.

## 5. Where the languages go

### 5.1 Zig: through the contract only

The languages leave `src/` for `languages/<name>/`. Each is a module that
imports the kit and `fig.h`'s contract types, and nothing else of core's:
no `document`, `ast`, `editor`, `splice` or `materialize`. It parses to a
node table, prints from one, and edits through the contract, as the JS
twins already do. Their corpora and the language-specific tests now in
`patch.zig`, `materialize.zig` and `embed.zig` move with them. Core's own
tests run against `canonical` and a small test language held in the test
tree.

This is the rule that lets a language be built apart from core. Zig has
no link-time module boundary: an archive compiles every module it
imports. A language in its own archive that imported `document` would
carry a second copy of it, with its own state and a layout only assumed
to match the copy core uses. Through the contract, there is nothing of
core's in the language's archive to duplicate.

It is a port, not a recompile. YAML and TOML each have about 850 lines of
`editor_helper.zig` written against `editor.zig`, and both reach into
core for the value and reference layers. The JS twins are the evidence
that the contract holds them (§5.6), and the conformance corpora are what
a port is held to.

A compiled format avoids a copy today because `Editor(Lang)`
instantiates over `Document` directly. Through the contract there is no
copy to avoid: the table a language writes is the `Document` (§4.9).

### 5.2 Rust and C: the payload question

`fig-sys`'s payloads become language-free. The first draft assumed a
crate per language per target, seventy in all, and proposed one shared
languages payload instead. Measured (§8 step 1), neither the split by
target nor the shared payload is needed. A crate may hold all seven
targets' archives, since crates.io's 10 MB limit is on the compressed
`.crate`:

| Crate, all seven targets | `.crate` |
|---|---|
| core, no language | 1.86 MB |
| yaml | 0.81 MB |
| figl, zon | 0.66 MB |
| toml | 0.47 MB |
| json, nestedtext, plist | 0.38–0.41 MB |
| ini, dotenv, properties | 0.32–0.33 MB |

Today's seven payloads would fit in one crate too, at 4.3 MB.

So each language can be **a crate of its own** (`fig-yaml`, `fig-toml`,
…) holding its archive for every target and exporting one
`fig_lang_<name>_register` symbol, beside a language-free `fig-sys`.
A Rust binary depending on a core-only `fig-sys` and a yaml-only
`fig-yaml` linked and ran on macOS and wasm32-wasip1, and linked for iOS,
Linux and a Windows stand-in, with no duplicate or undefined symbols,
provided neither archive bundles Zig's compiler_rt (`fig-sys` already
leaves it out). Dead-stripping works per function; Linux needs
`link_function_sections` and `link_data_sections` set, which saves
about 1.1 MB there today.

What it costs:

- Each archive carries its own copy of the std and kit code it uses:
  31–33 KB per language on macOS, 83–88 KB on Linux, measured with a
  prototype bridge that also carried a copy routine §4.9 removes.
- A release builds 77 archives into 11 crates, against 7 into 8 today.
- A language crate is built against ABI 4 and checks it when it
  registers, so its `fig-sys` requirement is the ABI's, not core's
  version.
- A crate holding every target is downloaded whole: core and the eight
  default languages come to about 5.6 MB, against about 0.6 MB a target
  today.

Per-language crates also let a language fix ship without a core release,
since a language depends on the contract's ABI, not on core's internals.
They ship in 6.0 (decided 2026-10-12): `fig-sys` is language-free in one
crate, and each language is a crate of its own.

### 5.3 TypeScript

Unchanged in shape. The wasm drops the stand-in path, and the twins
remain the npm languages.

### 5.4 The CLI and LSP

Consumers. They link every language and register them at startup, so
`fig get config.yaml …` behaves exactly as it does in 5.x. They stay in
this repository.

### 5.5 New and house formats

A `fig lang new <name>` scaffold writes a language skeleton (description,
samples, a parse and print stub, and the harness invocation) in Zig, JS
or Rust. This is the only place copied code is the distribution. The
canonical languages are packages: a vendored YAML parser is a fork that
stops getting conformance fixes, and their value is that it does not.

### 5.6 How far the twins hold today

The JS twins are the closest thing to a contract-only YAML and TOML that
exists, so they were run over the corpora the Zig scoreboards use, on
2026-10-11 at acd8701. Each was registered in the language-free wasm and
scored with the scoreboards' own checks. Then its node table was
compared with `fig lang table` from the compiled format, row for row, on
every file, and its printed output with `fig convert`.

| Corpus | Compiled | YAML twin / TOML twin |
|---|---|---|
| yaml-test-suite accept: parses | 289/289 | 289/289; every table identical |
| … prints and re-parses | 289 | 289; every printed byte identical |
| … converts to JSON | 260 | 260; identical, refusing the same 29 |
| yaml-test-suite reject | 93/93 | 93/93 |
| multi-document streams | 19/19 | 0/19 |
| YAML 1.1 resolution (tagged) | 14/14 | 11/14 |
| toml-test 1.1 valid (tagged) | 218/218 | 218/218; every table identical |
| toml-test 1.1 invalid | 488/488 | 488/488 |
| toml-test 1.0 valid (tagged) | 209/209 | 209/209; every table identical |
| toml-test 1.0 invalid | 495/495 | 486/495 |

So on what the corpora test, the twins are the compiled formats. The
differences are four, and none of them is a gap in the contract's power:

- **Streams.** The compiled path splits a stream in core
  (`Embed.extractStream`). The twin has no `splitStream` to declare, and
  refuses at the second `---`. §4.5's declaration closes it.
- **YAML 1.1 values.** The twin's tables are right (`0777` is an `int`
  row). The three failures are the bindings decoding the lexeme, §4.2's
  last paragraph. Whether the compiled path shares the fault is
  unchecked: its scoreboard decodes the raw text itself, and no CLI
  option selects compiled YAML 1.1.
- **TOML 1.0.** The twin implements 1.1 only. The nine are 1.1 additions
  (seconds-free times, newlines and a trailing comma in inline tables,
  `\x` escapes). The twin needs a `toml-1.0` dialect.
- **A printer refusal.** Converting TOML's `inf` and `nan` to JSON, the
  compiled printer refuses. The JSON twin writes `inf`, which is not
  JSON. The twins are held to the compiled formats' parse refusals and
  not yet to their print refusals.

Invalid UTF-8 reaches no language: the core rejects it before the
contract is called, which is right, and a conformance runner over the
wire cannot send it.

The parse corpora do not reach editing, so an edit corpus was built for
it in fig-conformance ([conformance suite](conformance-suite.md) §4.1):
27,503 cases generated from the twins' own fixtures, every entry, mapping
and sequence put through each `Editor` operation, with the compiled
formats' answers recorded as the expectation. On 2026-10-11 every twin
matched every case, after two fixes the corpus forced. One was a twin bug:
the ZON twin read a lone CR into a `\\` string (25ba6ae). The other was a
compiled one: compiled `.properties` spliced a trailing `#` comment into
the value, where the twin refused (a062c32).

Parity cannot see a fault that both sides share, and three exist. The
recorder checks that a comment edit changes no value, which needs no
reference, and six recorded answers fail it. They come down to three
faults in core's editor, filed as tasks:
CRLF (since fixed),
ZON multiline strings (since fixed)
and .properties continuations (since fixed: the editor ends a continued
line with a blank one before writing a line after it).

So for the twins, §8's gate stands at: parse and print parity on the
corpora, edit parity on the edit corpus, and the four differences above.

## 6. The check that holds the line

`zig build check` gains a step that scans core's sources and `fig.h` for
every registered language, dialect and container name, and fails on a
match outside a history note. It matches identifiers and string literals
as whole tokens, not substrings: `ini` must not match `initRegion`. Two
names need the rename and an exception to be checkable at all:

- `figl` makes the dialect distinct from the library. `fig` remains the
  library's name everywhere in core, and the check never looks for it.
- `json` is legitimately in core, because the wire is JSON lines
  (`wire.zig` reads it with `std.json`). The exception is stated as "the
  wire's encoding" and covers `wire.zig` and `std.json` alone.

The pins this proposal deletes were the opposite kind of check,
asserting that a specific language was present. Their replacement
asserts that none is.

## 7. What breaks

Everything here is 6.0, ABI 4.

- `FIG_FORMAT_*`, Rust `Format::Json`…`Format::Plist` and TypeScript
  core `Format` members are gone. Integers 1 to 13 mean nothing, and
  handles are process-local.
- A consumer that called `parse(…, Format::Yaml)` without registering
  gets `UnsupportedFormat` until it calls `fig_yaml::register()`.
- The authoring dialect's name is `figl`. `-i fig` and
  `format_by_name("fig")` stop resolving; there is no alias (decided
  2026-10-12).
- `fig`'s Cargo features `json` … `nestedtext` are gone. The `default`
  feature set links no language.
- Content sniffing happens only for a consumer that passes an order. The
  CLI does; a library consumer that relied on `fig_detect` sniffing must
  pass one.
- `FIG_EXT_ENUM_LITERAL`, `FIG_EXT_CHAR_LITERAL`, `FIG_EXT_PLIST_DATE`
  and `FIG_EXT_PLIST_DATA` are renamed or merged (§4.2). A plist date now
  converts as a datetime.
- The preset embed containers are gone. Embedding needs `fig-embed`
  registered, and a bare `---` means YAML only because the frontmatter
  container says so.
- `FigComments.style`, `key_style` and `section_noun` change shape, and
  every registered language built against ABI 3 has to be rebuilt.
- Setting a value by text on a language with a value renderer takes a
  typed `Literal` (§4.4).
- `fig_indent` and `flow` move out of `FigPrintOptions`, and
  `SerializeError` loses its format-named members.
- `Fig` is no longer exported from the Zig root module.

## 8. Sequencing

1. **Measure.** Done, 2026-10-12: the node table's copy, a compact
   layout adopted in place, and per-language archives on every target.
   They decided §4.9 and §5.2.
2. **Declarations.** Add `render_key`, `resolveScalar`,
   `splitStream`, `caps.root_container`, `degrades_to`, the declared
   noun, delimiter-driven comment scanning, fragment printing, the
   lexeme's value (§4.2) and the consumer's sniff order. Have every
   compiled language declare them, and switch core to read them. Each pin
   and switch is deleted in the commit that makes it redundant. All of
   this is additive, and could have shipped at ABI 3.
3. **The kit.** Move the shared helpers, and the JSON pieces the
   other languages borrow.
4. **Registered containers,** beside the existing presets, which
   become thin wrappers.
5. **The parity gate.** The [conformance suite](conformance-suite.md)
   scores any language through the contract. The twins close §5.6's four
   differences against it, and gain an editing corpus. This is the bar
   for step 6.
6. **The Document is the table** (§4.9). `FigNodeRow` and its sparse
   tables, core's accessors and `view`, the wire's decode into rows, and
   the twins and compiled languages writing them. Held to the
   conformance suites before and after.
7. **Port the Zig languages to the contract.** Each moves to
   `languages/<name>/`, drops its imports of core, and clears the same
   suite at the compiled format's scores before the compiled one is
   deleted. Core builds and tests with no language. The CLI and LSP
   register theirs. Add the §6 check.
8. **6.0, ABI 4.** Delete the identity enums, the stand-ins, the
   presets, the closed `KeyStyle` and `CommentStyle`, and the
   one-language options. Rename `ExtKind` and the dialect.
9. **Bindings.** A language-free `fig-sys` in one crate, the language
   crates (§5.2), the C headers, and the npm wasm without stand-ins.

Steps 2 to 5 would be worth doing even without 6.0, since each
replaces a switch with a declaration a registered language can also make,
or holds a registered language to the bar a compiled one meets.

## 9. Open questions


- **One repository or several.** This proposal keeps the languages in
  this repository under one version. §5.1 makes splitting them out
  possible, since a language no longer builds against core's internals.
  Splitting would follow the twins' history but multiply release work.
- **`fig-embed`'s scope.** Does it carry the Markdown and HTML
  containers together, or one package each?
- **The CLI's `--format` names.** Today the names are fixed. When
  registration is open, the CLI's `fig lang` helpers and its built-in
  languages share one namespace, so it needs a rule for a helper named
  `yaml`. §4.1's rule (refuse a second registration of a name or an
  extension) is the default; the CLI may want "the configured helper
  wins" instead.
- **One implementation or two.** With a Zig and a JS implementation of
  every language held to the same suite, is the Zig one still needed for
  native consumers, or could they run the JS twin under fig-quickjs? The
  measurements in §8 step 1 and the engine's size decide it. This draft
  assumes both.
