//! Port of languages/dotenv/src/printer.zig.
//!
//! dotenv printer: renders a fig AST as canonical `.env`.
//!
//! Flat only: every root entry prints as `KEY=value`; a nested
//! mapping/sequence value has no dotenv form at all (unlike INI, which can
//! at least hold one level of `[section]` nesting) — `UnsupportedValue`.
//! A key must already be a valid bash identifier (`[A-Za-z_][A-Za-z0-9_]*`,
//! matching the parser's grammar) — anything else, from a cross-format
//! conversion, is `InvalidKey`.
//!
//! A value prints bare when that's unambiguous, else double-quoted with
//! escapes (`\n \t \r \\ \"`) — this printer never emits a literal embedded
//! newline even though the parser can read one back (a canonical single-line-
//! per-entry form is friendlier output; see `needs_quoting`/`write_text`).
//! Comments: leading comments print as `# ...` lines above a key; UNLIKE INI,
//! dotenv's grammar has a real trailing-comment form (a `#` after a value,
//! gated by preceding whitespace — see the tokenizer), so a trailing comment
//! prints inline as `KEY=value # ...`, not pushed to its own line.

use fig_kit::err;
use fig_kit::ast::{Comment, NodeId, NodeKind, SerializeOptions, AST};
use fig_kit::Result;

/// `Error`: the names the printer returns.
pub const ERRORS: &[&str] = &["NullUnsupported", "NonStringKey", "InvalidKey", "UnsupportedValue", "UnresolvedAlias"];

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
        if let Some(c) = ast.comments(value_id).trailing {
            self.w.extend_from_slice(b" #");
            if !c.text.is_empty() {
                self.w.push(b' ');
                for &ch in c.text {
                    self.w.push(if ch == b'\n' { b' ' } else { ch });
                }
            }
        }
        self.w.push(b'\n');
        Ok(())
    }
}

/// `ast` from its root, as a dotenv document, into `out`.
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

fn is_ident_start(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphabetic()
}
fn is_ident_char(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}
fn is_identifier(name: &[u8]) -> bool {
    if name.is_empty() || !is_ident_start(name[0]) {
        return false;
    }
    name[1..].iter().all(|&c| is_ident_char(c))
}

fn write_key(w: &mut Vec<u8>, ast: &AST, key_id: NodeId) -> Result<()> {
    let name = match ast.node(key_id).kind {
        NodeKind::String(s) => s,
        _ => return Err(err!(NonStringKey)),
    };
    if !is_identifier(name) {
        return Err(err!(InvalidKey));
    }
    w.extend_from_slice(name);
    Ok(())
}

/// Render a value. dotenv has no typed scalars, so a non-string node (from a
/// cross-format conversion) stringifies to its canonical text; a
/// sequence/mapping has no dotenv value form at all (not even one level, per
/// the module doc).
fn write_value(w: &mut Vec<u8>, ast: &AST, id: NodeId) -> Result<()> {
    match ast.node(id).kind {
        NodeKind::String(s) => write_text(w, s),
        NodeKind::Number(n) => w.extend_from_slice(n.raw),
        NodeKind::Boolean(b) => w.extend_from_slice(if b { b"true" } else { b"false" }),
        NodeKind::Extended(ext) => write_text(w, ext.text),
        NodeKind::Null => return Err(err!(NullUnsupported)),
        NodeKind::Sequence(_) | NodeKind::Mapping(_) => return Err(err!(UnsupportedValue)),
        NodeKind::KeyValue(_) => unreachable!("never a value position"),
        NodeKind::Alias(_) => return Err(err!(UnresolvedAlias)),
    }
    Ok(())
}

fn write_text(w: &mut Vec<u8>, value: &[u8]) {
    if !needs_quoting(value) {
        w.extend_from_slice(value);
        return;
    }
    w.push(b'"');
    for &c in value {
        match c {
            b'\n' => w.extend_from_slice(b"\\n"),
            b'\r' => w.extend_from_slice(b"\\r"),
            b'\t' => w.extend_from_slice(b"\\t"),
            b'"' => w.extend_from_slice(b"\\\""),
            b'\\' => w.extend_from_slice(b"\\\\"),
            _ => w.push(c),
        }
    }
    w.push(b'"');
}

/// An empty value is fine bare (`KEY=`); anything else needs quoting if it
/// has leading/trailing whitespace (would be trimmed back on reread) or
/// contains a byte this printer would otherwise have to worry about
/// disambiguating (newline, `"`, `\`, or `#` — conservative: quoting is
/// always SAFE, so any `#` triggers it even though some positions would
/// technically round-trip unquoted too).
fn needs_quoting(value: &[u8]) -> bool {
    if value.is_empty() {
        return false;
    }
    if value[0] == b' ' || value[0] == b'\t' || value[value.len() - 1] == b' ' || value[value.len() - 1] == b'\t' {
        return true;
    }
    value.iter().any(|&c| c == b'\n' || c == b'\r' || c == b'"' || c == b'\\' || c == b'#')
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
        let ast = parser::parse_abstract(input, Type::Dotenv).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast, &SerializeOptions::default()).unwrap();
        assert_eq!(String::from_utf8_lossy(expected), String::from_utf8_lossy(&out));
    }

    fn expect_round_trip(input: &[u8]) {
        let ast = parser::parse_abstract(input, Type::Dotenv).unwrap();
        let mut out1 = Vec::new();
        print(&mut out1, &ast, &SerializeOptions::default()).unwrap();
        let reparsed = parser::parse_abstract(&out1, Type::Dotenv).unwrap();
        let mut out2 = Vec::new();
        print(&mut out2, &reparsed, &SerializeOptions::default()).unwrap();
        assert_eq!(String::from_utf8_lossy(&out1), String::from_utf8_lossy(&out2));
    }

    #[test]
    fn plain_values_print_bare() {
        expect_print(b"NAME=fig\n", b"NAME=fig\n");
    }

    #[test]
    fn a_value_needing_quoting_round_trips_through_double_quotes() {
        expect_round_trip(b"MSG=\"line1\\nline2\"\n");
        expect_round_trip(b"A=' leading space'\n");
    }

    #[test]
    fn trailing_comment_prints_inline() {
        expect_print(b"A=1 # note\n", b"A=1 # note\n");
    }

    #[test]
    fn leading_comment_prints_as_a_hash_line() {
        expect_print(b"# header\nA=1\n", b"# header\nA=1\n");
    }

    #[test]
    fn round_trips_a_mixed_document() {
        expect_round_trip(b"# config\nNAME=fig\nPATH='C:\\Users\\bob'\nMSG=\"hi \\\"there\\\"\"\nEMPTY=\n");
    }

    #[test]
    fn a_non_identifier_key_is_unrepresentable() {
        let mut b = Builder::init();
        let v = b.add_string(b"x").unwrap();
        let k = b.add_string(b"not-an-ident").unwrap();
        let root = b.add_mapping(&[Entry { key: k, value: v }]).unwrap();
        let ast = b.finish(root).unwrap();
        let mut out = Vec::new();
        assert_eq!(err!(InvalidKey), print(&mut out, &ast, &SerializeOptions::default()).unwrap_err());
    }

    #[test]
    fn a_nested_mapping_value_is_unsupported() {
        let mut b = Builder::init();
        let inner = b.add_mapping(&[]).unwrap();
        let k = b.add_string(b"SERVER").unwrap();
        let root = b.add_mapping(&[Entry { key: k, value: inner }]).unwrap();
        let ast = b.finish(root).unwrap();
        let mut out = Vec::new();
        assert_eq!(err!(UnsupportedValue), print(&mut out, &ast, &SerializeOptions::default()).unwrap_err());
    }
}
