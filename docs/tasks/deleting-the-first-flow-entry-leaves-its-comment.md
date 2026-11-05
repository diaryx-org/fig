```fig
title = Deleting the first flow entry leaves its comment
part_of = [Tasks](/docs/tasks/tasks.md)
status = done
created = 2026-11-04
updated = 2026-11-04
description = Deleting a flow mapping's or sequence's first member leaves the comment on its line, which then reads as the next member's; a comment before the separator makes the same delete refuse, and a later member's delete already takes its comment
```

# Deleting the first flow entry leaves its comment

**Status.** Done, in `fix(editor): a flow member on its own line goes with its
comments` (2026-11-04). `own_line_flow_item` now serves a first member as well
as a later one, and an only member with no separator after it, and it reads a
paired comment before the separator as the member's. A last member takes the
comments ending its line in the backward scan, and an item of a comma sequence
takes the comment block above it, as a mapping's entry already did. The shapes
are in `rust/fig-core/tests/flow_delete.rs`. An emptied multi-line container
now keeps its closing bracket's indent, which changed one ZON edit fixture.

fig-conformance records the old splice in 23 edit cases (json 3, jsonc 5,
json5 6, zon 8, figl 1, in each suite's `crlf`, `everything`, `comments` and
`flow` sources), so its edit suites want re-recording from a fig with this fix.

A delete of a flow member that stands on its own line should take that line,
comment included. A later member's delete does. The first member's leaves the
comment behind, where it reads as a comment on the next member.

## Repro

At 7d3e75e1, in JSONC:

```console
$ printf '{\n  "a": 1, // about a\n  "b": 2\n}\n' > a.jsonc
$ fig delete a.jsonc a
$ cat a.jsonc
{
  // about a
  "b": 2
}
```

The same file with `"a"` second loses the comment with its entry:

```console
$ printf '{\n  "z": 0,\n  "a": 1, // about a\n  "b": 2\n}\n' > b.jsonc
$ fig delete b.jsonc a
$ cat b.jsonc
{
  "z": 0,
  "b": 2
}
```

JSON5, ZON and a YAML flow mapping do what JSONC does, and so does a
sequence: `fig delete` of `x[0]` in `[\n 1, // one\n 2\n]` leaves `// one`.

The same splice has four more shapes:

| before | delete | after |
|---|---|---|
| `"a": 1 /* c */,` then `"b": 2`, each on a line | `a` | refused: the file would not parse after it |
| `"a": 1,` then `"b": 2 /* c */,` then `"c": 3`, each on a line | `b` | `"a": 1 /* c */,` then `"c": 3`: the comment moves up onto `a` |
| `"a": 1 // only`, alone in its braces | `a` | `{`, `// only`, `}` |
| `// over a` then `"a": 1, // about a` then `"b": 2`, each on a line | `a` | `// about a` at column 0, then `"b": 2` |

Nothing is lost that a value holds. What changes is what a comment describes:
the note on a deleted entry now sits over an entry it says nothing about.

## A neighbour's indent

The same scan also takes the next member's indent, against the promise that
only the edited span changes:

```console
$ printf '{\n  "a": 1,\n    "b": 2\n}\n' > c.jsonc
$ fig delete c.jsonc a
$ cat c.jsonc
{
  "b": 2
}
```

fig-css's `with_declaration` shows it from the other side: taking `color` out
of `"\ncolor: red;\n  fill: blue;\n"` gives `"\nfill: blue;\n"`.

## Cause

`remove_flow_item` in `rust/fig-core/src/editor/mod.rs` has two paths. A later
member goes through `own_line_flow_item`, which takes the whole line and any
comment after the separator. That is the fix in `fix(editor): four deletes take
only their own entry or item`. A first member does not: its branch deletes from
the item to the next non-blank byte, so it stops at a comment, and where it does
not stop it ends at the next member's first byte. A comment before the separator
stops the scan short of the separator too, which is the refusal, and
`own_line_flow_item` gives up on that shape as well, which is the second row.

## Scope

This task is about a member on a line of its own. A comment between two members
on one line, as in `{ "a": 1, /* c */ "b": 2 }`, stays where it is: it reads as
well before `b` as after `a`, and nothing here needs it moved.

The CRLF half of this splice, where the scan stops at the `\r`, belongs to
[An inserted line ends in LF in a CRLF file](/docs/tasks/an-inserted-line-ends-in-lf-in-a-crlf-file.md).
The fix for that adds `\r` to `is_flow_item_ws`, and the shapes above reproduce
on that fix's branch as well.

## Done when

Each repro above leaves no comment of the deleted member and no change to a
neighbour's bytes, for a mapping and a sequence, with a test for each shape in
`rust/fig-core/tests/`, and the fig-conformance edit suites re-recorded if any
of them records the old splice.
