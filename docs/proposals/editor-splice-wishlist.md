```fig
title = Editor splice: findings and wishlist
description = Downstream findings against fig's editor splice path, and what shipped in response
created = 2026-07-25
updated = 2026-07-25
part_of = [proposals](proposals.md)
```

# Editor splice: findings and wishlist

> **Status: §1–§4 fixed in fig 2.5.3. §5 was my error — `width` works; see
> below.** Re-verified 2026-07-25 against `rust/v2.5.3` and, side by side,
> against fig-sys 2.5.2, where every original symptom still reproduces exactly.
> prov has since deleted the workaround this document was written to justify
> (`set_meta_in_text` is now a single splice), so nothing downstream depends on
> the old behaviour any more.
>
> **Pin the floor at fig-sys, not fig.** The fix is Zig-side. `fig` 2.5.2 built
> against `fig-sys` 2.5.3 behaves correctly, and `fig` 2.5.3 requires
> `fig-sys ^2.5.3`, so depending on fig 2.5.3 is sufficient — but a lock pinned
> to an older `fig-sys` silently restores the corruption.

Written 2026-07-25 from downstream use in [prov] / diaryx, which drives
`fig::Editor` through `prov::edit::set_meta_in_text` to make format-preserving
edits to YAML config documents. Everything below was reproduced against
**fig 2.5.0** (`bindings/rust`, `default-features = false`, features
`["serde", "yaml", "json", "derive", "indexmap"]`), Format::Yaml throughout.

Two of these are correctness bugs that lose data silently. The rest is a
capability gap that forced an awkward workaround downstream.

## What 2.5.3 does now

| § | was | now |
|---|---|---|
| 1 | `set(a.b, [x])` → `a: {b: - x}`, reports `Ok` | `a:\n  b:\n  - x` — ancestors seeded block |
| 1 | into an *existing* flow container: corrupts, reports `Ok` | `Err(InvalidArgument)` |
| 2 | one-entry map at a fresh path → `Err` | `Ok`, block |
| 3 | failed set leaves an orphan `a: {}` | no partial state |
| 4 | `set_value_with` cannot create | upserts |

The remaining `Err(InvalidArgument)` — a structured value into an existing
*flow* container — is the honest answer rather than a bug: it is reported, not
papered over, which is what §1's wish asked for. Worth knowing that the
leaf-by-leaf path cannot serve that case either, so there is no fallback a
caller could write.

## Reproduction harness

```rust
use fig::{Format, Segment, Value};
fn map(p: &[(&str, Value)]) -> Value {
    Value::Map(p.iter().map(|(k, v)| (Value::Str((*k).into()), v.clone())).collect())
}
fn s(v: &str) -> Value { Value::Str(v.into()) }
fn ed(t: &str) -> fig::Editor { fig::Editor::open(t.as_bytes(), Format::Yaml).unwrap() }
```

Base document is `"title: t\n"` unless stated.

---

## 1. A block value spliced into an inline container corrupts, and reports `Ok`

**Severity: highest.** No error is returned; the document silently stops meaning
what was written.

```rust
// A fresh nested path. fig creates the `a` ancestor inline, then splices a
// block-rendered sequence into it.
let mut e = ed("title: t\n");
e.set_value(&[Segment::Key("a"), Segment::Key("b")], Value::Seq(vec![s("a")])).unwrap(); // Ok!
// source: "title: t\na: {b: - a}\n"
```

`a: {b: - a}` is not the sequence that was written. Re-parsing yields a string,
or fails, depending on the surrounding text. The same happens against an
already-inline container:

```rust
let mut e = ed("title: t\na: {b: 1}\n");
e.set_value(&[Segment::Key("a"), Segment::Key("c")], Value::Seq(vec![s("q")])).unwrap(); // Ok!
// source: "title: t\na: {b: 1, c: - q}\n"
```

**Wish:** either render the value in flow when the target container is flow, or
promote the container to block, or return an error. Any of the three beats
returning `Ok` on a document that no longer round-trips.

Downstream this had to be worked around by writing every value, then re-parsing
the whole document and comparing the value's *kind* at the path to what was
written — an expensive check that exists purely because the return value cannot
be trusted.

## 2. A single-entry map cannot be set at a path that does not exist

**Severity: high.** Same call succeeds with two entries and fails with one.

```rust
// Fresh top-level key
ed("title: t\n").set_value(&[Segment::Key("fresh")], map(&[("k", s("v"))]));
// => Err(NotFound)

ed("title: t\n").set_value(&[Segment::Key("fresh")], map(&[("k", s("v")), ("j", s("w"))]));
// => Ok, "title: t\nfresh:\n  k: v\n  j: w\n"
```

Also fails nested, with a different error:

```rust
ed("title: t\n").set_value(&[Segment::Key("a"), Segment::Key("b")], map(&[("k", s("v"))]));
// => Err(Parse("failed to parse input"))
```

And into an existing inline container:

```rust
ed("title: t\na: {b: 1}\n").set_value(&[Segment::Key("a"), Segment::Key("c")], map(&[("k", s("v"))]));
// => Err(NotFound)
```

A one-entry map renders as `k: v`, which is presumably being re-read as a scalar
rather than as a block mapping. Sequences and multi-entry maps are unaffected.

**Wish:** a one-entry map behaves like a two-entry map.

## 3. A failed `set_value` leaves partial state behind

```rust
let mut e = ed("title: t\n");
let r = e.set_value(&[Segment::Key("a"), Segment::Key("b")], map(&[("k", s("v"))]));
assert!(r.is_err());
e.source().unwrap(); // "title: t\na: {}\n"  <- the ancestor was created anyway
```

The ancestor is created before the value splice fails, and is not rolled back. A
caller that retries, or that treats `Err` as "nothing happened", is now editing a
document it did not expect.

**Wish:** a failed edit is a no-op, or the partial state is documented.

## 4. No way to create a key *and* control its layout

`set_value` creates missing ancestors but takes no `SerializeOptions`.
`set_value_with` takes options but cannot create:

```rust
let opts = SerializeOptions { width: 0, ..Default::default() };
ed("title: t\n").set_value_with(&[Segment::Key("fresh")], map(&[("k", s("v"))]), opts);
// => Err(NotFound)
```

So there is no call that says "create this key, and render its value as a block".
Combined with (1), a caller who needs a block container at a fresh path has no
way to ask for one.

**Wish:** `set_value_with` upserts, matching `set_value`.

## 5. ~~`SerializeOptions::width` appears to be a no-op for YAML~~ — WRONG, retracted

**This finding was my mistake. `width` works for YAML. No action needed.**

The original claim rested on this:

```rust
map(&[("k", s("v"))]).serialize_with(Format::Yaml, /* width: 80 */);  // => Ok("k: v\n")
map(&[("k", s("v"))]).serialize_with(Format::Yaml, /* width: 0  */);  // => Ok("k: v\n")
```

Identical — but only because a **top-level** mapping has nothing to be inlined
*into*. YAML's flow-vs-block choice applies to a container nested inside
another, and there `width` governs it exactly as documented:

```rust
let v = Value::Map(vec![(s("contents"), Value::Seq(vec![s("a"), s("b")]))]);
v.serialize_with(Format::Yaml, SerializeOptions::default().width(80)); // "contents: [a, b]\n"
v.serialize_with(Format::Yaml, SerializeOptions::default().width(1));  // "contents:\n- a\n- b\n"
```

diaryx has relied on this the whole time — `diaryx_core::yaml::serialize_mapping`
passes `width(1)` to keep note frontmatter one-item-per-line — so the claim was
contradicted by working code in the same tree I wrote it from. Generalizing from
the one shape that cannot show the effect is the same error that produced the
retracted "fig cannot serialize a bare sequence" claim noted below; both came
from probing a single case and reporting it as a general rule.

The doc comment on `width` is accurate. What it does *not* say, and could, is
that a top-level container has no flow form, so width has no effect there.

## What "good" looks like downstream

The whole class of problem would disappear if `set_value` on a nested path
created its ancestors as **block** containers for YAML. Every failure above is
some consequence of the ancestors being created inline:

- a block value cannot be spliced into them (1),
- a one-entry map trips the same path (2),
- and there is no way to ask for anything different (4, 5).

If inline ancestors are deliberate — for compactness — then the minimum viable
fix is (1) returning an error instead of `Ok`, so callers can at least detect it.

**This is what 2.5.3 did**, and it resolved §1–§4 together exactly as predicted:
block ancestors, plus an `Err` for the one case that remains unsatisfiable.

## Notes for whoever picks this up

- All of these were in the **editor splice** path, not the parser or serializer:
  `Value::serialize` round-trips every one of these values correctly on its own.
- `bindings/rust/fig/src/editor.rs` `set_value` → `value_text` →
  `ffi::fig_editor_set`; `value_text` forces flow only for `Format::Fig` and
  leaves YAML at its default, so the interesting logic is Zig-side in
  `fig_editor_set`. The 2.5.3 fix landed there, which is why the version floor
  has to be read as a `fig-sys` floor.
- **Two claims in this document were wrong, both from the same habit.** An
  earlier version said fig cannot serialize a bare sequence to YAML (actually a
  missing `yaml` feature flag in my probe crate), and §5 said `width` is a no-op
  for YAML (actually true only of top-level containers). Each came from probing
  one shape and writing up the result as a general rule. Anyone extending this
  document: vary the shape before generalizing, and check the claim against
  code that already works.

[prov]: https://github.com/diaryx-org/prov
