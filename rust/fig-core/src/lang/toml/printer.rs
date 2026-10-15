//! Port of languages/toml/src/printer.zig.
//!
//! TOML printer: renders a fig AST as canonical TOML.
//!
//! This is a *canonical* serializer, not a format-preserving round-tripper — the
//! editor handles in-place, comment-preserving edits via source spans. The
//! printer takes a bare `&AST` (no spans) and emits clean TOML.
//!
//! Structure mapping (the inverse of the parser):
//!   * the root mapping's scalar/array entries print as bare `key = value` lines;
//!   * a mapping value prints as a `[header.path]` section (so an inline-table
//!     value `p = {x=1}` canonicalizes to a `[p]` section — same AST);
//!   * a non-empty all-mapping sequence prints as `[[header.path]]` array-of-
//!     tables elements;
//!   * every other value (scalars, scalar/mixed arrays, empty arrays) prints
//!     inline, with nested mappings becoming inline tables `{ ... }`.
//!
//! A table's own header is suppressed when it has no direct inline entries but
//! does have sub-tables/AoTs (their headers imply it); an otherwise-empty table
//! still emits its `[header]` so its existence round-trips.
//!
//! **Key order is preserved.** TOML forces section headers to trail a table's
//! `key = value` lines, so a table child that precedes later inline siblings
//! cannot become a `[section]` without semantically reordering the map. Such
//! mid-order children *demote* to order-preserving spellings — dotted keys for
//! a table, an inline array for an array-of-tables — when they are
//! comment-free; a commented one keeps the trailing-section form (comments
//! outrank order). See `Ctx::body`.
//!
//! TOML has no null, so a `null` value is `NullUnsupported` (e.g. a JSON
//! `null` cannot convert to TOML). `enum_literal`/`char_literal` extended scalars
//! (only minted by ZON) degrade to a string / integer; datetimes print verbatim.
//!
//! Errors: `NullUnsupported`, `NonStringKey`, `UnresolvedAlias` (TOML cannot
//! represent a YAML alias — materialize expands them first —, a null, or a
//! non-string table key).

use crate::kit::ast::{ExtKind, Extended, NodeId, NodeKind, SerializeOptions, AST};
use crate::kit::number as num;
use crate::kit::width;
use crate::{err, Result};

/// A header path built on the call stack (one link per nesting level), rendered
/// parent-first so no allocation is needed to accumulate `a.b.c`.
struct Path<'a> {
    key: &'a [u8],
    parent: Option<&'a Path<'a>>,
}

/// How a value renders inside its parent table body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// `key = value` on one line (scalars, arrays, inline tables).
    Inline,
    /// `[header]` section (any mapping value).
    Section,
    /// `[[header]]` array-of-tables (non-empty, all-mapping sequence).
    Aot,
}

/// Document-emit state: `wrote` gates the blank line printed before each
/// section/AoT header (suppressed at the very start of the output); `opts`
/// carries the width budget / indent / pretty knobs that drive the
/// inline-vs-expanded layout decisions below.
struct Ctx<'o> {
    w: &'o mut Vec<u8>,
    opts: SerializeOptions,
    wrote: bool,
}

impl Ctx<'_> {
    /// Emit a table's body: its `key = value` lines in document order, then —
    /// as TOML requires — the trailing sub-table / array-of-tables sections.
    /// `path` is this table's header path (`None` at root).
    ///
    /// TOML cannot re-open a table header, so section children may only TRAIL
    /// the body lines. A table child that sits BEFORE a later inline sibling
    /// would be silently reordered by a naive inline-then-sections split — a
    /// semantic key-order change. Instead, such a child is **demoted** to an
    /// order-preserving spelling: dotted keys (`fig.version = "1.0.0"`) for a
    /// table, an inline array for an array-of-tables. Demotion requires a
    /// comment-free entry; a commented mid-position child keeps the
    /// trailing-section form — comments outrank order.
    fn body(&mut self, ast: &AST, node_id: NodeId, first_child: Option<NodeId>, path: Option<&Path<'_>>) -> Result<()> {
        // The last inline-classified child bounds the demotion zone: only
        // section/AoT children before it are out of order.
        let mut last_inline: Option<NodeId> = None;
        {
            let mut cur = first_child;
            while let Some(id) = cur {
                if classify(&self.opts, ast, id) == Class::Inline {
                    last_inline = Some(id);
                }
                cur = ast.node(id).next_sibling;
            }
        }
        // Pass 1: body lines, in document order.
        let mut cur = first_child;
        let mut in_body_zone = last_inline.is_some();
        while let Some(id) = cur {
            if !in_body_zone {
                break;
            }
            let kv = keyvalue(ast, id);
            match classify(&self.opts, ast, id) {
                Class::Inline => self.kv_line(ast, kv.key, kv.value)?,
                Class::Section => {
                    if demotable(ast, id) {
                        let seg = Path { key: key_text(ast, kv.key)?, parent: None };
                        self.dotted_body(ast, &seg, mapping_first(ast, kv.value))?;
                    }
                }
                Class::Aot => {
                    if demotable(ast, id) {
                        self.kv_line(ast, kv.key, kv.value)?;
                    }
                }
            }
            if Some(id) == last_inline {
                in_body_zone = false;
            }
            cur = ast.node(id).next_sibling;
        }
        // Comments dangling at the end of this table's own lines (after its
        // inline entries, before any sub-tables).
        for c in ast.comments(node_id).dangling.iter() {
            for line in c.text.split(|&b| b == b'\n') {
                write_hash_line(self.w, trim_hs(line));
                self.w.push(b'\n');
            }
            self.wrote = true;
        }
        // Pass 2: trailing sections — children after the last inline line,
        // plus any commented mid-position child that could not demote.
        let mut cur = first_child;
        let mut in_demote_zone = last_inline.is_some();
        while let Some(id) = cur {
            let kv = keyvalue(ast, id);
            let demoted = in_demote_zone && demotable(ast, id);
            match classify(&self.opts, ast, id) {
                Class::Inline => {}
                Class::Section => {
                    if !demoted {
                        self.section(ast, kv.key, kv.value, path)?;
                    }
                }
                Class::Aot => {
                    if !demoted {
                        self.aot(ast, kv.key, kv.value, path)?;
                    }
                }
            }
            if in_demote_zone && Some(id) == last_inline {
                in_demote_zone = false;
            }
            cur = ast.node(id).next_sibling;
        }
        Ok(())
    }

    /// Emit a demoted table as dotted-key lines (`fig.version = "1.0.0"`),
    /// preserving document order where a `[section]` would have to move past
    /// later inline siblings. Only reached for comment-free subtrees. A
    /// non-empty sub-table recurses into deeper dots; everything else —
    /// scalars, arrays (including arrays of tables, rendered inline), empty
    /// tables — is one `prefix.key = value` line.
    fn dotted_body(&mut self, ast: &AST, prefix: &Path<'_>, first_child: Option<NodeId>) -> Result<()> {
        let mut cur = first_child;
        while let Some(id) = cur {
            let kv = keyvalue(ast, id);
            let seg = Path { key: key_text(ast, kv.key)?, parent: Some(prefix) };
            if let NodeKind::Mapping(first @ Some(_)) = ast.node(kv.value).kind {
                self.dotted_body(ast, &seg, first)?;
                cur = ast.node(id).next_sibling;
                continue;
            }
            write_path(self.w, &seg);
            self.w.extend_from_slice(b" = ");
            let col = path_byte_len(&seg).unwrap_or(0) + 3;
            self.write_value(ast, kv.value, col)?;
            self.w.push(b'\n');
            self.wrote = true;
            cur = ast.node(id).next_sibling;
        }
        Ok(())
    }

    fn kv_line(&mut self, ast: &AST, key_id: NodeId, value_id: NodeId) -> Result<()> {
        self.leading(ast, key_id);
        write_key(self.w, ast, key_id)?;
        self.w.extend_from_slice(b" = ");
        let col = key_byte_len(ast, key_id).unwrap_or(0) + 3; // `key` + ` = `
        self.write_value(ast, value_id, col)?;
        self.trailing(ast, value_id);
        self.w.push(b'\n');
        self.wrote = true;
        Ok(())
    }

    /// Render a `key = value` right-hand side. A scalar/mixed array that would
    /// overflow `opts.width` (and `pretty` is set) wraps one element per line;
    /// everything else renders inline. (Mappings and array-of-tables that reach
    /// here already fit by construction — see `classify`.)
    fn write_value(&mut self, ast: &AST, value_id: NodeId, col: usize) -> Result<()> {
        if let NodeKind::Sequence(first) = ast.node(value_id).kind {
            if self.opts.pretty && first.is_some() {
                // A null/alias inside makes the array un-measurable; fall
                // through so `write_inline` surfaces the proper error.
                if let Some(len) = inline_byte_len(ast, value_id) {
                    if col + len > self.opts.width as usize {
                        return self.write_array_multiline(ast, first);
                    }
                }
            }
        }
        write_inline(self.w, ast, value_id)
    }

    /// Wrap an array across lines: `[`, then each element indented one level with
    /// a trailing comma, then `]` at column 0. Elements themselves render inline.
    fn write_array_multiline(&mut self, ast: &AST, first: Option<NodeId>) -> Result<()> {
        self.w.extend_from_slice(b"[\n");
        let mut cur = first;
        while let Some(id) = cur {
            let n = self.w.len() + self.opts.indent as usize;
            self.w.resize(n, b' ');
            write_inline(self.w, ast, id)?;
            self.w.extend_from_slice(b",\n");
            cur = ast.node(id).next_sibling;
        }
        self.w.push(b']');
        Ok(())
    }

    fn section(&mut self, ast: &AST, key_id: NodeId, value_id: NodeId, parent: Option<&Path<'_>>) -> Result<()> {
        let seg = Path { key: key_text(ast, key_id)?, parent };
        let first = mapping_first(ast, value_id);
        if needs_header(&self.opts, ast, first) {
            if self.wrote {
                self.w.push(b'\n');
            }
            self.leading(ast, key_id); // comments above the `[header]` line
            self.w.push(b'[');
            write_path(self.w, &seg);
            self.w.extend_from_slice(b"]\n");
            self.wrote = true;
        }
        self.body(ast, value_id, first, Some(&seg))
    }

    // ── Comments ──────────────────────────────────────────────────────────
    // TOML has only `#` line comments. A block comment carried from another
    // format degrades to a run of `#` lines (one per content line).

    /// Emit a key's leading comments above its line, at column 0.
    fn leading(&mut self, ast: &AST, key_id: NodeId) {
        for c in ast.comments(key_id).leading.iter() {
            for line in c.text.split(|&b| b == b'\n') {
                write_hash_line(self.w, trim_hs(line));
                self.w.push(b'\n');
            }
            self.wrote = true;
        }
    }

    /// Emit a value's trailing comment after its line (` # …`). A multi-line
    /// block flattens to one line (newlines → spaces).
    fn trailing(&mut self, ast: &AST, value_id: NodeId) {
        let Some(c) = ast.comments(value_id).trailing else { return };
        self.w.extend_from_slice(b" #");
        if !c.text.is_empty() {
            self.w.push(b' ');
            for &ch in c.text {
                self.w.push(if ch == b'\n' { b' ' } else { ch });
            }
        }
    }

    fn aot(&mut self, ast: &AST, key_id: NodeId, value_id: NodeId, parent: Option<&Path<'_>>) -> Result<()> {
        let seg = Path { key: key_text(ast, key_id)?, parent };
        let mut elem = match ast.node(value_id).kind {
            NodeKind::Sequence(first) => first,
            _ => None,
        };
        while let Some(eid) = elem {
            if self.wrote {
                self.w.push(b'\n');
            }
            self.w.extend_from_slice(b"[[");
            write_path(self.w, &seg);
            self.w.extend_from_slice(b"]]\n");
            self.wrote = true;
            self.body(ast, eid, mapping_first(ast, eid), Some(&seg))?;
            elem = ast.node(eid).next_sibling;
        }
        Ok(())
    }
}

/// Entry `id`'s key and value.
fn keyvalue(ast: &AST, id: NodeId) -> crate::kit::ast::KeyValue {
    ast.node(id).kind.as_keyvalue().expect("a mapping child is a keyvalue")
}

/// A mapping's first entry (`None` for an empty one, or for a node that is
/// not a mapping, which Zig's union access would trap on).
fn mapping_first(ast: &AST, id: NodeId) -> Option<NodeId> {
    match ast.node(id).kind {
        NodeKind::Mapping(first) => first,
        _ => None,
    }
}

/// `std.mem.trim(u8, s, " \t")`.
fn trim_hs(s: &[u8]) -> &[u8] {
    let is_hs = |c: &u8| *c == b' ' || *c == b'\t';
    let start = s.iter().position(|c| !is_hs(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !is_hs(c)).map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

/// Whether a section emits its own `[header]`: yes when empty (records its
/// existence) or when it has at least one direct inline entry; no when it has
/// only sub-tables/AoTs (whose own headers imply this table). Note an inline
/// table counts as a direct inline entry, so a table whose sub-mappings all
/// collapse to inline tables does regain its header.
fn needs_header(opts: &SerializeOptions, ast: &AST, first_child: Option<NodeId>) -> bool {
    let Some(mut cur) = first_child else { return true }; // empty table
    loop {
        if classify(opts, ast, cur) == Class::Inline {
            return true;
        }
        match ast.node(cur).next_sibling {
            Some(n) => cur = n,
            None => return false,
        }
    }
}

/// Decide how a key/value entry renders. A mapping collapses to an inline table
/// (`{ ... }`) when `fits_inline` allows it, otherwise it expands to a
/// `[section]`. A non-empty all-mapping sequence is always an `[[array.of.tables]]`.
/// Scalars and scalar/mixed arrays are always `Inline` (the array may still
/// *wrap*, but it stays in place).
fn classify(opts: &SerializeOptions, ast: &AST, kv_id: NodeId) -> Class {
    let kv = keyvalue(ast, kv_id);
    match ast.node(kv.value).kind {
        NodeKind::Mapping(_) => {
            if fits_inline(opts, ast, kv_id) {
                Class::Inline
            } else {
                Class::Section
            }
        }
        NodeKind::Sequence(first) => {
            if all_mappings(ast, first) {
                Class::Aot
            } else {
                Class::Inline
            }
        }
        _ => Class::Inline,
    }
}

/// Whether a mapping value should render inline (`k = { ... }`) rather than as a
/// `[section]`. Inlining is allowed only when the whole `key = value` line fits in
/// `opts.width`, the subtree carries no comments (the inline form can't emit them
/// — they'd be silently dropped), and the value isn't an empty mapping (kept as
/// `[header]` so the table's existence round-trips).
fn fits_inline(opts: &SerializeOptions, ast: &AST, kv_id: NodeId) -> bool {
    let kv = keyvalue(ast, kv_id);
    let value_id = kv.value;
    if let NodeKind::Mapping(None) = ast.node(value_id).kind {
        return false;
    }
    if node_has_comments(ast, kv_id) {
        return false;
    }
    if subtree_has_comments(ast, kv.key) {
        return false;
    }
    if subtree_has_comments(ast, value_id) {
        return false;
    }
    let Some(klen) = key_byte_len(ast, kv.key) else { return false };
    let Some(vlen) = inline_byte_len(ast, value_id) else { return false };
    klen + 3 + vlen <= opts.width as usize // `key` + ` = ` + value
}

// ── Width / comment measurement ─────────────────────────────────────────────
// The inline-layout decision measures a value's rendered width by printing it
// with the very functions that emit the real output (see `width`), so the
// estimate can never drift from what's actually written.

/// Rendered byte width of a value's inline form, or `None` if it can't be
/// inlined (a `null`/alias inside makes `write_inline` error).
fn inline_byte_len(ast: &AST, id: NodeId) -> Option<usize> {
    width::rendered(|w| write_inline(w, ast, id))
}

/// Rendered byte width of a key (bare or quoted), or `None` for a non-string key.
fn key_byte_len(ast: &AST, key_id: NodeId) -> Option<usize> {
    width::rendered(|w| write_key(w, ast, key_id))
}

/// A mid-order table/AoT child may demote to dotted keys / an inline array
/// only when the whole entry (kv node, key, and value subtree) is
/// comment-free.
fn demotable(ast: &AST, kv_id: NodeId) -> bool {
    let kv = keyvalue(ast, kv_id);
    if node_has_comments(ast, kv_id) {
        return false;
    }
    if subtree_has_comments(ast, kv.key) {
        return false;
    }
    if subtree_has_comments(ast, kv.value) {
        return false;
    }
    true
}

/// Rendered byte width of a dotted path.
fn path_byte_len(path: &Path<'_>) -> Option<usize> {
    width::rendered(|w| -> Result<()> {
        write_path(w, path);
        Ok(())
    })
}

fn node_has_comments(ast: &AST, id: NodeId) -> bool {
    !ast.comments(id).is_empty()
}

/// Whether any node in the subtree rooted at `id` carries a comment.
fn subtree_has_comments(ast: &AST, id: NodeId) -> bool {
    if node_has_comments(ast, id) {
        return true;
    }
    match ast.node(id).kind {
        NodeKind::Sequence(first) => {
            let mut cur = first;
            while let Some(e) = cur {
                if subtree_has_comments(ast, e) {
                    return true;
                }
                cur = ast.node(e).next_sibling;
            }
        }
        NodeKind::Mapping(first) => {
            let mut cur = first;
            while let Some(kvid) = cur {
                if node_has_comments(ast, kvid) {
                    return true;
                }
                let kv = keyvalue(ast, kvid);
                if subtree_has_comments(ast, kv.key) {
                    return true;
                }
                if subtree_has_comments(ast, kv.value) {
                    return true;
                }
                cur = ast.node(kvid).next_sibling;
            }
        }
        _ => {}
    }
    false
}

/// True for a non-empty sequence whose every element is a mapping — the shape
/// TOML writes as `[[array.of.tables]]`.
fn all_mappings(ast: &AST, first: Option<NodeId>) -> bool {
    let Some(mut cur) = first else { return false };
    loop {
        if !matches!(ast.node(cur).kind, NodeKind::Mapping(_)) {
            return false;
        }
        match ast.node(cur).next_sibling {
            Some(n) => cur = n,
            None => return true,
        }
    }
}

// ── Inline value rendering ──────────────────────────────────────────────────

fn write_inline(w: &mut Vec<u8>, ast: &AST, id: NodeId) -> Result<()> {
    match ast.node(id).kind {
        NodeKind::Null => return Err(err!(NullUnsupported)),
        NodeKind::Boolean(b) => w.extend_from_slice(if b { b"true" } else { b"false" }),
        // A non-finite float arrives in its source's spelling (YAML's `.inf`,
        // JSON5's `Infinity`); TOML's own is `inf`/`-inf`/`nan`.
        NodeKind::Number(n) => match num::non_finite(n.raw) {
            Some(v) => w.extend_from_slice(num::PLAIN_NON_FINITE.of(v)),
            None => w.extend_from_slice(n.raw),
        },
        NodeKind::String(s) => write_basic_string(w, s),
        NodeKind::Extended(ext) => write_extended(w, ext),
        NodeKind::Sequence(first) => write_inline_array(w, ast, first)?,
        NodeKind::Mapping(first) => write_inline_table(w, ast, first)?,
        NodeKind::KeyValue(_) => unreachable!("a keyvalue is never a value position"),
        NodeKind::Alias(_) => return Err(err!(UnresolvedAlias)),
    }
    Ok(())
}

fn write_inline_array(w: &mut Vec<u8>, ast: &AST, first: Option<NodeId>) -> Result<()> {
    if first.is_none() {
        w.extend_from_slice(b"[]");
        return Ok(());
    }
    w.push(b'[');
    let mut cur = first;
    while let Some(id) = cur {
        write_inline(w, ast, id)?;
        cur = ast.node(id).next_sibling;
        if cur.is_some() {
            w.extend_from_slice(b", ");
        }
    }
    w.push(b']');
    Ok(())
}

fn write_inline_table(w: &mut Vec<u8>, ast: &AST, first: Option<NodeId>) -> Result<()> {
    if first.is_none() {
        w.extend_from_slice(b"{}");
        return Ok(());
    }
    w.extend_from_slice(b"{ ");
    let mut cur = first;
    while let Some(id) = cur {
        let kv = keyvalue(ast, id);
        write_key(w, ast, kv.key)?;
        w.extend_from_slice(b" = ");
        write_inline(w, ast, kv.value)?;
        cur = ast.node(id).next_sibling;
        if cur.is_some() {
            w.extend_from_slice(b", ");
        }
    }
    w.extend_from_slice(b" }");
    Ok(())
}

/// Render an `extended` scalar. Datetimes print verbatim (their text is already
/// valid TOML). An `enum_literal` (ZON-only) has no TOML form, so it degrades to
/// a string; a `char_literal`'s text is a decimal codepoint, valid as an integer.
fn write_extended(w: &mut Vec<u8>, ext: Extended<'_>) {
    match ext.kind {
        ExtKind::OffsetDatetime | ExtKind::LocalDatetime | ExtKind::LocalDate | ExtKind::LocalTime => w.extend_from_slice(ext.text),
        ExtKind::EnumLiteral => write_basic_string(w, ext.text),
        ExtKind::CharLiteral => w.extend_from_slice(ext.text),
        // JSON5 `Infinity`/`NaN` map onto TOML's native lowercase float forms.
        ExtKind::NumberSpecial => w.extend_from_slice(toml_special(ext.text)),
        // plist's date/data have no TOML primitive; degrade to a string, same
        // as a ZON enum literal.
        ExtKind::PlistDate | ExtKind::Data => write_basic_string(w, ext.text),
    }
}

/// Map a JSON5 non-finite lexeme (`Infinity`, `-Infinity`, `+Infinity`, `NaN`,
/// `-NaN`, `+NaN`) to its TOML float spelling (`inf`/`-inf`/`+inf`/`nan`).
fn toml_special(text: &[u8]) -> &'static [u8] {
    if text.ends_with(b"NaN") {
        return if !text.is_empty() && text[0] == b'-' {
            b"-nan"
        } else if !text.is_empty() && text[0] == b'+' {
            b"+nan"
        } else {
            b"nan"
        };
    }
    if text.starts_with(b"-") {
        return b"-inf";
    }
    if text.starts_with(b"+") {
        return b"+inf";
    }
    b"inf"
}

// ── Keys and strings ────────────────────────────────────────────────────────

fn key_text(ast: &AST, key_id: NodeId) -> Result<&[u8]> {
    match ast.node(key_id).kind {
        NodeKind::String(s) => Ok(s),
        _ => Err(err!(NonStringKey)),
    }
}

/// A key prints bare when non-empty and all of `[A-Za-z0-9_-]`, else quoted.
fn write_key(w: &mut Vec<u8>, ast: &AST, key_id: NodeId) -> Result<()> {
    let name = key_text(ast, key_id)?;
    if is_bare_key(name) {
        w.extend_from_slice(name);
    } else {
        write_basic_string(w, name);
    }
    Ok(())
}

/// Write `# text` (or a bare `#` for an empty comment).
fn write_hash_line(w: &mut Vec<u8>, text: &[u8]) {
    w.push(b'#');
    if !text.is_empty() {
        w.push(b' ');
        w.extend_from_slice(text);
    }
}

fn is_bare_key(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    name.iter().all(|&c| c.is_ascii_uppercase() || c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// A key's name: bare when it can be, else a basic string.
pub fn write_key_name(w: &mut Vec<u8>, name: &[u8]) {
    if is_bare_key(name) {
        return w.extend_from_slice(name);
    }
    write_basic_string(w, name);
}

/// Render a header path (`a.b.c`), parent segment first.
fn write_path(w: &mut Vec<u8>, path: &Path<'_>) {
    if let Some(par) = path.parent {
        write_path(w, par);
        w.push(b'.');
    }
    write_key_name(w, path.key);
}

/// Emit a TOML basic string: double-quoted, with `"`, `\`, and control bytes
/// escaped (TOML keeps other Unicode literal).
pub fn write_basic_string(w: &mut Vec<u8>, value: &[u8]) {
    w.push(b'"');
    for &ch in value {
        match ch {
            b'"' => w.extend_from_slice(b"\\\""),
            b'\\' => w.extend_from_slice(b"\\\\"),
            0x08 => w.extend_from_slice(b"\\b"),
            b'\t' => w.extend_from_slice(b"\\t"),
            b'\n' => w.extend_from_slice(b"\\n"),
            0x0c => w.extend_from_slice(b"\\f"),
            b'\r' => w.extend_from_slice(b"\\r"),
            0x00..=0x07 | 0x0b | 0x0e..=0x1f | 0x7f => write_unicode_escape(w, ch),
            _ => w.push(ch),
        }
    }
    w.push(b'"');
}

/// Whether `value` can be a literal string: no control character but a tab,
/// and, outside a multi-line one, no line break or `'` either. A multi-line
/// literal also refuses `'''`, which would close it, and a closing `'`,
/// which would read as part of the delimiter.
pub fn literal_holds(value: &[u8], multiline: bool) -> bool {
    for &ch in value {
        match ch {
            b'\t' => {}
            b'\n' | b'\'' => {
                if !multiline {
                    return false;
                }
            }
            0x00..=0x08 | 0x0b..=0x1f | 0x7f => return false,
            _ => {}
        }
    }
    if multiline {
        return !value.windows(3).any(|x| x == b"'''") && !value.ends_with(b"'");
    }
    true
}

/// Emit `value`, which `literal_holds`, as a literal string: `'…'`, or
/// `'''…'''` when `multiline`. `newline_first` breaks the line after the
/// opener, which TOML drops; a value that starts with a line break takes
/// one there whatever it says, or that break would be the one dropped.
pub fn write_literal_string(w: &mut Vec<u8>, value: &[u8], multiline: bool, newline_first: bool) {
    let quote: &[u8] = if multiline { b"'''" } else { b"'" };
    w.extend_from_slice(quote);
    if multiline && (newline_first || value.starts_with(b"\n")) {
        w.push(b'\n');
    }
    w.extend_from_slice(value);
    w.extend_from_slice(quote);
}

/// Emit `value` as a multi-line basic string, `"""…"""`: line breaks as
/// they are, `"` and `\` escaped (so no run of quotes can close it), and
/// every other control byte as `write_basic_string` escapes it.
/// `newline_first` is `write_literal_string`'s.
pub fn write_multiline_basic_string(w: &mut Vec<u8>, value: &[u8], newline_first: bool) {
    w.extend_from_slice(b"\"\"\"");
    if newline_first || value.starts_with(b"\n") {
        w.push(b'\n');
    }
    for &ch in value {
        match ch {
            b'"' => w.extend_from_slice(b"\\\""),
            b'\\' => w.extend_from_slice(b"\\\\"),
            b'\n' | b'\t' => w.push(ch),
            0x08 => w.extend_from_slice(b"\\b"),
            0x0c => w.extend_from_slice(b"\\f"),
            b'\r' => w.extend_from_slice(b"\\r"),
            0x00..=0x07 | 0x0b | 0x0e..=0x1f | 0x7f => write_unicode_escape(w, ch),
            _ => w.push(ch),
        }
    }
    w.extend_from_slice(b"\"\"\"");
}

fn write_unicode_escape(w: &mut Vec<u8>, ch: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    w.extend_from_slice(b"\\u00");
    w.push(HEX[(ch >> 4) as usize]);
    w.push(HEX[(ch & 0x0f) as usize]);
}

// ── Public entry points ─────────────────────────────────────────────────────

pub fn print(writer: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    let mut ctx = Ctx { w: writer, opts: *options, wrote: false };
    match ast.node(ast.root).kind {
        NodeKind::Mapping(first) => ctx.body(ast, ast.root, first, None)?,
        // A non-table root (e.g. converting a JSON array) has no valid TOML
        // document form; emit the inline value as a best-effort fragment.
        _ => {
            write_inline(ctx.w, ast, ast.root)?;
            ctx.w.push(b'\n');
        }
    }
    Ok(())
}

/// Print the value as the editor takes it spliced after `key = ` — see
/// `SerializeOptions::splice`. A value position holds only an inline
/// value, so a table prints as an inline table (`{ a = 1 }`) and an array of
/// tables as an inline array, never as the `[header]` sections a document
/// gives them. A new `[section]` is `Editor.insertContainer`'s, not a value's.
pub fn print_splice(writer: &mut Vec<u8>, ast: &AST, _options: &SerializeOptions) -> Result<()> {
    write_inline(writer, ast, ast.root)
}

/// Print the subtree at `id` as a standalone fragment (used by `get <path>`): a
/// mapping prints as a document body rooted there; any other node prints inline.
pub fn print_node(writer: &mut Vec<u8>, ast: &AST, id: NodeId, _depth: usize, options: &SerializeOptions) -> Result<()> {
    let mut ctx = Ctx { w: writer, opts: *options, wrote: false };
    match ast.node(id).kind {
        NodeKind::Mapping(first) => ctx.body(ast, id, first, None)?,
        _ => {
            write_inline(ctx.w, ast, id)?;
            ctx.w.push(b'\n');
        }
    }
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::builder::{Builder, Entry};
    use crate::kit::ast::{Comment, Node, NodeComments, Style};
    use crate::lang::toml::parser;
    use crate::lang::toml::Type;
    use std::borrow::Cow;

    fn expect_print(input: &str, expected: &str) {
        expect_print_with(input, expected, SerializeOptions::default());
    }

    fn expect_print_with(input: &str, expected: &str, options: SerializeOptions) {
        let ast = parser::parse_abstract(input.as_bytes(), Type::Toml1_1).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast, &options).unwrap();
        assert_eq!(expected, String::from_utf8_lossy(&out));
    }

    /// Round-trip: the printed TOML must reparse without error, and the canonical
    /// form must be a fixed point — re-printing the reparse yields identical bytes.
    fn expect_round_trip(input: &str) {
        let ast = parser::parse_abstract(input.as_bytes(), Type::Toml1_1).unwrap();
        let mut out1 = Vec::new();
        print(&mut out1, &ast, &SerializeOptions::default()).unwrap();
        let reparsed = parser::parse_abstract(&out1, Type::Toml1_1).unwrap();
        let mut out2 = Vec::new();
        print(&mut out2, &reparsed, &SerializeOptions::default()).unwrap();
        assert_eq!(String::from_utf8_lossy(&out1), String::from_utf8_lossy(&out2), "printed:\n{}", String::from_utf8_lossy(&out1));
    }

    fn with(width: u16) -> SerializeOptions {
        SerializeOptions { width, ..SerializeOptions::default() }
    }

    #[test]
    fn captures_and_re_emits_comments_leading_trailing_table_header() {
        let src = "# document header\nname = \"Tom\" # the name\ncount = 42\n\n# the server section\n[server]\nhost = \"a\" # primary host\n";
        expect_print(src, src);
    }

    #[test]
    fn block_comment_carried_in_degrades_to_a_hash_run() {
        let mut b = Builder::init();
        let v = b.add_int(1).unwrap();
        let k = b.add_string(b"x").unwrap();
        let leading = [Comment { text: b"line one\nline two", style: Style::Block }];
        b.set_comments(k, &NodeComments { leading: Cow::Borrowed(&leading), ..NodeComments::default() }).unwrap();
        let root = b.add_mapping(&[Entry { key: k, value: v }]).unwrap();
        let ast = b.finish(root).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast, &SerializeOptions::default()).unwrap();
        assert_eq!(b"# line one\n# line two\nx = 1\n", &out[..]);
    }

    #[test]
    fn prints_root_scalars() {
        let src = "name = \"Tom\"\ncount = 42\npi = 3.14\nflag = true\n";
        expect_print(src, src);
    }

    #[test]
    fn canonicalizes_numbers() {
        expect_print("hex = 0xDEAD_beef\noct = 0o17\nneg = -0\n", "hex = 3735928559\noct = 15\nneg = 0\n");
    }

    #[test]
    fn datetimes_print_verbatim() {
        let src = "when = 1979-05-27T07:32:00Z\nd = 1979-05-27\nt = 07:32:00\n";
        expect_print(src, src);
    }

    #[test]
    fn a_small_table_collapses_to_an_inline_table() {
        expect_print("[server]\nhost = \"a\"\nport = 80\n", "server = { host = \"a\", port = 80 }\n");
    }

    #[test]
    fn a_table_too_wide_for_the_budget_stays_a_section() {
        expect_print_with("[server]\nhost = \"a\"\nport = 80\n", "[server]\nhost = \"a\"\nport = 80\n", with(20));
    }

    #[test]
    fn a_table_carrying_a_comment_stays_a_section() {
        let src = "[server]\nhost = \"a\" # primary\nport = 80\n";
        expect_print(src, src);
    }

    #[test]
    fn scalars_precede_sub_tables_and_blank_line_separates_when_expanded() {
        expect_print_with("[a]\nx = 1\n[a.b]\ny = 2\n", "[a]\nx = 1\n\n[a.b]\ny = 2\n", with(8));
    }

    #[test]
    fn a_mid_order_sub_table_demotes_to_dotted_keys_order_preserved() {
        let src = "[a]\nb.x = 1\nb.y = 2\nc = 3\n";
        expect_print_with(src, src, with(8));
    }

    #[test]
    fn a_commented_mid_order_sub_table_keeps_its_section_comments_outrank_order() {
        expect_print_with("[a]\nb.x = 1 # note\nb.y = 2\nc = 3\n", "[a]\nc = 3\n\n[a.b]\nx = 1 # note\ny = 2\n", with(8));
    }

    #[test]
    fn a_mid_order_array_of_tables_demotes_to_an_inline_array_order_preserved() {
        let src = "[t]\na = [{ x = 1 }, { x = 2 }]\nb = \"a long enough string value to keep the whole table t from collapsing inline\"\n";
        expect_print(src, src);
    }

    #[test]
    fn order_preserving_demotions_round_trip() {
        expect_round_trip("[a]\nb.x = 1\nb.y = 2\nc = 3\n");
        expect_round_trip("[t]\na = [{ x = 1 }, { x = 2 }]\nb = 3\n");
        expect_round_trip("fig.version = \"1.0.0\"\nfig.default-features = false\nserde = \"1.0\"\n");
    }

    #[test]
    fn supertable_header_is_suppressed_when_it_has_only_sub_tables() {
        expect_print_with("[a.b]\ny = 2\n", "[a.b]\ny = 2\n", with(8));
    }

    #[test]
    fn empty_table_still_emits_its_header() {
        expect_print("[a]\n", "[a]\n");
    }

    #[test]
    fn a_small_inline_table_stays_inline_within_the_budget() {
        expect_print("point = { x = 1, y = 2 }\n", "point = { x = 1, y = 2 }\n");
    }

    #[test]
    fn a_wide_inline_table_expands_to_a_section() {
        expect_print_with("point = { x = 1, y = 2 }\n", "[point]\nx = 1\ny = 2\n", with(10));
    }

    #[test]
    fn arrays_of_scalars_print_inline() {
        let src = "nums = [1, 2, 3]\nnested = [[1, 2], [\"a\", \"b\"]]\n";
        expect_print(src, src);
    }

    #[test]
    fn a_wide_array_wraps_one_element_per_line_with_a_trailing_comma() {
        expect_print_with(
            "members = [\"alpha\", \"beta\", \"gamma\"]\n",
            "members = [\n  \"alpha\",\n  \"beta\",\n  \"gamma\",\n]\n",
            with(20),
        );
    }

    #[test]
    fn pretty_false_keeps_a_wide_array_on_one_line() {
        expect_print_with(
            "members = [\"alpha\", \"beta\", \"gamma\"]\n",
            "members = [\"alpha\", \"beta\", \"gamma\"]\n",
            SerializeOptions { width: 20, pretty: false, ..SerializeOptions::default() },
        );
    }

    #[test]
    fn wrapped_array_indent_honors_the_indent_option() {
        expect_print_with("xs = [1, 2]\n", "xs = [\n  1,\n  2,\n]\n", SerializeOptions { width: 4, indent: 2, ..SerializeOptions::default() });
    }

    #[test]
    fn array_of_tables_prints_as_double_bracket_sections() {
        let src = "[[fruit]]\nname = \"apple\"\n\n[[fruit]]\nname = \"pear\"\n";
        expect_print(src, src);
    }

    #[test]
    fn quotes_non_bare_keys() {
        expect_print("\"a.b\" = 1\n\"\" = 2\n", "\"a.b\" = 1\n\"\" = 2\n");
    }

    #[test]
    fn escapes_control_characters_in_strings() {
        expect_print("s = \"a\\tb\\nc\"\n", "s = \"a\\tb\\nc\"\n");
    }

    #[test]
    fn null_value_is_unsupported() {
        // Build an AST with a null by hand (TOML never parses one).
        let nodes = [
            Node { id: 0, kind: NodeKind::Mapping(Some(1)), next_sibling: None },
            Node { id: 1, kind: NodeKind::KeyValue(crate::kit::ast::KeyValue { key: 2, value: 3 }), next_sibling: None },
            Node { id: 2, kind: NodeKind::String(b"k"), next_sibling: None },
            Node { id: 3, kind: NodeKind::Null, next_sibling: None },
        ];
        let ast = AST::from_nodes(0, &nodes).unwrap();
        let mut out = Vec::new();
        assert_eq!(Some(err!(NullUnsupported)), print(&mut out, &ast, &SerializeOptions::default()).err());
    }

    #[test]
    fn round_trips_a_mixed_document() {
        expect_round_trip(
            "title = \"demo\"\nports = [8000, 8001]\n\n[owner]\nname = \"Tom\"\ndob = 1979-05-27T07:32:00Z\n\n[servers.alpha]\nip = \"10.0.0.1\"\n\n[servers.beta]\nip = \"10.0.0.2\"\n\n[[products]]\nname = \"Hammer\"\nsku = 738594937\n\n[[products]]\nname = \"Nail\"\ncolor = \"gray\"\n",
        );
    }

    #[test]
    fn round_trips_nested_arrays_of_tables() {
        expect_round_trip("[[a]]\nx = 1\n\n[[a.b]]\ny = 2\n\n[[a.b]]\ny = 3\n");
    }

    #[test]
    fn round_trips_deeply_nested_empty_and_dotted_tables() {
        expect_round_trip("a.b.c = 1\n\n[x]\n\n[x.y.z]\nw = \"deep\"\n");
    }

    #[test]
    fn print_splice_writes_a_table_and_an_array_of_tables_inline_as_a_value_position_holds_them() {
        let ast = parser::parse_abstract(b"a = 1\nb = [\"x\"]\n[[t]]\nc = 2\n", Type::Toml1_1).unwrap();
        let mut out = Vec::new();
        print_splice(&mut out, &ast, &SerializeOptions::default()).unwrap();
        assert_eq!(b"{ a = 1, b = [\"x\"], t = [{ c = 2 }] }", &out[..]);
    }
}
