```fig
title = The Rust editor spells a key through the format's printer, which for plist is `<string>k</string>`
description = `Editor::insert_value(&[], "n", 42)` on a plist fails to reparse — the key is rendered by `value_text(Value::Str("n"), Plist)`, which prints a typed `<string>` element, and the entry renderer then escapes that into `<key>&lt;string&gt;n&lt;/string&gt;</key>`; every format whose scalar spelling is not the bare text is affected
status = open
created = 2026-09-10
updated = 2026-09-10
part_of = [tasks](tasks.md)
```

# The Rust editor spells a key through the format's printer

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
