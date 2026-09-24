#!/bin/sh
# The built CLI's usage errors, end to end — the half of argument parsing
# the unit tests cannot reach, since the test runner fails any test that logs
# an error. An unknown flag and a surplus positional are exit 2 with nothing
# on stdout and no file touched; `--` ends the flags; a missing comment is
# exit 1, like a missing path. Run by `zig build check` with the built CLI as
# $1; run by hand as `sh tools/cli-args-check.sh zig-out/bin/fig`.
set -eu

fig="$1"
case "$fig" in /*) ;; *) fig="$PWD/$fig" ;; esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"

fail() {
    echo "cli-args-check: $1" >&2
    exit 1
}
# exits <label> <want> <command...>: the command exits <want> and, when that
# is not 0, prints nothing on stdout.
exits() {
    label="$1" want="$2"
    shift 2
    set +e
    out="$("$fig" "$@" 2>/dev/null)"
    got=$?
    set -e
    [ "$got" -eq "$want" ] || fail "$label: exited $got, want $want"
    [ "$want" -eq 0 ] || [ -z "$out" ] || fail "$label: printed to stdout: $out"
}

printf '# lead\na: 1\nb: 2 # trail\n' >f.yaml
cp f.yaml ./-x.yaml
cp f.yaml orig.yaml

# An unknown flag is a usage error, never a file name — in every action.
exits "get --bogus" 2 get --bogus f.yaml
exits "set --bogus" 2 set --bogus f.yaml a 1
[ ! -e ./--bogus ] || fail "set --bogus created a file named --bogus"
exits "edit --bogus" 2 edit f.yaml a 1 --bogus
exits "insert -x" 2 insert -x f.yaml c 1
exits "delete --bogus" 2 delete --bogus f.yaml a
exits "comment --bogus" 2 comment --bogus f.yaml a hi
exits "check --bogus" 2 check --bogus f.yaml
exits "fmt --bogus" 2 fmt --bogus f.yaml
exits "convert --bogus" 2 convert --bogus -o json f.yaml
exits "patch --bogus" 2 patch --bogus f.yaml orig.yaml
exits "lang --bogus" 2 lang --bogus

# One positional past the action's last is a usage error.
exits "get surplus" 2 get f.yaml a b
exits "set surplus" 2 set f.yaml a 1 2
exits "edit surplus" 2 edit f.yaml a 1 2
exits "insert surplus" 2 insert f.yaml c 1 2
exits "delete surplus" 2 delete f.yaml a b
exits "comment surplus" 2 comment f.yaml a hi there
exits "comment --get surplus" 2 comment --get f.yaml a b
exits "lang table surplus" 2 lang table f.yaml g.yaml
exits "lang verb" 2 lang bogus
cmp -s f.yaml orig.yaml || fail "a refused command line changed f.yaml"

# `--` ends the flags; `-` is stdin; a negative number is a value.
exits "get -- -x" 0 get -- -x.yaml a
[ "$("$fig" get -- -x.yaml a 2>/dev/null)" = 1 ] || fail "get -- -x.yaml a did not read -x.yaml"
[ "$(printf 'a: 7\n' | "$fig" get - -i yaml a 2>/dev/null)" = 7 ] || fail "get - did not read stdin"
exits "set negative" 0 set f.yaml a -5
[ "$("$fig" get f.yaml a 2>/dev/null)" = -5 ] || fail "set f.yaml a -5 did not write -5"
exits "set after --" 0 set f.yaml a -- --dash
[ "$("$fig" get f.yaml a 2>/dev/null)" = --dash ] || fail "set after -- did not write --dash"

# A missing comment is a missing path; an absent one to delete is a no-op.
exits "comment --get present" 0 comment --get f.yaml a
[ "$("$fig" comment --get f.yaml a 2>/dev/null)" = lead ] || fail "comment --get f.yaml a"
exits "comment --get missing" 1 comment --get f.yaml b
exits "comment --get --inline missing" 1 comment --get --inline f.yaml a
exits "comment --get no such path" 1 comment --get f.yaml zz
exits "comment --delete missing" 0 comment --delete f.yaml b

echo "cli-args-check: ok"
