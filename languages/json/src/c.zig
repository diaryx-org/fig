//! JSON, JSONC and JSON5 as a C library of its own, `libfig_json.a`: the one
//! export `fig_json_register` (`include/fig/json.h`), which registers the
//! language with the fig it is linked beside.

comptime {
    _ = @import("kit").c_language.Exports(@import("json.zig").Language, "json");
}
