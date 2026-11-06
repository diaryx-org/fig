```fig
title = A block comment after a one-line container moves inside it
part_of = [Tasks](/docs/tasks/tasks.md)
status = done
created = 2026-11-05
updated = 2026-11-05
description = In the canonical form, a /* */ comment after the close of a non-empty container written on one line prints beside its opening bracket and reparses as the first child's leading comment, so canonical_round_trip fails; the nightly Fuzz job still fails on it after the multi-line root fix
```

# A block comment after a one-line container moves inside it

**Status.** Done, in `fix(canonical): keep a block comment after a one-line
container's close at its bottom`. The parser now asks what the printer will
do: `prints_as_block` says whether a closed container has a child or a dangling
comment, and a block comment after one goes to its dangling run. It reads the
tree rather than the source between the brackets, which the throwaway version
below did.

An enumeration of short inputs found one more shape by the same door: `[]/*a*//*a*/`,
an empty root whose end-of-file orphan makes it print as a block after the
first comment had already been bound as its trailing one. `parse_once` now
reads those orphans before it places the comment. With that, all 73.2 million
sequences of up to seven tokens from brackets, commas, a number, a key, both
comment styles, whitespace, an anchor and an alias passed
`canonical_round_trip`. Whether the nightly `Fuzz` run agrees is for its next
run to say.

**Where.** `rust/fig-core/src/canonical/parser.rs`, `parse_once` and
`finish_element`, against `printer.rs`, `container` and `is_block_container`.

The parser and the printer ask different questions about one container. The
parser binds a comment after a container's close by the *source*:
`multiline_container` asks whether a newline lies between the brackets, and
says a one-line container keeps the comment as its trailing comment. The
printer places that trailing comment by the *print*: a container with a child
or a dangling comment prints as a block, whatever its source looked like, and
writes its trailing comment beside the opening bracket.
`capture_open_trailing` takes back only a line comment there, so a block
comment is left for `collect_leading`, which gives it to the first child, or
to the dangling run when there is none.

[A block comment after the root's closing bracket moves inside
it](/docs/tasks/canonical-root-block-comment-moves-inside.md) closed this for
a multi-line root. A one-line container is the same bug by the other door, at
the root and nested. That task's test keeps `{} /* b */`, which holds because
an empty container prints inline. The sentence beside it, that an inline root
keeps a block comment after its close, is true only of an empty one.

## Repro

At 360522f3, a `/* */` comment after the close of a non-empty container
written on one line:

| input | first print | its reparse prints |
|---|---|---|
| `[1] /* b */` | `[ /* b */\n  1\n]\n` | `[\n  /* b */\n  1\n]\n` |
| `{"k": [1] /* b */}` | `{\n  "k": [ /* b */\n    1\n  ]\n}\n` | `{\n  "k": [\n    /* b */\n    1\n  ]\n}\n` |

`ast.eql` holds and `ast.comments_eql` does not, so
`fig_fuzz::canonical_round_trip` panics with "the reparsed tree's comments
differ from the parsed one's". In the first row the comment starts as the
root's trailing comment and ends as the leading comment of `1`.

Each of these fails the same way:

```text
[1]/**/
{"k":1} /*b*/
[/*a*/] /*b*/
{/*a*/} /*b*/
[[1] /*b*/]
[[1], /*b*/ 2]
{"":[/**/]/**/}
```

`[/*a*/]` counts as non-empty here: it has no child, but its dangling comment
makes it print as a block.

These do not fail: `[] /*b*/` and `[[] /*b*/]`, since an empty container prints
inline; `[1] // b` and `[[1] // b\n]`, since `capture_open_trailing` takes a
line comment back; and `[[\n1\n] /*b*/]`, since `multiline_container` sees the
newline.

`fig fmt` on a `.fig` file holding `[1] /* b */` writes the first print. A
second `fig fmt` leaves those bytes alone, so the move shows through the
canonical parser and printer, not as a file that keeps changing.

## Found by

The nightly `Fuzz` job, which has failed on `canonical_round_trip` every night
since 2026-10-16 ([#13](https://github.com/diaryx-org/fig/issues/13)). Run
[41858059323](https://github.com/diaryx-org/fig/actions/runs/41858059323), on
2026-11-04 at e202e50bf, is the first run to include b75ae862, the
multi-line root fix. Its 177-byte input
(`crash-566f4495e1e42f4c354f1572dac7b7832316c2bd`) shrinks to
`{"":[/**/]/**/}` by removing bytes while the same panic holds, and still
panics at 360522f3.

## A way to fix it

The parser's test can ask what the printer will do. A block comment after a
container that will print as a block, meaning one with a child or a dangling
comment, goes to its dangling run, as it already does after a multi-line
container. A line comment stays where it is.

A throwaway version of this, reverted and not on any branch, stopped the panic
on every input above and on the 177-byte crash, and `cargo test -p fig-core`
and `cargo test -p fig-fuzz` passed. It read the source between the brackets
to decide, so a real fix may want a better test than that. It moves the comment
to the bottom of the container on the first print, so it wants a
`Behavioural-change:` trailer. It is one night's crash: whether another shape
is queued behind this one is for the next `Fuzz` run to say.

## Done when

The inputs above round-trip in `rust/fig-core/src/canonical/parser.rs`'s tests,
`[1]/**/` and `{"":[/**/]/**/}` are `canonical_seeds` in fig-fuzz, the comment
beside `a_block_comment_after_a_multi_line_roots_close_stays_at_its_bottom`
says what an inline container does, and the next nightly `Fuzz` run does not
fail on this shape.
