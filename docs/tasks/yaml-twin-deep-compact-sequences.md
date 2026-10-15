```fig
title = The JavaScript YAML twin misreads compact sequences two deep
part_of = [Tasks](/docs/tasks/tasks.md)
description = `js/yaml.js` still has the compact-nesting bug 0f784cb fixed in the Zig parser, so it refuses `- - k: v` over `    w: x` over `  - y`, and 53 fig-conformance edit cases in nested-seqs.yaml
status = done
created = 2026-10-13
updated = 2026-10-14
```

# The JavaScript YAML twin misreads compact sequences two deep

**Status.** Done, in `fix(yaml): the JavaScript twin hands a line under
compact nested items to the item at its column` (2026-10-14), which ports
0f784cb's `landContinuations` and the tokenizer's dedent to any `- `
content column. Every source in the compact-sequence task's table reads as
PyYAML reads it, `npm run edits -- yaml` in fig-conformance at 53fc34c
scores the twin 4377/4377 (4300 before), and
`test/fixtures/yaml/compact-nested.yaml` with a refusal test in
`twins.test.ts` covers the shapes.

**Repro.** In fig-conformance at 9415fc2, `npm run edits -- yaml` scores
the JavaScript languages 4324/4377; every miss is in `nested-seqs.yaml`,
whose source the twin cannot parse (`ParseError` on each case), where fig's
native YAML matches all 4377.

**Cause.** 0f784cb fixed the Zig parser (`landContinuations` in
`languages/yaml/src/parser.zig`, now `land_continuations` in
`languages/yaml/rust/parser.rs`, and the tokenizer's dedent to any `- `
content column on a dash line) so that a line under compact nested items
belongs to the item that begins at its column. `bindings/typescript/js/yaml.js`
reads compact items the way the Zig parser did before.

**Done when** the twin reads every source in the
[compact-sequence task](/docs/tasks/closed/yaml-deep-compact-sequence-misparsed.md)'s
table as PyYAML does, `npm run edits -- yaml` in fig-conformance matches
4377/4377, and the twins test in `bindings/typescript/test/twins.test.ts`
covers the shapes.
