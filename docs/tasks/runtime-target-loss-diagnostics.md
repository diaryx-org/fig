```fig
title = A runtime target has no loss diagnostics
description = `fig get -o <runtime>` and `fig convert -o <runtime>` print without the dropped-comment and lossy-value warnings a compiled target gets, and `fig_document_diagnose` answers `FIG_STATUS_UNSUPPORTED_OPERATION` for a runtime format
status = open
created = 2026-09-10
updated = 2026-09-10
part_of = [tasks](tasks.md)
```

# A runtime target has no loss diagnostics

**Repro.** With a `languages.figl` naming a flat-map helper (the
`tinykv_helper` example in `bindings/rust/fig`):

```bash
$ printf 'a:\n  b: 1\nc: 2\n' > n.yaml
$ fig get n.yaml -o tinykv
c=2
$ fig get n.yaml -o dotenv
warning: dropped table value at `a` (dotenv cannot represent it)
warning: degraded value at `a.b` to string (dotenv has no native type for it)
c=2
```

The compiled target warns about what the strip lost; the runtime target
strips the same way (`parse_dispatch.printRuntime` applies the null strip
and the depth strip its capabilities declare) and says nothing.

**Why it is deferred.** `fig.Diagnostics.analyze` takes a
`SerializeFormat`, and every rule in it reads that format's `Syntax`
(comment styles, native kinds) through the compiled enum. A runtime
target has the same `Syntax` and `lossless` declaration on its
`Runtime.Entry`; `analyze` needs to take the declaration rather than the
enum, which is a refactor of `diagnostics.zig` and its callers (the CLI,
`fig_document_diagnose`) rather than of the carrier. The C API returns
`FIG_STATUS_UNSUPPORTED_OPERATION` for a runtime format so a host can tell
"not yet" from "nothing lost".

**Done when** `fig get n.yaml -o tinykv` warns as `-o dotenv` does, and
`fig_document_diagnose` answers for a runtime format with the same
warnings it would give a compiled one of the same declaration.
