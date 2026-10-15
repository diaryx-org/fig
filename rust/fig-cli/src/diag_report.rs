//! Port of src/cli/diag_report.zig.
//!
//! CLI-only teaching-style diagnostic rendering: cargo/rustc-shaped
//! `file:line:col` reports with a source-line gutter and a colored
//! underline, plus the shared `--quiet`/`--strict` warning contract every
//! action's parse path uses. This is a presentation layer over the
//! language-agnostic `ParseDiagnostic` shape — it never knows about a
//! specific language's own diagnostic types.

use fig_core::kit::ast::NodeId;
use fig_core::kit::document::Document;
use fig_core::kit::language::ParseDiagnostic;
use fig_core::kit::parse_diagnostic::locate_offset;
use fig_core::kit::span::Span;
use fig_core::registry;
use fig_core::{Error, Result};

use crate::fileio::{self, Color, Terminal};
use crate::types::{self, EditTextKind, Format, PathSeg};

/// Print a teaching report straight to `term`, cargo/rustc-style:
/// ```text
///   <label>: <message>
///   --> <file>:<line>:<col>
///    |
///   7 | <source line>
///    |          ~~~~ <short_label>
/// ```
/// highlighting the reported `[offset, end)` span (a `~~~~` underline, or a
/// single `^` when `end` is `None` or the span is one byte), coloring the label
/// word and the highlight+`short_label` in `color`, and the `-->` pointer plus
/// the `N |` gutter in blue. Language-agnostic (every field is plain data), so
/// every covered language renders through this one function.
///
/// Deliberately never buffered into an intermediate string: colour is set
/// live against the real terminal.
#[allow(clippy::too_many_arguments)]
pub fn print_diag(
    term: &mut Terminal,
    source: &[u8],
    file: &str,
    offset: usize,
    end: Option<usize>,
    label: &str,
    color: Color,
    message: &[u8],
    short_label: &[u8],
) -> Result<()> {
    let loc = locate_offset(source, offset);
    term.set_color(color)?;
    term.write_str(label)?;
    term.set_color(Color::Reset)?;
    term.write_str(": ")?;
    term.write_all(message)?;
    term.write_str("\n")?;
    term.set_color(Color::Blue)?;
    term.write_str("--> ")?;
    term.set_color(Color::Reset)?;
    term.print(format_args!("{}:{}:{}\n", file, loc.line, loc.column))?;

    // Mirrors `renderReport`'s source-line + caret, but in the cargo/rustc
    // gutter shape: a blank `|` line, the numbered source line, then a
    // highlight line under the offending span carrying `short_label`. Capped
    // so a pathological line can't flood the terminal; the highlight mirrors
    // tabs in the source to stay aligned under them. The gutter's width
    // tracks the line number's digit count so the blank/highlight `|` lines
    // up under the source line's `|`.
    const MAX_SHOWN: usize = 160;
    let shown = &loc.line_text[..loc.line_text.len().min(MAX_SHOWN)];
    if shown.is_empty() {
        return Ok(()); // EOF/blank line: nothing to point into
    }

    let line_num = format!("{}", loc.line);

    term.set_color(Color::Blue)?;
    term.splat_byte_all(b' ', line_num.len())?;
    term.write_str(" |\n")?;
    term.print(format_args!("{line_num} | "))?;
    term.set_color(Color::Reset)?;
    term.write_all(shown)?;
    if shown.len() < loc.line_text.len() {
        term.write_str("…")?;
    }
    term.write_str("\n")?;

    if loc.column - 1 <= shown.len() {
        term.set_color(Color::Blue)?;
        term.splat_byte_all(b' ', line_num.len())?;
        term.write_str(" | ")?;
        term.set_color(Color::Reset)?;
        for &c in &shown[..loc.column - 1] {
            term.write_byte(if c == b'\t' { b'\t' } else { b' ' })?;
        }
        term.set_color(color)?;
        // Highlight the reported `[offset, end)` span rather than a single
        // point: a `~~~~` underline when the parser gave a real multi-byte
        // extent (`end`), a single `^` when it didn't (fall back to "just the
        // start") or when the span is exactly one byte — matching how a `^`
        // and a `~~~~` read identically for a one-character span anyway.
        // Never runs past the portion of the line actually printed above.
        let span_len = match end {
            Some(e) => {
                if e > offset {
                    e - offset
                } else {
                    1
                }
            }
            None => 1,
        };
        let draw_len = 1.max(span_len.min(shown.len() - (loc.column - 1)));
        if draw_len <= 1 {
            term.write_str("^")?;
        } else {
            term.splat_byte_all(b'~', draw_len)?;
        }
        term.write_str(" ")?;
        term.write_all(short_label)?;
        term.write_str("\n")?;
        term.set_color(Color::Reset)?;
    }
    Ok(())
}

/// Render one parse failure as a `print_diag` teaching report and exit(1) — the
/// `get`-time twin of `check`'s per-error loop, for the single diagnostic a
/// non-recovering parse produces.
pub fn report_parse_error(
    term: &mut Terminal,
    source: &[u8],
    file: &str,
    offset: usize,
    end: Option<usize>,
    message: &[u8],
    short_label: &[u8],
) -> Result<()> {
    print_diag(term, source, file, offset, end, "error", Color::Red, message, short_label)?;
    term.flush()?;
    // A file that does not parse is a failure on the document, not a wrong
    // command line: exit 1, as `check` and the editing actions always did.
    fileio::exit(1);
}

/// The binary's last line of defense: report an error that reached `main`
/// without any handler of its own, then exit(1). Named errors that a user can
/// actually act on get a sentence; the rest print their name. The
/// overwhelmingly common cause, a file that does not parse, never reaches
/// here in a format with a located report: `main` re-parses the target the
/// way `check` does and prints that report instead.
pub fn report_unhandled(term: &mut Terminal, err: Error, file: Option<&str>, binary_name: &str) -> ! {
    let _ = report_unhandled_impl(term, err, file, binary_name);
    let _ = term.flush();
    fileio::exit(1);
}

fn help_line(term: &mut Terminal, word: &str, text: &str) -> Result<()> {
    term.set_color(Color::Blue)?;
    term.write_str(word)?;
    term.set_color(Color::Reset)?;
    term.write_str(text)
}

fn report_unhandled_impl(term: &mut Terminal, err: Error, file: Option<&str>, binary_name: &str) -> Result<()> {
    term.set_color(Color::Red)?;
    term.write_str("error")?;
    term.set_color(Color::Reset)?;
    let file = file.unwrap_or("(unknown)");
    match err.name() {
        "FileNotFound" => term.print(format_args!(": no such file: {file}\n"))?,
        "AccessDenied" => term.print(format_args!(": permission denied: {file}\n"))?,
        "IsDir" => term.print(format_args!(": {file} is a directory\n"))?,
        // The editor's navigation failures — the path, not the document.
        "NotFound" => term.write_str(": no such path in the document (`get` it to see what's there)\n")?,
        "NotAMapping" => term.write_str(": a segment of this path is not a mapping, so it has no keys to address\n")?,
        "NotASequence" => term.write_str(": a segment of this path is not a sequence, so it has no indices to address\n")?,
        "IndexOutOfBounds" => term.write_str(": that index is past the end of the sequence\n")?,
        "ContainerClosesOnItsLine" => {
            term.write_str(": that container closes on the line of its last entry or item, so a new one has no line of its own to go on\n")?;
            help_line(term, "help", ": put the container's close on a line of its own first; an entry appended after the line would land in the container around it.\n")?;
        }
        // The engine's section refusal on a value replace (TOML tables, INI
        // sections, fig block containers). Worth a sentence of its own: the
        // raw error name reads like a limitation of the tool, when what it
        // means is that the path names a header rather than a value.
        "CannotReplaceSection" => {
            term.write_str(": that path names a whole block table/section, which has no single value to replace\n")?;
            help_line(term, "note", ": a `[table]`/`[section]` owns nothing contiguous but the name in its header — its entries are separate lines, and it may be reopened further down the file. Replacing it as one value would rewrite that NAME and leave the entries where they are.\n")?;
            help_line(term, "help", &format!(": edit the keys inside it instead — `{binary_name} set <file> <path>.<key> <value>`.\n"))?;
        }
        // The same refusal on an op that removes lines — `comment` taking a
        // node out. `delete` itself never reaches here: it hands a section to
        // the whole-container delete.
        "CannotDeleteSection" => {
            term.write_str(": that path names a whole block table/section, whose entries are lines this op cannot take out one by one\n")?;
            help_line(term, "help", &format!(": to remove it whole — header, entries, and every place it is reopened — `{binary_name} delete <file> <path>`.\n"))?;
        }
        // `insert` naming a key the mapping already holds. The engine
        // refuses it rather than write a second entry of that name.
        "DuplicateKey" => {
            term.write_str(": that key already exists, and `insert` only adds new ones\n")?;
            help_line(term, "help", &format!(": to change its value — `{binary_name} set <file> <path> <value>` (or `{binary_name} replace`, which only replaces).\n"))?;
        }
        // `rename` onto a name another entry of the mapping holds: the
        // engine refuses it rather than write a second entry of that name.
        "RenameTargetExists" => {
            term.write_str(": the mapping already has a key of that name, and a rename would give it two\n")?;
            help_line(term, "help", &format!(": `{binary_name} delete <file> <path>` the other one first, or pick another name.\n"))?;
        }
        // `rename` at a path that ends in an index, or the empty path.
        "NotAKey" => term.write_str(": that path names a sequence item or the root, which has no key to rename\n")?,
        // An insert under a section that is only ever named as the prefix
        // of its children's headers — TOML's implicit `a` made by `[a.b]`.
        "ImplicitSection" => {
            term.write_str(": that section has no header line of its own — it is only named as part of its children's headers — so there is no line to put the new entry under\n")?;
            help_line(term, "note", ": writing it after the first of those headers would put it in that child instead.\n")?;
            help_line(term, "help", ": give the section a header of its own first — `[a]` for TOML's `[a.b]` — and try again.\n")?;
        }
        // `fig comment` on an element or entry of a one-line flow collection.
        "CommentsUnanchored" => {
            term.write_str(": that path names an item of a one-line `[...]`/`{...}`, which shares its parent's line and so owns no comment of its own\n")?;
            help_line(term, "note", ": a comment written inside a flow collection is discarded when the file is read back, so there is nowhere on that line to put one that would survive — and the line above it, and its end, belong to the key the collection is the value of.\n")?;
            help_line(term, "help", &format!(": comment the whole collection instead — `{binary_name} comment <file> <path-without-the-index> <text>` — or rewrite it with one item per line, where each item does own its line.\n"))?;
        }
        // `--seq` (`Editor.set_sequence`) is the only caller that reaches here.
        "UnsupportedShape" => {
            term.write_str(": this is not a flat list of scalars, so `--seq` cannot diff it\n")?;
            help_line(term, "note", ": `--seq` pairs new items with current ones by value, so both lists must be non-empty and every item on each side must be a scalar that parses on its own. TOML never qualifies — its scalars cannot stand alone as a document.\n")?;
            help_line(term, "help", &format!(": replace the whole list instead — `{binary_name} set <file> <path> '[...]'`. On TOML that loses nothing, as its arrays carry no per-item comments.\n"))?;
        }
        // A runtime language's renderer declined the text it was handed. The
        // language said why, in its own words.
        "RendererRefused" => {
            term.write_str(": the language declined to spell this edit: ")?;
            term.write_all(&registry::last_refusal())?;
            term.write_str("\n")?;
        }
        // The editor's uncomment refusal.
        "CommentNotAnEntry" => {
            term.write_str(": those comment lines do not come back as an entry — uncommenting them changed nodes elsewhere in the document\n")?;
            help_line(term, "note", ": nothing was written — the edit was rolled back, and the file is byte-for-byte as it was.\n")?;
        }
        // A delete the editor rolled back because its result read back with
        // more changed than the one entry (`Editor.keepOthers`).
        "DeleteChangedOtherValues" => {
            term.write_str(": deleting that would change other values in the document, so it was not done\n")?;
            help_line(term, "note", ": nothing was written — the edit was rolled back, and the file is byte-for-byte as it was.\n")?;
        }
        // A replace, insert, move or reorder the editor rolled back because
        // its result read back as something other than what was asked
        // (`Editor.readsBack`).
        "EditReadsBackWrong" => {
            term.write_str(": the file would not read back as that edit asks — other values would change, or the value written would read as another — so it was not done\n")?;
            help_line(term, "note", ": nothing was written — the edit was rolled back, and the file is byte-for-byte as it was.\n")?;
        }
        // `fig comment --inline` on a value with no end delimiter of its own
        // (a ZON `\\` string): its line is value text to the newline.
        "CommentInsideValue" => {
            term.write_str(": that value runs to the end of its line, so a comment there would be read back as part of the value\n")?;
            help_line(term, "note", ": nothing was written — the edit was rolled back, and the file is byte-for-byte as it was.\n")?;
            help_line(term, "help", &format!(": put the comment on its own line above the entry instead — `{binary_name} comment <file> <path> <text>`.\n"))?;
        }
        // A delete that would leave the document root an empty list, in a
        // format that has no spelling for one.
        "NoEmptyRootList" => {
            term.write_str(": that is the only item of the document's root list, and this format has no spelling for an empty root list — an empty document reads back as an empty mapping\n")?;
            help_line(term, "note", ": nothing was written — the file is byte-for-byte as it was.\n")?;
        }
        // The refusals `fig_core::patch` makes rather than guessing.
        "PatchThroughAlias" => {
            term.write_str(": the patch targets a value that is a YAML alias (`*name`), which belongs to whatever defined its anchor\n")?;
            help_line(term, "note", ": writing there would either change every other user of that anchor or silently shadow it, and nothing in the two files says which was meant.\n")?;
            help_line(term, "help", &format!(": patch the anchor's own definition instead, or give the alias a local value first — `{binary_name} set <file> <path> <value>`.\n"))?;
        }
        "PatchRootNotMergeable" => {
            term.write_str(": a patch whose root is not a mapping has nothing to merge INTO the document root\n")?;
            help_line(term, "help", &format!(": name where it lands — `{binary_name} patch <file> <patch-file> --at <path>` — or, to take the patch document whole, copy it.\n"))?;
        }
        "NonStringPatchKey" => {
            term.write_str(": the patch has a mapping key that is not a string, and paths are string-keyed, so there is no way to address that entry\n")?;
        }
        "PatchRenderRejected" => {
            term.write_str(": a value from the patch has no spelling the target format can read back\n")?;
            help_line(term, "note", ": nothing was written — the edit was rolled back where it failed.\n")?;
            help_line(term, "help", ": --lossless carries values the target has no native form for (a null into TOML, a datetime into JSON) through a $fig envelope.\n")?;
        }
        // No pointer at `fig check` here: `main` has already run the parse
        // `check` would, and found nothing it could locate.
        name => term.print(format_args!(": {name}\n"))?,
    }
    Ok(())
}

/// Report a document the reference-layer pass (`fig_core::materialize`)
/// refused on its way out of YAML, and exit(1). `node` is the source node the
/// pass failed on, when its span is an offset into `file`; the report then
/// points at the tag — or, for a cyclic alias, at the alias — with the source
/// line under it. `target` names the format being converted to.
pub fn report_materialize_error(term: &mut Terminal, err: Error, doc: &Document, node: Option<NodeId>, file: &str, target: &str) -> ! {
    let _ = report_materialize_error_impl(term, err, doc, node, file, target);
    let _ = term.flush();
    fileio::exit(1);
}

fn report_materialize_error_impl(term: &mut Terminal, err: Error, doc: &Document, node: Option<NodeId>, file: &str, target: &str) -> Result<()> {
    let source: &[u8] = &doc.source;
    // The span to point at: the tag for a tag failure, the alias itself for
    // a cycle. `None` when there is nothing to point into.
    let span: Option<Span> = match node {
        Some(id) => {
            if err.is("AliasCycle") {
                if (id as usize) < doc.ast.node_count() { Some(doc.span_of(id)) } else { None }
            } else {
                doc.tag_span(&doc.ast.node(id))
            }
        }
        None => None,
    };
    let tag: Option<&[u8]> = match span {
        Some(s) if !err.is("AliasCycle") && s.end <= source.len() => Some(&source[s.start..s.end]),
        _ => None,
    };
    let tag_str = tag.map(|t| String::from_utf8_lossy(t).into_owned());

    let (message, short_label): (String, &str) = match err.name() {
        "UnknownTag" => (
            match &tag_str {
                Some(t) => format!("`{t}` is a custom YAML tag, and {target} has no way to carry one"),
                None => format!("this document has a custom YAML tag, and {target} has no way to carry one"),
            },
            "unknown tag",
        ),
        "TagTypeMismatch" => (
            match &tag_str {
                Some(t) => format!("this value does not read as the type its `{t}` tag names"),
                None => "a value in this document does not read as the type its `!!` tag names".to_string(),
            },
            "tag does not fit value",
        ),
        _ => (
            format!("this alias sits inside the node it refers to, so it cannot be expanded into {target}, which has no references"),
            "cyclic alias",
        ),
    };

    if let Some(s) = span {
        print_diag(term, source, file, s.start, Some(s.end), "error", Color::Red, message.as_bytes(), short_label.as_bytes())?;
    } else {
        term.set_color(Color::Red)?;
        term.write_str("error")?;
        term.set_color(Color::Reset)?;
        term.print(format_args!(": {file}: {message}\n"))?;
    }
    match err.name() {
        "UnknownTag" => help_line(term, "help", ": pass --lax-tags to drop unknown tags and keep each value as it was parsed, or convert to YAML, which keeps them.\n")?,
        "TagTypeMismatch" => help_line(term, "help", ": change the value to one the tag's type can read, or remove the tag.\n")?,
        _ => {}
    }
    Ok(())
}

/// How a format takes the caller's text, which decides what the fix is when the
/// text turns out not to fit: spliced in verbatim as source (so a string needs
/// its own quotes), wrapped as a JSON string (so `"`/`\` need escaping), or
/// written as raw characters (so only the format's own separators can break
/// it). Read straight off the format registry, which is also what
/// `edit_ops::route` acts on.
use fig_core::kit::manifest::SpliceStyle;

fn splice_style(format: Format) -> SpliceStyle {
    match format {
        // Neither has an in-place editor, so no edit text ever reaches here.
        Format::CANONICAL | Format::GRON => SpliceStyle::Literal,
        _ => match types::runtime_entry(format) {
            Some(e) => e.splice,
            None => SpliceStyle::Literal,
        },
    }
}

/// Report an edit whose *argument* — not the file — is what doesn't parse, and
/// exit: 2 when the text was source the user typed, 1 when it was a figl value
/// the document refused (see below). `replace`/`rename`/`set`/`insert` splice the text
/// into the document, so when the reparse fails the underlying error describes the
/// spliced bytes ("not a valid TOML number" for a git sha) while pointing at a
/// file the user believes is fine. `edit_ops::apply_edit` turns that case into
/// `InvalidEditText` (it knows the document parsed before the splice);
/// this is where it becomes a message that names the argument and the fix.
///
/// `format` is `None` under `--detect`, where the resolved format isn't known
/// here — the wording then stays format-agnostic rather than risk naming the
/// wrong one. `text` is `None` for `set --seq`, whose several values give
/// nothing single to quote back.
pub fn report_bad_edit_text(term: &mut Terminal, file: &str, format: Option<Format>, kind: EditTextKind, text: Option<&[u8]>, rendered: Option<&[u8]>) -> ! {
    let _ = report_bad_edit_text_impl(term, file, format, kind, text, rendered);
    let _ = term.flush();
    // Text the user wrote as source (a `--raw` value, a key, a comment) that
    // does not parse is the command line's fault: exit 2. A fig value was
    // already read cleanly; the document refusing its spelling is exit 1.
    fileio::exit(if kind == EditTextKind::Value { 1 } else { 2 });
}

fn report_bad_edit_text_impl(term: &mut Terminal, file: &str, format: Option<Format>, kind: EditTextKind, text: Option<&[u8]>, rendered: Option<&[u8]>) -> Result<()> {
    // Long text (a pasted blob, a whole inline table) would bury the message;
    // enough is shown to recognize which argument is meant.
    const MAX_SHOWN: usize = 120;
    let shown: Option<&[u8]> = text.map(|t| &t[..t.len().min(MAX_SHOWN)]);
    let elided = text.is_some_and(|t| t.len() > MAX_SHOWN);
    let ell = if elided { "…" } else { "" };

    term.set_color(Color::Red)?;
    term.write_str("error")?;
    term.set_color(Color::Reset)?;
    if let Some(s) = shown {
        term.write_str(": `")?;
        term.write_all(s)?;
        term.print(format_args!("{ell}` is not a valid {} ", kind.noun()))?;
    } else {
        term.write_str(": one of the new values is not valid ")?;
    }
    match format {
        Some(f) => term.print(format_args!("for {file} ({})\n", types::name(f)))?,
        None => term.print(format_args!("for {file}\n"))?,
    }

    let style = match format {
        Some(f) => splice_style(f),
        None => SpliceStyle::Literal,
    };
    term.set_color(Color::Blue)?;
    term.write_str("note")?;
    term.set_color(Color::Reset)?;
    if kind == EditTextKind::Value {
        // Read as a figl value and spelled as this format spells it: the
        // spelling was refused where it landed, not the user's text. Show
        // that spelling when it differs from what was typed.
        let fmt_name = match format {
            Some(f) => types::name(f),
            None => "the file's format".to_string(),
        };
        if let Some(r) = rendered {
            if text.is_none() || r != text.unwrap() {
                let r_shown = &r[..r.len().min(MAX_SHOWN)];
                term.print(format_args!(": the value was read as a figl value and written as {fmt_name} writes it — `"))?;
                term.write_all(r_shown)?;
                term.print(format_args!(
                    "{}` — and the document would not take that there; --raw splices your text as it stands.\n",
                    if r.len() > MAX_SHOWN { "…" } else { "" }
                ))?;
                return Ok(());
            }
        }
        term.print(format_args!(
            ": the value was read as a figl value and written as {fmt_name} writes it, and the document would not take it there; --raw splices your text as it stands.\n"
        ))?;
        return Ok(());
    }
    match style {
        // The JSON family splices its keys, comments and `--raw` values as
        // source, like every other literal format.
        SpliceStyle::Raw => term.print(format_args!(
            ": the {} is written out as-is, so it cannot contain a line break or this format's own separators.\n",
            kind.noun()
        ))?,
        _ => term.print(format_args!(
            ": the {} is spliced in verbatim, as source text — so it has to stand on its own as a valid {} literal.\n",
            kind.noun(),
            match format {
                Some(f) => types::name(f),
                None => "document".to_string(),
            }
        ))?,
    }

    // The overwhelmingly common case: text that needed quotes and lost the
    // ones the shell ate. Only offered when the text isn't already quoted —
    // re-suggesting quotes on `"..."` would just be wrong.
    if style != SpliceStyle::Raw && kind == EditTextKind::RawValue {
        if let Some(s) = shown {
            if !s.is_empty() && s[0] != b'"' && s[0] != b'\'' {
                term.set_color(Color::Blue)?;
                term.write_str("help")?;
                term.set_color(Color::Reset)?;
                term.write_str(": for a string, drop --raw, or pass the quotes too — your shell strips the ones you type: '\"")?;
                term.write_all(s)?;
                term.print(format_args!("{ell}\"'\n"))?;
            }
        }
    }
    Ok(())
}

/// Report an edit that splices no text of the caller's — a delete, a comment
/// taken out — whose result does not parse, and exit(1). `report_bad_edit_text`
/// blames the argument; here there is none to blame, only what the edit left
/// behind (deleting a YAML anchor an alias still names), so the report names
/// the path and the file, and that nothing was written: the editor rolls a
/// splice that fails its reparse back.
pub fn report_unparseable_edit(term: &mut Terminal, file: &str, path: &[PathSeg]) -> ! {
    let _ = report_unparseable_edit_impl(term, file, path);
    let _ = term.flush();
    fileio::exit(1);
}

fn report_unparseable_edit_impl(term: &mut Terminal, file: &str, path: &[PathSeg]) -> Result<()> {
    term.set_color(Color::Red)?;
    term.write_str("error")?;
    term.set_color(Color::Reset)?;
    term.write_str(": the edit at ")?;
    let mut w = Vec::new();
    write_path(&mut w, path);
    term.write_all(&w)?;
    term.print(format_args!(" in {file} was refused: the file would not parse after it\n"))?;
    help_line(term, "note", ": nothing was written — the edit was rolled back, and the file is byte-for-byte as it was.\n")
}

/// `path` as the command line spells it, in backticks — the inverse of
/// `args::parse_path`: a backslash carries a key's `.`, `[`, quote or
/// backslash, and the end token prints as `[-]`. The empty path is the root.
fn write_path(w: &mut Vec<u8>, path: &[PathSeg]) {
    if path.is_empty() {
        w.extend_from_slice(b"the root");
        return;
    }
    w.push(b'`');
    for (i, seg) in path.iter().enumerate() {
        match seg {
            PathSeg::Key(k) => {
                if i > 0 {
                    w.push(b'.');
                }
                for &c in k {
                    if b".[\\\"'".contains(&c) {
                        w.push(b'\\');
                    }
                    w.push(c);
                }
            }
            PathSeg::Index(n) => {
                if *n == types::APPEND_INDEX {
                    w.extend_from_slice(b"[-]");
                } else {
                    w.extend_from_slice(format!("[{n}]").as_bytes());
                }
            }
        }
    }
    w.push(b'`');
}

/// A value argument the file's format has no spelling for — `null` into
/// TOML, a sequence into dotenv — refused before anything was written, and
/// exit(1): the command line was fine, the document cannot hold it.
pub fn report_unwritable_value(term: &mut Terminal, file: &str, err: Error) -> ! {
    let why: &str = match err.name() {
        "UnwritableNull" => "this format has no null; to leave the key without a value, `delete` it",
        "UnwritableNested" => "this format holds only flat values there, so a sequence or a table cannot be written",
        "UnwritableKey" => "the value has a key this format cannot spell",
        "UnwritableNonFinite" => "this format has no way to write infinity or NaN",
        name => name,
    };
    let _ = term.set_color(Color::Red);
    let _ = term.write_str("error");
    let _ = term.set_color(Color::Reset);
    let _ = term.print(format_args!(": the value cannot be written to {file}: {why}\n"));
    let _ = term.flush();
    fileio::exit(1);
}

/// A scalar/null value reaching the fig printer as a document root has no
/// authoring spelling there — print the teaching message and exit(1).
#[allow(dead_code)]
pub fn report_unrepresentable_root(term: &mut Terminal) -> ! {
    let _ = term.write_str("error: a scalar value cannot be the root of a .figl document; use canonical form or another output format instead (see docs/spec.md § 2).\n");
    let _ = term.flush();
    fileio::exit(1);
}

/// Every OTHER way a printer can fail — a value/shape the target format has no
/// spelling for at all (an array/nested table reaching INI/dotenv/`.properties`,
/// a non-identifier dotenv key, a non-string mapping key reaching TOML/ZON,
/// ...), and exit(1).
pub fn report_serialize_error(term: &mut Terminal, err: Error) -> ! {
    let message: &str = match err.name() {
        "WriteFailed" => "failed to write output",
        "UnresolvedAlias" => "an unresolved YAML alias reached the printer (internal error — please report this)",
        "NullUnsupported" => "a `null` value has no representation in this output format",
        "NonStringKey" => "a non-string mapping key has no representation in this output format",
        "FormatDisabled" => "the requested format was not compiled into this build",
        "NestingTooDeep" => "this document nests too deeply for the canonical printer's depth guard",
        "UnexpectedNodeKind" => "an internal fig printer error occurred (please report this)",
        "UnrepresentableRoot" => "a scalar value cannot be the root of a .figl document; use canonical form or another output format instead (see docs/spec.md § 2)",
        "UnsupportedValue" => "this document contains an array, or a table nested deeper than this format allows (INI: one level of `[section]`; dotenv/`.properties`: none)",
        "InvalidKey" => "a mapping key is not valid in this output format (a dotenv key must be a bash identifier: `[A-Za-z_][A-Za-z0-9_]*`)",
        "NonFiniteNumber" => "this output format has no way to write infinity or NaN",
        "PrintRefused" => "the language's printer refused this document",
        name => name,
    };
    let _ = term.print(format_args!("error: {message}\n"));
    let _ = term.flush();
    fileio::exit(1);
}

/// `report_serialize_error` for a runtime language's print: the language
/// refused the tree, or it declares no printer.
pub fn report_runtime_print_error(term: &mut Terminal, err: Error) -> ! {
    match err.name() {
        // A refusal the language named: the same words as a compiled printer's.
        "NullUnsupported" | "NonStringKey" | "UnrepresentableRoot" | "UnsupportedValue" | "InvalidKey" | "NonFiniteNumber"
        | "UnresolvedAlias" | "NestingTooDeep" | "UnexpectedNodeKind" => report_serialize_error(term, err),
        _ => {}
    }
    let message: &str = match err.name() {
        "RuntimePrintFailed" => "the language's printer refused this document",
        "FormatNotSerializable" | "FormatDisabled" => "this language declares no printer; it can be read but not written",
        "WriteFailed" => "failed to write output",
        name => name,
    };
    let _ = term.print(format_args!("error: {message}\n"));
    let _ = term.flush();
    fileio::exit(1);
}

/// Print every parse-time authoring warning in `warnings` (unless `--quiet`),
/// then exit(1) if `--strict` and any fired — the shared `--quiet`/`--strict`
/// contract for a language's authoring-time lints, for warnings already
/// rendered.
#[allow(clippy::too_many_arguments)]
pub fn handle_rendered_warnings(
    term: &mut Terminal,
    source: &[u8],
    file: &str,
    kind_name: &str,
    warnings: &[ParseDiagnostic],
    quiet: bool,
    strict: bool,
) -> Result<()> {
    if warnings.is_empty() {
        return Ok(());
    }
    if !quiet {
        for w in warnings {
            print_diag(term, source, file, w.offset, w.end, "warning", Color::Yellow, &w.message, &w.short_label)?;
        }
        term.flush()?;
    }
    if strict {
        term.print(format_args!("error: {} {} warning(s); --strict aborts.\n", warnings.len(), kind_name))?;
        term.flush()?;
        fileio::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_path_spells_a_path_the_way_the_command_line_reads_it() {
        crate::languages::init();
        let mut w = Vec::new();
        write_path(
            &mut w,
            &[PathSeg::Key(b"a".to_vec()), PathSeg::Index(0), PathSeg::Key(b"b.c".to_vec()), PathSeg::Index(types::APPEND_INDEX)],
        );
        assert_eq!(w, b"`a[0].b\\.c[-]`");
        let mut w = Vec::new();
        write_path(&mut w, &[]);
        assert_eq!(w, b"the root");
    }
}
