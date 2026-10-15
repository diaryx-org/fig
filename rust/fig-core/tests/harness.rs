//! Port of src/contract/harness.zig's tests: the checks that hold for ANY
//! format, run over every dialect of every language this crate bundles.
//!
//!   * every `empty_doc_seed` parses (it is what `set` writes to a file that
//!     does not exist yet) and round-trips;
//!   * every sample a format declares parses, prints, and reparses to the
//!     same tree;
//!   * `Document.regions_of` is well-formed — whole lines, container nodes,
//!     sorted — and a section format's parser actually fills it;
//!   * a dangling comment written at the root reads back and deletes clean;
//!   * `Editor` constructs over every sample of an editable format, and a
//!     no-op splice leaves the source untouched and the document parsed.
//!
//! The registry's registration harness (`registry.rs`'s
//! `check_registration`, Zig's runtime.zig) runs the same checks over a
//! registration, through the registered language.

use fig_core::editor::Editor;
use fig_core::kit::ast::{NodeKind, SerializeOptions, AST};
use fig_core::kit::document::Document;
use fig_core::kit::language::{Dialect, DialectInfo, Language};
use fig_core::kit::span::Span;
use fig_core::lang;
use fig_core::serialize::{serialize_with, Target};

/// The languages this crate bundles, in `LIST_ROWS` order (Zig's
/// `list.rows` filtered by `bundle.bundles`).
fn bundled() -> Vec<&'static dyn Language> {
    vec![
        &lang::json::LANGUAGE,
        &lang::yaml::LANGUAGE,
        &lang::toml::LANGUAGE,
        &lang::zon::LANGUAGE,
        &lang::figl::LANGUAGE,
        &lang::ini::LANGUAGE,
        &lang::dotenv::LANGUAGE,
        &lang::properties::LANGUAGE,
        &lang::plist::LANGUAGE,
        &lang::nestedtext::LANGUAGE,
    ]
}

/// Every declared dialect of every bundled language, with its language.
fn dialects() -> Vec<(&'static dyn Language, &'static DialectInfo)> {
    let mut out = Vec::new();
    for l in bundled() {
        for d in l.dialects() {
            out.push((l, d));
        }
    }
    out
}

/// Whether the language declares `samples` (Zig's `@hasDecl(Lang, "samples")`).
fn has_samples(l: &dyn Language) -> bool {
    !l.samples().is_empty()
}

/// Parse `input` as dialect `d` through the language's own entry point.
fn parse_with(l: &dyn Language, d: Dialect, input: &[u8]) -> Document {
    match l.parse(input, d) {
        Ok(doc) => doc,
        Err(e) => panic!("{}: does not parse ({}):\n{}", l.name(), e.name(), String::from_utf8_lossy(input)),
    }
}

/// The tree's canonical encoding, which is how two trees are compared here:
/// node ids depend on the order a parser met the nodes in, so a positional
/// comparison could call an identical tree changed.
fn canonical_of(ast: &AST) -> String {
    let mut out = Vec::new();
    serialize_with(ast, &mut out, &Target::Canonical, &SerializeOptions::default()).unwrap();
    String::from_utf8(out).unwrap()
}

/// Parse, print, reparse; the two trees must be the same tree.
fn expect_round_trip(l: &dyn Language, d: &DialectInfo, sample: &[u8]) {
    let first = parse_with(l, d.dialect, sample);
    if !l.caps().serialize {
        return;
    }
    let mut printed = Vec::new();
    l.print(d.dialect, &first.ast, &SerializeOptions::default(), &mut printed)
        .unwrap_or_else(|e| panic!("{}: sample does not print ({})", d.name, e.name()));
    let second = l.parse(&printed, d.dialect).unwrap_or_else(|e| {
        panic!(
            "{}: printed output does not parse ({})\n--- sample ---\n{}\n--- printed ---\n{}",
            d.name,
            e.name(),
            String::from_utf8_lossy(sample),
            String::from_utf8_lossy(&printed)
        )
    });
    let a = canonical_of(&first.ast);
    let b = canonical_of(&second.ast);
    assert_eq!(
        a,
        b,
        "{}: print → reparse changed the tree\n--- sample ---\n{}\n--- printed ---\n{}",
        d.name,
        String::from_utf8_lossy(sample),
        String::from_utf8_lossy(&printed)
    );
}

/// `Document.regions_of` well-formedness: each row is a whole physical
/// line of a container node, and the table is sorted by `(node_id, start)`.
fn expect_regions_well_formed(doc: &Document, source: &[u8]) {
    let mut prev: Option<(u32, u32)> = None;
    for r in doc.ast.region_facts() {
        assert!((r.node as usize) < doc.ast.node_count());
        assert!(matches!(doc.ast.kind_of(r.node), NodeKind::Mapping(_) | NodeKind::Sequence(_)));
        let (start, end) = (r.start as usize, r.end as usize);
        assert!(start < end && end <= source.len());
        assert!(start == 0 || source[start - 1] == b'\n');
        assert!(end == source.len() || source[end - 1] == b'\n');
        if let Some((node, at)) = prev {
            assert!(node < r.node || (node == r.node && at < r.start));
        }
        prev = Some((r.node, r.start));
    }
}

/// Whether some dialect of `l` declares itself a section format.
fn is_section_format(l: &dyn Language) -> bool {
    if !l.caps().edit {
        return false;
    }
    (0..l.type_count()).any(|t| l.syntax(t as Dialect).section_noun.is_some())
}

#[test]
fn every_empty_doc_seed_parses_and_round_trips() {
    for (l, d) in dialects() {
        if let Some(seed) = d.empty_doc_seed {
            expect_round_trip(l, d, seed);
        }
    }
}

#[test]
fn every_declared_sample_parses_prints_and_reparses_to_the_same_tree() {
    for (l, d) in dialects() {
        for sample in l.samples() {
            expect_round_trip(l, d, sample);
        }
    }
}

#[test]
fn the_region_list_is_well_formed_and_a_section_formats_parser_fills_it() {
    for (l, d) in dialects() {
        if !has_samples(l) {
            continue;
        }
        let mut any_regions = false;
        for sample in l.samples() {
            let doc = parse_with(l, d.dialect, sample);
            expect_regions_well_formed(&doc, sample);
            if !doc.ast.region_facts().is_empty() {
                any_regions = true;
            }
        }
        if is_section_format(l) {
            // A section format whose samples produce no section is either a
            // parser that stopped recording headers or a sample set that
            // never opens one; both are the format's to fix.
            assert!(any_regions, "{}: declares section_noun but no sample produced a header region", d.name);
        } else {
            assert!(!any_regions, "{}: records regions but declares no section_noun", d.name);
        }
    }
}

#[test]
fn a_dangling_comment_written_at_the_root_reads_back_and_deletes_clean() {
    // The dangling anchor is derived from the parse (the container's last
    // own-line child) rather than declared per format, so every editable
    // format's samples are put through the write/read/delete cycle here; the
    // two refusals it may legitimately give — no comment syntax at all, and
    // no body line to anchor on — are the documented answers.
    for (l, d) in dialects() {
        if !l.caps().edit || !has_samples(l) {
            continue;
        }
        for &sample in l.samples() {
            let mut ed = Editor::new(l, d.dialect);
            ed.init(sample).unwrap();
            match ed.add_dangling_comment(&[], b"harness") {
                Ok(()) => {
                    let got = ed.get_dangling_comment(&[]).unwrap();
                    assert_eq!(Some(&b"harness"[..]), got.as_deref(), "{}", d.name);
                    ed.delete_dangling_comments(&[]).unwrap();
                    assert_eq!(String::from_utf8_lossy(sample), String::from_utf8_lossy(ed.source()), "{}", d.name);
                }
                Err(e) if e.is("CommentsUnsupported") || e.is("UnsupportedShape") => {}
                Err(e) => panic!("{}: {}", d.name, e.name()),
            }
        }
    }
}

#[test]
fn editor_constructs_over_every_sample_and_a_no_op_splice_changes_nothing() {
    for (l, d) in dialects() {
        if !l.caps().edit || !has_samples(l) {
            continue;
        }
        for &sample in l.samples() {
            let mut ed = Editor::new(l, d.dialect);
            ed.init(sample).unwrap();
            ed.replace_at_span(Span::new(0, 0), b"").unwrap();
            assert_eq!(String::from_utf8_lossy(sample), String::from_utf8_lossy(ed.source()), "{}", d.name);
            ed.get_parsed().unwrap();
        }
    }
}
