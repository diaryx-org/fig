```fig
title = Kit parity
part_of = [Proposals](/docs/proposals/proposals.md)
description = Every language fig supports authoring in gets the same kit — node table, row writer, tree, scanner, grammar, printer, rules — with one implementation of each rule and each binding's layers held to the same answers
author = adammharris
created = 2026-10-14
status = implemented
updated = 2026-10-14
```

# Kit parity

**Status.** Accepted, 2026-10-14: Adam took the recommendation on all four
decisions. Grammar combinators come to Rust and Zig. C gets the rules,
`fig_table_finish` and `fig_table_finish_in_order` (and
`fig_table_child_by_key`). `Table.finish` comes to the JavaScript kit, with
fig-lua's and fig-quickjs's twins left to their own repositories. XML
comes to Rust and C through `fig_kit_xml_tokenize`. The names below were
fixed first, before the 6.0.0 tag; the rest is additive, for 6.1. The
choices marked **Decision** are kept as they were argued.

Implemented in fig, 2026-10-14, every layer in every binding but fig-lua's
and fig-quickjs's, which are their repositories' to take up. Where the
work departed from the plan:

- C also gained `fig_table_row(t, id)`: without a way to reach a row
  already written, a C parser cannot link it to one written later, which
  is what `fig_table_finish` is for.
- `RowWriter.finish` was fixed on the way: it left a moved table's side
  facts in the order they were written rather than node order, left each
  moved row's links naming old rows, and could loop on a cycle in links a
  caller wrote.
- Grammar semantics are held by the dotenv twins rather than by a
  `kit/testdata/grammar/` of their own; the twins exercise every rule
  dotenv uses, in four bindings, against one compiled format.
- The dotenv fixtures stayed where the JavaScript twins have them
  (`bindings/typescript/test/fixtures/dotenv/`), and the Zig, C and Rust
  twins read them there, rather than moving them to `languages/dotenv/`.
- The JavaScript XML kit stays grammar-shaped and is held by the plist
  twin; the token fixtures (`kit/testdata/xml/`) hold Zig, C and Rust.

## Names

| Layer | Zig `kit` | JS `@diaryx/fig/kit` | Rust `fig::kit` | C `fig.h` |
|---|---|---|---|---|
| 1 Rules | `number`, `datetime`, `literal`, `json_string` | `/number`, `/datetime`, `/literal`, `/json-string` | `number`, `datetime`, `literal`, `json_string` | `fig_kit_number_*`, `fig_kit_datetime_*`, `fig_kit_literal_*`, `fig_kit_json_string_*` |
| 2 Row writer | `RowWriter`, `ChildIndex`, `flat_map` | `Table`: `append`, `first`, `next`, `put`, `childByKey`, `finish` | `rows::RowWriter` | `fig_table_finish`, `fig_table_finish_in_order`, `fig_table_child_by_key` |
| 3 Tree | `AST` (unchanged) | `mapping`, `sequence`, `entry`, `scalar`, `rows` | `tree` | — |
| 4 Scanner | `Scanner` | `Scanner`, `Cursor` | `scan::Scanner` | — |
| 5 Grammar | `grammar` | `/grammar` | `grammar` | — |
| 6 Printer | `Writer` | `index`, `Writer` | `print::{Index, Writer}` | — |
| 7 XML | `xml` | `/xml` | `xml` | `fig_kit_xml_tokenize` |

The duplicate policies are named as the JavaScript kit names them,
`last_value`, `first_value`, `keep` and `error`, everywhere but Zig's
`flat_map`, which keeps its `overwrite` and `err` for the first and last.

## The problem

fig 6.0's core ships no formats. Every format is a language registered
through one contract, and "bring your own format" is the README's pitch. How
easy that is depends on the binding. A JavaScript or Lua author gets a node
table, a tree that lays itself out, a byte scanner, grammar combinators, a
printer's writer and the XML rules, so a dotenv twin is about 150 lines of
description. A Rust author gets the contract types, `NodeTable::push`, and
`fig::kit::{number, datetime}`. They write every row by hand, in pre-order,
with every comment bound by row id. A C author gets `fig_table_*`, which
writes the rows in pre-order, and the eight `fig_kit_*` rule functions. The
Zig kit is the richest underneath (`RowWriter`, `ChildIndex`, `flat_map`,
the lexical rules) but has no grammar layer, no scanner and no printer's
writer. Its own languages are hand-written tokenizers.

Two of the runners fall behind in a different way. fig-lua and fig-quickjs
have a full JS-shaped kit, but it is a native copy, pinned to `fig = "4"`.
Their number and datetime rules are their own twins rather than the Zig
kit's. fig-quickjs's `fig.js`, `number.js` and `datetime.js` have drifted
from `@diaryx/fig/kit`, and its vendored `wire.js` from `dist/wire.js`.

## The layers

The kit is named here by layer, from the contract up. Every binding already
has layer 0. Parity is about layers 1 to 7.

| # | Layer | What it is for |
|---|---|---|
| 0 | Contract | `Language`/vtable, `Description`, the node table's shape, the error with an offset. Not kit, but the floor every layer writes into. |
| 1 | Rules | Answers that have one right value and that fig's own languages must give too: number spelling (`spellable`, `canonical`, `non_finite`), datetime classification, bare-literal sniffing (`literal.sniffBare`/`sniffNumber`), JSON-style string escaping (`json_string`). |
| 2 | Row writer | Write a node table in whatever order the parser meets nodes: rows linked by first child and next sibling, side facts by row, then `finish` into pre-order. Includes the by-key child index and the flat-map duplicate policy (`ChildIndex`, `flat_map.putEntry`: first place, last value, or refuse). |
| 3 | Tree | Nodes that hold their children and comments as fields (`mapping`, `sequence`, `entry`, `scalar`), laid out as rows by `rows(root)`. |
| 4 | Scanner | A byte cursor whose positions are already the wire's offsets: `eof`, `byte`, `starts`, `lit`, `match`, `find`, `hs`, `slice`, `fail`. Plus a token cursor over a token list, and the line helpers (`lines.zig`). |
| 5 | Grammar | Combinators whose primitives are layer 3's nodes (`seq`, `choice`, `opt`, `many`, `bare`, `quoted`, `comment`, `trivia`, `entry`, `map`, `sequence`, `document`, `sections`). A format becomes a description, and its spans, comment binding and duplicate policy follow from it. |
| 6 | Printer | Reading the table a `print` receives (`index(t)`: children, key/value, comments by slot), and a writer that knows indentation, lines and comment leaders. |
| 7 | XML | The lexical XML rules a plist-shaped format needs. |

Zig-internal pieces (`ascii`, `unicode`, `width`, `span`, `token`,
`yaml_schema`, `describe_json`, the AST/`Document` conversions) stay Zig's.
`yaml_schema` is YAML's own, and the rest are a language's stdlib in every
other binding.

## Where each binding stands

✓ has it · ◐ partial · ✗ missing. "native" is written in that language.
"bridged" is the Zig kit reached through the C ABI or `fig-kit.wasm`.

| Layer | Zig (`kit/`) | JS/TS (`@diaryx/fig/kit`) | Rust (`fig`) | C (`fig.h`) | Lua (fig-lua) | QuickJS (fig-quickjs) |
|---|---|---|---|---|---|---|
| 0 Contract | ✓ `contract.VTable`, `manifest` | ✓ `registerLanguage`, `serve` | ✓ `Language`, `register`, `helper::serve` | ✓ `FigLanguageVTable` | ✓ script table | ✓ module default export |
| 1 Rules: number, datetime | ✓ native | ✓ bridged (wasm) | ✓ bridged (`fig::kit`) | ✓ `fig_kit_*` | ◐ native twin, drifted | ◐ native twin, drifted |
| 1 Rules: literal, json_string | ✓ native | ◐ inside `G.quoted`, no escaper | ✗ | ✗ | ◐ inside grammar | ◐ inside grammar |
| 2 Row writer: any order + finish | ✓ `RowWriter` | ✗ (pre-order `t.row` only, so any-order goes through the tree) | ✗ (`NodeTable::push`, pre-order by hand) | ✗ (`fig_table_rows`, pre-order by hand; `FigRow` already has the link fields) | ✗ | ✗ |
| 2 Child index, duplicate policy | ✓ `ChildIndex`, `flat_map` | ◐ `Node.put` (policy on the tree) | ✗ | ✗ | ◐ `Node:put` | ◐ `Node.put` |
| 3 Tree | ◐ AST, used by printers rather than by parsers | ✓ | ✗ | ✗ (not wanted, see below) | ✓ | ✓ |
| 4 Scanner, cursor, lines | ◐ `lines`, `Token`/`Span`, no scanner | ✓ `Scanner`, `Cursor` | ✗ | ✗ (not wanted) | ✓ | ✓ |
| 5 Grammar | ✗ | ✓ `kit/grammar` | ✗ | ✗ (not wanted) | ✓ `grammar.lua` | ✓ vendored |
| 6 Printer: read the table | ✓ `Document`/AST via `convert` | ✓ `index(t)` | ✗ (raw `rows`) | ◐ `FigTableView`, links filled | ✓ (`t` arrives indexed) | ✓ |
| 6 Printer: writer | ✗ (each printer on `std.Io.Writer`) | ✓ `Writer` | ✗ | ✗ (not wanted) | ✓ | ✓ |
| 7 XML | ✓ token stream | ✓ grammar-shaped, native port | ✗ | ✗ | ✓ native | ✓ vendored |
| fig version | 6.0 | 6.0 | 6.0 | 6.0 | **4.1** | **4.1** |

## What parity means

Parity means that any layer a language author in that binding would
otherwise write themselves is in the kit. It does not mean the same code
everywhere. Each layer is written in the binding's idiom and answers the same
questions the same way.

**Decision 1: grammar combinators in Zig and Rust.**

- *Recommended:* both get layer 5. Rust gets a native port of `grammar.js`'s
  semantics: closures returning `Option<Node>`, with the same rule names, the
  same comment binding and the same messages' slots. Zig gets
  `kit/src/grammar.zig`: comptime rule values over a `Scanner`, writing
  through `RowWriter`, not a tree. That is Zig's idiom, and it allocates no
  node structs. A Rust author's dotenv then reads like the JS one. A Zig
  author can describe a simple format without a hand-written tokenizer, and
  fig's own languages stay hand-written, where speed matters.
- *Alternative:* Rust only. Zig's authors are mostly fig's own language
  authors, who write tokenizers anyway. That leaves Zig the one binding where
  a dotenv-like language is not a description, which is the gap this
  proposal exists to close.

**Decision 2: what C gets.** C gets only what a C author cannot easily write
and that must agree with fig:

- layer 1, complete: `fig_kit_literal_*` and `fig_kit_json_string_*`, beside
  the number and datetime rules;
- layer 2 as one function, **`fig_table_finish(FigTable *t, uint32_t root)`**,
  the Zig `RowWriter.finish` over the table fig hands a parse. `FigRow`
  already carries `first_child` and `next_sibling`. With this, a C parser
  writes rows in any order, links them, and finishes, with every side fact
  moved to its node's new row. That move is the part nobody should write
  twice. `fig_table_finish_in_order` is the cheap form for a parser already
  in pre-order. Optionally there is also `fig_table_child_by_key` (a linear
  scan C could write, but one that must compare keys the way fig does, text
  in the buffer or in the source).
- layer 7 as `fig_kit_xml_tokenize`, if Decision 4 says yes.

C gets no tree, scanner, grammar or writer. Pointer arithmetic is C's
scanner, and callbacks with a `ctx` pointer are not a grammar anyone would
choose. *Alternative:* expose `RowWriter`'s whole surface (`add`, `set`,
`set_next`, `anchor`, `tag`, …) as `fig_rows_*`. That is more ABI to freeze,
for little a C author cannot do with `fig_table_rows` plus `finish`.

**Decision 3: the row writer in JS, Lua and QuickJS.** These kits reach any
order through the tree (layer 3), which is the idiomatic route there.
*Recommended:* add `t.finish(root)` to the JS `Table` (and its Lua and
QuickJS twins), so a script that writes rows directly can write them in any
order too, with the same semantics and fixtures as Zig and C. It is about 60
lines and gives layer 2 one meaning everywhere. *Alternative:* leave it, and
count the tree as their layer 2.

**Decision 4: XML in Rust and C.** *Recommended:* yes, bridged. Expose the
Zig tokenizer as `fig_kit_xml_tokenize` (a token array of kind and span, plus
an error offset) and wrap it in `fig::kit::xml`. The JS, Lua and QuickJS
grammar-shaped XML stays native, held to the same fixtures. *Alternative:*
defer layer 7 outside Zig and JS. Only plist-shaped formats need it, and
none outside fig's own is known.

## Native or bridged

The rule is that **rules are bridged and structure is native.**

- **Rules (layer 1, 7) are one implementation, in Zig.** They have one right
  answer, and fig's own languages give it. A copy is a second opinion that
  will drift: fig-lua's and fig-quickjs's number and datetime twins already
  have. C and Rust call `fig_kit_*`. JS calls the same exports in
  `fig-kit.wasm`. QuickJS, which has no WebAssembly, answers `kit/zig.js`'s
  imports from Rust's `fig::kit` (the swap that file's comment already
  describes). Lua does the same through `mlua` functions. The rules work
  over byte slices and return small answers, so crossing the boundary is
  cheap.
- **Structure (layers 2 to 6) is native to each binding.** It deals in the
  binding's own values and closures: a Rust `NodeTable`, a JS object, a Lua
  table. A call across FFI per row or per byte would cost more than the code
  it saves. The exception is C, where the ABI *is* native, so
  `fig_table_finish` is the Zig code.

Per gap:

| Gap | How |
|---|---|
| Rust layers 2–6 (`fig::kit::{rows, tree, scan, grammar, print}`) | native, over `NodeTable` |
| Rust literal, json_string, xml | bridged: new `fig_kit_*`, wrapped in `fig::kit` |
| C literal, json_string, xml, `fig_table_finish` | bridged: Zig kit exported from libfig |
| JS literal, json_string | bridged: the same exports added to `fig-kit.wasm` and `kit/zig.js` |
| JS `Table.finish` | native |
| Zig scanner, grammar, writer | native: `kit/src/{scanner,grammar,writer}.zig` |
| Lua, QuickJS: number, datetime, literal, json_string | bridged through Rust `fig::kit`, replacing their own twins |
| QuickJS kit files | vendored verbatim from `@diaryx/fig/kit` by a script, with a CI check that they match the pinned fig's |
| Lua structure | native, as now, brought to the JS kit's current semantics |
| Lua, QuickJS on fig 6 | a port in their own repos (6.0 removed `FigFormat` and the bundled formats). It is a prerequisite for anything else there. |

## Tests: the same answers everywhere

Two mechanisms, and no layer is tested in only one binding.

**Shared fixtures, `kit/testdata/`.** Data only, read by every binding's
suite:

- `rules/{number,datetime,literal,json-string}.tsv`: an input, options and
  the expected answer, one case per line. The Zig kit test generates nothing.
  It reads the files like everyone else, so the fixtures are the spec, not a
  recording.
- `rows/*.json`: a row-writer script (rows added in a scrambled order,
  links, side facts, a root) and the pre-order table `finish` must give, in
  the helper wire's table JSON (`docs/helper-protocol.md`). Run by Zig's
  `RowWriter`, C's `fig_table_finish`, Rust's `fig::kit::rows`, and JS's,
  Lua's and QuickJS's `Table.finish`.
- `xml/*.xml` beside `*.tokens.json`, if Decision 4 is yes.
- `grammar/*`: small grammars' inputs and expected tables, for the rule
  semantics (comment binding, trivia, `bare`'s stop and trim, `quoted`'s
  escapes and `\r\n`) that a whole-language twin exercises only indirectly.
  Each binding states the same grammar in its own syntax.

The runners: Zig in `kit/`'s `zig build test`; C through a Zig test that
calls the exported functions; Rust in `tests/kit_fixtures.rs`
(`include_str!` across the workspace, which is not packaged); TypeScript in
`test/kit-fixtures.test.ts`. fig-lua and fig-quickjs copy `kit/testdata/`
at the fig tag they pin, with an `xtask` check that the copy matches.

**Twins, for layers 3 to 6 together.** Each binding writes a small
dotenv-like language with its kit, which is the last step of the work. Each
one is held to the Zig `dotenv`, row for row, over
`bindings/typescript/test/fixtures/dotenv/*.env` (moved to a shared
`languages/dotenv/testdata/` so it is not the TypeScript suite's alone):

- **Rust, JS, Lua, QuickJS:** served as helpers and run through
  `fig lang check <name> --against dotenv`. Rust's is
  `examples/kit_dotenv_helper.rs`, added to `tools/cli-lang-check.sh`. JS's
  is `js/dotenv.js`, which exists, now also run through the CLI. Lua and
  QuickJS use their existing `languages/dotenv.*`, ported.
- **Zig and C:** not processes, so they are registered in-process in a
  `zig build check` test. That test runs the same comparison `fig lang check`
  does. The comparison is factored out of `src/cli/languages.zig` into core,
  so the CLI and the test share it rather than agreeing by coincidence. The
  Zig twin is `kit/examples/dotenv.zig` (grammar-written). The C twin is
  `bindings/c/examples/dotenv.c` (`fig_table_rows` plus `fig_table_finish`
  plus the `fig_kit_*` rules).

## Release

6.0.0 is in the tree and untagged; the last tag is v5.2.0. Under
[VERSIONING](/docs/VERSIONING.md), everything here is **additive**: new Zig
modules, new `fig::kit` submodules, new `@diaryx/fig/kit` exports and
`Table.finish`, and new `fig_kit_*` and `fig_table_finish*` functions. An
appended C function moves neither `FIG_ABI_VERSION` nor the vtable version.
So it is a **minor** release whichever side of the tag it lands on, and
nothing here needs to hold the 6.0.0 tag.

- **Should land before the tag, if it is going to:** nothing is required.
  Two small things are cheaper before than after. (a) Fix the names of the
  `fig::kit` submodules now, so 6.0's `fig::kit::{number, datetime}` sit
  beside their siblings without a later rename. (b) Make sure nothing in
  6.0's `@diaryx/fig/kit` surface (`t.row`'s argument order, `Node.put`'s
  policy) needs to change to share fixtures with the other bindings. A change
  there after the tag is a major, so the rows fixtures should be written
  first and checked against JS before tagging.
- **After the tag, as 6.1.0:** every layer above, layer by layer.
- **Would be major (avoid):** changing `NodeTable`/`NodeRow`'s shape,
  `FigRow`'s layout, or the JS `Table`/tree API's existing semantics. The
  plan adds beside them and does not touch them. `NodeTable` and `NodeRow`
  are `#[non_exhaustive]`, so a field could be added, but none needs to be.
- **fig-lua and fig-quickjs** version on their own tracks. Moving them to
  `fig = "6"` changes what a script sees (the bundled formats are gone, and
  `fig_kit_*` replaces their own rules), so it is a major for each.

## Order of work

1. `kit/testdata/` fixtures for rules and rows, run by Zig and JS, which
   pins the meaning before anything else implements it.
2. The C exports (`fig_kit_literal_*`, `fig_kit_json_string_*`,
   `fig_table_finish*`), with `fig.h` comments, and the same exports in
   `fig-kit.wasm`.
3. Rust: `fig::kit` rules wrappers, then `rows`, `tree`, `scan`, `print`,
   `grammar`, each a commit with fixtures and `docs/rust.md`.
4. Zig: `scanner`, `writer`, `grammar`, with `docs/zig.md`.
5. JS: `Table.finish` and the new rule exports, with `docs/typescript.md`.
6. XML in C and Rust, if Decision 4 is yes.
7. The twins: Rust, Zig and C dotenv-likes, the shared dotenv fixtures, and
   the in-process `lang check` comparison. Then `languages/README.md` gets a
   "writing a language in each binding" table that points at them.
8. fig-lua and fig-quickjs: port to fig 6, bridge their rules, vendor the JS
   kit, and run the twins through `fig lang check`. These are separate
   sessions in their own repos.
