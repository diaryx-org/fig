```fig
title = Tasks
description = Deferred work with a done state — a bug is a task with a repro
author = adammharris
created = 2026-09-04
updated = 2026-09-04
part_of = [docs](/docs/docs.md)
contents
> * [YAML printer panics on some accept-corpus documents](yaml-printer-panics-on-accept-corpus.md)
> * [Everything-on `zig build test` fails to compile](everything-on-test-build-fails.md)
```

# Tasks

Deferred work, one file per item, each with a `status` of `open`,
`in-progress`, `done` or `dropped`. `contents` above lists what is open;
closing a task is setting its status and naming the commit that resolved it,
not deleting the file. A bug is a task with a repro. Arguments for a change
that may lose are [proposals](/docs/proposals/proposals.md), not tasks.
