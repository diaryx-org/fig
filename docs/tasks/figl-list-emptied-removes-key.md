```fig
title = Deleting the only element of a figl list removes the key
part_of = [Tasks](/docs/tasks/tasks.md)
description = Deleting the only element of a figl list removes the key and its header, where YAML and every flow list now leave an empty list (`[]`)
status = open
created = 2026-10-13
```

# Deleting the only element of a figl list removes the key

**Repro.** On `main` at 5dfb6ab:

```sh
printf 'a = 1\nreplacements[]\n> file = README.md\n' > r.figl
fig delete r.figl 'replacements[0]'    # exit 0
cat r.figl                             # a = 1
```

`replacements` is gone. In YAML the same delete now leaves `key: []`
(9fc2b4f), and in a flow list, JSON and TOML it leaves an empty list, so a
caller that deletes the last item of a list and then appends to it works in
every format but figl.

**Done when** deleting the only element of a figl list leaves
`replacements = []`, keeping the key's comments, or the decision that figl
removes the key instead is written down here and in the spec, with a test
either way.
