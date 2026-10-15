//! Port of languages/nestedtext/src/printer.zig.
//!
//! NestedText printer: renders a fig AST as canonical, always-block-style
//! NestedText (never the inline `{...}`/`[...]` forms — those are the
//! parser's business to accept, not the printer's to choose; matches how
//! INI's printer always writes `[section]` blocks rather than picking a
//! denser form).
//!
//! Every value is untyped text (NestedText has no typed scalars — same
//! simplification INI makes): a number/boolean/`extended` node stringifies
//! to its canonical text.
//!
//! Same-line (`key: value` / `- value`) vs. nested `>`-block is a
//! ROUND-TRIP-FORCED choice, not a style preference: rest-of-line values are
//! taken 100% literally on reparse (see `parser.rs`'s module doc), so
//! same-line is always safe EXCEPT for two cases forced by the grammar
//! itself: an EMPTY value has no same-line spelling at all (`key:` with
//! nothing after means "value is a nested block", and an absent nested block
//! is a parse error — not empty string; the only way to spell "" is a nested
//! bare `>` line), and a value containing a literal `\n` obviously can't fit
//! one line. A multiline KEY (`: key` form) is similarly forced whenever the
//! key is empty, starts with a byte the tokenizer would misread as a
//! different tag or a comment/indentation character, or contains a literal
//! `\n` or a `: ` substring that would confuse the key/value split scan; see
//! `needs_multiline_key`. A multiline key's value is ALWAYS a nested block
//! (never same-line — the grammar has no same-line form after a multiline
//! key), so a scalar value there is always rendered as a `>`-block even when
//! it would otherwise fit on one line.
//!
//! Comments: `#` full-line only (matches INI's own limitation) — a
//! *trailing* comment has no same-line spelling in this grammar, so it
//! prints as its own `#` line immediately after the item, same convention
//! INI's printer uses.
//!
//! Errors: `NullUnsupported`, `NonStringKey`, `UnresolvedAlias`.

use crate::err;
use crate::kit::ast::{NodeId, NodeKind, SerializeOptions, AST};
use crate::Result;

const INDENT_WIDTH: usize = 4;

pub fn print(w: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    print_at(w, ast, ast.root, options)
}

/// `print` of the tree re-rooted at `root` (Zig re-roots a shallow copy of
/// the AST; here the root is passed along).
fn print_at(w: &mut Vec<u8>, ast: &AST, root: NodeId, options: &SerializeOptions) -> Result<()> {
    match ast.node(root).kind {
        NodeKind::Null => {}
        NodeKind::Mapping(_) => print_mapping(w, ast, root, 0)?,
        NodeKind::Sequence(_) => print_sequence(w, ast, root, 0)?,
        _ => write_string_block(w, 0, scalar_text(ast, root)?),
    }
    let _ = options;
    Ok(())
}

/// Print `ast.root` as splice text — what follows `key:` or `-` in place,
/// not a document. A one-line scalar is its plain text, which the editor's
/// `render_tail`/`render_entry`/`render_item` put on the same line (or
/// block, if empty). Anything that must be NESTED — a container, or a string
/// with a line break — is a newline and then its nested block at depth 0,
/// which the renderers re-indent under the key rather than block as a
/// string. See `SerializeOptions::splice` and `edit.rs`'s `nested_block`.
pub fn print_splice(w: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    match ast.node(ast.root).kind {
        NodeKind::Null => return print(w, ast, options),
        NodeKind::Mapping(_) | NodeKind::Sequence(_) => {
            w.push(b'\n');
            return print(w, ast, options);
        }
        _ => {
            let text = scalar_text(ast, ast.root)?;
            if text.contains(&b'\n') {
                w.push(b'\n');
                write_string_block(w, 0, text);
            } else {
                w.extend_from_slice(text);
            }
        }
    }
    Ok(())
}

/// Print the subtree at `id` as a standalone fragment, re-rooting the tree —
/// same trick INI's `print_node` uses.
pub fn print_node(w: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize, options: &SerializeOptions) -> Result<()> {
    let _ = depth;
    print_at(w, ast, id, options)
}

// ── Containers ──────────────────────────────────────────────────────────────

fn print_mapping(w: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize) -> Result<()> {
    // An empty mapping has no block spelling — `key:` over nothing reads as
    // the empty string — so it takes the inline `{}` on a line of its own.
    let first = ast.node(id).kind.as_mapping().expect("a mapping");
    if first.is_none() {
        print_empty(w, ast, id, depth, b"{}");
        return Ok(());
    }
    let mut cur = first;
    while let Some(kv_id) = cur {
        let kv = ast.node(kv_id).kind.as_keyvalue().expect("a mapping's child is an entry");
        print_leading_comments(w, ast, ast.leading_comment_anchor(kv_id), depth);
        let key_text = key_text(ast, kv.key)?;
        let multiline_key = needs_multiline_key(key_text);
        if multiline_key {
            write_multiline_key_lines(w, depth, key_text);
        } else {
            write_indent(w, depth);
            w.extend_from_slice(key_text);
            w.push(b':');
        }
        print_item_value(w, ast, kv.value, depth, multiline_key)?;
        print_trailing_comment(w, ast, ast.trailing_comment_anchor(kv_id), depth);
        cur = ast.node(kv_id).next_sibling;
    }
    print_dangling_comments(w, ast, id, depth);
    Ok(())
}

fn print_sequence(w: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize) -> Result<()> {
    let first = ast.node(id).kind.as_sequence().expect("a sequence");
    if first.is_none() {
        print_empty(w, ast, id, depth, b"[]");
        return Ok(());
    }
    let mut cur = first;
    while let Some(item_id) = cur {
        print_leading_comments(w, ast, ast.leading_comment_anchor(item_id), depth);
        write_indent(w, depth);
        w.push(b'-');
        print_item_value(w, ast, item_id, depth, false)?;
        print_trailing_comment(w, ast, ast.trailing_comment_anchor(item_id), depth);
        cur = ast.node(item_id).next_sibling;
    }
    print_dangling_comments(w, ast, id, depth);
    Ok(())
}

/// An empty container as its inline form on a line of its own, after any
/// comments it carries.
fn print_empty(w: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize, inline_form: &[u8]) {
    print_dangling_comments(w, ast, id, depth);
    write_indent(w, depth);
    w.extend_from_slice(inline_form);
    w.push(b'\n');
}

/// Render the value half of a `key:`/`-` item already written up to (but
/// not including) its own line terminator. `force_nested` is set for a
/// multiline key, which has no same-line value form at all.
fn print_item_value(w: &mut Vec<u8>, ast: &AST, value_id: NodeId, depth: usize, force_nested: bool) -> Result<()> {
    // `force_nested` (a multiline key) means the key's own last line already
    // ended with its own `\n` — the nested block starts writing indented
    // lines immediately. Otherwise the current line (`key:`/`-`) is still
    // open and needs closing with `\n` before anything nested can start.
    match ast.node(value_id).kind {
        NodeKind::Mapping(_) => {
            if !force_nested {
                w.push(b'\n');
            }
            print_mapping(w, ast, value_id, depth + 1)?;
        }
        NodeKind::Sequence(_) => {
            if !force_nested {
                w.push(b'\n');
            }
            print_sequence(w, ast, value_id, depth + 1)?;
        }
        NodeKind::Alias(_) => return Err(err!(UnresolvedAlias)),
        NodeKind::KeyValue(_) => unreachable!(),
        _ => {
            let text = scalar_text(ast, value_id)?;
            if !force_nested && !text.is_empty() && !text.contains(&b'\n') {
                w.push(b' ');
                w.extend_from_slice(text);
                w.push(b'\n');
            } else {
                if !force_nested {
                    w.push(b'\n');
                }
                write_string_block(w, depth + 1, text);
            }
        }
    }
    Ok(())
}

// ── Keys and scalars ────────────────────────────────────────────────────────

fn key_text(ast: &AST, key_id: NodeId) -> Result<&[u8]> {
    match ast.node(key_id).kind {
        NodeKind::String(s) => Ok(s),
        _ => Err(err!(NonStringKey)),
    }
}

fn scalar_text(ast: &AST, id: NodeId) -> Result<&[u8]> {
    match ast.node(id).kind {
        NodeKind::String(s) => Ok(s),
        NodeKind::Number(n) => Ok(n.raw),
        NodeKind::Boolean(b) => Ok(if b { b"true" } else { b"false" }),
        NodeKind::Extended(e) => Ok(e.text),
        NodeKind::Null => Err(err!(NullUnsupported)),
        NodeKind::Alias(_) => Err(err!(UnresolvedAlias)),
        // handled by callers before reaching here
        NodeKind::Sequence(_) | NodeKind::Mapping(_) | NodeKind::KeyValue(_) => unreachable!(),
    }
}

/// Whether `key` needs the `: key` multiline form instead of plain `key:` —
/// forced whenever plain form would misparse or (for a leading space) be
/// silently absorbed into indentation on reread. See the module doc comment.
fn needs_multiline_key(key: &[u8]) -> bool {
    if key.is_empty() {
        return true;
    }
    let c0 = key[0];
    if c0 == b'#' || c0 == b'{' || c0 == b'[' || c0 == b' ' || c0 == b'\t' {
        return true;
    }
    if (c0 == b'-' || c0 == b':' || c0 == b'>') && (key.len() == 1 || key[1] == b' ') {
        return true;
    }
    if key.contains(&b'\n') {
        return true;
    }
    if key.windows(2).any(|w| w == b": ") {
        return true;
    }
    false
}

// ── Line writers ────────────────────────────────────────────────────────────

fn write_indent(w: &mut Vec<u8>, depth: usize) {
    w.resize(w.len() + depth * INDENT_WIDTH, b' ');
}

fn write_multiline_key_lines(w: &mut Vec<u8>, depth: usize, text: &[u8]) {
    for line in text.split(|&c| c == b'\n') {
        write_indent(w, depth);
        if line.is_empty() {
            w.extend_from_slice(b":\n");
        } else {
            w.extend_from_slice(b": ");
            w.extend_from_slice(line);
            w.push(b'\n');
        }
    }
}

/// One or more `> line` (or bare `>` for an empty line) lines at `depth`.
/// Splitting `""` on `\n` yields exactly one empty part, so an empty string
/// renders as a single bare `>` — the only way to spell "" in this grammar.
fn write_string_block(w: &mut Vec<u8>, depth: usize, text: &[u8]) {
    for line in text.split(|&c| c == b'\n') {
        write_indent(w, depth);
        if line.is_empty() {
            w.extend_from_slice(b">\n");
        } else {
            w.extend_from_slice(b"> ");
            w.extend_from_slice(line);
            w.push(b'\n');
        }
    }
}

fn write_hash_line(w: &mut Vec<u8>, depth: usize, text: &[u8]) {
    write_indent(w, depth);
    w.push(b'#');
    if !text.is_empty() {
        w.push(b' ');
        w.extend_from_slice(text);
    }
    w.push(b'\n');
}

/// `std.mem.trim(u8, s, " \t")`.
fn trim_hs(s: &[u8]) -> &[u8] {
    let is_hs = |c: &u8| *c == b' ' || *c == b'\t';
    let start = s.iter().position(|c| !is_hs(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !is_hs(c)).map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

fn write_comment_lines(w: &mut Vec<u8>, depth: usize, text: &[u8]) {
    for line in text.split(|&c| c == b'\n') {
        write_hash_line(w, depth, trim_hs(line));
    }
}

fn print_leading_comments(w: &mut Vec<u8>, ast: &AST, anchor_id: NodeId, depth: usize) {
    for c in ast.comments(anchor_id).leading.iter() {
        write_comment_lines(w, depth, c.text);
    }
}

fn print_trailing_comment(w: &mut Vec<u8>, ast: &AST, anchor_id: NodeId, depth: usize) {
    if let Some(c) = ast.comments(anchor_id).trailing {
        write_comment_lines(w, depth, c.text);
    }
}

fn print_dangling_comments(w: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize) {
    for c in ast.comments(id).dangling.iter() {
        write_comment_lines(w, depth, c.text);
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::parser;
    use super::super::Type;
    use super::*;
    use crate::kit::ast::{Builder, Entry};

    fn expect_print(input: &str, expected: &str) {
        let ast = parser::parse_abstract(input.as_bytes(), Type::NestedText).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast, &SerializeOptions::default()).unwrap();
        assert_eq!(expected, String::from_utf8(out).unwrap());
    }

    fn expect_round_trip(input: &str) {
        let ast = parser::parse_abstract(input.as_bytes(), Type::NestedText).unwrap();
        let mut out1 = Vec::new();
        print(&mut out1, &ast, &SerializeOptions::default()).unwrap();
        let reparsed = parser::parse_abstract(&out1, Type::NestedText).unwrap();
        let mut out2 = Vec::new();
        print(&mut out2, &reparsed, &SerializeOptions::default()).unwrap();
        assert_eq!(String::from_utf8(out1).unwrap(), String::from_utf8(out2).unwrap());
    }

    fn print_built(build: impl FnOnce(&mut Builder) -> NodeId) -> Result<Vec<u8>> {
        let mut b = Builder::init();
        let root = build(&mut b);
        let ast = b.finish(root).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast, &SerializeOptions::default())?;
        Ok(out)
    }

    #[test]
    fn simple_mapping_round_trips_through_print_in_place() {
        expect_print("name: fig\n", "name: fig\n");
    }

    #[test]
    fn nested_mapping() {
        expect_print("server:\n    host: localhost\n    port: 80\n", "server:\n    host: localhost\n    port: 80\n");
    }

    #[test]
    fn list() {
        expect_print("- a\n- b\n", "- a\n- b\n");
    }

    #[test]
    fn empty_string_value_prints_as_a_nested_bare_gt_line() {
        expect_print(":\n  >\n", ":\n    >\n");
    }

    #[test]
    fn multiline_value_prints_as_a_gt_block() {
        let out = print_built(|b| {
            let key = b.add_string(b"k").unwrap();
            let value = b.add_string(b"line1\nline2").unwrap();
            b.add_mapping(&[Entry { key, value }]).unwrap()
        })
        .unwrap();
        assert_eq!("k:\n    > line1\n    > line2\n", String::from_utf8(out).unwrap());
    }

    #[test]
    fn key_needing_multiline_form() {
        let out = print_built(|b| {
            let key = b.add_string(b"- starts like a list tag").unwrap();
            let value = b.add_string(b"v").unwrap();
            b.add_mapping(&[Entry { key, value }]).unwrap()
        })
        .unwrap();
        assert_eq!(": - starts like a list tag\n    > v\n", String::from_utf8(out).unwrap());
    }

    #[test]
    fn round_trips_a_mixed_document_with_comments() {
        expect_round_trip("# header\nname: fig\nserver:\n    host: localhost\n    port: 80\nitems:\n    - a\n    - b\n");
    }

    #[test]
    fn round_trips_a_leading_comment_above_a_list_item_with_a_nested_value() {
        // Regression for the `take_leading`/`attach_leading` split in
        // `parser.rs`: the comment must stay above the `-` line, not migrate
        // onto the nested `nested` key's own line.
        expect_round_trip("- a\n# middle item note\n-\n    nested: 1\n- c\n");
    }

    #[test]
    fn empty_document_prints_as_nothing() {
        expect_print("", "");
    }

    #[test]
    fn root_scalar_prints_as_a_gt_block() {
        expect_print("> hello\n", "> hello\n");
    }

    #[test]
    fn null_mid_tree_is_unsupported() {
        let e = print_built(|b| {
            let key = b.add_string(b"k").unwrap();
            let value = b.add_null().unwrap();
            b.add_mapping(&[Entry { key, value }]).unwrap()
        })
        .unwrap_err();
        assert_eq!(err!(NullUnsupported), e);
    }

    #[test]
    fn a_multi_line_strings_splice_text_is_its_nested_gt_block_after_a_newline() {
        let mut b = Builder::init();
        let root = b.add_string(b"two\nlines").unwrap();
        let ast = b.finish(root).unwrap();
        let mut out = Vec::new();
        print_splice(&mut out, &ast, &SerializeOptions::default()).unwrap();
        assert_eq!("\n> two\n> lines\n", String::from_utf8(out).unwrap());
    }

    #[test]
    fn an_empty_container_prints_as_its_inline_form_on_a_line_of_its_own() {
        expect_round_trip("a:\n    {}\nb:\n    []\nc:\n    - []\n");
        expect_print("{}\n", "{}\n");
    }
}
