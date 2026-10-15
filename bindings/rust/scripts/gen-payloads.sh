#!/usr/bin/env bash
#
# Generate the per-target `fig-sys-<key>` payload crates from
# prebuilt-targets.tsv. Each payload crate carries the prebuilt core
# (`lib/libfig.a`) and every language fig ships (`lib/libfig_<name>.a`),
# produced separately by build-payload-lib.sh / CI and git-ignored, and a tiny
# build script that hands their location to `fig-sys` via `links` metadata.
#
# Idempotent: safe to re-run after editing the table. Does NOT touch lib/.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)" # bindings/rust
table="$root/prebuilt-targets.tsv"

while IFS=$'\t' read -r rust_target zig_target key cfg; do
    [[ "$rust_target" =~ ^#|^$ ]] && continue
    dir="$root/fig-sys-$key"
    underscored="${key//-/_}"
    links="fig_prebuilt_$underscored"
    libname="fig_sys_$underscored"
    # `tr`, not `${links^^}`, which macOS's bash 3.2 does not have.
    links_upper="$(printf '%s' "$links" | tr '[:lower:]' '[:upper:]')"
    # Zig names a static library `fig.lib` on Windows and `libfig.a` elsewhere.
    # rustc's `-l static=fig` resolves either name from the search dir.
    if [[ "$rust_target" == *windows* ]]; then archives="lib/*.lib"; else archives="lib/*.a"; fi
    mkdir -p "$dir/src"

    cat >"$dir/Cargo.toml" <<EOF
[package]
name = "fig-sys-$key"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
description = "Prebuilt fig core and language libraries for $rust_target. Support crate for fig-sys; not for direct use."
repository = "https://github.com/diaryx-org/fig"
# The build script hands \`lib/\`'s location to \`fig-sys\` via this
# \`links\` key (as \`DEP_${links_upper}_LIBDIR\`). Unique per payload crate.
links = "$links"
# Allowlist: the prebuilt archives plus the trivial crate skeleton. \`lib/\` is
# git-ignored and populated by scripts/build-payload-lib.sh (or CI) before publish.
include = ["build.rs", "src/lib.rs", "$archives", "README.md"]

[lib]
name = "$libname"
path = "src/lib.rs"
EOF

    # lib/ holds the archives build-payload-lib.sh drops in; never committed.
    printf 'lib\n' >"$dir/.gitignore"

    cat >"$dir/build.rs" <<'EOF'
// Hand the prebuilt archives' directory to `fig-sys`'s build script. Because
// this crate sets `links`, the `cargo:libdir=…` line below reaches dependents
// as `DEP_<LINKS>_LIBDIR`. This crate emits no link directives itself:
// `fig-sys` decides to link the core, and passes the directory on to the
// language crates, which link their own.
use std::env;
use std::path::Path;

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let libdir = Path::new(&manifest).join("lib");
    println!("cargo:libdir={}", libdir.display());
    println!("cargo:rerun-if-changed={}", libdir.display());
}
EOF

    cat >"$dir/src/lib.rs" <<EOF
//! Prebuilt fig core and language libraries for \`$rust_target\`.
//!
//! Support crate for [\`fig-sys\`](https://docs.rs/fig-sys); do not depend on it
//! directly. The archives are delivered to \`fig-sys\`'s build script through
//! this crate's \`links\` metadata (see \`build.rs\`); there is no Rust API here.
EOF

    cat >"$dir/README.md" <<EOF
# fig-sys-$key

Prebuilt fig core (\`libfig.a\`) and the library of every language fig ships
(\`libfig_json.a\`, …) for \`$rust_target\`, letting \`fig-sys\` and the language
crates (\`fig-json\`, \`fig-yaml\`, …) link on this target **without a Zig
toolchain**.

This is an implementation detail of [\`fig-sys\`](https://crates.io/crates/fig-sys)
and [\`fig\`](https://crates.io/crates/fig). You should not depend on it directly;
\`fig-sys\` pulls in the right payload crate automatically for your target.

## License

MIT OR Apache-2.0
EOF

    echo "gen-payloads: wrote fig-sys-$key ($rust_target)"
done <"$table"
