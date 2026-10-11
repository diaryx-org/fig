```fig
title = A comment appended after a .properties continuation joins the value
description = addDanglingComment on a document whose last line ends in a `\` continuation writes `# note` on the next line, which the continuation makes part of the last value
status = open
created = 2026-10-11
updated = 2026-10-11
part_of = [Tasks](/docs/tasks/tasks.md)
```

# A comment appended after a .properties continuation joins the value

**Repro.** The edit cases in fig-conformance
(`suites/properties/edits/continuations.json`, marked `suspect`), or:

```ts
using ed = Editor.open("trail=v\\\n", Format.Properties);
ed.addDanglingComment([], "note");
ed.source();                                  // "trail=v\\\n# note\n"
parse(ed.source(), Format.Properties).trail;  // "v# note", where it was "v"
```

**Cause.** The dangling comment is appended as a new last line. A line
ending in an odd number of backslashes continues onto the next one, so the
new line is read as the rest of `trail`'s value. The same holds for any
splice that writes a line directly after a continued one.

**Done when** a comment or entry written after a continued last line
cannot be joined into it (the continuation is ended first, or the edit is
refused), and the fig-conformance recording no longer marks the case
`suspect`.
