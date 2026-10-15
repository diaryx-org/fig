//! Port of languages/plist/src/edit.zig.
//!
//! plist-specific editing helpers for the editor.
//!
//! The generic span-splice engine lives in core's editor; this module holds
//! the plist-only logic it delegates to. plist needs far MORE of its own
//! logic than TOML/YAML/fig/INI, because it is the one editable format that
//! isn't line-oriented `key<sep>value`: it's XML, so a dict entry is a *pair
//! of sibling elements* — `<key>name</key>` then a typed value element
//! (`<string>fig</string>`) — on two separate lines, and a value has no bare
//! literal spelling at all. That breaks every assumption the generic
//! block-map helpers make (`kv_sep`, one-line entries, splice-a-literal-
//! value), so the two string renderers below say how plist spells them, and
//! the generic engine does the rest:
//!
//!   - `render_value`: the crux. A CLI value string (`fig set app.plist
//!     k=42`) has no plist meaning until it's wrapped in a typed element. We
//!     reuse the `.fig` dialect's own literal-else-string classifier — the
//!     engine's `literalOf`, handed in as `Literal` — to pick the type —
//!     `true`/`false` → `<true/>`/`<false/>`, integer → `<integer>`, float →
//!     `<real>`, any datetime shape → `<date>`, everything else → `<string>`
//!     (XML-escaped). `null` has no plist type (`NullUnsupported`). A value
//!     that already starts with `<` is spliced VERBATIM — the escape hatch
//!     for `<data>` (which can't be sniffed from bare text), an explicit
//!     `<date>`, a nested `<dict>`/`<array>`, or forcing a type
//!     (`<string>2.0`).
//!   - `render_entry`: a dict entry is two lines, `<key>k</key>` then the
//!     value element at the same indent.
//!
//! Replace, insert, append and prepend are the engine's: it renders the
//! value through `render_value`, writes an entry through `render_entry` or an
//! item as the bare element (`seq_item_marker = ""`), and expands an empty
//! `<dict/>` or `<array/>` from `Syntax.closed_containers`.
//!
//! Delete-key, remove-seq-item and every comment op are generic too. A plist
//! entry/item occupies whole lines, and the keyvalue's full-extent span
//! (recorded by `parser.rs`) covers both the key and value lines, so the
//! generic line-based delete removes the entry cleanly, owned `<!-- -->`
//! block and all. And `<!-- … -->` is declared as a `CommentDelimiter` pair
//! in `mod.rs`, so the generic leading and trailing comment ops write and
//! strip it themselves. No plist branch needed for any of them.

use super::Type;
use crate::err;
use crate::kit::manifest::{Literal, RenderRequest};
use crate::Result;

// ── value rendering ─────────────────────────────────────────────────────────

/// Render a CLI value string into a plist typed element, appended to `out`.
/// See the module header for the typing rules and the `<`-prefix escape
/// hatch.
pub fn render_value(_: Type, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()> {
    let value_text = r.value;
    let literal = r.literal;
    let t = trim(value_text, b" \t\r\n");
    if !t.is_empty() && t[0] == b'<' {
        // Explicit element (or element tree): the caller has spelled the
        // plist syntax themselves — splice it as-is. Reparse validates it; a
        // bogus `<foo` rolls back via `replaceAtSpan`.
        out.extend_from_slice(t);
        return Ok(());
    }
    match literal {
        Literal::Null => return Err(err!(NullUnsupported)), // plist has no null primitive
        Literal::Bool => out.extend_from_slice(if t == b"true" { b"<true/>" } else { b"<false/>" }),
        Literal::Int => wrap_text(out, b"integer", t, false),
        Literal::Float => wrap_text(out, b"real", t, false),
        Literal::Datetime => wrap_text(out, b"date", t, false),
        Literal::String => wrap_text(out, b"string", t, true),
    }
    Ok(())
}

/// `<tag>text</tag>`, XML-escaping `text` when `escape` (PCDATA content,
/// i.e. strings/keys — numbers and dates are already lexically safe).
fn wrap_text(out: &mut Vec<u8>, tag: &[u8], text: &[u8], escape: bool) {
    out.push(b'<');
    out.extend_from_slice(tag);
    out.push(b'>');
    if escape {
        append_escaped(out, text);
    } else {
        out.extend_from_slice(text);
    }
    out.extend_from_slice(b"</");
    out.extend_from_slice(tag);
    out.push(b'>');
}

/// Escape `&`/`<`/`>` in PCDATA content (mirrors the printer's
/// `write_escaped`).
fn append_escaped(out: &mut Vec<u8>, s: &[u8]) {
    for &c in s {
        match c {
            b'&' => out.extend_from_slice(b"&amp;"),
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            _ => out.push(c),
        }
    }
}

// ── entry rendering ─────────────────────────────────────────────────────────

/// `<key>key</key>\n<indent><value…>` — a dict entry's two lines, the key
/// and value at the same indent (matching the printer's layout). The first
/// line's indent is the engine's; `r.value` has been through `render_value`,
/// and a container's further lines (a `<dict>` fragment's entries and close
/// tag) are moved under `indent` as well. See the editor's `writeEntry`.
pub fn render_entry(_: Type, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()> {
    let indent = r.indent;
    let key = r.key;
    let rendered_value = r.value;
    out.extend_from_slice(b"<key>");
    append_escaped(out, key);
    out.extend_from_slice(b"</key>\n");
    out.extend_from_slice(indent);
    let mut lines = rendered_value.split(|&c| c == b'\n');
    out.extend_from_slice(lines.next().unwrap_or(b""));
    for line in lines {
        out.push(b'\n');
        if !line.is_empty() {
            out.extend_from_slice(indent);
        }
        out.extend_from_slice(line);
    }
    Ok(())
}

/// A renamed key: `<key>new_key</key>`, escaped. A key's span is the whole
/// element, so a bare name spliced over it would drop the tags. See the
/// editor's `replaceKeyAtPath`.
pub fn render_key(_: Type, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Result<()> {
    out.extend_from_slice(b"<key>");
    append_escaped(out, r.key);
    out.extend_from_slice(b"</key>");
    Ok(())
}

/// `std.mem.trim(u8, s, set)`.
fn trim<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let start = s.iter().position(|c| !set.contains(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !set.contains(c)).map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}
