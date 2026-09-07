```fig
title = Editor: read and write the dangling anchor, and comment a node out and back in
description = The editor exposes leading and trailing comments per path but not the dangling run at the end of a container, and has no op that turns an entry into a comment run or a comment run back into an entry — the two things a structural editor needs to show a commented-out line as a disabled entry
status = done
created = 2026-09-07
updated = 2026-09-07
part_of = [tasks](tasks.md)
```

# Editor: read and write the dangling anchor, and comment a node out and back in

**Status.** Done, in `feat(editor): the dangling comment anchor, and
comment-out and back` (2026-09-07) — one commit, because the two halves share
the marker-column arithmetic and the uncomment ops address the anchor. All
nine ops are on `Editor` and cross `c_api.zig`, `fig.h`, `fig-sys`, the `fig`
crate's `Editor`/`Embed` and the TypeScript `Editable`, with round-trip tests
over YAML, TOML, fig and JSONC in the core suite and one each in the Rust and
TypeScript suites. Two deviations from the wording below, both documented at
their call sites: the comment-out pair refuses a node that does not have its
LINES to itself rather than any node in a flow collection — a pretty-printed
JSONC member is flow-spelled and perfectly commentable, a `[a, b]` item is not
— and the dangling trio works on a multi-line flow container for the same
reason, since a JSONC object's `// note` before the closing brace is a
dangling run nothing else can address.

**Why.** [flower](https://github.com/diaryx-org/flower) now reads and edits a
node's leading and trailing comments through `fig::Editor` (flower's
`docs/tasks/disabled-entries.md` is the other half of this task). The next
thing it wants to show is a *commented-out entry* — `# port = 8080` under a
`[server]` table — as a disabled row with a toggle, rather than as prose
above whatever entry happens to follow it. That needs two things the editor
does not have, and both belong here rather than downstream, because only the
editor has the source spans to do them byte-exactly.

**What is there.** `src/editor.zig` has `addLeadingComment`,
`setTrailingComment`, `deleteLeadingComments`, `deleteTrailingComment`,
`getLeadingComment`, and `getTrailingComment`, each crossing `c_api.zig`,
`fig-sys`, the `fig` crate, and `embed.zig`. The AST side-table
(`src/ast/ast.zig`, `NodeComments`) carries a third anchor, `dangling` — the
run at the end of a container's body (§ 3.4 of the spec) — that the printers
emit and nothing reads or writes through the editor.

**Gap one: the dangling anchor.** A comment line after the last entry of a
container is that container's dangling run, and a commented-out *last* entry
is exactly that. The editor cannot read it (`getLeadingComment` on the next
sibling finds nothing, because there is none) and cannot write there
(`addLeadingComment` needs a node to anchor below). Wanted:

- `getDanglingComment(path)`, `addDanglingComment(path, text)`,
  `deleteDanglingComments(path)` — the container at `path` (the root for an
  empty path), with the same semantics as the leading trio: markers and
  indentation stripped on read, one comment line per line on write, at the
  body's child depth, `CommentsUnsupported` for strict JSON, `null` for none
  and `""` for a bare marker.
- Mirrored through `c_api.zig`, `fig-sys`, the `fig` crate's `Editor`, and
  `embed.zig` / `Embed`, like the six that exist.

**Gap two: comment out, and back.** Turning an entry into a comment run by
re-serialising its value and calling `addLeadingComment` loses the entry's own
spelling — its quoting, its layout, the comments inside it — and the reverse
(strip the markers, `insertValue`) loses the same in the other direction.
Both are one splice over the node's span. Wanted:

- `commentOut(path)`: prefix every line of the node's source span (the key
  through the end of its value, for a mapping entry; the item for a sequence
  item) with the line marker at the line's own indentation, so the entry
  becomes a comment run — the leading block of its next sibling, or the
  parent's dangling run when it was last. The node's own leading block stays
  above it, untouched, so a `# why` above `# port = 8080` survives as a note
  on the note. The tree afterwards no longer has the node; a `getNodeByPath`
  for it is `NotFound`.
- `uncommentLeading(path, first_line, line_count)` and
  `uncommentDangling(container_path, first_line, line_count)`: strip the
  marker (and the one space after it) from `line_count` lines of the named
  block starting at `first_line`, then reparse. If the result does not parse,
  or parses to a document whose other nodes changed, the splice is rolled back
  and the call returns an error (`CommentNotAnEntry`, or the parse error) —
  the document is unchanged, as every editor op promises. Lines are addressed
  by index within the block because *which* lines look like an entry is the
  caller's judgement (flower parses the block as a fragment and decides); the
  editor's part is the byte edit and the guarantee that it parsed.
- `CommentsUnsupported` for strict JSON; `UnsupportedShape` for a node inside
  a flow collection, where a comment is discarded at parse (§ 6.3) and so has
  nowhere to go.

**Done when** the nine functions exist in `editor.zig` with language hooks
where a format's comment placement needs one (NestedText's `seqItemLineStart`
is the precedent), cross the four bindings, and are covered over TOML, YAML,
fig, and JSONC by tests that round-trip: `commentOut` then `uncomment*` is
byte-identical to the start, and `getDanglingComment` reads what
`addDanglingComment` wrote. fig's `Editor` then ships in a release and
flower's task picks it up from there.
