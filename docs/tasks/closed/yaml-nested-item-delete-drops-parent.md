```fig
title = Deleting an item of a YAML list nested on its parent's marker line drops the parent
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
description = Deleting `[0][0]` from `- - a` / `  - c` writes `  - c`: the outer item's marker goes with the inner item, so `[[a, c]]` reads back as `[c]`, and the command reports success
status = done
created = 2026-10-13
updated = 2026-10-13
```

# Deleting an item of a YAML list nested on its parent's marker line drops the parent

**Status.** Done, in `fix(editor): a YAML delete on a parent's marker line
keeps the parent` (346db34, 2026-10-13). Deleting the first item or key that
sits on its parent's `-` line moves the next sibling up onto that line (`- -
c`, `- j: w`), or leaves the bare `-` when a comment line comes first, at
the root, under a key and deeper. `deleteKey` and `removeSeqItem` now
compare the document before and after and refuse with
`DeleteChangedOtherValues` when any other value changed, so a shape still
mishandled (two-deep compact sequences, a parser bug filed apart) is refused
rather than corrupted.

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
