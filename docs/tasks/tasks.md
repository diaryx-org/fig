```fig
title = Tasks
description = Deferred work with a done state — a bug is a task with a repro
author = adammharris
created = 2026-09-04
updated = 2026-09-07
part_of = [docs](/docs/docs.md)
contents
> * [YAML `!!int`/`!!float` is applied without checking the lexeme, and `%TAG !!` is not honoured](yaml-int-tag-applied-without-checking-the-lexeme.md)
```

# Tasks

Deferred work, one file per item, each with a `status` of `open`,
`in-progress`, `done` or `dropped`. `contents` above lists what is open;
closing a task is setting its status and naming the commit that resolved it,
not deleting the file. A bug is a task with a repro. Arguments for a change
that may lose are [proposals](/docs/proposals/proposals.md), not tasks.
