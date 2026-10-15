//! Port of languages/properties/src/printer.zig.
//!
//! `.properties` printer: renders a fig AST as canonical Java `.properties`.
//!
//! Flat only, like dotenv: every root entry prints as `key=value`; a nested
//! mapping/sequence value has no `.properties` form — `UnsupportedValue`.
//! Always uses `=` as the separator (never `:` or bare whitespace — those are
//! reader-only conveniences, see `parser.rs`).
//!
//! Escaping: `\n \r \t \f \\` always escape (a raw newline/CR can never
//! appear in a key/value span at all — see the tokenizer; a literal one must
//! round-trip through the `\n`/`\r` escape, not a physical line break). A
//! key ALSO escapes `=`, `:`, and any whitespace (those are what terminate
//! key-scanning — see the tokenizer's `is_separator`) wherever they appear,
//! plus a LEADING `#`/`!` specifically (so it can't be misread as starting a
//! comment line on reread). A value only escapes LEADING whitespace (the
//! parser would otherwise skip it as post-separator padding); `=`/`:`/`#`/`!`
//! are never special inside a value and print bare.

use fig_kit::err;
use fig_kit::ast::{Comment, NodeId, NodeKind, SerializeOptions, AST};
use fig_kit::Result;

/// `Error`: the names the printer returns.
pub const ERRORS: &[&str] = &["NullUnsupported", "NonStringKey", "UnsupportedValue", "UnresolvedAlias"];

struct Ctx<'o> {
    w: &'o mut Vec<u8>,
    #[allow(dead_code)]
    opts: SerializeOptions,
}

impl Ctx<'_> {
    fn comment_block(&mut self, c: &Comment<'_>) {
        for line in c.text.split(|&b| b == b'\n') {
            self.w.push(b'#');
            let trimmed = trim_ws(line);
            if !trimmed.is_empty() {
                self.w.push(b' ');
                self.w.extend_from_slice(trimmed);
            }
            self.w.push(b'\n');
        }
    }

    fn leading(&mut self, ast: &AST, key_id: NodeId) {
        for c in ast.comments(key_id).leading.iter() {
            self.comment_block(c);
        }
    }

    fn dangling(&mut self, ast: &AST, id: NodeId) {
        for c in ast.comments(id).dangling.iter() {
            self.comment_block(c);
        }
    }

    fn kv_line(&mut self, ast: &AST, key_id: NodeId, value_id: NodeId) -> Result<()> {
        self.leading(ast, key_id);
        write_key(self.w, ast, key_id)?;
        self.w.push(b'=');
        write_value(self.w, ast, value_id)?;
        self.w.push(b'\n');
        Ok(())
    }
}

/// `ast` from its root, as a `.properties` document, into `out`.
pub fn print(out: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    print_from(out, ast, ast.root, options)
}

/// `print` with `root` as the document's root (Zig's `fragment.root = id`).
fn print_from(out: &mut Vec<u8>, ast: &AST, root: NodeId, options: &SerializeOptions) -> Result<()> {
    match ast.node(root).kind {
        NodeKind::Mapping(first) => {
            let mut ctx = Ctx { w: out, opts: *options };
            let mut cur = first;
            while let Some(id) = cur {
                let kv = ast.node(id).kind.as_keyvalue().expect("a mapping's child is an entry");
                ctx.kv_line(ast, kv.key, kv.value)?;
                cur = ast.node(id).next_sibling;
            }
            ctx.dangling(ast, root);
        }
        _ => {
            write_value(out, ast, root)?;
            out.push(b'\n');
        }
    }
    Ok(())
}

/// Print the subtree at `id` as a standalone fragment: a mapping prints as a
/// full document rooted there, anything else prints as a bare value line.
pub fn print_node(out: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize, options: &SerializeOptions) -> Result<()> {
    let _ = depth;
    match ast.node(id).kind {
        NodeKind::Mapping(_) => print_from(out, ast, id, options),
        _ => {
            write_value(out, ast, id)?;
            out.push(b'\n');
            Ok(())
        }
    }
}

// ── Keys and values ─────────────────────────────────────────────────────────

fn is_key_special(c: u8) -> bool {
    c == b'=' || c == b':' || c == b' ' || c == b'\t' || c == 0x0c
}

fn write_escaped_byte(w: &mut Vec<u8>, c: u8) {
    match c {
        b'\n' => w.extend_from_slice(b"\\n"),
        b'\r' => w.extend_from_slice(b"\\r"),
        b'\t' => w.extend_from_slice(b"\\t"),
        0x0c => w.extend_from_slice(b"\\f"),
        b'\\' => w.extend_from_slice(b"\\\\"),
        _ => {
            w.push(b'\\');
            w.push(c);
        }
    }
}

#[allow(clippy::if_same_then_else)] // the Zig's two escaping cases, kept apart
fn write_key(w: &mut Vec<u8>, ast: &AST, key_id: NodeId) -> Result<()> {
    let name = match ast.node(key_id).kind {
        NodeKind::String(s) => s,
        _ => return Err(err!(NonStringKey)),
    };
    for (idx, &c) in name.iter().enumerate() {
        if idx == 0 && (c == b'#' || c == b'!') {
            write_escaped_byte(w, c);
        } else if c == b'\n' || c == b'\r' || c == b'\t' || c == 0x0c || c == b'\\' || is_key_special(c) {
            write_escaped_byte(w, c);
        } else {
            w.push(c);
        }
    }
    Ok(())
}

/// Render a value. `.properties` has no typed scalars, so a non-string node
/// (from a cross-format conversion) stringifies to its canonical text; a
/// sequence/mapping has no `.properties` value form at all.
fn write_value(w: &mut Vec<u8>, ast: &AST, id: NodeId) -> Result<()> {
    match ast.node(id).kind {
        NodeKind::String(s) => write_value_text(w, s),
        NodeKind::Number(n) => w.extend_from_slice(n.raw),
        NodeKind::Boolean(b) => w.extend_from_slice(if b { b"true" } else { b"false" }),
        NodeKind::Extended(ext) => write_value_text(w, ext.text),
        NodeKind::Null => return Err(err!(NullUnsupported)),
        NodeKind::Sequence(_) | NodeKind::Mapping(_) => return Err(err!(UnsupportedValue)),
        NodeKind::KeyValue(_) => unreachable!("never a value position"),
        NodeKind::Alias(_) => return Err(err!(UnresolvedAlias)),
    }
    Ok(())
}

fn write_value_text(w: &mut Vec<u8>, value: &[u8]) {
    for (idx, &c) in value.iter().enumerate() {
        if idx == 0 && (c == b' ' || c == b'\t' || c == 0x0c) {
            write_escaped_byte(w, c);
        } else {
            match c {
                b'\n' | b'\r' | b'\t' | 0x0c | b'\\' => write_escaped_byte(w, c),
                _ => w.push(c),
            }
        }
    }
}

/// Zig's `std.mem.trim(u8, s, " \t")`.
fn trim_ws(s: &[u8]) -> &[u8] {
    let mut a = 0;
    let mut b = s.len();
    while a < b && (s[a] == b' ' || s[a] == b'\t') {
        a += 1;
    }
    while b > a && (s[b - 1] == b' ' || s[b - 1] == b'\t') {
        b -= 1;
    }
    &s[a..b]
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::parser;
    use super::super::Type;
    use super::*;
    use fig_kit::ast::{Builder, Entry};

    fn expect_print(input: &[u8], expected: &[u8]) {
        let ast = parser::parse_abstract(input, Type::Properties).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast, &SerializeOptions::default()).unwrap();
        assert_eq!(String::from_utf8_lossy(expected), String::from_utf8_lossy(&out));
    }

    fn expect_round_trip(input: &[u8]) {
        let ast = parser::parse_abstract(input, Type::Properties).unwrap();
        let mut out1 = Vec::new();
        print(&mut out1, &ast, &SerializeOptions::default()).unwrap();
        let reparsed = parser::parse_abstract(&out1, Type::Properties).unwrap();
        let mut out2 = Vec::new();
        print(&mut out2, &reparsed, &SerializeOptions::default()).unwrap();
        assert_eq!(String::from_utf8_lossy(&out1), String::from_utf8_lossy(&out2));
    }

    #[test]
    fn canonicalizes_to_equals_regardless_of_the_source_separator() {
        expect_print(b"a: 1\n", b"a=1\n");
        expect_print(b"a 1\n", b"a=1\n");
    }

    #[test]
    fn escapes_control_characters_and_a_leading_space_in_a_value() {
        expect_print(b"v=a\\tb\\nc\n", b"v=a\\tb\\nc\n");
        expect_round_trip(b"v=\\ leading\n");
    }

    #[test]
    fn escapes_a_leading_hash_or_bang_in_a_key() {
        expect_round_trip(b"\\#weird=1\n");
        expect_round_trip(b"\\!weird=1\n");
    }

    #[test]
    fn escapes_separator_like_characters_in_a_key() {
        expect_round_trip(b"a\\:b\\=c\\ d=1\n");
    }

    #[test]
    fn line_continuation_collapses_round_trips_as_one_line() {
        expect_print(b"long=part1\\\n  part2\n", b"long=part1part2\n");
    }

    #[test]
    fn leading_comment_prints_as_a_hash_line() {
        expect_print(b"# header\na=1\n", b"# header\na=1\n");
    }

    #[test]
    fn round_trips_a_mixed_document() {
        expect_round_trip(b"# config\ndb.host=localhost\ndb.port=5432\ngreeting=hi \\\\ttab\n");
    }

    #[test]
    fn a_nested_mapping_value_is_unsupported() {
        let mut b = Builder::init();
        let inner = b.add_mapping(&[]).unwrap();
        let k = b.add_string(b"server").unwrap();
        let root = b.add_mapping(&[Entry { key: k, value: inner }]).unwrap();
        let ast = b.finish(root).unwrap();
        let mut out = Vec::new();
        assert_eq!(err!(UnsupportedValue), print(&mut out, &ast, &SerializeOptions::default()).unwrap_err());
    }
}
