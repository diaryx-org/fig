```fig
title = The Rust editor spells a key through the format's printer, which for plist is `<string>k</string>`
description = `Editor::insert_value(&[], "n", 42)` on a plist fails to reparse — the key is rendered by `value_text(Value::Str("n"), Plist)`, which prints a typed `<string>` element, and the entry renderer then escapes that into `<key>&lt;string&gt;n&lt;/string&gt;</key>`; every format whose scalar spelling is not the bare text is affected
status = done
created = 2026-09-10
updated = 2026-09-22
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# The Rust editor spells a key through the format's printer

**Status.** Done, in `fix(editor): a binding hands the editor a key's name
and a value's splice text, not a standalone document` (2026-09-22). The
rename half is `replaceNamedKey` and the C ABI's
`fig_editor_replace_named_key` / `fig_embed_replace_named_key`, which the
Rust `replace_key`, the TypeScript `replaceKey` and the CLI's `edit --key`
now go through; plist gained a `renderKey`, since its key span is the whole
`<key>…</key>` element. The repro below still failed after the insert half,
and not for the key: `42i64` printed as a whole plist document — XML
declaration, DOCTYPE, `<plist>` — so "the value path is the intended one"
was wrong. A `splice` bit in `FigSerializeOptions`, which the bindings'
splice paths and `fig patch` set, now renders a value as the editor takes
it — for plist through the printer's `printSplice`, the bare element — so
`fig patch` into plist, refused outright before, works too. A value
serialized to be written out keeps its wrapper.

**Status (2026-09-12):** the insert half is done — `447bcda` added
`insertNamedKey` and the C ABI's `fig_editor_insert_named_key` /
`fig_embed_insert_named_key`, and the Rust and TypeScript `insert_value`
family hands the key over as a name for the format to spell. What remains
is `replace_key` (Rust) / `replaceKey` (TypeScript), which still spell the
new key through `value_text`; the repro below for plist now lands the key,
and the same call on ZON or NestedText spells `"k"` / `> k`. See also
[the value path for a format that renders tails](/docs/tasks/closed/rust-editor-spells-a-nestedtext-value-through-the-printer.md),
the sibling of this one on the value side.

**Repro** (`bindings/rust`, with the `plist` feature):

```rust
let mut ed = Editor::open(b"<dict>\n  <key>a</key>\n  <string>x</string>\n</dict>\n", Format::Plist)?;
ed.insert_value(&[], "n", 42i64)?;   // Err(Parse("failed to parse input"))
```

The same edit from the command line — `fig insert f.plist n 42` — lands as
`<key>n</key>` over `<integer>42</integer>`, because the CLI hands the
editor bare text and plist's `renderEntry`/`renderValue` spell it.

**Why.** `Editor::insert_value`, `insert_value_with` and `replace_key` in
`bindings/rust/fig/src/editor.rs` build the key text with
`value_text(&Value::Str(key), self.format)`, which serializes the key as a
scalar document in the format. For every line-oriented format that is the
bare text and the call is a no-op; for plist a scalar prints as its typed
element, so the key arrives at `fig_editor_insert_key` as
`<string>n</string>`, and `renderEntry` escapes it into the `<key>`. The
value goes the same way — `42i64` prints as `<integer>42</integer>`, which
`renderValue` then splices verbatim (its `<` escape hatch), so the value
happens to be right and the key is not.

Found while writing `fig-lua`'s `plist.lua` twin, whose in-process test met
the same failure through the same call — the twin reproduces the compiled
format exactly, which is how it surfaced.

**Fix.** A key is text, not a value: pass it to the C API as written and let
the format's own `renderKey`/`renderEntry` spell it — which is what the
CLI does, and what `fig_editor_insert_key`'s contract already says of its
`key` argument. The value path is the intended one and stays.

**Done when** the repro lands `<key>n</key>` and `<integer>42</integer>`
on their own lines, and a test in `bindings/rust/fig/tests` holds plist
to it.
