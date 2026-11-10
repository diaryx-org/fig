```fig
title = The lossless envelope's placeholders read as HTML on docs.rs
part_of = [Tasks](/docs/tasks/tasks.md)
status = done
created = 2026-11-07
updated = 2026-11-10
description = fig_core::lossless's module docs write the envelope schema as a two-space-indented paragraph, so rustdoc passes <type>, <extkind> and <text> to the browser as HTML tags: docs.rs shows the schema with its placeholders missing, and cargo doc warns three times
```

# The lossless envelope's placeholders read as HTML on docs.rs

**Status.** Done, in `docs(core): fence the lossless envelope schema in a code
block`. The two schema lines are a fenced `text` block, so rustdoc escapes
`<type>`, `<extkind>` and `<text>`, and `RUSTDOCFLAGS='-D warnings' cargo doc
-p fig-core --no-deps` passes. The docs.rs page shows them from the next
release.

**Where.** `rust/fig-core/src/lossless.rs`, lines 34–39 of the module doc.

The module doc shows the envelope a lossless value travels in:

```text
//! Envelope schema, as it appears once parsed back into an AST:
//!   { "$fig": { "t": "<type>" } }            // a null
//!   { "$fig": { "t": "<extkind>", "v": "<text>" } }   // an extended scalar
//! where `<type>` is `"null"` and `<extkind>` is the `ExtKind` field name
```

Two spaces of indent is not a code block in Markdown, so the two schema
lines are prose. In prose, `<type>`, `<extkind>` and `<text>` are HTML tags,
which rustdoc hands to the browser as they stand. A browser draws an unknown
tag as nothing, so the reader sees the schema with its placeholders gone:
`{ “$fig”: { “t”: “” } }`, with the quotes curled, all on the lines of one
paragraph. The sentence after it, which names `<type>` and `<extkind>` in
backticks, then explains placeholders the reader cannot see.

## Repro

On docs.rs, at <https://docs.rs/fig-core/7.0.0/fig_core/lossless/index.html>,
the page's HTML holds the raw tags:

```text
{ “$fig”: { “t”: “<type>” } }            // a null
{ “$fig”: { “t”: “<extkind>”, “v”: “<text>” } }   // an extended scalar
```

Locally, at d97b314f:

```console
$ RUSTDOCFLAGS='-D warnings' cargo doc -p fig-core --no-deps
error: unclosed HTML tag `type`
  --> rust/fig-core/src/lossless.rs:35:25
error: unclosed HTML tag `extkind`
  --> rust/fig-core/src/lossless.rs:36:25
error: unclosed HTML tag `text`
  --> rust/fig-core/src/lossless.rs:36:43
error: could not document `fig-core`
```

Without `-D warnings` these are the three warnings `cargo doc` prints for
fig-core. fig's CI runs no `cargo doc` job, so nothing has caught them.

## Done when

The schema lines are a fenced code block (or each placeholder is in
backticks), the docs.rs page of the next release shows `<type>`,
`<extkind>` and `<text>`, and `RUSTDOCFLAGS='-D warnings' cargo doc -p
fig-core --no-deps` passes.
