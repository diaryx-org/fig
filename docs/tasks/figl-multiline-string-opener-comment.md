```fig
title = A comment after a figl multi-line string's opening quotes is invisible to the editor
part_of = [Tasks](/docs/tasks/tasks.md)
description = figl reads a comment after an opening `'''` or `"""`, but the editor looks for a value's trailing comment only after the closing quotes, so get misses it and set writes a second comment beside it
status = open
created = 2026-10-13
```

# A comment after a figl multi-line string's opening quotes is invisible to the editor

**Repro.** Found while fixing the YAML block-scalar comment (e6ca1eb):

```sh
printf "x = \x27\x27\x27 # op\ntext\n\x27\x27\x27\n" > s.figl
fig comment --get s.figl x              # nothing: the editor never sees `op`
fig comment --inline s.figl x note      # adds `# note` after the close
```

The parser accepts the comment after the opener, and nothing reads it
back through the editor; a set leaves the document with two comments on
one value, of which a reprint keeps whichever the printer knows.

**Cause.** `trailingCommentWindow` (`src/editor.zig`) looks after a value's
end, which for a multi-line string is its closing quotes. Where else a
language allows a value's comment is not something `Syntax` says: YAML's
block scalar takes it on the header line only because its span ends in a
newline (e6ca1eb), which a figl string's does not.

**Done when** it is decided where a figl multi-line string's comment
lives (the opener, the closer, or both, and which one `comment --inline`
writes), the spec says so, the parser and editor agree on it, and a test
in `languages/figl/tests/editor.zig` gets, sets and deletes it.
