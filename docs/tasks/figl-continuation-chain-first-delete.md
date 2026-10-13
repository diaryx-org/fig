```fig
title = Deleting the first element of a figl + chain is refused
part_of = [Tasks](/docs/tasks/tasks.md)
description = Deleting `r[0]` from `r[]` … `+` … leaves the `+` with no append header to re-run, so the edit does not parse and is rolled back; the next element should take the header
status = open
created = 2026-10-13
```

# Deleting the first element of a figl + chain is refused

**Repro.** On `main` at dcc6617:

```sh
printf 'r[]\n> file = a\n+\n> file = b\n' > p.figl
fig delete p.figl 'r[0]'
# error: the edit at `r[0]` in p.figl was refused: the file would not parse after it
```

The file is left as it was (08fa574 reports the refusal; before it the
CLI panicked). Deleting a later element, or the only one (dcc6617), works.

**Cause.** The first element owns the `r[]` header line, and the line
delete takes it; what follows starts with `+`, which needs a preceding
`[]` header (spec § 8, `FigDanglingContinuation`).

**Done when** deleting the first element of a `+` chain rewrites the next
element's `+` as the header (`r[]`), keeping both elements' comments, and a
test in `languages/figl/tests/editor.zig` covers a two- and a three-element
chain.
