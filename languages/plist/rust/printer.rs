//! Port of languages/plist/src/printer.zig.
//!
//! plist printer: renders a fig AST as an XML property list — the documented
//! inverse of the reader (`parser.rs`). Read that file's header first; this
//! one mirrors its data model exactly, in reverse:
//!
//!   * `mapping` → `<dict>` of alternating `<key>`/value pairs (a mapping key
//!     must be a string — `NonStringKey` otherwise).
//!   * `sequence` → `<array>` of the same items, in order.
//!   * `string`/`number`(integer|float)/`boolean` → `<string>`/`<integer>`/
//!     `<real>`/`<true/>`/`<false/>`.
//!   * `extended` → `<date>`/`<data>` for its date and a byte string
//!     (`ExtKind::PlistDate`/`Data`, whatever wrote it); any OTHER extended
//!     kind reaching here from a foreign AST (a TOML datetime, a ZON
//!     enum/char literal, a JSON5 non-finite float) has no dedicated plist
//!     primitive and degrades to a plain `<string>` of its text — the same
//!     blanket treatment XML's printer gives every scalar type it has no
//!     primitive for.
//!   * `null` has no plist type at all — `NullUnsupported` (same convention
//!     as TOML).
//!   * A YAML `*alias` reaching here is `UnresolvedAlias` (materialize
//!     first).
//!
//! The whole document is wrapped in the standard `<?xml ...?>` declaration,
//! `<!DOCTYPE plist ...>`, and a `<plist version="1.0">` root element —
//! unlike the reader (which also accepts a bare, unwrapped root object), the
//! writer always emits the full canonical wrapper.
//!
//! Errors: `UnresolvedAlias`, `NullUnsupported`, `NonStringKey`.

use fig_kit::err;
use fig_kit::ast::{ExtKind, NodeId, NodeKind, NumberKind, SerializeOptions, AST};
use fig_kit::Result;

struct Printer<'a> {
    out: &'a mut Vec<u8>,
    ast: &'a AST,
    options: SerializeOptions,
}

const HEADER: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">";

/// Prints the whole document: the standard plist wrapper around `ast.root`.
pub fn print(out: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    let mut p = Printer { out, ast, options: *options };
    p.out.extend_from_slice(HEADER);
    p.nl();
    p.value(ast.root, 1)?;
    p.nl();
    p.out.extend_from_slice(b"</plist>\n");
    Ok(())
}

/// Prints `ast.root` as splice text — the bare plistObject with no wrapper,
/// which is what the editor splices (`render_value` takes a leading `<` as an
/// element already spelled). See `SerializeOptions::splice`.
pub fn print_splice(out: &mut Vec<u8>, ast: &AST, options: &SerializeOptions) -> Result<()> {
    print_node(out, ast, ast.root, 0, options)
}

/// Prints the subtree rooted at `id` as a bare plistObject — no `<plist>`
/// wrapper, no trailing newline (used for partial/`--path` renders,
/// mirroring every other printer's `print_node`). `depth` 0 is the top
/// level, as it is for every other printer; `value` counts a container's
/// CHILDREN's depth, one more than its own, which is the level `print` gives
/// the root.
pub fn print_node(out: &mut Vec<u8>, ast: &AST, id: NodeId, depth: usize, options: &SerializeOptions) -> Result<()> {
    let mut p = Printer { out, ast, options: *options };
    p.value(id, depth + 1)
}

impl Printer<'_> {
    fn value(&mut self, id: NodeId, depth: usize) -> Result<()> {
        let ast = self.ast;
        match ast.node(id).kind {
            NodeKind::Null => return Err(err!(NullUnsupported)),
            NodeKind::Boolean(b) => self.out.extend_from_slice(if b { b"<true/>" } else { b"<false/>" }),
            NodeKind::Number(num) => match num.kind {
                NumberKind::Integer => {
                    self.out.extend_from_slice(b"<integer>");
                    self.out.extend_from_slice(num.raw);
                    self.out.extend_from_slice(b"</integer>");
                }
                NumberKind::Float => {
                    self.out.extend_from_slice(b"<real>");
                    self.out.extend_from_slice(num.raw);
                    self.out.extend_from_slice(b"</real>");
                }
            },
            NodeKind::String(s) => self.text_element(b"string", s),
            NodeKind::Extended(e) => match e.kind {
                ExtKind::PlistDate => self.text_element(b"date", e.text),
                ExtKind::Data => self.text_element(b"data", e.text),
                _ => self.text_element(b"string", e.text),
            },
            NodeKind::Sequence(first) => self.array(first, depth)?,
            NodeKind::Mapping(first) => self.dict(first, depth)?,
            NodeKind::KeyValue(_) => unreachable!("a keyvalue is never a value position"),
            NodeKind::Alias(_) => return Err(err!(UnresolvedAlias)),
        }
        Ok(())
    }

    fn text_element(&mut self, tag: &[u8], text: &[u8]) {
        self.out.push(b'<');
        self.out.extend_from_slice(tag);
        self.out.push(b'>');
        write_escaped(self.out, text);
        self.out.extend_from_slice(b"</");
        self.out.extend_from_slice(tag);
        self.out.push(b'>');
    }

    fn dict(&mut self, first_entry: Option<NodeId>, depth: usize) -> Result<()> {
        if first_entry.is_none() {
            self.out.extend_from_slice(b"<dict/>");
            return Ok(());
        }
        self.out.extend_from_slice(b"<dict>");
        self.nl();
        let ast = self.ast;
        let mut cur = first_entry;
        while let Some(eid) = cur {
            let kv = ast.node(eid).kind.as_keyvalue().expect("a mapping's child is an entry");
            let key = match ast.node(kv.key).kind {
                NodeKind::String(s) => s,
                _ => return Err(err!(NonStringKey)),
            };
            self.indent(depth);
            self.text_element(b"key", key);
            self.nl();
            self.indent(depth);
            self.value(kv.value, depth + 1)?;
            self.nl();
            cur = ast.node(eid).next_sibling;
        }
        self.indent(depth - 1);
        self.out.extend_from_slice(b"</dict>");
        Ok(())
    }

    fn array(&mut self, first_item: Option<NodeId>, depth: usize) -> Result<()> {
        if first_item.is_none() {
            self.out.extend_from_slice(b"<array/>");
            return Ok(());
        }
        self.out.extend_from_slice(b"<array>");
        self.nl();
        let ast = self.ast;
        let mut cur = first_item;
        while let Some(iid) = cur {
            self.indent(depth);
            self.value(iid, depth + 1)?;
            self.nl();
            cur = ast.node(iid).next_sibling;
        }
        self.indent(depth - 1);
        self.out.extend_from_slice(b"</array>");
        Ok(())
    }

    fn nl(&mut self) {
        if self.options.pretty {
            self.out.push(b'\n');
        }
    }

    fn indent(&mut self, depth: usize) {
        if !self.options.pretty {
            return;
        }
        let n = depth * self.options.indent as usize;
        self.out.resize(self.out.len() + n, b' ');
    }
}

/// Escape `&`/`<`/`>` while copying `s` to `out` (plist text content is
/// never inside an attribute value in this printer's own output, so `"`
/// never needs escaping here).
fn write_escaped(out: &mut Vec<u8>, s: &[u8]) {
    for &c in s {
        match c {
            b'&' => out.extend_from_slice(b"&amp;"),
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            _ => out.push(c),
        }
    }
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::parser;
    use super::super::Type;
    use super::*;
    use fig_kit::ast::{Extended, KeyValue, Node, Number};
    use fig_kit::document::Document;

    /// `src` as fig adopts it from the plist parser.
    fn parse_doc(src: &str) -> Document {
        parser::parse(src.as_bytes(), Type::Xml).unwrap()
    }

    fn expect_print(src: &str, expected: &str) {
        let doc = parse_doc(src);
        let mut out = Vec::new();
        print(&mut out, &doc.ast, &SerializeOptions::default()).unwrap();
        assert_eq!(expected, String::from_utf8(out).unwrap());
    }

    /// Parse `src`, print it, reparse the printed bytes, and assert the two
    /// ASTs are equal — the round-trip property this design exists for.
    fn expect_round_trip(src: &str) {
        let doc = parse_doc(src);
        let mut out = Vec::new();
        print(&mut out, &doc.ast, &SerializeOptions::default()).unwrap();
        let reparsed = parser::parse(&out, Type::Xml).unwrap();
        assert!(doc.ast.eql(&reparsed.ast));
    }

    const WRAPPER_OPEN: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n";
    const WRAPPER_CLOSE: &str = "\n</plist>\n";

    #[test]
    fn empty_dict() {
        expect_print("<dict/>", &format!("{WRAPPER_OPEN}<dict/>{WRAPPER_CLOSE}"));
    }

    #[test]
    fn scalars() {
        expect_print(
            "<dict><key>s</key><string>hi</string><key>i</key><integer>-42</integer><key>r</key><real>3.14</real><key>t</key><true/><key>f</key><false/></dict>",
            &format!(
                "{WRAPPER_OPEN}<dict>\n  <key>s</key>\n  <string>hi</string>\n  <key>i</key>\n  <integer>-42</integer>\n  <key>r</key>\n  <real>3.14</real>\n  <key>t</key>\n  <true/>\n  <key>f</key>\n  <false/>\n</dict>{WRAPPER_CLOSE}"
            ),
        );
    }

    #[test]
    fn date_and_data() {
        expect_print(
            "<dict><key>d</key><date>2011-11-01T12:00:00Z</date><key>b</key><data>SGVsbG8=</data></dict>",
            &format!(
                "{WRAPPER_OPEN}<dict>\n  <key>d</key>\n  <date>2011-11-01T12:00:00Z</date>\n  <key>b</key>\n  <data>SGVsbG8=</data>\n</dict>{WRAPPER_CLOSE}"
            ),
        );
    }

    #[test]
    fn nested_array_and_dict() {
        expect_print(
            "<array><string>one</string><dict><key>k</key><string>v</string></dict></array>",
            &format!(
                "{WRAPPER_OPEN}<array>\n  <string>one</string>\n  <dict>\n    <key>k</key>\n    <string>v</string>\n  </dict>\n</array>{WRAPPER_CLOSE}"
            ),
        );
    }

    #[test]
    fn escapes_amp_lt_gt_in_text() {
        expect_print("<string>x &amp; y &lt;z&gt;</string>", &format!("{WRAPPER_OPEN}<string>x &amp; y &lt;z&gt;</string>{WRAPPER_CLOSE}"));
    }

    #[test]
    fn compact_non_pretty_output_has_no_inserted_whitespace() {
        let doc = parse_doc("<dict><key>a</key><integer>1</integer></dict>");
        let mut out = Vec::new();
        print(&mut out, &doc.ast, &SerializeOptions { pretty: false, ..Default::default() }).unwrap();
        assert_eq!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>a</key><integer>1</integer></dict></plist>\n",
            String::from_utf8(out).unwrap()
        );
    }

    #[test]
    fn round_trip_through_parse_print_parse_for_every_reader_fixture_shape() {
        for src in [
            "<dict/>",
            "<array/>",
            "<string>hi</string>",
            "<integer>-42</integer>",
            "<real>3.14</real>",
            "<true/>",
            "<false/>",
            "<date>2011-11-01T12:00:00Z</date>",
            "<data>SGVsbG8=</data>",
            "<dict><key>a</key><string>1</string><key>b</key><array><integer>1</integer><integer>2</integer></array></dict>",
        ] {
            expect_round_trip(src);
        }
    }

    fn print_err(nodes: &[Node<'_>]) -> fig_kit::Error {
        let ast = AST::from_nodes(0, nodes).unwrap();
        print(&mut Vec::new(), &ast, &SerializeOptions::default()).unwrap_err()
    }

    #[test]
    fn error_null_is_unsupported() {
        assert_eq!(err!(NullUnsupported), print_err(&[Node { id: 0, kind: NodeKind::Null, next_sibling: None }]));
    }

    #[test]
    fn error_non_string_key() {
        let nodes = [
            Node { id: 0, kind: NodeKind::Mapping(Some(1)), next_sibling: None },
            Node { id: 1, kind: NodeKind::KeyValue(KeyValue { key: 2, value: 3 }), next_sibling: None },
            Node { id: 2, kind: NodeKind::Number(Number { raw: b"1", kind: NumberKind::Integer }), next_sibling: None },
            Node { id: 3, kind: NodeKind::String(b"x"), next_sibling: None },
        ];
        assert_eq!(err!(NonStringKey), print_err(&nodes));
    }

    #[test]
    fn error_unresolved_alias() {
        assert_eq!(err!(UnresolvedAlias), print_err(&[Node { id: 0, kind: NodeKind::Alias(b"anchor"), next_sibling: None }]));
    }

    #[test]
    fn foreign_extended_kind_degrades_to_string() {
        let nodes =
            [Node { id: 0, kind: NodeKind::Extended(Extended { kind: ExtKind::EnumLiteral, text: b"fast" }), next_sibling: None }];
        let ast = AST::from_nodes(0, &nodes).unwrap();
        let mut out = Vec::new();
        print(&mut out, &ast, &SerializeOptions::default()).unwrap();
        assert!(out.windows(21).any(|w| w == b"<string>fast</string>"));
    }

    #[test]
    fn splice_text_is_the_bare_element_and_a_containers_lines_sit_at_the_top_level() {
        let doc = parse_doc("<dict><key>q</key><array><integer>1</integer></array></dict>");
        let mut out = Vec::new();
        print_splice(&mut out, &doc.ast, &SerializeOptions::default()).unwrap();
        assert_eq!("<dict>\n  <key>q</key>\n  <array>\n    <integer>1</integer>\n  </array>\n</dict>", String::from_utf8(out).unwrap());
        let mut out = Vec::new();
        print_node(&mut out, &doc.ast, doc.ast.root, 0, &SerializeOptions::default()).unwrap();
        assert_eq!("<dict>\n  <key>q</key>\n  <array>\n    <integer>1</integer>\n  </array>\n</dict>", String::from_utf8(out).unwrap());
    }
}
