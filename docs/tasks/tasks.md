```fig
title = Tasks
description = Deferred work with a done state — a bug is a task with a repro
author = adammharris
created = 2026-09-04
updated = 2026-09-07
part_of = [docs](/docs/docs.md)
contents
> * [JSON printer emits a non-string mapping key as invalid JSON](json-printer-emits-non-string-keys.md)
> * [YAML printer drops `%TAG` directives and root collection properties](yaml-printer-drops-directives-and-root-props.md)
> * [Editor: read and write the dangling anchor, and comment a node out and back in](dangling-comments-and-comment-out-ops.md)
> * [A flow sequence item reports, and deletes, its parent's leading comment](flow-item-leading-comment-is-the-parents.md)
```

# Tasks

Deferred work, one file per item, each with a `status` of `open`,
`in-progress`, `done` or `dropped`. `contents` above lists what is open;
closing a task is setting its status and naming the commit that resolved it,
not deleting the file. A bug is a task with a repro. Arguments for a change
that may lose are [proposals](/docs/proposals/proposals.md), not tasks.
