//! INI as a C library of its own, `libfig_ini.a`: the one export
//! `fig_ini_register` (`include/fig/ini.h`), which registers the language with
//! the fig it is linked beside.

pub const panic = @import("kit").c_language.panic;

comptime {
    _ = @import("kit").c_language.Exports(@import("ini.zig").Language, "ini");
}
