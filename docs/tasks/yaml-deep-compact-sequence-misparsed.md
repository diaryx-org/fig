```fig
title = YAML compact sequences two deep misplace the item after the inner one
part_of = [Tasks](/docs/tasks/tasks.md)
description = `- - - a` over `  - d` reads as `[[[a, d]]]`, where YAML reads `[[[a], d]]`; with more lines it fails to parse
status = open
created = 2026-10-13
```

# YAML compact sequences two deep misplace the item after the inner one

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
