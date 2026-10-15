//! Parse each file with the Rust ZON, plist or NestedText language and print
//! its reprint (parse, then `print` in the same dialect with default
//! `SerializeOptions`), or the parse error — what `fig fmt --dry-run` and
//! `fig check` say, for a differential check against the Zig CLI. A refusal
//! prints `error: <message>` (the parser's diagnostic, or the error's name),
//! `--> FILE:LINE:COL` and the caret line's label.
//!
//! Usage: `check_zon [-i zon|plist|nestedtext] [-o LANG] [--table] FILE...`.
//! Without `-i`, the language is the file's extension (`.zon`, `.plist`,
//! `.nt`). With several files, each output is preceded by a `==> FILE <==`
//! line. `-o LANG` prints the tree in another of the three languages (`fig
//! convert`). `--table` prints the parsed node table as `fig lang table`
//! does, instead of the reprint.

use std::process::ExitCode;

use fig_core::kit::ast::{ExtKind, SerializeOptions};
use fig_core::kit::language::{Language, ParseReport};
use fig_core::kit::parse_diagnostic::locate_offset;
use fig_core::kit::table::Table;
use fig_core::lang::{nestedtext, plist, zon};

fn language(name: &str) -> Option<&'static dyn Language> {
    match name {
        "zon" => Some(&zon::LANGUAGE),
        "plist" => Some(&plist::LANGUAGE),
        "nestedtext" | "nt" => Some(&nestedtext::LANGUAGE),
        _ => None,
    }
}

/// `std.json.Stringify.encodeJsonString` with the default options: bytes
/// past ASCII are written as they are.
fn json_string(out: &mut Vec<u8>, s: &[u8]) {
    out.push(b'"');
    for &c in s {
        match c {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0c => out.extend_from_slice(b"\\f"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            c if c < 0x20 => out.extend_from_slice(format!("\\u{:04x}", c).as_bytes()),
            c => out.push(c),
        }
    }
    out.push(b'"');
}

/// The table as `fig lang table` prints it (`src/contract/wire.zig`'s
/// `tableToJson`).
fn table_json(t: &Table) -> Vec<u8> {
    use fig_core::kit::table::{self as tb, Kind as K, NONE};
    let v = t.view();
    let mut w: Vec<u8> = b"{\"rows\":[".to_vec();
    let span = |w: &mut Vec<u8>, key: &str, start: u32, end: u32| {
        if start != NONE {
            w.extend_from_slice(format!(",\"{key}\":[{start},{end}]").as_bytes());
        }
    };
    let text = |w: &mut Vec<u8>, key: &str, x: tb::Text| {
        if let Some(x) = v.text_of(x) {
            w.extend_from_slice(format!(",\"{key}\":").as_bytes());
            json_string(w, x);
        }
    };
    for (i, r) in v.rows.iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        if r.kind >= K::EXT {
            let ek = ExtKind::from_ordinal(r.kind - K::EXT).unwrap();
            let kind = if ek == ExtKind::CharLiteral { "int" } else { "string" };
            w.extend_from_slice(format!("{{\"kind\":\"{kind}\",\"ext_kind\":\"{}\"", ek.name()).as_bytes());
        } else {
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
        }
        if r.parent == NONE {
            w.extend_from_slice(",\"parent\":null".as_bytes());
        } else {
            w.extend_from_slice(format!(",\"parent\":{}", r.parent).as_bytes());
        }
        span(&mut w, "span", r.start, r.end);
        if r.kind == K::FALSE {
            w.extend_from_slice(",\"text\":\"false\"".as_bytes());
        } else if r.kind == K::TRUE {
            w.extend_from_slice(",\"text\":\"true\"".as_bytes());
        } else {
            text(&mut w, "text", r.text);
        }
        let f = v.facts(i as u32);
        if let Some(a) = f.anchor {
            text(&mut w, "anchor", a.name);
            span(&mut w, "anchor_span", a.start, a.end);
        }
        if let Some(tg) = f.tag {
            text(&mut w, "tag", tg.text);
            span(&mut w, "tag_span", tg.start, tg.end);
        }
        if let Some(m) = f.marker {
            span(&mut w, "marker", m.start, m.end);
        }
        if let Some(m) = f.sep {
            span(&mut w, "sep", m.start, m.end);
        }
        if let Some(is) = f.item_sep {
            text(&mut w, "item_sep", is.text);
        }
        if r.flags & tb::ROW_OPEN != 0 {
            w.extend_from_slice(",\"open\":true".as_bytes());
        }
        w.push(b'}');
    }
    w.extend_from_slice("],\"regions\":[".as_bytes());
    for (i, r) in v.regions.iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        w.extend_from_slice(format!("{{\"node\":{},\"start\":{},\"end\":{}}}", r.node, r.start, r.end).as_bytes());
    }
    w.extend_from_slice("],\"mentions\":[".as_bytes());
    w.extend_from_slice("],\"comments\":[".as_bytes());
    for (i, c) in v.comments.iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        let slot = match c.slot {
            tb::COMMENT_TRAILING => "trailing",
            tb::COMMENT_DANGLING => "dangling",
            _ => "leading",
        };
        let style = if c.style == tb::COMMENT_BLOCK { "block" } else { "line" };
        w.extend_from_slice(format!("{{\"node\":{},\"slot\":\"{slot}\",\"style\":\"{style}\",\"text\":", c.node).as_bytes());
        json_string(&mut w, v.text_of(c.text).unwrap_or(b""));
        w.push(b'}');
    }
    w.extend_from_slice("]}".as_bytes());
    w
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut forced: Option<String> = None;
    let mut output: Option<String> = None;
    let mut show_table = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-i" => {
                forced = Some(args.remove(i + 1));
                args.remove(i);
            }
            "-o" => {
                output = Some(args.remove(i + 1));
                args.remove(i);
            }
            "--table" => {
                show_table = true;
                args.remove(i);
            }
            _ => i += 1,
        }
    }
    let many = args.len() > 1;
    let mut status = ExitCode::SUCCESS;
    for path in &args {
        let name = forced.clone().unwrap_or_else(|| path.rsplit('.').next().unwrap_or("").to_string());
        let Some(lang) = language(&name) else {
            eprintln!("{path}: no language for `{name}`");
            return ExitCode::from(2);
        };
        let input = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{path}: {e}");
                return ExitCode::from(2);
            }
        };
        if many {
            println!("==> {path} <==");
        }
        let mut report = ParseReport::default();
        match lang.parse_with_report(&input, 0, &mut report) {
            Err(e) => {
                // As the CLI reports a refusal: the parser's first diagnostic,
                // or the error's name at the top of the file.
                let (message, offset, end, label) = match report.errors.first() {
                    Some(d) => (String::from_utf8_lossy(&d.message).into_owned(), d.offset, d.end, String::from_utf8_lossy(&d.short_label).into_owned()),
                    None => (e.name().to_string(), 0, None, lang.name().to_string()),
                };
                let loc = locate_offset(&input, offset);
                // The caret line: a `^` at the column, or a `~` run under
                // the span on its line.
                let pad = " ".repeat(loc.column - 1);
                let marks = match end {
                    Some(end) if end > offset + 1 => {
                        let line_end = loc.line_text.len() + offset + 1 - loc.column;
                        let n = end.min(line_end).max(offset + 1) - offset;
                        if n > 1 { "~".repeat(n) } else { "^".to_string() }
                    }
                    _ => "^".to_string(),
                };
                println!("error: {message}\n--> {path}:{}:{}\nlabel: {pad}{marks} {label}", loc.line, loc.column);
                status = ExitCode::from(1);
            }
            Ok(doc) => {
                if show_table {
                    // As `fig lang table` does: the adopted document's full
                    // table.
                    let t = fig_core::kit::convert::full_table(&doc).expect("a table");
                    use std::io::Write;
                    let mut o = table_json(&t);
                    o.push(b'\n');
                    std::io::stdout().write_all(&o).unwrap();
                    continue;
                }
                let mut out = Vec::new();
                let target = match &output {
                    Some(o) => language(o).expect("an output language"),
                    None => lang,
                };
                match target.print(0, &doc.ast, &SerializeOptions::default(), &mut out) {
                    Ok(()) => {
                        use std::io::Write;
                        std::io::stdout().write_all(&out).unwrap();
                    }
                    Err(e) => {
                        println!("print error: {}", e.name());
                        status = ExitCode::from(1);
                    }
                }
            }
        }
    }
    status
}
