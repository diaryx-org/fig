```fig
title = YAML printer panics on some accept-corpus documents
description = `fig get` and `fig fmt` on 30 of the 289 yaml-test-suite accept documents crash in `printKeyValue` — a non-string mapping key reaches a `.string` union access
status = done
created = 2026-09-04
updated = 2026-09-04
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# YAML printer panics on some accept-corpus documents

**Status.** Done, in `fix(yaml): spell every mapping key kind; fix two
explicit-key parser gaps` (2026-09-04). The printer spells each key kind
(`null: a`, `23: x`, `*ref : x`, and the explicit `? key` form for a
collection, inlined as flow when it fits), every accept document prints and
re-parses except the two carrying a `%TAG` directive, and the conformance
scoreboard now ratchets a print-and-re-parse count so this cannot regress.
Two parser gaps the re-parse exposed are fixed in the same commit: a `:`
right of a `?`'s column no longer closes the outer key (`?\n  ? a\n  : b`),
and a property on an implicit key's own line (`&a a: b`, E76Z/74H7)
decorates the key, not the mapping it opens.

**Repro.** With any `fig` since at least cli 4.0.0:

```
fig get testdata/yaml/accept/26DV.yaml
thread N panic: access of union field 'string' while field 'alias' is active
src/languages/yaml/printer.zig:158:56 in printKeyValue
```

`fig fmt` and `fig get -o yaml` on the same file panic identically. The
parser accepts every one of these documents (`fig check` passes, and the
conformance suite scores them as accepted); it is the printer's key path
that assumes a mapping key is a `.string` node.

**Scope.** 30 of the 289 files in `testdata/yaml/accept/` crash, by the
active union field the key actually holds:

- `alias`: 26DV, E76Z
- `null_`: 2JQS, 6M2F, CFD4, FH7J, NHX8, S3PD, SM9W-1, WZ62
- `sequence`: 4FJ6, 6PBE, KK5P, M5DY, RZP5, SBG9, X38W
- `mapping`: 9MMW, M2N8-1, Q9WF, V9D5
- `number`: 74H7

A complex key (`? [a, b] : c`), a null key (`: value`), an alias as key and
a numeric key are all valid YAML; the printer either has to spell them
(`? ` complex-key syntax, `!!null`, the alias) or refuse with an error the
CLI reports, never panic.

**Done when** every file in `testdata/yaml/accept/` survives `fig get` and
`fig fmt` without a panic, and a test in `languages/yaml/printer.zig` covers
each of the five key kinds above. Found by the CLI byte-diff run while
verifying the pluggable-formats steps (docs/proposals/pluggable-formats.md
§9); it is not caused by them — the parent commit panics identically.
