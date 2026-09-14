```fig
title = `fig delete` never reaches `deleteContainer`, so a section, table or block cannot be deleted from the command line
description = the CLI's delete routes to `deleteKey`, which the engine refuses for a section node (`CannotDeleteSection` / `CannotDeleteTable` / `CannotDeleteContainer`); the container op that exists for exactly this is reachable from the bindings and not from the CLI
status = open
created = 2026-09-14
updated = 2026-09-14
part_of = [tasks](tasks.md)
```

# `fig delete` never reaches `deleteContainer`, so a section, table or block cannot be deleted from the command line

**Repro.** Compiled INI:

```
$ printf '[core]\nbare = false\n[user]\nname = x\n' > g.ini
$ fig delete g.ini user
error: CannotDeleteSection
note: if g.ini itself does not parse, `fig check g.ini` says where.
```

The same for a TOML `[table]`, a fig block container, and any runtime
section — a git config section or an HCL block in fig-quickjs. The Rust
and TypeScript editors have `delete_container` for it and the CLI has no
spelling: `fig delete --help` names a mapping entry and a list item only.
The error also arrives as its enum name with a hint about parse errors,
which is not what went wrong.

**Where.** `cli/edit_ops.zig`, `applyToFile` / the `delete_key` op;
`editor.zig`'s `deleteKey` refuses through `refuse(.delete)` and
`deleteContainer` is the op it points at.

**Done when** `fig delete <file> <path>` on a path whose value is a section
node deletes the container — either by routing to `deleteContainer` when
`deleteKey` refuses with the section error, or under a flag if the
distinction should stay visible — and the refusal, where it still applies,
says "a section; use …" rather than `CannotDeleteSection`.
