```fig
title = The Rust and TypeScript editors spell a NestedText value through the printer, which is a `>` block
description = `Editor::replace_value(&[Key("name")], "h2")` on a NestedText document lands `name: > h2` — the value is rendered by `value_text(Value::Str("h2"), Nestedtext)`, which prints a scalar root as a `> h2` string block, and the format's tail renderer then blocks that text again; every format whose scalar root is not the bare text and whose editor renders tails is affected
status = open
created = 2026-09-12
updated = 2026-09-12
part_of = [tasks](tasks.md)
```

# The Rust and TypeScript editors spell a NestedText value through the printer

**Repro** (`bindings/rust`; NestedText is in the default feature set):

```rust
let mut ed = Editor::open(b"name: fig\n", Format::Nestedtext)?;
ed.replace_value(&[Segment::Key("name")], "h2")?;
assert_eq!(ed.source()?, "name: h2\n");   // is "name: > h2\n"
ed.insert_value(&[], "new", "two\nlines")?;   // lands "new:\n    > > two\n    > > lines\n"
```

The same edits from the command line — `fig set f.nt name h2` — land
`name: h2`, because the CLI hands the editor bare text and NestedText's
`renderTail` spells the block form when the text needs one.

**Why.** `value_text` in `bindings/rust/fig/src/value.rs` (and `valueText`
in `bindings/typescript/src/value.ts`) serializes the value as a scalar
document in the format and hands the bytes to the editor as splice text.
The C header's contract for a print of a scalar root is "as the scalar
stands alone in the format" — for NestedText that is a `> text` block,
which is right for `fig convert` and wrong as splice text: the editor's
`renderTail`/`renderItem`/`renderEntry` take the *plain* text and add the
`>` lines themselves, so the block is blocked twice. INI, dotenv and
`.properties` are unaffected only because a scalar stands alone in them as
its bare (escaped) text, which is also what their raw splice wants.

Found while writing the runtime NestedText twins (`fig-lua`,
`fig-quickjs`): the twins reproduce the compiled printer exactly, so the
in-process edit test met the same doubled block through the same call, and
the twins hold their edits to the compiled format through the CLI instead.

**Fix.** The bindings need to know when a scalar's splice text is its plain
text rather than its standalone spelling: a format whose editor renders
tails (`Editor.hasRenderer(.tail)`) takes plain text there. That is a
question the C ABI does not answer today — a `fig_format_has_renderer(format,
renderer)` beside `fig_format_capabilities` would, for compiled and runtime
formats alike — after which `value_text` passes a scalar's text through for
such a format and serializes everything else as it does now.

**Done when** the repro lands `name: h2` and a nested `> two` / `> lines`
block, and a test in `bindings/rust/fig/tests` and `bindings/typescript/test`
holds NestedText to it.
