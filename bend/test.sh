#!/bin/sh
# fig/bend's checks: the laws, the type checker, and the README's edit, byte
# for byte. Needs `bend` on PATH (curl -fsSL https://bend-lang.com/install.sh | sh)
# and clang 14+.
set -eu
cd "$(dirname "$0")"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export BEND_NO_TELEMETRY=1

echo "laws:";  bend PROOF.bend
echo "core:";  bend fig.bend
bend main.bend -o "$tmp/fig"

cp testdata/config.fig "$tmp/config.fig"
"$tmp/fig" set "$tmp/config.fig" service.replicas 5
"$tmp/fig" comment "$tmp/config.fig" service.replicas "bumped for Black Friday"
cmp "$tmp/config.fig" testdata/config.edited.fig
# The edit is one line: every other byte is preserved.
test "$(diff testdata/config.fig "$tmp/config.fig" | grep -c '^[<>]')" = 2

"$tmp/fig" json5 "$tmp/config.fig" > "$tmp/out.json5"
cmp "$tmp/out.json5" testdata/config.edited.json5

test "$("$tmp/fig" get "$tmp/config.fig" service.name)" = api
test "$("$tmp/fig" get "$tmp/config.fig" service.ports.1)" = 443
status=0; "$tmp/fig" get "$tmp/config.fig" nope 2>/dev/null || status=$?
test "$status" = 1
status=0; "$tmp/fig" frobnicate x 2>/dev/null || status=$?
test "$status" = 2
echo "edits: ok"
