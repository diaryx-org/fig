//! Java .properties as a C library of its own, `libfig_properties.a`: the one export
//! `fig_properties_register` (`include/fig/properties.h`), which registers the
//! language with the fig it is linked beside.

comptime {
    _ = @import("kit").c_language.Exports(@import("properties.zig").Language, "properties");
}
