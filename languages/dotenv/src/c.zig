//! The dotenv language as a C library of its own, `libfig_dotenv.a`: the one
//! export `fig_dotenv_register` (`include/fig/dotenv.h`), which registers the
//! language with the fig it is linked beside.

comptime {
    _ = @import("kit").c_language.Exports(@import("dotenv.zig").Language, "dotenv");
}
