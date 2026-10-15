//! The test surface: the unit-test suite (`test`), the same tests with every
//! format compiled in (`all-on`), coverage-guided fuzzing (`fuzz`), and
//! installing the test binaries for debugging (`install-tests`). Returns the two
//! step handles the `check` gate folds in.
//!
//! The conformance corpora (yaml-test-suite, toml-test and the rest) are not
//! here: they live in fig-conformance, which scores any implementation of a
//! language through the contract.

const std = @import("std");
const Context = @import("Context.zig");
const Options = @import("Options.zig");
const artifacts = @import("artifacts.zig");
const list = @import("../contract/list.zig");

/// The step handles `checks` wires into the `check` gate.
pub const Result = struct {
    test_step: *std.Build.Step,
    all_on_step: *std.Build.Step,
};

pub fn add(ctx: Context, arts: artifacts.Result) Result {
    const b = ctx.b;
    const target = ctx.target;
    const resolved_target = target.result;
    const mod = arts.fig_mod;
    const exe = arts.exe;

    const test_filters =
        b.option([]const []const u8, "test-filter", "Only run tests matching this filter") orelse &.{};

    const mod_tests = b.addTest(.{
        .root_module = mod,
        .filters = test_filters,
    });
    // `mod` already carries `build_options` (added in artifacts.zig); the test
    // artifact reuses that module, so no second `addOptions` is needed here.

    const install_mod_tests = b.addInstallArtifact(mod_tests, .{
        .dest_sub_path = "fig-tests",
    });

    // A run step that will run the test executable.
    const run_mod_tests = b.addRunArtifact(mod_tests);
    // The kit fixture tests read `kit/testdata/` from the build root.
    run_mod_tests.setCwd(b.path("."));

    const exe_tests = b.addTest(.{
        .root_module = exe.root_module,
        .filters = test_filters,
    });

    const run_exe_tests = b.addRunArtifact(exe_tests);

    // The version sync's scanners (tools/version-sync.zig). It lives out in
    // tools/ rather than src/, but it splices version strings into manifests by
    // hand, and a scanner that takes the wrong line is only visible once a
    // release has written it — so its `test` blocks run with everything else.
    // `addTest` compiles the file for those blocks; the tool's `main` is not run.
    const version_sync_tests = b.addTest(.{
        .root_module = b.createModule(.{
            .root_source_file = b.path("tools/version-sync.zig"),
            .target = target,
            .optimize = ctx.optimize,
            .imports = &.{
                .{ .name = "languages", .module = b.createModule(.{ .root_source_file = b.path("src/contract/list.zig") }) },
            },
        }),
        .filters = test_filters,
    });
    const run_version_sync_tests = b.addRunArtifact(version_sync_tests);

    // The kit's own tests: a package of its own, so its tests are a
    // compilation of their own.
    const kit_tests = b.addTest(.{
        .root_module = b.createModule(.{
            .root_source_file = ctx.kit_dep.path("src/kit.zig"),
            .target = target,
            .optimize = ctx.optimize,
        }),
        .filters = test_filters,
    });
    const run_kit_tests = b.addRunArtifact(kit_tests);

    const test_step = b.step("test", "Run tests");
    test_step.dependOn(&run_kit_tests.step);

    // Each language's own tests: its package's `test` step, which needs
    // nothing of fig's.
    for (ctx.languages) |dep| {
        test_step.dependOn(&dep.builder.top_level_steps.get("test").?.step);
    }

    // Each language's editor tests (`languages/<name>/tests/editor.zig`):
    // fig's editor over the language. A language's package cannot depend
    // on fig, which depends on it, so fig's build compiles them, with the
    // language's module and fig's.
    for (list.rows, ctx.languages) |row, dep| {
        dep.builder.root.access(b.graph.io, "tests/editor.zig", .{}) catch continue;
        const editor_tests = b.addTest(.{
            .name = b.fmt("{s}-editor", .{row.name}),
            .root_module = b.createModule(.{
                .root_source_file = dep.path("tests/editor.zig"),
                .target = target,
                .optimize = ctx.optimize,
                .imports = &.{.{ .name = "fig", .module = mod }},
            }),
            .filters = test_filters,
        });
        ctx.importKit(editor_tests.root_module);
        ctx.importLanguages(editor_tests.root_module);
        test_step.dependOn(&b.addRunArtifact(editor_tests).step);
    }
    test_step.dependOn(&run_mod_tests.step);
    test_step.dependOn(&run_exe_tests.step);
    test_step.dependOn(&run_version_sync_tests.step);

    // The library's tests again with every format compiled in (see
    // `Options.BuildOptions.all_on`). Options are baked into a module at
    // configure time, so this needs a *variant* of `mod` carrying different
    // options, not a flag on `mod_tests`. Hence the second module and the
    // second `addOptions` instance (a second instance is also what keeps Zig
    // from rejecting one generated file shared by two modules — see
    // `options_mod` in artifacts.zig).
    const all_on_options_mod = Options.addFigOptions(b, Options.BuildOptions.all_on, ctx.ver).createModule();
    const all_on_mod = b.createModule(.{
        .root_source_file = b.path("src/root.zig"),
        .target = target,
    });
    all_on_mod.addImport("build_options", all_on_options_mod);
    ctx.importKit(all_on_mod);
    ctx.importLanguages(all_on_mod);
    ctx.importWire(all_on_mod);
    // Same libc reasoning as `mod` in artifacts.zig: root.zig's test block pulls
    // in c_api.zig, which reaches for `std.heap.c_allocator` on every non-wasm
    // target.
    all_on_mod.link_libc = !resolved_target.cpu.arch.isWasm();

    const all_on_lib_tests = b.addTest(.{
        .root_module = all_on_mod,
        .filters = test_filters,
    });
    const run_all_on_lib_tests = b.addRunArtifact(all_on_lib_tests);

    // The CLI's test root, built against the everything-on library. The
    // variant above is the one build that turns every format on,
    // but on its own it proves only that the LIBRARY compiles that way: the
    // CLI instantiates the generic engines (patch.zig, the editor) for every
    // compiled-in language, and a comptime error there — a `switch` arm naming
    // an error a plist-hooked op cannot return — was invisible to every
    // default build, where plist is gated off. Running the CLI's tests here
    // too makes `zig build check` the gate for the whole everything-on
    // configuration, not just the library's half of it.
    const all_on_exe_tests = b.addTest(.{
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/cli/main.zig"),
            .target = target,
            .optimize = ctx.optimize,
            .imports = &.{
                .{ .name = "fig", .module = all_on_mod },
                .{ .name = "build_options", .module = all_on_options_mod },
            },
        }),
        .filters = test_filters,
    });
    const run_all_on_exe_tests = b.addRunArtifact(all_on_exe_tests);

    const all_on_step = b.step("all-on", "Run the library's and the CLI's tests with every format compiled in");
    all_on_step.dependOn(&run_all_on_lib_tests.step);
    all_on_step.dependOn(&run_all_on_exe_tests.step);

    // Coverage-guided fuzzing for the hand-written tokenizers/parsers — the
    // targets themselves live in src/fuzz.zig. Conformance proves fig handles the
    // inputs somebody thought to write down; this is what goes looking for the
    // ones nobody did.
    //
    // Two things about `--fuzz` are worth knowing before touching this:
    //
    //   1. It is a build-RUNNER flag, not something build.zig can turn on. So the
    //      step alone does not fuzz — `zig build fuzz` just runs the targets once.
    //      To actually fuzz: `zig build fuzz --fuzz=1M`.
    //   2. Its argument is an ITERATION CAP, not a time budget, and omitting it
    //      implies `--webui` — a web server that runs until interrupted. That is
    //      lovely locally (live coverage view) and a hang in CI, which is why
    //      .github/workflows/fuzz.yml always passes an explicit `=<n>`.
    //
    // This uses the stock test runner. Zig 0.16.0 needed a vendored, patched
    // copy here because its runner's fuzz path did not compile; 0.17.0 fixed
    // that upstream, and the copy went with the move to 0.17.
    const fuzz_tests = b.addTest(.{
        .root_module = mod,
        // Default to the src/fuzz.zig targets rather than the whole suite — under
        // `--fuzz` every non-fuzz test would otherwise be rebuilt with coverage
        // instrumentation for nothing. An explicit `-Dtest-filter` still wins, so
        // a single target can be driven on its own while working on a parser.
        .filters = if (test_filters.len > 0) test_filters else &.{"fuzz"},
    });
    const run_fuzz_tests = b.addRunArtifact(fuzz_tests);

    const fuzz_step = b.step("fuzz", "Fuzz the hand-written parsers — needs the runner flag too, e.g. `zig build fuzz --fuzz=1M`");
    fuzz_step.dependOn(&run_fuzz_tests.step);

    // Deliberately NOT wired into `check`. Fuzzing has no natural stopping point,
    // so it has no business on a release gate or the PR critical path; the nightly
    // workflow is where it runs for real. The targets still get smoke-tested on
    // every `zig build test` — src/root.zig imports src/fuzz.zig unconditionally,
    // so a target that stops compiling, leaks, or has its property broken fails
    // CI the normal way, and they cannot rot unnoticed between nightly runs.

    const install_tests_step = b.step("install-tests", "Install test executables for debugging");
    install_tests_step.dependOn(&install_mod_tests.step);

    return .{ .test_step = test_step, .all_on_step = all_on_step };
}
