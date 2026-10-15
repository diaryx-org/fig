//! Port of kit/src/describe_json.zig.
//!
//! A language's description as the helper wire carries it
//! (docs/helper-protocol.md, "The description"): the JSON a helper answers
//! `describe` with. Zig writes it from the C vtable records
//! `kit/contract.zig` builds from a language module; here it is written from
//! the `Language` trait directly, through the same records: the dialect rows
//! (each declared dialect, then each `--spec` version of one, named
//! `<dialect>-<version>`), their extensions, the syntax of each row when the
//! language is editable, and the renderers it declares. The inverse of
//! fig's `src/contract/wire.zig`'s `vtableFromDescription`, field for field.
//!
//! The C records carry NUL-terminated strings, so a declared string is
//! written up to its first NUL byte, as the vtable reads it back; samples
//! carry a length and are written whole.

use super::ast::ExtKind;
use super::language::Language;
use super::manifest::{CommentDelimiter, Dialect, NativeKinds, Syntax};

/// A dialect row, as `kit/contract.zig`'s `VTable` lists them.
struct Row {
    name: Vec<u8>,
    /// The declared row it is, or is a version of.
    of: usize,
    dialect: Dialect,
    declared: bool,
}

/// Write `l`'s description.
pub fn write(w: &mut Vec<u8>, l: &dyn Language) {
    let dialects = l.dialects();
    let caps = l.caps();
    let editable = caps.edit;
    let mut rows: Vec<Row> = Vec::new();
    for (i, d) in dialects.iter().enumerate() {
        rows.push(Row { name: d.name.as_bytes().to_vec(), of: i, dialect: d.dialect, declared: true });
    }
    for (i, d) in dialects.iter().enumerate() {
        for sp in d.specs {
            let mut name = d.name.as_bytes().to_vec();
            name.push(b'-');
            name.extend_from_slice(sp.name.as_bytes());
            rows.push(Row { name, of: i, dialect: sp.dialect, declared: false });
        }
    }
    let syntaxes: Vec<Option<Syntax>> = rows.iter().map(|r| if editable { Some(l.syntax(r.dialect)) } else { None }).collect();

    w.extend_from_slice(b"{\"name\":");
    str(w, z(dialects.first().map_or(&b""[..], |d| d.name.as_bytes())));
    w.extend_from_slice(
        format!(
            ",\"caps\":{{\"read\":{},\"edit\":{},\"serialize\":{},\"references\":{}}}",
            caps.read, caps.edit, caps.serialize, caps.references
        )
        .as_bytes(),
    );
    w.extend_from_slice(b",\"max_mapping_depth\":");
    match caps.max_mapping_depth {
        Some(m) => w.extend_from_slice(m.to_string().as_bytes()),
        None => w.extend_from_slice(b"null"),
    }
    w.extend_from_slice(b",\"lossless\":");
    match &caps.lossless {
        Some(k) => kinds(w, k),
        None => w.extend_from_slice(b"null"),
    }
    w.extend_from_slice(b",\"syntax\":");
    match syntaxes.first().copied().flatten() {
        Some(s) => syntax(w, &s),
        None => w.extend_from_slice(b"null"),
    }

    w.extend_from_slice(b",\"dialects\":[");
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        dialect(w, l, r, syntaxes[i].as_ref());
    }
    w.extend_from_slice(b"],\"samples\":[");
    for (i, s) in l.samples().iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        str(w, s);
    }
    w.extend_from_slice(b"],\"renderers\":[");
    let mut first = true;
    for which in super::manifest::Renderer::ALL {
        if l.has_renderer(l.default_dialect(), which) {
            if !first {
                w.push(b',');
            }
            first = false;
            w.push(b'"');
            w.extend_from_slice(which.name().as_bytes());
            w.push(b'"');
        }
    }
    w.extend_from_slice(format!("],\"split_stream\":{}}}", l.has_split_stream()).as_bytes());
}

fn dialect(w: &mut Vec<u8>, l: &dyn Language, r: &Row, s: Option<&Syntax>) {
    let d = &l.dialects()[r.of];
    w.extend_from_slice(b"{\"name\":");
    str(w, z(&r.name));
    w.extend_from_slice(b",\"extensions\":[");
    if r.declared {
        // Every extension of the language goes to the dialect named after
        // it, else to the first.
        let mut i = 0usize;
        for e in l.extensions() {
            let mut owner = 0usize;
            for (j, dd) in l.dialects().iter().enumerate() {
                if dd.name == *e {
                    owner = j;
                }
            }
            if owner == r.of {
                if i > 0 {
                    w.push(b',');
                }
                str(w, z(e.as_bytes()));
                i += 1;
            }
        }
    }
    let splice = match d.splice.ordinal() {
        1 => "escaped",
        2 => "raw",
        _ => "literal",
    };
    w.extend_from_slice(format!("],\"splice\":\"{splice}\"").as_bytes());
    w.extend_from_slice(b",\"empty_doc_seed\":");
    opt_str(w, d.empty_doc_seed);
    w.extend_from_slice(b",\"syntax\":");
    match s {
        Some(s) => syntax(w, s),
        None => w.extend_from_slice(b"null"),
    }
    w.extend_from_slice(b",\"prints\":");
    match &d.prints {
        None => w.extend_from_slice(b"null"),
        Some(p) => {
            w.extend_from_slice(b"{\"holds\":");
            kinds(w, &p.holds);
            w.extend_from_slice(
                format!(
                    ",\"typed_scalars\":{},\"null_at_root\":{},\"root_container\":{},\"char_as\":",
                    p.typed_scalars, p.null_at_root, p.root_container
                )
                .as_bytes(),
            );
            str(w, z(p.char_as.as_bytes()));
            w.extend_from_slice(b",\"number_special_as\":");
            str(w, z(p.number_special_as.as_bytes()));
            let comments = match p.comments.ordinal() {
                1 => "pretty",
                2 => "never",
                _ => "always",
            };
            w.extend_from_slice(format!(",\"comments\":\"{comments}\"}}").as_bytes());
        }
    }
    w.push(b'}');
}

/// `NativeKinds`: `null`, then one field per `ExtKind`, in its order.
fn kinds(w: &mut Vec<u8>, k: &NativeKinds) {
    w.extend_from_slice(format!("{{\"null\":{}", k.null).as_bytes());
    for e in ExtKind::ALL {
        w.extend_from_slice(format!(",\"{}\":{}", e.name(), k.ext(e)).as_bytes());
    }
    w.push(b'}');
}

fn syntax(w: &mut Vec<u8>, s: &Syntax) {
    w.extend_from_slice(b"{\"comments\":{\"line\":");
    delimiter(w, s.comments.line.as_ref());
    w.extend_from_slice(b",\"trailing\":");
    delimiter(w, s.comments.trailing.as_ref());
    w.extend_from_slice(b",\"block\":");
    delimiter(w, s.comments.block.as_ref());
    w.push(b'}');
    let strs: [(&str, Option<&[u8]>); 14] = [
        ("kv_sep", s.kv_sep),
        ("flow_map_pad", Some(s.flow_map_pad)),
        ("empty_map_literal", s.empty_map_literal),
        ("indent_unit", Some(s.indent_unit)),
        ("seq_item_marker", Some(s.seq_item_marker)),
        ("flow_map_open", Some(s.flow_map_open)),
        ("flow_map_close", Some(s.flow_map_close)),
        ("section_noun", s.section_noun),
        ("merge_key", s.merge_key),
        ("flow_entry_sep", Some(s.flow_entry_sep)),
        ("line_prefix_marks", Some(s.line_prefix_marks)),
        ("flow_map_kv_sep", Some(s.flow_map_kv_sep)),
        ("flow_map_entry_sep", Some(s.flow_map_entry_sep)),
        ("empty_seq_literal", s.empty_seq_literal),
    ];
    for (name, v) in strs {
        w.extend_from_slice(format!(",\"{name}\":").as_bytes());
        opt_str(w, v);
    }
    let bools: [(&str, bool); 9] = [
        ("flow_kv_sep_from_siblings", s.flow_kv_sep_from_siblings),
        ("block_seq_editable", s.block_seq_editable),
        ("flow_containers", s.flow_containers),
        ("single_line_block_mapping", s.single_line_block_mapping),
        ("bare_document_mapping", s.bare_document_mapping),
        ("structural_indent", s.structural_indent),
        ("flow_root", s.flow_root),
        ("flow_maps_only", s.flow_maps_only),
        ("opener_line_comment", s.opener_line_comment),
    ];
    for (name, v) in bools {
        w.extend_from_slice(format!(",\"{name}\":{v}").as_bytes());
    }
    w.extend_from_slice(format!(",\"key_sigil\":{}", s.key_sigil.unwrap_or(0)).as_bytes());
    w.extend_from_slice(format!(",\"line_continuation\":{}", s.line_continuation.unwrap_or(0)).as_bytes());
    if let Some(c) = &s.closed_containers {
        w.extend_from_slice(b",\"closed_containers\":{");
        let fields: [(&str, &[u8]); 4] =
            [("map_open", c.map.open), ("map_close", c.map.close), ("seq_open", c.seq.open), ("seq_close", c.seq.close)];
        for (i, (name, v)) in fields.into_iter().enumerate() {
            if i > 0 {
                w.push(b',');
            }
            w.extend_from_slice(format!("\"{name}\":").as_bytes());
            opt_str(w, Some(v));
        }
        w.push(b'}');
    }
    if let Some(h) = &s.section_header {
        w.extend_from_slice(b",\"section_header\":{");
        let fields: [(&str, Option<&[u8]>); 5] = [
            ("open", Some(h.open)),
            ("close", Some(h.close)),
            ("seq_open", h.seq_open),
            ("seq_close", h.seq_close),
            ("sep", Some(h.sep)),
        ];
        for (i, (name, v)) in fields.into_iter().enumerate() {
            if i > 0 {
                w.push(b',');
            }
            w.extend_from_slice(format!("\"{name}\":").as_bytes());
            opt_str(w, v);
        }
        w.extend_from_slice(format!(",\"skip_index\":{}}}", h.skip_index).as_bytes());
    }
    w.push(b'}');
}

fn delimiter(w: &mut Vec<u8>, d: Option<&CommentDelimiter>) {
    let Some(d) = d else {
        w.extend_from_slice(b"null");
        return;
    };
    w.extend_from_slice(b"{\"open\":");
    opt_str(w, Some(d.open));
    w.extend_from_slice(b",\"close\":");
    opt_str(w, if d.close.is_empty() { None } else { Some(d.close) });
    w.extend_from_slice(b",\"forbidden\":");
    opt_str(w, d.forbidden);
    w.push(b'}');
}

/// A declared string as the C record holds it: up to its first NUL.
fn z(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

fn opt_str(w: &mut Vec<u8>, s: Option<&[u8]>) {
    match s {
        Some(x) => str(w, z(x)),
        None => w.extend_from_slice(b"null"),
    }
}

/// `std.json`'s string encoding: quoted, `"` and `\` escaped, the C0
/// controls as `\b \f \n \r \t` or `\u00xx`, every other byte as itself.
fn str(w: &mut Vec<u8>, s: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    w.push(b'"');
    for &c in s {
        match c {
            b'"' => w.extend_from_slice(b"\\\""),
            b'\\' => w.extend_from_slice(b"\\\\"),
            0x08 => w.extend_from_slice(b"\\b"),
            0x0c => w.extend_from_slice(b"\\f"),
            b'\n' => w.extend_from_slice(b"\\n"),
            b'\r' => w.extend_from_slice(b"\\r"),
            b'\t' => w.extend_from_slice(b"\\t"),
            0x00..=0x1f => {
                w.extend_from_slice(b"\\u00");
                w.push(HEX[(c >> 4) as usize]);
                w.push(HEX[(c & 0x0f) as usize]);
            }
            _ => w.push(c),
        }
    }
    w.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::language::{Caps, Comments, DialectInfo, SpecName, SpliceStyle};
    use crate::kit::table::Table;
    use crate::Result;

    struct Spec;

    const DIALECTS: &[DialectInfo] = &[DialectInfo {
        specs: &[SpecName { name: "1.1", dialect: 1 }],
        ..DialectInfo::new("spec", SpliceStyle::Literal, None)
    }];

    impl Language for Spec {
        fn name(&self) -> &str {
            "spec"
        }
        fn extensions(&self) -> &[&'static str] {
            &["sp"]
        }
        fn caps(&self) -> Caps {
            Caps { edit: true, ..Caps::DEFAULT }
        }
        fn dialects(&self) -> &[DialectInfo] {
            DIALECTS
        }
        fn samples(&self) -> &[&'static [u8]] {
            &[b"a\n"]
        }
        fn syntax(&self, _: Dialect) -> Syntax {
            Syntax::new(Comments::HASH, Some(b": "), Some(b"{}"))
        }
        fn parse_table(&self, _: &[u8], _: Dialect, _: &mut Table) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_description_lists_rows_versions_and_syntax() {
        let mut out = Vec::new();
        write(&mut out, &Spec);
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("{\"name\":\"spec\",\"caps\":{\"read\":true,\"edit\":true,\"serialize\":false,\"references\":false},\"max_mapping_depth\":null,\"lossless\":null,\"syntax\":{\"comments\":{\"line\":{\"open\":\"#\",\"close\":null,\"forbidden\":null}"));
        assert!(s.contains("{\"name\":\"spec\",\"extensions\":[\"sp\"],\"splice\":\"literal\",\"empty_doc_seed\":null,"));
        assert!(s.contains("{\"name\":\"spec-1.1\",\"extensions\":[],"));
        assert!(s.ends_with("\"samples\":[\"a\\n\"],\"renderers\":[],\"split_stream\":false}"));
    }
}
