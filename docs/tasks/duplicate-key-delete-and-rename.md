```fig
title = Deleting or renaming a duplicated key leaves the key readable
part_of = [Tasks](/docs/tasks/tasks.md)
description = In INI, dotenv, .properties and plist, which read a key written twice as one, a delete or rename takes one occurrence and the key still reads with the other's value (22 fig-conformance edit cases)
status = open
created = 2026-10-14
updated = 2026-10-14
```

# Deleting or renaming a duplicated key leaves the key readable

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
