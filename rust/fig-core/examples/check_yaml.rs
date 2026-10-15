//! Differential check of the YAML port against the Zig implementation.
//!
//!     check_yaml [--1.1] print <file>        the reprint, or the error name (exit 1)
//!     check_yaml [--1.1] dump <file>...      each file's parse table, row by row
//!     check_yaml split <file>                the stream's documents: `start end explicit`
//!     check_yaml replay <width> <dump>       rebuild each table another language's Zig
//!                                            parser wrote (a Zig `xdump`), print as YAML
//!
//! `print` matches `fig fmt --dry-run -i yaml <file>`; `dump` matches a Zig
//! program that writes the table the Zig parser fills, field for field.

use std::io::Write;
use std::sync::Arc;

use fig_core::kit::ast::SerializeOptions;
use fig_core::kit::language::Language;
use fig_core::kit::table::{self, Table};
use fig_core::lang::yaml::{parser, Type, LANGUAGE};

fn text(out: &mut Vec<u8>, t: &Table, x: table::Text) {
    if x.is_absent() {
        out.extend_from_slice(b"-");
        return;
    }
    let Some(bytes) = t.text_of(x) else {
        out.extend_from_slice(b"?");
        return;
    };
    out.extend_from_slice(if x.len & table::Text::BUFFER_BIT != 0 { b"B\"" } else { b"S\"" });
    for &c in bytes {
        if c < 0x20 || c >= 0x7f || c == b'"' || c == b'\\' {
            out.extend_from_slice(format!("\\x{c:02x}").as_bytes());
        } else {
            out.push(c);
        }
    }
    out.push(b'"');
}

fn snake(name: &str) -> String {
    let mut s = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                s.push('_');
            }
            s.push(c.to_ascii_lowercase());
        } else {
            s.push(c);
        }
    }
    s
}

fn dump(out: &mut Vec<u8>, path: &str, ty: Type) {
    let src = std::fs::read(path).expect("read");
    out.extend_from_slice(format!("== {path}\n").as_bytes());
    let mut t = Table::init(Arc::<[u8]>::from(src));
    let mut report = parser::Report::default();
    if let Err(e) = parser::parse_with_report_into(&mut t, ty, &mut report) {
        out.extend_from_slice(format!("error {}", e.name()).as_bytes());
        if let Some(d) = report.diag {
            let end = d.end.map_or("null".to_string(), |e| e.to_string());
            out.extend_from_slice(format!(" code={} off={} end={}", snake(&format!("{:?}", d.code)), d.offset, end).as_bytes());
        }
        out.push(b'\n');
        return;
    }
    for (i, r) in t.rows.iter().enumerate() {
        out.extend_from_slice(format!("row {i} k={} f={} p={} s={} e={} t=", r.kind, r.flags, r.parent, r.start, r.end).as_bytes());
        text(out, &t, r.text);
        out.push(b'\n');
    }
    for a in &t.anchors {
        out.extend_from_slice(format!("anchor n={} s={} e={} ", a.node, a.start, a.end).as_bytes());
        text(out, &t, a.name);
        out.push(b'\n');
    }
    for a in &t.tags {
        out.extend_from_slice(format!("tag n={} s={} e={} r={} a={} ", a.node, a.start, a.end, a.resolved, a.agrees).as_bytes());
        text(out, &t, a.text);
        out.push(b'\n');
    }
    for a in &t.markers {
        out.extend_from_slice(format!("marker n={} s={} e={}\n", a.node, a.start, a.end).as_bytes());
    }
    for a in &t.seps {
        out.extend_from_slice(format!("sep n={} s={} e={}\n", a.node, a.start, a.end).as_bytes());
    }
    for c in &t.comments {
        out.extend_from_slice(format!("comment n={} slot={} style={} ", c.node, c.slot, c.style).as_bytes());
        text(out, &t, c.text);
        out.push(b'\n');
    }
    for d in &t.directives {
        out.extend_from_slice(b"directive ");
        text(out, &t, d.handle);
        out.push(b' ');
        text(out, &t, d.prefix);
        out.push(b'\n');
    }
    let source = t.source_arc();
    let doc = fig_core::kit::row_writer::RowWriter::adopt(source, &mut t).unwrap();
    out.extend_from_slice(b"--print\n");
    fig_core::lang::yaml::printer::print(out, &doc.ast);
    out.extend_from_slice(b"--end\n");
}

fn unescape(tok: &str) -> Option<Vec<u8>> {
    if tok == "-" {
        return None;
    }
    let b = tok.as_bytes();
    let inner = &b[1..b.len() - 1];
    let mut out = Vec::new();
    let mut i = 0;
    while i < inner.len() {
        if inner[i] == b'\\' && inner[i + 1] == b'x' {
            out.push(u8::from_str_radix(std::str::from_utf8(&inner[i + 2..i + 4]).unwrap(), 16).unwrap());
            i += 4;
        } else {
            out.push(inner[i]);
            i += 1;
        }
    }
    Some(out)
}

/// Rebuild the tables a Zig `xdump` wrote and print each as YAML, in the
/// same `== path` / `--print` / `--end` frame.
fn replay(out: &mut Vec<u8>, width: u16, dump: &str) {
    let mut lines = dump.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(path) = line.strip_prefix("== ") else { continue };
        out.extend_from_slice(format!("== {path}\n").as_bytes());
        let src = std::fs::read(path).expect("read");
        let mut t = Table::init(Arc::<[u8]>::from(src));
        let mut failed = false;
        while let Some(l) = lines.peek() {
            if l.starts_with("== ") {
                break;
            }
            let l = lines.next().unwrap();
            let f: Vec<&str> = l.split(' ').collect();
            let num = |i: usize| -> u32 { f[i].parse().unwrap() };
            match f[0] {
                "error" => {
                    out.extend_from_slice(format!("{l}\n").as_bytes());
                    failed = true;
                }
                "row" => {
                    let text = match unescape(f[6]) {
                        Some(b) => t.add_string(&b).unwrap(),
                        None => table::Text::ABSENT,
                    };
                    let row = table::Row { kind: num(1) as u8, flags: num(2) as u8, parent: num(3), start: num(4), end: num(5), text, ..Default::default() };
                    t.add_row(row).unwrap();
                }
                "anchor" => {
                    let name = t.add_string(&unescape(f[2]).unwrap()).unwrap();
                    t.anchors.push(table::Anchor { node: num(1), start: table::NONE, end: table::NONE, name });
                }
                "tag" => {
                    let text = t.add_string(&unescape(f[4]).unwrap()).unwrap();
                    t.tags.push(table::Tag { node: num(1), resolved: num(2) as u8, agrees: num(3) as u8, text, ..Default::default() });
                }
                "comment" => {
                    let text = t.add_string(&unescape(f[4]).unwrap()).unwrap();
                    t.comments.push(table::Comment { node: num(1), slot: num(2) as u8, style: num(3) as u8, text });
                }
                "directive" => {
                    let handle = t.add_string(&unescape(f[1]).unwrap()).unwrap();
                    let prefix = t.add_string(&unescape(f[2]).unwrap()).unwrap();
                    t.directives.push(table::Directive { handle, prefix });
                }
                "--print" => {
                    // Skip Zig's print, up to its `--end`.
                    for l in lines.by_ref() {
                        if l == "--end" || l.starts_with("print error") {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        if failed {
            continue;
        }
        let source = t.source_arc();
        let doc = fig_core::kit::row_writer::RowWriter::adopt(source, &mut t).unwrap();
        out.extend_from_slice(b"--print\n");
        let opts = SerializeOptions { width, ..Default::default() };
        fig_core::lang::yaml::printer::print_with(out, &doc.ast, &opts);
        out.extend_from_slice(b"--end\n");
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut ty = Type::V1_2_2;
    if args.first().map(String::as_str) == Some("--1.1") {
        ty = Type::V1_1;
        args.remove(0);
    }
    let mode = args.remove(0);
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    match mode.as_str() {
        "print" => {
            let src = std::fs::read(&args[0]).expect("read");
            match LANGUAGE.parse(&src, ty as u8) {
                Ok(doc) => {
                    let mut out = Vec::new();
                    LANGUAGE.print(ty as u8, &doc.ast, &SerializeOptions::default(), &mut out).unwrap();
                    stdout.write_all(&out).unwrap();
                }
                Err(e) => {
                    eprintln!("{}", e.name());
                    std::process::exit(1);
                }
            }
        }
        "dump" => {
            let mut out = Vec::new();
            for p in &args {
                dump(&mut out, p, ty);
            }
            stdout.write_all(&out).unwrap();
        }
        "replay" => {
            let width: u16 = args[0].parse().unwrap();
            let dump = std::fs::read_to_string(&args[1]).expect("read");
            let mut out = Vec::new();
            replay(&mut out, width, &dump);
            stdout.write_all(&out).unwrap();
        }
        "split" => {
            let src = std::fs::read(&args[0]).expect("read");
            for d in LANGUAGE.split_stream(&src, ty as u8).unwrap().unwrap() {
                writeln!(stdout, "{} {} {}", d.start, d.end, d.explicit).unwrap();
            }
        }
        _ => panic!("unknown mode {mode}"),
    }
}
