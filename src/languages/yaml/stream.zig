//! YAML's multi-document stream: `---` and `...` markers at column 0 split a
//! file into documents, each parsed on its own by the single-document parser.
//! YAML's own concept, so it lives with YAML rather than in core's embed
//! layer. `locate` is YAML's `splitStream` declaration, which core and the
//! bindings reach through the contract (docs/proposals/core-only.md §4.5).

const std = @import("std");
const Allocator = std.mem.Allocator;
const testing = std.testing;
const AST = @import("../../ast/ast.zig");
const Document = @import("../../document.zig");
const Span = @import("../../util/span.zig");
const Parser = @import("parser.zig");
const lang = @import("../manifest.zig");

/// One document within a multi-document YAML stream, located in *outer-source*
/// byte coordinates. `content` is the exact slice handed to the single-document
/// parser: it INCLUDES a leading `---` marker line (which the parser consumes)
/// but excludes a trailing `...`. `explicit` records whether the document opened
/// with a `---` marker (vs. a bare document at stream start or after `...`).
pub const StreamDoc = struct {
    content: Span,
    explicit: bool,
    document: Document,

    /// Lift a node span (relative to this document's content) into outer-file
    /// coordinates — as an embedded region's `outerSpan` does.
    pub fn outerSpan(self: StreamDoc, s: Span) Span {
        const base = self.content.start;
        return Span.init(s.start + base, s.end + base);
    }
};

/// A parsed multi-document YAML stream. `source` is the borrowed outer file;
/// each document's `content` indexes into it. The single-document parser only
/// ever sees one document at a time, so this never trips its multi-document
/// guard — the stream concept lives here, in the splitter, not in the parser.
pub const Stream = struct {
    source: []const u8,
    documents: []const StreamDoc,

    pub fn deinit(self: Stream, allocator: Allocator) void {
        for (self.documents) |d| d.document.deinit(allocator);
        allocator.free(self.documents);
    }
};

/// Locate the documents of a YAML stream: each one's byte range and whether
/// it opened with a `---` marker, without parsing any. This is YAML's
/// `splitStream`; `split` parses each range on top of it.
///
/// A stream is a sequence of documents delimited by `---` (start) and `...`
/// (end) markers at column 0. Rather than teach the parser to span
/// documents — which would complicate in-place editing and round-tripping —
/// each document's range is located here and handed to the parser on its
/// own. A range INCLUDES a leading `---` marker line (which the parser
/// consumes) and excludes a trailing `...`.
///
/// Always returns at least one document: an empty or comment-only stream is
/// a single bare document.
pub fn locate(allocator: Allocator, source: []const u8) Allocator.Error![]lang.StreamDocument {
    var docs: std.ArrayList(lang.StreamDocument) = .empty;
    errdefer docs.deinit(allocator);

    var start: usize = 0;
    if (std.mem.startsWith(u8, source, "\xEF\xBB\xBF")) start += 3; // UTF-8 BOM

    var seg_start: usize = start;
    var seg_explicit = false;

    var line = start;
    while (line < source.len) {
        const next = lineEnd(source, line);
        switch (markerKind(source, line)) {
            .start => {
                // A `---` ends the current segment and opens a new explicit
                // document *at* this line — the marker stays in the slice so
                // inline content (`--- foo`) is preserved and the parser eats
                // the marker token.
                //
                // Directives (`%YAML`/`%TAG`) preceding the `---` belong to the
                // document it introduces. If the pending segment is just those
                // directives (and trivia), don't close it here — let the explicit
                // document include them, so the parser sees `%YAML\n---\n…` as one
                // document and validates the directive against its marker.
                if (!segmentIsDirectives(source[seg_start..line])) {
                    try pushSegment(allocator, source, &docs, seg_start, line, seg_explicit);
                    seg_start = line;
                }
                seg_explicit = true;
            },
            .end => {
                // A `...` ends the current document; the marker itself is left
                // out of the slice. What follows is a fresh bare document.
                try pushSegment(allocator, source, &docs, seg_start, line, seg_explicit);
                seg_start = next;
                seg_explicit = false;
            },
            .none => {},
        }
        line = next;
    }
    try pushSegment(allocator, source, &docs, seg_start, source.len, seg_explicit);

    // An empty / trivia-only stream is one null document.
    if (docs.items.len == 0) try docs.append(allocator, .{ .start = start, .end = source.len, .explicit = false });

    return docs.toOwnedSlice(allocator);
}

/// Split a YAML stream into its documents (`locate`) and parse each one
/// independently with the single-document parser. A parse error in any
/// document propagates, and the documents already parsed are freed.
pub fn split(allocator: Allocator, source: []const u8) !Stream {
    const located = try locate(allocator, source);
    defer allocator.free(located);
    var docs: std.ArrayList(StreamDoc) = .empty;
    errdefer {
        for (docs.items) |d| d.document.deinit(allocator);
        docs.deinit(allocator);
    }
    for (located) |l| {
        const doc = try parseYamlSlice(allocator, source[l.start..l.end]);
        try docs.append(allocator, .{ .content = Span.init(l.start, l.end), .explicit = l.explicit, .document = doc });
    }
    return .{ .source = source, .documents = try docs.toOwnedSlice(allocator) };
}

/// Append `source[seg_start..seg_end]` as a document, unless it is a bare
/// segment with no real content (blank/comment-only) — such a segment is not
/// a document (e.g. a leading comment before the first `---`). An explicit
/// segment (opened by `---`) is always a document, even when its body is empty.
fn pushSegment(
    allocator: Allocator,
    source: []const u8,
    docs: *std.ArrayList(lang.StreamDocument),
    seg_start: usize,
    seg_end: usize,
    explicit: bool,
) Allocator.Error!void {
    if (!explicit and !hasContent(source[seg_start..seg_end])) return;
    try docs.append(allocator, .{ .start = seg_start, .end = seg_end, .explicit = explicit });
}

/// Parse one segment as a single YAML document.
fn parseYamlSlice(allocator: Allocator, slice: []const u8) !Document {
    return Parser.parse(allocator, slice, .v1_2_2);
}

const MarkerKind = enum { start, end, none };

/// Classify the column-0 line at `at` as a document `---`/`...` marker. A `---`
/// marker is the bare token or a `--- `/`---\t` prefix (inline content allowed);
/// `---foo` is a plain scalar, not a marker. A `...` marker must occupy the
/// whole line. Indented lines never match (callers only pass line starts).
fn markerKind(source: []const u8, at: usize) MarkerKind {
    const eol = lineEnd(source, at);
    const line = std.mem.trimEnd(u8, source[at..eol], "\r\n");
    if (std.mem.eql(u8, line, "---")) return .start;
    if (std.mem.startsWith(u8, line, "--- ") or std.mem.startsWith(u8, line, "---\t")) return .start;
    if (std.mem.eql(u8, std.mem.trimEnd(u8, line, " \t"), "...")) return .end;
    return .none;
}

/// True if every content line of `slice` (ignoring blanks and comments) is a
/// column-0 directive (`%…`), and there is at least one. Such a pre-`---`
/// segment is a directives prefix that belongs to the following document, not a
/// document of its own.
fn segmentIsDirectives(slice: []const u8) bool {
    var any = false;
    var i: usize = 0;
    while (i < slice.len) {
        const eol = lineEnd(slice, i);
        const line = std.mem.trimEnd(u8, slice[i..eol], "\r\n");
        i = eol;
        if (line.len == 0) continue;
        if (line[0] == '%') {
            any = true; // a directive sits at column 0
            continue;
        }
        const body = std.mem.trim(u8, line, " \t");
        if (body.len == 0 or body[0] == '#') continue; // blank or comment
        return false; // some other content — not a pure directives prefix
    }
    return any;
}

/// True if `slice` has any line that is neither blank nor a comment.
fn hasContent(slice: []const u8) bool {
    var i: usize = 0;
    while (i < slice.len) {
        const eol = lineEnd(slice, i);
        const trimmed = std.mem.trim(u8, slice[i..eol], " \t\r\n");
        if (trimmed.len != 0 and trimmed[0] != '#') return true;
        i = eol;
    }
    return false;
}

fn lineEnd(source: []const u8, from: usize) usize {
    return if (std.mem.findScalarPos(u8, source, from, '\n')) |nl| nl + 1 else source.len;
}

// =======
// TESTS
// =======

fn rootKind(doc: Document) AST.Node.Kind {
    return doc.ast.nodes[doc.ast.root].kind;
}

test "stream: two explicit documents in a stream (JHB9)" {
    const src =
        \\# Ranking of 1998 home runs
        \\---
        \\- Mark McGwire
        \\- Sammy Sosa
        \\
        \\# Team ranking
        \\---
        \\- Chicago Cubs
        \\- St Louis Cardinals
        \\
    ;
    const stream = try split(testing.allocator, src);
    defer stream.deinit(testing.allocator);

    // The leading comment-only segment is not a document.
    try testing.expectEqual(@as(usize, 2), stream.documents.len);
    try testing.expect(stream.documents[0].explicit);
    try testing.expect(stream.documents[1].explicit);
    try testing.expectEqualSlices(u8, "Mark McGwire", (try stream.documents[0].document.ast.getValByPath(&.{.{ .index = 0 }})).kind.string);
    try testing.expectEqualSlices(u8, "St Louis Cardinals", (try stream.documents[1].document.ast.getValByPath(&.{.{ .index = 1 }})).kind.string);
}

test "stream: directives fold into the following document (6ZKB-shaped)" {
    // Each `%…` prefix belongs to the `---` it introduces, not a document of its
    // own: `Document` is doc 1, the empty `---` is doc 2, and `%YAML 1.2\n---\n…`
    // is doc 3.
    const src =
        \\Document
        \\---
        \\# Empty
        \\...
        \\%YAML 1.2
        \\---
        \\matches %: 20
        \\
    ;
    const stream = try split(testing.allocator, src);
    defer stream.deinit(testing.allocator);
    try testing.expectEqual(@as(usize, 3), stream.documents.len);
    try testing.expectEqualSlices(u8, "Document", rootKind(stream.documents[0].document).string);
    try testing.expect(rootKind(stream.documents[1].document) == .null_);
    try testing.expectEqualSlices(u8, "20", (try stream.documents[2].document.ast.getValByPath(&.{.{ .key = "matches %" }})).kind.number.raw);
}

test "stream: a tag handle scoped to the first document fails later use (QLJ7)" {
    // `!prefix!` is declared only for the first document; documents 2 and 3 use
    // it undeclared, so the splitter must reject the stream.
    const src =
        \\%TAG !prefix! tag:example.com,2011:
        \\--- !prefix!A
        \\a: b
        \\--- !prefix!B
        \\c: d
        \\
    ;
    try testing.expectError(error.UndefinedTagHandle, split(testing.allocator, src));
}

test "stream: two document start markers yields two null docs (6XDY)" {
    const stream = try split(testing.allocator, "---\n---\n");
    defer stream.deinit(testing.allocator);
    try testing.expectEqual(@as(usize, 2), stream.documents.len);
    try testing.expect(rootKind(stream.documents[0].document) == .null_);
    try testing.expect(rootKind(stream.documents[1].document) == .null_);
}

test "stream: document start on last line (PUW8)" {
    const stream = try split(testing.allocator, "---\na: b\n---\n");
    defer stream.deinit(testing.allocator);
    try testing.expectEqual(@as(usize, 2), stream.documents.len);
    try testing.expectEqualSlices(u8, "b", (try stream.documents[0].document.ast.getValByPath(&.{.{ .key = "a" }})).kind.string);
    try testing.expect(rootKind(stream.documents[1].document) == .null_);
}

test "stream: bare docs separated by ... with a comment-only segment (M7A3)" {
    const src = "Bare\ndocument\n...\n# No document\n...\n|\n  %!PS-Adobe-2.0 # Not the first line\n";
    const stream = try split(testing.allocator, src);
    defer stream.deinit(testing.allocator);
    // The `# No document` segment is comment-only and produces no document.
    try testing.expectEqual(@as(usize, 2), stream.documents.len);
    try testing.expect(!stream.documents[0].explicit);
    try testing.expectEqualSlices(u8, "Bare document", rootKind(stream.documents[0].document).string);
}

test "stream: inline content on the marker line (L383)" {
    const stream = try split(testing.allocator, "--- foo  # comment\n--- foo  # comment\n");
    defer stream.deinit(testing.allocator);
    try testing.expectEqual(@as(usize, 2), stream.documents.len);
    try testing.expectEqualSlices(u8, "foo", rootKind(stream.documents[0].document).string);
    try testing.expectEqualSlices(u8, "foo", rootKind(stream.documents[1].document).string);
}

test "stream: explicit doc then bare doc after ... (7Z25)" {
    const stream = try split(testing.allocator, "---\nscalar1\n...\nkey: value\n");
    defer stream.deinit(testing.allocator);
    try testing.expectEqual(@as(usize, 2), stream.documents.len);
    try testing.expect(stream.documents[0].explicit);
    try testing.expectEqualSlices(u8, "scalar1", rootKind(stream.documents[0].document).string);
    try testing.expect(!stream.documents[1].explicit);
    try testing.expectEqualSlices(u8, "value", (try stream.documents[1].document.ast.getValByPath(&.{.{ .key = "key" }})).kind.string);
}

test "stream: single bare document" {
    const stream = try split(testing.allocator, "key: value\n");
    defer stream.deinit(testing.allocator);
    try testing.expectEqual(@as(usize, 1), stream.documents.len);
    try testing.expect(!stream.documents[0].explicit);
}

test "stream: empty stream is one null document" {
    const stream = try split(testing.allocator, "");
    defer stream.deinit(testing.allocator);
    try testing.expectEqual(@as(usize, 1), stream.documents.len);
    try testing.expect(rootKind(stream.documents[0].document) == .null_);
}

test "stream: outerSpan lifts node spans into outer coordinates" {
    const src = "---\nfoo\n---\nbar\n";
    const stream = try split(testing.allocator, src);
    defer stream.deinit(testing.allocator);
    try testing.expectEqual(@as(usize, 2), stream.documents.len);
    const d1 = stream.documents[1];
    const node = d1.document.ast.nodes[d1.document.ast.root];
    const outer = d1.outerSpan(d1.document.span(node));
    try testing.expectEqualSlices(u8, "bar", src[outer.start..outer.end]);
}
