```fig
title = Compact objects
description = A fig-inline object is whitespace-separated and its values are one token each, so `{a.b=1 a.c=2 a.d="Hello world"}` parses — dotted keys nest, commas become optional, and twig's attribute block reads the same bytes the same way
author = adammharris
created = 2026-10-09
status = implemented
updated = 2026-10-09
part_of = [proposals](proposals.md)
```

# Compact objects

> **Status: IMPLEMENTED** on `main`, unreleased, written against 6f71004
> (5.2.0): the core parser and printer, the TypeScript twin, spec 2.0 § 6.3.
> A language change: every input whose meaning moves either already warned
> or now fails loudly, except `{a.b = 1}`, whose old reading (a key with a
> dot in it) was a bug against § 5.1. It is a major; the C ABI does not
> move. Still to follow: the editor's separators (§ 6, a task), the
> tree-sitter grammar (a task), and the fig twins in fig-lua and
> fig-quickjs.

## 1. The claim

A fig-inline object (`=` pairs) should read like an attribute list, not like a
JSON object with different punctuation:

```
p = {a.b=1 a.c=2 a.d="Hello world"}
```

Three rules make that parse, and each one also fixes a defect in the current
flow layer:

1. **Pairs are separated by whitespace.** A `,` is still accepted between pairs
   but is never written by `fig fmt`.
2. **A value is one token.** An unquoted value ends at whitespace, `,`, `]`, or
   `}`. A value with spaces is quoted.
3. **A key is a key path.** `a.b` steps into a sub-object, exactly as it does in
   block position; `"a.b"` is the literal key.

JSON-spelled objects (`:` pairs, quoted keys) and flow arrays do not change.

## 2. Why

**Twig.** Twig's attribute block is the djot brace block with fig inside it
(twig `docs/proposals/twig-native-language.md` Part 4):

```
{#intro .lead .wide  lang = en  data.count = 3  disabled}
```

Under today's rule, `lang = en  data.count = 3  disabled` is one pair whose
value is the string `en  data.count = 3  disabled`, with a `flow_missing_comma`
warning. The block only means what it says if a value stops at whitespace. Twig
does not depend on fig and owns its own grammar — `#id`, `.class`, and the bare
`disabled` are twig's sugar — but the `key = value` part of it should mean the
same bytes in both, and that is only possible if fig's objects are
whitespace-separated.

**The bugs the current flow layer has.** A review of § 6.3 on 2026-10-09 found:

| Input | Today | Under this proposal |
|---|---|---|
| `{ a.b = 1 }` | key `"a.b"`, silently | `{a: {b: 1}}` |
| `{ @x = 1, -x = 2, [q = 3 }` | keys `"@x"`, `"-x"`, `"[q"` | `FigBadKey` (§ 5.1 applies) |
| `{ x = 1 y = 2 }` | `x = "1 y = 2"`, warned | two pairs |
| `{ x }` | `FigUnclosedFlow` | `FigFlowMissingEquals` |
| `[a, 'b c' d]` | `FigUnclosedFlow` | `FigQuotedTrailingContent` |
| `[a,,b]`, `[,]` | "write one after `=`" | `FigFlowEmptyElement` |
| `{ x = """` ⏎ … | `FigUnclosedFlow` | `FigFlowMultilineString` |

`FigUnclosedFlow` had become the code for any unexpected byte inside a flow
value, and every message it gave named a fix that did not apply.

## 3. Why not an opt-in compact mode

A second, opt-in grammar has to be selected somehow. Selecting it by sniffing
commas is ambiguous — `{a = Hello world}` has none, so is it a one-pair object
holding `"Hello world"` or a compact-mode error? — and selecting it by a marker
puts noise on every object for the sake of the other mode.

The split that does work is the one fig already has. An object picks its
spelling by its first pair separator: `=` is fig, `:` is JSON. JSON spelling
keeps commas and multi-word bare values; fig spelling becomes compact. An
author who wants unquoted prose in an object writes it JSON-style, and pasted
JSON keeps parsing.

**Arrays stay comma-separated with multi-word values.** Splitting
`[Adam Harris, Makena Harris]` on whitespace would silently turn one name into
two, and lists of names are the diary case fig was built for.

## 4. The grammar

```
FIGOBJ   ::= '{' WS* ( PAIR ( SEP PAIR )* SEP? )? WS* '}'
SEP      ::= WS+ | WS* ',' WS*
PAIR     ::= KEYPATH WS* '=' WS* TOKEN
KEYPATH  ::= KEYSEG ( '.' KEYSEG )*            # § 5.1 segments; no [i] / []
TOKEN    ::= QUOTED | FLOW | BRACKETBARE | BARE | DATE ' ' TIME
```

- **Separators.** Whitespace (newlines included) or a comma, or both. The
  separator is mandatory: a value glued to the next key is an error
  (`{a="x"b=2}` is `FigQuotedTrailingContent`). Spaces around `=` are
  optional — `{a=1}` and `{ a = 1 }` are the same object.
- **Bare tokens** end at whitespace, `,`, `]`, `}`. The literal-else-string
  sniff applies to the token as before. A `#` glued to the token is content
  (`{tag=a#b}`); a whitespace-preceded `#` opens a comment, as everywhere.
- **Flow values** (`[…]`, `{…}`) are tokens: `{a=[1, 2] b=3}` is an array and
  a second pair. Inside the array, array rules apply.
- **Bracket-led bare strings** survive when the trailing content is glued:
  `{href=[Blog](/x.md)}` is the string `[Blog](/x.md)`. With whitespace before
  the trailing content, the bracket group is a flow value and the rest is the
  next pair.
- **Datetimes with a space.** `2026-07-01 09:00` is one token: a local date
  followed by one space and a time is joined. This is unambiguous, because a
  time can never be a key (`:` is not a bare-key character).
- **Keys** follow § 5.1: bare segments of `[A-Za-z0-9_-]` not starting with `-`,
  or quoted segments, joined by `.`. Index steps are not allowed.
- **Dotted keys nest, TOML's rules.** Dotted keys in one object build and share
  implicit sub-objects; an explicitly written `{…}` value stays closed:

  ```
  {a.b=1 a.c=2}        # {a: {b: 1, c: 2}}
  {a={b=1} a.c=2}      # FigClosedFlowValue
  {a.b=1 a=2}          # FigDuplicateKey
  {a=1 a.b=2}          # FigKeyNotContainer
  ```

  The whole object, implicit sub-objects included, is closed to the outside
  (§ 6.4).
- **A word with no `=`** is `FigFlowMissingEquals`, whose message names both
  fixes: add `=value`, or quote a multi-word value.

The JSON spelling is unchanged: commas between members, a single quoted key,
values that run to `,`/`}`. Mixing `=` and `:` in one object is still
`FigMixedFlowSeparators`.

## 5. Canonical form

`fig fmt` writes `{x=1 y=2}`: no padding inside the braces, no spaces around
`=`, one space between pairs. A string value that is not one token is quoted.
JSON-spelled objects are still normalized to fig spelling. Arrays print as
before (`[a, b]`).

The fits-or-breaks rule is unchanged, including "no object directly contains
another object". A dotted key in source (`{a.b=1 a.c=2}`) builds a nested
object, so `fmt` breaks it to block form like any other object-in-object.

## 6. Editing

The splice engine inserts into a flow object using `Syntax.flow_entry_sep`
(`,`) and, for fig, the key/value separator copied from the first sibling
(`flow_kv_sep_from_siblings`). After this change an insert into `{a=1}` writes
`{a=1, b=2}` — valid, since commas are still accepted, but not canonical.
Making the engine write `{a=1 b=2}` needs a per-object entry separator, which
`Syntax` cannot say today; that is a follow-up, filed as [a task](/docs/tasks/compact-object-editor-separator.md).

## 7. Compatibility

- `{ x = 1, y = 2 }` still parses to the same value. Every such object in the
  org's repositories does (46 as of 2026-10-09); `fig fmt` rewrites them to
  `{x=1 y=2}`.
- `{ name = Adam Harris }` is now `FigFlowMissingEquals`. No document in the
  org's repositories has one, but `fig fmt` has been writing them, so a journal
  might. The error is loud and its message says to quote the value.
- `{ a.b = 1 }` changes meaning silently, from the key `"a.b"` to a nested
  object. That was a defect against § 5.1, and a TOML-literate reader expected
  the new reading.
- The new error codes are additive.
- The TypeScript twin (`bindings/typescript/languages/fig.js`), the tree-sitter
  grammar, and the fig twins in fig-lua and fig-quickjs read flow objects and
  move with the core.

A breaking change to the language is a major for every artifact
([VERSIONING](/docs/VERSIONING.md)).
