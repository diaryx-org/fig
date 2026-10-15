//! Port of languages/yaml/src/printer.zig.
//!
//! Prints a document in YAML block style. A short collection value inlines as
//! flow (`[a, b]`, `{ k: v }`) when the whole line fits `opts.width` and the
//! collection carries no comments; otherwise it stays block.

use fig_kit::ast::{KeyValue, NodeId, NodeKind, SerializeOptions, Tag, AST};
use fig_kit::ast::ExtKind;
use fig_kit::number as num;
use fig_kit::width;

/// Prints a document in YAML block style, using default serialize options.
pub fn print(writer: &mut Vec<u8>, ast: &AST) {
    print_with(writer, ast, &SerializeOptions::default())
}

/// Prints a document in YAML block style. A short collection value inlines as
/// flow (`[a, b]`, `{ k: v }`) when the whole line fits `opts.width` and the
/// collection carries no comments; otherwise it stays block. The AST records no
/// flow-vs-block memory (no CST), so this is a pure width heuristic, not a
/// replay of the source's own choice.
pub fn print_with(writer: &mut Vec<u8>, ast: &AST, opts: &SerializeOptions) {
    // Document-level leading comments (those bound to the root) sit at column 0
    // above everything else — above the directives too.
    leading_comments(writer, ast, ast.leading_comment_anchor(ast.root), 0);
    print_directives(writer, ast);
    print_root_node(writer, ast, opts);
    // End-of-document comments dangling off the root, at column 0.
    dangling_comments(writer, ast, ast.root, 0);
}

/// The root node, preceded by its own anchor/tag. Every other node's properties
/// are written by whatever wrote its lead-in (`- `, `key:`, `?`); the root has
/// no lead-in, so they are written here — on a line of their own above a block
/// collection, inline for anything with a single-line spelling.
fn print_root_node(writer: &mut Vec<u8>, ast: &AST, opts: &SerializeOptions) {
    let root = ast.root;
    if !has_props(ast, root) {
        return print_node(writer, ast, root, 0, opts);
    }
    match ast.node(root).kind {
        NodeKind::Sequence(Some(child)) => {
            write_props_own_line(writer, ast, root);
            return print_sequence(writer, ast, Some(child), 0, opts);
        }
        NodeKind::Mapping(Some(child)) => {
            write_props_own_line(writer, ast, root);
            return print_mapping(writer, ast, Some(child), 0, opts);
        }
        _ => {}
    }
    write_props(writer, ast, root);
    print_node(writer, ast, root, 0, opts);
}

// ── Directives ──────────────────────────────────────────────────────────────
// A `%TAG` handle is the one part of a document's directives prefix its body
// depends on: `!e!foo` is a parse error in a document that does not declare
// `!e!`, so a printer that keeps the tag must keep the declaration.

/// Emit each `%TAG` directive whose handle a tag in this document actually
/// uses, then the `---` that ends the directives prefix (mandatory once any
/// directive is written). An unused declaration is dropped.
fn print_directives(writer: &mut Vec<u8>, ast: &AST) {
    let mut wrote_any = false;
    for directive in ast.tag_directives() {
        if !handle_is_used(ast, directive.handle) {
            continue;
        }
        writer.extend_from_slice(b"%TAG ");
        writer.extend_from_slice(directive.handle);
        writer.push(b' ');
        writer.extend_from_slice(directive.prefix);
        writer.push(b'\n');
        wrote_any = true;
    }
    if wrote_any {
        writer.extend_from_slice(b"---\n");
    }
}

/// Whether any node's tag is spelled with `handle`.
fn handle_is_used(ast: &AST, handle: &[u8]) -> bool {
    for f in ast.tag_facts() {
        let tag = ast.tag_of_fact(*f);
        let Some(used) = tag_handle_of(tag_text(tag)) else { continue };
        if used == handle {
            return true;
        }
    }
    false
}

/// The handle a tag spelling uses, including both `!`s: `!e!foo` → `!e!`,
/// `!!str` → `!!`, and a local `!foo` (or the bare non-specific `!`) → the
/// primary `!`. A verbatim `!<uri>` tag uses none.
fn tag_handle_of(text: &[u8]) -> Option<&[u8]> {
    if text.is_empty() || text[0] != b'!' {
        return None;
    }
    if text.len() > 1 && text[1] == b'<' {
        return None;
    }
    match text[1..].iter().position(|&c| c == b'!') {
        Some(p) => Some(&text[..p + 2]),
        None => Some(&text[..1]),
    }
}

pub fn print_node(writer: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize, opts: &SerializeOptions) {
    let node = ast.node(id);
    match node.kind {
        NodeKind::Null => writer.extend_from_slice(b"null\n"),
        NodeKind::Boolean(value) => {
            writer.extend_from_slice(if value { b"true" } else { b"false" });
            writer.push(b'\n');
        }
        NodeKind::Number(value) => {
            write_number(writer, value.raw);
            writer.push(b'\n');
        }
        NodeKind::Extended(value) => {
            // YAML's core schema has none of these types (timestamps were YAML
            // 1.1). Enum literals become string scalars (may need quoting);
            // datetimes and char codepoints emit verbatim as plain scalars.
            match value.kind {
                ExtKind::EnumLiteral => print_scalar(writer, value.text),
                ExtKind::Data => write_data(writer, ast, id, value.text),
                _ => writer.extend_from_slice(value.text),
            }
            writer.push(b'\n');
        }
        NodeKind::String(value) => {
            print_scalar(writer, value);
            writer.push(b'\n');
        }
        NodeKind::Sequence(first_child) => print_sequence(writer, ast, first_child, depth, opts),
        NodeKind::Mapping(first_child) => print_mapping(writer, ast, first_child, depth, opts),
        NodeKind::KeyValue(kv) => print_key_value(writer, ast, kv, depth, false, opts),
        NodeKind::Alias(name) => {
            writer.push(b'*');
            writer.extend_from_slice(name);
            writer.push(b'\n');
        }
    }
}

fn write_number(writer: &mut Vec<u8>, raw: &[u8]) {
    // YAML spells every non-finite float, so this cannot fail.
    let _ = num::write(writer, raw, num::YAML_1_2);
}

fn print_sequence(writer: &mut Vec<u8>, document: &AST, first_child: Option<NodeId>, depth: usize, opts: &SerializeOptions) {
    if first_child.is_none() {
        writer.extend_from_slice(b"[]\n");
        return;
    }

    let mut current_id = first_child;
    while let Some(id) = current_id {
        let item = document.node(id);
        // A scalar element owns its leading directly. A block-mapping element
        // parked its leading on its first key, which still renders above the
        // `- ` line.
        leading_comments(writer, document, seq_item_lead_anchor(document, id), depth);
        write_indent(writer, depth);
        match item.kind {
            // Sequence items stay block even when short: `- uses: x` reads far
            // better than `- { uses: x }`.
            NodeKind::Mapping(child) => {
                writer.extend_from_slice(b"- ");
                if let Some(first_pair) = child {
                    // An item mapping's own props cannot ride the `- key: v`
                    // line: the parser reads `- &a key: v` as the KEY's anchor.
                    // They take the rest of the dash line instead, and the
                    // mapping starts beneath.
                    if has_props(document, id) {
                        write_props_own_line(writer, document, id);
                        print_mapping(writer, document, Some(first_pair), depth + 1, opts);
                    } else {
                        print_sequence_mapping(writer, document, first_pair, depth, opts);
                    }
                } else {
                    write_props(writer, document, id); // `- &a {}`
                    writer.extend_from_slice(b"{}\n");
                }
            }
            NodeKind::Sequence(child) => {
                if child.is_none() {
                    writer.extend_from_slice(b"- ");
                    write_props(writer, document, id); // `- &a []`
                    writer.extend_from_slice(b"[]\n");
                } else if has_props(document, id) {
                    // `- &a` — the props are all that follows the dash, and the
                    // nested block sits under them.
                    writer.extend_from_slice(b"- ");
                    write_props_own_line(writer, document, id);
                    print_sequence(writer, document, child, depth + 1, opts);
                } else {
                    // A bare dash, not `- ` — nothing follows on the line.
                    writer.extend_from_slice(b"-\n");
                    print_sequence(writer, document, child, depth + 1, opts);
                }
            }
            _ => {
                writer.extend_from_slice(b"- ");
                if !try_write_block_string_value(writer, document, id, depth + 1) {
                    print_inline_value(writer, document, id);
                    trailing_comment(writer, document, id);
                    writer.push(b'\n');
                }
            }
        }
        current_id = item.next_sibling;
    }
}

/// Where a sequence item's leading comment lives. A block-mapping item parks
/// it on the item-mapping's first key; every other item owns its leading on
/// the item node itself.
fn seq_item_lead_anchor(ast: &AST, id: NodeId) -> NodeId {
    match ast.node(id).kind {
        NodeKind::Mapping(Some(kv)) => ast.node(kv).kind.as_keyvalue().unwrap().key,
        _ => id,
    }
}

fn print_mapping(writer: &mut Vec<u8>, document: &AST, first_child: Option<NodeId>, depth: usize, opts: &SerializeOptions) {
    if first_child.is_none() {
        writer.extend_from_slice(b"{}\n");
        return;
    }

    let mut current_id = first_child;
    while let Some(id) = current_id {
        leading_comments(writer, document, document.leading_comment_anchor(id), depth);
        print_key_value(writer, document, document.node(id).kind.as_keyvalue().unwrap(), depth, false, opts);
        current_id = document.node(id).next_sibling;
    }
}

fn print_sequence_mapping(writer: &mut Vec<u8>, document: &AST, first_pair: NodeId, depth: usize, opts: &SerializeOptions) {
    // Every pair sits one level deeper than the sequence: the `- ` prefix
    // already occupies that level's two columns. The first pair is written
    // right after the `- `, so its own leading indent is suppressed, but its
    // base depth is still `depth + 1`.
    print_key_value(writer, document, document.node(first_pair).kind.as_keyvalue().unwrap(), depth + 1, true, opts);

    let mut current_id = document.node(first_pair).next_sibling;
    while let Some(id) = current_id {
        leading_comments(writer, document, document.leading_comment_anchor(id), depth + 1);
        print_key_value(writer, document, document.node(id).kind.as_keyvalue().unwrap(), depth + 1, false, opts);
        current_id = document.node(id).next_sibling;
    }
}

fn print_key_value(writer: &mut Vec<u8>, document: &AST, kv: KeyValue, depth: usize, skip_indent: bool, opts: &SerializeOptions) {
    let value = document.node(kv.value);
    if !skip_indent {
        write_indent(writer, depth);
    }
    // Columns already on the value's line — indent, key (with props), and `: `
    // — the budget the flow form must fit within.
    let value_prefix: usize = match document.node(kv.key).kind {
        // A collection key has no single-line spelling to put before a `:`, so
        // it takes the explicit form: `? key` on its own line(s), then the
        // value on a `: ` line at the key's column.
        NodeKind::Sequence(_) | NodeKind::Mapping(_) => {
            write_explicit_key(writer, document, kv.key, depth, opts);
            2 * depth + 2
        }
        _ => {
            write_key_lead(writer, document, kv.key); // `&k key:` / `!!str key:`
            2 * depth + key_cols(document, kv.key) + 2
        }
    };
    match value.kind {
        NodeKind::Mapping(child) => {
            if child.is_some() && flow_fits(document, kv.value, value_prefix, opts) {
                writer.extend_from_slice(b": ");
                write_flow(writer, document, kv.value);
                writer.push(b'\n');
                return;
            }
            writer.push(b':');
            write_props_after_colon(writer, document, kv.value); // `: &a` before the block
            if child.is_none() {
                writer.extend_from_slice(b" {}\n");
            } else {
                // A comment on the `key:` line rides here, above the block value.
                trailing_comment(writer, document, kv.value);
                writer.push(b'\n');
                print_mapping(writer, document, child, depth + 1, opts);
            }
        }
        NodeKind::Sequence(child) => {
            if child.is_some() && flow_fits(document, kv.value, value_prefix, opts) {
                writer.extend_from_slice(b": ");
                write_flow(writer, document, kv.value);
                writer.push(b'\n');
                return;
            }
            writer.push(b':');
            write_props_after_colon(writer, document, kv.value);
            if child.is_none() {
                writer.extend_from_slice(b" []\n");
            } else {
                trailing_comment(writer, document, kv.value);
                writer.push(b'\n');
                // Indentless: a sequence value's dashes sit at the key's column.
                print_sequence(writer, document, child, depth, opts);
            }
        }
        _ => {
            writer.extend_from_slice(b": ");
            if !try_write_block_string_value(writer, document, kv.value, depth + 1) {
                print_inline_value(writer, document, kv.value);
                // Trailing comment rides the value's line (`key: value # note`).
                trailing_comment(writer, document, kv.value);
                writer.push(b'\n');
            }
        }
    }
}

// ── Flow (inline) collections ───────────────────────────────────────────────
// A collection value/element may render inline (`[a, b]` / `{ k: v }`) instead
// of as block lines when it fits the width budget and carries no comments. The
// width is measured by rendering the real flow form (see `width`).

/// Whether the collection at `id` may render as flow starting at column
/// `prefix`, within `opts.width`.
fn flow_fits(ast: &AST, id: NodeId, prefix: usize, opts: &SerializeOptions) -> bool {
    if !flow_eligible(ast, id) {
        return false;
    }
    let Some(w) = width::rendered(|out| {
        write_flow(out, ast, id);
        Ok::<(), ()>(())
    }) else {
        return false;
    };
    prefix + w <= opts.width as usize
}

/// True when `id`'s whole subtree can render as flow with no loss: nothing in
/// it carries a comment or an anchor/tag property, and every leaf has a flow
/// spelling (no multi-line strings, aliases, or extended scalars).
fn flow_eligible(ast: &AST, id: NodeId) -> bool {
    if !ast.comments(id).is_empty() {
        return false;
    }
    if has_props(ast, id) {
        return false;
    }
    match ast.node(id).kind {
        NodeKind::Null | NodeKind::Boolean(_) | NodeKind::Number(_) => true,
        NodeKind::String(s) => !s.contains(&b'\n'),
        NodeKind::Extended(_) | NodeKind::Alias(_) | NodeKind::KeyValue(_) => false,
        NodeKind::Sequence(first) => {
            let mut cur = first;
            while let Some(el) = cur {
                // A list of mappings reads far better as block `- key: value`
                // lines than as `[{ ... }, { ... }]`; keep it block.
                if let NodeKind::Mapping(_) = ast.node(el).kind {
                    return false;
                }
                if !flow_eligible(ast, el) {
                    return false;
                }
                cur = ast.node(el).next_sibling;
            }
            true
        }
        NodeKind::Mapping(first) => {
            let mut cur = first;
            while let Some(kv_id) = cur {
                if !ast.comments(kv_id).is_empty() {
                    return false;
                }
                let kv = ast.node(kv_id).kind.as_keyvalue().unwrap();
                if !ast.comments(kv.key).is_empty() {
                    return false;
                }
                // A key must have a one-line flow spelling: a scalar, never a
                // collection or an alias.
                match ast.node(kv.key).kind {
                    NodeKind::String(s) => {
                        if s.contains(&b'\n') {
                            return false;
                        }
                    }
                    NodeKind::Null | NodeKind::Boolean(_) | NodeKind::Number(_) => {}
                    _ => return false,
                }
                // A mapping directly nested in a mapping is too much structure
                // for one flow line; keep it block.
                if let NodeKind::Mapping(_) = ast.node(kv.value).kind {
                    return false;
                }
                if !flow_eligible(ast, kv.value) {
                    return false;
                }
                cur = ast.node(kv_id).next_sibling;
            }
            true
        }
    }
}

fn has_props(ast: &AST, id: NodeId) -> bool {
    ast.anchor_of(id).is_some() || ast.tag_of(id).is_some()
}

/// Rendered width of a key (its anchor/tag props plus the key scalar).
fn key_cols(ast: &AST, key_id: NodeId) -> usize {
    width::rendered(|out| {
        write_key_lead(out, ast, key_id);
        Ok::<(), ()>(())
    })
    .unwrap_or(0)
}

/// The left-hand side of an implicit `key: value` line: the key's props and
/// its single-line spelling.
fn write_key_lead(writer: &mut Vec<u8>, ast: &AST, key_id: NodeId) {
    write_props(writer, ast, key_id);
    write_scalar_key(writer, ast, key_id);
}

/// A non-collection key's own spelling, without props.
fn write_scalar_key(writer: &mut Vec<u8>, ast: &AST, key_id: NodeId) {
    match ast.node(key_id).kind {
        NodeKind::String(s) => print_scalar(writer, s),
        // An empty key (`: a`) reads back as a null key; spelling it `null`
        // says so, and re-parses to the same node.
        NodeKind::Null => writer.extend_from_slice(b"null"),
        NodeKind::Boolean(b) => writer.extend_from_slice(if b { b"true" } else { b"false" }),
        NodeKind::Number(n) => write_number(writer, n.raw),
        NodeKind::Extended(value) => match value.kind {
            ExtKind::EnumLiteral => print_scalar(writer, value.text),
            ExtKind::Data => write_data(writer, ast, key_id, value.text),
            _ => writer.extend_from_slice(value.text),
        },
        // `*a : b` — the space is load-bearing: `:` is a legal anchor-name
        // character, so `*a:` would be an alias to an anchor called `a:`.
        NodeKind::Alias(name) => {
            writer.push(b'*');
            writer.extend_from_slice(name);
            writer.push(b' ');
        }
        NodeKind::Sequence(_) | NodeKind::Mapping(_) | NodeKind::KeyValue(_) => unreachable!("write_explicit_key's"),
    }
}

/// A collection key, in explicit form. Everything up to and including the
/// indent of the `:` line is written. The key inlines as `? [a, b]` when it
/// fits the width; otherwise it is `?` alone on the line — with the key's own
/// props after it — and the block form indented beneath.
fn write_explicit_key(writer: &mut Vec<u8>, ast: &AST, key_id: NodeId, depth: usize, opts: &SerializeOptions) {
    if flow_fits(ast, key_id, 2 * depth + 2, opts) {
        writer.extend_from_slice(b"? ");
        write_flow(writer, ast, key_id);
        writer.push(b'\n');
    } else {
        writer.push(b'?');
        write_props_after_colon(writer, ast, key_id);
        writer.push(b'\n');
        print_node(writer, ast, key_id, depth + 1, opts);
    }
    write_indent(writer, depth);
}

/// Emit `id` as inline flow. Assumes `flow_eligible(ast, id)`.
fn write_flow(writer: &mut Vec<u8>, ast: &AST, id: NodeId) {
    match ast.node(id).kind {
        NodeKind::Null => writer.extend_from_slice(b"null"),
        NodeKind::Boolean(b) => writer.extend_from_slice(if b { b"true" } else { b"false" }),
        NodeKind::Number(n) => write_number(writer, n.raw),
        NodeKind::String(s) => print_flow_scalar(writer, s),
        NodeKind::Sequence(first) => {
            if first.is_none() {
                writer.extend_from_slice(b"[]");
                return;
            }
            writer.push(b'[');
            let mut cur = first;
            let mut i = 0usize;
            while let Some(el) = cur {
                if i > 0 {
                    writer.extend_from_slice(b", ");
                }
                write_flow(writer, ast, el);
                cur = ast.node(el).next_sibling;
                i += 1;
            }
            writer.push(b']');
        }
        NodeKind::Mapping(first) => {
            if first.is_none() {
                writer.extend_from_slice(b"{}");
                return;
            }
            writer.extend_from_slice(b"{ ");
            let mut cur = first;
            let mut i = 0usize;
            while let Some(kv_id) = cur {
                if i > 0 {
                    writer.extend_from_slice(b", ");
                }
                let kv = ast.node(kv_id).kind.as_keyvalue().unwrap();
                match ast.node(kv.key).kind {
                    NodeKind::String(s) => print_flow_scalar(writer, s),
                    _ => write_scalar_key(writer, ast, kv.key),
                }
                writer.extend_from_slice(b": ");
                write_flow(writer, ast, kv.value);
                cur = ast.node(kv_id).next_sibling;
                i += 1;
            }
            writer.extend_from_slice(b" }");
        }
        NodeKind::Extended(_) | NodeKind::Alias(_) | NodeKind::KeyValue(_) => unreachable!(),
    }
}

/// Like `print_scalar`, but also single-quotes a plain scalar carrying a flow
/// indicator (`,` `[` `]` `{` `}`), which would otherwise close or split the
/// surrounding flow collection.
fn print_flow_scalar(writer: &mut Vec<u8>, raw: &[u8]) {
    if !has_control_char(raw) && !needs_quoting(raw) && contains_flow_indicator(raw) {
        write_single_quoted(writer, raw);
    } else {
        print_scalar(writer, raw);
    }
}

fn contains_flow_indicator(s: &[u8]) -> bool {
    s.iter().any(|c| matches!(c, b',' | b'[' | b']' | b'{' | b'}'))
}

fn print_inline_value(writer: &mut Vec<u8>, document: &AST, id: NodeId) {
    let node = document.node(id);
    write_props(writer, document, id);
    match node.kind {
        NodeKind::Null => writer.extend_from_slice(b"null"),
        NodeKind::Boolean(value) => writer.extend_from_slice(if value { b"true" } else { b"false" }),
        NodeKind::Number(value) => write_number(writer, value.raw),
        NodeKind::Extended(value) => match value.kind {
            ExtKind::EnumLiteral => print_scalar(writer, value.text),
            ExtKind::Data => write_data(writer, document, id, value.text),
            _ => writer.extend_from_slice(value.text),
        },
        NodeKind::String(value) => print_scalar(writer, value),
        NodeKind::Sequence(child) => writer.extend_from_slice(if child.is_none() { b"[]" } else { b"[...]" }),
        NodeKind::Mapping(child) => writer.extend_from_slice(if child.is_none() { b"{}" } else { b"{...}" }),
        NodeKind::Alias(name) => {
            writer.push(b'*');
            writer.extend_from_slice(name);
        }
        NodeKind::KeyValue(_) => unreachable!(),
    }
}

/// Emit a node's anchor/tag properties (`&name `, `!tag `) from the AST
/// side-tables. Order matches YAML's `c-ns-properties`: anchor then tag.
fn write_props(writer: &mut Vec<u8>, ast: &AST, id: NodeId) {
    if let Some(name) = ast.anchor_of(id) {
        writer.push(b'&');
        writer.extend_from_slice(name);
        writer.push(b' ');
    }
    if let Some(tag) = ast.tag_of(id) {
        writer.extend_from_slice(tag_text(tag));
        writer.push(b' ');
    }
}

/// A byte string, as YAML spells one: `!!binary` and its base64, a plain
/// scalar. A node read from YAML already carries its tag, which `write_props`
/// wrote, so it is not written twice.
fn write_data(writer: &mut Vec<u8>, ast: &AST, id: NodeId, base64: &[u8]) {
    if ast.tag_of(id).is_none() {
        writer.extend_from_slice(b"!!binary ");
    }
    // An empty byte string is an empty scalar, which a tag alone reads as
    // null; quote it so it reads back as text.
    if base64.is_empty() {
        writer.extend_from_slice(b"\"\"");
    } else {
        writer.extend_from_slice(base64);
    }
}

/// Render a type tag as YAML: a verbatim text tag as-is, or a normalized kind
/// tag as the core-schema shorthand (`!!int`, `!!str`, …).
fn tag_text(tag: Tag<'_>) -> &[u8] {
    tag.spelling()
}

/// Like `write_props` but for a line of its own — `&a !tag` with no trailing
/// space, then the newline.
fn write_props_own_line(writer: &mut Vec<u8>, ast: &AST, id: NodeId) {
    let mut wrote_any = false;
    if let Some(name) = ast.anchor_of(id) {
        writer.push(b'&');
        writer.extend_from_slice(name);
        wrote_any = true;
    }
    if let Some(tag) = ast.tag_of(id) {
        if wrote_any {
            writer.push(b' ');
        }
        writer.extend_from_slice(tag_text(tag));
    }
    writer.push(b'\n');
}

/// Like `write_props` but for the position right after a mapping value's `:`,
/// before a block collection or `{}`/`[]`: emits ` &name`/` !tag` with a
/// leading (not trailing) space.
fn write_props_after_colon(writer: &mut Vec<u8>, ast: &AST, id: NodeId) {
    if let Some(name) = ast.anchor_of(id) {
        writer.extend_from_slice(b" &");
        writer.extend_from_slice(name);
    }
    if let Some(tag) = ast.tag_of(id) {
        writer.push(b' ');
        writer.extend_from_slice(tag_text(tag));
    }
}

// ── Scalar emission ─────────────────────────────────────────────────────────

/// Emit a scalar inline — plain, single-quoted, or double-quoted as the value
/// requires so it reads back unchanged.
fn print_scalar(writer: &mut Vec<u8>, raw: &[u8]) {
    if has_control_char(raw) {
        write_double_quoted(writer, raw);
    } else if needs_quoting(raw) {
        write_single_quoted(writer, raw);
    } else {
        writer.extend_from_slice(raw);
    }
}

/// True if any byte is an ASCII control character (newline/tab included).
pub fn has_control_char(s: &[u8]) -> bool {
    s.iter().any(|&c| c < 0x20 || c == 0x7f)
}

/// Whether a plain (unquoted) scalar would be misread — as another type, or as
/// YAML structure — and so needs single-quoting.
fn needs_quoting(s: &[u8]) -> bool {
    if s.is_empty() {
        return true; // empty plain scalar reads back as null
    }
    if resolves_to_non_string(s) {
        return true;
    }
    if s[0] == b' ' || s[s.len() - 1] == b' ' {
        return true; // leading/trailing space is lost
    }
    match s[0] {
        // A plain scalar may not begin with an indicator character.
        b'!' | b'&' | b'*' | b'?' | b'|' | b'>' | b'%' | b'@' | b'`' | b'"' | b'\'' | b'#' | b',' | b'[' | b']' | b'{' | b'}' => return true,
        // `-`/`:` are unsafe as the first char only before a space (or alone).
        b'-' | b':' => {
            if s.len() == 1 || s[1] == b' ' {
                return true;
            }
        }
        _ => {}
    }
    // Interior `: ` (mapping indicator), trailing `:`, or ` #` (comment) force
    // quoting.
    if contains(s, b": ") {
        return true;
    }
    if s[s.len() - 1] == b':' {
        return true;
    }
    if contains(s, b" #") {
        return true;
    }
    false
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Plain scalars that YAML 1.2's core schema resolves to a non-string type
/// (null, bool, the special floats) or a number.
fn resolves_to_non_string(s: &[u8]) -> bool {
    const KEYWORDS: [&[u8]; 20] = [
        b"null", b"Null", b"NULL", b"~", b"true", b"True", b"TRUE", b"false", b"False", b"FALSE", b".inf", b".Inf", b".INF", b"-.inf",
        b"-.Inf", b"-.INF", b"+.inf", b".nan", b".NaN", b".NAN",
    ];
    // Every keyword is at most five bytes and starts with one of these.
    if !s.is_empty() && s.len() <= 5 && b"nNtTfF~.-+".contains(&s[0]) && KEYWORDS.iter().any(|kw| *kw == s) {
        return true;
    }
    looks_numeric(s)
}

/// Whether `s` would parse as a YAML number. Zig's float parser also accepts
/// bare `inf`/`nan`, which the core schema does not, so those are excluded.
fn looks_numeric(s: &[u8]) -> bool {
    // Both parsers read a sign, a digit or (a float) a `.` first.
    if s.is_empty() || !(s[0].is_ascii_digit() || s[0] == b'+' || s[0] == b'-' || s[0] == b'.') {
        return false;
    }
    if ascii_contains(s, b"inf") || ascii_contains(s, b"nan") {
        return false;
    }
    zig_parse_int_ok(s, true) || zig_parse_int_ok(s, false) || zig_parse_float_ok(s)
}

/// Whether Zig's `std.fmt.parseInt(i64 or u64, s, 10)` reads `s`: an optional
/// sign, then digits with `_` anywhere but first or last, in range.
fn zig_parse_int_ok(s: &[u8], signed: bool) -> bool {
    if s.is_empty() {
        return false;
    }
    let (neg, body) = match s[0] {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    if body.is_empty() || body[0] == b'_' || body[body.len() - 1] == b'_' {
        return false;
    }
    let mut acc: i128 = 0;
    for &c in body {
        if c == b'_' {
            continue;
        }
        if !c.is_ascii_digit() {
            return false;
        }
        acc = acc * 10 + (c - b'0') as i128;
        if acc > u64::MAX as i128 {
            return false;
        }
    }
    let v = if neg { -acc } else { acc };
    if signed {
        v >= i64::MIN as i128 && v <= i64::MAX as i128
    } else {
        v >= 0 && v <= u64::MAX as i128
    }
}

/// Whether Zig's `std.fmt.parseFloat(f64, s)` reads `s` (its syntax; every
/// spelling it reads converts).
fn zig_parse_float_ok(s: &[u8]) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut i = 0usize;
    if s[0] == b'-' || s[0] == b'+' {
        i += 1;
    }
    if s.len() == i {
        return false;
    }
    let body = &s[i..];
    float_number_ok(body) || float_inf_or_nan_ok(body)
}

fn float_inf_or_nan_ok(s: &[u8]) -> bool {
    let starts = |s: &[u8], p: &[u8]| s.len() >= p.len() && s[..p.len()].eq_ignore_ascii_case(p);
    if starts(s, b"inf") {
        let n = if starts(&s[3..], b"inity") { 8 } else { 3 };
        return s.len() == n;
    }
    if starts(s, b"nan") {
        return s.len() == 3;
    }
    false
}

fn float_number_ok(s: &[u8]) -> bool {
    let (rest, base, exp_char): (&[u8], u32, u8) =
        if s.len() >= 2 && s[0] == b'0' && s[1].to_ascii_lowercase() == b'x' { (&s[2..], 16, b'p') } else { (s, 10, b'e') };
    let is_digit = |c: u8| (c as char).is_digit(base);
    let mut off = 0usize;
    let mut underscores = 0usize;
    let mut n_digits = 0usize;
    // Digits (and `_`) as `scanDigit` reads them.
    let scan = |off: &mut usize, underscores: &mut usize, base_digit: &dyn Fn(u8) -> bool| -> usize {
        let mut n = 0;
        while *off < rest.len() {
            let c = rest[*off];
            if base_digit(c) {
                n += 1;
                *off += 1;
            } else if c == b'_' {
                *underscores += 1;
                *off += 1;
            } else {
                break;
            }
        }
        n
    };
    n_digits += scan(&mut off, &mut underscores, &is_digit);
    if off < rest.len() && rest[off] == b'.' {
        off += 1;
        n_digits += scan(&mut off, &mut underscores, &is_digit);
    }
    if n_digits == 0 {
        return false;
    }
    if off < rest.len() && rest[off] | 0x20 == exp_char {
        off += 1;
        if off < rest.len() && (rest[off] == b'-' || rest[off] == b'+') {
            off += 1;
        }
        if !(off < rest.len() && rest[off].is_ascii_digit()) {
            return false;
        }
        scan(&mut off, &mut underscores, &|c: u8| c.is_ascii_digit());
    }
    if off != rest.len() {
        return false;
    }
    if underscores > 0 && !valid_underscores(rest, base) {
        return false;
    }
    true
}

/// Zig's `parse_float.validUnderscores`.
fn valid_underscores(s: &[u8], base: u32) -> bool {
    let is_digit = |c: u8| (c as char).is_digit(base);
    let mut i = 0usize;
    while i < s.len() {
        if s[i] == b'_' {
            if i == 0 || i + 1 == s.len() {
                return false;
            }
            if !is_digit(s[i - 1]) || !is_digit(s[i + 1]) {
                return false;
            }
            i += 1;
        }
        i += 1;
    }
    true
}

/// Case-insensitive ASCII substring test.
fn ascii_contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

pub fn write_single_quoted(writer: &mut Vec<u8>, s: &[u8]) {
    writer.push(b'\'');
    for &c in s {
        if c == b'\'' {
            writer.push(b'\''); // '' escapes a quote
        }
        writer.push(c);
    }
    writer.push(b'\'');
}

pub fn write_double_quoted(writer: &mut Vec<u8>, s: &[u8]) {
    writer.push(b'"');
    for &c in s {
        match c {
            b'"' => writer.extend_from_slice(b"\\\""),
            b'\\' => writer.extend_from_slice(b"\\\\"),
            b'\n' => writer.extend_from_slice(b"\\n"),
            b'\t' => writer.extend_from_slice(b"\\t"),
            b'\r' => writer.extend_from_slice(b"\\r"),
            _ => {
                if c < 0x20 || c == 0x7f {
                    writer.extend_from_slice(format!("\\x{c:02x}").as_bytes());
                } else {
                    writer.push(c);
                }
            }
        }
    }
    writer.push(b'"');
}

// ── Block scalars (multi-line strings) ──────────────────────────────────────

/// Emit `id` as a `|` block scalar when it is a multi-line string a block
/// scalar can round-trip; returns whether it did.
fn try_write_block_string_value(writer: &mut Vec<u8>, ast: &AST, id: NodeId, indent: usize) -> bool {
    let NodeKind::String(s) = ast.node(id).kind else { return false };
    if !block_scalar_ok(s) {
        return false;
    }
    write_props(writer, ast, id);
    write_block_scalar(writer, s, indent);
    true
}

/// Whether a multi-line string can be faithfully emitted as a `|` block
/// scalar. Anything rejected here is double-quoted instead.
fn block_scalar_ok(s: &[u8]) -> bool {
    if !s.contains(&b'\n') {
        return false;
    }
    if s.ends_with(b"\n\n") {
        return false;
    }
    let body = if s.ends_with(b"\n") { &s[..s.len() - 1] } else { s };
    let mut first_content = true;
    for line in body.split(|&c| c == b'\n') {
        if line.is_empty() {
            continue; // interior blank lines are fine (emitted empty)
        }
        if first_content && (line[0] == b' ' || line[0] == b'\t') {
            return false;
        }
        first_content = false;
        if line[line.len() - 1] == b' ' || line[line.len() - 1] == b'\t' {
            return false;
        }
        if line.iter().any(|&c| c != b'\t' && (c < 0x20 || c == 0x7f)) {
            return false;
        }
    }
    true
}

/// Write the block-scalar indicator and indented content for a string that
/// passed `block_scalar_ok`. `|` clips a single trailing newline, `|-` strips
/// a missing one.
fn write_block_scalar(writer: &mut Vec<u8>, s: &[u8], indent: usize) {
    let clip = s.ends_with(b"\n");
    writer.extend_from_slice(if clip { b"|\n" } else { b"|-\n" });
    let body = if clip { &s[..s.len() - 1] } else { s };
    for line in body.split(|&c| c == b'\n') {
        if line.is_empty() {
            writer.push(b'\n');
        } else {
            write_indent(writer, indent);
            writer.extend_from_slice(line);
            writer.push(b'\n');
        }
    }
}

fn write_indent(writer: &mut Vec<u8>, depth: usize) {
    for _ in 0..depth {
        writer.extend_from_slice(b"  ");
    }
}

// ── Comments ────────────────────────────────────────────────────────────────
// YAML has only `#` line comments. A `block` comment captured from another
// format degrades to a run of `#` lines, one per content line.

/// Emit a node's leading comments above its line, at `depth`.
fn leading_comments(writer: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize) {
    for c in ast.comments(id).leading.iter() {
        for line in c.text.split(|&ch| ch == b'\n') {
            write_indent(writer, depth);
            write_hash_line(writer, trim_blank(line));
            writer.push(b'\n');
        }
    }
}

/// Emit a node's trailing comment after its value (` # …`). A multi-line block
/// is flattened to one line (newlines → spaces).
fn trailing_comment(writer: &mut Vec<u8>, ast: &AST, id: NodeId) {
    let Some(c) = ast.comments(id).trailing else { return };
    writer.extend_from_slice(b" #");
    if !c.text.is_empty() {
        writer.push(b' ');
        for &ch in c.text {
            writer.push(if ch == b'\n' { b' ' } else { ch });
        }
    }
}

/// Emit a container's dangling comments at `depth`, each as one or more `#`
/// lines.
fn dangling_comments(writer: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize) {
    for c in ast.comments(id).dangling.iter() {
        for line in c.text.split(|&ch| ch == b'\n') {
            write_indent(writer, depth);
            write_hash_line(writer, trim_blank(line));
            writer.push(b'\n');
        }
    }
}

fn trim_blank(s: &[u8]) -> &[u8] {
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

/// Write `# text` (or a bare `#` for an empty comment).
fn write_hash_line(writer: &mut Vec<u8>, text: &[u8]) {
    writer.push(b'#');
    if !text.is_empty() {
        writer.push(b' ');
        writer.extend_from_slice(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fig_kit::ast::{Builder, Comment, Entry, NodeComments, Style};
    use fig_kit::document::Document;
    use crate::{parser, Type};
    use std::borrow::Cow;

    /// `src` as fig adopts it from the compiled YAML parser.
    fn test_doc(src: &[u8]) -> Document {
        parser::parse(src, Type::V1_2_2).unwrap()
    }

    fn printed(ast: &AST) -> Vec<u8> {
        let mut out = Vec::new();
        print(&mut out, ast);
        out
    }

    fn printed_with(ast: &AST, opts: &SerializeOptions) -> Vec<u8> {
        let mut out = Vec::new();
        print_with(&mut out, ast, opts);
        out
    }

    fn str_of(b: &[u8]) -> &str {
        std::str::from_utf8(b).unwrap()
    }

    /// Parse `src` as YAML and re-emit it with default options, asserting the
    /// exact output.
    fn expect_round_trip(src: &str, expected: &str) {
        let doc = test_doc(src.as_bytes());
        assert_eq!(expected, str_of(&printed(&doc.ast)), "{src:?}");
    }

    fn line(text: &'static str) -> Comment<'static> {
        Comment { text: text.as_bytes(), style: Style::Line }
    }

    #[test]
    fn yaml_emits_comments_block_degrades_to_a_hash_run() {
        let mut b = Builder::init();
        let v_name = b.add_string(b"fig").unwrap();
        b.set_comments(v_name, &NodeComments { trailing: Some(line("inline")), ..Default::default() }).unwrap();
        let k_name = b.add_string(b"name").unwrap();
        b.set_comments(k_name, &NodeComments { leading: Cow::Owned(vec![line("greeting")]), ..Default::default() }).unwrap();

        let n1 = b.add_int(1).unwrap();
        b.set_comments(n1, &NodeComments { leading: Cow::Owned(vec![Comment { text: b"first\nsecond", style: Style::Block }]), ..Default::default() })
            .unwrap();
        let n2 = b.add_int(2).unwrap();
        b.set_comments(n2, &NodeComments { trailing: Some(line("two")), ..Default::default() }).unwrap();
        let v_nums = b.add_sequence(&[n1, n2]).unwrap();
        let k_nums = b.add_string(b"nums").unwrap();

        let root = b.add_mapping(&[Entry { key: k_name, value: v_name }, Entry { key: k_nums, value: v_nums }]).unwrap();
        b.set_comments(root, &NodeComments { leading: Cow::Owned(vec![line("config")]), ..Default::default() }).unwrap();

        let ast = b.finish(root).unwrap();
        assert_eq!("# config\n# greeting\nname: fig # inline\nnums:\n# first\n# second\n- 1\n- 2 # two\n", str_of(&printed(&ast)));
    }

    #[test]
    fn a_comment_on_a_key_line_with_a_block_value_rides_the_key_line() {
        for src in ["contents: # note\n- a\n- b\n", "outer: # note\n  inner: 1\n"] {
            expect_round_trip(src, src);
        }
    }

    #[test]
    fn prints_yaml_document() {
        let mut b = Builder::init();
        let zig = b.add_string(b"zig").unwrap();
        let t = b.add_bool(true).unwrap();
        let nl = b.add_null().unwrap();
        let tags = b.add_sequence(&[zig, t, nl]).unwrap();
        let kn = b.add_string(b"name").unwrap();
        let vn = b.add_string(b"Ada").unwrap();
        let kt = b.add_string(b"tags").unwrap();
        let root = b.add_mapping(&[Entry { key: kn, value: vn }, Entry { key: kt, value: tags }]).unwrap();
        let ast = b.finish(root).unwrap();
        // The short, comment-free `tags` sequence inlines as flow within the width.
        assert_eq!("name: Ada\ntags: [zig, true, null]\n", str_of(&printed(&ast)));
    }

    #[test]
    fn nested_block_value_on_a_sequence_items_first_pair_indents_under_the_key() {
        let mut b = Builder::init();
        let ik = b.add_string(b"inner").unwrap();
        let iv = b.add_int(1).unwrap();
        let inner = b.add_mapping(&[Entry { key: ik, value: iv }]).unwrap();
        let ok = b.add_string(b"outer").unwrap();
        let item0 = b.add_mapping(&[Entry { key: ok, value: inner }]).unwrap();
        let lk = b.add_string(b"last").unwrap();
        let lv = b.add_int(3).unwrap();
        let item1 = b.add_mapping(&[Entry { key: lk, value: lv }]).unwrap();
        let seq = b.add_sequence(&[item0, item1]).unwrap();
        let ak = b.add_string(b"audience").unwrap();
        let root = b.add_mapping(&[Entry { key: ak, value: seq }]).unwrap();
        let ast = b.finish(root).unwrap();
        let opts = SerializeOptions { width: 0, ..Default::default() };
        assert_eq!("audience:\n- outer:\n    inner: 1\n- last: 3\n", str_of(&printed_with(&ast, &opts)));
    }

    #[test]
    fn nested_sequence_on_a_sequence_items_first_pair_indents_under_the_key() {
        let mut b = Builder::init();
        let one = b.add_int(1).unwrap();
        let two = b.add_int(2).unwrap();
        let nums = b.add_sequence(&[one, two]).unwrap();
        let ok = b.add_string(b"outer").unwrap();
        let item0 = b.add_mapping(&[Entry { key: ok, value: nums }]).unwrap();
        let seq = b.add_sequence(&[item0]).unwrap();
        let ak = b.add_string(b"audience").unwrap();
        let root = b.add_mapping(&[Entry { key: ak, value: seq }]).unwrap();
        let ast = b.finish(root).unwrap();
        let opts = SerializeOptions { width: 0, ..Default::default() };
        assert_eq!("audience:\n- outer:\n  - 1\n  - 2\n", str_of(&printed_with(&ast, &opts)));
    }

    #[test]
    fn flow_short_comment_free_mapping_values_inline_sequence_items_stay_block() {
        expect_round_trip("branches:\n- main\n", "branches: [main]\n");
        expect_round_trip("concurrency:\n  group: ci\n  cancel: true\n", "concurrency: { group: ci, cancel: true }\n");
        expect_round_trip("steps:\n- uses: checkout\n", "steps:\n- uses: checkout\n");
    }

    #[test]
    fn flow_a_comment_or_overflow_keeps_a_collection_block() {
        expect_round_trip("nums:\n# note\n- 1\n- 2\n", "nums:\n# note\n- 1\n- 2\n");
        let long = "x".repeat(90);
        let src = format!("items:\n- {long}\n");
        expect_round_trip(&src, &src);
    }

    #[test]
    fn a_nested_block_sequence_leaves_no_trailing_space_after_the_dash() {
        expect_round_trip("- - a\n  - b\n- c\n", "-\n  - a\n  - b\n- c\n");
        expect_round_trip("- []\n", "- []\n");
    }

    #[test]
    fn a_byte_string_is_binary_its_tag_written_once() {
        expect_round_trip("a: !!binary aGk=\n", "a: !!binary aGk=\n");
        expect_round_trip("a: !!binary |\n  aGVs\n  bG8=\n", "a: !!binary aGVsbG8=\n");
        expect_round_trip("a: !!binary \"\"\n", "a: !!binary \"\"\n");
        expect_round_trip("[!!binary aGk=]\n", "- !!binary aGk=\n");
    }

    #[test]
    fn every_key_kind_has_a_spelling_that_re_parses() {
        expect_round_trip(": a\n", "null: a\n");
        expect_round_trip("23: x\n!!str 23: y\n", "23: x\n!!str 23: y\n");
        expect_round_trip("true: x\n", "true: x\n");
        expect_round_trip("x: &n v\n*n : w\n", "x: &n v\n*n : w\n");
        expect_round_trip("? - a\n  - b\n: c\n", "? [a, b]\n: c\n");
        expect_round_trip("? []\n: c\n", "? []\n: c\n");
        expect_round_trip("{ &a [a, &b b]: *b, *a : c }\n", "? &a\n  - a\n  - &b b\n: *b\n*a : c\n");
        expect_round_trip("? a: b\n: c\n", "? { a: b }\n: c\n");
        expect_round_trip("? []: x\n", "?\n  ? []\n  : x\n: null\n");
        expect_round_trip("- ? [[b, c]]\n  : d\n", "- ? [[b, c]]\n  : d\n");
        expect_round_trip("x:\n  1: a\n  null: b\n", "x: { 1: a, null: b }\n");
    }

    #[test]
    fn a_re_printed_key_re_parses_to_the_same_node_kind() {
        for src in [": a\n", "23: x\n", "x: &n v\n*n : w\n", "? - a\n  - b\n: c\n", "? a: b\n: c\n", "? []: x\n"] {
            let doc = test_doc(src.as_bytes());
            let again = test_doc(&printed(&doc.ast));
            let key_of = |ast: &AST| ast.node(ast.node(ast.node(ast.root).kind.as_mapping().unwrap().unwrap()).kind.as_keyvalue().unwrap().key).kind.tag();
            assert_eq!(key_of(&doc.ast), key_of(&again.ast), "{src:?}");
        }
    }

    #[test]
    fn flow_a_flow_indicator_in_a_scalar_forces_quoting() {
        expect_round_trip("concurrency:\n  group: ci-${{ github.ref }}\n  x: 1\n", "concurrency: { group: 'ci-${{ github.ref }}', x: 1 }\n");
    }

    #[test]
    fn flow_output_is_idempotent() {
        let src = "on:\n  push:\n    branches: [main]\nconcurrency: { group: ci, cancel: true }\n";
        let first = printed(&test_doc(src.as_bytes()).ast);
        let second = printed(&test_doc(&first).ast);
        assert_eq!(first, second);
    }

    /// Build `{ s: value }`, serialize it, and return the YAML.
    fn emit_string_value(value: &[u8]) -> Vec<u8> {
        let mut b = Builder::init();
        let v = b.add_string(value).unwrap();
        let k = b.add_string(b"s").unwrap();
        let root = b.add_mapping(&[Entry { key: k, value: v }]).unwrap();
        let ast = b.finish(root).unwrap();
        printed(&ast)
    }

    #[test]
    fn multi_line_string_value_emits_a_strip_block_scalar() {
        assert_eq!("s: |-\n  multi\n  line\n  text\n", str_of(&emit_string_value(b"multi\nline\ntext")));
    }

    #[test]
    fn a_trailing_newline_clips_two_fall_back_to_double_quote() {
        assert_eq!("s: |\n  a\n  b\n", str_of(&emit_string_value(b"a\nb\n")));
        assert_eq!("s: \"a\\nb\\n\\n\"\n", str_of(&emit_string_value(b"a\nb\n\n")));
    }

    #[test]
    fn indented_interior_lines_still_block_scalar() {
        assert_eq!("s: |\n  if x; then\n    echo hi\n  fi\n", str_of(&emit_string_value(b"if x; then\n  echo hi\nfi\n")));
    }

    #[test]
    fn block_scalars_round_trip_through_the_parser() {
        let cases: [&[u8]; 9] = [
            b"multi\nline\ntext",
            b"a\nb\n",
            b"a\n\nb",
            b"a\nb\n\n",
            b"trailing \nspace",
            b" leading\nline",
            b"if x; then\n  echo hi\nfi\n",
            b"tab\there\nx",
            b"\rreturn\nx",
        ];
        for s in cases {
            let yaml = emit_string_value(s);
            let doc = test_doc(&yaml);
            let kv = doc.ast.node(doc.ast.node(doc.ast.root).kind.as_mapping().unwrap().unwrap()).kind.as_keyvalue().unwrap();
            assert_eq!(s, doc.ast.node(kv.value).kind.as_string().unwrap());
        }
    }

    #[test]
    fn a_tag_directive_is_written_back_above_the_tags_that_use_it() {
        let tagged = "%TAG !e! tag:example.com,2000:app/\n---\n- !local foo\n- !!str bar\n- !e!tag%21 baz\n";
        expect_round_trip(tagged, tagged);
        let secondary = "%TAG !! tag:example.com,2000:app/\n---\n- !!foo bar\n";
        expect_round_trip(secondary, secondary);
        expect_round_trip("%TAG !e! tag:example.com,2000:app/\n---\na: b\n", "a: b\n");
        expect_round_trip("%YAML 1.2\n---\na: b\n", "a: b\n");
    }

    #[test]
    fn a_root_collections_own_props_sit_on_a_line_above_it() {
        expect_round_trip("&m\na: b\n", "&m\na: b\n");
        expect_round_trip("!!seq\n- a\n", "!!seq\n- a\n");
        expect_round_trip("&m !!map\na: b\n", "&m !!map\na: b\n");
        expect_round_trip("&m {}\n", "&m {}\n");
        expect_round_trip("!!str foo\n", "!!str foo\n");
    }

    #[test]
    fn a_collection_sequence_item_keeps_its_props_on_the_dash_line() {
        expect_round_trip("- &a\n  - x\n", "- &a\n  - x\n");
        expect_round_trip("- &a\n  k: v\n", "- &a\n  k: v\n");
        expect_round_trip("- !!map\n  k: v\n", "- !!map\n  k: v\n");
        expect_round_trip("- &a {}\n", "- &a {}\n");
        expect_round_trip("- &a []\n", "- &a []\n");
    }

    #[test]
    fn reprinted_root_item_props_re_parse_onto_the_same_nodes() {
        for src in [
            "&m\na: b\n",
            "!!seq\n- a\n",
            "&m !!map\na: b\n",
            "- &a\n  - x\n",
            "- &a\n  k: v\n",
            "%TAG !e! tag:example.com,2000:app/\n---\n&m\na: !e!foo b\n",
        ] {
            let doc = test_doc(src.as_bytes());
            let again = test_doc(&printed(&doc.ast));
            assert!(doc.ast.eql(&again.ast), "{src:?}");
            assert!(doc.ast.tags_eql(&again.ast), "{src:?}");
            for id in 0..doc.ast.node_count() {
                let want = doc.ast.anchor_of(id as NodeId);
                let got = if id < again.ast.node_count() { again.ast.anchor_of(id as NodeId) } else { None };
                assert_eq!(want, got, "{src:?}");
            }
        }
    }

    #[test]
    fn zig_float_and_int_syntax() {
        // What Zig's `parseInt`/`parseFloat` read, which `looks_numeric` mirrors.
        for s in ["1", "-1", "+1", "1_0", "1__0", "1.5", ".5", "5.", "1e3", "1E+3", "1_0.5", "0x1F", "0x1.8p1", "1e1_0", "99999999999999999999999", "-0x10", "0X1f", "1_000.000_1"] {
            assert!(looks_numeric(s.as_bytes()), "{s}");
        }
        for s in ["_1", "1_", "1._5", "1e", "1e+", ".", "-", "+", "1.5x", "0x", "1,0", "1e_5", "--1"] {
            assert!(!looks_numeric(s.as_bytes()), "{s}");
        }
    }
}
