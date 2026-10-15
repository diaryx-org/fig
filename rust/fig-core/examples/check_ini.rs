//! A differential check for fig's flat formats (INI, dotenv, `.properties`)
//! against the Zig CLI.
//!
//! ```text
//! cargo run --example check_ini -- [-i ini|dotenv|properties] [--table] <file>...
//! ```
//!
//! For each file, parse it with the Rust language (the dialect from `-i`, or
//! the file's extension) and print either the reprint — `print` in the same
//! dialect with default `SerializeOptions`, what `fig fmt --dry-run` prints —
//! or, on a refusal, `error: <ZigName>` on stderr (exit status 1). With
//! `--table`, print instead the node table the parse wrote, as the JSON
//! `fig lang table` writes (`src/contract/wire.zig`'s `tableToJson`).

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use fig_core::kit::ast::SerializeOptions;
use fig_core::kit::document::MentionKind;
use fig_core::kit::language::{Language, ParseReport};
use fig_core::kit::parse_diagnostic;
use fig_core::kit::table::{self, Table};
use fig_core::lang::{dotenv, ini, properties};

fn language_of(name: &str) -> Option<&'static dyn Language> {
    match name {
        "ini" => Some(&ini::LANGUAGE),
        "dotenv" | "env" => Some(&dotenv::LANGUAGE),
        "properties" => Some(&properties::LANGUAGE),
        _ => None,
    }
}

fn json_string(w: &mut Vec<u8>, s: &[u8]) {
    w.push(b'"');
    for &c in s {
        match c {
            b'"' => w.extend_from_slice(b"\\\""),
            b'\\' => w.extend_from_slice(b"\\\\"),
            b'\n' => w.extend_from_slice(b"\\n"),
            b'\r' => w.extend_from_slice(b"\\r"),
            b'\t' => w.extend_from_slice(b"\\t"),
            0x08 => w.extend_from_slice(b"\\b"),
            0x0c => w.extend_from_slice(b"\\f"),
            c if c < 0x20 => w.extend_from_slice(format!("\\u{:04x}", c).as_bytes()),
            c => w.push(c),
        }
    }
    w.push(b'"');
}

/// `tableToJson`, for the facts a flat format writes.
fn table_json(t: &Table) -> Vec<u8> {
    use table::Kind as K;
    let mut w = Vec::new();
    w.extend_from_slice(b"{\"rows\":[");
    for (i, r) in t.rows.iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        let name = match r.kind {
            K::NULL => "null",
            K::FALSE | K::TRUE => "bool",
            K::STRING => "string",
            K::INT => "int",
            K::FLOAT => "float",
            K::SEQUENCE => "sequence",
            K::MAPPING => "mapping",
            K::KEYVALUE => "keyvalue",
            K::ALIAS => "alias",
            _ => "null",
        };
        w.extend_from_slice(format!("{{\"kind\":\"{name}\"").as_bytes());
        if r.parent == table::NONE {
            w.extend_from_slice(b",\"parent\":null");
        } else {
            w.extend_from_slice(format!(",\"parent\":{}", r.parent).as_bytes());
        }
        if r.start != table::NONE {
            w.extend_from_slice(format!(",\"span\":[{},{}]", r.start, r.end).as_bytes());
        }
        if let Some(x) = t.text_of(r.text) {
            w.extend_from_slice(b",\"text\":");
            json_string(&mut w, x);
        }
        w.push(b'}');
    }
    w.extend_from_slice(b"],\"regions\":[");
    for (i, r) in t.regions.iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        w.extend_from_slice(format!("{{\"node\":{},\"start\":{},\"end\":{}}}", r.node, r.start, r.end).as_bytes());
    }
    w.extend_from_slice(b"],\"mentions\":[");
    for (i, m) in t.mentions.iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        let kind = match MentionKind::of(m.kind) {
            MentionKind::Header => "header",
            MentionKind::Entry => "entry",
            MentionKind::Index => "index",
            MentionKind::Append => "append",
        };
        w.extend_from_slice(format!("{{\"node\":{},\"span\":[{},{}],\"kind\":\"{kind}\"}}", m.node, m.start, m.end).as_bytes());
    }
    w.extend_from_slice(b"],\"comments\":[");
    for (i, c) in t.comments.iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        let slot = match c.slot {
            table::COMMENT_TRAILING => "trailing",
            table::COMMENT_DANGLING => "dangling",
            _ => "leading",
        };
        let style = if c.style == table::COMMENT_BLOCK { "block" } else { "line" };
        w.extend_from_slice(format!("{{\"node\":{},\"slot\":\"{slot}\",\"style\":\"{style}\",\"text\":", c.node).as_bytes());
        json_string(&mut w, t.text_of(c.text).unwrap_or(b""));
        w.push(b'}');
    }
    w.extend_from_slice(b"]}");
    w
}

fn main() -> ExitCode {
    let mut dialect: Option<String> = None;
    let mut table_mode = false;
    let mut diag_mode = false;
    let mut files: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-i" | "--input" => dialect = args.next(),
            "--table" => table_mode = true,
            "--diag" => diag_mode = true,
            _ => files.push(a),
        }
    }
    let mut status = ExitCode::SUCCESS;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for f in &files {
        let name = dialect.clone().unwrap_or_else(|| f.rsplit('.').next().unwrap_or("").to_string());
        let Some(lang) = language_of(&name) else {
            eprintln!("{f}: no flat language named '{name}'");
            status = ExitCode::from(2);
            continue;
        };
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{f}: {e}");
                status = ExitCode::from(2);
                continue;
            }
        };
        if diag_mode {
            let mut report = ParseReport::default();
            let parsed = lang.parse_with_report(&bytes, 0, &mut report);
            let mut line = |sev: &str, d: &fig_core::kit::language::ParseDiagnostic| {
                let loc = parse_diagnostic::locate_offset(&bytes, d.offset);
                let _ = writeln!(
                    out,
                    "{sev}|{}|{}:{}|{}/{}",
                    String::from_utf8_lossy(&d.message),
                    loc.line,
                    loc.column,
                    String::from_utf8_lossy(&d.short_label),
                    d.end.map_or(1, |e| (e - d.offset).max(1))
                );
            };
            for d in &report.errors {
                line("error", d);
            }
            for d in &report.warnings {
                line("warning", d);
            }
            if let Err(e) = parsed {
                if report.errors.is_empty() {
                    let _ = writeln!(out, "error|{}|1:1|/1", e.name());
                }
                status = ExitCode::from(1);
            }
            continue;
        }
        if table_mode {
            let mut t = Table::init(Arc::<[u8]>::from(&bytes[..]));
            match lang.parse_table(&bytes, 0, &mut t) {
                Ok(()) => {
                    let _ = out.write_all(&table_json(&t));
                    let _ = out.write_all(b"\n");
                }
                Err(e) => {
                    eprintln!("error: {}", e.name());
                    status = ExitCode::from(1);
                }
            }
            continue;
        }
        let printed = lang.parse(&bytes, 0).and_then(|doc| {
            let mut buf = Vec::new();
            lang.print(0, &doc.ast, &SerializeOptions::default(), &mut buf).map(|()| buf)
        });
        match printed {
            Ok(buf) => {
                let _ = out.write_all(&buf);
            }
            Err(e) => {
                eprintln!("error: {}", e.name());
                status = ExitCode::from(1);
            }
        }
    }
    status
}
