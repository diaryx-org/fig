```fig
title = fig in Bend
part_of = [fig docs](../docs/docs.md)
```

# The core of fig, in Bend

This directory holds a working sketch of fig's core written in
[Bend](https://bend-lang.com/) 2.0.27. It answers the question "what would fig
look like in Bend?" with code that runs. It is not a port. It covers the fig
dialect's block layer and the one idea everything else in fig serves: **an edit
is a splice**. That idea is proven here as a theorem.

```sh
curl -fsSL https://bend-lang.com/install.sh | sh    # installs to ~/.bend
bend PROOF.bend               # "All terms check." once every law holds
bend main.bend -o fig         # native binary via clang
./fig set testdata/config.fig service.replicas 5
./fig comment testdata/config.fig service.replicas "bumped for Black Friday"
./fig get testdata/config.fig service.ports.1
./fig json5 testdata/config.fig
./test.sh                     # all of the above, checked against goldens
```

The top-level README's demo reproduces byte for byte:

```diff
 service
 > name = api          # must match the DNS record
-> replicas = 2
+> replicas = 5 # bumped for Black Friday
 > ports
```

and `fig json5` keeps the comments (`testdata/config.edited.json5`).

| File | What it is |
| --- | --- |
| `fig.bend` | The core: lexer, tokens, scalars, tree, paths, splice edits, JSON5 printer (731 lines, 75 defs, 15 types) |
| `main.bend` | The CLI: file IO, dispatch, and exit codes (1 for the document, 2 for the command line) |
| `LAWS.bend` | What must hold. Human-owned claims |
| `PROOF.bend` | Proofs of those claims. `bend PROOF.bend` is the gate |
| `test.sh` | Laws, type check, golden edits, exit codes |

## How fig's core maps onto Bend

| fig (Zig) | fig (Bend) | Why it changed |
| --- | --- | --- |
| `ast.zig`: a flat `[]Node`, containers point at their first child, every node at its `next_sibling` | One inductive `Node` with `kids` and `next`, the same first-child/next-sibling shape | Bend has no mutual recursion. With one type, every walk recurses structurally on that type. A "`List<Node>` inside `Node`" design would need two mutually recursive walkers. |
| Comments in a node-id-indexed side-table (`node_comments`), so that `eql` can skip trivia | `lead`/`trail` fields on the node | The side-table exists to keep equality comment-blind. In Bend, equality is a law you state, so it can ignore any field it likes. |
| `Document.node_spans` beside the AST | `Raw{text, span}` on each leaf | Same fact, carried where it is used |
| `tokenizer.zig`: index-and-`while` scanning | A pure per-character state machine (`step`) run by Base's `List.foldl` | `step` is not recursive, so the lexer terminates by construction. Termination is mandatory in Bend. |
| `parser.zig`: a depth stack over lines | Depth markers become `Down`/`Up` tokens. A fuel-bounded `build` then matches on structure. | A recursive def may not branch on a computed test such as `depth == prev`. Structural tokens avoid the comparison. |
| Recursive descent with helper functions | One `build` def with a `Job` selector (`Run`/`Kids`/`LeafSibs`/`BranchSibs`) | This is the guide's recipe for mutual recursion. A def also cannot destructure a computed pair, so each step's result rides inside the next job. |
| `editor/splice.zig` plus review and tests | `splice` plus **proven laws** | See below |
| Allocator discipline (`owned_strings`, "aliases source, do not double free") | Nothing | The values are affine, a `match` frees what it opens, and there is no allocator to thread through |
| `c_api.zig` (5.3k lines), WASI builds, npm | `bend x.bend -o x.c` / `-o x.js`. JS can `import Fig from "./fig.bend"` directly. | Bend emits C and JS itself |

## What Bend buys: the lossless promise becomes a theorem

fig's headline is "fig produces a single-line diff; every other byte is
preserved." In Zig that promise is kept by review, by unit tests, and by
fuzzing. In Bend, every edit goes through `Fig.splice`, and `LAWS.bend` states
what a splice may not touch:

```python
law splice_keeps_prefix:   # every byte before the span survives
  for +s: String
  for +a: Nat
  for +b: Nat
  for +x: String
  {String.take(Fig.splice(s, a, b, x), String.length(String.take(s, a))) == String.take(s, a) : String}

law splice_keeps_suffix:   # every byte after it survives, right after the new text
  ...
  {String.drop(Fig.splice(s, a, b, x),
               Nat.add(String.length(String.take(s, a)), String.length(x)))
   == String.drop(s, b) : String}
```

`PROOF.bend` proves both by induction, through three lemmas about
`take`/`drop`/`append`. The proofs are checked, not just tested. Changing
`splice` to drop one byte too many (`String.drop(s, 1n+b)`) makes
`bend PROOF.bend` fail with the expected and observed terms.

Two more guarantees come for free:

- **Totality.** Every def in `fig.bend` passes Bend's termination checker. A
  lexer or parser that hangs on hostile input is ruled out statically. Today
  `fuzz.yml` notes that crashes and broken invariants are caught but hangs are
  not.
- **No memory bugs.** Nothing is freed by hand, so the class of bugs the Zig
  AST's `deinit` comments warn about cannot happen.

These laws would be natural to state next (not attempted here):

- `get(set(s, p, v), p) == v`: an edit lands where it says.
- For a set to a scalar, the output's lines outside the value's line equal the
  input's lines. This is the "single-line diff" claim itself.
- A canonical-form round trip, `parse(print(t)) == t`. fig already calls the
  canonical form the "lossless, single-spelling oracle", and that is a law
  waiting to be stated.

## What Bend costs

Honest findings from writing this:

- **Branching is expensive to write.** There is no `if`, and a `match` may
  only inspect a parameter or a pattern-bound variable. Every decision on a
  computed value (`is this char '>'?`) is therefore its own def. That is most
  of the 75 defs. Base gets around this with a forward-declared `law f` plus a
  helper that calls `f`, but the same pattern in user code is rejected ("an
  unfilled law is a dead claim"). The workable idioms are:
  - fold a non-recursive `step` (the lexer);
  - compute both recursive results and pick one (`find`, which still visits
    each node once);
  - a fuel-bounded selector def (`build`).
- **fig's dialect is the easy case.** Counted `>` markers make every line
  self-describing, so the parser needs no indentation stack and no lookahead.
  fig's `yaml/parser.zig` (4.3k lines) is a mutually recursive
  grammar with indentation state, and the Job-selector encoding would get
  heavy there. Formats would likely be added one line-oriented or
  token-oriented machine at a time.
- **Strings are cons lists of `Char`, and `Nat` is unary.** That suits proofs
  but is slow for bulk text. Measured with the compiled binary on a 4-core
  container:

  | Input | `get` | `set` | `json5` |
  | --- | --- | --- | --- |
  | 12k lines, 170 KB | 0.04 s | 0.07 s | 0.06 s |
  | 120k lines, 1.7 MB | 0.47 s | 0.74 s | 0.77 s |

  Scaling is linear, and fine for config files. A production port would lex
  from `Array<U32>` bytes with `U32` offsets, and keep `Nat`/`String` at the
  proof boundary (`splice`). Zig was not available in this environment, so
  there is no side-by-side number.
- **A Base gotcha.** `String.concat` ends in `append(last, "")`, so it copies
  its last element. Writing `concat([..., rest_of_document])` inside a
  recursive printer made `json5` quadratic (4 s instead of 0.04 s on 12k
  lines). The printer now appends the rest explicitly.
- **Parallelism is incidental here.** `find` and `json5.chain` fork the
  children and the rest of the chain with parallel calls
  (`a b = f(x) g(y)`). Those calls are unbalanced, and the timings were the
  same on one thread. Config files are small, and the GPU is irrelevant.
  Parallelism would matter for batch work (`fig fmt` over a tree of files),
  not for one document.
- **Young toolchain.** Bend 2 is at 2.0.x. There is no counterpart yet to
  Zig's `comptime`, which powers fig's language registry and its
  `validate` checks. Templates (`~f`) cover some of that ground.

## Not covered

This is a sketch of the core, not of fig. It supports:

- depth markers (`>`, contiguous or spaced), headers, `key = value` and
  `*` elements;
- leading and trailing `#` comments, including the rule that `#` opens a
  comment only after whitespace;
- `"…"` values, kept as committed strings but not unescaped;
- sniffing of `null`/`true`/`false`, and of decimal numbers with the
  leading-zero rule;
- dotted paths with element indexes, `get`, `set`, `comment`, and JSON5
  output.

It does not have:

- flow collections, multi-line strings, `: type =` annotations, datetimes,
  radix numbers, sections or `[]` headers, or `+` continuations;
- diagnostics and warnings;
- inserting new keys;
- the other ten formats, the canonical form, the LSP, or the C ABI.

Offsets count characters rather than bytes. That is consistent everywhere
here, but it differs from fig's byte spans for non-ASCII input.

## Verdict

A full rewrite would trade fig's speed and broad format coverage for proofs,
and the branching and mutual-recursion restrictions would weigh hardest on the
largest parsers (YAML, TOML). The best use of Bend here is probably a
**verified reference model** that runs beside the Zig implementation, not in
place of it. That model would be:

- the fig dialect;
- the splice engine;
- the laws above, plus the canonical round trip.

It would be differential-tested against the Zig CLI on the fuzz corpus. The
laws then serve as the normative part of `docs/spec.md`, in a form a machine
checks.
