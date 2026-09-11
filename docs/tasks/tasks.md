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
> * [Editor: read and write the dangling anchor, and comment a node out and back in](dangling-comments-and-comment-out-ops.md)
> * [Everything-on `zig build test` fails to compile](everything-on-test-build-fails.md)
> * [A flow sequence item reports, and deletes, its parent's leading comment](flow-item-leading-comment-is-the-parents.md)
> * [JSON printer emits a non-string mapping key as invalid JSON](json-printer-emits-non-string-keys.md)
> * [YAML printer drops `%TAG` directives and root collection properties](yaml-printer-drops-directives-and-root-props.md)
> * [YAML printer panics on some accept-corpus documents](yaml-printer-panics-on-accept-corpus.md)
```

# Tasks

Deferred work, one file per item, each with a `status` of `open`,
`in-progress`, `done` or `dropped`. `contents` above lists every task, open
and closed — the index is the spine, and what is open is a view of it (`dx
tasks`); closing a task is setting its status and naming the commit that
resolved it, not deleting the file and not delisting it. A bug is a task with a repro. Arguments for a change
that may lose are [proposals](/docs/proposals/proposals.md), not tasks.
