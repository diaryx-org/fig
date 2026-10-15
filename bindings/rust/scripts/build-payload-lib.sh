#!/usr/bin/env bash
#
# Build the prebuilt libraries for one tier-1 target (by `key`, or `all`) and
# drop them into the matching `fig-sys-<key>/lib/` payload crate: the core
# (`libfig.a`, which carries no language) and every language fig ships
# (`libfig_<name>.a`). Uses Zig's cross-compiler, so every target builds from
# a single host (a macOS one, for the iOS SDK). Run before `cargo
# package`/publish of the payload crates (this is what CI does).
#
#   scripts/build-payload-lib.sh macos-arm64
#   scripts/build-payload-lib.sh all
#
# `fig-sys` links the core from the payload, and each language crate
# (`fig-json`, …) links its own library from it, through `fig-sys`'s
# `links` metadata; a language fig does not ship builds from source.
#
# Env: FIG_ZIG_ROOT overrides the Zig source root (defaults to the repo root).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"                 # bindings/rust
repo_root="$(cd "$root/../.." && pwd)"
zig_root="${FIG_ZIG_ROOT:-$repo_root}"
table="$root/prebuilt-targets.tsv"
want="${1:-all}"

build_one() {
    local rust_target="$1" zig_target="$2" key="$3"
    local dir="$root/fig-sys-$key"
    local prefix
    prefix="$(mktemp -d)"

    echo "build-payload-lib: $key ($rust_target -> zig $zig_target)"
    # The C library carries no helper wire, so the build needs none of its
    # dependency (zigjr), as fig-sys's source build passes.
    zig build install-c-lib c-languages \
        -Doptimize=ReleaseFast \
        -Dstrip=true \
        -Dcpu=baseline \
        -Dhelper-wire=false \
        -Dtarget="$zig_target" \
        --prefix "$prefix" \
        --build-file "$zig_root/build.zig"

    # Zig names a static library `<name>.lib` on Windows, `lib<name>.a`
    # elsewhere.
    local core="libfig.a"
    [[ "$rust_target" == *windows* ]] && core="fig.lib"
    [ -f "$prefix/lib/$core" ] || { echo "  ERROR: $prefix/lib/$core not produced" >&2; exit 1; }

    rm -rf "$dir/lib"
    mkdir -p "$dir/lib"
    local archive name
    shopt -s nullglob
    for archive in "$prefix"/lib/*.a "$prefix"/lib/*.lib; do
        name="$(basename "$archive")"
        # Apple's ld rejects static-archive members that aren't 8-byte
        # aligned, and Zig's archiver can emit an unaligned member. Repack
        # apple archives with `ar` (writes aligned members) at build time so
        # the shipped archive links cleanly with no repack on the consumer
        # side. `ar` (unlike macOS-only `libtool`) is available on any host
        # that cross-builds these.
        if [[ "$rust_target" == *apple* ]]; then
            local work; work="$(mktemp -d)"
            ( cd "$work" && ar x "$archive" && chmod u+rw ./*.o \
                && ar crs "$name" ./*.o )
            archive="$work/$name"
        fi
        cp "$archive" "$dir/lib/$name"
    done
    echo "  -> $dir/lib/ ($(ls "$dir/lib" | wc -l | tr -d ' ') archives, $(du -sh "$dir/lib" | cut -f1))"
}

matched=0
while IFS=$'\t' read -r rust_target zig_target key cfg; do
    [[ "$rust_target" =~ ^#|^$ ]] && continue
    if [ "$want" = "all" ] || [ "$want" = "$key" ]; then
        build_one "$rust_target" "$zig_target" "$key"
        matched=1
    fi
done <"$table"

[ "$matched" = 1 ] || { echo "build-payload-lib: no target matched '$want'" >&2; exit 1; }
