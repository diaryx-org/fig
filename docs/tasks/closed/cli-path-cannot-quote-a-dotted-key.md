```fig
title = A command-line path cannot name a key that contains a `.` or a `[`
description = `parsePath` splits on `.` and `[` with no quoting or escaping, so `maven.compiler.source` under `<properties>`, an ssh `Host github.com`, or any TOML quoted key with a dot is unreachable from `get`, `set`, `delete` and `comment`
status = done
created = 2026-09-14
updated = 2026-09-22
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# A command-line path cannot name a key that contains a `.` or a `[`

**Status.** Done, in `feat(cli): a path key can be quoted or escaped to
hold a . or [` (2026-09-22). `parsePath` takes `a."b.c"` (JSON's escapes
inside), `a.'b.c'` (verbatim), `a["b.c"]` / `a['b.c']` — the bracket form
`-o gron` prints, so a gron line reads back — and `a.b\.c`; every
action's `--help` says so under "path format".

**Repro.** Compiled TOML:

```
$ printf '[a]\n"b.c" = 1\n' > t.toml
$ fig get t.toml a.b.c
error: no such path in the document (`get` it to see what's there)
```

There is no spelling that reaches `"b.c"`. The bindings take a
`Segment::Key("b.c")` and are fine; the CLI is the one caller that has to
go through text. Real files are full of these: every Maven property
(`maven.compiler.source`), every ssh host pattern with a domain, git's
`[branch "feature/x"]`, YAML keys that are file names.

**Where.** `cli/args.zig`, `parsePath`: a `.` is a separator, a `[` opens
an index, and everything else runs to the next of those.

**Done when** a key segment can be quoted — `a."b.c"` and `a.'b.c'`, with
the quote characters removed and the segment taken verbatim, which is what
TOML, fig and every shell user already expect — or escaped (`a.b\.c`), and
`fig get --help` says so; `gron` output for such a key should be
readable back through the same syntax.
