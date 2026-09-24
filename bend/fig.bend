# fig/bend — the core of fig, in Bend.
#
# Part 1: the line lexer.
#
# fig's dialect is line-oriented and nesting is carried by counted `>`
# markers, never by indentation, so one line lexes with no lookahead and no
# knowledge of its neighbours. In Bend that makes each line a fold of a pure
# state machine over its characters: `step` is not recursive, so Base's
# `List.foldl` does all the looping and the lexer terminates by construction.
import Base

type Span is Data:
  Span{start: Nat, end: Nat}

# Where the machine is within one line.
type Phase is Data:
  PMark{}   # the `> > ` marker run
  PKey{}    # a bare key, or the `*` element marker
  PSep{}    # spaces between the key and `=`
  PPre{}    # spaces between `=` and the value
  PVal{}    # a bare value
  PQuo{}    # inside a "..." value
  PCom{}    # after `#`: comment text

# The character classes the machine distinguishes.
type Cls is Data:
  CGt{}
  CSp{}
  CHash{}
  CEq{}
  CStar{}
  CQuote{}
  COther{}

def cls.sp(sp: Bool) -> Cls:
  match sp:
    case True{}:
      CSp{}
    case False{}:
      COther{}

def cls.pick(c: Bool, k: Cls, e: Cls) -> Cls:
  match c:
    case True{}:
      k
    case False{}:
      e

def classify(+c: Char) -> Cls:
  cls.pick(Char.is_eq(c, '>'), CGt{},
  cls.pick(Char.is_eq(c, '#'), CHash{},
  cls.pick(Char.is_eq(c, '='), CEq{},
  cls.pick(Char.is_eq(c, '*'), CStar{},
  cls.pick(Char.is_eq(c, '"'), CQuote{},
  cls.sp(Char.is_space(c)))))))

# One line's lexer state. `key`, `val` and `com` accumulate reversed.
type Lex is Data:
  Lex{phase: Phase, depth: Nat, col: Nat, elem: Bool, key: String,
      vstart: Nat, val: String, com: String}

def Lex.new() -> Lex:
  Lex{PMark{}, 0n, 0n, False{}, SNil{}, 0n, SNil{}, SNil{}}

# Each phase decides on the class of the next character. `+col` is the
# column of `c`; every arm advances to `1n+col`.
def step.mark(k: Cls, c: Char, depth: Nat, +col: Nat, elem: Bool,
              key: String, vs: Nat, val: String, com: String) -> Lex:
  match k:
    case CGt{}:
      Lex{PMark{}, 1n+depth, 1n+col, elem, key, vs, val, com}
    case CSp{}:
      Lex{PMark{}, depth, 1n+col, elem, key, vs, val, com}
    case CHash{}:
      Lex{PCom{}, depth, 1n+col, elem, key, vs, val, com}
    case CStar{}:
      Lex{PSep{}, depth, 1n+col, True{}, key, vs, val, com}
    case _:
      Lex{PKey{}, depth, 1n+col, elem, SCon{c, key}, vs, val, com}

def step.key(k: Cls, c: Char, depth: Nat, +col: Nat, elem: Bool,
             key: String, vs: Nat, val: String, com: String) -> Lex:
  match k:
    case CSp{}:
      Lex{PSep{}, depth, 1n+col, elem, key, vs, val, com}
    case CEq{}:
      Lex{PPre{}, depth, 1n+col, elem, key, vs, val, com}
    case _:
      Lex{PKey{}, depth, 1n+col, elem, SCon{c, key}, vs, val, com}

# After a key (or after `*`, which takes its value with no `=`).
def step.sep(k: Cls, c: Char, depth: Nat, +col: Nat, elem: Bool,
             key: String, vs: Nat, val: String, com: String) -> Lex:
  match k:
    case CSp{}:
      Lex{PSep{}, depth, 1n+col, elem, key, vs, val, com}
    case CEq{}:
      Lex{PPre{}, depth, 1n+col, elem, key, vs, val, com}
    case CHash{}:
      Lex{PCom{}, depth, 1n+col, elem, key, vs, val, com}
    case CQuote{}:
      Lex{PQuo{}, depth, 1n+col, elem, key, col, SCon{c, val}, com}
    case _:
      Lex{PVal{}, depth, 1n+col, elem, key, col, SCon{c, val}, com}

def step.pre(k: Cls, c: Char, depth: Nat, +col: Nat, elem: Bool,
             key: String, vs: Nat, val: String, com: String) -> Lex:
  match k:
    case CSp{}:
      Lex{PPre{}, depth, 1n+col, elem, key, vs, val, com}
    case CHash{}:
      Lex{PCom{}, depth, 1n+col, elem, key, vs, val, com}
    case CQuote{}:
      Lex{PQuo{}, depth, 1n+col, elem, key, col, SCon{c, val}, com}
    case _:
      Lex{PVal{}, depth, 1n+col, elem, key, col, SCon{c, val}, com}

# A `#` opens a comment only after whitespace (spec 3.4): `x/v1#frag` is
# one value. `val` is reversed, so its head is the previous character.
def step.val.hash(sp: Bool, c: Char, depth: Nat, col: Nat, elem: Bool,
                  key: String, vs: Nat, val: String, com: String) -> Lex:
  match sp:
    case True{}:
      Lex{PCom{}, depth, col, elem, key, vs, val, com}
    case False{}:
      Lex{PVal{}, depth, col, elem, key, vs, SCon{c, val}, com}

def ends_in_space(s: String) -> Bool:
  match s:
    case SNil{}:
      False{}
    case SCon{h, t}:
      Char.is_space(h)

def step.val(k: Cls, c: Char, depth: Nat, +col: Nat, elem: Bool,
             +key: String, vs: Nat, +val: String, com: String) -> Lex:
  match k:
    case CHash{}:
      step.val.hash(ends_in_space(val), c, depth, 1n+col, elem, key, vs, val, com)
    case _:
      Lex{PVal{}, depth, 1n+col, elem, key, vs, SCon{c, val}, com}

def step.quo(k: Cls, c: Char, depth: Nat, +col: Nat, elem: Bool,
             key: String, vs: Nat, val: String, com: String) -> Lex:
  match k:
    case CQuote{}:
      Lex{PVal{}, depth, 1n+col, elem, key, vs, SCon{c, val}, com}
    case _:
      Lex{PQuo{}, depth, 1n+col, elem, key, vs, SCon{c, val}, com}

def step.go(+c: Char, st: Lex) -> Lex:
  match st:
    case Lex{phase, depth, col, elem, key, vs, val, com}:
      match phase:
        case PMark{}:
          step.mark(classify(c), c, depth, col, elem, key, vs, val, com)
        case PKey{}:
          step.key(classify(c), c, depth, col, elem, key, vs, val, com)
        case PSep{}:
          step.sep(classify(c), c, depth, col, elem, key, vs, val, com)
        case PPre{}:
          step.pre(classify(c), c, depth, col, elem, key, vs, val, com)
        case PVal{}:
          step.val(classify(c), c, depth, col, elem, key, vs, val, com)
        case PQuo{}:
          step.quo(classify(c), c, depth, col, elem, key, vs, val, com)
        case PCom{}:
          Lex{PCom{}, depth, 1n+col, elem, key, vs, val, SCon{c, com}}

def step(st: Lex, c: Char) -> Lex:
  step.go(c, st)

def lex_line(line: String) -> Lex:
  List.foldl(~&2, ~Char, ~Lex, ~step, String.to_list(line), Lex.new())

# ---------------------------------------------------------------------------
# Lines
# ---------------------------------------------------------------------------

# `*` is an anonymous positional key (spec 6.2); every other key has a name.
type Key is Data:
  Elem{}
  Name{text: String}

# A value's text exactly as written, and where it was written. Keeping the
# span is what makes every edit a splice: fig never re-prints a document.
type Raw is Data:
  Raw{text: String, span: Span}

type Line is Data:
  Blank{}
  Note{depth: Nat, text: String}
  Entry{depth: Nat, key: Key, value: Maybe<&2, Raw>, trail: Maybe<&2, String>}

def mk_key(elem: Bool, rkey: String) -> Key:
  match elem:
    case True{}:
      Elem{}
    case False{}:
      Name{String.reverse(rkey)}

# `rval` is the value reversed, so its head is the value's end: trimming its
# start trims the value's trailing whitespace. Its leading whitespace was
# never recorded (`vstart` is the first non-space column).
def mk_raw.go(+off: Nat, text: String) -> Maybe<&2, Raw>:
  match text:
    case SNil{}:
      None{}
    case SCon{+h, +t}:
      Some{Raw{SCon{h, t}, Span{off, Nat.add(off, String.length(SCon{h, t}))}}}

def mk_raw(off: Nat, vs: Nat, rval: String) -> Maybe<&2, Raw>:
  mk_raw.go(Nat.add(off, vs), String.reverse(String.trim_start(rval)))

def mk_entry(off: Nat, depth: Nat, elem: Bool, key: String, vs: Nat,
             val: String, trail: Maybe<&2, String>) -> Line:
  Entry{depth, mk_key(elem, key), mk_raw(off, vs, val), trail}

# A line that reached `#` is a comment line if nothing came before it.
def finish.com(named: Bool, off: Nat, depth: Nat, elem: Bool, key: String,
               vs: Nat, val: String, com: String) -> Line:
  match named:
    case False{}:
      Note{depth, com}
    case True{}:
      mk_entry(off, depth, elem, key, vs, val, Some{com})

def is_named(+elem: Bool, +key: String) -> Bool:
  Bool.or(elem, Bool.not(String.is_empty(key)))

def finish(off: Nat, st: Lex) -> Line:
  match st:
    case Lex{phase, depth, col, +elem, +key, vs, val, com}:
      match phase:
        case PMark{}:
          Blank{}
        case PCom{}:
          finish.com(is_named(elem, key), off, depth, elem, key, vs, val,
                     String.trim(String.reverse(com)))
        case _:
          mk_entry(off, depth, elem, key, vs, val, None{})

# Lex every line, each knowing the byte offset where it starts.
def lines.step(acc: Nat & List<&2, Line>, line: String) -> Nat & List<&2, Line>:
  (+off, ls) = acc
  +line = line
  (Nat.add(off, 1n+String.length(line)), finish(off, lex_line(line)) <> ls)

def lex.fin(acc: Nat & List<&2, Line>) -> List<&2, Line>:
  (end, ls) = acc
  List.reverse(&2, Line, ls)

def lex(src: String) -> List<&2, Line>:
  lex.fin(List.foldl(~&2, ~String, ~Nat & List<&2, Line>, ~lines.step,
                     String.lines(src), (0n, Nil{})))

# ---------------------------------------------------------------------------
# Part 2: tokens. Depth markers become explicit Down/Up tokens, so the tree
# builder below matches structure instead of comparing numbers.
# ---------------------------------------------------------------------------

type Tok is Data:
  Down{}
  Up{}
  Item{key: Key, value: Maybe<&2, Raw>, lead: List<&2, String>,
       trail: Maybe<&2, String>}

# Tokens that move from depth `prev` to depth `d`. fig allows one level
# deeper at a time (a deeper jump is FigSkippedLevel; here it opens one) and
# any number shallower.
def moves.go(c: Cmp, prev: Nat, d: Nat) -> List<&2, Tok>:
  match c:
    case LT{}:
      [Down{}]
    case EQ{}:
      []
    case GT{}:
      List.replicate(Tok, Nat.sub(prev, d), Up{})

def moves(+prev: Nat, +d: Nat) -> List<&2, Tok>:
  moves.go(Nat.cmp(prev, d), prev, d)

# Fold state: depth of the previous entry, comment lines waiting for the next
# entry (reversed), and tokens so far (reversed).
type TokSt is Data:
  TokSt{prev: Nat, lead: List<&2, String>, toks: List<&2, Tok>}

def rev_onto(xs: List<&2, Tok>, acc: List<&2, Tok>) -> List<&2, Tok>:
  List.foldl(~&2, ~Tok, ~List<&2, Tok>, ~(a => x => x <> a), xs, acc)

def tok.step(st: TokSt, line: Line) -> TokSt:
  match st:
    case TokSt{prev, lead, toks}:
      match line:
        case Blank{}:
          TokSt{prev, lead, toks}
        case Note{d, text}:
          TokSt{prev, text <> lead, toks}
        case Entry{+d, key, value, trail}:
          it : Tok = Item{key, value, List.reverse(&2, String, lead), trail}
          TokSt{d, Nil{}, it <> rev_onto(moves(prev, d), toks)}

def tokens.fin(st: TokSt) -> List<&2, Tok>:
  match st:
    case TokSt{p, l, toks}:
      List.reverse(&2, Tok, toks)

def tokens(lines: List<&2, Line>) -> List<&2, Tok>:
  tokens.fin(List.foldl(~&2, ~Line, ~TokSt, ~tok.step, lines, TokSt{0n, Nil{}, Nil{}}))

# ---------------------------------------------------------------------------
# Part 3: scalars. A bare value is sniffed by the literal-else-string rule
# (spec 4.1); numbers keep their lexeme (spec 2), never a converted value.
# ---------------------------------------------------------------------------

type Scalar is Data:
  Null{}
  Flag{value: Bool}
  Num{raw: String, float: Bool}
  Str{text: String}

# Number shapes (spec 4.3, without radix, exponent or `_` separators):
# an optional sign, digits with no leading zero, then optionally `.digits`.
type NumSt is Data:
  NStart{}
  NSign{}
  NZero{}
  NInt{}
  NDot{}
  NFrac{}
  NBad{}

def num.digit(zero: Bool, st: NumSt) -> NumSt:
  match st:
    case NStart{}:
      Bool.pick(NumSt, zero, NZero{}, NInt{})
    case NSign{}:
      Bool.pick(NumSt, zero, NZero{}, NInt{})
    case NInt{}:
      NInt{}
    case NDot{}:
      NFrac{}
    case NFrac{}:
      NFrac{}
    case _:
      NBad{}

def num.other(st: NumSt, sign: Bool, dot: Bool) -> NumSt:
  match st sign dot:
    case NStart{} True{} _:
      NSign{}
    case NInt{} _ True{}:
      NDot{}
    case NZero{} _ True{}:
      NDot{}
    case _ _ _:
      NBad{}

def num.class(digit: Bool, +c: Char, st: NumSt) -> NumSt:
  match digit:
    case True{}:
      num.digit(Char.is_eq(c, '0'), st)
    case False{}:
      num.other(st, Bool.or(Char.is_eq(c, '-'), Char.is_eq(c, '+')), Char.is_eq(c, '.'))

def num.step.go(+c: Char, st: NumSt) -> NumSt:
  num.class(Char.is_digit(c), c, st)

def num.step(st: NumSt, c: Char) -> NumSt:
  num.step.go(c, st)

def sniff.num(st: NumSt, text: String) -> Scalar:
  match st:
    case NZero{}:
      Num{text, False{}}
    case NInt{}:
      Num{text, False{}}
    case NFrac{}:
      Num{text, True{}}
    case _:
      Str{text}

def sniff.lit(null: Bool, yes: Bool, no: Bool, +text: String) -> Scalar:
  match null yes no:
    case True{} _ _:
      Null{}
    case False{} True{} _:
      Flag{True{}}
    case False{} False{} True{}:
      Flag{False{}}
    case False{} False{} False{}:
      sniff.num(List.foldl(~&2, ~Char, ~NumSt, ~num.step, String.to_list(text), NStart{}), text)

# A quoted value is committed (spec 4.7): it is a string whatever it holds.
# (Escapes are left undecoded in this sketch.)
def sniff.quoted(quoted: Bool, +text: String) -> Scalar:
  match quoted:
    case True{}:
      Str{String.take(String.drop(text, 1n), Nat.sub(String.length(text), 2n))}
    case False{}:
      sniff.lit(String.eq(text, "null"), String.eq(text, "true"),
                String.eq(text, "false"), text)

def sniff(+text: String) -> Scalar:
  sniff.quoted(String.starts_with(text, "\""), text)

# ---------------------------------------------------------------------------
# Part 4: the tree.
#
# fig's Zig AST (src/ast/ast.zig) is a flat node array where containers point
# at their first child and every node at its `next_sibling`. The same
# first-child/next-sibling shape, written as one inductive type, is what lets
# every walk below recurse structurally on a single type — no mutual
# recursion, which Bend forbids.
#
# Comments ride on the node (`lead`, `trail`) instead of in a side-table: the
# side-table exists in Zig so that `eql` can skip trivia; here equality is a
# law you state, so the choice is free. Source spans ride on leaves (`Raw`).
# ---------------------------------------------------------------------------

type Node is Data:
  End{}
  Leaf{key: Key, value: Scalar, raw: Raw, lead: List<&2, String>,
       trail: Maybe<&2, String>, next: Node}
  Branch{key: Key, lead: List<&2, String>, trail: Maybe<&2, String>,
         kids: Node, next: Node}

# The builder's work items. Bend has no mutual recursion and a def cannot
# destructure a computed pair, so "build the children, then the siblings"
# is one def whose job says which step it is on (the guide's recipe for
# mutual recursion). Each job carries the finished result of the step before.
type Built is Data:
  Built{node: Node, rest: List<&2, Tok>}

type Job is Data:
  Run{toks: List<&2, Tok>}
  Kids{key: Key, lead: List<&2, String>, trail: Maybe<&2, String>,
       res: Built}
  LeafSibs{key: Key, value: Maybe<&2, Raw>, lead: List<&2, String>,
           trail: Maybe<&2, String>, res: Built}
  BranchSibs{key: Key, lead: List<&2, String>, trail: Maybe<&2, String>,
             kids: Node, res: Built}

# An entry with no value and no children is an empty container, which fig
# rejects (FigEmptyContainer); the sketch keeps it as one.
def mk_leaf(key: Key, value: Maybe<&2, Raw>, lead: List<&2, String>,
            trail: Maybe<&2, String>, next: Node) -> Node:
  match value:
    case None{}:
      Branch{key, lead, trail, End{}, next}
    case Some{Raw{+text, span}}:
      Leaf{key, sniff(text), Raw{text, span}, lead, trail, next}

# `fuel` bounds the work, so the checker sees every call shrink it; three
# units per token is always enough (a token costs at most three jobs).
def build(+fuel: Nat, job: Job) -> Built:
  match fuel:
    case 0n:
      Built{End{}, []}
    case 1n+f:
      match job:
        case Run{toks}:
          match toks:
            case Nil{}:
              Built{End{}, []}
            case Con{Up{}, rest}:
              Built{End{}, rest}
            case Con{Down{}, rest}:
              build(f, Run{rest})
            case Con{Item{k, v, l, t}, Con{Down{}, rest}}:
              build(f, Kids{k, l, t, build(f, Run{rest})})
            case Con{Item{k, v, l, t}, rest}:
              build(f, LeafSibs{k, v, l, t, build(f, Run{rest})})
        case Kids{k, l, t, Built{kids, rest}}:
          build(f, BranchSibs{k, l, t, kids, build(f, Run{rest})})
        case LeafSibs{k, v, l, t, Built{sibs, rest}}:
          Built{mk_leaf(k, v, l, t, sibs), rest}
        case BranchSibs{k, l, t, kids, Built{sibs, rest}}:
          Built{Branch{k, l, t, kids, sibs}, rest}

def parse.fin(res: Built) -> Node:
  Built{kids, rest} = res
  Branch{Name{""}, [], None{}, kids, End{}}

# The document root is an anonymous container holding the top-level entries.
def parse(src: String) -> Node:
  +toks = tokens(lex(src))
  parse.fin(build(Nat.mul(3n, 1n+List.length(&2, Tok, toks)), Run{toks}))

# ---------------------------------------------------------------------------
# Part 5: paths. `a.b.0` names key `a`, then key `b`, then element 0.
# ---------------------------------------------------------------------------

def key_is(key: Key, idx: Nat, seg: String) -> Bool:
  match key:
    case Elem{}:
      String.eq(Nat.show(idx), seg)
    case Name{k}:
      String.eq(k, seg)

def hit(key: Key, idx: Nat, path: List<&2, String>) -> Bool:
  match path:
    case Nil{}:
      False{}
    case Con{seg, rest}:
      key_is(key, idx, seg)

def is_last(path: List<&2, String>) -> Bool:
  match path:
    case Nil{}:
      False{}
    case Con{seg, rest}:
      List.is_empty(&2, String, rest)

def found(-A: Type, c: Bool, a: A, b: A) -> A:
  Bool.pick(A, c, a, b)

# Search a sibling chain for `path`; `idx` is the position in the chain.
# Bend cannot branch on a computed test inside a recursive def, so both the
# descent and the sibling walk are computed and the test picks one; that
# visits each node once, and the two halves are independent, so they are
# written as a parallel call and fork across cores on a large document.
def find(+n: Node, +path: List<&2, String>, +idx: Nat) -> Maybe<&2, Node>:
  match n:
    case End{}:
      None{}
    case Leaf{key, value, raw, lead, trail, next}:
      sib = find(next, path, 1n+idx)
      found(Maybe<&2, Node>, Bool.and(hit(key, idx, path), is_last(path)),
            Some{Leaf{key, value, raw, lead, trail, End{}}}, sib)
    case Branch{key, lead, trail, kids, next}:
      here sib = find(kids, List.tail(&2, String, path), 0n) find(next, path, 1n+idx)
      found(Maybe<&2, Node>, hit(key, idx, path),
            found(Maybe<&2, Node>, is_last(path),
                  Some{Branch{key, lead, trail, kids, End{}}}, here),
            sib)

def lookup(root: Node, path: String) -> Maybe<&2, Node>:
  match root:
    case Branch{k, l, t, kids, next}:
      find(kids, String.split(path, '.'), 0n)
    case _:
      None{}

# ---------------------------------------------------------------------------
# Part 6: lossless edits. fig never re-prints a document to edit it: it finds
# the bytes that spell the value and splices new bytes over exactly those
# (src/editor/splice.zig). Everything outside the span is untouched, which
# LAWS.bend states and PROOF.bend proves.
# ---------------------------------------------------------------------------

def splice(s: String, +a: Nat, b: Nat, x: String) -> String:
  +s = s
  String.append(String.take(s, a), String.append(x, String.drop(s, b)))

type Edit is Data:
  Ok{text: String}
  Err{msg: String}

def set.at(src: String, n: Maybe<&2, Node>, text: String) -> Edit:
  match n:
    case None{}:
      Err{"no such key"}
    case Some{Leaf{k, v, Raw{r, Span{a, b}}, l, t, x}}:
      Ok{splice(src, a, b, text)}
    case Some{_}:
      Err{"not a scalar (containers are replaced key by key)"}

# `fig set FILE PATH VALUE`: VALUE is written as given, in fig spelling.
def set(+src: String, path: String, text: String) -> Edit:
  set.at(src, lookup(parse(src), path), text)

# The end of the line holding byte `at`: a trailing comment runs to it.
def line_end(+src: String, +at: Nat) -> Nat:
  first = List.head(&2, String, String.lines(String.drop(src, at)))
  Nat.add(at, String.length(Maybe.default(&2, String, first, "")))

def comment.at(+src: String, n: Maybe<&2, Node>, text: String) -> Edit:
  match n:
    case None{}:
      Err{"no such key"}
    case Some{Leaf{k, v, Raw{r, Span{a, +b}}, l, t, x}}:
      Ok{splice(src, b, line_end(src, b), String.append(" # ", text))}
    case Some{_}:
      Err{"not a scalar"}

# `fig comment --inline FILE PATH TEXT`: sets (or replaces) the trailing
# comment of a value; the splice covers the value's end through its line's
# end, so an old comment is replaced and nothing else moves.
def comment(+src: String, path: String, text: String) -> Edit:
  comment.at(src, lookup(parse(src), path), text)

# ---------------------------------------------------------------------------
# Part 7: conversion. Printing is only for converting to another format; the
# comments ride along (JSON5 spells them `//`).
# ---------------------------------------------------------------------------

def esc.one(q: Bool, bs: Bool, c: Char) -> String:
  match q bs:
    case True{} _:
      "\\\""
    case False{} True{}:
      "\\\\"
    case False{} False{}:
      SCon{c, SNil{}}

def esc.go(+c: Char, acc: String) -> String:
  String.append(esc.one(Char.is_eq(c, '"'), Char.is_eq(c, '\\'), c), acc)

def esc(c: Char, acc: String) -> String:
  esc.go(c, acc)

def quote(s: String) -> String:
  String.concat(["\"", List.foldr(~&2, ~Char, ~String, ~esc, String.to_list(s), ""), "\""])

# A key stays bare when JSON5 reads it as an identifier.
def ident_char.go(+c: Char) -> Bool:
  Bool.or(Char.is_alpha(c), Bool.or(Char.is_digit(c), Char.is_eq(c, '_')))

def ident_char(c: Char) -> Bool:
  ident_char.go(c)

def bare_key.go(ok: Bool, +k: String) -> String:
  match ok:
    case True{}:
      k
    case False{}:
      quote(k)

def first_not_digit(s: String) -> Bool:
  match s:
    case SNil{}:
      False{}
    case SCon{h, t}:
      Bool.not(Char.is_digit(h))

def json5.key(key: Key) -> String:
  match key:
    case Elem{}:
      ""
    case Name{+k}:
      ok = Bool.and(first_not_digit(k), List.all(~&2, ~Char, ~ident_char, String.to_list(k)))
      String.append(bare_key.go(ok, k), ": ")

def json5.scalar(v: Scalar) -> String:
  match v:
    case Null{}:
      "null"
    case Flag{b}:
      Bool.show(b)
    case Num{raw, f}:
      raw
    case Str{s}:
      quote(s)

def json5.lead(cs: List<&2, String>, +ind: String) -> String:
  match cs:
    case Nil{}:
      ""
    case Con{c, rest}:
      String.concat([ind, "// ", c, "\n", json5.lead(rest, ind)])

def json5.trail(t: Maybe<&2, String>) -> String:
  match t:
    case None{}:
      ""
    case Some{c}:
      String.append(" // ", c)

def is_seq(kids: Node) -> Bool:
  match kids:
    case Leaf{Elem{}, v, r, l, t, n}:
      True{}
    case Branch{Elem{}, l, t, k, n}:
      True{}
    case _:
      False{}

def json5.open(seq: Bool) -> String:
  Bool.pick(String, seq, "[", "{")

def json5.close(seq: Bool) -> String:
  Bool.pick(String, seq, "]", "}")

# One sibling chain at indent `ind`. JSON5 allows a trailing comma, so every
# entry ends in one. The children and the rest of the chain don't depend on
# each other, so they print in parallel.
#
# The rest of the chain is joined with `String.append`, never as the last
# item of `String.concat`: concat ends in `append(last, "")`, which copies
# its last item, and copying the rest of the document at every sibling is
# quadratic (4 s instead of 0.04 s for 12k lines).
def json5.chain(+n: Node, +ind: String) -> String:
  match n:
    case End{}:
      ""
    case Leaf{key, value, raw, lead, trail, next}:
      String.append(String.concat([json5.lead(lead, ind), ind, json5.key(key),
                                   json5.scalar(value), ",", json5.trail(trail), "\n"]),
                    json5.chain(next, ind))
    case Branch{key, lead, trail, kids, next}:
      body rest = json5.chain(kids, String.append(ind, "  ")) json5.chain(next, ind)
      +seq = is_seq(kids)
      head = String.concat([json5.lead(lead, ind), ind, json5.key(key), json5.open(seq),
                            json5.trail(trail), "\n"])
      String.append(head, String.append(body,
        String.append(String.concat([ind, json5.close(seq), ",\n"]), rest)))

def json5(+root: Node) -> String:
  match root:
    case Branch{k, l, t, kids, next}:
      +seq = is_seq(kids)
      String.concat([json5.open(seq), "\n", json5.chain(kids, "  "), json5.close(seq), "\n"])
    case _:
      "null\n"

# `fig get FILE PATH`: a scalar prints as its source text, byte for byte; a
# container prints as JSON5.
def get.at(n: Maybe<&2, Node>) -> Edit:
  match n:
    case None{}:
      Err{"no such key"}
    case Some{Leaf{k, v, Raw{r, s}, l, t, x}}:
      Ok{String.append(r, "\n")}
    case Some{Branch{k, l, t, kids, x}}:
      Ok{json5(Branch{Name{""}, [], None{}, kids, End{}})}
    case Some{End{}}:
      Err{"no such key"}

def get(src: String, path: String) -> Edit:
  get.at(lookup(parse(src), path))
