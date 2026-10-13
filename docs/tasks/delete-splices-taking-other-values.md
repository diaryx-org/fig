```fig
title = Four deletes are refused because their splice takes another value
part_of = [Tasks](/docs/tasks/tasks.md)
description = The delete guard refuses four shapes whose splice reads the source wrongly and takes values beside the deleted one; each should delete only its entry or item
status = open
created = 2026-10-13
```

# Four deletes are refused because their splice takes another value

The delete guard (`keepOthers` in `src/editor.zig`) rolls these back with
`DeleteChangedOtherValues`, or the reparse fails. Before 346db34 each was
reported as a success, and the fig-conformance edit suites recorded the
wrong result. Each splice is the line delete or the flow splice applied to
a shape it misreads.

**Repro.** On `main`, each a `deleteKey` or `removeSeqItem`:

- JSON5, `n` in `"b c": [1, 2], // tb` over `  n: NaN, i: -Infinity,`:
  the splice takes `\n  n: NaN`, and the rest of `n`'s line joins the
  `// tb` comment, so `i` and every key after it on that line go.
- YAML, `seq[1][0]` where `seq[1]` is `- &i [1, 2]`: the anchored
  sequence's span starts at `&i`, so the flow sniff (`isFlow`) reads it as
  block. The old splice took the whole item; now deleting the first item
  does not parse and deleting the last is refused.
- YAML, `n[0].a` in `n: [a: b, ? c, : d, "e":f]`: a single-pair flow
  mapping inside a flow sequence has no braces, and the delete takes the
  whole `n:` line.
- ZON, `c` in `.ml =\n    \\line two\n    ,\n    .c = 'A', .nl = '\n',`:
  the splice takes from the multi-line string's line end through `.c =
  'A'`, so the rest of `c`'s line joins the string.

**Done when** each deletes only its entry or item, leaving every other
value as it was, with a test beside the guard's in each language's
`tests/editor.zig`.
