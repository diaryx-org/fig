```fig
title = The bindings and the JavaScript twins keep a replaced string's quote style
part_of = [Tasks](/docs/tasks/tasks.md)
status = open
description = The TypeScript and Rust replaceValue and setValue still splice the printer's spelling of a string, and the JavaScript YAML, TOML and JSON5 twins declare no string renderer; adopting both moves the conformance edit suites
created = 2026-10-15
updated = 2026-10-15
```

# The bindings and the JavaScript twins keep a replaced string's quote style

**Status.** Open. The core half of the
[quote-style proposal](/docs/proposals/keep-quote-style.md) has landed:
`renderString` on YAML, TOML and JSON's `Language`, the engine's
`replaceStringAtPath` and `setString`, the C ABI's
`fig_editor_replace_string` and `fig_editor_set_string`, and the CLI's
`replace`, `set` and `patch` through them. This is the rest.

## What is left

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
