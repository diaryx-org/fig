```fig
title = Proposals
description = Design proposals, findings, and wishlists for `fig` — from inside and outside the project
author = adammharris
created = 2026-07-25
updated = 2026-09-07
part_of = [docs](/docs/docs.md)
contents
> * [Editor splice: findings and wishlist](editor-splice-wishlist.md)
> * [A declared Language interface](language-interface.md)
> * [Derived regions](derived-regions.md)
> * [Pluggable formats](pluggable-formats.md)
> * [Runtime languages](runtime-languages.md)
```

# Proposals

Design proposals and findings for `fig`, including reports from downstream
consumers. A document lands here when it argues for a change rather than
describing what `fig` already does — so this folder is where an idea is discussed
*before* it is settled, and the record of the argument *after*.

It is deliberately not a to-do list, and not a source of truth:

- Behavior that shipped is documented in the format guides ([Rust](/docs/rust.md),
  [TypeScript](/docs/typescript.md), [Zig](/docs/zig.md)) and the
  [spec](/docs/spec.md) — never here.
- A breaking change that is *coming* is a `Behavioural-change:` trailer on the
  commit that lands it, gathered into the [CHANGELOG](/docs/CHANGELOG.md)'s
  unreleased section — a commitment to consumers; a proposal here is not.
  (Until core 3.0 there was a separate `BREAKING-CHANGES.md` for changes that
  were planned but not yet made; everything it listed shipped in 3.0, and
  nothing is queued that way now.)
- Anything written from outside the project is kept as its author wrote it, with
  corrections and outcomes added in a **Status** section at the top rather than
  edited into the body. A proposal whose findings turned out to be wrong is still
  worth keeping — the reasoning is the value, and the correction is part of it.

Each document says which release, if any, resolved it.
