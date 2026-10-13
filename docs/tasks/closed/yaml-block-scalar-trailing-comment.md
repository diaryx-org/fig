```fig
title = An inline comment on a YAML block scalar lands on the next key's line
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
description = A trailing comment on a `|` or `>` block scalar is written after the scalar's last line, which is the next entry's line, so `fig comment --inline` on `a: |` lands as `b: 1 # note`
status = done
created = 2026-10-13
updated = 2026-10-13
```

# An inline comment on a YAML block scalar lands on the next key's line

**Status.** Done, in `fix(editor): a YAML block scalar's trailing comment
goes on its header line` (e6ca1eb, 2026-10-13). A trailing comment on a YAML
`|` or `>` block scalar goes on its header line (`a: | # note`), and get and
delete find it there rather than on the next entry's line. A test covers
both styles with chomping and indentation indicators, a sequence item and
CRLF. A block scalar that ends the file with no final newline is still
refused with `CommentInsideValue`, which writes nothing.

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
