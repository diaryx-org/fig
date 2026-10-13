```fig
title = An inline comment on a YAML block scalar lands on the next key's line
part_of = [Tasks](/docs/tasks/tasks.md)
description = A trailing comment on a `|` or `>` block scalar is written after the scalar's last line, which is the next entry's line, so `fig comment --inline` on `a: |` lands as `b: 1 # note`
status = open
created = 2026-10-13
```

# An inline comment on a YAML block scalar lands on the next key's line

**Repro.** On `main` at 5dfb6ab:

```sh
printf 'a: |\n  text\nb: 1\n' > c.yaml
fig comment --inline c.yaml a note
cat c.yaml
# a: |
#   text
# b: 1 # note
```

Every value reads back unchanged, so the check that refuses a comment
which would become value text (aa95dad) does not catch it, but the comment
now belongs to `b`.

**Cause.** The trailing-comment splice puts the comment at the end of the
line the value's span ends on. A block scalar's span ends after its last
content line, so that line's end is the next entry's.

**Done when** a trailing comment on a block scalar goes on its header line
(`a: | # note`), where YAML allows one, and a test covers `|`, `>` and their
chomping and indentation indicators.
