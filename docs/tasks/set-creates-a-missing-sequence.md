```fig
title = `set` cannot append to a sequence that is not there yet
description = `set` makes missing parent maps but not a missing list, so `set f 'tags[-]' x` fails where `set f tags.x 1` succeeds; `set --seq` and `insert` cannot start a list either
status = open
created = 2026-10-11
updated = 2026-10-11
part_of = [Tasks](/docs/tasks/tasks.md)
```

# `set` cannot append to a sequence that is not there yet

**Repro.**

```sh
printf 'a: 1\n' >t.yaml
fig set t.yaml 'tags[-]' x      # error: no such path in the document
fig set --seq t.yaml tags x     # exit 1, nothing written
fig insert t.yaml 'tags[-]' x   # exit 1
fig set new.yaml 'tags[-]' x    # error: a segment of this path is not a mapping, …
```

`set` is the upsert, and it already makes every missing parent map along a
path (`mkdir -p`). A missing list is the one container it will not make, so
the only way to start one is `set t.yaml tags '[x]'`, which an agent or a
script has to know to reach for. The last line's message is also wrong
about the cause: nothing in an empty new file is a scalar.

**Done when.** `set f 'list[-]' x` and `set --seq f list x …` create `list`
when it is absent, in every format with a spelling for an empty sequence,
and a format without one (INI, dotenv, `.properties`) refuses with a message
that says so. The vivify follows `Editor.set`'s: an absent key or a null
stands in for the list, and a scalar there is a type error, never clobbered.

**Where.** `runSet` in `src/cli/actions.zig` turns a trailing `[-]` into
`append_seq` on the parent path. The seed wants an `empty_seq_literal`
beside `Syntax.empty_map_literal`, and the seed-then-append has to be
atomic the way `Editor.set`'s vivify-then-land is.
