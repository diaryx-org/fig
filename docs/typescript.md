```fig
title = Using fig in Typescript
author = adammharris
created = 2026-07-05T21:35:14-06:00
updated = 2026-09-27T12:00:00-06:00
part_of = [docs](docs.md)
```

# fig for TypeScript & JavaScript

`fig` parses, **edits**, and serializes configuration files — JSON, JSONC, JSON5,
YAML, TOML, INI, dotenv, Java `.properties`, NestedText, ZON, Apple property
lists and the native `figl` dialect — from one package, in which you import
only the formats you use (see [Formats](#formats)). Its
distinguishing feature is *comment-preserving editing*: you can change one value
deep in a YAML or TOML file and every comment, blank line, key order, and quoting
style elsewhere stays byte-for-byte identical. It also converts losslessly
between formats.

The core is a Zig library compiled to WebAssembly and embedded directly in the
package, so there is **no native build step and no separate `.wasm` file to
serve** — it runs in Node, Bun, Deno, and the browser. Each format is a
JavaScript module beside it that you import and register, so a page that
reads YAML carries YAML and no TOML.

- [Install](#install)
- [Loading the module](#loading-the-module)
- [Quick start](#quick-start)
- [Formats](#formats)
- [Reading data](#reading-data)
- [The value tree](#the-value-tree)
- [Editing without reserializing](#editing-without-reserializing)
- [Config inside a host file](#config-inside-a-host-file)
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

Requires Node 22+, or a current Bun, Deno or browser. The package is ESM-only
and ships its own TypeScript types. Working on the binding itself needs a newer
Node — see [Developing the binding](#developing-the-binding).

## Loading the module

The WebAssembly core initializes **lazily** — the first call that needs it
compiles it. Under Node, Bun, Deno, and Web Workers that "just works" and you can
call any API directly:

```ts
import { parse, registerLanguage } from "@diaryx/fig";
import json from "@diaryx/fig/languages/json";

const Json = registerLanguage(json);
parse('{"ok":true}', Json); // → { ok: true }
```

In the **browser main thread**, synchronous compilation of a module larger than
4 KB is disallowed by the platform, so call `init()` once at startup before any
other fig API, passing it the languages you will register — each is a module
of its own, and `init` compiles them too:

```ts
import { init, parse, registerLanguage } from "@diaryx/fig";
import json from "@diaryx/fig/languages/json";

await init(json);                   // do this once, e.g. during app bootstrap
const Json = registerLanguage(json);
parse('{"ok":true}', Json);         // now synchronous everywhere
```

`init()` is idempotent and safe to call anywhere; `isReady()` reports whether the
module is already loaded. If you forget it on a browser main thread, the first
call throws a clear error telling you to `await init()`.

## Quick start

```ts
import { parse, stringify, convert, registerLanguage } from "@diaryx/fig";
import json from "@diaryx/fig/languages/json";
import toml from "@diaryx/fig/languages/toml";
import yaml from "@diaryx/fig/languages/yaml";

// Bring in the formats you use, once, at startup. Each registration hands
// back the language's `Format`.
const Json = registerLanguage(json);
const Toml = registerLanguage(toml);
const Yaml = registerLanguage(yaml);

// Parse any format straight to plain JS values.
const cfg = parse('name = "fig"\nport = 8080\n', Toml);
// → { name: "fig", port: 8080 }

// Serialize plain JS values to any format.
stringify({ name: "fig", tags: ["a", "b"] }, Yaml);
// → "name: fig\ntags: [a, b]\n"

// Convert one format to another in a single call (comments preserved where the
// target allows).
convert("name: fig\nport: 8080\n", Yaml, Json);
// → '{\n  "name": "fig",\n  "port": 8080\n}\n'
```

The examples in the rest of this guide assume the formats they use are
registered this way, each held in a constant named after it (`Json`, `Toml`,
`Yaml`).

`parse` takes an optional type parameter to assert the shape you expect (no
runtime check is performed):

```ts
interface Config { name: string; port: number }
const cfg = parse<Config>('name = "fig"\nport = 8080\n', Toml);
cfg.port; // typed as number
```

## Formats

Every format is a language you import from `@diaryx/fig/languages/<name>` and
pass to `registerLanguage`, which returns its `Format`: an opaque handle that
`parse`, `Editor`, `convert` and the rest take. The language object holds it
from then on too (`yaml.format`), and every dialect is found by its name
(`formatByName("jsonc")`).
A format you don't import costs nothing.

Each is fig's own Zig parser and printer — the code the `fig` command line,
the Rust crates and the C library run — compiled to a WebAssembly module of
its own, 28 to 69 KB gzipped. A parse writes its node table in the
language's module and the table crosses into fig's as its rows, so a
document parses at native speed. The same formats written in JavaScript are
at `@diaryx/fig/js/<name>` ([Runtime languages](#runtime-languages)).

| Dialect      | Import                          | Notes |
| ------------ | ------------------------------- | ----- |
| `json`       | `@diaryx/fig/languages/json`    | Strict JSON (no comments). |
| `jsonc`      | `@diaryx/fig/languages/json`    | JSON with `//` and `/* */` comments; a dialect JSON's module serves. |
| `json5`      | `@diaryx/fig/languages/json`    | Unquoted keys, trailing commas, etc.; `formatByName("json5")`. `@diaryx/fig/languages/json5` is the same module. |
| `yaml`       | `@diaryx/fig/languages/yaml`    | YAML 1.2.2; its `yaml-1.1` dialect resolves scalars as 1.1. |
| `toml`       | `@diaryx/fig/languages/toml`    | TOML 1.0 / 1.1, incl. datetimes. |
| `figl`       | `@diaryx/fig/languages/figl`    | The native `figl` authoring dialect. |
| `ini`        | `@diaryx/fig/languages/ini`     | `[section]` + `key = value`. Untyped scalars — `port = 8080` reads back as the string `"8080"`. |
| `dotenv`     | `@diaryx/fig/languages/dotenv`  | Flat `KEY=value`: no nesting, untyped scalars. |
| `properties` | `@diaryx/fig/languages/properties` | Java `.properties`; same flat, untyped limits as dotenv. |
| `nestedtext` | `@diaryx/fig/languages/nestedtext` | [NestedText](https://nestedtext.org) — nested (dict/list) but deliberately untyped. |
| `zon`        | `@diaryx/fig/languages/zon`     | Zig Object Notation. |
| `plist`      | `@diaryx/fig/languages/plist`   | Apple XML property list; typed and nested. |

A format is a handle assigned per process, in registration order: persist
its **name**, never the number, and resolve it with `formatByName(name)`
(`null` when no registered language has the name) or `requireFormat(name)`,
which throws instead. `formatName(format)` goes the other way.

Every language reads, edits and serializes. Each is held by the package's
tests to fig's Zig language of the same name: the same tree for every
fixture, and the same bytes printed.

Register at startup, before the first call that uses the format. Registering
the same language again returns the format it already has, so a library and
the application using it may each register what they import. Looking up a
format nobody registered with `requireFormat` throws a `FigError` naming the
module to import:

```ts
requireFormat("toml");
// FigError: requireFormat("toml"): no language is registered for the format
// "toml" — import toml from "@diaryx/fig/languages/toml" and pass it to
// registerLanguage()
```

`capabilities(format)` reports what a format can do — the `caps` its
language declared:

```ts
import { capabilities, registerLanguage } from "@diaryx/fig";
import yaml from "@diaryx/fig/languages/yaml";

capabilities(registerLanguage(yaml)); // → { read: true, edit: true, serialize: true, references: true }
```

The four untyped formats — INI, dotenv, `.properties`, NestedText — parse
every scalar as a string; nothing infers a number or a boolean from them. The
first three are also shape-limited (INI holds one level of sections; dotenv
and `.properties` are flat), so serializing a nested value to one of them drops
what it cannot hold — run `diagnose` first to see exactly what.

## Reading data

For most cases, `parse` is all you need. When you want one value out of a large
document without materializing the whole thing, open a `Document` and use `get`:

```ts
import { Document } from "@diaryx/fig";

using doc = Document.parse(
  "[server]\nhost = \"localhost\"\nports = [80, 443]\n",
  Toml,
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

`get`, `has`, `toJS`, `toValue` and `parse` read through YAML's references: an
alias (`*x`) reads as the value of the node its anchor names, and a `<<` merge
key is flattened into its mapping, the mapping's own keys winning. An alias
inside the node it names has no value, and reading it throws a `ParseError`
located at the alias. The node API and `nodeAt` walk the document as written,
where an alias is a node of kind `NodeKind.Alias` and `<<` is a key.

## The value tree

`toJS()`/`parse()` give you plain JavaScript. Because JS can't represent every
config value faithfully, note:

- **Integers** that fit a safe JS number come back as `number`; larger ones come
  back as `bigint`. `fromJS`/`stringify` accept both.
- **Maps** with all-string keys become plain objects — *unless* a key is an
  "array index" string (e.g. `"0"`, `"10"`), in which case you get a `Map`
  instead, because JS objects would silently reorder those keys. Non-string keys
  always yield a `Map`.
- **Format-specific scalars** — TOML datetimes, ZON enum and char literals,
  JSON5's non-finite numbers (`Infinity`, `-Infinity`, `NaN`), and plist
  `<date>` and `<data>` — come back from `toJS` as their source text (a
  plist `<data>` as its base64 with whitespace stripped). The `Value` tree
  keeps their kind as well (`{ kind: "extended", ext: ExtKind.…, text }`),
  which is what lets them round-trip.

When you need full fidelity — distinguishing `int` from `uint`, ordered non-string
keys, or building datetimes — use the `Value` tree and its `V` constructors:

```ts
import { V, stringify } from "@diaryx/fig";

const value = V.map([
  [V.string("name"), V.string("fig")],
  [V.string("nums"), V.seq([V.int(1), V.int(2)])],
]);

stringify(value, Json); // '{\n  "name": "fig",\n  "nums": [\n    1,\n    2\n  ]\n}\n'
```

`fromJS(jsValue)` lifts plain JS into a `Value`; `toJS(value)` lowers it back.

## Editing without reserializing

This is what sets `fig` apart. `Editor` splices only the bytes of the node you
touch — everything else in the file is preserved exactly.

```ts
import { Editor } from "@diaryx/fig";

using ed = Editor.open(
  "# app config\nhost = \"localhost\"  # dev box\nport = 8080\n",
  Toml,
);

ed.replaceValue(["port"], 9090);
ed.setValue(["debug"], true); // replace if present, else insert

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

Common operations:

```ts
ed.insertValue([], "key", value);      // add a mapping entry
ed.replaceValue(path, value);          // change a value
ed.renameKey(path, "newKey");         // rename a key (a name, spelled as the format spells a key)
ed.setValue(path, value);                   // upsert (replace or insert)
ed.deleteKey(path);                       // remove a mapping entry
ed.appendValue(["list"], value);       // push onto a sequence
ed.prependValue(["list"], value);
ed.insertItem(["list"], 2, value);     // insert so it is item 2, before the item there now
ed.deleteItem(["list"], 0);            // remove sequence item by index
ed.moveKey(["a"], ["b"]);              // reorder mapping entries
ed.reorderKeys([], ["title", "body"]); // named keys first, rest follow
ed.moveItem(["list"], 2, 0);           // reorder sequence items
ed.reorderItems(["list"], [2, 0]);     // bring these indices to the front
ed.setSequence(["tags"], ["c", "a"]);  // reconcile a list, keeping survivors' comments
```

`replaceValue`, `insertValue` and `setValue` each have a `*With` twin taking a
`SerializeOptions`, for when the spliced value's own rendering needs
controlling — `replaceValueWith`, `insertValueWith`, `setValueWith` — and so
do the sequence inserts, `appendValueWith`, `prependValueWith` and
`insertItemWith`.

`insertItem(path, index, value)` is the insert at an index: 0 is
`prependValue`, the sequence's length is `appendValue`, and an index past it
throws `NotFound` with nothing written. Between them, a flow item takes a
separator laid out as its sequence is (one per line when the sequence is),
and a block item goes on its own line above the item at `index` and that
item's comment block.

### Whole containers (`Editor` only)

The operations above address a container the same way they address a scalar: by
the one range of source it occupies. A TOML `[header]` table occupies no such
range — its body is the lines after the header, and an `[a.b]` header further
down the file extends it — and neither does an INI `[section]` or a `figl` block
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

| | TOML | INI | figl | others |
|---|---|---|---|---|
| `deleteContainer`, `moveContainer`, `reorderContainers` | ✓ | ✓ | ✓ | — |
| `insertContainer`, `renameContainer`, `appendContainerToSeq` | ✓ | — | — | — |

"Others" is not a gap: YAML, JSON and the rest nest a container in one
contiguous region, so `delete` and `replaceValue` already handle it — which is
why they succeed on a YAML block mapping where TOML's refuse.

`setSequence` has a narrower domain than the rest: it matches new items to old
ones by *value* so a kept-or-merely-reordered item keeps its comments, which
means each item has to parse as a standalone document. It therefore throws
`InvalidArgument` on TOML (whose scalars can't stand alone), on an
empty list on either side, and on any non-scalar item. Nothing is lost there —
a TOML inline array carries no per-element comments, so `replaceValue` on the
whole list is equivalent. It earns its keep on YAML and figl, where per-item
comments are real.

Comments are first-class:

```ts
ed.addLeadingComment(["port"], "the listening port"); // own-line comment above
ed.setTrailingComment(["port"], "default 8080");      // same-line comment
ed.getLeadingComment(["port"]);   // read it back ("" = bare marker, null = none)
ed.getTrailingComment(["port"]);  // same convention
ed.deleteTrailingComment(["port"]);
ed.deleteLeadingComment(["port"]); // drops the whole owned block
```

The comment marker (`#`, `//`, `;`) is chosen for the format; strict JSON has
no comments and throws `UnsupportedFormat` if you try. Not every format has
both halves either: INI and NestedText have real leading comments but no
trailing-comment syntax, so `setTrailingComment` throws there too — a `;`/`#`
after a value on those formats' value lines is literal text, not a comment.

A container has a third anchor — the **dangling** run at the end of its body,
after its last entry, which is where a commented-out *last* entry lives. It is
addressed by the container's own path (`[]` = the document root):

```ts
ed.addDanglingComment(["server"], "was: here"); // at the body's child depth
ed.getDanglingComment(["server"]);   // "" = bare marker, null = none
ed.deleteDanglingComment([]);       // the run at the end of the document
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
`appendValueRaw`, `prependValueRaw`, `insertItemRaw`, `setValueRaw` — that takes a
string instead of a `Value`.

## Config inside a host file

fig edits a config document, not the file it sits in. Frontmatter,
endmatter, and fenced or HTML blocks in Markdown are located by
[twig](https://github.com/diaryx-org/twig): open an `Editor` over the block's
contents and splice its `source()` back between the byte spans twig reported.

## Serialization options

`stringify`, `convert`, and `Document.serialize` all take an optional
`SerializeOptions`:

```ts
stringify(value, Json, { pretty: false });   // minified
stringify(value, Json, { indent: 4 });       // 4-space indent
stringify(value, Toml, { width: 40 });       // inline vs [section] budget
convert(src, Yaml, Json, { stripComments: true });
```

| Option          | Applies to           | Meaning                                            |
| --------------- | -------------------- | -------------------------------------------------- |
| `pretty`        | JSON, ZON, TOML      | Multi-line (default) vs. compact. For TOML it gates array wrapping. |
| `indent`        | JSON, TOML           | Spaces per level (default 2).                      |
| `width`         | TOML, YAML, figl     | Column budget for inline (flow) vs. expanded layout (default 80). |
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
import { Document, WarningCode } from "@diaryx/fig";

using doc = Document.parse("a: null\nb: 1 # keep\n", Yaml);

// TOML has no null, so `a` would be dropped (its comments would survive).
doc.diagnose(Toml); // → [{ code: ValueDropped, path: "a", ... }]

// Strict JSON has no comments, so the `# keep` comment on `b` would be dropped.
doc.diagnose(Json); // → [{ code: CommentDropped, path: "b", ... }]
```

To *preserve* those values instead, pass `{ lossless: true }` — unrepresentable
values are round-tripped through a `$fig` envelope, and `diagnose` then reports
nothing lost:

```ts
convert("a: null\nb: 1\n", Yaml, Toml, { lossless: true });
```

There's also a top-level `diagnose(value, format, options?)` for a built `Value`.

## Runtime languages

Every format is an object. `registerLanguage`
takes a `Language` — what the format declares, and the functions it is — and
hands back a `Format` that every call above accepts from then on, at the tier
its `caps` declare: `Document.parse`, `Editor.open`, `convert`, `capabilities`,
all of them, with the format's own parser reading and its own printer writing,
and fig's splice engine editing between them.

```ts
import { registerLanguage, parse, convert, Editor, type Language, type NodeRow } from "@diaryx/fig";

const tinykv: Language = {
  name: "tinykv",
  caps: { read: true, edit: true, serialize: true },
  max_mapping_depth: 0,                     // flat: no mapping inside the root
  syntax: {
    comments: { line: { open: "#" }, trailing: { open: "#" } },
    kv_sep: "=",
    empty_map_literal: "{}",
    flow_containers: false,
  },
  dialects: [{ name: "tinykv", extensions: ["tkv"], splice: "raw", empty_doc_seed: "" }],
  samples: ["a=1\n# two\nb=two words # trailing\n"],

  parse(dialect, input) {
    // one row per node, in pre-order; row index is node id; spans are
    // BYTE offsets into the input, [start, end)
    const rows: NodeRow[] = [{ kind: "mapping", parent: null, span: [0, input.length] }];
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
convert("a=1\n", fmt, Json);         // → '{\n  "a": "1"\n}\n'
using ed = Editor.open("a=1\n", fmt);
ed.setValue(["b"], "2");                          // the splice engine writes `b=2` from `syntax`
ed.source();                                 // → "a=1\nb=2\n"
```

A dialect may also say what its printer makes of another format's tree, in
`prints`, and `Document.diagnose` and `convert`'s warnings then answer from
it: `holds` the kinds it writes as
themselves, `typed_scalars: false` where every scalar becomes text,
`char_as` and `number_special_as` what those two become when not held, and
`comments` whether it writes comments `"always"`, only when `"pretty"`, or
`"never"`. Without it the warnings come from `max_mapping_depth` and
`lossless` alone.

The object is fig's **helper wire**, as a value: the same `description` a
helper process answers `describe` with, the same node table its `parse`
answers, spelled with the wire's own field names — `max_mapping_depth`,
`empty_doc_seed`, `ext_kind` — rather than camelCase, on purpose. That is
what makes it one contract across hosts: a `.lua` script for `fig-lua`, a
Rust `Language`, and this object all declare the same fields, and what
`fig lang table <file>` prints for any file the CLI can read is exactly what
your `parse` must return for it. The shapes are specified once, in
[the helper protocol](helper-protocol.md);
the TypeScript types `Language`, `NodeTable`, `NodeRow`, `Syntax` and the
rest mirror them field for field, and `docs/proposals/runtime-languages.md`
in the core is the design.

**What registration checks.** The description is validated by the rules
every language is held to, and every sample is parsed, printed, reparsed and
edited before anything is registered — so a language whose `syntax` cannot
splice its own samples, or whose `print` does not round-trip its `parse`, is
refused with the reason as a `FigError`, and nothing is registered. A name a
language registered earlier already has is refused the same way, unless it is
the same object again, which returns the format it has. A name is free until
a language takes it: to set your own YAML beside the package's, name it
apart (`js-yaml`). Refuse
input from `parse` by throwing a
`LanguageError` with a message and byte offset; anything else you throw is
reported as a parse error without one. A `print` that refuses a table can
say why with a third argument, one of `Refusal`'s codes
(`new LanguageError("JSON has no null key", undefined, Refusal.NonStringKey)`),
and fig reports the failure that names. A registered language lives for the
rest of the process, and its `Format` is assigned per process — persist
the **name** and resolve it with `formatByName(name)`.

**Editing needs no code.** fig's splice engine writes an edit from `syntax`
alone — what a comment looks like, what separates a key from its value, how
containers open and close — and reparses through your `parse` to find its
spans. A format whose fragments cannot be spelled from constants (a typed
element, an entry whose key wraps its value) declares `renderers` and answers
`render(which, args)` for `"value"`, `"entry"`, `"item"`, `"tail"` or `"key"`;
the value renderer is told what fig's own literal rules made of the text
(`args.literal`: `"int"`, `"bool"`, `"string"`, …), so every format means the
same thing by `42` and a renderer spells a kind rather than deciding one.

A sequence whose items are joined by something other than one per line or
the dialect's `flow_entry_sep` says so on its row: `item_sep` is the bytes
written between two of its items on one line, and `open: true` says it has
no delimiters. A `.tune` voice's bars are
`{ kind: "sequence", item_sep: " | ", open: true }`, a bar's tokens
`item_sep: " "`, and blocks a blank line apart `item_sep: "\n\n"`. Such a sequence takes every item edit — append,
prepend, `insertItem`, delete and move — whatever its first byte: a delete
takes one item and the separator beside it, and an append splices after the
last item, so a trailing separator (`1 | 2 |`) stays trailing. The kit's
`sequence(span)` takes the same two fields.

**Every format in [Formats](#formats) is also written this way**, at
`@diaryx/fig/js/<name>`: a `Language` named after the format it serves,
held to the compiled format's node table row for row, as worked examples of
a language in JavaScript and as a second implementation of each. `canonical`,
fig's canonical form, is one more. Register one in place of
`@diaryx/fig/languages/<name>`, not beside it: both answer to the format's
name.

The languages are written with `@diaryx/fig/kit`, which the package ships
for your own formats too:

- `@diaryx/fig/kit`: the node table, a tree that lays itself out as one, and
  a byte-offset scanner;
- `@diaryx/fig/kit/grammar`: combinators, so a format is a description and
  its spans, comment binding and duplicate-key policy follow from it;
- `@diaryx/fig/kit/xml`: the XML tokenizer;
- `@diaryx/fig/kit/number` and `@diaryx/fig/kit/datetime`: how a number is
  spelled for a format, and which datetime a bare token is.

All of it is plain JavaScript that runs unchanged in Node, the browser, and
the QuickJS that `fig-quickjs` embeds. `languages/json.js` in the package is
the one to read first.

**The same object is a CLI helper.** `serve(lang)` runs the wire over a
process's stdin and stdout, which is what the `fig` command line speaks to a
helper it spawns — so the language you wrote for the browser is a format the
CLI reads, converts and edits, by name or by extension. Import it from
`@diaryx/fig/helper`, which is the wire and `serve` alone: a helper process
never needs the wasm module, and this entry does not load it.

```js
// ~/.config/fig/languages/tinykv.mjs
import { serve } from "@diaryx/fig/helper";
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
fig lang check js-dotenv --against dotenv *.env # a twin, held to its Zig sibling row for row
```

`bindings/typescript/test/languages/dotenv.ts` in the repository is a complete twin of the Zig
`dotenv` language — parser and printer, held by the test suite to the Zig
parser's tables and to its edits — and is what a new language is best written
against. `handle(lang, requestLine)` is the wire itself, one JSON line to one,
if you want to carry it over something other than a pipe.

Nothing guesses a format from content unless asked: `detectFormat(input,
order)` tries the dialects `order` names, in that order, and returns the
first whose parser accepts `input`, or `null`. `Document.diagnose` against a
format whose dialect declares no `prints` reports what its registration
declares it cannot hold: a kind missing from `lossless`,
what `max_mapping_depth` forbids, and a comment its `syntax` has no delimiter
for.

## Errors

Failures throw a `FigError` carrying a `status` (`Status` enum), the `op` that
failed, and, for parse failures, the core's diagnostic message and source
location when available:

```ts
import { Document, FigError, Status } from "@diaryx/fig";

try {
  Document.parse("{ not valid", Json);
} catch (err) {
  if (err instanceof FigError && err.status === Status.ParseError) {
    console.error(err.op);               // "fig_parse"
    console.error(err.message);          // "fig_parse: ..."
    console.error(err.byteOffset);       // when the core reports one
    console.error(err.line, err.column); // likewise
  }
}
```

`Editor.open` reports a parse failure the same way `Document.parse` does,
with the core's message. A location the core does not report is
`undefined`. A language's refusal carries its byte offset — except an offset
of 0, which the C ABI cannot tell apart from "none".

Calling a method on a `Document` or `Editor` after `dispose()` throws
a `FigError` with `Status.InvalidArgument`.

## Managing resources

`Document` and `Editor` each own a native handle that must be released.
The best way is a `using` declaration, which disposes it at the end of the scope
even on a throw:

```ts
using ed = Editor.open(src, Yaml);
// ...edit...
return ed.source();
// handle released here automatically
```

If you can't use `using`, call `.dispose()` yourself (it's idempotent), ideally in
a `finally`:

```ts
const doc = Document.parse(src, Json);
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

The one-shot helpers — `parse`, `stringify`, `convert`, `diagnose` —
manage the handle for you, so no cleanup is needed.

## API reference

**Top-level functions**

- `init(): Promise<void>` — async-initialize the wasm module (browser main thread).
- `isReady(): boolean` — whether the module is loaded.
- `parse<T>(input, format): T` — parse to plain JS.
- `stringify(value, format, options?)` — serialize plain JS / `Value` to text.
- `convert(input, from, to, options?)` — parse `from` and serialize to `to`.
- `fromJS(input)` / `toJS(value)` — bridge plain JS ↔ `Value`.
- `diagnose(value, format, options?)` — lossy-conversion warnings for a `Value`.
- `version()` / `versionString()` / `capabilities(format)` — introspection.
- `registerLanguage(lang)` — register a format written in JavaScript, one of
  `@diaryx/fig/languages/*` or your own; returns its `Format` (also
  `lang.format` from then on), and the same `Format` again for the same
  object. `formatByName(name)` — the `Format` of a registered dialect name,
  or `null`; `requireFormat(name)` — the same, throwing a `FigError` that
  names the module to import; `formatName(format)` — a format's dialect
  name, or `null`.
- `serve(lang, io?)` — run `lang` as a `fig` CLI helper over stdin/stdout.
  `handle(lang, line)` — the wire, one request line to one response line.
  `describe(lang)` — the wire's `description` of `lang`. All three, the
  `Language` types and `LanguageError` are also `@diaryx/fig/helper`, which
  loads no wasm.

**Classes**

- `Document` — read path: `parse`, `get`, `has`, `nodeAt`, `toJS`, `toValue`,
  `serialize`, `diagnose`, plus low-level node accessors (`root`, `kind`,
  `firstChild`, `nextSibling`, `childCount`, `keyOf`, `valueOf`, `asBool`,
  `asString`, `asNumberRaw`, `asExtended`).
- `Editor` — comment-preserving editor: `open`, `source`, and the edit methods.

**Values & enums**

- `V` — `Value` constructors (`V.null()`, `V.int()`, `V.uint()`, `V.float()`,
  `V.string()`, `V.bool()`, `V.extended()`, `V.seq()`, `V.map()`).
- `Format` — a type only: the opaque handle of a registered dialect, from
  `registerLanguage`, `formatByName` or a language's `format`.
- `NodeKind`, `ExtKind`, `Status`, `WarningCode`,
  `WarningCause` — enums, each an `as const` object with a type of the same
  name (`NodeKind.Mapping` is a value, `NodeKind` a type), not a TypeScript
  `enum`. A number the core reports that has no name here — a node kind or
  warning code from a newer core — passes through unchanged and compares
  numerically, so each type is its named members or any number; there is no
  reverse lookup from a value to its name.
- `FigError` — the thrown error type. `LanguageError` — how a `Language`
  refuses its input, with a byte offset.
- Types: `Value`, `JsValue` (read side), `JsInput` (write side), `Segment`,
  `SerializeOptions`, `Warning`, `ParseDetail`, `Version`,
  `Capabilities`.
- Runtime-language types, the wire's shapes field for field: `Language`,
  `Dialect`, `Syntax`, `Comments`, `CommentDelimiter`, `SectionHeader`,
  `ClosedContainers`, `NativeKinds`, `Renderer`, `Literal`, `RenderArgs`,
  `PrintOptions`, `NodeTable`, `NodeRow`, `RowKind`, `RowExtKind`, `RowSpan`,
  `RegionRow`, `MentionRow`, `CommentRow`, `DirectiveRow`, `HelperIo`.

## Developing the binding

Consumers need Node 22+; working on `bindings/typescript` from a checkout needs
**Node 24+**. `npm test` type-checks the sources and tests with `tsc`, then runs
the `.ts` files directly under Node's strip-only TypeScript support. That works
because the package is written in erasable syntax only — its enums are
`as const` objects, and `erasableSyntaxOnly` in `tsconfig.json` refuses a
TypeScript `enum`, a `namespace` or a parameter property — but stripping
leaves the tests' `using` declarations as they are, and Node runs `using`
natively only from 24.

```sh
cd bindings/typescript
npm ci
npm run build   # builds the wasm module, then compiles with tsc
npm test
```

The module the package ships, and the one `npm run build` builds, has no
language in it. The tests register the package's languages, and
`test/twins.test.ts` holds them to fig's Zig languages through what those
recorded for every fixture: the tables `fig lang table` prints, and the
`.printed` and `.converted` files `scripts/record-compiled.mjs` writes from a
module with every Zig language built in (`FIG_WASM_LANGUAGES=all npm run
build:wasm`). Rerun it when a Zig printer changes.

This is a test-time requirement only. The published package stays at
`"engines": { "node": ">=22" }`, because `tsc` downlevels `using` in the shipped
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
