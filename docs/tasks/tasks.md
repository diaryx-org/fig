```fig
title = Tasks
description = Deferred work with a done state — a bug is a task with a repro
author = adammharris
created = 2026-09-04
updated = 2026-09-14
part_of = [docs](/docs/docs.md)
contents
> * [An insert into a section with no entries of its own lands inside its first child](insert-into-a-section-with-no-own-entries-lands-in-its-first-child.md)
> * [`set` falls back to an insert on any replace error, so a refused replace becomes a duplicate entry](set-falls-back-to-an-insert-on-any-replace-error.md)
> * [The lossy strips and the `$fig` envelope rebuild the AST without its tags, anchors and directives](lossy-strips-drop-tags-and-anchors.md)
> * [The item and value renderers are not told the key they are rendering under](renderers-are-not-told-the-parent-key.md)
> * [`indentAt` pads to the anchor token's column, which is wrong when the anchor is a value or a header key](indent-at-pads-to-the-anchor-column.md)
> * [`fig delete` never reaches `deleteContainer`, so a section, table or block cannot be deleted from the command line](cli-delete-never-reaches-delete-container.md)
> * [A command-line path cannot name a key that contains a `.` or a `[`](cli-path-cannot-quote-a-dotted-key.md)
> * [YAML `!!int`/`!!float` is applied without checking the lexeme, and `%TAG !!` is not honoured](yaml-int-tag-applied-without-checking-the-lexeme.md)
> * [A runtime target has no loss diagnostics](runtime-target-loss-diagnostics.md)
> * [`fig patch` into a runtime target is refused](patch-into-a-runtime-target.md)
> * [An entry appended after `export KEY=value` is indented to the key's column](dotenv-export-indents-an-appended-entry.md)
> * [The Rust editor spells a key through the format's printer, which for plist is `<string>k</string>`](rust-editor-spells-a-plist-key-through-the-printer.md)
> * [The Rust and TypeScript editors spell a NestedText value through the printer, which is a `>` block](rust-editor-spells-a-nestedtext-value-through-the-printer.md)
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
