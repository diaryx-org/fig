```fig
title = A YAML alias cannot be read into a value by the Rust or TypeScript binding
description = `toValue`/`to_value`, `parse()` and serde stop at an alias node with an internal error, where `convert` resolves the same alias; a document with `&a`/`*a` converts but cannot be read
status = done
created = 2026-09-27
updated = 2026-10-11
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# A YAML alias cannot be read into a value by the Rust or TypeScript binding

**Status.** Done, in `fix: the bindings read values through a YAML alias
and a merge key` (2026-10-11). The core gained
`fig_document_resolve_aliases`, which makes a copy of a document with each
alias a copy of its anchored node and each `<<` flattened, by the same
`materialize` pass `convert` runs (in a `keep` tag mode, so tags stay as
the readers have always seen them). `to_value`, serde, `toValue`, `toJS`,
`get` and `has` read that copy; the node API and `nodeAt` still walk the
document as written. A cyclic alias is `PARSE_ERROR` at the alias; an
undefined one was already refused by both YAML parsers.

**Repro.** TypeScript, compiled YAML:

```ts
import { parse, convert, Format } from "@diaryx/fig";

convert("a: &x 1\nb: *x\n", Format.Yaml, Format.Json); // '{\n  "a": 1,\n  "b": 1\n}\n'
parse("a: &x 1\nb: *x\n", Format.Yaml);                // FigError: fig_node_kind: node kind 8 is not a value
```

The Rust binding does the same thing. `Document::to_value` answers
`Error::Internal` for a `FigNodeKind::Alias`, and the serde deserializer
answers "malformed document". A YAML language registered at runtime is no
different, because its table carries the same alias rows.

**Cause.** Both readers walk the parsed document node by node through the C
ABI. The document keeps YAML's reference layer: an alias is a node of its
own, whose text is the anchor's name. The serializer collapses that layer
when a document leaves YAML (`materialize`), which is why `convert`
resolves the alias. The readers have no such step, and no ABI call
resolves an alias to the node it names, so they refuse.

**Done when** `parse`, `toValue`, `to_value` and a serde read give the
anchored node's value wherever an alias stands, merge keys included, and
refuse an undefined or cyclic alias as a parse-level error rather than an
internal one. The likeliest shape is a call that resolves an alias node to
its target (`AST.resolveDeep` already does this in the core), which both
readers follow.
