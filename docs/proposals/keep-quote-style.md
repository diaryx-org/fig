```fig
title = A replaced string keeps its quote style
part_of = [Proposals](/docs/proposals/proposals.md)
description = When set, replace or patch changes a string, write the new one in the old one's quote style where it can hold the value, rather than the printer's
author = adammharris
created = 2026-10-14
status = proposed
```

# A replaced string keeps its quote style

**Status.** Proposed, 2026-10-14. Split out of the `fix(patch)` commit that
made `fig patch` leave alone a value the target already holds in another
spelling. This is the other half: a value that *does* change.

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
| YAML | `\|`, `>` | has the line breaks the style needs; else the printer's |
| TOML | `'…'` | has no `'`, no line break and no control character |
| TOML | `"""…"""`, `'''…'''` | fits the multi-line rules |
| JSON5 | `'…'` | always: it escapes `'` |

Plain YAML is already what the printer chooses when it can, so it needs
nothing. Only a string replacing a string: a number replacing a string, or a
string replacing a mapping, has no old style to keep.

## Why not in the patch fix

It is not a patch question. The new value is rendered by the editor's value
replacement, which `set`, `replace`, `patch` and the language contract's
`replaceValue` share, so the change moves the bytes of every string
replacement in every format, and with them the fig-conformance edit suites'
recorded expectations (`replaceValue` alone is 16,748 cases). It wants its
own commit, its own `Behavioural-change:` trailer, and its own re-recording.

## Open questions

- **An explicit style wins.** `fig replace --raw a.yaml env.LOG "'info'"`
  asks for single quotes and must get them. Only a value fig renders itself
  takes the old style.
- **A patch's own style.** A YAML patch into a YAML target carries a style
  of its own. Everywhere else patch prefers the target's spelling, so the
  old style should win here too.
- **Where the style is read.** The AST holds a string's value, not its
  quoting. The old style is in the source bytes at the value's span, which
  the editor has when it splices, so this needs no AST change.
