```fig
title = An insert into a figl element with a nested header lands in the wrong element
part_of = [Tasks](/docs/tasks/tasks.md)
description = Inserting a key into a figl element whose body continues under a nested header (`c[]` / `> n = a` / `c[].p[]` / `> x = 1`) appends it after the nested element's lines, so it lands in the port, not the container; the read-back guard now refuses it
status = open
created = 2026-10-13
```

# An insert into a figl element with a nested header lands in the wrong element

**Repro.** fig-conformance's `append-headers.figl` and `kitchen-sink.figl`,
`spec.containers.[0]: insert …`: the insert was recorded as
`> containerPort = 80` / `> added_bool = true` at the end of the element,
which is inside the last `ports[]` element. Before 5a1a559 the edit reported success and wrote wrong data; the read-back guard now refuses it and leaves the file as it was. fig-conformance's recording (53fc34c) has these cases as refusals.

**Cause.** The insert appends after the element's last line, and in figl an
element's own keys must come before a nested `[]` header that opens a
child of it; a `> key` after it belongs to the child.

**Done when** an insert into such an element goes among its own keys, before
its first nested header (or, if it has none of its own, right after its
header line), and the fig-conformance cases succeed. This is close to
[moving elements](/docs/tasks/figl-move-chains-and-indices.md), which needs
the same knowledge of where an element's own lines end.
