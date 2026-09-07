```fig
title = A flow sequence item reports, and deletes, its parent's leading comment
description = `getLeadingComment` on an item of a one-line array (`members = ["a", "b"]`, `members: [a, b]`) returns the block above `members`, and `deleteLeadingComments` on the item removes it — the item shares its parent's line, and the op anchors on the line rather than on the node
status = open
created = 2026-09-07
updated = 2026-09-07
part_of = [tasks](tasks.md)
```

# A flow sequence item reports, and deletes, its parent's leading comment

**Repro.** With `fig` 3.2.0 (the Rust crate; the Zig editor is the same code):

```rust
use fig::{Editor, Format, Segment};
let src = "# above members\nmembers = [\"a\", \"b\"]\n";
let e = Editor::open(src.as_bytes(), Format::Toml).unwrap();
let item0 = [Segment::Key("members".into()), Segment::Index(0)];
e.leading_comment(&item0)            // Ok(Some("above members")) — should be Ok(None)
let mut e = Editor::open(src.as_bytes(), Format::Toml).unwrap();
e.delete_leading_comments(&item0).unwrap();
e.source()                            // "members = [\"a\", \"b\"]\n" — the parent's block is gone
```

YAML behaves the same (`# above\nmembers: [a, b]\n` — both items report
`above`). A TOML inline-table entry does *not*: `nested = { k = "v" }` under a
comment answers `None` for `nested.k`, and a multi-line array (one item per
line) answers `None` for each item, which is the right answer in both cases.

**Cause.** `leadingCommentLineStart` in `src/editor.zig` anchors a leading
op on `lineStartBefore(source, span.start)` — the start of the line the node
is on. For a flow item that line is its parent's key line, so
`commentBlockStart` walks up from `members = [` and finds `members`' own
block. Every leading op then treats it as the item's: the read returns it,
`deleteLeadingComments` removes it, and `addLeadingComment` inserts above
the parent's line, where the new comment becomes the parent's.

**What it should do.** Per § 3.4 a comment inside a flow collection is
discarded at parse, so a flow item cannot own a leading block at all. A
leading op on a node that does not begin its line — anything but leading
whitespace or the fig marker run between the line start and the node's span
— should answer as if the node had none: `null` from the read, a no-op from
the delete, and `UnsupportedShape` (or a new `CommentsUnanchored`) from
`addLeadingComment`, since there is no line to put one on that the item
would own. The multi-line flow array keeps working as it does, because each
item there does begin its line.

**Found by** flower, whose page view showed `members`' three-line comment
once above the group and once more above each of its four items, and whose
comment editor would have deleted it on an empty commit against any item.
flower guards both until this ships (its `Model` drops an item's leading
comment when it is byte-equal to its parent's, and refuses to edit it), and
the guard comes out when the pin reaches the fixed version.

**Done when** the three leading ops answer as above for a flow-sequence item
on its parent's line, in TOML, YAML, fig, and JSONC, with a test per format
that also asserts the multi-line and inline-table cases keep their current
answers; and `docs/BREAKING-CHANGES.md` or the changelog says the read
changed, since a caller may have been reading the parent's comment through
an item by accident.
