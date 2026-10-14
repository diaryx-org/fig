```fig
title = A leading BOM hides a YAML document's first key from the editor
part_of = [Tasks](/docs/tasks/tasks.md)
description = After a BOM, `set` on the first key appends a duplicate and `replace` cannot find it; the CLI reads the key with the BOM in it, and the JavaScript binding drops the BOM
status = done
created = 2026-10-14
updated = 2026-10-14
```

# A leading BOM hides a YAML document's first key from the editor

**Status.** Done, in `fix(yaml): a BOM-led document reads and edits its
first key, keeping the BOM` (2026-10-14). YAML's tokenizer starts its first
line past a BOM, as every other format's does, so the key is `a`. The
editor's line start (`lineStartBefore` in `kit/src/lines.zig`) puts the
first line past it too, so a column on that line counts from past the BOM
and a splice at a line's start (a delete, a leading comment, a root key
above a TOML or INI file's first header) leaves it first in the file. The
JavaScript binding's decoder keeps a BOM at the start of what the core hands
back, where it dropped it from `Editor.source()`. The bom.yaml case scores
faithful against a module built with every format; the other bom sources'
recorded answers change too, since they were recorded without the BOM.

**Repro.** At eca3f34, with the CLI:

```sh
printf '\xef\xbb\xbfa: 1\n' > b.yaml
fig get b.yaml -o json    # {"﻿a":1}: the BOM is read as part of the key
fig set b.yaml a set      # appends a second key: BOM, a: 1 / a: set
fig replace b.yaml a 2    # error: no such path in the document
```

Through the JavaScript binding (`Editor.open("﻿a: 1\n", yaml)`), the
key reads as `a`, but `setValue(["a"], "set")` still appends, and drops
the BOM: `"a: 1\na: set\n"`. Without the BOM it gives `"a: set\n"`.

So a BOM is not stripped where a key is read (CLI) or where the editor
finds one (binding), and the binding's editor loses it from the output.

fig-conformance (66be227) marks `root: set existing key` in bom.yaml
`suspect`.

**Done when** a document that opens with a BOM reads, finds and edits its
first key as `a`, in the CLI and the binding alike, and keeps the BOM in
what it writes. `fig get` shows `{"a":1}`, `set` and `replace` edit `a` in
place, and the bom.yaml case re-records as faithful.
