//! INI as a WebAssembly module of its own, `fig-ini.wasm`: the exports
//! the TypeScript binding registers it into fig's module through.

comptime {
    _ = @import("kit").wasm_language.Exports(@import("ini.zig").Language);
}
