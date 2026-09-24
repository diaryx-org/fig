# fig/bend — the laws. Human-owned: each is an open claim until PROOF.bend
# proves it, and `bend PROOF.bend` fails while any is open or false.
#
# fig's promise is that an edit is a splice: `set` and `comment` rewrite the
# bytes of one span and nothing else ("every other byte is preserved"). In
# Zig that promise is kept by review and by tests; here it is a theorem about
# `Fig.splice`, the one function every edit goes through.
import Base
import ./fig.bend as Fig

# Appending never disturbs the front: the first |p| characters of p ++ q are p.
law take_append:
  for p: String
  for q: String
  {String.take(String.append(p, q), String.length(p)) == p : String}

# ...and dropping them leaves q, from any point past p on.
law drop_append:
  for p: String
  for q: String
  for k: Nat
  {String.drop(String.append(p, q), Nat.add(String.length(p), k)) == String.drop(q, k) : String}

# ...and dropping exactly |p| leaves exactly q.
law drop_append_exact:
  for p: String
  for q: String
  {String.drop(String.append(p, q), String.length(p)) == q : String}

# Every byte before the span survives an edit.
law splice_keeps_prefix:
  for +s: String
  for +a: Nat
  for +b: Nat
  for +x: String
  {String.take(Fig.splice(s, a, b, x), String.length(String.take(s, a))) == String.take(s, a) : String}

# Every byte after the span survives an edit, right after the new text.
law splice_keeps_suffix:
  for +s: String
  for +a: Nat
  for +b: Nat
  for +x: String
  {String.drop(Fig.splice(s, a, b, x),
               Nat.add(String.length(String.take(s, a)), String.length(x)))
   == String.drop(s, b) : String}
