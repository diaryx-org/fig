```fig
title = JSON-RPC wire
description = fig and twig speak one helper protocol, JSON-RPC 2.0 over newline-delimited stdio, on a fork of zigjr, so neither keeps a wire of its own and a helper needs no fig or twig library to speak it
author = adammharris
created = 2026-10-12
updated = 2026-10-12
status = accepted
part_of = [Proposals](/docs/proposals/proposals.md)
```

# JSON-RPC wire

> **Status: ACCEPTED; fig's half is built.** Written against `main` at
> 4f348cb (6.0 in progress, ABI 4) and twig 4.1.0. fig speaks the protocol
> [docs/helper-protocol.md](/docs/helper-protocol.md) specifies, on
> zigjr; where what was built differs from what is argued below, the
> protocol page is right and the difference is noted here. Left: §6 steps
> 4 and 5 (fig-quickjs, fig-lua, twig), and the Rust helper's JSON (§4).

## 1. Two wires, one shape

A runtime language crosses a process boundary on the helper wire:
newline-delimited JSON, `describe` once, then one request line to one
response line. fig has one (`src/languages/wire.zig`); twig has another
(`twig/src/wire.zig`), whose header calls it "fig's wire, with twig's node
table in it". They already differ where nothing requires it:

| | fig | twig |
| --- | --- | --- |
| request | `{"op":"parse","dialect":…,"input":…}` | the same, plus `features` |
| render | `"which":"item"`, fig's `RenderArgs` | `"which":"render_text"`, `render_block`, `spells_autolink` |
| failure | `{"ok":false,"message","byte_offset"}` | `{"ok":false,"message"}` |
| transport | `callFn` → a parsed `std.json.Value` | `exchange` → the raw line |
| answering end | `fig::helper` (Rust), `@diaryx/fig/helper` | `wire.Server` (Zig), `twig::helper` (Rust) |

The parts that are the same are the parts that are not about config or
documents: framing, the success/failure envelope, matching an answer to
its question, and the loop that serves one. Each project carries them, and
each helper SDK (Rust, TypeScript, the fig-quickjs and twig-quickjs
runners) carries them again, because the protocol is ours and no library
outside speaks it.

## 2. JSON-RPC 2.0 is that part

[JSON-RPC 2.0](https://www.jsonrpc.org/specification) is the envelope the
two wires reinvented, and the one LSP and MCP use over stdio:

```json
{"jsonrpc":"2.0","id":3,"method":"parse","params":{"dialect":"hcl","input":"…"}}
{"jsonrpc":"2.0","id":3,"result":{"table":{…}}}
{"jsonrpc":"2.0","id":3,"error":{"code":1,"message":"…","data":{"byte_offset":12}}}
```

The proposal: both wires become JSON-RPC 2.0, one message per line (no
`Content-Length` headers — every message is one JSON value, and a JSON
string carries no raw newline). What each project keeps is its methods and
their payloads: the node table, the description, the render arguments.
What it gives up is everything around them.

### 2.1 The mapping

| now | JSON-RPC |
| --- | --- |
| `"op":"parse"` | `"method":"parse"` |
| every other request field | a member of `params` (always an object) |
| `{"ok":true,…}` | `result`: the value itself — the description, the table, the text — not an object around it |
| `{"ok":false,"message":m}` | `error: {code, message: m}` |
| `byte_offset` on a failure | `error.data.byte_offset` |
| — | `id`: an integer the caller counts up per transport |

twig's `render` with a `which` becomes three methods, `render_text`,
`render_block` and `spells_autolink`; fig's `render` keeps `which`, since its
renderers are a list the language declares (`renderers` in a description),
not a fixed set.

### 2.2 Error codes

The spec reserves −32768…−32000. A helper answers −32700 for a line that is
not JSON, −32600 for a message that is not a request, −32601 for a method it
does not have and −32602 for params it cannot read; fig reports all four as
the helper not handling its request. A language's own refusal is −32000, the
first of the codes JSON-RPC leaves to the server; a printer's refusal with a
reason is that reason's `abi.Refusal` value, 1–9 (the codes `FigError.code`
already carries). `0` is never an error code, so an error object is never
mistaken for its absence. twig's status codes go the same way.

### 2.3 Ids and order

The caller sends one request and waits for its answer, as now; there is no
pipelining and no batch. The id is there so a caller can tell an answer to
this request from a stray line, and refuse a helper that has fallen out of
step, rather than read the previous call's table as this one's. A helper
echoes it and does nothing else with it.

### 2.4 Telling an old helper from a new one

A 5.x helper reads `{"jsonrpc":"2.0",…}`, finds no `op`, and answers
`{"ok":false,…}`. A response without `"jsonrpc":"2.0"` is therefore
recognisably the old wire, and fig says so by name ("this helper speaks
fig 5's wire; rebuild it against fig 6") instead of a parse failure. No
version field is needed beyond the one JSON-RPC already sends.

## 3. Zig: a fork of zigjr

[zigjr](https://github.com/williamw520/zigjr) (MIT) parses and composes
requests, responses, notifications and errors, streams them by delimiter
or by `Content-Length`, and dispatches a request to a plain Zig function.
Upstream targets Zig 0.15.1; a port to 0.16.1 has sat as an open pull
request since May 2026. The fork,
[adammharris/zigjr](https://github.com/adammharris/zigjr), is that pull
request plus a 0.17.0 port (`eadd3ae`); all 127 of its tests pass. fig and
twig depend on it by `build.zig.zon` URL and hash, pinned to a commit.

Each project uses it on both ends:

- **Calling end.** `wire.zig` composes a request with zigjr and reads the
  response through it; the `Transport` shrinks to "trade one line for one
  line" — twig's `exchange` shape, which both adopt. The two transports
  that exist stay where they are: the CLI's child process
  (`src/cli/languages.zig`) and the wasm module's host call
  (`src/wasm_host.zig`), neither of which is zigjr's business.
- **Answering end.** A Zig helper (twig's `wire.Server`, and a Zig
  language served out of process) is a zigjr dispatcher over the
  language's functions.

The node-table codec (`tableToJson`, `tableFromJson`) and the description
reader stay in each project. They are the payload, and they differ.

### 3.1 What it costs

fig's wire reads a response with its own walk over `std.json.Scanner`
(`parseLine`) because `std.json.parseFromSlice`'s number path links the
float parser into every host, and the wire carries no floats. zigjr parses
a message into `std.json.Value` with `parseFromSlice`, which links it back.
That matters for the wasm module and not for the CLI. Step 1 measured it.

**Measured (2026-10-12).** In the language-free `fig.wasm` (704,735 bytes
stripped), reading a response through upstream zigjr added 25 KB of code;
through `std.json.parseFromSlice` into a `Value`, 2 KB. A first stripped
build looked 134 KB larger, but that was debug sections: zigjr's module set
`strip = false` and the host's target, so it overrode the importing wasm
build. The fork now leaves those to its importer, and reads every message
with a token walk that keeps integers as integers and other numbers as
text (`jsonrpc/wire_value.zig`). Reading a response through the fork costs
5.3 KB in all.

zigjr's own line count (about 8.7k, much of it the dispatcher's reflection)
is not a cost: Zig compiles what is referenced, and the calling end
references the composer and the response reader.

## 4. Rust and TypeScript need no wire library

The helper SDKs exist because the protocol is ours. Under JSON-RPC they keep
only what is fig's or twig's:

- **TypeScript.** `@diaryx/fig/helper`'s `serve` reads a line, `JSON.parse`s
  it and dispatches on `method`; the envelope is a dozen lines and needs no
  package. A JS helper may as well use any JSON-RPC library it likes.
- **Rust.** `fig::helper` reads the wire "with fig itself" so that the crate
  adds no JSON dependency. In 6.0 that means a helper silently needs the
  `json` language built in, which a build may leave out. The helper module
  moves to `serde_json`, behind the `helper` feature, so the crate's default
  graph is unchanged. twig's `twig::helper` does the same. *Not done in
  the first pass:* the module's public functions (`table_to_value`,
  `description_to_value`, `handle`) trade `fig::Value`, which fig-lua and
  fig-quickjs build on, so the move is its own change
  ([task](/docs/tasks/helper-serde-json.md)). fig's half has since landed:
  the functions trade `serde_json::Value`; the runners follow with their
  move to 6.0.
- **Anything else.** A helper in Python, Go or Lua implements `describe`,
  `parse`, `print` and `render` against whatever JSON-RPC library its
  language has. That is the point of the change.

So there is no third package of ours. What fig and twig share is zigjr and a
specification; what each ships is its methods.

## 5. What breaks

Every helper, in the same release that already breaks them (6.0's contract
changes and the `figl` rename):

- `fig::helper`, `@diaryx/fig/helper`, the CLI's runner and the wasm host's
  JavaScript half;
- fig-quickjs, which writes and reads the wire itself; fig-lua, which
  serves through `fig::helper` and needs only a rebuild; and
  twig-quickjs's `js/wire.js`;
- `tools/cli-lang-check.sh` and the `tinykv_helper` test helper.

`fig lang table` and `fig lang check` are unaffected: they print and compare
the node table, which keeps its shape.

## 6. Order

1. Add zigjr to fig; measure the wasm module with a response read through
   it against `parseLine`. Decide §3.1.
2. fig's calling end on JSON-RPC (`wire.zig`, the CLI runner, the wasm
   host), `Transport` as `exchange`; the old-wire detection of §2.4.
3. fig's answering ends: `fig::helper` on `serde_json`, `@diaryx/fig/helper`,
   `tinykv_helper`, the checks. The spec of the wire moves from
   `helper.rs`'s doc comment to its own page (`docs/helper-protocol.md`;
   `docs/spec.md` is figl's), where a helper author in another language
   finds it.
4. fig-quickjs and fig-lua.
5. twig, the same three steps, with its own methods (§2.1).

## 7. Open questions

- **twig depending on fig.** Not needed for this; zigjr is the shared part.
  twig's `parse_diagnostic.zig` stays a copy of fig's until twig depends on
  fig for another reason (its languages file as `languages.figl`).
- **Notifications.** None is proposed. A helper's log line is stderr, as
  now; if a helper ever needs to report progress, a notification is the
  form it would take.
- **The fork's future.** Upstream has announced a breaking 2.0. Keeping the
  fork's changes to porting (and §3.1, if needed) keeps a later merge
  cheap; nothing here depends on 2.0.
