#!/bin/sh
# `fig patch` as the writing half of a jq pipeline, end to end: `fig convert
# -o json f | jq '.n += 1' | fig patch f -` changes `n` and no other byte, on
# a YAML, a TOML and a JSON5 file full of number spellings JSON has no words
# for — hex, octal, binary, `_`, a `+`, bare dots, exponents, trailing zeros.
# JSON hands each back in decimal, and patch keeps the target's spelling of
# every value the two agree on. Run by `ship ci` with the built CLI
# as $1; run by hand as `sh tools/cli-patch-check.sh target/debug/fig`.
#
# Skips with a note when jq is absent or older than 1.7: 1.6 rewrites every
# number through a double (`1e3` comes back as the integer `1000`, a 20-digit
# integer loses its tail), so the patch it writes really does change them.
set -eu

fig="$1"
case "$fig" in /*) ;; *) fig="$PWD/$fig" ;; esac

jq="$(command -v jq || true)"
if [ -z "$jq" ]; then
    echo "cli-patch-check: jq not found — skipping."
    exit 0
fi
case "$("$jq" --version)" in
jq-1.[0-6]*)
    echo "cli-patch-check: $("$jq" --version) predates 1.7 — skipping (it rewrites number literals)."
    exit 0
    ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"

# roundtrip <file> <want>: bump `n` through JSON and jq, patch it back, and
# compare the whole file.
roundtrip() {
    cp "$1" orig
    "$fig" convert -q -o json "$1" | "$jq" '.n += 1' | "$fig" patch -q "$1" -
    if [ "$(cat "$1")" != "$2" ]; then
        printf 'cli-patch-check: %s\n--- want\n%s\n--- got\n%s\n' "$1" "$2" "$(cat "$1")" >&2
        exit 1
    fi
    # A second run is the no-op `fig patch --help` promises.
    cp "$1" once
    "$fig" convert -q -o json "$1" | "$fig" patch -q "$1" -
    cmp -s "$1" once || { echo "cli-patch-check: $1: re-applying its own JSON changed it" >&2; exit 1; }
}

cat >f.yaml <<'YAML'
# every one of these comes back from JSON spelled another way
n: 1
hex: 0x1F
upper: 0xDEAD_BEEF
oct: 0o755
mode: 0755
plus: +42
ratio: 1.50
exp: 1e3
small: 2.5E-03
dot: .5
trail: 5.
negzero: -0.0
big: 123456789012345678901234567890
list: [0x10, 1.0e2, +7]
YAML
roundtrip f.yaml "$(sed 's/^n: 1$/n: 2/' f.yaml)"

cat >f.toml <<'TOML'
# every one of these comes back from JSON spelled another way
n = 1
hex = 0x1F
upper = 0xDEAD_BEEF
oct = 0o755
bin = 0b1010
sep = 1_000_000
plus = +42
ratio = 1.50
exp = 1e3
small = 2.5E-03
fsep = 1_000.000_5
negzero = -0.0
big = 9_223_372_036_854_775_807
list = [0x10, 1.0e2, +7]
TOML
roundtrip f.toml "$(sed 's/^n = 1$/n = 2/' f.toml)"

cat >f.json5 <<'JSON5'
// every one of these comes back from JSON spelled another way
{
  n: 1,
  hex: 0x1F,
  upper: 0XdeadBEEF,
  neghex: -0x10,
  plus: +42,
  ratio: 1.50,
  exp: 1e3,
  small: 2.5E-03,
  dot: .5,
  trail: 5.,
  big: 123456789012345678901234567890,
  list: [0x10, 1.0e2, +7,],
}
JSON5
roundtrip f.json5 "$(sed 's/^  n: 1,$/  n: 2,/' f.json5)"

echo "cli-patch-check: ok"
