```fig
title = Everything-on `zig build test` fails to compile
description = `zig build test -Dxml=true -Dplist=true -Dcanonical=true` fails in `patch.zig` with an error-set mismatch on plist's `getLeadingComment`, while `zig build conformance` (which forces the same configuration) passes
status = done
created = 2026-09-04
updated = 2026-09-04
part_of = [tasks](tasks.md)
```

# Everything-on `zig build test` fails to compile

**Status.** Done, in `fix(patch): compare comment-op errors instead of
switching on them; test the CLI everything-on` (2026-09-04). `patch.zig`
compares the error (`err == error.CommentsUnsupported`, which Zig accepts
for an error outside the set) rather than naming it in a `switch` arm, and
the `conformance` step — which `check` folds in — now also builds and runs
the CLI's test root against the everything-on library, so the whole
configuration is proven, not just the library's half.

**Repro.** On `main` at c9761cd and after:

```
zig build test -Dxml=true -Dplist=true -Dcanonical=true
src/patch.zig:356:17: error: expected type
  '@typeInfo(@typeInfo(@TypeOf(editor.Editor(languages.plist.plist.Language).getLeadingComment)).@"fn".return_type.?).error_union.error_set',
  found 'error{CommentsUnsupported}'
```

`patch.zig`'s `carryLeading` switches on `error.CommentsUnsupported` from
`self.editor.getLeadingComment(...)`, but plist hooks all six comment ops
(its `<!-- -->` has no line marker), and its `getLeadingComment` hook's
error set does not contain `CommentsUnsupported`, so the `switch` arm names
an error the set cannot produce. Every default build has plist gated off,
which is why nothing sees it.

**Why `conformance` passes.** `zig build conformance` builds
`BuildOptions.all_on` too, but only the library module's test root; the
failing compile is one of the other `zig build test` roots, so the
everything-on configuration is proven to build only for the library. Either
`patch.zig`'s switch should be written so that an arm for an error outside
the set is not a compile error (an `else` that re-raises, or a check that
the set has the member), or plist's hook should share the engine's error
set.

**Done when** `zig build test -Dxml=true -Dplist=true -Dcanonical=true` is
green, and `zig build check` runs that configuration for every test root, so
that it cannot regress unseen. Found while verifying the pluggable-formats
steps (docs/proposals/pluggable-formats.md §9); it predates them.
