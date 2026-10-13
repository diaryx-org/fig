```fig
title = fig delete panics when its edit leaves the file unparseable
part_of = [Tasks](/docs/tasks/tasks.md)
description = The CLI unwraps `splicedText(config).?` on `InvalidEditText`, which is null for an edit with no text of the caller's, such as a delete, so such an edit crashes instead of reporting
status = open
created = 2026-10-13
```

# fig delete panics when its edit leaves the file unparseable

**Where.** `src/cli/main.zig`, in the `dispatch` error switch: both
`error.InvalidEditText` and the arm below it call
`types.splicedText(config).?`. `splicedText` is null for an operation that
splices no text the caller typed, and a delete is one. A delete whose
result does not parse returns `InvalidEditText` from `edit_ops`, and the
unwrap panics.

**Repro.** Reported while fixing
[the YAML block-list task](/docs/tasks/closed/yaml-block-list-emptied-to-null.md):
deleting the only item of a figl block list, or of a YAML list on its
parent's marker line, crashed. Both of those deletes now succeed, so a
current repro needs a delete that still leaves an unparseable file; the
code path is the same whatever the edit.

**Done when** an edit with no text of the caller's that leaves the file
unparseable reports that the edit was refused and nothing was written,
naming the path, and a check in `tools/cli-args-check.sh` holds it.
