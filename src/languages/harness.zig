//! The per-format harness: the checks that hold for ANY format, run over
//! every compiled-in dialect of the registry.
//!
//! Each language has its own tests — its parser's, its printer's, its
//! editor helper's — and the ones with a vendored corpus have a
//! `conformance.zig` that scores it. What none of that covers is the set of
//! properties the *engine* relies on every format having, which used to be
//! stated in prose and checked by whichever language's tests happened to
//! exercise them. This file states them once and runs them over
//! `Language.dialects`:
//!
//!   * every `empty_doc_seed` parses (it is what `set` writes to a file that
//!     does not exist yet, so a seed that does not parse strands the user
//!     with an empty file that `set` then refuses);
//!   * every sample a format declares parses, prints, and reparses to the
//!     same tree — the round trip the CLI's `fmt` and every cross-format
//!     `convert` assume;
//!   * `Document.node_regions` is well-formed — whole lines, container nodes,
//!     sorted — and a section format's parser actually fills it (the check
//!     `derived-regions.md` §10 asked for: `validate` cannot see inside a
//!     parser, so this is where a parser that stopped recording headers is
//!     caught);
//!   * `Editor` constructs over every sample of an editable format, and a
//!     no-op splice leaves the source untouched and the document parsed.
//!
//! A format opts in by declaring `samples` — an optional `Language` decl
//! (`Decls.optional` in language.zig): a few small documents in its own
//! grammar that exercise a mapping, a sequence where the format has one,
//! and a section where the format has those. The seeds need no opt-in; they
//! are on the registry row already. The corpora under `testdata/` are
//! deliberately not walked here: each is shaped by the suite that vendored
//! it (accept/reject directories, a single `tests.json`, per-version trees)
//! and its `conformance.zig` is the reader that knows the shape.
//!
//! The registry's own invariants — the list ↔ slot pairing, unique ABI
//! values and sniff ranks, the pinned probe order — live in `language.zig`
//! beside the tables they check, not here.

const std = @import("std");
const Language = @import("language.zig");
const AST = @import("../ast/ast.zig");
const Document = @import("../document.zig");
const Span = @import("../util/span.zig");
const Editor = @import("../editor.zig").Editor;

const testing = std.testing;

/// Parse `input` as dialect `d` through the language's own entry point.
fn parseWith(comptime d: anytype, allocator: std.mem.Allocator, input: []const u8) !Document {
    var parser = d.Lang.Parser{ .allocator = allocator };
    return d.Lang.parse(&parser, input, d.dialect);
}

/// Print `ast` in dialect `d` through the serializer's registry dispatch —
/// the same path the CLI takes — into an owned buffer.
fn printAs(comptime d: anytype, allocator: std.mem.Allocator, ast: *const AST) ![]u8 {
    var out: std.Io.Writer.Allocating = .init(allocator);
    defer out.deinit();
    try ast.serializeWith(&out.writer, @field(AST.SerializeFormat, d.name), .{});
    return allocator.dupe(u8, out.written());
}

/// The tree's canonical encoding — the AST's own 1:1 text form, compiled
/// into every test build — which is how two trees are compared here.
/// `AST.eql` compares node arrays positionally, and node ids depend on the
/// order a parser met the nodes in: TOML prints a short `[table]` as an
/// inline table, whose reparse numbers the same tree differently, so a
/// positional comparison would call an identical tree changed.
fn canonicalOf(allocator: std.mem.Allocator, ast: *const AST) ![]u8 {
    var out: std.Io.Writer.Allocating = .init(allocator);
    defer out.deinit();
    try ast.serializeWith(&out.writer, .canonical, .{});
    return allocator.dupe(u8, out.written());
}

/// Parse, print, reparse; the two trees must be the same tree. On a
/// mismatch the sample and what it printed as are shown, since that is the
/// whole diagnosis.
fn expectRoundTrip(comptime d: anytype, allocator: std.mem.Allocator, sample: []const u8) !void {
    const first = try parseWith(d, allocator, sample);
    defer first.deinit(allocator);
    if (comptime !d.Lang.caps.serialize) return;
    const printed = try printAs(d, allocator, &first.ast);
    defer allocator.free(printed);
    const second = parseWith(d, allocator, printed) catch |err| {
        std.debug.print("\n{s}: printed output does not parse ({s})\n--- sample ---\n{s}\n--- printed ---\n{s}\n", .{ d.name, @errorName(err), sample, printed });
        return err;
    };
    defer second.deinit(allocator);
    const a = try canonicalOf(allocator, &first.ast);
    defer allocator.free(a);
    const b = try canonicalOf(allocator, &second.ast);
    defer allocator.free(b);
    if (!std.mem.eql(u8, a, b)) {
        std.debug.print("\n{s}: print → reparse changed the tree\n--- sample ---\n{s}\n--- printed ---\n{s}\n--- canonical before ---\n{s}\n--- canonical after ---\n{s}\n", .{ d.name, sample, printed, a, b });
        return error.RoundTripChangedTree;
    }
}

/// `Document.node_regions` well-formedness: each row is a whole physical
/// line of a container node, and the table is sorted by `(node_id, start)`.
fn expectRegionsWellFormed(doc: Document, source: []const u8) !void {
    var prev: ?Document.NodeRegion = null;
    for (doc.node_regions) |r| {
        try testing.expect(r.node_id < doc.ast.nodes.len);
        const kind = doc.ast.nodes[r.node_id].kind;
        try testing.expect(kind == .mapping or kind == .sequence);
        try testing.expect(r.start < r.end and r.end <= source.len);
        try testing.expect(r.start == 0 or source[r.start - 1] == '\n');
        try testing.expect(r.end == source.len or source[r.end - 1] == '\n');
        if (prev) |p| {
            try testing.expect(p.node_id < r.node_id or (p.node_id == r.node_id and p.start < r.start));
        }
        prev = r;
    }
}

/// Whether some dialect of `Lang` declares itself a section format.
fn isSectionFormat(comptime Lang: type) bool {
    comptime {
        if (!Lang.caps.edit) return false;
        for (std.meta.tags(Lang.Type)) |t| {
            if (Lang.syntax(t).section_noun != null) return true;
        }
        return false;
    }
}

test "harness: every empty_doc_seed parses and round-trips" {
    inline for (Language.dialects) |d| {
        if (comptime d.Lang != void) {
            if (comptime d.empty_doc_seed) |seed| {
                try expectRoundTrip(d, testing.allocator, seed);
            }
        }
    }
}

test "harness: every declared sample parses, prints, and reparses to the same tree" {
    inline for (Language.dialects) |d| {
        if (comptime d.Lang != void and @hasDecl(d.Lang, "samples")) {
            for (d.Lang.samples) |sample| try expectRoundTrip(d, testing.allocator, sample);
        }
    }
}

test "harness: node_regions is well-formed, and a section format's parser fills it" {
    inline for (Language.dialects) |d| {
        if (comptime d.Lang != void and @hasDecl(d.Lang, "samples")) {
            var any_regions = false;
            for (d.Lang.samples) |sample| {
                const doc = try parseWith(d, testing.allocator, sample);
                defer doc.deinit(testing.allocator);
                try expectRegionsWellFormed(doc, sample);
                if (doc.node_regions.len > 0) any_regions = true;
            }
            if (comptime isSectionFormat(d.Lang)) {
                // A section format whose samples produce no section is either
                // a parser that stopped recording headers or a sample set
                // that never opens one; both are the format's to fix.
                if (!any_regions) {
                    std.debug.print("\n{s}: declares section_noun but no sample produced a node_regions row\n", .{d.name});
                    return error.SectionFormatRecordsNoRegions;
                }
            } else {
                try testing.expect(!any_regions);
            }
        }
    }
}

test "harness: Editor constructs over every sample, and a no-op splice changes nothing" {
    inline for (Language.dialects) |d| {
        if (comptime d.Lang != void and d.Lang.caps.edit and @hasDecl(d.Lang, "samples")) {
            for (d.Lang.samples) |sample| {
                var ed: Editor(d.Lang) = .{ .allocator = testing.allocator, .format = d.dialect };
                defer ed.deinit();
                try ed.init(sample);
                try ed.replaceAtSpan(Span.init(0, 0), "");
                try testing.expectEqualStrings(sample, ed.source.items);
                _ = try ed.getParsed();
            }
        }
    }
}
