//! `--help`/usage text for the `fig` CLI, one function per action plus the
//! top-level `general` summary. Pure output — no parsing or state.
//!
//! The two lists that drifted per action — the format names `--input`/
//! `--output` take and the `--embed` archetypes — are derived or shared
//! (`formatList`, `embed_archetypes`), so every action's help says what the
//! argument parser accepts.
const std = @import("std");
const fig = @import("fig");
const build_options = @import("build_options");
const types = @import("types.zig");
const Io = std.Io;

/// Whether the compiled `Format` member `name` is in this build: a registry
/// entry whose language was not compiled out, `canonical` when
/// `-Dcanonical=true`, and `gron`, which the CLI always carries.
fn compiledIn(comptime name: []const u8) bool {
    if (comptime std.mem.eql(u8, name, "gron")) return true;
    if (comptime std.mem.eql(u8, name, "canonical")) return build_options.lang_canonical;
    return fig.Language.entryFor(name).Lang != void;
}

/// The format names `parseFormatName` accepts in this build — every compiled
/// `Format` member, less `except` (gron for an action that writes a stored
/// document) — comma-separated and wrapped under `indent` to fit 72 columns.
/// Derived from the enum, so a new format is listed everywhere at once.
fn formatList(comptime indent: []const u8, comptime except: []const []const u8) []const u8 {
    comptime {
        @setEvalBranchQuota(50_000);
        var out: []const u8 = indent;
        var col = indent.len;
        var first = true;
        fields: for (@typeInfo(types.Format).@"enum".fields) |f| {
            for (except) |e| if (std.mem.eql(u8, e, f.name)) continue :fields;
            if (!compiledIn(f.name)) continue;
            const word = f.name;
            if (!first) {
                if (col + 2 + word.len > 72) {
                    out = out ++ ",\n" ++ indent;
                    col = indent.len;
                } else {
                    out = out ++ ", ";
                    col += 2;
                }
            }
            out = out ++ word;
            col += word.len;
            first = false;
        }
        return out ++ "\n";
    }
}

/// The languages an embed archetype's `<lang>` takes — the registry's
/// embeddable formats, the same set `args.embedTypeFromName` loops over.
const embed_langs = blk: {
    var out: []const u8 = "";
    for (@typeInfo(fig.Embed.InnerFormat).@"enum".fields, 0..) |f, i|
        out = out ++ (if (i == 0) "" else ", ") ++ f.name;
    break :blk out;
};

/// The `--embed <archetype>` vocabulary, indented to sit under the flag's own
/// line — the one text every action that takes `--embed` prints, matching
/// `args.embedTypeFromName`.
pub const embed_archetypes =
    "    Parametric families take a <lang> — " ++ embed_langs ++ ":\n" ++
    \\    `md-<lang>` (---<lang> frontmatter; bare `frontmatter` is ---/YAML,
    \\    so there is no `md-yaml`), `fenced-<lang>` (```lang code block),
    \\    `html-script[-<lang>]` (<script type="application/<lang>"> data
    \\    island) and `html-code[-<lang>]` (<pre><code class="language-<lang>">
    \\    visible block); a bare html-script/html-code is fig. Plus the fixed
    \\    presets `frontmatter-json` (;;;), `frontmatter-toml` (+++), and
    \\    `endmatter` (trailing ```endmatter block). Also accepted:
    \\    `frontmatter-yaml`, `semicolons`, `plus`, `endmatter-yaml`, and
    \\    `frontmatter-fig` (the older name of `fenced-fig`).
    \\
;

/// How `edit`/`set`/`insert` read a value argument (`value_arg.zig`).
pub const value_reading =
    \\  <value> is read as a fig value and written in the file's own syntax,
    \\    so it means the same thing in every format: 5, 2.5, true, null and
    \\    2026-09-22 are typed; hello, hello world, Yes and 007 are strings;
    \\    [1, 2] and {{a = 1, b = [x]}} are a sequence and a mapping; '"5"' is
    \\    the string 5. An empty value, one with a line break or with space
    \\    at either end, and one whose # would start a comment are strings
    \\    as written.
    \\  --string: take <value> as a string, whatever it looks like
    \\    (`set f.json version --string 1.10`)
    \\  --raw: splice <value> verbatim as source text in the file's format,
    \\    for what only it can spell (a YAML anchor, a TOML local datetime)
    \\
;

pub const title_string = "\n=========\n   FIG\n=========\n\n";

pub const Help = struct {
    pub fn general(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage:
            \\  {s} <action> [action options] --[flags]
            \\Possible actions:
            \\  help: prints this text (default action)
            \\  version: prints version number
            \\  edit: edits part of file
            \\  set: upsert a value (create the key, embed block, or file, if absent)
            \\  insert: add a new key or list item to a file
            \\  delete: remove a key or list item from a file
            \\  get: print a file or a specific part of a file to stdout
            \\  comment: add or edit a comment on part of a file
            \\  check: validate that one or more files parse cleanly
            \\  fmt: reformat a file in place (house style; gofmt-style)
            \\  convert: convert a file (or a host document's embedded region)
            \\    from one format/archetype to another; prints to stdout, or
            \\    writes the file in place with --write
            \\  patch: merge one document into another, in place and losslessly
            \\  lang: list the languages fig knows, or check a configured one
            \\
            \\Every action takes --lang <name> to read and write a file through
            \\a language configured in languages.figl rather than by extension
            \\(`{s} lang --help`).
            \\
            \\Arguments are read strictly: a word beginning with - that is not one
            \\of the action's flags, or one more argument than the action takes,
            \\is a usage error (exit 2). `--` ends the flags, so a file or value
            \\that begins with - goes after it (`{s} get -- -x.yaml`); `-` alone is
            \\stdin, and a negative number (-5, -2.5) is a value wherever it
            \\stands. fig has no bare -inf or nan (they are strings), so
            \\-inf reads as a flag.
            \\
            \\Exit status, for every action:
            \\  0  done (for `fmt --dry-run`/`--diff` and `check`: nothing to
            \\     change, everything parses)
            \\  1  the operation failed on the document: it does not parse, a
            \\     path or file is missing, an edit was refused, `fmt --dry-run`
            \\     found a change, `--strict` found a warning
            \\  2  the command line is wrong: an unknown action or flag, a missing
            \\     or surplus argument, a path or value that does not parse
            \\
            \\Any other action is handed to a `fig-<action>` program on your PATH,
            \\the way git does: `{s} schema lint f.json` runs `fig-schema lint f.json`
            \\with every argument after `schema` passed through untouched.
            \\
            \\Colour: stdout and stderr are each coloured when they are a
            \\terminal. NO_COLOR (set, to anything) turns colour off;
            \\CLICOLOR_FORCE turns it on where the stream is not a terminal.
            \\NO_COLOR wins when both are set.
            \\
            \\For information on action options, pass --help or -h
            \\to the action you would like to learn about.
            \\
        , .{ binary_name, binary_name, binary_name, binary_name });
        try term.writer.flush();
    }

    pub fn edit(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage: {s} edit [--string | --raw] <file> <path> <value>
            \\       {s} edit --key <file> <path> <name>
            \\  Replaces the value at <path>.
            \\  --key: rename the key at <path> to <name> instead
            \\
        ++ value_reading ++
            \\  path format: dot syntax for keys, bracket syntax for indices
            \\    a key holding a . or [ is quoted or escaped: a."b.c", a.'b.c',
            \\    a["b.c"] (as `-o gron` prints it), or a.b\.c
            \\    example: school.class[0].student[3]
            \\  .md/.markdown files: edits the frontmatter/endmatter in place —
            \\    its archetype (YAML/JSON/TOML/fig frontmatter, fenced ```lang
            \\    frontmatter, YAML endmatter) is sniffed from the file,
            \\    defaulting to YAML when none is found
            \\
        , .{ binary_name, binary_name });
        try term.writer.flush();
    }

    pub fn set(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage: {s} set [--embed <archetype>] [--string | --raw] <file> <path> <value>
            \\       {s} set [--embed <archetype>] [--string | --raw] --seq <file> <path> <item>...
            \\  Upsert: replace the value at <path>, or create it when absent —
            \\    one verb for `edit`+`insert`. Missing parent maps along <path>
            \\    are auto-created (`mkdir -p`); a segment that is an existing
            \\    non-map scalar is a type error and left untouched.
            \\  When <file> itself does not exist, it is CREATED and seeded with
            \\    <path>: <value>; `fig get <file>` then prints what was written.
            \\    The format comes from the extension, so a new file needs a known
            \\    one — .figl (.fig also accepted)/.json/.jsonc/.yaml/.yml/.toml (or a .md host, via
            \\    --embed). .zon/.json5 have no from-scratch seed and must already
            \\    exist.
            \\  --seq: reconcile the sequence at <path> to exactly <item>..., keeping
            \\    the comments on items that survive (only new items are inserted,
            \\    only dropped ones removed; result order matches the arguments).
            \\  --embed <archetype>: target an embedded region of a host file.
            \\
        ++ embed_archetypes ++
            \\    When the host has no such block, it is CREATED (frontmatter at
            \\    the top, endmatter at the bottom) and seeded with <path>:
            \\    <value> — unless one would go at the top of a host that already
            \\    has frontmatter of another kind, which is refused (`{s} convert
            \\    --to-embed` changes it).
            \\
        ++ value_reading ++
            \\    A created key is written in the file's syntax too, so new keys
            \\    work for strict JSON. Each --seq <item> is read the same way.
            \\  path format: dot syntax for keys, bracket syntax for indices
            \\    a key holding a . or [ is quoted or escaped: a."b.c", a.'b.c',
            \\    a["b.c"] (as `-o gron` prints it), or a.b\.c
            \\    example: school.class[0].student[3]
            \\  .md/.markdown files: upserts the frontmatter/endmatter, creating
            \\    it (as YAML) if absent — the archetype is otherwise sniffed
            \\    from the file, not assumed from the extension.
            \\
        , .{ binary_name, binary_name, binary_name });
        try term.writer.flush();
    }

    pub fn insert(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage: {s} insert [--string | --raw] <file> <path> <value>
            \\  Adds a new entry. The last path segment names the slot to create:
            \\    a.b.newkey   -> insert key `newkey` into the mapping at a.b
            \\    a.list[0]    -> prepend <value> as the first item of a.list
            \\    a.list[-]    -> append <value> as the last item ([$] also works)
            \\  An empty parent targets the root container, so the document's own
            \\    root (mapping vs list) decides which form applies — not the format.
            \\  Mid-sequence insert (e.g. list[2]) is not yet supported.
            \\
        ++ value_reading ++
            \\  path format: dot syntax for keys, bracket syntax for indices.
            \\    a key holding a . or [ is quoted or escaped: a."b.c", a.'b.c',
            \\    a["b.c"] (as `-o gron` prints it), or a.b\.c
            \\  .md/.markdown files: edits the frontmatter/endmatter in place —
            \\    its archetype is sniffed from the file (YAML by default).
            \\
        , .{binary_name});
        try term.writer.flush();
    }

    pub fn delete(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage: {s} delete <file> <path>
            \\  Removes the entry the path points at. The last path segment decides:
            \\    a.b.key      -> delete that mapping entry (with its own comments)
            \\    a.list[2]    -> remove item 2 from the sequence a.list
            \\    a.list[-]    -> remove the last item of the sequence a.list
            \\  A path naming a whole [table]/[section]/block container — or one
            \\  [[array-of-tables]] element — removes all of it, header, entries
            \\  and every place it is reopened further down the file.
            \\  path format: dot syntax for keys, bracket syntax for indices
            \\    a key holding a . or [ is quoted or escaped: a."b.c", a.'b.c',
            \\    a["b.c"] (as `-o gron` prints it), or a.b\.c
            \\    example: school.class[0].student[3]
            \\    [-] or [$] in place of an index means "the last item"
            \\  .md/.markdown files: edits the frontmatter/endmatter in place —
            \\    its archetype is sniffed from the file (YAML by default).
            \\
        , .{binary_name});
        try term.writer.flush();
    }

    pub fn comment(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage: {s} comment [--inline] [--delete | --get] <file> <path> [<text>]
            \\  default: add an own-line comment ABOVE the node at <path>
            \\  --inline: target the same-line trailing comment on the value at
            \\    <path> instead (set replaces any existing one on that line)
            \\  --delete: remove the targeted comment instead of adding it; <text>
            \\    is then omitted (a no-op when there is no such comment)
            \\  --get: print the targeted comment to stdout (markers stripped) and
            \\    make no change; <text> is then omitted (exits 1, printing
            \\    nothing, when there is no such comment)
            \\  the comment marker is added for you: # for YAML/TOML, // for
            \\    JSONC/JSON5/ZON. Strict JSON has no comments (rejected).
            \\  <text> may span multiple lines (leading only): one comment line each.
            \\  path format: dot syntax for keys, bracket syntax for indices
            \\    a key holding a . or [ is quoted or escaped: a."b.c", a.'b.c',
            \\    a["b.c"] (as `-o gron` prints it), or a.b\.c
            \\    example: school.class[0].student[3]
            \\  .md/.markdown files: comments the frontmatter/endmatter in
            \\    place — its archetype is sniffed from the file (YAML by
            \\    default).
            \\
        , .{binary_name});
        try term.writer.flush();
    }

    pub fn get(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage: {s} get [--input <format>] [--output <format>] <file> [path]
            \\  -i, --input: input format of file (defaults to the file extension,
            \\    then to sniffing the file's contents if the extension is unknown)
            \\  -o, --output:   output format (defaults to the input format)
            \\  [path]: a scalar there prints as its text and a newline — a string
            \\    unquoted, anything else as fig spells it (42, true, null) — in
            \\    every format, so $(fig get f name) is the value. A container
            \\    prints as a document. With -o, the fragment prints in that
            \\    format's own spelling (-o json: "hi").
            \\  <format> is one of these, or a language configured in
            \\    languages.figl (`{s} lang list`):
            \\
        ++ formatList("    ", &.{}) ++
            \\  canonical: the AST's 1:1 oracle text encoding; usable as input or
            \\    output, e.g. to inspect how any document parses. (Owns no file
            \\    extension — select it explicitly.) Compiled in only with
            \\    `-Dcanonical=true` (opt-in, off by default — it is a
            \\    test/debugging oracle, not exposed through the C ABI or bindings).
            \\  fig: the human-facing authoring dialect (`.figl`; `.fig` still
            \\    accepted); lossy at the edges (non-string keys, YAML refs) —
            \\    use `canonical`/`--lossless` for those. `-o fig` prints in
            \\    house style; use `fig fmt` to
            \\    rewrite a file in place instead of printing to stdout.
            \\  ini: `[section]` headers + `key = value` lines, `;`/`#` full-line
            \\    comments; every value is plain text (no typed scalars). Holds a
            \\    root mapping and one level of section nesting only — a value
            \\    nested any deeper, or an array anywhere, has no INI spelling.
            \\    Editable in place, except that a `[section]` is a name rather
            \\    than a value — `edit`/`delete` at a section's own path refuse,
            \\    since a section owns no contiguous text but its header and may
            \\    be reopened further down the file. Edit the keys inside it.
            \\  dotenv (.env): flat `KEY=value` only, no sections/nesting; keys
            \\    are bash identifiers, an optional `export ` prefix is accepted
            \\    and discarded, and `"`/`'` quoting is real (escapes, multi-line
            \\    values) — no `$VAR` interpolation is performed.
            \\  properties (Java .properties): flat `key=value` only (also
            \\    accepts `key: value`/`key value`); backslash escapes on both
            \\    key and value (`\t \n \r \f \\ \uXXXX`, plus `\` at end-of-line
            \\    as a line continuation); `#`/`!` full-line comments.
            \\  nestedtext (.nt, nestedtext.org): indentation-nested `key: value`/
            \\    `- item`/`> multiline string` lines, arbitrary nesting depth;
            \\    every value is plain text (no typed scalars, like ini). `#`
            \\    full-line comments. Detected from content only as a last
            \\    resort (after every other format, including yaml, since plain
            \\    `key: value`/`- item` text is valid in both) — select it with
            \\    `-i nestedtext` or a `.nt` extension.
            \\  gron: a line-oriented `path = value;` projection (greppable, and
            \\    reversible with `-i gron`); must be selected explicitly, never
            \\    sniffed. Fidelity matches JSON (drops comments/anchors).
            \\  --gron-root NAME: root identifier for `-o gron` (default "json").
            \\  --gron-sep STR: key/value separator for `-o gron` (default " = ").
            \\    Print-only: ungron always splits on " = ", so a custom separator
            \\    is one-way unless it matches the default.
            \\  --gron-term STR: per-line terminator for `-o gron` (default ";");
            \\    pass "" to drop it. ungron strips an optional ";" regardless.
            \\  --compact: single-line output with minimal whitespace (JSON, JSON5, ZON).
            \\  --pretty: multi-line, indented output (the default).
            \\  --indent N: spaces per indent level for pretty JSON, and for TOML's
            \\    wrapped arrays (default 2).
            \\  --width N: TOML column budget (default 80); a mapping/array that fits
            \\    stays inline, a wider one expands to a [section] / wrapped array.
            \\  --strip-comments: drop comments instead of carrying them across formats.
            \\  --lossless: preserve values the target can't represent natively
            \\    (e.g. a null in TOML, a TOML datetime in JSON) via a $fig
            \\    envelope, and reconstruct any such envelope in the input.
            \\    --lossy (the default) emits clean, idiomatic output instead.
            \\  --lax-tags: drop unknown/custom YAML tags instead of erroring, when
            \\    converting away from YAML.
            \\  -q, --quiet, --no-warnings: suppress warnings on stderr — lossy
            \\    conversions, and fig authoring lints (`Yes`-style strings, a
            \\    likely missing comma in a flow value, indent/marker-count
            \\    disagreement, ...).
            \\  --strict: treat any warning as an error (exit 1).
            \\  --embed <archetype>: read an embedded region of a host file.
            \\    Without this flag, a `.md`/`.markdown` file has its archetype
            \\    sniffed from the content (falling back to `frontmatter`/YAML
            \\    when none is found).
            \\
        ++ embed_archetypes ++
            \\  --body: print the host prose OUTSIDE the fences (the body span) instead
            \\    of the embed content; the whole file when there is no such region.
            \\  path format: dot syntax for keys, bracket syntax for indices
            \\    a key holding a . or [ is quoted or escaped: a."b.c", a.'b.c',
            \\    a["b.c"] (as `-o gron` prints it), or a.b\.c
            \\    example: school.class[0].student[3]
            \\  .md/.markdown files: reads the frontmatter/endmatter, whichever
            \\    archetype it turns out to be
            \\
        , .{ binary_name, binary_name });
        try term.writer.flush();
    }

    pub fn check(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage: {s} check [--input <format>] [-q|--quiet] <file>...
            \\  Validate that each file parses cleanly as its format. Prints an
            \\  `ok` line per file and exits 0 when all parse; prints an error
            \\  line to stderr for each failing file and exits 1 if any fail.
            \\  -i, --input: parse every file as this format — one of these, or a
            \\    language configured in languages.figl (`{s} lang list`):
            \\
        ++ formatList("    ", &.{}) ++
            \\    Default: infer from each file's extension, then by sniffing
            \\    its contents.
            \\  -s, --spec: validate against a specific language version, where one
            \\    is selectable: TOML `1.0`/`1.1` (default 1.1), YAML `1.2.2`/`1.1`
            \\    (default 1.2.2).
            \\    JSON strictness is the format itself (json vs jsonc vs json5).
            \\  -q, --quiet, --no-warnings: suppress the per-file `ok` lines and
            \\    fig authoring warnings; errors still print.
            \\  reads stdin when <file> is `-`.
            \\  .md/.markdown files: validates the frontmatter/endmatter,
            \\    whichever archetype it turns out to be (YAML by default).
            \\
        , .{ binary_name, binary_name });
        try term.writer.flush();
    }

    pub fn fmt(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage: {s} fmt [--input <format>] [--dry-run | --diff] <file>
            \\  Reformat a file in place: parse then re-emit in the format's house
            \\  style — `.figl`'s printer applies the style DESIGN.md describes
            \\  (spaced marker runs, `[]`/`+` list sigils, ...); other formats get
            \\  their own printer's canonical layout. Unlike `get`, the output
            \\  format always matches the input — reformatting never converts.
            \\  A file already in house style is left byte-identical (no-op write).
            \\  --dry-run: print the reformatted result to stdout instead of
            \\    writing it back; exit 1 if reformatting would change the file,
            \\    0 if it's already clean — a check gate for pre-commit/CI.
            \\  --diff: like --dry-run, but print a unified diff of the change to
            \\    stdout instead of the whole reformatted file; nothing is printed
            \\    (and exit is 0) when the file is already clean.
            \\  -i, --input: input format (defaults to the file extension, then
            \\    to sniffing the file's contents if the extension is unknown) —
            \\    one of these, or a language configured in languages.figl:
            \\
        ++ formatList("    ", &.{"gron"}) ++
            \\  --compact: single-line output with minimal whitespace (JSON, JSON5, ZON).
            \\  --pretty: multi-line, indented output (the default).
            \\  --indent N: spaces per indent level for pretty JSON, and for TOML's
            \\    wrapped arrays (default 2).
            \\  --width N: TOML column budget (default 80); a mapping/array that fits
            \\    stays inline, a wider one expands to a [section] / wrapped array.
            \\  --strip-comments: drop comments instead of re-emitting them.
            \\  -q, --quiet, --no-warnings: suppress warnings on stderr.
            \\  --strict: treat any warning as an error (exit 1, no write).
            \\  --embed <archetype>: reformat an embedded region of a host file
            \\    instead of the whole file. Without this flag, a `.md`/`.markdown`
            \\    file has its archetype sniffed from the content (falling back to
            \\    `frontmatter`/YAML when none is found).
            \\
        ++ embed_archetypes ++
            \\  reads stdin when <file> is `-`, but only with --dry-run/--diff:
            \\    there is nowhere to write an in-place result back to.
            \\  .md/.markdown files: reformats the frontmatter/endmatter in
            \\    place, whichever archetype it turns out to be.
            \\
        , .{binary_name});
        try term.writer.flush();
    }

    pub fn convert(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage: {s} convert --output <format> [--input <format>] [--write | --diff] <file>
            \\       {s} convert --to-embed <archetype> [--embed <archetype>] [--write | --diff] <file>
            \\  Convert a file — `fmt`'s twin for when the target format differs
            \\  from the source. Exactly one of --output/--to-embed picks the
            \\  target; the other flag group is unused (rejected together).
            \\  Like `get`, it prints the converted result to stdout by default;
            \\  pass --write to write it back to <file> in place instead.
            \\
            \\  Whole-file mode (--output): parse the whole file as --input (else the
            \\    extension, else sniffed from its contents) and re-emit it as
            \\    --output, in the target format's house style. A host document
            \\    whose extension implies an embedded region (`.md`/`.markdown`) is
            \\    rejected here — use embed-archetype mode, or pass --input to force
            \\    whole-file conversion anyway.
            \\  -i, --input, -o, --output: one of these, or a language configured
            \\    in languages.figl:
            \\
        ++ formatList("    ", &.{"gron"}) ++
            \\
            \\  Embed-archetype mode (--to-embed): rehouse a host document's
            \\    embedded region from one archetype's fence-and-content convention
            \\    to another's — e.g. turn YAML frontmatter into JSON frontmatter —
            \\    splicing the new fences and re-serialized content in place while
            \\    leaving the surrounding prose byte-identical. The source archetype
            \\    is --embed, else sniffed from the file's own fences (falling back
            \\    to frontmatter/YAML when none is found).
            \\  --embed, --to-embed <archetype>:
            \\
        ++ embed_archetypes ++
            \\
            \\  -w, --write: write the converted result back to <file> in place
            \\    (skipped if it's already byte-identical) instead of printing it.
            \\  --diff: print a unified diff of the change instead of the whole
            \\    converted file. Combine with --write to write AND see what changed.
            \\  --compact / --pretty: single-line vs multi-line output (default pretty).
            \\  --indent N / --width N: as in `get`/`fmt`.
            \\  --strip-comments: drop comments instead of carrying them across formats.
            \\  --lossless / --lossy: preserve values the target can't represent
            \\    natively via a $fig envelope (default --lossy).
            \\  --lax-tags: drop unknown/custom YAML tags instead of erroring, when
            \\    converting away from YAML.
            \\  -q, --quiet, --no-warnings: suppress warnings on stderr.
            \\  --strict: treat any warning as an error (exit 1, no write).
            \\  reads stdin when <file> is `-`, but only without --write.
            \\
        , .{ binary_name, binary_name });
        try term.writer.flush();
    }

    pub fn patch(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage: {s} patch [flags] <file> <patch-file>
            \\  Merge <patch-file> into <file>, in place. Every byte of <file>
            \\  outside the paths the patch actually names is left untouched —
            \\  comments, key order, quoting style and all. The two files need not
            \\  share a format: a TOML patch merges into a YAML target, rendered in
            \\  the target's syntax on the way in.
            \\
            \\  The merge rule, per path:
            \\    absent in <file>          -> created, with the patch's own comments
            \\    mapping on both sides     -> merged key by key, recursively
            \\    sequence on both sides    -> --seq decides
            \\    anything else             -> the patch's value wins
            \\  A value the two files already agree on is not rewritten at all, so
            \\  re-running a patch is a no-op and the diff is only what changed.
            \\  A container the patch DOES change is re-rendered by the target
            \\  format's printer, so it comes back in that format's house style
            \\  (in YAML, a block collection) rather than the patch's spelling.
            \\
            \\  <file> must already exist — `patch` merges into a document, it does
            \\  not create one; use `set` for that. `-` reads stdin for either file
            \\  (not both), and stdin as <file> needs --dry-run or --diff.
            \\
            \\  --at <path>: merge into that path in <file> instead of its root.
            \\  --from <path>: take only that subtree of <patch-file>.
            \\    Together they move one part of one file into another part of
            \\    another: `fig patch app.yaml defaults.toml --from db --at service.db`
            \\  --delete <path>: remove a path from <file> after the merge; repeatable.
            \\    A path that isn't there is not an error.
            \\  --seq replace|append|union: what to do when both files hold a
            \\    sequence at the same path. `replace` (default) takes the patch's;
            \\    `append` adds every item; `union` adds only the items <file> does
            \\    not already hold, compared by value rather than by source text.
            \\  --comments ours|theirs|none: whose comment wins where both files
            \\    carry one in the same position. `ours` (default) keeps <file>'s and
            \\    contributes the patch's only where <file> has none; `theirs` lets
            \\    the patch overwrite; `none` carries no comment from the patch at
            \\    all. Comments nested inside a subtree the patch contributes whole
            \\    always ride along with it unless `none`.
            \\  -i, --input <format>: the format of <file> (else its extension, else
            \\    sniffed). --patch-input does the same for <patch-file>.
            \\  --embed <archetype>: patch the embedded region of a host <file>
            \\    (e.g. a markdown post's frontmatter), splicing the result back
            \\    between its fences and leaving the prose byte-identical.
            \\    --patch-embed does the same for <patch-file>. A `.md`/`.markdown`
            \\    extension implies the sniff on either side without the flag.
            \\  --lossless / --lossy: preserve values the target format can't hold
            \\    natively via a $fig envelope (default --lossy).
            \\  --indent N / --width N / --compact / --pretty: style knobs for the
            \\    values the patch contributes, as in `get`/`convert`.
            \\  --dry-run: print the patched document to stdout; write nothing.
            \\  --diff: print a unified diff of the change; write nothing. Neither
            \\    preview mode sets a non-zero exit status — a patch is expected to
            \\    change the file.
            \\  -q, --quiet: suppress the summary line and the dropped-comment note.
            \\
            \\  Refused rather than guessed at: merging into a YAML `*alias` (the
            \\  reference belongs to whatever defined the anchor), and replacing the
            \\  whole document root with a non-mapping (that is a copy, not a patch).
            \\
        , .{binary_name});
        try term.writer.flush();
    }
    pub fn lang(term: *Io.Terminal, binary_name: []const u8) !void {
        try term.writer.print(
            \\Usage: {s} lang list
            \\       {s} lang check <name> [--against <format>] [files...]
            \\       {s} lang table <file> [-i <format>] [--spec <version>]
            \\  Languages fig did not compile in. One is a helper program that
            \\  speaks the wire in fig's Rust crate (`fig::helper`), configured in
            \\  a `languages.figl`:
            \\
            \\    language[]
            \\    > name = lua-dotenv
            \\    > extensions = [env]
            \\    > command = [fig-lua, ~/.config/fig/languages/dotenv.lua]
            \\
            \\  found, earlier file winning a name, at $FIG_LANGUAGES (a file path),
            \\  .fig/languages.figl in the working directory or any ancestor, and
            \\  $XDG_CONFIG_HOME/fig/languages.figl (~/.config/fig/languages.figl).
            \\  `name` is what --input, --output and --lang accept; `extensions`
            \\  resolves a file to it, but a compiled format's extension always
            \\  wins, so a twin of a compiled language is reached with --lang.
            \\
            \\  Nothing is spawned until a name or extension the CLI cannot resolve
            \\  itself is asked for; the helper is then started once and asked to
            \\  parse its own samples, print and reparse them, and take a no-op
            \\  edit, and is refused with the reason if any of that fails.
            \\
            \\  list: every compiled format, and every configured language with
            \\    what it can do (read, edit, serialize), its extensions and the
            \\    file that configured it — or why it was refused.
            \\  check <name>: load the language and report what the harness found.
            \\    --against <format>: also hold it to a compiled format — parse each
            \\    of the language's samples and each file given with both, and
            \\    compare the tables row for row, the way a reimplementation of a
            \\    compiled language is proven (`{s} lang check lua-dotenv --against
            \\    dotenv secrets.env`). Exits 1 on the first difference.
            \\  table <file>: parse the file (as -i <format>, else by its extension,
            \\    else by its contents) and print its node table as the JSON a
            \\    helper would answer `parse` with — what a twin has to produce,
            \\    row for row, to pass `check --against`. --spec selects a version
            \\    of the format where one is selectable, as `check`'s does.
            \\
            \\  --lang <name>, on any action, names the language a file is read and
            \\  written in, whatever its extension: `{s} get secrets.env --lang lua-dotenv`.
            \\
        , .{ binary_name, binary_name, binary_name, binary_name, binary_name });
        try term.writer.flush();
    }
};

test "every format the help lists is one the argument parser accepts" {
    const args = @import("args.zig");
    inline for (comptime .{ formatList("", &.{}), formatList("", &.{"gron"}) }) |list| {
        var it = std.mem.tokenizeAny(u8, list, ", \n");
        while (it.next()) |name| try std.testing.expect(args.parseFormatName(name) != null);
    }
}

test "every --embed archetype the help names is one the argument parser accepts" {
    const args = @import("args.zig");
    var langs = std.mem.tokenizeAny(u8, embed_langs, ", ");
    while (langs.next()) |l| {
        var buf: [64]u8 = undefined;
        for ([_][]const u8{ "md-", "fenced-", "html-script-", "html-code-" }) |family| {
            const name = try std.fmt.bufPrint(&buf, "{s}{s}", .{ family, l });
            // `md-yaml` is deliberately not a spelling, and the help says so.
            if (std.mem.eql(u8, name, "md-yaml")) {
                try std.testing.expect(args.embedTypeFromName(name) == null);
                continue;
            }
            try std.testing.expect(args.embedTypeFromName(name) != null);
        }
    }
    for ([_][]const u8{
        "frontmatter",      "frontmatter-json", "frontmatter-toml", "endmatter",
        "html-script",      "html-code",        "frontmatter-yaml", "semicolons",
        "plus",             "endmatter-yaml",   "frontmatter-fig",
    }) |name| try std.testing.expect(args.embedTypeFromName(name) != null);
}
