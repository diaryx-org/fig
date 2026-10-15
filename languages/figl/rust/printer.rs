//! Port of languages/figl/src/printer.zig.
//!
//! fig authoring dialect printer — renders an AST in the `fig fmt` house
//! style (DESIGN.md "What `fig fmt` normalizes"):
//!
//!   * Spaced marker runs (`> > key`) and NO leading indentation by default —
//!     the markers themselves form the visual ruler (each level shifts the
//!     line two columns, the same geometry indentation would add, without a
//!     redundant second signal to maintain). `options.indent_requested` is an
//!     opt-in (`fig fmt --indent`) cosmetic overlay that prefixes each marker
//!     run with `2 × depth` literal spaces on top — purely visual (see
//!     `write_cosmetic_indent`); canonical (default) output is unaffected.
//!   * Fits-or-breaks: a container value renders as inline flow iff every
//!     descendant is flow-representable, no comment would be dropped or
//!     re-anchored, no object directly contains another object, and the whole
//!     line fits `options.width`. All-or-nothing per node — no partial
//!     hoisting of the widest member — so output is stable under small edits.
//!   * Dotted-key collapse: chains of single-child maps collapse to `a.b.c`.
//!   * Sections at the document root: scalar/inline children print as dotted
//!     assignments; a map prints under a dotted section header while its block
//!     body stays within depth 2 and hoists each child to its own section when
//!     it would nest deeper; a sequence of maps prints as a `path[]` append
//!     header with `+` continuation blocks for the rest of its elements.
//!   * Comments are conservative blockers: any spelling a re-parse would not
//!     re-anchor identically falls back to the nested form that does.
//!
//! Every emitted document parses back to the same AST (modulo comments, which
//! DO round-trip).
//!
//! Errors (Zig's `Printer.Error`, less the writer's own): `UnresolvedAlias`,
//! `NonStringKey`, `UnexpectedNodeKind`, `UnrepresentableRoot`, and
//! `NonFiniteNumber` — an infinite or NaN float where no `: float =`
//! annotation can go (a scalar fragment, or inside a flow container a
//! fragment was asked to keep in flow). figl spells one only as `key: float =
//! inf` (`-inf`, `nan`); bare, `inf` reads back as a string.

use std::cell::Cell;

use super::tokenizer as tok;
use super::tokenizer::{BracketCommit, Sniffed};
use fig_kit::err;
use fig_kit::ast::{Comment, ExtKind, Extended, KindTag, NodeId, NodeKind, SerializeOptions, Tag, AST};
use fig_kit::number as num;
use fig_kit::table;
use fig_kit::Result;

/// Deepest `>` count a section header's block body may reach before the map is
/// hoisted into per-child sections instead.
const MAX_BODY_DEPTH: usize = 2;
/// A flow-representable sequence with more than this many elements breaks onto
/// multiple lines (one element per line) even when it would fit the width
/// budget. Short lists (`[a, b, c]`) stay inline; longer ones read and diff
/// better stacked. Independent of the width test — either trigger (too wide,
/// or too many items) breaks the list.
const MAX_INLINE_SEQ_ITEMS: usize = 6;
/// Recursion guard for the section path buffer (collapse/hoist recursion is
/// bounded by tree depth; 128 dotted segments is far past any real config).
const PATH_CAP: usize = 128;
/// Columns of `" = "`, the operator between a key and its inline value.
const ASSIGN_OP_WIDTH: usize = 3;
/// Columns one marker level occupies — a `> ` run cell, or the `* ` bullet.
const MARKER_CELL: usize = 2;

/// Memoized `inline_width` result for a node, keyed by node id (`width_memo`).
/// `inline_width` is asked for the same subtree repeatedly (the block-depth
/// pre-pass, the fits-inline test, the multiline-flow test, then real
/// emission), and each call otherwise walks the whole subtree — so caching the
/// comment-policy-independent core collapses that from superlinear to one pass.
#[derive(Debug, Clone, Copy, Default)]
struct WidthMemo {
    done: bool,
    width: Option<usize>,
}

pub struct Printer<'a> {
    writer: &'a mut Vec<u8>,
    ast: &'a AST,
    options: SerializeOptions,
    /// Per-node-id `inline_width` cache, sized to `ast.node_count()` by the
    /// entry points.
    width_memo: Vec<Cell<WidthMemo>>,
    /// Dotted path (key node ids) of the section currently being emitted.
    path: Vec<NodeId>,
    /// Section separation state: a blank line goes between two sections when
    /// either side is multi-line (or carries leading comments).
    started: bool,
    prev_multiline: bool,
}

/// Write `ast` as a whole figl document.
pub fn print(writer: &mut Vec<u8>, ast: &AST, options: SerializeOptions) -> Result<()> {
    let mut p = Printer::new(writer, ast, options);
    p.leading_comments(ast.leading_comment_anchor(ast.root), 0)?;
    p.root(ast.root, RootMode::Document)
}

/// Render `ast.root` as a value *fragment* — the caller-built-value path
/// behind `fig_value_serialize_opts`, which backs the editor's `replace`/`set`
/// (the editor splices a replacement value's text into existing source, e.g.
/// after `key = `). Unlike `print`, a fragment is never asked to stand alone
/// as a document, so a scalar/null root — which `root`'s `Document` mode
/// rejects as `UnrepresentableRoot` — is fine here: `root`'s `Fragment` mode
/// renders it with the same bare/quoted spelling `value` uses for any other
/// scalar.
pub fn print_fragment(writer: &mut Vec<u8>, ast: &AST, options: SerializeOptions) -> Result<()> {
    let mut p = Printer::new(writer, ast, options);
    p.leading_comments(ast.leading_comment_anchor(ast.root), 0)?;
    p.root(ast.root, RootMode::Fragment)
}

/// The subtree at `id` as block lines at marker depth `depth` (a depth-0
/// mapping as the document's sections).
pub fn print_node(writer: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize, options: SerializeOptions) -> Result<()> {
    let mut p = Printer::new(writer, ast, options);
    match ast.kind_of(id) {
        NodeKind::Mapping(_) => {
            if depth == 0 {
                p.emit_sections(id)?;
                for c in p.dangling_of(id) {
                    p.comment_lines(c, 0)?;
                }
                Ok(())
            } else {
                p.map_body(id, depth)
            }
        }
        NodeKind::Sequence(_) => p.seq_body(id, depth),
        // A scalar reaching here at depth 0 is being asked to stand as an
        // entire fig document/subdocument — see `root`, which this mirrors
        // exactly (a non-zero depth never reaches this arm from the printer
        // itself: nested scalars go through `value`).
        _ => {
            if depth == 0 {
                return Err(err!(UnrepresentableRoot));
            }
            p.value(id, Pos::Block)?;
            p.writer.push(b'\n');
            Ok(())
        }
    }
}

/// `Document`: `print`'s mode — a scalar/null/extended/alias root is rejected,
/// since it has no authoring spelling as a *whole fig document*. `Fragment`:
/// `print_fragment`'s mode — such a root is instead rendered with its ordinary
/// scalar spelling, because a fragment is never reparsed alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootMode {
    Document,
    Fragment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommentPolicy {
    /// Any comment disqualifies (interior flow position — comments would drop).
    Strict,
    /// The node's own trailing comment prints after the line (value position).
    AllowTrailing,
    /// Leading comments print before the line too (element position).
    AllowLeadingTrailing,
}

/// Where a value is written, which decides what may go unquoted: `List` (an
/// array element) tightens the bare-string rules to what survives inside
/// `[…]`, and `Compact` (a `{…}` object's value) further to one token
/// (docs/proposals/compact-objects.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pos {
    Block,
    List,
    Compact,
}

#[derive(Debug, Clone, Copy)]
struct Chain {
    end_kv: NodeId,
    key_width: usize,
}

/// Columns before a map entry's value: the marker run (`> ` per level), the
/// dotted key, then `" = "`.
fn map_entry_prefix_width(depth: usize, key_width: usize) -> usize {
    MARKER_CELL * depth + key_width + ASSIGN_OP_WIDTH
}

/// Columns before an element's value: `* ` at root, `> > * ` etc. below — the
/// `> ` marker run plus the `* ` bullet cell (one more `MARKER_CELL`).
fn element_prefix_width(depth: usize) -> usize {
    MARKER_CELL * (depth + 1)
}

impl<'a> Printer<'a> {
    fn new(writer: &'a mut Vec<u8>, ast: &'a AST, options: SerializeOptions) -> Printer<'a> {
        Printer {
            writer,
            ast,
            options,
            width_memo: vec![Cell::new(WidthMemo::default()); ast.node_count()],
            path: Vec::with_capacity(PATH_CAP),
            started: false,
            prev_multiline: false,
        }
    }

    fn write_all(&mut self, s: &[u8]) {
        self.writer.extend_from_slice(s);
    }

    fn write_byte(&mut self, b: u8) {
        self.writer.push(b);
    }

    fn mapping_first(&self, id: NodeId) -> Option<NodeId> {
        self.ast.kind_of(id).as_mapping().flatten()
    }

    fn sequence_first(&self, id: NodeId) -> Option<NodeId> {
        self.ast.kind_of(id).as_sequence().flatten()
    }

    fn next_of(&self, id: NodeId) -> Option<NodeId> {
        self.ast.node(id).next_sibling
    }

    fn kv_of(&self, id: NodeId) -> fig_kit::ast::KeyValue {
        self.ast.kind_of(id).as_keyvalue().expect("a mapping's child is a keyvalue")
    }

    /// The root is a map or a sequence in `Document` mode (never a bare
    /// scalar — see DESIGN.md and docs/spec.md § 2); `Fragment` mode
    /// additionally accepts a bare scalar/null root (see `print_fragment`). A
    /// root map is emitted as sections; a root sequence as zero-marker `*`
    /// elements.
    fn root(&mut self, id: NodeId, mode: RootMode) -> Result<()> {
        // A fragment destined for an inline splice (`key = <fragment>`,
        // requested via `options.flow`) has no valid block spelling: `* `
        // element lines and section headers only parse as standalone lines, so
        // a block container spliced inline re-reads as a bare string. Flow is
        // the one spelling that survives the round-trip; scalar fragments fall
        // through unchanged.
        if mode == RootMode::Fragment
            && self.options.flow
            && matches!(self.ast.kind_of(id), NodeKind::Mapping(_) | NodeKind::Sequence(_))
        {
            self.flow_value(id)?;
            self.write_byte(b'\n');
            return Ok(());
        }
        match self.ast.kind_of(id) {
            NodeKind::Mapping(_) => {
                self.emit_sections(id)?;
                for c in self.dangling_of(id) {
                    self.comment_lines(c, 0)?;
                }
                Ok(())
            }
            NodeKind::Sequence(_) => self.seq_body(id, 0), // prints its own dangling run
            _ => match mode {
                RootMode::Fragment => {
                    // The splice lands after `key = `, where the annotation a
                    // non-finite float needs cannot go.
                    if self.is_non_finite(id) {
                        return Err(err!(NonFiniteNumber));
                    }
                    self.value(id, Pos::Block)?;
                    self.write_byte(b'\n');
                    Ok(())
                }
                // A scalar/null/extended/alias root has no authoring spelling
                // in the fig dialect: a bare token at the document root reads
                // as a container header with no children (`FigEmptyContainer`),
                // not a scalar. Hard-error instead of silently emitting
                // non-conforming output (docs/spec.md § 2).
                RootMode::Document => Err(err!(UnrepresentableRoot)),
            },
        }
    }

    // ── Section emission (document root) ────────────────────────────────────

    fn emit_sections(&mut self, map_id: NodeId) -> Result<()> {
        let mut cur = self.mapping_first(map_id);
        while let Some(kv_id) = cur {
            self.emit_section(kv_id)?;
            cur = self.next_of(kv_id);
        }
        Ok(())
    }

    /// Emit one map entry at section level: the current dotted `path` plus
    /// this entry's key names the target, and the value's shape picks the
    /// spelling.
    fn emit_section(&mut self, kv_id: NodeId) -> Result<()> {
        let kv = self.kv_of(kv_id);
        self.path.push(kv.key);
        let r = self.emit_section_body(kv_id, kv);
        self.path.pop();
        r
    }

    fn emit_section_body(&mut self, kv_id: NodeId, kv: fig_kit::ast::KeyValue) -> Result<()> {
        let can_extend = self.path.len() < PATH_CAP;

        // Dotted collapse: step through single-child maps (`a.b.c …`).
        if can_extend && let Some(child) = self.collapse_child(kv_id) {
            return self.emit_section(child);
        }

        let v = kv.value;
        match self.ast.kind_of(v) {
            NodeKind::Mapping(first_opt) => {
                if self.fits_inline(v, self.path_width() + ASSIGN_OP_WIDTH, CommentPolicy::AllowTrailing) {
                    return self.section_assign(kv.key, v);
                }
                // Hoist: a body that would pass the depth budget reads better
                // as sections. Only when every comment re-anchors: the map
                // itself gets no dedicated line, so it must carry none of its
                // own.
                //
                // Consecutive FLAT children (scalar / inline-fitting) group
                // under ONE header re-entry — naming the path once instead of
                // repeating it per line — while each deep child gets its own
                // section. A lone flat child stays a dotted assignment.
                if can_extend
                    && first_opt.is_some()
                    && self.block_depth_map(v, 1) > MAX_BODY_DEPTH
                    && !self.key_has_leading(kv.key)
                    && !self.has_trailing_or_dangling(v)
                {
                    let mut cur = first_opt;
                    while cur.is_some() {
                        // Measure the run of consecutive flat children at `cur`.
                        let mut run_len = 0usize;
                        let mut scan = cur;
                        while let Some(child) = scan {
                            if !self.is_flat_child(child) {
                                break;
                            }
                            run_len += 1;
                            scan = self.next_of(child);
                        }
                        if run_len == 1 {
                            let c = cur.unwrap();
                            self.emit_section(c)?;
                            cur = self.next_of(c);
                        } else if run_len >= 2 {
                            self.begin_section(true, None)?;
                            self.write_path()?;
                            self.write_byte(b'\n');
                            for _ in 0..run_len {
                                let c = cur.unwrap();
                                self.map_entry_line(c, 1)?;
                                cur = self.next_of(c);
                            }
                        }
                        // `cur` now sits on a deep child (or the end).
                        if let Some(deep) = cur {
                            self.emit_section(deep)?;
                            cur = self.next_of(deep);
                        }
                    }
                    return Ok(());
                }
                // Dotted section header + block body.
                self.begin_section(true, Some(kv.key))?;
                self.write_path()?;
                self.trailing_comment(v);
                self.write_byte(b'\n');
                self.map_body(v, 1)
            }
            NodeKind::Sequence(first_opt) => {
                if self.seq_fits_inline(v, self.path_width() + ASSIGN_OP_WIDTH, CommentPolicy::AllowTrailing) {
                    return self.section_assign(kv.key, v);
                }
                // A list of maps is the append header's home turf: `path[]` for
                // the first element, `+` for the rest, fields one `>` deep.
                if first_opt.is_some() && self.all_elements_mappings(v) && !self.key_has_leading(kv.key) && !self.has_trailing_or_dangling(v) {
                    return self.emit_append_group(v);
                }
                // A flow-representable scalar/nested-list list that overflowed
                // the inline budget (or the item-count threshold) stacks as
                // multi-line flow — `path = [` / one element per line / `]`.
                if self.multiline_flow_eligible(v, CommentPolicy::AllowTrailing) {
                    return self.section_multiline_flow_seq(kv.key, v);
                }
                // Otherwise a header + `* v` element lines.
                self.begin_section(true, Some(kv.key))?;
                self.write_path()?;
                self.trailing_comment(v);
                self.write_byte(b'\n');
                self.seq_body(v, 1)
            }
            _ => self.section_assign(kv.key, v),
        }
    }

    /// A single-line section: `path = value` (scalar or inline flow).
    fn section_assign(&mut self, anchor_key: NodeId, v: NodeId) -> Result<()> {
        self.begin_section(false, Some(anchor_key))?;
        self.write_path()?;
        match self.ast.kind_of(v) {
            NodeKind::Mapping(_) | NodeKind::Sequence(_) => {
                self.write_all(b" = ");
                self.flow_value(v)?;
            }
            _ => {
                self.write_type_annotation(v);
                self.write_all(b" = ");
                self.scalar_and_comment(v)?;
                self.write_byte(b'\n');
                return Ok(());
            }
        }
        self.trailing_comment(v);
        self.write_byte(b'\n');
        Ok(())
    }

    /// A section whose value is a multi-line flow list: `path = [` then one
    /// element per line then `]`. Deliberately NOT blank-line separated
    /// (`multiline = false`): a stacked list is still one assignment, so it
    /// packs tight with its scalar neighbors — the frontmatter look.
    fn section_multiline_flow_seq(&mut self, anchor_key: NodeId, v: NodeId) -> Result<()> {
        self.begin_section(false, Some(anchor_key))?;
        self.write_path()?;
        self.write_all(b" = ");
        self.multiline_flow_seq(v, 0, Some(v))?;
        self.write_byte(b'\n');
        Ok(())
    }

    /// `path[]` for the first element, `+` for each further one. Emitted as ONE
    /// section (blank lines around the group, none inside it).
    fn emit_append_group(&mut self, seq_id: NodeId) -> Result<()> {
        let mut cur = self.sequence_first(seq_id);
        let mut i = 0usize;
        while let Some(el) = cur {
            if i == 0 {
                self.begin_section(true, None)?;
            }
            self.leading_comments(el, 0)?;
            if i == 0 {
                self.write_path()?;
                self.write_all(b"[]");
            } else {
                self.write_byte(b'+');
            }
            self.trailing_comment(el);
            self.write_byte(b'\n');
            self.map_body(el, 1)?;
            cur = self.next_of(el);
            i += 1;
        }
        Ok(())
    }

    /// Blank-line separation, then the section's leading comments (anchored to
    /// `anchor` — the key a re-parse of the section's first line binds them
    /// to).
    fn begin_section(&mut self, multiline: bool, anchor: Option<NodeId>) -> Result<()> {
        let has_lead = match anchor {
            Some(a) => self.comments_on() && self.leading_count(a) > 0,
            None => false,
        };
        let ml = multiline || has_lead;
        if self.started && (ml || self.prev_multiline) {
            self.write_byte(b'\n');
        }
        self.started = true;
        self.prev_multiline = ml;
        if let Some(a) = anchor {
            self.leading_comments(a, 0)?;
        }
        Ok(())
    }

    fn write_path(&mut self) -> Result<()> {
        for i in 0..self.path.len() {
            if i > 0 {
                self.write_byte(b'.');
            }
            let key_id = self.path[i];
            self.write_key(key_id)?;
        }
        Ok(())
    }

    fn path_width(&self) -> usize {
        let mut w = if !self.path.is_empty() { self.path.len() - 1 } else { 0 }; // dots
        for &key_id in &self.path {
            w += self.key_width(key_id);
        }
        w
    }

    /// Would this entry render as a single body line under a header at depth 1
    /// (scalar, or a container fitting inline)? Deep children — anything
    /// needing its own nested body — get hoisted to their own sections
    /// instead.
    fn is_flat_child(&self, kv_id: NodeId) -> bool {
        let chain = self.resolve_chain(kv_id);
        let end = self.kv_of(chain.end_kv);
        match self.ast.kind_of(end.value) {
            NodeKind::Mapping(_) => self.fits_inline(end.value, map_entry_prefix_width(1, chain.key_width), CommentPolicy::AllowTrailing),
            // Past the item-count threshold a sequence stacks (multiple
            // lines), so it is not flat — `seq_fits_inline` is the shared rule
            // map_entry_line uses.
            NodeKind::Sequence(_) => self.seq_fits_inline(end.value, map_entry_prefix_width(1, chain.key_width), CommentPolicy::AllowTrailing),
            _ => true,
        }
    }

    fn all_elements_mappings(&self, seq_id: NodeId) -> bool {
        let mut cur = self.sequence_first(seq_id);
        while let Some(el) = cur {
            if !matches!(self.ast.kind_of(el), NodeKind::Mapping(_)) {
                return false;
            }
            cur = self.next_of(el);
        }
        true
    }

    fn seq_len(&self, seq_id: NodeId) -> usize {
        let mut n = 0usize;
        let mut cur = self.sequence_first(seq_id);
        while let Some(el) = cur {
            n += 1;
            cur = self.next_of(el);
        }
        n
    }

    /// A sequence renders as inline flow (`[a, b, c]`) iff it is under the
    /// item-count threshold AND its inline spelling fits the width budget at
    /// `prefix_width`. The single home for this rule.
    fn seq_fits_inline(&self, seq_id: NodeId, prefix_width: usize, policy: CommentPolicy) -> bool {
        self.seq_len(seq_id) <= MAX_INLINE_SEQ_ITEMS && self.fits_inline(seq_id, prefix_width, policy)
    }

    fn has_mapping_element(&self, seq_id: NodeId) -> bool {
        let mut cur = self.sequence_first(seq_id);
        while let Some(el) = cur {
            if matches!(self.ast.kind_of(el), NodeKind::Mapping(_)) {
                return true;
            }
            cur = self.next_of(el);
        }
        false
    }

    /// Is this sequence a candidate for MULTI-LINE flow (`[` / one element per
    /// line / `]`) rather than `> *` block lines? Requires a non-empty list,
    /// no direct mapping element, no element that flow would quote but block
    /// leaves bare (the quote-avoidance rule below), and full
    /// flow-representability with no comment dropped.
    fn multiline_flow_eligible(&self, seq_id: NodeId, policy: CommentPolicy) -> bool {
        if self.sequence_first(seq_id).is_none() {
            return false; // empty → `= []`
        }
        if self.has_mapping_element(seq_id) {
            return false;
        }
        if self.flow_quotes_what_block_leaves_bare(seq_id) {
            return false;
        }
        self.inline_width(seq_id, policy).is_some()
    }

    /// The quote-avoidance rule (DESIGN.md "Multi-line flow"): would the flow
    /// spelling force quotes onto an element that block position renders bare?
    /// Prose with a top-level comma is the common case. Elements needing
    /// quotes in BOTH positions don't count against flow. Recurses into nested
    /// sequences.
    fn flow_quotes_what_block_leaves_bare(&self, id: NodeId) -> bool {
        match self.ast.kind_of(id) {
            NodeKind::String(s) => !is_bare_safe(s, false, Pos::List) && is_bare_safe(s, false, Pos::Block),
            NodeKind::Sequence(first_opt) => {
                let mut cur = first_opt;
                while let Some(el) = cur {
                    if self.flow_quotes_what_block_leaves_bare(el) {
                        return true;
                    }
                    cur = self.next_of(el);
                }
                false
            }
            _ => false,
        }
    }

    /// Render a sequence as multi-line flow: `[`, then each element on its own
    /// line (two-space cosmetic indent, trailing comma), then `]` back at the
    /// opening line's `indent`. The interior is a suspended flow region — the
    /// re-parse ignores the indentation. `comment_anchor`'s trailing comment
    /// (if any) prints after the opening `[` — the multiline-string opener
    /// rule's flow twin. Each element re-decides fits-or-breaks independently.
    fn multiline_flow_seq(&mut self, seq_id: NodeId, indent: usize, comment_anchor: Option<NodeId>) -> Result<()> {
        self.write_byte(b'[');
        if let Some(a) = comment_anchor {
            self.trailing_comment(a);
        }
        self.write_byte(b'\n');
        let inner = indent + 2;
        let mut cur = self.sequence_first(seq_id);
        while let Some(el) = cur {
            for _ in 0..inner {
                self.write_byte(b' ');
            }
            let nested_nonempty = matches!(self.ast.kind_of(el), NodeKind::Sequence(Some(_)));
            if nested_nonempty && !self.seq_fits_inline(el, inner + 1, CommentPolicy::Strict) {
                // +1: the trailing comma
                self.multiline_flow_seq(el, inner, None)?;
            } else {
                self.value(el, Pos::List)?;
            }
            self.write_all(b",\n");
            cur = self.next_of(el);
        }
        for _ in 0..indent {
            self.write_byte(b' ');
        }
        self.write_byte(b']');
        Ok(())
    }

    // ── Block bodies ─────────────────────────────────────────────────────────

    fn body(&mut self, id: NodeId, depth: usize) -> Result<()> {
        match self.ast.kind_of(id) {
            NodeKind::Mapping(_) => self.map_body(id, depth),
            NodeKind::Sequence(_) => self.seq_body(id, depth),
            // `body` is only reached for containers today, but a scalar
            // arriving from a hand-built or cross-format AST is recoverable.
            _ => Err(err!(UnexpectedNodeKind)),
        }
    }

    /// Emit every entry of map `id` as block lines at `depth` (then the map's
    /// own dangling comment run).
    fn map_body(&mut self, map_id: NodeId, depth: usize) -> Result<()> {
        let mut cur = self.mapping_first(map_id);
        while let Some(kv_id) = cur {
            self.map_entry_line(kv_id, depth)?;
            cur = self.next_of(kv_id);
        }
        for c in self.dangling_of(map_id) {
            self.comment_lines(c, depth)?;
        }
        Ok(())
    }

    /// One map entry as block line(s) at `depth`: leading comments, marker
    /// run, (collapsed) dotted key, then value — inline, nested body, or
    /// scalar.
    fn map_entry_line(&mut self, kv_id: NodeId, depth: usize) -> Result<()> {
        let chain = self.resolve_chain(kv_id);
        let end = self.kv_of(chain.end_kv);
        self.leading_comments(end.key, depth)?;
        self.write_markers(depth);
        self.write_chain_keys(kv_id, chain.end_kv)?;
        let v = end.value;
        let prefix_width = map_entry_prefix_width(depth, chain.key_width);
        match self.ast.kind_of(v) {
            NodeKind::Mapping(_) => {
                if self.fits_inline(v, prefix_width, CommentPolicy::AllowTrailing) {
                    self.write_all(b" = ");
                    self.flow_value(v)?;
                    self.trailing_comment(v);
                    self.write_byte(b'\n');
                } else {
                    // Nested container header (`> key`, children one deeper).
                    self.trailing_comment(v);
                    self.write_byte(b'\n');
                    self.body(v, depth + 1)?;
                }
            }
            NodeKind::Sequence(_) => {
                if self.seq_fits_inline(v, prefix_width, CommentPolicy::AllowTrailing) {
                    self.write_all(b" = ");
                    self.flow_value(v)?;
                    self.trailing_comment(v);
                    self.write_byte(b'\n');
                } else if self.multiline_flow_eligible(v, CommentPolicy::AllowTrailing) {
                    self.write_all(b" = ");
                    self.multiline_flow_seq(v, 2 * depth, Some(v))?;
                    self.write_byte(b'\n');
                } else {
                    // Nested sequence header (`> key`, `>> *` elements deeper).
                    self.trailing_comment(v);
                    self.write_byte(b'\n');
                    self.body(v, depth + 1)?;
                }
            }
            _ => {
                self.write_type_annotation(v);
                self.write_all(b" = ");
                self.scalar_and_comment(v)?;
                self.write_byte(b'\n');
            }
        }
        Ok(())
    }

    /// Emit every element of sequence `id` as `> *` lines at `depth` (bare `*`
    /// at the root), then the sequence's own dangling comment run.
    fn seq_body(&mut self, seq_id: NodeId, depth: usize) -> Result<()> {
        let mut cur = self.sequence_first(seq_id);
        while let Some(el) = cur {
            self.leading_comments(el, depth)?;
            self.write_element_markers(depth);
            match self.ast.kind_of(el) {
                NodeKind::Mapping(_) => {
                    // Element leading comments are already printed, so they
                    // don't block the inline form.
                    if self.fits_inline(el, element_prefix_width(depth), CommentPolicy::AllowLeadingTrailing) {
                        self.write_byte(b' ');
                        self.flow_value(el)?;
                        self.trailing_comment(el);
                        self.write_byte(b'\n');
                    } else {
                        self.trailing_comment(el);
                        self.write_byte(b'\n');
                        self.body(el, depth + 1)?;
                    }
                }
                NodeKind::Sequence(_) => {
                    if self.seq_fits_inline(el, element_prefix_width(depth), CommentPolicy::AllowLeadingTrailing) {
                        self.write_byte(b' ');
                        self.flow_value(el)?;
                        self.trailing_comment(el);
                        self.write_byte(b'\n');
                    } else if self.multiline_flow_eligible(el, CommentPolicy::AllowLeadingTrailing) {
                        self.write_byte(b' ');
                        self.multiline_flow_seq(el, element_prefix_width(depth), Some(el))?;
                        self.write_byte(b'\n');
                    } else {
                        self.trailing_comment(el);
                        self.write_byte(b'\n');
                        self.body(el, depth + 1)?;
                    }
                }
                _ => {
                    let annotated = self.write_type_annotation(el);
                    self.write_all(if annotated { b" = " } else { b" " });
                    self.scalar_and_comment(el)?;
                    self.write_byte(b'\n');
                }
            }
            cur = self.next_of(el);
        }
        for c in self.dangling_of(seq_id) {
            self.comment_lines(c, depth)?;
        }
        Ok(())
    }

    // ── Layout decisions ─────────────────────────────────────────────────────

    fn budget(&self) -> usize {
        self.options.width as usize
    }

    fn fits_inline(&self, id: NodeId, prefix_width: usize, policy: CommentPolicy) -> bool {
        match self.inline_width(id, policy) {
            Some(w) => prefix_width + w <= self.budget(),
            None => false,
        }
    }

    /// Width of `id` rendered as inline flow, or `None` when it is not
    /// flow-eligible: a comment would be dropped or re-anchored (dangling
    /// always; leading/trailing per `policy`), the node has no flow spelling
    /// (enum literal, non-finite float, char literal, alias), or an object
    /// directly contains another object.
    ///
    /// `policy` gates ONLY `id`'s own comments; everything else is a pure
    /// function of `id` (children always recurse with `Strict`), factored into
    /// the memoized `structural_inline_width`.
    fn inline_width(&self, id: NodeId, policy: CommentPolicy) -> Option<usize> {
        if self.comments_on() {
            let (leading, trailing, dangling) = self.comment_counts(id);
            if dangling > 0 {
                return None;
            }
            match policy {
                CommentPolicy::Strict => {
                    if leading > 0 || trailing {
                        return None;
                    }
                }
                CommentPolicy::AllowTrailing => {
                    if leading > 0 {
                        return None;
                    }
                }
                CommentPolicy::AllowLeadingTrailing => {}
            }
        }
        self.structural_inline_width(id)
    }

    /// The comment-policy-independent core of `inline_width`, memoized by node
    /// id in `width_memo`.
    fn structural_inline_width(&self, id: NodeId) -> Option<usize> {
        if let Some(m) = self.width_memo.get(id as usize)
            && m.get().done
        {
            return m.get().width;
        }
        let w = self.compute_inline_width(id);
        if let Some(m) = self.width_memo.get(id as usize) {
            m.set(WidthMemo { done: true, width: w });
        }
        w
    }

    fn compute_inline_width(&self, id: NodeId) -> Option<usize> {
        // A tag-annotated scalar needs `: type =`, which only exists in block
        // position — so it forces its container out of inline flow. A tag the
        // printer DROPS (see `emittable_kind_tag`) doesn't count.
        if self.emittable_kind_tag(id).is_some() {
            return None;
        }
        match self.ast.kind_of(id) {
            NodeKind::Null => Some(4),
            NodeKind::Boolean(b) => Some(if b { 4 } else { 5 }),
            // A non-finite float needs `: float =`, like `NumberSpecial` below.
            NodeKind::Number(n) => {
                if num::non_finite(n.raw).is_some() {
                    None
                } else {
                    Some(n.raw.len())
                }
            }
            // A multi-line string deserves a real `'''`/`"""` block, which only
            // exists in block position — so it forces its container out of
            // flow.
            NodeKind::String(s) => {
                if s.contains(&b'\n') {
                    None
                } else {
                    Some(scalar_string_width(s, false, Pos::List))
                }
            }
            NodeKind::Extended(e) => match e.kind {
                ExtKind::OffsetDatetime | ExtKind::LocalDatetime | ExtKind::LocalDate | ExtKind::LocalTime => Some(e.text.len()),
                // enum/char/non-finite need a `: type =` annotation, which
                // exists only in block position — so none is flow-eligible.
                _ => None,
            },
            NodeKind::Alias(_) | NodeKind::KeyValue(_) => None,
            NodeKind::Sequence(first_opt) => {
                let mut w = 2usize; // "[" + "]"
                let mut cur = first_opt;
                let mut i = 0usize;
                while let Some(el) = cur {
                    let ew = self.inline_width(el, CommentPolicy::Strict)?;
                    w += ew + if i > 0 { 2 } else { 0 }; // ", "
                    cur = self.next_of(el);
                    i += 1;
                }
                Some(w)
            }
            NodeKind::Mapping(first_opt) => {
                if first_opt.is_none() {
                    return Some(2); // "{}"
                }
                let mut w = 2usize; // "{" + "}"
                let mut cur = first_opt;
                let mut i = 0usize;
                while let Some(kv_id) = cur {
                    let kv = self.kv_of(kv_id);
                    if self.comments_on() && self.leading_count(kv.key) > 0 {
                        return None;
                    }
                    if matches!(self.ast.kind_of(kv.value), NodeKind::Mapping(_)) {
                        return None; // object-in-object
                    }
                    let vw = self.compact_value_width(kv.value)?;
                    w += self.key_width(kv.key) + 1 + vw + if i > 0 { 1 } else { 0 }; // "=", " "
                    cur = self.next_of(kv_id);
                    i += 1;
                }
                Some(w)
            }
        }
    }

    /// `inline_width` for a `{…}` object's value: a string there is one
    /// token, so its bare-or-quoted width is the compact one
    /// (`is_bare_safe(Compact)`).
    fn compact_value_width(&self, id: NodeId) -> Option<usize> {
        let w = self.inline_width(id, CommentPolicy::Strict)?;
        match self.ast.kind_of(id) {
            NodeKind::String(s) => Some(scalar_string_width(s, false, Pos::Compact)),
            _ => Some(w),
        }
    }

    /// Deepest `>` count `map_body(map_id, depth)` would emit — used to decide
    /// header-vs-hoist at section level. Mirrors map_body's decisions exactly.
    fn block_depth_map(&self, map_id: NodeId, depth: usize) -> usize {
        let mut maxd = depth;
        let mut cur = self.mapping_first(map_id);
        while let Some(kv_id) = cur {
            cur = self.next_of(kv_id);
            let chain = self.resolve_chain(kv_id);
            let end = self.kv_of(chain.end_kv);
            match self.ast.kind_of(end.value) {
                NodeKind::Mapping(_) => {
                    if self.fits_inline(end.value, map_entry_prefix_width(depth, chain.key_width), CommentPolicy::AllowTrailing) {
                        continue;
                    }
                    maxd = maxd.max(self.block_depth_map(end.value, depth + 1));
                }
                NodeKind::Sequence(_) => {
                    if self.fits_inline(end.value, map_entry_prefix_width(depth, chain.key_width), CommentPolicy::AllowTrailing) {
                        continue;
                    }
                    if self.multiline_flow_eligible(end.value, CommentPolicy::AllowTrailing) {
                        continue; // flow value, no block depth
                    }
                    maxd = maxd.max(self.block_depth_seq(end.value, depth + 1));
                }
                _ => {}
            }
        }
        maxd
    }

    fn block_depth_seq(&self, seq_id: NodeId, depth: usize) -> usize {
        let mut maxd = depth;
        let mut cur = self.sequence_first(seq_id);
        while let Some(el) = cur {
            cur = self.next_of(el);
            match self.ast.kind_of(el) {
                NodeKind::Mapping(_) => {
                    if self.fits_inline(el, element_prefix_width(depth), CommentPolicy::AllowLeadingTrailing) {
                        continue;
                    }
                    maxd = maxd.max(self.block_depth_map(el, depth + 1));
                }
                NodeKind::Sequence(_) => {
                    if self.fits_inline(el, element_prefix_width(depth), CommentPolicy::AllowLeadingTrailing) {
                        continue;
                    }
                    if self.multiline_flow_eligible(el, CommentPolicy::AllowLeadingTrailing) {
                        continue; // flow value, no block depth
                    }
                    maxd = maxd.max(self.block_depth_seq(el, depth + 1));
                }
                _ => {}
            }
        }
        maxd
    }

    /// Walk the dotted-key collapse chain starting at `kv_id`: follow
    /// single-child maps as far as comment anchors allow, accumulating the
    /// dotted key width.
    fn resolve_chain(&self, kv_id: NodeId) -> Chain {
        let mut cur = kv_id;
        let mut w = self.key_width(self.kv_of(cur).key);
        while let Some(child) = self.collapse_child(cur) {
            cur = child;
            w += 1 + self.key_width(self.kv_of(cur).key);
        }
        Chain { end_kv: cur, key_width: w }
    }

    /// If `kv_id`'s value is a single-entry map that a dotted-key collapse may
    /// step through, return that inner keyvalue. Blocked by any comment whose
    /// anchor a re-parse of the collapsed spelling would move: the outer key's
    /// leading run (it would re-anchor onto the chain's final key) and the
    /// intermediate map's own comments (its line disappears entirely).
    fn collapse_child(&self, kv_id: NodeId) -> Option<NodeId> {
        let kv = self.kv_of(kv_id);
        match self.ast.kind_of(kv.value) {
            NodeKind::Mapping(first_opt) => {
                let first = first_opt?;
                if self.next_of(first).is_some() {
                    return None;
                }
                if self.comments_on() {
                    if self.leading_count(kv.key) > 0 {
                        return None;
                    }
                    let (l, t, d) = self.comment_counts(kv.value);
                    if l > 0 || t || d > 0 {
                        return None;
                    }
                }
                Some(first)
            }
            _ => None,
        }
    }

    /// Write the dotted keys of a collapse chain: `kv_id`'s key through
    /// `end_kv`'s, joined by `.`.
    fn write_chain_keys(&mut self, kv_id: NodeId, end_kv: NodeId) -> Result<()> {
        let mut cur = kv_id;
        self.write_key(self.kv_of(cur).key)?;
        while cur != end_kv {
            cur = self.collapse_child(cur).unwrap();
            self.write_byte(b'.');
            self.write_key(self.kv_of(cur).key)?;
        }
        Ok(())
    }

    // ── Comment queries ──────────────────────────────────────────────────────

    fn comments_on(&self) -> bool {
        !self.options.strip_comments
    }

    /// How many leading comments `id` carries, whether it has a trailing one,
    /// and how many dangling ones — read from the comment runs without
    /// building the comments.
    fn comment_counts(&self, id: NodeId) -> (usize, bool, usize) {
        let runs = self.ast.comment_runs();
        let mut lo = runs.partition_point(|r| r.node < id);
        let (mut leading, mut trailing, mut dangling) = (0usize, false, 0usize);
        while lo < runs.len() && runs[lo].node == id {
            let r = runs[lo];
            match r.slot {
                table::COMMENT_LEADING => leading = r.len as usize,
                table::COMMENT_TRAILING => trailing = r.len > 0,
                _ => dangling = r.len as usize,
            }
            lo += 1;
        }
        (leading, trailing, dangling)
    }

    fn leading_count(&self, id: NodeId) -> usize {
        self.comment_counts(id).0
    }

    fn dangling_of(&self, id: NodeId) -> Vec<Comment<'a>> {
        if !self.comments_on() {
            return Vec::new();
        }
        self.ast.comments(id).dangling.into_owned()
    }

    fn key_has_leading(&self, key_id: NodeId) -> bool {
        self.comments_on() && self.leading_count(key_id) > 0
    }

    fn has_trailing_or_dangling(&self, id: NodeId) -> bool {
        if !self.comments_on() {
            return false;
        }
        let (_, t, d) = self.comment_counts(id);
        t || d > 0
    }

    // ── Line primitives ──────────────────────────────────────────────────────

    /// Opt-in (`options.indent_requested`) cosmetic indentation ahead of a
    /// marker run: `2 × depth` literal spaces, agreeing with the `> ` markers
    /// that alone carry the real parse depth (docs/spec.md § 3.3) — purely
    /// visual, never emitted by default.
    fn write_cosmetic_indent(&mut self, depth: usize) {
        if self.options.indent_requested {
            for _ in 0..2 * depth {
                self.write_byte(b' ');
            }
        }
    }

    /// The spaced marker run for a key/comment line: `"> "` per level, so the
    /// last space doubles as the marker↔key separator (`> > key`).
    fn write_markers(&mut self, depth: usize) {
        self.write_cosmetic_indent(depth);
        for _ in 0..depth {
            self.write_all(b"> ");
        }
    }

    /// The marker run for an element line: the depth run, then the `*` bullet
    /// in its own cell (`> > *`); a root element is a bare `*`.
    fn write_element_markers(&mut self, depth: usize) {
        self.write_cosmetic_indent(depth);
        for _ in 0..depth {
            self.write_all(b"> ");
        }
        self.write_byte(b'*');
    }

    fn write_key(&mut self, key_id: NodeId) -> Result<()> {
        let name = match self.ast.kind_of(key_id) {
            NodeKind::String(s) => s,
            _ => return Err(err!(NonStringKey)),
        };
        self.write_bare_or_quoted(name, true, Pos::Block);
        Ok(())
    }

    fn key_width(&self, key_id: NodeId) -> usize {
        match self.ast.kind_of(key_id) {
            NodeKind::String(s) => scalar_string_width(s, true, Pos::Block),
            _ => 0, // fails with NonStringKey at write time regardless
        }
    }

    /// The kind type tag on `id` (`AST::tag_of`), or `None` when the node is
    /// untagged or carries a verbatim text tag (a YAML custom tag fig can't
    /// spell). This is how a fig `: type =` annotation round-trips.
    fn kind_tag_of(&self, id: NodeId) -> Option<KindTag> {
        match self.ast.tag_of(id)? {
            Tag::Kind(k) => Some(k),
            Tag::Text(_) => None,
        }
    }

    /// Whether `id` is an infinite or NaN float, which figl spells only after
    /// a `: float =` annotation: a `NumberSpecial`, or a number whose lexeme
    /// is another format's spelling of one.
    fn is_non_finite(&self, id: NodeId) -> bool {
        match self.ast.kind_of(id) {
            NodeKind::Number(n) => num::non_finite(n.raw).is_some(),
            NodeKind::Extended(e) => e.kind == ExtKind::NumberSpecial,
            _ => false,
        }
    }

    fn is_string_tagged(&self, id: NodeId) -> bool {
        self.emittable_kind_tag(id) == Some(KindTag::String)
    }

    /// The kind tag `write_type_annotation` will actually re-emit for `id`:
    /// `kind_tag_of` minus any `String` tag whose value the all-raw `: string`
    /// form cannot carry. The annotated RHS is read VERBATIM to end of line
    /// (the total sink), so a string value with no bare spelling
    /// (`is_total_sink_safe` false) must DROP its tag and take the ordinary
    /// untagged quoted/multiline form instead of corrupting the value.
    fn emittable_kind_tag(&self, id: NodeId) -> Option<KindTag> {
        let k = self.kind_tag_of(id)?;
        if k == KindTag::String
            && let NodeKind::String(s) = self.ast.kind_of(id)
            && !is_total_sink_safe(s)
        {
            return None;
        }
        Some(k)
    }

    /// Write an explicit `: type` annotation for `id`, if it needs one. Two
    /// sources: a stored kind tag (a fig `: int/float/string/bool =` that
    /// round-trips via `AST::tag_of`), or an `Extended` scalar with no bare
    /// spelling (`EnumLiteral`/`NumberSpecial`, DESIGN.md "Enum:
    /// explicit-only"). Returns whether an annotation was written (an
    /// annotated line needs ` = ` where a plain one has a space).
    fn write_type_annotation(&mut self, id: NodeId) -> bool {
        if let Some(k) = self.emittable_kind_tag(id) {
            let a: Option<&[u8]> = match k {
                KindTag::Integer => Some(b": int"),
                KindTag::Float => Some(b": float"),
                KindTag::String => Some(b": string"),
                KindTag::Boolean => Some(b": bool"),
                // null/seq/map carry no fig `: type =` spelling.
                KindTag::Null | KindTag::Sequence | KindTag::Mapping => None,
            };
            if let Some(a) = a {
                self.write_all(a);
                return true;
            }
        }
        match self.ast.kind_of(id) {
            // A non-finite float from another format (`.inf`, `Infinity`):
            // bare, figl reads every spelling of it as a string.
            NodeKind::Number(n) => {
                if num::non_finite(n.raw).is_some() {
                    self.write_all(b": float");
                    true
                } else {
                    false
                }
            }
            NodeKind::Extended(e) => match e.kind {
                ExtKind::EnumLiteral => {
                    self.write_all(b": enum");
                    true
                }
                ExtKind::CharLiteral => {
                    self.write_all(b": char");
                    true
                }
                ExtKind::NumberSpecial => {
                    self.write_all(b": float");
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// A scalar (or, in flow position, any) value.
    fn value(&mut self, id: NodeId, pos: Pos) -> Result<()> {
        let in_flow = pos != Pos::Block;
        match self.ast.kind_of(id) {
            NodeKind::Null => self.write_all(b"null"),
            NodeKind::Boolean(b) => self.write_all(if b { b"true" } else { b"false" }),
            // A non-finite float takes the annotated parser's own words, after
            // the `: float` `write_type_annotation` wrote. Flow has no
            // annotation.
            NodeKind::Number(n) => match num::non_finite(n.raw) {
                Some(v) => {
                    if in_flow {
                        return Err(err!(NonFiniteNumber));
                    }
                    self.write_all(num::PLAIN_NON_FINITE.of(v));
                }
                None => self.write_all(n.raw),
            },
            NodeKind::String(s) => {
                if !in_flow && s.contains(&b'\n') {
                    self.write_multiline_string(s, None);
                } else if !in_flow && self.is_string_tagged(id) {
                    // A `: string =`-tagged value re-emits BARE: the annotated
                    // RHS is read verbatim (the total sink), so no quotes are
                    // needed. `is_string_tagged` is already gated on
                    // `is_total_sink_safe`.
                    self.write_all(s);
                } else {
                    self.write_bare_or_quoted(s, false, pos);
                }
            }
            NodeKind::Extended(e) => {
                if in_flow && e.kind == ExtKind::NumberSpecial {
                    return Err(err!(NonFiniteNumber));
                }
                self.write_extended(e);
            }
            NodeKind::Mapping(_) | NodeKind::Sequence(_) => self.flow_value(id)?,
            NodeKind::KeyValue(_) => unreachable!("a keyvalue is not a value"),
            NodeKind::Alias(_) => return Err(err!(UnresolvedAlias)),
        }
        Ok(())
    }

    /// A container in flow spelling: `[a, b]` / `{x=1 y=2}` (compact pairs).
    fn flow_value(&mut self, id: NodeId) -> Result<()> {
        match self.ast.kind_of(id) {
            NodeKind::Sequence(first_opt) => {
                self.write_byte(b'[');
                let mut cur = first_opt;
                let mut i = 0usize;
                while let Some(el) = cur {
                    if i > 0 {
                        self.write_all(b", ");
                    }
                    self.value(el, Pos::List)?;
                    cur = self.next_of(el);
                    i += 1;
                }
                self.write_byte(b']');
                Ok(())
            }
            NodeKind::Mapping(first_opt) => {
                if first_opt.is_none() {
                    self.write_all(b"{}");
                    return Ok(());
                }
                self.write_byte(b'{');
                let mut cur = first_opt;
                let mut i = 0usize;
                while let Some(kv_id) = cur {
                    if i > 0 {
                        self.write_byte(b' ');
                    }
                    let kv = self.kv_of(kv_id);
                    self.write_key(kv.key)?;
                    self.write_byte(b'=');
                    self.value(kv.value, Pos::Compact)?;
                    cur = self.next_of(kv_id);
                    i += 1;
                }
                self.write_byte(b'}');
                Ok(())
            }
            // Only containers have a flow spelling; a non-container reaching
            // here from a foreign AST is recoverable.
            _ => Err(err!(UnexpectedNodeKind)),
        }
    }

    fn write_extended(&mut self, e: Extended<'_>) {
        match e.kind {
            ExtKind::OffsetDatetime | ExtKind::LocalDatetime | ExtKind::LocalDate | ExtKind::LocalTime => self.write_all(e.text),
            ExtKind::EnumLiteral | ExtKind::NumberSpecial => self.write_all(e.text),
            // A char literal renders as the ZON-style `'A'` atom, paired with
            // the `: char =` annotation `write_type_annotation` emits.
            ExtKind::CharLiteral => write_char_literal(self.writer, e.text),
            // plist date/data text is always flow-safe bare content — same
            // untagged raw-passthrough treatment as a datetime above.
            ExtKind::PlistDate | ExtKind::Data => self.write_all(e.text),
        }
    }

    /// A block-position scalar and its trailing comment: after the value, or
    /// for a multi-line string on its opener line (`x = ''' # note`), the one
    /// place spec § 4.6 gives it.
    fn scalar_and_comment(&mut self, id: NodeId) -> Result<()> {
        if let NodeKind::String(s) = self.ast.kind_of(id)
            && s.contains(&b'\n')
        {
            self.write_multiline_string(s, Some(id));
            return Ok(());
        }
        self.value(id, Pos::Block)?;
        self.trailing_comment(id);
        Ok(())
    }

    /// A string containing newlines, in block value position: `'''` raw when
    /// the content round-trips verbatim, else `"""` with `\`, `"`, and `\r`
    /// escaped. Content and closer are emitted flush-left. `comment_anchor`'s
    /// trailing comment, if any, goes on the opener line, where the value's
    /// comment lives (spec § 4.6).
    fn write_multiline_string(&mut self, s: &[u8], comment_anchor: Option<NodeId>) {
        let raw_ok = find(s, b"'''").is_none() && !s.contains(&b'\r');
        if raw_ok {
            self.write_all(b"'''");
            if let Some(a) = comment_anchor {
                self.trailing_comment(a);
            }
            self.write_byte(b'\n');
            self.write_all(s);
            self.write_all(b"\n'''");
            return;
        }
        self.write_all(b"\"\"\"");
        if let Some(a) = comment_anchor {
            self.trailing_comment(a);
        }
        self.write_byte(b'\n');
        for &c in s {
            match c {
                b'\\' => self.write_all(b"\\\\"),
                b'"' => self.write_all(b"\\\""),
                b'\r' => self.write_all(b"\\r"),
                _ => self.write_byte(c),
            }
        }
        self.write_all(b"\n\"\"\"");
    }

    /// Bare when the literal-else-string sniff would round-trip it unchanged
    /// (and, for a key, when it contains no structural character); otherwise
    /// the minimal quoted form that round-trips it — raw `'…'` (§ 4.5) when
    /// the content holds neither `'` nor a newline, else escaped `"…"`.
    fn write_bare_or_quoted(&mut self, s: &[u8], is_key: bool, pos: Pos) {
        if is_bare_safe(s, is_key, pos) {
            self.write_all(s);
            return;
        }
        if can_raw_quote(s) {
            self.write_byte(b'\'');
            self.write_all(s);
            self.write_byte(b'\'');
            return;
        }
        self.write_byte(b'"');
        for &c in s {
            match c {
                b'"' => self.write_all(b"\\\""),
                b'\\' => self.write_all(b"\\\\"),
                b'\n' => self.write_all(b"\\n"),
                b'\r' => self.write_all(b"\\r"),
                b'\t' => self.write_all(b"\\t"),
                _ => self.write_byte(c),
            }
        }
        self.write_byte(b'"');
    }

    fn leading_comments(&mut self, id: NodeId, depth: usize) -> Result<()> {
        if !self.comments_on() {
            return Ok(());
        }
        let leading = self.ast.comments(id).leading.into_owned();
        for c in leading {
            self.comment_lines(c, depth)?;
        }
        Ok(())
    }

    fn trailing_comment(&mut self, id: NodeId) {
        if !self.comments_on() {
            return;
        }
        let Some(c) = self.ast.comments(id).trailing else { return };
        self.write_all(b" #");
        if !c.text.is_empty() {
            self.write_byte(b' ');
            for &ch in c.text {
                self.write_byte(if ch == b'\n' { b' ' } else { ch });
            }
        }
    }

    fn comment_lines(&mut self, c: Comment<'_>, depth: usize) -> Result<()> {
        for line in c.text.split(|&b| b == b'\n') {
            self.write_markers(depth);
            self.write_byte(b'#');
            let trimmed = tok::trim(line, b" \t");
            if !trimmed.is_empty() {
                self.write_byte(b' ');
                self.write_all(trimmed);
            }
            self.write_byte(b'\n');
        }
        Ok(())
    }
}

/// Re-encode a `char_literal` from its stored decimal codepoint as `'A'`.
/// Mirrors the ZON printer: common escapes and printable ASCII emit directly,
/// anything else uses `'\u{...}'`. A codepoint that fails to parse (only if
/// minted oddly by another format) falls back to its numeric text.
fn write_char_literal(writer: &mut Vec<u8>, text: &[u8]) {
    let Some(cp) = tok::zig_parse_unsigned(text, 10, 21) else {
        writer.extend_from_slice(text);
        return;
    };
    writer.push(b'\'');
    match cp {
        0x27 => writer.extend_from_slice(b"\\'"),
        0x5c => writer.extend_from_slice(b"\\\\"),
        0x0a => writer.extend_from_slice(b"\\n"),
        0x0d => writer.extend_from_slice(b"\\r"),
        0x09 => writer.extend_from_slice(b"\\t"),
        0x20..=0x26 | 0x28..=0x5b | 0x5d..=0x7e => writer.push(cp as u8),
        _ => writer.extend_from_slice(format!("\\u{{{cp:x}}}").as_bytes()),
    }
    writer.push(b'\'');
}

/// Whether `s` survives verbatim inside a raw `'…'` (§ 4.5: MUST NOT contain
/// `'` or a newline — there is no escape to spell either one raw).
fn can_raw_quote(s: &[u8]) -> bool {
    !s.iter().any(|&c| c == b'\'' || c == b'\n')
}

fn quoted_width(s: &[u8]) -> usize {
    if can_raw_quote(s) {
        return s.len() + 2; // '…' — no escaping, just the two quotes
    }
    let mut w = 2usize;
    for &c in s {
        w += match c {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' => 2,
            _ => 1,
        };
    }
    w
}

fn scalar_string_width(s: &[u8], is_key: bool, pos: Pos) -> usize {
    if is_bare_safe(s, is_key, pos) { s.len() } else { quoted_width(s) }
}

/// Whether `s` survives verbatim as the RHS of a `: string =` assignment — the
/// total-sink form, which `scan_bare_rest_of_line` reads to end-of-line with
/// sniffing and flow-commitment BOTH off. Only the mechanical scan hazards
/// matter: a leading/trailing space is trimmed, a control char breaks the
/// line, and a `#` at line-start or after whitespace opens a comment. When
/// false, the printer quotes instead.
fn is_total_sink_safe(s: &[u8]) -> bool {
    if s.is_empty() {
        return false;
    }
    if s[0] == b' ' || s[s.len() - 1] == b' ' {
        return false;
    }
    if s.iter().any(|&c| c == b'\n' || c == b'\r' || c == b'\t') {
        return false;
    }
    let mut prev_ws = true; // line-start counts as "after whitespace"
    for &c in s {
        if c == b'#' && prev_ws {
            return false;
        }
        prev_ws = c == b' ';
    }
    true
}

fn is_bare_safe(s: &[u8], is_key: bool, pos: Pos) -> bool {
    let in_flow = pos != Pos::Block;
    if s.is_empty() {
        return false;
    }
    if s[0] == b' ' || s[s.len() - 1] == b' ' {
        return false;
    }
    for &c in s {
        if c == b'\n' || c == b'\r' || c == b'\t' {
            return false;
        }
        if is_key && !tok::is_bare_key_char(c) {
            return false;
        }
    }
    if is_key && (s[0] == b'-' || s[0] == b'>') {
        return false;
    }
    if !is_key {
        // A bare non-key value must round-trip through the literal-else-string
        // sniff as a plain string, and must not open a committed form.
        match s[0] {
            b'\'' | b'"' => return false, // opens a committed string form
            b'[' | b'{' => {
                // A leading bracket is bare-safe only when it re-parses as a
                // bare string — a balanced close with trailing content. In flow
                // position the same rule holds via the bracket-aware flow
                // scanner, so it short-circuits here.
                if in_flow {
                    return flow_leading_bracket_bare(s, pos == Pos::Compact);
                }
                if tok::classify_bracket_commit(s, 0) != BracketCommit::BareTrailing {
                    return false;
                }
            }
            _ => {}
        }
        if in_flow {
            for &c in s {
                match c {
                    // A bare flow value runs to the next `,`/`]`/`}` — any of
                    // these inside the string would truncate it.
                    b',' | b']' | b'}' => return false,
                    // A compact object's value is one token: whitespace ends it.
                    b' ' if pos == Pos::Compact => return false,
                    _ => {}
                }
            }
        }
        // A `#` at the start or after whitespace would re-parse as a comment
        // and truncate the bare value — quote to preserve it.
        let mut prev_ws = true;
        for &c in s {
            if c == b'#' && prev_ws {
                return false;
            }
            prev_ws = c == b' ' || c == b'\t';
        }
        if tok::sniff_bare(s) != Sniffed::String {
            return false;
        }
    }
    true
}

/// A flow value whose first char is `[`/`{` is bare-safe only as a
/// balanced-then-trailing string (markdown link, glob, regex) that the
/// bracket-aware flow scanner reproduces verbatim. That needs a tail after the
/// first balanced group and no depth-0 `,`/`]`/`}` or `#`-comment that would
/// truncate the value. `\n`/`\r`/`\t` and edge spaces are already rejected by
/// the caller.
fn flow_leading_bracket_bare(s: &[u8], compact: bool) -> bool {
    if tok::classify_flow_bracket(s, 0, compact) != BracketCommit::BareTrailing {
        return false;
    }
    if compact && s.iter().any(|&c| c == b' ' || c == b'\t') {
        return false;
    }
    let mut depth = 0usize;
    let mut prev_space = false;
    for &c in s {
        match c {
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            b',' => {
                if depth == 0 {
                    return false;
                }
            }
            b'#' => {
                if prev_space && depth == 0 {
                    return false;
                }
            }
            _ => {}
        }
        prev_space = c == b' ' || c == b'\t';
    }
    depth == 0
}

/// The index of `needle` in `hay` (`std.mem.indexOf`).
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fig_kit::ast::{Builder, Entry, PathSegment};
    use crate::parser;
    use crate::Type;

    fn parse(input: &str) -> AST {
        parser::parse_abstract(input.as_bytes(), Type::Fig).unwrap()
    }

    fn str(b: &[u8]) -> &str {
        std::str::from_utf8(b).unwrap()
    }

    fn expect_print(input: &str, expected: &str) {
        let ast = parse(input);
        let mut out = Vec::new();
        print(&mut out, &ast, SerializeOptions::default()).unwrap();
        assert_eq!(expected, str(&out));
    }

    fn expect_round_trip(input: &str) {
        let ast1 = parse(input);
        let mut out = Vec::new();
        print(&mut out, &ast1, SerializeOptions::default()).unwrap();
        let ast2 = parse(str(&out));
        assert!(ast1.eql(&ast2));
        // Idempotence: a second fmt pass must be byte-identical.
        let mut out2 = Vec::new();
        print(&mut out2, &ast2, SerializeOptions::default()).unwrap();
        assert_eq!(str(&out), str(&out2));
    }

    #[test]
    fn a_scalar_root_ast_errors_rather_than_emitting_non_conforming_output() {
        // Not reachable by parsing fig source, but reachable from a foreign
        // AST (e.g. `fig get scalar.json -o fig`, where `scalar.json` is just
        // `42`). A bare `42` line does NOT re-parse (a bare token at the
        // document root reads as an empty container header), so this must
        // hard error instead.
        let mut b = Builder::init();
        let root_id = b.add_number_raw(b"42", false).unwrap();
        let ast = b.finish(root_id).unwrap();
        let mut out = Vec::new();
        assert_eq!(err!(UnrepresentableRoot), print(&mut out, &ast, SerializeOptions::default()).unwrap_err());
        assert!(out.is_empty());

        // A root map/sequence is unaffected.
        let mut b2 = Builder::init();
        let key = b2.add_string(b"a").unwrap();
        let value = b2.add_number_raw(b"1", false).unwrap();
        let map_root = b2.add_mapping(&[Entry { key, value }]).unwrap();
        let ast2 = b2.finish(map_root).unwrap();
        let mut out2 = Vec::new();
        print(&mut out2, &ast2, SerializeOptions::default()).unwrap();
        assert_eq!("a = 1\n", str(&out2));
    }

    #[test]
    fn prints_a_flat_map() {
        expect_print("x = 1\ny = hello\n",
            concat!(
                "x = 1\n",
                "y = hello\n",
            )
        );
    }

    #[test]
    fn small_nested_maps_inline_single_child_chains_collapse_to_dotted_keys() {
        expect_print(
            concat!(
                "database\n",
                "> host = localhost\n",
                "> pool\n",
                ">> size = 10",
            )
        ,
            concat!(
                "database\n",
                "> host = localhost\n",
                "> pool.size = 10\n",
            )
        );
    }

    #[test]
    fn a_leaf_map_of_scalars_fits_inline_at_section_level() {
        expect_print(
            concat!(
                "point\n",
                "> x = 1\n",
                "> y = 2",
            )
        ,
            concat!(
                "point = {x=1 y=2}\n",
            )
        );
    }

    #[test]
    fn a_compact_object_quotes_any_value_that_is_not_one_token() {
        expect_print(
            concat!(
                "a = { name = \"Adam Harris\", n = 1, lead = \"#x\", sp = \"[a b](c)\" }\n",
                "b = { \"x\": hello world }",
            )
        ,
            concat!(
                "a = {name='Adam Harris' n=1 lead='#x' sp='[a b](c)'}\n",
                "b.x = hello world\n",
            )
        );
        // Glued `#`, a glued bracket tail, and a date-space-time stay bare.
        expect_round_trip(
            concat!(
                "c = {t=2026-07-01 09:00 tag=a#b href=[Blog](/x.md) 'a.b'=1}\n",
            )
        );
    }

    #[test]
    fn spaced_markers_no_indentation() {
        // Long values defeat the inline budget, forcing genuine nesting.
        expect_print(
            concat!(
                "server\n",
                "> description = this line is deliberately padded far past the eighty column budget so it cannot inline\n",
                "> limits\n",
                ">> connections = this line is also deliberately padded far past the eighty column budget so it stays put\n",
                ">> burst = and one more long sibling so the limits container cannot be collapsed to one dotted key line",
            )
        ,
            concat!(
                "server\n",
                "> description = this line is deliberately padded far past the eighty column budget so it cannot inline\n",
                "> limits\n",
                "> > connections = this line is also deliberately padded far past the eighty column budget so it stays put\n",
                "> > burst = and one more long sibling so the limits container cannot be collapsed to one dotted key line\n",
            )
        );
    }

    #[test]
    fn indent_requested_opt_in_cosmetic_indentation_is_2_x_depth_canonical_output_unaffected_both_re_parse_equal() {
        let input = concat!(
                "server\n",
                "> description = this line is deliberately padded far past the eighty column budget so it cannot inline\n",
                "> limits\n",
                ">> connections = this line is also deliberately padded far past the eighty column budget so it stays put\n",
                ">> burst = and one more long sibling so the limits container cannot be collapsed to one dotted key line",
            );
        let ast = parse(input);

        let mut out = Vec::new();
        print(&mut out, &ast, SerializeOptions { indent_requested: true, ..SerializeOptions::default() }).unwrap();
        assert_eq!(
            concat!(
                "server\n",
                "  > description = this line is deliberately padded far past the eighty column budget so it cannot inline\n",
                "  > limits\n",
                "    > > connections = this line is also deliberately padded far past the eighty column budget so it stays put\n",
                "    > > burst = and one more long sibling so the limits container cannot be collapsed to one dotted key line\n",
            )
        , str(&out));

        // Canonical (default) output is completely unchanged by the field's
        // existence — the default still means zero-indent, markers-only output.
        let mut out_plain = Vec::new();
        print(&mut out_plain, &ast, SerializeOptions::default()).unwrap();
        assert_eq!(
            concat!(
                "server\n",
                "> description = this line is deliberately padded far past the eighty column budget so it cannot inline\n",
                "> limits\n",
                "> > connections = this line is also deliberately padded far past the eighty column budget so it stays put\n",
                "> > burst = and one more long sibling so the limits container cannot be collapsed to one dotted key line\n",
            )
        , str(&out_plain));

        // Both re-parse to the same AST: indentation is cosmetic, the markers
        // alone carry parse depth (docs/spec.md § 3.3's "clean" convention).
        let ast_indented = parse(str(&out));
        let ast_plain = parse(str(&out_plain));
        assert!(ast_indented.eql(&ast_plain));
    }

    #[test]
    fn a_deep_map_hoists_into_dotted_sections_and_nested_lists_of_maps_into_headers() {
        expect_print(
            concat!(
                "workspace\n",
                "> resolver = \"2\"\n",
                "> metadata\n",
                ">> release\n",
                ">>> shared-version-key = a deliberately long value to keep the release table from fitting inline here\n",
                ">>> tag-message-etc = another deliberately long value to keep the release table from fitting inline\n",
                ">> replacements\n",
                ">>> *\n",
                ">>>> file = README.md\n",
                ">>>> search = a deliberately long pattern string so this element cannot fit in the inline flow budget",
            )
        ,
            concat!(
                "workspace.resolver = '2'\n",
                "\n",
                "workspace.metadata.release\n",
                "> shared-version-key = a deliberately long value to keep the release table from fitting inline here\n",
                "> tag-message-etc = another deliberately long value to keep the release table from fitting inline\n",
                "\n",
                "workspace.metadata.replacements[]\n",
                "> file = README.md\n",
                "> search = a deliberately long pattern string so this element cannot fit in the inline flow budget\n",
            )
        );
    }

    #[test]
    fn a_sequence_of_maps_prints_as_a_append_header_with_continuations() {
        expect_print(
            concat!(
                "replacements[]\n",
                "> file = README.md\n",
                "> search = a deliberately long pattern string so this element cannot fit in the inline flow budget\n",
                "replacements[]\n",
                "> file = CHANGELOG.md\n",
                "> search = another deliberately long pattern string so the sequence cannot fit inline either way",
            )
        ,
            concat!(
                "replacements[]\n",
                "> file = README.md\n",
                "> search = a deliberately long pattern string so this element cannot fit in the inline flow budget\n",
                "+\n",
                "> file = CHANGELOG.md\n",
                "> search = another deliberately long pattern string so the sequence cannot fit inline either way\n",
            )
        );
    }

    #[test]
    fn scalar_lists_inline_when_short_multi_line_flow_when_long() {
        // A short list stays inline; one that overflows the width budget stacks as
        // multi-line flow (one element per line) rather than `> *` block lines.
        // A stacked list is still one assignment, so it packs tight with its
        // neighbors — no blank-line separation.
        expect_print(
            concat!(
                "ports\n",
                "> * 1\n",
                "> * 2\n",
                "members\n",
                ">* crates/some_long_crate_name_one\n",
                ">* crates/some_long_crate_name_two\n",
                ">* crates/some_long_crate_name_three\n",
                ">* crates/some_long_crate_name_four",
            )
        ,
            concat!(
                "ports = [1, 2]\n",
                "members = [\n",
                "  crates/some_long_crate_name_one,\n",
                "  crates/some_long_crate_name_two,\n",
                "  crates/some_long_crate_name_three,\n",
                "  crates/some_long_crate_name_four,\n",
                "]\n",
            )
        );
    }

    #[test]
    fn a_list_of_more_than_six_short_items_stacks_even_when_it_would_fit_inline() {
        expect_print(
            "letters = [a, b, c, d, e, f, g]"
        ,
            concat!(
                "letters = [\n",
                "  a,\n",
                "  b,\n",
                "  c,\n",
                "  d,\n",
                "  e,\n",
                "  f,\n",
                "  g,\n",
                "]\n",
            )
        );
        // Exactly six still fits on one line.
        expect_print(
            "letters = [a, b, c, d, e, f]"
        ,
            concat!(
                "letters = [a, b, c, d, e, f]\n",
            )
        );
    }

    #[test]
    fn a_scalar_list_with_a_comment_on_an_element_keeps_the_block_form() {
        // A dropped comment is never worth a prettier shape, so a list whose
        // elements carry comments falls back to `> *` lines (multi-line flow
        // discards interior comments).
        expect_print(
            concat!(
                "members\n",
                "> # the first crate\n",
                "> * crates/some_long_crate_name_one\n",
                "> * crates/some_long_crate_name_two\n",
                "> * crates/some_long_crate_name_three\n",
                "> * crates/some_long_crate_name_four",
            )
        ,
            concat!(
                "members\n",
                "> # the first crate\n",
                "> * crates/some_long_crate_name_one\n",
                "> * crates/some_long_crate_name_two\n",
                "> * crates/some_long_crate_name_three\n",
                "> * crates/some_long_crate_name_four\n",
            )
        );
    }

    #[test]
    fn markdown_links_stay_bare_inside_multi_line_flow_the_frontmatter_case() {
        // The motivating example: a `contents` list of markdown links. Each link is
        // a balanced-then-trailing bare string — no `> *`, no quotes.
        expect_print(
            concat!(
                "contents\n",
                "> * [Archived Documents](</Archive/Archived documents.md>)\n",
                "> * [Blog](/Blog/Blog.md)\n",
                "> * [Creative Writing](</Creative Writing/Creative Writing.md>)\n",
                "> * [Daily Index](/Daily/daily_index.md)\n",
                "> * [Resume](/Resume.md)\n",
                "> * [Utility](/Utility/utility.md)\n",
                "> * [Writing For Today](</Writing for today.md>)",
            )
        ,
            concat!(
                "contents = [\n",
                "  [Archived Documents](</Archive/Archived documents.md>),\n",
                "  [Blog](/Blog/Blog.md),\n",
                "  [Creative Writing](</Creative Writing/Creative Writing.md>),\n",
                "  [Daily Index](/Daily/daily_index.md),\n",
                "  [Resume](/Resume.md),\n",
                "  [Utility](/Utility/utility.md),\n",
                "  [Writing For Today](</Writing for today.md>),\n",
                "]\n",
            )
        );
    }

    #[test]
    fn a_short_markdown_link_list_stays_inline_and_bare() {
        expect_print(
            "links = [\"[Blog](/Blog/Blog.md)\", \"[Resume](/Resume.md)\"]"
        ,
            concat!(
                "links = [[Blog](/Blog/Blog.md), [Resume](/Resume.md)]\n",
            )
        );
    }

    #[test]
    fn quote_avoidance_prose_with_commas_prefers_block_lines_over_stacked_flow() {
        // A top-level `,` terminates a bare flow value, so flow would force quotes
        // onto the second element; block position keeps both bare. Bare beats
        // quoted, so the list takes the `> *` form.
        expect_print(
            "random = [test of very long things in a list to see if it works, \"hello there, this is also a long test; hopefully it works\"]"
        ,
            concat!(
                "random\n",
                "> * test of very long things in a list to see if it works\n",
                "> * hello there, this is also a long test; hopefully it works\n",
            )
        );
        // A short comma-carrying list that FITS stays inline — quote-avoidance
        // only arbitrates between the two broken forms, never un-inlines.
        expect_print(
            "pair = [a, \"b, c\"]"
        ,
            concat!(
                "pair = [a, 'b, c']\n",
            )
        );
        // An element quoted in BOTH positions (`true` sniffs to bool bare) doesn't
        // count against flow: quoting is the same price either way.
        expect_print(
            "flags = [crates/some_long_crate_name_one, crates/some_long_crate_name_two, crates/some_long_crate_name_three, \"true\"]"
        ,
            concat!(
                "flags = [\n",
                "  crates/some_long_crate_name_one,\n",
                "  crates/some_long_crate_name_two,\n",
                "  crates/some_long_crate_name_three,\n",
                "  'true',\n",
                "]\n",
            )
        );
        // The rule recurses: a comma-prose string trapped inside a NESTED list
        // would be flow-quoted too, so the whole value goes block.
        expect_print(
            "notes = [[first long note about the thing, \"second note, with a comma inside it somewhere\"], [third]]"
        ,
            concat!(
                "notes\n",
                "> *\n",
                "> > * first long note about the thing\n",
                "> > * second note, with a comma inside it somewhere\n",
                "> * [third]\n",
            )
        );
    }

    #[test]
    fn a_multi_line_flow_list_s_trailing_comment_sits_on_the_opener_line() {
        // The `'''` opener rule's flow twin: `key = [  # c` parses the comment as
        // block-layer trailing on the sequence, and fmt emits it there — next to
        // the key, not after the far-away `]`. A comment written on the `]` line
        // migrates to the opener on the next fmt.
        let stacked = concat!(
                "contents = [ # this is a contents comment\n",
                "  crates/some_long_crate_name_one,\n",
                "  crates/some_long_crate_name_two,\n",
                "  crates/some_long_crate_name_three,\n",
                "  crates/some_long_crate_name_four,\n",
                "]\n",
            );
        expect_print(stacked, stacked);
        expect_print(
            concat!(
                "contents = [\n",
                "  crates/some_long_crate_name_one,\n",
                "  crates/some_long_crate_name_two,\n",
                "  crates/some_long_crate_name_three,\n",
                "  crates/some_long_crate_name_four,\n",
                "] # this is a contents comment",
            )
        , stacked);
        expect_round_trip(stacked);
    }

    #[test]
    fn nested_lists_inside_multi_line_flow_re_decide_fits_or_breaks() {
        // A nested list that overflows the width budget stacks recursively (two
        // more columns per level, closer back at the opener's indent); a short
        // nested sibling stays inline.
        expect_print(
            "pairs = [[crates/some_long_crate_name_one, crates/some_long_crate_name_two, crates/some_long_crate_name_three], [a, b]]"
        ,
            concat!(
                "pairs = [\n",
                "  [\n",
                "    crates/some_long_crate_name_one,\n",
                "    crates/some_long_crate_name_two,\n",
                "    crates/some_long_crate_name_three,\n",
                "  ],\n",
                "  [a, b],\n",
                "]\n",
            )
        );
    }

    #[test]
    fn multi_line_flow_packs_tight_between_scalar_assignments_frontmatter() {
        expect_print(
            concat!(
                "title = Adam's Archive\n",
                "contents = [one_long_crate_name_here, two_long_crate_name_here, three_long_crate_name_here]\n",
                "author = Adam Harris",
            )
        ,
            concat!(
                "title = Adam's Archive\n",
                "contents = [\n",
                "  one_long_crate_name_here,\n",
                "  two_long_crate_name_here,\n",
                "  three_long_crate_name_here,\n",
                "]\n",
                "author = Adam Harris\n",
            )
        );
    }

    #[test]
    fn multi_line_flow_lists_round_trip_bare_links_nested_lists_quoted_edges() {
        expect_round_trip(
            concat!(
                "contents = [\n",
                "  [Archived Documents](</Archive/Archived documents.md>),\n",
                "  [Blog](/Blog/Blog.md),\n",
                "  [Creative Writing](</Creative Writing/Creative Writing.md>),\n",
                "  \"a, b needs quotes\",\n",
                "  \"[a-flow-looking-string]\",\n",
                "  plain,\n",
                "]",
            )
        );
        // A list of nested lists stacks the outer level, keeps the inner ones inline.
        expect_round_trip(
            "grid = [[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12], [13, 14, 15], [16, 17, 18], [19, 20, 21]]"
        );
    }

    #[test]
    fn empty_containers_round_trip_via() {
        expect_print("a = {}\nb = []\n", "a = {}\nb = []\n");
    }

    #[test]
    fn empty_document_round_trips_as_empty_output() {
        // An empty file parses to an empty root map, which emits zero sections — so
        // it prints back to nothing, keeping `fig fmt new.figl` a no-op.
        expect_print("", "");
        // Comments-only stays comments-only (the dangling run re-anchors to root).
        expect_print("# just a note\n", "# just a note\n");
    }

    #[test]
    fn minimal_quote_selection_raw_single_quote_when_possible_double_quote_only_for_or_newline() {
        // "2" needs quoting (bare would sniff as a number); no `'`/newline inside,
        // so the raw form is picked.
        expect_print("resolver = \"2\"\n", "resolver = '2'\n");
        // A value starting with `'` can't itself be bare (opens a committed string
        // form) and can't use the raw form either (§ 4.5: raw MUST NOT contain
        // `'`) — falls back to escaped double-quotes, same as before this change.
        expect_print("s = \"'tis the season\"\n", "s = \"'tis the season\"\n");

        // A key containing a literal newline (unreachable from fig source — keys
        // can't be authored with one — but reachable from a foreign/built AST)
        // can't use the raw form either (single-line raw MUST NOT contain a
        // newline): falls back to escaped double-quotes with `\n` escaped.
        let mut b = Builder::init();
        let key = b.add_string(b"a\nb").unwrap();
        let value = b.add_number_raw(b"1", false).unwrap();
        let root_id = b.add_mapping(&[Entry { key, value }]).unwrap();
        let ast = b.finish(root_id).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast, SerializeOptions::default()).unwrap();
        assert_eq!("\"a\\nb\" = 1\n", str(&out));
    }

    #[test]
    fn flow_strings_that_would_not_survive_bare_are_quoted() {
        // A comma inside a flow element would split it; a leading bracket would
        // nest; a sniffable number-string would change type.
        expect_print(
            "xs = [\"a, b\", \"[glob]\", \"99\", plain]"
        ,
            concat!(
                "xs = ['a, b', '[glob]', '99', plain]\n",
            )
        );
    }

    #[test]
    fn comments_keep_their_anchors_through_fmt() {
        expect_print(
            concat!(
                "# on the section\n",
                "deps\n",
                "> # on serde's key\n",
                "> serde = 1 # trailing\n",
                "> other = this value is long enough to keep the whole deps table from ever fitting the inline budget",
            )
        ,
            concat!(
                "# on the section\n",
                "deps\n",
                "> # on serde's key\n",
                "> serde = 1 # trailing\n",
                "> other = this value is long enough to keep the whole deps table from ever fitting the inline budget\n",
            )
        );
    }

    #[test]
    fn round_trips_through_a_second_parse_and_fmt_is_idempotent() {
        expect_round_trip(
            concat!(
                "database\n",
                "> host = localhost\n",
                "> port = 5432\n",
                "servers\n",
                "> *\n",
                ">> host = a.com\n",
                ">> port = 1",
            )
        );
        expect_round_trip(
            concat!(
                "workspace\n",
                "> resolver = \"2\"\n",
                "> members\n",
                ">>* crates/one_rather_long_member_path_aaaaaaaaaaaa\n",
                ">>* crates/two_rather_long_member_path_bbbbbbbbbbbb\n",
                "> package\n",
                ">> version = 1.6.1\n",
                ">> license-file = LICENSE.md\n",
                "> dependencies\n",
                ">> serde = {version=\"1.0\" features=[derive]}\n",
                ">> fig\n",
                ">>> version = 1.0.0\n",
                ">>> features-list = [serde, yaml, derive, indexmap, and, extra, entries, to, defeat, inlining, of, fig]\n",
                "> metadata\n",
                ">> replacements\n",
                ">>> *\n",
                ">>>> file = README.md\n",
                ">>>> search = a deliberately long pattern string so this element cannot fit in the inline flow budget\n",
                ">>> *\n",
                ">>>> file = CHANGELOG.md\n",
                ">>>> search = another deliberately long pattern string keeping the sequence out of the inline budget\n",
                "profile\n",
                "> release = {lto=fat codegen-units=1}",
            )
        );
        expect_round_trip(
            concat!(
                "values\n",
                "> answer = 42\n",
                "> flag = \"true\"\n",
                "> zip = 007\n",
                "> movie = 12 monkeys\n",
                "> when = 2026-07-01T12:00:00Z\n",
                "> class: enum = minecraft\n",
                "> huge: float = inf\n",
                "> long-tail = a sufficiently long value that the values table cannot be rendered through inline flow",
            )
        );
    }

    #[test]
    fn char_literals_print_as_char_atoms_and_round_trip() {
        // Each char re-emits as its ZON-style `'…'` literal beside a `: char`
        // annotation — the explicit-typing form, never a flow value.
        expect_print(
            concat!(
                "letter: char = 'A'\n",
                "tab: char = '\\t'\n",
                "quote: char = '\\''\n",
                "emoji: char = '\\u{1F600}'",
            )
        ,
            concat!(
                "letter: char = 'A'\n",
                "tab: char = '\\t'\n",
                "quote: char = '\\''\n",
                "emoji: char = '\\u{1f600}'\n",
            )
        );
        expect_round_trip(
            concat!(
                "letter: char = 'A'\n",
                "newline: char = '\\n'\n",
                "emoji: char = '\\u{1f600}'",
            )
        );
    }

    #[test]
    fn hoisting_groups_flat_sibling_runs_under_one_header_re_entry() {
        // A map over the depth budget with a run of scalars AND a deep child:
        // the scalars share one `workspace.metadata.release` header instead of
        // each repeating the full dotted path; the deep child gets its own
        // `[]` section after.
        expect_print(
            concat!(
                "workspace\n",
                "> metadata\n",
                ">> release\n",
                ">>> shared-version = true\n",
                ">>> consolidate-commits = true\n",
                ">>> push = false\n",
                ">>> publish = false\n",
                ">>> tag = false\n",
                ">>> tag-name = v{{version}}\n",
                ">>> tag-message = Release v{{version}}\n",
                ">>> pre-release-commit-message = chore: release v{{version}}\n",
                ">>> pre-release-replacements\n",
                ">>>> *\n",
                ">>>>> file = README.md\n",
                ">>>>> search = a deliberately long pattern string so this element cannot fit in the inline flow budget\n",
                ">>>> *\n",
                ">>>>> file = CHANGELOG.md\n",
                ">>>>> search = another deliberately long pattern string keeping the sequence out of the inline budget",
            )
        ,
            concat!(
                "workspace.metadata.release\n",
                "> shared-version = true\n",
                "> consolidate-commits = true\n",
                "> push = false\n",
                "> publish = false\n",
                "> tag = false\n",
                "> tag-name = v{{version}}\n",
                "> tag-message = Release v{{version}}\n",
                "> pre-release-commit-message = chore: release v{{version}}\n",
                "\n",
                "workspace.metadata.release.pre-release-replacements[]\n",
                "> file = README.md\n",
                "> search = a deliberately long pattern string so this element cannot fit in the inline flow budget\n",
                "+\n",
                "> file = CHANGELOG.md\n",
                "> search = another deliberately long pattern string keeping the sequence out of the inline budget\n",
            )
        );
    }

    #[test]
    fn a_lone_flat_child_between_deep_siblings_stays_a_dotted_assignment() {
        expect_print(
            concat!(
                "workspace\n",
                "> resolver = \"2\"\n",
                "> dependencies\n",
                ">> fig\n",
                ">>> version = 1.0.0\n",
                ">>> features-list = [serde, yaml, derive, indexmap, extras, and-more]\n",
                ">> chrono\n",
                ">>> version = \"0.4\"\n",
                ">>> features-list = [serde, std, clock, alloc, extra, padding]",
            )
        ,
            concat!(
                "workspace.resolver = '2'\n",
                "\n",
                "workspace.dependencies\n",
                "> fig\n",
                "> > version = 1.0.0\n",
                "> > features-list = [serde, yaml, derive, indexmap, extras, and-more]\n",
                "> chrono\n",
                "> > version = '0.4'\n",
                "> > features-list = [serde, std, clock, alloc, extra, padding]\n",
            )
        );
    }

    #[test]
    fn multi_line_strings_print_as_raw_blocks_escaped_when_needed() {
        // A trailing newline in the value becomes one empty line before the
        // flush-left closer (the tokenizer drops the newline before the closer's
        // line). Interior indentation is content and survives verbatim.
        expect_print("script = \"set -e\\nif x; then\\n  echo hi\\nfi\\n\"\n",
            concat!(
                "script = '''\n",
                "set -e\n",
                "if x; then\n",
                "  echo hi\n",
                "fi\n",
                "\n",
                "'''\n",
            )
        );
        // Content containing ''' cannot be raw — escaped flavor, quotes escaped.
        expect_print("q = \"has '''\\nquotes\"\n",
            concat!(
                "q = \"\"\"\n",
                "has '''\n",
                "quotes\n",
                "\"\"\"\n",
            )
        );
    }

    // The comment went after the closing quotes, where neither the spec nor the
    // editor looks for it. It prints on the opener line, as an entry's value at
    // the root and nested, and as a sequence element; one the source wrote after
    // the close moves there too.
    #[test]
    fn a_multi_line_string_s_trailing_comment_prints_on_its_opener_line() {
        expect_print("a = ''' # raw\nx\ny\n'''\nb = \"\"\" # escaped\n  has '''\n  more\n  \"\"\"\n",
            concat!(
                "a = ''' # raw\n",
                "x\n",
                "y\n",
                "'''\n",
                "b = \"\"\" # escaped\n",
                "has '''\n",
                "more\n",
                "\"\"\"\n",
            )
        );
        expect_print("s\n> long.k = '''\nx\ny\n''' # after\n> n = 1\n",
            concat!(
                "s\n",
                "> long.k = ''' # after\n",
                "x\n",
                "y\n",
                "'''\n",
                "> n = 1\n",
            )
        );
        expect_print("xs\n> * ''' # el\nx\ny\n'''\n> * z\n",
            concat!(
                "xs\n",
                "> * ''' # el\n",
                "x\n",
                "y\n",
                "'''\n",
                "> * z\n",
            )
        );
    }

    #[test]
    fn multi_line_strings_round_trip_raw_escaped_trailing_newline_shapes() {
        expect_round_trip("a = \"x\\ny\"\nb = \"x\\ny\\n\"\nc = \"tail '''\\nline2\"\nd = \"\\n\\nleading blanks\"\n");
    }

    #[test]
    fn a_container_holding_a_multi_line_string_breaks_out_of_flow() {
        expect_print("wrap\n> cmd = \"a\\nb\"\n",
            concat!(
                "wrap.cmd = '''\n",
                "a\n",
                "b\n",
                "'''\n",
            )
        );
    }

    #[test]
    fn typed_sequence_elements_keep_their_annotations() {
        expect_print(
            concat!(
                "weights\n",
                ">*: int = 1\n",
                ">*: enum = heavy\n",
                ">* and a long plain string element so the whole weights sequence stays out of the inline budget",
            )
        ,
            concat!(
                "weights\n",
                "> *: int = 1\n",
                "> *: enum = heavy\n",
                "> * and a long plain string element so the whole weights sequence stays out of the inline budget\n",
            )
        );
    }

    #[test]
    fn explicit_type_annotations_round_trip_the_type_surface_verbatim_lexeme() {
        // A REDUNDANT annotation (the value already sniffs to the type) is preserved.
        expect_print("num: int = 3\n", "num: int = 3\n");
        // A trailing-dot float keeps BOTH its lexeme (`1.`, not `1.0`) and its `: float`.
        expect_print("sig: float = 1.\n", "sig: float = 1.\n");
        // A leading-zero int keeps its padding (`09`, not `9`).
        expect_print("lead: int = 09\n", "lead: int = 09\n");
        // A `: string` total sink re-emits BARE — no re-quoting the bracketed value.
        expect_print("strs: string = [ 1 + 2 ]\n", "strs: string = [ 1 + 2 ]\n");
        // Idempotence + parse→print→parse stability across all four together.
        expect_round_trip("num: int = 3\nsig: float = 1.\nlead: int = 09\nstrs: string = [ 1 + 2 ]\n");
    }

    #[test]
    fn a_string_value_containing_quote_characters_round_trips_bare() {
        // Under the all-raw rule the RHS of `s: string = "hello"` is the
        // seven-character string `"hello"` — and because the annotation re-reads
        // the RHS verbatim, the printer re-emits exactly those bytes bare.
        expect_print("s: string = \"hello\"\n", "s: string = \"hello\"\n");
        expect_round_trip("s: string = \"hello\"\n");
        // Same for a value that merely STARTS with a quote character.
        expect_print("s: string = \"  x\"\n", "s: string = \"  x\"\n");
        expect_round_trip("s: string = \"  x\"\n");
        // And a lone triple-quote opener: literal content, never a multiline form.
        expect_print("s: string = '''\n", "s: string = '''\n");
        expect_round_trip("s: string = '''\n");
    }



    /// Swap in a `: string` kind tag on `id` — simulates a string-type
    /// assertion arriving from a surface fig itself can no longer produce for
    /// this value (a YAML `!!str`, a builder API), so the printer's
    /// drop-the-tag guard can be pinned.
    fn inject_string_tag(ast: &mut AST, id: NodeId) {
        ast.set_tag(id, Tag::Kind(KindTag::String)).unwrap();
    }

    /// Print `ast`, assert the exact bytes, then re-parse and assert the value
    /// at key `s` is byte-identical to `want_value` and that a second print is
    /// byte-identical to the first (idempotence). Dropping the tag is the
    /// CORRECT lossy step, so only the value and the emitted bytes are
    /// asserted.
    fn expect_tag_dropped(ast: &AST, expected: &str, want_value: &str) {
        let mut out = Vec::new();
        print(&mut out, ast, SerializeOptions::default()).unwrap();
        assert_eq!(expected, str(&out));
        let ast2 = parse(str(&out));
        let v = ast2.get_val_by_path(&[PathSegment::Key(b"s")]).unwrap();
        assert_eq!(want_value.as_bytes(), v.kind.as_string().unwrap());
        assert!(ast2.tag_of(v.id).is_none());
        let mut out2 = Vec::new();
        print(&mut out2, &ast2, SerializeOptions::default()).unwrap();
        assert_eq!(str(&out), str(&out2));
    }

    #[test]
    fn a_string_tagged_multiline_value_drops_the_tag_and_uses_the_ordinary_block_form() {
        // Raw `: string` text is single-line, so `s: string = '''…` would
        // re-parse as the literal characters `'''`. The printer must never
        // emit the annotation with a multiline value.
        let mut ast = parse("s = '''\na\nb\n'''\n");
        let v = ast.get_val_by_path(&[PathSegment::Key(b"s")]).unwrap();
        assert_eq!(b"a\nb", v.kind.as_string().unwrap());
        let id = v.id;
        inject_string_tag(&mut ast, id);
        expect_tag_dropped(&ast, "s = '''\na\nb\n'''\n", "a\nb");
    }

    #[test]
    fn a_string_tagged_value_with_no_bare_spelling_drops_the_tag_and_quotes_normally() {
        // Leading whitespace would be skipped by the raw scan, so bare
        // emission under the annotation can't represent it.
        let mut ast = parse("s = \"  x\"\n");
        let v = ast.get_val_by_path(&[PathSegment::Key(b"s")]).unwrap();
        assert_eq!(b"  x", v.kind.as_string().unwrap());
        let id = v.id;
        inject_string_tag(&mut ast, id);
        expect_tag_dropped(&ast, "s = '  x'\n", "  x");
    }

    #[test]
    fn a_string_tagged_value_that_is_bare_representable_keeps_the_tag_when_injected() {
        // The same injection on a total-sink-safe value keeps the annotation
        // and emits bare — including a value that starts with a quote
        // character.
        let mut ast = parse("s = \"\\\"q\\\" and more\"\n");
        let v = ast.get_val_by_path(&[PathSegment::Key(b"s")]).unwrap();
        assert_eq!(b"\"q\" and more", v.kind.as_string().unwrap());
        let id = v.id;
        inject_string_tag(&mut ast, id);
        let mut out = Vec::new();
        print(&mut out, &ast, SerializeOptions::default()).unwrap();
        assert_eq!("s: string = \"q\" and more\n", str(&out));
        let ast2 = parse(str(&out));
        assert!(ast.eql(&ast2));
        assert!(ast.tags_eql(&ast2));
    }

    #[test]
    fn a_splice_fragment_keeps_a_container_in_block_unless_the_caller_asks_for_flow() {
        // The bindings' `*With` editor paths ask for the splice alone, and
        // want a container in its natural block layout, which the editor
        // re-frames under the key; the plain paths also ask for flow.
        let ast = parse("a = 1\nb\n> c = 2\n");
        let mut block = Vec::new();
        print_fragment(&mut block, &ast, SerializeOptions { splice: true, width: 1, ..SerializeOptions::default() }).unwrap();
        assert!(!block.contains(&b'{'));
        let mut flow = Vec::new();
        print_fragment(&mut flow, &ast, SerializeOptions { splice: true, flow: true, ..SerializeOptions::default() }).unwrap();
        assert!(flow.starts_with(b"{"));
    }

    #[test]
    fn a_non_finite_float_from_another_format_prints_under_a_float_annotation() {
        // `.inf` from YAML, `Infinity` from JSON5: figl has no bare spelling
        // for either, and reads `inf` bare as a string, so each takes the
        // annotation.
        let mut b = Builder::init();
        let entries = [
            Entry { key: b.add_string(b"up").unwrap(), value: b.add_number_raw(b".inf", true).unwrap() },
            Entry { key: b.add_string(b"down").unwrap(), value: b.add_number_raw(b"-Infinity", true).unwrap() },
            Entry { key: b.add_string(b"odd").unwrap(), value: b.add_number_raw(b".NaN", true).unwrap() },
        ];
        let a = b.add_mapping(&entries).unwrap();
        let items = [b.add_number_raw(b"1", false).unwrap(), b.add_number_raw(b"+.inf", true).unwrap()];
        let xs = b.add_sequence(&items).unwrap();
        let doc_entries = [
            Entry { key: b.add_string(b"x").unwrap(), value: b.add_number_raw(b"-.inf", true).unwrap() },
            Entry { key: b.add_string(b"a").unwrap(), value: a },
            Entry { key: b.add_string(b"xs").unwrap(), value: xs },
        ];
        let doc = b.add_mapping(&doc_entries).unwrap();
        let ast = b.finish(doc).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast, SerializeOptions::default()).unwrap();
        // The map and the list leave flow, which has no annotation.
        let want = concat!(
            "x: float = -inf\n",
            "\n",
            "a\n",
            "> up: float = inf\n",
            "> down: float = -inf\n",
            "> odd: float = nan\n",
            "\n",
            "xs\n",
            "> * 1\n",
            "> *: float = inf\n",
        );
        assert_eq!(want, str(&out));

        // Each reads back as the float it was, not a string.
        let back = parse(want);
        let first = back.kind_of(back.root).as_mapping().unwrap().unwrap();
        let x = back.kind_of(back.kind_of(first).as_keyvalue().unwrap().value);
        let NodeKind::Extended(e) = x else { panic!("not extended") };
        assert_eq!(ExtKind::NumberSpecial, e.kind);
        assert_eq!(b"-inf", e.text);
        expect_round_trip(want);
    }

    #[test]
    fn a_fragment_refuses_a_non_finite_float_where_no_annotation_can_go() {
        // The editor splices a fragment after `key = `, where `inf` reads back
        // as a string; the annotation goes before the `=`.
        let mut out = Vec::new();
        let mut b = Builder::init();
        let inf = b.add_number_raw(b".inf", true).unwrap();
        let scalar = b.finish(inf).unwrap();
        let splice = SerializeOptions { splice: true, ..SerializeOptions::default() };
        assert_eq!(err!(NonFiniteNumber), print_fragment(&mut out, &scalar, splice).unwrap_err());

        // A `number_special` the figl parser made is refused the same way.
        let mut b2 = Builder::init();
        let special = b2.add_extended(ExtKind::NumberSpecial, b"nan").unwrap();
        let ext = b2.finish(special).unwrap();
        assert_eq!(err!(NonFiniteNumber), print_fragment(&mut out, &ext, splice).unwrap_err());

        // And so is one inside a container the caller asked to keep in flow.
        let ast = parse("xs\n> * 1\n> *: float = inf\n");
        assert_eq!(err!(NonFiniteNumber), print_fragment(&mut out, &ast, SerializeOptions { flow: true, ..splice }).unwrap_err());

        // In block, the same container keeps its annotation.
        let mut block = Vec::new();
        print_fragment(&mut block, &ast, splice).unwrap();
        assert_eq!("xs\n> * 1\n> *: float = inf\n", str(&block));
    }
}
