```fig
title = A dotenv value followed by a spaced comment cannot be replaced
part_of = [Tasks](/docs/tasks/tasks.md)
description = Replacing an empty dotenv value followed by ` #c` drops the space before the `#`, so `A= #c` would read back as `42#c`; the read-back guard now refuses it
status = open
created = 2026-10-13
```

# A dotenv value followed by a spaced comment cannot be replaced

**Repro.** On `main` at 5a1a559:

```sh
printf 'A= #c\nB=1\n' > e.env
fig replace e.env A 42        # refused: would not read back as that edit asks
```

Before 5a1a559 the edit reported success and wrote wrong data; the read-back guard now refuses it and leaves the file as it was. fig-conformance's recording (53fc34c) has these cases as refusals. Before, it wrote `A= 42#c`, whose value is `42#c`: a `#` is a comment only after whitespace.

**Done when** a replace keeps a separating space between the new value and
a following comment, in dotenv and `.properties` (whose `#` is a comment
only at a line's start, so check what its cases need), and the
corresponding fig-conformance cases succeed.
