//! TOML as a C library of its own, `libfig_toml.a`: the one export
//! `fig_toml_register` (`include/fig/toml.h`), which registers the
//! language with the fig it is linked beside.

comptime {
    _ = @import("kit").c_language.Exports(@import("toml.zig").Language, "toml");
}
