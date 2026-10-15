```fig
title = A replaced string keeps its quote style
part_of = [Proposals](/docs/proposals/proposals.md)
description = When set, replace or patch changes a string, write the new one in the old one's quote style where it can hold the value, rather than the printer's
author = adammharris
created = 2026-10-14
status = implemented
updated = 2026-10-15
```

# A replaced string keeps its quote style

**Status.** Implemented, 2026-10-15: the engine, the C ABI, and the CLI's
`replace`, `set` and `patch`, then the TypeScript and Rust bindings, the
JavaScript twins and the fig-conformance re-recording
([the task](/docs/tasks/bindings-keep-quote-style.md)). Proposed 2026-10-14, split
out of the `fix(patch)` commit that made `fig patch` leave alone a value the
target already holds in another spelling. This is the other half: a value
that *does* change.

## The problem

When `fig replace`, `fig set` or `fig patch` rewrites a string, the new
value is spelled by the target format's printer, and the old value's quote
style is lost:

```
$ cat a.yaml
env:
  LOG: "debug"
$ fig replace a.yaml env.LOG info
$ cat a.yaml
env:
  LOG: info
```

The file's author chose the double quotes; fig's printer did not. Every
other byte of the line is still the author's, and this one should be too
where it can be. The same holds for a TOML literal string (`'…'`) that comes
back as a basic one (`"…"`), and a JSON5 single-quoted string that comes back
double-quoted.

## The proposal

When an edit replaces a string with a string, write the new one in the old
one's style if that style can hold the new value, and in the printer's
spelling if it cannot:

| Format | Old style | Kept when the new value… |
| --- | --- | --- |
| YAML | `"…"` | always: it escapes anything |
| YAML | `'…'` | has no line break (a `'` is doubled) |
| YAML | `\|`, `>` | the printer's, which writes `\|` for a string with line breaks |
| TOML | `'…'` | has no `'`, no line break and no control character |
| TOML | `"""…"""`, `'''…'''` | fits the multi-line rules |
| JSON5 | `'…'` | always: it escapes `'` |

Plain YAML is already what the printer chooses when it can, so it needs
nothing. Only a string replacing a string: a number replacing a string, or a
string replacing a mapping, has no old style to keep.

## Why not in the patch fix

It is not a patch question. Every string replacement in every format moves,
and with them the fig-conformance edit suites' recorded expectations
(`replaceValue` alone is 16,748 cases). It wants its own commit, its own
`Behavioural-change:` trailer, and its own re-recording.

## How it landed

The proposal first said the editor renders the new value. It does not: the
editor takes text already spelled, and four callers render it (the CLI's
`value_arg.render`, `patch.zig`, and the TypeScript and Rust bindings'
`valueText`). By the time a `"info"` reaches the editor it cannot tell
fig's spelling from a `--raw` one, so the editor cannot restyle every string
it is handed.

So the caller says when the text is fig's. `Editor.replaceStringAtPath` and
`Editor.setString` take the rendered text and the string it spells; where
the value replaced is a string, the format's new `renderString` renderer
(the seventh, after `renderKeyName`) is told the string and the old one's
spelling, `old_value`, and writes the string in that style or nothing, and
the editor splices `rendered` when it writes nothing. YAML, TOML and JSON
declare one. The C ABI has `fig_editor_replace_string` and
`fig_editor_set_string`, the vtable `render_string` after `split_stream`,
and the helper wire `which: "string"` with `old_value`. The CLI turns a
string `replace` or `set` into these; `patch` does for a string it writes.
`--raw` and the `*Raw` binding methods go to the plain ops, and keep what
they were given.

A style the renderer judges too risky is the printer's rather than a
refusal: TOML's `'''` refuses a closing `'` as well as `'''`, and YAML's
`'…'` any control character, as the printer would.

## Open questions

- **An explicit style wins.** `fig replace --raw a.yaml env.LOG "'info'"`
  asks for single quotes and must get them. Only a value fig renders itself
  takes the old style.
- **A patch's own style.** A YAML patch into a YAML target carries a style
  of its own. Everywhere else patch prefers the target's spelling, so the
  old style should win here too.
- **Where the style is read.** The AST holds a string's value, not its
  quoting. The old style is in the source bytes at the value's span, which
  the editor has when it splices, so this needs no AST change. It is read
  there, and told to the renderer as `old_value`.
