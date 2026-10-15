//! Differential check for the TOML port: for each file given, parse it with
//! the Rust TOML language and print either the reprint (parse, then `print`
//! in the same dialect with default `SerializeOptions`) or the parse error's
//! Zig name.
//!
//! Usage: `check_toml [--spec 1.0|1.1] [--width N] [--canon | --table] <file>...`
//!
//! With one file, the reprint goes to stdout as is (exit 0), or the error
//! name to stderr (exit 1). With several, each file's result is a record:
//! `=== <path>` then `OK` and the reprint, or `ERR <name>`, so a script can
//! compare a whole corpus in one run. `--canon` prints a tree dump instead
//! of the reprint, for eyeballing against `fig get -o canonical`; `--table`
//! the node table as `fig lang table` prints it. A `.rows` file is a tree
//! from another format (see `print_rows`), printed as TOML.

use fig_core::kit::ast::{NodeId, NodeKind, SerializeOptions, AST};
use fig_core::kit::language::{Dialect, Language, ParseReport};
use fig_core::lang::toml::LANGUAGE;

fn dump(ast: &AST, id: NodeId, depth: usize, out: &mut Vec<u8>) {
    let pad = "  ".repeat(depth);
    let c = ast.comments(id);
    for l in c.leading.iter() {
        out.extend_from_slice(format!("{pad}# leading {:?}\n", String::from_utf8_lossy(l.text)).as_bytes());
    }
    match ast.node(id).kind {
        NodeKind::Mapping(first) => {
            out.extend_from_slice(format!("{pad}map\n").as_bytes());
            let mut cur = first;
            while let Some(kv) = cur {
                dump(ast, kv, depth + 1, out);
                cur = ast.node(kv).next_sibling;
            }
        }
        NodeKind::Sequence(first) => {
            out.extend_from_slice(format!("{pad}seq\n").as_bytes());
            let mut cur = first;
            while let Some(e) = cur {
                dump(ast, e, depth + 1, out);
                cur = ast.node(e).next_sibling;
            }
        }
        NodeKind::KeyValue(kv) => {
            out.extend_from_slice(format!("{pad}kv\n").as_bytes());
            dump(ast, kv.key, depth + 1, out);
            dump(ast, kv.value, depth + 1, out);
        }
        k => out.extend_from_slice(format!("{pad}{k:?}\n").as_bytes()),
    }
    if let Some(t) = c.trailing {
        out.extend_from_slice(format!("{pad}# trailing {:?}\n", String::from_utf8_lossy(t.text)).as_bytes());
    }
    for l in c.dangling.iter() {
        out.extend_from_slice(format!("{pad}# dangling {:?}\n", String::from_utf8_lossy(l.text)).as_bytes());
    }
}

fn json_str(out: &mut Vec<u8>, s: &[u8]) {
    out.push(b'"');
    for ch in String::from_utf8_lossy(s).chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            c if (c as u32) < 0x20 => out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes()),
            c => out.extend_from_slice(c.to_string().as_bytes()),
        }
    }
    out.push(b'"');
}

/// The parse's node table as `fig lang table` prints it.
fn table_json(input: &[u8], dialect: Dialect) -> Result<Vec<u8>, String> {
    use fig_core::kit::table::{self, Kind, Table, NONE};
    let mut t = Table::init(input.to_vec());
    LANGUAGE.parse_table(input, dialect, &mut t).map_err(|e| e.name().to_string())?;
    let mut o = Vec::new();
    o.extend_from_slice(b"{\"rows\":[");
    for (i, r) in t.rows.iter().enumerate() {
        if i > 0 {
            o.push(b',');
        }
        let (kind, ext) = match r.kind {
            Kind::NULL => ("null", None),
            Kind::FALSE | Kind::TRUE => ("bool", None),
            Kind::STRING => ("string", None),
            Kind::INT => ("int", None),
            Kind::FLOAT => ("float", None),
            Kind::SEQUENCE => ("sequence", None),
            Kind::MAPPING => ("mapping", None),
            Kind::KEYVALUE => ("keyvalue", None),
            Kind::ALIAS => ("alias", None),
            k => ("string", fig_core::kit::ast::ExtKind::from_ordinal(k - Kind::EXT).map(|e| e.name())),
        };
        o.extend_from_slice(format!("{{\"kind\":\"{kind}\"").as_bytes());
        if let Some(e) = ext {
            o.extend_from_slice(format!(",\"ext_kind\":\"{e}\"").as_bytes());
        }
        let parent = if r.parent == NONE { "null".to_string() } else { r.parent.to_string() };
        o.extend_from_slice(format!(",\"parent\":{parent},\"span\":[{},{}]", r.start, r.end).as_bytes());
        let text: Option<Vec<u8>> = match r.kind {
            Kind::FALSE => Some(b"false".to_vec()),
            Kind::TRUE => Some(b"true".to_vec()),
            _ => t.text_of(r.text).map(|x| x.to_vec()),
        };
        if let Some(x) = text {
            o.extend_from_slice(b",\"text\":");
            json_str(&mut o, &x);
        }
        o.push(b'}');
    }
    o.extend_from_slice(b"],\"regions\":[");
    for (i, r) in t.regions.iter().enumerate() {
        if i > 0 {
            o.push(b',');
        }
        o.extend_from_slice(format!("{{\"node\":{},\"start\":{},\"end\":{}}}", r.node, r.start, r.end).as_bytes());
    }
    o.extend_from_slice(b"],\"mentions\":[");
    for (i, m) in t.mentions.iter().enumerate() {
        if i > 0 {
            o.push(b',');
        }
        let k = ["header", "entry", "index", "append"][m.kind as usize];
        o.extend_from_slice(format!("{{\"node\":{},\"span\":[{},{}],\"kind\":\"{k}\"}}", m.node, m.start, m.end).as_bytes());
    }
    o.extend_from_slice(b"],\"comments\":[");
    for (i, c) in t.comments.iter().enumerate() {
        if i > 0 {
            o.push(b',');
        }
        let slot = match c.slot {
            table::COMMENT_LEADING => "leading",
            table::COMMENT_TRAILING => "trailing",
            _ => "dangling",
        };
        let style = if c.style == table::COMMENT_LINE { "line" } else { "block" };
        o.extend_from_slice(format!("{{\"node\":{},\"slot\":\"{slot}\",\"style\":\"{style}\",\"text\":", c.node).as_bytes());
        json_str(&mut o, t.text_of(c.text).unwrap_or(b""));
        o.push(b'}');
    }
    o.extend_from_slice(b"]}\n");
    Ok(o)
}

fn unhex(s: &str) -> Vec<u8> {
    if s == "-" {
        return Vec::new();
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// A tree from another format, as a print receives it: a table in the line
/// format `R <kind> <parent|-> <hex text|-> [<ext kind>]` and
/// `C <node> <slot> <style> <hex text>` (a script writes it from
/// `fig lang table`), printed as TOML.
fn print_rows(path: &str, dialect: Dialect, options: &SerializeOptions) -> Result<Vec<u8>, String> {
    use fig_core::kit::ast::ExtKind;
    use fig_core::kit::table::{self, Kind, Row, Table, Text, NONE};
    let text = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let mut t = Table::default();
    for line in text.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        match f[0] {
            "R" => {
                let parent = if f[2] == "-" { NONE } else { f[2].parse().unwrap() };
                let bytes = unhex(f[3]);
                let kind = match f[1] {
                    "null" => Kind::NULL,
                    "bool" => if bytes == b"true" { Kind::TRUE } else { Kind::FALSE },
                    "string" => match f.get(4) {
                        Some(e) => Kind::EXT + ExtKind::ALL.iter().find(|k| k.name() == *e).unwrap().ordinal(),
                        None => Kind::STRING,
                    },
                    "int" => Kind::INT,
                    "float" => Kind::FLOAT,
                    "sequence" => Kind::SEQUENCE,
                    "mapping" => Kind::MAPPING,
                    "keyvalue" => Kind::KEYVALUE,
                    "alias" => Kind::ALIAS,
                    k => panic!("kind {k}"),
                };
                let has_text = f[3] != "-" && !matches!(kind, Kind::TRUE | Kind::FALSE);
                let tx = if has_text { t.add_string(&bytes).unwrap() } else { Text::ABSENT };
                t.add_row(Row { kind, parent, text: tx, ..Row::default() }).unwrap();
            }
            "C" => {
                let node: u32 = f[1].parse().unwrap();
                let slot = match f[2] {
                    "leading" => table::COMMENT_LEADING,
                    "trailing" => table::COMMENT_TRAILING,
                    _ => table::COMMENT_DANGLING,
                };
                let style = if f[3] == "line" { table::COMMENT_LINE } else { table::COMMENT_BLOCK };
                let tx = t.add_string(&unhex(f[4])).unwrap();
                t.comments.push(table::Comment { node, slot, style, text: tx });
            }
            _ => {}
        }
    }
    if options.strip_comments {
        t.comments.clear();
    }
    // The child links a print's table carries: each row's parent's children
    // are the rows after it that name it, in order.
    let n = t.rows.len();
    let mut last = vec![NONE; n];
    for i in 0..n {
        let p = t.rows[i].parent;
        if p != NONE {
            let p = p as usize;
            if last[p] == NONE {
                t.rows[p].first_child = i as u32;
            } else {
                let l = last[p] as usize;
                t.rows[l].next_sibling = i as u32;
            }
            last[p] = i as u32;
        }
    }
    let doc = fig_core::kit::convert::view_to_document(std::sync::Arc::from(&b""[..]), &t.view()).map_err(|e| e.name().to_string())?;
    let mut out = Vec::new();
    LANGUAGE.print(dialect, &doc.ast, options, &mut out).map_err(|e| e.name().to_string())?;
    Ok(out)
}

fn run(path: &str, dialect: Dialect, options: &SerializeOptions, canon: bool, as_table: bool) -> Result<Vec<u8>, String> {
    if path.ends_with(".rows") {
        return print_rows(path, dialect, options);
    }
    let input = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    if as_table {
        return table_json(&input, dialect);
    }
    let mut report = ParseReport::default();
    let mut doc = LANGUAGE.parse_with_report(&input, dialect, &mut report).map_err(|e| {
        // The name, the teaching message the Zig CLI prints, and where.
        let at = report.errors.first().map(|d| {
            let loc = fig_core::kit::parse_diagnostic::locate_offset(&input, d.offset);
            format!("{}:{}", loc.line, loc.column)
        });
        format!("{}\t{}\t{}", e.name(), fig_core::lang::toml::parser::describe(e), at.unwrap_or_default())
    })?;
    let mut out = Vec::new();
    if canon {
        dump(&doc.ast, doc.ast.root, 0, &mut out);
        return Ok(out);
    }
    if options.strip_comments {
        doc.ast.strip_comments();
    }
    LANGUAGE.print(dialect, &doc.ast, options, &mut out).map_err(|e| e.name().to_string())?;
    Ok(out)
}

fn main() {
    let mut dialect: Dialect = LANGUAGE.default_dialect();
    let mut options = SerializeOptions::default();
    let mut canon = false;
    let mut as_table = false;
    let mut files: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--spec" => {
                let v = args.next().expect("--spec needs a version");
                dialect = match v.as_str() {
                    "1.0" | "1.0.0" => 0,
                    "1.1" | "1.1.0" => 1,
                    _ => panic!("unknown spec {v}"),
                };
            }
            "--width" => options.width = args.next().expect("--width needs N").parse().expect("a number"),
            "--indent" => options.indent = args.next().expect("--indent needs N").parse().expect("a number"),
            "--compact" => options.pretty = false,
            // Core strips before printing (`serialize`'s comment view); so does this.
            "--strip-comments" => options.strip_comments = true,
            "--canon" => canon = true,
            "--table" => as_table = true,
            _ => files.push(a),
        }
    }
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    if files.len() == 1 {
        match run(&files[0], dialect, &options, canon, as_table) {
            Ok(out) => stdout.write_all(&out).unwrap(),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    for f in &files {
        writeln!(stdout, "=== {f}").unwrap();
        match run(f, dialect, &options, canon, as_table) {
            Ok(out) => {
                writeln!(stdout, "OK").unwrap();
                stdout.write_all(&out).unwrap();
            }
            Err(e) => writeln!(stdout, "ERR {e}").unwrap(),
        }
    }
}
