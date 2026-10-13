```fig
title = fig delete panics when its edit leaves the file unparseable
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
description = The CLI unwraps `splicedText(config).?` on `InvalidEditText`, which is null for an edit with no text of the caller's, such as a delete, so such an edit crashes instead of reporting
status = done
created = 2026-10-13
updated = 2026-10-13
```

# fig delete panics when its edit leaves the file unparseable

**Status.** Done, in `fix(cli): an edit with no text of the caller's that
leaves the file unparseable is reported, not a panic` (08fa574, 2026-10-13).
When an edit splices no text of the caller's, `main` reports through
`diag_report.reportUnparseableEdit`: it names the path and file, says
nothing was written, and exits 1. `tools/cli-args-check.sh` holds it with a
YAML delete of an anchor an alias still names, which panicked before.

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
