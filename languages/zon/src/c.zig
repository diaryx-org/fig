//! ZON as a C library of its own, `libfig_zon.a`: the one export
//! `fig_zon_register` (`include/fig/zon.h`), which registers the
//! language with the fig it is linked beside.

comptime {
    _ = @import("kit").c_language.Exports(@import("zon.zig").Language, "zon");
}
