```fig
title = Languages
part_of = [fig](/README.md)
```

# Languages

Every format fig reads is a language, and fig itself carries none: a
program registers the languages it uses, and each brings its formats.
Each language fig ships is a package of its own, in a folder of its own
here: the language written in Zig, and beside it its bindings for C, Rust
and JavaScript.

| Folder                       | Dialects                         | Rust crate       | npm package              |
| ---------------------------- | -------------------------------- | ---------------- | ------------------------ |
| [json](json/)                | `json`, `jsonc`, `json5`         | `fig-json`       | `@diaryx/fig-json`       |
| [yaml](yaml/)                | `yaml`, `yaml-1.1`, …            | `fig-yaml`       | `@diaryx/fig-yaml`       |
| [toml](toml/)                | `toml`, `toml-1.0`, …            | `fig-toml`       | `@diaryx/fig-toml`       |
| [zon](zon/)                  | `zon`                            | `fig-zon`        | `@diaryx/fig-zon`        |
| [figl](figl/)                | `figl`                           | `fig-figl`       | `@diaryx/fig-figl`       |
| [ini](ini/)                  | `ini`                            | `fig-ini`        | `@diaryx/fig-ini`        |
| [dotenv](dotenv/)            | `dotenv`                         | `fig-dotenv`     | `@diaryx/fig-dotenv`     |
| [properties](properties/)    | `properties`                     | `fig-properties` | `@diaryx/fig-properties` |
| [plist](plist/)              | `plist`                          | `fig-plist`      | `@diaryx/fig-plist`      |
| [nestedtext](nestedtext/)    | `nestedtext`                     | `fig-nestedtext` | `@diaryx/fig-nestedtext` |

Each folder is also a worked example of writing a language for fig in a
compiled language and binding it for three more. A language depends on
fig's kit (`kit/`) and on nothing else of fig's, and the build holds it
to that: the package has no path into fig, so a language that reached
for fig's internals would not compile.

## A language's folder

YAML's, for one:

```
yaml/
  build.zig            the Zig package: `language` from the kit
  build.zig.zon        its one dependency, the kit
  src/
    yaml.zig           the language: `Language`, and its tests
    tokenizer.zig      …
    parser.zig         …and the rest of the language
    printer.zig
    c.zig              the root of its C library
    wasm.zig           the root of its WebAssembly module
  include/fig/yaml.h   its C header
  Cargo.toml           its Rust crate, fig-yaml
  build.rs
  rust/lib.rs
  package.json         its npm package, @diaryx/fig-yaml
  js/
    build.mjs
    yaml.d.ts
  tests/editor.zig     fig's editor over the language
  README.md            for the folder, the crate and the npm package
```

In the folder, `zig build test` runs the language's tests, `zig build c`
builds its C library and header into `zig-out/`, and `zig build wasm` its
WebAssembly module. None of them builds fig.

## The language

`src/<name>.zig` declares `Language`, the language's half of fig's
contract. It must declare `Type` (its dialects, as an enum), `Parser`,
`Printer`, `default_type`, `parse`, `print`, `name`, `extensions`, `caps`
and `dialects`, and `syntax` if it can be edited: how fig's editor writes
it, as values (its comment markers, its key–value separator, the literal
an empty mapping is spelled with, and so on). It may declare
`parseTable`, a parse that writes its rows into the table fig hands it;
`samples`, small documents fig's checks parse, print, reparse and edit
when the language registers; and, where it spells a fragment in a way no
`syntax` value covers, a renderer: `renderValue`, `renderEntry`,
`renderItem`, `renderTail`, `renderKey`, `renderKeyName` or `renderString`,
each a pure function to a string fig splices. fig's
`src/contract/language.zig` holds the full list, and why each is there.

The parser writes a node table — a row per node, in pre-order, and
sparse side facts such as comments and the spans of separators — through
the kit's `RowWriter`; the printer reads the tree back. dotenv's
`src/dotenv.zig` and `.properties`' `src/properties.zig` are the smallest
whole `Language`s here.

## C

`src/c.zig` is the C library's root, three lines:

```zig
comptime {
    _ = @import("kit").c_language.Exports(@import("yaml.zig").Language, "yaml");
}
```

The kit's `c_language.Exports` gives the library one export,
`fig_yaml_register`, which `include/fig/yaml.h` declares. A C program
links `libfig_yaml.a` beside fig's own `libfig.a`, calls it, and uses the
format it hands back with the rest of `fig.h`. The library carries its own
copy of the kit, so a parse writes its own table and copies its rows into
fig's through `fig_table_extend`.

## Rust

The crate is the same folder: `Cargo.toml` beside `build.zig`, with its
source in `rust/lib.rs` and the Zig in its package. The build script is
one call to [`fig-language-build`](../bindings/rust/fig-language-build),
which runs `zig build c` for the target Cargo is building and links the
library:

```rust
fn main() {
    fig_language_build::compile("yaml");
}
```

`rust/lib.rs` declares the library's export and wraps it in `register()`,
which hands back the language's first `fig::Format`. `links =
"fig_yaml"` in `Cargo.toml` names the library, and the crate depends on
`fig` and `fig-sys`, whose `libfig.a` resolves the three functions the
language's library imports from fig.

## JavaScript

`src/wasm.zig` is the WebAssembly module's root, the same three lines with
the kit's `wasm_language.Exports`. `npm run build` builds the module and
runs `js/build.mjs`, which wraps it as `js/<name>.js`: the module, base64,
under the language's name — what `registerLanguage` from `@diaryx/fig`
takes. `js/<name>.d.ts` types it. The package pins `@diaryx/fig` as a
peer at its own version, since the two modules share the table's layout.

## Tests

`zig build test` in a folder runs the language's own tests, which need
only the kit. The editor tests in `tests/editor.zig` drive fig's editor
over the language, so they need fig — and a language fig ships cannot
depend on fig, which depends on it: Zig refuses the cycle. fig's own
`zig build test` compiles them, with fig and the language, and runs each
language's own tests too.

## A language outside fig

A crate written elsewhere builds as fig's own do: its build script calls
`fig_language_build::compile`, which builds a copy of the package beside
the kit `fig-language-build` carries, the kit of the fig release it
belongs to. In the copy's `build.zig.zon`, `fig_kit` names that kit, a
dependency fetched by URL stays, and any other by path is dropped; a
package whose tests need one, as plist's need JSON, asks
`b.available_deps` before it asks for it. `zig build` in the package
itself reaches the kit as its own `build.zig.zon` says, by path to a
checkout of fig's `kit/`: Zig fetches a package by URL only at a
repository's root, and the kit is a directory of fig's.

A language written in another language altogether — JavaScript, or
anything that speaks fig's helper protocol — registers through the same
contract at runtime; see [fig in TypeScript](../docs/typescript.md) and
[the helper protocol](../docs/helper-protocol.md). The kit's number and
datetime rules reach it too, so it spells a number and reads a datetime
as these languages do: `fig_kit_*` in `fig.h`, `fig::kit` in Rust, and
`@diaryx/fig/kit/number` and `/datetime` in JavaScript.
