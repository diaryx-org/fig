//! Port of languages/nestedtext/src/edit.zig.
//!
//! NestedText-specific editing helpers for the editor.
//!
//! The generic span-splice engine lives in core's editor; this module holds
//! the NestedText-only logic it delegates to, mirroring TOML/YAML/fig/
//! plist's own `edit` split. NestedText needs MORE of its own logic than any
//! format but plist, for two independent reasons:
//!
//!   1. **Value framing.** Every value here is untyped text (no typed/quoted
//!      literal syntax to splice verbatim the way YAML/TOML/fig do), so a
//!      caller-supplied `value_text` is always a raw scalar that must be
//!      RENDERED into this grammar's own same-line-vs-nested-`>`-block form:
//!      non-empty and free of `\n` stays on the same line (`key: value` /
//!      `- value`); empty or multi-line becomes a nested `>`-block, one line
//!      per physical line, at 4 extra spaces of indent (mirroring
//!      `printer.rs`'s fixed `INDENT_WIDTH` — hardcoded like YAML/TOML's own
//!      `col + 2`, not sniffed from the document). This also means `set`/
//!      `replaceValAtPath` needs a real reframe (like YAML/fig's own): the
//!      NEW value's shape can differ from the OLD one's (inline <-> nested),
//!      so the whole `key`-to-value-end (or `-`-to-value-end) region gets
//!      regenerated rather than splicing into the old value's slot.
//!   2. **Sequence items have no keyvalue-shaped wrapper node.** A mapping
//!      entry's `keyvalue` node spans from its KEY's own line (always,
//!      regardless of where the value ends up), so the generic engine's
//!      line-position math already works for mapping entries with no
//!      NestedText-specific help. A sequence item is just its bare value
//!      node re-used as the sequence's child directly — and NestedText,
//!      unlike YAML in practice, allows genuinely NOTHING after a `-` on its
//!      own line whenever the value is nested/empty (a same-line `- key:
//!      value` is parsed as the LITERAL string `"key: value"`, never a nested
//!      mapping — see `parser.rs`'s module doc, "region" algorithm). So an
//!      item's OWN span can start on a later line than its `-`. The parser
//!      records every item's `-` in `Document::marker_span`, and the engine's
//!      block-sequence ops — append/prepend's prefix, remove and reorder's
//!      block boundaries, the leading-comment ops when `path` ends in an
//!      index — read it, so none of those is NestedText's any more. What is
//!      still this module's is RENDERING: a same-line value versus a nested
//!      `>`-block, which `render_item` and `render_tail` decide; the engine
//!      reframes an item from its recorded `-` and an entry from its
//!      recorded `:`.
//!
//! An inline `{}`/`[]` container is a FLOW container to the engine, so an
//! insert into one is the generic comma-aware splice (`{a: 1}`), using the
//! `kv_sep` declared for exactly that. `set`'s auto-vivify still excludes
//! NestedText: its printer never writes the inline form, so a vivified
//! ancestor would be a shape the format itself avoids.
//!
//! Out of scope: a `value_text` that's itself a nested container fragment
//! (inserting/setting a whole new sub-mapping/sub-list via CLI text) — every
//! op here treats `value_text` as a raw SCALAR string, matching NestedText's
//! own "strings all the way down" design; structural composition of
//! brand-new nested containers isn't exposed by this editor (existing
//! containers can still be freely inserted-into/deleted-from/reordered).

use super::parser;
use super::Type;
use fig_kit::err;
use fig_kit::ast::NodeKind;
use fig_kit::manifest::RenderRequest;
use fig_kit::Result;

/// The nesting step, as `mod.rs` declares it (`Syntax::indent_unit`) — the
/// same fixed step `printer.rs` writes. NestedText only requires a nested
/// region's indent to be GREATER than its parent's, but this editor never
/// sniffs the document's own convention, exactly as YAML's engine path does
/// not.
const INDENT_UNIT: &[u8] = b"    ";

// ── Value rendering ─────────────────────────────────────────────────────────

/// Append the tail of a `key:`/`-` line already written up to (not
/// including) its own line terminator: `" " ++ text` when `text` fits on the
/// same line (non-empty, no literal `\n`) and isn't `force_nested`;
/// otherwise a nested `>`-block, one line per physical line of `text` (an
/// empty line becomes a bare `>`), each under `child_indent` — mirroring
/// `printer.rs`'s `write_string_block`. Never emits a trailing newline after
/// the last line (the caller decides whether one is needed).
fn append_value_tail(out: &mut Vec<u8>, child_indent: &[u8], text: &[u8], force_nested: bool) {
    if let Some(block) = nested_block(text) {
        for line in block.split(|&c| c == b'\n') {
            out.push(b'\n');
            if !line.is_empty() {
                out.extend_from_slice(child_indent);
            }
            out.extend_from_slice(line);
        }
        return;
    }
    if !force_nested && !text.is_empty() && !text.contains(&b'\n') {
        out.push(b' ');
        out.extend_from_slice(text);
        return;
    }
    for line in text.split(|&c| c == b'\n') {
        out.push(b'\n');
        out.extend_from_slice(child_indent);
        if line.is_empty() {
            out.push(b'>');
        } else {
            out.extend_from_slice(b"> ");
            out.extend_from_slice(line);
        }
    }
}

/// The nested block `text` spells, when it spells one: a newline and then
/// lines that read as a NestedText value on their own — a dict, a list, a
/// `>` string, an inline `{}`/`[]` — returned without the newline and
/// without a trailing one. Otherwise `None`, and `text` is a string. This is
/// how splice text says a value is nested (`printer.rs`'s `print_splice`
/// writes a container or a multi-line string so); a string argument means
/// structure only if it starts with a line break AND the rest is NestedText.
fn nested_block(text: &[u8]) -> Option<&[u8]> {
    if text.len() < 2 || text[0] != b'\n' {
        return None;
    }
    let rest = &text[1..];
    let end = rest.iter().rposition(|&c| c != b'\n').map_or(0, |e| e + 1);
    let block = &rest[..end];
    if block.is_empty() {
        return None;
    }
    let doc = parser::parse(block, Type::NestedText).ok()?;
    if doc.ast.node(doc.ast.root).kind == NodeKind::Null {
        None
    } else {
        Some(block)
    }
}

/// Render `text` as a ROOT document value: a `>`-block at column 0, no
/// leading marker (the whole-document root has no `key:`/`-` to follow) and
/// always nested — a bare top-level scalar line has no grammar at all (see
/// `parser.rs`: an unrecognized `Other` line at the top level is a parse
/// error; only dict/list/string(`>`)/inline forms are valid there).
fn append_root_block(out: &mut Vec<u8>, text: &[u8]) {
    if let Some(block) = nested_block(text) {
        out.extend_from_slice(block);
        return;
    }
    let mut first = true;
    for line in text.split(|&c| c == b'\n') {
        if !first {
            out.push(b'\n');
        }
        first = false;
        if line.is_empty() {
            out.push(b'>');
        } else {
            out.extend_from_slice(b"> ");
            out.extend_from_slice(line);
        }
    }
}

/// Whether `key` needs the `: key` multiline form instead of plain `key:` —
/// ported verbatim from `printer.rs`'s `needs_multiline_key` so a freshly
/// inserted/renamed key round-trips exactly the way the printer would have
/// written it.
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

/// Append `key`'s multiline `: line` form (one `: line` — or bare `:` for an
/// empty line — per physical line of `key`), mirroring `printer.rs`'s
/// `write_multiline_key_lines`. The FIRST line's indent is the caller's (the
/// engine has written it); every later line is prefixed with `indent`. No
/// leading/trailing newline (the caller sequences it against whatever
/// follows), matching `append_value_tail`'s convention.
fn append_multiline_key_lines(out: &mut Vec<u8>, indent: &[u8], key: &[u8]) {
    let mut first = true;
    for line in key.split(|&c| c == b'\n') {
        if !first {
            out.push(b'\n');
            out.extend_from_slice(indent);
        }
        first = false;
        if line.is_empty() {
            out.push(b':');
        } else {
            out.extend_from_slice(b": ");
            out.extend_from_slice(line);
        }
    }
}

/// Whether `key` as WRITTEN is in the multiline `: key` form rather than the
/// plain `key:` form: its first line, once you skip its indentation, starts
/// with a `:` tag exactly like a tokenizer `Colon` line (`:` at end-of-line,
/// or `:` followed by a space) — a plain key's text can never start that
/// way (the tokenizer would have dispatched it as a multiline-key line to
/// begin with), so this check is exact, not a heuristic.
fn is_multiline_key_text(key: &[u8]) -> bool {
    let start = key.iter().position(|&c| c != b' ' && c != b'\t').unwrap_or(key.len());
    let k = &key[start..];
    if k.is_empty() || k[0] != b':' {
        return false;
    }
    if k.len() == 1 {
        return true;
    }
    k[1] == b' ' || k[1] == b'\n' || k[1] == b'\r'
}

fn child_indent(indent: &[u8]) -> Vec<u8> {
    let mut child = Vec::with_capacity(indent.len() + INDENT_UNIT.len());
    child.extend_from_slice(indent);
    child.extend_from_slice(INDENT_UNIT);
    child
}

// ── render_entry / render_item ──────────────────────────────────────────────

/// One block-mapping entry after its line's `indent`: `key:` plus the value
/// tail, or the `: key` multiline form (per `needs_multiline_key`) over a
/// value that is then always nested. Continuation lines sit at `indent` plus
/// one `INDENT_UNIT`. No trailing newline. See the editor's `writeEntry`.
pub fn render_entry(_: Type, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()> {
    let indent = r.indent;
    let key_text = r.key;
    let value_text = r.value;
    let child = child_indent(indent);
    if needs_multiline_key(key_text) {
        append_multiline_key_lines(out, indent, key_text);
        // A multiline key's value has no same-line form at all — always nested.
        append_value_tail(out, &child, value_text, true);
    } else {
        out.extend_from_slice(key_text);
        out.push(b':');
        append_value_tail(out, &child, value_text, false);
    }
    Ok(())
}

/// One block-sequence item after its line's `indent`: `-` plus the value
/// tail (same-line, or a nested `>`-block one `INDENT_UNIT` deeper). No
/// trailing newline. See the editor's `writeItem`.
pub fn render_item(_: Type, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()> {
    let child = child_indent(r.indent);
    out.push(b'-');
    append_value_tail(out, &child, r.value, false);
    Ok(())
}

// ── render_tail / render_key ────────────────────────────────────────────────

/// What follows a key: `:` and the value tail after a plain key, the tail
/// alone (always nested) after a multiline `: key` — `key_text` is the key
/// as written, so the form is read off it — or, for an empty `key_text`, the
/// DOCUMENT ROOT: a `>`-block at column 0, always nested, since a bare
/// top-level scalar line has no grammar at all (see `parser.rs`: an
/// unrecognized `Other` line at the top level is a parse error). No trailing
/// newline. See the editor's `writeTail`.
pub fn render_tail(_: Type, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()> {
    let key_text = r.key;
    let value_text = r.value;
    if key_text.is_empty() {
        append_root_block(out, value_text);
        return Ok(());
    }
    let child = child_indent(r.indent);
    let multiline_key = is_multiline_key_text(key_text);
    if !multiline_key {
        out.push(b':');
    }
    append_value_tail(out, &child, value_text, multiline_key);
    Ok(())
}

/// The key `r.key` spelled over the old key `r.old_key` (as written).
/// Plain-to-plain and multiline-to-multiline rename in place (the colon,
/// when present, sits outside the key's own span either way, so it's
/// untouched); multiline-to-plain adds the trailing `:` a plain key needs (a
/// multiline key's span carries no separator colon anywhere). A multiline
/// key's span starts at its line's indent, so that form carries `indent`
/// itself. Plain-to-multiline is declined (`KeyRequiresMultilineForm`): when
/// the current value is on the SAME line as the key, switching key forms
/// would also have to relocate the value onto a nested line (multiline keys
/// never have a same-line value), which is a value reframe this op doesn't
/// attempt — delete and re-insert the entry instead. See the editor's
/// `replaceKeyAtPath`.
pub fn render_key(_: Type, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()> {
    let indent = r.indent;
    let new_key = r.key;
    let old_key = r.old_key;
    let was_multiline = is_multiline_key_text(old_key);
    let wants_multiline = needs_multiline_key(new_key);
    if wants_multiline && !was_multiline {
        return Err(err!(KeyRequiresMultilineForm));
    }
    if wants_multiline {
        out.extend_from_slice(indent);
        append_multiline_key_lines(out, indent, new_key);
    } else if was_multiline {
        out.extend_from_slice(new_key);
        out.push(b':');
    } else {
        out.extend_from_slice(new_key);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(f: fn(Type, &mut Vec<u8>, &RenderRequest<'_>) -> Result<()>, r: RenderRequest<'_>) -> Result<String> {
        let mut out = Vec::new();
        f(Type::NestedText, &mut out, &r)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn an_entry_is_same_line_or_a_nested_block() {
        let r = |key: &'static str, value: &'static str| RenderRequest {
            indent: b"  ",
            key: key.as_bytes(),
            value: value.as_bytes(),
            ..Default::default()
        };
        assert_eq!("k: v", render(render_entry, r("k", "v")).unwrap());
        assert_eq!("k:\n      >", render(render_entry, r("k", "")).unwrap());
        assert_eq!("k:\n      > a\n      > b", render(render_entry, r("k", "a\nb")).unwrap());
        assert_eq!(": - k\n      > v", render(render_entry, r("- k", "v")).unwrap());
        // Splice text that spells a nested block is re-indented under the key.
        assert_eq!("k:\n      a: 1\n      b: 2", render(render_entry, r("k", "\na: 1\nb: 2\n")).unwrap());
    }

    #[test]
    fn an_item_a_tail_and_a_key() {
        let item = RenderRequest { value: b"x", ..Default::default() };
        assert_eq!("- x", render(render_item, item).unwrap());
        let root = RenderRequest { value: b"one\ntwo", ..Default::default() };
        assert_eq!("> one\n> two", render(render_tail, root).unwrap());
        let multi = RenderRequest { key: b": k", value: b"v", ..Default::default() };
        assert_eq!("\n    > v", render(render_tail, multi).unwrap());
        let rename = RenderRequest { key: b"new", old_key: b": old", ..Default::default() };
        assert_eq!("new:", render(render_key, rename).unwrap());
        let refuse = RenderRequest { key: b"- new", old_key: b"old", ..Default::default() };
        assert_eq!(err!(KeyRequiresMultilineForm), render(render_key, refuse).unwrap_err());
    }
}
