//! Differential check for the JSON port: for each file given, parse it with
//! the Rust JSON language and print the reprint (parse, then `print` in the
//! same dialect), or `error: <ZigErrorName>` when the parse is refused.
//!
//! Usage: check_json [-d json|jsonc|json5] [--compact] [--indent N] [--strip-comments] [--diag] FILE...
//!
//! `--diag` adds, after a refusal, `diag: LINE:COL: <message>` for the first
//! error the parse reported.
//!
//! Without `-d`, the dialect is the file's extension (`.jsonc`, `.json5`),
//! else `json`. With several files each output is preceded by
//! `==> FILE <==`.

use std::io::Write;

use fig_core::kit::ast::SerializeOptions;
use fig_core::kit::language::{Language, ParseReport};
use fig_core::kit::parse_diagnostic::locate_offset;
use fig_json::{Type, LANGUAGE};

fn dialect_named(name: &str) -> Option<Type> {
    match name {
        "json" => Some(Type::JSON),
        "jsonc" => Some(Type::JSONC),
        "json5" => Some(Type::JSON5),
        _ => None,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut dialect: Option<Type> = None;
    let mut options = SerializeOptions::default();
    let mut files = Vec::new();
    let mut diag = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "-d" | "--dialect" => {
                let d = args.next().expect("-d needs a dialect");
                dialect = Some(dialect_named(&d).unwrap_or_else(|| panic!("no dialect {d}")));
            }
            "--diag" => diag = true,
            "--compact" => options.pretty = false,
            "--strip-comments" => options.strip_comments = true,
            "--indent" => options.indent = args.next().expect("--indent needs N").parse().expect("a number"),
            _ => files.push(a),
        }
    }
    let many = files.len() > 1;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut failed = false;
    for f in &files {
        let t = dialect.unwrap_or_else(|| {
            let ext = std::path::Path::new(f).extension().and_then(|e| e.to_str()).unwrap_or("");
            dialect_named(ext).unwrap_or(Type::JSON)
        });
        if many {
            writeln!(out, "==> {f} <==").unwrap();
        }
        let input = std::fs::read(f).unwrap_or_else(|e| panic!("{f}: {e}"));
        let mut report = ParseReport::default();
        match LANGUAGE.parse_with_report(&input, t.dialect(), &mut report) {
            Ok(mut doc) => {
                // Core strips comments before printing, as `serialize` does.
                if options.strip_comments {
                    doc.ast.strip_comments();
                }
                let mut buf = Vec::new();
                match LANGUAGE.print(t.dialect(), &doc.ast, &options, &mut buf) {
                    Ok(()) => out.write_all(&buf).unwrap(),
                    Err(e) => {
                        failed = true;
                        writeln!(out, "print error: {}", e.name()).unwrap();
                    }
                }
            }
            Err(e) => {
                failed = true;
                writeln!(out, "error: {}", e.name()).unwrap();
                if diag {
                    if let Some(d) = report.errors.first() {
                        let loc = locate_offset(&input, d.offset);
                        writeln!(out, "diag: {}:{}: {}", loc.line, loc.column, String::from_utf8_lossy(&d.message)).unwrap();
                    }
                }
            }
        }
    }
    if failed && !many {
        std::process::exit(1);
    }
}
