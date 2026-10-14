```fig
title = Renaming a figl container written over several lines renames only its first line
part_of = [Tasks](/docs/tasks/tasks.md)
description = `renameKey`/`renameContainer` on a figl container spread over dotted keys or repeated headers rewrites only the first line that names it, so the rest stays under the old name (20 fig-conformance edit cases)
status = done
created = 2026-10-14
updated = 2026-10-14
```

# Renaming a figl container written over several lines renames only its first line

**Status.** Done, in `fix(figl): a key that re-opens a container mid-path is
a mention of it` (2026-10-14). The parser recorded a name mention for the
key that made a container and for a header's final key that re-opened one,
but not for a key that re-opened one mid-path: in `a.b.d = 2` after `a.b.c =
1`, `a` and `b` were no mentions, so the generic rename, which rewrites
mentions, never reached them. Every such key is now one, except on a `+`
line, whose steps are the header it re-runs. The JavaScript twin had the
same gap and records them too. fig-conformance's figl edits go from 6489 to
6509 faithful of 6509; besides the 20 cases below, eight renames recorded as
`ParseError` (of `spec` and `spec.containers` in append-headers.figl and
kitchen-sink.figl, and of `xs` and `ys` in index-addressing.figl) now rename
whole. The suite still has to re-record those 28 cases.

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
