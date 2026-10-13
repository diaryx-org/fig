//! JSON, JSONC and JSON5 as a WebAssembly module of its own, `fig-json.wasm`: the exports
//! the TypeScript binding registers it into fig's module through.

comptime {
    _ = @import("kit").wasm_language.Exports(@import("json.zig").Language);
}
