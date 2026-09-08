```fig
title = Runtime languages
description = A format fig did not compile in — the Language contract carried as a vtable in-process and as a helper protocol out-of-process, with fig-lua as the first implementor and nothing added to core's size
created = 2026-09-07
status = draft
updated = 2026-09-07
part_of = [proposals](proposals.md)
```

# Runtime languages

> **Status: DRAFT.** Written against `main` at 838424f, with core 3.0.0
> (ABI 2) built and unreleased. §9 names the one thing this proposal asks
> of 3.0 before it tags: a reserved range of format integers. Everything
> else here is additive to core and is a 3.x minor, plus one new repository.
>
> "Runtime" throughout means *resolved when the program runs*, as opposed to
> a format compiled into `src/languages/`. It does not name Lua, wasm, or
> any engine; §5 is about why it cannot.

## 1. The claim

A format should not have to live in fig's source tree to be a format. Today
every format is a directory under `src/languages/`, a row in `list.zig`, and
a slot in `language.zig`, and [pluggable formats](pluggable-formats.md) made
that as small as a compiled-in format can be: one directory, one row, one
line. That is the ceiling for a format fig ships. It is not the ceiling for
a format fig *uses*. Someone with an HCL file, a `.cfg` in a house dialect,
or a format that exists in one company should be able to teach fig to read,
write, and edit it without a Zig toolchain, a fork, or a release.

The reason this is now cheap is that the 2.x proposals turned the format
contract into data. The [Language interface](language-interface.md) made
every per-format fact a declaration that `validate` checks. [Derived
regions](derived-regions.md) made whole-container editing generic over a
table the parser fills. Pluggable formats made every enum a projection of one
registry and named the boundary a format writes against. What is left of "a
format" is a fixed set of declarations, a node table with spans, and a
`syntax` record. None of that is inherently comptime. It is a vtable that
happens to be spelled as declarations, and a vtable can be filled at runtime.

This proposal was argued before and lost, as a request to embed a scripting
engine in fig. It is argued again here as something narrower: **core defines
the contract's two runtime carriers and nothing else.** The engines live in
their own repositories and are peers of each other.

## 2. Where 3.0 stands

`Language.validate` (`src/languages/language.zig`) enforces a closed set:

- **Required** — `Type`, `Parser`, `Printer`, `default_type`, `parse`,
  `print`, `name`, `extensions`, `caps`, `dialects`.
- **Required of an editable format** — `syntax(t: Type) Syntax`.
- **Optional** — `printNode`, `samples`, `materialize`/`TagMode` (YAML),
  `parseAbstract` (deserializable dialects).
- **Twenty-two editing hooks** and **three section-only ops**, each
  dispatched by `@hasDecl` and each overriding a generic engine method.

A parse produces a `Document`: an AST, `node_spans` indexed by node id, the
optional anchor and tag span tables, and `node_regions`, the header lines of
every section node. The editor (`src/editor.zig`, `editor/splice.zig`,
`editor/regions.zig`) is generic over that plus `syntax`; a format that
declares no hooks gets every operation the splice engine can spell from
`syntax` alone, and refuses the rest in its own vocabulary.

The per-format harness (`src/languages/harness.zig`) states what the engine
relies on every format having and runs it over every compiled dialect: seeds
parse, samples round-trip, regions are well-formed, an `Editor` constructs
and a no-op splice is the identity.

The C ABI takes `int format` everywhere and resolves it against
`FigFormat`, whose integers are `abi_value` on each dialect row and are
permanent. `fig_format_capabilities` reports what a format can do. The
threading note in `fig.h` promises that fig keeps no shared mutable global
state.

So: the contract is closed and checked, the parse result is a flat table
the editor already consumes, the harness is the conformance suite, and the
ABI addresses formats by integer. Each of those is a precondition for what
follows, and each exists.

## 3. What is being asked for

Three things, in order of how much they cost core.

**3.1 A format resolved at runtime is a peer of a compiled one.** Every
entry point that takes a format today accepts a runtime one: the CLI
actions, `fig_parse`, `fig_document_serialize`, `fig_editor_*`, the Rust
`Document::parse`, the TypeScript `parse`. Nothing gains a second API for
"but loaded".

**3.2 The engine that produced the format is core's business only through
one contract.** Core never links Lua, a wasm runtime, or a dynamic loader.
What it links is a struct of function pointers and a protocol over pipes.

**3.3 The first engine is a separate repository, `fig-lua`.** It is the
canonical and recommended way to write a runtime format, because one script
serves the CLI, Rust, and the browser. It is not the only way, and core
does not know its name.

## 4. The contract

The contract is what a compiled `Language` already declares, restated as
values rather than declarations, and it has exactly one new part: the parse
result crosses a boundary, so its shape has to be written down.

### 4.1 The node table

A parse returns a flat table, one row per node in pre-order, plus a comment
table. This is the shape `Document` already holds; it is stated here because
it is now a contract rather than an implementation.

| Column | Meaning |
|---|---|
| `kind` | `FigNodeKind`: null, bool, int, float, string, sequence, mapping, keyvalue. `alias` is not accepted from a runtime format (§8.2). |
| `parent` | Row index of the parent, or none for the root. |
| `span` | `[start, end)` byte offsets into the input. Required of every node; the editor splices by it. |
| `text` | For scalars, the *decoded* value as bytes; for int and float, the lexeme, as `fig_node_number` already returns it. |
| `region` | For a section node only: one or more whole header lines, the rows `Document.node_regions` would hold. |

Comments are a side table keyed by row: leading (a run), trailing (at most
one), dangling (a run), each with `text` and a line/block style, matching
`AST.NodeComments`. A format with no comment syntax declares `comments =
null` in its `syntax` and returns none.

Key–value pairs are three rows, as they are in the AST: the `keyvalue`, its
key, its value. That is the one place the table is less obvious than a
tree, and it is chosen because it is what the editor, `fig_node_first_child`,
and every binding already walk.

A print takes the same table, produced by core from a document, and returns
bytes. A runtime format's printer sees the AST the same way a compiled one
does, through the tree, and nothing else about the engine is exposed to it.

### 4.2 The declarations

Everything else a `Language` declares becomes a field of one record:

- `name`, `extensions`, `caps` (`read`, `edit`, `serialize`,
  `max_mapping_depth`; `lossless` is always null for a runtime format, §8.3).
- `syntax`, exactly the `manifest.Syntax` fields: `comments`, `kv_sep`,
  `key_style`, `key_sigil`, `empty_map_literal`, `block_seq_editable`,
  `single_line_block_mapping`, `bare_document_mapping`, the flow-map
  brackets, `structural_indent`, `section_noun`.
- One dialect: `splice` (literal, json_string, or raw) and `empty_doc_seed`.
  A runtime language has one dialect in this proposal; §8.1 is the argument
  for leaving it there.
- `samples`, which are not optional here. §6 says why.

No hooks. A runtime format edits through the splice engine and `syntax`
alone, and refuses what `syntax` cannot spell. This is the largest
deliberate omission in the proposal and §8.4 argues it.

### 4.3 The two carriers

**In-process: a vtable.** A C struct with a `version` field first, the
declarations of §4.2 as plain fields, and four function pointers: `parse`
(input → node table), `print` (node table → bytes), `free` for what those
allocated, and `describe`, which returns the record so a host can register a
language it did not write. Registration is `fig_language_register(const
FigLanguageVTable *, int *out_format)`. The vtable's `version` is the fifth
versioned surface, and it is bumped on the same rule as `FIG_ABI_VERSION`:
only when a field changes meaning.

**Out-of-process: a helper protocol.** The same four calls as a request and
response over a child process's stdin and stdout. The request carries the
input bytes; the response carries the node table serialized. This is the
git remote helper model: the helper is any executable, in any language, that
speaks the protocol. A helper is spawned once per fig invocation and is
asked `describe` first.

The serialization is JSON, for one reason: every host that could write a
helper already has a JSON library, and fig itself parses it. It is not fig's
own format because a helper author should not need fig to write a helper.

**These are the same contract.** Core holds one implementation of the
helper protocol, and it is a vtable whose four functions talk to a process.
The CLI registers helpers through the same `fig_language_register` path the
libraries use. There is no third way in.

## 5. Why core links no engine

The previous argument for a Lua extension was lost on bundling: a scripting
VM in the core library and therefore in every binding, the wasm build, and
the static payloads. That objection was right and still is. The design here
survives it because the engines are not in core, and each host gets the
carrier it can actually run:

| Host | In-process carrier | Out-of-process carrier |
|---|---|---|
| CLI (`fig` binary) | none: no engine linked | helpers on PATH, configured (§7.1) |
| Rust (`fig` crate) | a `Language` trait; `fig-lua` implements it | the same helper runner, exposed as a function |
| TypeScript (`@diaryx/fig`) | a JS object with the four members, called through wasm imports | none: no process |
| Zig | the vtable directly | the helper runner |

A wasm runtime cannot run inside the wasm build. A dynamic loader cannot
run there either. A Lua VM could, but it would be a cost paid by every
consumer for a feature most never use. Putting the engines outside makes
every one of those a decision the consumer takes, and makes the set of
engines open: a helper written in Python is a peer of `fig-lua` on the day
it is written.

## 6. Validation is where the safety is

`validate` runs at compile time and the harness at test time. A runtime
format has neither moment, so it gets one: **load**. Registering a language
runs the same checks in the same order, and refuses on the first failure:

1. The record is well-formed: `name` is an identifier, `extensions` are
   non-empty and not owned by a compiled format, `caps.edit` implies a
   `syntax`, `syntax` is coherent (a `section_noun` implies regions;
   `comments = null` implies no comment ops).
2. Every `sample` parses, prints, and reparses to the same node table.
3. Every node table is well-formed: spans are within the input, nested
   correctly, pre-ordered; regions are whole lines on container nodes,
   sorted.
4. The `empty_doc_seed` parses.
5. If `caps.edit`, an `Editor` constructs over every sample and a no-op
   splice is the identity.

That list is the harness. It is why `samples` is required of a runtime
language when it is optional of a compiled one: a compiled format has its
own test suite beside it, and a runtime one has only what it declares.

A parse that fails after load is a diagnostic naming the language and, for
a helper, the command. A helper that exits or writes malformed output is
the same diagnostic. Neither is a crash of the host, and neither can be,
because core validates every node table it receives, not just the samples.

`fig lang check <helper-or-script>` runs this list from the CLI and prints
what failed. That command is the whole development loop for a format
author, and it is the same list the load path runs, so a format that passes
it loads.

## 7. The experience

### 7.1 CLI

Discovery is a config file, self-hosted, read only when an extension or a
`--input`/`--output` name is not built in:

```fig
[languages.hcl]
extensions = [hcl]
command = [fig-lua, ~/.config/fig/languages/hcl.lua]
```

Searched in `.fig/languages.figl` from the working directory upward, then
`$XDG_CONFIG_HOME/fig/languages.figl`. A format found there registers
through the helper carrier, and then:

```
fig get app.hcl service.port
fig set app.hcl service.port 8080
fig convert app.hcl app.yaml
fig check app.hcl
fig fmt app.hcl
fig set notes.md --embed hcl-frontmatter title Hi
```

each does what it does for a built-in. Extension resolution already asks
each language for the extensions it owns (`cli/args.zig`,
`extensionFormat`); a registered language is asked last.

Two things stay different on purpose. A runtime format never joins content
sniffing: `sniff_rank` is a frozen order over the built-ins and a helper that
claimed too much would take files from them, so a runtime format resolves
by extension or by name only. And a runtime format has no `--spec`
versions.

New actions: `fig lang list`, the built-in and registered formats with their
capabilities, and `fig lang check`, §6.

Cost: one process per invocation, a few milliseconds, against files whose
whole point is to be small. The `fig` binary grows by the protocol and a
spawn.

### 7.2 Rust

```rust
pub trait Language {
    fn describe(&self) -> Description;
    fn parse(&self, input: &[u8]) -> Result<NodeTable, ParseError>;
    fn print(&self, doc: &NodeTable) -> Result<Vec<u8>, PrintError>;
}
pub fn register(lang: impl Language + 'static) -> Result<Format, LoadError>;
```

`Format` is `#[non_exhaustive]` already, so it gains a `Runtime(RuntimeId)`
variant without a Rust major. `Document::parse`, `serialize`,
`capabilities`, and the editor take it as they take any other. The helper
runner is `fig::helper::spawn(command) -> Result<Format>`, for a Rust host
that wants to use a helper without linking its engine.

`fig-lua` is a crate implementing `Language` over a vendored Lua, plus a
`fig-lua` binary speaking the helper protocol. One crate, both carriers.

### 7.3 TypeScript

```ts
const hcl = registerLanguage({
  describe: () => ({ name: "hcl", extensions: ["hcl"], caps, syntax, samples }),
  parse: (input) => nodeTable,
  print: (table) => bytes,
});
parse(input, hcl);
```

The object is the in-process carrier; the wasm calls back through imports,
and the node table is passed as typed arrays. `@diaryx/fig-lua`, if it
exists, is a second package carrying its own wasm with the Lua VM in it,
registering through the same call. The core package does not change size.

### 7.4 Zig

A runtime format is one more `Language`: `src/languages/runtime/runtime.zig`
declares `Type = RuntimeId`, a `Parser` and `Printer` that forward to the
vtable at that id, one dialect row named `runtime`, and a `syntax` that
reads the record. It passes `validate` like every other format, so the
engine does not learn a new case. The reified enums each gain the one
member `.runtime`, and the id rides beside it. That is the single break in
the Zig API this proposal takes, and it is taken once for every engine that
will ever exist.

## 8. Open questions and the answers taken

**8.1 One dialect per runtime language.** A compiled language may declare
several (`json`/`jsonc`/`json5`). A runtime one declares one. A format with
dialects registers them as separate languages sharing a helper. Revisit if a
real script needs otherwise.

**8.2 No aliases.** YAML's `alias` node kind and its `materialize` step are
a reference layer no other format has. A runtime format cannot produce an
alias row. This is the one `FigNodeKind` the table refuses.

**8.3 No lossless envelope.** `caps.lossless` is null for every runtime
format: the `$fig` envelope needs a typed value model and a place to carry
it, and both are things a format author should get right in a compiled
format first.

**8.4 No hooks.** Twenty-two editing hooks exist because a handful of
formats need to spell a fragment the splice engine cannot: a TOML `[header]`
line, a YAML block scalar. The first cut of runtime languages gets none of
them. A runtime format edits through `syntax` and refuses the rest. The
argument for stopping here: every hook is a function call with engine state
on both sides, which is exactly what a boundary makes expensive and hard to
validate, and the formats that need hooks are the ones that are already
compiled in. If a runtime format turns out to need one, the right shape is
probably a *data* answer, a new `Syntax` field, which is how the 2.x
proposals removed hooks from compiled formats too.

**8.5 Process-global state.** `fig.h` promises no shared mutable global
state, and a registry is one. The answer taken: the registry is
append-only, guarded by a mutex, and a registration is complete before its
integer is returned, so every read of a registered entry is of an immutable
value. The threading note gains a sentence saying so. Unregistration does
not exist; a format integer is valid for the life of the process.

**8.6 Integers are not stable across processes.** A runtime format's
integer is assigned in registration order. Callers that persist a format
persist its name, and `fig_format_by_name` resolves it. This is also why
§7.1 configures by name.

**8.7 Where the SDK lives.** A helper author in another language needs to
serialize a node table and run a request loop. That is small, and the first
one lives as a module in the `fig` crate. A `fig-extension` crate is not
created until a second implementor wants it.

## 9. What 3.0 needs before it tags

One thing: **a reserved range.** `fig.h` gains

```c
// Format integers at or above this value name a language registered at
// runtime (fig_language_register); they are assigned per process and are
// never pinned here. Every compiled-in format is below it.
#define FIG_FORMAT_RUNTIME_BASE 4096
```

and `c_api.zig`'s pinned table gains a comptime check that no built-in value
reaches it. That is a comment and an assertion, and it costs 3.0 nothing.
Without it, a later minor could not add a runtime integer without arguing
about whether the value collided with a possible future compiled format.

## 10. Sequencing

1. **core 3.0.0** (now): §9.
2. **core 3.1**: the node table as a stated shape; `runtime.zig`; the
   `.runtime` member; `fig_language_register`, `fig_format_by_name`, and the
   vtable in `fig.h`; the load-time harness; abi-check holds the vtable's
   `version`. Additive, ABI stays 2.
3. **cli 4.1**: the helper carrier, `languages.figl`, `fig lang list`,
   `fig lang check`.
4. **fig-lua 0.1**: a new repository in `repos.figl`. The crate, the
   binary, and one worked format, HCL or NestedText-in-Lua as the
   conformance twin of a compiled format, so the harness has a known
   answer to compare against.
5. **rust 3.6, npm 3.1**: the trait, the object, the runner.

Each step is verified as pluggable formats §7 was: `zig build check` green,
the harness over the compiled formats unchanged, and the harness over the
worked Lua format passing the same list.

## 11. What this is not

It is not a plugin system for the engine. Nothing here lets a format change
how the editor works, what a path means, or how a convert is diagnosed. It
is exactly the format contract, carried across a boundary, and validated on
the way in. That narrowness is what makes it a minor, and it is what makes
"any format, from anywhere" true without making "anything, from anywhere"
true.
