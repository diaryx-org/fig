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
> * [Four deletes are refused because their splice takes another value](/docs/tasks/delete-splices-taking-other-values.md)
> * [The JavaScript YAML twin misreads compact sequences two deep](/docs/tasks/yaml-twin-deep-compact-sequences.md)
> * [Moving or reordering figl elements breaks + chains and index addresses](/docs/tasks/figl-move-chains-and-indices.md)
> * [A ZON enum-literal value cannot be replaced](/docs/tasks/zon-enum-literal-replace.md)
> * [A dotenv value followed by a spaced comment cannot be replaced](/docs/tasks/dotenv-replace-before-comment.md)
> * [Three YAML value shapes cannot be replaced](/docs/tasks/yaml-replace-refused-shapes.md)
```

# Tasks

Deferred work, one file per item, each with a `status` of `open`,
`in-progress`, `done` or `dropped`. `contents` above lists every task, open
and closed — the index is the spine, and what is open is a view of it (`dx
tasks`); closing a task is setting its status and naming the commit that
resolved it, not deleting the file and not delisting it. A bug is a task with a repro. Arguments for a change
that may lose are [proposals](/docs/proposals/proposals.md), not tasks.
