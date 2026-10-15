```fig
title = Migrating to 6.0
description = What changes for a caller of the fig CLI, the Rust crates, @diaryx/fig, @diaryx/fig-wasi, the Zig module or the C ABI moving to 6.0, and what to write instead
author = adammharris
created = 2026-10-12
updated = 2026-10-14
part_of = [docs](docs.md)
```

# Migrating to 6.0

In 6.0 fig's core names no language. Every format, the ones fig ships
included, is a language that registers with it, and a format is the handle
its registration hands back. [The proposal](proposals/core-only.md) argues
the change; this page is the part of it that asks you to change something.
6.0 is not released yet, and this page grows with it.

## Every artifact

- **The authoring dialect is `figl`.** `fig` is the library's name and no
  format's: `-i fig`, `-o fig`, `fig_format_by_name("fig")`,
  `Format::by_name("fig")` and `formatByName("fig")` find nothing. There is
  no alias. Write `figl`.
- **No fixed format numbers.** A format is a per-process handle, assigned in
  registration order from 1. Find one by name; persist the name, never the
  number.
- **`--spec` versions are dialects.** YAML 1.1 is the dialect `yaml-1.1`,
  TOML 1.0 is `toml-1.0`, and so on for every spelling `--spec` takes.
- **A byte string is one kind, `data`, in every binding.** plist's
  `<data>` kind was named for plist; it is now the kind any format's byte
  string is, and keeps its number (8) and its text, always base64. YAML's
  `!!binary` reads as one: it used to be a string under a custom tag, which
  a conversion to a format without tags refused. Now it converts to plist's
  `<data>`, and to JSON or TOML as its base64, and a byte string from plist
  is written to YAML as `!!binary`.

## The CLI

| 5.x | 6.0 |
| --- | --- |
| `-i fig` / `-o fig` | `-i figl` / `-o figl` |
| `fig lang list` under `compiled:`, a left-out format "compiled out" | under `built in:`, a left-out format "not built in" |
| a helper named after a left-out format "stands in" for it | a helper of that name *is* that format; its extension reaches it too |
| plist left out unless built with `-Dplist=true`; a `languages.figl` language named `plist` serves `.plist` | plist is built in; a configured `plist` is refused as a taken name, and `-Dplist=false` leaves the name to it |
| `--strict` counting "fig authoring warning(s)" | counts under the dialect's name: "3 yaml warning(s)" |
| messages saying "a fig value" | "a figl value" |
| `zig build -Dfig=false` | `-Dfigl=false` |

## Embedding has left fig

fig reads and edits a config document, never the file it sits in. The CLI's
`--embed`, `--to-embed`, `--patch-embed` and `get --body` are gone, and a
Markdown or HTML file is refused rather than sniffed; `-i <format>` reads
the whole file as that format. Rust `Embed`, `EmbedType`, `Extracted`,
`Region`, `split` and `detect`, TypeScript `Embed`, `EmbedType`, `Region`
and `Span`, Zig
`fig.Embed` and C `fig_embed_*`, `FigEmbed`, `FigEmbedContainer`,
`FigRegion` and `FigSpan` are gone too.

[twig](https://github.com/diaryx-org/twig) locates frontmatter, endmatter
and fenced or HTML blocks: a Markdown `metadata` node's byte spans, through
twig's query API. Open fig's `Editor` over the block's contents, edit, and
splice its source back between those spans.

## Helpers speak JSON-RPC 2.0

The helper wire is [JSON-RPC 2.0](helper-protocol.md), one message per
line. A helper built against 5.x is refused with a message saying it
needs rebuilding.

| 5.x | 6.0 |
| --- | --- |
| `{"op":"parse","dialect":…,"input":…}` | `{"jsonrpc":"2.0","id":n,"method":"parse","params":{"dialect":…,"input":…}}` |
| `{"ok":true,"table":{…}}` (`description`, `output`, `documents`) | `{"jsonrpc":"2.0","id":n,"result":{…}}`, the result itself |
| `{"ok":false,"message":m,"byte_offset":o}` | `{"jsonrpc":"2.0","id":n,"error":{"code":-32000,"message":m,"data":{"byte_offset":o}}}` |
| `"op":"render","which":…` | `"method":"render"`, `which` in `params` |

A helper written with `fig::helper::serve` or `@diaryx/fig/helper`'s
`serve` needs a rebuild and nothing else. One that answers by hand echoes
the request's `id`, and answers a method it does not have with -32601.
`fig::helper::handle` and TypeScript's `handle` answer the new envelope;
TypeScript's `LanguageError` takes a third argument, a `Refusal` code, for
a printer to say why it refuses.

The Zig `fig.Wire.Transport` trades lines: `exchangeFn` returns the
response line, where `callFn` returned a parsed `std.json.Value`, and
`fig.Wire.call(transport, arena, method, params)` sends one request. The
wire needs the `zigjr` package, which `build.zig.zon` names; a build that
does not carry the wire passes `-Dhelper-wire=false` and fetches nothing.

## Rust

| 5.x | 6.0 |
| --- | --- |
| `ExtKind::PlistData`, `NativeKinds::plist_data` | `ExtKind::Data`, `NativeKinds::data`: any format's byte string, its text base64 as before |
| `Format::Json`, `Format::Yaml`, … | `Format::by_name("json")` → `Option<Format>`; an unregistered format is `None` |
| `Format::Runtime(RuntimeFormat)`, `RuntimeFormat` | gone: a registered language's handle is a `Format` like any other |
| `match format { Format::Yaml => … }` | compare handles, or `format.name() == "yaml"` |
| `Debug` printing `Yaml` | `Format(<n>)`; use `name()` |
| the crate feature `fig` | `figl` |
| `fig_sys::FigFormat`, `FIG_FORMAT_RUNTIME_BASE` | gone; formats are `c_int` handles; `fig_sys::fig_format_name` names one |
| the crate features `json`, `yaml`, `toml`, `zon`, `figl`, `ini`, `dotenv`, `properties`, `plist`, `nestedtext` | gone: depend on the language's crate, `fig-yaml` and the rest; its `format()` is its format, registered the first time it is asked for |
| a language there by default | nothing is: `fig-sys` carries no language, and `Format::by_name("yaml")` is `None` until something registers YAML (`fig_yaml::format()`, `fig_yaml::register()`, or any of the crate's functions) |
| `fig::from_yaml_str`, `fig::to_yaml_string` | `fig_yaml::from_str`, `fig_yaml::to_string`; every language crate has them, with `from_slice`, `from_reader`, `to_vec` and `to_writer` |
| no serde unless `fig`'s `serde` feature was asked for | a language crate's `serde` feature is on by default and turns on `fig`'s; a build that wants no serde depends on each language crate with `default-features = false, features = ["fig"]` (with no features at all, a language crate is the language alone, with no `fig` binding) |
| a mapping error for a nested field returned as itself (`Error::MissingField { .. }`, `Error::TypeMismatch { .. }`, …) | `Error::At { path, error }`, which names where (`servers[1].port`) and leads its text with the path; match on `e.innermost()`, and read `e.path()` |
| `Document` and `Editor` neither `Send` nor `Sync` | both `Send`; neither `Sync` |
| `fig::helper` always there | behind the `helper` feature, which adds `serde_json` |
| `helper::handle`, `encode`, `decode`, `table_to_value`, `table_from_value`, `description_to_value`, `description_from_value`, `syntax_to_value`, `syntax_from_value` trading `fig::Value` | they trade `serde_json::Value`; the wire needs no language registered |

On a tier-1 target nothing needs Zig, a language crate included: the
`fig-sys-<target>` payload carries every language's library beside the
core. A build inside a checkout of fig builds from source unless
`FIG_SYS_PREBUILT=1`.

Error text names the dialect ("would not parse
as toml"). `ParseError::byte_offset` is filled where the language reports
one. Registering a language under a name already taken is refused with
`Error::Language`.

## TypeScript — `@diaryx/fig`

`Format` is a type only: a branded handle, no longer an object of members.

```ts
import { registerLanguage, formatByName } from "@diaryx/fig";
import yaml from "@diaryx/fig-yaml";

const yamlFormat = registerLanguage(yaml); // also yaml.format
formatByName("yaml") === yamlFormat;       // true
```

| 5.x | 6.0 |
| --- | --- |
| `ExtKind.PlistData`, the wire's `"plist_data"` | `ExtKind.Data`, `"data"`: any format's byte string, its text base64 as before |
| `Format.Json`, `Format.Yaml`, … | `registerLanguage(json)`'s return, `json.format`, `formatByName("json")` or `requireFormat("json")` |
| a raw number as a `Format` | does not type-check; a number nobody registered throws `UnsupportedFormat` |
| `@diaryx/fig/languages/<name>`, a format written in JavaScript | `@diaryx/fig-<name>`, a package of its own you install beside `@diaryx/fig`: fig's own Zig, compiled to a WebAssembly module of its own. The JavaScript is at `@diaryx/fig/js/<name>` |
| `@diaryx/fig/languages/fig` | `@diaryx/fig-figl` |
| `@diaryx/fig/languages/json5` | `@diaryx/fig-json`, whose language has JSON5 and JSONC as dialects: `formatByName("json5")` |
| `await init()` in a browser | `await init(json, yaml, …)` with the languages you register, so their modules compile asynchronously too |

`formatName(format)` names a handle, and `requireFormat(name)` throws
`UnsupportedFormat` naming the package to import. `registerLanguage` writes
a non-enumerable `format` onto the language object.

## Zig

| 5.x | 6.0 |
| --- | --- |
| `ExtKind.plist_data`, `NativeKinds.plist_data` | `.data`: any format's byte string, its text base64 as before |
| `fig.Language.JSON.Parser.parse(a, src, .JSON)` | `fig.Runtime.Language.Parser.parse(a, src, fig.Runtime.entryByName("json").?.typeOf())` |
| `fig.Editor(fig.Language.TOML)` | `fig.Editor(fig.Runtime.Language)` with `.format = entry.typeOf()` |
| `fig.Serialize.serialize(&ast, w, .yaml)`, `SerializeFormat` | `serialize(&ast, w, fig.Serialize.Target.named("yaml").?)`; a `Target` is `.canonical` or `.{ .language = entry }` |
| `fig.deserialize.parseFromSlice(T, a, src, .yaml, opts)` | `…(T, a, src, "yaml", opts)`; `error.UnsupportedFormat` for a name nobody registered |
| `fig.Language.detect`, `Detected`, `sniff_order` | `fig.Runtime.detectAmong(a, input, order)` in your own order |
| `fig.Diagnostics.analyze(…, .toml, …)`, `analyzeFor` | `analyze(…, target, …)`; `Warning.render(w, target.name())` |
| `fig.Fig`, `fig.Language.of`, `dialects`, `compiled`, `moduleFor` | `fig.bundle.Language(name)` / `fig.bundle.module(name)` for a shipped language's Zig module |
| `ast.nodes[id]`, `ast.nodes.len` | `ast.node(id)` (a `Node` view, its `kind` switchable as before), `ast.nodeCount()` |
| `ast.node_tags[id]`, `ast.node_anchors[id]`, `ast.node_comments[id]` | `ast.tagOf(id)`, `ast.anchorOf(id)`, `ast.comments(id)`; `ast.tagFact(id)` for a tag's resolution |
| `ast.anchors`, `ast.owned_strings` | `ast.anchorIterator()`; the AST owns its strings |
| `doc.node_spans[id]` and the other `Document` span tables | `doc.spanOf(id)` / `doc.span(node)`, `anchorSpan`, `tagSpan`, `markerSpan`, `sepSpan`, `itemSep`, `regionsOf`, `mentionsOf` |
| `AST{ .nodes = … }` built by hand | `AST.Builder`, or `AST.fromNodes(allocator, root, nodes)` |
| `Runtime.tableToDocument(a, src, &t)` with `t` reused after | it adopts `t` as the document's storage and leaves it empty; `t.clone(a)` first to keep a copy |
| `SerializeError` without `PrintRefused` | an exhaustive `switch` needs a `PrintRefused` arm |

The `AST` is the node table a language's parse filled, adopted without a
copy: rows, a string buffer, and side facts sorted by node. A Zig parser
fig ships writes its rows into that table through `fig.RowWriter`; its
module's `parse` returns the adopted `Document`, and `parseTable` writes
into a table fig hands it.

The languages a build bundles register on the first lookup; call
`fig.bundle.registerAll(allocator)` first to choose the allocator, which
every later `Runtime.register` must share.

## The C ABI

`libfig` carries no language. Link a language's own library beside it —
`libfig_yaml.a`, from `zig build c-languages` — include its header,
`<fig/yaml.h>`, and call `fig_yaml_register(&format, &err)` before using
the format. `fig_table_extend` appends a whole list to a table at once.

| 5.x | 6.0 |
| --- | --- |
| `FIG_EXT_PLIST_DATA` | `FIG_EXT_DATA`, the same value (8): any format's byte string, its text base64 as before |
| `FigFormat`, `FIG_FORMAT_JSON` … `FIG_FORMAT_NESTEDTEXT` | gone; `int fig_format_by_name("json")`, -1 for a name nobody registered |
| `FIG_FORMAT_RUNTIME_BASE`, a registered language's handle ≥ 4096 | gone; every handle counts from 1 |
| a language named after a compiled-out format taking its number | refused if the name is taken; otherwise a handle like any other |
| — | `const char *fig_format_name(int format)` |
| a print failure's reason lost | a language sets `FigError.code` to a refusal, and fig reports it as the matching failure |

The static library still bundles the languages its build includes and
registers them on the first lookup of any format; `fig_format_by_name`
finds them by name. A parse failure fills `FigError.byte_offset` where the
language reports one.
