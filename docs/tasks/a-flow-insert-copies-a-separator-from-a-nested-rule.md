```fig
title = A flow insert copies a separator from a nested rule
part_of = [Tasks](/docs/tasks/tasks.md)
status = open
created = 2026-11-06
updated = 2026-11-06
description = With flow_kv_sep_from_siblings, an insert takes its key-value separator from the first member and its joint from the gap after it; a fig-css nested rule has neither, so a declaration added to a block that opens with one is refused as NotFound or UnsupportedOperation
```

# A flow insert copies a separator from a nested rule

**Where.** `rust/fig-core/src/editor/mod.rs`, `insert_flow_map_entry` and
`flow_map_joint`, for a dialect that sets `flow_kv_sep_from_siblings`.

An insert into a non-empty flow mapping spells the new entry as its neighbours
are. `insert_flow_map_entry` takes the separator between key and value from the
first member: the bytes from its key's end to its value's start.
`flow_map_joint` takes what goes between entries from the gap between the first
two members. Both assume the first member is written `key <sep> value`, with a
separator after it.

[fig-css](https://github.com/diaryx-org/fig-css) has a member that is not. A
declaration block may hold a nested rule, which it reads as a selector over a
flow mapping, so `&:hover { b: c }` has only a space before its value and needs
nothing after its `}`. Where a block opens with one, the insert copies those
bytes: a separator of `" "`, or nothing where the rule is written `&:hover{`,
and, when another member follows, a joint of one space with no `;`. The entry it
would write is not a declaration, and the editor refuses it.

fig-css's crate docs say its editor inserts declarations in a block or a list,
and name one exception, an at-rule's block of rules. These are declaration
blocks.

## Repro

fig-css at 601c5c2, against fig 7.0.0 and against fig's `main` at 7344d3aa
through a `[patch.crates-io]`, with the same result:

```rust
use fig::{Editor, Segment::Key};

let sheet = fig_css::formats().unwrap().stylesheet;
let set = |css: &str| {
    let mut e = Editor::open(css.as_bytes(), sheet).unwrap();
    let r = e.set_value(&[Key(".a"), Key("x")], "y");
    (r, e.source().unwrap().to_string())
};

set(".a { &:hover { b: c } }");          // Err(NotFound), source unchanged
set(".a {\n  &:hover { b: c }\n}\n");    // Err(NotFound), source unchanged
set(".a { &:hover { b: c } z: w }");     // Err(UnsupportedOperation), source unchanged
set(".a { z: w; &:hover { b: c } }");    // Ok: ".a { z: w; &:hover { b: c }; x: y }"

// The same through a declaration list.
fig_css::with_declaration("&:hover{b:c}", "a", Some("blue")); // Err("path or region not found")
```

The last `set` is the control: with a declaration first, the same two members
take the entry. Logging `sep` and the joint in `insert_flow_map_entry` shows
`" "` for the three `set`s that fail, `""` for `with_declaration`, and `": "`
for the one that works. The joint is `;` and a space in all but the third
`set`, where it is a space alone.

## What it costs

Nothing is damaged: the edit is refused and the source is left as it was. A
caller editing a sheet that uses CSS nesting gets `NotFound` for a rule that is
there, or `UnsupportedOperation` for an edit the crate says it makes.

No shipped path is known to reach it yet. thorn opens a stylesheet with the
editor only in a test (`profile.rs`), and thorn and leaf otherwise edit `style`
attributes through `with_declaration`, where a nested rule is unusual.

## What was tried

In a scratch copy, on no branch: taking the separator from the first member
whose value is not a mapping, and from `kv_sep` where there is none. That
fixes the first two `set`s, writing `.a { &:hover { b: c }; x: y }` and
`.a {\n  &:hover { b: c };\n  x: y\n}\n`, both of which fig-css reads back. It
does not fix the third, whose joint still comes from the gap after the nested
rule, so `flow_map_joint` needs its half too.

That change is too blunt to take as it stands. figl also sets
`flow_kv_sep_from_siblings`, and there the bytes before a `{` are a real
separator (`{a={b=1}}`), so skipping every mapping would stop an entry added
there copying its neighbour. The test of a usable sibling may be whether the
bytes hold the dialect's `kv_sep` once trimmed, not what kind of value follows.
This was not tried against figl.

## Done when

An insert into a flow mapping whose first member has no separator of its own
writes a valid entry: each failing `set` above returns `Ok`, fig-css reads the
result with the declaration in place and the nested rule unchanged, and
`with_declaration("&:hover{b:c}", "a", Some("blue"))` returns a list whose
`declaration` for `a` is `blue`. figl's copied separators and joints stay as
they are. A test holds it in fig with a small registered language, since fig
does not depend on fig-css, and fig-css takes the change at its next fig
release.
