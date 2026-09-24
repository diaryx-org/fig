#!/bin/sh
# The CLI's end of the runtime-language carrier, end to end: a helper
# executable (the Rust crate's `tinykv_helper` example, over
# `fig::helper::serve`) is named in a `languages.figl`, and the `fig` binary
# is driven through it — `lang list`, `lang check`, `get` by `--lang` and by
# extension, `set`, `insert`, `delete`, `comment`, `check`, `convert` both
# ways, `fmt`, a parse error with the helper's message and offset, and a
# refused helper. Run by `zig build check` with the built CLI as $1; run by
# hand as `sh tools/cli-lang-check.sh zig-out/bin/fig`.
#
# Skips with a note when cargo is absent, the same way the Rust test suite
# does: the helper is a cargo example, and a contributor without Rust still
# gets a useful `check`.
set -eu

fig="$1"
case "$fig" in /*) ;; *) fig="$PWD/$fig" ;; esac
root="$(cd "$(dirname "$0")/.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
    echo "cli-lang-check: cargo not found — skipping (the helper is a cargo example)."
    exit 0
fi

# Built from this tree's core, like the Rust tests (see checks.zig).
(cd "$root/bindings/rust" && FIG_SYS_FORCE_SOURCE=1 cargo build --quiet --example tinykv_helper)
helper="$root/bindings/rust/target/debug/examples/tinykv_helper"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"

fail() {
    echo "cli-lang-check: $1" >&2
    exit 1
}
expect() { # expect <label> <want> <got>
    if [ "$2" != "$3" ]; then
        printf 'cli-lang-check: %s\n--- want\n%s\n--- got\n%s\n' "$1" "$2" "$3" >&2
        exit 1
    fi
}

printf 'language[]\n> name = tinykv\n> extensions = [tkv]\n> command = [%s]\n' "$helper" > languages.figl
export FIG_LANGUAGES="$tmp/languages.figl"
export NO_COLOR=1

# `lang list` names it with what it can do; `lang check` runs the harness.
out="$("$fig" lang list 2>/dev/null)"
case "$out" in *"tinykv         read edit serialize  .tkv"*) ;; *) fail "lang list did not list tinykv: $out" ;; esac
out="$("$fig" lang check tinykv 2>/dev/null)"
expect "lang check" "tinykv: registered (read edit serialize); every sample parsed, printed and reparsed to the same tree, and took a no-op edit" "$out"

# `--lang` reads a file a compiled format owns by extension through the
# helper; the extension reaches it without the flag; a path prints a value
# with the line ended, as a compiled printer would.
printf '# secret\nA=1\nB=two\n' > secrets.env
cp secrets.env s.tkv
expect "get --lang" "$(printf '# secret\nA=1\nB=two')" "$("$fig" get secrets.env --lang tinykv 2>/dev/null)"
expect "get by extension" "$(printf '# secret\nA=1\nB=two')" "$("$fig" get s.tkv 2>/dev/null)"
expect "get a scalar" "1" "$("$fig" get s.tkv A 2>/dev/null)"
expect "get as json" "$(printf '{\n  "A": "1",\n  "B": "two"\n}')" "$("$fig" get s.tkv -o json -q 2>/dev/null)"

# `lang table` prints the node table the helper answered `parse` with —
# spans, texts and comments as it gave them, and what `check --against`
# compares.
expect "lang table" \
    '{"rows":[{"kind":"mapping","parent":null,"span":[0,19]},{"kind":"keyvalue","parent":0,"span":[9,12],"sep":[10,11]},{"kind":"string","parent":1,"span":[9,10],"text":"A"},{"kind":"string","parent":1,"span":[11,12],"text":"1"},{"kind":"keyvalue","parent":0,"span":[13,18],"sep":[14,15]},{"kind":"string","parent":4,"span":[13,14],"text":"B"},{"kind":"string","parent":4,"span":[15,18],"text":"two"}],"regions":[],"mentions":[],"comments":[{"node":2,"slot":"leading","style":"line","text":"secret"}]}' \
    "$("$fig" lang table s.tkv 2>/dev/null)"

# The editor over the helper: every edit lands, and the rest of the file is
# untouched.
"$fig" set s.tkv A 10 2>/dev/null
"$fig" insert s.tkv C three 2>/dev/null
"$fig" delete s.tkv B 2>/dev/null
"$fig" comment s.tkv C "about c" 2>/dev/null
expect "edited file" "$(printf '# secret\nA=10\n# about c\nC=three')" "$(cat s.tkv)"
expect "check" "ok: s.tkv (tinykv)" "$("$fig" check s.tkv 2>/dev/null)"

# Conversion both ways, and fmt through the helper's printer. A nested
# mapping is stripped before the print the way a compiled flat format's
# is: the helper declared max_mapping_depth 0, and 0 is a limit, not the
# "no limit" sentinel.
expect "convert out" "$(printf '{\n  "A": "10",\n  "C": "three"\n}')" "$("$fig" convert s.tkv -o json -q 2>/dev/null)"
printf '{"k":"v"}' > j.json
expect "convert in" "k=v" "$("$fig" convert j.json -o tinykv 2>/dev/null)"
printf 'a:\n  b: 1\nc: 2\nd: true\n' > n.yaml
expect "flat strip" "$(printf 'c=2\nd=true')" "$("$fig" get n.yaml -o tinykv 2>/dev/null)"
printf 'A=1\n\n\nB=2\n' > f.tkv
"$fig" fmt f.tkv 2>/dev/null
expect "fmt" "$(printf 'A=1\nB=2')" "$(cat f.tkv)"

# An edit whose text does not reparse is reported as the text's fault, and
# names the language (a runtime format has no `@tagName`).
set +e
err="$("$fig" set s.tkv D "$(printf 'two\nlines')" 2>&1 >/dev/null)"
status=$?
set -e
[ "$status" -eq 1 ] || fail "a value with a line break the helper refuses exited $status, want 1"
case "$err" in *"is not a valid value for s.tkv (tinykv)"*) ;; *) fail "bad edit text not reported for the language: $err" ;; esac

# A parse failure is reported with the helper's message and at its offset.
printf 'A=1\nnope\n' > bad.tkv
set +e
err="$("$fig" get bad.tkv 2>&1 >/dev/null)"
status=$?
set -e
[ "$status" -eq 1 ] || fail "a parse error exited $status, want 1"
case "$err" in *"expected key=value"*"bad.tkv:2:1"*) ;; *) fail "parse error not reported at the helper's offset: $err" ;; esac

# A name nobody configured, and a helper that cannot start, are refused
# with the reason.
set +e
err="$("$fig" get s.tkv --lang nosuch 2>&1 >/dev/null)"
status=$?
set -e
[ "$status" -eq 2 ] || fail "--lang nosuch exited $status, want 2"
case "$err" in *"No language named \`nosuch\`"*) ;; *) fail "unknown --lang not refused by name: $err" ;; esac
printf 'language[]\n> name = broken\n> command = [/nonexistent/helper]\n' > broken.figl
set +e
out="$(FIG_LANGUAGES="$tmp/broken.figl" "$fig" lang check broken 2>/dev/null)"
status=$?
set -e
[ "$status" -eq 1 ] || fail "lang check of a broken helper exited $status"
case "$out" in "broken: refused: could not start \`/nonexistent/helper\`: "*) ;; *) fail "broken helper not reported: $out" ;; esac

echo "cli-lang-check: the CLI reads, edits, converts and checks through a configured helper"
