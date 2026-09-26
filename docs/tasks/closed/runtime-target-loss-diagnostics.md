```fig
title = A runtime target has no loss diagnostics
description = `fig get -o <runtime>` and `fig convert -o <runtime>` print without the dropped-comment and lossy-value warnings a compiled target gets, and `fig_document_diagnose` answers `FIG_STATUS_UNSUPPORTED_OPERATION` for a runtime format
status = done
created = 2026-09-10
updated = 2026-09-26
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# A runtime target has no loss diagnostics

**Status.** Done, in `feat: loss diagnostics for a runtime target, and a
value refused where the target cannot hold it`.
`Diagnostics.analyzeFor` takes a `Runtime.Target`. For a runtime language
it reads the same two declarations `printRuntime` strips by: `lossless`
(a missing `null` dropped, a missing extended scalar degraded) and
`max_mapping_depth` (a `null`, a sequence, or a mapping past the depth
dropped). It also reads `syntax` for comments, and claims nothing about
them for a language that declares none, whose printer is handed every
comment. `printRuntime` now applies
both strips when a language declares both. The CLI's `get`, `convert` and
`fmt` warn through one `reportLoss`, and `fig_document_diagnose` and
`fig_value_diagnose` answer for a runtime format. One difference from
`-o dotenv`: a runtime target is not warned that a number or boolean
degrades to a string, because nothing in its declaration says so.

The same analysis, run at the depth where an edited value lands
(`Options.depth`, `Diagnostics.firstDropped`), now refuses a `set`,
`insert` or `patch` whose value the target cannot hold there. Before, a
mapping into dotenv, `.properties`, INI below a section, or a flat runtime
language was printed as a document and spliced in as text (`n=x=1`).
INI's printer gets a `printSplice` that refuses a mapping, since a value
splice cannot write a section.

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
