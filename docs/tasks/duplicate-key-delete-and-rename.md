```fig
title = Deleting or renaming a duplicated key leaves the key readable
part_of = [Tasks](/docs/tasks/tasks.md)
description = In INI, dotenv, .properties and plist, which read a key written twice as one, a delete or rename takes one occurrence and the key still reads with the other's value (22 fig-conformance edit cases)
status = done
created = 2026-10-14
updated = 2026-10-14
```

# Deleting or renaming a duplicated key leaves the key readable

**Status.** Done, in `fix(editor): a delete or rename of a repeated key
takes every spelling` (2026-10-14). Every op takes every spelling; none
refuses. `deleteKey` and `replaceKeyAtPath` (so `fig delete`, `fig
rename`, `renameKey`) take the spelling the entry's span names and ask
again while the entry's value still lies past its span, the parser's mark
of a key written again later (`spelledAgain` in `src/editor.zig`). The
guard compares the document before the first spelling with the one after
the last, so `duplicateSurfaced` is gone. `deleteContainer` first deletes,
spelling by spelling, every entry of the section written more than once,
then the section. Each op is one edit, refused whole if any spelling's
splice is.

Why every spelling rather than a refusal: the reader already settles what
the key is. It reads as one entry, the INI warning says a reopened
section's "entries merge into the first occurrence", and the engine already
treats such an entry as one thing. An INI section rename rewrites every
reopening, and `deleteContainer` gathers every header. A refusal would
leave nothing to delete or rename a repeated key with, short of editing
the text.

`replaceValAtPath` and `set` already act on the entry as it reads: they
write the value node, which is the last spelling's, so the key reads back
as asked. The earlier spellings are shadowed before and after, so they are
left alone. `moveKey` and `reorderKeys` are outside this task and
unchanged. Under the trigger above, figl's container written over several
lines is not a repeated key. Its value is not past its span, so
[its rename](/docs/tasks/figl-rename-multi-line-container.md) is
unchanged here.

fig-conformance at 66be227, scored with an all-format wasm: the 22 cases
are now faithful and every one succeeds, so none is a new refusal. ini
1253/1253 faithful (1242 exact), dotenv 709/709 (705), .properties
1007/1007 (1003), plist 3499/3499 (3496). Every other suite scores as
before. They await re-recording.

**Repro.** At eca3f34, with the CLI:

```sh
printf 'A=1\nB=2\nA=3\n' > x.env
fig delete x.env A; fig get x.env A          # 3: A is still there

printf 'a=1\nb=2\na=3\n' > x.properties
fig rename x.properties a z                  # z=1 / b=2 / a=3: a still reads 3

printf '[s]\na = 1\n[t]\nb = 2\n[s]\na = 4\n' > r.ini
fig delete r.ini s.a; fig get r.ini -o json  # {"s":{"a":"4"},"t":{"b":"2"}}
```

These formats read a key written twice as one entry: dotenv, .properties
and plist take the last value, and INI merges a reopened section into the
first. The editor acts on one occurrence, so after a delete the key still
reads, with the other's value, and after a rename it reads under both
names. Deleting a reopened INI section leaves its other half.

fig-conformance (66be227) marks 22 cases `suspect`: in INI, renames and
deletes of keys and sections in duplicate-keys.ini, everything.ini and
reopened-section.ini (11); in dotenv, `A` in duplicate-keys.env and
`export` in export-as-a-key.env (4); in .properties, duplicate-keys and
everything (4); in plist, dup-keys and typed-and-duplicate (3). YAML,
JSON and ZON keep both entries in the tree, so a delete of one is what it
says there.

**Done when** in every format that reads duplicates as one entry, a
delete, rename or delete-container of a duplicated key or section either
takes every occurrence, so the document no longer reads the old key, or
refuses with a status that says the key is written more than once. Those
22 cases then re-record as faithful (a refusal of the reference counts as
faithful), with no `suspect` mark.
