```fig
title = A runtime printer is not told when it prints splice text, so it prints a whole document
description = the splice text the bindings and `fig patch` hand the editor comes from a runtime format's `print`, whose `PrintOptions` carry no `splice` bit — a runtime plist or NestedText twin spells a scalar as its wrapped or `>`-blocked document, which the compiled formats stopped doing when they declared `printSplice`
status = open
created = 2026-09-22
updated = 2026-09-22
part_of = [tasks](tasks.md)
```

# A runtime printer is not told when it prints splice text, so it prints a whole document

**Where.** The bindings' editor paths and `Patch.render` serialize a value
with the `splice` option — the text the editor takes. For a compiled
format, `AST.serializeFragmentWith` then uses a printer's `printSplice`
where it declares one: plist's drops the XML declaration, DOCTYPE and
`<plist>` wrapper, and NestedText's writes a scalar as its plain text. For
a runtime format it is `Runtime.printNodeWith`, which calls the vtable's
`print` with `PrintOptions` that carry no `splice` bit, so the language
cannot tell splice text from a document and prints the document.

**Effect.** A runtime twin of plist or NestedText, held to the compiled
format's printer, prints a scalar as a whole document, and an edit through
a binding or `fig patch` splices that document — the doubled `>` block and
the refused plist value the compiled formats had before 2026-09-22. The
twins hold their edits to the compiled format through the CLI for now,
which hands the editor bare text and never meets this.

**Done when** a runtime format is told — a `splice` field in
`PrintOptions`, appended so an older language reads it as zero — and `fig-quickjs`'s plist and NestedText twins, given it, splice a
scalar through a binding as the compiled formats do.
