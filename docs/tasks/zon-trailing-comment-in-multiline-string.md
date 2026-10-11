```fig
title = A trailing comment on a ZON multiline string becomes part of the string
description = setTrailingComment appends `// note` to the last `\\` line, and a `\\` line runs to its end, so the comment is read back as string text
status = open
created = 2026-10-11
updated = 2026-10-11
part_of = [Tasks](/docs/tasks/tasks.md)
```

# A trailing comment on a ZON multiline string becomes part of the string

**Repro.** The edit cases in fig-conformance (`suites/zon/edits/strings.json`
and `everything.json`, marked `suspect`), or from the CLI:

```sh
printf '.{\n    .ml =\n        \\\\two\n    ,\n}\n' > m.zon
fig comment --inline m.zon ml note
# m.zon's string line is now   \\two // note
fig get m.zon ml             # "two // note", where it was "two"
```

The edit parses, so nothing refuses it, and the value changes.

**Cause.** The editor puts a trailing comment after the last byte of the
value's span on the same line. A `\\` line has no end delimiter: everything
up to the newline is the string. There is no place on that line for a
comment.

**Done when** a trailing comment on a multiline string value goes where
it cannot be string text (after the `,` that ends the entry, on the line
below), or the op is refused for that value, and the fig-conformance
recording no longer marks these cases `suspect`.
