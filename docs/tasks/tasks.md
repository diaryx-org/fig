```fig
title = Tasks
description = Deferred work with a done state — a bug is a task with a repro
author = adammharris
created = 2026-09-04
updated = 2026-09-10
part_of = [docs](/docs/docs.md)
contents
> * [YAML `!!int`/`!!float` is applied without checking the lexeme, and `%TAG !!` is not honoured](yaml-int-tag-applied-without-checking-the-lexeme.md)
> * [A runtime target has no loss diagnostics](runtime-target-loss-diagnostics.md)
> * [`fig patch` into a runtime target is refused](patch-into-a-runtime-target.md)
> * [An entry appended after `export KEY=value` is indented to the key's column](dotenv-export-indents-an-appended-entry.md)
> * [The Rust editor spells a key through the format's printer, which for plist is `<string>k</string>`](rust-editor-spells-a-plist-key-through-the-printer.md)
```

# Tasks

Deferred work, one file per item, each with a `status` of `open`,
`in-progress`, `done` or `dropped`. `contents` above lists what is open;
closing a task is setting its status and naming the commit that resolved it,
not deleting the file. A bug is a task with a repro. Arguments for a change
that may lose are [proposals](/docs/proposals/proposals.md), not tasks.
