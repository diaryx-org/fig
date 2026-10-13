```fig
title = An edit to a compact fig object writes commas and spaced `=`
description = The splice engine joins flow members with `Syntax.flow_entry_sep` and pads an empty object with `flow_map_pad`, so inserting into `{a=1}` writes `{a=1, b = 2}` — valid, but not what `fig fmt` writes
status = done
created = 2026-10-09
updated = 2026-10-13
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# An edit to a compact fig object writes commas and spaced `=`

**Status.** Done, in `fix(editor): an insert into a compact fig object
writes it as fmt does` (40c1175, 2026-10-13). `Syntax` gained
`flow_map_kv_sep` and `flow_map_entry_sep`; an insert copies the joint
between an object's first two members when they share a line, and otherwise
writes figl's compact form, so `{a=1}` plus `b` is `{a=1 b=2}`.

**Repro.** With compact objects ([proposal](/docs/proposals/compact-objects.md)):

```
fig set doc.figl p.b 2      # doc.figl: p = {a=1}
# → p = {a=1, b=2}          # fmt writes p = {a=1 b=2}

fig set doc.figl q.a 1      # doc.figl: q = {}
# → q = { a = 1 }           # fmt writes q = {a=1}
```

Both results parse to the right value — a compact object still accepts
commas and spaces around `=` — so nothing is lost. They are just not the
canonical form, and an object edited twice mixes separators
(`{a=1 c=3, b=2}`).

**Cause.** `insertFlowMapEntry` and `removeFlowItem` (`src/editor.zig`) take
the member separator from `Syntax.flow_entry_sep`, one value per language,
shared by arrays and objects. fig needs `,` between array members and a space
between a compact object's pairs, and a JSON-spelled object inside a fig
document still wants `,`. `flow_kv_sep_from_siblings` already copies the
key/value separator from an object's first entry; the empty-object case
falls back to `kv_sep` (` = `) and `flow_map_pad` (` `).

**Done when** an insert into, and a delete from, a compact object leaves it
in the form `fig fmt` writes, a JSON-spelled object in a fig document keeps
its commas, and an array is unchanged. The likely shape is a sibling-copy
rule for the member separator alongside `flow_kv_sep_from_siblings`, plus a
flow-specific separator for an empty object — a `Syntax` addition, which is
additive at the C ABI (`FigSyntax` is size-gated).
