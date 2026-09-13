//! By convention, root.zig is the root source file when making a package.
const build_options = @import("build_options");
pub const Language = @import("languages/language.zig");
/// A format resolved when the program runs: the contract as a vtable, the
/// registry, and the `Language` over it. `docs/proposals/runtime-languages.md`.
pub const Runtime = @import("languages/runtime.zig");
/// The helper wire — the contract as newline-delimited JSON — as a vtable
/// over any transport: the CLI's child process, the wasm module's host call.
pub const Wire = @import("languages/wire.zig");
// TODO: Language.detect(file: []const u8);

pub const Editor = @import("editor.zig").Editor;
/// Structure-aware patching: merge one document into another through the
/// editor's span splices, so everything the patch does not name stays
/// byte-identical. See its module doc for the merge rule and what it refuses.
pub const Patch = @import("patch.zig");
pub const Document = @import("document.zig");
/// A `[start, end)` byte range — what `Document`'s span tables hold.
pub const Span = @import("util/span.zig");
pub const AST = @import("ast/ast.zig");
pub const Embed = @import("embed.zig");
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
pub const ParseDiagnostic = @import("parse_diagnostic.zig");
/// The canonical form: the AST's own 1:1, total, bijective text encoding — the
/// comparison oracle and lossless serialization.
pub const Canonical = @import("canonical/canonical.zig");
/// The fig authoring dialect: the human-facing, hand-writable surface over the
/// same AST. Reader + `fig fmt` printer; see src/languages/fig/DESIGN.md.
pub const Fig = @import("languages/fig/fig.zig");

/// Reflection-based deserialization into native Zig types (à la `std.json`).
pub const deserialize = @import("deserialize.zig");

test {
    // `language.zig`'s own test block references every language module in
    // its `slots`, and each module's `test {}` block pulls in its submodules'
    // tests, so root names no language. Build-option-gated conformance
    // suites stay enumerated below: each is a file, and a file is imported by
    // literal.
    _ = @import("languages/language.zig");
    _ = @import("languages/harness.zig");
    _ = @import("languages/runtime.zig");
    _ = @import("languages/wire.zig");
    _ = @import("languages/shared/flat_map.zig");
    _ = @import("document.zig");
    _ = @import("editor.zig");
    _ = @import("editor/regions.zig");
    _ = @import("patch.zig");
    _ = @import("embed.zig");
    _ = @import("lossless.zig");
    _ = @import("materialize.zig");
    _ = @import("flat_strip.zig");
    _ = @import("diagnostics.zig");
    _ = @import("parse_diagnostic.zig");
    _ = @import("canonical/canonical.zig");
    _ = @import("deserialize.zig");
    _ = @import("c_api.zig");
    _ = @import("util/util.zig");
    // Unconditional, unlike the gated suites below: the fuzz targets double as
    // deterministic smoke tests under a plain `zig build test` (Zig runs each
    // one over an empty Smith tape when the binary is not built in fuzz mode),
    // so they should never need a flag to be compiled or discovered.
    _ = @import("fuzz.zig");
    if (build_options.json_conformance) {
        _ = @import("languages/json/conformance.zig");
    }
    if (build_options.json5_conformance) {
        _ = @import("languages/json/json5_conformance.zig");
    }
    if (build_options.yaml_conformance) {
        _ = @import("languages/yaml/conformance.zig");
        _ = @import("languages/yaml/conformance_1_1.zig");
    }
    if (build_options.toml_conformance) {
        _ = @import("languages/toml/conformance.zig");
    }
    if (build_options.plist_conformance) {
        _ = @import("languages/plist/conformance.zig");
    }
    if (build_options.nestedtext_conformance) {
        _ = @import("languages/nestedtext/conformance.zig");
    }
}
