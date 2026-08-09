```fig
title = fig
version = 2.5.3
author = adammharris
created = 2026-05-08
updated = 2026-08-08T21:07:09-06:00
contents = [[fig docs](docs/docs.md)]
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

`fig` is a Zig library (and CLI) for parsing and editing config files.

Editing config files programmatically shouldn't be a hassle.
Take a messy, real-world config:

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

And easily edit and comment from the command-line:

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

`fig` produces a single-line diff.
Every other byte is untouched.

`fig` supports lots of formats.
Converting to another format shouldn't mean sacrificing your comments:

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

`fig`can also edit (and convert) config *embedded in* other files:

```bash
$ fig set post.md tags --seq notes zig
$ fig convert post.md --to-embed frontmatter-toml --diff
--- post.md
+++ post.md
@@ -1,7 +1,7 @@
----
-title: Hello
-tags: [notes, zig]   # taxonomy
----
++++
+title = "Hello"
+tags = ["notes", "zig"] # taxonomy
++++
 
 # Hello
```

Originally made for [Diaryx](https://diaryx.org),
`fig` was made to edit frontmatter in markdown files without reserializing.
`fig` has since been expanded to include many different kinds of configuration formats:

- YAML (1.2.2 and 1.1)
- JSON (strict, JSONC, JSON5)
- TOML (1.1 and 1.2)
- ZON (Zig Object Notation, via `std.zig.AST`)
- NestedText (<https://nestedtext.org>)
- Java (`.properties`)
- dotenv (`.dotenv`)
- INI (`.ini`)
- Property list (`.plist`)
- Fig (`.figl`), an in-house authoring dialect authored by yours truly!

And has bindings in the following programming languages:

- [Zig](docs/zig.md)
- [Rust](docs/rust.md)
- [Typescript](docs/typescript.md) (experimental)
- C (not tested, but likely works)

## Command-line interface

Download from Github Releases or with Homebrew:

```bash
brew tap diaryx-org/tap
brew install diaryx-org/tap/fig
```

Or run it with no install at all (needs Node 20+)—see
[docs/npm-wasi.md](docs/npm-wasi.md)):

```bash
npx @diaryx/fig-wasi get config.yaml
```

Run `fig help` for instructions for how to use it on your files.

## Planned features

- Styling directives (maintain styling across formats, such as mapping TOML inline->YAML inline)
- More distribution options (depends on user need)
- More bindings (depends on user need)
- Advanced querying capabilties.
  - Filtering nodes
  - Redacting node
  - Multi-match `fig get`
- Structure-aware diff & 3-way merge.
- `fig patch` (config overlays / patch apply, lossless)
- `fig fmt` enhancements (sort keys, dedupe, stable array, etc.)

**In consideration**
- Schema validation (see [fig-schema](https://github.com/diaryx-org/fig-schema))
- LSP enhancements
- $ENV interpolation

## Fine print

**Contibutions**

Contributions are welcome, subject to my approval.

**AI Use**

`fig`, like many deceptively simple systems-level codebases,
require careful thought and intention.
AI tools can generate code rapidly,
often at the cost of this important design thinking.
Therefore, I have chosen to limit the use of AI code generation in this codebase.

I started writing this library by hand (no AI) for my own education,
and for use in my larger project, [Diaryx](https://diaryx.org).
After writing a JSON tokenizer and parser by hand,
and designing the Document, Token, and Language abstractions,
I decided to make use of the Codex AI tool
to generate specific portions of the code that would otherwise require hours of tedious, repetitive work.

Using Codex, I was able to make a compliant YAML parser
much faster than I would have been able to otherwise.
Later, I used Claude Code to do the same for TOML, ZON, and JSON5.
For each of these, I made a conformance suite in order to ensure a correct implementation.

All of the code generated was carefully reviewed and edited according to my taste before being accepted.
I take full responsibility and ownership of the code in this repository.
If you have any questions or concerns about AI use in this project, [please contact me!](<#Contact Me>)

**License**

MIT or Apache 2.0, at your discretion.
If you use `fig` in your work, I would love to hear from you and feature you here!
[Please contact me!](<#Contact Me>)

**Credits**

I took the JSON test suite at `testdata/json` from [Nicolas Seriot's JSONTestSuite repository](https://github.com/nst/JSONTestSuite).
I'm grateful that it is licensed under the MIT license, so I am allowed to use it for `fig`.
A copy of the license is included in this repository at `testdata/json/LICENSE`.

I've done likewise for the other testing suites:

- JSON5 <https://github.com/json5/json5-tests>
- TOML <https://github.com/toml-lang/toml-test>
- YAML <https://github.com/yaml/yaml-test-suite>
- NestedText [KenKundert/nestedtext_tests](https://github.com/KenKundert/nestedtext_tests)

I am thankful for each of them.

I am also thankful for the `toml-edit` Rust crate,
which provided guidance for the complex structural edits required by any format-preserving TOML editor.

## Contact Me

<amh421@icloud.com>, or leave an issue.
