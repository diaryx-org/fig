//! Java .properties as a WebAssembly module of its own, `fig-properties.wasm`: the exports
//! the TypeScript binding registers it into fig's module through.

comptime {
    _ = @import("kit").wasm_language.Exports(@import("properties.zig").Language);
}
