//! Port of src/cli/gron.zig.
//!
//! gron — a CLI-only, line-oriented projection of the AST.
//!
//! `gron` "explodes" a document into one `path = value` assignment per leaf, so
//! arbitrarily nested data becomes greppable, line-addressable text:
//!
//! ```text
//!     json = {};
//!     json.tags = [];
//!     json.tags[0] = "zig";
//!     json.name = "fig";
//! ```
//!
//! and `-i gron` reverses it ("ungron"), reconstructing the AST — so a value can
//! be exploded, filtered with `grep`/`sed`, and converted back (to gron or any
//! other format) with no structural loss. Fidelity equals JSON's: the RHS of
//! every line is a JSON value, so comments and the YAML reference layer
//! (anchors/tags) do not survive, exactly as a JSON round-trip would drop them.
//!
//! This format lives entirely in the CLI binary. It is NOT a core
//! serialize target, is absent from the C ABI / language bindings, and is
//! never content-sniffed by `Language.detect` (a flat `a = 1` is ambiguous with
//! TOML, so gron must be selected explicitly). It derives straight from the
//! public AST + `Builder`, which is the whole point: it demonstrates that a
//! new format convertible to/from every other fig format needs only a
//! parse(bytes → AST) / print(AST → bytes) pair over the public surface — no
//! changes to the core enums, dispatch, or ABI.
//!
//! gron is "exploded JSON", so it reuses fig's JSON parser (for each RHS value)
//! and JSON printer (to render each RHS) verbatim.

use std::sync::Arc;

use fig_core::kit::ast::{Builder, Entry, NodeId, NodeKind, NumberKind, SerializeOptions, AST};
use fig_core::kit::document::Document;
use fig_core::lang::json::{self as json, Type as JsonType};
use fig_core::{err, Result};

/// The customizable surface of the projection. The field defaults reproduce
/// gron's exact syntax (also exposed as `Projection::gron()`); overrides
/// change only the *printed* form. Note what is deliberately **not** here: the
/// newline between records. Line-orientation is the whole point of the format
/// (greppable, line-addressable), and the parser hard-splits on `\n`, so the
/// record separator is structural, not a knob.
///
/// Because ungron stays maximally tolerant — it discards whatever leading
/// identifier it sees and strips an optional trailing `;` — `root_name` and
/// `terminator` round-trip for free under the default parser. `assign` is the
/// one load-bearing knob: ungron always splits on the default ` = `, so a custom
/// separator is a one-way (print-only) projection unless it matches the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    /// The identifier every path hangs off (`json` in gron). Free-form: ungron
    /// skips whatever leading token it finds, so any root reverses — except one
    /// containing a `.` or `[`, which would be misread as path structure.
    pub root_name: Vec<u8>,
    /// What separates a path from its value (` = ` in gron). Print-only; see the
    /// struct doc — ungron always splits on ` = `.
    pub assign: Vec<u8>,
    /// Trailing punctuation after each value, before the newline (`;` in gron).
    /// May be empty. Cosmetic: ungron strips an optional `;` regardless.
    pub terminator: Vec<u8>,
}

impl Default for Projection {
    fn default() -> Self {
        Projection { root_name: b"json".to_vec(), assign: b" = ".to_vec(), terminator: b";".to_vec() }
    }
}

impl Projection {
    /// The gron preset: upstream gron's exact syntax. Equal to the field
    /// defaults, named so call sites read intentionally.
    pub fn gron() -> Projection {
        Projection::default()
    }
}

// ── Printing (AST → gron) ───────────────────────────────────────────────────

/// A path segment, built one link per nesting level on the call stack — the same
/// linked-list-on-the-stack the TOML printer uses for its `[header.path]`s. The
/// full path is re-rendered on demand at each leaf, so no buffer is allocated.
struct Path<'p> {
    seg: Seg,
    parent: Option<&'p Path<'p>>,
}

#[derive(Clone, Copy)]
enum Seg {
    Root,
    Key(NodeId),
    Index(usize),
}

/// Emit the subtree rooted at `node_id` as gron, one assignment per line
/// (trailing newline on each). `proj` selects the printed syntax — pass
/// `Projection::gron()` for upstream gron.
///
/// gron renders every value — and every bracketed key segment — through the
/// JSON printer, so it inherits that printer's refusals: `UnresolvedAlias` for
/// an unmaterialized YAML alias, `NonStringKey` for a collection used as an
/// object key *inside* a bracketed key segment, and `NonFiniteNumber`.
pub fn print_node(writer: &mut Vec<u8>, ast: &AST, node_id: NodeId, proj: &Projection) -> Result<()> {
    emit(writer, ast, node_id, &Path { seg: Seg::Root, parent: None }, proj)
}

fn emit(writer: &mut Vec<u8>, ast: &AST, id: NodeId, path: &Path<'_>, proj: &Projection) -> Result<()> {
    match ast.node(id).kind {
        // A container emits its own `= {}` / `= []` line first (so ungron can
        // create the empty container before its children populate it), then
        // recurses with the child's path appended.
        NodeKind::Mapping(first) => {
            emit_line(writer, ast, path, b"{}", proj)?;
            let mut cur = first;
            while let Some(kv_id) = cur {
                let kv = ast.node(kv_id).kind.as_keyvalue().expect("a mapping entry");
                emit(writer, ast, kv.value, &Path { seg: Seg::Key(kv.key), parent: Some(path) }, proj)?;
                cur = ast.node(kv_id).next_sibling;
            }
        }
        NodeKind::Sequence(first) => {
            emit_line(writer, ast, path, b"[]", proj)?;
            let mut index = 0usize;
            let mut cur = first;
            while let Some(el_id) = cur {
                emit(writer, ast, el_id, &Path { seg: Seg::Index(index), parent: Some(path) }, proj)?;
                cur = ast.node(el_id).next_sibling;
                index += 1;
            }
        }
        // Any leaf (scalar, extended, alias) renders its RHS via the JSON
        // printer, so escaping, number normalization, and the JSON degradation
        // of extended scalars (datetimes → string, etc.) all stay in lockstep
        // with the `json` format — and the line reparses as JSON on ungron.
        _ => {
            render_path(writer, ast, path, proj)?;
            writer.extend_from_slice(&proj.assign);
            emit_value(writer, ast, id)?;
            writer.extend_from_slice(&proj.terminator);
            writer.push(b'\n');
        }
    }
    Ok(())
}

/// Emit one `<path><assign><rhs><terminator>\n` line whose RHS is a literal
/// (the empty-container `{}` / `[]` markers). Leaves render their RHS through the
/// JSON printer instead and so write the pieces inline above.
fn emit_line(writer: &mut Vec<u8>, ast: &AST, path: &Path<'_>, rhs: &[u8], proj: &Projection) -> Result<()> {
    render_path(writer, ast, path, proj)?;
    writer.extend_from_slice(&proj.assign);
    writer.extend_from_slice(rhs);
    writer.extend_from_slice(&proj.terminator);
    writer.push(b'\n');
    Ok(())
}

/// Walk to the root link, then unwind, writing each segment in path order.
fn render_path(writer: &mut Vec<u8>, ast: &AST, path: &Path<'_>, proj: &Projection) -> Result<()> {
    if let Some(p) = path.parent {
        render_path(writer, ast, p, proj)?;
    }
    match path.seg {
        Seg::Root => writer.extend_from_slice(&proj.root_name),
        Seg::Index(i) => writer.extend_from_slice(format!("[{i}]").as_bytes()),
        Seg::Key(key_id) => key_segment(writer, ast, key_id)?,
    }
    Ok(())
}

/// A string key that is a bare identifier prints as `.key`; anything else
/// (a key needing escapes, or a non-string key) prints as a bracketed,
/// JSON-quoted `["key"]` — the inverse of what `parse_path` accepts.
fn key_segment(writer: &mut Vec<u8>, ast: &AST, key_id: NodeId) -> Result<()> {
    match ast.node(key_id).kind {
        NodeKind::String(s) if is_bare_identifier(s) => {
            writer.push(b'.');
            writer.extend_from_slice(s);
        }
        _ => {
            writer.push(b'[');
            emit_value(writer, ast, key_id)?;
            writer.push(b']');
        }
    }
    Ok(())
}

/// Render a single node as a compact JSON value (the gron RHS / bracket key).
fn emit_value(writer: &mut Vec<u8>, ast: &AST, id: NodeId) -> Result<()> {
    json::printer::print_node(writer, ast, id, 0, &SerializeOptions { pretty: false, ..SerializeOptions::default() })
}

/// The ASCII identifier shape printed unquoted as `.key` and accepted unquoted
/// by `parse_path` — the two MUST agree, so the predicate is defined once here.
fn is_bare_identifier(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    for (i, &c) in name.iter().enumerate() {
        if !(if i == 0 { is_ident_start(c) } else { is_ident_part(c) }) {
            return false;
        }
    }
    true
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_uppercase() || c == b'_' || c == b'$'
}

fn is_ident_part(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

// ── Parsing (gron → AST, "ungron") ──────────────────────────────────────────

/// Parse gron text into a `Document`. The returned document holds `input` as
/// its source and carries placeholder spans (gron has no source-coupled
/// editing). Errors: `InvalidGron`, and the JSON parser's for a malformed RHS.
pub fn parse_document(input: &[u8]) -> Result<Document> {
    // Build a mutable tree top-down (gron emits parents before children), then
    // freeze it bottom-up through the Builder.
    let mut root = GNode::default();
    for raw in input.split(|&c| c == b'\n') {
        let line = trim(raw, b" \t\r");
        if line.is_empty() {
            continue;
        }
        apply_line(&mut root, line)?;
    }

    let mut b = Builder::init();
    let root_id = build_node(&mut b, &root)?;
    let ast = b.finish(root_id)?;
    // gron carries no source spans: every node's is the empty span at 0.
    Ok(Document { source: Arc::from(input), ast })
}

fn trim<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let start = s.iter().position(|c| !set.contains(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !set.contains(c)).map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

/// The intermediate tree node. `Unset` is a container slot created while
/// descending a path before its own `= {}` / `= []` line has been seen.
#[derive(Default)]
enum GNode {
    #[default]
    Unset,
    /// Verbatim RHS text, JSON-parsed at build time.
    Leaf(Vec<u8>),
    Object(Vec<(Vec<u8>, GNode)>),
    Array(Vec<GNode>),
}

enum PSeg {
    Key(Vec<u8>),
    Index(usize),
}

fn apply_line(root: &mut GNode, line: &[u8]) -> Result<()> {
    // Drop one optional trailing `;`, then split on the first ` = `.
    let mut stmt = line;
    if stmt.last() == Some(&b';') {
        stmt = trim(&stmt[..stmt.len() - 1], b" \t\r");
    }
    let eq = find(stmt, b" = ").ok_or(err!(InvalidGron))?;
    let lhs = trim(&stmt[..eq], b" \t\r");
    let rhs = trim(&stmt[eq + 3..], b" \t\r");

    let mut segs: Vec<PSeg> = Vec::new();
    parse_path(lhs, &mut segs)?;

    // Navigate from the root through the descend segments, creating containers
    // as needed, then assign the value at the destination node.
    let mut node = root;
    for seg in &segs {
        node = descend(node, seg)?;
    }
    set_value(node, rhs);
    Ok(())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Parse the LHS path: a leading root token (its name discarded), then a run of
/// `.key` / `["key"]` / `[index]` segments. The root segment itself is not
/// emitted — only the descent from it.
///
/// The root is skipped by consuming everything up to the first separator (`.` or
/// `[`), NOT by matching an identifier: the printer's `root_name` is freely
/// customizable (`adams-archive`, etc.) and never round-tripped, so ungron must
/// accept whatever leading token it finds. A root containing a `.` or `[` is the
/// one shape that can't survive (it would be read as path structure).
fn parse_path(lhs: &[u8], out: &mut Vec<PSeg>) -> Result<()> {
    let mut i = 0;
    while i < lhs.len() && lhs[i] != b'.' && lhs[i] != b'[' {
        i += 1;
    }
    if i == 0 {
        return Err(err!(InvalidGron)); // empty LHS, or a leading separator
    }

    while i < lhs.len() {
        match lhs[i] {
            b'.' => {
                i += 1;
                let start = i;
                if i >= lhs.len() || !is_ident_start(lhs[i]) {
                    return Err(err!(InvalidGron));
                }
                while i < lhs.len() && is_ident_part(lhs[i]) {
                    i += 1;
                }
                out.push(PSeg::Key(lhs[start..i].to_vec()));
            }
            b'[' => {
                i += 1;
                if i < lhs.len() && lhs[i] == b'"' {
                    // Quoted key: scan to the closing unescaped quote, then JSON
                    // decode the literal (escapes and all) to the real key bytes.
                    let start = i;
                    i += 1;
                    while i < lhs.len() && lhs[i] != b'"' {
                        if lhs[i] == b'\\' {
                            i += 1; // skip the escaped byte
                        }
                        i += 1;
                    }
                    if i >= lhs.len() {
                        return Err(err!(InvalidGron));
                    }
                    i += 1; // past closing quote
                    let literal = &lhs[start..i];
                    if i >= lhs.len() || lhs[i] != b']' {
                        return Err(err!(InvalidGron));
                    }
                    i += 1; // past ']'
                    out.push(PSeg::Key(decode_json_string(literal)?));
                } else {
                    let start = i;
                    while i < lhs.len() && lhs[i] != b']' {
                        i += 1;
                    }
                    if i >= lhs.len() {
                        return Err(err!(InvalidGron));
                    }
                    let digits = &lhs[start..i];
                    i += 1; // past ']'
                    let index = parse_usize(digits).ok_or(err!(InvalidGron))?;
                    out.push(PSeg::Index(index));
                }
            }
            _ => return Err(err!(InvalidGron)),
        }
    }
    Ok(())
}

/// `std.fmt.parseInt(usize, s, 10)`: an optional `+`, then decimal digits
/// (with `_` separators between them), no overflow.
pub fn parse_usize(s: &[u8]) -> Option<usize> {
    let digits = match s.first() {
        Some(b'+') => &s[1..],
        _ => s,
    };
    if digits.is_empty() || digits[0] == b'_' || digits[digits.len() - 1] == b'_' {
        return None;
    }
    let mut v: usize = 0;
    let mut prev_us = false;
    for &c in digits {
        if c == b'_' {
            if prev_us {
                return None;
            }
            prev_us = true;
            continue;
        }
        prev_us = false;
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((c - b'0') as usize)?;
    }
    Some(v)
}

/// Decode a JSON string literal (quotes included) to its bytes, reusing the JSON
/// parser so escape handling matches the printer exactly.
fn decode_json_string(literal: &[u8]) -> Result<Vec<u8>> {
    let doc = json::parser::parse(literal, JsonType::JSON)?;
    match doc.ast.node(doc.ast.root).kind {
        NodeKind::String(s) => Ok(s.to_vec()),
        _ => Err(err!(InvalidGron)),
    }
}

/// Descend (creating as needed) one level into `node`, auto-vivifying the
/// container kind the segment implies. A key needs an object; an index needs an
/// array, grown with `Unset` slots up to that index.
fn descend<'a>(node: &'a mut GNode, seg: &PSeg) -> Result<&'a mut GNode> {
    match seg {
        PSeg::Key(key) => {
            if let GNode::Unset = node {
                *node = GNode::Object(Vec::new());
            }
            let GNode::Object(members) = node else { return Err(err!(InvalidGron)) };
            let at = match members.iter().position(|(k, _)| k == key) {
                Some(at) => at,
                None => {
                    members.push((key.clone(), GNode::Unset));
                    members.len() - 1
                }
            };
            Ok(&mut members[at].1)
        }
        PSeg::Index(index) => {
            if let GNode::Unset = node {
                *node = GNode::Array(Vec::new());
            }
            let GNode::Array(items) = node else { return Err(err!(InvalidGron)) };
            while items.len() <= *index {
                items.push(GNode::Unset);
            }
            Ok(&mut items[*index])
        }
    }
}

/// Assign a line's RHS to its destination node. `{}` / `[]` establish an empty
/// container (preserving any children already vivified into it); anything else
/// is a leaf whose verbatim text is JSON-parsed at build time.
fn set_value(node: &mut GNode, rhs: &[u8]) {
    if rhs == b"{}" {
        if !matches!(node, GNode::Object(_)) {
            *node = GNode::Object(Vec::new());
        }
    } else if rhs == b"[]" {
        if !matches!(node, GNode::Array(_)) {
            *node = GNode::Array(Vec::new());
        }
    } else {
        *node = GNode::Leaf(rhs.to_vec());
    }
}

fn build_node(b: &mut Builder, node: &GNode) -> Result<NodeId> {
    match node {
        // A path was descended through but never assigned: treat as null.
        GNode::Unset => b.add_null(),
        GNode::Leaf(text) => build_leaf(b, text),
        GNode::Object(members) => {
            let mut entries = Vec::with_capacity(members.len());
            for (k, n) in members {
                let key_id = b.add_string(k)?;
                let val_id = build_node(b, n)?;
                entries.push(Entry { key: key_id, value: val_id });
            }
            b.add_mapping(&entries)
        }
        GNode::Array(items) => {
            let mut ids = Vec::with_capacity(items.len());
            for item in items {
                ids.push(build_node(b, item)?);
            }
            b.add_sequence(&ids)
        }
    }
}

/// Parse a leaf's RHS as a JSON value and copy it into the builder. Canonical
/// gron leaves are scalars, but a full JSON value is copied faithfully too, so a
/// hand-written or non-exploding gron stream still ungrons.
fn build_leaf(b: &mut Builder, text: &[u8]) -> Result<NodeId> {
    let ast = json::parser::parse_abstract(text, JsonType::JSON)?;
    copy_node(b, &ast, ast.root)
}

fn copy_node(b: &mut Builder, src: &AST, id: NodeId) -> Result<NodeId> {
    match src.node(id).kind {
        NodeKind::Null => b.add_null(),
        NodeKind::Boolean(v) => b.add_bool(v),
        NodeKind::Number(n) => b.add_number_raw(n.raw, n.kind == NumberKind::Float),
        NodeKind::String(s) => b.add_string(s),
        NodeKind::Extended(e) => b.add_extended(e.kind, e.text),
        NodeKind::Sequence(first) => {
            let mut ids = Vec::new();
            let mut cur = first;
            while let Some(c) = cur {
                ids.push(copy_node(b, src, c)?);
                cur = src.node(c).next_sibling;
            }
            b.add_sequence(&ids)
        }
        NodeKind::Mapping(first) => {
            let mut entries = Vec::new();
            let mut cur = first;
            while let Some(kv_id) = cur {
                let kv = src.node(kv_id).kind.as_keyvalue().expect("a mapping entry");
                let key_id = copy_node(b, src, kv.key)?;
                let val_id = copy_node(b, src, kv.value)?;
                entries.push(Entry { key: key_id, value: val_id });
                cur = src.node(kv_id).next_sibling;
            }
            b.add_mapping(&entries)
        }
        // Reached only via mapping, handled above.
        NodeKind::KeyValue(_) => Err(err!(InvalidGron)),
        NodeKind::Alias(_) => Err(err!(InvalidGron)), // JSON never yields one
    }
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use fig_core::serialize::{serialize_with, Target};

    fn gron_of_with(ast: &AST, proj: &Projection) -> String {
        let mut out = Vec::new();
        print_node(&mut out, ast, ast.root, proj).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn compact_json(ast: &AST) -> String {
        let mut out = Vec::new();
        let t = Target::named("json").unwrap();
        serialize_with(ast, &mut out, &t, &SerializeOptions { pretty: false, ..SerializeOptions::default() }).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn prints_nested_data_as_path_value_lines() {
        let mut b = Builder::init();
        let a_v = b.add_int(1).unwrap();
        let b0 = b.add_string(b"x").unwrap();
        let b1 = b.add_mapping(&[]).unwrap();
        let b_v = b.add_sequence(&[b0, b1]).unwrap();
        let w_v = b.add_bool(true).unwrap();
        let ka = b.add_string(b"a").unwrap();
        let kb = b.add_string(b"b").unwrap();
        let kw = b.add_string(b"weird key").unwrap();
        let root = b
            .add_mapping(&[Entry { key: ka, value: a_v }, Entry { key: kb, value: b_v }, Entry { key: kw, value: w_v }])
            .unwrap();
        let ast = b.finish(root).unwrap();
        assert_eq!(
            gron_of_with(&ast, &Projection::gron()),
            "json = {};\njson.a = 1;\njson.b = [];\njson.b[0] = \"x\";\njson.b[1] = {};\njson[\"weird key\"] = true;\n"
        );
    }

    #[test]
    fn a_custom_projection_swaps_root_separator_and_terminator() {
        let mut b = Builder::init();
        let k = b.add_string(b"a").unwrap();
        let v = b.add_int(1).unwrap();
        let root = b.add_mapping(&[Entry { key: k, value: v }]).unwrap();
        let ast = b.finish(root).unwrap();
        let proj = Projection { root_name: b"cfg".to_vec(), assign: b": ".to_vec(), terminator: Vec::new() };
        assert_eq!(gron_of_with(&ast, &proj), "cfg: {}\ncfg.a: 1\n");
        let doc = parse_document(b"cfg = {};\ncfg.a = 1;").unwrap();
        assert_eq!(compact_json(&doc.ast), "{\"a\":1}\n");
    }

    #[test]
    fn ungron_tolerates_a_non_identifier_root() {
        let doc = parse_document(b"adams-archive = {}\nadams-archive.title = \"hi\"\nadams-archive[\"a-b\"] = 2").unwrap();
        assert_eq!(compact_json(&doc.ast), "{\"title\":\"hi\",\"a-b\":2}\n");
        let scalar = parse_document(b"adams-archive = 7").unwrap();
        assert_eq!(compact_json(&scalar.ast), "7\n");
    }

    #[test]
    fn ungron_reconstructs_the_document() {
        let input = b"json = {};\njson.a = 1;\njson.b = [];\njson.b[0] = \"x\";\njson.b[1] = {};\njson[\"weird key\"] = true;\njson.n = null;";
        let doc = parse_document(input).unwrap();
        assert_eq!(compact_json(&doc.ast), "{\"a\":1,\"b\":[\"x\",{}],\"weird key\":true,\"n\":null}\n");
    }

    #[test]
    fn gron_round_trips_through_parse_and_print() {
        let input = "json = {};\njson.tab = \"a\\tb\";\njson.q = \"she said \\\"hi\\\"\";\njson[\"dotted.key\"] = [];\njson[\"dotted.key\"][0] = -2.5;\njson.flag = false;\njson.nothing = null;";
        let doc = parse_document(input.as_bytes()).unwrap();
        assert_eq!(gron_of_with(&doc.ast, &Projection::gron()), format!("{input}\n"));
    }

    #[test]
    fn top_level_scalar_is_a_bare_assignment() {
        let mut b = Builder::init();
        let root = b.add_string(b"hi").unwrap();
        let ast = b.finish(root).unwrap();
        assert_eq!(gron_of_with(&ast, &Projection::gron()), "json = \"hi\";\n");
    }

    #[test]
    fn malformed_lines_are_rejected() {
        assert_eq!(parse_document(b"no equals here").unwrap_err(), err!(InvalidGron));
        assert_eq!(parse_document(b"json[ = 1").unwrap_err(), err!(InvalidGron));
    }
}
