//! The figl language as a C library of its own, `libfig_figl.a`: the one export
//! `fig_figl_register` (`include/fig/figl.h`), which registers the language
//! with the fig it is linked beside.

comptime {
    _ = @import("kit").c_language.Exports(@import("figl.zig").Language, "figl");
}
