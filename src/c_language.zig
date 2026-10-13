//! The root of a language's own static library: the language
//! `c_language_options.name` names, with the export `kit/c_language.zig`
//! gives it. `zig build c-languages` builds one library per language,
//! `libfig_<name>.a`.

const options = @import("c_language_options");

const modules = .{
    .json = @import("languages/json/json.zig"),
    .yaml = @import("languages/yaml/yaml.zig"),
    .toml = @import("languages/toml/toml.zig"),
    .zon = @import("languages/zon/zon.zig"),
    .figl = @import("languages/figl/figl.zig"),
    .ini = @import("languages/ini/ini.zig"),
    .dotenv = @import("languages/dotenv/dotenv.zig"),
    .properties = @import("languages/properties/properties.zig"),
    .plist = @import("languages/plist/plist.zig"),
    .nestedtext = @import("languages/nestedtext/nestedtext.zig"),
};

comptime {
    _ = @import("kit/c_language.zig").Exports(@field(modules, options.name).Language, options.name);
}
