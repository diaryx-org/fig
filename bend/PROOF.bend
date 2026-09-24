# fig/bend — proofs of LAWS.bend. `bend PROOF.bend` prints "All terms check."
# only when every law there holds.
import Base
import ./LAWS.bend as Laws

# Induction on p. `take` is stuck on an unknown string at length 0n, so the
# base case also splits q.
def Laws.take_append(p, q):
  match p q:
    case SNil{} SNil{}:
      {==}
    case SNil{} SCon{h, t}:
      {==}
    case SCon{h, t} _:
      %Laws.take_append(t, q) : {SCon{h, String.take(String.append(t, q), String.length(t))} == SCon{h, _} : String}
      {==}

def Laws.drop_append(p, q, k):
  match p:
    case SNil{}:
      {==}
    case SCon{h, t}:
      Laws.drop_append(t, q, k)

def Laws.drop_append_exact(p, q):
  match p q:
    case SNil{} SNil{}:
      {==}
    case SNil{} SCon{h, t}:
      {==}
    case SCon{h, t} _:
      Laws.drop_append_exact(t, q)

# The prefix is `take(s, a)` and the rest of the edit is appended after it.
def Laws.splice_keeps_prefix(s, a, b, x):
  Laws.take_append(String.take(s, a), String.append(x, String.drop(s, b)))

# Skip the prefix (drop_append), then the new text (drop_append_exact).
def Laws.splice_keeps_suffix(s, a, b, x):
  Equal.trans(String,
    String.drop(String.append(String.take(s, a), String.append(x, String.drop(s, b))),
                Nat.add(String.length(String.take(s, a)), String.length(x))),
    String.drop(String.append(x, String.drop(s, b)), String.length(x)),
    String.drop(s, b),
    Laws.drop_append(String.take(s, a), String.append(x, String.drop(s, b)), String.length(x)),
    Laws.drop_append_exact(x, String.drop(s, b)))
