```fig
title = `fig patch` into a runtime target is refused
description = `fig patch target.tkv overlay.yaml` exits with `UnsupportedRuntimePatch`; the merge is a compiled-editor operation and has not been made to go through `Editor(Runtime.Language)`
status = done
created = 2026-09-10
updated = 2026-09-26
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# `fig patch` into a runtime target is refused

**Status.** Done, in `feat(cli): patch into a runtime language`.
`fig.Patch.applyTo` takes a `Patch.Target`, which is a compiled
`SerializeFormat` or a runtime entry whose printer renders each subtree,
and `patch_ops.applyToSlice` fills the runtime arm with
`Editor(Runtime.Language)` as `edit_ops.route` does. `fig.Patch.apply`
is unchanged. `tools/cli-lang-check.sh` patches a `.tkv` file.

**Repro.** With a `languages.figl` naming the `tinykv_helper` example:

```bash
$ printf 'a=1\n' > t.tkv
$ printf 'b: 2\n' > o.yaml
$ fig patch t.tkv o.yaml
error: UnsupportedRuntimePatch
```

Every other action goes through a runtime format — `set`, `insert`,
`delete` and `comment` edit through `Editor(Runtime.Language)` the same as
a compiled one, and a patch is those operations in a loop.

**Why it is deferred.** `src/cli/patch_ops.zig` dispatches on the
compiled `Format` to pick the editor type, and its merge walks the target
through that concrete editor; the runtime arm (`_ =>`) returns the error
rather than instantiating `Editor(Runtime.Language)` with the entry's
`Type`. It is the same `_` arm `edit_ops.route` already fills for the
single-operation actions, applied to the merge loop, and was left out of
the cli 4.1 step to keep it to the carrier.

**Done when** the repro leaves `t.tkv` as `a=1\nb=2\n`, and
`tools/cli-lang-check.sh` runs a `patch` alongside its `set`.
