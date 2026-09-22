```fig
title = A mapping or list spliced into NestedText lands as a multi-line string
description = `fig patch` of `{"m": {"x": "1"}}` into a NestedText file, or a binding's `insert_value` of a map, writes `m:` over `> x: 1` — the editor's renderers take their value text as a string, which is right for a CLI argument and wrong for a container fragment
status = open
created = 2026-09-22
updated = 2026-09-22
part_of = [tasks](tasks.md)
```

# A mapping or list spliced into NestedText lands as a multi-line string

**Repro.**

```bash
$ printf 'name: fig\n' > c.nt
$ printf '{"m":{"x":"1","y":"2"},"l":["a","b"]}' > c.json
$ fig patch c.nt c.json && cat c.nt
name: fig
m:
    > x: 1
    > y: 2
l:
    > - a
    > - b
```

The document reads back with `m` as the string `"x: 1\ny: 2"`. The Rust
`insert_value` and TypeScript `insertValue` of a map or a list do the same,
through the same splice path.

**Why.** NestedText's `renderEntry`/`renderTail`/`renderItem` take their
value text as a STRING and block it when it has a line break — the right
reading of a CLI argument, since every NestedText scalar is a string and
`fig set f.nt k 'x: 1'` means the string. A container's splice text is block
text too (`x: 1\ny: 2`), and the renderers cannot tell the two apart. A
scalar's splice text became its plain text in the change that closed
[the NestedText value task](/docs/tasks/closed/rust-editor-spells-a-nestedtext-value-through-the-printer.md);
a container's has no such reading, because the text alone does not say
which it is.

**Done when** a mapping or list patched or inserted into NestedText lands
as nested entries or items under the key, a scalar string with a line
break still lands as a `>` block, and a test holds both.
