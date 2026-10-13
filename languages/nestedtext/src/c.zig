//! NestedText as a C library of its own, `libfig_nestedtext.a`: the one export
//! `fig_nestedtext_register` (`include/fig/nestedtext.h`), which registers the
//! language with the fig it is linked beside.

comptime {
    _ = @import("kit").c_language.Exports(@import("nestedtext.zig").Language, "nestedtext");
}
