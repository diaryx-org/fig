//! Port of src/editor.zig.
//!
//! The generic span-splice engine: `Editor`, which edits a document of any
//! editable `Language` by splicing bytes into its source and reparsing, so
//! every comment, blank line and spelling outside the edit is kept. In Zig it
//! is `Editor(Language)`, generic over a comptime language; here it holds a
//! `&dyn Language` and the dialect (`format`) it reads the document as, and
//! every comptime question (`@hasDecl(Language, "renderX")`,
//! `is_section_format`, …) is asked of the language at run time.
//!
//! # Public API
//!
//! Construction and state:
//!
//! - `Editor::new(lang: &'l dyn Language, format: Dialect) -> Editor<'l>`
//! - `init(&mut self, input: &[u8]) -> Result<()>` — parse `input`; once only
//!   (`MultipleInit`).
//! - `source(&self) -> &[u8]` — the edited bytes.
//! - `get_parsed(&self) -> Result<&Document>` — the parse of `source()`
//!   (`NotInitialized` before `init`).
//! - `language(&self) -> &'l dyn Language`, `pub format: Dialect`,
//!   `pub splice_rejected: bool` (set when the last splice's reparse failed:
//!   the caller's text, not the file, is what does not parse).
//!
//! Every edit takes a path `&[PathSegment]` (`kit::ast::PathSegment`, `Key`
//! or `Index`; an empty path is the root). Value, key and body texts are the
//! format's own SPELLING (what the format would write), except where a
//! method says it takes a NAME. An edit is atomic: on `Err` the source and
//! the parse are exactly as they were. Errors are `crate::Error`s named as
//! Zig names them (`NotFound`, `NotAMapping`, `DuplicateKey`,
//! `CannotDeleteSection`, `EditReadsBackWrong`, …, or the parser's own).
//!
//! Values:
//! - `set(path, value_text)` — upsert; vivifies missing parents.
//! - `set_string(path, rendered, string)` — `set`, keeping a replaced
//!   string's quote style.
//! - `set_append(path, value_text)` — append, starting the list if missing.
//! - `set_sequence(path, items: &[&[u8]])` — reconcile a scalar list.
//! - `replace_val_at_path(path, replacement)`,
//!   `replace_string_at_path(path, rendered, string)`,
//!   `replace_val_at_path_following(path, replacement)` (edits through an
//!   alias to its anchor).
//!
//! Keys and entries:
//! - `replace_key_at_path(path, key_syntax)`, `replace_named_key(path, name)`
//! - `insert_key(path, key_syntax, value_text)`,
//!   `insert_named_key(path, name, value_text)`
//! - `delete_key(path)`, `move_key(src_path, dest_path)`,
//!   `reorder_keys(path, keys: &[&[u8]])`
//! - `format_insert_key(name) -> Result<Vec<u8>>` — a name as key syntax.
//!
//! Sequences:
//! - `append_to_seq(path, v)`, `prepend_to_seq(path, v)`,
//!   `insert_item(path, index, v)`, `remove_seq_item(path, index)` (index
//!   `usize::MAX` is "the end"), `move_item(path, from, to)`,
//!   `reorder_items(path, indices: &[usize])`.
//!
//! Comments (`Result<Option<Vec<u8>>>` for the getters; `None` is "none"):
//! - `add_leading_comment`, `delete_leading_comments`, `get_leading_comment`
//! - `set_trailing_comment`, `delete_trailing_comment`, `get_trailing_comment`
//! - `add_dangling_comment`, `delete_dangling_comments`,
//!   `get_dangling_comment`
//! - `comment_out(path)`, `uncomment_leading(path, first_line, line_count)`,
//!   `uncomment_dangling(container_path, first_line, line_count)`
//!
//! Whole containers (section formats — TOML, INI, figl; a format without
//! the op answers `UnsupportedFormat`, see `has_container_op` and
//! `dialect_has_container_op`):
//! - `delete_container(path)`, `insert_container(path, body_text)`,
//!   `rename_container(path, new_leaf_name)`,
//!   `move_container(src_path, dest_path: Option<&[PathSegment]>)`,
//!   `reorder_containers(order: &[&[u8]])`,
//!   `append_container_to_seq(path, body_text)`.
//!
//! Lower level (public in Zig, used by the CLI and the languages' tests):
//! `replace_at_span(span, text)`, `indent_at(at)`, `write_map_value`,
//! `insert_block_key`, `gather_regions`, `section_extent_end`, and the free
//! function `literal_of`.

pub mod regions;
pub mod splice;

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use self::regions::Region;
use self::splice::{
    append_block_sep, at_line_start, column_of, comment_block_start, continuation_end, find, find_any, find_byte,
    find_byte_from, find_from, first_non_space, flow_open_end, full_order, is_flow, is_whitespace, line_end_after,
    line_start_before, text_start, tile_blocks, trim, trim_end, trim_start, Block, Comments,
};
use crate::err;
use crate::kit::ast::{Node, NodeId, NodeKind, NodeTag, PathSegment, AST};
use crate::kit::document::{Document, MentionIterator, MentionKind};
use crate::kit::language::Language;
use crate::kit::literal as bare_literal;
use crate::kit::manifest::{
    CommentDelimiter, Delimiters, Dialect, Literal, RenderRequest, Renderer, SectionHeader, Syntax,
};
use crate::kit::span::Span;
use crate::kit::table;
use crate::{Error, Result};

/// What the bare-literal rule (`kit/literal.rs`) makes of `text`, trimmed
/// of whitespace: the `Literal` a value renderer is handed. `true`, `42`,
/// `2.5`, `2026-09-10` and `null` are typed and `Yes`, `007` and `TRUE`
/// stay strings, in every format alike.
pub fn literal_of(text: &[u8]) -> Literal {
    match bare_literal::sniff_bare(trim(text, b" \t\r\n")) {
        bare_literal::Sniffed::Null => Literal::Null,
        bare_literal::Sniffed::Boolean(_) => Literal::Bool,
        bare_literal::Sniffed::Number(n) => {
            if n.kind == bare_literal::NumberKind::Integer {
                Literal::Int
            } else {
                Literal::Float
            }
        }
        bare_literal::Sniffed::Datetime { .. } => Literal::Datetime,
        bare_literal::Sniffed::String => Literal::String,
    }
}

// ── small helpers over paths and kinds ──────────────────────────────────────

fn tag(n: &Node<'_>) -> NodeTag {
    n.kind.tag()
}

fn is_mapping(n: &Node<'_>) -> bool {
    matches!(n.kind, NodeKind::Mapping(_))
}

fn is_sequence(n: &Node<'_>) -> bool {
    matches!(n.kind, NodeKind::Sequence(_))
}

fn is_keyvalue(n: &Node<'_>) -> bool {
    matches!(n.kind, NodeKind::KeyValue(_))
}

fn is_null(n: &Node<'_>) -> bool {
    matches!(n.kind, NodeKind::Null)
}

/// The keyvalue payload of `n`, which the caller knows is a keyvalue.
fn kv_of(n: &Node<'_>) -> crate::kit::ast::KeyValue {
    n.kind.as_keyvalue().expect("a keyvalue")
}

/// The key a path's last segment names, or `None` for an index or the root.
fn last_key<'p>(path: &[PathSegment<'p>]) -> Option<&'p [u8]> {
    match path.last() {
        Some(PathSegment::Key(k)) => Some(k),
        _ => None,
    }
}

fn last_is_index(path: &[PathSegment<'_>]) -> bool {
    matches!(path.last(), Some(PathSegment::Index(_)))
}

/// Zig's `.?` on a container's first or last child, which the caller knows
/// is there.
fn present<T>(x: Option<T>) -> Result<T> {
    x.ok_or(err!(NotFound))
}

/// How the items of the flow sequence `node` are joined: the bytes
/// written between two of them on one line, split around the token
/// a delete consumes. The dialect's is `flow_entry_sep` and a space
/// after it (`, `); a separated sequence's is the one its parser
/// named (`Document::item_sep`), split at its whitespace
/// (` | ` is `|` with a space either side; ` ` is all whitespace and
/// no token, so a delete consumes only the run of whitespace). An
/// insert into a flow mapping is joined by one too (`flow_map_joint`).
#[derive(Debug, Clone, Copy)]
struct Joint<'a> {
    lead: &'a [u8],
    token: &'a [u8],
    trail: &'a [u8],
    /// The language named this separator for this container. Such a
    /// sequence's items may be anything the language reads there, so
    /// no flow-only rule refuses a value before the reparse does.
    declared: bool,
}

impl<'a> Joint<'a> {
    /// Whether the separator itself breaks the line (`"\n\n"`), so
    /// an item written after it starts a line of its own.
    fn breaks_line(&self) -> bool {
        find_byte(self.lead, b'\n').is_some() || find_byte(self.trail, b'\n').is_some()
    }

    /// `text` split at its whitespace: what comes before its first
    /// other byte, those bytes, and what comes after.
    fn split(text: &'a [u8], declared: bool) -> Joint<'a> {
        let ws: &[u8] = b" \t\r\n";
        let Some(start) = text.iter().position(|c| !ws.contains(c)) else {
            return Joint { lead: text, token: b"", trail: b"", declared };
        };
        let end = text.iter().rposition(|c| !ws.contains(c)).unwrap() + 1;
        Joint { lead: &text[..start], token: &text[start..end], trail: &text[end..], declared }
    }
}

/// Where a fragment is written, as a renderer is told it: the
/// container it goes into, by the name of the key that container
/// stands under and by its tag. See `RenderRequest`.
#[derive(Debug, Clone, Copy, Default)]
struct Place<'a> {
    key: &'a [u8],
    tag: &'a [u8],
}

/// `container` as a `Place`: the key of the entry holding it —
/// empty at the root and for a sequence item — and its tag.
fn place_of<'a>(parsed: &'a Document, container: Node<'_>) -> Place<'a> {
    let ast = &parsed.ast;
    let key: &[u8] = match ast.parent_of(container.id) {
        Some(p) => match ast.kind_of(p) {
            NodeKind::KeyValue(kv) if kv.value == container.id => key_name(ast.node(kv.key)),
            _ => b"",
        },
        None => b"",
    };
    Place { key, tag: ast.tag_of(container.id).map(|t| t.spelling()).unwrap_or(b"") }
}

/// The `Place` of the container holding node `id` — a value, an
/// item, an entry or its key — or the empty place for the root.
fn place_of_child(parsed: &Document, id: NodeId) -> Place<'_> {
    let ast = &parsed.ast;
    // The container is the parent, or — for an entry's key or
    // value — the entry's parent.
    let Some(mut p) = ast.parent_of(id) else { return Place::default() };
    if matches!(ast.kind_of(p), NodeKind::KeyValue(_)) {
        match ast.parent_of(p) {
            Some(pp) => p = pp,
            None => return Place::default(),
        }
    }
    match ast.kind_of(p) {
        NodeKind::Mapping(_) | NodeKind::Sequence(_) => place_of(parsed, ast.node(p)),
        _ => Place::default(),
    }
}

/// A key's name, decoded: what a path segment names it by.
fn key_name<'a>(key: Node<'a>) -> &'a [u8] {
    match key.kind {
        NodeKind::String(t) => t,
        NodeKind::Number(n) => n.raw,
        NodeKind::Extended(x) => x.text,
        NodeKind::Boolean(b) => {
            if b {
                b"true"
            } else {
                b"false"
            }
        }
        _ => b"",
    }
}

/// Where `node` BEGINS on its own line: its item marker's start when
/// the parser recorded one (`Document::marker_span`), else its
/// span's. This is the byte every line-anchored computation starts
/// from — the line an item lives on, the prefix a sibling copies, the
/// block a delete or reorder takes — because a nested or empty item's
/// span can start lines below the `-` that introduces it.
fn marker_start(parsed: &Document, node: Node<'_>) -> usize {
    if let Some(m) = parsed.marker_span(&node) {
        return m.start;
    }
    parsed.span(&node).start
}

/// The line-splice ops the section rule refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionOp {
    Delete,
    Replace,
    Move,
    Reorder,
    NotASection,
    Exists,
}

/// The engine's refusal of `op` on a section. The errors are the
/// operation's, not any one format's: TOML's `[table]`, figl's block
/// container and INI's `[section]` are one thing here, and a format's own
/// word for it is its declared `section_noun`, for a consumer's message.
fn refuse(op: SectionOp) -> Error {
    match op {
        SectionOp::NotASection => err!(NotASection),
        SectionOp::Delete => err!(CannotDeleteSection),
        SectionOp::Replace => err!(CannotReplaceSection),
        SectionOp::Move => err!(CannotMoveSection),
        SectionOp::Reorder => err!(CannotReorderSections),
        SectionOp::Exists => err!(SectionExists),
    }
}

/// Where a container's dangling run lives: the offset it starts at
/// (just past the container body's last line) and the line prefix its
/// lines carry (the body's child depth).
struct DanglingAnchor {
    at: usize,
    indent: Vec<u8>,
}

/// One rewrite `following_edits` asks for: `text` over `span`.
#[derive(Debug, Clone)]
struct PathEdit {
    span: Span,
    text: Vec<u8>,
}

/// A table made by dotted keys inside a flow mapping, as
/// `dotted_in_flow` finds it: `flow` is the flow mapping, and `top`
/// the table among its children whose name begins each entry that
/// writes the table (`a` in `{ a.b.c = 1 }`, for `a` or `a.b`).
#[derive(Clone, Copy)]
struct DottedInFlow<'a> {
    flow: Node<'a>,
    top: Node<'a>,
}

/// A table with no header of its own, as `headerless_holder` finds
/// it: `holder` is the nearest mapping up that has lines or braces
/// of its own, which write the table's entries; `top` is the table
/// among the holder's children on the way down, whose name begins
/// each of those entries; `depth` is how many keys down from the
/// holder the table is; `flow` is whether the holder is a flow
/// mapping.
#[derive(Clone, Copy)]
struct Headerless<'a> {
    holder: Node<'a>,
    top: Node<'a>,
    depth: usize,
    flow: bool,
}

/// How a splice's `value_text` is *spelled* — whether it stands as an
/// inline value right after `key<sep>`, or is a block construct that has
/// to descend onto the following lines.
///
/// A splice carries only text, so this is the classification the framing
/// decisions hang off. The container cases are settled by PARSING the
/// text rather than sniffing its shape: a one-entry block mapping
/// (`k: v`) is a single line with no dash and no line break, so shape
/// alone cannot tell it from a scalar — and splicing it inline yields
/// `key: k: v`, which is not YAML at all. That indistinguishability is
/// the whole reason this enum exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueShape {
    /// A scalar, a flow container (`[a, b]` / `{k: v}`), or a quoted
    /// string: splices directly after the separator.
    Inline,
    /// A block-scalar header (`|`/`>`): also splices after the separator
    /// (its body is already indented), but has no flow spelling.
    BlockScalar,
    /// A block sequence (`- a`), which descends at the KEY's own column.
    BlockSeq,
    /// A block mapping (`k: v`), which descends indented under the key.
    BlockMap,
}

/// Where a write puts its value, as `reading_of` reads it: the
/// document, a mapping entry, an item of a block list (one per line,
/// after `seq_item_marker`), or an item of any other list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteSlot {
    Root,
    Entry,
    BlockItem,
    FlowItem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadSlot {
    Item,
    Entry,
    Alone,
}

/// Which item insert `add_item` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemOp {
    Append,
    Prepend,
    Insert,
}

/// The source as it stood before an edit, for a check to compare the
/// edit's result with and to roll it back to, and the document parsed
/// from it (Zig keeps that document alive across the edit's reparse
/// — `kept` — so the check need not parse the backup again; here the
/// document is shared, and simply held).
struct Before {
    bytes: Vec<u8>,
    doc: Option<Arc<Document>>,
}

/// What a write, move or reorder is checked against once its splice
/// is in: the source before it, the container it changes (`route`,
/// from the root), and the one change it may make there.
struct Guard {
    before: Before,
    route: Vec<usize>,
    write: Write,
}

/// The editor: a source, its parse, and the language and dialect it is
/// read as. See the module doc for the API.
pub struct Editor<'l> {
    lang: &'l dyn Language,
    /// The dialect the document is read (and every splice reparsed) as.
    pub format: Dialect,
    pub(crate) source: Vec<u8>,
    document: Option<Arc<Document>>,
    /// Set by `replace_at_span` when the reparse it does after splicing
    /// FAILED — i.e. the caller's replacement text, not the document it
    /// went into, is what doesn't parse (the document parsed at `init`,
    /// and every edit routes through that one splice gate). The error
    /// itself can't say this: it is an ordinary parse error, identical to
    /// what a malformed input file produces. Callers that hand a user's
    /// raw text through — the CLI's `replace`/`set`/`insert` — read this to
    /// blame the text instead of the file. Cleared at the top of each
    /// splice, so it always describes the most recent one.
    pub splice_rejected: bool,
    /// Whether this format has section nodes at all — a non-null
    /// `section_noun` in any dialect (Zig's comptime `is_section_format`).
    /// A runtime language answers "may be", with `syntax()` deciding per
    /// entry at the call.
    is_section_format: bool,
    /// Whether any dialect declares a `section_header`, and whether any
    /// declares its sequence form: the gates on the two ops that write a
    /// header line.
    has_section_header: bool,
    has_seq_header: bool,
}

impl<'l> Editor<'l> {
    /// An editor for `lang`, reading documents as `format`. Call `init`
    /// with the source before any edit.
    pub fn new(lang: &'l dyn Language, format: Dialect) -> Editor<'l> {
        let runtime = lang.is_runtime();
        let mut any_section = false;
        let mut any_header = false;
        let mut any_seq_header = false;
        for t in 0..lang.type_count() {
            let s = lang.syntax(t as Dialect);
            if s.section_noun.is_some() {
                any_section = true;
            }
            if let Some(h) = s.section_header {
                any_header = true;
                if h.seq_open.is_some() {
                    any_seq_header = true;
                }
            }
        }
        Editor {
            lang,
            format,
            source: Vec::new(),
            document: None,
            splice_rejected: false,
            is_section_format: runtime || any_section,
            has_section_header: runtime || any_header,
            has_seq_header: runtime || any_seq_header,
        }
    }

    /// The language this editor reads its document as.
    pub fn language(&self) -> &'l dyn Language {
        self.lang
    }

    /// Whether this format has section nodes at all (Zig's
    /// `is_section_format`): a non-null `section_noun` in any dialect, or a
    /// runtime language, which may.
    pub fn is_section_format(&self) -> bool {
        self.is_section_format
    }

    /// The edited bytes.
    pub fn source(&self) -> &[u8] {
        &self.source
    }

    /// Everything this format declares about its own surface syntax, for
    /// the dialect the document is currently being read as.
    ///
    /// Indexed by `self.format` because the comment markers genuinely vary
    /// by dialect (strict JSON has no comment syntax; JSONC and JSON5 do),
    /// and every splice is reparsed under whichever dialect the editor is
    /// holding.
    fn syntax(&self) -> Syntax {
        self.lang.syntax(self.format)
    }

    /// Whether `node` is a FLOW container — spelled inline, edited by
    /// comma-aware splice — rather than a block one edited by line.
    ///
    /// The one place the engine asks. Four answers, in order:
    ///
    ///   0. A sequence the parser gave a separator of its own
    ///      (`Document::item_sep`) is flow.
    ///   1. A format with no flow syntax (`Syntax.flow_containers ==
    ///      false`) has no flow containers, whatever a span begins with.
    ///   2. A section format's root, and any section node, is block: the
    ///      root's span opens on the first `[header]` (TOML, INI), and a
    ///      section node's span is its header's name token, so the
    ///      first-byte sniff would read either as a bracket-delimited
    ///      flow container.
    ///   3. Otherwise the first-byte sniff, `splice::is_flow`, which is
    ///      exact for every format that has both shapes.
    fn is_flow_node(&self, parsed: &Document, node: Node<'_>) -> bool {
        if parsed.item_sep(&node).is_some() {
            return true;
        }
        let syn = self.syntax();
        if !syn.flow_containers {
            return false;
        }
        // `is_section_format` is "may be" for a runtime language, so the
        // dialect's own `section_noun` settles it at the call.
        if self.is_section_format
            && syn.section_noun.is_some()
            && (node.id == parsed.ast.root || parsed.is_section(&node))
        {
            return false;
        }
        if self.is_open_root(parsed, node) {
            return true;
        }
        let source = &self.source[..];
        // A YAML node's span starts at its properties (`&i [1, 2]`), and
        // the sniff reads the value after them.
        let span = self.value_span_without_props(parsed, node);
        if !is_flow(source, span, syn.flow_map_open) {
            return false;
        }
        // `flow_maps_only`: a `[` is not an opener, so what it begins
        // (a CSS attribute selector) is read as block.
        !(syn.flow_maps_only && source[first_non_space(source, span.start)] == b'[')
    }

    /// Whether `node` is the root of a `flow_root` format: a flow mapping
    /// with no delimiters, whose members run from the source's first
    /// byte to its last (a CSS `style` attribute).
    fn is_open_root(&self, parsed: &Document, node: Node<'_>) -> bool {
        self.syntax().flow_root && node.id == parsed.ast.root && is_mapping(&node)
    }

    /// Whether the flow container `node` has no delimiters: an open root,
    /// or a sequence the parser said is open.
    fn is_open(&self, parsed: &Document, node: Node<'_>) -> bool {
        if let Some(is) = parsed.item_sep(&node) {
            return is.open;
        }
        self.is_open_root(parsed, node)
    }

    fn joint_of<'a>(&self, parsed: &'a Document, node: Node<'_>) -> Joint<'a> {
        let dialect = Joint { lead: b"", token: self.syntax().flow_entry_sep, trail: b" ", declared: false };
        let Some(is) = parsed.item_sep(&node) else { return dialect };
        let Some(text) = is.text else {
            return Joint { lead: b"", token: dialect.token, trail: b" ", declared: true };
        };
        Joint::split(text, true)
    }

    /// Append the bytes that go between an item and the next one of the
    /// flow sequence `node` to `out`, laid out as the sequence is: the
    /// separator as written when it breaks the line itself, then the
    /// first item's indent; the token, a newline and that indent when
    /// the sequence is one item per line; else the separator as written.
    fn write_joint(&self, out: &mut Vec<u8>, parsed: &Document, node: Node<'_>, joint: Joint<'_>) -> Result<()> {
        let source = &self.source[..];
        let first = present(parsed.ast.child(&node)?)?;
        let last = present(parsed.ast.last_child(&node)?)?;
        let col = column_of(source, parsed.span(&first).start);
        if joint.breaks_line() {
            out.extend_from_slice(joint.lead);
            out.extend_from_slice(joint.token);
            out.extend_from_slice(joint.trail);
            out.resize(out.len() + col, b' ');
        } else if self.flow_is_multi_line(parsed, node, parsed.span(&node), parsed.span(&last).end)? {
            out.extend_from_slice(joint.token);
            out.push(b'\n');
            out.resize(out.len() + col, b' ');
        } else {
            out.extend_from_slice(joint.lead);
            out.extend_from_slice(joint.token);
            out.extend_from_slice(joint.trail);
        }
        Ok(())
    }

    /// The prefix a new line at the same nesting as the byte `at` needs.
    ///
    /// For a format whose line prefix is STRUCTURAL (fig's `>` marker
    /// run, `Syntax.structural_indent`) it is the raw bytes from the line
    /// start to `at`, which reproduces the depth and the file's spaced or
    /// glued marker style. For every other format it is the line's
    /// leading whitespace, kept as the bytes it is so a tab-indented file
    /// stays tab-indented, padded with spaces out to `at`'s column only
    /// when what stands between is sequence item markers — a key inside
    /// a `- key: v` item, whose siblings align under the key and not
    /// under the dash. Past any other token — a value's `{` after its
    /// key, a section key inside `[…]`, a plist `<key>…</key>` — the
    /// line's own indent is the answer.
    pub fn indent_at(&self, at: usize) -> Vec<u8> {
        let source = &self.source[..];
        let line_start = line_start_before(source, at);
        let mut buf = Vec::new();
        let syn = self.syntax();
        if syn.structural_indent {
            buf.extend_from_slice(&source[line_start..at]);
        } else {
            let ws_end = first_non_space(source, line_start).min(at);
            buf.extend_from_slice(&source[line_start..ws_end]);
            if only_item_markers(&source[ws_end..at], syn.seq_item_marker) {
                buf.resize(buf.len() + (at - ws_end), b' ');
            }
        }
        buf
    }

    /// Whether this format renders `which` for the editor's dialect (Zig's
    /// `@hasDecl`, or a runtime language's own `hasRenderer`).
    fn has_renderer(&self, which: Renderer) -> bool {
        self.lang.has_renderer(self.format, which)
    }

    /// Call the renderer `which`, which `has_renderer` said is there.
    fn render(&self, which: Renderer, out: &mut Vec<u8>, r: RenderRequest<'_>) -> Result<()> {
        match self.lang.render(which, self.format, out, &r) {
            Some(res) => res,
            None => Ok(()),
        }
    }

    /// `value_text` as this format spells a value in place — rendered
    /// through the format's `render_value` when it declares one (plist
    /// wraps every literal in a typed element), else verbatim.
    ///
    /// **Renderer** `render_value`, where the request's `literal` is
    /// `literal_of(value_text)`: the engine classifies the text once, by
    /// fig's own bare-literal rules, so a renderer spells a kind it is told
    /// rather than deciding one. `place` is the container the value is
    /// written into.
    fn rendered_value<'v>(&self, value_text: &'v [u8], place: Place<'_>) -> Result<Cow<'v, [u8]>> {
        if !self.has_renderer(Renderer::Value) {
            return Ok(Cow::Borrowed(value_text));
        }
        let mut buf = Vec::new();
        self.render(
            Renderer::Value,
            &mut buf,
            RenderRequest {
                value: value_text,
                literal: literal_of(value_text),
                parent_key: place.key,
                parent_tag: place.tag,
                ..RenderRequest::default()
            },
        )?;
        Ok(Cow::Owned(buf))
    }

    /// Write one block-mapping entry after its line's `indent` has been
    /// written: the key, the separator and the value, with any further
    /// lines the entry spans prefixed by `indent` (plist's value element
    /// on a second line, NestedText's `>`-block). No trailing newline.
    ///
    /// **Renderer** `render_entry` — told the indent, key and value, and
    /// declared by a format whose entry is not `key`, `kv_sep`, value on
    /// one line. Without it the entry is the key followed by
    /// `write_map_value`. `value_text` has been through `rendered_value`
    /// already.
    fn write_entry(&self, out: &mut Vec<u8>, indent: &[u8], key_text: &[u8], value_text: &[u8], place: Place<'_>) -> Result<()> {
        if self.has_renderer(Renderer::Entry) {
            return self.render(
                Renderer::Entry,
                out,
                RenderRequest {
                    indent,
                    key: key_text,
                    value: value_text,
                    parent_key: place.key,
                    parent_tag: place.tag,
                    ..RenderRequest::default()
                },
            );
        }
        out.extend_from_slice(key_text);
        self.write_tail(out, indent, key_text, value_text, place)
    }

    /// Write everything that follows a key on its entry: the separator
    /// and the value inline (`: v`, ` = v`), or the value re-framed onto
    /// the following lines as a block (`:` then indented lines; fig's
    /// bare header over a `>` body). `key_text` is the key as written —
    /// so a renderer can tell a NestedText multiline key, which takes no
    /// separator — or empty for the DOCUMENT ROOT, where the value stands
    /// alone. No trailing newline.
    ///
    /// **Renderer** `render_tail`, told the indent, key and value. Without
    /// it the tail is `write_map_value`, which is YAML's answer and the
    /// default for every line-structured format.
    fn write_tail(&self, out: &mut Vec<u8>, indent: &[u8], key_text: &[u8], value_text: &[u8], place: Place<'_>) -> Result<()> {
        if self.has_renderer(Renderer::Tail) {
            return self.render(
                Renderer::Tail,
                out,
                RenderRequest {
                    indent,
                    key: key_text,
                    value: value_text,
                    parent_key: place.key,
                    parent_tag: place.tag,
                    ..RenderRequest::default()
                },
            );
        }
        self.write_map_value(out, indent, value_text)
    }

    /// Write one block-sequence item after its line's `indent`: the
    /// `seq_item_marker` and the value, with continuation lines of a
    /// multi-line value re-indented past the marker. No trailing newline.
    ///
    /// **Renderer** `render_item` — told the indent, the value and the
    /// sequence (`place`), and declared by a format whose item is not
    /// marker-then-value.
    fn write_item(&self, out: &mut Vec<u8>, indent: &[u8], value_text: &[u8], place: Place<'_>) -> Result<()> {
        if self.has_renderer(Renderer::Item) {
            return self.render(
                Renderer::Item,
                out,
                RenderRequest {
                    indent,
                    value: value_text,
                    parent_key: place.key,
                    parent_tag: place.tag,
                    ..RenderRequest::default()
                },
            );
        }
        let marker = self.syntax().seq_item_marker;
        let mut cont = indent.to_vec();
        cont.resize(cont.len() + marker.len(), b' ');
        out.extend_from_slice(marker);
        reindent_into(out, value_text, &cont);
        Ok(())
    }

    /// Whether the entry `kv` of a block mapping is written OUTSIDE the
    /// mapping's own lines: its value is a section whose name on that
    /// line is a `Header` mention — the line opens the child's own
    /// region rather than sitting on the parent's.
    fn out_of_region(&self, parsed: &Document, kv: Node<'_>) -> bool {
        let NodeKind::KeyValue(e) = kv.kind else { return false };
        let val = parsed.ast.node(e.value);
        if !parsed.is_section(&val) {
            return false;
        }
        let source = &self.source[..];
        let line = line_start_before(source, parsed.span(&kv).start);
        for m in parsed.mentions_of(val.id) {
            if m.kind == MentionKind::Header && line_start_before(source, m.span.start) == line {
                return true;
            }
        }
        false
    }

    /// The end of the first header line recorded for the section
    /// `mapping` that is its OWN — not the header of one of its
    /// children. A section whose every header line opens a child has no
    /// line to take an entry under, and an entry spliced after the first
    /// of them would land in that child; it is refused (`ImplicitSection`).
    fn own_header_line_end(&self, parsed: &Document, mapping: Node<'_>) -> Result<usize> {
        let source = &self.source[..];
        'header: for region in parsed.regions_of(mapping.id) {
            let line = line_start_before(source, region.start as usize);
            let mut cur = parsed.ast.child(&mapping)?;
            while let Some(kv) = cur {
                if self.out_of_region(parsed, kv) && line_start_before(source, parsed.span(&kv).start) == line {
                    continue 'header;
                }
                cur = parsed.ast.next(&kv);
            }
            return Ok(line_end_after(source, region.start as usize));
        }
        Err(err!(ImplicitSection))
    }

    /// Rewrite the EMPTY closed container at `span` (`<dict/>`,
    /// `<array></array>`) into its multi-line form around `body`, one
    /// entry or item already rendered against the child indent.
    fn expand_empty_container(&mut self, span: Span, delims: Delimiters, base: &[u8], body: &[u8]) -> Result<()> {
        let mut out = Vec::new();
        out.extend_from_slice(delims.open);
        out.push(b'\n');
        out.extend_from_slice(base);
        out.extend_from_slice(self.syntax().indent_unit);
        out.extend_from_slice(body);
        out.push(b'\n');
        out.extend_from_slice(base);
        out.extend_from_slice(delims.close);
        self.replace_at_span(span, &out)
    }

    /// Whether `path`'s final segment names a key the document RESOLVES but
    /// no physical entry declares — one supplied by the format's reference
    /// layer (YAML's `<<`), which path navigation does not follow and
    /// therefore reports as `NotFound`.
    fn key_is_inherited(&self, parsed: &Document, path: &[PathSegment<'_>]) -> bool {
        if self.syntax().merge_key.is_none() {
            return false;
        }
        let Some(key) = last_key(path) else { return false };
        let Ok(parent) = parsed.ast.get_val_by_path(&path[..path.len() - 1]) else { return false };
        if !is_mapping(&parent) {
            return false;
        }
        matches!(parsed.ast.merged_child(&parent, key), Ok(Some(_)))
    }

    /// The current parse, shared: what the engine reads while it edits.
    fn parsed(&self) -> Result<Arc<Document>> {
        self.document.clone().ok_or(err!(NotInitialized))
    }

    /// The parse of `source()`.
    pub fn get_parsed(&self) -> Result<&Document> {
        self.document.as_deref().ok_or(err!(NotInitialized))
    }

    /// Parse `input` as the document to edit. Once only (`MultipleInit`).
    pub fn init(&mut self, input: &[u8]) -> Result<()> {
        if !self.source.is_empty() || self.document.is_some() {
            return Err(err!(MultipleInit));
        }
        self.source.extend_from_slice(input);
        self.document = Some(Arc::new(self.parse_source()?));
        Ok(())
    }

    /// Replace a span with a new span. Atomic: on success the document is
    /// the reparse of the edited source; if the edit produces source that no
    /// longer parses, the source is rolled back and the prior document
    /// stays valid, so a failed edit leaves the editor exactly as it was.
    ///
    /// A line inserted where the line before it continues
    /// (`Syntax.line_continuation`) is written after a blank line, which
    /// ends the continuation; written straight after it, the new line
    /// would be read as the rest of it. Every op that adds a line — an
    /// entry, a dangling or leading comment — inserts it here.
    pub fn replace_at_span(&mut self, span: Span, replacement: &[u8]) -> Result<()> {
        if span.end < span.start || span.end > self.source.len() {
            return Err(err!(InvalidSpan));
        }
        // Snapshot the whole source so a failed reparse can be undone.
        let backup = self.source.clone();
        self.splice_rejected = false;
        let syn = self.syntax();
        let end_continuation: &[u8] =
            if span.is_empty() { continuation_end(&self.source[..span.start], replacement, &syn) } else { b"" };
        self.replace_source(span, replacement)?;
        self.replace_source(Span::new(span.start, span.start), end_continuation)?;
        if let Err(e) = self.reparse() {
            // Restore byte-for-byte. `backup` parsed at `init`, so the only
            // new thing in the buffer was `replacement`: it is what `e` is
            // about.
            self.source = backup;
            self.splice_rejected = true;
            return Err(e);
        }
        Ok(())
    }

    /// Replace the value at `path`. Reference-layer behavior is copy-on-write:
    /// editing a value that is an alias (`b: *x`) replaces the `*x` text with
    /// the new literal (severing only that alias — its anchor and any other
    /// alias are untouched), which falls out of splicing the alias node's own
    /// span. A key supplied only by a `<<` merge is materialized locally,
    /// shadowing the merge. Use `replace_val_at_path_following` to edit
    /// through to a shared anchor instead.
    ///
    /// Two things stand between `replacement` and the old value's slot,
    /// each a fact the format states rather than logic it supplies:
    ///
    ///   * The slot is not a bare literal. plist wraps every value in a
    ///     typed element (`<integer>42</integer>`), so the text is
    ///     RENDERED through the format's `render_value` first.
    ///   * The slot's shape can change. Where the parser recorded the
    ///     entry's separator (`Document::sep_span` — YAML, fig,
    ///     NestedText), everything after the key is rewritten through
    ///     `write_tail`, so a block collection can descend onto the
    ///     following lines where the old value was inline. An item
    ///     reframes the same way from its recorded marker when the format
    ///     renders items.
    ///
    /// **Engine rule**: a SECTION node is refused before the generic splice
    /// runs (`CannotReplaceSection`). In TOML and INI a container's node
    /// span is only its KEY segment inside a `[header]`, so splicing into
    /// it would silently rename the section. A format whose block container
    /// hangs under a bare header (fig) records a zero-width separator
    /// there, and the reframe — which runs first — rewrites the
    /// container's body in place.
    ///
    /// The result is held to what a replace promises (`reads_back`): the
    /// value reads back as `replacement` reads, and nothing else changed,
    /// or `EditReadsBackWrong` and the source as it was.
    pub fn replace_val_at_path(&mut self, path: &[PathSegment<'_>], replacement: &[u8]) -> Result<()> {
        let parsed = self.parsed()?;
        // A path that resolves to nothing is the splice's to report, or
        // an inherited key's insert, which is checked as an insert.
        let node = match parsed.ast.get_val_by_path(path) {
            Ok(n) => n,
            Err(_) => return self.splice_value(path, replacement),
        };
        let guard = self.replace_guard(&parsed, node, path.is_empty(), replacement)?;
        self.splice_value(path, replacement)?;
        self.reads_back(&guard)
    }

    /// `replace_val_at_path` for a string the caller rendered: `rendered`
    /// is its spelling of the string `string`, the printer's. Where the
    /// value replaced is a string too, the new one is written in the old
    /// one's quote style, when that style can hold it — a YAML
    /// `"debug"` replaced by `info` stays double-quoted — and as
    /// `rendered` where it cannot.
    ///
    /// **Renderer** `render_string`, told the string as `value` and the
    /// old one's spelling as `old_value`; it writes nothing for a style
    /// that is the printer's own or cannot hold the string.
    pub fn replace_string_at_path(&mut self, path: &[PathSegment<'_>], rendered: &[u8], string: &[u8]) -> Result<()> {
        let text = self.in_old_style(path, rendered, string)?;
        self.replace_val_at_path(path, &text)
    }

    /// `set` for a string the caller rendered, as `replace_string_at_path`
    /// replaces one: a string that replaces a string keeps the old one's
    /// quote style. A new entry has no old style, and takes `rendered`.
    pub fn set_string(&mut self, path: &[PathSegment<'_>], rendered: &[u8], string: &[u8]) -> Result<()> {
        let text = self.in_old_style(path, rendered, string)?;
        self.set(path, &text)
    }

    /// `string` in the style of the string at `path`, or `rendered` when
    /// there is no string there or the format's `render_string` writes
    /// nothing for it.
    fn in_old_style(&self, path: &[PathSegment<'_>], rendered: &[u8], string: &[u8]) -> Result<Vec<u8>> {
        if !self.has_renderer(Renderer::String) {
            return Ok(rendered.to_vec());
        }
        let parsed = self.parsed()?;
        let Ok(node) = parsed.ast.get_val_by_path(path) else { return Ok(rendered.to_vec()) };
        if !matches!(node.kind, NodeKind::String(_)) {
            return Ok(rendered.to_vec());
        }
        let span = parsed.span(&node);
        let place = place_of_child(&parsed, node.id);
        let mut buf = Vec::new();
        self.render(
            Renderer::String,
            &mut buf,
            RenderRequest {
                value: string,
                old_value: &self.source[span.start..span.end],
                parent_key: place.key,
                parent_tag: place.tag,
                ..RenderRequest::default()
            },
        )?;
        Ok(if buf.is_empty() { rendered.to_vec() } else { buf })
    }

    fn splice_value(&mut self, path: &[PathSegment<'_>], replacement: &[u8]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = match parsed.ast.get_val_by_path(path) {
            Ok(n) => n,
            Err(e) => {
                // An inherited key surfaces as NotFound (path nav doesn't
                // follow the reference layer); copy-on-write it by inserting
                // a local `key: value` entry that shadows what it inherits.
                if e == err!(NotFound) && self.key_is_inherited(&parsed, path) {
                    let key = last_key(path).unwrap();
                    return self.insert_key(&path[..path.len() - 1], key, replacement);
                }
                return Err(e);
            }
        };
        let span = parsed.span(&node);
        let place = place_of_child(&parsed, node.id);
        let rendered = self.rendered_value(replacement, place)?;
        let mut out: Vec<u8> = Vec::new();
        let source = &self.source[..];

        // The document root: the value stands alone. A format that
        // renders tails spells it (NestedText's `>` block); the rest
        // splice it as written.
        if path.is_empty() {
            if !self.has_renderer(Renderer::Tail) {
                return self.replace_at_span(span, &rendered);
            }
            self.write_tail(&mut out, b"", b"", &rendered, place)?;
            self.terminate_line(&mut out, span.end);
            return self.replace_at_span(Span::new(0, span.end), &out);
        }
        // A mapping value whose entry records its separator is REFRAMED:
        // everything after the key is rewritten through `write_tail`, so
        // the value may change shape between inline and block. This runs
        // ahead of the section veto because a format whose block
        // container hangs under a bare header (fig) records a zero-width
        // separator there, and its reframe rewrites the container's body
        // in place.
        if last_key(path).is_some() {
            let kv = parsed.ast.get_node_by_path(path)?;
            if let Some(sep) = parsed.sep_span(&kv) {
                let key_span = parsed.span(&parsed.ast.node(kv_of(&kv).key));
                // An explicit key's separator opens a line of its own
                // (YAML's `? k` over `: v`): the tail is rewritten from
                // there, under that line's indent, so the key keeps its
                // `?` line instead of taking the value onto it.
                let own_line =
                    !self.has_renderer(Renderer::Tail) && find_byte(&source[key_span.end..sep.start], b'\n').is_some();
                let from = if own_line { sep.start } else { key_span.end };
                let indent = self.indent_at(if own_line { sep.start } else { key_span.start });
                self.write_tail(&mut out, &indent, &source[key_span.start..key_span.end], &rendered, place)?;
                // A null value is a zero-width span at the separator.
                let end = span.end.max(sep.end);
                self.terminate_line(&mut out, end);
                return self.replace_at_span(Span::new(from, end), &out);
            }
        }
        // The engine's veto, before anything is spliced — a target whose
        // span is a header/dotted KEY, not a value slot.
        if parsed.is_section(&node) {
            return Err(refuse(SectionOp::Replace));
        }
        // An empty value whose span sits exactly at the key's end was
        // written with NO separator — a bare `.properties` key, `flag`
        // alone on its line — so the value cannot simply take the slot:
        // `flagx` would read back as a longer key. The separator goes in
        // with it, as an insert would write it.
        if span.start == span.end && last_key(path).is_some() {
            let kv = parsed.ast.get_node_by_path(path)?;
            let key_span = parsed.span(&parsed.ast.node(kv_of(&kv).key));
            if span.start == key_span.end {
                if let Some(sep) = self.syntax().kv_sep {
                    out.extend_from_slice(sep);
                    out.extend_from_slice(&rendered);
                    return self.replace_at_span(span, &out);
                }
            }
        }
        // A sequence item is reframed the same way when the format
        // renders items and the parser recorded the item's marker: the
        // marker and value are rewritten together through `write_item`.
        if self.has_renderer(Renderer::Item) && last_is_index(path) {
            if let Some(m) = parsed.marker_span(&node) {
                let indent = self.indent_at(m.start);
                self.write_item(&mut out, &indent, &rendered, place)?;
                self.terminate_line(&mut out, span.end);
                return self.replace_at_span(Span::new(m.start, span.end), &out);
            }
        }
        // A block item spliced in place keeps its marker and what stands
        // between that and the value (a comment, an anchor's line), and
        // a block value's later lines go under the item's content column
        // (`- x: 1` over `  y: two`). At column 0 they would leave the
        // item: `y` read as a key of the mapping around the list. A
        // block scalar's span takes its last newline, which the item
        // gives back (`terminate_line`). Where a line's prefix is
        // structure (figl's `>`), a value's later lines are its own: a
        // `'''` string's text stays as written.
        if last_is_index(path) && parsed.marker_span(&node).is_some() && !self.syntax().structural_indent {
            let indent = self.indent_at(span.start);
            reindent_into(&mut out, &rendered, &indent);
            self.terminate_line(&mut out, span.end);
            return self.replace_at_span(span, &out);
        }
        // An empty value can sit right against a trailing comment, its
        // separating blank before it (dotenv's `A= #c`): the new text
        // keeps one between them, or the comment would read as more of
        // the value (`A= 42#c` is `42#c`).
        if span.is_empty() && !rendered.is_empty() {
            if let Some(c) = self.syntax().comments.trailing {
                if source[span.end..].starts_with(c.open) {
                    out.extend_from_slice(&rendered);
                    out.push(b' ');
                    return self.replace_at_span(span, &out);
                }
            }
        }
        self.replace_at_span(span, &rendered)
    }

    /// A reframe ends at `end`; when that byte sits at a LINE START —
    /// the old value was an implicit empty whose zero-width span the
    /// parser anchored on the following line, an empty document, or the
    /// file's end after its last newline — the rewritten text has
    /// consumed the old line's terminator (or never had one) and
    /// supplies its own. Everywhere else the original newline still
    /// follows the splice.
    fn terminate_line(&self, out: &mut Vec<u8>, end: usize) {
        if at_line_start(&self.source, end) && !out.ends_with(b"\n") {
            out.push(b'\n');
        }
    }

    /// Upsert a mapping value: replace the value at `path`, or — when only
    /// the trailing key is absent — insert it as a fresh `key: value` entry
    /// in the parent mapping.
    ///
    /// The path's last segment MUST name a key — `set` only ever *creates* a
    /// mapping entry, never a sequence item, so a path ending in an index is
    /// rejected with `NotAMapping`. Missing *intermediate* containers are
    /// auto-vivified (`mkdir -p` for config): if the parent mapping doesn't
    /// exist yet, `set` seeds it — and any of ITS missing ancestors, deepest
    /// first — as an empty map, then lands the leaf. Vivification fires when
    /// an intermediate key is genuinely absent (`NotFound`), or when what
    /// stands in its place is an EMPTY node — a null. A segment that resolves
    /// to a non-map SCALAR is a real type error (`NotAMapping`) and is never
    /// clobbered. See `Syntax.empty_map_literal` for what a seed is spelled
    /// as per format.
    ///
    /// Vivify-then-land is atomic as a whole: if the leaf cannot be placed
    /// after a seed has been spliced, the seed is rolled back. Where the
    /// seeds are FLOW containers (every format but YAML) a block-spelled
    /// value cannot land inside one, and is refused with
    /// `BlockValueIntoFlow`.
    ///
    /// Key duality: the replace branch matches the trailing segment
    /// *logically* (against decoded key names), but the insert branch needs
    /// the key as *syntax*, so `set` renders the logical key into the
    /// format's key syntax (`format_insert_key`) when it inserts.
    pub fn set(&mut self, path: &[PathSegment<'_>], value_text: &[u8]) -> Result<()> {
        let Some(last) = last_key(path) else { return Err(err!(NotAMapping)) };
        let replace_err = match self.replace_val_at_path(path, value_text) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        // Every replace error but these means the key IS there and the
        // replace was refused for cause; an insert does not cure that.
        if replace_err != err!(NotFound) && replace_err != err!(NotAMapping) {
            return Err(replace_err);
        }
        let key_text = self.format_insert_key(last)?;
        let parent = &path[..path.len() - 1];
        let insert_err = match self.insert_key(parent, &key_text, value_text) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        // One insert failure is more informative than any replace error can
        // be: the parent was found and IS a mapping, just one that cannot
        // hold this value (`BlockValueIntoFlow`), has no line of its own
        // (`ImplicitSection`), or took the entry and read it wrong
        // (`EditReadsBackWrong`).
        if insert_err == err!(BlockValueIntoFlow) || insert_err == err!(ImplicitSection) || insert_err == err!(EditReadsBackWrong)
        {
            return Err(insert_err);
        }
        // The parent mapping itself is missing: auto-vivify it as an empty
        // map — recursing to seed any missing ancestor deepest-first — then
        // retry the leaf insert into the now-existing parent. `NotAMapping`
        // joins `NotFound` as vivifiable only when what stands in the way is
        // an EMPTY node (a null), not a scalar.
        let vivifiable =
            insert_err == err!(NotFound) || (insert_err == err!(NotAMapping) && self.blocked_by_empty_node(parent));
        // A format with no `empty_map_literal` has no literal spelling for
        // "an empty nested mapping" to seed WITH, so it cannot vivify at
        // all — INI, plist and NestedText.
        let seed = self.syntax().empty_map_literal;
        match seed {
            Some(seed) if path.len() >= 2 && vivifiable => {
                // Vivify-then-insert is TWO splices, so it needs a snapshot
                // of its own.
                let backup = self.source.clone();
                if let Err(seed_err) = self.set(parent, seed) {
                    self.restore_source(&backup)?;
                    return Err(seed_err);
                }
                if let Err(leaf_err) = self.insert_key(parent, &key_text, value_text) {
                    self.restore_source(&backup)?;
                    return Err(leaf_err);
                }
                Ok(())
            }
            _ => Err(if insert_err == err!(ContainerClosesOnItsLine) { insert_err } else { replace_err }),
        }
    }

    /// Append `value_text` to the sequence at `path`, starting the
    /// sequence first when it is not there: `set`'s upsert for a list,
    /// as `set` is `insert_key`'s. A sequence that is there takes the item
    /// exactly as `append_to_seq` would place it.
    ///
    /// An absent key or a null stands in for the list, and is replaced
    /// by `Syntax.empty_seq_literal` through `set` before the item lands. A
    /// scalar or a mapping there is real data, refused as `append_to_seq`
    /// refuses it (`NotASequence`). A format with no empty-sequence spelling
    /// cannot start one, and reports the lookup's own `NotFound`.
    ///
    /// Seed-then-append is atomic as a whole.
    pub fn set_append(&mut self, path: &[PathSegment<'_>], value_text: &[u8]) -> Result<()> {
        if !self.seq_missing(path) {
            return self.append_to_seq(path, value_text);
        }
        let backup = self.source.clone();
        if !self.seed_seq(path)? {
            return self.append_to_seq(path, value_text);
        }
        if let Err(e) = self.append_to_seed(path, value_text) {
            self.restore_source(&backup)?;
            return Err(e);
        }
        Ok(())
    }

    /// Whether the sequence at `path` is yet to exist: the path does not
    /// resolve, or resolves to a null.
    fn seq_missing(&self, path: &[PathSegment<'_>]) -> bool {
        let Ok(parsed) = self.parsed() else { return false };
        match parsed.ast.get_val_by_path(path) {
            Ok(node) => is_null(&node),
            Err(_) => true,
        }
    }

    /// Write `Syntax.empty_seq_literal` at `path` with `set`, returning
    /// whether there was one to write.
    fn seed_seq(&mut self, path: &[PathSegment<'_>]) -> Result<bool> {
        let Some(seed) = self.syntax().empty_seq_literal else { return Ok(false) };
        if last_key(path).is_none() {
            return Ok(false);
        }
        self.set(path, seed)?;
        Ok(true)
    }

    /// `append_to_seq` into a sequence `seed_seq` started. ZON's `.{}` is
    /// its empty tuple and its empty struct at once, and parses as the
    /// struct; the seed is a list by declaration, so an empty flow
    /// mapping in its place takes the item as a list's first.
    fn append_to_seed(&mut self, path: &[PathSegment<'_>], value_text: &[u8]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        if matches!(node.kind, NodeKind::Mapping(None)) && self.is_flow_node(&parsed, node) {
            return self.insert_flow_item(&parsed, node, parsed.span(&node), false, value_text);
        }
        self.append_to_seq(path, value_text)
    }

    /// Whether what blocks navigation to `path` is an EMPTY node — a null —
    /// rather than real data. Walks back from `path` to the deepest prefix
    /// that still resolves and reports whether that node is a null; an
    /// unresolvable path with a null root (an empty document) counts too.
    fn blocked_by_empty_node(&self, path: &[PathSegment<'_>]) -> bool {
        let Ok(parsed) = self.parsed() else { return false };
        let mut len = path.len();
        while len > 0 {
            if let Ok(node) = parsed.ast.get_val_by_path(&path[..len]) {
                return is_null(&node);
            }
            len -= 1;
        }
        is_null(&parsed.ast.node(parsed.ast.root))
    }

    /// Restore the source to `backup` and reparse, undoing a compound
    /// (multi-splice) op. `backup` is a snapshot of source that parsed.
    fn restore_source(&mut self, backup: &[u8]) -> Result<()> {
        self.source.clear();
        self.source.extend_from_slice(backup);
        self.reparse()
    }

    /// Spell a logical mapping key as this format writes a key — for
    /// the `set` insert branch, and for a caller of `insert_key` that has
    /// a key NAME rather than key syntax. A language that declares
    /// `render_key_name` spells it (JSON quotes it, ZON writes `.name`,
    /// TOML quotes what is not bare); otherwise the name is the syntax.
    pub fn format_insert_key(&self, key: &[u8]) -> Result<Vec<u8>> {
        if !self.has_renderer(Renderer::KeyName) {
            return Ok(key.to_vec());
        }
        let mut out = Vec::new();
        self.render(Renderer::KeyName, &mut out, RenderRequest { key, ..RenderRequest::default() })?;
        Ok(out)
    }

    /// Like `replace_val_at_path`, but follow into the reference layer: when
    /// the target value is an alias, edit the *anchored node* (the shared
    /// source), so every alias to that anchor reflects the change. The
    /// `&name` (and any tag) prefix is preserved — only the anchored value's
    /// bytes are replaced. A non-alias target behaves exactly like
    /// `replace_val_at_path`.
    pub fn replace_val_at_path_following(&mut self, path: &[PathSegment<'_>], replacement: &[u8]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = match parsed.ast.get_val_by_path(path) {
            Ok(n) => n,
            Err(_) => return self.replace_val_at_path(path, replacement),
        };
        if matches!(node.kind, NodeKind::Alias(_)) {
            let target = parsed.ast.node(parsed.ast.resolve_alias(&node)?);
            // Held to `reads_back` at the anchored node, as a replace
            // is at its own: the aliases read as they did, by name.
            let guard = self.replace_guard(&parsed, target, target.id == parsed.ast.root, replacement)?;
            let rendered = self.rendered_value(replacement, place_of_child(&parsed, target.id))?;
            let span = self.value_span_without_props(&parsed, target);
            self.replace_at_span(span, &rendered)?;
            return self.reads_back(&guard);
        }
        self.replace_val_at_path(path, replacement)
    }

    /// The span of `node`'s value bytes, excluding any leading anchor or
    /// tag property (the node's stored span starts at the property), so
    /// that editing an anchored value keeps its anchor intact.
    fn value_span_without_props(&self, parsed: &Document, node: Node<'_>) -> Span {
        let source = &self.source[..];
        let full = parsed.span(&node);
        let mut start = full.start;
        if let Some(a) = parsed.anchor_span(&node) {
            start = start.max(a.end);
        }
        if let Some(t) = parsed.tag_span(&node) {
            start = start.max(t.end);
        }
        while start < full.end && (source[start] == b' ' || source[start] == b'\t') {
            start += 1;
        }
        Span::new(start, full.end)
    }

    /// Rename the key at `path`, leaving its value untouched. `replacement`
    /// is key SYNTAX, spliced as given; a section's name is rewritten
    /// wherever the format spells it, and a format whose key syntax has
    /// more than one form spells the new key through `render_key`.
    ///
    /// A path that names no key — a sequence item, or the root — is
    /// refused with `NotAKey`. A rename that gives the mapping a second
    /// entry of one name is rolled back with `DuplicateKey`, counted as
    /// `insert_key` counts.
    ///
    /// A key written more than once and read as one entry is renamed at
    /// every spelling (`spelled_again`), so the old name no longer reads.
    pub fn replace_key_at_path(&mut self, path: &[PathSegment<'_>], replacement: &[u8]) -> Result<()> {
        if path.is_empty() || last_is_index(path) {
            return Err(err!(NotAKey));
        }
        let parent_path = &path[..path.len() - 1];
        let before = self.parsed()?;
        let repeated_before = match before.ast.get_val_by_path(parent_path) {
            Ok(parent) => repeated_key_count(&before.ast, parent),
            Err(_) => 0,
        };
        let backup = self.source.clone();
        let mut spellings = 0usize;
        while self.spelled_again(path) {
            let prior = self.source.clone();
            if let Err(e) = self.splice_key_at_path(path, replacement) {
                if spellings > 0 {
                    self.restore_source(&backup)?;
                }
                return Err(e);
            }
            // A rename to the name it had finds the same spelling again.
            if prior == self.source {
                break;
            }
            spellings += 1;
        }
        if let Err(e) = self.splice_key_at_path(path, replacement) {
            if spellings > 0 {
                self.restore_source(&backup)?;
            }
            return Err(e);
        }
        let after = self.parsed()?;
        let Ok(parent_after) = after.ast.get_val_by_path(parent_path) else { return Ok(()) };
        if repeated_key_count(&after.ast, parent_after) > repeated_before {
            self.restore_source(&backup)?;
            return Err(err!(DuplicateKey));
        }
        Ok(())
    }

    /// The splice behind `replace_key_at_path`, once the path is known to
    /// name a key.
    fn splice_key_at_path(&mut self, path: &[PathSegment<'_>], replacement: &[u8]) -> Result<()> {
        let parsed = self.parsed()?;
        // A section node's name is written wherever the format spells
        // it — every `[a.b]` sharing the prefix, every dotted line, every
        // reopening — and only the first has a key node. Splicing that
        // one span would leave the rest behind and SPLIT the container,
        // so a section renames through its recorded mentions.
        if !path.is_empty() {
            if let Ok(value) = parsed.ast.get_val_by_path(path) {
                let mentions = parsed.mentions_of(value.id);
                if parsed.is_section(&value) && names_in(mentions.clone()) > 0 {
                    return self.rewrite_mentions(mentions, replacement);
                }
            }
        }
        let node = parsed.ast.get_key_by_path(path)?;
        let span = parsed.span(&node);
        // **Renderer** `render_key`, told the indent, the new key and the
        // old one — spells the new key in the form the old one's syntax
        // allows (NestedText's plain `key:` versus multiline `: key`).
        // Without it the key is spliced verbatim.
        if self.has_renderer(Renderer::Key) {
            let source = &self.source[..];
            let line_start = line_start_before(source, span.start);
            let indent = self.indent_at(first_non_space(source, line_start));
            let place = place_of_child(&parsed, node.id);
            let mut out = Vec::new();
            self.render(
                Renderer::Key,
                &mut out,
                RenderRequest {
                    indent: &indent,
                    key: replacement,
                    old_key: &source[span.start..span.end],
                    parent_key: place.key,
                    parent_tag: place.tag,
                    ..RenderRequest::default()
                },
            )?;
            return self.replace_at_span(span, &out);
        }
        // A quoted key may stand against its separator (YAML's
        // `{ "k":v }`), but a plain one runs through a `:` that no blank
        // follows: `renamed:v` is one scalar, a key with no value. The
        // separator then takes the blank `kv_sep` writes after it, so the
        // entry reads as the rename asked.
        if let Some(sep_end) = self.joined_separator(span, replacement) {
            let mut text = replacement.to_vec();
            text.extend_from_slice(&self.source[span.end..sep_end]);
            text.push(b' ');
            return self.replace_at_span(Span::new(span.start, sep_end), &text);
        }
        self.replace_at_span(span, replacement)
    }

    /// Where the separator ends when `new_key`, spliced over the quoted
    /// key at `key_span`, would run into it, or `None`. That is when
    /// `kv_sep` is a separator and a blank (`: `), the old key closes
    /// with a quote and the separator stands directly after it with the
    /// value directly after that, and the new key closes with no quote.
    fn joined_separator(&self, key_span: Span, new_key: &[u8]) -> Option<usize> {
        let kv_sep = self.syntax().kv_sep?;
        if kv_sep.len() < 2 || kv_sep[kv_sep.len() - 1] != b' ' {
            return None;
        }
        let sep = &kv_sep[..kv_sep.len() - 1];
        if find_any(sep, b" \t").is_some() {
            return None;
        }
        if new_key.is_empty() || is_quote(new_key[new_key.len() - 1]) {
            return None;
        }
        let source = &self.source[..];
        if key_span.end == key_span.start || !is_quote(source[key_span.end - 1]) {
            return None;
        }
        if !source[key_span.end..].starts_with(sep) {
            return None;
        }
        let sep_end = key_span.end + sep.len();
        if sep_end >= source.len() {
            return None;
        }
        match source[sep_end] {
            b' ' | b'\t' | b'\r' | b'\n' => None,
            _ => Some(sep_end),
        }
    }

    /// `replace_key_at_path` for a caller that has the new key's NAME rather
    /// than its syntax, as `insert_named_key` is to `insert_key`: the name is
    /// spelled by `format_insert_key` and the rename goes on as before. A
    /// ZON key's span is the field name after its `.`, so the dot that
    /// spelling leads with is left where it already stands.
    ///
    /// A name another entry of the mapping already holds is refused with
    /// `DuplicateKey` before anything is spliced. Renaming a key to its own
    /// name is not a repeat.
    pub fn replace_named_key(&mut self, path: &[PathSegment<'_>], name: &[u8]) -> Result<()> {
        if path.is_empty() || last_is_index(path) {
            return Err(err!(NotAKey));
        }
        let parsed = self.parsed()?;
        if let Ok(parent) = parsed.ast.get_val_by_path(&path[..path.len() - 1]) {
            // `replace_key_at_path` reports a missing key in its own terms.
            if let Ok(own) = parsed.ast.get_node_by_path(path) {
                if holds_name(&parsed.ast, parent, name, Some(own.id)) {
                    return Err(err!(DuplicateKey));
                }
            }
        }
        let rendered = self.format_insert_key(name)?;
        // A key's span leaves out the sigil it carries (`key_sigil`,
        // ZON's `.`), which stays where it already stands.
        let sigil = self.syntax().key_sigil;
        let text: &[u8] = match sigil {
            Some(s) if !rendered.is_empty() && rendered[0] == s => &rendered[1..],
            _ => &rendered,
        };
        self.replace_key_at_path(path, text)
    }

    // ========
    // COMMENTS
    // ========
    //
    // Comments are trivia — they live OUTSIDE every AST node span — so these
    // ops reuse the same splice + reparse machinery as the structural edits:
    // compute a byte position from a node's span, splice the comment text,
    // reparse. The reparse is the safety net (`replace_at_span` rolls back if
    // the result no longer parses).

    /// The line-comment marker for the dialect this document is being read
    /// as, or `None` when that dialect forbids comments (strict JSON) — in
    /// which case the comment ops return `CommentsUnsupported`.
    fn line_comment_marker(&self) -> Option<CommentDelimiter> {
        self.syntax().comments.line
    }

    /// The own-line comment marker as a bare PREFIX, or `None` for a format
    /// with no line comment or a paired one. The dangling and comment-out
    /// ops prefix existing lines with a marker and strip one off; a pair
    /// (`<!-- -->`) cannot be written that way, so they refuse it with
    /// `CommentsUnsupported`.
    fn prefix_comment_marker(&self) -> Option<&'static [u8]> {
        let d = self.line_comment_marker()?;
        if d.close.is_empty() { Some(d.open) } else { None }
    }

    /// The marker for a same-line TRAILING comment specifically, or `None`
    /// for a format that has no such syntax (INI, NestedText — where a
    /// `;`/`#` after a value is literal value text).
    fn trailing_comment_marker(&self) -> Option<CommentDelimiter> {
        self.syntax().comments.trailing
    }

    /// The key/value separator the GENERIC entry-insert helpers splice, or
    /// `UnsupportedShape` for a format that declares none (its entries are
    /// spelled by `render_entry`, which replaces every path that asks).
    fn kv_sep(&self) -> Result<&'static [u8]> {
        self.syntax().kv_sep.ok_or(err!(UnsupportedShape))
    }

    /// Whether a comment op at `path` has no line of its own to work with:
    /// the node is an element or entry of a flow collection that shares its
    /// parent's line (`members = ["a", "b"]`, `members: [a, b]`,
    /// `nested = { k = "v" }`).
    ///
    /// The test is what SHARES the line, not the node's column: the parent
    /// is flow, and either the parent's opener (`key = [`) or a preceding
    /// element sits on the node's own line. An item of a MULTI-LINE flow
    /// collection (one element per line) owns its line.
    ///
    /// The document root is excluded. Infallible: an unresolvable path
    /// answers "anchored" so the caller's own lookup raises the real
    /// `NotFound`.
    fn comments_unanchored(&self, parsed: &Document, path: &[PathSegment<'_>]) -> bool {
        if path.is_empty() {
            return false;
        }
        let source = &self.source[..];
        let Ok(parent) = parsed.ast.get_val_by_path(&path[..path.len() - 1]) else { return false };
        if parent.id == parsed.ast.root {
            return false;
        }
        let parent_span = parsed.span(&parent);
        if !self.is_flow_node(parsed, parent) {
            return false;
        }
        let Ok(node) = parsed.ast.get_node_by_path(path) else { return false };
        let line = line_start_before(source, parsed.span(&node).start);
        // The opener is on this line, so what precedes the node is the
        // parent's own `key = [` — the line, and the block above it, are
        // the parent's.
        if line_start_before(source, parent_span.start) == line {
            return true;
        }
        // Otherwise the node is unanchored only if a preceding element ends
        // on the same line.
        let mut prev_end: Option<usize> = None;
        let Ok(Some(mut cur)) = parsed.ast.child(&parent) else { return false };
        while cur.id != node.id {
            prev_end = Some(parsed.span(&cur).end);
            match parsed.ast.next(&cur) {
                Some(n) => cur = n,
                None => return false,
            }
        }
        let Some(end) = prev_end else { return false };
        line_start_before(source, end.saturating_sub(1)) == line
    }

    /// The line a leading-comment op anchors on: the node's own line start,
    /// which for a mapping entry is its key's line and for a sequence item
    /// is its marker's line. See `marker_start`.
    fn leading_comment_line_start(&self, parsed: &Document, node: Node<'_>) -> usize {
        line_start_before(&self.source, marker_start(parsed, node))
    }

    /// Add an own-line comment ABOVE the node at `path` — the key's line for a
    /// mapping entry, else the node's own line — matched to that line's
    /// indentation. It lands at the BOTTOM of any existing leading comment
    /// block. `text` may be multi-line; each line becomes its own comment
    /// line. `CommentsUnsupported` for a dialect without comment syntax,
    /// `InvalidComment` for text the delimiter forbids, and
    /// `CommentsUnanchored` for a node that shares its parent's line inside
    /// a flow collection.
    pub fn add_leading_comment(&mut self, path: &[PathSegment<'_>], text: &[u8]) -> Result<()> {
        let marker = self.line_comment_marker().ok_or(err!(CommentsUnsupported))?;
        if let Some(f) = marker.forbidden {
            if find(text, f).is_some() {
                return Err(err!(InvalidComment));
            }
        }
        let parsed = self.parsed()?;
        if self.comments_unanchored(&parsed, path) {
            return Err(err!(CommentsUnanchored));
        }
        let node = parsed.ast.get_node_by_path(path)?;
        let span = parsed.span(&node);
        let source = &self.source[..];
        let line_start = self.leading_comment_line_start(&parsed, node);
        // A format whose line prefix is STRUCTURAL (fig's `>` marker run)
        // copies the raw prefix; everywhere else it is pure whitespace.
        let indent = if self.syntax().structural_indent {
            &source[line_start..span.start]
        } else {
            &source[line_start..first_non_space(source, line_start)]
        };
        let mut buf = Vec::new();
        render_line_comments(&mut buf, indent, marker, text);
        self.replace_at_span(Span::new(line_start, line_start), &buf)
    }

    /// The byte window `[start, line_end)` on the entry-at-`path`'s line where a
    /// same-line trailing comment lives, shared by the set/delete/get trailing
    /// ops. For a scalar or flow value the window runs from just past the value
    /// to that line's newline. For a BLOCK-style mapping/sequence value the
    /// trailing comment rides the key's line, so the window starts just past
    /// the key. A scalar that ends with its last line's newline — a YAML block
    /// scalar — takes it on its first line, the header's; so does a scalar
    /// over several lines in a format whose opener line is its comment's
    /// (`Syntax.opener_line_comment`), and a flow list or object over several
    /// lines whose opener line holds no element.
    ///
    /// `None` when the node has no such window at all: a flow element or
    /// entry sharing its parent's line (see `comments_unanchored`).
    fn trailing_comment_window(&self, path: &[PathSegment<'_>]) -> Result<Option<(usize, usize)>> {
        let parsed = self.parsed()?;
        if self.comments_unanchored(&parsed, path) {
            return Ok(None);
        }
        let val = parsed.ast.get_val_by_path(path)?;
        let val_span = parsed.span(&val);
        let source = &self.source[..];
        let syn = self.syntax();
        // A block collection with no closing token of its own has its
        // trailing comment on the key's line; one that closes itself
        // (`</dict>`) takes it after the close, like any scalar.
        let is_block_collection = match val.kind {
            NodeKind::Mapping(_) | NodeKind::Sequence(_) => {
                !self.is_flow_node(&parsed, val) && syn.closed_containers.is_none()
            }
            _ => false,
        };
        // A value whose span takes its last line's newline with it (a
        // YAML `|`/`>` block scalar) leaves nothing of that line: its
        // comment rides the line it opens on, after the header.
        let takes_line_end = val_span.end > val_span.start && source[val_span.end - 1] == b'\n';
        let is_scalar = !matches!(val.kind, NodeKind::Mapping(_) | NodeKind::Sequence(_));
        let spans_lines = find_byte(&source[val_span.start..val_span.end], b'\n').is_some();
        let on_opener_line = syn.opener_line_comment
            && spans_lines
            && (is_scalar || (!is_block_collection && self.flow_opener_line_is_empty(val_span.start)));
        let start = if is_block_collection {
            parsed.span(&parsed.ast.get_key_by_path(path)?).end
        } else if takes_line_end || on_opener_line {
            val_span.start
        } else {
            val_span.end
        };
        let mut line_end = find_byte_from(source, start, b'\n').unwrap_or(source.len());
        // The line's content ends before a CRLF's CR, which stays with the
        // LF: a comment appended after it would leave a bare CR in the
        // value.
        if line_end > start && source[line_end - 1] == b'\r' {
            line_end -= 1;
        }
        Ok(Some((start, line_end)))
    }

    /// Whether the flow container opening at `open` (its `[`/`{`) has
    /// nothing after the opener on its line but blanks and a comment —
    /// figl's `xs = [ # note`.
    fn flow_opener_line_is_empty(&self, open: usize) -> bool {
        let source = &self.source[..];
        let mut i = open + 1;
        while i < source.len() && (source[i] == b' ' || source[i] == b'\t') {
            i += 1;
        }
        if i >= source.len() || source[i] == b'\n' || source[i] == b'\r' {
            return true;
        }
        let Some(marker) = self.trailing_comment_marker() else { return false };
        source[i..].starts_with(marker.open)
    }

    /// Set the same-line trailing comment on the value at `path`: replace an
    /// existing trailing comment on that line, or append one if there is none.
    /// `text` must be a single line. `CommentsUnsupported` for a dialect
    /// without trailing comment syntax, `MultilineComment` if `text`
    /// contains a newline, `InvalidComment` for text the delimiter forbids,
    /// `CommentsUnanchored` for a node that shares its parent's line inside
    /// a flow collection, and `CommentInsideValue` for a value that runs to
    /// the end of its line (see `replace_keeping_values`).
    pub fn set_trailing_comment(&mut self, path: &[PathSegment<'_>], text: &[u8]) -> Result<()> {
        let marker = self.trailing_comment_marker().ok_or(err!(CommentsUnsupported))?;
        if find_byte(text, b'\n').is_some() {
            return Err(err!(MultilineComment));
        }
        if let Some(f) = marker.forbidden {
            if find(text, f).is_some() {
                return Err(err!(InvalidComment));
            }
        }
        let (start, line_end) = self.trailing_comment_window(path)?.ok_or(err!(CommentsUnanchored))?;
        let source = &self.source[..];

        // If a comment marker already follows on this line, splice from it
        // (replace); otherwise splice from the line's end (append).
        let mut cut = match find(&source[start..line_end], marker.open) {
            Some(rel) => start + rel,
            None => line_end,
        };
        // Drop the run of spaces/tabs just before the splice so the rebuilt
        // " <marker> text" controls its own single leading space.
        while cut > start && (source[cut - 1] == b' ' || source[cut - 1] == b'\t') {
            cut -= 1;
        }
        let mut buf = b" ".to_vec();
        render_comment(&mut buf, marker, text);
        self.replace_keeping_values(Span::new(cut, line_end), &buf)
    }

    /// The trailing-comment splice, held to what a comment edit promises:
    /// the document's values are the same after it as before. A splice
    /// that changed any value is rolled back byte-for-byte with
    /// `CommentInsideValue`.
    fn replace_keeping_values(&mut self, span: Span, replacement: &[u8]) -> Result<()> {
        let backup = self.source.clone();
        let before_doc = self.document.clone();
        // A splice that doesn't parse rolls itself back (`replace_at_span`).
        self.replace_at_span(span, replacement)?;
        let before = match before_doc {
            Some(d) => d,
            None => match self.lang.parse(&backup, self.format) {
                Ok(d) => Arc::new(d),
                Err(_) => {
                    self.restore_source(&backup)?;
                    return Err(err!(CommentInsideValue));
                }
            },
        };
        let after = self.parsed()?;
        if !nodes_equal_deep(&before, before.ast.root, &after, after.ast.root) {
            self.restore_source(&backup)?;
            return Err(err!(CommentInsideValue));
        }
        Ok(())
    }

    /// Remove the run of own-line comments immediately ABOVE the node at
    /// `path` — its owned leading block. A no-op when the node has none —
    /// including a node that shares its parent's line inside a flow
    /// collection. `CommentsUnsupported` for a dialect without comment
    /// syntax.
    pub fn delete_leading_comments(&mut self, path: &[PathSegment<'_>]) -> Result<()> {
        self.line_comment_marker().ok_or(err!(CommentsUnsupported))?;
        let parsed = self.parsed()?;
        if self.comments_unanchored(&parsed, path) {
            return Ok(());
        }
        let node = parsed.ast.get_node_by_path(path)?;
        let line_start = self.leading_comment_line_start(&parsed, node);
        let block_start = comment_block_start(&self.source, line_start, self.syntax().comments);
        if block_start == line_start {
            return Ok(()); // nothing above to remove
        }
        self.replace_at_span(Span::new(block_start, line_start), b"")
    }

    /// Remove the same-line trailing comment on the value at `path`, if any.
    /// A no-op when there is none. `CommentsUnsupported` for a dialect
    /// without trailing comment syntax.
    pub fn delete_trailing_comment(&mut self, path: &[PathSegment<'_>]) -> Result<()> {
        let marker = self.trailing_comment_marker().ok_or(err!(CommentsUnsupported))?;
        let Some((start, line_end)) = self.trailing_comment_window(path)? else { return Ok(()) };
        let source = &self.source[..];
        let Some(rel) = find(&source[start..line_end], marker.open) else { return Ok(()) };
        let mut cut = start + rel;
        // Take the whitespace separating the value from the comment with it.
        while cut > start && (source[cut - 1] == b' ' || source[cut - 1] == b'\t') {
            cut -= 1;
        }
        self.replace_at_span(Span::new(cut, line_end), b"")
    }

    /// Read back the own-line comment block immediately ABOVE the node at
    /// `path` — the same owned block `delete_leading_comments` removes — with
    /// each line's indentation and marker (and one following space)
    /// stripped, lines rejoined by '\n'. `None` when there is no block
    /// above the node (distinct from a present-but-empty comment — a bare
    /// `#` — which yields ""), which includes a node that shares its
    /// parent's line inside a flow collection.
    pub fn get_leading_comment(&self, path: &[PathSegment<'_>]) -> Result<Option<Vec<u8>>> {
        let marker = self.line_comment_marker().ok_or(err!(CommentsUnsupported))?;
        let parsed = self.parsed()?;
        if self.comments_unanchored(&parsed, path) {
            return Ok(None);
        }
        let node = parsed.ast.get_node_by_path(path)?;
        let source = &self.source[..];
        let line_start = self.leading_comment_line_start(&parsed, node);
        let block_start = comment_block_start(source, line_start, self.syntax().comments);
        if block_start == line_start {
            return Ok(None); // no block above
        }
        let mut out = Vec::new();
        let mut first = true;
        for raw in source[block_start..line_start].split(|&c| c == b'\n') {
            let line = trim_end(raw, b"\r");
            let trimmed = trim_start(line, b" \t");
            if trimmed.is_empty() {
                continue; // skip a trailing empty split slice
            }
            if !first {
                out.push(b'\n');
            }
            first = false;
            out.extend_from_slice(strip_line_comment_marker(trimmed, marker));
        }
        Ok(Some(out))
    }

    /// Read back the same-line trailing comment on the value at `path`, its
    /// marker (and one following space) stripped. `None` when there is no
    /// trailing comment (distinct from a present-but-empty bare `#`, which
    /// yields ""), which includes a node that shares its parent's line
    /// inside a flow collection.
    pub fn get_trailing_comment(&self, path: &[PathSegment<'_>]) -> Result<Option<Vec<u8>>> {
        let marker = self.trailing_comment_marker().ok_or(err!(CommentsUnsupported))?;
        let Some((start, line_end)) = self.trailing_comment_window(path)? else { return Ok(None) };
        let source = &self.source[..];
        let Some(rel) = find(&source[start..line_end], marker.open) else { return Ok(None) };
        let after = trim_end(&source[start + rel..line_end], b" \t\r");
        Ok(Some(strip_line_comment_marker(after, marker).to_vec()))
    }

    // ── The dangling anchor ─────────────────────────────────────────────
    //
    // A container's THIRD comment anchor (spec § 3.4): the run of own-line
    // comments at the END of its body, after its last entry, which the AST
    // side-table carries as `NodeComments.dangling` and every printer
    // emits. Where the run is, in source: from just past the last line of
    // the container's body, forward over each contiguous own-line comment
    // whose line carries at least the body's child indent — spec § 3.4's own
    // rule, read off the source rather than the parse, which is what makes
    // it the exact inverse of what `add_dangling_comment` writes.

    /// Resolve the dangling anchor of the container at `path` (the root for
    /// an empty path). `UnsupportedShape` for a scalar — a dangling run is
    /// a container's — and for a flow container with no room for an
    /// own-line comment: one written on a single line or with no children.
    ///
    /// The body ends after the last child written on the container's OWN
    /// lines. A child that is itself a SECTION is skipped. A block container
    /// with no such child (an empty `[table]`) anchors just past its header
    /// line(s).
    fn dangling_anchor(&self, parsed: &Document, path: &[PathSegment<'_>]) -> Result<DanglingAnchor> {
        let node = parsed.ast.get_val_by_path(path)?;
        if !matches!(node.kind, NodeKind::Mapping(_) | NodeKind::Sequence(_)) {
            return Err(err!(UnsupportedShape));
        }
        let source = &self.source[..];
        let span = parsed.span(&node);
        let flow = self.is_flow_node(parsed, node);
        let structural = self.syntax().structural_indent;

        let mut last: Option<Node<'_>> = None;
        let mut maybe = parsed.ast.child(&node)?;
        while let Some(c) = maybe {
            let val = match c.kind {
                NodeKind::KeyValue(kv) => parsed.ast.node(kv.value),
                _ => c,
            };
            if !parsed.is_section(&val) {
                last = Some(c);
            }
            maybe = parsed.ast.next(&c);
        }
        if let Some(c) = last {
            let c_span = parsed.span(&c);
            let line_start = line_start_before(source, c_span.start);
            let at = line_end_after(source, c_span.end.saturating_sub(1));
            // A flow container whose closing delimiter shares the last
            // child's line has no line for a run to sit on.
            if flow && at >= span.end {
                return Err(err!(UnsupportedShape));
            }
            return Ok(DanglingAnchor {
                at,
                indent: source[line_start..comment_column(source, line_start, structural)].to_vec(),
            });
        }
        // Nothing on its own lines. A flow container has no body line to
        // anchor on at all; a section container still has its header
        // line(s) recorded; anything else falls back to its own line.
        if flow {
            return Err(err!(UnsupportedShape)); // `{}` — no body to end
        }
        let regs = parsed.regions_of(node.id);
        let anchor_at = match regs.last() {
            Some(r) => r.end as usize,
            None => line_end_after(source, span.end.saturating_sub(1)),
        };
        let own_line = line_start_before(source, regs.last().map_or(span.start, |r| r.start as usize));
        Ok(DanglingAnchor { at: anchor_at, indent: source[own_line..comment_column(source, own_line, structural)].to_vec() })
    }

    /// Byte offset just past the dangling run starting at `anchor.at`: the
    /// contiguous own-line `marker` comments carrying at least
    /// `anchor.indent`. Equal to `anchor.at` when there is no run.
    fn dangling_run_end(&self, anchor: &DanglingAnchor, marker: &[u8]) -> usize {
        let source = &self.source[..];
        let structural = self.syntax().structural_indent;
        let mut pos = anchor.at;
        while pos < source.len() {
            let line_end = line_end_after(source, pos);
            if !source[pos..line_end].starts_with(&anchor.indent) {
                break;
            }
            let col = comment_column(source, pos, structural);
            if !source[col..line_end].starts_with(marker) {
                break;
            }
            pos = line_end;
        }
        pos
    }

    /// Read back the dangling comment run at the end of the container at
    /// `path`'s body (the root for an empty path), with each line's prefix
    /// and marker (and one following space) stripped, lines rejoined by
    /// '\n'. `None` when there is no run. `CommentsUnsupported` for a
    /// dialect without a bare-prefix comment syntax; `UnsupportedShape`
    /// for a scalar, or a flow container with no line for a run to sit on.
    pub fn get_dangling_comment(&self, path: &[PathSegment<'_>]) -> Result<Option<Vec<u8>>> {
        let marker = self.prefix_comment_marker().ok_or(err!(CommentsUnsupported))?;
        let parsed = self.parsed()?;
        let anchor = self.dangling_anchor(&parsed, path)?;
        let end = self.dangling_run_end(&anchor, marker);
        if end == anchor.at {
            return Ok(None);
        }
        let source = &self.source[..];
        let structural = self.syntax().structural_indent;
        let mut out = Vec::new();
        let mut pos = anchor.at;
        let mut first = true;
        while pos < end {
            let line_end = line_end_after(source, pos);
            let body = trim_end(&source[comment_column(source, pos, structural)..line_end], b"\r\n");
            if !first {
                out.push(b'\n');
            }
            first = false;
            out.extend_from_slice(strip_line_comment_marker(body, CommentDelimiter::open(marker)));
            pos = line_end;
        }
        Ok(Some(out))
    }

    /// Add own-line comment line(s) at the END of the container at `path`'s
    /// body, at the body's child depth — the dangling twin of
    /// `add_leading_comment`, landing at the bottom of any run already there.
    pub fn add_dangling_comment(&mut self, path: &[PathSegment<'_>], text: &[u8]) -> Result<()> {
        let marker = self.prefix_comment_marker().ok_or(err!(CommentsUnsupported))?;
        let parsed = self.parsed()?;
        let anchor = self.dangling_anchor(&parsed, path)?;
        let at = self.dangling_run_end(&anchor, marker);
        let mut buf = Vec::new();
        // A final line with no newline of its own would otherwise weld the
        // first comment onto it.
        if !at_line_start(&self.source, at) {
            buf.push(b'\n');
        }
        render_line_comments(&mut buf, &anchor.indent, CommentDelimiter::open(marker), text);
        self.replace_at_span(Span::new(at, at), &buf)
    }

    /// Remove the whole dangling run at the end of the container at
    /// `path`'s body. A no-op when there is none.
    pub fn delete_dangling_comments(&mut self, path: &[PathSegment<'_>]) -> Result<()> {
        let marker = self.prefix_comment_marker().ok_or(err!(CommentsUnsupported))?;
        let parsed = self.parsed()?;
        let anchor = self.dangling_anchor(&parsed, path)?;
        let end = self.dangling_run_end(&anchor, marker);
        if end == anchor.at {
            return Ok(());
        }
        self.replace_at_span(Span::new(anchor.at, end), b"")
    }

    // ── Comment out, and back ───────────────────────────────────────────

    /// Whether the node at `span`, whose own line begins at `line_start`,
    /// has those lines TO ITSELF — the precondition both halves of the
    /// comment-out pair rest on, since a line marker hides everything after
    /// it to end of line.
    ///
    /// Before the node, only indentation and the format's own line
    /// introducers may stand (a `- `/`* ` item dash, a fig `>` marker run,
    /// a `+` continuation). After it, only whitespace, one separating
    /// comma, and a trailing comment.
    fn owns_its_lines(&self, line_start: usize, span: Span, marker: &[u8]) -> bool {
        let source = &self.source[..];
        let syn = self.syntax();
        for &c in &source[line_start..span.start] {
            if c == b' ' || c == b'\t' {
                continue;
            }
            if syn.seq_item_marker.contains(&c) {
                continue;
            }
            if syn.line_prefix_marks.contains(&c) {
                continue;
            }
            return false;
        }
        let line_end = line_end_after(source, span.end.saturating_sub(1));
        let sep = syn.flow_entry_sep;
        let mut i = span.end;
        let mut comma = false;
        while i < line_end {
            match source[i] {
                b' ' | b'\t' | b'\r' | b'\n' => {}
                _ => {
                    if source[i..line_end].starts_with(sep) {
                        if comma {
                            return false;
                        }
                        comma = true;
                        i += sep.len();
                        continue;
                    }
                    return source[i..line_end].starts_with(marker);
                }
            }
            i += 1;
        }
        true
    }

    /// Turn the node at `path` into a comment run: every line of its source
    /// span gains the line marker at that line's own indentation (past a
    /// structural prefix, so a nested fig line keeps its depth). The node's
    /// own leading comment block stays above it, untouched.
    ///
    /// `CommentsUnsupported` for a dialect without a bare-prefix comment
    /// syntax. `UnsupportedShape` for the root (an empty path);
    /// `CommentsUnanchored` for one that does not have its lines to itself
    /// (`owns_its_lines`). A node whose value is a SECTION is refused with
    /// `CannotDeleteSection`.
    pub fn comment_out(&mut self, path: &[PathSegment<'_>]) -> Result<()> {
        let marker = self.prefix_comment_marker().ok_or(err!(CommentsUnsupported))?;
        if path.is_empty() {
            return Err(err!(UnsupportedShape));
        }
        let parsed = self.parsed()?;
        let node = parsed.ast.get_node_by_path(path)?;
        let val = match node.kind {
            NodeKind::KeyValue(kv) => parsed.ast.node(kv.value),
            _ => node,
        };
        if parsed.is_section(&val) {
            return Err(refuse(SectionOp::Delete));
        }
        let source = &self.source[..];
        let span = parsed.span(&node);
        let start = self.leading_comment_line_start(&parsed, node);
        let end = line_end_after(source, span.end.saturating_sub(1));
        if !self.owns_its_lines(start, span, marker) {
            return Err(err!(CommentsUnanchored));
        }
        let structural = self.syntax().structural_indent;

        let mut buf = Vec::new();
        let mut pos = start;
        while pos < end {
            let line_end = line_end_after(source, pos);
            let col = comment_column(source, pos, structural);
            buf.extend_from_slice(&source[pos..col]);
            buf.extend_from_slice(marker);
            // No trailing space on an otherwise empty line — the same rule
            // `render_line_comments` follows.
            let rest = &source[col..line_end];
            if !rest.is_empty() && rest[0] != b'\n' && rest[0] != b'\r' {
                buf.push(b' ');
            }
            buf.extend_from_slice(rest);
            pos = line_end;
        }
        self.replace_at_span(Span::new(start, end), &buf)
    }

    /// Bring `line_count` lines of the LEADING comment block above the node
    /// at `path`, starting at `first_line` (0-based within that block),
    /// back as entries: strip the marker and one following space from each,
    /// then reparse. If the result does not parse, or parses to a document
    /// whose other nodes changed, the splice is rolled back and the call
    /// fails (the parse error, or `CommentNotAnEntry`).
    ///
    /// `NotFound` when the block has fewer than `first_line + line_count`
    /// lines; a `line_count` of 0 is a no-op. `CommentsUnsupported`,
    /// `UnsupportedShape` for the root, and `CommentsUnanchored` as the
    /// comment ops answer them.
    pub fn uncomment_leading(&mut self, path: &[PathSegment<'_>], first_line: usize, line_count: usize) -> Result<()> {
        let marker = self.prefix_comment_marker().ok_or(err!(CommentsUnsupported))?;
        if path.is_empty() {
            return Err(err!(UnsupportedShape));
        }
        let parsed = self.parsed()?;
        let node = parsed.ast.get_node_by_path(path)?;
        let parent_path = &path[..path.len() - 1];
        let span = parsed.span(&node);
        let line_start = self.leading_comment_line_start(&parsed, node);
        if !self.owns_its_lines(line_start, span, marker) {
            return Err(err!(CommentsUnanchored));
        }
        let block_start = comment_block_start(&self.source, line_start, self.syntax().comments);
        self.uncomment_block(block_start, line_start, first_line, line_count, parent_path)
    }

    /// The dangling twin of `uncomment_leading`: bring `line_count` lines of
    /// the DANGLING run at the end of the container at `container_path`'s
    /// body, starting at `first_line`, back as entries of that container.
    pub fn uncomment_dangling(&mut self, container_path: &[PathSegment<'_>], first_line: usize, line_count: usize) -> Result<()> {
        let marker = self.prefix_comment_marker().ok_or(err!(CommentsUnsupported))?;
        let parsed = self.parsed()?;
        let anchor = self.dangling_anchor(&parsed, container_path)?;
        let end = self.dangling_run_end(&anchor, marker);
        self.uncomment_block(anchor.at, end, first_line, line_count, container_path)
    }

    /// The shared body of the two uncomment ops: strip the marker (and one
    /// following space) from lines `[first_line, first_line + line_count)`
    /// of the comment block `[block_start, block_end)`, splice the whole
    /// block back in one edit, and keep the result only if it parses AND
    /// adds nodes to nothing but the container at `container_path`.
    fn uncomment_block(
        &mut self,
        block_start: usize,
        block_end: usize,
        first_line: usize,
        line_count: usize,
        container_path: &[PathSegment<'_>],
    ) -> Result<()> {
        if line_count == 0 {
            return Ok(());
        }
        let marker = self.prefix_comment_marker().ok_or(err!(CommentsUnsupported))?;
        let structural = self.syntax().structural_indent;
        let source = &self.source[..];

        let mut out = Vec::new();
        let mut pos = block_start;
        let mut index = 0usize;
        let mut stripped = 0usize;
        while pos < block_end {
            let line_end = line_end_after(source, pos).min(block_end);
            if index >= first_line && index < first_line.saturating_add(line_count) {
                let col = comment_column(source, pos, structural);
                if !source[col..line_end].starts_with(marker) {
                    return Err(err!(CommentNotAnEntry));
                }
                out.extend_from_slice(&source[pos..col]);
                let mut rest = &source[col + marker.len()..line_end];
                if !rest.is_empty() && rest[0] == b' ' {
                    rest = &rest[1..];
                }
                out.extend_from_slice(rest);
                stripped += 1;
            } else {
                out.extend_from_slice(&source[pos..line_end]);
            }
            index += 1;
            pos = line_end;
        }
        if stripped != line_count {
            return Err(err!(NotFound)); // fewer lines than asked for
        }

        // Snapshot before the splice: the check below compares the parse of
        // these bytes against the parse of the edited ones.
        let before = self.take_before();
        // A splice that doesn't parse rolls itself back (`replace_at_span`).
        self.replace_at_span(Span::new(block_start, block_end), &out)?;
        let Ok(b) = self.parse_of_backup(&before) else {
            self.restore_source(&before.bytes)?;
            return Err(err!(CommentNotAnEntry));
        };
        let after = self.parsed()?;
        if !changed_only_at(&b, &after, container_path, Change::Added) {
            self.restore_source(&before.bytes)?;
            return Err(err!(CommentNotAnEntry));
        }
        Ok(())
    }

    // ===============
    // INSERT / DELETE
    // ===============
    //
    // These ops never reserialize the document: each computes a byte span +
    // replacement text and reuses `replace_at_span` (splice + reparse).
    // Inserts splice at a zero-length span; deletes splice an empty
    // replacement. `value_text`/`key_text` arrive already serialized
    // (single-line scalars, or multi-line block text indented from column
    // 0); the editor only re-frames indentation and newline/comma context
    // for the splice site.

    /// Insert `key_text: value_text` into the mapping at `path` (empty path =
    /// root). Appends after the mapping's last entry for block mappings, or
    /// inside the braces for flow `{}`. If `path` resolves to a `null` value
    /// (a bare `key:`), promotes it to a one-entry nested mapping.
    ///
    /// The flow/block decision is `is_flow_node`'s; a block entry is spelled
    /// through `write_entry`, a flow entry with `kv_sep` or the separator
    /// its siblings use. A table with no header of its own (TOML's `a` in
    /// `a.b = 1`) has no lines to append to: its entry is a key path in the
    /// lines or braces that hold it (`insert_headerless`).
    ///
    /// **Engine rule**: an insert never gives a mapping a second entry of
    /// a name it already holds: the mapping's repeated names are counted
    /// before and after, and an insert that adds one is rolled back with
    /// `DuplicateKey`.
    ///
    /// The result is held to what an insert promises (`reads_back`): the
    /// mapping gained one entry, reading as `key_text` and `value_text`
    /// read, and nothing else changed, or `EditReadsBackWrong` and the
    /// source as it was.
    pub fn insert_key(&mut self, path: &[PathSegment<'_>], key_text: &[u8], value_text: &[u8]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        let guard = self.add_guard(&parsed, node, None, Some(key_text), value_text)?;
        self.splice_entry(path, key_text, value_text)?;
        self.reads_back(&guard)
    }

    fn splice_entry(&mut self, path: &[PathSegment<'_>], key_text: &[u8], value_text: &[u8]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        let span = parsed.span(&node);
        let repeated_before = repeated_key_count(&parsed.ast, node);
        // Snapshot for the duplicate rollback below; the splice's own
        // rollback covers a reparse failure.
        let backup = self.source.clone();
        let inserted = match node.kind {
            NodeKind::Mapping(first) => {
                if self.is_flow_node(&parsed, node) {
                    self.insert_flow_map_entry(&parsed, node, span, first.is_some(), key_text, value_text)
                } else if let Some(owner) = self.headerless_holder(&parsed, node) {
                    self.insert_headerless(&parsed, path, node, owner, key_text, value_text)
                } else {
                    self.insert_block_key(&parsed, node, key_text, value_text)
                }
            }
            NodeKind::Null => self.promote_null_to_mapping(
                span,
                node.id == parsed.ast.root,
                key_text,
                value_text,
                place_of(&parsed, node),
            ),
            _ => return Err(err!(NotAMapping)),
        };
        if let Err(e) = inserted {
            // A parser that refuses a repeated key (TOML's) fails the
            // reparse, and through the contract its reason is only a
            // message. The splice has rolled back; if the key syntax is
            // how this format spells a name the mapping already holds,
            // that is the refusal to report.
            let Ok(now) = self.parsed() else { return Err(e) };
            let Ok(held) = now.ast.get_val_by_path(path) else { return Err(e) };
            if self.spells_held_name(&now.ast, held, key_text)? {
                return Err(err!(DuplicateKey));
            }
            return Err(e);
        }
        let after = self.parsed()?;
        let Ok(node_after) = after.ast.get_val_by_path(path) else { return Ok(()) };
        if repeated_key_count(&after.ast, node_after) > repeated_before {
            self.restore_source(&backup)?;
            return Err(err!(DuplicateKey));
        }
        Ok(())
    }

    /// Whether `key_text` is how this format spells the key of a direct
    /// entry of `mapping`.
    fn spells_held_name(&self, ast: &AST, mapping: Node<'_>, key_text: &[u8]) -> Result<bool> {
        let NodeKind::Mapping(mut entry) = mapping.kind else { return Ok(false) };
        let want = trim(key_text, b" \t");
        while let Some(id) = entry {
            let n = ast.node(id);
            entry = n.next_sibling;
            let NodeKind::KeyValue(kv) = n.kind else { continue };
            let NodeKind::String(name) = ast.node(kv.key).kind else { continue };
            if name == want {
                return Ok(true);
            }
            let spelled = self.format_insert_key(name)?;
            if spelled == want {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether the entry at `path` is one spelling of a key written
    /// again later. dotenv, INI, `.properties` and plist read a key
    /// written more than once as one entry: the parser keeps the first
    /// spelling's place, as the entry's span, and the last spelling's
    /// value, which so lies past it. A splice at the span takes one
    /// spelling, and the key reads on with another's value, so
    /// `delete_key` and `replace_key_at_path` take this spelling and ask
    /// again, until the entry is the last spelling and holds its own
    /// value.
    fn spelled_again(&self, path: &[PathSegment<'_>]) -> bool {
        if last_key(path).is_none() {
            return false;
        }
        let Ok(parsed) = self.parsed() else { return false };
        let Ok(kv) = parsed.ast.get_node_by_path(path) else { return false };
        value_past_entry(&parsed, kv)
    }

    /// `insert_key` for a caller that has the key's NAME rather than its
    /// syntax: the name is spelled as this format spells a key
    /// (`format_insert_key`) and inserted. A name the mapping at `path`
    /// already holds is refused with `DuplicateKey` before anything is
    /// spliced. `set` is the op that replaces an existing key's value.
    pub fn insert_named_key(&mut self, path: &[PathSegment<'_>], name: &[u8], value_text: &[u8]) -> Result<()> {
        let parsed = self.parsed()?;
        // `insert_key` reports a bad parent path in its own terms.
        if let Ok(parent) = parsed.ast.get_val_by_path(path) {
            if holds_name(&parsed.ast, parent, name, None) {
                return Err(err!(DuplicateKey));
            }
        }
        let rendered = self.format_insert_key(name)?;
        self.insert_key(path, &rendered, value_text)
    }

    /// Delete the mapping entry at `path` (which must name a key). For a
    /// *block* mapping, removes the entry's full line(s) plus any owned
    /// leading comment block, leaving no blank gap. For a *flow* (`{…}`)
    /// mapping routes through a comma-aware entry splice instead.
    ///
    /// A key written more than once and read as one entry goes at every
    /// spelling (`spelled_again`), in one edit: refused whole if any
    /// spelling's splice is.
    ///
    /// Whatever the arm, the result is held to what a delete promises
    /// (`keep_others`): a document that reads back with that entry gone
    /// and nothing else changed, or `DeleteChangedOtherValues` and the
    /// source as it was.
    pub fn delete_key(&mut self, path: &[PathSegment<'_>]) -> Result<()> {
        let removed = self.entry_index(path);
        let before = self.take_before();
        let mut spellings = 0usize;
        while self.spelled_again(path) {
            let len = self.source.len();
            if let Err(e) = self.splice_out_entry(path) {
                if spellings > 0 {
                    self.restore_source(&before.bytes)?;
                }
                return Err(e);
            }
            if self.source.len() >= len {
                break; // the guard has the last word
            }
            spellings += 1;
        }
        if let Err(e) = self.splice_out_entry(path) {
            if spellings > 0 {
                self.restore_source(&before.bytes)?;
            }
            return Err(e);
        }
        if let Some(at) = removed {
            self.keep_others(&before, &path[..path.len() - 1], at)?;
        }
        Ok(())
    }

    /// Where the entry at `path` stands among its container's children,
    /// for `keep_others` — or `None` when `path` names no physical entry of
    /// a container the path also reaches.
    fn entry_index(&self, path: &[PathSegment<'_>]) -> Option<usize> {
        if path.is_empty() {
            return None;
        }
        let parsed = self.parsed().ok()?;
        let node = parsed.ast.get_node_by_path(path).ok()?;
        let parent = parsed.ast.get_val_by_path(&path[..path.len() - 1]).ok()?;
        let mut i = 0usize;
        let mut sib = parsed.ast.child(&parent).ok()?;
        while let Some(c) = sib {
            if c.id == node.id {
                return Some(i);
            }
            i += 1;
            sib = parsed.ast.next(&c);
        }
        None
    }

    /// The delete guard. A delete is a splice chosen by reading the
    /// source around an entry, and a wrong reading still parses. So
    /// the two parses are compared (`changed_only_at`): the container at
    /// `container_path` must have lost its child at `removed` and nothing
    /// else, every other node must be as it was, and otherwise the splice
    /// is rolled back byte-for-byte with `DeleteChangedOtherValues`.
    fn keep_others(&mut self, before: &Before, container_path: &[PathSegment<'_>], removed: usize) -> Result<()> {
        let Ok(b) = self.parse_of_backup(before) else {
            self.restore_source(&before.bytes)?;
            return Err(err!(DeleteChangedOtherValues));
        };
        let after = self.parsed()?;
        if !changed_only_at(&b, &after, container_path, Change::Removed(removed)) {
            self.restore_source(&before.bytes)?;
            return Err(err!(DeleteChangedOtherValues));
        }
        Ok(())
    }

    /// The source as it stands, and its parse, for a check after an edit
    /// to compare with and roll back to.
    fn take_before(&self) -> Before {
        Before { bytes: self.source.clone(), doc: self.document.clone() }
    }

    /// The parse of `before`'s bytes: the document taken with them, which
    /// was parsed from those same bytes, or a parse of them.
    fn parse_of_backup(&self, before: &Before) -> Result<Arc<Document>> {
        if let Some(d) = &before.doc {
            return Ok(d.clone());
        }
        Ok(Arc::new(self.lang.parse(&before.bytes, self.format)?))
    }

    /// A `Guard` for `write` to `container`.
    fn guard_at(&self, parsed: &Document, container: Node<'_>, write: Write) -> Result<Guard> {
        let route = route_to(parsed, container.id)?;
        Ok(Guard { before: self.take_before(), route, write })
    }

    /// The guard for replacing the value `node` with `replacement`: the
    /// node reads back as the replacement reads (`reading_of`). `root`
    /// says the node is the document, whose replacement is a document.
    fn replace_guard(&self, parsed: &Document, node: Node<'_>, root: bool, replacement: &[u8]) -> Result<Guard> {
        let place = place_of_child(parsed, node.id);
        let in_ = if root {
            WriteSlot::Root
        } else if let Some(p) = parsed.ast.parent_of(node.id) {
            if matches!(parsed.ast.kind_of(p), NodeKind::Sequence(_)) {
                self.item_slot(parsed, parsed.ast.node(p))
            } else {
                WriteSlot::Entry
            }
        } else {
            WriteSlot::Entry
        };
        let rendered = self.rendered_value(replacement, place)?;
        let want = self.reading_of(None, &rendered, place, in_)?;
        match slot_of(parsed, node.id) {
            None => self.guard_at(parsed, node, Write::Whole(want)),
            Some(slot) => self.guard_at(parsed, parsed.ast.node(slot.container), Write::Replaced { at: slot.at, want }),
        }
    }

    /// The guard for adding one child to `container`: an item at `at`,
    /// or an entry (`at` `None`) reading as `key_text` and `value_text`
    /// read.
    fn add_guard(
        &self,
        parsed: &Document,
        container: Node<'_>,
        at: Option<usize>,
        key_text: Option<&[u8]>,
        value_text: &[u8],
    ) -> Result<Guard> {
        let place = place_of(parsed, container);
        let in_ = if is_sequence(&container) { self.item_slot(parsed, container) } else { WriteSlot::Entry };
        let rendered = self.rendered_value(value_text, place)?;
        let want = self.reading_of(key_text, &rendered, place, in_)?;
        self.guard_at(parsed, container, Write::Added { at, want })
    }

    /// The `WriteSlot` of an item of the sequence `seq`. A format whose
    /// line prefix is structure (figl) writes an item's later lines as
    /// its own, not under `write_item`'s indent, so it has no block item
    /// to read one in.
    fn item_slot(&self, parsed: &Document, seq: Node<'_>) -> WriteSlot {
        if self.syntax().structural_indent {
            return WriteSlot::FlowItem;
        }
        if self.is_flow_node(parsed, seq) { WriteSlot::FlowItem } else { WriteSlot::BlockItem }
    }

    /// How `rendered` reads in this format, or `None` when that is not
    /// known. It is read in an otherwise empty document, in the slot the
    /// write put it in — one entry under `key_text` (or `k`, when only
    /// the value is wanted), the one item of a root block list — through
    /// the renderers that spelled it there; and on its own, as a document,
    /// which is all there is for the root and for an item of a flow list.
    /// A reading is only known when every one of these that reads as one
    /// value reads as the same value. `None` leaves the value unchecked and
    /// the rest of the document checked still.
    fn reading_of(&self, key_text: Option<&[u8]>, rendered: &[u8], place: Place<'_>, in_: WriteSlot) -> Result<Option<Reading>> {
        let mut found: Option<Reading> = None;
        for slot in [ReadSlot::Item, ReadSlot::Entry, ReadSlot::Alone] {
            let wanted = match slot {
                ReadSlot::Item => in_ == WriteSlot::BlockItem,
                ReadSlot::Entry => in_ == WriteSlot::Entry,
                ReadSlot::Alone => true,
            };
            if !wanted {
                continue;
            }
            if let Some(c) = self.reading_in(slot, key_text, rendered, place)? {
                match &found {
                    Some(f) => {
                        let cv = c.value();
                        if !reads_as(&f.value(), &cv.doc, cv.id) {
                            return Ok(None);
                        }
                    }
                    None => found = Some(c),
                }
            }
        }
        Ok(found)
    }

    /// `reading_of` in one slot: a document of one `Item`, of one `Entry`,
    /// or of the text `Alone`; `None` when it does not read as that.
    fn reading_in(&self, slot: ReadSlot, key_text: Option<&[u8]>, rendered: &[u8], place: Place<'_>) -> Result<Option<Reading>> {
        let mut text = Vec::new();
        match slot {
            ReadSlot::Item => self.write_item(&mut text, b"", rendered, place)?,
            ReadSlot::Entry => self.write_entry(&mut text, b"", key_text.unwrap_or(b"k"), rendered, place)?,
            ReadSlot::Alone => text.extend_from_slice(rendered),
        }
        if slot != ReadSlot::Alone {
            text.push(b'\n');
        }
        let Ok(doc) = self.lang.parse(&text, self.format) else { return Ok(None) };
        let read: Option<(NodeId, bool)> = {
            let top = doc.ast.node(doc.ast.root);
            let only: Option<Node<'_>> = if is_mapping(&top) || is_sequence(&top) {
                match doc.ast.child(&top) {
                    Ok(Some(child)) => {
                        if doc.ast.next(&child).is_none() {
                            Some(child)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };
            match slot {
                ReadSlot::Item => match only {
                    Some(o) if is_sequence(&top) => Some((o.id, false)),
                    _ => None,
                },
                ReadSlot::Entry => match only {
                    Some(o) if is_mapping(&top) && is_keyvalue(&o) => {
                        if key_text.is_some() {
                            Some((o.id, true))
                        } else {
                            Some((kv_of(&o).value, false))
                        }
                    }
                    _ => None,
                },
                // A root container with nothing of its own around its one
                // child — a TOML or `.properties` document of one entry, a
                // bar list's open root of one item — is the format reading
                // the text as an entry or an item of the root, not as the
                // value: no reading.
                ReadSlot::Alone => match only {
                    Some(o) if only_blank_around(&text, doc.span(&o)) => None,
                    _ => Some((doc.ast.root, false)),
                },
            }
        };
        Ok(read.map(|(id, entry)| Reading { doc: Arc::new(doc), id, entry }))
    }

    /// The write guard. A write is a splice chosen by reading the source
    /// around its slot, and a wrong reading still parses. So the two
    /// parses are compared (`written_only_at`): the container on
    /// `guard.route` must have changed only as `guard.write` says, every
    /// other node must be as it was, and otherwise the splice is rolled
    /// back byte-for-byte with `EditReadsBackWrong`.
    fn reads_back(&mut self, guard: &Guard) -> Result<()> {
        let Ok(b) = self.parse_of_backup(&guard.before) else {
            self.restore_source(&guard.before.bytes)?;
            return Err(err!(EditReadsBackWrong));
        };
        let after = self.parsed()?;
        if !written_only_at(&b, b.ast.root, &after, after.ast.root, &guard.route, &guard.write) {
            self.restore_source(&guard.before.bytes)?;
            return Err(err!(EditReadsBackWrong));
        }
        Ok(())
    }

    fn splice_out_entry(&mut self, path: &[PathSegment<'_>]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = match parsed.ast.get_node_by_path(path) {
            Ok(n) => n,
            Err(e) => {
                // A key with no physical entry of its own — inherited through
                // the format's reference layer — has no line to delete, and
                // there is no syntax to un-inherit it. Refuse explicitly.
                if e == err!(NotFound) && self.key_is_inherited(&parsed, path) {
                    return Err(err!(MergeOnlyKey));
                }
                return Err(e);
            }
        };
        let NodeKind::KeyValue(kv) = node.kind else { return Err(err!(NotAMapping)) };
        let span = parsed.span(&node);
        let syn = self.syntax();
        // The engine's veto, before anything is spliced: an entry whose
        // value is a SECTION node, assembled from lines elsewhere in the
        // file. `delete_container` is the op for it.
        if parsed.is_section(&parsed.ast.node(kv.value)) {
            return Err(refuse(SectionOp::Delete));
        }
        // A flow (`{...}`) mapping stores its entries comma-separated, not
        // one-per-line, so every flow-mapping entry goes through
        // `remove_flow_item` — the same comma-aware splice a flow
        // *sequence* delete uses — which drops exactly one adjoining
        // separator and always leaves the enclosing braces intact.
        let parent = parsed.ast.get_val_by_path(&path[..path.len() - 1])?;
        if let Some(pair) = self.flow_pair_span(&parsed, parent, node) {
            let mut empty = syn.flow_map_open.to_vec();
            empty.extend_from_slice(syn.flow_map_close);
            return self.replace_at_span(pair, &empty);
        }
        if is_mapping(&parent) && self.is_flow_node(&parsed, parent) {
            // Find the entry's immediate predecessor: `remove_flow_item`
            // drops the *following* comma for the first entry and the
            // *preceding* comma for any later one.
            let mut item = present(parsed.ast.child(&parent)?)?;
            let mut prev: Option<Node<'_>> = None;
            while item.id != node.id {
                prev = Some(item);
                item = parsed.ast.next(&item).ok_or(err!(NotFound))?;
            }
            // A key span that EXCLUDES the format's key sigil — ZON's
            // leading `.` — is backed up over so the splice carries `.name`
            // as a unit rather than stranding a bare `.`.
            let source = &self.source[..];
            let entry_start = match syn.key_sigil {
                Some(sigil) if span.start > 0 && source[span.start - 1] == sigil => span.start - 1,
                _ => span.start,
            };
            return self.remove_flow_entry(Span::new(entry_start, span.end), prev.is_none());
        }
        // An entry of a table made by dotted keys inside a flow mapping
        // (TOML's `b` in `it = { a.b = 1 }`) is that flow mapping's
        // entry `a.b = 1`, not a line.
        if is_mapping(&parent) {
            if let Some(owner) = self.dotted_in_flow(&parsed, parent) {
                let entry = dotted_entry_span(&parsed, owner.top, node);
                return self.remove_flow_entry(entry, entry.start <= flow_first_entry_start(&parsed, owner.flow));
            }
        }
        let source = &self.source[..];
        let line_start = line_start_before(source, span.start);
        // A block entry's lines are its own in every compiled format, but
        // a runtime one may put two on a line (a minified stylesheet's
        // `a{} b{}`). Such an entry is deleted by its span, with the
        // spaces that part it from whichever neighbour stays on its line.
        let mut prev_end: Option<usize> = None;
        let mut next_start: Option<usize> = None;
        let mut sib = parsed.ast.child(&parent)?;
        while let Some(c) = sib {
            if c.id == node.id {
                if let Some(n) = parsed.ast.next(&c) {
                    next_start = Some(marker_start(&parsed, n));
                }
                break;
            }
            prev_end = Some(parsed.span(&c).end);
            sib = parsed.ast.next(&c);
        }
        let shares_start = prev_end.is_some_and(|e| line_start_before(source, e.saturating_sub(1)) == line_start);
        let shares_end = next_start
            .is_some_and(|s| line_start_before(source, s) == line_start_before(source, span.end.saturating_sub(1)));
        if shares_start || shares_end {
            let mut s = span.start;
            let mut e = span.end;
            if shares_end {
                while e < source.len() && (source[e] == b' ' || source[e] == b'\t') {
                    e += 1;
                }
            } else {
                while s > 0 && (source[s - 1] == b' ' || source[s - 1] == b'\t') {
                    s -= 1;
                }
            }
            return self.replace_at_span(Span::new(s, e), b"");
        }
        if prev_end.is_none() {
            if let Some(shared) = self.shared_line_delete(&parsed, parent, span.start, span.end, next_start) {
                return self.replace_at_span(shared, b"");
            }
        }
        let del_start = comment_block_start(source, line_start, syn.comments);
        let del_end = line_end_after(source, span.end.saturating_sub(1));
        self.replace_at_span(Span::new(del_start, del_end), b"")
    }

    /// The span of a flow pair whose one entry is `entry`: a mapping
    /// with no braces of its own, an item of a flow sequence (YAML's
    /// `a: b` in `[a: b, c]`, or `? c` from its `?`), which its entry's
    /// going leaves an empty mapping. `None` for any other entry.
    fn flow_pair_span(&self, parsed: &Document, parent: Node<'_>, entry: Node<'_>) -> Option<Span> {
        if !is_mapping(&parent) || self.is_flow_node(parsed, parent) {
            return None;
        }
        let span = parsed.span(&parent);
        if span.start != parsed.span(&entry).start || span.end != parsed.span(&entry).end {
            return None;
        }
        let seq = parsed.ast.node(parsed.ast.parent_of(parent.id)?);
        if !is_sequence(&seq) || parsed.item_sep(&seq).is_some() || !self.is_flow_node(parsed, seq) {
            return None;
        }
        let source = &self.source[..];
        let mut s = span.start;
        while s > 0 && is_flow_item_ws(source[s - 1]) {
            s -= 1;
        }
        Some(Span::new(if s > 0 && source[s - 1] == b'?' { s - 1 } else { span.start }, span.end))
    }

    /// Delete the entry of a flow mapping that runs over `entry`, and
    /// one separator beside it (`remove_flow_item`); `is_first` says
    /// whether no entry stands before it.
    ///
    /// When the entry begins its own physical line, any owned leading
    /// comment block above it goes too. A packed entry is not
    /// first-on-line, so this is skipped.
    fn remove_flow_entry(&mut self, entry: Span, is_first: bool) -> Result<()> {
        let source = &self.source[..];
        let syn = self.syntax();
        let line_start = line_start_before(source, entry.start);
        let on_own_line = first_non_space(source, line_start) == entry.start;
        let cbs = if on_own_line { comment_block_start(source, line_start, syn.comments) } else { line_start };
        let del_start = if cbs < line_start { cbs } else { entry.start };
        let len = source.len();
        self.remove_flow_item(Span::new(del_start, entry.end), is_first, syn.flow_entry_sep, Span::new(0, len))
    }

    /// Whether the mapping `node` is a table made by dotted keys inside
    /// a flow mapping — TOML's `a` in `it = { a.b = 1 }` — and which.
    /// `None` for any other node.
    fn dotted_in_flow<'a>(&self, parsed: &'a Document, node: Node<'a>) -> Option<DottedInFlow<'a>> {
        let mut cur = node;
        loop {
            if !is_mapping(&cur) || !parsed.is_section(&cur) {
                return None;
            }
            let mentions = parsed.mentions_of(cur.id);
            if MentionIterator::count(&mentions) == 0 {
                return None;
            }
            for m in mentions {
                if m.kind != MentionKind::Entry {
                    return None;
                }
            }
            let kv_id = parsed.ast.parent_of(cur.id)?;
            if !matches!(parsed.ast.kind_of(kv_id), NodeKind::KeyValue(_)) {
                return None;
            }
            let up = parsed.ast.node(parsed.ast.parent_of(kv_id)?);
            if is_mapping(&up) && self.is_flow_node(parsed, up) {
                return Some(DottedInFlow { flow: up, top: cur });
            }
            cur = up;
        }
    }

    /// Where the last entry of a flow mapping that writes its child
    /// `kv` ends: `kv`'s span, or the end of the last entry under it
    /// when it is a table made by dotted keys, whose own span is only
    /// its name (`a` in `{ c = 2, a.b = 1 }`).
    fn flow_entry_end(&self, parsed: &Document, kv: Node<'_>) -> Result<usize> {
        if let Some(value) = dotted_value(parsed, kv) {
            if let Some(leaf) = self.last_written_leaf(parsed, value, true)? {
                return Ok(parsed.span(&leaf).end);
            }
        }
        Ok(parsed.span(&kv).end)
    }

    /// Append the span of every entry of the flow mapping `owner.flow`
    /// that writes the dotted table `node` to `out`, in the tree's
    /// order rather than the source's.
    fn dotted_entry_spans(&self, parsed: &Document, owner: DottedInFlow<'_>, node: Node<'_>, out: &mut Vec<Span>) -> Result<()> {
        let mut cur = parsed.ast.child(&node)?;
        while let Some(kv) = cur {
            cur = parsed.ast.next(&kv);
            let NodeKind::KeyValue(e) = kv.kind else { continue };
            let value = parsed.ast.node(e.value);
            if is_mapping(&value) && parsed.is_section(&value) {
                self.dotted_entry_spans(parsed, owner, value, out)?;
            } else {
                out.push(dotted_entry_span(parsed, owner.top, kv));
            }
        }
        Ok(())
    }

    /// Append `value_text` as a new item to the sequence at `path`.
    ///
    /// A flow sequence takes the separator-aware splice after its last item;
    /// a block item is spelled through `write_item` after the first item's
    /// line prefix.
    ///
    /// The result is held to what an insert promises (`reads_back`): the
    /// sequence gained one item, last, reading as `value_text` reads, and
    /// nothing else changed, or `EditReadsBackWrong` and the source as it
    /// was. `prepend_to_seq` and `insert_item` are held to the same.
    pub fn append_to_seq(&mut self, path: &[PathSegment<'_>], value_text: &[u8]) -> Result<()> {
        self.add_item(path, usize::MAX, value_text, ItemOp::Append)
    }

    fn splice_append(&mut self, path: &[PathSegment<'_>], value_text: &[u8]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        let NodeKind::Sequence(first) = node.kind else { return Err(err!(NotASequence)) };
        let span = parsed.span(&node);
        if self.is_flow_node(&parsed, node) {
            return self.insert_flow_item(&parsed, node, span, first.is_some(), value_text);
        }
        // A non-flow TOML sequence is an array-of-tables; use
        // `append_container_to_seq` for those.
        let syn = self.syntax();
        if !syn.block_seq_editable {
            return Err(err!(NotAnInlineArray));
        }
        let Some(last) = parsed.ast.last_child(&node)? else { return self.expand_empty_seq(&parsed, node, value_text) };
        let first_item = present(parsed.ast.child(&node)?)?;
        let source = &self.source[..];
        let insert_at = line_end_after(source, parsed.span(&last).end.saturating_sub(1));
        if let Some(closed) = syn.closed_containers {
            if closes_on_last_line(source, parsed.span(&node), parsed.span(&last), closed.seq.close, insert_at) {
                return Err(err!(ContainerClosesOnItsLine));
            }
        }
        self.insert_seq_line(insert_at, marker_start(&parsed, first_item), value_text, place_of(&parsed, node))
    }

    /// The first item into an EMPTY block sequence: only a format whose
    /// containers close themselves has a spelling for one (`<array/>`,
    /// expanded); any other has no line to splice after.
    fn expand_empty_seq(&mut self, parsed: &Document, node: Node<'_>, value_text: &[u8]) -> Result<()> {
        let syn = self.syntax();
        let closed = syn.closed_containers.ok_or(err!(NotASequence))?;
        let span = parsed.span(&node);
        let base = self.indent_at(span.start);
        let mut child = base.clone();
        child.extend_from_slice(syn.indent_unit);
        let place = place_of(parsed, node);
        let rendered = self.rendered_value(value_text, place)?;
        let mut body = Vec::new();
        self.write_item(&mut body, &child, &rendered, place)?;
        self.expand_empty_container(span, closed.seq, &base, &body)
    }

    /// Insert `value_text` before the first item of the sequence at `path`.
    /// The twin of `append_to_seq`, on the same terms; `insert_item` at 0.
    pub fn prepend_to_seq(&mut self, path: &[PathSegment<'_>], value_text: &[u8]) -> Result<()> {
        self.add_item(path, 0, value_text, ItemOp::Prepend)
    }

    fn splice_prepend(&mut self, path: &[PathSegment<'_>], value_text: &[u8]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        if !is_sequence(&node) {
            return Err(err!(NotASequence));
        }
        let span = parsed.span(&node);
        if self.is_flow_node(&parsed, node) {
            let first = parsed.ast.child(&node)?;
            return self.insert_flow_item_before(&parsed, node, span, first, value_text, false);
        }
        if !self.syntax().block_seq_editable {
            return Err(err!(NotAnInlineArray));
        }
        let Some(first_item) = parsed.ast.child(&node)? else { return self.expand_empty_seq(&parsed, node, value_text) };
        let first_start = marker_start(&parsed, first_item);
        let at = line_start_before(&self.source, first_start);
        self.insert_seq_line(at, first_start, value_text, place_of(&parsed, node))
    }

    /// Insert `value_text` into the sequence at `path` so that it is the
    /// item at `index` afterwards: before the item now there, or after
    /// the last one when `index` is the length. `usize::MAX` is the "end"
    /// sentinel (`[-]`/`[$]`) and appends. An index past the length is
    /// `NotFound`, and nothing is written.
    ///
    /// 0 is `prepend_to_seq` and the length is `append_to_seq`, exactly.
    /// Between them, a flow item goes in before the item at `index`, its
    /// separator laid out as the sequence is (`write_joint`); a block item
    /// goes on a line of its own above the item at `index` and the
    /// comment block that item owns, which stays with it.
    pub fn insert_item(&mut self, path: &[PathSegment<'_>], index: usize, value_text: &[u8]) -> Result<()> {
        self.add_item(path, index, value_text, ItemOp::Insert)
    }

    /// An item insert by `op`, at `index`, held to `reads_back`. A path
    /// that names no sequence is the splice's to report.
    fn add_item(&mut self, path: &[PathSegment<'_>], index: usize, value_text: &[u8], op: ItemOp) -> Result<()> {
        let parsed = self.parsed()?;
        let guard = match parsed.ast.get_val_by_path(path) {
            Ok(n) if is_sequence(&n) => {
                let at = index.min(seq_len(&parsed, n)?);
                Some(self.add_guard(&parsed, n, Some(at), None, value_text)?)
            }
            _ => None,
        };
        match op {
            ItemOp::Append => self.splice_append(path, value_text)?,
            ItemOp::Prepend => self.splice_prepend(path, value_text)?,
            ItemOp::Insert => self.splice_item(path, index, value_text)?,
        }
        if let Some(g) = &guard {
            self.reads_back(g)?;
        }
        Ok(())
    }

    fn splice_item(&mut self, path: &[PathSegment<'_>], index: usize, value_text: &[u8]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        if !is_sequence(&node) {
            return Err(err!(NotASequence));
        }
        let n = seq_len(&parsed, node)?;
        if index == usize::MAX || index == n {
            return self.splice_append(path, value_text);
        }
        if index > n {
            return Err(err!(NotFound));
        }
        if index == 0 {
            return self.splice_prepend(path, value_text);
        }
        let mut item = present(parsed.ast.child(&node)?)?;
        for _ in 0..index {
            item = present(parsed.ast.next(&item))?;
        }
        if self.is_flow_node(&parsed, node) {
            return self.insert_flow_item_before(&parsed, node, parsed.span(&node), Some(item), value_text, true);
        }
        if !self.syntax().block_seq_editable {
            return Err(err!(NotAnInlineArray));
        }
        let source = &self.source[..];
        let at = marker_start(&parsed, item);
        let insert_at = comment_block_start(source, line_start_before(source, at), self.syntax().comments);
        self.insert_seq_line(insert_at, at, value_text, place_of(&parsed, node))
    }

    /// Remove the item at `index` from the sequence at `path`. `index ==
    /// usize::MAX` is the "end" sentinel and means "the last item" here.
    ///
    /// The block arm takes the item's whole owned block, from its leading
    /// comment run above its marker's line through its last line. The flow
    /// arm takes the item and one separator beside it (`remove_flow_item`),
    /// the scan staying inside an open sequence's span. In a sequence the
    /// parser gave a separator of its own, an item that begins its line
    /// also takes the comment block above it, and the comment that ends
    /// its last line when the next item is on a later one.
    ///
    /// The first item of a block sequence that shares its line with the
    /// sequence's own item marker (`- - a`) takes only its own text
    /// (`shared_line_delete`). Every arm is held to `keep_others`, as
    /// `delete_key` is.
    pub fn remove_seq_item(&mut self, path: &[PathSegment<'_>], index: usize) -> Result<()> {
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        if !is_sequence(&node) {
            return Err(err!(NotASequence));
        }
        let n = seq_len(&parsed, node)?;
        let removed = if index == usize::MAX { n.saturating_sub(1) } else { index };
        let before = self.take_before();
        self.splice_out_item(path, index)?;
        self.keep_others(&before, path, removed)
    }

    fn splice_out_item(&mut self, path: &[PathSegment<'_>], index: usize) -> Result<()> {
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        if !is_sequence(&node) {
            return Err(err!(NotASequence));
        }
        let syn = self.syntax();
        // Walk to the target item, keeping `prev` (its immediate
        // preceding sibling, or `None` when it's first) alongside.
        let mut item = parsed.ast.child(&node)?.ok_or(err!(NotFound))?;
        let mut prev: Option<Node<'_>> = None;
        let mut at = index;
        if index == usize::MAX {
            at = 0;
            while let Some(nxt) = parsed.ast.next(&item) {
                prev = Some(item);
                item = nxt;
                at += 1;
            }
        } else {
            for _ in 0..index {
                prev = Some(item);
                item = parsed.ast.next(&item).ok_or(err!(NotFound))?;
            }
        }
        let is_first = prev.is_none();
        let item_span = parsed.span(&item);
        if self.is_flow_node(&parsed, node) {
            let source = &self.source[..];
            let joint = self.joint_of(&parsed, node);
            let span = parsed.span(&node);
            let limit = if self.is_open(&parsed, node) { span } else { Span::new(0, source.len()) };
            let mut start = item_span.start;
            let mut end = item_span.end;
            if joint.declared {
                let line_start = line_start_before(source, start);
                if first_non_space(source, line_start) == start {
                    let cbs = comment_block_start(source, line_start, syn.comments);
                    if cbs < line_start {
                        start = first_non_space(source, cbs).max(limit.start);
                    }
                    let next_on_later_line = match parsed.ast.next(&item) {
                        Some(n) => {
                            line_start_before(source, parsed.span(&n).start)
                                > line_start_before(source, end.saturating_sub(1))
                        }
                        None => true,
                    };
                    if next_on_later_line {
                        if let Some(trail) = self.line_end_comment(end, joint.token) {
                            end = trail.end;
                        }
                    }
                }
            }
            return self.remove_flow_item(
                Span::new(start, end),
                is_first,
                joint.token,
                Span::new(limit.start, limit.end.max(end)),
            );
        }
        if !syn.block_seq_editable {
            return Err(err!(NotAnInlineArray));
        }
        let (line_start, del_end) = {
            let source = &self.source[..];
            (
                comment_block_start(source, line_start_before(source, marker_start(&parsed, item)), syn.comments),
                line_end_after(source, item_span.end.saturating_sub(1)),
            )
        };
        let next = parsed.ast.next(&item);
        if is_first && next.is_none() {
            if self.emptied_section_seq(&parsed, node)? {
                return Ok(());
            }
            let mut out = Vec::new();
            if let Some(span) = self.emptied_block_seq(&mut out, &parsed, node, item, line_start, del_end)? {
                return self.replace_at_span(span, &out);
            }
        }
        if is_first {
            let next_start = next.map(|n| marker_start(&parsed, n));
            if let Some(span) = self.shared_line_delete(&parsed, node, marker_start(&parsed, item), item_span.end, next_start) {
                return self.replace_at_span(span, b"");
            }
        }
        // A section item may be written in several places (figl's `ys[0]`
        // over `ys[1]` over `ys[0]` again), and its span runs from its
        // first line to its last, over the items between: its own lines
        // are its gathered regions, as `delete_container` takes them.
        let edits = self.following_edits(&parsed, item, at);
        if parsed.is_section(&item) {
            let used = self.gather_regions(&parsed, item, true);
            return self.splice_out_edited(&used, &edits);
        }
        self.splice_out_edited(&[Region { start: line_start, end: del_end }], &edits)
    }

    /// What keeps each item after `item`, which stands at `index` in its
    /// block sequence, the item it was once `item`'s lines are deleted,
    /// as rewrites in source order. Every format that records no item path
    /// gets none.
    ///
    /// An item addressed by its index is renumbered: figl's index is a
    /// position (spec § 7.4). The next item's `Append` mention takes
    /// `item`'s, when the two differ: a line that repeats the header
    /// before it (figl's `+`, spec § 8) repeated `item`'s, which goes with
    /// `item`. Two headers spelled alike are left as they are.
    fn following_edits(&self, parsed: &Document, item: Node<'_>, index: usize) -> Vec<PathEdit> {
        let source = &self.source[..];
        let mut edits: Vec<PathEdit> = Vec::new();
        let mut header: Option<&[u8]> = None;
        for m in parsed.mentions_of(item.id) {
            if m.kind == MentionKind::Append {
                header = Some(&source[m.span.start..m.span.end]);
            }
        }
        let mut at = index + 1;
        let mut later = parsed.ast.next(&item);
        while let Some(n) = later {
            for m in parsed.mentions_of(n.id) {
                match m.kind {
                    MentionKind::Index => edits.push(PathEdit { span: m.span, text: format!("{}", at - 1).into_bytes() }),
                    MentionKind::Append => {
                        if at == index + 1 {
                            if let Some(h) = header {
                                if &source[m.span.start..m.span.end] != h {
                                    edits.push(padded(source, m.span, h));
                                }
                            }
                        }
                    }
                    MentionKind::Header | MentionKind::Entry => {}
                }
            }
            later = parsed.ast.next(&n);
            at += 1;
        }
        edits.sort_by_key(|e| e.span.start);
        edits
    }

    /// Delete `removed` (ascending, disjoint, as `regions::normalize`
    /// leaves them) and write each of `edits` (ascending, from
    /// `following_edits`) over its span, in one splice. An edit inside a
    /// removed region goes with it.
    fn splice_out_edited(&mut self, removed: &[Region], edits: &[PathEdit]) -> Result<()> {
        if edits.is_empty() {
            if removed.len() == 1 {
                return self.replace_at_span(Span::new(removed[0].start, removed[0].end), b"");
            }
            return regions::splice_out(self, removed);
        }
        let source = &self.source[..];
        let mut out = Vec::with_capacity(source.len());
        let mut pos = 0usize;
        let mut e = 0usize;
        for r in removed {
            while e < edits.len() && edits[e].span.start < r.start {
                if edits[e].span.start >= pos {
                    out.extend_from_slice(&source[pos..edits[e].span.start]);
                    out.extend_from_slice(&edits[e].text);
                    pos = edits[e].span.end;
                }
                e += 1;
            }
            out.extend_from_slice(&source[pos..r.start]);
            pos = r.end;
        }
        for edit in &edits[e..] {
            if edit.span.start < pos {
                continue;
            }
            out.extend_from_slice(&source[pos..edit.span.start]);
            out.extend_from_slice(&edit.text);
            pos = edit.span.end;
        }
        out.extend_from_slice(&source[pos..]);
        let len = source.len();
        self.replace_at_span(Span::new(0, len), &out)
    }

    /// The span to delete to remove the first entry or item of the
    /// block container `node`, running from `start` to `end`, when it
    /// shares its line with `node`'s own item marker — `- - a`, `- k: v`
    /// — or `None` when it begins its line and the line delete is right.
    /// `next` is where the sibling after it starts, if there is one.
    ///
    /// The sibling after it moves up into its place, at the column it
    /// already stands at, when nothing but the entry's trailing comment
    /// and blank lines stand between them; otherwise the sibling stays
    /// where it is and the parent's marker ends its line.
    fn shared_line_delete(&self, parsed: &Document, node: Node<'_>, start: usize, end: usize, next: Option<usize>) -> Option<Span> {
        let source = &self.source[..];
        let marker = parsed.marker_span(&node)?;
        let line_start = line_start_before(source, start);
        if marker.end > start || line_start_before(source, marker.start) != line_start {
            return None;
        }
        if let Some(n) = next {
            let next_line = line_start_before(source, n);
            if !(next_line <= line_start || n - next_line != start - line_start) {
                let mut gap = match self.line_end_comment(end, b"") {
                    Some(c) => c.end,
                    None => end,
                };
                while gap < n && is_whitespace(source[gap]) {
                    gap += 1;
                }
                if gap == n {
                    return Some(Span::new(start, n));
                }
            }
        }
        let mut s = start;
        while s > line_start && (source[s - 1] == b' ' || source[s - 1] == b'\t') {
            s -= 1;
        }
        let mut e = line_end_after(source, end.saturating_sub(1));
        if e > 0 && source[e - 1] == b'\n' {
            e -= 1;
        }
        if e > 0 && source[e - 1] == b'\r' {
            e -= 1;
        }
        Some(Span::new(s, e))
    }

    /// The span to splice `out` over to remove the ONLY item of the
    /// block sequence `node`, whose lines run from `line_start` to
    /// `del_end`, and leave `[]` in its place — or `None` when the plain
    /// line delete is right.
    ///
    /// A block sequence is nothing but its items, so deleting the lines
    /// of the last one deletes the sequence too: `links:` over `- a`
    /// would read back as `links:`, a null. A format with flow syntax
    /// spells the empty list `[]`, and it goes where the value began:
    /// in place of the item when something else shares its marker's
    /// line; after the entry's separator, or the parent item's marker,
    /// and any anchor or tag the sequence carries; alone, for the root.
    fn emptied_block_seq(
        &self,
        out: &mut Vec<u8>,
        parsed: &Document,
        node: Node<'_>,
        item: Node<'_>,
        line_start: usize,
        del_end: usize,
    ) -> Result<Option<Span>> {
        if !self.syntax().flow_containers || self.has_renderer(Renderer::Tail) {
            return Ok(None);
        }
        let source = &self.source[..];
        // The sequence's own text starts past its anchor and tag, which
        // stand on the line the value began on, and past the comment
        // that may end that line.
        let seq_span = parsed.span(&node);
        if seq_span.end > del_end {
            return Ok(None);
        }
        let mut body = seq_span.start;
        for p in [parsed.anchor_span(&node), parsed.tag_span(&node)].into_iter().flatten() {
            body = body.max(p.end);
        }
        if let Some(c) = self.line_end_comment(body, b"") {
            body = c.end;
        }
        let ends_line = del_end > 0 && source[del_end - 1] == b'\n';
        let marker = marker_start(parsed, item);
        if first_non_space(source, line_start_before(source, marker)) != marker {
            if body < marker {
                return Ok(None);
            }
            out.extend_from_slice(if ends_line { b"[]\n" } else { b"[]" });
            return Ok(Some(Span::new(marker, del_end)));
        }
        if body < line_start && !trim(&source[body..line_start], b" \t\r\n").is_empty() {
            return Ok(None);
        }
        let mut point: Option<usize> = None;
        if let Some(p) = parsed.ast.parent_of(node.id) {
            if matches!(parsed.ast.kind_of(p), NodeKind::KeyValue(_)) {
                if let Some(sep) = parsed.sep_span(&parsed.ast.node(p)) {
                    if sep.end > sep.start {
                        point = Some(sep.end);
                    }
                }
            }
        }
        for p in [parsed.marker_span(&node), parsed.anchor_span(&node), parsed.tag_span(&node)].into_iter().flatten() {
            point = Some(point.unwrap_or(p.end).max(p.end));
        }
        if let Some(at) = point {
            if at > line_start {
                return Ok(None);
            }
            out.extend_from_slice(b" []");
            // A file that ended without a newline still does.
            let kept = &source[at..line_start];
            out.extend_from_slice(if ends_line { kept } else { trim_end(kept, b"\r\n") });
            return Ok(Some(Span::new(at, del_end)));
        }
        if node.id != parsed.ast.root {
            return Ok(None);
        }
        out.extend_from_slice(if ends_line { b"[]\n" } else { b"[]" });
        Ok(Some(Span::new(line_start, del_end)))
    }

    /// Delete the only item of the sequence `node` when the sequence
    /// hangs under a header key, leaving the key and the format's empty
    /// list after it — `xs = []` — and return whether it did; false
    /// leaves the delete to the caller.
    ///
    /// A list under a header is a section: figl's `xs` over `> * a`, or
    /// the `xs[]` that appends an element and is that element's header
    /// too. The parser says a value hangs under its key by recording a
    /// zero-width separator at the key's end (`Document::sep_span`), and
    /// `Syntax.empty_seq_literal` goes there through `write_tail`.
    ///
    /// The document root has no key. A format that renders its own
    /// tails writes the empty list in place of the item, and refuses with
    /// `NoEmptyRootList` when it does not read that back.
    fn emptied_section_seq(&mut self, parsed: &Document, node: Node<'_>) -> Result<bool> {
        let Some(empty) = self.syntax().empty_seq_literal else { return Ok(false) };
        if node.id == parsed.ast.root {
            if !self.has_renderer(Renderer::Tail) {
                return Ok(false);
            }
            let used = self.gather_regions(parsed, node, true);
            if used.is_empty() {
                return Ok(false);
            }
            let source = &self.source[..];
            let mut out = Vec::with_capacity(source.len());
            let mut pos = 0usize;
            for (i, r) in used.iter().enumerate() {
                out.extend_from_slice(&source[pos..r.start]);
                if i == 0 {
                    out.extend_from_slice(empty);
                    out.push(b'\n');
                }
                pos = r.end;
            }
            out.extend_from_slice(&source[pos..]);
            let len = source.len();
            if self.replace_at_span(Span::new(0, len), &out).is_err() {
                // The text was the engine's, not a caller's.
                self.splice_rejected = false;
                return Err(err!(NoEmptyRootList));
            }
            return Ok(true);
        }
        if !parsed.is_section(&node) {
            return Ok(false);
        }
        let Some(kv_id) = parsed.ast.parent_of(node.id) else { return Ok(false) };
        if !matches!(parsed.ast.kind_of(kv_id), NodeKind::KeyValue(_)) {
            return Ok(false);
        }
        let kv = parsed.ast.node(kv_id);
        let Some(sep) = parsed.sep_span(&kv) else { return Ok(false) };
        let key_span = parsed.span(&parsed.ast.node(kv_of(&kv).key));
        if sep.start != key_span.end || sep.end != sep.start {
            return Ok(false);
        }

        let source = &self.source[..];
        let key_line = line_start_before(source, key_span.start);
        let key_line_end = line_end_after(source, key_span.end);
        let mut content_end = key_line_end;
        if content_end > key_line && source[content_end - 1] == b'\n' {
            content_end -= 1;
        }
        if content_end > key_line && source[content_end - 1] == b'\r' {
            content_end -= 1;
        }

        let place = place_of_child(parsed, node.id);
        let indent = self.indent_at(key_span.start);
        let rendered = self.rendered_value(empty, place)?;
        let mut line = Vec::new();
        line.extend_from_slice(&source[key_line..key_span.end]);
        self.write_tail(&mut line, &indent, &source[key_span.start..key_span.end], &rendered, place)?;
        if let Some(c) = self.header_comment(parsed, node, key_span.end, content_end) {
            line.extend_from_slice(&source[c.start..c.end]);
        }
        line.extend_from_slice(&source[content_end..key_line_end]);

        let used = self.gather_regions(parsed, node, true);
        let mut out = Vec::with_capacity(source.len());
        let mut pos = 0usize;
        let mut written = false;
        for r in &used {
            if !written && r.start <= key_line && key_line < r.end {
                // The comment block above the key's line is the key's.
                out.extend_from_slice(&source[pos..key_line]);
                out.extend_from_slice(&line);
                written = true;
            } else {
                out.extend_from_slice(&source[pos..r.start]);
            }
            pos = r.end;
        }
        if !written {
            return Ok(false);
        }
        out.extend_from_slice(&source[pos..]);
        let len = source.len();
        self.replace_at_span(Span::new(0, len), &out)?;
        Ok(true)
    }

    /// Delete the only element of the sequence `node`, at `path`, when
    /// each element is a header of its own (TOML's `[[x]]`), leaving
    /// the format's empty list in the body of the table that holds the
    /// sequence — `x = []` — and return whether it did; false leaves the
    /// delete to the caller.
    ///
    /// The key goes in the body of the table that holds `x`, through
    /// `insert_key`, as any new entry of that table does. A table with no
    /// header of its own has no body to take it, so the nearest table up
    /// that has a body takes the key path from there, spelled as a header
    /// spells it (`t.x = []`).
    ///
    /// The comment block above the element's header and the comment
    /// ending its line were the list's, and stay with `x = []`. The
    /// element's lines go as `delete_container` takes them, with the
    /// blank lines that set its header apart when a blank line or the
    /// end of the file follows it. The result must read back with an
    /// empty list at `path`, or nothing is written
    /// (`DeleteChangedOtherValues`).
    fn emptied_header_seq(&mut self, parsed: &Document, node: Node<'_>, path: &[PathSegment<'_>]) -> Result<bool> {
        let syn = self.syntax();
        let Some(empty) = syn.empty_seq_literal else { return Ok(false) };
        let Some(hdr) = syn.section_header else { return Ok(false) };
        if path.is_empty() || !parsed.is_section(&node) {
            return Ok(false);
        }
        let Some(kv_id) = parsed.ast.parent_of(node.id) else { return Ok(false) };
        if !matches!(parsed.ast.kind_of(kv_id), NodeKind::KeyValue(_)) {
            return Ok(false);
        }
        let Some(h) = parsed.ast.parent_of(kv_id) else { return Ok(false) };
        let mut holder = parsed.ast.node(h);
        let mut depth = 1usize;
        while !self.owns_header_line(parsed, holder) {
            let Some(up) = parsed.ast.parent_of(holder.id) else { return Ok(false) };
            if !matches!(parsed.ast.kind_of(up), NodeKind::KeyValue(_)) {
                return Ok(false);
            }
            let Some(hh) = parsed.ast.parent_of(up) else { return Ok(false) };
            holder = parsed.ast.node(hh);
            depth += 1;
        }
        if depth > path.len() {
            return Ok(false);
        }
        let key_path = &path[path.len() - depth..];
        if key_path.iter().any(|seg| !matches!(seg, PathSegment::Key(_))) {
            return Ok(false);
        }

        let (above, trail) = {
            let source = &self.source[..];
            let heads = parsed.regions_of(node.id);
            let line = line_start_before(source, heads[0].start as usize);
            let above = source[comment_block_start(source, line, syn.comments)..line].to_vec();
            let trail = match self.header_line_comment(parsed, node, hdr, line) {
                Some(c) => source[c.start..c.end].to_vec(),
                None => Vec::new(),
            };
            (above, trail)
        };
        let mut key_text = Vec::new();
        self.write_header_path(&mut key_text, hdr, key_path)?;

        let backup = self.source.clone();
        let mut used = self.gather_regions(parsed, node, true);
        {
            let source = &self.source[..];
            // The blank lines that set the element's header apart go with
            // it when a blank line or the end of the file follows it.
            let mut prev_end = 0usize;
            for r in used.iter_mut() {
                if r.end == source.len() || source[r.end] == b'\n' {
                    while r.start >= prev_end + 2 && source[r.start - 1] == b'\n' && source[r.start - 2] == b'\n' {
                        r.start -= 1;
                    }
                }
                prev_end = r.end;
            }
        }
        regions::splice_out(self, &used)?;
        if self.write_emptied(path, &path[..path.len() - depth], &key_text, empty, &above, &trail).is_err() {
            self.restore_source(&backup)?;
            self.splice_rejected = false;
            return Err(err!(DeleteChangedOtherValues));
        }
        Ok(true)
    }

    /// The comment ending the element header of the section sequence
    /// `node` that stands on the line at `line` — `# c` in `[[x]] # c`
    /// — as the span from the spaces before it, or `None`. It is found
    /// where it is written: past the sequence's name on that line and the
    /// header's close.
    fn header_line_comment(&self, parsed: &Document, node: Node<'_>, hdr: SectionHeader, line: usize) -> Option<Span> {
        let source = &self.source[..];
        let close = hdr.seq_close.unwrap_or(hdr.close);
        for m in parsed.mentions_of(node.id) {
            if line_start_before(source, m.span.start) != line {
                continue;
            }
            let mut i = first_non_space(source, m.span.end);
            if !source[i..].starts_with(close) {
                return None;
            }
            i += close.len();
            return self.line_end_comment(i, b"");
        }
        None
    }

    /// Write `key_text = empty` into the table at `holder`, with `above`
    /// over its line and `trail` after it, and check that the list at
    /// `path` reads back empty. `emptied_header_seq`'s second half.
    fn write_emptied(
        &mut self,
        path: &[PathSegment<'_>],
        holder: &[PathSegment<'_>],
        key_text: &[u8],
        empty: &[u8],
        above: &[u8],
        trail: &[u8],
    ) -> Result<()> {
        self.insert_key(holder, key_text, empty)?;
        let parsed = self.parsed()?;
        let seq = parsed.ast.get_val_by_path(path)?;
        if !matches!(seq.kind, NodeKind::Sequence(None)) {
            return Err(err!(DeleteChangedOtherValues));
        }
        if above.is_empty() && trail.is_empty() {
            return Ok(());
        }
        let source = &self.source[..];
        let span = parsed.span(&parsed.ast.get_node_by_path(path)?);
        let line = line_start_before(source, span.start);
        let mut out = Vec::new();
        out.extend_from_slice(above);
        out.extend_from_slice(&source[line..span.end]);
        out.extend_from_slice(trail);
        self.replace_at_span(Span::new(line, span.end), &out)
    }

    /// Whether the mapping `mapping` has a body of its own to take a
    /// new entry: it is the root, an element of a sequence of sections
    /// (its `[[a]]` line), or a section with a header line that is its
    /// own rather than one of its children's (`[t]`, not the `t` in
    /// `[t.u]` or `[[t.x]]`). A dotted table's lines are its parent's.
    fn owns_header_line(&self, parsed: &Document, mapping: Node<'_>) -> bool {
        if !is_mapping(&mapping) {
            return false;
        }
        let Some(parent) = parsed.ast.parent_of(mapping.id) else { return true };
        if matches!(parsed.ast.kind_of(parent), NodeKind::Sequence(_)) {
            return true;
        }
        let source = &self.source[..];
        'header: for m in parsed.mentions_of(mapping.id) {
            if m.kind != MentionKind::Header {
                continue;
            }
            let line = line_start_before(source, m.span.start);
            let Ok(mut cur) = parsed.ast.child(&mapping) else { return false };
            while let Some(kv) = cur {
                if self.out_of_region(parsed, kv) && line_start_before(source, parsed.span(&kv).start) == line {
                    continue 'header;
                }
                cur = parsed.ast.next(&kv);
            }
            return true;
        }
        false
    }

    /// Whether the mapping `node` is a table with no header line of its
    /// own — TOML's `a` made by `a.b = 1` alone, by `[a.b]` alone, or by
    /// `a.b = 1` inside an inline table, and fig's `a` in `{a.b=1}` —
    /// and what holds it. `None` for any other node.
    fn headerless_holder<'a>(&self, parsed: &'a Document, node: Node<'a>) -> Option<Headerless<'a>> {
        if !self.is_section_format {
            return None;
        }
        if !self.written_by_parent(parsed, node) {
            return None;
        }
        let mut cur = node;
        let mut depth = 1usize;
        loop {
            let up = parsed.ast.node(parsed.ast.parent_of(parsed.ast.parent_of(cur.id)?)?);
            if !is_mapping(&up) {
                return None;
            }
            if self.is_flow_node(parsed, up) {
                return Some(Headerless { holder: up, top: cur, depth, flow: true });
            }
            if !self.written_by_parent(parsed, up) {
                // A dotted table among the block lines of a format whose
                // line prefix is structural (fig's) is left to
                // `insert_block_key`, which copies the bytes up to the
                // first entry's key, key path and all, as that prefix.
                if !parsed.is_section(&cur) || self.syntax().structural_indent {
                    return None;
                }
                return Some(Headerless { holder: up, top: cur, depth, flow: false });
            }
            cur = up;
            depth += 1;
        }
    }

    /// Whether the mapping `node` is a table made by dotted keys, whose
    /// entries are its parent's lines or entries: a section with no
    /// header line of its own, or a value `dotted_value` names.
    fn written_by_parent(&self, parsed: &Document, node: Node<'_>) -> bool {
        if !is_mapping(&node) {
            return false;
        }
        let Some(kv) = parsed.ast.parent_of(node.id) else { return false };
        if !matches!(parsed.ast.kind_of(kv), NodeKind::KeyValue(_)) {
            return false;
        }
        if parsed.is_section(&node) {
            return !self.owns_header_line(parsed, node);
        }
        dotted_value(parsed, parsed.ast.node(kv)).is_some()
    }

    /// The entry under the headerless table `node` that is written last
    /// among its holder's lines or braces, or `None` when none is: one
    /// that is not itself a table written there, found down through
    /// the tables that are. An entry under a header of its own is passed
    /// over.
    fn last_written_leaf<'a>(&self, parsed: &'a Document, node: Node<'a>, flow: bool) -> Result<Option<Node<'a>>> {
        let mut last: Option<Node<'a>> = None;
        let mut cur = parsed.ast.child(&node)?;
        while let Some(kv) = cur {
            cur = parsed.ast.next(&kv);
            if !is_keyvalue(&kv) {
                continue;
            }
            if !flow && self.out_of_region(parsed, kv) {
                continue;
            }
            let leaf = match dotted_value(parsed, kv) {
                Some(value) => self.last_written_leaf(parsed, value, flow)?,
                None => Some(kv),
            };
            if let Some(l) = leaf {
                if last.is_none() || parsed.span(&l).end > parsed.span(&last.unwrap()).end {
                    last = Some(l);
                }
            }
        }
        Ok(last)
    }

    /// The first entry of the headerless table `node` written among its
    /// holder's lines or braces, or `None` when none is.
    fn first_written_child<'a>(&self, parsed: &'a Document, node: Node<'a>, flow: bool) -> Option<Node<'a>> {
        let mut cur = parsed.ast.child(&node).ok()?;
        while let Some(kv) = cur {
            cur = parsed.ast.next(&kv);
            if !is_keyvalue(&kv) {
                continue;
            }
            if !flow && self.out_of_region(parsed, kv) {
                continue;
            }
            return Some(kv);
        }
        None
    }

    /// Insert `key_text` and `value_text` into the table `node` at
    /// `path`, which has no header of its own (`headerless_holder`).
    ///
    /// Its entries are lines of the holder's body or entries of the
    /// holder's braces, each a key path from the holder down (`a.b = 1`
    /// for `a`), and a new one is written the same way. The key path is
    /// copied from the table's first entry as written, up to the key under
    /// the table, so its segments keep their quoting and spacing. A table
    /// with no entry there (`a` made by `[a.b]` alone) takes the path
    /// spelled as a header spells it, in the holder's body. A format with
    /// no header spelling leaves such a table to `insert_block_key`, which
    /// refuses it (`ImplicitSection`).
    ///
    /// The result must read back with the table grown and nothing else
    /// changed, or nothing is written (`ImplicitSection`).
    fn insert_headerless(
        &mut self,
        parsed: &Document,
        path: &[PathSegment<'_>],
        node: Node<'_>,
        owner: Headerless<'_>,
        key_text: &[u8],
        value_text: &[u8],
    ) -> Result<()> {
        let mut key: Vec<u8> = Vec::new();
        let leaf = self.last_written_leaf(parsed, node, owner.flow)?;
        if let Some(below) = self.first_written_child(parsed, node, owner.flow) {
            // From the place the top table's name begins the table's
            // first entry to the key just under `node` on it.
            let first = first_written_leaf(parsed, below);
            let from = dotted_entry_start(parsed, owner.top, parsed.span(&first).start);
            let to = match dotted_value(parsed, below) {
                Some(table) => dotted_entry_start(parsed, table, parsed.span(&first).start),
                None => parsed.span(&below).start,
            };
            key.extend_from_slice(&self.source[from..to]);
        } else {
            let Some(hdr) = self.syntax().section_header else {
                return self.insert_block_key(parsed, node, key_text, value_text);
            };
            if owner.depth > path.len() {
                return Err(err!(ImplicitSection));
            }
            let key_path = &path[path.len() - owner.depth..];
            if key_path.iter().any(|seg| !matches!(seg, PathSegment::Key(_))) {
                return Err(err!(ImplicitSection));
            }
            self.write_header_path(&mut key, hdr, key_path)?;
            key.extend_from_slice(hdr.sep);
        }
        key.extend_from_slice(key_text);

        let before = self.take_before();
        if owner.flow {
            self.insert_flow_map_entry(parsed, owner.holder, parsed.span(&owner.holder), true, &key, value_text)?;
        } else if let Some(l) = leaf {
            let place = place_of(parsed, node);
            let rendered = self.rendered_value(value_text, place)?;
            let indent = self.indent_at(dotted_entry_start(parsed, owner.top, parsed.span(&l).start));
            let source = &self.source[..];
            let insert_at = line_end_after(source, parsed.span(&l).end.saturating_sub(1));
            let mut out = Vec::new();
            if !at_line_start(source, insert_at) {
                out.push(b'\n');
            }
            out.extend_from_slice(&indent);
            self.write_entry(&mut out, &indent, &key, &rendered, place)?;
            out.push(b'\n');
            self.replace_at_span(Span::new(insert_at, insert_at), &out)?;
        } else {
            self.insert_block_key(parsed, owner.holder, &key, value_text)?;
        }

        let Ok(b) = self.parse_of_backup(&before) else {
            self.restore_source(&before.bytes)?;
            return Err(err!(ImplicitSection));
        };
        let after = self.parsed()?;
        if !changed_only_at(&b, &after, path, Change::Added) {
            self.restore_source(&before.bytes)?;
            return Err(err!(ImplicitSection));
        }
        Ok(())
    }

    /// The comment ending the header line of the section sequence
    /// `node` between `from` and `to` — the sequence's own trailing
    /// comment, or its only item's when the item's first header line is
    /// that line — as the span from the spaces before it; `None` when
    /// neither has one there.
    fn header_comment(&self, parsed: &Document, node: Node<'_>, from: usize, to: usize) -> Option<Span> {
        let marker = self.trailing_comment_marker()?;
        let source = &self.source[..];
        let line = line_start_before(source, from);
        let mut text = parsed.ast.comments(node.id).trailing;
        if text.is_none() {
            if let Ok(Some(item)) = parsed.ast.child(&node) {
                let heads = parsed.regions_of(item.id);
                if !heads.is_empty() && heads[0].start as usize == line {
                    text = parsed.ast.comments(item.id).trailing;
                }
            }
        }
        let want = text?.text;
        let mut i = from;
        while let Some(at) = find_from(&source[..to], i, marker.open) {
            i = at + 1;
            if at == from || (source[at - 1] != b' ' && source[at - 1] != b'\t') {
                continue;
            }
            let Some(c) = self.line_end_comment(at, b"") else { continue };
            let body = trim(&source[at + marker.open.len()..c.end], b" \t");
            if body != want {
                continue;
            }
            let mut start = at;
            while start > from && (source[start - 1] == b' ' || source[start - 1] == b'\t') {
                start -= 1;
            }
            return Some(Span::new(start, c.end));
        }
        None
    }

    /// Reconcile the sequence at `path` so its items are exactly `items` —
    /// each an already-serialized *scalar* value in this document's format —
    /// while preserving the comments on items that survive the change.
    ///
    /// Items are matched to the current items by abstract value (kind +
    /// value, honoring multiplicity), so an item that is kept or merely
    /// reordered keeps its leading and trailing comments; only a genuinely
    /// new value is inserted and only a genuinely dropped value is deleted.
    /// The final item order matches `items`.
    ///
    /// It is a thin orchestration over `append_to_seq` / `remove_seq_item` /
    /// `reorder_items`: append the new values, delete the dropped ones, then
    /// reorder to `items`. The compound edit is atomic — on any error the
    /// document is restored byte-for-byte.
    ///
    /// Declines (errors) rather than guessing when the shape isn't a flat
    /// scalar list it can safely diff:
    ///   * a target that isn't a sequence value -> `NotASequence`;
    ///   * empty `items`, an empty current list, or any non-scalar item on
    ///     either side -> `UnsupportedShape`.
    /// A format whose scalars cannot stand alone as a document (TOML) also
    /// surfaces as `UnsupportedShape`.
    ///
    /// A sequence that is not there — an absent key or a null — is
    /// started as `set_append` starts one, and `items` appended to it in
    /// order, there being no item to keep; atomic as the rest is.
    pub fn set_sequence(&mut self, path: &[PathSegment<'_>], items: &[&[u8]]) -> Result<()> {
        if items.is_empty() {
            return Err(err!(UnsupportedShape));
        }

        if self.seq_missing(path) {
            let backup = self.source.clone();
            if self.seed_seq(path)? {
                for text in items {
                    if let Err(e) = self.append_to_seed(path, text) {
                        self.restore_source(&backup)?;
                        return Err(e);
                    }
                }
                return Ok(());
            }
        }

        // ---- plan against the current parse (no mutation yet) ----
        let parsed = self.parsed()?;
        let mut cur: Vec<NodeKind<'_>> = Vec::new();
        {
            let node = parsed.ast.get_val_by_path(path)?;
            if !is_sequence(&node) {
                return Err(err!(NotASequence));
            }
            let mut maybe = parsed.ast.child(&node)?;
            while let Some(item) = maybe {
                if !is_scalar_kind(&item.kind) {
                    return Err(err!(UnsupportedShape));
                }
                cur.push(item.kind);
                maybe = parsed.ast.next(&item);
            }
        }
        if cur.is_empty() {
            return Err(err!(UnsupportedShape));
        }

        // Target item kinds: parse each serialized value back to a scalar so
        // matching is by abstract value, not formatting (`1` != `'1'`). A
        // format whose scalar can't stand alone as a document (TOML) fails
        // the parse and is declined here.
        let mut tdocs: Vec<Document> = Vec::with_capacity(items.len());
        for text in items {
            let d = self.lang.parse(text, self.format).map_err(|_| err!(UnsupportedShape))?;
            if !is_scalar_kind(&d.ast.node(d.ast.root).kind) {
                return Err(err!(UnsupportedShape));
            }
            tdocs.push(d);
        }
        let tgt: Vec<NodeKind<'_>> = tdocs.iter().map(|d| d.ast.node(d.ast.root).kind).collect();

        let m = cur.len();
        let t = tgt.len();

        // Occurrence index of element `i` = how many earlier elements share
        // its value. (kind, occ) is the per-item identity used for matching,
        // so duplicate values are paired up by their order of appearance.
        let occ = |kinds: &[NodeKind<'_>], i: usize| -> usize { kinds[..i].iter().filter(|k| k.eql(&kinds[i])).count() };

        // A current item survives iff some target item shares its identity.
        let mut removed = vec![true; m];
        let mut removed_count = 0usize;
        for i in 0..m {
            for j in 0..t {
                if cur[i].eql(&tgt[j]) && occ(&cur, i) == occ(&tgt, j) {
                    removed[i] = false;
                    break;
                }
            }
            if removed[i] {
                removed_count += 1;
            }
        }

        // A target item is an addition iff no current item shares its identity.
        let mut additions: Vec<usize> = Vec::new();
        for j in 0..t {
            let present = (0..m).any(|i| cur[i].eql(&tgt[j]) && occ(&cur, i) == occ(&tgt, j));
            if !present {
                additions.push(j);
            }
        }

        // The physical order after append+remove is survivors (old order)
        // then additions (target order).
        #[derive(Clone, Copy)]
        enum Slot {
            Keep(usize),
            Add(usize),
        }
        let mut slots: Vec<Slot> = Vec::new();
        for i in 0..m {
            if !removed[i] {
                slots.push(Slot::Keep(i));
            }
        }
        for &j in &additions {
            slots.push(Slot::Add(j));
        }

        // `order[k]` = the slot holding target item `k`, so a reorder by
        // `order` (a full permutation) lands the sequence in target order.
        let mut order = vec![0usize; t];
        let mut used = vec![false; slots.len()];
        for k in 0..t {
            let mut found: Option<usize> = None;
            for (s, slot) in slots.iter().enumerate() {
                if used[s] {
                    continue;
                }
                let hit = match *slot {
                    Slot::Keep(i) => cur[i].eql(&tgt[k]) && occ(&cur, i) == occ(&tgt, k),
                    Slot::Add(j) => j == k,
                };
                if hit {
                    found = Some(s);
                    break;
                }
            }
            let s = found.ok_or(err!(UnsupportedShape))?; // unreachable by construction
            order[k] = s;
            used[s] = true;
        }

        // No-op: same items, same order — leave the bytes untouched so a
        // redundant set never churns formatting.
        let needs_reorder = order.iter().enumerate().any(|(k, &o)| o != k);
        if removed_count == 0 && additions.is_empty() && !needs_reorder {
            return Ok(());
        }

        // ---- apply: append, remove, reorder — atomic across all steps ----
        let backup = self.source.clone();
        let applied = (|| -> Result<()> {
            // Append first so a full replacement never empties the block
            // mid-edit (an empty block sequence has no valid syntax).
            for &j in &additions {
                self.append_to_seq(path, items[j])?;
            }
            // Remove dropped originals high-index-first so lower indices
            // stay put.
            let mut di = m;
            while di > 0 {
                di -= 1;
                if removed[di] {
                    self.remove_seq_item(path, di)?;
                }
            }
            if needs_reorder {
                self.reorder_items(path, &order)?;
            }
            Ok(())
        })();
        if let Err(e) = applied {
            // `backup` parsed before, so the reparse cannot fail.
            self.source = backup;
            let _ = self.reparse();
            return Err(e);
        }
        Ok(())
    }

    // ============
    // MOVE / REORDER
    // ============
    //
    // Like insert/delete, these never reserialize: they relocate whole entry
    // blocks (a mapping key's owned comment block + line(s), or a sequence
    // item's) and reuse `replace_at_span` to splice + reparse. The moved
    // bytes are the originals, so comments, quoting, and formatting ride
    // along. Block containers tile into per-entry blocks (trailing trivia
    // rides with the preceding entry); a flow sequence (`[a, b]`) reuses its
    // original separators so only the items move.

    /// Move the mapping entry named by `src_path` to sit immediately before
    /// the entry named by `dest_path`. Both paths must name keys in the
    /// *same* block mapping. The moved entry carries its owned leading
    /// comment block and any trailing same-line comment; the bytes between
    /// the two entries are preserved. Moving an entry to before itself (or
    /// into its own comment block) is a no-op.
    ///
    /// **Engine rule**: a SECTION node at either end is refused before any
    /// splice (`CannotMoveSection`). `move_container` is the op that
    /// relocates a scattered container whole.
    ///
    /// Every move and reorder is held to what it promises (`reads_back`):
    /// the container's children read back as they were, in the order
    /// asked for, and nothing else changed, or `EditReadsBackWrong` and
    /// the source as it was.
    pub fn move_key(&mut self, src_path: &[PathSegment<'_>], dest_path: &[PathSegment<'_>]) -> Result<()> {
        let guard = self.move_guard(src_path, dest_path)?;
        self.splice_move_key(src_path, dest_path)?;
        if let Some(g) = &guard {
            self.reads_back(g)?;
        }
        Ok(())
    }

    /// `move_key`'s guard: the entry at `src_path` lifted out of its
    /// mapping and set down before the one at `dest_path`. A `dest` that
    /// is not its sibling leaves the order as it was. `None` when either
    /// path names no entry, which the splice reports.
    fn move_guard(&self, src_path: &[PathSegment<'_>], dest_path: &[PathSegment<'_>]) -> Result<Option<Guard>> {
        let parsed = self.parsed()?;
        let Ok(src) = parsed.ast.get_node_by_path(src_path) else { return Ok(None) };
        let Ok(dest) = parsed.ast.get_node_by_path(dest_path) else { return Ok(None) };
        let Some(from) = slot_of(&parsed, src.id) else { return Ok(None) };
        let Some(to) = slot_of(&parsed, dest.id) else { return Ok(None) };
        let before = if to.container == from.container { to.at } else { from.at };
        let container = parsed.ast.node(from.container);
        let n = seq_len(&parsed, container)?;
        let mut order = Vec::with_capacity(n);
        for j in 0..n {
            if before != from.at && j == from.at {
                continue;
            }
            if before != from.at && j == before {
                order.push(from.at);
            }
            order.push(j);
        }
        Ok(Some(self.guard_at(&parsed, container, Write::Moved(order))?))
    }

    fn splice_move_key(&mut self, src_path: &[PathSegment<'_>], dest_path: &[PathSegment<'_>]) -> Result<()> {
        let parsed = self.parsed()?;
        let src = parsed.ast.get_node_by_path(src_path)?;
        let NodeKind::KeyValue(src_kv) = src.kind else { return Err(err!(NotAMapping)) };
        let dest = parsed.ast.get_node_by_path(dest_path)?;
        let NodeKind::KeyValue(dest_kv) = dest.kind else { return Err(err!(NotAMapping)) };
        if parsed.is_section(&parsed.ast.node(src_kv.value)) || parsed.is_section(&parsed.ast.node(dest_kv.value)) {
            return Err(refuse(SectionOp::Move));
        }
        // A flow mapping's members are not one per line and are joined by
        // separators, the last having none: moving lines would strand a
        // comma or drop one. Move by member instead, as `reorder_keys` does.
        if !src_path.is_empty() {
            let parent = parsed.ast.get_val_by_path(&src_path[..src_path.len() - 1])?;
            if is_mapping(&parent) && self.is_flow_node(&parsed, parent) {
                let mut entries: Vec<Node<'_>> = Vec::new();
                let mut src_idx: Option<usize> = None;
                let mut dest_idx: Option<usize> = None;
                let mut maybe = parsed.ast.child(&parent)?;
                while let Some(entry) = maybe {
                    if entry.id == src.id {
                        src_idx = Some(entries.len());
                    }
                    if entry.id == dest.id {
                        dest_idx = Some(entries.len());
                    }
                    entries.push(entry);
                    maybe = parsed.ast.next(&entry);
                }
                let from = src_idx.ok_or(err!(NotFound))?;
                let before = dest_idx.ok_or(err!(NotFound))?;
                if from == before {
                    return Ok(());
                }
                // Every member in its place, with `from` lifted out and
                // set down just before `before`.
                let mut order: Vec<usize> = Vec::new();
                for i in 0..entries.len() {
                    if i == from {
                        continue;
                    }
                    if i == before {
                        order.push(from);
                    }
                    order.push(i);
                }
                return self.reorder_flow_entries(&parsed, &entries, &order, self.syntax().flow_entry_sep);
            }
        }
        let source = &self.source[..];
        let comments = self.syntax().comments;
        let src_start = entry_block_start(source, parsed.span(&src), comments);
        let src_end = entry_block_end(source, parsed.span(&src));
        let dest_start = entry_block_start(source, parsed.span(&dest), comments);
        self.move_block(src_start, src_end, dest_start)
    }

    /// Move the sequence item at index `from` to index `to` (both positions
    /// in the current order; standard array-move semantics — the item is
    /// removed and reinserted, shifting the others to fill). A block item
    /// carries its owned leading comment block. No-op when `from == to`.
    pub fn move_item(&mut self, path: &[PathSegment<'_>], from: usize, to: usize) -> Result<()> {
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        if !is_sequence(&node) {
            return Err(err!(NotASequence));
        }
        let n = seq_len(&parsed, node)?;
        if from >= n || to >= n {
            return Err(err!(NotFound));
        }
        if from == to {
            return Ok(());
        }
        // Build the post-move index order, then reorder by it.
        let mut order: Vec<usize> = (0..n).collect();
        let val = order[from];
        if from < to {
            for i in from..to {
                order[i] = order[i + 1];
            }
        } else {
            let mut i = from;
            while i > to {
                order[i] = order[i - 1];
                i -= 1;
            }
        }
        order[to] = val;
        let guard = self.guard_at(&parsed, node, Write::Moved(order.clone()))?;
        self.reorder_seq_node(&parsed, node, &order)?;
        self.reads_back(&guard)
    }

    /// Reorder the entries of the block mapping at `path` (empty path =
    /// root) so the keys listed in `keys` come first, in that order; entries
    /// whose key is not listed keep their original relative order and follow.
    /// Keys in `keys` that the mapping does not contain are ignored. Each
    /// entry's owned comments — and any interleaved blank lines / orphan
    /// comments, which ride with the entry that precedes them — are
    /// preserved. A flow mapping takes `reorder_flow_entries` instead.
    ///
    /// **Engine rule**: once the new order is known and before any splice,
    /// an entry whose position actually changes and whose value is a
    /// SECTION node is refused (`CannotReorderSections`).
    /// `reorder_containers` is the op that reorders scattered containers
    /// whole.
    pub fn reorder_keys(&mut self, path: &[PathSegment<'_>], keys: &[&[u8]]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = match parsed.ast.get_val_by_path(path) {
            Ok(n) => n,
            Err(_) => return self.splice_key_order(path, keys),
        };
        if !is_mapping(&node) {
            return self.splice_key_order(path, keys);
        }
        // The order asked for: the listed keys first, each its first
        // entry not yet taken, then the rest as they stand.
        let mut listed: Vec<usize> = Vec::new();
        let n = seq_len(&parsed, node)?;
        let mut taken = vec![false; n];
        for k in keys {
            let mut maybe = parsed.ast.child(&node)?;
            let mut i = 0usize;
            while let Some(entry) = maybe {
                if let NodeKind::KeyValue(kv) = entry.kind {
                    if !taken[i] && key_node_is(&parsed, kv.key, k) {
                        taken[i] = true;
                        listed.push(i);
                        break;
                    }
                }
                maybe = parsed.ast.next(&entry);
                i += 1;
            }
        }
        let guard = self.guard_at(&parsed, node, Write::Moved(full_order(&listed, n)))?;
        self.splice_key_order(path, keys)?;
        self.reads_back(&guard)
    }

    fn splice_key_order(&mut self, path: &[PathSegment<'_>], keys: &[&[u8]]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        let NodeKind::Mapping(first) = node.kind else { return Err(err!(NotAMapping)) };
        let Some(first_id) = first else { return Ok(()) }; // empty mapping

        // Gather each entry and its key (for matching), in document order.
        let mut entry_keys: Vec<&[u8]> = Vec::new();
        let mut entries: Vec<Node<'_>> = Vec::new();
        let mut cur = parsed.ast.node(first_id);
        loop {
            let NodeKind::KeyValue(kv) = cur.kind else { return Err(err!(InvalidDocument)) };
            let NodeKind::String(key) = parsed.ast.node(kv.key).kind else { return Err(err!(InvalidDocument)) };
            entry_keys.push(key);
            entries.push(cur);
            match parsed.ast.next(&cur) {
                Some(n) => cur = n,
                None => break,
            }
        }

        // Translate the requested keys into entry indices (first unused match
        // wins), then reorder the blocks by that index list.
        let mut order: Vec<usize> = Vec::new();
        let mut chosen = vec![false; entries.len()];
        for k in keys {
            for (i, seen) in entry_keys.iter().enumerate() {
                if !chosen[i] && seen == k {
                    order.push(i);
                    chosen[i] = true;
                    break;
                }
            }
        }
        // A flow mapping holds no section, so the veto below has nothing
        // to refuse there; its members are spliced comma-aware.
        if self.is_flow_node(&parsed, node) {
            return self.reorder_flow_entries(&parsed, &entries, &order, self.syntax().flow_entry_sep);
        }
        // The engine's veto, before anything is spliced — over exactly the
        // entries whose position this reorder changes, since an entry left
        // where it was is never at risk. Skipped for a format with no
        // sections, where nothing could be refused.
        if self.is_section_format {
            // Where each entry ends up: the listed keys first, in `order`,
            // then the unlisted ones in their original relative order.
            let mut final_pos = vec![0usize; entries.len()];
            let mut pos = 0usize;
            for &i in &order {
                final_pos[i] = pos;
                pos += 1;
            }
            for (i, c) in chosen.iter().enumerate() {
                if !c {
                    final_pos[i] = pos;
                    pos += 1;
                }
            }
            for (idx, entry) in entries.iter().enumerate() {
                if final_pos[idx] != idx && parsed.is_section(&parsed.ast.node(kv_of(entry).value)) {
                    return Err(refuse(SectionOp::Reorder));
                }
            }
        }
        let source = &self.source[..];
        let comments = self.syntax().comments;
        let mut blocks: Vec<Block> = entries
            .iter()
            .map(|entry| Block { start: entry_block_start(source, parsed.span(entry), comments), end: 0 })
            .collect();
        let last_end = entry_block_end(source, parsed.span(&entries[entries.len() - 1]));
        tile_blocks(&mut blocks, last_end);
        self.reorder_blocks(blocks[0].start, last_end, &blocks, &order)
    }

    /// Reorder the items of the sequence at `path` (block or flow) so the
    /// items at the indices listed in `indices` (positions in the current
    /// order) come first, in that order; items not listed keep their
    /// original relative order and follow. Out-of-range indices are ignored.
    /// Block items carry their owned comments; a flow sequence keeps its
    /// original separators so only the items move.
    pub fn reorder_items(&mut self, path: &[PathSegment<'_>], indices: &[usize]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        if !is_sequence(&node) {
            return Err(err!(NotASequence));
        }
        let n = seq_len(&parsed, node)?;
        let guard = self.guard_at(&parsed, node, Write::Moved(full_order(indices, n)))?;
        self.reorder_seq_node(&parsed, node, indices)?;
        self.reads_back(&guard)
    }

    // --- move / reorder internals ---

    /// Reorder a sequence node's items by `order` (bring-to-front indices),
    /// dispatching on flow vs block style.
    fn reorder_seq_node(&mut self, parsed: &Document, node: Node<'_>, order: &[usize]) -> Result<()> {
        let mut spans: Vec<Span> = Vec::new();
        // Each item's block starts on its MARKER's line (see
        // `marker_start`), which for a nested or empty item is above its
        // span's.
        let mut starts: Vec<usize> = Vec::new();
        let mut maybe = parsed.ast.child(&node)?;
        while let Some(item) = maybe {
            spans.push(parsed.span(&item));
            starts.push(marker_start(parsed, item));
            maybe = parsed.ast.next(&item);
        }
        if spans.is_empty() {
            return Ok(());
        }
        // A sequence the parser gave a separator of its own moves its
        // items as a flow mapping moves its members, so a `.tune` block
        // takes its comments with it; a sequence with the dialect's
        // separator keeps its slots' trivia where they are.
        let joint = self.joint_of(parsed, node);
        if joint.declared {
            let mut items: Vec<Node<'_>> = Vec::new();
            let mut it = parsed.ast.child(&node)?;
            while let Some(item) = it {
                items.push(item);
                it = parsed.ast.next(&item);
            }
            return self.reorder_flow_entries(parsed, &items, order, joint.token);
        }
        if self.is_flow_node(parsed, node) {
            return self.reorder_flow_items(&spans, order);
        }
        let syn = self.syntax();
        if !syn.block_seq_editable {
            return Err(err!(NotAnInlineArray));
        }
        let source = &self.source[..];
        let mut blocks: Vec<Block> = starts
            .iter()
            .map(|&at| Block { start: comment_block_start(source, line_start_before(source, at), syn.comments), end: 0 })
            .collect();
        let last_end = entry_block_end(source, spans[spans.len() - 1]);
        tile_blocks(&mut blocks, last_end);
        let perm = full_order(order, blocks.len());
        let edits = self.reorder_edits(parsed, node, &perm)?;
        if edits.is_empty() {
            return self.reorder_blocks(blocks[0].start, last_end, &blocks, &perm);
        }
        // The whole source is rewritten, since an item may be addressed
        // on a line outside the blocks (figl's `ys[0]` written again
        // after `ys[1]`).
        let mut region: Vec<u8> = Vec::new();
        let mut block: Vec<u8> = Vec::new();
        for &i in &perm {
            block.clear();
            append_edited(&mut block, source, blocks[i].start, blocks[i].end, &edits);
            append_block_sep(&mut region, &block, &syn);
        }
        let mut out = Vec::with_capacity(source.len());
        append_edited(&mut out, source, 0, blocks[0].start, &edits);
        out.extend_from_slice(&region);
        append_edited(&mut out, source, last_end, source.len(), &edits);
        let len = source.len();
        self.replace_at_span(Span::new(0, len), &out)
    }

    /// What keeps each item of the block sequence `seq` addressing
    /// itself once its items stand in the order `perm` (`perm[k]` the
    /// item that goes to position `k`), as rewrites in source order;
    /// `following_edits` is the delete's. Every other format records no
    /// item path and gets none.
    ///
    /// An item addressed by its index takes its new one at each `Index`
    /// mention. A line that appends an item (figl's `a[]`, or the `+`
    /// that repeats it) is how the position it stands in is written, so
    /// the item that comes to stand there is written with it. The item's
    /// trailing comment stays with it, in its column.
    fn reorder_edits(&self, parsed: &Document, seq: Node<'_>, perm: &[usize]) -> Result<Vec<PathEdit>> {
        let source = &self.source[..];
        let mut items: Vec<Node<'_>> = Vec::new();
        let mut it = parsed.ast.child(&seq)?;
        while let Some(item) = it {
            items.push(item);
            it = parsed.ast.next(&item);
        }
        if items.len() != perm.len() {
            return Ok(Vec::new());
        }
        // Each item's one `Append` mention, by its place in the source.
        let mut appended: Vec<Option<Span>> = vec![None; perm.len()];
        for (item, a) in items.iter().zip(appended.iter_mut()) {
            let mut count = 0usize;
            for m in parsed.mentions_of(item.id) {
                if m.kind == MentionKind::Append {
                    *a = Some(m.span);
                    count += 1;
                }
            }
            if count > 1 {
                *a = None;
            }
        }
        let mut edits: Vec<PathEdit> = Vec::new();
        for (to, &from) in perm.iter().enumerate() {
            if from == to {
                continue;
            }
            for m in parsed.mentions_of(items[from].id) {
                if m.kind == MentionKind::Index {
                    edits.push(PathEdit { span: m.span, text: format!("{}", to).into_bytes() });
                }
            }
            let Some(own) = appended[from] else { continue };
            let Some(slot) = appended[to] else { continue };
            let text = &source[slot.start..slot.end];
            if &source[own.start..own.end] != text {
                edits.push(padded(source, own, text));
            }
        }
        edits.sort_by_key(|e| e.span.start);
        Ok(edits)
    }

    /// Splice a block container's region so the entries indexed by `order`
    /// (in document order) come first, then the rest in original order.
    /// `blocks` must be in document order and tile `[region_start, region_end)`.
    fn reorder_blocks(&mut self, region_start: usize, region_end: usize, blocks: &[Block], order: &[usize]) -> Result<()> {
        let perm = full_order(order, blocks.len());
        let syn = self.syntax();
        let source = &self.source[..];
        let mut out = Vec::new();
        for &i in &perm {
            append_block_sep(&mut out, &source[blocks[i].start..blocks[i].end], &syn);
        }
        self.replace_at_span(Span::new(region_start, region_end), &out)
    }

    /// Splice a flow sequence (`[a, b, …]`) so its items follow `order`,
    /// reusing each slot's original separator bytes so the comma/space
    /// framing is preserved while only the item contents move.
    fn reorder_flow_items(&mut self, items: &[Span], order: &[usize]) -> Result<()> {
        let perm = full_order(order, items.len());
        let source = &self.source[..];
        let mut out = Vec::new();
        for (slot, &src_idx) in perm.iter().enumerate() {
            out.extend_from_slice(&source[items[src_idx].start..items[src_idx].end]);
            // Reuse the separator that originally sat after position `slot`.
            if slot + 1 < perm.len() {
                out.extend_from_slice(&source[items[slot].end..items[slot + 1].start]);
            }
        }
        self.replace_at_span(Span::new(items[0].start, items[items.len() - 1].end), &out)
    }

    /// Splice a flow mapping's members (`entries`, in document order) so
    /// they follow `order` (bring-to-front indices, as `reorder_blocks`).
    /// Also a separated sequence's items: `sep` is the token between
    /// two of them (`flow_entry_sep`, or the sequence's own).
    ///
    /// Each slot keeps the separator bytes that followed it, so the comma
    /// count and the layout stay where they were. A member's own comments
    /// travel with it: the comment block on the lines above it, when it
    /// begins its line, and the comment that closes its line after its
    /// comma — the second only when every member starts a line of its own.
    fn reorder_flow_entries(&mut self, parsed: &Document, entries: &[Node<'_>], order: &[usize], sep: &[u8]) -> Result<()> {
        let n = entries.len();
        let perm = full_order(order, n);
        let syn = self.syntax();
        let source = &self.source[..];
        let has_comments = self.line_comment_marker().is_some();

        // `body` is the member with its leading comment block; `trail` the
        // comment closing its line, with the spaces before it (empty when
        // it has none).
        #[derive(Clone, Copy)]
        struct Member {
            body: Span,
            trail: Span,
        }
        let mut members: Vec<Member> = Vec::with_capacity(n);
        let mut prev_end = 0usize;
        for entry in entries {
            let span = parsed.span(entry);
            // Back over a key sigil the span leaves out (ZON's `.`), as
            // `delete_key` does, so `.name` moves as a unit.
            let mut start = match syn.key_sigil {
                Some(sigil) if is_keyvalue(entry) && span.start > 0 && source[span.start - 1] == sigil => span.start - 1,
                _ => span.start,
            };
            if has_comments {
                let line_start = line_start_before(source, start);
                if first_non_space(source, line_start) == start {
                    let cbs = comment_block_start(source, line_start, syn.comments);
                    let lead = first_non_space(source, cbs);
                    if cbs < line_start && lead >= prev_end {
                        start = lead;
                    }
                }
            }
            let m = Member {
                body: Span::new(start, span.end),
                trail: self.line_end_comment(span.end, sep).unwrap_or(Span::new(span.end, span.end)),
            };
            prev_end = m.body.end.max(m.trail.end);
            members.push(m);
        }
        // Trails ride only when every gap between members crosses a line.
        for i in 0..n.saturating_sub(1) {
            let m = members[i];
            let next = members[i + 1];
            let gap_from = m.body.end.max(m.trail.end);
            let gap = source.get(gap_from..next.body.start).unwrap_or(b"");
            if find_byte(gap, b'\n').is_none() {
                for each in members.iter_mut() {
                    each.trail = Span::new(each.body.end, each.body.end);
                }
                break;
            }
        }

        // The region runs from the first member to the end of the last
        // member's trail, or else of the trailing separator a JSON5 or
        // TOML 1.1 member may carry, so the slot keeps it and a trail
        // landing there goes after it. Past that (the closer) is untouched.
        let last = members[n - 1];
        let region_end = if !last.trail.is_empty() {
            last.trail.end
        } else {
            let i = first_non_space(source, last.body.end);
            if !sep.is_empty() && source[i..].starts_with(sep) { i + sep.len() } else { last.body.end }
        };
        let region = Span::new(members[0].body.start, region_end);
        let mut out = Vec::new();
        for (slot, &src_idx) in perm.iter().enumerate() {
            let at = members[slot];
            let moved = members[src_idx];
            let gap_end = if slot + 1 < n { members[slot + 1].body.start } else { region.end };
            // Split the slot's own gap where its trail sat, or else at the
            // end of its line: before the split is the separator, after it
            // the line break and the next member's indent.
            let (pre, post): (&[u8], &[u8]) = if !at.trail.is_empty() {
                (&source[at.body.end..at.trail.start], &source[at.trail.end..gap_end])
            } else {
                let gap = &source[at.body.end..gap_end];
                let mut split = find_byte(gap, b'\n').unwrap_or(gap.len());
                if split > 0 && gap[split - 1] == b'\r' {
                    split -= 1;
                }
                (&gap[..split], &gap[split..])
            };
            out.extend_from_slice(&source[moved.body.start..moved.body.end]);
            if !moved.trail.is_empty() {
                out.extend_from_slice(trim_end(pre, b" \t"));
                out.extend_from_slice(&source[moved.trail.start..moved.trail.end]);
            } else {
                out.extend_from_slice(pre);
            }
            out.extend_from_slice(post);
        }
        self.replace_at_span(region, &out)
    }

    /// Move the block `[src_start, src_end)` so it begins at `dest_start`,
    /// preserving the bytes between source and destination. No-op when the
    /// destination falls within the source block.
    fn move_block(&mut self, src_start: usize, src_end: usize, dest_start: usize) -> Result<()> {
        if dest_start >= src_start && dest_start <= src_end {
            return Ok(());
        }
        let syn = self.syntax();
        let source = &self.source[..];
        let moved = &source[src_start..src_end];
        let mut out = Vec::new();
        if src_end <= dest_start {
            // src precedes dest: [src][between][dest..] -> [between][src][dest..]
            append_block_sep(&mut out, &source[src_end..dest_start], &syn);
            append_block_sep(&mut out, moved, &syn);
            // `dest` is not spliced, but now follows `src`'s last line.
            let end = continuation_end(&out, &source[dest_start..], &syn);
            out.extend_from_slice(end);
            self.replace_at_span(Span::new(src_start, dest_start), &out)
        } else {
            // dest precedes src: [dest..][between][src] -> [src][dest..][between]
            append_block_sep(&mut out, moved, &syn);
            append_block_sep(&mut out, &source[dest_start..src_start], &syn);
            self.replace_at_span(Span::new(dest_start, src_end), &out)
        }
    }

    // --- insert helpers (build text, then splice) ---

    /// Insert `key_text<kv_sep>value_text` as a new entry in the block
    /// (non-flow) mapping `mapping`, after its last existing entry written
    /// on the mapping's OWN lines (or, if it has none, at a
    /// language-appropriate fallback point).
    ///
    /// A child reached through a header line of its own (`[a.b]` under
    /// `[a]`, a `[[x]]` element) sits outside the mapping's region, and an
    /// entry spliced after it would be silently reparented; a dotted child
    /// or a fig header at the parent's depth is on the parent's lines and
    /// anchors the insert like any scalar.
    pub fn insert_block_key(&mut self, parsed: &Document, mapping: Node<'_>, key_text: &[u8], value_text: &[u8]) -> Result<()> {
        let mut maybe_last: Option<Node<'_>> = None;
        let mut first_key: Option<Node<'_>> = None;
        let mut skipped = false;
        let mut cur = parsed.ast.child(&mapping)?;
        while let Some(kv) = cur {
            cur = parsed.ast.next(&kv);
            if self.out_of_region(parsed, kv) {
                skipped = true;
                continue;
            }
            if first_key.is_none() {
                first_key = Some(match kv.kind {
                    NodeKind::KeyValue(e) => parsed.ast.node(e.key),
                    _ => kv,
                });
            }
            maybe_last = Some(kv);
        }
        let syn = self.syntax();
        let place = place_of(parsed, mapping);
        let rendered = self.rendered_value(value_text, place)?;
        // The new entry copies the first in-region key's line prefix (see
        // `indent_at`); a mapping with no such key starts at column 0.
        let indent: Vec<u8> = match first_key {
            Some(key_node) => self.indent_at(parsed.span(&key_node).start),
            None => Vec::new(),
        };
        let mut out = Vec::new();
        let insert_at = if let Some(last) = maybe_last {
            line_end_after(&self.source, parsed.span(&last).end.saturating_sub(1))
        } else if let Some(closed) = syn.closed_containers {
            // A childless closed container (`<dict/>`, root or nested):
            // rewrite it in its multi-line form around the new entry.
            let span = parsed.span(&mapping);
            let base = self.indent_at(span.start);
            let mut child = base.clone();
            child.extend_from_slice(syn.indent_unit);
            self.write_entry(&mut out, &child, key_text, &rendered, place)?;
            return self.expand_empty_container(span, closed.map, &base, &out);
        } else if mapping.id == parsed.ast.root {
            // A root with every child under a header of its own (a
            // header-first TOML or INI file) takes its first own entry at
            // the top, above the first header; an empty (or comments-only)
            // document takes it at the end of whatever is there. The top is
            // past a leading BOM.
            if skipped { text_start(&self.source) } else { self.source.len() }
        } else if parsed.is_section(&mapping) {
            // A SECTION with no entry on its own lines: its span is
            // anchored at just the header's name token, so anchor on the
            // end of a header LINE of its own instead.
            self.own_header_line_end(parsed, mapping)?
        } else {
            // A childless block mapping with no closing token has no line
            // to splice after and no spelling for its first entry.
            return Err(err!(EmptyInlineContainer));
        };

        // A container that closes on its last entry's line has no line of
        // its own to append after. See `closes_on_last_line`.
        if let (Some(closed), Some(last)) = (syn.closed_containers, maybe_last) {
            if closes_on_last_line(&self.source, parsed.span(&mapping), parsed.span(&last), closed.map.close, insert_at) {
                return Err(err!(ContainerClosesOnItsLine));
            }
        }

        if !at_line_start(&self.source, insert_at) {
            out.push(b'\n');
        }
        out.extend_from_slice(&indent);
        self.write_entry(&mut out, &indent, key_text, &rendered, place)?;
        out.push(b'\n');
        self.replace_at_span(Span::new(insert_at, insert_at), &out)
    }

    // --- Whole-container structural editing (section formats) ---
    //
    // The ops for a format whose logical containers are SCATTERED through
    // the source (TOML's `[table]` headers, fig's `>` marker runs and
    // re-entered headers, INI's reopened `[section]`): there is no single
    // `[min,max)` range to splice. What there is instead is a DERIVED region
    // set — `editor/regions.rs`'s `gather` — built from the one fact the
    // parser records that spans cannot carry: each section node's header
    // lines (`Document::regions_of`).
    //
    // In Zig a format that is not a section format cannot instantiate these
    // ops (a `@compileError`); here they answer `UnsupportedFormat`
    // (`has_container_op`), which is what the C ABI turns the compile-time
    // gate into.

    /// The section node at `path`, or `NotASection` — for a scalar, a flow
    /// container, the root, or a path that resolves to no section.
    fn section_at<'a>(&self, parsed: &'a Document, path: &[PathSegment<'_>]) -> Result<Node<'a>> {
        let node = parsed.ast.get_val_by_path(path)?;
        if !parsed.is_section(&node) {
            return Err(refuse(SectionOp::NotASection));
        }
        Ok(node)
    }

    /// The coalesced region set of the section node `node`: its header
    /// lines (creating line and every re-entry, each with its owned
    /// comment block) plus every line of its subtree, in one ascending,
    /// disjoint list. `merge_touching = false` keeps two regions that meet
    /// exactly apart, for a caller that addresses each region's own start.
    pub fn gather_regions(&self, parsed: &Document, node: Node<'_>, merge_touching: bool) -> Vec<Region> {
        regions::gather_normalized(parsed, &self.source, node, self.syntax().comments, merge_touching)
    }

    /// Byte offset just past the last line of the section node `node`'s
    /// subtree — where a new sibling section can be spliced without
    /// splitting it.
    pub fn section_extent_end(&self, parsed: &Document, node: Node<'_>) -> usize {
        regions::extent_end(parsed, &self.source, node, self.syntax().comments)
    }

    /// Whether this format has the whole-container op `op` (Zig's comptime
    /// `hasContainerOp`): `insertContainer` and `appendContainerToSeq` by a
    /// format declaring `section_header` (and its sequence form), every
    /// other op by a section format. `op` is the Zig method name.
    pub fn has_container_op(&self, op: &str) -> bool {
        match op {
            "insertContainer" => self.has_section_header,
            "appendContainerToSeq" => self.has_seq_header,
            _ => self.is_section_format,
        }
    }

    /// `has_container_op` for this editor's own dialect, asked at the
    /// call: whether the dialect declares sections (and, for the two ops
    /// that write a header, a header to write).
    pub fn dialect_has_container_op(&self, op: &str) -> bool {
        if !self.has_container_op(op) {
            return false;
        }
        let syn = self.syntax();
        if syn.section_noun.is_none() {
            return false;
        }
        match op {
            "insertContainer" => syn.section_header.is_some(),
            "appendContainerToSeq" => syn.section_header.is_some_and(|h| h.seq_open.is_some()),
            _ => true,
        }
    }

    /// The refusal for a whole-container op the format does not have —
    /// Zig's `requireSectionFormat`/`requireHeaderSyntax`, a compile
    /// error there.
    fn require_container_op(&self, op: &str) -> Result<()> {
        if self.has_container_op(op) { Ok(()) } else { Err(err!(UnsupportedFormat)) }
    }

    /// Delete the whole container named by `path` — every scattered region
    /// of its subtree, leaving interleaved foreign content untouched. For
    /// TOML that is a table, array-of-tables, or single AoT element; for
    /// fig a block container (a path may end in an index, deleting one
    /// sequence element entire); for INI a `[section]`, reopened
    /// occurrences included. A scalar or flow-valued target is refused
    /// (`NotASection`): `delete_key`/`remove_seq_item` is the op for those.
    ///
    /// An entry of the section written more than once goes at every
    /// spelling first (`repeated_entries`), and the whole is one edit.
    pub fn delete_container(&mut self, path: &[PathSegment<'_>]) -> Result<()> {
        self.require_container_op("deleteContainer")?;
        let repeated = self.repeated_entries(path)?;
        if repeated.is_empty() {
            return self.delete_section(path);
        }
        let backup = self.source.clone();
        for name in &repeated {
            let mut child: Vec<PathSegment<'_>> = path.to_vec();
            child.push(PathSegment::Key(name));
            if let Err(e) = self.delete_key(&child) {
                self.restore_source(&backup)?;
                return Err(e);
            }
        }
        if let Err(e) = self.delete_section(path) {
            self.restore_source(&backup)?;
            return Err(e);
        }
        Ok(())
    }

    /// The names of the section at `path`'s entries that are written
    /// more than once (`spelled_again`). The gather finds an entry's lines
    /// through its span, which is one spelling's, so a spelling it left
    /// would read on under whatever comes before it once the headers go.
    fn repeated_entries(&self, path: &[PathSegment<'_>]) -> Result<Vec<Vec<u8>>> {
        let mut names = Vec::new();
        let parsed = self.parsed()?;
        let Ok(node) = parsed.ast.get_val_by_path(path) else { return Ok(names) };
        let NodeKind::Mapping(first) = node.kind else { return Ok(names) };
        if !parsed.is_section(&node) {
            return Ok(names);
        }
        let mut cur = first;
        while let Some(id) = cur {
            let n = parsed.ast.node(id);
            cur = n.next_sibling;
            let NodeKind::KeyValue(kv) = n.kind else { continue };
            let NodeKind::String(name) = parsed.ast.node(kv.key).kind else { continue };
            if !value_past_entry(&parsed, n) {
                continue;
            }
            names.push(name.to_vec());
        }
        Ok(names)
    }

    /// `delete_container`, once each entry of the section is written once.
    fn delete_section(&mut self, path: &[PathSegment<'_>]) -> Result<()> {
        let parsed = self.parsed()?;
        let node = self.section_at(&parsed, path)?;
        // A list's only item leaves the list, emptied, as
        // `remove_seq_item` leaves it; the items after any other keep
        // what addresses them (`following_edits`).
        let mut edits: Vec<PathEdit> = Vec::new();
        if let Some(p) = parsed.ast.parent_of(node.id) {
            let seq = parsed.ast.node(p);
            if matches!(seq.kind, NodeKind::Sequence(Some(f)) if f == node.id) && parsed.ast.next(&node).is_none() {
                if self.emptied_section_seq(&parsed, seq)? {
                    return Ok(());
                }
                if self.emptied_header_seq(&parsed, seq, &path[..path.len() - 1])? {
                    return Ok(());
                }
            }
            if is_sequence(&seq) {
                let mut at = 0usize;
                let mut sib = parsed.ast.child(&seq)?;
                while let Some(c) = sib {
                    if c.id == node.id {
                        break;
                    }
                    at += 1;
                    sib = parsed.ast.next(&c);
                }
                edits = self.following_edits(&parsed, node, at);
            }
        }
        // A table made by dotted keys inside a flow mapping is written
        // by entries of that mapping, not by lines. Its entries go, each
        // with a separator, and the delete is held to what one promises.
        if let Some(owner) = self.dotted_in_flow(&parsed, node) {
            let mut entries: Vec<Span> = Vec::new();
            self.dotted_entry_spans(&parsed, owner, node, &mut entries)?;
            entries.sort_by(|a, b| b.start.cmp(&a.start));
            let first = flow_first_entry_start(&parsed, owner.flow);
            let removed = self.entry_index(path);
            let before = self.take_before();
            // Last first, so the spans before each splice still stand.
            for entry in &entries {
                if let Err(e) = self.remove_flow_entry(*entry, entry.start <= first) {
                    self.restore_source(&before.bytes)?;
                    return Err(e);
                }
            }
            if let Some(at) = removed {
                self.keep_others(&before, &path[..path.len() - 1], at)?;
            }
            return Ok(());
        }
        let used = self.gather_regions(&parsed, node, true);
        self.splice_out_edited(&used, &edits)
    }

    /// Splice `rendered` over every one of `mentions` (sorted by start,
    /// as `Document::mentions_of` returns them) in one whole-document
    /// rewrite, so a rename either lands everywhere or rolls back.
    fn rewrite_mentions(&mut self, mentions: MentionIterator<'_>, rendered: &[u8]) -> Result<()> {
        let source = &self.source[..];
        let mut out = Vec::with_capacity(source.len());
        let mut pos = 0usize;
        for m in mentions {
            if !m.kind.is_name() {
                continue; // an item's path, not a name
            }
            if m.span.start < pos {
                continue; // the same token reached twice
            }
            out.extend_from_slice(&source[pos..m.span.start]);
            out.extend_from_slice(rendered);
            pos = m.span.end;
        }
        out.extend_from_slice(&source[pos..]);
        let len = source.len();
        self.replace_at_span(Span::new(0, len), &out)
    }

    /// Render `path` as a header's dotted path: each key through
    /// `format_insert_key`, joined by the header's `sep`, index segments
    /// left out when the header says so.
    fn write_header_path(&self, out: &mut Vec<u8>, hdr: SectionHeader, path: &[PathSegment<'_>]) -> Result<()> {
        let mut first = true;
        for seg in path {
            match *seg {
                PathSegment::Index(i) => {
                    if !hdr.skip_index {
                        if !first {
                            out.extend_from_slice(hdr.sep);
                        }
                        first = false;
                        out.extend_from_slice(format!("{}", i).as_bytes());
                    }
                }
                PathSegment::Key(k) => {
                    if !first {
                        out.extend_from_slice(hdr.sep);
                    }
                    first = false;
                    out.extend_from_slice(&self.format_insert_key(k)?);
                }
            }
        }
        Ok(())
    }

    /// Splice a new header line (`open` + path + `close`) and `body_text`
    /// (verbatim entry lines, possibly empty) at `insert_at`, on a line
    /// of its own with a blank line before it when it does not open the
    /// file.
    fn splice_header(&mut self, insert_at: usize, open: &[u8], close: &[u8], path: &[PathSegment<'_>], body_text: &[u8]) -> Result<()> {
        let hdr = self.syntax().section_header.ok_or(err!(UnsupportedFormat))?;
        let source = &self.source[..];
        let mut out = Vec::new();
        if !at_line_start(source, insert_at) {
            out.push(b'\n');
        }
        if insert_at > text_start(source) {
            out.push(b'\n');
        }
        out.extend_from_slice(open);
        self.write_header_path(&mut out, hdr, path)?;
        out.extend_from_slice(close);
        out.push(b'\n');
        if !body_text.is_empty() {
            out.extend_from_slice(body_text);
            if body_text[body_text.len() - 1] != b'\n' {
                out.push(b'\n');
            }
        }
        self.replace_at_span(Span::new(insert_at, insert_at), &out)
    }

    /// Create a new container at `path` whose body is `body_text` (verbatim
    /// entry lines, possibly empty): a header line spelled per
    /// `Syntax.section_header`, spliced past the parent's whole derived
    /// extent — or at end-of-file for a root-level container — so no
    /// existing key is reparented. Refuses an existing target
    /// (`SectionExists`).
    pub fn insert_container(&mut self, path: &[PathSegment<'_>], body_text: &[u8]) -> Result<()> {
        self.require_container_op("insertContainer")?;
        if path.is_empty() {
            return Err(refuse(SectionOp::NotASection));
        }
        let parsed = self.parsed()?;
        if parsed.ast.get_val_by_path(path).is_ok() {
            return Err(refuse(SectionOp::Exists));
        }
        let hdr = self.syntax().section_header.ok_or(err!(UnsupportedFormat))?;
        let mut insert_at = self.source.len();
        if path.len() > 1 {
            if let Ok(parent) = parsed.ast.get_val_by_path(&path[..path.len() - 1]) {
                if is_mapping(&parent) {
                    insert_at = self.section_extent_end(&parsed, parent);
                }
            }
        }
        self.splice_header(insert_at, hdr.open, hdr.close, path, body_text)
    }

    /// Rename the leaf segment of the container at `path` to the logical
    /// key `new_leaf`, spelled by `format_insert_key` and written over every
    /// place the format spells the name (`Document::mentions_of`): the
    /// container's own header, every descendant header sharing the
    /// prefix, every dotted line, every reopening. Refuses a target that
    /// is not a section, or whose format recorded no mentions
    /// (`NotASection`).
    pub fn rename_container(&mut self, path: &[PathSegment<'_>], new_leaf: &[u8]) -> Result<()> {
        self.require_container_op("renameContainer")?;
        if path.is_empty() {
            return Err(refuse(SectionOp::NotASection));
        }
        let parsed = self.parsed()?;
        let node = self.section_at(&parsed, path)?;
        let mentions = parsed.mentions_of(node.id);
        if names_in(mentions.clone()) == 0 {
            return Err(refuse(SectionOp::NotASection));
        }
        let rendered = self.format_insert_key(new_leaf)?;
        self.rewrite_mentions(mentions, &rendered)
    }

    /// Move the container at `src_path` before the container at
    /// `dest_path` (or to EOF if `None`), re-emitting its scattered
    /// fragments contiguously, separated from surrounding content by a
    /// blank line; interleaved foreign content stays put. Both ends must
    /// be section nodes. A no-op when the destination falls inside the
    /// source's own region.
    pub fn move_container(&mut self, src_path: &[PathSegment<'_>], dest_path: Option<&[PathSegment<'_>]>) -> Result<()> {
        self.require_container_op("moveContainer")?;
        let parsed = self.parsed()?;
        let node = self.section_at(&parsed, src_path)?;
        let used = self.gather_regions(&parsed, node, true);
        // Destination: the start of the dest container's own first header
        // line (owned comment block included), or EOF.
        let dest_at = match dest_path {
            Some(dp) => {
                let dn = self.section_at(&parsed, dp)?;
                let first = parsed.regions_of(dn.id)[0];
                regions::header_line_region(&self.source, first, self.syntax().comments).start
            }
            None => self.source.len(),
        };
        let guard = self.guard_at(&parsed, parsed.ast.node(parsed.ast.root), Write::Regrouped)?;
        regions::relocate(self, &used, dest_at)?;
        self.reads_back(&guard)
    }

    /// Reorder top-level containers to the order given by `order` (their
    /// keys). Each named container's scattered fragments are removed and
    /// re-emitted contiguously, in `order`, at the position the earliest
    /// of them currently occupies. Containers not named are untouched.
    /// Each name must resolve to a section node.
    pub fn reorder_containers(&mut self, order: &[&[u8]]) -> Result<()> {
        self.require_container_op("reorderContainers")?;
        if order.is_empty() {
            return Ok(());
        }
        let parsed = self.parsed()?;
        let mut all: Vec<Region> = Vec::new();
        let mut bundles: Vec<Vec<u8>> = Vec::new();
        for &name in order {
            let path = [PathSegment::Key(name)];
            let node = self.section_at(&parsed, &path)?;
            let used = self.gather_regions(&parsed, node, true);
            bundles.push(regions::capture_bundle(&self.source, &used, &mut all));
        }
        let total = regions::normalize(&mut all, true);
        let guard = self.guard_at(&parsed, parsed.ast.node(parsed.ast.root), Write::Regrouped)?;
        let syn = self.syntax();
        regions::reorder_bundles(self, &all[..total], &bundles, &syn)?;
        self.reads_back(&guard)
    }

    /// Append a new element (body `body_text`) to the container sequence at
    /// `path` — TOML's `[[header]]` array-of-tables append. The element's
    /// header is spelled per `Syntax.section_header`'s sequence form and
    /// spliced past the current last element's whole extent, so a nested
    /// sub-table inside it is not split. A target that is not a sequence
    /// of containers is `NotAnArrayOfTables`.
    pub fn append_container_to_seq(&mut self, path: &[PathSegment<'_>], body_text: &[u8]) -> Result<()> {
        self.require_container_op("appendContainerToSeq")?;
        let parsed = self.parsed()?;
        let node = parsed.ast.get_val_by_path(path)?;
        let NodeKind::Sequence(first) = node.kind else { return Err(err!(NotAnArrayOfTables)) };
        let mut elem = first.ok_or(err!(NotAnArrayOfTables))?;
        let mut last_elem;
        loop {
            if !is_mapping(&parsed.ast.node(elem)) {
                return Err(err!(NotAnArrayOfTables));
            }
            last_elem = elem;
            match parsed.ast.node(elem).next_sibling {
                Some(n) => elem = n,
                None => break,
            }
        }
        let hdr = self.syntax().section_header.ok_or(err!(UnsupportedFormat))?;
        let seq_open = hdr.seq_open.ok_or(err!(NotAnArrayOfTables))?;
        let insert_at = self.section_extent_end(&parsed, parsed.ast.node(last_elem));
        self.splice_header(insert_at, seq_open, hdr.seq_close.unwrap_or(b""), path, body_text)
    }

    /// Classify `value_text` for the framing decisions (see `ValueShape`).
    fn value_shape(&self, value_text: &[u8]) -> ValueShape {
        let v = strip_trailing_newline(value_text);
        let nl = find_byte(v, b'\n');
        let first_line = trim_start(match nl {
            Some(i) => &v[..i],
            None => v,
        }, b" ");
        if first_line.is_empty() {
            return ValueShape::Inline;
        }
        if first_line[0] == b'|' || first_line[0] == b'>' {
            return ValueShape::BlockScalar;
        }
        // A block sequence is recognizable even on a single line (`- a`); it
        // must still descend, since `key: - a` is invalid.
        if first_line.starts_with(b"- ") || first_line == b"-" {
            return ValueShape::BlockSeq;
        }
        if nl.is_some() {
            return ValueShape::BlockMap;
        }
        if self.single_line_is_block_mapping(first_line) { ValueShape::BlockMap } else { ValueShape::Inline }
    }

    /// Whether single-line `text` is a block MAPPING entry (`k: v`) rather
    /// than a scalar — the one shape that cannot be told apart by sniffing,
    /// so it is read by the language's own parser. Only asked of a format
    /// that declares `single_line_block_mapping` (YAML alone today).
    fn single_line_is_block_mapping(&self, text: &[u8]) -> bool {
        if !self.syntax().single_line_block_mapping {
            return false;
        }
        // A flow container also parses as a mapping/sequence but must stay
        // inline; a quoted scalar parses as a string.
        if matches!(text[0], b'{' | b'[' | b'"' | b'\'') {
            return false;
        }
        let Ok(doc) = self.lang.parse(text, self.format) else { return false };
        is_mapping(&doc.ast.node(doc.ast.root))
    }

    /// Append `: value` for a mapping entry whose key is already written
    /// after `indent`. Scalars and block scalars stay inline (`key: value`);
    /// a block collection goes on the following lines, indented (a nested
    /// mapping one `indent_unit` deeper, an indentless sequence at the
    /// key's own column).
    pub fn write_map_value(&self, out: &mut Vec<u8>, indent: &[u8], value_text: &[u8]) -> Result<()> {
        let v = strip_trailing_newline(value_text);
        let shape = self.value_shape(v);
        let is_seq = shape == ValueShape::BlockSeq;
        // No value at all — a bare `key:`. The separator's padding is
        // trimmed so the line doesn't end in whitespace.
        if v.is_empty() {
            out.extend_from_slice(trim_end(self.kv_sep()?, b" "));
            return Ok(());
        }
        if shape == ValueShape::Inline || shape == ValueShape::BlockScalar {
            out.extend_from_slice(self.kv_sep()?);
            reindent_into(out, v, indent);
            return Ok(());
        }
        // Block collection value: descend onto the next lines. The literal
        // `:` below is that block-mapping syntax, not `kv_sep`.
        let mut child = indent.to_vec();
        if !is_seq {
            child.extend_from_slice(self.syntax().indent_unit);
        }
        out.push(b':');
        for line in v.split(|&c| c == b'\n') {
            out.push(b'\n');
            if !line.is_empty() {
                out.extend_from_slice(&child);
            }
            out.extend_from_slice(line);
        }
        Ok(())
    }

    /// Splice a new block-sequence item line at `insert_at`, shaped like
    /// the item introduced at `sibling_marker`: that item's line prefix
    /// (`indent_at`), then the item as `write_item` spells it.
    fn insert_seq_line(&mut self, insert_at: usize, sibling_marker: usize, value_text: &[u8], place: Place<'_>) -> Result<()> {
        let indent = self.indent_at(sibling_marker);
        let rendered = self.rendered_value(value_text, place)?;
        let mut out = Vec::new();
        if !at_line_start(&self.source, insert_at) {
            out.push(b'\n');
        }
        out.extend_from_slice(&indent);
        self.write_item(&mut out, &indent, &rendered, place)?;
        out.push(b'\n');
        self.replace_at_span(Span::new(insert_at, insert_at), &out)
    }

    fn promote_null_to_mapping(
        &mut self,
        null_span: Span,
        is_root: bool,
        key_text: &[u8],
        value_text: &[u8],
        place: Place<'_>,
    ) -> Result<()> {
        let mut out = Vec::new();
        // A format with no bare `key: value` document form (ZON) promotes a
        // `null` value in place to a flow container, root or nested alike.
        let syn = self.syntax();
        if !syn.bare_document_mapping {
            out.extend_from_slice(syn.flow_map_open);
            out.push(b' ');
            out.extend_from_slice(key_text);
            out.extend_from_slice(self.kv_sep()?);
            out.extend_from_slice(value_text);
            out.push(b' ');
            out.extend_from_slice(syn.flow_map_close);
            return self.replace_at_span(null_span, &out);
        }
        // `write_map_value` emits the separator itself, so a block value
        // (`- a`, `k: v`) descends onto the following lines instead of being
        // jammed inline after the `:` — where it would not parse.
        let rendered = self.rendered_value(value_text, place)?;
        if is_root {
            // Empty document: the whole source becomes a single entry.
            self.write_entry(&mut out, b"", key_text, &rendered, place)?;
            out.push(b'\n');
            let len = self.source.len();
            return self.replace_at_span(Span::new(0, len), &out);
        }
        // The promoted mapping's first entry sits one `indent_unit` under
        // the key that owned the null: that key's line prefix plus one.
        let line_start = line_start_before(&self.source, null_span.start);
        let mut child = self.indent_at(first_non_space(&self.source, line_start));
        child.extend_from_slice(syn.indent_unit);
        out.push(b'\n');
        out.extend_from_slice(&child);
        self.write_entry(&mut out, &child, key_text, &rendered, place)?;
        self.replace_at_span(null_span, &out)
    }

    /// Reject `value_text` that has no flow spelling before it is spliced
    /// into a `{…}`/`[…]` container (`BlockValueIntoFlow`). A flow container
    /// holds one line of comma-separated members: a block sequence (`- a`),
    /// a block mapping (`k: v`), a block scalar (`|`), or anything
    /// multi-line means something else entirely once wrapped in braces —
    /// and a lenient reader may well accept `{b: - a}` as the STRING
    /// `"- a"`, so the reparse cannot be the net.
    fn require_flow_value(&self, value_text: &[u8]) -> Result<()> {
        if self.value_shape(value_text) != ValueShape::Inline {
            return Err(err!(BlockValueIntoFlow));
        }
        Ok(())
    }

    /// Whether the flow container `node` is laid out one member per line:
    /// its closer is separated from the last member (ending at `last_end`)
    /// by a newline — or, for an open container, which has no closer, its
    /// first and last members are on different lines.
    fn flow_is_multi_line(&self, parsed: &Document, node: Node<'_>, span: Span, last_end: usize) -> Result<bool> {
        let source = &self.source[..];
        if self.is_open(parsed, node) {
            let first = present(parsed.ast.child(&node)?)?;
            let from = parsed.span(&first).start;
            return Ok(find_byte(source.get(from..last_end).unwrap_or(b""), b'\n').is_some());
        }
        let close = span.end.saturating_sub(1); // the '}' or ']'
        Ok(find_byte(source.get(last_end..close).unwrap_or(b""), b'\n').is_some())
    }

    /// Insert a `key: value` entry into a brace-delimited (flow) mapping,
    /// matching its layout. A pretty-printed mapping — one whose closing `}`
    /// sits on its own line below the members — gets the new entry on its
    /// own line, indented to match the existing members. A compact
    /// single-line mapping keeps the inline `", key: value"` style.
    fn insert_flow_map_entry(
        &mut self,
        parsed: &Document,
        node: Node<'_>,
        span: Span,
        non_empty: bool,
        key_text: &[u8],
        value_text: &[u8],
    ) -> Result<()> {
        self.require_flow_value(value_text)?;
        if !non_empty {
            return self.insert_flow_entry(parsed, node, span, key_text, value_text);
        }
        let source = &self.source[..];
        let syn = self.syntax();
        // The member written last, which a table made by dotted keys may
        // be though it is not the last child.
        let mut last_end = 0usize;
        let mut each = parsed.ast.child(&node)?;
        while let Some(kv) = each {
            last_end = last_end.max(self.flow_entry_end(parsed, kv)?);
            each = parsed.ast.next(&kv);
        }
        // The separator: `kv_sep`, or the bytes the first entry writes
        // between its key and value for a format whose flow objects fix
        // their own mode. See `Syntax.flow_kv_sep_from_siblings`.
        let sep: &[u8] = if syn.flow_kv_sep_from_siblings {
            let first_id = node.kind.as_mapping().flatten().ok_or(err!(NotFound))?;
            let first = first_written_leaf(parsed, parsed.ast.node(first_id));
            let kv = kv_of(&first);
            let key_end = parsed.span(&parsed.ast.node(kv.key)).end;
            let value_start = parsed.span(&parsed.ast.node(kv.value)).start;
            &source[key_end..value_start]
        } else {
            self.kv_sep()?
        };
        let joint = self.flow_map_joint(parsed, node, sep)?;
        let mut out = Vec::new();
        // Multi-line layout: the closing brace is separated from the last
        // member by a newline. Splice after the last member's value so the
        // new entry lands on its own line, not jammed before the brace.
        if self.flow_is_multi_line(parsed, node, span, last_end)? {
            // Column of the key's line, not the key node's own span start:
            // for ZON the key span covers only the bare identifier after
            // its leading `.`.
            let key_node = present(parsed.ast.first_child_key(&node)?)?;
            let col = column_of(source, first_non_space(source, line_start_before(source, parsed.span(&key_node).start)));
            out.extend_from_slice(joint.token);
            out.push(b'\n');
            out.resize(out.len() + col, b' ');
            out.extend_from_slice(key_text);
            out.extend_from_slice(sep);
            out.extend_from_slice(value_text);
            return self.replace_at_span(Span::new(last_end, last_end), &out);
        }
        // Single-line layout: splice right after the last member's own
        // value — NOT right before the closing brace, which would land
        // inside any padding space before `}`.
        out.extend_from_slice(joint.lead);
        out.extend_from_slice(joint.token);
        out.extend_from_slice(joint.trail);
        out.extend_from_slice(key_text);
        out.extend_from_slice(sep);
        out.extend_from_slice(value_text);
        self.replace_at_span(Span::new(last_end, last_end), &out)
    }

    /// The bytes an insert writes between the last member of the
    /// non-empty flow mapping `node` and a new one on its line, the new
    /// one's key and value parted by `kv`. A format that copies its flow
    /// separators (`Syntax.flow_kv_sep_from_siblings`) takes the bytes
    /// between the first two members when they share a line and hold
    /// nothing but spaces and at most one `flow_entry_sep`. Otherwise an
    /// object whose entries are spelled with `flow_map_kv_sep` is joined by
    /// `flow_map_entry_sep` (fig's `{a=1 b=2}`), and any other by
    /// `flow_entry_sep` and a space.
    fn flow_map_joint<'a>(&'a self, parsed: &Document, node: Node<'_>, kv: &[u8]) -> Result<Joint<'a>> {
        let source = &self.source[..];
        let syn = self.syntax();
        if syn.flow_kv_sep_from_siblings {
            let first = present(parsed.ast.child(&node)?)?;
            if let Some(second) = parsed.ast.next(&first) {
                let from = parsed.span(&first_written_leaf(parsed, first)).end;
                let to = flow_entry_start(parsed, second);
                if to > from {
                    let gap = &source[from..to];
                    let joint = Joint::split(gap, false);
                    if find_any(gap, b"\r\n").is_none()
                        && (joint.token.is_empty() || joint.token == syn.flow_entry_sep)
                    {
                        return Ok(joint);
                    }
                }
            }
        }
        let ws: &[u8] = b" \t";
        if !syn.flow_map_entry_sep.is_empty()
            && !syn.flow_map_kv_sep.is_empty()
            && trim(kv, ws) == trim(syn.flow_map_kv_sep, ws)
        {
            return Ok(Joint::split(syn.flow_map_entry_sep, false));
        }
        Ok(Joint { lead: b"", token: syn.flow_entry_sep, trail: b" ", declared: false })
    }

    /// Insert the first entry into an EMPTY flow mapping (`{}` / ZON's
    /// `.{}`): a tight `{key: value}` splice, padded by `flow_map_pad`.
    fn insert_flow_entry(&mut self, parsed: &Document, node: Node<'_>, span: Span, key_text: &[u8], value_text: &[u8]) -> Result<()> {
        self.require_flow_value(value_text)?;
        let syn = self.syntax();
        let mut out = Vec::new();
        // An open root has no braces for the pad to stand inside.
        let open_root = self.is_open_root(parsed, node);
        let pad: &[u8] = if open_root { b"" } else { syn.flow_map_pad };
        out.extend_from_slice(pad);
        out.extend_from_slice(key_text);
        // A format that spells flow entries apart from block ones says
        // how: `Syntax.flow_map_kv_sep`.
        let kv = syn.flow_map_kv_sep;
        out.extend_from_slice(if !kv.is_empty() { kv } else { self.kv_sep()? });
        out.extend_from_slice(value_text);
        out.extend_from_slice(pad);
        // Just after '{' (or ZON's '.{'); an open root has no opener, and
        // its first member starts the source.
        let at = if open_root {
            first_non_space(&self.source, span.start)
        } else {
            flow_open_end(&self.source, span, syn.flow_map_open)
        };
        self.replace_at_span(Span::new(at, at), &out)
    }

    /// Insert `value_text` as the new last item of the flow sequence
    /// `node`/`span`. Splices immediately after the current last
    /// element rather than before the closing `]`, so a pre-existing
    /// trailing comma isn't doubled into an empty element — and so a
    /// separated sequence's trailing separator stays trailing. The
    /// separator before the new item is laid out as the sequence is
    /// (`write_joint`).
    fn insert_flow_item(&mut self, parsed: &Document, node: Node<'_>, span: Span, non_empty: bool, value_text: &[u8]) -> Result<()> {
        let joint = self.joint_of(parsed, node);
        if !joint.declared {
            self.require_flow_value(value_text)?;
        }
        let mut out = Vec::new();
        if non_empty {
            let last = present(parsed.ast.last_child(&node)?)?;
            let last_end = parsed.span(&last).end;
            self.write_joint(&mut out, parsed, node, joint)?;
            out.extend_from_slice(value_text);
            return self.replace_at_span(Span::new(last_end, last_end), &out);
        }
        let at = self.flow_body_start(parsed, node, span);
        self.replace_at_span(Span::new(at, at), value_text)
    }

    /// Insert `value_text` into the flow sequence `node`/`span` as the
    /// item before `before` — or, with none, as the first item of an
    /// empty one. Splices right before `before`'s own span, so any
    /// padding or indent before it stays before the new item, and
    /// `before` follows the new item after the separator `write_joint`
    /// lays out — or, when not `laid_out`, after the separator as written
    /// on one line.
    fn insert_flow_item_before(
        &mut self,
        parsed: &Document,
        node: Node<'_>,
        span: Span,
        before: Option<Node<'_>>,
        value_text: &[u8],
        laid_out: bool,
    ) -> Result<()> {
        let joint = self.joint_of(parsed, node);
        if !joint.declared {
            self.require_flow_value(value_text)?;
        }
        let mut out = value_text.to_vec();
        let at = match before {
            Some(b) => {
                if laid_out || joint.declared {
                    self.write_joint(&mut out, parsed, node, joint)?;
                } else {
                    out.extend_from_slice(joint.lead);
                    out.extend_from_slice(joint.token);
                    out.extend_from_slice(joint.trail);
                }
                parsed.span(&b).start
            }
            None => self.flow_body_start(parsed, node, span),
        };
        self.replace_at_span(Span::new(at, at), &out)
    }

    /// Where the first item of an EMPTY flow container goes: just after
    /// its opener (`[`, ZON's `.{`), or for an open one, which has none,
    /// its span's own start.
    fn flow_body_start(&self, parsed: &Document, node: Node<'_>, span: Span) -> usize {
        if self.is_open(parsed, node) {
            return first_non_space(&self.source, span.start);
        }
        flow_open_end(&self.source, span, self.syntax().flow_map_open)
    }

    /// The trailing comment that ends the line a flow member ending at
    /// `from` is on, past spaces and one `sep` (`a, // c`), as the span
    /// from the spaces before it to the line's end; `None` when anything
    /// else comes first, or follows a paired comment on its line
    /// (`/* x */ }`).
    fn line_end_comment(&self, from: usize, sep: &[u8]) -> Option<Span> {
        let marker = self.trailing_comment_marker()?;
        let source = &self.source[..];
        let mut i = first_non_space(source, from);
        if !sep.is_empty() && source[i..].starts_with(sep) {
            i = first_non_space(source, i + sep.len());
        }
        if !source[i..].starts_with(marker.open) {
            return None;
        }
        let mut line_end = find_byte_from(source, i, b'\n').unwrap_or(source.len());
        if line_end > i && source[line_end - 1] == b'\r' {
            line_end -= 1;
        }
        if !marker.close.is_empty() {
            let close = find_from(&source[..line_end], i + marker.open.len(), marker.close)?;
            let after = close + marker.close.len();
            if first_non_space(source, after) != line_end {
                return None;
            }
            line_end = after;
        }
        let mut ts = i;
        while ts > from && (source[ts - 1] == b' ' || source[ts - 1] == b'\t') {
            ts -= 1;
        }
        Some(Span::new(ts, line_end))
    }

    /// The splice for a later flow item that begins its line and has a
    /// separator after it on that line: the item and that separator,
    /// not the one before it. An item alone on its line takes the whole
    /// line, with its trailing comment; one with a sibling after it leaves
    /// that sibling where it stands. `None` when the item does not begin
    /// its line or no separator follows it there, which the backward
    /// splice handles.
    fn own_line_flow_item(&self, item_span: Span, sep: &[u8], limit: Span) -> Option<Span> {
        if sep.is_empty() {
            return None;
        }
        let source = &self.source[..limit.end];
        let line_start = line_start_before(source, item_span.start);
        if line_start < limit.start {
            return None;
        }
        if source[line_start..item_span.start].iter().any(|&c| c != b' ' && c != b'\t') {
            return None;
        }
        let mut e = first_non_space(source, item_span.end);
        if !source[e..].starts_with(sep) {
            return None;
        }
        e = first_non_space(source, e + sep.len());
        if let Some(trail) = self.line_end_comment(e, b"") {
            e = trail.end;
        }
        if e >= source.len() || source[e] == b'\n' || source[e] == b'\r' {
            return Some(Span::new(line_start, line_end_after(source, e)));
        }
        Some(Span::new(first_non_space(source, item_span.start), e))
    }

    /// Delete the flow item or member at `item_span` and exactly one
    /// separator beside it: the one after it when it is first, else the
    /// one before it, so the last of `a, b,` leaves `a,` and keeps a
    /// trailing separator trailing. `sep` is the token the separator
    /// is spelled with (`,`, a separated sequence's `|`), or empty for
    /// items joined by whitespace alone. Nothing outside `limit` is taken.
    fn remove_flow_item(&mut self, item_span: Span, is_first: bool, sep: &[u8], limit: Span) -> Result<()> {
        let source = &self.source[..limit.end];
        if is_first {
            // Drop the item and a following ", " if present, consuming
            // forward through trailing whitespace *including newlines*.
            let mut e = item_span.end;
            while e < source.len() && is_flow_item_ws(source[e]) {
                e += 1;
            }
            if !sep.is_empty() && source[e..].starts_with(sep) {
                e += sep.len();
                while e < source.len() && is_flow_item_ws(source[e]) {
                    e += 1;
                }
            }
            self.replace_at_span(Span::new(item_span.start, e), b"")
        } else if let Some(own) = self.own_line_flow_item(item_span, sep, limit) {
            self.replace_at_span(own, b"")
        } else {
            // Drop a preceding ", " and the item, scanning backward across
            // newlines to find the *previous* item's separator.
            let mut s = item_span.start;
            while s > limit.start && is_flow_item_ws(source[s - 1]) {
                s -= 1;
            }
            if !sep.is_empty() && source[limit.start..s].ends_with(sep) {
                s -= sep.len();
                while s > limit.start && is_flow_item_ws(source[s - 1]) {
                    s -= 1;
                }
            }
            self.replace_at_span(Span::new(s, item_span.end), b"")
        }
    }

    /// Replace a span of bytes with a new span of bytes. Not aware of the
    /// format; the document is stale until reparsed.
    fn replace_source(&mut self, old_span: Span, text: &[u8]) -> Result<()> {
        if old_span.end < old_span.start || old_span.end > self.source.len() {
            return Err(err!(InvalidSpan));
        }
        self.source.splice(old_span.start..old_span.end, text.iter().copied());
        Ok(())
    }

    /// After an edit, restores the document so node spans are valid again.
    fn reparse(&mut self) -> Result<()> {
        let parsed = self.parse_source()?;
        self.document = Some(Arc::new(parsed));
        Ok(())
    }

    fn parse_source(&self) -> Result<Document> {
        self.lang.parse(&self.source, self.format)
    }
}

// ======================
// SOURCE-COORDINATE UTILS
// ======================
//
// The shared ones live in `editor/splice.rs`; what remains here is used by
// the engine alone.

/// Start of a mapping entry's full block: its owned leading comment block
/// (`comment_block_start`) at the start of the key's line.
fn entry_block_start(source: &[u8], kv_span: Span, comments: Comments) -> usize {
    comment_block_start(source, line_start_before(source, kv_span.start), comments)
}

/// End of a mapping entry's full block: just past the newline ending its last
/// line (or `source.len()` when the final line is unterminated).
fn entry_block_end(source: &[u8], kv_span: Span) -> usize {
    line_end_after(source, kv_span.end.saturating_sub(1))
}

/// Strip a leading line-comment `marker` (and one following space) from
/// `line`, the inverse of how `render_comment` emits a comment. `line` must
/// already have its leading whitespace trimmed. A line that doesn't start
/// with `marker` is returned unchanged.
fn strip_line_comment_marker(line: &[u8], marker: CommentDelimiter) -> &[u8] {
    if !line.starts_with(marker.open) {
        return line;
    }
    let mut rest = &line[marker.open.len()..];
    if !rest.is_empty() && rest[0] == b' ' {
        rest = &rest[1..];
    }
    // A paired delimiter: drop the close and the one space before it, the
    // inverse of `render_comment`.
    if !marker.close.is_empty() && rest.ends_with(marker.close) {
        rest = &rest[..rest.len() - marker.close.len()];
        if !rest.is_empty() && rest[rest.len() - 1] == b' ' {
            rest = &rest[..rest.len() - 1];
        }
    }
    rest
}

/// The column on the line starting at `line_start` where an own-line comment's
/// marker belongs — past the line's leading whitespace, and (for a format whose
/// line prefix is STRUCTURAL, `Syntax.structural_indent`) past that prefix too.
///
/// The one place `comment_out` writes a marker and the one place `uncomment*`
/// looks for it again, so the two are inverses by construction. The `*` of a
/// fig sequence item is NOT skipped: it introduces the item, so it is part of
/// what a comment-out has to hide.
fn comment_column(source: &[u8], line_start: usize, structural: bool) -> usize {
    let mut i = first_non_space(source, line_start);
    if !structural {
        return i;
    }
    while i < source.len() && source[i] == b'>' {
        i = first_non_space(source, i + 1);
    }
    i
}

/// Render one comment into `out`: `marker.open`, then a space and `line`
/// unless `line` is empty, then — for a paired delimiter — a space and
/// `marker.close`. `# text`, `<!-- text -->`, a bare `#`, `<!-- -->`.
fn render_comment(out: &mut Vec<u8>, marker: CommentDelimiter, line: &[u8]) {
    out.extend_from_slice(marker.open);
    if !line.is_empty() {
        out.push(b' ');
        out.extend_from_slice(line);
    }
    if !marker.close.is_empty() {
        out.push(b' ');
        out.extend_from_slice(marker.close);
    }
}

/// Render `text` as one or more own-line comments into `out`, each line being
/// `indent` + one `render_comment` + '\n'. A single trailing newline in `text`
/// is ignored so it never yields a stray empty comment line.
fn render_line_comments(out: &mut Vec<u8>, indent: &[u8], marker: CommentDelimiter, text: &[u8]) {
    let body = if text.ends_with(b"\n") { &text[..text.len() - 1] } else { text };
    for line in body.split(|&c| c == b'\n') {
        out.extend_from_slice(indent);
        render_comment(out, marker, line);
        out.push(b'\n');
    }
}

/// Whether a node is a leaf scalar — the kinds `set_sequence` can match by
/// value.
fn is_scalar_kind(kind: &NodeKind<'_>) -> bool {
    matches!(
        kind,
        NodeKind::Null | NodeKind::Boolean(_) | NodeKind::String(_) | NodeKind::Number(_) | NodeKind::Extended(_)
    )
}

/// Count the children of a container node.
fn seq_len(parsed: &Document, node: Node<'_>) -> Result<usize> {
    let mut n = 0usize;
    let mut maybe = parsed.ast.child(&node)?;
    while let Some(c) = maybe {
        n += 1;
        maybe = parsed.ast.next(&c);
    }
    Ok(n)
}

fn is_quote(c: u8) -> bool {
    c == b'"' || c == b'\''
}

/// Whitespace the flow-item scan may cross while hunting for the adjoining
/// comma: spaces/tabs plus newlines, so a one-item-per-line layout is treated
/// the same as a packed single-line layout.
fn is_flow_item_ws(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n'
}

/// How many direct entries of `mapping` repeat a string key an earlier entry
/// already has — 0 for a node that is not a mapping.
fn repeated_key_count(ast: &AST, mapping: Node<'_>) -> usize {
    let NodeKind::Mapping(mut entry) = mapping.kind else { return 0 };
    let mut seen: HashSet<&[u8]> = HashSet::new();
    let mut repeated = 0usize;
    while let Some(id) = entry {
        let n = ast.node(id);
        entry = n.next_sibling;
        let NodeKind::KeyValue(kv) = n.kind else { continue };
        let NodeKind::String(name) = ast.node(kv.key).kind else { continue };
        if !seen.insert(name) {
            repeated += 1;
        }
    }
    repeated
}

/// Whether `kv` is an entry whose value lies past its own span, a later
/// spelling's (`spelled_again`). A section's value is spread through the
/// file by design and never counts.
fn value_past_entry(parsed: &Document, kv: Node<'_>) -> bool {
    let NodeKind::KeyValue(e) = kv.kind else { return false };
    let value = parsed.ast.node(e.value);
    !parsed.is_section(&value) && parsed.span(&value).start > parsed.span(&kv).end
}

/// Whether `mapping` has a direct entry whose key is `name`, other than the
/// entry `except` — false for a node that is not a mapping.
fn holds_name(ast: &AST, mapping: Node<'_>, name: &[u8], except: Option<NodeId>) -> bool {
    let NodeKind::Mapping(mut entry) = mapping.kind else { return false };
    while let Some(id) = entry {
        let n = ast.node(id);
        entry = n.next_sibling;
        if except == Some(id) {
            continue;
        }
        let NodeKind::KeyValue(kv) = n.kind else { continue };
        if let NodeKind::String(s) = ast.node(kv.key).kind {
            if s == name {
                return true;
            }
        }
    }
    false
}

/// The span of the flow mapping's entry that writes `leaf`, an entry of a
/// table under the dotted table `top` (`DottedInFlow`): from the last place
/// `top`'s name is written before `leaf`'s key, which begins the entry's key
/// path, to the end of `leaf`'s value.
fn dotted_entry_span(parsed: &Document, top: Node<'_>, leaf: Node<'_>) -> Span {
    let span = parsed.span(&leaf);
    let mut start = span.start;
    for m in parsed.mentions_of(top.id) {
        if m.span.start > span.start {
            break;
        }
        start = m.span.start;
    }
    Span::new(start, span.end)
}

/// Where the first entry of the flow mapping `flow` begins: its first
/// child's span, or the first place its name is written when it is a table
/// made by dotted keys.
fn flow_first_entry_start(parsed: &Document, flow: Node<'_>) -> usize {
    match parsed.ast.child(&flow) {
        Ok(Some(first)) => flow_entry_start(parsed, first),
        _ => parsed.span(&flow).end,
    }
}

/// Where the first entry of a flow mapping that writes its child `kv`
/// begins: `kv`'s span, or the first place its name is written when it is a
/// table made by dotted keys.
fn flow_entry_start(parsed: &Document, kv: Node<'_>) -> usize {
    if let NodeKind::KeyValue(e) = kv.kind {
        let mentions = parsed.mentions_of(e.value);
        if MentionIterator::count(&mentions) > 0 {
            return mentions.at(0).span.start.min(parsed.span(&kv).start);
        }
    }
    parsed.span(&kv).start
}

/// The value of the entry `kv` when it is a table made by dotted keys,
/// written by entries of its parent rather than by a value of its own: a
/// section whose name is written on its parent's lines or in its braces
/// (TOML's), or a mapping whose span begins where its key does (fig's
/// `{a.b=1}`). `None` for any other entry.
fn dotted_value<'a>(parsed: &'a Document, kv: Node<'_>) -> Option<Node<'a>> {
    let NodeKind::KeyValue(e) = kv.kind else { return None };
    let value = parsed.ast.node(e.value);
    if !is_mapping(&value) {
        return None;
    }
    if parsed.is_section(&value) || parsed.span(&value).start <= parsed.span(&parsed.ast.node(e.key)).start {
        return Some(value);
    }
    None
}

/// The first entry written with a key and a value under the child `kv` of a
/// flow mapping: `kv`, or the first entry of the table made by dotted keys
/// that it is, and so on down.
fn first_written_leaf<'a>(parsed: &'a Document, kv: Node<'a>) -> Node<'a> {
    let mut cur = kv;
    while let Some(value) = dotted_value(parsed, cur) {
        match parsed.ast.child(&value) {
            Ok(Some(c)) => cur = c,
            _ => break,
        }
    }
    cur
}

/// Where the dotted table `table`'s name begins the entry at `at`: the last
/// place it is written up to there, or its key's span for a parser that
/// records no such place (fig's), which is right for the table's first
/// entry.
fn dotted_entry_start(parsed: &Document, table: Node<'_>, at: usize) -> usize {
    let mut start: Option<usize> = None;
    for m in parsed.mentions_of(table.id) {
        if m.span.start > at {
            break;
        }
        start = Some(m.span.start);
    }
    if let Some(s) = start {
        return s;
    }
    let Some(kv) = parsed.ast.parent_of(table.id) else { return at };
    match parsed.ast.kind_of(kv) {
        NodeKind::KeyValue(e) => at.min(parsed.span(&parsed.ast.node(e.key)).start),
        _ => at,
    }
}

/// `text` over `span`, and over the spaces after it when something follows
/// them on the line — a trailing comment — so that it stays at the column it
/// stood at, or as many spaces after `text` as were after `span` when `text`
/// reaches it.
fn padded(source: &[u8], span: Span, text: &[u8]) -> PathEdit {
    let mut end = span.end;
    while end < source.len() && source[end] == b' ' {
        end += 1;
    }
    if end == span.end || end == source.len() || source[end] == b'\n' || source[end] == b'\r' {
        return PathEdit { span, text: text.to_vec() };
    }
    let width = end - span.start;
    let pad = if width > text.len() { width - text.len() } else { end - span.end };
    let mut out = text.to_vec();
    out.resize(text.len() + pad, b' ');
    PathEdit { span: Span::new(span.start, end), text: out }
}

/// `source[from..to]` appended to `out`, with each of `edits` (ascending)
/// that lies inside it written over its span.
fn append_edited(out: &mut Vec<u8>, source: &[u8], from: usize, to: usize, edits: &[PathEdit]) {
    let mut pos = from;
    for edit in edits {
        if edit.span.start < pos || edit.span.end > to {
            continue;
        }
        out.extend_from_slice(&source[pos..edit.span.start]);
        out.extend_from_slice(&edit.text);
        pos = edit.span.end;
    }
    out.extend_from_slice(&source[pos..to]);
}

// ── "nothing else moved": the uncomment and delete guards ───────────────────
//
// `uncomment_leading`/`uncomment_dangling` strip a marker off lines the
// CALLER judged to be an entry, so the editor's half of the bargain is to
// prove the result is what was claimed: a document that parses, and that
// differs from the one before the splice only by what was ADDED to the
// container the lines sit in. A delete makes the same promise the other way
// round (`keep_others`): the document differs only by the one child REMOVED
// from its container. What reads back differently only because that child is
// gone counts as the child: its container read as empty (`reads_empty`), or
// an implicit table that held only it (`implied_only_by`).
//
// The comparison is over the two parses, not their bytes: the after-tree is
// walked to the target container along the same path the caller named, every
// node passed on the way compared whole, and only that container is allowed
// to have changed, and only in the one way.

/// Whether every child of `bn` still appears, in order and unchanged, among
/// the children of `an` — i.e. `an` differs from `bn` only by insertions.
/// Matching is greedy: each before-child claims the earliest after-child it
/// equals.
fn children_only_grew(b: &Document, bn: Node<'_>, a: &Document, an: Node<'_>) -> bool {
    let Ok(mut b_cur) = b.ast.child(&bn) else { return false };
    let Ok(mut a_cur) = a.ast.child(&an) else { return false };
    while let Some(bc) = b_cur {
        let mut matched = false;
        while let Some(ac) = a_cur {
            a_cur = a.ast.next(&ac);
            if nodes_equal_deep(b, bc.id, a, ac.id) {
                matched = true;
                break;
            }
        }
        if !matched {
            return false;
        }
        b_cur = b.ast.next(&bc);
    }
    true
}

/// Whether the children of `an` are those of `bn` with the one at `removed`
/// left out, each of the rest unchanged and in its order.
fn children_lost_one(b: &Document, bn: Node<'_>, a: &Document, an: Node<'_>, removed: usize) -> bool {
    let Ok(mut b_cur) = b.ast.child(&bn) else { return false };
    let Ok(mut a_cur) = a.ast.child(&an) else { return false };
    let mut index = 0usize;
    let mut lost = false;
    while let Some(bc) = b_cur {
        if index == removed {
            lost = true;
        } else {
            let Some(ac) = a_cur else { return false };
            if !nodes_equal_deep(b, bc.id, a, ac.id) {
                return false;
            }
            a_cur = a.ast.next(&ac);
        }
        b_cur = b.ast.next(&bc);
        index += 1;
    }
    lost && a_cur.is_none()
}

/// Structural equality of two subtrees across two parses: same kinds, same
/// scalar bytes, same children in the same order. Node ids differ between
/// the two documents, so `NodeKind::eql` (which compares child ids) cannot be
/// used for containers.
///
/// Read off the rows: a row's kind byte tells the kinds apart as finely as
/// `NodeKind::eql` does, and a leaf with text compares it.
fn nodes_equal_deep(b: &Document, bid: NodeId, a: &Document, aid: NodeId) -> bool {
    use table::Kind as K;
    let b_rows = b.ast.rows();
    let a_rows = a.ast.rows();
    let br = b_rows[bid as usize];
    let ar = a_rows[aid as usize];
    if br.kind != ar.kind {
        return false;
    }
    if br.kind == K::MAPPING || br.kind == K::SEQUENCE {
        let mut bc = br.first_child;
        let mut ac = ar.first_child;
        while bc != table::NONE {
            if ac == table::NONE {
                return false;
            }
            if !nodes_equal_deep(b, bc, a, ac) {
                return false;
            }
            bc = sibling_row(b_rows, bc);
            ac = sibling_row(a_rows, ac);
        }
        return ac == table::NONE;
    }
    if br.kind == K::KEYVALUE {
        return nodes_equal_deep(b, br.first_child, a, ar.first_child)
            && nodes_equal_deep(
                b,
                b_rows[br.first_child as usize].next_sibling,
                a,
                a_rows[ar.first_child as usize].next_sibling,
            );
    }
    if !K::has_text(br.kind) {
        return true;
    }
    b.ast.text_of(br.text).unwrap_or(b"") == a.ast.text_of(ar.text).unwrap_or(b"")
}

/// Row `id`'s next sibling, as `AST::node` reads it: none for a keyvalue's
/// key or value.
fn sibling_row(rows: &[table::Row], id: u32) -> u32 {
    let r = rows[id as usize];
    if r.next_sibling == table::NONE {
        return table::NONE;
    }
    if r.parent != table::NONE && rows[r.parent as usize].kind == table::Kind::KEYVALUE {
        return table::NONE;
    }
    r.next_sibling
}

/// The one change a guarded op may make to its container: nodes added
/// (uncomment), or the child at an index removed (delete).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Added,
    Removed(usize),
}

/// Whether `after` differs from `before` only by `change` to the container
/// at `path` (the root for an empty path). Every other node — including every
/// container passed through on the way down, and every sibling of the ones
/// that are — must be structurally identical.
fn changed_only_at(before: &Document, after: &Document, path: &[PathSegment<'_>], change: Change) -> bool {
    changed_only_under(before, before.ast.root, after, after.ast.root, path, change)
}

fn changed_only_under(b: &Document, bid: NodeId, a: &Document, aid: NodeId, path: &[PathSegment<'_>], change: Change) -> bool {
    let bn = b.ast.node(bid);
    let an = a.ast.node(aid);
    // A container emptied of its only child reads back as the format reads
    // an empty value: null (a block container, `k:`), an empty string
    // (NestedText's `-` or `k:`), or an empty container of either kind
    // (ZON's `.{}` is the one literal for both).
    if let Change::Removed(removed) = change {
        if path.is_empty() && reads_empty(a, an) && tag(&bn) != tag(&an) {
            return removed == 0 && matches!(seq_len(b, bn), Ok(1));
        }
    }
    if tag(&bn) != tag(&an) {
        return false;
    }
    if path.is_empty() {
        return match change {
            Change::Added => children_only_grew(b, bn, a, an),
            Change::Removed(at) => children_lost_one(b, bn, a, an, at),
        };
    }

    // Not the target yet: walk the two child lists in lockstep. They must
    // have the same length and pair up, with only the child the next path
    // segment names allowed to differ — and only by what is added deeper
    // down, or taken: an implicit table that held nothing but the deleted
    // entry goes with it (`implied_only_by`).
    let Ok(mut b_cur) = b.ast.child(&bn) else { return false };
    let Ok(mut a_cur) = a.ast.child(&an) else { return false };
    let mut index = 0usize;
    while let Some(bc) = b_cur {
        let next_b = b.ast.next(&bc);
        let on_path = match path[0] {
            PathSegment::Key(k) => match bc.kind {
                NodeKind::KeyValue(kv) => key_node_is(b, kv.key, k),
                _ => false,
            },
            PathSegment::Index(i) => index == i,
        };
        let paired = match a_cur {
            Some(ac) => {
                !on_path
                    || matches!(path[0], PathSegment::Index(_))
                    || match (ac.kind, path[0]) {
                        (NodeKind::KeyValue(kv), PathSegment::Key(k)) => key_node_is(a, kv.key, k),
                        _ => false,
                    }
            }
            None => false,
        };
        if !paired {
            if let Change::Removed(removed) = change {
                if on_path && implied_only_by(b, bc, &path[1..], removed) {
                    b_cur = next_b;
                    index += 1;
                    continue;
                }
            }
            return false;
        }
        let ac = a_cur.unwrap();
        a_cur = a.ast.next(&ac);
        let b_next = match bc.kind {
            NodeKind::KeyValue(kv) => kv.value,
            _ => bc.id,
        };
        let a_next = match ac.kind {
            NodeKind::KeyValue(kv) => kv.value,
            _ => ac.id,
        };
        if on_path {
            if let NodeKind::KeyValue(bkv) = bc.kind {
                let NodeKind::KeyValue(akv) = ac.kind else { return false };
                if !nodes_equal_deep(b, bkv.key, a, akv.key) {
                    return false;
                }
            }
            if !changed_only_under(b, b_next, a, a_next, &path[1..], change) {
                return false;
            }
        } else if !nodes_equal_deep(b, bc.id, a, ac.id) {
            return false;
        }
        b_cur = next_b;
        index += 1;
    }
    a_cur.is_none()
}

/// Whether `node` is what an emptied container may read back as: null, an
/// empty string, or a container with no children.
fn reads_empty(doc: &Document, node: Node<'_>) -> bool {
    match node.kind {
        NodeKind::Null => true,
        NodeKind::String(s) => s.is_empty(),
        NodeKind::Mapping(_) | NodeKind::Sequence(_) => matches!(doc.ast.child(&node), Ok(None)),
        _ => false,
    }
}

/// How many of `mentions` are a section's name, which a rename rewrites;
/// the rest are an item's path (`MentionKind::is_name`).
fn names_in(mentions: MentionIterator<'_>) -> usize {
    mentions.filter(|m| m.kind.is_name()).count()
}

/// Whether the entry `kv` of the parse before a delete names an IMPLICIT
/// table that held nothing but the path down to the deleted entry, so that
/// the entry's going takes it too: TOML's `a.b.c = 1` is the only thing that
/// makes `a.b` a table. A table is implicit when every place its name is
/// written is on an entry line (an `Entry` mention), none of them a header of
/// its own — and a container whose format records no mentions is never
/// implicit. `rest` is the path below `kv`, and `at` the deleted child's
/// index in the container at its end.
fn implied_only_by(b: &Document, kv: Node<'_>, rest: &[PathSegment<'_>], at: usize) -> bool {
    let NodeKind::KeyValue(e) = kv.kind else { return false };
    let table = b.ast.node(e.value);
    if !is_mapping(&table) {
        return false;
    }
    let mentions = b.mentions_of(table.id);
    if MentionIterator::count(&mentions) == 0 {
        return false;
    }
    for m in mentions {
        if m.kind != MentionKind::Entry {
            return false;
        }
    }
    let Ok(Some(only)) = b.ast.child(&table) else { return false };
    if b.ast.next(&only).is_some() {
        return false;
    }
    if rest.is_empty() {
        return at == 0;
    }
    match (rest[0], only.kind) {
        (PathSegment::Key(k), NodeKind::KeyValue(okv)) => key_node_is(b, okv.key, k) && implied_only_by(b, only, &rest[1..], at),
        _ => false,
    }
}

/// Whether the key node `id` is the string `name` — the physical spelling a
/// path segment matches against while walking two parses side by side.
fn key_node_is(doc: &Document, id: NodeId, name: &[u8]) -> bool {
    matches!(doc.ast.node(id).kind, NodeKind::String(s) if s == name)
}

// ── "only what was asked": the write guard ──────────────────────────────────
//
// A replace, an insert, a move and a reorder make the delete's promise in
// their own terms (`reads_back`): the document after the splice reads as the
// one before with one change to one container, and that change is what was
// asked for. The value written must read as the value asked for, which is
// its text read in the same format (`reading_of`). The container is named by
// a route of child indices rather than by the caller's path, so a key
// written twice on the way down is passed as the one the edit reached.

/// How the value or entry a write asked for reads: node `id` of `doc` (the
/// parse of the text written), an entry (key and value) when `entry`, else a
/// value.
#[derive(Clone)]
struct Reading {
    doc: Arc<Document>,
    id: NodeId,
    entry: bool,
}

impl Reading {
    /// The value read: the entry's, for an entry.
    fn value(&self) -> Reading {
        if !self.entry {
            return self.clone();
        }
        let id = kv_of(&self.doc.ast.node(self.id)).value;
        Reading { doc: self.doc.clone(), id, entry: false }
    }
}

/// The one change a guarded write may make to the container its route
/// reaches. A `None` reading is a value that does not read on its own, an
/// alias say, and is taken as written.
enum Write {
    /// The container is the node written, the document root.
    Whole(Option<Reading>),
    /// The child at `at` now reads as `want`; an entry keeps its key.
    Replaced { at: usize, want: Option<Reading> },
    /// One child was added, reading as `want`: at `at` for an item, and
    /// anywhere for an entry, which a format may place where it sorts.
    Added { at: Option<usize>, want: Option<Reading> },
    /// The children are those before, child `i` the one that stood at
    /// `order[i]`.
    Moved(Vec<usize>),
    /// Every mapping reads with the entries it had, in any order: a section
    /// moved whole, whose fragments the format gathers.
    Regrouped,
}

/// Where a node stands: child `at` of `container`, the child being the
/// node's entry when it is an entry's value.
#[derive(Debug, Clone, Copy)]
struct ChildSlot {
    container: NodeId,
    at: usize,
}

/// The `ChildSlot` of node `id`, or `None` for the root.
fn slot_of(doc: &Document, id: NodeId) -> Option<ChildSlot> {
    let ast = &doc.ast;
    let mut child = id;
    let mut parent = ast.parent_of(id)?;
    if matches!(ast.kind_of(parent), NodeKind::KeyValue(_)) {
        child = parent;
        parent = ast.parent_of(parent)?;
    }
    let container = ast.node(parent);
    let mut cur = ast.child(&container).ok()?;
    let mut at = 0usize;
    while let Some(c) = cur {
        if c.id == child {
            return Some(ChildSlot { container: parent, at });
        }
        at += 1;
        cur = ast.next(&c);
    }
    None
}

/// The child indices from the root down to node `id`, each a child of the
/// container the ones before reach (an entry's value, for an entry).
fn route_to(doc: &Document, id: NodeId) -> Result<Vec<usize>> {
    let mut route = Vec::new();
    let mut cur = id;
    while let Some(slot) = slot_of(doc, cur) {
        route.push(slot.at);
        cur = slot.container;
    }
    if cur != doc.ast.root {
        return Err(err!(NotFound));
    }
    route.reverse();
    Ok(route)
}

/// Whether `after` (node `aid`) is `before` (node `bid`) with only `write`
/// made to the container at the end of `route`: every node the route passes
/// on the way down, and every sibling of those, must be as it was.
fn written_only_at(b: &Document, bid: NodeId, a: &Document, aid: NodeId, route: &[usize], write: &Write) -> bool {
    let bn = b.ast.node(bid);
    let an = a.ast.node(aid);
    if route.is_empty() {
        return container_written(b, bn, a, an, write);
    }
    if tag(&bn) != tag(&an) {
        return false;
    }
    let Ok(mut b_cur) = b.ast.child(&bn) else { return false };
    let Ok(mut a_cur) = a.ast.child(&an) else { return false };
    let mut index = 0usize;
    while let Some(bc) = b_cur {
        let Some(ac) = a_cur else { return false };
        a_cur = a.ast.next(&ac);
        if index != route[0] {
            if !nodes_equal_deep(b, bc.id, a, ac.id) {
                return false;
            }
        } else if let NodeKind::KeyValue(bkv) = bc.kind {
            let NodeKind::KeyValue(akv) = ac.kind else { return false };
            if !nodes_equal_deep(b, bkv.key, a, akv.key) {
                return false;
            }
            if !written_only_at(b, bkv.value, a, akv.value, &route[1..], write) {
                return false;
            }
        } else if !written_only_at(b, bc.id, a, ac.id, &route[1..], write) {
            return false;
        }
        b_cur = b.ast.next(&bc);
        index += 1;
    }
    a_cur.is_none()
}

/// Whether the container `an` is `bn` with `write` made to it.
fn container_written(b: &Document, bn: Node<'_>, a: &Document, an: Node<'_>, write: &Write) -> bool {
    match write {
        Write::Whole(want) => match want {
            Some(w) => reads_as(w, a, an.id),
            None => true,
        },
        Write::Regrouped => nodes_equal_unordered(b, bn.id, a, an.id),
        Write::Replaced { at, want } => {
            if tag(&bn) != tag(&an) {
                return false;
            }
            let Ok(mut b_cur) = b.ast.child(&bn) else { return false };
            let Ok(mut a_cur) = a.ast.child(&an) else { return false };
            let mut index = 0usize;
            while let Some(bc) = b_cur {
                let Some(ac) = a_cur else { return false };
                a_cur = a.ast.next(&ac);
                if index != *at {
                    if !nodes_equal_deep(b, bc.id, a, ac.id) {
                        return false;
                    }
                } else {
                    if let NodeKind::KeyValue(bkv) = bc.kind {
                        let NodeKind::KeyValue(akv) = ac.kind else { return false };
                        if !nodes_equal_deep(b, bkv.key, a, akv.key) {
                            return false;
                        }
                    }
                    if let Some(w) = want {
                        if !reads_as(w, a, ac.id) {
                            return false;
                        }
                    }
                }
                b_cur = b.ast.next(&bc);
                index += 1;
            }
            a_cur.is_none()
        }
        Write::Added { at, want } => children_gained_one(b, bn, a, an, *at, want.as_ref()),
        Write::Moved(order) => {
            if tag(&bn) != tag(&an) {
                return false;
            }
            match seq_len(b, bn) {
                Ok(n) if n == order.len() => {}
                _ => return false,
            }
            let Ok(mut a_cur) = a.ast.child(&an) else { return false };
            for &from in order {
                let Some(ac) = a_cur else { return false };
                a_cur = a.ast.next(&ac);
                let Ok(Some(mut bc)) = b.ast.child(&bn) else { return false };
                for _ in 0..from {
                    match b.ast.next(&bc) {
                        Some(n) => bc = n,
                        None => return false,
                    }
                }
                if !nodes_equal_deep(b, bc.id, a, ac.id) {
                    return false;
                }
            }
            a_cur.is_none()
        }
    }
}

/// Whether the children of `an` are those of `bn`, each unchanged and in
/// its order, with one more: at `at`, or (`None`) anywhere, reading as
/// `want`. A container that read as empty (`reads_empty`) may come back as
/// the container its first child makes of it: YAML's `k:` given an entry is
/// a mapping, ZON's `.{}` given an item a tuple.
fn children_gained_one(b: &Document, bn: Node<'_>, a: &Document, an: Node<'_>, at: Option<usize>, want: Option<&Reading>) -> bool {
    if !is_mapping(&an) && !is_sequence(&an) {
        return false;
    }
    let same = tag(&bn) == tag(&an);
    if !same && !reads_empty(b, bn) {
        return false;
    }
    if same && at.is_none() {
        let (Ok(nb), Ok(na)) = (seq_len(b, bn), seq_len(a, an)) else { return false };
        if nb == na {
            return match want {
                Some(w) => entry_merged(b, bn, a, an, w),
                None => false,
            };
        }
    }
    let mut b_cur = if same {
        match b.ast.child(&bn) {
            Ok(c) => c,
            Err(_) => return false,
        }
    } else {
        None
    };
    let Ok(mut a_cur) = a.ast.child(&an) else { return false };
    let mut index = 0usize;
    let mut extra: Option<Node<'_>> = None;
    while let Some(ac) = a_cur {
        let next_a = a.ast.next(&ac);
        // The first child that is not the next one before is the new one;
        // one equal to its neighbour is either, and reads the same.
        if extra.is_none() {
            let new = match at {
                Some(i) => index == i,
                None => match b_cur {
                    Some(bc) => !nodes_equal_deep(b, bc.id, a, ac.id),
                    None => true,
                },
            };
            if new {
                extra = Some(ac);
                a_cur = next_a;
                index += 1;
                continue;
            }
        }
        let Some(bc) = b_cur else { return false };
        if !nodes_equal_deep(b, bc.id, a, ac.id) {
            return false;
        }
        b_cur = b.ast.next(&bc);
        a_cur = next_a;
        index += 1;
    }
    let Some(added) = extra else { return false };
    if b_cur.is_some() {
        return false;
    }
    match want {
        Some(w) => reads_as(w, a, added.id),
        None => true,
    }
}

/// Whether the mapping `an` is `bn` with the entry `want` written into an
/// entry it already has: a dotted key names tables it passes through, and
/// TOML's `t.x = []` beside `t.k = 1` adds `x` to the table `t`, which
/// gains it as `children_gained_one` says. Every other entry is unchanged.
fn entry_merged(b: &Document, bn: Node<'_>, a: &Document, an: Node<'_>, want: &Reading) -> bool {
    if !want.entry || !is_mapping(&bn) {
        return false;
    }
    let wdoc = &want.doc;
    let wkv_node = wdoc.ast.node(want.id);
    let NodeKind::KeyValue(wkv) = wkv_node.kind else { return false };
    let inner = wdoc.ast.node(wkv.value);
    if !is_mapping(&inner) {
        return false;
    }
    let Ok(Some(only)) = wdoc.ast.child(&inner) else { return false };
    if wdoc.ast.next(&only).is_some() {
        return false;
    }
    let Ok(mut b_cur) = b.ast.child(&bn) else { return false };
    let Ok(mut a_cur) = a.ast.child(&an) else { return false };
    let mut merged = false;
    while let Some(bc) = b_cur {
        b_cur = b.ast.next(&bc);
        let Some(ac) = a_cur else { return false };
        a_cur = a.ast.next(&ac);
        if nodes_equal_deep(b, bc.id, a, ac.id) {
            continue;
        }
        if merged {
            return false;
        }
        let (NodeKind::KeyValue(bkv), NodeKind::KeyValue(akv)) = (bc.kind, ac.kind) else { return false };
        if !nodes_equal_deep(b, bkv.key, a, akv.key) || !nodes_equal_deep(wdoc, wkv.key, a, akv.key) {
            return false;
        }
        let deeper = Reading { doc: want.doc.clone(), id: only.id, entry: true };
        if !children_gained_one(b, b.ast.node(bkv.value), a, a.ast.node(akv.value), None, Some(&deeper)) {
            return false;
        }
        merged = true;
    }
    merged && a_cur.is_none()
}

/// Whether node `aid` of `a` reads as `want`: the whole entry, or the value
/// (an entry's value, for an entry). Two values that each read as empty
/// (`reads_empty`) are equal: the format reads an empty container one way
/// wherever it is written.
fn reads_as(want: &Reading, a: &Document, aid: NodeId) -> bool {
    let node = a.ast.node(aid);
    if want.entry {
        return nodes_equal_deep(&want.doc, want.id, a, aid);
    }
    let value = match node.kind {
        NodeKind::KeyValue(kv) => a.ast.node(kv.value),
        _ => node,
    };
    if reads_empty(&want.doc, want.doc.ast.node(want.id)) && reads_empty(a, value) {
        return true;
    }
    nodes_equal_deep(&want.doc, want.id, a, value.id)
}

/// Whether nothing but whitespace stands in `source` outside `span`.
fn only_blank_around(source: &[u8], span: Span) -> bool {
    let ws: &[u8] = b" \t\r\n";
    trim(&source[..span.start], ws).is_empty() && trim(&source[span.end..], ws).is_empty()
}

/// `nodes_equal_deep`, with a mapping's entries matched in any order, each
/// to one of the other's: the reading a section move must leave alone.
fn nodes_equal_unordered(b: &Document, bid: NodeId, a: &Document, aid: NodeId) -> bool {
    let bn = b.ast.node(bid);
    let an = a.ast.node(aid);
    if tag(&bn) != tag(&an) {
        return false;
    }
    match bn.kind {
        NodeKind::Sequence(_) => {
            let Ok(mut b_cur) = b.ast.child(&bn) else { return false };
            let Ok(mut a_cur) = a.ast.child(&an) else { return false };
            while let Some(bc) = b_cur {
                let Some(ac) = a_cur else { return false };
                if !nodes_equal_unordered(b, bc.id, a, ac.id) {
                    return false;
                }
                b_cur = b.ast.next(&bc);
                a_cur = a.ast.next(&ac);
            }
            a_cur.is_none()
        }
        NodeKind::Mapping(_) => {
            match (seq_len(b, bn), seq_len(a, an)) {
                (Ok(x), Ok(y)) if x == y => {}
                _ => return false,
            }
            // Each entry as many times on either side: with the counts
            // equal, the entries pair up one to one.
            let Ok(mut b_cur) = b.ast.child(&bn) else { return false };
            while let Some(bc) = b_cur {
                if count_equal(b, bn, bc.id, b) != count_equal(a, an, bc.id, b) {
                    return false;
                }
                b_cur = b.ast.next(&bc);
            }
            true
        }
        NodeKind::KeyValue(kv) => {
            let akv = kv_of(&an);
            nodes_equal_unordered(b, kv.key, a, akv.key) && nodes_equal_unordered(b, kv.value, a, akv.value)
        }
        _ => bn.kind.eql(&an.kind),
    }
}

/// How many children of `container` (in `doc`) equal node `id` of `of`, as
/// `nodes_equal_unordered` compares.
fn count_equal(doc: &Document, container: Node<'_>, id: NodeId, of: &Document) -> usize {
    let Ok(mut cur) = doc.ast.child(&container) else { return 0 };
    let mut n = 0usize;
    while let Some(c) = cur {
        if nodes_equal_unordered(of, id, doc, c.id) {
            n += 1;
        }
        cur = doc.ast.next(&c);
    }
    n
}

/// Drop a single trailing '\n' (the serializer ends every value with one).
fn strip_trailing_newline(text: &[u8]) -> &[u8] {
    if !text.is_empty() && text[text.len() - 1] == b'\n' {
        return &text[..text.len() - 1];
    }
    text
}

/// Whether a closed-container format's container closes on the line its last
/// child ends on (plist's `<key>o</key><dict>…</dict>`, OpenStep's
/// `o = {a = 1; };`): its span ends on its `close` token, after that child
/// and no later than the line (`insert_at`, the line end an append splices
/// at). Such a container has no line of its own to append after.
fn closes_on_last_line(source: &[u8], span: Span, last: Span, close: &[u8], insert_at: usize) -> bool {
    if span.end <= last.end || span.end > insert_at {
        return false;
    }
    !close.is_empty() && source[..span.end].ends_with(close)
}

/// Whether `gap` — the bytes between a line's indent and a token on it — is
/// one or more sequence item markers and the blanks after them (`- `, or
/// `- - ` for an item of an item), which is when the token's siblings align
/// under the token rather than under the line's indent. See `indent_at`.
fn only_item_markers(gap: &[u8], seq_item_marker: &[u8]) -> bool {
    let marker = trim(seq_item_marker, b" \t");
    if marker.is_empty() {
        return false;
    }
    let mut i = 0usize;
    let mut saw = false;
    while i < gap.len() {
        if gap[i] == b' ' || gap[i] == b'\t' {
            i += 1;
        } else if gap[i..].starts_with(marker) {
            i += marker.len();
            saw = true;
        } else {
            return false;
        }
    }
    saw
}

/// Append `value_text` to `out`, re-indented so its lines sit under `indent`.
/// The first line is emitted verbatim (it follows `key: ` or `- `); every
/// subsequent non-blank line is prefixed with the `indent` bytes, preserving
/// the serializer's own relative indentation. One trailing '\n' is stripped.
fn reindent_into(out: &mut Vec<u8>, value_text: &[u8], indent: &[u8]) {
    let text = strip_trailing_newline(value_text);
    let mut first = true;
    for line in text.split(|&c| c == b'\n') {
        if !first {
            out.push(b'\n');
            if !line.is_empty() {
                out.extend_from_slice(indent);
            }
        }
        out.extend_from_slice(line);
        first = false;
    }
}

#[cfg(test)]
mod tests {
    //! The Zig tests of `src/editor.zig` all drive a concrete language and
    //! live on as the languages' `tests/edits.json` fixtures. These drive
    //! the engine through a tiny private line format (`key = value`, `#`
    //! comments) to check the splice, the reparse net and the guards.

    use super::*;
    use crate::kit::ast::KeyValue;
    use crate::kit::language::add_diagnostic;
    use crate::kit::manifest::{Caps, DialectInfo, SpliceStyle};
    use crate::kit::row_writer::RowWriter;
    use crate::kit::table::Table;

    struct Lines;

    const LINES_DIALECTS: &[DialectInfo] = &[DialectInfo::new("lines", SpliceStyle::Raw, Some(b""))];

    impl Language for Lines {
        fn name(&self) -> &str {
            "lines"
        }
        fn extensions(&self) -> &[&'static str] {
            &["lines"]
        }
        fn caps(&self) -> Caps {
            Caps { edit: true, ..Caps::DEFAULT }
        }
        fn dialects(&self) -> &[DialectInfo] {
            LINES_DIALECTS
        }
        fn syntax(&self, _: Dialect) -> Syntax {
            Syntax { flow_containers: false, ..Syntax::new(Comments::HASH, Some(b" = "), None) }
        }
        fn parse_table(&self, _input: &[u8], _: Dialect, out: &mut Table) -> Result<()> {
            let src = out.source_arc();
            let mut w = RowWriter::init(out);
            let root = w.add(NodeKind::Mapping(None), Span::new(0, src.len()))?;
            let mut last: Option<u32> = None;
            let mut at = 0usize;
            for line in src.split(|&c| c == b'\n') {
                let start = at;
                at += line.len() + 1;
                let body = trim(line, b" \t\r");
                if body.is_empty() || body[0] == b'#' {
                    continue;
                }
                let Some(eq) = find_byte(line, b'=') else {
                    add_diagnostic(w.t, table::DIAGNOSTIC_ERROR, start, None, b"expected `=`", b"here")?;
                    return Err(err!(MissingEquals));
                };
                let ks = start + first_non_space(line, 0);
                let ke = start + trim_end(&line[..eq], b" \t").len();
                let vs = start + first_non_space(line, eq + 1);
                let ve = (start + trim_end(line, b" \t\r").len()).max(vs);
                let k = w.add(NodeKind::String(&src[ks..ke]), Span::new(ks, ke))?;
                let v = w.add(NodeKind::String(&src[vs..ve]), Span::new(vs, ve))?;
                let kv = w.add(NodeKind::KeyValue(KeyValue { key: k, value: v }), Span::new(ks, ve))?;
                match last {
                    Some(p) => w.set_next(p, Some(kv)),
                    None => w.set(root, NodeKind::Mapping(Some(kv)))?,
                }
                last = Some(kv);
            }
            w.finish(root)
        }
    }

    fn ed(src: &[u8]) -> Editor<'static> {
        let mut e = Editor::new(&Lines, 0);
        e.init(src).unwrap();
        e
    }

    fn k(name: &[u8]) -> PathSegment<'_> {
        PathSegment::Key(name)
    }

    #[test]
    fn literal_of_types_bare_text_by_fig_rules() {
        assert_eq!(Literal::Int, literal_of(b" 42\n"));
        assert_eq!(Literal::Float, literal_of(b"2.5"));
        assert_eq!(Literal::Bool, literal_of(b"true"));
        assert_eq!(Literal::Null, literal_of(b"null"));
        assert_eq!(Literal::Datetime, literal_of(b"2026-09-10"));
        assert_eq!(Literal::String, literal_of(b"Yes"));
        assert_eq!(Literal::String, literal_of(b"007"));
    }

    #[test]
    fn init_once_and_not_before() {
        let mut e = Editor::new(&Lines, 0);
        assert_eq!(err!(NotInitialized), e.set(&[k(b"a")], b"1").unwrap_err());
        e.init(b"a = 1\n").unwrap();
        assert_eq!(err!(MultipleInit), e.init(b"b = 2\n").unwrap_err());
    }

    #[test]
    fn set_replaces_then_inserts() {
        let mut e = ed(b"a = 1\nb = 2\n");
        e.set(&[k(b"b")], b"3").unwrap();
        assert_eq!(b"a = 1\nb = 3\n", e.source());
        e.set(&[k(b"c")], b"4").unwrap();
        assert_eq!(b"a = 1\nb = 3\nc = 4\n", e.source());
        assert_eq!(err!(NotAMapping), e.set(&[], b"x").unwrap_err());
    }

    #[test]
    fn a_splice_that_does_not_parse_rolls_back_and_blames_the_text() {
        let mut e = ed(b"a = 1\n");
        assert_eq!(err!(MissingEquals), e.replace_val_at_path(&[k(b"a")], b"1\nnope").unwrap_err());
        assert!(e.splice_rejected);
        assert_eq!(b"a = 1\n", e.source());
        e.set(&[k(b"a")], b"2").unwrap();
        assert!(!e.splice_rejected);
    }

    #[test]
    fn a_write_that_reads_back_wrong_is_rolled_back() {
        // `2 = 3` spliced as a value parses — as the value `2 = 3`, one
        // entry — so the guard has nothing to object to; but a value that
        // smuggles in a second entry does change another node.
        let mut e = ed(b"a = 1\nb = 2\n");
        assert_eq!(err!(EditReadsBackWrong), e.replace_val_at_path(&[k(b"a")], b"1\nc = 3").unwrap_err());
        assert_eq!(b"a = 1\nb = 2\n", e.source());
    }

    #[test]
    fn insert_named_key_refuses_a_held_name_and_insert_key_counts_repeats() {
        let mut e = ed(b"a = 1\n");
        assert_eq!(err!(DuplicateKey), e.insert_named_key(&[], b"a", b"2").unwrap_err());
        assert_eq!(err!(DuplicateKey), e.insert_key(&[], b"a", b"2").unwrap_err());
        assert_eq!(b"a = 1\n", e.source());
        e.insert_named_key(&[], b"b", b"2").unwrap();
        assert_eq!(b"a = 1\nb = 2\n", e.source());
    }

    #[test]
    fn delete_key_takes_the_owned_comment_block() {
        let mut e = ed(b"a = 1\n\n# about b\nb = 2\nc = 3\n");
        e.delete_key(&[k(b"b")]).unwrap();
        assert_eq!(b"a = 1\n\nc = 3\n", e.source());
        assert_eq!(err!(NotFound), e.delete_key(&[k(b"zz")]).unwrap_err());
    }

    #[test]
    fn rename_keeps_the_value_and_refuses_a_duplicate() {
        let mut e = ed(b"a = 1\nb = 2\n");
        e.replace_key_at_path(&[k(b"a")], b"x").unwrap();
        assert_eq!(b"x = 1\nb = 2\n", e.source());
        assert_eq!(err!(DuplicateKey), e.replace_named_key(&[k(b"x")], b"b").unwrap_err());
        assert_eq!(err!(NotAKey), e.replace_key_at_path(&[], b"y").unwrap_err());
    }

    #[test]
    fn leading_comments_round_trip() {
        let mut e = ed(b"a = 1\nb = 2\n");
        e.add_leading_comment(&[k(b"b")], b"one\ntwo").unwrap();
        assert_eq!(b"a = 1\n# one\n# two\nb = 2\n", e.source());
        assert_eq!(Some(b"one\ntwo".to_vec()), e.get_leading_comment(&[k(b"b")]).unwrap());
        assert_eq!(None, e.get_leading_comment(&[k(b"a")]).unwrap());
        e.delete_leading_comments(&[k(b"b")]).unwrap();
        assert_eq!(b"a = 1\nb = 2\n", e.source());
    }

    #[test]
    fn a_trailing_comment_the_value_would_swallow_is_refused() {
        let mut e = ed(b"a = 1\n");
        assert_eq!(err!(CommentInsideValue), e.set_trailing_comment(&[k(b"a")], b"note").unwrap_err());
        assert_eq!(b"a = 1\n", e.source());
        assert_eq!(err!(MultilineComment), e.set_trailing_comment(&[k(b"a")], b"x\ny").unwrap_err());
    }

    #[test]
    fn dangling_comments_round_trip_at_the_root() {
        let mut e = ed(b"a = 1\n");
        e.add_dangling_comment(&[], b"end").unwrap();
        assert_eq!(b"a = 1\n# end\n", e.source());
        assert_eq!(Some(b"end".to_vec()), e.get_dangling_comment(&[]).unwrap());
        e.delete_dangling_comments(&[]).unwrap();
        assert_eq!(b"a = 1\n", e.source());
        assert_eq!(err!(UnsupportedShape), e.get_dangling_comment(&[k(b"a")]).unwrap_err());
    }

    #[test]
    fn comment_out_and_back() {
        let mut e = ed(b"a = 1\nb = 2\nc = 3\n");
        e.comment_out(&[k(b"b")]).unwrap();
        assert_eq!(b"a = 1\n# b = 2\nc = 3\n", e.source());
        e.uncomment_leading(&[k(b"c")], 0, 1).unwrap();
        assert_eq!(b"a = 1\nb = 2\nc = 3\n", e.source());
        assert_eq!(err!(NotFound), e.uncomment_leading(&[k(b"c")], 0, 1).unwrap_err());
        e.comment_out(&[k(b"c")]).unwrap();
        e.uncomment_dangling(&[], 0, 1).unwrap();
        assert_eq!(b"a = 1\nb = 2\nc = 3\n", e.source());
    }

    #[test]
    fn uncommenting_a_line_that_is_no_entry_is_refused() {
        let mut e = ed(b"a = 1\n# just a note\nb = 2\n");
        assert_eq!(err!(MissingEquals), e.uncomment_leading(&[k(b"b")], 0, 1).unwrap_err());
        assert_eq!(b"a = 1\n# just a note\nb = 2\n", e.source());
    }

    #[test]
    fn moves_and_reorders_carry_their_comments() {
        let mut e = ed(b"a = 1\n# bee\nb = 2\nc = 3\n");
        e.move_key(&[k(b"c")], &[k(b"a")]).unwrap();
        assert_eq!(b"c = 3\na = 1\n# bee\nb = 2\n", e.source());
        e.reorder_keys(&[], &[b"b", b"a"]).unwrap();
        assert_eq!(b"# bee\nb = 2\na = 1\nc = 3\n", e.source());
    }

    #[test]
    fn container_ops_belong_to_section_formats() {
        let mut e = ed(b"a = 1\n");
        assert!(!e.has_container_op("deleteContainer"));
        assert!(!e.dialect_has_container_op("insertContainer"));
        assert_eq!(err!(UnsupportedFormat), e.delete_container(&[k(b"a")]).unwrap_err());
        assert_eq!(err!(UnsupportedFormat), e.reorder_containers(&[b"a"]).unwrap_err());
    }

    #[test]
    fn sequences_are_refused_where_there_are_none() {
        let mut e = ed(b"a = 1\n");
        assert_eq!(err!(NotASequence), e.append_to_seq(&[k(b"a")], b"2").unwrap_err());
        assert_eq!(err!(NotASequence), e.remove_seq_item(&[k(b"a")], 0).unwrap_err());
        // No empty-sequence spelling: `set_append` cannot start a list.
        assert_eq!(err!(NotFound), e.set_append(&[k(b"zz")], b"2").unwrap_err());
    }

    #[test]
    fn helpers() {
        assert!(only_item_markers(b"- ", b"- "));
        assert!(only_item_markers(b"- - ", b"- "));
        assert!(!only_item_markers(b"x ", b"- "));
        assert!(!only_item_markers(b"", b"- "));
        let mut out = Vec::new();
        reindent_into(&mut out, b"a\nb\n\nc\n", b"  ");
        assert_eq!(b"a\n  b\n\n  c", &out[..]);
        assert_eq!(b"x", strip_line_comment_marker(b"# x", CommentDelimiter::open(b"#")));
        assert_eq!(b"x", strip_line_comment_marker(b"<!-- x -->", CommentDelimiter::pair(b"<!--", b"-->")));
        let p = padded(b"ab   # c\n", Span::new(0, 2), b"abcd");
        assert_eq!((Span::new(0, 5), &b"abcd "[..]), (p.span, &p.text[..]));
        assert_eq!(3, comment_column(b"> > x", 0, true) - 1);
    }
}
