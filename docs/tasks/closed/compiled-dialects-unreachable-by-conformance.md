```fig
title = Compiled TOML 1.0 and YAML 1.1 cannot be scored by fig-conformance
description = The C ABI parses a format only at its default spec, so 718 cases the old Zig scoreboards ran against the compiled formats have no compiled run since the corpora moved out
status = done
created = 2026-10-11
updated = 2026-10-13
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# Compiled TOML 1.0 and YAML 1.1 cannot be scored by fig-conformance

**Status.** Done, in `test: toml-1.0 and yaml-1.1 parse at their spec by
name, through the C ABI, the CLI and the native modules` (f74ffe2,
2026-10-13). Nothing was unreachable by then: the contract registers every
version a language lists as a dialect of its own, so `toml-1.0` and
`yaml-1.1` resolve by name. Tests now hold it, and fig-conformance scores
toml-test 1.0 at 209/209 valid and 495/495 invalid, and the YAML 1.1
fixtures at 14/14.

**Repro.** In fig-conformance, against a wasm with every format compiled in:

```
FIG_REFERENCE_WASM=… npm run parse:compiled -- toml yaml
toml  1.0 valid    not run: no dialect named "toml-1.0"
toml  1.0 invalid  not run: no dialect named "toml-1.0"
yaml  1.1          not run      (compiled; the JS language has `yaml-1.1`)
```

Until b6df12e the Zig scoreboards ran these against the compiled parsers
directly: toml-test 1.0 (209 valid, 495 invalid), fig's YAML 1.1 resolution
fixtures (14) and the yaml-test-suite streams (19, and 1 that must fail).
The suites now reach a language only through its registered dialects and
the bindings, and neither reaches these. (The streams were the fourth;
`splitStream` reaches them since 2026-10-12, and both the compiled YAML
and its twin split all 19.)

**Cause.** A compiled format's other spec versions are selectable only
through the CLI's `--spec`; the registry has no dialect row for them, so
`fig_format_by_name("toml-1.0")` finds nothing.

**Done when** the compiled TOML and YAML register `toml-1.0` and
`yaml-1.1` dialects (the JS YAML language already has `yaml-1.1`), and
fig-conformance's compiled scores include all three checks at the numbers
above.
