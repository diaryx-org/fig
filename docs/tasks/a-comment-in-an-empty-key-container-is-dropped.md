```fig
title = A comment in an empty container used as a key is dropped
part_of = [Tasks](/docs/tasks/tasks.md)
status = done
created = 2026-11-06
updated = 2026-11-07
description = In the canonical form, a line comment after the opening bracket of an empty container that is a mapping key is read as the key's trailing comment, which nothing prints, so the comment is lost and canonical_round_trip fails
```

# A comment in an empty container used as a key is dropped

**Status.** Done, in `fix(canonical): keep a comment in an empty key beside its
opener`. The `KeyValue` arm writes its key through `Printer::key`, which prints
an empty container holding a trailing comment as a block, as proposed below.
`bindings/typescript/js/canonical.js`, which this task did not name, lost the
comment the same way, and its `writeKey` keeps the two printers alike.
`{[//\n]:1}` is one of `canonical_seeds`.

**Where.** `rust/fig-core/src/canonical/printer.rs`, the `KeyValue` arm of
`node` and the inline path of `container`, against `capture_open_trailing` in
`parser.rs`.

`capture_open_trailing` makes a line comment after an opening bracket that
container's trailing comment. A block container prints it itself, beside its
opener. An empty one prints inline and leaves it to whoever writes its line:
`container`'s loop, which writes the trailing comment of
`trailing_comment_anchor(id)`. For an entry that anchor is the value, so when
the empty container is the entry's key, nothing writes its comment.

## Repro

At 17e31b51, built with `--features canonical`:

```console
$ printf '{\n  [ // why\n  ]: 1\n}\n' > k.txt
$ fig get -i canonical k.txt -o canonical
{
  []: 1
}
```

The same comment on a value survives:

```console
$ printf '{\n  "k": [ // why\n  ]\n}\n' > v.txt
$ fig get -i canonical v.txt -o canonical
{
  "k": [] // why
}
```

`{[//\n]:1}` is the smallest input. Through `fig_fuzz::canonical_round_trip`
it panics with "the reparsed tree's comments differ from the parsed one's":
`ast.eql` holds and `ast.comments_eql` does not. `{{//\n}:1}` and
`[{[//\n]:1}]` fail the same way.

These do not fail:

| input | why it holds |
|---|---|
| `{[/*a*/]: 1}` | a block comment goes to the dangling run, so the key prints as a block |
| `{[1 // a\n]: 1}` | the key has a child, so it prints as a block |
| `{[// a\n 1]: 1}` | the same, with the comment beside its opener |
| `{[[// a\n]]: 1}` | the inner container is a sequence child, whose line is written for it |

A comment after a key's close cannot reach this: `{[] // a\n: 1}` is refused
with `ExpectedColon`, as is the same comment after any key.

## What it costs

A comment is lost without a word, which is what `canonical_round_trip` checks
comments separately to catch. It is reachable only through canonical text, an
opt-in format. YAML's `? [ # why` does not lose it: `fig get k.yaml -o
canonical` gives `[]: // why` with the value on the next line, so the comment
survives, on the value. The JSON family refuses a key that is a container.

## Found by

Not by the nightly `Fuzz` job yet. Its last run,
[42239902099](https://github.com/diaryx-org/fig/actions/runs/42239902099), was
at 7344d3aa, before `fix(canonical): keep a block comment after a one-line
container's close at its bottom` merged, so the nightly has not run at a commit
with that fix. All 11,827 inputs of that run's artifact, its crash included,
pass at 17e31b51. A throwaway mutator over that corpus, with the bytes of
canonical's punctuation and comments, then hit this once in 319 million inputs
over 150 seconds, in a release build. It is a guess that `Fuzz` reaches the
same input; if it does, [#13](https://github.com/diaryx-org/fig/issues/13)
gets this crash next.

## A way to fix it

The parser refuses a comment after a key, so the printer cannot write this one
after the key's close. It can write the key as a block when it has a trailing
comment, in the `KeyValue` arm:

```text
{
  [ // why
  ]: 1
}
```

That reparses to the same slot on the same key. A throwaway version of this,
reverted and not on any branch, stopped the panic on each failing input above,
on `{[// a\n]: {[// b\n]: 2}}` and on `{&x [// a\n]: 1}`.
`cargo test -p fig-core canonical` and `cargo test -p fig-fuzz` passed, the
artifact's inputs still passed, and the mutator then found nothing in 427
million more. The full gate was not run against it. It changes what the
canonical printer writes for these inputs, so it wants a `Behavioural-change:`
trailer.

## Done when

`{[//\n]:1}`, `{{//\n}:1}` and `[{[//\n]:1}]` round-trip in
`rust/fig-core/src/canonical/parser.rs`'s tests, the comment printing beside
its key's opener, and `{[//\n]:1}` is one of `canonical_seeds` in fig-fuzz.
