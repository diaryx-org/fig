//! Port of src/materialize.zig.
//!
//! Materialization: collapse a document's reference/annotation layer (aliases,
//! merge keys, tags, anchors) into the universal core AST, so a printer for a
//! format without one — or any consumer that wants concrete data — never has
//! to know about it.
//!
//! The layer is YAML's, and the rules here are YAML's (the `<<` merge, the
//! core-schema `!!` tags), but the pass reads only the AST and its
//! side-tables, so it collapses the layer whichever language built it — the
//! compiled YAML or a runtime twin of it. Which languages HAVE the layer is
//! declared, as `Caps.references` (`manifest.rs`); the CLI and the C ABI run
//! this pass when a document leaves a language that declares it for one that
//! does not. `lossless` and `flat_strip` are its siblings, the other two
//! AST-to-AST passes a conversion may run.
//!
//! `materialize` returns a NEW AST:
//!   - aliases (`*name`)      → a deep copy of the anchor's subtree
//!   - merge keys (`<<`)      → flattened into the host mapping (host keys win;
//!                              earlier merge sources win over later)
//!   - core `!!` tags         → applied to the node's type, then dropped
//!   - unknown/custom tags    → strict: error; lax: dropped (node kept as parsed)
//!   - anchors                → dropped
//!
//! The third mode, `Keep`, collapses only the reference half: every tag rides
//! along on its node's copy, unapplied and unchecked, with the `%TAG`
//! directives that spell it. It is what a binding reads values through
//! (`fig_document_resolve_aliases`), which follows an alias but has never
//! applied a tag.
//!
//! The pass writes its tree as rows, into a table with no source, so the
//! result owns its strings: every text is copied into its own buffer, and it
//! outlives `ast` and the source `ast` was read from.
//!
//! Errors: `AliasCycle`, `UnknownTag`, `TagTypeMismatch`, and the
//! resolution errors (`UndefinedAlias`, `AliasCycle`, `TooDeep`,
//! `NotAMapping`).

use crate::err;
use crate::kit::ast::{KindTag, Node, NodeId, NodeKind, Number, NumberKind, Tag, AST};
use crate::kit::row_writer::RowWriter;
use crate::kit::table::{self, Row, Table};
// The core schema's number grammar, as the compiled YAML reads a plain
// scalar — pure functions of a lexeme, so a runtime twin's AST is judged by
// the same rules.
use crate::kit::yaml_schema::{self as yaml_scalar, NumberClass};
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagMode {
    Strict,
    Lax,
    Keep,
}

/// Collapse `ast`'s reference layer into a fresh core AST.
pub fn materialize(ast: &AST, mode: TagMode) -> Result<AST> {
    let mut culprit: Option<NodeId> = None;
    materialize_reporting(ast, mode, &mut culprit)
}

/// `materialize`, but on failure also sets `culprit` to the id (in `ast`) of
/// the node the pass failed on — the tagged node an `UnknownTag` or
/// `TagTypeMismatch` is about, the alias an `AliasCycle` came back through —
/// so a caller holding the node's source span can point at it. Untouched on
/// success.
pub fn materialize_reporting(ast: &AST, mode: TagMode, culprit: &mut Option<NodeId>) -> Result<AST> {
    let mut m = Materializer { src: ast, mode, t: Table::default(), path: Vec::new(), culprit: None };
    let root = match m.copy(ast.node(ast.root)) {
        Ok(r) => r,
        Err(e) => {
            *culprit = m.culprit;
            return Err(e);
        }
    };
    // A kept tag may be spelled through a `%TAG` handle, so its declaration
    // rides along too.
    if mode == TagMode::Keep {
        for d in ast.tag_directives() {
            m.writer().directive(d.handle, d.prefix)?;
        }
    }
    AST::own(&mut m.t, root, ast.merge_key)
}

struct Materializer<'a> {
    src: &'a AST,
    mode: TagMode,
    /// The rows of the tree being written, a row per node as `copy` emits it,
    /// with the comments and tags carried onto them. Every text is copied
    /// into its buffer: it has no source.
    t: Table,
    /// Anchor target ids currently being expanded, to catch structural cycles
    /// (`&a [*a]`) and self-merges (`&m { <<: *m }`).
    path: Vec<NodeId>,
    /// The innermost source node whose copy failed — the first `copy` frame
    /// an error unwinds through. See `materialize_reporting`.
    culprit: Option<NodeId>,
}

/// What node `id`'s tag says, as the language resolved it (§4.3 of
/// docs/proposals/core-only.md): a core type, and whether the node agrees
/// with it, or custom, or YAML's non-specific `!`. A tag its language left
/// unstated — a runtime twin's — is read by its text, with the document's
/// `%TAG` directives, and its agreement is judged here.
#[derive(Debug, Clone, Copy)]
enum Resolved {
    None,
    NonSpecific,
    Custom,
    Core { kind: KindTag, agrees: Option<bool> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    Seq,
    Map,
}

impl<'a> Materializer<'a> {
    fn writer(&mut self) -> RowWriter<'_> {
        RowWriter::init(&mut self.t)
    }

    /// A new row of `kind`, with no span: a keyvalue's key and value linked
    /// to it, a container's children added by `link`.
    fn emit(&mut self, kind: NodeKind<'_>) -> Result<NodeId> {
        let id = self.t.add_row(Row::default())?;
        self.writer().set(id, kind)?;
        Ok(id)
    }

    /// Copy the comments bound to source node `src_id` onto the freshly emitted
    /// node `new_id`, their texts into the table's buffer.
    fn carry(&mut self, src_id: NodeId, new_id: NodeId) -> Result<()> {
        let nc = self.src.comments(src_id);
        self.writer().comments(new_id, &nc)
    }

    fn enter(&mut self, id: NodeId) -> Result<()> {
        if self.path.contains(&id) {
            return Err(err!(AliasCycle));
        }
        self.path.push(id);
        Ok(())
    }

    fn leave(&mut self) {
        self.path.pop();
    }

    /// Deep-copy `node` into the output, expanding aliases/merges and applying
    /// tags. Returns the new node id.
    fn copy(&mut self, node: Node<'a>) -> Result<NodeId> {
        let r = self.copy_inner(node);
        if r.is_err() && self.culprit.is_none() {
            self.culprit = Some(node.id);
        }
        r
    }

    fn copy_inner(&mut self, node: Node<'a>) -> Result<NodeId> {
        match node.kind {
            NodeKind::Alias(_) => {
                let target = self.src.resolve_deep(&node)?;
                self.enter(target)?;
                let r = self.copy(self.src.node(target));
                self.leave();
                r
            }
            // `.extended` never appears in a YAML AST, but the match must be
            // exhaustive; treat it as a plain scalar for completeness.
            NodeKind::Null | NodeKind::Boolean(_) | NodeKind::Number(_) | NodeKind::Extended(_) | NodeKind::String(_) => {
                let kind = self.apply_scalar_tag(node)?;
                let id = self.emit(kind)?;
                self.carry(node.id, id)?;
                self.carry_scalar_tag(node.id, id)?;
                Ok(id)
            }
            NodeKind::Sequence(first) => {
                self.check_collection_tag(node, Want::Seq)?;
                let id = self.emit(NodeKind::Sequence(None))?;
                self.carry(node.id, id)?;
                self.keep_tag(node.id, id)?;
                let mut last: Option<NodeId> = None;
                let mut child = first;
                while let Some(cid) = child {
                    let new_id = self.copy(self.src.node(cid))?;
                    self.link(id, &mut last, new_id);
                    child = self.src.node(cid).next_sibling;
                }
                Ok(id)
            }
            NodeKind::Mapping(_) => self.copy_mapping(node),
            NodeKind::KeyValue(_) => unreachable!("keyvalues are only produced inside copy_mapping"),
        }
    }

    /// Append `child_id` to container `container_id`, after `last`.
    fn link(&mut self, container_id: NodeId, last: &mut Option<NodeId>, child_id: NodeId) {
        let rows = &mut self.t.rows;
        rows[child_id as usize].parent = container_id;
        match *last {
            Some(p) => rows[p as usize].next_sibling = child_id,
            None => rows[container_id as usize].first_child = child_id,
        }
        *last = Some(child_id);
    }

    fn copy_mapping(&mut self, node: Node<'a>) -> Result<NodeId> {
        self.check_collection_tag(node, Want::Map)?;
        let id = self.emit(NodeKind::Mapping(None))?;
        self.carry(node.id, id)?;
        self.keep_tag(node.id, id)?;
        let mut last: Option<NodeId> = None;
        let mut seen: Vec<&'a [u8]> = Vec::new();

        // Local entries first (they win); remember any `<<` merge value.
        let mut merge_value: Option<NodeId> = None;
        let mut entry = node.kind.as_mapping().flatten();
        while let Some(eid) = entry {
            let kv = self.src.kind_of(eid).as_keyvalue().expect("a mapping child is a keyvalue");
            entry = self.src.node(eid).next_sibling;
            if let Some(ks) = self.string_key(kv.key) {
                if self.is_merge_key(ks) {
                    merge_value = Some(kv.value);
                    continue;
                }
            }
            self.append_entry(id, &mut last, &mut seen, kv.key, kv.value)?;
        }
        if let Some(mv) = merge_value {
            self.merge_into(id, &mut last, &mut seen, mv)?;
        }
        Ok(id)
    }

    /// Copy a key/value pair into mapping `map_id` and record the (string) key.
    fn append_entry(&mut self, map_id: NodeId, last: &mut Option<NodeId>, seen: &mut Vec<&'a [u8]>, key: NodeId, value: NodeId) -> Result<()> {
        let key_str = self.string_key(key);
        let new_key = self.copy(self.src.node(key))?;
        let new_val = self.copy(self.src.node(value))?;
        let kvid = self.emit(NodeKind::KeyValue(crate::kit::ast::KeyValue { key: new_key, value: new_val }))?;
        self.link(map_id, last, kvid);
        if let Some(ks) = key_str {
            seen.push(ks);
        }
        Ok(())
    }

    /// Splice the entries of a `<<` merge value into mapping `map_id`, skipping
    /// keys already present (`seen`). Handles a single alias/mapping or a sequence
    /// of them (earlier sources win because they are processed first).
    fn merge_into(&mut self, map_id: NodeId, last: &mut Option<NodeId>, seen: &mut Vec<&'a [u8]>, mv: NodeId) -> Result<()> {
        let mvn = self.src.node(mv);
        match mvn.kind {
            NodeKind::Sequence(first) => {
                let mut e = first;
                while let Some(eid) = e {
                    self.merge_one_source(map_id, last, seen, self.src.node(eid))?;
                    e = self.src.node(eid).next_sibling;
                }
            }
            _ => self.merge_one_source(map_id, last, seen, mvn)?,
        }
        Ok(())
    }

    fn merge_one_source(&mut self, map_id: NodeId, last: &mut Option<NodeId>, seen: &mut Vec<&'a [u8]>, source: Node<'a>) -> Result<()> {
        let target_id = self.src.resolve_deep(&source)?;
        self.enter(target_id)?; // guards a self-referential merge
        let r = self.merge_entries(map_id, last, seen, target_id);
        self.leave();
        r
    }

    fn merge_entries(&mut self, map_id: NodeId, last: &mut Option<NodeId>, seen: &mut Vec<&'a [u8]>, target_id: NodeId) -> Result<()> {
        let NodeKind::Mapping(first) = self.src.kind_of(target_id) else {
            return Ok(()); // a non-mapping merge source is ignored
        };
        let mut entry = first;
        while let Some(eid) = entry {
            let kv = self.src.kind_of(eid).as_keyvalue().expect("a mapping child is a keyvalue");
            entry = self.src.node(eid).next_sibling;
            let key_str = self.string_key(kv.key);
            if let Some(ks) = key_str {
                if self.is_merge_key(ks) {
                    self.merge_into(map_id, last, seen, kv.value)?; // nested merge in a source
                    continue;
                }
                if seen.contains(&ks) {
                    continue; // shadowed by a local or earlier source
                }
            }
            self.append_entry(map_id, last, seen, kv.key, kv.value)?;
        }
        Ok(())
    }

    fn is_merge_key(&self, key: &[u8]) -> bool {
        match self.src.merge_key {
            Some(mk) => key == mk,
            None => false,
        }
    }

    fn string_key(&self, key: NodeId) -> Option<&'a [u8]> {
        match self.src.kind_of(key) {
            NodeKind::String(s) => Some(s),
            _ => None,
        }
    }

    // ── tags ──────────────────────────────────────────────────────────────

    fn resolve(&self, id: NodeId) -> Resolved {
        let Some(f) = self.src.tag_fact(id) else { return Resolved::None };
        if f.resolved >= table::TAG_KIND {
            let kind = KindTag::from_ordinal(f.resolved - table::TAG_KIND).expect("a tag resolved to a kind there is");
            return Resolved::Core { kind, agrees: Some(f.agrees != 0) };
        }
        if f.resolved == table::TAG_CUSTOM {
            return Resolved::Custom;
        }
        if f.resolved == table::TAG_NON_SPECIFIC {
            return Resolved::NonSpecific;
        }
        match self.src.tag_of_fact(f) {
            Tag::Kind(k) => Resolved::Core { kind: k, agrees: None },
            Tag::Text(t) => {
                if t == b"!" {
                    return Resolved::NonSpecific;
                }
                let directives = self.src.tag_directives();
                let Some(name) = yaml_scalar::core_tag_name(&directives, t) else { return Resolved::Custom };
                // `!!binary` on a byte string the language already made it.
                if name == b"binary" {
                    if let NodeKind::Extended(e) = self.src.kind_of(id) {
                        if e.kind == crate::kit::ast::ExtKind::Data {
                            return Resolved::NonSpecific;
                        }
                    }
                }
                match Tag::of_core_name(name) {
                    Some(k) => Resolved::Core { kind: k, agrees: None },
                    None => Resolved::Custom,
                }
            }
        }
    }

    fn apply_scalar_tag(&self, node: Node<'a>) -> Result<NodeKind<'a>> {
        if self.mode == TagMode::Keep {
            return Ok(node.kind);
        }
        let (kind, agrees) = match self.resolve(node.id) {
            Resolved::None | Resolved::NonSpecific => return Ok(node.kind),
            Resolved::Custom => return self.custom_tag(node.kind),
            Resolved::Core { kind, agrees } => (kind, agrees),
        };
        if agrees == Some(false) {
            return Err(err!(TagTypeMismatch));
        }
        match kind {
            KindTag::String => Ok(NodeKind::String(scalar_text(node))),
            KindTag::Null => Ok(NodeKind::Null),
            KindTag::Boolean => as_bool(node),
            KindTag::Integer => as_number(node, NumberKind::Integer, agrees),
            KindTag::Float => as_number(node, NumberKind::Float, agrees),
            KindTag::Sequence | KindTag::Mapping => Err(err!(TagTypeMismatch)),
        }
    }

    fn check_collection_tag(&self, node: Node<'a>, want: Want) -> Result<()> {
        if self.mode == TagMode::Keep {
            return Ok(());
        }
        match self.resolve(node.id) {
            Resolved::None | Resolved::NonSpecific => {}
            Resolved::Custom => {
                self.custom_tag(node.kind)?; // strict-errors on unknown; lax no-op
            }
            Resolved::Core { kind, .. } => {
                let ok = if want == Want::Seq { KindTag::Sequence } else { KindTag::Mapping };
                if kind != ok {
                    return Err(err!(TagTypeMismatch));
                }
            }
        }
        Ok(())
    }

    /// Keep an applied core SCALAR tag on the output node as a normalized `.kind`
    /// type tag, so a tag-aware target re-surfaces it (fig `: int =`, canonical
    /// `!!int`), while JSON/TOML/ZON ignore it (the value is already concrete).
    /// A custom/collection tag is not carried — it was already applied
    /// (collections) or errored/dropped (custom) by `apply_scalar_tag`.
    fn carry_scalar_tag(&mut self, src_id: NodeId, new_id: NodeId) -> Result<()> {
        if self.mode == TagMode::Keep {
            return self.keep_tag(src_id, new_id);
        }
        let Resolved::Core { kind, .. } = self.resolve(src_id) else { return Ok(()) };
        if matches!(kind, KindTag::Sequence | KindTag::Mapping) {
            return Ok(());
        }
        self.writer().tag(new_id, Tag::Kind(kind), table::TAG_UNSTATED, true, None)
    }

    /// Under `Keep`, put source node `src_id`'s tag, whatever it is, on its
    /// copy `new_id`. A no-op in the other modes, which apply or drop it.
    fn keep_tag(&mut self, src_id: NodeId, new_id: NodeId) -> Result<()> {
        if self.mode != TagMode::Keep {
            return Ok(());
        }
        let Some(tag) = self.src.tag_of(src_id) else { return Ok(()) };
        self.writer().tag(new_id, tag, table::TAG_UNSTATED, true, None)
    }

    /// A custom tag, which strict mode refuses and lax mode drops, keeping
    /// the node as parsed.
    fn custom_tag(&self, kind: NodeKind<'a>) -> Result<NodeKind<'a>> {
        if self.mode == TagMode::Strict {
            return Err(err!(UnknownTag));
        }
        Ok(kind)
    }
}

/// A `!!int`/`!!float` payload, refused unless its text reads as that
/// kind — every printer writes a number's text bare, so `!!int 1 - 3`
/// would print as `1 - 3` in JSON. Read by YAML 1.2's core schema or
/// 1.1's, since the AST does not say which the document was: `!!int
/// 0x1A` and `!!int 1_000` both pass. A `!!float` also takes a decimal
/// integer's text (`!!float 1`), as the core schema's float does.
fn as_number<'a>(node: Node<'a>, want: NumberKind, stated: Option<bool>) -> Result<NodeKind<'a>> {
    let t = scalar_text(node);
    let ok = match stated {
        Some(x) => x,
        None => match want {
            NumberKind::Integer => {
                yaml_scalar::classify_number(t) == NumberClass::Integer || yaml_scalar::classify1_1_number(t) == NumberClass::Integer
            }
            NumberKind::Float => {
                (yaml_scalar::classify_number(t) != NumberClass::NotNumber && !yaml_scalar::has_radix_prefix(t))
                    || yaml_scalar::classify1_1_number(t) == NumberClass::Float
            }
        },
    };
    if !ok {
        return Err(err!(TagTypeMismatch));
    }
    Ok(NodeKind::Number(Number { raw: t, kind: want }))
}

fn as_bool<'a>(node: Node<'a>) -> Result<NodeKind<'a>> {
    let t = scalar_text(node);
    if [&b"true"[..], b"True", b"TRUE"].contains(&t) {
        return Ok(NodeKind::Boolean(true));
    }
    if [&b"false"[..], b"False", b"FALSE"].contains(&t) {
        return Ok(NodeKind::Boolean(false));
    }
    Err(err!(TagTypeMismatch))
}

/// The scalar's text for `!!str`-style forcing. Numbers and strings are exact;
/// bool/null are reconstructed in canonical form (the original presentation is
/// not retained on the AST, which is source-free).
fn scalar_text<'a>(node: Node<'a>) -> &'a [u8] {
    match node.kind {
        NodeKind::String(s) => s,
        NodeKind::Number(n) => n.raw,
        NodeKind::Boolean(b) => {
            if b {
                b"true"
            } else {
                b"false"
            }
        }
        NodeKind::Null => b"null",
        _ => b"",
    }
}

// ── tests ────────────────────────────────────────────────────────────────
//
// These exercise the pass over what the bundled YAML parser builds, and
// return early while the global registry has no `yaml` (before the
// languages are wired into `registry::bundled_languages`). The last runs
// over the canonical parser, which spells anchors, aliases and tags too.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::PathSegment as P;
    use crate::kit::document::Document;
    use crate::registry;

    fn has_alias(ast: &AST) -> bool {
        (0..ast.node_count()).any(|i| matches!(ast.kind_of(i as NodeId), NodeKind::Alias(_)))
    }

    /// `src` as the bundled YAML reads it (the `yaml` row, YAML 1.2.2).
    fn parse_yaml(src: &[u8]) -> Option<Document> {
        let e = registry::global().entry_by_name("yaml")?;
        Some(e.parse(src).unwrap())
    }

    fn raw(ast: &AST, path: &[P<'_>]) -> Vec<u8> {
        ast.get_val_by_path(path).unwrap().kind.as_number().unwrap().raw.to_vec()
    }

    #[test]
    fn alias_expands_to_a_copied_subtree() {
        let Some(doc) = parse_yaml(b"a: &x [1, 2]\nb: *x\n") else { return };
        let mat = materialize(&doc.ast, TagMode::Strict).unwrap();
        assert!(!has_alias(&mat)); // no alias survives
        let b = mat.get_val_by_path(&[P::Key(b"b")]).unwrap();
        assert!(matches!(b.kind, NodeKind::Sequence(_)));
        assert_eq!(b"1".to_vec(), raw(&mat, &[P::Key(b"b"), P::Index(0)]));
        // The copy is independent of the original `a` (distinct node ids).
        let a = mat.get_val_by_path(&[P::Key(b"a")]).unwrap();
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn merge_flattens_with_host_and_earlier_source_precedence() {
        let src = b"base: &b\n  x: 1\n  y: 2\nover: &o\n  x: 9\nd:\n  <<: [*o, *b]\n  y: 3\n";
        let Some(doc) = parse_yaml(src) else { return };
        let mat = materialize(&doc.ast, TagMode::Strict).unwrap();
        assert_eq!(b"3".to_vec(), raw(&mat, &[P::Key(b"d"), P::Key(b"y")]));
        assert_eq!(b"9".to_vec(), raw(&mat, &[P::Key(b"d"), P::Key(b"x")]));
        // The `<<` key itself is gone from the flattened mapping.
        assert_eq!(err!(NotFound), mat.get_val_by_path(&[P::Key(b"d"), P::Key(b"<<")]).unwrap_err());
    }

    #[test]
    fn core_tags_applied_and_kept_as_normalized_kind_tags() {
        let Some(doc) = parse_yaml(b"a: !!str 123\nb: !!int \"42\"\nc: !!null x\nd: 7\n") else { return };
        let mat = materialize(&doc.ast, TagMode::Strict).unwrap();
        // The tag is applied to the node's kind …
        let a = mat.get_val_by_path(&[P::Key(b"a")]).unwrap();
        assert_eq!(b"123", a.kind.as_string().unwrap());
        let b = mat.get_val_by_path(&[P::Key(b"b")]).unwrap();
        assert_eq!(NumberKind::Integer, b.kind.as_number().unwrap().kind);
        assert!(matches!(mat.get_val_by_path(&[P::Key(b"c")]).unwrap().kind, NodeKind::Null));
        // … and its type identity is KEPT as a normalized `.kind` tag.
        assert_eq!(Some(Tag::Kind(KindTag::String)), mat.tag_of(a.id));
        assert_eq!(Some(Tag::Kind(KindTag::Integer)), mat.tag_of(b.id));
        // An untagged scalar carries no tag.
        assert!(mat.tag_of(mat.get_val_by_path(&[P::Key(b"d")]).unwrap().id).is_none());
    }

    #[test]
    fn unknown_tag_strict_errors_lax_drops() {
        let Some(doc) = parse_yaml(b"x: !custom 1\n") else { return };
        assert_eq!(err!(UnknownTag), materialize(&doc.ast, TagMode::Strict).unwrap_err());
        let mat = materialize(&doc.ast, TagMode::Lax).unwrap();
        assert_eq!(b"1".to_vec(), raw(&mat, &[P::Key(b"x")]));
    }

    #[test]
    fn materialize_reporting_names_the_node_a_tag_failure_is_about() {
        let src = b"ok: 1\nlist:\n  - 2\n  - !custom 3\n";
        let Some(doc) = parse_yaml(src) else { return };
        let mut culprit = None;
        assert_eq!(err!(UnknownTag), materialize_reporting(&doc.ast, TagMode::Strict, &mut culprit).unwrap_err());
        // The tagged item itself, not the sequence or mapping around it: its tag
        // span is what a report underlines.
        let span = doc.tag_span(&doc.ast.node(culprit.unwrap())).unwrap();
        assert_eq!(b"!custom", &src[span.start..span.end]);

        // Reached through an alias, the failure is still about the anchored node
        // that carries the tag.
        let aliased = b"a: &x !custom 1\nb: *x\n";
        let doc2 = parse_yaml(aliased).unwrap();
        culprit = None;
        assert_eq!(err!(UnknownTag), materialize_reporting(&doc2.ast, TagMode::Strict, &mut culprit).unwrap_err());
        let span2 = doc2.tag_span(&doc2.ast.node(culprit.unwrap())).unwrap();
        assert_eq!(b"!custom", &aliased[span2.start..span2.end]);
    }

    #[test]
    fn collection_tag_mismatch_and_cyclic_alias_error() {
        let Some(doc) = parse_yaml(b"x: !!str [1, 2]\n") else { return };
        assert_eq!(err!(TagTypeMismatch), materialize(&doc.ast, TagMode::Strict).unwrap_err());
        let doc = parse_yaml(b"- &a [*a]\n").unwrap();
        assert_eq!(err!(AliasCycle), materialize(&doc.ast, TagMode::Strict).unwrap_err());
    }

    #[test]
    fn a_numeric_tag_refuses_a_payload_that_is_not_that_kind_of_number() {
        let cases: [(&[u8], bool); 9] = [
            (b"x: !!int 1 - 3\n", false),
            (b"x: !!int 1.5\n", false),
            (b"x: !!float abc\n", false),
            (b"x: !!float 0x1A\n", false),
            (b"x: !!int 0x1A\n", true),
            (b"x: !!int \"42\"\n", true),
            (b"x: !!int 1_000\n", true), // YAML 1.1's spelling
            (b"x: !!float 1\n", true),
            (b"x: !!float -.inf\n", true),
        ];
        for (src, ok) in cases {
            let Some(doc) = parse_yaml(src) else { return };
            let got = materialize(&doc.ast, TagMode::Strict);
            if ok {
                got.unwrap();
            } else {
                assert_eq!(err!(TagTypeMismatch), got.unwrap_err());
            }
        }
    }

    #[test]
    fn a_tag_directive_that_rebinds_bang_bang_or_bang_decides_whether_a_tag_is_core() {
        // P76L: `!!` bound elsewhere makes `!!int` a custom tag.
        let Some(doc) = parse_yaml(b"%TAG !! tag:example.com,2000:app/\n---\n!!int 1 - 3\n") else { return };
        assert_eq!(err!(UnknownTag), materialize(&doc.ast, TagMode::Strict).unwrap_err());
        // `!` bound to the core prefix makes `!int` the core int.
        let doc = parse_yaml(b"%TAG ! tag:yaml.org,2002:\n---\nx: !int \"7\"\n").unwrap();
        let mat = materialize(&doc.ast, TagMode::Strict).unwrap();
        let x = mat.get_val_by_path(&[P::Key(b"x")]).unwrap();
        assert_eq!(NumberKind::Integer, x.kind.as_number().unwrap().kind);
    }

    #[test]
    fn keep_collapses_aliases_and_merges_and_leaves_every_tag_as_written() {
        let src = b"base: &b {x: !!str 1, y: !custom 2}\nd:\n  <<: *b\n  y: 3\ne: *b\n";
        let Some(doc) = parse_yaml(src) else { return };
        let mat = materialize(&doc.ast, TagMode::Keep).unwrap();
        assert!(!has_alias(&mat));
        // The merge is flattened, the host's key winning …
        assert_eq!(b"3".to_vec(), raw(&mat, &[P::Key(b"d"), P::Key(b"y")]));
        assert_eq!(err!(NotFound), mat.get_val_by_path(&[P::Key(b"d"), P::Key(b"<<")]).unwrap_err());
        // … and `!!str 1` is still the number it was read as, its tag carried
        // verbatim onto the merged copy and the alias's copy alike.
        for host in [&b"d"[..], b"e"] {
            let x = mat.get_val_by_path(&[P::Key(host), P::Key(b"x")]).unwrap();
            assert_eq!(b"1", x.kind.as_number().unwrap().raw);
            assert_eq!(b"!!str", mat.tag_of(x.id).unwrap().spelling());
        }
        // A custom tag is neither refused nor dropped.
        let ey = mat.get_val_by_path(&[P::Key(b"e"), P::Key(b"y")]).unwrap();
        assert_eq!(Some(Tag::Text(b"!custom")), mat.tag_of(ey.id));
    }

    #[test]
    fn a_merge_key_merges_only_where_the_language_declares_it_as_its_merge_key() {
        let Some(mut doc) = parse_yaml(b"base: &b {x: 1}\nd: {<<: *b}\n") else { return };
        let merged = materialize(&doc.ast, TagMode::Strict).unwrap();
        merged.get_val_by_path(&[P::Key(b"d"), P::Key(b"x")]).unwrap();

        // The same tree from a language with no merge key keeps `<<` as a key.
        doc.ast.merge_key = None;
        let kept = materialize(&doc.ast, TagMode::Strict).unwrap();
        assert_eq!(err!(NotFound), kept.get_val_by_path(&[P::Key(b"d"), P::Key(b"x")]).unwrap_err());
        kept.get_val_by_path(&[P::Key(b"d"), P::Key(b"<<")]).unwrap();
    }

    #[test]
    fn the_layer_collapses_whichever_parser_built_it() {
        // The canonical parser spells the same layer: its anchors, aliases,
        // and tags left unstated, read here by their text.
        let mut ast = crate::canonical::parse_abstract(
            b"{\"base\": &b {\"x\": !!str 1}, \"d\": {\"<<\": *b, \"y\": 2}, \"e\": *b, \"n\": !!int \"0x1A\"}",
        )
        .unwrap();
        ast.merge_key = Some(b"<<");
        let mat = materialize(&ast, TagMode::Strict).unwrap();
        assert!(!has_alias(&mat));
        let x = mat.get_val_by_path(&[P::Key(b"d"), P::Key(b"x")]).unwrap();
        assert_eq!(b"1", x.kind.as_string().unwrap());
        assert_eq!(Some(Tag::Kind(KindTag::String)), mat.tag_of(x.id));
        assert_eq!(NumberKind::Integer, mat.get_val_by_path(&[P::Key(b"n")]).unwrap().kind.as_number().unwrap().kind);
        let mut culprit = None;
        let cyclic = crate::canonical::parse_abstract(b"[&a [*a]]").unwrap();
        assert_eq!(err!(AliasCycle), materialize_reporting(&cyclic, TagMode::Strict, &mut culprit).unwrap_err());
        assert!(matches!(cyclic.kind_of(culprit.unwrap()), NodeKind::Alias(_)));
    }
}
