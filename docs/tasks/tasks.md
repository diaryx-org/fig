```fig
title = Tasks
description = Deferred work with a done state — a bug is a task with a repro
author = adammharris
created = 2026-09-04
updated = 2026-10-11
part_of = [docs](/docs/docs.md)
contents
> * [A source build of fig-sys on a Mac targets the build machine's macOS](/docs/tasks/source-build-deployment-target.md)
> * [An infinite or NaN float written to the fig dialect reads back as a string](/docs/tasks/fig-dialect-non-finite-floats.md)
> * [An edit to a compact fig object writes commas and spaced `=`](/docs/tasks/compact-object-editor-separator.md)
> * [The tree-sitter grammar reads a fig object as comma-separated](/docs/tasks/tree-sitter-compact-objects.md)
> * [A trailing comment on a CRLF line is spliced between the CR and the LF](/docs/tasks/trailing-comment-splits-crlf.md)
> * [A trailing comment on a ZON multiline string becomes part of the string](/docs/tasks/zon-trailing-comment-in-multiline-string.md)
> * [A comment appended after a .properties continuation joins the value](/docs/tasks/properties-comment-after-continuation.md)
> * [`set` cannot append to a sequence that is not there yet](/docs/tasks/set-creates-a-missing-sequence.md)
> * [Compiled TOML 1.0, YAML 1.1 and YAML streams cannot be scored by fig-conformance](/docs/tasks/compiled-dialects-unreachable-by-conformance.md)
> * [A language cannot make its sequences editable when their items are not one per line or comma-joined](/docs/tasks/sequences-with-a-language-separator.md)
> * [Closed tasks](/docs/tasks/closed/closed.md)
```

# Tasks

Deferred work, one file per item, each with a `status` of `open`,
`in-progress`, `done` or `dropped`. `contents` above lists every task, open
and closed — the index is the spine, and what is open is a view of it (`dx
tasks`); closing a task is setting its status and naming the commit that
resolved it, not deleting the file and not delisting it. A bug is a task with a repro. Arguments for a change
that may lose are [proposals](/docs/proposals/proposals.md), not tasks.
