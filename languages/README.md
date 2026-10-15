```fig
title = Languages
part_of = [fig](/README.md)
```

# Languages

Every format fig reads is a language, and fig itself carries none: a
program registers the languages it uses, and each brings its formats.
Each language fig ships is a package of its own, in a folder of its own
here: the language written in Rust, and from it its bindings for C, Rust
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

Each folder is also a worked example of writing a language for fig and
binding it for three more. A language depends on fig's kit (`fig-kit`, in
`rust/fig-kit/`) and on nothing else of fig's, and the build holds it to
that: without its default features the crate's one dependency is the kit,
so a language that reached for fig's internals would not compile.

## A language's folder

YAML's, for one:

```
yaml/
  Cargo.toml           its Rust crate, fig-yaml
  rust/
    lib.rs             the language: `LANGUAGE`, and its `fig` binding API
    tokenizer.rs       …
    parser.rs          …and the rest of the language
    printer.rs
  include/fig/yaml.h   its C header
  package.json         its npm package, @diaryx/fig-yaml
  js/
    build.mjs
    yaml.d.ts
  tests/edits.json     fig's editor over the language, as fixtures
  README.md            for the folder, the crate and the npm package
```

From the repository root, `cargo test -p fig-yaml` runs the language's
tests. Its C library and its WebAssembly module are built from
`rust/fig-lang-export`, which wraps any one of the languages (below).

## The language

`rust/lib.rs` declares `LANGUAGE`, a value implementing the kit's
`Language` trait (`rust/fig-kit/src/language.rs`), the language's half of
fig's contract. It must say its `name`, `extensions`, `caps` and
`dialects`, parse (`parse_table`) and print, and give its `syntax` if it
can be edited: how fig's editor writes it, as values (its comment markers,
its key–value separator, the literal an empty mapping is spelled with, and
so on). It may give `samples`, small documents fig's checks parse, print,
reparse and edit when the language registers; and, where it spells a
fragment in a way no `syntax` value covers, a renderer: `render_value`,
`render_entry`, `render_item`, `render_tail`, `render_key`,
`render_key_name` or `render_string`, each a pure function to a string fig
splices. The trait's doc comments hold the full list, and why each is
there.

The parser writes a node table — a row per node, in pre-order, and
sparse side facts such as comments and the spans of separators — through
the kit's row writer; the printer reads the tree back. dotenv's
`rust/` and `.properties`' are the smallest whole languages here.

## C

`rust/fig-lang-export` builds a language as a C library, the language's
feature naming which:

```sh
cargo rustc -p fig-lang-export --release --features yaml --crate-type staticlib
```

Cargo writes it as `target/release/libfig_lang_export.a`, whatever the
language; copy it to `libfig_yaml.a`. It has one export,
`fig_yaml_register`, which `include/fig/yaml.h` declares. A C program
links `libfig_yaml.a` beside fig's own `libfig.a`, calls it, and uses the
format it hands back with the rest of `fig.h`. The library carries its own
copy of the kit, so a parse writes its own table and copies its rows into
fig's through `fig_table_extend`. `cargo xtask abi-check` builds the yaml
and json libraries and links them beside a libfig that names neither.

## Rust

The crate is the same folder: `Cargo.toml`, with its source in `rust/`.
It is the language itself, in pure Rust, written on fig's kit alone, with
no build script and no C. `rust/lib.rs` is the crate root: the language's
`LANGUAGE` and its modules (`fig_yaml::parser`, …), and, behind the
crate's default `fig` feature, `register()`, which registers it with the
`fig` binding and hands back its first `fig::Format`, with the serde
functions (`from_str`, `to_string`, …) behind its default `serde`
feature. Without its default features the crate is the language alone,
which fig's CLI, its C library and its core tests take and register with
`fig-core` themselves. Every crate is a member of the one Cargo workspace
at the repository root.

## JavaScript

`npm run build` in the folder runs `js/build.mjs`, which builds the
language's WebAssembly module with cargo (`fig-lang-export` with the
language's feature, `--crate-type cdylib --target wasm32-unknown-unknown`,
the size-first `wasm` profile) and wraps it as `js/<name>.js`: the module,
base64, under the language's name — what `registerLanguage` from
`@diaryx/fig` takes. `js/<name>.d.ts` types it. The package pins
`@diaryx/fig` as a peer at its own version, since the two modules share
the table's layout. The build needs
`rustup target add wasm32-unknown-unknown`.

## Tests

`cargo test -p <crate>` runs the language's own tests. `tests/edits.json`
holds fig's editor over the language: each case an edit and the exact bytes
(or the refusal) it must give. `cargo xtask ci` runs every language's
fixtures through the Rust editor, behind fig-conformance's adapter
(`rust/fig-conform`), and again through the TypeScript binding, so the
JavaScript packages are held to the same bytes.

## A language outside fig

A Rust crate written elsewhere is written as fig's own are: on
`fig-kit`, implementing its `Language` trait, and registered with the
`fig` binding (`fig::language::register_core`).

A language written in another language altogether — C, Zig, JavaScript,
or anything that speaks fig's helper protocol — registers through the same
contract at runtime; see `fig.h`'s runtime section,
[fig in TypeScript](../docs/typescript.md) and
[the helper protocol](../docs/helper-protocol.md). The kit's number and
datetime rules reach it too, so it spells a number and reads a datetime
as these languages do: `fig_kit_*` in `fig.h`, `fig::kit` in Rust, and
`@diaryx/fig/kit/number` and `/datetime` in JavaScript.

## Writing a language in another binding

The languages here are written in Rust and bound for C, Rust and
JavaScript. A language can as well be written in any of them, with the
same kit: the rules (`number`, `datetime`, `literal`, `json_string`,
`xml`), a row writer that takes rows in any order and finishes them into
pre-order, and, where the binding's idiom has room for one, a tree, a
scanner, grammar combinators and a printer's writer. `kit/testdata/` in
the repository holds every binding's kit to the same answers, and each
binding has dotenv written with its kit:

| Binding    | The kit                                  | dotenv written with it                    | Held to `dotenv` by |
| ---------- | ---------------------------------------- | ----------------------------------------- | ------------------- |
| C          | `fig_kit_*`, `fig_table_*` (`fig.h`)     | `bindings/c/examples/dotenv.c`            | nothing yet: the Zig test that held it (`src/kit_twins_test.zig`) went with the Zig |
| Rust       | `fig::kit`                               | `bindings/rust/fig/examples/kit_dotenv_helper` | `tests/kit_dotenv.rs`, and `fig lang check` in `tools/cli-lang-check.sh` |
| JavaScript | `@diaryx/fig/kit`                        | `bindings/typescript/js/dotenv.js`        | `test/twins.test.ts` |

A language written outside this repository is registered at run time
(`fig_language_register`, `fig::language::register`, `registerLanguage`)
or served to the `fig` command line as a helper; `fig lang check <name>
--against <format>` holds it to a compiled sibling.
