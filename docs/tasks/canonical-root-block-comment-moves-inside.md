```fig
title = A block comment after the root's closing bracket moves inside it
part_of = [Tasks](/docs/tasks/tasks.md)
status = open
description = In the canonical form, a /* */ comment on the line that closes a multi-line root container prints after its opening bracket and reparses as its first child's leading comment, so canonical_round_trip fails; the nightly Fuzz job has failed on it since 2026-10-20
created = 2026-10-26
updated = 2026-10-26
```

# A block comment after the root's closing bracket moves inside it

**Where.** `rust/fig-core/src/canonical/parser.rs`, `parse_once`, against
`printer.rs`, `print` and `container`. `parse_once` binds any comment on the
root's last line, after its closing bracket, as the root's trailing comment.
The printer writes a block container's trailing comment beside its opening
bracket (`{ /* b */`), and the parser takes a comment there back as the
container's own trailing only when it is a line comment
(`capture_open_trailing`). A block comment there is left for
`collect_leading`, which gives it to the first child, or to the dangling run
when there is none.

**Repro.** At 99312a5, a `/* */` comment after the close of a multi-line root
`{ … }` or `[ … ]`:

| input | first print | its reparse prints |
|---|---|---|
| `{\n"k": 1\n} /* b */` | `{ /* b */\n  "k": 1\n}\n` | `{\n  /* b */\n  "k": 1\n}\n` |
| `{\n// a\n} /* b */` | `{ /* b */\n  // a\n}\n` | `{\n  /* b */\n  // a\n}\n` |
| `[\n// a\n] /* b */` | `[ /* b */\n  // a\n]\n` | `[\n  /* b */\n  // a\n]\n` |

`ast.eql` holds and `ast.comments_eql` does not, so
`fig_fuzz::canonical_round_trip` panics with "the reparsed tree's comments
differ from the parsed one's". The smallest input is the 12 bytes
`{\n//\n}/**/`.

The same comment does not move in three nearby shapes. A line comment
(`} // b`) prints as `{ // b` and reparses as the root's trailing comment.
An inline root (`{} /* b */`) prints the comment after its close. A nested
container's close (`"k": {\n// a\n} /* b */`) goes through `finish_element`,
which puts the comment in the container's dangling run when the container is
multi-line, so it prints inside the closing bracket and stays there.

**What it costs.** A block comment written after a document's closing
bracket stays in the document but changes owner the first time the document
is printed. The bigger cost is that the nightly `Fuzz` workflow has
failed on `canonical_round_trip` every day since 2026-10-20 (each day's
failing input is under `fuzz/artifacts/canonical_round_trip/` in the run's
`fuzz-corpus-<run id>` artifact). While it stays red, a second fuzz finding
would go unnoticed.

**Done when** the three inputs above round-trip under `canonical_round_trip`,
`{\n//\n}/**/` is a seed or a unit test, and the nightly `Fuzz` run is green.
One way to get there is for `parse_once` to do what `finish_element` does for a
nested container, and append a comment after a multi-line root's close to its
dangling run.
