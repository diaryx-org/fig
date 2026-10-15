//! Port of languages/zon/src/printer.zig.
//!
//! ZON printer: renders a fig AST as Zig Object Notation.
//!
//! Container literals use `.{ ... }` with trailing commas (idiomatic ZON, as
//! `zig fmt` would produce). Struct field names are emitted bare (`.name`)
//! when they form a legal Zig identifier, otherwise quoted (`.@"name"`).
//!
//! Native ZON scalars carried as `extended` nodes round-trip exactly: an
//! `enum_literal` re-emits as `.name` (or `.@"name"`), a `char_literal` as
//! `'a'` (re-encoded from its stored codepoint). A datetime `extended` node
//! (only produced by TOML) has no ZON form, so it falls back to a quoted
//! string.
//!
//! Best-effort, niche output:
//!   * `number.raw` is emitted verbatim. When the AST came from ZON that is
//!     always valid; values minted by other formats are assumed decimal.
//!
//! Errors: ZON cannot represent a YAML alias (`UnresolvedAlias`; materialize
//! expands them first) nor a non-string struct-field key (`NonStringKey`).

use super::zig_tokenizer;
use fig_kit::err;
use fig_kit::ast::{ExtKind, Extended, NodeId, NodeKind, SerializeOptions, AST};
use fig_kit::number;
use fig_kit::Result;

/// Multi-line ZON uses a fixed four-space indent (what `zig fmt` produces),
/// so `SerializeOptions::indent` is not applied here — only `pretty` is
/// honored (compact single-line vs. multi-line).
const BLOCK_INDENT: &[u8] = b"    ";

struct Printer<'a> {
    w: &'a mut Vec<u8>,
    ast: &'a AST,
    options: SerializeOptions,
}

pub fn print(w: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    let mut p = Printer { w, ast, options: *options };
    p.leading_comments(ast.leading_comment_anchor(ast.root), 0);
    p.node(ast.root, 0)?;
    // A container root emitted its own trailing beside its `.{`; a scalar
    // root's trailing is emitted here.
    if !p.is_container(ast.trailing_comment_anchor(ast.root)) {
        p.trailing_comment(ast.trailing_comment_anchor(ast.root));
    }
    p.w.push(b'\n');
    Ok(())
}

pub fn print_node(w: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize, options: &SerializeOptions) -> Result<()> {
    let mut p = Printer { w, ast, options: *options };
    p.node(id, depth)
}

impl Printer<'_> {
    fn node(&mut self, id: NodeId, depth: usize) -> Result<()> {
        let ast = self.ast;
        match ast.node(id).kind {
            NodeKind::Null => self.w.extend_from_slice(b"null"),
            NodeKind::Boolean(value) => self.w.extend_from_slice(if value { b"true" } else { b"false" }),
            // ZON spells a non-finite float `inf`/`-inf`/`nan`, whatever its
            // source did.
            NodeKind::Number(value) => match number::non_finite(value.raw) {
                Some(v) => self.w.extend_from_slice(number::PLAIN_NON_FINITE.of(v)),
                None => self.w.extend_from_slice(value.raw),
            },
            NodeKind::String(value) => write_string(self.w, value),
            NodeKind::Extended(ext) => write_extended(self.w, ext),
            NodeKind::Sequence(first_child) => self.container(id, first_child, depth)?,
            NodeKind::Mapping(first_child) => self.container(id, first_child, depth)?,
            NodeKind::KeyValue(kv) => {
                write_field_name(self.w, ast, kv.key)?;
                self.w.extend_from_slice(b" = ");
                self.node(kv.value, depth)?;
            }
            NodeKind::Alias(_) => return Err(err!(UnresolvedAlias)),
        }
        Ok(())
    }

    /// Sequences and structs share ZON's `.{ ... }` literal; each child
    /// renders the same way (a bare value or a `.field = value`, dispatched
    /// by `node`), so one routine serves both. Compact output keeps
    /// everything on one line with `.{ a, b }` spacing; multi-line output is
    /// the `zig fmt` shape with a trailing comma per element.
    fn container(&mut self, node_id: NodeId, first_child: Option<NodeId>, depth: usize) -> Result<()> {
        let ast = self.ast;
        // Dangling comments (orphans at the body's end) force the multi-line
        // form so they have somewhere to print; only an empty, comment-free,
        // or compact container stays inline.
        let comments = ast.comments(node_id);
        let dangling = if self.comments_on() { &comments.dangling[..] } else { &[] };
        if first_child.is_none() && dangling.is_empty() {
            self.w.extend_from_slice(b".{}");
            self.trailing_comment(node_id); // empty: `.{} // c`
            return Ok(());
        }

        if !self.options.pretty {
            self.w.extend_from_slice(b".{ ");
            let mut current = first_child;
            while let Some(id) = current {
                self.node(id, depth + 1)?;
                current = ast.node(id).next_sibling;
                if current.is_some() {
                    self.w.extend_from_slice(b", ");
                }
            }
            self.w.extend_from_slice(b" }");
            return Ok(());
        }

        self.w.extend_from_slice(b".{");
        // The container's own trailing comment rides the line it opened on.
        self.trailing_comment(node_id);
        self.w.push(b'\n');
        let mut current = first_child;
        while let Some(id) = current {
            self.leading_comments(ast.leading_comment_anchor(id), depth + 1);
            write_indent(self.w, depth + 1);
            self.node(id, depth + 1)?;
            self.w.push(b',');
            // A container child emits its own trailing beside its opener;
            // skip here.
            let anchor = ast.trailing_comment_anchor(id);
            if !self.is_container(anchor) {
                self.trailing_comment(anchor);
            }
            self.w.push(b'\n');
            current = ast.node(id).next_sibling;
        }
        // Comments dangling at the end of the body (after the last element,
        // or the entire body of an otherwise-empty container).
        for c in dangling {
            for line in c.text.split(|&b| b == b'\n') {
                write_indent(self.w, depth + 1);
                write_slash_line(self.w, trim_hs(line));
                self.w.push(b'\n');
            }
        }
        write_indent(self.w, depth);
        self.w.push(b'}');
        Ok(())
    }

    // ── Comments ─────────────────────────────────────────────────────────
    // ZON has only `//` line comments (Zig's syntax), and only in multi-line
    // output (a `//` can't survive on a compact single line). A block comment
    // carried from another format degrades to a run of `//` lines.

    /// True when comments may be emitted: multi-line output only.
    fn comments_on(&self) -> bool {
        self.options.pretty
    }

    /// Whether `id` is a container node (whose own trailing comment is
    /// emitted beside its `.{`, not by its parent).
    fn is_container(&self, id: NodeId) -> bool {
        matches!(self.ast.node(id).kind, NodeKind::Sequence(_) | NodeKind::Mapping(_))
    }

    /// Emit a node's leading comments above its line, at `depth`.
    fn leading_comments(&mut self, id: NodeId, depth: usize) {
        if !self.comments_on() {
            return;
        }
        for c in self.ast.comments(id).leading.iter() {
            for line in c.text.split(|&b| b == b'\n') {
                write_indent(self.w, depth);
                write_slash_line(self.w, trim_hs(line));
                self.w.push(b'\n');
            }
        }
    }

    /// Emit a node's trailing comment after its line (` // …`), newlines
    /// flattened.
    fn trailing_comment(&mut self, id: NodeId) {
        if !self.comments_on() {
            return;
        }
        let Some(c) = self.ast.comments(id).trailing else { return };
        self.w.extend_from_slice(b" //");
        if !c.text.is_empty() {
            self.w.push(b' ');
            for &ch in c.text {
                self.w.push(if ch == b'\n' { b' ' } else { ch });
            }
        }
    }
}

/// Write `// text` (or a bare `//` for an empty comment).
fn write_slash_line(w: &mut Vec<u8>, text: &[u8]) {
    w.extend_from_slice(b"//");
    if !text.is_empty() {
        w.push(b' ');
        w.extend_from_slice(text);
    }
}

/// Emit a struct field name: `.name` if it's a bare identifier, else
/// `.@"..."`.
fn write_field_name(w: &mut Vec<u8>, ast: &AST, key_id: NodeId) -> Result<()> {
    let name = match ast.node(key_id).kind {
        NodeKind::String(s) => s,
        _ => return Err(err!(NonStringKey)), // ZON field names must derive from a string key
    };
    write_dot_name(w, name);
    Ok(())
}

/// Render an `extended` scalar back to ZON. Enum and char literals reproduce
/// their source form; a datetime (TOML-only) has no ZON type, so it degrades
/// to a quoted string.
fn write_extended(w: &mut Vec<u8>, ext: Extended<'_>) {
    match ext.kind {
        ExtKind::EnumLiteral => write_dot_name(w, ext.text),
        ExtKind::CharLiteral => write_char_literal(w, ext.text),
        _ => write_string(w, ext.text),
    }
}

/// Emit a dotted name (`.name` or `.@"..."`) — shared by struct field names
/// and enum literals, which have identical lexical rules.
pub fn write_dot_name(w: &mut Vec<u8>, name: &[u8]) {
    w.push(b'.');
    if is_bare_identifier(name) {
        w.extend_from_slice(name);
    } else {
        w.extend_from_slice(b"@\"");
        write_string_inner(w, name);
        w.push(b'"');
    }
}

/// Re-encode a char literal from its stored decimal codepoint. Common
/// escapes and printable ASCII emit directly; anything else uses
/// `'\u{...}'`. A codepoint that fails to parse (only if minted oddly by
/// another format) falls back to its numeric text, which is still valid ZON.
fn write_char_literal(w: &mut Vec<u8>, text: &[u8]) {
    let Some(cp) = parse_int_u21(text, 10) else {
        w.extend_from_slice(text);
        return;
    };
    w.push(b'\'');
    match cp {
        0x27 => w.extend_from_slice(b"\\'"),
        0x5c => w.extend_from_slice(b"\\\\"),
        0x0a => w.extend_from_slice(b"\\n"),
        0x0d => w.extend_from_slice(b"\\r"),
        0x09 => w.extend_from_slice(b"\\t"),
        0x20..=0x26 | 0x28..=0x5b | 0x5d..=0x7e => w.push(cp as u8),
        _ => w.extend_from_slice(format!("\\u{{{cp:x}}}").as_bytes()),
    }
    w.push(b'\'');
}

/// Zig's `std.fmt.parseInt(u21, buf, base)`: an optional sign (a negative is
/// only `-0`), `_` between digits, and no overflow past 21 bits.
fn parse_int_u21(buf: &[u8], base: u32) -> Option<u32> {
    if buf.is_empty() {
        return None;
    }
    let (neg, digits) = match buf[0] {
        b'+' => (false, &buf[1..]),
        b'-' => (true, &buf[1..]),
        _ => (false, buf),
    };
    if digits.is_empty() || digits[0] == b'_' || digits[digits.len() - 1] == b'_' {
        return None;
    }
    let mut acc: u32 = 0;
    for &c in digits {
        if c == b'_' {
            continue;
        }
        let d = match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 10,
            b'A'..=b'Z' => (c - b'A') as u32 + 10,
            _ => return None,
        };
        if d >= base {
            return None;
        }
        if acc != 0 {
            acc = acc.checked_mul(base)?;
            if acc >= 1 << 21 {
                return None;
            }
        } else if neg {
            if d != 0 {
                return None;
            }
            continue;
        }
        acc += d;
        if acc >= 1 << 21 {
            return None;
        }
    }
    Some(acc)
}

fn write_string(w: &mut Vec<u8>, value: &[u8]) {
    w.push(b'"');
    write_string_inner(w, value);
    w.push(b'"');
}

fn write_string_inner(w: &mut Vec<u8>, value: &[u8]) {
    for &ch in value {
        match ch {
            b'"' => w.extend_from_slice(b"\\\""),
            b'\\' => w.extend_from_slice(b"\\\\"),
            b'\n' => w.extend_from_slice(b"\\n"),
            b'\r' => w.extend_from_slice(b"\\r"),
            b'\t' => w.extend_from_slice(b"\\t"),
            // Other control bytes: Zig uses `\xNN` (no `\uNNNN` form for these).
            0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f => write_hex_escape(w, ch),
            _ => w.push(ch),
        }
    }
}

fn write_hex_escape(w: &mut Vec<u8>, ch: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    w.extend_from_slice(b"\\x");
    w.push(HEX[(ch >> 4) as usize]);
    w.push(HEX[(ch & 0x0f) as usize]);
}

fn write_indent(w: &mut Vec<u8>, depth: usize) {
    for _ in 0..depth {
        w.extend_from_slice(BLOCK_INDENT);
    }
}

/// `std.mem.trim(u8, s, " \t")`.
fn trim_hs(s: &[u8]) -> &[u8] {
    let is_hs = |c: &u8| *c == b' ' || *c == b'\t';
    let start = s.iter().position(|c| !is_hs(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !is_hs(c)).map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

/// A name is a bare ZON identifier when it matches `[A-Za-z_][A-Za-z0-9_]*`
/// and is not a Zig keyword (`error`, `true`, ...), which would have to be
/// quoted.
fn is_bare_identifier(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    for (i, &c) in name.iter().enumerate() {
        let ok = c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit());
        if !ok {
            return false;
        }
    }
    zig_tokenizer::get_keyword(name).is_none()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::parser;
    use super::super::Type;
    use super::*;

    fn expect_print(input: &str, expected: &str) {
        expect_print_opts(input, expected, SerializeOptions::default());
    }

    fn expect_print_opts(input: &str, expected: &str, options: SerializeOptions) {
        let ast = parser::parse_abstract(input.as_bytes(), Type::Zon).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast, &options).unwrap();
        assert_eq!(expected, String::from_utf8(out).unwrap());
    }

    #[test]
    fn container_comment_opening_line_is_head_closing_line_is_bottom() {
        // `.{ // head` rides the open line; `} // tail` normalizes to a
        // bottom comment.
        let src = ".{\n    .a = .{ // head\n        1,\n    },\n    .b = .{\n        2,\n        // tail\n    },\n}";
        expect_print(src, &format!("{src}\n"));
    }

    #[test]
    fn captures_and_re_emits_comments_leading_trailing_nested() {
        let src = ".{\n    // the name\n    .name = \"Ada\", // a person\n    .nums = .{\n        1, // first\n        2,\n    },\n}";
        expect_print(src, &format!("{src}\n"));
    }

    #[test]
    fn prints_a_struct_literal() {
        expect_print(".{ .name = \"Ada\", .age = 36 }", ".{\n    .name = \"Ada\",\n    .age = 36,\n}\n");
    }

    #[test]
    fn prints_nested_arrays_and_structs() {
        expect_print(".{ .xs = .{ 1, 2 }, .ok = true }", ".{\n    .xs = .{\n        1,\n        2,\n    },\n    .ok = true,\n}\n");
    }

    #[test]
    fn prints_empty_container() {
        expect_print(".{}", ".{}\n");
    }

    #[test]
    fn quotes_non_identifier_field_names() {
        expect_print(".{ .@\"has space\" = 1 }", ".{\n    .@\"has space\" = 1,\n}\n");
    }

    #[test]
    fn keyword_field_names_are_quoted() {
        expect_print(".{ .@\"error\" = 1 }", ".{\n    .@\"error\" = 1,\n}\n");
    }

    #[test]
    fn escapes_strings() {
        expect_print(".{ .s = \"a\\tb\\nc\" }", ".{\n    .s = \"a\\tb\\nc\",\n}\n");
    }

    #[test]
    fn enum_literals_round_trip() {
        expect_print(
            ".{ .mode = .fast, .@\"weird key\" = .@\"weird val\" }",
            ".{\n    .mode = .fast,\n    .@\"weird key\" = .@\"weird val\",\n}\n",
        );
    }

    #[test]
    fn char_literals_round_trip() {
        expect_print(
            ".{ .a = 'A', .nl = '\\n', .q = '\\'', .emoji = '😀' }",
            ".{\n    .a = 'A',\n    .nl = '\\n',\n    .q = '\\'',\n    .emoji = '\\u{1f600}',\n}\n",
        );
    }

    #[test]
    fn compact_struct_stays_on_one_line() {
        expect_print_opts(
            ".{ .name = \"Ada\", .age = 36 }",
            ".{ .name = \"Ada\", .age = 36 }\n",
            SerializeOptions { pretty: false, ..Default::default() },
        );
    }

    #[test]
    fn compact_nests_structs_and_arrays_inline() {
        expect_print_opts(
            ".{ .xs = .{ 1, 2 }, .ok = true }",
            ".{ .xs = .{ 1, 2 }, .ok = true }\n",
            SerializeOptions { pretty: false, ..Default::default() },
        );
    }

    #[test]
    fn compact_empty_container() {
        expect_print_opts(".{}", ".{}\n", SerializeOptions { pretty: false, ..Default::default() });
    }
}
