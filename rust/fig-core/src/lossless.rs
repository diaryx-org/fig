//! Port of src/lossless.zig.
//!
//! Lossless cross-format conversion via a reserved "$fig" envelope.
//!
//! Converting between formats normally degrades any value the target format has
//! no native form for: a `null` bound for TOML, a TOML datetime bound for JSON,
//! a ZON enum/char literal bound for anything else. Lossless mode preserves these
//! by wrapping the value in an ordinary mapping carrying a reserved sentinel key,
//! which a later fig run recognizes and reconstructs.
//!
//! Two AST passes implement it, both format-agnostic:
//!   - `encode` runs BEFORE printing. It rewrites every node the *target* format
//!     can't represent natively (`needs_envelope`) into a `{ "$fig": { … } }`
//!     mapping. Nodes the target can hold (a `null` for JSON, a datetime for
//!     TOML) are left untouched, so idiomatic output is preserved where possible.
//!   - `decode` runs AFTER parsing. It recognizes the envelope shape in the
//!     parsed AST and rebuilds the original node kind.
//!
//! The CLI gates BOTH on `--lossless` (symmetric): a round-trip is `--lossless`
//! on each leg. By default neither runs, so a literal `$fig` key in real data is
//! treated as ordinary data both ways.
//!
//! A third pass, `lossy_strip`, is the LOSSY counterpart used when `--lossless` is
//! off: rather than the printer aborting mid-document on a value the target can't
//! represent at all (a `null` for TOML — datetimes/enums merely degrade, they
//! don't error), it removes that value up front and returns the dropped paths so
//! the CLI can warn. Output stays valid and complete; nothing is half-written.
//!
//! SCOPE (this pass): `null` and the `extended` scalars (TOML datetimes, ZON
//! enum/char literals). The YAML reference layer (custom tags, anchors, aliases)
//! is a separate future pass — those need side-table reconstruction; these are
//! pure node-kind transforms.
//!
//! Envelope schema, as it appears once parsed back into an AST:
//!   { "$fig": { "t": "<type>" } }            // a null
//!   { "$fig": { "t": "<extkind>", "v": "<text>" } }   // an extended scalar
//! where `<type>` is `"null"` and `<extkind>` is the `ExtKind` field name
//! (`offset_datetime`, `enum_literal`, …). `v` carries the scalar's intrinsic
//! text verbatim (for a char literal, its decimal codepoint, as fig stores it).

use crate::kit::ast::{ExtKind, Extended, KeyValue, Node, NodeId as Id, NodeKind, AST};
use crate::kit::row_writer::RowWriter;
use crate::kit::table::{self, Row, Table};
use crate::serialize::Target;
use crate::Result;

/// The reserved key marking a lossless envelope. Chosen to be unlikely in real
/// config data; a real mapping that exactly mimics the envelope shape would be
/// mis-decoded (a documented limitation — escaping is a future concern).
const SENTINEL: &[u8] = b"$fig";

/// What an `encode` pass targets: the scalar kinds the output format spells
/// natively, as the format itself declares them (`Caps.lossless`).
/// Everything not marked is enveloped. This module never names a format — it
/// reads the declaration and applies one rule to it.
pub use crate::kit::manifest::NativeKinds;

/// The native-kinds declaration the envelope pass should encode for when the
/// output is `target`, or `None` when it takes no envelope at all (see
/// `Caps.lossless` for which formats say `None` and why): what the language
/// registered declares. `canonical`, the AST's own oracle grammar, takes none.
pub fn native_for(target: &Target) -> Option<NativeKinds> {
    match target {
        Target::Canonical => None,
        Target::Language(e) => e.language.caps.lossless,
    }
}

/// Does a format that holds `native` need the lossless envelope to represent
/// `kind` without loss? True only for the scalar kinds the format did not mark
/// native; every other kind (and any container) is copied through verbatim.
pub fn needs_envelope(native: NativeKinds, kind: &NodeKind<'_>) -> bool {
    match kind {
        NodeKind::Null => !native.null,
        // One field per `ExtKind`, named identically.
        NodeKind::Extended(e) => !native.ext(e.kind),
        _ => false,
    }
}

/// Whether a format that holds `native` cannot represent `kind` AT ALL (even
/// degraded) — the values the lossy `lossy_strip` pass removes. Distinct from
/// `needs_envelope`: a TOML datetime → JSON is a `needs_envelope` case (degrades
/// to a string in lossy mode, no data type lost beyond the tag) but NOT
/// unrepresentable. A `null` is the only kind with nothing to degrade TO — an
/// extended scalar has its text — so it is the only kind this is true for,
/// and only where the format has no `null` (TOML, today).
pub fn is_unrepresentable(native: NativeKinds, kind: &NodeKind<'_>) -> bool {
    matches!(kind, NodeKind::Null) && !native.null
}

// ── Public entry points ─────────────────────────────────────────────────────

/// Build a fresh AST where every node a format holding `native` can't
/// represent is wrapped in a `$fig` envelope. The result owns its strings.
pub fn encode(ast: &AST, native: NativeKinds) -> Result<AST> {
    let mut e = Encoder { src: ast, native, out: Rows::new() };
    let root = e.copy(ast.node(ast.root))?;
    e.out.finish(ast, root)
}

/// Build a fresh AST where every `$fig` envelope is reconstructed to the node
/// kind it encodes. A mapping that doesn't match the envelope shape (or
/// carries an unknown type tag) is copied through as ordinary data.
pub fn decode(ast: &AST) -> Result<AST> {
    let mut d = Decoder { src: ast, out: Rows::new() };
    let root = d.copy(ast.node(ast.root))?;
    d.out.finish(ast, root)
}

/// The result of a `lossy_strip`: a new AST with every node the target can't
/// represent removed, plus the dot/bracket paths of what was dropped (for
/// warnings). `ast` is `None` only when the navigated root node was ITSELF
/// unrepresentable (a bare `null` → TOML), in which case there is nothing to
/// print and `dropped` names it.
#[derive(Debug)]
pub struct StripResult {
    pub ast: Option<AST>,
    pub dropped: Vec<Vec<u8>>,
}

/// Build a fresh AST rooted at the subtree `root_id`, dropping every mapping
/// entry and sequence element the target can't represent at all (today: a
/// `null` for TOML). Used in lossy mode so the printer never aborts a document
/// partway through. Dropped paths are reported relative to `root_id`.
pub fn lossy_strip(ast: &AST, root_id: Id, native: NativeKinds) -> Result<StripResult> {
    let mut s = Stripper { src: ast, native, out: Rows::new(), dropped: Vec::new() };
    if is_unrepresentable(native, &ast.kind_of(root_id)) {
        s.dropped.push(b"(value)".to_vec());
        return Ok(StripResult { ast: None, dropped: s.dropped });
    }
    let root = s.copy(ast.node(root_id), b"")?;
    let ast = s.out.finish(ast, root)?;
    Ok(StripResult { ast: Some(ast), dropped: s.dropped })
}

// ── Encoder ─────────────────────────────────────────────────────────────────

struct Encoder<'a> {
    src: &'a AST,
    native: NativeKinds,
    out: Rows,
}

impl<'a> Copier<'a> for Encoder<'a> {
    fn src(&self) -> &'a AST {
        self.src
    }
    fn out(&mut self) -> &mut Rows {
        &mut self.out
    }
    fn copy(&mut self, node: Node<'a>) -> Result<Id> {
        match node.kind {
            NodeKind::Sequence(_) => return copy_seq(self, node),
            NodeKind::Mapping(_) => return copy_map(self, node),
            NodeKind::KeyValue(_) => unreachable!("keyvalues are only reached inside copy_map"),
            _ => {}
        }
        // A leaf scalar. If it needs enveloping, the value's comments ride on the
        // wrapping mapping; otherwise they ride on the copied scalar.
        let enveloped = matches!(node.kind, NodeKind::Null | NodeKind::Extended(_)) && needs_envelope(self.native, &node.kind);
        let id = if enveloped { self.envelope(&node.kind)? } else { self.out.emit(node.kind)? };
        // The envelope records the scalar's kind in its `t`; a tag carried
        // onto the wrapper would assert a type on the MAPPING, which is a
        // different claim, and a custom tag has nowhere in the envelope to
        // go. Leave the wrapper untagged.
        self.out.carry(self.src, node.id, id, !enveloped)?;
        Ok(id)
    }
}

impl Encoder<'_> {
    /// Emit `{ "$fig": { "t": <type>, ["v": <text>] } }` for a null/extended
    /// node, leaves first so child ids precede their containers.
    fn envelope(&mut self, kind: &NodeKind<'_>) -> Result<Id> {
        let type_name: &[u8] = match kind {
            NodeKind::Null => b"null",
            NodeKind::Extended(e) => e.kind.name().as_bytes(),
            _ => unreachable!(),
        };
        let text: Option<&[u8]> = match kind {
            NodeKind::Extended(e) => Some(e.text),
            _ => None,
        };

        let o = &mut self.out;
        let t_key = o.emit(NodeKind::String(b"t"))?;
        let t_val = o.emit(NodeKind::String(type_name))?;
        let t_kv = o.emit(NodeKind::KeyValue(KeyValue { key: t_key, value: t_val }))?;
        let v_kv = match text {
            Some(vt) => {
                let v_key = o.emit(NodeKind::String(b"v"))?;
                let v_val = o.emit(NodeKind::String(vt))?;
                Some(o.emit(NodeKind::KeyValue(KeyValue { key: v_key, value: v_val }))?)
            }
            None => None,
        };
        let inner = o.emit(NodeKind::Mapping(None))?;
        let mut last: Option<Id> = None;
        o.link(inner, &mut last, t_kv);
        if let Some(x) = v_kv {
            o.link(inner, &mut last, x);
        }
        let s_key = o.emit(NodeKind::String(SENTINEL))?;
        let fig_kv = o.emit(NodeKind::KeyValue(KeyValue { key: s_key, value: inner }))?;
        let outer = o.emit(NodeKind::Mapping(None))?;
        last = None;
        o.link(outer, &mut last, fig_kv);
        Ok(outer)
    }
}

// ── Decoder ─────────────────────────────────────────────────────────────────

struct Decoder<'a> {
    src: &'a AST,
    out: Rows,
}

impl<'a> Copier<'a> for Decoder<'a> {
    fn src(&self) -> &'a AST {
        self.src
    }
    fn out(&mut self) -> &mut Rows {
        &mut self.out
    }
    fn copy(&mut self, node: Node<'a>) -> Result<Id> {
        match node.kind {
            NodeKind::Mapping(_) => {
                // A decoded envelope collapses the wrapper mapping back to a
                // scalar; the wrapper's comments move onto that scalar.
                if let Some(kind) = self.envelope_kind(&node) {
                    let id = self.out.emit(kind)?;
                    self.out.carry(self.src, node.id, id, true)?;
                    return Ok(id);
                }
                copy_map(self, node)
            }
            NodeKind::Sequence(_) => copy_seq(self, node),
            NodeKind::KeyValue(_) => unreachable!(),
            _ => {
                let id = self.out.emit(node.kind)?;
                self.out.carry(self.src, node.id, id, true)?;
                Ok(id)
            }
        }
    }
}

impl<'a> Decoder<'a> {
    /// If `node` is a well-formed `$fig` envelope, return the node kind it
    /// encodes; otherwise `None` (so it is copied through as ordinary data). The
    /// envelope must be a mapping whose ONLY entry is the sentinel key mapping to
    /// an inner mapping carrying a string `t` (and, for extended scalars, `v`).
    fn envelope_kind(&self, node: &Node<'_>) -> Option<NodeKind<'a>> {
        let first = node.kind.as_mapping()??;
        let entry = self.src.node(first);
        if entry.next_sibling.is_some() {
            return None; // more than one key → not an envelope
        }
        let kv = entry.kind.as_keyvalue()?;
        if self.src.kind_of(kv.key).as_string() != Some(SENTINEL) {
            return None;
        }
        let NodeKind::Mapping(inner) = self.src.kind_of(kv.value) else { return None };

        let mut type_name: Option<&'a [u8]> = None;
        let mut text: Option<&'a [u8]> = None;
        let mut c = inner;
        while let Some(cid) = c {
            c = self.src.node(cid).next_sibling;
            let ikv = self.src.kind_of(cid).as_keyvalue()?;
            let NodeKind::String(ik) = self.src.kind_of(ikv.key) else { continue };
            let val: Option<&'a [u8]> = self.src.kind_of(ikv.value).as_string();
            if ik == b"t" {
                type_name = val;
            } else if ik == b"v" {
                text = val;
            }
        }

        let t = type_name?;
        if t == b"null" {
            return Some(NodeKind::Null);
        }
        if let Some(ek) = ExtKind::ALL.into_iter().find(|k| k.name().as_bytes() == t) {
            return Some(NodeKind::Extended(Extended { kind: ek, text: text.unwrap_or(b"") }));
        }
        None // unknown type tag → leave as ordinary mapping
    }
}

// ── Stripper (lossy) ─────────────────────────────────────────────────────────

struct Stripper<'a> {
    src: &'a AST,
    native: NativeKinds,
    out: Rows,
    dropped: Vec<Vec<u8>>,
}

impl Stripper<'_> {
    fn copy(&mut self, node: Node<'_>, path: &[u8]) -> Result<Id> {
        match node.kind {
            NodeKind::Mapping(_) => self.copy_map(node, path),
            NodeKind::Sequence(_) => self.copy_seq(node, path),
            NodeKind::KeyValue(_) => unreachable!(),
            _ => {
                let id = self.out.emit(node.kind)?;
                self.out.carry(self.src, node.id, id, true)?;
                Ok(id)
            }
        }
    }

    fn copy_map(&mut self, src_node: Node<'_>, path: &[u8]) -> Result<Id> {
        let id = self.out.emit(NodeKind::Mapping(None))?;
        self.out.carry(self.src, src_node.id, id, true)?;
        let mut last: Option<Id> = None;
        let mut c = src_node.kind.as_mapping().flatten();
        while let Some(cid) = c {
            c = self.src.node(cid).next_sibling;
            let kv = self.src.kind_of(cid).as_keyvalue().expect("a mapping child is a keyvalue");
            let child_path = key_path(self.src, path, kv.key);
            if is_unrepresentable(self.native, &self.src.kind_of(kv.value)) {
                self.dropped.push(child_path);
                continue;
            }
            let new_key = self.copy(self.src.node(kv.key), path)?;
            let new_val = self.copy(self.src.node(kv.value), &child_path)?;
            let kvid = self.out.emit(NodeKind::KeyValue(KeyValue { key: new_key, value: new_val }))?;
            self.out.link(id, &mut last, kvid);
        }
        Ok(id)
    }

    fn copy_seq(&mut self, src_node: Node<'_>, path: &[u8]) -> Result<Id> {
        let id = self.out.emit(NodeKind::Sequence(None))?;
        self.out.carry(self.src, src_node.id, id, true)?;
        let mut last: Option<Id> = None;
        let mut c = src_node.kind.as_sequence().flatten();
        let mut i: usize = 0;
        while let Some(cid) = c {
            c = self.src.node(cid).next_sibling;
            let child_path = index_path(path, i);
            i += 1;
            if is_unrepresentable(self.native, &self.src.kind_of(cid)) {
                self.dropped.push(child_path);
                continue;
            }
            let new_id = self.copy(self.src.node(cid), &child_path)?;
            self.out.link(id, &mut last, new_id);
        }
        Ok(id)
    }
}

/// `parent.key` (or just `key` at the root). A non-string key shows as `?`
/// (it can't be a drop target — only its value can — so this is purely a hint).
pub(crate) fn key_path(src: &AST, parent: &[u8], key_id: Id) -> Vec<u8> {
    let name: &[u8] = match src.kind_of(key_id) {
        NodeKind::String(s) => s,
        _ => b"?",
    };
    if parent.is_empty() {
        return name.to_vec();
    }
    let mut out = parent.to_vec();
    out.push(b'.');
    out.extend_from_slice(name);
    out
}

fn index_path(parent: &[u8], i: usize) -> Vec<u8> {
    let mut out = parent.to_vec();
    out.extend_from_slice(format!("[{i}]").as_bytes());
    out
}

// ── Shared row-writing helpers (over Encoder/Decoder/Stripper) ──
// `pub` so `flat_strip.rs`'s own Stripper (INI/dotenv/`.properties`'s
// depth-based capability model doesn't fit `NativeKinds`' scalar-kind shape —
// see that file's module doc) can reuse this tree-copying plumbing instead of
// a third copy of it.

/// The table a rebuilding pass writes its tree into, a row per node as it is
/// emitted, linked as it goes, and handed to `AST::own` by `finish`. It has
/// no source: every text is copied into its buffer, so the AST a pass
/// returns owns its strings and outlives the one it was read from.
///
/// Besides the nodes it carries what is bound to them — type tags, anchors,
/// comments — and the document's `%TAG` directives: a printer that spells
/// tags (YAML's `!!str`, a runtime language's own) reads them off the AST it
/// is handed, so a pass that rebuilt the tree without them would print a
/// different document. Not carried: the marker and separator spans, which
/// describe the SOURCE text — a rebuilt tree has none.
#[derive(Debug, Default)]
pub struct Rows {
    pub t: Table,
}

impl Rows {
    pub fn new() -> Rows {
        Rows { t: Table::default() }
    }

    fn writer(&mut self) -> RowWriter<'_> {
        RowWriter::init(&mut self.t)
    }

    /// A new node of `kind`, with no span: a keyvalue's key and value linked
    /// to it, a container's children added by `link`.
    pub fn emit(&mut self, kind: NodeKind<'_>) -> Result<Id> {
        let id = self.t.add_row(Row::default())?;
        self.writer().set(id, kind)?;
        Ok(id)
    }

    /// Append `child` to container `container`, after `last`.
    pub fn link(&mut self, container: Id, last: &mut Option<Id>, child: Id) {
        let rows = &mut self.t.rows;
        rows[child as usize].parent = container;
        match *last {
            Some(p) => rows[p as usize].next_sibling = child,
            None => rows[container as usize].first_child = child,
        }
        *last = Some(child);
    }

    /// Copy the type tag (unless `tag` is false), anchor and comments bound
    /// to `src`'s node `src_id` onto the freshly emitted node `new_id`. A
    /// tag is carried as `tag_of` reads it, its resolution left unstated.
    pub fn carry(&mut self, src: &AST, src_id: Id, new_id: Id, tag: bool) -> Result<()> {
        let mut w = self.writer();
        if tag {
            if let Some(t) = src.tag_of(src_id) {
                w.tag(new_id, t, table::TAG_UNSTATED, true, None)?;
            }
        }
        if let Some(name) = src.anchor_of(src_id) {
            w.anchor(new_id, name, None)?;
        }
        w.comments(new_id, &src.comments(src_id))
    }

    /// The tree rooted at `root` as an AST, with `src`'s `%TAG` directives
    /// and merge key.
    pub fn finish(mut self, src: &AST, root: Id) -> Result<AST> {
        let mut w = self.writer();
        for d in src.tag_directives() {
            w.directive(d.handle, d.prefix)?;
        }
        AST::own(&mut self.t, root, src.merge_key)
    }
}

/// What `copy_seq`/`copy_map` need of a pass (Zig's `anytype` duck typing).
trait Copier<'a> {
    fn src(&self) -> &'a AST;
    fn out(&mut self) -> &mut Rows;
    fn copy(&mut self, node: Node<'a>) -> Result<Id>;
}

fn copy_seq<'a, C: Copier<'a>>(c: &mut C, src_node: Node<'a>) -> Result<Id> {
    let src = c.src();
    let id = c.out().emit(NodeKind::Sequence(None))?;
    c.out().carry(src, src_node.id, id, true)?;
    let mut last: Option<Id> = None;
    let mut child = src_node.kind.as_sequence().flatten();
    while let Some(cid) = child {
        let new_id = c.copy(src.node(cid))?;
        c.out().link(id, &mut last, new_id);
        child = src.node(cid).next_sibling;
    }
    Ok(id)
}

fn copy_map<'a, C: Copier<'a>>(c: &mut C, src_node: Node<'a>) -> Result<Id> {
    let src = c.src();
    let id = c.out().emit(NodeKind::Mapping(None))?;
    c.out().carry(src, src_node.id, id, true)?;
    let mut last: Option<Id> = None;
    let mut child = src_node.kind.as_mapping().flatten();
    while let Some(cid) = child {
        let kv = src.kind_of(cid).as_keyvalue().expect("a mapping child is a keyvalue");
        let new_key = c.copy(src.node(kv.key))?;
        let new_val = c.copy(src.node(kv.value))?;
        let kvid = c.out().emit(NodeKind::KeyValue(KeyValue { key: new_key, value: new_val }))?;
        c.out().link(id, &mut last, kvid);
        child = src.node(cid).next_sibling;
    }
    Ok(id)
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// The round trips parse and print through the bundled JSON, TOML, ZON and
// YAML, reached through the global registry by name; each returns early
// while that dialect is not registered (a test build's global registry holds fig's ten,
// from the language crates). The canonical parser is the AST-literal
// syntax for tests whose subject isn't a particular format.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::{NumberKind, PathSegment as P, Tag};
    use crate::registry::{self, Entry};
    use crate::serialize::{self, SerializeOptions};
    use std::sync::Arc;

    fn entry(name: &str) -> Option<Arc<Entry>> {
        registry::global().entry_by_name(name)
    }

    /// The declaration the language registered as `name` encodes for.
    fn native(name: &str) -> Option<NativeKinds> {
        entry(name)?.language.caps.lossless
    }

    /// A format with no `null` and every extended scalar but enum/char,
    /// plist's date and data: TOML's declaration, spelled out for the tests
    /// that need no TOML printer.
    const TOML_LIKE: NativeKinds = NativeKinds {
        null: false,
        offset_datetime: true,
        local_datetime: true,
        local_date: true,
        local_time: true,
        enum_literal: false,
        char_literal: false,
        number_special: true,
        plist_date: false,
        data: false,
    };

    /// Parse `input` as `from`, run `decode` then `encode(native)`, print as
    /// `to`. `None` while either is unregistered.
    fn convert(from: &str, to: &str, native: NativeKinds, input: &[u8]) -> Option<String> {
        let (f, t) = (entry(from)?, entry(to)?);
        let doc = f.parse(input).unwrap();
        let decoded = decode(&doc.ast).unwrap();
        let encoded = encode(&decoded, native).unwrap();
        let mut out = Vec::new();
        serialize::serialize_with(&encoded, &mut out, &serialize::Target::Language(t), &SerializeOptions::default()).unwrap();
        Some(String::from_utf8(out).unwrap())
    }

    fn has(s: &str, needle: &str) -> bool {
        s.contains(needle)
    }

    #[test]
    fn null_round_trips_through_toml() {
        let (Some(toml_n), Some(json_n)) = (native("toml"), native("json")) else { return };
        // JSON null → TOML (must wrap; TOML has no null) → JSON (must restore).
        let Some(toml) = convert("json", "toml", toml_n, b"{\"k\": null}") else { return };
        assert!(has(&toml, "$fig")); // wrapped
        let json = convert("toml", "json", json_n, toml.as_bytes()).unwrap();
        assert_eq!("{\n  \"k\": null\n}\n", json);
    }

    #[test]
    fn null_stays_native_for_json_yaml_zon_targets() {
        let Some(json_n) = native("json") else { return };
        let json = convert("json", "json", json_n, b"{\"k\": null}").unwrap();
        assert_eq!("{\n  \"k\": null\n}\n", json);
        assert!(!has(&json, "$fig"));
    }

    #[test]
    fn toml_datetime_round_trips_through_json() {
        let (Some(toml_n), Some(json_n)) = (native("toml"), native("json")) else { return };
        let src = "t = 1979-05-27T07:32:00Z\n";
        let json = convert("toml", "json", json_n, src.as_bytes()).unwrap();
        assert!(has(&json, "offset_datetime"));
        let toml = convert("json", "toml", toml_n, json.as_bytes()).unwrap();
        assert_eq!(src, toml);
    }

    #[test]
    fn toml_datetime_stays_native_for_a_toml_target() {
        let Some(toml_n) = native("toml") else { return };
        let src = "t = 1979-05-27T07:32:00Z\n";
        assert_eq!(src, convert("toml", "toml", toml_n, src.as_bytes()).unwrap()); // not wrapped
    }

    #[test]
    fn a_byte_string_rides_an_envelope_through_json_and_is_binary_again_in_yaml() {
        let (Some(json_n), Some(yaml_n)) = (native("json"), native("yaml")) else { return };
        let json = convert("yaml", "json", json_n, b"k: !!binary aGk=\n").unwrap();
        assert!(has(&json, "$fig")); // wrapped
        let yaml = convert("json", "yaml", yaml_n, json.as_bytes()).unwrap();
        assert_eq!("k: !!binary aGk=\n", yaml);
    }

    #[test]
    fn zon_enum_and_char_literals_round_trip_through_json() {
        let (Some(json_n), Some(zon_n)) = (native("json"), native("zon")) else { return };
        let json = convert("zon", "json", json_n, b".{ .mode = .fast, .ch = 'A' }").unwrap();
        assert!(has(&json, "enum_literal"));
        assert!(has(&json, "char_literal"));
        let zon = convert("json", "zon", zon_n, json.as_bytes()).unwrap();
        assert_eq!(".{\n    .mode = .fast,\n    .ch = 'A',\n}\n", zon);
    }

    #[test]
    fn lossy_strip_drops_nulls_for_a_toml_target_and_reports_paths() {
        let ast = crate::canonical::parse_abstract(b"{\"a\": 1, \"b\": null, \"c\": [1, null, 2]}").unwrap();
        let result = lossy_strip(&ast, ast.root, TOML_LIKE).unwrap();
        let stripped = result.ast.unwrap();
        assert_eq!(2, result.dropped.len());
        assert_eq!(b"b", &result.dropped[0][..]);
        assert_eq!(b"c[1]", &result.dropped[1][..]);
        assert_eq!(err_not_found(), stripped.get_val_by_path(&[P::Key(b"b")]).unwrap_err());

        if let Some(toml) = entry("toml") {
            let result = lossy_strip(&ast, ast.root, toml.language.caps.lossless.unwrap()).unwrap();
            let mut out = Vec::new();
            serialize::serialize(&result.ast.unwrap(), &mut out, &serialize::Target::Language(toml)).unwrap();
            assert_eq!("a = 1\nc = [1, 2]\n", String::from_utf8_lossy(&out));
        }
    }

    fn err_not_found() -> crate::Error {
        crate::err!(NotFound)
    }

    #[test]
    fn lossy_strip_reports_a_bare_null_root_and_yields_no_ast() {
        let ast = crate::canonical::parse_abstract(b"null").unwrap();
        let result = lossy_strip(&ast, ast.root, TOML_LIKE).unwrap();
        assert!(result.ast.is_none());
        assert_eq!(1, result.dropped.len());
    }

    #[test]
    fn lossy_strip_on_a_nested_null_reports_a_dotted_path() {
        let ast = crate::canonical::parse_abstract(b"{\"outer\": {\"inner\": null, \"keep\": 2}}").unwrap();
        let result = lossy_strip(&ast, ast.root, TOML_LIKE).unwrap();
        assert!(result.ast.is_some());
        assert_eq!(1, result.dropped.len());
        assert_eq!(b"outer.inner", &result.dropped[0][..]);
    }

    #[test]
    fn encode_decode_and_lossy_strip_keep_a_documents_tags_and_tag_directives() {
        let Some(yaml) = entry("yaml") else { return };
        // `!e!foo` prints only beside the `%TAG` line declaring `!e!`, and
        // `!!str` is a kind tag: all three rebuilds must hand both on, or the
        // printer writes a different document (`1` an integer, `bar` untagged).
        let src = b"%TAG !e! tag:x/\n---\nk: !e!foo bar\nt: !!str 1\nn: null\n";
        let ast = yaml.parse(src).unwrap().ast;
        let toml_n = native("toml").unwrap_or(TOML_LIKE);

        let encoded = encode(&ast, toml_n).unwrap();
        assert_eq!(1, encoded.tag_directives().len());
        let decoded = decode(&encoded).unwrap();
        let target = serialize::Target::Language(yaml.clone());
        let mut out = Vec::new();
        serialize::serialize(&decoded, &mut out, &target).unwrap();
        assert_eq!("%TAG !e! tag:x/\n---\nk: !e!foo bar\nt: !!str 1\nn: null\n", String::from_utf8_lossy(&out));

        let stripped = lossy_strip(&ast, ast.root, toml_n).unwrap().ast.unwrap();
        let mut out2 = Vec::new();
        serialize::serialize(&stripped, &mut out2, &target).unwrap();
        assert_eq!("%TAG !e! tag:x/\n---\nk: !e!foo bar\nt: !!str 1\n", String::from_utf8_lossy(&out2));
    }

    #[test]
    fn an_envelope_carries_no_tag_of_the_scalar_it_wraps() {
        // A tag on the wrapper would assert a type on a mapping. The envelope
        // already records the kind in its `t`. (The canonical parser spells
        // the tagged null the YAML test reads.)
        let ast = crate::canonical::parse_abstract(b"{\"k\": !!null null}").unwrap();
        let encoded = encode(&ast, TOML_LIKE).unwrap();
        let first = encoded.kind_of(encoded.root).as_mapping().unwrap().unwrap();
        let kv = encoded.kind_of(first).as_keyvalue().unwrap();
        assert!(matches!(encoded.kind_of(kv.value), NodeKind::Mapping(_)));
        assert!(encoded.tag_of(kv.value).is_none());
        // And decoded, the null is back, its wrapper's comments and no tag.
        let decoded = decode(&encoded).unwrap();
        assert!(matches!(decoded.get_val_by_path(&[P::Key(b"k")]).unwrap().kind, NodeKind::Null));
        if let Some(yaml) = entry("yaml") {
            let ast = yaml.parse(b"k: !!null null\n").unwrap().ast;
            let encoded = encode(&ast, TOML_LIKE).unwrap();
            let first = encoded.kind_of(encoded.root).as_mapping().unwrap().unwrap();
            let kv = encoded.kind_of(first).as_keyvalue().unwrap();
            assert!(encoded.tag_of(kv.value).is_none());
        }
    }

    #[test]
    fn decode_leaves_a_non_envelope_fig_mapping_untouched() {
        // value is a number, not the inner mapping shape → ordinary data.
        let ast = crate::canonical::parse_abstract(b"{\"$fig\": 1}").unwrap();
        let decoded = decode(&ast).unwrap();
        assert_eq!(b"1", decoded.get_val_by_path(&[P::Key(b"$fig")]).unwrap().kind.as_number().unwrap().raw);
        let Some(json_n) = native("json") else { return };
        let json = convert("json", "json", json_n, b"{\"$fig\": 1}").unwrap();
        assert_eq!("{\n  \"$fig\": 1\n}\n", json);
    }

    #[test]
    fn an_envelope_round_trips_through_the_canonical_form() {
        let ast = crate::canonical::parse_abstract(
            b"{\"d\": @local_date \"1979-05-27\", \"n\": null, \"e\": @enum_literal \"fast\", \"i\": 1, \"t\": !!int \"2\"}",
        )
        .unwrap();
        let encoded = encode(&ast, TOML_LIKE).unwrap();
        // The date stays (TOML holds it); the null and the enum are wrapped.
        assert!(matches!(encoded.get_val_by_path(&[P::Key(b"d")]).unwrap().kind, NodeKind::Extended(_)));
        let n = encoded.get_val_by_path(&[P::Key(b"n"), P::Key(b"$fig"), P::Key(b"t")]).unwrap();
        assert_eq!(b"null", n.kind.as_string().unwrap());
        let v = encoded.get_val_by_path(&[P::Key(b"e"), P::Key(b"$fig"), P::Key(b"v")]).unwrap();
        assert_eq!(b"fast", v.kind.as_string().unwrap());
        // A tag on a scalar that stays rides along.
        let t = encoded.get_val_by_path(&[P::Key(b"t")]).unwrap().id;
        assert_eq!(b"!!int", encoded.tag_of(t).map(|t: Tag<'_>| t.spelling()).unwrap());
        let decoded = decode(&encoded).unwrap();
        let mut a = Vec::new();
        let mut b = Vec::new();
        crate::canonical::print(&mut a, &ast).unwrap();
        crate::canonical::print(&mut b, &decoded).unwrap();
        assert_eq!(String::from_utf8_lossy(&a), String::from_utf8_lossy(&b));
        assert_eq!(NumberKind::Integer, decoded.get_val_by_path(&[P::Key(b"i")]).unwrap().kind.as_number().unwrap().kind);
    }

    /// One `Extended` of kind `k`.
    fn ext(k: ExtKind) -> NodeKind<'static> {
        NodeKind::Extended(Extended { kind: k, text: b"" })
    }

    // The envelope table as it was written before the formats declared it, pinned
    // against the declarations that replaced it. `needsEnvelope` used to be a
    // hand-written switch over `enum { json, yaml, toml, zon }` × kind; the four
    // `caps.lossless` declarations are now the only source, so this is the one
    // place that still states the OLD answers — a declaration that drifts from
    // them fails here, not in a round-trip that quietly changed shape.
    #[test]
    fn declared_native_kinds_reproduce_the_pre_declaration_envelope_table() {
        let (Some(json_native), Some(yaml_native), Some(toml_native), Some(zon_native)) =
            (native("json"), native("yaml"), native("toml"), native("zon"))
        else {
            return;
        };
        use ExtKind as E;
        let rows: Vec<(NativeKinds, NodeKind<'static>, bool, bool)> = vec![
            // Only TOML lacks a null; JSON/YAML/ZON all have one.
            (json_native, NodeKind::Null, false, false),
            (yaml_native, NodeKind::Null, false, false),
            (zon_native, NodeKind::Null, false, false),
            (toml_native, NodeKind::Null, true, true),
            // TOML: datetimes and inf/nan native; enum/char and plist's two not.
            (toml_native, ext(E::OffsetDatetime), false, false),
            (toml_native, ext(E::LocalDatetime), false, false),
            (toml_native, ext(E::LocalDate), false, false),
            (toml_native, ext(E::LocalTime), false, false),
            (toml_native, ext(E::NumberSpecial), false, false),
            (toml_native, ext(E::EnumLiteral), true, false),
            (toml_native, ext(E::CharLiteral), true, false),
            (toml_native, ext(E::PlistDate), true, false),
            (toml_native, ext(E::Data), true, false),
            // ZON: enum/char native; everything else not.
            (zon_native, ext(E::EnumLiteral), false, false),
            (zon_native, ext(E::CharLiteral), false, false),
            (zon_native, ext(E::OffsetDatetime), true, false),
            (zon_native, ext(E::LocalDatetime), true, false),
            (zon_native, ext(E::LocalDate), true, false),
            (zon_native, ext(E::LocalTime), true, false),
            (zon_native, ext(E::NumberSpecial), true, false),
            (zon_native, ext(E::PlistDate), true, false),
            (zon_native, ext(E::Data), true, false),
            // Containers and the core scalars never envelope anywhere.
            (toml_native, NodeKind::Mapping(None), false, false),
            (toml_native, NodeKind::Sequence(None), false, false),
            (toml_native, NodeKind::String(b""), false, false),
            (toml_native, NodeKind::Boolean(true), false, false),
            (json_native, NodeKind::Mapping(None), false, false),
        ];
        for (n, kind, envelope, unrepresentable) in rows {
            assert_eq!(envelope, needs_envelope(n, &kind));
            assert_eq!(unrepresentable, is_unrepresentable(n, &kind));
        }
        // JSON and YAML: every extended kind is enveloped, none is dropped,
        // except that YAML holds a byte string as `!!binary`.
        for k in ExtKind::ALL {
            assert!(needs_envelope(json_native, &ext(k)));
            assert_eq!(k != E::Data, needs_envelope(yaml_native, &ext(k)));
            assert!(!is_unrepresentable(json_native, &ext(k)));
            assert!(!is_unrepresentable(yaml_native, &ext(k)));
        }
    }

    // `targetFor` used to answer `.json` for the three JSON dialects, `.yaml`,
    // `.toml`, `.zon` for their own, and null for the other formats. Pinned by
    // name against what each language registers.
    #[test]
    fn native_for_reproduces_the_pre_declaration_target_for_table() {
        let json_native = native("json");
        let cases: [(&str, Option<NativeKinds>); 13] = [
            ("json", json_native),
            ("jsonc", json_native),
            ("json5", json_native),
            ("yaml", native("yaml")),
            ("toml", native("toml")),
            ("zon", native("zon")),
            ("canonical", None),
            ("fig", None),
            ("ini", None),
            ("dotenv", None),
            ("properties", None),
            ("plist", None),
            ("nestedtext", None),
        ];
        for (fmt, want) in cases {
            // A language this build does not bundle has nothing registered to ask.
            let Some(target) = serialize::Target::named(fmt) else { continue };
            assert_eq!(want, native_for(&target), "{fmt}");
        }
        // And the four typed formats do declare one.
        for fmt in ["json", "yaml", "toml", "zon"] {
            if entry(fmt).is_some() {
                assert!(native(fmt).is_some(), "{fmt}");
            }
        }
    }
}
