//! YAML as a C library of its own, `libfig_yaml.a`: the one export
//! `fig_yaml_register` (`include/fig/yaml.h`), which registers the
//! language with the fig it is linked beside.

comptime {
    _ = @import("kit").c_language.Exports(@import("yaml.zig").Language, "yaml");
}
