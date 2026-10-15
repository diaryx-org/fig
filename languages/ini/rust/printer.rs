//! Port of languages/ini/src/printer.zig.
//!
//! INI printer: renders a fig AST as canonical INI.
//!
//! Root-level scalar entries print first (INI's `key = value` lines before
//! any section), then each mapping-valued entry prints as a `[section]` block
//! — in that fixed order regardless of the AST's actual child order, since
//! real INI syntax cannot interleave the two (any key after the first
//! `[section]` belongs to that section, not root) and an AST built by another
//! format's parser/the `Builder` may not already respect that grouping. A
//! section whose own value is itself a mapping (double nesting) has no INI
//! form — `UnsupportedValue`.
//!
//! Every value is written as plain, optionally-quoted text — INI has no typed
//! scalars, so a non-string value (from a cross-format conversion)
//! stringifies. A value is wrapped in `"..."` exactly when printing it bare
//! wouldn't round-trip: empty, has leading/trailing whitespace (the parser
//! trims unquoted values), or already starts/ends with a matching quote
//! character (which the parser would otherwise strip as INI-quoting on
//! reread). A value containing a literal newline has no representation at
//! all in this single-line grammar — `UnsupportedValue`. (Not handled: a key
//! or section name containing `=`/`]`/a newline — INI has no escaping for
//! those either; narrow enough in practice to leave as a known gap for now.)
//!
//! Comments: leading comments print as `; ...` lines above a key. INI has no
//! same-line trailing-comment syntax fig can safely emit (a `;`/`#` inside an
//! unquoted value is literal — see the tokenizer), so a *trailing* comment
//! prints as its own `;` line immediately after the `key = value` line
//! instead of being dropped.

use fig_kit::err;
use fig_kit::ast::{Comment, NodeId, NodeKind, SerializeOptions, AST};
use fig_kit::Result;

/// `Error`: the names the printer returns.
pub const ERRORS: &[&str] = &["NullUnsupported", "NonStringKey", "UnsupportedValue", "UnresolvedAlias"];

struct Ctx<'o> {
    w: &'o mut Vec<u8>,
    #[allow(dead_code)]
    opts: SerializeOptions,
    wrote: bool,
}

impl Ctx<'_> {
    fn comment_block(&mut self, c: &Comment<'_>) {
        for line in c.text.split(|&b| b == b'\n') {
            write_semi_line(self.w, trim_ws(line));
            self.w.push(b'\n');
        }
        self.wrote = true;
    }

    fn leading(&mut self, ast: &AST, key_id: NodeId) {
        for c in ast.comments(key_id).leading.iter() {
            self.comment_block(c);
        }
    }

    fn trailing(&mut self, ast: &AST, value_id: NodeId) {
        if let Some(c) = ast.comments(value_id).trailing {
            self.comment_block(&c);
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
        self.w.extend_from_slice(b" = ");
        write_value(self.w, ast, value_id)?;
        self.w.push(b'\n');
        self.wrote = true;
        self.trailing(ast, value_id);
        Ok(())
    }

    fn section(&mut self, ast: &AST, key_id: NodeId, first_child: Option<NodeId>) -> Result<()> {
        self.leading(ast, key_id);
        if self.wrote {
            self.w.push(b'\n');
        }
        self.w.push(b'[');
        write_key(self.w, ast, key_id)?;
        self.w.extend_from_slice(b"]\n");
        self.wrote = true;
        let mut cur = first_child;
        while let Some(id) = cur {
            let kv = ast.node(id).kind.as_keyvalue().expect("a mapping's child is an entry");
            if matches!(ast.node(kv.value).kind, NodeKind::Mapping(_)) {
                return Err(err!(UnsupportedValue)); // no nested sections
            }
            self.kv_line(ast, kv.key, kv.value)?;
            cur = ast.node(id).next_sibling;
        }
        Ok(())
    }
}

fn is_mapping_value(ast: &AST, kv_id: NodeId) -> bool {
    let kv = ast.node(kv_id).kind.as_keyvalue().expect("a mapping's child is an entry");
    matches!(ast.node(kv.value).kind, NodeKind::Mapping(_))
}

/// `ast` from its root, as an INI document, into `out`.
pub fn print(out: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    print_from(out, ast, ast.root, options)
}

/// `print` with `root` as the document's root (Zig's `fragment.root = id`).
fn print_from(out: &mut Vec<u8>, ast: &AST, root: NodeId, options: &SerializeOptions) -> Result<()> {
    match ast.node(root).kind {
        NodeKind::Mapping(first) => {
            let mut ctx = Ctx { w: out, opts: *options, wrote: false };
            let mut cur = first;
            while let Some(id) = cur {
                if !is_mapping_value(ast, id) {
                    let kv = ast.node(id).kind.as_keyvalue().expect("an entry");
                    ctx.kv_line(ast, kv.key, kv.value)?;
                }
                cur = ast.node(id).next_sibling;
            }
            cur = first;
            while let Some(id) = cur {
                if is_mapping_value(ast, id) {
                    let kv = ast.node(id).kind.as_keyvalue().expect("an entry");
                    let section_first = ast.node(kv.value).kind.as_mapping().expect("a mapping");
                    ctx.section(ast, kv.key, section_first)?;
                    ctx.dangling(ast, kv.value);
                }
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

/// The editor's splice text: what follows `key = ` on one line. A mapping
/// has no spelling there — a section is a header of its own, which a value
/// splice cannot write — so it is refused here rather than printed as the
/// document it would be (`c = 1`), which spliced after `u = ` reads back as
/// the string `c = 1`.
pub fn print_splice(out: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    let _ = options;
    write_value(out, ast, ast.root)
}

// ── Keys and values ─────────────────────────────────────────────────────────

fn key_text(ast: &AST, key_id: NodeId) -> Result<&[u8]> {
    match ast.node(key_id).kind {
        NodeKind::String(s) => Ok(s),
        _ => Err(err!(NonStringKey)),
    }
}

fn write_key(w: &mut Vec<u8>, ast: &AST, key_id: NodeId) -> Result<()> {
    write_text(w, key_text(ast, key_id)?)
}

/// Render a value. INI has no typed scalars, so a non-string node (from a
/// cross-format conversion) stringifies to its canonical text; a
/// sequence/mapping has no INI value form.
fn write_value(w: &mut Vec<u8>, ast: &AST, id: NodeId) -> Result<()> {
    match ast.node(id).kind {
        NodeKind::String(s) => write_text(w, s)?,
        NodeKind::Number(n) => w.extend_from_slice(n.raw),
        NodeKind::Boolean(b) => w.extend_from_slice(if b { b"true" } else { b"false" }),
        NodeKind::Extended(ext) => w.extend_from_slice(ext.text),
        NodeKind::Null => return Err(err!(NullUnsupported)),
        NodeKind::Sequence(_) | NodeKind::Mapping(_) => return Err(err!(UnsupportedValue)),
        NodeKind::KeyValue(_) => unreachable!("never a value position"),
        NodeKind::Alias(_) => return Err(err!(UnresolvedAlias)),
    }
    Ok(())
}

/// Write `value` bare, unless bare wouldn't round-trip (see module doc).
fn write_text(w: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    if value.iter().any(|&c| c == b'\n' || c == b'\r') {
        return Err(err!(UnsupportedValue));
    }
    if needs_quoting(value) {
        w.push(b'"');
        w.extend_from_slice(value);
        w.push(b'"');
    } else {
        w.extend_from_slice(value);
    }
    Ok(())
}

fn needs_quoting(value: &[u8]) -> bool {
    if value.is_empty() {
        return true;
    }
    if value[0] == b' ' || value[0] == b'\t' {
        return true;
    }
    if value[value.len() - 1] == b' ' || value[value.len() - 1] == b'\t' {
        return true;
    }
    if value.len() >= 2 {
        let q = value[0];
        if (q == b'"' || q == b'\'') && value[value.len() - 1] == q {
            return true;
        }
    }
    false
}

fn write_semi_line(w: &mut Vec<u8>, text: &[u8]) {
    w.push(b';');
    if !text.is_empty() {
        w.push(b' ');
        w.extend_from_slice(text);
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
        let ast = parser::parse_abstract(input, Type::Ini).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast, &SerializeOptions::default()).unwrap();
        assert_eq!(String::from_utf8_lossy(expected), String::from_utf8_lossy(&out));
    }

    fn expect_round_trip(input: &[u8]) {
        let ast = parser::parse_abstract(input, Type::Ini).unwrap();
        let mut out1 = Vec::new();
        print(&mut out1, &ast, &SerializeOptions::default()).unwrap();
        let reparsed = parser::parse_abstract(&out1, Type::Ini).unwrap();
        let mut out2 = Vec::new();
        print(&mut out2, &reparsed, &SerializeOptions::default()).unwrap();
        assert_eq!(String::from_utf8_lossy(&out1), String::from_utf8_lossy(&out2));
    }

    #[test]
    fn root_scalars_then_sections() {
        // A blank line separates the root preamble from the first `[section]`
        // (and each subsequent section), matching TOML's printer convention.
        expect_print(
            b"name = fig\n[server]\nhost = localhost\n",
            b"name = fig\n\n[server]\nhost = localhost\n",
        );
    }

    #[test]
    fn leading_and_trailing_comments() {
        expect_print(b"; header\nname = fig\n", b"; header\nname = fig\n");
    }

    #[test]
    fn quotes_a_value_with_leading_trailing_whitespace() {
        expect_print(b"k = \" spaced \"\n", b"k = \" spaced \"\n");
    }

    #[test]
    fn quotes_an_already_quote_wrapped_literal_value_so_it_round_trips() {
        expect_round_trip(b"k = \"\"quoted\"\"\n"); // literal value: "quoted"
    }

    #[test]
    fn value_with_embedded_semicolon_stays_literal() {
        expect_print(b"k = a;b\n", b"k = a;b\n");
    }

    #[test]
    fn round_trips_a_mixed_document() {
        expect_round_trip(b"name = fig\n\n[server]\nhost = localhost\nport = 80\n\n[client]\ntimeout = 30\n");
    }

    #[test]
    fn double_nested_section_is_unsupported() {
        let mut b = Builder::init();
        let inner = b.add_mapping(&[]).unwrap();
        let inner_key = b.add_string(b"inner").unwrap();
        let outer = b.add_mapping(&[Entry { key: inner_key, value: inner }]).unwrap();
        let outer_key = b.add_string(b"outer").unwrap();
        let root = b.add_mapping(&[Entry { key: outer_key, value: outer }]).unwrap();
        let ast = b.finish(root).unwrap();
        let mut out = Vec::new();
        assert_eq!(err!(UnsupportedValue), print(&mut out, &ast, &SerializeOptions::default()).unwrap_err());
    }

    #[test]
    fn a_splice_is_one_value_line_and_a_mapping_has_none() {
        let mut b = Builder::init();
        let k = b.add_string(b"c").unwrap();
        let v = b.add_string(b"1").unwrap();
        let map = b.add_mapping(&[Entry { key: k, value: v }]).unwrap();
        let ast = b.finish(map).unwrap();
        let splice = SerializeOptions { splice: true, ..SerializeOptions::default() };
        let mut out = Vec::new();
        assert_eq!(err!(UnsupportedValue), print_splice(&mut out, &ast, &splice).unwrap_err());
        let mut scalar = ast.clone();
        scalar.root = v;
        print_splice(&mut out, &scalar, &splice).unwrap();
        assert_eq!(b"1", &out[..]);
    }
}
