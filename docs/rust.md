```fig
title = Using fig in Rust
author = adammharris
created = 2026-07-05T21:35:14-06:00
updated = 2026-10-12T12:00:00-06:00
part_of = [docs](docs.md)
```

# `fig` for Rust

`fig` parses, **edits**, and serializes configuration files — JSON, JSONC, JSON5,
YAML, TOML, ZON, and the native `figl` dialect — from one small crate. Its
distinguishing feature is *comment-preserving editing*: you can change one value
deep in a YAML or TOML file and every comment, blank line, key order, and quoting
style elsewhere stays byte-for-byte identical. It also converts losslessly
between formats.

The core is a Zig library statically linked into your crate — as a prebuilt
archive, so it needs no Zig toolchain — and it carries no language: each
format is a crate of its own (`fig-yaml`, `fig-toml`, …) that links its
language's library beside the core's, and you register the ones you use.
It carries an optional `serde` layer, or you can use its own
`serde`-independent `Value` tree and `#[derive(ToValue, FromValue)]` macros —
`fig` can replace `serde` in your project entirely. Diaryx uses `fig` in
production as a `serde` replacement.

- [Install](#install)
- [Quick start](#quick-start)
- [Languages and features](#languages-and-features)
- [Formats](#formats)
- [Reading data](#reading-data)
- [The value tree](#the-value-tree)
- [Typed structs: serde or derive](#typed-structs-serde-or-derive)
- [Editing without reserializing](#editing-without-reserializing)
- [Config inside a host file](#config-inside-a-host-file)
- [Serialization options](#serialization-options)
- [Diagnostics & lossless conversion](#diagnostics--lossless-conversion)
- [Errors](#errors)
- [Managing resources](#managing-resources)
- [API reference](#api-reference)
- [Forward compatibility (`#[non_exhaustive]`)](#forward-compatibility-non_exhaustive)

## Install

```toml
[dependencies]
fig = "6"
fig-yaml = "6"   # one crate per language you read or write
fig-json = "6"
```

On a tier-1 target `fig-sys` links a prebuilt `libfig.a` shipped by a
per-target payload crate — Cargo downloads exactly the one matching your
target, and **no Zig toolchain is required** — unless the target has no
payload crate or `FIG_SYS_FORCE_SOURCE=1` is set. The core carries no
language, so there is one build of it. A language crate compiles its
library from fig's Zig source, which for now needs Zig 0.17+.

Either way there is no separate native library to ship: the core and each
language are linked straight into your binary.

## Quick start

```rust
use fig::{Document, Format};

fn main() -> Result<(), fig::Error> {
    // Register each language you use; each hands back its format. A
    // dialect is found by name after (`Format::by_name("json5")`).
    let json = fig_json::register()?;
    let yaml = fig_yaml::register()?;

    // `Document::serialize` is the cross-format primitive: it preserves comments
    // where the target allows and collapses YAML's reference layer on the way out.
    let doc = Document::parse(br#"{"name":"fig","nums":[1,2]}"#, json)?;
    println!("{}", doc.serialize(yaml)?);
    // name: fig
    // nums: [1, 2]

    // Or read the whole document into an owned Value tree.
    let value = doc.to_value()?;
    println!("{value:?}");
    Ok(())
}
```

With the optional `serde` feature (see [Languages and features](#languages-and-features))
you also get `serde_json`-style helpers:

```rust
let toml = fig_toml::register()?;
let json = fig_json::register()?;

// Parse straight into a typed value, in any format…
let cfg: MyConfig = fig::from_slice(input, toml)?;
// …or YAML text, with `fig-yaml`'s `serde` feature.
let cfg: MyConfig = fig_yaml::from_str(yaml)?;

// Serialize any `Serialize` type: to any format through a `Value`…
let json = fig::to_value(&cfg)?.serialize(json)?;
// …or straight to YAML.
let yaml = fig_yaml::to_string(&cfg)?;
```

## Languages and features

Each language is a crate of its own, which links its library beside the
core's and registers it with `register()`, returning the format of its
first dialect; registering one again returns the format it already has.
A language you don't depend on costs nothing.

| Crate            | Registers |
| ---------------- | --------- |
| `fig-json`       | `json`, `jsonc`, `json5` |
| `fig-yaml`       | `yaml`, `yaml-1.1`, `yaml-1.2` (`serde` feature: `from_str`, `to_string`) |
| `fig-toml`       | `toml`, `toml-1.0`, `toml-1.1` |
| `fig-zon`        | `zon` |
| `fig-figl`       | `figl` |
| `fig-ini`        | `ini` |
| `fig-dotenv`     | `dotenv` |
| `fig-properties` | `properties` |
| `fig-plist`      | `plist` |
| `fig-nestedtext` | `nestedtext` |

The `fig` crate's features are the typed-mapping layers and the helper
wire, none of them on by default:

| Feature    | What it adds                                                              |
| ---------- | ------------------------------------------------------------------------- |
| `serde`    | `from_slice`/`to_value` and `serde` impls on `Value`.                     |
| `derive`   | `#[derive(fig::ToValue, fig::FromValue)]` — typed mapping without serde.   |
| `indexmap` | `ToValue`/`FromValue` for `IndexMap<String, T>` (insertion order kept).    |
| `helper`   | `fig::helper`, which serves a `Language` over stdio; adds `serde_json`.   |

A language nobody registered is not there: [`Format::by_name`] returns
`None` for its names. Once you hold a `Format`, [`capabilities`] says what
it can do:

```rust
use fig::capabilities;

let toml = fig_toml::register()?;
let caps = capabilities(toml);
// caps.read, caps.edit, caps.serialize, caps.references — all bools; the
// last says the format has a reference layer (anchors, aliases, tags —
// YAML's), which a conversion collapses when leaving it for a format without.
```

## Formats

A [`Format`] is an opaque, `Copy` handle naming a dialect registered with
the linked core. You get one from a language crate's `register()`, by name
once registered — `Format::by_name("toml")` — or from
[runtime registration](#runtime-languages) or `Format::detect`;
[`Format::name`] gives the name back. The languages fig ships, by the name
`by_name` takes:

| Name         | Parse | Edit | Serialize | Notes                                |
| ------------ | :---: | :--: | :-------: | ------------------------------------ |
| `json`       |  ✅   |  ✅  |    ✅     | Strict JSON (no comments).           |
| `jsonc`      |  ✅   |  ✅  |    ✅     | JSON with `//` and `/* */` comments. |
| `json5`      |  ✅   |  ✅  |    ✅     | Unquoted keys, trailing commas, etc. |
| `yaml`       |  ✅   |  ✅  |    ✅     | YAML 1.2.2 / 1.1.                    |
| `toml`       |  ✅   |  ✅  |    ✅     | TOML 1.0 / 1.1, incl. datetimes.     |
| `zon`        |  ✅   |  ✅  |    ✅     | Zig Object Notation. |
| `figl`       |  ✅   |  ✅  |    ✅     | The native `figl` authoring dialect. |
| `ini`        |  ✅   |  ✅  |    ✅     | `[section]` + `key = value`; untyped-string scalars, one level of sections. |
| `dotenv`     |  ✅   |  ✅  |    ✅     | Flat `KEY=value`; a flat string map.  |
| `properties` |  ✅   |  ✅  |    ✅     | Java `.properties`; same flat, untyped limits as dotenv. |
| `plist`      |  ✅   |  ✅  |    ✅     | Apple XML property list, typed and nested. |
| `nestedtext` |  ✅   |  ✅  |    ✅     | NestedText; nested but every leaf is a string. |

A language's versions are dialects of their own, each with its own handle:
`yaml-1.1`, `yaml-1.2`, `toml-1.0`, `toml-1.1`, and the full `yaml-1.2.2`-style
spellings.

Handles are assigned per process, in the order languages register, so a
`Format` is something to compare and pass around, never to persist: store
`format.name()` and resolve it again with `Format::by_name`. Being opaque, a
`Format` has no variants to match — branch on `name()` when you need to — and
a language the core gains later is a **minor** release.

```rust
use fig::Format;

let yaml = fig_yaml::register()?;
assert_eq!(yaml.name(), "yaml");
assert_ne!(Format::by_name("yaml-1.1"), Some(yaml));
assert_eq!(Format::by_name("no-such-language"), None);
```

### Runtime languages

The languages fig ships are not special: a language registered while the
program runs is a `Format` like any other. Implement
[`language::Language`](https://docs.rs/fig/latest/fig/language/trait.Language.html)
— a [`Description`](https://docs.rs/fig/latest/fig/language/struct.Description.html)
of what the format declares (the same declarations every bundled language makes:
capabilities, dialects, the `Syntax` the splice engine writes it with, and a
few sample documents), a `parse` that returns a
[`NodeTable`](https://docs.rs/fig/latest/fig/language/struct.NodeTable.html),
a `print` where the format serializes, and any fragment renderers the editor
needs — and hand it to
[`language::register`](https://docs.rs/fig/latest/fig/language/fn.register.html):

```rust
use fig::language::{Description, Language, LanguageError, NodeKind, NodeRow, NodeTable};
use fig::{Document, Format, Span};

struct Hcl;
impl Language for Hcl {
    fn describe(&self) -> Description {
        let mut d = Description::new("hcl");
        d.samples = vec!["a = 1\n".into()];
        d
    }
    fn parse(&self, _dialect: &str, input: &[u8]) -> Result<NodeTable, LanguageError> {
        let mut t = NodeTable::new();
        t.push(NodeRow::new(NodeKind::Mapping, None, Span { start: 0, end: input.len() }));
        // … one row per node, in pre-order
        Ok(t)
    }
}

let hcl = fig::language::register(Hcl)?[0];  // one Format per dialect; the first is the language's own
let doc = Document::parse(b"a = 1\n", hcl)?;   // a peer of the bundled languages at every call
assert_eq!(hcl.name(), "hcl");
assert_eq!(Format::by_name("hcl"), Some(hcl));
```

Every type in `fig::language` that grows with the core's contract is
`#[non_exhaustive]` — `Description`, `Dialect`, `Syntax`, `Comments`,
`CommentDelimiter`, `SectionHeader`, `ClosedContainers`, `NativeKinds`,
`Renderers`, `PrintOptions`, `LanguageError`, `RenderArgs`, the node table
and each of its rows — so a field the contract gains is a minor release.
Their fields stay public: build one from its constructor or `Default` and
assign what differs, rather than with a struct literal.

A sequence whose items are joined by something other than one per line or
the dialect's `flow_entry_sep` says so on its row:
`NodeRow::new(NodeKind::Sequence, parent, span).with_item_sep(" | ", true)`
names the bytes written between two items on one line, and `true` says it
has no delimiters (a `.tune` voice's bars run from after `Name:` to the end
of the line). Such a sequence takes every item edit — append, prepend,
`insert_item`, delete and move — whatever its first byte: a delete takes one
item and the separator beside it, and an append splices after the last item,
so a trailing separator stays trailing.

```rust
use fig::language::{Dialect, SectionHeader, Splice, Syntax};

let mut syntax = Syntax::default();
syntax.kv_sep = Some(" = ".into());
syntax.section_header = Some(SectionHeader::new("[", "]", "."));

let mut dialect = Dialect::new("hcl");
dialect.extensions = vec!["hcl".into(), "tf".into()];
dialect.splice = Splice::Raw;

// Rows: `NodeRow::new` and its `with_*` setters, and a `new` for each
// side-table row — `RegionRow::new(node, span)`, `MentionRow::new(node,
// span, kind)`, `CommentRow::new(node, slot, style, text)`,
// `DirectiveRow::new(handle, prefix)`.
```

The enums a language matches on — `NodeKind`, `Renderer`, `Literal`,
`CommentForm`, `Splice`, `MentionKind` — are non-exhaustive too, so a `match`
on one takes a `_` arm.
`CommentSlot` is the exception, on purpose: a printer must put every comment
somewhere, and a slot it had never heard of could only be dropped by a
wildcard, so a fourth anchor is a major release and a compile error.

`register` returns a `Vec<Format>`, one per dialect the description
declares, in declaration order. The core validates the description by the
rules it holds every language to and runs its harness over the samples —
each is parsed, printed, reparsed and edited — before anything is
registered; a description that fails, or a dialect whose name is already
registered (the bundled languages' names included), is refused as
[`Error::Language`], whose [`LanguageFailure`] carries the core's reason as
`message` (and a `byte_offset` where the failure names a place — the NUL in
a declared string that has one). A registered language lives for the rest
of the process, and its `Format` is per process: persist the name and
resolve it with `Format::by_name`.

The same `Language` can be served to the `fig` command line as a helper
process — JSON over stdin and stdout — with [`helper::serve`], which is what
a helper crate's `main` is. It comes with the `helper` feature, and reads
and writes the wire with `serde_json`, so a helper registers no language
for it; the module's functions trade `serde_json::Value`:

```rust,ignore
fn main() -> std::io::Result<()> {
    fig::helper::serve(Hcl)
}
```

The CLI spawns it from a `languages.figl` — `name`, the `extensions` it
owns, and the `command` to run — and it is then a format every action
accepts, by extension or by `--lang <name>`; `fig lang list` shows what
loaded and `fig lang check <name> --against <format>` holds it to a
compiled twin. A helper may take the name of a language the build leaves
out, and is then reached by that name and its extensions. `examples/tinykv_helper.rs` is a complete helper, and
`tools/cli-lang-check.sh` in the core drives the CLI through it. The
protocol, JSON-RPC 2.0 by the line, is specified in
[the helper protocol](helper-protocol.md); see
`docs/proposals/runtime-languages.md` in the core for the design.

## Reading data

For a typed result, reach for the [serde or derive](#typed-structs-serde-or-derive)
paths below. For dynamic structural access, parse a [`Document`] and read it into
an owned [`Value`] tree:

```rust
use fig::{Document, Format};

let toml = Format::by_name("toml").ok_or(fig::Error::UnsupportedFormat)?;
let doc = Document::parse(b"[server]\nhost = \"localhost\"\nports = [80, 443]\n", toml)?;
let value = doc.to_value()?; // the whole document as a Value
```

`to_value` and the serde paths read through YAML's references: an alias (`*x`)
reads as the value of the node its anchor names, and a `<<` merge key is
flattened into its mapping, the mapping's own keys winning. An alias inside the
node it names has no value, and is `Error::Parse` located at the alias.

[`Document::serialize`] is the cross-format conversion primitive — unlike
`to_value()?.serialize()`, it preserves comments carried on the source where the
target allows and collapses YAML's reference layer on the way out:

```rust
let json = doc.serialize(Format::by_name("json").ok_or(fig::Error::UnsupportedFormat)?)?;
// TOML in, JSON out, comments kept
```

A lower-level node API backs `to_value` (root/first-child/next-sibling walking),
but it is crate-internal; `to_value` and the typed paths cover reading from
application code.

## The value tree

[`Value`] is an owned, format-independent tree mirroring fig's AST:

```rust
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Str(String),
    Extended { kind: ExtKind, text: String }, // format-specific scalar
    Seq(Vec<Value>),
    Map(Vec<(Value, Value)>),                  // ordered entries
}
```

A few things to know:

- **Integers** split into `Int(i64)` and `Uint(u64)` so the full unsigned range
  round-trips; reading widens `i64` → `u64` → `Float` as needed. The split has a
  canonical side: a value that fits in `i64` is always `Int`, whatever Rust type
  it was built from, so `Value::from(3u64) == Value::from(3i64)`. `Uint` holds
  only magnitudes past `i64::MAX`.
- **Equality is structural**, which leaves two edges worth knowing: a
  hand-built `Uint(3)` is not `==` to `Int(3)` (nothing in the crate builds
  one), and `Float(f64::NAN)` is not equal to itself — `.nan` is a scalar fig
  parses *and* writes, so a dirty check spelled `if new != old { … }` never
  converges on one. [`Value::eq_canonical`] is the comparison that does:
  integers compare numerically across the two variants and floats compare by bit
  pattern (which also makes `0.0` and `-0.0` differ, as their text does).
- **Maps are ordered `Vec`s of pairs**, not hash maps — key order is preserved,
  and a key can be any `Value` (non-string keys serialize only to formats whose
  printer accepts them).
- **Format-specific scalars** — TOML datetimes, ZON enum/char literals, JSON5
  `Infinity`/`NaN` — read into `Value::Extended { kind, text }` (see [`ExtKind`])
  and serialize back verbatim instead of degrading to strings.

`From` conversions make literals ergonomic (`bool`, `i32`/`i64`, `u64`, `f64`,
`&str`, `String`, `Vec<Value>`), and [`Value::serialize`] renders through fig's
core — no formatting happens in Rust:

```rust
use fig::{Value, Format};

let value = Value::Map(vec![
    ("name".into(), "fig".into()),
    ("nums".into(), Value::Seq(vec![1i64.into(), 2i64.into()])),
]);

let json = Format::by_name("json").ok_or(fig::Error::UnsupportedFormat)?;
value.serialize(json)?; // {\n  "name": "fig",\n  "nums": [\n    1,\n    2\n  ]\n}\n
```

The reverse direction — scalar *text* back into a `Value` — is
[`Value::parse_number`], the same mapping `Document::to_value` uses, plus
[`Value::parse_float`] for the `f64` half on its own. Reach for these rather than
`str::parse` when turning edited text back into a value: `str::parse::<f64>()`
rejects `.inf`/`.nan`, the exact spellings fig writes, so a field holding one
would silently become a string on an edit that changed nothing.

```rust
use fig::Value;

Value::parse_number("3", false)?;    // Int(3)  — integer field
Value::parse_number("3", true)?;     // Float(3.0) — float field, same text
Value::parse_number(".inf", true)?;  // Float(f64::INFINITY), not Str(".inf")
```

## Typed structs: serde or derive

Two independent ways to map fig documents onto your own types — the `derive` path
needs no serde, and neither is more capable than the other.

**With the `serde` feature** — use the derive from `serde` and the
`fig::from_*`/`fig::to_*` helpers, exactly like `serde_json`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Config { name: String, port: u16 }

let toml = fig_toml::register()?;
let json = fig_json::register()?;

let cfg: Config = fig::from_slice(b"name = \"fig\"\nport = 8080\n", toml)?;
let json = fig::to_value(&cfg)?.serialize(json)?;         // to any format
let yaml = fig_yaml::to_string(&cfg)?;                     // to YAML (`fig-yaml`'s `serde` feature)
```

`from_slice` takes any registered [`Format`], and
`to_value(&x)?.serialize(format)` writes any of them. `fig_yaml::from_str`
and `fig_yaml::to_string` are the YAML shorthands, with `fig-yaml`'s
`serde` feature. `Value` itself implements
`Serialize`/`Deserialize`, so `from_slice::<Value>` gives you a dynamic tree
the way `serde_json::Value` does.

**Without serde** — enable `derive` instead and map straight onto the concrete
`Value` tree. The generated code is straight-line field extraction with no
format-generic machinery, so it stays small:

```rust
use fig::{FromValue, ToValue};

#[derive(ToValue, FromValue)]
struct Config { name: String, port: u16 }

let toml = fig::Format::by_name("toml").ok_or(fig::Error::UnsupportedFormat)?;
let value = fig::Document::parse(src, toml)?.to_value()?;
let cfg = Config::from_value(&value)?;
let back = cfg.to_value();          // -> Value, ready to serialize
```

The macros support the attributes you'd expect from serde:

- Field: `#[fig(rename = "..")]`, `#[fig(skip)]`, `#[fig(flatten)]`,
  `#[fig(default)]` / `#[fig(default = "path")]`,
  `#[fig(skip_serializing_if = "path")]`, `#[fig(alias = "..")]`,
  `#[fig(deserialize_with = "path")]`, and `Option<T>` fields (absent → `None`).
- Container: `#[fig(rename_all = "..")]` (`camelCase`, `snake_case`, …).
- Enums, in all four taggings: external (default), internal
  (`#[fig(tag = "type")]`), adjacent (`#[fig(tag = "type", content = "data")]`),
  and untagged (`#[fig(untagged)]`), across unit/newtype/tuple/struct variants.

## Editing without reserializing

This is what sets `fig` apart. [`Editor`] splices only the bytes of the node you
touch — everything else in the file is preserved exactly.

```rust
use fig::{Editor, Format, Segment};

let toml = Format::by_name("toml").ok_or(fig::Error::UnsupportedFormat)?;
let mut ed = Editor::open(
    b"# app config\nhost = \"localhost\"  # dev box\nport = 8080\n",
    toml,
)?;

ed.replace_value(&[Segment::Key("port")], 9090i64)?;
ed.set_value(&[Segment::Key("debug")], true)?; // replace if present, else insert

println!("{}", ed.source()?);
// # app config
// host = "localhost"  # dev box
// port = 9090
// debug = true
```

Edits are addressed by a **path** — a slice of [`Segment`], each a `Key(&str)`
(mapping key) or `Index(usize)` (sequence index). `Segment` has `From` impls, so
`"port".into()` and `0.into()` work; an empty path `&[]` is the document root.
The value methods take any `impl Into<Value>`, so scalars (`9i64`, `"x"`, `true`),
a built [`Value`], or a `&Value` all pass directly. Whatever you pass is rendered
in the document's own format automatically (a string becomes `"x"` for TOML/JSON
but a bare `x` for YAML).

Common operations:

```rust
ed.insert_value(&[], "key", &value)?;                 // add a mapping entry
ed.replace_value(path, &value)?;                      // change a value
ed.set_value(path, &value)?;                          // upsert (replace or insert)
ed.append_value(&[Segment::Key("list")], &value)?;    // push onto a sequence
ed.prepend_value(&[Segment::Key("list")], &value)?;
ed.insert_item(&[Segment::Key("list")], 2, &value)?;  // insert so it is item 2, before the item there now
ed.rename_key(path, "new_key")?;                      // rename a key (a name, spelled as the format spells a key)
ed.delete_key(path)?;                                 // remove a mapping entry
ed.delete_item(&[Segment::Key("list")], 0)?;          // remove a sequence item by index
ed.move_key(&["a".into()], &["b".into()])?;           // reorder mapping entries
ed.reorder_keys(&[], &["title", "body"])?;            // named keys first, rest follow
ed.move_item(&[Segment::Key("list")], 2, 0)?;         // reorder sequence items
ed.reorder_items(&[Segment::Key("list")], &[2, 0])?;  // bring these indices to the front
ed.set_sequence(&[Segment::Key("tags")], ["a", "b"])?; // reconcile a list, keeping survivors' comments
```

Every value edit has a `*_with` twin taking a [`SerializeOptions`], for when
the spliced value's own rendering needs controlling: `replace_value_with`,
`insert_value_with`, `set_value_with`, `append_value_with`,
`prepend_value_with`, `insert_item_with`. `set_sequence` takes anything iterable over
`impl Into<Value>` — a `&[Value]`, a `Vec<String>`, an array of `&str`.

`insert_item(path, index, value)` is the insert at an index: 0 is
`prepend_value`, the sequence's length (or `usize::MAX`) is `append_value`,
and an index past it is [`Error::NotFound`] with nothing written. Between
them, a flow item takes a separator laid out as its sequence is, and a block
item goes on its own line above the item at `index` and that item's comment
block.

The names follow one scheme. Value edits
are `<verb>_value`, except the insert at an index, `insert_item`, named by its
noun as `delete_item` is because `insert_value` inserts a key. Structural edits are `<verb>_<noun>`, the noun being what
the path names — a `key`, an `item` (by index), or a `container` (below) — so
`delete_key`/`delete_item`/`delete_container`, `move_key`/`move_item`/
`move_container`, `reorder_keys`/`reorder_items`/`reorder_containers`, and
`rename_key`/`rename_container`. Comments are named by their anchor, and each
`delete_<anchor>_comment` removes exactly what the matching read returns.

### Whole containers

The operations above address a container the same way they address a scalar:
by the one range of source it occupies. A TOML `[header]` table occupies no
such range — its body is the lines after the header, and `[a.b]` further down
the file extends it — and neither does an INI `[section]` or a `figl` block
container. At a path naming one, `delete_key`, `replace_value`, `move_key` and
`reorder_keys` all answer `Error::InvalidArgument` rather than rewrite the
header and leave the entries behind. These six are the route for those shapes:

```rust
ed.delete_container(&[Segment::Key("a")])?;                    // header + body, every region
ed.insert_container(&[Segment::Key("c")], "z = 3\n")?;         // a new [c] with these entries
ed.rename_container(&[Segment::Key("a")], "q")?;               // [a], [a.b] and [[a.c]] alike
ed.move_container(&[Segment::Key("a")], None)?;                // None = to EOF; Some(&[]) = the root
ed.reorder_containers(&["b", "a"])?;                           // top-level containers only, so no path
ed.append_container_to_seq(&[Segment::Key("bin")], "name = \"b\"\n")?;  // a new [[bin]]
```

`body` is verbatim entry lines in the document's format, spliced and reparsed
like any other edit, so a body that doesn't parse rolls the document back.

Support varies by format, and a format that lacks an operation answers
`Error::UnsupportedFormat`:

| | `toml` | `ini` | `figl` | others |
|---|---|---|---|---|
| `delete_container`, `move_container`, `reorder_containers` | ✓ | ✓ | ✓ | — |
| `insert_container`, `rename_container`, `append_container_to_seq` | ✓ | — | — | — |

"Others" is not a gap: YAML, JSON and the rest nest a container in one
contiguous region, so `delete_key` and `replace_value` already handle it — which is
why they succeed on a YAML block mapping where TOML's refuse.

`set_sequence` has a narrower domain than the rest: it matches new items to old
ones by *value* so a kept-or-merely-reordered item keeps its comments, which
means each item has to parse as a standalone document. It therefore declines —
with `Error::InvalidArgument` — on `toml` (whose scalars can't stand alone), on
an empty list on either side, and on any non-scalar item. Nothing is lost
there: a TOML inline array carries no per-element comments, so `replace_value`
on the whole list is equivalent. It earns its keep on `yaml` and `figl`, where
per-item comments are real.

Comments are first-class:

```rust
ed.add_leading_comment(&["port".into()], "the listening port")?; // own-line comment above
ed.set_trailing_comment(&["port".into()], "default 8080")?;      // same-line comment
ed.leading_comment(&["port".into()])?;   // read it back (Some("") = bare marker, None = none)
ed.trailing_comment(&["port".into()])?;  // same convention
ed.delete_trailing_comment(&["port".into()])?;
ed.delete_leading_comment(&["port".into()])?;    // drops the whole owned block, as read
```

The write verb says what the anchor holds: the leading block is a run of
lines, so `add_leading_comment` appends one to it; the trailing comment is a
single line, so `set_trailing_comment` replaces it.

The comment marker (`#`, `//`) is chosen for the format; strict `json` has no
comments and returns [`Error::UnsupportedFormat`] if you try.

A container has a third anchor — the **dangling** run at the end of its body,
after its last entry, which is where a commented-out *last* entry lives. It is
addressed by the container's own path (an empty path = the document root):

```rust
ed.add_dangling_comment(&["server".into()], "was: here")?; // at the body's child depth
ed.dangling_comment(&["server".into()])?;   // Some("") = bare marker, None = none
ed.delete_dangling_comment(&[])?;           // the run at the end of the document
```

A scalar has no body to end, and neither has a flow container written on one
line (`{ "a": 1 }`) — both give [`Error::InvalidArgument`]. A pretty-printed
JSONC object is fine: its `// note` before the closing brace is the root's
dangling run.

An entry can also be turned INTO a comment run and back, which is how a
structural editor shows a disabled row:

```rust
ed.comment_out(&["server".into(), "port".into()])?;
// server:
//   # port: 8080
//   host: local
ed.uncomment_leading(&["server".into(), "host".into()], 0, 1)?; // byte-identical again
```

`comment_out` prefixes every line of the node's span with the marker at that
line's own indentation, leaving the node's own leading block above it
untouched; afterwards the tree has no node at that path. The run is the leading
block of whatever followed it, or — when the entry was last — the parent's
dangling run, which `uncomment_dangling(container_path, first_line, line_count)`
addresses instead. Lines are taken by index within the block because *which*
lines are an entry is the caller's judgement; if the result does not parse, or
parses to a document whose other nodes moved, the splice is rolled back and the
call returns [`Error::Parse`] or [`Error::UnsupportedOperation`] with the
document byte-for-byte as it was.

Everything above uses the `*_value` methods, which take `impl Into<Value>` and
need no serde. Scalars, strings and bools pass straight through; a typed
struct goes in as a `Value`, through the `derive` feature's `to_value()` or,
**with the `serde` feature**, through `fig::to_value(&x)?`:

```rust
ed.set_value(&["debug".into()], true)?;
ed.append_value(&["tags".into()], "published")?;
ed.set_value(&["server".into()], server.to_value())?;         // server: #[derive(ToValue)]
ed.set_value(&["server".into()], fig::to_value(&server)?)?;   // server: #[derive(Serialize)]
```

## Config inside a host file

fig edits a config document, not the file it sits in. Frontmatter,
endmatter, and fenced or HTML blocks in Markdown are located by
[twig](https://github.com/diaryx-org/twig): open an [`Editor`] over the
block's contents and splice its `source()` back between the byte spans twig
reported.

## Serialization options

[`Value::serialize_with`], [`Document::serialize_with`], and the `diagnose`
methods take a [`SerializeOptions`]. The [`Default`] is fig's historical style
(pretty, two-space indent), and there are builder helpers:

```rust
use fig::{Format, SerializeOptions};

let json = Format::by_name("json").ok_or(fig::Error::UnsupportedFormat)?;
let yaml = Format::by_name("yaml").ok_or(fig::Error::UnsupportedFormat)?;
value.serialize_with(json, SerializeOptions::compact())?;      // minified
value.serialize_with(json, SerializeOptions::pretty(4))?;      // 4-space indent
value.serialize_with(yaml, SerializeOptions::default().width(120))?; // wider inline budget
doc.serialize_with(yaml, SerializeOptions::default().strip_comments())?;
```

| Field            | Applies to           | Meaning                                            |
| ---------------- | -------------------- | -------------------------------------------------- |
| `pretty`         | JSON, ZON, TOML      | Multi-line (default) vs. compact.                  |
| `indent`         | JSON, TOML           | Spaces per level (default 2).                      |
| `width`          | TOML, YAML, figl     | Column budget for inline (flow) vs. expanded layout. |
| `strip_comments` | all                  | Drop carried comments instead of emitting them.    |
| `lossless`       | `Document`           | Round-trip values the target can't natively hold.  |

`lossless` is honored only on [`Document`] (a built [`Value`] carries no source
envelopes to round-trip).

## Diagnostics & lossless conversion

Converting between formats can lose information — TOML has no `null`, JSON has no
datetimes or comments. `diagnose` tells you exactly what *would* be lost, without
doing it, returning one [`Warning`] per lossy event:

```rust
use fig::{Document, Format, SerializeOptions, WarningCode};

let yaml = Format::by_name("yaml").ok_or(fig::Error::UnsupportedFormat)?;
let toml = Format::by_name("toml").ok_or(fig::Error::UnsupportedFormat)?;
let doc = Document::parse(b"a: null\nb: 1 # keep\n", yaml)?;

// TOML has no null, so `a` would be dropped.
let warns = doc.diagnose(toml, SerializeOptions::default())?;
assert_eq!(warns[0].code, WarningCode::ValueDropped);
assert_eq!(warns[0].path, "a");
```

Each [`Warning`] carries a [`WarningCode`] (`CommentDropped`,
`CommentStyleDegraded`, `ValueDropped`, `TypeDegraded`), a [`WarningCause`]
(`FormatLimitation` or `ExplicitOption`), the node `path`, and a `note` (e.g. the
degraded-to type). Both [`Document`] and [`Value`] have `diagnose`.

To *preserve* those values instead, serialize with `lossless` — unrepresentable
values round-trip through a `$fig` envelope, and `diagnose` then reports nothing
lost:

```rust
let out = doc.serialize_with(toml, SerializeOptions::default().lossless())?;
```

## Errors

Fallible calls return `Result<_, fig::Error>`. [`Error`] is a plain enum
implementing `std::error::Error` (and, with `serde`, `serde::de/ser::Error`):

```rust
use fig::{Document, Format, Error};

let json = Format::by_name("json").ok_or(Error::UnsupportedFormat)?;
match Document::parse(b"{ not valid", json) {
    Ok(doc) => { /* … */ }
    Err(Error::Parse(detail)) => {
        eprintln!("{}", detail.message);          // the language's diagnostic
        eprintln!("{:?}", detail.byte_offset);    // Some(2): where it reports one
    }
    Err(e) => eprintln!("{e}"),
}
```

Notable variants: `Parse(ParseError)`, `UnsupportedFormat` (a format this
build has not registered, or one that cannot do what was asked), `NotFound` (a path
or key), `InvalidArgument`, `UnsupportedOperation` (every argument valid,
but the operation is not defined for them), `Utf8`, `Language(LanguageFailure)`
(a runtime language refused at registration), plus serde/derive mapping errors
(`Message`, `MissingField`, `UnknownVariant`, `TypeMismatch`, …). The struct
variants are `#[non_exhaustive]`, so a pattern on one ends in `..`
(`Error::MissingField { field, .. }`), and each is built through its
constructor (`Error::missing_field(field, ty)`), as the derive macros do.

`NotFound` carries no path: the core reports that something on the way was
missing but not which segment, so the only path the binding could attach is the
one you just passed.

[`ParseError`] carries the message the language's parser gave, and the
`byte_offset` it reported where it reports one — the bundled JSON does, a
runtime language does when its parser says where. `line` and `column` are
`None` unless the core fills them, and an offset of 0 reads as `None` too.

## Managing resources

Unlike the TypeScript binding, there is **no manual cleanup**. [`Document`]
and [`Editor`] each own a native handle freed by their `Drop` impl, so
they release deterministically when they go out of scope — normal Rust RAII:

```rust
{
    let mut ed = Editor::open(src, yaml)?;
    // …edit…
    let out = ed.source()?.to_owned();
    out
    // handle freed here when `ed` drops
}
```

The one thing to watch is **borrows**: `Editor::source` and the
comment reads return `&str` (or `Option<String>`) that borrow handle memory and
are invalidated by the next mutation. Copy out with `.to_owned()` if you need the
text to outlive the next edit — the borrow checker enforces this for you.

## API reference

**Top-level functions**

- `capabilities(format) -> Capabilities` — what this build can read/edit/serialize.
- `version() -> Version` / `version_string() -> &'static str` — linked core version.
- *(serde)* `from_slice<T>(bytes, format) -> Result<T>` — deserialize any format.
- *(serde)* `to_value<T>(&value) -> Result<Value>` — build a `Value` from any `Serialize`.
- `language::register_library(fig_<name>_register)` — what each language crate's `register()` is.
- *(fig-yaml, serde)* `fig_yaml::from_str<T>(s)` / `fig_yaml::to_string(&value)` — YAML text.

**Types**

- [`Document`] — read path: `parse`, `to_value`, `serialize`/`serialize_with`, `diagnose`.
- [`Editor`] — comment-preserving editor: `open`, `source`, and the edit/comment methods.
- [`Value`] — the owned value tree; `serialize`/`serialize_with`/`diagnose`, plus `From` impls.
- [`Format`] — an opaque, `Copy` handle to a registered dialect: `by_name`, `name`, `detect`, `split_stream`.
- `Segment<'a>` — path step (`Key(&str)` / `Index(usize)`), with `From<&str>`/`From<usize>`.
- `SerializeOptions` — output style (`compact()`, `pretty(n)`, `.indent(n)`, `.width(n)`, `.strip_comments()`, `.lossless()`).
- `Span` — a `[start, end)` byte range; `Hash`, and `From`/`Into` `Range<usize>`.
- `Warning` / `Version` / `Capabilities` / `ParseError` / `LanguageFailure`.
- `fig::language` — `Language`, `register`, `Description` and its parts, `NodeTable` and its rows, `RenderArgs`, `PrintOptions`, `LanguageError`; see [Runtime languages](#runtime-languages).

**Enums**

- `ExtKind`, `WarningCode`, `WarningCause` — plain data enums.
- `Error` — the returned error type.

**Traits** *(the `derive` feature)*

- `ToValue` / `FromValue` — typed mapping to/from `Value`, with
  `#[derive(fig::ToValue, fig::FromValue)]` and `#[fig(...)]` attributes.

## Forward compatibility (`#[non_exhaustive]`)

The public types that grow with the core are `#[non_exhaustive]`: fig adds
formats, statuses, diagnostics and runtime-language declarations regularly, and
marking these means such an addition is a **minor** release instead of a major
one. (A new format needs no marking: [`Format`] is an opaque handle, not an
enum, so a language the core gains is just another name `by_name` finds.)
They are `ExtKind`, `Error` and its struct variants,
`WarningCode`, `WarningCause`, `SerializeOptions`, `Version`, `Capabilities`,
`Warning`, `ParseError`, `LanguageFailure`, and in `fig::language`
`Description`, `Dialect`, `Syntax`, `Comments`, `CommentDelimiter`,
`SectionHeader`, `ClosedContainers`, `NativeKinds`, `Renderers`, `PrintOptions`,
`LanguageError`, `RenderArgs`, `NodeTable`, `NodeRow`, `RegionRow`,
`MentionRow`, `CommentRow`, `DirectiveRow`, and the enums `NodeKind`, `Renderer`,
`Literal`, `CommentForm`, `Splice`,
`MentionKind`.

What it asks of you:

- **Match with a wildcard.** `match code { WarningCode::ValueDropped => …, _ => … }`.
  Constructing a variant is unaffected — only exhaustive matching needs the `_`.
  A `Format` is matched by name: `match format.name() { "yaml" => …, _ => … }`.
- **Match a struct variant with `..`**: `Error::MissingField { field, .. }`.
- **Build from a constructor, then set or assign**, not a struct literal:
  `SerializeOptions::default().width(1).strip_comments()` (a chainable setter
  per field); `Capabilities::new(true, true, false).with_references(true)`;
  `RenderArgs::default().value(b"42")`; and for the rest of `fig::language`, a
  `new` or `Default` followed by plain field assignment
  (`let mut s = Syntax::default(); s.kv_sep = Some("=".into());`). Reading
  fields is unchanged everywhere.
- **Returned structs are read-only to you.** `Version`, `Warning`, `ParseError`
  and `LanguageFailure` come from the library; their fields stay public
  and readable, but there is no constructor, because nothing takes one back.

[`Value`], `Segment`, `Span` and `language::CommentSlot` are deliberately
**exhaustive**. `Value` and `Segment` are the data model, and matching a `Value`
without a wildcard is the normal way to consume it; `Span` is a plain pair built
by literal everywhere; and a printer that met an unknown `CommentSlot` through a
wildcard could only drop the comment. Adding to any of them would be a real
breaking change, and is treated as one.

