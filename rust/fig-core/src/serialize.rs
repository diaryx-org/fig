//! Port of src/serialize.zig.
//!
//! Serialization: the `serialize*` entry points, which print a tree as
//! core's canonical form or through a registered language's printer. The
//! options and errors a printer takes are the kit's (`kit/ast/options.rs`).
//!
//! # Public API
//!
//! ```text
//! pub use crate::kit::ast::{SerializeOptions, SERIALIZE_ERRORS};
//! pub enum Target { Canonical, Language(Arc<registry::Entry>) }
//! impl Target {
//!     pub fn named(dialect: &str) -> Option<Target>;                 // in registry::global()
//!     pub fn named_in(registry: &Registry, dialect: &str) -> Option<Target>;
//!     pub fn name(&self) -> &str;
//! }
//! pub fn serialize(ast: &AST, out: &mut Vec<u8>, target: &Target) -> Result<()>;
//! pub fn serialize_with(ast: &AST, out: &mut Vec<u8>, target: &Target, options: &SerializeOptions) -> Result<()>;
//! pub fn serialize_fragment_with(ast: &AST, out: &mut Vec<u8>, target: &Target, options: &SerializeOptions) -> Result<()>;
//! pub fn serialize_node(ast: &AST, out: &mut Vec<u8>, target: &Target, id: NodeId) -> Result<()>;
//! pub fn serialize_node_with(ast: &AST, out: &mut Vec<u8>, target: &Target, id: NodeId, options: &SerializeOptions) -> Result<()>;
//! ```
//!
//! Output is appended to `out`. Canonical output is the whole document with
//! a trailing newline (`serialize_with`), or the bare subtree with none
//! (`serialize_node_with`); a canonical print that fails (`NestingTooDeep`)
//! leaves what it wrote before the failure in `out`, as Zig's streaming
//! writer does. A language's print writes nothing on failure.
//!
//! Errors (the `SerializeError` names): a language's refusal keeps its name
//! when it is one of `NullUnsupported`, `NonStringKey`,
//! `UnrepresentableRoot`, `UnsupportedValue`, `InvalidKey`,
//! `NonFiniteNumber`, `UnresolvedAlias`, `NestingTooDeep`,
//! `UnexpectedNodeKind`; `FormatDisabled` for a language that does not
//! serialize; `WriteFailed` for `OutOfMemory`; and `PrintRefused` for any
//! other (a printer's own error name, `RuntimePrintFailed`, a table error).

use std::sync::Arc;

use crate::canonical::printer as canonical_printer;
use crate::err;
use crate::kit::ast::{NodeId, AST};
use crate::registry::{self, Entry, Registry};
use crate::Result;

pub use crate::kit::ast::{SerializeOptions, SERIALIZE_ERRORS};

/// What a tree is printed as: core's canonical form, or a registered
/// language, whose printer is reached through its entry.
#[derive(Debug, Clone)]
pub enum Target {
    Canonical,
    Language(Arc<Entry>),
}

impl Target {
    /// The registered dialect named `dialect` (in the global registry), or
    /// `None`.
    pub fn named(dialect: &str) -> Option<Target> {
        if dialect == "canonical" {
            return Some(Target::Canonical);
        }
        Self::named_in(registry::global(), dialect)
    }

    /// `named`, in `registry`.
    pub fn named_in(registry: &Registry, dialect: &str) -> Option<Target> {
        if dialect == "canonical" {
            return Some(Target::Canonical);
        }
        Some(Target::Language(registry.entry_by_name(dialect)?))
    }

    /// What the target is called in a message.
    pub fn name(&self) -> &str {
        match self {
            Target::Canonical => "canonical",
            Target::Language(e) => &e.name,
        }
    }
}

/// `ast`, or a comment-stripped copy of it when `options.strip_comments` is
/// set (Zig's `commentView`, a view that blanks the comment lists).
fn comment_view<'a>(ast: &'a AST, options: &SerializeOptions, buf: &'a mut Option<AST>) -> &'a AST {
    if !options.strip_comments {
        return ast;
    }
    let mut c = ast.clone();
    c.strip_comments();
    buf.insert(c)
}

/// Render the whole AST into `out` as `target`, using default options.
pub fn serialize(ast: &AST, out: &mut Vec<u8>, target: &Target) -> Result<()> {
    serialize_with(ast, out, target, &SerializeOptions::default())
}

/// Render the whole AST into `out` as `target`, controlling output style via
/// `options`.
pub fn serialize_with(ast: &AST, out: &mut Vec<u8>, target: &Target, options: &SerializeOptions) -> Result<()> {
    if let Target::Canonical = target {
        let mut buf = None;
        return canonical_printer::print(out, comment_view(ast, options, &mut buf));
    }
    serialize_node_with(ast, out, target, ast.root, options)
}

/// Render the whole AST as a value *fragment*. A language's printer is told
/// a fragment by the options it reads (`splice`, `flow`) and by its root, so
/// this is `serialize_with`; the name says what the caller holds.
pub fn serialize_fragment_with(ast: &AST, out: &mut Vec<u8>, target: &Target, options: &SerializeOptions) -> Result<()> {
    serialize_with(ast, out, target, options)
}

/// Render the subtree rooted at `id`, using default options.
pub fn serialize_node(ast: &AST, out: &mut Vec<u8>, target: &Target, id: NodeId) -> Result<()> {
    serialize_node_with(ast, out, target, id, &SerializeOptions::default())
}

/// Render the subtree rooted at `id`, controlling output style via `options`.
pub fn serialize_node_with(ast: &AST, out: &mut Vec<u8>, target: &Target, id: NodeId, options: &SerializeOptions) -> Result<()> {
    let mut buf = None;
    let view = comment_view(ast, options, &mut buf);
    match target {
        Target::Canonical => canonical_printer::print_node(out, view, id, 0),
        Target::Language(e) => registry::print_node_with(e, view, id, options, out).map_err(|e| match e.name() {
            // A writer reports its own allocation failure as `WriteFailed`.
            "OutOfMemory" | "WriteFailed" => err!(WriteFailed),
            "NullUnsupported" | "NonStringKey" | "UnrepresentableRoot" | "UnsupportedValue" | "InvalidKey"
            | "NonFiniteNumber" | "UnresolvedAlias" | "NestingTooDeep" | "UnexpectedNodeKind" | "FormatDisabled" => e,
            _ => err!(PrintRefused),
        }),
    }
}

// ── trees built by hand, printed through the registry ──────────────────────
//
// These need the bundled JSON, JSON5 and YAML; each returns early while the
// global registry has no such dialect (a test build's global registry holds
// fig's ten, from the language crates).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::{Builder, Comment, Entry as MapEntry, KindTag, NodeComments, Style, Tag};
    use std::borrow::Cow;

    fn find(s: &[u8], needle: &str) -> bool {
        s.windows(needle.len()).any(|w| w == needle.as_bytes())
    }

    #[test]
    fn builder_constructs_an_ast_that_serializes() {
        let Some(json) = Target::named("json") else { return };
        let mut b = Builder::init();
        // Build { "name": "fig", "nums": [1, 2] } bottom-up.
        let v_name = b.add_string(b"fig").unwrap();
        let n1 = b.add_int(1).unwrap();
        let n2 = b.add_int(2).unwrap();
        let v_nums = b.add_sequence(&[n1, n2]).unwrap();
        let k_name = b.add_string(b"name").unwrap();
        let k_nums = b.add_string(b"nums").unwrap();
        let root = b
            .add_mapping(&[MapEntry { key: k_name, value: v_name }, MapEntry { key: k_nums, value: v_nums }])
            .unwrap();
        let ast = b.finish(root).unwrap();

        let mut out = Vec::new();
        serialize(&ast, &mut out, &json).unwrap();
        assert_eq!(
            "{\n  \"name\": \"fig\",\n  \"nums\": [\n    1,\n    2\n  ]\n}\n",
            String::from_utf8_lossy(&out)
        );

        // Same AST, YAML — exercises the empty side-table (anchor/tag) guard path.
        if let Some(yaml) = Target::named("yaml") {
            let mut y = Vec::new();
            serialize(&ast, &mut y, &yaml).unwrap();
            assert_eq!("name: fig\nnums: [1, 2]\n", String::from_utf8_lossy(&y));
        }
    }

    #[test]
    fn builder_set_tag_surfaces_a_normalized_kind_tag_as_a_yaml_core_tag() {
        let Some(yaml) = Target::named("yaml") else { return };
        let mut b = Builder::init();
        let v = b.add_number_raw(b"09", false).unwrap(); // integer lexeme kept verbatim
        b.set_tag(v, Tag::Kind(KindTag::Integer)).unwrap();
        let k = b.add_string(b"id").unwrap();
        let root = b.add_mapping(&[MapEntry { key: k, value: v }]).unwrap();
        let ast = b.finish(root).unwrap();
        assert_eq!(Some(Tag::Kind(KindTag::Integer)), ast.tag_of(v));

        let mut out = Vec::new();
        serialize(&ast, &mut out, &yaml).unwrap();
        // A `.kind` tag renders as its YAML core-schema shorthand; the lexeme is kept.
        assert!(find(&out, "!!int"));
        assert!(find(&out, "09"));
    }

    #[test]
    fn strip_comments_drops_carried_comments_across_formats() {
        let Some(json5) = Target::named("json5") else { return };
        let mut b = Builder::init();
        let v = b.add_string(b"fig").unwrap();
        let k = b.add_string(b"name").unwrap();
        b.set_comments(k, &NodeComments { leading: Cow::Owned(vec![Comment { text: b"hi", style: Style::Line }]), ..Default::default() })
            .unwrap();
        let root = b.add_mapping(&[MapEntry { key: k, value: v }]).unwrap();
        let ast = b.finish(root).unwrap();

        // Default: comment preserved (JSON5).
        let mut kept = Vec::new();
        serialize_with(&ast, &mut kept, &json5, &SerializeOptions::default()).unwrap();
        assert!(find(&kept, "// hi"));

        // Stripped: same AST, no comment, in JSON5 and YAML.
        let strip = SerializeOptions { strip_comments: true, ..Default::default() };
        let mut stripped = Vec::new();
        serialize_with(&ast, &mut stripped, &json5, &strip).unwrap();
        assert!(!find(&stripped, "hi"));

        if let Some(yaml) = Target::named("yaml") {
            let mut y = Vec::new();
            serialize_with(&ast, &mut y, &yaml, &strip).unwrap();
            assert_eq!("name: fig\n", String::from_utf8_lossy(&y));
        }
    }

    #[test]
    fn builder_incremental_comment_helpers_append_and_serialize() {
        let mut b = Builder::init();
        let v = b.add_number_raw(b"1", false).unwrap();
        let k = b.add_string(b"a").unwrap();
        // Two leading comments accumulate in call order; a trailing rides the value.
        b.add_leading_comment(k, Comment::line(b"first")).unwrap();
        b.add_leading_comment(k, Comment::line(b"second")).unwrap();
        b.set_trailing_comment(v, Comment::line(b"tail")).unwrap();
        let root = b.add_mapping(&[MapEntry { key: k, value: v }]).unwrap();
        // A dangling comment sits at the end of the mapping body.
        b.add_dangling_comment(root, Comment::line(b"end")).unwrap();
        let ast = b.finish(root).unwrap();

        // The leading run kept insertion order and the trailing/dangling bound too.
        let kc = ast.comments(k);
        assert_eq!(2, kc.leading.len());
        assert_eq!(b"first", kc.leading[0].text);
        assert_eq!(b"second", kc.leading[1].text);
        assert_eq!(b"tail", ast.comments(v).trailing.unwrap().text);
        assert_eq!(b"end", ast.comments(root).dangling[0].text);

        // A comment-bearing format renders them; this also proves the runs are valid.
        let Some(json5) = Target::named("json5") else { return };
        let mut out = Vec::new();
        serialize_with(&ast, &mut out, &json5, &SerializeOptions::default()).unwrap();
        for c in ["// first", "// second", "// tail", "// end"] {
            assert!(find(&out, c));
        }
    }

    #[test]
    fn canonical_is_a_target_by_name() {
        let t = Target::named("canonical").unwrap();
        assert_eq!("canonical", t.name());
        let ast = crate::canonical::parse_abstract(b"{\"a\": [1, null]}").unwrap();
        let mut out = Vec::new();
        serialize_node(&ast, &mut out, &t, ast.root).unwrap();
        assert_eq!("{\n  \"a\": [\n    1,\n    null\n  ]\n}", String::from_utf8_lossy(&out));
        out.clear();
        serialize(&ast, &mut out, &t).unwrap();
        assert_eq!("{\n  \"a\": [\n    1,\n    null\n  ]\n}\n", String::from_utf8_lossy(&out));
    }
}
