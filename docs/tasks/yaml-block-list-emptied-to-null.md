```fig
title = Deleting the last item of a YAML block list leaves the key null
part_of = [Tasks](/docs/tasks/tasks.md)
status = open
description = Deleting the only item of a YAML block list leaves `key:` (null), where a flow list, JSON and TOML all leave `[]`
created = 2026-10-12
updated = 2026-10-12
```

# Deleting the last item of a YAML block list leaves the key null

**Repro.**

```sh
printf 'links:\n  - a\nother: 1\n' >t.yaml
fig delete t.yaml 'links[0]'    # links:   ← null, not an empty list
printf 'links: [a]\n' >u.yaml
fig delete u.yaml 'links[0]'    # links: []
printf 'links = ["a"]\n' >v.toml
fig delete v.toml 'links[0]'    # links = []
```

The same happens nested (`x.links[0]`) and in Markdown frontmatter. Every
other shape keeps the list and empties it; a block list is the one that
turns into a different value, so a reader of the file sees `null` where the
key was a list a moment ago.

**Done when.** Deleting the last item of a block sequence leaves an empty
sequence, written `key: []` (YAML's only spelling for one), with the key's
comments kept, in a block list at any depth and in frontmatter.

**Why it matters now.** prov works around it: `MetaEditor::remove_item` in
`prov-store/src/edit.rs` (prov 05289f0) rewrites a null left behind by a
removal as `[]`. That can go once fig does it.
