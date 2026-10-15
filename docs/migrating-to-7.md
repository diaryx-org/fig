```fig
title = Migrating to 7.0
description = What changes for a Zig program that used fig's package, for anyone building fig from source, and for anyone fetching its release assets, moving to 7.0
author = adammharris
created = 2026-10-15
updated = 2026-10-15
part_of = [fig docs](/docs/docs.md)
```

# Migrating to 7.0

In 7.0 fig is Rust throughout: the core, the kit, the ten languages, the
editor, the command line, the C library and the WebAssembly modules the npm
packages carry are all built from one Cargo workspace, and the Zig they were
written in is gone. [The proposal](proposals/rust-core.md) argues the
change, and [the Rust port's record](/rust/README.md) is how it was held to
the Zig before the switch.

Most callers change nothing. The Rust crates' API, `@diaryx/fig` and each
`@diaryx/fig-<name>`, the `fig` command line, and the C ABI in
[`fig.h`](/bindings/c/include/fig.h) are what they were in 6.x, and
`FIG_ABI_VERSION` has not moved. The major is for the one surface that went
away: fig as a Zig package.

## Zig: use the C ABI

There is no `zig fetch` dependency, no `fig` module to `addImport`, and no
`kit/` for a Zig language to be written with. A Zig program reaches fig the
way a C program does, through `fig.h` and the C library.

Build the library from the repository with cargo:

```sh
cargo rustc -p fig-capi --release --crate-type staticlib
```

Cargo names it `target/release/libfig_capi.a`; it is the `libfig.a` the
header describes, so copy it to that name if your build expects it. Like
6.x's, it carries no language. Each language is a library of its own, built
from `rust/fig-lang-export` with the language's feature:

```sh
cargo rustc -p fig-lang-export --release --features yaml --crate-type staticlib
```

That writes `target/release/libfig_lang_export.a` whatever the language, so
copy each to its own name (`libfig_yaml.a`) before building the next. Include
`<fig/yaml.h>` from `languages/yaml/include/` and call
`fig_yaml_register(&format, &err)` as before. Both are Rust static libraries:
link the system libraries `cargo rustc … -- --print native-static-libs`
names beside them (`-lSystem -lc -lm` on macOS).

From Zig, `@cImport` the header, add `bindings/c/include` and each language's
`include/` to the include path, and link the archives:

```zig
exe.root_module.addIncludePath(b.path("vendor/fig/include"));
exe.root_module.addObjectFile(b.path("vendor/fig/libfig.a"));
exe.root_module.addObjectFile(b.path("vendor/fig/libfig_yaml.a"));
exe.root_module.link_libc = true;
```

What the Zig module offered and the C ABI does not — build options that
trimmed languages out, zero-copy reads over the source buffer, reflective
deserialization into a Zig type — has no replacement. Choosing languages is
now choosing which libraries to link.

A language written in Zig still registers: a C-ABI language is a
`FigLanguage` vtable, whatever compiled it, and `fig.h`'s runtime section
describes it.

## Building from source

Building fig needs a Rust toolchain (1.88 or newer), not Zig. Every
`zig build` step has a `cargo` equivalent:

| 6.x | 7.0 |
| --- | --- |
| `zig build` | `cargo build -p fig-cli` (the CLI and `fig-lsp`, in `target/debug/`) |
| `zig build -Doptimize=ReleaseSafe` | `cargo build --release -p fig-cli` |
| `zig build -D<name>=false` | gone: the CLI carries all ten formats; the C library carries none |
| `zig build c`, `zig build c-languages` | `cargo rustc -p fig-capi …`, `cargo rustc -p fig-lang-export --features <name> …` (above) |
| `zig build wasi` | `cargo build --release -p fig-cli --bin fig --target wasm32-wasip1` |
| `zig build check` | `ship ci` |
| `zig build version-sync`, `abi-check`, `semver-check`, `check-figl` | `cargo xtask version-sync`, `abi-check`, `semver-check`, `check-figl` |
| `zig-out/` | `target/` |

The Cargo workspace is the repository root now, not `bindings/rust/`, so its
`Cargo.lock` and `target/` are there too. The npm packages' `npm run build`
runs cargo and needs `rustup target add wasm32-unknown-unknown` (and
`wasm32-wasip1` for `@diaryx/fig-wasi`). See [Building](BUILD.md).

## Release assets

- **No `build_options.zig`.** It existed for a Zig on WASI to build fig from
  source, and there is no Zig source to build.
- **The tangled.org mirror attaches less.** Its release artifacts are the
  Linux x86_64 (musl, static) and WASI CLI. The macOS, aarch64 Linux and
  Windows binaries are on the GitHub release only.
- **The Linux binaries Homebrew installs are linked against glibc**, built by
  cargo on GitHub's Linux runners, where Zig built them static. A system whose
  glibc is older than the runner's cannot run them; the static Linux x86_64
  binary is the tangled artifact. The macOS binaries
  are native builds, as before.
- **`fig-wasi.wasm`** on the GitHub release is the Rust CLI with every format
  compiled in, run as before (`wasmtime run --dir=. fig-wasi.wasm get
  config.yaml`).

## `@diaryx/fig-wasi`

The command line is the same. Its host now preopens the working directory at
its own path and hands it to the CLI as `PWD`, rather than as `.`, and no
longer patches `node:wasi`'s rights checks, which Rust's WASI build does not
trip. On Windows the working directory is `/cwd` inside the module, and an
absolute Windows path (`C:\…`) does not resolve; use a path relative to the
working directory, as 6.x's documentation already advised.
