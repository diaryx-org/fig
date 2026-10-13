# Dotenv for fig

Dotenv for [fig](https://github.com/diaryx-org/fig): fig's own parser,
printer and editing rules for the language, written in Zig, with its
bindings for C, Rust and JavaScript. fig carries no language of its own;
this one links beside it, and you register it before reaching for a
format of it.

It registers `dotenv`. Each dialect is a format, found by name.

## Rust

```toml
[dependencies]
fig = "6"
fig-dotenv = "6"
```

```rust
let format = fig_dotenv::register()?;
let doc = fig::Document::parse(b"A=1\n", format)?;
```

The crate compiles the language from its Zig with
[`fig-language-build`](https://docs.rs/fig-language-build), so building it
needs Zig 0.17 or later.

## JavaScript

```sh
npm install @diaryx/fig @diaryx/fig-dotenv
```

```ts
import { parse, registerLanguage } from "@diaryx/fig";
import dotenv from "@diaryx/fig-dotenv";

const format = registerLanguage(dotenv);
parse("A=1\n", format);
```

`@diaryx/fig-dotenv` is the language compiled to a WebAssembly module of its
own, and pins `@diaryx/fig` at its own version.

## C

Build `libfig_dotenv.a` and its header with `zig build c` in this folder,
link it beside fig's `libfig.a`, and register it:

```c
#include <string.h>
#include <fig/dotenv.h>

const char *src = "A=1\n";
int format;
FigError err = { .size = sizeof err };
if (fig_dotenv_register(&format, &err) != FIG_STATUS_OK) { /* err says why */ }
FigDocument *doc = NULL;
fig_parse((const uint8_t *)src, strlen(src), format, &doc);
```

## This folder

This folder is the language's own package: its Zig in `src/`, its C header
in `include/`, its crate in `Cargo.toml` and `rust/`, its npm package in
`package.json` and `js/`. It depends on fig's kit and on nothing else of
fig's. [fig's `languages/`](https://github.com/diaryx-org/fig/tree/main/languages)
says how a language package is put together.

## License

MIT OR Apache-2.0.
