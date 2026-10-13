```fig
title = A trailing comment on a CRLF line is spliced between the CR and the LF
description = setTrailingComment places the comment after the value's line content, which on a CRLF file includes the CR, so YAML reads `a: 1` back as the string `"1\r"`
status = done
created = 2026-10-11
updated = 2026-10-13
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# A trailing comment on a CRLF line is spliced between the CR and the LF

**Status.** Done, in `fix(editor): a trailing comment on a CRLF line goes
before the CR` (13478e3, 2026-10-13). The trailing-comment window stops
before a CRLF's CR, so a new comment goes before it and a replaced or
deleted one keeps it.

**Repro.** The edit cases in fig-conformance (`suites/yaml/edits/crlf.json`,
marked `suspect`), or from the CLI:

```sh
printf 'a: 1\r\nb: 2\r\n' > s.yaml
fig comment --inline s.yaml a note
# s.yaml is now  a: 1\r # note\nb: 2\r\n
fig get s.yaml a -o json     # "1\r", where it was 1
```

The value changes from the integer 1 to the string `"1\r"`, and the line
loses its CRLF. It happens where the line has no comment yet: replacing
an existing one (ZON's `.a = 1, // t` on a CRLF file) is right. A CRLF ZON
`\\` string gets the same splice, which there does not parse, so the edit
is refused; [the ZON task](/docs/tasks/closed/zon-trailing-comment-in-multiline-string.md)
covers that value.

**Cause.** The trailing-comment splice finds the end of the value's line
as the byte before the LF, not before a CRLF. It wants the line ending the
document already uses (`\r\n` here), as the other splices do.

**Done when** a new trailing comment on a CRLF line goes before the CR,
the edited file reads back with every value unchanged, and the
fig-conformance recording no longer marks these cases `suspect`.
