#!/usr/bin/env bash
#
# Prove one payload crate does what it is for: build its archives, then link
# fig and its languages for the target from them alone, with Zig shadowed by
# a `zig` that fails, and run the result where the host can.
#
#   scripts/check-payload.sh linux-x64-musl
#   scripts/check-payload.sh wasm32-wasi "wasmtime run"
#   scripts/check-payload.sh android-arm64 -
#
# The second argument is how to run the linked program: empty runs it
# directly (a host target, or one Rosetta or the kernel runs), a command is
# put in front of it, and `-` links without running (Android, whose programs
# need its own loader). The program is `fig`'s `wasm_smoke` example, which
# converts JSON to YAML through fig-json and fig-yaml; building it links
# every language crate, as fig's dev-dependencies.
#
# The Rust target must be installed (`rustup target add`), and a target the
# host cannot link for needs its linker named the way Cargo reads one
# (`CARGO_TARGET_<TRIPLE>_LINKER`).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)" # bindings/rust
key="${1:?usage: check-payload.sh <key> [runner | -]}"
runner="${2:-}"

rust_target="$(grep -v '^#' "$root/prebuilt-targets.tsv" | awk -F'\t' -v k="$key" '$3 == k { print $1 }')"
[ -n "$rust_target" ] || { echo "check-payload: no target with key '$key'" >&2; exit 1; }

bash "$here/build-payload-lib.sh" "$key"

# From here on any call to Zig is a failure: the payload must be enough.
nozig="$(mktemp -d)"
trap 'rm -rf "$nozig"' EXIT
printf '#!/bin/sh\necho "check-payload: zig was called ($*), the payload was not enough" >&2\nexit 97\n' >"$nozig/zig"
chmod +x "$nozig/zig"

# In a checkout fig-sys builds from source unless told to link the payload.
(cd "$root" && PATH="$nozig:$PATH" FIG_SYS_PREBUILT=1 \
    cargo build -p fig --example wasm_smoke --target "$rust_target")

ext=""
case "$rust_target" in wasm32-*) ext=".wasm" ;; *windows*) ext=".exe" ;; esac
program="$root/target/$rust_target/debug/examples/wasm_smoke$ext"
[ -f "$program" ] || { echo "check-payload: $program was not linked" >&2; exit 1; }

if [ "$runner" = "-" ]; then
    echo "check-payload: $key links ($rust_target); not run on this host"
    exit 0
fi
# `$runner` is a command and its arguments, split on spaces on purpose.
# shellcheck disable=SC2086
out="$($runner "$program")"
[ "$out" = "ok" ] || { echo "check-payload: $key printed '$out', not 'ok'" >&2; exit 1; }
echo "check-payload: $key links and runs ($rust_target)"
