```fig
title = Core only
description = Every format moves out of core, fig's own dialect included — core names no language in its source or its C ABI, and every consumer registers the languages it uses
created = 2026-10-09
status = draft
part_of = [proposals](proposals.md)
```

# Core only

> **Status: DRAFT.** Written against `main` at f459cce (5.2.0, core ABI
> 3). Nothing here is built. §7 lists what breaks; §8 orders the work so
> that most of it lands in 5.x as additive changes and 6.0 only deletes.

## 1. The claim

The [runtime languages](runtime-languages.md) contract has been crossed
in both directions. Twelve JS twins drive the npm build, which compiles
no format in, and the compiled formats edit through the same contract as
a registered one. A format no longer needs to be inside core to be
complete. What still puts the formats there is packaging: `list.zig`'s
rows, `language.zig`'s `slots`, and the `default_on` flags that decide
what every Rust binary, static payload and C consumer links.

This proposal moves all of them out, **fig's own dialect included**, and
makes that a rule rather than a default:

> **Core names no language.** No file under `src/` that core builds
> imports a language, switches on one, or spells one's name, and neither
> does `fig.h`. A language is something a consumer registers.

The test is mechanical: grep core and `fig.h` for any format name and
find nothing outside the history notes. §6 makes that grep a check.

"Core" means what the library builds: the value model, `Document`, the
editor engine, regions, patch, diagnostics, the contract (`language.zig`,
`manifest.zig`, `runtime.zig`, `wire.zig`, `harness.zig`) and `c_api.zig`.
The CLI, the LSP and the formats themselves become consumers of it.

## 2. Where 5.2 stands

The npm package already works this way: the wasm is language-free, and
`registerLanguage(json)` is how a caller gets JSON. Rust has
`fig::Language` and `fig::register`, and a registered row named after a
compiled-out format stands in for it. Every other consumer (Rust by
default, C, Zig, the prebuilt payloads, the CLI) links the eight
`default_on` formats whether it uses them or not.

A survey of core outside tests found about forty-five couplings. They
fall into nine groups, and §4 takes each in turn:

| Group | Where | Example |
|---|---|---|
| Identity | `list.zig`, `language.zig`, `c_api.zig`, `fig.h` | `FIG_FORMAT_YAML = 3`; `slots`; `standInAbi` |
| Value model | `ast.zig`, `manifest.zig`, `fig.h` | `FIG_EXT_PLIST_DATE`; `NativeKinds` |
| References | `materialize.zig`, `ast/reader.zig`, `c_api.zig` | imports `languages/yaml/parser.zig`; `"<<"`; `tag:yaml.org,2002:`; the pin that only YAML may declare `caps.references` |
| Editor | `editor.zig`, `editor/splice.zig`, `manifest.zig` | `KeyStyle.zon_field`; `literalOf` through fig's tokenizer; hardcoded `.{`; closed `CommentStyle` |
| Embed | `embed.zig`, `fig.h` | the comptime pin requiring exactly `json`, `yaml`, `toml` and `fig`; bare `---` means YAML; `+++`/`;;;`/endmatter presets; YAML stream splitting |
| Serialization | `serialize_options.zig`, `patch.zig` | `fig_indent`; a `.fig` arm for fragments; `FigUnrepresentableRoot` |
| Diagnostics | `diagnostics.zig` | `valueLoss`, `degradedNote` and `commentsEmitted` switch over every format |
| Shared helpers | `util/number.zig`, `util/datetime.zig`, `util/json_string.zig` | `Spelling.yaml_1_2`; TOML/YAML leniency flags |
| Wire and ABI text | `wire.zig`, `fig.h` | splice style `"json_string"`; prose naming formats |

Most of these are pins and switches written before the contract could
say the thing itself, so their fix is a declaration a language makes.
Three groups are more than that: identity, the value model and embed.

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

Detection by extension and by sniff already reads each language's
`extensions` and `sniff_rank`, and stays as it is.

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

The plist mapping is the one real semantic change, because a plist date
then converts to a TOML datetime instead of degrading to a string. §9
leaves it open.

### 4.3 References

`caps.references` stays: the anchor/alias/tag layer of `Document` is a
generic concept that YAML happens to be the only user of. Its YAML parts
move into YAML through declarations:

- `syntax.merge_key` (already present) replaces the hardcoded `"<<"` in
  `ast/reader.zig` and `materialize.zig`.
- A `resolveScalar` declaration replaces the import of YAML's
  `classifyNumber` in `materialize.zig`, and covers the bool spellings
  and the core-schema tag resolution (`tag:yaml.org,2002:`, `!!`).
- The `c_api.zig` pin that only YAML may declare references is deleted.

### 4.4 The editor

| Coupling | Becomes |
|---|---|
| `KeyStyle` {verbatim, json_quoted, zon_field, bare_or_quoted} | a `render_key` renderer; every language spells its own keys |
| hardcoded `.{` in `flowOpenEnd` and `isFlow` | `syntax.flow_map_open` |
| `- * > +` in `ownsItsLines` | the declared `seq_item_marker` and related syntax |
| closed `CommentStyle` scanners | scanning driven by the delimiters `syntax.comments` declares |
| `SectionNoun` choosing `NotATable`/`NotASection`/`NotAContainer` | one neutral error carrying a noun the language declares |
| `literalOf` classifying a caller's literal with fig's `sniffBare` | the caller supplies a typed `Literal` (the type `RenderArgs.literal` already carries); the CLI, which reads value arguments as fig (CLI 5 §1), parses them with the fig language |

The last row is how fig's dialect leaves core without taking the CLI's
value grammar with it: that grammar belongs to the CLI, which now
depends on the fig language like any other consumer.

### 4.5 Embed

The strict reading of the rule takes Markdown and HTML out as well, and
this proposal takes the strict reading. Core keeps the region machinery:
locating, the span model of `FigRegion`, editing through a region, and
re-housing. Containers are registered, as languages are. A container
declares its open and close delimiters, how an info string or attribute
names the inner language, an optional codec (HTML entities), and an
optional default for an untagged block.

- Bare `---` meaning YAML becomes the frontmatter container's declared
  default, set by whoever registers that container.
- `+++` (TOML), `;;;` (JSON) and endmatter (YAML) become containers that
  pin an inner language by *name*. `FIG_EMBED_SEMICOLONS_JSON`,
  `FIG_EMBED_PLUS_TOML` and `FIG_EMBED_ENDMATTER_YAML` are deleted.
- `fig_embed_detect` iterates the registered containers.
- YAML's multi-document splitting and `%YAML` directive handling
  (`extractStream`, `segmentIsDirectives`) move into YAML as a
  `splitStream` declaration.
- The comptime pin that the embeddable set is exactly four is deleted.

The standard containers ship as a package of their own, `fig-embed`.
That costs a step for frontmatter users and is the one part of this
proposal that adds a dependency rather than removing one.

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
mapping or a sequence, which is fig's rule) and a per-kind `degrades_to`
(TOML's `integer`, plist's `string`, JSON's `quoted string`).

### 4.8 Shared helpers and the wire

`util/number.zig`'s spelling presets, the leniency flags in
`util/datetime.zig` and `util/json_string.zig` move to a Zig kit module
that the languages import. It is the Zig counterpart of the npm package's
`kit/`. The wire's splice style `"json_string"` becomes `"escaped"`, and
its escape grammar is defined in the spec, not by reference to a format.
`lossless.zig`'s `nativeFor` reads the registry entry, not the module.

`canonical` stays, because the harness checks every registration against
it. It is never registered, has no handle, and is absent from `fig.h`.
It is the harness's oracle, not a format.

## 5. Where the languages go

### 5.1 Zig

The languages leave `src/` for `languages/<name>/`, each a module of its
own that imports core and the kit, and none imported by core. Their
corpora, conformance suites and the language-specific tests now in
`patch.zig`, `materialize.zig` and `embed.zig` move with them. Core's own
tests run against `canonical` and a small test language held in the test
tree.

A Zig language registers through the same vtable as any other. Today a
compiled format avoids the `NodeTable` copy because `Editor(Lang)`
instantiates over it directly. Whether that saving matters once every
format pays the copy is the second measurement in §8. If it does, a Zig
module linked into the same image can register a `Document`-returning
`parse` through an internal slot that `fig.h` never states. That keeps
the copy off the hot path without tying the C ABI to the AST layout.

### 5.2 Rust and C: the payload question

`fig-sys`'s seven payloads become language-free. Twelve languages times
seven targets is eighty-four payload crates, which nobody should
maintain, so the languages cannot each have their own.

The proposed shape is **one more payload set**, `fig-languages-sys`. It
holds every Zig language, and each exports a single
`fig_lang_<name>_register` symbol and nothing that core references. Thin
crates (`fig-yaml`, `fig-toml`, …) each call one symbol, and ship one
header for C. A binary that never calls `fig_yaml::register` never
references YAML, so the linker's dead-code pass drops it.

That only holds if the dead-code pass can see the boundary inside a Zig
static library, which compiles a module to few objects. It needs function
sections, and has to be measured before anything is built (§8 step 1).
If it fails, the fallback is to build the languages payload from source
for any subset and prebuild only the full set: the rule `fig-sys`
already follows for non-default features.

Per-language crates also let a language fix ship without a core release.
They keep the single version of CLI 5 §6 unless that proves to be a cost.

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

## 6. The check that holds the line

`zig build check` gains a step that greps core's sources and `fig.h` for
every registered language name, container name and dialect name, and
fails on a match outside a history note. The pins this proposal deletes
were the opposite kind of check, asserting that a specific language was
present. Their replacement asserts that none is.

## 7. What breaks

Everything here is 6.0, ABI 4.

- `FIG_FORMAT_*`, Rust `Format::Json`…`Format::Plist` and TypeScript
  core `Format` members are gone. Integers 1 to 13 mean nothing, and
  handles are process-local.
- A consumer that called `parse(…, Format::Yaml)` without registering
  gets `UnsupportedFormat` until it calls `fig_yaml::register()`.
- `fig`'s Cargo features `json` … `nestedtext` are gone. The `default`
  feature set links no language.
- `FIG_EXT_ENUM_LITERAL`, `FIG_EXT_CHAR_LITERAL`, `FIG_EXT_PLIST_DATE`
  and `FIG_EXT_PLIST_DATA` are renamed or merged (§4.2). A plist date now
  converts as a datetime.
- The preset embed containers are gone. Embedding needs `fig-embed`
  registered, and a bare `---` means YAML only because the frontmatter
  container says so.
- `FigComments.style`, `key_style` and `section_noun` change shape, and
  every registered language built against ABI 3 has to be rebuilt.
- `fig_indent` and `flow` move out of `FigPrintOptions`, and
  `SerializeError` loses its format-named members.
- `Fig` is no longer exported from the Zig root module.

## 8. Sequencing

1. **Measure.** Build `libfig.a` with every language behind its own
   register symbol, link a Rust binary that registers one, and compare
   its size against 5.2. Measure the `NodeTable` copy on the YAML
   corpus. These two numbers decide §5.2 and §5.1, and neither design
   is settled until they exist.
2. **Declarations, in 5.x.** Add `render_key`, `resolveScalar`,
   `splitStream`, `caps.root_container`, `degrades_to`, the declared
   noun, delimiter-driven comment scanning and fragment printing. Have
   every compiled language declare them, and switch core to read them.
   Each pin and switch is deleted in the commit that makes it redundant.
   All of this is additive at ABI 3.
3. **The kit, in 5.x.** Move the shared helpers.
4. **Registered containers, in 5.x,** beside the existing presets, which
   become thin wrappers.
5. **Move the languages out of `src/`.** Core builds and tests with no
   language. The CLI and LSP register theirs. Add the §6 check.
6. **6.0, ABI 4.** Delete the identity enums, the stand-ins, the
   presets, the closed `KeyStyle` and `CommentStyle`, and the
   one-language options. Rename `ExtKind`.
7. **Bindings.** `fig-languages-sys` and the per-language crates, the
   C headers, and the npm wasm without stand-ins.

Steps 2 to 4 are worth doing whether or not 6.0 happens, since each
replaces a switch with a declaration a registered language can also make.

## 9. Open questions

- **Plist dates.** Should they merge into `offset_datetime` (§4.2), or
  stay a kind of their own under a neutral name (`timestamp`)? Merging is
  better for conversion. Keeping them separate is the smaller change.
- **One repository or several.** This proposal keeps the languages in
  this repository under one version. Splitting them out would follow the
  twins' history but multiply release work.
- **`fig-embed`'s scope.** Does it carry the Markdown and HTML
  containers together, or one package each?
- **Default registration for the CLI's `--format` names.** Today the
  names are fixed. When registration is open, the CLI's `fig lang`
  helpers and its built-in languages share one namespace, so it needs a
  rule for a helper named `yaml`.
