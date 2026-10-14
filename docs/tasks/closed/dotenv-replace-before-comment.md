```fig
title = A dotenv value followed by a spaced comment cannot be replaced
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
description = Replacing an empty dotenv value followed by ` #c` drops the space before the `#`, so `A= #c` would read back as `42#c`; the read-back guard now refuses it
status = done
created = 2026-10-13
updated = 2026-10-14
```

# A dotenv value followed by a spaced comment cannot be replaced

**Status.** Done, in `fix(editor): replacing an empty value keeps a blank
before a trailing comment` (cc907ae, 2026-10-14). A replace into an empty
span that a trailing-comment opener follows writes a blank after the new
text: `A= #c` becomes `A= 42 #c`. `.properties` needed nothing, since its
`a= #c` is the value `#c` and is replaced whole. The fig-conformance cases
were not re-run from the change's session.

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
