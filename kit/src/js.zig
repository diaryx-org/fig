//! The kit's rules as a WebAssembly module of their own, `fig-kit.wasm`,
//! for the JavaScript kit: `@diaryx/fig/kit/number` and `/datetime` answer
//! through it (`kit/zig.js` in the TypeScript binding), so a language
//! written in JavaScript spells a number and reads a datetime by the same
//! code a Zig language does. The functions are `rules.zig`'s, under the
//! names `fig.h` gives them.
//!
//! The module imports nothing, allocates nothing and keeps no state. The
//! caller writes its input from `fig_kit_heap()` on, growing the memory to
//! hold it and an answer behind it.

const rules = @import("rules.zig");

extern const __heap_base: u8;

comptime {
    rules.exportAll();
    @export(&heap, .{ .name = "fig_kit_heap" });
}

/// The first byte no static data holds, where the caller's input goes.
fn heap() callconv(.c) usize {
    return @intFromPtr(&__heap_base);
}
