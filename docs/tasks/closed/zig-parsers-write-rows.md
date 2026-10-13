```fig
title = The Zig parsers write rows
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
status = done
created = '2026-10-12'
description = Every Zig parser writes its rows into fig's table through RowWriter, and ParseTree is gone; a parse's peak is a quarter to a third of what it was before the Document was the table
```

# The Zig parsers write rows

fig adopts the table a language's parse fills as the `Document`'s storage,
with no copy (`AST.adopt`, [core-only §4.9](/docs/proposals/core-only.md)).
When that landed, the Zig languages fig ships did not fill the table
themselves: each parser built a `ParseTree` — a `Node` per id, with spans
and side facts in tables indexed by id — and `ParseTree.emit` wrote it as
rows, so a parse held two trees at its peak. The conversions that build a
tree in core (`materialize`, `Lossless`, `FlatStrip`, the canonical parser)
built `ParseTree.Nodes` and converted them.

## Done

Every Zig parser writes its rows through `RowWriter`
(`src/kit/row_writer.zig`): in the order it meets them, linked as its tree
was, then `finish` puts them in pre-order with their parents and moves each
side fact to its node's row. The AST-to-AST passes and the canonical parser
write rows too, and `ParseTree` is gone. The conformance suites score as
before, compiled and JS.

Measured on the scaled lockfiles (ReleaseFast, best of seven,
2026-10-12): before the Document was the table, after it was adopted with
the parse tree still built, and now.

| file | `get` ms | `get` peak MB | `fmt` ms | `fmt` peak MB |
| --- | --- | --- | --- | --- |
| large.yaml (1.4 MB) | 34 / 36 / 21 | 135 / 118 / 31 | 80 / 73 / 57 | 174 / 147 / 60 |
| large.json (3.4 MB) | 38 / 35 / 24 | 147 / 123 / 54 | 73 / 67 / 56 | 218 / 173 / 104 |
| large.toml (4.9 MB) | 101 / 85 / 45 | 277 / 195 / 89 | 194 / 175 / 114 | 606 / 464 / 218 |

The figl parser still builds a tree of its own in an arena and writes it
bottom-up; it is not on this table, and is the one parser with a second
copy left.
