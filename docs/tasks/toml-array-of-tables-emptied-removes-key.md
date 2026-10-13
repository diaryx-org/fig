```fig
title = Deleting the only table of a TOML array of tables removes the key
part_of = [Tasks](/docs/tasks/tasks.md)
description = Deleting the only `[[x]]` table removes `x`, where every other TOML list emptied by a delete leaves `x = []`; the empty array has to go in the parent table's body, not where the header was
status = open
created = 2026-10-13
```

# Deleting the only table of a TOML array of tables removes the key

**Repro.** On `main` at dcc6617:

```sh
printf 'a = 1\n\n[[x]]\nb = 2\n' > t.toml
fig delete t.toml 'x[0]'     # exit 0
cat t.toml                   # a = 1
```

`x` is gone. An inline array (`x = [1]`) emptied the same way keeps
`x = []`, as YAML (9fc2b4f) and figl (dcc6617) now do for their block
lists, so the result of the same edit differs by spelling. With `[t]` and
`[[t.x]]`, deleting `t.x[0]` leaves `[t]` alone.

**Cause.** An array of tables has no line of its own to empty: each
element is a `[[x]]` header. `x = []` belongs in the parent table's own
body (the root's entries before any header, or `[t]`'s), which may sit far
from the deleted header, or may not exist yet.

**Done when** deleting the only element of an array of tables leaves
`x = []` in the parent table's body, keeping the header's comments where
they can go, or the decision that TOML removes the key is recorded here and
in `docs/zig.md`, with a test either way.
