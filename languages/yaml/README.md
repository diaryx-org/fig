# YAML for fig

YAML for [fig](https://github.com/diaryx-org/fig): fig's own parser,
printer and editing rules for the language, written in Rust, with its
bindings for C, Rust and JavaScript. fig carries no language of its own;
this one links beside it, and you register it before reaching for a
format of it.

It registers `yaml` (1.2.2), and each version as `yaml-<version>`:
`yaml-1.1`, `yaml-1.2`, and the same with a patch number. Each dialect is
a format, found by name.

## Rust

```toml
[dependencies]
fig = "6"
fig-yaml = "6"
```

```rust
let doc = fig::Document::parse(b"a: [1, 2]\n", fig_yaml::format()?)?;
```

`format()` registers the language the first time it is asked for. With
the crate's default `serde` feature it reads and writes typed values as
`serde_json` does, with `fig_yaml::from_str` and `fig_yaml::to_string`, and
their `slice`, `reader`, `vec` and `writer` twins.

The language is pure Rust, written on fig's kit alone, so the crate needs
no other toolchain and no native library, on any target.

## JavaScript

```sh
npm install @diaryx/fig @diaryx/fig-yaml
```

```ts
import { parse, registerLanguage } from "@diaryx/fig";
import yaml from "@diaryx/fig-yaml";

const format = registerLanguage(yaml);
parse("a: [1, 2]\n", format);
```

`@diaryx/fig-yaml` is the language compiled to a WebAssembly module of its
own, and pins `@diaryx/fig` at its own version.

## C

Build the C library from fig's repository root, where its Rust is wrapped
as a C library by `fig-lang-export`:

```sh
cargo rustc -p fig-lang-export --release --features yaml --crate-type staticlib
```

Copy `target/release/libfig_lang_export.a` to `libfig_yaml.a`, add this
folder's `include/` to the include path, link it beside fig's `libfig.a`, and
register it:

```c
#include <string.h>
#include <fig/yaml.h>

const char *src = "a: [1, 2]\n";
int format;
FigError err = { .size = sizeof err };
if (fig_yaml_register(&format, &err) != FIG_STATUS_OK) { /* err says why */ }
FigDocument *doc = NULL;
fig_parse((const uint8_t *)src, strlen(src), format, &doc);
```

## This folder

This folder is the language's own package: its crate in `Cargo.toml` and
`rust/`, its C header in `include/`, its npm package in
`package.json` and `js/`. It depends on fig's kit and on nothing else of
fig's. [fig's `languages/`](https://github.com/diaryx-org/fig/tree/main/languages)
says how a language package is put together.

## License

MIT OR Apache-2.0.
