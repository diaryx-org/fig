```fig
title = Tasks
description = Deferred work with a done state — a bug is a task with a repro
author = adammharris
created = 2026-09-04
updated = 2026-10-12
part_of = [docs](/docs/docs.md)
contents
> * [Closed tasks](/docs/tasks/closed/closed.md)
> * [The Rust helper reads JSON with fig's json format](/docs/tasks/helper-serde-json.md)
> * [A comment after a figl multi-line string's opening quotes is invisible to the editor](/docs/tasks/figl-multiline-string-opener-comment.md)
> * [Deleting the first element of a figl + chain is refused](/docs/tasks/figl-continuation-chain-first-delete.md)
> * [Deleting the only table of a TOML array of tables removes the key](/docs/tasks/toml-array-of-tables-emptied-removes-key.md)
> * [A YAML plain scalar's continuation line that starts with a dash reads as a nested list](/docs/tasks/yaml-plain-continuation-dash.md)
```

# Tasks

Deferred work, one file per item, each with a `status` of `open`,
`in-progress`, `done` or `dropped`. `contents` above lists every task, open
and closed — the index is the spine, and what is open is a view of it (`dx
tasks`); closing a task is setting its status and naming the commit that
resolved it, not deleting the file and not delisting it. A bug is a task with a repro. Arguments for a change
that may lose are [proposals](/docs/proposals/proposals.md), not tasks.
