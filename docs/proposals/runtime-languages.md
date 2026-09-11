```fig
title = Runtime languages
description = A format fig did not compile in — the Language contract carried as a vtable in-process and as a helper protocol out-of-process, with the compiled formats made to pass through the same contract first and fig-lua as the first outside implementor
created = 2026-09-07
status = draft
updated = 2026-09-10
part_of = [proposals](proposals.md)
```

# Runtime languages

> **Status: DRAFT; §3.3, §9, the carrier (core, rust, cli) and fig-lua implemented; npm (§10 step 7) next.**
> Written against `main` at e56489d, with core 3.0.0 (ABI 2) built and
> unreleased. §9's reserved range and §3.3's refactor — the compiled
> formats editing through the contract, every hook deleted — both landed
> on `main` ahead of the 3.0 tag; §4.4 and §10 say where and what
> differed. The carriers of §4.3 land in 3.0 as well: the tag waits until
> the first outside format runs through them (§10), so that a 3.0 consumer
> gets a contract that has been crossed, not one that has only been
> stated. What 3.0 then adds over 2.x is additive at the ABI (ABI stays 2);
> the rest is one new repository and the bindings' half.
>
> The second draft. The first stated the editing contract as "`syntax` and
> no hooks" and listed dialects, aliases, and the lossless envelope as
> things a runtime format does not get. Reading every hook (§4.4) showed
> that each of those four is a small addition to the contract rather than
> a limit of it, and that the compiled formats want the same additions.
> §4 is rewritten on that basis, §8 records what is still open, and §10
> sequences the core refactor before the first outside format.
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
registry and named the boundary a format writes against. Commit 2226998
then proved the boundary is real: a `Language` written outside the tree,
with its own name, extension, caps, dialect row, and syntax, passes
`validate` and drives `Editor`. What is left of "a format" is a fixed set of
declarations, a node table with spans, and a `syntax` record. None of that is
inherently comptime. It is a vtable that happens to be spelled as
declarations, and a vtable can be filled at runtime.

The obvious shape, a scripting engine embedded in the core library, is
rejected here before it is argued, because it fails on bundling: a VM in
every binding, the wasm payload, and the static builds, paid by every
consumer for a feature most never use. What is argued instead is narrower:
**core defines the contract's two runtime carriers and nothing else.** The
engines live in their own repositories and are peers of each other.

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

The editor's one mutator is `replaceAtSpan`: splice a byte range, reparse,
and roll the source back if the reparse fails. Every edit, generic or
hooked, ends there. This is the fact §4.4 is built on.

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
the editor already consumes, every edit is a splice under a reparse net,
the harness is the conformance suite, and the ABI addresses formats by
integer. Each of those is a precondition for what follows, and each exists.

## 3. What is being asked for

Five things, in order of how much they cost core.

**3.1 A format resolved at runtime is a peer of a compiled one.** Every
entry point that takes a format today accepts a runtime one: the CLI
actions, `fig_parse`, `fig_document_serialize`, `fig_editor_*`, the Rust
`Document::parse`, the TypeScript `parse`. Nothing gains a second API for
"but loaded".

A runtime format joins at one of three tiers, and `caps` says which:

- **Read.** `parse` only. `fig get`, `fig check`, and `fig convert` out of
  the format. No `syntax`. This is most of the value for an HCL file or a
  house dialect, and it is the tier a format author reaches first.
- **Print.** Adds `print`, so `fig convert` into the format and `fig fmt`.
- **Edit.** Adds `syntax` and, where the format needs them, the fragment
  renderers of §4.4. The full `fig set`, `fig delete`, `fig move`, and the
  comment ops.

**3.2 The engine that produced the format is core's business only through
one contract.** Core never links Lua, a wasm runtime, or a dynamic loader.
What it links is a struct of function pointers. The process-spawning half
lives beside the CLI and the Rust crate, not in the library, so the wasm
build never carries it.

**3.3 The compiled formats pass through the same contract.** Before the
first outside format lands, the in-tree formats are made to edit through
the node table, `Syntax`, and the renderers of §4, and the hooks that
those replace are deleted. That is the proof the contract is complete, and
it is what lets a runtime twin of a compiled format be checked against it
byte for byte. §3.5 says why the compiled formats nonetheless stay
compiled.

**3.4 The first engine is a separate repository, `fig-lua`.** It is the
canonical and recommended way to write a runtime format, because one script
serves the CLI, Rust, and the browser. It is not the only way, and core
does not know its name.

**3.5 Not every format in Lua.** Once §3.3 holds, the contract would allow
the compiled formats to be rewritten as scripts. That is not proposed. It
would put a VM back into every binding, which is the objection §1 starts
from, and it would make the YAML parser and its conformance suite slower
for no gain. The invariant worth having is that every compiled format is
*expressible* through the contract, checked by a runtime twin in the
harness, not that every format is carried by it.

## 4. The contract

The contract is what a compiled `Language` already declares, restated as
values rather than declarations. It has one new part, the node table,
because the parse result crosses a boundary and its shape has to be
written down, and one changed part, the editing surface, because the
hooks turned out to be mostly facts the parser dropped.

### 4.1 The node table

A parse returns a flat table, one row per node in pre-order, plus three
side tables. This is the shape `Document` already holds — its columns are
`AST.Node`, `node_spans`, and the span and region tables the §3.3 refactor
added — restated as values rather than as slices of a Zig struct. Row
index is node id, which is what `AST.Node.Id` already is.

| Column | Meaning |
|---|---|
| `kind` | `FigNodeKind`: null, bool, int, float, string, sequence, mapping, keyvalue, alias — what `fig_node_kind` reports. |
| `ext_kind` | `FigExtKind` or none. A plist date, a TOML datetime, a ZON enum literal: the node `fig_node_kind` reports as a string or int and `fig_node_extended` tells apart. |
| `parent` | Row index of the parent, or none for the root. Core rebuilds `next_sibling` from it. |
| `span` | `[start, end)` byte offsets into the input. Required of every node; the editor splices by it. For a node with an anchor or tag, the span includes them, as YAML's does today. |
| `text` | For scalars, the *decoded* value as bytes; for int and float, the lexeme, as `fig_node_number` already returns it; for an alias, the anchor name; for an extended kind, its payload. |
| `anchor`, `anchor_span` | The anchor name this node defines and where it is written, or none. Present so that aliases resolve; core builds the anchor table from it in row order. |
| `tag`, `tag_span` | The tag on this node, verbatim, and where it is written, or none. A format with no tag syntax leaves the column empty. |
| `marker` | For a block-sequence item: the span of the token that introduces it — the `-`, the `*` — or none. Where a leading comment goes and where a delete or reorder starts (`Document.node_marker_spans`). |
| `sep` | For a keyvalue: the span of the token that separates key from value, or none. A recorded separator is the parser's statement that the value *reframes* rather than splices in place; a zero-width one marks a value hanging under a bare key (`Document.node_sep_spans`). |

The side tables, each sorted by row:

- **regions** — `(node, start, end)`: the whole header lines of a
  *section* node, one per line that created or re-opened it
  (`Document.node_regions`). Presence here is what makes a node a
  section.
- **mentions** — `(node, span, header | entry)`: every place a section
  node's name is written, and whether that line is a header of the node's
  own or a mention on its parent's entry line (`Document.node_mentions`).
- **comments** — `(node, leading | trailing | dangling, line | block,
  text)`: `AST.NodeComments`, flattened. A format with no comment syntax
  declares `comments = null` in its `syntax` and returns none.

Key–value pairs are three rows, as they are in the AST: the `keyvalue`, its
key, its value. That is the one place the table is less obvious than a
tree, and it is chosen because it is what the editor, `fig_node_first_child`,
and every binding already walk.

The first draft of this table had `marker_start` and `value_slot_start` as
offsets and an `is_flow` column. Doing §3.3 settled all three: the marker
and the separator are spans (a zero-width separator is a fact the editor
reads), and `is_flow` is `Syntax.flow_containers` plus one engine rule. The
table above is the shape as built, and §4.4's "as implemented" note says
what each column retired.

A print takes the same table, produced by core from a document, and returns
bytes. Spans are none on that side. A runtime format's printer sees the
AST the same way a compiled one does, through the tree, and nothing else
about the engine is exposed to it.

### 4.2 The declarations

Everything else a `Language` declares becomes a field of one record:

- `name`, `extensions`, `caps` (`read`, `edit`, `serialize`,
  `max_mapping_depth`, `lossless`).
- `lossless` is the ten booleans of `manifest.NativeKinds`, declared as
  data. The `$fig` envelope encoder in `src/lossless.zig` never branches on
  format identity; it reads exactly those ten, so a runtime format that
  declares them gets the envelope. What a runtime format cannot do is add
  an eleventh kind; that is a new `ExtKind` member and stays a core change.
- `syntax`, the `manifest.Syntax` fields as they stand, plus the five §4.4
  adds.
- `dialects`, one or more rows, each with `name`, `extensions`, `splice`
  (literal, json_string, or raw), `empty_doc_seed`, `specs`, and its own
  `syntax` where it differs from the language's. A dialect in the compiled
  formats is a mode value threaded to `parse`, a printer entry point, and a
  `syntax` that may vary by mode; a runtime language's `parse` and `print`
  take the row's name and its `syntax` is looked up per row, which is the
  same thing with the mode spelled as a string.
- `samples`, which are not optional here. §6 says why.
- The fragment renderers of §4.4, each optional.

### 4.3 The two carriers

**In-process: a vtable.** A C struct with a `version` field first, the
declarations of §4.2 as plain fields, and function pointers: `parse` (input
and dialect to node table), `print` (node table and dialect to bytes),
`free` for what those allocated, `describe`, which returns the record so a
host can register a language it did not write, and one slot per renderer,
null where the format declares none. Registration is
`fig_language_register(const FigLanguageVTable *, int *out_format)`, one
call per dialect row. The vtable's `version` is the fifth versioned
surface, and it is bumped on the same rule as `FIG_ABI_VERSION`: only when
a field changes meaning.

**Out-of-process: a helper protocol.** The same calls as a request and
response over a child process's stdin and stdout. The request carries the
input bytes; the response carries the node table serialized. This is the
git remote helper model: the helper is any executable, in any language, that
speaks the protocol. A helper is asked `describe` first. The CLI spawns it
once per invocation; a long-lived host spawns it on first use, and a helper
that exits is respawned on the next request and reported as a diagnostic if
it exits again on the same one.

The serialization is JSON, for one reason: every host that could write a
helper already has a JSON library, and fig itself parses it. It is not fig's
own format because a helper author should not need fig to write a helper.

**These are the same contract.** The helper runner is a vtable whose
functions talk to a process. It lives beside the CLI and in the Rust crate,
never in the library, and it registers through the same
`fig_language_register` path a host uses for a vtable it wrote itself.
There is no third way in.

### 4.4 The editing surface

The first draft said a runtime format edits through `syntax` alone and
refuses the rest, and argued that hooks are for a handful of formats that
are already compiled in. The count says otherwise. Of the ten editable
compiled formats, six declare hooks, and every sectioned or
indentation-structured one does:

| Editable format | Hooks declared |
|---|---|
| json, dotenv, properties, zon | 0 |
| ini | 1 |
| toml | 2, plus 3 section ops |
| yaml | 3 |
| fig | 4 |
| nestedtext | 8 |
| plist | 10 |

So "no hooks" would have meant "flat and flow formats only", and the
worked examples the first draft named, HCL and NestedText, were both on
the wrong side of that line. Reading all twenty-five gives a different
picture. Every one ends in at most one `replaceAtSpan`. None mutates the
AST, writes a side table, or calls back into the engine with state; the
two engine methods hooks do call are read-only derivations over regions.
A hook is already a pure function from the source, the node table, and
the arguments to a splice. What forces it to be Zig is not engine access
but three kinds of missing input, in decreasing order of how many hooks
they account for.

**Facts the parser dropped.** These are the four columns §4.1 adds. With
`marker_start`, NestedText's `seqItemLineStart`, `removeSeqItem`, and
`reorderSeqItems` are byte-for-byte the generic bodies. With name spans
on regions, TOML's `replaceKeyAtPath` and `renameContainer` become
"replace every recorded mention". With `is_flow`, INI declares nothing.
With `value_slot_start`, the reframe in YAML and fig starts from a column
the engine reads rather than one the hook scans for.

**Engine constants that are really syntax.** Five fields join `Syntax`:

- `indent_unit`: the bytes one nesting level adds, `"  "` for YAML, four
  spaces for NestedText, `"> "` for fig. Today `col + 2` in the engine.
- `seq_item_marker`: `"- "` for YAML, `"* "` for fig, `""` for plist. Today
  a literal in `insertSeqLine`.
- `comments.line` and `comments.trailing` become a pair, `open` and an
  optional `close`, with an optional `forbidden_in_body`. plist's
  `<!-- -->` is a pair; a prefix cannot spell it. This one change deletes
  all six plist comment hooks, whose placement logic is identical to the
  generic and which differ only in the delimiter.
- `section_header`: `open`, `close`, `sep`, and whether index segments are
  skipped, so `[a.b]` and `[[a]]` are data. This retires TOML's
  `insertContainer` and `appendContainerToSeq`.
- A `KeyStyle` variant for TOML's bare-or-quoted rule.

And one policy change with no new field: the block insert copies the
anchor line's prefix bytes instead of counting a column. That is what
`structural_indent` already declares, and it is fig's whole reason for
hooking `insertKey`, `appendToSeq`, and `prependToSeq`.

**Fragment renderers.** What is left after the data is a short list of
functions, each taking strings and returning a string, none touching the
editor:

| Renderer | Signature | Who needs it |
|---|---|---|
| `render_value` | `(text) -> text` | plist, which wraps every literal in a typed element |
| `render_entry` | `(key, value, indent) -> text` | plist, NestedText |
| `render_item` | `(value, indent) -> text` | plist, NestedText |
| `render_key` | `(name, old_form) -> text or refuse` | NestedText's multiline keys |
| `render_block` | `(text, depth) -> text or refuse` | fig and YAML, re-framing a value onto following lines |

A renderer is called by the engine and its result is spliced under the
reparse net, so a renderer that returns bad text is blamed for it the way
a bad edit argument is today, and the file is untouched. `render_block`
is the one that parses: it takes a fragment, parses it with the format's
own `parse`, and reprints it at a depth. That is a call from the engine to
the format's reader, which the engine makes on every splice anyway.

No renderer receives engine state, performs a splice, or is called more
than once per edit. That is the whole of what a runtime format can do to
the editor, and after §3.3 it is the whole of what a compiled format can
do too.

**As implemented.** The refactor landed on `main` before the 3.0 tag, in
seven commits from 70dcfdd, and the contract it arrived at differs from
the draft above in the details that only doing it could settle:

- The node table's item column is a marker *span* (`node_marker_spans`),
  and `value_slot_start` is the *separator* span (`node_sep_spans`), from
  which the value slot follows; a recorded separator is also the parser's
  statement that the entry's value reframes, and a zero-width one marks a
  value hanging under a bare key (a fig header, a NestedText multiline
  key). `is_flow` is not a column: `Syntax.flow_containers` plus one engine
  rule (a section format's root and its section nodes are block) settled
  every sniff that a hook existed to dodge. The `region` column's name
  spans became a table of their own, `node_mentions`, each mention marked
  as a header line of the node's own or a mention on its parent's entry
  line; TOML, INI and fig all record them, so `renameContainer` is generic
  for all three.
- `Syntax` gained more than five fields: `indent_unit`, `seq_item_marker`,
  the `CommentDelimiter` pair with `forbidden` text, `section_header` (with
  its sequence form), and a `bare_or_quoted` `KeyStyle` as drafted, plus
  `flow_containers`, `closed_containers` (a self-closing container's
  tokens, which is also what expands an empty `<dict/>`),
  `flow_kv_sep_from_siblings` and `flow_map_pad` for fig's flow objects,
  and `merge_key`, which is what `keyIsInherited` became.
- The renderers are `renderValue`, `renderEntry`, `renderItem`,
  `renderTail` and `renderKey`. `render_block` became `renderTail` — what
  follows a key, separator included, inline or re-framed — because fig's
  block form drops the separator and YAML's keeps it, which no engine rule
  could know. An empty `key_text` to `renderTail` is the document root.
- `replaceValAtPathFollowing` needed nothing: the alias kind, its
  resolution and the anchor and tag span tables are all core.

**Aliases.** The alias node kind carries a name and nothing else; anchors
are a side table; `resolveAlias`, `resolveDeep`, and `mergedChild` are
core code in `src/ast/reader.zig` that key on kind, not on format. The
alias-and-merge half of YAML's `materialize` is generic; the tag half is
YAML vocabulary, `!!str` and friends and the `<<` spelling. A runtime
format that returns alias rows and an `anchor` column, in document order,
gets resolution, copy-on-write on edit, and `replaceValAtPathFollowing`
from the engine with no format code. YAML's `keyIsInherited` becomes a
question the engine asks the AST. Tag vocabulary and the merge key spelling
stay YAML's until a script needs them, at which point they are two more
fields. One guard is needed: `materialize` indexes the tag table without a
length check, which is safe today only because YAML's parser fills it.

## 5. Why core links no engine

A scripting VM in the core library is a cost paid by every binding, the
wasm build, and the static payloads for a feature most consumers never
use. That objection is right and the design survives it because the
engines are not in core, and each host gets the carrier it can actually
run:

| Host | In-process carrier | Out-of-process carrier |
|---|---|---|
| CLI (`fig` binary) | none: no engine linked | helpers on PATH, configured (§7.1) |
| Rust (`fig` crate) | a `Language` trait; `fig-lua` implements it | the same helper runner, exposed as a function |
| TypeScript (`@diaryx/fig`) | a JS object with the contract's members, reached through wasm imports | none: no process |
| Zig | the vtable directly | the helper runner |

A wasm runtime cannot run inside the wasm build. A dynamic loader cannot
run there either. A Lua VM could, but it would be a cost paid by every
consumer for a feature most never use. Putting the engines outside makes
every one of those a decision the consumer takes, and makes the set of
engines open: a helper written in Python is a peer of `fig-lua` on the day
it is written.

The TypeScript row is the one that needs a sentence. The npm build is
WASI, and a JS object is reached by the module importing a host function
per contract member and calling out through it. The node table crosses as
typed arrays in linear memory. A parse that calls out is not re-entrant
with itself, which is fine, because the contract never asks a format to
parse while it is parsing.

## 6. Validation is where the safety is

`validate` runs at compile time and the harness at test time. A runtime
format has neither moment, so it gets one: **load**. Registering a language
runs the same checks in the same order, and refuses on the first failure:

1. The record is well-formed: `name` is an identifier, `extensions` are
   non-empty and not owned by a compiled format, `caps.edit` implies a
   `syntax`, `syntax` is coherent (a `section_noun` implies regions;
   `comments = null` implies no comment ops; a `section_header` implies a
   `section_noun`).
2. Every `sample` parses, prints, and reparses to the same node table.
3. Every node table is well-formed: spans are within the input, nested
   correctly, pre-ordered; `marker_start` and `value_slot_start` fall
   inside their row's line; regions are whole lines on container nodes,
   sorted, each name span inside its line; every alias names an anchor
   defined on an earlier row.
4. The `empty_doc_seed` parses.
5. If `caps.edit`, an `Editor` constructs over every sample and a no-op
   splice is the identity.
6. If `caps.edit`, each declared renderer is called over every sample's
   nodes with an identity argument and the result reparses to the same
   table. A renderer that cannot render its own format's samples is
   refused at load, not at the first edit.

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

each does what it does for a built-in, at the tier the format declares. A
read-tier format answers `get`, `check`, and `convert` out of itself and
refuses `set` with the same diagnostic a read-only compiled format gives.
Extension resolution already asks each language for the extensions it owns
(`src/cli/args.zig`, `extensionFormat`); a registered language is asked
last.

One thing stays different on purpose. A runtime format never joins content
sniffing: `sniff_rank` is a frozen order over the built-ins and a helper that
claimed too much would take files from them, so a runtime format resolves
by extension or by name only. `--spec` works where the format declares
`specs` on a dialect row, as it does for a compiled one.

New actions: `fig lang list`, the built-in and registered formats with their
capabilities, and `fig lang check`, §6.

Cost: one process per invocation, a few milliseconds, against files whose
whole point is to be small. The `fig` binary grows by the protocol and a
spawn.

### 7.2 Rust

```rust
pub trait Language {
    fn describe(&self) -> Description;
    fn parse(&self, input: &[u8], dialect: &str) -> Result<NodeTable, ParseError>;
    fn print(&self, doc: &NodeTable, dialect: &str) -> Result<Vec<u8>, PrintError>;
    fn render(&self, which: Renderer, args: RenderArgs) -> Result<Option<Vec<u8>>, RenderError> {
        Ok(None)
    }
}
pub fn register(lang: impl Language + 'static) -> Result<Vec<Format>, LoadError>;
```

`Format` is `#[non_exhaustive]` already, so it gains a `Runtime(RuntimeId)`
variant without a Rust major. `Document::parse`, `serialize`,
`capabilities`, and the editor take it as they take any other. The helper
runner is `fig::helper::spawn(command) -> Result<Vec<Format>>`, for a Rust
host that wants to use a helper without linking its engine.

`fig-lua` is a crate implementing `Language` over a vendored Lua, plus a
`fig-lua` binary speaking the helper protocol. One crate, both carriers.

### 7.3 TypeScript

```ts
const hcl = registerLanguage({
  describe: () => ({ name: "hcl", dialects, caps, syntax, samples }),
  parse: (input, dialect) => nodeTable,
  print: (table, dialect) => bytes,
  render: { value: (text) => text },
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
vtable at that id, one dialect row named `runtime`, a `syntax` that reads
the record, and the renderers as hooks that forward to the vtable's slots.
It passes `validate` like every other format, so the engine does not learn
a new case. The reified enums each gain the one member `.runtime`, and the
id rides beside it. That is the single break in the Zig API this proposal
takes, and it is taken once for every engine that will ever exist.

After §3.3, the compiled formats declare the same renderers where they
declare hooks today, and the engine calls both through one path. A
compiled renderer is a Zig function; a runtime one is a vtable slot. The
engine does not know which.

## 8. Open questions and the answers taken

**8.1 Sniffing stays closed.** A runtime format is never a candidate in
`Language.detect`. The reason is stated in §7.1 and there is no version of
this proposal in which it changes.

**8.2 `FigFormat` is exhaustive.** The C entry points switch on it with
`inline else`, and a runtime integer cannot be a member. Every conversion
from the C `int` becomes a checked lookup that routes an integer at or
above `FIG_FORMAT_RUNTIME_BASE` to the registry before the switch. That is
work in §10 step 3, named here so it is not a surprise there.

**8.3 Tag vocabulary and the merge key.** The node table carries a `tag`
column, but what a tag means, `!!str` collapsing a kind, `<<` merging a
mapping, is YAML's and stays in its `materialize`. A runtime format that
wants either declares nothing yet and gets the generic half: aliases
resolve, tags ride through untouched. When a script needs the rest, it is
a `tag_vocabulary` and a `merge_key` field, and YAML's become the first
values of them.

**8.4 A renderer per dialect.** A renderer is declared on the language and
receives the dialect name. No compiled format needs a renderer that varies
by dialect; if a runtime one does, the argument is already there.

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

**8.8 What is not covered by the renderers.** The five renderers and five
`Syntax` fields are the set the twenty-five hooks reduce to. A format
that needs a sixth is a format no compiled one resembles, and the answer
is the same one that produced the first five: read what it does, and if
it is a fact the parser has, add a column; if it is a constant, add a
field; if it is a string function, add a renderer. What is refused is a
hook that receives the editor.

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

**Done, on `main` ahead of the 3.0 tag.** The number lives once, as
`Language.runtime_abi_base`; the registry refuses a row at or above it and
the pin in `c_api.zig` refuses a literal there; fig.h and `fig-sys` each
state it, and `zig build abi-check` holds both to the registry's value.

## 10. Sequencing

1. **core 3.0** (now): §9. Done.
2. **The refactor — done, in core 3.0** (70dcfdd and the
   six commits after it): the node-table columns, filled by every compiled
   parser; the `Syntax` fields and the prefix-bytes policy; the engine
   consuming them; the renderers as the hook set. Hooks were deleted one
   format at a time in the order given here — INI, plist's comment hooks,
   the sequence hooks, the renderers, the reframes, TOML, YAML — each step
   verified by the harness and the format's own editor tests. The
   behavioural changes found are recorded as `Behavioural-change:` trailers
   on the commits: a plist entry appended after a commented value now lands
   after the comment, an empty plist container expands with the declared
   unit, an empty comment is written `<!-- -->`, and NestedText's empty
   inline `{}` and `[]` accept an insert. §4.4's "as implemented" note has
   what differed from this draft.
3. **core 3.0, the carrier** — in 3.0 rather than a later minor, because
   the tag waits for step 6. The node table of §4.1 as a C shape;
   `runtime.zig`, which converts it to and from a `Document`, validates a
   vtable by the same rules `Language.validate` applies at comptime, and
   keeps the registry of §8.5; a `Runtime` `Language` whose `Type` indexes
   that registry, so `Editor` and the harness instantiate once over it and
   the C API's editor union gains one `.runtime` arm; `fig_language_register`,
   `fig_format_by_name`, the checked integer lookup of §8.2 at every
   `FigFormat` entry point, and the vtable in `fig.h`; registration runs the
   harness over the declared `samples` and refuses on failure; abi-check
   holds the vtable's `version`. One engine edit: the renderers' presence
   is a question (`hasRenderer`) rather than a `@hasDecl`, since a runtime
   language answers it with a null pointer. Additive, ABI stays 2.
4. **rust 3.6**: the `Language` trait and the object, registering through
   the vtable; the helper SDK of §8.7 as a module — a table serializer and
   a request loop — since the first helper is written against it.
5. **cli 4.1**: the helper runner, `languages.figl`, `fig lang list`,
   `fig lang check`, and `--lang <name>` to select a registered language
   by name where the extension would resolve to a compiled one. Done on
   `main`: `src/cli/languages.zig` is the runner (a vtable over a child
   process, registered through the same `Runtime.register` a host's own
   vtable is) and the configuration, and `fig lang table <file>` prints the
   table a compiled format gives — what a twin is written against;
   `tools/cli-lang-check.sh` drives the
   built CLI through the Rust crate's `tinykv_helper` example at every
   action, and `zig build check` runs it. Two things it does not yet do:
   a `get` of a runtime target has no loss diagnostics (`fig_document_diagnose`
   answers `unsupported_operation` for one), and `patch` into a runtime
   target is refused.
6. **fig-lua 0.1**: a new repository in `repos.figl`. A Rust crate on
   `mlua` with Lua 5.4 vendored, a binary that speaks the helper protocol,
   and two worked formats as `.lua` files: dotenv, the twin of the format
   commit 2226998 already used to prove the boundary, and plist, the twin
   of the format that declared the most hooks. `fig lang check` holds each
   to its compiled sibling table for table. The bar for the 3.0 tag is
   `fig get secrets.env --lang lua-dotenv` answering through the helper
   with the same table the compiled format gives. HCL follows as the first
   format with no sibling, at the read tier first. Done, in the
   `fig-lua` checkout (2026-09-10, ahead of the repository being created):
   `lua-dotenv` and `lua-plist` pass `fig lang check --against` on every
   fixture and on fig's plist corpus, refuse what the compiled parsers
   refuse with the same words at the same offsets, and leave the same file
   as the compiled format after the same edits. Two things the twins
   surfaced in the compiled formats are filed as tasks: an entry appended
   after `export KEY=value` is indented to the key's column, and the Rust
   editor spells a key through the printer, which for plist is an element.
7. **npm 3.1**: the object and the runner.

The first draft had the bindings after fig-lua. That was backwards: fig-lua
is a Rust crate registering through the Rust binding, so the binding's
half comes first, and the CLI's runner is what the tag is measured by, so
it comes before the repository it runs.

Each step is verified as pluggable formats §7 was: `zig build check` green,
the harness over the compiled formats unchanged, and the harness over each
Lua twin producing the same tables its sibling does.

## 11. What this is not

It is not a plugin system for the engine. Nothing here lets a format change
how the editor works, what a path means, or how a convert is diagnosed. It
is exactly the format contract, carried across a boundary, and validated on
the way in. A renderer returns text; it never receives the editor. That
narrowness is what makes it a minor, and it is what makes "any format, from
anywhere" true without making "anything, from anywhere" true.
