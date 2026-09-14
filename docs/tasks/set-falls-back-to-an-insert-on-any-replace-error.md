```fig
title = `set` falls back to an insert on any replace error, so a refused replace becomes a duplicate entry
description = `Editor.set` catches every error from `replaceValAtPath` and inserts the key into the parent instead; when the replace was refused for cause — a section veto, a splice the reparse rolled back — the parent gains a second entry of that name
status = open
created = 2026-09-14
updated = 2026-09-14
part_of = [tasks](tasks.md)
```

# `set` falls back to an insert on any replace error, so a refused replace becomes a duplicate entry

**Repro.** With fig-quickjs's `pom.mjs` configured as `js-pom` (or any
runtime language whose value renderer can produce text the reparse
refuses in place):

```
$ printf '<p>\n  <a>1</a>\n  <b/>\n</p>\n' > t.xml
$ fig set t.xml b x
$ cat t.xml
<p>
  <a>1</a>
  <b/>
  <b>x</b>
</p>
```

`<b/>` reads as the empty string over the whole element; splicing `x`
over it makes mixed content, the reparse refuses it and rolls back —
correctly — and then `set` inserts `b` again as a new entry beside the
old one. The same happens for a compiled section format when the replace
is vetoed: INI's `set f.ini user 1` with `[user]` present is saved only
because INI's parser refuses a root key named like a section; a format
whose parser accepts the duplicate keeps it.

**Where.** `editor.zig`, `set`: `self.replaceValAtPath(path, value_text)
catch |replace_err| { … self.insertKey(parent, key, value_text) … }`. The
comment there says falling back on *any* replace error is what lets a
scalar-blocked path surface `NotAMapping`, which is fair for `NotFound`
and the null-promotion case; it is wrong for `CannotReplaceTable` /
`CannotReplaceSection` / `CannotReplaceContainer` and for a reparse
failure (`splice_rejected`), where the key exists and the replace was
refused for a reason the insert does not cure.

**Done when** `set` falls back to `insertKey` only when the replace
failed because the key was not there (`NotFound`, and `NotAMapping`
through an empty node), and surfaces every other replace error as it
is; a test that a vetoed section replace and a rolled-back splice each
leave the document with one entry of that name and an error.
