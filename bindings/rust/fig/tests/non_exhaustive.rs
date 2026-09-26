//! The public types that grow are `#[non_exhaustive]`, so a new format, status,
//! diagnostic, or render knob is a minor release rather than a major one. This
//! file is an EXTERNAL crate, so it sees exactly what a downstream sees — its job
//! is to prove the sweep locked nobody out: every option field is still settable,
//! every value is still constructible, and every returned field is still readable.
//! If a future field/variant has no way to reach it from here, this test is where
//! that shows up.

use fig::{Document, Format, SerializeOptions, Value};

#[test]
fn every_serialize_options_field_is_reachable_without_a_struct_literal() {
    // Three constructors, then chainable setters — one per field.
    let opts = SerializeOptions::default()
        .indent(4)
        .width(120)
        .strip_comments()
        .lossless();
    assert!(opts.pretty && opts.indent == 4 && opts.width == 120);
    assert!(opts.strip_comments && opts.lossless);

    // `pretty: false` comes from the compact constructor, and composes.
    let compact = SerializeOptions::compact().width(1);
    assert!(!compact.pretty && compact.width == 1);
    assert_eq!(SerializeOptions::pretty(8).indent, 8);

    // And they still reach the serializer.
    let v = Value::Map(vec![(Value::Str("k".into()), Value::Str("v".into()))]);
    assert_eq!(
        v.serialize_with(Format::Json, SerializeOptions::compact())
            .unwrap(),
        "{\"k\":\"v\"}\n"
    );
}

#[test]
fn growable_enums_are_matchable_with_a_wildcard_and_still_constructible() {
    // Constructing a variant is unaffected by `non_exhaustive` — only exhaustive
    // matching needs the wildcard, which is the point: a new variant can't break
    // a downstream match.
    let format = Format::Yaml;
    let described = match format {
        Format::Yaml => "yaml",
        Format::Json | Format::Jsonc | Format::Json5 => "json-family",
        _ => "other",
    };
    assert_eq!(described, "yaml");

    let err = Document::parse(b"{ not valid", Format::Json).unwrap_err();
    let msg = match err {
        fig::Error::Parse(ref detail) => detail.message.clone(),
        ref other => format!("{other}"),
    };
    assert!(!msg.is_empty());

    let _embed = fig::EmbedType::FrontmatterYaml;
}

#[test]
fn returned_struct_fields_are_still_readable() {
    // `non_exhaustive` blocks construction, not field access — these are the
    // types the library hands back.
    let v = fig::version();
    assert!(v.major >= 2 || v.minor > 0 || v.patch > 0);
    let caps = fig::capabilities(Format::Yaml);
    assert!(caps.read || caps.edit || caps.serialize);

    let doc = Document::parse(b"a: null\n", Format::Yaml).unwrap();
    let warns = doc
        .diagnose(Format::Toml, SerializeOptions::default())
        .unwrap();
    assert_eq!(warns[0].path, "a");
    let _ = (&warns[0].code, &warns[0].cause, &warns[0].note);

    if let fig::Error::Parse(detail) = Document::parse(b"{ oops", Format::Json).unwrap_err() {
        let _ = (
            detail.message,
            detail.byte_offset,
            detail.line,
            detail.column,
        );
    }
}

#[test]
fn value_stays_exhaustively_matchable_on_purpose() {
    // `Value` is deliberately NOT non_exhaustive: it is the data model, and
    // matching it exhaustively is the primary way callers consume it. This match
    // has no wildcard — if a variant is ever added, this fails to compile, which
    // is the intended signal to think about it rather than a silent break.
    let v = Value::Seq(vec![Value::Null, Value::Bool(true)]);
    let name = match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Uint(_) => "uint",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::Extended { .. } => "extended",
        Value::Seq(_) => "seq",
        Value::Map(_) => "map",
    };
    assert_eq!(name, "seq");
}

#[test]
fn every_render_args_field_is_reachable_without_a_struct_literal() {
    use fig::language::{Literal, RenderArgs};
    let args = RenderArgs::default()
        .dialect("d")
        .indent(b"  ")
        .key(b"k")
        .value(b"42")
        .literal(Literal::Int)
        .old_key(b"o")
        .parent_key(b"deps")
        .parent_tag(b"!dep");
    assert_eq!(args.dialect, "d");
    assert_eq!(
        (args.indent, args.key, args.value),
        (&b"  "[..], &b"k"[..], &b"42"[..])
    );
    assert_eq!(args.literal, Literal::Int);
    assert_eq!(args.old_key, b"o");
    assert_eq!(
        (args.parent_key, args.parent_tag),
        (&b"deps"[..], &b"!dep"[..])
    );
}

#[test]
fn every_description_type_is_buildable_without_a_struct_literal() {
    use fig::language::{
        ClosedContainers, CommentDelimiter, CommentStyle, Comments, Description, Dialect, KeyStyle,
        LanguageError, NativeKinds, PrintOptions, Renderers, SectionHeader, SectionNoun, Splice,
        Syntax,
    };
    // A constructor or `Default`, then plain field assignment: the fields
    // stay public, only the literal is the crate's.
    let mut d = Description::new("kv");
    d.caps = fig::Capabilities::new(true, true, false);
    d.max_mapping_depth = Some(1);
    d.samples = vec!["a=1\n".into()];

    let mut syntax = Syntax::default();
    syntax.kv_sep = Some("=".into());
    syntax.key_style = KeyStyle::BareOrQuoted;
    let mut delimiter = CommentDelimiter::pair("/*", "*/");
    delimiter.forbidden = Some("*/".into());
    syntax.comments = Comments::new(
        CommentStyle::Slashes,
        Some(CommentDelimiter::open("//")),
        Some(delimiter),
    );
    let mut header = SectionHeader::new("[", "]", ".");
    header.seq_open = Some("[[".into());
    header.seq_close = Some("]]".into());
    syntax.section_header = Some(header);
    syntax.section_noun = Some(SectionNoun::Table);
    syntax.closed_containers = Some(ClosedContainers::new("{", "}", "[", "]"));
    d.syntax = Some(syntax);

    let mut dialect = Dialect::new("kv5");
    dialect.extensions = vec!["kv5".into()];
    dialect.splice = Splice::JsonString;
    d.dialects.push(dialect);

    let mut native = NativeKinds::default();
    native.null = true;
    d.lossless = Some(native);
    let mut renderers = Renderers::default();
    renderers.entry = true;
    d.renderers = renderers;

    assert_eq!(d.dialects.len(), 2);
    let header = d.syntax.as_ref().unwrap().section_header.as_ref().unwrap();
    assert_eq!((header.open.as_str(), header.sep.as_str()), ("[", "."));
    assert!(header.skip_index);

    let mut options = PrintOptions::default();
    options.strip_comments = true;
    assert!(options.pretty && options.strip_comments);

    let e = LanguageError::at("expected `=`", 3);
    assert_eq!(
        (e.message.as_str(), e.byte_offset),
        ("expected `=`", Some(3))
    );
}

#[test]
fn every_row_is_buildable_without_a_struct_literal() {
    use fig::language::{
        CommentForm, CommentRow, CommentSlot, DirectiveRow, MentionKind, MentionRow, NodeKind,
        NodeRow, NodeTable, RegionRow,
    };
    let span = fig::Span { start: 0, end: 5 };
    let mut t = NodeTable::new();
    let root = t.push(NodeRow::new(NodeKind::Mapping, None, span));
    t.regions.push(RegionRow::new(root, span));
    t.mentions
        .push(MentionRow::new(root, span, MentionKind::Header));
    t.comments.push(CommentRow::new(
        root,
        CommentSlot::Dangling,
        CommentForm::Block,
        "note",
    ));
    t.directives.push(DirectiveRow::new("!e!", "tag:x/"));
    assert_eq!(t.regions[0].span, span);
    assert_eq!(t.mentions[0].kind, MentionKind::Header);
    assert_eq!(t.comments[0].text, "note");
    assert_eq!(t.directives[0].prefix, "tag:x/");
}

#[test]
fn language_enums_take_a_wildcard_except_comment_slot() {
    use fig::language::{CommentForm, CommentSlot, Literal, NodeKind, Renderer};
    let kind = match NodeKind::Mapping {
        NodeKind::Mapping | NodeKind::Sequence => "container",
        _ => "scalar",
    };
    assert_eq!(kind, "container");
    let spelled = match Literal::Int {
        Literal::Int | Literal::Float => "number",
        _ => "string",
    };
    assert_eq!(spelled, "number");
    assert_eq!(Renderer::Tail.name(), "tail");
    let form = match CommentForm::Line {
        CommentForm::Line => "line",
        _ => "other",
    };
    assert_eq!(form, "line");
    // `CommentSlot` is exhaustive on purpose: no wildcard, and a fourth slot
    // would fail to compile here.
    let slot = match CommentSlot::Trailing {
        CommentSlot::Leading => 0,
        CommentSlot::Trailing => 1,
        CommentSlot::Dangling => 2,
    };
    assert_eq!(slot, 1);
}

#[test]
fn span_converts_to_and_from_a_range_and_hashes() {
    let span = fig::Span::from(2..5);
    assert_eq!(span, fig::Span { start: 2, end: 5 });
    assert_eq!(&"abcdefg"[std::ops::Range::from(span)], "cde");
    let set: std::collections::HashSet<fig::Span> = [span, span].into_iter().collect();
    assert_eq!(set.len(), 1);
}

#[test]
fn error_struct_variants_are_built_by_constructor_and_read_with_dotdot() {
    // Each struct variant is non-exhaustive: built through its constructor,
    // matched with `..`, its fields still readable.
    match fig::Error::missing_field("port", "Config") {
        fig::Error::MissingField { field, ty, .. } => assert_eq!((field, ty), ("port", "Config")),
        other => panic!("{other:?}"),
    }
    match fig::Error::type_mismatch("int", "string") {
        fig::Error::TypeMismatch { expected, .. } => assert_eq!(expected, "int"),
        other => panic!("{other:?}"),
    }
    // A fixed derive message is an ordinary `Message`.
    assert!(matches!(fig::Error::msg_static("x"), fig::Error::Message(ref m) if m == "x"));
}

#[test]
fn a_language_failure_is_readable() {
    // `LanguageFailure` comes back from `register`; nothing outside the crate
    // builds one, but both fields are read.
    fn read(e: &fig::Error) -> Option<(&str, Option<usize>)> {
        match e {
            fig::Error::Language(f) => Some((f.message.as_str(), f.byte_offset)),
            _ => None,
        }
    }
    assert_eq!(read(&fig::Error::NotFound), None);
}
