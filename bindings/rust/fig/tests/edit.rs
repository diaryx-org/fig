//! Comment-preserving write-path tests: edits must change only the targeted
//! node's bytes and leave comments, key order and formatting intact. The edits are the `*_value` methods, which need no serde; the file
//! is serde-gated because it reads results back with `from_yaml_str` and builds a
//! typed struct's value with `fig::to_value`. Run with
//! `cargo test -p fig --features serde`.
#![cfg(all(feature = "serde", feature = "yaml"))]

use fig::{Editor, Segment};

#[test]
fn editor_insert_appends_after_last_entry() {
    let mut ed = Editor::open(b"a: 1\nb: 2\n", by_name("yaml")).unwrap();
    ed.insert_value(&[], "c", 3).unwrap();
    assert_eq!(ed.source().unwrap(), "a: 1\nb: 2\nc: 3\n");
}

#[test]
fn editor_set_replaces_or_inserts() {
    let mut ed = Editor::open(b"a: 1\nb: 2\n", by_name("yaml")).unwrap();
    // Existing key → replace in place.
    ed.set_value(&[Segment::Key("a")], 9).unwrap();
    // Absent key → insert at the end.
    ed.set_value(&[Segment::Key("c")], 3).unwrap();
    assert_eq!(ed.source().unwrap(), "a: 9\nb: 2\nc: 3\n");
}

#[test]
fn editor_replace_quotes_when_needed() {
    let mut ed = Editor::open(b"title: Hello\n", by_name("yaml")).unwrap();
    // A `Serialize` value goes in through `fig::to_value`, which is what the
    // removed serde-sugar methods did for you.
    ed.replace_value(
        &[Segment::Key("title")],
        fig::to_value("has: colon").unwrap(),
    )
    .unwrap();
    // Reads back as the same logical value.
    let value: std::collections::BTreeMap<String, String> =
        fig::from_yaml_str(ed.source().unwrap()).unwrap();
    assert_eq!(value["title"], "has: colon");
}

#[test]
fn editor_delete_keeps_owned_comment_with_key() {
    let mut ed = Editor::open(b"a: 1\n# note for b\nb: 2\nc: 3\n", by_name("yaml")).unwrap();
    ed.delete_key(&[Segment::Key("b")]).unwrap();
    assert_eq!(ed.source().unwrap(), "a: 1\nc: 3\n");
}

#[test]
fn editor_reads_comments_distinguishing_absent_from_empty() {
    // `a` has both a leading block and a trailing comment; `b` has a bare `#`
    // trailing (present but empty); `c` has neither (absent).
    let ed = Editor::open(b"# why\na: 1 # two\nb: 2 #\nc: 3\n", by_name("yaml")).unwrap();

    assert_eq!(
        ed.leading_comment(&[Segment::Key("a")]).unwrap().as_deref(),
        Some("why")
    );
    assert_eq!(
        ed.trailing_comment(&[Segment::Key("a")])
            .unwrap()
            .as_deref(),
        Some("two")
    );

    // Present-but-empty bare marker → Some(""), not None.
    assert_eq!(
        ed.trailing_comment(&[Segment::Key("b")])
            .unwrap()
            .as_deref(),
        Some("")
    );

    // No comment at all → None.
    assert_eq!(ed.leading_comment(&[Segment::Key("c")]).unwrap(), None);
    assert_eq!(ed.trailing_comment(&[Segment::Key("c")]).unwrap(), None);
}

#[test]
fn editor_reads_trailing_comment_on_a_block_collection_key() {
    // The comment rides the `contents:` line above the block sequence.
    let ed = Editor::open(b"contents: # the list\n- one\n- two\n", by_name("yaml")).unwrap();
    assert_eq!(
        ed.trailing_comment(&[Segment::Key("contents")])
            .unwrap()
            .as_deref(),
        Some("the list"),
    );
}

#[test]
fn editor_flow_item_owns_no_comment_of_its_parents() {
    // An item of a one-line array sits on `members`' line, so the block above
    // that line — and the comment at its end — belong to `members`. The reads
    // used to hand the parent's text back through the item, and
    // `delete_leading_comments` used to remove it.
    let src = "# above members\nmembers = [\"a\", \"b\"]\n";
    let item0 = [Segment::Key("members"), Segment::Index(0)];

    let ed = Editor::open(src.as_bytes(), by_name("toml")).unwrap();
    assert_eq!(ed.leading_comment(&item0).unwrap(), None);
    assert_eq!(
        ed.leading_comment(&[Segment::Key("members")])
            .unwrap()
            .as_deref(),
        Some("above members"),
    );

    let mut ed = Editor::open(src.as_bytes(), by_name("toml")).unwrap();
    ed.delete_leading_comment(&item0).unwrap();
    assert_eq!(ed.source().unwrap(), src);

    // Adding one is refused rather than landing on the parent's line.
    let mut ed = Editor::open(src.as_bytes(), by_name("toml")).unwrap();
    let err = ed.add_leading_comment(&item0, "mine").unwrap_err();
    assert!(matches!(err, fig::Error::InvalidArgument), "{err:?}");
    assert_eq!(ed.source().unwrap(), src);

    // The trailing trio answers the same way.
    let trailing = "members = [\"a\", \"b\"] # note\n";
    let ed = Editor::open(trailing.as_bytes(), by_name("toml")).unwrap();
    assert_eq!(ed.trailing_comment(&item0).unwrap(), None);
    assert_eq!(
        ed.trailing_comment(&[Segment::Key("members")])
            .unwrap()
            .as_deref(),
        Some("note"),
    );

    // A multi-line array's items each begin their own line, and keep working.
    let multi = "# above members\nmembers = [\n  \"a\",\n  \"b\",\n]\n";
    let mut ed = Editor::open(multi.as_bytes(), by_name("toml")).unwrap();
    assert_eq!(ed.leading_comment(&item0).unwrap(), None);
    ed.add_leading_comment(&item0, "mine").unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "# above members\nmembers = [\n  # mine\n  \"a\",\n  \"b\",\n]\n",
    );
}

#[test]
fn editor_dangling_comment_round_trips() {
    // The third anchor: the run at the END of a container's body. Written at
    // the body's child depth, read back with the marker stripped, deleted whole.
    let mut ed = Editor::open(b"server:\n  port: 8080\nclient: 1\n", by_name("yaml")).unwrap();
    assert_eq!(
        ed.dangling_comment(&[Segment::Key("server")]).unwrap(),
        None
    );

    ed.add_dangling_comment(&[Segment::Key("server")], "was: here")
        .unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "server:\n  port: 8080\n  # was: here\nclient: 1\n"
    );
    assert_eq!(
        ed.dangling_comment(&[Segment::Key("server")])
            .unwrap()
            .as_deref(),
        Some("was: here")
    );

    ed.delete_dangling_comment(&[Segment::Key("server")])
        .unwrap();
    assert_eq!(ed.source().unwrap(), "server:\n  port: 8080\nclient: 1\n");
}

#[test]
fn editor_comment_out_and_back_is_byte_identical() {
    const SRC: &str = "server:\n  port: 8080\n  host: local\n";

    // A middle entry becomes the leading block of the entry below it.
    let mut ed = Editor::open(SRC.as_bytes(), by_name("yaml")).unwrap();
    ed.comment_out(&[Segment::Key("server"), Segment::Key("port")])
        .unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "server:\n  # port: 8080\n  host: local\n"
    );
    ed.uncomment_leading(&[Segment::Key("server"), Segment::Key("host")], 0, 1)
        .unwrap();
    assert_eq!(ed.source().unwrap(), SRC);

    // The LAST entry has no sibling below it: it becomes the parent's dangling
    // run, and comes back through `uncomment_dangling`.
    let mut ed = Editor::open(SRC.as_bytes(), by_name("yaml")).unwrap();
    ed.comment_out(&[Segment::Key("server"), Segment::Key("host")])
        .unwrap();
    assert_eq!(
        ed.dangling_comment(&[Segment::Key("server")])
            .unwrap()
            .as_deref(),
        Some("host: local")
    );
    ed.uncomment_dangling(&[Segment::Key("server")], 0, 1)
        .unwrap();
    assert_eq!(ed.source().unwrap(), SRC);
}

#[test]
fn editor_uncomment_refuses_lines_that_are_not_an_entry() {
    // `# two` is CONTENT of the block scalar. Stripping the marker parses, but
    // it changes `text` — a node the caller never named — so the splice is
    // rolled back and the document is byte-for-byte as it was.
    const SRC: &str = "text: |\n  one\n  # two\nb: 2\n";
    let mut ed = Editor::open(SRC.as_bytes(), by_name("yaml")).unwrap();
    let err = ed
        .uncomment_leading(&[Segment::Key("b")], 0, 1)
        .unwrap_err();
    assert!(matches!(err, fig::Error::UnsupportedOperation), "{err:?}");
    assert_eq!(ed.source().unwrap(), SRC);
}

#[test]
fn append_and_prepend_take_serialize_options_like_the_other_value_edits() {
    use fig::{SerializeOptions, Value};
    let item = Value::Map(vec![("k".into(), "v".into())]);
    let mut ed = Editor::open(b"items:\n- a\n", by_name("yaml")).unwrap();
    ed.append_value_with(&[Segment::Key("items")], &item, SerializeOptions::default())
        .unwrap();
    ed.prepend_value_with(&[Segment::Key("items")], "z", SerializeOptions::default())
        .unwrap();
    assert_eq!(ed.source().unwrap(), "items:\n- z\n- a\n- k: v\n");
}

#[test]
fn editor_sequence_ops() {
    let mut ed = Editor::open(b"items:\n- a\n- b\n", by_name("yaml")).unwrap();
    ed.append_value(&[Segment::Key("items")], "c").unwrap();
    ed.prepend_value(&[Segment::Key("items")], "z").unwrap();
    ed.delete_item(&[Segment::Key("items")], 2).unwrap();
    // z, a, c  (original b at index 2 after prepend was removed)
    assert_eq!(ed.source().unwrap(), "items:\n- z\n- a\n- c\n");
}

#[test]
fn insert_item_puts_an_item_at_an_index_in_block_and_flow_sequences() {
    let mut ed = Editor::open(
        b"items:\n- a\n# about c\n- c\nflow: [a, c]\n",
        by_name("yaml"),
    )
    .unwrap();
    ed.insert_item(&[Segment::Key("items")], 1, "b").unwrap();
    ed.insert_item(&[Segment::Key("flow")], 1, "b").unwrap();
    ed.insert_item(&[Segment::Key("flow")], usize::MAX, "d")
        .unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "items:\n- a\n- b\n# about c\n- c\nflow: [a, b, c, d]\n"
    );
    // Past the end: refused, and nothing written.
    let err = ed.insert_item(&[Segment::Key("flow")], 9, "z").unwrap_err();
    assert!(matches!(err, fig::Error::NotFound), "{err:?}");
    assert_eq!(
        ed.source().unwrap(),
        "items:\n- a\n- b\n# about c\n- c\nflow: [a, b, c, d]\n"
    );
}

#[test]
fn editor_set_sequence_reconciles_preserving_comments() {
    use fig::Value;
    let mut ed = Editor::open(
        b"tags:\n- a # first\n- b # second\n- c # third\n",
        by_name("yaml"),
    )
    .unwrap();
    // -> [c, a, d]: drop b, add d, reorder. Survivors keep their comments.
    let target = [
        Value::Str("c".into()),
        Value::Str("a".into()),
        Value::Str("d".into()),
    ];
    ed.set_sequence(&[Segment::Key("tags")], &target).unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "tags:\n- c # third\n- a # first\n- d\n",
    );
}

#[test]
fn editor_set_sequence_declines_empty_target() {
    let mut ed = Editor::open(b"tags:\n- a\n- b\n", by_name("yaml")).unwrap();
    let err = ed
        .set_sequence(&[Segment::Key("tags")], Vec::<fig::Value>::new())
        .unwrap_err();
    assert!(matches!(err, fig::Error::InvalidArgument));
    // Document untouched on a declined reconcile.
    assert_eq!(ed.source().unwrap(), "tags:\n- a\n- b\n");
}

#[test]
#[cfg(feature = "figl")]
fn fig_dialect_container_splices_render_flow_and_round_trip() {
    // A container value spliced into a figl document must render as flow
    // (`[a, b]` / `{k=v}`): the block spellings only parse as standalone
    // lines, so a block splice after `key = ` re-reads as a bare string.
    let mut em = Editor::open(b"t = x\n", by_name("figl")).unwrap();
    em.set_value(
        &[Segment::Key("contents")],
        fig::Value::Seq(vec![
            fig::Value::Str("a.md".into()),
            fig::Value::Str("b.md".into()),
        ]),
    )
    .unwrap();
    em.set_value(
        &[Segment::Key("meta")],
        fig::Value::Map(vec![(fig::Value::Str("k".into()), fig::Value::Int(1))]),
    )
    .unwrap();
    let rendered = em.source().unwrap().to_string();
    assert!(rendered.contains("contents = [a.md, b.md]"), "{rendered}");
    assert!(rendered.contains("meta = {k=1}"), "{rendered}");

    // And the result re-parses as the containers, not strings.
    let doc = fig::Document::parse(rendered.as_bytes(), by_name("figl")).unwrap();
    let v = doc.to_value().unwrap();
    let fig::Value::Map(entries) = &v else {
        panic!("{v:?}")
    };
    let contents = entries
        .iter()
        .find(|(k, _)| k == &fig::Value::Str("contents".into()));
    assert!(
        matches!(contents, Some((_, fig::Value::Seq(items))) if items.len() == 2),
        "{v:?}"
    );

    // Whole-document serialization of a Map is unchanged (still block sections).
    let map = fig::Value::Map(vec![(
        fig::Value::Str("title".into()),
        fig::Value::Str("T".into()),
    )]);
    assert_eq!(map.serialize(by_name("figl")).unwrap(), "title = T\n");
}

#[test]
#[cfg(feature = "figl")]
fn fig_dialect_block_map_splices_with_the_width_knob() {
    // The `*_with` twin honors the layout knob: a block map value lands as a
    // nested section under its key — the prov case
    // that plain `set_value` (forced inline flow) could not express.
    use fig::SerializeOptions;
    let mut em = Editor::open(b"title = hi\n", by_name("figl")).unwrap();
    em.set_value_with(
        &[Segment::Key("registry")],
        fig::Value::Map(vec![
            (fig::Value::Str("a".into()), fig::Value::Int(1)),
            (fig::Value::Str("b".into()), fig::Value::Int(2)),
        ]),
        SerializeOptions::default().width(1),
    )
    .unwrap();
    let rendered = em.source().unwrap().to_string();
    assert_eq!(rendered, "title = hi\nregistry\n> a = 1\n> b = 2\n");

    // It re-parses as the nested map, not a string.
    let doc = fig::Document::parse(rendered.as_bytes(), by_name("figl")).unwrap();
    let v = doc.to_value().unwrap();
    let fig::Value::Map(entries) = &v else {
        panic!("{v:?}")
    };
    let registry = entries
        .iter()
        .find(|(k, _)| k == &fig::Value::Str("registry".into()));
    assert!(
        matches!(registry, Some((_, fig::Value::Map(inner))) if inner.len() == 2),
        "{v:?}"
    );
}

#[test]
fn json5_editor_replaces_value_preserving_comments_and_unquoted_keys() {
    // JSON5 routes through the JSON editor in the JSON5 dialect. The edit splices
    // only the `8080` value node; unquoted keys, single quotes, the `//` comments,
    // and the trailing comma all stay byte-identical.
    let src = "{\n  // server config\n  host: 'localhost',\n  port: 8080, // default\n}\n";
    let mut ed = Editor::open(src.as_bytes(), by_name("json5")).unwrap();
    ed.replace_value(&[Segment::Key("port")], 9090).unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "{\n  // server config\n  host: 'localhost',\n  port: 9090, // default\n}\n",
    );
}

#[test]
fn json5_editor_delete_keeps_owned_line_comment_with_key() {
    let src = "{\n  host: 'localhost',\n  // the listening port\n  port: 8080,\n}\n";
    let mut ed = Editor::open(src.as_bytes(), by_name("json5")).unwrap();
    ed.delete_key(&[Segment::Key("port")]).unwrap();
    assert_eq!(ed.source().unwrap(), "{\n  host: 'localhost',\n}\n");
}

#[test]
fn toml_editor_renders_value_splice_as_toml_not_yaml() {
    // Splice text is rendered in the editor's own format. A replacement string
    // value must come out quoted (`"b"`) for TOML; the previous hardcoded-YAML
    // path emitted a bare `b`, which is not a valid TOML value and failed the
    // reparse. Integers are format-invariant, so `port` exercises the plain path.
    let mut ed = Editor::open(b"[server]\nhost = \"a\"\nport = 1\n", by_name("toml")).unwrap();
    ed.replace_value(&[Segment::Key("server"), Segment::Key("host")], "b")
        .unwrap();
    ed.replace_value(&[Segment::Key("server"), Segment::Key("port")], 9090)
        .unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "[server]\nhost = \"b\"\nport = 9090\n",
    );
}

#[test]
fn json_editor_rejects_json5_only_syntax() {
    // Sanity: the strict JSON dialect still refuses JSON5 input, so `by_name("json5")`
    // is a real, distinct selection and not just an alias.
    assert!(Editor::open(b"{ host: 'localhost' }", by_name("json")).is_err());
    assert!(Editor::open(b"{ host: 'localhost' }", by_name("json5")).is_ok());
}

#[test]
#[cfg(feature = "figl")]
fn fig_dialect_editor_edits_in_place() {
    let mut ed = Editor::open(b"title = old\nport = 8080\n", by_name("figl")).unwrap();
    ed.replace_value(&[Segment::Key("port")], 9090).unwrap();
    assert_eq!(ed.source().unwrap(), "title = old\nport = 9090\n");
}

// ── YAML container splices (regressions from downstream `set_meta_in_text`) ──

#[test]
#[cfg(feature = "yaml")]
fn yaml_single_entry_map_sets_like_a_multi_entry_one() {
    // A one-entry map renders `k: v`, which is shaped exactly like a scalar —
    // so every splice path used to read it as one and fail, while the same call
    // with two entries succeeded. Insert, nested insert, and replace must all
    // treat it as the mapping it is.
    use fig::Value;
    let one = || Value::Map(vec![(Value::Str("k".into()), Value::Str("v".into()))]);

    let mut ed = Editor::open(b"title: t\n", by_name("yaml")).unwrap();
    ed.set_value(&[Segment::Key("fresh")], one()).unwrap();
    assert_eq!(ed.source().unwrap(), "title: t\nfresh:\n  k: v\n");

    let mut ed = Editor::open(b"title: t\na:\n  b: 1\n", by_name("yaml")).unwrap();
    ed.set_value(&[Segment::Key("a"), Segment::Key("c")], one())
        .unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "title: t\na:\n  b: 1\n  c:\n    k: v\n"
    );

    let mut ed = Editor::open(b"title: t\nfresh: 0\n", by_name("yaml")).unwrap();
    ed.set_value(&[Segment::Key("fresh")], one()).unwrap();
    assert_eq!(ed.source().unwrap(), "title: t\nfresh:\n  k: v\n");

    // A null-valued key is a mapping waiting to happen — block values land in it.
    let mut ed = Editor::open(b"title: t\na:\n", by_name("yaml")).unwrap();
    ed.set_value(&[Segment::Key("a"), Segment::Key("b")], one())
        .unwrap();
    assert_eq!(ed.source().unwrap(), "title: t\na:\n  b:\n    k: v\n");
    let mut ed = Editor::open(b"title: t\na:\n", by_name("yaml")).unwrap();
    ed.set_value(
        &[Segment::Key("a"), Segment::Key("b")],
        fig::Value::Seq(vec![fig::Value::Str("q".into())]),
    )
    .unwrap();
    assert_eq!(ed.source().unwrap(), "title: t\na:\n  b:\n  - q\n");
}

#[test]
#[cfg(feature = "yaml")]
fn yaml_block_value_into_a_flow_container_errs_instead_of_corrupting() {
    // `a: {b: - a}` is not the sequence that was written — a lenient reader
    // takes it as the string "- a", so returning `Ok` here loses data silently.
    // The edit must be refused, and the document left byte-for-byte alone.
    use fig::Value;
    let seq = || Value::Seq(vec![Value::Str("a".into())]);

    // Into an existing flow mapping.
    let mut ed = Editor::open(b"title: t\na: {b: 1}\n", by_name("yaml")).unwrap();
    let err = ed
        .set_value(&[Segment::Key("a"), Segment::Key("c")], seq())
        .unwrap_err();
    assert!(matches!(err, fig::Error::InvalidArgument), "{err:?}");
    assert_eq!(ed.source().unwrap(), "title: t\na: {b: 1}\n");

    // A flow container the caller has to have written themselves is the only way
    // to reach this now: ancestors `set` creates are block (see the test below).
    let mut ed = Editor::open(b"title: t\na: {}\n", by_name("yaml")).unwrap();
    let err = ed
        .set_value(&[Segment::Key("a"), Segment::Key("b")], seq())
        .unwrap_err();
    assert!(matches!(err, fig::Error::InvalidArgument), "{err:?}");
    assert_eq!(ed.source().unwrap(), "title: t\na: {}\n");
}

#[test]
#[cfg(feature = "yaml")]
fn yaml_set_vivifies_a_fresh_nested_path_as_block_containers() {
    // Ancestors `set` creates are block mappings, so a fresh nested path reads
    // like hand-written YAML — and, unlike a flow seed, can hold a block value.
    use fig::Value;

    let mut ed = Editor::open(b"title: t\n", by_name("yaml")).unwrap();
    ed.set_value(
        &[Segment::Key("a"), Segment::Key("b"), Segment::Key("c")],
        1i64,
    )
    .unwrap();
    assert_eq!(ed.source().unwrap(), "title: t\na:\n  b:\n    c: 1\n");

    let mut ed = Editor::open(b"title: t\n", by_name("yaml")).unwrap();
    ed.set_value(
        &[Segment::Key("a"), Segment::Key("b")],
        Value::Seq(vec![Value::Str("q".into())]),
    )
    .unwrap();
    assert_eq!(ed.source().unwrap(), "title: t\na:\n  b:\n  - q\n");

    let mut ed = Editor::open(b"title: t\n", by_name("yaml")).unwrap();
    ed.set_value(
        &[Segment::Key("a"), Segment::Key("b")],
        Value::Map(vec![(Value::Str("k".into()), Value::Str("v".into()))]),
    )
    .unwrap();
    assert_eq!(ed.source().unwrap(), "title: t\na:\n  b:\n    k: v\n");
}

#[test]
#[cfg(feature = "yaml")]
fn yaml_flow_mapping_with_a_block_scalar_member_is_not_accepted_on_reparse() {
    // The reparse safety net's own regression: `{b: - a}` must not parse as a
    // string, or every flow splice loses its rollback.
    assert!(fig::Document::parse(b"a: {b: - a}\n", by_name("yaml")).is_err());
    assert!(fig::Document::parse(b"a: [- x]\n", by_name("yaml")).is_err());
    // Negative numbers and quoted dash-led text are untouched.
    assert!(fig::Document::parse(b"a: [-1, '- q']\n", by_name("yaml")).is_ok());
}

#[test]
#[cfg(feature = "yaml")]
fn yaml_set_seeds_a_nested_path_into_an_empty_document() {
    // A fresh, empty file is the `mkdir -p for config` case: a null root is a
    // container waiting to exist, not data standing in the way.
    let mut ed = Editor::open(b"", by_name("yaml")).unwrap();
    ed.set_value(&[Segment::Key("meta"), Segment::Key("title")], "Hi")
        .unwrap();
    assert_eq!(ed.source().unwrap(), "meta:\n  title: Hi\n");

    // A scalar standing where a parent mapping should be is still never clobbered.
    let mut ed = Editor::open(b"a: 1\n", by_name("yaml")).unwrap();
    assert!(
        ed.set_value(&[Segment::Key("a"), Segment::Key("b")], 2i64)
            .is_err()
    );
    assert_eq!(ed.source().unwrap(), "a: 1\n");
}

// --- whole-container ops ---
//
// The key methods cannot address a scattered container (a TOML `[header]`
// table, an INI `[section]`): its body is separate lines, so the editor's
// guards refuse rather than rewrite the header and rehome the entries. These
// six are the route for those shapes.

#[test]
#[cfg(feature = "toml")]
fn toml_container_ops_reach_a_table_the_key_ops_refuse() {
    let src = b"[a]\nx = 1\n[b]\ny = 2\n";

    // delete: the key op refuses, the container op takes the body with it.
    let mut ed = Editor::open(src, by_name("toml")).unwrap();
    assert!(matches!(
        ed.delete_key(&[Segment::Key("a")]),
        Err(fig::Error::InvalidArgument)
    ));
    ed.delete_container(&[Segment::Key("a")]).unwrap();
    assert_eq!(ed.source().unwrap(), "[b]\ny = 2\n");

    // rename: reaches every line that names the table, not just its header.
    let mut ed = Editor::open(b"[a]\nx = 1\n[a.b]\ny = 2\n", by_name("toml")).unwrap();
    ed.rename_container(&[Segment::Key("a")], "q").unwrap();
    assert_eq!(ed.source().unwrap(), "[q]\nx = 1\n[q.b]\ny = 2\n");

    // insert
    let mut ed = Editor::open(src, by_name("toml")).unwrap();
    ed.insert_container(&[Segment::Key("c")], "z = 3\n")
        .unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "[a]\nx = 1\n[b]\ny = 2\n\n[c]\nz = 3\n"
    );

    // append to an array-of-tables
    let mut ed = Editor::open(b"[[bin]]\nname = \"a\"\n", by_name("toml")).unwrap();
    ed.append_container_to_seq(&[Segment::Key("bin")], "name = \"b\"\n")
        .unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "[[bin]]\nname = \"a\"\n\n[[bin]]\nname = \"b\"\n"
    );
}

#[test]
#[cfg(feature = "toml")]
fn toml_move_and_reorder_containers_where_the_key_ops_refuse() {
    let src = b"[a]\nx = 1\n[b]\ny = 2\n";

    // `move_key` at a table path used to relocate the header alone, handing
    // `x = 1` to whichever table landed above it; it now refuses.
    let mut ed = Editor::open(src, by_name("toml")).unwrap();
    assert!(matches!(
        ed.move_key(&[Segment::Key("a")], &[Segment::Key("b")]),
        Err(fig::Error::InvalidArgument)
    ));
    // `None` is "to EOF" — the reason the destination is an Option rather than
    // an empty slice, which would name the root.
    ed.move_container(&[Segment::Key("a")], None).unwrap();
    assert_eq!(ed.source().unwrap(), "[b]\ny = 2\n\n[a]\nx = 1\n");

    let mut ed = Editor::open(src, by_name("toml")).unwrap();
    ed.move_container(&[Segment::Key("b")], Some(&[Segment::Key("a")]))
        .unwrap();
    assert_eq!(ed.source().unwrap(), "[b]\ny = 2\n[a]\nx = 1\n");

    // Same pairing for reorder.
    let mut ed = Editor::open(src, by_name("toml")).unwrap();
    assert!(matches!(
        ed.reorder_keys(&[], &["b", "a"]),
        Err(fig::Error::InvalidArgument)
    ));
    ed.reorder_containers(&["b", "a"]).unwrap();
    assert_eq!(ed.source().unwrap(), "[b]\ny = 2\n[a]\nx = 1\n");
}

#[test]
#[cfg(feature = "yaml")]
fn container_ops_are_unsupported_where_the_key_ops_already_suffice() {
    // YAML nests a container in one contiguous region, so it declares none of
    // the six — and `delete_key` handles the same shape directly.
    let mut ed = Editor::open(b"a:\n  x: 1\nb:\n  y: 2\n", by_name("yaml")).unwrap();
    assert!(matches!(
        ed.delete_container(&[Segment::Key("a")]),
        Err(fig::Error::UnsupportedFormat)
    ));
    ed.delete_key(&[Segment::Key("a")]).unwrap();
    assert_eq!(ed.source().unwrap(), "b:\n  y: 2\n");
}

#[test]
fn editor_insert_takes_the_keys_name_and_spells_it_as_the_format_does() {
    // JSON quotes and escapes it; TOML quotes it when it must. (ZON, where
    // a string value is not a key at all, is off in the default feature
    // set; the runtime twins hold it to `.name` there.)
    let mut json = Editor::open(b"{\"a\": 1}", by_name("json")).unwrap();
    json.insert_value(&[], "k\"q", 2).unwrap();
    assert_eq!(json.source().unwrap(), "{\"a\": 1, \"k\\\"q\": 2}");
    let mut toml = Editor::open(b"a = 1\n", by_name("toml")).unwrap();
    toml.insert_value(&[], "has space", 2).unwrap();
    assert_eq!(toml.source().unwrap(), "a = 1\n\"has space\" = 2\n");
}

#[test]
fn editor_replace_key_takes_the_keys_name_and_spells_it_as_the_format_does() {
    let mut json = Editor::open(b"{\"a\": 1}", by_name("json")).unwrap();
    json.rename_key(&[Segment::Key("a")], "k\"q").unwrap();
    assert_eq!(json.source().unwrap(), "{\"k\\\"q\": 1}");
    let mut toml = Editor::open(b"a = 1\n", by_name("toml")).unwrap();
    toml.rename_key(&[Segment::Key("a")], "has space").unwrap();
    assert_eq!(toml.source().unwrap(), "\"has space\" = 1\n");
    let mut nt = Editor::open(b"a: 1\n", by_name("nestedtext")).unwrap();
    nt.rename_key(&[Segment::Key("a")], "k").unwrap();
    assert_eq!(nt.source().unwrap(), "k: 1\n");
}

#[cfg(feature = "zon")]
#[test]
fn editor_replace_key_spells_a_zon_field() {
    let mut ed = Editor::open(b".{ .a = 1 }", by_name("zon")).unwrap();
    ed.rename_key(&[Segment::Key("a")], "k").unwrap();
    assert_eq!(ed.source().unwrap(), ".{ .k = 1 }");
}

#[cfg(feature = "plist")]
#[test]
fn editor_plist_keys_are_names_on_insert_and_replace() {
    let src = b"<dict>\n  <key>a</key>\n  <string>x</string>\n</dict>\n";
    let mut ed = Editor::open(src, by_name("plist")).unwrap();
    ed.insert_value(&[], "n", 42i64).unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "<dict>\n  <key>a</key>\n  <string>x</string>\n  <key>n</key>\n  <integer>42</integer>\n</dict>\n"
    );
    ed.rename_key(&[Segment::Key("a")], "b&c").unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "<dict>\n  <key>b&amp;c</key>\n  <string>x</string>\n  <key>n</key>\n  <integer>42</integer>\n</dict>\n"
    );
}

#[test]
fn editor_nestedtext_value_is_spliced_as_plain_text_and_blocked_once() {
    let mut ed = Editor::open(b"name: fig\n", by_name("nestedtext")).unwrap();
    ed.replace_value(&[Segment::Key("name")], "h2").unwrap();
    assert_eq!(ed.source().unwrap(), "name: h2\n");
    ed.insert_value(&[], "new", "two\nlines").unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "name: h2\nnew:\n    > two\n    > lines\n"
    );
}

#[test]
fn serialize_is_still_a_whole_document_where_splice_text_is_not() {
    // The editor's splice text is a NestedText scalar's plain text; a value
    // serialized to be written out stays a document, whose scalar is a block.
    let v = fig::Value::Str("h2".into());
    assert_eq!(v.serialize(by_name("nestedtext")).unwrap(), "> h2\n");
}

#[cfg(feature = "plist")]
#[test]
fn serialize_keeps_the_plist_wrapper() {
    let out = fig::Value::Int(42).serialize(by_name("plist")).unwrap();
    assert!(out.starts_with("<?xml"), "{out}");
    assert!(out.contains("<plist version=\"1.0\">"), "{out}");
}

#[test]
fn editor_nestedtext_takes_a_container_as_nested_entries() {
    use fig::Value;
    let mut ed = Editor::open(b"name: fig\n", by_name("nestedtext")).unwrap();
    let m = Value::Map(vec![
        (Value::Str("x".into()), Value::Str("1".into())),
        (
            Value::Str("l".into()),
            Value::Seq(vec![Value::Str("a".into()), Value::Str("b".into())]),
        ),
    ]);
    ed.insert_value(&[], "m", m).unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "name: fig\nm:\n    x: 1\n    l:\n        - a\n        - b\n"
    );
}

#[test]
fn json_reorder_keys_keeps_the_object_valid() {
    // Every JSON object is a flow mapping, which `reorder_keys` used to refuse
    // with `InvalidArgument`. It now moves the members and leaves each slot's
    // comma where it was, so the member moved off the end gains one.
    let src = "{\n  \"name\": \"music\",\n  \"private\": true,\n  \"type\": \"module\"\n}\n";
    let mut ed = Editor::open(src.as_bytes(), by_name("json")).unwrap();
    ed.reorder_keys(&[], &["type", "private", "name"]).unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "{\n  \"type\": \"module\",\n  \"private\": true,\n  \"name\": \"music\"\n}\n",
    );

    let src =
        "{\n  \"scripts\": {\n    \"dev\": \"vite\",\n    \"build\": \"vite build\"\n  }\n}\n";
    let mut ed = Editor::open(src.as_bytes(), by_name("json")).unwrap();
    ed.reorder_keys(&[Segment::Key("scripts")], &["build"])
        .unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "{\n  \"scripts\": {\n    \"build\": \"vite build\",\n    \"dev\": \"vite\"\n  }\n}\n",
    );

    // JSONC: a member's comments move with it.
    let src = "{\n  // pkg\n  \"name\": \"music\",\n  \"type\": \"module\" // esm\n}\n";
    let mut ed = Editor::open(src.as_bytes(), by_name("jsonc")).unwrap();
    ed.reorder_keys(&[], &["type"]).unwrap();
    assert_eq!(
        ed.source().unwrap(),
        "{\n  \"type\": \"module\", // esm\n  // pkg\n  \"name\": \"music\"\n}\n",
    );
}

/// The registered format named `name`; a test that names one this build
/// does not carry is a test bug.
#[allow(dead_code)]
fn by_name(name: &str) -> fig::Format {
    fig::Format::by_name(name).unwrap_or_else(|| panic!("no format named {name:?}"))
}
