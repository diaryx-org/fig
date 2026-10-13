```fig
title = A YAML plain scalar's continuation line that starts with a dash reads as a nested list
part_of = [Tasks](/docs/tasks/tasks.md)
description = `- a` over `  - b` reads as `["a", ["b"]]`, where YAML folds the more-indented line into the plain scalar, `["a - b"]`
status = open
created = 2026-10-13
```

# A YAML plain scalar's continuation line that starts with a dash reads as a nested list

**Repro.** On `main` at 0f784cb, against PyYAML 6.0.2:

```sh
printf -- '- a\n  - b\n' > s.yaml
fig get s.yaml '' -o json      # ["a", ["b"]]
python3 -c "import yaml; print(yaml.safe_load(open('s.yaml')))"   # ['a - b']
```

A plain scalar continues onto every more-indented line, and inside a
scalar's continuation `- ` is text, not an indicator: the line after `- a`
indented past the item's content column is `a`'s second line. fig opens a
nested sequence there instead, so it reads different data from every other
YAML reader.

**Found** while fixing [compact sequences two deep](/docs/tasks/closed/yaml-deep-compact-sequence-misparsed.md)
(0f784cb); it predates that change.

**Done when** a plain scalar's continuation line that starts with `- `,
`? ` or `: ` reads as text in a sequence item and under a mapping key, as
PyYAML reads it, with tests beside the parser's, and the yaml-test-suite
scores in fig-conformance do not drop.
