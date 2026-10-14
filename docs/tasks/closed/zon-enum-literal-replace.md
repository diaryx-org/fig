```fig
title = A ZON enum-literal value cannot be replaced
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
description = Replacing `.a = .fast` with any value writes after the enum literal's `.`, because the value's span starts past it, so `true` becomes the enum literal `.true`; the read-back guard now refuses it
status = done
created = 2026-10-13
updated = 2026-10-14
```

# A ZON enum-literal value cannot be replaced

**Status.** Done, in `fix(zon): an enum literal's span covers its leading
dot, so a replace takes the whole literal` (1a8407b, 2026-10-14). The
parser starts an enum literal's span at its `.` and keeps the bare name as
its text, and the JavaScript twin does the same, so `fig replace z.zon a
true` writes `.a = true`. The fig-conformance cases were not re-run from
the change's session.

**Repro.** On `main` at 5a1a559:

```sh
printf '.{ .a = .fast, .b = 1 }\n' > z.zon
fig replace z.zon a true      # refused: would not read back as that edit asks
```

Before 5a1a559 the edit reported success and wrote wrong data; the read-back guard now refuses it and leaves the file as it was. fig-conformance's recording (53fc34c) has these cases as refusals. Before, it wrote `.a = .true`.

**Cause.** An enum-literal value's span starts at its name, after the `.`,
so a replace splices into `.fast` past the dot and leaves it in front of
the new text.

**Done when** the span (or the splice) covers the `.`, a replace of an
enum-literal value with any kind writes it plainly (`.a = true`), and
`names.zon` and `everything.zon`'s replace cases in fig-conformance succeed.
