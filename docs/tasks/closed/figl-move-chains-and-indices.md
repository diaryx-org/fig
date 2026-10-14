```fig
title = Moving or reordering figl elements breaks + chains and index addresses
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
description = moveItem and reorderItems on a figl list written as a `+` chain or with index addresses either change other values (a `+` moved under the wrong header appends a container) or fail to parse, where they should rearrange the elements and re-spell headers and indices as deletes now do (6621ef0)
status = done
created = 2026-10-13
updated = 2026-10-14
```

# Moving or reordering figl elements breaks + chains and index addresses

**Status.** Done, in `fix(editor): a figl move or reorder re-spells each
element's header, + and index for its new place` (2026-10-14). A block
sequence's move and reorder write, in the same splice, each moved
element's `index` mentions as its new index, wherever the line is, and its
`append` mention as the one the position it lands in had (`reorderEdits`),
so the first of a chain carries the header and the rest a `+`, each
element keeping its trailing comment in its column.

**Repro.** On `main` at 2f7f865, through the editor API:

```
c[]
> n = a
c[].p[]
> x = 1
+
> x = 2
```

reads `{"c":[{"n":"a","p":[{"x":1},{"x":2}]}]}`. `moveItem(["c",0,"p"], 0, 1)`
reports success and writes `c[]` / `> n = a` / `+` / `> x = 2` / `c[].p[]` /
`> x = 1`, which reads `{"c":[{"n":"a"},{"x":2,"p":[{"x":1}]}]}`: the `+`
now re-runs `c[]` and appends a container. fig-conformance recorded the
kitchen-sink case `spec.containers.[0].ports: move first item last` as this
success. Moves and reorders of index-addressed elements (`xs[0]`, `ys`,
`clusters` in index-addressing.figl and kitchen-sink.figl) are refused with
a parse error.

**Cause.** A move splices an element's lines elsewhere as they are written,
and in figl how an element is written depends on where it is: the first of
a chain carries the `[]` header and the rest a `+`, and an index address
names a position. The delete path learned this in 6621ef0
(`followingEdits`, from the `index` and `append` mentions of 07b3b1b); the
move path did not. A read-back guard on moves (in progress alongside this
task) turns the corruption into a refusal; this task makes the move work.

**Done when** moving or reordering elements of a `+` chain, of a nested
header chain, and of index-addressed elements re-spells headers, `+` and
indices so the result reads back as exactly the requested order, keeping
each element's comments, with tests in `languages/figl/tests/editor.zig`.
