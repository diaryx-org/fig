# The kit's shared fixtures

Data only. Every binding's kit reads these files and must give the
answers they hold, so the files are the specification of a layer and not
a recording of any one implementation. A change to an answer here is a
change to the kit in every binding at once. See
`docs/proposals/kit-parity.md` in the fig repository.

## `rows/`: the row writer

Each file is one JSON object:

- `about`: what the case holds.
- `source`: the document the spans point into.
- `ops`: the writes, in order. Each `add` makes the next row id, from 0.
- `puts`: what each `put` answered, in order, when there are any.
- `table`: the table `finish` gives, as the helper wire spells one
  (`docs/helper-protocol.md`): a row's side facts sit on the row, and a
  list may be absent when it is empty.

The ops:

| Op | Meaning |
|---|---|
| `["add", kind, parent, span, extra?]` | A row of `kind` (`null`, `bool`, `int`, `float`, `string`, `sequence`, `mapping`, `keyvalue`, `alias`) over `span`. With a `parent` it is appended to that row as it is written. `extra` holds the row's `text`, `ext_kind` and side facts by their wire names: `marker`, `sep`, `anchor`, `anchor_span`, `tag`, `tag_span`, `item_sep`, `open`. |
| `["append", parent, child]` | Link `child` as `parent`'s last child. A keyvalue's first child is its key, its second its value. |
| `["first", id, child]` | Make `child` container `id`'s first child, or empty it for `null`. |
| `["next", id, next]` | Link `next` after `id` among its siblings, or end the chain for `null`. |
| `["put", map, key, value, policy]` | Put the entry in `map` by its duplicate policy (`last_value`, `first_value`, `keep`, `error`). Answers `[entry, repeat]`, the keyvalue row that holds the value and whether the key was already there, or `"error"` when the policy refuses it. A new entry is a new row whose span runs from the key's start to the value's end. |
| `["child_by_key", map, key, want]` | `map`'s entry keyed `key` is row `want`, or there is none for `null`. |
| `["comment", id, slot, style, text]` | A comment on `id`: `leading`, `trailing` or `dangling`; `line` or `block`. |
| `["region", id, start, end]` | A header line of section `id`. |
| `["mention", id, span, kind]` | A place `id`'s name is written: `header`, `entry`, `index` or `append`. |
| `["finish", root]` | Put the rows in pre-order from `root`. |
