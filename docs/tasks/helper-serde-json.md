```fig
title = The Rust helper reads JSON with fig's json format
description = `fig::helper` parses and writes JSON-RPC through the `json` language, so a helper built without that feature cannot read a request; it should use serde_json behind a `helper` feature
status = open
created = 2026-10-12
updated = 2026-10-12
part_of = [Tasks](/docs/tasks/tasks.md)
```

# The Rust helper reads JSON with fig's json format

`fig::helper` decodes each request with `Document::parse(line, json)` and
encodes each response with `Value::serialize_with(json, …)`. In 6.0 `json`
is a language like any other, and a crate built with
`default-features = false` and no `json` cannot read a single request: the
helper answers every line with `-32700`, "this build cannot read JSON".
The [JSON-RPC wire proposal](/docs/proposals/json-rpc-wire.md) §4 moves
the module to `serde_json`.

It was left out of the change that moved the wire to JSON-RPC because the
module's public functions trade `fig::Value` — `handle`, `encode`,
`table_to_value`, `table_from_value`, `description_to_value`,
`description_from_value` — and fig-lua and fig-quickjs build on them, so
the move changes their API too.

## Done when

`fig::helper` reads and writes the wire with `serde_json` behind a
`helper` feature that the crate's default set does not need, its public
functions trade `serde_json::Value`, `tinykv_helper` builds with
`--no-default-features --features helper`, and fig-lua and fig-quickjs
build against it.
