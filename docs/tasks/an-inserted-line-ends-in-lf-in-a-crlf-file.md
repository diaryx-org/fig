```fig
title = An inserted line ends in LF in a CRLF file
part_of = [Tasks](/docs/tasks/tasks.md)
status = in-progress
created = 2026-11-04
updated = 2026-11-04
description = A key inserted into a CRLF file is written with a bare LF, so one edit leaves the file with mixed line endings; insert_block_key and the flow insert spell the line break as a literal \n
```

# An inserted line ends in LF in a CRLF file

**Status.** Fixed in fig, in `fix(editor): a line written into a CRLF file ends
in CRLF` (2026-11-04); what is left is recording fig-conformance again, once
that is on `main`. Text the editor writes goes through `splice_lines`, which
ends each line as the line it follows does, the rule for a file with both
endings. A line break a value reads as text keeps its LF: TOML's `"""` and
plist's `<string>` take a carriage return into the string, and `splice_lines`
compares the two readings. Deleting the first item of a flow list now takes
the whole CRLF too. The same commit's `tests/crlf.rs` holds these.

Scored against the recordings through `fig-conform`, 301 of the edit cases
change, all in the CRLF suites. 292 differ only in `\r`. The other 9 are deletes,
and a `setSequence`, that no longer leave a line of spaces, which is what the same
edit does in an LF file. Every suite stays at 100% `faithful`.

An edit that adds a key to a CRLF file writes the new line with a bare LF.
The rest of the file keeps its CRLFs, so one `set` leaves it with mixed line
endings, and every later diff or `git` normalisation has that line to answer
for. The values all read back correctly: nothing is lost, the file's spelling
is what changes.

## Repro

At e202e50b (fig 7.0.0):

```console
$ printf 'a: 1\r\nb: 2\r\n' > a.yaml
$ fig set a.yaml c 3
$ cat -A a.yaml
a: 1^M$
b: 2^M$
c: 3$
```

TOML does the same at the root and under a `[t]` header, and so does INI.
JSON does it in the middle of the file, on the line it splits:

```console
$ printf '{\r\n  "a": 1\r\n}\r\n' > a.json
$ fig set a.json c 3
$ od -An -c a.json | tr -s ' ' | tr -d '\n'
 { \r \n " a " : 1 , \n " c " : 3 \r \n } \r \n
```

Found through fig-quickjs's HCL module, which goes through the same splice.

## Cause

`insert_block_key` in `rust/fig-core/src/editor/mod.rs` spells the line break
after the new entry, and the one before it when the anchor is not at a line
start, as a literal `b'\n'`. JSON's case is the flow path
(`insert_flow_map_entry`). The other inserting splices have not been checked
one by one.

[The trailing-comment task](/docs/tasks/closed/trailing-comment-splits-crlf.md)
asked for "the line ending the document already uses", and said the other
splices did that. These do not.

## fig-conformance

Its CRLF edit suites record today's splices, for example
`"splice": [25, 25, "added_int=42\n"]` in `suites/dotenv/edits/crlf.json`, and
do not mark them `suspect`. That is a recording of what the editor does rather
than a statement that LF is right, but the fix changes those cases, so they
have to be recorded again with it.

## Done when

A line inserted into a CRLF file ends in CRLF in every format, a test in
`rust/fig-core` holds it for a block and a flow insert, and the fig-conformance
recordings match. A file with both endings needs a rule too: the line the
insert follows is the obvious one.
