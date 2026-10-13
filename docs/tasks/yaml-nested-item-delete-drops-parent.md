```fig
title = Deleting an item of a YAML list nested on its parent's marker line drops the parent
part_of = [Tasks](/docs/tasks/tasks.md)
description = Deleting `[0][0]` from `- - a` / `  - c` writes `  - c`: the outer item's marker goes with the inner item, so `[[a, c]]` reads back as `[c]`, and the command reports success
status = open
created = 2026-10-13
```

# Deleting an item of a YAML list nested on its parent's marker line drops the parent

**Repro.** On `main` at 5dfb6ab:

```sh
printf -- '- - a\n  - c\n' > n.yaml
fig delete n.yaml '[0][0]'      # exit 0
cat n.yaml                      #   - c
```

The document was `[[a, c]]` and is now `[c]`: one level of nesting is
gone, and nothing says so. The same shape at any depth, and under a key
(`k:\n  - - a\n    - c`), is likely affected.

**Cause.** The inner list's first item shares its line with the outer
item's `-`. The line delete that removes an item removes the whole line,
marker included. When the inner item is the only one, the editor now
writes `- []` in its place (9fc2b4f, `emptiedBlockSeq` in `src/editor.zig`);
when it is not, nothing keeps the outer marker.

**Done when** deleting the first item of a list that starts on its
parent's marker line keeps the parent: the next item moves up onto the
marker line (`- - c`), or the edit is refused, and the file reads back with
every other value unchanged. A test covers it at the root and under a key.
