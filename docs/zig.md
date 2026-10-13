```fig
title = Using fig in Zig
author = adammharris
created = 2026-07-05T21:35:14-06:00
updated = 2026-10-12T16:30:00-06:00
part_of = [docs](docs.md)
```

# fig for Zig

`fig` is a Zig library that parses, **edits**, and serializes configuration
files — JSON, JSONC, JSON5, YAML, TOML, ZON, INI, dotenv, Java `.properties`,
NestedText, Apple XML property lists, and figl, fig's own authoring
dialect — from one small package with no external
dependencies. Its distinguishing feature is *comment-preserving editing*: you
can change one value deep in a YAML or TOML file and every comment, blank
line, key order, and quoting style elsewhere stays byte-for-byte identical. It
also converts losslessly between formats.

This is the core library — the same code the C ABI, the TypeScript/WASM
package, and the `fig` CLI are all built on. Used directly from Zig you get
the whole surface: build-time language selection, zero-copy reads over the
source buffer, and full control over allocation (there is no hidden global
state or GC — you manage every `deinit`).

- [Install](#install)
- [Quick start](#quick-start)
- [Formats](#formats)
- [Reading data](#reading-data)
- [The value tree](#the-value-tree)
- [Editing without reserializing](#editing-without-reserializing)
- [Config inside a host file](#config-inside-a-host-file)
- [Serialization options](#serialization-options)
- [Diagnostics & lossless conversion](#diagnostics--lossless-conversion)
- [Reflective deserialization](#reflective-deserialization)
- [Errors](#errors)
- [Managing resources](#managing-resources)
- [API reference](#api-reference)

## Install

```sh
zig fetch --save https://github.com/diaryx-org/fig
```

Then wire it into `build.zig`:

```zig
const fig_dep = b.dependency("fig", .{ .target = target, .optimize = optimize });
exe.root_module.addImport("fig", fig_dep.module("fig"));
```

fig's core names no language: every format is a *language* that registers
with it. The package ships one per format, and a build option per language
says whether this build bundles it, so a consumer that only needs JSON+YAML
can trim the rest out of the binary by passing options through to the
dependency:

```zig
const fig_dep = b.dependency("fig", .{
    .target = target,
    .optimize = optimize,
    .toml = false,  // drop TOML support
    .zon = false,   // drop ZON support
    .plist = true,  // plist is opt-in; enable it if you need it
});
```

| Option        | Default | Includes                                    |
| ------------- | :-----: | ------------------------------------------- |
| `json`        |   ✅    | The shared JSON/JSONC/JSON5 core.           |
| `yaml`        |   ✅    | YAML 1.2.2 / 1.1.                           |
| `toml`        |   ✅    | TOML 1.0 / 1.1.                             |
| `zon`         |   ✅    | Zig Object Notation.                        |
| `figl`        |   ✅    | figl, fig's authoring dialect.              |
| `ini`         |   ✅    | INI (`[section]` + `key = value`).          |
| `dotenv`      |   ✅    | dotenv / `.env`.                            |
| `properties`  |   ✅    | Java `.properties`.                         |
| `nestedtext`  |   ✅    | NestedText (nestedtext.org).                |
| `plist`       |         | Apple XML property lists.                   |
| `canonical`   |         | The AST's own oracle encoding (mostly for tests; always compiled for a test build). |

The languages a build bundles register the first time anything looks a
format up (`fig.bundle` names them, and `fig.bundle.registerAll(allocator)`
registers them up front, in an allocator of your choosing — the one any
`Runtime.register` you make later must use too).

## Quick start

```zig
const std = @import("std");
const fig = @import("fig");

pub fn main() !void {
    var gpa: std.heap.DebugAllocator(.{}) = .init;
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    // A format is a registered dialect, found by name.
    const json = fig.Runtime.entryByName("json").?;
    const yaml = fig.Serialize.Target.named("yaml").?;

    // Parse JSON into a Document (the AST plus source spans).
    const doc = try fig.Runtime.Language.Parser.parse(allocator, "{\"name\":\"fig\",\"nums\":[1,2]}", json.typeOf());
    defer doc.deinit(allocator);

    // Convert to YAML.
    var out: std.Io.Writer.Allocating = .init(allocator);
    defer out.deinit();
    try fig.Serialize.serialize(&doc.ast, &out.writer, yaml);
    std.debug.print("{s}", .{out.written()}); // name: fig\nnums: [1, 2]\n
}
```

Every format is read, edited and printed through the one
`fig.Runtime.Language`, whose `Type` names a registered dialect
(`entry.typeOf()`), so swapping `"json"` for `"toml"` is the whole adaptation
needed to target a different format.

## Formats

| Dialect      | Parse | Edit | Serialize | Notes                                    |
| ------------ | :---: | :--: | :-------: | ----------------------------------------- |
| `json`, `jsonc`, `json5` |  ✅   |  ✅  |    ✅     | One language, three dialects.  |
| `yaml`       |  ✅   |  ✅  |    ✅     | YAML 1.2.2; `yaml-1.1` for 1.1. Anchors/aliases. |
| `toml`       |  ✅   |  ✅  |    ✅     | TOML 1.1; `toml-1.0` for 1.0. Datetimes. |
| `zon`        |  ✅   |  ✅  |    ✅     | Zig Object Notation.                      |
| `figl`       |  ✅   |  ✅  |    ✅     | fig's authoring dialect.                  |
| `ini`        |  ✅   |  ✅  |    ✅     | Untyped scalars — `port = 8080` reads back as the *string* `"8080"`. |
| `dotenv`     |  ✅   |  ✅  |    ✅     | Flat string map: no nesting, untyped scalars. |
| `properties` |  ✅   |  ✅  |    ✅     | Java `.properties`; same flat, untyped limits as dotenv. |
| `plist`      |  ✅   |  ✅  |    ✅     | Opt-in (`-Dplist`). Typed *and* nested; date/data ride on `extended`. |
| `nestedtext` |  ✅   |  ✅  |    ✅     | Nested (dict/list) but deliberately untyped — every leaf is a string. |

A language's `--spec` versions are dialects of their own, named
`<dialect>-<version>`: `yaml-1.1`, `yaml-1.1.0`, `yaml-1.2`, `yaml-1.2.2`,
`toml-1.0`, `toml-1.0.0`, `toml-1.1`, `toml-1.1.0`.

The four untyped formats (INI, dotenv, `.properties`, NestedText) parse every
scalar as a string; nothing infers a number or a bool from them. INI, dotenv
and `.properties` are also shape-limited — see
[`FlatStrip`](#diagnostics--lossless-conversion) for what a serialize to one
of them drops.

Don't hard-code this table into your program: a language this build does not
bundle is not registered, and `entryByName` returns null for it.
`fig.Runtime.detectAmong(allocator, input, order)` sniffs content in the
order you give it — the first dialect of `order` whose parser accepts
`input` — since the grammars overlap and only the caller knows what
ambiguous input should mean. The CLI's order is strictest first: JSON,
JSON5, ZON, plist, TOML, figl, INI, dotenv, YAML, `.properties`, NestedText.

### Languages

A language is a contract: its declarations as fields of a
`fig.Runtime.VTable` — name, caps, dialect rows (with extensions, edit-text
style, empty-document seed and what its printer holds),
syntax, samples — `parse` and `print` as function pointers, and the six
renderers as pointers that may be null. `fig.Runtime.register(allocator,
&vtable)` validates the record, runs the checks of `src/contract/harness.zig`
over the samples it declares (parse, print, reparse to the same tree, an
`Editor` over each), and only then publishes it, handing back the handle of
its first dialect; a record that fails either is refused with the reason in
`Runtime.lastRefusal()` and registers nothing.

```zig
const handle = try fig.Runtime.register(allocator, &vtable);
const entry = fig.Runtime.entryByAbi(handle).?;
const doc = try fig.Runtime.Language.Parser.parse(allocator, source, entry.typeOf());
var ed: fig.Editor(fig.Runtime.Language) = .{ .allocator = allocator, .format = entry.typeOf() };
```

`parse` fills, and `print` reads, a node table (`Runtime.table`): fig
owns it, and `parse` writes into one fig allocated — `addRows`,
`addString`, and a list per side fact — while `print` is handed a `View`.
A row is 32 bytes: kind, flags, parent, span and text, in pre-order (row
index is node id), and anchors, tags, item markers, entry separators,
item separators, regions, mentions, comments, tag directives and
diagnostics are sparse side facts, each naming its row. The shapes are
`extern struct`s, stated in `fig.h` as `FigLanguageVTable`, `FigRow`,
`FigTableView` and the `fig_table_*` calls a C language fills its table
through, and registered from C through `fig_language_register`;
`tools/abi_runtime_probe.c` is the smallest complete host, written in C,
and `zig build abi-check` runs it. What a language cannot do is add a node
kind or receive the editor: a renderer returns text. See
`docs/proposals/runtime-languages.md` and `docs/proposals/core-only.md`.

fig's own languages are Zig packages (`languages/<name>/`), each
depending on the kit (`kit/`) and on nothing else of fig's, and
`kit/src/contract.zig`'s `VTable(L, prefix)` builds the vtable from one.
`fig.Language.validate(comptime L)` holds such a module to the contract at
comptime — the declarations it must and may make, and their coherence — and
`zig build validate-check` proves the claims rather than describing them:
one of its cases declares a `Language` in a temporary directory, hands it to
`Editor`, and runs a `set` and a `deleteKey` through it. There are no
editing hooks: what a language tells the editor is its `syntax` (the
declared half), what its parser records on `Document` beside the spans —
each block-sequence item's marker, each entry's separator, a section's
header lines and every place its name is written — and, where it spells a
fragment in a way no declaration covers, a *renderer*: `renderValue`,
`renderEntry`, `renderItem`, `renderTail`, `renderKey` or `renderKeyName`,
each a pure function from the dialect and a `RenderRequest` to a string
that the engine splices under the same reparse net as any other edit. The
renderers are documented on `Decls.renderers` in
`src/contract/language.zig` and on the `Editor` methods that call them.

Every "Edit" ✅ above goes through the *same* `Editor(Runtime.Language)`
engine (next section) — ZON included, splicing its `.key = value`
struct-field syntax and `.{}`/`.@"..."` quoting rules exactly like JSON gets
`"key": value`. TOML, figl and INI, whose containers are scattered, also
answer the whole-container operations listed near the end of that section.

## Reading data

`Runtime.Language.Parser.parse` gives you a `Document`: the source and the
parsed `AST`, which is the node table the language filled — one row per
node, with its span — adopted as it stands. `source` is *borrowed* — keep it
alive as long as the `Document` is:

```zig
const toml = fig.Runtime.entryByName("toml").?;
const doc = try fig.Runtime.Language.Parser.parse(
    allocator,
    "[server]\nhost = \"localhost\"\nports = [80, 443]\n",
    toml.typeOf(),
);
defer doc.deinit(allocator);

// Navigate by path — an array of `.key` (mapping) / `.index` (sequence) segments.
const host = try doc.ast.getValByPath(&.{ .{ .key = "server" }, .{ .key = "host" } });
host.kind.string; // "localhost"

const port1 = try doc.ast.getValByPath(&.{ .{ .key = "server" }, .{ .key = "ports" }, .{ .index = 1 } });
port1.kind.number.raw; // "443"

_ = doc.ast.getValByPath(&.{.{ .key = "missing" }}); // error.NotFound
```

`getValByPath`/`getKeyByPath`/`getNodeByPath` (`kit/ast/reader.zig`) are the
high-level accessors; `getValByPath` unwraps a `keyvalue` entry to its value,
`getKeyByPath` to its key, `getNodeByPath` returns the raw entry node
unwrapped either way. Lower-level walking is also available for hand-rolled
traversal: `ast.child(&node)`/`ast.next(&node)`/`ast.lastChild(&node)` follow
a container's first child and sibling chain; `doc.ast.node(id)` reads any
node by id, as a `Node` whose `kind` you can switch on, and
`doc.ast.nodeCount()` says how many there are; `doc.ast.parentOf(id)` goes
up; `doc.span(node)` recovers its source byte range.

YAML's reference layer (anchors/aliases/`<<` merges) is opt-in and separate
from default navigation, which treats an alias as an opaque leaf:
`ast.resolveAlias(node)`/`ast.resolveDeep(node)` follow `*name` to its `&name`
target (guarding cycles/depth), and `ast.mergedChild(mapping, key)` looks a
key up through a `<<` merge. `deserialize` (below) calls `resolveDeep`
automatically; hand-rolled traversal must opt in explicitly.

A multi-document stream is the language's to split:
`fig.Runtime.Language.splitStream(allocator, input, entry.typeOf())` locates
each document of a YAML stream (`---`/`...`-delimited) as a byte range to
parse on its own, and refuses with `error.UnsupportedOperation` for a
language without streams.

## The value tree

A `Node`'s `kind` is a tagged union — switch on it to read a value generically:

```zig
const AST = fig.AST;

fn describe(node: AST.Node) []const u8 {
    return switch (node.kind) {
        .null_ => "null",
        .boolean => "bool",
        .string => "string",
        .number => |n| if (n.kind == .integer) "int" else "float",
        .extended => "extended", // TOML datetime, ZON enum/char literal, ...
        .sequence => "sequence",
        .mapping => "mapping",
        .keyvalue => "keyvalue", // a mapping entry wrapper
        .alias => "alias",       // an unresolved YAML `*name`
    };
}
```

`number.raw` is the value's original decimal text (never a parsed `f64`/`i64`
— that conversion, and its overflow/precision decisions, is yours to make)
plus a `kind: .integer | .float` tag preserving the source's own distinction.
`extended` carries a format-specific scalar fig has no dedicated variant for —
a TOML datetime, a ZON enum (`.foo`) or char (`'A'`) literal, a JSON5
non-finite float — as `{ kind: ExtKind, text: []const u8 }`; `text` is the
value's own bytes (a datetime's timestamp, an enum's bare name, a char's
decimal codepoint).

To *build* a value tree from scratch (rather than parsing one), use
`AST.Builder`:

```zig
var b: fig.AST.Builder = .init(allocator);
defer b.deinit();

const name = try b.addString("fig");
const one = try b.addInt(1);
const two = try b.addInt(2);
const nums = try b.addSequence(&.{ one, two });
const root = try b.addMapping(&.{
    .{ .key = try b.addString("name"), .value = name },
    .{ .key = try b.addString("nums"), .value = nums },
});

var ast = try b.finish(root); // freezes the builder into an owned AST
defer ast.deinit();

var out: std.Io.Writer.Allocating = .init(allocator);
defer out.deinit();
try fig.Serialize.serialize(&ast, &out.writer, fig.Serialize.Target.named("json").?);
// {"name": "fig", "nums": [1, 2]}
```

Construction is bottom-up (children before the container that holds them);
every string passed in is copied into the built AST, so caller buffers need
not outlive it. `Builder` also has `addNull`/`addBool`/`addUint`/
`addNumberRaw`/`addExtended`; `setComments`/`addLeadingComment`/
`addDanglingComment`/`setTrailingComment` to attach comments programmatically;
and `setTag` for a cross-format type annotation (figl's `: int =`, YAML's
`!!int`). Use `b.view()` instead of `finish()` to inspect/serialize an
in-progress build without consuming the builder.

## Editing without reserializing

This is what sets `fig` apart. `Editor(Runtime.Language)` splices only the
bytes of the node you touch — everything else in the file is preserved
exactly. Give it the dialect to edit as, then `init` it with the source:

```zig
const toml = fig.Runtime.entryByName("toml").?;

var ed: fig.Editor(fig.Runtime.Language) = .{ .allocator = allocator, .format = toml.typeOf() };
defer ed.deinit();
try ed.init("# app config\nhost = \"localhost\"  # dev box\nport = 8080\n");

try ed.replaceValAtPath(&.{.{ .key = "port" }}, "9090");
try ed.set(&.{.{ .key = "debug" }}, "true"); // replace if present, else insert

std.debug.print("{s}", .{ed.source.items});
// # app config
// host = "localhost"  # dev box
// port = 9090
// debug = true
```

Unlike a higher-level binding, `Editor` never frames a value for you: every
`value_text`/`key_text` argument is text **already serialized in the
document's own format** — a bare `9090` for TOML/YAML, but `"9090"` for
strict JSON if you mean a string. For a literal scalar you can just write the
text by hand; to splice a *computed* value correctly quoted for the target
format, render it with `AST.Builder` + `Serialize.serializeNode` first:

```zig
var vb: fig.AST.Builder = .init(allocator);
defer vb.deinit();
const v = try vb.addString("a value with \"quotes\"");
var ast = try vb.view(v); // no need to `finish` just to serialize one node

var text: std.Io.Writer.Allocating = .init(allocator);
defer text.deinit();
try fig.Serialize.serializeNode(&ast, &text.writer, .{ .language = toml }, v); // -> `"a value with \"quotes\""`
try ed.replaceValAtPath(&.{.{ .key = "note" }}, text.written());
```

Every edit is addressed by a **path** (`[]const AST.PathSegment`, the same
type `getValByPath` takes) and reparses after splicing — on a failed reparse
the source is rolled back byte-for-byte, so a rejected edit leaves the editor
exactly as it was. Reach for `getParsed()` to read the current `Document`
back out of the editor (e.g. to compute the next edit's path).

Common operations:

```zig
try ed.insertKey(&.{}, "region", "\"us-east-1\"");    // add a mapping entry
try ed.replaceValAtPath(&.{.{ .key = "port" }}, "80"); // change a value
try ed.replaceKeyAtPath(&.{.{ .key = "port" }}, "listen"); // rename a key
try ed.set(&.{.{ .key = "debug" }}, "false");          // upsert (replace or insert)
try ed.deleteKey(&.{.{ .key = "debug" }});             // remove a mapping entry
try ed.appendToSeq(&.{.{ .key = "tags" }}, "\"c\"");   // push onto a sequence
try ed.prependToSeq(&.{.{ .key = "tags" }}, "\"a\"");
try ed.insertItem(&.{.{ .key = "tags" }}, 1, "\"b\"");  // insert so it is item 1; past the end is NotFound
try ed.removeSeqItem(&.{.{ .key = "tags" }}, 0);       // remove sequence item by index
try ed.moveKey(&.{.{ .key = "a" }}, &.{.{ .key = "b" }});     // reorder mapping entries
try ed.reorderKeys(&.{}, &.{ "title", "body" });              // named keys first, rest follow
try ed.moveItem(&.{.{ .key = "tags" }}, 2, 0);                // reorder sequence items
try ed.reorderItems(&.{.{ .key = "tags" }}, &.{ 2, 0 });      // bring these indices to the front
try ed.setSequence(&.{.{ .key = "tags" }}, &.{ "\"c\"", "\"a\"" }); // reconcile a list, keeping survivors' comments
```

`setSequence` is the one op above with a narrower domain than the rest: it
matches new items to old ones by *value* so that a kept-or-merely-reordered
item keeps its comments, which means it needs to parse each item text as a
standalone document. TOML scalars can't stand alone, so it declines there with
`error.UnsupportedShape` — as it does for an empty list on either side, or any
non-scalar item. (Nothing is lost: a TOML inline array carries no per-element
comments to preserve, so replacing the whole value is equivalent.) It is worth
reaching for on YAML and figl, where per-item comments are real:

```zig
var ed: fig.Editor(fig.Runtime.Language) = .{ .allocator = allocator, .format = fig.Runtime.entryByName("yaml").?.typeOf() };
defer ed.deinit();
try ed.init("tags:\n- a # keep me\n- b\n");
try ed.setSequence(&.{.{ .key = "tags" }}, &.{ "c", "a" });
// tags:
// - c
// - a # keep me
```

`replaceAtSpan(span, text)` is the escape hatch underneath all of them: splice
an arbitrary byte range you located yourself (via `getParsed()` and
`doc.span(node)`), with the same reparse-or-roll-back guarantee.

Comments are first-class:

```zig
try ed.addLeadingComment(&.{.{ .key = "port" }}, "the listening port"); // own-line, above
try ed.setTrailingComment(&.{.{ .key = "port" }}, "default 8080");     // same-line
const leading = try ed.getLeadingComment(&.{.{ .key = "port" }});      // "" = bare marker, null = none
if (leading) |text| allocator.free(text);
const trailing = try ed.getTrailingComment(&.{.{ .key = "port" }});    // same convention
if (trailing) |text| allocator.free(text);
try ed.deleteTrailingComment(&.{.{ .key = "port" }});
try ed.deleteLeadingComments(&.{.{ .key = "port" }});   // drops the whole owned block
```

Both getters return allocator-owned memory — free what you get back. Not every
format has both halves: INI and NestedText have real leading comments but no
trailing-comment syntax at all, so `setTrailingComment` there returns
`error.CommentsUnsupported` the way strict JSON does for every comment op.
A value that runs to the end of its line has no room after it either — a ZON
`\\` string is string text up to its newline — so a trailing comment there
would be read back as part of the value, and `setTrailingComment` rolls the
edit back with `error.CommentInsideValue`.

The comment marker (`#`, `//`) is chosen for the language; strict JSON has no
comments and every comment op returns `error.CommentsUnsupported`.

A container has a THIRD anchor: the **dangling** run at the end of its body,
after its last entry (spec § 3.4), which is where a commented-out *last* entry
lives. It is addressed by the container's own path — the empty path for the
document root:

```zig
try ed.addDanglingComment(&.{.{ .key = "server" }}, "was: here"); // at the body's child depth
const run = try ed.getDanglingComment(&.{.{ .key = "server" }}); // null = none, "" = bare marker
if (run) |text| allocator.free(text);
try ed.deleteDanglingComments(&.{});   // the run at the end of the document
```

A scalar has no body to end, and neither has a flow container written on one
line (`{ "a": 1 }`) — both return `error.UnsupportedShape`. A pretty-printed
JSONC object is fine: its `// note` before the closing brace is the root's
dangling run, and nothing else can address it.

Turning an entry into a comment run and back is `commentOut` / `uncomment*`:

```zig
try ed.commentOut(&.{ .{ .key = "server" }, .{ .key = "port" } });
// server:
//   # port: 8080
//   host: local
try ed.uncommentLeading(&.{ .{ .key = "server" }, .{ .key = "host" } }, 0, 1); // byte-identical again
```

`commentOut` prefixes every line of the node's span with the marker at that
line's own indentation (past a fig `>` marker run, so depth survives), leaving
the node's own leading block above it untouched; afterwards the tree has no
node at that path. The entry becomes the leading block of whatever followed it,
or — when it was last — the parent's dangling run, which
`uncommentDangling(container_path, first_line, line_count)` addresses instead.

The uncomment pair takes lines by index within the block, because *which* lines
look like an entry is the caller's judgement; the editor's part is the byte edit
and the guarantee that it landed. If the result does not parse, or parses to a
document whose other nodes moved, the splice is rolled back and the call returns
the parse error or `error.CommentNotAnEntry` — the document is byte-for-byte as
it was, as every editor op promises. `error.UnsupportedShape` for the root;
`error.CommentsUnanchored`, as the six comment ops answer the same node, for
one that does not have its lines to itself (an item of `[a, b]`, an entry of
`{ "a": 1, "b": 2 }`); a whole `[table]`/`[section]` is refused with
`CannotDeleteSection` for the reason `deleteKey` refuses it.

### Whole-container editing (section formats)

TOML, figl and INI have containers whose bytes are *scattered* — a TOML table
reopened by a later `[header]`, a fig container re-entered further down, an INI
`[section]` written twice. There is no single byte range to splice, so these
formats get extra operations that gather a container's disjoint regions and
rebuild the source in one edit:

```zig
// Paths are the usual `[]const AST.PathSegment`; only `reorderContainers`
// takes plain key-name strings (there's nothing to index into yet).
try ed.deleteContainer(&.{.{ .key = "servers" }});
try ed.moveContainer(&.{.{ .key = "a" }}, &.{.{ .key = "b" }}); // null dest = move to EOF
try ed.reorderContainers(&.{ "package", "dependencies" });

// TOML additionally, where a header path has descendants to follow:
try ed.insertContainer(&.{ .{ .key = "servers" }, .{ .key = "alpha" } }, "ip = \"10.0.0.1\"\n");
try ed.renameContainer(&.{.{ .key = "servers" }}, "hosts"); // also `[servers.x]`, `[[servers.y]]`
try ed.appendContainerToSeq(&.{.{ .key = "products" }}, "name = \"hammer\"\n"); // new `[[products]]`
```

Every `Editor` has them, and what they can do is read off the language's
declarations: the section regions its parser records, and its `syntax`'s
`section_header` for the two that write a header line, which TOML alone
declares. A format with contiguous containers — YAML, JSON, ZON, … — refuses
them (`NotASection`), since `deleteKey` is already the right operation there.

All six are also in the C ABI as `fig_editor_*_container`, and wrapped by the
Rust and TypeScript bindings.

## Config inside a host file

fig edits a config document, not the file it sits in. Frontmatter,
endmatter, and fenced or HTML blocks in Markdown are located by
[twig](https://github.com/diaryx-org/twig); run an `Editor` over the block's
contents and splice its `source` back between the bytes twig reported.

## Serialization options

`fig.Serialize.serialize`/`serializeWith` (whole tree) and
`serializeNode`/`serializeNodeWith` (one subtree) print to a
`Serialize.Target` — `.canonical`, or `.{ .language = entry }` for a
registered dialect (`Target.named("json")` finds either) — and take an
optional `AST.SerializeOptions`:

```zig
const json = fig.Serialize.Target.named("json").?;
try fig.Serialize.serializeWith(&ast, &out.writer, json, .{ .pretty = false });      // minified
try fig.Serialize.serializeWith(&ast, &out.writer, json, .{ .indent = 4 });          // 4-space indent
try fig.Serialize.serializeWith(&ast, &out.writer, fig.Serialize.Target.named("toml").?, .{ .width = 40 });  // inline vs [section] budget
try fig.Serialize.serializeWith(&ast, &out.writer, fig.Serialize.Target.named("yaml").?, .{ .strip_comments = true });
```

| Field           | Applies to        | Meaning                                          |
| --------------- | ----------------- | ------------------------------------------------- |
| `pretty`        | JSON/JSON5, ZON, TOML | Multi-line (default) vs. compact. TOML uses it to gate array wrapping; YAML ignores it. |
| `indent`        | JSON/JSON5, TOML  | Spaces per level (default 2).                    |
| `width`         | TOML, YAML, figl  | Column budget for inline (flow) vs. expanded layout (default 80). `1` means "always block". |
| `strip_comments`| all               | Drop carried comments instead of emitting them.  |
| `indent_requested`    | figl              | Opt-in cosmetic `2 × depth` indentation (`fig fmt --indent`). |
| `flow`          | figl, fragments   | Render a container root as inline flow (`[a, b]` / `{k=v}`) rather than block. |

`canonical` is the AST's own total, bijective encoding: every node kind,
including ones no other format can hold, round-trips through it unchanged,
which makes it useful both as a debug dump and as the comparison oracle
round-trip tests use. It is opt-in at build time (`-Dcanonical`), though a
test build always compiles it.

Serializing can fail for reasons the target format decides — `SerializeError`
covers `NullUnsupported` (TOML), `NonStringKey` (TOML/ZON),
`UnrepresentableRoot`, `UnsupportedValue` (a sequence or mapping
reaching INI/dotenv), `InvalidKey` (a dotenv key that isn't a bash
identifier), `NonFiniteNumber`, and `PrintRefused` for a refusal the
language gave no reason core names. A language's printer refuses through
the contract (`ErrorInfo.code`, `abi.Refusal`), and the refusal comes back as
the `SerializeError` it names.

## Diagnostics & lossless conversion

Converting between formats can lose information — TOML has no `null`, plain
JSON has no comments or datetimes. `fig.Diagnostics.analyze` tells you exactly
what *would* be lost, without doing it:

```zig
var arena_state = std.heap.ArenaAllocator.init(allocator);
defer arena_state.deinit();

const doc = try fig.Runtime.Language.Parser.parse(allocator, "a: null\nb: 1 # keep\n", fig.Runtime.entryByName("yaml").?.typeOf());
defer doc.deinit(allocator);

const toml = fig.Serialize.Target.named("toml").?;
const warnings = try fig.Diagnostics.analyze(arena_state.allocator(), &doc.ast, doc.ast.root, toml, .{});
for (warnings) |w| {
    var msg: std.Io.Writer.Allocating = .init(allocator);
    defer msg.deinit();
    try w.render(&msg.writer, toml.name());
    std.debug.print("{s}\n", .{msg.written()});
    // "dropped null value at `a` (toml cannot represent it)"
}
```

Each `Warning` carries a `code` (`.value_dropped`, `.type_degraded`,
`.comment_dropped`, `.comment_style_degraded`), a `cause`
(`.format_limitation` vs. `.explicit_option`, when you passed
`strip_comments`), and a dotted/`[i]` `path` to the affected node. What a
language loses is read off what it declares — the `prints` of its dialect
rows, its `max_mapping_depth`, its comment syntax — so a language you
register is held to the same warnings as fig's own.

To *preserve* an otherwise-lossy value instead of dropping or degrading it,
use `fig.Lossless`: `encode(arena, ast, native)` rewrites every value the
target can't hold natively into a reserved `{ "$fig": { ... } }` envelope
before printing; `decode(arena, ast)` reverses it after parsing back.
`needsEnvelope`/`isUnrepresentable` (the same capability table `Diagnostics`
consults) let you check a single node's fate up front, and `lossyStrip` gives
you the CLI's default (non-lossless) behavior explicitly: drop only what a
target truly cannot represent at all (today, just a `null` bound for TOML) and
report the dropped paths, rather than aborting the whole serialize.
`Lossless.NativeKinds` is what a target holds — the scalar kinds a format
spells natively, declared by the format itself in `Language.caps.lossless` —
and `Lossless.nativeFor(target)` reads a target's declaration, or `null` for
a format that takes no envelope.

`fig.FlatStrip` is the sibling pass for the three flat/shallow formats — INI,
dotenv, `.properties` — whose limits are about *depth* rather than scalar kind
(INI holds a root mapping plus one level of `[section]`s; dotenv and
`.properties` are flat outright). Run it before printing to drop what those
formats can't hold at all — a `null`, a sequence at any depth, a mapping
nested past the limit — so the printer never aborts a document partway
through. It reports the same set `Diagnostics.analyze` warns about for those
targets, so the usual sequence is analyze-then-strip-then-print.

## Reflective deserialization

For the common case — "give me my struct" — skip the AST entirely and let
`fig.deserialize` map a parsed document onto a native Zig type by
`@typeInfo` reflection, à la `std.json.parseFromSlice`:

```zig
const Config = struct {
    title: []const u8,
    count: i64,
    tags: []const []const u8,
    nickname: ?[]const u8 = null,
    retries: u8 = 7,
};

const parsed = try fig.deserialize.parseFromSlice(Config, allocator, source, "yaml", .{});
defer parsed.deinit();

parsed.value.title; // ...
```

`Parsed(T)` owns an arena backing every allocation `T` needed (slices,
strings); `deinit()` frees all of it in one shot. The format is any
registered dialect's name; one not registered is `error.UnsupportedFormat`.
Structs, optionals (missing → `null`),
defaulted fields (`= 7`), enums (from a string or ZON enum-literal scalar),
slices, and fixed-size arrays (which require an *exact*-length sequence) are
all supported; a mapping key with no matching field is ignored by default
(`Options.ignore_unknown_fields = true`) or `error.UnknownField` when you set
it `false`. Use `parseFromSliceLeaky` to allocate into a caller-owned arena
you manage yourself instead. For anything beyond this — comment-preserving
edits, full structural access, cross-format fidelity — use `Editor`/`Document`/
`AST` directly; `deserialize` is deliberately the lossy, tolerant one-shot path.

## Errors

Every entry point returns a plain Zig error union — `try`/`catch` it as usual.
A bare `parse` gives you only the error code:

```zig
const json = fig.Runtime.entryByName("json").?;
var parser: fig.Runtime.Language.Parser = .{ .allocator = allocator };
const doc = fig.Runtime.Language.parse(&parser, "{ not valid", json.typeOf()) catch |err| {
    std.debug.print("parse failed: {s}\n", .{parser.lastMessage()});
    return err;
};
```

A language says why it refused, where, and what it would warn about on
input it reads: the diagnostics cross the contract as rows of the node
table. Give the `Parser` an `errors` and a `warnings` list
(`std.ArrayList(ParseDiagnostic.Rendered)`) and it fills them — every error
of a failed parse where the language can keep going past one, the
authoring warnings of a clean one — each with its span, message and label.
`fig.ParseDiagnostic.locateOffset(source, offset)` turns a raw byte offset
into a 1-based `{ line, column, line_text }`, and `renderReport`/
`renderReportAlloc` format the `file:line:col: label: message` + caret text
every language's own diagnostic renders into.

## Managing resources

Nothing here is garbage collected — every owning value must be `deinit`ed,
almost always paired with `defer` right after construction:

```zig
const yaml = fig.Runtime.entryByName("yaml").?;
const doc = try fig.Runtime.Language.Parser.parse(allocator, source, yaml.typeOf());
defer doc.deinit(allocator); // frees the AST's rows, buffer and side facts

var ed: fig.Editor(fig.Runtime.Language) = .{ .allocator = allocator, .format = yaml.typeOf() };
defer ed.deinit(); // frees its internal Document + source buffer
try ed.init(source);

const docs = try fig.Runtime.Language.splitStream(allocator, source, yaml.typeOf());
defer allocator.free(docs); // the byte range of each document of a stream

const parsed = try fig.deserialize.parseFromSlice(Config, allocator, source, "yaml", .{});
defer parsed.deinit(); // frees the arena backing parsed.value

var b: fig.AST.Builder = .init(allocator);
defer b.deinit(); // harmless no-op after a successful `finish()`
```

`AST.deinit()` (called via `Document.deinit`, or directly on an AST you built/
decoded yourself) frees its rows, its string buffer and its side facts. A registration lives as long as the process: the registry
frees nothing until it ends. A `Document`'s `source` is always borrowed — freeing it, if it
was heap-allocated in the first place, is the caller's responsibility, done
only *after* the `Document` that borrows it is itself freed.

## API reference

**Top-level modules** (`@import("fig")` re-exports each as a member)

- `Runtime` — the registry and the one `Language` every format goes through:
  `register`/`registerWith`, `entryByName`/`entryByAbi`/`entryAt`/`count`,
  `detectAmong`, `Language` (`Parser`, `parse`, `splitStream`), `VTable` and
  the node table (`table`).
- `bundle` — the languages fig ships: `register`/`registerAll`, `bundles`,
  `Language`/`module` (a language's Zig module), `dialect_names`.
- `Language` — the Zig language contract: `validate`, and the declaration
  types (`Caps`, `Syntax`, `Dialect`, `Prints`, …).
- `Document` — `{ source, ast }`; `deinit`, `span`/`spanOf`,
  `anchorSpan`, `tagSpan`, `markerSpan`, `sepSpan`, `itemSep`, `regionsOf`,
  `mentionsOf`.
- `AST` — the value tree, stored as the node table: `node(id)`/`kindOf`/
  `nodeCount`/`parentOf`, `tagOf`/`tagFact`, `anchorOf`, `comments`, path navigation
  (`getValByPath`/`getKeyByPath`/`getNodeByPath`/`child`/`next`/`lastChild`),
  reference-layer resolution (`resolveAlias`/`resolveDeep`/`mergedChild`),
  `SerializeOptions`, and `AST.Builder` (the write path).
- `Serialize` — `serialize`/`serializeWith`/`serializeNode`/
  `serializeNodeWith` to a `Target` (`.canonical` or a registered language).
- `Editor(Runtime.Language)` — the comment-preserving editor; see
  [Editing without reserializing](#editing-without-reserializing).
- `Lossless` — `$fig`-envelope lossless conversion: `encode`, `decode`,
  `needsEnvelope`, `isUnrepresentable`, `lossyStrip`, `NativeKinds`,
  `nativeFor`.
- `FlatStrip` — depth-based lossy stripping for INI/dotenv/`.properties`, the
  `Lossless` sibling for the flat formats.
- `Diagnostics` — `analyze` reports what a serialize would lose (`Warning`,
  `Code`, `Cause`, `Options`).
- `ParseDiagnostic` — shared offset→line/col + report rendering
  (`locateOffset`, `renderReport`, `renderReportAlloc`, `Rendered`).
- `Canonical` — the AST's own 1:1 total
  encoding: `parse`, `parseAbstract`, `print`, `printNode`.
- `deserialize` — reflection-based `parseFromSlice`/`parseFromSliceLeaky`
  into a native Zig type, from a dialect named; see
  [Reflective deserialization](#reflective-deserialization).

**Per-`Language` shape** (what `Language.validate` requires of a Zig
language module)

- `Type` — the format's dialect enum (e.g. JSON's `JSON`/`JSONC`/`JSON5`).
- `default_type: Type` — the dialect `parse`/`print` use when none is named.
- `Parser` — a type with `parse(allocator, input, Type) !Document`
  (and, for most languages, `parseWithReport`/`parseCollecting` for rich
  diagnostics).
- `print` / `printNode` — render an `AST`/subtree to a `Writer`.
