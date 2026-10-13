//! The XML property list as a WebAssembly module of its own, `fig-plist.wasm`: the exports
//! the TypeScript binding registers it into fig's module through.

comptime {
    _ = @import("kit").wasm_language.Exports(@import("plist.zig").Language);
}
