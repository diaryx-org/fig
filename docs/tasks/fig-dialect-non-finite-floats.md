```fig
title = An infinite or NaN float written to the fig dialect reads back as a string
description = The fig printer writes a non-finite number's raw text bare (`.inf`, `inf`), which the fig dialect sniffs as a string; it has to be spelled `a: float = inf`
status = open
created = 2026-10-01
updated = 2026-10-01
part_of = [Tasks](/docs/tasks/tasks.md)
```

# An infinite or NaN float written to the fig dialect reads back as a string

**Repro.** The CLI, converting YAML to fig and back:

```
$ printf 'a: .inf\n' > x.yaml
$ fig convert -o fig x.yaml
a = .inf
$ printf 'a = .inf\n' > y.fig
$ fig convert -o json y.fig
{
  "a": ".inf"
}
```

The Rust binding does the same: `Editor::replace_value` with
`Value::Float(f64::INFINITY)` on a `.fig` document writes `version = .inf`
and reports success, and the value reads back as the string `".inf"`.

**Cause.** A number node keeps its source's lexeme in `raw`, and the fig
printer writes it verbatim (`languages/fig/printer.zig`, `value`). The fig
dialect has no bare spelling for a non-finite float: `inf`, `.inf` and
`Infinity` all sniff to strings, and the only spelling is the annotated
`a: float = inf`, which the parser lifts to an extended `number_special`.
The other printers learned to respell these through `util/number.zig`'s
`nonFinite` in 5.1.2; the fig printer was left out because the annotation
exists only in block position, so it touches flow eligibility
(`computeInlineWidth`) and the editor's scalar splice, which carries no
annotation.

**Done when** a non-finite number node prints as `: float = inf` (and
`-inf`, `nan`) in block position, forces its container out of flow as a
`number_special` does, and an editor replace of a fig scalar with one either
writes the annotation or refuses with a sentence saying why.
