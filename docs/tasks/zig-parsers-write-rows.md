```fig
title = The Zig parsers write rows
part_of = [Tasks](/docs/tasks/tasks.md)
status = open
created = '2026-10-12'
description = fig adopts a parse's table as the Document, but each Zig parser still builds a ParseTree and emits it into rows, and materialize, lossless and flat_strip build Nodes and convert them
```

# The Zig parsers write rows

fig now adopts the table a language's parse fills as the `Document`'s
storage, with no copy (`AST.adopt`, [core-only §4.9](/docs/proposals/core-only.md)).
The Zig languages fig ships do not fill that table themselves yet: each
parser builds a `ParseTree` — a `Node` per id, with spans and side facts
in tables indexed by id — and `ParseTree.emit` writes it as rows in
pre-order. So a parse still holds two trees at its peak, and the
conversions that build a tree in core (`materialize`, `Lossless`,
`FlatStrip`, the canonical parser, gron) build `ParseTree.Nodes` and
convert them with `toAst`.

Measured on the scaled lockfiles (ReleaseFast, `fig get` / `fig fmt`,
2026-10-12), against main before the change:

| file | `get` time | `get` peak | `fmt` peak |
| --- | --- | --- | --- |
| large.yaml (1.4 MB) | −20% | 135 → 118 MB | 174 → 147 MB |
| large.json (3.4 MB) | −13% | 147 → 123 MB | 218 → 173 MB |
| large.toml (4.9 MB) | −24% | 277 → 195 MB | 604 → 464 MB |

§4.9 projects about a third of main's peak once the parse tree is gone.

## Done when

Each Zig parser writes its rows, in pre-order with their parents, and its
side facts straight into the table it is handed; the AST-to-AST passes and
the canonical parser write rows through `AST.Builder` or the table; and
`ParseTree` and `parse_tree/` are deleted. The conformance suites score as
before, and the table above is measured again.
