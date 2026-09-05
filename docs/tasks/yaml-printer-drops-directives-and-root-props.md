```fig
title = YAML printer drops `%TAG` directives and root collection properties
description = `fig get -o yaml` and `fig fmt` lose a `%TAG` directive (so a `!e!foo` tag in the output no longer parses) and an anchor or tag on the root mapping/sequence itself (`&m` above the first key)
status = open
created = 2026-09-04
updated = 2026-09-04
part_of = [tasks](tasks.md)
```

# YAML printer drops `%TAG` directives and root collection properties

**Repro.** With `fig` at 0a0d646:

```
fig get testdata/yaml/accept/6CK3.yaml -o yaml
- !local foo
- !!str bar
- !e!tag%21 baz
fig check <that output>
error: UndefinedTagHandle
```

The source began `%TAG !e! tag:example.com,2000:app/` and `---`; the printer
writes the nodes with their verbatim tags but never the directive that
defines the `!e!` handle, so its own output does not parse. Z9M4 is the
same. These are the two `testdata/yaml/accept/` documents that fail the
conformance scoreboard's `reprint` ratchet (baseline 287 of 289).

Separately, a property on the root collection itself is not written:

```
printf '&m\na: b\n' | fig get -o yaml
a: b
```

The parser records the anchor on the mapping node (`node_anchors[root]`);
`printMapping`/`printSequence` write no props for the collection they
print, only `printKeyValue` does for a *value* collection (`key: &a`). A
sequence item that is a collection loses its props the same way
(`- &a\n  - x`).

**Done when** a `%TAG` directive the parser accepted is written back ahead
of `---` when any tag in the document uses its handle, a root collection's
anchor/tag is written on a line of its own above it (`&m\na: b`, or
`--- &m` — whichever the parser reads back to the same node), a collection
item's props are written after its `- `, and the `reprint` baseline in
`src/languages/yaml/conformance.zig` rises to 289. Found while closing
`yaml-printer-panics-on-accept-corpus.md`.
