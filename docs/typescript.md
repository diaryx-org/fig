```fig
title = Using fig in Typescript
author = adammharris
created = 2026-07-05T21:35:14-06:00
updated = 2026-09-12T11:00:00-06:00
part_of = [docs](docs.md)
```

# fig for TypeScript & JavaScript

`fig` parses, **edits**, and serializes configuration files — JSON, JSONC, JSON5,
YAML, TOML, INI, dotenv, Java `.properties`, NestedText, and the native `fig`
dialect — from one small package (ZON and Apple property lists too, if you
build your own module — see [Formats](#formats)). Its
distinguishing feature is *comment-preserving editing*: you can change one value
deep in a YAML or TOML file and every comment, blank line, key order, and quoting
style elsewhere stays byte-for-byte identical. It also converts losslessly
between formats and edits config embedded in markdown frontmatter.

The core is a Zig library compiled to WebAssembly and embedded directly in the
package, so there is **no native build step and no separate `.wasm` file to
serve** — it runs in Node, Bun, Deno, and the browser.

- [Install](#install)
- [Loading the module](#loading-the-module)
- [Quick start](#quick-start)
- [Formats](#formats)
- [Reading data](#reading-data)
- [The value tree](#the-value-tree)
- [Editing without reserializing](#editing-without-reserializing)
- [Markdown frontmatter & embeds](#markdown-frontmatter--embeds)
- [Serialization options](#serialization-options)
- [Diagnostics & lossless conversion](#diagnostics--lossless-conversion)
- [Runtime languages](#runtime-languages)
- [Errors](#errors)
- [Managing resources](#managing-resources)
- [API reference](#api-reference)

## Install

```sh
npm install @diaryx/fig
```

Requires Node 20+ (or any runtime with `Symbol.dispose`). The package is ESM-only
and ships its own TypeScript types. Working on the binding itself needs a newer
Node — see [Developing the binding](#developing-the-binding).

## Loading the module

The WebAssembly core initializes **lazily** — the first call that needs it
compiles it. Under Node, Bun, Deno, and Web Workers that "just works" and you can
call any API directly:

```ts
import { parse, Format } from "@diaryx/fig";

parse('{"ok":true}', Format.Json); // → { ok: true }
```

In the **browser main thread**, synchronous compilation of a module larger than
4 KB is disallowed by the platform, so call `init()` once at startup before any
other fig API:

```ts
import { init, parse, Format } from "@diaryx/fig";

await init();                       // do this once, e.g. during app bootstrap
parse('{"ok":true}', Format.Json);  // now synchronous everywhere
```

`init()` is idempotent and safe to call anywhere; `isReady()` reports whether the
module is already loaded. If you forget it on a browser main thread, the first
call throws a clear error telling you to `await init()`.

## Quick start

```ts
import { parse, stringify, convert, Format } from "@diaryx/fig";

// Parse any format straight to plain JS values.
const cfg = parse('name = "fig"\nport = 8080\n', Format.Toml);
// → { name: "fig", port: 8080 }

// Serialize plain JS values to any format.
stringify({ name: "fig", tags: ["a", "b"] }, Format.Yaml);
// → "name: fig\ntags: [a, b]\n"

// Convert one format to another in a single call (comments preserved where the
// target allows).
convert("name: fig\nport: 8080\n", Format.Yaml, Format.Json);
// → '{\n  "name": "fig",\n  "port": 8080\n}\n'
```

`parse` takes an optional type parameter to assert the shape you expect (no
runtime check is performed):

```ts
interface Config { name: string; port: number }
const cfg = parse<Config>('name = "fig"\nport = 8080\n', Format.Toml);
cfg.port; // typed as number
```

## Formats

| `Format` | Parse | Edit | Serialize | Notes                                |
| -------- | :---: | :--: | :-------: | ------------------------------------ |
| `Json`   |  ✅   |  ✅  |    ✅     | Strict JSON (no comments).           |
| `Jsonc`  |  ✅   |  ✅  |    ✅     | JSON with `//` and `/* */` comments. |
| `Json5`  |  ✅   |  ✅  |    ✅     | Unquoted keys, trailing commas, etc. |
| `Yaml`   |  ✅   |  ✅  |    ✅     | YAML 1.2.2 / 1.1.                    |
| `Toml`   |  ✅   |  ✅  |    ✅     | TOML 1.0 / 1.1, incl. datetimes.     |
| `Fig`    |  ✅   |  ✅  |    ✅     | The native `fig` authoring dialect.  |
| `Ini`    |  ✅   |  ✅  |    ✅     | `[section]` + `key = value`. Untyped scalars — `port = 8080` reads back as the string `"8080"`. |
| `Dotenv` |  ✅   |  ✅  |    ✅     | Flat `KEY=value`: no nesting, untyped scalars. |
| `Properties` | ✅ |  ✅  |    ✅     | Java `.properties`; same flat, untyped limits as `Dotenv`. |
| `Nestedtext` | ✅ |  ✅  |    ✅     | [NestedText](https://nestedtext.org) — nested (dict/list) but deliberately untyped. |
| `Zon`    |  ⚠️   |  ⚠️  |    ⚠️     | Zig Object Notation — opt-in build, see below. |
| `Plist`  |  ⚠️   |  ⚠️  |    ⚠️     | Apple XML property list; typed and nested — opt-in build, see below. |

`Zon` and `Plist` are fully editable — full parity with every other format —
but they are **not compiled into the wasm module published to npm**. ZON is the
newest editable format and the least likely to be needed by a typical
JSON/YAML/TOML/Fig consumer; plist's XML parser is the heaviest of the group.
Both are left out to keep the inlined base64 payload smaller for everyone else.
To get a module with either, build your own from a checkout:

```sh
FIG_WASM_ZON=1 npm run build:wasm     # add ZON
FIG_WASM_PLIST=1 npm run build:wasm   # add plist
```

That module parses, edits, and serializes the added format exactly like any
other. Whichever module you're running, don't hard-code the table above — ask
the build at runtime, since a format can be compiled out:

```ts
import { capabilities, Format } from "@diaryx/fig";

capabilities(Format.Toml); // → { read: true, edit: true, serialize: true }
capabilities(Format.Zon);  // → { read: false, edit: false, serialize: false } in the published module
                           // → { read: true, edit: true, serialize: true } after a FIG_WASM_ZON=1 build
```

The four untyped formats — `Ini`, `Dotenv`, `Properties`, `Nestedtext` — parse
every scalar as a string; nothing infers a number or a boolean from them. The
first three are also shape-limited (`Ini` holds one level of sections; `Dotenv`
and `Properties` are flat), so serializing a nested value to one of them drops
what it cannot hold — run `diagnose` first to see exactly what.

## Reading data

For most cases, `parse` is all you need. When you want one value out of a large
document without materializing the whole thing, open a `Document` and use `get`:

```ts
import { Document, Format } from "@diaryx/fig";

using doc = Document.parse(
  "[server]\nhost = \"localhost\"\nports = [80, 443]\n",
  Format.Toml,
);

doc.get(["server", "host"]);     // → "localhost"
doc.get(["server", "ports", 1]); // → 443  (numbers index sequences)
doc.has(["server", "tls"]);      // → false
doc.toJS();                      // → the whole document as plain JS
```

`using` (a TC39 explicit-resource-management declaration) releases the native
handle automatically at the end of the scope — see
[Managing resources](#managing-resources).

A lower-level node API (`root()`, `firstChild()`, `nextSibling()`,
`childCount()`, `kind()`, `keyOf()`, `valueOf()`, `asBool()`, `asString()`,
`asNumberRaw()`, `asExtended()`) is also available for walking the tree by hand;
`get`/`toJS`/`toValue` are built on top of it and cover almost every need.

## The value tree

`toJS()`/`parse()` give you plain JavaScript. Because JS can't represent every
config value faithfully, note:

- **Integers** that fit a safe JS number come back as `number`; larger ones come
  back as `bigint`. `fromJS`/`stringify` accept both.
- **Maps** with all-string keys become plain objects — *unless* a key is an
  "array index" string (e.g. `"0"`, `"10"`), in which case you get a `Map`
  instead, because JS objects would silently reorder those keys. Non-string keys
  always yield a `Map`.
- **Format-specific scalars** (TOML datetimes, ZON enum/char literals) round-trip
  as their source text.

When you need full fidelity — distinguishing `int` from `uint`, ordered non-string
keys, or building datetimes — use the `Value` tree and its `V` constructors:

```ts
import { V, serialize, Format } from "@diaryx/fig";

const value = V.map([
  [V.string("name"), V.string("fig")],
  [V.string("nums"), V.seq([V.int(1), V.int(2)])],
]);

serialize(value, Format.Json); // '{\n  "name": "fig",\n  "nums": [\n    1,\n    2\n  ]\n}\n'
```

`fromJS(jsValue)` lifts plain JS into a `Value`; `toJS(value)` lowers it back.

## Editing without reserializing

This is what sets `fig` apart. `Editor` splices only the bytes of the node you
touch — everything else in the file is preserved exactly.

```ts
import { Editor, Format } from "@diaryx/fig";

using ed = Editor.open(
  "# app config\nhost = \"localhost\"  # dev box\nport = 8080\n",
  Format.Toml,
);

ed.replaceValue(["port"], 9090);
ed.set(["debug"], true); // replace if present, else insert

console.log(ed.source());
// # app config
// host = "localhost"  # dev box
// port = 9090
// debug = true
```

Edits are addressed by a **path** — an array of `string` (mapping key) and
`number` (sequence index) `Segment`s. An empty path `[]` is the document root.
Values you pass are rendered in the document's own format automatically (a string
becomes `"x"` for TOML/JSON but a bare `x` for YAML), so pass plain JS or a
`Value` and let fig frame it.

Common operations (available on both `Editor` and `Embed`):

```ts
ed.insertValue([], "key", value);      // add a mapping entry
ed.replaceValue(path, value);          // change a value
ed.replaceKey(path, "newKey");         // rename a key (framed as the format's string)
ed.set(path, value);                   // upsert (replace or insert)
ed.delete(path);                       // remove a mapping entry
ed.appendValue(["list"], value);       // push onto a sequence
ed.prependValue(["list"], value);
ed.removeItem(["list"], 0);            // remove sequence item by index
ed.moveKey(["a"], ["b"]);              // reorder mapping entries
ed.reorderKeys([], ["title", "body"]); // named keys first, rest follow
ed.moveItem(["list"], 2, 0);           // reorder sequence items
ed.reorderItems(["list"], [2, 0]);     // bring these indices to the front
ed.setSequence(["tags"], ["c", "a"]);  // reconcile a list, keeping survivors' comments
```

`replaceValue`, `insertValue` and `set` each have a `*With` twin taking a
`SerializeOptions`, for when the spliced value's own rendering needs
controlling — `replaceValueWith`, `insertValueWith`, `setWith`.

### Whole containers (`Editor` only)

The operations above address a container the same way they address a scalar: by
the one range of source it occupies. A TOML `[header]` table occupies no such
range — its body is the lines after the header, and an `[a.b]` header further
down the file extends it — and neither does an INI `[section]` or a `fig` block
container. At a path naming one, `delete`, `replaceValue`, `moveKey` and
`reorderKeys` all throw `InvalidArgument` rather than rewrite the header and
leave the entries behind. These six are the route for those shapes:

```ts
ed.deleteContainer(["a"]);                        // header + body, every region
ed.insertContainer(["c"], "z = 3\n");             // a new [c] with these entries
ed.renameContainer(["a"], "q");                   // [a], [a.b] and [[a.c]] alike
ed.moveContainer(["a"], null);                    // null = to the end of the document
ed.reorderContainers(["b", "a"]);                 // top-level containers
ed.appendContainerToSeq(["bin"], 'name = "b"\n'); // a new [[bin]]
```

The body argument is verbatim entry lines in the document's format, spliced and
reparsed like any other edit, so a body that doesn't parse rolls the document
back.

Support varies by format, and an unsupported operation throws
`UnsupportedFormat`:

| | `Toml` | `Ini` | `Fig` | others |
|---|---|---|---|---|
| `deleteContainer`, `moveContainer`, `reorderContainers` | ✓ | ✓ | ✓ | — |
| `insertContainer`, `renameContainer`, `appendContainerToSeq` | ✓ | — | — | — |

"Others" is not a gap: `Yaml`, `Json` and the rest nest a container in one
contiguous region, so `delete` and `replaceValue` already handle it — which is
why they succeed on a YAML block mapping where TOML's refuse.

These live on `Editor` and not on `Embed`: the C ABI has no `fig_embed_*`
twins, since no embed archetype hosts a scattered-container format today.

`setSequence` has a narrower domain than the rest: it matches new items to old
ones by *value* so a kept-or-merely-reordered item keeps its comments, which
means each item has to parse as a standalone document. It therefore throws
`InvalidArgument` on `Format.Toml` (whose scalars can't stand alone), on an
empty list on either side, and on any non-scalar item. Nothing is lost there —
a TOML inline array carries no per-element comments, so `replaceValue` on the
whole list is equivalent. It earns its keep on `Yaml` and `Fig`, where per-item
comments are real.

Comments are first-class:

```ts
ed.addLeadingComment(["port"], "the listening port"); // own-line comment above
ed.setTrailingComment(["port"], "default 8080");      // same-line comment
ed.getLeadingComment(["port"]);   // read it back ("" = bare marker, null = none)
ed.getTrailingComment(["port"]);  // same convention
ed.deleteTrailingComment(["port"]);
ed.deleteLeadingComments(["port"]); // drops the whole owned block
```

The comment marker (`#`, `//`, `;`) is chosen for the format; strict `Json` has
no comments and throws `UnsupportedFormat` if you try. Not every format has
both halves either: `Ini` and `Nestedtext` have real leading comments but no
trailing-comment syntax, so `setTrailingComment` throws there too — a `;`/`#`
after a value on those formats' value lines is literal text, not a comment.

A container has a third anchor — the **dangling** run at the end of its body,
after its last entry, which is where a commented-out *last* entry lives. It is
addressed by the container's own path (`[]` = the document root):

```ts
ed.addDanglingComment(["server"], "was: here"); // at the body's child depth
ed.getDanglingComment(["server"]);   // "" = bare marker, null = none
ed.deleteDanglingComments([]);       // the run at the end of the document
```

A scalar has no body to end, and neither has a flow container written on one
line (`{ "a": 1 }`) — both throw `InvalidArgument`. A pretty-printed JSONC
object is fine: its `// note` before the closing brace is the root's dangling
run.

An entry can also be turned INTO a comment run and back, which is how a
structural editor shows a disabled row:

```ts
ed.commentOut(["server", "port"]);
// server:
//   # port: 8080
//   host: local
ed.uncommentLeading(["server", "host"], 0, 1); // byte-identical again
```

`commentOut` prefixes every line of the node's span with the marker at that
line's own indentation, leaving the node's own leading block above it
untouched; afterwards the tree has no node at that path. The run is the leading
block of whatever followed it, or — when the entry was last — the parent's
dangling run, which `uncommentDangling(containerPath, firstLine, lineCount)`
addresses instead. Lines are taken by index within the block because *which*
lines are an entry is the caller's judgement; if the result does not parse, or
parses to a document whose other nodes moved, the splice is rolled back and the
call throws `ParseError` or `UnsupportedOperation` with the document
byte-for-byte as it was.

Need to insert already-serialized text verbatim (e.g. preserving exact quoting)?
Every value method has a `*Raw` twin — `replaceValueRaw`, `insertValueRaw`,
`appendValueRaw`, `prependValueRaw`, `setRaw` — that takes a string instead of a
`Value`.

## Markdown frontmatter & embeds

`Embed` edits a config block embedded in a host file — YAML/JSON/`fig`
frontmatter, or YAML endmatter — leaving the fences and surrounding prose intact.

```ts
import { Embed, EmbedType } from "@diaryx/fig";

const md = "---\ntitle: Hello\ntags:\n- draft\n---\n# Body\n\ntext\n";

using fm = Embed.open(md, EmbedType.FrontmatterYaml);
fm.set(["title"], "Hello, world");
fm.appendValue(["tags"], "published");

console.log(fm.render());
// ---
// title: Hello, world
// tags:
// - draft
// - published
// ---
// # Body
//
// text
```

`EmbedType` selects the container *and* the inner format — four container
families crossed with the four embeddable formats (JSON, YAML, TOML, fig):

| Container | Variants |
| --------- | -------- |
| Markdown frontmatter | `FrontmatterYaml` (bare `---`), `MdFrontmatterJson` (`---json`), `MdFrontmatterToml`, `MdFrontmatterFig` |
| Fenced code block | `FrontmatterFig` (```` ```fig ````), `FencedYaml`, `FencedJson`, `FencedToml` |
| HTML data island | `HtmlScriptFig`, `HtmlScriptYaml`, `HtmlScriptJson`, `HtmlScriptToml` — `<script type="application/…">` |
| HTML visible code | `HtmlCodeFig`, `HtmlCodeYaml`, `HtmlCodeJson`, `HtmlCodeToml` — `<pre><code class="language-…">` |

Plus three conventions with their own distinct delimiter: `FrontmatterJson`
(`;;;`), `PlusToml` (`+++`, the Hugo/Zola convention), and `EndmatterYaml` (a
trailing ```` ```endmatter ```` block). The first four names are historical —
`FrontmatterJson` is the `;;;` form and `FrontmatterFig` the fenced one — and
are kept because their ABI values are frozen.

The `HtmlCode*` variants are entity-encoded on disk. Editing decodes on open and
re-encodes span-aware on `render`, so an edit preserves every untouched byte's
original encoding and canonically encodes only what changed.

- `Embed.openOrInit(host, kind)` creates the block if none exists, so the first
  `set` lands cleanly.
- `Embed.extract(host, kind)` / `split(host, kind)` locate the region *without*
  parsing — handy for just reading the raw frontmatter and body apart.
- `detect(source)` sniffs which `EmbedType` a host opens with, or `null`.
- `replaceBody(text)` swaps the prose while keeping the (possibly edited) config.
- `Embed.retype(host, from, to, content)` re-houses the block under a *different*
  archetype's fences — the splice half of "convert this file's embed style", with
  `content` the already re-serialized inner document. Every host byte outside the
  block survives, and the block moves only when the target puts it at the other
  end of the file, so retyping to the same archetype is a byte-identical rebuild.
  Moving a mid-document block (`HtmlScript*`, `HtmlCode*`) to an edge archetype
  throws `Status.UnsupportedOperation`: there is no honest place to put the host
  text above it. Mid-document to mid-document splices in place.

## Serialization options

`stringify`, `serialize`, `convert`, and `Document.serialize` all take an optional
`SerializeOptions`:

```ts
stringify(value, Format.Json, { pretty: false });   // minified
stringify(value, Format.Json, { indent: 4 });       // 4-space indent
stringify(value, Format.Toml, { width: 40 });       // inline vs [section] budget
convert(src, Format.Yaml, Format.Json, { stripComments: true });
```

| Option          | Applies to           | Meaning                                            |
| --------------- | -------------------- | -------------------------------------------------- |
| `pretty`        | JSON, ZON, TOML      | Multi-line (default) vs. compact. For TOML it gates array wrapping. |
| `indent`        | JSON, TOML           | Spaces per level (default 2).                      |
| `width`         | TOML, YAML, Fig      | Column budget for inline (flow) vs. expanded layout (default 80). |
| `stripComments` | all                  | Drop carried comments instead of emitting them.    |
| `lossless`      | `Document`/`convert` | Round-trip values the target can't natively hold.  |

Two gotchas on `width`: `0` means *unset* and resolves to the default 80 (a
zero-initialized options struct is ordinary across the C ABI), so pass `1` to
force block layout; and for YAML the budget governs nested containers only — a
root mapping or sequence always renders block.

## Diagnostics & lossless conversion

Converting between formats can lose information — TOML has no `null`, JSON has no
datetimes or comments. `diagnose` tells you exactly what *would* be lost, without
doing it:

```ts
import { Document, Format, WarningCode } from "@diaryx/fig";

using doc = Document.parse("a: null\nb: 1 # keep\n", Format.Yaml);

// TOML has no null, so `a` would be dropped (its comments would survive).
doc.diagnose(Format.Toml); // → [{ code: ValueDropped, path: "a", ... }]

// Strict JSON has no comments, so the `# keep` comment on `b` would be dropped.
doc.diagnose(Format.Json); // → [{ code: CommentDropped, path: "b", ... }]
```

To *preserve* those values instead, pass `{ lossless: true }` — unrepresentable
values are round-tripped through a `$fig` envelope, and `diagnose` then reports
nothing lost:

```ts
convert("a: null\nb: 1\n", Format.Yaml, Format.Toml, { lossless: true });
```

There's also a top-level `diagnose(value, format, options?)` for a built `Value`.

## Runtime languages

A format the module did not compile in can be an object. `registerLanguage`
takes a `Language` — what the format declares, and the functions it is — and
hands back a `Format` that every call above accepts from then on, at the tier
its `caps` declare: `Document.parse`, `Editor.open`, `convert`, `capabilities`,
all of them, with the format's own parser reading and its own printer writing,
and fig's splice engine editing between them.

```ts
import { registerLanguage, parse, convert, Editor, Format, type Language } from "@diaryx/fig";

const tinykv: Language = {
  name: "tinykv",
  caps: { read: true, edit: true, serialize: true },
  max_mapping_depth: 0,                     // flat: no mapping inside the root
  syntax: {
    comments: { style: "hash", line: { open: "#" }, trailing: { open: "#" } },
    kv_sep: "=",
    empty_map_literal: "{}",
    flow_containers: false,
  },
  dialects: [{ name: "tinykv", extensions: ["tkv"], splice: "raw", empty_doc_seed: "" }],
  samples: ["a=1\n# two\nb=two words # trailing\n"],

  parse(dialect, input) {
    // one row per node, in pre-order; row index is node id; spans are
    // BYTE offsets into the input, [start, end)
    const rows = [{ kind: "mapping", parent: null, span: [0, input.length] }];
    // … a keyvalue row, then its key row, then its value row, per line
    return { rows, comments: [] };
  },
  print(dialect, table, options) {
    // the same table back, spans absent; return the document as text
    return "…";
  },
};

const fmt = registerLanguage(tinykv);
parse("a=1\n", fmt);                        // → { a: "1" }
convert("a=1\n", fmt, Format.Json);         // → '{\n  "a": "1"\n}\n'
using ed = Editor.open("a=1\n", fmt);
ed.set(["b"], "2");                          // the splice engine writes `b=2` from `syntax`
ed.source();                                 // → "a=1\nb=2\n"
```

The object is fig's **helper wire**, as a value: the same `description` a
helper process answers `describe` with, the same node table its `parse`
answers, spelled with the wire's own field names — `max_mapping_depth`,
`empty_doc_seed`, `ext_kind` — rather than camelCase, on purpose. That is
what makes it one contract across hosts: a `.lua` script for `fig-lua`, a
Rust `Language`, and this object all declare the same fields, and what
`fig lang table <file>` prints for any file the CLI can read is exactly what
your `parse` must return for it. The shapes are documented once, on the
`helper` module of the [Rust crate](https://docs.rs/fig) (`bindings/rust/fig/src/helper.rs`);
the TypeScript types `Language`, `NodeTable`, `NodeRow`, `Syntax` and the
rest mirror them field for field, and `docs/proposals/runtime-languages.md`
in the core is the design.

**What registration checks.** The description is validated by the rules a
compiled format is held to, and every sample is parsed, printed, reparsed and
edited before anything is registered — so a language whose `syntax` cannot
splice its own samples, or whose `print` does not round-trip its `parse`, is
refused with the reason as a `FigError`, and nothing is registered. A name
already taken — a compiled format's, or a language registered earlier — is
refused the same way; twin a compiled format under a name of your own
(`js-dotenv`, not `dotenv`). Refuse input from `parse` by throwing a
`LanguageError` with a message and byte offset; anything else you throw is
reported as a parse error without one. A registered language lives for the
rest of the process, and its `Format` integer is assigned per process — persist
the **name** and resolve it with `formatByName(name)`, which also answers for
compiled formats (`formatByName("yaml")` is `Format.Yaml`).

**Editing needs no code.** fig's splice engine writes an edit from `syntax`
alone — what a comment looks like, what separates a key from its value, how
containers open and close — and reparses through your `parse` to find its
spans. A format whose fragments cannot be spelled from constants (a typed
element, an entry whose key wraps its value) declares `renderers` and answers
`render(which, args)` for `"value"`, `"entry"`, `"item"`, `"tail"` or `"key"`;
the value renderer is told what fig's own literal rules made of the text
(`args.literal`: `"int"`, `"bool"`, `"string"`, …), so every format means the
same thing by `42` and a renderer spells a kind rather than deciding one.

**The same object is a CLI helper.** `serve(lang)` runs the wire over a
process's stdin and stdout, which is what the `fig` command line speaks to a
helper it spawns — so the language you wrote for the browser is a format the
CLI reads, converts and edits, by name or by extension:

```js
// ~/.config/fig/languages/tinykv.mjs
import { serve } from "@diaryx/fig";
import { tinykv } from "./tinykv-language.mjs";
await serve(tinykv);
```

```fig
# ~/.config/fig/languages.figl
language[]
> name = tinykv
> extensions = [tkv]
> command = [node, ~/.config/fig/languages/tinykv.mjs]
```

```sh
fig get settings.tkv
fig lang check tinykv                          # the same harness registration runs
fig lang check js-dotenv --against dotenv *.env # a twin, held to its compiled sibling row for row
```

`bindings/typescript/test/languages/dotenv.ts` in the repository is a complete twin of the compiled
`dotenv` format — parser and printer, held by the test suite to the compiled
parser's tables and to its edits — and is what a new language is best written
against. `handle(lang, requestLine)` is the wire itself, one JSON line to one,
if you want to carry it over something other than a pipe.

Two things stay closed. A runtime format never joins content sniffing — it is
selected by name or by extension, never guessed — and `Document.diagnose`
against a runtime target reports `UnsupportedOperation` in this release.

## Errors

Failures throw a `FigError` carrying a `status` (`Status` enum) and, for parse
failures, the core's diagnostic message and source location when available:

```ts
import { Document, Format, FigError, Status } from "@diaryx/fig";

try {
  Document.parse("{ not valid", Format.Json);
} catch (err) {
  if (err instanceof FigError && err.status === Status.ParseError) {
    console.error(err.message);          // "fig_parse: ..."
    console.error(err.line, err.column); // when the core reports them
  }
}
```

## Managing resources

`Document`, `Editor`, and `Embed` each own a native handle that must be released.
The best way is a `using` declaration, which disposes it at the end of the scope
even on a throw:

```ts
using ed = Editor.open(src, Format.Yaml);
// ...edit...
return ed.source();
// handle released here automatically
```

If you can't use `using`, call `.dispose()` yourself (it's idempotent), ideally in
a `finally`:

```ts
const doc = Document.parse(src, Format.Json);
try {
  return doc.get(["version"]);
} finally {
  doc.dispose();
}
```

As a backstop, each wrapper is also registered with a `FinalizationRegistry`, so a
handle you forget to dispose is still freed when the object is garbage-collected.
**Don't rely on this** — GC timing is unspecified, and holding many live handles
wastes memory. `using`/`dispose()` is the deterministic path.

The one-shot helpers — `parse`, `stringify`, `convert`, `serialize`, `diagnose` —
manage the handle for you, so no cleanup is needed.

## API reference

**Top-level functions**

- `init(): Promise<void>` — async-initialize the wasm module (browser main thread).
- `isReady(): boolean` — whether the module is loaded.
- `parse<T>(input, format): T` — parse to plain JS.
- `stringify(value, format, options?)` — serialize plain JS / `Value` to text.
- `serialize(value, format, options?)` — alias of `stringify`.
- `convert(input, from, to, options?)` — parse `from` and serialize to `to`.
- `fromJS(input)` / `toJS(value)` — bridge plain JS ↔ `Value`.
- `diagnose(value, format, options?)` — lossy-conversion warnings for a `Value`.
- `valueText(value, format, options?)` — serialized form for splicing into edits.
- `version()` / `versionString()` / `capabilities(format)` — introspection.
- `registerLanguage(lang)` — register a format written in JavaScript; returns
  its `Format`. `formatByName(name)` — the `Format` of a compiled or
  registered name, or `null`.
- `serve(lang, io?)` — run `lang` as a `fig` CLI helper over stdin/stdout.
  `handle(lang, line)` — the wire, one request line to one response line.
  `describe(lang)` — the wire's `description` of `lang`.
- `split(host, kind)` — read-only `[content, body]` of an embed.
- `detect(source)` — which `EmbedType` a host opens with, or `null`.

**Classes**

- `Document` — read path: `parse`, `get`, `has`, `nodeAt`, `toJS`, `toValue`,
  `serialize`, `diagnose`, plus low-level node accessors (`root`, `kind`,
  `firstChild`, `nextSibling`, `childCount`, `keyOf`, `valueOf`, `asBool`,
  `asString`, `asNumberRaw`, `asExtended`).
- `Editor` — comment-preserving editor: `open`, `source`, and the edit methods.
- `Embed` — frontmatter/embed editor: `open`, `openOrInit`, `extract`, `retype`,
  `render`, `replaceBody`, and the edit methods.

**Values & enums**

- `V` — `Value` constructors (`V.null()`, `V.int()`, `V.uint()`, `V.float()`,
  `V.string()`, `V.bool()`, `V.extended()`, `V.seq()`, `V.map()`).
- `Format`, `NodeKind`, `ExtKind`, `EmbedType`, `Status`, `WarningCode`,
  `WarningCause` — enums.
- `FigError` — the thrown error type. `LanguageError` — how a `Language`
  refuses its input, with a byte offset.
- Types: `Value`, `JsValue` (read side), `JsInput` (write side), `Segment`,
  `SerializeOptions`, `Warning`, `Region`, `Span`, `Version`, `Capabilities`.
- Runtime-language types, the wire's shapes field for field: `Language`,
  `Dialect`, `Syntax`, `Comments`, `CommentDelimiter`, `SectionHeader`,
  `ClosedContainers`, `NativeKinds`, `Renderer`, `Literal`, `RenderArgs`,
  `PrintOptions`, `NodeTable`, `NodeRow`, `RowKind`, `RowExtKind`, `RowSpan`,
  `RegionRow`, `MentionRow`, `CommentRow`, `HelperIo`.

## Developing the binding

Consumers need Node 20+; working on `bindings/typescript` from a checkout needs
**Node 24+**. The floor is higher because `npm test` runs the `.ts` test sources
directly through Node's type-stripping, which erases type annotations but cannot
downlevel the `using` declarations the tests use. On older Node the suite dies
with a `SyntaxError` before a single test runs.

```sh
cd bindings/typescript
npm ci
npm run build   # builds the wasm module, then compiles with tsc
npm test
```

This is a test-time requirement only. The published package stays at
`"engines": { "node": ">=20" }`, because `tsc` downlevels `using` in the shipped
`dist/` output. Don't raise `engines` to match the dev floor.

`zig build check` runs this suite as part of the pre-release gate and skips it
with a note — rather than failing — when Node is older than 24, when `npm` is
missing, or when `node_modules` hasn't been populated. CI pins Node 24, so there
it always runs for real.

## See also

- [The Zig CLI / library](/README.md) — install via Homebrew or a release binary.
- [fig CLI via npm/npx](npm-wasi.md) (experimental) — the
  `@diaryx/fig-wasi` *CLI* package (same actions as the native binary,
  running under Node's WASI support), not a JS library — if you want to
  `import { parse } from "@diaryx/fig"` in your own code, this package
  (the one this guide is about) is the one you want instead.
