```fig
title = A comment after a figl multi-line list's opening bracket is invisible to the editor
part_of = [Tasks](/docs/tasks/tasks.md)
description = The figl parser reads a comment after a multi-line list's `[` as the list's, but the editor looks only after the `]`, so `comment --get --inline` misses it and `comment --inline` writes a second one after the close
status = open
created = 2026-10-13
```

# A comment after a figl multi-line list's opening bracket is invisible to the editor

**Repro.** On `main` at 8a1c8e4:

```sh
printf 'xs = [ # op\n  1,\n  2,\n]\ny = 1\n' > f.figl
fig comment --get --inline f.figl xs    # error: no inline comment at that path
fig comment --inline f.figl xs note     # writes `] # note`; `# op` stays
```

**Cause.** The same as [the multi-line string's](/docs/tasks/closed/figl-multiline-string-opener-comment.md),
which ce65966 fixed for scalars: `Syntax.opener_line_comment` moves the
trailing-comment window to the opener line only for a scalar that runs over
more than one line, and a flow list or object spread over lines is a
container.

**Done when** a figl flow list or object that runs over more than one line
has its trailing comment on its opener line for get, set and delete, as the
parser reads it, with the printer and the JavaScript twin agreeing, and a
one-line flow container is unchanged.
