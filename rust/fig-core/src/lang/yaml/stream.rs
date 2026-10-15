//! Port of languages/yaml/src/stream.zig.
//!
//! YAML's multi-document stream: `---` and `...` markers at column 0 split a
//! file into documents, each parsed on its own by the single-document parser.
//! YAML's own concept, so it lives with YAML rather than in core. `locate` is
//! YAML's `split_stream` declaration, which core and the bindings reach
//! through the contract (docs/proposals/core-only.md §4.5).

use super::parser;
use super::Type;
use crate::kit::document::Document;
use crate::kit::manifest::StreamDocument;
use crate::kit::span::Span;
use crate::Result;

/// One document within a multi-document YAML stream, located in *outer-source*
/// byte coordinates. `content` is the exact slice handed to the single-document
/// parser: it INCLUDES a leading `---` marker line (which the parser consumes)
/// but excludes a trailing `...`. `explicit` records whether the document opened
/// with a `---` marker (vs. a bare document at stream start or after `...`).
#[derive(Debug, Clone)]
pub struct StreamDoc {
    pub content: Span,
    pub explicit: bool,
    pub document: Document,
}

impl StreamDoc {
    /// Lift a node span (relative to this document's content) into outer-file
    /// coordinates.
    pub fn outer_span(&self, s: Span) -> Span {
        let base = self.content.start;
        Span::new(s.start + base, s.end + base)
    }
}

/// A parsed multi-document YAML stream. `source` is the borrowed outer file;
/// each document's `content` indexes into it.
#[derive(Debug, Clone)]
pub struct Stream<'a> {
    pub source: &'a [u8],
    pub documents: Vec<StreamDoc>,
}

/// Locate the documents of a YAML stream: each one's byte range and whether
/// it opened with a `---` marker, without parsing any. This is YAML's
/// `split_stream`; `split` parses each range on top of it.
///
/// A range INCLUDES a leading `---` marker line (which the parser consumes)
/// and excludes a trailing `...`.
///
/// Always returns at least one document: an empty or comment-only stream is
/// a single bare document.
pub fn locate(source: &[u8]) -> Vec<StreamDocument> {
    let mut docs: Vec<StreamDocument> = Vec::new();

    let mut start: usize = 0;
    if source.starts_with(b"\xEF\xBB\xBF") {
        start += 3; // UTF-8 BOM
    }

    let mut seg_start = start;
    let mut seg_explicit = false;

    let mut line = start;
    while line < source.len() {
        let next = line_end(source, line);
        match marker_kind(source, line) {
            MarkerKind::Start => {
                // A `---` ends the current segment and opens a new explicit
                // document *at* this line. Directives (`%YAML`/`%TAG`) preceding
                // the `---` belong to the document it introduces.
                if !segment_is_directives(&source[seg_start..line]) {
                    push_segment(source, &mut docs, seg_start, line, seg_explicit);
                    seg_start = line;
                }
                seg_explicit = true;
            }
            MarkerKind::End => {
                // A `...` ends the current document; the marker itself is left
                // out of the slice. What follows is a fresh bare document.
                push_segment(source, &mut docs, seg_start, line, seg_explicit);
                seg_start = next;
                seg_explicit = false;
            }
            MarkerKind::None => {}
        }
        line = next;
    }
    push_segment(source, &mut docs, seg_start, source.len(), seg_explicit);

    // An empty / trivia-only stream is one null document.
    if docs.is_empty() {
        docs.push(StreamDocument { start, end: source.len(), explicit: false });
    }
    docs
}

/// Split a YAML stream into its documents (`locate`) and parse each one
/// independently with the single-document parser. A parse error in any
/// document propagates.
pub fn split(source: &[u8]) -> Result<Stream<'_>> {
    let located = locate(source);
    let mut docs: Vec<StreamDoc> = Vec::new();
    for l in located {
        let doc = parse_yaml_slice(&source[l.start..l.end])?;
        docs.push(StreamDoc { content: Span::new(l.start, l.end), explicit: l.explicit, document: doc });
    }
    Ok(Stream { source, documents: docs })
}

/// Append `source[seg_start..seg_end]` as a document, unless it is a bare
/// segment with no real content (blank/comment-only). An explicit segment
/// (opened by `---`) is always a document, even when its body is empty.
fn push_segment(source: &[u8], docs: &mut Vec<StreamDocument>, seg_start: usize, seg_end: usize, explicit: bool) {
    if !explicit && !has_content(&source[seg_start..seg_end]) {
        return;
    }
    docs.push(StreamDocument { start: seg_start, end: seg_end, explicit });
}

/// Parse one segment as a single YAML document.
fn parse_yaml_slice(slice: &[u8]) -> Result<Document> {
    parser::parse(slice, Type::V1_2_2)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkerKind {
    Start,
    End,
    None,
}

/// Classify the column-0 line at `at` as a document `---`/`...` marker. A `---`
/// marker is the bare token or a `--- `/`---\t` prefix (inline content allowed);
/// `---foo` is a plain scalar, not a marker. A `...` marker must occupy the
/// whole line.
fn marker_kind(source: &[u8], at: usize) -> MarkerKind {
    let eol = line_end(source, at);
    let line = trim_end(&source[at..eol], b"\r\n");
    if line == b"---" {
        return MarkerKind::Start;
    }
    if line.starts_with(b"--- ") || line.starts_with(b"---\t") {
        return MarkerKind::Start;
    }
    if trim_end(line, b" \t") == b"..." {
        return MarkerKind::End;
    }
    MarkerKind::None
}

/// True if every content line of `slice` (ignoring blanks and comments) is a
/// column-0 directive (`%…`), and there is at least one.
fn segment_is_directives(slice: &[u8]) -> bool {
    let mut any = false;
    let mut i = 0usize;
    while i < slice.len() {
        let eol = line_end(slice, i);
        let line = trim_end(&slice[i..eol], b"\r\n");
        i = eol;
        if line.is_empty() {
            continue;
        }
        if line[0] == b'%' {
            any = true; // a directive sits at column 0
            continue;
        }
        let body = trim(line, b" \t");
        if body.is_empty() || body[0] == b'#' {
            continue; // blank or comment
        }
        return false; // some other content — not a pure directives prefix
    }
    any
}

/// True if `slice` has any line that is neither blank nor a comment.
fn has_content(slice: &[u8]) -> bool {
    let mut i = 0usize;
    while i < slice.len() {
        let eol = line_end(slice, i);
        let trimmed = trim(&slice[i..eol], b" \t\r\n");
        if !trimmed.is_empty() && trimmed[0] != b'#' {
            return true;
        }
        i = eol;
    }
    false
}

fn line_end(source: &[u8], from: usize) -> usize {
    match source[from..].iter().position(|&c| c == b'\n') {
        Some(nl) => from + nl + 1,
        None => source.len(),
    }
}

fn trim_end<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let mut b = s.len();
    while b > 0 && set.contains(&s[b - 1]) {
        b -= 1;
    }
    &s[..b]
}

fn trim<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let s = trim_end(s, set);
    let mut a = 0;
    while a < s.len() && set.contains(&s[a]) {
        a += 1;
    }
    &s[a..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::err;
    use crate::kit::ast::{NodeTag, PathSegment};

    fn root_kind(doc: &Document) -> crate::kit::ast::NodeKind<'_> {
        doc.ast.node(doc.ast.root).kind
    }

    #[test]
    fn two_explicit_documents_in_a_stream_jhb9() {
        let src = b"# Ranking of 1998 home runs\n---\n- Mark McGwire\n- Sammy Sosa\n\n# Team ranking\n---\n- Chicago Cubs\n- St Louis Cardinals\n";
        let stream = split(src).unwrap();
        // The leading comment-only segment is not a document.
        assert_eq!(2, stream.documents.len());
        assert!(stream.documents[0].explicit);
        assert!(stream.documents[1].explicit);
        assert_eq!(b"Mark McGwire", stream.documents[0].document.ast.get_val_by_path(&[PathSegment::Index(0)]).unwrap().kind.as_string().unwrap());
        assert_eq!(b"St Louis Cardinals", stream.documents[1].document.ast.get_val_by_path(&[PathSegment::Index(1)]).unwrap().kind.as_string().unwrap());
    }

    #[test]
    fn directives_fold_into_the_following_document_6zkb_shaped() {
        let src = b"Document\n---\n# Empty\n...\n%YAML 1.2\n---\nmatches %: 20\n";
        let stream = split(src).unwrap();
        assert_eq!(3, stream.documents.len());
        assert_eq!(b"Document", root_kind(&stream.documents[0].document).as_string().unwrap());
        assert_eq!(NodeTag::Null, root_kind(&stream.documents[1].document).tag());
        assert_eq!(b"20", stream.documents[2].document.ast.get_val_by_path(&[PathSegment::Key(b"matches %")]).unwrap().kind.as_number().unwrap().raw);
    }

    #[test]
    fn a_tag_handle_scoped_to_the_first_document_fails_later_use_qlj7() {
        let src = b"%TAG !prefix! tag:example.com,2011:\n--- !prefix!A\na: b\n--- !prefix!B\nc: d\n";
        assert_eq!(err!(UndefinedTagHandle), split(src).unwrap_err());
    }

    #[test]
    fn two_document_start_markers_yields_two_null_docs_6xdy() {
        let stream = split(b"---\n---\n").unwrap();
        assert_eq!(2, stream.documents.len());
        assert_eq!(NodeTag::Null, root_kind(&stream.documents[0].document).tag());
        assert_eq!(NodeTag::Null, root_kind(&stream.documents[1].document).tag());
    }

    #[test]
    fn document_start_on_last_line_puw8() {
        let stream = split(b"---\na: b\n---\n").unwrap();
        assert_eq!(2, stream.documents.len());
        assert_eq!(b"b", stream.documents[0].document.ast.get_val_by_path(&[PathSegment::Key(b"a")]).unwrap().kind.as_string().unwrap());
        assert_eq!(NodeTag::Null, root_kind(&stream.documents[1].document).tag());
    }

    #[test]
    fn bare_docs_separated_by_dots_with_a_comment_only_segment_m7a3() {
        let src = b"Bare\ndocument\n...\n# No document\n...\n|\n  %!PS-Adobe-2.0 # Not the first line\n";
        let stream = split(src).unwrap();
        // The `# No document` segment is comment-only and produces no document.
        assert_eq!(2, stream.documents.len());
        assert!(!stream.documents[0].explicit);
        assert_eq!(b"Bare document", root_kind(&stream.documents[0].document).as_string().unwrap());
    }

    #[test]
    fn inline_content_on_the_marker_line_l383() {
        let stream = split(b"--- foo  # comment\n--- foo  # comment\n").unwrap();
        assert_eq!(2, stream.documents.len());
        assert_eq!(b"foo", root_kind(&stream.documents[0].document).as_string().unwrap());
        assert_eq!(b"foo", root_kind(&stream.documents[1].document).as_string().unwrap());
    }

    #[test]
    fn explicit_doc_then_bare_doc_after_dots_7z25() {
        let stream = split(b"---\nscalar1\n...\nkey: value\n").unwrap();
        assert_eq!(2, stream.documents.len());
        assert!(stream.documents[0].explicit);
        assert_eq!(b"scalar1", root_kind(&stream.documents[0].document).as_string().unwrap());
        assert!(!stream.documents[1].explicit);
        assert_eq!(b"value", stream.documents[1].document.ast.get_val_by_path(&[PathSegment::Key(b"key")]).unwrap().kind.as_string().unwrap());
    }

    #[test]
    fn single_bare_document() {
        let stream = split(b"key: value\n").unwrap();
        assert_eq!(1, stream.documents.len());
        assert!(!stream.documents[0].explicit);
    }

    #[test]
    fn empty_stream_is_one_null_document() {
        let stream = split(b"").unwrap();
        assert_eq!(1, stream.documents.len());
        assert_eq!(NodeTag::Null, root_kind(&stream.documents[0].document).tag());
    }

    #[test]
    fn outer_span_lifts_node_spans_into_outer_coordinates() {
        let src = b"---\nfoo\n---\nbar\n";
        let stream = split(src).unwrap();
        assert_eq!(2, stream.documents.len());
        let d1 = &stream.documents[1];
        let node = d1.document.ast.node(d1.document.ast.root);
        let outer = d1.outer_span(d1.document.span(&node));
        assert_eq!(b"bar", &src[outer.start..outer.end]);
    }
}
