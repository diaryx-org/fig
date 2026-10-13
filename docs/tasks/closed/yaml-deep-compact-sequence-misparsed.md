```fig
title = YAML compact sequences two deep misplace the item after the inner one
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
description = `- - - a` over `  - d` reads as `[[[a, d]]]`, where YAML reads `[[[a], d]]`; with more lines it fails to parse
status = done
created = 2026-10-13
updated = 2026-10-13
```

# YAML compact sequences two deep misplace the item after the inner one

**Status.** Done, in `fix(yaml): a line under compact nested items belongs
to the item at its column` (0f784cb, 2026-10-13). Each source in the table
now reads as PyYAML reads it: a line under compact nested items goes to the
item that begins at its column, and an indent between two nested items is a
parse error, as PyYAML's is. Tests beside `yaml compact nested sequences
keep their nesting` cover three-deep and mixed nesting, and the two-deep
deletes the editor used to refuse with `DeleteChangedOtherValues` now
succeed.

**Repro.** On `main` at 88ec60e, against PyYAML:

| Source                               | fig                       | YAML                       |
| ------------------------------------ | ------------------------- | -------------------------- |
| `- - - a` / `  - d`                  | `[[["a","d"]]]`           | `[[["a"],"d"]]`            |
| `- - - a` / `    - c` / `  - d`      | `[[["a","c"]],"d"]`       | `[[["a","c"],"d"]]`        |
| `- - - a` / `    - b` / `  - d` / `- e` | parse error at `- e`   | `[[["a","b"],"d"],"e"]`    |
| `- - k: v` / `  - c`                 | parse error at `- c`      | `[[{"k":"v"},"c"]]`        |
| `- - k: v` / `    j: w` / `  - c`    | `[[{"k":"v","j":"w"}],"c"]` | `[[{"k":"v","j":"w"},"c"]]` |

One level of compact nesting (`- - a` / `  - d`) reads right. Two levels,
or a compact mapping inside a compact sequence, give the line at the
middle sequence's column to the wrong container.

**Cause.** Not traced. The dash-line containers are marked
`continues_sequence_item` in `languages/yaml/src/parser.zig`, and the
indent and dedent arms of `parseTokens` decide which of the stacked ones a
line at an intermediate column belongs to.

**Found** while fixing
[the nested-item delete](/docs/tasks/closed/yaml-nested-item-delete-drops-parent.md):
deleting `[0][0][0]` from `- - - a` / `    - c` / `  - d` is refused with
`DeleteChangedOtherValues`, because both parses are wrong in different ways.

**Done when** each source above reads as YAML reads it, with a parser
test beside `yaml compact nested sequences keep their nesting`.
