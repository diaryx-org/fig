```fig
title = An entry appended after `export KEY=value` is indented to the key's column
description = `fig set f.env NEW v` on a file whose last entry has an `export ` prefix writes `       NEW=v`, padded to the column the key starts at, because the entry's span starts at the key and the editor pads a new sibling to the previous one's column
status = done
created = 2026-09-10
updated = 2026-09-22
part_of = [Closed tasks](/docs/tasks/closed/closed.md)
```

# An entry appended after `export KEY=value` is indented to the key's column

**Status.** Done, by neither fix below: `fix(editor): a new line pads to
its anchor's column only past a sequence item marker` (2026-09-22) made
the editor's indent rule pad only past an item marker, which is the
"distinguish a marker prefix from a word" branch, reached from
[`indentAt`'s own task](/docs/tasks/closed/indent-at-pads-to-the-anchor-column.md) for every
format at once. The dotenv parser is unchanged — no span moved, so neither
the twins nor a span reader has anything to follow — and a test in
`editor.zig` holds the repro to `NEW=v` at column 0.

**Repro.**

```bash
$ printf 'export D=p\n' > v.env
$ fig set v.env NEW v
$ cat v.env
export D=p
       NEW=v
```

The seven spaces are the column of `D`. Every other edit on an `export`
entry is right — `set` on it keeps the prefix, `delete` takes the whole
line, a leading comment lands above it — and an entry appended after a
plain `KEY=value` lands at column 0. Found while holding `fig-lua`'s
`lua-dotenv` to the compiled format: the twin reproduces it exactly,
since it produces the same table, which is the point of the twin and the
reason this is the compiled format's bug and not the carrier's.

**Why.** The keyvalue node's span starts at the key (`flat_map.putEntry`
takes `spans[key_id].start`), so `export ` is prefix bytes before the
entry on its line, and the editor's indent rule for a format without
`structural_indent` — prefix bytes, whitespace padded to the column — pads
a new sibling to where the previous entry's span began. That rule is right
for a prefix that is an item marker (`- key: v`); `export ` is not one.

**Fix, and the choice in it.** Either dotenv's parser records the entry's
span from `export` (the entry line IS `export KEY=value`; the key node's
span stays at the key), which moves the keyvalue span for every `export`
entry and is a `Behavioural-change:` for anyone reading spans, and
`fig-lua`'s `languages/dotenv.lua` follows in the same change — or the
editor learns to distinguish a marker prefix from a word, which touches
every format. The first is the smaller and truer change.

**Done when** the repro writes `NEW=v` at column 0, `fig lang check
lua-dotenv --against dotenv` still passes on an `export` file, and the
trailer records the span that moved.
