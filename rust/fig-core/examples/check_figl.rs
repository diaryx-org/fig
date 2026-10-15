//! Differential check for the figl port: for each file given, parse it with
//! the Rust figl language and print either its reprint (parse, then `print`
//! with default `SerializeOptions`) or the parse error's Zig name.
//!
//! Usage: `check_figl [--dialect figl] [--tree] [--warnings] FILE...`
//!
//! With one file, the reprint goes to stdout as is (the shape `fig fmt
//! --dry-run` prints) and a refusal prints `error: <Name>` to stderr and
//! exits 1. With several, each result is framed by a `==> FILE <==` line.
//! `--width N`, `--indent-requested` and `--strip-comments` set those print
//! options. `--tree` prints a dump of the parsed tree instead of the reprint;
//! `--warnings` prints each warning and error diagnostic as
//! `warning|error OFFSET END label` to stderr; `--diag` prints, as
//! `fig fmt` reports them, the first error of a refusal or every warning of
//! a clean parse (`--diag-all`: every error, as `fig check` does), one per
//! line, as `D<TAB>severity<TAB>message<TAB>line:col<TAB>label<TAB>squiggle
//! width` (0 for a caret).
//!
//! Two more modes: `--from-json FILE` prints a JSON file's tree as figl (the
//! printer on a foreign tree, `fig convert -i json -o figl`'s twin), and
//! `--table-list LIST OUTDIR` writes the parse table of the i-th path in LIST
//! (rows, facts, diagnostics) to `OUTDIR/i.txt`, for comparison with the
//! same dump written from the Zig parser.

use std::io::Write;

use fig_core::kit::ast::{NodeId, NodeKind, SerializeOptions, AST};
use fig_core::kit::language::{Language, ParseReport};
use fig_core::lang::figl;

fn dialect_of(name: &str) -> u8 {
    match name {
        "figl" | "fig" => 0,
        other => panic!("unknown dialect {other}"),
    }
}

fn dump(ast: &AST, id: NodeId, depth: usize, out: &mut Vec<u8>) {
    let pad = "  ".repeat(depth);
    let c = ast.comments(id);
    let mut note = String::new();
    for l in c.leading.iter() {
        note.push_str(&format!(" lead={:?}", String::from_utf8_lossy(l.text)));
    }
    if let Some(t) = c.trailing {
        note.push_str(&format!(" trail={:?}", String::from_utf8_lossy(t.text)));
    }
    for d in c.dangling.iter() {
        note.push_str(&format!(" dangle={:?}", String::from_utf8_lossy(d.text)));
    }
    if let Some(t) = ast.tag_of(id) {
        note.push_str(&format!(" tag={:?}", String::from_utf8_lossy(t.spelling())));
    }
    let span = ast.span_of(id);
    note.push_str(&format!(" @{}..{}", span.start, span.end));
    match ast.kind_of(id) {
        NodeKind::Mapping(first) => {
            out.extend_from_slice(format!("{pad}map{note}\n").as_bytes());
            let mut cur = first;
            while let Some(kv) = cur {
                dump(ast, kv, depth + 1, out);
                cur = ast.node(kv).next_sibling;
            }
        }
        NodeKind::Sequence(first) => {
            out.extend_from_slice(format!("{pad}seq{note}\n").as_bytes());
            let mut cur = first;
            while let Some(el) = cur {
                dump(ast, el, depth + 1, out);
                cur = ast.node(el).next_sibling;
            }
        }
        NodeKind::KeyValue(kv) => {
            out.extend_from_slice(format!("{pad}entry{note}\n").as_bytes());
            dump(ast, kv.key, depth + 1, out);
            dump(ast, kv.value, depth + 1, out);
        }
        NodeKind::String(s) => out.extend_from_slice(format!("{pad}str {:?}{note}\n", String::from_utf8_lossy(s)).as_bytes()),
        NodeKind::Number(n) => out.extend_from_slice(format!("{pad}num {:?} {:?}{note}\n", n.kind, String::from_utf8_lossy(n.raw)).as_bytes()),
        NodeKind::Boolean(b) => out.extend_from_slice(format!("{pad}bool {b}{note}\n").as_bytes()),
        NodeKind::Null => out.extend_from_slice(format!("{pad}null{note}\n").as_bytes()),
        NodeKind::Extended(e) => out.extend_from_slice(format!("{pad}ext {} {:?}{note}\n", e.kind.name(), String::from_utf8_lossy(e.text)).as_bytes()),
        NodeKind::Alias(a) => out.extend_from_slice(format!("{pad}alias {:?}{note}\n", String::from_utf8_lossy(a)).as_bytes()),
    }
}

/// A strict-JSON reader good enough to build a foreign tree for the figl
/// printer (strings decoded, numbers by lexeme, keys in order).
struct Json<'a> {
    s: &'a [u8],
    i: usize,
}

impl Json<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn string(&mut self) -> Option<Vec<u8>> {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = *self.s.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return Some(out),
                b'\\' => {
                    let e = *self.s.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'/' | b'\\' | b'"' => out.push(e),
                        b'u' => {
                            let hex = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
                            let mut cp = u32::from_str_radix(hex, 16).ok()?;
                            self.i += 4;
                            if (0xD800..0xDC00).contains(&cp) && self.s.get(self.i..self.i + 2) == Some(b"\\u") {
                                let lo = u32::from_str_radix(std::str::from_utf8(self.s.get(self.i + 2..self.i + 6)?).ok()?, 16).ok()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                    self.i += 6;
                                }
                            }
                            let ch = char::from_u32(cp)?;
                            let mut b = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                        }
                        _ => return None,
                    }
                }
                _ => out.push(c),
            }
        }
    }

    fn value(&mut self, b: &mut fig_core::kit::ast::Builder) -> Option<NodeId> {
        self.ws();
        let c = *self.s.get(self.i)?;
        match c {
            b'{' => {
                self.i += 1;
                let mut entries = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return b.add_mapping(&entries).ok();
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return None;
                    }
                    let k = self.string()?;
                    self.ws();
                    if self.s.get(self.i) != Some(&b':') {
                        return None;
                    }
                    self.i += 1;
                    let key = b.add_string(&k).ok()?;
                    let value = self.value(b)?;
                    entries.push(fig_core::kit::ast::Entry { key, value });
                    self.ws();
                    match self.s.get(self.i)? {
                        b',' => self.i += 1,
                        b'}' => {
                            self.i += 1;
                            return b.add_mapping(&entries).ok();
                        }
                        _ => return None,
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return b.add_sequence(&items).ok();
                }
                loop {
                    items.push(self.value(b)?);
                    self.ws();
                    match self.s.get(self.i)? {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return b.add_sequence(&items).ok();
                        }
                        _ => return None,
                    }
                }
            }
            b'"' => {
                let s = self.string()?;
                b.add_string(&s).ok()
            }
            b't' if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                b.add_bool(true).ok()
            }
            b'f' if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                b.add_bool(false).ok()
            }
            b'n' if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                b.add_null().ok()
            }
            b'-' | b'0'..=b'9' => {
                let start = self.i;
                while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                let raw = &self.s[start..self.i];
                let float = raw.iter().any(|&c| matches!(c, b'.' | b'e' | b'E'));
                b.add_number_raw(raw, float).ok()
            }
            _ => None,
        }
    }
}

fn hex(t: &fig_core::kit::table::Table, x: fig_core::kit::table::Text) -> String {
    match t.text_of(x) {
        Some(s) => s.iter().map(|b| format!("{b:02x}")).collect(),
        None => "-".into(),
    }
}

fn opt(v: Option<usize>) -> String {
    v.map_or("null".into(), |v| v.to_string())
}

/// The parse table as the Zig dump test prints it (rows and facts of a clean
/// parse, else the report's diagnostics; then the warnings).
fn table_dump(input: &[u8]) -> String {
    use fig_core::kit::table::Table;
    use figl::parser;
    let src: std::sync::Arc<[u8]> = std::sync::Arc::from(input);
    let mut t = Table::init(src.clone());
    let mut report = parser::Report::default();
    let mut w = String::new();
    match parser::parse_collecting_into(&mut t, &src, figl::Type::Fig, &mut report) {
        Ok(()) => {
            w.push_str("ok\n");
            for (n, r) in t.rows.iter().enumerate() {
                w.push_str(&format!("row {n} {} {} {} {} {} {}\n", r.kind, r.flags, r.parent, r.start, r.end, hex(&t, r.text)));
            }
            for x in &t.tags {
                w.push_str(&format!("tag {} {} {} {} {} {}\n", x.node, x.resolved, x.agrees, x.start, x.end, hex(&t, x.text)));
            }
            for x in &t.markers {
                w.push_str(&format!("marker {} {} {}\n", x.node, x.start, x.end));
            }
            for x in &t.seps {
                w.push_str(&format!("sep {} {} {}\n", x.node, x.start, x.end));
            }
            for x in &t.comments {
                w.push_str(&format!("comment {} {} {} {}\n", x.node, x.slot, x.style, hex(&t, x.text)));
            }
            for x in &t.regions {
                w.push_str(&format!("region {} {} {}\n", x.node, x.start, x.end));
            }
            for x in &t.mentions {
                w.push_str(&format!("mention {} {} {} {}\n", x.node, x.start, x.end, x.kind));
            }
        }
        Err(e) => {
            w.push_str(&format!("err {}\n", e.name()));
            if let Some(d) = report.diag {
                w.push_str(&format!("diag {} {} {}\n", d.code.name(), d.offset, opt(d.end)));
            }
            for d in &report.errors {
                w.push_str(&format!("error {} {} {}\n", d.code.name(), d.offset, opt(d.end)));
            }
        }
    }
    for x in &report.warnings {
        w.push_str(&format!("warning {} {} {}\n", x.code.name(), x.offset, opt(x.end)));
    }
    w
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut dialect = 0u8;
    let mut tree = false;
    let mut warnings = false;
    let mut diag = false;
    let mut diag_all = false;
    let mut table_list: Option<(String, String)> = None;
    let mut opts = SerializeOptions::default();
    let mut files = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dialect" | "-i" => dialect = dialect_of(&args.next().expect("a dialect")),
            "--tree" => tree = true,
            "--warnings" => warnings = true,
            "--diag" => diag = true,
            "--diag-all" => {
                diag = true;
                diag_all = true;
            }
            "--width" => opts.width = args.next().expect("a width").parse().expect("a number"),
            "--indent-requested" => opts.indent_requested = true,
            "--strip-comments" => opts.strip_comments = true,
            "--from-json" => {
                // Print a JSON file's tree as figl: the printer on a foreign
                // tree, as `fig convert -i json -o figl` does.
                let path = args.next().expect("a file");
                let input = std::fs::read(&path).expect("a readable file");
                let mut b = fig_core::kit::ast::Builder::init();
                let mut j = Json { s: &input, i: 0 };
                let Some(root) = j.value(&mut b) else {
                    eprintln!("error: not JSON");
                    std::process::exit(2);
                };
                let ast = b.finish(root).unwrap();
                let mut out = Vec::new();
                match figl::LANGUAGE.print(0, &ast, &opts, &mut out) {
                    Ok(()) => std::io::stdout().write_all(&out).unwrap(),
                    Err(e) => {
                        eprintln!("print error: {}", e.name());
                        std::process::exit(1);
                    }
                }
                return;
            }
            "--table-list" => table_list = Some((args.next().expect("a list"), args.next().expect("an out dir"))),
            _ => files.push(a),
        }
    }
    if let Some((list, out)) = table_list {
        let list = std::fs::read_to_string(list).expect("a readable list");
        for (i, path) in list.split('\n').enumerate() {
            if path.is_empty() {
                continue;
            }
            let input = std::fs::read(path).expect("a readable file");
            std::fs::write(format!("{out}/{i}.txt"), table_dump(&input)).expect("a writable out dir");
        }
        return;
    }
    let lang = &figl::LANGUAGE;
    let many = files.len() > 1;
    let mut failed = false;
    let stdout = std::io::stdout();
    let mut so = stdout.lock();
    for f in &files {
        let input = std::fs::read(f).expect("a readable file");
        let mut report = ParseReport::default();
        let parsed = lang.parse_with_report(&input, dialect, &mut report);
        if many {
            writeln!(so, "==> {f} <==").unwrap();
        }
        if warnings {
            for d in &report.errors {
                eprintln!("error {} {:?} {}", d.offset, d.end, String::from_utf8_lossy(&d.short_label));
            }
            for d in &report.warnings {
                eprintln!("warning {} {:?} {}", d.offset, d.end, String::from_utf8_lossy(&d.short_label));
            }
        }
        if diag {
            let rows: Vec<(&str, &fig_core::kit::language::ParseDiagnostic)> = if parsed.is_err() {
                report.errors.iter().take(if diag_all { usize::MAX } else { 1 }).map(|d| ("error", d)).collect()
            } else {
                report.warnings.iter().map(|d| ("warning", d)).collect()
            };
            for (sev, d) in rows {
                let loc = fig_core::kit::parse_diagnostic::locate_offset(&input, d.offset);
                let width = match d.end {
                    Some(e) if e > d.offset => e - d.offset,
                    _ => 0,
                };
                eprintln!(
                    "D\t{sev}\t{}\t{}:{}\t{}\t{width}",
                    String::from_utf8_lossy(&d.message),
                    loc.line,
                    loc.column,
                    String::from_utf8_lossy(&d.short_label)
                );
            }
        }
        match parsed {
            Err(e) => {
                failed = true;
                if many {
                    writeln!(so, "error: {}", e.name()).unwrap();
                } else {
                    eprintln!("error: {}", e.name());
                }
            }
            Ok(doc) => {
                let mut out = Vec::new();
                if tree {
                    dump(&doc.ast, doc.ast.root, 0, &mut out);
                    so.write_all(&out).unwrap();
                    continue;
                }
                match lang.print(dialect, &doc.ast, &opts, &mut out) {
                    Ok(()) => so.write_all(&out).unwrap(),
                    Err(e) => {
                        failed = true;
                        if many {
                            writeln!(so, "print error: {}", e.name()).unwrap();
                        } else {
                            eprintln!("print error: {}", e.name());
                        }
                    }
                }
            }
        }
    }
    if failed && !many {
        std::process::exit(1);
    }
}
