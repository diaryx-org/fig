```fig
part_of = id:dx4hxn744/kv2bv2m
title = fig
version = 6.1.0
author = adammharris
created = 2026-05-08
updated = 2026-10-14T21:00:00-06:00
contents = [[fig docs](docs/docs.md), [Fig Banner Dark](/assets/fig-banner-dark.svg.yaml), [Fig Banner](/assets/fig-banner.svg.yaml), [fig on diaryx.org](/www/index.md), [Audiences](/vocab/audiences.md), [Languages](/languages/README.md)]
config = .config/prov.yaml
registry = registry.yaml
id = b9j9zgk
slug = fig
```

<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/fig-banner-dark.svg">
    <img src="assets/fig-banner.svg" width="220" alt="fig">
  </picture>
</h1>

<p align="center">
  <a href="https://github.com/diaryx-org/fig/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/diaryx-org/fig/ci.yml?branch=main" alt="CI"></a>
  <a href="https://crates.io/crates/fig"><img src="https://img.shields.io/crates/v/fig.svg" alt="crates.io"></a>
  <a href="https://www.npmjs.com/package/@diaryx/fig"><img src="https://img.shields.io/npm/v/%40diaryx%2Ffig.svg" alt="npm"></a>
  <a href="https://docs.rs/fig"><img src="https://img.shields.io/docsrs/fig" alt="docs.rs"></a>
  <a href="LICENSE-MIT"><img src="https://img.shields.io/crates/l/fig.svg" alt="license"></a>
</p>

`fig` edits config files losslessly: make an edit and every comment, blank line, quote and key order remains untouched. It converts between formats retaining as much as possible, including comments, and reports exactly what is dropped if a format doesn't support something.

`fig`'s command-line tool has ten formats built in. The underlying library is maintained for Rust,
TypeScript and C (and so any language with a C FFI, Zig among them) and has no languages built in. Every language is registered the same way, and a kit is included to make it easy to write your own in any of those languages.

## The CLI

Take a messy YAML file:

```yaml
# config.yaml — Deploy settings
service:
  name: api          # must match the DNS record
  replicas: 2
  ports: [80, 443]

defaults: &defaults
  retries: 3
  timeout: 30s

worker:
  <<: *defaults
  replicas: 1
```

Change a value and annotate it:

```bash
$ fig set config.yaml service.replicas 5
$ fig comment --inline config.yaml service.replicas "bumped for Black Friday"
```

```diff
@@ -2,5 +2,5 @@
 service:
   name: api          # must match the DNS record
-  replicas: 2
+  replicas: 5 # bumped for Black Friday
   ports: [80, 443]
```

One line changed.

Read through YAML's anchors and merge keys:

```bash
$ fig get config.yaml worker -o json
{
  "replicas": 1,
  "retries": 3,
  "timeout": "30s"
}
```

Convert to another format, and keep the comments:

```bash
$ fig get config.yaml service -o json5
{
  name: "api", // must match the DNS record
  replicas: 5, // bumped for Black Friday
  ports: [
    80,
    443
  ]
}
```

Merge one file into another, or one part of one file into part of another,
touching nothing the patch doesn't name. The two files need not share a
format:

```bash
$ cat overlay.toml
# overlay.toml — the us-west rollout
[service]
replicas = 8
region = "us-west"

$ fig patch config.yaml overlay.toml --diff
--- config.yaml
+++ config.yaml
@@ -1,8 +1,9 @@
 # config.yaml — Deploy settings
 service:
   name: api          # must match the DNS record
-  replicas: 5 # bumped for Black Friday
+  replicas: 8 # bumped for Black Friday
   ports: [80, 443]
+  region: us-west

 defaults: &defaults
   retries: 3
```

## Built-in formats

You can make your own formats with `fig`!
But these are the ones that are built-in:

| Format | Files | |
| --- | --- | --- |
| YAML | `.yaml`, `.yml` | 1.2.2 and 1.1; anchors, aliases, merge keys |
| JSON | `.json`, `.jsonc`, `.json5` | strict, JSONC and JSON5 |
| TOML | `.toml` | 1.1 and 1.0, datetimes |
| ZON | `.zon` | Zig Object Notation |
| NestedText | `.nt` | <https://nestedtext.org> |
| Java properties | `.properties` | |
| dotenv | `.env` | |
| INI | `.ini` | |
| Property list | `.plist` | Apple's XML plist |
| figl | `.figl` | fig's own authoring dialect |

## Installation

Install it with Homebrew, download it from GitHub Releases, or run it with no
install at all (Node 22+, see [docs/npm-wasi.md](docs/npm-wasi.md)):

```bash
brew install diaryx-org/tap/fig
npx @diaryx/fig-wasi get config.yaml
```

Run `fig help` for a list of commands.

## The library

The CLI is a thin layer over the `fig` library.

```rust
use fig::{Editor, Segment};

let path = [Segment::Key("service"), Segment::Key("replicas")];
let mut ed = Editor::open(src.as_bytes(), fig_yaml::format()?)?;
ed.set_value(&path, 5)?;
ed.set_trailing_comment(&path, "bumped for Black Friday")?;
std::fs::write("config.yaml", ed.source()?)?;

// serde_json-style, in any format you depend on
let cfg: Config = fig_yaml::from_str(ed.source()?)?;
let toml = fig_toml::to_string(&cfg)?;
```

```toml
[dependencies]
fig = "6"
fig-yaml = "6"   # one crate per format
fig-toml = "6"
```

fig is pure Rust: no C compiler or other toolchain. `serde` is optional: fig
has its own `#[derive(ToValue, FromValue)]`. See [Rust](docs/rust.md).

From TypeScript:

```ts
import { Editor, registerLanguage } from "@diaryx/fig";
import yaml from "@diaryx/fig-yaml";

using ed = Editor.open(src, registerLanguage(yaml));
ed.setValue(["service", "replicas"], 5);
ed.setTrailingComment(["service", "replicas"], "bumped for Black Friday");
```

See [TypeScript](docs/typescript.md), and for C (or Zig, or anything else
that calls C), [`bindings/c/include/fig.h`](bindings/c/include/fig.h).

## Bring your own format

A 'language' simply tells `fig`:
- how to parse its text into a table of nodes,
- how to print one back, 
- and how its syntax is spelled.

Bring your own comment markers, key–value separators,
and so forth, and `fig` will do everything else:

- comment-preserving edits: set, insert, delete, rename, move, reorder,
  comment out and back in
- conversion to and from every other registered format, comments included
- `diagnose`, `fig patch`, `fig get` and every other CLI action
- a check when it registers: fig parses, prints, reparses and edits its
  sample documents, and refuses a language that doesn't hold up

dotenv is the smallest language, for reference. Each built-in language package
builds a C library, a WebAssembly module, a Rust crate and an npm package.
See [Languages](languages/README.md).

A language can also be written in JavaScript, with `@diaryx/fig/kit`'s node
table, tree, scanner and grammar combinators; in Rust, by implementing
`fig::language::Language` (see
[Runtime languages](docs/rust.md#runtime-languages)); or as any program that
speaks the [helper protocol](docs/helper-protocol.md), JSON-RPC over stdio.
Name a helper in a `~/.config/fig/languages.figl` and every `fig` action accepts its
files, and `fig lang check` holds it to a built-in twin.

## Why `fig`?

`fig` was originally created for [Diaryx](https://diaryx.org),
a project whose goal is preservation and readability. Existing tools
read one file in and write an often very different new file out,
because they prioritize simply getting information from one place to another
rather than preserving the file as it is.

`toml_edit` accomplishes this goal, but only for the TOML format.
`fig` generalizes its structure to an arbitrary number of formats.
In this way, it is a bit like `serde` but with editing support as well---`serdedit`, if you will.

fig ensures fidelity with lots of testing and daily fuzzing.
The whole conformance suite is found at <https://github.com/diaryx-org/fig-conformance>.

## Possible future features

- Styling directives that carry across formats (a TOML inline table as a
  YAML flow mapping)
- Structure-aware diff and three-way merge (`fig patch` is the one-way half)
- `fig fmt`: sort keys, dedupe

## Fine print

**Contributions** are welcome, subject to my approval.

**License:** MIT or Apache 2.0, at your option. If you use `fig` in your
work, I would love to hear from you and feature you here.

**Credits:** I am thankful for the `toml_edit` Rust crate, which guided the
structural edits a format-preserving TOML editor needs.

## Contact me

<adam@diaryx.org>, or open an issue.
