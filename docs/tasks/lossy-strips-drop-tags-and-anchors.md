```fig
title = The lossy strips and the `$fig` envelope rebuild the AST without its tags, anchors and directives
description = `Lossless.lossyStrip`, `Lossless.encode`/`decode` and `FlatStrip.lossyStrip` copy nodes and comments into a fresh AST and leave `node_tags`, `node_anchors` and `tag_directives` behind, so a runtime printer reached through the CLI's lossy path never sees a tag it recorded
status = open
created = 2026-09-14
updated = 2026-09-14
part_of = [tasks](tasks.md)
```

# The lossy strips and the `$fig` envelope rebuild the AST without its tags, anchors and directives

**Repro.** A runtime language that records a tag and declares `lossless`
(so the CLI's `printRuntime` runs the null strip) — fig-quickjs's
`openstep.mjs` did, tagging a braceless `.strings` root `!strings`:

```
$ printf '"a" = "b";\n' > t.strings
$ fig lang table t.strings -i js-openstep | head -c 80
{"rows":[{"kind":"mapping","parent":null,"span":[0,11],"tag":"!strings"}, …
$ fig fmt t.strings -i js-openstep && cat t.strings
{
	"a" = "b";
}
```

The parse table carries the tag; the table the printer received did not,
and the printer wrote braces. `documentToTable` copies tags faithfully
(`appendRows`, `ast.tagOf`); what lost them is the strip in between. The
module now declares no `lossless`, which routes around it, and that is a
workaround, not an answer: YAML's own printer would lose `!!str` and
`%TAG` through the same path if it were a runtime language, and any
compiled target that goes through `Lossless.encode` loses a custom tag on
the way to its envelope.

**Where.** `lossless.zig` — `lossyStrip` (line ~175), `encode` (~138),
`decode` (~149) — and `flat_strip.zig` (~67): each builds `stripped` with
`node_comments` when any were seen, and nothing else off the side tables.
`materialize.zig` does it right (`out_tags`, `any_tags`), and is the
pattern.

**Done when** each rebuilder carries `node_tags`, `node_anchors`,
`node_marker_spans`/`node_sep_spans` where they are meaningful on the
print side, and `tag_directives`, or documents why a given one cannot; a
test through `printRuntime` that a tagged root reaches a runtime printer
tagged, and one through `Lossless.encode` that a YAML custom tag survives
an envelope round trip. fig-quickjs's `openstep.mjs` can then declare
`lossless: { plist_data: true }`.
