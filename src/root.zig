//! By convention, root.zig is the root source file when making a package.
const build_options = @import("build_options");
/// The Zig language contract: what a Zig language module declares, and the
/// comptime check that holds it to it.
pub const Language = @import("languages/language.zig");
/// A format resolved when the program runs: the contract as a vtable, the
/// registry, and the `Language` over it. `docs/proposals/runtime-languages.md`.
pub const Runtime = @import("languages/runtime.zig");
/// The languages fig ships, as a consumer registers them.
pub const bundle = @import("languages/bundle.zig");
/// The helper wire — the contract as JSON-RPC 2.0 by the line — as a vtable
/// over any transport: the CLI's child process, the wasm module's host call.
pub const Wire = @import("languages/wire.zig");
/// A runtime language served by the wasm module's host (`fig_host.call`).
pub const Host = @import("languages/host.zig");

pub const Editor = @import("editor.zig").Editor;
/// Structure-aware patching: merge one document into another through the
/// editor's span splices, so everything the patch does not name stays
/// byte-identical. See its module doc for the merge rule and what it refuses.
pub const Patch = @import("patch.zig");
pub const Document = @import("kit/document.zig");
/// A `[start, end)` byte range — a node's place in its source.
pub const Span = @import("kit/span.zig");
pub const AST = @import("kit/ast/ast.zig");
/// How a Zig parser writes the node table a parse hands it: rows in any
/// order, put in pre-order by `finish`.
pub const RowWriter = @import("kit/row_writer.zig");
/// Rendering a tree as canonical or through a registered language.
pub const Serialize = @import("serialize.zig");
pub const Lossless = @import("lossless.zig");
/// Collapsing a document's reference layer — aliases, merges, tags, anchors —
/// into a core AST, when it leaves a language that carries one for a
/// language that does not. See its module doc.
pub const Materialize = @import("materialize.zig");
/// Lossy-mode stripping for INI/dotenv/`.properties` — `Lossless`'s sibling
/// for the three flat/shallow-only formats, whose capability model is
/// depth-based rather than scalar-kind-based. See its module doc.
pub const FlatStrip = @import("flat_strip.zig");
/// Serialization diagnostics: report what a cross-format conversion would lose.
pub const Diagnostics = @import("diagnostics.zig");
/// Shared parse-diagnostic rendering (byte-offset → line/col, the
/// `file:line:col: label: message` report, the language-agnostic `Rendered`
/// shape). Each language keeps its own error/warning codes and teaching
/// messages; only the offset/rendering machinery is shared — see the module
/// doc comment.
pub const ParseDiagnostic = @import("kit/parse_diagnostic.zig");
/// The canonical form: the AST's own 1:1, total, bijective text encoding — the
/// comparison oracle and lossless serialization.
pub const Canonical = @import("canonical/canonical.zig");

/// Reflection-based deserialization into native Zig types (à la `std.json`).
pub const deserialize = @import("deserialize.zig");

test {
    // `bundle.zig`'s test block references every language module, and each
    // module's `test {}` block pulls in its submodules' tests, so root names
    // no language.
    _ = @import("languages/bundle.zig");
    _ = @import("languages/language.zig");
    _ = @import("languages/harness.zig");
    _ = @import("languages/runtime.zig");
    _ = @import("languages/wire.zig");
    _ = @import("kit/flat_map.zig");
    _ = @import("kit/document.zig");
    _ = @import("editor.zig");
    _ = @import("editor/regions.zig");
    _ = @import("patch.zig");
    _ = @import("lossless.zig");
    _ = @import("materialize.zig");
    _ = @import("flat_strip.zig");
    _ = @import("diagnostics.zig");
    _ = @import("kit/parse_diagnostic.zig");
    _ = @import("canonical/canonical.zig");
    _ = @import("deserialize.zig");
    _ = @import("c_api.zig");
    _ = @import("kit/kit.zig");
    _ = @import("languages/contract_test.zig");
    // The fuzz targets double as deterministic smoke tests under a plain
    // `zig build test` (Zig runs each one over an empty Smith tape when the
    // binary is not built in fuzz mode), so they should never need a flag to
    // be compiled or discovered.
    _ = @import("fuzz.zig");
}
