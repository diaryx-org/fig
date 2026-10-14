```fig
title = A comment after a figl multi-line list's opening bracket is invisible to the editor
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
description = The figl parser reads a comment after a multi-line list's `[` as the list's, but the editor looks only after the `]`, so `comment --get --inline` misses it and `comment --inline` writes a second one after the close
status = done
created = 2026-10-13
updated = 2026-10-14
```

# A comment after a figl multi-line list's opening bracket is invisible to the editor

**Status.** Done, in `fix(editor): a figl multi-line flow list's or
object's trailing comment lives on its opener line` (2026-10-14).
`Syntax.opener_line_comment` now also moves the trailing-comment window to
the opener line of a flow list or object that runs over more than one line
and holds no element on its `[`/`{` line, which is when the parser reads a
comment there, so get, set and delete act on `xs = [ # note`, as a map
entry's value or a sequence element, LF or CRLF. A comment after the close
is still read as the container's when the opener has none, and the editor
never touches it; a one-line flow container, or one with an element on its
opener line, keeps its comment after the close. The printer already wrote
the comment on the opener line, and the JavaScript twin reads it as the
parser does.

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
