```fig
title = `indentAt` pads to the anchor token's column, which is wrong when the anchor is a value or a header key
description = the indentation a new line copies is the anchor line's leading whitespace padded with spaces out to the anchor's column; for an empty container after `key = ` and for a section key inside `[…]` that column is not a line indent, and the splice comes out as `\t\t\t       \tx = y;` or ` user = 1`
status = open
created = 2026-09-14
updated = 2026-09-14
part_of = [tasks](tasks.md)
```

# `indentAt` pads to the anchor token's column, which is wrong when the anchor is a value or a header key

**Repro.** Any format with `closed_containers` whose empty container sits
after its key on the key's line — fig-quickjs's `openstep.mjs`, or plist
with `<key>k</key><dict/>` on one line:

```
$ printf '{\n\ta = {};\n}\n' > t.pbxproj
$ fig set t.pbxproj a.x y && cat -A t.pbxproj
{$
^Ia = {$
^I    ^Ix = y;$
^I    };$
}$
```

`expandEmptyContainer` takes `base = indentAt(span.start)` where
`span.start` is the `{` four columns past the tab, so the base becomes
`\t` + four spaces and the child line `\t    \tx = y;`. The same padding
reaches `insertBlockKey` through `first_key`: INI's `[remote "origin"]`
has its key at column 1, and an entry anchored on it is written as
` user = 1`.

**Where.** `editor.zig`, `indentAt`: leading whitespace, then
`appendNTimes(' ', at - ws_end)`. The padding is there for the case its
comment names — a key inside a `- key: v` YAML item, whose siblings align
under the key, not under the `-` — and it is applied to every anchor,
including a value brace and a header's key, where the line's leading
whitespace alone is the answer.

**Done when** the padding is applied only where the anchor is a key on a
line that begins with a sequence marker (the YAML case, which
`node_marker_spans` identifies), and `expandEmptyContainer` and the
section-key anchor take the line's leading whitespace; a test that the
repro above writes `\t\tx = y;` under `\ta = {`, and that YAML's
`- key: v` sibling insert still aligns under `key`.
