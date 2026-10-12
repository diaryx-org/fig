```fig
title = A language cannot make its sequences editable when their items are not one per line or comma-joined
description = Append, prepend, delete and move refuse every sequence in a format like dottune's .tune, whose items are joined by `|`, by spaces or by blank lines, and no sequence anywhere can take an item at an index
status = done
created = 2026-10-11
updated = 2026-10-11
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# A language cannot make its sequences editable when their items are not one per line or comma-joined

**Status.** Done, in `feat: a sequence row names its own item separator, and insertItem inserts at an index`.
A sequence row carries `item_sep` (the bytes written between two items on
one line) and `open` (no delimiters), on the wire, in `FigNodeRow` and in
the Rust and TypeScript rows; core keeps them as `Document.node_item_seps`.
Such a sequence goes to the flow arm with its own separator whatever its
first byte, an open one's scan stays inside its span, and an item that
begins its line takes its comments with it. `insertItem(path, index,
text)` is `Editor.insertItem`, `fig_editor_insert_seq_item` and
`fig_embed_insert_seq_item`, Rust's `insert_item`, TypeScript's
`insertItem`/`insertItemRaw`, and `fig insert list[N]`. The repro table
passes against a copy of `fig-tune.mjs` that declares its separators and
drops `block_seq_editable = false`; the language tests in
`src/languages/runtime.zig` (`tinytune`), `bindings/rust/fig/tests/separated_sequences.rs`
and `bindings/typescript/test/language.test.ts` hold the shape.

**Repro.** dottune's `.tune` language (`tools/fig-tune.mjs` in dottune), through `@diaryx/fig` 5.0.0:

```tune
key C major

RH: 1 2 3 4 | 5 6 7 1' |
LH: I | V |

RH: 3 - - - |
```

| edit | path | result |
|---|---|---|
| append a bar | `appendValueRaw(["blocks", 0, "RH"], "1 - - -")` | `invalid argument` |
| prepend a bar | `prependValueRaw(["blocks", 0, "RH"], …)` | `invalid argument` |
| delete a bar | `deleteItem(["blocks", 0, "RH"], 0)` | `invalid argument` |
| move a bar | `moveItem(["blocks", 0, "RH"], 1, 0)` | `invalid argument` |
| append a token to a bar | `appendValueRaw(["blocks", 0, "RH", 0], "5")` | `invalid argument` |
| delete a token | `deleteItem(["blocks", 0, "RH", 0], 1)` | `invalid argument` |
| append a block | `appendValueRaw(["blocks"], "RH: 1 |")` | `invalid argument` |
| delete or move a block | `deleteItem(["blocks"], 1)`, `moveItem(["blocks"], 1, 0)` | `invalid argument` |
| add a voice | `insertValueRaw(["blocks", 0], "Bass", "1 | 5 |")` | right |
| delete a voice | `deleteKey(["blocks", 0, "LH"])` | right |

Mapping edits work. Every sequence edit is refused, because the language has
to declare `block_seq_editable = false`. Without that declaration it would be
worse: the block arm (`appendToSeq`, `prependToSeq`, `removeSeqItem`,
`reorderItems` in `src/editor.zig`) splices whole lines, and a tune's bars and
tokens share one line, so a delete would take the whole voice. As things are,
dottune's piano-roll editor can rewrite any token in place with
`replaceValueRaw`, but it cannot add or remove a bar or a block. It has to
fall back to text splices that it works out itself from the compiler's spans,
outside fig's reparse-and-roll-back.

**The shapes.** The engine edits two shapes of sequence: block, one item per
line, and flow, items between delimiters joined by `flow_entry_sep`.
[Separated entries](/docs/proposals/separated-entries.md) made the separator
a per-dialect byte and let the *root* drop its delimiters. A tune needs
three things that proposal set aside:

- **A separator per container.** Bars are joined by `|`, tokens in a bar or
  `[ ]` group by whitespace, and blocks by a blank line. The proposal names
  "a separator that differs by container" as out of scope, and says such a
  format is "the argument for a per-node separator in the node table". This
  is that format.
- **An undelimited sequence below the root.** A voice's bars run from after
  `Name:` to the end of the line, with no brackets. A bar's tokens have no
  brackets either. `flow_root` covers the root only.
- **An optional trailing separator.** `RH: 1 | 2 |` and `RH: 1 | 2` are the
  same line. An append after a trailing `|` should write `3 |`, not `| 3`.

**And an index.** `insertItem(path, index, text)` exists in no shape: the API
offers append, prepend, delete and move, and nothing else. "Insert a bar
after bar 2" is an append and a `moveItem` today, and in a format where
`moveItem` is refused it cannot be done at all. A scalar insert at an index
is the obvious twin of `removeSeqItem(path, index)` for flow and block alike.

**Done when.**

- A language can say, per sequence node (a row field beside `sep`, or a
  separator the parse emits per container), what joins its items and
  whether the container has delimiters.
- Append, prepend, insert at an index, delete and move work on such a
  sequence. A delete takes exactly one adjoining separator, and an append
  keeps a trailing separator trailing.
- The reparse net still rolls back an edit the language rejects.
- dottune's repro table above passes once the language declares its
  separators and drops `block_seq_editable = false`.
- `insertItem` is in the Zig, C, Rust and TypeScript editors.

**Where.** `src/editor.zig`: `appendToSeq`, `prependToSeq`, `removeSeqItem`
and `reorderItems`, and `isFlowNode`, which decides between the arms. The flow
arm's `insertFlowItem`, `prependFlowItem` and `removeFlowItem` already write
and consume a separator, `flow_entry_sep`, and would take a per-node one. The
row field goes in the wire format (`src/languages/wire.zig`) and the runtime
language interface (`src/languages/runtime.zig`). The rule in separated
entries §4, that a block delete takes only lines its entry owns, already
stops a shared-line delete from taking a neighbour. It does not make the edit
right.
