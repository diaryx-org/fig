```fig
title = Using fig in Zig
author = adammharris
created = 2026-07-05T21:35:14-06:00
updated = 2026-08-08T10:00:00-06:00
part_of = [docs](docs.md)
```

# fig for Zig

`fig` is a Zig library that parses, **edits**, and serializes configuration
files — JSON, JSONC, JSON5, YAML, TOML, ZON, INI, dotenv, Java `.properties`,
NestedText, Apple XML property lists, XML (read-only), and the native `fig`
authoring dialect — from one small package with no external
dependencies. Its distinguishing feature is *comment-preserving editing*: you
can change one value deep in a YAML or TOML file and every comment, blank
line, key order, and quoting style elsewhere stays byte-for-byte identical. It
also converts losslessly between formats and edits config embedded in
markdown frontmatter.

This is the core library — the same code the C ABI, the TypeScript/WASM
package, and the `fig` CLI are all built on. Used directly from Zig you get
the whole surface: comptime language selection, zero-copy reads over the
source buffer, and full control over allocation (there is no hidden global
state or GC — you manage every `deinit`).

- [Install](#install)
- [Quick start](#quick-start)
- [Formats](#formats)
- [Reading data](#reading-data)
- [The value tree](#the-value-tree)
- [Editing without reserializing](#editing-without-reserializing)
- [Markdown frontmatter & embeds](#markdown-frontmatter--embeds)
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

Most formats are compiled in by default. Each is gated by its own build
option, so a consumer that only needs JSON+YAML can trim the rest out of the
binary by passing options through to the dependency:

```zig
const fig_dep = b.dependency("fig", .{
    .target = target,
    .optimize = optimize,
    .toml = false,  // drop TOML support
    .zon = false,   // drop ZON support
    .xml = true,    // XML is opt-in; enable it if you need it
    .plist = true,  // so is plist
});
```

| Option        | Default | Includes                                    |
| ------------- | :-----: | ------------------------------------------- |
| `json`        |   ✅    | The shared JSON/JSONC/JSON5 core.           |
| `yaml`        |   ✅    | YAML 1.2.2 / 1.1.                           |
| `toml`        |   ✅    | TOML 1.0 / 1.1.                             |
| `zon`         |   ✅    | Zig Object Notation.                        |
| `fig`         |   ✅    | The native `fig` authoring dialect.         |
| `ini`         |   ✅    | INI (`[section]` + `key = value`).          |
| `dotenv`      |   ✅    | dotenv / `.env`.                            |
| `properties`  |   ✅    | Java `.properties`.                         |
| `nestedtext`  |   ✅    | NestedText (nestedtext.org).                |
| `xml`         |         | Generic XML (read-only; slated for removal as a selectable format — see [BREAKING-CHANGES](BREAKING-CHANGES.md)). |
| `plist`       |         | Apple XML property lists.                   |
| `canonical`   |         | The AST's own oracle encoding (mostly for tests; always compiled for a test build). |

A disabled language's `Language.*` member (see below) resolves to `void` at
comptime rather than being omitted, so any code path that touches a gated-out
language must itself be guarded with `if (comptime build_options.lang_toml)
...` — see [Formats](#formats).

## Quick start

```zig
const std = @import("std");
const fig = @import("fig");

pub fn main() !void {
    var gpa: std.heap.DebugAllocator(.{}) = .init;
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    // Parse JSON into a Document (the AST plus source spans).
    const doc = try fig.Language.JSON.Parser.parse(allocator, "{\"name\":\"fig\",\"nums\":[1,2]}", .JSON);
    defer doc.deinit(allocator);

    // Convert to YAML.
    var out: std.Io.Writer.Allocating = .init(allocator);
    defer out.deinit();
    try doc.ast.serialize(&out.writer, .yaml);
    std.debug.print("{s}", .{out.written()}); // name: fig\nnums:\n- 1\n- 2\n
}
```

Every `Language` module exposes the same two entry points — `Parser.parse`
(text → `Document`) and `Printer.print`/`AST.serialize` (`AST` → text) — so
swapping `.JSON`/`json.Type` for `.TOML`/`toml.Type` etc. is the whole
adaptation needed to target a different format.

## Formats

| `Language.*` | Parse | Edit | Serialize | Notes                                    |
| ------------ | :---: | :--: | :-------: | ----------------------------------------- |
| `JSON`       |  ✅   |  ✅  |    ✅     | `Type` is `JSON` / `JSONC` / `JSON5`.     |
| `YAML`       |  ✅   |  ✅  |    ✅     | YAML 1.2.2 / 1.1, incl. anchors/aliases.  |
| `TOML`       |  ✅   |  ✅  |    ✅     | TOML 1.0 / 1.1, incl. datetimes.          |
| `ZON`        |  ✅   |  ✅  |    ✅     | Zig Object Notation.                      |
| `XML`        |  ✅   |      |    ✅     | Opt-in (`-Dxml`); root must be one element.|
| `FIG`        |  ✅   |  ✅  |    ✅     | The native `fig` authoring dialect.       |
| `INI`        |  ✅   |  ✅  |    ✅     | Untyped scalars — `port = 8080` reads back as the *string* `"8080"`. |
| `DOTENV`     |  ✅   |  ✅  |    ✅     | Flat string map: no nesting, untyped scalars. |
| `PROPERTIES` |  ✅   |  ✅  |    ✅     | Java `.properties`; same flat, untyped limits as dotenv. |
| `PLIST`      |  ✅   |  ✅  |    ✅     | Opt-in (`-Dplist`). Typed *and* nested; date/data ride on `extended`. |
| `NESTEDTEXT` |  ✅   |  ✅  |    ✅     | Nested (dict/list) but deliberately untyped — every leaf is a string. |

The four untyped formats (INI, dotenv, `.properties`, NestedText) parse every
scalar as a string; nothing infers a number or a bool from them. INI, dotenv
and `.properties` are also shape-limited — see
[`FlatStrip`](#diagnostics--lossless-conversion) for what a serialize to one
of them drops.

Don't hard-code this table into your program — each row is only compiled in
when its build option is set (`build_options.lang_json`, `.lang_yaml`,
`.lang_toml`, `.lang_zon`, `.lang_xml`, `.lang_fig`, `.lang_ini`,
`.lang_dotenv`, `.lang_properties`, `.lang_plist`, `.lang_nestedtext`), and
`Language.TOML` etc. is `void` when its flag is off. Guard any generic code
with the matching comptime check, the way `Language.detect` itself does:

```zig
if (comptime build_options.lang_toml) {
    const doc = try fig.Language.TOML.Parser.parse(allocator, source, fig.Language.TOML.default_type);
    defer doc.deinit(allocator);
}
```

`fig.Language.detect(allocator, input)` sniffs which compiled-in format
`input` parses as, returning a `Language.Detected`. It tries strictest first:
JSON, JSON5, ZON, plist, XML, TOML, fig, INI, dotenv, YAML, `.properties`,
NestedText. The tail of that order is what keeps the permissive formats from
starving the rest — `.properties` accepts nearly any UTF-8 text, and plain
`key: value` NestedText is also valid YAML (but types `port: 80` as the string
`"80"` rather than an integer), so both sit after YAML and reach selection
mainly via their file extension. See `languages/language.zig` for the
per-format reasoning.

`fig.Language.validate(comptime Lang)` is a comptime assertion that a type has
the shape (`Type`, `default_type`, `parse`, `print`) the generic engines
(`Editor(Lang)`, `deserialize`) require, and `fig.Language.compiled` is the
list of language modules this build actually contains.

Every "Edit" ✅ above goes through the *same* generic `Editor(Language)` engine
(next section) — ZON included, splicing its `.key = value` struct-field syntax
and `.{}`/`.@"..."` quoting rules exactly like JSON gets `"key": value`. Only
TOML, fig and INI additionally get the whole-table/whole-container structural
ops listed near the end of that section.

## Reading data

`Language.X.Parser.parse` gives you a `Document`: the parsed `AST` plus a
`node_spans` table mapping each node back to its byte range in `source`.
`source` is *borrowed* — keep it alive as long as the `Document` is:

```zig
const doc = try fig.Language.TOML.Parser.parse(
    allocator,
    "[server]\nhost = \"localhost\"\nports = [80, 443]\n",
    fig.Language.TOML.default_type, // .TOML_1_1
);
defer doc.deinit(allocator);

// Navigate by path — an array of `.key` (mapping) / `.index` (sequence) segments.
const host = try doc.ast.getValByPath(&.{ .{ .key = "server" }, .{ .key = "host" } });
host.kind.string; // "localhost"

const port1 = try doc.ast.getValByPath(&.{ .{ .key = "server" }, .{ .key = "ports" }, .{ .index = 1 } });
port1.kind.number.raw; // "443"

_ = doc.ast.getValByPath(&.{.{ .key = "missing" }}); // error.NotFound
```

`getValByPath`/`getKeyByPath`/`getNodeByPath` (`ast/reader.zig`) are the
high-level accessors; `getValByPath` unwraps a `keyvalue` entry to its value,
`getKeyByPath` to its key, `getNodeByPath` returns the raw entry node
unwrapped either way. Lower-level walking is also available for hand-rolled
traversal: `ast.child(&node)`/`ast.next(&node)`/`ast.lastChild(&node)` follow
a container's first child and sibling chain; `doc.ast.nodes[id]` indexes any
node by id; `doc.span(node)` recovers its source byte range.

YAML's reference layer (anchors/aliases/`<<` merges) is opt-in and separate
from default navigation, which treats an alias as an opaque leaf:
`ast.resolveAlias(node)`/`ast.resolveDeep(node)` follow `*name` to its `&name`
target (guarding cycles/depth), and `ast.mergedChild(mapping, key)` looks a
key up through a `<<` merge. `deserialize` (below) calls `resolveDeep`
automatically; hand-rolled traversal must opt in explicitly.

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
try ast.serialize(&out.writer, .json);
// {"name": "fig", "nums": [1, 2]}
```

Construction is bottom-up (children before the container that holds them);
every string passed in is copied into the built AST, so caller buffers need
not outlive it. `Builder` also has `addNull`/`addBool`/`addUint`/
`addNumberRaw`/`addExtended`; `setComments`/`addLeadingComment`/
`addDanglingComment`/`setTrailingComment` to attach comments programmatically;
and `setTag` for a cross-format type annotation (fig's `: int =`, YAML's
`!!int`). Use `b.view()` instead of `finish()` to inspect/serialize an
in-progress build without consuming the builder.

## Editing without reserializing

This is what sets `fig` apart. `Editor(Language)` splices only the bytes of
the node you touch — everything else in the file is preserved exactly.
Instantiate the generic per format, then `init` it with the source:

```zig
const Toml = fig.Language.TOML;

var ed: fig.Editor(Toml) = .{ .allocator = allocator };
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
format, render it with `AST.Builder` + `AST.serializeNode` first:

```zig
var vb: fig.AST.Builder = .init(allocator);
defer vb.deinit();
const v = try vb.addString("a value with \"quotes\"");
var ast = try vb.view(v); // no need to `finish` just to serialize one node

var text: std.Io.Writer.Allocating = .init(allocator);
defer text.deinit();
try ast.serializeNode(&text.writer, .toml, v); // -> `"a value with \"quotes\""`
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
reaching for on YAML and fig, where per-item comments are real:

```zig
var ed: fig.Editor(fig.Language.YAML) = .{ .allocator = allocator };
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

The comment marker (`#`, `//`) is chosen for the language; strict JSON has no
comments and every comment op returns `error.CommentsUnsupported`.

### Whole-container editing (section formats)

TOML, fig and INI have containers whose bytes are *scattered* — a TOML table
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

Which ops a format has is a declaration, not a hardcoded list: each is present
exactly when that language declares it (`@hasDecl(Lang, "deleteContainer")`),
and calling one a format doesn't declare is a compile error naming the format
and the missing declaration. Today TOML declares all six, fig and INI the first
three. Everything else — YAML, JSON, ZON, … — has contiguous containers, where
`deleteKey` is already the right operation.

## Markdown frontmatter & embeds

`fig.Embed` locates a config block embedded in a host file without touching an
`Editor` directly; you compose it with `Editor` yourself for edits (there is
no combined "embedded editor" type at this layer — see the CLI's
`applyToEmbed` in `src/cli/edit_ops.zig` for the exact pattern this section
follows).

### Archetypes

`fig.Embed.Type` is a *union*, not a flat enum: the four container shapes that
name their format in the source are parametric over an `Embed.InnerFormat`
(`.json`, `.yaml`, `.toml`, `.fig` — exactly the formats with an embedded
spelling), and three popular conventions whose delimiter is its own distinct
token stay as blessed presets:

```zig
const t: fig.Embed.Type = .{ .frontmatter = .yaml };  // ---      … --- / ...  (bare --- is YAML)
_ = fig.Embed.Type{ .fenced = .toml };                // ```toml  … ```
_ = fig.Embed.Type{ .html_script = .json };           // <script type="application/json"> … </script>
_ = fig.Embed.Type{ .html_code = .fig };              // <pre><code class="language-figl"> … </code></pre>
_ = fig.Embed.Type.semicolons_json;                   // ;;;      … ;;;
_ = fig.Embed.Type.plus_toml;                         // +++      … +++   (Hugo/Zola)
_ = fig.Embed.Type.endmatter_yaml;                    // trailing ```endmatter block
```

Read-only extraction:

```zig
const md = "---\ntitle: Hello\ntags:\n- draft\n---\n# Body\n\ntext\n";

const embedded = try fig.Embed.extract(allocator, md, .{ .frontmatter = .yaml });
defer embedded.deinit(allocator);

const title = try embedded.document.ast.getValByPath(&.{.{ .key = "title" }});
title.kind.string; // "Hello"

// `embedded.region` gives the open_fence/content/close_fence/body spans in
// *outer* coordinates; a node span from `embedded.document` is relative to the
// DECODED content — lift it back with `embedded.outerSpan(doc.span(node))`,
// which routes through the decode provenance map for the `<code>` archetype.
```

To edit an embedded block in place: locate the region, decode the content for
the archetype's codec, run `Editor` over just that slice (as the archetype's
inner format — `Embed.innerFormat` tells you which), re-encode, and splice the
result back between the retained fences:

```zig
fn setFrontmatterTitle(allocator: std.mem.Allocator, host: []const u8, new_title: []const u8) ![]u8 {
    const t: fig.Embed.Type = .{ .frontmatter = .yaml };
    const region = try fig.Embed.locateRegion(host, t);
    const inner = host[region.content.start..region.content.end];

    // Identity for every archetype but `html_code`, whose on-disk content is
    // entity-encoded — decode so the editor sees real config.
    const codec = fig.Embed.codecOf(t);
    const decoded = try fig.Embed.decodeForParse(allocator, inner, codec);
    defer decoded.deinit(allocator);

    var ed: fig.Editor(fig.Language.YAML) = .{ .allocator = allocator };
    defer ed.deinit();
    try ed.init(decoded.text);
    try ed.set(&.{.{ .key = "title" }}, new_title); // e.g. `"Hello, world"`

    // Span-aware: only the bytes the editor actually changed are re-encoded, so
    // an untouched `&#60;` never normalizes to `&lt;`. A passthrough copy for
    // `identity`.
    const edited = try fig.Embed.reencodeEdited(allocator, codec, inner, decoded, ed.source.items);
    defer allocator.free(edited);

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(allocator);
    try out.appendSlice(allocator, host[0..region.content.start]);
    try out.appendSlice(allocator, edited);
    try out.appendSlice(allocator, host[region.content.end..]);
    return out.toOwnedSlice(allocator);
}
```

If you only ever handle identity archetypes you can skip the codec pair and
splice `ed.source.items` directly — but going through it is what makes the same
function work for `.{ .html_code = ... }` unchanged.

- `fig.Embed.locateRegion(source, t)` finds the region without parsing it;
  `fig.Embed.extract` does both. `fig.Embed.parseSpan` parses a content span
  you already have.
- `fig.Embed.detect(source)` sniffs which archetype `source` opens (mirrors
  `Language.detect`); `fig.Embed.innerFormat(t)` reports the format that
  archetype's content is written in, and `fig.Embed.bodyIsBefore(t)` whether
  the host prose precedes the region (endmatter) or follows it.
- `fig.Embed.initRegion(allocator, source, t)` synthesizes a host with a fresh
  *empty* region when none exists yet — the "create" half of open-or-init, for
  seeding a brand-new frontmatter block before the first `set`. It returns
  `Initialized{ host, region }`, and `host` is caller-owned.
- `fig.Embed.retype(allocator, source, region, to, new_content)` rebuilds the
  region as a *different* archetype (e.g. YAML frontmatter → `;;;` JSON
  frontmatter) while keeping the host prose byte-identical.
- `fig.Embed.extractStream(allocator, source)` splits a multi-document YAML
  stream (`---`/`...`-delimited) into independently parsed `StreamDoc`s, each
  with its own `outerSpan` lift-back helper; always yields at least one
  (possibly `null`) document.

## Serialization options

`AST.serialize`/`serializeWith` (whole tree) and `AST.serializeNode`/
`serializeNodeWith` (one subtree) take an optional `AST.SerializeOptions`:

```zig
try ast.serializeWith(&out.writer, .json, .{ .pretty = false });      // minified
try ast.serializeWith(&out.writer, .json, .{ .indent = 4 });          // 4-space indent
try ast.serializeWith(&out.writer, .toml, .{ .width = 40 });          // inline vs [section] budget
try ast.serializeWith(&out.writer, .yaml, .{ .strip_comments = true });
```

| Field           | Applies to        | Meaning                                          |
| --------------- | ----------------- | ------------------------------------------------- |
| `pretty`        | JSON/JSON5, ZON, TOML | Multi-line (default) vs. compact. TOML uses it to gate array wrapping; YAML ignores it. |
| `indent`        | JSON/JSON5, TOML  | Spaces per level (default 2).                    |
| `width`         | TOML, YAML, fig   | Column budget for inline (flow) vs. expanded layout (default 80). `1` means "always block". |
| `strip_comments`| all               | Drop carried comments instead of emitting them.  |
| `fig_indent`    | fig               | Opt-in cosmetic `2 × depth` indentation (`fig fmt --indent`). |
| `flow`          | fig, fragments    | Render a container root as inline flow (`[a, b]` / `{ k = v }`) rather than block. |

`AST.SerializeFormat` is `json | jsonc | json5 | yaml | toml | zon | xml |
canonical | fig | ini | dotenv | properties | plist | nestedtext` — every
format-registry dialect, plus `canonical` spliced in after `xml`. `canonical`
(aliased as `fig.Native`, kept for backward compatibility — prefer
`fig.Canonical`) is the AST's own total, bijective encoding: every node kind,
including ones no other format can hold, round-trips through it unchanged,
which makes it useful both as a debug dump and as the comparison oracle
round-trip tests use. It is opt-in at build time (`-Dcanonical`), though a test
build always compiles it.

Serializing can fail for reasons the target format decides — `SerializeError`
covers `NullUnsupported` (TOML), `NonStringKey` (TOML/ZON/XML), the XML shape
errors, `FigUnrepresentableRoot`, `UnsupportedValue` (a sequence or mapping
reaching INI/dotenv), `InvalidKey` (a dotenv key that isn't a bash identifier),
and `FormatDisabled` when the target was compiled out of this build.

## Diagnostics & lossless conversion

Converting between formats can lose information — TOML has no `null`, plain
JSON has no comments or datetimes. `fig.Diagnostics.analyze` tells you exactly
what *would* be lost, without doing it:

```zig
var arena_state = std.heap.ArenaAllocator.init(allocator);
defer arena_state.deinit();

const doc = try fig.Language.YAML.Parser.parse(allocator, "a: null\nb: 1 # keep\n", .v1_2_2);
defer doc.deinit(allocator);

const warnings = try fig.Diagnostics.analyze(arena_state.allocator(), &doc.ast, doc.ast.root, .toml, .{});
for (warnings) |w| {
    var msg: std.Io.Writer.Allocating = .init(allocator);
    defer msg.deinit();
    try w.render(&msg.writer, .toml);
    std.debug.print("{s}\n", .{msg.written()});
    // "dropped null value at `a` (toml cannot represent it)"
}
```

Each `Warning` carries a `code` (`.value_dropped`, `.type_degraded`,
`.comment_dropped`, `.comment_style_degraded`), a `cause`
(`.format_limitation` vs. `.explicit_option`, when you passed
`strip_comments`), and a dotted/`[i]` `path` to the affected node.

To *preserve* an otherwise-lossy value instead of dropping or degrading it,
use `fig.Lossless`: `encode(arena, ast, target)` rewrites every value `target`
can't hold natively into a reserved `{ "$fig": { ... } }` envelope before
printing; `decode(arena, ast)` reverses it after parsing back. `needsEnvelope`/
`isUnrepresentable` (the same capability table `Diagnostics` consults) let you
check a single node's fate up front, and `lossyStrip` gives you the CLI's
default (non-lossless) behavior explicitly: drop only what a target truly
cannot represent at all (today, just a `null` bound for TOML) and report the
dropped paths, rather than aborting the whole serialize. `Lossless.Target` is
`json | yaml | toml | zon`; `Lossless.targetFor(format)` maps a
`SerializeFormat` onto one, or `null` for a format with no envelope support.

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

const parsed = try fig.deserialize.parseFromSlice(Config, allocator, source, .yaml, .{});
defer parsed.deinit();

parsed.value.title; // ...
```

`Parsed(T)` owns an arena backing every allocation `T` needed (slices,
strings); `deinit()` frees all of it in one shot. `fig.deserialize.Format` is
`json | jsonc | yaml | toml | zon` (a smaller set than `SerializeFormat` — no
XML/canonical/fig target here). Structs, optionals (missing → `null`),
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
const doc = fig.Language.JSON.Parser.parse(allocator, "{ not valid", .JSON) catch |err| {
    std.debug.print("parse failed: {s}\n", .{@errorName(err)});
    return err;
};
```

For a compiler-style `file:line:col: message` report with the offending
source line and a caret, most languages also expose a `parseWithReport`
(stop at the first diagnostic) and `parseCollecting` (recover past errors to
report every diagnostic in one pass) alongside `parse` — see
`languages/json/parser.zig`'s `Report`/`Diagnostic`/`Warning` for the shape.
`fig.ParseDiagnostic.locateOffset(source, offset)` turns a raw byte offset
into a 1-based `{ line, column, line_text }`, and `renderReport`/
`renderReportAlloc` format the `file:line:col: label: message` + caret text
every language's own diagnostic renders into.

## Managing resources

Nothing here is garbage collected — every owning value must be `deinit`ed,
almost always paired with `defer` right after construction:

```zig
const doc = try fig.Language.YAML.Parser.parse(allocator, source, .v1_2_2);
defer doc.deinit(allocator); // frees ast, node_spans, and friends

var ed: fig.Editor(fig.Language.YAML) = .{ .allocator = allocator };
defer ed.deinit(); // frees its internal Document + source buffer
try ed.init(source);

const embedded = try fig.Embed.extract(allocator, host, .{ .frontmatter = .yaml });
defer embedded.deinit(allocator); // frees embedded.document and its decoded buffer

const stream = try fig.Embed.extractStream(allocator, host);
defer stream.deinit(allocator); // frees every StreamDoc's document

const parsed = try fig.deserialize.parseFromSlice(Config, allocator, source, .yaml, .{});
defer parsed.deinit(); // frees the arena backing parsed.value

var b: fig.AST.Builder = .init(allocator);
defer b.deinit(); // harmless no-op after a successful `finish()`
```

`AST.deinit()` (called via `Document.deinit`, or directly on an AST you built/
decoded yourself) frees `nodes`, `owned_strings`, and the tag/anchor/comment
side-tables. A `Document`'s `source` is always borrowed — freeing it, if it
was heap-allocated in the first place, is the caller's responsibility, done
only *after* the `Document` that borrows it is itself freed.

## API reference

**Top-level modules** (`@import("fig")` re-exports each as a member)

- `Language` — per-format namespace (`.JSON`/`.YAML`/`.TOML`/`.ZON`/`.XML`/
  `.FIG`/`.INI`/`.DOTENV`/`.PROPERTIES`/`.PLIST`/`.NESTEDTEXT`, each `void`
  when compiled out) plus `detect`/`Detected`/`validate`/`compiled`.
- `Document` — `{ source, ast, node_spans, ... }`; `deinit`, `span`,
  `anchorSpan`, `tagSpan`.
- `AST` — the value tree: `Node`/`Node.Kind`, path navigation
  (`getValByPath`/`getKeyByPath`/`getNodeByPath`/`child`/`next`/`lastChild`),
  reference-layer resolution (`resolveAlias`/`resolveDeep`/`mergedChild`),
  serialization (`serialize`/`serializeWith`/`serializeNode`/
  `serializeNodeWith`, `SerializeFormat`, `SerializeOptions`), and
  `AST.Builder` (the write path).
- `Editor(Language)` — comment-preserving generic editor; see
  [Editing without reserializing](#editing-without-reserializing).
- `Embed` — frontmatter/embed location + splicing: `Type`, `InnerFormat`,
  `extract`, `locateRegion`, `parseSpan`, `detect`, `innerFormat`,
  `initRegion`, `retype`, `extractStream`, and the codec trio
  (`codecOf`/`decodeForParse`/`reencodeEdited`); see
  [Markdown frontmatter & embeds](#markdown-frontmatter--embeds).
- `Lossless` — `$fig`-envelope lossless conversion: `encode`, `decode`,
  `needsEnvelope`, `isUnrepresentable`, `lossyStrip`, `Target`, `targetFor`.
- `FlatStrip` — depth-based lossy stripping for INI/dotenv/`.properties`, the
  `Lossless` sibling for the flat formats.
- `Diagnostics` — `analyze` reports what a serialize would lose (`Warning`,
  `Code`, `Cause`, `Options`).
- `ParseDiagnostic` — shared offset→line/col + report rendering
  (`locateOffset`, `renderReport`, `renderReportAlloc`, `Rendered`).
- `Canonical` (aliased `Native`, deprecated) — the AST's own 1:1 total
  encoding: `parse`, `parseAbstract`, `print`, `printNode`.
- `Fig` — the native `fig` authoring dialect's `Language`-shaped module
  (`Parser`, `Printer`, `Type`).
- `deserialize` — reflection-based `parseFromSlice`/`parseFromSliceLeaky`
  into a native Zig type; see [Reflective deserialization](#reflective-deserialization).

**Per-`Language` shape** (what `Language.validate` requires)

- `Type` — the format's dialect enum (e.g. JSON's `JSON`/`JSONC`/`JSON5`).
- `default_type: Type` — the dialect `parse`/`print` use when none is named.
- `Parser` — a type with `parse(allocator, input, Type) !Document`
  (and, for most languages, `parseWithReport`/`parseCollecting` for rich
  diagnostics).
- `print` / `printNode` — render an `AST`/subtree to a `Writer`.
