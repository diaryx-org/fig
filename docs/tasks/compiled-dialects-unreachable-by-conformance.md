```fig
title = Compiled TOML 1.0, YAML 1.1 and YAML streams cannot be scored by fig-conformance
description = The C ABI parses a format only at its default spec and exposes no stream splitter, so 738 cases the old Zig scoreboards ran against the compiled formats have no compiled run since the corpora moved out
status = open
created = 2026-10-11
updated = 2026-10-11
part_of = [Tasks](/docs/tasks/tasks.md)
```

# Compiled TOML 1.0, YAML 1.1 and YAML streams cannot be scored by fig-conformance

**Repro.** In fig-conformance, against a wasm with every format compiled in:

```
FIG_REFERENCE_WASM=… npm run parse:compiled -- toml yaml
toml  1.0 valid    not run: no dialect named "toml-1.0"
toml  1.0 invalid  not run: no dialect named "toml-1.0"
yaml  1.1          not run      (compiled; the JS language has `yaml-1.1`)
yaml  stream       not run: fig's API offers no stream splitter
```

Until b6df12e the Zig scoreboards ran these against the compiled parsers
directly: toml-test 1.0 (209 valid, 495 invalid), fig's YAML 1.1 resolution
fixtures (14) and the yaml-test-suite streams (19, and 1 that must fail).
The suites now reach a language only through its registered dialects and
the bindings, and neither reaches these.

**Cause.** A compiled format's other spec versions are selectable only
through the CLI's `--spec`; the registry has no dialect row for them, so
`fig_format_by_name("toml-1.0")` finds nothing. Stream splitting is
`Embed.extractStream`, which no binding exposes and the wire has no
request for.

**Done when** the compiled TOML and YAML register `toml-1.0` and
`yaml-1.1` dialects (the JS YAML language already has `yaml-1.1`), a stream
can be split through the contract (the `splitStream` declaration of
[core only](/docs/proposals/core-only.md) § 4.5), and fig-conformance's
compiled scores include all four checks at the numbers above.
