```fig
title = JSON printer emits a non-string mapping key as invalid JSON
description = `fig get -o json` on a YAML document with a null, number, boolean or collection key writes the key bare (`null: "a"`, `[ ... ]: 23`) — output no JSON parser accepts — where the fig printer refuses with `NonStringKey`
status = done
created = 2026-09-04
updated = 2026-09-07
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# JSON printer emits a non-string mapping key as invalid JSON

**Status.** Done, in `fix(json): spell a scalar mapping key as a JSON string,
refuse a collection key` (2026-09-07). The second option was taken: a
non-string SCALAR key is spelled as the JSON string of its source text
(`"null"`, `"23"`, `"true"`, a datetime's timestamp), and a sequence or
mapping key — which has no faithful spelling — is refused with
`NonStringKey`, reported by the CLI as before; an alias key keeps
`UnresolvedAlias`, since like an alias value it means an unmaterialized YAML
AST rather than a key JSON has no room for. One `key` path serves JSON,
JSONC and JSON5, so all three moved together, and the YAML scoreboard now
ratchets a `json` count (materialize + print JSON + re-parse, 249 → 259 of
289) so this cannot regress. One accept document still prints JSON that does
not parse, for an unrelated reason: P76L's `!!int 1 - 3` becomes a number
node holding a non-numeric lexeme, because `materialize.applyScalarTag`
validates a `!!bool` payload but not an `!!int`/`!!float` one.

**Repro.** With `fig` at 0a0d646 (or any earlier build that survives the
YAML side):

```
fig get testdata/yaml/accept/2JQS.yaml -o json
{
  null: "a",
  null: "b"
}
fig get testdata/yaml/accept/4FJ6.yaml -o json
[
  {
    [
      "a",
      ...
    ]: 23
  }
]
```

A JSON object key is a string and nothing else. The YAML parser produces
null, number, boolean, alias, sequence and mapping keys (see
`yaml-printer-panics-on-accept-corpus.md` for the corpus files that carry
each), and the JSON printer writes whatever the key node is, so the output
is not JSON. The fig printer already answers this with
`error.NonStringKey`, which the CLI reports as "a non-string mapping key
has no representation in this output format" (`src/cli/diag_report.zig`).

**Done when** the JSON printer either refuses a non-string key with
`NonStringKey` like the fig printer, or spells a scalar key as its JSON
string (`"null"`, `"23"`, `"true"`) and refuses only a collection or alias
key — one of the two, decided and documented in the printer — and a test
covers each key kind. `-o json` over `testdata/yaml/accept/` should then
never produce bytes that a JSON parser rejects. Found while closing the
YAML printer task; it is the same class of bug one printer over.
