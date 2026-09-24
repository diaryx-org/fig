#!/bin/sh
# The built CLI's usage errors, end to end — the half of argument parsing
# the unit tests cannot reach, since the test runner fails any test that logs
# an error. An unknown flag and a surplus positional are exit 2 with nothing
# on stdout and no file touched; `--` ends the flags; a missing comment is
# exit 1, like a missing path; a value argument means the same thing in every
# format, and `get` prints a scalar as its text; and every action exits by the
# 0/1/2 table `fig --help` states. Run by `zig build check` with
# the built CLI as $1; run by hand as `sh tools/cli-args-check.sh zig-out/bin/fig`.
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

# A value argument is a fig value, spelled as each format spells it.
is() { # is <label> <want> <got>
    [ "$2" = "$3" ] || fail "$(printf '%s\n--- want\n%s\n--- got\n%s' "$1" "$2" "$3")"
}
printf '{"k": 0}\n' >v.json
printf 'k: 0\n' >v.yaml
printf 'k = 0\n' >v.toml
"$fig" set v.json n 5 2>/dev/null
"$fig" set v.json s hello 2>/dev/null
"$fig" set v.json z null 2>/dev/null
"$fig" set v.json l '[1, 2]' 2>/dev/null
"$fig" set v.json q '"5"' 2>/dev/null
"$fig" set v.json v --string 1.10 2>/dev/null
"$fig" edit v.json k true 2>/dev/null
is "values into JSON" '{"k": true, "n": 5, "s": "hello", "z": null, "l": [1,2], "q": "5", "v": "1.10"}' "$(cat v.json)"
"$fig" set v.yaml n 5 2>/dev/null
"$fig" set v.yaml s 'a: b' 2>/dev/null
"$fig" set v.yaml m '{a = 1}' 2>/dev/null
is "values into YAML" "$(printf "k: 0\nn: 5\ns: 'a: b'\nm:\n  a: 1")" "$(cat v.yaml)"
"$fig" set v.toml s hello 2>/dev/null
"$fig" set v.toml m '{a = 1, b = [x]}' 2>/dev/null
"$fig" set v.toml d --raw 1979-05-27T07:32:00 2>/dev/null
is "values into TOML" "$(printf 'k = 0\ns = "hello"\nm = { a = 1, b = ["x"] }\nd = 1979-05-27T07:32:00')" "$(cat v.toml)"
# A value that is not one is the command line's fault; one the format cannot
# hold is the document's. Neither writes anything.
cp v.toml orig.toml
exits "unclosed value" 2 set v.toml k '[1, 2'
exits "--string with --raw" 2 set v.toml k --string --raw x
exits "null into TOML" 1 set v.toml k null
cmp -s v.toml orig.toml || fail "a refused value changed v.toml"
rm -f new.toml
exits "null into a new TOML file" 1 set new.toml k null
[ ! -e new.toml ] || fail "a refused value left new.toml behind"

# `get` prints a scalar as its text and a newline, in every format; -o asks
# for that format's spelling.
printf '{"s": "hi", "n": 42, "z": null, "o": {"a": 1}}\n' >g.json
printf 's = "hi"\n' >g.toml
is "get a JSON string" hi "$("$fig" get g.json s 2>/dev/null)"
is "get a TOML string" hi "$("$fig" get g.toml s 2>/dev/null)"
is "get a number" 42 "$("$fig" get g.json n 2>/dev/null)"
is "get a null" null "$("$fig" get g.json z 2>/dev/null)"
is "get -o json" '"hi"' "$("$fig" get g.json s -o json 2>/dev/null)"
[ "$("$fig" get g.json s 2>/dev/null | od -An -c | tr -d ' ')" = 'hi\n' ] || fail "get of a scalar does not end in one newline"

# Exit status: every action holds each row of the contract `fig --help`
# states — 0 done, 1 failed on the document, 2 a wrong command line. Each
# row runs against fresh copies of the fixtures.
mkdir fixtures
printf 'a: 1\n' >fixtures/ok.yaml
printf 'a: [1\n' >fixtures/bad.yaml
printf 'a:   1\n' >fixtures/messy.yaml
printf 'b: 2\n' >fixtures/o.yaml
row() { # row <want> <args...>
    want="$1"
    shift
    rm -rf run && cp -R fixtures run
    set +e
    (cd run && "$fig" "$@" >/dev/null 2>&1)
    got=$?
    set -e
    [ "$got" -eq "$want" ] || fail "exit status: fig $*: exited $got, want $want"
}
row 0 get ok.yaml a
row 1 get bad.yaml
row 1 get ok.yaml zz
row 1 get nosuch.yaml
row 2 get ok.yaml 'a['
row 0 set ok.yaml b 2
row 1 set bad.yaml a 2
row 1 set ok.yaml a.x 1
row 2 set ok.yaml a '[1'
row 0 insert ok.yaml c 3
row 1 insert ok.yaml a 3
row 2 insert ok.yaml c
row 0 edit ok.yaml a 2
row 1 edit ok.yaml zz 2
row 2 edit ok.yaml a
row 0 delete ok.yaml a
row 1 delete ok.yaml zz
row 2 delete ok.yaml a b
row 0 comment ok.yaml a hi
row 1 comment --get ok.yaml a
row 1 comment bad.yaml a hi
row 2 comment ok.yaml
row 0 check ok.yaml
row 1 check bad.yaml
row 2 check
row 0 fmt --dry-run ok.yaml
row 1 fmt --dry-run messy.yaml
row 1 fmt bad.yaml
row 2 fmt --dry-run --diff ok.yaml
row 0 convert -o json ok.yaml
row 1 convert -o json bad.yaml
row 2 convert ok.yaml
row 0 patch --dry-run ok.yaml o.yaml
row 1 patch ok.yaml bad.yaml
row 2 patch ok.yaml
row 2 patch --at 'x[' ok.yaml o.yaml
row 0 lang list
row 1 lang table nosuch.yaml
row 2 lang bogus
row 0 version
row 2 nosuchaction

echo "cli-args-check: ok"
