```fig
title = Migrating to 5.0
description = What changes for a caller of the fig CLI, the Rust crates, @diaryx/fig, @diaryx/fig-wasi or the C ABI moving to 5.0, and what to write instead
author = adammharris
created = 2026-09-26
updated = 2026-09-26
part_of = [docs](docs.md)
```

# Migrating to 5.0

5.0 is the first release in which every artifact carries one version, and the
release the breaks below were saved for. It follows cli 4.0.1, rust 4.1.0,
core 3.1.0 · npm 3.1.0 and fig-wasi 4.0.0, and each section starts from the
artifact's own last release. The [CHANGELOG](CHANGELOG.md) lists every change
with its commit, and its **Behavioural changes** section has the exact wording
of each observable difference; this page is the part of it that asks you to
change something.

## Every artifact

- **`.figl` is fig's only extension.** A `.fig` file is no longer read as fig
  by its extension, and falls back to sniffing its contents like any unknown
  extension. Rename it `.figl`, or pass `--input fig`.
- **One version.** Every crate, both npm packages, the CLI and `fig_version()`
  are 5.0.0. `fig version` prints `fig 5.0.0 "Texas Everbearing"`, without the
  `(core …)` it used to carry.

## The CLI

`docs/proposals/cli-5.md` argues each of these.

| 4.x | 5.0 |
| --- | --- |
| `fig edit <file> <path> <value>` | `fig replace <file> <path> <value>` |
| `fig edit --key <file> <path> <name>` | `fig rename <file> <path> <name>` |
| `--embed frontmatter-json` / `frontmatter-toml` / `frontmatter-fig` | `--embed semicolons` / `plus` / `fenced-fig` (the same for `--to-embed` and `--patch-embed`) |
| a value argument spliced as written (`a: b`, `007`, `[1, 2]` in YAML) | read as a fig value and written in the file's own syntax; `--raw` splices as written |
| a number, boolean, null or bracketed value into JSON written as a string | written as that value; quote it (`'"42"'`) for a string |
| `fig get <file> <path>` on a scalar printing the format's spelling | prints the scalar's text and one newline; `-o <format>` for a spelling |
| an unknown `-flag` read as a file, path or value | a usage error, exit 2; put a file or value that starts with `-` after `--` |
| a surplus positional ignored | a usage error, exit 2 |
| `fig help <arg>`, `fig version <arg>`, the argument ignored | a usage error, exit 2; `fig <action> --help` is an action's help |
| `fig comment --get` with no comment: a blank line, exit 0 | nothing, exit 1 |
| `fig insert <file> 'list[N]' <value>` for a middle `N`: exit 2 | exit 1; `set --seq` rewrites the whole sequence |

Exit codes are written down: 2 is what the command line alone gets wrong,
before the file is read, and 1 is everything the document decides. So a file
that does not parse (`get`, `fmt`, `convert`, `patch`), `--strict` stopping on
a warning, and an edit whose value the document will not take exit 1 where
they exited 2; a path argument that does not parse exits 2 where it exited 1.
A script that branched on the old numbers needs the new ones.

## Rust

Most of these are compile errors. Three are not: the `rust-version` floor; a
derived `FromValue` impl's fixed messages, and anything built by
`Error::msg_static`, now arriving as `Error::Message`, so a `match` with an
`Error::Message(_)` arm ahead of a `_` arm takes that arm; and
`Error::Language` for a declared string holding a NUL naming the byte offset.

| 4.x | 5.0 |
| --- | --- |
| any rustc | rustc 1.88 or newer (`rust-version`) |
| `fig::from_str(s)` / `fig::to_string(&x)` | `fig::from_yaml_str(s)` / `fig::to_yaml_string(&x)`, which need the `yaml` feature; `from_slice(bytes, format)` / `to_value(&x)?.serialize(format)` for any format |
| `ed.replace(p, &x)`, `insert`, `set`, `append`, `prepend` (serde) | `ed.replace_value(p, x)` &c., with `fig::to_value(&x)?` for a `Serialize` value |
| `ed.delete(p)` / `ed.remove_item(p, i)` | `ed.delete_key(p)` / `ed.delete_item(p, i)` |
| `ed.replace_key(p, k)` | `ed.rename_key(p, k)` |
| `EmbedType::FrontmatterYaml` / `FrontmatterJson` / `EndmatterYaml` / `FrontmatterFig` / `PlusToml` | `EmbedType::Frontmatter` / `Semicolons` / `Endmatter` / `FencedFig` / `Plus` |
| `EmbedType::MdFrontmatterJson` / `…Toml` / `…Fig` | `EmbedType::MdJson` / `MdToml` / `MdFig` |
| `delete_leading_comments` / `delete_dangling_comments` | `delete_leading_comment` / `delete_dangling_comment` |
| `set_sequence(p, &[Value])` | `set_sequence(p, items)` over any `impl Into<Value>` items |
| `fig::split(s, k) -> (content, body)` | `fig::split(s, k) -> (before, content, after)` |
| `Region::body`, `Extracted::body()` | `body_before`/`body_after`, `host_before()`/`host_after()` — the old `body` was the prose after frontmatter or before endmatter, and only half the host around an HTML data island |
| `Error::Language(String)` | `Error::Language(LanguageFailure)` — `.message`, `.byte_offset` |
| `Error::Static(&str)` | `Error::Message(String)`, same text |
| `Error::MissingField { field, ty }` patterns | add `..`; build with `Error::missing_field` |
| `Syntax { .., ..Default::default() }`, `Dialect { .., ..Dialect::new(n) }` | `Syntax::default()` / `Dialect::new(n)`, then assign fields |
| `SectionHeader { .. }`, `ClosedContainers { .. }`, `Comments { .. }` | `SectionHeader::new(open, close, sep)`, `ClosedContainers::new(..)`, `Comments::new(..)` |
| `RegionRow { node, start, end }` | `RegionRow::new(node, span)`; `.span.start`/`.span.end` |
| `MentionRow { .. }`, `CommentRow { .. }`, `DirectiveRow { .. }` | `MentionRow::new(..)`, `CommentRow::new(..)`, `DirectiveRow::new(..)` |
| `RenderArgs { dialect, indent, .. }` | `RenderArgs::default().dialect(..).indent(..)`; it also carries `parent_key` and `parent_tag`, the container the fragment goes into |
| `match` on `NodeKind`, `Renderer`, `Literal`, `CommentStyle`, … | add a `_` arm |
| `fig_sys::FigFormat::from(format)` | gone — a runtime `Format` has no `FigFormat` |
| `fig_sys::FigStatus::OK` as a `c_int` | a `FigStatus`: `status == FigStatus::OK` |
| `fig_sys::FigRenderValueFn`, `FigRenderEntryFn`, `FigRenderItemFn`, `FigRenderTailFn`, `FigRenderKeyFn` | `fig_sys::FigRenderFn`, taking a `*const FigRenderRequest` |
| `fig_sys::FigNativeKinds` | the `lossless` bitmask: `FIG_LOSSLESS_ENVELOPE`, `FIG_NATIVE_NULL`, `fig_native_ext(k)` |
| `match` on `fig_sys::FigNodeKind` | add `FigNodeKind::Extended`, which `fig_node_kind` reports for a TOML datetime, a ZON enum or char literal, a JSON5 non-finite number, or a plist date or data |
| `fig_sys::FigLanguageVTable`, `FigSyntax`, `FigNodeTable`, `FigPrintOptions` literals | each takes a leading `size`, the vtable a `dialect_size` and the table a `row_size` — see *The C ABI* below |

## TypeScript — `@diaryx/fig`

Node 22 or later (`engines.node` was `>=20`).

**Formats are imported.** The module compiles no format in; each is a
JavaScript language at `@diaryx/fig/languages/<name>`, registered once at
startup, after which `Format.Yaml` and the rest work as before, frontmatter
included:

```ts
import { registerLanguage } from "@diaryx/fig";
import json from "@diaryx/fig/languages/json";
import yaml from "@diaryx/fig/languages/yaml";

registerLanguage(json);
registerLanguage(yaml);
```

Register what you use: `json`, `json5` (which serves `Format.Jsonc` too),
`yaml`, `toml`, `fig`, `ini`, `dotenv`, `properties`, `nestedtext`, and now
`zon` and `plist`, which 4.x left out. A format nobody registered throws
`UnsupportedFormat`, naming the module to import; registering the same
language twice is harmless. `FIG_WASM_ZON` and `FIG_WASM_PLIST` are gone: a
module with compiled formats is `FIG_WASM_LANGUAGES=<names>|all npm run
build:wasm`, and a language named after a format compiled into it is refused
as taken. A parse error is the language's own message, with a byte offset, and
not the compiled parser's words.

| 3.x | 5.0 |
| --- | --- |
| `ed.delete(p)`, `removeItem`, `replaceKey` | `ed.deleteKey(p)`, `deleteItem`, `renameKey` |
| `deleteLeadingComments` / `deleteDanglingComments` | `deleteLeadingComment` / `deleteDanglingComment` |
| `ed.set(p, v)`, `setWith`, `setRaw` | `ed.setValue(p, v)`, `setValueWith`, `setValueRaw` |
| `EmbedType.FrontmatterYaml` / `FrontmatterJson` / `FrontmatterFig` / `PlusToml` / `EndmatterYaml` | `EmbedType.Frontmatter` / `Semicolons` / `FencedFig` / `Plus` / `Endmatter` |
| `EmbedType.MdFrontmatterJson` / `…Toml` / `…Fig` | `EmbedType.MdJson` / `MdToml` / `MdFig` |
| a stored `EmbedType` number | its string: see below |
| `split(host, kind)`, `detect(host)` | `Embed.split(host, kind)`, `Embed.detect(host)` |
| `Region.body` | `bodyAfter` for frontmatter, `bodyBefore` for endmatter |
| `EmbedContainer`, `embedParts`, `embedTypeOf` | not exported |
| `serialize(value, format, options)` | `stringify(value, format, options)`; `Document#serialize` is unchanged |
| `valueText(…)` | `stringify(…)` without its trailing newline, or your own text to a `*Raw` method |
| `new FigError(status, detail?)` | `new FigError(status, op, detail?)` |
| use after `dispose()` throwing `Error` | throws `FigError` with `status` `InvalidArgument`; the message still says "already disposed" |
| `Format[1]`, `NodeKind[k]` (reverse lookups) | keep your own name map |
| an exhaustive `switch` over `Format`, `NodeKind`, `Status`, … | add a `default` arm |

`Format`, `NodeKind`, `ExtKind`, `WarningCode`, `WarningCause`, `Status` and
`EmbedType` are `as const` objects rather than TypeScript enums. Member access
(`Format.Json`) and annotations (`f: Format`) read as before, but the numeric
types admit any number, which is why a `switch` needs its `default`.

`EmbedType` is string-valued, and a number stored under 3.x maps as: 0
`"frontmatter"`, 1 `"semicolons"`, 2 `"endmatter"`, 3 `"fenced-fig"`, 4
`"plus"`, 5–7 `"fenced-yaml"`/`"fenced-json"`/`"fenced-toml"`, 8–10
`"md-json"`/`"md-toml"`/`"md-fig"`, 11–14 `"html-script-fig"`/`"-yaml"`/
`"-json"`/`"-toml"`, 15–18 `"html-code-fig"`/`"-yaml"`/`"-json"`/`"-toml"`.

Two things change without a compile error. `Editor.open`, `Embed.open` and
`Embed.openOrInit` on unparseable input throw with the core's diagnostic
(`fig_editor_create: <reason>`) where the message was the bare `parse error`.
`Embed.split` on a mid-document HTML island returns the host on both sides of
the block, where it returned only the text after it, and a failure other than
a missing or unterminated region throws instead of returning `null`.

## `@diaryx/fig-wasi`

Node 22 or later. Its commands are the CLI's, above.

## The C ABI and runtime languages

`FIG_ABI_VERSION` is 3 and `FIG_LANGUAGE_VTABLE_VERSION` is 2. A language
built against 4.x is refused at registration ("vtable version 1 is not the 2
this fig speaks"), and has to be rebuilt against the new `fig.h`:

- The five `render_*` slots are `FigRenderFn`, taking one
  `const FigRenderRequest *` in place of positional strings. The request
  names the container the fragment goes into (`parent_key`, `parent_tag`),
  and a helper's `render` request line carries both fields, so a helper that
  rejects unknown fields must accept them.
- `FigLanguageVTable` gains `size` after `version`, and `dialect_size`;
  `FigSyntax`, `FigNodeTable`, `FigPrintOptions` and `FigRenderRequest` gain a
  leading `size`, and `FigNodeTable` a `row_size`. A table whose `row_size` is
  shorter than a row's required fields is refused.
- `lossless` is a `uint32_t` bitmask and `FigNativeKinds` is gone. A
  capability or lossless bit fig does not know, or native kinds without
  `FIG_LOSSLESS_ENVELOPE`, is refused.
- `fig_node_kind` returns `int`, and reports `FIG_NODE_EXTENDED` for a
  format-specific scalar where it reported `STRING` or `INT`;
  `fig_node_extended` says which.
