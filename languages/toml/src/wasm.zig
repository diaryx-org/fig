//! TOML as a WebAssembly module of its own, `fig-toml.wasm`: the exports the
//! TypeScript binding registers it into fig's module through.

comptime {
    _ = @import("kit").wasm_language.Exports(@import("toml.zig").Language);
}
