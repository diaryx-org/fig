```fig
title = Tasks
description = Deferred work with a done state — a bug is a task with a repro
author = adammharris
created = 2026-09-04
updated = 2026-10-26
part_of = [docs](/docs/docs.md)
contents
> * [Closed tasks](/docs/tasks/closed/closed.md)
> * [Four deletes are refused because their splice takes another value](/docs/tasks/delete-splices-taking-other-values.md)
> * [The JavaScript YAML twin misreads compact sequences two deep](/docs/tasks/yaml-twin-deep-compact-sequences.md)
> * [Renaming a figl container written over several lines renames only its first line](/docs/tasks/figl-rename-multi-line-container.md)
> * [Renaming a YAML flow key written against its colon makes another key](/docs/tasks/yaml-rename-flow-key-against-colon.md)
> * [A leading BOM hides a YAML document's first key from the editor](/docs/tasks/yaml-bom-first-key.md)
> * [Deleting or renaming a duplicated key leaves the key readable](/docs/tasks/duplicate-key-delete-and-rename.md)
> * [The bindings and the JavaScript twins keep a replaced string's quote style](/docs/tasks/bindings-keep-quote-style.md)
> * [check-figl runs a fig that cargo did not build](/docs/tasks/check-figl-ignores-cargo-target-dir.md)
> * [A block comment after the root's closing bracket moves inside it](/docs/tasks/canonical-root-block-comment-moves-inside.md)
> * [ship ci builds fig-wasi without the wasm32-wasip1 target](/docs/tasks/xtask-ci-builds-fig-wasi-without-its-target.md)
```

# Tasks

Deferred work, one file per item, each with a `status` of `open`,
`in-progress`, `done` or `dropped`. `contents` above lists every task, open
and closed — the index is the spine, and what is open is a view of it (`org
tasks`); closing a task is setting its status and naming the commit that
resolved it, not deleting the file and not delisting it. A bug is a task with a repro. Arguments for a change
that may lose are [proposals](/docs/proposals/proposals.md), not tasks.
