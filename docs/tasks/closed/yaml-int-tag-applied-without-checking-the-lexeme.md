```fig
title = YAML `!!int`/`!!float` is applied without checking the lexeme, and `%TAG !!` is not honoured
description = `!!int 1 - 3` materializes to a number node whose text is `1 - 3`, which every printer then writes bare (`fig get -o json` emits `1 - 3`), and a `%TAG !!` directive that remaps the secondary handle is ignored, so the tag is read as the core int rather than a custom one
status = done
created = 2026-09-07
updated = 2026-09-22
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# YAML `!!int`/`!!float` is applied without checking the lexeme, and `%TAG !!` is not honoured

**Status.** Done, in `fix(yaml): a numeric tag checks its payload, and a
%TAG directive can rebind the !! and ! handles` (2026-09-22). Both halves
are in `materialize.zig` rather than the parser: a tag keeps its source
spelling and the document's `%TAG` directives ride on the AST, so the
materializer expands the handle through them before deciding a tag is
core, and `asNumber` refuses an `!!int`/`!!float` payload that neither the
1.2 nor the 1.1 number grammar reads as that kind. The `json` baseline is
260.

**Repro.** With `fig` at 3cf899f:

```
fig get testdata/yaml/accept/P76L.yaml -o json
1 - 3
fig get testdata/yaml/accept/P76L.yaml -o zon
1 - 3 // Interval, not integer
```

The source is:

```
%TAG !! tag:example.com,2000:app/
---
!!int 1 - 3 # Interval, not integer
```

Neither output is a document in its format. It is the last accept fixture
that `-o json` gets wrong: the `json` score in
`src/languages/yaml/conformance.zig` is held at 259 of 289 by it, with the
other 29 refused by design (a collection key, or a custom tag in strict
mode).

**Cause, two halves.** `applyScalarTag` in `src/languages/yaml/materialize.zig`
returns `.{ .number = .{ .raw = scalarText(node) } }` for `!!int` and
`!!float` without checking that the text sniffs as a number — it validates a
`!!bool` payload (`asBool` → `TagTypeMismatch`) but not a numeric one — so a
number node carries a lexeme that is not a number and every printer writes it
as-is. Separately, the parser skips handle resolution for any `!!` tag
(`parser.zig`, the tag-handle check near `stashAnchor`), so the document's
`%TAG !! tag:example.com,2000:app/` remap is ignored and `!!int` is read as
the core `tag:yaml.org,2002:int` when the document said otherwise; with the
remap honoured, strict materialize would refuse it as a custom tag, as it does
for 6CK3 and Z9M4.

**Done when** `applyScalarTag` refuses a `!!int`/`!!float` payload that does
not sniff as that kind with `TagTypeMismatch`; a `%TAG` directive that
redefines `!!` (or `!`) is honoured when resolving a tag's handle, so P76L's
`!!int` is a custom tag; a test covers each; and the `json` baseline in
`conformance.zig` rises to 260. Found while closing
`json-printer-emits-non-string-keys.md`.
