```fig
title = An insert into a section with no entries of its own lands inside its first child
description = `insertBlockKey` anchors a section that has only header-introduced children on the line after its first header, which is the first child's body; TOML's `set a.y 2` with only `[a.b]` present writes `y` into `a.b`
status = done
created = 2026-09-14
updated = 2026-09-22
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# An insert into a section with no entries of its own lands inside its first child

**Status.** Done, in `fix(editor): an insert into a section with no own
entries anchors on its own header, or is refused` (2026-09-22).
`insertBlockKey` anchors on the first header line recorded for the
section that is not a child's header — so `[a.b]` … `[a]` takes `y` under
`[a]` — and refuses with `ImplicitSection` when every one is, which `set`
reports as it is rather than as the replace's `NotFound`. Tests in
`editor.zig` cover TOML's implicit table (one and two levels deep), an
explicit header before and after the child's, and INI's empty section.

**Repro.** Compiled TOML, on `main` at 66c00f8:

```
$ printf '[a.b]\nx = 1\n' > t.toml
$ fig set t.toml a.y 2
$ cat t.toml
[a.b]
y = 2
x = 1
```

`a` exists — the implicit table `[a.b]` makes — and `y` was asked for
under it, but it is written under `a.b`. Nothing refuses, nothing warns,
and the file parses. INI does the same for `[a]` … `[a "b"]`-shaped
input in any runtime language that models it that way (fig-quickjs's
`gitconfig.mjs`: `set remote.pushDefault origin` with only `[remote
"origin"]` present lands in `origin`; its `hcl.mjs`: an attribute set on
a label level lands in the first block).

**Where.** `editor.zig`'s `insertBlockKey`. It skips every child that is
out of region (its value a section whose header mention is on the entry's
line), and when that leaves no anchor it takes the `isSection(mapping)`
branch: `lineEndAfter(source, parsed.span(mapping).start)`. For a section
whose own span is its key segment inside a header it did not open alone
— an implicit TOML table, a git config `[a "b"]` passing through `a` — that
line is the first child's header, so the entry lands in the child.

The branch is right for the case it was written for: a section with no
children at all (INI's empty `[section]`), whose one header line is its
own. What it cannot tell apart is a section whose *every* child is a
section of its own — there, the header at `span.start` belongs to a child.

**Done when** `insertBlockKey` refuses (or places correctly) an insert into
a section that has no in-region child and whose `span.start` line is a
child's header: the refusal is enough — the engine cannot spell a new
`[a]` header from here, and `insertContainer` is the op that can — and the
TOML repro above either writes `[a]\ny = 2` or errors. A test in
`editor.zig` for TOML's implicit table and for INI's empty section, so the
right case stays right.
