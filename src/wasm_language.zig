//! The root of a language's own WebAssembly module: the language
//! `wasm_language_options.name` names, with the exports
//! `kit/wasm_language.zig` gives it. `zig build wasm-languages` builds one
//! module per language, `fig-<name>.wasm`.

const options = @import("wasm_language_options");

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
    _ = @import("kit/wasm_language.zig").Exports(@field(modules, options.name).Language);
}
