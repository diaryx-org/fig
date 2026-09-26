```fig
title = The item and value renderers are not told the key they are rendering under
description = `RenderItemFn` and `RenderValueFn` receive the text and the indent and nothing about where the fragment goes, so a format whose item or value spelling depends on the parent's name — an XML list, whose items are `<dependency>` under `<dependencies>` — cannot append or expand in place
status = done
created = 2026-09-14
updated = 2026-09-26
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# The item and value renderers are not told the key they are rendering under

**Status.** Done, in `feat!: every renderer takes one request, which
names the container its fragment goes into`, with fig-quickjs b77b075
and fig-lua 009593a beside it. Every renderer slot is one `FigRenderFn`
taking a `FigRenderRequest`: the indent, key, value, literal and old key
as before, and `parent_key` and `parent_tag`, the container the fragment
is written into. The engine fills them for all five renderers, not only
the item and value ones. `vtable_version` and `FIG_ABI_VERSION` are
bumped. `RenderArgs` in the Rust crate is non-exhaustive, with a setter
per field, so the next field is a minor change there too. `helper.rs`,
`wire.ts` and fig-quickjs's `wire.js` carry the two fields on the wire.
`pom.mjs` declares the item renderer, turns `block_seq_editable` back
on, and appends and prepends `<dependency>` to `<dependencies>`.

The empty container in the second paragraph needed nothing.
`expandEmptyContainer` runs only for a format that declares
`closed_containers`, whose tokens are fixed. `pom.mjs` declares none,
because its container spans sit between the tags and an edit never
rewrites them. An empty `<dependencies></dependencies>` reads as the
empty string, so the way to fill it is `set` with a tagged list, not an
append. A format whose container spans include name-dependent tags would
need a renderer for the open and close tokens. That would be a new
vtable slot, and nothing needs one yet.

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

**Scope, 2026-09-26.** Taken with the CLI's next major, so the renderer
slots change shape once rather than gaining two arguments: every
renderer takes one `FigRenderRequest` — dialect, indent, key, value,
literal, old key, and the container the fragment is written into (its
key's name and its tag) — which fig writes and a language reads, so a
field appended to it later is not a `vtable_version` bump, as
`PrintOptions` already is not.
