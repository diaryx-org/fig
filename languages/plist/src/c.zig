//! The XML property list as a C library of its own, `libfig_plist.a`: the one
//! export `fig_plist_register` (`include/fig/plist.h`), which registers the
//! language with the fig it is linked beside.

comptime {
    _ = @import("kit").c_language.Exports(@import("plist.zig").Language, "plist");
}
