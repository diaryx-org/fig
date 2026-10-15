```fig
title = The bindings and the JavaScript twins keep a replaced string's quote style
part_of = [Tasks](/docs/tasks/tasks.md)
status = done
description = The TypeScript and Rust replaceValue and setValue now keep a replaced string's quote style, the JavaScript YAML, TOML and JSON5 twins declare a string renderer, and the conformance edit suites are re-recorded
created = 2026-10-15
updated = 2026-10-15
```

# The bindings and the JavaScript twins keep a replaced string's quote style

**Status.** Done, in `feat(bindings): the Rust and TypeScript editors keep
a replaced string's quote style` (2026-10-15), the rest of the
[quote-style proposal](/docs/proposals/keep-quote-style.md) after its core
half (`renderString` on YAML, TOML and JSON's `Language`, the engine's
`replaceStringAtPath` and `setString`, the C ABI's
`fig_editor_replace_string` and `fig_editor_set_string`, and the CLI's
`replace`, `set` and `patch` through them). The Rust `replace_value`,
`set_value` and their `*_with` twins, and the TypeScript `replaceValue`,
`setValue` and `*With`, hand a string to those exports beside its
rendering; the `*Raw` methods are as they were. `Renderer` names `string`
in Rust and TypeScript, with `old_value` on `RenderArgs`, and the YAML,
TOML and JSON5 twins declare one spelled as their Zig originals. The
fig-conformance edit suites were re-recorded in the same change: 88
`replaceValue` answers moved (yaml 39, toml 44, json5 5), and the compiled,
native and JavaScript languages score every edit case exact and faithful.

## What was left

- **The TypeScript binding.** `replaceValue`, `setValue` and their `*With`
  twins render a `Value` and call `replaceValueRaw` / `setValueRaw`. For a
  string they should call the new exports with the string beside its
  rendering, as the CLI's `renderEdit` does. The `*Raw` methods stay as they
  are: their text is the caller's own spelling.
- **The Rust binding.** The same, in `bindings/rust/fig/src/editor.rs`.
- **The JavaScript twins.** `bindings/typescript/js/yaml.js`, `toml.js` and
  `json5.js` declare their renderers (`renderers: ["key_name"]`); each
  needs `string`, spelled as its Zig original spells it, or a twin and its
  original answer the same edit differently.
- **Authoring a string renderer.** `Renderer` in
  `bindings/typescript/src/wire.ts` and in `bindings/rust/fig/src/language.rs`
  and `helper.rs` does not name `string` yet, so a language written in either
  cannot declare one. The C vtable has the slot (`render_string`, after
  `split_stream`) and the helper wire carries it.
- **fig-conformance.** Its edit expectations are fig's answers through the
  TypeScript `replaceValue`, so the first two items move every
  `replaceValue` case whose source string is single-quoted or a TOML literal.
  Re-record them in the same change, with the twins, so `scores/js.json` and
  `scores/compiled.json` move together.

## Done when

`replaceValue(path, "info")` on `LOG: "debug"` writes `LOG: "info"` from
TypeScript and Rust alike, every JavaScript twin answers the same, and the
conformance scores are re-recorded and no lower.
