```fig
title = Tasks
description = Deferred work with a done state — a bug is a task with a repro
author = adammharris
created = 2026-09-04
updated = 2026-09-22
part_of = [docs](/docs/docs.md)
contents
> * [The lossy strips and the `$fig` envelope rebuild the AST without its tags, anchors and directives](lossy-strips-drop-tags-and-anchors.md)
> * [The item and value renderers are not told the key they are rendering under](renderers-are-not-told-the-parent-key.md)
> * [YAML `!!int`/`!!float` is applied without checking the lexeme, and `%TAG !!` is not honoured](yaml-int-tag-applied-without-checking-the-lexeme.md)
> * [A runtime target has no loss diagnostics](runtime-target-loss-diagnostics.md)
> * [`fig patch` into a runtime target is refused](patch-into-a-runtime-target.md)
> * [An entry appended after `export KEY=value` is indented to the key's column](dotenv-export-indents-an-appended-entry.md)
> * [A mapping or list spliced into NestedText lands as a multi-line string](nestedtext-container-value-lands-as-a-string.md)
> * [A runtime printer is not told when it prints splice text, so it prints a whole document](/docs/tasks/runtime-printer-is-not-told-it-prints-splice-text.md)
> * [Closed tasks](/docs/tasks/closed/closed.md)
```

# Tasks

Deferred work, one file per item, each with a `status` of `open`,
`in-progress`, `done` or `dropped`. `contents` above lists every task, open
and closed — the index is the spine, and what is open is a view of it (`dx
tasks`); closing a task is setting its status and naming the commit that
resolved it, not deleting the file and not delisting it. A bug is a task with a repro. Arguments for a change
that may lose are [proposals](/docs/proposals/proposals.md), not tasks.
