```fig
title = Separated entries
description = Flow members joined by a separator a format names, a root that is a flow mapping with no delimiters, and a block delete that never takes a sibling's line — what CSS needs from the splice engine
author = adammharris
created = 2026-10-01
status = implemented
updated = 2026-10-01
part_of = [proposals](proposals.md)
```

# Separated entries

> **Status: IMPLEMENTED** on `main`, unreleased: the C ABI change is
> additive (three fields appended to `FigSyntax`, size-gated), so
> `semver-check` names 5.1.0. Written on branch `flow-entry-sep` against
> `main` at 7737847, for a CSS language in thorn (the drawing editor over
> twig's SVG), which edits the stylesheets and `style` attributes of SVGs it
> did not write; `bindings/rust/fig/tests/separated_entries.rs` is §2's
> table, passing. One thing beyond §3: `flow_map_pad` is not written into
> an empty open root, since there are no braces for it to stand inside.

## 1. The claim

The splice engine edits two shapes of container: a block one, one entry per
line, and a flow one, members between `{`/`}` or `[`/`]` joined by commas.
CSS has a third that is the second with one byte changed: a declaration
block, `rect { fill: none; stroke: #222 }`, is a flow mapping whose members
are joined by `;`; and a `style` attribute, `fill:red;stroke:blue`, is the
same mapping with no braces at all. Every edit the engine already makes to a
flow mapping — insert after the last member, keep a one-per-line layout,
delete one member and exactly one adjoining separator — is the edit CSS
wants, if the engine is told which byte the separator is.

Three `Syntax` fields say it, and one engine rule makes a block delete safe
for any format whose block entries can share a line.

## 2. What a runtime CSS gets today

A spike against fig 5.0.0 through the Rust carrier, a declaration list
parsed as a root mapping, `kv_sep = ": "`, `flow_containers = false`:

| edit | input | output |
|---|---|---|
| replace `stroke` | `fill:red;stroke:blue` | `fill:red;stroke:green` — right |
| rename `stroke` | `fill:red;stroke:blue` | `fill:red;color:blue` — right |
| delete `stroke` | `fill:red;stroke:blue` | `` — **the whole line, and `fill` with it** |
| insert `opacity` | `fill:red;stroke:blue` | `fill:red;stroke:blue\nopacity: 0.5\n` — reparses, as `stroke`'s value grown to `blue\nopacity: 0.5` |
| delete `stroke` | `fill:red;\n  stroke:blue;\n  opacity:1` | `fill:red;\n  opacity:1` — right |

Replace and rename are span splices and need nothing. Delete and insert are
the block path's line arithmetic, which is right only while every entry owns
its line. Declaring the list flow does not help: `isFlow` asks for a `{` or
`[` the list does not have, and `removeFlowItem` and the flow inserts write
and consume `,`.

The delete is the finding that matters beyond CSS. The block path deletes
the entry's lines whole, and nothing checks that no sibling shares them. No
compiled format produces such a table; a runtime one can, and the reparse
net does not catch it, because a document with one entry fewer still
parses.

## 3. The fields

**`flow_entry_sep`** — the bytes between two members of a flow container.
Default `,`. The flow inserts write it where they wrote `,`
(`, ` on one line, `,\n` and the members' indent on several); the flow
delete drops exactly one adjoining copy of it, as it drops one comma now;
`ownsItsLines` reads it as the one separator a line it owns may carry. CSS
declares `;`. A trailing separator, which CSS writes as often as not
(`fill:#FFF;`), is already what the engine expects of a format with
trailing commas: an insert splices after the last member's value, so
`a:1;` takes `b` as `a:1; b: 2;`.

**`flow_root`** — the document root is a flow mapping spelled with no
delimiters: members joined by `flow_entry_sep` from the first byte of the
source to the last. Default false. With it the root is flow without the
first-byte sniff; the first member of an empty root splices at the root's
start rather than past an opener; and a root is laid out on several lines
when its first and last members are on different lines, there being no
closing delimiter to ask. A CSS `style` attribute is the case; an HTTP
header's parameter list or a `key=value;key=value` connection string would
be others.

**`flow_maps_only`** — only a mapping opener (`{`, or `flow_map_open`) opens
a flow container; a `[` does not open a flow sequence. Default false. A CSS
stylesheet's root is a block of rules, and a sheet that opens with an
attribute selector — `[data-dash="dashed"] { … }` — would otherwise sniff
as a flow sequence.

In the C ABI the three are appended to `FigSyntax`, a NULL string or a zero
taking the default, so a language built against an older header — whose
`size` stops short of them — is read as it was. The Rust `Syntax`, the wire
and the TypeScript `wire.ts` gain the same three by the same names.

## 4. The rule

**A block delete takes only lines its entry owns.** Before deleting an
entry's lines, the engine checks whether a sibling ends on the line the
entry starts on, or starts on the line it ends on. If one does, it deletes
the entry's span and the horizontal whitespace after it, and nothing of its
neighbours or of a comment above, since a shared line has no comment block
of its own. Otherwise it deletes the lines, comment block included, as now.

No compiled format reaches the new branch: their block entries own their
lines. It is a behavioural change for a runtime format that produced a
shared line, which used to lose the sibling and now keeps it.

## 5. What a CSS language is then

Two dialects, each `kv_sep = ": "`, `flow_entry_sep = ";"`,
`flow_maps_only`, comments `/* */`:

- **`css`**, a stylesheet: a root mapping of rules, each a selector key over
  a declaration block — a flow mapping whose span includes its braces —
  and an at-rule with a block (`@media …`) a key over a mapping of rules.
  Block at the root: rules are separated by nothing but whitespace.
- **`css-declarations`**, a `style` attribute: `flow_root`, the declaration
  list.

That language belongs to its consumer, not to fig: thorn implements it in
Rust against `fig::language::Language`. What fig owes it is §3 and §4.

## 6. Out of scope

- **Editing inside an at-rule's block.** Its members are rules separated by
  whitespace, and `flow_entry_sep` is one value per dialect. A CSS language
  can make such a block read as block (its span starting at its first rule,
  not its `{`), which serves reading; inserting into a one-line `@media`
  block is not supported.
- **A separator that differs by container**, for the same reason. A format
  that needs it is the argument for a per-node separator in the node table.
- **Repeated keys.** CSS repeats a property as a fallback (`fill: red; fill:
  var(--ink)`) and a selector in a later rule. The tree holds both; a path
  names the first. That is fig's rule for every format.
