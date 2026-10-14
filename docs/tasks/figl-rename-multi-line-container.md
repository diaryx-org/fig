```fig
title = Renaming a figl container written over several lines renames only its first line
part_of = [Tasks](/docs/tasks/tasks.md)
description = `renameKey`/`renameContainer` on a figl container spread over dotted keys or repeated headers rewrites only the first line that names it, so the rest stays under the old name (20 fig-conformance edit cases)
status = open
created = 2026-10-14
updated = 2026-10-14
```

# Renaming a figl container written over several lines renames only its first line

**Repro.** At eca3f34, with the CLI:

```sh
printf 'a.b.c = 1\na.b.d = 2\n' > d.figl
fig rename d.figl a renamed      # renamed.b.c = 1 / a.b.d = 2
fig get d.figl -o json           # {"renamed":{"b":{"c":1}},"a":{"b":{"d":2}}}

printf 'jobs.test.steps[]\n> uses = x\njobs.test.steps[]\n> run = y\n' > h.figl
fig rename h.figl jobs renamed   # only the first header becomes renamed.test.steps[]
```

`a` is split in two: one half under the new name, one under the old.
`renameContainer` does the same, though its doc says it rewrites "every
line that names it". TOML is right in both shapes: `a.b.c` / `a.b.d` and
`[a.b]` / `[a.d]` rename whole.

fig-conformance (66be227) marks the 20 cases this makes `suspect`: every
rename and rename-container of `a`, `a.b`, `list` and `list.of` in
dotted-keys.figl, of `jobs` and `jobs.test` in append-headers.figl
(`jobs.test.steps` too), and of `jobs`, `jobs.test`, `cache.redis` and
`clusters` in kitchen-sink.figl.

**Done when** renaming a figl key or container rewrites every line, dotted
key and header that names it, so the document reads back with one
container under the new name and none under the old, and fig-conformance
re-records those 20 cases as faithful, with no `suspect` mark.
