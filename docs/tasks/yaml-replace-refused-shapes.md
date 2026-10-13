```fig
title = Three YAML value shapes cannot be replaced
part_of = [Tasks](/docs/tasks/tasks.md)
description = Replacing a YAML value is refused after a tab before its comment (`a: 1\t# c`), for an empty tagged value (`t: !!str`), and for an explicit key's value (`? k` / `: v`)
status = open
created = 2026-10-13
```

# Three YAML value shapes cannot be replaced

**Repro.** On `main` at 5a1a559, each refused (would not read back as that edit asks):

```sh
printf 'a: 1\t# c\nb: 2\n' > t.yaml;  fig replace t.yaml a 42
printf 't: !!str\nu: 1\n' > s.yaml;   fig replace s.yaml t x
printf '? k\n: v\n' > k.yaml;          fig replace k.yaml k 42
```

Before 5a1a559 the edit reported success and wrote wrong data; the read-back guard now refuses it and leaves the file as it was. fig-conformance's recording (53fc34c) has these cases as refusals. Before: the tab case wrote `42# c` (no separating space, so the comment became value text), the tagged case `x !!str` (the tag after the value), and the explicit key lost its `? k : v` shape.

**Done when** each replaces cleanly (`a: 42\t# c`, `t: !!str x`, `? k` / `: 42`), keeping the comment, tag and explicit-key form, with tests in `languages/yaml/tests/editor.zig` and the fig-conformance cases in `tabs.yaml`, `tags.yaml` and the explicit-key sources succeeding. Inserts into anchored flow lists (`- &i [1, 2]`) are the same misreading as [the delete task's](/docs/tasks/delete-splices-taking-other-values.md) and go with it.
