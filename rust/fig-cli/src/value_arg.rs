//! Port of src/cli/value_arg.zig.
//!
//! A value argument — `set`'s value and `--seq` items, `insert`'s value,
//! `replace`'s replacement — read once as a figl value and rendered into whatever
//! format it lands in, the way a binding's `Value` is (CLI 5 §1). The format
//! decides the spelling, never the meaning: `fig set f version 5` writes the
//! number 5 into JSON, YAML and TOML alike, and `hello` is a string in all
//! three.
//!
//! Reading, by `Mode`:
//!
//!   * `Fig` (the default) — the argument is what `v = <arg>` makes of it in
//!     fig's own dialect: `5`, `2.5`, `true`, `null` and `2026-09-22` are
//!     typed; `hello world`, `Yes` and `007` are strings (the last two with
//!     the lint the dialect gives them); `[1, 2]` and `{a = 1}` are
//!     structures; `'"5"'` is the string `5`. An empty argument, one with a
//!     line break in it or space at either end, and one in which a `#`
//!     would start a comment (`see #3`) are strings as written — a command
//!     line has no comments, and fig's reading would trim or split them.
//!     An argument that is none of these (`[1, 2`) is a usage error.
//!   * `String` (`--string`) — a string, whatever it looks like.
//!   * `Raw` (`--raw`) — the argument's own text, spliced as source in the
//!     target format and reparsed in place: for what only the target can
//!     spell (a YAML anchor, a TOML local datetime, a ZON enum literal).
//!
//! Rendering (`render`) is `fig patch`'s: the value re-rooted as a fragment
//! with `splice` set, flow for fig, and the printer's newline trimmed — on
//! one line for JSON, and for any format when the site is inside a flow
//! collection (`Layout::Flow`).
//!
//! With no language for figl registered (the Rust port before figl lands),
//! every `Fig` argument is read as a string, as Zig's build without figl
//! does.

use fig_core::diagnostics::{self, Dropped};
use fig_core::kit::ast::{Builder, PathSegment, SerializeOptions, AST};
use fig_core::serialize;
use fig_core::{err, Result};

use crate::fileio::{self, Color, Terminal};
use crate::types::{self, Format, ValueMode};
use crate::{args, diag_report, parse_dispatch};

pub type Mode = ValueMode;

/// A value argument as read: its tree, or (under `--raw`) its text.
#[derive(Debug, Clone)]
pub enum Value {
    Tree(AST),
    Raw(Vec<u8>),
}

/// What `v = ` puts in front of the argument to make it a fig document.
const PREFIX: &str = "v = ";

/// Read `text` under `mode`. A `Fig` argument that is not a value is
/// reported against the argument itself and exits 2 — the command line is
/// wrong, not the file; its lints are printed to `term` and reading goes on.
pub fn read(term: &mut Terminal, text: &str, mode: Mode) -> Result<Value> {
    match mode {
        Mode::Raw => Ok(Value::Raw(text.as_bytes().to_vec())),
        Mode::String => Ok(Value::Tree(string_tree(text.as_bytes())?)),
        Mode::Fig => read_fig(term, text),
    }
}

fn read_fig(term: &mut Terminal, text: &str) -> Result<Value> {
    // Empty, blank, with a line break, or with space at either end: fig's
    // reading would drop or split what the user typed, so it is a string as
    // typed (`' x'` stays ` x`, where `v =  x` would trim it).
    let trimmed = text.trim_matches([' ', '\t']);
    if trimmed.is_empty() || trimmed.len() != text.len() || text.contains(['\r', '\n']) {
        return Ok(Value::Tree(string_tree(text.as_bytes())?));
    }

    // With no language for fig at all every argument is a string, the one
    // reading that needs no parser.
    let Some(format) = args::parse_format_name("figl") else { return Ok(Value::Tree(string_tree(text.as_bytes())?)) };
    let source = format!("{PREFIX}{text}\n");
    let mut reports = parse_dispatch::Reports::default();
    let doc = match parse_dispatch::parse_slice_as(format, source.as_bytes(), &mut reports) {
        Ok(d) => d,
        Err(e) => {
            if text.contains('#') {
                return Ok(Value::Tree(string_tree(text.as_bytes())?));
            }
            let Some(d) = reports.diag else { return Err(e) };
            diag_report::print_diag(
                term,
                text.as_bytes(),
                "<value>",
                shift(d.offset, text),
                d.end.map(|e| shift(e, text)),
                "error",
                Color::Red,
                &d.message,
                &d.short_label,
            )?;
            term.write_str("help: this argument is read as a figl value; pass --string to take it as a string, or --raw to splice it as the file's own syntax\n")?;
            term.flush()?;
            fileio::exit(2);
        }
    };
    // A `#` the dialect took as a comment start was part of the argument.
    if doc.ast.has_comments() {
        return Ok(Value::Tree(string_tree(text.as_bytes())?));
    }

    for w in &reports.warnings {
        diag_report::print_diag(
            term,
            text.as_bytes(),
            "<value>",
            shift(w.offset, text),
            w.end.map(|e| shift(e, text)),
            "warning",
            Color::Yellow,
            &w.message,
            &w.short_label,
        )?;
    }
    term.flush()?;

    let mut ast = doc.ast;
    ast.root = ast.get_val_by_path(&[PathSegment::Key(b"v")])?.id;
    Ok(Value::Tree(ast))
}

/// An offset into `v = <text>\n`, as an offset into `text`.
fn shift(offset: usize, text: &str) -> usize {
    offset.saturating_sub(PREFIX.len()).min(text.len())
}

fn string_tree(text: &[u8]) -> Result<AST> {
    let mut b = Builder::init();
    let id = b.add_string(text)?;
    b.finish(id)
}

/// How a structure is laid out: the format's own layout, or on one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// A YAML block, a TOML inline table, a ZON literal across lines: what
    /// the format writes for a value of its own. JSON is compact either way,
    /// since a value spliced into an object cannot span lines.
    Natural,
    /// On one line, for a site inside a flow collection (`k: {a: 1}` in
    /// YAML), where a block spelling cannot land. YAML's is JSON's, which
    /// every YAML reads as flow.
    Flow,
}

/// The splice text for `value` in `format` — what the editor takes for it.
/// A value the format has no spelling for (`null` in TOML, a table in
/// dotenv) is `Unwritable*`, for the caller to report.
pub fn render(value: &Value, format: Format, layout: Layout) -> Result<Vec<u8>> {
    let ast = match value {
        Value::Raw(text) => return Ok(text.clone()),
        Value::Tree(t) => t,
    };
    let one_line = layout == Layout::Flow || types::is(format, "json") || types::is(format, "jsonc") || types::is(format, "json5");
    let options = SerializeOptions {
        // What the editor takes, not a document — see `SerializeOptions.splice`.
        splice: true,
        pretty: !one_line,
        // fig's block spellings only parse as standalone lines; a value
        // spliced after `key = ` has to be flow.
        flow: types::is(format, "figl"),
        ..SerializeOptions::default()
    };
    // A YAML flow site takes JSON's spelling, which every YAML reads as flow.
    let to = if layout == Layout::Flow && types::is(format, "yaml") {
        args::parse_format_name("json").unwrap_or(format)
    } else {
        format
    };
    let target = types::to_target(to).ok_or(err!(UnsupportedValueTarget))?;
    let mut w = Vec::new();
    // Renamed, so a value the format cannot hold is never mistaken for a
    // parse error the file raised under the same name (INI's and TOML's
    // parsers raise `InvalidKey` on a malformed file).
    serialize::serialize_fragment_with(ast, &mut w, &target, &options).map_err(|e| match e.name() {
        "NullUnsupported" => err!(UnwritableNull),
        "UnsupportedValue" => err!(UnwritableNested),
        "NonStringKey" | "InvalidKey" => err!(UnwritableKey),
        "NonFiniteNumber" => err!(UnwritableNonFinite),
        _ => e,
    })?;
    // Every printer ends a document with a newline; a spliced value never
    // carries one (an appended flow item would land on a line of its own).
    while w.last() == Some(&b'\n') {
        w.pop();
    }
    Ok(w)
}

/// Refuse a value `format` cannot hold once it lands `depth` levels below the
/// document root — a mapping into dotenv, a table under an INI section —
/// with the error `render` gives one the printer refuses outright. The
/// printer cannot tell on its own: a mapping is a fine dotenv DOCUMENT, and
/// prints as `x=1`, which spliced after `n=` reads back as the string `x=1`.
/// Text with no value behind it (`--raw`) is the user's own spelling and is
/// not second-guessed.
pub fn refuse_dropped(value: &Value, format: Format, depth: usize) -> Result<()> {
    let ast = match value {
        Value::Raw(_) => return Ok(()),
        Value::Tree(t) => t,
    };
    let Some(target) = types::to_target(format) else { return Ok(()) };
    match diagnostics::first_dropped(ast, ast.root, &target, depth) {
        None => Ok(()),
        Some(Dropped::Null) => Err(err!(UnwritableNull)),
        Some(Dropped::Container) => Err(err!(UnwritableNested)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fileio::{Mode as TermMode, Sink};

    #[test]
    fn a_fig_reading_types_what_fig_types_and_keeps_the_rest_a_string() {
        if args::parse_format_name("figl").is_none() {
            return;
        }
        let mut term = Terminal::new(Sink::Discard, TermMode::NoColor);
        let cases: [(&str, &str); 17] = [
            ("5", "5"),
            ("-2.5", "-2.5"),
            ("true", "true"),
            ("null", "null"),
            ("hello", "\"hello\""),
            ("hello world", "\"hello world\""),
            ("a: b", "\"a: b\""),
            ("\"5\"", "\"5\""),
            ("'q'", "\"q\""),
            ("see #3", "\"see #3\""),
            ("#3", "\"#3\""),
            ("", "\"\""),
            ("hello ", "\"hello \""),
            (" 5", "\" 5\""),
            ("two\nlines", "\"two\\nlines\""),
            ("[1, 2]", "[1,2]"),
            ("{a = 1, b = [x]}", "{\"a\":1,\"b\":[\"x\"]}"),
        ];
        for (input, want) in cases {
            let v = read(&mut term, input, Mode::Fig).unwrap();
            assert_eq!(String::from_utf8(render(&v, types::must_named("json"), Layout::Natural).unwrap()).unwrap(), want);
        }
    }

    #[test]
    fn string_and_raw_take_the_argument_as_it_stands() {
        let mut term = Terminal::new(Sink::Discard, TermMode::NoColor);
        let v = read(&mut term, "1.10", Mode::String).unwrap();
        assert_eq!(render(&v, types::must_named("json"), Layout::Natural).unwrap(), b"\"1.10\"");
        let v = read(&mut term, "[1,2", Mode::Raw).unwrap();
        assert_eq!(render(&v, types::must_named("json"), Layout::Natural).unwrap(), b"[1,2");
    }
}
