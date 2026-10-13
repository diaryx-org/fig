```fig
title = Helper protocol
description = What a fig helper speaks: JSON-RPC 2.0, one message per line, with the methods describe, parse, print, render and split_stream, and the description and node table they carry
author = adammharris
created = 2026-10-12
updated = 2026-10-12
part_of = [fig docs](/docs/docs.md)
```

# Helper protocol

A language fig does not bundle reaches it as a **helper**: an executable a
`languages.figl` names, which the CLI starts and talks to over its stdin
and stdout. The TypeScript binding's JavaScript languages speak the same
protocol to the wasm module, without the process. This page is the
protocol; a helper written in any language implements it with whatever
JSON-RPC library that language has. `fig::helper::serve` (Rust) and
`serve` from `@diaryx/fig/helper` (TypeScript) are it, written once.

## Framing

[JSON-RPC 2.0](https://www.jsonrpc.org/specification), one message per
line: fig writes a request and a newline to the helper's stdin, and the
helper writes one response and a newline to its stdout. A message never
spans lines; a JSON string carries no raw newline. Every string is UTF-8,
and a document crosses as a JSON string, so a helper never sees bytes that
are not text. A helper writes anything else — a log, a warning — to stderr,
which fig passes through.

fig sends one request and waits for its answer. Each request's `id` is an
integer, counting from 1 for each helper; the response carries the same
`id`. There are no notifications and no batches. `params` is always an
object, and absent when a method takes none.

```json
{"jsonrpc":"2.0","id":2,"method":"parse","params":{"dialect":"hcl","input":"a = 1\n"}}
{"jsonrpc":"2.0","id":2,"result":{"rows":[…],"regions":[],"mentions":[],"comments":[]}}
{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"expected `=`","data":{"byte_offset":2}}}
```

## Methods

| method | params | result |
| --- | --- | --- |
| `describe` | none | a [description](#the-description) |
| `parse` | `dialect`, `input` | a [node table](#the-node-table) with spans |
| `print` | `dialect`, `table`, `options` | the text, a string |
| `render` | `which`, `dialect`, the render arguments | the fragment, a string |
| `split_stream` | `dialect`, `input` | `[{"start","end","explicit"}]` |

`dialect` is the row the call is through: the language's name, or one of
its dialects'.

**`print`**'s `table` is a node table without spans, and `options` is
`{"pretty","strip_comments","indent","width","splice","flow"}`: booleans,
and integers for `indent` and `width`. A field that is absent takes its
default (`true`, `false`, `2`, `80`, `false`, `false`).

**`render`** asks for one of the six fragments the editor writes, named by
`which`: `value`, `entry`, `item`, `tail`, `key` or `key_name`. fig asks
only for those the description's `renderers` lists. The other params are
strings: `indent`, `key`, `value`, `old_key`, and `parent_key` and
`parent_tag`, the container the fragment goes into. `literal` comes with
`which: "value"` alone and says what fig made of the value — `null`,
`bool`, `int`, `float`, `datetime` or `string`; absent means a string.

**`split_stream`** is asked only of a language whose description says
`split_stream: true`. Each document is a range of BYTES of the UTF-8
input, and `explicit` says it opened with a document-start marker. An
empty stream is one document.

## Errors

| code | meaning |
| --- | --- |
| -32700 | the line is not JSON |
| -32600 | the message is not a request |
| -32601 | the helper has no such method, or does not serve this one (`print` from a language that does not serialize) |
| -32602 | the params are not what the method takes |
| -32000 | the language refuses: the input does not parse, a fragment cannot be spelled |
| 1–9 | a printer refuses the table, for the reason the code names (below) |

fig reports a reserved code as the helper not understanding its request,
and every other with the helper's own `message`. A refusal of `parse` or
`split_stream` may carry `data: {"byte_offset": n}`, where in the input
it is. A printer that refuses a table says why with its code, and fig
reports the failure that names:

| code | the table holds |
| --- | --- |
| 1 | a null the format cannot write |
| 2 | a key that is not a string |
| 3 | a root the format cannot write |
| 4 | a value the format cannot write |
| 5 | a key the format cannot spell |
| 6 | NaN or an infinity |
| 7 | an alias the format cannot resolve |
| 8 | more nesting than the format allows |
| 9 | a kind of node the printer does not expect |

A response from a helper built against fig 5 — whose wire was
`{"op":…}` in and `{"ok":…}` out — has no `"jsonrpc":"2.0"`, and fig says
the helper needs rebuilding.

## The description

`describe`'s result is the language: its name, what it can do, and
everything the editor needs to know about its syntax.

```text
{"name","caps":{"read","edit","serialize","references"},
 "max_mapping_depth":n|null,"lossless":{…}|null,"syntax":{…}|null,
 "dialects":[{"name","extensions":[…],"splice":"literal|escaped|raw",
   "empty_doc_seed":"…"|null,"syntax":{…}|null,"prints":{…}|null}],
 "samples":[…],"renderers":["value",…],"split_stream":bool}
```

A `syntax` is the fields of the Rust crate's `language::Syntax` by name,
and a `prints` those of `language::Prints`, with enums as their lower-case
names and absent optionals as `null`. `samples` are documents fig parses,
prints, reparses and edits when the language registers; a language whose
samples fail is refused.

## The node table

`parse`'s result, and `print`'s `table`:

```text
{"rows":[…],"regions":[…],"mentions":[…],"comments":[…],"directives":[…]}
```

- A **row** is `{"kind":"mapping","parent":0,"span":[s,e],"text":"…"}`,
  plus `ext_kind`, `anchor`, `anchor_span`, `tag`, `tag_span`, `marker`,
  `sep`, `item_sep` and `open` where present. An absent optional is
  omitted, not `null`, and `open` appears only when true. `kind` is one of
  `null`, `bool`, `int`, `float`, `string`, `sequence`, `mapping`,
  `keyvalue` and `alias`.
- A **region** is `{"node","start","end"}`.
- A **mention** is `{"node","span":[s,e],"kind":"header|entry|index|append"}`;
  `index` and `append` are where a sequence item's path is written (figl's
  `1` in `xs[1]`, its `a.b[]` header or `+`), and any other kind reads as
  `header`.
- A **comment** is `{"node","slot":"leading|trailing|dangling","style":"line|block","text"}`.
- A **directive** is `{"handle","prefix"}`; the array is omitted when empty.

Spans and offsets are byte offsets into the input, and integers; the
protocol carries no floats. `fig lang table <file>` prints the table a
bundled format gives for a file, and `fig lang check` holds a helper to it.
