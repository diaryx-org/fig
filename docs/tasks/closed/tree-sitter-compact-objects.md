```fig
title = The tree-sitter grammar reads a fig object as comma-separated
description = `editors/tree-sitter-fig` predates compact objects, so `{x=1 y=2}`, a dotted key in braces, and a one-token value followed by the next pair highlight with error nodes
status = done
created = 2026-10-09
updated = 2026-10-13
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# The tree-sitter grammar reads a fig object as comma-separated

**Status.** Done, in `fix(tree-sitter): a fig object is compact, with dotted
keys and one-token values` (c798507, 2026-10-13). `flow_object` has a fig
body (whitespace or comma separators, a dotted key path, a one-token bare
value) and a JSON body that keeps commas; a corpus case covers each, and
spec § 11's note is gone.

**Repro.** `npx tree-sitter parse` in `editors/tree-sitter-fig` on

```
p = {x=1 y=2}
q = {a.b=1 title="Hello world"}
```

gives `ERROR` nodes. `fig fmt` writes exactly this form since compact
objects ([proposal](/docs/proposals/compact-objects.md), spec § 6.3), so every
formatted file with an inline object is affected.

**Cause.** `flow_object` in `grammar.js` requires `,` between `_fpair`s, its
key is a single `bare_key` or string, and its value is `_fvalue`, whose
`flow_bare` (an external in `src/scanner.c`) runs to the next `,`/`]`/`}`
with spaces included.

**Done when** `grammar.js` has a fig-object rule with whitespace-or-comma
separators, a dotted key path, and a one-token bare value (a token that stops
at whitespace, with `flow_bracket_led_bare` told the same), a JSON-object rule
that keeps commas, `src/` regenerated, a corpus case for each in
`test/corpus/basics.txt`, and spec § 11's note on the gap removed.
