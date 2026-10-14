```fig
title = Renaming a YAML flow key written against its colon makes another key
part_of = [Tasks](/docs/tasks/tasks.md)
description = `{ "k":v }` renamed becomes `{ renamed:v }`, which reads as the key `renamed:v` with no value
status = open
created = 2026-10-14
updated = 2026-10-14
```

# Renaming a YAML flow key written against its colon makes another key

**Repro.** At eca3f34, with the CLI:

```sh
printf 'm: { "k":v, q: 1 }\nn: ["e":f]\n' > f.yaml
fig rename f.yaml m.k renamed
fig rename f.yaml 'n[0].e' renamed
cat f.yaml        # m: { renamed:v, q: 1 } / n: [renamed:f]
fig get f.yaml -o json   # {"m":{"renamed:v":null,"q":1},"n":["renamed:f"]}
```

In a flow collection YAML lets a `:` follow a quoted (JSON-like) key
directly, but a plain scalar runs through a `:` that no blank follows. The
rename writes the new key plain and keeps the colon against it, so
`renamed:v` becomes one scalar: the entry `m.renamed` is gone and a key
`renamed:v` with a null value takes its place. The item `"e":f` becomes
the string `renamed:f`. With a blank after the colon (`{ "k": v }`) the
rename is right, and JSON5 keeps the quotes (`{ "renamed":1 }`).

fig-conformance (66be227) marks the two cases `suspect`: `m.k: rename`
and `n.[3].e: rename` in flow.yaml.

**Done when** renaming a key that its colon follows directly keeps the
document reading as the rename asked: the new key keeps the old one's
quoting, or a blank goes after the colon. Both cases then re-record as
faithful, and a test in `languages/yaml/tests/editor.zig` covers both
shapes.
