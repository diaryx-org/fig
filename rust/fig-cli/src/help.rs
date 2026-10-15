//! Port of src/cli/help.zig, as the prose clap shows.
//!
//! clap writes fig's usage lines and `--help` pages from the command line
//! `args.rs` declares, and each flag's explanation is its doc comment there.
//! What is here is the prose no one flag owns: fig's rules for the whole
//! command line (`GENERAL`), the value and path syntax several actions
//! share, and each action's longer notes — plus the list of format names
//! `--input`/`--output` take, which is derived (`format_list`), so every
//! action's help says what the argument parser accepts.

use clap::CommandFactory;
use fig_core::Result;

use crate::args::Cli;
use crate::fileio::Terminal;
use crate::languages;

/// The format names this build knows by itself: its bundled dialects, with
/// `canonical` (when built with the `canonical` feature) after `zon` and
/// `gron` after `figl`, where the CLI's format list has always put them.
fn builtin_formats() -> Vec<String> {
    let mut out = Vec::new();
    for n in languages::dialect_names() {
        let figl = n == "figl";
        let zon = n == "zon";
        out.push(n);
        if zon && crate::parse_dispatch::CANONICAL_ENABLED {
            out.push("canonical".to_string());
        }
        if figl {
            out.push("gron".to_string());
        }
    }
    out
}

/// The format names `parse_format_name` accepts in this build by itself —
/// `builtin_formats`, less `except` (gron for an action that writes a stored
/// document) — comma-separated and wrapped under `indent` to fit 72 columns.
/// Derived from the bundle, so a new format is listed everywhere at once.
pub fn format_list(indent: &str, except: &[&str]) -> String {
    let mut out = indent.to_string();
    let mut col = indent.len();
    let mut first = true;
    for f_name in builtin_formats() {
        if except.contains(&f_name.as_str()) {
            continue;
        }
        let word = f_name;
        if !first {
            if col + 2 + word.len() > 72 {
                out.push_str(",\n");
                out.push_str(indent);
                col = indent.len();
            } else {
                out.push_str(", ");
                col += 2;
            }
        }
        out.push_str(&word);
        col += word.len();
        first = false;
    }
    out.push('\n');
    out
}

/// What `--input`/`--output` take, for the end of an action's `--help`:
/// the formats this build knows (less `except`), or a configured language.
pub fn formats(except: &[&str]) -> String {
    format!(
        "<FORMAT> is one of these, or a language configured in languages.figl (`fig lang list`):\n{}",
        format_list("  ", except)
    )
}

/// What fig is for, in one line: `fig help --oneline`, for a list of tools
/// where each gets a line and the rest is a `fig help` away.
pub const ONELINE: &str = "fig: read and edit JSON, YAML, TOML, INI, dotenv, plist and other config files by path, keeping comments and formatting. More: `fig help`.\n";

pub const TITLE_STRING: &str = "\n=========\n   FIG\n=========\n\n";

/// The line above fig's action list.
pub const ABOUT: &str = "Read and edit JSON, YAML, TOML, INI, dotenv, plist and other config files by path, keeping comments and formatting";

/// fig's rules for every action, below the action list in `fig help`.
pub const GENERAL: &str = "\
Every action takes --lang <name> to read and write a file through
a language configured in languages.figl rather than by extension
(`fig lang --help`).

Arguments are read strictly: a word beginning with - that is not one
of the action's flags, or one more argument than the action takes,
is a usage error (exit 2). `--` ends the flags, so a file or value
that begins with - goes after it (`fig get -- -x.yaml`); `-` alone is
stdin, and a negative number (-5, -2.5, -1e5) is a value wherever it
stands. Any other value that begins with - goes after `--`: fig has
no bare -inf or nan (they are strings), so -inf reads as a flag.

Paths: dots for keys, brackets for indices (`a.list[0].name`), and
[-] for the last item. Quote a path that holds brackets, since a
shell may read `[...]` as a glob. `fig get <file> -o gron` prints
every path in a file with its value.

Edits are made in place, and only the edited span changes: comments,
key order, quoting and blank lines elsewhere stay as they were. Only
`patch` previews (`--dry-run`, `--diff`); for the other edits, `get`
the value first or diff the file afterward.

Exit status, for every action:
  0  done (for `fmt --dry-run`/`--diff` and `check`: nothing to
     change, everything parses)
  1  the operation failed on the document: it does not parse, a
     path or file is missing, an edit was refused, `fmt --dry-run`
     found a change, `--strict` found a warning
  2  the command line is wrong: an unknown action or flag, a missing
     or surplus argument, a path or value that does not parse

Any other action is handed to a `fig-<action>` program on your PATH,
the way git does: `fig schema lint f.json` runs `fig-schema lint f.json`
with every argument after `schema` passed through untouched.

Colour: stdout and stderr are each coloured when they are a
terminal. NO_COLOR (set, to anything) turns colour off;
CLICOLOR_FORCE turns it on where the stream is not a terminal.
NO_COLOR wins when both are set.

For an action's options, pass --help (or -h, for a summary) to it:
`fig set --help`.";

/// How `replace`/`set`/`insert` read a value argument (`value_arg.rs`);
/// `--string` and `--raw` are `args::ValueReading`'s.
pub const VALUE: &str = "\
The value, read as a figl value and written in the file's own syntax,
so it means the same thing in every format: 5, 2.5, true, null and
2026-09-22 are typed; hello, hello world, Yes and 007 are strings;
[1, 2] and {a = 1, b = [x]} are a sequence and a mapping; '\"5\"' is
the string 5. An empty value, one with a line break or with space
at either end, and one whose # would start a comment are strings
as written. --string and --raw read it otherwise.";

/// The <path> syntax the editing actions share.
pub const PATH: &str = "\
Where in the file: dot syntax for keys, bracket syntax for indices.
A key holding a . or [ is quoted or escaped: a.\"b.c\", a.'b.c',
a[\"b.c\"] (as `-o gron` prints it), or a.b\\.c.
Example: school.class[0].student[3].
[-] or [$] in place of an index means \"the last item\".";

/// `set`'s <path>: `PATH`, where a final `[-]` appends instead.
pub const SET_PATH: &str = "\
Where in the file: dot syntax for keys, bracket syntax for indices.
A key holding a . or [ is quoted or escaped: a.\"b.c\", a.'b.c',
a[\"b.c\"] (as `-o gron` prints it), or a.b\\.c.
Example: school.class[0].student[3].
[-] or [$] before the end of the path means \"the last item\"; at
the end, it appends (see above).";

/// `insert`'s <path>: the slot to create, whose last segment decides.
pub const INSERT_PATH: &str = "\
The slot to create, in dot syntax for keys and bracket syntax for
indices; the last segment names it (see above). A key holding a .
or [ is quoted or escaped: a.\"b.c\", a.'b.c', a[\"b.c\"] (as `-o gron`
prints it), or a.b\\.c.";

/// `get`'s notes on the formats it reads and prints.
pub fn get_formats() -> String {
    let mut text = formats(&[]);
    text.push_str(
        "
canonical: the AST's 1:1 oracle text encoding; usable as input or
  output, e.g. to inspect how any document parses. (Owns no file
  extension — select it explicitly.) Compiled in only with
  `-Dcanonical=true` (opt-in, off by default — it is a
  test/debugging oracle, not exposed through the C ABI or bindings).
figl: the human-facing authoring dialect (`.figl`); lossy at the
  edges (non-string keys, YAML refs) —
  use `canonical`/`--lossless` for those. `-o figl` prints in
  house style; use `fig fmt` to
  rewrite a file in place instead of printing to stdout.
ini: `[section]` headers + `key = value` lines, `;`/`#` full-line
  comments; every value is plain text (no typed scalars). Holds a
  root mapping and one level of section nesting only — a value
  nested any deeper, or an array anywhere, has no INI spelling.
  Editable in place, except that a `[section]` is a name rather
  than a value — `replace`/`delete` at a section's own path refuse,
  since a section owns no contiguous text but its header and may
  be reopened further down the file. Edit the keys inside it.
dotenv (.env): flat `KEY=value` only, no sections/nesting; keys
  are bash identifiers, an optional `export ` prefix is accepted
  and discarded, and `\"`/`'` quoting is real (escapes, multi-line
  values) — no `$VAR` interpolation is performed.
properties (Java .properties): flat `key=value` only (also
  accepts `key: value`/`key value`); backslash escapes on both
  key and value (`\\t \\n \\r \\f \\\\ \\uXXXX`, plus `\\` at end-of-line
  as a line continuation); `#`/`!` full-line comments.
nestedtext (.nt, nestedtext.org): indentation-nested `key: value`/
  `- item`/`> multiline string` lines, arbitrary nesting depth;
  every value is plain text (no typed scalars, like ini). `#`
  full-line comments. Detected from content only as a last
  resort (after every other format, including yaml, since plain
  `key: value`/`- item` text is valid in both) — select it with
  `-i nestedtext` or a `.nt` extension.
gron: a line-oriented `path = value;` projection (greppable, and
  reversible with `-i gron`); must be selected explicitly, never
  sniffed. Fidelity matches JSON (drops comments/anchors).",
    );
    text
}

/// `patch`'s merge rules, below its flags.
pub const PATCH_RULES: &str = "\
The merge rule, per path:
  absent in <FILE>          -> created, with the patch's own comments
  mapping on both sides     -> merged key by key, recursively
  sequence on both sides    -> --seq decides
  anything else             -> the patch's value wins
A value the two files already agree on is not rewritten at all, so
re-running a patch is a no-op and the diff is only what changed.
Agreeing is by value in <FILE>'s own reading, not by spelling: a
patch's 31 leaves <FILE>'s 0x1F alone, 1.5 its 1.50, \"x\" its 'x'.
An integer never agrees with a float, so 1.0 does replace a 1.
A container the patch DOES change is re-rendered by the target
format's printer, so it comes back in that format's house style
(in YAML, a block collection) rather than the patch's spelling.

--indent, --width, --compact and --pretty style the values the patch
contributes, as in `get`/`convert`.

Refused rather than guessed at: merging into a YAML `*alias` (the
reference belongs to whatever defined the anchor), and replacing the
whole document root with a non-mapping (that is a copy, not a patch).";

/// `lang`'s account of the languages fig does not build in.
pub const LANG: &str = "\
Languages fig does not build in. One is a helper program that
speaks the wire in fig's Rust crate (`fig::helper`), configured in
a `languages.figl`:

  language[]
  > name = lua-dotenv
  > extensions = [env]
  > command = [fig-lua, ~/.config/fig/languages/dotenv.lua]

found, earlier file winning a name, at $FIG_LANGUAGES (a file path),
.fig/languages.figl in the working directory or any ancestor, and
$XDG_CONFIG_HOME/fig/languages.figl (~/.config/fig/languages.figl).
`name` is what --input, --output and --lang accept; `extensions`
resolves a file to it, but a built-in language's extension always
wins, so a twin of a built-in language is reached with --lang.
A language named after a format this build leaves out (`list` says
\"not built in\") is that format: its name and extension reach it.

Nothing is spawned until a name or extension the CLI cannot resolve
itself is asked for; the helper is then started once and asked to
parse its own samples, print and reparse them, and take a no-op
edit, and is refused with the reason if any of that fails.

With no verb, `lang` lists.";

/// `lang`'s closing note on `--lang`.
pub const LANG_AFTER: &str = "\
--lang <name>, on any action, names the language a file is read and
written in, whatever its extension: `fig get secrets.env --lang lua-dotenv`.";

/// fig's own help: the action list with `GENERAL` below it — `fig help`,
/// `fig`, and the end of an unknown action's report.
pub fn general(term: &mut Terminal, binary_name: &str) -> Result<()> {
    let mut cmd = Cli::command().bin_name(binary_name.to_string());
    let text = cmd.render_help().to_string();
    term.write_str(&text)?;
    term.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_the_help_lists_is_one_the_argument_parser_accepts() {
        crate::languages::init();
        for list in [format_list("", &[]), format_list("", &["gron"])] {
            for name in list.split([',', ' ', '\n']).filter(|s| !s.is_empty()) {
                assert!(crate::args::parse_format_name(name).is_some(), "{name}");
            }
        }
    }
}
