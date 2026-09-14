```fig
title = The item and value renderers are not told the key they are rendering under
description = `RenderItemFn` and `RenderValueFn` receive the text and the indent and nothing about where the fragment goes, so a format whose item or value spelling depends on the parent's name — an XML list, whose items are `<dependency>` under `<dependencies>` — cannot append or expand in place
status = open
created = 2026-09-14
updated = 2026-09-14
part_of = [tasks](tasks.md)
```

# The item and value renderers are not told the key they are rendering under

**What happens.** fig-quickjs's `pom.mjs` reads `<dependencies>` holding
two `<dependency>` elements as a sequence, and records the item element's
name on the sequence row's tag (`!dependency`) so its printer can spell
the items back. The editor cannot: `appendSeqItem` renders the new item
through `render_item(ctx, dialect, indent, value)`, and the name the item
element needs is not among the arguments — nor is the sequence's tag, nor
its key. The module declares `block_seq_editable: false` and every list
in a POM is read-only in place. `closed_containers` has the same shape of
gap one level up: `expandEmptyContainer` writes fixed `map_open`/`map_close`
tokens, and an XML element's are `<name>`/`</name>`.

**Where.** `languages/runtime.zig`: `RenderValueFn`, `RenderItemFn`, and
the `render` request the wire carries (`indent`, `key`, `value`,
`literal`, `old_key`); `editor.zig`'s `writeItem` and `renderedValue`,
which have the parent node in hand and pass none of it. `render_entry`
already takes `key`; the item and value renderers are the two that do
not, and `key` is already a field of `RenderArgs` on every binding.

**Done when** the item and value renderers receive the parent entry's key
text (empty at the root) and, for a sequence, the parent row's tag —
appended to the request, `vtable_version` bumped, the wire's
`RenderArgs` documented in `helper.rs`, `wire.ts` and fig-quickjs's
`wire.js` — and `pom.mjs` can turn `block_seq_editable` back on and
append `<dependency>` to `<dependencies>`.
